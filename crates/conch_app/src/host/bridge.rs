//! Host API bridge — implements the HostApi vtable for native plugins.
//!
//! Uses a global `OnceLock` to hold shared state accessible from the
//! `extern "C"` function pointers that plugins call.

use std::collections::HashMap;
use std::ffi::{c_char, c_void, CStr, CString};
use std::sync::{Arc, OnceLock};

use conch_plugin::bus::PluginBus;
use conch_plugin_sdk::{
    HostApi, OpenSessionResult, PanelHandle, PanelLocation,
    SessionBackendVtable, SessionHandle, SessionMeta,
};
use parking_lot::Mutex;
use tokio::sync::oneshot;

use super::dialogs::{self, DialogRequest, DialogSender};
use super::session_bridge::PluginSessionBridge;

// ---------------------------------------------------------------------------
// Panel Registry
// ---------------------------------------------------------------------------

/// Information about a registered panel.
#[derive(Debug, Clone)]
pub struct PanelInfo {
    pub name: String,
    pub location: PanelLocation,
    pub plugin_name: String,
    pub cached_widgets_json: String,
}

/// Tracks all panels registered by plugins.
pub struct PanelRegistry {
    panels: HashMap<u64, PanelInfo>,
    next_handle: u64,
}

impl PanelRegistry {
    pub fn new() -> Self {
        Self {
            panels: HashMap::new(),
            next_handle: 1,
        }
    }

    pub fn register(
        &mut self,
        location: PanelLocation,
        name: String,
        plugin_name: String,
    ) -> u64 {
        let handle = self.next_handle;
        self.next_handle += 1;
        self.panels.insert(
            handle,
            PanelInfo {
                name,
                location,
                plugin_name,
                cached_widgets_json: "[]".into(),
            },
        );
        handle
    }

    pub fn set_widgets(&mut self, handle: u64, json: String) {
        if let Some(panel) = self.panels.get_mut(&handle) {
            panel.cached_widgets_json = json;
        }
    }

    pub fn remove_by_plugin(&mut self, plugin_name: &str) {
        self.panels.retain(|_, p| p.plugin_name != plugin_name);
    }

    pub fn panels(&self) -> impl Iterator<Item = (u64, &PanelInfo)> {
        self.panels.iter().map(|(&h, p)| (h, p))
    }
}

// ---------------------------------------------------------------------------
// Global bridge state
// ---------------------------------------------------------------------------

struct BridgeInner {
    bus: Arc<PluginBus>,
    panels: Arc<Mutex<PanelRegistry>>,
    dialog_tx: DialogSender,
    session_registry: Arc<Mutex<SessionRegistry>>,
}

/// A status update queued by a plugin for one of its sessions.
pub struct SessionStatusUpdate {
    pub handle: SessionHandle,
    pub status: conch_plugin_sdk::SessionStatus,
    pub detail: Option<String>,
}

/// Registry of pending session open/close requests from plugins.
pub struct SessionRegistry {
    pub pending_open: Vec<PendingSession>,
    pub pending_close: Vec<SessionHandle>,
    pub pending_status: Vec<SessionStatusUpdate>,
    next_handle: u64,
}

/// A session that a plugin wants to open — drained by ConchApp each frame.
pub struct PendingSession {
    pub handle: SessionHandle,
    pub title: String,
    pub bridge: PluginSessionBridge,
    pub vtable: SessionBackendVtable,
    pub backend_handle: *mut c_void,
}

// SAFETY: PendingSession contains raw pointer backend_handle which is only
// accessed through the vtable callbacks (thread-safe by plugin contract).
unsafe impl Send for PendingSession {}

impl SessionRegistry {
    pub fn new() -> Self {
        Self {
            pending_open: Vec::new(),
            pending_close: Vec::new(),
            pending_status: Vec::new(),
            next_handle: 1,
        }
    }

    fn next_handle(&mut self) -> SessionHandle {
        let h = SessionHandle(self.next_handle);
        self.next_handle += 1;
        h
    }
}

static BRIDGE: OnceLock<BridgeInner> = OnceLock::new();

/// Shared theme JSON, updated whenever the app theme changes.
static THEME_JSON: parking_lot::Mutex<String> = parking_lot::Mutex::new(String::new());

/// Update the theme JSON that plugins receive via `get_theme()`.
///
/// Called by the app whenever `theme_dirty` is set.
pub fn update_theme_json(theme: &crate::ui_theme::UiTheme) {
    fn c32(c: egui::Color32) -> String {
        format!("#{:02x}{:02x}{:02x}", c.r(), c.g(), c.b())
    }
    let json = serde_json::json!({
        "bg": c32(theme.bg),
        "surface": c32(theme.surface),
        "surface_raised": c32(theme.surface_raised),
        "text": c32(theme.text),
        "text_secondary": c32(theme.text_secondary),
        "text_muted": c32(theme.text_muted),
        "accent": c32(theme.accent),
        "border": c32(theme.border),
        "warn": c32(theme.warn),
        "error": c32(theme.error),
        "font_small": theme.font_small,
        "font_normal": theme.font_normal,
        "dark_mode": theme.dark_mode,
    });
    *THEME_JSON.lock() = json.to_string();
}

/// Initialise the global bridge state.
///
/// Must be called exactly once before any plugin invokes a `HostApi` function.
/// Typically called during app startup after creating the bus and panel registry.
pub fn init_bridge(
    bus: Arc<PluginBus>,
    panels: Arc<Mutex<PanelRegistry>>,
    dialog_tx: DialogSender,
    session_registry: Arc<Mutex<SessionRegistry>>,
) {
    BRIDGE
        .set(BridgeInner { bus, panels, dialog_tx, session_registry })
        .ok()
        .expect("init_bridge must be called exactly once");
}

/// Build a `HostApi` vtable with all function pointers wired to this bridge.
pub fn build_host_api() -> HostApi {
    HostApi {
        register_panel: host_register_panel,
        set_widgets: host_set_widgets,
        open_session: host_open_session,
        close_session: host_close_session,
        set_session_status: host_set_session_status,
        show_form: host_show_form,
        show_confirm: host_show_confirm,
        show_prompt: host_show_prompt,
        show_alert: host_show_alert,
        show_error: host_show_error,
        notify: host_notify,
        log: host_log,
        publish_event: host_publish_event,
        subscribe: host_subscribe,
        query_plugin: host_query_plugin,
        register_service: host_register_service,
        get_config: host_get_config,
        set_config: host_set_config,
        register_menu_item: host_register_menu_item,
        clipboard_set: host_clipboard_set,
        clipboard_get: host_clipboard_get,
        get_theme: host_get_theme,
        show_context_menu: host_show_context_menu,
        free_string: host_free_string,
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Get the bridge state, panicking if `init_bridge` was never called.
fn bridge() -> &'static BridgeInner {
    BRIDGE
        .get()
        .expect("host bridge not initialised — call init_bridge() first")
}

/// Derive the calling plugin's name from the current thread name.
///
/// Plugin threads are named `"plugin:{name}"`. Returns `"unknown"` if the
/// thread name doesn't follow that convention.
fn current_plugin_name() -> String {
    std::thread::current()
        .name()
        .and_then(|n| n.strip_prefix("plugin:"))
        .unwrap_or("unknown")
        .to_string()
}

/// Safely read a `*const c_char` into a `&str`. Returns `""` on null or
/// invalid UTF-8.
///
/// # Safety
///
/// The pointer must either be null or point to a valid null-terminated C string
/// that remains valid for the duration of the call.
unsafe fn cstr_to_str<'a>(ptr: *const c_char) -> &'a str {
    if ptr.is_null() {
        return "";
    }
    unsafe { CStr::from_ptr(ptr) }.to_str().unwrap_or("")
}

/// Safely read `*const c_char` + `len` as a `&str`. Returns `""` on null or
/// invalid UTF-8.
///
/// # Safety
///
/// The pointer must either be null or point to at least `len` valid bytes.
unsafe fn slice_to_str<'a>(ptr: *const c_char, len: usize) -> &'a str {
    if ptr.is_null() || len == 0 {
        return "";
    }
    let bytes = unsafe { std::slice::from_raw_parts(ptr as *const u8, len) };
    std::str::from_utf8(bytes).unwrap_or("")
}

/// Allocate a host-owned `CString` and return its raw pointer.
///
/// The plugin is responsible for freeing this via `host_free_string`.
fn alloc_cstring(s: &str) -> *mut c_char {
    match CString::new(s) {
        Ok(c) => c.into_raw(),
        Err(_) => std::ptr::null_mut(),
    }
}

// ---------------------------------------------------------------------------
// Panel Management
// ---------------------------------------------------------------------------

extern "C" fn host_register_panel(
    location: PanelLocation,
    name: *const c_char,
    _icon: *const c_char,
) -> PanelHandle {
    let name_str = unsafe { cstr_to_str(name) }.to_string();
    let plugin_name = current_plugin_name();
    log::info!("plugin '{plugin_name}' registering panel '{name_str}' at {location:?}");

    let id = bridge().panels.lock().register(location, name_str, plugin_name);
    PanelHandle(id)
}

extern "C" fn host_set_widgets(handle: PanelHandle, json: *const c_char, len: usize) {
    let json_str = unsafe { slice_to_str(json, len) }.to_string();
    bridge().panels.lock().set_widgets(handle.0, json_str);
}

// ---------------------------------------------------------------------------
// Session Backends
// ---------------------------------------------------------------------------

extern "C" fn host_open_session(
    meta: *const SessionMeta,
    vtable: *const SessionBackendVtable,
    backend_handle: *mut c_void,
) -> OpenSessionResult {
    if meta.is_null() || vtable.is_null() {
        log::error!("host_open_session: null meta or vtable");
        return OpenSessionResult {
            handle: SessionHandle(0),
            output_cb: stub_output_cb,
            output_ctx: std::ptr::null_mut(),
        };
    }

    let b = bridge();
    let meta_ref = unsafe { &*meta };
    let vtable_val = unsafe { *vtable };
    let title = unsafe { cstr_to_str(meta_ref.title) }.to_string();

    let mut registry = b.session_registry.lock();
    let handle = registry.next_handle();

    // Create the bridge — provides term + output callback for the plugin.
    let term_config = alacritty_terminal::term::Config::default();
    let (bridge, output_cb, output_ctx) =
        PluginSessionBridge::new(handle, 80, 24, term_config);

    registry.pending_open.push(PendingSession {
        handle,
        title,
        bridge,
        vtable: vtable_val,
        backend_handle,
    });

    log::info!("host_open_session: queued session {:?}", handle);

    OpenSessionResult {
        handle,
        output_cb,
        output_ctx,
    }
}

extern "C" fn stub_output_cb(_ctx: *mut c_void, _buf: *const u8, _len: usize) {
    // no-op stub
}

extern "C" fn host_close_session(handle: SessionHandle) {
    let b = bridge();
    b.session_registry.lock().pending_close.push(handle);
    log::info!("host_close_session: queued close for {:?}", handle);
}

extern "C" fn host_set_session_status(
    handle: SessionHandle,
    status: conch_plugin_sdk::SessionStatus,
    detail: *const c_char,
) {
    let detail_str = if detail.is_null() {
        None
    } else {
        Some(unsafe { cstr_to_str(detail) }.to_string())
    };

    let b = bridge();
    b.session_registry.lock().pending_status.push(SessionStatusUpdate {
        handle,
        status,
        detail: detail_str,
    });
    log::debug!("host_set_session_status: {:?} -> {:?}", handle, status);
}

// ---------------------------------------------------------------------------
// Dialogs
// ---------------------------------------------------------------------------

extern "C" fn host_show_form(json: *const c_char, len: usize) -> *mut c_char {
    let json_str = unsafe { slice_to_str(json, len) };
    let descriptor = match dialogs::parse_form_descriptor(json_str) {
        Some(d) => d,
        None => {
            log::warn!("host_show_form: invalid form descriptor JSON");
            return std::ptr::null_mut();
        }
    };

    let (reply_tx, reply_rx) = oneshot::channel();
    let _ = bridge().dialog_tx.send(DialogRequest::Form {
        descriptor,
        reply: reply_tx,
    });

    // Block the plugin thread until the UI thread responds.
    match reply_rx.blocking_recv() {
        Ok(Some(result)) => alloc_cstring(&result),
        Ok(None) | Err(_) => std::ptr::null_mut(),
    }
}

extern "C" fn host_show_confirm(msg: *const c_char) -> bool {
    let msg_str = unsafe { cstr_to_str(msg) }.to_string();

    let (reply_tx, reply_rx) = oneshot::channel();
    let _ = bridge().dialog_tx.send(DialogRequest::Confirm {
        msg: msg_str,
        reply: reply_tx,
    });

    reply_rx.blocking_recv().unwrap_or(false)
}

extern "C" fn host_show_prompt(
    msg: *const c_char,
    default_value: *const c_char,
) -> *mut c_char {
    let msg_str = unsafe { cstr_to_str(msg) }.to_string();
    let default = unsafe { cstr_to_str(default_value) }.to_string();

    let (reply_tx, reply_rx) = oneshot::channel();
    let _ = bridge().dialog_tx.send(DialogRequest::Prompt {
        msg: msg_str,
        default_value: default,
        reply: reply_tx,
    });

    match reply_rx.blocking_recv() {
        Ok(Some(result)) => alloc_cstring(&result),
        Ok(None) | Err(_) => std::ptr::null_mut(),
    }
}

extern "C" fn host_show_alert(title: *const c_char, msg: *const c_char) {
    let title_str = unsafe { cstr_to_str(title) }.to_string();
    let msg_str = unsafe { cstr_to_str(msg) }.to_string();

    let (reply_tx, reply_rx) = oneshot::channel();
    let _ = bridge().dialog_tx.send(DialogRequest::Alert {
        title: title_str,
        msg: msg_str,
        reply: reply_tx,
    });

    // Block until user dismisses.
    let _ = reply_rx.blocking_recv();
}

extern "C" fn host_show_error(title: *const c_char, msg: *const c_char) {
    let title_str = unsafe { cstr_to_str(title) }.to_string();
    let msg_str = unsafe { cstr_to_str(msg) }.to_string();

    let (reply_tx, reply_rx) = oneshot::channel();
    let _ = bridge().dialog_tx.send(DialogRequest::Error {
        title: title_str,
        msg: msg_str,
        reply: reply_tx,
    });

    // Block until user dismisses.
    let _ = reply_rx.blocking_recv();
}

// ---------------------------------------------------------------------------
// Notifications & Logging
// ---------------------------------------------------------------------------

extern "C" fn host_notify(json: *const c_char, len: usize) {
    let json_str = unsafe { slice_to_str(json, len) };
    log::info!("plugin notification: {json_str}");
}

extern "C" fn host_log(level: u8, msg: *const c_char) {
    let msg_str = unsafe { cstr_to_str(msg) };
    let plugin = current_plugin_name();
    match level {
        0 => log::trace!("[plugin:{plugin}] {msg_str}"),
        1 => log::debug!("[plugin:{plugin}] {msg_str}"),
        2 => log::info!("[plugin:{plugin}] {msg_str}"),
        3 => log::warn!("[plugin:{plugin}] {msg_str}"),
        _ => log::error!("[plugin:{plugin}] {msg_str}"),
    }
}

// ---------------------------------------------------------------------------
// Plugin IPC (Message Bus)
// ---------------------------------------------------------------------------

extern "C" fn host_publish_event(
    event_type: *const c_char,
    data_json: *const c_char,
    len: usize,
) {
    let event_type_str = unsafe { cstr_to_str(event_type) };
    let data_str = unsafe { slice_to_str(data_json, len) };
    let source = current_plugin_name();

    let data: serde_json::Value =
        serde_json::from_str(data_str).unwrap_or(serde_json::Value::Null);
    bridge().bus.publish(&source, event_type_str, data);
}

extern "C" fn host_subscribe(event_type: *const c_char) {
    let event_type_str = unsafe { cstr_to_str(event_type) };
    let plugin = current_plugin_name();
    log::debug!("plugin '{plugin}' subscribing to '{event_type_str}'");
    bridge().bus.subscribe(&plugin, event_type_str);
}

extern "C" fn host_query_plugin(
    target: *const c_char,
    method: *const c_char,
    args_json: *const c_char,
    len: usize,
) -> *mut c_char {
    let target_str = unsafe { cstr_to_str(target) };
    let method_str = unsafe { cstr_to_str(method) };
    let args_str = unsafe { slice_to_str(args_json, len) };
    let source = current_plugin_name();

    let args: serde_json::Value =
        serde_json::from_str(args_str).unwrap_or(serde_json::Value::Null);

    match bridge()
        .bus
        .query_blocking(target_str, method_str, args, &source)
    {
        Ok(resp) => match resp.result {
            Ok(val) => alloc_cstring(&val.to_string()),
            Err(err) => {
                log::warn!("query_plugin({target_str}.{method_str}) error: {err}");
                std::ptr::null_mut()
            }
        },
        Err(err) => {
            log::warn!("query_plugin({target_str}.{method_str}) bus error: {err}");
            std::ptr::null_mut()
        }
    }
}

extern "C" fn host_register_service(name: *const c_char) {
    let service_name = unsafe { cstr_to_str(name) };
    let plugin = current_plugin_name();
    log::info!("plugin '{plugin}' registering service '{service_name}'");
    bridge().bus.register_service(&plugin, service_name);
}

// ---------------------------------------------------------------------------
// Config Persistence
// ---------------------------------------------------------------------------

/// Get the per-plugin config directory: `~/.config/conch/plugins/{plugin_name}/`
fn plugin_config_dir(plugin_name: &str) -> Option<std::path::PathBuf> {
    dirs::config_dir().map(|d| d.join("conch").join("plugins").join(plugin_name))
}

extern "C" fn host_get_config(key: *const c_char) -> *mut c_char {
    let key_str = unsafe { cstr_to_str(key) };
    let plugin = current_plugin_name();

    let Some(dir) = plugin_config_dir(&plugin) else {
        return std::ptr::null_mut();
    };
    let path = dir.join(format!("{key_str}.json"));

    match std::fs::read_to_string(&path) {
        Ok(contents) => alloc_cstring(&contents),
        Err(_) => std::ptr::null_mut(),
    }
}

extern "C" fn host_set_config(key: *const c_char, value: *const c_char) {
    let key_str = unsafe { cstr_to_str(key) };
    let value_str = unsafe { cstr_to_str(value) };
    let plugin = current_plugin_name();

    let Some(dir) = plugin_config_dir(&plugin) else {
        log::error!("host_set_config: cannot determine config directory");
        return;
    };

    if let Err(e) = std::fs::create_dir_all(&dir) {
        log::error!("host_set_config: failed to create config dir: {e}");
        return;
    }

    let path = dir.join(format!("{key_str}.json"));
    if let Err(e) = std::fs::write(&path, value_str) {
        log::error!("host_set_config: failed to write {}: {e}", path.display());
    }
}

// ---------------------------------------------------------------------------
// Menu Registration (stub)
// ---------------------------------------------------------------------------

extern "C" fn host_register_menu_item(
    _menu: *const c_char,
    _label: *const c_char,
    _action: *const c_char,
    _keybind: *const c_char,
) {
    let label = unsafe { cstr_to_str(_label) };
    log::debug!("host_register_menu_item: stub — '{label}'");
}

// ---------------------------------------------------------------------------
// Clipboard (stub)
// ---------------------------------------------------------------------------

extern "C" fn host_clipboard_set(text: *const c_char) {
    let text_str = unsafe { cstr_to_str(text) };
    match arboard::Clipboard::new() {
        Ok(mut clipboard) => {
            if let Err(e) = clipboard.set_text(text_str) {
                log::warn!("host_clipboard_set: failed to set clipboard: {e}");
            }
        }
        Err(e) => {
            log::warn!("host_clipboard_set: failed to open clipboard: {e}");
        }
    }
}

extern "C" fn host_clipboard_get() -> *mut c_char {
    match arboard::Clipboard::new() {
        Ok(mut clipboard) => match clipboard.get_text() {
            Ok(text) => alloc_cstring(&text),
            Err(e) => {
                log::debug!("host_clipboard_get: no text in clipboard: {e}");
                std::ptr::null_mut()
            }
        },
        Err(e) => {
            log::warn!("host_clipboard_get: failed to open clipboard: {e}");
            std::ptr::null_mut()
        }
    }
}

// ---------------------------------------------------------------------------
// Theme (stub)
// ---------------------------------------------------------------------------

extern "C" fn host_get_theme() -> *mut c_char {
    // Read the theme from the shared theme store.
    let json = THEME_JSON.lock().clone();
    if json.is_empty() {
        return std::ptr::null_mut();
    }
    alloc_cstring(&json)
}

// ---------------------------------------------------------------------------
// Context Menu (stub)
// ---------------------------------------------------------------------------

extern "C" fn host_show_context_menu(_json: *const c_char, _len: usize) -> *mut c_char {
    log::debug!("host_show_context_menu: stub — returning null");
    std::ptr::null_mut()
}

// ---------------------------------------------------------------------------
// Memory Management
// ---------------------------------------------------------------------------

extern "C" fn host_free_string(ptr: *mut c_char) {
    if !ptr.is_null() {
        // SAFETY: The pointer was allocated by `CString::into_raw` in this
        // module (via `alloc_cstring` or `query_plugin`). The plugin must
        // not use the pointer after calling free_string.
        unsafe {
            drop(CString::from_raw(ptr));
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panel_registry_register_and_iterate() {
        let mut reg = PanelRegistry::new();
        let h1 = reg.register(PanelLocation::Left, "Files".into(), "file_browser".into());
        let h2 = reg.register(PanelLocation::Right, "Sessions".into(), "ssh".into());

        assert_eq!(h1, 1);
        assert_eq!(h2, 2);

        let all: Vec<_> = reg.panels().collect();
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn panel_registry_set_widgets() {
        let mut reg = PanelRegistry::new();
        let h = reg.register(PanelLocation::Left, "Test".into(), "test_plugin".into());

        assert_eq!(
            reg.panels().find(|(id, _)| *id == h).unwrap().1.cached_widgets_json,
            "[]"
        );

        reg.set_widgets(h, r#"[{"type":"label","text":"hi"}]"#.into());

        assert_eq!(
            reg.panels().find(|(id, _)| *id == h).unwrap().1.cached_widgets_json,
            r#"[{"type":"label","text":"hi"}]"#
        );
    }

    #[test]
    fn panel_registry_set_widgets_nonexistent_handle_is_noop() {
        let mut reg = PanelRegistry::new();
        // Should not panic.
        reg.set_widgets(999, "ignored".into());
    }

    #[test]
    fn panel_registry_remove_by_plugin() {
        let mut reg = PanelRegistry::new();
        reg.register(PanelLocation::Left, "A".into(), "alpha".into());
        reg.register(PanelLocation::Right, "B".into(), "alpha".into());
        reg.register(PanelLocation::Bottom, "C".into(), "beta".into());

        reg.remove_by_plugin("alpha");

        let remaining: Vec<_> = reg.panels().collect();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].1.plugin_name, "beta");
    }

    #[test]
    fn panel_registry_handles_are_monotonic() {
        let mut reg = PanelRegistry::new();
        let h1 = reg.register(PanelLocation::None, "A".into(), "p".into());
        let h2 = reg.register(PanelLocation::None, "B".into(), "p".into());
        let h3 = reg.register(PanelLocation::None, "C".into(), "p".into());
        assert!(h1 < h2);
        assert!(h2 < h3);
    }

    #[test]
    fn current_plugin_name_without_prefix() {
        // When not on a plugin thread, should return "unknown".
        let name = current_plugin_name();
        assert_eq!(name, "unknown");
    }

    #[test]
    fn alloc_and_free_cstring() {
        let ptr = alloc_cstring("hello");
        assert!(!ptr.is_null());
        // Read it back to verify.
        let s = unsafe { CStr::from_ptr(ptr) }.to_str().unwrap();
        assert_eq!(s, "hello");
        // Free it (should not panic or leak).
        host_free_string(ptr);
    }

    #[test]
    fn free_null_is_safe() {
        host_free_string(std::ptr::null_mut());
    }

    #[test]
    fn alloc_cstring_with_interior_nul_returns_null() {
        let ptr = alloc_cstring("hello\0world");
        assert!(ptr.is_null());
    }
}

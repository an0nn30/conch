//! Host API bridge — implements the HostApi vtable for native plugins.
//!
//! Uses a global `OnceLock` to hold shared state accessible from the
//! `extern "C"` function pointers that plugins call.

use std::collections::HashMap;
use std::ffi::{c_char, c_void, CStr, CString};
use std::sync::{Arc, OnceLock};

use conch_plugin::bus::PluginBus;
use conch_plugin_sdk::{
    HostApi, OpenSessionResult, PanelHandle, PanelLocation, SessionBackendVtable, SessionHandle,
    SessionMeta,
};
use parking_lot::Mutex;

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
}

static BRIDGE: OnceLock<BridgeInner> = OnceLock::new();

/// Initialise the global bridge state.
///
/// Must be called exactly once before any plugin invokes a `HostApi` function.
/// Typically called during app startup after creating the bus and panel registry.
pub fn init_bridge(bus: Arc<PluginBus>, panels: Arc<Mutex<PanelRegistry>>) {
    BRIDGE
        .set(BridgeInner { bus, panels })
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
// Session Backends (stub)
// ---------------------------------------------------------------------------

extern "C" fn host_open_session(
    _meta: *const SessionMeta,
    _vtable: *const SessionBackendVtable,
    _backend_handle: *mut c_void,
) -> OpenSessionResult {
    log::warn!("host_open_session: not yet implemented");
    // Return a zeroed result — handle 0 signals failure.
    OpenSessionResult {
        handle: SessionHandle(0),
        output_cb: stub_output_cb,
        output_ctx: std::ptr::null_mut(),
    }
}

extern "C" fn stub_output_cb(_ctx: *mut c_void, _buf: *const u8, _len: usize) {
    // no-op stub
}

extern "C" fn host_close_session(_handle: SessionHandle) {
    log::warn!("host_close_session: not yet implemented");
}

// ---------------------------------------------------------------------------
// Dialogs (stub)
// ---------------------------------------------------------------------------

extern "C" fn host_show_form(_json: *const c_char, _len: usize) -> *mut c_char {
    log::debug!("host_show_form: stub — returning null (cancelled)");
    std::ptr::null_mut()
}

extern "C" fn host_show_confirm(_msg: *const c_char) -> bool {
    log::debug!("host_show_confirm: stub — returning false");
    false
}

extern "C" fn host_show_prompt(
    _msg: *const c_char,
    _default_value: *const c_char,
) -> *mut c_char {
    log::debug!("host_show_prompt: stub — returning null (cancelled)");
    std::ptr::null_mut()
}

extern "C" fn host_show_alert(_title: *const c_char, _msg: *const c_char) {
    let title = unsafe { cstr_to_str(_title) };
    let msg = unsafe { cstr_to_str(_msg) };
    log::info!("host_show_alert: [{title}] {msg}");
}

extern "C" fn host_show_error(_title: *const c_char, _msg: *const c_char) {
    let title = unsafe { cstr_to_str(_title) };
    let msg = unsafe { cstr_to_str(_msg) };
    log::error!("host_show_error: [{title}] {msg}");
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
// Config Persistence (stub)
// ---------------------------------------------------------------------------

extern "C" fn host_get_config(_key: *const c_char) -> *mut c_char {
    log::debug!("host_get_config: stub — returning null");
    std::ptr::null_mut()
}

extern "C" fn host_set_config(_key: *const c_char, _value: *const c_char) {
    log::debug!("host_set_config: stub — no-op");
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

extern "C" fn host_clipboard_set(_text: *const c_char) {
    log::debug!("host_clipboard_set: stub — no-op");
}

extern "C" fn host_clipboard_get() -> *mut c_char {
    log::debug!("host_clipboard_get: stub — returning null");
    std::ptr::null_mut()
}

// ---------------------------------------------------------------------------
// Theme (stub)
// ---------------------------------------------------------------------------

extern "C" fn host_get_theme() -> *mut c_char {
    log::debug!("host_get_theme: stub — returning null");
    std::ptr::null_mut()
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

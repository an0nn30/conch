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
    SftpHandle, SftpVtable,
};
use parking_lot::Mutex;
use tokio::sync::oneshot;

use super::dialogs::{self, DialogMessage, DialogRequest, DialogSender};
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

/// A prompt request that should be rendered inline in a session tab.
pub struct SessionPromptRequest {
    pub handle: SessionHandle,
    /// 0 = confirm (Accept/Reject), 1 = password input.
    pub prompt_type: u8,
    pub message: String,
    pub detail: String,
    pub reply: oneshot::Sender<Option<String>>,
}

/// Registry of pending session open/close requests from plugins.
pub struct SessionRegistry {
    pub pending_open: Vec<PendingSession>,
    pub pending_close: Vec<SessionHandle>,
    pub pending_status: Vec<SessionStatusUpdate>,
    pub pending_prompts: Vec<SessionPromptRequest>,
    next_handle: u64,
}

/// A session that a plugin wants to open — drained by ConchApp each frame.
pub struct PendingSession {
    pub handle: SessionHandle,
    pub title: String,
    pub bridge: PluginSessionBridge,
    pub vtable: SessionBackendVtable,
    pub backend_handle: *mut c_void,
    /// The viewport where the user interaction that triggered this session occurred.
    /// Used to route the session to the correct window (main vs. extra).
    pub target_viewport: Option<egui::ViewportId>,
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
            pending_prompts: Vec::new(),
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

// ---------------------------------------------------------------------------
// Status Bar
// ---------------------------------------------------------------------------

/// A status bar entry set by a plugin.
#[derive(Debug, Clone)]
pub struct StatusBarEntry {
    pub text: String,
    /// 0=info, 1=warn, 2=error, 3=success.
    pub level: u8,
    /// Optional progress fraction (0.0–1.0). Negative means no progress bar.
    pub progress: f32,
    pub timestamp: std::time::Instant,
}

static STATUS_BAR: parking_lot::Mutex<Option<StatusBarEntry>> = parking_lot::Mutex::new(None);

// ---------------------------------------------------------------------------
// PTY Write Queue
// ---------------------------------------------------------------------------

/// Pending writes to the focused window's active terminal session.
/// Plugins call `write_to_pty` from background threads; the render loop
/// drains this queue and writes to the actual PTY.
static PENDING_PTY_WRITES: parking_lot::Mutex<Vec<Vec<u8>>> = parking_lot::Mutex::new(Vec::new());

/// Queue bytes to be written to the active session's PTY on the next frame.
pub fn queue_pty_write(data: Vec<u8>) {
    PENDING_PTY_WRITES.lock().push(data);
}

/// Drain all pending PTY writes. Called by the render loop.
pub fn drain_pty_writes() -> Vec<Vec<u8>> {
    let mut queue = PENDING_PTY_WRITES.lock();
    std::mem::take(&mut *queue)
}

// ---------------------------------------------------------------------------
// New Tab Queue
// ---------------------------------------------------------------------------

/// A pending request to open a new local tab.
pub struct PendingNewTab {
    /// Optional command to write to the PTY after the tab opens.
    pub command: Option<String>,
    /// If true, open a plain default shell ignoring terminal.shell config.
    pub plain: bool,
}

static PENDING_NEW_TABS: parking_lot::Mutex<Vec<PendingNewTab>> =
    parking_lot::Mutex::new(Vec::new());

/// Queue a new tab request. Called from plugin threads.
pub fn queue_new_tab(command: Option<String>, plain: bool) {
    PENDING_NEW_TABS.lock().push(PendingNewTab { command, plain });
}

/// Drain all pending new-tab requests. Called by the render loop.
pub fn drain_new_tabs() -> Vec<PendingNewTab> {
    let mut queue = PENDING_NEW_TABS.lock();
    std::mem::take(&mut *queue)
}

/// Monotonically increasing version bumped on every `set_status_bar` call.
/// The render loop compares this against its last-seen value to detect changes
/// and request a repaint (needed because status updates come from background
/// threads that can't call `ctx.request_repaint()`).
static STATUS_BAR_VERSION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Set the global status bar entry. Pass `None` to clear.
pub fn set_status_bar(entry: Option<StatusBarEntry>) {
    *STATUS_BAR.lock() = entry;
    STATUS_BAR_VERSION.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

/// Get the current status bar entry (if any).
pub fn get_status_bar() -> Option<StatusBarEntry> {
    STATUS_BAR.lock().clone()
}

/// Get the current status bar version counter.
pub fn status_bar_version() -> u64 {
    STATUS_BAR_VERSION.load(std::sync::atomic::Ordering::Relaxed)
}

/// Shared theme JSON, updated whenever the app theme changes.
static THEME_JSON: parking_lot::Mutex<String> = parking_lot::Mutex::new(String::new());

/// Tracks the most recent viewport that dispatched widget events per plugin.
///
/// When a plugin panel interaction (button click, tree activate, etc.) occurs
/// in a specific viewport, we record which viewport it was. When the plugin
/// later calls `host_open_session`, we attach this viewport as the target so
/// that `drain_pending_sessions` can route the session to the correct window.
static LAST_EVENT_VIEWPORT: std::sync::LazyLock<parking_lot::Mutex<HashMap<String, egui::ViewportId>>> =
    std::sync::LazyLock::new(|| parking_lot::Mutex::new(HashMap::new()));

/// Record that `plugin_name` most recently received widget events from `viewport_id`.
pub fn set_event_viewport(plugin_name: &str, viewport_id: egui::ViewportId) {
    log::debug!(
        "set_event_viewport: plugin='{}' viewport={:?}",
        plugin_name, viewport_id
    );
    LAST_EVENT_VIEWPORT
        .lock()
        .insert(plugin_name.to_string(), viewport_id);
}

/// Look up the last viewport that sent widget events to `plugin_name`.
fn get_event_viewport(plugin_name: &str) -> Option<egui::ViewportId> {
    let result = LAST_EVENT_VIEWPORT.lock().get(plugin_name).copied();
    log::debug!(
        "get_event_viewport: plugin='{}' -> {:?}",
        plugin_name, result
    );
    result
}

// ---------------------------------------------------------------------------
// Plugin Menu Items
// ---------------------------------------------------------------------------

/// A menu item registered by a plugin at runtime.
#[derive(Debug, Clone)]
pub struct PluginMenuItem {
    pub menu: String,
    pub label: String,
    pub action: String,
    pub keybind: Option<String>,
    pub plugin_name: String,
}

/// Global registry of plugin-registered menu items.
static MENU_ITEMS: parking_lot::Mutex<Vec<PluginMenuItem>> = parking_lot::Mutex::new(Vec::new());

/// Version counter incremented whenever plugin menu items change.
static MENU_ITEMS_VERSION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Get a snapshot of all registered plugin menu items.
pub fn plugin_menu_items() -> Vec<PluginMenuItem> {
    MENU_ITEMS.lock().clone()
}

/// Get the current version of the plugin menu items registry.
pub fn plugin_menu_items_version() -> u64 {
    MENU_ITEMS_VERSION.load(std::sync::atomic::Ordering::Relaxed)
}

/// Remove all menu items registered by a specific plugin.
pub fn remove_menu_items_for_plugin(plugin_name: &str) {
    MENU_ITEMS.lock().retain(|item| item.plugin_name != plugin_name);
    MENU_ITEMS_VERSION.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

// ---------------------------------------------------------------------------
// Active Session Info (for Lua session.current())
// ---------------------------------------------------------------------------

/// Metadata about the currently active session, updated each frame by the app.
#[derive(Debug, Clone, Default)]
pub struct ActiveSessionInfo {
    pub title: String,
    pub session_type: String, // "local" or "ssh"
}

static ACTIVE_SESSION: parking_lot::Mutex<Option<ActiveSessionInfo>> =
    parking_lot::Mutex::new(None);

/// Update the active session info (called by the app each frame).
pub fn set_active_session(info: Option<ActiveSessionInfo>) {
    *ACTIVE_SESSION.lock() = info;
}

/// Get the current active session info.
pub fn get_active_session() -> Option<ActiveSessionInfo> {
    ACTIVE_SESSION.lock().clone()
}

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
        "focus_glow": c32(theme.focus_glow),
        "border": c32(theme.border),
        "rounding": theme.rounding,
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
        session_prompt: host_session_prompt,
        show_context_menu: host_show_context_menu,
        free_string: host_free_string,
        set_status: host_set_status,
        register_sftp: host_register_sftp,
        acquire_sftp: host_acquire_sftp,
        write_to_pty: host_write_to_pty,
        new_tab: host_new_tab,
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
        .and_then(|n| {
            n.strip_prefix("plugin:")
                .or_else(|| n.strip_prefix("lua-plugin:"))
        })
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

    // Look up the viewport that last dispatched widget events for this plugin.
    let plugin_name = current_plugin_name();
    let target_viewport = get_event_viewport(&plugin_name);

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
        target_viewport,
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
// Session Prompts (inline in tab)
// ---------------------------------------------------------------------------

extern "C" fn host_session_prompt(
    handle: SessionHandle,
    prompt_type: u8,
    msg: *const c_char,
    detail: *const c_char,
) -> *mut c_char {
    let msg_str = unsafe { cstr_to_str(msg) }.to_string();
    let detail_str = if detail.is_null() {
        String::new()
    } else {
        unsafe { cstr_to_str(detail) }.to_string()
    };

    log::info!(
        "host_session_prompt: handle={:?} type={} msg='{}'",
        handle, prompt_type, msg_str
    );

    let (reply_tx, reply_rx) = oneshot::channel();
    let b = bridge();
    b.session_registry.lock().pending_prompts.push(SessionPromptRequest {
        handle,
        prompt_type,
        message: msg_str,
        detail: detail_str,
        reply: reply_tx,
    });

    // Block the plugin thread until the user responds in the UI.
    match reply_rx.blocking_recv() {
        Ok(Some(result)) => alloc_cstring(&result),
        Ok(None) | Err(_) => std::ptr::null_mut(),
    }
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

    let plugin_name = current_plugin_name();
    let target_viewport = get_event_viewport(&plugin_name);

    let (reply_tx, reply_rx) = oneshot::channel();
    let _ = bridge().dialog_tx.send(DialogMessage {
        request: DialogRequest::Form {
            descriptor,
            reply: reply_tx,
        },
        target_viewport,
    });

    // Block the plugin thread until the UI thread responds.
    match reply_rx.blocking_recv() {
        Ok(Some(result)) => alloc_cstring(&result),
        Ok(None) | Err(_) => std::ptr::null_mut(),
    }
}

extern "C" fn host_show_confirm(msg: *const c_char) -> bool {
    let msg_str = unsafe { cstr_to_str(msg) }.to_string();

    let plugin_name = current_plugin_name();
    let thread_name = std::thread::current().name().unwrap_or("unnamed").to_string();
    let target_viewport = get_event_viewport(&plugin_name);
    log::info!(
        "host_show_confirm: plugin='{}' thread='{}' target_viewport={:?}",
        plugin_name, thread_name, target_viewport
    );

    let (reply_tx, reply_rx) = oneshot::channel();
    let _ = bridge().dialog_tx.send(DialogMessage {
        request: DialogRequest::Confirm {
            msg: msg_str,
            reply: reply_tx,
        },
        target_viewport,
    });

    reply_rx.blocking_recv().unwrap_or(false)
}

extern "C" fn host_show_prompt(
    msg: *const c_char,
    default_value: *const c_char,
) -> *mut c_char {
    let msg_str = unsafe { cstr_to_str(msg) }.to_string();
    let default = unsafe { cstr_to_str(default_value) }.to_string();

    let plugin_name = current_plugin_name();
    let thread_name = std::thread::current().name().unwrap_or("unnamed").to_string();
    let target_viewport = get_event_viewport(&plugin_name);
    log::info!(
        "host_show_prompt: plugin='{}' thread='{}' target_viewport={:?}",
        plugin_name, thread_name, target_viewport
    );

    let (reply_tx, reply_rx) = oneshot::channel();
    let _ = bridge().dialog_tx.send(DialogMessage {
        request: DialogRequest::Prompt {
            msg: msg_str,
            default_value: default,
            reply: reply_tx,
        },
        target_viewport,
    });

    match reply_rx.blocking_recv() {
        Ok(Some(result)) => alloc_cstring(&result),
        Ok(None) | Err(_) => std::ptr::null_mut(),
    }
}

extern "C" fn host_show_alert(title: *const c_char, msg: *const c_char) {
    let title_str = unsafe { cstr_to_str(title) }.to_string();
    let msg_str = unsafe { cstr_to_str(msg) }.to_string();

    let plugin_name = current_plugin_name();
    let target_viewport = get_event_viewport(&plugin_name);

    let (reply_tx, reply_rx) = oneshot::channel();
    let _ = bridge().dialog_tx.send(DialogMessage {
        request: DialogRequest::Alert {
            title: title_str,
            msg: msg_str,
            reply: reply_tx,
        },
        target_viewport,
    });

    // Block until user dismisses.
    let _ = reply_rx.blocking_recv();
}

extern "C" fn host_show_error(title: *const c_char, msg: *const c_char) {
    let title_str = unsafe { cstr_to_str(title) }.to_string();
    let msg_str = unsafe { cstr_to_str(msg) }.to_string();

    let plugin_name = current_plugin_name();
    let target_viewport = get_event_viewport(&plugin_name);

    let (reply_tx, reply_rx) = oneshot::channel();
    let _ = bridge().dialog_tx.send(DialogMessage {
        request: DialogRequest::Error {
            title: title_str,
            msg: msg_str,
            reply: reply_tx,
        },
        target_viewport,
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

    // Parse JSON and push to the notification system.
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(json_str) {
        let title = v.get("title").and_then(|t| t.as_str()).map(String::from);
        let body = v.get("body").and_then(|b| b.as_str()).unwrap_or("").to_string();
        let level_str = v.get("level").and_then(|l| l.as_str()).unwrap_or("info");
        let level = crate::notifications::NotificationLevel::from_str(level_str);
        let duration_ms = v.get("duration_ms").and_then(|d| d.as_u64());
        let notif = crate::notifications::Notification::new(title, body, level, duration_ms);
        crate::notifications::push(notif);
    }
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
    Some(conch_core::config::config_dir().join("plugins").join(plugin_name))
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
    menu: *const c_char,
    label: *const c_char,
    action: *const c_char,
    keybind: *const c_char,
) {
    let menu_str = unsafe { cstr_to_str(menu) }.to_string();
    let label_str = unsafe { cstr_to_str(label) }.to_string();
    let action_str = unsafe { cstr_to_str(action) }.to_string();
    let keybind_str = if keybind.is_null() {
        None
    } else {
        let s = unsafe { cstr_to_str(keybind) }.to_string();
        if s.is_empty() { None } else { Some(s) }
    };
    let plugin_name = current_plugin_name();

    log::info!(
        "plugin '{plugin_name}' registering menu item '{label_str}' in '{menu_str}' (action: {action_str})"
    );

    MENU_ITEMS.lock().push(PluginMenuItem {
        menu: menu_str,
        label: label_str,
        action: action_str,
        keybind: keybind_str,
        plugin_name,
    });
    MENU_ITEMS_VERSION.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
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

extern "C" fn host_show_context_menu(json: *const c_char, len: usize) -> *mut c_char {
    let json_str = unsafe { slice_to_str(json, len) };

    let plugin_name = current_plugin_name();
    let target_viewport = get_event_viewport(&plugin_name);

    let (reply_tx, reply_rx) = oneshot::channel();
    let _ = bridge().dialog_tx.send(DialogMessage {
        request: DialogRequest::ContextMenu {
            items_json: json_str.to_string(),
            reply: reply_tx,
        },
        target_viewport,
    });

    match reply_rx.blocking_recv() {
        Ok(Some(selected_id)) => alloc_cstring(&selected_id),
        Ok(None) | Err(_) => std::ptr::null_mut(),
    }
}

// ---------------------------------------------------------------------------
// Status Bar
// ---------------------------------------------------------------------------

extern "C" fn host_set_status(text: *const c_char, level: u8, progress: f32) {
    if text.is_null() {
        set_status_bar(None);
    } else {
        let text_str = unsafe { cstr_to_str(text) }.to_string();
        set_status_bar(Some(StatusBarEntry {
            text: text_str,
            level,
            progress,
            timestamp: std::time::Instant::now(),
        }));
    }
}

// ---------------------------------------------------------------------------
// SFTP Vtable Registry
// ---------------------------------------------------------------------------

struct SftpEntry {
    vtable: *const SftpVtable,
    ctx: *mut c_void,
}

// SAFETY: SftpEntry contains raw pointers but they are only accessed through
// the vtable functions which are thread-safe by contract.
unsafe impl Send for SftpEntry {}
unsafe impl Sync for SftpEntry {}

static SFTP_REGISTRY: parking_lot::Mutex<Option<HashMap<u64, SftpEntry>>> =
    parking_lot::Mutex::new(None);

fn sftp_registry() -> &'static parking_lot::Mutex<Option<HashMap<u64, SftpEntry>>> {
    &SFTP_REGISTRY
}

extern "C" fn host_register_sftp(session_id: u64, vtable: *const SftpVtable, ctx: *mut c_void) {
    if vtable.is_null() || ctx.is_null() {
        log::warn!("host_register_sftp: null vtable or ctx");
        return;
    }

    // Retain the context so the registry holds a reference.
    unsafe { ((*vtable).retain)(ctx) };

    let mut guard = sftp_registry().lock();
    let map = guard.get_or_insert_with(HashMap::new);

    // If there was a previous entry, release it.
    if let Some(old) = map.remove(&session_id) {
        unsafe { ((*old.vtable).release)(old.ctx) };
    }

    map.insert(session_id, SftpEntry { vtable, ctx });
    log::info!("host_register_sftp: registered SFTP for session {session_id}");
}

extern "C" fn host_acquire_sftp(session_id: u64) -> SftpHandle {
    let zeroed = SftpHandle {
        vtable: std::ptr::null(),
        ctx: std::ptr::null_mut(),
    };

    let guard = sftp_registry().lock();
    let Some(map) = guard.as_ref() else {
        return zeroed;
    };

    let Some(entry) = map.get(&session_id) else {
        return zeroed;
    };

    // Retain the context for the caller.
    unsafe { ((*entry.vtable).retain)(entry.ctx) };

    SftpHandle {
        vtable: entry.vtable,
        ctx: entry.ctx,
    }
}

/// Deregister and release the SFTP handle for a session.
///
/// Called when an SSH session disconnects. The entry's release is called to
/// drop the registry's reference.
pub fn deregister_sftp(session_id: u64) {
    let mut guard = sftp_registry().lock();
    if let Some(map) = guard.as_mut() {
        if let Some(entry) = map.remove(&session_id) {
            unsafe { ((*entry.vtable).release)(entry.ctx) };
            log::info!("deregister_sftp: removed SFTP for session {session_id}");
        }
    }
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
// PTY Write
// ---------------------------------------------------------------------------

extern "C" fn host_write_to_pty(data: *const u8, len: usize) {
    if data.is_null() || len == 0 {
        return;
    }
    let bytes = unsafe { std::slice::from_raw_parts(data, len) }.to_vec();
    queue_pty_write(bytes);
}

// ---------------------------------------------------------------------------
// New Tab
// ---------------------------------------------------------------------------

extern "C" fn host_new_tab(command: *const c_char, plain: bool) {
    let cmd = if command.is_null() {
        None
    } else {
        let s = unsafe { cstr_to_str(command) }.to_string();
        if s.is_empty() { None } else { Some(s) }
    };
    queue_new_tab(cmd, plain);
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

//! Host API vtable — function pointers the host exports for plugins to call.
//!
//! When the host calls `conch_plugin_setup()`, it passes a `&HostApi` that the
//! plugin stores. All host interaction goes through this vtable.

use std::ffi::{c_char, c_void};

use crate::session::{OpenSessionResult, PanelHandle, SessionBackendVtable, SessionHandle, SessionMeta};
use crate::plugin_info::PanelLocation;

/// The complete host API available to native plugins.
///
/// Every function pointer here is implemented by the host (`conch_app`).
/// Plugins call these to register panels, open sessions, publish events, etc.
///
/// String arguments are null-terminated UTF-8 C strings. String return values
/// are heap-allocated by the host and must be freed with `free_string`.
#[repr(C)]
pub struct HostApi {
    // -- Panel Management --------------------------------------------------

    /// Register a panel tab at the given location.
    ///
    /// Returns a handle used to update the panel's widgets.
    /// `name` and `icon` are null-terminated UTF-8 strings. `icon` may be null.
    pub register_panel: extern "C" fn(
        location: PanelLocation,
        name: *const c_char,
        icon: *const c_char,
    ) -> PanelHandle,

    /// Replace the widget tree for a panel.
    ///
    /// `json` is a UTF-8 JSON string (length `len`) encoding a `Vec<Widget>`.
    /// The host parses, caches, and renders the widgets. The plugin should call
    /// this whenever its UI state changes — not every frame.
    pub set_widgets: extern "C" fn(
        handle: PanelHandle,
        json: *const c_char,
        len: usize,
    ),

    // -- Session Backends --------------------------------------------------

    /// Open a new terminal tab backed by the plugin's byte-stream session.
    ///
    /// - `meta`: tab title, type, icon
    /// - `vtable`: function table for write/resize/shutdown/drop
    /// - `backend_handle`: opaque pointer passed to all vtable calls
    ///
    /// Returns an `OpenSessionResult` containing the session handle and the
    /// output callback the plugin uses to push bytes to the host's terminal.
    pub open_session: extern "C" fn(
        meta: *const SessionMeta,
        vtable: *const SessionBackendVtable,
        backend_handle: *mut c_void,
    ) -> OpenSessionResult,

    /// Close a session tab previously opened with `open_session`.
    pub close_session: extern "C" fn(handle: SessionHandle),

    // -- Dialogs (blocking — called from plugin thread) --------------------

    /// Show a form dialog and block until the user submits or cancels.
    ///
    /// `json`/`len` encode a form descriptor (title, fields). Returns a JSON
    /// string with the form results, or null if the user cancelled.
    /// Caller must free the result with `free_string`.
    pub show_form: extern "C" fn(json: *const c_char, len: usize) -> *mut c_char,

    /// Show a yes/no confirmation dialog. Returns true if confirmed.
    pub show_confirm: extern "C" fn(msg: *const c_char) -> bool,

    /// Show a text input prompt. Returns the entered text, or null if cancelled.
    /// Caller must free the result with `free_string`.
    pub show_prompt: extern "C" fn(msg: *const c_char, default_value: *const c_char) -> *mut c_char,

    /// Show an informational alert dialog (OK button only).
    pub show_alert: extern "C" fn(title: *const c_char, msg: *const c_char),

    /// Show an error dialog (OK button only).
    pub show_error: extern "C" fn(title: *const c_char, msg: *const c_char),

    // -- Notifications & Logging -------------------------------------------

    /// Show a toast notification.
    ///
    /// `json`/`len` encode: `{ "title": "...", "body": "...", "level": "info|warn|error", "duration_ms": 3000 }`
    pub notify: extern "C" fn(json: *const c_char, len: usize),

    /// Log a message at the given level.
    ///
    /// Levels: 0=trace, 1=debug, 2=info, 3=warn, 4=error.
    pub log: extern "C" fn(level: u8, msg: *const c_char),

    // -- Plugin IPC (Message Bus) ------------------------------------------

    /// Broadcast an event to all subscribers.
    ///
    /// `event_type` is a dotted name (e.g., "ssh.session_ready").
    /// `data_json`/`len` encode arbitrary event payload.
    pub publish_event: extern "C" fn(
        event_type: *const c_char,
        data_json: *const c_char,
        len: usize,
    ),

    /// Subscribe to events matching `event_type`.
    ///
    /// When a matching event is published, the host calls `conch_plugin_event()`
    /// on the plugin's thread with the event JSON.
    pub subscribe: extern "C" fn(event_type: *const c_char),

    /// Send a direct query to another plugin's registered service.
    ///
    /// - `target`: plugin name (e.g., "ssh")
    /// - `method`: service method (e.g., "exec")
    /// - `args_json`/`len`: JSON arguments
    ///
    /// Returns a JSON response string. Caller must free with `free_string`.
    /// Returns null if the target plugin or service is not available.
    ///
    /// This call blocks until the target plugin handles the query.
    pub query_plugin: extern "C" fn(
        target: *const c_char,
        method: *const c_char,
        args_json: *const c_char,
        len: usize,
    ) -> *mut c_char,

    /// Register a named service that other plugins can query.
    ///
    /// The service `name` should be unqualified (e.g., "exec", not "ssh.exec").
    /// The host prefixes it with the plugin name automatically.
    pub register_service: extern "C" fn(name: *const c_char),

    // -- Config Persistence ------------------------------------------------

    /// Read a config value from the plugin's config file.
    ///
    /// Returns the value as a JSON string, or null if the key doesn't exist.
    /// Caller must free with `free_string`.
    pub get_config: extern "C" fn(key: *const c_char) -> *mut c_char,

    /// Write a config value to the plugin's config file.
    ///
    /// `value` is a JSON-encoded string. Pass null to delete the key.
    pub set_config: extern "C" fn(key: *const c_char, value: *const c_char),

    // -- Menu Registration -------------------------------------------------

    /// Add an item to the app's menu bar.
    ///
    /// - `menu`: which menu to add to (e.g., "File", "Tools", "Plugins")
    /// - `label`: display text
    /// - `action`: action identifier sent back via `conch_plugin_event()`
    /// - `keybind`: keyboard shortcut string (e.g., "cmd+shift+s"), or null
    pub register_menu_item: extern "C" fn(
        menu: *const c_char,
        label: *const c_char,
        action: *const c_char,
        keybind: *const c_char,
    ),

    // -- Clipboard ---------------------------------------------------------

    /// Set the system clipboard contents.
    pub clipboard_set: extern "C" fn(text: *const c_char),

    /// Get the system clipboard contents.
    /// Caller must free with `free_string`.
    pub clipboard_get: extern "C" fn() -> *mut c_char,

    // -- Theme -------------------------------------------------------------

    /// Get the current UI theme as a JSON string.
    ///
    /// Contains colors, font sizes, dark_mode flag — everything a plugin needs
    /// to style custom rendering consistently with the host.
    /// Caller must free with `free_string`.
    pub get_theme: extern "C" fn() -> *mut c_char,

    // -- Context Menu ------------------------------------------------------

    /// Show a context menu at the current cursor position.
    ///
    /// `json`/`len` encode an array of menu items:
    /// `[{ "id": "copy", "label": "Copy", "enabled": true }, ...]`
    ///
    /// Returns the selected item's `id` as a string, or null if dismissed.
    /// Caller must free with `free_string`.
    pub show_context_menu: extern "C" fn(json: *const c_char, len: usize) -> *mut c_char,

    // -- Memory Management -------------------------------------------------

    /// Free a string previously returned by the host.
    ///
    /// All host functions that return `*mut c_char` allocate via the host's
    /// allocator. The plugin MUST call this to free them.
    pub free_string: extern "C" fn(ptr: *mut c_char),
}

// SAFETY: HostApi is a table of function pointers, all of which are thread-safe
// (they internally dispatch to the host's event loop / message bus).
unsafe impl Send for HostApi {}
unsafe impl Sync for HostApi {}

//! Session backend types for plugins that provide terminal sessions.
//!
//! A session backend is a bidirectional byte stream. The plugin provides raw
//! bytes (e.g., from an SSH channel) and the host feeds them into an
//! `alacritty_terminal::Term` via the VTE parser. The plugin never touches
//! terminal internals — it just provides I/O.

use std::ffi::{c_char, c_void};

/// Opaque handle to a session created by the host.
///
/// Returned by `HostApi::open_session` and used in subsequent session calls.
#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SessionHandle(pub u64);

/// Opaque handle to a registered panel.
#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PanelHandle(pub u64);

/// Metadata the plugin provides when opening a new session tab.
#[repr(C)]
pub struct SessionMeta {
    /// Full title (e.g., "dustin@lab.nexxuscraft.com").
    pub title: *const c_char,
    /// Short title for narrow tab bars (e.g., "lab").
    pub short_title: *const c_char,
    /// Session type identifier (e.g., "ssh", "serial", "telnet").
    pub session_type: *const c_char,
    /// Optional icon name or path. Null if none.
    pub icon: *const c_char,
}

// SAFETY: Same reasoning as PluginInfo — raw pointers to plugin-owned data,
// only read by host during open_session call.
unsafe impl Send for SessionMeta {}

/// Function table the plugin provides so the host can send input to the session.
///
/// All function pointers receive the plugin's opaque `backend_handle` as their
/// first argument. The plugin is responsible for routing that to the correct
/// internal session (e.g., an SSH channel).
#[repr(C)]
pub struct SessionBackendVtable {
    /// Write input bytes (user keystrokes) to the session.
    pub write: extern "C" fn(handle: *mut c_void, buf: *const u8, len: usize),
    /// Notify the backend that the terminal was resized.
    pub resize: extern "C" fn(handle: *mut c_void, cols: u16, rows: u16),
    /// Gracefully shut down the session.
    pub shutdown: extern "C" fn(handle: *mut c_void),
    /// Free the backend handle. Called after shutdown, or if the tab is closed.
    pub drop: extern "C" fn(handle: *mut c_void),
}

// SAFETY: Vtable contains only function pointers (which are Send+Sync).
unsafe impl Send for SessionBackendVtable {}
unsafe impl Sync for SessionBackendVtable {}

/// Callback the host provides so the plugin can push output bytes.
///
/// The plugin calls this from its own thread whenever data arrives (e.g., from
/// an SSH channel read). The host feeds these bytes into the VTE parser.
///
/// - `ctx`: opaque host context (do not dereference in plugin code)
/// - `buf`/`len`: output bytes from the remote session
pub type OutputCallback = extern "C" fn(ctx: *mut c_void, buf: *const u8, len: usize);

/// Result of `HostApi::open_session` — contains everything the plugin needs
/// to interact with the newly created session tab.
#[repr(C)]
pub struct OpenSessionResult {
    /// Handle for this session (use with `close_session`).
    pub handle: SessionHandle,
    /// Callback the plugin uses to push output bytes to the host's terminal.
    pub output_cb: OutputCallback,
    /// Opaque context passed as the first argument to `output_cb`.
    /// Host-owned — do not free or dereference.
    pub output_ctx: *mut c_void,
}

// SAFETY: output_cb is a function pointer (Send+Sync), output_ctx is only
// passed back to output_cb which is thread-safe by contract.
unsafe impl Send for OpenSessionResult {}

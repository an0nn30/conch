//! Conch Plugin SDK — C ABI types for native plugin authors.
//!
//! This crate defines the stable interface between Conch and native plugins
//! (shared libraries). It contains:
//!
//! - **Plugin metadata** (`PluginInfo`, `PluginType`, `PanelLocation`)
//! - **Host API vtable** (`HostApi`) — functions the host exports for plugins to call
//! - **Session backend** (`SessionBackendVtable`, `SessionMeta`) — for plugins that provide terminal sessions
//! - **Widget types** (`Widget`, `WidgetEvent`) — declarative UI via JSON/serde
//! - **Convenience macro** (`declare_plugin!`) — reduces boilerplate for Rust plugin authors
//!
//! Native plugins are shared libraries (`.dylib`/`.so`/`.dll`) that export a set of
//! well-known C symbols. The host discovers and loads them at runtime via `dlopen`.

pub mod host_api;
pub mod plugin_info;
pub mod session;
pub mod widgets;

// Re-export core types at crate root for convenience.
pub use host_api::*;
pub use plugin_info::*;
pub use session::*;
pub use widgets::{PluginEvent, Widget, WidgetEvent};

/// Declare a native Conch plugin by implementing the required C ABI exports.
///
/// This macro generates the five `extern "C"` symbols the host expects:
///
/// - `conch_plugin_info() -> PluginInfo`
/// - `conch_plugin_setup(*const HostApi) -> *mut c_void`
/// - `conch_plugin_event(*mut c_void, *const c_char, usize)`
/// - `conch_plugin_render(*mut c_void) -> *const c_char`
/// - `conch_plugin_teardown(*mut c_void)`
/// - `conch_plugin_query(*mut c_void, *const c_char, *const c_char, usize) -> *mut c_char`
///
/// # Usage
///
/// ```rust,ignore
/// use conch_plugin_sdk::*;
///
/// struct MyPlugin { /* ... */ }
///
/// impl MyPlugin {
///     fn new(api: &'static HostApi) -> Self { /* ... */ }
///     fn handle_event(&mut self, event: PluginEvent) { /* ... */ }
///     fn render(&self) -> Vec<Widget> { /* ... */ }
///     fn handle_query(&self, method: &str, args: serde_json::Value) -> serde_json::Value { /* ... */ }
/// }
///
/// declare_plugin!(
///     info: PluginInfo {
///         name: c"My Plugin",
///         description: c"Does cool things",
///         version: c"0.1.0",
///         plugin_type: PluginType::Panel,
///         panel_location: PanelLocation::Right,
///         dependencies: std::ptr::null(),
///         num_dependencies: 0,
///     },
///     state: MyPlugin,
///     setup: |api| MyPlugin::new(api),
///     event: |state, event| state.handle_event(event),
///     render: |state| state.render(),
///     query: |state, method, args| state.handle_query(method, args),
/// );
/// ```
#[macro_export]
macro_rules! declare_plugin {
    (
        info: $info:expr,
        state: $state_ty:ty,
        setup: |$api:ident| $setup:expr,
        event: |$eself:ident, $event:ident| $event_handler:expr,
        render: |$rself:ident| $render:expr,
        query: |$qself:ident, $method:ident, $args:ident| $query_handler:expr $(,)?
    ) => {
        #[unsafe(no_mangle)]
        pub extern "C" fn conch_plugin_info() -> $crate::PluginInfo {
            $info
        }

        #[unsafe(no_mangle)]
        pub extern "C" fn conch_plugin_setup(
            api: *const $crate::HostApi,
        ) -> *mut ::std::ffi::c_void {
            let $api: &'static $crate::HostApi = unsafe { &*api };
            let state: $state_ty = $setup;
            let boxed = Box::new(state);
            Box::into_raw(boxed) as *mut ::std::ffi::c_void
        }

        #[unsafe(no_mangle)]
        pub extern "C" fn conch_plugin_event(
            handle: *mut ::std::ffi::c_void,
            json: *const ::std::ffi::c_char,
            len: usize,
        ) {
            let $eself = unsafe { &mut *(handle as *mut $state_ty) };
            let slice = unsafe { ::std::slice::from_raw_parts(json as *const u8, len) };
            if let Ok(json_str) = ::std::str::from_utf8(slice) {
                if let Ok($event) = ::serde_json::from_str::<$crate::PluginEvent>(json_str) {
                    $event_handler
                }
            }
        }

        /// Returns a JSON-encoded widget tree. The host reads but does NOT free
        /// this pointer — the plugin owns the memory and overwrites it on the
        /// next `conch_plugin_render` call.
        #[unsafe(no_mangle)]
        pub extern "C" fn conch_plugin_render(
            handle: *mut ::std::ffi::c_void,
        ) -> *const ::std::ffi::c_char {
            use std::sync::OnceLock;
            use std::ffi::CString;

            // Static buffer so the returned pointer lives until next call.
            thread_local! {
                static RENDER_BUF: std::cell::RefCell<CString> =
                    std::cell::RefCell::new(CString::new("[]").unwrap());
            }

            let $rself = unsafe { &*(handle as *mut $state_ty) };
            let widgets: Vec<$crate::Widget> = $render;
            let json = ::serde_json::to_string(&widgets).unwrap_or_else(|_| "[]".to_string());
            let c_str = CString::new(json).unwrap_or_else(|_| CString::new("[]").unwrap());

            RENDER_BUF.with(|buf| {
                *buf.borrow_mut() = c_str;
                buf.borrow().as_ptr()
            })
        }

        #[unsafe(no_mangle)]
        pub extern "C" fn conch_plugin_teardown(handle: *mut ::std::ffi::c_void) {
            if !handle.is_null() {
                unsafe { drop(Box::from_raw(handle as *mut $state_ty)); }
            }
        }

        #[unsafe(no_mangle)]
        pub extern "C" fn conch_plugin_query(
            handle: *mut ::std::ffi::c_void,
            method_ptr: *const ::std::ffi::c_char,
            args_json: *const ::std::ffi::c_char,
            args_len: usize,
        ) -> *mut ::std::ffi::c_char {
            let $qself = unsafe { &*(handle as *mut $state_ty) };
            let $method = unsafe { ::std::ffi::CStr::from_ptr(method_ptr) }
                .to_str()
                .unwrap_or("");
            let args_slice = unsafe { ::std::slice::from_raw_parts(args_json as *const u8, args_len) };
            let $args: ::serde_json::Value = ::std::str::from_utf8(args_slice)
                .ok()
                .and_then(|s| ::serde_json::from_str(s).ok())
                .unwrap_or(::serde_json::Value::Null);
            let result: ::serde_json::Value = $query_handler;
            let json = ::serde_json::to_string(&result).unwrap_or_else(|_| "null".to_string());
            match ::std::ffi::CString::new(json) {
                Ok(c) => c.into_raw(),
                Err(_) => ::std::ptr::null_mut(),
            }
        }
    };
}


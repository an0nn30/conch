//! Safe Rust host API trait — replaces the C ABI vtable for Lua/Java/Tauri.
//!
//! Each plugin gets its own `Arc<dyn HostApi>` instance with the plugin name
//! baked in, eliminating the fragile thread-name-based identification.

use conch_plugin_sdk::PanelLocation;

/// Safe Rust interface for the host API.
///
/// This mirrors every function in `conch_plugin_sdk::HostApi` (the C vtable)
/// but uses safe Rust types (`&str`, `String`, `Option`, slices) instead of
/// raw `*const c_char` / `*mut c_void` pointers.
///
/// Implementations:
/// - `TauriHostApi` (in `conch_tauri`) — Tauri-based UI
/// - `CHostApiAdapter` (below) — wraps the C vtable for backward compat
pub trait HostApi: Send + Sync {
    // -- Identity --
    /// The name of the plugin this API instance belongs to.
    fn plugin_name(&self) -> &str;

    // -- Panel Management --
    fn register_panel(&self, location: PanelLocation, name: &str, icon: Option<&str>) -> u64;
    fn set_widgets(&self, handle: u64, widgets_json: &str);

    // -- Logging & Notifications --
    fn log(&self, level: u8, msg: &str);
    fn notify(&self, json: &str);
    fn set_status(&self, text: Option<&str>, level: u8, progress: f32);

    // -- Event Bus --
    fn publish_event(&self, event_type: &str, data_json: &str);
    fn subscribe(&self, event_type: &str);
    fn query_plugin(&self, target: &str, method: &str, args_json: &str) -> Option<String>;
    fn register_service(&self, name: &str);

    // -- Config Persistence --
    fn get_config(&self, key: &str) -> Option<String>;
    fn set_config(&self, key: &str, value: &str);

    // -- Clipboard --
    fn clipboard_set(&self, text: &str);
    fn clipboard_get(&self) -> Option<String>;

    // -- Theme --
    fn get_theme(&self) -> Option<String>;

    // -- Menu --
    fn register_menu_item(
        &self,
        menu: &str,
        label: &str,
        action: &str,
        keybind: Option<&str>,
    );

    // -- Dialogs (blocking — called from plugin thread) --
    fn show_form(&self, json: &str) -> Option<String>;
    fn show_confirm(&self, msg: &str) -> bool;
    fn show_prompt(&self, msg: &str, default_value: &str) -> Option<String>;
    fn show_alert(&self, title: &str, msg: &str);
    fn show_error(&self, title: &str, msg: &str);
    fn show_context_menu(&self, json: &str) -> Option<String>;

    // -- Terminal / Tabs --
    fn write_to_pty(&self, data: &[u8]);
    fn new_tab(&self, command: Option<&str>, plain: bool);

    // -- Session Management --
    /// Open a new terminal tab backed by the plugin.
    /// Returns a session handle (0 = failed).
    /// `meta_json` encodes `{ "title": "...", "short_title": "...", "session_type": "..." }`.
    fn open_session(&self, meta_json: &str) -> u64;
    fn close_session(&self, handle: u64);
    fn set_session_status(&self, handle: u64, status: u8, detail: Option<&str>);

    /// Show a prompt inline in a session's connecting screen. Blocks.
    fn session_prompt(
        &self,
        handle: u64,
        prompt_type: u8,
        msg: &str,
        detail: Option<&str>,
    ) -> Option<String>;

    // -- SFTP (no-op for Tauri, functional for egui) --
    fn register_sftp(&self, _session_id: u64) {
        // Default no-op — Tauri has native SFTP.
    }
    fn acquire_sftp(&self, _session_id: u64) -> Option<u64> {
        None
    }
}

/// Adapter that wraps the C ABI `conch_plugin_sdk::HostApi` vtable and
/// implements the safe `HostApi` trait. Used by the egui app for backward
/// compatibility — the Lua/Java runners call trait methods, and this adapter
/// forwards them to the C function pointers.
pub struct CHostApiAdapter {
    name: String,
    api: &'static conch_plugin_sdk::HostApi,
}

impl CHostApiAdapter {
    /// # Safety
    /// The caller must ensure `api` is valid for `'static` (typically it is,
    /// since the host allocates it once and never deallocates).
    pub unsafe fn new(name: String, api: &'static conch_plugin_sdk::HostApi) -> Self {
        Self { name, api }
    }
}

impl HostApi for CHostApiAdapter {
    fn plugin_name(&self) -> &str {
        &self.name
    }

    fn register_panel(&self, location: PanelLocation, name: &str, icon: Option<&str>) -> u64 {
        let c_name = std::ffi::CString::new(name).unwrap();
        let c_icon = icon.and_then(|i| std::ffi::CString::new(i).ok());
        let icon_ptr = c_icon.as_ref().map_or(std::ptr::null(), |c| c.as_ptr());
        let handle = (self.api.register_panel)(location, c_name.as_ptr(), icon_ptr);
        handle.0
    }

    fn set_widgets(&self, handle: u64, widgets_json: &str) {
        let c_json = std::ffi::CString::new(widgets_json).unwrap();
        (self.api.set_widgets)(
            conch_plugin_sdk::PanelHandle(handle),
            c_json.as_ptr(),
            widgets_json.len(),
        );
    }

    fn log(&self, level: u8, msg: &str) {
        if let Ok(c) = std::ffi::CString::new(msg) {
            (self.api.log)(level, c.as_ptr());
        }
    }

    fn notify(&self, json: &str) {
        let c = std::ffi::CString::new(json).unwrap();
        (self.api.notify)(c.as_ptr(), json.len());
    }

    fn set_status(&self, text: Option<&str>, level: u8, progress: f32) {
        let c = text.and_then(|t| std::ffi::CString::new(t).ok());
        let ptr = c.as_ref().map_or(std::ptr::null(), |c| c.as_ptr());
        (self.api.set_status)(ptr, level, progress);
    }

    fn publish_event(&self, event_type: &str, data_json: &str) {
        let c_type = std::ffi::CString::new(event_type).unwrap();
        let c_data = std::ffi::CString::new(data_json).unwrap();
        (self.api.publish_event)(c_type.as_ptr(), c_data.as_ptr(), data_json.len());
    }

    fn subscribe(&self, event_type: &str) {
        let c = std::ffi::CString::new(event_type).unwrap();
        (self.api.subscribe)(c.as_ptr());
    }

    fn query_plugin(&self, target: &str, method: &str, args_json: &str) -> Option<String> {
        let c_target = std::ffi::CString::new(target).unwrap();
        let c_method = std::ffi::CString::new(method).unwrap();
        let c_args = std::ffi::CString::new(args_json).unwrap();
        let result = (self.api.query_plugin)(
            c_target.as_ptr(),
            c_method.as_ptr(),
            c_args.as_ptr(),
            args_json.len(),
        );
        if result.is_null() {
            None
        } else {
            let s = unsafe { std::ffi::CStr::from_ptr(result) }
                .to_str()
                .unwrap_or("")
                .to_string();
            (self.api.free_string)(result);
            Some(s)
        }
    }

    fn register_service(&self, name: &str) {
        let c = std::ffi::CString::new(name).unwrap();
        (self.api.register_service)(c.as_ptr());
    }

    fn get_config(&self, key: &str) -> Option<String> {
        let c_key = std::ffi::CString::new(key).unwrap();
        let result = (self.api.get_config)(c_key.as_ptr());
        if result.is_null() {
            None
        } else {
            let s = unsafe { std::ffi::CStr::from_ptr(result) }
                .to_str()
                .unwrap_or("")
                .to_string();
            (self.api.free_string)(result);
            Some(s)
        }
    }

    fn set_config(&self, key: &str, value: &str) {
        let c_key = std::ffi::CString::new(key).unwrap();
        let c_val = std::ffi::CString::new(value).unwrap();
        (self.api.set_config)(c_key.as_ptr(), c_val.as_ptr());
    }

    fn clipboard_set(&self, text: &str) {
        let c = std::ffi::CString::new(text).unwrap();
        (self.api.clipboard_set)(c.as_ptr());
    }

    fn clipboard_get(&self) -> Option<String> {
        let result = (self.api.clipboard_get)();
        if result.is_null() {
            None
        } else {
            let s = unsafe { std::ffi::CStr::from_ptr(result) }
                .to_str()
                .unwrap_or("")
                .to_string();
            (self.api.free_string)(result);
            Some(s)
        }
    }

    fn get_theme(&self) -> Option<String> {
        let result = (self.api.get_theme)();
        if result.is_null() {
            None
        } else {
            let s = unsafe { std::ffi::CStr::from_ptr(result) }
                .to_str()
                .unwrap_or("")
                .to_string();
            (self.api.free_string)(result);
            Some(s)
        }
    }

    fn register_menu_item(
        &self,
        menu: &str,
        label: &str,
        action: &str,
        keybind: Option<&str>,
    ) {
        let c_menu = std::ffi::CString::new(menu).unwrap();
        let c_label = std::ffi::CString::new(label).unwrap();
        let c_action = std::ffi::CString::new(action).unwrap();
        let c_keybind = keybind.and_then(|k| std::ffi::CString::new(k).ok());
        let kb_ptr = c_keybind
            .as_ref()
            .map_or(std::ptr::null(), |c| c.as_ptr());
        (self.api.register_menu_item)(
            c_menu.as_ptr(),
            c_label.as_ptr(),
            c_action.as_ptr(),
            kb_ptr,
        );
    }

    fn show_form(&self, json: &str) -> Option<String> {
        let c = std::ffi::CString::new(json).unwrap();
        let result = (self.api.show_form)(c.as_ptr(), json.len());
        if result.is_null() {
            None
        } else {
            let s = unsafe { std::ffi::CStr::from_ptr(result) }
                .to_str()
                .unwrap_or("")
                .to_string();
            (self.api.free_string)(result);
            Some(s)
        }
    }

    fn show_confirm(&self, msg: &str) -> bool {
        let c = std::ffi::CString::new(msg).unwrap();
        (self.api.show_confirm)(c.as_ptr())
    }

    fn show_prompt(&self, msg: &str, default_value: &str) -> Option<String> {
        let c_msg = std::ffi::CString::new(msg).unwrap();
        let c_default = std::ffi::CString::new(default_value).unwrap();
        let result = (self.api.show_prompt)(c_msg.as_ptr(), c_default.as_ptr());
        if result.is_null() {
            None
        } else {
            let s = unsafe { std::ffi::CStr::from_ptr(result) }
                .to_str()
                .unwrap_or("")
                .to_string();
            (self.api.free_string)(result);
            Some(s)
        }
    }

    fn show_alert(&self, title: &str, msg: &str) {
        let c_title = std::ffi::CString::new(title).unwrap();
        let c_msg = std::ffi::CString::new(msg).unwrap();
        (self.api.show_alert)(c_title.as_ptr(), c_msg.as_ptr());
    }

    fn show_error(&self, title: &str, msg: &str) {
        let c_title = std::ffi::CString::new(title).unwrap();
        let c_msg = std::ffi::CString::new(msg).unwrap();
        (self.api.show_error)(c_title.as_ptr(), c_msg.as_ptr());
    }

    fn show_context_menu(&self, json: &str) -> Option<String> {
        let c = std::ffi::CString::new(json).unwrap();
        let result = (self.api.show_context_menu)(c.as_ptr(), json.len());
        if result.is_null() {
            None
        } else {
            let s = unsafe { std::ffi::CStr::from_ptr(result) }
                .to_str()
                .unwrap_or("")
                .to_string();
            (self.api.free_string)(result);
            Some(s)
        }
    }

    fn write_to_pty(&self, data: &[u8]) {
        (self.api.write_to_pty)(data.as_ptr(), data.len());
    }

    fn new_tab(&self, command: Option<&str>, plain: bool) {
        let c = command.and_then(|cmd| std::ffi::CString::new(cmd).ok());
        let ptr = c.as_ref().map_or(std::ptr::null(), |c| c.as_ptr());
        (self.api.new_tab)(ptr, plain);
    }

    fn open_session(&self, meta_json: &str) -> u64 {
        // The C API expects SessionMeta + vtable, not JSON.
        // This adapter method is a simplified passthrough — the full
        // open_session flow is complex and only used by native plugins.
        // Lua/Java plugins that need sessions should use the trait directly.
        let _ = meta_json;
        0
    }

    fn close_session(&self, handle: u64) {
        (self.api.close_session)(conch_plugin_sdk::SessionHandle(handle));
    }

    fn set_session_status(&self, handle: u64, status: u8, detail: Option<&str>) {
        let c_detail = detail.and_then(|d| std::ffi::CString::new(d).ok());
        let ptr = c_detail
            .as_ref()
            .map_or(std::ptr::null(), |c| c.as_ptr());
        let ss = match status {
            0 => conch_plugin_sdk::SessionStatus::Connecting,
            1 => conch_plugin_sdk::SessionStatus::Connected,
            2 => conch_plugin_sdk::SessionStatus::Error,
            _ => conch_plugin_sdk::SessionStatus::Error,
        };
        (self.api.set_session_status)(conch_plugin_sdk::SessionHandle(handle), ss, ptr);
    }

    fn session_prompt(
        &self,
        handle: u64,
        prompt_type: u8,
        msg: &str,
        detail: Option<&str>,
    ) -> Option<String> {
        let c_msg = std::ffi::CString::new(msg).unwrap();
        let c_detail = detail.and_then(|d| std::ffi::CString::new(d).ok());
        let detail_ptr = c_detail
            .as_ref()
            .map_or(std::ptr::null(), |c| c.as_ptr());
        let result = (self.api.session_prompt)(
            conch_plugin_sdk::SessionHandle(handle),
            prompt_type,
            c_msg.as_ptr(),
            detail_ptr,
        );
        if result.is_null() {
            None
        } else {
            let s = unsafe { std::ffi::CStr::from_ptr(result) }
                .to_str()
                .unwrap_or("")
                .to_string();
            (self.api.free_string)(result);
            Some(s)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // A minimal mock implementation for testing the trait.
    struct MockHostApi {
        name: String,
    }

    impl HostApi for MockHostApi {
        fn plugin_name(&self) -> &str { &self.name }
        fn register_panel(&self, _: PanelLocation, _: &str, _: Option<&str>) -> u64 { 1 }
        fn set_widgets(&self, _: u64, _: &str) {}
        fn log(&self, _: u8, _: &str) {}
        fn notify(&self, _: &str) {}
        fn set_status(&self, _: Option<&str>, _: u8, _: f32) {}
        fn publish_event(&self, _: &str, _: &str) {}
        fn subscribe(&self, _: &str) {}
        fn query_plugin(&self, _: &str, _: &str, _: &str) -> Option<String> { None }
        fn register_service(&self, _: &str) {}
        fn get_config(&self, _: &str) -> Option<String> { None }
        fn set_config(&self, _: &str, _: &str) {}
        fn clipboard_set(&self, _: &str) {}
        fn clipboard_get(&self) -> Option<String> { None }
        fn get_theme(&self) -> Option<String> { None }
        fn register_menu_item(&self, _: &str, _: &str, _: &str, _: Option<&str>) {}
        fn show_form(&self, _: &str) -> Option<String> { None }
        fn show_confirm(&self, _: &str) -> bool { false }
        fn show_prompt(&self, _: &str, _: &str) -> Option<String> { None }
        fn show_alert(&self, _: &str, _: &str) {}
        fn show_error(&self, _: &str, _: &str) {}
        fn show_context_menu(&self, _: &str) -> Option<String> { None }
        fn write_to_pty(&self, _: &[u8]) {}
        fn new_tab(&self, _: Option<&str>, _: bool) {}
        fn open_session(&self, _: &str) -> u64 { 0 }
        fn close_session(&self, _: u64) {}
        fn set_session_status(&self, _: u64, _: u8, _: Option<&str>) {}
        fn session_prompt(&self, _: u64, _: u8, _: &str, _: Option<&str>) -> Option<String> { None }
    }

    #[test]
    fn mock_host_api_implements_trait() {
        let api: Box<dyn HostApi> = Box::new(MockHostApi { name: "test".into() });
        assert_eq!(api.plugin_name(), "test");
        assert_eq!(api.register_panel(PanelLocation::Right, "panel", None), 1);
        assert!(api.get_config("key").is_none());
        assert!(!api.show_confirm("question?"));
    }

    #[test]
    fn trait_object_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Box<dyn HostApi>>();
    }
}

//! Safe Rust host API trait for Lua/Java/Tauri plugin integration.
//!
//! Each plugin gets its own `Arc<dyn HostApi>` instance with the plugin name
//! baked in, eliminating the fragile thread-name-based identification.

use conch_plugin_sdk::PanelLocation;

/// Safe Rust interface for the host API.
///
/// Implemented by `TauriHostApi` in `conch_tauri`.
pub trait HostApi: Send + Sync {
    fn plugin_name(&self) -> &str;
    fn check_permission(&self, _capability: &str) -> bool {
        true
    }

    // -- Panel Management --
    fn register_panel(&self, location: PanelLocation, name: &str, icon: Option<&str>) -> u64;
    fn set_widgets(&self, handle: u64, widgets_json: &str);
    fn open_docked_view(&self, req_json: &str) -> Option<String>;
    fn close_docked_view(&self, view_id: &str) -> bool;
    fn focus_docked_view(&self, view_id: &str) -> bool;

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
    fn get_active_session(&self) -> Option<String> {
        None
    }
    fn exec_active_session(&self, _command: &str) -> Option<String> {
        None
    }

    // -- Menu --
    fn register_menu_item(&self, menu: &str, label: &str, action: &str, keybind: Option<&str>);

    /// Like `register_menu_item` but allows the caller to specify the plugin
    /// name that the item is filed under.  The default implementation ignores
    /// `plugin_name` and delegates to `register_menu_item`.  TauriHostApi
    /// overrides this to use the provided name, which is important for Java
    /// plugins where a single shared HostApi serves all plugins.
    fn register_menu_item_as(
        &self,
        _plugin_name: &str,
        menu: &str,
        label: &str,
        action: &str,
        keybind: Option<&str>,
    ) {
        self.register_menu_item(menu, label, action, keybind);
    }

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
    fn open_session(&self, meta_json: &str) -> u64;
    fn close_session(&self, handle: u64);
    fn set_session_status(&self, handle: u64, status: u8, detail: Option<&str>);
    fn session_prompt(
        &self,
        handle: u64,
        prompt_type: u8,
        msg: &str,
        detail: Option<&str>,
    ) -> Option<String>;
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MockHostApi {
        name: String,
    }

    impl HostApi for MockHostApi {
        fn plugin_name(&self) -> &str {
            &self.name
        }
        fn register_panel(&self, _: PanelLocation, _: &str, _: Option<&str>) -> u64 {
            1
        }
        fn set_widgets(&self, _: u64, _: &str) {}
        fn open_docked_view(&self, _: &str) -> Option<String> {
            None
        }
        fn close_docked_view(&self, _: &str) -> bool {
            false
        }
        fn focus_docked_view(&self, _: &str) -> bool {
            false
        }
        fn log(&self, _: u8, _: &str) {}
        fn notify(&self, _: &str) {}
        fn set_status(&self, _: Option<&str>, _: u8, _: f32) {}
        fn publish_event(&self, _: &str, _: &str) {}
        fn subscribe(&self, _: &str) {}
        fn query_plugin(&self, _: &str, _: &str, _: &str) -> Option<String> {
            None
        }
        fn register_service(&self, _: &str) {}
        fn get_config(&self, _: &str) -> Option<String> {
            None
        }
        fn set_config(&self, _: &str, _: &str) {}
        fn clipboard_set(&self, _: &str) {}
        fn clipboard_get(&self) -> Option<String> {
            None
        }
        fn get_theme(&self) -> Option<String> {
            None
        }
        fn register_menu_item(&self, _: &str, _: &str, _: &str, _: Option<&str>) {}
        fn show_form(&self, _: &str) -> Option<String> {
            None
        }
        fn show_confirm(&self, _: &str) -> bool {
            false
        }
        fn show_prompt(&self, _: &str, _: &str) -> Option<String> {
            None
        }
        fn show_alert(&self, _: &str, _: &str) {}
        fn show_error(&self, _: &str, _: &str) {}
        fn show_context_menu(&self, _: &str) -> Option<String> {
            None
        }
        fn write_to_pty(&self, _: &[u8]) {}
        fn new_tab(&self, _: Option<&str>, _: bool) {}
        fn open_session(&self, _: &str) -> u64 {
            0
        }
        fn close_session(&self, _: u64) {}
        fn set_session_status(&self, _: u64, _: u8, _: Option<&str>) {}
        fn session_prompt(&self, _: u64, _: u8, _: &str, _: Option<&str>) -> Option<String> {
            None
        }
    }

    #[test]
    fn mock_host_api_implements_trait() {
        let api: Box<dyn HostApi> = Box::new(MockHostApi {
            name: "test".into(),
        });
        assert_eq!(api.plugin_name(), "test");
        assert!(!api.show_confirm("?"));
    }

    #[test]
    fn trait_object_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Box<dyn HostApi>>();
    }
}

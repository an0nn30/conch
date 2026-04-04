//! Permission-checked HostApi wrapper.
//!
//! Plugins declare requested capabilities.
//! Host checks capabilities before sensitive HostApi calls and denies by default.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use conch_plugin::HostApi;
use conch_plugin_sdk::PanelLocation;
use parking_lot::{Mutex, RwLock};

#[derive(Clone, Debug)]
pub(crate) struct PermissionProfile {
    pub allow_all: bool,
    pub allowed: HashSet<String>,
}

impl PermissionProfile {
    pub fn deny_all() -> Self {
        Self {
            allow_all: false,
            allowed: HashSet::new(),
        }
    }

    pub fn from_declared(declared: &[String]) -> Self {
        let mut allowed = HashSet::new();
        for p in declared {
            let norm = p.trim().to_ascii_lowercase();
            if !norm.is_empty() {
                allowed.insert(norm);
            }
        }
        Self {
            allow_all: false,
            allowed,
        }
    }
}

pub(crate) struct PermissionCheckedHostApi {
    inner: Arc<dyn HostApi>,
    base_plugin_name: String,
    profiles: Arc<RwLock<HashMap<String, PermissionProfile>>>,
    denied_notices: Mutex<HashSet<String>>,
}

impl PermissionCheckedHostApi {
    pub fn new(
        inner: Arc<dyn HostApi>,
        base_plugin_name: String,
        profiles: Arc<RwLock<HashMap<String, PermissionProfile>>>,
    ) -> Self {
        Self {
            inner,
            base_plugin_name,
            profiles,
            denied_notices: Mutex::new(HashSet::new()),
        }
    }

    fn effective_plugin_name(&self) -> String {
        if self.base_plugin_name == "java" {
            if let Some(thread_name) = std::thread::current().name() {
                if let Some(name) = thread_name.strip_prefix("plugin:") {
                    return name.to_string();
                }
            }
        }
        self.base_plugin_name.clone()
    }

    fn check_capability(&self, method: &str, capability: &str) -> bool {
        let plugin_name = self.effective_plugin_name();
        let profiles = self.profiles.read();
        let profile = profiles
            .get(&plugin_name)
            .or_else(|| profiles.get(&self.base_plugin_name));

        if let Some(p) = profile {
            if p.allow_all || p.allowed.contains(capability) {
                return true;
            }
        } else {
            let msg = format!(
                "Plugin '{}' attempted '{}' but has no permission profile. Access denied.",
                plugin_name, method
            );
            self.warn_user_once(&plugin_name, method, capability, &msg);
            return false;
        }

        let msg = format!(
            "Plugin '{}' was denied '{}' (missing capability '{}').",
            plugin_name, method, capability
        );
        self.warn_user_once(&plugin_name, method, capability, &msg);
        false
    }

    fn warn_user_once(&self, plugin_name: &str, method: &str, capability: &str, msg: &str) {
        log::warn!("{msg}");
        let key = format!("{plugin_name}:{method}:{capability}");
        let mut seen = self.denied_notices.lock();
        if seen.insert(key) {
            self.inner.show_error("Plugin Permission Denied", msg);
        }
    }
}

impl HostApi for PermissionCheckedHostApi {
    fn plugin_name(&self) -> &str {
        self.inner.plugin_name()
    }

    fn check_permission(&self, capability: &str) -> bool {
        self.check_capability("check_permission", capability)
    }

    fn register_panel(&self, location: PanelLocation, name: &str, icon: Option<&str>) -> u64 {
        if !self.check_capability("register_panel", "ui.panel") {
            return 0;
        }
        self.inner.register_panel(location, name, icon)
    }

    fn set_widgets(&self, handle: u64, widgets_json: &str) {
        if !self.check_capability("set_widgets", "ui.panel") {
            return;
        }
        self.inner.set_widgets(handle, widgets_json)
    }

    fn log(&self, level: u8, msg: &str) {
        self.inner.log(level, msg)
    }

    fn notify(&self, json: &str) {
        if !self.check_capability("notify", "ui.notify") {
            return;
        }
        self.inner.notify(json)
    }

    fn set_status(&self, text: Option<&str>, level: u8, progress: f32) {
        if !self.check_capability("set_status", "ui.notify") {
            return;
        }
        self.inner.set_status(text, level, progress)
    }

    fn publish_event(&self, event_type: &str, data_json: &str) {
        if !self.check_capability("publish_event", "bus.publish") {
            return;
        }
        self.inner.publish_event(event_type, data_json)
    }

    fn subscribe(&self, event_type: &str) {
        if !self.check_capability("subscribe", "bus.subscribe") {
            return;
        }
        self.inner.subscribe(event_type)
    }

    fn query_plugin(&self, target: &str, method: &str, args_json: &str) -> Option<String> {
        if !self.check_capability("query_plugin", "bus.query") {
            return None;
        }
        self.inner.query_plugin(target, method, args_json)
    }

    fn register_service(&self, name: &str) {
        if !self.check_capability("register_service", "bus.publish") {
            return;
        }
        self.inner.register_service(name)
    }

    fn get_config(&self, key: &str) -> Option<String> {
        if !self.check_capability("get_config", "config.read") {
            return None;
        }
        self.inner.get_config(key)
    }

    fn set_config(&self, key: &str, value: &str) {
        if !self.check_capability("set_config", "config.write") {
            return;
        }
        self.inner.set_config(key, value)
    }

    fn clipboard_set(&self, text: &str) {
        if !self.check_capability("clipboard_set", "clipboard.write") {
            return;
        }
        self.inner.clipboard_set(text)
    }

    fn clipboard_get(&self) -> Option<String> {
        if !self.check_capability("clipboard_get", "clipboard.read") {
            return None;
        }
        self.inner.clipboard_get()
    }

    fn get_theme(&self) -> Option<String> {
        self.inner.get_theme()
    }

    fn get_active_session(&self) -> Option<String> {
        if !self.check_capability("get_active_session", "session.status") {
            return None;
        }
        self.inner.get_active_session()
    }

    fn exec_active_session(&self, command: &str) -> Option<String> {
        if !self.check_capability("exec_active_session", "session.exec") {
            return None;
        }
        self.inner.exec_active_session(command)
    }

    fn register_menu_item(&self, menu: &str, label: &str, action: &str, keybind: Option<&str>) {
        if !self.check_capability("register_menu_item", "ui.menu") {
            return;
        }
        self.inner.register_menu_item(menu, label, action, keybind)
    }

    fn show_form(&self, json: &str) -> Option<String> {
        if !self.check_capability("show_form", "ui.dialog") {
            return None;
        }
        self.inner.show_form(json)
    }

    fn show_confirm(&self, msg: &str) -> bool {
        if !self.check_capability("show_confirm", "ui.dialog") {
            return false;
        }
        self.inner.show_confirm(msg)
    }

    fn show_prompt(&self, msg: &str, default_value: &str) -> Option<String> {
        if !self.check_capability("show_prompt", "ui.dialog") {
            return None;
        }
        self.inner.show_prompt(msg, default_value)
    }

    fn show_alert(&self, title: &str, msg: &str) {
        if !self.check_capability("show_alert", "ui.dialog") {
            return;
        }
        self.inner.show_alert(title, msg)
    }

    fn show_error(&self, title: &str, msg: &str) {
        if !self.check_capability("show_error", "ui.dialog") {
            return;
        }
        self.inner.show_error(title, msg)
    }

    fn show_context_menu(&self, json: &str) -> Option<String> {
        if !self.check_capability("show_context_menu", "ui.dialog") {
            return None;
        }
        self.inner.show_context_menu(json)
    }

    fn write_to_pty(&self, data: &[u8]) {
        if !self.check_capability("write_to_pty", "session.write") {
            return;
        }
        self.inner.write_to_pty(data)
    }

    fn new_tab(&self, command: Option<&str>, plain: bool) {
        if !self.check_capability("new_tab", "session.new_tab") {
            return;
        }
        self.inner.new_tab(command, plain)
    }

    fn rename_active_tab(&self, title: &str) {
        if !self.check_capability("rename_active_tab", "session.rename_tab") {
            return;
        }
        self.inner.rename_active_tab(title)
    }

    fn rename_tab_by_id(&self, tab_id: &str, title: &str) {
        if !self.check_capability("rename_tab_by_id", "session.rename_tab") {
            return;
        }
        self.inner.rename_tab_by_id(tab_id, title)
    }

    fn focus_tab_by_id(&self, tab_id: &str) {
        if !self.check_capability("focus_tab_by_id", "session.rename_tab") {
            return;
        }
        self.inner.focus_tab_by_id(tab_id)
    }

    fn new_tab_with_title(
        &self,
        command: Option<&str>,
        plain: bool,
        title: Option<&str>,
    ) -> Option<String> {
        if !self.check_capability("new_tab_with_title", "session.new_tab") {
            return None;
        }
        let filtered_title = if title.is_some()
            && !self.check_capability("new_tab_with_title", "session.rename_tab")
        {
            None
        } else {
            title
        };
        self.inner
            .new_tab_with_title(command, plain, filtered_title)
    }

    fn open_session(&self, meta_json: &str) -> u64 {
        if !self.check_capability("open_session", "session.open") {
            return 0;
        }
        self.inner.open_session(meta_json)
    }

    fn close_session(&self, handle: u64) {
        if !self.check_capability("close_session", "session.close") {
            return;
        }
        self.inner.close_session(handle)
    }

    fn set_session_status(&self, handle: u64, status: u8, detail: Option<&str>) {
        if !self.check_capability("set_session_status", "session.status") {
            return;
        }
        self.inner.set_session_status(handle, status, detail)
    }

    fn session_prompt(
        &self,
        handle: u64,
        prompt_type: u8,
        msg: &str,
        detail: Option<&str>,
    ) -> Option<String> {
        if !self.check_capability("session_prompt", "session.exec") {
            return None;
        }
        self.inner.session_prompt(handle, prompt_type, msg, detail)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use conch_plugin_sdk::PanelLocation;

    struct MockHost {
        name: String,
        errors: Arc<Mutex<Vec<String>>>,
    }

    impl HostApi for MockHost {
        fn plugin_name(&self) -> &str {
            &self.name
        }
        fn register_panel(&self, _: PanelLocation, _: &str, _: Option<&str>) -> u64 {
            1
        }
        fn set_widgets(&self, _: u64, _: &str) {}
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
            Some("secret".into())
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
        fn show_error(&self, _: &str, msg: &str) {
            self.errors.lock().push(msg.to_string());
        }
        fn show_context_menu(&self, _: &str) -> Option<String> {
            None
        }
        fn write_to_pty(&self, _: &[u8]) {}
        fn new_tab(&self, _: Option<&str>, _: bool) {}
        fn rename_active_tab(&self, _: &str) {}
        fn rename_tab_by_id(&self, _: &str, _: &str) {}
        fn focus_tab_by_id(&self, _: &str) {}
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
    fn denies_when_profile_missing() {
        let errors = Arc::new(Mutex::new(Vec::new()));
        let inner: Arc<dyn HostApi> = Arc::new(MockHost {
            name: "test".into(),
            errors: Arc::clone(&errors),
        });
        let profiles = Arc::new(RwLock::new(HashMap::new()));
        let api = PermissionCheckedHostApi::new(inner, "missing".into(), profiles);
        assert_eq!(api.clipboard_get(), None);
        assert!(!errors.lock().is_empty(), "expected denial warning");
    }

    #[test]
    fn allows_declared_capability() {
        let errors = Arc::new(Mutex::new(Vec::new()));
        let inner: Arc<dyn HostApi> = Arc::new(MockHost {
            name: "test".into(),
            errors: Arc::clone(&errors),
        });
        let mut map = HashMap::new();
        map.insert(
            "demo".into(),
            PermissionProfile::from_declared(&["clipboard.read".into()]),
        );
        let profiles = Arc::new(RwLock::new(map));
        let api = PermissionCheckedHostApi::new(inner, "demo".into(), profiles);
        assert_eq!(api.clipboard_get().as_deref(), Some("secret"));
        assert!(errors.lock().is_empty(), "no denial expected");
    }
}

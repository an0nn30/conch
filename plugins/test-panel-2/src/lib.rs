//! Second test panel plugin — validates multi-panel same-location behavior.
//!
//! Registers in the same location (Left) as test-panel to test tabbed panels.

use std::ffi::CString;

use conch_plugin_sdk::{
    widgets::{PluginEvent, Widget, WidgetEvent},
    HostApi, PanelHandle, PanelLocation, PluginInfo, PluginType,
};

struct TestPanel2 {
    _api: &'static HostApi,
    _panel: PanelHandle,
    clicks: u64,
    checked: bool,
}

impl TestPanel2 {
    fn new(api: &'static HostApi) -> Self {
        let msg = CString::new("Test panel 2 loaded").unwrap();
        (api.log)(2, msg.as_ptr());

        let name = CString::new("Panel Two").unwrap();
        let panel = (api.register_panel)(PanelLocation::Left, name.as_ptr(), std::ptr::null());

        Self {
            _api: api,
            _panel: panel,
            clicks: 0,
            checked: false,
        }
    }

    fn handle_event(&mut self, event: PluginEvent) {
        match event {
            PluginEvent::Widget(WidgetEvent::ButtonClick { id }) => match id.as_str() {
                "click_me" => self.clicks += 1,
                "reset" => self.clicks = 0,
                _ => {}
            },
            PluginEvent::Widget(WidgetEvent::CheckboxChanged { id, checked }) => {
                if id == "toggle" {
                    self.checked = checked;
                }
            }
            _ => {}
        }
    }

    fn render(&self) -> Vec<Widget> {
        vec![
            Widget::heading("Panel Two"),
            Widget::Label {
                text: "This is a second panel sharing the Left location.".into(),
                style: None,
            },
            Widget::Separator,
            Widget::KeyValue {
                key: "Clicks".into(),
                value: self.clicks.to_string(),
            },
            Widget::button("click_me", "Click Me"),
            Widget::button("reset", "Reset"),
            Widget::Separator,
            Widget::Checkbox {
                id: "toggle".into(),
                label: "Toggle me".into(),
                checked: self.checked,
            },
            Widget::Badge {
                text: if self.checked { "ON".into() } else { "OFF".into() },
                variant: if self.checked {
                    conch_plugin_sdk::widgets::BadgeVariant::Success
                } else {
                    conch_plugin_sdk::widgets::BadgeVariant::Warn
                },
            },
        ]
    }

    fn handle_query(&self, method: &str, args: serde_json::Value) -> serde_json::Value {
        match method {
            "echo" => args,
            _ => serde_json::json!({ "error": "unknown method" }),
        }
    }
}

conch_plugin_sdk::declare_plugin!(
    info: PluginInfo {
        name: c"Test Panel 2".as_ptr(),
        description: c"Second test panel for multi-panel validation".as_ptr(),
        version: c"0.1.0".as_ptr(),
        plugin_type: PluginType::Panel,
        panel_location: PanelLocation::Left,
        dependencies: std::ptr::null(),
        num_dependencies: 0,
    },
    state: TestPanel2,
    setup: |api| TestPanel2::new(api),
    event: |state, event| state.handle_event(event),
    render: |state| state.render(),
    query: |state, method, args| state.handle_query(method, args),
);

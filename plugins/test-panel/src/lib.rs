//! Trivial native panel plugin for validation.
//!
//! Shows a heading, counter display, and increment/reset buttons.
//! Exercises: register_panel, widget rendering, event handling.

use std::ffi::CString;

use conch_plugin_sdk::{
    widgets::{PluginEvent, Widget, WidgetEvent},
    HostApi, PanelHandle, PanelLocation, PluginInfo, PluginType,
};

struct TestPanel {
    api: &'static HostApi,
    _panel: PanelHandle,
    counter: u64,
    text_value: String,
}

impl TestPanel {
    fn new(api: &'static HostApi) -> Self {
        let msg = CString::new("Test panel plugin loaded").unwrap();
        (api.log)(2, msg.as_ptr());

        let name = CString::new("Test Panel").unwrap();
        let panel = (api.register_panel)(PanelLocation::Left, name.as_ptr(), std::ptr::null());

        // Register a test service.
        let svc = CString::new("echo").unwrap();
        (api.register_service)(svc.as_ptr());

        Self {
            api,
            _panel: panel,
            counter: 0,
            text_value: String::new(),
        }
    }

    fn handle_event(&mut self, event: PluginEvent) {
        match event {
            PluginEvent::Widget(WidgetEvent::ButtonClick { id }) => match id.as_str() {
                "increment" => self.counter += 1,
                "reset" => self.counter = 0,
                "notify" => {
                    let notif = serde_json::json!({
                        "title": "Test",
                        "body": format!("Counter is {}", self.counter),
                        "level": "info",
                        "duration_ms": 2000,
                    });
                    let json = CString::new(notif.to_string()).unwrap();
                    let bytes = json.as_bytes();
                    (self.api.notify)(json.as_ptr(), bytes.len());
                }
                _ => {}
            },
            PluginEvent::Widget(WidgetEvent::TextInputChanged { id, value }) => {
                if id == "input" {
                    self.text_value = value;
                }
            }
            PluginEvent::Widget(WidgetEvent::TextInputSubmit { id, value }) => {
                if id == "input" {
                    let msg = CString::new(format!("Input submitted: {value}")).unwrap();
                    (self.api.log)(2, msg.as_ptr());
                    self.text_value.clear();
                }
            }
            _ => {}
        }
    }

    fn render(&self) -> Vec<Widget> {
        vec![
            Widget::heading("Test Panel"),
            Widget::Label {
                text: "A trivial native panel for testing.".into(),
                style: None,
            },
            Widget::Separator,
            Widget::KeyValue {
                key: "Counter".into(),
                value: self.counter.to_string(),
            },
            Widget::button("increment", "Increment"),
            Widget::button("reset", "Reset"),
            Widget::button("notify", "Show Notification"),
            Widget::Separator,
            Widget::TextInput {
                id: "input".into(),
                value: self.text_value.clone(),
                hint: Some("Type something...".into()),
                submit_on_enter: Some(true),
            },
        ]
    }

    fn handle_query(&self, method: &str, args: serde_json::Value) -> serde_json::Value {
        match method {
            "echo" => args,
            "get_counter" => serde_json::json!({ "counter": self.counter }),
            _ => serde_json::json!({ "error": "unknown method" }),
        }
    }
}

conch_plugin_sdk::declare_plugin!(
    info: PluginInfo {
        name: c"Test Panel".as_ptr(),
        description: c"Trivial test panel for validation".as_ptr(),
        version: c"0.1.0".as_ptr(),
        plugin_type: PluginType::Panel,
        panel_location: PanelLocation::Left,
        dependencies: std::ptr::null(),
        num_dependencies: 0,
    },
    state: TestPanel,
    setup: |api| TestPanel::new(api),
    event: |state, event| state.handle_event(event),
    render: |state| state.render(),
    query: |state, method, args| state.handle_query(method, args),
);

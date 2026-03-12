//! Echo session backend test plugin.
//!
//! Opens a terminal tab that echoes back whatever is typed. Exercises the
//! full session backend lifecycle:
//! - `open_session()` with SessionMeta and SessionBackendVtable
//! - Output callback for pushing bytes to the terminal
//! - Write/resize/shutdown vtable callbacks

use std::ffi::{c_void, CString};

use conch_plugin_sdk::{
    widgets::{PluginEvent, Widget, WidgetEvent},
    HostApi, OpenSessionResult, OutputCallback, PanelHandle, PanelLocation, PluginInfo,
    PluginType, SessionBackendVtable, SessionHandle, SessionMeta,
};

/// Per-session state for the echo backend.
struct EchoSession {
    handle: SessionHandle,
    output_cb: OutputCallback,
    output_ctx: *mut c_void,
    bytes_echoed: u64,
}

// SAFETY: The output callback and context are thread-safe (host-provided).
unsafe impl Send for EchoSession {}

struct EchoPlugin {
    api: &'static HostApi,
    _panel: PanelHandle,
    session: Option<EchoSession>,
}

impl EchoPlugin {
    fn new(api: &'static HostApi) -> Self {
        let msg = CString::new("Echo session plugin loaded").unwrap();
        (api.log)(2, msg.as_ptr());

        let name = CString::new("Echo").unwrap();
        let panel = (api.register_panel)(PanelLocation::Bottom, name.as_ptr(), std::ptr::null());

        Self {
            api,
            _panel: panel,
            session: None,
        }
    }

    fn handle_event(&mut self, event: PluginEvent) {
        match event {
            PluginEvent::Widget(WidgetEvent::ButtonClick { id }) => match id.as_str() {
                "open" => self.open_echo_session(),
                "close" => self.close_echo_session(),
                _ => {}
            },
            PluginEvent::Shutdown => {
                self.close_echo_session();
            }
            _ => {}
        }
    }

    fn open_echo_session(&mut self) {
        if self.session.is_some() {
            return; // Already open.
        }

        let title = CString::new("Echo Terminal").unwrap();
        let short_title = CString::new("echo").unwrap();
        let session_type = CString::new("echo").unwrap();
        let meta = SessionMeta {
            title: title.as_ptr(),
            short_title: short_title.as_ptr(),
            session_type: session_type.as_ptr(),
            icon: std::ptr::null(),
        };

        let vtable = SessionBackendVtable {
            write: echo_write,
            resize: echo_resize,
            shutdown: echo_shutdown,
            drop: echo_drop,
        };

        // Create a heap-allocated context for the vtable callbacks.
        let echo_ctx = Box::new(EchoBackendCtx {
            output_cb: None,
            output_ctx: std::ptr::null_mut(),
            bytes_echoed: 0,
        });
        let ctx_ptr = Box::into_raw(echo_ctx) as *mut c_void;

        let result: OpenSessionResult =
            (self.api.open_session)(&meta, &vtable, ctx_ptr);

        // Store the output callback on the context so write can use it.
        let echo_ctx = unsafe { &mut *(ctx_ptr as *mut EchoBackendCtx) };
        echo_ctx.output_cb = Some(result.output_cb);
        echo_ctx.output_ctx = result.output_ctx;

        // Send a welcome message.
        let welcome = b"\x1b[32mEcho Terminal\x1b[0m\r\nType anything -- it will be echoed back.\r\n\r\n";
        (result.output_cb)(result.output_ctx, welcome.as_ptr(), welcome.len());

        self.session = Some(EchoSession {
            handle: result.handle,
            output_cb: result.output_cb,
            output_ctx: result.output_ctx,
            bytes_echoed: 0,
        });

        let msg = CString::new("Echo session opened").unwrap();
        (self.api.log)(2, msg.as_ptr());

        // Keep CStrings alive.
        let _ = (title, short_title, session_type);
    }

    fn close_echo_session(&mut self) {
        if let Some(session) = self.session.take() {
            (self.api.close_session)(session.handle);
            let msg = CString::new("Echo session closed").unwrap();
            (self.api.log)(2, msg.as_ptr());
        }
    }

    fn render(&self) -> Vec<Widget> {
        let mut widgets = vec![
            Widget::heading("Echo Session"),
            Widget::Separator,
        ];

        if let Some(session) = &self.session {
            widgets.push(Widget::KeyValue {
                key: "Status".into(),
                value: "Open".into(),
            });
            widgets.push(Widget::KeyValue {
                key: "Session ID".into(),
                value: session.handle.0.to_string(),
            });
            widgets.push(Widget::KeyValue {
                key: "Bytes Echoed".into(),
                value: session.bytes_echoed.to_string(),
            });
            widgets.push(Widget::Separator);
            widgets.push(Widget::button("close", "Close Session"));
        } else {
            widgets.push(Widget::Label {
                text: "No active echo session.".into(),
                style: None,
            });
            widgets.push(Widget::Separator);
            widgets.push(Widget::button("open", "Open Echo Session"));
        }

        widgets
    }

    fn handle_query(&self, method: &str, _args: serde_json::Value) -> serde_json::Value {
        match method {
            "status" => serde_json::json!({
                "active": self.session.is_some(),
                "bytes_echoed": self.session.as_ref().map(|s| s.bytes_echoed).unwrap_or(0),
            }),
            _ => serde_json::json!({ "error": "unknown method" }),
        }
    }
}

// ---------------------------------------------------------------------------
// Echo backend callbacks (C ABI)
// ---------------------------------------------------------------------------

/// Opaque context passed to vtable callbacks.
struct EchoBackendCtx {
    output_cb: Option<OutputCallback>,
    output_ctx: *mut c_void,
    bytes_echoed: u64,
}

/// Write callback — echo bytes back to the terminal.
extern "C" fn echo_write(handle: *mut c_void, buf: *const u8, len: usize) {
    if handle.is_null() || buf.is_null() || len == 0 {
        return;
    }
    let ctx = unsafe { &mut *(handle as *mut EchoBackendCtx) };
    let input = unsafe { std::slice::from_raw_parts(buf, len) };

    if let Some(output_cb) = ctx.output_cb {
        // Echo each byte, converting CR to CR+LF for the terminal.
        for &byte in input {
            if byte == b'\r' || byte == b'\n' {
                let crlf = b"\r\n";
                output_cb(ctx.output_ctx, crlf.as_ptr(), crlf.len());
            } else {
                output_cb(ctx.output_ctx, &byte, 1);
            }
        }
        ctx.bytes_echoed += len as u64;
    }
}

/// Resize callback — no-op for echo.
extern "C" fn echo_resize(_handle: *mut c_void, _cols: u16, _rows: u16) {}

/// Shutdown callback — no-op, cleanup is in drop.
extern "C" fn echo_shutdown(_handle: *mut c_void) {}

/// Drop callback — free the backend context.
extern "C" fn echo_drop(handle: *mut c_void) {
    if !handle.is_null() {
        unsafe {
            drop(Box::from_raw(handle as *mut EchoBackendCtx));
        }
    }
}

// ---------------------------------------------------------------------------
// Plugin declaration
// ---------------------------------------------------------------------------

conch_plugin_sdk::declare_plugin!(
    info: PluginInfo {
        name: c"Echo Session".as_ptr(),
        description: c"Opens an echo terminal for testing session backends".as_ptr(),
        version: c"0.1.0".as_ptr(),
        plugin_type: PluginType::Panel,
        panel_location: PanelLocation::Bottom,
        dependencies: std::ptr::null(),
        num_dependencies: 0,
    },
    state: EchoPlugin,
    setup: |api| EchoPlugin::new(api),
    event: |state, event| state.handle_event(event),
    render: |state| state.render(),
    query: |state, method, args| state.handle_query(method, args),
);

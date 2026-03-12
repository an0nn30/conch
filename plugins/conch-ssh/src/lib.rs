//! Conch SSH Plugin — pseudocode stub for API validation.
//!
//! This file exercises every HostApi function the SSH plugin will need.
//! It validates that the SDK surface area is complete before we build
//! the real implementation. Each function body describes what the real
//! plugin would do.
//!
//! NOT a working plugin — no actual SSH connections. This is a design
//! validation artifact.

mod config;
mod server_tree;
mod session_backend;

use std::collections::HashMap;
use std::ffi::{CStr, CString};

use conch_plugin_sdk::{
    widgets::{PluginEvent, Widget, WidgetEvent},
    HostApi, PanelHandle, PanelLocation, PluginInfo, PluginType,
    SessionHandle, SessionMeta,
};

use crate::config::{ServerEntry, SshConfig};
use crate::server_tree::build_server_tree;
use crate::session_backend::SshSessionBackend;

/// The SSH plugin's runtime state, created in `setup()` and held by the host.
struct SshPlugin {
    api: &'static HostApi,
    _panel: PanelHandle,
    config: SshConfig,
    /// Active SSH sessions, keyed by the host-assigned SessionHandle.
    sessions: HashMap<u64, SshSessionBackend>,
    /// Currently selected node in the server tree.
    selected_node: Option<String>,
    /// Quick-connect input value.
    quick_connect_value: String,
    /// Whether the server tree needs re-rendering.
    dirty: bool,
}

// ---------------------------------------------------------------------------
// Plugin lifecycle (exercised by declare_plugin! macro)
// ---------------------------------------------------------------------------

impl SshPlugin {
    /// Called once when the host loads the plugin.
    ///
    /// Exercises: register_panel, register_service, register_menu_item,
    ///            subscribe, get_config, log
    fn new(api: &'static HostApi) -> Self {
        // Log startup.
        let msg = CString::new("SSH plugin initializing").unwrap();
        (api.log)(2, msg.as_ptr()); // level 2 = info

        // Register our panel in the right sidebar.
        let name = CString::new("Sessions").unwrap();
        let icon = CString::new("server.png").unwrap();
        let panel = (api.register_panel)(PanelLocation::Right, name.as_ptr(), icon.as_ptr());

        // Register services other plugins can query.
        for svc in &["connect", "exec", "get_sessions", "get_handle"] {
            let svc_name = CString::new(*svc).unwrap();
            (api.register_service)(svc_name.as_ptr());
        }

        // Subscribe to app-level events.
        let tab_changed = CString::new("app.tab_changed").unwrap();
        (api.subscribe)(tab_changed.as_ptr());
        let theme_changed = CString::new("app.theme_changed").unwrap();
        (api.subscribe)(theme_changed.as_ptr());

        // Register menu items.
        let menu = CString::new("File").unwrap();
        let label = CString::new("New SSH Connection...").unwrap();
        let action = CString::new("ssh.new_connection").unwrap();
        let keybind = CString::new("cmd+shift+s").unwrap();
        (api.register_menu_item)(menu.as_ptr(), label.as_ptr(), action.as_ptr(), keybind.as_ptr());

        // Load saved servers from plugin config.
        let config = Self::load_config(api);

        SshPlugin {
            api,
            _panel: panel,
            config,
            sessions: HashMap::new(),
            selected_node: None,
            quick_connect_value: String::new(),
            dirty: true,
        }
    }

    /// Load plugin config from the host's per-plugin config store.
    ///
    /// Exercises: get_config
    fn load_config(api: &'static HostApi) -> SshConfig {
        let key = CString::new("servers").unwrap();
        let result = (api.get_config)(key.as_ptr());
        if result.is_null() {
            return SshConfig::default();
        }
        let json_str = unsafe { CStr::from_ptr(result) }.to_str().unwrap_or("{}");
        let config: SshConfig = serde_json::from_str(json_str).unwrap_or_default();
        (api.free_string)(result);
        config
    }

    /// Save plugin config.
    ///
    /// Exercises: set_config
    fn save_config(&self) {
        let key = CString::new("servers").unwrap();
        let json = serde_json::to_string(&self.config).unwrap_or_default();
        let value = CString::new(json).unwrap();
        (self.api.set_config)(key.as_ptr(), value.as_ptr());
    }

    // -----------------------------------------------------------------------
    // Event handling
    // -----------------------------------------------------------------------

    /// Handle all events dispatched by the host.
    ///
    /// Exercises: show_form, show_confirm, show_prompt, show_error,
    ///            open_session, close_session, publish_event, notify,
    ///            clipboard_set, query_plugin
    fn handle_event(&mut self, event: PluginEvent) {
        match event {
            // -- Widget interactions from our panel --
            PluginEvent::Widget(widget_event) => self.handle_widget_event(widget_event),

            // -- Menu item triggered --
            PluginEvent::MenuAction { action } => self.handle_menu_action(&action),

            // -- IPC event from another plugin or the host --
            PluginEvent::BusEvent { event_type, data } => {
                self.handle_bus_event(&event_type, data);
            }

            // -- Another plugin querying our service --
            PluginEvent::BusQuery { request_id: _, method, args } => {
                // This is handled via conch_plugin_query(), not here.
                // But the event system could also route queries as events
                // if we prefer async handling. For now, queries go through
                // the synchronous conch_plugin_query() export.
                let _ = (method, args);
            }

            PluginEvent::ThemeChanged { .. } => {
                // Re-render to pick up new colors (our widgets inherit
                // theme automatically, but badges/icons might change).
                self.dirty = true;
            }

            PluginEvent::Shutdown => {
                // Gracefully close all SSH sessions.
                let handles: Vec<u64> = self.sessions.keys().copied().collect();
                for h in handles {
                    self.disconnect(SessionHandle(h));
                }
            }
        }
    }

    fn handle_widget_event(&mut self, event: WidgetEvent) {
        match event {
            // Quick-connect input changed (for filtering / autocomplete).
            WidgetEvent::ToolbarInputChanged { id, value } if id == "quick_connect" => {
                self.quick_connect_value = value;
            }

            // Quick-connect submitted — parse "user@host:port" and connect.
            WidgetEvent::ToolbarInputSubmit { id, value } if id == "quick_connect" => {
                self.quick_connect(&value);
            }

            // Server tree — node selected.
            WidgetEvent::TreeSelect { id: _, node_id } => {
                self.selected_node = Some(node_id);
                self.dirty = true;
            }

            // Server tree — double-click connects.
            WidgetEvent::TreeActivate { id: _, node_id } => {
                self.connect_to_server(&node_id);
            }

            // Server tree — expand/collapse folder.
            WidgetEvent::TreeToggle { id: _, node_id, expanded } => {
                self.config.set_folder_expanded(&node_id, expanded);
                self.dirty = true;
            }

            // Server tree — context menu action.
            WidgetEvent::TreeContextMenu { id: _, node_id, action } => {
                match action.as_str() {
                    "connect" => self.connect_to_server(&node_id),
                    "edit" => self.edit_server(&node_id),
                    "delete" => self.delete_server(&node_id),
                    "duplicate" => self.duplicate_server(&node_id),
                    "copy_host" => self.copy_host_to_clipboard(&node_id),
                    _ => {}
                }
            }

            // Toolbar "Add Server" button.
            WidgetEvent::ButtonClick { id } if id == "add_server" => {
                self.add_server_dialog(None);
            }

            // Toolbar "Add Folder" button.
            WidgetEvent::ButtonClick { id } if id == "add_folder" => {
                self.add_folder_dialog();
            }

            _ => {}
        }
    }

    fn handle_menu_action(&mut self, action: &str) {
        match action {
            "ssh.new_connection" => self.add_server_dialog(None),
            _ => {}
        }
    }

    fn handle_bus_event(&mut self, event_type: &str, _data: serde_json::Value) {
        match event_type {
            "app.tab_changed" => {
                // Could highlight the active session in the server tree.
                self.dirty = true;
            }
            "app.theme_changed" => {
                self.dirty = true;
            }
            _ => {}
        }
    }

    // -----------------------------------------------------------------------
    // Connection lifecycle
    // -----------------------------------------------------------------------

    /// Connect to a saved server by node ID.
    ///
    /// Exercises: show_form (password prompt), show_confirm (host key),
    ///            open_session, publish_event, notify, show_error
    fn connect_to_server(&mut self, node_id: &str) {
        let server = match self.config.find_server(node_id) {
            Some(s) => s.clone(),
            None => return,
        };

        // If the server requires a password (no key auth), prompt for it.
        // Exercises: show_prompt
        let password = if server.auth_method == "password" {
            let msg = CString::new(format!("Password for {}@{}:", server.user, server.host)).unwrap();
            let default = CString::new("").unwrap();
            let result = (self.api.show_prompt)(msg.as_ptr(), default.as_ptr());
            if result.is_null() {
                return; // User cancelled.
            }
            let pw = unsafe { CStr::from_ptr(result) }.to_str().unwrap_or("").to_string();
            (self.api.free_string)(result);
            Some(pw)
        } else {
            None
        };

        // TODO: Real plugin would:
        // 1. Start async SSH handshake on plugin thread
        // 2. If host key is unknown, call show_confirm() with fingerprint
        // 3. On success, create SshSessionBackend with the SSH channel
        // 4. Call open_session() to create a tab in the host

        // --- Stub: pretend we connected ---
        let backend = SshSessionBackend::new_stub(&server);

        // Open a session tab in the host.
        let title = CString::new(format!("{}@{}", server.user, server.host)).unwrap();
        let short_title = CString::new(server.host.clone()).unwrap();
        let session_type = CString::new("ssh").unwrap();
        let meta = SessionMeta {
            title: title.as_ptr(),
            short_title: short_title.as_ptr(),
            session_type: session_type.as_ptr(),
            icon: std::ptr::null(),
        };

        let vtable = backend.vtable();
        let backend_handle = backend.as_handle();

        // open_session returns everything we need in one struct.
        let result = (self.api.open_session)(
            &meta,
            &vtable,
            backend_handle,
        );
        let session_handle = result.handle;

        // Store the output callback on the backend so it can push data.
        // backend.set_output(result.output_cb, result.output_ctx);

        self.sessions.insert(session_handle.0, backend);

        // Publish event so other plugins (SFTP, Files) know a session is ready.
        let event_type = CString::new("ssh.session_ready").unwrap();
        let event_data = serde_json::json!({
            "session_id": session_handle.0,
            "host": server.host,
            "user": server.user,
            "port": server.port,
        });
        let data_json = CString::new(event_data.to_string()).unwrap();
        let data_bytes = data_json.as_bytes();
        (self.api.publish_event)(event_type.as_ptr(), data_json.as_ptr(), data_bytes.len());

        // Show a toast notification.
        let notif = serde_json::json!({
            "title": "Connected",
            "body": format!("{}@{}", server.user, server.host),
            "level": "info",
            "duration_ms": 3000,
        });
        let notif_json = CString::new(notif.to_string()).unwrap();
        let notif_bytes = notif_json.as_bytes();
        (self.api.notify)(notif_json.as_ptr(), notif_bytes.len());

        self.dirty = true;

        // Keep CStrings alive until after calls.
        let _ = (title, short_title, session_type, password);
    }

    /// Quick-connect: parse "user@host:port" and connect.
    fn quick_connect(&mut self, input: &str) {
        // Parse the input into a temporary ServerEntry.
        let parts: Vec<&str> = input.splitn(2, '@').collect();
        let (user, host_port) = if parts.len() == 2 {
            (parts[0].to_string(), parts[1])
        } else {
            (std::env::var("USER").unwrap_or_else(|_| "root".to_string()), parts[0])
        };

        let parts: Vec<&str> = host_port.rsplitn(2, ':').collect();
        let (host, port) = if parts.len() == 2 {
            (parts[1].to_string(), parts[0].parse().unwrap_or(22))
        } else {
            (parts[0].to_string(), 22u16)
        };

        // Create a temporary server entry and connect.
        let entry = ServerEntry {
            id: "quick_connect".to_string(),
            label: format!("{}@{}:{}", user, host, port),
            host,
            port,
            user,
            auth_method: "key".to_string(),
            key_path: None,
        };

        // TODO: Would call connect_to_server logic with this temporary entry.
        let _ = entry;
    }

    /// Disconnect an active session.
    ///
    /// Exercises: close_session, publish_event
    fn disconnect(&mut self, handle: SessionHandle) {
        if let Some(_backend) = self.sessions.remove(&handle.0) {
            (self.api.close_session)(handle);

            let event_type = CString::new("ssh.session_closed").unwrap();
            let data = serde_json::json!({ "session_id": handle.0 });
            let data_json = CString::new(data.to_string()).unwrap();
            let data_bytes = data_json.as_bytes();
            (self.api.publish_event)(event_type.as_ptr(), data_json.as_ptr(), data_bytes.len());
        }
        self.dirty = true;
    }

    // -----------------------------------------------------------------------
    // Server management dialogs
    // -----------------------------------------------------------------------

    /// Show the "Add/Edit Server" form dialog.
    ///
    /// Exercises: show_form
    fn add_server_dialog(&mut self, existing: Option<&ServerEntry>) {
        let form = serde_json::json!({
            "title": if existing.is_some() { "Edit Server" } else { "Add Server" },
            "fields": [
                { "id": "label", "type": "text", "label": "Name", "value": existing.map(|s| &s.label).unwrap_or(&String::new()) },
                { "id": "host", "type": "text", "label": "Host", "value": existing.map(|s| &s.host).unwrap_or(&String::new()) },
                { "id": "port", "type": "number", "label": "Port", "value": existing.map(|s| s.port).unwrap_or(22) },
                { "id": "user", "type": "text", "label": "Username", "value": existing.map(|s| &s.user).unwrap_or(&String::new()) },
                { "id": "auth_method", "type": "combo", "label": "Auth Method", "options": ["key", "password"], "value": existing.map(|s| s.auth_method.as_str()).unwrap_or("key") },
                { "id": "key_path", "type": "text", "label": "Key Path (optional)", "value": existing.and_then(|s| s.key_path.as_deref()).unwrap_or("") },
            ],
        });

        let json = CString::new(form.to_string()).unwrap();
        let json_bytes = json.as_bytes();
        let result = (self.api.show_form)(json.as_ptr(), json_bytes.len());
        if result.is_null() {
            return; // Cancelled.
        }

        let result_str = unsafe { CStr::from_ptr(result) }.to_str().unwrap_or("{}");
        // Parse form results and create/update the server entry.
        let _form_data: serde_json::Value = serde_json::from_str(result_str).unwrap_or_default();
        (self.api.free_string)(result);

        // TODO: Create/update ServerEntry from form data, save config.
        self.save_config();
        self.dirty = true;
    }

    /// Show "Add Folder" prompt.
    ///
    /// Exercises: show_prompt
    fn add_folder_dialog(&mut self) {
        let msg = CString::new("Folder name:").unwrap();
        let default = CString::new("New Folder").unwrap();
        let result = (self.api.show_prompt)(msg.as_ptr(), default.as_ptr());
        if result.is_null() {
            return;
        }
        let name = unsafe { CStr::from_ptr(result) }.to_str().unwrap_or("").to_string();
        (self.api.free_string)(result);

        self.config.add_folder(&name);
        self.save_config();
        self.dirty = true;
    }

    /// Edit a server.
    fn edit_server(&mut self, node_id: &str) {
        let server = self.config.find_server(node_id).cloned();
        if let Some(s) = server.as_ref() {
            self.add_server_dialog(Some(s));
        }
    }

    /// Delete a server with confirmation.
    ///
    /// Exercises: show_confirm
    fn delete_server(&mut self, node_id: &str) {
        let label = self.config.find_server(node_id)
            .map(|s| s.label.clone())
            .unwrap_or_default();

        let msg = CString::new(format!("Delete \"{}\"?", label)).unwrap();
        let confirmed = (self.api.show_confirm)(msg.as_ptr());
        if confirmed {
            self.config.remove_server(node_id);
            self.save_config();
            self.dirty = true;
        }
    }

    /// Duplicate a server entry.
    fn duplicate_server(&mut self, node_id: &str) {
        if let Some(server) = self.config.find_server(node_id).cloned() {
            let mut dup = server;
            dup.id = uuid_stub();
            dup.label = format!("{} (copy)", dup.label);
            self.config.add_server(dup);
            self.save_config();
            self.dirty = true;
        }
    }

    /// Copy a server's hostname to clipboard.
    ///
    /// Exercises: clipboard_set
    fn copy_host_to_clipboard(&self, node_id: &str) {
        if let Some(server) = self.config.find_server(node_id) {
            let text = CString::new(server.host.clone()).unwrap();
            (self.api.clipboard_set)(text.as_ptr());
        }
    }

    // -----------------------------------------------------------------------
    // Rendering
    // -----------------------------------------------------------------------

    /// Build the widget tree for the Sessions panel.
    ///
    /// Exercises: the full Widget type surface — TreeView, Toolbar, TextInput,
    ///            Button, ContextMenu items, badges
    fn render(&self) -> Vec<Widget> {
        build_server_tree(&self.config, &self.sessions, self.selected_node.as_deref())
    }

    // -----------------------------------------------------------------------
    // Service queries (from other plugins)
    // -----------------------------------------------------------------------

    /// Handle a direct query from another plugin.
    ///
    /// Services: "connect", "exec", "get_sessions", "get_handle"
    fn handle_query(&self, method: &str, args: serde_json::Value) -> serde_json::Value {
        match method {
            "get_sessions" => {
                // Return list of active SSH sessions.
                let sessions: Vec<serde_json::Value> = self.sessions.iter().map(|(id, backend)| {
                    serde_json::json!({
                        "session_id": id,
                        "host": backend.host(),
                        "user": backend.user(),
                    })
                }).collect();
                serde_json::json!(sessions)
            }

            "exec" => {
                // Execute a command on a specific SSH session.
                // Used by SFTP plugin, tunnels plugin, etc.
                let session_id = args["session_id"].as_u64().unwrap_or(0);
                let command = args["command"].as_str().unwrap_or("");
                if let Some(_backend) = self.sessions.get(&session_id) {
                    // TODO: Open a separate SSH channel, run command, return output.
                    serde_json::json!({
                        "status": "ok",
                        "stdout": format!("(stub) executed: {}", command),
                        "exit_code": 0,
                    })
                } else {
                    serde_json::json!({ "status": "error", "message": "session not found" })
                }
            }

            "connect" => {
                // Programmatic connect request from another plugin.
                let host = args["host"].as_str().unwrap_or("");
                let _user = args["user"].as_str().unwrap_or("");
                let _port = args["port"].as_u64().unwrap_or(22);
                // TODO: Would trigger connection flow.
                serde_json::json!({ "status": "ok", "host": host })
            }

            "get_handle" => {
                // Return an opaque handle to the SSH session's underlying
                // channel — used by the SFTP plugin to open an SFTP subsystem.
                let session_id = args["session_id"].as_u64().unwrap_or(0);
                if self.sessions.contains_key(&session_id) {
                    serde_json::json!({ "status": "ok", "session_id": session_id })
                } else {
                    serde_json::json!({ "status": "error", "message": "session not found" })
                }
            }

            _ => serde_json::json!({ "status": "error", "message": "unknown method" }),
        }
    }
}

// ---------------------------------------------------------------------------
// declare_plugin! macro usage — validates the macro works
// ---------------------------------------------------------------------------

conch_plugin_sdk::declare_plugin!(
    info: PluginInfo {
        name: c"SSH Manager".as_ptr(),
        description: c"SSH connections and session management".as_ptr(),
        version: c"0.1.0".as_ptr(),
        plugin_type: PluginType::Panel,
        panel_location: PanelLocation::Right,
        dependencies: std::ptr::null(),
        num_dependencies: 0,
    },
    state: SshPlugin,
    setup: |api| SshPlugin::new(api),
    event: |state, event| state.handle_event(event),
    render: |state| state.render(),
    query: |state, method, args| state.handle_query(method, args),
);

/// Stub UUID generator (real plugin would use `uuid` crate).
fn uuid_stub() -> String {
    "stub-uuid".to_string()
}


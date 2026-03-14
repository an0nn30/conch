//! Conch SSH Plugin — real SSH connections via russh.

mod config;
mod known_hosts;
mod server_tree;
mod session_backend;
mod sftp;
pub(crate) mod sftp_vtable;
mod ssh_config_parser;
mod tunnel;

use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::sync::Arc;

/// Log a message through the HostApi. Levels: 0=trace, 1=debug, 2=info, 3=warn, 4=error.
fn host_log(api: &HostApi, level: u8, msg: &str) {
    if let Ok(c) = CString::new(msg) {
        (api.log)(level, c.as_ptr());
    }
}

/// Expand a leading `~` or `~/` to the user's home directory.
fn expand_tilde(path: &str) -> std::path::PathBuf {
    if path == "~" || path.starts_with("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(&path[2..]);  // skip "~/"
        }
    }
    std::path::PathBuf::from(path)
}

use conch_plugin_sdk::{
    widgets::{PluginEvent, Widget, WidgetEvent},
    HostApi, PanelHandle, PanelLocation, PluginInfo, PluginType,
    SessionHandle, SessionMeta,
};
use russh::client;
use tokio::process::Command;
use tokio::runtime::Runtime;

use crate::config::{ServerEntry, SshConfig};
use crate::server_tree::{build_server_tree, first_matching_server_id};
use crate::session_backend::{SshBackendState, ssh_vtable};
use crate::tunnel::TunnelManager;

/// The SSH plugin's runtime state.
struct SshPlugin {
    api: &'static HostApi,
    _panel: PanelHandle,
    config: SshConfig,
    /// Hosts imported from `~/.ssh/config` (read-only, not persisted).
    ssh_config_entries: Vec<config::ServerEntry>,
    /// Active SSH sessions keyed by host-assigned SessionHandle.
    sessions: HashMap<u64, Box<SshBackendState>>,
    selected_node: Option<String>,
    quick_connect_value: String,
    /// Request focus on the quick connect text input next frame.
    focus_quick_connect: std::cell::Cell<bool>,
    dirty: bool,
    /// Tokio runtime for async SSH operations.
    rt: Runtime,
    /// Manages active port-forwarding tunnels.
    tunnel_manager: TunnelManager,
}

// ---------------------------------------------------------------------------
// Plugin lifecycle
// ---------------------------------------------------------------------------

impl SshPlugin {
    fn new(api: &'static HostApi) -> Self {
        let msg = CString::new("SSH plugin initializing").unwrap();
        (api.log)(2, msg.as_ptr());

        let name = CString::new("Sessions").unwrap();
        let icon = CString::new("server.png").unwrap();
        let panel = (api.register_panel)(PanelLocation::Right, name.as_ptr(), icon.as_ptr());

        for svc in &[
            "connect", "exec", "get_sessions", "get_handle",
            "list_dir", "stat", "read_file", "write_file", "mkdir", "rename", "delete",
        ] {
            let svc_name = CString::new(*svc).unwrap();
            (api.register_service)(svc_name.as_ptr());
        }

        let tab_changed = CString::new("app.tab_changed").unwrap();
        (api.subscribe)(tab_changed.as_ptr());
        let theme_changed = CString::new("app.theme_changed").unwrap();
        (api.subscribe)(theme_changed.as_ptr());

        let menu = CString::new("File").unwrap();
        let label = CString::new("New SSH Connection...").unwrap();
        let action = CString::new("ssh.new_connection").unwrap();
        let keybind = CString::new("cmd+shift+s").unwrap();
        (api.register_menu_item)(
            menu.as_ptr(), label.as_ptr(),
            action.as_ptr(), keybind.as_ptr(),
        );

        let tools_menu = CString::new("Tools").unwrap();
        let tunnels_label = CString::new("Manage SSH Tunnels\u{2026}").unwrap();
        let tunnels_action = CString::new("ssh.manage_tunnels").unwrap();
        let tunnels_keybind = CString::new("cmd+shift+t").unwrap();
        (api.register_menu_item)(
            tools_menu.as_ptr(), tunnels_label.as_ptr(),
            tunnels_action.as_ptr(), tunnels_keybind.as_ptr(),
        );

        // Quick connect search bar (no menu entry, keybinding only).
        let hidden_menu = CString::new("_hidden").unwrap();
        let qc_label = CString::new("Quick Connect").unwrap();
        let qc_action = CString::new("ssh.focus_quick_connect").unwrap();
        let qc_keybind = CString::new("cmd+/").unwrap();
        (api.register_menu_item)(
            hidden_menu.as_ptr(), qc_label.as_ptr(),
            qc_action.as_ptr(), qc_keybind.as_ptr(),
        );

        let config = Self::load_config(api);
        let ssh_config_entries = ssh_config_parser::parse_ssh_config();

        let rt = Runtime::new().expect("failed to create tokio runtime");

        SshPlugin {
            api,
            _panel: panel,
            config,
            ssh_config_entries,
            sessions: HashMap::new(),
            selected_node: None,
            quick_connect_value: String::new(),
            focus_quick_connect: std::cell::Cell::new(false),
            dirty: true,
            rt,
            tunnel_manager: TunnelManager::new(),
        }
    }

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

    fn save_config(&self) {
        let key = CString::new("servers").unwrap();
        let json = serde_json::to_string(&self.config).unwrap_or_default();
        let value = CString::new(json).unwrap();
        (self.api.set_config)(key.as_ptr(), value.as_ptr());
    }

    // -----------------------------------------------------------------------
    // Event handling
    // -----------------------------------------------------------------------

    fn handle_event(&mut self, event: PluginEvent) {
        match event {
            PluginEvent::Widget(widget_event) => self.handle_widget_event(widget_event),
            PluginEvent::MenuAction { action } => self.handle_menu_action(&action),
            PluginEvent::BusEvent { event_type, data } => {
                self.handle_bus_event(&event_type, data);
            }
            PluginEvent::BusQuery { .. } => {}
            PluginEvent::ThemeChanged { .. } => {
                self.dirty = true;
            }
            PluginEvent::Shutdown => {
                self.rt.block_on(self.tunnel_manager.stop_all());
                let handles: Vec<u64> = self.sessions.keys().copied().collect();
                for h in handles {
                    self.disconnect(SessionHandle(h));
                }
            }
        }
    }

    fn handle_widget_event(&mut self, event: WidgetEvent) {
        match event {
            WidgetEvent::TextInputChanged { id, value } if id == "quick_connect" => {
                self.quick_connect_value = value;
            }
            WidgetEvent::TextInputSubmit { id, value } if id == "quick_connect" => {
                // If the filter matches an existing server, connect to it.
                // Otherwise, treat the input as a user@host:port connection string.
                if let Some(server_id) = first_matching_server_id(&self.config, &self.ssh_config_entries, &value) {
                    self.connect_to_server(&server_id);
                } else {
                    self.quick_connect(&value);
                }
                self.quick_connect_value.clear();
            }
            WidgetEvent::TreeSelect { id: _, node_id } => {
                self.selected_node = Some(node_id);
                self.dirty = true;
            }
            WidgetEvent::TreeActivate { id: _, node_id } => {
                self.connect_to_server(&node_id);
            }
            WidgetEvent::TreeToggle { id: _, node_id, expanded } => {
                self.config.set_folder_expanded(&node_id, expanded);
                self.dirty = true;
            }
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
            WidgetEvent::ButtonClick { id } if id == "add_server" => {
                self.add_server_dialog(None);
            }
            WidgetEvent::ButtonClick { id } if id == "add_folder" => {
                self.add_folder_dialog();
            }
            _ => {}
        }
    }

    fn handle_menu_action(&mut self, action: &str) {
        match action {
            "ssh.new_connection" => self.add_server_dialog(None),
            "ssh.manage_tunnels" => self.tunnel_manager_dialog(),
            "ssh.focus_quick_connect" => {
                self.focus_quick_connect.set(true);
                self.dirty = true;
            }
            _ => {}
        }
    }

    fn handle_bus_event(&mut self, event_type: &str, _data: serde_json::Value) {
        match event_type {
            "app.tab_changed" | "app.theme_changed" => {
                self.dirty = true;
            }
            _ => {}
        }
    }

    // -----------------------------------------------------------------------
    // Connection lifecycle
    // -----------------------------------------------------------------------

    fn connect_to_server(&mut self, node_id: &str) {
        let server = match self.config.find_server(node_id)
            .or_else(|| self.ssh_config_entries.iter().find(|s| s.id == node_id))
        {
            Some(s) => s.clone(),
            None => return,
        };

        let connect_result = do_ssh_connect_sync(
            &server, self.api, &self.rt,
        );

        match connect_result {
            Ok((session_handle, backend_state)) => {
                self.sessions.insert(session_handle.0, backend_state);

                // Publish event.
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

                // Toast notification.
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
            }
            Err(e) => {
                let title = CString::new("Connection Failed").unwrap();
                let msg = CString::new(format!("{e}")).unwrap();
                (self.api.show_error)(title.as_ptr(), msg.as_ptr());
            }
        }
    }

    fn quick_connect(&mut self, input: &str) {
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

        let entry = ServerEntry {
            id: uuid::Uuid::new_v4().to_string(),
            label: format!("{user}@{host}:{port}"),
            host,
            port,
            user,
            auth_method: "key".to_string(),
            key_path: None,
            proxy_command: None,
            proxy_jump: None,
        };

        // Attempt key-based auth first, then fall back to password.
        let server_id = entry.id.clone();
        self.config.add_server(entry);
        self.connect_to_server(&server_id);
    }

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
    // Tunnel management
    // -----------------------------------------------------------------------

    /// Activate a saved tunnel in the background. Spawns a std::thread that
    /// establishes the SSH connection (with prompts for host key / password)
    /// and starts the port forward. Returns immediately.
    fn activate_tunnel(&mut self, tunnel_id: uuid::Uuid) {
        let tunnel = match self.config.find_tunnel(&tunnel_id) {
            Some(t) => t.clone(),
            None => return,
        };

        // Find the server entry that matches the tunnel's session_key.
        let server = match self.find_server_for_tunnel(&tunnel.session_key) {
            Some(s) => s,
            None => {
                let title = CString::new("Tunnel Error").unwrap();
                let msg = CString::new(format!(
                    "No server configured for {}.",
                    tunnel.session_key,
                )).unwrap();
                (self.api.show_error)(title.as_ptr(), msg.as_ptr());
                return;
            }
        };

        // Mark as connecting.
        let mgr = self.tunnel_manager.clone();
        self.rt.block_on(mgr.set_connecting(tunnel.id));

        let api = self.api;
        let label = tunnel.label.clone();

        // Spawn a named background thread so HostApi functions (show_form,
        // show_confirm, etc.) resolve the correct plugin name and viewport.
        std::thread::Builder::new()
            .name("plugin:conch-ssh".into())
            .spawn(move || {
                let rt = tokio::runtime::Runtime::new().expect("tunnel runtime");
                let (prompt_tx, mut prompt_rx) =
                    tokio::sync::mpsc::channel::<tunnel::PromptRequest>(4);
                let (result_tx, result_rx) = std::sync::mpsc::channel();

                let mgr_inner = mgr.clone();
                let server_inner = server.clone();
                rt.spawn(async move {
                    let r = mgr_inner
                        .start_tunnel(
                            tunnel.id,
                            &server_inner,
                            tunnel.local_port,
                            tunnel.remote_host.clone(),
                            tunnel.remote_port,
                            prompt_tx,
                        )
                        .await;
                    let _ = result_tx.send(r);
                });

                // Service prompt requests from the SSH handler while
                // waiting for the connection result. The handler holds a
                // clone of prompt_tx that lives as long as the SSH
                // connection, so we can't rely on channel closure. Instead,
                // poll result_rx between prompt requests.
                let result = loop {
                    // Check if connection finished.
                    match result_rx.try_recv() {
                        Ok(r) => break r,
                        Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                            break Err("connection lost".into());
                        }
                        Err(std::sync::mpsc::TryRecvError::Empty) => {}
                    }
                    // Wait briefly for a prompt request, then loop back to
                    // check result_rx again.
                    match prompt_rx.try_recv() {
                        Ok(req) => handle_tunnel_prompt(api, req),
                        Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                            break result_rx
                                .recv()
                                .unwrap_or_else(|_| Err("connection lost".into()));
                        }
                        Err(tokio::sync::mpsc::error::TryRecvError::Empty) => {
                            std::thread::sleep(std::time::Duration::from_millis(50));
                        }
                    }
                };

                match result {
                    Ok(()) => {
                        let notif = serde_json::json!({
                            "title": "Tunnel Active",
                            "body": label,
                            "level": "info",
                            "duration_ms": 3000,
                        });
                        let json = CString::new(notif.to_string()).unwrap();
                        let bytes = json.as_bytes();
                        (api.notify)(json.as_ptr(), bytes.len());
                    }
                    Err(e) => {
                        log::error!("tunnel activation failed: {e}");
                        rt.block_on(mgr.set_error(&tunnel.id, e.clone()));
                        let title = CString::new("Tunnel Error").unwrap();
                        let msg = CString::new(e).unwrap();
                        (api.show_error)(title.as_ptr(), msg.as_ptr());
                    }
                }
            })
            .ok();
    }

    /// Find a ServerEntry matching a tunnel session_key (user@host:port).
    fn find_server_for_tunnel(&self, session_key: &str) -> Option<ServerEntry> {
        let all_servers = self.config.ungrouped.iter()
            .chain(self.config.folders.iter().flat_map(|f| f.entries.iter()))
            .chain(self.ssh_config_entries.iter());

        for s in all_servers {
            if tunnel::SavedTunnel::make_session_key(&s.user, &s.host, s.port) == session_key {
                return Some(s.clone());
            }
        }

        // If no exact match, try parsing the session_key and creating a
        // minimal entry (for servers that were removed from config but
        // tunnels still reference them).
        tunnel::SavedTunnel::parse_session_key(session_key).map(|(user, host, port)| {
            ServerEntry {
                id: String::new(),
                label: session_key.to_string(),
                host,
                port,
                user,
                auth_method: "key".to_string(),
                key_path: None,
                proxy_command: None,
                proxy_jump: None,
            }
        })
    }

    /// Stop an active tunnel.
    fn stop_tunnel(&mut self, tunnel_id: uuid::Uuid) {
        let mgr = self.tunnel_manager.clone();
        self.rt.block_on(mgr.stop(&tunnel_id));
    }


    // -----------------------------------------------------------------------
    // Server management dialogs
    // -----------------------------------------------------------------------

    fn add_server_dialog(&mut self, existing: Option<&ServerEntry>) {
        let default_user = std::env::var("USER").unwrap_or_default();
        let is_editing = existing.is_some();

        // Determine which proxy type is active for the collapsible.
        let proxy_type = if existing.and_then(|s| s.proxy_jump.as_ref()).is_some() {
            "ProxyJump"
        } else if existing.and_then(|s| s.proxy_command.as_ref()).is_some() {
            "ProxyCommand"
        } else {
            "None"
        };
        let has_proxy = proxy_type != "None";

        // Build folder options for the "Save to folder" dropdown.
        let mut folder_options: Vec<String> = vec!["(none)".to_string()];
        folder_options.extend(self.config.folders.iter().map(|f| f.name.clone()));

        // Default folder: if editing and server is in a folder, select that.
        // If there's a selected folder in the tree, use that.
        let default_folder = if is_editing {
            existing
                .and_then(|e| self.config.find_server_folder(&e.id))
                .and_then(|fid| self.config.folders.iter().find(|f| f.id == fid))
                .map(|f| f.name.clone())
                .unwrap_or_else(|| "(none)".to_string())
        } else {
            self.selected_node
                .as_deref()
                .and_then(|sel| {
                    // If a folder is selected, use it; if a server in a folder is selected, use its folder.
                    self.config.folders.iter().find(|f| f.id == sel)
                        .or_else(|| {
                            self.config.find_server_folder(sel)
                                .and_then(|fid| self.config.folders.iter().find(|f| f.id == fid))
                        })
                })
                .map(|f| f.name.clone())
                .unwrap_or_else(|| "(none)".to_string())
        };

        let form = serde_json::json!({
            "title": if is_editing { "Edit SSH Connection" } else { "New SSH Connection" },
            "min_width": 460,
            "label_width": 130,
            "fields": [
                { "type": "text", "id": "label", "label": "Session Name:", "hint": "optional",
                  "value": existing.map(|s| s.label.as_str()).unwrap_or("") },
                { "type": "host_port", "host_id": "host", "port_id": "port", "label": "Host / IP:",
                  "host_value": existing.map(|s| s.host.as_str()).unwrap_or(""),
                  "port_value": existing.map(|s| s.port.to_string()).unwrap_or_else(|| "22".to_string()) },
                { "type": "text", "id": "user", "label": "Username:",
                  "value": existing.map(|s| s.user.as_str()).unwrap_or(default_user.as_str()) },
                { "type": "password", "id": "password", "label": "Password:", "value": "" },
                { "type": "file_picker", "id": "key_path", "label": "Private Key:",
                  "value": existing.and_then(|s| s.key_path.as_deref()).unwrap_or(""),
                  "start_dir": "~/.ssh" },
                { "type": "text", "id": "startup_command", "label": "Startup Command:", "hint": "optional",
                  "value": "" },
                { "type": "collapsible", "label": "Advanced", "expanded": has_proxy, "fields": [
                    { "type": "combo", "id": "proxy_type", "label": "Proxy Type:",
                      "options": ["None", "ProxyJump", "ProxyCommand"], "value": proxy_type },
                    { "type": "text", "id": "proxy_value", "label": "Proxy Value:", "hint": "user@jumphost or ssh -W %h:%p host",
                      "value": existing.and_then(|s| s.proxy_jump.as_deref().or(s.proxy_command.as_deref())).unwrap_or("") },
                ]},
                { "type": "separator" },
                { "type": "combo", "id": "folder", "label": "Save to folder:",
                  "options": folder_options, "value": default_folder },
            ],
            "buttons": [
                { "id": "cancel", "label": "Cancel" },
                { "id": "save", "label": "Save", "enabled_when": "host" },
                { "id": "save_connect", "label": "Save & Connect", "enabled_when": "host" },
            ],
        });

        let json = CString::new(form.to_string()).unwrap();
        let json_bytes = json.as_bytes();
        let result = (self.api.show_form)(json.as_ptr(), json_bytes.len());
        if result.is_null() {
            return;
        }

        let result_str = unsafe { CStr::from_ptr(result) }.to_str().unwrap_or("{}");
        let form_data: serde_json::Value = serde_json::from_str(result_str).unwrap_or_default();
        (self.api.free_string)(result);

        let action = form_data["_action"].as_str().unwrap_or("");
        let label = form_data["label"].as_str().unwrap_or("").to_string();
        let host = form_data["host"].as_str().unwrap_or("").to_string();
        let port: u16 = form_data["port"].as_str()
            .and_then(|s| s.parse().ok())
            .unwrap_or(22);
        let user = form_data["user"].as_str().unwrap_or("").to_string();
        let password = form_data["password"].as_str().unwrap_or("").to_string();
        let key_path = form_data["key_path"].as_str()
            .filter(|s| !s.is_empty())
            .map(String::from);

        // Determine auth method from the form data.
        let auth_method = if !password.is_empty() {
            "password".to_string()
        } else if key_path.is_some() {
            "key".to_string()
        } else {
            "key".to_string()
        };

        // Parse proxy settings.
        let proxy_type_str = form_data["proxy_type"].as_str().unwrap_or("None");
        let proxy_val = form_data["proxy_value"].as_str().unwrap_or("").to_string();
        let proxy_jump = if proxy_type_str == "ProxyJump" && !proxy_val.is_empty() {
            Some(proxy_val.clone())
        } else {
            None
        };
        let proxy_command = if proxy_type_str == "ProxyCommand" && !proxy_val.is_empty() {
            Some(proxy_val)
        } else {
            None
        };

        if host.is_empty() {
            return;
        }

        let id = existing
            .map(|e| e.id.clone())
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

        if existing.is_some() {
            self.config.remove_server(&id);
        }

        // Determine target folder.
        let folder_name = form_data["folder"].as_str().unwrap_or("(none)");
        let target_folder_id = self.config.folders.iter()
            .find(|f| f.name == folder_name)
            .map(|f| f.id.clone());

        let entry = ServerEntry {
            id,
            label: if label.is_empty() { format!("{user}@{host}") } else { label },
            host,
            port,
            user,
            auth_method,
            key_path,
            proxy_jump,
            proxy_command,
        };

        if let Some(fid) = &target_folder_id {
            self.config.add_server_to_folder(entry.clone(), fid);
        } else {
            self.config.add_server(entry.clone());
        }

        self.save_config();
        self.dirty = true;

        // If "Save & Connect", immediately connect.
        if action == "save_connect" {
            self.connect_to_server(&entry.id);
        }
    }

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

    fn edit_server(&mut self, node_id: &str) {
        let server = self.config.find_server(node_id).cloned();
        if let Some(s) = server.as_ref() {
            self.add_server_dialog(Some(s));
        }
    }

    fn delete_server(&mut self, node_id: &str) {
        let label = self.config.find_server(node_id)
            .map(|s| s.label.clone())
            .unwrap_or_default();

        let msg = CString::new(format!("Delete \"{label}\"?")).unwrap();
        let confirmed = (self.api.show_confirm)(msg.as_ptr());
        if confirmed {
            self.config.remove_server(node_id);
            self.save_config();
            self.dirty = true;
        }
    }

    fn duplicate_server(&mut self, node_id: &str) {
        if let Some(server) = self.config.find_server(node_id).cloned() {
            let mut dup = server;
            dup.id = uuid::Uuid::new_v4().to_string();
            dup.label = format!("{} (copy)", dup.label);
            self.config.add_server(dup);
            self.save_config();
            self.dirty = true;
        }
    }

    fn copy_host_to_clipboard(&self, node_id: &str) {
        if let Some(server) = self.config.find_server(node_id) {
            let text = CString::new(server.host.clone()).unwrap();
            (self.api.clipboard_set)(text.as_ptr());
        }
    }

    // -----------------------------------------------------------------------
    // Tunnel management dialogs
    // -----------------------------------------------------------------------

    fn tunnel_manager_dialog(&mut self) {
        loop {
            // Build table rows from saved tunnels with live status.
            let rows: Vec<serde_json::Value> = self.config.tunnels.iter().map(|t| {
                let status_info = self.rt.block_on(self.tunnel_manager.status(&t.id));
                let (status_text, color) = match &status_info {
                    Some(tunnel::TunnelStatus::Active) => ("\u{25CF} Active", Some("green")),
                    Some(tunnel::TunnelStatus::Connecting) => ("\u{25CB} Connecting\u{2026}", Some("yellow")),
                    Some(tunnel::TunnelStatus::Error(_)) => ("\u{25CF} Error", Some("red")),
                    None => ("\u{25CB} Inactive", None),
                };
                let remote = format!("{}:{}", t.remote_host, t.remote_port);
                serde_json::json!({
                    "id": t.id.to_string(),
                    "cells": [status_text, &t.label, t.local_port.to_string(), remote, &t.session_key],
                    "color": color,
                })
            }).collect();

            // Check if any tunnels are in a transient state (Connecting).
            let has_connecting = self.config.tunnels.iter().any(|t| {
                matches!(
                    self.rt.block_on(self.tunnel_manager.status(&t.id)),
                    Some(tunnel::TunnelStatus::Connecting)
                )
            });

            let mut form = serde_json::json!({
                "title": "SSH Tunnels",
                "min_width": 620,
                "label_width": 0,
                "fields": [
                    { "type": "selectable_table", "id": "selected_tunnel",
                      "columns": ["Status", "Label", "Local Port", "Remote", "Via"],
                      "rows": rows,
                      "value": "" },
                ],
                "buttons": [
                    { "id": "close", "label": "Close" },
                    { "id": "stop", "label": "Stop" },
                    { "id": "delete", "label": "Delete" },
                    { "id": "activate", "label": "Activate" },
                    { "id": "new_tunnel", "label": "New Tunnel\u{2026}" },
                ],
            });

            // Auto-refresh while tunnels are connecting so status updates.
            if has_connecting {
                form["auto_refresh_ms"] = serde_json::json!(1000);
            }

            let json = CString::new(form.to_string()).unwrap();
            let json_bytes = json.as_bytes();
            let result = (self.api.show_form)(json.as_ptr(), json_bytes.len());
            if result.is_null() {
                return;
            }

            let result_str = unsafe { CStr::from_ptr(result) }.to_str().unwrap_or("{}");
            let form_data: serde_json::Value = serde_json::from_str(result_str).unwrap_or_default();
            (self.api.free_string)(result);

            let action = form_data["_action"].as_str().unwrap_or("");
            let selected_id = form_data["selected_tunnel"].as_str().unwrap_or("");

            match action {
                "new_tunnel" => {
                    self.new_tunnel_dialog();
                    // Loop back to show the manager again.
                }
                "activate" => {
                    if let Ok(id) = uuid::Uuid::parse_str(selected_id) {
                        // Non-blocking — spawns a background thread.
                        self.activate_tunnel(id);
                        // Loop back immediately to show updated status.
                    }
                }
                "stop" => {
                    if let Ok(id) = uuid::Uuid::parse_str(selected_id) {
                        self.stop_tunnel(id);
                    }
                }
                "delete" => {
                    if let Ok(id) = uuid::Uuid::parse_str(selected_id) {
                        self.stop_tunnel(id);
                        self.config.remove_tunnel(&id);
                        self.save_config();
                    }
                }
                "_refresh" => {
                    // Auto-refresh: just loop back to rebuild with fresh status.
                }
                "close" | _ => return,
            }
        }
    }

    fn new_tunnel_dialog(&mut self) {
        // Build server options from all configured + ssh_config servers.
        let mut server_options: Vec<String> = Vec::new();
        let all_servers: Vec<&ServerEntry> = self.config.ungrouped.iter()
            .chain(self.config.folders.iter().flat_map(|f| f.entries.iter()))
            .chain(self.ssh_config_entries.iter())
            .collect();

        for s in &all_servers {
            let key = tunnel::SavedTunnel::make_session_key(&s.user, &s.host, s.port);
            let label = format!("{} \u{2014} {key}", s.label);
            server_options.push(label);
        }

        let default_server = server_options.first().cloned().unwrap_or_default();

        let form = serde_json::json!({
            "title": "New SSH Tunnel",
            "min_width": 420,
            "label_width": 100,
            "fields": [
                { "type": "combo", "id": "server", "label": "SSH Server:",
                  "options": server_options, "value": default_server },
                { "type": "text", "id": "local_port", "label": "Local Port:", "value": "" },
                { "type": "text", "id": "remote_host", "label": "Remote Host:", "value": "localhost" },
                { "type": "text", "id": "remote_port", "label": "Remote Port:", "value": "" },
                { "type": "text", "id": "label", "label": "Label (opt.):", "value": "" },
            ],
            "buttons": [
                { "id": "cancel", "label": "Cancel" },
                { "id": "save_connect", "label": "Save & Connect", "enabled_when": "local_port" },
            ],
        });

        let json = CString::new(form.to_string()).unwrap();
        let json_bytes = json.as_bytes();
        let result = (self.api.show_form)(json.as_ptr(), json_bytes.len());
        if result.is_null() {
            return;
        }

        let result_str = unsafe { CStr::from_ptr(result) }.to_str().unwrap_or("{}");
        let form_data: serde_json::Value = serde_json::from_str(result_str).unwrap_or_default();
        (self.api.free_string)(result);

        let action = form_data["_action"].as_str().unwrap_or("");
        if action != "save_connect" {
            return;
        }

        let server_label = form_data["server"].as_str().unwrap_or("");
        let local_port: u16 = form_data["local_port"].as_str()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let remote_host = form_data["remote_host"].as_str().unwrap_or("localhost").to_string();
        let remote_port: u16 = form_data["remote_port"].as_str()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let label = form_data["label"].as_str().unwrap_or("").to_string();

        if local_port == 0 || remote_port == 0 {
            let title = CString::new("Invalid Tunnel").unwrap();
            let msg = CString::new("Local port and remote port must be valid numbers (1-65535).").unwrap();
            (self.api.show_error)(title.as_ptr(), msg.as_ptr());
            return;
        }

        // Parse the session key from the server dropdown selection.
        // Format is "label — user@host:port"
        let session_key = server_label
            .split(" \u{2014} ")
            .nth(1)
            .unwrap_or(server_label)
            .to_string();

        let tunnel_label = if label.is_empty() {
            format!(":{local_port} -> {remote_host}:{remote_port}")
        } else {
            label
        };

        let tunnel = tunnel::SavedTunnel {
            id: uuid::Uuid::new_v4(),
            label: tunnel_label,
            session_key,
            local_port,
            remote_host,
            remote_port,
            auto_start: false,
        };

        let tunnel_id = tunnel.id;
        self.config.add_tunnel(tunnel);
        self.save_config();

        // Start connecting in the background.
        self.activate_tunnel(tunnel_id);
    }

    // -----------------------------------------------------------------------
    // Rendering
    // -----------------------------------------------------------------------

    fn render(&self) -> Vec<Widget> {
        let focus = self.focus_quick_connect.replace(false);
        build_server_tree(&self.config, &self.ssh_config_entries, &self.sessions, self.selected_node.as_deref(), &self.quick_connect_value, focus)
    }

    // -----------------------------------------------------------------------
    // Service queries
    // -----------------------------------------------------------------------

    fn handle_query(&mut self, method: &str, args: serde_json::Value) -> serde_json::Value {
        match method {
            "get_sessions" => {
                let sessions: Vec<serde_json::Value> = self.sessions.iter().map(|(id, backend)| {
                    serde_json::json!({
                        "session_id": id,
                        "host": backend.host(),
                        "port": backend.port(),
                        "user": backend.user(),
                        "status": if backend.connected { "connected" } else { "connecting" },
                    })
                }).collect();
                serde_json::json!(sessions)
            }
            "exec" => {
                let session_id = args["session_id"].as_u64().unwrap_or(0);
                let command = args["command"].as_str().unwrap_or("").to_string();
                match self.sessions.get(&session_id) {
                    Some(backend) if backend.connected => {
                        match self.rt.block_on(backend.exec(&command)) {
                            Ok((stdout, stderr, exit_code)) => {
                                serde_json::json!({
                                    "status": "ok",
                                    "stdout": stdout,
                                    "stderr": stderr,
                                    "exit_code": exit_code,
                                })
                            }
                            Err(e) => {
                                serde_json::json!({ "status": "error", "message": e })
                            }
                        }
                    }
                    Some(_) => {
                        serde_json::json!({ "status": "error", "message": "session not connected" })
                    }
                    None => {
                        serde_json::json!({ "status": "error", "message": "session not found" })
                    }
                }
            }
            "connect" => {
                // Support connecting by server_name (label) or by explicit host/user/port.
                if let Some(server_name) = args["server_name"].as_str() {
                    if let Some(server) = self.config.find_server_by_label(server_name).cloned() {
                        self.connect_to_server(&server.id);
                        // Find the session that was just created for this server.
                        let session_id = self.sessions.keys().max().copied().unwrap_or(0);
                        serde_json::json!({ "status": "ok", "session_id": session_id })
                    } else {
                        serde_json::json!({ "status": "error", "message": "server not found" })
                    }
                } else {
                    let host = args["host"].as_str().unwrap_or("").to_string();
                    let port = args["port"].as_u64().unwrap_or(22) as u16;
                    let user = args["user"].as_str()
                        .map(String::from)
                        .unwrap_or_else(|| std::env::var("USER").unwrap_or_else(|_| "root".to_string()));
                    let auth_method = args["auth_method"].as_str().unwrap_or("key").to_string();

                    if host.is_empty() {
                        return serde_json::json!({ "status": "error", "message": "host is required" });
                    }

                    let entry = ServerEntry {
                        id: uuid::Uuid::new_v4().to_string(),
                        label: format!("{user}@{host}:{port}"),
                        host,
                        port,
                        user,
                        auth_method,
                        key_path: args["key_path"].as_str().map(String::from),
                        proxy_command: None,
                        proxy_jump: None,
                    };

                    let server_id = entry.id.clone();
                    self.config.add_server(entry);
                    self.connect_to_server(&server_id);
                    let session_id = self.sessions.keys().max().copied().unwrap_or(0);
                    serde_json::json!({ "status": "ok", "session_id": session_id })
                }
            }
            "get_handle" => {
                let session_id = args["session_id"].as_u64().unwrap_or(0);
                if let Some(backend) = self.sessions.get(&session_id) {
                    serde_json::json!({
                        "status": "ok",
                        "session_id": session_id,
                        "host": backend.host(),
                        "port": backend.port(),
                        "user": backend.user(),
                        "connected": backend.connected,
                    })
                } else {
                    serde_json::json!({ "status": "error", "message": "session not found" })
                }
            }
            // SFTP operations — all require session_id and an SSH handle.
            "list_dir" | "stat" | "read_file" | "write_file" | "mkdir" | "rename" | "delete" | "realpath" => {
                let session_id = args["session_id"].as_u64().unwrap_or(0);
                match self.sessions.get(&session_id) {
                    Some(backend) if backend.connected => {
                        let ssh_handle = backend.ssh_handle().unwrap();
                        let result = self.rt.block_on(async {
                            match method {
                                "list_dir" => {
                                    let path = args["path"].as_str().unwrap_or("/");
                                    sftp::list_dir(ssh_handle, path).await
                                }
                                "stat" => {
                                    let path = args["path"].as_str().unwrap_or("/");
                                    sftp::stat(ssh_handle, path).await
                                }
                                "read_file" => {
                                    let path = args["path"].as_str().unwrap_or("");
                                    let offset = args["offset"].as_u64().unwrap_or(0);
                                    let length = args["length"].as_u64().unwrap_or(4096) as usize;
                                    sftp::read_file(ssh_handle, path, offset, length).await
                                }
                                "write_file" => {
                                    let path = args["path"].as_str().unwrap_or("");
                                    let data = args["data"].as_str().unwrap_or("");
                                    sftp::write_file(ssh_handle, path, data).await
                                }
                                "mkdir" => {
                                    let path = args["path"].as_str().unwrap_or("");
                                    sftp::mkdir(ssh_handle, path).await
                                }
                                "rename" => {
                                    let from = args["from"].as_str().unwrap_or("");
                                    let to = args["to"].as_str().unwrap_or("");
                                    sftp::rename(ssh_handle, from, to).await
                                }
                                "delete" => {
                                    let path = args["path"].as_str().unwrap_or("");
                                    let is_dir = args["is_dir"].as_bool().unwrap_or(false);
                                    if is_dir {
                                        sftp::remove_dir(ssh_handle, path).await
                                    } else {
                                        sftp::remove_file(ssh_handle, path).await
                                    }
                                }
                                "realpath" => {
                                    let path = args["path"].as_str().unwrap_or(".");
                                    sftp::realpath(ssh_handle, path).await
                                }
                                _ => unreachable!(),
                            }
                        });
                        result.unwrap_or_else(|e| serde_json::json!({ "status": "error", "message": e }))
                    }
                    Some(_) => serde_json::json!({ "status": "error", "message": "session not connected" }),
                    None => serde_json::json!({ "status": "error", "message": "session not found" }),
                }
            }
            _ => serde_json::json!({ "status": "error", "message": "unknown method" }),
        }
    }
}

// ---------------------------------------------------------------------------
// SSH connection logic
// ---------------------------------------------------------------------------

/// The russh client handler — implements host key verification via the host
/// session_prompt API, with `~/.ssh/known_hosts` support.
pub(crate) struct SshHandler {
    api: &'static HostApi,
    host: String,
    port: u16,
    /// Session handle for inline prompt rendering.
    session_handle: SessionHandle,
}

#[async_trait::async_trait]
impl client::Handler for SshHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &ssh_key::PublicKey,
    ) -> Result<bool, Self::Error> {
        // Check known_hosts first.
        match known_hosts::check_known_host(&self.host, self.port, server_public_key) {
            Some(true) => {
                host_log(self.api, 1, &format!(
                    "Host key for {}:{} matches known_hosts", self.host, self.port
                ));
                return Ok(true);
            }
            Some(false) => {
                // Key mismatch — possible MITM.
                let fingerprint = server_public_key.fingerprint(ssh_key::HashAlg::Sha256);
                let msg = CString::new(format!(
                    "WARNING: HOST KEY HAS CHANGED for {}:{}\n\n\
                     This could indicate a man-in-the-middle attack.\n\
                     It is also possible that the host key has just been changed.",
                    self.host, self.port
                )).unwrap();
                let detail = CString::new(format!(
                    "{}\n{fingerprint}",
                    server_public_key.algorithm().as_str(),
                )).unwrap();
                let result = (self.api.session_prompt)(
                    self.session_handle, 0, msg.as_ptr(), detail.as_ptr(),
                );
                let accepted = !result.is_null();
                if !result.is_null() {
                    (self.api.free_string)(result);
                }
                return Ok(accepted);
            }
            None => {
                // Unknown host — ask the user.
            }
        }

        let fingerprint = server_public_key.fingerprint(ssh_key::HashAlg::Sha256);
        let msg = CString::new(format!(
            "The authenticity of host '{}' can't be established.",
            if self.port != 22 {
                format!("[{}]:{}", self.host, self.port)
            } else {
                self.host.clone()
            }
        )).unwrap();
        let detail = CString::new(format!(
            "{} key fingerprint is:\n{fingerprint}",
            server_public_key.algorithm().as_str(),
        )).unwrap();
        let result = (self.api.session_prompt)(
            self.session_handle, 0, msg.as_ptr(), detail.as_ptr(),
        );
        let accepted = !result.is_null();
        if !result.is_null() {
            (self.api.free_string)(result);
        }

        if accepted {
            if let Err(e) = known_hosts::add_known_host(&self.host, self.port, server_public_key) {
                host_log(self.api, 3, &format!("Failed to save host key: {e}"));
            } else {
                host_log(self.api, 2, &format!(
                    "Host key for {}:{} saved to known_hosts", self.host, self.port
                ));
            }
        }

        Ok(accepted)
    }
}

/// Handle a prompt request from a background tunnel connection thread.
/// Called from a std::thread named "plugin:conch-ssh" so HostApi functions
/// resolve the correct plugin name and viewport.
fn handle_tunnel_prompt(api: &'static HostApi, req: tunnel::PromptRequest) {
    match req {
        tunnel::PromptRequest::ConfirmHostKey { message, detail, reply } => {
            let form = serde_json::json!({
                "title": "SSH Host Key Verification",
                "min_width": 500,
                "label_width": 0,
                "fields": [
                    { "type": "label", "text": message },
                    { "type": "label", "text": detail },
                    { "type": "label", "text": "Do you want to continue connecting?" },
                ],
                "buttons": [
                    { "id": "reject", "label": "Reject" },
                    { "id": "accept", "label": "Accept & Save" },
                ],
            });
            let json = CString::new(form.to_string()).unwrap();
            let json_bytes = json.as_bytes();
            let result = (api.show_form)(json.as_ptr(), json_bytes.len());
            let accepted = if result.is_null() {
                false
            } else {
                let s = unsafe { CStr::from_ptr(result) }.to_str().unwrap_or("");
                let data: serde_json::Value = serde_json::from_str(s).unwrap_or_default();
                (api.free_string)(result);
                data["_action"].as_str() == Some("accept")
            };
            let _ = reply.send(accepted);
        }
        tunnel::PromptRequest::Password { message, reply } => {
            let form = serde_json::json!({
                "title": "SSH Tunnel Authentication",
                "min_width": 400,
                "label_width": 100,
                "fields": [
                    { "type": "label", "text": message },
                    { "type": "password", "id": "password", "label": "Password:", "value": "" },
                ],
                "buttons": [
                    { "id": "cancel", "label": "Cancel" },
                    { "id": "ok", "label": "Connect", "enabled_when": "password" },
                ],
            });
            let json = CString::new(form.to_string()).unwrap();
            let json_bytes = json.as_bytes();
            let result = (api.show_form)(json.as_ptr(), json_bytes.len());
            let pw = if result.is_null() {
                None
            } else {
                let s = unsafe { CStr::from_ptr(result) }.to_str().unwrap_or("");
                let data: serde_json::Value = serde_json::from_str(s).unwrap_or_default();
                (api.free_string)(result);
                if data["_action"].as_str() == Some("ok") {
                    data["password"].as_str().map(String::from)
                } else {
                    None
                }
            };
            let _ = reply.send(pw);
        }
    }
}

/// Set session status via the HostApi.
fn set_session_status(api: &HostApi, handle: SessionHandle, status: conch_plugin_sdk::SessionStatus, detail: Option<&str>) {
    let c_detail = detail.and_then(|d| CString::new(d).ok());
    let detail_ptr = c_detail.as_ref().map(|c| c.as_ptr()).unwrap_or(std::ptr::null());
    (api.set_session_status)(handle, status, detail_ptr);
}

/// Open a tab immediately (in "Connecting" state), then perform the SSH
/// handshake. On success the tab transitions to "Connected" and the terminal
/// becomes live. On failure the tab shows an error screen.
///
/// Synchronous — blocks the plugin thread.
fn do_ssh_connect_sync(
    server: &ServerEntry,
    api: &'static HostApi,
    rt: &Runtime,
) -> Result<(SessionHandle, Box<SshBackendState>), String> {
    // Phase 1: Open session tab immediately (shows "Connecting..." screen).
    let mut backend_state = SshBackendState::new_preallocated(
        server.host.clone(),
        server.user.clone(),
        server.port,
    );

    let title = CString::new(format!("{}@{}", server.user, server.host)).unwrap();
    let short_title = CString::new(server.host.clone()).unwrap();
    let session_type = CString::new("ssh").unwrap();
    let meta = SessionMeta {
        title: title.as_ptr(),
        short_title: short_title.as_ptr(),
        session_type: session_type.as_ptr(),
        icon: std::ptr::null(),
    };

    let vtable = ssh_vtable();
    let backend_handle = SshBackendState::as_handle_ptr(&mut backend_state);
    let open_result = (api.open_session)(&meta, &vtable, backend_handle);
    let session_handle = open_result.handle;

    if session_handle.0 == 0 {
        return Err("Host refused to open session tab".to_string());
    }

    // Set connecting status with detail.
    let detail = format!("{}@{}:{}", server.user, server.host, server.port);
    set_session_status(api, session_handle, conch_plugin_sdk::SessionStatus::Connecting, Some(&detail));

    // Password prompt (inline in session tab) if needed.
    let password = if server.auth_method == "password" {
        let msg = CString::new(format!("Password for {}@{}", server.user, server.host)).unwrap();
        let detail_c = CString::new(detail.clone()).unwrap();
        let result = (api.session_prompt)(session_handle, 1, msg.as_ptr(), detail_c.as_ptr());
        if result.is_null() {
            // User cancelled — close the session tab.
            (api.close_session)(session_handle);
            return Err("Password entry cancelled".to_string());
        }
        let pw = unsafe { CStr::from_ptr(result) }.to_str().unwrap_or("").to_string();
        (api.free_string)(result);
        Some(pw)
    } else {
        None
    };

    // Phase 2: SSH handshake (async).
    host_log(api, 2, &format!("SSH connect: {}@{}:{} auth={} key={:?} proxy_jump={:?} proxy_cmd={:?}",
        server.user, server.host, server.port, server.auth_method,
        server.key_path, server.proxy_jump, server.proxy_command));

    let channel_result = rt.block_on(async {
        let config = Arc::new(client::Config::default());
        let handler = SshHandler {
            api,
            host: server.host.clone(),
            port: server.port,
            session_handle,
        };

        // Determine effective proxy: proxy_command takes precedence, then
        // proxy_jump is converted to `ssh -W %h:%p <jump>`.
        let effective_proxy = server.proxy_command.clone()
            .or_else(|| {
                server.proxy_jump.as_ref().map(|jump| {
                    format!("ssh -W %h:%p {jump}")
                })
            });

        if let Some(ref proxy) = effective_proxy {
            host_log(api, 2, &format!("SSH using proxy: {proxy}"));
        }

        let mut session = if let Some(proxy_cmd) = &effective_proxy {
            connect_via_proxy(proxy_cmd, &server.host, server.port, config, handler).await?
        } else {
            let addr = format!("{}:{}", server.host, server.port);
            host_log(api, 1, &format!("SSH direct connect to {addr}"));
            client::connect(config, &addr, handler)
                .await
                .map_err(|e| format!("Connection failed: {e}"))?
        };

        host_log(api, 2, "SSH transport established, authenticating...");

        let authenticated = if server.auth_method == "password" {
            host_log(api, 1, &format!("SSH auth: using password for user '{}'", server.user));
            let pw = password.as_deref().unwrap_or("");
            session.authenticate_password(&server.user, pw)
                .await
                .map_err(|e| format!("Auth failed: {e}"))?
        } else {
            host_log(api, 1, &format!("SSH auth: trying key-based for user '{}'", server.user));
            try_key_auth(&mut session, &server.user, server.key_path.as_deref(), api).await?
        };

        if !authenticated {
            host_log(api, 3, &format!("SSH authentication failed for {}@{}", server.user, server.host));
            return Err("Authentication failed".to_string());
        }

        host_log(api, 2, "SSH authenticated, opening channel...");

        let channel = session.channel_open_session()
            .await
            .map_err(|e| format!("Channel open failed: {e}"))?;

        host_log(api, 1, "SSH requesting PTY (xterm-256color 80x24)");
        channel.request_pty(
            false, "xterm-256color", 80, 24, 0, 0,
            &[],
        ).await.map_err(|e| format!("PTY request failed: {e}"))?;

        host_log(api, 1, "SSH requesting shell");
        channel.request_shell(false)
            .await
            .map_err(|e| format!("Shell request failed: {e}"))?;

        host_log(api, 2, &format!("SSH session ready for {}@{}", server.user, server.host));
        Ok::<_, String>((channel, session))
    });

    match channel_result {
        Ok((channel, ssh_handle)) => {
            // Phase 3: Activate — wire up the channel and output callback.
            backend_state.activate(
                channel,
                ssh_handle,
                open_result.output_cb,
                open_result.output_ctx,
                rt.handle(),
                session_handle,
                api.close_session,
            );

            // Register SFTP vtable so other plugins can do direct SFTP.
            // SAFETY: backend_state is Box-heap-allocated and will live in
            // self.sessions for the entire session lifetime. The vtable
            // registration is removed when the session disconnects.
            let backend_ptr: *const SshBackendState = &*backend_state as *const SshBackendState;
            let sftp_ctx = unsafe {
                sftp_vtable::SftpContext::new(backend_ptr, rt.handle().clone())
            };
            (api.register_sftp)(
                session_handle.0,
                &sftp_vtable::SFTP_VTABLE as *const _,
                sftp_ctx as *mut std::ffi::c_void,
            );

            // Transition to Connected — host now renders the terminal.
            set_session_status(api, session_handle, conch_plugin_sdk::SessionStatus::Connected, None);

            Ok((session_handle, backend_state))
        }
        Err(e) => {
            // Transition to Error — host shows error screen with "Close Tab".
            host_log(api, 4, &format!("SSH connection failed: {e}"));
            set_session_status(api, session_handle, conch_plugin_sdk::SessionStatus::Error, Some(&e));

            // Return Ok with the handle so the plugin tracks it (host will
            // close the tab when the user clicks "Close Tab").
            Ok((session_handle, backend_state))
        }
    }
}

/// Connect to an SSH server via a ProxyCommand.
///
/// Spawns the proxy command as a shell subprocess and uses its stdin/stdout
/// as the SSH transport via `russh::client::connect_stream`.
async fn connect_via_proxy(
    proxy_cmd: &str,
    host: &str,
    port: u16,
    config: Arc<client::Config>,
    handler: SshHandler,
) -> Result<client::Handle<SshHandler>, String> {
    // Expand %h and %p placeholders.
    let expanded = proxy_cmd
        .replace("%h", host)
        .replace("%p", &port.to_string());

    log::info!("ProxyCommand: {expanded}"); // Note: may not appear — use RUST_LOG for host-side

    // Spawn via login shell so PATH is properly set (important when launched
    // from a desktop environment with minimal env).
    #[cfg(unix)]
    let child = Command::new("sh")
        .arg("-lc")
        .arg(&expanded)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::inherit())
        .spawn()
        .map_err(|e| format!("Failed to spawn ProxyCommand: {e}"))?;

    #[cfg(windows)]
    let child = Command::new("cmd")
        .arg("/C")
        .arg(&expanded)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::inherit())
        .spawn()
        .map_err(|e| format!("Failed to spawn ProxyCommand: {e}"))?;

    let stdin = child.stdin.unwrap();
    let stdout = child.stdout.unwrap();
    let stream = tokio::io::join(stdout, stdin);

    client::connect_stream(config, stream, handler)
        .await
        .map_err(|e| format!("Connection via proxy failed: {e}"))
}

/// Try key-based authentication with common SSH key files.
async fn try_key_auth(
    session: &mut client::Handle<SshHandler>,
    user: &str,
    explicit_key_path: Option<&str>,
    api: &'static HostApi,
) -> Result<bool, String> {
    let key_paths: Vec<std::path::PathBuf> = if let Some(path) = explicit_key_path {
        let expanded = expand_tilde(path);
        host_log(api, 1, &format!("SSH key auth: using explicit key path: {}", expanded.display()));
        vec![expanded]
    } else {
        let home = dirs::home_dir().unwrap_or_default();
        let ssh_dir = home.join(".ssh");
        let paths = vec![
            ssh_dir.join("id_ed25519"),
            ssh_dir.join("id_rsa"),
            ssh_dir.join("id_ecdsa"),
        ];
        host_log(api, 1, &format!("SSH key auth: trying default key paths: {:?}", paths));
        paths
    };

    for key_path in &key_paths {
        if !key_path.exists() {
            host_log(api, 1, &format!("SSH key auth: {} does not exist, skipping", key_path.display()));
            continue;
        }

        host_log(api, 1, &format!("SSH key auth: loading key from {}", key_path.display()));
        match russh_keys::load_secret_key(key_path, None) {
            Ok(key) => {
                host_log(api, 1, &format!("SSH key auth: key loaded, attempting auth as '{user}'"));
                match session.authenticate_publickey(user, Arc::new(key)).await {
                    Ok(true) => {
                        host_log(api, 2, &format!("SSH key auth: success with {}", key_path.display()));
                        return Ok(true);
                    }
                    Ok(false) => {
                        host_log(api, 1, &format!("SSH key auth: rejected by server for {}", key_path.display()));
                        continue;
                    }
                    Err(e) => {
                        host_log(api, 3, &format!("SSH key auth: error with {}: {e}", key_path.display()));
                        continue;
                    }
                }
            }
            Err(e) => {
                host_log(api, 3, &format!("SSH key auth: failed to load {}: {e}", key_path.display()));
                continue;
            }
        }
    }

    host_log(api, 3, &format!("SSH key auth: no keys succeeded for user '{user}'"));
    Ok(false)
}

// ---------------------------------------------------------------------------
// declare_plugin! macro
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

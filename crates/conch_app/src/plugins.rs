//! Plugin system: discovery, lifecycle, and command handling.

use std::collections::HashSet;
use std::path::PathBuf;

use conch_core::config;
use conch_plugin::{
    PluginCommand, PluginContext, PluginMeta, PluginResponse, PluginType, SessionInfoData,
    SessionTarget, discover_plugins, run_plugin, run_panel_plugin,
};

use crate::app::{ConchApp, RunningPlugin};
use crate::state::SessionBackend;
use crate::ui::dialogs::plugin_dialog::{ActivePluginDialog, FormFieldState};

impl ConchApp {
    /// Drain commands from all running plugins and handle them.
    pub(crate) fn poll_plugin_events(&mut self, ctx: &egui::Context) {
        // (command, response_sender, discovered_idx for panel plugins)
        let mut immediate_cmds: Vec<(PluginCommand, tokio::sync::mpsc::UnboundedSender<PluginResponse>, Option<usize>)> = Vec::new();
        self.running_plugins.retain_mut(|rp| {
            loop {
                match rp.commands_rx.try_recv() {
                    Ok((cmd, resp_tx)) => {
                        if is_dialog_command(&cmd) {
                            rp.pending_dialogs.push((cmd, resp_tx));
                        } else {
                            immediate_cmds.push((cmd, resp_tx, rp.discovered_idx));
                        }
                        ctx.request_repaint();
                    }
                    Err(tokio::sync::mpsc::error::TryRecvError::Empty) => return true,
                    Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => return false,
                }
            }
        });

        for (cmd, resp_tx, discovered_idx) in immediate_cmds {
            self.handle_plugin_command(cmd, resp_tx, discovered_idx);
        }

        if self.active_plugin_dialog.is_none() {
            for rp in &mut self.running_plugins {
                if !rp.pending_dialogs.is_empty() {
                    let (cmd, resp_tx) = rp.pending_dialogs.remove(0);
                    self.promote_dialog_command(cmd, resp_tx);
                    break;
                }
            }
        }
    }

    /// Handle a non-dialog plugin command immediately.
    pub(crate) fn handle_plugin_command(
        &mut self,
        cmd: PluginCommand,
        resp_tx: tokio::sync::mpsc::UnboundedSender<PluginResponse>,
        discovered_idx: Option<usize>,
    ) {
        match cmd {
            PluginCommand::Send { target, text } => {
                if let Some(session) = self.resolve_session(&target) {
                    session.backend.write(text.as_bytes());
                }
                let _ = resp_tx.send(PluginResponse::Ok);
            }
            PluginCommand::Exec { target, command } => {
                let ssh_handle = self.resolve_session(&target).and_then(|s| {
                    match &s.backend {
                        crate::state::SessionBackend::Ssh(ssh) => Some(ssh.ssh_handle().clone()),
                        crate::state::SessionBackend::Local(_) => None,
                    }
                });
                if let Some(handle) = ssh_handle {
                    // Run on a separate SSH channel — doesn't touch the terminal
                    self.rt.spawn(async move {
                        match conch_session::ssh_exec_command(handle, command).await {
                            Ok(output) => { let _ = resp_tx.send(PluginResponse::Output(output)); }
                            Err(e) => { let _ = resp_tx.send(PluginResponse::Error(e.to_string())); }
                        }
                    });
                    return; // response sent async
                } else {
                    // Run locally via std::process::Command (local session or no session)
                    let resp = match std::process::Command::new("sh")
                        .args(["-c", &command])
                        .output()
                    {
                        Ok(out) => PluginResponse::Output(
                            String::from_utf8_lossy(&out.stdout).into_owned(),
                        ),
                        Err(e) => PluginResponse::Error(e.to_string()),
                    };
                    let _ = resp_tx.send(resp);
                }
            }
            PluginCommand::OpenSession { name } => {
                let servers = self.collect_all_servers();
                if let Some(server) = servers.iter().find(|s| s.name == name || s.host == name) {
                    self.start_ssh_connect(
                        server.host.clone(),
                        server.port,
                        server.user.clone(),
                        server.identity_file.clone(),
                        server.proxy_command.clone(),
                        server.proxy_jump.clone(),
                        None,
                    );
                }
                let _ = resp_tx.send(PluginResponse::Ok);
            }
            PluginCommand::Clipboard(text) => {
                self.pending_clipboard = Some(text);
                let _ = resp_tx.send(PluginResponse::Ok);
            }
            PluginCommand::Notify(request) => {
                use crate::notifications::Notification;
                if request.buttons.is_empty() {
                    // Fire-and-forget notification
                    self.notifications.push(Notification::simple(
                        request.body,
                        request.title,
                        request.level,
                        request.duration_secs,
                        discovered_idx,
                    ));
                    let _ = resp_tx.send(PluginResponse::Ok);
                } else {
                    // Blocking notification with buttons — response sent when user clicks
                    self.notifications.push(Notification::with_buttons(
                        request.body,
                        request.title,
                        request.level,
                        request.buttons,
                        resp_tx,
                        discovered_idx,
                    ));
                    // Don't send response here — it'll be sent when button is clicked
                }
            }
            PluginCommand::Log(msg) => {
                log::info!("[plugin] {msg}");
                let _ = resp_tx.send(PluginResponse::Ok);
            }
            PluginCommand::UiAppend(text) => {
                self.plugin_output_lines.push(text);
                let _ = resp_tx.send(PluginResponse::Ok);
            }
            PluginCommand::UiClear => {
                self.plugin_output_lines.clear();
                let _ = resp_tx.send(PluginResponse::Ok);
            }
            PluginCommand::GetPlatform { target } => {
                let ssh_handle = self.resolve_session(&target).and_then(|s| {
                    match &s.backend {
                        crate::state::SessionBackend::Ssh(ssh) => Some(ssh.ssh_handle().clone()),
                        crate::state::SessionBackend::Local(_) => None,
                    }
                });
                if let Some(handle) = ssh_handle {
                    // Ask the remote host for its OS
                    self.rt.spawn(async move {
                        match conch_session::ssh_exec_command(handle, "uname -s".into()).await {
                            Ok(output) => {
                                let platform = normalize_platform(&output.trim().to_lowercase());
                                let _ = resp_tx.send(PluginResponse::Output(platform));
                            }
                            Err(_) => {
                                let _ = resp_tx.send(PluginResponse::Output("unknown".into()));
                            }
                        }
                    });
                    return;
                } else {
                    let _ = resp_tx.send(PluginResponse::Output(local_platform().into()));
                }
            }
            PluginCommand::GetCurrentSession => {
                let info = self.state.active_tab.and_then(|id| {
                    self.state.sessions.get(&id).map(|s| SessionInfoData {
                        id: id.to_string(),
                        title: s.custom_title.as_ref().unwrap_or(&s.title).clone(),
                        session_type: match &s.backend {
                            SessionBackend::Local(_) => "local".into(),
                            SessionBackend::Ssh(_) => "ssh".into(),
                        },
                    })
                });
                let _ = resp_tx.send(PluginResponse::SessionInfo(info));
            }
            PluginCommand::GetAllSessions => {
                let list: Vec<SessionInfoData> = self
                    .state
                    .sessions
                    .iter()
                    .map(|(id, s)| SessionInfoData {
                        id: id.to_string(),
                        title: s.custom_title.as_ref().unwrap_or(&s.title).clone(),
                        session_type: match &s.backend {
                            SessionBackend::Local(_) => "local".into(),
                            SessionBackend::Ssh(_) => "ssh".into(),
                        },
                    })
                    .collect();
                let _ = resp_tx.send(PluginResponse::SessionList(list));
            }
            PluginCommand::GetNamedSession { name } => {
                let info = self.state.sessions.iter().find_map(|(id, s)| {
                    let title = s.custom_title.as_ref().unwrap_or(&s.title);
                    if title == &name {
                        Some(SessionInfoData {
                            id: id.to_string(),
                            title: title.clone(),
                            session_type: match &s.backend {
                                SessionBackend::Local(_) => "local".into(),
                                SessionBackend::Ssh(_) => "ssh".into(),
                            },
                        })
                    } else {
                        None
                    }
                });
                let _ = resp_tx.send(PluginResponse::SessionInfo(info));
            }
            PluginCommand::GetServers => {
                let names: Vec<String> = self
                    .collect_all_servers()
                    .iter()
                    .map(|s| s.name.clone())
                    .collect();
                let _ = resp_tx.send(PluginResponse::ServerList(names));
            }
            PluginCommand::GetServerDetails => {
                let details: Vec<(String, String)> = self
                    .collect_all_servers()
                    .iter()
                    .map(|s| (s.name.clone(), s.host.clone()))
                    .collect();
                let _ = resp_tx.send(PluginResponse::ServerDetailList(details));
            }
            PluginCommand::ShowProgress { message } => {
                self.plugin_progress = Some(message);
                let _ = resp_tx.send(PluginResponse::Ok);
            }
            PluginCommand::HideProgress => {
                self.plugin_progress = None;
                let _ = resp_tx.send(PluginResponse::Ok);
            }
            PluginCommand::SetIcon { path } => {
                if let Some(idx) = discovered_idx {
                    let icon_path = std::path::PathBuf::from(&path);
                    if icon_path.is_file() {
                        if let Some(bytes) = load_icon_bytes(&icon_path) {
                            self.pending_plugin_icons.push((idx, bytes));
                            let _ = resp_tx.send(PluginResponse::Ok);
                        } else {
                            let _ = resp_tx.send(PluginResponse::Error(
                                "Invalid image file".into(),
                            ));
                        }
                    } else {
                        let _ = resp_tx.send(PluginResponse::Error(
                            format!("File not found: {path}"),
                        ));
                    }
                } else {
                    let _ = resp_tx.send(PluginResponse::Error(
                        "No plugin index for icon".into(),
                    ));
                }
            }
            PluginCommand::RegisterKeybind { action, binding, description: _ } => {
                use crate::input::KeyBinding;
                use crate::app::ResolvedPluginKeybind;

                if let Some(idx) = discovered_idx {
                    if let Some(parsed) = KeyBinding::parse(&binding) {
                        // Check for conflict with app shortcuts
                        let mods = egui::Modifiers {
                            alt: parsed.alt,
                            ctrl: false,
                            shift: parsed.shift,
                            mac_cmd: false,
                            command: parsed.command,
                        };
                        if self.shortcuts.is_app_shortcut(&parsed.key, &mods) {
                            let _ = resp_tx.send(PluginResponse::Error(
                                "Conflicts with app shortcut".into(),
                            ));
                        } else {
                            // Remove any existing binding for this plugin+action
                            self.plugin_keybinds.retain(|kb| {
                                !(kb.plugin_idx == idx && kb.action == action)
                            });
                            self.plugin_keybinds.push(ResolvedPluginKeybind {
                                binding: parsed,
                                plugin_idx: idx,
                                action,
                            });
                            let _ = resp_tx.send(PluginResponse::Ok);
                        }
                    } else {
                        let _ = resp_tx.send(PluginResponse::Error(
                            format!("Invalid binding: {binding}"),
                        ));
                    }
                } else {
                    let _ = resp_tx.send(PluginResponse::Error(
                        "No plugin index for keybind registration".into(),
                    ));
                }
            }
            PluginCommand::PanelSetWidgets(widgets) => {
                if let Some(idx) = discovered_idx {
                    self.panel_widgets.insert(idx, widgets);
                }
                let _ = resp_tx.send(PluginResponse::Ok);
            }
            PluginCommand::PanelSetRefresh(_seconds) => {
                // Refresh interval is handled in the plugin runner's loop, not here.
                let _ = resp_tx.send(PluginResponse::Ok);
            }
            PluginCommand::PanelPollEvent => {
                if let Some(idx) = discovered_idx {
                    // Check button events first.
                    if let Some(events) = self.panel_button_events.get_mut(&idx) {
                        if let Some(button_id) = events.pop() {
                            let _ = resp_tx.send(PluginResponse::PanelEvent(button_id));
                            return;
                        }
                    }
                    // Then check keybind events.
                    if let Some(events) = self.panel_keybind_events.get_mut(&idx) {
                        if let Some(action) = events.pop() {
                            let _ = resp_tx.send(PluginResponse::KeybindTriggered(action));
                            return;
                        }
                    }
                }
                // No event pending — return Ok immediately
                let _ = resp_tx.send(PluginResponse::Ok);
            }
            PluginCommand::PanelWaitEvent => {
                if let Some(idx) = discovered_idx {
                    // Check if there's already a pending button event.
                    if let Some(events) = self.panel_button_events.get_mut(&idx) {
                        if let Some(button_id) = events.pop() {
                            let _ = resp_tx.send(PluginResponse::PanelEvent(button_id));
                            return;
                        }
                    }
                    // Check keybind events.
                    if let Some(events) = self.panel_keybind_events.get_mut(&idx) {
                        if let Some(action) = events.pop() {
                            let _ = resp_tx.send(PluginResponse::KeybindTriggered(action));
                            return;
                        }
                    }
                    // No event pending — store the waiter
                    self.panel_event_waiters.insert(idx, resp_tx);
                } else {
                    let _ = resp_tx.send(PluginResponse::Ok);
                }
            }
            _ => {
                let _ = resp_tx.send(PluginResponse::Ok);
            }
        }
    }

    /// Promote a dialog command into the active dialog slot.
    pub(crate) fn promote_dialog_command(
        &mut self,
        cmd: PluginCommand,
        resp_tx: tokio::sync::mpsc::UnboundedSender<PluginResponse>,
    ) {
        let dialog = match cmd {
            PluginCommand::ShowForm { title, fields } => {
                let field_states: Vec<FormFieldState> =
                    fields.iter().map(FormFieldState::from_field).collect();
                ActivePluginDialog::Form {
                    title,
                    fields: field_states,
                    resp_tx,
                    focus_first: true,
                }
            }
            PluginCommand::ShowPrompt { message } => ActivePluginDialog::Prompt {
                message,
                input: String::new(),
                resp_tx,
                focus_first: true,
            },
            PluginCommand::ShowConfirm { message } => ActivePluginDialog::Confirm {
                message,
                resp_tx,
            },
            PluginCommand::ShowAlert { title, message } => ActivePluginDialog::Alert {
                title,
                message,
                resp_tx,
            },
            PluginCommand::ShowError { title, message } => ActivePluginDialog::Error {
                title,
                message,
                resp_tx,
            },
            PluginCommand::ShowText { title, text } => ActivePluginDialog::Text {
                title,
                text,
                copied_at: None,
                resp_tx,
            },
            PluginCommand::ShowTable {
                title,
                columns,
                rows,
            } => ActivePluginDialog::Table {
                title,
                columns,
                rows,
                resp_tx,
            },
            _ => return,
        };
        self.active_plugin_dialog = Some(dialog);
    }

    /// Resolve a session target to a `&Session`.
    pub(crate) fn resolve_session(&self, target: &SessionTarget) -> Option<&crate::state::Session> {
        match target {
            SessionTarget::Current => {
                self.state.active_tab.and_then(|id| self.state.sessions.get(&id))
            }
            SessionTarget::Named(name) => {
                self.state.sessions.values().find(|s| {
                    let title = s.custom_title.as_ref().unwrap_or(&s.title);
                    title == name
                })
            }
        }
    }

    /// Launch a discovered plugin by its index in `discovered_plugins`.
    pub(crate) fn run_plugin_by_index(&mut self, idx: usize) {
        let Some(meta) = self.discovered_plugins.get(idx).cloned() else {
            return;
        };
        // Panel plugins should be activated, not run as one-shot
        if meta.plugin_type == PluginType::Panel {
            self.activate_panel_plugin(idx);
            return;
        }
        if meta.plugin_type == PluginType::BottomPanel {
            self.activate_bottom_panel_plugin(idx);
            return;
        }
        let (ctx, commands_rx) = PluginContext::new();
        let path = meta.path.clone();
        self.rt.spawn(async move {
            if let Err(e) = run_plugin(&path, ctx).await {
                log::error!("Plugin '{}' failed: {e}", path.display());
            }
        });
        self.running_plugins.push(RunningPlugin {
            meta,
            discovered_idx: None,
            commands_rx,
            pending_dialogs: Vec::new(),
        });
    }

    /// Activate a panel plugin: start it and add a sidebar tab.
    pub(crate) fn activate_panel_plugin(&mut self, idx: usize) {
        // Don't activate twice
        if self.panel_names.contains_key(&idx) {
            // Just switch to the tab
            self.state.sidebar_tab = crate::ui::sidebar::SidebarTab::PluginPanel(idx);
            return;
        }
        let Some(meta) = self.discovered_plugins.get(idx).cloned() else {
            return;
        };
        let (ctx, commands_rx) = PluginContext::new();
        let path = meta.path.clone();
        let name = meta.name.clone();
        let icon_path = meta.icon.clone();
        self.rt.spawn(async move {
            if let Err(e) = run_panel_plugin(&path, ctx).await {
                log::error!("Panel plugin '{}' failed: {e}", path.display());
            }
        });
        self.running_plugins.push(RunningPlugin {
            meta,
            discovered_idx: Some(idx),
            commands_rx,
            pending_dialogs: Vec::new(),
        });
        self.panel_names.insert(idx, name);
        self.panel_widgets.insert(idx, Vec::new());
        // Queue icon loading if the plugin declares one
        if let Some(icon_path) = &icon_path {
            if let Some(bytes) = load_icon_bytes(icon_path) {
                self.pending_plugin_icons.push((idx, bytes));
            }
        }
        // Switch to the panel tab
        self.state.sidebar_tab = crate::ui::sidebar::SidebarTab::PluginPanel(idx);
    }

    /// Deactivate a panel plugin: stop it and remove its tab.
    pub(crate) fn deactivate_panel_plugin(&mut self, idx: usize) {
        if let Some(meta) = self.discovered_plugins.get(idx) {
            let path = meta.path.clone();
            if let Some(pos) = self.running_plugins.iter().position(|rp| rp.meta.path == path) {
                self.running_plugins.remove(pos);
            }
        }
        self.panel_names.remove(&idx);
        self.panel_widgets.remove(&idx);
        self.panel_button_events.remove(&idx);
        self.panel_keybind_events.remove(&idx);
        self.panel_event_waiters.remove(&idx);
        self.plugin_icons.remove(&idx);
        // Switch back to Plugins tab
        self.state.sidebar_tab = crate::ui::sidebar::SidebarTab::Plugins;
    }

    /// Activate a bottom-panel plugin: start it and add a bottom panel tab.
    pub(crate) fn activate_bottom_panel_plugin(&mut self, idx: usize) {
        // Don't activate twice
        if self.panel_names.contains_key(&idx) {
            // Just switch to the tab
            self.active_bottom_panel = Some(idx);
            self.show_bottom_panel = true;
            return;
        }
        let Some(meta) = self.discovered_plugins.get(idx).cloned() else {
            return;
        };
        let (ctx, commands_rx) = PluginContext::new();
        let path = meta.path.clone();
        let name = meta.name.clone();
        let icon_path = meta.icon.clone();
        self.rt.spawn(async move {
            if let Err(e) = run_panel_plugin(&path, ctx).await {
                log::error!("Bottom panel plugin '{}' failed: {e}", path.display());
            }
        });
        self.running_plugins.push(RunningPlugin {
            meta,
            discovered_idx: Some(idx),
            commands_rx,
            pending_dialogs: Vec::new(),
        });
        self.panel_names.insert(idx, name);
        self.panel_widgets.insert(idx, Vec::new());
        if let Some(icon_path) = &icon_path {
            if let Some(bytes) = load_icon_bytes(icon_path) {
                self.pending_plugin_icons.push((idx, bytes));
            }
        }
        // Add to bottom panel tabs and select it
        if !self.bottom_panel_tabs.contains(&idx) {
            self.bottom_panel_tabs.push(idx);
        }
        self.active_bottom_panel = Some(idx);
        self.show_bottom_panel = true;
    }

    /// Deactivate a bottom-panel plugin: stop it and remove its tab.
    pub(crate) fn deactivate_bottom_panel_plugin(&mut self, idx: usize) {
        if let Some(meta) = self.discovered_plugins.get(idx) {
            let path = meta.path.clone();
            if let Some(pos) = self.running_plugins.iter().position(|rp| rp.meta.path == path) {
                self.running_plugins.remove(pos);
            }
        }
        self.panel_names.remove(&idx);
        self.panel_widgets.remove(&idx);
        self.panel_button_events.remove(&idx);
        self.panel_keybind_events.remove(&idx);
        self.panel_event_waiters.remove(&idx);
        self.plugin_icons.remove(&idx);
        self.bottom_panel_tabs.retain(|&i| i != idx);
        // Select another tab or hide the panel
        if self.active_bottom_panel == Some(idx) {
            self.active_bottom_panel = self.bottom_panel_tabs.first().copied();
        }
        if self.bottom_panel_tabs.is_empty() {
            self.show_bottom_panel = false;
        }
    }

    /// Send a button click event to a panel plugin.
    pub(crate) fn send_panel_button_event(&mut self, plugin_idx: usize, button_id: String) {
        // If there's a waiter, send immediately
        if let Some(tx) = self.panel_event_waiters.remove(&plugin_idx) {
            let _ = tx.send(PluginResponse::PanelEvent(button_id));
        } else {
            // Queue it for later
            self.panel_button_events
                .entry(plugin_idx)
                .or_default()
                .push(button_id);
        }
    }


    /// Build the resolved plugin keybinding list from discovered plugins + config overrides.
    pub(crate) fn resolve_plugin_keybinds(&mut self) {
        use crate::input::KeyBinding;
        use crate::app::ResolvedPluginKeybind;

        let config_overrides = &self.state.user_config.conch.keyboard.plugins;
        let loaded = &self.state.persistent.loaded_plugins;

        let mut resolved = Vec::new();

        for (idx, meta) in self.discovered_plugins.iter().enumerate() {
            // Only resolve keybindings for loaded plugins
            let filename = meta.path.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            if !loaded.contains(&filename) {
                continue;
            }

            let stem = meta.path.file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();

            for kb in &meta.keybindings {
                // Check config override: "plugin-stem.action"
                let override_key = format!("{}.{}", stem, kb.action);
                let binding_str = config_overrides
                    .get(&override_key)
                    .unwrap_or(&kb.default_binding);

                if let Some(binding) = KeyBinding::parse(binding_str) {
                    // Skip if it conflicts with an app-level shortcut
                    if self.shortcuts.is_app_shortcut(&binding.key, &egui::Modifiers {
                        alt: binding.alt,
                        ctrl: false,
                        shift: binding.shift,
                        mac_cmd: false,
                        command: binding.command,
                    }) {
                        log::warn!(
                            "Plugin '{}' keybind '{}' ({}) conflicts with app shortcut, skipping",
                            meta.name, kb.action, binding_str
                        );
                        continue;
                    }

                    resolved.push(ResolvedPluginKeybind {
                        binding,
                        plugin_idx: idx,
                        action: kb.action.clone(),
                    });
                }
            }
        }

        self.plugin_keybinds = resolved;
    }

    /// Handle a triggered plugin keybinding.
    pub(crate) fn handle_plugin_keybind(&mut self, plugin_idx: usize, action: &str) {
        let Some(meta) = self.discovered_plugins.get(plugin_idx) else { return };

        match action {
            "open_panel" => {
                if meta.plugin_type == PluginType::Panel {
                    // Ensure sidebar is visible and switch to the panel tab
                    if !self.state.show_left_sidebar {
                        self.state.show_left_sidebar = true;
                        self.state.persistent.layout.left_panel_collapsed = false;
                        let _ = conch_core::config::save_persistent_state(&self.state.persistent);
                    }
                    self.state.sidebar_tab = crate::ui::sidebar::SidebarTab::PluginPanel(plugin_idx);
                } else if meta.plugin_type == PluginType::BottomPanel {
                    self.active_bottom_panel = Some(plugin_idx);
                    self.show_bottom_panel = true;
                }
            }
            "run" => {
                if meta.plugin_type == PluginType::Action {
                    self.run_plugin_by_index(plugin_idx);
                }
            }
            _ => {
                // Custom action — send as event to the running plugin
                self.send_plugin_keybind_event(plugin_idx, action.to_string());
            }
        }
    }

    /// Send a keybind event to a running plugin.
    fn send_plugin_keybind_event(&mut self, plugin_idx: usize, action: String) {
        // Find the running plugin for this index
        for rp in &self.running_plugins {
            if rp.discovered_idx == Some(plugin_idx) {
                // Send via the command channel's response mechanism
                // We need a way to notify the plugin. Use the panel event waiter
                // if available, or queue it.
                if let Some(tx) = self.panel_event_waiters.remove(&plugin_idx) {
                    let _ = tx.send(PluginResponse::KeybindTriggered(action));
                } else {
                    // Queue for next poll
                    self.panel_keybind_events
                        .entry(plugin_idx)
                        .or_default()
                        .push(action);
                }
                return;
            }
        }
    }

    /// Re-scan the plugins directory.
    pub(crate) fn refresh_plugins(&mut self) {
        self.discovered_plugins = scan_plugin_dirs();
        // Reset pending loads to match new list
        self.pending_plugin_loads.clear();
    }

    /// Apply plugin load/unload changes.
    /// `loaded_indices` is the list of discovered_plugins indices that should be loaded.
    pub(crate) fn apply_plugin_changes(&mut self, loaded_indices: Vec<usize>) {
        // Build the new loaded_plugins filename list
        let new_loaded: Vec<String> = loaded_indices
            .iter()
            .filter_map(|&i| {
                self.discovered_plugins.get(i).map(|meta| {
                    meta.path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned()
                })
            })
            .collect();

        let old_loaded = self.state.persistent.loaded_plugins.clone();

        // Save to persistent state
        self.state.persistent.loaded_plugins = new_loaded;
        let _ = config::save_persistent_state(&self.state.persistent);

        // Collect panel changes to apply (avoid borrow conflict).
        let mut to_activate = Vec::new();
        let mut to_deactivate = Vec::new();
        let mut to_activate_bottom = Vec::new();
        let mut to_deactivate_bottom = Vec::new();
        for (i, meta) in self.discovered_plugins.iter().enumerate() {
            let filename = meta.path.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            let was_loaded = old_loaded.contains(&filename);
            let now_loaded = loaded_indices.contains(&i);

            if meta.plugin_type == PluginType::Panel {
                if now_loaded && !was_loaded {
                    to_activate.push(i);
                } else if !now_loaded && was_loaded {
                    to_deactivate.push(i);
                }
            } else if meta.plugin_type == PluginType::BottomPanel {
                if now_loaded && !was_loaded {
                    to_activate_bottom.push(i);
                } else if !now_loaded && was_loaded {
                    to_deactivate_bottom.push(i);
                }
            }
        }
        for idx in to_deactivate {
            self.deactivate_panel_plugin(idx);
        }
        for idx in to_activate {
            self.activate_panel_plugin(idx);
        }
        for idx in to_deactivate_bottom {
            self.deactivate_bottom_panel_plugin(idx);
        }
        for idx in to_activate_bottom {
            self.activate_bottom_panel_plugin(idx);
        }

        // Re-resolve plugin keybindings with new load state
        self.resolve_plugin_keybinds();

        // Reset pending loads
        self.pending_plugin_loads.clear();
    }

    /// Activate loaded panel plugins on startup.
    pub(crate) fn activate_loaded_panel_plugins(&mut self) {
        let loaded = self.state.persistent.loaded_plugins.clone();
        for (i, meta) in self.discovered_plugins.iter().enumerate() {
            let is_panel = meta.plugin_type == PluginType::Panel;
            let is_bottom = meta.plugin_type == PluginType::BottomPanel;
            if !is_panel && !is_bottom {
                continue;
            }
            let filename = meta.path.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            if loaded.contains(&filename) {
                // Activate without duplicate check (fresh startup)
                let (ctx, commands_rx) = PluginContext::new();
                let path = meta.path.clone();
                let name = meta.name.clone();
                self.rt.spawn(async move {
                    if let Err(e) = run_panel_plugin(&path, ctx).await {
                        log::error!("Panel plugin '{}' failed: {e}", path.display());
                    }
                });
                self.running_plugins.push(RunningPlugin {
                    meta: meta.clone(),
                    discovered_idx: Some(i),
                    commands_rx,
                    pending_dialogs: Vec::new(),
                });
                self.panel_names.insert(i, name);
                self.panel_widgets.insert(i, Vec::new());
                // Queue icon loading
                if let Some(icon_path) = &meta.icon {
                    if let Some(bytes) = load_icon_bytes(icon_path) {
                        self.pending_plugin_icons.push((i, bytes));
                    }
                }
                // Bottom panels get added to the bottom panel tabs
                if is_bottom && !self.bottom_panel_tabs.contains(&i) {
                    self.bottom_panel_tabs.push(i);
                    if self.active_bottom_panel.is_none() {
                        self.active_bottom_panel = Some(i);
                    }
                }
            }
        }
    }

    /// Create textures for any pending plugin icons (must be called with egui Context available).
    pub(crate) fn flush_pending_icons(&mut self, ctx: &egui::Context) {
        for (idx, bytes) in self.pending_plugin_icons.drain(..) {
            if let Ok(img) = image::load_from_memory(&bytes) {
                let rgba = img.into_rgba8();
                let (w, h) = rgba.dimensions();
                let pixels = rgba.into_raw();
                let color_image = egui::ColorImage::from_rgba_unmultiplied(
                    [w as usize, h as usize],
                    &pixels,
                );
                let handle = ctx.load_texture(
                    format!("plugin_icon_{idx}"),
                    color_image,
                    egui::TextureOptions::LINEAR,
                );
                self.plugin_icons.insert(idx, handle);
            } else {
                log::warn!("Failed to decode plugin icon for index {idx}");
            }
        }
    }
}

pub(crate) fn is_dialog_command(cmd: &PluginCommand) -> bool {
    matches!(
        cmd,
        PluginCommand::ShowForm { .. }
            | PluginCommand::ShowPrompt { .. }
            | PluginCommand::ShowConfirm { .. }
            | PluginCommand::ShowAlert { .. }
            | PluginCommand::ShowError { .. }
            | PluginCommand::ShowText { .. }
            | PluginCommand::ShowTable { .. }
    )
}

/// Scan for plugins in the native config dir and the legacy `~/.config/conch/` dir.
pub(crate) fn scan_plugin_dirs() -> Vec<PluginMeta> {
    let mut plugins = Vec::new();
    let mut seen_names = HashSet::new();

    let native_dir = config::config_dir().join("plugins");
    if let Ok(found) = discover_plugins(&native_dir) {
        for p in found {
            let key = p.path.file_name().unwrap_or_default().to_string_lossy().into_owned();
            seen_names.insert(key);
            plugins.push(p);
        }
    }

    if let Some(home) = std::env::var_os("HOME") {
        let legacy_dir = PathBuf::from(home).join(".config/conch/plugins");
        if legacy_dir != native_dir {
            if let Ok(found) = discover_plugins(&legacy_dir) {
                for p in found {
                    let key = p.path.file_name().unwrap_or_default().to_string_lossy().into_owned();
                    if !seen_names.contains(&key) {
                        seen_names.insert(key);
                        plugins.push(p);
                    }
                }
            }
        }
    }

    plugins
}

/// Try to load and validate plugin icon bytes from a file path.
fn load_icon_bytes(path: &std::path::Path) -> Option<Vec<u8>> {
    let data = std::fs::read(path).ok()?;
    if conch_plugin::validate_icon_bytes(&data) {
        Some(data)
    } else {
        log::warn!("Plugin icon at {} failed validation", path.display());
        None
    }
}

/// Return the local machine's platform as a normalized string.
fn local_platform() -> &'static str {
    if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else if cfg!(target_os = "windows") {
        "windows"
    } else {
        "unknown"
    }
}

/// Normalize a `uname -s` output to a friendly platform name.
fn normalize_platform(uname: &str) -> String {
    if uname.contains("darwin") {
        "macos".into()
    } else if uname.contains("linux") {
        "linux".into()
    } else if uname.contains("freebsd") {
        "freebsd".into()
    } else {
        uname.to_string()
    }
}

/// Send a cancel/close response for a plugin dialog so the plugin coroutine doesn't hang.
pub(crate) fn send_plugin_dialog_cancel(dialog: &ActivePluginDialog) {
    match dialog {
        ActivePluginDialog::Form { resp_tx, .. } => {
            let _ = resp_tx.send(PluginResponse::FormResult(None));
        }
        ActivePluginDialog::Prompt { resp_tx, .. } => {
            let _ = resp_tx.send(PluginResponse::Ok);
        }
        ActivePluginDialog::Confirm { resp_tx, .. } => {
            let _ = resp_tx.send(PluginResponse::Bool(false));
        }
        ActivePluginDialog::Alert { resp_tx, .. }
        | ActivePluginDialog::Error { resp_tx, .. }
        | ActivePluginDialog::Text { resp_tx, .. }
        | ActivePluginDialog::Table { resp_tx, .. } => {
            let _ = resp_tx.send(PluginResponse::Ok);
        }
    }
}

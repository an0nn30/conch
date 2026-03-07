//! Sidebar action handlers: file browser navigation, transfers, and session panel CRUD.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use conch_core::config;
use conch_session::SftpCmd;

use crate::app::ConchApp;
use crate::sessions::load_local_entries;
use crate::ui::dialogs::new_connection::NewConnectionForm;
use crate::ui::session_panel::{ServerAddress, SessionPanelAction};
use crate::ui::sidebar::{self, SidebarAction};

impl ConchApp {
    /// Save a server entry into the given folder (by top-level index).
    pub(crate) fn save_server_entry(
        &mut self,
        entry: conch_core::models::ServerEntry,
        folder_index: usize,
    ) {
        if self.state.sessions_config.folders.is_empty() {
            self.state
                .sessions_config
                .folders
                .push(conch_core::models::ServerFolder::new("Servers"));
        }
        let idx = folder_index.min(self.state.sessions_config.folders.len() - 1);
        self.state.sessions_config.folders[idx].servers.push(entry);
        let _ = config::save_sessions(&self.state.sessions_config);
    }

    /// Save or update a server entry. If editing (editing_server_addr is set),
    /// replaces the existing entry in-place (or moves it if folder changed).
    /// Otherwise appends to the folder.
    pub(crate) fn save_or_update_server(
        &mut self,
        entry: conch_core::models::ServerEntry,
        folder_index: usize,
    ) {
        if let Some(addr) = self.state.editing_server_addr.take() {
            // Check if the folder changed (user wants to move the server).
            let same_folder = self
                .state
                .sessions_config
                .folders
                .get(folder_index)
                .map(|f| {
                    !addr.folder_path.is_empty() && f.name == addr.folder_path[0]
                })
                .unwrap_or(false);

            if same_folder {
                // Replace in-place.
                if let Some(server) = find_server_mut(&mut self.state.sessions_config.folders, &addr) {
                    *server = entry;
                }
            } else {
                // Delete from old location and add to new folder.
                delete_server(&mut self.state.sessions_config.folders, &addr);
                if self.state.sessions_config.folders.is_empty() {
                    self.state.sessions_config.folders
                        .push(conch_core::models::ServerFolder::new("Servers"));
                }
                let idx = folder_index.min(self.state.sessions_config.folders.len() - 1);
                self.state.sessions_config.folders[idx].servers.push(entry);
            }
            let _ = config::save_sessions(&self.state.sessions_config);
        } else {
            // New: append to folder.
            self.save_server_entry(entry, folder_index);
        }
    }

    pub(crate) fn handle_sidebar_action(&mut self, action: SidebarAction) {
        match action {
            SidebarAction::NavigateLocal(path) => {
                let old = self.state.file_browser.local_path.clone();
                self.state.file_browser.local_back_stack.push(old);
                self.state.file_browser.local_forward_stack.clear();
                self.state.file_browser.local_entries = load_local_entries(&path);
                self.state.file_browser.local_path_edit = path.to_string_lossy().into_owned();
                self.state.file_browser.local_path = path;
                self.state.file_browser.local_selected = None;
            }
            SidebarAction::GoBackLocal => {
                if let Some(prev) = self.state.file_browser.local_back_stack.pop() {
                    let current = self.state.file_browser.local_path.clone();
                    self.state.file_browser.local_forward_stack.push(current);
                    self.state.file_browser.local_entries = load_local_entries(&prev);
                    self.state.file_browser.local_path_edit = prev.to_string_lossy().into_owned();
                    self.state.file_browser.local_path = prev;
                    self.state.file_browser.local_selected = None;
                }
            }
            SidebarAction::GoForwardLocal => {
                if let Some(next) = self.state.file_browser.local_forward_stack.pop() {
                    let current = self.state.file_browser.local_path.clone();
                    self.state.file_browser.local_back_stack.push(current);
                    self.state.file_browser.local_entries = load_local_entries(&next);
                    self.state.file_browser.local_path_edit = next.to_string_lossy().into_owned();
                    self.state.file_browser.local_path = next;
                    self.state.file_browser.local_selected = None;
                }
            }
            SidebarAction::RefreshLocal => {
                let path = self.state.file_browser.local_path.clone();
                self.state.file_browser.local_entries = load_local_entries(&path);
                self.state.file_browser.local_selected = None;
            }
            SidebarAction::GoHomeLocal => {
                let home = dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("/"));
                let old = self.state.file_browser.local_path.clone();
                self.state.file_browser.local_back_stack.push(old);
                self.state.file_browser.local_forward_stack.clear();
                self.state.file_browser.local_entries = load_local_entries(&home);
                self.state.file_browser.local_path_edit = home.to_string_lossy().into_owned();
                self.state.file_browser.local_path = home;
                self.state.file_browser.local_selected = None;
            }
            SidebarAction::SelectFile(path) => {
                log::info!("File selected: {}", path.display());
            }
            SidebarAction::NavigateRemote(path) => {
                if let Some(old) = self.state.file_browser.remote_path.clone() {
                    self.state.file_browser.remote_back_stack.push(old);
                    self.state.file_browser.remote_forward_stack.clear();
                }
                if let Some(tx) = &self.sftp_cmd_tx {
                    let _ = tx.send(SftpCmd::List(path));
                }
            }
            SidebarAction::GoBackRemote => {
                if let Some(prev) = self.state.file_browser.remote_back_stack.pop() {
                    if let Some(current) = self.state.file_browser.remote_path.clone() {
                        self.state.file_browser.remote_forward_stack.push(current);
                    }
                    if let Some(tx) = &self.sftp_cmd_tx {
                        let _ = tx.send(SftpCmd::List(prev));
                    }
                }
            }
            SidebarAction::GoForwardRemote => {
                if let Some(next) = self.state.file_browser.remote_forward_stack.pop() {
                    if let Some(current) = self.state.file_browser.remote_path.clone() {
                        self.state.file_browser.remote_back_stack.push(current);
                    }
                    if let Some(tx) = &self.sftp_cmd_tx {
                        let _ = tx.send(SftpCmd::List(next));
                    }
                }
            }
            SidebarAction::RefreshRemote => {
                if let Some(tx) = &self.sftp_cmd_tx {
                    if let Some(rp) = &self.state.file_browser.remote_path {
                        let _ = tx.send(SftpCmd::List(rp.clone()));
                    }
                }
            }
            SidebarAction::GoHomeRemote => {
                if let Some(home) = self.remote_home.clone() {
                    if let Some(old) = self.state.file_browser.remote_path.clone() {
                        self.state.file_browser.remote_back_stack.push(old);
                        self.state.file_browser.remote_forward_stack.clear();
                    }
                    if let Some(tx) = &self.sftp_cmd_tx {
                        let _ = tx.send(SftpCmd::List(home));
                    }
                }
            }
            SidebarAction::Upload { local_path, remote_dir } => {
                let filename = local_path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let cancel = Arc::new(AtomicBool::new(false));
                self.transfers.push(sidebar::TransferStatus {
                    filename,
                    upload: true,
                    done: false,
                    error: None,
                    bytes_transferred: 0,
                    total_bytes: 0,
                    cancel: cancel.clone(),
                });
                if let Some(tx) = &self.sftp_cmd_tx {
                    let _ = tx.send(SftpCmd::Upload { local_path, remote_dir, cancel });
                }
            }
            SidebarAction::Download { remote_path, local_dir } => {
                let filename = remote_path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let cancel = Arc::new(AtomicBool::new(false));
                self.transfers.push(sidebar::TransferStatus {
                    filename,
                    upload: false,
                    done: false,
                    error: None,
                    bytes_transferred: 0,
                    total_bytes: 0,
                    cancel: cancel.clone(),
                });
                if let Some(tx) = &self.sftp_cmd_tx {
                    let _ = tx.send(SftpCmd::Download { remote_path, local_dir, cancel });
                }
            }
            SidebarAction::CancelTransfer(filename) => {
                if let Some(ts) = self
                    .transfers
                    .iter_mut()
                    .find(|t| t.filename == filename && !t.done)
                {
                    ts.cancel.store(true, Ordering::Relaxed);
                }
            }
            SidebarAction::RunPlugin(_)
            | SidebarAction::RefreshPlugins
            | SidebarAction::ApplyPluginChanges(_)
            | SidebarAction::PanelButtonClick { .. }
            | SidebarAction::DeactivatePanel(_)
            | SidebarAction::None => {}
        }
    }

    pub(crate) fn handle_session_panel_action(&mut self, action: SessionPanelAction) {
        match action {
            SessionPanelAction::Connect(req) => {
                self.start_ssh_connect(
                    req.host,
                    req.port,
                    req.user,
                    req.identity_file,
                    req.proxy_command,
                    req.proxy_jump,
                    req.password,
                );
                if self.quick_connect_opened_sidebar {
                    self.state.show_right_sidebar = false;
                    self.quick_connect_opened_sidebar = false;
                }
            }
            SessionPanelAction::CreateFolder { parent_path, name } => {
                let folder = conch_core::models::ServerFolder::new(name);
                if parent_path.is_empty() {
                    self.state.sessions_config.folders.push(folder);
                } else if let Some(parent) = find_folder_mut(&mut self.state.sessions_config.folders, &parent_path) {
                    parent.subfolders.push(folder);
                }
                let _ = config::save_sessions(&self.state.sessions_config);
            }
            SessionPanelAction::RenameFolder { path, new_name } => {
                if let Some(f) = find_folder_mut(&mut self.state.sessions_config.folders, &path) {
                    f.name = new_name;
                }
                let _ = config::save_sessions(&self.state.sessions_config);
            }
            SessionPanelAction::DeleteFolder { path } => {
                delete_folder(&mut self.state.sessions_config.folders, &path);
                let _ = config::save_sessions(&self.state.sessions_config);
            }
            SessionPanelAction::CreateServer { folder_path } => {
                let entry = conch_core::models::ServerEntry {
                    name: "New Server".into(),
                    host: String::new(),
                    port: 22,
                    user: String::new(),
                    identity_file: None,
                    proxy_command: None,
                    proxy_jump: None,
                    startup_command: None,
                    session_key: None,
                    from_ssh_config: false,
                };
                if folder_path.is_empty() {
                    if self.state.sessions_config.folders.is_empty() {
                        self.state.sessions_config.folders.push(
                            conch_core::models::ServerFolder::new("Servers"),
                        );
                    }
                    self.state.sessions_config.folders[0].servers.push(entry);
                } else if let Some(f) = find_folder_mut(&mut self.state.sessions_config.folders, &folder_path) {
                    f.servers.push(entry);
                }
                let _ = config::save_sessions(&self.state.sessions_config);
            }
            SessionPanelAction::RenameServer { addr, new_name } => {
                if let Some(server) = find_server_mut(&mut self.state.sessions_config.folders, &addr) {
                    server.name = new_name;
                }
                let _ = config::save_sessions(&self.state.sessions_config);
            }
            SessionPanelAction::DeleteServer { addr } => {
                delete_server(&mut self.state.sessions_config.folders, &addr);
                let _ = config::save_sessions(&self.state.sessions_config);
            }
            SessionPanelAction::EditServer { addr } => {
                // Find the folder index for the form's folder dropdown.
                let folder_index = self
                    .state
                    .sessions_config
                    .folders
                    .iter()
                    .position(|f| !addr.folder_path.is_empty() && f.name == addr.folder_path[0])
                    .unwrap_or(0);

                if let Some(server) = find_server_mut(&mut self.state.sessions_config.folders, &addr) {
                    let form = NewConnectionForm::from_server_entry(server, folder_index);
                    self.state.new_connection_form = Some(form);
                    self.state.editing_server_addr = Some(addr);
                }
            }
            SessionPanelAction::OpenNewConnectionDialog => {
                self.state.new_connection_form =
                    Some(NewConnectionForm::with_defaults());
            }
            SessionPanelAction::None => {}
        }
    }
}

/// Walk a path of folder names and return a mutable reference to the target folder.
fn find_folder_mut<'a>(
    folders: &'a mut Vec<conch_core::models::ServerFolder>,
    path: &[String],
) -> Option<&'a mut conch_core::models::ServerFolder> {
    if path.is_empty() {
        return None;
    }
    let first = &path[0];
    let rest = &path[1..];
    let folder = folders.iter_mut().find(|f| &f.name == first)?;
    if rest.is_empty() {
        Some(folder)
    } else {
        find_folder_mut(&mut folder.subfolders, rest)
    }
}

fn delete_folder(folders: &mut Vec<conch_core::models::ServerFolder>, path: &[String]) {
    if path.is_empty() {
        return;
    }
    if path.len() == 1 {
        folders.retain(|f| f.name != path[0]);
    } else if let Some(parent) = find_folder_mut(folders, &path[..path.len() - 1]) {
        let target = &path[path.len() - 1];
        parent.subfolders.retain(|f| &f.name != target);
    }
}

fn find_server_mut<'a>(
    folders: &'a mut Vec<conch_core::models::ServerFolder>,
    addr: &ServerAddress,
) -> Option<&'a mut conch_core::models::ServerEntry> {
    let folder = find_folder_mut(folders, &addr.folder_path)?;
    folder.servers.get_mut(addr.index)
}

fn delete_server(
    folders: &mut Vec<conch_core::models::ServerFolder>,
    addr: &ServerAddress,
) {
    if let Some(folder) = find_folder_mut(folders, &addr.folder_path) {
        if addr.index < folder.servers.len() {
            folder.servers.remove(addr.index);
        }
    }
}

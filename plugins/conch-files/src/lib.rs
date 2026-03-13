//! Conch Files Plugin — local & remote file explorer.

mod format;
pub(crate) mod local;
mod remote;

use std::collections::HashMap;
use std::ffi::{CStr, CString};

use conch_plugin_sdk::{
    declare_plugin,
    widgets::{
        ContextMenuItem, PluginEvent, TableCell, TableColumn, TableRow, TextStyle, ToolbarItem,
        Widget, WidgetEvent,
    },
    HostApi, PanelHandle, PanelLocation, PluginInfo, PluginType,
};

/// Log a message through the HostApi.
fn host_log(api: &HostApi, level: u8, msg: &str) {
    if let Ok(c) = CString::new(msg) {
        (api.log)(level, c.as_ptr());
    }
}

// Column IDs.
const COL_NAME: &str = "name";
const COL_EXT: &str = "ext";
const COL_SIZE: &str = "size";
const COL_MODIFIED: &str = "modified";

/// A single file/directory entry.
pub struct FileEntry {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    pub modified: Option<u64>,
}

/// Browsing context — local or remote.
enum BrowseMode {
    Local,
    Remote {
        session_id: u64,
        host: String,
        user: String,
    },
}

/// Cached info about an SSH session.
struct SshSessionInfo {
    host: String,
    user: String,
}

/// The file explorer plugin state.
struct FilesPlugin {
    api: &'static HostApi,
    _panel: PanelHandle,

    // Navigation
    current_path: String,
    path_input: String,
    back_stack: Vec<String>,
    forward_stack: Vec<String>,

    // Content
    entries: Vec<FileEntry>,
    selected_row: Option<String>,
    sort_column: String,
    sort_ascending: bool,

    // Column visibility
    col_ext_visible: bool,
    col_size_visible: bool,
    col_modified_visible: bool,

    // Context
    mode: BrowseMode,
    ssh_sessions: HashMap<u64, SshSessionInfo>,
    active_session_id: Option<u64>,

    // UI
    dirty: bool,
    error: Option<String>,
    home_path: String,
}

impl FilesPlugin {
    fn new(api: &'static HostApi) -> Self {
        host_log(api, 2, "Files plugin initializing");

        let name = CString::new("Files").unwrap();
        let icon = CString::new("tab-files").unwrap();
        let panel = (api.register_panel)(PanelLocation::Left, name.as_ptr(), icon.as_ptr());

        // Subscribe to bus events.
        for event in &["ssh.session_ready", "ssh.session_closed", "app.tab_changed"] {
            let ev = CString::new(*event).unwrap();
            (api.subscribe)(ev.as_ptr());
        }

        let home = dirs::home_dir()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| "/".to_string());

        let mut plugin = FilesPlugin {
            api,
            _panel: panel,
            current_path: home.clone(),
            path_input: home.clone(),
            back_stack: Vec::new(),
            forward_stack: Vec::new(),
            entries: Vec::new(),
            selected_row: None,
            sort_column: COL_NAME.to_string(),
            sort_ascending: true,
            col_ext_visible: true,
            col_size_visible: true,
            col_modified_visible: true,
            mode: BrowseMode::Local,
            ssh_sessions: HashMap::new(),
            active_session_id: None,
            dirty: true,
            error: None,
            home_path: home,
        };

        plugin.load_entries();
        plugin
    }

    // -----------------------------------------------------------------------
    // Navigation
    // -----------------------------------------------------------------------

    fn navigate_to(&mut self, path: &str) {
        // Push current path to back stack.
        self.back_stack.push(self.current_path.clone());
        self.forward_stack.clear();
        self.current_path = path.to_string();
        self.path_input = path.to_string();
        self.selected_row = None;
        self.load_entries();
    }

    fn go_back(&mut self) {
        if let Some(prev) = self.back_stack.pop() {
            self.forward_stack.push(self.current_path.clone());
            self.current_path = prev.clone();
            self.path_input = prev;
            self.selected_row = None;
            self.load_entries();
        }
    }

    fn go_forward(&mut self) {
        if let Some(next) = self.forward_stack.pop() {
            self.back_stack.push(self.current_path.clone());
            self.current_path = next.clone();
            self.path_input = next;
            self.selected_row = None;
            self.load_entries();
        }
    }

    fn go_home(&mut self) {
        let home = self.home_path.clone();
        self.navigate_to(&home);
    }

    fn refresh(&mut self) {
        self.selected_row = None;
        self.load_entries();
    }

    fn load_entries(&mut self) {
        self.error = None;
        let result = match &self.mode {
            BrowseMode::Local => local::list_dir(&self.current_path),
            BrowseMode::Remote { session_id, .. } => {
                remote::list_dir(self.api, *session_id, &self.current_path)
            }
        };

        match result {
            Ok(entries) => {
                self.entries = entries;
                self.apply_sort();
            }
            Err(e) => {
                self.error = Some(e);
                self.entries.clear();
            }
        }
        self.dirty = true;
    }

    // -----------------------------------------------------------------------
    // Sorting
    // -----------------------------------------------------------------------

    fn apply_sort(&mut self) {
        let col = self.sort_column.as_str();
        let asc = self.sort_ascending;

        self.entries.sort_by(|a, b| {
            // Directories always first.
            let dir_ord = b.is_dir.cmp(&a.is_dir);
            if dir_ord != std::cmp::Ordering::Equal {
                return dir_ord;
            }

            let ord = match col {
                COL_NAME => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
                COL_EXT => {
                    let ext_a = a.name.rsplit_once('.').map(|(_, e)| e.to_lowercase()).unwrap_or_default();
                    let ext_b = b.name.rsplit_once('.').map(|(_, e)| e.to_lowercase()).unwrap_or_default();
                    ext_a.cmp(&ext_b)
                }
                COL_SIZE => a.size.cmp(&b.size),
                COL_MODIFIED => a.modified.cmp(&b.modified),
                _ => std::cmp::Ordering::Equal,
            };

            if asc { ord } else { ord.reverse() }
        });
    }

    // -----------------------------------------------------------------------
    // Mode switching
    // -----------------------------------------------------------------------

    fn switch_to_local(&mut self) {
        if matches!(self.mode, BrowseMode::Local) {
            return;
        }
        self.mode = BrowseMode::Local;
        self.back_stack.clear();
        self.forward_stack.clear();
        self.current_path = self.home_path.clone();
        self.path_input = self.home_path.clone();
        self.selected_row = None;
        self.load_entries();
    }

    fn switch_to_remote(&mut self, session_id: u64, host: &str, user: &str) {
        self.mode = BrowseMode::Remote {
            session_id,
            host: host.to_string(),
            user: user.to_string(),
        };
        self.back_stack.clear();
        self.forward_stack.clear();
        // Start at remote home directory.
        self.current_path = ".".to_string();
        self.path_input = ".".to_string();
        self.selected_row = None;
        self.load_entries();

        // If list_dir returned successfully, the path might have been resolved.
        // Keep current_path as-is; the plugin shows what was listed.
    }

    // -----------------------------------------------------------------------
    // Event handling
    // -----------------------------------------------------------------------

    fn handle_event(&mut self, event: PluginEvent) {
        match event {
            PluginEvent::Widget(widget_event) => self.handle_widget_event(widget_event),
            PluginEvent::BusEvent { event_type, data } => {
                self.handle_bus_event(&event_type, data);
            }
            PluginEvent::Shutdown => {
                host_log(self.api, 2, "Files plugin shutting down");
            }
            _ => {}
        }
    }

    fn handle_widget_event(&mut self, event: WidgetEvent) {
        match event {
            WidgetEvent::ButtonClick { id } => match id.as_str() {
                "back" => self.go_back(),
                "forward" => self.go_forward(),
                "home" => self.go_home(),
                "refresh" => self.refresh(),
                _ => {}
            },

            WidgetEvent::ToolbarInputSubmit { id, value } => {
                if id == "path" {
                    self.navigate_to(&value);
                }
            }

            WidgetEvent::TextInputSubmit { id, value } => {
                if id == "path" {
                    self.navigate_to(&value);
                }
            }

            WidgetEvent::TableActivate { row_id, .. } => {
                // Double-click: navigate into directory.
                if let Some(entry) = self.entries.iter().find(|e| e.name == row_id) {
                    if entry.is_dir {
                        let new_path = if self.current_path.ends_with('/') || self.current_path == "." {
                            if self.current_path == "." {
                                entry.name.clone()
                            } else {
                                format!("{}{}", self.current_path, entry.name)
                            }
                        } else {
                            format!("{}/{}", self.current_path, entry.name)
                        };
                        self.navigate_to(&new_path);
                    }
                }
            }

            WidgetEvent::TableSelect { row_id, .. } => {
                self.selected_row = Some(row_id);
                self.dirty = true;
            }

            WidgetEvent::TableSort {
                column, ascending, ..
            } => {
                self.sort_column = column;
                self.sort_ascending = ascending;
                self.apply_sort();
                self.dirty = true;
            }

            WidgetEvent::TableHeaderContextMenu { column, .. } => {
                // Toggle column visibility.
                match column.as_str() {
                    COL_EXT => self.col_ext_visible = !self.col_ext_visible,
                    COL_SIZE => self.col_size_visible = !self.col_size_visible,
                    COL_MODIFIED => self.col_modified_visible = !self.col_modified_visible,
                    _ => {}
                }
                self.dirty = true;
            }

            WidgetEvent::TableContextMenu { row_id, action, .. } => {
                self.handle_context_action(&row_id, &action);
            }

            _ => {}
        }
    }

    fn handle_bus_event(&mut self, event_type: &str, data: serde_json::Value) {
        match event_type {
            "ssh.session_ready" => {
                let session_id = data["session_id"].as_u64().unwrap_or(0);
                let host = data["host"].as_str().unwrap_or("").to_string();
                let user = data["user"].as_str().unwrap_or("").to_string();

                host_log(
                    self.api,
                    1,
                    &format!("SSH session ready: {session_id} ({user}@{host})"),
                );

                self.ssh_sessions.insert(
                    session_id,
                    SshSessionInfo {
                        host,
                        user,
                    },
                );

                // Auto-switch to the newly connected session.
                // The new SSH tab is now active, so show its files.
                if let Some(info) = self.ssh_sessions.get(&session_id) {
                    let host = info.host.clone();
                    let user = info.user.clone();
                    self.active_session_id = Some(session_id);
                    self.switch_to_remote(session_id, &host, &user);
                }
            }

            "ssh.session_closed" => {
                let session_id = data["session_id"].as_u64().unwrap_or(0);
                self.ssh_sessions.remove(&session_id);

                // If the closed session was our active remote, fall back to local.
                if let BrowseMode::Remote {
                    session_id: active_id,
                    ..
                } = &self.mode
                {
                    if *active_id == session_id {
                        self.active_session_id = None;
                        self.switch_to_local();
                    }
                }
            }

            "app.tab_changed" => {
                // Check if the new active tab is an SSH session.
                let session_id = data["session_id"].as_u64();
                let is_ssh = data["is_ssh"].as_bool().unwrap_or(false);

                if is_ssh {
                    if let Some(sid) = session_id {
                        if let Some(info) = self.ssh_sessions.get(&sid) {
                            let host = info.host.clone();
                            let user = info.user.clone();
                            self.active_session_id = Some(sid);
                            // Only switch if we're not already on this session.
                            let already_active = matches!(
                                &self.mode,
                                BrowseMode::Remote { session_id: active, .. } if *active == sid
                            );
                            if !already_active {
                                self.switch_to_remote(sid, &host, &user);
                            }
                        }
                    }
                } else {
                    self.active_session_id = None;
                    self.switch_to_local();
                }
                self.dirty = true;
            }

            _ => {}
        }
    }

    fn handle_context_action(&mut self, row_id: &str, action: &str) {
        match action {
            "new_folder" => {
                let msg = CString::new("Enter folder name:").unwrap();
                let default = CString::new("New Folder").unwrap();
                let result_ptr = (self.api.show_prompt)(msg.as_ptr(), default.as_ptr());
                if !result_ptr.is_null() {
                    let name = unsafe { CStr::from_ptr(result_ptr) }
                        .to_string_lossy()
                        .to_string();
                    (self.api.free_string)(result_ptr);
                    if !name.is_empty() {
                        let new_path = format!("{}/{}", self.current_path, name);
                        let result = match &self.mode {
                            BrowseMode::Local => {
                                std::fs::create_dir(&new_path)
                                    .map_err(|e| e.to_string())
                            }
                            BrowseMode::Remote { session_id, .. } => {
                                remote::mkdir(self.api, *session_id, &new_path)
                            }
                        };
                        if let Err(e) = result {
                            self.error = Some(format!("Failed to create folder: {e}"));
                        }
                        self.refresh();
                    }
                }
            }

            "delete" => {
                if let Some(entry) = self.entries.iter().find(|e| e.name == row_id) {
                    let msg = CString::new(format!("Delete \"{}\"?", entry.name)).unwrap();
                    if (self.api.show_confirm)(msg.as_ptr()) {
                        let path = format!("{}/{}", self.current_path, entry.name);
                        let is_dir = entry.is_dir;
                        let result = match &self.mode {
                            BrowseMode::Local => {
                                if is_dir {
                                    std::fs::remove_dir_all(&path).map_err(|e| e.to_string())
                                } else {
                                    std::fs::remove_file(&path).map_err(|e| e.to_string())
                                }
                            }
                            BrowseMode::Remote { session_id, .. } => {
                                remote::delete(self.api, *session_id, &path, is_dir)
                            }
                        };
                        if let Err(e) = result {
                            self.error = Some(format!("Delete failed: {e}"));
                        }
                        self.refresh();
                    }
                }
            }

            "rename" => {
                if let Some(entry) = self.entries.iter().find(|e| e.name == row_id) {
                    let msg = CString::new("Enter new name:").unwrap();
                    let default = CString::new(entry.name.as_str()).unwrap();
                    let result_ptr = (self.api.show_prompt)(msg.as_ptr(), default.as_ptr());
                    if !result_ptr.is_null() {
                        let new_name = unsafe { CStr::from_ptr(result_ptr) }
                            .to_string_lossy()
                            .to_string();
                        (self.api.free_string)(result_ptr);
                        if !new_name.is_empty() && new_name != entry.name {
                            let from = format!("{}/{}", self.current_path, entry.name);
                            let to = format!("{}/{}", self.current_path, new_name);
                            let result = match &self.mode {
                                BrowseMode::Local => {
                                    std::fs::rename(&from, &to).map_err(|e| e.to_string())
                                }
                                BrowseMode::Remote { session_id, .. } => {
                                    remote::rename(self.api, *session_id, &from, &to)
                                }
                            };
                            if let Err(e) = result {
                                self.error = Some(format!("Rename failed: {e}"));
                            }
                            self.refresh();
                        }
                    }
                }
            }

            "copy_path" => {
                let path = format!("{}/{}", self.current_path, row_id);
                let c = CString::new(path).unwrap();
                (self.api.clipboard_set)(c.as_ptr());
            }

            _ => {}
        }
    }

    // -----------------------------------------------------------------------
    // Rendering
    // -----------------------------------------------------------------------

    /// Display label for the current browsing context.
    fn context_label(&self) -> String {
        match &self.mode {
            BrowseMode::Local => {
                // Use the machine's hostname.
                hostname::get()
                    .ok()
                    .and_then(|h| h.into_string().ok())
                    .unwrap_or_else(|| "localhost".to_string())
            }
            BrowseMode::Remote { host, user, .. } => format!("{user}@{host}"),
        }
    }

    fn render(&self) -> Vec<Widget> {
        let mut widgets = Vec::new();

        // Title row — hostname or user@host (rendered above the toolbar by the host).
        widgets.push(Widget::heading(self.context_label()));

        // Toolbar: [back] [forward] | path input | [home] [refresh]
        widgets.push(Widget::Toolbar {
            id: Some("nav".into()),
            items: vec![
                ToolbarItem::Button {
                    id: "back".into(),
                    icon: Some("go-previous".into()),
                    label: None,
                    tooltip: Some("Back".into()),
                    enabled: Some(!self.back_stack.is_empty()),
                },
                ToolbarItem::Button {
                    id: "forward".into(),
                    icon: Some("go-next".into()),
                    label: None,
                    tooltip: Some("Forward".into()),
                    enabled: Some(!self.forward_stack.is_empty()),
                },
                ToolbarItem::Separator,
                ToolbarItem::TextInput {
                    id: "path".into(),
                    value: self.path_input.clone(),
                    hint: Some("Path...".into()),
                },
                ToolbarItem::Separator,
                ToolbarItem::Button {
                    id: "home".into(),
                    icon: Some("go-home".into()),
                    label: None,
                    tooltip: Some("Home".into()),
                    enabled: None,
                },
                ToolbarItem::Button {
                    id: "refresh".into(),
                    icon: Some("refresh".into()),
                    label: None,
                    tooltip: Some("Refresh".into()),
                    enabled: None,
                },
            ],
        });

        // Error message if any.
        if let Some(err) = &self.error {
            widgets.push(Widget::Label {
                text: err.clone(),
                style: Some(TextStyle::Error),
            });
        }

        // File table.
        let mut columns = vec![TableColumn {
            id: COL_NAME.into(),
            label: "Name".into(),
            sortable: Some(true),
            width: None, // Fill remaining space
            visible: None,
        }];
        columns.push(TableColumn {
            id: COL_EXT.into(),
            label: "Ext".into(),
            sortable: Some(true),
            width: Some(50.0),
            visible: Some(self.col_ext_visible),
        });
        columns.push(TableColumn {
            id: COL_SIZE.into(),
            label: "Size".into(),
            sortable: Some(true),
            width: Some(55.0),
            visible: Some(self.col_size_visible),
        });
        columns.push(TableColumn {
            id: COL_MODIFIED.into(),
            label: "Modified".into(),
            sortable: Some(true),
            width: Some(100.0),
            visible: Some(self.col_modified_visible),
        });

        let rows: Vec<TableRow> = self
            .entries
            .iter()
            .map(|entry| {
                let icon = if entry.is_dir { "folder" } else { "file" };
                let ext_label = format::extension_label(&entry.name, entry.is_dir);
                let size_label = if entry.is_dir {
                    "<DIR>".to_string()
                } else {
                    format::format_size(entry.size)
                };
                let date_label = entry
                    .modified
                    .map(format::format_date)
                    .unwrap_or_else(|| "\u{2014}".to_string()); // em dash

                TableRow {
                    id: entry.name.clone(),
                    cells: vec![
                        TableCell::Rich {
                            text: entry.name.clone(),
                            icon: Some(icon.into()),
                            badge: None,
                        },
                        TableCell::Text(ext_label),
                        TableCell::Text(size_label),
                        TableCell::Text(date_label),
                    ],
                    context_menu: Some(vec![
                        ContextMenuItem {
                            id: "new_folder".into(),
                            label: "New Folder".into(),
                            icon: Some("folder-new".into()),
                            enabled: None,
                            shortcut: None,
                        },
                        ContextMenuItem {
                            id: "rename".into(),
                            label: "Rename".into(),
                            icon: None,
                            enabled: None,
                            shortcut: None,
                        },
                        ContextMenuItem {
                            id: "delete".into(),
                            label: "Delete".into(),
                            icon: None,
                            enabled: None,
                            shortcut: None,
                        },
                        ContextMenuItem {
                            id: "copy_path".into(),
                            label: "Copy Path".into(),
                            icon: None,
                            enabled: None,
                            shortcut: None,
                        },
                    ]),
                }
            })
            .collect();

        widgets.push(Widget::Table {
            id: "files".into(),
            columns,
            rows,
            sort_column: Some(self.sort_column.clone()),
            sort_ascending: Some(self.sort_ascending),
            selected_row: self.selected_row.clone(),
        });

        // Footer (separator + label = pinned to bottom).
        widgets.push(Widget::Separator);

        let count = self.entries.len();
        widgets.push(Widget::Label {
            text: format!("{count} items"),
            style: Some(TextStyle::Secondary),
        });

        widgets
    }

    fn handle_query(&mut self, _method: &str, _args: serde_json::Value) -> serde_json::Value {
        serde_json::json!({ "status": "error", "message": "not implemented" })
    }
}

declare_plugin!(
    info: PluginInfo {
        name: c"File Explorer".as_ptr(),
        description: c"Browse local and remote files".as_ptr(),
        version: c"0.1.0".as_ptr(),
        plugin_type: PluginType::Panel,
        panel_location: PanelLocation::Left,
        dependencies: std::ptr::null(),
        num_dependencies: 0,
    },
    state: FilesPlugin,
    setup: |api| FilesPlugin::new(api),
    event: |state, event| state.handle_event(event),
    render: |state| state.render(),
    query: |state, method, args| state.handle_query(method, args),
);

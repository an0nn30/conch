use egui::Ui;

use conch_core::models::{ServerEntry, ServerFolder};

use crate::icons::{Icon, IconCache};

/// Action from session panel interaction.
pub struct SshConnectRequest {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub identity_file: Option<String>,
    pub proxy_command: Option<String>,
    pub proxy_jump: Option<String>,
}

/// Identifies a server entry by the folder path it lives in + its index.
#[derive(Clone, Debug, PartialEq)]
pub struct ServerAddress {
    pub folder_path: Vec<String>,
    pub index: usize,
}

/// All possible actions returned from the session panel.
pub enum SessionPanelAction {
    None,
    Connect(SshConnectRequest),
    /// Create a new folder.
    CreateFolder {
        parent_path: Vec<String>,
        name: String,
    },
    /// Rename a folder.
    RenameFolder {
        path: Vec<String>,
        new_name: String,
    },
    /// Delete a folder.
    DeleteFolder { path: Vec<String> },
    /// Create a new (empty) server entry in a folder.
    CreateServer { folder_path: Vec<String> },
    /// Rename a server entry.
    RenameServer {
        addr: ServerAddress,
        new_name: String,
    },
    /// Delete a server entry.
    DeleteServer { addr: ServerAddress },
    /// Open the edit dialog for a server entry (stub — not yet wired).
    #[allow(dead_code)]
    EditServer { addr: ServerAddress },
}

/// What kind of item a pending delete-confirmation targets.
#[derive(Clone, Debug, PartialEq)]
enum DeleteTarget {
    Folder(Vec<String>),
    Server(ServerAddress),
}

/// What kind of item is being renamed.
#[derive(Clone, Debug, PartialEq)]
enum RenameTarget {
    Folder(Vec<String>),
    Server(ServerAddress),
}

/// Transient UI state for the session panel.
#[derive(Default)]
pub struct SessionPanelState {
    // -- new folder inline input --
    new_folder_parent: Option<Vec<String>>,
    new_folder_name: String,
    new_folder_focus: bool,

    // -- inline rename --
    renaming: Option<RenameTarget>,
    rename_buf: String,
    rename_focus: bool,

    // -- delete confirmation --
    confirm_delete: Option<DeleteTarget>,

    // -- quick connect search --
    pub quick_connect_query: String,
    pub quick_connect_focus: bool,
}

/// Render the right sidebar showing server folders and SSH config entries.
pub fn show_session_panel(
    ui: &mut Ui,
    folders: &[ServerFolder],
    ssh_hosts: &[ServerEntry],
    icons: Option<&IconCache>,
    panel_state: &mut SessionPanelState,
) -> SessionPanelAction {
    let mut action = SessionPanelAction::None;

    // Header: icon + "Sessions" + "+" new-folder button
    let dark_mode = ui.visuals().dark_mode;
    ui.horizontal(|ui| {
        if let Some(img) = icons.and_then(|ic| ic.themed_image(Icon::TabSessions, dark_mode)) {
            ui.add(img);
        }
        ui.label("Sessions");

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let btn = if let Some(img) = icons.and_then(|ic| ic.themed_image(Icon::FolderNew, dark_mode)) {
                ui.add(egui::ImageButton::new(img).frame(false))
                    .on_hover_text("New folder")
            } else {
                ui.small_button("+").on_hover_text("New folder")
            };
            if btn.clicked() {
                panel_state.new_folder_parent = Some(Vec::new());
                panel_state.new_folder_name.clear();
                panel_state.new_folder_focus = true;
            }
        });
    });
    ui.separator();

    // Quick connect search bar
    let search_resp = ui.add(
        crate::ui::widgets::text_edit(&mut panel_state.quick_connect_query)
            .hint_text("Quick connect\u{2026}")
            .desired_width(ui.available_width() - 8.0),
    );
    if panel_state.quick_connect_focus {
        search_resp.request_focus();
        panel_state.quick_connect_focus = false;
    }

    let query = panel_state.quick_connect_query.trim().to_lowercase();
    let is_filtering = !query.is_empty();

    // Enter on search bar → connect to top match
    if search_resp.lost_focus()
        && ui.input(|i| i.key_pressed(egui::Key::Enter))
        && is_filtering
    {
        let all = collect_all_servers(folders, ssh_hosts);
        let top_match = all.iter().find(|e| {
            e.name.to_lowercase().contains(&query)
                || e.host.to_lowercase().contains(&query)
                || e.user.to_lowercase().contains(&query)
        });
        if let Some(entry) = top_match {
            action = SessionPanelAction::Connect(SshConnectRequest {
                host: entry.host.clone(),
                port: entry.port,
                user: entry.user.clone(),
                identity_file: entry.identity_file.clone(),
                proxy_command: entry.proxy_command.clone(),
                proxy_jump: entry.proxy_jump.clone(),
            });
            panel_state.quick_connect_query.clear();
        }
    }

    let scroll_resp = egui::ScrollArea::vertical()
        .show(ui, |ui| {
            if is_filtering {
                // Filtered view: flat list of matching servers
                let all = collect_all_servers(folders, ssh_hosts);
                for entry in &all {
                    let matches = entry.name.to_lowercase().contains(&query)
                        || entry.host.to_lowercase().contains(&query)
                        || entry.user.to_lowercase().contains(&query);
                    if !matches {
                        continue;
                    }
                    let clicked = ui
                        .horizontal(|ui| {
                            if let Some(img) = icons.and_then(|ic| ic.themed_image(Icon::Computer, dark_mode)) {
                                ui.add(img);
                            }
                            let mut job = egui::text::LayoutJob::default();
                            job.append(
                                entry.display_name(),
                                0.0,
                                egui::TextFormat::simple(
                                    egui::FontId::proportional(ui.style().text_styles[&egui::TextStyle::Body].size),
                                    ui.visuals().text_color(),
                                ),
                            );
                            job.append(
                                &format!(" {}@{}", entry.user, entry.host),
                                0.0,
                                egui::TextFormat::simple(
                                    egui::FontId::proportional(ui.style().text_styles[&egui::TextStyle::Body].size),
                                    ui.visuals().weak_text_color(),
                                ),
                            );
                            ui.add(egui::Label::new(job).sense(egui::Sense::click()))
                                .clicked()
                        })
                        .inner;
                    if clicked {
                        action = SessionPanelAction::Connect(SshConnectRequest {
                            host: entry.host.clone(),
                            port: entry.port,
                            user: entry.user.clone(),
                            identity_file: entry.identity_file.clone(),
                            proxy_command: entry.proxy_command.clone(),
                            proxy_jump: entry.proxy_jump.clone(),
                        });
                        panel_state.quick_connect_query.clear();
                    }
                }
            } else {
                // Normal tree view
                // User folders
                for folder in folders {
                    let a = show_folder(ui, folder, &[], icons, panel_state, dark_mode);
                    if !matches!(a, SessionPanelAction::None) {
                        action = a;
                    }
                }

                // Inline new-folder field at root level
                if panel_state.new_folder_parent.as_deref() == Some(&[]) {
                    if let Some(a) = show_new_folder_input(ui, panel_state, icons) {
                        action = a;
                    }
                }

                // SSH config section — collapsible
                if !ssh_hosts.is_empty() {
                    ui.separator();

                    let id = ui.make_persistent_id("ssh_config_section");
                    let mut coll =
                        egui::collapsing_header::CollapsingState::load_with_default_open(
                            ui.ctx(),
                            id,
                            true,
                        );

                    let header_resp = ui.horizontal(|ui| {
                        if let Some(img) = icons.and_then(|ic| ic.themed_image(Icon::SidebarFolder, dark_mode)) {
                            ui.add(img);
                        }
                        if ui
                            .add(
                                egui::Label::new("~/.ssh/config").sense(egui::Sense::click()),
                            )
                            .clicked()
                        {
                            coll.toggle(ui);
                        }
                    });

                    if header_resp.response.clicked() {
                        coll.toggle(ui);
                    }

                    coll.show_body_unindented(ui, |ui| {
                        ui.indent(id, |ui| {
                            for host in ssh_hosts {
                                let a = show_server_entry_readonly(ui, host, icons, dark_mode);
                                if !matches!(a, SessionPanelAction::None) {
                                    action = a;
                                }
                            }
                        });
                    });

                    coll.store(ui.ctx());
                }
            }

            // Consume remaining space so we can detect right-click on empty area.
            let remaining = ui.available_size();
            let (_rect, empty_resp) =
                ui.allocate_exact_size(remaining, egui::Sense::click());
            empty_resp
        })
        .inner;

    // Right-click on empty space in the scroll area
    scroll_resp.context_menu(|ui: &mut Ui| {
        if ui.button("New Folder").clicked() {
            panel_state.new_folder_parent = Some(Vec::new());
            panel_state.new_folder_name.clear();
            panel_state.new_folder_focus = true;
            ui.close_menu();
        }
        if ui.button("New Session").clicked() {
            action = SessionPanelAction::CreateServer {
                folder_path: Vec::new(),
            };
            ui.close_menu();
        }
    });

    // Delete confirmation dialog
    if let Some(ref target) = panel_state.confirm_delete.clone() {
        let title = match target {
            DeleteTarget::Folder(path) => format!("Delete \"{}\"?", path.last().unwrap_or(&String::new())),
            DeleteTarget::Server(_) => "Delete server?".to_string(),
        };
        let detail = match target {
            DeleteTarget::Folder(_) => "This folder and all its contents will be removed.",
            DeleteTarget::Server(_) => "This server entry will be removed.",
        };

        egui::Window::new(title)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ui.ctx(), |ui| {
                ui.label(detail);
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Delete").clicked() {
                        match target {
                            DeleteTarget::Folder(path) => {
                                action = SessionPanelAction::DeleteFolder {
                                    path: path.clone(),
                                };
                            }
                            DeleteTarget::Server(addr) => {
                                action = SessionPanelAction::DeleteServer {
                                    addr: addr.clone(),
                                };
                            }
                        }
                        panel_state.confirm_delete = None;
                    }
                    if ui.button("Cancel").clicked() {
                        panel_state.confirm_delete = None;
                    }
                });
            });
    }

    action
}

// ---------------------------------------------------------------------------
// Folders
// ---------------------------------------------------------------------------

fn show_folder(
    ui: &mut Ui,
    folder: &ServerFolder,
    parent_path: &[String],
    icons: Option<&IconCache>,
    panel_state: &mut SessionPanelState,
    dark_mode: bool,
) -> SessionPanelAction {
    let mut action = SessionPanelAction::None;

    let folder_path: Vec<String> = parent_path
        .iter()
        .cloned()
        .chain(std::iter::once(folder.name.clone()))
        .collect();

    let id = ui.make_persistent_id(("folder", &folder_path));
    let mut coll = egui::collapsing_header::CollapsingState::load_with_default_open(
        ui.ctx(),
        id,
        false,
    );

    let is_renaming =
        panel_state.renaming.as_ref() == Some(&RenameTarget::Folder(folder_path.clone()));

    if is_renaming {
        // Inline rename field
        let resp = ui
            .horizontal(|ui| {
                let icon = if coll.is_open() {
                    Icon::FolderOpen
                } else {
                    Icon::Folder
                };
                if let Some(img) = icons.and_then(|ic| ic.image(icon)) {
                    ui.add(img);
                }
                let te = ui.add(
                    crate::ui::widgets::text_edit(&mut panel_state.rename_buf)
                        .desired_width(ui.available_width() - 4.0),
                );
                if panel_state.rename_focus {
                    te.request_focus();
                    panel_state.rename_focus = false;
                }
                te
            })
            .inner;

        if resp.lost_focus() {
            let new_name = panel_state.rename_buf.trim().to_string();
            panel_state.renaming = None;
            if !new_name.is_empty() && new_name != folder.name {
                action = SessionPanelAction::RenameFolder {
                    path: folder_path.clone(),
                    new_name,
                };
            }
            panel_state.rename_buf.clear();
        }
    } else {
        // Normal header row
        let header_resp = ui.horizontal(|ui| {
            let icon = if coll.is_open() {
                Icon::FolderOpen
            } else {
                Icon::Folder
            };
            if let Some(img) = icons.and_then(|ic| ic.image(icon)) {
                ui.add(img);
            }
            if ui
                .add(egui::Label::new(&folder.name).sense(egui::Sense::click()))
                .clicked()
            {
                coll.toggle(ui);
            }
        });

        if header_resp.response.clicked() {
            coll.toggle(ui);
        }

        // Right-click context menu
        header_resp.response.context_menu(|ui| {
            if ui.button("New Subfolder").clicked() {
                panel_state.new_folder_parent = Some(folder_path.clone());
                panel_state.new_folder_name.clear();
                panel_state.new_folder_focus = true;
                ui.close_menu();
            }
            if ui.button("New Session").clicked() {
                action = SessionPanelAction::CreateServer {
                    folder_path: folder_path.clone(),
                };
                ui.close_menu();
            }
            ui.separator();
            if ui.button("Rename").clicked() {
                panel_state.renaming = Some(RenameTarget::Folder(folder_path.clone()));
                panel_state.rename_buf = folder.name.clone();
                panel_state.rename_focus = true;
                ui.close_menu();
            }
            let has_children =
                !folder.servers.is_empty() || !folder.subfolders.is_empty();
            if ui.button("Delete").clicked() {
                if has_children {
                    panel_state.confirm_delete =
                        Some(DeleteTarget::Folder(folder_path.clone()));
                } else {
                    action = SessionPanelAction::DeleteFolder {
                        path: folder_path.clone(),
                    };
                }
                ui.close_menu();
            }
        });
    }

    coll.show_body_unindented(ui, |ui| {
        ui.indent(id, |ui| {
            for (i, server) in folder.servers.iter().enumerate() {
                let addr = ServerAddress {
                    folder_path: folder_path.clone(),
                    index: i,
                };
                let a = show_server_entry_editable(ui, server, &addr, icons, panel_state, dark_mode);
                if !matches!(a, SessionPanelAction::None) {
                    action = a;
                }
            }
            for sub in &folder.subfolders {
                let a = show_folder(ui, sub, &folder_path, icons, panel_state, dark_mode);
                if !matches!(a, SessionPanelAction::None) {
                    action = a;
                }
            }

            // Inline new-folder field if creating a subfolder here
            if panel_state.new_folder_parent.as_ref() == Some(&folder_path) {
                if let Some(a) = show_new_folder_input(ui, panel_state, icons) {
                    action = a;
                }
            }
        });
    });

    coll.store(ui.ctx());

    action
}

// ---------------------------------------------------------------------------
// Server entries
// ---------------------------------------------------------------------------

/// A user-defined server entry (supports right-click rename/delete/edit).
fn show_server_entry_editable(
    ui: &mut Ui,
    entry: &ServerEntry,
    addr: &ServerAddress,
    icons: Option<&IconCache>,
    panel_state: &mut SessionPanelState,
    dark_mode: bool,
) -> SessionPanelAction {
    let is_renaming =
        panel_state.renaming.as_ref() == Some(&RenameTarget::Server(addr.clone()));

    if is_renaming {
        let resp = ui
            .horizontal(|ui| {
                if let Some(img) = icons.and_then(|ic| ic.themed_image(Icon::Computer, dark_mode)) {
                    ui.add(img);
                }
                let te = ui.add(
                    crate::ui::widgets::text_edit(&mut panel_state.rename_buf)
                        .desired_width(ui.available_width() - 4.0),
                );
                if panel_state.rename_focus {
                    te.request_focus();
                    panel_state.rename_focus = false;
                }
                te
            })
            .inner;

        if resp.lost_focus() {
            let new_name = panel_state.rename_buf.trim().to_string();
            panel_state.renaming = None;
            if !new_name.is_empty() && new_name != entry.name {
                return SessionPanelAction::RenameServer {
                    addr: addr.clone(),
                    new_name,
                };
            }
            panel_state.rename_buf.clear();
        }
        return SessionPanelAction::None;
    }

    let resp = ui
        .horizontal(|ui| {
            if let Some(img) = icons.and_then(|ic| ic.themed_image(Icon::Computer, dark_mode)) {
                ui.add(img);
            }
            ui.add(
                egui::Label::new(entry.display_name()).sense(egui::Sense::click()),
            )
        })
        .inner;

    // Right-click context menu
    resp.context_menu(|ui| {
        if ui.button("Edit…").clicked() {
            // Stub — will open edit dialog later.
            ui.close_menu();
        }
        if ui.button("Rename").clicked() {
            panel_state.renaming = Some(RenameTarget::Server(addr.clone()));
            panel_state.rename_buf = entry.name.clone();
            panel_state.rename_focus = true;
            ui.close_menu();
        }
        if ui.button("Delete").clicked() {
            panel_state.confirm_delete = Some(DeleteTarget::Server(addr.clone()));
            ui.close_menu();
        }
    });

    if resp.clicked() {
        SessionPanelAction::Connect(SshConnectRequest {
            host: entry.host.clone(),
            port: entry.port,
            user: entry.user.clone(),
            identity_file: entry.identity_file.clone(),
            proxy_command: entry.proxy_command.clone(),
            proxy_jump: entry.proxy_jump.clone(),
        })
    } else {
        SessionPanelAction::None
    }
}

/// SSH-config entries are read-only — click to connect, no context menu.
fn show_server_entry_readonly(
    ui: &mut Ui,
    entry: &ServerEntry,
    icons: Option<&IconCache>,
    dark_mode: bool,
) -> SessionPanelAction {
    let clicked = ui
        .horizontal(|ui| {
            if let Some(img) = icons.and_then(|ic| ic.themed_image(Icon::Computer, dark_mode)) {
                ui.add(img);
            }
            ui.add(
                egui::Label::new(entry.display_name()).sense(egui::Sense::click()),
            )
            .clicked()
        })
        .inner;

    if clicked {
        SessionPanelAction::Connect(SshConnectRequest {
            host: entry.host.clone(),
            port: entry.port,
            user: entry.user.clone(),
            identity_file: entry.identity_file.clone(),
            proxy_command: entry.proxy_command.clone(),
            proxy_jump: entry.proxy_jump.clone(),
        })
    } else {
        SessionPanelAction::None
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Recursively collect all server entries from folders + ssh_hosts into a flat list.
fn collect_all_servers<'a>(
    folders: &'a [ServerFolder],
    ssh_hosts: &'a [ServerEntry],
) -> Vec<&'a ServerEntry> {
    fn walk<'a>(folders: &'a [ServerFolder], out: &mut Vec<&'a ServerEntry>) {
        for folder in folders {
            out.extend(folder.servers.iter());
            walk(&folder.subfolders, out);
        }
    }
    let mut result = Vec::new();
    walk(folders, &mut result);
    result.extend(ssh_hosts.iter());
    result
}

fn show_new_folder_input(
    ui: &mut Ui,
    panel_state: &mut SessionPanelState,
    icons: Option<&IconCache>,
) -> Option<SessionPanelAction> {
    let mut result = None;

    let resp = ui
        .horizontal(|ui| {
            let dark_mode = ui.visuals().dark_mode;
            if let Some(img) = icons.and_then(|ic| ic.themed_image(Icon::SidebarFolder, dark_mode)) {
                ui.add(img);
            }
            let te = ui.add(
                crate::ui::widgets::text_edit(&mut panel_state.new_folder_name)
                    .hint_text("Folder name\u{2026}")
                    .desired_width(ui.available_width() - 4.0),
            );
            if panel_state.new_folder_focus {
                te.request_focus();
                panel_state.new_folder_focus = false;
            }
            te
        })
        .inner;

    if resp.lost_focus() {
        let name = panel_state.new_folder_name.trim().to_string();
        let parent = panel_state.new_folder_parent.take().unwrap_or_default();
        panel_state.new_folder_name.clear();
        if !name.is_empty() {
            result = Some(SessionPanelAction::CreateFolder {
                parent_path: parent,
                name,
            });
        }
    }

    result
}

//! Left sidebar with vertical tab strip (Files, Tools, Macros) and content panels.
//!
//! Rendered as two adjacent side panels: a narrow fixed-width tab strip on the
//! far left, and a resizable content panel beside it.

use std::collections::HashMap;
use std::f32::consts::FRAC_PI_2;
use std::path::PathBuf;
use std::sync::Arc;

use egui::{
    Color32, Context, FontFamily, FontId, Pos2, Rect, Sense, Shape, Stroke, Vec2,
    epaint::TextShape,
};
use egui_extras::{TableBuilder, Column};

use crate::icons::{Icon, IconCache};
use crate::ui::file_browser::{FileBrowserState, FileListEntry, display_size, format_modified};

/// Which tab is active in the left sidebar.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum SidebarTab {
    #[default]
    Files,
    Plugins,
    /// A panel plugin tab, identified by plugin index.
    PluginPanel(usize),
}

/// Actions that can be triggered by the sidebar.
#[allow(dead_code)]
pub enum SidebarAction {
    None,
    NavigateLocal(PathBuf),
    SelectFile(PathBuf),
    NavigateRemote(PathBuf),
    RefreshLocal,
    RefreshRemote,
    GoHomeLocal,
    GoHomeRemote,
    GoBackLocal,
    GoForwardLocal,
    GoBackRemote,
    GoForwardRemote,
    NavigateLocal2(PathBuf),
    RefreshLocal2,
    GoHomeLocal2,
    GoBackLocal2,
    GoForwardLocal2,
    /// Copy a file from one local pane to the other.
    CopyLocal { source: PathBuf, dest_dir: PathBuf },
    /// Upload a local file to the current remote directory.
    Upload { local_path: PathBuf, remote_dir: PathBuf },
    /// Download a remote file to the current local directory.
    Download { remote_path: PathBuf, local_dir: PathBuf },
    /// Cancel an in-progress transfer by filename.
    CancelTransfer(String),
    RunPlugin(usize),
    RefreshPlugins,
    /// Apply plugin load/unload changes (carries list of plugin indices that should be loaded).
    ApplyPluginChanges(Vec<usize>),
    /// A button was clicked in a panel plugin.
    PanelButtonClick { plugin_idx: usize, button_id: String },
    /// Deactivate (stop) a panel plugin.
    DeactivatePanel(usize),
}

/// Transfer progress shown in the sidebar.
#[derive(Clone)]
pub struct TransferStatus {
    pub filename: String,
    pub upload: bool,
    pub done: bool,
    pub error: Option<String>,
    pub bytes_transferred: u64,
    pub total_bytes: u64,
    /// Set to true to cancel this transfer.
    pub cancel: Arc<std::sync::atomic::AtomicBool>,
}

/// Width of the vertical tab strip in pixels.
const TAB_STRIP_WIDTH: f32 = 28.0;

/// Width of the accent bar on the selected tab's right edge.
const ACCENT_WIDTH: f32 = 3.0;

/// A tab entry for the sidebar tab strip (static or dynamic).
struct TabEntry {
    tab: SidebarTab,
    label: String,
    icon: Icon,
    /// Optional plugin icon texture (overrides the default Icon for plugin tabs).
    plugin_tex_id: Option<egui::TextureId>,
}

/// Render the narrow vertical tab strip (far-left panel).
/// `panel_tabs` is a list of (plugin_index, name) for active panel plugins.
/// `plugin_icons` maps plugin index to loaded texture handles.
pub fn show_tab_strip(
    ctx: &Context,
    active_tab: &mut SidebarTab,
    icons: Option<&IconCache>,
    panel_tabs: &[(usize, String)],
    plugin_icons: &HashMap<usize, egui::TextureHandle>,
    plugins_enabled: bool,
    panel_id: egui::Id,
) {
    // Build the tab list: fixed tabs + dynamic panel plugin tabs.
    let mut tabs = vec![
        TabEntry { tab: SidebarTab::Files, label: "Files".into(), icon: Icon::TabFiles, plugin_tex_id: None },
    ];
    if plugins_enabled {
        tabs.push(TabEntry { tab: SidebarTab::Plugins, label: "Plugins".into(), icon: Icon::TabTools, plugin_tex_id: None });
    }
    for (idx, name) in panel_tabs {
        tabs.push(TabEntry {
            tab: SidebarTab::PluginPanel(*idx),
            label: name.clone(),
            icon: Icon::TabTools,
            plugin_tex_id: plugin_icons.get(idx).map(|h| h.id()),
        });
    }

    egui::SidePanel::left(panel_id)
        .resizable(false)
        .exact_width(TAB_STRIP_WIDTH)
        .frame(egui::Frame::NONE)
        .show(ctx, |ui| {
            let panel_rect = ui.available_rect_before_wrap();
            let painter = ui.painter_at(panel_rect);

            let style = ui.style();
            let base_bg = style.visuals.panel_fill;
            let darker_bg = darken_color(base_bg, 18);
            let accent_color = Color32::from_rgb(47, 101, 202);
            let text_color = style.visuals.strong_text_color();
            let font_id = FontId::new(13.0, FontFamily::Proportional);

            let tab_height = panel_rect.height() / tabs.len() as f32;

            painter.rect_filled(panel_rect, 0.0, darker_bg);

            for (i, entry) in tabs.iter().enumerate() {
                let y_min = panel_rect.min.y + i as f32 * tab_height;
                let tab_rect = Rect::from_min_size(
                    Pos2::new(panel_rect.min.x, y_min),
                    Vec2::new(TAB_STRIP_WIDTH, tab_height),
                );

                let selected = *active_tab == entry.tab;

                if selected {
                    painter.rect_filled(tab_rect, 0.0, base_bg);

                    let accent_rect = Rect::from_min_size(
                        Pos2::new(tab_rect.max.x - ACCENT_WIDTH, tab_rect.min.y),
                        Vec2::new(ACCENT_WIDTH, tab_height),
                    );
                    painter.rect_filled(accent_rect, 0.0, accent_color);
                }

                let galley =
                    painter.layout_no_wrap(entry.label.clone(), font_id.clone(), text_color);
                let text_w = galley.size().x;
                let text_h = galley.size().y;

                let icon_size = 16.0;
                let gap = 4.0;
                let total_h = text_w + gap + icon_size;

                let cx = tab_rect.center().x;
                let cy = tab_rect.center().y;

                let text_top = cy - total_h / 2.0;
                let pos = Pos2::new(cx - text_h / 2.0, text_top + text_w);

                let text_shape = TextShape::new(pos, Arc::clone(&galley), text_color)
                    .with_angle(-FRAC_PI_2);
                painter.add(Shape::Text(text_shape));

                let tex_id = entry.plugin_tex_id
                    .or_else(|| icons.and_then(|ic| ic.texture_id(entry.icon)));
                if let Some(tex_id) = tex_id {
                    let icon_top = text_top + text_w + gap;
                    let icon_rect = Rect::from_min_size(
                        Pos2::new(cx - icon_size / 2.0, icon_top),
                        Vec2::new(icon_size, icon_size),
                    );
                    painter.image(
                        tex_id,
                        icon_rect,
                        Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                        Color32::WHITE,
                    );
                }

                if i > 0 {
                    painter.line_segment(
                        [
                            Pos2::new(panel_rect.min.x + 4.0, y_min),
                            Pos2::new(panel_rect.max.x - 4.0, y_min),
                        ],
                        Stroke::new(
                            1.0,
                            style.visuals.widgets.noninteractive.bg_stroke.color,
                        ),
                    );
                }

                let response =
                    ui.interact(tab_rect, ui.id().with(("sidebar_tab", i)), Sense::click());
                if response.clicked() {
                    *active_tab = entry.tab.clone();
                }
            }

            painter.line_segment(
                [
                    Pos2::new(panel_rect.max.x, panel_rect.min.y),
                    Pos2::new(panel_rect.max.x, panel_rect.max.y),
                ],
                Stroke::new(1.0, style.visuals.widgets.noninteractive.bg_stroke.color),
            );
        });
}

/// Info about a discovered plugin, passed from the app to the sidebar for rendering.
pub struct PluginDisplayInfo {
    pub name: String,
    pub description: String,
    pub is_panel: bool,
    pub is_bottom_panel: bool,
    pub is_loaded: bool,
}

/// Render the sidebar content panel (file browser, plugins, or panel plugin).
/// Always shown with a stable panel ID so the user-resized width persists
/// across tab switches.
pub fn show_sidebar_content(
    ctx: &Context,
    active_tab: &SidebarTab,
    file_browser_state: &mut FileBrowserState,
    icons: Option<&IconCache>,
    plugins: &[PluginDisplayInfo],
    plugin_output: &[String],
    selected_plugin: &mut Option<usize>,
    transfers: &[TransferStatus],
    plugin_search_query: &mut String,
    plugin_search_focus: &mut bool,
    panel_widgets: &HashMap<usize, Vec<conch_plugin::PanelWidget>>,
    panel_names: &HashMap<usize, String>,
    pending_plugin_loads: &mut Vec<bool>,
    panel_id: egui::Id,
) -> SidebarAction {
    let mut action = SidebarAction::None;

    egui::SidePanel::left(panel_id)
        .resizable(true)
        .default_width(200.0)
        .min_width(100.0)
        .max_width(400.0)
        .show(ctx, |ui| {
            let w = ui.available_width();
            ui.set_min_width(w);
            ui.set_max_width(w);

            action = match active_tab {
                SidebarTab::Files => show_files_panel(ui, file_browser_state, icons, transfers),
                SidebarTab::Plugins => show_plugins_panel(ui, plugins, plugin_output, selected_plugin, icons, plugin_search_query, plugin_search_focus, pending_plugin_loads),
                SidebarTab::PluginPanel(idx) => {
                    let name = panel_names.get(idx).map(|s| s.as_str()).unwrap_or("Panel");
                    let widgets = panel_widgets.get(idx).map(|v| v.as_slice()).unwrap_or(&[]);
                    show_panel_plugin(ui, *idx, name, widgets)
                }
            };
        });

    action
}

fn show_plugins_panel(
    ui: &mut egui::Ui,
    plugins: &[PluginDisplayInfo],
    output: &[String],
    _selected: &mut Option<usize>,
    icons: Option<&IconCache>,
    search_query: &mut String,
    search_focus: &mut bool,
    pending_loads: &mut Vec<bool>,
) -> SidebarAction {
    let mut action = SidebarAction::None;
    let dark_mode = ui.visuals().dark_mode;

    // Ensure pending_loads is synced with plugin count.
    if pending_loads.len() != plugins.len() {
        *pending_loads = plugins.iter().map(|p| p.is_loaded).collect();
    }

    // Header with refresh icon button.
    ui.horizontal(|ui| {
        ui.strong("Plugins");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let clicked = if let Some(img) = icons.and_then(|ic| ic.themed_image(Icon::Refresh, dark_mode)) {
                ui.add(egui::ImageButton::new(img).frame(false))
                    .on_hover_text("Refresh")
                    .clicked()
            } else {
                ui.small_button("\u{21BB}")
                    .on_hover_text("Refresh")
                    .clicked()
            };
            if clicked {
                action = SidebarAction::RefreshPlugins;
            }
        });
    });
    ui.separator();

    // Search bar
    let search_resp = ui.add(
        crate::ui::widgets::text_edit(search_query)
            .hint_text("Search plugins...")
            .desired_width(ui.available_width()),
    );
    if *search_focus {
        search_resp.request_focus();
        *search_focus = false;
    }

    // Build filtered list: (original_index, &PluginDisplayInfo)
    let query_lower = search_query.to_lowercase();
    let filtered: Vec<(usize, &PluginDisplayInfo)> = plugins
        .iter()
        .enumerate()
        .filter(|(_, p)| {
            query_lower.is_empty()
                || p.name.to_lowercase().contains(&query_lower)
                || p.description.to_lowercase().contains(&query_lower)
        })
        .collect();

    // Enter on search bar → run first matching loaded action plugin
    if search_resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
        if let Some(&(orig_idx, plugin)) = filtered.first() {
            if !plugin.is_panel && !plugin.is_bottom_panel && plugin.is_loaded {
                action = SidebarAction::RunPlugin(orig_idx);
            }
        }
    }

    ui.add_space(2.0);

    if plugins.is_empty() {
        ui.weak("No plugins found");
        ui.add_space(4.0);
        ui.weak("Place .lua files in:");
        ui.small("~/.config/conch/plugins/");
    } else if filtered.is_empty() {
        ui.weak("No matching plugins");
    } else {
        // Check if there are pending changes
        let has_changes = plugins.iter().enumerate().any(|(i, p)| {
            pending_loads.get(i).copied().unwrap_or(false) != p.is_loaded
        });

        // Reserve space for apply button + output at the bottom.
        let btn_bar_height = if has_changes { 36.0 } else { 0.0 };
        let output_height = 100.0;
        let reserved = btn_bar_height + output_height + 30.0;
        let list_height = (ui.available_height() - reserved).max(60.0);

        // Scrollable plugin list with checkboxes.
        egui::ScrollArea::vertical()
            .id_salt("plugin_list")
            .max_height(list_height)
            .show(ui, |ui| {
                for &(i, plugin) in &filtered {
                    ui.push_id(i, |ui| {
                        ui.horizontal(|ui| {
                            // Checkbox for load/unload
                            if let Some(checked) = pending_loads.get_mut(i) {
                                ui.checkbox(checked, "");
                            }

                            ui.vertical(|ui| {
                                // Name + type badge
                                ui.horizontal(|ui| {
                                    ui.label(egui::RichText::new(&plugin.name).size(12.0));
                                    if plugin.is_bottom_panel {
                                        ui.label(
                                            egui::RichText::new("bottom")
                                                .size(9.0)
                                                .color(Color32::from_rgb(180, 140, 80)),
                                        );
                                    } else if plugin.is_panel {
                                        ui.label(
                                            egui::RichText::new("panel")
                                                .size(9.0)
                                                .color(Color32::from_rgb(100, 160, 220)),
                                        );
                                    }
                                });

                                // Status indicator
                                if plugin.is_loaded {
                                    ui.label(
                                        egui::RichText::new("\u{25cf} Loaded")
                                            .size(10.0)
                                            .color(Color32::from_rgb(60, 180, 60)),
                                    );
                                } else {
                                    ui.label(
                                        egui::RichText::new("\u{25cb} Not loaded")
                                            .size(10.0)
                                            .color(Color32::from_rgb(140, 140, 140)),
                                    );
                                }

                                // Description
                                if !plugin.description.is_empty() {
                                    ui.label(
                                        egui::RichText::new(&plugin.description)
                                            .size(10.0)
                                            .weak(),
                                    );
                                }
                            });
                        });
                    });

                    ui.separator();
                }
            });

        // Apply button (only visible when there are pending changes)
        if has_changes {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Apply").clicked() {
                        let loaded_indices: Vec<usize> = pending_loads
                            .iter()
                            .enumerate()
                            .filter_map(|(i, &loaded)| if loaded { Some(i) } else { None })
                            .collect();
                        action = SidebarAction::ApplyPluginChanges(loaded_indices);
                    }
                });
            });
        }
    }

    // Output panel at bottom.
    ui.add_space(4.0);
    ui.strong("Output");
    ui.separator();
    egui::ScrollArea::vertical()
        .id_salt("plugin_output")
        .stick_to_bottom(true)
        .max_height(100.0)
        .show(ui, |ui| {
            for line in output {
                ui.label(egui::RichText::new(line).size(11.0).monospace());
            }
        });

    action
}

/// Which pane we're rendering inside the file browser.
#[derive(Clone, Copy, PartialEq, Eq)]
enum PaneKind {
    Remote,
    Local,
    /// Second local pane (shown when no remote session is active).
    Local2,
}

fn show_files_panel(
    ui: &mut egui::Ui,
    state: &mut FileBrowserState,
    icons: Option<&IconCache>,
    transfers: &[TransferStatus],
) -> SidebarAction {
    let mut action = SidebarAction::None;

    let available = ui.available_height();
    let remote_connected = state.remote_path.is_some();

    // Reserve space for transfer buttons, transfer progress, etc.
    let transfer_height = if transfers.is_empty() { 0.0 } else { 100.0 };
    let button_bar_height = 28.0;

    // Always show two panes: remote+local when connected, local+local2 otherwise.
    let pane_height = ((available - button_bar_height - 12.0 - transfer_height) / 2.0).max(60.0);

    // Top pane: remote (if connected) or second local.
    let top_pane_kind = if remote_connected { PaneKind::Remote } else { PaneKind::Local2 };
    ui.allocate_ui(Vec2::new(ui.available_width(), pane_height), |ui| {
        ui.set_min_height(pane_height);
        ui.push_id("top_pane", |ui| {
            let a = show_file_pane(ui, state, top_pane_kind, icons);
            if !matches!(a, SidebarAction::None) {
                action = a;
            }
        });
    });

    // Button bar between the panes.
    ui.add_space(2.0);
    if remote_connected {
        // Upload / Download buttons for remote transfers.
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;

            let can_upload = state.local_selected.is_some() && state.remote_path.is_some();
            if ui
                .add_enabled(can_upload, egui::Button::new("\u{2191} Upload").small())
                .on_hover_text("Upload selected local item to remote directory")
                .clicked()
            {
                if let (Some(idx), Some(remote_dir)) =
                    (state.local_selected, state.remote_path.clone())
                {
                    if let Some(entry) = state.local_entries.get(idx) {
                        action = SidebarAction::Upload {
                            local_path: entry.path.clone(),
                            remote_dir,
                        };
                    }
                }
            }

            let can_download = state.remote_selected.is_some();
            if ui
                .add_enabled(can_download, egui::Button::new("\u{2193} Download").small())
                .on_hover_text("Download selected remote item to local directory")
                .clicked()
            {
                if let Some(idx) = state.remote_selected {
                    if let Some(entry) = state.remote_entries.get(idx) {
                        action = SidebarAction::Download {
                            remote_path: entry.path.clone(),
                            local_dir: state.local_path.clone(),
                        };
                    }
                }
            }
        });
    } else {
        // Copy buttons between two local panes.
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;

            // Copy from local → local2
            let can_copy_down = state.local_selected.is_some();
            if ui
                .add_enabled(can_copy_down, egui::Button::new("\u{2193} Copy \u{2193}").small())
                .on_hover_text("Copy selected file to the other pane's directory")
                .clicked()
            {
                if let Some(idx) = state.local_selected {
                    if let Some(entry) = state.local_entries.get(idx) {
                        action = SidebarAction::CopyLocal {
                            source: entry.path.clone(),
                            dest_dir: state.local2_path.clone(),
                        };
                    }
                }
            }

            // Copy from local2 → local
            let can_copy_up = state.local2_selected.is_some();
            if ui
                .add_enabled(can_copy_up, egui::Button::new("\u{2191} Copy \u{2191}").small())
                .on_hover_text("Copy selected file to the other pane's directory")
                .clicked()
            {
                if let Some(idx) = state.local2_selected {
                    if let Some(entry) = state.local2_entries.get(idx) {
                        action = SidebarAction::CopyLocal {
                            source: entry.path.clone(),
                            dest_dir: state.local_path.clone(),
                        };
                    }
                }
            }
        });
    }
    ui.add_space(2.0);

    // Bottom pane: always local.
    ui.allocate_ui(Vec2::new(ui.available_width(), pane_height), |ui| {
        ui.push_id("local_pane", |ui| {
            let a = show_file_pane(ui, state, PaneKind::Local, icons);
            if !matches!(a, SidebarAction::None) {
                action = a;
            }
        });
    });

    // Transfer progress area at the bottom.
    if !transfers.is_empty() {
        ui.separator();
        ui.small("Transfers");
        egui::ScrollArea::vertical()
            .id_salt("transfer_progress")
            .max_height(100.0)
            .stick_to_bottom(true)
            .show(ui, |ui| {
                for ts in transfers {
                    let arrow = if ts.upload { "\u{2191}" } else { "\u{2193}" };
                    if ts.done {
                        let (label, color) = if let Some(e) = &ts.error {
                            (format!("{arrow} {} — {e}", ts.filename), Color32::from_rgb(255, 100, 100))
                        } else {
                            (
                                format!("{arrow} {} — {} done", ts.filename, display_size(ts.total_bytes)),
                                Color32::from_rgb(100, 200, 100),
                            )
                        };
                        ui.add(egui::Label::new(
                            egui::RichText::new(label).size(10.0).color(color),
                        ).truncate());
                    } else {
                        // In-progress: filename row with cancel button.
                        ui.horizontal(|ui| {
                            ui.add(egui::Label::new(
                                egui::RichText::new(format!("{arrow} {}", ts.filename))
                                    .size(10.0),
                            ).truncate());
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if ui
                                    .add(egui::Button::new(
                                        egui::RichText::new("\u{2715}").size(9.0),
                                    ).small().frame(false))
                                    .on_hover_text("Cancel transfer")
                                    .clicked()
                                {
                                    action = SidebarAction::CancelTransfer(ts.filename.clone());
                                }
                            });
                        });
                        // Thin progress bar + size label.
                        let frac = if ts.total_bytes > 0 {
                            ts.bytes_transferred as f32 / ts.total_bytes as f32
                        } else {
                            0.0
                        };
                        let size_text = format!(
                            "{} / {}",
                            display_size(ts.bytes_transferred),
                            display_size(ts.total_bytes),
                        );
                        ui.horizontal(|ui| {
                            let bar_width = (ui.available_width() - 90.0).max(20.0);
                            let bar_height = 6.0;
                            let (rect, _) = ui.allocate_exact_size(
                                Vec2::new(bar_width, bar_height),
                                Sense::hover(),
                            );
                            let track_color = ui.visuals().widgets.inactive.bg_fill;
                            ui.painter().rect_filled(rect, 3.0, track_color);
                            if frac > 0.0 {
                                let fill_rect = Rect::from_min_size(
                                    rect.min,
                                    Vec2::new(rect.width() * frac, bar_height),
                                );
                                let accent = ui.visuals().selection.bg_fill;
                                ui.painter().rect_filled(fill_rect, 3.0, accent);
                            }
                            ui.label(egui::RichText::new(size_text).size(9.0).weak());
                        });
                        ui.add_space(2.0);
                    }
                }
            });
    }

    action
}

fn show_file_pane(
    ui: &mut egui::Ui,
    state: &mut FileBrowserState,
    kind: PaneKind,
    icons: Option<&IconCache>,
) -> SidebarAction {
    use crate::ui::file_browser::FileBrowserPane;

    let mut action = SidebarAction::None;
    let pane_focused = state.focused && match kind {
        PaneKind::Local | PaneKind::Local2 => state.active_pane == FileBrowserPane::Local,
        PaneKind::Remote => state.active_pane == FileBrowserPane::Remote,
    };

    let (label, entries, current_path, path_edit, selected): (&str, &[FileListEntry], Option<&PathBuf>, &mut String, &mut Option<usize>) = match kind {
        PaneKind::Remote => (
            "Remote",
            &state.remote_entries as &[_],
            state.remote_path.as_ref(),
            &mut state.remote_path_edit,
            &mut state.remote_selected,
        ),
        PaneKind::Local => (
            "Local",
            &state.local_entries as &[_],
            Some(&state.local_path),
            &mut state.local_path_edit,
            &mut state.local_selected,
        ),
        PaneKind::Local2 => (
            "Local",
            &state.local2_entries as &[_],
            Some(&state.local2_path),
            &mut state.local2_path_edit,
            &mut state.local2_selected,
        ),
    };

    // Header (highlight when this pane has keyboard focus).
    if pane_focused {
        let accent = Color32::from_rgb(47, 101, 202);
        ui.colored_label(accent, egui::RichText::new(format!("▸ {label}")).strong());
    } else {
        ui.strong(label);
    }

    // Check if remote is disconnected
    if kind == PaneKind::Remote && current_path.is_none() {
        ui.add_space(8.0);
        ui.weak("No remote session");
        return action;
    }

    let dark_mode = ui.visuals().dark_mode;
    let (back_stack, forward_stack) = match kind {
        PaneKind::Local => (&state.local_back_stack, &state.local_forward_stack),
        PaneKind::Remote => (&state.remote_back_stack, &state.remote_forward_stack),
        PaneKind::Local2 => (&state.local2_back_stack, &state.local2_forward_stack),
    };
    let has_back = !back_stack.is_empty();
    let has_forward = !forward_stack.is_empty();
    let row_height = 24.0;
    let mut back_clicked = false;
    let mut forward_clicked = false;
    let mut home_clicked = false;
    let mut refresh_clicked = false;
    let mut path_submitted = false;
    ui.allocate_ui_with_layout(
        Vec2::new(ui.available_width(), row_height),
        egui::Layout::right_to_left(egui::Align::Center),
        |ui| {
        ui.spacing_mut().item_spacing.x = 4.0;

        // Right side: refresh, then home (right-to-left order).
        ui.add_space(2.0);
        refresh_clicked = if let Some(img) = icons.and_then(|ic| ic.themed_image(Icon::Refresh, dark_mode)) {
            ui.add(egui::ImageButton::new(img).frame(false))
                .on_hover_text("Refresh")
                .clicked()
        } else {
            ui.small_button("\u{21BB}")
                .on_hover_text("Refresh")
                .clicked()
        };

        home_clicked = if let Some(img) = icons.and_then(|ic| ic.themed_image(Icon::GoHome, dark_mode)) {
            ui.add(egui::ImageButton::new(img).frame(false))
                .on_hover_text("Home")
                .clicked()
        } else {
            ui.small_button("\u{2302}")
                .on_hover_text("Home")
                .clicked()
        };

        // Left side: back, forward, path edit (left-to-right nested).
        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            ui.add_space(2.0);

            back_clicked = if let Some(img) = icons.and_then(|ic| ic.themed_image(Icon::GoPrevious, dark_mode)) {
                let btn = ui.add_enabled(has_back, egui::ImageButton::new(img).frame(false));
                btn.on_hover_text("Back").clicked()
            } else {
                let btn = ui.add_enabled(has_back, egui::Button::new("\u{2190}").small());
                btn.on_hover_text("Back").clicked()
            };

            forward_clicked = if let Some(img) = icons.and_then(|ic| ic.themed_image(Icon::GoNext, dark_mode)) {
                let btn = ui.add_enabled(has_forward, egui::ImageButton::new(img).frame(false));
                btn.on_hover_text("Forward").clicked()
            } else {
                let btn = ui.add_enabled(has_forward, egui::Button::new("\u{2192}").small());
                btn.on_hover_text("Forward").clicked()
            };

            let response = ui.add(
                crate::ui::widgets::text_edit(path_edit)
                    .desired_width(ui.available_width()),
            );
            if response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                path_submitted = true;
            }
        });
    });

    if back_clicked {
        match kind {
            PaneKind::Local => action = SidebarAction::GoBackLocal,
            PaneKind::Remote => action = SidebarAction::GoBackRemote,
            PaneKind::Local2 => action = SidebarAction::GoBackLocal2,
        }
    }
    if forward_clicked {
        match kind {
            PaneKind::Local => action = SidebarAction::GoForwardLocal,
            PaneKind::Remote => action = SidebarAction::GoForwardRemote,
            PaneKind::Local2 => action = SidebarAction::GoForwardLocal2,
        }
    }
    if path_submitted {
        let target = PathBuf::from(path_edit.as_str());
        match kind {
            PaneKind::Local => action = SidebarAction::NavigateLocal(target),
            PaneKind::Remote => action = SidebarAction::NavigateRemote(target),
            PaneKind::Local2 => action = SidebarAction::NavigateLocal2(target),
        }
    }
    if home_clicked {
        match kind {
            PaneKind::Local => action = SidebarAction::GoHomeLocal,
            PaneKind::Remote => action = SidebarAction::GoHomeRemote,
            PaneKind::Local2 => action = SidebarAction::GoHomeLocal2,
        }
    }
    if refresh_clicked {
        match kind {
            PaneKind::Local => action = SidebarAction::RefreshLocal,
            PaneKind::Remote => action = SidebarAction::RefreshRemote,
            PaneKind::Local2 => action = SidebarAction::RefreshLocal2,
        }
    }

    // File table — reserve space for the status bar below.
    // The 18px covers the "N items" label + spacing; the extra 20px accounts
    // for the table header row that sits outside max_scroll_height.
    let status_bar_height = 38.0;
    let table_height = (ui.available_height() - status_bar_height).max(0.0);
    TableBuilder::new(ui)
        .striped(true)
        .resizable(true)
        .max_scroll_height(table_height)
        .column(Column::initial(100.0).at_least(60.0).resizable(true))
        .column(Column::auto().at_least(40.0).resizable(true))
        .column(Column::remainder().at_least(70.0))
        .header(16.0, |mut header| {
            header.col(|ui| { ui.label(egui::RichText::new("Name").strong().size(10.0)); });
            header.col(|ui| { ui.label(egui::RichText::new("Size").strong().size(10.0)); });
            header.col(|ui| { ui.label(egui::RichText::new("Modified").strong().size(10.0)); });
        })
        .body(|body| {
            body.rows(16.0, entries.len(), |mut row| {
                let idx = row.index();
                let entry = &entries[idx];
                let is_selected = *selected == Some(idx);

                row.col(|ui| {
                    // Draw selection highlight behind the entry.
                    if is_selected {
                        let rect = ui.available_rect_before_wrap();
                        ui.painter().rect_filled(rect, 0.0, ui.visuals().selection.bg_fill);
                    }

                    let resp = ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 3.0;
                        let icon = if entry.is_dir { Icon::SidebarFolder } else { Icon::File };
                        if let Some(img) = icons.and_then(|ic| ic.themed_image(icon, dark_mode)) {
                            ui.add(img.fit_to_exact_size(Vec2::new(14.0, 14.0)));
                        }
                        ui.add(
                            egui::Label::new(egui::RichText::new(&entry.name).size(12.0))
                                .truncate()
                                .sense(Sense::click()),
                        )
                    }).inner;

                    // Single click → select.
                    if resp.clicked() {
                        *selected = Some(idx);
                    }

                    // Double click on directory → navigate.
                    if resp.double_clicked() && entry.is_dir {
                        *selected = None;
                        match kind {
                            PaneKind::Local => action = SidebarAction::NavigateLocal(entry.path.clone()),
                            PaneKind::Remote => action = SidebarAction::NavigateRemote(entry.path.clone()),
                            PaneKind::Local2 => action = SidebarAction::NavigateLocal2(entry.path.clone()),
                        }
                    }
                });

                row.col(|ui| {
                    let size_text = if entry.is_dir {
                        "<DIR>".to_string()
                    } else {
                        display_size(entry.size)
                    };
                    ui.label(egui::RichText::new(size_text).size(11.0).weak());
                });

                row.col(|ui| {
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(format_modified(entry.modified))
                                .size(11.0)
                                .weak(),
                        )
                        .truncate(),
                    );
                });
            });
        });

    // Status bar
    ui.add_space(2.0);
    ui.small(format!("{} items", entries.len()));

    action
}

/// Render a panel plugin's declarative widgets in the sidebar content area.
fn show_panel_plugin(
    ui: &mut egui::Ui,
    plugin_idx: usize,
    name: &str,
    widgets: &[conch_plugin::PanelWidget],
) -> SidebarAction {
    let mut action = SidebarAction::None;

    // Header
    ui.horizontal(|ui| {
        ui.strong(name);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.small_button("\u{2715}").on_hover_text("Close panel").clicked() {
                action = SidebarAction::DeactivatePanel(plugin_idx);
            }
        });
    });
    ui.separator();

    if widgets.is_empty() {
        ui.weak("Loading...");
        return action;
    }

    // Check if any widget is ScrollText — if so, split rendering to avoid
    // nested vertical ScrollAreas (inner would get unbounded height from outer).
    let has_scroll_text = widgets.iter().any(|w| matches!(w, conch_plugin::PanelWidget::ScrollText(_)));

    // Render non-ScrollText widgets in a scroll area (or directly if ScrollText present).
    let render_regular = |ui: &mut egui::Ui, action: &mut SidebarAction| {
        for widget in widgets {
            match widget {
                conch_plugin::PanelWidget::ScrollText(_) => {} // rendered separately below
                conch_plugin::PanelWidget::Heading(text) => {
                    ui.add_space(4.0);
                    ui.strong(text);
                    ui.add_space(2.0);
                }
                conch_plugin::PanelWidget::Text(text) => {
                    ui.label(egui::RichText::new(text).monospace().size(11.0));
                }
                conch_plugin::PanelWidget::Label(text) => {
                    ui.label(text);
                }
                conch_plugin::PanelWidget::Separator => {
                    ui.separator();
                }
                conch_plugin::PanelWidget::Table { columns, rows } => {
                    let num_cols = columns.len();
                    if num_cols > 0 {
                        TableBuilder::new(ui)
                            .striped(true)
                            .resizable(true)
                            .columns(Column::remainder().at_least(40.0), num_cols)
                            .header(16.0, |mut header| {
                                for col in columns {
                                    header.col(|ui| {
                                        ui.label(
                                            egui::RichText::new(col).strong().size(10.0),
                                        );
                                    });
                                }
                            })
                            .body(|body| {
                                body.rows(16.0, rows.len(), |mut row| {
                                    let idx = row.index();
                                    if let Some(cells) = rows.get(idx) {
                                        for cell in cells {
                                            row.col(|ui| {
                                                ui.label(
                                                    egui::RichText::new(cell)
                                                        .size(11.0)
                                                        .monospace(),
                                                );
                                            });
                                        }
                                    }
                                });
                            });
                    }
                }
                conch_plugin::PanelWidget::Progress {
                    label,
                    fraction,
                    text,
                } => {
                    ui.label(egui::RichText::new(label).size(11.0));
                    let bar = egui::ProgressBar::new(*fraction)
                        .text(text)
                        .desired_width(ui.available_width());
                    ui.add(bar);
                }
                conch_plugin::PanelWidget::Button { id, label } => {
                    if ui.button(label).clicked() {
                        *action = SidebarAction::PanelButtonClick {
                            plugin_idx,
                            button_id: id.clone(),
                        };
                    }
                }
                conch_plugin::PanelWidget::KeyValue { key, value } => {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(key).strong().size(11.0));
                        ui.label(egui::RichText::new(value).size(11.0).monospace());
                    });
                }
            }
        }
    };

    if has_scroll_text {
        // When ScrollText is present, render regular widgets without an outer
        // scroll area, then let ScrollText fill the remaining space.
        render_regular(ui, &mut action);
        for widget in widgets {
            if let conch_plugin::PanelWidget::ScrollText(lines) = widget {
                egui::ScrollArea::vertical()
                    .id_salt(("sidebar_scroll_text", plugin_idx))
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        for line in lines {
                            ui.label(egui::RichText::new(line).monospace().size(11.0));
                        }
                    });
            }
        }
    } else {
        egui::ScrollArea::vertical()
            .id_salt(("panel_plugin", plugin_idx))
            .show(ui, |ui| {
                render_regular(ui, &mut action);
            });
    }

    action
}

/// Darken a `Color32` by subtracting `amount` from each RGB channel.
pub fn darken_color(color: Color32, amount: u8) -> Color32 {
    Color32::from_rgba_premultiplied(
        color.r().saturating_sub(amount),
        color.g().saturating_sub(amount),
        color.b().saturating_sub(amount),
        color.a(),
    )
}

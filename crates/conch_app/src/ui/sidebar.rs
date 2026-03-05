//! Left sidebar with vertical tab strip (Files, Tools, Macros) and content panels.
//!
//! Rendered as two adjacent side panels: a narrow fixed-width tab strip on the
//! far left, and a resizable content panel beside it.

use std::f32::consts::FRAC_PI_2;
use std::path::PathBuf;
use std::sync::Arc;

use egui::{
    Color32, Context, FontFamily, FontId, Pos2, Rect, Sense, Shape, Stroke, Vec2,
    epaint::TextShape,
};
use egui_extras::{TableBuilder, Column};

use crate::icons::{Icon, IconCache};
use crate::ui::file_browser::{FileBrowserState, display_size, format_modified};

/// Which tab is active in the left sidebar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SidebarTab {
    #[default]
    Files,
    Plugins,
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
    RunPlugin(usize),
    StopPlugin(usize),
    RefreshPlugins,
}

/// Width of the vertical tab strip in pixels.
const TAB_STRIP_WIDTH: f32 = 28.0;

/// Width of the accent bar on the selected tab's right edge.
const ACCENT_WIDTH: f32 = 3.0;

const TABS: &[(SidebarTab, &str, Icon)] = &[
    (SidebarTab::Files, "Files", Icon::TabFiles),
    (SidebarTab::Plugins, "Plugins", Icon::TabTools),
];

/// Render the narrow vertical tab strip (far-left panel).
pub fn show_tab_strip(ctx: &Context, active_tab: &mut SidebarTab, icons: Option<&IconCache>) {
    egui::SidePanel::left("sidebar_tabs")
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
            let text_color = style.visuals.text_color();
            let font_id = FontId::new(11.0, FontFamily::Proportional);

            let tab_height = panel_rect.height() / TABS.len() as f32;

            // Fill the entire strip with the darker background first.
            painter.rect_filled(panel_rect, 0.0, darker_bg);

            for (i, &(tab, label, icon)) in TABS.iter().enumerate() {
                let y_min = panel_rect.min.y + i as f32 * tab_height;
                let tab_rect = Rect::from_min_size(
                    Pos2::new(panel_rect.min.x, y_min),
                    Vec2::new(TAB_STRIP_WIDTH, tab_height),
                );

                let selected = *active_tab == tab;

                // Selected tab gets the lighter panel background.
                if selected {
                    painter.rect_filled(tab_rect, 0.0, base_bg);

                    // Accent bar on the right edge.
                    let accent_rect = Rect::from_min_size(
                        Pos2::new(tab_rect.max.x - ACCENT_WIDTH, tab_rect.min.y),
                        Vec2::new(ACCENT_WIDTH, tab_height),
                    );
                    painter.rect_filled(accent_rect, 0.0, accent_color);
                }

                // Rotated label (90° CCW — reading bottom to top).
                let galley =
                    painter.layout_no_wrap(label.to_string(), font_id.clone(), text_color);
                let text_w = galley.size().x;
                let text_h = galley.size().y;

                // Icon goes below the rotated text. Reserve 16px for the icon
                // and 4px gap. Total content height = text_w (rotated) + 4 + 16.
                let icon_size = 16.0;
                let gap = 4.0;
                let total_h = text_w + gap + icon_size;

                let cx = tab_rect.center().x;
                let cy = tab_rect.center().y;

                // Rotated text: pivot so the text + icon block is centered.
                let text_top = cy - total_h / 2.0;
                let pos = Pos2::new(cx - text_h / 2.0, text_top + text_w);

                let text_shape = TextShape::new(pos, Arc::clone(&galley), text_color)
                    .with_angle(-FRAC_PI_2);
                painter.add(Shape::Text(text_shape));

                // Icon below the rotated text.
                if let Some(tex_id) = icons.and_then(|ic| ic.texture_id(icon)) {
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

                // Separator line between tabs.
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

                // Click detection.
                let response =
                    ui.interact(tab_rect, ui.id().with(("sidebar_tab", i)), Sense::click());
                if response.clicked() {
                    *active_tab = tab;
                }
            }

            // Right-edge separator.
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
    pub is_running: bool,
}

/// Render the sidebar content panel (file browser, plugins).
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
) -> SidebarAction {
    let mut action = SidebarAction::None;

    egui::SidePanel::left("sidebar_content")
        .resizable(true)
        .default_width(200.0)
        .min_width(100.0)
        .show(ctx, |ui| {
            // Fill the panel width so content never shrinks the panel.
            ui.set_min_width(ui.available_width());

            action = match active_tab {
                SidebarTab::Files => show_files_panel(ui, file_browser_state, icons),
                SidebarTab::Plugins => show_plugins_panel(ui, plugins, plugin_output, selected_plugin, icons),
            };
        });

    action
}

fn show_plugins_panel(
    ui: &mut egui::Ui,
    plugins: &[PluginDisplayInfo],
    output: &[String],
    selected: &mut Option<usize>,
    icons: Option<&IconCache>,
) -> SidebarAction {
    let mut action = SidebarAction::None;
    let dark_mode = ui.visuals().dark_mode;

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

    if plugins.is_empty() {
        ui.weak("No plugins found");
        ui.add_space(4.0);
        ui.weak("Place .lua files in:");
        ui.small("~/.config/conch/plugins/");
    } else {
        // Clamp selection if plugin list changed.
        if let Some(sel) = *selected {
            if sel >= plugins.len() {
                *selected = None;
            }
        }

        // Reserve space for buttons + output at the bottom.
        let btn_bar_height = 32.0;
        let output_height = 120.0;
        let reserved = btn_bar_height + output_height + 40.0;
        let list_height = (ui.available_height() - reserved).max(60.0);

        // Scrollable plugin list with selectable rows.
        egui::ScrollArea::vertical()
            .id_salt("plugin_list")
            .max_height(list_height)
            .show(ui, |ui| {
                for (i, plugin) in plugins.iter().enumerate() {
                    let is_selected = *selected == Some(i);

                    let resp = ui.push_id(i, |ui| {
                        let (rect, _) = ui.allocate_at_least(
                            Vec2::new(ui.available_width(), 0.0),
                            Sense::click(),
                        );

                        // Layout the text to measure its height.
                        let name_galley = ui.painter().layout_no_wrap(
                            plugin.name.clone(),
                            FontId::new(12.0, FontFamily::Proportional),
                            ui.visuals().text_color(),
                        );
                        let desc_galley = if !plugin.description.is_empty() {
                            Some(ui.painter().layout(
                                plugin.description.clone(),
                                FontId::new(10.0, FontFamily::Proportional),
                                ui.visuals().weak_text_color(),
                                rect.width() - 8.0,
                            ))
                        } else {
                            None
                        };

                        let padding = 4.0;
                        let total_h = padding
                            + name_galley.size().y
                            + desc_galley.as_ref().map_or(0.0, |g| g.size().y + 2.0)
                            + padding;

                        // Re-allocate with the actual height.
                        let row_rect = Rect::from_min_size(
                            rect.min,
                            Vec2::new(rect.width(), total_h),
                        );
                        ui.allocate_rect(row_rect, Sense::hover());
                        let resp = ui.interact(row_rect, ui.id().with(("plugin_row", i)), Sense::click());

                        // Draw highlight.
                        if is_selected {
                            ui.painter().rect_filled(
                                row_rect,
                                0.0,
                                ui.visuals().selection.bg_fill,
                            );
                        } else if resp.hovered() {
                            ui.painter().rect_filled(
                                row_rect,
                                0.0,
                                ui.visuals().widgets.hovered.bg_fill,
                            );
                        }

                        // Running indicator.
                        let name_text = if plugin.is_running {
                            format!("{} (running)", plugin.name)
                        } else {
                            plugin.name.clone()
                        };
                        let name_galley = ui.painter().layout_no_wrap(
                            name_text,
                            FontId::new(12.0, FontFamily::Proportional),
                            if is_selected {
                                Color32::WHITE
                            } else {
                                ui.visuals().text_color()
                            },
                        );

                        let mut y = row_rect.min.y + padding;
                        ui.painter().galley(
                            Pos2::new(row_rect.min.x + 4.0, y),
                            name_galley,
                            Color32::PLACEHOLDER,
                        );
                        y += ui.painter().layout_no_wrap(
                            String::new(),
                            FontId::new(12.0, FontFamily::Proportional),
                            Color32::TRANSPARENT,
                        ).size().y;

                        if let Some(desc_g) = desc_galley {
                            // Re-layout with correct color for selection.
                            let desc_galley2 = ui.painter().layout(
                                plugin.description.clone(),
                                FontId::new(10.0, FontFamily::Proportional),
                                if is_selected {
                                    Color32::from_gray(200)
                                } else {
                                    ui.visuals().weak_text_color()
                                },
                                row_rect.width() - 8.0,
                            );
                            y += 2.0;
                            ui.painter().galley(
                                Pos2::new(row_rect.min.x + 4.0, y),
                                desc_galley2,
                                Color32::PLACEHOLDER,
                            );
                            let _ = desc_g;
                        }

                        resp
                    }).inner;

                    // Click to select, double-click to run.
                    if resp.clicked() {
                        *selected = Some(i);
                    }
                    if resp.double_clicked() {
                        *selected = Some(i);
                        if plugin.is_running {
                            action = SidebarAction::StopPlugin(i);
                        } else {
                            action = SidebarAction::RunPlugin(i);
                        }
                    }

                    ui.separator();
                }
            });

        // Button bar: single Run / Stop button for the selected plugin.
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let btn_size = egui::Vec2::new(60.0, 24.0);
                if let Some(sel) = *selected {
                    if let Some(plugin) = plugins.get(sel) {
                        if plugin.is_running {
                            if ui.add_sized(btn_size, egui::Button::new("Stop")).clicked() {
                                action = SidebarAction::StopPlugin(sel);
                            }
                        } else if ui.add_sized(btn_size, egui::Button::new("Run")).clicked() {
                            action = SidebarAction::RunPlugin(sel);
                        }
                    }
                } else {
                    ui.add_enabled(false, egui::Button::new("Run").min_size(btn_size));
                }
            });
        });
    }

    // Output panel at bottom.
    ui.add_space(4.0);
    ui.strong("Output");
    ui.separator();
    egui::ScrollArea::vertical()
        .id_salt("plugin_output")
        .stick_to_bottom(true)
        .max_height(120.0)
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
}

fn show_files_panel(
    ui: &mut egui::Ui,
    state: &mut FileBrowserState,
    icons: Option<&IconCache>,
) -> SidebarAction {
    let mut action = SidebarAction::None;

    let available = ui.available_height();
    let remote_connected = state.remote_path.is_some();

    if remote_connected {
        // Both panes active — split evenly.
        let pane_height = (available - 8.0) / 2.0;

        ui.allocate_ui(Vec2::new(ui.available_width(), pane_height), |ui| {
            ui.push_id("remote_pane", |ui| {
                let a = show_file_pane(ui, state, PaneKind::Remote, icons);
                if !matches!(a, SidebarAction::None) {
                    action = a;
                }
            });
        });

        ui.separator();

        ui.allocate_ui(Vec2::new(ui.available_width(), pane_height), |ui| {
            ui.push_id("local_pane", |ui| {
                let a = show_file_pane(ui, state, PaneKind::Local, icons);
                if !matches!(a, SidebarAction::None) {
                    action = a;
                }
            });
        });
    } else {
        // No remote session — local pane fills all available space.
        ui.push_id("local_pane", |ui| {
            let a = show_file_pane(ui, state, PaneKind::Local, icons);
            if !matches!(a, SidebarAction::None) {
                action = a;
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
    let mut action = SidebarAction::None;

    use crate::ui::file_browser::FileListEntry;
    let (label, entries, current_path, path_edit): (&str, &[FileListEntry], Option<&PathBuf>, &mut String) = match kind {
        PaneKind::Remote => (
            "Remote",
            &state.remote_entries as &[_],
            state.remote_path.as_ref(),
            &mut state.remote_path_edit,
        ),
        PaneKind::Local => (
            "Local",
            &state.local_entries as &[_],
            Some(&state.local_path),
            &mut state.local_path_edit,
        ),
    };

    // Header
    ui.strong(label);

    // Check if remote is disconnected
    if kind == PaneKind::Remote && current_path.is_none() {
        ui.add_space(8.0);
        ui.weak("No remote session");
        return action;
    }

    // Toolbar + path edit — all on one row, vertically centered.
    // Layout: [back] [forward] [path textbox] [home] [refresh]
    // Use right-to-left outer layout so home/refresh consume exact space from the
    // right edge, then a nested left-to-right layout fills the rest — no guessing.
    let dark_mode = ui.visuals().dark_mode;
    let (back_stack, forward_stack) = match kind {
        PaneKind::Local => (&state.local_back_stack, &state.local_forward_stack),
        PaneKind::Remote => (&state.remote_back_stack, &state.remote_forward_stack),
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

            // Back button
            back_clicked = if let Some(img) = icons.and_then(|ic| ic.themed_image(Icon::GoPrevious, dark_mode)) {
                let btn = ui.add_enabled(has_back, egui::ImageButton::new(img).frame(false));
                btn.on_hover_text("Back").clicked()
            } else {
                let btn = ui.add_enabled(has_back, egui::Button::new("\u{2190}").small());
                btn.on_hover_text("Back").clicked()
            };

            // Forward button
            forward_clicked = if let Some(img) = icons.and_then(|ic| ic.themed_image(Icon::GoNext, dark_mode)) {
                let btn = ui.add_enabled(has_forward, egui::ImageButton::new(img).frame(false));
                btn.on_hover_text("Forward").clicked()
            } else {
                let btn = ui.add_enabled(has_forward, egui::Button::new("\u{2192}").small());
                btn.on_hover_text("Forward").clicked()
            };

            // Path edit field — fills all remaining space exactly.
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
        }
    }
    if forward_clicked {
        match kind {
            PaneKind::Local => action = SidebarAction::GoForwardLocal,
            PaneKind::Remote => action = SidebarAction::GoForwardRemote,
        }
    }
    if path_submitted {
        let target = PathBuf::from(path_edit.as_str());
        match kind {
            PaneKind::Local => action = SidebarAction::NavigateLocal(target),
            PaneKind::Remote => action = SidebarAction::NavigateRemote(target),
        }
    }
    if home_clicked {
        match kind {
            PaneKind::Local => action = SidebarAction::GoHomeLocal,
            PaneKind::Remote => action = SidebarAction::GoHomeRemote,
        }
    }
    if refresh_clicked {
        match kind {
            PaneKind::Local => action = SidebarAction::RefreshLocal,
            PaneKind::Remote => action = SidebarAction::RefreshRemote,
        }
    }

    // File table with aligned, resizable columns
    let status_bar_height = 18.0;
    let table_height = (ui.available_height() - status_bar_height).max(0.0);
    TableBuilder::new(ui)
        .striped(true)
        .resizable(true)
        .max_scroll_height(table_height)
        .column(Column::initial(100.0).at_least(60.0).resizable(true))  // Name
        .column(Column::auto().at_least(40.0).resizable(true))         // Size
        .column(Column::remainder().at_least(70.0))                    // Modified — fills remaining
        .header(16.0, |mut header| {
            header.col(|ui| { ui.label(egui::RichText::new("Name").strong().size(10.0)); });
            header.col(|ui| { ui.label(egui::RichText::new("Size").strong().size(10.0)); });
            header.col(|ui| { ui.label(egui::RichText::new("Modified").strong().size(10.0)); });
        })
        .body(|body| {
            body.rows(16.0, entries.len(), |mut row| {
                let idx = row.index();
                let entry = &entries[idx];

                // Name column: icon + clickable label
                row.col(|ui| {
                    let resp = ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 3.0;
                        if entry.is_dir {
                            if let Some(img) = icons.and_then(|ic| ic.themed_image(Icon::SidebarFolder, dark_mode)) {
                                ui.add(img.fit_to_exact_size(Vec2::new(14.0, 14.0)));
                            }
                        } else if let Some(img) = icons.and_then(|ic| ic.themed_image(Icon::File, dark_mode)) {
                            ui.add(img.fit_to_exact_size(Vec2::new(14.0, 14.0)));
                        }
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(&entry.name).size(12.0),
                            )
                            .truncate()
                            .sense(Sense::click()),
                        )
                    }).inner;

                    if resp.clicked() {
                        if entry.is_dir {
                            match kind {
                                PaneKind::Local => {
                                    action = SidebarAction::NavigateLocal(entry.path.clone());
                                }
                                PaneKind::Remote => {
                                    action = SidebarAction::NavigateRemote(entry.path.clone());
                                }
                            }
                        } else {
                            action = SidebarAction::SelectFile(entry.path.clone());
                        }
                    }
                });

                // Size column
                row.col(|ui| {
                    let size_text = if entry.is_dir {
                        "<DIR>".to_string()
                    } else {
                        display_size(entry.size)
                    };
                    ui.label(egui::RichText::new(size_text).size(11.0).weak());
                });

                // Modified column
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

/// Darken a `Color32` by subtracting `amount` from each RGB channel.
pub fn darken_color(color: Color32, amount: u8) -> Color32 {
    Color32::from_rgba_premultiplied(
        color.r().saturating_sub(amount),
        color.g().saturating_sub(amount),
        color.b().saturating_sub(amount),
        color.a(),
    )
}

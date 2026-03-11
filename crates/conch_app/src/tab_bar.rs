//! Custom tab bar widget with animated open/close transitions.

use std::collections::HashMap;
use std::time::Instant;

use egui::{Color32, Rect, Sense, Vec2};
use uuid::Uuid;

use crate::state::AppState;
use crate::terminal::color::ResolvedColors;

/// Duration of tab open/close animations in seconds.
const ANIM_SECS: f32 = 0.15;

/// Action returned by the tab bar for the app to process.
pub enum TabBarAction {
    SwitchTo(Uuid),
    Close(Uuid),
}

/// A tab that is animating closed (removed from sessions but still visible).
struct ClosingTab {
    id: Uuid,
    title: String,
    index: usize,
    started: Instant,
}

/// Persistent state for tab bar animations.
pub struct TabBarState {
    closing: Vec<ClosingTab>,
    /// Tracks when each tab was first seen (for open animation).
    open_times: HashMap<Uuid, Instant>,
}

impl Default for TabBarState {
    fn default() -> Self {
        Self {
            closing: Vec::new(),
            open_times: HashMap::new(),
        }
    }
}

impl TabBarState {
    /// Mark a tab as closing with a snapshot of its title and position.
    pub fn begin_close(&mut self, id: Uuid, title: String, index: usize) {
        if !self.closing.iter().any(|c| c.id == id) {
            self.closing.push(ClosingTab { id, title, index, started: Instant::now() });
        }
        self.open_times.remove(&id);
    }
}

/// Easing function: cubic ease-out for smooth deceleration.
fn ease_out(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

/// Compute animation weight (0.0 to 1.0) for an opening tab.
fn open_weight(started: Instant) -> f32 {
    let elapsed = started.elapsed().as_secs_f32();
    ease_out((elapsed / ANIM_SECS).min(1.0))
}

/// Compute animation weight (1.0 to 0.0) for a closing tab.
fn close_weight(started: Instant) -> f32 {
    let elapsed = started.elapsed().as_secs_f32();
    let t = (elapsed / ANIM_SECS).min(1.0);
    1.0 - ease_out(t)
}

/// A tab to render — either a live session tab or a closing ghost.
struct TabEntry {
    id: Uuid,
    title: String,
    is_active: bool,
    is_closing: bool,
    weight: f32,
    /// Position among live tabs (for numbering). None for closing tabs.
    live_index: Option<usize>,
}

/// Show the tab bar. Only renders when there are 2+ visible tabs.
/// Returns a list of actions for the caller to apply.
pub fn show(ctx: &egui::Context, state: &AppState, tab_state: &mut TabBarState) -> Vec<TabBarAction> {
    // Register open times for newly-appearing tabs.
    for &id in &state.tab_order {
        tab_state.open_times.entry(id).or_insert_with(Instant::now);
    }

    // Build the merged tab list: live tabs with closing ghosts interleaved.
    let mut entries: Vec<TabEntry> = Vec::new();

    for (i, &id) in state.tab_order.iter().enumerate() {
        let title = state.sessions.get(&id)
            .map(|s| s.display_title().to_string())
            .unwrap_or_default();
        let weight = tab_state.open_times.get(&id)
            .map(|&t| open_weight(t))
            .unwrap_or(1.0);
        entries.push(TabEntry {
            id,
            title,
            is_active: state.active_tab == Some(id),
            is_closing: false,
            weight,
            live_index: Some(i),
        });
    }

    // Insert closing ghosts at their original positions (clamped).
    for closing in &tab_state.closing {
        let insert_at = closing.index.min(entries.len());
        entries.insert(insert_at, TabEntry {
            id: closing.id,
            title: closing.title.clone(),
            is_active: false,
            is_closing: true,
            weight: close_weight(closing.started),
            live_index: None,
        });
    }

    // Hide the bar when there's only 1 live tab and no active animations.
    if state.tab_order.len() <= 1 && tab_state.closing.is_empty() {
        return Vec::new();
    }

    // Request repaint while any animation is in progress.
    let any_animating = entries.iter().any(|e| {
        if e.is_closing { e.weight > 0.01 } else { e.weight < 0.99 }
    });
    if any_animating {
        ctx.request_repaint();
    }

    let colors = TabBarColors::from_scheme(&state.colors);
    let mut actions = Vec::new();

    let tab_height = 28.0;
    egui::TopBottomPanel::top("tab_bar_panel")
        .exact_height(tab_height)
        .frame(egui::Frame::NONE.fill(colors.bar_bg))
        .show(ctx, |ui| {
            let available_width = ui.available_width();

            let total_weight: f32 = entries.iter().map(|e| e.weight).sum();
            if total_weight < 0.001 {
                return;
            }

            let (_, bar_rect) = ui.allocate_space(Vec2::new(available_width, tab_height));

            let mut x_offset = 0.0;
            let mut live_divider_positions: Vec<f32> = Vec::new();

            for entry in &entries {
                let tab_width = (entry.weight / total_weight) * available_width;

                if tab_width < 0.5 {
                    continue;
                }

                let tab_rect = Rect::from_min_size(
                    bar_rect.min + Vec2::new(x_offset, 0.0),
                    Vec2::new(tab_width, tab_height),
                );
                x_offset += tab_width;

                // Fade text opacity for closing/opening tabs.
                let alpha = (entry.weight.clamp(0.0, 1.0) * 255.0) as u8;

                let resp = ui.interact(tab_rect, ui.id().with(("tab", entry.id)), Sense::click());

                // Background.
                let fill = if entry.is_active {
                    colors.active_bg
                } else if resp.hovered() && !entry.is_closing {
                    colors.hover_bg
                } else {
                    colors.bar_bg
                };
                ui.painter().rect_filled(tab_rect, 0.0, fill);

                // Track divider positions between live tabs.
                if !entry.is_closing {
                    live_divider_positions.push(tab_rect.right());
                }

                // Bottom highlight for active tab.
                if entry.is_active {
                    let highlight_rect = Rect::from_min_max(
                        egui::pos2(tab_rect.left(), tab_rect.bottom() - 2.0),
                        tab_rect.max,
                    );
                    ui.painter().rect_filled(highlight_rect, 0.0, colors.text);
                }

                // Tab number + title.
                let label = match entry.live_index {
                    Some(idx) if idx < 9 => format!("{}  {}", idx + 1, entry.title),
                    _ => entry.title.clone(),
                };
                let base_color = if entry.is_active { colors.text } else { colors.text_dim };
                let color = Color32::from_rgba_unmultiplied(
                    base_color.r(), base_color.g(), base_color.b(), alpha,
                );
                let galley = ui.painter().layout_no_wrap(
                    label,
                    egui::FontId::proportional(11.0),
                    color,
                );

                // Clip text to tab bounds.
                let text_x = (tab_rect.center().x - galley.size().x / 2.0)
                    .max(tab_rect.left() + 4.0);
                let text_pos = egui::pos2(
                    text_x,
                    tab_rect.center().y - galley.size().y / 2.0,
                );
                ui.painter().with_clip_rect(tab_rect).galley(text_pos, galley, color);

                if !entry.is_closing {
                    if resp.clicked() {
                        actions.push(TabBarAction::SwitchTo(entry.id));
                    }
                    if resp.middle_clicked() {
                        actions.push(TabBarAction::Close(entry.id));
                    }
                }
            }

            // Draw dividers between live tabs (skip the last one).
            if let Some(positions) = live_divider_positions.get(..live_divider_positions.len().saturating_sub(1)) {
                for &x in positions {
                    ui.painter().line_segment(
                        [egui::pos2(x, bar_rect.top() + 6.0), egui::pos2(x, bar_rect.bottom() - 6.0)],
                        egui::Stroke::new(1.0, colors.divider),
                    );
                }
            }
        });

    // Prune closing tabs whose animation has finished.
    tab_state.closing.retain(|c| close_weight(c.started) > 0.01);

    actions
}

/// Pre-computed colors for the tab bar, derived from the terminal color scheme.
struct TabBarColors {
    bar_bg: Color32,
    active_bg: Color32,
    hover_bg: Color32,
    text: Color32,
    text_dim: Color32,
    divider: Color32,
}

impl TabBarColors {
    fn from_scheme(colors: &ResolvedColors) -> Self {
        let bg_r = (colors.background[0] * 255.0) as u8;
        let bg_g = (colors.background[1] * 255.0) as u8;
        let bg_b = (colors.background[2] * 255.0) as u8;

        Self {
            bar_bg: Color32::from_rgb(bg_r.saturating_add(15), bg_g.saturating_add(15), bg_b.saturating_add(15)),
            active_bg: Color32::from_rgb(bg_r.saturating_add(35), bg_g.saturating_add(35), bg_b.saturating_add(35)),
            hover_bg: Color32::from_rgb(bg_r.saturating_add(25), bg_g.saturating_add(25), bg_b.saturating_add(25)),
            text: Color32::from_rgb(
                (colors.foreground[0] * 255.0) as u8,
                (colors.foreground[1] * 255.0) as u8,
                (colors.foreground[2] * 255.0) as u8,
            ),
            text_dim: Color32::from_rgb(
                (colors.foreground[0] * 180.0) as u8,
                (colors.foreground[1] * 180.0) as u8,
                (colors.foreground[2] * 180.0) as u8,
            ),
            divider: Color32::from_rgb(
                (colors.foreground[0] * 50.0) as u8,
                (colors.foreground[1] * 50.0) as u8,
                (colors.foreground[2] * 50.0) as u8,
            ),
        }
    }
}

//! Toast notification system.
//!
//! Notifications slide in from the right edge, stay visible for a configurable
//! duration, and slide back out.  They can optionally contain buttons that
//! send a response back to the caller (plugin or internal app code).

use std::time::{Duration, Instant, SystemTime};

use conch_plugin::{NotificationLevel, PluginResponse};
use tokio::sync::mpsc;

/// A record of a past notification for the history dialog.
pub(crate) struct HistoryEntry {
    pub(crate) timestamp: SystemTime,
    pub(crate) source: String,
    pub(crate) body: String,
    pub(crate) level: NotificationLevel,
}

/// Animation durations.
const ANIM_IN_MS: u64 = 300;
const ANIM_OUT_MS: u64 = 250;
/// Default time a notification stays visible (seconds).
const DEFAULT_DURATION_SECS: f32 = 5.0;
/// Max width of a notification card.
const CARD_WIDTH: f32 = 340.0;
/// Margin from the top-right corner.
const MARGIN_RIGHT: f32 = 12.0;
const MARGIN_TOP: f32 = 12.0;
/// Vertical gap between stacked notifications.
const GAP: f32 = 8.0;

#[derive(Debug)]
enum Phase {
    AnimatingIn { start: Instant },
    Visible { until: Option<Instant> },
    AnimatingOut { start: Instant },
    Done,
}

pub(crate) struct Notification {
    id: u64,
    pub(crate) title: Option<String>,
    pub(crate) body: String,
    pub(crate) level: NotificationLevel,
    pub(crate) buttons: Vec<String>,
    phase: Phase,
    /// Response channel for blocking notifications (with buttons).
    resp_tx: Option<mpsc::UnboundedSender<PluginResponse>>,
    /// For panel plugin notifications — clicking navigates to this panel.
    pub(crate) plugin_idx: Option<usize>,
    /// Duration to stay visible. `None` = persistent until dismissed.
    duration: Option<Duration>,
}

static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

fn next_id() -> u64 {
    NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

impl Notification {
    /// Create a fire-and-forget notification (no buttons, auto-dismiss).
    pub(crate) fn simple(
        body: String,
        title: Option<String>,
        level: NotificationLevel,
        duration_secs: Option<f32>,
        plugin_idx: Option<usize>,
    ) -> Self {
        let duration = match duration_secs {
            Some(d) if d <= 0.0 => None, // persistent
            Some(d) => Some(Duration::from_secs_f32(d)),
            None => Some(Duration::from_secs_f32(DEFAULT_DURATION_SECS)),
        };
        Self {
            id: next_id(),
            title,
            body,
            level,
            buttons: Vec::new(),
            phase: Phase::AnimatingIn { start: Instant::now() },
            resp_tx: None,
            plugin_idx,
            duration,
        }
    }

    /// Create a blocking notification with buttons.
    pub(crate) fn with_buttons(
        body: String,
        title: Option<String>,
        level: NotificationLevel,
        buttons: Vec<String>,
        resp_tx: mpsc::UnboundedSender<PluginResponse>,
        plugin_idx: Option<usize>,
    ) -> Self {
        Self {
            id: next_id(),
            title,
            body,
            level,
            buttons,
            phase: Phase::AnimatingIn { start: Instant::now() },
            resp_tx: Some(resp_tx),
            plugin_idx,
            duration: None, // persistent — waits for button click
        }
    }

    fn is_done(&self) -> bool {
        matches!(self.phase, Phase::Done)
    }

    fn dismiss(&mut self) {
        if !matches!(self.phase, Phase::AnimatingOut { .. } | Phase::Done) {
            self.phase = Phase::AnimatingOut { start: Instant::now() };
        }
    }

    fn click_button(&mut self, label: &str) {
        if let Some(tx) = self.resp_tx.take() {
            let _ = tx.send(PluginResponse::Output(label.to_string()));
        }
        self.dismiss();
    }

    /// Progress the phase state machine. Returns the current x-offset for
    /// slide animation (0.0 = fully visible, 1.0 = fully off-screen right).
    fn tick(&mut self) -> f32 {
        let now = Instant::now();
        match self.phase {
            Phase::AnimatingIn { start } => {
                let elapsed = now.duration_since(start).as_millis() as f32;
                let total = ANIM_IN_MS as f32;
                let t = (elapsed / total).min(1.0);
                if t >= 1.0 {
                    // Transition to Visible
                    let until = self.duration.map(|d| now + d);
                    self.phase = Phase::Visible { until };
                    0.0
                } else {
                    // ease-out: 1 - (1-t)^3
                    let ease = 1.0 - (1.0 - t).powi(3);
                    1.0 - ease
                }
            }
            Phase::Visible { until } => {
                if let Some(deadline) = until {
                    if now >= deadline {
                        self.phase = Phase::AnimatingOut { start: now };
                    }
                }
                0.0
            }
            Phase::AnimatingOut { start } => {
                let elapsed = now.duration_since(start).as_millis() as f32;
                let total = ANIM_OUT_MS as f32;
                let t = (elapsed / total).min(1.0);
                if t >= 1.0 {
                    self.phase = Phase::Done;
                    1.0
                } else {
                    // ease-in: t^2
                    t * t
                }
            }
            Phase::Done => 1.0,
        }
    }
}

/// Manages the notification stack and renders them as an overlay.
pub(crate) struct NotificationManager {
    notifications: Vec<Notification>,
    history: Vec<HistoryEntry>,
}

impl NotificationManager {
    pub(crate) fn new() -> Self {
        Self {
            notifications: Vec::new(),
            history: Vec::new(),
        }
    }

    pub(crate) fn push(&mut self, notification: Notification) {
        self.history.push(HistoryEntry {
            timestamp: SystemTime::now(),
            source: notification.title.clone().unwrap_or_else(|| "Notification".into()),
            body: notification.body.clone(),
            level: notification.level,
        });
        // Cap history at 500 entries.
        if self.history.len() > 500 {
            self.history.remove(0);
        }
        self.notifications.push(notification);
    }

    pub(crate) fn history(&self) -> &[HistoryEntry] {
        &self.history
    }

    #[allow(dead_code)]
    pub(crate) fn is_empty(&self) -> bool {
        self.notifications.is_empty()
    }

    /// Render all notifications as an overlay. Returns an optional plugin index
    /// if the user clicked a non-button notification (for panel navigation).
    pub(crate) fn show(&mut self, ctx: &egui::Context) -> Option<usize> {
        // Remove completed notifications.
        self.notifications.retain(|n| !n.is_done());

        if self.notifications.is_empty() {
            return None;
        }

        // Request repaint while animating.
        ctx.request_repaint();

        let screen = ctx.screen_rect();
        let mut y_offset = MARGIN_TOP;
        let mut navigate_to_plugin: Option<usize> = None;

        // We need indices for mutation, so iterate by index.
        let count = self.notifications.len();
        for i in 0..count {
            let slide_t = self.notifications[i].tick();
            let x_slide = slide_t * (CARD_WIDTH + MARGIN_RIGHT + 20.0);

            let card_x = screen.max.x - CARD_WIDTH - MARGIN_RIGHT + x_slide;
            let card_pos = egui::pos2(card_x, y_offset);

            let area_id = egui::Id::new("notification").with(self.notifications[i].id);

            let resp = egui::Area::new(area_id)
                .fixed_pos(card_pos)
                .order(egui::Order::Foreground)
                .interactable(true)
                .show(ctx, |ui| {
                    self.render_card(ui, i)
                });

            // Track card height for stacking.
            let card_height = resp.response.rect.height();
            y_offset += card_height + GAP;

            // Check if user clicked the card body (non-button notification → navigate).
            if resp.response.clicked() {
                let notif = &mut self.notifications[i];
                if notif.buttons.is_empty() {
                    if let Some(idx) = notif.plugin_idx {
                        navigate_to_plugin = Some(idx);
                    }
                    notif.dismiss();
                }
            }
        }

        // Clean up done notifications and send cancel for any unsent responses.
        self.notifications.retain_mut(|n| {
            if n.is_done() {
                // If there's still a response channel (user never clicked), send nil.
                if let Some(tx) = n.resp_tx.take() {
                    let _ = tx.send(PluginResponse::Output(String::new()));
                }
                false
            } else {
                true
            }
        });

        navigate_to_plugin
    }

    fn render_card(&mut self, ui: &mut egui::Ui, idx: usize) -> CardAction {
        let notif = &self.notifications[idx];
        let dark_mode = ui.visuals().dark_mode;

        // Card colors based on level.
        let (accent, bg) = level_colors(notif.level, dark_mode);

        let frame = egui::Frame::new()
            .fill(bg)
            .corner_radius(egui::CornerRadius::same(8))
            .stroke(egui::Stroke::new(1.0, accent))
            .shadow(egui::epaint::Shadow {
                offset: [0, 2],
                blur: 8,
                spread: 0,
                color: egui::Color32::from_black_alpha(40),
            })
            .inner_margin(egui::Margin::same(12));

        let mut action = CardAction::None;

        frame.show(ui, |ui| {
            ui.set_width(CARD_WIDTH - 24.0); // account for inner margin

            // Top row: accent bar + title + dismiss X
            ui.horizontal(|ui| {
                // Small accent dot
                let (r, painter) = ui.allocate_painter(
                    egui::vec2(6.0, 6.0),
                    egui::Sense::hover(),
                );
                painter.circle_filled(r.rect.center(), 3.0, accent);

                if let Some(title) = &notif.title {
                    ui.strong(title);
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button("✕").clicked() {
                        action = CardAction::Dismiss;
                    }
                });
            });

            // Body text
            ui.add_space(4.0);
            ui.label(&notif.body);

            // Buttons (if any)
            if !notif.buttons.is_empty() {
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    for label in &notif.buttons {
                        if ui.button(label).clicked() {
                            action = CardAction::ClickButton(label.clone());
                        }
                    }
                });
            }
        });

        // Apply action
        match &action {
            CardAction::Dismiss => self.notifications[idx].dismiss(),
            CardAction::ClickButton(label) => {
                let label = label.clone();
                self.notifications[idx].click_button(&label);
            }
            CardAction::None => {}
        }

        action
    }
}

enum CardAction {
    None,
    Dismiss,
    ClickButton(String),
}

pub(crate) fn level_colors(level: NotificationLevel, dark_mode: bool) -> (egui::Color32, egui::Color32) {
    if dark_mode {
        match level {
            NotificationLevel::Info => (
                egui::Color32::from_rgb(100, 160, 240),
                egui::Color32::from_rgba_premultiplied(35, 40, 50, 240),
            ),
            NotificationLevel::Success => (
                egui::Color32::from_rgb(80, 200, 120),
                egui::Color32::from_rgba_premultiplied(30, 45, 35, 240),
            ),
            NotificationLevel::Warning => (
                egui::Color32::from_rgb(240, 180, 60),
                egui::Color32::from_rgba_premultiplied(50, 45, 30, 240),
            ),
            NotificationLevel::Error => (
                egui::Color32::from_rgb(240, 80, 80),
                egui::Color32::from_rgba_premultiplied(50, 30, 30, 240),
            ),
        }
    } else {
        match level {
            NotificationLevel::Info => (
                egui::Color32::from_rgb(40, 100, 200),
                egui::Color32::from_rgba_premultiplied(240, 245, 255, 245),
            ),
            NotificationLevel::Success => (
                egui::Color32::from_rgb(30, 150, 70),
                egui::Color32::from_rgba_premultiplied(235, 250, 240, 245),
            ),
            NotificationLevel::Warning => (
                egui::Color32::from_rgb(200, 150, 30),
                egui::Color32::from_rgba_premultiplied(255, 250, 235, 245),
            ),
            NotificationLevel::Error => (
                egui::Color32::from_rgb(200, 50, 50),
                egui::Color32::from_rgba_premultiplied(255, 240, 240, 245),
            ),
        }
    }
}

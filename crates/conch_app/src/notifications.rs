//! Toast notification system.
//!
//! Notifications appear instantly in the top-right corner, stay visible for a
//! configurable duration, then disappear. They render as an overlay on top of
//! all other UI elements.
//!
//! Plugins push notifications via the `HostApi::notify` FFI function, which
//! routes through the bridge into this module. Internal app code (e.g. config
//! reload) can also push notifications directly.

use std::sync::mpsc;
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const DEFAULT_DURATION_SECS: f32 = 5.0;
const CARD_WIDTH: f32 = 300.0;
const MARGIN_RIGHT: f32 = 12.0;
const MARGIN_TOP: f32 = 12.0;
const GAP: f32 = 6.0;

// ---------------------------------------------------------------------------
// Notification level
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotificationLevel {
    Info,
    Success,
    Warning,
    Error,
}

impl NotificationLevel {
    pub fn from_str(s: &str) -> Self {
        match s {
            "success" => Self::Success,
            "warn" | "warning" => Self::Warning,
            "error" => Self::Error,
            _ => Self::Info,
        }
    }
}

// ---------------------------------------------------------------------------
// Phase state machine
// ---------------------------------------------------------------------------

enum Phase {
    Visible { until: Option<Instant> },
    Done,
}

// ---------------------------------------------------------------------------
// Notification
// ---------------------------------------------------------------------------

pub(crate) struct Notification {
    id: u64,
    title: Option<String>,
    body: String,
    level: NotificationLevel,
    phase: Phase,
}

static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

impl Notification {
    fn next_id() -> u64 {
        NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    }

    pub(crate) fn new(
        title: Option<String>,
        body: String,
        level: NotificationLevel,
        duration_ms: Option<u64>,
    ) -> Self {
        let until = match duration_ms {
            Some(0) => None, // persistent
            Some(ms) => Some(Instant::now() + Duration::from_millis(ms)),
            None => Some(Instant::now() + Duration::from_secs_f32(DEFAULT_DURATION_SECS)),
        };
        Self {
            id: Self::next_id(),
            title,
            body,
            level,
            phase: Phase::Visible { until },
        }
    }

    fn is_done(&self) -> bool {
        matches!(self.phase, Phase::Done)
    }

    fn dismiss(&mut self) {
        self.phase = Phase::Done;
    }

    /// Check if the notification has expired and mark it done.
    fn tick(&mut self) {
        if let Phase::Visible { until: Some(deadline) } = self.phase {
            if Instant::now() >= deadline {
                self.phase = Phase::Done;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Global notification channel (bridge → app)
// ---------------------------------------------------------------------------

static NOTIFICATION_TX: std::sync::OnceLock<mpsc::Sender<Notification>> =
    std::sync::OnceLock::new();

/// Initialize the global notification channel. Called once during app startup.
pub(crate) fn init_channel() -> mpsc::Receiver<Notification> {
    let (tx, rx) = mpsc::channel();
    let _ = NOTIFICATION_TX.set(tx);
    rx
}

/// Push a notification from any thread (bridge, app code, etc.).
pub fn push(notification: Notification) {
    if let Some(tx) = NOTIFICATION_TX.get() {
        let _ = tx.send(notification);
    }
}

// ---------------------------------------------------------------------------
// Notification manager (owned by the app, renders overlay)
// ---------------------------------------------------------------------------

pub(crate) struct NotificationManager {
    rx: mpsc::Receiver<Notification>,
    notifications: Vec<Notification>,
}

impl NotificationManager {
    pub(crate) fn new(rx: mpsc::Receiver<Notification>) -> Self {
        Self {
            rx,
            notifications: Vec::new(),
        }
    }

    /// Whether there are any active (visible or pending) notifications.
    pub(crate) fn has_active(&self) -> bool {
        !self.notifications.is_empty()
    }

    /// Render all notifications as an overlay. Call at the end of the update
    /// loop so toasts render on top of everything.
    pub(crate) fn show(&mut self, ctx: &egui::Context) {
        // Drain incoming notifications.
        while let Ok(notif) = self.rx.try_recv() {
            self.notifications.push(notif);
        }

        // Tick and remove completed.
        for n in &mut self.notifications {
            n.tick();
        }
        self.notifications.retain(|n| !n.is_done());

        if self.notifications.is_empty() {
            return;
        }

        let screen = ctx.screen_rect();
        // Use available_rect().min.y to start below any top panels (titlebar
        // spacer, menu bar, tab bar) so notifications don't hide behind them.
        let top_y = ctx.available_rect().min.y;
        let card_x = screen.max.x - CARD_WIDTH - MARGIN_RIGHT;
        let mut y_offset = top_y + MARGIN_TOP;

        for notif in &mut self.notifications {
            let area_id = egui::Id::new("toast").with(notif.id);

            let resp = egui::Area::new(area_id)
                .fixed_pos(egui::pos2(card_x, y_offset))
                .order(egui::Order::Foreground)
                .interactable(true)
                .show(ctx, |ui| {
                    render_card(ui, notif);
                });

            y_offset += resp.response.rect.height() + GAP;
        }
    }
}

// ---------------------------------------------------------------------------
// Card rendering
// ---------------------------------------------------------------------------

fn render_card(ui: &mut egui::Ui, notif: &mut Notification) {
    let v = ui.visuals();
    let dark = v.dark_mode;

    // Use the app's existing surface/panel colors for the card background.
    let bg = v.window_fill;
    let border_color = v.window_stroke.color;
    let accent = level_accent(notif.level, dark);

    let frame = egui::Frame::new()
        .fill(bg)
        .corner_radius(egui::CornerRadius::ZERO)
        .stroke(egui::Stroke::new(1.0, border_color))
        .shadow(egui::epaint::Shadow {
            offset: [1, 1],
            blur: 0,
            spread: 0,
            color: egui::Color32::from_black_alpha(if dark { 80 } else { 30 }),
        })
        .inner_margin(egui::Margin::symmetric(10, 8));

    frame.show(ui, |ui| {
        ui.set_width(CARD_WIDTH - 20.0);

        ui.horizontal(|ui| {
            // Accent bar — thin vertical line on the left.
            let (r, painter) = ui.allocate_painter(egui::vec2(3.0, 16.0), egui::Sense::hover());
            painter.rect_filled(
                r.rect,
                egui::CornerRadius::ZERO,
                accent,
            );

            ui.add_space(4.0);

            ui.vertical(|ui| {
                if let Some(title) = &notif.title {
                    ui.strong(title);
                }
                ui.label(egui::RichText::new(&notif.body).small().color(
                    if dark {
                        egui::Color32::from_gray(170)
                    } else {
                        egui::Color32::from_gray(80)
                    },
                ));
            });

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                let dismiss = ui.add(
                    egui::Button::new(
                        egui::RichText::new("\u{2715}").small().color(
                            if dark {
                                egui::Color32::from_gray(120)
                            } else {
                                egui::Color32::from_gray(140)
                            },
                        ),
                    )
                    .frame(false),
                );
                if dismiss.clicked() {
                    notif.dismiss();
                }
            });
        });
    });
}

fn level_accent(level: NotificationLevel, dark: bool) -> egui::Color32 {
    match level {
        NotificationLevel::Info => {
            if dark {
                egui::Color32::from_rgb(100, 160, 240)
            } else {
                egui::Color32::from_rgb(50, 120, 220)
            }
        }
        NotificationLevel::Success => {
            if dark {
                egui::Color32::from_rgb(80, 200, 120)
            } else {
                egui::Color32::from_rgb(40, 160, 80)
            }
        }
        NotificationLevel::Warning => {
            if dark {
                egui::Color32::from_rgb(240, 180, 60)
            } else {
                egui::Color32::from_rgb(210, 150, 30)
            }
        }
        NotificationLevel::Error => {
            if dark {
                egui::Color32::from_rgb(240, 80, 80)
            } else {
                egui::Color32::from_rgb(210, 50, 50)
            }
        }
    }
}

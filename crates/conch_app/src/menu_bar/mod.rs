//! Cross-platform menu bar.
//!
//! Resolves the menu bar rendering strategy from the user config and
//! platform capabilities — no hardcoded OS checks leak into `app.rs`.
//!
//! On macOS with `native_menu_bar = true`: native NSMenu global menu bar.
//! Otherwise: egui in-window menu bar (themed by the UI engine).
//!
//! Designed for extensibility: plugins will register additional items
//! via `MenuBarState` in a future phase.

#[cfg(target_os = "macos")]
mod native_macos;

pub(crate) mod egui_menu;

use egui::ViewportCommand;

use crate::platform::PlatformCapabilities;

/// Actions that menu items can trigger.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuAction {
    // File
    NewTab,
    NewWindow,
    CloseTab,
    Quit,
    // Edit
    Copy,
    Paste,
    SelectAll,
    // View
    ZenMode,
    ZoomIn,
    ZoomOut,
    ZoomReset,
    // Tools
    PluginManager,
    /// Action from a plugin-registered menu item.
    PluginAction {
        plugin_name: String,
        action: String,
    },
}

/// Resolved rendering strategy for the menu bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MenuBarMode {
    /// Native OS global menu bar (macOS NSMenu).
    Native,
    /// egui in-window menu bar (all platforms).
    InWindow,
}

/// Persistent menu bar state. Plugins will register items here in Phase 2.
pub struct MenuBarState {
    mode: MenuBarMode,
    /// Whether the native menu has been set up (done once).
    native_setup_done: bool,
}

impl MenuBarState {
    /// Create menu bar state by resolving the rendering mode from config
    /// and platform capabilities. The menu bar owns this decision — callers
    /// don't need to know how it works.
    pub fn new(config_native: bool, platform: &PlatformCapabilities) -> Self {
        let mode = if config_native && platform.native_global_menu {
            MenuBarMode::Native
        } else {
            MenuBarMode::InWindow
        };

        Self {
            mode,
            native_setup_done: false,
        }
    }

    /// Whether this menu bar renders inside the window (vs native OS menu).
    pub fn is_in_window(&self) -> bool {
        self.mode == MenuBarMode::InWindow
    }

    /// Re-resolve the mode after a config reload.
    pub fn update_mode(&mut self, config_native: bool, platform: &PlatformCapabilities) {
        let new_mode = if config_native && platform.native_global_menu {
            MenuBarMode::Native
        } else {
            MenuBarMode::InWindow
        };

        if new_mode != self.mode {
            self.mode = new_mode;
            // If switching away from native, the NSMenu stays installed
            // (no way to remove it), but we stop draining its actions
            // and render the egui bar instead.
        }
    }
}

/// Ensure the native menu is set up if needed (called once).
fn ensure_setup(state: &mut MenuBarState) {
    if state.native_setup_done {
        return;
    }

    #[cfg(target_os = "macos")]
    {
        if state.mode == MenuBarMode::Native {
            native_macos::setup_menu_bar();
            state.native_setup_done = true;
            return;
        }
    }

    state.native_setup_done = true;
}

/// Render the menu bar and collect any triggered actions.
///
/// Automatically uses the resolved mode — native or in-window.
pub fn show(
    ctx: &egui::Context,
    state: &mut MenuBarState,
) -> Option<MenuAction> {
    ensure_setup(state);

    match state.mode {
        MenuBarMode::Native => {
            #[cfg(target_os = "macos")]
            {
                return native_macos::drain_actions().into_iter().next();
            }
            #[cfg(not(target_os = "macos"))]
            {
                // Can't happen — mode resolution prevents this.
                unreachable!()
            }
        }
        MenuBarMode::InWindow => egui_menu::show(ctx),
    }
}

/// Handle a menu action, mutating app state as needed.
pub fn handle_action(
    action: MenuAction,
    ctx: &egui::Context,
    app: &mut super::app::ConchApp,
) {
    match action {
        MenuAction::NewTab => app.open_local_tab(),
        MenuAction::NewWindow => app.spawn_extra_window(),
        MenuAction::CloseTab => {
            if let Some(id) = app.state.active_tab {
                app.remove_session(id);
            }
        }
        MenuAction::Quit => {
            app.quit_requested = true;
        }
        MenuAction::Copy => {
            if let Some((start, end)) = app.selection.normalized() {
                if let Some(session) = app.state.active_session() {
                    let text = crate::terminal::widget::get_selected_text(session.term(), start, end);
                    if !text.is_empty() {
                        ctx.copy_text(text);
                    }
                }
            }
        }
        MenuAction::Paste => {
            ctx.send_viewport_cmd(ViewportCommand::RequestPaste);
        }
        MenuAction::SelectAll => {
            // TODO: implement select-all for terminal content
        }
        MenuAction::ZenMode => {
            // TODO: toggle zen mode (hide chrome)
        }
        MenuAction::ZoomIn => {
            let current = ctx.pixels_per_point();
            ctx.set_pixels_per_point(current + 0.5);
        }
        MenuAction::ZoomOut => {
            let current = ctx.pixels_per_point();
            ctx.set_pixels_per_point((current - 0.5).max(0.5));
        }
        MenuAction::ZoomReset => {
            ctx.set_pixels_per_point(1.0);
        }
        MenuAction::PluginManager => {
            app.show_plugin_manager = !app.show_plugin_manager;
        }
        MenuAction::PluginAction { plugin_name, action } => {
            // Send MenuAction event directly to the target plugin.
            let event = conch_plugin_sdk::PluginEvent::MenuAction { action };
            if let Ok(json) = serde_json::to_string(&event) {
                if let Some(sender) = app.plugin_bus.sender_for(&plugin_name) {
                    let _ = sender.try_send(conch_plugin::bus::PluginMail::WidgetEvent { json });
                }
            }
        }
    }
}

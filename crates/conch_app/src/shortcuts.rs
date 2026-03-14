//! Keyboard shortcut handling.

use crate::app::ConchApp;
use crate::input;

impl ConchApp {
    /// Process keyboard events: app shortcuts always run, PTY forwarding
    /// only when `forward_to_pty` is true (i.e. no text widget has focus).
    pub(crate) fn handle_keyboard(&mut self, ctx: &egui::Context, forward_to_pty: bool) {
        use alacritty_terminal::term::TermMode;

        let app_cursor = forward_to_pty
            && self.state.active_session().map_or(false, |s| {
                s.term()
                    .try_lock_unfair()
                    .map_or(false, |term| term.mode().contains(TermMode::APP_CURSOR))
            });

        ctx.input(|input| {
            for event in &input.events {
                match event {
                    egui::Event::Key {
                        key,
                        pressed: true,
                        modifiers,
                        ..
                    } => {
                        // Command+number -> switch to tab N.
                        if modifiers.command && !modifiers.alt && !modifiers.shift {
                            let tab_num = match key {
                                egui::Key::Num1 => Some(0usize),
                                egui::Key::Num2 => Some(1),
                                egui::Key::Num3 => Some(2),
                                egui::Key::Num4 => Some(3),
                                egui::Key::Num5 => Some(4),
                                egui::Key::Num6 => Some(5),
                                egui::Key::Num7 => Some(6),
                                egui::Key::Num8 => Some(7),
                                egui::Key::Num9 => Some(8),
                                _ => None,
                            };
                            if let Some(idx) = tab_num {
                                if let Some(&id) = self.state.tab_order.get(idx) {
                                    self.state.active_tab = Some(id);
                                    return;
                                }
                            }
                        }

                        // App-level configurable shortcuts.
                        if let Some(ref kb) = self.shortcuts.new_window {
                            if kb.matches(key, modifiers) {
                                self.spawn_extra_window();
                                return;
                            }
                        }
                        if let Some(ref kb) = self.shortcuts.new_tab {
                            if kb.matches(key, modifiers) { self.open_local_tab(); return; }
                        }
                        if let Some(ref kb) = self.shortcuts.close_tab {
                            if kb.matches(key, modifiers) {
                                if let Some(id) = self.state.active_tab {
                                    self.remove_session(id);
                                }
                                return;
                            }
                        }
                        if let Some(ref kb) = self.shortcuts.quit {
                            if kb.matches(key, modifiers) {
                                self.quit_requested = true;
                                return;
                            }
                        }
                        if let Some(ref kb) = self.shortcuts.toggle_left_panel {
                            if kb.matches(key, modifiers) {
                                self.left_panel_visible = !self.left_panel_visible;
                                return;
                            }
                        }
                        if let Some(ref kb) = self.shortcuts.toggle_right_panel {
                            if kb.matches(key, modifiers) {
                                self.right_panel_visible = !self.right_panel_visible;
                                return;
                            }
                        }
                        if let Some(ref kb) = self.shortcuts.toggle_bottom_panel {
                            if kb.matches(key, modifiers) {
                                self.bottom_panel_visible = !self.bottom_panel_visible;
                                return;
                            }
                        }
                        if let Some(ref kb) = self.shortcuts.zen_mode {
                            if kb.matches(key, modifiers) {
                                self.toggle_zen_mode();
                                return;
                            }
                        }

                        // Plugin-registered global keybindings.
                        for pkb in &self.plugin_keybindings {
                            if pkb.binding.matches(key, modifiers) {
                                let action = crate::menu_bar::MenuAction::PluginAction {
                                    plugin_name: pkb.plugin_name.clone(),
                                    action: pkb.action.clone(),
                                };
                                crate::menu_bar::handle_action(action, ctx, self);
                                return;
                            }
                        }

                        // Ctrl+Shift+C for copy on non-macOS.
                        #[cfg(not(target_os = "macos"))]
                        if forward_to_pty && modifiers.ctrl && modifiers.shift && *key == egui::Key::C {
                            if let Some((start, end)) = self.selection.normalized() {
                                if let Some(session) = self.state.active_session() {
                                    let text = crate::terminal::widget::get_selected_text(session.term(), start, end);
                                    if !text.is_empty() {
                                        ctx.copy_text(text);
                                    }
                                }
                            }
                            return;
                        }

                        // Forward to active terminal.
                        if forward_to_pty {
                            if let Some(bytes) = input::key_to_bytes(key, modifiers, None, &self.shortcuts, app_cursor, &self.plugin_keybindings) {
                                if let Some(session) = self.state.active_session() {
                                    if let Some(mut term) = session.term().try_lock_unfair() {
                                        term.scroll_display(alacritty_terminal::grid::Scroll::Bottom);
                                    }
                                    session.write(&bytes);
                                }
                            }
                        }
                    }
                    egui::Event::Text(text) => {
                        if forward_to_pty {
                            if let Some(session) = self.state.active_session() {
                                if let Some(mut term) = session.term().try_lock_unfair() {
                                    term.scroll_display(alacritty_terminal::grid::Scroll::Bottom);
                                }
                                session.write(text.as_bytes());
                            }
                        }
                    }
                    _ => {}
                }
            }
        });
    }
}

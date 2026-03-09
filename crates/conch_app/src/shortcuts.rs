//! Keyboard shortcut handling and file browser navigation.

use crate::app::ConchApp;
use crate::input;
#[cfg(not(target_os = "macos"))]
use crate::terminal::widget::get_selected_text;
use crate::ui::dialogs::new_connection::NewConnectionForm;
use crate::ui::dialogs::tunnels::TunnelManagerState;
use crate::ui::file_browser::FileBrowserPane;
use crate::ui::sidebar::{self, SidebarAction};
use conch_core::config;

impl ConchApp {
    /// Process keyboard events: app shortcuts always run, PTY forwarding
    /// only when `forward_to_pty` is true (i.e. no text widget has focus).
    pub(crate) fn handle_keyboard(&mut self, ctx: &egui::Context, forward_to_pty: bool) {
        use alacritty_terminal::term::TermMode;

        let app_cursor = forward_to_pty
            && self.state.active_session().map_or(false, |s| {
                s.backend
                    .term()
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
                        // ESC closes the topmost dialog (when no text field has focus).
                        if *key == egui::Key::Escape && !modifiers.command && !modifiers.alt && !modifiers.shift {
                            if self.close_topmost_dialog() {
                                return;
                            }
                        }

                        // Command+number → switch to tab N (checked first).
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
                                    log::debug!("close_tab: removing session {id}");
                                    self.remove_session(id);
                                    log::debug!("close_tab: session removed, {} remaining", self.state.sessions.len());
                                    if self.state.sessions.is_empty() {
                                        log::debug!("close_tab: opening new local tab");
                                        self.open_local_tab();
                                        log::debug!("close_tab: new tab opened");
                                    }
                                }
                                return;
                            }
                        }
                        if let Some(ref kb) = self.shortcuts.toggle_left_sidebar {
                            if kb.matches(key, modifiers) { self.toggle_left_sidebar(); return; }
                        }
                        if let Some(ref kb) = self.shortcuts.toggle_right_sidebar {
                            if kb.matches(key, modifiers) { self.toggle_right_sidebar(); return; }
                        }
                        if let Some(ref kb) = self.shortcuts.new_connection {
                            if kb.matches(key, modifiers) {
                                self.state.new_connection_form =
                                    Some(NewConnectionForm::with_defaults());
                                return;
                            }
                        }
                        if let Some(ref kb) = self.shortcuts.focus_quick_connect {
                            if kb.matches(key, modifiers) {
                                if self.quick_connect_opened_sidebar && self.state.show_right_sidebar {
                                    self.state.show_right_sidebar = false;
                                    self.quick_connect_opened_sidebar = false;
                                    self.session_panel_state.quick_connect_query.clear();
                                } else if !self.state.show_right_sidebar {
                                    self.quick_connect_opened_sidebar = true;
                                    self.state.show_right_sidebar = true;
                                    self.session_panel_state.quick_connect_focus = true;
                                } else {
                                    self.session_panel_state.quick_connect_focus = true;
                                }
                                return;
                            }
                        }
                        if let Some(ref kb) = self.shortcuts.focus_plugin_search {
                            if kb.matches(key, modifiers) {
                                if !self.state.show_left_sidebar {
                                    self.plugin_search_opened_sidebar = true;
                                }
                                self.state.show_left_sidebar = true;
                                self.state.sidebar_tab = sidebar::SidebarTab::Plugins;
                                self.plugin_search_focus = true;
                                return;
                            }
                        }
                        if let Some(ref kb) = self.shortcuts.focus_files {
                            if kb.matches(key, modifiers) {
                                self.focus_file_browser();
                                return;
                            }
                        }
                        if let Some(ref kb) = self.shortcuts.zen_mode {
                            if kb.matches(key, modifiers) {
                                self.toggle_zen_mode();
                                return;
                            }
                        }
                        if let Some(ref kb) = self.shortcuts.ssh_tunnels {
                            if kb.matches(key, modifiers) {
                                self.tunnel_dialog = Some(TunnelManagerState::new());
                                return;
                            }
                        }
                        if let Some(ref kb) = self.shortcuts.notification_history {
                            if kb.matches(key, modifiers) {
                                self.notification_history_dialog = Some(
                                    crate::ui::dialogs::notification_history::NotificationHistoryState::new(),
                                );
                                return;
                            }
                        }
                        if let Some(ref kb) = self.shortcuts.toggle_bottom_panel {
                            if kb.matches(key, modifiers) {
                                self.toggle_bottom_panel();
                                return;
                            }
                        }
                        if let Some(ref kb) = self.shortcuts.quit {
                            if kb.matches(key, modifiers) {
                                self.quit_requested = true;
                                return;
                            }
                        }

                        // Plugin keybindings (lower priority than app shortcuts).
                        {
                            let mut triggered = None;
                            for pkb in &self.plugin_keybinds {
                                if pkb.binding.matches(key, modifiers) {
                                    triggered = Some((pkb.plugin_idx, pkb.action.clone()));
                                    break;
                                }
                            }
                            if let Some((idx, action)) = triggered {
                                self.handle_plugin_keybind(idx, &action);
                                return;
                            }
                        }

                        // On Linux/Windows, Ctrl+Shift+C copies terminal selection
                        // (since Ctrl+C is forwarded to the PTY as SIGINT).
                        #[cfg(not(target_os = "macos"))]
                        if forward_to_pty && modifiers.ctrl && modifiers.shift && *key == egui::Key::C {
                            self.copy_terminal_selection(ctx);
                            return;
                        }

                        // File browser keyboard navigation.
                        if self.state.file_browser.focused {
                            self.handle_file_browser_key(key, modifiers);
                            return;
                        }

                        // Forward to active terminal only when no text widget has focus.
                        if forward_to_pty {
                            if let Some(bytes) = input::key_to_bytes(key, modifiers, None, &self.shortcuts, app_cursor) {
                                if let Some(session) = self.state.active_session() {
                                    if let Some(mut term) = session.backend.term().try_lock_unfair() {
                                        term.scroll_display(alacritty_terminal::grid::Scroll::Bottom);
                                    }
                                    session.backend.write(&bytes);
                                }
                            }
                        }
                    }
                    egui::Event::Text(text) => {
                        if forward_to_pty && !self.state.file_browser.focused {
                            if let Some(session) = self.state.active_session() {
                                if let Some(mut term) = session.backend.term().try_lock_unfair() {
                                    term.scroll_display(alacritty_terminal::grid::Scroll::Bottom);
                                }
                                session.backend.write(text.as_bytes());
                            }
                        }
                    }
                    _ => {}
                }
            }
        });
    }

    /// Copy the current terminal text selection to the clipboard.
    #[cfg(not(target_os = "macos"))]
    pub(crate) fn copy_terminal_selection(&self, ctx: &egui::Context) {
        if let Some((start, end)) = self.selection.normalized() {
            if let Some(session) = self.state.active_session() {
                let text = get_selected_text(session.backend.term(), start, end);
                if !text.is_empty() {
                    ctx.copy_text(text);
                }
            }
        }
    }

    pub(crate) fn toggle_left_sidebar(&mut self) {
        self.state.show_left_sidebar = !self.state.show_left_sidebar;
        self.state.persistent.layout.left_panel_collapsed = !self.state.show_left_sidebar;
        if !self.state.show_left_sidebar {
            self.state.file_browser.focused = false;
        }
        let _ = config::save_persistent_state(&self.state.persistent);
    }

    pub(crate) fn toggle_right_sidebar(&mut self) {
        self.state.show_right_sidebar = !self.state.show_right_sidebar;
        self.state.persistent.layout.right_panel_collapsed = !self.state.show_right_sidebar;
        let _ = config::save_persistent_state(&self.state.persistent);
    }

    pub(crate) fn toggle_bottom_panel(&mut self) {
        if self.show_bottom_panel {
            // Always allow hiding.
            self.show_bottom_panel = false;
        } else if !self.bottom_panel_tabs.is_empty() {
            // Only show if there are active bottom panel plugins.
            self.show_bottom_panel = true;
        } else {
            return;
        }
        self.state.persistent.layout.bottom_panel_collapsed = !self.show_bottom_panel;
        let _ = config::save_persistent_state(&self.state.persistent);
    }

    pub(crate) fn toggle_zen_mode(&mut self) {
        if self.state.show_left_sidebar || self.state.show_right_sidebar || self.show_bottom_panel {
            self.state.show_left_sidebar = false;
            self.state.show_right_sidebar = false;
            self.show_bottom_panel = false;
            self.state.file_browser.focused = false;
        } else {
            self.state.show_left_sidebar = true;
            self.state.show_right_sidebar = true;
            self.show_bottom_panel = !self.bottom_panel_tabs.is_empty();
        }
        self.state.persistent.layout.left_panel_collapsed = !self.state.show_left_sidebar;
        self.state.persistent.layout.right_panel_collapsed = !self.state.show_right_sidebar;
        self.state.persistent.layout.bottom_panel_collapsed = !self.show_bottom_panel;
        let _ = config::save_persistent_state(&self.state.persistent);
    }

    pub(crate) fn focus_file_browser(&mut self) {
        if self.state.file_browser.focused {
            self.state.file_browser.focused = false;
            return;
        }
        if !self.state.show_left_sidebar {
            self.state.show_left_sidebar = true;
            self.state.persistent.layout.left_panel_collapsed = false;
            let _ = config::save_persistent_state(&self.state.persistent);
        }
        self.state.sidebar_tab = sidebar::SidebarTab::Files;
        self.state.file_browser.focused = true;
        if self.state.file_browser.remote_path.is_some() && self.state.file_browser.local_selected.is_none() {
            self.state.file_browser.active_pane = FileBrowserPane::Remote;
        } else {
            self.state.file_browser.active_pane = FileBrowserPane::Local;
        }
        match self.state.file_browser.active_pane {
            FileBrowserPane::Local => {
                if self.state.file_browser.local_selected.is_none() && !self.state.file_browser.local_entries.is_empty() {
                    self.state.file_browser.local_selected = Some(0);
                }
            }
            FileBrowserPane::Remote => {
                if self.state.file_browser.remote_selected.is_none() && !self.state.file_browser.remote_entries.is_empty() {
                    self.state.file_browser.remote_selected = Some(0);
                }
            }
        }
    }

    pub(crate) fn handle_file_browser_key(&mut self, key: &egui::Key, _modifiers: &egui::Modifiers) {
        let fb = &mut self.state.file_browser;
        let pane = fb.active_pane;

        // Arrow keys and simple state changes — handle directly and return.
        match key {
            egui::Key::Escape => { fb.focused = false; return; }
            egui::Key::ArrowUp | egui::Key::ArrowDown => {
                let (entries_len, selected) = match pane {
                    FileBrowserPane::Local => (fb.local_entries.len(), &mut fb.local_selected),
                    FileBrowserPane::Remote => (fb.remote_entries.len(), &mut fb.remote_selected),
                };
                if *key == egui::Key::ArrowUp {
                    if let Some(sel) = *selected {
                        if sel > 0 { *selected = Some(sel - 1); }
                    } else if entries_len > 0 {
                        *selected = Some(0);
                    }
                } else if let Some(sel) = *selected {
                    if sel + 1 < entries_len { *selected = Some(sel + 1); }
                } else if entries_len > 0 {
                    *selected = Some(0);
                }
                return;
            }
            egui::Key::Tab => {
                if fb.remote_path.is_some() {
                    fb.active_pane = match pane {
                        FileBrowserPane::Local => FileBrowserPane::Remote,
                        FileBrowserPane::Remote => FileBrowserPane::Local,
                    };
                    let (len, sel) = match fb.active_pane {
                        FileBrowserPane::Local => (fb.local_entries.len(), &mut fb.local_selected),
                        FileBrowserPane::Remote => (fb.remote_entries.len(), &mut fb.remote_selected),
                    };
                    if sel.is_none() && len > 0 { *sel = Some(0); }
                }
                return;
            }
            _ => {}
        }

        // Actions that produce a SidebarAction — read state immutably.
        let fb = &self.state.file_browser;
        let action = match key {
            egui::Key::Enter => {
                let entry = match pane {
                    FileBrowserPane::Local => fb.local_selected.and_then(|i| fb.local_entries.get(i)),
                    FileBrowserPane::Remote => fb.remote_selected.and_then(|i| fb.remote_entries.get(i)),
                };
                if let Some(entry) = entry.filter(|e| e.is_dir) {
                    let path = entry.path.clone();
                    match pane {
                        FileBrowserPane::Local => SidebarAction::NavigateLocal(path),
                        FileBrowserPane::Remote => SidebarAction::NavigateRemote(path),
                    }
                } else {
                    return;
                }
            }
            egui::Key::Backspace => {
                let parent = match pane {
                    FileBrowserPane::Local => fb.local_path.parent().map(|p| p.to_path_buf()),
                    FileBrowserPane::Remote => fb.remote_path.as_ref().and_then(|p| p.parent().map(|p| p.to_path_buf())),
                };
                if let Some(parent) = parent {
                    match pane {
                        FileBrowserPane::Local => SidebarAction::NavigateLocal(parent),
                        FileBrowserPane::Remote => SidebarAction::NavigateRemote(parent),
                    }
                } else {
                    return;
                }
            }
            egui::Key::U => {
                if let (Some(idx), Some(remote_dir)) = (fb.local_selected, fb.remote_path.clone()) {
                    if let Some(entry) = fb.local_entries.get(idx) {
                        SidebarAction::Upload { local_path: entry.path.clone(), remote_dir }
                    } else { return; }
                } else { return; }
            }
            egui::Key::D => {
                if let Some(idx) = fb.remote_selected {
                    if let Some(entry) = fb.remote_entries.get(idx) {
                        SidebarAction::Download { remote_path: entry.path.clone(), local_dir: fb.local_path.clone() }
                    } else { return; }
                } else { return; }
            }
            egui::Key::R => match pane {
                FileBrowserPane::Local => SidebarAction::RefreshLocal,
                FileBrowserPane::Remote => SidebarAction::RefreshRemote,
            },
            egui::Key::H => match pane {
                FileBrowserPane::Local => SidebarAction::GoHomeLocal,
                FileBrowserPane::Remote => SidebarAction::GoHomeRemote,
            },
            _ => return,
        };

        // Clear selection on navigation.
        if matches!(action, SidebarAction::NavigateLocal(_) | SidebarAction::NavigateRemote(_)) {
            match pane {
                FileBrowserPane::Local => self.state.file_browser.local_selected = None,
                FileBrowserPane::Remote => self.state.file_browser.remote_selected = None,
            }
        }

        self.handle_sidebar_action(action);
    }
}

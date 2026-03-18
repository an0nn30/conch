//! Root coordinator — manages window lifecycle, plugin infrastructure, and
//! background tasks.  The root eframe viewport is invisible; all visible
//! windows are deferred viewports rendered by `render_window()`.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use conch_core::config;
use conch_plugin::bus::PluginBus;
use conch_plugin::jvm::runtime::JavaPluginManager;
use conch_plugin::lua::runner::RunningLuaPlugin;
use conch_plugin::native::manager::NativePluginManager;
use egui::ViewportCommand;
use parking_lot::Mutex;
use tokio::sync::oneshot;

use crate::host::bridge::{self, PanelRegistry, SessionRegistry};
use crate::host::dialogs;
use crate::host::plugin_manager_ui::PluginManagerState;
use crate::input::{KeyBinding, ResolvedPluginKeybind, ResolvedShortcuts};
use crate::ipc::{IpcListener, IpcMessage};
use crate::menu_bar::MenuBarState;
use crate::notifications::NotificationManager;
use crate::platform::PlatformCapabilities;
use crate::sessions::create_local_session;
use crate::terminal::color::ResolvedColors;
use crate::watcher::{FileChangeKind, FileWatcher};
use crate::window_state::{render_window, SharedAppState, SharedConfig, WindowAction, WindowState};

pub struct ConchApp {
    pub(crate) shared: Arc<SharedAppState>,

    /// ALL user-visible windows.  There is no privileged "main" window.
    pub(crate) windows: Vec<Arc<Mutex<WindowState>>>,
    pub(crate) next_viewport_num: u32,

    // Plugin managers (Option — None when that type is disabled by config).
    pub(crate) native_plugin_mgr: Option<NativePluginManager>,
    pub(crate) lua_plugins: HashMap<String, RunningLuaPlugin>,
    /// Boxed HostApi for Lua plugins (kept alive so the pointer stays valid).
    pub(crate) lua_host_api: Option<Box<conch_plugin_sdk::HostApi>>,
    pub(crate) java_plugin_mgr: Option<JavaPluginManager>,
    pub(crate) render_pending: HashMap<String, oneshot::Receiver<String>>,
    pub(crate) render_last_request: HashMap<String, Instant>,

    /// Per-window tab-change tracking for plugin bus events.
    prev_active_tabs: HashMap<egui::ViewportId, Option<uuid::Uuid>>,

    // System.
    pub(crate) ipc_listener: Option<IpcListener>,
    pub(crate) file_watcher: Option<FileWatcher>,
    pub(crate) quit_requested: bool,
    pub(crate) rt: Arc<tokio::runtime::Runtime>,

    /// Icon + initial window size passed from main().
    initial_window_icon: Option<Arc<egui::IconData>>,
    initial_window_size: [f32; 2],
}

impl ConchApp {
    pub fn new(
        rt: Arc<tokio::runtime::Runtime>,
        initial_window_size: [f32; 2],
        icon: Arc<egui::IconData>,
    ) -> Self {
        let user_config = config::load_user_config().unwrap_or_else(|e| {
            log::error!("Failed to load config: {e:#}");
            config::UserConfig::default()
        });
        let persistent = config::load_persistent_state().unwrap_or_default();

        let shortcuts = ResolvedShortcuts::from_config(&user_config.conch.keyboard);
        let platform = PlatformCapabilities::current();
        let menu_bar_state = MenuBarState::new(user_config.conch.ui.native_menu_bar, &platform);

        let scheme = conch_core::color_scheme::resolve_theme(&user_config.colors.theme);
        let colors = ResolvedColors::from_scheme(&scheme);
        let theme = crate::ui_theme::UiTheme::from_colors(&colors, user_config.colors.appearance_mode, &user_config.conch.ui.font);

        let ipc_listener = IpcListener::start();
        let file_watcher = FileWatcher::start();

        // Plugin infrastructure.
        // The bus, registry, and bridge are always created (lightweight),
        // but per-type managers are only created when enabled in config.
        let plugins_cfg = &user_config.conch.plugins;
        let plugins_enabled = plugins_cfg.any_enabled();

        let plugin_bus = Arc::new(PluginBus::new());
        let panel_registry = Arc::new(Mutex::new(PanelRegistry::new()));
        let (dialog_tx, dialog_state) = dialogs::dialog_channel();
        let session_registry = Arc::new(Mutex::new(SessionRegistry::new()));
        let notification_rx = crate::notifications::init_channel();
        let notifications = NotificationManager::new(notification_rx);
        bridge::init_bridge(
            Arc::clone(&plugin_bus),
            Arc::clone(&panel_registry),
            dialog_tx,
            Arc::clone(&session_registry),
        );

        // Only build HostApi vtables and managers for enabled plugin types.
        // This avoids allocating resources (and starting a JVM) for types
        // the user has explicitly disabled.
        let native_plugin_mgr = if plugins_enabled && plugins_cfg.native {
            let host_api = bridge::build_host_api();
            Some(NativePluginManager::new(Arc::clone(&plugin_bus), host_api))
        } else {
            if !plugins_cfg.native { log::info!("Native plugins disabled by config"); }
            None
        };
        let java_plugin_mgr = if plugins_enabled && plugins_cfg.java {
            let c_host_api = bridge::build_host_api();
            // Leak the C vtable so it's 'static, then wrap in the safe trait adapter.
            let leaked: &'static conch_plugin_sdk::HostApi = Box::leak(Box::new(c_host_api));
            let host_api: std::sync::Arc<dyn conch_plugin::HostApi> = std::sync::Arc::new(
                unsafe { conch_plugin::CHostApiAdapter::new("java".to_string(), leaked) }
            );
            Some(JavaPluginManager::new(Arc::clone(&plugin_bus), host_api))
        } else {
            if !plugins_cfg.java { log::info!("Java plugins disabled by config"); }
            None
        };
        let lua_host_api = if plugins_enabled && plugins_cfg.lua {
            Some(Box::new(bridge::build_host_api()))
        } else {
            if plugins_enabled && !plugins_cfg.lua { log::info!("Lua plugins disabled by config"); }
            None
        };
        if !plugins_cfg.enabled {
            log::info!("Plugin system disabled by config");
        }

        let shared_config = SharedConfig {
            user_config,
            persistent,
            colors,
            theme,
            theme_dirty: true,
            theme_version: 1,
            shortcuts,
            plugin_keybindings: Vec::new(),
            plugin_keybindings_version: 0,
        };

        let shared = Arc::new(SharedAppState {
            config: Mutex::new(shared_config),
            plugin_bus,
            panel_registry,
            session_registry,
            render_cache: Mutex::new(HashMap::new()),
            dialog_state: Mutex::new(dialog_state),
            notifications: Mutex::new(notifications),
            icon_cache: Mutex::new(None),
            menu_bar_state: Mutex::new(menu_bar_state),
            plugin_manager: Mutex::new(PluginManagerState::default()),
            platform,
            force_render: Mutex::new(Vec::new()),
        });

        let mut app = Self {
            shared,
            windows: Vec::new(),
            next_viewport_num: 1,
            native_plugin_mgr,
            lua_plugins: HashMap::new(),
            lua_host_api,
            java_plugin_mgr,
            render_pending: HashMap::new(),
            render_last_request: HashMap::new(),
            prev_active_tabs: HashMap::new(),
            ipc_listener,
            file_watcher,
            quit_requested: false,
            rt,
            initial_window_icon: Some(icon),
            initial_window_size,
        };

        if plugins_enabled {
            app.discover_plugins();
            app.auto_load_plugins();
        }
        app
    }

    fn refresh_plugin_keybindings(&self) {
        let version = bridge::plugin_menu_items_version();
        let mut cfg = self.shared.config.lock();
        if version == cfg.plugin_keybindings_version { return; }
        cfg.plugin_keybindings_version = version;
        cfg.plugin_keybindings = bridge::plugin_menu_items()
            .into_iter()
            .filter_map(|item| {
                let kb_str = item.keybind.as_deref()?;
                let binding = KeyBinding::parse(kb_str)?;
                Some(ResolvedPluginKeybind { binding, plugin_name: item.plugin_name, action: item.action })
            })
            .collect();
    }

    fn build_window_viewport(&self, size: [f32; 2]) -> egui::ViewportBuilder {
        let cfg = self.shared.config.lock();
        let decorations = self.shared.platform.effective_decorations(cfg.user_config.window.decorations);
        let mut builder = egui::ViewportBuilder::default().with_inner_size(size);
        if let Some(icon) = &self.initial_window_icon {
            builder = builder.with_icon(Arc::clone(icon));
        }
        drop(cfg);
        crate::build_viewport(builder, decorations, &self.shared.platform)
    }

    pub(crate) fn spawn_window(&mut self, cwd: Option<std::path::PathBuf>) -> Option<egui::ViewportId> {
        let cwd = cwd.or_else(|| {
            self.windows.iter()
                .find(|w| w.lock().has_focus)
                .and_then(|w| {
                    let win = w.lock();
                    win.active_session().and_then(|s| s.child_pid()).and_then(conch_pty::get_cwd_of_pid)
                })
        });
        let user_config = self.shared.config.lock().user_config.clone();
        let (_, session) = create_local_session(&user_config, cwd)?;
        // First window uses ROOT viewport (rendered directly by update()).
        // Extra windows get their own viewport IDs (rendered via show_viewport_immediate).
        let viewport_id = if self.windows.is_empty() {
            egui::ViewportId::ROOT
        } else {
            let num = self.next_viewport_num;
            self.next_viewport_num += 1;
            egui::ViewportId::from_hash_of(format!("conch_window_{num}"))
        };
        let size = if self.windows.is_empty() { self.initial_window_size } else { [800.0, 600.0] };
        let builder = self.build_window_viewport(size);
        let layout = self.shared.config.lock().persistent.layout.clone();
        let win = WindowState::with_session(viewport_id, builder, session, &layout);
        self.windows.push(Arc::new(Mutex::new(win)));
        Some(viewport_id)
    }

    fn handle_ipc(&mut self) {
        let Some(listener) = &self.ipc_listener else { return };
        for msg in listener.drain() {
            match msg {
                IpcMessage::CreateWindow { working_directory } => {
                    self.spawn_window(working_directory.map(std::path::PathBuf::from));
                }
                IpcMessage::CreateTab { working_directory } => {
                    let cwd = working_directory.map(std::path::PathBuf::from);
                    let user_config = self.shared.config.lock().user_config.clone();
                    if let Some((id, session)) = create_local_session(&user_config, cwd) {
                        let target = self.windows.iter()
                            .find(|w| w.lock().has_focus)
                            .or_else(|| self.windows.first());
                        if let Some(w) = target {
                            let mut win = w.lock();
                            win.sessions.insert(id, session);
                            win.tab_order.push(id);
                            win.active_tab = Some(id);
                        }
                    }
                }
            }
        }
    }

    fn drain_pending_sessions(&mut self) {
        let mut registry = self.shared.session_registry.lock();
        let pending: Vec<_> = registry.pending_open.drain(..).collect();
        let closing: Vec<_> = registry.pending_close.drain(..).collect();
        let status_updates: Vec<_> = registry.pending_status.drain(..).collect();
        drop(registry);

        for mut ps in pending {
            let id = uuid::Uuid::new_v4();
            let event_rx = ps.bridge.take_event_rx();
            let session = crate::state::Session {
                id, title: ps.title, custom_title: None,
                backend: crate::state::SessionBackend::Plugin { bridge: ps.bridge, vtable: ps.vtable, backend_handle: ps.backend_handle },
                event_rx, status: conch_plugin_sdk::SessionStatus::Connecting,
                status_detail: None, connect_started: Some(Instant::now()), prompt: None,
            };
            let target = ps.target_viewport
                .and_then(|vp| self.windows.iter().find(|w| w.lock().viewport_id == vp))
                .or_else(|| self.windows.first());
            if let Some(window_arc) = target {
                let mut win = window_arc.lock();
                if win.last_cols > 0 && win.last_rows > 0 {
                    session.resize(win.last_cols, win.last_rows, win.cell_width as u16, win.cell_height as u16);
                }
                win.sessions.insert(id, session);
                win.tab_order.push(id);
                win.active_tab = Some(id);
            }
        }

        for handle in closing {
            for window_arc in &self.windows {
                let mut win = window_arc.lock();
                let found = win.sessions.iter().find_map(|(sid, s)| {
                    if let crate::state::SessionBackend::Plugin { bridge, .. } = &s.backend {
                        if bridge.handle == handle { return Some(*sid); }
                    }
                    None
                });
                if let Some(sid) = found { win.remove_session(sid); break; }
            }
        }

        let prompts: Vec<_> = {
            let mut reg = self.shared.session_registry.lock();
            reg.pending_prompts.drain(..).collect()
        };
        for prompt_req in prompts {
            let handle = prompt_req.handle;
            let prompt_state = crate::state::SessionPrompt {
                prompt_type: prompt_req.prompt_type, message: prompt_req.message,
                detail: prompt_req.detail, password_buf: String::new(),
                focus_password: true, show_password: false, reply: Some(prompt_req.reply),
            };
            for window_arc in &self.windows {
                let mut win = window_arc.lock();
                let found = win.sessions.values_mut().find(|s| {
                    matches!(&s.backend, crate::state::SessionBackend::Plugin { bridge, .. } if bridge.handle == handle)
                });
                if let Some(session) = found { session.prompt = Some(prompt_state); break; }
            }
        }

        for update in status_updates {
            for window_arc in &self.windows {
                let mut win = window_arc.lock();
                let found = win.sessions.iter().find_map(|(sid, s)| {
                    if let crate::state::SessionBackend::Plugin { bridge, .. } = &s.backend {
                        if bridge.handle == update.handle { return Some(*sid); }
                    }
                    None
                });
                if let Some(sid) = found {
                    if let Some(session) = win.sessions.get_mut(&sid) {
                        session.status = update.status;
                        session.status_detail = update.detail;
                    }
                    break;
                }
            }
        }
    }

    fn drain_new_tabs(&mut self) {
        let tabs = crate::host::bridge::drain_new_tabs();
        if tabs.is_empty() {
            return;
        }
        let user_config = self.shared.config.lock().user_config.clone();
        for pending in tabs {
            let session_result = if pending.plain {
                crate::sessions::create_plain_session(&user_config, None)
            } else {
                create_local_session(&user_config, None)
            };
            if let Some((id, session)) = session_result {
                let target = self.windows.iter().find(|w| w.lock().has_focus)
                    .or_else(|| self.windows.first());
                if let Some(window_arc) = target {
                    let mut win = window_arc.lock();
                    if win.last_cols > 0 && win.last_rows > 0 {
                        session.resize(
                            win.last_cols, win.last_rows,
                            win.cell_width as u16, win.cell_height as u16,
                        );
                    }
                    // Write command to the new session after a brief delay
                    // to let the shell initialize.
                    if let Some(cmd) = pending.command {
                        let cmd_bytes = cmd.into_bytes();
                        // Write directly — the shell should be ready for input
                        // within the first frame since PTY setup is synchronous.
                        session.write(&cmd_bytes);
                    }
                    win.sessions.insert(id, session);
                    win.tab_order.push(id);
                    win.active_tab = Some(id);
                }
            }
        }
    }

    fn drain_pty_writes(&mut self) {
        let writes = crate::host::bridge::drain_pty_writes();
        if writes.is_empty() {
            return;
        }
        // Write to the focused window's active session.
        let target = self.windows.iter().find(|w| w.lock().has_focus)
            .or_else(|| self.windows.first());
        if let Some(window_arc) = target {
            let win = window_arc.lock();
            if let Some(session) = win.active_session() {
                for data in writes {
                    session.write(&data);
                }
            }
        }
    }

    fn check_tab_changes(&mut self) {
        for window_arc in &self.windows {
            let win = window_arc.lock();
            let prev = self.prev_active_tabs.get(&win.viewport_id).copied().flatten();
            if win.active_tab != prev {
                self.prev_active_tabs.insert(win.viewport_id, win.active_tab);
                let (is_ssh, session_id) = if let Some(session) = win.active_session() {
                    match &session.backend {
                        crate::state::SessionBackend::Plugin { bridge, .. } => (true, Some(bridge.handle.0)),
                        crate::state::SessionBackend::Local(_) => (false, None),
                    }
                } else { (false, None) };
                let mut data = serde_json::json!({ "is_ssh": is_ssh });
                if let Some(sid) = session_id { data["session_id"] = serde_json::json!(sid); }
                self.shared.plugin_bus.publish("app", "app.tab_changed", data);
            }
        }
    }
}

impl eframe::App for ConchApp {
    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        // Strip Tab key from the root viewport (windows[0]).  render_window()
        // also strips via ctx.input_mut(), but raw_input_hook catches it
        // earlier before egui's focus system ever sees it.
        if let Some(win_arc) = self.windows.first() {
            let win = win_arc.lock();
            if self.shared.dialog_state.lock().is_active_for(win.viewport_id) {
                return;
            }
            let mut tab_bytes: Option<Vec<u8>> = None;
            raw_input.events.retain(|e| match e {
                egui::Event::Key { key: egui::Key::Tab, pressed: true, modifiers, .. } => {
                    tab_bytes = Some(if modifiers.shift { b"\x1b[Z".to_vec() } else { b"\t".to_vec() });
                    false
                }
                _ => true,
            });
            if let Some(bytes) = tab_bytes {
                if let Some(session) = win.active_session() {
                    session.write(&bytes);
                }
            }
        }
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // ── Coordinator background work ──
        self.refresh_plugin_keybindings();
        if let Some(watcher) = &mut self.file_watcher {
            for change in watcher.poll() {
                match change.kind {
                    FileChangeKind::Config => {
                        log::info!("Config file changed, reloading...");
                        match config::load_user_config() {
                            Ok(new_config) => {
                                let scheme = conch_core::color_scheme::resolve_theme(&new_config.colors.theme);
                                let colors = ResolvedColors::from_scheme(&scheme);
                                let theme = crate::ui_theme::UiTheme::from_colors(&colors, new_config.colors.appearance_mode, &new_config.conch.ui.font);
                                let shortcuts = ResolvedShortcuts::from_config(&new_config.conch.keyboard);
                                self.shared.menu_bar_state.lock().update_mode(new_config.conch.ui.native_menu_bar, &self.shared.platform);
                                let mut cfg = self.shared.config.lock();
                                cfg.shortcuts = shortcuts; cfg.colors = colors; cfg.theme = theme;
                                cfg.theme_dirty = true; cfg.theme_version += 1; cfg.user_config = new_config;
                                drop(cfg);
                                crate::notifications::push(crate::notifications::Notification::new(
                                    Some("Config Reloaded".into()), "Configuration updated successfully.".into(),
                                    crate::notifications::NotificationLevel::Success, None));
                            }
                            Err(e) => {
                                crate::notifications::push(crate::notifications::Notification::new(
                                    Some("Config Error".into()), format!("Failed to reload config: {e}"),
                                    crate::notifications::NotificationLevel::Error, None));
                            }
                        }
                    }
                    FileChangeKind::Themes => {
                        log::info!("Themes changed, reloading...");
                        let mut cfg = self.shared.config.lock();
                        let scheme = conch_core::color_scheme::resolve_theme(&cfg.user_config.colors.theme);
                        cfg.colors = ResolvedColors::from_scheme(&scheme);
                        cfg.theme = crate::ui_theme::UiTheme::from_colors(&cfg.colors, cfg.user_config.colors.appearance_mode, &cfg.user_config.conch.ui.font);
                        cfg.theme_dirty = true; cfg.theme_version += 1;
                        drop(cfg);
                        crate::notifications::push(crate::notifications::Notification::new(
                            Some("Theme Reloaded".into()), "Theme updated successfully.".into(),
                            crate::notifications::NotificationLevel::Success, None));
                    }
                }
            }
        }
        self.handle_ipc();
        self.poll_plugin_renders();
        self.drain_pending_sessions();
        self.drain_pty_writes();
        self.drain_new_tabs();
        self.check_tab_changes();

        // ── Ensure at least one window exists ──
        if self.windows.is_empty() {
            self.spawn_window(None);
        }

        // ── Render windows ──
        // windows[0] maps to the ROOT viewport and is rendered directly.
        // windows[1..] use show_viewport_immediate.  All call render_window().
        let root_visible = {
            let win = self.windows[0].lock();
            !win.should_close
        };
        if root_visible {
            let mut win = self.windows[0].lock();
            render_window(ctx, &mut win, &self.shared);
        } else {
            // Root has no sessions — hide it but keep it alive so
            // show_viewport_immediate windows can still render.
            ctx.send_viewport_cmd(ViewportCommand::Visible(false));
        }

        for i in 1..self.windows.len() {
            let win = self.windows[i].lock();
            if win.should_close { continue; }
            let viewport_id = win.viewport_id;
            let builder = win.viewport_builder.clone()
                .unwrap_or_default()
                .with_title(&win.title);
            drop(win);

            let w = Arc::clone(&self.windows[i]);
            let s = Arc::clone(&self.shared);
            ctx.show_viewport_immediate(viewport_id, builder, move |vp_ctx, _class| {
                let mut win = w.lock();
                render_window(vp_ctx, &mut win, &s);
            });
        }

        // ── Drain actions from all windows ──
        let mut spawn_new = false;
        let mut pm_actions = Vec::new();
        for window_arc in &self.windows {
            let mut win = window_arc.lock();
            for action in win.pending_actions.drain(..) {
                match action {
                    WindowAction::SpawnNewWindow => spawn_new = true,
                    WindowAction::Quit => self.quit_requested = true,
                    WindowAction::PluginAction(a) => pm_actions.push(a),
                    WindowAction::WindowClosed(_) => {}
                    WindowAction::SaveLayoutState {
                        window_width, window_height, zoom_factor,
                        left_panel_width, right_panel_width, bottom_panel_height,
                        left_panel_visible, right_panel_visible,
                        bottom_panel_visible, status_bar_visible,
                    } => {
                        let mut cfg = self.shared.config.lock();
                        let layout = &mut cfg.persistent.layout;
                        if window_width > 100.0 && window_height > 100.0 {
                            layout.window_width = window_width;
                            layout.window_height = window_height;
                        }
                        layout.zoom_factor = zoom_factor;
                        if let Some(w) = left_panel_width { layout.left_panel_width = w; }
                        if let Some(w) = right_panel_width { layout.right_panel_width = w; }
                        if let Some(h) = bottom_panel_height { layout.bottom_panel_height = h; }
                        layout.left_panel_visible = left_panel_visible;
                        layout.right_panel_visible = right_panel_visible;
                        layout.bottom_panel_visible = bottom_panel_visible;
                        layout.status_bar_visible = status_bar_visible;
                    }
                    WindowAction::PublishTabChanged { is_ssh, session_id } => {
                        let mut data = serde_json::json!({ "is_ssh": is_ssh });
                        if let Some(sid) = session_id { data["session_id"] = serde_json::json!(sid); }
                        self.shared.plugin_bus.publish("app", "app.tab_changed", data);
                    }
                }
            }
        }
        for a in pm_actions { self.handle_plugin_manager_action(a); }

        // ── Window lifecycle ──
        // Remove closed non-root windows.  The root viewport can't be
        // destroyed (eframe owns it), so it stays in the list but hidden.
        if self.windows.len() > 1 {
            let root_vp = self.windows[0].lock().viewport_id;
            self.windows.retain(|w| {
                let win = w.lock();
                win.viewport_id == root_vp || !win.should_close
            });
        }

        if spawn_new { self.spawn_window(None); }

        // ── Exit when ALL windows are closed ──
        let all_closed = self.windows.iter().all(|w| w.lock().should_close);
        if all_closed || self.quit_requested {
            if self.quit_requested {
                for w in &self.windows {
                    let mut win = w.lock();
                    for (_, session) in &win.sessions { session.shutdown(); }
                }
            }
            ctx.send_viewport_cmd(ViewportCommand::Close);
            return;
        }

        // If the root closed but other windows remain, make the root
        // visible again if a new window was promoted into it (future),
        // or just keep it hidden.  The app stays alive.
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.save_loaded_plugins();
        let lua_names: Vec<String> = self.lua_plugins.keys().cloned().collect();
        for name in lua_names {
            if let Some(mut running) = self.lua_plugins.remove(&name) {
                let _ = running.sender.try_send(conch_plugin::bus::PluginMail::Shutdown);
                if let Some(handle) = running.thread.take() { let _ = handle.join(); }
                self.shared.plugin_bus.unregister_plugin(&name);
            }
        }
        if let Some(mgr) = &mut self.native_plugin_mgr { mgr.shutdown_all(); }
        if let Some(mgr) = &mut self.java_plugin_mgr { mgr.shutdown_all(); }
        let cfg = self.shared.config.lock();
        let _ = config::save_persistent_state(&cfg.persistent);
    }
}

// ---------------------------------------------------------------------------
// Connecting / Error screens (free functions, used by render_window)
// ---------------------------------------------------------------------------

pub(crate) enum ConnectingAction { None, Accept, Reject, SubmitPassword(String) }

pub(crate) fn show_connecting_screen(
    ui: &mut egui::Ui, title: &str, detail: Option<&str>,
    started: Option<std::time::Instant>,
    prompt: Option<&mut crate::state::SessionPrompt>,
    icon_cache: Option<&crate::icons::IconCache>,
) -> ConnectingAction {
    let rect = ui.available_rect_before_wrap();
    let bg = if ui.visuals().dark_mode { egui::Color32::from_gray(30) } else { egui::Color32::from_gray(241) };
    ui.painter().rect_filled(rect, 0.0, bg);
    let center = rect.center();

    if let Some(prompt) = prompt {
        let content_width = (rect.width() * 0.7).min(560.0);
        let content_rect = egui::Rect::from_center_size(center, egui::Vec2::new(content_width, rect.height() * 0.7));
        let mut action = ConnectingAction::None;
        if prompt.prompt_type == 0 {
            ui.allocate_new_ui(egui::UiBuilder::new().max_rect(content_rect), |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(20.0);
                    let is_changed = prompt.message.contains("HAS CHANGED");
                    if is_changed {
                        ui.label(egui::RichText::new("WARNING: HOST KEY HAS CHANGED!").size(22.0).strong().color(egui::Color32::from_rgb(220, 50, 50)));
                        ui.add_space(8.0);
                        ui.label(egui::RichText::new(&prompt.message).size(13.0).color(if ui.visuals().dark_mode { egui::Color32::from_gray(180) } else { egui::Color32::from_gray(60) }));
                    } else { ui.label(egui::RichText::new(&prompt.message).size(18.0)); }
                    if !prompt.detail.is_empty() { ui.add_space(16.0); ui.label(egui::RichText::new(&prompt.detail).size(15.0).family(egui::FontFamily::Monospace).strong()); }
                    ui.add_space(20.0);
                    ui.label(egui::RichText::new("Are you sure you want to continue connecting?").size(14.0));
                    ui.add_space(12.0);
                    let btn_size = egui::Vec2::new(120.0, 34.0);
                    ui.horizontal(|ui| {
                        let total_w = btn_size.x * 2.0 + ui.spacing().item_spacing.x;
                        let avail = ui.available_width();
                        if avail > total_w { ui.add_space((avail - total_w) / 2.0); }
                        if ui.add_sized(btn_size, egui::Button::new("Accept")).clicked() { action = ConnectingAction::Accept; }
                        if ui.add_sized(btn_size, egui::Button::new("Reject")).clicked() { action = ConnectingAction::Reject; }
                    });
                });
            });
        } else {
            ui.allocate_new_ui(egui::UiBuilder::new().max_rect(content_rect), |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(20.0);
                    ui.label(egui::RichText::new(&prompt.message).size(22.0));
                    if !prompt.detail.is_empty() { ui.add_space(4.0); ui.label(egui::RichText::new(&prompt.detail).size(14.0).color(if ui.visuals().dark_mode { egui::Color32::from_gray(160) } else { egui::Color32::from_gray(80) })); }
                    ui.add_space(16.0);
                    let field_width = 340.0; let field_height = 34.0; let btn_zone = 32.0;
                    let (outer_rect, _) = ui.allocate_exact_size(egui::Vec2::new(field_width, field_height), egui::Sense::hover());
                    let visuals = ui.visuals();
                    ui.painter().rect(outer_rect, egui::CornerRadius::ZERO, visuals.widgets.inactive.bg_fill, visuals.widgets.active.bg_stroke, egui::StrokeKind::Outside);
                    let text_rect = egui::Rect::from_min_max(outer_rect.min, egui::Pos2::new(outer_rect.max.x - btn_zone, outer_rect.max.y));
                    let mut text_child = ui.new_child(egui::UiBuilder::new().max_rect(text_rect.shrink2(egui::vec2(8.0, 0.0))));
                    let pw_resp = text_child.add(egui::TextEdit::singleline(&mut prompt.password_buf).password(!prompt.show_password).frame(false).margin(egui::Margin { left: 0, right: 0, top: 8, bottom: 4 }).font(egui::TextStyle::Body).desired_width(text_rect.width() - 16.0).hint_text("Password"));
                    if prompt.focus_password { pw_resp.request_focus(); prompt.focus_password = false; }
                    let enter_pressed = pw_resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    let btn_rect = egui::Rect::from_min_max(egui::Pos2::new(outer_rect.max.x - btn_zone, outer_rect.min.y), outer_rect.max).shrink(4.0);
                    let dark_mode = ui.visuals().dark_mode;
                    let tooltip = if prompt.show_password { "Hide password" } else { "Show password" };
                    let icon_size = egui::vec2(16.0, 16.0);
                    let icon_pos = egui::Pos2::new(btn_rect.center().x - 8.0, btn_rect.center().y - 8.0);
                    let icon_rect = egui::Rect::from_min_size(icon_pos, icon_size);
                    let eye_resp = ui.allocate_rect(icon_rect, egui::Sense::click());
                    if let Some(img) = icon_cache.and_then(|ic| ic.themed_image(crate::icons::Icon::Eye, dark_mode)) { img.fit_to_exact_size(icon_size).paint_at(ui, icon_rect); }
                    if eye_resp.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(tooltip).clicked() { prompt.show_password = !prompt.show_password; }
                    if enter_pressed && !prompt.password_buf.is_empty() { action = ConnectingAction::SubmitPassword(prompt.password_buf.clone()); }
                    ui.add_space(8.0);
                    let cancel_text = egui::RichText::new("Cancel").size(13.0).color(if ui.visuals().dark_mode { egui::Color32::from_gray(140) } else { egui::Color32::from_gray(100) });
                    if ui.add(egui::Label::new(cancel_text).sense(egui::Sense::click())).clicked() { action = ConnectingAction::Reject; }
                });
            });
        }
        return action;
    }

    let heading = format!("Connecting to {title}\u{2026}");
    let heading_galley = ui.painter().layout_no_wrap(heading, egui::FontId::new(28.0, egui::FontFamily::Proportional), if ui.visuals().dark_mode { egui::Color32::WHITE } else { egui::Color32::BLACK });
    ui.painter().galley(egui::Pos2::new(center.x - heading_galley.size().x / 2.0, center.y - 40.0), heading_galley, egui::Color32::PLACEHOLDER);
    if let Some(detail) = detail {
        let dg = ui.painter().layout_no_wrap(detail.to_string(), egui::FontId::new(16.0, egui::FontFamily::Proportional), if ui.visuals().dark_mode { egui::Color32::from_gray(200) } else { egui::Color32::from_gray(40) });
        ui.painter().galley(egui::Pos2::new(center.x - dg.size().x / 2.0, center.y + 5.0), dg, egui::Color32::PLACEHOLDER);
    }
    let bar_w = 400.0_f32.min(rect.width() * 0.6); let bar_h = 6.0; let bar_y = center.y + 50.0;
    let bar_rect = egui::Rect::from_min_size(egui::Pos2::new(center.x - bar_w / 2.0, bar_y), egui::Vec2::new(bar_w, bar_h));
    ui.painter().rect_filled(bar_rect, bar_h / 2.0, if ui.visuals().dark_mode { egui::Color32::from_gray(60) } else { egui::Color32::from_gray(210) });
    let elapsed = started.map(|s| s.elapsed().as_secs_f32()).unwrap_or(0.0);
    let t = (elapsed % 1.8) / 1.8; let pos_t = if t < 0.5 { t * 2.0 } else { 2.0 - t * 2.0 }; let eased = pos_t * pos_t * (3.0 - 2.0 * pos_t);
    let iw = bar_w * 0.15; let ix = bar_rect.min.x + eased * (bar_w - iw);
    ui.painter().rect_filled(egui::Rect::from_min_size(egui::Pos2::new(ix, bar_y), egui::Vec2::new(iw, bar_h)), bar_h / 2.0, egui::Color32::from_rgb(66, 133, 244));
    ConnectingAction::None
}

pub(crate) fn show_error_screen(ui: &mut egui::Ui, title: &str, error: &str) -> bool {
    let rect = ui.available_rect_before_wrap();
    ui.painter().rect_filled(rect, 0.0, if ui.visuals().dark_mode { egui::Color32::from_gray(30) } else { egui::Color32::from_gray(241) });
    let center = rect.center();
    let content_rect = egui::Rect::from_center_size(center, egui::Vec2::new((rect.width() * 0.7).min(600.0), rect.height() * 0.8));
    let mut close = false;
    ui.allocate_new_ui(egui::UiBuilder::new().max_rect(content_rect), |ui| {
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.vertical_centered(|ui| { ui.add_space(20.0); ui.label(egui::RichText::new(format!("Connection to {title} failed")).size(24.0).color(egui::Color32::from_rgb(220, 50, 50))); ui.add_space(16.0); });
            ui.label(egui::RichText::new(error).size(13.0).family(egui::FontFamily::Monospace).color(if ui.visuals().dark_mode { egui::Color32::from_gray(180) } else { egui::Color32::from_gray(60) }));
            ui.add_space(16.0);
            ui.vertical_centered(|ui| { if ui.button("Close Tab").clicked() { close = true; } });
            ui.add_space(12.0);
        });
    });
    close
}

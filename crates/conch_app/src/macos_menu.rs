//! Native macOS menu bar using NSMenu/NSMenuItem via objc2.
//!
//! Replaces the in-window egui menu on macOS so the app feels native.
//! Menu actions are communicated back via a global channel that
//! the app polls each frame.

use std::sync::{LazyLock, Mutex};

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, Sel};
use objc2::{define_class, msg_send, sel, AnyThread, MainThreadOnly};
use objc2_app_kit::{NSApplication, NSMenu, NSMenuItem};
use objc2_foundation::{MainThreadMarker, NSString};

/// Actions that can be triggered from the native menu bar.
#[derive(Debug, Clone, PartialEq)]
pub enum MenuAction {
    NewConnection,
    NewWindow,
    NewLocalTerminal,
    NewSshSession,
    SshTunnels,
    ToggleLeftSidebar,
    ToggleRightSidebar,
    Preferences,
    AboutConch,
    RunPlugin(usize),
}

/// Global channel for menu actions.
static MENU_ACTIONS: LazyLock<Mutex<Vec<MenuAction>>> =
    LazyLock::new(|| Mutex::new(Vec::new()));

fn push_action(action: MenuAction) {
    if let Ok(mut v) = MENU_ACTIONS.lock() {
        v.push(action);
    }
}

pub fn drain_actions() -> Vec<MenuAction> {
    MENU_ACTIONS
        .lock()
        .map(|mut v| std::mem::take(&mut *v))
        .unwrap_or_default()
}

// ── ObjC responder class ──

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "ConchMenuResponder"]
    #[ivars = ()]
    struct MenuResponder;

    impl MenuResponder {
        #[unsafe(method(newConnection:))]
        fn new_connection(&self, _sender: *mut AnyObject) {
            push_action(MenuAction::NewConnection);
        }

        #[unsafe(method(newWindow:))]
        fn new_window(&self, _sender: *mut AnyObject) {
            push_action(MenuAction::NewWindow);
        }

        #[unsafe(method(newLocalTerminal:))]
        fn new_local_terminal(&self, _sender: *mut AnyObject) {
            push_action(MenuAction::NewLocalTerminal);
        }

        #[unsafe(method(newSshSession:))]
        fn new_ssh_session(&self, _sender: *mut AnyObject) {
            push_action(MenuAction::NewSshSession);
        }

        #[unsafe(method(sshTunnels:))]
        fn ssh_tunnels(&self, _sender: *mut AnyObject) {
            push_action(MenuAction::SshTunnels);
        }

        #[unsafe(method(toggleLeftSidebar:))]
        fn toggle_left_sidebar(&self, _sender: *mut AnyObject) {
            push_action(MenuAction::ToggleLeftSidebar);
        }

        #[unsafe(method(toggleRightSidebar:))]
        fn toggle_right_sidebar(&self, _sender: *mut AnyObject) {
            push_action(MenuAction::ToggleRightSidebar);
        }

        #[unsafe(method(openPreferences:))]
        fn open_preferences(&self, _sender: *mut AnyObject) {
            push_action(MenuAction::Preferences);
        }

        #[unsafe(method(aboutConch:))]
        fn about_conch(&self, _sender: *mut AnyObject) {
            push_action(MenuAction::AboutConch);
        }

        #[unsafe(method(runPlugin:))]
        fn run_plugin(&self, sender: *mut AnyObject) {
            let tag: isize = unsafe { msg_send![sender, tag] };
            push_action(MenuAction::RunPlugin(tag as usize));
        }
    }
);

impl MenuResponder {
    fn create() -> Retained<Self> {
        let this = Self::alloc().set_ivars(());
        unsafe { msg_send![super(this), init] }
    }
}

/// Global responder — must stay alive for the app's lifetime.
static RESPONDER: LazyLock<Mutex<Option<Retained<MenuResponder>>>> =
    LazyLock::new(|| Mutex::new(None));

/// Set up the native macOS menu bar. Call once at startup.
pub fn setup_menu_bar(plugin_names: &[String]) {
    let mtm = MainThreadMarker::new().expect("setup_menu_bar must be called from the main thread");
    let responder = MenuResponder::create();

    unsafe {
        let app = NSApplication::sharedApplication(mtm);
        let main_menu = NSMenu::new(mtm);

        // ── App menu (Conch) ──
        let app_menu = NSMenu::new(mtm);
        app_menu.addItem(&make_item(mtm, "About Conch", sel!(aboutConch:), "", &responder));
        app_menu.addItem(&NSMenuItem::separatorItem(mtm));
        app_menu.addItem(&make_item(mtm, "Preferences...", sel!(openPreferences:), ",", &responder));
        app_menu.addItem(&NSMenuItem::separatorItem(mtm));
        app_menu.addItem(&make_item_no_target(mtm, "Quit Conch", sel!(terminate:), "q"));
        let app_item = NSMenuItem::new(mtm);
        app_item.setSubmenu(Some(&app_menu));
        main_menu.addItem(&app_item);

        // ── File ──
        let file_menu = make_menu(mtm, "File");
        file_menu.addItem(&make_item(mtm, "New Window", sel!(newWindow:), "N", &responder));
        file_menu.addItem(&make_item(mtm, "New Connection...", sel!(newConnection:), "n", &responder));
        let file_item = NSMenuItem::new(mtm);
        file_item.setSubmenu(Some(&file_menu));
        main_menu.addItem(&file_item);

        // ── Sessions ──
        let sessions_menu = make_menu(mtm, "Sessions");
        sessions_menu.addItem(&make_item(mtm, "New Local Terminal", sel!(newLocalTerminal:), "t", &responder));
        sessions_menu.addItem(&make_item(mtm, "New SSH Session...", sel!(newSshSession:), "n", &responder));
        let sessions_item = NSMenuItem::new(mtm);
        sessions_item.setSubmenu(Some(&sessions_menu));
        main_menu.addItem(&sessions_item);

        // ── Tools ──
        let tools_menu = make_menu(mtm, "Tools");
        tools_menu.addItem(&make_item(mtm, "SSH Tunnels...", sel!(sshTunnels:), "", &responder));
        if !plugin_names.is_empty() {
            tools_menu.addItem(&NSMenuItem::separatorItem(mtm));
            for (i, name) in plugin_names.iter().enumerate() {
                let item = make_item(mtm, name, sel!(runPlugin:), "", &responder);
                item.setTag(i as isize);
                tools_menu.addItem(&item);
            }
        }
        let tools_item = NSMenuItem::new(mtm);
        tools_item.setSubmenu(Some(&tools_menu));
        main_menu.addItem(&tools_item);

        // ── View ──
        let view_menu = make_menu(mtm, "View");
        view_menu.addItem(&make_item(mtm, "Toggle Left Toolbar", sel!(toggleLeftSidebar:), "", &responder));
        view_menu.addItem(&make_item(mtm, "Toggle Right Toolbar", sel!(toggleRightSidebar:), "", &responder));
        let view_item = NSMenuItem::new(mtm);
        view_item.setSubmenu(Some(&view_menu));
        main_menu.addItem(&view_item);

        // ── Help ──
        let help_menu = make_menu(mtm, "Help");
        help_menu.addItem(&make_item(mtm, "About Conch", sel!(aboutConch:), "", &responder));
        let help_item = NSMenuItem::new(mtm);
        help_item.setSubmenu(Some(&help_menu));
        main_menu.addItem(&help_item);

        app.setMainMenu(Some(&main_menu));
    }

    // Keep responder alive.
    *RESPONDER.lock().unwrap() = Some(responder);
}

unsafe fn make_menu(mtm: MainThreadMarker, title: &str) -> Retained<NSMenu> {
    let ns_title = NSString::from_str(title);
    NSMenu::initWithTitle(NSMenu::alloc(mtm), &ns_title)
}

unsafe fn make_item(
    mtm: MainThreadMarker,
    title: &str,
    action: Sel,
    key_equiv: &str,
    target: &MenuResponder,
) -> Retained<NSMenuItem> {
    let ns_title = NSString::from_str(title);
    let ns_key = NSString::from_str(key_equiv);
    let item = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &ns_title,
            Some(action),
            &ns_key,
        )
    };
    let target_ptr: *const MenuResponder = target;
    let _: () = msg_send![&*item, setTarget: target_ptr];
    item
}

unsafe fn make_item_no_target(
    mtm: MainThreadMarker,
    title: &str,
    action: Sel,
    key_equiv: &str,
) -> Retained<NSMenuItem> {
    let ns_title = NSString::from_str(title);
    let ns_key = NSString::from_str(key_equiv);
    unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &ns_title,
            Some(action),
            &ns_key,
        )
    }
}

/// Make the title bar transparent so app content shows through it.
/// Call once after the window has been created.
pub fn set_titlebar_transparent() {
    let mtm = MainThreadMarker::new()
        .expect("set_titlebar_transparent must be called from the main thread");
    let app = NSApplication::sharedApplication(mtm);
    let windows = app.windows();
    for window in windows.iter() {
        window.setTitlebarAppearsTransparent(true);
    }
}

/// Set the tabbing identifier on all windows so macOS groups them together
/// in the native tab bar (Window > Merge All Windows).
pub fn set_tabbing_identifier(identifier: &str) {
    let mtm = MainThreadMarker::new()
        .expect("set_tabbing_identifier must be called from the main thread");
    let app = NSApplication::sharedApplication(mtm);
    let windows = app.windows();
    let ns_id = NSString::from_str(identifier);
    for window in windows.iter() {
        window.setTabbingIdentifier(&ns_id);
    }
}

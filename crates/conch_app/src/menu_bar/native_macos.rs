//! Native macOS menu bar using NSMenu/NSMenuItem via objc2.
//!
//! Installs a global menu bar so the app feels native on macOS.
//! Menu actions are communicated back via a global channel that
//! the app polls each frame.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex};

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, Sel};
use objc2::{define_class, msg_send, sel, MainThreadOnly};
use objc2_app_kit::{NSApplication, NSMenu, NSMenuItem};
use objc2_foundation::{MainThreadMarker, NSString};

use super::MenuAction;
use crate::host::bridge;

/// Global channel for menu actions.
static MENU_ACTIONS: LazyLock<Mutex<Vec<MenuAction>>> =
    LazyLock::new(|| Mutex::new(Vec::new()));

fn push_action(action: MenuAction) {
    if let Ok(mut v) = MENU_ACTIONS.lock() {
        v.push(action);
    }
}

pub fn drain_actions() -> Vec<MenuAction> {
    // Check if plugin menu items have changed and refresh if needed.
    refresh_plugin_menus_if_needed();

    MENU_ACTIONS
        .lock()
        .map(|mut v| std::mem::take(&mut *v))
        .unwrap_or_default()
}

// ── Plugin menu tag map ──

/// Maps NSMenuItem tag values to (plugin_name, action) pairs.
static PLUGIN_TAG_MAP: LazyLock<Mutex<Vec<(isize, String, String)>>> =
    LazyLock::new(|| Mutex::new(Vec::new()));

/// Tracks which version of plugin menu items we've installed.
static INSTALLED_MENU_VERSION: AtomicU64 = AtomicU64::new(0);

/// Next tag value for plugin menu items (start at 1000 to avoid collisions).
static NEXT_PLUGIN_TAG: AtomicU64 = AtomicU64::new(1000);

// ── ObjC responder class ──

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "ConchMenuResponder"]
    #[ivars = ()]
    struct MenuResponder;

    impl MenuResponder {
        #[unsafe(method(newTab:))]
        fn new_tab(&self, _sender: *mut AnyObject) {
            push_action(MenuAction::NewTab);
        }

        #[unsafe(method(newWindow:))]
        fn new_window(&self, _sender: *mut AnyObject) {
            push_action(MenuAction::NewWindow);
        }

        #[unsafe(method(closeTab:))]
        fn close_tab(&self, _sender: *mut AnyObject) {
            push_action(MenuAction::CloseTab);
        }

        #[unsafe(method(doCopy:))]
        fn do_copy(&self, _sender: *mut AnyObject) {
            push_action(MenuAction::Copy);
        }

        #[unsafe(method(doPaste:))]
        fn do_paste(&self, _sender: *mut AnyObject) {
            push_action(MenuAction::Paste);
        }

        #[unsafe(method(selectAll:))]
        fn select_all(&self, _sender: *mut AnyObject) {
            push_action(MenuAction::SelectAll);
        }

        #[unsafe(method(zenMode:))]
        fn zen_mode(&self, _sender: *mut AnyObject) {
            push_action(MenuAction::ZenMode);
        }

        #[unsafe(method(zoomIn:))]
        fn zoom_in(&self, _sender: *mut AnyObject) {
            push_action(MenuAction::ZoomIn);
        }

        #[unsafe(method(zoomOut:))]
        fn zoom_out(&self, _sender: *mut AnyObject) {
            push_action(MenuAction::ZoomOut);
        }

        #[unsafe(method(zoomReset:))]
        fn zoom_reset(&self, _sender: *mut AnyObject) {
            push_action(MenuAction::ZoomReset);
        }

        #[unsafe(method(pluginMenuAction:))]
        fn plugin_menu_action(&self, sender: *mut AnyObject) {
            if sender.is_null() {
                return;
            }
            let tag: isize = unsafe { msg_send![sender, tag] };
            if let Ok(map) = PLUGIN_TAG_MAP.lock() {
                if let Some((_, plugin_name, action)) = map.iter().find(|(t, _, _)| *t == tag) {
                    push_action(MenuAction::PluginAction {
                        plugin_name: plugin_name.clone(),
                        action: action.clone(),
                    });
                }
            }
        }
    }
);

impl MenuResponder {
    fn create(mtm: MainThreadMarker) -> Retained<Self> {
        let this = mtm.alloc::<Self>().set_ivars(());
        unsafe { msg_send![super(this), init] }
    }
}

/// Global responder — must stay alive for the app's lifetime.
static RESPONDER: LazyLock<Mutex<Option<Retained<MenuResponder>>>> =
    LazyLock::new(|| Mutex::new(None));

/// Set up the native macOS menu bar. Call once from the main thread.
pub fn setup_menu_bar() {
    let mtm = MainThreadMarker::new()
        .expect("setup_menu_bar must be called from the main thread");
    let responder = MenuResponder::create(mtm);

    unsafe {
        let app = NSApplication::sharedApplication(mtm);
        let main_menu = NSMenu::new(mtm);

        // ── App menu (Conch) ──
        let app_menu = NSMenu::new(mtm);
        app_menu.addItem(&make_item_no_target(mtm, "About Conch", sel!(orderFrontStandardAboutPanel:), ""));
        app_menu.addItem(&NSMenuItem::separatorItem(mtm));
        app_menu.addItem(&make_item_no_target(mtm, "Quit Conch", sel!(terminate:), "q"));
        let app_item = NSMenuItem::new(mtm);
        app_item.setSubmenu(Some(&app_menu));
        main_menu.addItem(&app_item);

        // ── File ──
        let file_menu = make_menu(mtm, "File");
        file_menu.addItem(&make_item(mtm, "New Tab", sel!(newTab:), "t", &responder));
        file_menu.addItem(&make_item(mtm, "New Window", sel!(newWindow:), "N", &responder));
        file_menu.addItem(&NSMenuItem::separatorItem(mtm));
        file_menu.addItem(&make_item(mtm, "Close Tab", sel!(closeTab:), "w", &responder));
        let file_item = NSMenuItem::new(mtm);
        file_item.setSubmenu(Some(&file_menu));
        main_menu.addItem(&file_item);

        // ── Edit ──
        let edit_menu = make_menu(mtm, "Edit");
        edit_menu.addItem(&make_item(mtm, "Copy", sel!(doCopy:), "c", &responder));
        edit_menu.addItem(&make_item(mtm, "Paste", sel!(doPaste:), "v", &responder));
        edit_menu.addItem(&NSMenuItem::separatorItem(mtm));
        edit_menu.addItem(&make_item(mtm, "Select All", sel!(selectAll:), "a", &responder));
        let edit_item = NSMenuItem::new(mtm);
        edit_item.setSubmenu(Some(&edit_menu));
        main_menu.addItem(&edit_item);

        // ── View ──
        let view_menu = make_menu(mtm, "View");
        view_menu.addItem(&make_item(mtm, "Zen Mode", sel!(zenMode:), "", &responder));
        view_menu.addItem(&NSMenuItem::separatorItem(mtm));
        view_menu.addItem(&make_item(mtm, "Zoom In", sel!(zoomIn:), "+", &responder));
        view_menu.addItem(&make_item(mtm, "Zoom Out", sel!(zoomOut:), "-", &responder));
        view_menu.addItem(&make_item(mtm, "Reset Zoom", sel!(zoomReset:), "0", &responder));
        let view_item = NSMenuItem::new(mtm);
        view_item.setSubmenu(Some(&view_menu));
        main_menu.addItem(&view_item);

        // ── Help ──
        let help_menu = make_menu(mtm, "Help");
        help_menu.addItem(&make_item_no_target(mtm, "About Conch", sel!(orderFrontStandardAboutPanel:), ""));
        let help_item = NSMenuItem::new(mtm);
        help_item.setSubmenu(Some(&help_menu));
        main_menu.addItem(&help_item);

        app.setMainMenu(Some(&main_menu));
    }

    // Keep responder alive.
    *RESPONDER.lock().unwrap() = Some(responder);

    // Install any plugin menu items that were registered before setup.
    refresh_plugin_menus_if_needed();
}

/// Check if plugin menu items have changed and update the native menu bar.
fn refresh_plugin_menus_if_needed() {
    let current_version = bridge::plugin_menu_items_version();
    let installed = INSTALLED_MENU_VERSION.load(Ordering::Relaxed);
    if current_version == installed {
        return;
    }

    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };

    let responder_guard = RESPONDER.lock().unwrap();
    let Some(responder) = responder_guard.as_ref() else {
        return;
    };

    let plugin_items = bridge::plugin_menu_items();

    // Clear old plugin tag mappings.
    if let Ok(mut map) = PLUGIN_TAG_MAP.lock() {
        map.clear();
    }

    unsafe {
        let app = NSApplication::sharedApplication(mtm);
        let Some(main_menu) = app.mainMenu() else {
            return;
        };

        // Group plugin items by menu name.
        let mut grouped = std::collections::BTreeMap::<&str, Vec<&bridge::PluginMenuItem>>::new();
        for item in &plugin_items {
            grouped.entry(&item.menu).or_default().push(item);
        }

        let standard_menus = ["File", "Edit", "View", "Help"];

        // Remove any previously added custom plugin menus (tagged with tag >= 900).
        let count = main_menu.numberOfItems();
        let mut to_remove = Vec::new();
        for i in 0..count {
            if let Some(item) = main_menu.itemAtIndex(i) {
                let tag: isize = msg_send![&*item, tag];
                if tag >= 900 {
                    to_remove.push(i);
                }
            }
        }
        // Remove in reverse order to keep indices stable.
        for &i in to_remove.iter().rev() {
            main_menu.removeItemAtIndex(i);
        }

        // For standard menus, find them and append plugin items.
        for (menu_name, items) in &grouped {
            if standard_menus.contains(menu_name) {
                // Find the existing submenu by title.
                if let Some(submenu) = find_submenu_by_title(&main_menu, menu_name) {
                    // Remove old plugin items (identified by tag >= 1000).
                    let sub_count = submenu.numberOfItems();
                    let mut sub_remove = Vec::new();
                    for i in 0..sub_count {
                        if let Some(item) = submenu.itemAtIndex(i) {
                            let tag: isize = msg_send![&*item, tag];
                            if tag >= 1000 {
                                sub_remove.push(i);
                            }
                        }
                    }
                    for &i in sub_remove.iter().rev() {
                        submenu.removeItemAtIndex(i);
                    }

                    // Add separator + plugin items.
                    if !items.is_empty() {
                        submenu.addItem(&NSMenuItem::separatorItem(mtm));
                        for item in items {
                            let tag = NEXT_PLUGIN_TAG.fetch_add(1, Ordering::Relaxed) as isize;
                            let ns_item = make_item(
                                mtm,
                                &item.label,
                                sel!(pluginMenuAction:),
                                "",
                                responder,
                            );
                            let _: () = msg_send![&*ns_item, setTag: tag];
                            submenu.addItem(&ns_item);
                            if let Ok(mut map) = PLUGIN_TAG_MAP.lock() {
                                map.push((tag, item.plugin_name.clone(), item.action.clone()));
                            }
                        }
                    }
                }
            } else if menu_name.starts_with('_') {
                // Hidden menu — keybinding-only items, skip rendering.
            } else {
                // Custom menu — create a new menu button.
                let custom_menu = make_menu(mtm, menu_name);
                for item in items {
                    let tag = NEXT_PLUGIN_TAG.fetch_add(1, Ordering::Relaxed) as isize;
                    let ns_item = make_item(
                        mtm,
                        &item.label,
                        sel!(pluginMenuAction:),
                        "",
                        responder,
                    );
                    let _: () = msg_send![&*ns_item, setTag: tag];
                    custom_menu.addItem(&ns_item);
                    if let Ok(mut map) = PLUGIN_TAG_MAP.lock() {
                        map.push((tag, item.plugin_name.clone(), item.action.clone()));
                    }
                }
                let custom_item = NSMenuItem::new(mtm);
                custom_item.setSubmenu(Some(&custom_menu));
                // Tag with 900 so we can identify and remove on refresh.
                let _: () = msg_send![&*custom_item, setTag: 900_isize];
                // Insert before Help (last menu item).
                let insert_idx = main_menu.numberOfItems() - 1;
                main_menu.insertItem_atIndex(&custom_item, insert_idx);
            }
        }
    }

    INSTALLED_MENU_VERSION.store(current_version, Ordering::Relaxed);
}

/// Find a submenu by its title in the main menu bar.
unsafe fn find_submenu_by_title(main_menu: &NSMenu, title: &str) -> Option<Retained<NSMenu>> {
    let count = main_menu.numberOfItems();
    for i in 0..count {
        if let Some(item) = main_menu.itemAtIndex(i) {
            if let Some(submenu) = item.submenu() {
                if submenu.title().to_string() == title {
                    return Some(submenu);
                }
            }
        }
    }
    None
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

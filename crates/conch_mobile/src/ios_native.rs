//! Native iOS integration — UITabBar overlay on the WKWebView.
//!
//! Creates a native UITabBar at the bottom of the screen and sends
//! `tab-changed` events to the webview when tabs are tapped.
//! The webview's HTML tab bar is removed; navigation is fully native.

use std::ffi::c_void;
use std::sync::OnceLock;

use objc2::encode::{Encode, Encoding};
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, Bool, Sel};
use objc2::{class, msg_send, sel};

use tauri::{Emitter, WebviewWindow};

/// Height of the native tab bar (points).
const TAB_BAR_HEIGHT: f64 = 49.0;

/// Store the Tauri window handle so the delegate can emit events.
static WEBVIEW_WINDOW: OnceLock<WebviewWindow> = OnceLock::new();

/// Tab identifiers — must match the JS tab IDs.
const TAB_IDS: [&str; 3] = ["vault", "connections", "profile"];

// ---------------------------------------------------------------------------
// CoreGraphics / UIKit struct types with Encode impls
// ---------------------------------------------------------------------------

#[repr(C)]
#[derive(Debug, Copy, Clone)]
struct CGPoint {
    x: f64,
    y: f64,
}

unsafe impl Encode for CGPoint {
    const ENCODING: Encoding = Encoding::Struct("CGPoint", &[f64::ENCODING, f64::ENCODING]);
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
struct CGSize {
    width: f64,
    height: f64,
}

unsafe impl Encode for CGSize {
    const ENCODING: Encoding = Encoding::Struct("CGSize", &[f64::ENCODING, f64::ENCODING]);
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
struct CGRect {
    origin: CGPoint,
    size: CGSize,
}

unsafe impl Encode for CGRect {
    const ENCODING: Encoding =
        Encoding::Struct("CGRect", &[CGPoint::ENCODING, CGSize::ENCODING]);
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
struct UIEdgeInsets {
    top: f64,
    left: f64,
    bottom: f64,
    right: f64,
}

unsafe impl Encode for UIEdgeInsets {
    const ENCODING: Encoding = Encoding::Struct(
        "UIEdgeInsets",
        &[f64::ENCODING, f64::ENCODING, f64::ENCODING, f64::ENCODING],
    );
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Set up a native UITabBar overlaying the bottom of the WKWebView.
pub fn setup_native_tab_bar(webview: &WebviewWindow) {
    WEBVIEW_WINDOW.set(webview.clone()).ok();

    let _ = webview.with_webview(|wv| {
        // SAFETY: We are on the main thread (Tauri setup), accessing UIKit
        // views through their raw pointers obtained from WKWebView.
        unsafe {
            let wk_webview = wv.inner() as *mut AnyObject;

            // Get the WKWebView's superview (the main UIView container)
            let superview: *mut AnyObject = msg_send![wk_webview, superview];
            if superview.is_null() {
                log::error!("[ios_native] WKWebView has no superview");
                return;
            }

            // Fix edge-to-edge: set scroll view contentInsetAdjustmentBehavior = .never
            let scroll_view: *mut AnyObject = msg_send![wk_webview, scrollView];
            let _: () = msg_send![scroll_view, setContentInsetAdjustmentBehavior: 2isize];

            // Get screen bounds for sizing
            let screen: *mut AnyObject = msg_send![class!(UIScreen), mainScreen];
            let bounds: CGRect = msg_send![screen, bounds];

            // Get the safe area bottom inset from the superview's window
            let window: *mut AnyObject = msg_send![superview, window];
            let safe_insets: UIEdgeInsets = msg_send![window, safeAreaInsets];
            let bottom_inset = safe_insets.bottom;

            // Total tab bar height including safe area
            let total_tab_height = TAB_BAR_HEIGHT + bottom_inset;

            // Create the UITabBar
            let tab_bar_frame = CGRect {
                origin: CGPoint {
                    x: 0.0,
                    y: bounds.size.height - total_tab_height,
                },
                size: CGSize {
                    width: bounds.size.width,
                    height: total_tab_height,
                },
            };

            let tab_bar: *mut AnyObject = msg_send![class!(UITabBar), alloc];
            let tab_bar: *mut AnyObject = msg_send![tab_bar, initWithFrame: tab_bar_frame];

            // Style the tab bar with Dracula colors
            let _: () = msg_send![tab_bar, setTranslucent: Bool::NO];

            // Background color: --tab-bg (#21222c)
            let bg_color = color_from_hex(0x21, 0x22, 0x2C, 1.0);
            let bar_appearance: *mut AnyObject = msg_send![class!(UITabBarAppearance), alloc];
            let bar_appearance: *mut AnyObject = msg_send![bar_appearance, init];
            let _: () = msg_send![bar_appearance, configureWithOpaqueBackground];
            let _: () = msg_send![bar_appearance, setBackgroundColor: bg_color];

            let _: () = msg_send![tab_bar, setStandardAppearance: bar_appearance];
            let _: () = msg_send![tab_bar, setScrollEdgeAppearance: bar_appearance];

            // Set tint colors
            // Selected: --purple (#bd93f9)
            let selected_color = color_from_hex(0xBD, 0x93, 0xF9, 1.0);
            // Normal: --fg-dim (#6272a4)
            let normal_color = color_from_hex(0x62, 0x72, 0xA4, 1.0);
            let _: () = msg_send![tab_bar, setTintColor: selected_color];
            let _: () = msg_send![tab_bar, setUnselectedItemTintColor: normal_color];

            // Create tab bar items
            let items = create_tab_items();
            let _: () = msg_send![tab_bar, setItems: &*items, animated: Bool::NO];

            // Set the default selected item (connections = index 1)
            let selected: *mut AnyObject = msg_send![&*items, objectAtIndex: 1usize];
            let _: () = msg_send![tab_bar, setSelectedItem: selected];

            // Register our delegate class and set it
            let delegate = create_tab_bar_delegate();
            let _: () = msg_send![tab_bar, setDelegate: delegate];

            // Keep webview full-screen — tab bar floats on top.
            // The webview JS adds bottom padding via the native-tab-bar-ready event.
            // No setFrame needed — webview already fills the screen.

            // Add the tab bar on top of the webview
            let _: () = msg_send![superview, addSubview: tab_bar];

            log::info!(
                "[ios_native] Native tab bar installed (height={total_tab_height}, safe_bottom={bottom_inset})"
            );
        }
    });

    // Emit initial tab and height info to webview
    let _ = webview.emit("native-tab-bar-ready", TAB_BAR_HEIGHT);
    let _ = webview.emit("tab-changed", "connections");
}

// ---------------------------------------------------------------------------
// UITabBar items
// ---------------------------------------------------------------------------

unsafe fn create_tab_items() -> Retained<AnyObject> {
    let vault_item = create_tab_item("Vault", "lock.fill", 0);
    let conn_item = create_tab_item("Connect", "terminal.fill", 1);
    let profile_item = create_tab_item("Profile", "person.fill", 2);

    // Create NSArray with the 3 items
    let items: [*mut AnyObject; 3] = [vault_item, conn_item, profile_item];
    let arr: Retained<AnyObject> = msg_send![
        class!(NSArray),
        arrayWithObjects: items.as_ptr(),
        count: 3usize
    ];
    arr
}

unsafe fn create_tab_item(title: &str, system_icon: &str, tag: isize) -> *mut AnyObject {
    let title_ns = nsstring(title);
    let icon_name = nsstring(system_icon);

    // Create SF Symbol image
    let image: *mut AnyObject =
        msg_send![class!(UIImage), systemImageNamed: &*icon_name];

    let item: *mut AnyObject = msg_send![class!(UITabBarItem), alloc];
    let item: *mut AnyObject =
        msg_send![item, initWithTitle: &*title_ns, image: image, tag: tag];
    item
}

// ---------------------------------------------------------------------------
// UITabBarDelegate
// ---------------------------------------------------------------------------

unsafe fn create_tab_bar_delegate() -> *mut AnyObject {
    static REGISTERED: OnceLock<()> = OnceLock::new();

    REGISTERED.get_or_init(|| {
        // SAFETY: Registering an ObjC class — must happen once.
        unsafe {
            let superclass = class!(NSObject);
            let mut cls = objc2::runtime::ClassBuilder::new(c"ConchTabBarDelegate", superclass)
                .expect("Failed to create ConchTabBarDelegate class");

            // Add UITabBarDelegate protocol
            if let Some(protocol) = objc2::runtime::AnyProtocol::get(c"UITabBarDelegate") {
                cls.add_protocol(protocol);
            }

            // Implement tabBar:didSelectItem:
            unsafe extern "C-unwind" fn did_select_item(
                _this: *mut AnyObject,
                _sel: Sel,
                _tab_bar: *mut AnyObject,
                item: *mut AnyObject,
            ) {
                // SAFETY: item is a valid UITabBarItem, tag returns isize.
                let tag: isize = unsafe { msg_send![item, tag] };
                if let Some(tab_id) = TAB_IDS.get(tag as usize) {
                    if let Some(window) = WEBVIEW_WINDOW.get() {
                        let _ = window.emit("tab-changed", *tab_id);
                    }
                }
            }

            cls.add_method(
                sel!(tabBar:didSelectItem:),
                did_select_item as unsafe extern "C-unwind" fn(_, _, _, _),
            );

            cls.register();
        }
    });

    let cls = AnyClass::get(c"ConchTabBarDelegate").unwrap();
    let delegate: *mut AnyObject = msg_send![cls, alloc];
    let delegate: *mut AnyObject = msg_send![delegate, init];
    delegate
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

unsafe fn nsstring(s: &str) -> Retained<AnyObject> {
    let bytes = s.as_ptr();
    let len = s.len();
    msg_send![
        class!(NSString),
        stringWithBytes: bytes as *const c_void,
        length: len,
        encoding: 4usize // NSUTF8StringEncoding
    ]
}

unsafe fn color_from_hex(r: u8, g: u8, b: u8, a: f64) -> *mut AnyObject {
    msg_send![
        class!(UIColor),
        colorWithRed: r as f64 / 255.0,
        green: g as f64 / 255.0,
        blue: b as f64 / 255.0,
        alpha: a
    ]
}

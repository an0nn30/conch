//! Icon name constants for the built-in TermLab icon set.
//!
//! Plugins reference icons by string name in widget fields like
//! `TreeNode::icon`. These constants ensure plugins use valid icon names
//! from the shared set rather than inventing their own.
//!
//! All icons are 16x16 PNGs with theme-aware variants (dark/light) where
//! applicable. The host automatically selects the correct variant.

/// File icon.
pub const FILE: &str = "file";

/// Closed folder icon.
pub const FOLDER: &str = "folder";

/// Open folder icon.
pub const FOLDER_OPEN: &str = "folder-open";

/// Server/rack icon.
pub const SERVER: &str = "server";

/// Network server icon.
pub const NETWORK_SERVER: &str = "network-server";

/// Terminal/console icon.
pub const TERMINAL: &str = "terminal";

/// Sessions tab icon.
pub const TAB_SESSIONS: &str = "tab-sessions";

/// Files tab icon.
pub const TAB_FILES: &str = "tab-files";

/// Tools tab icon.
pub const TAB_TOOLS: &str = "tab-tools";

/// Macros tab icon.
pub const TAB_MACROS: &str = "tab-macros";

/// Down arrow icon.
pub const GO_DOWN: &str = "go-down";

/// Up arrow icon.
pub const GO_UP: &str = "go-up";

/// Home/house icon.
pub const GO_HOME: &str = "go-home";

/// Refresh/reload icon.
pub const REFRESH: &str = "refresh";

/// New folder icon.
pub const FOLDER_NEW: &str = "folder-new";

/// Sidebar folder icon.
pub const SIDEBAR_FOLDER: &str = "sidebar-folder";

/// Previous/back arrow icon.
pub const GO_PREVIOUS: &str = "go-previous";

/// Next/forward arrow icon.
pub const GO_NEXT: &str = "go-next";

/// Computer/monitor icon.
pub const COMPUTER: &str = "computer";

/// Close tab icon (X).
pub const TAB_CLOSE: &str = "tab-close";

/// Download/transfer-down icon.
pub const TRANSFER_DOWN: &str = "transfer-down";

/// Upload/transfer-up icon.
pub const TRANSFER_UP: &str = "transfer-up";

/// Locked padlock icon.
pub const LOCKED: &str = "locked";

/// Unlocked padlock icon.
pub const UNLOCKED: &str = "unlocked";

/// Eye/view icon (show/hide toggle).
pub const EYE: &str = "eye";

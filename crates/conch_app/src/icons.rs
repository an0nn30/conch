//! Compile-time embedded PNG icons with a texture cache for egui.

use std::collections::HashMap;

use egui::{ColorImage, Context, TextureHandle, TextureOptions};

// Embed PNGs at compile time.
const FILE_DARK_PNG: &[u8] = include_bytes!("../icons/file-dark.png");
const FILE_LIGHT_PNG: &[u8] = include_bytes!("../icons/file-light.png");
const FOLDER_PNG: &[u8] = include_bytes!("../icons/folder.png");
const FOLDER_OPEN_PNG: &[u8] = include_bytes!("../icons/folder-open.png");
const SERVER_PNG: &[u8] = include_bytes!("../icons/server.png");
const NETWORK_SERVER_PNG: &[u8] = include_bytes!("../icons/network-server.png");
const TERMINAL_PNG: &[u8] = include_bytes!("../icons/terminal.png");
const TAB_SESSIONS_DARK_PNG: &[u8] = include_bytes!("../icons/tab-sessions-dark.png");
const TAB_SESSIONS_LIGHT_PNG: &[u8] = include_bytes!("../icons/tab-sessions-light.png");
const TAB_FILES_PNG: &[u8] = include_bytes!("../icons/tab-files.png");
const TAB_TOOLS_PNG: &[u8] = include_bytes!("../icons/tab-tools.png");
const TAB_MACROS_PNG: &[u8] = include_bytes!("../icons/tab-macros.png");
const GO_DOWN_PNG: &[u8] = include_bytes!("../icons/go-down.png");

// Navigation icons — dark variant (for light themes) and light variant (for dark themes).
const GO_UP_DARK_PNG: &[u8] = include_bytes!("../icons/go-up-dark.png");
const GO_UP_LIGHT_PNG: &[u8] = include_bytes!("../icons/go-up-light.png");
const GO_HOME_DARK_PNG: &[u8] = include_bytes!("../icons/go-home-dark.png");
const GO_HOME_LIGHT_PNG: &[u8] = include_bytes!("../icons/go-home-light.png");
const REFRESH_DARK_PNG: &[u8] = include_bytes!("../icons/view-refresh-dark.png");
const REFRESH_LIGHT_PNG: &[u8] = include_bytes!("../icons/view-refresh-light.png");
const FOLDER_NEW_DARK_PNG: &[u8] = include_bytes!("../icons/folder-new-dark.png");
const FOLDER_NEW_LIGHT_PNG: &[u8] = include_bytes!("../icons/folder-new-light.png");
const SIDEBAR_FOLDER_DARK_PNG: &[u8] = include_bytes!("../icons/sidebar-folder-dark.png");
const SIDEBAR_FOLDER_LIGHT_PNG: &[u8] = include_bytes!("../icons/sidebar-folder-light.png");
const GO_PREVIOUS_DARK_PNG: &[u8] = include_bytes!("../icons/go-previous-dark.png");
const GO_PREVIOUS_LIGHT_PNG: &[u8] = include_bytes!("../icons/go-previous-light.png");
const GO_NEXT_DARK_PNG: &[u8] = include_bytes!("../icons/go-next-dark.png");
const GO_NEXT_LIGHT_PNG: &[u8] = include_bytes!("../icons/go-next-light.png");
const COMPUTER_DARK_PNG: &[u8] = include_bytes!("../icons/computer-dark.png");
const COMPUTER_LIGHT_PNG: &[u8] = include_bytes!("../icons/computer-light.png");
const TAB_CLOSE_DARK_PNG: &[u8] = include_bytes!("../icons/tab-close-dark.png");
const TAB_CLOSE_LIGHT_PNG: &[u8] = include_bytes!("../icons/tab-close-light.png");

/// Icons that have a single (color) variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Icon {
    File,
    Folder,
    FolderOpen,
    Server,
    NetworkServer,
    Terminal,
    TabSessions,
    TabFiles,
    TabTools,
    TabMacros,
    GoDown,
    // Themed icons (resolved at lookup time).
    GoUp,
    GoHome,
    Refresh,
    FolderNew,
    SidebarFolder,
    GoPrevious,
    GoNext,
    Computer,
    TabClose,
}

/// Internal key for the texture map — separates dark/light variants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum TexKey {
    Single(Icon),
    Dark(Icon),
    Light(Icon),
}

/// Which icons have dark/light variants.
const THEMED_ICONS: &[Icon] = &[Icon::File, Icon::TabSessions, Icon::TabClose, Icon::GoUp, Icon::GoHome, Icon::Refresh, Icon::FolderNew, Icon::SidebarFolder, Icon::GoPrevious, Icon::GoNext, Icon::Computer];

fn is_themed(icon: Icon) -> bool {
    matches!(icon, Icon::File | Icon::TabSessions | Icon::TabClose | Icon::GoUp | Icon::GoHome | Icon::Refresh | Icon::FolderNew | Icon::SidebarFolder | Icon::GoPrevious | Icon::GoNext | Icon::Computer)
}

fn single_bytes(icon: Icon) -> &'static [u8] {
    match icon {
        Icon::File => FILE_DARK_PNG,
        Icon::Computer => COMPUTER_DARK_PNG,
        Icon::Folder => FOLDER_PNG,
        Icon::FolderOpen => FOLDER_OPEN_PNG,
        Icon::Server => SERVER_PNG,
        Icon::NetworkServer => NETWORK_SERVER_PNG,
        Icon::Terminal => TERMINAL_PNG,
        Icon::TabSessions => TAB_SESSIONS_DARK_PNG,
        Icon::TabClose => TAB_CLOSE_DARK_PNG,
        Icon::TabFiles => TAB_FILES_PNG,
        Icon::TabTools => TAB_TOOLS_PNG,
        Icon::TabMacros => TAB_MACROS_PNG,
        Icon::GoDown => GO_DOWN_PNG,
        // Themed icons are handled separately; fallback to dark.
        Icon::GoUp => GO_UP_DARK_PNG,
        Icon::GoHome => GO_HOME_DARK_PNG,
        Icon::Refresh => REFRESH_DARK_PNG,
        Icon::FolderNew => FOLDER_NEW_DARK_PNG,
        Icon::SidebarFolder => SIDEBAR_FOLDER_DARK_PNG,
        Icon::GoPrevious => GO_PREVIOUS_DARK_PNG,
        Icon::GoNext => GO_NEXT_DARK_PNG,
    }
}

fn dark_bytes(icon: Icon) -> &'static [u8] {
    match icon {
        Icon::File => FILE_DARK_PNG,
        Icon::Computer => COMPUTER_DARK_PNG,
        Icon::TabSessions => TAB_SESSIONS_DARK_PNG,
        Icon::TabClose => TAB_CLOSE_DARK_PNG,
        Icon::GoUp => GO_UP_DARK_PNG,
        Icon::GoHome => GO_HOME_DARK_PNG,
        Icon::Refresh => REFRESH_DARK_PNG,
        Icon::FolderNew => FOLDER_NEW_DARK_PNG,
        Icon::SidebarFolder => SIDEBAR_FOLDER_DARK_PNG,
        Icon::GoPrevious => GO_PREVIOUS_DARK_PNG,
        Icon::GoNext => GO_NEXT_DARK_PNG,
        _ => unreachable!(),
    }
}

fn light_bytes(icon: Icon) -> &'static [u8] {
    match icon {
        Icon::File => FILE_LIGHT_PNG,
        Icon::Computer => COMPUTER_LIGHT_PNG,
        Icon::TabSessions => TAB_SESSIONS_LIGHT_PNG,
        Icon::TabClose => TAB_CLOSE_LIGHT_PNG,
        Icon::GoUp => GO_UP_LIGHT_PNG,
        Icon::GoHome => GO_HOME_LIGHT_PNG,
        Icon::Refresh => REFRESH_LIGHT_PNG,
        Icon::FolderNew => FOLDER_NEW_LIGHT_PNG,
        Icon::SidebarFolder => SIDEBAR_FOLDER_LIGHT_PNG,
        Icon::GoPrevious => GO_PREVIOUS_LIGHT_PNG,
        Icon::GoNext => GO_NEXT_LIGHT_PNG,
        _ => unreachable!(),
    }
}

fn tex_name(key: TexKey) -> String {
    match key {
        TexKey::Single(i) => format!("icon_{:?}", i),
        TexKey::Dark(i) => format!("icon_{:?}_dark", i),
        TexKey::Light(i) => format!("icon_{:?}_light", i),
    }
}

const ALL_SINGLE: &[Icon] = &[
    Icon::Folder,
    Icon::FolderOpen,
    Icon::Server,
    Icon::NetworkServer,
    Icon::Terminal,
    Icon::TabFiles,
    Icon::TabTools,
    Icon::TabMacros,
    Icon::GoDown,
];

/// Caches decoded PNG textures for use in egui widgets.
pub struct IconCache {
    textures: HashMap<TexKey, TextureHandle>,
}

impl IconCache {
    /// Decode all embedded PNGs and upload as egui textures.
    pub fn load(ctx: &Context) -> Self {
        let mut textures = HashMap::new();

        // Single-variant icons.
        for &icon in ALL_SINGLE {
            let key = TexKey::Single(icon);
            if let Some(h) = decode_and_upload(ctx, &tex_name(key), single_bytes(icon)) {
                textures.insert(key, h);
            }
        }

        // Themed navigation icons — both variants.
        for &icon in THEMED_ICONS {
            let dk = TexKey::Dark(icon);
            if let Some(h) = decode_and_upload(ctx, &tex_name(dk), dark_bytes(icon)) {
                textures.insert(dk, h);
            }
            let lk = TexKey::Light(icon);
            if let Some(h) = decode_and_upload(ctx, &tex_name(lk), light_bytes(icon)) {
                textures.insert(lk, h);
            }
        }

        IconCache { textures }
    }

    /// Get a sized `Image` for the given icon (16×16).
    /// For themed icons, automatically selects the variant matching the current theme.
    pub fn image(&self, icon: Icon) -> Option<egui::Image<'_>> {
        self.texture_handle(icon).map(|h| {
            egui::Image::new(egui::load::SizedTexture::new(h.id(), [16.0, 16.0]))
        })
    }

    /// Get a `TextureId` for painter-level drawing.
    /// For themed icons, uses the dark variant (assumes dark bg in tab strip).
    pub fn texture_id(&self, icon: Icon) -> Option<egui::TextureId> {
        // Tab strip always has a dark background, so always use light variant there.
        let key = if is_themed(icon) {
            TexKey::Light(icon)
        } else {
            TexKey::Single(icon)
        };
        self.textures.get(&key).map(|h| h.id())
    }

    /// Get the right `Image` for a themed icon given an explicit dark_mode flag.
    pub fn themed_image(&self, icon: Icon, dark_mode: bool) -> Option<egui::Image<'_>> {
        let key = if is_themed(icon) {
            if dark_mode { TexKey::Light(icon) } else { TexKey::Dark(icon) }
        } else {
            TexKey::Single(icon)
        };
        self.textures.get(&key).map(|h| {
            egui::Image::new(egui::load::SizedTexture::new(h.id(), [16.0, 16.0]))
        })
    }

    fn texture_handle(&self, icon: Icon) -> Option<&TextureHandle> {
        if is_themed(icon) {
            // Default: assume dark mode (most common for terminal apps).
            self.textures.get(&TexKey::Light(icon))
        } else {
            self.textures.get(&TexKey::Single(icon))
        }
    }
}

fn decode_and_upload(ctx: &Context, name: &str, bytes: &[u8]) -> Option<TextureHandle> {
    let img = image::load_from_memory(bytes).ok()?.into_rgba8();
    let (w, h) = img.dimensions();
    let pixels = img.into_raw();
    let color_image = ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &pixels);
    Some(ctx.load_texture(name, color_image, TextureOptions::LINEAR))
}

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;

/// Whether a plugin is a run-once action or a persistent panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PluginType {
    #[default]
    Action,
    Panel,
    BottomPanel,
    SessionPanel,
}

/// A keybinding declared by a plugin.
#[derive(Debug, Clone)]
pub struct PluginKeybind {
    /// Action name, e.g. "open_panel", "run", or a custom name.
    pub action: String,
    /// Default key binding string, e.g. "cmd+shift+i".
    pub default_binding: String,
    /// Optional human-readable description.
    pub description: String,
}

/// Metadata parsed from a Lua plugin header.
#[derive(Debug, Clone)]
pub struct PluginMeta {
    pub name: String,
    pub description: String,
    pub version: String,
    pub plugin_type: PluginType,
    pub keybindings: Vec<PluginKeybind>,
    /// Optional icon path (resolved relative to plugin file).
    pub icon: Option<PathBuf>,
    pub path: PathBuf,
}

/// Discover plugins in the given directory.
pub fn discover_plugins(dir: &Path) -> Result<Vec<PluginMeta>> {
    let mut plugins = Vec::new();

    if !dir.exists() {
        return Ok(plugins);
    }

    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "lua") {
            if let Ok(meta) = parse_plugin_header(&path) {
                plugins.push(meta);
            }
        }
    }

    Ok(plugins)
}

/// Allowed image extensions for plugin icons.
const ALLOWED_ICON_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "bmp", "ico", "webp"];

/// Validate that a path points to a real image file.
/// Checks: exists, has an allowed image extension, file size is reasonable,
/// and the first bytes match a known image magic number.
fn validate_icon_path(path: &Path) -> bool {
    // Must exist
    if !path.is_file() {
        return false;
    }

    // Must have an allowed image extension
    let ext = path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_default();
    if !ALLOWED_ICON_EXTENSIONS.contains(&ext.as_str()) {
        return false;
    }

    // File size sanity check: must be between 16 bytes and 2MB
    let metadata = match fs::metadata(path) {
        Ok(m) => m,
        Err(_) => return false,
    };
    let size = metadata.len();
    if size < 16 || size > 2 * 1024 * 1024 {
        return false;
    }

    // Check magic bytes to confirm it's actually an image
    let mut file = match fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return false,
    };
    let mut header = [0u8; 12];
    use std::io::Read;
    if file.read_exact(&mut header).is_err() {
        return false;
    }

    // PNG: 89 50 4E 47
    if header[0..4] == [0x89, 0x50, 0x4E, 0x47] {
        return true;
    }
    // JPEG: FF D8 FF
    if header[0..3] == [0xFF, 0xD8, 0xFF] {
        return true;
    }
    // GIF: GIF87a or GIF89a
    if header[0..3] == *b"GIF" {
        return true;
    }
    // BMP: BM
    if header[0..2] == *b"BM" {
        return true;
    }
    // WebP: RIFF....WEBP
    if header[0..4] == *b"RIFF" && header[8..12] == *b"WEBP" {
        return true;
    }
    // ICO: 00 00 01 00
    if header[0..4] == [0x00, 0x00, 0x01, 0x00] {
        return true;
    }

    false
}

/// Validate icon bytes (for runtime-set icons). Returns true if the data
/// starts with a known image magic number.
pub fn validate_icon_bytes(data: &[u8]) -> bool {
    if data.len() < 12 {
        return false;
    }
    if data[0..4] == [0x89, 0x50, 0x4E, 0x47] { return true; }
    if data[0..3] == [0xFF, 0xD8, 0xFF] { return true; }
    if data[0..3] == *b"GIF" { return true; }
    if data[0..2] == *b"BM" { return true; }
    if data[0..4] == *b"RIFF" && data[8..12] == *b"WEBP" { return true; }
    if data[0..4] == [0x00, 0x00, 0x01, 0x00] { return true; }
    false
}

/// Parse plugin metadata from source text (for use by the checker without disk I/O).
pub fn parse_plugin_header_from_source(source: &str, path: &Path) -> Option<PluginMeta> {
    Some(parse_plugin_header_inner(source, path))
}

fn parse_plugin_header(path: &Path) -> Result<PluginMeta> {
    let contents = fs::read_to_string(path)?;
    Ok(parse_plugin_header_inner(&contents, path))
}

fn parse_plugin_header_inner(contents: &str, path: &Path) -> PluginMeta {
    let mut name = path.file_stem().unwrap_or_default().to_string_lossy().into_owned();
    let mut description = String::new();
    let mut version = String::from("0.0.0");
    let mut plugin_type = PluginType::Action;
    let mut keybindings = Vec::new();
    let mut icon: Option<PathBuf> = None;

    let plugin_dir = path.parent().unwrap_or(Path::new("."));

    for line in contents.lines() {
        let line = line.trim();
        if !line.starts_with("--") {
            break;
        }
        let comment = line.trim_start_matches('-').trim();
        if let Some(val) = comment.strip_prefix("plugin-name:") {
            name = val.trim().to_string();
        } else if let Some(val) = comment.strip_prefix("plugin-description:") {
            description = val.trim().to_string();
        } else if let Some(val) = comment.strip_prefix("plugin-version:") {
            version = val.trim().to_string();
        } else if let Some(val) = comment.strip_prefix("plugin-type:") {
            match val.trim() {
                "panel" => plugin_type = PluginType::Panel,
                "bottom-panel" => plugin_type = PluginType::BottomPanel,
                "session-panel" => plugin_type = PluginType::SessionPanel,
                _ => {}
            }
        } else if let Some(val) = comment.strip_prefix("plugin-keybind:") {
            // Format: "action_name = binding" or "action_name = binding | description"
            if let Some((action_part, rest)) = val.split_once('=') {
                let action = action_part.trim().to_string();
                let (binding, desc) = if let Some((b, d)) = rest.split_once('|') {
                    (b.trim().to_string(), d.trim().to_string())
                } else {
                    (rest.trim().to_string(), String::new())
                };
                keybindings.push(PluginKeybind {
                    action,
                    default_binding: binding,
                    description: desc,
                });
            }
        } else if let Some(val) = comment.strip_prefix("plugin-icon:") {
            let icon_str = val.trim();
            if !icon_str.is_empty() {
                let icon_path = if Path::new(icon_str).is_absolute() {
                    PathBuf::from(icon_str)
                } else {
                    plugin_dir.join(icon_str)
                };
                if validate_icon_path(&icon_path) {
                    icon = Some(icon_path);
                }
            }
        }
    }

    PluginMeta {
        name,
        description,
        version,
        plugin_type,
        keybindings,
        icon,
        path: path.to_path_buf(),
    }
}

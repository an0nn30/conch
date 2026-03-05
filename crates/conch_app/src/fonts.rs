use font_kit::family_name::FamilyName;
use font_kit::properties::Properties;
use font_kit::source::SystemSource;

/// Loads the system UI font bytes for use as egui's Proportional family.
///
/// If `ui_family_override` is non-empty, attempts to load that family.
/// Otherwise tries a platform-specific list of concrete font names.
/// Returns `None` on failure (egui keeps its bundled fallback).
pub fn load_system_ui_font(ui_family_override: &str) -> Option<(String, Vec<u8>)> {
    if !ui_family_override.is_empty() {
        return try_load_font(ui_family_override);
    }

    for name in default_ui_families() {
        if let Some(result) = try_load_font(name) {
            return Some(result);
        }
    }

    log::warn!("Could not load any system UI font; using egui default");
    None
}

fn try_load_font(name: &str) -> Option<(String, Vec<u8>)> {
    let source = SystemSource::new();
    let handle = source
        .select_best_match(
            &[FamilyName::Title(name.to_owned())],
            &Properties::new(),
        )
        .map_err(|e| log::debug!("Font '{}' not found: {}", name, e))
        .ok()?;

    let font = handle
        .load()
        .map_err(|e| log::debug!("Failed to load font '{}': {}", name, e))
        .ok()?;

    let data = font.copy_font_data()?;

    log::info!("Loaded system UI font: {}", name);
    Some((name.to_owned(), (*data).clone()))
}

/// Concrete font family names to try, in preference order.
/// `.AppleSystemUIFont` is a virtual Core Text alias that cannot be
/// exported as raw bytes, so we use the real family names instead.
fn default_ui_families() -> &'static [&'static str] {
    if cfg!(target_os = "macos") {
        &["SF Pro", "SF Pro Text", "Helvetica Neue", "Helvetica"]
    } else if cfg!(target_os = "windows") {
        &["Segoe UI", "Tahoma", "Arial"]
    } else {
        &["DejaVu Sans", "Liberation Sans", "sans-serif"]
    }
}

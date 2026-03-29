//! System font enumeration for the settings UI.

use font_kit::source::SystemSource;
use serde::Serialize;
use ts_rs::TS;

#[derive(Serialize, TS)]
#[ts(export)]
pub(crate) struct SystemFonts {
    all: Vec<String>,
    monospace: Vec<String>,
}

/// Check whether the first available font in a family is monospace by comparing
/// the advance widths of 'i' (narrow) and 'M' (wide).  If they match the font
/// is fixed-pitch.
fn is_monospace(source: &SystemSource, family: &str) -> bool {
    let handle = match source.select_family_by_name(family) {
        Ok(h) => h,
        Err(_) => return false,
    };
    let fonts = handle.fonts();
    let font = match fonts.first() {
        Some(f) => match f.load() {
            Ok(f) => f,
            Err(_) => return false,
        },
        None => return false,
    };
    let glyph_i = font.glyph_for_char('i');
    let glyph_m = font.glyph_for_char('M');
    match (glyph_i, glyph_m) {
        (Some(gi), Some(gm)) => {
            let adv_i = font.advance(gi).ok();
            let adv_m = font.advance(gm).ok();
            match (adv_i, adv_m) {
                (Some(ai), Some(am)) => (ai.x() - am.x()).abs() < 0.01,
                _ => false,
            }
        }
        _ => false,
    }
}

/// Enumerate system fonts.  Returns all families (sorted) and the monospace
/// subset.  `current_terminal_font` is always included in the monospace list
/// even if detection misses it.
pub(crate) fn enumerate_system_fonts(current_terminal_font: Option<&str>) -> SystemFonts {
    let source = SystemSource::new();
    let mut all = source.all_families().unwrap_or_default();
    all.sort_by(|a, b| a.to_lowercase().cmp(&b.to_lowercase()));
    all.dedup();

    let mut monospace: Vec<String> = all
        .iter()
        .filter(|f| is_monospace(&source, f))
        .cloned()
        .collect();

    // Ensure the user's current terminal font is always present.
    if let Some(current) = current_terminal_font {
        if !current.is_empty() && !monospace.iter().any(|f| f == current) {
            monospace.push(current.to_string());
            monospace.sort_by(|a, b| a.to_lowercase().cmp(&b.to_lowercase()));
        }
    }

    SystemFonts { all, monospace }
}

#[tauri::command]
pub(crate) fn list_system_fonts(state: tauri::State<'_, crate::TauriState>) -> SystemFonts {
    let current = {
        let cfg = state.config.read();
        let family = cfg.resolved_terminal_font().normal.family.clone();
        if family.is_empty() { None } else { Some(family) }
    };
    enumerate_system_fonts(current.as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enumerate_returns_non_empty_lists() {
        let fonts = enumerate_system_fonts(None);
        assert!(!fonts.all.is_empty(), "System should have at least one font");
        assert!(
            !fonts.monospace.is_empty(),
            "System should have at least one monospace font"
        );
    }

    #[test]
    fn all_fonts_are_sorted_case_insensitive() {
        let fonts = enumerate_system_fonts(None);
        for pair in fonts.all.windows(2) {
            assert!(
                pair[0].to_lowercase() <= pair[1].to_lowercase(),
                "Fonts not sorted: {:?} > {:?}",
                pair[0],
                pair[1]
            );
        }
    }

    #[test]
    fn monospace_fonts_are_sorted_case_insensitive() {
        let fonts = enumerate_system_fonts(None);
        for pair in fonts.monospace.windows(2) {
            assert!(
                pair[0].to_lowercase() <= pair[1].to_lowercase(),
                "Monospace fonts not sorted: {:?} > {:?}",
                pair[0],
                pair[1]
            );
        }
    }

    #[test]
    fn monospace_is_subset_of_all_unless_injected() {
        let fonts = enumerate_system_fonts(None);
        for m in &fonts.monospace {
            assert!(
                fonts.all.iter().any(|a| a == m),
                "Monospace font {:?} missing from all-fonts list",
                m
            );
        }
    }

    #[test]
    fn current_terminal_font_always_in_monospace_list() {
        let fake_font = "ZZZ-Nonexistent-Mono-Test";
        let fonts = enumerate_system_fonts(Some(fake_font));
        assert!(
            fonts.monospace.iter().any(|f| f == fake_font),
            "Current terminal font should always be included in monospace list"
        );
    }

    #[test]
    fn empty_current_font_not_injected() {
        let fonts = enumerate_system_fonts(Some(""));
        assert!(
            !fonts.monospace.iter().any(|f| f.is_empty()),
            "Empty string should not be injected into monospace list"
        );
    }

    #[test]
    fn known_monospace_font_detected() {
        // Menlo is available on all macOS systems, Courier New on all platforms.
        let fonts = enumerate_system_fonts(None);
        let has_known_mono = fonts.monospace.iter().any(|f| {
            f == "Menlo" || f == "Courier New" || f == "Consolas" || f == "DejaVu Sans Mono"
        });
        assert!(
            has_known_mono,
            "At least one well-known monospace font should be detected. Found: {:?}",
            &fonts.monospace[..fonts.monospace.len().min(10)]
        );
    }
}

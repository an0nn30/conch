//! System font enumeration for the settings UI.
//!
//! On macOS, uses Core Text symbolic traits (cached metadata — no font loading).
//! On other platforms, falls back to font-kit glyph advance comparison.

use serde::Serialize;
use ts_rs::TS;

#[derive(Serialize, TS)]
#[ts(export)]
pub(crate) struct SystemFonts {
    all: Vec<String>,
    monospace: Vec<String>,
}

/// Shared post-processing: sort, dedup, inject current terminal font.
fn finalize(
    mut all: Vec<String>,
    mut monospace: Vec<String>,
    current_terminal_font: Option<&str>,
) -> SystemFonts {
    all.sort_by(|a, b| a.to_lowercase().cmp(&b.to_lowercase()));
    all.dedup();
    monospace.sort_by(|a, b| a.to_lowercase().cmp(&b.to_lowercase()));
    monospace.dedup();

    if let Some(current) = current_terminal_font {
        if !current.is_empty() && !monospace.iter().any(|f| f == current) {
            monospace.push(current.to_string());
            monospace.sort_by(|a, b| a.to_lowercase().cmp(&b.to_lowercase()));
        }
    }

    SystemFonts { all, monospace }
}

// ---------------------------------------------------------------------------
// macOS: Core Text symbolic traits (fast — reads cached metadata, no disk I/O)
// ---------------------------------------------------------------------------

#[cfg(target_os = "macos")]
pub(crate) fn enumerate_system_fonts(current_terminal_font: Option<&str>) -> SystemFonts {
    use core_text::font_collection;
    use core_text::font_descriptor::{SymbolicTraitAccessors, TraitAccessors};
    use std::collections::HashSet;

    let collection = font_collection::create_for_all_families();
    let descriptors = match collection.get_descriptors() {
        Some(d) => d,
        None => return finalize(Vec::new(), Vec::new(), current_terminal_font),
    };

    let mut all_set = HashSet::new();
    let mut mono_set = HashSet::new();

    for i in 0..descriptors.len() {
        let desc = descriptors.get(i).unwrap();
        let family = desc.family_name();
        all_set.insert(family.clone());

        let symbolic = desc.traits().symbolic_traits();
        if symbolic.is_monospace() {
            mono_set.insert(family);
        }
    }

    let all: Vec<String> = all_set.into_iter().collect();
    let monospace: Vec<String> = mono_set.into_iter().collect();

    finalize(all, monospace, current_terminal_font)
}

// ---------------------------------------------------------------------------
// Non-macOS: font-kit glyph advance comparison (slower but cross-platform)
// ---------------------------------------------------------------------------

#[cfg(not(target_os = "macos"))]
pub(crate) fn enumerate_system_fonts(current_terminal_font: Option<&str>) -> SystemFonts {
    use font_kit::source::SystemSource;

    let source = SystemSource::new();
    let all = source.all_families().unwrap_or_default();

    let monospace: Vec<String> = all
        .iter()
        .filter(|f| is_monospace_fontkit(&source, f))
        .cloned()
        .collect();

    finalize(all, monospace, current_terminal_font)
}

#[cfg(not(target_os = "macos"))]
fn is_monospace_fontkit(source: &font_kit::source::SystemSource, family: &str) -> bool {
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

// ---------------------------------------------------------------------------
// Tauri command
// ---------------------------------------------------------------------------

#[tauri::command]
pub(crate) async fn list_system_fonts(
    state: tauri::State<'_, crate::TauriState>,
) -> Result<SystemFonts, String> {
    let current = {
        let cfg = state.config.read();
        let family = cfg.resolved_terminal_font().normal.family.clone();
        if family.is_empty() { None } else { Some(family) }
    };
    tokio::task::spawn_blocking(move || enumerate_system_fonts(current.as_deref()))
        .await
        .map_err(|e| format!("Font enumeration failed: {e}"))
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

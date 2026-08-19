//! Mapping Alacritty's sparse `colors.indexed_colors[]` onto xterm.js's
//! dense `ITheme.extendedAnsi` array.
//!
//! ## The shape mismatch
//!
//! Alacritty stores extended-palette overrides as `{ index, color }` pairs
//! that may skip indices freely (`16`, `17`, then `232`). xterm.js's
//! `ITheme.extendedAnsi` is a plain array whose element 0 is ANSI slot 16.
//!
//! ## Hole tolerance — traced against the vendored bundle
//!
//! Vendored source: `crates/termlab_tauri/frontend/vendor/xterm/xterm.min.js`
//! (`@xterm/xterm@5.5.0`, per the header comment). Three sites decide this,
//! quoted verbatim from the minified bundle:
//!
//! 1. The theme setter seeds the palette from the defaults before touching
//!    `extendedAnsi` at all:
//!    `i.ansi = t.DEFAULT_ANSI_COLORS.slice()`.
//!    `DEFAULT_ANSI_COLORS` is the full 256-entry standard table — 16 named
//!    colors, then the 216-entry cube built from the levels
//!    `[0,95,135,175,215,255]`, then 24 greys at `8 + 10*t`.
//! 2. The overlay loop:
//!    `if (e.extendedAnsi) { const s = Math.min(i.ansi.length - 16, e.extendedAnsi.length);
//!     for (let r = 0; r < s; r++) i.ansi[r+16] = p(e.extendedAnsi[r], t.DEFAULT_ANSI_COLORS[r+16]) }`
//! 3. The per-entry parser:
//!    `function p(e, t) { if (void 0 !== e) try { return o.css.toColor(e) } catch {} return t }`
//!
//! So an absent entry (`undefined`) short-circuits on `void 0 !== e` and the
//! slot keeps `DEFAULT_ANSI_COLORS[r+16]` — which is exactly what step 1 had
//! already written there. Holes are tolerated, and a hole is *semantically
//! identical* to sending the standard default for that slot.
//!
//! That settles the design question the Task-1 review flagged: building a
//! dense 240-entry base in Rust would mean re-deriving xterm's own default
//! table, duplicating it with a drift risk, to produce a byte-identical
//! result. So this emits a SPARSE array — `None` for every index the theme
//! does not override — and `app/core/config-service.js` maps those `None`s
//! (JSON `null`) to `undefined` before handing the array to xterm, hitting
//! the clean `void 0 !== e` branch rather than relying on `css.toColor(null)`
//! throwing into `p`'s `catch`.
//!
//! ## `colors.dim` has no carrier
//!
//! For the record, since it is the sibling question: `_setTheme(e = {})` in
//! the same bundle reads exactly `foreground`, `background`, `cursor`,
//! `cursorAccent`, `selectionBackground`, `selectionInactiveBackground`,
//! `selectionForeground`, `black`..`brightWhite`, and `extendedAnsi`. There
//! is no dim key. The renderers derive dim instead of looking it up — the DOM
//! renderer pushes the `xterm-dim` class (`I.isDim() && B.push("xterm-dim")`)
//! and halves the minimum-contrast ratio (`s.isDim() ? 2 : 1`) — so `dim` is
//! an opacity transform on the already-chosen color, never a palette entry.
//! `colors.dim.*` is therefore parsed and preserved but not applied; there is
//! nothing to hand xterm, and forcing it would mean overwriting the bright or
//! normal slots with dim values, which is worse than not applying it.

use termlab_core::color_scheme::IndexedColor;

/// The first ANSI slot `extendedAnsi` addresses. Element 0 of the emitted
/// array is this slot.
const EXTENDED_ANSI_BASE: usize = 16;

/// Build the sparse `extendedAnsi` array for a theme's `indexed_colors`.
///
/// The returned vector has one entry per ANSI slot from 16 up to the highest
/// index the theme overrides; every slot the theme leaves alone is `None`.
/// An empty result means "omit the key entirely", which the frontend does.
///
/// - Indices below 16 are skipped with a warning. Alacritty documents
///   `indexed_colors` as covering 16-255, and slots 0-15 are already carried
///   by the 16 named `ThemeColors` fields — writing them here would silently
///   fight those.
/// - Duplicate indices: the last entry wins, matching a plain in-order
///   overlay.
pub(crate) fn build_extended_ansi(indexed: &[IndexedColor]) -> Vec<Option<String>> {
    let mut highest: Option<usize> = None;
    for entry in indexed {
        let index = entry.index as usize;
        if index < EXTENDED_ANSI_BASE {
            log::warn!(
                "Ignoring indexed_colors entry with index {index}: extended palette starts at {EXTENDED_ANSI_BASE} (0-15 are the named ANSI colors)"
            );
            continue;
        }
        highest = Some(highest.map_or(index, |h| h.max(index)));
    }

    let Some(highest) = highest else {
        return Vec::new();
    };

    let mut out = vec![None; highest - EXTENDED_ANSI_BASE + 1];
    for entry in indexed {
        let index = entry.index as usize;
        if index < EXTENDED_ANSI_BASE {
            continue;
        }
        out[index - EXTENDED_ANSI_BASE] = Some(entry.color.clone());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn indexed(pairs: &[(u8, &str)]) -> Vec<IndexedColor> {
        pairs
            .iter()
            .map(|(index, color)| IndexedColor {
                index: *index,
                color: (*color).to_string(),
            })
            .collect()
    }

    #[test]
    fn no_indexed_colors_produces_an_empty_array() {
        assert!(build_extended_ansi(&[]).is_empty());
    }

    /// The github_dark fixture's exact shape: indices 16 and 17, contiguous
    /// from the base, so the array is fully dense with no holes.
    #[test]
    fn contiguous_indices_from_the_base_map_densely() {
        let out = build_extended_ansi(&indexed(&[(16, "#d18616"), (17, "#f97583")]));
        assert_eq!(
            out,
            vec![Some("#d18616".to_string()), Some("#f97583".to_string())]
        );
    }

    /// The sparse case, with the exact expected array asserted: index 16 and
    /// index 20 leave three holes between them, and the array stops at the
    /// highest overridden slot rather than padding out to 255.
    #[test]
    fn sparse_indices_leave_holes_and_stop_at_the_highest_index() {
        let out = build_extended_ansi(&indexed(&[(16, "#111111"), (20, "#222222")]));
        assert_eq!(
            out,
            vec![
                Some("#111111".to_string()),
                None,
                None,
                None,
                Some("#222222".to_string()),
            ]
        );
        assert_eq!(out.len(), 5, "20 - 16 + 1, not 240");
    }

    /// A theme that only overrides a high slot still gets element 0 == slot
    /// 16; everything below its index is a hole.
    #[test]
    fn a_single_high_index_pads_the_leading_slots_with_holes() {
        let out = build_extended_ansi(&indexed(&[(255, "#abcdef")]));
        assert_eq!(out.len(), 240, "16..=255 inclusive");
        assert_eq!(out[239], Some("#abcdef".to_string()));
        assert!(out[..239].iter().all(Option::is_none));
    }

    #[test]
    fn indices_below_sixteen_are_skipped() {
        let out = build_extended_ansi(&indexed(&[(0, "#000000"), (15, "#ffffff")]));
        assert!(
            out.is_empty(),
            "slots 0-15 belong to the named ANSI fields, not extendedAnsi"
        );
    }

    #[test]
    fn a_below_sixteen_index_does_not_shift_the_valid_ones() {
        let out = build_extended_ansi(&indexed(&[(3, "#000000"), (16, "#d18616")]));
        assert_eq!(out, vec![Some("#d18616".to_string())]);
    }

    #[test]
    fn duplicate_indices_take_the_last_entry() {
        let out = build_extended_ansi(&indexed(&[(16, "#111111"), (16, "#222222")]));
        assert_eq!(out, vec![Some("#222222".to_string())]);
    }

    #[test]
    fn out_of_order_entries_still_land_in_index_order() {
        let out = build_extended_ansi(&indexed(&[(18, "#333333"), (16, "#111111")]));
        assert_eq!(
            out,
            vec![
                Some("#111111".to_string()),
                None,
                Some("#333333".to_string())
            ]
        );
    }
}

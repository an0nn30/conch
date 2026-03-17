//! Mouse handling for terminal views: text selection and mouse-mode forwarding.

use std::time::Instant;

use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::Term;
use conch_pty::EventProxy;
use std::sync::Arc;

use crate::terminal::size_info::SizeInfo;
use crate::terminal::widget::{line_selection_at, pixel_to_cell, word_selection_at};

/// Mouse text selection state for a terminal view.
#[derive(Default)]
pub struct Selection {
    /// Cell coordinate where the drag began.
    pub start: Option<(usize, usize)>,
    /// Cell coordinate where the drag currently ends.
    pub end: Option<(usize, usize)>,
    /// Whether a drag is in progress.
    pub active: bool,
    /// Click count for multi-click detection (1=normal, 2=word, 3=line).
    click_count: u8,
    /// When the last click occurred (for multi-click timing).
    last_click_time: Option<Instant>,
    /// Where the last click was (cell coords, for multi-click proximity check).
    last_click_cell: Option<(usize, usize)>,
    /// Whether the current scroll session (including momentum) started over the terminal.
    /// Prevents trackpad momentum from leaking into the terminal when the pointer
    /// moves away from the panel where scrolling originated.
    scroll_engaged: bool,
}

/// Maximum interval between clicks to count as multi-click.
const MULTI_CLICK_THRESHOLD: std::time::Duration = std::time::Duration::from_millis(400);

impl Selection {
    /// Return the selection with start <= end in row-major order, or `None` if empty.
    pub fn normalized(&self) -> Option<((usize, usize), (usize, usize))> {
        let s = self.start?;
        let e = self.end?;
        if s == e && self.click_count <= 1 {
            return None;
        }
        if (s.1, s.0) <= (e.1, e.0) {
            Some((s, e))
        } else {
            Some((e, s))
        }
    }

    pub fn clear(&mut self) {
        self.start = None;
        self.end = None;
        self.active = false;
        self.click_count = 0;
        // Preserve last_click_time and last_click_cell for multi-click detection.
    }

    /// Register a click and return the new click count.
    fn register_click(&mut self, cell: (usize, usize)) -> u8 {
        let now = Instant::now();
        let is_multi = self.last_click_time.is_some_and(|t| now.duration_since(t) < MULTI_CLICK_THRESHOLD)
            && self.last_click_cell == Some(cell);

        if is_multi {
            self.click_count = ((self.click_count) % 3) + 1;
        } else {
            self.click_count = 1;
        }
        self.last_click_time = Some(now);
        self.last_click_cell = Some(cell);
        self.click_count
    }
}

/// Encode a mouse event as an xterm escape sequence.
///
/// `button`: 0=left, 1=middle, 2=right, 32=motion+left, 64=scroll-up, 65=scroll-down
/// `col`/`row`: 0-indexed cell coordinates
/// `sgr`: use SGR encoding (`\x1b[<...M/m`) when true, legacy X10 when false
/// `press`: true for press/motion, false for release
pub fn encode_mouse(button: u8, col: usize, row: usize, sgr: bool, press: bool) -> Vec<u8> {
    if sgr {
        // SGR: \x1b[<button;col+1;row+1M (press) or m (release)
        let suffix = if press { 'M' } else { 'm' };
        format!("\x1b[<{};{};{}{}", button, col + 1, row + 1, suffix).into_bytes()
    } else {
        // Legacy X10: \x1b[M (button+32) (col+33) (row+33)
        // Release is button 3 in legacy mode.
        let cb = if press { button + 32 } else { 3 + 32 };
        let cx = (col as u8).saturating_add(33).min(255);
        let cy = (row as u8).saturating_add(33).min(255);
        vec![0x1b, b'[', b'M', cb, cx, cy]
    }
}

/// Unified mouse handling for a terminal view.
///
/// Handles both mouse-mode forwarding (when the terminal application captures
/// the mouse) and normal text selection with multi-click support.
///
/// - `term` — the terminal for word/line selection queries.
/// - `write_fn` — callback to write escape bytes to the session's PTY.
/// - `cell_height` — height of a single cell in pixels (for scroll conversion).
/// - `scroll_sensitivity` — multiplier for scroll delta (0.0–1.0, lower = slower).
pub fn handle_terminal_mouse(
    ctx: &egui::Context,
    response: &egui::Response,
    size_info: &SizeInfo,
    selection: &mut Selection,
    term: &Arc<FairMutex<Term<EventProxy>>>,
    write_fn: &dyn Fn(&[u8]),
    cell_height: f32,
    scroll_sensitivity: f32,
) {
    use alacritty_terminal::term::TermMode;

    let (mouse_mode, sgr, alt_screen, alternate_scroll, app_cursor) = term
        .try_lock_unfair()
        .map(|t| {
            let mode = t.mode();
            (
                mode.intersects(TermMode::MOUSE_MODE),
                mode.contains(TermMode::SGR_MOUSE),
                mode.contains(TermMode::ALT_SCREEN),
                mode.contains(TermMode::ALTERNATE_SCROLL),
                mode.contains(TermMode::APP_CURSOR),
            )
        })
        .unwrap_or((false, false, false, false, false));

    // Scroll events (mouse wheel) — only process when scrolling originated over
    // the terminal. We use raw_scroll_delta (physical input only, no momentum) to
    // detect the start of a scroll gesture, and smooth_scroll_delta (includes
    // momentum smoothing) for the actual scroll amount. This prevents trackpad
    // momentum from leaking into the terminal when the user scrolls in a panel
    // and then moves the pointer over the terminal while momentum is still active.
    let scroll_delta = ctx.input(|i| i.smooth_scroll_delta);
    let raw_scroll = ctx.input(|i| i.raw_scroll_delta);
    // Use response.contains_pointer() instead of raw hover_pos — this respects
    // egui's layer ordering, so dialogs/popups on top of the terminal will
    // prevent scroll events from bleeding through to the terminal.
    let pointer_over_terminal = response.contains_pointer();

    // On Windows/Linux, discrete mouse wheels have no momentum — raw and smooth
    // deltas are identical (or raw may not be populated at all). In that case,
    // skip the engagement tracking and just check pointer position directly.
    // On macOS, use the raw/smooth distinction to prevent trackpad momentum
    // from leaking into the terminal when the pointer drifts away.
    let has_momentum_scrolling = cfg!(target_os = "macos");

    if has_momentum_scrolling {
        // raw_scroll_delta is non-zero only on actual physical scroll input (wheel
        // ticks, trackpad touch), not during momentum. Use it to decide whether
        // this is a new gesture that started over the terminal.
        if raw_scroll.y.abs() > 0.1 {
            // Physical scroll input — engage/disengage based on pointer position.
            selection.scroll_engaged = pointer_over_terminal;
        } else if scroll_delta.y.abs() < 0.1 {
            // No scroll at all (raw + smooth both idle) — reset for next gesture.
            selection.scroll_engaged = false;
        }
        // else: smooth momentum only (raw is zero) — keep scroll_engaged as-is.
    } else {
        // No momentum on this platform — engage whenever the pointer is over
        // the terminal and there is scroll input.
        selection.scroll_engaged = pointer_over_terminal && scroll_delta.y.abs() > 0.1;
    }

    // Dampen trackpad scroll — macOS trackpads produce very large pixel deltas
    // which translate to too many lines per frame. Scale down to feel natural.
    let dampened_delta = scroll_delta.y * scroll_sensitivity;

    if dampened_delta.abs() > 0.5 && selection.scroll_engaged {
        if mouse_mode {
            if let Some(pos) = ctx.input(|i| i.pointer.hover_pos()) {
                let (col, row) = pixel_to_cell(pos, response.rect.min, size_info);
                // Scroll up = button 64, scroll down = button 65.
                let button = if dampened_delta > 0.0 { 64u8 } else { 65u8 };
                let bytes = encode_mouse(button, col, row, sgr, true);
                write_fn(&bytes);
            }
        }
        // Not in mouse mode: handle scrollback or alt-screen arrow conversion.
        // (Also handles alt_screen+alternate_scroll even in mouse mode, matching
        // the original main-window behaviour.)
        if alt_screen && alternate_scroll {
            // Convert scroll to arrow key sequences (for less, man, etc.).
            let lines = (dampened_delta.abs() / cell_height).max(1.0) as usize;
            let arrow = if dampened_delta > 0.0 {
                if app_cursor { b"\x1bOA".as_slice() } else { b"\x1b[A".as_slice() }
            } else {
                if app_cursor { b"\x1bOB".as_slice() } else { b"\x1b[B".as_slice() }
            };
            for _ in 0..lines {
                write_fn(arrow);
            }
        } else if !alt_screen && !mouse_mode {
            // Normal screen: scroll through scrollback history.
            let lines = (dampened_delta.abs() / cell_height).max(1.0) as i32;
            let delta = if dampened_delta > 0.0 { lines } else { -lines };
            if let Some(mut t) = term.try_lock_unfair() {
                t.scroll_display(alacritty_terminal::grid::Scroll::Delta(delta));
            }
            ctx.request_repaint();
        }
    }

    if mouse_mode {
        // Forward mouse events to the terminal application using raw pointer
        // events. We avoid egui's drag_started_by() because it requires a
        // minimum drag distance before firing — quick clicks (e.g. tmux pane
        // switching) would be missed or delayed.
        let rect = response.rect;
        let pointer = ctx.input(|i| (
            i.pointer.hover_pos(),
            i.pointer.button_pressed(egui::PointerButton::Primary),
            i.pointer.button_pressed(egui::PointerButton::Secondary),
            i.pointer.button_pressed(egui::PointerButton::Middle),
            i.pointer.button_released(egui::PointerButton::Primary),
            i.pointer.button_released(egui::PointerButton::Secondary),
            i.pointer.button_released(egui::PointerButton::Middle),
            i.pointer.button_down(egui::PointerButton::Primary),
            i.pointer.button_down(egui::PointerButton::Secondary),
            i.pointer.button_down(egui::PointerButton::Middle),
        ));
        let (hover_pos, lmb_pressed, rmb_pressed, mmb_pressed,
             lmb_released, rmb_released, mmb_released,
             lmb_down, rmb_down, mmb_down) = pointer;

        // Only process events if the pointer is within the terminal rect.
        if let Some(pos) = hover_pos {
            if rect.contains(pos) {
                let (col, row) = pixel_to_cell(pos, rect.min, size_info);

                // Button presses.
                if lmb_pressed {
                    let bytes = encode_mouse(0, col, row, sgr, true);
                    write_fn(&bytes);
                }
                if rmb_pressed {
                    let bytes = encode_mouse(2, col, row, sgr, true);
                    write_fn(&bytes);
                }
                if mmb_pressed {
                    let bytes = encode_mouse(1, col, row, sgr, true);
                    write_fn(&bytes);
                }

                // Button releases.
                if lmb_released {
                    let bytes = encode_mouse(0, col, row, sgr, false);
                    write_fn(&bytes);
                }
                if rmb_released {
                    let bytes = encode_mouse(2, col, row, sgr, false);
                    write_fn(&bytes);
                }
                if mmb_released {
                    let bytes = encode_mouse(1, col, row, sgr, false);
                    write_fn(&bytes);
                }

                // Motion with button held (drag reporting).
                // Button code = button_id + 32 (motion flag).
                if !lmb_pressed && !rmb_pressed && !mmb_pressed {
                    let motion_button = if lmb_down {
                        Some(32u8) // 0 + 32
                    } else if rmb_down {
                        Some(34) // 2 + 32
                    } else if mmb_down {
                        Some(33) // 1 + 32
                    } else {
                        None
                    };
                    if let Some(btn) = motion_button {
                        if response.dragged() {
                            let bytes = encode_mouse(btn, col, row, sgr, true);
                            write_fn(&bytes);
                        }
                    }
                }
            }
        }
    } else {
        // Normal text selection (no mouse reporting) with multi-click support.
        if response.drag_started() {
            if let Some(pos) = response.interact_pointer_pos() {
                let cell = pixel_to_cell(pos, response.rect.min, size_info);
                let click_count = selection.register_click(cell);
                match click_count {
                    2 => {
                        // Double-click: select word.
                        let (start, end) = word_selection_at(term, cell.0, cell.1);
                        selection.start = Some(start);
                        selection.end = Some(end);
                        selection.active = false; // Already a complete selection.
                    }
                    3 => {
                        // Triple-click: select line.
                        let (start, end) = line_selection_at(term, cell.1);
                        selection.start = Some(start);
                        selection.end = Some(end);
                        selection.active = false;
                    }
                    _ => {
                        // Single click: start drag selection.
                        selection.start = Some(cell);
                        selection.end = Some(cell);
                        selection.active = true;
                    }
                }
            }
        }
        if response.dragged() && selection.active {
            if let Some(pos) = response.interact_pointer_pos() {
                selection.end = Some(pixel_to_cell(pos, response.rect.min, size_info));
            }
        }
        if response.drag_stopped() {
            selection.active = false;
        }
        // Single click without drag clears selection (but only if it's truly a single click,
        // not the start of a double/triple click that already set a selection).
        if response.clicked() && selection.click_count <= 1 {
            selection.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- encode_mouse --

    #[test]
    fn encode_mouse_sgr_press_left() {
        // Left button press at (5, 10) in SGR mode.
        let bytes = encode_mouse(0, 5, 10, true, true);
        assert_eq!(bytes, b"\x1b[<0;6;11M"); // col+1, row+1
    }

    #[test]
    fn encode_mouse_sgr_release_left() {
        let bytes = encode_mouse(0, 5, 10, true, false);
        assert_eq!(bytes, b"\x1b[<0;6;11m"); // lowercase 'm' for release
    }

    #[test]
    fn encode_mouse_sgr_right_button() {
        let bytes = encode_mouse(2, 0, 0, true, true);
        assert_eq!(bytes, b"\x1b[<2;1;1M");
    }

    #[test]
    fn encode_mouse_sgr_scroll_up() {
        let bytes = encode_mouse(64, 3, 7, true, true);
        assert_eq!(bytes, b"\x1b[<64;4;8M");
    }

    #[test]
    fn encode_mouse_sgr_motion_with_left() {
        let bytes = encode_mouse(32, 10, 20, true, true);
        assert_eq!(bytes, b"\x1b[<32;11;21M");
    }

    #[test]
    fn encode_mouse_legacy_press_left() {
        let bytes = encode_mouse(0, 5, 10, false, true);
        // Legacy: \x1b[M (button+32) (col+33) (row+33)
        assert_eq!(bytes, vec![0x1b, b'[', b'M', 32, 38, 43]);
    }

    #[test]
    fn encode_mouse_legacy_release() {
        let bytes = encode_mouse(0, 5, 10, false, false);
        // Release in legacy = button 3 + 32 = 35.
        assert_eq!(bytes, vec![0x1b, b'[', b'M', 35, 38, 43]);
    }

    #[test]
    fn encode_mouse_legacy_clamps_large_coords() {
        // Large coordinates should be clamped to 255.
        let bytes = encode_mouse(0, 250, 250, false, true);
        assert_eq!(bytes[4], 255); // 250 + 33 > 255, clamped.
        assert_eq!(bytes[5], 255);
    }

    #[test]
    fn encode_mouse_legacy_origin() {
        let bytes = encode_mouse(0, 0, 0, false, true);
        assert_eq!(bytes, vec![0x1b, b'[', b'M', 32, 33, 33]);
    }

    // -- Selection::normalized --

    #[test]
    fn selection_normalized_none_when_empty() {
        let sel = Selection::default();
        assert!(sel.normalized().is_none());
    }

    #[test]
    fn selection_normalized_none_for_single_click() {
        let mut sel = Selection::default();
        sel.start = Some((5, 3));
        sel.end = Some((5, 3));
        sel.click_count = 1;
        assert!(sel.normalized().is_none());
    }

    #[test]
    fn selection_normalized_some_for_double_click_same_cell() {
        let mut sel = Selection::default();
        sel.start = Some((5, 3));
        sel.end = Some((5, 3));
        sel.click_count = 2; // Double-click selects a word.
        assert!(sel.normalized().is_some());
    }

    #[test]
    fn selection_normalized_orders_start_before_end() {
        let mut sel = Selection::default();
        sel.start = Some((10, 5)); // col=10, row=5
        sel.end = Some((3, 2));    // col=3, row=2 — earlier in row-major
        sel.click_count = 1;
        let (s, e) = sel.normalized().unwrap();
        // Row 2 comes before row 5.
        assert_eq!(s, (3, 2));
        assert_eq!(e, (10, 5));
    }

    #[test]
    fn selection_normalized_same_row_orders_by_col() {
        let mut sel = Selection::default();
        sel.start = Some((10, 3));
        sel.end = Some((2, 3)); // Same row, earlier column.
        sel.click_count = 1;
        let (s, e) = sel.normalized().unwrap();
        assert_eq!(s, (2, 3));
        assert_eq!(e, (10, 3));
    }

    #[test]
    fn selection_normalized_already_ordered() {
        let mut sel = Selection::default();
        sel.start = Some((2, 1));
        sel.end = Some((10, 5));
        sel.click_count = 1;
        let (s, e) = sel.normalized().unwrap();
        assert_eq!(s, (2, 1));
        assert_eq!(e, (10, 5));
    }

    // -- Selection::clear --

    #[test]
    fn selection_clear_resets_state() {
        let mut sel = Selection::default();
        sel.start = Some((1, 2));
        sel.end = Some((3, 4));
        sel.active = true;
        sel.click_count = 2;
        sel.clear();
        assert!(sel.start.is_none());
        assert!(sel.end.is_none());
        assert!(!sel.active);
        assert_eq!(sel.click_count, 0);
    }

    // -- Selection::register_click --

    #[test]
    fn register_click_first_click_returns_1() {
        let mut sel = Selection::default();
        assert_eq!(sel.register_click((5, 3)), 1);
    }

    #[test]
    fn register_click_fast_same_cell_increments() {
        let mut sel = Selection::default();
        sel.register_click((5, 3));
        // Second immediate click on same cell.
        assert_eq!(sel.register_click((5, 3)), 2);
        // Third.
        assert_eq!(sel.register_click((5, 3)), 3);
    }

    #[test]
    fn register_click_wraps_after_triple() {
        let mut sel = Selection::default();
        sel.register_click((5, 3));
        sel.register_click((5, 3));
        sel.register_click((5, 3)); // 3
        // Fourth click wraps back to 1.
        assert_eq!(sel.register_click((5, 3)), 1);
    }

    #[test]
    fn register_click_different_cell_resets() {
        let mut sel = Selection::default();
        sel.register_click((5, 3));
        sel.register_click((5, 3)); // 2
        // Different cell resets to 1.
        assert_eq!(sel.register_click((10, 7)), 1);
    }
}

//! Terminal rendering via egui's `Painter` API.
//!
//! The core rendering loop iterates over `Term::renderable_content().display_iter`,
//! painting cell backgrounds and characters with `rect_filled` and `galley`.

use std::sync::Arc;

use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::cell::Flags as CellFlags;
use alacritty_terminal::term::Term;
use conch_pty::EventProxy;
use egui::{Color32, FontFamily, FontId, Painter, Pos2, Rect, Sense, Vec2};

use super::color::{convert_color, ResolvedColors};
use super::size_info::SizeInfo;

/// Convert an `[f32; 4]` RGBA color to egui's `Color32`.
#[inline]
fn rgba_to_color32(c: [f32; 4]) -> Color32 {
    Color32::from_rgba_unmultiplied(
        (c[0] * 255.0) as u8,
        (c[1] * 255.0) as u8,
        (c[2] * 255.0) as u8,
        (c[3] * 255.0) as u8,
    )
}

/// Measure the monospace font's cell dimensions from egui's layout engine.
///
/// Uses differential measurement -- `width(10 chars) - width(1 char)` divided by 9 --
/// to eliminate any fixed side-bearing overhead in galley sizes.
pub fn measure_cell_size(ctx: &egui::Context, font_size: f32) -> (f32, f32) {
    let font_id = FontId::new(font_size, FontFamily::Monospace);
    ctx.fonts(|fonts| {
        let g1 = fonts.layout_no_wrap("M".to_string(), font_id.clone(), Color32::WHITE);
        let g10 = fonts.layout_no_wrap("MMMMMMMMMM".to_string(), font_id, Color32::WHITE);
        let width = (g10.size().x - g1.size().x) / 9.0;
        let height = g1.size().y;
        (width, height)
    })
}

/// Convert a pixel position (relative to the window) to a terminal cell `(col, row)`.
pub fn pixel_to_cell(pos: Pos2, rect_min: Pos2, size_info: &SizeInfo) -> (usize, usize) {
    let x = (pos.x - rect_min.x - size_info.padding_x).max(0.0);
    let y = (pos.y - rect_min.y - size_info.padding_y).max(0.0);
    let col = (x / size_info.cell_width) as usize;
    let row = (y / size_info.cell_height) as usize;
    (col, row)
}

/// Check whether cell `(col, row)` falls within the normalized selection range.
#[inline]
fn is_in_selection(col: usize, row: usize, start: (usize, usize), end: (usize, usize)) -> bool {
    if row < start.1 || row > end.1 {
        return false;
    }
    if start.1 == end.1 {
        return col >= start.0 && col <= end.0;
    }
    if row == start.1 {
        return col >= start.0;
    }
    if row == end.1 {
        return col <= end.0;
    }
    true
}

/// Which underline style to draw (if any).
#[derive(Clone, Copy, PartialEq, Eq)]
enum UnderlineStyle {
    None,
    Single,
    Double,
    Dotted,
    Dashed,
    Undercurl,
}

/// Copied cell data for rendering after releasing the terminal lock.
#[derive(Clone)]
struct CellInfo {
    c: char,
    col: usize,
    row: usize,
    fg: [f32; 4],
    bg: [f32; 4],
    bold: bool,
    italic: bool,
    underline: UnderlineStyle,
    underline_color: Option<[f32; 4]>,
    strikeout: bool,
    wide: bool,
}

/// Cached frame data from the last successful terminal lock.
/// Re-used when the lock is contended to avoid flashing a blank frame.
#[derive(Clone)]
pub struct TerminalFrameCache {
    cells: Vec<CellInfo>,
    cursor_pos: Option<(usize, usize, alacritty_terminal::vte::ansi::CursorShape)>,
}

impl Default for TerminalFrameCache {
    fn default() -> Self {
        Self {
            cells: Vec::new(),
            cursor_pos: None,
        }
    }
}

/// Paint the terminal grid into the given UI region.
///
/// Returns the `Response` (for mouse interaction) and the computed `SizeInfo`.
pub fn show_terminal(
    ui: &mut egui::Ui,
    term: &Arc<FairMutex<Term<EventProxy>>>,
    cell_width: f32,
    cell_height: f32,
    colors: &ResolvedColors,
    font_size: f32,
    cursor_visible: bool,
    selection: Option<((usize, usize), (usize, usize))>,
    frame_cache: &mut TerminalFrameCache,
) -> (egui::Response, SizeInfo) {
    let available = ui.available_size();
    let (response, painter) = ui.allocate_painter(available, Sense::click_and_drag());
    let rect = response.rect;

    let size_info = SizeInfo::new(rect.width(), rect.height(), cell_width, cell_height);

    // Fill the entire allocation with the terminal background.
    painter.rect_filled(rect, 0.0, rgba_to_color32(colors.background));

    // ── Collect cell data under lock, then release ──────────────────────
    // This minimises FairMutex hold time so the EventLoop can keep
    // processing VTE data while we do the (expensive) font-layout paint.
    // Use try_lock_unfair to avoid blocking the main thread if the
    // event loop is holding the FairMutex lease during PTY reads.
    let (cells, cursor_pos) = {
        let Some(term) = term.try_lock_unfair() else {
            // Lock contended — re-use the last frame's cached data to avoid
            // flashing a blank terminal background.
            let cells = &frame_cache.cells;
            let cursor_pos = frame_cache.cursor_pos;
            paint_cells(&painter, cells, cursor_pos, &size_info, colors, font_size, cell_width, cell_height, rect);
            return (response, size_info);
        };
        let content = term.renderable_content();

        let show_cursor = cursor_visible
            && selection.is_none()
            && content
                .mode
                .contains(alacritty_terminal::term::TermMode::SHOW_CURSOR);

        let cursor_pos = if show_cursor {
            Some((content.cursor.point.column.0, content.cursor.point.line.0 as usize, content.cursor.shape))
        } else {
            None
        };

        let mut cells = Vec::with_capacity(size_info.columns() * size_info.rows());
        for indexed in content.display_iter {
            let cell = indexed.cell;
            let point = indexed.point;
            let flags = cell.flags;

            if flags.contains(CellFlags::WIDE_CHAR_SPACER) {
                continue;
            }

            let wide = flags.contains(CellFlags::WIDE_CHAR);

            let mut fg = convert_color(cell.fg, colors);
            let mut bg = convert_color(cell.bg, colors);

            if flags.contains(CellFlags::INVERSE) {
                std::mem::swap(&mut fg, &mut bg);
            }

            let col = point.column.0;
            let row = point.line.0 as usize;

            if let Some((sel_start, sel_end)) = selection {
                if is_in_selection(col, row, sel_start, sel_end) {
                    if let (Some(sel_bg), Some(sel_fg)) = (colors.selection_bg, colors.selection_text) {
                        bg = sel_bg;
                        fg = sel_fg;
                    } else {
                        std::mem::swap(&mut fg, &mut bg);
                    }
                }
            }

            // Hidden text: replace character with space.
            let c = if flags.contains(CellFlags::HIDDEN) { ' ' } else { cell.c };

            // Dim: reduce foreground brightness to 2/3.
            if flags.contains(CellFlags::DIM) {
                fg = [fg[0] * 0.67, fg[1] * 0.67, fg[2] * 0.67, fg[3]];
            }

            // Determine underline style.
            let underline = if flags.contains(CellFlags::UNDERCURL) {
                UnderlineStyle::Undercurl
            } else if flags.contains(CellFlags::DOUBLE_UNDERLINE) {
                UnderlineStyle::Double
            } else if flags.contains(CellFlags::DOTTED_UNDERLINE) {
                UnderlineStyle::Dotted
            } else if flags.contains(CellFlags::DASHED_UNDERLINE) {
                UnderlineStyle::Dashed
            } else if flags.contains(CellFlags::UNDERLINE) {
                UnderlineStyle::Single
            } else {
                UnderlineStyle::None
            };

            // Underline color from cell extras (e.g. LSP diagnostic colors).
            let underline_color = cell.underline_color().map(|uc| convert_color(uc, colors));

            cells.push(CellInfo {
                c,
                col,
                row,
                fg,
                bg,
                bold: flags.contains(CellFlags::BOLD),
                italic: flags.contains(CellFlags::ITALIC),
                underline,
                underline_color,
                strikeout: flags.contains(CellFlags::STRIKEOUT),
                wide,
            });
        }

        (cells, cursor_pos)
    }; // ── lock released here ──────────────────────────────────────────────

    // Cache this frame's data for re-use when the lock is contended.
    frame_cache.cells = cells.clone();
    frame_cache.cursor_pos = cursor_pos;

    // Paint cells (no lock held — EventLoop can process data concurrently).
    paint_cells(&painter, &cells, cursor_pos, &size_info, colors, font_size, cell_width, cell_height, rect);

    (response, size_info)
}

/// Find word boundaries around the given cell position.
///
/// Returns `((start_col, row), (end_col, row))` for the word at `(col, row)`.
pub fn word_selection_at(
    term: &Arc<FairMutex<Term<EventProxy>>>,
    col: usize,
    row: usize,
) -> ((usize, usize), (usize, usize)) {
    let Some(term) = term.try_lock_unfair() else {
        return ((col, row), (col, row));
    };
    let content = term.renderable_content();

    // Collect all characters in the target row.
    let mut row_chars: Vec<(usize, char)> = Vec::new();
    for indexed in content.display_iter {
        let r = indexed.point.line.0 as usize;
        if r < row {
            continue;
        }
        if r > row {
            break;
        }
        row_chars.push((indexed.point.column.0, indexed.cell.c));
    }

    if row_chars.is_empty() {
        return ((col, row), (col, row));
    }

    // Find the character at col.
    let is_word_char = |c: char| c.is_alphanumeric() || c == '_' || c == '-' || c == '.';
    let target_char = row_chars.iter().find(|&&(c, _)| c == col).map(|&(_, ch)| ch).unwrap_or(' ');
    let target_is_word = is_word_char(target_char);
    let target_is_space = target_char == ' ';

    // Expand left.
    let mut start_col = col;
    for &(c, ch) in row_chars.iter().rev() {
        if c > col {
            continue;
        }
        if target_is_space {
            if ch != ' ' { break; }
        } else if target_is_word {
            if !is_word_char(ch) { break; }
        } else {
            // Punctuation: select contiguous same-type.
            if is_word_char(ch) || ch == ' ' { break; }
        }
        start_col = c;
    }

    // Expand right.
    let mut end_col = col;
    for &(c, ch) in &row_chars {
        if c < col {
            continue;
        }
        if target_is_space {
            if ch != ' ' { break; }
        } else if target_is_word {
            if !is_word_char(ch) { break; }
        } else {
            if is_word_char(ch) || ch == ' ' { break; }
        }
        end_col = c;
    }

    ((start_col, row), (end_col, row))
}

/// Select the entire line at the given row.
///
/// Returns `((0, row), (last_col, row))`.
pub fn line_selection_at(
    term: &Arc<FairMutex<Term<EventProxy>>>,
    row: usize,
) -> ((usize, usize), (usize, usize)) {
    let Some(term) = term.try_lock_unfair() else {
        return ((0, row), (0, row));
    };
    let content = term.renderable_content();

    let mut max_col = 0usize;
    for indexed in content.display_iter {
        let r = indexed.point.line.0 as usize;
        if r < row {
            continue;
        }
        if r > row {
            break;
        }
        max_col = indexed.point.column.0;
    }

    ((0, row), (max_col, row))
}

/// Extract the text within a normalized selection range from the terminal buffer.
pub fn get_selected_text(
    term: &Arc<FairMutex<Term<EventProxy>>>,
    sel_start: (usize, usize),
    sel_end: (usize, usize),
) -> String {
    let Some(term) = term.try_lock_unfair() else {
        return String::new();
    };
    let content = term.renderable_content();

    let mut lines: Vec<String> = Vec::new();
    let mut current_row: Option<usize> = None;
    let mut current_line = String::new();

    for indexed in content.display_iter {
        let col = indexed.point.column.0;
        let row = indexed.point.line.0 as usize;

        if row > sel_end.1 {
            break;
        }
        if !is_in_selection(col, row, sel_start, sel_end) {
            continue;
        }

        if current_row != Some(row) {
            if current_row.is_some() {
                lines.push(current_line.trim_end().to_string());
                current_line = String::new();
            }
            current_row = Some(row);
        }

        current_line.push(indexed.cell.c);
    }

    if !current_line.is_empty() {
        lines.push(current_line.trim_end().to_string());
    }

    lines.join("\n")
}

/// Paint collected cell data and cursor onto the terminal area.
fn paint_cells(
    painter: &Painter,
    cells: &[CellInfo],
    cursor_pos: Option<(usize, usize, alacritty_terminal::vte::ansi::CursorShape)>,
    size_info: &SizeInfo,
    colors: &ResolvedColors,
    font_size: f32,
    cell_width: f32,
    cell_height: f32,
    rect: Rect,
) {
    let font_regular = FontId::new(font_size, FontFamily::Monospace);

    for ci in cells {
        let (x, y) = size_info.cell_position(ci.col, ci.row);

        let char_cell_width = if ci.wide { cell_width * 2.0 } else { cell_width };

        if ci.bg != colors.background {
            let cell_rect = Rect::from_min_size(
                Pos2::new(rect.min.x + x, rect.min.y + y),
                Vec2::new(char_cell_width, cell_height),
            );
            painter.rect_filled(cell_rect, 0.0, rgba_to_color32(ci.bg));
        }

        let fg_color = rgba_to_color32(ci.fg);

        if ci.c != ' ' && ci.c != '\0' {
            paint_char(
                painter,
                ci.c,
                Pos2::new(rect.min.x + x, rect.min.y + y),
                &font_regular,
                fg_color,
                ci.bold,
                ci.italic,
                char_cell_width,
                cell_height,
            );
        }

        // Draw underline (various styles).
        if ci.underline != UnderlineStyle::None {
            let ul_color = ci.underline_color.map(rgba_to_color32).unwrap_or(fg_color);
            let y_base = rect.min.y + y + cell_height - 1.0;
            let x_start = rect.min.x + x;
            let x_end = x_start + char_cell_width;
            draw_underline(painter, ci.underline, x_start, x_end, y_base, char_cell_width, ul_color);
        }

        // Draw strikeout.
        if ci.strikeout {
            let y_mid = rect.min.y + y + cell_height * 0.5;
            painter.line_segment(
                [Pos2::new(rect.min.x + x, y_mid), Pos2::new(rect.min.x + x + char_cell_width, y_mid)],
                egui::Stroke::new(1.0, fg_color),
            );
        }
    }

    // Draw cursor (Block, Underline, or Beam).
    if let Some((col, row, shape)) = cursor_pos {
        let (cx, cy) = size_info.cell_position(col, row);
        let cursor_c = colors.cursor_color.unwrap_or(colors.foreground);
        let color = rgba_to_color32(cursor_c);
        match shape {
            alacritty_terminal::vte::ansi::CursorShape::Block => {
                let cursor_rect = Rect::from_min_size(
                    Pos2::new(rect.min.x + cx, rect.min.y + cy),
                    Vec2::new(cell_width, cell_height),
                );
                painter.rect_filled(cursor_rect, 0.0, color);
            }
            alacritty_terminal::vte::ansi::CursorShape::Underline => {
                let thickness = (cell_height * 0.1).max(1.0);
                let cursor_rect = Rect::from_min_size(
                    Pos2::new(rect.min.x + cx, rect.min.y + cy + cell_height - thickness),
                    Vec2::new(cell_width, thickness),
                );
                painter.rect_filled(cursor_rect, 0.0, color);
            }
            alacritty_terminal::vte::ansi::CursorShape::Beam => {
                let thickness = (cell_width * 0.12).max(1.0);
                let cursor_rect = Rect::from_min_size(
                    Pos2::new(rect.min.x + cx, rect.min.y + cy),
                    Vec2::new(thickness, cell_height),
                );
                painter.rect_filled(cursor_rect, 0.0, color);
            }
            alacritty_terminal::vte::ansi::CursorShape::HollowBlock => {
                let cursor_rect = Rect::from_min_size(
                    Pos2::new(rect.min.x + cx, rect.min.y + cy),
                    Vec2::new(cell_width, cell_height),
                );
                painter.rect_stroke(cursor_rect, 0.0, egui::Stroke::new(1.0, color), egui::StrokeKind::Inside);
            }
            alacritty_terminal::vte::ansi::CursorShape::Hidden => {}
        }
    }
}

/// Render a single character in its cell, with synthetic bold/italic.
///
/// Characters are left-aligned within the cell to maintain consistent grid
/// positioning. This is critical for box-drawing, braille art, and other
/// characters that must align precisely across cells.
fn paint_char(
    painter: &Painter,
    c: char,
    pos: Pos2,
    font_id: &FontId,
    color: Color32,
    bold: bool,
    italic: bool,
    _cell_width: f32,
    cell_height: f32,
) {
    let galley = painter.layout_no_wrap(c.to_string(), font_id.clone(), color);

    let mut offset_x = 0.0;

    // Synthetic italic: shift top of glyph right by ~12% of cell height.
    if italic {
        offset_x += cell_height * 0.06;
    }

    let text_pos = Pos2::new(pos.x + offset_x, pos.y);
    painter.galley(text_pos, galley, color);

    // Synthetic bold: draw the glyph a second time offset by 1px.
    if bold {
        let galley2 = painter.layout_no_wrap(c.to_string(), font_id.clone(), color);
        painter.galley(Pos2::new(text_pos.x + 1.0, text_pos.y), galley2, color);
    }
}

/// Draw an underline with the given style.
fn draw_underline(
    painter: &Painter,
    style: UnderlineStyle,
    x_start: f32,
    x_end: f32,
    y: f32,
    cell_width: f32,
    color: Color32,
) {
    let stroke = egui::Stroke::new(1.0, color);
    match style {
        UnderlineStyle::None => {}
        UnderlineStyle::Single => {
            painter.line_segment([Pos2::new(x_start, y), Pos2::new(x_end, y)], stroke);
        }
        UnderlineStyle::Double => {
            painter.line_segment([Pos2::new(x_start, y), Pos2::new(x_end, y)], stroke);
            painter.line_segment([Pos2::new(x_start, y - 2.0), Pos2::new(x_end, y - 2.0)], stroke);
        }
        UnderlineStyle::Dotted => {
            let mut x = x_start;
            while x < x_end {
                let end = (x + 1.0).min(x_end);
                painter.line_segment([Pos2::new(x, y), Pos2::new(end, y)], stroke);
                x += 3.0;
            }
        }
        UnderlineStyle::Dashed => {
            let dash = cell_width * 0.4;
            let gap = cell_width * 0.2;
            let mut x = x_start;
            while x < x_end {
                let end = (x + dash).min(x_end);
                painter.line_segment([Pos2::new(x, y), Pos2::new(end, y)], stroke);
                x += dash + gap;
            }
        }
        UnderlineStyle::Undercurl => {
            // Wavy underline: approximate with short line segments.
            let amplitude = 1.5;
            let wavelength = cell_width;
            let steps = 8;
            let step_w = wavelength / steps as f32;
            let mut x = x_start;
            while x < x_end {
                for i in 0..steps {
                    let x0 = x + i as f32 * step_w;
                    let x1 = (x0 + step_w).min(x_end);
                    if x0 >= x_end {
                        break;
                    }
                    let t0 = i as f32 / steps as f32;
                    let t1 = (i + 1) as f32 / steps as f32;
                    let y0 = y + (t0 * std::f32::consts::TAU).sin() * amplitude;
                    let y1 = y + (t1 * std::f32::consts::TAU).sin() * amplitude;
                    painter.line_segment([Pos2::new(x0, y0), Pos2::new(x1, y1)], stroke);
                }
                x += wavelength;
            }
        }
    }
}

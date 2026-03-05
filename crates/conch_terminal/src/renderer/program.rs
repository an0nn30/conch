use std::sync::Arc;

use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::cell::Flags as CellFlags;
use alacritty_terminal::term::Term;
use alacritty_terminal::vte::ansi::{Color as TermColor, NamedColor};
use iced::mouse;
use iced::widget::shader;
use iced::Rectangle;

use super::primitive::{RenderCell, RenderCursor, TerminalPrimitive};
use crate::size_info::SizeInfo;
use conch_session::EventProxy;

/// iced shader::Program that reads from an alacritty_terminal Term and
/// produces a TerminalPrimitive for GPU rendering.
pub struct TerminalProgram {
    pub term: Arc<FairMutex<Term<EventProxy>>>,
    pub size_info: SizeInfo,
    pub bg_color: [f32; 4],
    pub fg_color: [f32; 4],
}

impl<Message> shader::Program<Message> for TerminalProgram {
    type State = ();
    type Primitive = TerminalPrimitive;

    fn draw(
        &self,
        _state: &Self::State,
        _cursor: mouse::Cursor,
        bounds: Rectangle,
    ) -> Self::Primitive {
        // Compute SizeInfo from actual widget bounds
        let size_info = SizeInfo::new(
            bounds.width,
            bounds.height,
            self.size_info.cell_width,
            self.size_info.cell_height,
        );

        let term = self.term.lock();
        let content = term.renderable_content();

        let mut cells = Vec::new();

        for indexed in content.display_iter {
            let cell = indexed.cell;
            let point = indexed.point;

            let c = cell.c;
            let flags = cell.flags;

            // Skip wide char spacers
            if flags.contains(CellFlags::WIDE_CHAR_SPACER) {
                continue;
            }

            let fg = convert_color(cell.fg, &self.fg_color);
            let bg = convert_color(cell.bg, &self.bg_color);

            // Handle INVERSE flag
            let (fg, bg) = if flags.contains(CellFlags::INVERSE) {
                (bg, fg)
            } else {
                (fg, bg)
            };

            cells.push(RenderCell {
                col: point.column.0,
                row: point.line.0 as usize,
                c,
                fg,
                bg,
                bold: flags.contains(CellFlags::BOLD),
                italic: flags.contains(CellFlags::ITALIC),
                underline: flags.contains(CellFlags::UNDERLINE),
                strikeout: flags.contains(CellFlags::STRIKEOUT),
            });
        }

        let cursor = if content.mode.contains(alacritty_terminal::term::TermMode::SHOW_CURSOR) {
            let cp = content.cursor.point;
            Some(RenderCursor {
                col: cp.column.0,
                row: cp.line.0 as usize,
                color: self.fg_color,
                visible: true,
            })
        } else {
            None
        };

        TerminalPrimitive {
            cells,
            cursor,
            size_info,
            bg_color: self.bg_color,
        }
    }
}

/// Convert alacritty_terminal Color to RGBA [f32; 4].
fn convert_color(color: TermColor, _default: &[f32; 4]) -> [f32; 4] {
    match color {
        TermColor::Named(named) => named_color_to_rgba(named),
        TermColor::Spec(rgb) => [rgb.r as f32 / 255.0, rgb.g as f32 / 255.0, rgb.b as f32 / 255.0, 1.0],
        TermColor::Indexed(idx) => indexed_color_to_rgba(idx),
    }
}

fn named_color_to_rgba(c: NamedColor) -> [f32; 4] {
    // Standard terminal colors (Dracula-inspired defaults)
    match c {
        NamedColor::Black => [0.16, 0.16, 0.21, 1.0],
        NamedColor::Red => [1.0, 0.33, 0.33, 1.0],
        NamedColor::Green => [0.31, 0.98, 0.48, 1.0],
        NamedColor::Yellow => [0.95, 0.98, 0.48, 1.0],
        NamedColor::Blue => [0.74, 0.58, 0.98, 1.0],
        NamedColor::Magenta => [1.0, 0.47, 0.66, 1.0],
        NamedColor::Cyan => [0.55, 0.96, 0.96, 1.0],
        NamedColor::White => [0.97, 0.97, 0.95, 1.0],
        NamedColor::BrightBlack => [0.38, 0.40, 0.50, 1.0],
        NamedColor::BrightRed => [1.0, 0.44, 0.44, 1.0],
        NamedColor::BrightGreen => [0.41, 1.0, 0.58, 1.0],
        NamedColor::BrightYellow => [0.95, 0.98, 0.58, 1.0],
        NamedColor::BrightBlue => [0.84, 0.68, 1.0, 1.0],
        NamedColor::BrightMagenta => [1.0, 0.57, 0.76, 1.0],
        NamedColor::BrightCyan => [0.65, 1.0, 1.0, 1.0],
        NamedColor::BrightWhite => [1.0, 1.0, 1.0, 1.0],
        NamedColor::Foreground => [0.97, 0.97, 0.95, 1.0],
        NamedColor::Background => [0.16, 0.16, 0.21, 1.0],
        NamedColor::Cursor => [0.97, 0.97, 0.95, 1.0],
        _ => [0.97, 0.97, 0.95, 1.0],
    }
}

fn indexed_color_to_rgba(idx: u8) -> [f32; 4] {
    if idx < 16 {
        // Use named colors for 0-15
        let named = match idx {
            0 => NamedColor::Black,
            1 => NamedColor::Red,
            2 => NamedColor::Green,
            3 => NamedColor::Yellow,
            4 => NamedColor::Blue,
            5 => NamedColor::Magenta,
            6 => NamedColor::Cyan,
            7 => NamedColor::White,
            8 => NamedColor::BrightBlack,
            9 => NamedColor::BrightRed,
            10 => NamedColor::BrightGreen,
            11 => NamedColor::BrightYellow,
            12 => NamedColor::BrightBlue,
            13 => NamedColor::BrightMagenta,
            14 => NamedColor::BrightCyan,
            15 => NamedColor::BrightWhite,
            _ => unreachable!(),
        };
        named_color_to_rgba(named)
    } else if idx < 232 {
        // 216 color cube (6x6x6)
        let idx = idx - 16;
        let r = (idx / 36) as f32 / 5.0;
        let g = ((idx / 6) % 6) as f32 / 5.0;
        let b = (idx % 6) as f32 / 5.0;
        [r, g, b, 1.0]
    } else {
        // 24 grayscale
        let gray = (idx - 232) as f32 / 23.0;
        [gray, gray, gray, 1.0]
    }
}

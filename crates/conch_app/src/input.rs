//! Keyboard input translation from egui key events to terminal escape sequences.
//!
//! Handles Ctrl+key combos, named keys (arrows, function keys, etc.), and
//! modifier-aware sequences (Shift+Tab, Ctrl+Home, Alt+key).
//!
//! Also provides configurable `KeyBinding` parsing and `ResolvedShortcuts`.

use conch_core::config::KeyboardConfig;
use egui::{Key, Modifiers};

/// A parsed key binding (e.g. "cmd+t" → Key::T + command modifier).
///
/// Uses the cross-platform `command` modifier: Cmd on macOS, Ctrl on
/// Windows/Linux.  The keywords "cmd", "super", "meta", "ctrl", and
/// "control" all map to this single modifier so that config strings
/// like `"cmd+t"` work on every platform.
#[derive(Debug, Clone)]
pub struct KeyBinding {
    pub key: Key,
    pub command: bool,
    pub alt: bool,
    pub shift: bool,
}

impl KeyBinding {
    /// Parse a binding string like "cmd+t", "ctrl+shift+n", "cmd+1".
    pub fn parse(s: &str) -> Option<Self> {
        let parts: Vec<&str> = s.split('+').map(|p| p.trim()).collect();
        if parts.is_empty() {
            return None;
        }

        let mut command = false;
        let mut alt = false;
        let mut shift = false;
        let mut key_part = None;

        for part in &parts {
            match part.to_lowercase().as_str() {
                "cmd" | "super" | "meta" | "ctrl" | "control" => command = true,
                "alt" | "option" => alt = true,
                "shift" => shift = true,
                _ => key_part = Some(*part),
            }
        }

        let key = parse_key_name(key_part?)?;

        Some(Self { key, command, alt, shift })
    }

    /// Check if this binding matches the given key + modifiers.
    pub fn matches(&self, key: &Key, modifiers: &Modifiers) -> bool {
        *key == self.key
            && modifiers.command == self.command
            && modifiers.alt == self.alt
            && modifiers.shift == self.shift
    }
}

/// Resolved set of app-level keyboard shortcuts.
#[derive(Debug, Clone)]
pub struct ResolvedShortcuts {
    pub new_tab: Option<KeyBinding>,
    pub close_tab: Option<KeyBinding>,
    pub new_connection: Option<KeyBinding>,
    pub quit: Option<KeyBinding>,
    pub toggle_left_sidebar: Option<KeyBinding>,
    pub toggle_right_sidebar: Option<KeyBinding>,
    pub focus_quick_connect: Option<KeyBinding>,
    pub focus_plugin_search: Option<KeyBinding>,
    pub new_window: Option<KeyBinding>,
    pub focus_files: Option<KeyBinding>,
    pub zen_mode: Option<KeyBinding>,
}

impl ResolvedShortcuts {
    /// Parse all shortcuts from the keyboard config.
    pub fn from_config(config: &KeyboardConfig) -> Self {
        Self {
            new_tab: KeyBinding::parse(&config.new_tab),
            close_tab: KeyBinding::parse(&config.close_tab),
            new_connection: KeyBinding::parse(&config.new_connection),
            quit: KeyBinding::parse(&config.quit),
            toggle_left_sidebar: KeyBinding::parse(&config.toggle_left_sidebar),
            toggle_right_sidebar: KeyBinding::parse(&config.toggle_right_sidebar),
            focus_quick_connect: KeyBinding::parse(&config.focus_quick_connect),
            focus_plugin_search: KeyBinding::parse(&config.focus_plugin_search),
            new_window: KeyBinding::parse(&config.new_window),
            focus_files: KeyBinding::parse(&config.focus_files),
            zen_mode: KeyBinding::parse(&config.zen_mode),
        }
    }

    /// Check if the given key+modifiers match any app shortcut.
    pub fn is_app_shortcut(&self, key: &Key, modifiers: &Modifiers) -> bool {
        let bindings = [
            &self.new_tab,
            &self.close_tab,
            &self.new_connection,
            &self.quit,
            &self.toggle_left_sidebar,
            &self.toggle_right_sidebar,
            &self.focus_quick_connect,
            &self.focus_plugin_search,
            &self.new_window,
            &self.focus_files,
            &self.zen_mode,
        ];
        if bindings.iter().any(|b| b.as_ref().is_some_and(|kb| kb.matches(key, modifiers))) {
            return true;
        }
        // Command+number for tab switching (Cmd on macOS, Ctrl on Linux/Windows).
        if modifiers.command && !modifiers.alt && !modifiers.shift {
            matches!(key, Key::Num1 | Key::Num2 | Key::Num3 | Key::Num4 | Key::Num5 | Key::Num6 | Key::Num7 | Key::Num8 | Key::Num9)
        } else {
            false
        }
    }
}

/// Convert an egui key event into bytes to send to the PTY.
///
/// Returns `None` for events that are app shortcuts or have no terminal mapping.
///
/// `app_cursor` should be `true` when the terminal is in application cursor mode
/// (DECCKM / `TermMode::APP_CURSOR`). This changes arrow keys and Home/End from
/// CSI sequences (`\x1b[A`) to SS3 sequences (`\x1bOA`).
pub fn key_to_bytes(
    key: &Key,
    modifiers: &Modifiers,
    text: Option<&str>,
    shortcuts: &ResolvedShortcuts,
    app_cursor: bool,
) -> Option<Vec<u8>> {
    // Suppress app shortcuts from reaching the terminal.
    if shortcuts.is_app_shortcut(key, modifiers) {
        return None;
    }

    // Ctrl+letter -> ASCII control character (0x01..0x1F).
    if modifiers.ctrl {
        if let Some(ch) = key_to_char(key) {
            if ch.is_ascii_lowercase() {
                return Some(vec![ch as u8 - b'a' + 1]);
            }
            return match ch {
                '[' => Some(vec![0x1b]),
                '\\' => Some(vec![0x1c]),
                ']' => Some(vec![0x1d]),
                '^' => Some(vec![0x1e]),
                '_' => Some(vec![0x1f]),
                _ => None,
            };
        }
    }

    // Named / special keys.
    match key {
        Key::Enter => return Some(b"\r".to_vec()),
        Key::Backspace => return Some(vec![0x7f]),
        Key::Tab if modifiers.shift => return Some(b"\x1b[Z".to_vec()),
        Key::Tab => return Some(b"\t".to_vec()),
        Key::Escape => return Some(vec![0x1b]),
        Key::Delete => return Some(b"\x1b[3~".to_vec()),
        Key::Insert => return Some(b"\x1b[2~".to_vec()),
        Key::Home if modifiers.ctrl => return Some(b"\x1b[1;5H".to_vec()),
        Key::Home if app_cursor => return Some(b"\x1bOH".to_vec()),
        Key::Home => return Some(b"\x1b[H".to_vec()),
        Key::End if modifiers.ctrl => return Some(b"\x1b[1;5F".to_vec()),
        Key::End if app_cursor => return Some(b"\x1bOF".to_vec()),
        Key::End => return Some(b"\x1b[F".to_vec()),
        Key::PageUp => return Some(b"\x1b[5~".to_vec()),
        Key::PageDown => return Some(b"\x1b[6~".to_vec()),
        Key::ArrowUp => return Some(arrow_key(b'A', modifiers, app_cursor)),
        Key::ArrowDown => return Some(arrow_key(b'B', modifiers, app_cursor)),
        Key::ArrowRight => return Some(arrow_key(b'C', modifiers, app_cursor)),
        Key::ArrowLeft => return Some(arrow_key(b'D', modifiers, app_cursor)),
        Key::F1 => return Some(b"\x1bOP".to_vec()),
        Key::F2 => return Some(b"\x1bOQ".to_vec()),
        Key::F3 => return Some(b"\x1bOR".to_vec()),
        Key::F4 => return Some(b"\x1bOS".to_vec()),
        Key::F5 => return Some(b"\x1b[15~".to_vec()),
        Key::F6 => return Some(b"\x1b[17~".to_vec()),
        Key::F7 => return Some(b"\x1b[18~".to_vec()),
        Key::F8 => return Some(b"\x1b[19~".to_vec()),
        Key::F9 => return Some(b"\x1b[20~".to_vec()),
        Key::F10 => return Some(b"\x1b[21~".to_vec()),
        Key::F11 => return Some(b"\x1b[23~".to_vec()),
        Key::F12 => return Some(b"\x1b[24~".to_vec()),
        _ => {}
    }

    // Regular printable text (with optional Alt prefix).
    if let Some(txt) = text {
        if !txt.is_empty() && !modifiers.ctrl {
            if modifiers.alt {
                let mut bytes = vec![0x1b];
                bytes.extend_from_slice(txt.as_bytes());
                return Some(bytes);
            }
            return Some(txt.as_bytes().to_vec());
        }
    }

    None
}

/// Parse a key name string to an egui Key.
fn parse_key_name(name: &str) -> Option<Key> {
    match name.to_lowercase().as_str() {
        "a" => Some(Key::A),
        "b" => Some(Key::B),
        "c" => Some(Key::C),
        "d" => Some(Key::D),
        "e" => Some(Key::E),
        "f" => Some(Key::F),
        "g" => Some(Key::G),
        "h" => Some(Key::H),
        "i" => Some(Key::I),
        "j" => Some(Key::J),
        "k" => Some(Key::K),
        "l" => Some(Key::L),
        "m" => Some(Key::M),
        "n" => Some(Key::N),
        "o" => Some(Key::O),
        "p" => Some(Key::P),
        "q" => Some(Key::Q),
        "r" => Some(Key::R),
        "s" => Some(Key::S),
        "t" => Some(Key::T),
        "u" => Some(Key::U),
        "v" => Some(Key::V),
        "w" => Some(Key::W),
        "x" => Some(Key::X),
        "y" => Some(Key::Y),
        "z" => Some(Key::Z),
        "0" => Some(Key::Num0),
        "1" => Some(Key::Num1),
        "2" => Some(Key::Num2),
        "3" => Some(Key::Num3),
        "4" => Some(Key::Num4),
        "5" => Some(Key::Num5),
        "6" => Some(Key::Num6),
        "7" => Some(Key::Num7),
        "8" => Some(Key::Num8),
        "9" => Some(Key::Num9),
        "enter" | "return" => Some(Key::Enter),
        "tab" => Some(Key::Tab),
        "escape" | "esc" => Some(Key::Escape),
        "space" => Some(Key::Space),
        "backspace" => Some(Key::Backspace),
        "delete" | "del" => Some(Key::Delete),
        "insert" | "ins" => Some(Key::Insert),
        "home" => Some(Key::Home),
        "end" => Some(Key::End),
        "pageup" | "pgup" => Some(Key::PageUp),
        "pagedown" | "pgdn" => Some(Key::PageDown),
        "up" => Some(Key::ArrowUp),
        "down" => Some(Key::ArrowDown),
        "left" => Some(Key::ArrowLeft),
        "right" => Some(Key::ArrowRight),
        "f1" => Some(Key::F1),
        "f2" => Some(Key::F2),
        "f3" => Some(Key::F3),
        "f4" => Some(Key::F4),
        "f5" => Some(Key::F5),
        "f6" => Some(Key::F6),
        "f7" => Some(Key::F7),
        "f8" => Some(Key::F8),
        "f9" => Some(Key::F9),
        "f10" => Some(Key::F10),
        "f11" => Some(Key::F11),
        "f12" => Some(Key::F12),
        "/" | "slash" => Some(Key::Slash),
        _ => None,
    }
}

/// Map an egui `Key` variant to its lowercase ASCII character, if applicable.
fn key_to_char(key: &Key) -> Option<char> {
    match key {
        Key::A => Some('a'),
        Key::B => Some('b'),
        Key::C => Some('c'),
        Key::D => Some('d'),
        Key::E => Some('e'),
        Key::F => Some('f'),
        Key::G => Some('g'),
        Key::H => Some('h'),
        Key::I => Some('i'),
        Key::J => Some('j'),
        Key::K => Some('k'),
        Key::L => Some('l'),
        Key::M => Some('m'),
        Key::N => Some('n'),
        Key::O => Some('o'),
        Key::P => Some('p'),
        Key::Q => Some('q'),
        Key::R => Some('r'),
        Key::S => Some('s'),
        Key::T => Some('t'),
        Key::U => Some('u'),
        Key::V => Some('v'),
        Key::W => Some('w'),
        Key::X => Some('x'),
        Key::Y => Some('y'),
        Key::Z => Some('z'),
        Key::OpenBracket => Some('['),
        Key::CloseBracket => Some(']'),
        Key::Backslash => Some('\\'),
        _ => None,
    }
}

/// Build an arrow-key escape sequence with modifier parameters.
///
/// In application cursor mode (no extra modifiers), arrows use SS3: `\x1bOA`.
/// With modifiers or in normal mode, arrows use CSI: `\x1b[A` or `\x1b[1;2A`.
fn arrow_key(dir: u8, modifiers: &Modifiers, app_cursor: bool) -> Vec<u8> {
    let modifier = modifier_param(modifiers);
    if modifier > 1 {
        // Modifiers always use CSI format, even in app cursor mode.
        format!("\x1b[1;{modifier}{}", dir as char).into_bytes()
    } else if app_cursor {
        vec![0x1b, b'O', dir]
    } else {
        vec![0x1b, b'[', dir]
    }
}

/// Compute the xterm modifier parameter (1 = none, +1 shift, +2 alt, +4 ctrl).
fn modifier_param(m: &Modifiers) -> u8 {
    let mut p = 1u8;
    if m.shift {
        p += 1;
    }
    if m.alt {
        p += 2;
    }
    if m.ctrl {
        p += 4;
    }
    p
}

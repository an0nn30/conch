//! Keyboard input translation from egui key events to terminal escape sequences.
//!
//! Handles Ctrl+key combos, named keys (arrows, function keys, etc.), and
//! modifier-aware sequences (Shift+Tab, Ctrl+Home, Alt+key).
//!
//! Also provides configurable `KeyBinding` parsing and `ResolvedShortcuts`.

use conch_core::config::KeyboardConfig;
use egui::{Key, Modifiers};

/// A parsed key binding (e.g. "cmd+t" -> Key::T + command modifier).
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

/// A resolved plugin keybinding — maps a key combo to a plugin action.
#[derive(Debug, Clone)]
pub struct ResolvedPluginKeybind {
    pub binding: KeyBinding,
    pub plugin_name: String,
    pub action: String,
}

/// Resolved set of app-level keyboard shortcuts.
#[derive(Debug, Clone)]
pub struct ResolvedShortcuts {
    pub new_tab: Option<KeyBinding>,
    pub close_tab: Option<KeyBinding>,
    pub quit: Option<KeyBinding>,
    pub new_window: Option<KeyBinding>,
    pub zen_mode: Option<KeyBinding>,
    pub toggle_left_panel: Option<KeyBinding>,
    pub toggle_right_panel: Option<KeyBinding>,
    pub toggle_bottom_panel: Option<KeyBinding>,
}

impl ResolvedShortcuts {
    /// Parse all shortcuts from the keyboard config.
    pub fn from_config(config: &KeyboardConfig) -> Self {
        Self {
            new_tab: KeyBinding::parse(&config.new_tab),
            close_tab: KeyBinding::parse(&config.close_tab),
            quit: KeyBinding::parse(&config.quit),
            new_window: KeyBinding::parse(&config.new_window),
            zen_mode: KeyBinding::parse(&config.zen_mode),
            toggle_left_panel: KeyBinding::parse(&config.toggle_left_panel),
            toggle_right_panel: KeyBinding::parse(&config.toggle_right_panel),
            toggle_bottom_panel: KeyBinding::parse(&config.toggle_bottom_panel),
        }
    }

    /// Check if the given key+modifiers match any app shortcut.
    pub fn is_app_shortcut(&self, key: &Key, modifiers: &Modifiers) -> bool {
        let bindings: [&Option<KeyBinding>; 8] = [
            &self.new_tab,
            &self.close_tab,
            &self.quit,
            &self.new_window,
            &self.zen_mode,
            &self.toggle_left_panel,
            &self.toggle_right_panel,
            &self.toggle_bottom_panel,
        ];
        if bindings.iter().any(|b| b.as_ref().is_some_and(|kb| kb.matches(key, modifiers))) {
            return true;
        }
        // Command+number for tab switching.
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
pub fn key_to_bytes(
    key: &Key,
    modifiers: &Modifiers,
    text: Option<&str>,
    shortcuts: &ResolvedShortcuts,
    app_cursor: bool,
    plugin_keybindings: &[ResolvedPluginKeybind],
) -> Option<Vec<u8>> {
    if shortcuts.is_app_shortcut(key, modifiers) {
        return None;
    }

    // Plugin keybindings should not be forwarded to PTY.
    if plugin_keybindings.iter().any(|pkb| pkb.binding.matches(key, modifiers)) {
        return None;
    }

    // Ctrl+letter -> ASCII control character.
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

fn parse_key_name(name: &str) -> Option<Key> {
    match name.to_lowercase().as_str() {
        "a" => Some(Key::A), "b" => Some(Key::B), "c" => Some(Key::C),
        "d" => Some(Key::D), "e" => Some(Key::E), "f" => Some(Key::F),
        "g" => Some(Key::G), "h" => Some(Key::H), "i" => Some(Key::I),
        "j" => Some(Key::J), "k" => Some(Key::K), "l" => Some(Key::L),
        "m" => Some(Key::M), "n" => Some(Key::N), "o" => Some(Key::O),
        "p" => Some(Key::P), "q" => Some(Key::Q), "r" => Some(Key::R),
        "s" => Some(Key::S), "t" => Some(Key::T), "u" => Some(Key::U),
        "v" => Some(Key::V), "w" => Some(Key::W), "x" => Some(Key::X),
        "y" => Some(Key::Y), "z" => Some(Key::Z),
        "0" => Some(Key::Num0), "1" => Some(Key::Num1), "2" => Some(Key::Num2),
        "3" => Some(Key::Num3), "4" => Some(Key::Num4), "5" => Some(Key::Num5),
        "6" => Some(Key::Num6), "7" => Some(Key::Num7), "8" => Some(Key::Num8),
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
        "f1" => Some(Key::F1), "f2" => Some(Key::F2), "f3" => Some(Key::F3),
        "f4" => Some(Key::F4), "f5" => Some(Key::F5), "f6" => Some(Key::F6),
        "f7" => Some(Key::F7), "f8" => Some(Key::F8), "f9" => Some(Key::F9),
        "f10" => Some(Key::F10), "f11" => Some(Key::F11), "f12" => Some(Key::F12),
        "/" | "slash" => Some(Key::Slash),
        _ => None,
    }
}

fn key_to_char(key: &Key) -> Option<char> {
    match key {
        Key::A => Some('a'), Key::B => Some('b'), Key::C => Some('c'),
        Key::D => Some('d'), Key::E => Some('e'), Key::F => Some('f'),
        Key::G => Some('g'), Key::H => Some('h'), Key::I => Some('i'),
        Key::J => Some('j'), Key::K => Some('k'), Key::L => Some('l'),
        Key::M => Some('m'), Key::N => Some('n'), Key::O => Some('o'),
        Key::P => Some('p'), Key::Q => Some('q'), Key::R => Some('r'),
        Key::S => Some('s'), Key::T => Some('t'), Key::U => Some('u'),
        Key::V => Some('v'), Key::W => Some('w'), Key::X => Some('x'),
        Key::Y => Some('y'), Key::Z => Some('z'),
        Key::OpenBracket => Some('['),
        Key::CloseBracket => Some(']'),
        Key::Backslash => Some('\\'),
        _ => None,
    }
}

fn arrow_key(dir: u8, modifiers: &Modifiers, app_cursor: bool) -> Vec<u8> {
    let modifier = modifier_param(modifiers);
    if modifier > 1 {
        format!("\x1b[1;{modifier}{}", dir as char).into_bytes()
    } else if app_cursor {
        vec![0x1b, b'O', dir]
    } else {
        vec![0x1b, b'[', dir]
    }
}

fn modifier_param(m: &Modifiers) -> u8 {
    let mut p = 1u8;
    if m.shift { p += 1; }
    if m.alt { p += 2; }
    if m.ctrl { p += 4; }
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_mods() -> Modifiers {
        Modifiers { alt: false, ctrl: false, shift: false, command: false, mac_cmd: false }
    }

    fn cmd() -> Modifiers {
        Modifiers { command: true, ..no_mods() }
    }

    fn ctrl() -> Modifiers {
        Modifiers { ctrl: true, ..no_mods() }
    }

    fn shift() -> Modifiers {
        Modifiers { shift: true, ..no_mods() }
    }

    fn alt() -> Modifiers {
        Modifiers { alt: true, ..no_mods() }
    }

    fn default_shortcuts() -> ResolvedShortcuts {
        ResolvedShortcuts::from_config(&conch_core::config::KeyboardConfig::default())
    }

    // -- parse_key_name --

    #[test]
    fn parse_key_name_letters() {
        assert_eq!(parse_key_name("a"), Some(Key::A));
        assert_eq!(parse_key_name("z"), Some(Key::Z));
        assert_eq!(parse_key_name("A"), Some(Key::A)); // Case insensitive.
    }

    #[test]
    fn parse_key_name_digits() {
        assert_eq!(parse_key_name("0"), Some(Key::Num0));
        assert_eq!(parse_key_name("9"), Some(Key::Num9));
    }

    #[test]
    fn parse_key_name_special_keys() {
        assert_eq!(parse_key_name("enter"), Some(Key::Enter));
        assert_eq!(parse_key_name("return"), Some(Key::Enter));
        assert_eq!(parse_key_name("tab"), Some(Key::Tab));
        assert_eq!(parse_key_name("escape"), Some(Key::Escape));
        assert_eq!(parse_key_name("esc"), Some(Key::Escape));
        assert_eq!(parse_key_name("space"), Some(Key::Space));
        assert_eq!(parse_key_name("backspace"), Some(Key::Backspace));
        assert_eq!(parse_key_name("delete"), Some(Key::Delete));
        assert_eq!(parse_key_name("del"), Some(Key::Delete));
        assert_eq!(parse_key_name("insert"), Some(Key::Insert));
        assert_eq!(parse_key_name("ins"), Some(Key::Insert));
        assert_eq!(parse_key_name("home"), Some(Key::Home));
        assert_eq!(parse_key_name("end"), Some(Key::End));
        assert_eq!(parse_key_name("pageup"), Some(Key::PageUp));
        assert_eq!(parse_key_name("pgup"), Some(Key::PageUp));
        assert_eq!(parse_key_name("pagedown"), Some(Key::PageDown));
        assert_eq!(parse_key_name("pgdn"), Some(Key::PageDown));
    }

    #[test]
    fn parse_key_name_arrows() {
        assert_eq!(parse_key_name("up"), Some(Key::ArrowUp));
        assert_eq!(parse_key_name("down"), Some(Key::ArrowDown));
        assert_eq!(parse_key_name("left"), Some(Key::ArrowLeft));
        assert_eq!(parse_key_name("right"), Some(Key::ArrowRight));
    }

    #[test]
    fn parse_key_name_function_keys() {
        assert_eq!(parse_key_name("f1"), Some(Key::F1));
        assert_eq!(parse_key_name("f12"), Some(Key::F12));
    }

    #[test]
    fn parse_key_name_slash() {
        assert_eq!(parse_key_name("/"), Some(Key::Slash));
        assert_eq!(parse_key_name("slash"), Some(Key::Slash));
    }

    #[test]
    fn parse_key_name_unknown() {
        assert_eq!(parse_key_name("foobar"), None);
        assert_eq!(parse_key_name(""), None);
    }

    // -- KeyBinding::parse --

    #[test]
    fn parse_cmd_t() {
        let kb = KeyBinding::parse("cmd+t").unwrap();
        assert_eq!(kb.key, Key::T);
        assert!(kb.command);
        assert!(!kb.alt);
        assert!(!kb.shift);
    }

    #[test]
    fn parse_ctrl_shift_n() {
        let kb = KeyBinding::parse("ctrl+shift+n").unwrap();
        assert_eq!(kb.key, Key::N);
        assert!(kb.command); // "ctrl" maps to command.
        assert!(kb.shift);
        assert!(!kb.alt);
    }

    #[test]
    fn parse_alt_option() {
        let kb = KeyBinding::parse("option+a").unwrap();
        assert!(kb.alt);
        assert!(!kb.command);
    }

    #[test]
    fn parse_super_meta() {
        let kb1 = KeyBinding::parse("super+q").unwrap();
        assert!(kb1.command);
        let kb2 = KeyBinding::parse("meta+q").unwrap();
        assert!(kb2.command);
    }

    #[test]
    fn parse_case_insensitive_modifiers() {
        let kb = KeyBinding::parse("CMD+SHIFT+Z").unwrap();
        assert_eq!(kb.key, Key::Z);
        assert!(kb.command);
        assert!(kb.shift);
    }

    #[test]
    fn parse_digit_binding() {
        let kb = KeyBinding::parse("cmd+1").unwrap();
        assert_eq!(kb.key, Key::Num1);
    }

    #[test]
    fn parse_empty_returns_none() {
        assert!(KeyBinding::parse("").is_none());
    }

    #[test]
    fn parse_invalid_key_returns_none() {
        assert!(KeyBinding::parse("cmd+foobar").is_none());
    }

    #[test]
    fn parse_no_key_only_modifiers() {
        assert!(KeyBinding::parse("cmd+shift").is_none());
    }

    // -- KeyBinding::matches --

    #[test]
    fn matches_exact() {
        let kb = KeyBinding::parse("cmd+t").unwrap();
        assert!(kb.matches(&Key::T, &cmd()));
    }

    #[test]
    fn matches_rejects_wrong_key() {
        let kb = KeyBinding::parse("cmd+t").unwrap();
        assert!(!kb.matches(&Key::W, &cmd()));
    }

    #[test]
    fn matches_rejects_extra_modifier() {
        let kb = KeyBinding::parse("cmd+t").unwrap();
        let mods = Modifiers { command: true, shift: true, ..no_mods() };
        assert!(!kb.matches(&Key::T, &mods));
    }

    #[test]
    fn matches_rejects_missing_modifier() {
        let kb = KeyBinding::parse("cmd+shift+t").unwrap();
        assert!(!kb.matches(&Key::T, &cmd()));
    }

    // -- ResolvedShortcuts --

    #[test]
    fn default_shortcuts_parse() {
        let s = default_shortcuts();
        assert!(s.new_tab.is_some());
        assert!(s.close_tab.is_some());
        assert!(s.quit.is_some());
        assert!(s.new_window.is_some());
        assert!(s.zen_mode.is_some());
    }

    #[test]
    fn is_app_shortcut_matches_new_tab() {
        let s = default_shortcuts();
        assert!(s.is_app_shortcut(&Key::T, &cmd()));
    }

    #[test]
    fn is_app_shortcut_matches_cmd_number() {
        let s = default_shortcuts();
        assert!(s.is_app_shortcut(&Key::Num1, &cmd()));
        assert!(s.is_app_shortcut(&Key::Num9, &cmd()));
    }

    #[test]
    fn is_app_shortcut_rejects_regular_key() {
        let s = default_shortcuts();
        assert!(!s.is_app_shortcut(&Key::A, &no_mods()));
    }

    // -- key_to_char --

    #[test]
    fn key_to_char_letters() {
        assert_eq!(key_to_char(&Key::A), Some('a'));
        assert_eq!(key_to_char(&Key::Z), Some('z'));
    }

    #[test]
    fn key_to_char_brackets() {
        assert_eq!(key_to_char(&Key::OpenBracket), Some('['));
        assert_eq!(key_to_char(&Key::CloseBracket), Some(']'));
        assert_eq!(key_to_char(&Key::Backslash), Some('\\'));
    }

    #[test]
    fn key_to_char_non_letter() {
        assert_eq!(key_to_char(&Key::Enter), None);
        assert_eq!(key_to_char(&Key::F1), None);
    }

    // -- modifier_param --

    #[test]
    fn modifier_param_none() {
        assert_eq!(modifier_param(&no_mods()), 1);
    }

    #[test]
    fn modifier_param_shift() {
        assert_eq!(modifier_param(&shift()), 2);
    }

    #[test]
    fn modifier_param_alt() {
        assert_eq!(modifier_param(&alt()), 3);
    }

    #[test]
    fn modifier_param_ctrl() {
        assert_eq!(modifier_param(&ctrl()), 5);
    }

    #[test]
    fn modifier_param_shift_alt() {
        let m = Modifiers { shift: true, alt: true, ..no_mods() };
        assert_eq!(modifier_param(&m), 4); // 1 + 1 + 2
    }

    #[test]
    fn modifier_param_all() {
        let m = Modifiers { shift: true, alt: true, ctrl: true, ..no_mods() };
        assert_eq!(modifier_param(&m), 8); // 1 + 1 + 2 + 4
    }

    // -- arrow_key --

    #[test]
    fn arrow_key_plain_no_app_cursor() {
        assert_eq!(arrow_key(b'A', &no_mods(), false), b"\x1b[A");
    }

    #[test]
    fn arrow_key_plain_app_cursor() {
        assert_eq!(arrow_key(b'A', &no_mods(), true), b"\x1bOA");
    }

    #[test]
    fn arrow_key_with_shift() {
        // modifier_param = 2, so: \x1b[1;2A
        assert_eq!(arrow_key(b'A', &shift(), false), b"\x1b[1;2A");
    }

    #[test]
    fn arrow_key_with_ctrl() {
        // modifier_param = 5, so: \x1b[1;5A
        assert_eq!(arrow_key(b'A', &ctrl(), false), b"\x1b[1;5A");
    }

    #[test]
    fn arrow_key_with_modifier_ignores_app_cursor() {
        // When modifier > 1, app_cursor is irrelevant.
        assert_eq!(
            arrow_key(b'B', &shift(), true),
            arrow_key(b'B', &shift(), false)
        );
    }

    // -- key_to_bytes --

    #[test]
    fn key_to_bytes_enter() {
        let s = default_shortcuts();
        assert_eq!(key_to_bytes(&Key::Enter, &no_mods(), None, &s, false, &[]), Some(b"\r".to_vec()));
    }

    #[test]
    fn key_to_bytes_backspace() {
        let s = default_shortcuts();
        assert_eq!(key_to_bytes(&Key::Backspace, &no_mods(), None, &s, false, &[]), Some(vec![0x7f]));
    }

    #[test]
    fn key_to_bytes_tab() {
        let s = default_shortcuts();
        assert_eq!(key_to_bytes(&Key::Tab, &no_mods(), None, &s, false, &[]), Some(b"\t".to_vec()));
    }

    #[test]
    fn key_to_bytes_shift_tab() {
        let s = default_shortcuts();
        assert_eq!(key_to_bytes(&Key::Tab, &shift(), None, &s, false, &[]), Some(b"\x1b[Z".to_vec()));
    }

    #[test]
    fn key_to_bytes_escape() {
        let s = default_shortcuts();
        assert_eq!(key_to_bytes(&Key::Escape, &no_mods(), None, &s, false, &[]), Some(vec![0x1b]));
    }

    #[test]
    fn key_to_bytes_ctrl_c() {
        let s = default_shortcuts();
        // Ctrl+C = ASCII 3 (ETX).
        assert_eq!(key_to_bytes(&Key::C, &ctrl(), None, &s, false, &[]), Some(vec![3]));
    }

    #[test]
    fn key_to_bytes_ctrl_a() {
        let s = default_shortcuts();
        assert_eq!(key_to_bytes(&Key::A, &ctrl(), None, &s, false, &[]), Some(vec![1]));
    }

    #[test]
    fn key_to_bytes_ctrl_z() {
        let s = default_shortcuts();
        assert_eq!(key_to_bytes(&Key::Z, &ctrl(), None, &s, false, &[]), Some(vec![26]));
    }

    #[test]
    fn key_to_bytes_ctrl_bracket_escape() {
        let s = default_shortcuts();
        // Ctrl+[ = ESC (0x1b).
        assert_eq!(key_to_bytes(&Key::OpenBracket, &ctrl(), None, &s, false, &[]), Some(vec![0x1b]));
    }

    #[test]
    fn key_to_bytes_ctrl_backslash() {
        let s = default_shortcuts();
        assert_eq!(key_to_bytes(&Key::Backslash, &ctrl(), None, &s, false, &[]), Some(vec![0x1c]));
    }

    #[test]
    fn key_to_bytes_delete() {
        let s = default_shortcuts();
        assert_eq!(key_to_bytes(&Key::Delete, &no_mods(), None, &s, false, &[]), Some(b"\x1b[3~".to_vec()));
    }

    #[test]
    fn key_to_bytes_home_normal() {
        let s = default_shortcuts();
        assert_eq!(key_to_bytes(&Key::Home, &no_mods(), None, &s, false, &[]), Some(b"\x1b[H".to_vec()));
    }

    #[test]
    fn key_to_bytes_home_app_cursor() {
        let s = default_shortcuts();
        assert_eq!(key_to_bytes(&Key::Home, &no_mods(), None, &s, true, &[]), Some(b"\x1bOH".to_vec()));
    }

    #[test]
    fn key_to_bytes_home_ctrl() {
        let s = default_shortcuts();
        assert_eq!(key_to_bytes(&Key::Home, &ctrl(), None, &s, false, &[]), Some(b"\x1b[1;5H".to_vec()));
    }

    #[test]
    fn key_to_bytes_end_normal() {
        let s = default_shortcuts();
        assert_eq!(key_to_bytes(&Key::End, &no_mods(), None, &s, false, &[]), Some(b"\x1b[F".to_vec()));
    }

    #[test]
    fn key_to_bytes_page_up_down() {
        let s = default_shortcuts();
        assert_eq!(key_to_bytes(&Key::PageUp, &no_mods(), None, &s, false, &[]), Some(b"\x1b[5~".to_vec()));
        assert_eq!(key_to_bytes(&Key::PageDown, &no_mods(), None, &s, false, &[]), Some(b"\x1b[6~".to_vec()));
    }

    #[test]
    fn key_to_bytes_function_keys() {
        let s = default_shortcuts();
        assert_eq!(key_to_bytes(&Key::F1, &no_mods(), None, &s, false, &[]), Some(b"\x1bOP".to_vec()));
        assert_eq!(key_to_bytes(&Key::F5, &no_mods(), None, &s, false, &[]), Some(b"\x1b[15~".to_vec()));
        assert_eq!(key_to_bytes(&Key::F12, &no_mods(), None, &s, false, &[]), Some(b"\x1b[24~".to_vec()));
    }

    #[test]
    fn key_to_bytes_arrow_keys() {
        let s = default_shortcuts();
        assert_eq!(key_to_bytes(&Key::ArrowUp, &no_mods(), None, &s, false, &[]), Some(b"\x1b[A".to_vec()));
        assert_eq!(key_to_bytes(&Key::ArrowDown, &no_mods(), None, &s, false, &[]), Some(b"\x1b[B".to_vec()));
    }

    #[test]
    fn key_to_bytes_printable_text() {
        let s = default_shortcuts();
        assert_eq!(key_to_bytes(&Key::A, &no_mods(), Some("a"), &s, false, &[]), Some(b"a".to_vec()));
    }

    #[test]
    fn key_to_bytes_alt_text() {
        let s = default_shortcuts();
        // Alt+a = ESC + "a".
        assert_eq!(key_to_bytes(&Key::A, &alt(), Some("a"), &s, false, &[]), Some(vec![0x1b, b'a']));
    }

    #[test]
    fn key_to_bytes_app_shortcut_returns_none() {
        let s = default_shortcuts();
        // Cmd+T is new_tab shortcut — should not produce bytes.
        assert_eq!(key_to_bytes(&Key::T, &cmd(), Some("t"), &s, false, &[]), None);
    }

    #[test]
    fn key_to_bytes_cmd_number_returns_none() {
        let s = default_shortcuts();
        // Cmd+1 is tab switch — should not produce bytes.
        assert_eq!(key_to_bytes(&Key::Num1, &cmd(), None, &s, false, &[]), None);
    }
}

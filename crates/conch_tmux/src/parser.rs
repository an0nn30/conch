//! Control mode output parser.
//!
//! Tmux control mode emits lines prefixed with `%`. This parser accumulates
//! bytes, splits on newlines, and converts each `%`-line into a typed
//! [`Notification`].

use std::collections::HashMap;

use crate::protocol::Notification;

/// Parses raw bytes from a `tmux -CC` process into typed notifications.
pub struct ControlModeParser {
    /// Accumulates bytes until a complete line (ending in `\n`) is available.
    buffer: Vec<u8>,
    /// Tracks in-progress command responses between %begin and %end.
    pending_responses: HashMap<u64, Vec<String>>,
}

impl ControlModeParser {
    pub fn new() -> Self {
        Self {
            buffer: Vec::new(),
            pending_responses: HashMap::new(),
        }
    }

    /// Feed raw bytes from the control mode stream. Returns any complete
    /// notifications that were parsed from complete lines in the input.
    pub fn feed(&mut self, data: &[u8]) -> Vec<Notification> {
        self.buffer.extend_from_slice(data);
        let mut notifications = Vec::new();

        loop {
            let newline_pos = match self.buffer.iter().position(|&b| b == b'\n') {
                Some(pos) => pos,
                None => break,
            };

            let line_bytes: Vec<u8> = self.buffer.drain(..=newline_pos).collect();
            let line = String::from_utf8_lossy(&line_bytes).trim_end().to_string();

            if line.is_empty() {
                continue;
            }

            if let Some(notif) = self.parse_line(&line) {
                notifications.push(notif);
            }
        }

        notifications
    }

    fn parse_line(&mut self, line: &str) -> Option<Notification> {
        if !line.starts_with('%') {
            for lines in self.pending_responses.values_mut() {
                lines.push(line.to_string());
            }
            return None;
        }

        let (name, rest) = match line[1..].split_once(' ') {
            Some((n, r)) => (n, r),
            None => (&line[1..], ""),
        };

        match name {
            "sessions-changed" => Some(Notification::SessionsChanged),

            "session-changed" => {
                let (id, name) = parse_id_and_name(rest, '$')?;
                Some(Notification::SessionChanged { session_id: id, name })
            }

            "session-renamed" => {
                let (id, name) = parse_id_and_name(rest, '$')?;
                Some(Notification::SessionRenamed { session_id: id, name })
            }

            "session-window-changed" => {
                let (session_id, window_part) = parse_id_and_name(rest, '$')?;
                let window_id = parse_prefixed_id(&window_part, '@')?;
                Some(Notification::SessionWindowChanged { session_id, window_id })
            }

            "window-add" | "unlinked-window-add" => {
                let id = parse_prefixed_id(rest.trim(), '@')?;
                Some(Notification::WindowAdd { window_id: id })
            }

            "window-close" | "unlinked-window-close" => {
                let id = parse_prefixed_id(rest.trim(), '@')?;
                Some(Notification::WindowClose { window_id: id })
            }

            "window-renamed" | "unlinked-window-renamed" => {
                let (id, name) = parse_id_and_name(rest, '@')?;
                Some(Notification::WindowRenamed { window_id: id, name })
            }

            "window-pane-changed" => {
                let (window_id, pane_part) = parse_id_and_name(rest, '@')?;
                let pane_id = parse_prefixed_id(&pane_part, '%')?;
                Some(Notification::WindowPaneChanged { window_id, pane_id })
            }

            "pane-mode-changed" => {
                let parts: Vec<&str> = rest.split_whitespace().collect();
                let pane_id = parse_prefixed_id(parts.first()?, '%')?;
                let mode = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
                Some(Notification::PaneModeChanged { pane_id, mode })
            }

            "output" => {
                // 3.1 and earlier:
                //   %output %<pane id> <data...>
                // 3.2+ with pause mode:
                //   %output %<pane id> <latency> <data...>
                let (pane_id, data) = parse_output_payload(rest)?;
                Some(Notification::Output { pane_id, data })
            }

            "extended-output" => {
                // 3.2+:
                //   %extended-output %<pane id> <latency> [flags...] : <data...>
                let (pane_id, data) = parse_extended_output_payload(rest)?;
                Some(Notification::Output { pane_id, data })
            }

            "layout-change" => {
                let (id, layout) = parse_id_and_name(rest, '@')?;
                Some(Notification::LayoutChange { window_id: id, layout })
            }

            "begin" => {
                let parts: Vec<&str> = rest.split_whitespace().collect();
                let ts_and_cmd = parts.first()?;
                let command_number: u64 = ts_and_cmd.parse().ok()?;
                let flags: u32 = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
                self.pending_responses.entry(command_number).or_default();
                Some(Notification::Begin { command_number, flags })
            }

            "end" => {
                let parts: Vec<&str> = rest.split_whitespace().collect();
                let command_number: u64 = parts.first()?.parse().ok()?;
                let flags: u32 = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
                self.pending_responses.remove(&command_number);
                Some(Notification::End { command_number, flags })
            }

            "error" => {
                let parts: Vec<&str> = rest.splitn(2, ' ').collect();
                let command_number: u64 = parts.first()?.parse().ok()?;
                let message = parts.get(1).unwrap_or(&"").to_string();
                self.pending_responses.remove(&command_number);
                Some(Notification::Error { command_number, message })
            }

            "exit" => {
                let reason = if rest.is_empty() { None } else { Some(rest.to_string()) };
                Some(Notification::Exit { reason })
            }

            _ => Some(Notification::Unknown {
                name: name.to_string(),
                args: rest.to_string(),
            }),
        }
    }

    /// Collect a completed command response. Call after receiving an `End`
    /// or `Error` notification to retrieve the accumulated output lines.
    pub fn take_response(&mut self, command_number: u64) -> Option<Vec<String>> {
        self.pending_responses.remove(&command_number)
    }
}

impl Default for ControlModeParser {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Parse a tmux ID like `$1`, `@2`, or `%3` — returns the numeric part.
fn parse_prefixed_id(s: &str, prefix: char) -> Option<u64> {
    s.strip_prefix(prefix)?.parse().ok()
}

/// Parse `"$1 session-name"` or `"@2 window-name"` into `(id, name)`.
fn parse_id_and_name(s: &str, prefix: char) -> Option<(u64, String)> {
    let (id_part, name_part) = s.split_once(' ')?;
    let id = parse_prefixed_id(id_part, prefix)?;
    Some((id, name_part.to_string()))
}

/// Decode tmux octal escapes in `%output` data.
///
/// Tmux escapes non-printable bytes as `\OOO` (three-digit octal).
fn decode_octal_escapes(s: &str) -> Vec<u8> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 3 < bytes.len() {
            let d1 = bytes[i + 1];
            let d2 = bytes[i + 2];
            let d3 = bytes[i + 3];
            if d1.is_ascii_digit() && d2.is_ascii_digit() && d3.is_ascii_digit() {
                let val = (d1 - b'0') as u16 * 64 + (d2 - b'0') as u16 * 8 + (d3 - b'0') as u16;
                if val <= 255 {
                    out.push(val as u8);
                    i += 4;
                    continue;
                }
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    out
}

fn parse_output_payload(rest: &str) -> Option<(u64, Vec<u8>)> {
    let trimmed = rest.trim_start();
    if trimmed.is_empty() {
        return None;
    }
    let (pane_part, mut remaining) = match trimmed.split_once(' ') {
        Some((pane, rest)) => (pane, rest.trim_start()),
        None => (trimmed, ""),
    };
    let pane_id = parse_prefixed_id(pane_part, '%')?;

    // Optional latency token in pause mode.
    if let Some((token, rest_after_token)) = remaining.split_once(' ') {
        if !token.is_empty() && token.chars().all(|c| c.is_ascii_digit()) {
            remaining = rest_after_token.trim_start();
        }
    }

    Some((pane_id, decode_octal_escapes(remaining)))
}

fn parse_extended_output_payload(rest: &str) -> Option<(u64, Vec<u8>)> {
    let trimmed = rest.trim_start();
    if trimmed.is_empty() {
        return None;
    }

    let (pane_part, remaining) = trimmed.split_once(' ')?;
    let pane_id = parse_prefixed_id(pane_part, '%')?;

    // Must include latency token first.
    let (_latency, rest_after_latency) = remaining.trim_start().split_once(' ')?;
    let rest_after_latency = rest_after_latency.trim_start();

    // Skip optional flags until ": ".
    let (_metadata, encoded) = rest_after_latency.split_once(": ")?;
    Some((pane_id, decode_octal_escapes(encoded)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_sessions_changed() {
        let mut p = ControlModeParser::new();
        let notifs = p.feed(b"%sessions-changed\n");
        assert_eq!(notifs, vec![Notification::SessionsChanged]);
    }

    #[test]
    fn parse_session_changed() {
        let mut p = ControlModeParser::new();
        let notifs = p.feed(b"%session-changed $1 my-session\n");
        assert_eq!(notifs, vec![Notification::SessionChanged { session_id: 1, name: "my-session".into() }]);
    }

    #[test]
    fn parse_session_renamed() {
        let mut p = ControlModeParser::new();
        let notifs = p.feed(b"%session-renamed $3 new-name\n");
        assert_eq!(notifs, vec![Notification::SessionRenamed { session_id: 3, name: "new-name".into() }]);
    }

    #[test]
    fn parse_session_window_changed() {
        let mut p = ControlModeParser::new();
        let notifs = p.feed(b"%session-window-changed $1 @2\n");
        assert_eq!(notifs, vec![Notification::SessionWindowChanged { session_id: 1, window_id: 2 }]);
    }

    #[test]
    fn parse_window_add() {
        let mut p = ControlModeParser::new();
        let notifs = p.feed(b"%window-add @5\n");
        assert_eq!(notifs, vec![Notification::WindowAdd { window_id: 5 }]);
    }

    #[test]
    fn parse_window_close() {
        let mut p = ControlModeParser::new();
        let notifs = p.feed(b"%window-close @3\n");
        assert_eq!(notifs, vec![Notification::WindowClose { window_id: 3 }]);
    }

    #[test]
    fn parse_unlinked_window_close() {
        let mut p = ControlModeParser::new();
        let notifs = p.feed(b"%unlinked-window-close @3\n");
        assert_eq!(notifs, vec![Notification::WindowClose { window_id: 3 }]);
    }

    #[test]
    fn parse_window_renamed() {
        let mut p = ControlModeParser::new();
        let notifs = p.feed(b"%window-renamed @1 my-window\n");
        assert_eq!(notifs, vec![Notification::WindowRenamed { window_id: 1, name: "my-window".into() }]);
    }

    #[test]
    fn parse_unlinked_window_renamed() {
        let mut p = ControlModeParser::new();
        let notifs = p.feed(b"%unlinked-window-renamed @1 my-window\n");
        assert_eq!(notifs, vec![Notification::WindowRenamed { window_id: 1, name: "my-window".into() }]);
    }

    #[test]
    fn parse_window_pane_changed() {
        let mut p = ControlModeParser::new();
        let notifs = p.feed(b"%window-pane-changed @1 %3\n");
        assert_eq!(notifs, vec![Notification::WindowPaneChanged { window_id: 1, pane_id: 3 }]);
    }

    #[test]
    fn parse_pane_mode_changed() {
        let mut p = ControlModeParser::new();
        let notifs = p.feed(b"%pane-mode-changed %2 1\n");
        assert_eq!(notifs, vec![Notification::PaneModeChanged { pane_id: 2, mode: 1 }]);
    }

    #[test]
    fn parse_layout_change() {
        let mut p = ControlModeParser::new();
        let notifs = p.feed(b"%layout-change @1 abc123,80x24,0,0\n");
        assert_eq!(notifs, vec![Notification::LayoutChange { window_id: 1, layout: "abc123,80x24,0,0".into() }]);
    }

    #[test]
    fn parse_exit_no_reason() {
        let mut p = ControlModeParser::new();
        let notifs = p.feed(b"%exit\n");
        assert_eq!(notifs, vec![Notification::Exit { reason: None }]);
    }

    #[test]
    fn parse_exit_with_reason() {
        let mut p = ControlModeParser::new();
        let notifs = p.feed(b"%exit server exited\n");
        assert_eq!(notifs, vec![Notification::Exit { reason: Some("server exited".into()) }]);
    }

    #[test]
    fn parse_unknown_notification() {
        let mut p = ControlModeParser::new();
        let notifs = p.feed(b"%future-thing arg1 arg2\n");
        assert_eq!(notifs, vec![Notification::Unknown { name: "future-thing".into(), args: "arg1 arg2".into() }]);
    }

    #[test]
    fn parse_output_plain_text() {
        let mut p = ControlModeParser::new();
        let notifs = p.feed(b"%output %0 hello world\n");
        assert_eq!(notifs, vec![Notification::Output { pane_id: 0, data: b"hello world".to_vec() }]);
    }

    #[test]
    fn parse_output_octal_escapes() {
        let mut p = ControlModeParser::new();
        let notifs = p.feed(b"%output %1 \\033[31mred\\012\n");
        assert_eq!(notifs, vec![Notification::Output { pane_id: 1, data: vec![0x1b, b'[', b'3', b'1', b'm', b'r', b'e', b'd', 0x0a] }]);
    }

    #[test]
    fn parse_output_empty_data() {
        let mut p = ControlModeParser::new();
        let notifs = p.feed(b"%output %5 \n");
        assert_eq!(notifs, vec![Notification::Output { pane_id: 5, data: Vec::new() }]);
    }

    #[test]
    fn parse_output_with_latency_token() {
        let mut p = ControlModeParser::new();
        let notifs = p.feed(b"%output %5 123 \\033[31mred\\012\n");
        assert_eq!(
            notifs,
            vec![Notification::Output {
                pane_id: 5,
                data: vec![0x1b, b'[', b'3', b'1', b'm', b'r', b'e', b'd', 0x0a],
            }]
        );
    }

    #[test]
    fn parse_extended_output() {
        let mut p = ControlModeParser::new();
        let notifs = p.feed(b"%extended-output %11 3129 : \\033[32mok\\012\n");
        assert_eq!(
            notifs,
            vec![Notification::Output {
                pane_id: 11,
                data: vec![0x1b, b'[', b'3', b'2', b'm', b'o', b'k', 0x0a],
            }]
        );
    }

    #[test]
    fn parse_begin_end() {
        let mut p = ControlModeParser::new();
        let notifs = p.feed(b"%begin 1234 0\n%end 1234 0\n");
        assert_eq!(notifs, vec![
            Notification::Begin { command_number: 1234, flags: 0 },
            Notification::End { command_number: 1234, flags: 0 },
        ]);
    }

    #[test]
    fn parse_error_response() {
        let mut p = ControlModeParser::new();
        let notifs = p.feed(b"%error 42 session not found\n");
        assert_eq!(notifs, vec![Notification::Error { command_number: 42, message: "session not found".into() }]);
    }

    #[test]
    fn partial_line_across_feeds() {
        let mut p = ControlModeParser::new();
        let n1 = p.feed(b"%sessions-");
        assert!(n1.is_empty(), "incomplete line should yield nothing");
        let n2 = p.feed(b"changed\n");
        assert_eq!(n2, vec![Notification::SessionsChanged]);
    }

    #[test]
    fn multiple_notifications_one_feed() {
        let mut p = ControlModeParser::new();
        let notifs = p.feed(b"%sessions-changed\n%window-add @1\n%window-add @2\n");
        assert_eq!(notifs, vec![
            Notification::SessionsChanged,
            Notification::WindowAdd { window_id: 1 },
            Notification::WindowAdd { window_id: 2 },
        ]);
    }

    #[test]
    fn empty_feed() {
        let mut p = ControlModeParser::new();
        let notifs = p.feed(b"");
        assert!(notifs.is_empty());
    }

    #[test]
    fn blank_lines_ignored() {
        let mut p = ControlModeParser::new();
        let notifs = p.feed(b"\n\n%sessions-changed\n\n");
        assert_eq!(notifs, vec![Notification::SessionsChanged]);
    }

    #[test]
    fn interleaved_output_and_notifications() {
        let mut p = ControlModeParser::new();
        let notifs = p.feed(b"%output %0 data\n%window-add @1\n%output %0 more\n");
        assert_eq!(notifs.len(), 3);
        assert!(matches!(notifs[0], Notification::Output { pane_id: 0, .. }));
        assert_eq!(notifs[1], Notification::WindowAdd { window_id: 1 });
        assert!(matches!(notifs[2], Notification::Output { pane_id: 0, .. }));
    }

    #[test]
    fn decode_octal_simple() {
        assert_eq!(decode_octal_escapes("abc"), b"abc");
    }

    #[test]
    fn decode_octal_escape_sequence() {
        assert_eq!(decode_octal_escapes("\\033"), vec![0x1b]);
    }

    #[test]
    fn decode_octal_mixed() {
        assert_eq!(decode_octal_escapes("A\\101B"), vec![b'A', 0x41, b'B']);
    }

    #[test]
    fn decode_octal_backslash_not_followed_by_digits() {
        assert_eq!(decode_octal_escapes("a\\bc"), b"a\\bc");
    }

    #[test]
    fn decode_octal_empty() {
        assert_eq!(decode_octal_escapes(""), b"");
    }
}

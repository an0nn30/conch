//! Accumulates raw bytes and yields only complete UTF-8 text.
//!
//! PTY and SSH channels deliver data in arbitrary chunks that can split
//! multi-byte UTF-8 sequences at buffer boundaries.  `String::from_utf8_lossy`
//! replaces the orphaned bytes with U+FFFD, corrupting characters like
//! box-drawing `─` (U+2500, 3 bytes).
//!
//! [`Utf8Accumulator`] holds back any trailing partial sequence and prepends
//! it to the next chunk, so the output is always cleanly split on character
//! boundaries.

/// Buffers raw bytes and emits complete UTF-8, holding partial trailing
/// multi-byte sequences for the next push.
pub(crate) struct Utf8Accumulator {
    pending: Vec<u8>,
}

impl Utf8Accumulator {
    pub fn new() -> Self {
        Self {
            pending: Vec::new(),
        }
    }

    /// Append `data` and return all complete UTF-8 text.
    ///
    /// Any incomplete multi-byte sequence at the very end is kept internally
    /// and will be prepended to the next call.  Invalid bytes in the middle
    /// are still replaced with U+FFFD (same as `from_utf8_lossy`).
    pub fn push(&mut self, data: &[u8]) -> String {
        self.pending.extend_from_slice(data);

        let trailing = trailing_partial_utf8(&self.pending);
        let emit_end = self.pending.len() - trailing;

        if emit_end == 0 {
            return String::new();
        }

        let text = String::from_utf8_lossy(&self.pending[..emit_end]).into_owned();
        self.pending.drain(..emit_end);
        text
    }
}

/// Count bytes at the tail of `buf` that form an incomplete UTF-8 sequence.
///
/// Scans backwards (up to 3 bytes) for a lead byte whose expected multi-byte
/// sequence extends past the end of the buffer.
fn trailing_partial_utf8(buf: &[u8]) -> usize {
    let len = buf.len();
    for back in 1..=3usize.min(len) {
        let b = buf[len - back];
        let seq_len = match b {
            0xC0..=0xDF => 2,
            0xE0..=0xEF => 3,
            0xF0..=0xF7 => 4,
            _ => continue, // ASCII or continuation byte — keep scanning
        };
        if seq_len > back {
            // Lead byte found but sequence is incomplete.
            return back;
        }
        // Lead byte whose sequence IS complete — no trailing partial.
        return 0;
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_ascii() {
        let mut acc = Utf8Accumulator::new();
        assert_eq!(acc.push(b"hello world"), "hello world");
    }

    #[test]
    fn complete_multibyte() {
        let mut acc = Utf8Accumulator::new();
        // U+2500 BOX DRAWINGS LIGHT HORIZONTAL = E2 94 80
        let input = "─── line ───";
        assert_eq!(acc.push(input.as_bytes()), input);
    }

    #[test]
    fn split_three_byte_char() {
        let mut acc = Utf8Accumulator::new();
        // U+2500 = E2 94 80
        // Split after first 2 bytes
        let text1 = acc.push(&[0xE2, 0x94]);
        assert_eq!(text1, "", "Incomplete sequence should be held back");

        let text2 = acc.push(&[0x80, b'x']);
        assert_eq!(text2, "\u{2500}x", "Completed sequence should be emitted");
    }

    #[test]
    fn split_two_byte_char() {
        let mut acc = Utf8Accumulator::new();
        // U+00E9 (é) = C3 A9
        let text1 = acc.push(&[b'a', 0xC3]);
        assert_eq!(text1, "a");

        let text2 = acc.push(&[0xA9, b'b']);
        assert_eq!(text2, "\u{00E9}b");
    }

    #[test]
    fn split_four_byte_char() {
        let mut acc = Utf8Accumulator::new();
        // U+1F600 (😀) = F0 9F 98 80
        let text1 = acc.push(&[b'a', 0xF0, 0x9F]);
        assert_eq!(text1, "a");

        let text2 = acc.push(&[0x98, 0x80, b'b']);
        assert_eq!(text2, "\u{1F600}b");
    }

    #[test]
    fn multiple_splits_in_sequence() {
        let mut acc = Utf8Accumulator::new();
        // First chunk: ASCII + start of U+2500
        assert_eq!(acc.push(&[b'a', 0xE2]), "a");
        // Second chunk: middle byte only
        assert_eq!(acc.push(&[0x94]), "");
        // Third chunk: final byte + ASCII
        assert_eq!(acc.push(&[0x80, b'z']), "\u{2500}z");
    }

    #[test]
    fn invalid_bytes_in_middle_still_replaced() {
        let mut acc = Utf8Accumulator::new();
        // 0xFF is never valid UTF-8
        let text = acc.push(&[b'a', 0xFF, b'b']);
        assert_eq!(text, "a\u{FFFD}b");
    }

    #[test]
    fn empty_input() {
        let mut acc = Utf8Accumulator::new();
        assert_eq!(acc.push(&[]), "");
    }

    #[test]
    fn only_partial_sequence() {
        let mut acc = Utf8Accumulator::new();
        assert_eq!(acc.push(&[0xE2, 0x94]), "");
        // Complete it
        assert_eq!(acc.push(&[0x80]), "\u{2500}");
    }

    #[test]
    fn realistic_pty_output_with_box_drawing() {
        let mut acc = Utf8Accumulator::new();

        // Simulate: "──" split across a buffer boundary
        // U+2500 U+2500 = E2 94 80 E2 94 80
        // Buffer boundary falls after the 4th byte (mid-second char)
        let chunk1 = &[0xE2, 0x94, 0x80, 0xE2];
        let chunk2 = &[0x94, 0x80];

        let text1 = acc.push(chunk1);
        assert_eq!(text1, "\u{2500}", "First complete char emitted");

        let text2 = acc.push(chunk2);
        assert_eq!(text2, "\u{2500}", "Second char completed and emitted");
    }
}

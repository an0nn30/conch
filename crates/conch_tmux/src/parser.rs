//! Control mode output parser.

use crate::protocol::Notification;

/// Parses raw bytes from a `tmux -CC` process into typed notifications.
pub struct ControlModeParser {
    buffer: Vec<u8>,
}

impl ControlModeParser {
    pub fn new() -> Self {
        Self { buffer: Vec::new() }
    }

    pub fn feed(&mut self, _data: &[u8]) -> Vec<Notification> {
        Vec::new()
    }
}

impl Default for ControlModeParser {
    fn default() -> Self {
        Self::new()
    }
}

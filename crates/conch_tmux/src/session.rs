//! In-memory tmux session model.

/// A tmux session.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct TmuxSession {
    pub id: u64,
    pub name: String,
    pub window_count: usize,
    pub attached: bool,
    pub created: Option<u64>,
}

/// Tracks the set of known tmux sessions.
#[derive(Debug, Clone, Default)]
pub struct SessionList {
    sessions: Vec<TmuxSession>,
}

impl SessionList {
    pub fn sessions(&self) -> &[TmuxSession] {
        &self.sessions
    }
}

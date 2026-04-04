//! Serializable event payloads for tmux → frontend communication.

use serde::Serialize;

#[derive(Clone, Serialize)]
pub(crate) struct TmuxSessionInfo {
    pub id: u64,
    pub name: String,
    pub window_count: usize,
    pub attached: bool,
    pub created: Option<u64>,
}

impl From<&conch_tmux::TmuxSession> for TmuxSessionInfo {
    fn from(s: &conch_tmux::TmuxSession) -> Self {
        Self {
            id: s.id,
            name: s.name.clone(),
            window_count: s.window_count,
            attached: s.attached,
            created: s.created,
        }
    }
}

#[derive(Clone, Serialize)]
pub(crate) struct TmuxPaneInfo {
    pub id: u64,
    pub active: bool,
    pub width: u16,
    pub height: u16,
    pub left: u16,
    pub top: u16,
    pub content: String,
}

#[derive(Clone, Serialize)]
pub(crate) struct TmuxWindowInfo {
    pub id: u64,
    pub name: String,
    pub active: bool,
    pub panes: Vec<TmuxPaneInfo>,
}

#[derive(Clone, Serialize)]
pub(crate) struct TmuxConnectedEvent {
    pub session: String,
}

#[derive(Clone, Serialize)]
pub(crate) struct TmuxDisconnectedEvent {
    pub reason: Option<String>,
}

#[derive(Clone, Serialize)]
pub(crate) struct TmuxOutputEvent {
    pub pane_id: u64,
    pub data: String,
}

#[derive(Clone, Serialize)]
pub(crate) struct TmuxWindowEvent {
    pub window_id: u64,
    pub name: Option<String>,
}

#[derive(Clone, Serialize)]
pub(crate) struct TmuxPaneEvent {
    pub window_id: Option<u64>,
    pub pane_id: u64,
}

#[derive(Clone, Serialize)]
pub(crate) struct TmuxLayoutEvent {
    pub window_id: u64,
    pub layout: String,
}

#[derive(Clone, Serialize)]
pub(crate) struct TmuxSessionsChangedEvent {
    pub sessions: Vec<TmuxSessionInfo>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tmux_session_info_from_tmux_session() {
        let session = conch_tmux::TmuxSession {
            id: 1,
            name: "work".into(),
            window_count: 3,
            attached: true,
            created: Some(1711990800),
        };
        let info = TmuxSessionInfo::from(&session);
        assert_eq!(info.id, 1);
        assert_eq!(info.name, "work");
        assert_eq!(info.window_count, 3);
        assert!(info.attached);
        assert_eq!(info.created, Some(1711990800));
    }

    #[test]
    fn tmux_session_info_from_session_no_created() {
        let session = conch_tmux::TmuxSession {
            id: 2,
            name: "scratch".into(),
            window_count: 1,
            attached: false,
            created: None,
        };
        let info = TmuxSessionInfo::from(&session);
        assert_eq!(info.id, 2);
        assert!(!info.attached);
        assert!(info.created.is_none());
    }
}

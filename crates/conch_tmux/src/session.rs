//! In-memory tmux session model.

use crate::protocol::Notification;

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
    pub fn new() -> Self {
        Self::default()
    }

    pub fn sessions(&self) -> &[TmuxSession] {
        &self.sessions
    }

    pub fn update_from_list_output(&mut self, raw: &str) {
        let mut new_sessions = Vec::new();
        for line in raw.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if let Some(session) = parse_session_line(line) {
                new_sessions.push(session);
            }
        }
        self.sessions = new_sessions;
    }

    pub fn apply_notification(&mut self, notif: &Notification) {
        match notif {
            Notification::SessionsChanged => {}
            Notification::SessionRenamed { session_id, name } => {
                if let Some(s) = self.sessions.iter_mut().find(|s| s.id == *session_id) {
                    s.name = name.clone();
                }
            }
            Notification::SessionChanged { session_id, name } => {
                for s in &mut self.sessions {
                    s.attached = s.id == *session_id;
                }
                if let Some(s) = self.sessions.iter_mut().find(|s| s.id == *session_id) {
                    s.name = name.clone();
                }
            }
            Notification::WindowAdd { .. } => {
                if let Some(s) = self.sessions.iter_mut().find(|s| s.attached) {
                    s.window_count += 1;
                }
            }
            Notification::WindowClose { .. } => {
                if let Some(s) = self.sessions.iter_mut().find(|s| s.attached) {
                    s.window_count = s.window_count.saturating_sub(1);
                }
            }
            _ => {}
        }
    }
}

fn parse_session_line(line: &str) -> Option<TmuxSession> {
    let parts: Vec<&str> = line.splitn(5, ' ').collect();
    if parts.len() < 4 {
        return None;
    }
    let id = parts[0].strip_prefix('$')?.parse().ok()?;
    let name = parts[1].to_string();
    let window_count = parts[2].parse().ok()?;
    let attached = parts[3] == "1";
    let created = parts.get(4).and_then(|s| s.parse().ok());

    Some(TmuxSession {
        id,
        name,
        window_count,
        attached,
        created,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_session_list() {
        let list = SessionList::new();
        assert!(list.sessions().is_empty());
    }

    #[test]
    fn update_from_list_output() {
        let mut list = SessionList::new();
        list.update_from_list_output("$1 work 3 1 1711990800\n$2 scratch 1 0 1711990900\n");
        assert_eq!(list.sessions().len(), 2);
        assert_eq!(list.sessions()[0].name, "work");
        assert_eq!(list.sessions()[0].window_count, 3);
        assert!(list.sessions()[0].attached);
        assert_eq!(list.sessions()[1].name, "scratch");
        assert!(!list.sessions()[1].attached);
    }

    #[test]
    fn update_replaces_previous() {
        let mut list = SessionList::new();
        list.update_from_list_output("$1 old 1 0 0\n");
        assert_eq!(list.sessions().len(), 1);
        list.update_from_list_output("$2 new 2 1 0\n");
        assert_eq!(list.sessions().len(), 1);
        assert_eq!(list.sessions()[0].name, "new");
    }

    #[test]
    fn apply_session_renamed() {
        let mut list = SessionList::new();
        list.update_from_list_output("$1 old-name 2 1 0\n");
        list.apply_notification(&Notification::SessionRenamed { session_id: 1, name: "new-name".into() });
        assert_eq!(list.sessions()[0].name, "new-name");
    }

    #[test]
    fn apply_session_renamed_unknown_id_is_noop() {
        let mut list = SessionList::new();
        list.update_from_list_output("$1 work 2 1 0\n");
        list.apply_notification(&Notification::SessionRenamed { session_id: 99, name: "new-name".into() });
        assert_eq!(list.sessions()[0].name, "work");
    }

    #[test]
    fn apply_session_changed_updates_attached() {
        let mut list = SessionList::new();
        list.update_from_list_output("$1 work 2 1 0\n$2 play 1 0 0\n");
        list.apply_notification(&Notification::SessionChanged { session_id: 2, name: "play".into() });
        assert!(!list.sessions()[0].attached);
        assert!(list.sessions()[1].attached);
    }

    #[test]
    fn apply_window_add_increments_count() {
        let mut list = SessionList::new();
        list.update_from_list_output("$1 work 2 1 0\n");
        list.apply_notification(&Notification::WindowAdd { window_id: 5 });
        assert_eq!(list.sessions()[0].window_count, 3);
    }

    #[test]
    fn apply_window_close_decrements_count() {
        let mut list = SessionList::new();
        list.update_from_list_output("$1 work 2 1 0\n");
        list.apply_notification(&Notification::WindowClose { window_id: 1 });
        assert_eq!(list.sessions()[0].window_count, 1);
    }

    #[test]
    fn apply_window_close_does_not_underflow() {
        let mut list = SessionList::new();
        list.update_from_list_output("$1 work 0 1 0\n");
        list.apply_notification(&Notification::WindowClose { window_id: 1 });
        assert_eq!(list.sessions()[0].window_count, 0);
    }

    #[test]
    fn parse_session_line_valid() {
        let s = parse_session_line("$1 my-session 3 1 1711990800").unwrap();
        assert_eq!(s.id, 1);
        assert_eq!(s.name, "my-session");
        assert_eq!(s.window_count, 3);
        assert!(s.attached);
        assert_eq!(s.created, Some(1711990800));
    }

    #[test]
    fn parse_session_line_not_attached() {
        let s = parse_session_line("$2 dev 1 0 0").unwrap();
        assert!(!s.attached);
    }

    #[test]
    fn parse_session_line_invalid_returns_none() {
        assert!(parse_session_line("garbage").is_none());
        assert!(parse_session_line("").is_none());
    }

    #[test]
    fn update_from_empty_string() {
        let mut list = SessionList::new();
        list.update_from_list_output("");
        assert!(list.sessions().is_empty());
    }

    #[test]
    fn update_skips_blank_lines() {
        let mut list = SessionList::new();
        list.update_from_list_output("\n\n$1 work 1 0 0\n\n");
        assert_eq!(list.sessions().len(), 1);
    }
}

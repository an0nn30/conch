//! Tmux command builder for control mode.
//!
//! Each method returns a newline-terminated `String` ready to write to
//! the control mode connection's stdin.

/// Builds tmux command strings to send over a control mode connection.
pub struct CommandBuilder;

impl CommandBuilder {
    pub fn list_sessions() -> String {
        "list-sessions\n".into()
    }

    pub fn new_session(name: Option<&str>) -> String {
        match name {
            Some(n) => format!("new-session -d -s {}\n", quote(n)),
            None => "new-session -d\n".into(),
        }
    }

    pub fn kill_session(target: &str) -> String {
        format!("kill-session -t {}\n", quote(target))
    }

    pub fn rename_session(target: &str, new_name: &str) -> String {
        format!("rename-session -t {} {}\n", quote(target), quote(new_name))
    }

    pub fn switch_client(target: &str) -> String {
        format!("switch-client -t {}\n", quote(target))
    }

    pub fn detach_client() -> String {
        "detach-client\n".into()
    }

    pub fn new_window(target_session: &str) -> String {
        format!("new-window -t {}\n", quote(target_session))
    }

    pub fn kill_window(target: &str) -> String {
        format!("kill-window -t {}\n", quote(target))
    }

    pub fn rename_window(target: &str, new_name: &str) -> String {
        format!("rename-window -t {} {}\n", quote(target), quote(new_name))
    }

    pub fn split_window(target: &str, horizontal: bool) -> String {
        let flag = if horizontal { "-h" } else { "-v" };
        format!("split-window {} -t {}\n", flag, quote(target))
    }

    pub fn select_pane(target: &str) -> String {
        format!("select-pane -t {}\n", quote(target))
    }

    pub fn kill_pane(target: &str) -> String {
        format!("kill-pane -t {}\n", quote(target))
    }

    pub fn resize_pane(target: &str, cols: u16, rows: u16) -> String {
        format!("resize-pane -t {} -x {} -y {}\n", quote(target), cols, rows)
    }

    pub fn send_keys(target: &str, keys: &str) -> String {
        format!("send-keys -t {} -l -- {}\n", quote(target), quote(keys))
    }

    pub fn list_windows(target_session: &str) -> String {
        format!(
            "list-windows -t {} -F '#{{window_id}} #{{window_name}} #{{window_active}}'\n",
            quote(target_session)
        )
    }

    pub fn list_panes(target_window: &str) -> String {
        format!(
            "list-panes -t {} -F '#{{pane_id}} #{{pane_active}} #{{pane_width}} #{{pane_height}}'\n",
            quote(target_window)
        )
    }
}

fn quote(s: &str) -> String {
    if s.contains(|c: char| c.is_whitespace() || c == '\'' || c == '"' || c == '\\') {
        format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_sessions_cmd() {
        assert_eq!(CommandBuilder::list_sessions(), "list-sessions\n");
    }

    #[test]
    fn new_session_named() {
        assert_eq!(CommandBuilder::new_session(Some("work")), "new-session -d -s work\n");
    }

    #[test]
    fn new_session_unnamed() {
        assert_eq!(CommandBuilder::new_session(None), "new-session -d\n");
    }

    #[test]
    fn kill_session_cmd() {
        assert_eq!(CommandBuilder::kill_session("old"), "kill-session -t old\n");
    }

    #[test]
    fn rename_session_cmd() {
        assert_eq!(CommandBuilder::rename_session("old", "new"), "rename-session -t old new\n");
    }

    #[test]
    fn switch_client_cmd() {
        assert_eq!(CommandBuilder::switch_client("work"), "switch-client -t work\n");
    }

    #[test]
    fn detach_client_cmd() {
        assert_eq!(CommandBuilder::detach_client(), "detach-client\n");
    }

    #[test]
    fn new_window_cmd() {
        assert_eq!(CommandBuilder::new_window("work"), "new-window -t work\n");
    }

    #[test]
    fn kill_window_cmd() {
        assert_eq!(CommandBuilder::kill_window("@1"), "kill-window -t @1\n");
    }

    #[test]
    fn rename_window_cmd() {
        assert_eq!(CommandBuilder::rename_window("@1", "editor"), "rename-window -t @1 editor\n");
    }

    #[test]
    fn split_window_horizontal() {
        assert_eq!(CommandBuilder::split_window("%1", true), "split-window -h -t %1\n");
    }

    #[test]
    fn split_window_vertical() {
        assert_eq!(CommandBuilder::split_window("%1", false), "split-window -v -t %1\n");
    }

    #[test]
    fn select_pane_cmd() {
        assert_eq!(CommandBuilder::select_pane("%3"), "select-pane -t %3\n");
    }

    #[test]
    fn kill_pane_cmd() {
        assert_eq!(CommandBuilder::kill_pane("%2"), "kill-pane -t %2\n");
    }

    #[test]
    fn resize_pane_cmd() {
        assert_eq!(CommandBuilder::resize_pane("%1", 120, 40), "resize-pane -t %1 -x 120 -y 40\n");
    }

    #[test]
    fn session_name_with_spaces_is_quoted() {
        assert_eq!(CommandBuilder::new_session(Some("my project")), "new-session -d -s \"my project\"\n");
    }

    #[test]
    fn session_name_with_quotes_is_escaped() {
        assert_eq!(CommandBuilder::kill_session("test\"name"), "kill-session -t \"test\\\"name\"\n");
    }

    #[test]
    fn quote_plain_string() {
        assert_eq!(quote("simple"), "simple");
    }

    #[test]
    fn quote_string_with_space() {
        assert_eq!(quote("has space"), "\"has space\"");
    }
}

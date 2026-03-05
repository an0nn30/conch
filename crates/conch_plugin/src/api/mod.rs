pub mod app;
pub mod crypto;
pub mod session;
pub mod ui;

use std::collections::HashMap;

use tokio::sync::mpsc;

/// How a plugin targets a session.
#[derive(Debug, Clone)]
pub enum SessionTarget {
    Current,
    Named(String),
}

/// A single field in a plugin form dialog.
#[derive(Debug, Clone)]
pub enum FormField {
    Text     { name: String, label: String, default: String },
    Password { name: String, label: String },
    ComboBox { name: String, label: String, options: Vec<String>, default: String },
    CheckBox { name: String, label: String, default: bool },
    Separator,
    Label    { text: String },
}

/// Metadata about a session, returned to plugins.
#[derive(Debug, Clone)]
pub struct SessionInfoData {
    pub id: String,
    pub title: String,
    pub session_type: String, // "local" or "ssh"
}

/// Commands that a plugin can send to the host application.
#[derive(Debug, Clone)]
pub enum PluginCommand {
    /// Execute a command on a session and return stdout.
    Exec { target: SessionTarget, command: String },
    /// Send raw text to a session.
    Send { target: SessionTarget, text: String },
    /// Open a new SSH session by name or host.
    OpenSession { name: String },
    /// Copy text to clipboard.
    Clipboard(String),
    /// Show a notification to the user.
    Notify(String),
    /// Log a message.
    Log(String),
    /// Append text to the plugin output panel.
    UiAppend(String),
    /// Clear the plugin output panel.
    UiClear,

    // Session queries
    /// Get info about the current (active) session.
    GetCurrentSession,
    /// Get info about all sessions.
    GetAllSessions,
    /// Get a named session.
    GetNamedSession { name: String },
    /// Get all configured server names.
    GetServers,

    // UI dialogs (blocking — plugin awaits response)
    /// Show a form dialog with multiple fields.
    ShowForm { title: String, fields: Vec<FormField> },
    /// Show a text input prompt.
    ShowPrompt { message: String },
    /// Show a yes/no confirmation dialog.
    ShowConfirm { message: String },
    /// Show an informational alert.
    ShowAlert { title: String, message: String },
    /// Show an error alert.
    ShowError { title: String, message: String },
    /// Show a read-only text viewer.
    ShowText { title: String, text: String },
    /// Show a table viewer.
    ShowTable { title: String, columns: Vec<String>, rows: Vec<Vec<String>> },
    /// Show a progress spinner.
    ShowProgress { message: String },
    /// Hide the progress spinner.
    HideProgress,
}

/// Response from the host application to a plugin command.
#[derive(Debug, Clone)]
pub enum PluginResponse {
    /// Command output (from Exec).
    Output(String),
    /// Success with no data.
    Ok,
    /// Error message.
    Error(String),
    /// Boolean result (from Confirm).
    Bool(bool),
    /// Form result — None means cancelled, Some contains field name→value map.
    FormResult(Option<HashMap<String, String>>),
    /// Single session info.
    SessionInfo(Option<SessionInfoData>),
    /// List of session info.
    SessionList(Vec<SessionInfoData>),
    /// List of server names.
    ServerList(Vec<String>),
}

/// Context passed to plugin execution — provides a channel to communicate with the app.
#[derive(Clone)]
pub struct PluginContext {
    pub command_tx: mpsc::UnboundedSender<(PluginCommand, mpsc::UnboundedSender<PluginResponse>)>,
}

impl PluginContext {
    pub fn new() -> (Self, mpsc::UnboundedReceiver<(PluginCommand, mpsc::UnboundedSender<PluginResponse>)>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (Self { command_tx: tx }, rx)
    }

    /// Send a command and wait for a response.
    pub async fn send_command(&self, cmd: PluginCommand) -> PluginResponse {
        let cmd_name = format!("{:?}", std::mem::discriminant(&cmd));
        let t0 = std::time::Instant::now();
        let (resp_tx, mut resp_rx) = mpsc::unbounded_channel();
        if self.command_tx.send((cmd, resp_tx)).is_err() {
            return PluginResponse::Error("Plugin host disconnected".into());
        }
        eprintln!("[plugin] sent {cmd_name}, waiting for response...");
        let resp = resp_rx.recv().await.unwrap_or(PluginResponse::Error("No response".into()));
        eprintln!("[plugin] {cmd_name} response received in {:?}", t0.elapsed());
        resp
    }

    /// Send a fire-and-forget command (no response needed).
    pub fn send_fire_and_forget(&self, cmd: PluginCommand) {
        let (resp_tx, _) = mpsc::unbounded_channel();
        let _ = self.command_tx.send((cmd, resp_tx));
    }
}

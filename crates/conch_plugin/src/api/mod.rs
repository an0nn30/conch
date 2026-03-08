pub mod app;
pub mod crypto;
pub mod net;
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

/// A declarative widget for panel plugins.
#[derive(Debug, Clone)]
pub enum PanelWidget {
    Heading(String),
    Text(String),
    Label(String),
    Separator,
    Table { columns: Vec<String>, rows: Vec<Vec<String>> },
    Progress { label: String, fraction: f32, text: String },
    Button { id: String, label: String },
    KeyValue { key: String, value: String },
}

/// Metadata about a session, returned to plugins.
#[derive(Debug, Clone)]
pub struct SessionInfoData {
    pub id: String,
    pub title: String,
    pub session_type: String, // "local" or "ssh"
}

/// Severity/style level for notifications.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NotificationLevel {
    #[default]
    Info,
    Success,
    Warning,
    Error,
}

/// A rich notification request from a plugin (or internal app code).
#[derive(Debug, Clone)]
pub struct NotificationRequest {
    pub title: Option<String>,
    pub body: String,
    pub level: NotificationLevel,
    /// Duration in seconds. `None` = default (5s). `Some(0)` = persistent until dismissed.
    pub duration_secs: Option<f32>,
    /// If non-empty, notification blocks and returns the clicked button label.
    pub buttons: Vec<String>,
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
    /// Show a notification toast.
    Notify(NotificationRequest),
    /// Log a message.
    Log(String),
    /// Append text to the plugin output panel.
    UiAppend(String),
    /// Clear the plugin output panel.
    UiClear,

    // Session queries
    /// Get the platform/OS of the current session (e.g. "macos", "linux").
    GetPlatform { target: SessionTarget },
    /// Get info about the current (active) session.
    GetCurrentSession,
    /// Get info about all sessions.
    GetAllSessions,
    /// Get a named session.
    GetNamedSession { name: String },
    /// Get all configured server names.
    GetServers,
    /// Get all configured servers with name and host.
    GetServerDetails,

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

    // Plugin metadata commands
    /// Set the plugin's icon from a file path. The path is validated.
    SetIcon { path: String },

    // Keybinding commands
    /// Register a keybinding at runtime.
    RegisterKeybind {
        action: String,
        binding: String,
        description: String,
    },

    // Panel plugin commands
    /// Replace the panel's widget list.
    PanelSetWidgets(Vec<PanelWidget>),
    /// Set the panel refresh interval in seconds (0 = manual only).
    PanelSetRefresh(f64),
    /// Wait for a button click event from the panel. Returns the button id.
    PanelWaitEvent,
    /// Non-blocking poll for a panel event. Returns PanelEvent or Ok (no event).
    PanelPollEvent,
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
    /// List of server name+host pairs.
    ServerDetailList(Vec<(String, String)>),
    /// A panel button was clicked (carries the button id).
    PanelEvent(String),
    /// A keybinding was triggered (carries the action name).
    KeybindTriggered(String),
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

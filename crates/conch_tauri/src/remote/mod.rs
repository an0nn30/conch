//! Remote module — unified SSH connections, SFTP, and file operations.
//!
//! Exposes Tauri commands for SSH session lifecycle. The frontend sees
//! the same `pty-output` / `pty-exit` events as local PTY tabs — xterm.js
//! doesn't care whether bytes come from a local shell or an SSH channel.
//!
//! All SSH/SFTP/transfer/tunnel logic is delegated to `conch_remote`.
//! This module provides the Tauri command wrappers and the
//! `TauriRemoteCallbacks` implementation that bridges `RemoteCallbacks`
//! to Tauri events + oneshot prompt channels.

pub(crate) mod auth;
pub(crate) mod local_fs;
pub(crate) mod server_commands;
pub(crate) mod sftp_commands;
pub(crate) mod ssh_commands;
pub(crate) mod transfer_commands;
pub(crate) mod tunnel_commands;

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::Mutex;
use serde::Serialize;
use tauri::Emitter;
use tokio::sync::mpsc;

use conch_remote::callbacks::{RemoteCallbacks, RemotePaths};
use conch_remote::config::{ServerEntry, SshConfig};
use conch_remote::handler::ConchSshHandler;
use conch_remote::ssh::{ChannelInput, SshCredentials};
use conch_remote::transfer::{TransferProgress, TransferRegistry};
use conch_remote::tunnel::TunnelManager;

use crate::pty::{PtyExitEvent, PtyOutputEvent};

const BOOTSTRAP_END_MARKER: &str = "__CONCH_BOOTSTRAP_END__";
const BOOTSTRAP_FILTER_MAX_BUFFER: usize = 24 * 1024;

#[derive(Copy, Clone, Eq, PartialEq)]
enum BootstrapFilterState {
    SearchEnd,
    Done,
}

// ---------------------------------------------------------------------------
// TauriRemoteCallbacks — bridges RemoteCallbacks to Tauri events
// ---------------------------------------------------------------------------

/// Bridges `conch_remote::callbacks::RemoteCallbacks` to Tauri events and
/// oneshot prompt channels. When the SSH handler needs user interaction
/// (host key confirmation, password entry), this implementation emits a
/// Tauri event and waits on a oneshot channel that the frontend will
/// resolve via `auth_respond_host_key` / `auth_respond_password`.
pub(crate) struct TauriRemoteCallbacks {
    pub app: tauri::AppHandle,
    pub pending_prompts: Arc<Mutex<PendingPrompts>>,
}

#[async_trait::async_trait]
impl RemoteCallbacks for TauriRemoteCallbacks {
    async fn verify_host_key(&self, message: &str, fingerprint: &str) -> bool {
        let prompt_id = uuid::Uuid::new_v4().to_string();
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.pending_prompts
            .lock()
            .host_key
            .insert(prompt_id.clone(), tx);
        let _ = self.app.emit(
            "ssh-host-key-prompt",
            HostKeyPromptEvent {
                prompt_id,
                message: message.to_string(),
                detail: fingerprint.to_string(),
            },
        );
        rx.await.unwrap_or(false)
    }

    async fn prompt_password(&self, message: &str) -> Option<String> {
        let prompt_id = uuid::Uuid::new_v4().to_string();
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.pending_prompts
            .lock()
            .password
            .insert(prompt_id.clone(), tx);
        let _ = self.app.emit(
            "ssh-password-prompt",
            PasswordPromptEvent {
                prompt_id,
                message: message.to_string(),
            },
        );
        rx.await.unwrap_or(None)
    }

    fn on_transfer_progress(&self, _transfer_id: &str, _bytes: u64, _total: Option<u64>) {
        // Transfer progress is handled via the existing mpsc channel pattern
        // in the transfer module, not through callbacks.
    }
}

// ---------------------------------------------------------------------------
// Desktop paths
// ---------------------------------------------------------------------------

/// Build the `RemotePaths` for a desktop environment.
fn desktop_remote_paths() -> RemotePaths {
    let home = dirs::home_dir().unwrap_or_default();
    let ssh_dir = home.join(".ssh");
    RemotePaths {
        known_hosts_file: ssh_dir.join("known_hosts"),
        config_dir: conch_core::config::config_dir().join("remote"),
        default_key_paths: vec![
            ssh_dir.join("id_ed25519"),
            ssh_dir.join("id_rsa"),
            ssh_dir.join("id_ecdsa"),
        ],
    }
}

fn update_session_cwd_from_output(state: &mut RemoteState, session_key: &str, text: &str) {
    let mut buffer = state
        .pane_cwd_buffers
        .remove(session_key)
        .unwrap_or_default();
    buffer.push_str(text);
    if buffer.len() > 8192 {
        buffer = buffer.split_off(buffer.len().saturating_sub(8192));
    }

    let marker = "\x1b]7;";
    let mut latest_path: Option<String> = None;
    loop {
        let Some(start) = buffer.find(marker) else { break };
        let content_start = start + marker.len();
        let bel_idx = buffer[content_start..].find('\x07').map(|n| content_start + n);
        let st_idx = buffer[content_start..]
            .find("\x1b\\")
            .map(|n| content_start + n);

        let (end_idx, term_len) = match (bel_idx, st_idx) {
            (Some(bel), Some(st)) => {
                if bel < st {
                    (bel, 1)
                } else {
                    (st, 2)
                }
            }
            (Some(bel), None) => (bel, 1),
            (None, Some(st)) => (st, 2),
            (None, None) => {
                buffer = buffer[start..].to_string();
                state
                    .pane_cwd_buffers
                    .insert(session_key.to_string(), buffer);
                return;
            }
        };

        let content = &buffer[content_start..end_idx];
        if let Some(path) = parse_osc7_file_path(content) {
            latest_path = Some(path);
        }
        buffer = buffer[(end_idx + term_len)..].to_string();
    }

    if let Some(path) = latest_path {
        if let Some(home) = derive_home_from_path(&path) {
            state.pane_home_dirs.insert(session_key.to_string(), home);
        }
        state
            .pane_cwd_needs_sync
            .insert(session_key.to_string(), false);
        state.pane_cwds.insert(session_key.to_string(), path);
    } else {
        let home_hint = state.pane_home_dirs.get(session_key).map(String::as_str);
        if let Some(path) = parse_prompt_cwd_from_text(text, home_hint) {
            if let Some(home) = derive_home_from_path(&path) {
                state.pane_home_dirs.insert(session_key.to_string(), home);
            }
            state
                .pane_cwd_needs_sync
                .insert(session_key.to_string(), false);
            state.pane_cwds.insert(session_key.to_string(), path);
        }
    }

    let keep = if let Some(tail_start) = buffer.rfind('\x1b') {
        buffer[tail_start..].to_string()
    } else if buffer.len() > 64 {
        buffer[buffer.len() - 64..].to_string()
    } else {
        buffer
    };
    if !keep.is_empty() {
        state
            .pane_cwd_buffers
            .insert(session_key.to_string(), keep);
    }
}

fn parse_osc7_file_path(content: &str) -> Option<String> {
    let raw = content.trim();
    if raw.is_empty() || !raw.to_ascii_lowercase().starts_with("file://") {
        return None;
    }

    let after = &raw["file://".len()..];
    let slash = after.find('/')?;
    let path_part = &after[slash..];
    let decoded = percent_decode(path_part);
    if decoded.is_empty() {
        return None;
    }
    if decoded.len() > 1 {
        Some(decoded.trim_end_matches('/').to_string())
    } else {
        Some(decoded)
    }
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(input.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let h1 = bytes[i + 1] as char;
            let h2 = bytes[i + 2] as char;
            if let (Some(a), Some(b)) = (h1.to_digit(16), h2.to_digit(16)) {
                let v = ((a << 4) | b) as u8;
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub(crate) fn derive_home_from_path(path: &str) -> Option<String> {
    if let Some(rest) = path.strip_prefix("/home/") {
        let user = rest.split('/').next()?;
        if user.is_empty() {
            return None;
        }
        return Some(format!("/home/{user}"));
    }
    if let Some(rest) = path.strip_prefix("/Users/") {
        let user = rest.split('/').next()?;
        if user.is_empty() {
            return None;
        }
        return Some(format!("/Users/{user}"));
    }
    None
}

fn strip_ansi_sequences(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let bytes = input.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] != 0x1b {
            out.push(bytes[i] as char);
            i += 1;
            continue;
        }
        i += 1;
        if i >= bytes.len() {
            break;
        }
        match bytes[i] {
            b'[' => {
                i += 1;
                while i < bytes.len() {
                    let b = bytes[i];
                    i += 1;
                    if (0x40..=0x7E).contains(&b) {
                        break;
                    }
                }
            }
            b']' => {
                i += 1;
                while i < bytes.len() {
                    if bytes[i] == 0x07 {
                        i += 1;
                        break;
                    }
                    if bytes[i] == 0x1b && i + 1 < bytes.len() && bytes[i + 1] == b'\\' {
                        i += 2;
                        break;
                    }
                    i += 1;
                }
            }
            _ => {
                i += 1;
            }
        }
    }
    out
}

fn parse_prompt_cwd_from_text(text: &str, home_hint: Option<&str>) -> Option<String> {
    let cleaned = strip_ansi_sequences(text);
    for line in cleaned.lines().rev() {
        let trimmed = line.trim_end();
        if !(trimmed.ends_with('$') || trimmed.ends_with('#')) {
            continue;
        }
        let core = trimmed[..trimmed.len().saturating_sub(1)].trim_end();
        let Some(idx) = core.rfind(':') else { continue };
        let cwd = core[idx + 1..].trim();
        if cwd.is_empty() {
            continue;
        }
        if cwd == "~" {
            if let Some(home) = home_hint {
                return Some(home.to_string());
            }
            continue;
        }
        if let Some(rest) = cwd.strip_prefix("~/") {
            if let Some(home) = home_hint {
                return Some(normalize_unix_path(&format!("{home}/{rest}")));
            }
            continue;
        }
        if cwd.starts_with('/') {
            return Some(normalize_unix_path(cwd));
        }
    }
    None
}

fn strip_bootstrap_noise(
    chunk: &str,
    state: &mut BootstrapFilterState,
    buffer: &mut String,
) -> String {
    if *state == BootstrapFilterState::Done {
        return chunk.to_string();
    }

    buffer.push_str(chunk);
    let mut out = String::new();

    loop {
        match *state {
            BootstrapFilterState::SearchEnd => {
                if let Some(pos) = buffer.find(BOOTSTRAP_END_MARKER) {
                    let drain_to = pos + BOOTSTRAP_END_MARKER.len();
                    buffer.drain(..drain_to);
                    while buffer.starts_with('\n') || buffer.starts_with('\r') {
                        buffer.drain(..1);
                    }
                    *state = BootstrapFilterState::Done;
                    continue;
                }

                // Safety valve: if marker never appears, stop filtering so the
                // terminal is still usable.
                if buffer.len() > BOOTSTRAP_FILTER_MAX_BUFFER {
                    out.push_str(buffer);
                    buffer.clear();
                    *state = BootstrapFilterState::Done;
                }
                break;
            }
            BootstrapFilterState::Done => {
                out.push_str(buffer);
                buffer.clear();
                break;
            }
        }
    }

    out
}

/// Spawn an async task that drains `output_rx` and emits `pty-output` events,
/// buffering partial UTF-8 sequences between channel messages.
fn spawn_output_forwarder(
    app: &tauri::AppHandle,
    remote: &Arc<Mutex<RemoteState>>,
    session_key: &str,
    window_label: &str,
    pane_id: u32,
    bootstrap_expected: bool,
    mut output_rx: mpsc::UnboundedReceiver<Vec<u8>>,
) {
    let app = app.clone();
    let remote = Arc::clone(remote);
    let session_key = session_key.to_owned();
    let wl = window_label.to_owned();
    tokio::spawn(async move {
        let mut utf8 = crate::utf8_stream::Utf8Accumulator::new();
        let mut bootstrap_state = if bootstrap_expected {
            BootstrapFilterState::SearchEnd
        } else {
            BootstrapFilterState::Done
        };
        let mut bootstrap_buffer = String::new();
        while let Some(data) = output_rx.recv().await {
            let text = utf8.push(&data);
            if text.is_empty() {
                continue;
            }
            {
                let mut state = remote.lock();
                update_session_cwd_from_output(&mut state, &session_key, &text);
            }
            let filtered = strip_bootstrap_noise(&text, &mut bootstrap_state, &mut bootstrap_buffer);
            if filtered.is_empty() {
                continue;
            }
            let _ = app.emit_to(
                &wl,
                "pty-output",
                PtyOutputEvent {
                    window_label: wl.clone(),
                    pane_id,
                    data: filtered,
                },
            );
        }
    });
}

// ---------------------------------------------------------------------------
// Auth prompt events — frontend receives these, responds via commands
// ---------------------------------------------------------------------------

/// Emitted to the frontend when the SSH handler needs host key confirmation.
#[derive(Clone, Serialize)]
struct HostKeyPromptEvent {
    prompt_id: String,
    message: String,
    detail: String,
}

/// Emitted to the frontend when the SSH handler needs a password.
#[derive(Clone, Serialize)]
struct PasswordPromptEvent {
    prompt_id: String,
    message: String,
}

/// Pending auth prompts waiting for frontend responses.
pub(crate) struct PendingPrompts {
    pub(crate) host_key: HashMap<String, tokio::sync::oneshot::Sender<bool>>,
    pub(crate) password: HashMap<String, tokio::sync::oneshot::Sender<Option<String>>>,
}

impl PendingPrompts {
    pub(crate) fn new() -> Self {
        Self {
            host_key: HashMap::new(),
            password: HashMap::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Session state
// ---------------------------------------------------------------------------

/// A shared SSH connection that may serve multiple pane channels.
pub(crate) struct SshConnection {
    pub ssh_handle: Arc<conch_remote::russh::client::Handle<ConchSshHandler>>,
    pub host: String,
    pub user: String,
    pub port: u16,
    pub ref_count: u32,
}

/// A live SSH session tracked by the backend.
pub(crate) struct SshSession {
    pub input_tx: mpsc::UnboundedSender<ChannelInput>,
    pub connection_id: String,
    pub host: String,
    pub user: String,
    pub port: u16,
    /// Handle to abort the channel loop task on cleanup.
    pub abort_handle: Option<tokio::task::AbortHandle>,
}

/// Shared state for all remote operations.
pub(crate) struct RemoteState {
    /// SSH sessions keyed by `"{window_label}:{pane_id}"` (same as local PTY keys).
    pub sessions: HashMap<String, SshSession>,
    /// Shared SSH connections keyed by `"conn:{window_label}:{pane_id}"`.
    /// Multiple sessions may reference the same connection via `connection_id`.
    pub connections: HashMap<String, SshConnection>,
    /// Server configuration.
    pub config: SshConfig,
    /// Hosts imported from `~/.ssh/config`.
    pub ssh_config_entries: Vec<ServerEntry>,
    /// Pending auth prompts waiting for frontend responses.
    pub pending_prompts: Arc<Mutex<PendingPrompts>>,
    /// Active tunnel manager.
    pub tunnel_manager: TunnelManager,
    /// Active file transfers.
    pub transfers: Arc<Mutex<TransferRegistry>>,
    /// Channel for transfer progress events (forwarded to Tauri events).
    pub transfer_progress_tx: mpsc::UnboundedSender<TransferProgress>,
    /// Platform-specific paths for SSH operations.
    pub paths: RemotePaths,
    /// Last known cwd per SSH pane session key (`"{window_label}:{pane_id}"`).
    pub pane_cwds: HashMap<String, String>,
    /// Incremental OSC 7 parse buffer per SSH pane session key.
    pub pane_cwd_buffers: HashMap<String, String>,
    /// Incremental shell input line buffer per SSH pane session key.
    pub pane_input_buffers: HashMap<String, String>,
    /// Previous cwd per SSH pane session key (for `cd -`).
    pub pane_prev_cwds: HashMap<String, String>,
    /// Whether pane cwd should be refreshed from a one-shot `pwd` sync.
    pub pane_cwd_needs_sync: HashMap<String, bool>,
    /// Best-effort remote home dir per SSH pane session key.
    pub pane_home_dirs: HashMap<String, String>,
}

impl RemoteState {
    pub fn new(transfer_progress_tx: mpsc::UnboundedSender<TransferProgress>) -> Self {
        let paths = desktop_remote_paths();
        let config = conch_remote::config::load_config(&paths.config_dir);
        let ssh_config_entries = conch_remote::config::parse_ssh_config();
        Self {
            sessions: HashMap::new(),
            connections: HashMap::new(),
            config,
            ssh_config_entries,
            pending_prompts: Arc::new(Mutex::new(PendingPrompts::new())),
            tunnel_manager: TunnelManager::new(),
            transfers: Arc::new(Mutex::new(TransferRegistry::new())),
            transfer_progress_tx,
            paths,
            pane_cwds: HashMap::new(),
            pane_cwd_buffers: HashMap::new(),
            pane_input_buffers: HashMap::new(),
            pane_prev_cwds: HashMap::new(),
            pane_cwd_needs_sync: HashMap::new(),
            pane_home_dirs: HashMap::new(),
        }
    }
}

fn normalize_unix_path(path: &str) -> String {
    let absolute = path.starts_with('/');
    let mut stack: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                let _ = stack.pop();
            }
            _ => stack.push(part),
        }
    }
    if absolute {
        if stack.is_empty() {
            "/".to_string()
        } else {
            format!("/{}", stack.join("/"))
        }
    } else if stack.is_empty() {
        ".".to_string()
    } else {
        stack.join("/")
    }
}

fn resolve_cd_target(current: Option<&str>, previous: Option<&str>, arg: Option<&str>) -> Option<String> {
    let cur = current?;
    let home = if cur.starts_with("/home/") || cur.starts_with("/Users/") {
        Some(cur.to_string())
    } else {
        None
    };
    let raw = arg.map(str::trim).unwrap_or("");
    if raw.is_empty() || raw == "~" {
        return Some(home.unwrap_or_else(|| cur.to_string()));
    }
    if raw == "-" {
        return previous.map(ToString::to_string);
    }
    if let Some(rest) = raw.strip_prefix("~/") {
        if let Some(h) = home {
            return Some(normalize_unix_path(&format!("{h}/{rest}")));
        }
        return None;
    }
    if raw.starts_with('~') {
        return None;
    }
    if raw.starts_with('/') {
        return Some(normalize_unix_path(raw));
    }
    Some(normalize_unix_path(&format!("{cur}/{raw}")))
}

fn maybe_apply_cd_command(state: &mut RemoteState, session_key: &str, line: &str) {
    let line = line.trim();
    if line.is_empty() {
        return;
    }
    // Ignore complex shell statements where static parsing is unreliable.
    if line.contains(['|', ';', '&', '`', '$', '(', ')', '{', '}']) {
        return;
    }
    let mut tokens = line.split_whitespace();
    let cmd = tokens.next().unwrap_or_default();
    if cmd != "cd" {
        return;
    }
    state
        .pane_cwd_needs_sync
        .insert(session_key.to_string(), true);

    if line.contains('\t') {
        // Tab completion means we only saw partial typed input (e.g. "proj\t"),
        // so defer to one-shot sync for accurate final cwd.
        return;
    }
    let arg = tokens.next();
    let current = state.pane_cwds.get(session_key).cloned();
    let previous = state.pane_prev_cwds.get(session_key).cloned();
    let Some(next) = resolve_cd_target(current.as_deref(), previous.as_deref(), arg) else {
        return;
    };
    if current.as_deref() != Some(next.as_str()) {
        if let Some(cur) = current {
            state.pane_prev_cwds.insert(session_key.to_string(), cur);
        }
        log::info!(
            "ssh cwd heuristic: key={} line={:?} -> {:?}",
            session_key,
            line,
            next
        );
        state.pane_cwds.insert(session_key.to_string(), next);
    }
}

pub(crate) fn update_session_cwd_from_input(state: &mut RemoteState, session_key: &str, text: &str) {
    let mut line_buf = state
        .pane_input_buffers
        .remove(session_key)
        .unwrap_or_default();
    for ch in text.chars() {
        match ch {
            '\r' | '\n' => {
                let line = line_buf.replace('\t', " ");
                maybe_apply_cd_command(state, session_key, &line);
                line_buf.clear();
            }
            '\u{3}' | '\u{15}' => {
                // Ctrl-C / Ctrl-U clear the in-progress command.
                line_buf.clear();
            }
            '\u{8}' | '\u{7f}' => {
                let _ = line_buf.pop();
            }
            '\u{1b}' => {
                // Start of escape sequence (arrows, etc) — ignore.
            }
            _ => {
                if !ch.is_control() {
                    line_buf.push(ch);
                }
            }
        }
    }
    if !line_buf.is_empty() {
        state
            .pane_input_buffers
            .insert(session_key.to_string(), line_buf);
    }
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

fn session_key(window_label: &str, pane_id: u32) -> String {
    format!("{window_label}:{pane_id}")
}

fn connection_key(window_label: &str, pane_id: u32) -> String {
    format!("conn:{window_label}:{pane_id}")
}

/// Shared logic for establishing an SSH session: duplicate check, SSH
/// connection, channel I/O loop, output forwarder, and cleanup task.
///
/// Both `ssh_connect` and `ssh_quick_connect` delegate to this after
/// resolving their respective server entry and credentials.
async fn establish_ssh_session(
    window_label: &str,
    app: &tauri::AppHandle,
    remote: &Arc<Mutex<RemoteState>>,
    pane_id: u32,
    server: &ServerEntry,
    credentials: &SshCredentials,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    let key = session_key(window_label, pane_id);

    // Check for duplicate session.
    let (pending_prompts, paths) = {
        let state = remote.lock();
        if state.sessions.contains_key(&key) {
            return Err(format!(
                "Pane {pane_id} already has an SSH session on window {window_label}"
            ));
        }
        (Arc::clone(&state.pending_prompts), state.paths.clone())
    };

    // Build callbacks and connect via conch_remote.
    let callbacks: Arc<dyn RemoteCallbacks> = Arc::new(TauriRemoteCallbacks {
        app: app.clone(),
        pending_prompts: Arc::clone(&pending_prompts),
    });

    let (ssh_handle, channel) =
        conch_remote::ssh::connect_and_open_shell(server, credentials, callbacks, &paths)
            .await
            .map_err(|e| e.to_string())?;

    // Set up the channel I/O loop.
    let (input_tx, input_rx) = mpsc::unbounded_channel();
    let (output_tx, output_rx) = mpsc::unbounded_channel::<Vec<u8>>();

    // Request initial resize.
    let _ = input_tx.send(ChannelInput::Resize { cols, rows });

    // Store the connection and session.
    let conn_key = connection_key(window_label, pane_id);
    let remote_clone = Arc::clone(remote);
    {
        let mut state = remote_clone.lock();
        state.connections.insert(
            conn_key.clone(),
            SshConnection {
                ssh_handle: Arc::new(ssh_handle),
                host: server.host.clone(),
                user: credentials.username.clone(),
                port: server.port,
                ref_count: 1,
            },
        );
        state.sessions.insert(
            key.clone(),
            SshSession {
                input_tx,
                connection_id: conn_key.clone(),
                host: server.host.clone(),
                user: credentials.username.clone(),
                port: server.port,
                abort_handle: None,
            },
        );
    }

    // Spawn channel loop.
    let remote_for_loop = Arc::clone(&remote_clone);
    let key_for_loop = key.clone();
    let wl = window_label.to_owned();
    let app_handle = app.clone();
    let task = tokio::spawn(async move {
        let exited_naturally = conch_remote::ssh::channel_loop(channel, input_rx, output_tx).await;

        // Clean up session and decrement connection ref count.
        let mut state = remote_for_loop.lock();
        if let Some(session) = state.sessions.remove(&key_for_loop) {
            state.pane_cwds.remove(&key_for_loop);
            state.pane_cwd_buffers.remove(&key_for_loop);
            state.pane_input_buffers.remove(&key_for_loop);
            state.pane_prev_cwds.remove(&key_for_loop);
            state.pane_cwd_needs_sync.remove(&key_for_loop);
            state.pane_home_dirs.remove(&key_for_loop);
            if let Some(conn) = state.connections.get_mut(&session.connection_id) {
                conn.ref_count = conn.ref_count.saturating_sub(1);
                if conn.ref_count == 0 {
                    state.connections.remove(&session.connection_id);
                }
            }
        }
        drop(state);

        if exited_naturally {
            let _ = app_handle.emit_to(
                &wl,
                "pty-exit",
                PtyExitEvent {
                    window_label: wl.clone(),
                    pane_id,
                },
            );
        }
    });

    // Store the abort handle so the channel loop can be cancelled on window close.
    {
        let mut state = remote_clone.lock();
        if let Some(session) = state.sessions.get_mut(&key) {
            session.abort_handle = Some(task.abort_handle());
        }
    }

    spawn_output_forwarder(app, remote, &key, window_label, pane_id, false, output_rx);

    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_key_format() {
        assert_eq!(session_key("main", 3), "main:3");
    }

    #[test]
    fn remote_state_new_has_no_sessions() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let state = RemoteState::new(tx);
        assert!(state.sessions.is_empty());
    }

    #[test]
    fn connection_key_format() {
        let key = connection_key("main", 1);
        assert_eq!(key, "conn:main:1");
    }

    #[test]
    fn connection_key_differs_from_session_key() {
        let ck = connection_key("main", 1);
        let sk = session_key("main", 1);
        assert_ne!(ck, sk);
        assert!(ck.starts_with("conn:"));
    }

    #[test]
    fn remote_state_new_has_no_connections() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let state = RemoteState::new(tx);
        assert!(state.connections.is_empty());
    }

    #[test]
    fn pending_prompts_new_is_empty() {
        let prompts = PendingPrompts::new();
        assert!(prompts.host_key.is_empty());
        assert!(prompts.password.is_empty());
    }

    #[test]
    fn desktop_remote_paths_populated() {
        let paths = desktop_remote_paths();
        // Should have 3 default key paths.
        assert_eq!(paths.default_key_paths.len(), 3);
        assert!(
            paths
                .known_hosts_file
                .to_str()
                .unwrap()
                .contains("known_hosts")
        );
        assert!(paths.config_dir.to_str().unwrap().contains("remote"));
    }
}

//! SSH tunnel management — local port forwarding via standalone SSH connections.
//!
//! Each tunnel establishes its own SSH connection (no terminal/PTY), managed
//! as a tokio task.  `SavedTunnel` is the persisted definition; `TunnelManager`
//! tracks active tunnels.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::net::TcpListener;
use tokio::sync::{mpsc, Mutex};
use uuid::Uuid;

use russh::client;
use serde::{Deserialize, Serialize};

use crate::config::ServerEntry;
use crate::known_hosts;

// ---------------------------------------------------------------------------
// Persisted tunnel definition
// ---------------------------------------------------------------------------

/// A saved SSH tunnel (local port forward).
///
/// `session_key` links the tunnel to a specific SSH server in the form
/// `user@host:port`, matching the `ServerEntry` it should connect through.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedTunnel {
    #[serde(default = "Uuid::new_v4")]
    pub id: Uuid,
    pub label: String,
    /// Identifies the SSH server: `user@host:port`.
    pub session_key: String,
    pub local_port: u16,
    pub remote_host: String,
    pub remote_port: u16,
    #[serde(default)]
    pub auto_start: bool,
}

impl SavedTunnel {
    pub fn description(&self) -> String {
        format!(
            ":{} -> {}:{}",
            self.local_port, self.remote_host, self.remote_port
        )
    }

    /// Build the session key for a server entry.
    pub fn make_session_key(user: &str, host: &str, port: u16) -> String {
        format!("{user}@{host}:{port}")
    }

    /// Parse a session key back into (user, host, port).
    pub fn parse_session_key(key: &str) -> Option<(String, String, u16)> {
        let (user, rest) = key.split_once('@')?;
        let (host, port_str) = rest.rsplit_once(':')?;
        let port = port_str.parse().ok()?;
        Some((user.to_string(), host.to_string(), port))
    }
}

// ---------------------------------------------------------------------------
// Prompt requests (tunnel → UI thread via std::thread bridge)
// ---------------------------------------------------------------------------

/// A prompt request sent from the async SSH handler to the std::thread that
/// services HostApi blocking calls.
pub enum PromptRequest {
    /// Host key verification — unknown or changed key.
    ConfirmHostKey {
        message: String,
        detail: String,
        reply: tokio::sync::oneshot::Sender<bool>,
    },
    /// Password authentication prompt.
    Password {
        message: String,
        reply: tokio::sync::oneshot::Sender<Option<String>>,
    },
}

// ---------------------------------------------------------------------------
// Tunnel-only SSH handler (no PTY, no terminal tab)
// ---------------------------------------------------------------------------

/// Minimal SSH client handler for tunnel-only connections.
/// Sends prompt requests through a channel for the service thread to handle.
struct TunnelSshHandler {
    host: String,
    port: u16,
    prompt_tx: mpsc::Sender<PromptRequest>,
}

#[async_trait::async_trait]
impl client::Handler for TunnelSshHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &ssh_key::PublicKey,
    ) -> Result<bool, Self::Error> {
        match known_hosts::check_known_host(&self.host, self.port, server_public_key) {
            Some(true) => {
                log::debug!(
                    "tunnel ssh: host key for {}:{} matches known_hosts",
                    self.host, self.port
                );
                return Ok(true);
            }
            Some(false) => {
                // Key mismatch — possible MITM. Ask user.
                let fingerprint = server_public_key.fingerprint(ssh_key::HashAlg::Sha256);
                let message = format!(
                    "WARNING: HOST KEY HAS CHANGED for {}:{}\n\n\
                     This could indicate a man-in-the-middle attack.\n\
                     It is also possible that the host key has just been changed.",
                    self.host, self.port
                );
                let detail = format!(
                    "{}\n{fingerprint}",
                    server_public_key.algorithm().as_str(),
                );

                let (tx, rx) = tokio::sync::oneshot::channel();
                let _ = self.prompt_tx.send(PromptRequest::ConfirmHostKey {
                    message,
                    detail,
                    reply: tx,
                }).await;

                return Ok(rx.await.unwrap_or(false));
            }
            None => {
                // Unknown host — ask user.
            }
        }

        let fingerprint = server_public_key.fingerprint(ssh_key::HashAlg::Sha256);
        let host_display = if self.port != 22 {
            format!("[{}]:{}", self.host, self.port)
        } else {
            self.host.clone()
        };
        let message = format!(
            "The authenticity of host '{host_display}' can't be established."
        );
        let detail = format!(
            "{} key fingerprint is:\n{fingerprint}",
            server_public_key.algorithm().as_str(),
        );

        let (tx, rx) = tokio::sync::oneshot::channel();
        let _ = self.prompt_tx.send(PromptRequest::ConfirmHostKey {
            message,
            detail,
            reply: tx,
        }).await;

        let accepted = rx.await.unwrap_or(false);
        if accepted {
            if let Err(e) = known_hosts::add_known_host(&self.host, self.port, server_public_key) {
                log::warn!("tunnel ssh: failed to save host key: {e}");
            }
        }
        Ok(accepted)
    }
}

// ---------------------------------------------------------------------------
// Tunnel status tracking
// ---------------------------------------------------------------------------

/// Current status of a tunnel.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum TunnelStatus {
    Connecting,
    Active,
    Error(String),
}

struct TunnelState {
    status: TunnelStatus,
    abort_handle: Option<tokio::task::AbortHandle>,
}

// ---------------------------------------------------------------------------
// Active tunnel manager
// ---------------------------------------------------------------------------

/// Manages active SSH port-forwarding tunnels.
///
/// Internally wraps an `Arc<Mutex<…>>` so clones share the same state.
pub struct TunnelManager {
    tunnels: Arc<Mutex<HashMap<Uuid, TunnelState>>>,
}

impl Clone for TunnelManager {
    fn clone(&self) -> Self {
        Self {
            tunnels: Arc::clone(&self.tunnels),
        }
    }
}

impl TunnelManager {
    pub fn new() -> Self {
        Self {
            tunnels: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Mark a tunnel as "Connecting" (called before spawning background thread).
    pub async fn set_connecting(&self, id: Uuid) {
        self.tunnels.lock().await.insert(id, TunnelState {
            status: TunnelStatus::Connecting,
            abort_handle: None,
        });
    }

    /// Mark a tunnel as errored (called from background thread on failure).
    pub async fn set_error(&self, id: &Uuid, msg: String) {
        if let Some(state) = self.tunnels.lock().await.get_mut(id) {
            state.status = TunnelStatus::Error(msg);
        }
    }

    /// Clear a tunnel from tracking (remove connecting/error state).
    pub async fn clear_status(&self, id: &Uuid) {
        let mut tunnels = self.tunnels.lock().await;
        // Only remove if not active (don't accidentally remove a running tunnel).
        if let Some(state) = tunnels.get(id) {
            if !matches!(state.status, TunnelStatus::Active) {
                tunnels.remove(id);
            }
        }
    }

    /// Start a tunnel: establishes a standalone SSH connection, then listens
    /// on `127.0.0.1:local_port` and forwards connections to
    /// `remote_host:remote_port` via direct-tcpip channels.
    ///
    /// `prompt_tx` is used for host key verification and password prompts.
    /// It is consumed (dropped) once the connection is established.
    pub async fn start_tunnel(
        &self,
        id: Uuid,
        server: &ServerEntry,
        local_port: u16,
        remote_host: String,
        remote_port: u16,
        prompt_tx: mpsc::Sender<PromptRequest>,
    ) -> Result<(), String> {
        log::info!(
            "tunnel[{id}]: connecting to {}@{}:{} for 127.0.0.1:{local_port} -> {remote_host}:{remote_port}",
            server.user, server.host, server.port,
        );

        // Bind the local listener first so we fail fast on port conflicts.
        let listener = TcpListener::bind(format!("127.0.0.1:{local_port}"))
            .await
            .map_err(|e| format!("Failed to bind local port {local_port}: {e}"))?;

        log::info!("tunnel[{id}]: listening on 127.0.0.1:{local_port}");

        // Establish a standalone SSH connection (consumes prompt_tx).
        let ssh_handle = connect_for_tunnel(server, prompt_tx).await?;

        log::info!("tunnel[{id}]: SSH connection established");

        let join_handle = tokio::spawn(async move {
            loop {
                let (mut local_stream, peer_addr) = match listener.accept().await {
                    Ok(conn) => {
                        log::debug!("tunnel[{id}]: accepted connection from {}", conn.1);
                        conn
                    }
                    Err(e) => {
                        log::error!("tunnel[{id}]: accept error: {e}");
                        break;
                    }
                };

                // Open a direct-tcpip channel.
                let channel = match ssh_handle
                    .channel_open_direct_tcpip(
                        &remote_host,
                        remote_port as u32,
                        &peer_addr.ip().to_string(),
                        peer_addr.port() as u32,
                    )
                    .await
                {
                    Ok(ch) => {
                        log::debug!("tunnel[{id}]: direct-tcpip channel opened for {peer_addr}");
                        ch
                    }
                    Err(e) => {
                        log::error!("tunnel[{id}]: failed to open direct-tcpip: {e}");
                        if e.to_string().contains("disconnect")
                            || e.to_string().contains("closed")
                        {
                            log::error!("tunnel[{id}]: SSH session lost, stopping tunnel");
                            break;
                        }
                        continue;
                    }
                };

                let tunnel_id = id;
                tokio::spawn(async move {
                    let mut channel_stream = channel.into_stream();
                    match tokio::io::copy_bidirectional(&mut local_stream, &mut channel_stream)
                        .await
                    {
                        Ok((tx, rx)) => {
                            log::debug!(
                                "tunnel[{tunnel_id}]: connection closed for {peer_addr} (tx={tx}, rx={rx})"
                            );
                        }
                        Err(e) => {
                            log::debug!(
                                "tunnel[{tunnel_id}]: connection error for {peer_addr}: {e}"
                            );
                        }
                    }
                });
            }
        });

        let abort_handle = join_handle.abort_handle();
        self.tunnels.lock().await.insert(id, TunnelState {
            status: TunnelStatus::Active,
            abort_handle: Some(abort_handle),
        });
        log::info!("tunnel[{id}]: registered as active");

        Ok(())
    }

    /// Stop a running tunnel.
    pub async fn stop(&self, id: &Uuid) {
        if let Some(state) = self.tunnels.lock().await.remove(id) {
            if let Some(handle) = state.abort_handle {
                log::info!("tunnel[{id}]: stopping");
                handle.abort();
            }
        }
    }

    /// Get the status of a tunnel (Connecting, Active, Error, or None).
    pub async fn status(&self, id: &Uuid) -> Option<TunnelStatus> {
        self.tunnels.lock().await.get(id).map(|s| s.status.clone())
    }

    /// Check whether a tunnel is currently active.
    pub async fn is_active(&self, id: &Uuid) -> bool {
        matches!(
            self.tunnels.lock().await.get(id).map(|s| &s.status),
            Some(TunnelStatus::Active)
        )
    }

    /// Stop all tunnels.
    pub async fn stop_all(&self) {
        let mut tunnels = self.tunnels.lock().await;
        for (id, state) in tunnels.drain() {
            if let Some(handle) = state.abort_handle {
                log::info!("tunnel[{id}]: stopping (stop_all)");
                handle.abort();
            }
        }
    }

    /// Return the IDs of all currently active tunnels.
    pub async fn active_ids(&self) -> Vec<Uuid> {
        self.tunnels
            .lock()
            .await
            .iter()
            .filter(|(_, s)| matches!(s.status, TunnelStatus::Active))
            .map(|(id, _)| *id)
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Standalone SSH connection for tunnels
// ---------------------------------------------------------------------------

/// Establish an SSH connection for tunneling only (no PTY, no shell).
/// `prompt_tx` is consumed — prompts are only needed during connection.
async fn connect_for_tunnel(
    server: &ServerEntry,
    prompt_tx: mpsc::Sender<PromptRequest>,
) -> Result<client::Handle<TunnelSshHandler>, String> {
    let config = Arc::new(client::Config::default());
    let handler = TunnelSshHandler {
        host: server.host.clone(),
        port: server.port,
        prompt_tx: prompt_tx.clone(),
    };

    // Determine effective proxy.
    let effective_proxy = server
        .proxy_command
        .clone()
        .or_else(|| {
            server
                .proxy_jump
                .as_ref()
                .map(|jump| format!("ssh -W %h:%p {jump}"))
        });

    let mut session = if let Some(proxy_cmd) = &effective_proxy {
        log::info!("tunnel ssh: using proxy: {proxy_cmd}");
        connect_tunnel_via_proxy(proxy_cmd, &server.host, server.port, config, handler).await?
    } else {
        let addr = format!("{}:{}", server.host, server.port);
        log::info!("tunnel ssh: direct connect to {addr}");
        client::connect(config, &addr, handler)
            .await
            .map_err(|e| format!("Connection failed: {e}"))?
    };

    // Authenticate.
    let authenticated = if server.auth_method == "password" {
        // Request password via prompt channel.
        let (tx, rx) = tokio::sync::oneshot::channel();
        let msg = format!(
            "Password for {}@{}:{}",
            server.user, server.host, server.port
        );
        let _ = prompt_tx
            .send(PromptRequest::Password {
                message: msg,
                reply: tx,
            })
            .await;

        let password = rx
            .await
            .map_err(|_| "Password prompt cancelled".to_string())?
            .ok_or_else(|| "Password prompt cancelled".to_string())?;

        session
            .authenticate_password(&server.user, &password)
            .await
            .map_err(|e| format!("Auth failed: {e}"))?
    } else {
        try_tunnel_key_auth(&mut session, &server.user, server.key_path.as_deref()).await?
    };

    if !authenticated {
        return Err(format!(
            "Authentication failed for {}@{}",
            server.user, server.host
        ));
    }

    log::info!(
        "tunnel ssh: authenticated as {}@{}:{}",
        server.user, server.host, server.port
    );

    // prompt_tx dropped here — signals the service thread that prompts are done.
    Ok(session)
}

/// Try key-based authentication for a tunnel connection.
async fn try_tunnel_key_auth(
    session: &mut client::Handle<TunnelSshHandler>,
    user: &str,
    explicit_key_path: Option<&str>,
) -> Result<bool, String> {
    let key_paths: Vec<std::path::PathBuf> = if let Some(path) = explicit_key_path {
        let expanded = crate::expand_tilde(path);
        log::debug!(
            "tunnel ssh key auth: using explicit key: {}",
            expanded.display()
        );
        vec![expanded]
    } else {
        let home = dirs::home_dir().unwrap_or_default();
        let ssh_dir = home.join(".ssh");
        vec![
            ssh_dir.join("id_ed25519"),
            ssh_dir.join("id_rsa"),
            ssh_dir.join("id_ecdsa"),
        ]
    };

    for key_path in &key_paths {
        if !key_path.exists() {
            continue;
        }

        log::debug!("tunnel ssh key auth: trying {}", key_path.display());
        match russh_keys::load_secret_key(key_path, None) {
            Ok(key) => match session.authenticate_publickey(user, Arc::new(key)).await {
                Ok(true) => return Ok(true),
                Ok(false) => continue,
                Err(e) => {
                    log::warn!(
                        "tunnel ssh key auth: error with {}: {e}",
                        key_path.display()
                    );
                    continue;
                }
            },
            Err(e) => {
                log::warn!(
                    "tunnel ssh key auth: failed to load {}: {e}",
                    key_path.display()
                );
                continue;
            }
        }
    }

    Ok(false)
}

/// Connect to an SSH server via a ProxyCommand for tunnel use.
async fn connect_tunnel_via_proxy(
    proxy_cmd: &str,
    host: &str,
    port: u16,
    config: Arc<client::Config>,
    handler: TunnelSshHandler,
) -> Result<client::Handle<TunnelSshHandler>, String> {
    use tokio::process::Command;

    let expanded = proxy_cmd
        .replace("%h", host)
        .replace("%p", &port.to_string());

    #[cfg(unix)]
    let child = Command::new("sh")
        .arg("-lc")
        .arg(&expanded)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::inherit())
        .spawn()
        .map_err(|e| format!("Failed to spawn ProxyCommand: {e}"))?;

    #[cfg(windows)]
    let child = Command::new("cmd")
        .arg("/C")
        .arg(&expanded)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::inherit())
        .spawn()
        .map_err(|e| format!("Failed to spawn ProxyCommand: {e}"))?;

    let stdin = child.stdin.unwrap();
    let stdout = child.stdout.unwrap();
    let stream = tokio::io::join(stdout, stdin);

    client::connect_stream(config, stream, handler)
        .await
        .map_err(|e| format!("Connection via proxy failed: {e}"))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_tunnel_description() {
        let t = SavedTunnel {
            id: Uuid::new_v4(),
            label: "test".into(),
            session_key: "user@host:22".into(),
            local_port: 8080,
            remote_host: "localhost".into(),
            remote_port: 80,
            auto_start: false,
        };
        assert_eq!(t.description(), ":8080 -> localhost:80");
    }

    #[test]
    fn make_session_key() {
        assert_eq!(
            SavedTunnel::make_session_key("root", "example.com", 22),
            "root@example.com:22"
        );
        assert_eq!(
            SavedTunnel::make_session_key("admin", "10.0.0.1", 2222),
            "admin@10.0.0.1:2222"
        );
    }

    #[test]
    fn parse_session_key_valid() {
        let (user, host, port) = SavedTunnel::parse_session_key("root@example.com:22").unwrap();
        assert_eq!(user, "root");
        assert_eq!(host, "example.com");
        assert_eq!(port, 22);
    }

    #[test]
    fn parse_session_key_invalid() {
        assert!(SavedTunnel::parse_session_key("garbage").is_none());
        assert!(SavedTunnel::parse_session_key("user@host").is_none());
    }

    #[test]
    fn serde_roundtrip() {
        let t = SavedTunnel {
            id: Uuid::new_v4(),
            label: "web".into(),
            session_key: "user@host:22".into(),
            local_port: 3000,
            remote_host: "127.0.0.1".into(),
            remote_port: 3000,
            auto_start: true,
        };
        let json = serde_json::to_string(&t).unwrap();
        let parsed: SavedTunnel = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.id, t.id);
        assert_eq!(parsed.label, "web");
        assert_eq!(parsed.local_port, 3000);
        assert!(parsed.auto_start);
    }

    #[test]
    fn serde_defaults() {
        let json = r#"{
            "label": "db",
            "session_key": "u@h:22",
            "local_port": 5432,
            "remote_host": "localhost",
            "remote_port": 5432
        }"#;
        let t: SavedTunnel = serde_json::from_str(json).unwrap();
        assert!(!t.auto_start);
        // id should have been generated
        assert!(!t.id.is_nil());
    }

    #[tokio::test]
    async fn tunnel_manager_lifecycle() {
        let mgr = TunnelManager::new();
        let id = Uuid::new_v4();
        assert!(!mgr.is_active(&id).await);
        assert!(mgr.active_ids().await.is_empty());
    }

    #[tokio::test]
    async fn tunnel_manager_stop_nonexistent() {
        let mgr = TunnelManager::new();
        // Should not panic.
        mgr.stop(&Uuid::new_v4()).await;
    }

    #[tokio::test]
    async fn tunnel_manager_stop_all_empty() {
        let mgr = TunnelManager::new();
        mgr.stop_all().await;
        assert!(mgr.active_ids().await.is_empty());
    }

    #[tokio::test]
    async fn tunnel_status_transitions() {
        let mgr = TunnelManager::new();
        let id = Uuid::new_v4();

        assert!(mgr.status(&id).await.is_none());

        mgr.set_connecting(id).await;
        assert!(matches!(mgr.status(&id).await, Some(TunnelStatus::Connecting)));
        assert!(!mgr.is_active(&id).await);

        mgr.set_error(&id, "test error".into()).await;
        assert!(matches!(mgr.status(&id).await, Some(TunnelStatus::Error(_))));

        mgr.clear_status(&id).await;
        assert!(mgr.status(&id).await.is_none());
    }
}

//! SSH tunnel management — local port forwarding via standalone SSH connections.
//!
//! Each tunnel establishes its own SSH connection (no terminal/PTY). The
//! `TunnelManager` tracks active tunnels as tokio tasks.

use std::collections::HashMap;
use std::sync::Arc;

use russh::client;
use serde::Serialize;
use tokio::net::TcpListener;
use tokio::sync::{mpsc, Mutex};
use uuid::Uuid;

use super::config::ServerEntry;
use super::known_hosts;
use super::ssh::expand_tilde;

// ---------------------------------------------------------------------------
// Tunnel status
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TunnelStatus {
    Connecting,
    Active,
    Error(String),
}

#[derive(Serialize)]
pub(crate) struct TunnelInfo {
    pub id: String,
    pub status: TunnelStatus,
}

struct TunnelState {
    status: TunnelStatus,
    abort_handle: Option<tokio::task::AbortHandle>,
}

// ---------------------------------------------------------------------------
// Tunnel-only SSH handler (no PTY, no terminal tab)
// ---------------------------------------------------------------------------

/// Auth prompt sent from the tunnel SSH handler.
pub(crate) enum TunnelPrompt {
    ConfirmHostKey {
        message: String,
        detail: String,
        reply: tokio::sync::oneshot::Sender<bool>,
    },
    Password {
        message: String,
        reply: tokio::sync::oneshot::Sender<Option<String>>,
    },
}

struct TunnelSshHandler {
    host: String,
    port: u16,
    prompt_tx: mpsc::Sender<TunnelPrompt>,
}

#[async_trait::async_trait]
impl client::Handler for TunnelSshHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &ssh_key::PublicKey,
    ) -> Result<bool, Self::Error> {
        log::debug!(
            "tunnel ssh: check_server_key called for {}:{}, algo={}",
            self.host,
            self.port,
            server_public_key.algorithm().as_str()
        );
        match known_hosts::check_known_host(&self.host, self.port, server_public_key) {
            Some(true) => {
                log::debug!("tunnel ssh: host key already known and matches for {}:{}", self.host, self.port);
                return Ok(true);
            }
            Some(false) => {
                log::warn!("tunnel ssh: HOST KEY MISMATCH for {}:{}", self.host, self.port);
                let fingerprint = server_public_key.fingerprint(ssh_key::HashAlg::Sha256);
                let message = format!(
                    "WARNING: HOST KEY HAS CHANGED for {}:{}",
                    self.host, self.port
                );
                let detail = format!(
                    "{}\n{fingerprint}",
                    server_public_key.algorithm().as_str(),
                );
                let (tx, rx) = tokio::sync::oneshot::channel();
                log::debug!("tunnel ssh: sending host key mismatch prompt to frontend");
                let _ = self
                    .prompt_tx
                    .send(TunnelPrompt::ConfirmHostKey {
                        message,
                        detail,
                        reply: tx,
                    })
                    .await;
                let result = rx.await.unwrap_or(false);
                log::debug!("tunnel ssh: host key mismatch prompt result: {result}");
                return Ok(result);
            }
            None => {
                log::debug!("tunnel ssh: host key not in known_hosts for {}:{}", self.host, self.port);
            }
        }

        let fingerprint = server_public_key.fingerprint(ssh_key::HashAlg::Sha256);
        let host_display = if self.port != 22 {
            format!("[{}]:{}", self.host, self.port)
        } else {
            self.host.clone()
        };
        let message = format!("The authenticity of host '{host_display}' can't be established.");
        let detail = format!(
            "{} key fingerprint is:\n{fingerprint}",
            server_public_key.algorithm().as_str(),
        );

        let (tx, rx) = tokio::sync::oneshot::channel();
        log::debug!("tunnel ssh: sending new host key confirm prompt to frontend");
        let _ = self
            .prompt_tx
            .send(TunnelPrompt::ConfirmHostKey {
                message,
                detail,
                reply: tx,
            })
            .await;

        let accepted = rx.await.unwrap_or(false);
        log::debug!("tunnel ssh: new host key confirm result: {accepted}");
        if accepted {
            if let Err(e) =
                known_hosts::add_known_host(&self.host, self.port, server_public_key)
            {
                log::warn!("tunnel ssh: failed to save host key: {e}");
            }
        }
        Ok(accepted)
    }
}

// ---------------------------------------------------------------------------
// Tunnel manager
// ---------------------------------------------------------------------------

/// Manages active SSH port-forwarding tunnels.
pub(crate) struct TunnelManager {
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

    pub async fn set_connecting(&self, id: Uuid) {
        self.tunnels.lock().await.insert(
            id,
            TunnelState {
                status: TunnelStatus::Connecting,
                abort_handle: None,
            },
        );
    }

    pub async fn set_error(&self, id: &Uuid, msg: String) {
        if let Some(state) = self.tunnels.lock().await.get_mut(id) {
            state.status = TunnelStatus::Error(msg);
        }
    }

    /// Clear an error state so the tunnel can be retried.
    pub async fn clear_error(&self, id: &Uuid) {
        let mut tunnels = self.tunnels.lock().await;
        if let Some(state) = tunnels.get(id) {
            if matches!(state.status, TunnelStatus::Error(_)) {
                tunnels.remove(id);
            }
        }
    }

    /// Start a tunnel: bind local port, establish SSH, forward via direct-tcpip.
    pub async fn start_tunnel(
        &self,
        id: Uuid,
        server: &ServerEntry,
        local_port: u16,
        remote_host: String,
        remote_port: u16,
        prompt_tx: mpsc::Sender<TunnelPrompt>,
    ) -> Result<(), String> {
        log::info!(
            "tunnel[{id}]: starting — local_port={local_port}, remote={remote_host}:{remote_port}, \
             server={}@{}:{}, auth={}",
            server.user, server.host, server.port, server.auth_method
        );

        // Bind local port first for fast failure.
        log::debug!("tunnel[{id}]: binding 127.0.0.1:{local_port}");
        let listener = TcpListener::bind(format!("127.0.0.1:{local_port}"))
            .await
            .map_err(|e| {
                log::error!("tunnel[{id}]: failed to bind local port {local_port}: {e}");
                format!("Failed to bind local port {local_port}: {e}")
            })?;

        log::info!("tunnel[{id}]: listening on 127.0.0.1:{local_port}");

        log::debug!("tunnel[{id}]: initiating SSH connection to {}@{}:{}", server.user, server.host, server.port);
        let ssh_handle = connect_for_tunnel(server, prompt_tx).await.map_err(|e| {
            log::error!("tunnel[{id}]: SSH connection failed: {e}");
            e
        })?;

        log::info!("tunnel[{id}]: SSH connection established");

        log::debug!("tunnel[{id}]: spawning accept loop for forwarding {remote_host}:{remote_port}");
        let join_handle = tokio::spawn(async move {
            log::debug!("tunnel[{id}]: accept loop started, waiting for connections");
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

                log::debug!(
                    "tunnel[{id}]: opening direct-tcpip channel to {remote_host}:{remote_port} for {peer_addr}"
                );
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
                        log::error!("tunnel[{id}]: direct-tcpip failed for {peer_addr}: {e}");
                        if e.to_string().contains("disconnect")
                            || e.to_string().contains("closed")
                        {
                            log::warn!("tunnel[{id}]: SSH session appears disconnected, stopping accept loop");
                            break;
                        }
                        continue;
                    }
                };

                let tunnel_id = id;
                log::debug!("tunnel[{id}]: spawning bidirectional copy for {peer_addr}");
                tokio::spawn(async move {
                    let mut channel_stream = channel.into_stream();
                    match tokio::io::copy_bidirectional(&mut local_stream, &mut channel_stream)
                        .await
                    {
                        Ok((tx, rx)) => {
                            log::debug!(
                                "tunnel[{tunnel_id}]: closed {peer_addr} (tx={tx}, rx={rx})"
                            );
                        }
                        Err(e) => {
                            log::debug!("tunnel[{tunnel_id}]: copy error {peer_addr}: {e}");
                        }
                    }
                });
            }
            log::info!("tunnel[{id}]: accept loop exited");
        });

        let abort_handle = join_handle.abort_handle();
        self.tunnels.lock().await.insert(
            id,
            TunnelState {
                status: TunnelStatus::Active,
                abort_handle: Some(abort_handle),
            },
        );

        Ok(())
    }

    pub async fn stop(&self, id: &Uuid) {
        log::info!("tunnel[{id}]: stop requested");
        if let Some(state) = self.tunnels.lock().await.remove(id) {
            if let Some(handle) = state.abort_handle {
                log::debug!("tunnel[{id}]: aborting task");
                handle.abort();
            } else {
                log::debug!("tunnel[{id}]: no abort handle (was in {:?} state)", state.status);
            }
        } else {
            log::debug!("tunnel[{id}]: not found in active tunnels");
        }
    }

    pub async fn status(&self, id: &Uuid) -> Option<TunnelStatus> {
        self.tunnels.lock().await.get(id).map(|s| s.status.clone())
    }

    pub async fn is_active(&self, id: &Uuid) -> bool {
        matches!(
            self.tunnels.lock().await.get(id).map(|s| &s.status),
            Some(TunnelStatus::Active)
        )
    }

    pub async fn stop_all(&self) {
        let mut tunnels = self.tunnels.lock().await;
        for (id, state) in tunnels.drain() {
            if let Some(handle) = state.abort_handle {
                log::info!("tunnel[{id}]: stopping");
                handle.abort();
            }
        }
    }

    pub async fn all_statuses(&self) -> Vec<(Uuid, TunnelStatus)> {
        self.tunnels
            .lock()
            .await
            .iter()
            .map(|(id, s)| (*id, s.status.clone()))
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Standalone SSH connection for tunnels
// ---------------------------------------------------------------------------

async fn connect_for_tunnel(
    server: &ServerEntry,
    prompt_tx: mpsc::Sender<TunnelPrompt>,
) -> Result<client::Handle<TunnelSshHandler>, String> {
    log::debug!(
        "connect_for_tunnel: server={}@{}:{}, auth_method={}, key_path={:?}, proxy_command={:?}, proxy_jump={:?}",
        server.user, server.host, server.port, server.auth_method,
        server.key_path, server.proxy_command, server.proxy_jump
    );

    let config = Arc::new(client::Config::default());
    let handler = TunnelSshHandler {
        host: server.host.clone(),
        port: server.port,
        prompt_tx: prompt_tx.clone(),
    };

    let effective_proxy = server
        .proxy_command
        .clone()
        .or_else(|| {
            server
                .proxy_jump
                .as_ref()
                .map(|jump| format!("ssh -W %h:%p {jump}"))
        });

    log::debug!("connect_for_tunnel: effective_proxy={effective_proxy:?}");

    let mut session = if let Some(proxy_cmd) = &effective_proxy {
        log::debug!("connect_for_tunnel: connecting via proxy command: {proxy_cmd}");
        connect_tunnel_via_proxy(proxy_cmd, &server.host, server.port, config, handler).await.map_err(|e| {
            log::error!("connect_for_tunnel: proxy connection failed: {e}");
            e
        })?
    } else {
        let addr = format!("{}:{}", server.host, server.port);
        log::debug!("connect_for_tunnel: direct TCP connect to {addr}");
        client::connect(config, &addr, handler)
            .await
            .map_err(|e| {
                log::error!("connect_for_tunnel: direct connection failed: {e}");
                format!("Connection failed: {e}")
            })?
    };

    log::debug!("connect_for_tunnel: SSH transport established, starting authentication");

    // Authenticate.
    let authenticated = if server.auth_method == "password" {
        log::debug!("connect_for_tunnel: using password auth for user '{}'", server.user);
        let (tx, rx) = tokio::sync::oneshot::channel();
        let msg = format!(
            "Password for {}@{}:{}",
            server.user, server.host, server.port
        );
        log::debug!("connect_for_tunnel: sending password prompt to frontend");
        let _ = prompt_tx
            .send(TunnelPrompt::Password {
                message: msg,
                reply: tx,
            })
            .await;

        log::debug!("connect_for_tunnel: waiting for password response from frontend");
        let password = rx
            .await
            .map_err(|_| {
                log::error!("connect_for_tunnel: password prompt channel closed (cancelled)");
                "Password prompt cancelled".to_string()
            })?
            .ok_or_else(|| {
                log::error!("connect_for_tunnel: user cancelled password prompt");
                "Password prompt cancelled".to_string()
            })?;

        log::debug!("connect_for_tunnel: got password, attempting authenticate_password");
        session
            .authenticate_password(&server.user, &password)
            .await
            .map_err(|e| {
                log::error!("connect_for_tunnel: password auth failed: {e}");
                format!("Auth failed: {e}")
            })?
    } else {
        log::debug!("connect_for_tunnel: using key auth for user '{}', key_path={:?}", server.user, server.key_path);
        try_tunnel_key_auth(&mut session, &server.user, server.key_path.as_deref()).await?
    };

    log::debug!("connect_for_tunnel: authentication result: authenticated={authenticated}");

    if !authenticated {
        log::error!(
            "connect_for_tunnel: authentication failed for {}@{}",
            server.user, server.host
        );
        return Err(format!(
            "Authentication failed for {}@{}",
            server.user, server.host
        ));
    }

    log::info!("connect_for_tunnel: successfully connected and authenticated {}@{}:{}", server.user, server.host, server.port);
    Ok(session)
}

async fn try_tunnel_key_auth(
    session: &mut client::Handle<TunnelSshHandler>,
    user: &str,
    explicit_key_path: Option<&str>,
) -> Result<bool, String> {
    log::debug!("try_tunnel_key_auth: user={user}, explicit_key_path={explicit_key_path:?}");

    let key_paths: Vec<std::path::PathBuf> = if let Some(path) = explicit_key_path {
        let expanded = expand_tilde(path);
        log::debug!("try_tunnel_key_auth: using explicit key path: {}", expanded.display());
        vec![expanded]
    } else {
        let home = dirs::home_dir().unwrap_or_default();
        let ssh_dir = home.join(".ssh");
        log::debug!("try_tunnel_key_auth: no explicit key, scanning default keys in {}", ssh_dir.display());
        vec![
            ssh_dir.join("id_ed25519"),
            ssh_dir.join("id_rsa"),
            ssh_dir.join("id_ecdsa"),
        ]
    };

    for key_path in &key_paths {
        if !key_path.exists() {
            log::debug!("try_tunnel_key_auth: key not found: {}", key_path.display());
            continue;
        }

        log::debug!("try_tunnel_key_auth: loading key: {}", key_path.display());
        match russh_keys::load_secret_key(key_path, None) {
            Ok(key) => {
                log::debug!("try_tunnel_key_auth: key loaded, attempting publickey auth with {}", key_path.display());
                match session.authenticate_publickey(user, Arc::new(key)).await {
                    Ok(true) => {
                        log::info!("try_tunnel_key_auth: success with {}", key_path.display());
                        return Ok(true);
                    }
                    Ok(false) => {
                        log::debug!("try_tunnel_key_auth: key rejected by server: {}", key_path.display());
                        continue;
                    }
                    Err(e) => {
                        log::warn!("try_tunnel_key_auth: auth error with {}: {e}", key_path.display());
                        continue;
                    }
                }
            }
            Err(e) => {
                log::warn!("try_tunnel_key_auth: failed to load key {}: {e}", key_path.display());
                continue;
            }
        }
    }

    log::warn!("try_tunnel_key_auth: no key succeeded for user '{user}'");
    Ok(false)
}

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

    log::debug!("connect_tunnel_via_proxy: expanded command: {expanded}");

    #[cfg(unix)]
    let child = Command::new("sh")
        .arg("-lc")
        .arg(&expanded)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::inherit())
        .spawn()
        .map_err(|e| {
            log::error!("connect_tunnel_via_proxy: failed to spawn: {e}");
            format!("Failed to spawn ProxyCommand: {e}")
        })?;

    #[cfg(windows)]
    let child = {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        log::debug!("connect_tunnel_via_proxy: spawning cmd /C {expanded}");
        Command::new("cmd")
            .arg("/C")
            .arg(&expanded)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::inherit())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .map_err(|e| {
                log::error!("connect_tunnel_via_proxy: failed to spawn: {e}");
                format!("Failed to spawn ProxyCommand: {e}")
            })?
    };

    log::debug!("connect_tunnel_via_proxy: proxy process spawned, connecting SSH stream");
    let stdin = child.stdin.unwrap();
    let stdout = child.stdout.unwrap();
    let stream = tokio::io::join(stdout, stdin);

    let result = client::connect_stream(config, stream, handler)
        .await
        .map_err(|e| {
            log::error!("connect_tunnel_via_proxy: SSH over proxy failed: {e}");
            format!("Connection via proxy failed: {e}")
        });

    log::debug!("connect_tunnel_via_proxy: connect_stream result: {}", if result.is_ok() { "ok" } else { "err" });
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn tunnel_manager_lifecycle() {
        let mgr = TunnelManager::new();
        let id = Uuid::new_v4();
        assert!(!mgr.is_active(&id).await);
        assert!(mgr.all_statuses().await.is_empty());
    }

    #[tokio::test]
    async fn tunnel_manager_stop_nonexistent() {
        let mgr = TunnelManager::new();
        mgr.stop(&Uuid::new_v4()).await;
    }

    #[tokio::test]
    async fn tunnel_manager_stop_all_empty() {
        let mgr = TunnelManager::new();
        mgr.stop_all().await;
        assert!(mgr.all_statuses().await.is_empty());
    }

    #[tokio::test]
    async fn tunnel_status_transitions() {
        let mgr = TunnelManager::new();
        let id = Uuid::new_v4();

        assert!(mgr.status(&id).await.is_none());

        mgr.set_connecting(id).await;
        assert!(matches!(
            mgr.status(&id).await,
            Some(TunnelStatus::Connecting)
        ));
        assert!(!mgr.is_active(&id).await);

        mgr.set_error(&id, "test error".into()).await;
        assert!(matches!(
            mgr.status(&id).await,
            Some(TunnelStatus::Error(_))
        ));
    }

    #[test]
    fn tunnel_status_serializes() {
        let status = TunnelStatus::Active;
        let json = serde_json::to_string(&status).unwrap();
        assert_eq!(json, "\"active\"");
    }

    #[test]
    fn tunnel_info_serializes() {
        let info = TunnelInfo {
            id: Uuid::new_v4().to_string(),
            status: TunnelStatus::Connecting,
        };
        let json = serde_json::to_string(&info).unwrap();
        assert!(json.contains("\"connecting\""));
    }
}

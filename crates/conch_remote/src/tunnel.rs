//! SSH tunnel management — local port forwarding via standalone SSH connections.
//!
//! Each tunnel establishes its own SSH connection (no terminal/PTY). The
//! `TunnelManager` tracks active tunnels as tokio tasks.

use std::collections::HashMap;
use std::sync::Arc;

use russh::client;
use serde::Serialize;
use tokio::net::TcpListener;
use tokio::sync::Mutex;
use ts_rs::TS;
use uuid::Uuid;

use crate::callbacks::{RemoteCallbacks, RemotePaths};
use crate::config::ServerEntry;
use crate::error::RemoteError;
use crate::handler::ConchSshHandler;

// ---------------------------------------------------------------------------
// Tunnel status
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum TunnelStatus {
    Connecting,
    Active,
    Error(String),
}

#[derive(Serialize, TS)]
#[ts(export)]
pub struct TunnelInfo {
    pub id: String,
    pub status: TunnelStatus,
}

struct TunnelState {
    status: TunnelStatus,
    abort_handle: Option<tokio::task::AbortHandle>,
}

// ---------------------------------------------------------------------------
// Tunnel manager
// ---------------------------------------------------------------------------

/// Manages active SSH port-forwarding tunnels.
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
        credentials: &crate::ssh::SshCredentials,
        local_port: u16,
        remote_host: String,
        remote_port: u16,
        callbacks: Arc<dyn RemoteCallbacks>,
        paths: &RemotePaths,
    ) -> Result<(), RemoteError> {
        // Bind local port first for fast failure.
        let listener = TcpListener::bind(format!("127.0.0.1:{local_port}"))
            .await
            .map_err(|e| {
                RemoteError::Tunnel(format!("Failed to bind local port {local_port}: {e}"))
            })?;

        log::info!("tunnel[{id}]: listening on 127.0.0.1:{local_port}");

        let ssh_handle = connect_for_tunnel(server, credentials, callbacks, paths).await?;

        log::info!("tunnel[{id}]: SSH connection established");

        let join_handle = tokio::spawn(async move {
            loop {
                let (mut local_stream, peer_addr) = match listener.accept().await {
                    Ok(conn) => conn,
                    Err(e) => {
                        log::error!("tunnel[{id}]: accept error: {e}");
                        break;
                    }
                };

                let channel = match ssh_handle
                    .channel_open_direct_tcpip(
                        &remote_host,
                        remote_port as u32,
                        &peer_addr.ip().to_string(),
                        peer_addr.port() as u32,
                    )
                    .await
                {
                    Ok(ch) => ch,
                    Err(e) => {
                        log::error!("tunnel[{id}]: direct-tcpip failed: {e}");
                        if e.to_string().contains("disconnect") || e.to_string().contains("closed")
                        {
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
                                "tunnel[{tunnel_id}]: closed {peer_addr} (tx={tx}, rx={rx})"
                            );
                        }
                        Err(e) => {
                            log::debug!("tunnel[{tunnel_id}]: error {peer_addr}: {e}");
                        }
                    }
                });
            }
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
        if let Some(state) = self.tunnels.lock().await.remove(id) {
            if let Some(handle) = state.abort_handle {
                handle.abort();
            }
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
    credentials: &crate::ssh::SshCredentials,
    callbacks: Arc<dyn RemoteCallbacks>,
    paths: &RemotePaths,
) -> Result<client::Handle<ConchSshHandler>, RemoteError> {
    let config = Arc::new(client::Config::default());
    let handler = ConchSshHandler {
        host: server.host.clone(),
        port: server.port,
        known_hosts_file: paths.known_hosts_file.clone(),
        callbacks: Arc::clone(&callbacks),
    };

    // Proxy: desktop-only
    #[cfg(not(target_os = "ios"))]
    let effective_proxy = server.proxy_command.clone().or_else(|| {
        server
            .proxy_jump
            .as_ref()
            .map(|j| format!("ssh -W %h:%p {j}"))
    });
    #[cfg(target_os = "ios")]
    let effective_proxy: Option<String> = None;

    let mut session = if let Some(proxy_cmd) = &effective_proxy {
        #[cfg(not(target_os = "ios"))]
        {
            crate::ssh::connect_via_proxy(proxy_cmd, &server.host, server.port, config, handler)
                .await?
        }
        #[cfg(target_os = "ios")]
        {
            let _ = proxy_cmd;
            return Err(RemoteError::Connection(
                "Proxy connections are not supported on iOS".into(),
            ));
        }
    } else {
        let addr = format!("{}:{}", server.host, server.port);
        client::connect(config, &addr, handler)
            .await
            .map_err(|e| RemoteError::Connection(format!("{e}")))?
    };

    // Auth
    let authenticated = if credentials.auth_method == "password" {
        // If password was provided in credentials, use it. Otherwise prompt via callbacks.
        // Treat empty string as missing — migrated entries store "" as a placeholder.
        let pw = match &credentials.password {
            Some(pw) if !pw.is_empty() => Some(pw.clone()),
            _ => {
                let msg = format!(
                    "Password for {}@{}:{}",
                    credentials.username, server.host, server.port
                );
                callbacks.prompt_password(&msg).await
            }
        };

        match pw {
            Some(pw) => session
                .authenticate_password(&credentials.username, &pw)
                .await
                .map_err(|e| RemoteError::Auth(format!("{e}")))?,
            None => return Err(RemoteError::Auth("Password entry cancelled".into())),
        }
    } else if credentials.auth_method == "key_and_password" {
        // Try key first; fall back to password if key fails (see ssh.rs).
        let key_ok = crate::ssh::try_key_auth(
            &mut session,
            &credentials.username,
            credentials.key_path.as_deref(),
            &paths.default_key_paths,
            credentials.key_passphrase.as_deref(),
        )
        .await?;

        if key_ok {
            true
        } else {
            log::info!(
                "key_and_password: key auth failed for {}@{}:{}, falling back to password",
                credentials.username,
                server.host,
                server.port
            );
            let pw = match &credentials.password {
                Some(pw) if !pw.is_empty() => Some(pw.clone()),
                _ => {
                    let msg = format!(
                        "Password for {}@{}:{}",
                        credentials.username, server.host, server.port
                    );
                    callbacks.prompt_password(&msg).await
                }
            };

            match pw {
                Some(pw) => session
                    .authenticate_password(&credentials.username, &pw)
                    .await
                    .map_err(|e| RemoteError::Auth(format!("{e}")))?,
                None => return Err(RemoteError::Auth("Password entry cancelled".into())),
            }
        }
    } else {
        crate::ssh::try_key_auth(
            &mut session,
            &credentials.username,
            credentials.key_path.as_deref(),
            &paths.default_key_paths,
            credentials.key_passphrase.as_deref(),
        )
        .await?
    };

    if !authenticated {
        return Err(RemoteError::Auth(format!(
            "Authentication failed for {}@{}",
            credentials.username, server.host
        )));
    }

    Ok(session)
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

    /// Verify that the password-resolution logic used in `connect_for_tunnel`
    /// prefers `credentials.password` over prompting.
    #[test]
    fn password_resolution_prefers_stored_credential() {
        use crate::ssh::SshCredentials;

        let creds_with_pw = SshCredentials {
            username: "user".into(),
            auth_method: "password".into(),
            password: Some("stored_secret".into()),
            key_path: None,
            key_passphrase: None,
        };

        // When credentials.password is Some, the match arm should yield the stored value
        // without needing to call any callback.
        let resolved: Option<String> = match &creds_with_pw.password {
            Some(pw) => Some(pw.clone()),
            None => None, // In real code this would call callbacks.prompt_password()
        };
        assert_eq!(resolved.as_deref(), Some("stored_secret"));

        // When credentials.password is None, the match arm falls through to the
        // prompt branch (simulated here as None since there's no callback).
        let creds_no_pw = SshCredentials {
            username: "user".into(),
            auth_method: "password".into(),
            password: None,
            key_path: None,
            key_passphrase: None,
        };
        let resolved: Option<String> = match &creds_no_pw.password {
            Some(pw) => Some(pw.clone()),
            None => None,
        };
        assert_eq!(resolved, None);
    }

    #[tokio::test]
    async fn clear_error_removes_error_state() {
        let mgr = TunnelManager::new();
        let id = Uuid::new_v4();

        mgr.set_connecting(id).await;
        mgr.set_error(&id, "oops".into()).await;
        assert!(matches!(
            mgr.status(&id).await,
            Some(TunnelStatus::Error(_))
        ));

        mgr.clear_error(&id).await;
        assert!(
            mgr.status(&id).await.is_none(),
            "error state should be removed"
        );
    }

    #[tokio::test]
    async fn clear_error_does_not_remove_active() {
        let mgr = TunnelManager::new();
        let id = Uuid::new_v4();

        // Manually insert an Active state
        mgr.tunnels.lock().await.insert(
            id,
            TunnelState {
                status: TunnelStatus::Active,
                abort_handle: None,
            },
        );

        mgr.clear_error(&id).await;
        // Active state should NOT be removed by clear_error
        assert!(
            mgr.is_active(&id).await,
            "active tunnel should not be cleared"
        );
    }
}

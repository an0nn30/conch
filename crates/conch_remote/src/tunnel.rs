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
use uuid::Uuid;

use crate::callbacks::{RemoteCallbacks, RemotePaths};
use crate::config::ServerEntry;
use crate::handler::ConchSshHandler;

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
        local_port: u16,
        remote_host: String,
        remote_port: u16,
        callbacks: Arc<dyn RemoteCallbacks>,
        paths: &RemotePaths,
    ) -> Result<(), String> {
        // Bind local port first for fast failure.
        let listener = TcpListener::bind(format!("127.0.0.1:{local_port}"))
            .await
            .map_err(|e| format!("Failed to bind local port {local_port}: {e}"))?;

        log::info!("tunnel[{id}]: listening on 127.0.0.1:{local_port}");

        let ssh_handle = connect_for_tunnel(server, callbacks, paths).await?;

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
                        if e.to_string().contains("disconnect")
                            || e.to_string().contains("closed")
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
    callbacks: Arc<dyn RemoteCallbacks>,
    paths: &RemotePaths,
) -> Result<client::Handle<ConchSshHandler>, String> {
    let config = Arc::new(client::Config::default());
    let handler = ConchSshHandler {
        host: server.host.clone(),
        port: server.port,
        known_hosts_file: paths.known_hosts_file.clone(),
        callbacks: Arc::clone(&callbacks),
    };

    // Proxy: desktop-only
    #[cfg(not(target_os = "ios"))]
    let effective_proxy = server
        .proxy_command
        .clone()
        .or_else(|| {
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
            return Err("Proxy connections are not supported on iOS".to_string());
        }
    } else {
        let addr = format!("{}:{}", server.host, server.port);
        client::connect(config, &addr, handler)
            .await
            .map_err(|e| format!("Connection failed: {e}"))?
    };

    // Auth
    let authenticated = if server.auth_method == "password" {
        let msg = format!(
            "Password for {}@{}:{}",
            server.user, server.host, server.port
        );
        let password = callbacks
            .prompt_password(&msg)
            .await
            .ok_or_else(|| "Password prompt cancelled".to_string())?;
        session
            .authenticate_password(&server.user, &password)
            .await
            .map_err(|e| format!("Auth failed: {e}"))?
    } else {
        crate::ssh::try_key_auth(
            &mut session,
            &server.user,
            server.key_path.as_deref(),
            &paths.default_key_paths,
        )
        .await?
    };

    if !authenticated {
        return Err(format!(
            "Authentication failed for {}@{}",
            server.user, server.host
        ));
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
}

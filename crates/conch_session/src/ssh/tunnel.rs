use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Context, Result};
use russh::client;
use tokio::net::TcpListener;
use tokio::sync::Mutex;
use uuid::Uuid;

use super::client::ClientHandler;

/// Manages active SSH port-forwarding tunnels.
pub struct TunnelManager {
    active: Arc<Mutex<HashMap<Uuid, TunnelHandle>>>,
}

struct TunnelHandle {
    abort_handle: tokio::task::AbortHandle,
}

impl Clone for TunnelManager {
    fn clone(&self) -> Self {
        Self {
            active: Arc::clone(&self.active),
        }
    }
}

impl TunnelManager {
    pub fn new() -> Self {
        Self {
            active: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Cheap clone (Arc) for passing into async tasks.
    pub fn clone_inner(&self) -> Self {
        self.clone()
    }

    /// Start a local port forward: listens on `local_port` and forwards
    /// connections to `remote_host:remote_port` via the SSH handle.
    pub async fn start_local_forward(
        &self,
        id: Uuid,
        handle: Arc<client::Handle<ClientHandler>>,
        local_port: u16,
        remote_host: String,
        remote_port: u16,
    ) -> Result<()> {
        log::info!(
            "tunnel[{id}]: binding 127.0.0.1:{local_port} -> {remote_host}:{remote_port}",
        );
        let listener = TcpListener::bind(format!("127.0.0.1:{}", local_port))
            .await
            .context("Failed to bind local port for tunnel")?;
        log::info!("tunnel[{id}]: listening on 127.0.0.1:{local_port}");

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

                let remote_host = remote_host.clone();
                let handle = Arc::clone(&handle);

                tokio::spawn(async move {
                    log::debug!(
                        "tunnel[{id}]: opening direct-tcpip channel to {remote_host}:{remote_port} for {peer_addr}",
                    );
                    let channel = match handle
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
                            log::error!("tunnel[{id}]: failed to open direct-tcpip channel: {e}");
                            return;
                        }
                    };

                    let mut channel_stream = channel.into_stream();
                    match tokio::io::copy_bidirectional(&mut local_stream, &mut channel_stream).await {
                        Ok((tx, rx)) => log::debug!("tunnel[{id}]: connection closed for {peer_addr} (tx={tx}, rx={rx})"),
                        Err(e) => log::debug!("tunnel[{id}]: connection error for {peer_addr}: {e}"),
                    }
                });
            }
        });

        let abort_handle = join_handle.abort_handle();
        self.active.lock().await.insert(id, TunnelHandle { abort_handle });
        log::info!("tunnel[{id}]: registered as active");

        Ok(())
    }

    /// Stop a running tunnel.
    pub async fn stop(&self, id: &Uuid) {
        if let Some(handle) = self.active.lock().await.remove(id) {
            handle.abort_handle.abort();
        }
    }

    /// Check whether a tunnel is currently active.
    pub async fn is_active(&self, id: &Uuid) -> bool {
        self.active.lock().await.contains_key(id)
    }

    /// Stop all tunnels.
    pub async fn stop_all(&self) {
        let mut active = self.active.lock().await;
        for (_, handle) in active.drain() {
            handle.abort_handle.abort();
        }
    }
}

use std::net::ToSocketAddrs;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use russh::client::{self, Handle};
use russh::keys::{self, PrivateKeyWithHashAlg, agent};
use russh::Channel;

use conch_core::models::server::ServerEntry;

/// SSH connection parameters.
pub struct ConnectParams {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub identity_file: Option<PathBuf>,
    pub password: Option<String>,
    pub proxy_command: Option<String>,
    pub proxy_jump: Option<String>,
}

impl From<&ServerEntry> for ConnectParams {
    fn from(entry: &ServerEntry) -> Self {
        Self {
            host: entry.host.clone(),
            port: entry.port,
            user: entry.user.clone(),
            identity_file: entry.identity_file.as_ref().map(PathBuf::from),
            password: None,
            proxy_command: entry.proxy_command.clone(),
            proxy_jump: entry.proxy_jump.clone(),
        }
    }
}

/// An established SSH session with a shell channel.
pub struct SshConnection {
    pub handle: Handle<ClientHandler>,
    pub channel: Channel<client::Msg>,
}

impl SshConnection {
    /// Send data to the remote shell.
    pub async fn write(&self, data: &[u8]) -> Result<()> {
        self.channel.data(&data[..]).await?;
        Ok(())
    }

    /// Send a window change (resize) to the remote PTY.
    pub async fn resize(&self, cols: u32, rows: u32) -> Result<()> {
        self.channel.window_change(cols, rows, 0, 0).await?;
        Ok(())
    }

    /// Close the SSH channel and disconnect.
    pub async fn close(self) -> Result<()> {
        self.channel.eof().await?;
        self.channel.close().await?;
        self.handle
            .disconnect(russh::Disconnect::ByApplication, "", "")
            .await?;
        Ok(())
    }
}

/// Connect to an SSH server and open an interactive shell.
pub async fn connect_shell(
    params: &ConnectParams,
    cols: u32,
    rows: u32,
) -> Result<SshConnection> {
    // Determine the effective proxy command: explicit proxy_command takes
    // priority, then proxy_jump is converted to `ssh -W %h:%p <jump>`.
    let effective_proxy = params
        .proxy_command
        .clone()
        .or_else(|| {
            params.proxy_jump.as_ref().map(|jump| {
                format!("ssh -W %h:%p {jump}")
            })
        });

    let mut handle = if let Some(proxy_cmd) = &effective_proxy {
        super::proxy::connect_via_proxy(proxy_cmd, params).await?
    } else {
        let config = Arc::new(client::Config::default());
        let handler = ClientHandler;

        let addr = format!("{}:{}", params.host, params.port);
        let sock_addr = addr
            .to_socket_addrs()
            .context("Failed to resolve SSH host")?
            .next()
            .context("No address found for SSH host")?;

        client::connect(config, sock_addr, handler)
            .await
            .context("Failed to connect to SSH server")?
    };

    // Authenticate
    authenticate(&mut handle, &params.user, &params.identity_file, &params.password).await?;

    // Open a session channel
    let channel = handle
        .channel_open_session()
        .await
        .context("Failed to open session channel")?;

    // Request PTY
    channel
        .request_pty(true, "xterm-256color", cols, rows, 0, 0, &[])
        .await
        .context("Failed to request PTY")?;

    // Request shell
    channel
        .request_shell(true)
        .await
        .context("Failed to request shell")?;

    Ok(SshConnection { handle, channel })
}

/// Authenticate using a cascade: explicit key → default keys → agent → password.
async fn authenticate(
    handle: &mut Handle<ClientHandler>,
    user: &str,
    identity_file: &Option<PathBuf>,
    password: &Option<String>,
) -> Result<()> {
    // 1. Try explicit identity file
    if let Some(path) = identity_file {
        let expanded = expand_tilde(path);
        if expanded.exists() {
            if try_key_auth(handle, user, &expanded).await? {
                return Ok(());
            }
        }
    }

    // 2. Try default key files
    let home = dirs::home_dir().unwrap_or_default();
    let default_keys: [PathBuf; 3] = [
        home.join(".ssh/id_ed25519"),
        home.join(".ssh/id_ecdsa"),
        home.join(".ssh/id_rsa"),
    ];

    for key_path in &default_keys {
        if key_path.exists() {
            if try_key_auth(handle, user, key_path).await? {
                return Ok(());
            }
        }
    }

    // 3. Try SSH agent
    if try_agent_auth(handle, user).await? {
        return Ok(());
    }

    // 4. Try password
    if let Some(pass) = password {
        let result = handle
            .authenticate_password(user, pass)
            .await
            .context("Password authentication failed")?;
        if result.success() {
            return Ok(());
        }
    }

    bail!("All authentication methods failed for user '{}'", user)
}

/// Try authenticating with a private key file.
async fn try_key_auth(
    handle: &mut Handle<ClientHandler>,
    user: &str,
    path: &std::path::Path,
) -> Result<bool> {
    match keys::load_secret_key(path, None) {
        Ok(key) => {
            let key_with_alg = PrivateKeyWithHashAlg::new(Arc::new(key), None);
            match handle.authenticate_publickey(user, key_with_alg).await {
                Ok(result) => Ok(result.success()),
                Err(_) => Ok(false),
            }
        }
        Err(_) => Ok(false), // Key couldn't be loaded (wrong format, encrypted, etc.)
    }
}

/// Try authenticating via SSH agent.
async fn try_agent_auth(
    handle: &mut Handle<ClientHandler>,
    user: &str,
) -> Result<bool> {
    let agent_path = match std::env::var("SSH_AUTH_SOCK") {
        Ok(path) => path,
        Err(_) => return Ok(false),
    };

    let stream = match tokio::net::UnixStream::connect(&agent_path).await {
        Ok(s) => s,
        Err(_) => return Ok(false),
    };

    let mut agent_client = agent::client::AgentClient::connect(stream);

    let identities = match agent_client.request_identities().await {
        Ok(ids) => ids,
        Err(_) => return Ok(false),
    };

    for pubkey in identities {
        match handle
            .authenticate_publickey_with(user, pubkey, None, &mut agent_client)
            .await
        {
            Ok(result) if result.success() => return Ok(true),
            _ => continue,
        }
    }

    Ok(false)
}

/// Connect and authenticate to an SSH server for tunnel use only (no PTY/shell).
/// Returns the raw handle for port forwarding.
pub async fn connect_tunnel(params: &ConnectParams) -> Result<Arc<Handle<ClientHandler>>> {
    log::debug!(
        "connect_tunnel: resolving {}:{} (proxy_command={:?}, proxy_jump={:?})",
        params.host, params.port, params.proxy_command, params.proxy_jump,
    );

    let effective_proxy = params
        .proxy_command
        .clone()
        .or_else(|| {
            params.proxy_jump.as_ref().map(|jump| {
                format!("ssh -W %h:%p {jump}")
            })
        });

    let mut handle = if let Some(ref proxy_cmd) = effective_proxy {
        log::debug!("connect_tunnel: connecting via proxy: {proxy_cmd}");
        super::proxy::connect_via_proxy(proxy_cmd, params).await?
    } else {
        let config = Arc::new(client::Config::default());
        let handler = ClientHandler;

        let addr = format!("{}:{}", params.host, params.port);
        log::debug!("connect_tunnel: resolving address {addr}");
        let sock_addr = addr
            .to_socket_addrs()
            .context("Failed to resolve SSH host")?
            .next()
            .context("No address found for SSH host")?;

        log::debug!("connect_tunnel: TCP connecting to {sock_addr}");
        client::connect(config, sock_addr, handler)
            .await
            .context("Failed to connect to SSH server")?
    };

    log::debug!("connect_tunnel: TCP connected, authenticating as '{}'", params.user);
    authenticate(&mut handle, &params.user, &params.identity_file, &params.password).await?;
    log::debug!("connect_tunnel: authentication succeeded");

    Ok(Arc::new(handle))
}

/// Expand ~ to home directory in paths.
fn expand_tilde(path: &std::path::Path) -> PathBuf {
    if let Some(s) = path.to_str() {
        if s.starts_with("~/") {
            if let Some(home) = dirs::home_dir() {
                return home.join(&s[2..]);
            }
        }
    }
    path.to_path_buf()
}

/// Client handler for russh — accepts all host keys (for now).
pub struct ClientHandler;

impl client::Handler for ClientHandler {
    type Error = anyhow::Error;

    async fn check_server_key(
        &mut self,
        _server_public_key: &russh::keys::PublicKey,
    ) -> Result<bool, Self::Error> {
        // TODO: Implement known_hosts checking
        Ok(true)
    }
}

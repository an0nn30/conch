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

/// Result of a shell connection attempt.
pub enum ShellConnectResult {
    /// Fully connected with an authenticated shell.
    Connected(SshConnection),
    /// TCP/SSH connected but needs a password to authenticate.
    NeedsPassword(PendingAuth),
}

/// A connection that is waiting for password authentication.
pub struct PendingAuth {
    handle: Handle<ClientHandler>,
    user: String,
    cols: u32,
    rows: u32,
}

impl PendingAuth {
    /// Try password authentication. Returns `true` on success, `false` on wrong password.
    /// Returns an error only on connection/protocol failure.
    pub async fn try_password(&mut self, password: &str) -> Result<bool> {
        let result = self.handle
            .authenticate_password(&self.user, password)
            .await
            .context("Password authentication failed")?;
        Ok(result.success())
    }

    /// Open a shell after successful authentication. Consumes self.
    pub async fn open_shell(self) -> Result<SshConnection> {
        open_shell_owned(self.handle, self.cols, self.rows).await
    }
}

/// Establish TCP+SSH connection to a server (shared between connect_shell and connect_tunnel).
async fn establish_connection(params: &ConnectParams) -> Result<Handle<ClientHandler>> {
    let effective_proxy = params
        .proxy_command
        .clone()
        .or_else(|| {
            params.proxy_jump.as_ref().map(|jump| {
                format!("ssh -W %h:%p {jump}")
            })
        });

    if let Some(proxy_cmd) = &effective_proxy {
        super::proxy::connect_via_proxy(proxy_cmd, params).await
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
            .context("Failed to connect to SSH server")
    }
}

/// Connect to an SSH server and open an interactive shell.
pub async fn connect_shell(
    params: &ConnectParams,
    cols: u32,
    rows: u32,
) -> Result<ShellConnectResult> {
    let mut handle = establish_connection(params).await?;

    match authenticate(&mut handle, &params.user, &params.identity_file, &params.password).await? {
        AuthResult::Ok => {
            let conn = open_shell_owned(handle, cols, rows).await?;
            Ok(ShellConnectResult::Connected(conn))
        }
        AuthResult::NeedsPassword => {
            Ok(ShellConnectResult::NeedsPassword(PendingAuth {
                handle,
                user: params.user.clone(),
                cols,
                rows,
            }))
        }
    }
}

/// Open a session channel, request PTY and shell — takes ownership of the handle.
async fn open_shell_owned(
    handle: Handle<ClientHandler>,
    cols: u32,
    rows: u32,
) -> Result<SshConnection> {
    let channel = handle
        .channel_open_session()
        .await
        .context("Failed to open session channel")?;

    channel
        .request_pty(true, "xterm-256color", cols, rows, 0, 0, &[])
        .await
        .context("Failed to request PTY")?;

    channel
        .request_shell(true)
        .await
        .context("Failed to request shell")?;

    Ok(SshConnection { handle, channel })
}

/// Result of the authentication cascade.
enum AuthResult {
    /// Successfully authenticated.
    Ok,
    /// Key/agent auth failed but connection is alive — need a password.
    NeedsPassword,
}

/// Authenticate using a cascade: explicit key → default keys → agent → password.
async fn authenticate(
    handle: &mut Handle<ClientHandler>,
    user: &str,
    identity_file: &Option<PathBuf>,
    password: &Option<String>,
) -> Result<AuthResult> {
    // 1. Try explicit identity file
    if let Some(path) = identity_file {
        let expanded = expand_tilde(path);
        if expanded.exists() {
            log::debug!("Trying explicit key: {}", expanded.display());
            if try_key_auth(handle, user, &expanded).await? {
                log::debug!("Authenticated with explicit key: {}", expanded.display());
                return Ok(AuthResult::Ok);
            }
            log::debug!("Explicit key auth failed: {}", expanded.display());
        } else {
            log::debug!("Explicit key not found: {}", expanded.display());
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
            log::debug!("Trying default key: {}", key_path.display());
            if try_key_auth(handle, user, key_path).await? {
                log::debug!("Authenticated with key: {}", key_path.display());
                return Ok(AuthResult::Ok);
            }
            log::debug!("Key auth failed: {}", key_path.display());
        }
    }

    // 3. Try SSH agent
    log::debug!("Trying SSH agent auth (SSH_AUTH_SOCK={:?})", std::env::var("SSH_AUTH_SOCK").ok());
    if try_agent_auth(handle, user).await? {
        log::debug!("Authenticated via SSH agent");
        return Ok(AuthResult::Ok);
    }
    log::debug!("SSH agent auth failed or unavailable");

    // 4. Try password if provided
    if let Some(pass) = password {
        let result = handle
            .authenticate_password(user, pass)
            .await
            .context("Password authentication failed")?;
        if result.success() {
            return Ok(AuthResult::Ok);
        }
        bail!("Password authentication failed for user '{}'", user);
    }

    // No password provided and key/agent auth failed — ask for password
    Ok(AuthResult::NeedsPassword)
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
#[cfg(unix)]
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

/// SSH agent auth is not available on Windows (no Unix domain socket).
#[cfg(not(unix))]
async fn try_agent_auth(
    _handle: &mut Handle<ClientHandler>,
    _user: &str,
) -> Result<bool> {
    Ok(false)
}

/// Connect and authenticate to an SSH server for tunnel use only (no PTY/shell).
/// Returns the raw handle for port forwarding.
pub async fn connect_tunnel(params: &ConnectParams) -> Result<Arc<Handle<ClientHandler>>> {
    log::debug!(
        "connect_tunnel: resolving {}:{} (proxy_command={:?}, proxy_jump={:?})",
        params.host, params.port, params.proxy_command, params.proxy_jump,
    );

    let mut handle = establish_connection(params).await?;

    log::debug!("connect_tunnel: TCP connected, authenticating as '{}'", params.user);
    match authenticate(&mut handle, &params.user, &params.identity_file, &params.password).await? {
        AuthResult::Ok => {
            log::debug!("connect_tunnel: authentication succeeded");
            Ok(Arc::new(handle))
        }
        AuthResult::NeedsPassword => {
            bail!("All authentication methods failed for tunnel to '{}' (password required but not provided)", params.user)
        }
    }
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

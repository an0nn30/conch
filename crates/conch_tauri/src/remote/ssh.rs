//! SSH connection logic — handler, auth, proxy support.

use std::path::PathBuf;
use std::sync::Arc;

use russh::client;
use russh::ChannelMsg;
use tokio::process::Command;
use tokio::sync::{mpsc, oneshot};

use super::known_hosts;
use crate::remote::config::ServerEntry;

/// Expand a leading `~` or `~/` to the user's home directory.
pub(crate) fn expand_tilde(path: &str) -> PathBuf {
    if path == "~" {
        return dirs::home_dir().unwrap_or_else(|| PathBuf::from("~"));
    }
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    }
    PathBuf::from(path)
}

// ---------------------------------------------------------------------------
// Auth prompt bridging — async events to frontend, response via oneshot
// ---------------------------------------------------------------------------

/// A request sent to the frontend for user interaction during SSH handshake.
#[derive(Debug)]
pub(crate) enum AuthPrompt {
    /// Ask the user to accept/reject a host key.
    HostKeyConfirm {
        message: String,
        detail: String,
        reply: oneshot::Sender<bool>,
    },
    /// Ask the user for a password.
    PasswordPrompt {
        message: String,
        reply: oneshot::Sender<Option<String>>,
    },
}

// ---------------------------------------------------------------------------
// SSH client handler
// ---------------------------------------------------------------------------

/// The russh client handler — checks host keys via prompts sent to the frontend.
pub(crate) struct SshHandler {
    pub host: String,
    pub port: u16,
    /// Channel to send auth prompts to the frontend bridge.
    pub prompt_tx: mpsc::UnboundedSender<AuthPrompt>,
}

#[async_trait::async_trait]
impl client::Handler for SshHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &ssh_key::PublicKey,
    ) -> Result<bool, Self::Error> {
        match known_hosts::check_known_host(&self.host, self.port, server_public_key) {
            Some(true) => {
                log::debug!(
                    "Host key for {}:{} matches known_hosts",
                    self.host,
                    self.port
                );
                return Ok(true);
            }
            Some(false) => {
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

                let (tx, rx) = oneshot::channel();
                let _ = self.prompt_tx.send(AuthPrompt::HostKeyConfirm {
                    message,
                    detail,
                    reply: tx,
                });
                let accepted = rx.await.unwrap_or(false);
                return Ok(accepted);
            }
            None => {
                // Unknown host — ask the user.
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

        let (tx, rx) = oneshot::channel();
        let _ = self.prompt_tx.send(AuthPrompt::HostKeyConfirm {
            message,
            detail,
            reply: tx,
        });
        let accepted = rx.await.unwrap_or(false);

        if accepted {
            if let Err(e) =
                known_hosts::add_known_host(&self.host, self.port, server_public_key)
            {
                log::warn!("Failed to save host key: {e}");
            }
        }

        Ok(accepted)
    }
}

// ---------------------------------------------------------------------------
// Connection
// ---------------------------------------------------------------------------

/// Establish an SSH connection, authenticate, open a shell channel.
///
/// Returns the client handle and the shell channel. Auth prompts are sent
/// through `prompt_tx` for the caller to bridge to the frontend.
pub(crate) async fn connect_and_open_shell(
    server: &ServerEntry,
    password: Option<String>,
    prompt_tx: mpsc::UnboundedSender<AuthPrompt>,
) -> Result<(client::Handle<SshHandler>, russh::Channel<russh::client::Msg>), String> {
    let config = Arc::new(client::Config::default());
    let handler = SshHandler {
        host: server.host.clone(),
        port: server.port,
        prompt_tx,
    };

    // Determine effective proxy.
    let effective_proxy = server
        .proxy_command
        .clone()
        .or_else(|| server.proxy_jump.as_ref().map(|jump| format!("ssh -W %h:%p {jump}")));

    let mut session = if let Some(proxy_cmd) = &effective_proxy {
        connect_via_proxy(proxy_cmd, &server.host, server.port, config, handler).await?
    } else {
        let addr = format!("{}:{}", server.host, server.port);
        client::connect(config, &addr, handler)
            .await
            .map_err(|e| format!("Connection failed: {e}"))?
    };

    // Authenticate.
    let authenticated = if server.auth_method == "password" {
        let pw = password.as_deref().unwrap_or("");
        session
            .authenticate_password(&server.user, pw)
            .await
            .map_err(|e| format!("Auth failed: {e}"))?
    } else {
        try_key_auth(&mut session, &server.user, server.key_path.as_deref()).await?
    };

    if !authenticated {
        return Err("Authentication failed".to_string());
    }

    // Open shell channel.
    let channel = session
        .channel_open_session()
        .await
        .map_err(|e| format!("Channel open failed: {e}"))?;

    channel
        .request_pty(false, "xterm-256color", 80, 24, 0, 0, &[])
        .await
        .map_err(|e| format!("PTY request failed: {e}"))?;

    channel
        .request_shell(false)
        .await
        .map_err(|e| format!("Shell request failed: {e}"))?;

    Ok((session, channel))
}

/// Connect via a ProxyCommand.
async fn connect_via_proxy(
    proxy_cmd: &str,
    host: &str,
    port: u16,
    config: Arc<client::Config>,
    handler: SshHandler,
) -> Result<client::Handle<SshHandler>, String> {
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

/// Try key-based authentication with common SSH key files.
async fn try_key_auth(
    session: &mut client::Handle<SshHandler>,
    user: &str,
    explicit_key_path: Option<&str>,
) -> Result<bool, String> {
    let key_paths: Vec<PathBuf> = if let Some(path) = explicit_key_path {
        vec![expand_tilde(path)]
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

        match russh_keys::load_secret_key(key_path, None) {
            Ok(key) => {
                match session
                    .authenticate_publickey(user, Arc::new(key))
                    .await
                {
                    Ok(true) => {
                        log::info!("SSH key auth success with {}", key_path.display());
                        return Ok(true);
                    }
                    Ok(false) => continue,
                    Err(e) => {
                        log::warn!("SSH key auth error with {}: {e}", key_path.display());
                        continue;
                    }
                }
            }
            Err(e) => {
                log::warn!("Failed to load SSH key {}: {e}", key_path.display());
                continue;
            }
        }
    }

    Ok(false)
}

// ---------------------------------------------------------------------------
// Channel message types
// ---------------------------------------------------------------------------

/// Messages sent to the SSH channel loop.
pub(crate) enum ChannelInput {
    Write(Vec<u8>),
    Resize { cols: u16, rows: u16 },
    Shutdown,
}

/// Run the SSH channel I/O loop. Reads from the SSH channel and sends data
/// back through `output_tx`. Receives user input from `input_rx`.
pub(crate) async fn channel_loop(
    mut channel: russh::Channel<russh::client::Msg>,
    mut input_rx: mpsc::UnboundedReceiver<ChannelInput>,
    output_tx: mpsc::UnboundedSender<Vec<u8>>,
) -> bool {
    let mut initiated_by_host = false;

    loop {
        tokio::select! {
            msg = channel.wait() => {
                match msg {
                    Some(ChannelMsg::Data { data }) => {
                        let _ = output_tx.send(data.to_vec());
                    }
                    Some(ChannelMsg::ExtendedData { data, .. }) => {
                        let _ = output_tx.send(data.to_vec());
                    }
                    Some(ChannelMsg::ExitStatus { exit_status }) => {
                        log::info!("SSH channel exited with status {exit_status}");
                        break;
                    }
                    Some(ChannelMsg::Eof) | Some(ChannelMsg::Close) | None => {
                        log::debug!("SSH channel closed/EOF");
                        break;
                    }
                    _ => {}
                }
            }

            input = input_rx.recv() => {
                match input {
                    Some(ChannelInput::Write(data)) => {
                        if let Err(e) = channel.data(&data[..]).await {
                            log::warn!("SSH write error: {e}");
                            break;
                        }
                    }
                    Some(ChannelInput::Resize { cols, rows }) => {
                        if let Err(e) = channel.window_change(cols as u32, rows as u32, 0, 0).await {
                            log::warn!("SSH resize error: {e}");
                        }
                    }
                    Some(ChannelInput::Shutdown) | None => {
                        initiated_by_host = true;
                        let _ = channel.eof().await;
                        let _ = channel.close().await;
                        break;
                    }
                }
            }
        }
    }

    // Returns true if the channel closed on its own (shell exit).
    !initiated_by_host
}

/// Execute a command on a separate SSH channel and return (stdout, stderr, exit_code).
pub(crate) async fn exec(
    ssh_handle: &client::Handle<SshHandler>,
    command: &str,
) -> Result<(String, String, u32), String> {
    let mut channel = ssh_handle
        .channel_open_session()
        .await
        .map_err(|e| format!("failed to open exec channel: {e}"))?;

    channel
        .exec(true, command)
        .await
        .map_err(|e| format!("exec failed: {e}"))?;

    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut exit_code = 0u32;

    loop {
        match channel.wait().await {
            Some(ChannelMsg::Data { data }) => {
                stdout.extend_from_slice(&data[..]);
            }
            Some(ChannelMsg::ExtendedData { data, ext }) => {
                if ext == 1 {
                    stderr.extend_from_slice(&data[..]);
                } else {
                    stdout.extend_from_slice(&data[..]);
                }
            }
            Some(ChannelMsg::ExitStatus { exit_status }) => {
                exit_code = exit_status;
            }
            Some(ChannelMsg::Eof) | Some(ChannelMsg::Close) | None => {
                break;
            }
            _ => {}
        }
    }

    Ok((
        String::from_utf8_lossy(&stdout).to_string(),
        String::from_utf8_lossy(&stderr).to_string(),
        exit_code,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expand_tilde_home() {
        let path = expand_tilde("~/foo/bar");
        assert!(path.to_str().unwrap().contains("foo/bar"));
        assert!(!path.to_str().unwrap().starts_with('~'));
    }

    #[test]
    fn expand_tilde_absolute_passthrough() {
        let path = expand_tilde("/usr/bin/ssh");
        assert_eq!(path, PathBuf::from("/usr/bin/ssh"));
    }

    #[test]
    fn expand_tilde_bare() {
        let path = expand_tilde("~");
        assert!(!path.to_str().unwrap().contains('~'));
    }
}

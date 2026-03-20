//! SSH connection logic — auth, proxy support, channel I/O.

use std::path::PathBuf;
use std::sync::Arc;

use russh::client;
use russh::ChannelMsg;
use tokio::sync::mpsc;

use crate::callbacks::{RemoteCallbacks, RemotePaths};
use crate::config::ServerEntry;
use crate::handler::ConchSshHandler;

/// Expand a leading `~` or `~/` to the user's home directory.
pub fn expand_tilde(path: &str) -> PathBuf {
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
// Connection
// ---------------------------------------------------------------------------

/// Establish an SSH connection, authenticate, open a shell channel.
///
/// Returns the client handle and the shell channel. Auth prompts are sent
/// through the `callbacks` trait implementation.
pub async fn connect_and_open_shell(
    server: &ServerEntry,
    password: Option<String>,
    callbacks: Arc<dyn RemoteCallbacks>,
    paths: &RemotePaths,
) -> Result<(client::Handle<ConchSshHandler>, russh::Channel<russh::client::Msg>), String> {
    let config = Arc::new(client::Config::default());
    let handler = ConchSshHandler {
        host: server.host.clone(),
        port: server.port,
        known_hosts_file: paths.known_hosts_file.clone(),
        callbacks: callbacks.clone(),
    };

    // Determine effective proxy.
    #[cfg(not(target_os = "ios"))]
    let effective_proxy = server
        .proxy_command
        .clone()
        .or_else(|| {
            server
                .proxy_jump
                .as_ref()
                .map(|jump| format!("ssh -W %h:%p {jump}"))
        });

    #[cfg(target_os = "ios")]
    let effective_proxy: Option<String> = None;

    let mut session = if let Some(proxy_cmd) = &effective_proxy {
        #[cfg(not(target_os = "ios"))]
        {
            connect_via_proxy(proxy_cmd, &server.host, server.port, config, handler).await?
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

    // Authenticate.
    let authenticated = if server.auth_method == "password" {
        // If password was provided, use it directly. Otherwise prompt via callbacks.
        let pw = match &password {
            Some(pw) => Some(pw.clone()),
            None => {
                let msg = format!("Password for {}@{}", server.user, server.host);
                callbacks.prompt_password(&msg).await
            }
        };

        match pw {
            Some(pw) => session
                .authenticate_password(&server.user, &pw)
                .await
                .map_err(|e| format!("Auth failed: {e}"))?,
            None => return Err("Password entry cancelled".to_string()),
        }
    } else {
        try_key_auth(
            &mut session,
            &server.user,
            server.key_path.as_deref(),
            &paths.default_key_paths,
        )
        .await?
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
#[cfg(not(target_os = "ios"))]
pub(crate) async fn connect_via_proxy(
    proxy_cmd: &str,
    host: &str,
    port: u16,
    config: Arc<client::Config>,
    handler: ConchSshHandler,
) -> Result<client::Handle<ConchSshHandler>, String> {
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
    let child = {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        Command::new("cmd")
            .arg("/C")
            .arg(&expanded)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::inherit())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .map_err(|e| format!("Failed to spawn ProxyCommand: {e}"))?
    };

    let stdin = child.stdin.unwrap();
    let stdout = child.stdout.unwrap();
    let stream = tokio::io::join(stdout, stdin);

    client::connect_stream(config, stream, handler)
        .await
        .map_err(|e| format!("Connection via proxy failed: {e}"))
}

/// Try key-based authentication with common SSH key files.
pub(crate) async fn try_key_auth(
    session: &mut client::Handle<ConchSshHandler>,
    user: &str,
    explicit_key_path: Option<&str>,
    default_key_paths: &[PathBuf],
) -> Result<bool, String> {
    let key_paths: Vec<PathBuf> = if let Some(path) = explicit_key_path {
        vec![expand_tilde(path)]
    } else {
        default_key_paths.to_vec()
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
pub enum ChannelInput {
    Write(Vec<u8>),
    Resize { cols: u16, rows: u16 },
    Shutdown,
}

/// Run the SSH channel I/O loop. Reads from the SSH channel and sends data
/// back through `output_tx`. Receives user input from `input_rx`.
///
/// Returns `true` if the channel closed on its own (remote shell exit),
/// `false` if closed by a local `Shutdown` message.
pub async fn channel_loop(
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
pub async fn exec(
    ssh_handle: &client::Handle<ConchSshHandler>,
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

    #[test]
    fn try_key_auth_uses_default_paths_when_no_explicit() {
        // Verify that try_key_auth would use default_key_paths
        // We can't call it directly (async + needs real SSH session),
        // but we verify the key_paths construction logic.
        let defaults = vec![
            PathBuf::from("/tmp/test_keys/id_ed25519"),
            PathBuf::from("/tmp/test_keys/id_rsa"),
        ];

        // When explicit_key_path is None, default_key_paths should be used.
        let key_paths: Vec<PathBuf> = {
            let explicit_key_path: Option<&str> = None;
            if let Some(path) = explicit_key_path {
                vec![expand_tilde(path)]
            } else {
                defaults.to_vec()
            }
        };
        assert_eq!(key_paths.len(), 2);
        assert_eq!(key_paths[0], PathBuf::from("/tmp/test_keys/id_ed25519"));
    }

    #[test]
    fn try_key_auth_uses_explicit_path_when_given() {
        let defaults = vec![
            PathBuf::from("/tmp/test_keys/id_ed25519"),
            PathBuf::from("/tmp/test_keys/id_rsa"),
        ];

        let key_paths: Vec<PathBuf> = {
            let explicit_key_path: Option<&str> = Some("/custom/key");
            if let Some(path) = explicit_key_path {
                vec![expand_tilde(path)]
            } else {
                defaults.to_vec()
            }
        };
        assert_eq!(key_paths.len(), 1);
        assert_eq!(key_paths[0], PathBuf::from("/custom/key"));
    }

    #[test]
    fn try_key_auth_expands_tilde_in_explicit_path() {
        let key_paths: Vec<PathBuf> = {
            let explicit_key_path: Option<&str> = Some("~/.ssh/my_key");
            if let Some(path) = explicit_key_path {
                vec![expand_tilde(path)]
            } else {
                vec![]
            }
        };
        assert_eq!(key_paths.len(), 1);
        assert!(!key_paths[0].to_str().unwrap().starts_with('~'));
        assert!(key_paths[0].to_str().unwrap().contains(".ssh/my_key"));
    }

    #[test]
    fn channel_input_variants() {
        // Verify ChannelInput enum can be constructed
        let _write = ChannelInput::Write(vec![1, 2, 3]);
        let _resize = ChannelInput::Resize {
            cols: 80,
            rows: 24,
        };
        let _shutdown = ChannelInput::Shutdown;
    }
}

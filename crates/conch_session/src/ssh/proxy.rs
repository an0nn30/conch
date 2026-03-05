use std::sync::Arc;

use anyhow::{Context, Result};
use russh::client;
use tokio::process::Command;

use super::client::{ClientHandler, ConnectParams};

/// Connect to an SSH server via a ProxyCommand.
///
/// The proxy command is executed as a shell subprocess, and its stdin/stdout
/// are used as the SSH transport (via `russh::client::connect_stream`).
pub async fn connect_via_proxy(
    proxy_cmd: &str,
    params: &ConnectParams,
) -> Result<client::Handle<ClientHandler>> {
    // Expand %h and %p placeholders
    let expanded = proxy_cmd
        .replace("%h", &params.host)
        .replace("%p", &params.port.to_string());

    // Spawn the proxy command
    let child = Command::new("sh")
        .arg("-c")
        .arg(&expanded)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::inherit())
        .spawn()
        .context("Failed to spawn ProxyCommand")?;

    let stdin = child.stdin.unwrap();
    let stdout = child.stdout.unwrap();

    // Combine stdin/stdout into a single async stream
    let stream = tokio::io::join(stdout, stdin);

    let config = Arc::new(client::Config::default());
    let handler = ClientHandler;

    let handle = client::connect_stream(config, stream, handler)
        .await
        .context("Failed to connect via ProxyCommand")?;

    Ok(handle)
}

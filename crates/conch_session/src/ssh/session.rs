use std::sync::Arc;

use alacritty_terminal::event::EventListener;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::{self, Term};
use alacritty_terminal::vte::ansi::Processor;
use anyhow::{Context, Result};
use russh::client::{self, Handle};
use russh::{Channel, ChannelMsg};
use tokio::sync::mpsc;

use crate::connector::EventProxy;
use crate::sftp::SftpFileProvider;
use super::client::{ConnectParams, ClientHandler, HostKeyTx, ShellConnectResult, SshConnection, connect_shell};

/// SSH connection metadata needed for rsync transport.
#[derive(Debug, Clone)]
pub struct SshConnectInfo {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub identity_file: Option<std::path::PathBuf>,
}

/// SSH terminal session — bridges an async SSH channel to alacritty_terminal's Term.
pub struct SshSession {
    /// The terminal state (same as LocalSession).
    pub term: Arc<FairMutex<Term<EventProxy>>>,
    /// Sender for input data to the SSH channel.
    input_tx: mpsc::UnboundedSender<SshInput>,
    /// Async receiver for terminal events.
    event_rx: Option<mpsc::UnboundedReceiver<alacritty_terminal::event::Event>>,
    /// SSH handle for opening additional channels (SFTP, tunnels).
    ssh_handle: Arc<Handle<ClientHandler>>,
    /// Connection metadata for rsync transport.
    connect_info: SshConnectInfo,
}

enum SshInput {
    Data(Vec<u8>),
    Resize { cols: u32, rows: u32 },
    Shutdown,
}

/// Simple Dimensions impl for creating a Term.
struct TermSize {
    columns: usize,
    lines: usize,
}

impl Dimensions for TermSize {
    fn total_lines(&self) -> usize { self.lines }
    fn screen_lines(&self) -> usize { self.lines }
    fn columns(&self) -> usize { self.columns }
}

/// Result of a password authentication attempt.
pub enum SshPasswordResult {
    /// Password accepted — session is ready.
    Connected(SshSession),
    /// Wrong password — pending auth returned for retry.
    WrongPassword(SshConnectResult),
}

/// Result of an SSH session connect attempt.
pub enum SshConnectResult {
    /// Fully connected session ready to use.
    Connected(SshSession),
    /// Server reached but needs password — holds pending auth + pre-built terminal state.
    NeedsPassword {
        pending_auth: super::client::PendingAuth,
        term: Arc<FairMutex<Term<EventProxy>>>,
        event_proxy: EventProxy,
        event_rx: mpsc::UnboundedReceiver<alacritty_terminal::event::Event>,
        term_config: term::Config,
        connect_info: SshConnectInfo,
    },
}

impl SshSession {
    /// Connect to an SSH server and set up the terminal bridge.
    pub async fn connect(
        params: &ConnectParams,
        cols: u16,
        rows: u16,
        term_config: term::Config,
        host_key_tx: Option<HostKeyTx>,
    ) -> Result<SshConnectResult> {
        let (event_proxy, event_rx) = EventProxy::new();

        // Create terminal state
        let term_size = TermSize {
            columns: cols as usize,
            lines: rows as usize,
        };
        let term = Term::new(term_config.clone(), &term_size, event_proxy.clone());
        let term = Arc::new(FairMutex::new(term));

        // Connect SSH
        let result = connect_shell(params, cols as u32, rows as u32, host_key_tx)
            .await
            .context("Failed to establish SSH shell session")?;

        let connect_info = SshConnectInfo {
            host: params.host.clone(),
            port: params.port,
            user: params.user.clone(),
            identity_file: params.identity_file.clone(),
        };

        match result {
            ShellConnectResult::Connected(ssh_conn) => {
                Ok(SshConnectResult::Connected(
                    Self::finish_setup(ssh_conn, term, event_proxy, event_rx, connect_info),
                ))
            }
            ShellConnectResult::NeedsPassword(pending_auth) => {
                Ok(SshConnectResult::NeedsPassword {
                    pending_auth,
                    term,
                    event_proxy,
                    event_rx,
                    term_config,
                    connect_info,
                })
            }
        }
    }

    /// Try password auth on a pending connection. On success, returns a connected session.
    /// On wrong password, returns the `SshConnectResult::NeedsPassword` back so the caller
    /// can prompt again. On connection error, returns `Err`.
    pub async fn try_password(
        mut pending_auth: super::client::PendingAuth,
        password: &str,
        term: Arc<FairMutex<Term<EventProxy>>>,
        event_proxy: EventProxy,
        event_rx: mpsc::UnboundedReceiver<alacritty_terminal::event::Event>,
        term_config: term::Config,
        connect_info: SshConnectInfo,
    ) -> Result<SshPasswordResult> {
        match pending_auth.try_password(password).await? {
            true => {
                let ssh_conn = pending_auth.open_shell().await?;
                Ok(SshPasswordResult::Connected(
                    Self::finish_setup(ssh_conn, term, event_proxy, event_rx, connect_info),
                ))
            }
            false => {
                Ok(SshPasswordResult::WrongPassword(SshConnectResult::NeedsPassword {
                    pending_auth,
                    term,
                    event_proxy,
                    event_rx,
                    term_config,
                    connect_info,
                }))
            }
        }
    }

    /// Wire up the SSH connection to the terminal bridge.
    fn finish_setup(
        ssh_conn: SshConnection,
        term: Arc<FairMutex<Term<EventProxy>>>,
        event_proxy: EventProxy,
        event_rx: mpsc::UnboundedReceiver<alacritty_terminal::event::Event>,
        connect_info: SshConnectInfo,
    ) -> Self {
        let ssh_handle = Arc::new(ssh_conn.handle);
        let (input_tx, input_rx) = mpsc::unbounded_channel();

        let term_clone = Arc::clone(&term);
        tokio::spawn(ssh_bridge_task(
            ssh_conn.channel,
            term_clone,
            event_proxy,
            input_rx,
        ));

        Self {
            term,
            input_tx,
            event_rx: Some(event_rx),
            ssh_handle,
            connect_info,
        }
    }

    /// Send raw bytes to the SSH channel (keyboard input).
    pub fn write(&self, data: &[u8]) {
        let _ = self.input_tx.send(SshInput::Data(data.to_vec()));
    }

    /// Resize the remote PTY.
    pub fn resize(&self, cols: u16, rows: u16, _cell_width: u16, _cell_height: u16) {
        let _ = self.input_tx.send(SshInput::Resize {
            cols: cols as u32,
            rows: rows as u32,
        });
    }

    /// Shut down the SSH session.
    pub fn shutdown(&self) {
        let _ = self.input_tx.send(SshInput::Shutdown);
    }

    /// Take the event receiver (can only be called once).
    pub fn take_event_rx(&mut self) -> mpsc::UnboundedReceiver<alacritty_terminal::event::Event> {
        self.event_rx.take().expect("event_rx already taken")
    }

    /// Get a reference to the underlying SSH handle (for spawning SFTP workers, etc.).
    pub fn ssh_handle(&self) -> &Arc<Handle<ClientHandler>> {
        &self.ssh_handle
    }

    /// Get the SSH connection metadata (for rsync transport).
    pub fn connect_info(&self) -> &SshConnectInfo {
        &self.connect_info
    }

    /// Open an SFTP session over this SSH connection.
    pub async fn open_sftp(&self) -> Result<SftpFileProvider> {
        let channel = self
            .ssh_handle
            .channel_open_session()
            .await
            .context("Failed to open SFTP channel")?;
        channel
            .request_subsystem(true, "sftp")
            .await
            .context("Failed to request SFTP subsystem")?;
        let sftp = russh_sftp::client::SftpSession::new(channel.into_stream())
            .await
            .context("Failed to initialize SFTP session")?;
        Ok(SftpFileProvider::new(sftp))
    }
}

/// Execute a command on a separate SSH channel (does not touch the terminal PTY).
/// Returns the combined stdout output. Takes a cloned handle so it can run independently.
pub async fn ssh_exec_command(handle: Arc<Handle<ClientHandler>>, command: String) -> anyhow::Result<String> {
    let channel = handle
        .channel_open_session()
        .await
        .context("Failed to open exec channel")?;
    channel
        .exec(true, command.as_bytes().to_vec())
        .await
        .context("Failed to exec command")?;

    let mut output = Vec::new();
    let mut stream = channel.into_stream();
    use tokio::io::AsyncReadExt;
    let mut buf = [0u8; 4096];
    loop {
        match stream.read(&mut buf).await {
            Ok(0) => break,
            Ok(n) => output.extend_from_slice(&buf[..n]),
            Err(_) => break,
        }
    }

    Ok(String::from_utf8_lossy(&output).into_owned())
}

impl Drop for SshSession {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Background task that bridges SSH channel I/O with the terminal state.
async fn ssh_bridge_task(
    channel: Channel<client::Msg>,
    term: Arc<FairMutex<Term<EventProxy>>>,
    event_proxy: EventProxy,
    mut input_rx: mpsc::UnboundedReceiver<SshInput>,
) {
    let mut processor: Processor = Processor::new();
    let (mut reader, writer) = channel.split();

    loop {
        tokio::select! {
            // Data from SSH server → parse into terminal
            msg = reader.wait() => {
                match msg {
                    Some(ChannelMsg::Data { data }) => {
                        let mut term_lock = term.lock();
                        processor.advance(&mut *term_lock, &data);
                        drop(term_lock);
                        event_proxy.send_event(alacritty_terminal::event::Event::Wakeup);
                    }
                    Some(ChannelMsg::ExtendedData { data, .. }) => {
                        // stderr — also display in terminal
                        let mut term_lock = term.lock();
                        processor.advance(&mut *term_lock, &data);
                        drop(term_lock);
                        event_proxy.send_event(alacritty_terminal::event::Event::Wakeup);
                    }
                    Some(ChannelMsg::ExitStatus { .. }) | Some(ChannelMsg::Eof) | None => {
                        event_proxy.send_event(alacritty_terminal::event::Event::Exit);
                        break;
                    }
                    _ => {} // Other channel messages (window adjust, etc.)
                }
            }

            // Input from user → send to SSH channel
            input = input_rx.recv() => {
                match input {
                    Some(SshInput::Data(data)) => {
                        if writer.data(&data[..]).await.is_err() {
                            break;
                        }
                    }
                    Some(SshInput::Resize { cols, rows }) => {
                        let _ = writer.window_change(cols, rows, 0, 0).await;
                        // Also resize the local Term
                        let mut term_lock = term.lock();
                        let size = TermSize {
                            columns: cols as usize,
                            lines: rows as usize,
                        };
                        term_lock.resize(size);
                        drop(term_lock);
                    }
                    Some(SshInput::Shutdown) | None => {
                        let _ = writer.eof().await;
                        let _ = writer.close().await;
                        break;
                    }
                }
            }
        }
    }
}

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
use super::client::{ConnectParams, ClientHandler, connect_shell};

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
    /// Receiver for CWD updates extracted from OSC 7 sequences in SSH output.
    cwd_rx: Option<std::sync::mpsc::Receiver<String>>,
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

impl SshSession {
    /// Connect to an SSH server and set up the terminal bridge.
    pub async fn connect(
        params: &ConnectParams,
        cols: u16,
        rows: u16,
    ) -> Result<Self> {
        let (event_proxy, event_rx) = EventProxy::new();

        // Create terminal state
        let term_config = term::Config::default();
        let term_size = TermSize {
            columns: cols as usize,
            lines: rows as usize,
        };
        let term = Term::new(term_config, &term_size, event_proxy.clone());
        let term = Arc::new(FairMutex::new(term));

        // Connect SSH
        let ssh_conn = connect_shell(params, cols as u32, rows as u32)
            .await
            .context("Failed to establish SSH shell session")?;

        let ssh_handle = Arc::new(ssh_conn.handle);

        // Create input channel
        let (input_tx, input_rx) = mpsc::unbounded_channel();

        // Create CWD tracking channel (OSC 7 extracted from raw SSH output)
        let (cwd_tx, cwd_rx) = std::sync::mpsc::channel();

        // Spawn the bridge task
        let term_clone = Arc::clone(&term);
        tokio::spawn(ssh_bridge_task(
            ssh_conn.channel,
            term_clone,
            event_proxy,
            input_rx,
            cwd_tx,
        ));

        Ok(Self {
            term,
            input_tx,
            event_rx: Some(event_rx),
            ssh_handle,
            cwd_rx: Some(cwd_rx),
        })
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

    /// Take the CWD receiver (can only be called once).
    pub fn take_cwd_rx(&mut self) -> Option<std::sync::mpsc::Receiver<String>> {
        self.cwd_rx.take()
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
    cwd_tx: std::sync::mpsc::Sender<String>,
) {
    let mut processor: Processor = Processor::new();
    let (mut reader, writer) = channel.split();

    loop {
        tokio::select! {
            // Data from SSH server → parse into terminal
            msg = reader.wait() => {
                match msg {
                    Some(ChannelMsg::Data { data }) => {
                        // Extract OSC 7 CWD updates and strip them from the
                        // byte stream before VTE processing (matches Java
                        // Conch's OscParser approach — prevents VTE side-effects
                        // from interfering with zsh's PROMPT_SP detection).
                        let (cwd, cleaned) = crate::shell_integration::extract_and_strip_osc7(&data);
                        if let Some(path) = cwd {
                            let _ = cwd_tx.send(path);
                        }
                        let mut term_lock = term.lock();
                        processor.advance(&mut *term_lock, &cleaned);
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

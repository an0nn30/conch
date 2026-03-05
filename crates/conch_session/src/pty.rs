use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Arc;
use std::thread::JoinHandle;

use alacritty_terminal::event::WindowSize;
use alacritty_terminal::event_loop::{EventLoop, EventLoopSender, Msg};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::{self, Term};
use alacritty_terminal::tty;
use anyhow::{Context, Result};
use tokio::sync::mpsc;

use crate::connector::EventProxy;

/// Simple Dimensions impl for creating a Term.
struct TermSize {
    columns: usize,
    lines: usize,
}

impl Dimensions for TermSize {
    fn total_lines(&self) -> usize {
        self.lines
    }
    fn screen_lines(&self) -> usize {
        self.lines
    }
    fn columns(&self) -> usize {
        self.columns
    }
}

/// A local terminal session backed by a PTY.
pub struct LocalSession {
    /// The terminal state (shared with the event loop thread).
    pub term: Arc<FairMutex<Term<EventProxy>>>,
    /// Channel to send input/resize/shutdown to the event loop.
    loop_tx: EventLoopSender,
    /// Async receiver for terminal events (Wakeup, Title, Exit, etc.).
    event_rx: Option<mpsc::UnboundedReceiver<alacritty_terminal::event::Event>>,
    /// Join handle for the event loop thread.
    _join_handle: JoinHandle<(EventLoop<tty::Pty, EventProxy>, alacritty_terminal::event_loop::State)>,
    /// Child process PID (for CWD polling via macOS proc_pidinfo).
    child_pid: u32,
}

impl LocalSession {
    /// Spawn a new local PTY session.
    pub fn new(cols: u16, rows: u16, cell_width: u16, cell_height: u16) -> Result<Self> {
        let window_size = WindowSize {
            num_lines: rows,
            num_cols: cols,
            cell_width,
            cell_height,
        };

        let (event_proxy, event_rx) = EventProxy::new();

        // Terminal state
        let term_config = term::Config::default();
        let term_size = TermSize {
            columns: cols as usize,
            lines: rows as usize,
        };
        let term = Term::new(term_config, &term_size, event_proxy.clone());
        let term = Arc::new(FairMutex::new(term));

        // PTY options — use default shell
        let mut env = HashMap::new();
        env.insert("TERM".into(), "xterm-256color".into());
        env.insert("COLORTERM".into(), "truecolor".into());
        let options = tty::Options {
            shell: None, // uses $SHELL
            working_directory: None,
            drain_on_exit: true,
            env,
        };

        // Create the PTY
        let pty = tty::new(&options, window_size, 0)
            .context("Failed to create PTY")?;

        // Extract child PID before EventLoop takes ownership of the Pty.
        let child_pid = pty.child().id();

        // Create the event loop
        let event_loop = EventLoop::new(
            Arc::clone(&term),
            event_proxy,
            pty,
            true,  // drain_on_exit
            false, // ref_test
        )?;

        // Get sender BEFORE spawning
        let loop_tx = event_loop.channel();
        let join_handle = event_loop.spawn();

        Ok(Self {
            term,
            loop_tx,
            event_rx: Some(event_rx),
            _join_handle: join_handle,
            child_pid,
        })
    }

    /// Take the event receiver (can only be called once).
    pub fn take_event_rx(&mut self) -> mpsc::UnboundedReceiver<alacritty_terminal::event::Event> {
        self.event_rx.take().expect("event_rx already taken")
    }

    /// Get the child process PID (for CWD polling).
    pub fn child_pid(&self) -> u32 {
        self.child_pid
    }

    /// Send raw bytes to the PTY (keyboard input).
    pub fn write(&self, data: &[u8]) {
        let _ = self.loop_tx.send(Msg::Input(Cow::Owned(data.to_vec())));
    }

    /// Resize the terminal.
    pub fn resize(&self, cols: u16, rows: u16, cell_width: u16, cell_height: u16) {
        // Resize the Term grid directly — Msg::Resize only resizes the PTY,
        // not the terminal grid. Without this, display_iter stays at the old size.
        let mut term = self.term.lock();
        term.resize(TermSize {
            columns: cols as usize,
            lines: rows as usize,
        });
        drop(term);

        // Resize the PTY (sends TIOCSWINSZ → SIGWINCH to the shell).
        let size = WindowSize {
            num_lines: rows,
            num_cols: cols,
            cell_width,
            cell_height,
        };
        let _ = self.loop_tx.send(Msg::Resize(size));
    }

    /// Shut down the event loop and PTY.
    pub fn shutdown(&self) {
        let _ = self.loop_tx.send(Msg::Shutdown);
    }
}

impl Drop for LocalSession {
    fn drop(&mut self) {
        self.shutdown();
    }
}

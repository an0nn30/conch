//! Control mode connection manager.

use std::io::{self, Read, Write};

use crate::parser::ControlModeParser;
use crate::protocol::Notification;
use portable_pty::{CommandBuilder as PtyCommandBuilder, PtySize, native_pty_system};

/// The write half of a control mode connection.
pub struct ConnectionWriter {
    writer: Box<dyn Write + Send>,
}

impl ConnectionWriter {
    pub fn send_command(&mut self, cmd: &str) -> io::Result<()> {
        self.writer.write_all(cmd.as_bytes())?;
        self.writer.flush()
    }
}

/// The read half of a control mode connection.
pub struct ConnectionReader {
    reader: Box<dyn Read + Send>,
    parser: ControlModeParser,
}

impl ConnectionReader {
    pub fn stdout(&mut self) -> &mut (dyn Read + Send) {
        self.reader.as_mut()
    }

    pub fn parse_bytes(&mut self, data: &[u8]) -> Vec<Notification> {
        self.parser.feed(data)
    }
}

/// A handle to the tmux child process. Drop this to kill tmux.
pub struct ConnectionHandle {
    child: Box<dyn portable_pty::Child + Send>,
}

impl ConnectionHandle {
    pub fn pid(&self) -> u32 {
        self.child.process_id().unwrap_or(0)
    }

    pub fn kill(mut self) -> io::Result<()> {
        self.child.kill()?;
        self.child.wait()?;
        Ok(())
    }
}

impl Drop for ConnectionHandle {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Spawn a tmux control mode process and split into reader, writer, and handle.
pub fn spawn(binary: &str, args: &[&str]) -> io::Result<(ConnectionReader, ConnectionWriter, ConnectionHandle)> {
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| io::Error::other(format!("failed to open tmux PTY: {e}")))?;

    let mut command = PtyCommandBuilder::new(binary);
    for arg in args {
        command.arg(arg);
    }

    // GUI-launched app processes may not have TERM/COLORTERM set.
    if std::env::var_os("TERM").is_none() {
        command.env("TERM", "xterm-256color");
    }
    if std::env::var_os("COLORTERM").is_none() {
        command.env("COLORTERM", "truecolor");
    }

    let child = pair
        .slave
        .spawn_command(command)
        .map_err(|e| io::Error::other(format!("failed to spawn tmux control mode: {e}")))?;
    drop(pair.slave);

    let reader = pair
        .master
        .try_clone_reader()
        .map_err(|e| io::Error::other(format!("failed to clone tmux reader: {e}")))?;
    let writer = pair
        .master
        .take_writer()
        .map_err(|e| io::Error::other(format!("failed to take tmux writer: {e}")))?;

    Ok((
        ConnectionReader {
            reader,
            parser: ControlModeParser::new(),
        },
        ConnectionWriter { writer },
        ConnectionHandle { child },
    ))
}

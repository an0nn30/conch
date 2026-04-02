//! Control mode connection manager.

use std::io::{self, BufWriter, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use crate::parser::ControlModeParser;
use crate::protocol::Notification;

/// The write half of a control mode connection.
pub struct ConnectionWriter {
    writer: BufWriter<ChildStdin>,
}

impl ConnectionWriter {
    pub fn send_command(&mut self, cmd: &str) -> io::Result<()> {
        self.writer.write_all(cmd.as_bytes())?;
        self.writer.flush()
    }
}

/// The read half of a control mode connection.
pub struct ConnectionReader {
    stdout: ChildStdout,
    parser: ControlModeParser,
}

impl ConnectionReader {
    pub fn stdout(&mut self) -> &mut ChildStdout {
        &mut self.stdout
    }

    pub fn parse_bytes(&mut self, data: &[u8]) -> Vec<Notification> {
        self.parser.feed(data)
    }
}

/// A handle to the tmux child process. Drop this to kill tmux.
pub struct ConnectionHandle {
    child: Child,
}

impl ConnectionHandle {
    pub fn pid(&self) -> u32 {
        self.child.id()
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
    let mut child = Command::new(binary)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;

    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| io::Error::new(io::ErrorKind::Other, "failed to open tmux stdin"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::new(io::ErrorKind::Other, "failed to open tmux stdout"))?;

    Ok((
        ConnectionReader {
            stdout,
            parser: ControlModeParser::new(),
        },
        ConnectionWriter {
            writer: BufWriter::new(stdin),
        },
        ConnectionHandle { child },
    ))
}

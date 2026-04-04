//! Raw PTY backend using `portable-pty`.
//!
//! Unlike `conch_pty` (which wraps alacritty_terminal for grid-level access),
//! this module provides raw byte-level PTY I/O — xterm.js handles all terminal
//! emulation on the frontend side.

use std::collections::HashMap;
use std::ffi::CStr;
use std::io::Write;

use anyhow::{Context, Result};
use parking_lot::Mutex;
use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};

pub(crate) struct PtyBackend {
    master: Box<dyn MasterPty + Send>,
    writer: Mutex<Box<dyn Write + Send>>,
}

impl PtyBackend {
    /// Spawn a new PTY with the given dimensions and shell/env overrides.
    pub fn new(
        cols: u16,
        rows: u16,
        shell: Option<&str>,
        shell_args: &[String],
        extra_env: &HashMap<String, String>,
        clear_tmux_env: bool,
    ) -> Result<Self> {
        let pty_system = native_pty_system();

        let size = PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        };

        let pair = pty_system
            .openpty(size)
            .context("Failed to open PTY pair")?;

        let actual_shell = match shell {
            Some(s) if !s.is_empty() => s.to_string(),
            _ => default_shell_program(),
        };

        let mut cmd = CommandBuilder::new(&actual_shell);
        for arg in shell_args {
            cmd.arg(arg);
        }

        // Match conch_pty behavior: defaults first, then user overrides.
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        if clear_tmux_env {
            cmd.env_remove("TMUX");
        }
        for (k, v) in extra_env {
            cmd.env(k, v);
        }

        pair.slave
            .spawn_command(cmd)
            .context("Failed to spawn shell in PTY")?;

        drop(pair.slave);

        let writer = pair
            .master
            .take_writer()
            .context("Failed to get PTY writer")?;

        Ok(Self {
            master: pair.master,
            writer: Mutex::new(writer),
        })
    }

    /// Write raw bytes to the PTY (user keyboard input).
    pub fn write(&self, data: &[u8]) -> Result<()> {
        let mut writer = self.writer.lock();
        writer.write_all(data).context("PTY write failed")?;
        writer.flush().context("PTY flush failed")?;
        Ok(())
    }

    /// Resize the PTY.
    pub fn resize(&self, cols: u16, rows: u16) -> Result<()> {
        self.master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("PTY resize failed")
    }

    /// Clone the reader for use in a separate thread.
    pub fn try_clone_reader(&self) -> Option<Box<dyn std::io::Read + Send>> {
        self.master.try_clone_reader().ok()
    }
}

#[cfg(unix)]
fn default_shell_program() -> String {
    // Use the account's configured login shell instead of the inherited
    // SHELL env var so "plain shell" tabs bypass wrapper commands like
    // `bash -c tmux new-session` from terminal config.
    let uid = unsafe { libc::getuid() };
    let pwd = unsafe { libc::getpwuid(uid) };
    if !pwd.is_null() {
        let shell_ptr = unsafe { (*pwd).pw_shell };
        if !shell_ptr.is_null() {
            let shell = unsafe { CStr::from_ptr(shell_ptr) }
                .to_string_lossy()
                .trim()
                .to_string();
            if !shell.is_empty() {
                return shell;
            }
        }
    }

    std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string())
}

#[cfg(not(unix))]
fn default_shell_program() -> String {
    "cmd.exe".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_shell_env_var_used() {
        let shell = default_shell_program();
        assert!(!shell.is_empty());
    }

    #[test]
    fn pty_size_struct_fields() {
        let size = PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        };
        assert_eq!(size.rows, 24);
        assert_eq!(size.cols, 80);
    }
}

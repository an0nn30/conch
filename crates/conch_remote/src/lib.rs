//! Platform-agnostic SSH, SFTP, and tunnel operations for Conch.
//!
//! This crate provides the core remote connectivity logic shared by
//! the desktop app (`conch_tauri`) and mobile app (`conch_mobile`).

pub mod callbacks;
pub mod config;
pub mod error;
pub mod handler;
pub mod known_hosts;
pub mod sftp;
pub mod ssh;
pub mod transfer;
pub mod tunnel;

pub use error::RemoteError;

/// Default SSH port.
pub const SSH_DEFAULT_PORT: u16 = 22;
/// Default PTY width in columns.
pub const DEFAULT_PTY_COLS: u16 = 80;
/// Default PTY height in rows.
pub const DEFAULT_PTY_ROWS: u16 = 24;

// Re-export russh types used by app crates (Handle, Channel, ChannelMsg).
// App crates reference these when storing session handles and running channel loops.
pub use russh;

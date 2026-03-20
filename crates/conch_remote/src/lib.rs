//! Platform-agnostic SSH, SFTP, and tunnel operations for Conch.
//!
//! This crate provides the core remote connectivity logic shared by
//! the desktop app (`conch_tauri`) and mobile app (`conch_mobile`).

pub mod callbacks;
pub mod config;
pub mod handler;
pub mod known_hosts;

// Re-export russh types used by app crates (Handle, Channel, ChannelMsg).
// App crates reference these when storing session handles and running channel loops.
pub use russh;

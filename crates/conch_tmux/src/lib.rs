//! Tmux control mode protocol library.
//!
//! Provides a parser for `tmux -CC` output, a command builder for sending
//! tmux commands, and an in-memory session model.

pub mod command;
pub mod connection;
pub mod parser;
pub mod protocol;
pub mod session;

pub use command::CommandBuilder;
pub use connection::{spawn, ConnectionHandle, ConnectionReader, ConnectionWriter};
pub use parser::ControlModeParser;
pub use protocol::Notification;
pub use session::{SessionList, TmuxSession};

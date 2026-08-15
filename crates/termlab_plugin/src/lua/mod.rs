//! Lua plugin runtime — embeds a Lua 5.4 VM and maps plugin API calls
//! to the host's Widget types and HostApi vtable.
//!
//! - [`metadata`] — parse `-- plugin-*` headers from `.lua` files.
//! - [`api`] — register `ui`, `session`, `app`, `net` Lua tables.
//! - [`runner`] — lifecycle management (setup → event loop → teardown).

pub mod api;
pub mod metadata;
pub mod runner;

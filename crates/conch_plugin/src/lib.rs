//! Host-side plugin infrastructure for Conch.
//!
//! This crate provides:
//!
//! - **Message bus** (`bus`) — event broadcast (pub/sub), direct query routing
//!   (request/response), and a service registry for inter-plugin communication.
//! - **Lua plugin runtime** (`lua`) — discovers, loads, and manages Lua plugins.
//! - **Java plugin runtime** (`jvm`) — discovers, loads, and manages Java plugins.

pub mod bus;
pub mod host_api;
#[cfg(feature = "java")]
pub mod jvm;
#[cfg(not(feature = "java"))]
pub mod jvm_stub;
#[cfg(not(feature = "java"))]
pub use jvm_stub as jvm;
pub mod lua;

pub use host_api::HostApi;

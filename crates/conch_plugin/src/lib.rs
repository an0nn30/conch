//! Host-side plugin infrastructure for Conch.
//!
//! This crate provides:
//!
//! - **Message bus** (`bus`) — event broadcast (pub/sub), direct query routing
//!   (request/response), and a service registry for inter-plugin communication.
//!
//! - **Native plugin loader** (`native`) — discovers, loads, and manages native
//!   plugins (shared libraries) via `dlopen`/`libloading`. Each plugin runs on
//!   its own OS thread with a bounded thread pool.

pub mod bus;
pub mod host_api;
#[cfg(java_sdk_available)]
pub mod jvm;
#[cfg(not(java_sdk_available))]
pub mod jvm_stub;
#[cfg(not(java_sdk_available))]
pub use jvm_stub as jvm;
pub mod lua;
pub mod native;

pub use host_api::{CHostApiAdapter, HostApi};

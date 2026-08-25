// Task 2 establishes protocol and document foundations that the manager,
// session, diagnostics, and command modules consume in later tasks.
#![allow(dead_code)]

pub(crate) mod catalog;
pub(crate) mod client;
pub(crate) mod document;
pub(crate) mod ownership;
pub(crate) mod root;
pub(crate) mod session;
#[cfg(test)]
pub(crate) mod test_support;
pub(crate) mod trust;
pub(crate) mod types;

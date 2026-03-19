//! Session metadata types.

/// Opaque handle to a session created by the host.
#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SessionHandle(pub u64);

/// Opaque handle to a registered panel.
#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PanelHandle(pub u64);

/// Status of a plugin-owned session.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionStatus {
    Connecting = 0,
    Connected = 1,
    Error = 2,
}

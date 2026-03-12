//! SSH session backend — byte-stream bridge between SSH channel and the host's
//! terminal emulator.
//!
//! This exercises: SessionBackendVtable, OutputCallback.
//!
//! In the real plugin, this wraps a `russh` SSH channel. For the stub, it's
//! just metadata.

use std::ffi::c_void;

use conch_plugin_sdk::SessionBackendVtable;

/// A single SSH session backend (one per connected tab).
pub struct SshSessionBackend {
    host: String,
    user: String,
    // Real plugin would hold:
    // channel: russh::ChannelHandle,
    // output_cb: OutputCallback,
    // output_ctx: *mut c_void,
}

impl SshSessionBackend {
    pub fn new_stub(server: &crate::config::ServerEntry) -> Self {
        Self {
            host: server.host.clone(),
            user: server.user.clone(),
        }
    }

    pub fn host(&self) -> &str {
        &self.host
    }

    pub fn user(&self) -> &str {
        &self.user
    }

    /// Build the vtable the host uses to send input/resize/shutdown.
    pub fn vtable(&self) -> SessionBackendVtable {
        SessionBackendVtable {
            write: ssh_backend_write,
            resize: ssh_backend_resize,
            shutdown: ssh_backend_shutdown,
            drop: ssh_backend_drop,
        }
    }

    /// Get an opaque handle to this backend for the vtable callbacks.
    pub fn as_handle(&self) -> *mut c_void {
        // Real plugin: Box::into_raw of a per-session state struct.
        // Stub: just a null pointer.
        std::ptr::null_mut()
    }
}

// -- Vtable implementations (stubs) --

extern "C" fn ssh_backend_write(_handle: *mut c_void, _buf: *const u8, _len: usize) {
    // Real: write bytes to the SSH channel.
    // channel.write_all(slice::from_raw_parts(buf, len))
}

extern "C" fn ssh_backend_resize(_handle: *mut c_void, _cols: u16, _rows: u16) {
    // Real: send window-change request to SSH server.
    // channel.request_pty_size(cols, rows)
}

extern "C" fn ssh_backend_shutdown(_handle: *mut c_void) {
    // Real: send EOF + close the SSH channel gracefully.
    // channel.close()
}

extern "C" fn ssh_backend_drop(_handle: *mut c_void) {
    // Real: free the boxed per-session state.
    // Box::from_raw(handle as *mut SessionState)
}

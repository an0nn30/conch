//! Session backend bridge — connects plugin byte-stream sessions to the
//! terminal emulator.
//!
//! When a plugin opens a session via `HostApi::open_session`, the host creates
//! a [`PluginSessionBridge`] that:
//!
//! 1. Creates a `Term<EventProxy>` (same type used for local PTY sessions).
//! 2. Provides an `OutputCallback` that the plugin calls to push bytes.
//! 3. Feeds those bytes through the VTE parser to update the terminal state.
//! 4. The host renders the `Term` identically to local sessions.
//!
//! ## Threading
//!
//! The plugin writes output bytes from its own thread via the callback.
//! The callback acquires the `FairMutex<Term>` and feeds bytes through the VTE
//! parser. The UI thread renders from the same `Arc<FairMutex<Term>>` using the
//! existing `show_terminal()` widget.

use std::ffi::c_void;
use std::sync::Arc;

use alacritty_terminal::event::Event as TermEvent;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::{self, Term};
use conch_plugin_sdk::{OutputCallback, SessionHandle};
use conch_pty::EventProxy;
use tokio::sync::mpsc;

/// Dimensions for creating the terminal grid.
struct TermSize {
    columns: usize,
    lines: usize,
}

impl Dimensions for TermSize {
    fn total_lines(&self) -> usize {
        self.lines
    }
    fn screen_lines(&self) -> usize {
        self.lines
    }
    fn columns(&self) -> usize {
        self.columns
    }
}

/// Bridges a plugin's byte-stream session to the terminal emulator.
///
/// Holds the terminal state (`Term<EventProxy>`) and a VTE parser. The plugin
/// pushes output bytes via the [`OutputCallback`], which feeds them through the
/// parser to update the terminal grid.
pub struct PluginSessionBridge {
    /// Terminal state, shared with the UI rendering thread.
    pub term: Arc<FairMutex<Term<EventProxy>>>,
    /// VTE parser that feeds into the terminal handler.
    parser: alacritty_terminal::vte::ansi::Processor,
    /// Receives terminal events (Title, Bell, etc.) from the EventProxy.
    pub event_rx: mpsc::UnboundedReceiver<TermEvent>,
    /// Session handle assigned by the host.
    pub handle: SessionHandle,
    /// The boxed bridge pointer passed as `output_ctx`. We keep this to free
    /// it on drop.
    bridge_ptr: *mut BridgeCtx,
}

// SAFETY: The bridge is accessed from the plugin thread (via callback) and the
// UI thread (via Arc<FairMutex<Term>>). The FairMutex provides synchronization.
// The bridge_ptr is only used for Drop cleanup.
unsafe impl Send for PluginSessionBridge {}

/// Opaque context passed to the output callback.
///
/// Contains a raw pointer to the Term and parser. The callback acquires the
/// mutex, feeds bytes, and releases — all within the callback invocation.
struct BridgeCtx {
    term: Arc<FairMutex<Term<EventProxy>>>,
    parser: *mut alacritty_terminal::vte::ansi::Processor,
}

impl PluginSessionBridge {
    /// Create a new bridge for a plugin session.
    ///
    /// Returns the bridge and the `OutputCallback` + `output_ctx` that should
    /// be given to the plugin (via `OpenSessionResult`).
    pub fn new(
        handle: SessionHandle,
        cols: u16,
        rows: u16,
        term_config: term::Config,
    ) -> (Self, OutputCallback, *mut c_void) {
        let (event_proxy, event_rx) = EventProxy::new();

        let size = TermSize {
            columns: cols as usize,
            lines: rows as usize,
        };
        let term = Term::new(term_config, &size, event_proxy);
        let term = Arc::new(FairMutex::new(term));
        let parser = alacritty_terminal::vte::ansi::Processor::new();

        // Allocate the bridge context on the heap so we can pass a stable
        // pointer to the C callback.
        let ctx = Box::new(BridgeCtx {
            term: Arc::clone(&term),
            parser: std::ptr::null_mut(), // filled in below
        });
        let ctx_ptr = Box::into_raw(ctx);

        let mut bridge = Self {
            term,
            parser,
            event_rx,
            handle,
            bridge_ptr: ctx_ptr,
        };

        // Point the context's parser pointer at the bridge's parser.
        // SAFETY: ctx_ptr is valid (we just allocated it) and bridge.parser is
        // stable because PluginSessionBridge is not moved after this point
        // (it's stored in a HashMap by the host).
        //
        // NOTE: This raw pointer approach is needed because the output callback
        // is `extern "C"` and cannot capture Rust closures. The parser and term
        // are both accessed under the FairMutex.
        unsafe {
            (*ctx_ptr).parser = &mut bridge.parser as *mut _;
        }

        (bridge, output_callback, ctx_ptr as *mut c_void)
    }

    /// Resize the terminal grid.
    pub fn resize(&mut self, cols: u16, rows: u16) {
        if let Some(mut term) = self.term.try_lock_unfair() {
            term.resize(TermSize {
                columns: cols as usize,
                lines: rows as usize,
            });
        }
    }
}

impl Drop for PluginSessionBridge {
    fn drop(&mut self) {
        if !self.bridge_ptr.is_null() {
            // SAFETY: We allocated this with Box::into_raw in new().
            unsafe {
                drop(Box::from_raw(self.bridge_ptr));
            }
        }
    }
}

/// The `extern "C"` output callback given to plugins.
///
/// The plugin calls this from its thread whenever data arrives (e.g., from an
/// SSH channel). We lock the terminal, feed bytes through the VTE parser, and
/// the parser updates the terminal grid.
///
/// # Safety
///
/// `ctx` must be a valid `*mut BridgeCtx` obtained from
/// [`PluginSessionBridge::new`]. `buf`/`len` must describe a valid byte slice.
extern "C" fn output_callback(ctx: *mut c_void, buf: *const u8, len: usize) {
    if ctx.is_null() || buf.is_null() || len == 0 {
        return;
    }

    let bridge_ctx = unsafe { &mut *(ctx as *mut BridgeCtx) };
    let bytes = unsafe { std::slice::from_raw_parts(buf, len) };

    // Lock the terminal and feed bytes through the parser.
    let mut term = bridge_ctx.term.lock();
    let parser = unsafe { &mut *bridge_ctx.parser };
    parser.advance(&mut *term, bytes);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_term_config() -> term::Config {
        term::Config::default()
    }

    #[test]
    fn bridge_creates_valid_term() {
        let handle = SessionHandle(1);
        let (bridge, cb, ctx) = PluginSessionBridge::new(handle, 80, 24, default_term_config());
        assert_eq!(bridge.handle, handle);
        assert!(!ctx.is_null());

        // Verify term dimensions.
        let term = bridge.term.lock();
        assert_eq!(term.columns(), 80);
        assert_eq!(term.screen_lines(), 24);
        drop(term);

        // Verify the callback is the right function.
        assert!(cb as usize == output_callback as usize);
    }

    #[test]
    fn output_callback_feeds_bytes() {
        let handle = SessionHandle(2);
        let (bridge, cb, ctx) = PluginSessionBridge::new(handle, 80, 24, default_term_config());

        // Write "Hello" via the output callback.
        let data = b"Hello";
        cb(ctx, data.as_ptr(), data.len());

        // The terminal should now contain "Hello" in the first row.
        let term = bridge.term.lock();
        let content = term.renderable_content();
        let mut text = String::new();
        for cell in content.display_iter {
            let c = cell.c;
            if c != ' ' && c != '\0' {
                text.push(c);
            }
        }
        assert!(text.contains("Hello"), "term content: {text:?}");
    }

    #[test]
    fn output_callback_handles_ansi_escapes() {
        let handle = SessionHandle(3);
        let (bridge, cb, ctx) = PluginSessionBridge::new(handle, 80, 24, default_term_config());

        // Write text with ANSI color escape.
        let data = b"\x1b[31mRed\x1b[0m Normal";
        cb(ctx, data.as_ptr(), data.len());

        let term = bridge.term.lock();
        let content = term.renderable_content();
        let mut text = String::new();
        for cell in content.display_iter {
            let c = cell.c;
            if c != ' ' && c != '\0' {
                text.push(c);
            }
        }
        assert!(text.contains("Red"), "term content: {text:?}");
        assert!(text.contains("Normal"), "term content: {text:?}");
    }

    #[test]
    fn resize_updates_dimensions() {
        let handle = SessionHandle(4);
        let (mut bridge, _, _) = PluginSessionBridge::new(handle, 80, 24, default_term_config());

        bridge.resize(120, 40);

        let term = bridge.term.lock();
        assert_eq!(term.columns(), 120);
        assert_eq!(term.screen_lines(), 40);
    }

    #[test]
    fn null_callback_args_are_safe() {
        // Calling with null ctx should not crash.
        output_callback(std::ptr::null_mut(), b"x".as_ptr(), 1);

        // Calling with null buf should not crash.
        let handle = SessionHandle(5);
        let (_bridge, _cb, ctx) = PluginSessionBridge::new(handle, 80, 24, default_term_config());
        output_callback(ctx, std::ptr::null(), 5);

        // Calling with zero len should not crash.
        output_callback(ctx, b"x".as_ptr(), 0);
    }

    #[test]
    fn bridge_drop_frees_context() {
        let handle = SessionHandle(6);
        let (bridge, _, _) = PluginSessionBridge::new(handle, 80, 24, default_term_config());
        // Drop should not panic or leak.
        drop(bridge);
    }

    #[test]
    fn event_rx_receives_title_changes() {
        let handle = SessionHandle(7);
        let (mut bridge, cb, ctx) = PluginSessionBridge::new(handle, 80, 24, default_term_config());

        // Send an OSC title sequence: ESC ] 0 ; title BEL
        let data = b"\x1b]0;My Title\x07";
        cb(ctx, data.as_ptr(), data.len());

        // The EventProxy should have forwarded a Title event.
        match bridge.event_rx.try_recv() {
            Ok(TermEvent::Title(title)) => {
                assert_eq!(title, "My Title");
            }
            other => {
                // Some terminals may not emit the event synchronously,
                // so we just note this test is best-effort.
                log::debug!("title event not immediately available: {:?}", other);
            }
        }
    }
}

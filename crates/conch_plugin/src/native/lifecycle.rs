//! Plugin thread lifecycle — setup, event loop, teardown.
//!
//! Each native plugin runs on a dedicated OS thread. The thread:
//! 1. Calls `conch_plugin_setup(host_api)` to initialize the plugin state.
//! 2. Enters a loop draining the plugin's mailbox (`mpsc::Receiver<PluginMail>`).
//! 3. On `Shutdown`, calls `conch_plugin_teardown(state)` and exits.

use std::ffi::{CStr, CString};
use std::thread::JoinHandle;

use tokio::sync::mpsc;

use crate::bus::{PluginMail, QueryResponse};

use super::library::PluginLibrary;
use super::PluginMeta;

/// A loaded and running plugin with its communication handle.
pub struct LoadedPlugin {
    pub meta: PluginMeta,
    /// Send commands to the plugin thread.
    pub sender: mpsc::Sender<PluginMail>,
    /// Join handle for the plugin thread. `None` after shutdown + join.
    pub thread_handle: Option<JoinHandle<()>>,
}

impl LoadedPlugin {
    /// Block until the plugin thread exits.
    pub fn join(&mut self) {
        if let Some(handle) = self.thread_handle.take() {
            if let Err(e) = handle.join() {
                log::error!("plugin thread for {:?} panicked: {:?}", self.meta.name, e);
            }
        }
    }
}

/// Entry point for the plugin's dedicated OS thread.
///
/// # Safety
///
/// `host_api` must point to a valid, long-lived `HostApi` struct.
/// `library` must have been successfully loaded with all symbols resolved.
pub(crate) unsafe fn plugin_thread(
    library: PluginLibrary,
    host_api: *const conch_plugin_sdk::HostApi,
    mut mailbox: mpsc::Receiver<PluginMail>,
    plugin_name: String,
) {
    // -- Setup ---------------------------------------------------------------
    log::info!("plugin [{plugin_name}]: calling setup");
    let state = unsafe { (library.setup_fn)(host_api) };
    if state.is_null() {
        log::error!("plugin [{plugin_name}]: setup returned null state, aborting");
        return;
    }

    // -- Event loop ----------------------------------------------------------
    log::debug!("plugin [{plugin_name}]: entering event loop");
    while let Some(mail) = mailbox.blocking_recv() {
        match mail {
            PluginMail::BusEvent(msg) => {
                handle_bus_event(&library, state, &plugin_name, &msg);
            }
            PluginMail::BusQuery(req) => {
                handle_bus_query(&library, state, &plugin_name, req);
            }
            PluginMail::RenderRequest { reply } => {
                let json = handle_render(&library, state, &plugin_name);
                let _ = reply.send(json);
            }
            PluginMail::WidgetEvent { json } => {
                // Forward raw JSON directly to the plugin's event handler.
                unsafe {
                    (library.event_fn)(state, json.as_ptr() as *const _, json.len());
                }
            }
            PluginMail::Shutdown => {
                log::info!("plugin [{plugin_name}]: shutting down");
                break;
            }
        }
    }

    // -- Teardown ------------------------------------------------------------
    log::info!("plugin [{plugin_name}]: calling teardown");
    unsafe { (library.teardown_fn)(state) };
}

/// Serialize a `BusMessage` into a `PluginEvent::BusEvent` JSON and call
/// `conch_plugin_event()`.
fn handle_bus_event(
    library: &PluginLibrary,
    state: *mut std::ffi::c_void,
    plugin_name: &str,
    msg: &crate::bus::BusMessage,
) {
    let event = serde_json::json!({
        "BusEvent": {
            "event_type": msg.event_type,
            "data": serde_json::to_string(&msg.data).unwrap_or_default(),
        }
    });
    let json = serde_json::to_string(&event).unwrap_or_default();

    unsafe {
        (library.event_fn)(state, json.as_ptr() as *const _, json.len());
    }
    log::trace!("plugin [{plugin_name}]: delivered event {}", msg.event_type);
}

/// Forward a query to the plugin's `conch_plugin_query()` and send the
/// response back through the oneshot channel.
fn handle_bus_query(
    library: &PluginLibrary,
    state: *mut std::ffi::c_void,
    plugin_name: &str,
    req: crate::bus::QueryRequest,
) {
    let method = match CString::new(req.method.clone()) {
        Ok(c) => c,
        Err(_) => {
            let _ = req.reply.send(QueryResponse {
                result: Err("invalid method name".into()),
            });
            return;
        }
    };

    let args_json = serde_json::to_string(&req.args).unwrap_or_else(|_| "null".into());

    let result_ptr = unsafe {
        (library.query_fn)(
            state,
            method.as_ptr(),
            args_json.as_ptr() as *const _,
            args_json.len(),
        )
    };

    let result = if result_ptr.is_null() {
        Err(format!("plugin [{plugin_name}]: query '{}' returned null", req.method))
    } else {
        let c_str = unsafe { CStr::from_ptr(result_ptr) };
        let s = c_str.to_string_lossy().into_owned();
        // Free the plugin-allocated string. Since both host and plugin use
        // Rust's global allocator, CString::from_raw is safe here.
        unsafe { drop(CString::from_raw(result_ptr)); }
        serde_json::from_str(&s)
            .map_err(|e| format!("invalid JSON from plugin [{plugin_name}]: {e}"))
    };

    let _ = req.reply.send(QueryResponse { result });
    log::trace!("plugin [{plugin_name}]: handled query '{}'", req.method);
}

/// Call `conch_plugin_render()` and return the JSON string.
fn handle_render(
    library: &PluginLibrary,
    state: *mut std::ffi::c_void,
    plugin_name: &str,
) -> String {
    let ptr = unsafe { (library.render_fn)(state) };
    if ptr.is_null() {
        log::warn!("plugin [{plugin_name}]: render returned null");
        return "[]".to_string();
    }

    // The pointer is plugin-owned (thread-local buffer) — read but do NOT free.
    let c_str = unsafe { CStr::from_ptr(ptr) };
    c_str.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loaded_plugin_meta_accessible() {
        let meta = PluginMeta {
            name: "test".into(),
            description: "desc".into(),
            version: "0.1.0".into(),
            plugin_type: conch_plugin_sdk::PluginType::Action,
            panel_location: conch_plugin_sdk::PanelLocation::None,
            dependencies: vec![],
        };
        let (tx, _rx) = mpsc::channel(16);
        let plugin = LoadedPlugin {
            meta,
            sender: tx,
            thread_handle: None,
        };
        assert_eq!(plugin.meta.name, "test");
        assert_eq!(plugin.meta.version, "0.1.0");
    }
}

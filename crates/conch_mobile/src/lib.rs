//! Conch Mobile — iOS SSH client.

#[cfg(target_os = "ios")]
mod ios_native;

mod callbacks;
mod commands;
mod state;

use std::sync::Arc;
use parking_lot::Mutex;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let mobile_state = Arc::new(Mutex::new(state::MobileState::new()));

    tauri::Builder::default()
        .manage(mobile_state)
        .invoke_handler(tauri::generate_handler![
            commands::ssh_quick_connect,
            commands::ssh_write,
            commands::ssh_resize,
            commands::ssh_disconnect,
            commands::auth_respond_host_key,
            commands::auth_respond_password,
            commands::get_sessions,
        ])
        .setup(|app| {
            #[cfg(target_os = "ios")]
            {
                use tauri::Manager;
                if let Some(webview) = app.get_webview_window("main") {
                    ios_native::setup_native_tab_bar(&webview);
                }
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running Conch Mobile");
}

#[cfg(test)]
mod tests {
    #[test]
    fn app_module_loads() {
        assert!(true);
    }
}

//! Conch Mobile — iOS SSH client.

#[cfg(target_os = "ios")]
mod ios_native;

mod state;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
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
        // Smoke test — verifies the crate compiles and links.
        assert!(true);
    }
}

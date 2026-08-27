//! CLI/IPC "open this path" routing: each request opens a new app window and
//! parks the path under that window's label until the window's frontend
//! pulls it with `take_pending_open_paths` — the same request-pull boot
//! pattern the panel host uses, so a slow-booting window cannot miss an
//! event that fired before it subscribed.

use std::collections::HashMap;
use std::sync::Mutex;

#[derive(Default)]
pub(crate) struct PendingOpens(Mutex<HashMap<String, Vec<String>>>);

impl PendingOpens {
    pub(crate) fn take(&self, label: &str) -> Vec<String> {
        self.0.lock().unwrap().remove(label).unwrap_or_default()
    }
}

pub(crate) fn seed_for_label(pending: &PendingOpens, label: &str, mut paths: Vec<String>) {
    pending
        .0
        .lock()
        .unwrap()
        .entry(label.to_string())
        .or_default()
        .append(&mut paths);
}

pub(crate) fn open_in_new_window<R: tauri::Runtime>(app: &tauri::AppHandle<R>, path: &str) {
    use tauri::Manager;
    match crate::windows::create_new_window(app) {
        Ok(label) => {
            let pending = app.state::<PendingOpens>();
            seed_for_label(&pending, &label, vec![path.to_string()]);
        }
        Err(e) => log::error!("open-path: could not create window for {path}: {e}"),
    }
}

#[tauri::command]
pub(crate) fn take_pending_open_paths(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, PendingOpens>,
) -> Vec<String> {
    state.take(window.label())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn take_returns_and_clears_per_label() {
        let pending = PendingOpens::default();
        seed_for_label(&pending, "main", vec!["/a.txt".into(), "/b.txt".into()]);
        seed_for_label(&pending, "window-1", vec!["/c.txt".into()]);
        assert_eq!(
            pending.take("main"),
            vec!["/a.txt".to_string(), "/b.txt".to_string()]
        );
        assert!(pending.take("main").is_empty(), "take drains");
        assert_eq!(pending.take("window-1"), vec!["/c.txt".to_string()]);
    }

    #[test]
    fn take_unknown_label_is_empty() {
        let pending = PendingOpens::default();
        assert!(pending.take("window-9").is_empty());
    }

    #[test]
    fn seed_appends_rather_than_replaces() {
        let pending = PendingOpens::default();
        seed_for_label(&pending, "main", vec!["/a.txt".into()]);
        seed_for_label(&pending, "main", vec!["/b.txt".into()]);
        assert_eq!(pending.take("main").len(), 2);
    }
}

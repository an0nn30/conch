//! Reader thread that drives the control mode parser and emits Tauri events.

use std::io::Read;
use std::sync::{Arc, RwLock};

use conch_tmux::{ConnectionReader, Notification, SessionList};
use tauri::{AppHandle, Emitter};

use super::events::*;

/// Spawn a reader loop for a control mode connection.
pub(crate) fn spawn_reader_thread(
    app: AppHandle,
    window_label: String,
    mut reader: ConnectionReader,
    sessions: Arc<RwLock<SessionList>>,
) -> std::thread::JoinHandle<()> {
    std::thread::Builder::new()
        .name(format!("tmux-reader-{window_label}"))
        .spawn(move || {
            let mut buf = [0u8; 8192];
            loop {
                match reader.stdout().read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        for notif in reader.parse_bytes(&buf[..n]) {
                            if let Ok(mut list) = sessions.write() {
                                list.apply_notification(&notif);
                            }
                            emit_notification(&app, &window_label, &notif, &sessions);
                        }
                    }
                    Err(e) => {
                        log::error!("tmux reader error for {window_label}: {e}");
                        break;
                    }
                }
            }
            let _ = app.emit_to(
                &window_label,
                "tmux-disconnected",
                TmuxDisconnectedEvent { reason: None },
            );
        })
        .expect("failed to spawn tmux reader thread")
}

fn emit_notification(
    app: &AppHandle,
    window_label: &str,
    notif: &Notification,
    sessions: &Arc<RwLock<SessionList>>,
) {
    match notif {
        Notification::Output { pane_id, data } => {
            let _ = app.emit_to(
                window_label,
                "tmux-output",
                TmuxOutputEvent {
                    pane_id: *pane_id,
                    data: String::from_utf8_lossy(data).into_owned(),
                },
            );
        }
        Notification::WindowAdd { window_id } => {
            let _ = app.emit_to(
                window_label,
                "tmux-window-add",
                TmuxWindowEvent {
                    window_id: *window_id,
                    name: None,
                },
            );
        }
        Notification::WindowClose { window_id } => {
            let _ = app.emit_to(
                window_label,
                "tmux-window-close",
                TmuxWindowEvent {
                    window_id: *window_id,
                    name: None,
                },
            );
        }
        Notification::WindowRenamed { window_id, name } => {
            let _ = app.emit_to(
                window_label,
                "tmux-window-renamed",
                TmuxWindowEvent {
                    window_id: *window_id,
                    name: Some(name.clone()),
                },
            );
        }
        Notification::LayoutChange { window_id, layout } => {
            let _ = app.emit_to(
                window_label,
                "tmux-layout-change",
                TmuxLayoutEvent {
                    window_id: *window_id,
                    layout: layout.clone(),
                },
            );
        }
        Notification::WindowPaneChanged { window_id, pane_id } => {
            let _ = app.emit_to(
                window_label,
                "tmux-pane-changed",
                TmuxPaneEvent {
                    window_id: Some(*window_id),
                    pane_id: *pane_id,
                },
            );
        }
        Notification::PaneModeChanged { pane_id, .. } => {
            log::debug!("tmux pane mode changed: %{pane_id}");
        }
        Notification::SessionsChanged
        | Notification::SessionChanged { .. }
        | Notification::SessionRenamed { .. } => {
            if let Ok(list) = sessions.read() {
                let infos: Vec<TmuxSessionInfo> =
                    list.sessions().iter().map(TmuxSessionInfo::from).collect();
                let _ = app.emit_to(
                    window_label,
                    "tmux-sessions-changed",
                    TmuxSessionsChangedEvent { sessions: infos },
                );
            }
        }
        Notification::Exit { reason } => {
            let _ = app.emit_to(
                window_label,
                "tmux-disconnected",
                TmuxDisconnectedEvent {
                    reason: reason.clone(),
                },
            );
        }
        _ => {}
    }
}

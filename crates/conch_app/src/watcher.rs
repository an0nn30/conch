//! File watching for live-reload of config, plugins, themes, and SSH config.

use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use notify::{RecommendedWatcher, RecursiveMode, Watcher, Event};

/// What kind of file change was detected.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum FileChangeKind {
    Config,
    Plugins,
    Themes,
    SshConfig,
}

/// Debounced file change event.
pub(crate) struct FileChange {
    pub kind: FileChangeKind,
}

/// Manages file watchers and debounces change events.
pub(crate) struct FileWatcher {
    /// Receives raw notify events.
    rx: mpsc::Receiver<notify::Result<Event>>,
    /// Must be kept alive so the watcher thread continues.
    _watcher: RecommendedWatcher,
    /// Paths being watched and their associated change kinds.
    watched: Vec<(PathBuf, FileChangeKind)>,
    /// Debounce: last event time per kind.
    last_event: std::collections::HashMap<FileChangeKind, Instant>,
}

/// Minimum time between emitting the same kind of change event.
const DEBOUNCE: Duration = Duration::from_secs(1);

impl FileWatcher {
    /// Start watching config, plugin, theme, and SSH config paths.
    pub(crate) fn start() -> Option<Self> {
        let (tx, rx) = mpsc::channel();

        let mut watcher = match RecommendedWatcher::new(
            move |res| { let _ = tx.send(res); },
            notify::Config::default().with_poll_interval(Duration::from_secs(2)),
        ) {
            Ok(w) => w,
            Err(e) => {
                log::warn!("Failed to create file watcher: {e}");
                return None;
            }
        };

        let config_path = conch_core::config::config_path();
        let plugins_dir = conch_core::config::config_dir().join("plugins");
        let themes_dir = conch_core::config::config_dir().join("themes");
        let ssh_config = conch_core::ssh_config::ssh_config_path();

        let mut watched = Vec::new();

        // Watch config.toml (file-level).
        if config_path.exists() {
            if let Err(e) = watcher.watch(&config_path, RecursiveMode::NonRecursive) {
                log::warn!("Cannot watch config.toml: {e}");
            } else {
                watched.push((config_path.clone(), FileChangeKind::Config));
            }
        } else {
            // Watch the config directory so we detect config.toml being created.
            let dir = conch_core::config::config_dir();
            if dir.exists() {
                if let Err(e) = watcher.watch(&dir, RecursiveMode::NonRecursive) {
                    log::warn!("Cannot watch config dir: {e}");
                }
                watched.push((config_path.clone(), FileChangeKind::Config));
            }
        }

        // Watch plugins directory (recursive — subdirs may contain plugins).
        if plugins_dir.exists() {
            if let Err(e) = watcher.watch(&plugins_dir, RecursiveMode::Recursive) {
                log::warn!("Cannot watch plugins dir: {e}");
            }
        }
        watched.push((plugins_dir, FileChangeKind::Plugins));

        // Watch themes directory.
        if themes_dir.exists() {
            if let Err(e) = watcher.watch(&themes_dir, RecursiveMode::Recursive) {
                log::warn!("Cannot watch themes dir: {e}");
            }
        }
        watched.push((themes_dir, FileChangeKind::Themes));

        // Watch SSH config.
        if ssh_config.exists() {
            if let Err(e) = watcher.watch(&ssh_config, RecursiveMode::NonRecursive) {
                log::warn!("Cannot watch SSH config: {e}");
            }
        } else {
            // Watch ~/.ssh/ directory to detect config creation.
            let ssh_dir = ssh_config.parent().map(|p| p.to_path_buf());
            if let Some(ref dir) = ssh_dir {
                if dir.exists() {
                    if let Err(e) = watcher.watch(dir, RecursiveMode::NonRecursive) {
                        log::warn!("Cannot watch .ssh dir: {e}");
                    }
                }
            }
        }
        watched.push((ssh_config, FileChangeKind::SshConfig));

        Some(Self {
            rx,
            _watcher: watcher,
            watched,
            last_event: std::collections::HashMap::new(),
        })
    }

    /// Drain pending events and return debounced change kinds.
    pub(crate) fn poll(&mut self) -> Vec<FileChange> {
        let now = Instant::now();
        let mut triggered = std::collections::HashSet::new();

        while let Ok(event_result) = self.rx.try_recv() {
            let Ok(event) = event_result else { continue };

            // Determine which kind(s) this event maps to.
            for path in &event.paths {
                for (watched_path, kind) in &self.watched {
                    let matches = if watched_path.is_dir() {
                        path.starts_with(watched_path)
                    } else {
                        path == watched_path
                    };
                    if matches {
                        // Check debounce.
                        let should_emit = self
                            .last_event
                            .get(kind)
                            .map(|last| now.duration_since(*last) >= DEBOUNCE)
                            .unwrap_or(true);
                        if should_emit {
                            triggered.insert(kind.clone());
                            self.last_event.insert(kind.clone(), now);
                        }
                    }
                }
            }
        }

        triggered
            .into_iter()
            .map(|kind| FileChange { kind })
            .collect()
    }
}

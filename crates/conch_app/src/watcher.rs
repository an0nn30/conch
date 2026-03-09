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
    /// Known paths and their associated change kinds.
    config_path: PathBuf,
    plugins_dir: PathBuf,
    themes_dir: PathBuf,
    ssh_config: PathBuf,
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

        let config_dir = conch_core::config::config_dir();
        let config_path = conch_core::config::config_path();
        let plugins_dir = config_dir.join("plugins");
        let themes_dir = config_dir.join("themes");
        let ssh_config = conch_core::ssh_config::ssh_config_path();

        // Ensure the config directory exists so we can watch it.
        if !config_dir.exists() {
            let _ = std::fs::create_dir_all(&config_dir);
        }

        // Watch the entire conch config directory recursively.
        // This covers config.toml, plugins/, and themes/ — even if the
        // subdirectories don't exist yet (they'll be detected when created).
        if let Err(e) = watcher.watch(&config_dir, RecursiveMode::Recursive) {
            log::warn!("Cannot watch config dir {}: {e}", config_dir.display());
        }

        // Watch SSH config separately (it's outside the conch config dir).
        if ssh_config.exists() {
            if let Err(e) = watcher.watch(&ssh_config, RecursiveMode::NonRecursive) {
                log::warn!("Cannot watch SSH config: {e}");
            }
        } else if let Some(ssh_dir) = ssh_config.parent() {
            // Watch ~/.ssh/ so we detect config being created.
            if ssh_dir.exists() {
                if let Err(e) = watcher.watch(ssh_dir, RecursiveMode::NonRecursive) {
                    log::warn!("Cannot watch .ssh dir: {e}");
                }
            }
        }

        Some(Self {
            rx,
            _watcher: watcher,
            config_path,
            plugins_dir,
            themes_dir,
            ssh_config,
            last_event: std::collections::HashMap::new(),
        })
    }

    /// Classify a changed path into its FileChangeKind.
    fn classify(&self, path: &std::path::Path) -> Option<FileChangeKind> {
        if path == self.config_path {
            return Some(FileChangeKind::Config);
        }
        if path.starts_with(&self.plugins_dir) {
            return Some(FileChangeKind::Plugins);
        }
        if path.starts_with(&self.themes_dir) {
            return Some(FileChangeKind::Themes);
        }
        // SSH config or anything inside ~/.ssh/
        if path == self.ssh_config || path.starts_with(self.ssh_config.parent().unwrap_or(path)) {
            if path == self.ssh_config
                || path.file_name().and_then(|f| f.to_str()) == Some("config")
            {
                return Some(FileChangeKind::SshConfig);
            }
        }
        None
    }

    /// Drain pending events and return debounced change kinds.
    pub(crate) fn poll(&mut self) -> Vec<FileChange> {
        let now = Instant::now();
        let mut triggered = std::collections::HashSet::new();

        while let Ok(event_result) = self.rx.try_recv() {
            let Ok(event) = event_result else { continue };

            for path in &event.paths {
                if let Some(kind) = self.classify(path) {
                    let should_emit = self
                        .last_event
                        .get(&kind)
                        .map(|last| now.duration_since(*last) >= DEBOUNCE)
                        .unwrap_or(true);
                    if should_emit {
                        triggered.insert(kind.clone());
                        self.last_event.insert(kind, now);
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

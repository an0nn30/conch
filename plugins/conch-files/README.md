# conch-files

A native file explorer plugin for Conch. Provides a side panel that browses local and remote (SFTP) filesystems, switching automatically based on the active terminal tab.

## Features

- **Local browsing** — lists files via `std::fs` when a local terminal tab is active
- **Remote browsing** — lists files via SFTP (queries the `conch-ssh` plugin's `list_dir` service) when an SSH tab is active
- **Auto-switching** — reacts to `ssh.session_ready`, `ssh.session_closed`, and `app.tab_changed` bus events to follow the focused session
- **Navigation** — back/forward history, home button, path text input with submit-on-enter
- **Sorting** — click column headers to sort by name, extension, size, or modified date
- **Column visibility** — right-click column headers to toggle Ext/Size/Modified columns
- **Context menu** — right-click rows for New Folder, Rename, Delete, Copy Path
- **Dynamic title** — shows the local hostname or `user@host` for remote sessions

## Architecture

```
src/
  lib.rs      — plugin state, event handling, widget rendering (declarative)
  local.rs    — local filesystem listing (std::fs::read_dir)
  remote.rs   — SFTP operations via query_plugin("SSH Manager", ...)
  format.rs   — file size, date, and extension formatting helpers
```

The plugin is a `cdylib` built with the `declare_plugin!` macro from `conch_plugin_sdk`. It registers a left-side panel and communicates with the host through:

- **Widget events** — button clicks, table interactions, toolbar input
- **Bus events** — subscribes to `ssh.session_ready`, `ssh.session_closed`, `app.tab_changed`
- **Plugin queries** — calls `conch-ssh` SFTP services (`list_dir`, `mkdir`, `rename`, `delete`)

## Dependencies

- `conch_plugin_sdk` — plugin ABI and widget types
- `conch-ssh` — provides SFTP operations for remote browsing (runtime dependency via plugin bus)
- `hostname` — resolves local machine name for the panel title
- `dirs` — resolves the user's home directory

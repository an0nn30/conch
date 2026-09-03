# SFTP / FileZilla Feature Gap

Current state: TermLab ships a built-in File Explorer panel with SFTP access via `crates/termlab_remote/src/sftp.rs`, Tauri commands in `crates/termlab_tauri/src/remote/sftp_commands.rs`, and the UI in `crates/termlab_tauri/frontend/app/panels/files-panel.js` / `features/files/*`.

The panel already provides:
* Dual-pane local / remote browsing, back/forward/home/refresh/path input
* Sortable Name / Ext / Size / Modified table
* Context menu: New Folder, Rename, Delete, Copy Path, Upload / Download
* SFTP ops: list_dir, stat, read_file, write_file, mkdir, rename, remove, realpath
* Transfer engine with progress events and cancel
* Remote host dropdown with detached sessions, vault auth, pinning
* Navigation history per pane

## Missing to reach FileZilla-class completeness

### Site / Connection Management
* Dedicated Site Manager UI with create/edit/delete, folders/groups, import/export
* Protocol support beyond SFTP: FTP, FTPS, SFTP, SCP
* Connection profiles with custom port, cipher, keep-alive, transfer mode settings
* SSH config import is present, but no per-site transfer settings, bandwidth limits, or proxy/Jump host per site
* Connection queue / automatic reconnect with retry count and backoff
* Session health monitoring and background keep-alive pings

### Transfer Engine
* Transfer queue with priority, pause/resume per transfer and globally
* Resume interrupted transfers / partial file support
* Concurrent transfer limits and bandwidth throttling per site / globally
* Directory sync / mirror / compare with diff UI
* Transfer log with detailed error codes and retry history
* Drag-and-drop between local and remote panes and from OS
* Bulk operations: multi-select upload/download, queue folder recursively

### Browsing & File Operations
* Dual-pane side-by-side inside the SFTP panel, not just local vs remote context switch
* Directory bookmarking / favorites and quick access
* Hidden files toggle, file filters, and column chooser persistence
* File preview / hex view / image preview in pane
* Inline text editor with syntax highlighting for remote files
* Permissions editor UI, ownership change, chmod dialog
* Symlink handling, remote realpath resolution UI, follow symlinks toggle
* Empty folder creation with confirmation, recursive delete with warning

### UI / UX
* Transfer panel with live list, progress bars, speed, ETA, cancel/pause/retry
* Site quick connect bar and recent connections
* Keyboard shortcuts for all file ops, focus management for power users
* Context menu customization and additional actions: chmod, download as, upload queue, compare
* Search within directory, find files by name/content
* File size human readable with consistent formatting, date formats per locale
* Column visibility toggle, custom column order, saved views
* Status bar with connection info, transfer stats, and error indicators

### Security & Credentials
* Per-site password save toggle, master password vault integration already partial
* SSH key passphrases with agent support
* Certificate-based auth, host key verification UI with fingerprint display
* Private browsing mode with no credential persistence

### Advanced
* Rsync / delta sync support
* Remote command execution tied to file ops
* Server-side file listing caching for large directories
* Virtual file system integration for OS file dialogs
* Plugin API for custom transfer handlers / custom commands
* Logging and auditing for compliance

Priorities for incremental work:
1. Transfer queue + resume
2. Site Manager UI with protocol selection
3. Dual-pane split view and drag-drop
4. Directory sync / compare
5. Transfer panel with pause/resume/cancel

This list can be used as a backlog for turning the current SFTP panel into a FileZilla-like workstation.

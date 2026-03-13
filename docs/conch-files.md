# conch-files — File Explorer Plugin Plan

## Overview

A native Rust panel plugin that provides dual-context file browsing: local filesystem (via `std::fs`) and remote filesystem (via SFTP, queried through the conch-ssh plugin's RPC services). Renders in the **Left** panel location.

**Reference layout** (from screenshot and main branch implementation):
```
┌─ Header: "Local" / "Remote (user@host)" label
├─ Toolbar row
│  ├─ [<] Back  [>] Forward
│  ├─ [ /path/to/current/dir           ] (editable text input)
│  └─ [Home] [Refresh]
├─ Table (scrollable, sortable)
│  ├─ Header: Name | Ext | Size | Modified
│  └─ Rows: icon + name | ext | size | date
└─ Footer: "42 items"
```

---

## Architecture

### Plugin Type
- **Native Rust plugin** (`cdylib`), same pattern as `conch-ssh`
- `PluginType::Panel`, `PanelLocation::Left`
- Workspace crate at `plugins/conch-files/`

### Filesystem Backends

1. **Local** — `std::fs::read_dir()`, always available
2. **Remote (SFTP)** — RPC to conch-ssh via `query_plugin("ssh", "list_dir", ...)`. Activated when the active terminal tab is an SSH session.

### Session Awareness

The plugin subscribes to bus events to track which sessions are SSH:
- `ssh.session_ready` — SSH session connected, cache `{ session_id, host, user }`
- `app.tab_changed` — Active tab switched; check if new tab is an SSH session
- When active tab is SSH → show remote browser, query SFTP
- When active tab is local → show local browser

### State Model

```rust
struct FilesPlugin {
    api: HostApi,
    // Navigation
    current_path: String,        // Current directory path
    path_input: String,          // Editable text input value
    back_stack: Vec<String>,     // Navigation history
    forward_stack: Vec<String>,
    // Content
    entries: Vec<FileEntry>,     // Current directory listing
    selected_row: Option<usize>, // Selected row index
    sort_column: Option<usize>,  // Column index to sort by
    sort_ascending: bool,
    // Context
    mode: BrowseMode,           // Local or Remote { session_id, host, user }
    ssh_sessions: HashMap<String, SshSessionInfo>, // session_id -> info
    active_session_id: Option<String>,
    // UI
    dirty: bool,                // Needs re-render
    error: Option<String>,      // Error message to display
    item_count: usize,          // For footer
}

struct FileEntry {
    name: String,
    is_dir: bool,
    size: u64,
    modified: Option<u64>,      // Unix timestamp
    permissions: Option<String>, // Octal string (remote only)
}

enum BrowseMode {
    Local,
    Remote { session_id: String, host: String, user: String },
}
```

### Widget Composition

Using SDK widget types already available on v2:

```
Vertical [
    Toolbar {
        items: [
            Button { id: "back", icon: "go-previous", enabled: !back_stack.is_empty() },
            Button { id: "forward", icon: "go-next", enabled: !forward_stack.is_empty() },
            Separator,
            TextInput { id: "path", value: current_path, submit_on_enter: true },
            Separator,
            Button { id: "home", icon: "go-home" },
            Button { id: "refresh", icon: "refresh" },
        ]
    },
    Table {
        id: "files",
        columns: [
            { name: "Name", width: 200 },
            { name: "Ext", width: 80 },
            { name: "Size", width: 80 },
            { name: "Modified", width: 130 },
        ],
        rows: [...],   // Built from entries
        sort_column, sort_ascending, selected_row,
    },
    Separator,  // Footer pinning separator
    Label { text: "42 items", style: Secondary },
]
```

---

## Dependencies on conch-ssh

### Required SFTP Services (already registered by conch-ssh)

| Service     | Args                  | Returns                                  |
|-------------|-----------------------|------------------------------------------|
| `list_dir`  | `{ path: "/foo" }`    | `{ status: "ok", entries: [...] }`       |
| `stat`      | `{ path: "/foo/bar" }`| `{ status: "ok", size, is_dir, ... }`    |
| `mkdir`     | `{ path: "/new" }`    | `{ status: "ok" }`                       |
| `rename`    | `{ from, to }`        | `{ status: "ok" }`                       |
| `delete`    | `{ path }`            | `{ status: "ok" }`                       |

### Missing from conch-ssh `list_dir` Response

The current `list_dir` returns `name`, `size`, `is_dir`, `permissions` but **not `mtime`**. We need to add `mtime` (modification timestamp) to the JSON response. The `russh_sftp` `Metadata` struct provides `mtime` via its SFTP attrs.

### Session Targeting

RPC queries go to the "ssh" plugin globally. When querying SFTP, we need to specify *which* SSH session to use. The conch-ssh plugin needs a way to route SFTP ops to a specific session. Options:
1. Add `session_id` field to all SFTP query args (preferred)
2. Use the "active" session implicitly

We'll use option 1: all SFTP queries include `{ session_id: "...", path: "..." }`.

This means conch-ssh's SFTP query handlers need to look up the session by ID and route to the correct SSH handle. This may require a small change if they currently only use the "active" session.

---

## Phases

### Phase 0: Prerequisites (conch-ssh changes)

**Goal:** Ensure the SSH plugin's SFTP services support everything the file explorer needs.

**Tasks:**
1. **Add `mtime` to `list_dir` response** — Read `metadata().mtime` from russh_sftp entries, include in JSON
2. **Add `session_id` routing to SFTP queries** — SFTP operations currently use a single SSH handle; add `session_id` field to query args so the correct session handle is selected
3. **Verify `get_sessions` service** — Ensure it returns session IDs that match `ssh.session_ready` events

**Estimated scope:** ~30 lines changed in `plugins/conch-ssh/src/sftp.rs` and the query dispatcher in `lib.rs`.

---

### Phase 1: Scaffold & Local File Browsing

**Goal:** Plugin loads, renders a file table with local filesystem entries.

**Tasks (can be parallelized where noted):**

1. **Create crate scaffold** `plugins/conch-files/`
   - `Cargo.toml` (deps: conch_plugin_sdk, serde, serde_json, log)
   - `src/lib.rs` with `declare_plugin!` macro
   - Add to workspace `Cargo.toml` members

2. **Implement `FilesPlugin` state struct**
   - Fields: current_path, entries, path_input, back/forward stacks, sort state
   - Initialize with user's home directory (`dirs::home_dir()`)

3. **Implement local `list_dir`** (parallel with #2)
   - `std::fs::read_dir()` → `Vec<FileEntry>`
   - Extract: name, is_dir, size (metadata.len()), modified (metadata.modified() → unix timestamp)
   - Sort: directories first, then alphabetical by name (case-insensitive)

4. **Implement `render()`** — Build widget tree:
   - Toolbar with back/forward/path input/home/refresh buttons
   - Table with Name, Ext, Size, Modified columns
   - Footer with item count
   - Format sizes (B/KB/MB/GB), dates (YYYY-MM-DD HH:MM), extensions

5. **Implement `handle_event()`** — Handle widget events:
   - `ButtonClick("back")` → pop back_stack, push current to forward_stack, reload
   - `ButtonClick("forward")` → pop forward_stack, push current to back_stack, reload
   - `ButtonClick("home")` → navigate to home dir
   - `ButtonClick("refresh")` → reload current dir
   - `TextInputSubmit("path")` → navigate to entered path
   - `TableActivate("files")` → if directory, navigate into it
   - `TableSort("files")` → re-sort entries by column
   - `TableSelect("files")` → update selected_row

**Deliverable:** Plugin loads, displays local files, navigation works.

---

### Phase 2: Remote File Browsing (SFTP)

**Goal:** Detect SSH sessions, switch to remote browsing, query SFTP.

**Tasks:**

1. **Subscribe to bus events**
   - `ssh.session_ready` — Cache session info (session_id, host, user)
   - `app.tab_changed` — Check if active tab is SSH, switch mode

2. **Implement mode switching**
   - When `BrowseMode::Remote`: header shows "Remote (user@host)"
   - When switching to remote: query `list_dir` for home directory (".")
   - When switching to local: revert to local path

3. **Implement remote `list_dir`**
   - `query_plugin("ssh", "list_dir", { session_id, path })` → parse response JSON
   - Convert response entries to `Vec<FileEntry>`
   - Handle errors gracefully (show error label, don't crash)

4. **Handle session disconnect**
   - If SSH session closes while in remote mode, fall back to local mode
   - Listen for session close events or handle query failures

**Deliverable:** Seamless switching between local and remote file browsing.

---

### Phase 3: Polish & UX

**Goal:** File type icons, context menus, error handling, keyboard navigation.

**Tasks:**

1. **File type icons** — Use SDK icon constants:
   - `icons::FOLDER` / `icons::FOLDER_OPEN` for directories
   - `icons::FILE` for files
   - Map into `IconLabel` widgets or Table row icon column

2. **Extension labels** — Map common extensions to descriptions:
   - "rs" → "Rust Source", "py" → "Python", "pdf" → "PDF Document", etc.
   - Show in Ext column (directories show `<DIR>`)

3. **Context menu** — Right-click on files:
   - "New Folder" (calls mkdir locally or via SFTP)
   - "Delete" (with confirm dialog)
   - "Rename" (with prompt dialog)
   - "Copy Path" (clipboard)

4. **Error handling**
   - Permission denied → show error label, don't navigate
   - SFTP timeout → show retry option
   - Invalid path → show error, stay in current directory

5. **Empty state** — Show helpful message when directory is empty

**Deliverable:** Polished file explorer with icons, context menus, good error UX.

---

### Phase 4: File Operations (Future)

**Goal:** Upload/download between local and remote panes.

> This phase is intentionally deferred. The initial plugin focuses on browsing. Transfer functionality can be added later once the browsing UX is solid.

**Future tasks:**
- Drag-and-drop between file browsers
- Upload/download buttons
- Transfer progress UI
- Dual-pane split view (local + remote side by side)

---

## Parallel Work Breakdown

```
Phase 0                    Phase 1                         Phase 2
─────────────────         ─────────────────────────       ──────────────
[mtime in list_dir]  ──→  [crate scaffold + plugin macro] ──→ [bus subscriptions]
[session_id routing] ──→  [local list_dir impl]      ─┐      [mode switching]
                          [render() widget tree]      ├──→    [remote list_dir]
                          [handle_event() navigation] ─┘      [disconnect handling]
                          [format helpers: size, date, ext]
```

- Phase 0 tasks are independent of each other (parallel)
- Phase 1 tasks 2 and 3 are independent (parallel), task 4 depends on 2+3, task 5 depends on 4
- Phase 2 depends on Phase 0 completion (for session_id routing) and Phase 1 (for plugin scaffold)
- Phase 3 items are mostly independent (parallel)

---

## Key Files to Create

| File | Purpose |
|------|---------|
| `plugins/conch-files/Cargo.toml` | Crate manifest |
| `plugins/conch-files/src/lib.rs` | Plugin entry, state, declare_plugin! |
| `plugins/conch-files/src/local.rs` | Local filesystem operations |
| `plugins/conch-files/src/remote.rs` | SFTP operations via RPC |
| `plugins/conch-files/src/format.rs` | Size/date/extension formatting helpers |

## Key Files to Modify

| File | Change |
|------|--------|
| `plugins/conch-ssh/src/sftp.rs` | Add `mtime` to list_dir response |
| `plugins/conch-ssh/src/lib.rs` | Add session_id routing to SFTP query handlers |
| `Cargo.toml` (workspace root) | Add `conch-files` to members |

---

## Widget Events to Handle

| Event | Action |
|-------|--------|
| `ButtonClick("back")` | Navigate back |
| `ButtonClick("forward")` | Navigate forward |
| `ButtonClick("home")` | Go to home directory |
| `ButtonClick("refresh")` | Reload current directory |
| `TextInputSubmit("path")` | Navigate to entered path |
| `TableActivate("files", row)` | Double-click: navigate into directory |
| `TableSelect("files", row)` | Single-click: select file |
| `TableSort("files", col, asc)` | Re-sort by column |
| `BusEvent("ssh.session_ready")` | Cache SSH session info |
| `BusEvent("app.tab_changed")` | Check if active tab is SSH, switch mode |

---

## Formatting Reference

**Size:** `format_size(bytes: u64) -> String`
- 0 → "0 B", 1023 → "1023 B", 1024 → "1.0 KB", 1048576 → "1.0 MB", etc.
- Directories → `<DIR>`

**Date:** `format_date(timestamp: u64) -> String`
- Unix timestamp → "2024-03-15 14:30"
- Missing → "—"

**Extension:** `extension_label(name: &str, is_dir: bool) -> String`
- Directories → `<DIR>`
- "main.rs" → "Rust Source"
- "photo.jpg" → "JPEG Image"
- Unknown → uppercase extension ("TXT", "BIN", etc.)

---

## Open Questions

1. **Column visibility toggle** — The main branch supports right-click on table headers to toggle column visibility. The SDK `Table` widget doesn't currently emit header context menu events. Should we add `TableHeaderContextMenu` to the SDK, or defer this feature?

Answer: Add this feature, extend the SDK

2. **Dual-pane layout** — The main branch shows local+remote side-by-side. As a left panel plugin, we have limited width. Should we show only one context at a time (switching based on active tab), or attempt a split? **Decision: Single pane, context-switched.** Dual-pane can be revisited in Phase 4 with a `SplitPane` widget.



3. **Hidden files** — Should we show dotfiles by default? Main branch does not filter them. **Decision: Show all files; add a toggle button later if needed.**

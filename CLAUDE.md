# TermLab — Claude Instructions

This file is the published working agreement for AI coding agents operating in this repository, including Codex and Claude-style agents.

## Critical Engineering Standards

### 1. Unit Tests Are Required
Every new function, module, or behavior change MUST have unit tests if at all possible. The project already has `#[cfg(test)]` modules in most files — follow that pattern. If adding a new `.rs` file, add a `#[cfg(test)] mod tests` at the bottom. Pure logic, parsers, config handling, widget building — all testable without a GUI context. Only skip tests for code that truly requires a live Tauri context or OS-level resources.

### 2. Modularity — No Monoliths
Code MUST be broken into small, focused modules. When adding new functionality:
- Extract into its own file/module
- Group related files into subdirectories with `mod.rs` (e.g., `remote/`, `plugins/`)
- Each file should have a single responsibility
- Prefer many small files over few large files
- New features go in new modules, not appended to existing large files
- `lib.rs` should delegate to submodules — avoid growing it beyond ~1000 lines

## Git Workflow (STRICT)

- **Claude must never commit or push directly to `main`.**
- **Codex and all other AI agents must never commit or push directly to `main`.**
- The repo owner (`an0nn30`) may push directly to `main` when appropriate.
- Every feature, fix, or change — no matter how small — must go on its own branch.
- Branch naming convention:
  - `feat/short-description` — new features
  - `fix/short-description` — bug fixes
  - `chore/short-description` — docs, config, tooling, cleanup
  - `perf/short-description` — performance improvements
- Pick the branch prefix based on the primary intent of the work. Bug fixes belong on `fix/*` branches, new user-facing behavior belongs on `feat/*`, and refactors or cleanup without behavior changes belong on `chore/*` unless they are clearly performance-focused.
- Before starting any work, check the current branch. If on `main`, create a new branch first.
- Push the branch to origin. Never open PRs unless the user explicitly asks.
- Never use `--force` push.
- Keep branches narrowly scoped. Do not mix unrelated fixes, features, and refactors into one branch.

## Commit Rules

- Never add Co-Authored-By lines to commits.
- Write concise, descriptive commit messages in the imperative mood.
- PRs should be small and focused — one concern per PR.
- This is a public, open-source repo. Be thoughtful about what goes into commits.
- Do not commit generated noise, local scratch files, or agent-specific workspace artifacts unless the user explicitly asks.

## Delivery Standards

- Always add tests for behavior changes when the code is testable without a live GUI or OS-bound environment.
- Prefer extending existing focused modules over growing large catch-all files.
- Do not introduce new monoliths in Rust, JavaScript, or docs. If a file is already large, strongly prefer extracting a helper module instead of adding another major feature to it.
- Follow the existing architecture and naming patterns in the surrounding crate or frontend runtime area before introducing a new abstraction.
- When changing behavior, update nearby docs, examples, or inline help text if the user-visible contract changed.
- Favor incremental, reviewable changes over broad rewrites.

## Architecture

### Workspace (4 crates, no egui, no native plugins)
```
crates/
  termlab_core/         — Config loading, color schemes, persistent state
  termlab_plugin_sdk/   — Widget/event types shared with Lua and Java plugins
  termlab_plugin/       — Plugin host: message bus, Lua runner, Java runtime, HostApi trait
  termlab_tauri/        — The app: Tauri v2 / xterm.js UI, built-in SSH/SFTP/tunnels
java-sdk/             — Java Plugin SDK: HostApi, TermLabPlugin, Widgets, PluginInfo
editors/
  vscode/             — VS Code extension for Lua plugin development
```

### termlab_tauri — The App
```
src/
  main.rs             — Entry point, config loading, launches Tauri app
  lib.rs              — Tauri setup, commands, menu building, window management
  theme.rs            — Color theme loading (Alacritty .toml → CSS variables)
  pty_backend.rs      — Local PTY via portable-pty (raw byte I/O for xterm.js)
  ipc.rs              — Unix socket IPC listener (termlab msg new-tab/new-window)
  watcher.rs          — File watcher for config/theme hot-reload
  remote/             — Built-in SSH + SFTP + file operations (not a plugin)
    mod.rs            — Session registry, Tauri commands, auth prompt bridge
    ssh.rs            — SSH connection (russh 0.48), auth, proxy, channel I/O
    sftp.rs           — SFTP operations via russh-sftp
    local_fs.rs       — Local filesystem operations (same FileEntry interface)
    config.rs         — Server entries, folders, tunnels, ~/.ssh/config import
    known_hosts.rs    — OpenSSH known_hosts read/write
    transfer.rs       — Upload/download engine with progress events
    tunnel.rs         — SSH tunnel manager (local port forwarding)
  plugins/            — Plugin integration for Tauri
    mod.rs            — PluginState, discovery, enable/disable, dialog responses
    tauri_host_api.rs — TauriHostApi implementing the safe HostApi trait
frontend/
  index.html          — Main HTML/CSS/JS, xterm.js terminal, layout, all inline styles
  utils.js            — Shared utilities (esc, attr, formatSize, formatDate)
  toast.js            — Global toast notification system
  ssh-panel.js        — SSH server tree, quick connect, tunnels, connection forms
  files-panel.js      — Dual-pane file explorer (local + remote)
  tunnel-manager.js   — SSH tunnel CRUD dialog
  plugin-widgets.js   — Widget JSON → HTML renderer, plugin dialog handlers
  plugin-manager.js   — Plugin discovery/enable/disable dialog
  app/features/editor/          — Light editor: pane, open/save service, file dialog
  app/features/editor/preview/  — Markdown preview: renderer, sanitizer policy,
                                  fence highlighting, image resolver, sandboxed
                                  frame, mode state, per-pane controller
  icons/              — PNG icon set (file, folder, server, navigation, etc.)
```

### Plugin System (2 tiers — Lua + Java only)
- **Java** (Java/Kotlin/Scala): `.jar` files loaded by embedded JVM via JNI
- **Lua** (5.4): single `.lua` files loaded by mlua
- Both tiers call the safe `HostApi` Rust trait (no C ABI, no vtables, no unsafe)
- Declarative widget system: plugins return JSON widget trees → rendered as HTML
- Pub/sub event bus + RPC queries for inter-plugin communication
- Blocking dialog APIs (form, prompt, confirm) via oneshot channels
- Plugin config persistence: `~/.config/termlab/plugins/{name}/{key}.json`
- Plugins are NOT auto-loaded — managed via Tools > Plugin Manager
- Enabled plugins persisted in `state.toml` and restored on restart

### Built-in Features (not plugins)
SSH sessions, SFTP file browsing, file transfers, SSH tunnels, server
management, `~/.ssh/config` import, and host key verification are all
built directly into `termlab_tauri/src/remote/`. They were previously
separate native plugins but were consolidated for reliability.

### Key Patterns
- **Tauri v2 webview**: HTML/CSS/JS frontend, Rust backend via commands + events
- **xterm.js**: handles all terminal emulation; backend provides raw byte streams
- **CSS custom properties**: all colors derived from Alacritty theme files (`var(--bg)`, etc.)
- **Shared JS utilities**: `utils.js` provides `esc()`, `attr()`, `formatSize()`, `formatDate()` — no duplicating these across modules
- **Toast notifications**: all user-facing messages go through `toast.js` — never use `alert()` or `confirm()`
- **Overlay dialogs**: use the `ssh-overlay` / `ssh-form` CSS pattern with Escape to close
- **Auth prompts**: oneshot channels — emit event to frontend, block calling thread on response
- **SSH sessions**: reuse the same `pty-output`/`pty-exit` events as local PTY tabs
- **Plugin menu items**: stored in shared state, native menu rebuilt dynamically after enable
- **State persistence**: window size, panel widths, panel visibility, enabled plugins in `state.toml`
- **Hot-reload**: `watcher.rs` polls config.toml + themes/ every 2s, emits `config-changed` event
- **Markdown preview**: rendered HTML is sanitized and displayed in an iframe
  sandboxed WITHOUT `allow-scripts` — never add it. `csp` is null app-wide, so
  the sandbox is what keeps an opened `.md` file from reaching `__TAURI__`.

## Style Guide

### Rust
- Use `pub(crate)` for internal visibility, not `pub` (unless it's a library API)
- Prefer `if let` / `match` over `.unwrap()` — handle errors gracefully
- Use `log::error!`/`log::warn!` for recoverable errors, not panics
- `#[serde(default)]` on config structs for backward compatibility
- Keep `unsafe` blocks minimal and well-commented
- No unnecessary `clone()` — borrow where possible
- Factory methods for repeated struct construction (e.g., `PluginState::make_host_api()`)
- Prefer small structs and focused helper functions over deeply stateful "manager" objects when simpler composition will do
- Keep modules cohesive: parsing with parsing, persistence with persistence, UI wiring with UI wiring
- Add or extend `#[cfg(test)]` coverage in the same file when practical, especially for parsing, config, widget-building, and state-transition logic
- Preserve backward compatibility for persisted config/state unless the user explicitly approves a breaking change
- Prefer explicit types and straightforward control flow over clever abstractions

### Frontend (JS)
- Each JS module is a self-contained IIFE exposing a global (e.g., `window.sshPanel`)
- Use `window.utils.esc()` / `window.utils.attr()` — never define local copies
- Use the global `toast.js` system for all notifications — never use `alert()` or `confirm()`
- CSS uses custom properties (`var(--bg)`, `var(--fg)`, `var(--text-secondary)`) — never hardcode hex colors
- Overlay dialogs use the `ssh-overlay` / `ssh-form` CSS pattern
- Escape handlers must use capture phase (`addEventListener(..., true)`) to fire before xterm.js
- Icons: use PNG assets from `frontend/icons/` via `<img>` tags or `iconHtml()` helper
- Keep runtime modules small and purpose-specific; if a frontend file starts becoming a grab bag, split it
- Prefer event delegation and shared helpers over duplicating DOM wiring across panels
- Preserve focus behavior and keyboard navigation when changing interactive UI

### Config
- User config: `~/.config/termlab/config.toml` (loaded by termlab_core)
- Persistent state: `~/.config/termlab/state.toml` (window size, plugins, layout)
- SSH server config: `~/.config/termlab/remote/servers.json`
- Plugin config: `~/.config/termlab/plugins/{plugin_name}/{key}.json`
- Keyboard shortcuts: configurable in `[termlab.keyboard]` section
- Default shortcuts use `cmd+` prefix (maps to Cmd on macOS, Ctrl on Linux/Windows)

### Testing Standards
- `#[cfg(test)] mod tests` at the bottom of each file
- Test pure logic: parsing, config defaults, widget building
- Use `assert_eq!` with descriptive messages
- Test edge cases: empty input, missing fields, boundary values
- Plugin SDK: test widget serialization/deserialization
- Config: test defaults, serde round-trips, backward compat with `serde(default)`
- Currently 192 tests across the workspace — keep this growing
- If a change cannot reasonably be covered by an automated test, explain that clearly in the final summary and describe the manual verification performed

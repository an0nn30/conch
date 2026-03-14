# Conch — Claude Instructions

## Critical Engineering Standards

### 1. Unit Tests Are Required
Every new function, module, or behavior change MUST have unit tests if at all possible. The project already has `#[cfg(test)]` modules in most files — follow that pattern. If adding a new `.rs` file, add a `#[cfg(test)] mod tests` at the bottom. Pure logic, parsers, config handling, widget building, keybinding resolution — all testable without a GUI context. Only skip tests for code that truly requires a live egui context or OS-level resources.

### 2. Modularity — No Monoliths
Code MUST be broken into small, focused modules. `app.rs` is already too large and must not grow further. When adding new functionality:
- Extract into its own file/module (e.g., `shortcuts.rs`, `input.rs`, `icons.rs`)
- Group related files into subdirectories with `mod.rs` (e.g., `host/`, `terminal/`, `menu_bar/`, `platform/`)
- Each file should have a single responsibility
- Prefer many small files over few large files
- New features go in new modules, not appended to existing large files

## Git Workflow (STRICT)

- **Claude must never commit or push directly to `main`.**
- The repo owner (`an0nn30`) may push directly to `main` when appropriate.
- Every feature, fix, or change — no matter how small — must go on its own branch.
- Branch naming convention:
  - `feat/short-description` — new features
  - `fix/short-description` — bug fixes
  - `chore/short-description` — docs, config, tooling, cleanup
  - `perf/short-description` — performance improvements
- Before starting any work, check the current branch. If on `main`, create a new branch first.
- Push the branch to origin. Never open PRs unless the user explicitly asks.
- Never use `--force` push.

## Commit Rules

- Never add Co-Authored-By lines to commits.
- Write concise, descriptive commit messages in the imperative mood.
- PRs should be small and focused — one concern per PR.
- This is a public, open-source repo. Be thoughtful about what goes into commits.

## Architecture

### Workspace Structure
```
crates/
  conch_app/        — GUI application (egui/eframe), the main binary
  conch_core/       — Config loading, color schemes, shared types
  conch_plugin/     — Plugin host: Lua runner, native plugin manager, plugin bus
  conch_plugin_sdk/ — SDK for native (Rust) plugins: HostApi, widgets, FFI types
  conch_pty/        — PTY abstraction and connector
plugins/
  conch-ssh/        — Native plugin: SSH sessions, SFTP, server tree
  conch-files/      — Native plugin: dual-pane file explorer with local + SFTP
  test-*/           — Test plugins for development
```

### conch_app Module Layout
```
app.rs              — ConchApp struct, eframe::App impl, update() loop (KEEP SMALL)
main.rs             — Entry point, CLI parsing, window setup
input.rs            — KeyBinding parsing, key_to_bytes conversion, ResolvedShortcuts
shortcuts.rs        — handle_keyboard() — shortcut dispatch and PTY forwarding
state.rs            — AppState, persistent state
sessions.rs         — Session management helpers
icons.rs            — IconCache, icon loading
ui_theme.rs         — UiTheme struct, font sizes, colors
context_menu.rs     — Right-click context menus
tab_bar.rs          — Tab bar rendering
ipc.rs              — Unix socket IPC
watcher.rs          — File system watcher
mouse.rs            — Mouse event handling
extra_window.rs     — Multi-window support
host/               — Plugin hosting bridge
  bridge.rs         — HostApi FFI implementation, SFTP registry, global state
  panel_renderer.rs — Widget rendering (tables, toolbars, trees, buttons, etc.)
  plugin_panels.rs  — Panel layout (left/right/bottom panels with tabs)
  plugin_lifecycle.rs — Plugin start/stop/reload
  plugin_manager_ui.rs — Plugin manager UI
  session_bridge.rs — Session<->plugin bridge
  dialogs.rs        — Plugin-triggered dialogs
terminal/           — Terminal rendering
  widget.rs         — Terminal grid rendering, selection, cursor
  color.rs          — ANSI color mapping
  size_info.rs      — Terminal size calculations
menu_bar/           — Menu bar (egui + native macOS)
platform/           — Platform-specific code (macOS, Linux, Windows)
```

### Native Plugin Architecture
- Plugins are shared libraries (`.dylib`/`.so`/`.dll`) loaded at runtime
- Communicate with the host via `HostApi` — a `#[repr(C)]` struct of function pointers
- Declarative widget system: plugins return `Vec<Widget>` JSON, host renders them
- Events flow back via `WidgetEvent` enum (button clicks, text input, tree selection, etc.)
- Cross-plugin FFI: `SftpVtable` pattern for direct function-pointer access between plugins
- Plugin config persistence via `HostApi::get_config`/`set_config` (JSON files per plugin)

### Key Patterns
- egui immediate-mode: all UI rebuilt every frame, state lives on ConchApp
- Plugin bus: pub/sub event system for plugin<->app and plugin<->plugin communication
- `query_plugin`: IPC between plugins (JSON messages over mpsc channels)
- Panel registry: plugins register panels at locations (Left, Right, Bottom)
- `#[repr(C)]` vtables with manual ref counting for cross-plugin FFI
- Terminal owns keyboard input by default — only divert when a widget explicitly has focus
- Tab key: intercepted in `raw_input_hook` before egui sees it, sent directly to PTY

## Style Guide

### Rust
- Use `pub(crate)` for internal visibility, not `pub` (unless it's a library API)
- Prefer `if let` / `match` over `.unwrap()` — handle errors gracefully
- Use `log::error!`/`log::warn!` for recoverable errors, not panics
- `#[serde(default)]` on config structs for backward compatibility
- Keep `unsafe` blocks minimal and well-commented
- No unnecessary `clone()` — borrow where possible

### Config
- User config: `config.toml` (loaded by conch_core)
- Persistent state: `state.toml` (window size, loaded plugins, layout)
- Plugin config: `{plugin_name}/{key}.json` files via HostApi
- Keyboard shortcuts: configurable in `[conch.keyboard]` section
- Default shortcuts use `cmd+` prefix (maps to Cmd on macOS, Ctrl on Linux/Windows)

### Testing Standards
- `#[cfg(test)] mod tests` at the bottom of each file
- Test pure logic: parsing, config defaults, widget building, keybinding matching
- Use `assert_eq!` with descriptive messages
- Test edge cases: empty input, missing fields, boundary values
- Plugin SDK: test widget serialization/deserialization
- Config: test defaults, serde round-trips, backward compat with `serde(default)`
- Keybindings: test parsing, matching, modifier combos

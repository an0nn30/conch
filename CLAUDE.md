# TermLab — Claude Instructions

This file is the published working agreement for AI coding agents operating in this repository, including Codex and Claude-style agents. `AGENTS.md` defers to this file.

TermLab is a Rust + Tauri v2 terminal workstation (terminal, SSH/SFTP, tunnels, credential vault, light editor, plugins). Current version: `3.0.0-rc.2`, Rust edition 2024.

## Commands

```bash
make help          # list all targets
make dev           # cargo run -p termlab_tauri (skips vendor step)
make run           # vendor frontend deps, then run
make build         # release build -> target/release/termlab
make java-sdk      # build the Java plugin SDK jar (needed for Java plugin tests)
make app           # macOS .app bundle;  make dmg-native | deb | rpm | msi | exe
```

Verification — run these before claiming work is complete:

```bash
cargo test --workspace
cargo clippy --all-targets
cargo fmt -- --check
```

Frontend integration tests are plain Node scripts:

```bash
node scripts/tests/<name>.mjs      # ~59 suites covering editor, transfers, panels, shortcuts
```

CI (`.github/workflows/ci.yml`) runs `make -C java-sdk build`, `cargo test --workspace`,
`cargo clippy --all-targets`, and `cargo fmt -- --check` on every push.

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

**Known violations — do not use these as precedent.** `remote/transfer_queue/runner.rs` (~7.1k lines) and `engine.rs` (~6.1k lines) are the worst offenders; `lib.rs` is already at ~1029 lines. Prefer extracting from these files over adding to them.

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
- Do not commit generated noise, local scratch files, or agent-specific workspace artifacts unless the user explicitly asks. Design docs belong in `docs/superpowers/{specs,plans,notes}/`, not the repo root.

## Delivery Standards

- Always add tests for behavior changes when the code is testable without a live GUI or OS-bound environment.
- Prefer extending existing focused modules over growing large catch-all files.
- Do not introduce new monoliths in Rust, JavaScript, or docs. If a file is already large, strongly prefer extracting a helper module instead of adding another major feature to it.
- Follow the existing architecture and naming patterns in the surrounding crate or frontend runtime area before introducing a new abstraction.
- When changing behavior, update nearby docs, examples, or inline help text if the user-visible contract changed.
- Favor incremental, reviewable changes over broad rewrites.

## Architecture

### Workspace (7 crates, no egui, no native plugins)
```
crates/
  termlab_core/         — Config loading, color schemes, effective theme, persistent state
  termlab_plugin_sdk/   — Widget/event types shared with Lua and Java plugins
  termlab_plugin/       — Plugin host: message bus, Lua runner, JVM runtime, HostApi trait
  termlab_remote/       — Platform-agnostic SSH, SFTP, tunnels, transfer engine (russh)
  termlab_vault/        — Encrypted credential vault, SSH keygen, in-memory agent, keychain
  termlab_share/        — .termlabshare export/import bundles (servers, keys, vault entries)
  termlab_tauri/        — The app: Tauri v2 / xterm.js UI, Tauri commands, windows, menus
java-sdk/             — Java Plugin SDK: HostApi, TermLabPlugin, Widgets, PluginInfo
editors/
  vscode/             — VS Code extension for Lua plugin development
  neovim/             — Neovim/LuaLS type definitions for Lua plugin development
```

`termlab_remote` is deliberately Tauri-free so the SSH/SFTP core can be shared with a
future mobile client. Keep protocol logic there, not in `termlab_tauri`.

### termlab_tauri — The App
```
src/
  main.rs             — Entry point, config loading, launches Tauri app
  lib.rs              — Tauri setup, command registration, window management
  cli.rs              — Dual-mode CLI entry point (termlab binary doubles as a CLI)
  cli_install.rs      — Install/uninstall the `termlab` PATH symlink
  open_path.rs        — CLI/IPC "open this path" routing into windows
  ipc.rs              — Unix socket IPC listener (termlab msg new-tab/new-window)
  windows.rs          — Window creation and management
  chooser_window.rs   — File chooser as its own window-modal OS window
  panel_host.rs       — Pop-out tool windows ("panel hosts")
  menu.rs             — Native menu building (incl. Tools > Plugins)
  commands.rs         — General Tauri commands: config, layout persistence, zoom
  settings.rs         — Settings dialog commands
  theme.rs            — Color theme loading (Alacritty .toml → CSS variables)
  theme_catalog.rs    — Terminal theme catalog for the settings picker
  bundled_themes.rs   — Themes compiled into the binary
  pty.rs              — PTY session registry
  pty_backend.rs      — Local PTY via portable-pty (raw byte I/O for xterm.js)
  utf8_stream.rs      — Incremental UTF-8 decoding of PTY bytes
  editor_fs.rs        — Filesystem access for the light editor
  updater.rs          — In-app update check/download/install
  watcher.rs          — File watcher for config/theme hot-reload
  vault_commands.rs   — Tauri commands over termlab_vault
  share_commands.rs   — Tauri commands over termlab_share
  remote/             — Tauri glue over termlab_remote (NOT the protocol impl)
    mod.rs            — Session registry, OSC-7 cwd tracking, auth prompt bridge
    *_commands.rs     — ssh / sftp / tunnel / server / transfer / detached commands
    local_fs.rs       — Local filesystem ops (same FileEntry interface)
    transfer_queue/   — Durable transfer queue: scheduler, engine, runner, store, reducer
  plugins/            — Plugin integration for Tauri
    mod.rs            — PluginState, discovery, enable/disable, permissions, dialogs
    tauri_host_api.rs — TauriHostApi implementing the safe HostApi trait
frontend/
  index.html          — Main shell; loads app/ modules via plain <script> tags
  settings.html       — Settings window   chooser.html — File chooser window
  app/
    *-runtime.js      — 29 top-level orchestration modules (bootstrap, main-runtime,
                        terminal-runtime, shortcut-runtime, plugin-runtime, tab-manager,
                        pane-manager, state, …) loaded in dependency order
    core/             — utils, tauri-client, config-service, dialog-service,
                        keyboard-router, layout-service, appearance, commands
    ui/               — Design-system components: tl-dialog, tl-combo, tl-menu,
                        tl-icon, tl-spinner, toast, titlebar, notification-panel
    layout/           — split-pane, split-tree, pane-dnd, dock-highlight,
                        tool-window-manager
    panels/           — ssh-panel, files-panel, tunnels-panel, transfer-center,
                        notifications-panel, settings, vault, plugin-widgets
    features/         — editor/, files/, settings/, ssh/, transfers/, vault/,
                        tab-switcher/  (feature-scoped logic)
  styles/design-system/ — tokens-light.css, tokens-dark.css, components/
  types/              — ts-rs generated TypeScript types (do not hand-edit)
  vendor/             — xterm.js, CodeMirror, icons, fonts (built by build-vendor.mjs)
```

### Plugin System (2 tiers — Lua + Java only)
- **Java** (Java/Kotlin/Scala): `.jar` files loaded by embedded JVM via JNI
- **Lua** (5.4): single `.lua` files loaded by mlua
- Both tiers call the safe `HostApi` Rust trait (no C ABI, no vtables, no unsafe)
- Declarative widget system: plugins return JSON widget trees → rendered as HTML
- Pub/sub event bus + RPC queries for inter-plugin communication
- Blocking dialog APIs (form, prompt, confirm) via oneshot channels
- Plugin config persistence: `~/.config/termlab/plugins/{name}/{key}.json`
- Plugins are NOT auto-loaded — enable them in **Settings > Plugins**
- Plugin commands appear in the native menu under **Tools > Plugins > \<Plugin Name\>**
- Enabled plugins persisted in `state.toml` and restored on restart
- See `docs/plugin-sdk.md` and `docs/plugin-security-model.md`

### Built-in Features (not plugins)
SSH sessions, SFTP browsing, file transfers, SSH tunnels, server management,
`~/.ssh/config` import, host key verification, the credential vault, share
bundles, and the light editor are all built in. The SSH/SFTP/tunnel protocol
core lives in `termlab_remote`; `termlab_tauri/src/remote/` is only the Tauri
command layer over it.

### Key Patterns
- **Tauri v2 webview**: HTML/CSS/JS frontend, Rust backend via commands + events
- **xterm.js**: handles all terminal emulation; backend provides raw byte streams
- **Typed IPC**: Rust types exported to `frontend/types/*.ts` via `ts-rs` — regenerate rather than hand-editing
- **Design tokens**: colors come from `styles/design-system/tokens-{light,dark}.css` and Alacritty themes (`var(--bg)`, etc.) — never hardcode hex
- **Shared JS utilities**: `app/core/utils.js` provides `esc()`, `attr()`, `formatSize()`, `formatDate()` — no duplicating these
- **Toast notifications**: all user-facing messages go through `app/ui/toast.js` — never `alert()`/`confirm()`
- **Dialogs**: use the `tl-dialog` component. The older `ssh-overlay`/`ssh-form` pattern is legacy — don't add to it
- **Auth prompts**: oneshot channels — emit event to frontend, block calling thread on response
- **SSH sessions**: reuse the same `pty-output`/`pty-exit` events as local PTY tabs
- **Transfers**: go through the durable queue in `remote/transfer_queue/`, which survives restarts
- **Plugin menu items**: stored in shared state, native menu rebuilt dynamically after enable
- **State persistence**: window size, panel widths, panel visibility, enabled plugins in `state.toml`
- **Hot-reload**: `watcher.rs` polls config.toml + themes/ every 2s, emits `config-changed` event

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
- Protocol/transport logic belongs in `termlab_remote`; keep `termlab_tauri` to command wiring

### Frontend (JS)
- No bundler and no ES modules: each file is a self-contained IIFE exposing a global (e.g. `window.sshPanel`), loaded by `<script>` tags in `index.html`. Order matters — add new tags after dependencies.
- Use `window.utils.esc()` / `window.utils.attr()` — never define local copies
- Use the global toast system for all notifications — never use `alert()` or `confirm()`
- CSS uses custom properties from the design system — never hardcode hex colors, and keep light and dark tokens in sync
- Build dialogs with `tl-dialog` and the other `tl-*` primitives rather than bespoke markup
- Escape handlers must use capture phase (`addEventListener(..., true)`) to fire before xterm.js
- Icons: use the `tl-icon` component; raw `<img>` PNG references are legacy
- Keep runtime modules small and purpose-specific; if a frontend file starts becoming a grab bag, split it
- Prefer event delegation and shared helpers over duplicating DOM wiring across panels
- Preserve focus behavior and keyboard navigation when changing interactive UI

### Config
- User config: `~/.config/termlab/config.toml` (loaded by termlab_core)
- Persistent state: `~/.config/termlab/state.toml` (window size, plugins, layout)
- SSH server config: `~/.config/termlab/remote/servers.json`
- Themes: `~/.config/termlab/themes/*.toml` (Alacritty format)
- Plugin config: `~/.config/termlab/plugins/{plugin_name}/{key}.json`
- Encrypted vault: `vault.enc` (AES-256-GCM + Argon2id)
- Keyboard shortcuts: configurable in `[termlab.keyboard]`; see `config.example.toml`
- Default shortcuts use `cmd+` prefix (maps to Cmd on macOS, Ctrl on Linux/Windows)
- Some shortcuts (zoom, quick connect) are native menu accelerators in `menu.rs`, not `[termlab.keyboard]` entries

### Testing Standards
- `#[cfg(test)] mod tests` at the bottom of each file
- Test pure logic: parsing, config defaults, widget building
- Use `assert_eq!` with descriptive messages
- Test edge cases: empty input, missing fields, boundary values
- Plugin SDK: test widget serialization/deserialization
- Config: test defaults, serde round-trips, backward compat with `serde(default)`
- Currently ~1,143 Rust tests across 91 files, plus ~59 frontend suites in `scripts/tests/` — keep this growing
- If a change cannot reasonably be covered by an automated test, explain that clearly in the final summary and describe the manual verification performed

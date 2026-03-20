# Conch Mobile (iOS) — SSH Client Design Spec

## Overview

A purpose-built iOS SSH client sharing Conch's Rust backend. Provides SSH terminal sessions, SFTP file browsing with preview, file transfers, and iCloud-synced server configuration. No local PTY. Plugin architecture wired but disabled at launch (Lua-only, future expansion).

**Target platform:** iOS first, Android later.

## Workspace Structure

```
crates/
  conch_core/            — (existing) Config, color schemes, state
  conch_plugin_sdk/      — (existing) Widget/event types
  conch_plugin/          — (existing) Plugin host (Lua only on mobile)
  conch_remote/          — (NEW) Extracted from conch_tauri/src/remote/
  conch_tauri/           — (existing) Desktop app, depends on conch_remote
  conch_mobile/          — (NEW) iOS app crate
```

### Key Principle

Shared Rust backend via `conch_remote`. Each app crate (`conch_tauri`, `conch_mobile`) has its own Tauri config, frontend, and thin command wrappers. Desktop is never modified when working on mobile and vice versa.

## `conch_remote` Crate — Extracted Shared Backend

### Moves to `conch_remote`

| File | Responsibility |
|------|---------------|
| `ssh.rs` | SSH connection, auth, proxy, channel I/O (russh 0.48) |
| `sftp.rs` | SFTP operations (russh-sftp) |
| `config.rs` | Server entries, folders, tunnels, `~/.ssh/config` import |
| `known_hosts.rs` | OpenSSH known_hosts read/write |
| `transfer.rs` | Upload/download engine with progress reporting |
| `tunnel.rs` | SSH tunnel manager (local port forwarding) |

**Note:** `local_fs.rs` stays in `conch_tauri` — it provides the desktop dual-pane local file browser via direct filesystem access, which is not applicable on iOS (mobile uses iOS document picker APIs instead).

### Handler Unification (Critical Refactor)

The current codebase has two separate `russh::client::Handler` implementations:
- `SshHandler` in `ssh.rs` — for interactive sessions
- `TunnelSshHandler` in `tunnel.rs` — for tunnel connections, with duplicated auth logic

During extraction, these must be **unified into a single generic handler** parameterized by `RemoteCallbacks`. Additionally, the duplicated functions (`try_key_auth` / `try_tunnel_key_auth`, `connect_via_proxy` / `connect_tunnel_via_proxy`) must be merged into single implementations.

**Type propagation:** `sftp.rs` and `transfer.rs` currently take `&russh::client::Handle<SshHandler>` on every public function. After unification, these functions must become generic over the handler type (e.g., `fn list_dir<H: client::Handler>(ssh: &client::Handle<H>, ...)`) or use the unified concrete handler type. This ripples through every function signature in both files.

**Key discovery paths:** `try_key_auth` hardcodes `~/.ssh/id_ed25519`, `~/.ssh/id_rsa`, `~/.ssh/id_ecdsa` as fallback key paths. These must be injected or made platform-aware as part of the path abstraction work (no `~/.ssh/` on iOS).

### Path Abstraction (Critical for iOS)

Several modules use hardcoded desktop paths that don't exist on iOS:
- `known_hosts.rs` uses `dirs::home_dir()` to find `~/.ssh/known_hosts` — no such path on iOS
- `config.rs` uses `conch_core::config::config_dir().join("remote")` — resolves to `~/.config/conch/remote/` on desktop

**Solution:** `conch_remote` must accept paths at initialization rather than hardcoding them. Persistence functions take `&Path` parameters, and each app crate passes its platform-appropriate path (desktop: `~/.config/conch/`, iOS: app sandbox or iCloud container).

**Note:** `config.rs` also contains `parse_ssh_config()` which reads `~/.ssh/config` via `dirs::home_dir()`. This is desktop-only (no `~/.ssh/config` on iOS) and should either stay in `conch_tauri` or be gated behind `#[cfg(not(target_os = "ios"))]` in `conch_remote`.

### ProxyCommand Limitation

`ssh.rs` and `tunnel.rs` spawn `sh -lc <proxy_command>` for ProxyCommand support. iOS apps cannot spawn subprocesses. **ProxyCommand and ProxyJump are desktop-only features.** In `conch_remote`, `connect_via_proxy` returns an error on iOS (`#[cfg(target_os = "ios")]`). The mobile frontend hides proxy-related fields in server configuration.

### Stays in App Crates (thin wrappers)

- Tauri `#[command]` functions calling `conch_remote`
- Session registry (managed in Tauri state)
- Auth prompt bridge (frontend-specific event flow)
- Progress event emission (Tauri events)

### Interface Pattern

`conch_remote` exposes an async API with a trait for platform-specific concerns. Uses `#[async_trait]` (already a workspace dependency) for trait-object compatibility:

```rust
#[async_trait]
pub trait RemoteCallbacks: Send + Sync {
    async fn prompt_auth(&self, server: &str, methods: &[AuthMethod]) -> Option<AuthResponse>;
    async fn verify_host_key(&self, host: &str, key: &PublicKey) -> bool;
    fn on_transfer_progress(&self, id: &str, bytes: u64, total: Option<u64>);
}
```

Each app crate provides its own `RemoteCallbacks` implementation that bridges to its frontend via Tauri events.

### Dependency Split

SSH-related dependencies move from `conch_tauri/Cargo.toml` to `conch_remote/Cargo.toml`: `russh`, `russh-keys`, `russh-sftp`, `ssh-key`, `async-trait`, `base64`, `tokio`, `dirs`. `conch_tauri` and `conch_mobile` depend on `conch_remote` instead of listing these individually.

## `conch_mobile` Crate — iOS App

### Crate Structure

```
crates/conch_mobile/
  Cargo.toml              — Tauri v2 (mobile), conch_remote/core/plugin_sdk/plugin
  tauri.conf.json         — iOS bundle ID, points to frontend-mobile/
  build.rs                — Tauri mobile build setup
  src/
    main.rs               — Entry point
    lib.rs                — Tauri setup, command registration, state management
    commands.rs            — Thin #[tauri::command] wrappers
    callbacks.rs           — RemoteCallbacks impl for iOS
    icloud.rs              — iCloud sync for servers.json and config
    state.rs               — App state (sessions, preferences)
  frontend-mobile/
    index.html             — Entry point
    terminal.js            — xterm.js with touch-optimized settings
    keyboard-bar.js        — Accessory bar (Esc, Tab, Ctrl, Alt, arrows, pipe, etc.)
    server-list.js         — Server list (connect, add, edit, delete, folders)
    file-browser.js        — SFTP browser with preview
    file-preview.js        — In-app preview (text, images, PDF, logs)
    transfer-manager.js    — Upload/download UI, iOS Files integration
    quick-connect.js       — Quick connect form
    settings.js            — App settings, theme selection
    utils.js               — Shared utilities
    toast.js               — Toast notifications
    styles/
      main.css             — Mobile-first layout, safe area insets
      terminal.css         — Terminal styles
      theme.css            — CSS custom properties (same theme system as desktop)
  icons/                   — iOS app icons
```

### Dependencies

**Included:** `conch_core`, `conch_remote`, `conch_plugin_sdk`, `conch_plugin` (Lua only), `tauri` (mobile feature)

**Excluded:** `portable-pty`, `arboard`, `jni`, `objc2` (macOS-specific), `libc` (Unix IPC)

## Mobile Frontend UX

### Navigation — Tab-based

| Tab | Purpose |
|-----|---------|
| **Sessions** | Active SSH sessions. Tap to switch, swipe to disconnect. |
| **Servers** | Server list (iCloud-synced), folders, quick connect. |
| **Files** | SFTP browser, visible when connected to a server. |
| **Transfers** | Active/completed uploads and downloads. |

### Terminal Screen

- xterm.js fills viewport above keyboard
- **Accessory bar** between terminal and iOS keyboard: `Esc` `Tab` `Ctrl` `Alt` `|` `/` `~` `-` `arrows`
- Long-press `Ctrl` enters sticky mode (next keypress is Ctrl+X, then unsticks)
- Pinch to zoom adjusts font size
- Two-finger tap for paste, selection for copy
- Swipe left/right to switch between active sessions

**WKWebView note:** Tauri v2 on iOS uses WKWebView. xterm.js must use the **canvas renderer** (not WebGL) — WebGL support in WKWebView is limited. Keyboard event handling in WKWebView differs from desktop and needs thorough testing.

### File Browser

- iOS-style list view for remote files
- Tap file: preview (text, image, PDF, log) or download prompt for unsupported types
- Long-press: context menu (download, rename, delete, info)
- Upload: iOS document picker (`UIDocumentPickerViewController`) — Files app, Photos
- Download: saves to iOS Files app
- Transfer progress: overlay bar + detailed view in Transfers tab

### Server List

- Grouped by folders (matching desktop config)
- Tap to connect, swipe to edit/delete
- "+" button for new server or quick connect
- Auth: password prompt, key selection from iOS keychain or imported files, agent forwarding

### Theming

- Same Alacritty `.toml` theme format as desktop
- CSS custom properties applied identically
- Ships with built-in themes; custom themes sync via iCloud

## iCloud Sync

### What Syncs

- `servers.json` — server entries, folders, tunnels
- Theme files (`.toml`)
- App preferences (optional)

### Mechanism

- `icloud.rs` reads/writes to the app's iCloud container
- Desktop writes config to iCloud Drive (or user copies manually)
- On mobile launch: read from iCloud container, merge with local changes
- Conflict resolution: entry-level merge (diff individual `ServerEntry` objects by unique ID). When both devices modify the same entry, last-write-wins by timestamp. When both devices add new entries, both are preserved. Requires adding a `uuid` and `updated_at` field to `ServerEntry`. Full iCloud sync design is deferred to Phase 5.

## Plugin Architecture

- `conch_plugin_sdk` and `conch_plugin` (Lua host) are compiled into the mobile crate
- Plugin loading infrastructure is wired but **not exposed in the UI**
- Java plugins are excluded entirely (no JNI dependency)
- Lua plugin SDK will be expanded later for mobile-specific APIs
- Plugin Manager UI is absent from the mobile frontend at launch

**`conch_plugin` feature gating:** The `jni` dependency in `conch_plugin/Cargo.toml` is currently unconditional. Must be gated behind a cargo feature flag so mobile can depend on `conch_plugin` without pulling in JNI:

```toml
[features]
default = ["java"]
java = ["dep:jni"]
```

The existing `cfg(java_sdk_available)` mechanism in `conch_plugin/src/lib.rs` should be replaced with `cfg(feature = "java")` to consolidate gating into a single system.

`conch_mobile` depends on `conch_plugin` with `default-features = false`. `conch_tauri` continues using default features (Java + Lua).

## iOS Platform Constraints

### Background Execution

iOS aggressively suspends background apps. SSH connections will be dropped when the user switches away from Conch. Strategy:

- **Phase 1 (launch):** Auto-reconnect on foreground resume. Show a "Reconnecting..." indicator. Session history is lost (standard for most iOS SSH clients).
- **Phase 2 (future):** Explore `BGTaskScheduler` for brief keepalive pings, or mosh protocol support for seamless reconnection.

### SSH Key Management

SSH keys on iOS need explicit import paths since there is no `~/.ssh/` directory:

- Import `.pem` / private key files via iOS Files app (share sheet / document picker)
- Keys stored in the iOS Keychain (encrypted at rest)
- Generate keys on-device (Ed25519/RSA)
- Sync public keys via iCloud (private keys stay in Keychain per device)
- Detailed key management UX is deferred to its own design — the initial launch supports password auth and imported key files.

## Build & CI

### Local Development

```bash
# Mobile
cd crates/conch_mobile && cargo tauri ios dev    # iOS Simulator
cd crates/conch_mobile && cargo tauri ios build   # Release .ipa

# Desktop (unchanged)
cd crates/conch_tauri && cargo tauri dev
```

### CI

- New `ci-mobile.yml` workflow, separate from desktop CI
- Runs on macOS runner (Xcode/iOS SDK required)
- Steps: install Rust, install Tauri CLI, `cargo test` on `conch_remote`, `cargo tauri ios build`
- App Store submission manual initially (Xcode upload)

### Workspace Cargo.toml

```toml
members = [
    "crates/conch_core",
    "crates/conch_plugin_sdk",
    "crates/conch_plugin",
    "crates/conch_remote",
    "crates/conch_tauri",
    "crates/conch_mobile",
]
```

## Phased Delivery

1. **Extract `conch_remote`** — Refactor out of `conch_tauri/src/remote/`, no behavior change, all desktop tests pass
2. **Scaffold `conch_mobile`** — Bare Tauri iOS app that launches in Simulator
3. **Wire SSH + terminal** — First working SSH session via `conch_remote` + xterm.js
4. **Server list + file browser + transfers** — Core SFTP functionality, iOS Files integration
5. **iCloud sync** — Shared server config between desktop and mobile
6. **Polish UX** — Accessory bar, file preview, themes, gesture navigation, session switching

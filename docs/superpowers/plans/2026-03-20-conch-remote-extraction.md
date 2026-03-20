# Conch Remote Extraction — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Extract SSH/SFTP/tunnel/transfer logic from `conch_tauri` into a new `conch_remote` crate with a platform-agnostic API, enabling both desktop and future mobile apps to share the same backend.

**Architecture:** Create `crates/conch_remote/` containing all SSH connection, SFTP, transfer, tunnel, config, and known_hosts logic. Unify the duplicated `SshHandler`/`TunnelSshHandler` into a single handler backed by a `RemoteCallbacks` trait. Replace hardcoded paths with injectable parameters. `conch_tauri` becomes a thin wrapper that implements `RemoteCallbacks` via Tauri events and delegates to `conch_remote`.

**Tech Stack:** Rust (edition 2024), russh 0.48, russh-sftp 2.1, ssh-key 0.6, async-trait 0.1, tokio 1, serde 1

**Spec:** `docs/superpowers/specs/2026-03-20-mobile-ios-ssh-client-design.md`

**Baseline:** 218 tests passing across workspace (`cargo test --workspace`). This number must not decrease.

---

## File Map

### New files (conch_remote)

| File | Responsibility |
|------|---------------|
| `crates/conch_remote/Cargo.toml` | Crate manifest with SSH dependencies moved from conch_tauri |
| `crates/conch_remote/src/lib.rs` | Public API surface — re-exports all modules |
| `crates/conch_remote/src/callbacks.rs` | `RemoteCallbacks` trait + `RemotePaths` config struct |
| `crates/conch_remote/src/handler.rs` | Unified `ConchSshHandler` implementing `russh::client::Handler` |
| `crates/conch_remote/src/ssh.rs` | Connection, auth, channel loop, exec — moved from conch_tauri |
| `crates/conch_remote/src/sftp.rs` | SFTP operations — moved from conch_tauri |
| `crates/conch_remote/src/config.rs` | Server config types + path-parameterized persistence — moved from conch_tauri |
| `crates/conch_remote/src/known_hosts.rs` | Known hosts with path parameter — moved from conch_tauri |
| `crates/conch_remote/src/transfer.rs` | File transfer engine — moved from conch_tauri |
| `crates/conch_remote/src/tunnel.rs` | Tunnel manager using unified handler — moved from conch_tauri |

### Modified files (conch_tauri)

| File | Change |
|------|--------|
| `crates/conch_tauri/Cargo.toml` | Remove direct SSH deps (russh-keys, russh-sftp, ssh-key), add `conch_remote`. Keep `russh` (mod.rs uses `Handle`/`Channel` directly), keep `async-trait` (for TauriRemoteCallbacks impl). |
| `crates/conch_tauri/src/remote/mod.rs` | Thin wrappers: implement `RemoteCallbacks`, delegate commands to `conch_remote`. All ~30 Tauri commands updated. |
| `crates/conch_tauri/src/remote/local_fs.rs` | Update import: `use conch_remote::sftp::FileEntry` (was `use super::sftp::FileEntry`) |

### Deleted files (conch_tauri)

| File | Reason |
|------|--------|
| `crates/conch_tauri/src/remote/ssh.rs` | Moved to conch_remote |
| `crates/conch_tauri/src/remote/sftp.rs` | Moved to conch_remote |
| `crates/conch_tauri/src/remote/config.rs` | Moved to conch_remote |
| `crates/conch_tauri/src/remote/known_hosts.rs` | Moved to conch_remote |
| `crates/conch_tauri/src/remote/transfer.rs` | Moved to conch_remote |
| `crates/conch_tauri/src/remote/tunnel.rs` | Moved to conch_remote |

### Modified files (conch_plugin)

| File | Change |
|------|--------|
| `crates/conch_plugin/Cargo.toml` | Add `java` feature flag, gate `jni` dependency |
| `crates/conch_plugin/src/lib.rs` | Replace `cfg(java_sdk_available)` with `cfg(feature = "java")` |

### Modified files (workspace)

| File | Change |
|------|--------|
| `Cargo.toml` | Add `conch_remote` to workspace members + dependencies |

---

## Task 1: Create `conch_remote` crate scaffold

**Files:**
- Create: `crates/conch_remote/Cargo.toml`
- Create: `crates/conch_remote/src/lib.rs`
- Modify: `Cargo.toml` (workspace root)

- [ ] **Step 1: Create Cargo.toml**

```toml
[package]
name = "conch_remote"
version.workspace = true
edition.workspace = true
license.workspace = true
description = "Platform-agnostic SSH, SFTP, and tunnel operations for Conch"

[dependencies]
serde = { workspace = true }
serde_json = { workspace = true }
tokio = { workspace = true }
log = { workspace = true }
parking_lot = { workspace = true }
uuid = { workspace = true }
dirs = { workspace = true }
base64 = { workspace = true }
async-trait = "0.1"
russh = "0.48"
russh-keys = "0.48"
russh-sftp = "2.1"
ssh-key = { version = "0.6", features = ["ed25519", "rsa", "p256"] }

[dev-dependencies]
tempfile = "3"
```

- [ ] **Step 2: Create lib.rs stub**

```rust
//! Platform-agnostic SSH, SFTP, and tunnel operations for Conch.
//!
//! This crate provides the core remote connectivity logic shared by
//! the desktop app (`conch_tauri`) and mobile app (`conch_mobile`).

pub mod callbacks;

// Re-export russh types used by app crates (Handle, Channel, ChannelMsg).
// App crates reference these when storing session handles and running channel loops.
pub use russh;
```

- [ ] **Step 3: Create callbacks.rs stub**

```rust
//! Platform callback trait — implemented by each app crate.
```

- [ ] **Step 4: Add to workspace Cargo.toml**

Add `"crates/conch_remote"` to the `members` list and add `conch_remote = { path = "crates/conch_remote" }` to `[workspace.dependencies]`.

- [ ] **Step 5: Verify workspace compiles**

Run: `cargo check --workspace`
Expected: compiles with no errors

- [ ] **Step 6: Commit**

```bash
git add crates/conch_remote/ Cargo.toml
git commit -m "Add conch_remote crate scaffold"
```

---

## Task 2: Define `RemoteCallbacks` trait and `RemotePaths`

**Files:**
- Modify: `crates/conch_remote/src/callbacks.rs`
- Test: `crates/conch_remote/src/callbacks.rs` (inline `#[cfg(test)]`)

- [ ] **Step 1: Write the tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn remote_paths_default_key_paths() {
        let paths = RemotePaths {
            known_hosts_file: PathBuf::from("/tmp/known_hosts"),
            config_dir: PathBuf::from("/tmp/config"),
            default_key_paths: vec![
                PathBuf::from("/tmp/keys/id_ed25519"),
                PathBuf::from("/tmp/keys/id_rsa"),
            ],
        };
        assert_eq!(paths.default_key_paths.len(), 2);
        assert_eq!(paths.known_hosts_file, PathBuf::from("/tmp/known_hosts"));
    }

    #[test]
    fn remote_paths_empty_keys() {
        let paths = RemotePaths {
            known_hosts_file: PathBuf::from("/tmp/known_hosts"),
            config_dir: PathBuf::from("/tmp/config"),
            default_key_paths: vec![],
        };
        assert!(paths.default_key_paths.is_empty());
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p conch_remote`
Expected: FAIL — `RemotePaths` not defined

- [ ] **Step 3: Write the trait and struct**

```rust
//! Platform callback trait and path configuration.
//!
//! Each app crate (desktop, mobile) provides its own implementation.

use std::path::PathBuf;

/// Platform-specific paths injected at initialization.
///
/// Desktop uses `~/.ssh/known_hosts`, `~/.config/conch/remote/`, `~/.ssh/id_*`.
/// iOS uses app sandbox / iCloud container paths.
#[derive(Debug, Clone)]
pub struct RemotePaths {
    /// Path to the known_hosts file (e.g., `~/.ssh/known_hosts`).
    pub known_hosts_file: PathBuf,
    /// Directory for server config persistence (e.g., `~/.config/conch/remote/`).
    pub config_dir: PathBuf,
    /// Default SSH key file paths to try during key auth.
    /// Empty on iOS (keys come from Keychain).
    pub default_key_paths: Vec<PathBuf>,
}

/// Callbacks for platform-specific user interaction during SSH operations.
///
/// Implemented by each app crate to bridge prompts to their UI framework.
#[async_trait::async_trait]
pub trait RemoteCallbacks: Send + Sync {
    /// Prompt the user to accept or reject a host key.
    ///
    /// `message` is a human-readable description (e.g., "The authenticity of host X
    /// can't be established."). `fingerprint` is the key fingerprint string.
    /// Returns `true` if the user accepts.
    async fn verify_host_key(&self, message: &str, fingerprint: &str) -> bool;

    /// Prompt the user for a password.
    ///
    /// `message` is a human-readable description (e.g., "Password for user@host").
    /// Returns `Some(password)` or `None` if cancelled.
    async fn prompt_password(&self, message: &str) -> Option<String>;

    /// Report file transfer progress. Called periodically during uploads/downloads.
    fn on_transfer_progress(&self, transfer_id: &str, bytes: u64, total: Option<u64>);
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p conch_remote`
Expected: 2 passed

- [ ] **Step 5: Commit**

```bash
git add crates/conch_remote/src/callbacks.rs
git commit -m "Define RemoteCallbacks trait and RemotePaths config"
```

---

## Task 3: Move and adapt `known_hosts.rs`

**Files:**
- Create: `crates/conch_remote/src/known_hosts.rs`
- Modify: `crates/conch_remote/src/lib.rs`

This module is the simplest to extract — it has no dependencies on other remote modules. The key change: replace `known_hosts_path()` (which uses `dirs::home_dir()`) with an explicit `&Path` parameter.

- [ ] **Step 1: Write the tests**

Copy the existing 4 tests from `crates/conch_tauri/src/remote/known_hosts.rs:106-133` into the new file. The 4 existing tests cover helper functions (`host_key`, `key_data_from_openssh`). Add 1 new test for the default path helper:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    // --- existing tests (copied) ---

    #[test]
    fn host_key_standard_port() {
        assert_eq!(host_key("example.com", 22), "example.com");
    }

    #[test]
    fn host_key_custom_port() {
        assert_eq!(host_key("example.com", 2222), "[example.com]:2222");
    }

    #[test]
    fn key_data_strips_comment() {
        let input = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAITest user@host";
        let data = key_data_from_openssh(input);
        assert_eq!(data, "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAITest");
    }

    #[test]
    fn key_data_no_comment() {
        let input = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAITest";
        let data = key_data_from_openssh(input);
        assert_eq!(data, "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAITest");
    }

    // --- new test ---

    #[test]
    fn default_known_hosts_path_is_under_ssh_dir() {
        if let Some(path) = default_known_hosts_path() {
            assert!(path.ends_with(".ssh/known_hosts"));
        }
    }
}
```

- [ ] **Step 2: Write the adapted module**

Copy `crates/conch_tauri/src/remote/known_hosts.rs` to `crates/conch_remote/src/known_hosts.rs` with these changes:

1. Replace `fn known_hosts_path() -> Option<PathBuf>` with `pub fn default_known_hosts_path() -> Option<PathBuf>` (same body, but public for app crates to call).

2. Change `check_known_host` to accept a `&Path`:
```rust
pub fn check_known_host(
    known_hosts_file: &Path,
    host: &str,
    port: u16,
    server_key: &ssh_key::PublicKey,
) -> Option<bool> {
    let contents = fs::read_to_string(known_hosts_file).ok()?;
    // ... rest unchanged
}
```

3. Change `add_known_host` to accept a `&Path`:
```rust
pub fn add_known_host(
    known_hosts_file: &Path,
    host: &str,
    port: u16,
    server_key: &ssh_key::PublicKey,
) -> Result<(), String> {
    if let Some(parent) = known_hosts_file.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("cannot create dir: {e}"))?;
    }
    // ... rest unchanged, using known_hosts_file instead of path
}
```

4. Make `host_key` and `key_data_from_openssh` `pub(crate)` (needed by handler.rs).

- [ ] **Step 3: Add module to lib.rs**

```rust
pub mod callbacks;
pub mod known_hosts;
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p conch_remote`
Expected: all pass (existing 4 + any new tests)

- [ ] **Step 5: Verify workspace still compiles**

Run: `cargo check --workspace`
Expected: compiles (conch_tauri still has its own copy)

- [ ] **Step 6: Commit**

```bash
git add crates/conch_remote/src/known_hosts.rs crates/conch_remote/src/lib.rs
git commit -m "Add known_hosts module to conch_remote with path parameterization"
```

---

## Task 4: Move and adapt `config.rs`

**Files:**
- Create: `crates/conch_remote/src/config.rs`
- Modify: `crates/conch_remote/src/lib.rs`

The config module contains types (`ServerEntry`, `ServerFolder`, `SshConfig`, `SavedTunnel`, `ExportPayload`) and persistence (`load_config`, `save_config`). Types move as-is. Persistence gets a `&Path` parameter. `parse_ssh_config()` is gated behind `#[cfg(not(target_os = "ios"))]`.

- [ ] **Step 1: Copy and adapt the module**

Copy `crates/conch_tauri/src/remote/config.rs` to `crates/conch_remote/src/config.rs` with these changes:

1. Remove `use conch_core::config::config_dir;` — no longer needed.

2. Replace hardcoded path functions:
```rust
// OLD:
fn config_dir() -> PathBuf {
    conch_core::config::config_dir().join("remote")
}
fn config_path() -> PathBuf {
    config_dir().join("servers.json")
}
pub fn load_config() -> SshConfig { ... }
pub fn save_config(config: &SshConfig) { ... }

// NEW:
pub fn load_config(config_dir: &Path) -> SshConfig {
    let path = config_dir.join("servers.json");
    match fs::read_to_string(&path) {
        Ok(contents) => serde_json::from_str(&contents).unwrap_or_default(),
        Err(_) => SshConfig::default(),
    }
}

pub fn save_config(config_dir: &Path, config: &SshConfig) {
    let _ = fs::create_dir_all(config_dir);
    let path = config_dir.join("servers.json");
    if let Ok(json) = serde_json::to_string_pretty(config) {
        let _ = fs::write(path, json);
    }
}
```

3. Gate `parse_ssh_config` and related code for desktop only. **Important:** The 3 tests that use `parse_ssh_config_str` (`parse_ssh_config_basic`, `parse_ssh_config_skip_wildcard`, `parse_ssh_config_proxy_jump`) must also be gated:
```rust
/// Parse `~/.ssh/config` into server entries. Desktop-only — iOS has no `~/.ssh/`.
#[cfg(not(target_os = "ios"))]
pub fn parse_ssh_config() -> Vec<ServerEntry> {
    // ... unchanged
}

#[cfg(not(target_os = "ios"))]
fn parse_ssh_config_str(contents: &str) -> Vec<ServerEntry> {
    // ... unchanged
}

#[cfg(not(target_os = "ios"))]
struct PartialEntry { ... }

#[cfg(not(target_os = "ios"))]
impl PartialEntry { ... }

// In the test module:
#[cfg(not(target_os = "ios"))]
#[test]
fn parse_ssh_config_basic() { ... }
// etc.
```

4. All types become `pub` (they were already `pub` but accessed via `pub(crate)` in mod.rs).

- [ ] **Step 2: Copy all existing tests**

Copy the test module from `crates/conch_tauri/src/remote/config.rs:435-597`. The type tests (`default_config_is_empty`, `add_server_appends`, `find_server_*`, `remove_server_*`, `serde_roundtrip`, `all_servers_iterates`, `saved_tunnel_*`, `parse_ssh_config_*`) all work as-is since they test pure logic.

Add a new test for path-parameterized persistence:

```rust
#[test]
fn load_config_from_path_missing_dir() {
    let cfg = load_config(&PathBuf::from("/nonexistent/dir"));
    assert!(cfg.folders.is_empty());
    assert!(cfg.ungrouped.is_empty());
}

#[test]
fn save_and_load_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = SshConfig::default();
    cfg.add_server(make_entry("s1", "host1"));
    save_config(dir.path(), &cfg);
    let loaded = load_config(dir.path());
    assert_eq!(loaded.ungrouped.len(), 1);
    assert_eq!(loaded.ungrouped[0].host, "host1");
}
```

Note: Add `tempfile = "3"` to `conch_remote/Cargo.toml` under `[dev-dependencies]`.

- [ ] **Step 3: Add module to lib.rs**

```rust
pub mod callbacks;
pub mod config;
pub mod known_hosts;
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p conch_remote`
Expected: all pass (existing config tests + new persistence tests)

- [ ] **Step 5: Commit**

```bash
git add crates/conch_remote/src/config.rs crates/conch_remote/src/lib.rs crates/conch_remote/Cargo.toml
git commit -m "Add config module to conch_remote with path-parameterized persistence"
```

---

## Task 5: Create unified SSH handler

**Files:**
- Create: `crates/conch_remote/src/handler.rs`
- Modify: `crates/conch_remote/src/lib.rs`

This unifies `SshHandler` (ssh.rs:52-133) and `TunnelSshHandler` (tunnel.rs:59-131) into a single `ConchSshHandler` that delegates to `RemoteCallbacks`.

- [ ] **Step 1: Write the handler**

```rust
//! Unified SSH client handler — delegates host key verification to RemoteCallbacks.

use std::path::PathBuf;
use std::sync::Arc;

use russh::client;

use crate::callbacks::RemoteCallbacks;
use crate::known_hosts;

/// Unified SSH handler for both interactive sessions and tunnels.
///
/// Implements `russh::client::Handler` and delegates host key verification
/// to the provided `RemoteCallbacks` implementation.
pub struct ConchSshHandler {
    pub host: String,
    pub port: u16,
    pub known_hosts_file: PathBuf,
    pub callbacks: Arc<dyn RemoteCallbacks>,
}

#[async_trait::async_trait]
impl client::Handler for ConchSshHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &ssh_key::PublicKey,
    ) -> Result<bool, Self::Error> {
        let check = known_hosts::check_known_host(
            &self.known_hosts_file,
            &self.host,
            self.port,
            server_public_key,
        );

        match check {
            Some(true) => {
                log::debug!("Host key for {}:{} matches known_hosts", self.host, self.port);
                return Ok(true);
            }
            Some(false) => {
                let fingerprint = server_public_key.fingerprint(ssh_key::HashAlg::Sha256);
                let message = format!(
                    "WARNING: HOST KEY HAS CHANGED for {}:{}\n\n\
                     This could indicate a man-in-the-middle attack.\n\
                     It is also possible that the host key has just been changed.",
                    self.host, self.port
                );
                let fp_str = format!(
                    "{}\n{fingerprint}",
                    server_public_key.algorithm().as_str(),
                );
                let accepted = self.callbacks.verify_host_key(&message, &fp_str).await;
                return Ok(accepted);
            }
            None => {
                // Unknown host — ask the user.
            }
        }

        let fingerprint = server_public_key.fingerprint(ssh_key::HashAlg::Sha256);
        let host_display = if self.port != 22 {
            format!("[{}]:{}", self.host, self.port)
        } else {
            self.host.clone()
        };
        let message = format!("The authenticity of host '{host_display}' can't be established.");
        let fp_str = format!(
            "{} key fingerprint is:\n{fingerprint}",
            server_public_key.algorithm().as_str(),
        );

        let accepted = self.callbacks.verify_host_key(&message, &fp_str).await;

        if accepted {
            if let Err(e) = known_hosts::add_known_host(
                &self.known_hosts_file,
                &self.host,
                self.port,
                server_public_key,
            ) {
                log::warn!("Failed to save host key: {e}");
            }
        }

        Ok(accepted)
    }
}
```

- [ ] **Step 2: Add module to lib.rs**

```rust
pub mod callbacks;
pub mod config;
pub mod handler;
pub mod known_hosts;
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo check -p conch_remote`
Expected: compiles

- [ ] **Step 4: Commit**

```bash
git add crates/conch_remote/src/handler.rs crates/conch_remote/src/lib.rs
git commit -m "Add unified ConchSshHandler delegating to RemoteCallbacks"
```

---

## Task 6: Move and adapt `ssh.rs`

**Files:**
- Create: `crates/conch_remote/src/ssh.rs`
- Modify: `crates/conch_remote/src/lib.rs`

The SSH module provides connection establishment, authentication, channel I/O, and exec. Key changes: use `ConchSshHandler` instead of `SshHandler`, inject key paths via `RemotePaths`, unify `try_key_auth`/`connect_via_proxy`, and gate proxy behind `cfg`.

- [ ] **Step 1: Write the adapted module**

Copy `crates/conch_tauri/src/remote/ssh.rs` with these changes:

1. **Remove `SshHandler` and `AuthPrompt`** — replaced by `ConchSshHandler` + `RemoteCallbacks`.

2. **Update `connect_and_open_shell` signature:**
```rust
use crate::callbacks::{RemoteCallbacks, RemotePaths};
use crate::config::ServerEntry;
use crate::handler::ConchSshHandler;

pub async fn connect_and_open_shell(
    server: &ServerEntry,
    password: Option<String>,
    callbacks: Arc<dyn RemoteCallbacks>,
    paths: &RemotePaths,
) -> Result<(client::Handle<ConchSshHandler>, russh::Channel<russh::client::Msg>), String> {
    let config = Arc::new(client::Config::default());
    let handler = ConchSshHandler {
        host: server.host.clone(),
        port: server.port,
        known_hosts_file: paths.known_hosts_file.clone(),
        callbacks: Arc::clone(&callbacks),
    };
    // ... proxy + connect logic (see step 2)
    // ... auth logic using callbacks.prompt_password() instead of AuthPrompt
    // ... open shell channel (unchanged)
}
```

3. **Unify `try_key_auth`** — make it generic via the concrete handler type:
```rust
pub(crate) async fn try_key_auth(
    session: &mut client::Handle<ConchSshHandler>,
    user: &str,
    explicit_key_path: Option<&str>,
    default_key_paths: &[PathBuf],
) -> Result<bool, String> {
    let key_paths: Vec<PathBuf> = if let Some(path) = explicit_key_path {
        vec![expand_tilde(path)]
    } else {
        default_key_paths.to_vec()
    };
    // ... rest unchanged
}
```

4. **Unify `connect_via_proxy`** (must be `pub(crate)` so tunnel.rs can call it):
```rust
#[cfg(not(target_os = "ios"))]
pub(crate) async fn connect_via_proxy(
    proxy_cmd: &str,
    host: &str,
    port: u16,
    config: Arc<client::Config>,
    handler: ConchSshHandler,
) -> Result<client::Handle<ConchSshHandler>, String> {
    // ... same body as current ssh.rs version
}
```

5. **Auth flow now uses callbacks** (and `try_key_auth` passes `paths.default_key_paths`):
```rust
// Password auth:
let pw = match &password {
    Some(pw) => Some(pw.clone()),
    None => {
        let msg = format!("Password for {}@{}", server.user, server.host);
        callbacks.prompt_password(&msg).await
    }
};
// ...
// Key auth:
try_key_auth(&mut session, &server.user, server.key_path.as_deref(), &paths.default_key_paths).await?
```

6. **Update `exec` to use `ConchSshHandler`:**
```rust
pub async fn exec(
    ssh_handle: &client::Handle<ConchSshHandler>,
    command: &str,
) -> Result<(String, String, u32), String> {
    // ... body unchanged
}
```

7. **Keep `ChannelInput`, `channel_loop`, `expand_tilde`** — these are handler-agnostic.

- [ ] **Step 2: Copy existing tests**

The 3 `expand_tilde` tests copy as-is.

- [ ] **Step 3: Add module to lib.rs**

```rust
pub mod callbacks;
pub mod config;
pub mod handler;
pub mod known_hosts;
pub mod ssh;
```

- [ ] **Step 4: Verify it compiles**

Run: `cargo check -p conch_remote`
Expected: compiles

- [ ] **Step 5: Commit**

```bash
git add crates/conch_remote/src/ssh.rs crates/conch_remote/src/lib.rs
git commit -m "Add ssh module to conch_remote with unified handler and path injection"
```

---

## Task 7: Move and adapt `sftp.rs`

**Files:**
- Create: `crates/conch_remote/src/sftp.rs`
- Modify: `crates/conch_remote/src/lib.rs`

Key change: replace `Handle<SshHandler>` with `Handle<ConchSshHandler>` in all function signatures.

- [ ] **Step 1: Copy and adapt**

Copy `crates/conch_tauri/src/remote/sftp.rs`. Change every occurrence of:
```rust
use super::ssh::SshHandler;
// ... and all ...
ssh: &russh::client::Handle<SshHandler>
```
to:
```rust
use crate::handler::ConchSshHandler;
// ... and all ...
ssh: &russh::client::Handle<ConchSshHandler>
```

This is a mechanical find-and-replace. The `FileEntry`, `ReadFileResult` types, `open_sftp`, and all SFTP functions are unchanged except for the handler type parameter.

- [ ] **Step 2: Copy existing tests**

The 2 serialization tests (`file_entry_serializes`, `file_entry_dir_serializes`) copy as-is.

- [ ] **Step 3: Add module to lib.rs**

- [ ] **Step 4: Verify it compiles**

Run: `cargo check -p conch_remote`
Expected: compiles

- [ ] **Step 5: Commit**

```bash
git add crates/conch_remote/src/sftp.rs crates/conch_remote/src/lib.rs
git commit -m "Add sftp module to conch_remote"
```

---

## Task 8: Move and adapt `transfer.rs`

**Files:**
- Create: `crates/conch_remote/src/transfer.rs`
- Modify: `crates/conch_remote/src/lib.rs`

Key change: replace `Handle<SshHandler>` with `Handle<ConchSshHandler>`. Also replace local `open_sftp` with import from `crate::sftp`.

- [ ] **Step 1: Copy and adapt**

Copy `crates/conch_tauri/src/remote/transfer.rs`. Changes:

1. Replace `use super::ssh::SshHandler;` with `use crate::handler::ConchSshHandler;`
2. Replace all `Handle<SshHandler>` with `Handle<ConchSshHandler>`
3. Replace `use super::sftp;` with `use crate::sftp;`
4. Replace `open_sftp` function body to call `crate::sftp::open_sftp` (make `open_sftp` pub(crate) in sftp.rs), or duplicate the helper — it's small. Simplest: make `open_sftp` `pub(crate)` in sftp.rs, then in transfer.rs replace the local `open_sftp` with `crate::sftp::open_sftp`.

- [ ] **Step 2: Copy existing tests**

The 3 tests (`transfer_progress_serializes`, `transfer_registry_new_is_empty`, `transfer_registry_cancel_nonexistent`) copy as-is.

- [ ] **Step 3: Add module to lib.rs, make sftp::open_sftp pub(crate)**

- [ ] **Step 4: Verify it compiles**

Run: `cargo check -p conch_remote`
Expected: compiles

- [ ] **Step 5: Commit**

```bash
git add crates/conch_remote/src/transfer.rs crates/conch_remote/src/sftp.rs crates/conch_remote/src/lib.rs
git commit -m "Add transfer module to conch_remote"
```

---

## Task 9: Move and adapt `tunnel.rs`

**Files:**
- Create: `crates/conch_remote/src/tunnel.rs`
- Modify: `crates/conch_remote/src/lib.rs`

This is where the duplication gets eliminated. Key changes:
- Remove `TunnelSshHandler` entirely — use `ConchSshHandler`
- Remove `TunnelPrompt` enum — use `RemoteCallbacks`
- Remove `try_tunnel_key_auth` — use unified `ssh::try_key_auth`
- Remove `connect_tunnel_via_proxy` — use unified `ssh::connect_via_proxy`
- `connect_for_tunnel` uses `ConchSshHandler` + `RemoteCallbacks`
- `TunnelManager::start_tunnel` takes `callbacks: Arc<dyn RemoteCallbacks>` and `paths: &RemotePaths`

- [ ] **Step 1: Write the adapted module**

Copy `crates/conch_tauri/src/remote/tunnel.rs` with these changes:

1. **Remove** `TunnelSshHandler`, `TunnelPrompt`, `try_tunnel_key_auth`, `connect_tunnel_via_proxy`.

2. **Update `connect_for_tunnel`:**
```rust
use crate::callbacks::{RemoteCallbacks, RemotePaths};
use crate::config::ServerEntry;
use crate::handler::ConchSshHandler;
use crate::ssh;

async fn connect_for_tunnel(
    server: &ServerEntry,
    callbacks: Arc<dyn RemoteCallbacks>,
    paths: &RemotePaths,
) -> Result<client::Handle<ConchSshHandler>, String> {
    let config = Arc::new(client::Config::default());
    let handler = ConchSshHandler {
        host: server.host.clone(),
        port: server.port,
        known_hosts_file: paths.known_hosts_file.clone(),
        callbacks: Arc::clone(&callbacks),
    };

    // Proxy logic
    #[cfg(not(target_os = "ios"))]
    let effective_proxy = server.proxy_command.clone()
        .or_else(|| server.proxy_jump.as_ref().map(|j| format!("ssh -W %h:%p {j}")));
    #[cfg(target_os = "ios")]
    let effective_proxy: Option<String> = None;

    let mut session = if let Some(proxy_cmd) = &effective_proxy {
        #[cfg(not(target_os = "ios"))]
        { ssh::connect_via_proxy(proxy_cmd, &server.host, server.port, config, handler).await? }
        #[cfg(target_os = "ios")]
        { unreachable!() }
    } else {
        let addr = format!("{}:{}", server.host, server.port);
        client::connect(config, &addr, handler)
            .await
            .map_err(|e| format!("Connection failed: {e}"))?
    };

    // Auth
    let authenticated = if server.auth_method == "password" {
        let msg = format!("Password for {}@{}:{}", server.user, server.host, server.port);
        let password = callbacks.prompt_password(&msg).await
            .ok_or_else(|| "Password prompt cancelled".to_string())?;
        session.authenticate_password(&server.user, &password)
            .await
            .map_err(|e| format!("Auth failed: {e}"))?
    } else {
        ssh::try_key_auth(
            &mut session,
            &server.user,
            server.key_path.as_deref(),
            &paths.default_key_paths,
        ).await?
    };

    if !authenticated {
        return Err(format!("Authentication failed for {}@{}", server.user, server.host));
    }

    Ok(session)
}
```

3. **Update `TunnelManager::start_tunnel` signature:**
```rust
pub async fn start_tunnel(
    &self,
    id: Uuid,
    server: &ServerEntry,
    local_port: u16,
    remote_host: String,
    remote_port: u16,
    callbacks: Arc<dyn RemoteCallbacks>,
    paths: &RemotePaths,
) -> Result<(), String> {
    // ... bind local port (unchanged)
    let ssh_handle = connect_for_tunnel(server, callbacks, paths).await?;
    // ... rest unchanged
}
```

- [ ] **Step 2: Copy existing tests**

The 6 tunnel tests (`tunnel_manager_lifecycle`, `tunnel_manager_stop_nonexistent`, `tunnel_manager_stop_all_empty`, `tunnel_status_transitions`, `tunnel_status_serializes`, `tunnel_info_serializes`) copy as-is.

- [ ] **Step 3: Add module to lib.rs**

- [ ] **Step 4: Verify it compiles**

Run: `cargo check -p conch_remote`
Expected: compiles

- [ ] **Step 5: Run all conch_remote tests**

Run: `cargo test -p conch_remote`
Expected: all pass

- [ ] **Step 6: Commit**

```bash
git add crates/conch_remote/src/tunnel.rs crates/conch_remote/src/lib.rs
git commit -m "Add tunnel module to conch_remote, eliminate handler duplication"
```

---

## Task 10a: Update `conch_tauri/Cargo.toml` and add foundation

**Files:**
- Modify: `crates/conch_tauri/Cargo.toml`

- [ ] **Step 1: Update Cargo.toml**

Add `conch_remote`, remove deps that are now transitive via `conch_remote`:

```toml
# Remove these lines (now provided transitively via conch_remote):
# russh-keys = "0.48"
# russh-sftp = "2.1"
# ssh-key = { ... }
# base64 = "0.22"

# KEEP these (still used directly in mod.rs):
# russh = "0.48"        — mod.rs uses Handle<_>, Channel<_>, ChannelMsg directly
# async-trait = "0.1"   — needed for TauriRemoteCallbacks impl

# Add:
conch_remote = { workspace = true }
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo check -p conch_tauri`
Expected: compiles (old modules still exist alongside new conch_remote dep)

- [ ] **Step 3: Commit**

```bash
git add crates/conch_tauri/Cargo.toml
git commit -m "Add conch_remote dependency to conch_tauri"
```

---

## Task 10b: Implement `TauriRemoteCallbacks` + `RemotePaths` + update state types

**Files:**
- Modify: `crates/conch_tauri/src/remote/mod.rs`

At this point old modules still exist. We're adding new code alongside them. Once everything compiles, we'll switch over.

- [ ] **Step 1: Add TauriRemoteCallbacks**

```rust
use conch_remote::callbacks::{RemoteCallbacks, RemotePaths};

/// Bridges RemoteCallbacks to Tauri events for the desktop app.
pub(crate) struct TauriRemoteCallbacks {
    pub app: tauri::AppHandle,
    pub pending_prompts: Arc<Mutex<PendingPrompts>>,
}

#[async_trait::async_trait]
impl RemoteCallbacks for TauriRemoteCallbacks {
    async fn verify_host_key(&self, message: &str, fingerprint: &str) -> bool {
        let prompt_id = uuid::Uuid::new_v4().to_string();
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.pending_prompts.lock().host_key.insert(prompt_id.clone(), tx);
        let _ = self.app.emit("ssh-host-key-prompt", HostKeyPromptEvent {
            prompt_id,
            message: message.to_string(),
            detail: fingerprint.to_string(),
        });
        rx.await.unwrap_or(false)
    }

    async fn prompt_password(&self, message: &str) -> Option<String> {
        let prompt_id = uuid::Uuid::new_v4().to_string();
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.pending_prompts.lock().password.insert(prompt_id.clone(), tx);
        let _ = self.app.emit("ssh-password-prompt", PasswordPromptEvent {
            prompt_id,
            message: message.to_string(),
        });
        rx.await.unwrap_or(None)
    }

    fn on_transfer_progress(&self, _transfer_id: &str, _bytes: u64, _total: Option<u64>) {
        // Transfer progress is handled via the existing mpsc channel pattern
    }
}
```

- [ ] **Step 2: Add `desktop_remote_paths` helper**

```rust
fn desktop_remote_paths() -> RemotePaths {
    let home = dirs::home_dir().unwrap_or_default();
    let ssh_dir = home.join(".ssh");
    RemotePaths {
        known_hosts_file: ssh_dir.join("known_hosts"),
        config_dir: conch_core::config::config_dir().join("remote"),
        default_key_paths: vec![
            ssh_dir.join("id_ed25519"),
            ssh_dir.join("id_rsa"),
            ssh_dir.join("id_ecdsa"),
        ],
    }
}
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo check -p conch_tauri`
Expected: compiles (new code added alongside old)

- [ ] **Step 4: Commit**

```bash
git add crates/conch_tauri/src/remote/mod.rs
git commit -m "Add TauriRemoteCallbacks and desktop_remote_paths"
```

---

## Task 10c: Switch SSH connection commands to conch_remote

**Files:**
- Modify: `crates/conch_tauri/src/remote/mod.rs`

This is the core switchover. Update `RemoteState`, `SshSession`, `ssh_connect`, `ssh_quick_connect`, and auth response commands. The prompt bridge spawn logic (~60 lines each for `ssh_connect` and `ssh_quick_connect`) is completely replaced by constructing a `TauriRemoteCallbacks` and passing it to `conch_remote::ssh::connect_and_open_shell`. This dramatically simplifies these functions.

- [ ] **Step 1: Update `SshSession` type**

```rust
pub(crate) struct SshSession {
    pub input_tx: mpsc::UnboundedSender<conch_remote::ssh::ChannelInput>,
    pub ssh_handle: Arc<conch_remote::russh::client::Handle<conch_remote::handler::ConchSshHandler>>,
    pub host: String,
    pub user: String,
    pub port: u16,
}
```

Note: `conch_remote::russh` is available because `conch_remote` re-exports `russh` via `pub use russh;`.

- [ ] **Step 2: Update `RemoteState`**

```rust
pub(crate) struct RemoteState {
    pub sessions: HashMap<String, SshSession>,
    pub config: conch_remote::config::SshConfig,
    pub ssh_config_entries: Vec<conch_remote::config::ServerEntry>,
    pending_prompts: Arc<Mutex<PendingPrompts>>,
    pub tunnel_manager: conch_remote::tunnel::TunnelManager,
    pub transfers: Arc<parking_lot::Mutex<conch_remote::transfer::TransferRegistry>>,
    pub transfer_progress_tx: mpsc::UnboundedSender<conch_remote::transfer::TransferProgress>,
    pub paths: RemotePaths,
}
```

Update `RemoteState::new()` to use `conch_remote::config::load_config(&paths.config_dir)` and `conch_remote::config::parse_ssh_config()`.

- [ ] **Step 3: Rewrite `ssh_connect` and `ssh_quick_connect`**

The old pattern (spawn prompt bridge task, await connection, bridge auth prompts to Tauri events) is replaced with:

```rust
let callbacks = Arc::new(TauriRemoteCallbacks {
    app: app.clone(),
    pending_prompts: Arc::clone(&state.pending_prompts),
});
let paths = state.paths.clone();

let (ssh_handle, channel) = conch_remote::ssh::connect_and_open_shell(
    &server, password, callbacks, &paths
).await?;
```

The prompt bridging is now handled inside `TauriRemoteCallbacks`. The channel loop spawning and output forwarder stay the same but use `conch_remote::ssh::channel_loop` and `conch_remote::ssh::ChannelInput`.

- [ ] **Step 4: Verify it compiles**

Run: `cargo check -p conch_tauri`
Expected: compiles (may have warnings about unused old modules)

- [ ] **Step 5: Commit**

```bash
git add crates/conch_tauri/src/remote/mod.rs
git commit -m "Switch SSH connection commands to conch_remote"
```

---

## Task 10d: Switch SFTP, transfer, and config commands

**Files:**
- Modify: `crates/conch_tauri/src/remote/mod.rs`
- Modify: `crates/conch_tauri/src/remote/local_fs.rs`

- [ ] **Step 1: Update SFTP commands**

All 8 SFTP commands (`sftp_list_dir`, `sftp_stat`, `sftp_read_file`, `sftp_write_file`, `sftp_mkdir`, `sftp_rename`, `sftp_remove`, `sftp_realpath`) become thin wrappers:

```rust
#[tauri::command]
pub(crate) async fn sftp_list_dir(
    window: tauri::WebviewWindow,
    remote: tauri::State<'_, Arc<parking_lot::Mutex<RemoteState>>>,
    tab_id: u32,
    path: String,
) -> Result<Vec<conch_remote::sftp::FileEntry>, String> {
    let key = session_key(window.label(), tab_id);
    let ssh_handle = {
        let state = remote.lock();
        state.sessions.get(&key)
            .ok_or("No active session")?
            .ssh_handle.clone()
    };
    conch_remote::sftp::list_dir(&ssh_handle, &path).await
}
```

- [ ] **Step 2: Update transfer commands**

`transfer_download`, `transfer_upload`, `transfer_cancel` — delegate to `conch_remote::transfer::*`.

- [ ] **Step 3: Update config commands**

All server config commands (`remote_get_servers`, `remote_save_server`, `remote_delete_server`, `remote_add_folder`, `remote_delete_folder`, `remote_import_ssh_config`, `remote_rename_folder`, `remote_set_folder_expanded`, `remote_move_server`, `remote_export`, `remote_import`, `remote_duplicate_server`) — use `conch_remote::config::*` types and `conch_remote::config::save_config(&state.paths.config_dir, &state.config)`.

- [ ] **Step 4: Update tunnel commands**

`tunnel_start`, `tunnel_stop`, `tunnel_save`, `tunnel_delete`, `tunnel_get_all` — delegate to `conch_remote::tunnel::*`. `tunnel_start` now passes `callbacks` and `&paths` to the tunnel manager.

- [ ] **Step 5: Update `local_fs.rs` import**

Change:
```rust
// OLD:
use super::sftp::FileEntry;
// NEW:
use conch_remote::sftp::FileEntry;
```

- [ ] **Step 6: Update local_* commands in mod.rs**

`local_list_dir`, `local_stat`, etc. return `conch_remote::sftp::FileEntry` but still call the local `local_fs` module functions.

- [ ] **Step 7: Verify it compiles**

Run: `cargo check -p conch_tauri`
Expected: compiles

- [ ] **Step 8: Commit**

```bash
git add crates/conch_tauri/src/remote/mod.rs crates/conch_tauri/src/remote/local_fs.rs
git commit -m "Switch SFTP, transfer, config, and tunnel commands to conch_remote"
```

---

## Task 10e: Delete old modules, update mod.rs declarations, fix tests

**Files:**
- Delete: `crates/conch_tauri/src/remote/ssh.rs`
- Delete: `crates/conch_tauri/src/remote/sftp.rs`
- Delete: `crates/conch_tauri/src/remote/config.rs`
- Delete: `crates/conch_tauri/src/remote/known_hosts.rs`
- Delete: `crates/conch_tauri/src/remote/transfer.rs`
- Delete: `crates/conch_tauri/src/remote/tunnel.rs`
- Modify: `crates/conch_tauri/src/remote/mod.rs`

- [ ] **Step 1: Remove old module declarations**

In mod.rs, remove:
```rust
pub(crate) mod config;
mod known_hosts;
pub(crate) mod sftp;
pub(crate) mod ssh;
pub(crate) mod transfer;
pub(crate) mod tunnel;
```

Keep only:
```rust
pub(crate) mod local_fs;
```

- [ ] **Step 2: Delete the 6 old module files**

- [ ] **Step 3: Update mod.rs tests**

The ~15 tests in mod.rs (`parse_quick_connect_*`, `session_key_format`, `remote_state_new_has_no_sessions`, `find_server_for_tunnel`, `resolve_imported_tunnel_keys`) need import path updates. Change:
- `config::ServerEntry` → `conch_remote::config::ServerEntry`
- `config::SshConfig` → `conch_remote::config::SshConfig`
- `config::SavedTunnel` → `conch_remote::config::SavedTunnel`
- etc.

The test logic is unchanged — only imports shift.

- [ ] **Step 4: Verify workspace compiles**

Run: `cargo check --workspace`
Expected: compiles with no errors

- [ ] **Step 5: Run all workspace tests**

Run: `cargo test --workspace`
Expected: 218+ tests pass. Breakdown: conch_tauri loses ~33 tests (moved to conch_remote), conch_remote gains those ~33 plus new ones. mod.rs retains its ~15 tests. Net: at least 218.

- [ ] **Step 6: Commit**

```bash
git add -A crates/conch_tauri/
git commit -m "Delete old remote modules, complete conch_remote migration"
```

---

## Task 11: Feature-gate JNI in `conch_plugin`

**Files:**
- Modify: `crates/conch_plugin/Cargo.toml`
- Modify: `crates/conch_plugin/src/lib.rs`
- Modify: `crates/conch_plugin/build.rs` (if it exists — remove dead `java_sdk_available` cfg emission)

- [ ] **Step 1: Add feature flag to Cargo.toml**

```toml
[features]
default = ["java"]
java = ["dep:jni"]

[dependencies]
# Change jni line to:
jni = { version = "0.21", features = ["invocation"], optional = true }
```

- [ ] **Step 2: Replace cfg(java_sdk_available) with cfg(feature = "java") in lib.rs**

```rust
// OLD:
#[cfg(java_sdk_available)]
pub mod jvm;
#[cfg(not(java_sdk_available))]
pub mod jvm_stub;
#[cfg(not(java_sdk_available))]
pub use jvm_stub as jvm;

// NEW:
#[cfg(feature = "java")]
pub mod jvm;
#[cfg(not(feature = "java"))]
pub mod jvm_stub;
#[cfg(not(feature = "java"))]
pub use jvm_stub as jvm;
```

- [ ] **Step 3: Update or remove build.rs**

If `crates/conch_plugin/build.rs` emits `cargo:rustc-cfg=java_sdk_available`, remove that logic (it's now dead code — the feature flag controls compilation instead). If build.rs has no other purpose, delete it.

- [ ] **Step 4: Verify with default features (java enabled)**

Run: `cargo check -p conch_plugin`
Expected: compiles

- [ ] **Step 5: Verify without java feature**

Run: `cargo check -p conch_plugin --no-default-features`
Expected: compiles (uses jvm_stub)

- [ ] **Step 6: Run all workspace tests**

Run: `cargo test --workspace`
Expected: all tests pass

- [ ] **Step 7: Commit**

```bash
git add crates/conch_plugin/Cargo.toml crates/conch_plugin/src/lib.rs crates/conch_plugin/build.rs
git commit -m "Feature-gate JNI behind java feature in conch_plugin"
```

---

## Task 12: Final verification and cleanup

**Files:**
- Possibly: `crates/conch_remote/src/lib.rs` (final re-exports)

- [ ] **Step 1: Run full workspace test suite**

Run: `cargo test --workspace`
Expected: 218+ tests pass (original tests + new conch_remote tests)

- [ ] **Step 2: Run clippy**

Run: `cargo clippy --workspace -- -D warnings`
Expected: no warnings

- [ ] **Step 3: Verify desktop app builds**

Run: `cd crates/conch_tauri && cargo build`
Expected: builds successfully

- [ ] **Step 4: Verify conch_remote compiles independently**

Run: `cargo check -p conch_remote`
Expected: compiles

- [ ] **Step 5: Verify conch_plugin compiles without java**

Run: `cargo check -p conch_plugin --no-default-features`
Expected: compiles

- [ ] **Step 6: Final commit if any cleanup needed**

```bash
git add -A
git commit -m "Final cleanup after conch_remote extraction"
```

- [ ] **Step 7: Push branch**

```bash
git push -u origin feat/extract-conch-remote
```

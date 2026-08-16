# Share Bundle Implementation Plan (Sub-project 1)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship encrypted `.termlabshare` export and import — hosts, tunnels, credentials and embedded private keys — so a bundle works on a recipient's clean machine.

**Architecture:** A new `termlab_share` crate holds the bundle type, an encrypted-envelope codec, a pure export planner, a pure import planner, and the one unit that mutates state (the import executor). The crypto is not reimplemented: `termlab_vault::encryption` grows a generic blob layer that both the vault and the bundle use. The Tauri layer holds commands only.

**Tech Stack:** Rust (aes-gcm, argon2, serde_json, base64, uuid, chrono), Tauri 2 commands, vanilla IIFE frontend with the `tl-dialog` design system.

## Global Constraints

- Spec: `docs/superpowers/specs/2026-08-16-share-bundle-design.md`. It governs; this plan implements it.
- **Bundle envelope, exact:** 8-byte magic `TRMLBSHR`, `u32` LE version = 1, 16-byte salt, 12-byte nonce, AES-256-GCM ciphertext. The ciphertext is **JSON** (the vault uses bincode; the bundle deliberately does not, so bundles stay readable across versions).
- **No new crypto.** Argon2id parameters and AES-GCM usage come from `termlab_vault::encryption`. Never re-specify them.
- **Export is encrypted-only.** No plaintext export path survives. **Import must still accept the legacy plaintext `termlab-connections.json`.**
- **Import preserves UUIDs.** New UUID → add; existing UUID → replace. This is what stops today's silent duplication.
- Materialised private keys are written to `~/.config/termlab/keys/<key-id>` with mode `0600` **set before any content is written** (Unix). On Windows, the app config dir with default ACLs.
- Secrets (`passphrase`, `password`, key `material`) live in types that implement `Zeroize`, matching `termlab_vault::model::AuthMethod`.
- `rg` is NOT installed — use `grep`. Run `cargo fmt` and `cargo clippy` on touched crates; `cargo test --workspace` must stay green (13 `test result: ok` lines today).
- Frontend is vanilla IIFE, no bundler: new JS/CSS must be registered in **both** `index.html` and `settings.html`. Build on the design system (`tl-dialog`, `.tl-check`, `.tl-input`, `.tl-btn`) — never bespoke CSS. Raw hex only in `styles/design-system/base.css`.
- **Never run `screencapture`** — no display access; the controller does visual checks.
- Do not touch `crates/termlab_tauri/src/platform.rs` or `src/main.rs` — another session owns them.
- **Every command output pasted into a report must come from a command actually executed.** Two reports in the previous phase fabricated grep output.

### Verified anchors

- `crates/termlab_vault/src/encryption.rs` — `MAGIC = b"TRMLBVLT"` :13, `LEGACY_MAGIC = b"CONCHVLT"` :16, `SALT_LEN = 16` :18, `NONCE_LEN = 12` :19, `derive_key` :26, `encrypt_vault` :37, `decrypt_vault` :60, `generate_salt` :91, `save_vault_file` :97, `load_vault_file` :144.
- `crates/termlab_vault/src/error.rs` — `VaultError::{Locked, AlreadyUnlocked, NotFound, WrongPassword, Corrupted(String), AccountNotFound(Uuid), Encryption(String), Serialization(String)}`.
- `crates/termlab_vault/src/model.rs` — `Vault` :9, `GeneratedKeyEntry` :30, `VaultAccount` :41, `AuthMethod::{Password(String), Key { path, passphrase }, KeyAndPassword { key_path, passphrase, password }}` :51, with `Zeroize`/`Drop` impls following.
- `crates/termlab_remote/src/config.rs` — `ServerEntry` :34, `ServerFolder` :61, `SavedTunnel` :70 (`session_key` legacy, `server_entry_id: Option<String>`), `ExportPayload` :265, `to_export_filtered` :274, `merge_import` :347, `load_config` :236, `save_config` :247, test `export_strips_vault_account_id` :767.
- `crates/termlab_tauri/src/remote/server_commands.rs` — `remote_export` :154, `remote_import` :198.
- `crates/termlab_tauri/src/vault_commands.rs` — `VaultState = Arc<Mutex<VaultManager>>` :12.
- `crates/termlab_tauri/src/lib.rs` — command registration list, `remote_export`/`remote_import` at :545-546.
- Frontend — `app/panels/ssh-panel.js` `exportConfig` :400, `importConfig` :578; `app/features/ssh/data-service.js` `exportSelection` :21, `importConfig` :25; `app/menu-actions.js` :139, :143.

---

### Task 1: Generic encrypted-blob layer in `termlab_vault`

**Files:**
- Modify: `crates/termlab_vault/src/encryption.rs`

**Interfaces:**
- Produces: `pub fn encrypt_blob(magic: &[u8; 8], version: u32, plaintext: &[u8], password: &[u8]) -> Result<Vec<u8>, VaultError>` and `pub fn decrypt_blob(expected_magic: &[u8; 8], legacy_magic: Option<&[u8; 8]>, data: &[u8], password: &[u8]) -> Result<(u32, Vec<u8>), VaultError>`. Task 2 consumes both.
- `encrypt_vault`/`decrypt_vault` keep their exact current signatures and behaviour, including the `CONCHVLT` legacy-magic fallback.

- [ ] **Step 1: Write the failing tests**

Add to the bottom of `crates/termlab_vault/src/encryption.rs`, inside the existing `#[cfg(test)] mod tests` block (create the block if absent):

```rust
    const TEST_MAGIC: &[u8; 8] = b"TESTMGC1";

    #[test]
    fn blob_round_trips() {
        let out = encrypt_blob(TEST_MAGIC, 7, b"hello world", b"pw").unwrap();
        assert_eq!(&out[..8], TEST_MAGIC);
        let (version, plaintext) = decrypt_blob(TEST_MAGIC, None, &out, b"pw").unwrap();
        assert_eq!(version, 7);
        assert_eq!(plaintext, b"hello world");
    }

    #[test]
    fn blob_rejects_wrong_password() {
        let out = encrypt_blob(TEST_MAGIC, 1, b"secret", b"right").unwrap();
        assert!(matches!(
            decrypt_blob(TEST_MAGIC, None, &out, b"wrong"),
            Err(VaultError::WrongPassword)
        ));
    }

    #[test]
    fn blob_rejects_foreign_magic() {
        let out = encrypt_blob(b"OTHERMGC", 1, b"x", b"pw").unwrap();
        assert!(matches!(
            decrypt_blob(TEST_MAGIC, None, &out, b"pw"),
            Err(VaultError::Corrupted(_))
        ));
    }

    #[test]
    fn blob_rejects_truncated_input() {
        let out = encrypt_blob(TEST_MAGIC, 1, b"x", b"pw").unwrap();
        assert!(matches!(
            decrypt_blob(TEST_MAGIC, None, &out[..20], b"pw"),
            Err(VaultError::Corrupted(_))
        ));
    }

    #[test]
    fn blob_reports_version_without_decrypting() {
        // A caller must be able to reject a future version by header alone, so
        // the version is returned even when it is not the one expected.
        let out = encrypt_blob(TEST_MAGIC, 999, b"x", b"pw").unwrap();
        let (version, _) = decrypt_blob(TEST_MAGIC, None, &out, b"pw").unwrap();
        assert_eq!(version, 999);
    }

    #[test]
    fn vault_round_trip_still_works() {
        let vault = Vault::default();
        let bytes = encrypt_vault(&vault, b"master").unwrap();
        let back = decrypt_vault(&bytes, b"master").unwrap();
        assert_eq!(back.version, vault.version);
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p termlab_vault blob_`
Expected: FAIL — `cannot find function encrypt_blob in this scope`.

- [ ] **Step 3: Implement the blob layer**

Add to `crates/termlab_vault/src/encryption.rs`:

```rust
/// Encrypt arbitrary bytes into the standard TermLab envelope:
/// magic(8) | version(u32 LE) | salt(16) | nonce(12) | AES-256-GCM ciphertext.
pub fn encrypt_blob(
    magic: &[u8; 8],
    version: u32,
    plaintext: &[u8],
    password: &[u8],
) -> Result<Vec<u8>, VaultError> {
    let mut salt = [0u8; SALT_LEN];
    rand::thread_rng().fill_bytes(&mut salt);
    let key = derive_key(password, &salt)?;
    let cipher =
        Aes256Gcm::new_from_slice(&key).map_err(|e| VaultError::Encryption(e.to_string()))?;
    let mut nonce_bytes = [0u8; NONCE_LEN];
    rand::thread_rng().fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);
    let ciphertext = cipher
        .encrypt(nonce, plaintext)
        .map_err(|e| VaultError::Encryption(e.to_string()))?;
    let mut output = Vec::with_capacity(8 + 4 + SALT_LEN + NONCE_LEN + ciphertext.len());
    output.extend_from_slice(magic);
    output.extend_from_slice(&version.to_le_bytes());
    output.extend_from_slice(&salt);
    output.extend_from_slice(&nonce_bytes);
    output.extend_from_slice(&ciphertext);
    Ok(output)
}

/// Inverse of `encrypt_blob`. Returns the envelope's version alongside the
/// plaintext so callers can reject versions they do not understand; the version
/// is read from the header, so a caller can also inspect it before trusting the
/// payload. `legacy_magic` accepts a second, historical magic value.
pub fn decrypt_blob(
    expected_magic: &[u8; 8],
    legacy_magic: Option<&[u8; 8]>,
    data: &[u8],
    password: &[u8],
) -> Result<(u32, Vec<u8>), VaultError> {
    let header_len = 8 + 4 + SALT_LEN + NONCE_LEN;
    if data.len() < header_len {
        return Err(VaultError::Corrupted("file too short".into()));
    }
    let magic_ok = &data[..8] == expected_magic
        || legacy_magic.is_some_and(|legacy| &data[..8] == legacy);
    if !magic_ok {
        return Err(VaultError::Corrupted("invalid magic bytes".into()));
    }
    let version = u32::from_le_bytes(
        data[8..12]
            .try_into()
            .map_err(|_| VaultError::Corrupted("invalid version header".into()))?,
    );
    let salt = &data[12..12 + SALT_LEN];
    let nonce_bytes = &data[12 + SALT_LEN..header_len];
    let ciphertext = &data[header_len..];
    let key = derive_key(password, salt)?;
    let cipher =
        Aes256Gcm::new_from_slice(&key).map_err(|e| VaultError::Encryption(e.to_string()))?;
    let nonce = Nonce::from_slice(nonce_bytes);
    let plaintext = cipher
        .decrypt(nonce, ciphertext)
        .map_err(|_| VaultError::WrongPassword)?;
    Ok((version, plaintext))
}
```

- [ ] **Step 4: Rewire the vault functions through it**

Replace the bodies of `encrypt_vault` (:37) and `decrypt_vault` (:60) with:

```rust
pub fn encrypt_vault(vault: &Vault, password: &[u8]) -> Result<Vec<u8>, VaultError> {
    let payload =
        bincode::serialize(vault).map_err(|e| VaultError::Serialization(e.to_string()))?;
    encrypt_blob(MAGIC, FORMAT_VERSION, &payload, password)
}

pub fn decrypt_vault(data: &[u8], password: &[u8]) -> Result<Vault, VaultError> {
    let (version, plaintext) = decrypt_blob(MAGIC, Some(LEGACY_MAGIC), data, password)?;
    if version != FORMAT_VERSION {
        return Err(VaultError::Corrupted(format!(
            "unsupported version: {version}"
        )));
    }
    deserialize_vault(&plaintext)
}
```

The version check stays in `decrypt_vault` so its behaviour is unchanged — the vault still rejects foreign versions with the same error text.

- [ ] **Step 5: Run the full vault suite**

```bash
cargo test -p termlab_vault 2>&1 | tail -5
cargo clippy -p termlab_vault --all-targets 2>&1 | grep -E "^(warning|error)" | head
```
Expected: all tests pass, including every pre-existing vault test. If `load_vault_file` has a test that opens a `CONCHVLT` fixture, it must still pass — that is the legacy path.

- [ ] **Step 6: Commit**

```bash
git add crates/termlab_vault/src/encryption.rs
git commit -m "refactor(vault): extract generic encrypted-blob layer"
```

---

### Task 2: `termlab_share` crate — bundle type and codec

**Files:**
- Create: `crates/termlab_share/Cargo.toml`, `crates/termlab_share/src/lib.rs`, `crates/termlab_share/src/bundle.rs`, `crates/termlab_share/src/codec.rs`
- Modify: `Cargo.toml` (workspace members)

**Interfaces:**
- Consumes: `termlab_vault::encryption::{encrypt_blob, decrypt_blob}` from Task 1.
- Produces: `ShareBundle`, `BundleMetadata`, `BundledVault`, `BundledKey`, `ShareError`, `codec::{encode, decode}`, `SCHEMA_VERSION`. Tasks 3-6 consume these.

- [ ] **Step 1: Create the crate skeleton**

`crates/termlab_share/Cargo.toml`:

```toml
[package]
name = "termlab_share"
version.workspace = true
edition.workspace = true

[dependencies]
termlab_remote = { path = "../termlab_remote" }
termlab_vault = { path = "../termlab_vault" }
serde = { workspace = true, features = ["derive"] }
serde_json = { workspace = true }
uuid = { workspace = true, features = ["v4", "serde"] }
chrono = { workspace = true, features = ["serde"] }
base64 = { workspace = true }
thiserror = { workspace = true }
zeroize = { workspace = true }

[dev-dependencies]
tempfile = { workspace = true }
```

Before writing this file, run `grep -n "base64\|tempfile\|thiserror\|zeroize\|serde_json\|chrono" Cargo.toml` at the repo root. Every dependency above must exist in `[workspace.dependencies]`; add any that do not, pinning the same major version the other crates use. If `base64` is absent workspace-wide, add `base64 = "0.22"`.

Add `"crates/termlab_share",` to the `members` list in the root `Cargo.toml`.

- [ ] **Step 2: Write the failing codec tests**

`crates/termlab_share/src/codec.rs` (tests first, at the bottom of the file):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundle::{BundleMetadata, BundledVault, ShareBundle};

    fn sample() -> ShareBundle {
        ShareBundle {
            schema_version: crate::SCHEMA_VERSION,
            metadata: BundleMetadata {
                created_at: chrono::Utc::now(),
                source_host: "test-host".into(),
                termlab_version: "0.0.0-test".into(),
                includes_credentials: false,
            },
            folders: Vec::new(),
            servers: Vec::new(),
            tunnels: Vec::new(),
            vault: BundledVault::default(),
        }
    }

    #[test]
    fn round_trips() {
        let bytes = encode(&sample(), b"hunter2").unwrap();
        let back = decode(&bytes, b"hunter2").unwrap();
        assert_eq!(back.metadata.source_host, "test-host");
        assert_eq!(back.schema_version, crate::SCHEMA_VERSION);
    }

    #[test]
    fn wrong_password_is_reported_as_such() {
        let bytes = encode(&sample(), b"right").unwrap();
        assert!(matches!(
            decode(&bytes, b"wrong"),
            Err(ShareError::WrongPassword)
        ));
    }

    #[test]
    fn foreign_file_is_not_a_bundle() {
        assert!(matches!(
            decode(b"this is not a bundle at all, not even close", b"pw"),
            Err(ShareError::NotABundle)
        ));
    }

    #[test]
    fn future_schema_version_is_rejected_by_name() {
        let mut b = sample();
        b.schema_version = 999;
        let bytes = encode(&b, b"pw").unwrap();
        assert!(matches!(
            decode(&bytes, b"pw"),
            Err(ShareError::UnsupportedVersion(999))
        ));
    }

    #[test]
    fn envelope_starts_with_the_share_magic() {
        let bytes = encode(&sample(), b"pw").unwrap();
        assert_eq!(&bytes[..8], b"TRMLBSHR");
    }
}
```

- [ ] **Step 3: Run to verify failure**

Run: `cargo test -p termlab_share`
Expected: FAIL to compile — `bundle`/`codec` items do not exist yet.

- [ ] **Step 4: Implement `bundle.rs`**

```rust
use serde::{Deserialize, Serialize};
use termlab_remote::config::{SavedTunnel, ServerEntry, ServerFolder};
use termlab_vault::model::VaultAccount;
use uuid::Uuid;
use zeroize::Zeroize;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BundleMetadata {
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub source_host: String,
    pub termlab_version: String,
    /// Display only. The authoritative test is whether `vault.accounts` and
    /// `vault.keys` are both empty.
    pub includes_credentials: bool,
}

/// A private key carried inside a bundle. `material` is the raw key file's
/// bytes, base64-encoded by serde as a plain String.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BundledKey {
    pub id: Uuid,
    pub original_path: String,
    pub material: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub public_material: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub passphrase: Option<String>,
    pub comment: String,
}

impl Zeroize for BundledKey {
    fn zeroize(&mut self) {
        self.material.zeroize();
        if let Some(p) = &mut self.passphrase {
            p.zeroize();
        }
    }
}

impl Drop for BundledKey {
    fn drop(&mut self) {
        self.zeroize();
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BundledVault {
    #[serde(default)]
    pub accounts: Vec<VaultAccount>,
    #[serde(default)]
    pub keys: Vec<BundledKey>,
}

impl BundledVault {
    /// The authoritative credentials test named in the spec.
    pub fn is_empty(&self) -> bool {
        self.accounts.is_empty() && self.keys.is_empty()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShareBundle {
    pub schema_version: u32,
    pub metadata: BundleMetadata,
    #[serde(default)]
    pub folders: Vec<ServerFolder>,
    #[serde(default)]
    pub servers: Vec<ServerEntry>,
    #[serde(default)]
    pub tunnels: Vec<SavedTunnel>,
    #[serde(default)]
    pub vault: BundledVault,
}
```

If `VaultAccount`, `ServerEntry`, `ServerFolder` or `SavedTunnel` are not public at those paths, export them rather than duplicating the types — a parallel schema is explicitly forbidden by the spec.

- [ ] **Step 5: Implement `codec.rs` and `lib.rs`**

`crates/termlab_share/src/lib.rs`:

```rust
pub mod bundle;
pub mod codec;

pub const SCHEMA_VERSION: u32 = 1;
pub const BUNDLE_MAGIC: &[u8; 8] = b"TRMLBSHR";
pub const BUNDLE_EXTENSION: &str = "termlabshare";

#[derive(Debug, thiserror::Error)]
pub enum ShareError {
    #[error("Incorrect password")]
    WrongPassword,
    #[error("Not a valid TermLab share bundle")]
    NotABundle,
    #[error("This bundle was created by a newer version of TermLab")]
    UnsupportedVersion(u32),
    #[error("{0}")]
    Io(String),
    #[error("{0}")]
    Malformed(String),
}

pub use bundle::{BundleMetadata, BundledKey, BundledVault, ShareBundle};
```

`crates/termlab_share/src/codec.rs` (above the test module):

```rust
use crate::{bundle::ShareBundle, ShareError, BUNDLE_MAGIC, SCHEMA_VERSION};
use termlab_vault::encryption::{decrypt_blob, encrypt_blob};
use termlab_vault::error::VaultError;

/// Serialise and encrypt a bundle. The payload is JSON, not bincode: bundles
/// cross machines and versions, so the wire format stays self-describing.
pub fn encode(bundle: &ShareBundle, password: &[u8]) -> Result<Vec<u8>, ShareError> {
    let json = serde_json::to_vec(bundle).map_err(|e| ShareError::Malformed(e.to_string()))?;
    encrypt_blob(BUNDLE_MAGIC, bundle.schema_version, &json, password).map_err(map_vault_err)
}

pub fn decode(data: &[u8], password: &[u8]) -> Result<ShareBundle, ShareError> {
    let (version, plaintext) =
        decrypt_blob(BUNDLE_MAGIC, None, data, password).map_err(map_vault_err)?;
    if version > SCHEMA_VERSION {
        return Err(ShareError::UnsupportedVersion(version));
    }
    let bundle: ShareBundle =
        serde_json::from_slice(&plaintext).map_err(|e| ShareError::Malformed(e.to_string()))?;
    if bundle.schema_version > SCHEMA_VERSION {
        return Err(ShareError::UnsupportedVersion(bundle.schema_version));
    }
    Ok(bundle)
}

fn map_vault_err(e: VaultError) -> ShareError {
    match e {
        VaultError::WrongPassword => ShareError::WrongPassword,
        VaultError::Corrupted(_) => ShareError::NotABundle,
        other => ShareError::Io(other.to_string()),
    }
}
```

Note the envelope version and the JSON `schema_version` are written from the same field, so they cannot disagree; both are checked on the way back in because a hand-edited file could disagree.

- [ ] **Step 6: Run tests and commit**

```bash
cargo test -p termlab_share 2>&1 | tail -5
cargo clippy -p termlab_share --all-targets 2>&1 | grep -E "^(warning|error)" | head
git add Cargo.toml crates/termlab_share
git commit -m "feat(share): bundle type and encrypted codec"
```

---

### Task 3: Export planner

**Files:**
- Create: `crates/termlab_share/src/export_planner.rs`
- Modify: `crates/termlab_share/src/lib.rs` (add `pub mod export_planner;`)

**Interfaces:**
- Consumes: `ShareBundle`, `BundledKey`, `BundledVault` from Task 2.
- Produces:

```rust
pub trait KeyReader {
    /// Returns (private bytes, optional public bytes) for a key path.
    fn read_key(&self, path: &str) -> Result<(Vec<u8>, Option<Vec<u8>>), String>;
}

pub struct ExportRequest<'a> {
    pub config: &'a termlab_remote::config::SshConfig,
    pub ssh_config_entries: &'a [termlab_remote::config::ServerEntry],
    pub server_ids: Vec<String>,
    pub tunnel_ids: Vec<String>,
    pub include_credentials: bool,
    pub accounts: Vec<termlab_vault::model::VaultAccount>,
    pub source_host: String,
    pub termlab_version: String,
}

pub struct ExportPlan {
    pub bundle: ShareBundle,
    pub warnings: Vec<String>,
    pub auto_pulled: Vec<String>,
}

pub fn plan(req: ExportRequest<'_>, keys: &dyn KeyReader) -> ExportPlan;
```

Task 4 consumes `plan`, `ExportRequest`, `ExportPlan`, `KeyReader`.

- [ ] **Step 1: Write the failing tests**

At the bottom of `export_planner.rs`. Build fixtures with the real config types; use a fake `KeyReader` so no filesystem is touched:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    struct FakeKeys(HashMap<String, Vec<u8>>);
    impl KeyReader for FakeKeys {
        fn read_key(&self, path: &str) -> Result<(Vec<u8>, Option<Vec<u8>>), String> {
            self.0
                .get(path)
                .map(|b| (b.clone(), None))
                .ok_or_else(|| format!("Key file not found: {path}"))
        }
    }

    #[test]
    fn tunnel_pulls_in_its_host_even_when_unselected() {
        // server "s1" is NOT in server_ids; the tunnel referencing it is.
        let plan = plan_with(vec![], vec!["t1".into()], false, FakeKeys(HashMap::new()));
        assert_eq!(plan.bundle.servers.len(), 1);
        assert_eq!(plan.bundle.servers[0].id, "s1");
        assert!(plan.auto_pulled.iter().any(|s| s.contains("s1")));
    }

    #[test]
    fn credentials_off_strips_vault_account_id() {
        let plan = plan_with(vec!["s1".into()], vec![], false, FakeKeys(HashMap::new()));
        assert!(plan.bundle.servers[0].vault_account_id.is_none());
        assert!(plan.bundle.vault.is_empty());
        assert!(!plan.bundle.metadata.includes_credentials);
    }

    #[test]
    fn credentials_on_copies_account_and_embeds_key_material() {
        let mut files = HashMap::new();
        files.insert("/home/u/.ssh/id_ed25519".to_string(), b"PRIVATE-KEY-BYTES".to_vec());
        let plan = plan_with(vec!["s1".into()], vec![], true, FakeKeys(files));
        assert_eq!(plan.bundle.vault.accounts.len(), 1);
        assert_eq!(plan.bundle.vault.keys.len(), 1);
        // Material is base64 of the file's bytes.
        use base64::Engine;
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(&plan.bundle.vault.keys[0].material)
            .unwrap();
        assert_eq!(decoded, b"PRIVATE-KEY-BYTES");
        // The account now points at the bundled key by id, not the old path.
        assert!(plan.bundle.vault.keys[0].original_path.contains("id_ed25519"));
        assert!(plan.bundle.metadata.includes_credentials);
    }

    #[test]
    fn missing_key_file_warns_but_still_exports_the_host() {
        let plan = plan_with(vec!["s1".into()], vec![], true, FakeKeys(HashMap::new()));
        assert_eq!(plan.bundle.servers.len(), 1, "host must still export");
        assert!(
            plan.warnings.iter().any(|w| w.contains("Key file not found")),
            "warnings were: {:?}",
            plan.warnings
        );
    }

    #[test]
    fn tunnel_referencing_an_ssh_config_alias_exports_it_as_a_real_server() {
        // The alias lives only in ssh_config_entries, never in config.ungrouped.
        let plan = plan_alias_case();
        assert!(plan.bundle.servers.iter().any(|s| s.label == "bastion"));
        let tunnel = &plan.bundle.tunnels[0];
        let alias_id = plan
            .bundle
            .servers
            .iter()
            .find(|s| s.label == "bastion")
            .unwrap()
            .id
            .clone();
        assert_eq!(tunnel.server_entry_id.as_deref(), Some(alias_id.as_str()));
    }

    #[test]
    fn unresolvable_tunnel_host_warns_and_keeps_session_key() {
        let plan = plan_unresolvable_tunnel();
        assert_eq!(plan.bundle.tunnels.len(), 1);
        assert_eq!(plan.bundle.tunnels[0].session_key, "ghost@nowhere:22");
        assert!(plan.warnings.iter().any(|w| w.contains("ghost@nowhere")));
    }
}
```

Write the three fixture helpers (`plan_with`, `plan_alias_case`, `plan_unresolvable_tunnel`) in the same module, constructing `SshConfig`, `ServerEntry`, `SavedTunnel` and `VaultAccount` values directly. Fixture shape for `plan_with`: one `ServerEntry { id: "s1", label: "prod", host: "prod.example.com", port: 22, vault_account_id: Some(<account uuid>), .. }` in `config.ungrouped`; one `SavedTunnel { id: <uuid>, label: "t", session_key: "u@prod.example.com:22", server_entry_id: Some("s1"), .. }` with the string id `"t1"` mapped through the `tunnel_ids` filter by its UUID string; one `VaultAccount { id: <account uuid>, auth: AuthMethod::Key { path: "/home/u/.ssh/id_ed25519".into(), passphrase: None }, .. }`.

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p termlab_share export_planner`
Expected: FAIL to compile — `plan` does not exist.

- [ ] **Step 3: Implement the planner**

Implement `plan()` to run these stages in order, each of which the tests above pin:

1. Filter `config` to the selected server ids and tunnel ids, reusing `SshConfig::to_export_filtered` where it fits, and fold in any selected `ssh_config_entries` (the behaviour `remote_export` has today at `server_commands.rs:166-172`).
2. For each selected tunnel, resolve its host: `server_entry_id` first; failing that, match `session_key` (`user@host:port`) against known servers by host and port; failing that, against `ssh_config_entries`. A resolved host that is not already in the bundle is appended and recorded in `auto_pulled`. A host resolved from `ssh_config_entries` is appended as a real `ServerEntry` and the tunnel's `server_entry_id` is rewritten to it **in the bundle only**. An unresolvable host produces a warning naming the `session_key` and leaves the tunnel untouched.
3. If `include_credentials` is false: clear `vault_account_id` on every bundled server, leave `vault` empty, set `includes_credentials = false`, and stop.
4. If true: for each bundled server with a `vault_account_id`, copy the matching `VaultAccount` from `req.accounts` into `vault.accounts` (de-duplicated by id). A referenced account that is not in `req.accounts` is a warning.
5. For each copied account whose auth is `Key { path, passphrase }` or `KeyAndPassword { key_path, passphrase, .. }`, call `keys.read_key(path)`. On success, push a `BundledKey` with a fresh `Uuid`, `material` = base64 of the bytes, the passphrase, `original_path` = the path, and `comment` = the path's file name; then rewrite the account's auth path to the marker `termlab-bundle:<key-id>` so the import executor knows to materialise it. On failure, push the reader's error string as a warning and leave the account's path untouched.
6. Also handle a server's legacy `key_path` field the same way when the server has no `vault_account_id`: synthesise a `VaultAccount` with `AuthMethod::Key`, using the server's `user` for the account username, then run stage 5 on it.
7. Set `includes_credentials = !vault.is_empty()`.

- [ ] **Step 4: Run tests, then commit**

```bash
cargo test -p termlab_share 2>&1 | tail -5
cargo clippy -p termlab_share --all-targets 2>&1 | grep -E "^(warning|error)" | head
git add crates/termlab_share
git commit -m "feat(share): export planner with credential and key embedding"
```

---

### Task 4: Export command and dialog

**Files:**
- Modify: `crates/termlab_tauri/src/remote/server_commands.rs` (replace `remote_export` :154-196)
- Modify: `crates/termlab_tauri/src/lib.rs` (:545, command registration)
- Create: `crates/termlab_tauri/src/share_commands.rs`
- Modify: `crates/termlab_tauri/frontend/app/panels/ssh-panel.js` (`exportConfig` :400), `app/features/ssh/data-service.js` (:21)

**Interfaces:**
- Consumes: Task 3's `plan`, `ExportRequest`, `ExportPlan`, `KeyReader`; Task 2's `codec::encode`.
- Produces: Tauri command `share_export(server_ids, tunnel_ids, include_credentials, password) -> Result<ShareExportSummary, String>` where `ShareExportSummary { path: String, servers: usize, tunnels: usize, credentials: usize, warnings: Vec<String> }`; frontend `sshDataService.exportBundle(invoke, serverIds, tunnelIds, includeCredentials, password)`.

- [ ] **Step 1: Implement the real `KeyReader`**

In `share_commands.rs`:

```rust
pub(crate) struct FsKeyReader;

impl termlab_share::export_planner::KeyReader for FsKeyReader {
    fn read_key(&self, path: &str) -> Result<(Vec<u8>, Option<Vec<u8>>), String> {
        let expanded = shellexpand_tilde(path);
        let private = std::fs::read(&expanded).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => format!("Key file not found: {path}"),
            std::io::ErrorKind::PermissionDenied => format!("Can't read key file: {path}"),
            _ => format!("Can't read key file: {path} ({e})"),
        })?;
        if !looks_like_private_key(&private) {
            return Err(format!("File doesn't look like a private key: {path}"));
        }
        let public = std::fs::read(format!("{expanded}.pub")).ok();
        Ok((private, public))
    }
}

fn looks_like_private_key(bytes: &[u8]) -> bool {
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(64)]);
    head.contains("-----BEGIN") && head.contains("PRIVATE KEY-----")
}
```

`shellexpand_tilde` expands a leading `~` using `dirs::home_dir()`. Check for an existing helper first — run `grep -rn "fn expand_tilde\|shellexpand" --include=*.rs crates/` and reuse what you find rather than adding a second one.

- [ ] **Step 2: Write the command**

`share_export` gathers state, calls the planner, surfaces warnings, encodes, and saves. Password handling: the frontend passes it; never log it, and zeroize the `String` after encoding.

```rust
#[tauri::command]
pub(crate) async fn share_export(
    app: tauri::AppHandle,
    remote: tauri::State<'_, Arc<Mutex<RemoteState>>>,
    vault: tauri::State<'_, crate::vault_commands::VaultState>,
    server_ids: Vec<String>,
    tunnel_ids: Vec<String>,
    include_credentials: bool,
    mut password: String,
) -> Result<ShareExportSummary, String> {
    // ... build ExportRequest from remote state; when include_credentials is
    // true, read accounts from the unlocked vault and return the error
    // "Unlock the vault to include credentials" if it is locked ...
}
```

The vault must already be unlocked: this command does not prompt. If `vault.lock().is_locked()`, return `Err("Unlock the vault to include credentials")` and let the frontend run its existing unlock flow first. Register the command in `lib.rs` next to `remote_export` (:545) and **delete `remote_export`** along with its registration — the spec forbids a surviving plaintext export path. Leave `remote_import` alone; Task 6 owns it.

- [ ] **Step 3: Extend the export dialog**

In `ssh-panel.js:400`, keep the existing selection tree and add, below it: a `.tl-check` labelled *"Include saved credentials (the recipient will receive passwords and private keys)"*, unchecked by default; two `.tl-input[type=password]` fields, "Bundle password" and "Confirm password", with the helper line *"Anyone with this password can read everything in the bundle."*; and an Export button that stays disabled until at least one item is checked **and** both passwords are non-empty and equal. Wire the disabled state through the button's live `disabled` property — `tl-dialog`'s click gate reads that property, not a construction-time snapshot.

Extract that enable rule into a named exported function so it can be tested without a browser — e.g. `window.termlabShareUi = { canExport({ selectedCount, password, confirm }) }` — and have the dialog call it.

On submit, call `sshDataService.exportBundle(...)`. On success, show the returned warnings in a second `tl-dialog` (title "Export complete", the summary counts, then a list of warnings) — if `warnings` is empty, a toast is enough.

`data-service.js`, replacing `exportSelection` (:21):

```js
  async function exportBundle(invoke, serverIds, tunnelIds, includeCredentials, password) {
    return invoke('share_export', { serverIds, tunnelIds, includeCredentials, password });
  }
```

- [ ] **Step 4: Test the enable rule**

Create `scripts/tests/test_share_export_gate.mjs`, following the stubbed-DOM pattern of `scripts/tests/test_tl_dialog.mjs` (no jsdom in this repo). Assert `canExport` is false for: nothing selected; selection but empty passwords; selection but mismatched passwords; and true for a selection with two equal non-empty passwords. Run it and confirm it passes.

- [ ] **Step 5: Verify and commit**

```bash
cargo test --workspace 2>&1 | grep -cE "^test result: ok"
node scripts/tests/test_share_export_gate.mjs
node --check crates/termlab_tauri/frontend/app/panels/ssh-panel.js
node --check crates/termlab_tauri/frontend/app/features/ssh/data-service.js
grep -rn "remote_export" crates/ | grep -v "^crates/.*\.md"   # expect empty
cargo build -p termlab_tauri && (RUST_LOG=info ./target/debug/termlab > /tmp/tl-share-t4.log 2>&1 &) && sleep 8 && pkill -f target/debug/termlab; grep -ci "error\|panic" /tmp/tl-share-t4.log
git add -A crates/termlab_tauri crates/termlab_share scripts/tests
git commit -m "feat(share): encrypted export command and dialog"
```

---

### Task 5: Import planner and executor

**Files:**
- Create: `crates/termlab_share/src/import_planner.rs`, `crates/termlab_share/src/import_executor.rs`
- Modify: `crates/termlab_share/src/lib.rs`

**Interfaces:**
- Consumes: Task 2's `ShareBundle`, `BundledKey`.
- Produces:

```rust
pub enum ItemAction { Add, Replace }

pub struct PlannedItem<T> { pub item: T, pub action: ItemAction }

pub struct ImportPlan {
    pub folders: Vec<PlannedItem<ServerFolder>>,
    pub servers: Vec<PlannedItem<ServerEntry>>,
    pub tunnels: Vec<PlannedItem<SavedTunnel>>,
    pub accounts: Vec<PlannedItem<VaultAccount>>,
    pub keys: Vec<BundledKey>,
    pub skipped: Vec<String>,
}

pub fn plan(bundle: &ShareBundle, config: &SshConfig, existing_account_ids: &[Uuid]) -> ImportPlan;

pub struct ImportOutcome {
    pub servers: usize, pub tunnels: usize, pub credentials: usize,
    pub skipped: Vec<String>, pub credentials_held_back: bool,
}

pub fn execute(
    plan: ImportPlan,
    config: &mut SshConfig,
    key_dir: &Path,
    vault_sink: Option<&mut dyn VaultSink>,
) -> Result<ImportOutcome, ShareError>;

pub trait VaultSink {
    fn upsert_account(&mut self, account: VaultAccount) -> Result<(), String>;
}
```

Task 6 consumes `plan`, `execute`, `ImportOutcome`, `VaultSink`.

- [ ] **Step 1: Write the failing planner tests**

```rust
#[test]
fn new_uuid_adds_and_existing_uuid_replaces() {
    let (bundle, config) = fixture_with_one_overlapping_server();
    let p = plan(&bundle, &config, &[]);
    assert!(matches!(p.servers[0].action, ItemAction::Replace));
    assert!(matches!(p.servers[1].action, ItemAction::Add));
}

#[test]
fn tunnel_with_unresolvable_host_is_skipped() {
    let (bundle, config) = fixture_tunnel_pointing_nowhere();
    let p = plan(&bundle, &config, &[]);
    assert!(p.tunnels.is_empty());
    assert!(p.skipped.iter().any(|s| s.contains("references a host")));
}

#[test]
fn tunnel_resolving_to_an_existing_local_host_is_kept() {
    let (bundle, config) = fixture_tunnel_pointing_at_existing_local_host();
    let p = plan(&bundle, &config, &[]);
    assert_eq!(p.tunnels.len(), 1);
}
```

And for the executor, using `tempfile::tempdir()`. Write `plan_with_one_bundled_key`, `plan_from`, `sample_bundle` and the `KEY_ID` constant as fixtures in the same module — `KEY_ID` is a fixed `Uuid` so the test can predict the written filename:

```rust
#[test]
fn materialises_bundled_keys_with_owner_only_permissions() {
    let dir = tempfile::tempdir().unwrap();
    let plan = plan_with_one_bundled_key();
    let mut config = SshConfig::default();
    let out = execute(plan, &mut config, dir.path(), None).unwrap();
    let written = std::fs::read(dir.path().join(format!("{KEY_ID}"))).unwrap();
    assert_eq!(written, b"PRIVATE-KEY-BYTES");
    assert!(out.credentials_held_back, "no vault sink means credentials are held back");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(dir.path().join(format!("{KEY_ID}")))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600, "key must be owner-read/write only");
    }
}

#[test]
fn importing_the_same_bundle_twice_does_not_duplicate() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = SshConfig::default();
    execute(plan_from(&sample_bundle()), &mut config, dir.path(), None).unwrap();
    let after_first = config.ungrouped.len();
    execute(plan_from(&sample_bundle()), &mut config, dir.path(), None).unwrap();
    assert_eq!(config.ungrouped.len(), after_first, "second import must replace, not append");
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p termlab_share import_`
Expected: FAIL to compile.

- [ ] **Step 3: Implement**

`plan()`: for each folder, server, tunnel and account, `Replace` when an item with that id already exists locally, otherwise `Add`. A tunnel is dropped into `skipped` when its `server_entry_id` names a host that is neither in the bundle nor in `config`. `keys` passes through untouched.

`execute()`, in this order so references resolve:

1. **Keys first.** For each `BundledKey`, base64-decode `material`, write to `key_dir/<id>`; on Unix create the file with `OpenOptions::new().mode(0o600).create_new(true)` so the permissions exist before any bytes do. If the file already exists, leave it and reuse it — ids are UUIDs, so a collision is the same key. Write the public material to `<id>.pub` when present. Build a map from key id to the written path.
2. **Accounts.** If `vault_sink` is `None`, skip every account and set `credentials_held_back = true`. Otherwise, rewrite any auth path of the form `termlab-bundle:<key-id>` to the materialised path from step 1, then `upsert_account`.
3. **Folders, then servers, then tunnels** into `config` — `Add` pushes, `Replace` overwrites the entry with the same id in place.
4. Return counts and the accumulated `skipped` list.

- [ ] **Step 4: Run tests and commit**

```bash
cargo test -p termlab_share 2>&1 | tail -5
cargo clippy -p termlab_share --all-targets 2>&1 | grep -E "^(warning|error)" | head
git add crates/termlab_share
git commit -m "feat(share): import planner and executor"
```

---

### Task 6: Import command, legacy routing, and summary

**Files:**
- Modify: `crates/termlab_tauri/src/share_commands.rs`, `crates/termlab_tauri/src/remote/server_commands.rs` (`remote_import` :198), `crates/termlab_tauri/src/lib.rs`
- Modify: `crates/termlab_tauri/frontend/app/panels/ssh-panel.js` (`importConfig` :578), `app/features/ssh/data-service.js`

**Interfaces:**
- Consumes: Task 2's `codec::decode`, Task 5's `plan`/`execute`/`VaultSink`.
- Produces: Tauri commands `share_pick_import_file() -> Result<ImportFileInfo, String>` (`ImportFileInfo { path: String, kind: "bundle" | "legacy_json" }`) and `share_import(path, password) -> Result<ImportOutcome, String>`.

- [ ] **Step 1: Route by file type**

The picker offers both `*.termlabshare` and `*.json`. Decide `kind` by reading the first 8 bytes: equal to `TRMLBSHR` → `bundle`; otherwise → `legacy_json`. Do not decide by extension — a renamed file must still work.

- [ ] **Step 2: Legacy path stays exactly as it is**

For `legacy_json`, call the existing `remote_import` logic unchanged (`merge_import`, regenerated UUIDs, `resolve_imported_tunnel_keys`). Refactor `remote_import` so the file-reading half and the applying half are separable, and call the applying half — do not duplicate it. Its behaviour must not change; the existing tests around `merge_import` must still pass untouched.

- [ ] **Step 3: Bundle path**

Prompt for the password on the frontend, pass it to `share_import`, decode, plan, execute. Pass a `VaultSink` only when the vault exists and is unlocked; otherwise pass `None` so credentials are held back and the outcome says so. Persist with `save_config` and reload state so both tool windows refresh.

- [ ] **Step 4: Frontend**

`importConfig` (:578) becomes: pick file → if `kind === 'bundle'`, open a `tl-dialog` with one password field and an Unlock button, re-showing it with an inline error on `Incorrect password`, `Not a valid TermLab share bundle`, or `This bundle was created by a newer version of TermLab` → call `share_import` → show the summary dialog *"Imported N hosts, M tunnels, K credentials. S skipped."*, listing skipped entries and, when `credentials_held_back` is true, the line *"Credentials were not imported because this machine has no unlocked vault. Create or unlock your vault and import again."*

- [ ] **Step 5: Verify and commit**

```bash
cargo test --workspace 2>&1 | grep -cE "^test result: ok"
node --check crates/termlab_tauri/frontend/app/panels/ssh-panel.js
cargo build -p termlab_tauri && (RUST_LOG=info ./target/debug/termlab > /tmp/tl-share-t6.log 2>&1 &) && sleep 8 && pkill -f target/debug/termlab; grep -ci "error\|panic" /tmp/tl-share-t6.log
git add -A crates/termlab_tauri crates/termlab_share
git commit -m "feat(share): bundle import with legacy JSON routing"
```

---

### Task 7: End-to-end round trip

**Files:**
- Create: `crates/termlab_share/tests/round_trip.rs`

**Interfaces:** consumes every unit from Tasks 2, 3 and 5.

- [ ] **Step 1: Write the integration test**

Seed a "machine A" `SshConfig` with two folders, three servers (one password account, one key account, one bare `key_path`), and two tunnels (one internal, one via a `~/.ssh/config` alias). Write a fake private key into a temp dir. Export with credentials on, encode with a password, decode, plan against an empty "machine B" config, execute into a second temp dir, then assert:

- every server and tunnel arrives with the **same UUIDs**;
- the key file exists in machine B's key dir with mode `0600` and byte-identical contents;
- the account's auth path points at machine B's key dir, not machine A's original path;
- a second execute of the same bundle leaves the counts unchanged;
- exporting with credentials off produces a bundle whose `vault.is_empty()` is true and whose servers all have `vault_account_id == None`.

- [ ] **Step 2: Run, then commit**

```bash
cargo test -p termlab_share --test round_trip 2>&1 | tail -5
cargo test --workspace 2>&1 | grep -cE "^test result: ok"
git add crates/termlab_share/tests
git commit -m "test(share): end-to-end bundle round trip"
```

---

## Exit criteria

- Export produces only `.termlabshare`; `remote_export` and every reference to it are gone.
- Import accepts both `.termlabshare` and legacy `termlab-connections.json`, deciding by magic bytes rather than extension.
- Importing the same bundle twice does not duplicate anything.
- A bundle carrying a key gives the recipient a working key at `~/.config/termlab/keys/<id>`, mode `0600`.
- `cargo test --workspace` green; new crate has unit tests per unit plus one end-to-end round trip.
- Sub-project 2 inherits: the four-status conflict model, the per-row preview table, and inline vault creation on import.

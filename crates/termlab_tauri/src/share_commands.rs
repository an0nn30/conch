//! Encrypted `.termlabshare` bundle export/import — wires `termlab_share`'s
//! pure `export_planner`/`import_planner`/`import_executor` and `codec` to a
//! real filesystem `KeyReader`/`VaultSink` and Tauri commands.
//!
//! Export is encrypted-only: the legacy plaintext export command has been
//! removed from `remote/server_commands.rs`, and this module's `share_export`
//! is now the only export path registered in `lib.rs`.
//!
//! Import routes by the file's magic bytes, never its extension (a renamed
//! file must still work): `share_pick_import_file` and `share_import` both
//! call `detect_import_kind` independently rather than trusting a
//! frontend-supplied `kind` round-tripped from the former. A `legacy_json`
//! file is hand off to `remote::server_commands::{read_legacy_export_payload,
//! apply_legacy_import}` — the exact same applying logic `remote_import`
//! itself now calls, so legacy behaviour (`merge_import`, regenerated ids,
//! `resolve_imported_tunnel_keys`) is untouched, just reachable from a path
//! that was already picked instead of `remote_import`'s own dialog.

use std::io::Read;
use std::path::Path;
use std::sync::Arc;

use parking_lot::Mutex;
use serde::Serialize;
use ts_rs::TS;
use zeroize::Zeroize;

use termlab_share::export_planner::{ExportRequest, KeyReader, plan};
use termlab_share::import_executor::{ImportOutcome, VaultSink, execute};
use termlab_vault::VaultManager;
use termlab_vault::model::VaultAccount;

use crate::remote::RemoteState;
use crate::remote::server_commands::{apply_legacy_import, read_legacy_export_payload};
use crate::vault_commands::VaultState;

// ---------------------------------------------------------------------------
// KeyReader
// ---------------------------------------------------------------------------

/// Reads private (and, if present, public) key material from disk for the
/// export planner. The only real implementation of
/// `termlab_share::export_planner::KeyReader` — the planner itself stays
/// pure and is tested against a fake.
///
/// Security note: every error string returned from `read_key` is forwarded
/// verbatim by the planner into a warning shown to the user (see
/// `export_planner::embed_key_for_account`). Never fold file contents,
/// passphrases, or key material into these messages — only the (untouched,
/// caller-supplied) `path`.
pub(crate) struct FsKeyReader;

impl KeyReader for FsKeyReader {
    fn read_key(&self, path: &str) -> Result<(Vec<u8>, Option<Vec<u8>>), String> {
        let expanded = termlab_remote::ssh::expand_tilde(path);
        let private = std::fs::read(&expanded).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => format!("Key file not found: {path}"),
            std::io::ErrorKind::PermissionDenied => format!("Can't read key file: {path}"),
            _ => format!("Can't read key file: {path} ({e})"),
        })?;
        if !looks_like_private_key(&private) {
            return Err(format!("File doesn't look like a private key: {path}"));
        }
        let expanded_str = expanded.to_string_lossy().to_string();
        let public = std::fs::read(format!("{expanded_str}.pub")).ok();
        Ok((private, public))
    }
}

fn looks_like_private_key(bytes: &[u8]) -> bool {
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(64)]);
    head.contains("-----BEGIN") && head.contains("PRIVATE KEY-----")
}

// ---------------------------------------------------------------------------
// share_export
// ---------------------------------------------------------------------------

#[derive(Serialize, TS)]
#[ts(export)]
pub(crate) struct ShareExportSummary {
    pub path: String,
    pub servers: usize,
    pub tunnels: usize,
    pub credentials: usize,
    pub warnings: Vec<String>,
    /// Servers pulled into the bundle beyond what the user ticked, because a
    /// selected tunnel depends on them (see
    /// `export_planner::resolve_tunnel_host`). Always surfaced, even when
    /// `warnings` is empty — silently dropping this is what let a declined
    /// dependency travel into a bundle unnoticed (2026-08-16 review finding).
    pub auto_pulled: Vec<String>,
}

/// Grouped selection args for `do_export`, so it stays under clippy's
/// argument-count lint now that `declined_server_ids` joined the other
/// selection fields. `share_export` itself must keep its arguments flat —
/// Tauri command arguments are one per wire key the frontend's `invoke()`
/// call sends, not a nested object — so it packs them into this struct
/// immediately on entry instead.
struct ExportSelection {
    server_ids: Vec<String>,
    tunnel_ids: Vec<String>,
    declined_server_ids: Vec<String>,
    include_credentials: bool,
}

/// Export the selected servers/tunnels (and, if `include_credentials`,
/// their vault-backed accounts and key material) into an encrypted
/// `.termlabshare` bundle chosen via a native save dialog.
///
/// Password handling: `password` arrives as a plain `String` command
/// argument (Tauri has no secret-string type). It is never logged, is
/// forwarded to `codec::encode` by reference only, and is zeroized before
/// this function returns on every path — success, planner warning, or
/// error — via the `do_export`/zeroize split below.
#[allow(clippy::too_many_arguments)] // Tauri command args are flat by design — see ExportSelection above.
#[tauri::command]
pub(crate) async fn share_export(
    app: tauri::AppHandle,
    remote: tauri::State<'_, Arc<Mutex<RemoteState>>>,
    vault: tauri::State<'_, VaultState>,
    server_ids: Vec<String>,
    tunnel_ids: Vec<String>,
    declined_server_ids: Vec<String>,
    include_credentials: bool,
    mut password: String,
) -> Result<ShareExportSummary, String> {
    let selection = ExportSelection {
        server_ids,
        tunnel_ids,
        declined_server_ids,
        include_credentials,
    };
    let result = do_export(&app, &remote, &vault, selection, &password);
    password.zeroize();
    result
}

fn do_export(
    app: &tauri::AppHandle,
    remote: &Arc<Mutex<RemoteState>>,
    vault: &VaultState,
    selection: ExportSelection,
    password: &str,
) -> Result<ShareExportSummary, String> {
    let ExportSelection {
        server_ids,
        tunnel_ids,
        declined_server_ids,
        include_credentials,
    } = selection;
    // The vault must already be unlocked; this command does not prompt —
    // the frontend runs its existing unlock flow (window.vault.ensureUnlocked)
    // before calling share_export with include_credentials: true.
    let accounts = if include_credentials {
        let vault_mgr = vault.lock();
        if vault_mgr.is_locked() {
            return Err("Unlock the vault to include credentials".to_string());
        }
        vault_mgr.list_accounts().map_err(|e| e.to_string())?
    } else {
        Vec::new()
    };

    let export_plan = {
        let state = remote.lock();
        let req = ExportRequest {
            config: &state.config,
            ssh_config_entries: &state.ssh_config_entries,
            server_ids,
            tunnel_ids,
            declined_server_ids,
            include_credentials,
            accounts,
            source_host: local_hostname(),
            termlab_version: env!("CARGO_PKG_VERSION").to_string(),
        };
        plan(req, &FsKeyReader)
    };

    let bytes = termlab_share::codec::encode(&export_plan.bundle, password.as_bytes())
        .map_err(|e| e.to_string())?;

    use tauri_plugin_dialog::DialogExt;
    let default_name = format!(
        "termlab-share-{}.{}",
        chrono::Local::now().format("%Y-%m-%d"),
        termlab_share::BUNDLE_EXTENSION
    );
    let path = app
        .dialog()
        .file()
        .set_file_name(default_name)
        .add_filter("TermLab Share Bundle", &[termlab_share::BUNDLE_EXTENSION])
        .blocking_save_file();

    let path = match path {
        Some(p) => p,
        None => return Err("Export cancelled".to_string()),
    };
    let file_path = path
        .as_path()
        .ok_or_else(|| "Invalid file path".to_string())?;

    // Write temp-file-then-rename so a partial bundle is never left behind
    // if the process dies mid-write (design spec's "Writing" section).
    let mut tmp_name = file_path.as_os_str().to_os_string();
    tmp_name.push(".tmp");
    let tmp_path = std::path::PathBuf::from(tmp_name);
    std::fs::write(&tmp_path, &bytes).map_err(|e| format!("Failed to write file: {e}"))?;
    std::fs::rename(&tmp_path, file_path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp_path);
        format!("Failed to write file: {e}")
    })?;

    Ok(ShareExportSummary {
        path: file_path.to_string_lossy().to_string(),
        servers: export_plan.bundle.servers.len(),
        tunnels: export_plan.bundle.tunnels.len(),
        credentials: export_plan.bundle.vault.accounts.len(),
        warnings: export_plan.warnings,
        auto_pulled: export_plan.auto_pulled,
    })
}

fn local_hostname() -> String {
    hostname::get()
        .ok()
        .and_then(|h| h.into_string().ok())
        .unwrap_or_else(|| "unknown-host".to_string())
}

// ---------------------------------------------------------------------------
// Import
// ---------------------------------------------------------------------------

#[derive(Serialize, TS)]
#[ts(export)]
pub(crate) struct ImportFileInfo {
    pub path: String,
    /// `"bundle"` for an encrypted `.termlabshare` file, `"legacy_json"` for
    /// anything else — decided by `detect_import_kind`'s magic-byte check,
    /// never by the file's extension.
    pub kind: String,
}

/// Offer a native open dialog for both bundle and legacy JSON files, then
/// classify the chosen file by its magic bytes (see `detect_import_kind`)
/// so the frontend knows whether to prompt for a password before calling
/// `share_import`.
#[tauri::command]
pub(crate) async fn share_pick_import_file(
    app: tauri::AppHandle,
) -> Result<ImportFileInfo, String> {
    use tauri_plugin_dialog::DialogExt;
    let path = app
        .dialog()
        .file()
        .add_filter("TermLab Share Bundle", &[termlab_share::BUNDLE_EXTENSION])
        .add_filter("Legacy JSON Export", &["json"])
        .blocking_pick_file();

    let path = match path {
        Some(p) => p,
        None => return Err("Import cancelled".to_string()),
    };
    let file_path = path
        .as_path()
        .ok_or_else(|| "Invalid file path".to_string())?;
    let kind = detect_import_kind(file_path)?;

    Ok(ImportFileInfo {
        path: file_path.to_string_lossy().to_string(),
        kind: kind.to_string(),
    })
}

/// Decide `bundle` vs `legacy_json` from `path`'s first 8 bytes — never
/// from its extension, so a bundle (or legacy export) renamed to the other
/// extension still routes correctly. `TRMLBSHR`
/// (`termlab_share::BUNDLE_MAGIC`) means an encrypted bundle; anything
/// else, including a file shorter than 8 bytes, is treated as the legacy
/// plaintext JSON format — whose own parser raises a clear "Invalid import
/// file" error if the bytes aren't actually JSON.
fn detect_import_kind(path: &Path) -> Result<&'static str, String> {
    let mut file = std::fs::File::open(path).map_err(|e| format!("Failed to read file: {e}"))?;
    let mut magic = [0u8; 8];
    match file.read_exact(&mut magic) {
        Ok(()) if &magic == termlab_share::BUNDLE_MAGIC => Ok("bundle"),
        Ok(()) => Ok("legacy_json"),
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => Ok("legacy_json"),
        Err(e) => Err(format!("Failed to read file: {e}")),
    }
}

#[derive(Serialize, TS)]
#[ts(export)]
pub(crate) struct ShareImportSummary {
    pub servers: usize,
    pub tunnels: usize,
    pub credentials: usize,
    pub skipped: Vec<String>,
    /// True when the bundle carried credentials but they were held back
    /// because no vault is unlocked on this machine (hosts and tunnels are
    /// still imported). Always `false` for a `legacy_json` import — that
    /// format never carries vault credentials.
    pub credentials_held_back: bool,
}

impl From<ImportOutcome> for ShareImportSummary {
    fn from(outcome: ImportOutcome) -> Self {
        Self {
            servers: outcome.servers,
            tunnels: outcome.tunnels,
            credentials: outcome.credentials,
            skipped: outcome.skipped,
            credentials_held_back: outcome.credentials_held_back,
        }
    }
}

/// Adapts the real, unlocked `VaultManager` to `import_executor`'s
/// `VaultSink` trait so `execute` can upsert imported accounts without
/// `termlab_share` depending on `termlab_vault`'s Tauri-facing state type.
struct TauriVaultSink<'a> {
    vault: &'a VaultManager,
}

impl VaultSink for TauriVaultSink<'_> {
    fn upsert_account(&mut self, account: VaultAccount) -> Result<(), String> {
        self.vault
            .upsert_account(account)
            .map_err(|e| e.to_string())
    }
}

/// Import `path` (as previously chosen by `share_pick_import_file`),
/// routed by its own magic-byte check rather than trusting a
/// frontend-supplied kind.
///
/// Password handling mirrors `share_export`: `password` arrives as a plain
/// `String` command argument (Tauri has no secret-string type), is never
/// logged, is forwarded to `codec::decode` by reference only, and is
/// zeroized before this function returns on every path via the
/// `do_import`/zeroize split below. A `legacy_json` file ignores the
/// password (that format has no encryption); it is zeroized anyway for
/// uniformity.
#[tauri::command]
pub(crate) async fn share_import(
    remote: tauri::State<'_, Arc<Mutex<RemoteState>>>,
    vault: tauri::State<'_, VaultState>,
    path: String,
    mut password: String,
) -> Result<ShareImportSummary, String> {
    let result = do_import(&remote, &vault, &path, &password);
    password.zeroize();
    result
}

fn do_import(
    remote: &Arc<Mutex<RemoteState>>,
    vault: &VaultState,
    path: &str,
    password: &str,
) -> Result<ShareImportSummary, String> {
    let file_path = Path::new(path);
    let kind = detect_import_kind(file_path)?;

    if kind == "legacy_json" {
        let payload = read_legacy_export_payload(file_path)?;
        let mut state = remote.lock();
        let (servers, _folders, tunnels) = apply_legacy_import(&mut state, vault, payload);
        return Ok(ShareImportSummary {
            servers,
            tunnels,
            credentials: 0,
            skipped: Vec::new(),
            credentials_held_back: false,
        });
    }

    let bytes = std::fs::read(file_path).map_err(|e| format!("Failed to read file: {e}"))?;
    let bundle =
        termlab_share::codec::decode(&bytes, password.as_bytes()).map_err(|e| e.to_string())?;

    let mut state = remote.lock();
    let vault_mgr = vault.lock();
    let use_vault = !vault_mgr.is_locked();

    let existing_account_ids: Vec<uuid::Uuid> = if use_vault {
        vault_mgr
            .list_accounts()
            .map_err(|e| e.to_string())?
            .iter()
            .map(|a| a.id)
            .collect()
    } else {
        Vec::new()
    };

    let import_plan =
        termlab_share::import_planner::plan(&bundle, &state.config, &existing_account_ids);

    let key_dir = state.paths.config_dir.join("imported-keys");
    std::fs::create_dir_all(&key_dir).map_err(|e| format!("Failed to prepare key storage: {e}"))?;

    let outcome = if use_vault {
        let mut sink = TauriVaultSink { vault: &vault_mgr };
        execute(import_plan, &mut state.config, &key_dir, Some(&mut sink))
    } else {
        execute(import_plan, &mut state.config, &key_dir, None)
    }
    .map_err(|e| e.to_string())?;

    if use_vault {
        vault_mgr.save().map_err(|e| e.to_string())?;
    }
    // `RemoteState` is a single app-managed value shared by every window's
    // Tauri commands (see `lib.rs`'s `.manage(remote_state)`), so mutating
    // `state.config` in place and persisting it here — exactly as
    // `apply_legacy_import` above already does for the legacy path — is
    // sufficient for both the main and settings windows to see the import
    // on their next `remote_get_servers`/`tunnel_get_all` call; there is no
    // separate per-window copy to reconcile.
    termlab_remote::config::save_config(&state.paths.config_dir, &state.config);

    Ok(outcome.into())
}

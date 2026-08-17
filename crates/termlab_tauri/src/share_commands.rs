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
//! apply_legacy_import}` — the same applying logic the now-removed
//! `remote_import` command used to call directly (2026-08-16 review finding
//! M17: it had no frontend caller left once this module's import path
//! superseded it), so legacy behaviour (`merge_import`, regenerated ids,
//! `resolve_imported_tunnel_keys`) is untouched, just reachable from a path
//! that was already picked rather than that command's own dialog.

use std::io::Read;
use std::path::Path;
use std::sync::Arc;

use parking_lot::Mutex;
use serde::Serialize;
use tauri::Emitter;
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
    fn read_key(&self, path: &str) -> Result<termlab_share::export_planner::KeyBytes, String> {
        let expanded = termlab_remote::ssh::expand_tilde(path);
        let private = zeroize::Zeroizing::new(std::fs::read(&expanded).map_err(|e| {
            match e.kind() {
                std::io::ErrorKind::NotFound => format!("Key file not found: {path}"),
                std::io::ErrorKind::PermissionDenied => format!("Can't read key file: {path}"),
                _ => format!("Can't read key file: {path} ({e})"),
            }
        })?);
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

/// Grouped selection args for `do_export`/`do_export_preview`, so they stay
/// under clippy's argument-count lint now that `declined_server_ids` joined
/// the other selection fields. The Tauri commands themselves must keep
/// their arguments flat — Tauri command arguments are one per wire key the
/// frontend's `invoke()` call sends, not a nested object — so each packs
/// them into this struct immediately on entry instead.
struct ExportSelection {
    server_ids: Vec<String>,
    tunnel_ids: Vec<String>,
    declined_server_ids: Vec<String>,
    include_credentials: bool,
}

/// Total server entries a bundle carries, counting both `bundle.servers`
/// (ungrouped) and every entry nested inside `bundle.folders` — a user whose
/// hosts all live in folders was previously told "0 server(s)" because only
/// `bundle.servers.len()` was counted (2026-08-16 review finding I7). The
/// import side already got this right (`import_executor::execute`'s
/// `servers_count`); this mirrors it for the export summary/preview.
fn count_bundled_servers(bundle: &termlab_share::ShareBundle) -> usize {
    bundle.servers.len() + bundle.folders.iter().map(|f| f.entries.len()).sum::<usize>()
}

/// Run the export planner against `selection` and return the resulting
/// [`termlab_share::export_planner::ExportPlan`] — pulled out so both the
/// preview step (`share_export_preview`) and the write step (`share_export`)
/// run the identical planning logic rather than one re-deriving it. Neither
/// encodes or writes anything; the caller decides what to do with the
/// bundle (and, per `embed_key_for_account`, its `BundledKey`s are zeroized
/// on drop, so a preview caller that only reads counts/names out of it and
/// drops the rest never holds embedded key material any longer than this
/// call).
fn build_export_plan(
    remote: &Arc<Mutex<RemoteState>>,
    vault: &VaultState,
    selection: ExportSelection,
) -> Result<termlab_share::export_planner::ExportPlan, String> {
    let ExportSelection {
        server_ids,
        tunnel_ids,
        declined_server_ids,
        include_credentials,
    } = selection;
    // The vault must already be unlocked; this command does not prompt —
    // the frontend runs its existing unlock flow (window.vault.ensureUnlocked)
    // before calling share_export/share_export_preview with
    // include_credentials: true.
    let accounts = if include_credentials {
        let vault_mgr = vault.lock();
        if vault_mgr.is_locked() {
            return Err("Unlock the vault to include credentials".to_string());
        }
        vault_mgr.list_accounts().map_err(|e| e.to_string())?
    } else {
        Vec::new()
    };

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
    Ok(plan(req, &FsKeyReader))
}

/// What a `share_export` call with this selection would produce, computed
/// by actually running the planner — never a frontend-side guess. Returned
/// to the frontend so it can show the user exactly what will be written
/// (auto-pulled hosts, which keys would be embedded, every warning) and let
/// them cancel before anything is encoded or saved to disk, per the design
/// spec's export step 5 ("Preview... The user confirms or cancels") and
/// 2026-08-16 review finding I3: today's flow ran the save dialog and wrote
/// the file *before* showing any of this, with no way to back out.
///
/// Deliberately carries no key material — only each embedded key's
/// `comment` (a filename, e.g. `id_ed25519`; see
/// `export_planner::embed_key_for_account`) — so the wire payload back to
/// the frontend never contains secrets, and the full plan (which does hold
/// key bytes, `Zeroizing` end-to-end) is dropped at the end of this
/// function rather than cached in any backend state.
#[derive(Serialize, TS)]
#[ts(export)]
pub(crate) struct ExportPreview {
    pub servers: usize,
    pub tunnels: usize,
    pub credentials: usize,
    pub keys: Vec<String>,
    pub warnings: Vec<String>,
    pub auto_pulled: Vec<String>,
}

/// Plan step of the export flow (see [`ExportPreview`]): runs the exact same
/// planner `share_export` will, and reports what it found, without encoding
/// or writing anything. No password argument — there is nothing to encrypt
/// yet.
#[tauri::command]
pub(crate) async fn share_export_preview(
    remote: tauri::State<'_, Arc<Mutex<RemoteState>>>,
    vault: tauri::State<'_, VaultState>,
    server_ids: Vec<String>,
    tunnel_ids: Vec<String>,
    declined_server_ids: Vec<String>,
    include_credentials: bool,
) -> Result<ExportPreview, String> {
    let selection = ExportSelection {
        server_ids,
        tunnel_ids,
        declined_server_ids,
        include_credentials,
    };
    let export_plan = build_export_plan(&remote, &vault, selection)?;
    Ok(ExportPreview {
        servers: count_bundled_servers(&export_plan.bundle),
        tunnels: export_plan.bundle.tunnels.len(),
        credentials: export_plan.bundle.vault.accounts.len(),
        keys: export_plan
            .bundle
            .vault
            .keys
            .iter()
            .map(|k| k.comment.clone())
            .collect(),
        warnings: export_plan.warnings,
        auto_pulled: export_plan.auto_pulled,
    })
    // `export_plan` (and the `BundledKey` material it holds) is dropped
    // here, zeroized via `BundledKey`'s own `Drop` impl — nothing from it
    // survives this call except the names/counts already copied above.
}

/// Write step of the export flow (see [`ExportPreview`]): the frontend calls
/// this only after the user has seen `share_export_preview`'s result and
/// confirmed. Re-runs the planner rather than reusing a cached plan from the
/// preview call — deliberately: holding the plaintext plan (embedded key
/// material included) in backend state between two IPC round trips is a
/// bigger risk than the cost of planning twice, and `RemoteState`/the vault
/// can't meaningfully change between the two calls in the single-user
/// desktop flow this dialog drives.
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
    let export_plan = build_export_plan(remote, vault, selection)?;

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
        servers: count_bundled_servers(&export_plan.bundle),
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
    app: tauri::AppHandle,
    remote: tauri::State<'_, Arc<Mutex<RemoteState>>>,
    vault: tauri::State<'_, VaultState>,
    path: String,
    mut password: String,
) -> Result<ShareImportSummary, String> {
    let result = do_import(&app, &remote, &vault, &path, &password);
    password.zeroize();
    result
}

fn do_import(
    app: &tauri::AppHandle,
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
        let (servers, _folders, tunnels) = apply_legacy_import(&mut state, vault, app, payload);
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
    // Owner-only, matching `~/.ssh` — every bundled key materialised below
    // lands in here (M12: `create_dir_all` alone leaves default, not
    // owner-only, permissions on the directory itself).
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&key_dir, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| format!("Failed to secure key storage: {e}"))?;
    }

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
    // `state.config` in place and persisting it here keeps every window's
    // *backend* view in sync. That alone doesn't repaint a second open
    // window's SSH panel — `ssh-panel.js` has no polling or refresh-on-focus
    // — so the event below (mirrors `apply_legacy_import`'s emit for the
    // legacy path) is what actually gets a second window to refresh.
    //
    // This write must be checked, not fire-and-forget: on a full disk or
    // read-only config dir, the keys and vault accounts above already
    // persisted, but `servers.json` would silently not — leaving the user
    // told "Imported N host(s)..." for hosts that vanish on next launch
    // (2026-08-16 review finding I5). `save_config`'s fire-and-forget
    // signature can't surface that, so this uses `try_save_config` instead.
    termlab_remote::config::try_save_config(&state.paths.config_dir, &state.config)
        .map_err(|e| format!("Import succeeded but saving the configuration failed: {e}"))?;
    let _ = app.emit(crate::remote::server_commands::SSH_CONFIG_CHANGED_EVENT, ());

    Ok(outcome.into())
}

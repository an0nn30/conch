//! Encrypted `.termlabshare` bundle export/import — wires `termlab_share`'s
//! pure `export_planner`/`import_planner`/`import_executor` and `codec` to a
//! real filesystem `KeyReader`/`VaultSink` and Tauri commands.
//!
//! Export is encrypted-only: the legacy plaintext export command has been
//! removed from `remote/server_commands.rs`, and this module's `share_export`
//! is now the only export path registered in `lib.rs`.
//!
//! Import routes by the file's magic bytes, never its extension (a renamed
//! file must still work): `share_pick_import_file`, `share_import_plan` and
//! `share_import_apply` all call `detect_import_kind` independently rather
//! than trusting a frontend-supplied `kind` round-tripped from the former. A
//! `legacy_json` file is hand off to
//! `remote::server_commands::{read_legacy_export_payload,
//! apply_legacy_import}` — the same applying logic the now-removed
//! `remote_import` command used to call directly (2026-08-16 review finding
//! M17: it had no frontend caller left once this module's import path
//! superseded it), so legacy behaviour (`merge_import`, regenerated ids,
//! `resolve_imported_tunnel_keys`) is untouched, just reachable from a path
//! that was already picked rather than that command's own dialog.
//!
//! Import used to be one combined command; it now splits into a
//! plan step (`share_import_plan`, decodes and plans against current state,
//! mutates nothing) and an apply step (`share_import_apply`, re-decodes,
//! re-plans, overlays the caller's per-row decisions, then executes) so the
//! frontend can show a conflict preview between the two. The apply step
//! deliberately re-decodes and re-plans rather than reusing a plan cached
//! from the preview call — see `share_import_apply`'s doc comment.

use std::io::Read;
use std::path::Path;
use std::sync::Arc;

use parking_lot::Mutex;
use serde::Serialize;
use tauri::Emitter;
use ts_rs::TS;
use zeroize::Zeroize;

use termlab_remote::config::{SavedTunnel, ServerEntry};
use termlab_share::export_planner::{ExportRequest, KeyReader, plan};
use termlab_share::import_executor::{ImportOutcome, VaultSink, execute};
use termlab_share::import_planner::{ConflictStatus, ImportPlan, ItemAction, PlannedItem};
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
        let private =
            zeroize::Zeroizing::new(std::fs::read(&expanded).map_err(|e| match e.kind() {
                std::io::ErrorKind::NotFound => format!("Key file not found: {path}"),
                std::io::ErrorKind::PermissionDenied => format!("Can't read key file: {path}"),
                _ => format!("Can't read key file: {path} ({e})"),
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
    bundle.servers.len()
        + bundle
            .folders
            .iter()
            .map(|f| f.entries.len())
            .sum::<usize>()
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
/// `share_import_plan`.
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

// ---------------------------------------------------------------------------
// share_import_plan
// ---------------------------------------------------------------------------

#[derive(Serialize, TS)]
#[ts(export)]
pub(crate) struct ImportPreviewRow {
    pub kind: String,
    pub id: String,
    pub label: String,
    pub detail: String,
    pub status: String,
    pub default_action: String,
}

/// What `share_import_apply` would do with every decision left at its
/// default — computed by actually running the planner, never guessed on the
/// frontend. Mutates nothing: no file is written, no config is touched, no
/// vault is opened beyond a read of its lock state.
#[derive(Serialize, TS)]
#[ts(export)]
pub(crate) struct ImportPreview {
    pub rows: Vec<ImportPreviewRow>,
    pub includes_credentials: bool,
    pub vault_state: String,
}

/// One user override for a previewed row, matched back to a re-planned row
/// by `(kind, id)` — see `share_import_apply`'s doc comment for why matching
/// rather than positional/cached state is used.
#[derive(serde::Deserialize)]
pub(crate) struct ImportDecision {
    pub kind: String,
    pub id: String,
    pub action: String,
    pub label: Option<String>,
}

/// Plan step of the import flow (see [`ImportPreview`]): decodes the bundle
/// and runs the exact same planner `share_import_apply` will, against
/// current `config` and the vault's current account ids, and reports the
/// result as flat rows for a table. Never writes anything — not to `config`,
/// not to disk, not to the vault.
#[tauri::command]
pub(crate) async fn share_import_plan(
    remote: tauri::State<'_, Arc<Mutex<RemoteState>>>,
    vault: tauri::State<'_, VaultState>,
    path: String,
    mut password: String,
) -> Result<ImportPreview, String> {
    let result = do_import_plan(&remote, &vault, &path, &password);
    password.zeroize();
    result
}

fn do_import_plan(
    remote: &Arc<Mutex<RemoteState>>,
    vault: &VaultState,
    path: &str,
    password: &str,
) -> Result<ImportPreview, String> {
    let file_path = Path::new(path);
    let kind = detect_import_kind(file_path)?;
    if kind != "bundle" {
        // The legacy JSON path has no conflicts to preview — it always
        // regenerates ids and appends. The frontend never calls this for a
        // `legacy_json` file (see `importConfig`'s branch on
        // `share_pick_import_file`'s `kind`); this guards against a
        // misrouted call instead of planning nonsense.
        return Err("Legacy JSON imports have no preview".to_string());
    }

    let bytes = std::fs::read(file_path).map_err(|e| format!("Failed to read file: {e}"))?;
    let bundle =
        termlab_share::codec::decode(&bytes, password.as_bytes()).map_err(|e| e.to_string())?;

    let vault_mgr = vault.lock();
    let existing_account_ids: Vec<uuid::Uuid> = if !vault_mgr.is_locked() {
        vault_mgr
            .list_accounts()
            .map_err(|e| e.to_string())?
            .iter()
            .map(|a| a.id)
            .collect()
    } else {
        Vec::new()
    };
    let vault_state = if !vault_mgr.vault_exists() {
        "absent"
    } else if vault_mgr.is_locked() {
        "locked"
    } else {
        "unlocked"
    };
    drop(vault_mgr);

    let state = remote.lock();
    let import_plan =
        termlab_share::import_planner::plan(&bundle, &state.config, &existing_account_ids);
    drop(state);

    Ok(ImportPreview {
        rows: flatten_import_rows(&import_plan),
        includes_credentials: bundle.metadata.includes_credentials,
        vault_state: vault_state.to_string(),
    })
}

/// Flatten an [`ImportPlan`] into display rows: folder entries and ungrouped
/// servers both become `kind: "host"` (the recipient's config doesn't
/// distinguish them for conflict purposes — see
/// `import_planner::plan`'s use of `config.find_server`, which scans both),
/// tunnels become `"tunnel"`, and vault accounts become `"credential"`.
/// Bundled keys produce no rows — see the design spec: a key is never
/// independently meaningful to a user and always follows whichever account
/// survives the user's decisions.
fn flatten_import_rows(plan: &ImportPlan) -> Vec<ImportPreviewRow> {
    let mut rows = Vec::new();
    for folder in &plan.folders {
        rows.extend(folder.entries.iter().map(host_row));
    }
    rows.extend(plan.servers.iter().map(host_row));
    rows.extend(plan.tunnels.iter().map(tunnel_row));
    rows.extend(plan.accounts.iter().map(credential_row));
    rows
}

fn host_row(planned: &PlannedItem<ServerEntry>) -> ImportPreviewRow {
    ImportPreviewRow {
        kind: "host".to_string(),
        id: planned.item.id.clone(),
        label: planned.item.label.clone(),
        detail: match &planned.item.user {
            Some(user) => format!("{user}@{}:{}", planned.item.host, planned.item.port),
            None => format!("{}:{}", planned.item.host, planned.item.port),
        },
        status: status_str(planned.status).to_string(),
        default_action: action_str(&planned.action).to_string(),
    }
}

fn tunnel_row(planned: &PlannedItem<SavedTunnel>) -> ImportPreviewRow {
    ImportPreviewRow {
        kind: "tunnel".to_string(),
        id: planned.item.id.to_string(),
        label: planned.item.label.clone(),
        detail: format!(
            "L{} → {}:{}",
            planned.item.local_port, planned.item.remote_host, planned.item.remote_port
        ),
        status: status_str(planned.status).to_string(),
        default_action: action_str(&planned.action).to_string(),
    }
}

fn credential_row(planned: &PlannedItem<VaultAccount>) -> ImportPreviewRow {
    ImportPreviewRow {
        kind: "credential".to_string(),
        id: planned.item.id.to_string(),
        label: planned.item.display_name.clone(),
        detail: planned.item.username.clone(),
        status: status_str(planned.status).to_string(),
        default_action: action_str(&planned.action).to_string(),
    }
}

fn status_str(status: ConflictStatus) -> &'static str {
    match status {
        ConflictStatus::New => "new",
        ConflictStatus::SameId => "same_id",
        ConflictStatus::LabelCollision => "label_collision",
        ConflictStatus::ReferenceBroken => "reference_broken",
    }
}

/// The planner never defaults an item to `Rename` (see `classify` in
/// `import_planner.rs`) — `Rename` only ever arrives as a user override via
/// `ImportDecision`, applied in `apply_decisions` below — so this arm is
/// unreachable in practice for `default_action`, but is handled rather than
/// panicking in case that ever changes.
fn action_str(action: &ItemAction) -> &'static str {
    match action {
        ItemAction::Add => "add",
        ItemAction::Replace => "replace",
        ItemAction::Skip => "skip",
        ItemAction::Rename(_) => "rename",
    }
}

#[derive(Serialize, TS, Default)]
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
    /// How many of the caller's `decisions` named an action `apply_decisions`
    /// judged no longer valid for the row's freshly re-planned status (e.g.
    /// a stale `"add"` echoed back for a row the re-plan now classifies as
    /// `SameId`) and silently fell back to the planner's fresh default for
    /// instead of honouring verbatim — see `apply_decisions`'s doc comment
    /// (2026-08-17 review finding C1). Always `0` for a `legacy_json`
    /// import, which has no decisions to reconcile.
    pub reconciled: usize,
}

impl From<ImportOutcome> for ShareImportSummary {
    fn from(outcome: ImportOutcome) -> Self {
        Self {
            servers: outcome.servers,
            tunnels: outcome.tunnels,
            credentials: outcome.credentials,
            skipped: outcome.skipped,
            credentials_held_back: outcome.credentials_held_back,
            reconciled: 0,
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

/// Turn one frontend [`ImportDecision`] into the [`ItemAction`] it names.
/// `"rename"` with no label is rejected outright — a silent no-op here
/// would mean the user picked "rename" in the UI and nothing happened, with
/// no error to explain why.
fn action_from_decision(decision: &ImportDecision) -> Result<ItemAction, String> {
    match decision.action.as_str() {
        "add" => Ok(ItemAction::Add),
        "replace" => Ok(ItemAction::Replace),
        "skip" => Ok(ItemAction::Skip),
        "rename" => match &decision.label {
            Some(label) => Ok(ItemAction::Rename(label.clone())),
            None => Err("Rename requires a label".to_string()),
        },
        other => Err(format!("Unknown import action: {other}")),
    }
}

fn decision_for<'a>(
    decisions: &'a [ImportDecision],
    kind: &str,
    id: &str,
) -> Option<&'a ImportDecision> {
    decisions.iter().find(|d| d.kind == kind && d.id == id)
}

/// Whether `action` is one of the actions the import-preview frontend's own
/// `ACTIONS_BY_STATUS` table (`import-preview.js`) would ever have offered
/// for a row currently at `status`. Mirrored here (rather than imported —
/// `termlab_share` stays free of any frontend-shaped concept) so
/// `apply_decisions` can independently judge whether a caller-supplied
/// decision is still consistent with the row it's being applied to, instead
/// of trusting it blindly (2026-08-17 review finding C1).
fn action_allowed_for_status(status: ConflictStatus, action: &ItemAction) -> bool {
    match status {
        ConflictStatus::New => matches!(action, ItemAction::Add | ItemAction::Skip),
        ConflictStatus::SameId => matches!(action, ItemAction::Replace | ItemAction::Skip),
        ConflictStatus::LabelCollision => {
            matches!(
                action,
                ItemAction::Add | ItemAction::Rename(_) | ItemAction::Skip
            )
        }
        ConflictStatus::ReferenceBroken => matches!(action, ItemAction::Skip),
    }
}

/// Overlay the frontend's per-row `decisions` onto a freshly re-planned
/// `plan`, matched by `(kind, id)` rather than position — see
/// `share_import_apply`'s doc comment for why the plan is rebuilt from
/// scratch on every apply call rather than reused from the preview. A row
/// with no matching decision keeps the default `.action` the planner already
/// gave it (covers both "the user didn't touch this row" and "this row is
/// new since the preview was shown"); a decision whose `(kind, id)` matches
/// nothing in `plan` is silently unused (covers "this row existed in the
/// preview but is gone now").
///
/// A decision that *does* match a row, but whose action is no longer valid
/// for that row's freshly re-planned status (`action_allowed_for_status`
/// says no), is also silently dropped rather than applied verbatim — the
/// row keeps whatever default action `import_planner::plan` just gave it.
/// This is C1's second layer of defense (2026-08-17 review): the frontend
/// always echoes every row's current action as an explicit decision,
/// including untouched rows sitting at the planner's default, so a decision
/// going stale between preview and apply (a retried apply after the first
/// one already landed, two windows racing on the same bundle) is the
/// ordinary case, not a rare one, and must not be allowed to hand the
/// executor an action that no longer makes sense for what the row actually
/// is now — `import_executor`'s `Add` arm being made idempotent for the
/// same finding is the first layer, this is the second, independent one.
/// Deliberately does not fail the batch: rejecting it outright would turn
/// the ordinary "the plan moved a little between preview and apply" case
/// into a dead end instead of the transparent fallback it is today.
/// Returns how many decisions were reconciled this way, so the caller can
/// report it in [`ShareImportSummary::reconciled`].
fn apply_decisions(plan: &mut ImportPlan, decisions: &[ImportDecision]) -> Result<usize, String> {
    let mut reconciled = 0usize;
    for folder in &mut plan.folders {
        for entry in &mut folder.entries {
            if let Some(d) = decision_for(decisions, "host", &entry.item.id) {
                let action = action_from_decision(d)?;
                if action_allowed_for_status(entry.status, &action) {
                    entry.action = action;
                } else {
                    reconciled += 1;
                }
            }
        }
    }
    for entry in &mut plan.servers {
        if let Some(d) = decision_for(decisions, "host", &entry.item.id) {
            let action = action_from_decision(d)?;
            if action_allowed_for_status(entry.status, &action) {
                entry.action = action;
            } else {
                reconciled += 1;
            }
        }
    }
    for entry in &mut plan.tunnels {
        let id = entry.item.id.to_string();
        if let Some(d) = decision_for(decisions, "tunnel", &id) {
            let action = action_from_decision(d)?;
            if action_allowed_for_status(entry.status, &action) {
                entry.action = action;
            } else {
                reconciled += 1;
            }
        }
    }
    for entry in &mut plan.accounts {
        let id = entry.item.id.to_string();
        if let Some(d) = decision_for(decisions, "credential", &id) {
            let action = action_from_decision(d)?;
            if action_allowed_for_status(entry.status, &action) {
                entry.action = action;
            } else {
                reconciled += 1;
            }
        }
    }
    Ok(reconciled)
}

/// Apply step of the import flow (see [`ImportPreview`]): the frontend calls
/// this after the user has seen `share_import_plan`'s preview and confirmed,
/// passing back whatever `ImportDecision`s override the planner's defaults.
///
/// Re-reads `path`, re-decodes and re-plans from scratch rather than reusing
/// a plan cached from the preview call — deliberately: holding the decoded
/// bundle (private key material included) in backend state between two IPC
/// round trips is a bigger risk than the cost of planning twice, mirroring
/// `share_export`'s preview/write split. The cost is that the plan may have
/// moved under us between the two calls, so decisions are matched to the
/// re-planned rows by `(kind, id)`: a decision for a row that no longer
/// exists is ignored, and a row that appeared keeps its default (see
/// `apply_decisions`).
///
/// Routed by its own magic-byte check rather than trusting a
/// frontend-supplied kind, same as `share_import_plan`.
///
/// Password handling mirrors `share_export`: `password` arrives as a plain
/// `String` command argument (Tauri has no secret-string type), is never
/// logged, is forwarded to `codec::decode` by reference only, and is
/// zeroized before this function returns on every path via the
/// `do_import_apply`/zeroize split below. A `legacy_json` file ignores the
/// password (that format has no encryption); it is zeroized anyway for
/// uniformity.
#[tauri::command]
pub(crate) async fn share_import_apply(
    app: tauri::AppHandle,
    remote: tauri::State<'_, Arc<Mutex<RemoteState>>>,
    vault: tauri::State<'_, VaultState>,
    path: String,
    mut password: String,
    decisions: Vec<ImportDecision>,
) -> Result<ShareImportSummary, String> {
    let result = do_import_apply(&app, &remote, &vault, &path, &password, decisions);
    password.zeroize();
    result
}

fn do_import_apply(
    app: &tauri::AppHandle,
    remote: &Arc<Mutex<RemoteState>>,
    vault: &VaultState,
    path: &str,
    password: &str,
    decisions: Vec<ImportDecision>,
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
            reconciled: 0,
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

    let mut import_plan =
        termlab_share::import_planner::plan(&bundle, &state.config, &existing_account_ids);
    let reconciled = apply_decisions(&mut import_plan, &decisions)?;

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
    //
    // I3 (2026-08-17 review): `state.config` above has already been mutated
    // in memory by `execute` regardless of whether this save succeeds, and
    // nothing here rolls that back on failure — a later, unrelated save
    // would silently persist the import the user was just told had failed.
    // The full fix (apply to a clone of `state.config`, swap it in only
    // after a successful save) is a follow-up; this message is honest about
    // the interim risk in the meantime, rather than implying only the save
    // itself needs retrying.
    termlab_remote::config::try_save_config(&state.paths.config_dir, &state.config).map_err(
        |e| {
            format!(
                "Import succeeded in memory but was not saved to disk ({e}). This change is not \
                 yet persisted — restart TermLab before making any further changes, or they may \
                 be lost along with this import."
            )
        },
    )?;
    let _ = app.emit(crate::remote::server_commands::SSH_CONFIG_CHANGED_EVENT, ());

    let mut summary: ShareImportSummary = outcome.into();
    summary.reconciled = reconciled;
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_server(id: &str) -> ServerEntry {
        ServerEntry {
            id: id.into(),
            label: "prod".into(),
            host: "prod.example.com".into(),
            port: 22,
            user: Some("u".into()),
            auth_method: None,
            key_path: None,
            vault_account_id: None,
            proxy_command: None,
            proxy_jump: None,
        }
    }

    /// C1 regression: a decision echoing a stale `"add"` for a row that
    /// re-planning now classifies as `SameId` (the shape of a retried apply
    /// after the first one already landed, or two windows racing on the
    /// same bundle — see `apply_decisions`'s doc comment) must not be
    /// honoured verbatim: `action_allowed_for_status` rejects `Add` for
    /// `SameId`, so `apply_decisions` must fall back to the planner's fresh
    /// default (`Replace`) and count the row as reconciled, rather than
    /// handing `import_executor::apply_server` an action it would (before
    /// its own idempotency fix) have pushed unconditionally.
    #[test]
    fn stale_add_decision_on_a_now_same_id_row_is_reconciled_to_the_fresh_default() {
        let mut plan = ImportPlan {
            folders: Vec::new(),
            servers: vec![PlannedItem {
                item: sample_server("s1"),
                status: ConflictStatus::SameId,
                action: ItemAction::Replace,
            }],
            tunnels: Vec::new(),
            accounts: Vec::new(),
            keys: Vec::new(),
            skipped: Vec::new(),
        };
        let decisions = vec![ImportDecision {
            kind: "host".into(),
            id: "s1".into(),
            action: "add".into(),
            label: None,
        }];

        let reconciled = apply_decisions(&mut plan, &decisions).unwrap();

        assert_eq!(reconciled, 1);
        assert_eq!(
            plan.servers[0].action,
            ItemAction::Replace,
            "must fall back to the fresh planner default, not the stale 'add'"
        );
    }

    /// The counterpart: a decision that is still valid for the row's
    /// current status must be applied normally and not counted as
    /// reconciled.
    #[test]
    fn a_decision_still_valid_for_the_current_status_is_applied_and_not_reconciled() {
        let mut plan = ImportPlan {
            folders: Vec::new(),
            servers: vec![PlannedItem {
                item: sample_server("s1"),
                status: ConflictStatus::New,
                action: ItemAction::Add,
            }],
            tunnels: Vec::new(),
            accounts: Vec::new(),
            keys: Vec::new(),
            skipped: Vec::new(),
        };
        let decisions = vec![ImportDecision {
            kind: "host".into(),
            id: "s1".into(),
            action: "skip".into(),
            label: None,
        }];

        let reconciled = apply_decisions(&mut plan, &decisions).unwrap();

        assert_eq!(reconciled, 0);
        assert_eq!(plan.servers[0].action, ItemAction::Skip);
    }

    /// A stale `Rename` (only ever valid for `LabelCollision`) surviving
    /// into a row the re-plan now says is `ReferenceBroken` (only `Skip` is
    /// ever valid there) must also be reconciled, not just the `Add`/
    /// `SameId` shape the bug was originally reproduced with.
    #[test]
    fn a_decision_invalid_for_reference_broken_is_reconciled_to_skip() {
        let mut plan = ImportPlan {
            folders: Vec::new(),
            servers: Vec::new(),
            tunnels: vec![PlannedItem {
                item: SavedTunnel {
                    id: uuid::Uuid::nil(),
                    label: "dangling".into(),
                    session_key: "u@ghost.example.com:22".into(),
                    server_entry_id: Some("ghost".into()),
                    local_port: 5432,
                    remote_host: "db.internal".into(),
                    remote_port: 5432,
                    auto_start: false,
                },
                status: ConflictStatus::ReferenceBroken,
                action: ItemAction::Skip,
            }],
            accounts: Vec::new(),
            keys: Vec::new(),
            skipped: Vec::new(),
        };
        let decisions = vec![ImportDecision {
            kind: "tunnel".into(),
            id: uuid::Uuid::nil().to_string(),
            action: "rename".into(),
            label: Some("renamed".into()),
        }];

        let reconciled = apply_decisions(&mut plan, &decisions).unwrap();

        assert_eq!(reconciled, 1);
        assert_eq!(plan.tunnels[0].action, ItemAction::Skip);
    }
}

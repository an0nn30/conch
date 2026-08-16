//! Encrypted `.termlabshare` bundle export — wires `termlab_share`'s pure
//! `export_planner` and `codec` to a real filesystem `KeyReader` and a Tauri
//! command.
//!
//! Export is encrypted-only: the legacy plaintext export command has been
//! removed from `remote/server_commands.rs`, and this module's `share_export`
//! is now the only export path registered in `lib.rs`. Import is Task 6's
//! responsibility; `remote_import` is untouched here.

use std::sync::Arc;

use parking_lot::Mutex;
use serde::Serialize;
use ts_rs::TS;
use zeroize::Zeroize;

use termlab_share::export_planner::{ExportRequest, KeyReader, plan};

use crate::remote::RemoteState;
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
#[tauri::command]
pub(crate) async fn share_export(
    app: tauri::AppHandle,
    remote: tauri::State<'_, Arc<Mutex<RemoteState>>>,
    vault: tauri::State<'_, VaultState>,
    server_ids: Vec<String>,
    tunnel_ids: Vec<String>,
    include_credentials: bool,
    mut password: String,
) -> Result<ShareExportSummary, String> {
    let result = do_export(
        &app,
        &remote,
        &vault,
        server_ids,
        tunnel_ids,
        include_credentials,
        &password,
    );
    password.zeroize();
    result
}

fn do_export(
    app: &tauri::AppHandle,
    remote: &Arc<Mutex<RemoteState>>,
    vault: &VaultState,
    server_ids: Vec<String>,
    tunnel_ids: Vec<String>,
    include_credentials: bool,
    password: &str,
) -> Result<ShareExportSummary, String> {
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
    })
}

fn local_hostname() -> String {
    hostname::get()
        .ok()
        .and_then(|h| h.into_string().ok())
        .unwrap_or_else(|| "unknown-host".to_string())
}

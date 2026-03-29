//! SSH session lifecycle commands — connect, write, resize, disconnect, open channel.

use std::sync::Arc;

use parking_lot::Mutex;
use serde::Serialize;
use tauri::Emitter;
use tokio::sync::mpsc;

use conch_remote::config::ServerEntry;
use conch_remote::ssh::{ChannelInput, SshCredentials};

use super::{RemoteState, SshSession, establish_ssh_session, session_key, spawn_output_forwarder};
use crate::pty::PtyExitEvent;
use crate::vault_commands::VaultState;

// ---------------------------------------------------------------------------
// Event types
// ---------------------------------------------------------------------------

/// Emitted after a successful SSH connect where no vault account was linked,
/// prompting the frontend to ask the user whether to save credentials.
#[derive(Clone, Serialize)]
struct VaultAutoSavePromptEvent {
    server_id: String,
    server_label: String,
    host: String,
    username: String,
    auth_method: String,
}

// ---------------------------------------------------------------------------
// SSH Tauri commands
// ---------------------------------------------------------------------------

/// Connect to an SSH server and open a shell channel in a tab.
#[tauri::command]
pub(crate) async fn ssh_connect(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    remote: tauri::State<'_, Arc<Mutex<RemoteState>>>,
    vault: tauri::State<'_, VaultState>,
    pane_id: u32,
    server_id: String,
    cols: u16,
    rows: u16,
    password: Option<String>,
) -> Result<(), String> {
    let window_label = window.label().to_string();

    // Reject early if pane already has an active session (before vault lookup).
    {
        let state = remote.lock();
        let key = session_key(&window_label, pane_id);
        if state.sessions.contains_key(&key) {
            return Err(format!(
                "Pane {pane_id} already has an SSH session on window {window_label}"
            ));
        }
    }

    // Find the server entry.
    let server = {
        let state = remote.lock();
        state
            .config
            .find_server(&server_id)
            .or_else(|| state.ssh_config_entries.iter().find(|s| s.id == server_id))
            .cloned()
            .ok_or_else(|| format!("Server '{server_id}' not found"))?
    };

    // Try vault credentials first, fall back to legacy ServerEntry fields.
    let used_vault = server.vault_account_id.is_some();
    let credentials = match try_vault_credentials(&vault, &server) {
        Err(e) => return Err(e),
        Ok(Some(creds)) => creds,
        Ok(None) => credentials_from_server(&server, password.clone()),
    };

    establish_ssh_session(
        &window_label,
        &app,
        &remote,
        pane_id,
        &server,
        &credentials,
        cols,
        rows,
    )
    .await?;

    // After successful connect: if no vault account was linked, offer to save.
    if !used_vault {
        let _ = app.emit(
            "vault-auto-save-prompt",
            VaultAutoSavePromptEvent {
                server_id: server.id.clone(),
                server_label: server.label.clone(),
                host: server.host.clone(),
                username: credentials.username.clone(),
                auth_method: credentials.auth_method.clone(),
            },
        );
    }

    Ok(())
}

/// Quick-connect by parsing a `user@host:port` string.
#[tauri::command]
pub(crate) async fn ssh_quick_connect(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    remote: tauri::State<'_, Arc<Mutex<RemoteState>>>,
    vault: tauri::State<'_, VaultState>,
    pane_id: u32,
    spec: String,
    cols: u16,
    rows: u16,
    password: Option<String>,
) -> Result<(), String> {
    let (user, host, port) = parse_quick_connect(&spec);

    let auth_method = if password.is_some() {
        "password".to_string()
    } else {
        "key".to_string()
    };

    let entry = ServerEntry {
        id: uuid::Uuid::new_v4().to_string(),
        label: format!("{user}@{host}:{port}"),
        host: host.clone(),
        port,
        user: Some(user.clone()),
        auth_method: Some(auth_method.clone()),
        key_path: None,
        vault_account_id: None,
        proxy_command: None,
        proxy_jump: None,
    };

    // Don't persist quick-connect entries to config — they're ephemeral.
    let window_label = window.label().to_string();

    // Reject early if pane already has an active session.
    {
        let state = remote.lock();
        let key = session_key(&window_label, pane_id);
        if state.sessions.contains_key(&key) {
            return Err(format!(
                "Pane {pane_id} already has an SSH session on window {window_label}"
            ));
        }
    }

    let credentials = credentials_from_server(&entry, password.clone());

    establish_ssh_session(
        &window_label,
        &app,
        &remote,
        pane_id,
        &entry,
        &credentials,
        cols,
        rows,
    )
    .await?;

    // After successful quick-connect: if a password was used, offer to save
    // the credentials to the vault and create a persistent server entry.
    if password.is_some() {
        let _ = app.emit(
            "vault-auto-save-prompt",
            VaultAutoSavePromptEvent {
                server_id: entry.id.clone(),
                server_label: entry.label.clone(),
                host,
                username: user,
                auth_method,
            },
        );
    }

    // Drop vault to satisfy the Send bound — we don't use it in quick-connect.
    let _ = &vault;

    Ok(())
}

/// Write data to an SSH session.
#[tauri::command]
pub(crate) fn ssh_write(
    window: tauri::WebviewWindow,
    remote: tauri::State<'_, Arc<Mutex<RemoteState>>>,
    pane_id: u32,
    data: String,
) -> Result<(), String> {
    let key = session_key(window.label(), pane_id);
    let state = remote.lock();
    let session = state.sessions.get(&key).ok_or("SSH session not found")?;
    session
        .input_tx
        .send(ChannelInput::Write(data.into_bytes()))
        .map_err(|_| "SSH channel closed".to_string())
}

/// Resize an SSH session's terminal.
#[tauri::command]
pub(crate) fn ssh_resize(
    window: tauri::WebviewWindow,
    remote: tauri::State<'_, Arc<Mutex<RemoteState>>>,
    pane_id: u32,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    let key = session_key(window.label(), pane_id);
    let state = remote.lock();
    let session = state.sessions.get(&key).ok_or("SSH session not found")?;
    session
        .input_tx
        .send(ChannelInput::Resize { cols, rows })
        .map_err(|_| "SSH channel closed".to_string())
}

/// Disconnect an SSH session.
///
/// Signals the channel loop to shut down. The loop's cleanup block handles
/// session removal and connection ref-count decrement.
#[tauri::command]
pub(crate) fn ssh_disconnect(
    window: tauri::WebviewWindow,
    remote: tauri::State<'_, Arc<Mutex<RemoteState>>>,
    pane_id: u32,
) {
    let key = session_key(window.label(), pane_id);
    let state = remote.lock();
    if let Some(session) = state.sessions.get(&key) {
        let _ = session.input_tx.send(ChannelInput::Shutdown);
    }
}

/// Open a new shell channel on an existing SSH connection.
///
/// This allows a split pane to reuse an SSH connection that was established
/// by another pane, avoiding a second authentication round-trip.
#[tauri::command]
pub(crate) async fn ssh_open_channel(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    remote: tauri::State<'_, Arc<Mutex<RemoteState>>>,
    pane_id: u32,
    connection_id: String,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    let window_label = window.label().to_string();
    let key = session_key(&window_label, pane_id);

    let ssh_handle = {
        let state = remote.lock();
        let conn = state
            .connections
            .get(&connection_id)
            .ok_or_else(|| format!("SSH connection '{connection_id}' not found"))?;
        Arc::clone(&conn.ssh_handle)
    };

    let channel = conch_remote::ssh::open_shell_channel(&ssh_handle, cols, rows)
        .await
        .map_err(|e| e.to_string())?;

    let (input_tx, input_rx) = mpsc::unbounded_channel();
    let (output_tx, output_rx) = mpsc::unbounded_channel::<Vec<u8>>();
    let _ = input_tx.send(ChannelInput::Resize { cols, rows });

    let (host, user, port) = {
        let state = remote.lock();
        let conn = state
            .connections
            .get(&connection_id)
            .ok_or_else(|| format!("SSH connection '{connection_id}' disappeared"))?;
        (conn.host.clone(), conn.user.clone(), conn.port)
    };

    let remote_clone = Arc::clone(&*remote);
    {
        let mut state = remote_clone.lock();
        if let Some(conn) = state.connections.get_mut(&connection_id) {
            conn.ref_count += 1;
        }
        state.sessions.insert(
            key.clone(),
            SshSession {
                input_tx,
                connection_id: connection_id.clone(),
                host,
                user,
                port,
                abort_handle: None,
            },
        );
    }

    let remote_for_loop = Arc::clone(&remote_clone);
    let key_for_loop = key.clone();
    let wl = window_label.clone();
    let conn_id = connection_id.clone();
    let app_handle = app.clone();
    let task = tokio::spawn(async move {
        let exited = conch_remote::ssh::channel_loop(channel, input_rx, output_tx).await;
        let mut state = remote_for_loop.lock();
        if state.sessions.remove(&key_for_loop).is_some()
            && let Some(conn) = state.connections.get_mut(&conn_id)
        {
            conn.ref_count = conn.ref_count.saturating_sub(1);
            if conn.ref_count == 0 {
                state.connections.remove(&conn_id);
            }
        }
        drop(state);
        if exited {
            let _ = app_handle.emit_to(
                &wl,
                "pty-exit",
                PtyExitEvent {
                    window_label: wl.clone(),
                    pane_id,
                },
            );
        }
    });

    // Store the abort handle so the channel loop can be cancelled on window close.
    {
        let mut state = remote_clone.lock();
        if let Some(session) = state.sessions.get_mut(&key) {
            session.abort_handle = Some(task.abort_handle());
        }
    }

    spawn_output_forwarder(&app, &window_label, pane_id, output_rx);
    Ok(())
}

// ---------------------------------------------------------------------------
// Credential helpers
// ---------------------------------------------------------------------------

/// Build `SshCredentials` from legacy `ServerEntry` fields (fallback
/// when no vault account is linked).
pub(super) fn credentials_from_server(
    server: &ServerEntry,
    password: Option<String>,
) -> SshCredentials {
    SshCredentials {
        username: server.user.clone().unwrap_or_else(|| "root".to_string()),
        auth_method: server
            .auth_method
            .clone()
            .unwrap_or_else(|| "key".to_string()),
        password,
        key_path: server.key_path.clone(),
        key_passphrase: None,
    }
}

/// Build `SshCredentials` from a vault account.
fn credentials_from_vault_account(account: &conch_vault::VaultAccount) -> SshCredentials {
    match &account.auth {
        conch_vault::AuthMethod::Password(pw) => SshCredentials {
            username: account.username.clone(),
            auth_method: "password".into(),
            password: Some(pw.clone()),
            key_path: None,
            key_passphrase: None,
        },
        conch_vault::AuthMethod::Key { path, passphrase } => SshCredentials {
            username: account.username.clone(),
            auth_method: "key".into(),
            password: None,
            key_path: Some(path.display().to_string()),
            key_passphrase: passphrase.clone(),
        },
        conch_vault::AuthMethod::KeyAndPassword {
            key_path,
            passphrase,
            password,
        } => SshCredentials {
            username: account.username.clone(),
            auth_method: "key_and_password".into(),
            password: Some(password.clone()),
            key_path: Some(key_path.display().to_string()),
            key_passphrase: passphrase.clone(),
        },
    }
}

/// Try to resolve credentials from the vault for a server entry.
/// Returns `Ok(Some(SshCredentials))` if credentials were resolved,
/// `Ok(None)` if the server has no vault_account_id,
/// or `Err("VAULT_LOCKED")` if the server needs vault credentials but the vault is locked.
pub(super) fn try_vault_credentials(
    vault: &VaultState,
    server: &ServerEntry,
) -> Result<Option<SshCredentials>, String> {
    let account_id = match server.vault_account_id {
        Some(id) => id,
        None => return Ok(None),
    };
    let mgr = vault.lock();
    if mgr.is_locked() {
        return Err("VAULT_LOCKED".into());
    }
    let account = mgr
        .get_account(account_id)
        .map_err(|_| format!("Vault account {account_id} not found — it may have been deleted"))?;
    Ok(Some(credentials_from_vault_account(&account)))
}

pub(super) fn parse_quick_connect(input: &str) -> (String, String, u16) {
    let parts: Vec<&str> = input.splitn(2, '@').collect();
    let (user, host_port) = if parts.len() == 2 {
        (parts[0].to_string(), parts[1])
    } else {
        (
            std::env::var("USER").unwrap_or_else(|_| "root".to_string()),
            parts[0],
        )
    };

    let parts: Vec<&str> = host_port.rsplitn(2, ':').collect();
    let (host, port) = if parts.len() == 2 {
        (parts[1].to_string(), parts[0].parse().unwrap_or(22))
    } else {
        (parts[0].to_string(), 22u16)
    };

    (user, host, port)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_quick_connect_full() {
        let (user, host, port) = parse_quick_connect("deploy@10.0.0.1:2222");
        assert_eq!(user, "deploy");
        assert_eq!(host, "10.0.0.1");
        assert_eq!(port, 2222);
    }

    #[test]
    fn parse_quick_connect_no_port() {
        let (user, host, port) = parse_quick_connect("root@example.com");
        assert_eq!(user, "root");
        assert_eq!(host, "example.com");
        assert_eq!(port, 22);
    }

    #[test]
    fn parse_quick_connect_host_only() {
        let (user, host, port) = parse_quick_connect("example.com");
        assert!(!user.is_empty()); // uses $USER or "root"
        assert_eq!(host, "example.com");
        assert_eq!(port, 22);
    }

    #[test]
    fn auto_save_prompt_event_serializes() {
        let event = VaultAutoSavePromptEvent {
            server_id: "s1".into(),
            server_label: "My Server".into(),
            host: "example.com".into(),
            username: "root".into(),
            auth_method: "password".into(),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"server_id\":\"s1\""));
        assert!(json.contains("\"host\":\"example.com\""));
    }

    // ---------------------------------------------------------------------------
    // Vault integration tests
    // ---------------------------------------------------------------------------

    use std::sync::Arc;

    use parking_lot::Mutex;

    use conch_remote::config::ServerEntry;

    fn make_server(label: &str, host: &str, user: &str, port: u16) -> ServerEntry {
        ServerEntry {
            id: format!("sshconfig_{label}"),
            label: label.to_string(),
            host: host.to_string(),
            port,
            user: Some(user.to_string()),
            auth_method: Some("key".to_string()),
            key_path: None,
            vault_account_id: None,
            proxy_command: None,
            proxy_jump: None,
        }
    }

    /// Helper: create a vault, add an account, and return the account.
    fn make_vault_account(
        username: &str,
        display_name: &str,
        auth: conch_vault::AuthMethod,
    ) -> conch_vault::VaultAccount {
        let dir = tempfile::tempdir().unwrap();
        let mgr = conch_vault::VaultManager::new(dir.path().join("vault.enc"));
        mgr.create(b"test").unwrap();
        let id = mgr
            .add_account(display_name.into(), username.into(), auth)
            .unwrap();
        mgr.get_account(id).unwrap()
    }

    #[test]
    fn credentials_from_vault_password_account() {
        let account = make_vault_account(
            "deploy",
            "Deploy Account",
            conch_vault::AuthMethod::Password("s3cret".into()),
        );
        let creds = credentials_from_vault_account(&account);
        assert_eq!(creds.username, "deploy");
        assert_eq!(creds.auth_method, "password");
        assert_eq!(creds.password.as_deref(), Some("s3cret"));
        assert!(creds.key_path.is_none());
        assert!(creds.key_passphrase.is_none());
    }

    #[test]
    fn credentials_from_vault_key_account() {
        let account = make_vault_account(
            "admin",
            "Admin Key",
            conch_vault::AuthMethod::Key {
                path: std::path::PathBuf::from("/home/admin/.ssh/id_ed25519"),
                passphrase: None,
            },
        );
        let creds = credentials_from_vault_account(&account);
        assert_eq!(creds.username, "admin");
        assert_eq!(creds.auth_method, "key");
        assert!(creds.password.is_none());
        assert_eq!(
            creds.key_path.as_deref(),
            Some("/home/admin/.ssh/id_ed25519")
        );
        assert!(creds.key_passphrase.is_none());
    }

    #[test]
    fn credentials_from_vault_key_with_passphrase_account() {
        let account = make_vault_account(
            "admin",
            "Admin Key",
            conch_vault::AuthMethod::Key {
                path: std::path::PathBuf::from("/home/admin/.ssh/id_ed25519"),
                passphrase: Some("mykeypass".into()),
            },
        );
        let creds = credentials_from_vault_account(&account);
        assert_eq!(creds.username, "admin");
        assert_eq!(creds.auth_method, "key");
        assert!(creds.password.is_none());
        assert_eq!(
            creds.key_path.as_deref(),
            Some("/home/admin/.ssh/id_ed25519")
        );
        assert_eq!(creds.key_passphrase.as_deref(), Some("mykeypass"));
    }

    #[test]
    fn credentials_from_vault_key_and_password_account() {
        let account = make_vault_account(
            "root",
            "Root Account",
            conch_vault::AuthMethod::KeyAndPassword {
                key_path: std::path::PathBuf::from("/root/.ssh/id_rsa"),
                passphrase: Some("keypass".into()),
                password: "srvpass".into(),
            },
        );
        let creds = credentials_from_vault_account(&account);
        assert_eq!(creds.username, "root");
        assert_eq!(creds.auth_method, "key_and_password");
        assert_eq!(creds.password.as_deref(), Some("srvpass"));
        assert_eq!(creds.key_path.as_deref(), Some("/root/.ssh/id_rsa"));
        assert_eq!(creds.key_passphrase.as_deref(), Some("keypass"));
    }

    #[test]
    fn try_vault_credentials_returns_none_when_no_account_id() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = conch_vault::VaultManager::new(dir.path().join("vault.enc"));
        mgr.create(b"test").unwrap();
        let vault: VaultState = Arc::new(Mutex::new(mgr));

        let server = make_server("test", "example.com", "root", 22);
        assert!(server.vault_account_id.is_none());
        assert!(try_vault_credentials(&vault, &server).unwrap().is_none());
    }

    #[test]
    fn try_vault_credentials_returns_creds_when_linked() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = conch_vault::VaultManager::new(dir.path().join("vault.enc"));
        mgr.create(b"test").unwrap();
        let account_id = mgr
            .add_account(
                "Deploy".into(),
                "deploy".into(),
                conch_vault::AuthMethod::Password("pw123".into()),
            )
            .unwrap();
        let vault: VaultState = Arc::new(Mutex::new(mgr));

        let mut server = make_server("test", "example.com", "root", 22);
        server.vault_account_id = Some(account_id);

        let creds = try_vault_credentials(&vault, &server).unwrap();
        assert!(creds.is_some());
        let creds = creds.unwrap();
        assert_eq!(creds.username, "deploy");
        assert_eq!(creds.auth_method, "password");
        assert_eq!(creds.password.as_deref(), Some("pw123"));
    }

    #[test]
    fn try_vault_credentials_returns_err_when_locked() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = conch_vault::VaultManager::new(dir.path().join("vault.enc"));
        mgr.create(b"test").unwrap();
        let account_id = mgr
            .add_account(
                "Deploy".into(),
                "deploy".into(),
                conch_vault::AuthMethod::Password("pw123".into()),
            )
            .unwrap();
        mgr.seal();
        let vault: VaultState = Arc::new(Mutex::new(mgr));

        let mut server = make_server("test", "example.com", "root", 22);
        server.vault_account_id = Some(account_id);

        let result = try_vault_credentials(&vault, &server);
        assert!(result.is_err());
        assert_eq!(result.err().unwrap(), "VAULT_LOCKED");
    }

    #[test]
    fn try_vault_credentials_errors_when_account_deleted() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = conch_vault::VaultManager::new(dir.path().join("vault.enc"));
        mgr.create(b"test").unwrap();
        let vault: VaultState = Arc::new(Mutex::new(mgr));

        // Server references a vault account that doesn't exist
        let mut server = make_server("test", "example.com", "root", 22);
        server.vault_account_id = Some(uuid::Uuid::new_v4());

        let result = try_vault_credentials(&vault, &server);
        assert!(result.is_err());
        let err = result.err().unwrap();
        assert!(
            err.contains("not found"),
            "expected 'not found' in error, got: {err}"
        );
    }
}

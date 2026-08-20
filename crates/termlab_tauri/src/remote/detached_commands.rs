//! Detached SFTP sessions — connect to a configured host without a terminal.
//!
//! The SFTP panel can bind to a host that no terminal tab is attached to. Such
//! a connection is a first-class entry in the SAME `RemoteState.sessions` /
//! `RemoteState.connections` maps a terminal SSH tab registers (so the chooser
//! sidebar, `remote_get_sessions`, the editor's save path and
//! `cleanup::cleanup_ssh_sessions` all see it for free) — it simply has no PTY
//! channel, no channel loop, and no output forwarder.
//!
//! Detached sessions are keyed by a reserved pane-id range (`>=
//! DETACHED_PANE_ID_BASE`) so they can never collide with the frontend's
//! sequential terminal pane ids, and so `sftp_disconnect` can refuse to tear
//! down a terminal-owned session by inspecting the key alone.
//!
//! Auth here is non-interactive by construction: unlike `ssh_connect`, which
//! lets `termlab_remote` raise a password prompt mid-connect, this module
//! returns a TYPED error (`SftpConnectError`) and lets the frontend drive the
//! vault-unlock / password dialogs, then call back in through
//! `sftp_connect_host_with_password`. Passwords live only in the invoke
//! payload and the `SshCredentials` (which zeroizes on drop) — they are never
//! logged and never placed in an error message.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use parking_lot::Mutex;
use serde::Serialize;
use tauri::Emitter;
use tokio::sync::mpsc;
use ts_rs::TS;

use termlab_remote::callbacks::RemoteCallbacks;
use termlab_remote::config::ServerEntry;
use termlab_remote::error::RemoteError;
use termlab_remote::ssh::SshCredentials;

use super::server_commands::{find_server_by_entry_id, save_server_preserving_folder};
use super::sftp_commands::session_caller_label;
use super::ssh_commands::{credentials_from_server, try_vault_credentials};
use super::{
    RemoteState, SshConnection, SshSession, TauriRemoteCallbacks, connection_key, session_key,
};
use crate::vault_commands::{AddAccountRequest, VaultState};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// First pane id reserved for detached (PTY-less) sessions.
///
/// The frontend's real pane ids are a sequential counter starting at 1
/// (`main-runtime.js`, `state.js`), so anything at or above this base is
/// unambiguously backend-minted and terminal-owned ids are untouched.
pub(crate) const DETACHED_PANE_ID_BASE: u32 = 1_000_000;

/// Emitted app-wide (no payload) whenever the set of live sessions changes
/// through this module, so every window's SFTP panel and chooser sidebar can
/// refresh. Terminal SSH tabs have never emitted a session-change event; this
/// is introduced by, and stays scoped to, the detached-session commands.
const SESSIONS_CHANGED_EVENT: &str = "remote-sessions-changed";

/// The single refusal message `sftp_disconnect` uses for EVERY rejected key.
/// Deliberately uniform: a caller must not be able to tell a terminal-owned
/// session, another window's session, a malformed key, and a key that never
/// existed apart from one another.
const NOT_A_DETACHED_SESSION: &str = "Not a detached SFTP session";

/// The sentinel `try_vault_credentials` (`ssh_commands.rs`) returns when the
/// server needs vault credentials but the vault is sealed.
const VAULT_LOCKED: &str = "VAULT_LOCKED";

// ---------------------------------------------------------------------------
// Pane id allocator
// ---------------------------------------------------------------------------

/// One process-wide counter rather than a per-window map.
///
/// Session keys are already `"{window_label}:{pane_id}"`, so uniqueness is only
/// required *within* a window — a globally monotone counter is a strict
/// superset of that, and it buys three things a per-window counter in
/// `RemoteState` would not: no map entry to grow or reap on window close, no
/// new `RemoteState` field threaded through every constructor, and ids that
/// never repeat process-wide, so a session key held by a frontend that missed
/// a disconnect can never silently address a *different* later session.
static NEXT_DETACHED_PANE_ID: AtomicU32 = AtomicU32::new(DETACHED_PANE_ID_BASE);

/// Mint the next id from `counter`, never returning one in terminal-owned id
/// space. The re-seed branch is only reachable if the counter wrapped past
/// `u32::MAX` (≈3.29 billion detached connects in one process); it exists so
/// the reserved-range invariant is structural rather than merely probable.
fn mint_pane_id(counter: &AtomicU32) -> u32 {
    let id = counter.fetch_add(1, Ordering::Relaxed);
    if id < DETACHED_PANE_ID_BASE {
        counter.store(DETACHED_PANE_ID_BASE + 1, Ordering::Relaxed);
        return DETACHED_PANE_ID_BASE;
    }
    id
}

fn next_detached_pane_id() -> u32 {
    mint_pane_id(&NEXT_DETACHED_PANE_ID)
}

// ---------------------------------------------------------------------------
// Wire types
// ---------------------------------------------------------------------------

/// A detached session as the frontend sees it.
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub(crate) struct ConnectedSession {
    pub session_key: String,
    pub host: String,
    pub user: String,
    pub port: u16,
    pub pane_id: u32,
}

/// Why a detached connect could not complete.
///
/// Internally tagged so the frontend discriminates on `err.kind` and reads the
/// variant's own fields off the same object. `AuthFailed`/`Unreachable`/`Other`
/// carry the underlying `RemoteError`'s message, which never contains a
/// credential: `termlab_remote` builds its auth errors from russh's error
/// Display, and no call site in this module formats a password into anything.
#[derive(Debug, Serialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export, tag = "kind", rename_all = "camelCase")]
pub(crate) enum SftpConnectError {
    /// The host has a vault account but the vault is sealed — the frontend
    /// raises the master-password dialog and retries.
    VaultLocked,
    /// Auth needs a password this backend does not have. `has_vault_account`
    /// tells the frontend whether "Save to vault" should default to checked.
    #[serde(rename_all = "camelCase")]
    #[ts(rename_all = "camelCase")]
    NeedsPassword {
        has_vault_account: bool,
    },
    /// Credentials were supplied and rejected.
    AuthFailed {
        message: String,
    },
    /// The transport never came up (DNS, TCP, proxy command).
    Unreachable {
        message: String,
    },
    Other {
        message: String,
    },
}

// ---------------------------------------------------------------------------
// Auth plumbing
// ---------------------------------------------------------------------------

/// Wraps the normal Tauri callbacks but refuses to raise an interactive
/// password prompt, recording that it did so.
///
/// `connect_and_auth` asks for a password whenever the credentials it was
/// handed cannot satisfy the server's auth method (a `password` account with
/// an empty stored password, or the `key_and_password` fallback after key auth
/// fails). In a terminal connect that becomes a modal prompt; here it must
/// become a typed `NeedsPassword`, so this returns `None` — which
/// `connect_and_auth` turns into `RemoteError::Auth("Password entry
/// cancelled")` — and the flag lets `classify_connect_error` recognize that
/// shape. Host key verification is NOT suppressed: it is an app-wide flow
/// (`ssh-host-key-prompt` / `auth_respond_host_key`) that is orthogonal to the
/// credential ladder.
struct DetachedCallbacks {
    inner: Arc<dyn RemoteCallbacks>,
    password_prompt_suppressed: Arc<AtomicBool>,
}

#[async_trait::async_trait]
impl RemoteCallbacks for DetachedCallbacks {
    async fn verify_host_key(&self, message: &str, fingerprint: &str) -> bool {
        self.inner.verify_host_key(message, fingerprint).await
    }

    async fn prompt_password(&self, _message: &str) -> Option<String> {
        self.password_prompt_suppressed
            .store(true, Ordering::SeqCst);
        None
    }

    fn on_transfer_progress(&self, _transfer_id: &str, _bytes: u64, _total: Option<u64>) {}
}

/// `connect_and_auth` treats an empty stored password as missing
/// (`ssh.rs`: `Some(pw) if !pw.is_empty()`); the eager-import path stores `""`
/// as a placeholder, so this mirrors that rule exactly.
fn has_usable_password(credentials: &SshCredentials) -> bool {
    credentials
        .password
        .as_deref()
        .is_some_and(|pw| !pw.is_empty())
}

/// Map a `connect_and_auth` failure onto the frontend's typed error.
///
/// The split is structural, not textual: `connect_and_auth` produces
/// `RemoteError::Connection` ONLY while establishing the transport
/// (`client::connect`, `connect_via_proxy`, the iOS no-proxy guard) and
/// `RemoteError::Auth` ONLY from the authentication half — so `Connection`
/// (plus a bare `Io`) is exactly "we never reached the SSH auth stage",
/// i.e. `Unreachable`, and `Auth` is a credential outcome. A rejected/changed
/// host key surfaces through russh as a connect failure and therefore reads as
/// `Unreachable`; the `KnownHosts` variant itself is only produced by the
/// known_hosts writers, never by the connect path, and maps to `Other` so the
/// frontend does not re-prompt for a password that would not help.
fn classify_connect_error(
    err: &RemoteError,
    has_vault_account: bool,
    password_prompt_suppressed: bool,
) -> SftpConnectError {
    match err {
        RemoteError::Connection(message) => SftpConnectError::Unreachable {
            message: message.clone(),
        },
        RemoteError::Io(e) => SftpConnectError::Unreachable {
            message: e.to_string(),
        },
        RemoteError::Auth(message) => {
            if password_prompt_suppressed {
                SftpConnectError::NeedsPassword { has_vault_account }
            } else {
                SftpConnectError::AuthFailed {
                    message: message.clone(),
                }
            }
        }
        other => SftpConnectError::Other {
            message: other.to_string(),
        },
    }
}

// ---------------------------------------------------------------------------
// Session registry
// ---------------------------------------------------------------------------

/// Build the `SshSession` record for a detached session.
///
/// `input_tx` is a placeholder: the field is not optional, but a detached
/// session has no channel loop to feed. The receiver is dropped immediately,
/// so every `send` on it fails — which is the honest outcome for the two
/// commands that could reach it (`ssh_write`/`ssh_resize` answer "SSH channel
/// closed", `ssh_disconnect`'s shutdown signal is a no-op). Nothing routes
/// terminal input at a reserved pane id in the first place, and teardown for
/// these sessions runs through `sftp_disconnect` /
/// `cleanup::cleanup_ssh_sessions`, neither of which depends on the channel.
/// `abort_handle` is `None` for the same reason — there is no spawned loop to
/// abort (`cleanup_ssh_sessions` already treats it as optional).
pub(super) fn detached_session(
    connection_id: String,
    host: String,
    user: String,
    port: u16,
) -> SshSession {
    let (input_tx, input_rx) = mpsc::unbounded_channel();
    drop(input_rx);
    SshSession {
        input_tx,
        connection_id,
        host,
        user,
        port,
        abort_handle: None,
    }
}

/// The ref-count step every teardown path shares: returns the new count and
/// whether the connection entry should now be dropped.
fn release_connection_ref(ref_count: u32) -> (u32, bool) {
    let next = ref_count.saturating_sub(1);
    (next, next == 0)
}

/// Register an authenticated handle as a detached session + connection,
/// mirroring what `establish_ssh_session` inserts minus the channel parts.
fn register_detached_session(
    state: &mut RemoteState,
    window_label: &str,
    server: &ServerEntry,
    credentials: &SshCredentials,
    ssh_handle: termlab_remote::russh::client::Handle<termlab_remote::handler::TermLabSshHandler>,
) -> ConnectedSession {
    let pane_id = next_detached_pane_id();
    let key = session_key(window_label, pane_id);
    let conn_key = connection_key(window_label, pane_id);

    state.connections.insert(
        conn_key.clone(),
        SshConnection {
            ssh_handle: Arc::new(ssh_handle),
            host: server.host.clone(),
            user: credentials.username.clone(),
            port: server.port,
            ref_count: 1,
        },
    );
    state.sessions.insert(
        key.clone(),
        detached_session(
            conn_key,
            server.host.clone(),
            credentials.username.clone(),
            server.port,
        ),
    );

    log::info!(
        "detached SFTP session {key} -> {}@{}:{}",
        credentials.username,
        server.host,
        server.port
    );

    ConnectedSession {
        session_key: key,
        host: server.host.clone(),
        user: credentials.username.clone(),
        port: server.port,
        pane_id,
    }
}

/// Remove a detached session and release its connection reference, mirroring
/// the cleanup block the channel loop runs for terminal sessions
/// (`mod.rs`) and `cleanup::cleanup_ssh_sessions`.
///
/// Returns whether a session was actually removed, so teardown is
/// exactly-once: a repeat call finds nothing and does nothing.
fn remove_detached_session(state: &mut RemoteState, key: &str) -> bool {
    let Some(session) = state.sessions.remove(key) else {
        return false;
    };
    state.pane_cwds.remove(key);
    state.pane_cwd_buffers.remove(key);
    state.pane_input_buffers.remove(key);
    state.pane_prev_cwds.remove(key);
    state.pane_cwd_needs_sync.remove(key);
    state.pane_home_dirs.remove(key);

    if let Some(conn) = state.connections.get_mut(&session.connection_id) {
        let (next, drop_it) = release_connection_ref(conn.ref_count);
        conn.ref_count = next;
        if drop_it {
            state.connections.remove(&session.connection_id);
        }
    }
    log::info!("disconnected detached SFTP session {key}");
    true
}

/// Split a session key into its pane-id tail and validate it as a detached
/// session owned by `window_label`. Every rejection returns the SAME message.
fn detached_pane_id_for_window(session_key: &str, window_label: &str) -> Result<u32, &'static str> {
    let tail = session_key
        .strip_prefix(&format!("{window_label}:"))
        .ok_or(NOT_A_DETACHED_SESSION)?;
    let pane_id: u32 = tail.parse().map_err(|_| NOT_A_DETACHED_SESSION)?;
    if pane_id < DETACHED_PANE_ID_BASE {
        return Err(NOT_A_DETACHED_SESSION);
    }
    Ok(pane_id)
}

// ---------------------------------------------------------------------------
// Connect
// ---------------------------------------------------------------------------

fn resolve_server(
    remote: &Arc<Mutex<RemoteState>>,
    server_entry_id: &str,
) -> Result<ServerEntry, SftpConnectError> {
    let state = remote.lock();
    find_server_by_entry_id(&state, Some(server_entry_id)).ok_or_else(|| SftpConnectError::Other {
        message: format!("Server '{server_entry_id}' not found"),
    })
}

/// Connect + authenticate without a PTY, then register the session and tell
/// the app the session set changed.
async fn connect_detached(
    app: &tauri::AppHandle,
    remote: &Arc<Mutex<RemoteState>>,
    window_label: &str,
    server: &ServerEntry,
    credentials: &SshCredentials,
    has_vault_account: bool,
) -> Result<ConnectedSession, SftpConnectError> {
    let (pending_prompts, paths) = {
        let state = remote.lock();
        (Arc::clone(&state.pending_prompts), state.paths.clone())
    };

    let suppressed = Arc::new(AtomicBool::new(false));
    let callbacks: Arc<dyn RemoteCallbacks> = Arc::new(DetachedCallbacks {
        inner: Arc::new(TauriRemoteCallbacks {
            app: app.clone(),
            pending_prompts,
        }),
        password_prompt_suppressed: Arc::clone(&suppressed),
    });

    let ssh_handle = termlab_remote::ssh::connect_and_auth(server, credentials, callbacks, &paths)
        .await
        .map_err(|e| {
            classify_connect_error(&e, has_vault_account, suppressed.load(Ordering::SeqCst))
        })?;

    let session = {
        let mut state = remote.lock();
        register_detached_session(&mut state, window_label, server, credentials, ssh_handle)
    };

    let _ = app.emit(SESSIONS_CHANGED_EVENT, ());
    Ok(session)
}

/// Connect the SFTP panel to a configured host with no user interaction.
///
/// The ladder, in order: resolve the entry → vault credentials → key auth per
/// the entry's `auth_method`. Anything that would need the user returns a
/// typed error instead of prompting, and the frontend decides what to ask.
#[tauri::command]
pub(crate) async fn sftp_connect_host(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    remote: tauri::State<'_, Arc<Mutex<RemoteState>>>,
    vault: tauri::State<'_, VaultState>,
    server_entry_id: String,
) -> Result<ConnectedSession, SftpConnectError> {
    // A popped-out panel host connects on behalf of its parent window, so the
    // session must key under the PARENT's label.
    let window_label = session_caller_label(&window);
    let remote = Arc::clone(&remote);
    let server = resolve_server(&remote, &server_entry_id)?;
    let has_vault_account = server.vault_account_id.is_some();

    let credentials = match try_vault_credentials(&vault, &server) {
        // A sealed vault stops the ladder here: unlocking is the user's
        // decision, and falling through to key auth would silently connect as
        // somebody other than the account they linked.
        Err(e) if e == VAULT_LOCKED => return Err(SftpConnectError::VaultLocked),
        Err(message) => return Err(SftpConnectError::Other { message }),
        Ok(Some(credentials)) => credentials,
        Ok(None) => credentials_from_server(&server, None),
    };

    // Password auth with nothing to send is decided before any socket: there
    // is no attempt that could succeed, and the frontend owns the prompt.
    if credentials.auth_method == "password" && !has_usable_password(&credentials) {
        return Err(SftpConnectError::NeedsPassword { has_vault_account });
    }

    connect_detached(
        &app,
        &remote,
        &window_label,
        &server,
        &credentials,
        has_vault_account,
    )
    .await
}

/// Retry a detached connect with a password the user just typed.
///
/// `save_to_vault` stores it as a new vault account AND links the server entry
/// to it — the auto-link the Hosts UI does not do (its connection form leaves
/// account linking a manual pick), so a host connected this way authenticates
/// silently next time, across restarts.
#[tauri::command]
pub(crate) async fn sftp_connect_host_with_password(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    remote: tauri::State<'_, Arc<Mutex<RemoteState>>>,
    vault: tauri::State<'_, VaultState>,
    server_entry_id: String,
    password: String,
    save_to_vault: bool,
) -> Result<ConnectedSession, SftpConnectError> {
    let window_label = session_caller_label(&window);
    let remote_state = Arc::clone(&remote);
    let server = resolve_server(&remote_state, &server_entry_id)?;
    let has_vault_account = server.vault_account_id.is_some();

    // Prefer the linked account's username (it can differ from the entry's
    // legacy `user` field, and is what `ssh_connect` would have used); fall
    // back to the entry, then to `root`, exactly as the terminal path does.
    let username = match try_vault_credentials(&vault, &server) {
        Ok(Some(credentials)) => credentials.username.clone(),
        _ => credentials_from_server(&server, None).username.clone(),
    };

    let credentials = SshCredentials {
        username,
        auth_method: "password".to_string(),
        password: Some(password),
        key_path: None,
        key_passphrase: None,
    };

    let session = connect_detached(
        &app,
        &remote_state,
        &window_label,
        &server,
        &credentials,
        has_vault_account,
    )
    .await?;

    // `credentials.password` is the argument we were handed, so it is always
    // `Some` here — the binding just avoids cloning it back out.
    if save_to_vault
        && let Some(pw) = credentials.password.as_deref()
        && let Err(e) = link_password_account(
            vault.clone(),
            &remote_state,
            &server,
            &credentials.username,
            pw,
        )
    {
        // The connection itself succeeded, which is what the user asked for; a
        // failed vault write (sealed vault, disk error) is reported as a
        // warning rather than failing the connect. `e` comes from the
        // vault/config layer and carries no secret.
        log::warn!("could not link a vault account for '{}': {e}", server.label);
    }

    Ok(session)
}

/// Create a password account for `server` and point the server entry at it.
///
/// Goes through exactly the surfaces the Hosts UI uses — `vault_add_account`
/// for the secret, then the `remote_save_server` upsert for the entry — so the
/// link is persisted to the config file and survives a restart.
fn link_password_account(
    vault: tauri::State<'_, VaultState>,
    remote: &Arc<Mutex<RemoteState>>,
    server: &ServerEntry,
    username: &str,
    password: &str,
) -> Result<uuid::Uuid, String> {
    let account_id = crate::vault_commands::vault_add_account(
        vault,
        AddAccountRequest {
            display_name: server.label.clone(),
            username: username.to_string(),
            auth_type: "password".to_string(),
            password: Some(password.to_string()),
            key_path: None,
            passphrase: None,
        },
    )?;

    let mut entry = server.clone();
    entry.vault_account_id = Some(account_id);
    let mut state = remote.lock();
    // Note: for a host that came from `~/.ssh/config` this writes a config-owned
    // copy of the entry, which is the same promotion the Hosts UI performs when
    // an imported host is saved — an ssh-config entry is re-parsed on every
    // launch and could not otherwise carry a vault link.
    save_server_preserving_folder(&mut state, entry);
    Ok(account_id)
}

// ---------------------------------------------------------------------------
// Disconnect
// ---------------------------------------------------------------------------

/// Tear down a detached session. Refuses terminal-owned sessions: those die
/// with their tab, and nothing about that is this command's business.
#[tauri::command]
pub(crate) fn sftp_disconnect(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    remote: tauri::State<'_, Arc<Mutex<RemoteState>>>,
    session_key: String,
) -> Result<(), String> {
    let window_label = session_caller_label(&window);
    detached_pane_id_for_window(&session_key, &window_label)?;

    let removed = {
        let mut state = remote.lock();
        remove_detached_session(&mut state, &session_key)
    };
    if removed {
        let _ = app.emit(SESSIONS_CHANGED_EVENT, ());
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::atomic::AtomicU32;

    // -----------------------------------------------------------------
    // Pane id allocator
    // -----------------------------------------------------------------

    #[test]
    fn allocator_starts_at_the_reserved_base() {
        let counter = AtomicU32::new(DETACHED_PANE_ID_BASE);
        assert_eq!(mint_pane_id(&counter), DETACHED_PANE_ID_BASE);
        assert_eq!(mint_pane_id(&counter), DETACHED_PANE_ID_BASE + 1);
    }

    #[test]
    fn allocator_is_strictly_monotone() {
        let counter = AtomicU32::new(DETACHED_PANE_ID_BASE);
        let mut previous = mint_pane_id(&counter);
        for _ in 0..256 {
            let next = mint_pane_id(&counter);
            assert!(
                next > previous,
                "ids must only ever grow: {next} <= {previous}"
            );
            previous = next;
        }
    }

    #[test]
    fn allocator_never_mints_into_terminal_id_space() {
        // A counter that wrapped past u32::MAX would otherwise alias real PTY
        // pane ids; the allocator re-seeds instead.
        let counter = AtomicU32::new(0);
        assert_eq!(mint_pane_id(&counter), DETACHED_PANE_ID_BASE);
        assert!(mint_pane_id(&counter) > DETACHED_PANE_ID_BASE);
    }

    #[test]
    fn process_allocator_mints_growing_reserved_ids() {
        let first = next_detached_pane_id();
        let second = next_detached_pane_id();
        assert!(first >= DETACHED_PANE_ID_BASE);
        assert!(second > first);
    }

    // -----------------------------------------------------------------
    // Error classification
    // -----------------------------------------------------------------

    #[test]
    fn connection_failure_is_unreachable() {
        let err = RemoteError::Connection("dns lookup failed".into());
        match classify_connect_error(&err, false, false) {
            SftpConnectError::Unreachable { message } => assert!(message.contains("dns")),
            other => panic!("expected Unreachable, got {other:?}"),
        }
    }

    #[test]
    fn io_failure_is_unreachable() {
        let err = RemoteError::Io(std::io::Error::new(
            std::io::ErrorKind::ConnectionRefused,
            "connection refused",
        ));
        assert!(matches!(
            classify_connect_error(&err, false, false),
            SftpConnectError::Unreachable { .. }
        ));
    }

    #[test]
    fn auth_failure_is_auth_failed() {
        let err = RemoteError::Auth("Authentication failed".into());
        match classify_connect_error(&err, true, false) {
            SftpConnectError::AuthFailed { message } => {
                assert_eq!(message, "Authentication failed");
            }
            other => panic!("expected AuthFailed, got {other:?}"),
        }
    }

    #[test]
    fn suppressed_password_prompt_becomes_needs_password_without_account() {
        let err = RemoteError::Auth("Password entry cancelled".into());
        assert!(matches!(
            classify_connect_error(&err, false, true),
            SftpConnectError::NeedsPassword {
                has_vault_account: false
            }
        ));
    }

    #[test]
    fn suppressed_password_prompt_becomes_needs_password_with_account() {
        let err = RemoteError::Auth("Password entry cancelled".into());
        assert!(matches!(
            classify_connect_error(&err, true, true),
            SftpConnectError::NeedsPassword {
                has_vault_account: true
            }
        ));
    }

    #[test]
    fn a_suppressed_prompt_does_not_turn_a_transport_failure_into_needs_password() {
        let err = RemoteError::Connection("no route to host".into());
        assert!(matches!(
            classify_connect_error(&err, true, true),
            SftpConnectError::Unreachable { .. }
        ));
    }

    #[test]
    fn host_key_and_misc_failures_are_other() {
        let known_hosts = RemoteError::KnownHosts("key mismatch".into());
        assert!(matches!(
            classify_connect_error(&known_hosts, false, false),
            SftpConnectError::Other { .. }
        ));
        let misc = RemoteError::Other("something else".into());
        assert!(matches!(
            classify_connect_error(&misc, false, false),
            SftpConnectError::Other { .. }
        ));
    }

    // -----------------------------------------------------------------
    // Usable-password rung
    // -----------------------------------------------------------------

    fn creds(auth_method: &str, password: Option<&str>) -> SshCredentials {
        SshCredentials {
            username: "deploy".into(),
            auth_method: auth_method.into(),
            password: password.map(str::to_string),
            key_path: None,
            key_passphrase: None,
        }
    }

    #[test]
    fn missing_and_empty_passwords_are_not_usable() {
        assert!(!has_usable_password(&creds("password", None)));
        assert!(!has_usable_password(&creds("password", Some(""))));
        assert!(has_usable_password(&creds("password", Some("hunter2"))));
    }

    // -----------------------------------------------------------------
    // Callbacks: the frontend owns password prompts
    // -----------------------------------------------------------------

    struct StubCallbacks {
        host_key_answer: bool,
        prompted: std::sync::Arc<AtomicBool>,
    }

    #[async_trait::async_trait]
    impl RemoteCallbacks for StubCallbacks {
        async fn verify_host_key(&self, _message: &str, _fingerprint: &str) -> bool {
            self.host_key_answer
        }
        async fn prompt_password(&self, _message: &str) -> Option<String> {
            self.prompted.store(true, Ordering::SeqCst);
            Some("should-never-be-used".into())
        }
        fn on_transfer_progress(&self, _id: &str, _bytes: u64, _total: Option<u64>) {}
    }

    #[tokio::test]
    async fn password_prompts_are_suppressed_and_flagged() {
        let prompted = std::sync::Arc::new(AtomicBool::new(false));
        let suppressed = std::sync::Arc::new(AtomicBool::new(false));
        let callbacks = DetachedCallbacks {
            inner: std::sync::Arc::new(StubCallbacks {
                host_key_answer: true,
                prompted: std::sync::Arc::clone(&prompted),
            }),
            password_prompt_suppressed: std::sync::Arc::clone(&suppressed),
        };

        assert!(
            callbacks
                .prompt_password("Password for deploy@host")
                .await
                .is_none()
        );
        assert!(suppressed.load(Ordering::SeqCst));
        assert!(
            !prompted.load(Ordering::SeqCst),
            "the interactive prompt must never be reached from a detached connect"
        );
    }

    #[tokio::test]
    async fn host_key_verification_still_delegates() {
        let prompted = std::sync::Arc::new(AtomicBool::new(false));
        let callbacks = DetachedCallbacks {
            inner: std::sync::Arc::new(StubCallbacks {
                host_key_answer: true,
                prompted,
            }),
            password_prompt_suppressed: std::sync::Arc::new(AtomicBool::new(false)),
        };
        assert!(callbacks.verify_host_key("m", "fp").await);
    }

    // -----------------------------------------------------------------
    // Wire shapes (Tasks 3 and 4 bind to these exactly)
    // -----------------------------------------------------------------

    #[test]
    fn connected_session_serializes_camel_case() {
        let session = ConnectedSession {
            session_key: "main:1000000".into(),
            host: "build.example.com".into(),
            user: "deploy".into(),
            port: 22,
            pane_id: 1_000_000,
        };
        let json = serde_json::to_value(&session).unwrap();
        assert_eq!(json["sessionKey"], "main:1000000");
        assert_eq!(json["host"], "build.example.com");
        assert_eq!(json["user"], "deploy");
        assert_eq!(json["port"], 22);
        assert_eq!(json["paneId"], 1_000_000);
    }

    #[test]
    fn connect_errors_serialize_tagged_and_camel_case() {
        let locked = serde_json::to_value(SftpConnectError::VaultLocked).unwrap();
        assert_eq!(locked["kind"], "vaultLocked");

        let needs = serde_json::to_value(SftpConnectError::NeedsPassword {
            has_vault_account: true,
        })
        .unwrap();
        assert_eq!(needs["kind"], "needsPassword");
        assert_eq!(needs["hasVaultAccount"], true);

        let failed = serde_json::to_value(SftpConnectError::AuthFailed {
            message: "nope".into(),
        })
        .unwrap();
        assert_eq!(failed["kind"], "authFailed");
        assert_eq!(failed["message"], "nope");

        let unreachable = serde_json::to_value(SftpConnectError::Unreachable {
            message: "no route".into(),
        })
        .unwrap();
        assert_eq!(unreachable["kind"], "unreachable");

        let other = serde_json::to_value(SftpConnectError::Other {
            message: "boom".into(),
        })
        .unwrap();
        assert_eq!(other["kind"], "other");
    }

    // -----------------------------------------------------------------
    // Disconnect: key validation
    // -----------------------------------------------------------------

    #[test]
    fn disconnect_accepts_a_detached_key_for_this_window() {
        let key = format!("main:{DETACHED_PANE_ID_BASE}");
        assert_eq!(
            detached_pane_id_for_window(&key, "main").unwrap(),
            DETACHED_PANE_ID_BASE
        );
    }

    #[test]
    fn disconnect_refuses_terminal_owned_pane_ids() {
        let err = detached_pane_id_for_window("main:3", "main").unwrap_err();
        assert_eq!(err, NOT_A_DETACHED_SESSION);
        let boundary = format!("main:{}", DETACHED_PANE_ID_BASE - 1);
        assert_eq!(
            detached_pane_id_for_window(&boundary, "main").unwrap_err(),
            NOT_A_DETACHED_SESSION
        );
    }

    #[test]
    fn disconnect_refuses_another_windows_session() {
        let key = format!("window-2:{DETACHED_PANE_ID_BASE}");
        assert_eq!(
            detached_pane_id_for_window(&key, "main").unwrap_err(),
            NOT_A_DETACHED_SESSION
        );
    }

    #[test]
    fn disconnect_refusals_are_indistinguishable() {
        // The refusal must not tell a caller whether the session exists, is
        // terminal-owned, belongs to another window, or was never a key.
        let cases = [
            "main:3",
            "main:not-a-number",
            "main:",
            "garbage",
            "window-2:1000000",
        ];
        for case in cases {
            assert_eq!(
                detached_pane_id_for_window(case, "main").unwrap_err(),
                NOT_A_DETACHED_SESSION,
                "case {case} leaked a distinguishable message"
            );
        }
    }

    // -----------------------------------------------------------------
    // Disconnect: teardown
    // -----------------------------------------------------------------

    #[test]
    fn removing_a_detached_session_is_exactly_once() {
        let mut state = super::super::test_remote_state();
        let key = format!("main:{DETACHED_PANE_ID_BASE}");
        state.sessions.insert(
            key.clone(),
            detached_session(
                "conn:main:1000000".into(),
                "build.example.com".into(),
                "deploy".into(),
                22,
            ),
        );
        state.pane_cwds.insert(key.clone(), "/home/deploy".into());
        state
            .pane_home_dirs
            .insert(key.clone(), "/home/deploy".into());

        assert!(remove_detached_session(&mut state, &key));
        assert!(state.sessions.is_empty());
        assert!(state.pane_cwds.is_empty(), "pane cwd state must be cleared");
        assert!(state.pane_home_dirs.is_empty());

        assert!(
            !remove_detached_session(&mut state, &key),
            "a second teardown must be a no-op"
        );
    }

    #[test]
    fn removing_a_session_leaves_other_sessions_alone() {
        let mut state = super::super::test_remote_state();
        let mine = format!("main:{DETACHED_PANE_ID_BASE}");
        let theirs = format!("main:{}", DETACHED_PANE_ID_BASE + 1);
        for key in [&mine, &theirs] {
            state.sessions.insert(
                key.clone(),
                detached_session("conn:x".into(), "h".into(), "u".into(), 22),
            );
        }
        assert!(remove_detached_session(&mut state, &mine));
        assert_eq!(state.sessions.len(), 1);
        assert!(state.sessions.contains_key(&theirs));
    }

    #[test]
    fn connection_refs_drop_only_when_the_last_session_goes() {
        assert_eq!(release_connection_ref(2), (1, false));
        assert_eq!(release_connection_ref(1), (0, true));
        // Saturating, mirroring the channel-loop cleanup in mod.rs.
        assert_eq!(release_connection_ref(0), (0, true));
    }

    #[test]
    fn a_detached_session_has_no_live_channel() {
        let session = detached_session("conn:main:1000000".into(), "h".into(), "u".into(), 22);
        assert!(session.abort_handle.is_none());
        assert!(
            session
                .input_tx
                .send(termlab_remote::ssh::ChannelInput::Shutdown)
                .is_err(),
            "the placeholder input channel must be closed, not silently swallowing writes"
        );
    }
}

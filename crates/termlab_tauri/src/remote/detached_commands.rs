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
use crate::vault_commands::{AddAccountRequest, UpdateAccountRequest, VaultState};

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
/// `u32::MAX` (≈4.29 billion detached connects in one process); it exists so
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
// In-flight connects
// ---------------------------------------------------------------------------

/// Connects currently running, keyed `(resolved window label, server entry id)`.
///
/// The duplicate guard (`find_detached_session_for_entry`) can only see a
/// session that already EXISTS; between a connect starting and its session
/// being registered there is a window — the whole network round-trip, plus a
/// password dialog on the with-password path — in which a second submit sees
/// nothing and races. This closes that window for both connect commands at
/// once, so a password submit cannot race a plain connect either.
static IN_FLIGHT_CONNECTS: Mutex<Option<std::collections::HashSet<(String, String)>>> =
    Mutex::new(None);

/// Releases its key when dropped, so every exit from a connect command —
/// success, typed error, or an early `?` — clears the entry.
struct InFlightGuard<'a> {
    registry: &'a Mutex<Option<std::collections::HashSet<(String, String)>>>,
    key: (String, String),
}

impl Drop for InFlightGuard<'_> {
    fn drop(&mut self) {
        if let Some(set) = self.registry.lock().as_mut() {
            set.remove(&self.key);
        }
    }
}

/// Claim `(window_label, entry_id)`, or `None` if a connect for it is already
/// running.
fn begin_connect<'a>(
    registry: &'a Mutex<Option<std::collections::HashSet<(String, String)>>>,
    window_label: &str,
    entry_id: &str,
) -> Option<InFlightGuard<'a>> {
    let key = (window_label.to_string(), entry_id.to_string());
    let mut guard = registry.lock();
    let set = guard.get_or_insert_with(std::collections::HashSet::new);
    if !set.insert(key.clone()) {
        return None;
    }
    drop(guard);
    Some(InFlightGuard { registry, key })
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
    /// A connect for this same host, in this same window, is already running.
    /// Retry once the one in flight settles: it will either have produced the
    /// session (which the next `sftp_connect_host` hands straight back) or
    /// failed with a variant of its own.
    ConnectInProgress,
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
/// VALID FOR `connect_and_auth` ONLY. `connect_and_open_shell` wraps it and
/// then emits `RemoteError::Connection` from three POST-auth channel steps
/// (channel open, PTY request, shell request) — routing a detached connect
/// through that function instead would silently report a post-auth failure as
/// `Unreachable`.
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
    server_entry_id: String,
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
        server_entry_id: Some(server_entry_id),
    }
}

/// The live detached session this window already holds for `entry_id`, if any.
///
/// The terminal path's duplicate guard is per-pane (`establish_ssh_session`
/// rejects a pane that already has a session), which does not translate here:
/// every detached connect mints a FRESH pane id, so without this the same host
/// picked twice would open a second TCP+SSH connection and list twice in the
/// chooser. The guard is per (window, entry) because that is the identity the
/// dropdown offers — a different window is a genuinely independent connection
/// (a stated product rule), and a terminal session on the same host is a
/// different thing that must not be handed out here (its pane id is below the
/// base, and it carries no `server_entry_id` at all).
fn find_detached_session_for_entry(
    state: &RemoteState,
    window_label: &str,
    entry_id: &str,
) -> Option<ConnectedSession> {
    state.sessions.iter().find_map(|(key, session)| {
        if session.server_entry_id.as_deref() != Some(entry_id) {
            return None;
        }
        let pane_id = detached_pane_id_for_window(key, window_label).ok()?;
        Some(ConnectedSession {
            session_key: key.clone(),
            host: session.host.clone(),
            user: session.user.clone(),
            port: session.port,
            pane_id,
        })
    })
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
            server.id.clone(),
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

/// Every rung of the ladder that needs no socket, as a pure function of what
/// `try_vault_credentials` answered and what the entry says.
///
/// Extracted from `sftp_connect_host` so each rung is testable with the vault
/// fixture harness (a real `VaultManager` in a tempdir) instead of only being
/// reachable through a `#[tauri::command]` that needs a live window and a live
/// server. `Ok` means "there is an attempt worth making"; every `Err` is a
/// decision the frontend must act on.
fn resolve_credentials(
    vault_result: Result<Option<SshCredentials>, String>,
    server: &ServerEntry,
) -> Result<SshCredentials, SftpConnectError> {
    let has_vault_account = server.vault_account_id.is_some();

    let credentials = match vault_result {
        // A sealed vault stops the ladder here: unlocking is the user's
        // decision, and falling through to key auth would silently connect as
        // somebody other than the account they linked.
        Err(e) if e == VAULT_LOCKED => return Err(SftpConnectError::VaultLocked),
        Err(message) => return Err(SftpConnectError::Other { message }),
        Ok(Some(credentials)) => credentials,
        Ok(None) => credentials_from_server(server, None),
    };

    // Password auth with nothing to send is decided before any socket: there is
    // no attempt that could succeed, and the frontend owns the prompt.
    if credentials.auth_method == "password" && !has_usable_password(&credentials) {
        return Err(SftpConnectError::NeedsPassword { has_vault_account });
    }

    Ok(credentials)
}

/// Which username a user-typed password should be tried against: the linked
/// vault account's, when there is one, else the entry's (else `root`) — the
/// same precedence `ssh_connect` applies. A vault error other than a sealed
/// vault also lands on the entry's user: this path already has a password in
/// hand, so the only thing a broken account link changes is the name to send.
fn resolve_password_username(
    vault_result: Result<Option<SshCredentials>, String>,
    server: &ServerEntry,
) -> String {
    match vault_result {
        Ok(Some(credentials)) => credentials.username.clone(),
        _ => credentials_from_server(server, None).username.clone(),
    }
}

/// Tell every window that the set of live sessions changed.
///
/// The one place the event name lives; `cleanup` calls it too, so a window
/// closing with detached sessions refreshes the OTHER windows' dropdowns
/// instead of leaving them listing sessions that are gone.
pub(crate) fn emit_sessions_changed<R: tauri::Runtime>(app: &tauri::AppHandle<R>) {
    let _ = app.emit(SESSIONS_CHANGED_EVENT, ());
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

    emit_sessions_changed(app);
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

    // Picking the same host twice hands back the session that is already open
    // rather than opening a second connection to it.
    if let Some(existing) = {
        let state = remote.lock();
        find_detached_session_for_entry(&state, &window_label, &server.id)
    } {
        return Ok(existing);
    }

    // Nothing registered yet — but a connect for this host may still be in
    // flight, with its session not yet inserted for the check above to find.
    let _in_flight = begin_connect(&IN_FLIGHT_CONNECTS, &window_label, &server.id)
        .ok_or(SftpConnectError::ConnectInProgress)?;

    let has_vault_account = server.vault_account_id.is_some();
    let credentials = resolve_credentials(try_vault_credentials(&vault, &server), &server)?;

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

    // Shares the plain connect's key, so a double-submitted password dialog —
    // or a password submit racing a plain connect for the same host — is told
    // to wait rather than opening a second connection and, with
    // `save_to_vault`, racing the vault link.
    let _in_flight = begin_connect(&IN_FLIGHT_CONNECTS, &window_label, &server.id)
        .ok_or(SftpConnectError::ConnectInProgress)?;

    let has_vault_account = server.vault_account_id.is_some();
    let username = resolve_password_username(try_vault_credentials(&vault, &server), &server);

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
        && let Err(e) =
            link_password_account(&vault, &remote_state, &server, &credentials.username, pw)
    {
        // The connection itself succeeded, which is what the user asked for; a
        // failed vault write (sealed vault, disk error) is reported as a
        // warning rather than failing the connect. `e` comes from the
        // vault/config layer and carries no secret.
        log::warn!("could not link a vault account for '{}': {e}", server.label);
    }

    Ok(session)
}

/// Store `password` in the vault for `server`, and make sure the entry points
/// at the account holding it.
///
/// Two paths, because the entry may already be linked — that is exactly the
/// `NeedsPassword { hasVaultAccount: true }` rung, an account whose stored
/// password is the eager-import `""` placeholder:
///
/// - **Already linked** → UPDATE that account (`vault_update_account`). Adding
///   a second account and repointing the entry would orphan the first one in
///   the vault with no UI affordance pointing at it, one per attempt.
/// - **Not linked** → create one (`vault_add_account`) and persist the link
///   through the same upsert `remote_save_server` uses, so it survives restart.
///
/// Both go through the vault commands' own bodies, so nothing about encryption
/// or on-disk format is duplicated here.
fn link_password_account(
    vault: &VaultState,
    remote: &Arc<Mutex<RemoteState>>,
    server: &ServerEntry,
    username: &str,
    password: &str,
) -> Result<uuid::Uuid, String> {
    // ONE acquisition covers re-read → decide → write. The `server` this
    // command captured at `resolve_server` time is a SNAPSHOT: by the time a
    // password has been typed and a connection made, another call (or the Hosts
    // UI) may have linked the entry already. Branching on the snapshot's
    // `vault_account_id` would send both racers down the create path, and the
    // second `save_server_preserving_folder` — a full remove-and-re-add, not a
    // merge — would drop the first one's link, orphaning its account. Reading
    // the live entry under the same lock that publishes the write makes the
    // decision and the write atomic with respect to each other.
    let mut state = remote.lock();
    let current = find_server_by_entry_id(&state, Some(&server.id)).unwrap_or_else(|| {
        // Only reachable if the entry was deleted mid-connect; the snapshot is
        // still the best description of what the user connected to.
        server.clone()
    });

    if let Some(account_id) = current.vault_account_id {
        let account = {
            let mgr = vault.lock();
            mgr.get_account(account_id).map_err(|e| e.to_string())?
        };
        crate::vault_commands::update_account(
            vault,
            password_update_request(account_id, &account, username, password),
        )?;
        // The entry already points at this account — nothing to persist.
        return Ok(account_id);
    }

    let account_id = crate::vault_commands::add_account(
        vault,
        AddAccountRequest {
            display_name: current.label.clone(),
            username: username.to_string(),
            auth_type: "password".to_string(),
            password: Some(password.to_string()),
            key_path: None,
            passphrase: None,
        },
    )?;

    // Written from `current`, not from the snapshot, so an unrelated edit made
    // while this connect was in flight (a rename, a new proxy) is not reverted.
    let mut entry = current;
    entry.vault_account_id = Some(account_id);
    // Note: for a host that came from `~/.ssh/config` this writes a config-owned
    // copy of the entry, which is the same promotion the Hosts UI performs when
    // an imported host is saved — an ssh-config entry is re-parsed on every
    // launch and could not otherwise carry a vault link.
    save_server_preserving_folder(&mut state, entry);
    Ok(account_id)
}

/// The update that stores `password` on an existing account WITHOUT discarding
/// a key the account already carries.
///
/// `vault_update_account` replaces the whole `AuthMethod`, so an account that
/// authenticates with a key plus a password (or with a key alone, whose owner
/// has now typed a password) must be rewritten as `key_and_password` carrying
/// the original key path and passphrase forward — writing a bare `password`
/// would silently drop the key.
fn password_update_request(
    account_id: uuid::Uuid,
    account: &termlab_vault::VaultAccount,
    username: &str,
    password: &str,
) -> UpdateAccountRequest {
    let (auth_type, key_path, passphrase) = match &account.auth {
        termlab_vault::AuthMethod::Password(_) => ("password", None, None),
        termlab_vault::AuthMethod::Key { path, passphrase } => (
            "key_and_password",
            Some(path.display().to_string()),
            passphrase.clone(),
        ),
        termlab_vault::AuthMethod::KeyAndPassword {
            key_path,
            passphrase,
            ..
        } => (
            "key_and_password",
            Some(key_path.display().to_string()),
            passphrase.clone(),
        ),
    };
    crate::vault_commands::UpdateAccountRequest {
        id: account_id,
        display_name: None,
        username: Some(username.to_string()),
        auth_type: Some(auth_type.to_string()),
        password: Some(password.to_string()),
        key_path,
        passphrase,
    }
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
        emit_sessions_changed(&app);
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
    // The pre-socket ladder (rungs inside sftp_connect_host)
    // -----------------------------------------------------------------

    /// The fixture idiom the existing `try_vault_credentials_*` tests use: a
    /// real `VaultManager` in a tempdir, no network.
    /// The tempdir is returned, not dropped: the vault file must outlive the
    /// manager for `save()` to keep working.
    fn vault_with_account(
        auth: termlab_vault::AuthMethod,
    ) -> (VaultState, uuid::Uuid, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let mgr = termlab_vault::VaultManager::new(dir.path().join("vault.enc"));
        mgr.create(b"test-password").unwrap();
        let id = mgr
            .add_account("Deploy".into(), "deploy".into(), auth)
            .unwrap();
        (std::sync::Arc::new(Mutex::new(mgr)), id, dir)
    }

    fn server_entry(auth_method: Option<&str>) -> ServerEntry {
        ServerEntry {
            id: "entry-1".into(),
            label: "build-box".into(),
            host: "build.example.com".into(),
            port: 22,
            user: Some("ops".into()),
            auth_method: auth_method.map(str::to_string),
            key_path: None,
            vault_account_id: None,
            proxy_command: None,
            proxy_jump: None,
        }
    }

    #[test]
    fn a_sealed_vault_stops_the_ladder_before_anything_else() {
        // Driven through the real `try_vault_credentials`, so the sentinel this
        // depends on is pinned end to end, not just its literal.
        let (vault, account_id, _vault_dir) =
            vault_with_account(termlab_vault::AuthMethod::Password("pw".into()));
        vault.lock().seal();
        let mut server = server_entry(Some("key"));
        server.vault_account_id = Some(account_id);

        let result = resolve_credentials(try_vault_credentials(&vault, &server), &server);
        assert!(
            matches!(result, Err(SftpConnectError::VaultLocked)),
            "a sealed vault must not fall through to a key attempt"
        );
    }

    #[test]
    fn a_broken_account_link_is_other_not_a_connect_attempt() {
        let (vault, _id, _vault_dir) =
            vault_with_account(termlab_vault::AuthMethod::Password("pw".into()));
        let mut server = server_entry(Some("key"));
        server.vault_account_id = Some(uuid::Uuid::new_v4()); // deleted account

        match resolve_credentials(try_vault_credentials(&vault, &server), &server) {
            Err(SftpConnectError::Other { message }) => assert!(message.contains("not found")),
            Err(other) => panic!("expected Other, got {other:?}"),
            Ok(_) => panic!("a broken account link must not produce a connect attempt"),
        }
    }

    #[test]
    fn a_stored_but_empty_password_needs_a_password_with_the_account_flag() {
        // The eager-import `""` placeholder — an account exists, so the dialog
        // should offer "Save to vault" checked.
        let (vault, account_id, _vault_dir) =
            vault_with_account(termlab_vault::AuthMethod::Password(String::new()));
        let mut server = server_entry(Some("password"));
        server.vault_account_id = Some(account_id);

        assert!(matches!(
            resolve_credentials(try_vault_credentials(&vault, &server), &server),
            Err(SftpConnectError::NeedsPassword {
                has_vault_account: true
            })
        ));
    }

    #[test]
    fn a_password_entry_with_no_account_needs_a_password_before_any_socket() {
        let server = server_entry(Some("password"));
        assert!(matches!(
            resolve_credentials(Ok(None), &server),
            Err(SftpConnectError::NeedsPassword {
                has_vault_account: false
            })
        ));
    }

    #[test]
    fn no_vault_account_falls_back_to_the_entrys_own_fields() {
        let server = server_entry(Some("key"));
        let credentials =
            resolve_credentials(Ok(None), &server).expect("a key attempt is worth making");
        assert_eq!(credentials.username, "ops");
        assert_eq!(credentials.auth_method, "key");

        // An entry with no auth_method at all defaults to key, and is attempted.
        let bare = server_entry(None);
        let credentials = resolve_credentials(Ok(None), &bare).unwrap();
        assert_eq!(credentials.auth_method, "key");
    }

    #[test]
    fn a_usable_vault_password_is_returned_for_the_attempt() {
        let (vault, account_id, _vault_dir) =
            vault_with_account(termlab_vault::AuthMethod::Password("pw123".into()));
        let mut server = server_entry(Some("key"));
        server.vault_account_id = Some(account_id);

        let credentials = resolve_credentials(try_vault_credentials(&vault, &server), &server)
            .expect("a stored password is worth attempting");
        assert_eq!(credentials.username, "deploy", "the account's user wins");
        assert_eq!(credentials.auth_method, "password");
    }

    #[test]
    fn the_password_path_prefers_the_linked_accounts_username() {
        let (vault, account_id, _vault_dir) =
            vault_with_account(termlab_vault::AuthMethod::Password(String::new()));
        let mut server = server_entry(Some("password"));
        server.vault_account_id = Some(account_id);
        assert_eq!(
            resolve_password_username(try_vault_credentials(&vault, &server), &server),
            "deploy"
        );

        // No account → the entry's user; no user either → root.
        let server = server_entry(Some("password"));
        assert_eq!(resolve_password_username(Ok(None), &server), "ops");
        let mut bare = server_entry(Some("password"));
        bare.user = None;
        assert_eq!(resolve_password_username(Ok(None), &bare), "root");

        // A sealed vault still yields a username to try the typed password with.
        assert_eq!(
            resolve_password_username(Err(VAULT_LOCKED.to_string()), &server),
            "ops"
        );
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

        let in_flight = serde_json::to_value(SftpConnectError::ConnectInProgress).unwrap();
        assert_eq!(in_flight["kind"], "connectInProgress");
    }

    // -----------------------------------------------------------------
    // Duplicate-connect guard
    // -----------------------------------------------------------------

    fn insert_detached(state: &mut RemoteState, key: &str, entry_id: &str) {
        state.sessions.insert(
            key.to_string(),
            detached_session(
                format!("conn:{key}"),
                "build.example.com".into(),
                "deploy".into(),
                22,
                entry_id.to_string(),
            ),
        );
    }

    #[test]
    fn a_second_connect_finds_the_session_already_open() {
        let mut state = super::super::test_remote_state();
        let key = format!("main:{DETACHED_PANE_ID_BASE}");
        insert_detached(&mut state, &key, "entry-1");

        let found = find_detached_session_for_entry(&state, "main", "entry-1")
            .expect("the live session for this entry must be reused");
        assert_eq!(found.session_key, key);
        assert_eq!(found.pane_id, DETACHED_PANE_ID_BASE);
        assert_eq!(found.host, "build.example.com");
        assert_eq!(found.user, "deploy");
        assert_eq!(found.port, 22);
    }

    #[test]
    fn the_guard_is_scoped_to_this_window_and_this_entry() {
        let mut state = super::super::test_remote_state();
        insert_detached(
            &mut state,
            &format!("main:{DETACHED_PANE_ID_BASE}"),
            "entry-1",
        );

        assert!(
            find_detached_session_for_entry(&state, "main", "entry-2").is_none(),
            "a different host must open its own connection"
        );
        assert!(
            find_detached_session_for_entry(&state, "window-2", "entry-1").is_none(),
            "another window connects independently — a stated product rule"
        );
    }

    #[test]
    fn a_terminal_session_is_never_handed_out_as_a_detached_one() {
        let mut state = super::super::test_remote_state();
        // Same entry id, but a terminal-range pane id: must not be reused.
        insert_detached(&mut state, "main:3", "entry-1");
        assert!(find_detached_session_for_entry(&state, "main", "entry-1").is_none());

        // And a real terminal session carries no entry id at all.
        state.sessions.insert(
            "main:4".into(),
            SshSession {
                input_tx: mpsc::unbounded_channel().0,
                connection_id: "conn:main:4".into(),
                host: "build.example.com".into(),
                user: "deploy".into(),
                port: 22,
                abort_handle: None,
                server_entry_id: None,
            },
        );
        assert!(find_detached_session_for_entry(&state, "main", "entry-1").is_none());
    }

    // -----------------------------------------------------------------
    // Vault linking: update in place, never orphan
    // -----------------------------------------------------------------

    fn account_by_id(vault: &VaultState, id: uuid::Uuid) -> termlab_vault::VaultAccount {
        vault.lock().get_account(id).unwrap()
    }

    fn linked_state() -> (std::sync::Arc<Mutex<RemoteState>>, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let mut state = super::super::test_remote_state();
        state.paths.config_dir = dir.path().to_path_buf();
        (std::sync::Arc::new(Mutex::new(state)), dir)
    }

    #[test]
    fn saving_to_an_unlinked_host_creates_and_links_an_account() {
        let (vault, _existing, _vault_dir) =
            vault_with_account(termlab_vault::AuthMethod::Password("other".into()));
        let (remote, _dir) = linked_state();
        let server = server_entry(Some("password"));

        let id = link_password_account(&vault, &remote, &server, "deploy", "hunter2").unwrap();

        assert_eq!(
            vault.lock().list_accounts().unwrap().len(),
            2,
            "one pre-existing unrelated account plus the new one"
        );
        assert!(
            matches!(account_by_id(&vault, id).auth, termlab_vault::AuthMethod::Password(ref p) if p == "hunter2")
        );
        assert_eq!(
            remote
                .lock()
                .config
                .find_server("entry-1")
                .unwrap()
                .vault_account_id,
            Some(id),
            "the entry must be linked and persisted"
        );
    }

    #[test]
    fn saving_to_an_already_linked_host_updates_it_and_orphans_nothing() {
        // The `hasVaultAccount: true` rung: an account whose stored password is
        // the eager-import placeholder.
        let (vault, account_id, _vault_dir) =
            vault_with_account(termlab_vault::AuthMethod::Password(String::new()));
        let (remote, _dir) = linked_state();
        let mut server = server_entry(Some("password"));
        server.vault_account_id = Some(account_id);

        for attempt in ["first", "second"] {
            let id = link_password_account(&vault, &remote, &server, "deploy", attempt).unwrap();
            assert_eq!(id, account_id, "the existing account must be reused");
            assert_eq!(
                vault.lock().list_accounts().unwrap().len(),
                1,
                "repeating the flow must not accumulate orphaned accounts"
            );
            assert!(
                matches!(account_by_id(&vault, account_id).auth, termlab_vault::AuthMethod::Password(ref p) if p == attempt)
            );
        }
    }

    #[test]
    fn two_racing_saves_link_one_account_and_orphan_nothing() {
        // Both callers hold the SNAPSHOT they resolved before connecting, in
        // which the entry is still unlinked — exactly what two concurrent
        // `sftp_connect_host_with_password(save_to_vault: true)` calls have.
        // Running them one after the other with that stale value reproduces the
        // interleaving without threads: the second must observe the first's
        // link through live state, not through its own snapshot.
        let (vault, _existing, _vault_dir) =
            vault_with_account(termlab_vault::AuthMethod::Password("unrelated".into()));
        let (remote, _dir) = linked_state();
        let stale = server_entry(Some("password"));
        assert!(stale.vault_account_id.is_none());
        remote.lock().config.add_server(stale.clone());

        let first = link_password_account(&vault, &remote, &stale, "deploy", "first").unwrap();
        let second = link_password_account(&vault, &remote, &stale, "deploy", "second").unwrap();

        assert_eq!(
            first, second,
            "the second save must find and update the first's account"
        );
        assert_eq!(
            vault.lock().list_accounts().unwrap().len(),
            2,
            "one unrelated account plus ONE linked account — no orphan"
        );
        assert!(
            matches!(account_by_id(&vault, first).auth, termlab_vault::AuthMethod::Password(ref p) if p == "second")
        );
        assert_eq!(
            remote
                .lock()
                .config
                .find_server("entry-1")
                .unwrap()
                .vault_account_id,
            Some(first),
            "the surviving link must be the account that actually holds the password"
        );
    }

    #[test]
    fn racing_saves_from_two_threads_still_link_one_account() {
        let (vault, _existing, _vault_dir) =
            vault_with_account(termlab_vault::AuthMethod::Password("unrelated".into()));
        let (remote, _dir) = linked_state();
        let stale = server_entry(Some("password"));
        remote.lock().config.add_server(stale.clone());

        let handles: Vec<_> = ["a", "b"]
            .into_iter()
            .map(|pw| {
                let vault = std::sync::Arc::clone(&vault);
                let remote = std::sync::Arc::clone(&remote);
                let server = stale.clone();
                std::thread::spawn(move || {
                    link_password_account(&vault, &remote, &server, "deploy", pw).unwrap()
                })
            })
            .collect();
        let ids: Vec<uuid::Uuid> = handles.into_iter().map(|h| h.join().unwrap()).collect();

        assert_eq!(ids[0], ids[1], "both threads must converge on one account");
        assert_eq!(vault.lock().list_accounts().unwrap().len(), 2);
        assert_eq!(
            remote
                .lock()
                .config
                .find_server("entry-1")
                .unwrap()
                .vault_account_id,
            Some(ids[0])
        );
    }

    #[test]
    fn a_link_written_after_the_snapshot_is_respected() {
        // The Hosts UI (or another window) links the entry while this connect
        // is in flight: the save must update that account, not create a rival.
        let (vault, account_id, _vault_dir) =
            vault_with_account(termlab_vault::AuthMethod::Password(String::new()));
        let (remote, _dir) = linked_state();
        let stale = server_entry(Some("password"));

        let mut linked = stale.clone();
        linked.vault_account_id = Some(account_id);
        linked.label = "renamed by someone else".into();
        remote.lock().config.add_server(linked);

        let id = link_password_account(&vault, &remote, &stale, "deploy", "hunter2").unwrap();

        assert_eq!(id, account_id);
        assert_eq!(vault.lock().list_accounts().unwrap().len(), 1);
        assert_eq!(
            remote.lock().config.find_server("entry-1").unwrap().label,
            "renamed by someone else",
            "a concurrent edit must not be reverted by this save"
        );
    }

    // -----------------------------------------------------------------
    // In-flight guard
    // -----------------------------------------------------------------

    fn empty_registry() -> Mutex<Option<std::collections::HashSet<(String, String)>>> {
        Mutex::new(None)
    }

    #[test]
    fn a_connect_already_running_blocks_a_second_one_until_it_finishes() {
        let registry = empty_registry();
        let first = begin_connect(&registry, "main", "entry-1").expect("first claim");
        assert!(
            begin_connect(&registry, "main", "entry-1").is_none(),
            "a second connect for the same host must be told one is in flight"
        );
        drop(first);
        assert!(
            begin_connect(&registry, "main", "entry-1").is_some(),
            "the key must be released however the first call exits"
        );
    }

    #[test]
    fn in_flight_connects_are_scoped_to_window_and_entry() {
        let registry = empty_registry();
        let _first = begin_connect(&registry, "main", "entry-1").unwrap();
        assert!(
            begin_connect(&registry, "main", "entry-2").is_some(),
            "a different host connects freely"
        );
        assert!(
            begin_connect(&registry, "window-2", "entry-1").is_some(),
            "another window connects independently"
        );
    }

    #[test]
    fn updating_an_account_never_discards_its_key() {
        let key_path = std::path::PathBuf::from("/home/deploy/.ssh/id_ed25519");
        let (vault, account_id, _vault_dir) =
            vault_with_account(termlab_vault::AuthMethod::KeyAndPassword {
                key_path: key_path.clone(),
                passphrase: Some("kp".into()),
                password: String::new(),
            });
        let (remote, _dir) = linked_state();
        let mut server = server_entry(Some("key_and_password"));
        server.vault_account_id = Some(account_id);

        link_password_account(&vault, &remote, &server, "deploy", "hunter2").unwrap();

        match &account_by_id(&vault, account_id).auth {
            termlab_vault::AuthMethod::KeyAndPassword {
                key_path: kp,
                passphrase,
                password,
            } => {
                assert_eq!(kp, &key_path);
                assert_eq!(passphrase.as_deref(), Some("kp"));
                assert_eq!(password, "hunter2");
            }
            // Deliberately not interpolating the auth method: its Debug would
            // render the stored password, even in a test failure message.
            _ => panic!("the key must survive a password save"),
        }
    }

    #[test]
    fn a_key_only_account_is_upgraded_rather_than_overwritten() {
        let account = {
            let (vault, id, _vault_dir) = vault_with_account(termlab_vault::AuthMethod::Key {
                path: std::path::PathBuf::from("/home/deploy/.ssh/id_rsa"),
                passphrase: None,
            });
            account_by_id(&vault, id)
        };
        let request = password_update_request(account.id, &account, "deploy", "hunter2");
        assert_eq!(request.auth_type.as_deref(), Some("key_and_password"));
        assert_eq!(
            request.key_path.as_deref(),
            Some("/home/deploy/.ssh/id_rsa")
        );
        assert_eq!(request.password.as_deref(), Some("hunter2"));
        assert_eq!(request.username.as_deref(), Some("deploy"));
        assert!(
            request.display_name.is_none(),
            "a background save must not rename the user's account"
        );
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
                "entry-1".into(),
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
                detached_session("conn:x".into(), "h".into(), "u".into(), 22, "e".into()),
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
        let session = detached_session(
            "conn:main:1000000".into(),
            "h".into(),
            "u".into(),
            22,
            "e".into(),
        );
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

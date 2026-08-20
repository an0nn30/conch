# SFTP Panel Independent Host Connections — Design

**Status:** Draft
**Date:** 2026-08-19
**Scope:** The SFTP tool window's remote pane gains a host dropdown that connects to any configured host on demand — no terminal tab required — with vault-unlock and password prompts where credentials need them. Connections are first-class sessions the whole app sees.

## Product rules (settled in brainstorm, 2026-08-19)

1. **First-class sessions:** a dropdown-initiated connection registers a real session (like a terminal SSH tab, minus the PTY). It appears in the chooser's Hosts sidebar, the Hosts panel's session awareness, and the editor can save through it. One connection model.
2. **Auth ladder:** vault credentials first (silent when unlocked and stored); locked vault → master-password unlock prompt then retry; no stored password (and key auth failed/absent) → one-shot password dialog with a "Save to vault" checkbox. Prompts chain (unlock → then password prompt if the account still has none). Cancel at any prompt aborts the connect cleanly.
3. **Key auth** (agent/identity files) works silently when configured — no prompt.
4. **No auto-connect, no auto-reconnect:** connecting is always an explicit pick; a dropped session shows the existing error/failed state and reconnect is re-picking the host.

## Context (verified 2026-08-19 at main a062eca)

- Sessions: `RemoteState.sessions` keyed `"{window_label}:{pane_id}"`; shared `SshConnection`s keyed `"conn:{window_label}:{pane_id}"`, referenced by `connection_id` (`remote/mod.rs:488-502`).
- Hosts: `remote_get_servers` returns the configured tree (folders + servers; ssh-config import exists via `remote_import_ssh_config`). `find_server_by_entry_id` (`server_commands.rs:310`).
- Vault: `try_vault_credentials(&vault, &server)` (`ssh_commands.rs:507`, used by the connect path at `:86`), `is_locked` checks, `vault_unlock` (`vault_commands.rs:137`).
- Session consumers: the chooser sidebar and files panel consume `remote_get_sessions` → `ActiveSession {key, host, user, port}`; scope objects carry `paneId` parsed from context; SFTP commands take `pane_id` + resolve the window via the shared caller→parent resolver.

## Design

### Rust: detached sessions (`remote/detached_commands.rs` or beside `ssh_commands.rs` — implementer places it in the module's idiom)

- **Pseudo-pane ids:** a reserved per-window allocator mints `sftp-1`, `sftp-2`, … (string ids in the pane-id position; they can never collide with numeric PTY pane ids). Session key `"{window_label}:sftp-{n}"`, connection key `"conn:{window_label}:sftp-{n}"` — both flowing through the EXISTING `SshSession`/`SshConnection` types unchanged. Anything that parses pane ids as integers must be audited (plan task one) — consumers found to assume numeric ids are fixed to treat ids opaquely, never the reverse.
- **Command:** `sftp_connect_host(window, server_entry_id: String) -> Result<ConnectedSession, SftpConnectError>` where `ConnectedSession {sessionKey, host, user, port, paneId}` (camelCase) and `SftpConnectError` is a typed enum serialized for the frontend: `VaultLocked`, `NeedsPassword {hasVaultAccount: bool}`, `AuthFailed {message}`, `Unreachable {message}`, `Other {message}`. The command attempts: stored vault credentials (when unlocked) → key auth per server config → returns `VaultLocked`/`NeedsPassword` for the frontend to drive prompts. The caller's window label resolves through the shared caller→parent resolver (a popped-out panel connects on behalf of its parent — the session keys under the PARENT's label).
- **Command:** `sftp_connect_host_with_password(window, server_entry_id, password: String, save_to_vault: bool) -> Result<ConnectedSession, SftpConnectError>` — one-shot password attempt; on success with `save_to_vault`, stores via the vault surface the Hosts UI uses (creating the host's account entry if absent — plan task traces the exact calls; password never logged, held only for the duration of the attempt).
- **Command:** `sftp_disconnect(window, session_key) -> Result<(), String>` — tears down a DETACHED session (pseudo-pane ids only); refuses (`Err`) for sessions whose pane id is numeric (terminal-owned — they die with their tab, unchanged today).
- **Events:** the existing session-change notification path (whatever `remote_get_sessions` consumers use to refresh — plan task verifies whether a `sessions-changed` event exists or polling is the pattern; if none exists, emit `remote-sessions-changed` on connect/disconnect and have the chooser/panel refresh on it, kept minimal).

### Frontend: the dropdown (files-panel remote pane toolbar)

- A `tl-combo` in the remote pane toolbar: **"Follow active tab"** (default — today's implicit behavior made visible) → connected sessions (label `user@host` + `(pane N)` disambiguation via the existing `sessionHostLabel`) → separator → configured hosts from `remote_get_servers` flattened with folder prefixes ("Work / build-box"). Picking a session binds the pane (pins it); picking a configured host runs the connect flow then binds; "Follow active tab" unpins.
- Connect-in-progress: combo shows a busy state; failures land in the panel's existing `.fp-error` strip. A detached session bound to the pane shows a disconnect button (⏏) beside the combo; disconnecting returns the pane to "Follow active tab".
- Pinning is per-pane UI state (not persisted in v1).

### Frontend: the auth dialogs (`features/files/connect-auth.js`, shared helpers)

Small tl-dialogs raised in the panel's own window (works docked and in panel hosts — tl-dialog is loaded in both):
- **Vault unlock:** master password field, error line on wrong password (re-prompt), Cancel aborts. On success, retry `sftp_connect_host` once; if it now returns `NeedsPassword`, chain into:
- **Host password:** title `user@host`, password field, `Save to vault` checkbox (checked default when the host has a vault account, unchecked otherwise), Connect/Cancel. Wrong password → error line, re-prompt (attempt counter shown after 2 failures). Cancel aborts.
- Both dialogs clear their password fields on close; values live only in the invoke payload.

### Cross-surface consequences (by construction, verified in testing)

- Chooser sidebar lists detached sessions (it consumes `remote_get_sessions`); the editor saves through them (scope carries the pseudo paneId; SFTP commands accept it); popped-out panels connect on behalf of their parent via the resolver. No chooser-side connect UI in this project.

## Non-Goals

- No auto-connect/reconnect; no connection health polling.
- No chooser-side host dropdown (its sidebar inherits sessions; connecting from the chooser is a later feature).
- No per-pane persistence of the pinned host across restarts (v1).
- No changes to terminal SSH tab lifecycle or keying.
- No new credential storage outside the vault.

## Constraints

- Branch `feat/sftp-connect-host`; CLAUDE.md rules (no main commits, no Co-Authored-By trailers, imperative commits, unit tests required). Never git in `/Users/dustin/projects/TermLab`.
- Zero behavior change for terminal-owned sessions and for the panel when the dropdown stays on "Follow active tab".
- Passwords: never logged, never in error strings, cleared from DOM on dialog close; `save_to_vault` writes only through the existing vault surface.
- Tokens-only CSS; boundary baseline `tl-dialog.js:334` only; suite baselines at branch: 779 cargo, 34 frontend.

## Testing

- **Rust:** pseudo-pane allocator (uniqueness per window, never collides with numeric ids, survives concurrent mints); `SftpConnectError` variant mapping (locked vault → `VaultLocked`; unlocked-but-passwordless account → `NeedsPassword{hasVaultAccount:true}`; no account → `NeedsPassword{hasVaultAccount:false}`; bad host → `Unreachable`); `sftp_disconnect` refuses numeric pane ids and tears down detached ones exactly-once; `remote_get_sessions` includes detached sessions; the numeric-id-consumer audit's fixes each pinned.
- **Frontend:** combo composition (follow + sessions + separator + flattened hosts); pick-host drives connect invoke; `VaultLocked` → unlock dialog → retry chain; `NeedsPassword` → password dialog with checkbox default by `hasVaultAccount`; cancel-at-each-rung aborts with pane unchanged; wrong-password re-prompt; disconnect returns to follow mode; error strip on hard failures; password field cleared on close (assert the DOM).
- **Manual checklist section M:** fresh launch, locked vault, connect to a real host → unlock prompt → connected; wrong master password; host without stored password → prompt → save-to-vault → disconnect → reconnect silently; `[SSH]` editor saves through a dropdown session; chooser sidebar shows it; popped-out SFTP panel runs the whole flow; two windows connect to the same host independently; disconnect refusal is invisible for terminal sessions (no UI offered).

## Known limitations

- A detached session dropped by the network shows as a failed scope on next use; no background health check.
- Pinning resets to "Follow active tab" on app restart.

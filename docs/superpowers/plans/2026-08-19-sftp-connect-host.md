# SFTP Independent Host Connections Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The SFTP panel's remote pane connects to any configured host on demand — vault-first auth with unlock/password prompt chaining — registering first-class sessions the chooser, editor, and panel hosts all see.

**Architecture:** `termlab_remote`'s monolithic connect gains a connect+auth half reusable without a PTY; `termlab_tauri` mints detached sessions under reserved numeric pseudo-pane ids (≥ 1,000,000) flowing through the EXISTING session/connection types and `u32` command signatures unchanged; the panel gets a `tl-combo` host dropdown with an explicit pinning path beside today's follow-the-active-tab gate; auth prompts are tl-dialogs chained off a typed error enum.

**Tech Stack:** russh via `termlab_remote::ssh`, existing vault commands, `tl-combo`, vm-sandbox `.mjs` suites.

**Spec:** `docs/superpowers/specs/2026-08-19-sftp-connect-host-design.md` — with four scouting amendments BINDING NOW (spec text syncs in Task 5): pseudo-pane ids are `u32` from a reserved range ≥ 1,000,000, NOT strings (every command takes `pane_id: u32`; the sequential real allocator starts at 1 — `main-runtime.js:249`, `state.js:11`); ssh-agent support does not exist in `termlab_remote` (identity files only — agent becomes a spec non-goal); vault save is ONE command (`vault_add_account`) plus NEW auto-link wiring (`entry.vault_account_id = uuid` → `remote_save_server`); no sessions event exists — this plan introduces `remote-sessions-changed`.

## Global Constraints

- Branch `feat/sftp-connect-host`; never commit to main; NO Co-Authored-By trailers; imperative commits; unit tests required (CLAUDE.md). Never git in `/Users/dustin/projects/TermLab`.
- Zero behavior change for terminal-owned sessions (numeric ids < 1,000,000) and for the panel while on "Follow active tab"; `connect_and_open_shell`'s auth sequencing byte-identical through the refactor (existing callers and tests untouched).
- Passwords: never logged, never in error strings, cleared from DOM on dialog close (asserted); held only in invoke payloads; vault writes only via `vault_add_account` + `remote_save_server`.
- Tokens-only CSS; boundary baseline `tl-dialog.js:334` only; baselines at branch: 779 cargo, 34 frontend suites. Branch base main@a062eca.

## File Structure

- `crates/termlab_remote/src/ssh.rs` — MODIFY: split `connect_and_open_shell` (:73-231) into `connect_and_auth` (config/proxy/connect/auth, :85-194's logic) returning the authenticated handle, and the PTY half consuming it; `connect_and_open_shell` becomes composition of the two.
- `crates/termlab_tauri/src/remote/detached_commands.rs` — NEW: allocator, `sftp_connect_host`, `sftp_connect_host_with_password`, `sftp_disconnect`, `SftpConnectError`, `remote-sessions-changed` emits; registered in `lib.rs`.
- `frontend/app/features/files/pane-view.js` + `panels/files-panel.js` — MODIFY: toolbar combo (+ re-attach after every `innerHTML` rebuild), pinning path, disconnect affordance.
- `frontend/app/features/files/connect-auth.js` — NEW: the two dialogs + the chain driver.
- Tests: Rust `#[cfg(test)]` in both crates; NEW `scripts/tests/test_sftp_connect.mjs`.

---

### Task 1: Split the SSH connect path (`termlab_remote`)

**Files:** Modify `crates/termlab_remote/src/ssh.rs`. Test: existing `termlab_remote` tests must pass unchanged; new unit tests for the split seam where feasible without a live server (config/proxy assembly, auth-method branching structure — whatever is testable pure; the network path is pinned by existing integration usage).

**Interfaces — Produces:** `pub async fn connect_and_auth(<same config/credential/prompt-callback args as connect_and_open_shell's auth-relevant subset>) -> Result<client::Handle<...>, Error>` — the exact arg list is derived from what :85-194 consumes; `connect_and_open_shell` keeps its public signature and becomes `connect_and_auth(...)` + the PTY half (mirroring how `open_shell_channel` (:405-433) already consumes an existing handle). Auth sequencing (password / key_and_password / key, `try_key_auth` iteration) moves VERBATIM — no behavior change, no reordering, no new auth capability.

- [ ] Read `ssh.rs:73-231` + `:288-326` + `:405-433` fully; extract; `cargo test -p termlab_remote` green; `cargo test --workspace` green (779 — count unchanged unless you add pure tests); clippy clean on the file. Commit: `git commit -m "Split SSH connect-and-auth from PTY channel opening"`

### Task 2: Detached sessions + typed connect commands (`termlab_tauri`)

**Files:** Create `crates/termlab_tauri/src/remote/detached_commands.rs`; modify `remote/mod.rs` (module + any pub(super) surface), `lib.rs` (register commands).

**Interfaces — Consumes:** Task 1's `connect_and_auth`; `try_vault_credentials` (`ssh_commands.rs:507` — returns `Err("VAULT_LOCKED")` when locked, `Ok(None)` when no account); `find_server_by_entry_id` (`server_commands.rs:310`); `session_key`/`connection_key` (`mod.rs:701,705`); `window_registry_resolver::effective_session_window_label` (the caller may be a panel host — the session keys under the PARENT label).
**Produces (Tasks 3-4 rely on exact shapes):**
- `DETACHED_PANE_ID_BASE: u32 = 1_000_000`; per-window allocator (a counter in `RemoteState` keyed by window label, or one process-wide AtomicU32 starting at the base — pick, justify; ids only ever grow).
- `sftp_connect_host(window, server_entry_id: String) -> Result<ConnectedSession, SftpConnectError>`; `ConnectedSession {sessionKey, host, user, port, paneId: u32}` camelCase.
- `SftpConnectError` (serde tagged, camelCase): `VaultLocked`, `NeedsPassword {hasVaultAccount: bool}`, `AuthFailed {message}`, `Unreachable {message}`, `Other {message}`. Mapping: `try_vault_credentials` Err("VAULT_LOCKED") → `VaultLocked`; `Ok(None)` + server `auth_method` of `password` (or key attempt failed for `key_and_password`) → `NeedsPassword{hasVaultAccount:false}`; account exists but yields no usable password → `NeedsPassword{hasVaultAccount:true}`; key-only failure → `AuthFailed`; TCP/DNS failure → `Unreachable`. Passwords never appear in any message field.
- `sftp_connect_host_with_password(window, server_entry_id, password: String, save_to_vault: bool) -> Result<ConnectedSession, SftpConnectError>` — on success with `save_to_vault`: `vault_add_account(AddAccountRequest {display_name: server label, username, auth_type: "password", password: Some(...), key_path: None, passphrase: None})` (`vault_commands.rs:176`, request shape `:70-77`) → set `entry.vault_account_id = uuid` → `remote_save_server` (the NEW auto-link wiring the Hosts UI lacks — `connect-form.js:107` leaves linking manual; this path links automatically; state so in a comment).
- `sftp_disconnect(window, session_key: String) -> Result<(), String>` — parses the pane-id tail; `Err` for ids < 1,000,000 (terminal-owned); exactly-once teardown (session + connection removal mirroring `ssh_disconnect`'s cleanup at `ssh_commands.rs:256` minus PTY parts).
- Emits `remote-sessions-changed` (no payload) app-wide on successful connect and disconnect.

- [ ] TDD: allocator tests (base respected, monotone, per the chosen scope); error-variant mapping tests (each rung, using the vault/server fixtures the existing `ssh_commands` tests use — `try_vault_credentials_*` tests at `:709+` show the harness idiom); disconnect refusal below base + exactly-once; `remote_get_sessions` includes a detached session (registry-level, no live network — construct `SshSession` entries directly the way existing tests do). Full workspace green; clippy. Commit: `git commit -m "Add detached SFTP sessions with typed connect errors"`

### Task 3: The dropdown and pinning path (frontend)

**Files:** Modify `frontend/app/features/files/pane-view.js` (toolbar markup + combo mount), `panels/files-panel.js` (pinning state + gate bypass + disconnect + busy/error states), `frontend/styles/panels.css` (combo/eject styling, tokens only). Test: NEW `scripts/tests/test_sftp_connect.mjs` part 1 (vm-harness idiom; the files-panel harness pattern from `test_panel_host.mjs` scenario 33 loads the real panel modules — reuse it).

**Interfaces — Consumes:** Task 2's commands/shapes; `remote_get_servers` (tree with folders); `remote_get_sessions`; `sessionHostLabel`; `tlCombo.attach` (`ui/tl-combo.js` — the STANDARD combo; its header comment claiming it's unused is stale; and CRITICAL: `renderPane` rebuilds the toolbar via `innerHTML` on every render — `pane-view.js:28` — so `tlCombo.attach` must re-run after every rebuild). **Produces:** `filesPanel.pinRemotePane(sessionKey|null)` (null = follow mode) for Task 4's dialogs to call on success.
**The gate bypass:** `onTabChanged`'s `tab.type !== 'ssh' || !tab.spawned` gate (`files-panel.js:185`) stays untouched for follow mode; pinning bypasses tabs entirely — `pinRemotePane` sets the pane's bound id directly and suppresses `onTabChanged` re-binding while pinned (a `pinnedSessionKey` module flag checked at the top of `onTabChanged`; unpin restores follow behavior on the next tab event). Combo composition: "Follow active tab" → live sessions (label via `sessionHostLabel`, value = sessionKey) → separator → configured hosts flattened with folder prefixes ("Work / build-box", value = server entry id). Picking a host: busy state on the combo → `sftp_connect_host` → on `Ok` pin; on typed errors delegate to Task 4's chain (until Task 4 lands, surface the variant in `.fp-error` — bridge removed by Task 4). Disconnect (⏏, visible only when pinned to a DETACHED session): `sftp_disconnect` → unpin → refresh. Listen for `remote-sessions-changed` → rebuild combo options (and drop a pin whose session vanished → follow mode + error strip note).

- [ ] TDD: combo composition (all three groups + prefixes); re-attach after a forced re-render (pin, force renderPane, assert combo still attached and value preserved); pick-session pins (no connect invoke); pick-host invokes connect with the entry id; pinned suppresses onTabChanged rebinding (drive the delegate with a pane — binding unchanged); unpin restores; disconnect flow; vanished-session pin drop. Full suite green; `node --check`; boundary. Commit: `git commit -m "Add a host dropdown with pinning to the SFTP remote pane"`

### Task 4: The auth dialog chain (frontend)

**Files:** Create `frontend/app/features/files/connect-auth.js`; modify `panels/files-panel.js` (route typed errors into the chain, remove Task 3's bridge), `index.html` (script tag; classic script before files-panel). Test: `test_sftp_connect.mjs` part 2.

**Interfaces — Consumes:** Task 2's `SftpConnectError` variants + both connect commands; `vault_unlock` (`vault_commands.rs:137`); `tlDialog` (loaded in index.html and panel hosts — verify chooser.html isn't involved). **Produces:** `termlabConnectAuth.run(serverEntryId, {invoke, onConnected})` — drives the full chain, resolves with `ConnectedSession` or null (cancelled), never rejects.
**The chain:** `sftp_connect_host` → `VaultLocked` → master-password tl-dialog (error line on wrong password → re-prompt; Cancel → null) → `vault_unlock` → retry `sftp_connect_host` → possibly `NeedsPassword{hasVaultAccount}` → password tl-dialog (title `user@host`; `Save to vault` checkbox defaulting CHECKED when `hasVaultAccount` is true else unchecked; attempt counter shown after 2 failures; Cancel → null) → `sftp_connect_host_with_password` → `AuthFailed` → re-prompt with error line; `Unreachable`/`Other` at any rung → resolve null after surfacing via the caller's error path. Password inputs `type="password"`, `value` cleared in the dialog's onClose before resolve (assert in tests via the harness DOM).

- [ ] TDD: each rung (stubbed invokes returning scripted variant sequences); the full lock→unlock→needs-password→save chain; cancel at each rung → null + no further invokes; wrong-master re-prompt; wrong-password re-prompt + counter at attempt 3; checkbox default by `hasVaultAccount`; DOM password cleared on close (both dialogs); save_to_vault=true passed through. Full suite; `node --check`. Commit: `git commit -m "Chain vault unlock and password prompts for SFTP connects"`

### Task 5: Sweep, checklist section M, spec sync

**Files:** checklist note (section M after L's last step, 146), spec, loose ends.

- [ ] Sweep: full cargo + all frontend suites + boundary + parity/goldens; greps — `DETACHED_PANE_ID_BASE` single-sourced; no password-bearing string in any log/error format (grep `password` in new Rust for format strings); `remote-sessions-changed` emitted only from detached_commands.
- [ ] Checklist M (steps 147+, house voice, `[SSH]` throughout, bold judgment call-outs): fresh launch + locked vault → connect → unlock prompt → connected; wrong master password; no-stored-password host → prompt → save-to-vault → disconnect → reconnect silently (the vault round-trip in anger); cancel at each rung leaves the pane untouched; editor saves through a dropdown session; chooser sidebar lists it; popped-out panel runs the whole flow (auth dialogs in the host window); two windows connect to the same host independently; pin survives tab switching, unpin follows again; dead-network host → Unreachable in the error strip; **judgment: combo density/labels and dialog copy**.
- [ ] Spec sync: the four binding amendments (numeric reserved-range ids; no ssh-agent — non-goal wording; one-command vault save + NEW auto-link; `remote-sessions-changed` introduced) + anything execution changed; grep-verify every backtick identifier (the phantom-citation lesson). Commit: `git commit -m "Add SFTP connect checklist section and sync the spec"`

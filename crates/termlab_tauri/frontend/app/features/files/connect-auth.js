// The SFTP panel's auth dialog chain — vault unlock and host-password
// prompts, driven off the typed `SftpConnectError` variants
// (frontend/types/SftpConnectError.ts) that `sftp_connect_host` /
// `sftp_connect_host_with_password` (remote/detached_commands.rs) reject
// with.
//
// Loaded as a classic script BEFORE panels/files-panel.js (see index.html):
// files-panel.js captures `exports.termlabConnectAuth` into a top-level
// const at ITS OWN load time (same pattern as its other module deps), so if
// this script tagged after it, that capture would permanently bind to the
// `{}` fallback — the "module-defer trap" the task brief calls out.
//
// Dialog construction follows features/editor/file-dialog-view.js's
// confirmOverwrite idiom: a small `el()` helper builds elements directly via
// document.createElement + direct JS references, never innerHTML+
// querySelector. Each password prompt is a fresh tl-dialog per attempt
// (closed and reopened on a wrong password) rather than one dialog patched
// in place — simpler to reason about (no manual disable/re-enable dance
// while an invoke is in flight) and it sidesteps needing querySelector
// support in the no-jsdom test harness, matching this file family's existing
// convention (test_sftp_connect.mjs's header, test_editor_close_guards.mjs).
(function initTermLabConnectAuth(global) {
  'use strict';

  function el(tag, className, text) {
    const node = document.createElement(tag);
    if (className) node.className = className;
    if (text != null) node.textContent = String(text);
    return node;
  }

  function passwordInputEl(className) {
    const input = document.createElement('input');
    input.type = 'password';
    input.className = className;
    input.autocomplete = 'current-password';
    input.spellcheck = false;
    return input;
  }

  // Any Error/typed-SftpConnectError/plain string -> a display message.
  // `SftpConnectError`'s `authFailed`/`unreachable`/`other` variants and a
  // rejected `vault_unlock` (a bare error string, per vault_commands.rs's
  // `.map_err(|e| e.to_string())`) are the only shapes this ever sees.
  function messageOf(err) {
    if (!err) return 'Could not connect.';
    if (typeof err === 'string') return err;
    return err.message || err.kind || 'Could not connect.';
  }

  // ---------------------------------------------------------------------
  // Server entry lookup — for the host-password dialog's "user@host" title.
  // Mirrors files-panel.js's buildConfiguredHostOptions dedup rule (a
  // config-owned folders/ungrouped entry is preferred over an ssh_config
  // entry sharing the same id, the shape vault-linking produces), but only
  // needs to find ONE entry rather than build combo options.
  // ---------------------------------------------------------------------
  function findServerEntry(servers, entryId) {
    const data = servers && typeof servers === 'object'
      ? servers
      : { folders: [], ungrouped: [], ssh_config: [] };
    const folders = Array.isArray(data.folders) ? data.folders : [];
    for (let i = 0; i < folders.length; i += 1) {
      const entries = Array.isArray(folders[i].entries) ? folders[i].entries : [];
      const hit = entries.find((e) => e && e.id === entryId);
      if (hit) return hit;
    }
    const ungrouped = Array.isArray(data.ungrouped) ? data.ungrouped : [];
    const ungroupedHit = ungrouped.find((e) => e && e.id === entryId);
    if (ungroupedHit) return ungroupedHit;
    const sshConfig = Array.isArray(data.ssh_config) ? data.ssh_config : [];
    return sshConfig.find((e) => e && e.id === entryId) || null;
  }

  function hostTitle(entry) {
    if (!entry || !entry.host) return 'Connect';
    return entry.user ? `${entry.user}@${entry.host}` : String(entry.host);
  }

  async function lookupServerEntry(data, invoke, serverEntryId) {
    if (!data || typeof data.getServers !== 'function') return null;
    try {
      const servers = await data.getServers(invoke);
      return findServerEntry(servers, serverEntryId);
    } catch (_) {
      return null;
    }
  }

  // ---------------------------------------------------------------------
  // Rung 1 — master password (vault unlock). Resolves the typed password,
  // or null if the user cancelled (Cancel button, Escape, or a backdrop
  // click — all three close the dialog, and onClose is where the DOM field
  // is cleared and the promise settles, in that order, so the value is
  // wiped before any caller can observe it via the resolved promise).
  // ---------------------------------------------------------------------
  function promptMasterPassword(errorMessage) {
    return new Promise((resolve) => {
      if (!global.tlDialog || typeof global.tlDialog.open !== 'function') {
        resolve(null);
        return;
      }
      let settled = false;
      let handle = null;
      let passwordInput = null;
      let pendingValue = null;

      function settle(value) {
        if (settled) return;
        settled = true;
        resolve(value);
      }

      handle = global.tlDialog.open({
        title: 'Unlock Vault',
        ariaLabel: 'Unlock vault',
        size: 'sm',
        body: (bodyEl) => {
          const field = el('div', 'tl-field');
          field.appendChild(el('span', 'tl-field__label', 'Master password'));
          passwordInput = passwordInputEl('tl-input ca-master-password');
          field.appendChild(passwordInput);
          bodyEl.appendChild(field);
          if (errorMessage) {
            bodyEl.appendChild(el('div', 'ssh-export-dim ca-error-line', errorMessage));
          }
          setTimeout(() => { if (passwordInput) passwordInput.focus(); }, 50);
        },
        buttons: [
          { label: 'Cancel', onSelect: () => { if (handle) handle.close(); } },
          {
            label: 'Unlock',
            primary: true,
            onSelect: () => {
              pendingValue = passwordInput ? passwordInput.value : '';
              if (handle) handle.close();
            },
          },
        ],
        // Fires for EVERY close path (Cancel, Unlock, Escape, backdrop) —
        // the DOM field is always cleared here, strictly before settle()
        // resolves the promise, regardless of which path got us here.
        onClose: () => {
          if (passwordInput) passwordInput.value = '';
          settle(pendingValue);
        },
      });
    });
  }

  async function unlockVaultViaDialog(invoke) {
    let errorMessage = '';
    // Cancel breaks the loop by resolving with null from the prompt itself;
    // a wrong password re-prompts (a fresh dialog, with the error line set)
    // rather than retrying invoke() silently.
    for (;;) {
      const password = await promptMasterPassword(errorMessage);
      if (password == null) return false;
      try {
        await invoke('vault_unlock', { request: { password } });
        return true;
      } catch (err) {
        errorMessage = messageOf(err) || 'Incorrect password.';
      }
    }
  }

  // ---------------------------------------------------------------------
  // Rung 2 — host password. Resolves { password, saveToVault }, or null on
  // cancel. `failedAttempts` (completed failures so far) drives both the
  // error line and the attempt counter, which only appears once two
  // attempts have already failed (i.e. this is the prompt for the 3rd try).
  // ---------------------------------------------------------------------
  function promptHostPassword(opts) {
    const { title, hasVaultAccount, failedAttempts, errorMessage } = opts;
    return new Promise((resolve) => {
      if (!global.tlDialog || typeof global.tlDialog.open !== 'function') {
        resolve(null);
        return;
      }
      let settled = false;
      let handle = null;
      let passwordInput = null;
      let saveCheckbox = null;
      let pendingValue = null;

      function settle(value) {
        if (settled) return;
        settled = true;
        resolve(value);
      }

      handle = global.tlDialog.open({
        title,
        ariaLabel: title,
        size: 'sm',
        body: (bodyEl) => {
          const field = el('div', 'tl-field');
          field.appendChild(el('span', 'tl-field__label', 'Password'));
          passwordInput = passwordInputEl('tl-input ca-host-password');
          field.appendChild(passwordInput);
          bodyEl.appendChild(field);

          const checkLabel = el('label', 'tl-check ca-save-field');
          saveCheckbox = document.createElement('input');
          saveCheckbox.type = 'checkbox';
          saveCheckbox.className = 'ca-save-checkbox';
          saveCheckbox.checked = !!hasVaultAccount;
          checkLabel.appendChild(saveCheckbox);
          checkLabel.appendChild(el('span', '', 'Save to vault'));
          bodyEl.appendChild(checkLabel);

          if (errorMessage) {
            bodyEl.appendChild(el('div', 'ssh-export-dim ca-error-line', errorMessage));
          }
          if (failedAttempts >= 2) {
            bodyEl.appendChild(el('div', 'ca-attempt-line', `Attempt ${failedAttempts + 1}`));
          }
          setTimeout(() => { if (passwordInput) passwordInput.focus(); }, 50);
        },
        buttons: [
          { label: 'Cancel', onSelect: () => { if (handle) handle.close(); } },
          {
            label: 'Connect',
            primary: true,
            onSelect: () => {
              pendingValue = {
                password: passwordInput ? passwordInput.value : '',
                saveToVault: saveCheckbox ? !!saveCheckbox.checked : false,
              };
              if (handle) handle.close();
            },
          },
        ],
        onClose: () => {
          if (passwordInput) passwordInput.value = '';
          settle(pendingValue);
        },
      });
    });
  }

  // ---------------------------------------------------------------------
  // Connect-command wrappers — go through `data` (the caller's
  // features/files/data-service.js) when it offers the function, else call
  // `invoke` directly. Keeps this module usable from a bare {invoke} ctx
  // (e.g. a future non-files-panel caller) without requiring the full data
  // service.
  // ---------------------------------------------------------------------
  function connectHost(data, invoke, serverEntryId) {
    return data && typeof data.connectHost === 'function'
      ? data.connectHost(invoke, serverEntryId)
      : invoke('sftp_connect_host', { serverEntryId });
  }

  function connectHostWithPassword(data, invoke, serverEntryId, password, saveToVault) {
    return data && typeof data.connectHostWithPassword === 'function'
      ? data.connectHostWithPassword(invoke, serverEntryId, password, saveToVault)
      : invoke('sftp_connect_host_with_password', { serverEntryId, password, saveToVault });
  }

  async function runPasswordChain(serverEntryId, hasVaultAccount, data, invoke, onError) {
    const entry = await lookupServerEntry(data, invoke, serverEntryId);
    const title = hostTitle(entry);
    let failedAttempts = 0;
    let errorMessage = '';

    for (;;) {
      const answer = await promptHostPassword({ title, hasVaultAccount, failedAttempts, errorMessage });
      if (!answer) return null; // Cancel — no further invokes.
      try {
        return await connectHostWithPassword(data, invoke, serverEntryId, answer.password, answer.saveToVault);
      } catch (err) {
        if (err && err.kind === 'authFailed') {
          failedAttempts += 1;
          errorMessage = `Authentication failed: ${err.message}`;
          continue; // re-prompt, same host, same hasVaultAccount default
        }
        // unreachable/other/connectInProgress/unknown — the ruling covers
        // this "at any rung": route the message to the caller's error
        // surface and stop, no further invokes.
        if (typeof onError === 'function') onError(messageOf(err));
        return null;
      }
    }
  }

  // ---------------------------------------------------------------------
  // Entry point. `serverEntryId` + the error that a connect attempt (Task
  // 3's files-panel.js connectToHost, or a future caller) already got back
  // from `sftp_connect_host`/`sftp_connect_host_with_password` determine
  // which rung the chain starts at:
  //
  //   - `vaultLocked`   -> master-password dialog, then ONE retry of
  //                        sftp_connect_host (which may itself come back
  //                        `needsPassword`, continuing the chain below).
  //   - `needsPassword` -> host-password dialog straight away.
  //   - `authFailed`/`unreachable`/`other` as the STARTING error (not
  //     expected from a first `sftp_connect_host` call today, but the type
  //     allows it) -> routed to onError, resolve null; no dialog.
  //   - `connectInProgress` reaching here at all is defensive: files-panel's
  //     connectToHost is the intended/only production caller and it handles
  //     connectInProgress itself, inline, BEFORE ever calling run() — that
  //     is deliberate, not an oversight. connectInProgress means a connect
  //     for this same host is already in flight (Rust's duplicate-connect
  //     guard); it is not an error and must not clear the caller's busy
  //     state, but this module has no access to that state (it is
  //     files-panel's private closure variable) to leave it alone the way
  //     the binding ruling requires. Handling it in files-panel — which
  //     already owns that state and, pre-Task-4, already had exactly this
  //     inline check — is the only place that can honor "leave the busy
  //     state as it was" without threading it through here. If this ever
  //     reaches run() (e.g. a future direct caller), it degrades safely: a
  //     quiet resolve(null), no dialog, no onError call, matching "not an
  //     error, not a dialog".
  //
  // Never rejects. Resolves with the `ConnectedSession` the chain
  // eventually won, or null (cancelled, or an error already routed to
  // onError).
  // ---------------------------------------------------------------------
  async function run(serverEntryId, startingError, ctx) {
    const opts = ctx || {};
    const invoke = opts.invoke;
    const data = opts.data;
    const onError = typeof opts.onError === 'function' ? opts.onError : () => {};

    let err = startingError;

    if (err && err.kind === 'connectInProgress') {
      return null;
    }

    if (err && err.kind === 'vaultLocked') {
      const unlocked = await unlockVaultViaDialog(invoke);
      if (!unlocked) return null; // Cancel — no further invokes.
      try {
        return await connectHost(data, invoke, serverEntryId);
      } catch (retryErr) {
        err = retryErr;
        if (err && err.kind === 'connectInProgress') return null;
      }
    }

    if (err && err.kind === 'needsPassword') {
      return await runPasswordChain(serverEntryId, !!err.hasVaultAccount, data, invoke, onError);
    }

    onError(err && err.kind === 'authFailed' ? `Authentication failed: ${err.message}` : messageOf(err));
    return null;
  }

  global.termlabConnectAuth = { run };
})(window);

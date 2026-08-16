(function initTermLabSshConnectionForm(global) {
  'use strict';

  async function populateAccountPicker(panelEl, selectedId, deps) {
    const d = deps || {};
    const attr = typeof d.attr === 'function'
      ? d.attr
      : (value) => String(value == null ? '' : value);
    const esc = typeof d.esc === 'function'
      ? d.esc
      : attr;

    const select = panelEl.querySelector('#cf-vault-account');
    if (!select) return;

    let accounts = [];
    if (global.vault && typeof global.vault.getAccounts === 'function') {
      try {
        accounts = await global.vault.getAccounts();
      } catch (_) {
        // Vault may not exist or be locked.
      }
    }

    let html = '<option value="">Manual credentials</option>';
    for (const account of accounts) {
      const authLabel = account.auth_type === 'password'
        ? 'password'
        : account.auth_type === 'key'
          ? 'key'
          : 'key+pw';
      html += `<option value="${attr(account.id)}">${esc(account.display_name)} (${esc(account.username)}, ${authLabel})</option>`;
    }
    html += '<option value="__create__">+ Create New Account...</option>';
    select.innerHTML = html;

    if (selectedId) select.value = selectedId;
    if (select._tlCombo && typeof select._tlCombo.refresh === 'function') select._tlCombo.refresh();
    updateCredentialFieldsVisibility(panelEl);
  }

  function updateCredentialFieldsVisibility(panelEl) {
    const select = panelEl.querySelector('#cf-vault-account');
    const manualCreds = panelEl.querySelector('#cf-manual-creds');
    const accountInfo = panelEl.querySelector('#cf-vault-account-info');
    if (!select || !manualCreds || !accountInfo) return;

    const value = select.value;
    if (value && value !== '__create__') {
      manualCreds.style.display = 'none';
      const selectedOption = select.options[select.selectedIndex];
      accountInfo.style.display = 'block';
      accountInfo.textContent = 'Using vault account: ' + selectedOption.textContent;
      return;
    }

    manualCreds.style.display = '';
    accountInfo.style.display = 'none';
  }

  function handleCreateNewAccount(panelEl, fallbackId, deps) {
    const d = deps || {};

    if (!global.vault) {
      if (d.toast && typeof d.toast.error === 'function') {
        d.toast.error('Vault Unavailable', 'Vault module not loaded');
      }
      const select = panelEl.querySelector('#cf-vault-account');
      if (select) {
        select.value = fallbackId || '';
        // Plain .value writes don't trigger tl-combo's change listener or
        // its attribute/childList MutationObserver, so the visible combo
        // button's label would otherwise go stale here.
        if (select._tlCombo && typeof select._tlCombo.refresh === 'function') select._tlCombo.refresh();
      }
      return;
    }

    global.vault.ensureUnlocked(() => {
      global.vault.showAccountForm(null);
      const checkInterval = setInterval(() => {
        const vaultOverlay = document.getElementById('vault-overlay');
        if (vaultOverlay) return;
        clearInterval(checkInterval);
        populateAccountPicker(panelEl, '', d);
      }, 300);
    });
  }

  function submitForm(panelEl, existing, andConnect, dismissForm, deps) {
    const d = deps || {};
    if (typeof d.invoke !== 'function') return;

    const hostInput = panelEl.querySelector('#cf-host');
    const host = hostInput ? hostInput.value.trim() : '';
    if (!host) {
      if (hostInput) hostInput.focus();
      return;
    }

    const labelInput = panelEl.querySelector('#cf-label');
    const portInput = panelEl.querySelector('#cf-port');
    const proxyTypeSelect = panelEl.querySelector('#cf-proxy-type');
    const proxyValueInput = panelEl.querySelector('#cf-proxy-value');
    const folderSelect = panelEl.querySelector('#cf-folder');

    const label = labelInput ? labelInput.value.trim() : '';
    const port = parseInt(portInput ? portInput.value : '', 10) || 22;
    const proxyType = proxyTypeSelect ? proxyTypeSelect.value : 'none';
    const proxyValue = proxyValueInput ? proxyValueInput.value.trim() : '';
    const folderId = (folderSelect && folderSelect.value) ? folderSelect.value : null;

    const proxyJump = proxyType === 'jump' && proxyValue ? proxyValue : null;
    const proxyCommand = proxyType === 'command' && proxyValue ? proxyValue : null;

    const accountSelect = panelEl.querySelector('#cf-vault-account');
    const vaultAccountId = accountSelect && accountSelect.value && accountSelect.value !== '__create__'
      ? accountSelect.value
      : null;

    const userInput = panelEl.querySelector('#cf-user');
    const passwordInput = panelEl.querySelector('#cf-password');
    const keyPathInput = panelEl.querySelector('#cf-key-path');

    const user = vaultAccountId
      ? (existing ? existing.user : null)
      : ((userInput ? userInput.value : '').trim() || 'root');
    const password = vaultAccountId ? '' : (passwordInput ? passwordInput.value : '');
    const keyPath = vaultAccountId
      ? null
      : (((keyPathInput ? keyPathInput.value : '').trim()) || null);
    const authMethod = vaultAccountId ? null : (password ? 'password' : 'key');

    const entry = {
      id: existing ? existing.id : crypto.randomUUID(),
      label: label || `${user || 'root'}@${host}`,
      host,
      port,
      user: user || null,
      auth_method: authMethod,
      key_path: keyPath,
      vault_account_id: vaultAccountId,
      proxy_command: proxyCommand,
      proxy_jump: proxyJump,
    };

    dismissForm();

    d.invoke('remote_save_server', { entry, folderId })
      .then(() => {
        if (typeof d.refreshAll === 'function') d.refreshAll();
        if (andConnect && typeof d.createSshTab === 'function') {
          d.createSshTab({ serverId: entry.id, password: password || undefined });
        }
      })
      .catch((error) => {
        if (d.toast && typeof d.toast.error === 'function') {
          d.toast.error('Save Failed', String(error));
        }
      });
  }

  function showConnectionForm(existing, defaultFolderId, deps) {
    const d = deps || {};
    if (!global.tlDialog || typeof global.tlDialog.open !== 'function') return null;
    if (!d.serverData || !Array.isArray(d.serverData.folders)) return null;
    if (typeof d.buildProxyJumpOptions !== 'function') return null;
    if (typeof d.renderProxyJumpOptions !== 'function') return null;
    if (typeof d.normalizeProxyJump !== 'function') return null;

    const attr = typeof d.attr === 'function'
      ? d.attr
      : (value) => String(value == null ? '' : value);
    const esc = typeof d.esc === 'function'
      ? d.esc
      : attr;

    const isEdit = !!existing;
    const title = isEdit ? 'Edit SSH Connection' : 'New SSH Connection';

    const folderOptions = [{ id: '', name: '(none)' }];
    for (const folder of d.serverData.folders) {
      folderOptions.push({ id: folder.id, name: folder.name });
    }

    let selectedFolder = defaultFolderId || '';
    if (isEdit && !selectedFolder) {
      for (const folder of d.serverData.folders) {
        if (!Array.isArray(folder.entries)) continue;
        if (folder.entries.some((entry) => entry.id === existing.id)) {
          selectedFolder = folder.id;
          break;
        }
      }
    }

    const proxyJumpOptions = d.buildProxyJumpOptions(existing ? existing.id : null);

    let proxyType = 'none';
    let proxyValue = '';
    if (existing) {
      if (existing.proxy_jump) {
        proxyType = 'jump';
        proxyValue = existing.proxy_jump;
      } else if (existing.proxy_command) {
        proxyType = 'command';
        proxyValue = existing.proxy_command;
      }
    }

    const normalizedExistingProxyJump = proxyType === 'jump' ? d.normalizeProxyJump(proxyValue) : null;
    const selectedProxyJumpOption = normalizedExistingProxyJump
      ? proxyJumpOptions.find((opt) => d.normalizeProxyJump(opt.spec) === normalizedExistingProxyJump)
      : null;

    const existingVaultId = existing ? (existing.vault_account_id || '') : '';

    let handle = null;
    let dismissed = false;
    const dismissForm = () => {
      if (dismissed) return;
      dismissed = true;
      if (handle) handle.close();
    };

    handle = global.tlDialog.open({
      title,
      ariaLabel: title,
      size: 'md',
      body: (bodyEl) => {
        bodyEl.innerHTML = `
          <div class="tl-field">
            <span class="tl-field__label">Session Name</span>
            <input type="text" class="tl-input" id="cf-label" value="${attr(existing ? existing.label : '')}"
                   placeholder="optional" spellcheck="false" />
          </div>
          <div class="tl-field">
            <span class="tl-field__label">Host / IP</span>
            <input type="text" class="tl-input" id="cf-host" value="${attr(existing ? existing.host : '')}"
                   placeholder="example.com" spellcheck="false" required />
          </div>
          <div class="tl-field">
            <span class="tl-field__label">Port</span>
            <input type="number" class="tl-input" id="cf-port" value="${existing ? existing.port : 22}" min="1" max="65535" />
          </div>
          <div class="tl-field">
            <span class="tl-field__label">Account</span>
            <select class="tl-combo-select" id="cf-vault-account">
              <option value="">Manual credentials</option>
              <option value="__create__">+ Create New Account...</option>
            </select>
          </div>
          <div id="cf-vault-account-info" style="display:none;padding:6px 8px;border-radius:4px;background:var(--tl-bg);border:1px solid var(--tl-border);margin-bottom:8px;font-size:12px"></div>
          <div id="cf-manual-creds">
            <div class="tl-field">
              <span class="tl-field__label">Username</span>
              <input type="text" class="tl-input" id="cf-user" value="${attr(existing ? existing.user : '')}"
                     placeholder="root" spellcheck="false" />
            </div>
            <div class="tl-field">
              <span class="tl-field__label">Password</span>
              <input type="password" class="tl-input" id="cf-password" value="" placeholder="leave empty for key auth" />
            </div>
            <div class="tl-field">
              <span class="tl-field__label">Private Key</span>
              <input type="text" class="tl-input" id="cf-key-path" value="${attr(existing && existing.key_path ? existing.key_path : '')}"
                     placeholder="~/.ssh/id_ed25519" spellcheck="false" />
            </div>
          </div>
          <details class="tl-details" ${proxyType !== 'none' ? 'open' : ''}>
            <summary>Advanced</summary>
            <div class="tl-field">
              <span class="tl-field__label">Proxy Type</span>
              <select class="tl-combo-select" id="cf-proxy-type">
                <option value="none" ${proxyType === 'none' ? 'selected' : ''}>None</option>
                <option value="jump" ${proxyType === 'jump' ? 'selected' : ''}>ProxyJump</option>
                <option value="command" ${proxyType === 'command' ? 'selected' : ''}>ProxyCommand</option>
              </select>
            </div>
            <div class="tl-field" id="cf-proxy-jump-row" style="display:${proxyType === 'jump' ? '' : 'none'}">
              <span class="tl-field__label">Proxy Jump Session</span>
              <select class="tl-combo-select" id="cf-proxy-jump-select">
                <option value="__custom__" ${selectedProxyJumpOption ? '' : 'selected'}>Custom value...</option>
                ${d.renderProxyJumpOptions(proxyJumpOptions)}
              </select>
            </div>
            <div class="tl-field" id="cf-proxy-value-row" style="display:${proxyType === 'none' ? 'none' : ''}">
              <span class="tl-field__label">Proxy Value</span>
              <input type="text" class="tl-input" id="cf-proxy-value" value="${attr(proxyValue)}"
                     placeholder="user@jumphost or ssh -W %h:%p host" spellcheck="false" />
            </div>
          </details>
          <div class="tl-field">
            <span class="tl-field__label">Save to Folder</span>
            <select class="tl-combo-select" id="cf-folder">
              ${folderOptions.map((folder) =>
                `<option value="${attr(folder.id)}" ${folder.id === selectedFolder ? 'selected' : ''}>${esc(folder.name)}</option>`
              ).join('')}
            </select>
          </div>
        `;

        bodyEl.querySelectorAll('select.tl-combo-select').forEach((select) => {
          if (global.tlCombo && typeof global.tlCombo.attach === 'function') global.tlCombo.attach(select);
        });

        // Port keeps its own min/max/step attributes and stays the source
        // of truth (tlSpinner.attach only adds a stepper column next to it,
        // same contract as tlCombo.attach above).
        const portInput = bodyEl.querySelector('#cf-port');
        if (portInput && global.tlSpinner && typeof global.tlSpinner.attach === 'function') {
          global.tlSpinner.attach(portInput);
        }

        // populateAccountPicker/updateCredentialFieldsVisibility/
        // handleCreateNewAccount only ever query IDs that live inside the
        // body (never the footer buttons), so bodyEl itself is a valid
        // stand-in for the old overlay-scoped querySelector root.
        populateAccountPicker(bodyEl, existingVaultId, d);

        const accountSelect = bodyEl.querySelector('#cf-vault-account');
        if (accountSelect) {
          accountSelect.addEventListener('change', () => {
            const value = accountSelect.value;
            if (value === '__create__') {
              handleCreateNewAccount(bodyEl, existingVaultId, d);
              return;
            }
            updateCredentialFieldsVisibility(bodyEl);
          });
        }

        const proxyTypeSelect = bodyEl.querySelector('#cf-proxy-type');
        const proxyValueInput = bodyEl.querySelector('#cf-proxy-value');
        const proxyValueRow = bodyEl.querySelector('#cf-proxy-value-row');
        const proxyJumpRow = bodyEl.querySelector('#cf-proxy-jump-row');
        const proxyJumpSelect = bodyEl.querySelector('#cf-proxy-jump-select');

        function syncProxyJumpSelectFromValue() {
          if (!proxyJumpSelect || !proxyTypeSelect || !proxyValueInput) return;
          if (proxyTypeSelect.value !== 'jump') return;

          const normalized = d.normalizeProxyJump(proxyValueInput.value);
          if (!normalized) {
            proxyJumpSelect.value = '__custom__';
            if (proxyJumpSelect._tlCombo) proxyJumpSelect._tlCombo.refresh();
            return;
          }
          const match = proxyJumpOptions.find((opt) => d.normalizeProxyJump(opt.spec) === normalized);
          proxyJumpSelect.value = match ? match.spec : '__custom__';
          if (proxyJumpSelect._tlCombo) proxyJumpSelect._tlCombo.refresh();
        }

        function syncProxyUi() {
          if (!proxyTypeSelect || !proxyValueInput || !proxyValueRow || !proxyJumpRow) return;
          const currentProxyType = proxyTypeSelect.value;
          proxyJumpRow.style.display = currentProxyType === 'jump' ? '' : 'none';
          proxyValueRow.style.display = currentProxyType === 'none' ? 'none' : '';

          if (currentProxyType === 'jump') {
            proxyValueInput.placeholder = 'user@jump-host or jump-host:2222';
            syncProxyJumpSelectFromValue();
          } else if (currentProxyType === 'command') {
            proxyValueInput.placeholder = 'ssh -W %h:%p jump-host';
          }
        }

        if (proxyJumpSelect && proxyValueInput) {
          proxyJumpSelect.addEventListener('change', () => {
            if (proxyJumpSelect.value === '__custom__') {
              proxyValueInput.focus();
              return;
            }
            proxyValueInput.value = proxyJumpSelect.value;
          });
        }

        if (proxyTypeSelect) proxyTypeSelect.addEventListener('change', syncProxyUi);
        if (proxyValueInput && proxyTypeSelect) {
          proxyValueInput.addEventListener('input', () => {
            if (proxyTypeSelect.value === 'jump') syncProxyJumpSelectFromValue();
          });
        }

        if (selectedProxyJumpOption && proxyJumpSelect) {
          proxyJumpSelect.value = selectedProxyJumpOption.spec;
          if (proxyJumpSelect._tlCombo) proxyJumpSelect._tlCombo.refresh();
        }
        syncProxyUi();

        // Host, not the shell's default first-focusable (Session Name),
        // gets initial focus — same as the original overlay. The shell
        // focuses its default candidate via requestAnimationFrame right
        // after open(), so this has to run after that, same as the
        // original's setTimeout(..., 50).
        const hostInput = bodyEl.querySelector('#cf-host');
        setTimeout(() => {
          if (hostInput) hostInput.focus();
        }, 50);
      },
      buttons: [
        { label: 'Cancel', onSelect: dismissForm },
        { label: 'Save', onSelect: () => submitForm(handle.el, existing, false, dismissForm, d) },
        { label: 'Save & Connect', primary: true, onSelect: () => submitForm(handle.el, existing, true, dismissForm, d) },
      ],
      onClose: dismissForm,
    });

    return handle;
  }

  global.termlabSshConnectionForm = {
    showConnectionForm,
  };
})(window);

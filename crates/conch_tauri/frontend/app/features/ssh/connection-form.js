(function initConchSshConnectionForm(global) {
  'use strict';

  async function populateAccountPicker(overlay, selectedId, deps) {
    const d = deps || {};
    const attr = typeof d.attr === 'function'
      ? d.attr
      : (value) => String(value == null ? '' : value);
    const esc = typeof d.esc === 'function'
      ? d.esc
      : attr;

    const select = overlay.querySelector('#cf-vault-account');
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
    updateCredentialFieldsVisibility(overlay);
  }

  function updateCredentialFieldsVisibility(overlay) {
    const select = overlay.querySelector('#cf-vault-account');
    const manualCreds = overlay.querySelector('#cf-manual-creds');
    const accountInfo = overlay.querySelector('#cf-vault-account-info');
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

  function handleCreateNewAccount(overlay, fallbackId, deps) {
    const d = deps || {};

    if (!global.vault) {
      if (d.toast && typeof d.toast.error === 'function') {
        d.toast.error('Vault Unavailable', 'Vault module not loaded');
      }
      const select = overlay.querySelector('#cf-vault-account');
      if (select) select.value = fallbackId || '';
      return;
    }

    global.vault.ensureUnlocked(() => {
      global.vault.showAccountForm(null);
      const checkInterval = setInterval(() => {
        const vaultOverlay = document.getElementById('vault-overlay');
        if (vaultOverlay) return;
        clearInterval(checkInterval);
        populateAccountPicker(overlay, '', d);
      }, 300);
    });
  }

  function submitForm(overlay, existing, andConnect, dismissOverlay, deps) {
    const d = deps || {};
    if (typeof d.invoke !== 'function') return;

    const hostInput = overlay.querySelector('#cf-host');
    const host = hostInput ? hostInput.value.trim() : '';
    if (!host) {
      if (hostInput) hostInput.focus();
      return;
    }

    const labelInput = overlay.querySelector('#cf-label');
    const portInput = overlay.querySelector('#cf-port');
    const proxyTypeSelect = overlay.querySelector('#cf-proxy-type');
    const proxyValueInput = overlay.querySelector('#cf-proxy-value');
    const folderSelect = overlay.querySelector('#cf-folder');

    const label = labelInput ? labelInput.value.trim() : '';
    const port = parseInt(portInput ? portInput.value : '', 10) || 22;
    const proxyType = proxyTypeSelect ? proxyTypeSelect.value : 'none';
    const proxyValue = proxyValueInput ? proxyValueInput.value.trim() : '';
    const folderId = (folderSelect && folderSelect.value) ? folderSelect.value : null;

    const proxyJump = proxyType === 'jump' && proxyValue ? proxyValue : null;
    const proxyCommand = proxyType === 'command' && proxyValue ? proxyValue : null;

    const accountSelect = overlay.querySelector('#cf-vault-account');
    const vaultAccountId = accountSelect && accountSelect.value && accountSelect.value !== '__create__'
      ? accountSelect.value
      : null;

    const userInput = overlay.querySelector('#cf-user');
    const passwordInput = overlay.querySelector('#cf-password');
    const keyPathInput = overlay.querySelector('#cf-key-path');

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

    if (typeof dismissOverlay === 'function') dismissOverlay();
    else if (typeof d.removeOverlay === 'function') d.removeOverlay();

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
    if (typeof d.removeOverlay !== 'function') return false;
    if (typeof d.setOverlayDialogAttributes !== 'function') return false;
    if (typeof d.registerOverlayKeys !== 'function') return false;
    if (!d.serverData || !Array.isArray(d.serverData.folders)) return false;
    if (typeof d.buildProxyJumpOptions !== 'function') return false;
    if (typeof d.renderProxyJumpOptions !== 'function') return false;
    if (typeof d.normalizeProxyJump !== 'function') return false;

    const attr = typeof d.attr === 'function'
      ? d.attr
      : (value) => String(value == null ? '' : value);
    const esc = typeof d.esc === 'function'
      ? d.esc
      : attr;

    d.removeOverlay();

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

    const overlay = document.createElement('div');
    overlay.className = 'ssh-overlay';
    d.setOverlayDialogAttributes(overlay, title);
    overlay.innerHTML = `
      <div class="ssh-form">
        <div class="ssh-form-title">${esc(title)}</div>
        <div class="ssh-form-body">
          <label class="ssh-form-label">Session Name
            <input type="text" id="cf-label" value="${attr(existing ? existing.label : '')}"
                   placeholder="optional" spellcheck="false" />
          </label>
          <div class="ssh-form-row">
            <label class="ssh-form-label" style="flex:1">Host / IP
              <input type="text" id="cf-host" value="${attr(existing ? existing.host : '')}"
                     placeholder="example.com" spellcheck="false" required />
            </label>
            <label class="ssh-form-label" style="width:80px">Port
              <input type="number" id="cf-port" value="${existing ? existing.port : 22}" min="1" max="65535" />
            </label>
          </div>
          <label class="ssh-form-label">Account
            <select id="cf-vault-account">
              <option value="">Manual credentials</option>
              <option value="__create__">+ Create New Account...</option>
            </select>
          </label>
          <div id="cf-vault-account-info" style="display:none;padding:6px 8px;border-radius:4px;background:var(--bg);border:1px solid var(--selection);margin-bottom:8px;font-size:12px"></div>
          <div id="cf-manual-creds">
            <label class="ssh-form-label">Username
              <input type="text" id="cf-user" value="${attr(existing ? existing.user : '')}"
                     placeholder="root" spellcheck="false" />
            </label>
            <label class="ssh-form-label">Password
              <input type="password" id="cf-password" value="" placeholder="leave empty for key auth" />
            </label>
            <label class="ssh-form-label">Private Key
              <input type="text" id="cf-key-path" value="${attr(existing && existing.key_path ? existing.key_path : '')}"
                     placeholder="~/.ssh/id_ed25519" spellcheck="false" />
            </label>
          </div>
          <details class="ssh-form-advanced" ${proxyType !== 'none' ? 'open' : ''}>
            <summary>Advanced</summary>
            <label class="ssh-form-label">Proxy Type
              <select id="cf-proxy-type">
                <option value="none" ${proxyType === 'none' ? 'selected' : ''}>None</option>
                <option value="jump" ${proxyType === 'jump' ? 'selected' : ''}>ProxyJump</option>
                <option value="command" ${proxyType === 'command' ? 'selected' : ''}>ProxyCommand</option>
              </select>
            </label>
            <label class="ssh-form-label" id="cf-proxy-jump-row" style="display:${proxyType === 'jump' ? '' : 'none'}">Proxy Jump Session
              <select id="cf-proxy-jump-select">
                <option value="__custom__" ${selectedProxyJumpOption ? '' : 'selected'}>Custom value...</option>
                ${d.renderProxyJumpOptions(proxyJumpOptions)}
              </select>
            </label>
            <label class="ssh-form-label" id="cf-proxy-value-row" style="display:${proxyType === 'none' ? 'none' : ''}">Proxy Value
              <input type="text" id="cf-proxy-value" value="${attr(proxyValue)}"
                     placeholder="user@jumphost or ssh -W %h:%p host" spellcheck="false" />
            </label>
          </details>
          <label class="ssh-form-label">Save to Folder
            <select id="cf-folder">
              ${folderOptions.map((folder) =>
                `<option value="${attr(folder.id)}" ${folder.id === selectedFolder ? 'selected' : ''}>${esc(folder.name)}</option>`
              ).join('')}
            </select>
          </label>
        </div>
        <div class="ssh-form-buttons">
          <button class="ssh-form-btn" id="cf-cancel">Cancel</button>
          <button class="ssh-form-btn" id="cf-save">Save</button>
          <button class="ssh-form-btn primary" id="cf-save-connect">Save & Connect</button>
        </div>
      </div>
    `;

    document.body.appendChild(overlay);
    populateAccountPicker(overlay, existingVaultId, d);

    const accountSelect = overlay.querySelector('#cf-vault-account');
    if (accountSelect) {
      accountSelect.addEventListener('change', () => {
        const value = accountSelect.value;
        if (value === '__create__') {
          handleCreateNewAccount(overlay, existingVaultId, d);
          return;
        }
        updateCredentialFieldsVisibility(overlay);
      });
    }

    const proxyTypeSelect = overlay.querySelector('#cf-proxy-type');
    const proxyValueInput = overlay.querySelector('#cf-proxy-value');
    const proxyValueRow = overlay.querySelector('#cf-proxy-value-row');
    const proxyJumpRow = overlay.querySelector('#cf-proxy-jump-row');
    const proxyJumpSelect = overlay.querySelector('#cf-proxy-jump-select');

    function syncProxyJumpSelectFromValue() {
      if (!proxyJumpSelect || !proxyTypeSelect || !proxyValueInput) return;
      if (proxyTypeSelect.value !== 'jump') return;

      const normalized = d.normalizeProxyJump(proxyValueInput.value);
      if (!normalized) {
        proxyJumpSelect.value = '__custom__';
        return;
      }
      const match = proxyJumpOptions.find((opt) => d.normalizeProxyJump(opt.spec) === normalized);
      proxyJumpSelect.value = match ? match.spec : '__custom__';
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
    }
    syncProxyUi();

    const hostInput = overlay.querySelector('#cf-host');
    setTimeout(() => {
      if (hostInput) hostInput.focus();
    }, 50);

    let dismissed = false;
    const dismissForm = () => {
      if (dismissed) return;
      dismissed = true;
      if (typeof unregisterKeys === 'function') unregisterKeys();
      d.removeOverlay();
    };

    const unregisterKeys = d.registerOverlayKeys(overlay, 'ssh-connection-form', (event) => {
      if (event.key !== 'Escape') return false;
      dismissForm();
      return true;
    });

    overlay.addEventListener('mousedown', (event) => {
      if (event.target === overlay) dismissForm();
    });

    const cancelBtn = overlay.querySelector('#cf-cancel');
    if (cancelBtn) cancelBtn.addEventListener('click', dismissForm);
    const saveBtn = overlay.querySelector('#cf-save');
    if (saveBtn) saveBtn.addEventListener('click', () => submitForm(overlay, existing, false, dismissForm, d));
    const saveConnectBtn = overlay.querySelector('#cf-save-connect');
    if (saveConnectBtn) saveConnectBtn.addEventListener('click', () => submitForm(overlay, existing, true, dismissForm, d));

    return true;
  }

  global.conchSshConnectionForm = {
    showConnectionForm,
  };
})(window);

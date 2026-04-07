(function initConchVaultAccountForm(global) {
  'use strict';

  async function showAccountForm(existing, deps) {
    const d = deps || {};
    if (typeof d.removeOverlay !== 'function') return false;
    if (typeof d.setOverlayDialogAttributes !== 'function') return false;
    if (typeof d.registerScopedOverlayKeys !== 'function') return false;
    if (typeof d.listKeys !== 'function') return false;
    if (typeof d.pickKeyFile !== 'function') return false;
    if (typeof d.updateAccount !== 'function') return false;
    if (typeof d.addAccount !== 'function') return false;

    const attr = typeof d.attr === 'function'
      ? d.attr
      : (value) => String(value == null ? '' : value);
    const esc = typeof d.esc === 'function'
      ? d.esc
      : attr;

    d.removeOverlay();

    const isEdit = existing != null && existing.id != null;
    const title = isEdit ? 'Edit Account' : 'New Account';

    const displayName = existing ? existing.display_name : '';
    const username = existing ? existing.username : '';
    const authType = existing ? existing.auth_type : 'password';
    const keyPath = existing ? (existing.key_path || '') : '';

    let savedKeys = [];
    try {
      savedKeys = await d.listKeys();
    } catch (_) {
      savedKeys = [];
    }

    const overlay = document.createElement('div');
    overlay.className = 'ssh-overlay';
    overlay.id = 'vault-overlay';
    overlay.style.zIndex = '3100';
    d.setOverlayDialogAttributes(overlay, title);

    let keySourceOptions = '';
    if (savedKeys.length > 0) {
      keySourceOptions += '<option value="">-- Select a key --</option>';
      for (const key of savedKeys) {
        const label = key.algorithm + (key.comment ? ' — ' + key.comment : '');
        const selected = keyPath && keyPath === key.private_path ? ' selected' : '';
        keySourceOptions += '<option value="' + attr(key.private_path) + '"' + selected + '>' + esc(label) + '</option>';
      }
      keySourceOptions += '<option value="__custom__">Enter path manually\u2026</option>';
    }

    const hasSavedKeys = savedKeys.length > 0;
    const existingMatchesSaved = hasSavedKeys && savedKeys.some((key) => key.private_path === keyPath);
    const showManualInput = !hasSavedKeys || (keyPath && !existingMatchesSaved);

    overlay.innerHTML = `
      <div class="ssh-form vault-account-form">
        <div class="ssh-form-title">${esc(title)}</div>
        <div class="ssh-form-body">
          <label class="ssh-form-label">Display Name
            <input type="text" id="vault-acct-name" value="${attr(displayName)}"
                   placeholder="e.g. Production Deploy Key" spellcheck="false" />
          </label>
          <label class="ssh-form-label">Username
            <input type="text" id="vault-acct-user" value="${attr(username)}"
                   placeholder="e.g. root, deploy, ubuntu" spellcheck="false" autocomplete="off" />
          </label>
          <label class="ssh-form-label">Authentication Method
            <select id="vault-acct-auth">
              <option value="password" ${authType === 'password' ? 'selected' : ''}>Password</option>
              <option value="key" ${authType === 'key' ? 'selected' : ''}>SSH Key</option>
              <option value="key_and_password" ${authType === 'key_and_password' ? 'selected' : ''}>SSH Key + Password</option>
            </select>
          </label>
          <div id="vault-acct-pw-fields" style="${authType === 'key' ? 'display:none' : ''}">
            <label class="ssh-form-label">Password
              <input type="password" id="vault-acct-pw" placeholder="${isEdit ? '(unchanged if empty)' : 'Enter password'}"
                     spellcheck="false" autocomplete="off" />
            </label>
          </div>
          <div id="vault-acct-key-fields" style="${authType === 'password' ? 'display:none' : ''}">
            ${hasSavedKeys ? `
              <label class="ssh-form-label">SSH Key
                <select id="vault-acct-key-select">${keySourceOptions}</select>
              </label>
            ` : ''}
            <div id="vault-acct-manual-key" style="${hasSavedKeys && !showManualInput ? 'display:none' : ''}">
              <label class="ssh-form-label">Key File Path
                <input type="text" id="vault-acct-keypath" value="${attr(showManualInput ? keyPath : '')}"
                       placeholder="~/.ssh/id_ed25519" spellcheck="false" />
              </label>
              <a href="#" class="vault-browse-link" id="vault-acct-browse">Browse\u2026</a>
            </div>
            ${!hasSavedKeys ? `
              <div class="vault-keygen-link">
                No saved keys. <a href="#" id="vault-acct-generate">Generate a new SSH key</a>
              </div>
            ` : ''}
            <label class="ssh-form-label">Key Passphrase (optional)
              <input type="password" id="vault-acct-passphrase"
                     placeholder="${isEdit ? '(unchanged if empty)' : 'Enter passphrase'}"
                     spellcheck="false" autocomplete="off" />
            </label>
          </div>
        </div>
        <div class="ssh-form-buttons">
          <button class="ssh-form-btn" id="vault-acct-cancel">Cancel</button>
          <button class="ssh-form-btn primary" id="vault-acct-save">${isEdit ? 'Save Changes' : 'Create Account'}</button>
        </div>
      </div>
    `;

    document.body.appendChild(overlay);
    setTimeout(() => {
      const nameInput = overlay.querySelector('#vault-acct-name');
      if (nameInput) nameInput.focus();
    }, 50);

    const authSelect = overlay.querySelector('#vault-acct-auth');
    if (authSelect) {
      authSelect.addEventListener('change', () => {
        const value = authSelect.value;
        const pwFields = overlay.querySelector('#vault-acct-pw-fields');
        const keyFields = overlay.querySelector('#vault-acct-key-fields');
        if (pwFields) pwFields.style.display = value === 'key' ? 'none' : '';
        if (keyFields) keyFields.style.display = value === 'password' ? 'none' : '';
      });
    }

    const keySelect = overlay.querySelector('#vault-acct-key-select');
    const manualKeyDiv = overlay.querySelector('#vault-acct-manual-key');
    const keyPathInput = overlay.querySelector('#vault-acct-keypath');
    if (keySelect && keyPathInput) {
      keySelect.addEventListener('change', () => {
        if (keySelect.value === '__custom__') {
          if (manualKeyDiv) manualKeyDiv.style.display = '';
          keyPathInput.value = '';
          keyPathInput.focus();
          return;
        }
        if (keySelect.value) {
          if (manualKeyDiv) manualKeyDiv.style.display = 'none';
          keyPathInput.value = keySelect.value;
          return;
        }
        if (manualKeyDiv) manualKeyDiv.style.display = 'none';
        keyPathInput.value = '';
      });

      if (keySelect.value && keySelect.value !== '__custom__') {
        keyPathInput.value = keySelect.value;
      }
    }

    const browseLink = overlay.querySelector('#vault-acct-browse');
    if (browseLink && keyPathInput) {
      browseLink.addEventListener('click', async (event) => {
        event.preventDefault();
        try {
          const selected = await d.pickKeyFile();
          if (selected) {
            keyPathInput.value = selected;
            if (manualKeyDiv) manualKeyDiv.style.display = '';
            if (keySelect) keySelect.value = '__custom__';
          }
        } catch (_) {
          keyPathInput.focus();
        }
      });
    }

    const genLink = overlay.querySelector('#vault-acct-generate');
    if (genLink) {
      genLink.addEventListener('click', (event) => {
        event.preventDefault();
        if (d.keygen && typeof d.keygen.showKeygenDialog === 'function') {
          d.keygen.showKeygenDialog({ linkToVault: true });
        }
      });
    }

    let closed = false;
    const closeDialog = () => {
      if (closed) return;
      closed = true;
      if (typeof unregisterKeys === 'function') unregisterKeys();
      if (overlay.isConnected) overlay.remove();
      if (typeof d.renderVaultDialog === 'function') d.renderVaultDialog();
    };

    const unregisterKeys = d.registerScopedOverlayKeys(overlay, 'vault-account-form', (event) => {
      if (event.key !== 'Escape') return false;
      closeDialog();
      return true;
    });

    overlay.addEventListener('mousedown', (event) => {
      if (event.target === overlay) closeDialog();
    });

    const cancelBtn = overlay.querySelector('#vault-acct-cancel');
    if (cancelBtn) cancelBtn.addEventListener('click', closeDialog);

    const saveBtn = overlay.querySelector('#vault-acct-save');
    if (saveBtn) {
      saveBtn.addEventListener('click', async () => {
        const nameInput = overlay.querySelector('#vault-acct-name');
        const userInput = overlay.querySelector('#vault-acct-user');
        const authInput = overlay.querySelector('#vault-acct-auth');
        const pwInput = overlay.querySelector('#vault-acct-pw');
        const keyPathField = overlay.querySelector('#vault-acct-keypath');
        const passphraseInput = overlay.querySelector('#vault-acct-passphrase');

        const name = nameInput ? nameInput.value.trim() : '';
        const user = userInput ? userInput.value.trim() : '';
        const auth = authInput ? authInput.value : 'password';
        const pw = pwInput ? pwInput.value : '';
        const kp = keyPathField ? keyPathField.value.trim() : '';
        const passphrase = passphraseInput ? passphraseInput.value : '';

        if (!name) {
          if (d.toast && typeof d.toast.warn === 'function') d.toast.warn('Vault', 'Display name is required.');
          if (nameInput) nameInput.focus();
          return;
        }
        if (!user) {
          if (d.toast && typeof d.toast.warn === 'function') d.toast.warn('Vault', 'Username is required.');
          if (userInput) userInput.focus();
          return;
        }
        if (auth === 'password' && !isEdit && !pw) {
          if (d.toast && typeof d.toast.warn === 'function') d.toast.warn('Vault', 'Password is required for password auth.');
          if (pwInput) pwInput.focus();
          return;
        }
        if ((auth === 'key' || auth === 'key_and_password') && !kp) {
          if (d.toast && typeof d.toast.warn === 'function') d.toast.warn('Vault', 'Key file path is required.');
          if (keyPathField) keyPathField.focus();
          return;
        }
        if (auth === 'key_and_password' && !isEdit && !pw) {
          if (d.toast && typeof d.toast.warn === 'function') d.toast.warn('Vault', 'Password is required for key+password auth.');
          if (pwInput) pwInput.focus();
          return;
        }

        try {
          if (isEdit) {
            await d.updateAccount({
              id: existing.id,
              display_name: name,
              username: user,
              auth_type: auth,
              password: pw || null,
              key_path: (auth === 'key' || auth === 'key_and_password') ? kp : null,
              passphrase: passphrase || null,
            });
            if (d.toast && typeof d.toast.success === 'function') d.toast.success('Updated', 'Account updated successfully.');
          } else {
            await d.addAccount({
              display_name: name,
              username: user,
              auth_type: auth,
              password: (auth === 'password' || auth === 'key_and_password') ? pw : null,
              key_path: (auth === 'key' || auth === 'key_and_password') ? kp : null,
              passphrase: passphrase || null,
            });
            if (d.toast && typeof d.toast.success === 'function') d.toast.success('Created', 'Account added to vault.');
          }

          closeDialog();
        } catch (error) {
          if (d.toast && typeof d.toast.error === 'function') d.toast.error('Save Failed', String(error));
        }
      });
    }

    return true;
  }

  global.conchVaultAccountForm = {
    showAccountForm,
  };
})(window);

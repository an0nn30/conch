(function initTermLabVaultAccountForm(global) {
  'use strict';

  async function showAccountForm(existing, deps) {
    const d = deps || {};
    if (typeof d.listKeys !== 'function') return false;
    if (typeof d.pickKeyFile !== 'function') return false;
    if (typeof d.updateAccount !== 'function') return false;
    if (typeof d.addAccount !== 'function') return false;
    if (!global.tlDialog || typeof global.tlDialog.open !== 'function') return false;

    const attr = typeof d.attr === 'function'
      ? d.attr
      : (value) => String(value == null ? '' : value);
    const esc = typeof d.esc === 'function'
      ? d.esc
      : attr;

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

    let keySourceOptions = '';
    if (savedKeys.length > 0) {
      keySourceOptions += '<option value="">-- Select a key --</option>';
      for (const key of savedKeys) {
        const label = key.algorithm + (key.comment ? ' — ' + key.comment : '');
        const selected = keyPath && keyPath === key.private_path ? ' selected' : '';
        keySourceOptions += '<option value="' + attr(key.private_path) + '"' + selected + '>' + esc(label) + '</option>';
      }
      keySourceOptions += '<option value="__custom__">Enter path manually…</option>';
    }

    const hasSavedKeys = savedKeys.length > 0;
    const existingMatchesSaved = hasSavedKeys && savedKeys.some((key) => key.private_path === keyPath);
    const showManualInput = !hasSavedKeys || (keyPath && !existingMatchesSaved);

    let handle = null;
    let closed = false;
    const closeDialog = () => {
      if (closed) return;
      closed = true;
      if (handle) handle.close();
      if (typeof d.renderVaultDialog === 'function') d.renderVaultDialog();
    };

    handle = global.tlDialog.open({
      title,
      ariaLabel: title,
      size: 'md',
      body: (bodyEl) => {
        bodyEl.innerHTML = `
          <div class="tl-field">
            <span class="tl-field__label">Display Name</span>
            <input type="text" class="tl-input" id="vault-acct-name" value="${attr(displayName)}"
                   placeholder="e.g. Production Deploy Key" spellcheck="false" />
          </div>
          <div class="tl-field">
            <span class="tl-field__label">Username</span>
            <input type="text" class="tl-input" id="vault-acct-user" value="${attr(username)}"
                   placeholder="e.g. root, deploy, ubuntu" spellcheck="false" autocomplete="off" />
          </div>
          <div class="tl-field">
            <span class="tl-field__label">Authentication Method</span>
            <select class="tl-combo-select" id="vault-acct-auth">
              <option value="password" ${authType === 'password' ? 'selected' : ''}>Password</option>
              <option value="key" ${authType === 'key' ? 'selected' : ''}>SSH Key</option>
              <option value="key_and_password" ${authType === 'key_and_password' ? 'selected' : ''}>SSH Key + Password</option>
            </select>
          </div>
          <div id="vault-acct-pw-fields" style="${authType === 'key' ? 'display:none' : ''}">
            <div class="tl-field">
              <span class="tl-field__label">Password</span>
              <input type="password" class="tl-input" id="vault-acct-pw" placeholder="${isEdit ? '(unchanged if empty)' : 'Enter password'}"
                     spellcheck="false" autocomplete="off" />
            </div>
          </div>
          <div id="vault-acct-key-fields" style="${authType === 'password' ? 'display:none' : ''}">
            ${hasSavedKeys ? `
              <div class="tl-field">
                <span class="tl-field__label">SSH Key</span>
                <select class="tl-combo-select" id="vault-acct-key-select">${keySourceOptions}</select>
              </div>
            ` : ''}
            <div id="vault-acct-manual-key" style="${hasSavedKeys && !showManualInput ? 'display:none' : ''}">
              <div class="tl-field">
                <span class="tl-field__label">Key File Path</span>
                <input type="text" class="tl-input" id="vault-acct-keypath" value="${attr(showManualInput ? keyPath : '')}"
                       placeholder="~/.ssh/id_ed25519" spellcheck="false" />
              </div>
              <a href="#" class="vault-browse-link" id="vault-acct-browse">Browse…</a>
            </div>
            ${!hasSavedKeys ? `
              <div class="vault-keygen-link">
                No saved keys. <a href="#" id="vault-acct-generate">Generate a new SSH key</a>
              </div>
            ` : ''}
            <div class="tl-field">
              <span class="tl-field__label">Key Passphrase (optional)</span>
              <input type="password" class="tl-input" id="vault-acct-passphrase"
                     placeholder="${isEdit ? '(unchanged if empty)' : 'Enter passphrase'}"
                     spellcheck="false" autocomplete="off" />
            </div>
          </div>
        `;

        bodyEl.querySelectorAll('select.tl-combo-select').forEach((select) => {
          if (global.tlCombo && typeof global.tlCombo.attach === 'function') global.tlCombo.attach(select);
        });

        const authSelect = bodyEl.querySelector('#vault-acct-auth');
        if (authSelect) {
          authSelect.addEventListener('change', () => {
            const value = authSelect.value;
            const pwFields = bodyEl.querySelector('#vault-acct-pw-fields');
            const keyFields = bodyEl.querySelector('#vault-acct-key-fields');
            if (pwFields) pwFields.style.display = value === 'key' ? 'none' : '';
            if (keyFields) keyFields.style.display = value === 'password' ? 'none' : '';
          });
        }

        const keySelect = bodyEl.querySelector('#vault-acct-key-select');
        const manualKeyDiv = bodyEl.querySelector('#vault-acct-manual-key');
        const keyPathInput = bodyEl.querySelector('#vault-acct-keypath');
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

        const browseLink = bodyEl.querySelector('#vault-acct-browse');
        if (browseLink && keyPathInput) {
          browseLink.addEventListener('click', async (event) => {
            event.preventDefault();
            try {
              const selected = await d.pickKeyFile();
              if (selected) {
                keyPathInput.value = selected;
                if (manualKeyDiv) manualKeyDiv.style.display = '';
                if (keySelect) {
                  keySelect.value = '__custom__';
                  // Plain .value writes don't trigger tl-combo's change
                  // listener (see connection-form.js's populateAccountPicker
                  // for the same fix), so the visible combo button's label
                  // would otherwise go stale here.
                  if (keySelect._tlCombo && typeof keySelect._tlCombo.refresh === 'function') keySelect._tlCombo.refresh();
                }
              }
            } catch (_) {
              keyPathInput.focus();
            }
          });
        }

        const genLink = bodyEl.querySelector('#vault-acct-generate');
        if (genLink) {
          genLink.addEventListener('click', (event) => {
            event.preventDefault();
            if (d.keygen && typeof d.keygen.showKeygenDialog === 'function') {
              d.keygen.showKeygenDialog({ linkToVault: true });
            }
          });
        }

        const nameInput = bodyEl.querySelector('#vault-acct-name');
        setTimeout(() => {
          if (nameInput) nameInput.focus();
        }, 50);
      },
      buttons: [
        { label: 'Cancel', onSelect: closeDialog },
        { label: isEdit ? 'Save Changes' : 'Create Account', primary: true, onSelect: doSave },
      ],
      onClose: closeDialog,
    });

    async function doSave() {
      const bodyEl = handle.el;
      const nameInput = bodyEl.querySelector('#vault-acct-name');
      const userInput = bodyEl.querySelector('#vault-acct-user');
      const authInput = bodyEl.querySelector('#vault-acct-auth');
      const pwInput = bodyEl.querySelector('#vault-acct-pw');
      const keyPathField = bodyEl.querySelector('#vault-acct-keypath');
      const passphraseInput = bodyEl.querySelector('#vault-acct-passphrase');

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
    }

    return handle;
  }

  global.termlabVaultAccountForm = {
    showAccountForm,
  };
})(window);

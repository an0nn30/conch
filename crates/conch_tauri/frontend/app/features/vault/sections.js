(function initConchVaultSections(global) {
  'use strict';

  const utils = global.utils || {};
  const esc = typeof utils.esc === 'function' ? utils.esc : (value) => String(value == null ? '' : value);
  const attr = typeof utils.attr === 'function' ? utils.attr : esc;

  function getInitials(name) {
    if (!name) return '?';
    const parts = String(name).trim().split(/\s+/);
    if (parts.length >= 2) return (parts[0][0] + parts[1][0]).toUpperCase();
    return parts[0].substring(0, 2).toUpperCase();
  }

  function formatAuthType(type) {
    switch (type) {
      case 'password': return 'Password';
      case 'key': return 'SSH Key';
      case 'key_and_password': return 'Key + Password';
      default: return type;
    }
  }

  function renderAccountsSection(container, accounts, deps) {
    const d = deps || {};
    const list = Array.isArray(accounts) ? accounts : [];

    let html = '<div class="vault-section-header">';
    html += '<h3>User Accounts</h3>';
    html += '<button class="ssh-form-btn primary vault-add-btn" id="vault-add-account">New Account</button>';
    html += '</div>';

    if (list.length === 0) {
      html += '<div class="vault-empty">No accounts yet. Create one to store SSH credentials.</div>';
    } else {
      html += '<div class="vault-account-list">';
      for (const acct of list) {
        const initials = getInitials(acct.display_name);
        const authLabel = formatAuthType(acct.auth_type);
        html += `
          <div class="vault-account-row" data-id="${attr(acct.id)}">
            <div class="vault-account-avatar">${esc(initials)}</div>
            <div class="vault-account-info">
              <div class="vault-account-name">${esc(acct.display_name)}</div>
              <div class="vault-account-detail">${esc(acct.username)} &middot; ${esc(authLabel)}</div>
            </div>
            <div class="vault-account-actions">
              <button class="vault-row-btn vault-edit-btn" data-id="${attr(acct.id)}" title="Edit">Edit</button>
              <button class="vault-row-btn vault-delete-btn danger" data-id="${attr(acct.id)}" title="Delete">Delete</button>
            </div>
          </div>
        `;
      }
      html += '</div>';
    }

    container.innerHTML = html;

    const addBtn = container.querySelector('#vault-add-account');
    if (addBtn) addBtn.addEventListener('click', () => d.showAccountForm && d.showAccountForm(null));

    container.querySelectorAll('.vault-edit-btn').forEach((btn) => {
      btn.addEventListener('click', async (event) => {
        event.stopPropagation();
        if (typeof d.getAccount !== 'function') return;
        try {
          const account = await d.getAccount(btn.dataset.id);
          if (typeof d.showAccountForm === 'function') d.showAccountForm(account);
        } catch (error) {
          if (d.toast && typeof d.toast.error === 'function') {
            d.toast.error('Vault Error', 'Failed to load account: ' + error);
          }
        }
      });
    });

    container.querySelectorAll('.vault-delete-btn').forEach((btn) => {
      btn.addEventListener('click', (event) => {
        event.stopPropagation();
        if (btn.dataset.confirm !== 'yes') {
          btn.dataset.confirm = 'yes';
          btn.textContent = 'Confirm?';
          btn.classList.add('confirm');
          setTimeout(() => {
            if (!btn.isConnected) return;
            btn.dataset.confirm = '';
            btn.textContent = 'Delete';
            btn.classList.remove('confirm');
          }, 3000);
          return;
        }
        if (typeof d.deleteAccount !== 'function') return;
        d.deleteAccount(btn.dataset.id)
          .then(() => {
            if (d.toast && typeof d.toast.success === 'function') {
              d.toast.success('Deleted', 'Account removed from vault.');
            }
            if (typeof d.renderVaultDialog === 'function') d.renderVaultDialog();
          })
          .catch((error) => {
            if (d.toast && typeof d.toast.error === 'function') {
              d.toast.error('Delete Failed', String(error));
            }
          });
      });
    });
  }

  async function renderKeysSection(container, deps) {
    const d = deps || {};
    let keys = [];
    try {
      keys = typeof d.listKeys === 'function' ? await d.listKeys() : [];
    } catch (error) {
      container.innerHTML = '<div class="vault-empty">Failed to load keys: ' + esc(String(error)) + '</div>';
      return;
    }

    let html = '<div class="vault-section-header">';
    html += '<h3>SSH Keys</h3>';
    html += '<button class="ssh-form-btn primary vault-add-btn" id="vault-gen-key">Generate Key</button>';
    html += '</div>';

    if (keys.length === 0) {
      html += '<div class="vault-empty">No generated keys yet. Use the Generate Key button to create a new key pair.</div>';
    } else {
      html += '<div class="vault-account-list">';
      for (const key of keys) {
        html += `
          <div class="vault-account-row" data-id="${attr(key.id)}">
            <div class="vault-account-avatar">&#128273;</div>
            <div class="vault-account-info">
              <div class="vault-account-name">${esc(key.algorithm)}</div>
              <div class="vault-account-detail">${esc(key.fingerprint)}</div>
              <div class="vault-account-detail">${esc(key.private_path)}</div>
            </div>
            <div class="vault-account-actions">
              <button class="vault-row-btn vault-delete-btn danger" data-id="${attr(key.id)}" title="Delete">Delete</button>
            </div>
          </div>
        `;
      }
      html += '</div>';
    }

    container.innerHTML = html;

    const genBtn = container.querySelector('#vault-gen-key');
    if (genBtn) {
      genBtn.addEventListener('click', () => {
        if (d.keygen && typeof d.keygen.showKeygenDialog === 'function') {
          d.keygen.showKeygenDialog({ linkToVault: true });
          return;
        }
        if (d.toast && typeof d.toast.info === 'function') {
          d.toast.info('Coming Soon', 'Key generation dialog is not yet available.');
        }
      });
    }

    container.querySelectorAll('.vault-delete-btn').forEach((btn) => {
      btn.addEventListener('click', (event) => {
        event.stopPropagation();
        if (btn.dataset.confirm !== 'yes') {
          btn.dataset.confirm = 'yes';
          btn.textContent = 'Confirm?';
          btn.classList.add('confirm');
          setTimeout(() => {
            if (!btn.isConnected) return;
            btn.dataset.confirm = '';
            btn.textContent = 'Delete';
            btn.classList.remove('confirm');
          }, 3000);
          return;
        }

        if (typeof d.deleteKey !== 'function') return;
        d.deleteKey(btn.dataset.id)
          .then(() => {
            if (d.toast && typeof d.toast.success === 'function') {
              d.toast.success('Deleted', 'Key entry removed from vault.');
            }
            if (typeof d.renderVaultDialog === 'function') d.renderVaultDialog();
          })
          .catch((error) => {
            if (d.toast && typeof d.toast.error === 'function') {
              d.toast.error('Delete Failed', String(error));
            }
          });
      });
    });
  }

  function renderSettingsSection(container, settings, deps) {
    const d = deps || {};
    if (!settings) {
      container.innerHTML = '<div class="vault-empty">Failed to load vault settings.</div>';
      return;
    }

    const autoSaveOptions = ['Always', 'Ask', 'Never'];
    container.innerHTML = `
      <div class="vault-section-header"><h3>Vault Settings</h3></div>
      <div class="vault-settings-form">
        <label class="ssh-form-label">Auto-Lock Timeout (minutes)
          <input type="number" id="vault-setting-timeout" value="${settings.auto_lock_minutes}"
                 min="1" max="1440" />
        </label>
        <label class="vault-checkbox-label">
          <input type="checkbox" id="vault-setting-agent" ${settings.push_to_system_agent ? 'checked' : ''} />
          Push keys to system SSH agent on unlock
        </label>
        <label class="ssh-form-label">Auto-Save Passwords
          <select id="vault-setting-autosave">
            ${autoSaveOptions.map((opt) =>
              '<option value="' + attr(opt) + '"' +
              (settings.auto_save_passwords === opt ? ' selected' : '') +
              '>' + esc(opt) + '</option>'
            ).join('')}
          </select>
        </label>
        <div class="vault-settings-actions">
          <button class="ssh-form-btn primary" id="vault-save-settings">Save Settings</button>
        </div>
      </div>
    `;

    const saveBtn = container.querySelector('#vault-save-settings');
    if (!saveBtn) return;
    saveBtn.addEventListener('click', async () => {
      const timeout = parseInt(container.querySelector('#vault-setting-timeout').value, 10);
      const agent = container.querySelector('#vault-setting-agent').checked;
      const autoSave = container.querySelector('#vault-setting-autosave').value;

      if (!timeout || timeout < 1 || timeout > 1440) {
        if (d.toast && typeof d.toast.warn === 'function') {
          d.toast.warn('Invalid', 'Timeout must be between 1 and 1440 minutes.');
        }
        return;
      }

      if (typeof d.updateVaultSettings !== 'function') return;
      try {
        await d.updateVaultSettings({
          auto_lock_minutes: timeout,
          push_to_system_agent: agent,
          auto_save_passwords: autoSave,
        });
        if (d.toast && typeof d.toast.success === 'function') {
          d.toast.success('Settings Saved', 'Vault settings updated.');
        }
      } catch (error) {
        if (d.toast && typeof d.toast.error === 'function') {
          d.toast.error('Settings Error', 'Failed to save settings: ' + error);
        }
      }
    });
  }

  global.conchVaultSections = {
    renderAccountsSection,
    renderKeysSection,
    renderSettingsSection,
  };
})(window);

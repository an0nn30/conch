(function initConchVaultDialogs(global) {
  'use strict';

  function showSetupDialog(onSuccess, deps) {
    const d = deps || {};
    if (typeof d.removeOverlay !== 'function') return false;
    if (typeof d.registerScopedOverlayKeys !== 'function') return false;
    if (typeof d.setOverlayDialogAttributes !== 'function') return false;
    if (typeof d.createVault !== 'function') return false;

    d.removeOverlay();

    const overlay = document.createElement('div');
    overlay.className = 'ssh-overlay';
    overlay.id = 'vault-overlay';
    d.setOverlayDialogAttributes(overlay, 'Create credential vault');

    overlay.innerHTML = `
      <div class="ssh-form vault-setup-dialog">
        <div class="ssh-form-title">Create Credential Vault</div>
        <div class="ssh-form-body">
          <p class="vault-description">
            The credential vault securely stores SSH credentials using AES-256-GCM
            encryption with an Argon2id-derived key. Choose a strong master password.
          </p>
          <label class="ssh-form-label">Master Password
            <input type="password" id="vault-setup-pw" placeholder="Enter master password"
                   spellcheck="false" autocomplete="off" />
          </label>
          <label class="ssh-form-label">Confirm Password
            <input type="password" id="vault-setup-pw-confirm" placeholder="Confirm master password"
                   spellcheck="false" autocomplete="off" />
          </label>
        </div>
        <div class="ssh-form-buttons">
          <button class="ssh-form-btn" id="vault-setup-cancel">Cancel</button>
          <button class="ssh-form-btn primary" id="vault-setup-create">Create Vault</button>
        </div>
      </div>
    `;

    document.body.appendChild(overlay);
    setTimeout(() => {
      const input = overlay.querySelector('#vault-setup-pw');
      if (input) input.focus();
    }, 50);

    let closed = false;
    const closeDialog = () => {
      if (closed) return;
      closed = true;
      if (typeof unregisterKeys === 'function') unregisterKeys();
      d.removeOverlay();
    };

    const unregisterKeys = d.registerScopedOverlayKeys(overlay, 'vault-setup-dialog', (event) => {
      if (event.key !== 'Escape') return false;
      closeDialog();
      return true;
    });

    overlay.addEventListener('mousedown', (event) => {
      if (event.target === overlay) closeDialog();
    });

    const cancelBtn = overlay.querySelector('#vault-setup-cancel');
    if (cancelBtn) cancelBtn.addEventListener('click', closeDialog);

    const createBtn = overlay.querySelector('#vault-setup-create');
    if (createBtn) {
      createBtn.addEventListener('click', async () => {
        const pwInput = overlay.querySelector('#vault-setup-pw');
        const confirmInput = overlay.querySelector('#vault-setup-pw-confirm');
        const pw = pwInput ? pwInput.value : '';
        const confirm = confirmInput ? confirmInput.value : '';

        if (!pw) {
          if (d.toast && typeof d.toast.warn === 'function') d.toast.warn('Vault', 'Master password is required.');
          if (pwInput) pwInput.focus();
          return;
        }
        if (pw.length < 8) {
          if (d.toast && typeof d.toast.warn === 'function') d.toast.warn('Vault', 'Password must be at least 8 characters.');
          if (pwInput) pwInput.focus();
          return;
        }
        if (pw !== confirm) {
          if (d.toast && typeof d.toast.warn === 'function') d.toast.warn('Vault', 'Passwords do not match.');
          if (confirmInput) confirmInput.focus();
          return;
        }

        try {
          await d.createVault(pw);
          closeDialog();
          if (d.toast && typeof d.toast.success === 'function') {
            d.toast.success('Vault Created', 'Your credential vault is ready.');
          }
          if (typeof onSuccess === 'function') onSuccess();
        } catch (error) {
          if (d.toast && typeof d.toast.error === 'function') {
            d.toast.error('Vault Error', 'Failed to create vault: ' + error);
          }
        }
      });
    }

    return true;
  }

  async function showUnlockDialog(onSuccess, deps) {
    const d = deps || {};
    if (typeof d.removeOverlay !== 'function') return false;
    if (typeof d.registerScopedOverlayKeys !== 'function') return false;
    if (typeof d.setOverlayDialogAttributes !== 'function') return false;
    if (typeof d.unlockVault !== 'function') return false;

    d.removeOverlay();

    const overlay = document.createElement('div');
    overlay.className = 'ssh-overlay';
    overlay.id = 'vault-overlay';
    d.setOverlayDialogAttributes(overlay, 'Unlock credential vault');

    overlay.innerHTML = `
      <div class="ssh-form vault-unlock-dialog">
        <div class="ssh-form-title">Unlock Vault</div>
        <div class="ssh-form-body">
          <label class="ssh-form-label">Master Password
            <input type="password" id="vault-unlock-pw" placeholder="Enter master password"
                   spellcheck="false" autocomplete="off" />
          </label>
        </div>
        <div class="ssh-form-buttons">
          <button class="ssh-form-btn" id="vault-unlock-cancel">Cancel</button>
          <button class="ssh-form-btn primary" id="vault-unlock-submit">Unlock</button>
        </div>
      </div>
    `;

    document.body.appendChild(overlay);
    setTimeout(() => {
      const input = overlay.querySelector('#vault-unlock-pw');
      if (input) input.focus();
    }, 50);

    let closed = false;
    const closeDialog = () => {
      if (closed) return;
      closed = true;
      if (typeof unregisterKeys === 'function') unregisterKeys();
      d.removeOverlay();
    };

    const unregisterKeys = d.registerScopedOverlayKeys(overlay, 'vault-unlock-dialog', (event) => {
      if (event.key !== 'Escape') return false;
      closeDialog();
      return true;
    });

    overlay.addEventListener('mousedown', (event) => {
      if (event.target === overlay) closeDialog();
    });

    const cancelBtn = overlay.querySelector('#vault-unlock-cancel');
    if (cancelBtn) cancelBtn.addEventListener('click', closeDialog);

    const submitUnlock = async () => {
      const pwInput = overlay.querySelector('#vault-unlock-pw');
      const submitBtn = overlay.querySelector('#vault-unlock-submit');
      const pw = pwInput ? pwInput.value : '';
      if (!pw) {
        if (d.toast && typeof d.toast.warn === 'function') d.toast.warn('Vault', 'Password is required.');
        if (pwInput) pwInput.focus();
        return;
      }

      if (submitBtn) {
        submitBtn.disabled = true;
        submitBtn.innerHTML = '<span class="vault-spinner"></span>Unlocking\u2026';
      }
      if (pwInput) pwInput.disabled = true;

      try {
        await d.unlockVault(pw);
        closeDialog();
        if (d.toast && typeof d.toast.success === 'function') {
          d.toast.success('Vault Unlocked', 'Credential vault is now unlocked.');
        }
        if (typeof onSuccess === 'function') onSuccess();
      } catch (error) {
        if (submitBtn) {
          submitBtn.disabled = false;
          submitBtn.textContent = 'Unlock';
        }
        if (pwInput) {
          pwInput.disabled = false;
          pwInput.value = '';
          pwInput.focus();
        }
        if (d.toast && typeof d.toast.error === 'function') {
          d.toast.error('Unlock Failed', String(error));
        }
      }
    };

    const submitBtn = overlay.querySelector('#vault-unlock-submit');
    if (submitBtn) submitBtn.addEventListener('click', submitUnlock);

    const pwInput = overlay.querySelector('#vault-unlock-pw');
    if (pwInput) {
      pwInput.addEventListener('keydown', (event) => {
        if (event.key !== 'Enter') return;
        event.preventDefault();
        submitUnlock();
      });
    }

    return true;
  }

  global.conchVaultDialogs = {
    showSetupDialog,
    showUnlockDialog,
  };
})(window);

(function initTermLabVaultDialogs(global) {
  'use strict';

  function showSetupDialog(onSuccess, deps) {
    const d = deps || {};
    if (!global.tlDialog || typeof global.tlDialog.open !== 'function') return false;
    if (typeof d.createVault !== 'function') return false;

    let handle = null;
    let closed = false;
    const closeDialog = () => {
      if (closed) return;
      closed = true;
      if (handle) handle.close();
    };

    async function doCreate() {
      const pwInput = handle.el.querySelector('#vault-setup-pw');
      const confirmInput = handle.el.querySelector('#vault-setup-pw-confirm');
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
    }

    handle = global.tlDialog.open({
      title: 'Create Credential Vault',
      ariaLabel: 'Create credential vault',
      size: 'sm',
      body: (bodyEl) => {
        bodyEl.innerHTML = `
          <p class="vault-description">
            The credential vault securely stores SSH credentials using AES-256-GCM
            encryption with an Argon2id-derived key. Choose a strong master password.
          </p>
          <div class="tl-field">
            <span class="tl-field__label">Master Password</span>
            <input type="password" class="tl-input" id="vault-setup-pw" placeholder="Enter master password"
                   spellcheck="false" autocomplete="off" />
          </div>
          <div class="tl-field">
            <span class="tl-field__label">Confirm Password</span>
            <input type="password" class="tl-input" id="vault-setup-pw-confirm" placeholder="Confirm master password"
                   spellcheck="false" autocomplete="off" />
          </div>
        `;
        const input = bodyEl.querySelector('#vault-setup-pw');
        setTimeout(() => { if (input) input.focus(); }, 50);
      },
      buttons: [
        { label: 'Cancel', onSelect: closeDialog },
        { label: 'Create Vault', primary: true, onSelect: doCreate },
      ],
      onClose: closeDialog,
      // No panel-level Enter shortcut here: the original overlay never
      // bound Enter for this dialog (only Escape, via the old
      // registerScopedOverlayKeys), so none is added here either.
    });

    return handle;
  }

  function showUnlockDialog(onSuccess, deps) {
    const d = deps || {};
    if (!global.tlDialog || typeof global.tlDialog.open !== 'function') return false;
    if (typeof d.unlockVault !== 'function') return false;

    let handle = null;
    let closed = false;
    const closeDialog = () => {
      if (closed) return;
      closed = true;
      if (handle) handle.close();
    };

    const submitUnlock = async () => {
      const pwInput = handle.el.querySelector('#vault-unlock-pw');
      const submitBtn = handle.el.querySelector('.tl-dialog__footer .tl-btn--primary');
      const pw = pwInput ? pwInput.value : '';
      if (!pw) {
        if (d.toast && typeof d.toast.warn === 'function') d.toast.warn('Vault', 'Password is required.');
        if (pwInput) pwInput.focus();
        return;
      }

      if (submitBtn) {
        submitBtn.disabled = true;
        submitBtn.innerHTML = '<span class="vault-spinner"></span>Unlocking…';
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

    handle = global.tlDialog.open({
      title: 'Unlock Vault',
      ariaLabel: 'Unlock credential vault',
      size: 'sm',
      body: (bodyEl) => {
        bodyEl.innerHTML = `
          <div class="tl-field">
            <span class="tl-field__label">Master Password</span>
            <input type="password" class="tl-input" id="vault-unlock-pw" placeholder="Enter master password"
                   spellcheck="false" autocomplete="off" />
          </div>
        `;
        const input = bodyEl.querySelector('#vault-unlock-pw');
        if (input) {
          // Input-scoped, matching the original (the old overlay only
          // wired Enter on this specific field, not the whole overlay).
          input.addEventListener('keydown', (event) => {
            if (event.key !== 'Enter') return;
            event.preventDefault();
            submitUnlock();
          });
        }
        setTimeout(() => { if (input) input.focus(); }, 50);
      },
      buttons: [
        { label: 'Cancel', onSelect: closeDialog },
        { label: 'Unlock', primary: true, onSelect: submitUnlock },
      ],
      onClose: closeDialog,
    });

    return handle;
  }

  global.termlabVaultDialogs = {
    showSetupDialog,
    showUnlockDialog,
  };
})(window);

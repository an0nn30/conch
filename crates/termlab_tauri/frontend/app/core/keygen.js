// SSH Key Generation — keygen dialog, result view.

(function (exports) {
  'use strict';

  let invoke = null;

  const esc = window.utils.esc;
  const attr = window.utils.attr;

  // Key type definitions: value sent to backend, display label, default filename.
  const KEY_TYPES = [
    { value: 'ed25519',    label: 'Ed25519 (recommended)', filename: 'id_ed25519' },
    { value: 'ecdsa-p256', label: 'ECDSA P-256',           filename: 'id_ecdsa' },
    { value: 'ecdsa-p384', label: 'ECDSA P-384',           filename: 'id_ecdsa' },
    { value: 'rsa-sha256', label: 'RSA (SHA-256)',           filename: 'id_rsa' },
    { value: 'rsa-sha512', label: 'RSA (SHA-512)',           filename: 'id_rsa' },
  ];

  function init(opts) {
    invoke = opts.invoke;
  }

  // ---------------------------------------------------------------------------
  // removeOverlay — close whichever keygen dialog (form or result) is open.
  // Only one of the two is ever open at a time (showResultDialog is reached
  // by closing the form first), so a single tracked handle is enough.
  // ---------------------------------------------------------------------------

  let activeHandle = null;

  function removeOverlay() {
    if (activeHandle) {
      const handle = activeHandle;
      activeHandle = null;
      handle.close();
    }
  }

  // ---------------------------------------------------------------------------
  // showKeygenDialog — main key generation form
  // ---------------------------------------------------------------------------

  function showKeygenDialog(opts) {
    opts = opts || {};
    removeOverlay();
    if (!window.tlDialog || typeof window.tlDialog.open !== 'function') return;

    const keyTypeOptions = KEY_TYPES.map((kt) =>
      '<option value="' + attr(kt.value) + '">' + esc(kt.label) + '</option>'
    ).join('');

    let handle = null;
    let dismissed = false;
    const dismissDialog = () => {
      if (dismissed) return;
      dismissed = true;
      if (activeHandle === handle) activeHandle = null;
      if (handle) handle.close();
    };

    handle = window.tlDialog.open({
      title: 'Generate SSH Key Pair',
      ariaLabel: 'Generate SSH key pair',
      size: 'md',
      body: (bodyEl) => {
        bodyEl.innerHTML = `
          <div class="tl-field">
            <span class="tl-field__label">Key Type</span>
            <select class="tl-combo-select" id="keygen-type">
              ${keyTypeOptions}
            </select>
          </div>
          <div class="tl-field">
            <span class="tl-field__label">Comment</span>
            <input type="text" class="tl-input" id="keygen-comment" value="user@termlab"
                   placeholder="user@hostname" spellcheck="false" autocomplete="off" />
          </div>
          <div class="tl-field">
            <span class="tl-field__label">Passphrase (optional)</span>
            <input type="password" class="tl-input" id="keygen-passphrase"
                   placeholder="Leave empty for no passphrase"
                   spellcheck="false" autocomplete="off" />
          </div>
          <div class="tl-field">
            <span class="tl-field__label">Confirm Passphrase</span>
            <input type="password" class="tl-input" id="keygen-passphrase-confirm"
                   placeholder="Confirm passphrase"
                   spellcheck="false" autocomplete="off" />
          </div>
          <div class="tl-field">
            <span class="tl-field__label">Save Path</span>
            <div class="keygen-path-row">
              <input type="text" class="tl-input" id="keygen-path" value="~/.ssh/id_ed25519"
                     placeholder="~/.ssh/id_ed25519" spellcheck="false" autocomplete="off" />
              <button class="tl-btn keygen-browse-btn" id="keygen-browse" type="button">Browse</button>
            </div>
          </div>
          <div class="keygen-note">
            Output format: OpenSSH. Public key saved as &lt;path&gt;.pub
          </div>
        `;

        const typeSelect = bodyEl.querySelector('#keygen-type');
        if (window.tlCombo && typeof window.tlCombo.attach === 'function') window.tlCombo.attach(typeSelect);
        const pathInput = bodyEl.querySelector('#keygen-path');

        // tlCombo.attach() hides the native <select> (display: none) and
        // shows a .tl-combo button in its place, so the select itself can
        // no longer receive focus — focus the visible button instead.
        setTimeout(() => {
          const focusTarget = (typeSelect._tlCombo && typeSelect._tlCombo.button) || typeSelect;
          focusTarget.focus();
        }, 50);

        // Auto-update the save path filename when key type changes.
        typeSelect.addEventListener('change', () => {
          const kt = KEY_TYPES.find((k) => k.value === typeSelect.value);
          if (!kt) return;
          // Replace only the filename portion — keep any directory the user set.
          const current = pathInput.value;
          const lastSlash = current.lastIndexOf('/');
          const dir = lastSlash >= 0 ? current.substring(0, lastSlash + 1) : '~/.ssh/';
          pathInput.value = dir + kt.filename;
        });

        // Browse button — use Tauri save dialog if available, otherwise focus the input.
        bodyEl.querySelector('#keygen-browse').addEventListener('click', async () => {
          try {
            const dialog = window.__TAURI__ && window.__TAURI__.dialog;
            if (dialog && dialog.save) {
              const selected = await dialog.save({
                title: 'Choose key save location',
                defaultPath: pathInput.value,
              });
              if (selected) {
                pathInput.value = selected;
              }
            } else {
              // Fallback: just focus the path input so user can type.
              pathInput.focus();
              pathInput.select();
            }
          } catch (_) {
            pathInput.focus();
            pathInput.select();
          }
        });

        // Helper: actually run the key generation.
        async function doGenerate() {
          const keyType = typeSelect.value;
          const comment = bodyEl.querySelector('#keygen-comment').value.trim();
          const passphrase = bodyEl.querySelector('#keygen-passphrase').value;
          const savePath = pathInput.value.trim();

          const generateBtn = bodyEl.closest('.tl-dialog').querySelector('.tl-dialog__footer .tl-btn--primary');
          if (generateBtn) {
            generateBtn.disabled = true;
            generateBtn.textContent = 'Generating…';
          }

          try {
            const result = await invoke('vault_generate_key', {
              request: {
                key_type: keyType,
                comment: comment || null,
                passphrase: passphrase || null,
                save_path: savePath,
              },
            });

            dismissDialog();
            showResultDialog(result, opts);
          } catch (e) {
            if (generateBtn) {
              generateBtn.disabled = false;
              generateBtn.textContent = 'Generate';
            }
            window.toast.error('Key Generation Failed', String(e));
          }
        }
        bodyEl._keygenDoGenerate = doGenerate;
      },
      buttons: [
        { label: 'Cancel', onSelect: dismissDialog },
        { label: 'Generate', primary: true, onSelect: async () => {
          const bodyEl = handle.el.querySelector('.tl-dialog__body');
          const passphrase = bodyEl.querySelector('#keygen-passphrase').value;
          const passphraseConfirm = bodyEl.querySelector('#keygen-passphrase-confirm').value;
          const pathInput = bodyEl.querySelector('#keygen-path');
          const savePath = pathInput.value.trim();

          if (!savePath) {
            window.toast.warn('Key Generation', 'Save path is required.');
            pathInput.focus();
            return;
          }

          if (passphrase !== passphraseConfirm) {
            window.toast.warn('Key Generation', 'Passphrases do not match.');
            bodyEl.querySelector('#keygen-passphrase-confirm').focus();
            return;
          }

          // Remove any previous overwrite warning.
          const oldWarn = bodyEl.querySelector('.keygen-overwrite-warning');
          if (oldWarn) oldWarn.remove();

          // Check if the file already exists on disk.
          try {
            const exists = await invoke('vault_check_path_exists', { path: savePath });
            if (exists) {
              const noteEl = bodyEl.querySelector('.keygen-note');
              const warning = document.createElement('div');
              warning.className = 'keygen-overwrite-warning';
              warning.innerHTML = '<span class="keygen-overwrite-text">A key file already exists at this path. Overwrite?</span>'
                + ' <button class="tl-btn keygen-overwrite-btn" id="keygen-overwrite">Overwrite</button>'
                + ' <button class="tl-btn" id="keygen-overwrite-cancel">Cancel</button>';
              noteEl.parentNode.insertBefore(warning, noteEl.nextSibling);

              warning.querySelector('#keygen-overwrite').addEventListener('click', () => {
                warning.remove();
                if (typeof bodyEl._keygenDoGenerate === 'function') bodyEl._keygenDoGenerate();
              });
              warning.querySelector('#keygen-overwrite-cancel').addEventListener('click', () => {
                warning.remove();
              });
              return;
            }
          } catch (_) {
            // If the check fails, proceed with generation — save_key_to_disk
            // will surface any real filesystem errors.
          }

          if (typeof bodyEl._keygenDoGenerate === 'function') bodyEl._keygenDoGenerate();
        } },
      ],
      onClose: dismissDialog,
    });
    activeHandle = handle;
  }

  // ---------------------------------------------------------------------------
  // showResultDialog — post-generation success view
  // ---------------------------------------------------------------------------

  function showResultDialog(result, opts) {
    opts = opts || {};
    removeOverlay();
    if (!window.tlDialog || typeof window.tlDialog.open !== 'function') return;

    let handle = null;
    let dismissed = false;
    const dismissDialog = () => {
      if (dismissed) return;
      dismissed = true;
      if (activeHandle === handle) activeHandle = null;
      if (handle) handle.close();
    };

    handle = window.tlDialog.open({
      title: '✓ Key Generated',
      ariaLabel: 'SSH key generated',
      size: 'md',
      body: (bodyEl) => {
        bodyEl.innerHTML = `
          <div class="keygen-result-row">
            <span class="keygen-result-label">Algorithm</span>
            <span class="keygen-result-value">${esc(result.algorithm || '')}</span>
          </div>
          <div class="keygen-result-row">
            <span class="keygen-result-label">Fingerprint</span>
            <span class="keygen-result-value keygen-mono">${esc(result.fingerprint || '')}</span>
          </div>
          <div class="keygen-result-row">
            <span class="keygen-result-label">Private key</span>
            <span class="keygen-result-value keygen-mono">${esc(result.private_path || '')}</span>
          </div>
          <div class="keygen-result-row">
            <span class="keygen-result-label">Public key</span>
            <span class="keygen-result-value keygen-mono">${esc(result.public_path || '')}</span>
          </div>
          <div class="keygen-pubkey-block">
            <div class="keygen-pubkey-header">
              <span class="keygen-pubkey-label">Public Key</span>
              <button class="tl-btn keygen-copy-btn" id="keygen-copy-pubkey" type="button">Copy</button>
            </div>
            <textarea class="keygen-pubkey-text" readonly rows="3">${esc(result.public_key || '')}</textarea>
          </div>
        `;

        // Copy public key to clipboard.
        bodyEl.querySelector('#keygen-copy-pubkey').addEventListener('click', async () => {
          try {
            await navigator.clipboard.writeText(result.public_key || '');
            window.toast.success('Copied', 'Public key copied to clipboard.');
          } catch (e) {
            window.toast.error('Copy Failed', 'Could not copy to clipboard: ' + e);
          }
        });
      },
      buttons: [
        { label: 'Close', onSelect: dismissDialog },
        { label: 'Create Vault Account with Key', primary: true, onSelect: () => {
          dismissDialog();
          if (window.vault && window.vault.showAccountForm) {
            // Pre-fill the account form with the generated key path.
            window.vault.ensureUnlocked(() => {
              window.vault.showAccountForm({
                display_name: '',
                username: '',
                auth_type: 'key',
                key_path: result.private_path || '',
              });
            });
          } else {
            window.toast.warn('Vault', 'Vault module is not available.');
          }
        } },
      ],
      onClose: dismissDialog,
    });
    activeHandle = handle;
  }

  exports.keygen = {
    init,
    showKeygenDialog,
    showResultDialog,
  };
})(window);

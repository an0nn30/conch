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
    { value: 'rsa-2048',   label: 'RSA 2048',              filename: 'id_rsa' },
    { value: 'rsa-4096',   label: 'RSA 4096',              filename: 'id_rsa' },
  ];

  function init(opts) {
    invoke = opts.invoke;
  }

  // ---------------------------------------------------------------------------
  // removeOverlay — clean up any existing keygen overlay
  // ---------------------------------------------------------------------------

  function removeOverlay() {
    const el = document.getElementById('keygen-overlay');
    if (el) el.remove();
  }

  // ---------------------------------------------------------------------------
  // showKeygenDialog — main key generation form
  // ---------------------------------------------------------------------------

  function showKeygenDialog(opts) {
    opts = opts || {};
    removeOverlay();

    const overlay = document.createElement('div');
    overlay.className = 'ssh-overlay';
    overlay.id = 'keygen-overlay';

    const keyTypeOptions = KEY_TYPES.map((kt) =>
      '<option value="' + attr(kt.value) + '">' + esc(kt.label) + '</option>'
    ).join('');

    overlay.innerHTML = `
      <div class="ssh-form keygen-dialog">
        <div class="ssh-form-title">Generate SSH Key Pair</div>
        <div class="ssh-form-body">
          <label class="ssh-form-label">Key Type
            <select id="keygen-type">
              ${keyTypeOptions}
            </select>
          </label>
          <label class="ssh-form-label">Comment
            <input type="text" id="keygen-comment" value="user@conch"
                   placeholder="user@hostname" spellcheck="false" autocomplete="off" />
          </label>
          <label class="ssh-form-label">Passphrase (optional)
            <input type="password" id="keygen-passphrase"
                   placeholder="Leave empty for no passphrase"
                   spellcheck="false" autocomplete="off" />
          </label>
          <label class="ssh-form-label">Confirm Passphrase
            <input type="password" id="keygen-passphrase-confirm"
                   placeholder="Confirm passphrase"
                   spellcheck="false" autocomplete="off" />
          </label>
          <label class="ssh-form-label">Save Path
            <div class="keygen-path-row">
              <input type="text" id="keygen-path" value="~/.ssh/id_ed25519"
                     placeholder="~/.ssh/id_ed25519" spellcheck="false" autocomplete="off" />
              <button class="ssh-form-btn keygen-browse-btn" id="keygen-browse" type="button">Browse</button>
            </div>
          </label>
          <div class="keygen-note">
            Output format: OpenSSH. Public key saved as &lt;path&gt;.pub
          </div>
        </div>
        <div class="ssh-form-buttons">
          <button class="ssh-form-btn" id="keygen-cancel">Cancel</button>
          <button class="ssh-form-btn primary" id="keygen-generate">Generate</button>
        </div>
      </div>
    `;

    document.body.appendChild(overlay);
    setTimeout(() => overlay.querySelector('#keygen-type').focus(), 50);

    // Auto-update the save path filename when key type changes.
    const typeSelect = overlay.querySelector('#keygen-type');
    const pathInput = overlay.querySelector('#keygen-path');

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
    overlay.querySelector('#keygen-browse').addEventListener('click', async () => {
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

    // Click outside to close.
    overlay.addEventListener('mousedown', (e) => {
      if (e.target === overlay) {
        removeOverlay();
        document.removeEventListener('keydown', onKey, true);
      }
    });

    const onKey = (e) => {
      if (e.key === 'Escape') {
        removeOverlay();
        document.removeEventListener('keydown', onKey, true);
      }
    };
    document.addEventListener('keydown', onKey, true);

    overlay.querySelector('#keygen-cancel').addEventListener('click', () => {
      removeOverlay();
      document.removeEventListener('keydown', onKey, true);
    });

    overlay.querySelector('#keygen-generate').addEventListener('click', async () => {
      const keyType = typeSelect.value;
      const comment = overlay.querySelector('#keygen-comment').value.trim();
      const passphrase = overlay.querySelector('#keygen-passphrase').value;
      const passphraseConfirm = overlay.querySelector('#keygen-passphrase-confirm').value;
      const savePath = pathInput.value.trim();

      if (!savePath) {
        window.toast.warn('Key Generation', 'Save path is required.');
        pathInput.focus();
        return;
      }

      if (passphrase !== passphraseConfirm) {
        window.toast.warn('Key Generation', 'Passphrases do not match.');
        overlay.querySelector('#keygen-passphrase-confirm').focus();
        return;
      }

      const generateBtn = overlay.querySelector('#keygen-generate');
      generateBtn.disabled = true;
      generateBtn.textContent = 'Generating…';

      try {
        const result = await invoke('vault_generate_key', {
          request: {
            key_type: keyType,
            comment: comment || null,
            passphrase: passphrase || null,
            save_path: savePath,
          },
        });

        // Clean up the key listener before showing result.
        document.removeEventListener('keydown', onKey, true);
        removeOverlay();
        showResultDialog(result, opts);
      } catch (e) {
        generateBtn.disabled = false;
        generateBtn.textContent = 'Generate';
        window.toast.error('Key Generation Failed', String(e));
      }
    });
  }

  // ---------------------------------------------------------------------------
  // showResultDialog — post-generation success view
  // ---------------------------------------------------------------------------

  function showResultDialog(result, opts) {
    opts = opts || {};
    removeOverlay();

    const overlay = document.createElement('div');
    overlay.className = 'ssh-overlay';
    overlay.id = 'keygen-overlay';

    overlay.innerHTML = `
      <div class="ssh-form keygen-result-dialog">
        <div class="ssh-form-title">
          <span class="keygen-success-icon">&#10003;</span> Key Generated
        </div>
        <div class="ssh-form-body">
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
              <button class="ssh-form-btn keygen-copy-btn" id="keygen-copy-pubkey" type="button">Copy</button>
            </div>
            <textarea class="keygen-pubkey-text" readonly rows="3">${esc(result.public_key || '')}</textarea>
          </div>
        </div>
        <div class="ssh-form-buttons">
          <button class="ssh-form-btn" id="keygen-result-close">Close</button>
          <button class="ssh-form-btn primary" id="keygen-create-vault-account">Create Vault Account with Key</button>
        </div>
      </div>
    `;

    document.body.appendChild(overlay);

    // Copy public key to clipboard.
    overlay.querySelector('#keygen-copy-pubkey').addEventListener('click', async () => {
      try {
        await navigator.clipboard.writeText(result.public_key || '');
        window.toast.success('Copied', 'Public key copied to clipboard.');
      } catch (e) {
        window.toast.error('Copy Failed', 'Could not copy to clipboard: ' + e);
      }
    });

    const onKey = (e) => {
      if (e.key === 'Escape') {
        removeOverlay();
        document.removeEventListener('keydown', onKey, true);
      }
    };
    document.addEventListener('keydown', onKey, true);

    overlay.addEventListener('mousedown', (e) => {
      if (e.target === overlay) {
        removeOverlay();
        document.removeEventListener('keydown', onKey, true);
      }
    });

    overlay.querySelector('#keygen-result-close').addEventListener('click', () => {
      removeOverlay();
      document.removeEventListener('keydown', onKey, true);
    });

    overlay.querySelector('#keygen-create-vault-account').addEventListener('click', () => {
      removeOverlay();
      document.removeEventListener('keydown', onKey, true);

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
    });
  }

  exports.keygen = {
    init,
    showKeygenDialog,
    showResultDialog,
  };
})(window);

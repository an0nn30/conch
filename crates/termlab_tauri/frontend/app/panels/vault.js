// Vault Management — unlock dialog, setup dialog, account CRUD, settings.

(function (exports) {
  'use strict';

  let invoke = null;
  let listen = null;

  // Cached state
  let cachedAccounts = [];
  let lockTimerInterval = null;

  const esc = window.utils.esc;
  const attr = window.utils.attr;
  const vaultDataService = exports.termlabVaultFeatureDataService || {};
  const vaultSections = exports.termlabVaultSections || {};
  const vaultDialogs = exports.termlabVaultDialogs || {};
  const vaultAccountFormFeature = exports.termlabVaultAccountForm || {};

  // The tl-dialog handle for whichever vault dialog is currently open
  // (setup, unlock, the main sidebar dialog, or the account form).
  // removeOverlay() below closes only this — never a blanket DOM query —
  // so it can't reach into ssh/keygen/files/plugin/tunnel dialogs. Same
  // pattern as ssh-panel.js's activeDialogHandle/trackDialogHandle.
  let activeVaultDialogHandle = null;

  function trackDialogHandle(handle) {
    if (!handle) return handle;
    const originalClose = handle.close;
    handle.close = function (result) {
      if (activeVaultDialogHandle === handle) activeVaultDialogHandle = null;
      return originalClose.call(handle, result);
    };
    activeVaultDialogHandle = handle;
    return handle;
  }

  function getVaultStatus() {
    if (!vaultDataService || typeof vaultDataService.getStatus !== 'function') {
      return Promise.reject(new Error('Vault data service unavailable: getStatus'));
    }
    return vaultDataService.getStatus(invoke);
  }

  function createVault(password) {
    if (!vaultDataService || typeof vaultDataService.createVault !== 'function') {
      return Promise.reject(new Error('Vault data service unavailable: createVault'));
    }
    return vaultDataService.createVault(invoke, password);
  }

  function unlockVault(password) {
    if (!vaultDataService || typeof vaultDataService.unlockVault !== 'function') {
      return Promise.reject(new Error('Vault data service unavailable: unlockVault'));
    }
    return vaultDataService.unlockVault(invoke, password);
  }

  function lockVault() {
    if (!vaultDataService || typeof vaultDataService.lockVault !== 'function') {
      return Promise.reject(new Error('Vault data service unavailable: lockVault'));
    }
    return vaultDataService.lockVault(invoke);
  }

  function listAccounts() {
    if (!vaultDataService || typeof vaultDataService.listAccounts !== 'function') {
      return Promise.reject(new Error('Vault data service unavailable: listAccounts'));
    }
    return vaultDataService.listAccounts(invoke);
  }

  function getAccount(id) {
    if (!vaultDataService || typeof vaultDataService.getAccount !== 'function') {
      return Promise.reject(new Error('Vault data service unavailable: getAccount'));
    }
    return vaultDataService.getAccount(invoke, id);
  }

  function addAccount(request) {
    if (!vaultDataService || typeof vaultDataService.addAccount !== 'function') {
      return Promise.reject(new Error('Vault data service unavailable: addAccount'));
    }
    return vaultDataService.addAccount(invoke, request);
  }

  function updateAccount(request) {
    if (!vaultDataService || typeof vaultDataService.updateAccount !== 'function') {
      return Promise.reject(new Error('Vault data service unavailable: updateAccount'));
    }
    return vaultDataService.updateAccount(invoke, request);
  }

  function deleteAccount(id) {
    if (!vaultDataService || typeof vaultDataService.deleteAccount !== 'function') {
      return Promise.reject(new Error('Vault data service unavailable: deleteAccount'));
    }
    return vaultDataService.deleteAccount(invoke, id);
  }

  function listKeys() {
    if (!vaultDataService || typeof vaultDataService.listKeys !== 'function') {
      return Promise.reject(new Error('Vault data service unavailable: listKeys'));
    }
    return vaultDataService.listKeys(invoke);
  }

  function deleteKey(id) {
    if (!vaultDataService || typeof vaultDataService.deleteKey !== 'function') {
      return Promise.reject(new Error('Vault data service unavailable: deleteKey'));
    }
    return vaultDataService.deleteKey(invoke, id);
  }

  function getVaultSettings() {
    if (!vaultDataService || typeof vaultDataService.getSettings !== 'function') {
      return Promise.reject(new Error('Vault data service unavailable: getSettings'));
    }
    return vaultDataService.getSettings(invoke);
  }

  function updateVaultSettings(settings) {
    if (!vaultDataService || typeof vaultDataService.updateSettings !== 'function') {
      return Promise.reject(new Error('Vault data service unavailable: updateSettings'));
    }
    return vaultDataService.updateSettings(invoke, settings);
  }

  function pickKeyFile() {
    if (!vaultDataService || typeof vaultDataService.pickKeyFile !== 'function') {
      return Promise.reject(new Error('Vault data service unavailable: pickKeyFile'));
    }
    return vaultDataService.pickKeyFile(invoke);
  }

  function init(opts) {
    invoke = opts.invoke;
    listen = opts.listen;

    // Listen for menu-driven vault events.
    listen('vault-locked', () => {
      // Auto-lock fired from backend — dismiss any vault dialogs and clear cache.
      cachedAccounts = [];
      stopLockTimer();
      removeOverlay();
      window.toast.info('Vault Locked', 'The credential vault has been locked.');
    });
  }

  // ---------------------------------------------------------------------------
  // ensureUnlocked — check status, prompt if needed, then call callback
  // ---------------------------------------------------------------------------

  async function ensureUnlocked(callback) {
    try {
      const status = await getVaultStatus();
      if (!status.exists) {
        showSetupDialog(() => {
          if (callback) callback();
        });
        return;
      }
      if (status.locked) {
        showUnlockDialog(() => {
          if (callback) callback();
        });
        return;
      }
      // Already unlocked.
      if (callback) callback();
    } catch (e) {
      window.toast.error('Vault Error', 'Failed to check vault status: ' + e);
    }
  }

  // ---------------------------------------------------------------------------
  // Setup dialog — first-time vault creation
  // ---------------------------------------------------------------------------

  function showSetupDialog(onSuccess) {
    removeOverlay();
    if (vaultDialogs && typeof vaultDialogs.showSetupDialog === 'function') {
      const handle = vaultDialogs.showSetupDialog(onSuccess, {
        createVault,
        toast: window.toast,
      });
      if (handle) { trackDialogHandle(handle); return; }
    }
    if (window.toast && typeof window.toast.error === 'function') {
      window.toast.error('Vault Error', 'Vault setup dialog module is unavailable.');
    }
  }

  // ---------------------------------------------------------------------------
  // Unlock dialog — master password input
  // ---------------------------------------------------------------------------

  async function showUnlockDialog(onSuccess) {
    removeOverlay();
    if (vaultDialogs && typeof vaultDialogs.showUnlockDialog === 'function') {
      const handle = vaultDialogs.showUnlockDialog(onSuccess, {
        unlockVault,
        toast: window.toast,
      });
      if (handle) { trackDialogHandle(handle); return; }
    }
    if (window.toast && typeof window.toast.error === 'function') {
      window.toast.error('Vault Error', 'Vault unlock dialog module is unavailable.');
    }
  }

  // ---------------------------------------------------------------------------
  // Vault management dialog — sidebar with Accounts / SSH Keys / Settings
  // ---------------------------------------------------------------------------

  const VAULT_SECTIONS = [
    { id: 'accounts', label: 'User Accounts' },
    { id: 'keys', label: 'SSH Keys' },
    { id: 'settings', label: 'Settings' },
  ];

  let currentSection = 'accounts';

  async function showVaultDialog() {
    // Ensure vault is unlocked first.
    const status = await getVaultStatus().catch(() => null);
    if (!status) return;

    if (!status.exists) {
      showSetupDialog(() => showVaultDialog());
      return;
    }
    if (status.locked) {
      showUnlockDialog(() => showVaultDialog());
      return;
    }

    currentSection = 'accounts';
    await renderVaultDialog();
  }

  async function renderVaultDialog() {
    removeOverlay();

    // Load data for current section.
    let accounts = [];
    let settings = null;
    try {
      accounts = await listAccounts();
      settings = await getVaultSettings();
      cachedAccounts = accounts;
    } catch (e) {
      window.toast.error('Vault Error', 'Failed to load vault data: ' + e);
      return;
    }

    const status = await getVaultStatus().catch(() => ({ exists: true, locked: false, seconds_remaining: 0 }));

    if (!window.tlDialog || typeof window.tlDialog.open !== 'function') {
      window.toast.error('Vault Error', 'Dialog shell module is unavailable.');
      return;
    }

    let handle = null;
    let closed = false;
    const closeDialog = () => {
      if (closed) return;
      closed = true;
      stopLockTimer();
      if (handle) handle.close();
    };

    handle = window.tlDialog.open({
      title: 'Credential Vault',
      ariaLabel: 'Credential vault',
      size: 'lg',
      body: (bodyEl) => {
        // Body = sidebar + content — internal layout markup unchanged from
        // the pre-migration overlay, only the surrounding shell moved.
        const body = document.createElement('div');
        body.className = 'vault-body';

        // Sidebar
        const sidebar = document.createElement('div');
        sidebar.className = 'vault-sidebar';

        // switchSection — swap content area without rebuilding the dialog.
        async function switchSection(sectionId) {
          currentSection = sectionId;

          // Update sidebar active state.
          sidebar.querySelectorAll('.vault-sidebar-item').forEach((el) => {
            const isActive = el.dataset.section === sectionId;
            el.classList.toggle('active', isActive);
            el.setAttribute('aria-current', isActive ? 'page' : 'false');
          });

          // Rebuild just the content area.
          const contentEl = bodyEl.querySelector('#vault-content');
          if (!contentEl) return;
          contentEl.innerHTML = '';

          if (sectionId === 'accounts') {
            // Re-fetch accounts so additions/edits are reflected.
            try {
              accounts = await listAccounts();
              cachedAccounts = accounts;
            } catch (_) {}
            renderAccountsSection(contentEl, accounts);
          } else if (sectionId === 'keys') {
            await renderKeysSection(contentEl);
          } else if (sectionId === 'settings') {
            try { settings = await getVaultSettings(); } catch (_) {}
            renderSettingsSection(contentEl, settings);
          }
        }

        for (const sec of VAULT_SECTIONS) {
          const item = document.createElement('div');
          item.className = 'vault-sidebar-item' + (sec.id === currentSection ? ' active' : '');
          item.dataset.section = sec.id;
          item.textContent = sec.label;
          item.setAttribute('role', 'button');
          item.tabIndex = 0;
          item.setAttribute('aria-current', sec.id === currentSection ? 'page' : 'false');
          item.addEventListener('click', () => switchSection(sec.id));
          item.addEventListener('keydown', (event) => {
            if (event.key !== 'Enter' && event.key !== ' ') return;
            event.preventDefault();
            switchSection(sec.id);
          });
          sidebar.appendChild(item);
        }

        // Sidebar footer — lock status + lock button
        const footer = document.createElement('div');
        footer.className = 'vault-sidebar-footer';
        footer.innerHTML = `
          <div class="vault-lock-status">
            <span class="vault-status-dot unlocked"></span>
            <span id="vault-lock-countdown">${formatCountdown(status.seconds_remaining)}</span>
          </div>
          <button class="vault-lock-btn" id="vault-lock-now">Lock Now</button>
        `;
        sidebar.appendChild(footer);

        body.appendChild(sidebar);

        // Content area
        const content = document.createElement('div');
        content.className = 'vault-content';
        content.id = 'vault-content';

        if (currentSection === 'accounts') {
          renderAccountsSection(content, accounts);
        } else if (currentSection === 'keys') {
          renderKeysSection(content);
        } else if (currentSection === 'settings') {
          renderSettingsSection(content, settings);
        }

        body.appendChild(content);
        bodyEl.appendChild(body);

        // Start countdown timer — queries #vault-lock-countdown/.vault-status-dot
        // within bodyEl, same as the old overlay-scoped lookup.
        startLockTimer(bodyEl);

        bodyEl.querySelector('#vault-lock-now').addEventListener('click', async () => {
          try {
            await lockVault();
            closeDialog();
            cachedAccounts = [];
            window.toast.info('Vault Locked', 'Credential vault has been locked.');
          } catch (e) {
            window.toast.error('Vault Error', 'Failed to lock vault: ' + e);
          }
        });
      },
      buttons: [
        { label: 'Close', onSelect: closeDialog },
      ],
      onClose: closeDialog,
    });

    trackDialogHandle(handle);
  }

  // ---------------------------------------------------------------------------
  // Accounts section
  // ---------------------------------------------------------------------------

  function renderAccountsSection(container, accounts) {
    if (vaultSections && typeof vaultSections.renderAccountsSection === 'function') {
      vaultSections.renderAccountsSection(container, accounts, {
        showAccountForm,
        getAccount,
        deleteAccount,
        renderVaultDialog,
        toast: window.toast,
      });
      return;
    }
    if (window.toast && typeof window.toast.error === 'function') {
      window.toast.error('Vault Error', 'Vault sections module is unavailable.');
    }
  }

  // ---------------------------------------------------------------------------
  // Keys section
  // ---------------------------------------------------------------------------

  async function renderKeysSection(container) {
    if (vaultSections && typeof vaultSections.renderKeysSection === 'function') {
      await vaultSections.renderKeysSection(container, {
        listKeys,
        deleteKey,
        renderVaultDialog,
        keygen: window.keygen,
        toast: window.toast,
      });
      return;
    }
    if (window.toast && typeof window.toast.error === 'function') {
      window.toast.error('Vault Error', 'Vault sections module is unavailable.');
    }
  }

  // ---------------------------------------------------------------------------
  // Settings section
  // ---------------------------------------------------------------------------

  function renderSettingsSection(container, settings) {
    if (vaultSections && typeof vaultSections.renderSettingsSection === 'function') {
      vaultSections.renderSettingsSection(container, settings, {
        updateVaultSettings,
        toast: window.toast,
      });
      return;
    }
    if (window.toast && typeof window.toast.error === 'function') {
      window.toast.error('Vault Error', 'Vault sections module is unavailable.');
    }
  }

  // ---------------------------------------------------------------------------
  // Account form — create / edit
  // ---------------------------------------------------------------------------

  async function showAccountForm(existing) {
    removeOverlay();
    if (vaultAccountFormFeature && typeof vaultAccountFormFeature.showAccountForm === 'function') {
      const handle = await vaultAccountFormFeature.showAccountForm(existing, {
        listKeys,
        pickKeyFile,
        updateAccount,
        addAccount,
        renderVaultDialog,
        attr,
        esc,
        toast: window.toast,
        keygen: window.keygen,
      });
      if (handle) { trackDialogHandle(handle); return; }
    }
    if (window.toast && typeof window.toast.error === 'function') {
      window.toast.error('Vault Error', 'Vault account form module is unavailable.');
    }
  }

  // ---------------------------------------------------------------------------
  // getAccounts — return cached account list for external consumers
  // ---------------------------------------------------------------------------

  async function getAccounts() {
    try {
      const status = await getVaultStatus();
      if (!status.exists || status.locked) return [];
      cachedAccounts = await listAccounts();
      return cachedAccounts;
    } catch (e) {
      return cachedAccounts;
    }
  }

  // ---------------------------------------------------------------------------
  // Lock timer
  // ---------------------------------------------------------------------------

  function startLockTimer(overlay) {
    stopLockTimer();
    lockTimerInterval = setInterval(async () => {
      try {
        const status = await getVaultStatus();
        const el = overlay.querySelector('#vault-lock-countdown');
        if (el) el.textContent = formatCountdown(status.seconds_remaining);

        const dot = overlay.querySelector('.vault-status-dot');
        if (dot) {
          dot.className = 'vault-status-dot ' + (status.locked ? 'locked' : 'unlocked');
        }

        if (status.locked) {
          stopLockTimer();
          removeOverlay();
          cachedAccounts = [];
          window.toast.info('Vault Locked', 'The vault was auto-locked due to inactivity.');
        }
      } catch (_) {
        // Ignore polling errors.
      }
    }, 5000);
  }

  function stopLockTimer() {
    if (lockTimerInterval) {
      clearInterval(lockTimerInterval);
      lockTimerInterval = null;
    }
  }

  function formatCountdown(seconds) {
    if (seconds <= 0) return 'Locked';
    const m = Math.floor(seconds / 60);
    const s = seconds % 60;
    return m + ':' + String(s).padStart(2, '0') + ' remaining';
  }

  // ---------------------------------------------------------------------------
  // Helpers
  // ---------------------------------------------------------------------------

  function removeOverlay() {
    if (activeVaultDialogHandle) {
      const handle = activeVaultDialogHandle;
      activeVaultDialogHandle = null;
      handle.close();
    }
  }

  exports.vault = {
    init,
    ensureUnlocked,
    showSetupDialog,
    showUnlockDialog,
    showVaultDialog,
    showAccountForm,
    getAccounts,
  };
})(window);

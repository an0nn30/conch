(function initTermLabDialogService(global) {
  'use strict';

  function escHtml(text) {
    if (global.utils && typeof global.utils.esc === 'function') {
      return global.utils.esc(text);
    }
    const span = document.createElement('span');
    span.textContent = String(text == null ? '' : text);
    return span.innerHTML;
  }

  // Last remaining producer of the legacy .ssh-overlay markup; migrated onto
  // tlDialog.open() (design-system-phase-5b task 4). Preserves the exact
  // promise-resolves-to-accepted contract both callers
  // (command-palette-runtime.js's confirmPluginPermissionsForPalette,
  // features/settings/actions.js's confirmPluginPermissions) already depend
  // on: resolve(true) for Allow, resolve(false) for Deny/Escape/backdrop.
  function confirmPluginPermissions(pluginName, permissions) {
    const perms = Array.isArray(permissions) ? permissions.filter(Boolean) : [];
    return new Promise((resolve) => {
      if (!global.tlDialog || typeof global.tlDialog.open !== 'function') {
        resolve(false);
        return;
      }

      const items = perms
        .map((permission) => `<div class="plugin-permissions-item">• ${escHtml(permission)}</div>`)
        .join('');

      let done = false;
      let handle = null;
      const finish = (accepted) => {
        if (done) return;
        done = true;
        resolve(accepted);
        if (handle) handle.close(accepted ? 'allow' : 'deny');
      };

      handle = global.tlDialog.open({
        title: 'Plugin Permissions',
        ariaLabel: 'Plugin permissions',
        size: 'md',
        body: (bodyEl) => {
          bodyEl.innerHTML =
            `<div class="plugin-permissions-label">Plugin "${escHtml(pluginName)}" requests:</div>` +
            `<div class="plugin-permissions-list">${items}</div>` +
            `<div class="plugin-permissions-footnote">Allow and enable this plugin?</div>`;
        },
        buttons: [
          { label: 'Deny', onSelect: () => finish(false) },
          { label: 'Allow', primary: true, onSelect: () => finish(true) },
        ],
        onClose: () => finish(false),
      });
    });
  }

  // Three-way close prompt for a modified editor. Escape and the backdrop
  // resolve to 'cancel' — the safe answer, since losing an edit to a stray
  // keystroke is the failure this dialog exists to prevent. Same shape as
  // confirmPluginPermissions above: a `done` latch so the onClose that
  // handle.close() itself triggers cannot overwrite the chosen answer, and a
  // resolve('cancel') when tlDialog is unavailable so a missing dialog stack
  // aborts the close rather than silently consenting to it.
  function confirmSave(fileName) {
    return new Promise((resolve) => {
      if (!global.tlDialog || typeof global.tlDialog.open !== 'function') {
        resolve('cancel');
        return;
      }

      let done = false;
      let handle = null;
      const finish = (choice) => {
        if (done) return;
        done = true;
        resolve(choice);
        if (handle) handle.close(choice);
      };

      handle = global.tlDialog.open({
        title: 'Unsaved Changes',
        ariaLabel: 'Unsaved changes',
        size: 'sm',
        body: (bodyEl) => {
          bodyEl.innerHTML =
            `<div class="tl-dialog-message">Save changes to "${escHtml(fileName)}" before closing?</div>`;
        },
        buttons: [
          { label: 'Cancel', onSelect: () => finish('cancel') },
          { label: "Don't Save", onSelect: () => finish('discard') },
          { label: 'Save', primary: true, onSelect: () => finish('save') },
        ],
        onClose: () => finish('cancel'),
      });
    });
  }

  global.termlabDialogService = {
    confirmPluginPermissions,
    confirmSave,
  };
})(window);

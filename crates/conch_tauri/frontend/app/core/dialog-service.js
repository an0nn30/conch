(function initConchDialogService(global) {
  'use strict';

  const openStack = [];

  function escHtml(text) {
    if (global.utils && typeof global.utils.esc === 'function') {
      return global.utils.esc(text);
    }
    const span = document.createElement('span');
    span.textContent = String(text == null ? '' : text);
    return span.innerHTML;
  }

  function detachOverlay(entry) {
    if (!entry) return;
    if (entry.unregisterEscape) {
      try { entry.unregisterEscape(); } catch (_) {}
      entry.unregisterEscape = null;
    }
    const idx = openStack.indexOf(entry);
    if (idx >= 0) openStack.splice(idx, 1);
    if (entry.overlay && entry.overlay.parentNode) {
      entry.overlay.parentNode.removeChild(entry.overlay);
    }
  }

  function registerEscape(entry, onEscape) {
    const router = global.conchKeyboardRouter;
    if (router && typeof router.register === 'function') {
      return router.register({
        name: entry.id ? `dialog:${entry.id}` : 'dialog:overlay',
        priority: 220,
        isActive: () => openStack.length > 0 && openStack[openStack.length - 1] === entry,
        onKeyDown: (event) => {
          if (event.key !== 'Escape') return false;
          onEscape(event);
          return true;
        },
      });
    }
    console.warn('dialog-service: keyboard router unavailable, skipping escape registration for', entry.id || 'overlay');
    return () => {};
  }

  function open(options) {
    const opts = options || {};
    const overlay = document.createElement('div');
    overlay.className = opts.overlayClassName || 'ssh-overlay';
    if (opts.id) overlay.id = opts.id;

    if (opts.ariaLabel) {
      overlay.setAttribute('role', 'dialog');
      overlay.setAttribute('aria-modal', 'true');
      overlay.setAttribute('aria-label', String(opts.ariaLabel));
    }

    if (typeof opts.render === 'function') {
      opts.render(overlay);
    } else if (opts.html) {
      overlay.innerHTML = String(opts.html);
    }

    const entry = {
      id: opts.id || '',
      overlay,
      unregisterEscape: null,
    };

    const close = (reason) => {
      detachOverlay(entry);
      if (typeof opts.onClose === 'function') {
        opts.onClose(reason || 'close');
      }
    };

    entry.unregisterEscape = registerEscape(entry, (event) => {
      if (event && event.preventDefault) event.preventDefault();
      if (event && event.stopPropagation) event.stopPropagation();
      if (opts.closeOnEscape === false) return;
      close('escape');
    });

    if (opts.closeOnBackdrop !== false) {
      overlay.addEventListener('mousedown', (event) => {
        if (event.target === overlay) {
          close('backdrop');
        }
      });
    }

    openStack.push(entry);
    document.body.appendChild(overlay);

    if (opts.initialFocusSelector) {
      const focusEl = overlay.querySelector(opts.initialFocusSelector);
      if (focusEl && typeof focusEl.focus === 'function') {
        setTimeout(() => focusEl.focus(), 0);
      }
    }

    return {
      overlay,
      close,
    };
  }

  function confirmPluginPermissions(pluginName, permissions) {
    const perms = Array.isArray(permissions) ? permissions.filter(Boolean) : [];
    return new Promise((resolve) => {
      const items = perms
        .map((permission) => `<div class="plugin-permissions-item">• ${escHtml(permission)}</div>`)
        .join('');

      const modal = open({
        id: 'plugin-permissions-overlay',
        ariaLabel: 'Plugin permissions',
        closeOnBackdrop: true,
        closeOnEscape: true,
        html:
          `<div class="ssh-form plugin-permissions-dialog">` +
            `<div class="ssh-form-title">Plugin Permissions</div>` +
            `<div class="ssh-form-body">` +
              `<div class="plugin-permissions-label">Plugin "${escHtml(pluginName)}" requests:</div>` +
              `<div class="plugin-permissions-list">${items}</div>` +
              `<div class="plugin-permissions-footnote">Allow and enable this plugin?</div>` +
            `</div>` +
            `<div class="ssh-form-buttons">` +
              `<button class="ssh-form-btn" data-action="deny">Deny</button>` +
              `<button class="ssh-form-btn primary" data-action="allow">Allow</button>` +
            `</div>` +
          `</div>`,
      });

      const finish = (accepted) => {
        modal.close(accepted ? 'allow' : 'deny');
        resolve(accepted);
      };

      const deny = modal.overlay.querySelector('[data-action="deny"]');
      const allow = modal.overlay.querySelector('[data-action="allow"]');
      if (deny) deny.addEventListener('click', () => finish(false));
      if (allow) allow.addEventListener('click', () => finish(true));
    });
  }

  global.conchDialogService = {
    open,
    confirmPluginPermissions,
  };
})(window);

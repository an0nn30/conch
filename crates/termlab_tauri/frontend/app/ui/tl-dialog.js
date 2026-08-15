// Shared modal dialog shell (window.tlDialog) — renders the .tl-dialog*
// component (styles/design-system/components/dialog.css). Intended to
// replace the app's many ad-hoc overlay/panel dialogs (dialog-service.js,
// files-panel.js, plugin-widgets.js, ssh/auth-prompts.js, ssh/dialogs.js,
// tunnel-manager.js, vault/account-form.js) one at a time; nothing consumes
// this module yet. Follows app/ui/tl-menu.js's conventions (IIFE, single
// global, keyboard-router registration) but supports a *stack* of dialogs
// (menus are single-instance; dialogs can legitimately nest — e.g. a
// confirm dialog opened from a settings dialog).
//
// Stacking: each open() call gets `z-index = 3000 + depth*10` on its own
// overlay (depth = number of dialogs already open). Keep the base/step here
// in sync with --tl-z-dialog in styles/design-system/base.css.
//
// Focus: on open, the element that had focus is remembered and the first
// focusable element inside the panel is focused (or the panel itself,
// which carries tabindex="-1" as a fallback). A capture-phase keydown on
// the overlay traps Tab/Shift+Tab within the panel's focusable set. On
// close, focus is restored to the remembered element (mirrors tl-menu.js's
// lastActiveElement contract) provided it is still `isConnected`.
(function initTermLabDialog(global) {
  'use strict';

  const Z_BASE = 3000;
  const Z_STEP = 10;

  const FOCUSABLE_SELECTOR =
    'a[href], button, input, select, textarea, [tabindex]:not([tabindex="-1"])';

  // Open dialogs, bottom to top. stack[stack.length - 1] is the topmost
  // (active) dialog.
  const stack = [];

  // Elements forced aria-hidden while a dialog is open, with their prior
  // aria-hidden value so it can be restored exactly (see refreshAriaHidden).
  let hiddenSiblings = [];

  function zIndexForDepth(depth) {
    return Z_BASE + depth * Z_STEP;
  }

  function isElementVisible(el) {
    if (!el) return false;
    if (typeof el.getClientRects === 'function') {
      const rects = el.getClientRects();
      if (rects && typeof rects.length === 'number') return rects.length > 0;
    }
    if (typeof el.offsetWidth === 'number' && typeof el.offsetHeight === 'number') {
      return el.offsetWidth > 0 || el.offsetHeight > 0;
    }
    // Stubbed/non-layout environments (tests): treat as visible since no
    // layout information is available to say otherwise.
    return true;
  }

  // Standalone predicate — does not rely on FOCUSABLE_SELECTOR having
  // already filtered the candidate, so it is independently correct for
  // callers (and tests) that hand it arbitrary elements.
  function isFocusableCandidate(el) {
    if (!el || el.disabled) return false;
    const getAttr = typeof el.getAttribute === 'function' ? el.getAttribute.bind(el) : () => null;
    const tabIndexAttr = getAttr('tabindex');
    if (tabIndexAttr === '-1') return false;

    const tag = String(el.tagName || '').toUpperCase();
    const hasHref = tag === 'A' &&
      (typeof el.hasAttribute === 'function' ? el.hasAttribute('href') : getAttr('href') != null);
    const isStandardTag = hasHref || tag === 'BUTTON' || tag === 'INPUT' || tag === 'SELECT' || tag === 'TEXTAREA';
    const hasExplicitTabIndex = tabIndexAttr != null && tabIndexAttr !== '';
    if (!isStandardTag && !hasExplicitTabIndex) return false;

    return isElementVisible(el);
  }

  function getFocusableCandidates(panel) {
    if (!panel || typeof panel.querySelectorAll !== 'function') return [];
    return Array.prototype.filter.call(panel.querySelectorAll(FOCUSABLE_SELECTOR), isFocusableCandidate);
  }

  // Hides every direct child of document.body except the topmost dialog's
  // overlay, so assistive tech only ever sees the active dialog. Recomputed
  // from scratch on every push/pop so it stays correct regardless of close
  // order (closeTop(), or an entry's own close() called out of turn).
  function clearAriaHidden() {
    for (const hidden of hiddenSiblings) {
      if (hidden.prevValue === null) {
        if (typeof hidden.el.removeAttribute === 'function') hidden.el.removeAttribute('aria-hidden');
      } else if (typeof hidden.el.setAttribute === 'function') {
        hidden.el.setAttribute('aria-hidden', hidden.prevValue);
      }
    }
    hiddenSiblings = [];
  }

  function refreshAriaHidden() {
    clearAriaHidden();
    if (!stack.length || !global.document || !document.body) return;
    const topOverlay = stack[stack.length - 1].overlay;
    const children = Array.prototype.slice.call(document.body.children || []);
    for (const child of children) {
      if (child === topOverlay) continue;
      const prevValue = typeof child.getAttribute === 'function' ? child.getAttribute('aria-hidden') : null;
      hiddenSiblings.push({ el: child, prevValue });
      if (typeof child.setAttribute === 'function') child.setAttribute('aria-hidden', 'true');
    }
  }

  function registerEscape(entry, close) {
    const router = global.termlabKeyboardRouter;
    if (router && typeof router.register === 'function') {
      return router.register({
        name: 'tl-dialog',
        // Above dialog-service.js's escape priority (220 — see its
        // dialog:${id} registration at dialog-service.js:28-44) so tl-dialog
        // reliably wins Escape over the legacy dialogs it is replacing
        // during the migration.
        priority: 225,
        isActive: () => stack.length > 0 && stack[stack.length - 1] === entry,
        onKeyDown: (event) => {
          if (event.key !== 'Escape') return false;
          close('escape');
          return true;
        },
      });
    }
    console.warn('tl-dialog: keyboard router unavailable, Escape handler not registered');
    return null;
  }

  function buildFooterButton(spec) {
    const btn = document.createElement('button');
    btn.type = 'button';
    let className = 'tl-btn';
    if (spec.primary) className += ' tl-btn--primary';
    if (spec.danger) className += ' is-danger';
    btn.className = className;
    btn.textContent = spec.label;
    if (spec.disabled) {
      btn.disabled = true;
      btn.setAttribute('aria-disabled', 'true');
    }
    // Buttons never auto-close the dialog: onSelect decides (e.g. a "Save"
    // button may need to validate before calling handle.close()).
    btn.addEventListener('click', () => {
      if (!spec.disabled && typeof spec.onSelect === 'function') spec.onSelect();
    });
    return btn;
  }

  function open(options) {
    const opts = options || {};
    const depth = stack.length;

    const overlay = document.createElement('div');
    overlay.className = 'tl-dialog__overlay';
    overlay.style.zIndex = String(zIndexForDepth(depth));

    const sizeKey = opts.size === 'sm' || opts.size === 'lg' ? opts.size : 'md';
    const panel = document.createElement('div');
    panel.className = 'tl-dialog tl-dialog--' + sizeKey;
    panel.setAttribute('role', 'dialog');
    panel.setAttribute('aria-modal', 'true');
    panel.tabIndex = -1;
    const ariaLabel = opts.ariaLabel || opts.title;
    if (ariaLabel) panel.setAttribute('aria-label', String(ariaLabel));

    if (opts.title) {
      const titleEl = document.createElement('div');
      titleEl.className = 'tl-dialog__title';
      titleEl.textContent = opts.title;
      panel.appendChild(titleEl);
    }

    const bodyEl = document.createElement('div');
    bodyEl.className = 'tl-dialog__body';
    if (typeof opts.body === 'function') {
      opts.body(bodyEl);
    } else if (opts.body) {
      bodyEl.appendChild(opts.body);
    }
    panel.appendChild(bodyEl);

    const entry = { overlay, panel, depth, unregisterEscape: null, restoreFocusEl: null, close: null };

    function close(result) {
      const idx = stack.indexOf(entry);
      if (idx === -1) return; // already closed
      stack.splice(idx, 1);
      if (typeof entry.unregisterEscape === 'function') {
        try { entry.unregisterEscape(); } catch (_) {}
        entry.unregisterEscape = null;
      }
      if (overlay.parentNode) overlay.parentNode.removeChild(overlay);
      refreshAriaHidden();
      const restoreEl = entry.restoreFocusEl;
      if (restoreEl && restoreEl.isConnected && typeof restoreEl.focus === 'function') {
        restoreEl.focus();
      }
      if (typeof opts.onClose === 'function') opts.onClose(result);
    }
    entry.close = close;

    if (Array.isArray(opts.buttons) && opts.buttons.length) {
      const footer = document.createElement('div');
      footer.className = 'tl-dialog__footer';
      for (const spec of opts.buttons) {
        footer.appendChild(buildFooterButton(spec));
      }
      panel.appendChild(footer);
    }

    overlay.appendChild(panel);

    overlay.addEventListener('mousedown', (event) => {
      if (event.target === overlay) close('backdrop');
    });

    // Capture-phase so this always sees Tab/Shift+Tab before anything
    // inside the panel, even a child that itself stops propagation.
    overlay.addEventListener('keydown', (event) => {
      if (event.key !== 'Tab') return;
      const candidates = getFocusableCandidates(panel);
      if (!candidates.length) {
        event.preventDefault();
        return;
      }
      const first = candidates[0];
      const last = candidates[candidates.length - 1];
      const activeEl = document.activeElement;
      const activeIdx = candidates.indexOf(activeEl);
      if (event.shiftKey) {
        if (activeEl === first || activeIdx === -1) {
          event.preventDefault();
          last.focus();
        }
      } else if (activeEl === last || activeIdx === -1) {
        event.preventDefault();
        first.focus();
      }
    }, true);

    entry.restoreFocusEl = global.document ? document.activeElement : null;

    stack.push(entry);
    document.body.appendChild(overlay);
    refreshAriaHidden();

    entry.unregisterEscape = registerEscape(entry, close);

    const focusFirst = () => {
      if (!overlay.isConnected) return;
      const candidates = getFocusableCandidates(panel);
      const target = candidates[0] || panel;
      if (target && typeof target.focus === 'function') target.focus();
    };
    if (typeof global.requestAnimationFrame === 'function') {
      global.requestAnimationFrame(focusFirst);
    } else {
      focusFirst();
    }

    if (typeof opts.onOpen === 'function') opts.onOpen(panel);

    return { el: panel, close };
  }

  function closeTop(result) {
    if (!stack.length) return;
    stack[stack.length - 1].close(result);
  }

  function count() {
    return stack.length;
  }

  global.tlDialog = {
    open,
    closeTop,
    count,
    // Exposed for scripts/tests/test_tl_dialog.mjs — pure logic, no DOM
    // required (matches tl-icon.js's _setDarkVariants precedent).
    _zIndexForDepth: zIndexForDepth,
    _isFocusableCandidate: isFocusableCandidate,
    _getFocusableCandidates: getFocusableCandidates,
  };
})(window);

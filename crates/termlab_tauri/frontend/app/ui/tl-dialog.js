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
// overlay (depth = number of dialogs already open).
//
// Focus: on open, the element that had focus is remembered and the first
// focusable element inside the panel is focused (or the panel itself,
// which carries tabindex="-1" as a fallback). A capture-phase keydown on
// `document` (not the overlay — a tl-menu popup opened from inside the
// dialog, e.g. via tl-combo, is appended to document.body as a SIBLING of
// the overlay, not a descendant, so a listener scoped to the overlay never
// sees Tab events targeting it; see design-system-phase-5a's final review,
// task 1) traps Tab/Shift+Tab within the union of the panel's focusable set
// and any open tl-menu popup's items, scoped to whichever dialog is
// currently topmost. On close, focus is restored to the remembered element
// (mirrors tl-menu.js's lastActiveElement contract) provided it is still
// `isConnected`.
(function initTermLabDialog(global) {
  'use strict';

  const Z_BASE = 3000;
  const Z_STEP = 10;

  const FOCUSABLE_SELECTOR =
    'a[href], button, input, select, textarea, [tabindex]:not([tabindex="-1"])';

  // Open dialogs, bottom to top. stack[stack.length - 1] is the topmost
  // (active) dialog.
  const stack = [];

  // Listeners for "the dialog stack just went from non-empty to empty" —
  // e.g. dialog-runtime.js's terminal-refocus logic, which used to infer
  // this from a MutationObserver counting .ssh-overlay nodes. Fired once
  // per close() call that empties the stack (not per-dialog), after focus
  // restoration and the closed dialog's own onClose.
  const allClosedListeners = [];

  function onAllClosed(listener) {
    if (typeof listener !== 'function') return () => {};
    allClosedListeners.push(listener);
    return function unregister() {
      const idx = allClosedListeners.indexOf(listener);
      if (idx !== -1) allClosedListeners.splice(idx, 1);
    };
  }

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
        // during the migration. tl-menu.js in turn bumps ITS OWN priority
        // above this (see its escapePriority()) whenever a dialog is open,
        // so a tl-menu popup opened from inside this dialog (e.g. via
        // tl-combo) still gets first crack at Escape and closes without
        // taking the dialog down with it.
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
    //
    // Gate on btn.disabled (the live DOM property), not spec.disabled (the
    // options object passed at build time). A caller that toggles the
    // button's enabled state after creation — e.g. Settings' Apply button,
    // see wireApplyDirtyTracking() in features/settings/renderers.js — only
    // has the returned <button> element to mutate, so it sets
    // `button.disabled = ...` directly; spec.disabled never changes after
    // this closure runs once at build time. Reading spec.disabled here left
    // the click handler permanently gated on whatever value was true at
    // open() (Apply starts disabled), so the button would visually
    // re-enable but silently no-op on every click for the dialog's whole
    // lifetime. btn.disabled is the single source of truth for both the
    // visual state (the browser applies :disabled styling / suppresses
    // native activation from it) and this click gate.
    btn.addEventListener('click', () => {
      if (!btn.disabled && typeof spec.onSelect === 'function') spec.onSelect();
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

    const entry = {
      overlay, panel, depth,
      unregisterEscape: null,
      unregisterTabTrap: null,
      restoreFocusEl: null,
      close: null,
    };

    function close(result) {
      const idx = stack.indexOf(entry);
      if (idx === -1) return; // already closed
      stack.splice(idx, 1);
      if (typeof entry.unregisterEscape === 'function') {
        try { entry.unregisterEscape(); } catch (_) {}
        entry.unregisterEscape = null;
      }
      if (typeof entry.unregisterTabTrap === 'function') {
        try { entry.unregisterTabTrap(); } catch (_) {}
        entry.unregisterTabTrap = null;
      }
      if (overlay.parentNode) overlay.parentNode.removeChild(overlay);
      refreshAriaHidden();
      const restoreEl = entry.restoreFocusEl;
      if (restoreEl && restoreEl.isConnected && typeof restoreEl.focus === 'function') {
        restoreEl.focus();
      }
      if (typeof opts.onClose === 'function') opts.onClose(result);
      if (!stack.length) {
        for (const listener of allClosedListeners.slice()) {
          try { listener(); } catch (error) { console.warn('tl-dialog: onAllClosed listener failed:', error); }
        }
      }
    }
    entry.close = close;

    // opts.footerStart is a left-aligned button group (e.g. a "?" help
    // button) rendered before the standard right-aligned opts.buttons group;
    // see components/dialog.css's .tl-dialog__footer-start/-end. A footer is
    // built whenever either is present so a footerStart-only dialog still
    // gets one.
    const hasButtons = Array.isArray(opts.buttons) && opts.buttons.length;
    const hasFooterStart = Array.isArray(opts.footerStart) && opts.footerStart.length;
    if (hasButtons || hasFooterStart) {
      const footer = document.createElement('div');
      footer.className = 'tl-dialog__footer';

      const startEl = document.createElement('div');
      startEl.className = 'tl-dialog__footer-start';
      if (hasFooterStart) {
        for (const spec of opts.footerStart) {
          startEl.appendChild(buildFooterButton(spec));
        }
      }
      footer.appendChild(startEl);

      const endEl = document.createElement('div');
      endEl.className = 'tl-dialog__footer-end';
      if (hasButtons) {
        for (const spec of opts.buttons) {
          endEl.appendChild(buildFooterButton(spec));
        }
      }
      footer.appendChild(endEl);

      panel.appendChild(footer);
    }

    overlay.appendChild(panel);

    overlay.addEventListener('mousedown', (event) => {
      if (event.target === overlay) close('backdrop');
    });

    // Capture-phase on `document` (not `overlay`) so this also sees Tab
    // events whose target is inside an open tl-menu popup — the popup is
    // appended to document.body as a SIBLING of `overlay`, not a
    // descendant (app/ui/tl-menu.js open()), so a listener scoped to
    // `overlay` never receives those events, and Tab could walk focus
    // straight out of the dialog+popup into the rest of the page. Scoped to
    // "this dialog is topmost" (same check as registerEscape's isActive)
    // so only one dialog's trap is ever live at a time when dialogs nest.
    // An open tl-menu popup's items are folded into the focusable set on
    // the assumption (shared with tl-menu.js's own menuZIndex()/
    // escapePriority()) that any menu open while a dialog is topmost was
    // opened from within that dialog.
    function handleTabTrap(event) {
      if (event.key !== 'Tab') return;
      if (stack[stack.length - 1] !== entry) return;
      const menuEl = global.tlMenu && typeof global.tlMenu.activeElement === 'function'
        ? global.tlMenu.activeElement()
        : null;
      const candidates = getFocusableCandidates(panel).concat(
        menuEl ? getFocusableCandidates(menuEl) : []
      );
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
    }
    document.addEventListener('keydown', handleTabTrap, true);
    entry.unregisterTabTrap = () => document.removeEventListener('keydown', handleTabTrap, true);

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
    onAllClosed,
    // Exposed for scripts/tests/test_tl_dialog.mjs — pure logic, no DOM
    // required (matches tl-icon.js's _setDarkVariants precedent).
    _zIndexForDepth: zIndexForDepth,
    _isFocusableCandidate: isFocusableCandidate,
    _getFocusableCandidates: getFocusableCandidates,
  };
})(window);

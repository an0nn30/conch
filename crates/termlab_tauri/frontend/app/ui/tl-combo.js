// Combo-box control (window.tlCombo) — wraps an EXISTING native <select> so
// the select stays in the DOM as the source of truth: its `change` handlers
// and `.value` reads keep working untouched at every one of the app's 17
// call sites. attach(selectEl) hides the select (display: none, still in
// the DOM) and inserts a button.tl-combo showing the selected option's text
// plus a chevron; clicking it opens a window.tlMenu popup, one checkable
// item per <option>, anchored under the button.
//
// Options are re-read from the select every time the popup opens (not
// cached at attach() time) because several selects are repopulated at
// runtime by replacing their innerHTML — see populateAccountPicker in
// app/features/ssh/connection-form.js and the server pickers in
// app/panels/tunnel-manager.js. Selecting an item sets selectEl.value and
// dispatches a bubbling `change` event so existing handlers fire exactly as
// they would for a real <select> interaction.
//
// Nothing consumes this module yet (design-system-phase-5a tasks 3-4
// migrate the dialogs onto it); attach() is not called anywhere in the app
// today.
(function initTermLabCombo(global) {
  'use strict';

  function currentOption(selectEl) {
    return selectEl.options[selectEl.selectedIndex] || null;
  }

  function currentLabel(selectEl) {
    const opt = currentOption(selectEl);
    return opt ? opt.textContent : '';
  }

  function attach(selectEl) {
    if (!selectEl || selectEl.tagName !== 'SELECT') return null;
    // Idempotent: a second attach() on the same select returns the existing
    // button/api instead of inserting a duplicate.
    if (selectEl._tlCombo) return selectEl._tlCombo;

    selectEl.style.display = 'none';

    const button = document.createElement('button');
    button.type = 'button';
    button.className = 'tl-combo';
    // aria-haspopup must agree with what the popup actually is. tl-menu
    // always renders role="menu" with role="menuitem"/"menuitemcheckbox"
    // items (see app/ui/tl-menu.js buildItemEl/open) — that's the "Menu
    // Button" ARIA pattern, not a listbox popup — and this combo reuses
    // tl-menu unmodified rather than teaching it a second listbox/option
    // role vocabulary for one consumer. So the trigger advertises "menu" to
    // match, not "listbox".
    button.setAttribute('aria-haspopup', 'menu');
    button.setAttribute('aria-expanded', 'false');

    const labelEl = document.createElement('span');
    labelEl.className = 'tl-combo__label';
    button.appendChild(labelEl);

    if (global.tlIcon && typeof global.tlIcon.create === 'function') {
      button.appendChild(global.tlIcon.create('chevronDown', { size: 16, alt: '' }));
    }

    selectEl.insertAdjacentElement('afterend', button);

    function refresh() {
      labelEl.textContent = currentLabel(selectEl);
      button.disabled = !!selectEl.disabled;
      button.setAttribute('aria-disabled', button.disabled ? 'true' : 'false');
    }
    refresh();

    function openPopup() {
      if (selectEl.disabled) return;
      refresh();
      const rect = button.getBoundingClientRect();
      const items = Array.prototype.map.call(selectEl.options, (opt) => ({
        label: opt.textContent,
        checked: opt.index === selectEl.selectedIndex,
        disabled: opt.disabled,
        onSelect: () => {
          selectEl.value = opt.value;
          selectEl.dispatchEvent(new Event('change', { bubbles: true }));
          refresh();
        },
      }));

      button.setAttribute('aria-expanded', 'true');
      // No routerPriority passed: tl-menu.js's open() already computes an
      // Escape priority above window.tlDialog's whenever a dialog is open
      // (see its escapePriority()), which covers every combo opened from
      // inside a tl-dialog (keygen, tunnel-manager, plugin-widgets,
      // connection-form, account-form) without this call site needing to
      // know about tl-dialog at all.
      const menu = global.tlMenu.open({
        x: rect.left,
        y: rect.bottom + 2,
        items,
        ariaLabel: button.getAttribute('aria-label') || selectEl.getAttribute('aria-label') || 'Select an option',
      });

      // tlMenu has no onClose hook (it's single-instance and closes itself
      // on Escape/outside-click/selection); watch for this specific popup
      // leaving the DOM so aria-expanded flips back once it's actually
      // gone, without adding a callback to the shared component for one
      // consumer.
      if (menu && global.MutationObserver) {
        const bodyObserver = new global.MutationObserver(() => {
          if (!menu.isConnected) {
            button.setAttribute('aria-expanded', 'false');
            bodyObserver.disconnect();
          }
        });
        bodyObserver.observe(document.body, { childList: true });
      }
    }

    button.addEventListener('click', openPopup);
    // Match native <select> keyboard behavior: Up/Down opens the popup when
    // focus is on the closed button (once open, tl-menu's own arrow-key
    // handling takes over).
    button.addEventListener('keydown', (event) => {
      if (event.key !== 'ArrowDown' && event.key !== 'ArrowUp') return;
      event.preventDefault();
      openPopup();
    });

    // Keep the button's disabled state (and, best-effort, its label) in
    // sync with the select for changes that happen after attach() — e.g.
    // code that toggles `select.disabled` directly, or repopulates options
    // via innerHTML. `change` covers user-driven and our own dispatched
    // selections; the observer covers everything else.
    selectEl.addEventListener('change', refresh);
    if (global.MutationObserver) {
      const selectObserver = new global.MutationObserver(refresh);
      selectObserver.observe(selectEl, { attributes: true, attributeFilter: ['disabled'], childList: true, subtree: true });
    }

    const api = { button, refresh };
    selectEl._tlCombo = api;
    return api;
  }

  global.tlCombo = { attach };
})(window);

// Shared popup menu (window.tlMenu) — renders the .tl-menu* component
// (styles/design-system/components/menu.css) used by every popup/context
// menu in the app. Single-instance: opening a menu closes any other tlMenu
// popup that's still open. Positioning, outside-click dismissal, and Escape
// handling (via window.termlabKeyboardRouter when available) mirror the
// pattern established by the original app/features/files/pane-view.js
// showRowContextMenu and app/features/ssh/context-menu.js showContextMenu
// (both since folded into this shared component; context-menu.js itself
// was deleted once its last consumer, tunnels-panel.js, was converted).
//
// Item shape: {label, icon?, checked?, disabled?, danger?, title?, onSelect}
// or {separator: true}. `checked` (boolean, when present) renders a
// checkable item (role="menuitemcheckbox") with a check glyph in the icon
// gutter when true — used by the SFTP column-chooser menu and the
// tool-window "Move to" menu's current-zone indicator. There is no vendored
// check icon in vendor/intellij-icons, so the glyph is a plain "✓" character
// styled with tokens (currentColor), not an image.
(function initTermLabMenu(global) {
  'use strict';

  let activeMenu = null;
  let activeOutsideClickHandler = null;
  let activeUnregisterEscape = null;
  // Element focused right before the currently-open menu grabbed focus
  // (open() auto-focuses its first enabled item). Restored on close() so
  // Escape/outside-click/item-selection don't strand focus on a node that's
  // about to be removed from the document — see close() for the ordering
  // contract with an item's onSelect.
  let lastActiveElement = null;

  function close() {
    if (activeOutsideClickHandler) {
      document.removeEventListener('click', activeOutsideClickHandler);
      activeOutsideClickHandler = null;
    }
    if (typeof activeUnregisterEscape === 'function') {
      activeUnregisterEscape();
      activeUnregisterEscape = null;
    }
    if (activeMenu) {
      const current = document.activeElement;
      // Only restore focus if nothing else has already deliberately claimed
      // it. If the dismissal itself moved focus elsewhere (outside click on
      // another focusable control, xterm re-focusing itself on click), the
      // active element right before removal will be neither body nor inside
      // this menu — leave that alone rather than fighting it. Otherwise
      // (focus was still on a menu item, or nothing much is focused) restore
      // it. Item selection (buildItemEl.activate) calls close() BEFORE
      // invoking onSelect(), so this restore always happens first and an
      // onSelect that moves focus itself (e.g. opening a dialog) still wins.
      const stolen = current && current !== document.body && !activeMenu.contains(current);
      activeMenu.remove();
      activeMenu = null;
      if (!stolen && lastActiveElement && lastActiveElement.isConnected && typeof lastActiveElement.focus === 'function') {
        lastActiveElement.focus();
      }
    }
    lastActiveElement = null;
  }

  function buildItemEl(item) {
    const isCheckable = typeof item.checked === 'boolean';
    const el = document.createElement('div');
    el.className = 'tl-menu__item' + (item.disabled ? ' is-disabled' : '') + (item.danger ? ' is-danger' : '');
    el.setAttribute('role', isCheckable ? 'menuitemcheckbox' : 'menuitem');
    if (isCheckable) el.setAttribute('aria-checked', item.checked ? 'true' : 'false');
    if (item.disabled) el.setAttribute('aria-disabled', 'true');
    if (item.title) el.title = item.title;
    el.tabIndex = item.disabled ? -1 : 0;

    const iconWrap = document.createElement('span');
    iconWrap.className = 'tl-menu__icon';
    if (isCheckable) {
      if (item.checked) {
        iconWrap.classList.add('tl-menu__icon--check');
        iconWrap.textContent = '✓';
      }
    } else if (item.icon && global.tlIcon && typeof global.tlIcon.create === 'function') {
      iconWrap.appendChild(global.tlIcon.create(item.icon, { size: 16, alt: '' }));
    }
    el.appendChild(iconWrap);

    const label = document.createElement('span');
    label.className = 'tl-menu__label';
    label.textContent = item.label;
    el.appendChild(label);

    if (!item.disabled) {
      const activate = () => {
        close();
        if (typeof item.onSelect === 'function') item.onSelect();
      };
      el.addEventListener('click', activate);
      el.addEventListener('keydown', (keyEvent) => {
        if (keyEvent.key !== 'Enter' && keyEvent.key !== ' ') return;
        keyEvent.preventDefault();
        activate();
      });
    }

    return el;
  }

  function open(opts) {
    const o = opts || {};
    close(); // single-instance: opening one closes any other tlMenu popup (restoring its focus first)

    // Captured after the close() above so a menu opened from within another
    // menu still anchors to the original pre-menu element, not the just-closed
    // menu's item.
    lastActiveElement = document.activeElement;

    const menu = document.createElement('div');
    menu.className = 'tl-menu';
    menu.setAttribute('role', 'menu');
    menu.setAttribute('aria-label', o.ariaLabel || 'Menu');
    menu.style.left = o.x + 'px';
    menu.style.top = o.y + 'px';

    for (const item of (Array.isArray(o.items) ? o.items : [])) {
      if (item.separator) {
        const sep = document.createElement('div');
        sep.className = 'tl-menu__separator';
        menu.appendChild(sep);
        continue;
      }
      menu.appendChild(buildItemEl(item));
    }

    document.body.appendChild(menu);
    activeMenu = menu;

    requestAnimationFrame(() => {
      if (!menu.isConnected) return;
      const rect = menu.getBoundingClientRect();
      if (rect.right > window.innerWidth) menu.style.left = Math.max(4, window.innerWidth - rect.width - 4) + 'px';
      if (rect.bottom > window.innerHeight) menu.style.top = Math.max(4, window.innerHeight - rect.height - 4) + 'px';
      const firstFocusable = menu.querySelector('.tl-menu__item:not(.is-disabled)');
      if (firstFocusable && typeof firstFocusable.focus === 'function') firstFocusable.focus();
    });

    activeOutsideClickHandler = () => close();
    setTimeout(() => document.addEventListener('click', activeOutsideClickHandler, { once: true }), 0);

    const keyboardRouter = global.termlabKeyboardRouter;
    if (keyboardRouter && typeof keyboardRouter.register === 'function') {
      activeUnregisterEscape = keyboardRouter.register({
        name: o.routerName || 'tl-menu',
        priority: typeof o.routerPriority === 'number' ? o.routerPriority : 220,
        isActive: () => menu.isConnected,
        onKeyDown: (keyEvent) => {
          if (!menu.isConnected) return false;
          if (keyEvent.key !== 'Escape') return false;
          close();
          return true;
        },
      });
    } else {
      console.warn('tl-menu: keyboard router unavailable, Escape handler not registered');
    }

    if (typeof o.onOpen === 'function') o.onOpen(menu);
    return menu;
  }

  global.tlMenu = { open, close };
})(window);

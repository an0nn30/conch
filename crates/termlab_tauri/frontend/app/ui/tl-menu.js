// Shared popup menu (window.tlMenu) — renders the .tl-menu* component
// (styles/design-system/components/menu.css) used by every context/dropdown
// menu in the app that has adopted the shared look. Single-instance: opening
// a menu closes any other tlMenu popup that's still open. Positioning,
// outside-click dismissal, and Escape handling (via window.termlabKeyboardRouter
// when available) mirror the pattern established by
// app/features/files/pane-view.js showRowContextMenu and
// app/features/ssh/context-menu.js showContextMenu.
(function initTermLabMenu(global) {
  'use strict';

  let activeMenu = null;
  let activeOutsideClickHandler = null;
  let activeUnregisterEscape = null;

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
      activeMenu.remove();
      activeMenu = null;
    }
  }

  function buildItemEl(item) {
    const el = document.createElement('div');
    el.className = 'tl-menu__item' + (item.disabled ? ' is-disabled' : '') + (item.danger ? ' is-danger' : '');
    el.setAttribute('role', 'menuitem');
    if (item.disabled) el.setAttribute('aria-disabled', 'true');
    if (item.title) el.title = item.title;
    el.tabIndex = item.disabled ? -1 : 0;

    const iconWrap = document.createElement('span');
    iconWrap.className = 'tl-menu__icon';
    if (item.icon && global.tlIcon && typeof global.tlIcon.create === 'function') {
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
    close(); // single-instance: opening one closes any other tlMenu popup

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

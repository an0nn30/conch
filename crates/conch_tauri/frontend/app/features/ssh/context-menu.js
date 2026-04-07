(function initConchSshContextMenu(global) {
  'use strict';

  function removeContextMenu() {
    document.querySelectorAll('.ssh-context-menu').forEach((el) => el.remove());
  }

  function showContextMenu(event, items, deps) {
    const d = deps || {};
    removeContextMenu();

    const menu = document.createElement('div');
    menu.className = 'ssh-context-menu';
    menu.setAttribute('role', 'menu');
    menu.setAttribute('aria-label', 'SSH context menu');
    menu.style.left = event.clientX + 'px';
    menu.style.top = event.clientY + 'px';

    for (const item of (Array.isArray(items) ? items : [])) {
      if (item.type === 'separator') {
        const sep = document.createElement('div');
        sep.className = 'ssh-context-menu-sep';
        menu.appendChild(sep);
        continue;
      }
      const el = document.createElement('div');
      el.className = 'ssh-context-menu-item' + (item.danger ? ' danger' : '');
      el.textContent = item.label;
      el.setAttribute('role', 'menuitem');
      el.tabIndex = 0;
      const activate = () => {
        removeContextMenu();
        if (typeof item.action === 'function') item.action();
      };
      el.addEventListener('click', activate);
      el.addEventListener('keydown', (keyEvent) => {
        if (keyEvent.key !== 'Enter' && keyEvent.key !== ' ') return;
        keyEvent.preventDefault();
        activate();
      });
      menu.appendChild(el);
    }

    document.body.appendChild(menu);
    requestAnimationFrame(() => {
      const rect = menu.getBoundingClientRect();
      if (rect.right > window.innerWidth) menu.style.left = (window.innerWidth - rect.width - 4) + 'px';
      if (rect.bottom > window.innerHeight) menu.style.top = (window.innerHeight - rect.height - 4) + 'px';
    });

    setTimeout(() => {
      document.addEventListener('click', removeContextMenu, { once: true });
    }, 0);

    if (typeof d.onOpen === 'function') d.onOpen(menu);
    return menu;
  }

  global.conchSshContextMenu = {
    showContextMenu,
    removeContextMenu,
  };
})(window);

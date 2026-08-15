(function initTermLabFilesPaneView(global) {
  'use strict';

  function renderPane(pane, el, deps) {
    if (!el || !pane) return;
    const d = deps || {};

    const isRemote = !pane.isLocal;
    const noSession = isRemote && !d.activeRemotePaneId;

    const esc = typeof d.esc === 'function' ? d.esc : (value) => String(value == null ? '' : value);
    const attr = typeof d.attr === 'function' ? d.attr : esc;
    const extOf = typeof d.extOf === 'function' ? d.extOf : () => '';
    const formatSize = typeof d.formatSize === 'function' ? d.formatSize : (value) => String(value || 0);
    const formatDate = typeof d.formatDate === 'function' ? d.formatDate : (value) => String(value || '');
    const fileIcons = d.fileIcons && typeof d.fileIcons.iconFor === 'function' ? d.fileIcons : null;
    const sortArrow = typeof d.sortArrow === 'function' ? d.sortArrow : () => '';

    const visibleEntries = (Array.isArray(pane.entries) ? pane.entries : [])
      .filter((entry) => pane.showHidden || !String(entry.name || '').startsWith('.'));
    const hiddenCount = (pane.entries || []).length - visibleEntries.length;
    const footerText = noSession
      ? 'Not connected'
      : (hiddenCount > 0
        ? `${visibleEntries.length} items (${hiddenCount} hidden)`
        : `${visibleEntries.length} items`);

    el.innerHTML = `
      <div class="fp-toolbar">
        <button class="fp-tb-btn" data-action="back" ${pane.backStack.length === 0 ? 'disabled' : ''} title="Back">${d.iconBack || ''}</button>
        <button class="fp-tb-btn" data-action="forward" ${pane.forwardStack.length === 0 ? 'disabled' : ''} title="Forward">${d.iconForward || ''}</button>
        <input class="fp-path-input" type="text" value="${attr(pane.pathInput)}" spellcheck="false" ${noSession ? 'disabled' : ''} />
        <button class="fp-tb-btn" data-action="home" title="Home" ${noSession ? 'disabled' : ''}>${d.iconHome || ''}</button>
        <button class="fp-tb-btn" data-action="refresh" title="Refresh" ${noSession ? 'disabled' : ''}>${d.iconRefresh || ''}</button>
        <button class="fp-tb-btn ${pane.showHidden ? 'active' : ''}" data-action="hidden" title="${pane.showHidden ? 'Hide hidden files' : 'Show hidden files'}">.*</button>
      </div>
      ${pane.error ? `<div class="fp-error">${esc(pane.error)}</div>` : ''}
      <div class="fp-table-wrap">
        <table class="fp-table">
          <thead><tr>
            <th class="fp-th-name" data-col="name">Name ${sortArrow(pane, 'name')}</th>
            ${pane.colExt ? `<th class="fp-th-ext" data-col="ext">Ext ${sortArrow(pane, 'ext')}</th>` : ''}
            ${pane.colSize ? `<th class="fp-th-size" data-col="size">Size ${sortArrow(pane, 'size')}</th>` : ''}
            ${pane.colModified ? `<th class="fp-th-mod" data-col="modified">Modified ${sortArrow(pane, 'modified')}</th>` : ''}
          </tr></thead>
          <tbody></tbody>
        </table>
      </div>
      <div class="fp-footer">${esc(footerText)}</div>
    `;

    const tbody = el.querySelector('tbody');
    for (const entry of visibleEntries) {
      const tr = document.createElement('tr');
      tr.className = 'fp-row';
      tr.tabIndex = 0;
      tr.setAttribute('aria-label', entry.is_dir ? `Folder ${entry.name}` : `File ${entry.name}`);
      const ts = pane.transferStatus && pane.transferStatus[entry.name];
      if (ts) {
        if (ts.status === 'completed') tr.classList.add('fp-transferred');
        else if (ts.status === 'in_progress') tr.classList.add('fp-transferring');
      }
      tr.dataset.name = entry.name;

      const icon = fileIcons ? fileIcons.iconFor(entry.name, entry.is_dir, !pane.isLocal) : '';
      let cells = `<td class="fp-cell-name">${icon} <span>${esc(entry.name)}</span>`;
      if (ts && ts.status === 'in_progress') {
        cells += `<span class="fp-transfer-pct">${ts.percent || 0}%</span>`;
      }
      cells += '</td>';
      if (pane.colExt) cells += `<td class="fp-cell-ext">${esc(extOf(entry.name))}</td>`;
      if (pane.colSize) cells += `<td class="fp-cell-size">${entry.is_dir ? '' : formatSize(entry.size)}</td>`;
      if (pane.colModified) cells += `<td class="fp-cell-mod">${entry.modified ? formatDate(entry.modified) : ''}</td>`;
      tr.innerHTML = cells;

      tr.addEventListener('dblclick', () => {
        if (typeof d.onActivateEntry === 'function') d.onActivateEntry(entry);
      });
      tr.addEventListener('click', () => {
        el.querySelectorAll('.fp-row.selected').forEach((row) => row.classList.remove('selected'));
        tr.classList.add('selected');
        if (typeof d.onSelectEntry === 'function') d.onSelectEntry(entry.name);
      });
      tr.addEventListener('contextmenu', (event) => {
        event.preventDefault();
        el.querySelectorAll('.fp-row.selected').forEach((row) => row.classList.remove('selected'));
        tr.classList.add('selected');
        if (typeof d.onSelectEntry === 'function') d.onSelectEntry(entry.name);
        if (typeof d.onOpenRowMenu === 'function') d.onOpenRowMenu(event, entry);
      });
      tr.addEventListener('keydown', (event) => {
        if (event.key === 'Enter') {
          event.preventDefault();
          if (typeof d.onActivateEntry === 'function') d.onActivateEntry(entry);
          return;
        }
        if (event.key === ' ') {
          event.preventDefault();
          el.querySelectorAll('.fp-row.selected').forEach((row) => row.classList.remove('selected'));
          tr.classList.add('selected');
          if (typeof d.onSelectEntry === 'function') d.onSelectEntry(entry.name);
        }
      });
      tbody.appendChild(tr);
    }

    el.querySelectorAll('.fp-tb-btn').forEach((btn) => {
      btn.addEventListener('click', () => {
        const action = btn.dataset.action;
        if (action === 'back' && typeof d.onBack === 'function') d.onBack();
        else if (action === 'forward' && typeof d.onForward === 'function') d.onForward();
        else if (action === 'home' && typeof d.onHome === 'function') d.onHome();
        else if (action === 'refresh' && typeof d.onRefresh === 'function') d.onRefresh();
        else if (action === 'hidden' && typeof d.onToggleHidden === 'function') d.onToggleHidden();
      });
    });

    const pathInput = el.querySelector('.fp-path-input');
    if (pathInput) {
      pathInput.addEventListener('keydown', (event) => {
        if (event.key !== 'Enter') return;
        const value = pathInput.value.trim();
        if (!value) return;
        if (typeof d.onNavigate === 'function') d.onNavigate(value);
      });
    }

    el.querySelectorAll('th[data-col]').forEach((th) => {
      th.style.cursor = 'pointer';
      th.tabIndex = 0;
      th.setAttribute('role', 'button');
      th.addEventListener('click', () => {
        if (typeof d.onSort !== 'function') return;
        d.onSort(th.dataset.col);
      });
      th.addEventListener('keydown', (event) => {
        if (event.key !== 'Enter' && event.key !== ' ') return;
        event.preventDefault();
        if (typeof d.onSort !== 'function') return;
        d.onSort(th.dataset.col);
      });
      th.addEventListener('contextmenu', (event) => {
        event.preventDefault();
        if (typeof d.onOpenColumnMenu === 'function') {
          d.onOpenColumnMenu(event);
        }
      });
    });
  }

  function showColumnMenu(event, pane, deps) {
    const d = deps || {};
    document.querySelectorAll('.fp-col-menu').forEach((menu) => menu.remove());

    const menu = document.createElement('div');
    menu.className = 'fp-col-menu';
    menu.setAttribute('role', 'menu');
    menu.setAttribute('aria-label', 'File columns');
    menu.style.left = event.clientX + 'px';
    menu.style.top = event.clientY + 'px';

    const cols = [
      { key: 'colExt', label: 'Extension' },
      { key: 'colSize', label: 'Size' },
      { key: 'colModified', label: 'Modified' },
    ];

    for (const col of cols) {
      const item = document.createElement('div');
      item.className = 'fp-col-menu-item';
      item.innerHTML = `<span class="fp-col-check">${pane[col.key] ? '✓' : ''}</span> ${col.label}`;
      item.setAttribute('role', 'menuitemcheckbox');
      item.setAttribute('aria-checked', pane[col.key] ? 'true' : 'false');
      item.tabIndex = 0;
      const toggle = () => {
        if (typeof d.onToggleColumn === 'function') d.onToggleColumn(col.key);
        menu.remove();
      };
      item.addEventListener('click', toggle);
      item.addEventListener('keydown', (keyEvent) => {
        if (keyEvent.key !== 'Enter' && keyEvent.key !== ' ') return;
        keyEvent.preventDefault();
        toggle();
      });
      menu.appendChild(item);
    }

    document.body.appendChild(menu);
    setTimeout(() => document.addEventListener('click', () => menu.remove(), { once: true }), 0);
  }

  // ---------------------------------------------------------------------------
  // Row context menu (right-click on a file/dir row) — pattern mirrors
  // showColumnMenu above (fixed-position popup, viewport-clamped, dismissed on
  // outside click) plus icon gutter, disabled items, and separators, matching
  // the ssh-context-menu / tunnel row-menu popups elsewhere in the app. Items
  // are pre-built by the caller (files-panel.js), which owns the business
  // logic (invoke, session state); this module only knows how to render them.
  let unregisterRowMenuEscape = null;

  function removeRowContextMenu() {
    document.querySelectorAll('.fp-context-menu').forEach((menu) => menu.remove());
    if (typeof unregisterRowMenuEscape === 'function') {
      unregisterRowMenuEscape();
      unregisterRowMenuEscape = null;
    }
  }

  function showRowContextMenu(event, items) {
    removeRowContextMenu();

    const menu = document.createElement('div');
    menu.className = 'fp-context-menu';
    menu.setAttribute('role', 'menu');
    menu.setAttribute('aria-label', 'File actions');
    menu.style.left = event.clientX + 'px';
    menu.style.top = event.clientY + 'px';

    for (const item of (Array.isArray(items) ? items : [])) {
      if (item.type === 'separator') {
        const sep = document.createElement('div');
        sep.className = 'fp-context-menu-sep';
        menu.appendChild(sep);
        continue;
      }

      const el = document.createElement('div');
      el.className = 'fp-context-menu-item' + (item.disabled ? ' disabled' : '');
      el.setAttribute('role', 'menuitem');
      if (item.disabled) el.setAttribute('aria-disabled', 'true');
      if (item.title) el.title = item.title;
      el.tabIndex = item.disabled ? -1 : 0;

      const iconWrap = document.createElement('span');
      iconWrap.className = 'fp-context-menu-icon';
      if (item.icon && global.tlIcon && typeof global.tlIcon.create === 'function') {
        iconWrap.appendChild(global.tlIcon.create(item.icon, { size: 16, alt: '' }));
      }
      el.appendChild(iconWrap);

      const label = document.createElement('span');
      label.className = 'fp-context-menu-label';
      label.textContent = item.label;
      el.appendChild(label);

      if (!item.disabled) {
        const activate = () => {
          removeRowContextMenu();
          if (typeof item.action === 'function') item.action();
        };
        el.addEventListener('click', activate);
        el.addEventListener('keydown', (keyEvent) => {
          if (keyEvent.key !== 'Enter' && keyEvent.key !== ' ') return;
          keyEvent.preventDefault();
          activate();
        });
      }

      menu.appendChild(el);
    }

    document.body.appendChild(menu);
    requestAnimationFrame(() => {
      const rect = menu.getBoundingClientRect();
      if (rect.right > window.innerWidth) menu.style.left = Math.max(4, window.innerWidth - rect.width - 4) + 'px';
      if (rect.bottom > window.innerHeight) menu.style.top = Math.max(4, window.innerHeight - rect.height - 4) + 'px';
    });

    setTimeout(() => document.addEventListener('click', removeRowContextMenu, { once: true }), 0);

    const keyboardRouter = global.termlabKeyboardRouter;
    if (keyboardRouter && typeof keyboardRouter.register === 'function') {
      unregisterRowMenuEscape = keyboardRouter.register({
        name: 'fp-row-context-menu',
        priority: 220,
        isActive: () => menu.isConnected,
        onKeyDown: (keyEvent) => {
          if (!menu.isConnected) return false;
          if (keyEvent.key !== 'Escape') return false;
          removeRowContextMenu();
          return true;
        },
      });
    } else {
      console.warn('files-pane-view: keyboard router unavailable, row context menu Escape handler not registered');
    }

    return menu;
  }

  global.termlabFilesPaneView = {
    renderPane,
    showColumnMenu,
    showRowContextMenu,
  };
})(window);

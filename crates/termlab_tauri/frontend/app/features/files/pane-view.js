(function initTermLabFilesPaneView(global) {
  'use strict';

  function transferDirection(status) {
    return status && status.direction === 'download' ? 'download' : 'upload';
  }

  function transferPercentText(percent) {
    const value = Number(percent) || 0;
    if (value > 0 && value < 1) return '&lt;1%';
    return `${Math.max(0, Math.min(100, Math.round(value)))}%`;
  }

  function transferBadgeHtml(status) {
    if (!status) return '';
    if (status.status === 'attention') {
      if (status.transferId) {
        const transferId = String(status.transferId)
          .replace(/&/g, '&amp;')
          .replace(/"/g, '&quot;')
          .replace(/</g, '&lt;')
          .replace(/>/g, '&gt;');
        return `<button type="button" class="fp-transfer-attention" data-transfer-id="${transferId}" aria-label="Resolve transfer issue">Needs attention</button>`;
      }
      return '<span class="fp-transfer-attention" role="status">Needs attention</span>';
    }
    if (status.status === 'preparing' || status.status === 'starting') {
      const phase = status.status === 'preparing' ? 'Preparing' : 'Starting';
      return `<span class="fp-transfer-phase" role="status">${phase} ${transferDirection(status)}…</span>`;
    }
    if (status.status === 'waiting') {
      return `<span class="fp-transfer-waiting" role="status">Waiting to retry ${transferDirection(status)}…</span>`;
    }
    if (status.status === 'in_progress') {
      return `<span class="fp-transfer-pct">${transferPercentText(status.percent)}</span>`;
    }
    return '';
  }

  function transferActivityText(transferStatus) {
    const active = Object.entries(transferStatus || {}).filter(([, status]) => (
      status
      && ['preparing', 'starting', 'in_progress'].includes(status.status)
    ));
    if (active.length === 0) return '';
    if (active.length > 1) return `${active.length} transfers active`;

    const [fileName, status] = active[0];
    const direction = transferDirection(status);
    if (status.status === 'preparing') return `Preparing ${direction}: ${fileName}`;
    if (status.status === 'starting') return `Starting ${direction}: ${fileName}`;
    const action = direction === 'download' ? 'Downloading' : 'Uploading';
    return `${action} ${fileName}: ${transferPercentText(status.percent).replace('&lt;', '<')}`;
  }

  function activateTransferBadge(status, invoker, deps) {
    if (!status || status.status !== 'attention' || !status.transferId) return;
    if (deps && typeof deps.onTransferAttention === 'function') {
      deps.onTransferAttention(status.transferId, invoker);
    }
  }

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
    const activityText = transferActivityText(pane.transferStatus);

    el.innerHTML = `
      <div class="fp-toolbar">
        ${isRemote ? '<span class="fp-tb-group fp-tb-conn"><span class="fp-host-combo-slot"></span></span>' : ''}
        <span class="fp-tb-group fp-tb-nav">
          <button class="fp-tb-btn" data-action="back" ${pane.backStack.length === 0 ? 'disabled' : ''} title="Back">${d.iconBack || ''}</button>
          <button class="fp-tb-btn" data-action="forward" ${pane.forwardStack.length === 0 ? 'disabled' : ''} title="Forward">${d.iconForward || ''}</button>
          <input class="fp-path-input" type="text" value="${attr(pane.pathInput)}" spellcheck="false" ${noSession ? 'disabled' : ''} />
          <button class="fp-tb-btn" data-action="home" title="Home" ${noSession ? 'disabled' : ''}>${d.iconHome || ''}</button>
          <button class="fp-tb-btn" data-action="refresh" title="Refresh" ${noSession ? 'disabled' : ''}>${d.iconRefresh || ''}</button>
          <button class="fp-tb-btn ${pane.showHidden ? 'active' : ''}" data-action="hidden" title="${pane.showHidden ? 'Hide hidden files' : 'Show hidden files'}">${d.iconHidden || '.*'}</button>
        </span>
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
      <div class="fp-footer">
        <span>${esc(footerText)}</span>
        ${activityText ? `<span class="fp-footer__activity" role="status" aria-live="polite">${esc(activityText)}</span>` : ''}
      </div>
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
        else if (['preparing', 'starting', 'in_progress'].includes(ts.status)) tr.classList.add('fp-transferring');
      }
      tr.dataset.name = entry.name;

      const icon = fileIcons ? fileIcons.iconFor(entry.name, entry.is_dir, !pane.isLocal) : '';
      let cells = `<td class="fp-cell-name">${icon} <span>${esc(entry.name)}</span>`;
      cells += transferBadgeHtml(ts);
      cells += '</td>';
      if (pane.colExt) cells += `<td class="fp-cell-ext">${esc(extOf(entry.name))}</td>`;
      if (pane.colSize) cells += `<td class="fp-cell-size">${entry.is_dir ? '' : formatSize(entry.size)}</td>`;
      if (pane.colModified) cells += `<td class="fp-cell-mod">${entry.modified ? formatDate(entry.modified) : ''}</td>`;
      tr.innerHTML = cells;

      const attentionControl = tr.querySelector('.fp-transfer-attention[data-transfer-id]');
      if (attentionControl) {
        attentionControl.addEventListener('click', (event) => {
          event.preventDefault();
          event.stopPropagation();
          activateTransferBadge(ts, attentionControl, d);
        });
        attentionControl.addEventListener('dblclick', (event) => {
          event.preventDefault();
          event.stopPropagation();
        });
      }

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
        if (event.target !== tr) return;
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

    // Host combo (remote pane only) — built with the DOM API rather than
    // interpolated into the innerHTML template above, because the
    // <select>'s live identity matters: tlCombo.attach() below must run
    // against THIS render's element, and el.innerHTML just wiped whatever
    // combo the previous render attached. Every renderPane call is a fresh
    // trap for a one-time attach() — see files-panel.js's onComboMount,
    // which is invoked here, every time, for exactly this reason.
    if (isRemote) {
      const slot = el.querySelector('.fp-host-combo-slot');
      if (slot) {
        const select = document.createElement('select');
        select.className = 'fp-host-select';
        (Array.isArray(d.hostOptions) ? d.hostOptions : []).forEach((opt) => {
          const optionEl = document.createElement('option');
          optionEl.value = opt.value;
          optionEl.textContent = opt.label;
          if (opt.disabled) optionEl.disabled = true;
          select.appendChild(optionEl);
        });
        select.value = d.hostComboValue || '';
        select.disabled = !!d.hostComboBusy;
        select.addEventListener('change', () => {
          if (typeof d.onHostComboChange === 'function') d.onHostComboChange(select.value);
        });
        slot.appendChild(select);

        if (d.showDisconnect) {
          const eject = document.createElement('button');
          eject.type = 'button';
          eject.className = 'fp-tb-btn fp-host-disconnect';
          eject.title = 'Disconnect';
          eject.textContent = '⏏';
          eject.addEventListener('click', () => {
            if (typeof d.onDisconnect === 'function') d.onDisconnect();
          });
          slot.appendChild(eject);
        }

        if (typeof d.onComboMount === 'function') d.onComboMount(select);
      }
    }
  }

  // Column-visibility toggle menu — renders through the shared window.tlMenu
  // component. Items are checkable (tlMenu's `checked` item property renders
  // a "✓" glyph in the icon gutter); clicking one toggles the column and
  // closes the menu, same as before.
  function showColumnMenu(event, pane, deps) {
    const d = deps || {};
    if (!global.tlMenu || typeof global.tlMenu.open !== 'function') {
      console.error('files-pane-view: window.tlMenu is unavailable');
      return null;
    }

    const cols = [
      { key: 'colExt', label: 'Extension' },
      { key: 'colSize', label: 'Size' },
      { key: 'colModified', label: 'Modified' },
    ];

    const items = cols.map((col) => ({
      label: col.label,
      checked: !!pane[col.key],
      onSelect: () => {
        if (typeof d.onToggleColumn === 'function') d.onToggleColumn(col.key);
      },
    }));

    return global.tlMenu.open({
      x: event.clientX,
      y: event.clientY,
      items,
      ariaLabel: 'File columns',
      routerName: 'fp-col-context-menu',
      routerPriority: 220,
    });
  }

  // ---------------------------------------------------------------------------
  // Row context menu (right-click on a file/dir row) — renders through the
  // shared window.tlMenu component (styles/design-system/components/menu.css,
  // app/ui/tl-menu.js), which owns positioning, dismissal, and single-instance
  // behavior. Items are pre-built by the caller (files-panel.js), which owns
  // the business logic (invoke, session state); this module only translates
  // that item shape ({type: 'separator'} / {action}) into tlMenu's
  // ({separator: true} / {onSelect}).
  function showRowContextMenu(event, items) {
    if (!global.tlMenu || typeof global.tlMenu.open !== 'function') {
      console.error('files-pane-view: window.tlMenu is unavailable');
      return null;
    }

    const menuItems = (Array.isArray(items) ? items : []).map((item) => {
      if (item.type === 'separator') return { separator: true };
      return {
        label: item.label,
        icon: item.icon,
        disabled: item.disabled,
        danger: item.danger,
        title: item.title,
        onSelect: item.action,
      };
    });

    return global.tlMenu.open({
      x: event.clientX,
      y: event.clientY,
      items: menuItems,
      ariaLabel: 'File actions',
      routerName: 'fp-row-context-menu',
      routerPriority: 220,
    });
  }

  global.termlabFilesPaneView = {
    renderPane,
    transferBadgeHtml,
    transferActivityText,
    activateTransferBadge,
    showColumnMenu,
    showRowContextMenu,
  };
})(window);

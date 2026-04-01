// File Explorer Panel — dual-pane local + remote file browser.

(function (exports) {
  'use strict';

  let invoke = null;
  let panelEl = null;
  let panelWrapEl = null;
  let resizeHandleEl = null;
  let fitActiveTabFn = null;
  let getActiveTabFn = null;

  // Navigation icons — PNG assets from icons/ directory
  const ICON_BACK = '<img src="icons/go-previous-dark.png" width="12" height="12" class="fp-icon">';
  const ICON_FWD = '<img src="icons/go-next-dark.png" width="12" height="12" class="fp-icon">';
  const ICON_HOME = '<img src="icons/go-home-dark.png" width="12" height="12" class="fp-icon">';
  const ICON_REFRESH = '<img src="icons/view-refresh-dark.png" width="12" height="12" class="fp-icon">';

  // Pane state
  const localPane = createPaneState('local', true);
  const remotePane = createPaneState('remote', false);
  let activeRemotePaneId = null;

  function createPaneState(prefix, isLocal) {
    return {
      prefix,
      isLocal,
      currentPath: '',
      pathInput: '',
      backStack: [],
      forwardStack: [],
      entries: [],
      sortColumn: 'name',
      sortAscending: true,
      showHidden: false,
      colExt: false,
      colSize: true,
      colModified: false,
      error: null,
      loading: false,
      // Transfer state per entry: { [name]: { status, percent } }
      transferStatus: {},
    };
  }

  function init(opts) {
    invoke = opts.invoke;
    panelEl = opts.panelEl;
    panelWrapEl = opts.panelWrapEl;
    resizeHandleEl = opts.resizeHandleEl;
    fitActiveTabFn = opts.fitActiveTab;
    getActiveTabFn = opts.getActiveTab;

    panelEl.innerHTML = `
      <div class="fp-pane-container">
        <div class="fp-pane" id="fp-remote"></div>
        <div class="fp-transfer-bar">
          <button class="fp-transfer-btn" id="fp-download" title="Download selected file from remote to local"><svg width="14" height="14" viewBox="0 0 16 16" fill="currentColor" style="vertical-align:-2px"><path d="m 2.001 8.211 1.386 -1.385 3.635 3.635 -0.021 -8.461 h 2 l 0.021 8.461 3.634 -3.635 1.385 1.385 -6.041 6.001 z"/></svg></button>
          <button class="fp-transfer-btn" id="fp-upload" title="Upload selected file from local to remote"><svg width="14" height="14" viewBox="0 0 16 16" fill="currentColor" style="vertical-align:-2px"><path d="m 2.001 7.789 1.386 1.385 3.635 -3.635 -0.021 8.461 h 2 l 0.021 -8.461 3.634 3.635 1.385 -1.385 -6.041 -6.001 z"/></svg></button>
        </div>
        <div class="fp-pane" id="fp-local"></div>
      </div>
    `;

    panelEl.querySelector('#fp-download').addEventListener('click', doDownload);
    panelEl.querySelector('#fp-upload').addEventListener('click', doUpload);

    initResize();
    restoreLayout();

    // Start local pane at home
    invoke('get_home_dir').then((home) => {
      localPane.currentPath = home;
      localPane.pathInput = home;
      loadEntries(localPane);
    }).catch(() => {
      localPane.currentPath = '/';
      localPane.pathInput = '/';
      loadEntries(localPane);
    });

    // Listen for transfer progress
    if (opts.listen) {
      opts.listen('transfer-progress', handleTransferProgress);
    }
  }

  // ---------------------------------------------------------------------------
  // Panel visibility & resize (mirrors ssh-panel pattern)
  // ---------------------------------------------------------------------------

  function isHidden() {
    if (window.toolWindowManager) return !window.toolWindowManager.isVisible('file-explorer');
    return panelWrapEl.classList.contains('hidden');
  }
  function showPanel() {
    if (window.toolWindowManager) { window.toolWindowManager.activate('file-explorer'); return; }
    panelWrapEl.classList.remove('hidden'); if (fitActiveTabFn) fitActiveTabFn(); saveLayoutState();
  }
  function hidePanel() {
    if (window.toolWindowManager) { window.toolWindowManager.deactivate('file-explorer'); return; }
    panelWrapEl.classList.add('hidden'); if (fitActiveTabFn) fitActiveTabFn(); saveLayoutState();
  }
  function togglePanel() {
    if (window.toolWindowManager) { window.toolWindowManager.toggle('file-explorer'); return; }
    if (isHidden()) showPanel(); else hidePanel();
  }

  function initResize() {
    if (!resizeHandleEl) return;
    let dragging = false, startX = 0, startWidth = 0;

    // Prevent native drag-and-drop from hijacking the resize gesture.
    resizeHandleEl.addEventListener('dragstart', (e) => e.preventDefault());
    resizeHandleEl.style.touchAction = 'none';

    resizeHandleEl.addEventListener('pointerdown', (e) => {
      e.preventDefault();
      resizeHandleEl.setPointerCapture(e.pointerId);
      dragging = true;
      startX = e.clientX;
      startWidth = panelEl.offsetWidth;
      resizeHandleEl.classList.add('dragging');
      document.body.style.cursor = 'col-resize';
      document.body.style.userSelect = 'none';
    });
    resizeHandleEl.addEventListener('pointermove', (e) => {
      if (!dragging) return;
      const delta = e.clientX - startX; // left panel: drag right = wider
      const newWidth = Math.max(200, Math.min(600, startWidth + delta));
      panelEl.style.width = newWidth + 'px';
      if (fitActiveTabFn) fitActiveTabFn();
    });
    resizeHandleEl.addEventListener('pointerup', (e) => {
      if (!dragging) return;
      resizeHandleEl.releasePointerCapture(e.pointerId);
      dragging = false;
      resizeHandleEl.classList.remove('dragging');
      document.body.style.cursor = '';
      document.body.style.userSelect = '';
      saveLayoutState();
    });
  }

  let saveTimer = null;
  function saveLayoutState() {
    if (saveTimer) clearTimeout(saveTimer);
    saveTimer = setTimeout(() => {
      invoke('save_window_layout', {
        layout: { files_panel_width: panelEl.offsetWidth, files_panel_visible: !isHidden() },
      }).catch(() => {});
    }, 300);
  }

  async function restoreLayout() {
    if (window.toolWindowManager) return;
    try {
      const saved = await invoke('get_saved_layout');
      if (saved.files_panel_width > 100) panelEl.style.width = saved.files_panel_width + 'px';
      if (saved.files_panel_visible === false) panelWrapEl.classList.add('hidden');
      else panelWrapEl.classList.remove('hidden');
      if (fitActiveTabFn) setTimeout(fitActiveTabFn, 100);
    } catch (e) { console.error('Failed to restore files layout:', e); }
  }

  // ---------------------------------------------------------------------------
  // Remote pane — activate on SSH tab switch
  // ---------------------------------------------------------------------------

  async function onTabChanged(tab) {
    if (!tab || tab.type !== 'ssh' || !tab.spawned) {
      activeRemotePaneId = null;
      remotePane.entries = [];
      remotePane.currentPath = '';
      remotePane.error = null;
      remotePane.loading = false;
      renderPane(remotePane, panelEl.querySelector('#fp-remote'));
      return;
    }
    // Accept either a pane object (with .paneId) or a tab object (with .id).
    const id = tab.paneId != null ? tab.paneId : tab.id;
    if (activeRemotePaneId === id) return;
    activeRemotePaneId = id;

    try {
      const path = await invoke('sftp_realpath', { paneId: id, path: '.' });
      remotePane.currentPath = path;
      remotePane.pathInput = path;
      remotePane.backStack = [];
      remotePane.forwardStack = [];
      await loadEntries(remotePane);
    } catch (e) {
      remotePane.error = String(e);
      renderPane(remotePane, panelEl.querySelector('#fp-remote'));
    }
  }

  // ---------------------------------------------------------------------------
  // Data loading
  // ---------------------------------------------------------------------------

  async function loadEntries(pane) {
    pane.error = null;
    pane.loading = true;
    const el = panelEl.querySelector(`#fp-${pane.prefix}`);
    renderPane(pane, el);

    try {
      let entries;
      if (pane.isLocal) {
        entries = await invoke('local_list_dir', { path: pane.currentPath });
      } else {
        if (!activeRemotePaneId) {
          pane.entries = [];
          pane.loading = false;
          renderPane(pane, el);
          return;
        }
        entries = await invoke('sftp_list_dir', { paneId: activeRemotePaneId, path: pane.currentPath });
      }
      pane.entries = entries;
      sortEntries(pane);
    } catch (e) {
      pane.error = String(e);
      pane.entries = [];
    }
    pane.loading = false;
    renderPane(pane, el);
  }

  function sortEntries(pane) {
    const col = pane.sortColumn;
    const asc = pane.sortAscending;
    pane.entries.sort((a, b) => {
      // Dirs first always
      if (a.is_dir !== b.is_dir) return a.is_dir ? -1 : 1;
      let ord = 0;
      if (col === 'name') ord = a.name.toLowerCase().localeCompare(b.name.toLowerCase());
      else if (col === 'ext') {
        const ea = extOf(a.name), eb = extOf(b.name);
        ord = ea.localeCompare(eb);
      }
      else if (col === 'size') ord = (a.size || 0) - (b.size || 0);
      else if (col === 'modified') ord = (a.modified || 0) - (b.modified || 0);
      return asc ? ord : -ord;
    });
  }

  // ---------------------------------------------------------------------------
  // Navigation
  // ---------------------------------------------------------------------------

  function navigate(pane, path) {
    pane.backStack.push(pane.currentPath);
    pane.forwardStack = [];
    pane.currentPath = path;
    pane.pathInput = path;
    loadEntries(pane);
  }

  function goBack(pane) {
    if (pane.backStack.length === 0) return;
    pane.forwardStack.push(pane.currentPath);
    pane.currentPath = pane.backStack.pop();
    pane.pathInput = pane.currentPath;
    loadEntries(pane);
  }

  function goForward(pane) {
    if (pane.forwardStack.length === 0) return;
    pane.backStack.push(pane.currentPath);
    pane.currentPath = pane.forwardStack.pop();
    pane.pathInput = pane.currentPath;
    loadEntries(pane);
  }

  async function goHome(pane) {
    if (pane.isLocal) {
      try {
        const home = await invoke('get_home_dir');
        navigate(pane, home);
      } catch (_) {
        navigate(pane, '/');
      }
    } else {
      navigate(pane, '.');
    }
  }

  function activateEntry(pane, entry) {
    if (!entry.is_dir) return;
    const sep = '/';
    let newPath;
    if (pane.currentPath.endsWith(sep)) {
      newPath = pane.currentPath + entry.name;
    } else {
      newPath = pane.currentPath + sep + entry.name;
    }
    navigate(pane, newPath);
  }

  // ---------------------------------------------------------------------------
  // Rendering
  // ---------------------------------------------------------------------------

  function renderPane(pane, el) {
    if (!el) return;
    const isRemote = !pane.isLocal;
    const noSession = isRemote && !activeRemotePaneId;
    const label = isRemote
      ? (noSession ? 'Remote — No SSH session' : 'Remote')
      : 'Local';

    const visibleEntries = pane.entries.filter((e) => pane.showHidden || !e.name.startsWith('.'));
    const hiddenCount = pane.entries.length - visibleEntries.length;
    const footerText = hiddenCount > 0
      ? `${visibleEntries.length} items (${hiddenCount} hidden)`
      : `${visibleEntries.length} items`;

    el.innerHTML = `
      <div class="fp-pane-label">${esc(label)}</div>
      <div class="fp-toolbar">
        <button class="fp-tb-btn" data-action="back" ${pane.backStack.length === 0 ? 'disabled' : ''} title="Back">${ICON_BACK}</button>
        <button class="fp-tb-btn" data-action="forward" ${pane.forwardStack.length === 0 ? 'disabled' : ''} title="Forward">${ICON_FWD}</button>
        <input class="fp-path-input" type="text" value="${attr(pane.pathInput)}" spellcheck="false" ${noSession ? 'disabled' : ''} />
        <button class="fp-tb-btn" data-action="home" title="Home" ${noSession ? 'disabled' : ''}>${ICON_HOME}</button>
        <button class="fp-tb-btn" data-action="refresh" title="Refresh" ${noSession ? 'disabled' : ''}>${ICON_REFRESH}</button>
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
      <div class="fp-footer">${noSession ? '' : footerText}</div>
    `;

    // Populate table body
    const tbody = el.querySelector('tbody');
    for (const entry of visibleEntries) {
      const tr = document.createElement('tr');
      tr.className = 'fp-row';
      const ts = pane.transferStatus[entry.name];
      if (ts) {
        if (ts.status === 'completed') tr.classList.add('fp-transferred');
        else if (ts.status === 'in_progress') tr.classList.add('fp-transferring');
      }
      tr.dataset.name = entry.name;

      const icon = window.fileIcons.iconFor(entry.name, entry.is_dir, !pane.isLocal);
      let cells = `<td class="fp-cell-name">${icon} <span>${esc(entry.name)}</span>`;
      if (ts && ts.status === 'in_progress') {
        cells += `<span class="fp-transfer-pct">${ts.percent || 0}%</span>`;
      }
      cells += '</td>';
      if (pane.colExt) cells += `<td class="fp-cell-ext">${esc(extOf(entry.name))}</td>`;
      if (pane.colSize) cells += `<td class="fp-cell-size">${entry.is_dir ? '' : formatSize(entry.size)}</td>`;
      if (pane.colModified) cells += `<td class="fp-cell-mod">${entry.modified ? formatDate(entry.modified) : ''}</td>`;
      tr.innerHTML = cells;

      tr.addEventListener('dblclick', () => activateEntry(pane, entry));
      tr.addEventListener('click', () => {
        el.querySelectorAll('.fp-row.selected').forEach((r) => r.classList.remove('selected'));
        tr.classList.add('selected');
        pane._selectedName = entry.name;
      });
      tbody.appendChild(tr);
    }

    // Wire toolbar buttons
    el.querySelectorAll('.fp-tb-btn').forEach((btn) => {
      btn.addEventListener('click', () => {
        const action = btn.dataset.action;
        if (action === 'back') goBack(pane);
        else if (action === 'forward') goForward(pane);
        else if (action === 'home') goHome(pane);
        else if (action === 'refresh') loadEntries(pane);
        else if (action === 'hidden') { pane.showHidden = !pane.showHidden; renderPane(pane, el); }
      });
    });

    // Path input
    const pathInput = el.querySelector('.fp-path-input');
    pathInput.addEventListener('keydown', (e) => {
      if (e.key === 'Enter') {
        const val = pathInput.value.trim();
        if (val) navigate(pane, val);
      }
    });

    // Column header click to sort
    el.querySelectorAll('th[data-col]').forEach((th) => {
      th.style.cursor = 'pointer';
      th.addEventListener('click', () => {
        const col = th.dataset.col;
        if (pane.sortColumn === col) pane.sortAscending = !pane.sortAscending;
        else { pane.sortColumn = col; pane.sortAscending = true; }
        sortEntries(pane);
        renderPane(pane, el);
      });
      // Right-click to toggle column visibility
      th.addEventListener('contextmenu', (e) => {
        e.preventDefault();
        showColumnMenu(e, pane, el);
      });
    });
  }

  function sortArrow(pane, col) {
    if (pane.sortColumn !== col) return '';
    return pane.sortAscending ? ' \u25B4' : ' \u25BE';
  }

  function showColumnMenu(e, pane, el) {
    // Remove existing
    document.querySelectorAll('.fp-col-menu').forEach((m) => m.remove());

    const menu = document.createElement('div');
    menu.className = 'fp-col-menu';
    menu.style.left = e.clientX + 'px';
    menu.style.top = e.clientY + 'px';

    const cols = [
      { key: 'colExt', label: 'Extension' },
      { key: 'colSize', label: 'Size' },
      { key: 'colModified', label: 'Modified' },
    ];

    for (const c of cols) {
      const item = document.createElement('div');
      item.className = 'fp-col-menu-item';
      item.innerHTML = `<span class="fp-col-check">${pane[c.key] ? '✓' : ''}</span> ${c.label}`;
      item.addEventListener('click', () => {
        pane[c.key] = !pane[c.key];
        menu.remove();
        renderPane(pane, el);
      });
      menu.appendChild(item);
    }

    document.body.appendChild(menu);
    setTimeout(() => document.addEventListener('click', () => menu.remove(), { once: true }), 0);
  }

  // ---------------------------------------------------------------------------
  // Transfers
  // ---------------------------------------------------------------------------

  function getSelectedEntry(pane) {
    if (!pane._selectedName) return null;
    return pane.entries.find((e) => e.name === pane._selectedName) || null;
  }

  async function doDownload() {
    const entry = getSelectedEntry(remotePane);
    if (!entry || !activeRemotePaneId) return;
    if (entry.is_dir) { window.toast.warn('Not Supported', 'Directory download not yet supported.'); return; }

    const remotePath = remotePane.currentPath + '/' + entry.name;
    const localPath = localPane.currentPath.replace(/\/$/, '') + '/' + entry.name;

    try {
      const transferId = await invoke('transfer_download', {
        paneId: activeRemotePaneId,
        remotePath,
        localPath,
      });
      // Mark as transferring in local pane
      localPane.transferStatus[entry.name] = { status: 'in_progress', percent: 0, transferId };
    } catch (e) {
      window.toast.error('Download Failed', String(e));
    }
  }

  async function doUpload() {
    const entry = getSelectedEntry(localPane);
    if (!entry || !activeRemotePaneId) return;
    if (entry.is_dir) { window.toast.warn('Not Supported', 'Directory upload not yet supported.'); return; }

    const localPath = localPane.currentPath.replace(/\/$/, '') + '/' + entry.name;
    const remotePath = remotePane.currentPath + '/' + entry.name;

    try {
      const transferId = await invoke('transfer_upload', {
        paneId: activeRemotePaneId,
        localPath,
        remotePath,
      });
      // Mark as transferring in remote pane
      remotePane.transferStatus[entry.name] = { status: 'in_progress', percent: 0, transferId };
    } catch (e) {
      window.toast.error('Upload Failed', String(e));
    }
  }

  // ---------------------------------------------------------------------------
  // Transfer progress toasts
  // ---------------------------------------------------------------------------

  // Active progress toasts keyed by transfer_id.
  const activeTransferToasts = new Map();

  function handleTransferProgress(event) {
    const p = event.payload;
    if (!p || !p.transfer_id) return;

    const pct = p.total_bytes > 0 ? Math.round((p.bytes_transferred / p.total_bytes) * 100) : 0;
    const pane = p.kind === 'download' ? localPane : remotePane;

    if (p.status === 'completed') {
      removeTransferToast(p.transfer_id);
      showCompletionToast(p.file_name, p.kind);
      pane.transferStatus[p.file_name] = { status: 'completed', percent: 100 };
      loadEntries(pane);
    } else if (p.status === 'failed' || p.status === 'cancelled') {
      removeTransferToast(p.transfer_id);
      delete pane.transferStatus[p.file_name];
      if (p.status === 'failed') {
        showCompletionToast(p.file_name, p.kind, p.error);
      }
    } else {
      pane.transferStatus[p.file_name] = { status: 'in_progress', percent: pct };
      updateOrCreateTransferToast(p);
    }
  }

  function updateOrCreateTransferToast(p) {
    let toast = activeTransferToasts.get(p.transfer_id);

    if (!toast) {
      toast = document.createElement('div');
      toast.className = 'fp-progress-toast';
      toast.innerHTML = `
        <div class="fp-pt-header">
          <span class="fp-pt-kind">${p.kind === 'download' ? '\u2193' : '\u2191'}</span>
          <span class="fp-pt-filename"></span>
          <button class="fp-pt-cancel" title="Cancel transfer">\u2715</button>
        </div>
        <div class="fp-pt-bar-wrap"><div class="fp-pt-bar"></div></div>
        <div class="fp-pt-details">
          <span class="fp-pt-bytes"></span>
          <span class="fp-pt-speed"></span>
        </div>
      `;
      toast.querySelector('.fp-pt-cancel').addEventListener('click', () => {
        invoke('transfer_cancel', { transferId: p.transfer_id }).catch(() => {});
        removeTransferToast(p.transfer_id);
      });
      toast._startTime = Date.now();
      toast._startBytes = 0;
      toast._lastBytes = 0;
      toast._lastTime = Date.now();

      // Use the global toast container so transfer toasts stack with other toasts.
      let container = document.getElementById('toast-container');
      if (!container) {
        container = document.createElement('div');
        container.id = 'toast-container';
        document.body.appendChild(container);
      }
      container.appendChild(toast);
      requestAnimationFrame(() => toast.classList.add('visible'));
      activeTransferToasts.set(p.transfer_id, toast);
    }

    // Update content
    toast.querySelector('.fp-pt-filename').textContent = p.file_name;
    const pct = p.total_bytes > 0 ? Math.round((p.bytes_transferred / p.total_bytes) * 100) : 0;
    toast.querySelector('.fp-pt-bar').style.width = pct + '%';

    const bytesStr = formatSize(p.bytes_transferred) + ' / ' + formatSize(p.total_bytes);
    toast.querySelector('.fp-pt-bytes').textContent = bytesStr;

    // Calculate speed (smoothed over last interval)
    const now = Date.now();
    const elapsed = (now - toast._lastTime) / 1000;
    if (elapsed > 0.05) {
      const bytesDelta = p.bytes_transferred - toast._lastBytes;
      const speed = bytesDelta / elapsed;
      toast.querySelector('.fp-pt-speed').textContent = formatSize(Math.round(speed)) + '/s';
      toast._lastBytes = p.bytes_transferred;
      toast._lastTime = now;
    }
  }

  function removeTransferToast(transferId) {
    const toast = activeTransferToasts.get(transferId);
    if (!toast) return;
    toast.classList.remove('visible');
    activeTransferToasts.delete(transferId);
    setTimeout(() => toast.remove(), 300);
  }

  function showCompletionToast(fileName, kind, error) {
    const arrow = kind === 'download' ? '\u2193' : '\u2191';
    if (error) {
      window.toast.error(`${arrow} Transfer Failed: ${fileName}`, error);
    } else {
      window.toast.success(`${arrow} Transfer Complete: ${fileName}`);
    }
  }

  // ---------------------------------------------------------------------------
  // Helpers
  // ---------------------------------------------------------------------------

  const formatSize = window.utils.formatSize;
  const formatDate = window.utils.formatDate;

  function extOf(name) {
    const i = name.lastIndexOf('.');
    return i > 0 ? name.slice(i + 1).toLowerCase() : '';
  }

  const esc = window.utils.esc;
  const attr = window.utils.attr;

  exports.filesPanel = { init, togglePanel, isHidden, onTabChanged };
})(window);

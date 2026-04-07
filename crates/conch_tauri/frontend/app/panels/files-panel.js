// File Explorer Panel — dual-pane local + remote file browser.

(function (exports) {
  'use strict';

  let invoke = null;
  let panelEl = null;
  let panelWrapEl = null;
  let resizeHandleEl = null;
  let layoutService = null;
  const filesDataService = exports.conchFilesFeatureDataService || {};
  const filesPaneStore = exports.conchFilesPaneStore || {};
  const filesActions = exports.conchFilesActions || {};
  const filesPaneView = exports.conchFilesPaneView || {};
  const filesTransfers = exports.conchFilesTransfers || {};
  let fitActiveTabFn = null;
  let getActiveTabFn = null;
  let transferController = null;

  // Navigation icons — PNG assets from icons/ directory
  const ICON_BACK = '<img src="icons/go-previous-dark.png" width="12" height="12" class="fp-icon">';
  const ICON_FWD = '<img src="icons/go-next-dark.png" width="12" height="12" class="fp-icon">';
  const ICON_HOME = '<img src="icons/go-home-dark.png" width="12" height="12" class="fp-icon">';
  const ICON_REFRESH = '<img src="icons/view-refresh-dark.png" width="12" height="12" class="fp-icon">';

  function createPaneState(prefix, isLocal) {
    if (!filesPaneStore || typeof filesPaneStore.createPaneState !== 'function') {
      throw new Error('files-pane-store missing createPaneState');
    }
    return filesPaneStore.createPaneState(prefix, isLocal);
  }

  // Pane state
  const localPane = createPaneState('local', true);
  const remotePane = createPaneState('remote', false);
  let activeRemotePaneId = null;
  let localCwdPollTimer = null;
  let localCwdPollInFlight = false;
  let lastLocalCwdByPaneId = new Map();
  let remoteCwdPollTimer = null;
  let remoteCwdPollInFlight = false;
  let lastRemoteCwdByPaneId = new Map();

  function applyFollowPathSetting(enabled) {
    if (!filesPaneStore || typeof filesPaneStore.applyFollowPathSetting !== 'function') {
      console.error('files-pane-store missing applyFollowPathSetting');
      return;
    }
    filesPaneStore.applyFollowPathSetting(localPane, remotePane, enabled);
  }

  function loadFollowPathSetting() {
    if (!invoke) return;
    if (!filesDataService || typeof filesDataService.getAllSettings !== 'function') {
      console.error('files-data-service missing getAllSettings');
      applyFollowPathSetting(true);
      return;
    }
    if (!filesPaneStore || typeof filesPaneStore.getFollowPathFromSettings !== 'function') {
      console.error('files-pane-store missing getFollowPathFromSettings');
      applyFollowPathSetting(true);
      return;
    }
    const loadSettings = filesDataService.getAllSettings(invoke);
    loadSettings
      .then((settings) => {
        const follow = filesPaneStore.getFollowPathFromSettings(settings);
        applyFollowPathSetting(follow);
      })
      .catch(() => {
        applyFollowPathSetting(true);
      });
  }

  function init(opts) {
    invoke = opts.invoke;
    panelEl = opts.panelEl;
    panelWrapEl = opts.panelWrapEl;
    resizeHandleEl = opts.resizeHandleEl;
    layoutService = opts.layoutService
      || (window.conchServices && window.conchServices.layoutService)
      || null;
    fitActiveTabFn = opts.fitActiveTab;
    getActiveTabFn = opts.getActiveTab;
    transferController = filesTransfers && typeof filesTransfers.createController === 'function'
      ? filesTransfers.createController({
        localPane,
        remotePane,
        loadEntries,
        formatSize: window.utils && window.utils.formatSize,
        toast: window.toast,
        cancelTransfer: (transferId) => (
          filesDataService && typeof filesDataService.transferCancel === 'function'
            ? filesDataService.transferCancel(invoke, transferId)
            : Promise.reject(new Error('Files data service unavailable: transferCancel'))
        ),
      })
      : null;

    if (!panelEl) {
      console.warn('filesPanel.init called without a panel element');
      return;
    }

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
    const homePromise = filesDataService && typeof filesDataService.getHomeDir === 'function'
      ? filesDataService.getHomeDir(invoke)
      : Promise.reject(new Error('Files data service unavailable: getHomeDir'));
    homePromise.then((home) => {
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
      opts.listen('config-changed', () => {
        loadFollowPathSetting();
      });
    }

    loadFollowPathSetting();
    startLocalCwdPolling();
    startRemoteCwdPolling();
  }

  function hasPanelDom() {
    return !!panelEl;
  }

  function getActivePaneIdForType(expectedType) {
    const activeTab = getActiveTabFn ? getActiveTabFn() : null;
    if (!activeTab || activeTab.type !== expectedType) return null;
    if (activeTab.paneId != null) return activeTab.paneId;
    if (activeTab.focusedPaneId != null) return activeTab.focusedPaneId;
    if (activeTab.id != null) return activeTab.id;
    return null;
  }

  function getPaneRoot(selector) {
    return panelEl ? panelEl.querySelector(selector) : null;
  }

  // ---------------------------------------------------------------------------
  // Panel visibility & resize (mirrors ssh-panel pattern)
  // ---------------------------------------------------------------------------

  function isHidden() {
    if (window.toolWindowManager) return !window.toolWindowManager.isVisible('file-explorer');
    if (!panelWrapEl) return true;
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
      if (!panelEl) return;
      const patch = { files_panel_width: panelEl.offsetWidth, files_panel_visible: !isHidden() };
      if (layoutService && typeof layoutService.savePartialLayout === 'function') {
        layoutService.savePartialLayout(patch);
      } else {
        invoke('save_window_layout', { layout: patch }).catch(() => {});
      }
    }, 300);
  }

  async function restoreLayout() {
    if (window.toolWindowManager) return;
    try {
      const saved = layoutService && typeof layoutService.getSavedLayout === 'function'
        ? await layoutService.getSavedLayout()
        : await invoke('get_saved_layout');
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
    if (!hasPanelDom()) return;
    if (tab && tab.type === 'local') {
      const paneId = tab.paneId != null ? tab.paneId : tab.focusedPaneId;
      if (paneId != null) {
        pollActiveLocalPaneCwd(paneId);
      }
    }
    if (!tab || tab.type !== 'ssh' || !tab.spawned) {
      activeRemotePaneId = null;
      remotePane.entries = [];
      remotePane.currentPath = '';
      remotePane.error = null;
      remotePane.loading = false;
      renderPane(remotePane, getPaneRoot('#fp-remote'));
      return;
    }
    // Accept either a pane object (with .paneId) or a tab object (with .id).
    const id = tab.paneId != null ? tab.paneId : tab.id;
    if (activeRemotePaneId === id) return;
    activeRemotePaneId = id;
    pollActiveRemotePaneCwd(id);

    try {
      const path = filesDataService && typeof filesDataService.getRemoteRealPath === 'function'
        ? await filesDataService.getRemoteRealPath(invoke, id, '.')
        : await Promise.reject(new Error('Files data service unavailable: getRemoteRealPath'));
      remotePane.currentPath = path;
      remotePane.pathInput = path;
      remotePane.backStack = [];
      remotePane.forwardStack = [];
      await loadEntries(remotePane);
    } catch (e) {
      remotePane.error = String(e);
      renderPane(remotePane, getPaneRoot('#fp-remote'));
    }
  }

  function startLocalCwdPolling() {
    if (localCwdPollTimer) clearInterval(localCwdPollTimer);
    localCwdPollTimer = setInterval(() => {
      const paneId = getActivePaneIdForType('local');
      if (paneId == null) return;
      pollActiveLocalPaneCwd(paneId);
    }, 600);
  }

  function startRemoteCwdPolling() {
    if (remoteCwdPollTimer) clearInterval(remoteCwdPollTimer);
    remoteCwdPollTimer = setInterval(() => {
      const paneId = getActivePaneIdForType('ssh');
      if (paneId == null) return;
      pollActiveRemotePaneCwd(paneId);
    }, 600);
  }

  function pollActiveLocalPaneCwd(paneId) {
    if (!invoke || localCwdPollInFlight || paneId == null) return;
    const activePaneId = getActivePaneIdForType('local');
    if (activePaneId !== paneId) return;

    localCwdPollInFlight = true;
    const localCwdPromise = filesDataService && typeof filesDataService.getLocalPaneCwd === 'function'
      ? filesDataService.getLocalPaneCwd(invoke, paneId)
      : Promise.reject(new Error('Files data service unavailable: getLocalPaneCwd'));
    localCwdPromise
      .then((path) => {
        if (!path) return;
        if (lastLocalCwdByPaneId.get(paneId) === path) return;
        lastLocalCwdByPaneId.set(paneId, path);
        if (localPane.followCwd && path !== localPane.currentPath) {
          navigate(localPane, path);
        }
      })
      .catch(() => {})
      .finally(() => {
        localCwdPollInFlight = false;
      });
  }

  function pollActiveRemotePaneCwd(paneId) {
    if (!invoke || remoteCwdPollInFlight || paneId == null) return;
    const activePaneId = getActivePaneIdForType('ssh');
    if (activePaneId !== paneId) return;

    remoteCwdPollInFlight = true;
    console.info('[files-cwd] polling ssh pane cwd', paneId);
    const remoteCwdPromise = filesDataService && typeof filesDataService.getRemotePaneCwd === 'function'
      ? filesDataService.getRemotePaneCwd(invoke, paneId)
      : Promise.reject(new Error('Files data service unavailable: getRemotePaneCwd'));
    remoteCwdPromise
      .then((path) => {
        if (!path) {
          console.info('[files-cwd] ssh pane cwd empty', paneId);
          return;
        }
        console.info('[files-cwd] ssh pane cwd resolved', paneId, path);
        if (lastRemoteCwdByPaneId.get(paneId) === path) return;
        lastRemoteCwdByPaneId.set(paneId, path);
        if (remotePane.followCwd && path !== remotePane.currentPath) {
          navigate(remotePane, path);
        }
      })
      .catch((e) => {
        console.warn('Remote cwd poll failed for pane', paneId, e);
      })
      .finally(() => {
        remoteCwdPollInFlight = false;
      });
  }

  // ---------------------------------------------------------------------------
  // Data loading
  // ---------------------------------------------------------------------------

  async function loadEntries(pane) {
    if (!hasPanelDom()) return;
    pane.error = null;
    pane.loading = true;
    const el = getPaneRoot(`#fp-${pane.prefix}`);
    renderPane(pane, el);

    try {
      let entries;
      if (pane.isLocal) {
        entries = filesDataService && typeof filesDataService.listLocalDir === 'function'
          ? await filesDataService.listLocalDir(invoke, pane.currentPath)
          : await Promise.reject(new Error('Files data service unavailable: listLocalDir'));
      } else {
        if (!activeRemotePaneId) {
          pane.entries = [];
          pane.loading = false;
          renderPane(pane, el);
          return;
        }
        entries = filesDataService && typeof filesDataService.listRemoteDir === 'function'
          ? await filesDataService.listRemoteDir(invoke, activeRemotePaneId, pane.currentPath)
          : await Promise.reject(new Error('Files data service unavailable: listRemoteDir'));
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
    if (!filesPaneStore || typeof filesPaneStore.sortEntries !== 'function') {
      console.error('files-pane-store missing sortEntries');
      return;
    }
    filesPaneStore.sortEntries(pane);
  }

  // ---------------------------------------------------------------------------
  // Navigation
  // ---------------------------------------------------------------------------

  function actionDeps() {
    return {
      loadEntries,
      getHomeDir: async () => (
        filesDataService && typeof filesDataService.getHomeDir === 'function'
          ? filesDataService.getHomeDir(invoke)
          : Promise.reject(new Error('Files data service unavailable: getHomeDir'))
      ),
    };
  }

  function navigate(pane, path) {
    if (!filesActions || typeof filesActions.navigate !== 'function') {
      console.error('files-actions missing navigate');
      return;
    }
    filesActions.navigate(pane, path, actionDeps());
  }

  function goBack(pane) {
    if (!filesActions || typeof filesActions.goBack !== 'function') {
      console.error('files-actions missing goBack');
      return;
    }
    filesActions.goBack(pane, actionDeps());
  }

  function goForward(pane) {
    if (!filesActions || typeof filesActions.goForward !== 'function') {
      console.error('files-actions missing goForward');
      return;
    }
    filesActions.goForward(pane, actionDeps());
  }

  async function goHome(pane) {
    if (!filesActions || typeof filesActions.goHome !== 'function') {
      console.error('files-actions missing goHome');
      return;
    }
    await filesActions.goHome(pane, actionDeps());
  }

  function activateEntry(pane, entry) {
    if (!filesActions || typeof filesActions.activateEntry !== 'function') {
      console.error('files-actions missing activateEntry');
      return;
    }
    filesActions.activateEntry(pane, entry, actionDeps());
  }

  // ---------------------------------------------------------------------------
  // Rendering
  // ---------------------------------------------------------------------------

  function renderPane(pane, el) {
    if (!el) return;
    if (!filesPaneView || typeof filesPaneView.renderPane !== 'function') {
      console.error('files-pane-view missing renderPane');
      el.innerHTML = '<div class="fp-error">Files pane view module unavailable.</div>';
      return;
    }
    filesPaneView.renderPane(pane, el, {
      activeRemotePaneId,
      iconBack: ICON_BACK,
      iconForward: ICON_FWD,
      iconHome: ICON_HOME,
      iconRefresh: ICON_REFRESH,
      fileIcons: window.fileIcons,
      sortArrow,
      extOf,
      formatSize,
      formatDate,
      esc,
      attr,
      onActivateEntry: (entry) => activateEntry(pane, entry),
      onSelectEntry: (name) => { pane._selectedName = name; },
      onBack: () => goBack(pane),
      onForward: () => goForward(pane),
      onHome: () => goHome(pane),
      onRefresh: () => loadEntries(pane),
      onToggleHidden: () => { pane.showHidden = !pane.showHidden; renderPane(pane, el); },
      onNavigate: (path) => navigate(pane, path),
      onSort: (col) => {
        if (pane.sortColumn === col) pane.sortAscending = !pane.sortAscending;
        else { pane.sortColumn = col; pane.sortAscending = true; }
        sortEntries(pane);
        renderPane(pane, el);
      },
      onOpenColumnMenu: (event) => showColumnMenu(event, pane, el),
    });
  }

  function sortArrow(pane, col) {
    if (pane.sortColumn !== col) return '';
    return pane.sortAscending ? ' \u25B4' : ' \u25BE';
  }

  function showColumnMenu(e, pane, el) {
    if (!filesPaneView || typeof filesPaneView.showColumnMenu !== 'function') {
      console.error('files-pane-view missing showColumnMenu');
      return;
    }
    filesPaneView.showColumnMenu(e, pane, {
      onToggleColumn: (key) => {
        pane[key] = !pane[key];
        renderPane(pane, el);
      },
    });
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
      if (!filesDataService || typeof filesDataService.transferDownload !== 'function') {
        throw new Error('Files data service unavailable: transferDownload');
      }
      const transferId = await filesDataService.transferDownload(invoke, activeRemotePaneId, remotePath, localPath);
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
      if (!filesDataService || typeof filesDataService.transferUpload !== 'function') {
        throw new Error('Files data service unavailable: transferUpload');
      }
      const transferId = await filesDataService.transferUpload(invoke, activeRemotePaneId, localPath, remotePath);
      // Mark as transferring in remote pane
      remotePane.transferStatus[entry.name] = { status: 'in_progress', percent: 0, transferId };
    } catch (e) {
      window.toast.error('Upload Failed', String(e));
    }
  }

  // ---------------------------------------------------------------------------
  // Transfer progress toasts
  // ---------------------------------------------------------------------------

  function handleTransferProgress(event) {
    if (!transferController || typeof transferController.handleTransferProgress !== 'function') {
      console.error('files-transfers missing handleTransferProgress controller');
      return;
    }
    transferController.handleTransferProgress(event);
  }

  // ---------------------------------------------------------------------------
  // Helpers
  // ---------------------------------------------------------------------------

  const formatSize = window.utils.formatSize;
  const formatDate = window.utils.formatDate;

  function extOf(name) {
    if (!filesPaneStore || typeof filesPaneStore.extOf !== 'function') {
      console.error('files-pane-store missing extOf');
      return '';
    }
    return filesPaneStore.extOf(name);
  }

  const esc = window.utils.esc;
  const attr = window.utils.attr;

  exports.filesPanel = { init, togglePanel, isHidden, onTabChanged };
})(window);

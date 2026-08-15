// File Explorer Panel — dual-pane local + remote file browser.

(function (exports) {
  'use strict';

  let invoke = null;
  let panelEl = null;
  let panelWrapEl = null;
  let resizeHandleEl = null;
  let layoutService = null;
  const filesDataService = exports.termlabFilesFeatureDataService || {};
  const filesPaneStore = exports.termlabFilesPaneStore || {};
  const filesActions = exports.termlabFilesActions || {};
  const filesPaneView = exports.termlabFilesPaneView || {};
  const filesTransfers = exports.termlabFilesTransfers || {};
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
      || (window.termlabServices && window.termlabServices.layoutService)
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
        <div class="fp-pane" id="fp-local"></div>
        <div class="fp-pane" id="fp-remote"></div>
      </div>
    `;

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
    return !window.toolWindowManager.isVisible('file-explorer');
  }
  function showPanel() {
    window.toolWindowManager.activate('file-explorer');
  }
  function hidePanel() {
    window.toolWindowManager.deactivate('file-explorer');
  }
  function togglePanel() {
    window.toolWindowManager.toggle('file-explorer');
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
      onOpenRowMenu: (event, entry) => showRowContextMenu(event, pane, entry),
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
  // Transfers — reached from the row context menu (see buildRowContextMenuItems).
  // Local pane rows offer upload actions; remote pane rows offer download
  // actions, mirroring the reference app's per-pane transfer direction.
  // ---------------------------------------------------------------------------

  function joinPath(base, name) {
    const trimmed = String(base || '').replace(/\/+$/, '');
    return (trimmed || '') + '/' + name;
  }

  async function doDownload(entry) {
    if (!entry || !activeRemotePaneId) return;
    if (entry.is_dir) { window.toast.warn('Not Supported', 'Directory download not yet supported.'); return; }

    const remotePath = joinPath(remotePane.currentPath, entry.name);
    const localPath = joinPath(localPane.currentPath, entry.name);

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

  async function doUpload(entry) {
    if (!entry || !activeRemotePaneId) return;
    if (entry.is_dir) { window.toast.warn('Not Supported', 'Directory upload not yet supported.'); return; }

    const localPath = joinPath(localPane.currentPath, entry.name);
    const remotePath = joinPath(remotePane.currentPath, entry.name);

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

  async function doUploadToPath(entry) {
    if (!entry || !activeRemotePaneId) return;
    if (entry.is_dir) { window.toast.warn('Not Supported', 'Directory upload not yet supported.'); return; }

    const localPath = joinPath(localPane.currentPath, entry.name);
    showTextPromptDialog({
      title: 'Upload to Path',
      label: 'Remote destination path',
      initialValue: joinPath(remotePane.currentPath, entry.name),
      confirmLabel: 'Upload',
      onConfirm: async (remotePath) => {
        try {
          if (!filesDataService || typeof filesDataService.transferUpload !== 'function') {
            throw new Error('Files data service unavailable: transferUpload');
          }
          const transferId = await filesDataService.transferUpload(invoke, activeRemotePaneId, localPath, remotePath);
          remotePane.transferStatus[entry.name] = { status: 'in_progress', percent: 0, transferId };
        } catch (e) {
          window.toast.error('Upload Failed', String(e));
        }
      },
    });
  }

  async function doDownloadToPath(entry) {
    if (!entry || !activeRemotePaneId) return;
    if (entry.is_dir) { window.toast.warn('Not Supported', 'Directory download not yet supported.'); return; }

    const remotePath = joinPath(remotePane.currentPath, entry.name);
    showTextPromptDialog({
      title: 'Download to Path',
      label: 'Local destination path',
      initialValue: joinPath(localPane.currentPath, entry.name),
      confirmLabel: 'Download',
      onConfirm: async (localPath) => {
        try {
          if (!filesDataService || typeof filesDataService.transferDownload !== 'function') {
            throw new Error('Files data service unavailable: transferDownload');
          }
          const transferId = await filesDataService.transferDownload(invoke, activeRemotePaneId, remotePath, localPath);
          localPane.transferStatus[entry.name] = { status: 'in_progress', percent: 0, transferId };
        } catch (e) {
          window.toast.error('Download Failed', String(e));
        }
      },
    });
  }

  // ---------------------------------------------------------------------------
  // Row actions — New Folder / Rename / Delete / Copy Path (row context menu)
  // ---------------------------------------------------------------------------

  function doNewFolder(pane) {
    showTextPromptDialog({
      title: 'New Folder',
      label: 'Name',
      initialValue: '',
      confirmLabel: 'Create',
      onConfirm: (name) => {
        const path = joinPath(pane.currentPath, name);
        const mkdirPromise = pane.isLocal
          ? (filesDataService && typeof filesDataService.localMkdir === 'function'
            ? filesDataService.localMkdir(invoke, path)
            : Promise.reject(new Error('Files data service unavailable: localMkdir')))
          : (filesDataService && typeof filesDataService.remoteMkdir === 'function'
            ? filesDataService.remoteMkdir(invoke, activeRemotePaneId, path)
            : Promise.reject(new Error('Files data service unavailable: remoteMkdir')));
        mkdirPromise
          .then(() => loadEntries(pane))
          .catch((e) => window.toast.error('New Folder Failed', String(e)));
      },
    });
  }

  function doRename(pane, entry) {
    showTextPromptDialog({
      title: 'Rename',
      label: 'Name',
      initialValue: entry.name,
      confirmLabel: 'Rename',
      onConfirm: (newName) => {
        if (newName === entry.name) return;
        const from = joinPath(pane.currentPath, entry.name);
        const to = joinPath(pane.currentPath, newName);
        const renamePromise = pane.isLocal
          ? (filesDataService && typeof filesDataService.localRename === 'function'
            ? filesDataService.localRename(invoke, from, to)
            : Promise.reject(new Error('Files data service unavailable: localRename')))
          : (filesDataService && typeof filesDataService.remoteRename === 'function'
            ? filesDataService.remoteRename(invoke, activeRemotePaneId, from, to)
            : Promise.reject(new Error('Files data service unavailable: remoteRename')));
        renamePromise
          .then(() => loadEntries(pane))
          .catch((e) => window.toast.error('Rename Failed', String(e)));
      },
    });
  }

  function doDelete(pane, entry) {
    showConfirmDialog({
      title: 'Delete',
      message: `Delete "${entry.name}"? This cannot be undone.`,
      confirmLabel: 'Delete',
      danger: true,
      onConfirm: () => {
        const path = joinPath(pane.currentPath, entry.name);
        const removePromise = pane.isLocal
          ? (filesDataService && typeof filesDataService.localRemove === 'function'
            ? filesDataService.localRemove(invoke, path, !!entry.is_dir)
            : Promise.reject(new Error('Files data service unavailable: localRemove')))
          : (filesDataService && typeof filesDataService.remoteRemove === 'function'
            ? filesDataService.remoteRemove(invoke, activeRemotePaneId, path, !!entry.is_dir)
            : Promise.reject(new Error('Files data service unavailable: remoteRemove')));
        removePromise
          .then(() => loadEntries(pane))
          .catch((e) => window.toast.error('Delete Failed', String(e)));
      },
    });
  }

  function doCopyPath(pane, entry) {
    const path = joinPath(pane.currentPath, entry.name);
    const copyPromise = filesDataService && typeof filesDataService.clipboardWriteText === 'function'
      ? filesDataService.clipboardWriteText(invoke, path)
      : Promise.reject(new Error('Files data service unavailable: clipboardWriteText'));
    Promise.resolve(copyPromise)
      .then(() => window.toast.success('Copied', 'Path copied to clipboard.'))
      .catch((e) => window.toast.error('Copy Failed', String(e)));
  }

  // ---------------------------------------------------------------------------
  // Row context menu
  // ---------------------------------------------------------------------------

  function buildRowContextMenuItems(pane, entry) {
    const noSession = !activeRemotePaneId;
    const sessionTitle = pane.isLocal
      ? 'Connect to an SSH session to upload files.'
      : 'Connect to an SSH session to download files.';

    const items = [
      { icon: 'newFolder', label: 'New Folder…', action: () => doNewFolder(pane) },
      { type: 'separator' },
      { icon: 'edit', label: 'Rename…', action: () => doRename(pane, entry) },
      { icon: 'remove', label: 'Delete', danger: true, action: () => doDelete(pane, entry) },
      { type: 'separator' },
      { icon: 'copy', label: 'Copy Path', action: () => doCopyPath(pane, entry) },
      { type: 'separator' },
    ];

    if (pane.isLocal) {
      items.push({
        label: 'Upload to remote host',
        disabled: noSession,
        title: noSession ? sessionTitle : undefined,
        action: () => doUpload(entry),
      });
      items.push({
        label: 'Upload to path…',
        disabled: noSession,
        title: noSession ? sessionTitle : undefined,
        action: () => doUploadToPath(entry),
      });
    } else {
      items.push({
        label: 'Download to local host',
        disabled: noSession,
        title: noSession ? sessionTitle : undefined,
        action: () => doDownload(entry),
      });
      items.push({
        label: 'Download to path…',
        disabled: noSession,
        title: noSession ? sessionTitle : undefined,
        action: () => doDownloadToPath(entry),
      });
    }

    items.push({ type: 'separator' });
    items.push({ icon: 'refresh', label: 'Refresh', action: () => loadEntries(pane) });

    return items;
  }

  function showRowContextMenu(event, pane, entry) {
    if (!filesPaneView || typeof filesPaneView.showRowContextMenu !== 'function') {
      console.error('files-pane-view missing showRowContextMenu');
      return;
    }
    filesPaneView.showRowContextMenu(event, buildRowContextMenuItems(pane, entry));
  }

  // ---------------------------------------------------------------------------
  // Small single-field / confirm dialogs — reuse the app-wide .ssh-overlay /
  // .ssh-form popup styling (see styles/dialogs.css) already used by the SSH,
  // tunnel, vault, and settings dialogs; no files-panel-specific CSS needed.
  // ---------------------------------------------------------------------------

  function removeFilesOverlay() {
    document.querySelectorAll('.fp-dialog-overlay').forEach((el) => el.remove());
  }

  function setFilesOverlayAttributes(overlay, label) {
    overlay.setAttribute('role', 'dialog');
    overlay.setAttribute('aria-modal', 'true');
    overlay.setAttribute('aria-label', String(label || 'Dialog'));
  }

  function registerFilesOverlayKeys(overlay, name, onKeyDown) {
    const keyboardRouter = window.termlabKeyboardRouter;
    if (keyboardRouter && typeof keyboardRouter.register === 'function') {
      return keyboardRouter.register({
        name: name || 'fp-overlay',
        priority: 220,
        isActive: () => !!(overlay && overlay.isConnected),
        onKeyDown: (event) => {
          if (!overlay || !overlay.isConnected) return false;
          return onKeyDown(event) === true;
        },
      });
    }
    console.warn('files-panel: keyboard router unavailable, skipping overlay handler registration:', name || 'fp-overlay');
    return () => {};
  }

  function showTextPromptDialog(opts) {
    const o = opts || {};
    removeFilesOverlay();

    const overlay = document.createElement('div');
    overlay.className = 'ssh-overlay fp-dialog-overlay';
    overlay.style.zIndex = '4500';
    setFilesOverlayAttributes(overlay, o.title);
    overlay.innerHTML = `
      <div class="ssh-form ssh-form-small">
        <div class="ssh-form-title">${esc(o.title)}</div>
        <div class="ssh-form-body">
          <label class="ssh-form-label">${esc(o.label)}
            <input type="text" id="fp-dlg-input" value="${attr(o.initialValue || '')}" spellcheck="false" />
          </label>
        </div>
        <div class="ssh-form-buttons">
          <button class="ssh-form-btn" id="fp-dlg-cancel">Cancel</button>
          <button class="ssh-form-btn primary" id="fp-dlg-confirm">${esc(o.confirmLabel || 'OK')}</button>
        </div>
      </div>
    `;
    document.body.appendChild(overlay);

    const input = overlay.querySelector('#fp-dlg-input');
    setTimeout(() => { input.focus(); input.select(); }, 50);

    let closed = false;
    const dismiss = () => {
      if (closed) return;
      closed = true;
      if (typeof unregisterKeys === 'function') unregisterKeys();
      overlay.remove();
    };
    const confirm = () => {
      const value = input.value.trim();
      if (!value) { input.focus(); return; }
      dismiss();
      if (typeof o.onConfirm === 'function') o.onConfirm(value);
    };
    const unregisterKeys = registerFilesOverlayKeys(overlay, 'fp-text-prompt-dialog', (event) => {
      if (event.key === 'Escape') { dismiss(); return true; }
      if (event.key === 'Enter') { confirm(); return true; }
      return false;
    });
    overlay.addEventListener('mousedown', (event) => { if (event.target === overlay) dismiss(); });
    overlay.querySelector('#fp-dlg-cancel').addEventListener('click', dismiss);
    overlay.querySelector('#fp-dlg-confirm').addEventListener('click', confirm);
  }

  function showConfirmDialog(opts) {
    const o = opts || {};
    removeFilesOverlay();

    const overlay = document.createElement('div');
    overlay.className = 'ssh-overlay fp-dialog-overlay';
    overlay.style.zIndex = '4500';
    setFilesOverlayAttributes(overlay, o.title);
    const dangerStyle = o.danger ? ' style="background:var(--red);border-color:var(--red)"' : '';
    overlay.innerHTML = `
      <div class="ssh-form ssh-form-small">
        <div class="ssh-form-title">${esc(o.title)}</div>
        <div class="ssh-form-body">
          <div class="ssh-auth-message">${esc(o.message)}</div>
        </div>
        <div class="ssh-form-buttons">
          <button class="ssh-form-btn" id="fp-dlg-cancel">Cancel</button>
          <button class="ssh-form-btn primary" id="fp-dlg-confirm"${dangerStyle}>${esc(o.confirmLabel || 'OK')}</button>
        </div>
      </div>
    `;
    document.body.appendChild(overlay);

    let closed = false;
    const dismiss = () => {
      if (closed) return;
      closed = true;
      if (typeof unregisterKeys === 'function') unregisterKeys();
      overlay.remove();
    };
    const confirm = () => {
      dismiss();
      if (typeof o.onConfirm === 'function') o.onConfirm();
    };
    const unregisterKeys = registerFilesOverlayKeys(overlay, 'fp-confirm-dialog', (event) => {
      if (event.key !== 'Escape') return false;
      dismiss();
      return true;
    });
    overlay.addEventListener('mousedown', (event) => { if (event.target === overlay) dismiss(); });
    overlay.querySelector('#fp-dlg-cancel').addEventListener('click', dismiss);
    overlay.querySelector('#fp-dlg-confirm').addEventListener('click', confirm);
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

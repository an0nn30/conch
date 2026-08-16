// Settings Dialog — thin orchestrator. State lives in the settings store,
// rendering in the settings renderers/section modules, and save/apply flows
// in the settings actions module (app/features/settings/).

(function (exports) {
  'use strict';

  let invoke = null;
  let standaloneEscapeHandler = null;
  let standaloneMode = false;   // true when running in its own window
  let standaloneRoot = null;    // root element in standalone mode
  let settingsSearchAutofocusTimer = null;
  // The tl-dialog handle for the modal shell (open()'s path only —
  // standalone mode never sets this). Same pattern as tunnel-manager.js's
  // activeDialogHandle: null while no modal is open, doubles as the
  // "is a settings dialog currently open" check.
  let dialogHandle = null;

  const settingsDataService = exports.termlabSettingsFeatureDataService || {};
  const settingsSearchFeature = exports.termlabSettingsFeatureSearch || {};
  const settingsSidebarFeature = exports.termlabSettingsSidebar || {};
  const settingsStoreFactory = exports.termlabSettingsStore || {};
  const settingsActions = exports.termlabSettingsActions || {};
  const settingsRenderers = exports.termlabSettingsRenderers || {};

  const store = typeof settingsStoreFactory.create === 'function'
    ? settingsStoreFactory.create()
    : null;

  function registerGlobalKeyHandler(name, onKeyDown, isActive) {
    const keyboardRouter = window.termlabKeyboardRouter;
    if (keyboardRouter && typeof keyboardRouter.register === 'function') {
      return keyboardRouter.register({
        name: name || 'settings-key-handler',
        priority: 210,
        isActive: typeof isActive === 'function' ? isActive : null,
        onKeyDown: (event) => onKeyDown(event) === true,
      });
    }

    console.warn('settings: keyboard router unavailable, skipping handler registration:', name || 'settings-key-handler');
    return () => {};
  }

  const recorder = typeof settingsRenderers.createShortcutRecorder === 'function' && store
    ? settingsRenderers.createShortcutRecorder({
        getShortcutValue: (ref) => store.getShortcutValue(ref),
        setShortcutValue: (ref, value) => store.setShortcutValue(ref, value),
        registerGlobalKeyHandler,
      })
    : null;

  const sectionRenderers = typeof settingsRenderers.createSectionRenderers === 'function' && store && recorder
    ? settingsRenderers.createSectionRenderers({
        store,
        recorder,
        getInvoke: () => invoke,
        renderCurrentSection: () => renderCurrentSection(),
        renderSidebarInto: (el) => renderSidebarInto(el),
        refreshPluginInventory: () => settingsDataService.refreshPluginInventory(invoke),
        confirmPluginPermissions: (pluginName, permissions) => settingsActions.confirmPluginPermissions(pluginName, permissions),
        invalidateCommandPaletteCache: (reason) => settingsActions.invalidateCommandPaletteCache(reason),
      })
    : null;

  function settingsModulesReady() {
    if (store && recorder && sectionRenderers) return true;
    if (window.toast && typeof window.toast.error === 'function') {
      window.toast.error('Settings Error', 'Settings feature modules are unavailable.');
    }
    return false;
  }

  function isRecording() {
    return !!(recorder && recorder.isRecording());
  }

  function clearSettingsAutofocusTimer() {
    if (settingsSearchAutofocusTimer) {
      clearTimeout(settingsSearchAutofocusTimer);
      settingsSearchAutofocusTimer = null;
    }
  }

  function init(opts) {
    invoke = opts.invoke;
  }

  function focusSettingsSearchInput(selectAll) {
    clearSettingsAutofocusTimer();
    settingsSearchAutofocusTimer = setTimeout(() => {
      const input = document.querySelector('#settings-sidebar .tl-settings__search');
      if (!input) return;
      input.focus();
      if (selectAll) input.select();
    }, 0);
  }

  function moveSidebarSearchSelection(delta) {
    const results = store.getSidebarResults();
    if (results.length === 0) return;
    const index = store.getSidebarSelectionIndex();
    if (index < 0) {
      store.setSidebarSelectionIndex(delta > 0 ? 0 : results.length - 1);
    } else {
      store.setSidebarSelectionIndex(Math.max(0, Math.min(results.length - 1, index + delta)));
    }
    const sidebar = document.getElementById('settings-sidebar');
    if (!sidebar) return;
    renderSidebarInto(sidebar);
    const selectedEl = sidebar.querySelector('.tl-settings__item.is-selected');
    if (selectedEl) selectedEl.scrollIntoView({ block: 'nearest' });
    const input = sidebar.querySelector('.tl-settings__search');
    if (input) {
      input.focus();
      input.setSelectionRange(input.value.length, input.value.length);
    }
  }

  function onSidebarSearchResultSelected(match) {
    if (match.section === 'keyboard' && match.kind === 'tool-window') {
      store.setKeyboardSearchQuery(match.label);
    } else if (match.section === 'keyboard') {
      store.setKeyboardSearchQuery(match.label);
    }
    store.registerPendingSettingsJump(match);
    selectSection(match.section);
  }

  function renderSidebarInto(sidebar) {
    if (!settingsSidebarFeature || typeof settingsSidebarFeature.renderSidebarInto !== 'function') {
      if (window.toast && typeof window.toast.error === 'function') {
        window.toast.error('Settings Error', 'Sidebar section module is unavailable.');
      }
      return;
    }
    settingsSidebarFeature.renderSidebarInto(sidebar, {
      sectionDefs: store.getSectionDefs(),
      normalizeSearchText: settingsSearchFeature.normalizeSearchText,
      getFuzzyMatchScore: settingsSearchFeature.getFuzzyMatchScore,
      getSidebarSearchResults: (query) => store.getSidebarSearchResults(query),
      appendHighlightedText: settingsSearchFeature.appendHighlightedText,
      getSidebarQuery: () => store.getSidebarQuery(),
      setSidebarQuery: (value) => store.setSidebarQuery(value),
      getSidebarSelectionIndex: () => store.getSidebarSelectionIndex(),
      setSidebarSelectionIndex: (value) => store.setSidebarSelectionIndex(value),
      getSidebarResults: () => store.getSidebarResults(),
      setSidebarResults: (results) => store.setSidebarResults(results),
      getCurrentSection: () => store.getCurrentSection(),
      isGroupCollapsed: (groupLabel) => store.isSidebarGroupCollapsed(groupLabel),
      toggleGroupCollapsed: (groupLabel) => store.toggleSidebarGroupCollapsed(groupLabel),
      moveSidebarSearchSelection,
      onSidebarSearchResultSelected,
      selectSection,
    });
  }

  async function open() {
    if (dialogHandle) { close(); return; }
    if (!settingsModulesReady()) return;

    try {
      await settingsActions.discardPluginSettingsDrafts(invoke);
      const loaded = await settingsDataService.loadRuntimeData(invoke);
      store.applyLoadedSettingsData(loaded);
      renderDialog();
    } catch (e) {
      if (window.toast) window.toast.error('Settings', 'Failed to load settings: ' + e);
    }
  }

  /** Open settings in a standalone window (called from settings.html). */
  async function openInWindow(rootEl) {
    standaloneMode = true;
    standaloneRoot = rootEl;
    if (!settingsModulesReady()) return;

    try {
      await settingsActions.discardPluginSettingsDrafts(invoke);
      const loaded = await settingsDataService.loadRuntimeData(invoke);
      store.applyLoadedSettingsData(loaded);
      renderStandalone();
    } catch (e) {
      if (window.toast) window.toast.error('Settings', 'Failed to load settings: ' + e);
    }
  }

  // Shared by both shells: Cancel discards (just close()); Apply saves
  // without closing (footer stays open, its own disabled state resets once
  // store.isDirty() goes false again — see renderers.js's
  // wireApplyDirtyTracking); OK saves and closes. See actions.js's
  // applySettings for the keepOpen/onApplied contract.
  function applySettingsKeepOpen() {
    return settingsActions.applySettings({
      invoke,
      getPendingSettings: () => store.getPendingSettings(),
      isStandaloneMode: () => standaloneMode,
      markSkipPluginDraftDiscard: () => store.setSkipPluginDraftDiscardOnClose(true),
      close,
      keepOpen: true,
      onApplied: () => store.commitPendingAsOriginal(),
    });
  }

  function applySettingsAndClose() {
    return settingsActions.applySettings({
      invoke,
      getPendingSettings: () => store.getPendingSettings(),
      isStandaloneMode: () => standaloneMode,
      markSkipPluginDraftDiscard: () => store.setSkipPluginDraftDiscardOnClose(true),
      close,
      keepOpen: false,
    });
  }

  function shellDeps() {
    return {
      close,
      applyKeepOpen: applySettingsKeepOpen,
      applyAndClose: applySettingsAndClose,
      isDirty: () => store.isDirty(),
      renderSidebarInto,
      isRecording,
      setSidebarQuery: (value) => store.setSidebarQuery(value),
    };
  }

  /** Render settings as a full-window layout (no overlay, no modal). */
  function renderStandalone() {
    settingsRenderers.renderStandaloneShell(standaloneRoot, shellDeps());

    if (standaloneEscapeHandler) {
      standaloneEscapeHandler();
      standaloneEscapeHandler = null;
    }
    standaloneEscapeHandler = registerGlobalKeyHandler(
      'settings-standalone-escape',
      (event) => {
        if (event.key !== 'Escape') return false;
        if (isRecording()) return false; // let recording handler handle it
        close();
        return true;
      },
      () => standaloneMode && !!standaloneRoot && standaloneRoot.isConnected
    );

    renderCurrentSection();
    focusSettingsSearchInput(true);
  }

  // Modal path: tl-dialog (app/ui/tl-dialog.js) owns Escape, the focus trap,
  // focus restore, depth stacking, and backdrop dismissal for this dialog —
  // there is no priority-210 Escape registration here anymore (that
  // responsibility moved to tl-dialog's own priority-225 registration when
  // this shell was ported onto tlDialog.open()). handleDialogClosed below is
  // wired through renderDialogShell's deps.onClose so store/plugin-draft
  // cleanup still runs on every close path (Escape, backdrop click, Cancel,
  // OK) — not just the ones that happen to call close() directly.
  function renderDialog() {
    dialogHandle = settingsRenderers.renderDialogShell({
      ...shellDeps(),
      onClose: handleDialogClosed,
    });

    // Render initial section
    renderCurrentSection();
    focusSettingsSearchInput(true);
  }

  function handleDialogClosed() {
    dialogHandle = null;
    if (recorder) recorder.stopRecording();
    clearSettingsAutofocusTimer();
    store.clearLoadedSettings();
    if (!store.getSkipPluginDraftDiscardOnClose()) {
      settingsActions.discardPluginSettingsDrafts(invoke);
    }
    store.setSkipPluginDraftDiscardOnClose(false);
  }

  function close() {
    if (standaloneMode) {
      if (recorder) recorder.stopRecording();
      clearSettingsAutofocusTimer();
      if (!store.getSkipPluginDraftDiscardOnClose()) {
        settingsActions.discardPluginSettingsDrafts(invoke);
      }
      store.setSkipPluginDraftDiscardOnClose(false);
      if (standaloneEscapeHandler) {
        standaloneEscapeHandler();
        standaloneEscapeHandler = null;
      }
      // In standalone window mode, close the window itself.
      const tauri = window.__TAURI__;
      if (tauri) {
        tauri.window.getCurrentWindow().close();
      }
      return;
    }
    // Route through the tl-dialog handle rather than duplicating teardown
    // here — its own close() triggers handleDialogClosed via the onClose
    // callback passed to renderDialogShell, which is the single place that
    // cleanup runs regardless of whether Cancel/OK, Escape, or a backdrop
    // click triggered it.
    if (dialogHandle) dialogHandle.close();
  }

  function selectSection(id) {
    store.setCurrentSection(id);
    const sidebar = document.getElementById('settings-sidebar');
    if (sidebar) renderSidebarInto(sidebar);
    renderCurrentSection();
  }

  // "<group> › <section>" per the reference's content header (METRICS.md:
  // "breadcrumb header (Appearance & Behavior > Appearance)").
  function getBreadcrumbParts(sectionId) {
    for (const group of store.getSectionDefs()) {
      for (const item of group.items) {
        if (item.id === sectionId) return { group: group.group, label: item.label };
      }
    }
    return null;
  }

  function renderCurrentSection() {
    const content = document.getElementById('settings-content');
    if (!content) return;
    content.innerHTML = '';

    const crumb = getBreadcrumbParts(store.getCurrentSection());
    if (crumb) {
      const crumbEl = document.createElement('div');
      crumbEl.className = 'tl-settings__breadcrumb';
      crumbEl.textContent = `${crumb.group} › ${crumb.label}`;
      content.appendChild(crumbEl);
    }

    sectionRenderers.renderSection(content, store.getCurrentSection());

    const jump = store.getPendingSettingsJump();
    if (jump && jump.section === store.getCurrentSection()) {
      requestAnimationFrame(() => {
        const root = document.getElementById('settings-content');
        if (!root) return;
        if (settingsRenderers.applyPendingSettingsJump(root, store.getPendingSettingsJump())) {
          store.clearPendingSettingsJump();
        }
      });
    }
  }

  exports.settings = { init, open, openInWindow, close };
})(window);

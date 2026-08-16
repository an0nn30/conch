// Settings feature — apply/save/restart flows and cross-surface side effects
// (plugin draft lifecycle, permission confirmation, command palette cache).

(function initTermLabSettingsActions(global) {
  'use strict';

  async function discardPluginSettingsDrafts(invoke) {
    if (!invoke) return;
    try {
      await invoke('discard_plugin_settings_drafts');
    } catch (_) {}
  }

  function invalidateCommandPaletteCache(reason) {
    if (typeof global.__termlabInvalidateCommandPaletteCache === 'function') {
      global.__termlabInvalidateCommandPaletteCache(reason || 'settings');
    }
  }

  function confirmPluginPermissions(pluginName, permissions) {
    if (global.termlabDialogService && typeof global.termlabDialogService.confirmPluginPermissions === 'function') {
      return global.termlabDialogService.confirmPluginPermissions(pluginName, permissions);
    }
    if (global.toast && typeof global.toast.error === 'function') {
      global.toast.error('Plugin Permissions', 'Dialog service unavailable; denying permission request.');
    }
    return Promise.resolve(false);
  }

  /**
   * Save pending settings and commit plugin drafts. By default also closes
   * the dialog/window (OK's behavior); pass deps.keepOpen: true for Apply's
   * behavior — save without closing, then call deps.onApplied() (Task 1's
   * shell wires this to store.commitPendingAsOriginal(), which is what makes
   * the footer's Apply button go disabled again until the next edit).
   * deps: {
   *   invoke, getPendingSettings, isStandaloneMode, markSkipPluginDraftDiscard,
   *   close, keepOpen, onApplied,
   * }
   */
  async function applySettings(deps) {
    const d = deps || {};
    try {
      const result = await d.invoke('save_settings', { settings: d.getPendingSettings() });
      await d.invoke('commit_plugin_settings_drafts');
      d.markSkipPluginDraftDiscard();

      if (d.keepOpen) {
        // The window/dialog stays open, so there's no need to relay the
        // restart toast to the main window the way the close path below
        // does — this surface can show it directly (toast.js is loaded in
        // both index.html and settings.html).
        if (typeof d.onApplied === 'function') d.onApplied();
        if (result && result.restart_required && global.toast) {
          global.toast.warn('Restart Required', 'Some changes require a restart to take effect.');
        }
        return;
      }

      if (d.isStandaloneMode() && result && result.restart_required) {
        // Emit to the main window so the toast is visible after this window closes.
        try {
          await global.__TAURI__.event.emit('settings-restart-required');
        } catch (_) {}
      }
      d.close();
      if (!d.isStandaloneMode() && result && result.restart_required) {
        if (global.toast) global.toast.warn('Restart Required', 'Some changes require a restart to take effect.');
      }
    } catch (e) {
      if (global.toast) global.toast.error('Settings Error', 'Failed to save settings: ' + e);
    }
  }

  global.termlabSettingsActions = {
    discardPluginSettingsDrafts,
    invalidateCommandPaletteCache,
    confirmPluginPermissions,
    applySettings,
  };
})(window);

(function initConchSettingsFeatureDataService(global) {
  'use strict';

  async function loadRuntimeData(invoke) {
    const [settings, themes, plugins, pluginMenuItems, fonts] = await Promise.all([
      invoke('get_all_settings'),
      invoke('list_themes'),
      invoke('scan_plugins'),
      invoke('get_plugin_menu_items').catch(() => []),
      invoke('list_system_fonts'),
    ]);
    return {
      settings,
      themes,
      plugins: Array.isArray(plugins) ? plugins : [],
      pluginMenuItems: Array.isArray(pluginMenuItems) ? pluginMenuItems : [],
      fonts: fonts && typeof fonts === 'object' ? fonts : { all: [], monospace: [] },
    };
  }

  async function refreshPluginInventory(invoke) {
    const [plugins, pluginMenuItems] = await Promise.all([
      invoke('scan_plugins'),
      invoke('get_plugin_menu_items').catch(() => []),
    ]);
    return {
      plugins: Array.isArray(plugins) ? plugins : [],
      pluginMenuItems: Array.isArray(pluginMenuItems) ? pluginMenuItems : [],
    };
  }

  async function setPluginLoadedState(invoke, plugin, nextLoaded, options) {
    const opts = options || {};
    if (!nextLoaded) {
      await invoke('disable_plugin', { name: plugin.name, source: plugin.source });
      await invoke('rebuild_menu').catch(() => {});
      return { status: 'disabled' };
    }

    const permissions = Array.isArray(plugin.permissions) ? plugin.permissions.filter(Boolean) : [];
    if (permissions.length > 0 && typeof opts.confirmPermissions === 'function') {
      const accepted = await opts.confirmPermissions(plugin.name, permissions);
      if (!accepted) return { status: 'cancelled' };
    }

    await invoke('enable_plugin', { name: plugin.name, source: plugin.source, path: plugin.path });
    await invoke('rebuild_menu').catch(() => {});
    return { status: 'enabled' };
  }

  global.conchSettingsFeatureDataService = {
    loadRuntimeData,
    refreshPluginInventory,
    setPluginLoadedState,
  };
})(window);

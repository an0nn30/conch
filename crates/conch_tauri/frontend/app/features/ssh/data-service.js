(function initConchSshFeatureDataService(global) {
  'use strict';

  async function getServers(invoke) {
    const data = await invoke('remote_get_servers');
    return data && typeof data === 'object'
      ? data
      : { folders: [], ungrouped: [], ssh_config: [] };
  }

  async function getTunnels(invoke) {
    const tunnels = await invoke('tunnel_get_all');
    return Array.isArray(tunnels) ? tunnels : [];
  }

  async function getSessions(invoke) {
    const sessions = await invoke('remote_get_sessions');
    return Array.isArray(sessions) ? sessions : [];
  }

  async function exportSelection(invoke, serverIds, tunnelIds) {
    return invoke('remote_export', { serverIds, tunnelIds });
  }

  async function importConfig(invoke) {
    return invoke('remote_import');
  }

  global.conchSshFeatureDataService = {
    getServers,
    getTunnels,
    getSessions,
    exportSelection,
    importConfig,
  };
})(window);

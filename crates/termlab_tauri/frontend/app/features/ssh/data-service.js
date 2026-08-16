(function initTermLabSshFeatureDataService(global) {
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

  async function exportBundle(invoke, serverIds, tunnelIds, includeCredentials, password) {
    return invoke('share_export', { serverIds, tunnelIds, includeCredentials, password });
  }

  async function importConfig(invoke) {
    return invoke('remote_import');
  }

  global.termlabSshFeatureDataService = {
    getServers,
    getTunnels,
    getSessions,
    exportBundle,
    importConfig,
  };
})(window);

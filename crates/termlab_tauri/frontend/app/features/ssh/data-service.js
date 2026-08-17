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

  async function exportBundle(invoke, serverIds, tunnelIds, includeCredentials, password, declinedServerIds) {
    return invoke('share_export', {
      serverIds,
      tunnelIds,
      declinedServerIds: declinedServerIds || [],
      includeCredentials,
      password,
    });
  }

  /** Open the native picker (offers both *.termlabshare and *.json) and
   * classify the chosen file by its magic bytes — never its extension —
   * so a renamed file still routes correctly. See
   * share_commands.rs::detect_import_kind. */
  async function pickImportFile(invoke) {
    return invoke('share_pick_import_file');
  }

  /** Import an already-picked `path` — routed server-side to the bundle
   * (decode/plan/execute) or legacy JSON (merge_import) path by its own
   * magic-byte check. `password` is ignored server-side for a legacy
   * file. */
  async function importFile(invoke, path, password) {
    return invoke('share_import', { path, password });
  }

  global.termlabSshFeatureDataService = {
    getServers,
    getTunnels,
    getSessions,
    exportBundle,
    pickImportFile,
    importFile,
  };
})(window);

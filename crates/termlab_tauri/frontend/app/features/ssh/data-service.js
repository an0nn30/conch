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

  /** Plan step of the export flow (2026-08-16 review finding I3): runs the
   * real backend planner and returns exactly what a `share_export` call
   * with this same selection would produce — auto-pulled hosts, which keys
   * would be embedded, every warning — without encoding or writing
   * anything. The caller shows this to the user for confirmation before
   * calling `exportBundle`. No password: nothing is encrypted at this
   * step. */
  async function previewExport(invoke, serverIds, tunnelIds, includeCredentials, declinedServerIds) {
    return invoke('share_export_preview', {
      serverIds,
      tunnelIds,
      declinedServerIds: declinedServerIds || [],
      includeCredentials,
    });
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
   * file.
   *
   * Calls `share_import_apply` with no decisions, so every row keeps the
   * planner's default action — the same net behaviour the old combined
   * import command had. A future task wires this to `share_import_plan`'s
   * preview and lets the caller pass real `ImportDecision`s here instead
   * of an empty array. */
  async function importFile(invoke, path, password) {
    return invoke('share_import_apply', { path, password, decisions: [] });
  }

  global.termlabSshFeatureDataService = {
    getServers,
    getTunnels,
    getSessions,
    previewExport,
    exportBundle,
    pickImportFile,
    importFile,
  };
})(window);

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
   * magic-byte check. `password` is ignored server-side for a legacy file.
   *
   * `decisions` is the `ImportDecision[]` array to send to
   * `share_import_apply` — task-5's preview dialog (ssh-panel.js's
   * showImportPreviewDialog/runImport) passes the real per-row overrides
   * the user made there. Omitted (or passed as a non-array), it defaults to
   * `[]`, meaning every row keeps the planner's default action — this is
   * the only option for the legacy-JSON path (ssh-panel.js's importConfig
   * calls this with no fourth argument for it): a legacy file has no
   * conflicts to preview in the first place, since `share_import_plan`
   * rejects it outright (see do_import_plan in share_commands.rs), so
   * there is no decisions array to pass. */
  async function importFile(invoke, path, password, decisions) {
    return invoke('share_import_apply', { path, password, decisions: Array.isArray(decisions) ? decisions : [] });
  }

  /** Plan step of the import flow (task-4): decodes the bundle at `path`
   * with `password` and reports `includes_credentials` and `vault_state`
   * ("absent" | "locked" | "unlocked") without writing anything — not to
   * config, not to disk, not to the vault. The caller uses this to resolve
   * the vault (create/unlock it) before the apply call. Only valid for a
   * bundle file; the backend rejects a legacy JSON path with "Legacy JSON
   * imports have no preview" (ssh-panel.js's importConfig never calls this
   * for one — see its branch on `share_pick_import_file`'s `kind`). */
  async function planImport(invoke, path, password) {
    return invoke('share_import_plan', { path, password });
  }

  global.termlabSshFeatureDataService = {
    getServers,
    getTunnels,
    getSessions,
    previewExport,
    exportBundle,
    pickImportFile,
    importFile,
    planImport,
  };
})(window);

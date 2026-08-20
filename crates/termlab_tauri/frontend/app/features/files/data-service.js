(function initTermLabFilesFeatureDataService(global) {
  'use strict';

  async function getAllSettings(invoke) {
    return invoke('get_all_settings');
  }

  async function getHomeDir(invoke) {
    return invoke('get_home_dir');
  }

  async function getRemoteRealPath(invoke, paneId, path) {
    return invoke('sftp_realpath', { paneId, path });
  }

  async function getLocalPaneCwd(invoke, paneId) {
    return invoke('get_local_pane_cwd', { paneId });
  }

  async function getRemotePaneCwd(invoke, paneId) {
    return invoke('ssh_get_pane_cwd', { paneId });
  }

  async function listLocalDir(invoke, path) {
    return invoke('local_list_dir', { path });
  }

  async function listRemoteDir(invoke, paneId, path) {
    return invoke('sftp_list_dir', { paneId, path });
  }

  // One entry, or a rejection when the path does not exist. Both backends
  // return the same `FileEntry` on success and a bare String on failure
  // (remote/sftp_commands.rs: sftp_stat / local_stat), which is what makes
  // "does this file already exist?" a rejection rather than a null.
  async function statLocal(invoke, path) {
    return invoke('local_stat', { path });
  }

  async function statRemote(invoke, paneId, path) {
    return invoke('sftp_stat', { paneId, path });
  }

  async function transferDownload(invoke, paneId, remotePath, localPath) {
    return invoke('transfer_download', { paneId, remotePath, localPath });
  }

  async function transferUpload(invoke, paneId, localPath, remotePath) {
    return invoke('transfer_upload', { paneId, localPath, remotePath });
  }

  async function transferCancel(invoke, transferId) {
    return invoke('transfer_cancel', { transferId });
  }

  async function localMkdir(invoke, path) {
    return invoke('local_mkdir', { path });
  }

  async function localRename(invoke, from, to) {
    return invoke('local_rename', { from, to });
  }

  async function localRemove(invoke, path, isDir) {
    return invoke('local_remove', { path, isDir });
  }

  async function remoteMkdir(invoke, paneId, path) {
    return invoke('sftp_mkdir', { paneId, path });
  }

  async function remoteRename(invoke, paneId, from, to) {
    return invoke('sftp_rename', { paneId, from, to });
  }

  async function remoteRemove(invoke, paneId, path, isDir) {
    return invoke('sftp_remove', { paneId, path, isDir });
  }

  async function clipboardWriteText(invoke, text) {
    return invoke('clipboard_write_text', { text });
  }

  // Active SSH sessions, keyed by `"{window_label}:{pane_id}"`, each carrying
  // host/user/port. The files panel uses these to name the host a remote file
  // was opened from — the pane objects themselves carry no host identity.
  async function getSessions(invoke) {
    const sessions = await invoke('remote_get_sessions');
    return Array.isArray(sessions) ? sessions : [];
  }

  // THE identity of a remote host, for editing purposes. Pure — no invoke.
  //
  // This lives here, in the layer both callers already go through, because it
  // is not a display string: `editor_temp_path(host_label, remote_path)`
  // (crates/termlab_tauri/src/editor_fs.rs) hashes it into the temp path that
  // the editor then treats as the file's identity — that path is what
  // `focusExistingEditor` matches on to decide "this file is already open".
  // Two spellings of the same host therefore mean two temp paths, two editor
  // tabs on the same bytes, and a last-save-wins data loss.
  //
  // Its two callers are `remoteHostLabel` in panels/files-panel.js (open from
  // the files tree) and `buildScopes` in features/editor/file-dialog-view.js
  // (open from the ⌘O chooser). They used to hold byte-identical private copies;
  // nothing connected them, so editing one would have silently split every
  // remote file across two tabs. Callers that want a disambiguated *display*
  // string (e.g. two panes on one host in the chooser's scope bar) must
  // decorate the value this returns — never change what it returns.
  //
  // `paneId` supplies the fallback: it keeps two panes in *this* window apart
  // when the session lookup fails. It is not a guarantee across windows —
  // pane ids are allocated per window, so window A's pane 3 and window B's
  // pane 3 both fall back to "pane-3". Only a lookup that succeeds removes
  // that; the fallback narrows it.
  function sessionHostLabel(session, paneId) {
    const fallback = `pane-${paneId}`;
    if (!session || !session.host) return fallback;
    const port = Number(session.port);
    const host = port && port !== 22 ? `${session.host}:${port}` : String(session.host);
    return session.user ? `${session.user}@${host}` : host;
  }

  async function getCurrentWindowLabel(invoke) {
    return invoke('current_window_label');
  }

  // Configured hosts, tree-shaped: folders (each carrying its own entries),
  // ungrouped entries, and ssh_config-derived entries. The host dropdown
  // flattens this with folder prefixes; see files-panel.js's
  // buildConfiguredHostOptions.
  async function getServers(invoke) {
    const data = await invoke('remote_get_servers');
    return data && typeof data === 'object'
      ? data
      : { folders: [], ungrouped: [], ssh_config: [] };
  }

  // Connect the SFTP panel directly to a configured host, no user
  // interaction (crates/termlab_tauri/src/remote/detached_commands.rs's
  // sftp_connect_host). Resolves to a ConnectedSession on success; rejects
  // with a typed SftpConnectError (frontend/types/SftpConnectError.ts) on
  // anything the caller must decide how to handle — including
  // `{kind: 'connectInProgress'}`, which is not a failure, just a
  // retry-after-current signal (see files-panel.js's connectToHost).
  async function connectHost(invoke, serverEntryId) {
    return invoke('sftp_connect_host', { serverEntryId });
  }

  // Retry a detached connect with a password the user just typed (Task 4's
  // auth dialog chain, features/files/connect-auth.js). `saveToVault` mirrors
  // the "Save to vault" checkbox: true also links the server entry to a new
  // vault account (detached_commands.rs's sftp_connect_host_with_password
  // doc comment), so a host connected this way authenticates silently next
  // time. Same resolve/reject shape as connectHost above.
  async function connectHostWithPassword(invoke, serverEntryId, password, saveToVault) {
    return invoke('sftp_connect_host_with_password', { serverEntryId, password, saveToVault });
  }

  // Tear down a detached (panel-only, not terminal-owned) session.
  async function disconnectSession(invoke, sessionKey) {
    return invoke('sftp_disconnect', { sessionKey });
  }

  global.termlabFilesFeatureDataService = {
    getAllSettings,
    getHomeDir,
    getRemoteRealPath,
    getLocalPaneCwd,
    getRemotePaneCwd,
    listLocalDir,
    listRemoteDir,
    statLocal,
    statRemote,
    transferDownload,
    transferUpload,
    transferCancel,
    localMkdir,
    localRename,
    localRemove,
    remoteMkdir,
    remoteRename,
    remoteRemove,
    clipboardWriteText,
    getSessions,
    sessionHostLabel,
    getCurrentWindowLabel,
    getServers,
    connectHost,
    connectHostWithPassword,
    disconnectSession,
  };
})(window);

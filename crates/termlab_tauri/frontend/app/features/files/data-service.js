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

  global.termlabFilesFeatureDataService = {
    getAllSettings,
    getHomeDir,
    getRemoteRealPath,
    getLocalPaneCwd,
    getRemotePaneCwd,
    listLocalDir,
    listRemoteDir,
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
  };
})(window);

(function initConchSshActions(global) {
  'use strict';

  async function setFolderExpanded(invoke, folderId, expanded) {
    return invoke('remote_set_folder_expanded', { folderId, expanded });
  }

  async function duplicateServer(invoke, serverId) {
    return invoke('remote_duplicate_server', { serverId });
  }

  async function deleteServer(invoke, serverId) {
    return invoke('remote_delete_server', { serverId });
  }

  async function deleteFolder(invoke, folderId) {
    return invoke('remote_delete_folder', { folderId });
  }

  async function startTunnel(invoke, tunnelId) {
    return invoke('tunnel_start', { tunnelId });
  }

  async function stopTunnel(invoke, tunnelId) {
    return invoke('tunnel_stop', { tunnelId });
  }

  async function deleteTunnel(invoke, tunnelId) {
    return invoke('tunnel_delete', { tunnelId });
  }

  global.conchSshActions = {
    setFolderExpanded,
    duplicateServer,
    deleteServer,
    deleteFolder,
    startTunnel,
    stopTunnel,
    deleteTunnel,
  };
})(window);

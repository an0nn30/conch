(function initConchVaultFeatureDataService(global) {
  'use strict';

  async function getStatus(invoke) {
    return invoke('vault_status');
  }

  async function createVault(invoke, password) {
    return invoke('vault_create', { request: { password } });
  }

  async function unlockVault(invoke, password) {
    return invoke('vault_unlock', { request: { password } });
  }

  async function lockVault(invoke) {
    return invoke('vault_lock');
  }

  async function listAccounts(invoke) {
    const accounts = await invoke('vault_list_accounts');
    return Array.isArray(accounts) ? accounts : [];
  }

  async function getAccount(invoke, id) {
    return invoke('vault_get_account', { id });
  }

  async function addAccount(invoke, request) {
    return invoke('vault_add_account', { request });
  }

  async function updateAccount(invoke, request) {
    return invoke('vault_update_account', { request });
  }

  async function deleteAccount(invoke, id) {
    return invoke('vault_delete_account', { id });
  }

  async function listKeys(invoke) {
    const keys = await invoke('vault_list_keys');
    return Array.isArray(keys) ? keys : [];
  }

  async function deleteKey(invoke, id) {
    return invoke('vault_delete_key', { id });
  }

  async function getSettings(invoke) {
    return invoke('vault_get_settings');
  }

  async function updateSettings(invoke, settings) {
    return invoke('vault_update_settings', { settings });
  }

  async function pickKeyFile(invoke) {
    return invoke('vault_pick_key_file');
  }

  global.conchVaultFeatureDataService = {
    getStatus,
    createVault,
    unlockVault,
    lockVault,
    listAccounts,
    getAccount,
    addAccount,
    updateAccount,
    deleteAccount,
    listKeys,
    deleteKey,
    getSettings,
    updateSettings,
    pickKeyFile,
  };
})(window);

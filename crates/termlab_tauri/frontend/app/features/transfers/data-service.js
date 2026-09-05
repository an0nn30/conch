(function initTermLabTransferDataService(global) {
  'use strict';

  function snapshot(invoke) {
    return invoke('transfer_queue_snapshot');
  }

  function pause(invoke, transferId) {
    return invoke('transfer_pause', { transferId });
  }

  function resume(invoke, transferId) {
    return invoke('transfer_resume', { transferId });
  }

  function cancel(invoke, transferId) {
    return invoke('transfer_cancel', { transferId });
  }

  function cancelBatch(invoke, batchId) {
    return invoke('transfer_cancel_batch', { batchId });
  }

  function cancelAll(invoke) {
    return invoke('transfer_cancel_all');
  }

  function retry(invoke, transferId) {
    return invoke('transfer_retry', { transferId });
  }

  function resolve(invoke, transferId, resolution) {
    return invoke('transfer_resolve', { transferId, resolution });
  }

  function pauseAll(invoke) {
    return invoke('transfer_pause_all');
  }

  function resumeAll(invoke) {
    return invoke('transfer_resume_all');
  }

  function reorder(invoke, transferId, before) {
    return invoke('transfer_reorder', { transferId, before });
  }

  function setPriority(invoke, transferId, priority) {
    return invoke('transfer_set_priority', { transferId, priority });
  }

  function clearCompleted(invoke) {
    return invoke('transfer_clear_completed');
  }

  function updateSettings(invoke, settings) {
    return invoke('transfer_update_settings', { settings });
  }

  global.termlabTransferDataService = {
    snapshot,
    pause,
    resume,
    cancel,
    cancelBatch,
    cancelAll,
    retry,
    resolve,
    pauseAll,
    resumeAll,
    reorder,
    setPriority,
    clearCompleted,
    updateSettings,
  };
})(window);

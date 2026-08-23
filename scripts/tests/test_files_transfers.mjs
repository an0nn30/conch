// Run: node scripts/tests/test_files_transfers.mjs

import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(here, '..', '..');
const source = fs.readFileSync(
  path.join(repoRoot, 'crates/termlab_tauri/frontend/app/features/files/transfers.js'),
  'utf8',
);

const successToasts = [];
const sandbox = {
  window: {
    toast: {
      success(message) { successToasts.push(message); },
    },
  },
  setTimeout,
  clearTimeout,
};
sandbox.window.window = sandbox.window;
vm.createContext(sandbox);
vm.runInContext(source, sandbox, { filename: 'transfers.js' });

const localPane = { transferStatus: {} };
const remotePane = { transferStatus: {} };
const refreshed = [];
const controller = sandbox.window.termlabFilesTransfers.createController({
  localPane,
  remotePane,
  toast: sandbox.window.toast,
  loadEntries(pane) { refreshed.push(pane); },
});

controller.handleTransferProgress({
  payload: {
    transfer_id: 'upload-1',
    kind: 'upload',
    status: 'completed',
    bytes_transferred: 8,
    total_bytes: 8,
    file_name: 'current.txt',
    error: null,
  },
});

assert.deepEqual(
  refreshed,
  [localPane, remotePane],
  'upload completion refreshes both source and destination listings',
);
assert.equal(successToasts.length, 1);

console.log('files transfer completion tests passed');

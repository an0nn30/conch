// Run: node scripts/tests/test_auth_prompts.mjs
//
// SSH auth prompts (host key verification, password entry) block the backend
// SSH handler on a oneshot reply, so their listeners must exist from window
// boot — not only once the Hosts panel happens to render. These tests load
// the real auth-prompts IIFE in a VM and drive its window-wide install.

import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';

const FRONTEND = path.resolve(import.meta.dirname, '../../crates/termlab_tauri/frontend');
const AUTH_PROMPTS_PATH = path.join(FRONTEND, 'app/features/ssh/auth-prompts.js');

function loadHarness() {
  const dialogs = [];
  const invokes = [];
  const listeners = new Map();

  const sandbox = {
    console,
    Promise,
    Map,
    Set,
    setTimeout: () => 0,
    tlDialog: {
      open(options) {
        const record = { options, closed: false };
        const bodyEl = { innerHTML: '', querySelector: () => null };
        const panelEl = { addEventListener: () => {} };
        let closed = false;
        record.handle = {
          el: { querySelector: () => null },
          close(result) {
            if (closed) return;
            closed = true;
            record.closed = true;
            if (typeof options.onClose === 'function') options.onClose(result);
          },
        };
        if (typeof options.body === 'function') options.body(bodyEl);
        if (typeof options.onOpen === 'function') options.onOpen(panelEl);
        dialogs.push(record);
        return record.handle;
      },
    },
  };
  sandbox.window = sandbox;
  vm.createContext(sandbox);
  vm.runInContext(fs.readFileSync(AUTH_PROMPTS_PATH, 'utf8'), sandbox, { filename: AUTH_PROMPTS_PATH });
  const prompts = sandbox.termlabSshAuthPrompts;
  assert.ok(prompts, 'auth prompts IIFE must expose window.termlabSshAuthPrompts');

  const invoke = (command, args) => {
    invokes.push({ command, args });
    return Promise.resolve();
  };
  const listen = (name, handler) => {
    if (!listeners.has(name)) listeners.set(name, []);
    listeners.get(name).push(handler);
    return Promise.resolve(() => {});
  };

  return {
    prompts,
    dialogs,
    invokes,
    install: () => prompts.install({ listen, invoke, esc: (value) => String(value == null ? '' : value) }),
    emit(name, payload) {
      for (const handler of listeners.get(name) || []) handler({ payload });
    },
    listenerCount: (name) => (listeners.get(name) || []).length,
  };
}

// Install wires all three events, exactly once per window.
{
  const harness = loadHarness();
  harness.install();
  assert.equal(harness.listenerCount('ssh-host-key-prompt'), 1);
  assert.equal(harness.listenerCount('ssh-password-prompt'), 1);
  assert.equal(harness.listenerCount('ssh-auth-prompt-resolved'), 1);
  harness.install();
  assert.equal(harness.listenerCount('ssh-host-key-prompt'), 1,
    'a second install must not double the dialogs');
}

// A host key prompt opens its dialog with no panel involved, and accepting
// responds through the auth command.
{
  const harness = loadHarness();
  harness.install();
  harness.emit('ssh-host-key-prompt', { prompt_id: 'p1', message: 'authenticity', detail: 'SHA256:fp' });
  assert.equal(harness.dialogs.length, 1, 'the prompt renders from the boot-time listener alone');
  assert.equal(harness.dialogs[0].options.title, 'SSH Host Key Verification');
  const accept = harness.dialogs[0].options.buttons.find((button) => button.label === 'Accept & Save');
  accept.onSelect();
  assert.deepEqual(JSON.parse(JSON.stringify(harness.invokes)), [
    { command: 'auth_respond_host_key', args: { promptId: 'p1', accepted: true } },
  ]);
  assert.ok(harness.dialogs[0].closed);
}

// Dismissing the dialog (Escape/backdrop) rejects the key.
{
  const harness = loadHarness();
  harness.install();
  harness.emit('ssh-host-key-prompt', { prompt_id: 'p2', message: 'm', detail: 'd' });
  harness.dialogs[0].handle.close('cancel');
  assert.deepEqual(JSON.parse(JSON.stringify(harness.invokes)), [
    { command: 'auth_respond_host_key', args: { promptId: 'p2', accepted: false } },
  ]);
}

// Prompts are broadcast to every window; when another window answers first,
// the backend's resolved broadcast closes this copy WITHOUT sending a
// second (already-consumed) response.
{
  const harness = loadHarness();
  harness.install();
  harness.emit('ssh-host-key-prompt', { prompt_id: 'p3', message: 'm', detail: 'd' });
  harness.emit('ssh-auth-prompt-resolved', { prompt_id: 'p3' });
  assert.ok(harness.dialogs[0].closed, 'the answered prompt closes everywhere');
  assert.deepEqual(JSON.parse(JSON.stringify(harness.invokes)), [], 'closing a resolved prompt sends no response');
}

// Password prompts ride the same wiring; cancel responds with a null
// password so the backend unblocks instead of hanging.
{
  const harness = loadHarness();
  harness.install();
  harness.emit('ssh-password-prompt', { prompt_id: 'p4', message: 'Enter password' });
  assert.equal(harness.dialogs.length, 1);
  const cancel = harness.dialogs[0].options.buttons.find((button) => button.label === 'Cancel');
  cancel.onSelect();
  assert.deepEqual(JSON.parse(JSON.stringify(harness.invokes)), [
    { command: 'auth_respond_password', args: { promptId: 'p4', password: null } },
  ]);
}

console.log('ssh auth prompts: all assertions passed');

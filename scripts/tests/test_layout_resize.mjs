// Run: node scripts/tests/test_layout_resize.mjs
//
// fitAndResizePane is the one place window/pane geometry reaches both xterm
// (fit) and the PTY (resize command). These tests pin its hardening: junk
// dimensions are skipped, duplicate sizes are deduped, and a throw from
// inside xterm's fit (the resize/reflow race that used to wedge the terminal
// until app restart) recovers via reset + refit + a PTY repaint nudge.

import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';

const FRONTEND = path.resolve(import.meta.dirname, '../../crates/termlab_tauri/frontend');
const LAYOUT_RUNTIME_PATH = path.join(FRONTEND, 'app/layout-runtime.js');

function loadHarness() {
  const invokes = [];
  const timers = [];
  const sandbox = {
    console,
    Promise,
    Map,
    Set,
    Number,
    Math,
    setTimeout: (callback, delay) => {
      timers.push({ callback, delay });
      return timers.length;
    },
    clearTimeout: () => {},
  };
  sandbox.window = sandbox;
  vm.createContext(sandbox);
  vm.runInContext(fs.readFileSync(LAYOUT_RUNTIME_PATH, 'utf8'), sandbox, { filename: LAYOUT_RUNTIME_PATH });
  const runtime = sandbox.termlabLayoutRuntime.create({
    invoke: (command, args) => {
      invokes.push({ command, args });
      return Promise.resolve();
    },
    getPanes: () => new Map(),
    allPanesInTab: () => [],
    getCurrentTab: () => null,
    renderTree: () => {},
  });
  return {
    runtime,
    invokes,
    flushNotifications() {
      for (const timer of timers.splice(0)) timer.callback();
    },
  };
}

function pane(dims, options = {}) {
  const record = {
    paneId: 7,
    type: options.type || 'pty',
    spawned: true,
    lastCols: null,
    lastRows: null,
    fitCalls: 0,
    resetCalls: 0,
    term: {
      reset() { record.resetCalls += 1; },
    },
    fitAddon: {
      proposeDimensions: () => (typeof dims === 'function' ? dims() : dims),
      fit() {
        record.fitCalls += 1;
        if (options.fitThrows && record.fitCalls <= options.fitThrows) {
          throw new TypeError("undefined is not an object (evaluating 'isWrapped')");
        }
      },
    },
  };
  return record;
}

// Junk dimensions never reach xterm or the PTY.
{
  const harness = loadHarness();
  for (const dims of [null, { cols: 0, rows: 24 }, { cols: NaN, rows: 24 }, { cols: 80, rows: 1 }]) {
    const target = pane(dims);
    harness.runtime.fitAndResizePane(target);
    assert.equal(target.fitCalls, 0, `dims ${JSON.stringify(dims)} must not fit`);
  }
  harness.flushNotifications();
  assert.deepEqual(harness.invokes, [], 'no PTY resize for junk dimensions');
}

// A clean fit notifies the PTY once on the trailing timer, and the same
// dimensions do not fit or notify twice.
{
  const harness = loadHarness();
  const target = pane({ cols: 120, rows: 40 });
  harness.runtime.fitAndResizePane(target);
  harness.runtime.fitAndResizePane(target);
  assert.equal(target.fitCalls, 1, 'unchanged dimensions are deduped');
  harness.flushNotifications();
  assert.deepEqual(JSON.parse(JSON.stringify(harness.invokes)), [
    { command: 'resize_pty', args: { paneId: 7, cols: 120, rows: 40 } },
  ]);
}

// SSH panes notify through ssh_resize.
{
  const harness = loadHarness();
  const target = pane({ cols: 100, rows: 30 }, { type: 'ssh' });
  harness.runtime.fitAndResizePane(target);
  harness.flushNotifications();
  assert.equal(harness.invokes[0].command, 'ssh_resize');
}

// A throw inside fit recovers: reset, refit, immediate one-column PTY nudge,
// then the real size on the trailing notify — a guaranteed size change so a
// fullscreen app repaints the reset screen.
{
  const harness = loadHarness();
  const target = pane({ cols: 120, rows: 40 }, { fitThrows: 1 });
  harness.runtime.fitAndResizePane(target);
  assert.equal(target.resetCalls, 1, 'a failed fit resets the terminal');
  assert.equal(target.fitCalls, 2, 'fit is retried after the reset');
  assert.deepEqual(JSON.parse(JSON.stringify(harness.invokes)), [
    { command: 'resize_pty', args: { paneId: 7, cols: 119, rows: 40 } },
  ], 'the nudge lands immediately');
  harness.flushNotifications();
  assert.deepEqual(JSON.parse(JSON.stringify(harness.invokes.at(-1))), {
    command: 'resize_pty', args: { paneId: 7, cols: 120, rows: 40 },
  }, 'the settled size follows');
}

// If even the reset path throws, nothing escapes, and the pane stays
// retryable at the same dimensions instead of being deduped into a wedge.
{
  const harness = loadHarness();
  const target = pane({ cols: 120, rows: 40 }, { fitThrows: 2 });
  assert.doesNotThrow(() => harness.runtime.fitAndResizePane(target));
  assert.equal(target.lastCols, null, 'failed recovery clears the dedupe size');
  harness.flushNotifications();
  assert.deepEqual(harness.invokes, [], 'no PTY notify for a failed fit');

  harness.runtime.fitAndResizePane(target);
  assert.equal(target.fitCalls, 3, 'the next attempt at the same size still fits');
}

console.log('layout resize hardening: all assertions passed');

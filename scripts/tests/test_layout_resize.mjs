// Run: node scripts/tests/test_layout_resize.mjs
//
// fitAndResizePane is the one place window/pane geometry reaches both xterm
// (fit) and the PTY (resize command). These tests pin its hardening: junk
// dimensions are skipped, sizes are deduped against LIVE state (xterm's real
// grid and the last size actually sent to the PTY — never a private cache
// that can go stale and strand tmux at the wrong size), a trailing settle
// pass re-verifies after every applied change, and a throw from inside
// xterm's fit (the resize/reflow race that used to wedge the terminal until
// app restart) recovers via reset + refit + a PTY repaint nudge.

import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';

const FRONTEND = path.resolve(import.meta.dirname, '../../crates/termlab_tauri/frontend');
const LAYOUT_RUNTIME_PATH = path.join(FRONTEND, 'app/layout-runtime.js');

function loadHarness() {
  const invokes = [];
  const timers = [];
  let timerId = 0;
  const sandbox = {
    console,
    Promise,
    Map,
    Set,
    Number,
    Math,
    setTimeout: (callback, delay) => {
      timerId += 1;
      timers.push({ id: timerId, callback, delay });
      return timerId;
    },
    clearTimeout: (id) => {
      const index = timers.findIndex((timer) => timer.id === id);
      if (index >= 0) timers.splice(index, 1);
    },
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
    timers,
    flushTimers(predicate) {
      const due = timers.filter((timer) => (predicate ? predicate(timer) : true));
      for (const timer of due) {
        const index = timers.indexOf(timer);
        if (index >= 0) timers.splice(index, 1);
        timer.callback();
      }
    },
    flushNotifications() {
      this.flushTimers((timer) => timer.delay === 60);
    },
    flushSettle() {
      this.flushTimers((timer) => timer.delay === 250);
    },
  };
}

function pane(dims, options = {}) {
  const record = {
    paneId: 7,
    type: options.type || 'pty',
    spawned: true,
    lastCols: 0,
    lastRows: 0,
    fitCalls: 0,
    resetCalls: 0,
    settleTimer: null,
    term: {
      cols: options.termCols || 0,
      rows: options.termRows || 0,
      reset() { record.resetCalls += 1; },
    },
    fitAddon: {
      proposeDimensions: () => (typeof dims === 'function' ? dims() : dims),
      fit() {
        record.fitCalls += 1;
        if (options.fitThrows && record.fitCalls <= options.fitThrows) {
          throw new TypeError("undefined is not an object (evaluating 'isWrapped')");
        }
        // A successful fit resizes xterm's real grid, like the FitAddon does.
        const proposed = typeof dims === 'function' ? dims() : dims;
        record.term.cols = Math.floor(proposed.cols);
        record.term.rows = Math.floor(proposed.rows);
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
  harness.flushTimers();
  assert.deepEqual(harness.invokes, [], 'no PTY resize for junk dimensions');
}

// A clean fit notifies the PTY once on the trailing timer, updates the
// informational size the tab title reads, and a consistent pane does not fit
// or notify twice.
{
  const harness = loadHarness();
  const target = pane({ cols: 120, rows: 40 });
  harness.runtime.fitAndResizePane(target);
  harness.flushNotifications();
  harness.runtime.fitAndResizePane(target);
  assert.equal(target.fitCalls, 1, 'a consistent pane is deduped');
  assert.equal(target.lastCols, 120, 'tab-title size stays fresh');
  assert.deepEqual(JSON.parse(JSON.stringify(harness.invokes)), [
    { command: 'resize_pty', args: { paneId: 7, cols: 120, rows: 40 } },
  ]);
}

// The dedupe reads live state: when xterm's real grid drifts from the
// settled layout (a mid-drag fit measured a transient size, or a direct
// fit() elsewhere resized the terminal), the next pass refits and re-notifies
// even though this module saw these dimensions before. This is the tmux
// stuck-until-you-nudge-the-window bug.
{
  const harness = loadHarness();
  const target = pane({ cols: 120, rows: 40 });
  harness.runtime.fitAndResizePane(target);
  harness.flushNotifications();
  target.term.cols = 100; // drifted behind this module's back

  harness.runtime.fitAndResizePane(target);
  assert.equal(target.fitCalls, 2, 'a drifted grid is refit');
  harness.flushNotifications();
  assert.deepEqual(JSON.parse(JSON.stringify(harness.invokes.at(-1))), {
    command: 'resize_pty', args: { paneId: 7, cols: 120, rows: 40 },
  }, 'the PTY is re-notified the settled size');
}

// Every applied change arms a trailing settle verify. When the layout truly
// settled it is a no-op; when the fit had measured a transient size, the
// settle pass applies the final one without any further window nudging.
{
  let current = { cols: 110, rows: 38 }; // mid-drag measurement
  const harness = loadHarness();
  const target = pane(() => current);
  harness.runtime.fitAndResizePane(target);
  harness.flushNotifications();
  assert.equal(harness.invokes.at(-1).args.cols, 110);

  current = { cols: 120, rows: 40 }; // the drag settled after the last event
  harness.flushSettle();
  assert.equal(target.fitCalls, 2, 'the settle pass catches the final size');
  harness.flushNotifications();
  assert.deepEqual(JSON.parse(JSON.stringify(harness.invokes.at(-1))), {
    command: 'resize_pty', args: { paneId: 7, cols: 120, rows: 40 },
  });

  // The settle pass converges: a consistent pane re-arms nothing.
  harness.flushSettle();
  assert.equal(target.fitCalls, 2);
  assert.equal(harness.timers.length, 0, 'no timers left once consistent');
}

// SSH panes notify through ssh_resize.
{
  const harness = loadHarness();
  const target = pane({ cols: 100, rows: 30 }, { type: 'ssh' });
  harness.runtime.fitAndResizePane(target);
  harness.flushNotifications();
  assert.equal(harness.invokes[0].command, 'ssh_resize');
}

// A settle verify racing a closed pane (proposeDimensions throws on a
// disposed addon) stays silent.
{
  const harness = loadHarness();
  const target = pane(() => { throw new Error('addon disposed'); });
  assert.doesNotThrow(() => harness.runtime.fitAndResizePane(target));
  assert.equal(target.fitCalls, 0);
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
// retryable at the same dimensions: the live-state dedupe sees the grid was
// never actually resized.
{
  const harness = loadHarness();
  const target = pane({ cols: 120, rows: 40 }, { fitThrows: 2 });
  assert.doesNotThrow(() => harness.runtime.fitAndResizePane(target));
  harness.flushTimers();
  assert.deepEqual(harness.invokes, [], 'no PTY notify for a failed fit');

  harness.runtime.fitAndResizePane(target);
  assert.equal(target.fitCalls, 3, 'the next attempt at the same size still fits');
}

console.log('layout resize hardening: all assertions passed');

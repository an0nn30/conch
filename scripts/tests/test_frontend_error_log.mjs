// Run: node scripts/tests/test_frontend_error_log.mjs
//
// features/diagnostics/error-log.js: uncaught frontend errors on their way to
// ~/.config/termlab/logs/frontend.log.
//
// Why it exists: the status banner startup-runtime.js paints shows the MESSAGE
// and nothing else, and disappears when dismissed. The report that started
// this was one line — "Frontend error: TypeError: undefined is not an object
// (evaluating 'n.isText')" — with no stack and no note of which document was
// open, and the diagnostic log added in 141c402 recorded navigation but not
// errors. Now the same two events also reach the file, with the stack and the
// focused pane.
//
// What this pins:
//   - the record's shape, which is the contract a bug report is read against;
//   - the stack is trimmed, because diag_log.rs truncates at 2000 characters
//     and a vendor stack would otherwise push the message out of the record;
//   - both event shapes are handled, including the ones that carry no Error at
//     all (a cross-origin `error`, a rejection with a plain value);
//   - nothing here can throw, in any window, with or without a diagnostic log;
//   - the banner is unchanged and installation happens exactly once;
//   - startup-runtime installs it, and index.html loads it after diag-log.js.
import assert from 'node:assert';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';

const ROOT = path.resolve(import.meta.dirname, '../../crates/termlab_tauri/frontend');
const APP = path.join(ROOT, 'app');
const ERROR_LOG = path.join(APP, 'features/diagnostics/error-log.js');
const DIAG_LOG = path.join(APP, 'features/diagnostics/diag-log.js');
const STARTUP_RUNTIME = path.join(APP, 'startup-runtime.js');
const INDEX_HTML = path.join(ROOT, 'index.html');

const results = [];
function check(name, fn) { results.push({ name, fn }); }

// Every harness records what reached termlabDiag rather than an IPC call:
// "which record, with what text" IS this module's contract.
function load(files, extra) {
  const sandbox = {
    console, setTimeout, clearTimeout, Promise, Map, Set, WeakMap, Array, Object, JSON, Number, String,
  };
  sandbox.window = sandbox;
  const logged = [];
  sandbox.termlabDiag = {
    log: (category, message, level) => { logged.push({ category, message, level }); },
  };
  const listeners = new Map();
  sandbox.addEventListener = (type, handler) => {
    if (!listeners.has(type)) listeners.set(type, []);
    listeners.get(type).push(handler);
  };
  sandbox.removeEventListener = (type, handler) => {
    const list = listeners.get(type) || [];
    const at = list.indexOf(handler);
    if (at >= 0) list.splice(at, 1);
  };
  Object.assign(sandbox, extra || {});
  vm.createContext(sandbox);
  for (const file of files) {
    vm.runInContext(fs.readFileSync(file, 'utf8'), sandbox, { filename: file });
  }
  const fire = (type, event) => {
    for (const handler of (listeners.get(type) || []).slice()) handler(event);
  };
  return { sandbox, logged, listeners, fire, log: sandbox.termlabErrorLog };
}

// --- the record's shape -------------------------------------------------------

check('a record names the message, where it happened and the stack', () => {
  const { log } = load([ERROR_LOG]);
  const line = log.describe({
    kind: '',
    message: "TypeError: undefined is not an object (evaluating 'n.isText')",
    stack: 'at InlineCoordsScan.scanTile (codemirror.js:8:5051)\nat HoverPlugin.startHover (codemirror.js:11:16063)',
    context: 'pane=2 kind=editor path=/repo/src/quests/mod.rs',
  });
  assert.match(line, /^TypeError: undefined is not an object/);
  assert.match(line, /at pane=2 kind=editor path=\/repo\/src\/quests\/mod\.rs/);
  assert.match(line, /stack: at InlineCoordsScan\.scanTile/);
  assert.match(line, /HoverPlugin\.startHover/, 'more than the innermost frame');
  assert.ok(!line.includes('\n'), 'one record is one line — the log is line-oriented');
});

check('the fields that are absent are omitted, not spelled as undefined', () => {
  const { log } = load([ERROR_LOG]);
  const line = log.describe({ kind: '', message: 'Script error.' });
  assert.strictEqual(line, 'Script error.');
  assert.ok(!line.includes('stack'), 'no empty stack field');
  assert.ok(!line.includes('undefined'));
});

check('an unprintable message still produces a record', () => {
  const { log } = load([ERROR_LOG]);
  const hostile = { toString() { throw new Error('nope'); } };
  assert.match(log.describe({ kind: '', message: hostile }), /unprintable/);
});

// --- the stack ------------------------------------------------------------------

check('the stack is capped at the innermost frames', () => {
  const { log } = load([ERROR_LOG]);
  const frames = Array.from({ length: 40 }, (_, i) => `    at frame_${i} (bundle.js:1:${i})`).join('\n');
  const formatted = log.formatStack(frames);
  assert.ok(formatted.includes('frame_0'), 'the throw site is kept');
  assert.ok(
    !formatted.includes(`frame_${log.MAX_STACK_FRAMES}`),
    'the outer bootstrap frames are dropped — diag_log.rs truncates at 2000 chars',
  );
  assert.ok(!formatted.includes('\n'), 'flattened onto one line');
});

check('one enormous minified frame cannot eat the whole record either', () => {
  const { log } = load([ERROR_LOG]);
  const formatted = log.formatStack(`at x (${'y'.repeat(9000)})`);
  assert.ok(formatted.length <= log.MAX_STACK_CHARS + 1, 'hard character cap on top of the frame cap');
});

check('no stack at all is reported as no stack', () => {
  const { log } = load([ERROR_LOG]);
  assert.strictEqual(log.formatStack(undefined), '');
  assert.strictEqual(log.formatStack(''), '');
  assert.strictEqual(log.formatStack(42), '');
  assert.strictEqual(log.formatStack('\n\n  \n'), '', 'blank frames are not a stack');
});

// --- the two event shapes ---------------------------------------------------------

check('a window error event contributes its message and its error stack', () => {
  const { log } = load([ERROR_LOG]);
  const parts = log.fromErrorEvent({
    message: "Uncaught TypeError: undefined is not an object (evaluating 'n.isText')",
    error: new TypeError('boom'),
  });
  assert.strictEqual(parts.kind, '', 'no prefix — the record\'s category already says "error"');
  assert.match(parts.message, /n\.isText/);
  assert.match(parts.stack, /TypeError/);
});

check('a cross-origin error event — message, no Error object — still records', () => {
  const { log } = load([ERROR_LOG]);
  const parts = log.fromErrorEvent({ message: 'Script error.' });
  assert.strictEqual(parts.message, 'Script error.');
  assert.strictEqual(parts.stack, '', 'and does not invent one');
});

check('a rejection with an Error reads its message and stack', () => {
  const { log } = load([ERROR_LOG]);
  const parts = log.fromRejectionEvent({ reason: new Error('open document failed') });
  assert.strictEqual(parts.kind, 'unhandled rejection');
  assert.strictEqual(parts.message, 'open document failed');
  assert.match(parts.stack, /open document failed/);
});

check('a rejection with a plain value is recorded as that value', () => {
  const { log } = load([ERROR_LOG]);
  assert.strictEqual(log.fromRejectionEvent({ reason: 'nope' }).message, 'nope');
  assert.strictEqual(log.fromRejectionEvent({ reason: undefined }).message, 'undefined');
  assert.strictEqual(log.fromRejectionEvent({ reason: null }).message, 'null');
  assert.strictEqual(log.fromRejectionEvent({}).message, 'undefined');
  assert.strictEqual(log.fromRejectionEvent(null).message, 'undefined');
});

// --- where the app was --------------------------------------------------------------

check('the record names the focused pane, so the document is in the report', () => {
  const { log } = load([ERROR_LOG], {
    __termlabPaneAccess: {
      currentPane: () => ({ paneId: 3, kind: 'editor', filePath: '/repo/src/quests/mod.rs', view: {} }),
    },
  });
  assert.strictEqual(log.describeContext(), 'pane=3 kind=editor path=/repo/src/quests/mod.rs');
});

check('a pane with no view says so — that is the state this class of bug lives in', () => {
  const { log } = load([ERROR_LOG], {
    __termlabPaneAccess: { currentPane: () => ({ paneId: 1, kind: 'terminal', filePath: null }) },
  });
  assert.strictEqual(log.describeContext(), 'pane=1 kind=terminal path=none noView');
});

check('a window with no panes, and a throwing pane access, both stay quiet', () => {
  assert.strictEqual(load([ERROR_LOG]).log.describeContext(), '', 'no pane access at all');
  assert.strictEqual(
    load([ERROR_LOG], { __termlabPaneAccess: { currentPane: () => null } }).log.describeContext(),
    'pane=none',
  );
  assert.strictEqual(
    load([ERROR_LOG], {
      __termlabPaneAccess: { currentPane: () => { throw new Error('composing'); } },
    }).log.describeContext(),
    '',
    'a diagnostic that throws while describing a failure is worse than no diagnostic',
  );
});

// --- reaching the file ----------------------------------------------------------------

check('an installed listener writes one record at category error, level error', () => {
  const h = load([ERROR_LOG], {
    __termlabPaneAccess: {
      currentPane: () => ({ paneId: 2, kind: 'editor', filePath: '/repo/src/main.rs', view: {} }),
    },
  });
  h.log.install(h.sandbox);
  h.fire('error', { message: 'TypeError: bad', error: new TypeError('bad') });
  assert.strictEqual(h.logged.length, 1);
  assert.strictEqual(h.logged[0].category, 'error', 'its own category, so it has its own budget');
  assert.strictEqual(h.logged[0].level, 'error');
  assert.match(h.logged[0].message, /path=\/repo\/src\/main\.rs/);
});

check('a rejection reaches the file too', () => {
  const h = load([ERROR_LOG]);
  h.log.install(h.sandbox);
  h.fire('unhandledrejection', { reason: new Error('transfer stalled') });
  assert.strictEqual(h.logged.length, 1);
  assert.match(h.logged[0].message, /^unhandled rejection: transfer stalled/);
});

check('install is idempotent — one error is never two records', () => {
  const h = load([ERROR_LOG]);
  h.log.install(h.sandbox);
  h.log.install(h.sandbox);
  assert.strictEqual((h.listeners.get('error') || []).length, 1);
  h.fire('error', { message: 'once' });
  assert.strictEqual(h.logged.length, 1);
});

check('the uninstall it returns really removes both listeners', () => {
  const h = load([ERROR_LOG]);
  const uninstall = h.log.install(h.sandbox);
  uninstall();
  h.fire('error', { message: 'gone' });
  h.fire('unhandledrejection', { reason: 'gone' });
  assert.deepStrictEqual(h.logged, []);
  assert.strictEqual((h.listeners.get('error') || []).length, 0);
  assert.strictEqual((h.listeners.get('unhandledrejection') || []).length, 0);
  // And the module is installable again afterwards: uninstall clears the
  // once-only latch rather than permanently disarming the reader.
  h.log.install(h.sandbox);
  h.fire('error', { message: 'back' });
  assert.strictEqual(h.logged.length, 1);
});

check('a window with no diagnostic log at all is unharmed', () => {
  const h = load([ERROR_LOG], { termlabDiag: undefined });
  h.log.install(h.sandbox);
  assert.strictEqual(h.log.noteError({ message: 'x' }), false, 'it reports that nothing was written');
  h.fire('error', { message: 'x' });
  assert.doesNotThrow(() => h.fire('unhandledrejection', { reason: 'x' }));
});

check('a diagnostic log that throws cannot take the app down with it', () => {
  const h = load([ERROR_LOG], {
    termlabDiag: { log: () => { throw new Error('log is broken'); } },
  });
  h.log.install(h.sandbox);
  assert.strictEqual(h.log.noteError({ message: 'x' }), false);
  assert.doesNotThrow(() => h.fire('error', { message: 'x' }));
});

check('install on a target that cannot listen is a harmless no-op', () => {
  const h = load([ERROR_LOG]);
  assert.doesNotThrow(() => h.log.install({}));
});

// --- the wiring --------------------------------------------------------------------------

check('startup-runtime installs the file reader alongside the banner it does not replace', () => {
  const source = fs.readFileSync(STARTUP_RUNTIME, 'utf8');
  assert.match(source, /termlabErrorLog[\s\S]{0,120}install/, 'initStatusController installs it');
  // The banner has to survive: it is the only feedback in the built app, and
  // the file is an ADDITIONAL reader of the same events.
  assert.match(source, /showStatus\('Frontend error: ' \+ event\.message\)/);
  assert.match(source, /showStatus\('Unhandled promise rejection: '/);
  assert.match(
    source, /typeof global\.termlabErrorLog\.install === 'function'/,
    'guarded, so a window without the module keeps exactly the banner it had',
  );
});

check('index.html loads error-log.js, after diag-log.js and before startup-runtime.js', () => {
  const html = fs.readFileSync(INDEX_HTML, 'utf8');
  const diag = html.indexOf('app/features/diagnostics/diag-log.js');
  const errorLog = html.indexOf('app/features/diagnostics/error-log.js');
  const startup = html.indexOf('app/startup-runtime.js');
  assert.ok(errorLog > 0, 'index.html must load error-log.js or every uncaught error is unrecorded');
  assert.ok(diag > 0 && diag < errorLog, 'window.termlabDiag has to exist before its caller');
  assert.ok(errorLog < startup, 'and the module has to exist before initStatusController runs');
});

check('the module writes through termlabDiag and never invokes on its own', () => {
  const source = fs.readFileSync(ERROR_LOG, 'utf8');
  // diag-log.js owns the throttle, the per-session cap and the tauriClient
  // seam. A second door to app_diag_log would bypass all three, and the
  // boundary check in scripts/check_frontend_boundaries.sh exists for that.
  for (const forbidden of ['__TAURI__', 'invoke(', 'app_diag_log']) {
    assert.ok(!source.includes(forbidden), `${forbidden} would bypass diag-log.js's throttle and cap`);
  }
  assert.ok(fs.existsSync(DIAG_LOG), 'and the module it writes through is there');
});

let failed = 0;
for (const { name, fn } of results) {
  try {
    await fn();
    console.log(`  ok   ${name}`);
  } catch (error) {
    failed += 1;
    console.log(`  FAIL ${name}`);
    console.log(`       ${error && error.message}`);
  }
}
if (failed) {
  console.log(`frontend error log: ${failed} of ${results.length} checks FAILED`);
  process.exitCode = 1;
} else {
  console.log(`frontend error log: all ${results.length} checks passed`);
}

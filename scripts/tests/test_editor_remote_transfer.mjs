// Run: node scripts/tests/test_editor_remote_transfer.mjs
//
// The SFTP side of the editor: opening a remote file, uploading it back, and
// giving up on a transfer that never finishes.
//
// The status strings below are the load-bearing part. `transfer_download` and
// `transfer_upload` are fire-and-forget — they return an id and report
// everything else through the shared 'transfer-progress' event — so the only
// thing that tells the editor a download finished is a payload whose `status`
// matches. They are the serde `snake_case` renderings of
// `termlab_remote::transfer::TransferStatus`
// (crates/termlab_remote/src/transfer.rs:33-39):
//
//     Pending | InProgress | Completed | Failed | Cancelled
//       -> 'pending' 'in_progress' 'completed' 'failed' 'cancelled'
//
// A mismatch here is invisible in review and fatal at runtime: the promise
// never settles, and opening a remote file hangs forever with no error.
import assert from 'node:assert';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';

const SERVICE_PATH = path.resolve(
  import.meta.dirname,
  '../../crates/termlab_tauri/frontend/app/features/editor/editor-service.js',
);

// Same stand-in for the real EditorView as test_editor_save_race.mjs.
function makeView(initialDoc, onDirtyChange) {
  let doc = initialDoc;
  let dirty = false;
  return {
    get state() {
      return { doc: { toString: () => doc } };
    },
    termlabResetDirty() {
      dirty = false;
      onDirtyChange(false);
    },
    isDirty: () => dirty,
    change(nextDoc) {
      if (nextDoc === doc) return;
      doc = nextDoc;
      if (dirty) return;
      dirty = true;
      onDirtyChange(true);
    },
    type(text) {
      this.change(doc + text);
    },
  };
}

// Each stub returns exactly the shape the real backend returns:
//
//   editor_can_open      Rust Result<(), String>  -> resolves null, or rejects
//                        with the bare String message (Tauri does not wrap it
//                        in an Error).
//   editor_temp_path     Rust Result<String, String> -> an absolute path,
//                        deterministic per (hostLabel, remotePath).
//   editor_read_file     Rust Result<String, String> -> the file's text, or a
//                        String rejection for a binary/oversized file.
//   editor_write_file    Rust Result<(), String>  -> resolves null.
//   editor_temp_cleanup  Rust Result<(), String>  -> resolves null.
//   transfer_download /
//   transfer_upload      Rust Result<String, String> -> a fresh transfer id,
//                        returned as soon as the task is spawned. Nothing else
//                        about the transfer comes back this way.
//   transfer_cancel      Rust bool -> resolves true.
//   listen()             Tauri event.listen -> a Promise of an unlisten fn.
function makeHarness(options = {}) {
  // setTimeout/clearTimeout are injected because the service arms a stall
  // watchdog; a vm context has no timers of its own.
  const sandbox = { console, setTimeout, clearTimeout };
  sandbox.window = sandbox;
  vm.createContext(sandbox);

  const calls = [];
  const toasts = [];
  const listeners = [];
  const started = [];
  const panes = new Map();
  const activated = [];
  let nextPaneId = 1;
  let nextTransferId = 1;

  const canOpen = options.canOpen || (() => null);
  const readFile = options.readFile || (() => 'remote contents');

  // Stands in for editor_temp_path's real (host hash)/(path hash)/basename
  // layout. Only the property the service depends on is reproduced: one path
  // per (hostLabel, remotePath) pair, and never a shared path across pairs.
  const tempPathFor = (hostLabel, remotePath) => {
    const basename = String(remotePath).split('/').filter(Boolean).pop() || 'untitled';
    const key = (s) => String(s).replace(/[^a-zA-Z0-9]/g, '_');
    return `/tmp/termlab-sftp-edits/${key(hostLabel)}/${key(remotePath)}/${basename}`;
  };

  sandbox.CM6 = {};
  sandbox.toast = {
    error: (title, body) => { toasts.push({ kind: 'error', title, body }); },
    success: (title, body) => { toasts.push({ kind: 'success', title, body }); },
    warn: (title, body) => { toasts.push({ kind: 'warn', title, body }); },
    info: (title, body) => { toasts.push({ kind: 'info', title, body }); },
  };

  sandbox.termlabServices = {
    tauriClient: {
      invoke(command, args) {
        calls.push({ command, args });
        switch (command) {
          case 'editor_can_open': {
            const rejection = canOpen(args);
            return rejection ? Promise.reject(rejection) : Promise.resolve(null);
          }
          case 'editor_temp_path':
            return Promise.resolve(tempPathFor(args.hostLabel, args.remotePath));
          case 'editor_read_file': {
            const result = readFile(args);
            return typeof result === 'string'
              ? Promise.resolve(result)
              : Promise.reject(result.rejection);
          }
          case 'editor_write_file':
          case 'editor_temp_cleanup':
            return Promise.resolve(null);
          case 'transfer_download':
          case 'transfer_upload': {
            const id = `transfer-${nextTransferId++}`;
            // Rust rejects with a bare String, e.g. from get_ssh_handle when
            // the pane's session is gone.
            if (options.failStart) return Promise.reject(options.failStart);
            const entry = { id, command, args };
            started.push(entry);
            // With deferTransferId the command reply is parked, reproducing
            // the real ordering hazard: the task is running (and can already
            // be emitting events) while the caller still does not know its id.
            if (!options.deferTransferId) return Promise.resolve(id);
            return new Promise((resolve) => { entry.release = () => resolve(id); });
          }
          case 'transfer_cancel':
            return Promise.resolve(true);
          default:
            return Promise.resolve(null);
        }
      },
      listen(eventName, handler) {
        const entry = { eventName, handler, active: true };
        listeners.push(entry);
        return Promise.resolve(() => { entry.active = false; });
      },
    },
  };

  sandbox.__termlabCreateEditorTab = (opts) => {
    const paneId = nextPaneId++;
    const pane = {
      paneId,
      tabId: paneId,
      kind: 'editor',
      filePath: opts.filePath,
      remote: opts.remote || null,
      dirty: false,
    };
    pane.view = makeView(opts.contents, (d) => { pane.dirty = d; });
    panes.set(paneId, pane);
    return pane;
  };

  sandbox.__termlabPaneAccess = {
    currentPane: () => {
      const all = [...panes.values()];
      return all.length ? all[all.length - 1] : null;
    },
    allPanes: () => panes,
    setFocusedPane: (paneId) => { activated.push(['pane', paneId]); },
    activateTab: (tabId) => { activated.push(['tab', tabId]); },
  };

  vm.runInContext(fs.readFileSync(SERVICE_PATH, 'utf8'), sandbox, { filename: SERVICE_PATH });

  return {
    service: sandbox.termlabEditorService,
    calls,
    toasts,
    started,
    panes,
    activated,
    tempPathFor,
    commandsNamed: (name) => calls.filter((c) => c.command === name),
    // Deliver a 'transfer-progress' event exactly as tauri-client would.
    emit: (progress) => {
      for (const entry of listeners) {
        if (entry.active && entry.eventName === 'transfer-progress') {
          entry.handler({ payload: progress });
        }
      }
    },
    activeListeners: () => listeners.filter((l) => l.active).length,
  };
}

const tick = () => new Promise((resolve) => setTimeout(resolve, 0));

// Every await below goes through this. The failure mode these tests exist to
// catch is a promise that never settles, and an unguarded `await` on one turns
// a regression into a wedged `node` with no message rather than a test failure.
// The loser's timer is always cleared, so a passing run exits immediately and
// no stray rejection is left unobserved.
const TIMEOUT_MS = 2000;
function settles(promise, what) {
  let timer = null;
  const bomb = new Promise((_, reject) => {
    timer = setTimeout(
      () => reject(new Error(`timed out after ${TIMEOUT_MS}ms waiting for ${what}`)),
      TIMEOUT_MS,
    );
  });
  return Promise.race([promise, bomb]).finally(() => { if (timer) clearTimeout(timer); });
}
const progress = (id, status, extra = {}) => ({
  transfer_id: id,
  kind: 'download',
  status,
  bytes_transferred: 0,
  total_bytes: 0,
  file_name: 'notes.txt',
  ...extra,
});

// --- 1. 'completed' is what opens the tab ---------------------------------
{
  const h = makeHarness();
  const opening = h.service.openRemoteFile({
    paneId: 4, remotePath: '/home/me/notes.txt', hostLabel: 'me@example.com', size: 120,
  });
  await tick();

  assert.strictEqual(h.started.length, 1, 'the download started');
  assert.strictEqual(h.started[0].command, 'transfer_download');
  // deepEqual, not deepStrictEqual: the args object is built inside the vm
  // realm, so its prototype is not this realm's Object.prototype.
  assert.deepEqual(h.started[0].args, {
    paneId: 4,
    remotePath: '/home/me/notes.txt',
    localPath: h.tempPathFor('me@example.com', '/home/me/notes.txt'),
  });

  // The two non-terminal statuses must not settle anything.
  h.emit(progress(h.started[0].id, 'pending'));
  h.emit(progress(h.started[0].id, 'in_progress'));
  await tick();
  assert.strictEqual(h.panes.size, 0, 'in_progress does not open a tab');

  // Nor does another transfer's completion.
  h.emit(progress('someone-elses-transfer', 'completed'));
  await tick();
  assert.strictEqual(h.panes.size, 0, 'another transfer id is ignored');

  h.emit(progress(h.started[0].id, 'completed'));
  await settles(opening, 'the open after a completed download');

  assert.strictEqual(h.panes.size, 1, 'completed opens exactly one tab');
  const pane = [...h.panes.values()][0];
  assert.strictEqual(pane.filePath, h.tempPathFor('me@example.com', '/home/me/notes.txt'));
  assert.deepEqual(pane.remote, {
    paneId: 4, remotePath: '/home/me/notes.txt', hostLabel: 'me@example.com',
  });
  assert.strictEqual(h.commandsNamed('editor_temp_cleanup').length, 0, 'nothing was cleaned up');
  assert.strictEqual(h.toasts.length, 0, 'a clean open is silent');
  assert.strictEqual(h.activeListeners(), 0, 'the progress listener was removed');
}

// --- 2. A terminal event that beats the transfer id still settles ----------
// The id is minted in Rust before the task is spawned and the event travels on
// a different channel from the command reply, so a small file can finish first.
{
  const h = makeHarness({ deferTransferId: true });
  const opening = h.service.openRemoteFile({
    paneId: 4, remotePath: '/etc/hosts', hostLabel: 'me@example.com', size: 200,
  });
  await tick();

  assert.strictEqual(h.started.length, 1, 'the download is running');
  assert.strictEqual(h.activeListeners(), 1, 'and the listener is already up');

  // The whole transfer finishes before the command reply arrives.
  h.emit(progress(h.started[0].id, 'completed'));
  await tick();
  assert.strictEqual(h.panes.size, 0, 'nothing can be matched yet — the id is unknown');

  h.started[0].release();
  await settles(opening, 'the open whose completion beat the command reply');
  assert.strictEqual(h.panes.size, 1, 'an early completion still opens the tab');
  assert.strictEqual(h.toasts.length, 0, 'and raises nothing');
  assert.strictEqual(h.activeListeners(), 0);
}

// --- 3. 'failed' and 'cancelled' both settle, and both clean up -----------
for (const [status, expected] of [['failed', /disk full/], ['cancelled', /Transfer cancelled/]]) {
  const h = makeHarness();
  const opening = h.service.openRemoteFile({
    paneId: 4, remotePath: '/home/me/notes.txt', hostLabel: 'me@example.com', size: 120,
  });
  await tick();
  h.emit(progress(h.started[0].id, status, { error: status === 'failed' ? 'disk full' : null }));
  await settles(opening, `the open after a ${status} download`);

  assert.strictEqual(h.panes.size, 0, `${status} opens no tab`);
  assert.strictEqual(h.toasts.length, 1, `${status} raises a toast`);
  assert.strictEqual(h.toasts[0].kind, 'error');
  assert.strictEqual(h.toasts[0].title, 'Cannot Open File');
  assert.match(h.toasts[0].body, expected);

  // A half-written download is litter, not content.
  const cleanups = h.commandsNamed('editor_temp_cleanup');
  assert.strictEqual(cleanups.length, 1, `${status} removes the temp file`);
  assert.strictEqual(
    cleanups[0].args.path,
    h.tempPathFor('me@example.com', '/home/me/notes.txt'),
  );
  assert.strictEqual(h.activeListeners(), 0, 'the progress listener was removed');
}

// --- 4. The guards reject before a single byte moves ----------------------
{
  const h = makeHarness({
    canOpen: (args) => (args.size > 5 * 1024 * 1024
      ? `"${args.name}" is 10.0 MB; the editor opens files up to 5.0 MB.`
      : null),
  });
  await settles(h.service.openRemoteFile({
    paneId: 4, remotePath: '/var/log/huge.log', hostLabel: 'me@example.com', size: 10 * 1024 * 1024,
  }), 'the rejected oversized open');

  assert.strictEqual(h.started.length, 0, 'no transfer was started');
  assert.strictEqual(h.commandsNamed('editor_temp_path').length, 0, 'and no temp path was made');
  assert.strictEqual(h.commandsNamed('editor_temp_cleanup').length, 0, 'so nothing to clean up');
  assert.strictEqual(h.panes.size, 0);
  assert.strictEqual(h.toasts[0].title, 'Cannot Open File');
  assert.match(h.toasts[0].body, /10\.0 MB/, 'the toast names the size');
}

// --- 5. A binary file that only reveals itself after the download ---------
{
  const h = makeHarness({
    readFile: () => ({ rejection: '"notes.txt" looks like a binary file.' }),
  });
  const opening = h.service.openRemoteFile({
    paneId: 4, remotePath: '/home/me/notes.txt', hostLabel: 'me@example.com', size: 120,
  });
  await tick();
  h.emit(progress(h.started[0].id, 'completed'));
  await settles(opening, 'the open of a binary file');

  assert.strictEqual(h.panes.size, 0, 'no tab for a binary file');
  assert.match(h.toasts[0].body, /binary/);
  assert.strictEqual(
    h.commandsNamed('editor_temp_cleanup').length,
    1,
    'the downloaded temp file is removed',
  );
}

// --- 6. The same remote file twice is one tab; two hosts are two ----------
{
  const h = makeHarness();
  const open = async (hostLabel) => {
    const p = h.service.openRemoteFile({
      paneId: 4, remotePath: '/home/me/notes.txt', hostLabel, size: 120,
    });
    await tick();
    if (h.started.length) h.emit(progress(h.started[h.started.length - 1].id, 'completed'));
    await settles(p, `the open of ${hostLabel}`);
  };

  await open('me@alpha');
  assert.strictEqual(h.panes.size, 1);

  const downloadsBefore = h.commandsNamed('transfer_download').length;
  await open('me@alpha');
  assert.strictEqual(h.panes.size, 1, 'the same file on the same host reuses the tab');
  assert.strictEqual(
    h.commandsNamed('transfer_download').length,
    downloadsBefore,
    'and re-downloads nothing',
  );
  assert.deepStrictEqual(h.activated.slice(-2), [['tab', 1], ['pane', 1]], 'it focuses instead');

  await open('me@beta');
  assert.strictEqual(h.panes.size, 2, 'the same filename on another host is its own tab');
}

// --- 6b. Two double-clicks DURING the download are still one tab ----------
// The existing-tab check can only see panes that exist, and during the seconds
// a download takes there is no pane to find. Two downloads onto one temp path
// would give two editors on one file — and then closing either would delete
// that file and the parent directories it empties, leaving the survivor's next
// save with nowhere to write (editor_write_file does not recreate them).
{
  const h = makeHarness();
  const descriptor = {
    paneId: 4, remotePath: '/home/me/notes.txt', hostLabel: 'me@example.com', size: 120,
  };
  const first = h.service.openRemoteFile({ ...descriptor });
  const second = h.service.openRemoteFile({ ...descriptor });
  await tick();

  assert.strictEqual(
    h.commandsNamed('transfer_download').length,
    1,
    'the second double-click joins the running download instead of starting its own',
  );

  h.emit(progress(h.started[0].id, 'completed'));
  await settles(first, 'the first of two concurrent opens');
  await settles(second, 'the second of two concurrent opens');

  assert.strictEqual(h.panes.size, 1, 'and there is exactly one tab on that temp path');
  assert.deepStrictEqual(
    h.activated.slice(-2),
    [['tab', 1], ['pane', 1]],
    'the joiner focuses the tab the first open produced',
  );
  assert.strictEqual(h.commandsNamed('editor_temp_cleanup').length, 0, 'nothing was deleted');
  assert.strictEqual(h.toasts.length, 0, 'and neither click raised an error');
}

// --- 6c. A failed concurrent open cleans up once and wedges nothing -------
{
  const h = makeHarness();
  const descriptor = {
    paneId: 4, remotePath: '/home/me/notes.txt', hostLabel: 'me@example.com', size: 120,
  };
  const first = h.service.openRemoteFile({ ...descriptor });
  const second = h.service.openRemoteFile({ ...descriptor });
  await tick();
  h.emit(progress(h.started[0].id, 'failed', { error: 'connection reset' }));
  await settles(first, 'the first of two concurrent opens that failed');
  await settles(second, 'the second of two concurrent opens that failed');

  assert.strictEqual(h.panes.size, 0);
  assert.strictEqual(h.toasts.length, 2, 'each click is told the open failed');
  assert.strictEqual(
    h.commandsNamed('editor_temp_cleanup').length,
    1,
    'but only the click that owned the download deletes the temp file',
  );

  // The path must not be stuck for the rest of the session.
  const retry = h.service.openRemoteFile({ ...descriptor });
  await tick();
  assert.strictEqual(
    h.commandsNamed('transfer_download').length,
    2,
    'a later open of the same path downloads again',
  );
  h.emit(progress(h.started[1].id, 'completed'));
  await settles(retry, 'the retry after a failed open');
  assert.strictEqual(h.panes.size, 1, 'and opens its tab');
}

// --- 7. Saving a remote pane uploads, then clears dirty -------------------
{
  const h = makeHarness();
  const opening = h.service.openRemoteFile({
    paneId: 4, remotePath: '/home/me/notes.txt', hostLabel: 'me@example.com', size: 120,
  });
  await tick();
  h.emit(progress(h.started[0].id, 'completed'));
  await settles(opening, 'the open before a save');

  const pane = [...h.panes.values()][0];
  pane.view.type(' and more');
  assert.strictEqual(pane.dirty, true);

  const saving = h.service.savePane(pane);
  await tick();

  const writes = h.commandsNamed('editor_write_file');
  assert.strictEqual(writes.length, 1, 'the temp file is written first');
  assert.strictEqual(writes[0].args.contents, 'remote contents and more');

  const upload = h.started[h.started.length - 1];
  assert.strictEqual(upload.command, 'transfer_upload');
  assert.deepEqual(upload.args, {
    paneId: 4,
    localPath: pane.filePath,
    remotePath: '/home/me/notes.txt',
  });
  assert.strictEqual(pane.dirty, true, 'still dirty while the upload is in flight');

  h.emit({ ...progress(upload.id, 'completed'), kind: 'upload' });
  await settles(saving, 'the save after a completed upload');

  assert.strictEqual(pane.dirty, false, 'clean once the bytes reached the host');
  const success = h.toasts.filter((t) => t.kind === 'success');
  assert.strictEqual(success.length, 1);
  assert.strictEqual(success[0].title, 'Uploaded');
  assert.strictEqual(success[0].body, 'me@example.com:/home/me/notes.txt');
}

// --- 8. A failed upload keeps the edit ------------------------------------
// The dropped-connection case. The pane must stay dirty (so the close guards
// still ask), the temp file must stay on disk (so the bytes are recoverable),
// and the save must reject (so nothing upstream reads it as success).
{
  const h = makeHarness();
  const opening = h.service.openRemoteFile({
    paneId: 4, remotePath: '/home/me/notes.txt', hostLabel: 'me@example.com', size: 120,
  });
  await tick();
  h.emit(progress(h.started[0].id, 'completed'));
  await settles(opening, 'the open before a failing save');

  const pane = [...h.panes.values()][0];
  pane.view.type('!');

  const saving = h.service.savePane(pane);
  await tick();
  const upload = h.started[h.started.length - 1];
  h.emit({ ...progress(upload.id, 'failed'), kind: 'upload', error: 'connection lost' });

  await assert.rejects(() => settles(saving, 'the save after a failed upload'), /connection lost/, 'the save fails');
  assert.strictEqual(pane.dirty, true, 'and the pane is still dirty');
  assert.strictEqual(
    h.commandsNamed('editor_temp_cleanup').length,
    0,
    'the temp file holding the only copy of the edit is left alone',
  );
  const failures = h.toasts.filter((t) => t.title === 'Upload Failed');
  assert.strictEqual(failures.length, 1);
  assert.match(failures[0].body, /connection lost/);
  assert.match(failures[0].body, /saved locally/);

  // A close guard asked at this point must refuse rather than discard. No
  // dialog service is registered in this sandbox, so confirmDirtyPanes takes
  // its "cannot ask, so do not close" branch — which only fires because the
  // failed upload left the pane dirty.
  const ok = await settles(h.service.confirmDirtyPanes([pane]), 'the close guard');
  assert.strictEqual(ok, false, 'a dirty pane with no way to ask is not closeable');
}

// --- 8b. A save onto a disconnected session ------------------------------
// transfer_upload itself rejects here — Rust's get_ssh_handle returns
// Err("No SSH session for main:4") — so no transfer id and no event ever
// exist. That path has to settle too.
{
  const h = makeHarness({ failStart: 'No SSH session for main:4' });
  const opening = h.service.openRemoteFile({
    paneId: 4, remotePath: '/home/me/notes.txt', hostLabel: 'me@example.com', size: 120,
  });
  await tick();
  await settles(opening, 'the open onto a dead session');

  assert.strictEqual(h.panes.size, 0, 'the download could not even start');
  assert.strictEqual(h.toasts[0].title, 'Cannot Open File');
  assert.match(h.toasts[0].body, /No SSH session/);
  assert.strictEqual(h.activeListeners(), 0, 'and the listener did not leak');
  assert.strictEqual(
    h.commandsNamed('editor_temp_cleanup').length,
    1,
    'the empty temp file made for the attempt is removed',
  );
}

// --- 9. A transfer that never reports still settles -----------------------
// The worst failure mode: a connection that dies mid-handshake emits nothing
// at all, so silence has to be an error rather than a hang.
{
  let armed = null;
  // A hand-driven clock: the sandbox's setTimeout records the callback instead
  // of scheduling it, so the watchdog can be fired without the test sitting
  // for a minute. `listen` here resolves but nothing is ever emitted.
  const realSetTimeout = setTimeout;
  const sandboxTimers = [];
  const sandbox = { console };
  sandbox.window = sandbox;
  sandbox.setTimeout = (fn, ms) => { sandboxTimers.push({ fn, ms }); return sandboxTimers.length; };
  sandbox.clearTimeout = (handle) => { if (handle) sandboxTimers[handle - 1] = null; };
  vm.createContext(sandbox);
  const calls = [];
  sandbox.CM6 = {};
  sandbox.toast = { error: (title, body) => { armed = { title, body }; } };
  sandbox.termlabServices = {
    tauriClient: {
      invoke(command, args) {
        calls.push({ command, args });
        if (command === 'editor_can_open') return Promise.resolve(null);
        if (command === 'editor_temp_path') return Promise.resolve('/tmp/termlab-sftp-edits/a/b/x.txt');
        if (command === 'transfer_download') return Promise.resolve('stuck-transfer');
        return Promise.resolve(null);
      },
      listen: () => Promise.resolve(() => {}),
    },
  };
  sandbox.__termlabPaneAccess = {
    currentPane: () => null,
    allPanes: () => new Map(),
    setFocusedPane: () => {},
    activateTab: () => {},
  };
  sandbox.__termlabCreateEditorTab = () => { throw new Error('must not open a tab'); };
  vm.runInContext(fs.readFileSync(SERVICE_PATH, 'utf8'), sandbox, { filename: SERVICE_PATH });

  const opening = sandbox.termlabEditorService.openRemoteFile({
    paneId: 4, remotePath: '/home/me/x.txt', hostLabel: 'me@example.com', size: 10,
  });
  await new Promise((resolve) => realSetTimeout(resolve, 0));

  const watchdog = sandboxTimers.filter(Boolean).pop();
  assert.ok(watchdog, 'a stall watchdog is armed');
  assert.strictEqual(watchdog.ms, 60000, 'it waits a minute before giving up');
  watchdog.fn();
  await settles(opening, 'the open abandoned by the stall watchdog');

  assert.ok(armed, 'silence produces an error rather than a hang');
  assert.strictEqual(armed.title, 'Cannot Open File');
  assert.match(armed.body, /stalled/);
  assert.ok(
    calls.some((c) => c.command === 'transfer_cancel' && c.args.transferId === 'stuck-transfer'),
    'and the abandoned backend transfer is cancelled',
  );
  assert.ok(
    calls.some((c) => c.command === 'editor_temp_cleanup'),
    'and the partial download is removed',
  );
}

// --- 10. discardRemoteTemp only touches remote panes ----------------------
{
  const h = makeHarness();
  h.service.discardRemoteTemp({
    kind: 'editor',
    filePath: '/tmp/termlab-sftp-edits/a/b/notes.txt',
    remote: { paneId: 4, remotePath: '/home/me/notes.txt', hostLabel: 'me@example.com' },
  });
  h.service.discardRemoteTemp({ kind: 'editor', filePath: '/home/me/local.txt', remote: null });
  h.service.discardRemoteTemp(null);

  const cleanups = h.commandsNamed('editor_temp_cleanup');
  assert.strictEqual(cleanups.length, 1, 'only the remote pane is cleaned up');
  assert.strictEqual(cleanups[0].args.path, '/tmp/termlab-sftp-edits/a/b/notes.txt');
}

console.log('editor remote transfer: all assertions passed');

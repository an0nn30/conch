// Open and save orchestration for the editor.
//
// The one place that knows a file has a location as well as contents: it
// reads through the Rust guards, hands the text to a tab, and writes it back.
(function initTermLabEditorService(global) {
  'use strict';

  function invoke(command, args) {
    const client = global.termlabServices && global.termlabServices.tauriClient;
    if (!client || typeof client.invoke !== 'function') {
      return Promise.reject(new Error('tauri client unavailable'));
    }
    return client.invoke(command, args);
  }

  function lspBridge() {
    return global.termlabLspBridge || null;
  }

  function lspState() {
    return global.termlabLspState || null;
  }

  function languageIdFor(filePath) {
    const base = String(filePath || '').split('/').pop().toLowerCase();
    const extension = base.includes('.') ? base.split('.').pop() : '';
    if (['ts', 'tsx'].includes(extension)) return 'typescript';
    if (['js', 'jsx', 'mjs', 'cjs'].includes(extension)) return 'javascript';
    if (['json', 'jsonc'].includes(extension)) return 'json';
    if (extension === 'py') return 'python';
    if (extension === 'rs') return 'rust';
    if (extension === 'go') return 'go';
    if (extension === 'c') return 'c';
    if (['cc', 'cpp', 'cxx', 'h', 'hpp', 'hxx'].includes(extension)) return 'cpp';
    if (extension === 'java') return 'java';
    // Empty/unknown deliberately lets the Rust catalog's file binding make
    // the authoritative decision (or keep this as a plain local document).
    return '';
  }

  function logLspError(operation, error) {
    const bridge = lspBridge();
    const normalized = bridge && typeof bridge.normalizeError === 'function'
      ? bridge.normalizeError(error, operation)
      : { message: 'Language features are unavailable; editing continues.' };
    console.warn(`${normalized.message} (${operation})`);
  }

  function toastError(title, body) {
    if (global.toast && typeof global.toast.error === 'function') {
      global.toast.error(title, body);
      return;
    }
    console.error(`${title}: ${body}`);
  }

  function toastSuccess(title, body) {
    if (global.toast && typeof global.toast.success === 'function') {
      global.toast.success(title, body);
    }
  }

  // beforeBuildCommand only fires under `cargo tauri build`/`dev`, so a plain
  // `cargo run` yields an index.html pointing at a bundle that was never
  // generated. Say so instead of failing as an editor that does nothing.
  function bundleMissing() {
    if (global.CM6) return false;
    toastError(
      'Editor unavailable',
      'The editor bundle is missing. Run "npm run build:vendor" in crates/termlab_tauri/frontend.',
    );
    return true;
  }

  // The composed tab and pane managers live inside main-runtime's closure.
  // `global.termlabTabManager` is the FACTORY ({create}) and there is no
  // `global.paneManager` at all, so both of the obvious-looking accessors
  // would be undefined here. manager-compose-runtime.js publishes the real
  // entry points; resolve them lazily because this script loads first.
  function paneAccess() {
    return global.__termlabPaneAccess || null;
  }

  function createEditorTab(options) {
    if (typeof global.__termlabCreateEditorTab !== 'function') {
      throw new Error('editor tabs are unavailable (app not composed yet)');
    }
    return global.__termlabCreateEditorTab(options);
  }

  function currentPane() {
    const access = paneAccess();
    return access ? access.currentPane() : null;
  }

  function eachEditorPane(fn) {
    const access = paneAccess();
    if (!access) return;
    const panes = access.allPanes();
    if (!panes || typeof panes.values !== 'function') return;
    for (const pane of panes.values()) {
      if (pane && pane.kind === 'editor') fn(pane);
    }
  }

  // Opening a file that is already open focuses its tab instead of making a
  // second view of the same bytes — two editors on one path would each hold a
  // doc and the last save would silently win.
  function focusExistingEditor(filePath) {
    // An untitled buffer has `filePath === null`, and two of those are not the
    // same file — they are two files that do not exist yet. Without this
    // guard, opening with a falsy path would match the first untitled pane and
    // hand the user their scratch buffer instead of the file they asked for.
    if (!filePath) return false;
    let found = null;
    eachEditorPane((pane) => {
      if (!found && pane.filePath && pane.filePath === filePath) found = pane;
    });
    if (!found) return false;
    const access = paneAccess();
    access.activateTab(found.tabId);
    access.setFocusedPane(found.paneId);
    return true;
  }

  // A local pane exists before its reservation is committed. Keep that
  // terminal ownership operation attached to the pane so a close cannot
  // destroy the view and let a late manager response attach state to it.
  const ownershipOpens = new WeakMap();

  async function openLocalFile(filePath) {
    if (bundleMissing()) return;
    if (focusExistingEditor(filePath)) return;
    const bridge = lspBridge();
    let reservation = null;
    try {
      if (bridge && typeof bridge.reserveDocument === 'function') {
        reservation = await bridge.reserveDocument(filePath);
        if (!reservation || reservation.kind !== 'reserved') {
          if (reservation && reservation.kind === 'focusOwner') await bridge.focusOwner(reservation);
          return;
        }
      }
      const canonicalPath = reservation && reservation.canonicalPath
        ? reservation.canonicalPath
        : filePath;
      const contents = await invoke('editor_read_file', { path: canonicalPath });
      let createdPane = null;
      createEditorTab({
        filePath: canonicalPath,
        contents,
        remote: null,
        onPaneCreated: (pane) => { createdPane = pane; },
      });
      if (reservation && bridge) {
        if (!createdPane) throw new Error('editor pane construction did not complete');
        const ownershipOpen = { closing: false, promise: null };
        ownershipOpen.promise = (async () => {
          try {
            const opened = await bridge.openDocument(
              reservation.reservationId,
              createdPane.paneId,
              contents,
              languageIdFor(canonicalPath),
            );
            reservation = null;
            await attachOpenedDocument(createdPane, opened, contents).catch((error) => {
              admissionFor(createdPane).desynchronized = true;
              logLspError('reconcile opened document', error);
            });
            return { committed: true, documentId: opened.documentId };
          } catch (error) {
            await bridge.releaseDocument(reservation.reservationId).catch(() => {});
            reservation = null;
            logLspError('open document', error);
            return { committed: false };
          }
        })();
        ownershipOpens.set(createdPane, ownershipOpen);
        await ownershipOpen.promise;
        if (!ownershipOpen.closing && ownershipOpens.get(createdPane) === ownershipOpen) {
          ownershipOpens.delete(createdPane);
        }
      }
    } catch (error) {
      if (reservation && bridge) {
        await bridge.releaseDocument(reservation.reservationId).catch(() => {});
      }
      toastError('Cannot Open File', String(error));
    }
  }

  // ---------------------------------------------------------------------------
  // Remote files
  // ---------------------------------------------------------------------------

  // No event at all for this long, for a transfer we started, and we give up.
  // A download reports at least every 100ms while bytes move, but a connection
  // that dies mid-SFTP-handshake reports nothing ever — russh just waits — and
  // "nothing ever" is exactly the case that must not hang.
  const TRANSFER_STALL_MS = 60000;

  // Run one transfer to a terminal status. `start` is called to kick it off and
  // must resolve with the transfer_id.
  //
  // transfer_download/transfer_upload are fire-and-forget: they mint an id,
  // spawn a task and return, and all news arrives on the shared
  // 'transfer-progress' event. Two ordering hazards follow, and both are
  // handled here rather than by the callers:
  //
  //  - The listener is registered BEFORE `start` runs. The id is minted in
  //    Rust before the task is spawned and the event travels on a different
  //    channel from the command's reply, so a small file really can finish
  //    before the invoke resolves. Terminal events for ids we do not know yet
  //    are held in `early` and re-examined once the id arrives.
  //  - Every exit settles the promise. Failure, cancellation and silence all
  //    reject with something a user can read.
  function runTransfer(start) {
    const client = global.termlabServices && global.termlabServices.tauriClient;
    if (!client || typeof client.invoke !== 'function' || typeof client.listen !== 'function') {
      return Promise.reject(new Error('tauri client unavailable'));
    }

    return new Promise((resolve, reject) => {
      let unlisten = null;
      let settled = false;
      let transferId = null;
      let stallTimer = null;
      const early = new Map();

      const finish = (fn, arg) => {
        if (settled) return;
        settled = true;
        if (stallTimer) clearTimeout(stallTimer);
        stallTimer = null;
        if (typeof unlisten === 'function') unlisten();
        fn(arg);
      };

      const armStall = () => {
        if (stallTimer) clearTimeout(stallTimer);
        stallTimer = setTimeout(() => {
          // Stop the backend task too, so it cannot keep writing to a temp
          // file this caller has already given up on.
          if (transferId) client.invoke('transfer_cancel', { transferId }).catch(() => {});
          finish(reject, new Error('Transfer stalled: no progress reported'));
        }, TRANSFER_STALL_MS);
      };

      // The only place statuses are interpreted. They are the serde
      // `snake_case` renderings of `termlab_remote::transfer::TransferStatus`
      // (crates/termlab_remote/src/transfer.rs):
      //
      //     Pending | InProgress | Completed | Failed | Cancelled
      //       -> 'pending' 'in_progress' 'completed' 'failed' 'cancelled'
      //
      // The three terminal ones are the same three files-panel's progress bars
      // discriminate on (features/files/transfers.js). Getting them wrong is
      // invisible in review and catastrophic at runtime: this promise would
      // simply never settle and opening a remote file would hang with no error.
      //
      // Returns true when `progress` was terminal and the promise is settled.
      const settleFrom = (progress) => {
        if (progress.status === 'completed') {
          finish(resolve);
          return true;
        }
        if (progress.status === 'failed' || progress.status === 'cancelled') {
          finish(reject, new Error(progress.error || `Transfer ${progress.status}`));
          return true;
        }
        return false;
      };

      const onProgress = (event) => {
        const progress = event && event.payload;
        if (!progress || !progress.transfer_id) return;
        if (transferId === null) {
          // The id is not known yet, so there is no way to tell whose event
          // this is. Hold the latest per id and judge it below.
          early.set(progress.transfer_id, progress);
          return;
        }
        if (progress.transfer_id !== transferId) return;
        if (!settleFrom(progress)) armStall();
      };

      armStall();

      client.listen('transfer-progress', onProgress).then((fn) => {
        unlisten = fn;
        if (settled) {
          if (typeof fn === 'function') fn();
          return null;
        }
        return Promise.resolve(start()).then((id) => {
          transferId = String(id);
          const seen = early.get(transferId);
          early.clear();
          if (seen) settleFrom(seen);
        });
      }).catch((error) => finish(reject, error instanceof Error ? error : new Error(String(error))));
    });
  }

  // Remote opens currently running, keyed by the temp path each is downloading
  // into. `focusExistingEditor` only sees panes that already exist, so during
  // the seconds a download takes there is nothing for it to find: a second
  // double-click would pass the same check and start its own download onto the
  // same path. That ends in the exact state focusExistingEditor exists to
  // prevent — two editors on one file, the last save silently winning — and
  // then in something worse. Closing either tab runs discardRemoteTemp, and
  // editor_temp_cleanup deletes the file AND climbs deleting the parent
  // directories it empties, while editor_write_file does not recreate them. So
  // the surviving tab's next save fails with "No such file or directory" and
  // the edit has nowhere to go.
  //
  // Scope: this closes the race **within one window only**. Every window runs
  // its own JS context and so its own `opensInFlight` (and its own
  // `focusExistingEditor`), while `editor_temp_path` is a pure function of
  // (host, path). Two windows opening the same remote file therefore still land
  // on one temp file and still reach the state described above. That is a known
  // limitation of the feature, not something this map covers; closing it needs
  // the registry to live in Rust, where the path is computed.
  const opensInFlight = new Map();

  async function downloadAndOpen(paneId, remotePath, hostLabel, localPath) {
    await runTransfer(() => invoke('transfer_download', {
      paneId,
      remotePath,
      localPath,
      origin: 'editor',
      conflictPolicy: { kind: 'overwrite' },
    }));

    // The guards run a second time here, against the bytes rather than the
    // directory listing: a stale size and binary contents are both only
    // knowable now.
    const contents = await invoke('editor_read_file', { path: localPath });
    createEditorTab({
      filePath: localPath,
      contents,
      remote: { paneId, remotePath, hostLabel },
    });
  }

  // Open a file that lives on an SSH host: download it to a temp path, edit it
  // there, and upload it back on save.
  async function openRemoteFile(descriptor) {
    if (bundleMissing()) return;
    const { paneId, remotePath, hostLabel, size } = descriptor || {};
    const name = String(remotePath || '').split('/').pop();
    let localPath = null;
    let owned = false;
    try {
      // Before a byte moves. A mis-click on a 2 GB file or a .jar has to cost
      // nothing, which is the whole reason editor_can_open takes a name and a
      // size instead of a path — the cap and the blocklist stay in Rust.
      await invoke('editor_can_open', { name, size: Number(size) || 0 });

      // Deterministic per (host, remote path), so this is also how "the same
      // remote file twice" resolves to one tab, and how the same filename on
      // two different hosts resolves to two.
      localPath = await invoke('editor_temp_path', { hostLabel, remotePath });
      if (focusExistingEditor(localPath)) return;

      const pending = opensInFlight.get(localPath);
      if (pending) {
        // Join the download already running instead of starting a second one,
        // then focus what it produced. A rejection here lands in the catch
        // below, so a second click on a file whose download failed is told so
        // too — but it cleans up nothing, because it owns nothing.
        await pending;
        focusExistingEditor(localPath);
        return;
      }

      owned = true;
      // .finally is attached before the promise is stored, so the entry is
      // gone by the time anything awaiting it wakes: a later open of the same
      // path is never handed a settled promise from an attempt that is over.
      // Clearing on failure as well as success is what keeps a failed download
      // from wedging that path for the rest of the session.
      const open = downloadAndOpen(paneId, remotePath, hostLabel, localPath)
        .finally(() => { opensInFlight.delete(localPath); });
      opensInFlight.set(localPath, open);
      await open;
    } catch (error) {
      // Nothing reached an editor, so whatever landed on disk is litter rather
      // than content — unlike an upload failure, where the temp file is the
      // only copy of the user's edit and must stay. Safe to delete: localPath
      // came from editor_temp_path, and editor_temp_cleanup refuses anything
      // outside the temp root regardless of what it is handed. Only the caller
      // that started the download deletes it, so a joiner cannot remove a file
      // some other open is still working on.
      if (owned && localPath) invoke('editor_temp_cleanup', { path: localPath }).catch(() => {});
      toastError('Cannot Open File', String(error));
    }
  }

  // Drop the temp file behind a closed remote editor tab, plus any parent
  // directories it leaves empty. Called by tab-manager's close path, which has
  // already settled the unsaved-changes question.
  function discardRemoteTemp(pane) {
    if (!pane || !pane.remote || !pane.filePath) return;
    invoke('editor_temp_cleanup', { path: pane.filePath }).catch(() => {});
  }

  // A pane being torn down can still be the subject of a Save As chooser: the
  // Unsaved Changes prompt STACKS on top of that chooser rather than replacing
  // it, so ⌘W → "Don't Save" destroys the pane with the chooser still on
  // screen and still bound to it. Both teardown paths (tab-manager's closeTab
  // and pane-manager's split close) call this, unconditionally — a pathless
  // pane can have a chooser up without being dirty at all (⌘S on an untouched
  // untitled buffer opens one), so hanging this off the dirty prompt would miss
  // exactly the case with no prompt to hang it off.
  //
  // Same shape as discardRemoteTemp above: the teardown says what happened, and
  // this module owns what that means.
  function cancelPendingChooser(pane) {
    const dialog = global.termlabFileDialog;
    if (!pane || !dialog || typeof dialog.cancelForPane !== 'function') return;
    dialog.cancelForPane(pane);
  }

  // ---------------------------------------------------------------------------
  // Untitled buffers
  // ---------------------------------------------------------------------------

  // Per-window session state. Numbers are never reused after a tab closes —
  // Notepad's rule: "Untitled" coming back for a third buffer reads as the one
  // the user just closed having reappeared.
  let untitledCount = 0;

  // File → New File. No invoke, no file: an untitled buffer exists only in its
  // pane until a save gives it a home, and savePane's diversion below is what
  // makes every save path ask where that is.
  function openUntitled() {
    if (bundleMissing()) return;
    untitledCount += 1;
    try {
      createEditorTab({
        filePath: null,
        contents: '',
        remote: null,
        untitledSeq: untitledCount,
      });
    } catch (error) {
      toastError('Cannot Create File', String(error));
    }
  }

  // What to call a pane that has no path. The prompt and the Save As field
  // have to say the same "Untitled-2" the tab does, so both ask the one
  // formula rather than composing a second one.
  function untitledName(pane) {
    const labels = global.termlabEditorTabLabel;
    if (!labels || typeof labels.editorTabLabel !== 'function') return 'Untitled';
    return labels.editorTabLabel(pane).label;
  }

  // Cancelling the Save As chooser is not a failure — but it is not a save
  // either, and savePane's contract is that resolving means saved. So it
  // rejects with this sentinel, which every catch-site treats as "not saved"
  // while showing nothing: `:wq` does not close, the close guards abort, and
  // no red toast flashes for a deliberate Escape.
  function saveCancelled() {
    const error = new Error('save cancelled');
    error.name = 'SaveCancelled';
    return error;
  }

  function isSaveCancelled(error) {
    return !!error && error.name === 'SaveCancelled';
  }

  // A pending CodeMirror ChangeSet is metadata, not a second copy of the
  // document. Composing sets keeps every emitted offset relative to the one
  // pre-change snapshot the manager versions.
  const documentAdmissions = new WeakMap();

  function admissionFor(pane) {
    let admission = documentAdmissions.get(pane);
    if (!admission) {
      admission = {
        sequence: 0,
        processed: 0,
        pending: null,
        queue: [],
        draining: null,
        reconciling: null,
        desynchronized: false,
        expectedVersion: null,
        closing: false,
        closeInvalidated: false,
        attachmentGeneration: 0,
        barrierTickets: [],
        operationTail: Promise.resolve(),
      };
      documentAdmissions.set(pane, admission);
    }
    return admission;
  }

  function promotePending(pane, admission, throughSequence) {
    const pending = admission.pending;
    if (!pending) return;
    if (Number.isInteger(throughSequence) && pending.lastSequence > throughSequence) return;
    if (pending.timer) clearTimeout(pending.timer);
    admission.pending = null;
    admission.queue.push(pending);
  }

  function documentTransaction(pane, update) {
    if (!pane || !update || !update.docChanged || !update.changes) return;
    const admission = admissionFor(pane);
    admission.sequence += 1;
    if (admission.closing) admission.closeInvalidated = true;
    let pending = admission.pending;
    if (!pending) {
      pending = {
        changes: update.changes,
        firstSequence: admission.sequence,
        lastSequence: admission.sequence,
        timer: null,
      };
      pending.timer = setTimeout(() => {
        if (admission.pending !== pending) return;
        promotePending(pane, admission);
        drainThrough(pane, admission.sequence, false).catch((error) => logLspError('apply changes', error));
      }, 40);
      admission.pending = pending;
      return;
    }
    if (pending.changes && typeof pending.changes.compose === 'function') {
      pending.changes = pending.changes.compose(update.changes);
      pending.lastSequence = admission.sequence;
    } else {
      // Test doubles and older CodeMirror shims may not expose compose. Flush
      // the admitted snapshot before starting the next one rather than ever
      // combining offsets from different snapshots.
      promotePending(pane, admission);
      const next = {
        changes: update.changes,
        firstSequence: admission.sequence,
        lastSequence: admission.sequence,
        timer: null,
      };
      next.timer = setTimeout(() => {
        if (admission.pending !== next) return;
        promotePending(pane, admission);
        drainThrough(pane, admission.sequence, false).catch((error) => logLspError('apply changes', error));
      }, 40);
      admission.pending = next;
      drainThrough(pane, pending.lastSequence, false).catch((error) => logLspError('apply changes', error));
    }
  }

  async function flushChangeSet(pane, changes, admission) {
    const bridge = lspBridge();
    const stateStore = lspState();
    const document = stateStore && stateStore.get(pane);
    if (!bridge || !document || !changes || typeof changes.iterChanges !== 'function') return;
    const edits = [];
    changes.iterChanges((fromA, toA, _fromB, _toB, inserted) => {
      edits.push({
        fromUtf16: fromA,
        toUtf16: toA,
        insertedText: inserted && typeof inserted.toString === 'function' ? inserted.toString() : '',
      });
    });
    if (!edits.length) return;
    edits.sort((left, right) => right.fromUtf16 - left.fromUtf16 || right.toUtf16 - left.toUtf16);
    const baseVersion = document.version;
    const nextVersion = baseVersion + 1;
    const attachmentGeneration = admission.attachmentGeneration;
    const documentId = document.documentId;
    const result = await bridge.applyChanges(document.documentId, {
      documentId: document.documentId,
      baseVersion,
      nextVersion,
      changes: edits,
    });
    const currentDocument = stateStore.get(pane);
    if (
      admission.attachmentGeneration !== attachmentGeneration
      || !currentDocument
      || currentDocument.documentId !== documentId
    ) return;
    if (result && result.kind === 'applied') {
      stateStore.setVersion(pane, result.version);
      return;
    }
    if (result && result.kind === 'resyncRequired' && pane.view) {
      admission.desynchronized = true;
      admission.expectedVersion = Number(result.expectedVersion);
      return;
    }
    throw new Error('language manager returned an uncertain change outcome');
  }

  function discardAdmissionsThrough(admission, sequence) {
    if (admission.pending && admission.pending.lastSequence <= sequence) {
      if (admission.pending.timer) clearTimeout(admission.pending.timer);
      admission.pending = null;
    }
    admission.queue = admission.queue.filter((entry) => entry.lastSequence > sequence);
    admission.processed = Math.max(admission.processed, sequence);
  }

  async function resyncSnapshot(pane, admission, throughSequence, contents, expectedVersion) {
    const stateStore = lspState();
    const document = stateStore && stateStore.get(pane);
    const bridge = lspBridge();
    if (!document || !bridge) return;
    const baseVersion = Number.isInteger(expectedVersion)
      ? Math.max(document.version, expectedVersion)
      : document.version;
    const version = baseVersion + 1;
    const attachmentGeneration = admission.attachmentGeneration;
    const documentId = document.documentId;
    const resynced = await bridge.resyncDocument(document.documentId, version, contents);
    const currentDocument = stateStore.get(pane);
    if (
      admission.attachmentGeneration !== attachmentGeneration
      || !currentDocument
      || currentDocument.documentId !== documentId
    ) return;
    stateStore.setVersion(
      pane,
      resynced && Number.isInteger(resynced.version) ? resynced.version : version,
    );
    if (resynced && resynced.status) stateStore.updateStatus(resynced.status);
    discardAdmissionsThrough(admission, throughSequence);
    admission.desynchronized = false;
    admission.expectedVersion = null;
  }

  async function drainQueue(pane, admission, targetSequence, snapshotContents) {
    if (admission.reconciling) await admission.reconciling;
    while (admission.queue.length && admission.queue[0].lastSequence <= targetSequence) {
      if (admission.desynchronized) {
        const contents = typeof snapshotContents === 'string'
          ? snapshotContents
          : (pane.view && pane.view.state.doc.toString());
        if (typeof contents !== 'string') throw new Error('editor text is unavailable for resync');
        await resyncSnapshot(
          pane,
          admission,
          targetSequence,
          contents,
          admission.expectedVersion,
        );
        continue;
      }
      const entry = admission.queue.shift();
      try {
        await flushChangeSet(pane, entry.changes, admission);
      } catch (error) {
        admission.desynchronized = true;
        logLspError('apply changes', error);
      }
      admission.processed = Math.max(admission.processed, entry.lastSequence);
    }
  }

  async function drainThrough(pane, targetSequence, synchronize, snapshotContents) {
    const admission = admissionFor(pane);
    const earliestTicket = admission.barrierTickets[0];
    const cappedTarget = earliestTicket
      ? Math.min(targetSequence, earliestTicket.targetSequence)
      : targetSequence;
    promotePending(pane, admission, cappedTarget);
    const stateStore = lspState();
    if (!stateStore || !stateStore.get(pane)) return;
    const hasEligibleQueue = () => (
      admission.queue.length > 0 && admission.queue[0].lastSequence <= cappedTarget
    );
    while (admission.processed < cappedTarget || hasEligibleQueue() || admission.reconciling) {
      if (!admission.draining) {
        let current = null;
        current = drainQueue(pane, admission, cappedTarget, snapshotContents).finally(() => {
          if (admission.draining === current) admission.draining = null;
        });
        admission.draining = current;
      }
      await admission.draining;
      promotePending(pane, admission, cappedTarget);
      if (!hasEligibleQueue() && admission.processed < cappedTarget) break;
    }
    if (synchronize && admission.desynchronized) {
      const contents = typeof snapshotContents === 'string'
        ? snapshotContents
        : (pane.view && pane.view.state.doc.toString());
      if (typeof contents !== 'string') throw new Error('editor text is unavailable for resync');
      await resyncSnapshot(pane, admission, cappedTarget, contents, admission.expectedVersion);
    }
  }

  async function flushDocument(pane) {
    const admission = admissionFor(pane);
    const targetSequence = admission.sequence;
    const contents = pane.view && pane.view.state.doc.toString();
    promotePending(pane, admission, targetSequence);
    await drainThrough(pane, targetSequence, true, contents);
    return { targetSequence, contents };
  }

  function resumeBackgroundDrain(pane, admission) {
    Promise.resolve().then(() => {
      if (admission.closing) return;
      const earliestTicket = admission.barrierTickets[0];
      const targetSequence = earliestTicket
        ? Math.min(admission.sequence, earliestTicket.targetSequence)
        : admission.sequence;
      const contents = pane.view && pane.view.state.doc.toString();
      promotePending(pane, admission, targetSequence);
      drainThrough(pane, targetSequence, false, contents)
        .catch((error) => logLspError('apply changes', error));
    });
  }

  function closeInProgressError() {
    return new Error('editor close is in progress');
  }

  function withFixedBarrier(pane, operation) {
    const admission = admissionFor(pane);
    if (admission.closing) return Promise.reject(closeInProgressError());
    const requestedTargetSequence = admission.sequence;
    const requestedContents = pane.view && pane.view.state.doc.toString();
    promotePending(pane, admission, requestedTargetSequence);
    const ticket = {
      targetSequence: requestedTargetSequence,
      contents: requestedContents,
    };
    admission.barrierTickets.push(ticket);
    const previous = admission.operationTail;
    const current = previous.catch(() => {}).then(async () => {
      const targetSequence = ticket.targetSequence;
      const contents = ticket.contents;
      let synchronizationError = null;
      try {
        await drainThrough(pane, targetSequence, true, contents);
      } catch (error) {
        synchronizationError = error;
      }
      const stateStore = lspState();
      const document = stateStore && stateStore.get(pane);
      const attachmentGeneration = admission.attachmentGeneration;
      try {
        return await operation({
          targetSequence,
          contents,
          synchronizationError,
          document,
          version: document && document.version,
          attachmentGeneration,
        });
      } finally {
        const ticketIndex = admission.barrierTickets.indexOf(ticket);
        if (ticketIndex >= 0) admission.barrierTickets.splice(ticketIndex, 1);
        resumeBackgroundDrain(pane, admission);
      }
    });
    admission.operationTail = current.then(() => undefined, () => undefined);
    return current;
  }

  async function attachOpenedDocument(pane, opened, openedContents) {
    const stateStore = lspState();
    if (!stateStore || typeof stateStore.attach !== 'function') return null;
    const admission = admissionFor(pane);
    admission.attachmentGeneration += 1;
    const state = stateStore.attach(pane, opened);
    const throughSequence = admission.sequence;
    const currentContents = pane.view && pane.view.state.doc.toString();
    if (typeof currentContents !== 'string' || currentContents === openedContents) {
      discardAdmissionsThrough(admission, throughSequence);
      return state;
    }
    let reconcile = null;
    reconcile = resyncSnapshot(pane, admission, throughSequence, currentContents).finally(() => {
      if (admission.reconciling === reconcile) admission.reconciling = null;
    });
    admission.reconciling = reconcile;
    await reconcile;
    return stateStore.get(pane);
  }

  const closesInFlight = new WeakMap();

  function restoreClosePreparation(panes) {
    for (const pane of panes) {
      const admission = admissionFor(pane);
      admission.closing = false;
      admission.closeInvalidated = false;
      if (pane.view && typeof pane.view.termlabSetReadOnly === 'function') {
        pane.view.termlabSetReadOnly(false);
      }
      resumeBackgroundDrain(pane, admission);
    }
  }

  async function closeDocumentGroup(panes) {
    const stateStore = lspState();
    const priorOperations = new Map();
    for (const pane of panes) {
      const admission = admissionFor(pane);
      admission.closing = true;
      admission.closeInvalidated = false;
      if (pane.view && typeof pane.view.termlabSetReadOnly === 'function') {
        pane.view.termlabSetReadOnly(true);
      }
    }
    for (const pane of panes) {
      priorOperations.set(pane, admissionFor(pane).operationTail);
    }
    try {
      for (const pane of panes) {
        const ownershipOpen = ownershipOpens.get(pane);
        if (ownershipOpen) {
          ownershipOpen.closing = true;
          await ownershipOpen.promise;
          if (ownershipOpens.get(pane) === ownershipOpen) ownershipOpens.delete(pane);
        }
      }
      await Promise.all(panes.map(async (pane) => {
        await priorOperations.get(pane);
        await flushDocument(pane);
      }));
      if (panes.some((pane) => admissionFor(pane).closeInvalidated)) {
        restoreClosePreparation(panes);
        return false;
      }
      const documents = panes
        .map((pane) => stateStore && stateStore.get(pane))
        .filter(Boolean);
      const documentIds = documents.map((document) => document.documentId);
      if (documentIds.length === 1) {
        await lspBridge().closeDocument(documentIds[0]);
      } else if (documentIds.length > 1) {
        await lspBridge().closeDocuments(documentIds);
      }
      for (const pane of panes) {
        documentAdmissions.delete(pane);
        if (stateStore) stateStore.clear(pane);
      }
      return true;
    } catch (error) {
      restoreClosePreparation(panes);
      logLspError('close document', error);
      return false;
    }
  }

  function closeDocuments(panes) {
    const uniquePanes = Array.from(new Set((panes || []).filter(Boolean)));
    if (!uniquePanes.length) return Promise.resolve(true);
    const existing = uniquePanes.map((pane) => closesInFlight.get(pane)).filter(Boolean);
    if (existing.length) {
      const shared = existing[0];
      return uniquePanes.every((pane) => closesInFlight.get(pane) === shared)
        ? shared
        : Promise.resolve(false);
    }
    let closing = null;
    closing = closeDocumentGroup(uniquePanes).finally(() => {
      for (const pane of uniquePanes) {
        if (closesInFlight.get(pane) === closing) closesInFlight.delete(pane);
      }
    });
    for (const pane of uniquePanes) closesInFlight.set(pane, closing);
    return closing;
  }

  function closeDocument(pane) {
    return closeDocuments([pane]);
  }

  async function requestFeature(pane, kind, position, trigger) {
    const bridge = lspBridge();
    const stateStore = lspState();
    if (!bridge || typeof bridge[kind] !== 'function') return null;
    if (admissionFor(pane).closing) return null;
    return withFixedBarrier(pane, async (barrier) => {
      const document = barrier.document;
      if (!document || barrier.synchronizationError) {
        if (barrier.synchronizationError) logLspError(kind, barrier.synchronizationError);
        return null;
      }
      try {
        const result = await bridge[kind](document.documentId, position, trigger || null);
        const current = stateStore && stateStore.get(pane);
        const admission = admissionFor(pane);
        if (
          admission.sequence !== barrier.targetSequence
          || admission.attachmentGeneration !== barrier.attachmentGeneration
          || !current
          || current.documentId !== document.documentId
          || current.version !== barrier.version
        ) return null;
        return result;
      } catch (error) {
        logLspError(kind, error);
        return null;
      }
    });
  }

  // One write. Rejects on failure so callers can decide what a failure means;
  // saveActiveEditor turns it into a toast, the close guards turn it into a
  // refusal to close.
  async function writeOnce(pane) {
    return withFixedBarrier(pane, async (barrier) => {
      const managerSynchronized = !barrier.synchronizationError;
      if (barrier.synchronizationError) {
        logLspError('flush before save', barrier.synchronizationError);
      }
      const contents = barrier.contents;
      await invoke('editor_write_file', { path: pane.filePath, contents });
    // For a remote file the local write is a staging step, not the save: what
    // "saved" means is that the bytes reached the host. Uploading BEFORE the
    // dirty reset is what makes a failed upload leave the pane dirty, so the
    // close guards still refuse to discard the tab and the temp file is not
    // deleted out from under an edit that never got off this machine. A local
    // pane has no remote and so is unaffected.
      if (pane.remote) await uploadRemote(pane);
      const document = barrier.document;
      if (managerSynchronized && document && lspBridge()) {
        await lspBridge().didSave(document.documentId).catch((error) => logLspError('did save', error));
      }
    // `dirty` is a plain boolean, not a diff against the document, and the
    // update listener stops firing once it is set. So a keystroke that lands
    // between the snapshot above and this line is on screen but not in the
    // file — clearing the flag unconditionally would mark it saved and, once
    // the close guards read pane.dirty, discard it without a prompt. Only
    // reset when the buffer still matches the bytes that were written.
      if (pane.view && pane.view.state.doc.toString() === contents) {
        pane.view.termlabResetDirty();
      }
    });
  }

  // The write currently in flight for a pane, if any. Two overlapping writes
  // for the same file can resolve out of order: the newer one lands first and
  // clears `dirty`, then the older one overwrites the file with stale bytes
  // and there is nothing left to say so. The close guards below read
  // `pane.dirty`, so that lie is what makes them close a tab over unsaved
  // text. A pane therefore never has more than one write outstanding.
  const savesInFlight = new WeakMap();

  // The Save As chooser currently open FOR a pane, if any — the window
  // between "the user pressed ⌘S on an untitled buffer" and "they answered
  // the dialog", which `savesInFlight` cannot cover because no write has
  // started yet. Deliberately a SEPARATE map: saveAs drains `savesInFlight`
  // before it writes, and openForSave calls saveAs from inside this very
  // await, so parking the placeholder there would make that saveAs wait for
  // the chooser it is answering — a deadlock, not a guard.
  const choosersInFlight = new WeakMap();

  // Save a specific pane. saveActiveEditor covers the keyboard path; the close
  // guards need to save panes that are not focused.
  async function savePane(pane, options) {
    if (!pane || pane.kind !== 'editor' || !pane.view) return;
    if (admissionFor(pane).closing) throw closeInProgressError();

    // THE CHOKE POINT. A pane with no path cannot be written anywhere, so
    // every save path — ⌘S, `:w`, `:wq`, the close guards' Save — asks where
    // to put it here, once, rather than each of them growing its own
    // untitled-file branch.
    if (!pane.filePath) {
      // TWO windows to join, not one.
      //
      // `savesInFlight` only fills once the chooser has been ANSWERED and the
      // Save As it routed has started. The chooser itself sits on screen for
      // as long as the user takes to read it, and ⌘S still reaches the
      // shortcut router while it is up — tl-dialog traps Tab and Escape, not
      // every key, and currentPane() is still this editor. A second save
      // arriving in THAT window would start its own openForSave, be handed
      // the first chooser's answer by file-dialog's `activeChoice`
      // short-circuit, and save the same buffer twice: two writes, and for a
      // remote first save two uploads and two "Saved" toasts, against this
      // file's own one-outstanding-write-per-pane invariant.
      const pending = choosersInFlight.get(pane) || savesInFlight.get(pane);
      if (pending) {
        try {
          await pending;
        } catch (error) {
          // Whatever we joined has already reported itself — saveAs toasts
          // "Save As Failed", and a cancel says nothing on purpose — and left
          // the pane pathless. Rethrowing its raw error would make our caller
          // toast the same failure a second time; the sentinel keeps the
          // outcome (not saved, quietly) without the duplicate.
          if (isSaveCancelled(error) || pane.filePath) throw error;
          throw saveCancelled();
        }
        // The Save As rebound the pane, so anything typed while it ran is
        // still owed an ordinary write — same one-shot retry as the join
        // below, and guarded on filePath so it can never re-enter this branch.
        if (pane.filePath && pane.dirty && !(options && options.noRetry)) {
          await savePane(pane, { noRetry: true });
        }
        return;
      }

      const dialog = global.termlabFileDialog;
      if (!dialog || typeof dialog.openForSave !== 'function') {
        // A real failure, not a cancel: the user asked to save and there is no
        // way to ask them where. Rejecting normally means it gets reported.
        throw new Error('the Save As dialog is unavailable');
      }
      // openForSave resolves null when the chooser was cancelled, and on
      // success has ALREADY routed the target through saveAs — which wrote the
      // file and rebound the pane. Falling through to writeOnce here would
      // write the same bytes a second time.
      //
      // It also resolves null when that saveAs FAILED, having already toasted
      // "Save As Failed" itself, and when a chooser for ANOTHER pane is
      // already up. The sentinel is right for all three: not saved (so `:wq`
      // keeps the tab and the close guards abort), and silent here (so one
      // failure is reported once, not twice, and a cancel not at all).
      //
      // .finally is attached before the entry is stored, exactly as the
      // ordinary save path does, so a joiner can never re-find its own
      // settled entry.
      const asking = Promise.resolve()
        .then(() => dialog.openForSave(pane))
        .then((chosen) => { if (chosen == null) throw saveCancelled(); })
        .finally(() => { choosersInFlight.delete(pane); });
      choosersInFlight.set(pane, asking);
      await asking;
      return;
    }

    const pending = savesInFlight.get(pane);
    if (pending) {
      // Join the write already running instead of starting a second one. It
      // rejects for us too if it fails, so the caller still learns about it.
      await pending;
      // That write may have snapshotted the document before our caller's text
      // existed, in which case the pane is still dirty and owes a write. One
      // retry covers it; retrying without a bound would spin forever against
      // someone who keeps typing.
      if (pane.dirty && !(options && options.noRetry)) {
        await savePane(pane, { noRetry: true });
      }
      return;
    }

    // .finally is attached before the promise is stored, so it settles only
    // after the map entry is gone — a joiner that wakes on it and retries can
    // never find this same entry still there and re-join itself.
    const inFlight = writeOnce(pane).finally(() => {
      savesInFlight.delete(pane);
    });
    savesInFlight.set(pane, inFlight);
    await inFlight;
  }

  async function saveActiveEditor() {
    const pane = currentPane();
    if (!pane || pane.kind !== 'editor' || !pane.view) return;
    try {
      await savePane(pane);
    } catch (error) {
      // A cancelled chooser is the user's own answer, not a failure to report.
      if (isSaveCancelled(error)) return;
      toastError('Save Failed', String(error));
    }
  }

  // Ask about each of `panes` in turn. Returns false — abort the whole close,
  // not just this one tab — the moment anything is less than certain: the user
  // cancelled, a save failed, the dialog service is missing, or a save
  // reported success but left the pane dirty anyway. Callers treat a false
  // return as "do not close".
  async function confirmDirtyPanes(panes) {
    const dirty = (panes || []).filter((pane) => pane && pane.kind === 'editor' && pane.dirty);
    if (dirty.length === 0) return true;

    const dialogs = global.termlabDialogService;
    if (!dialogs || typeof dialogs.confirmSave !== 'function') {
      // No way to ask means no consent. Refusing to close is recoverable;
      // closing is not.
      toastError('Unsaved Changes', 'Cannot confirm unsaved changes, so nothing was closed.');
      return false;
    }

    for (const pane of dirty) {
      const name = pane.filePath
        ? String(pane.filePath).split('/').pop()
        : untitledName(pane);
      const choice = await dialogs.confirmSave(name);
      if (choice === 'cancel') return false;
      if (choice !== 'save') continue;
      try {
        // For an untitled pane this opens the Save As chooser inside the close
        // flow. One dialog at a time: the prompt above has already resolved
        // and closed by the time this await starts.
        await savePane(pane);
      } catch (error) {
        // Cancelling the chooser aborts the close exactly as a failed save
        // does — the buffer is still unsaved either way — but silently: the
        // user just told us "not there", which is not news to report back.
        if (isSaveCancelled(error)) return false;
        toastError('Save Failed', String(error));
        return false; // a failed save must not be treated as consent to lose it
      }
      if (pane.dirty) {
        // The write resolved but the buffer moved on. Do not close over it.
        toastError('Save Failed', `"${name}" still has unsaved changes.`);
        return false;
      }
    }
    return true;
  }

  // Walk every dirty editor in this window and ask. Returns false the moment
  // the user cancels, which aborts the whole close rather than the one tab.
  async function confirmAllDirty() {
    const dirty = [];
    eachEditorPane((pane) => { if (pane.dirty) dirty.push(pane); });
    return confirmDirtyPanes(dirty);
  }

  // Push local bytes at an explicit remote binding. Split out of uploadRemote
  // because Save As uploads to a binding the pane does NOT have yet: the pane
  // is only rebound once this has succeeded, so reading the destination off
  // `pane.remote` here would upload the new file to the old host.
  function uploadTo(remote, localPath) {
    return runTransfer(() => invoke('transfer_upload', {
      paneId: remote.paneId,
      localPath,
      remotePath: remote.remotePath,
      origin: 'editor',
      conflictPolicy: { kind: 'overwrite' },
    }));
  }

  // Push a saved remote file back to its host. Called by writeOnce for panes
  // that have a `remote`, and rejects on failure so the save it belongs to
  // fails too.
  async function uploadRemote(pane) {
    const remote = pane && pane.remote;
    if (!remote) return;
    const { remotePath, hostLabel } = remote;
    try {
      await uploadTo(remote, pane.filePath);
      toastSuccess('Uploaded', `${hostLabel}:${remotePath}`);
    } catch (error) {
      // The temp file stays exactly where it is, and the pane stays dirty.
      // Losing an edit to a dropped connection is the wrong side to be wrong
      // on: the bytes are on disk and another save retries the upload.
      toastError(
        'Upload Failed',
        `${String(error)} — your edit is saved locally at ${pane.filePath}; save again to retry.`,
      );
      throw error;
    }
  }

  // ---------------------------------------------------------------------------
  // Save As
  // ---------------------------------------------------------------------------
  //
  // The only operation that changes what a pane IS. `filePath` and `remote`
  // are the identity that focusExistingEditor, the dirty guards, opensInFlight
  // and discardRemoteTemp all key on, so the rule this whole path is built
  // around is: a rebind is all-or-nothing.
  //
  //   1. everything that can fail happens FIRST, against the new location
  //      (temp path, write, upload) and touches the pane not at all;
  //   2. the pane is then mutated in ONE synchronous block — no await inside
  //      it, so nothing can observe a half-rebound pane;
  //   3. the old temp file is deleted only afterwards, and only if the pane
  //      owned one.
  //
  // A failure at any point in (1) leaves the pane on its OLD binding, dirty,
  // with the old temp file untouched — because that file is still the only
  // saved copy of the user's edit.

  function basename(p) {
    const segments = String(p || '').split('/').filter(Boolean);
    return segments.length ? segments[segments.length - 1] : '';
  }

  // What to call the pane's current location in a failure message.
  //
  // The untitled case is not a fallback for "we could not work it out" — it is
  // the whole of an untitled buffer's first save, and by far the most common
  // way this message is seen. "its previous location" is a lie there: there is
  // no previous location, and the phrase reads as a reassurance that some
  // existing file was left alone. Say what actually happened instead — the
  // buffer is still only in the tab.
  function describeBinding(pane) {
    if (pane.remote) return `${pane.remote.hostLabel}:${pane.remote.remotePath}`;
    if (pane.filePath) return pane.filePath;
    return 'nowhere yet (never saved)';
  }

  // Recompose the tab's caption and tooltip from the pane's CURRENT identity.
  // The composition itself lives in tab-label.js, which createEditorTab also
  // uses — one formula, so a rebound tab and a freshly opened one cannot drift.
  function refreshTabLabel(pane) {
    const access = paneAccess();
    const labels = global.termlabEditorTabLabel;
    if (!access || typeof access.setTabLabel !== 'function' || !labels) return;
    const { label, tooltip } = labels.editorTabLabel(pane);
    access.setTabLabel(pane.tabId, label, tooltip);
  }

  function setPaneLanguage(view, filename) {
    const pane = global.termlabEditorPane;
    if (!pane || typeof pane.setLanguage !== 'function') return;
    pane.setLanguage(view, filename);
  }

  // Two editors on one path is the exact state focusExistingEditor exists to
  // prevent — each holds its own doc and the last save silently wins. Opening
  // cannot produce it; Save As could, by aiming a pane at a path another tab
  // already holds. Refuse before anything is written.
  function pathHeldByAnotherPane(pane, filePath) {
    // Same rule as focusExistingEditor: a pathless pane holds no path, and
    // "nowhere" is not a location two panes can collide on.
    if (!filePath) return false;
    let held = false;
    eachEditorPane((other) => {
      if (other !== pane && other.filePath && other.filePath === filePath) held = true;
    });
    return held;
  }

  // Everything fallible, then the atomic rebind. Never called directly —
  // saveAs owns the in-flight guard around it.
  async function writeElsewhere(pane, target) {
    return withFixedBarrier(pane, async (barrier) => {
      if (barrier.synchronizationError) {
        logLspError('flush before Save As', barrier.synchronizationError);
        throw barrier.synchronizationError;
      }
      return writeElsewhereAtBarrier(pane, target, barrier);
    });
  }

  async function writeElsewhereAtBarrier(pane, target, barrier) {
    const contents = barrier.contents;
    // Captured BEFORE anything is written: after the rebind, `pane.remote` is
    // the new binding and this file would be unreachable.
    const oldTemp = pane.remote && pane.filePath ? pane.filePath : null;

    let nextFilePath;
    let nextRemote;
    let displayName;
    let targetReservation = null;
    let targetCommitted = false;
    const bridge = lspBridge();
    const stateStore = lspState();
    const sourceDocument = barrier.document;
    if (target.scope === 'remote') {
      nextRemote = {
        paneId: target.paneId,
        remotePath: target.remotePath,
        hostLabel: target.hostLabel,
      };
      displayName = basename(target.remotePath);
      // Deterministic per (host, remote path) — the same function that gives a
      // remote file its identity when it is opened, so a file saved here and
      // later reopened resolves to this same tab.
      nextFilePath = await invoke('editor_temp_path', {
        hostLabel: target.hostLabel,
        remotePath: target.remotePath,
      });
    } else {
      nextRemote = null;
      if (bridge && typeof bridge.reserveDocument === 'function') {
        targetReservation = await bridge.reserveDocument(target.path);
        if (!targetReservation || targetReservation.kind !== 'reserved') {
          if (targetReservation && targetReservation.kind === 'focusOwner') {
            await bridge.focusOwner(targetReservation);
          }
          throw new Error(`"${basename(target.path)}" is open in another tab; close it before saving over it.`);
        }
      }
      nextFilePath = targetReservation && targetReservation.canonicalPath
        ? targetReservation.canonicalPath
        : target.path;
      displayName = basename(target.path);
    }

    try {
      if (pathHeldByAnotherPane(pane, nextFilePath)) {
        throw new Error(`"${displayName}" is open in another tab; close it before saving over it.`);
      }

      await invoke('editor_write_file', { path: nextFilePath, contents });
      if (nextRemote) {
        await uploadTo(nextRemote, nextFilePath);
      }

      // Commit ownership after every target-side failure point, while the
      // pane still has its old identity. Any failure here therefore leaves
      // both the source binding and buffer intact.
      if (targetReservation && bridge) {
        if (sourceDocument) {
          const transferred = await bridge.transferDocument(
            sourceDocument.documentId,
            targetReservation.reservationId,
            pane.paneId,
          );
          targetCommitted = true;
          await attachOpenedDocument(pane, transferred, contents).catch((error) => {
            admissionFor(pane).desynchronized = true;
            logLspError('reconcile transferred document', error);
          });
        } else {
          const opened = await bridge.openDocument(
            targetReservation.reservationId,
            pane.paneId,
            contents,
            languageIdFor(nextFilePath),
          );
          targetCommitted = true;
          await attachOpenedDocument(pane, opened, contents).catch((error) => {
            admissionFor(pane).desynchronized = true;
            logLspError('reconcile saved document', error);
          });
        }
      } else if (nextRemote && sourceDocument && bridge) {
        await bridge.closeDocument(sourceDocument.documentId);
        if (stateStore) stateStore.clear(pane);
      }

      // ----- the rebind: one synchronous block, no awaits -----
      pane.filePath = nextFilePath;
      pane.remote = nextRemote;
    // Same rule as writeOnce: only claim the buffer is saved if it still
    // matches the bytes that were written. A keystroke during the upload
    // leaves the rebound pane honestly dirty. This runs before the cosmetic
      // steps below so the dirty flag reflects the bytes that actually landed,
      // regardless of whether relabelling succeeds.
      if (pane.view && pane.view.state.doc.toString() === contents) {
        try {
          pane.view.termlabResetDirty();
        } catch (error) {
          console.error('Save As: resetting the committed pane dirty state failed', error);
        }
      }
    // The write (and upload, if any) already succeeded once execution
    // reaches here — announce that now, not after the cosmetic steps below.
      if (nextRemote) {
        try {
          toastSuccess('Saved', `${nextRemote.hostLabel}:${nextRemote.remotePath}`);
        } catch (error) {
          console.error('Save As: announcing the committed remote save failed', error);
        }
      }
    // The bytes landed and the identity above is already committed: nothing
    // past this point may turn a real save into "Save As Failed". A bad tab
    // caption or language guess is cosmetic, so it is caught and logged
    // rather than thrown — otherwise a throw here would reject the whole
    // operation with a message claiming the rebind never happened, when it
    // already had.
      try {
        refreshTabLabel(pane);
        setPaneLanguage(pane.view, displayName);
      } catch (error) {
        console.error('Save As: relabelling the rebound pane failed', error);
      }
      // ----- end of the rebind -----

      // Only now, and only a temp file this pane owned. Before the rebind this
      // would delete the pane's own backing file while it still pointed at it.
      if (oldTemp && oldTemp !== nextFilePath) {
        try {
          invoke('editor_temp_cleanup', { path: oldTemp }).catch(() => {});
        } catch (_) {}
      }
    } catch (error) {
      if (targetReservation && !targetCommitted && bridge) {
        await bridge.releaseDocument(targetReservation.reservationId).catch(() => {});
      }
      throw error;
    }
  }

  /**
   * Save the pane's contents to a new location and rebind the pane to it.
   *
   *   target = { scope: 'local',  path }
   *          | { scope: 'remote', paneId, hostLabel, remotePath }
   *
   * Rejects if anything failed, having left the pane exactly as it was.
   */
  async function saveAs(pane, target) {
    if (!pane || pane.kind !== 'editor' || !pane.view) return;
    if (admissionFor(pane).closing) throw closeInProgressError();
    if (!target || (target.scope !== 'local' && target.scope !== 'remote')) {
      throw new Error(`Save As: unknown target scope ${target && target.scope}`);
    }

    // The existing guard, not a second queue: a pane never has more than one
    // write outstanding, so Save As waits for a save already running rather
    // than writing over it. Each turn of this loop waits for a write that is
    // genuinely in flight, so it cannot spin without work happening.
    //
    // The waited-for save's own outcome is deliberately swallowed: it wrote to
    // the OLD location, and saving elsewhere is exactly how a user recovers
    // from that failure. Its caller has already been told.
    let pending = savesInFlight.get(pane);
    while (pending) {
      await pending.catch(() => {});
      pending = savesInFlight.get(pane);
    }

    // Captured only now — after any save already in flight has drained, and
    // still before writeElsewhere runs — and used verbatim in the failure
    // toast below. A capture taken before the wait loop could go stale: a
    // concurrent Save As finishing during the wait rebinds the pane, and a
    // live `describeBinding(pane)` read from inside the catch happens AFTER
    // the promise has settled, by which point a rebind that got far enough
    // to mutate the pane before failing would already have overwritten
    // `pane.filePath`/`pane.remote` — naming the tab's NEW location while
    // claiming nothing was rebound.
    const where = describeBinding(pane);

    // Registered in the SAME map as an ordinary save, so a ⌘S landing during a
    // Save As joins this operation instead of writing the old file underneath
    // it. .finally is attached before the entry is stored, exactly as savePane
    // does, so a joiner can never re-find its own settled entry.
    const inFlight = writeElsewhere(pane, target).finally(() => {
      savesInFlight.delete(pane);
    });
    savesInFlight.set(pane, inFlight);
    try {
      await inFlight;
    } catch (error) {
      toastError(
        'Save As Failed',
        `${String(error)} — nothing was rebound: this tab still points at `
        + `${where} and still has unsaved changes.`,
      );
      throw error;
    }
  }

  global.termlabEditorService = {
    openLocalFile,
    openRemoteFile,
    openUntitled,
    saveActiveEditor,
    savePane,
    saveAs,
    confirmDirtyPanes,
    confirmAllDirty,
    eachEditorPane,
    discardRemoteTemp,
    cancelPendingChooser,
    uploadRemote,
    documentTransaction,
    flushDocument,
    closeDocument,
    closeDocuments,
    requestFeature,
  };
})(window);

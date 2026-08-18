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
    let found = null;
    eachEditorPane((pane) => {
      if (!found && pane.filePath === filePath) found = pane;
    });
    if (!found) return false;
    const access = paneAccess();
    access.activateTab(found.tabId);
    access.setFocusedPane(found.paneId);
    return true;
  }

  async function openLocalFile(filePath) {
    if (bundleMissing()) return;
    if (focusExistingEditor(filePath)) return;
    try {
      const contents = await invoke('editor_read_file', { path: filePath });
      createEditorTab({ filePath, contents, remote: null });
    } catch (error) {
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
    await runTransfer(() => invoke('transfer_download', { paneId, remotePath, localPath }));

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

  async function openScratch() {
    if (bundleMissing()) return;
    try {
      const [dir, existing] = await Promise.all([
        invoke('editor_scratch_dir'),
        invoke('editor_scratch_list'),
      ]);
      const name = global.termlabEditorScratch.nextScratchName(existing);
      const filePath = `${dir}/${name}`;
      await invoke('editor_write_file', { path: filePath, contents: '' });
      createEditorTab({ filePath, contents: '', remote: null });
    } catch (error) {
      toastError('Cannot Create Scratch', String(error));
    }
  }

  // One write. Rejects on failure so callers can decide what a failure means;
  // saveActiveEditor turns it into a toast, the close guards turn it into a
  // refusal to close.
  async function writeOnce(pane) {
    const contents = pane.view.state.doc.toString();
    await invoke('editor_write_file', { path: pane.filePath, contents });
    // For a remote file the local write is a staging step, not the save: what
    // "saved" means is that the bytes reached the host. Uploading BEFORE the
    // dirty reset is what makes a failed upload leave the pane dirty, so the
    // close guards still refuse to discard the tab and the temp file is not
    // deleted out from under an edit that never got off this machine. A local
    // pane has no remote and so is unaffected.
    if (pane.remote) await uploadRemote(pane);
    // `dirty` is a plain boolean, not a diff against the document, and the
    // update listener stops firing once it is set. So a keystroke that lands
    // between the snapshot above and this line is on screen but not in the
    // file — clearing the flag unconditionally would mark it saved and, once
    // the close guards read pane.dirty, discard it without a prompt. Only
    // reset when the buffer still matches the bytes that were written.
    if (pane.view && pane.view.state.doc.toString() === contents) {
      pane.view.termlabResetDirty();
    }
  }

  // The write currently in flight for a pane, if any. Two overlapping writes
  // for the same file can resolve out of order: the newer one lands first and
  // clears `dirty`, then the older one overwrites the file with stale bytes
  // and there is nothing left to say so. The close guards below read
  // `pane.dirty`, so that lie is what makes them close a tab over unsaved
  // text. A pane therefore never has more than one write outstanding.
  const savesInFlight = new WeakMap();

  // Save a specific pane. saveActiveEditor covers the keyboard path; the close
  // guards need to save panes that are not focused.
  async function savePane(pane, options) {
    if (!pane || pane.kind !== 'editor' || !pane.view) return;

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
      const name = String(pane.filePath || 'untitled').split('/').pop();
      const choice = await dialogs.confirmSave(name);
      if (choice === 'cancel') return false;
      if (choice !== 'save') continue;
      try {
        await savePane(pane);
      } catch (error) {
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
  function describeBinding(pane) {
    if (pane.remote) return `${pane.remote.hostLabel}:${pane.remote.remotePath}`;
    return pane.filePath || 'its previous location';
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
    let held = false;
    eachEditorPane((other) => {
      if (other !== pane && other.filePath === filePath) held = true;
    });
    return held;
  }

  // Everything fallible, then the atomic rebind. Never called directly —
  // saveAs owns the in-flight guard around it.
  async function writeElsewhere(pane, target) {
    const contents = pane.view.state.doc.toString();
    // Captured BEFORE anything is written: after the rebind, `pane.remote` is
    // the new binding and this file would be unreachable.
    const oldTemp = pane.remote && pane.filePath ? pane.filePath : null;

    let nextFilePath;
    let nextRemote;
    let displayName;
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
      nextFilePath = target.path;
      displayName = basename(target.path);
    }

    if (pathHeldByAnotherPane(pane, nextFilePath)) {
      throw new Error(`"${displayName}" is open in another tab; close it before saving over it.`);
    }

    await invoke('editor_write_file', { path: nextFilePath, contents });
    if (nextRemote) {
      try {
        await uploadTo(nextRemote, nextFilePath);
      } catch (error) {
        // The staged file at nextFilePath is deliberately NOT deleted. It may
        // be the temp path of a remote file another tab holds (editor_temp_path
        // is a pure function of host+path), and editor_temp_cleanup also
        // removes the parent directories it empties — so deleting here could
        // take out a file this pane never owned. Litter in the temp root is
        // the cheaper mistake.
        throw error;
      }
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
      pane.view.termlabResetDirty();
    }
    // The write (and upload, if any) already succeeded once execution
    // reaches here — announce that now, not after the cosmetic steps below.
    if (nextRemote) toastSuccess('Saved', `${nextRemote.hostLabel}:${nextRemote.remotePath}`);
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
      invoke('editor_temp_cleanup', { path: oldTemp }).catch(() => {});
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
    openScratch,
    saveActiveEditor,
    savePane,
    saveAs,
    confirmDirtyPanes,
    confirmAllDirty,
    eachEditorPane,
    discardRemoteTemp,
    uploadRemote,
  };
})(window);

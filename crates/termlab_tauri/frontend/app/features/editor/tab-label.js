// Editor tab labelling.
//
// A remote file's tab has to say which host it lives on: the SFTP panel that
// revealed the host closes as soon as the file opens, and after that nothing
// else on screen names it. Pure function, no DOM — a later rebind (e.g. Save
// As) calls this again on the same pane to refresh its label and tooltip, so
// it must not assume it only ever runs at tab-creation time.
(function initTermLabEditorTabLabel(global) {
  'use strict';

  function basename(p) {
    const segments = String(p || '').split('/').filter(Boolean);
    return segments.length ? segments[segments.length - 1] : '';
  }

  function editorTabLabel(pane) {
    if (!pane) return { label: 'untitled', tooltip: '' };

    const remote = pane.remote;
    if (remote) {
      // The basename comes from the remote path, not pane.filePath — that's
      // the local temp file the remote copy was downloaded to, an
      // implementation detail the user never sees.
      const name = basename(remote.remotePath) || 'untitled';
      return {
        label: `${name} — ${remote.hostLabel}`,
        tooltip: `${remote.hostLabel}:${remote.remotePath}`,
      };
    }

    // An untitled buffer has no file on disk to take a basename from, so its
    // name is a per-window sequence number allocated when the tab was created
    // (editor-service's openUntitled) and carried on the pane. The first one
    // is unnumbered — Notepad's naming, and what the Save As chooser prefills.
    //
    // Keyed on the sequence number rather than on `!pane.filePath` alone: a
    // pathless pane WITHOUT one is not an untitled buffer but a defensive
    // call, and keeps the lowercase fallback below.
    const seq = Number(pane.untitledSeq);
    if (!pane.filePath && Number.isInteger(seq) && seq >= 1) {
      return {
        label: seq > 1 ? `Untitled-${seq}` : 'Untitled',
        tooltip: 'Unsaved',
      };
    }

    const name = basename(pane.filePath) || 'untitled';
    return { label: name, tooltip: pane.filePath || '' };
  }

  global.termlabEditorTabLabel = { editorTabLabel };
})(window);

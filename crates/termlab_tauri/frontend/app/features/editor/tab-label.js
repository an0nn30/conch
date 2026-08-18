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

    const name = basename(pane.filePath) || 'untitled';
    return { label: name, tooltip: pane.filePath || '' };
  }

  global.termlabEditorTabLabel = { editorTabLabel };
})(window);

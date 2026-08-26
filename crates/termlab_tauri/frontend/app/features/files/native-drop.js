// OS file drops onto the remote pane (Task 8).
//
// Tauri v2 delivers native (Finder/Explorer) file drops as window-level
// events carrying real filesystem paths — `tauri://drag-drop` (paths +
// position), plus `tauri://drag-enter`/`drag-over`/`drag-leave` for the
// hover feedback. The DOM's own drag events never see real paths for an OS
// drop (that's what Task 7's pane-view.js intra-app drag/drop, with its
// synthetic `application/x-termlab-entry` payload, is for) — this module is
// the OS-drop counterpart, consumed by files-panel.js's init.
//
// Both helpers here are pure functions of their arguments (no DOM, no
// `invoke`, no module-level state) so they can be exercised in a bare VM
// context — see scripts/tests/test_files_dnd.mjs's native-drop section,
// which loads only this file.
(function initTermLabNativeDrop(global) {
  'use strict';

  function basenameOfPath(p) {
    const trimmed = String(p || '').replace(/\/+$/, '');
    const idx = trimmed.lastIndexOf('/');
    return idx >= 0 ? trimmed.slice(idx + 1) : trimmed;
  }

  function joinPath(base, name) {
    const trimmed = String(base || '').replace(/\/+$/, '');
    return (trimmed || '') + '/' + name;
  }

  // Decide what a native drag/drop event at `position` (already scaled to
  // LOGICAL px — see files-panel.js's devicePixelRatio note) should do
  // against the remote pane root's `paneRect` (a getBoundingClientRect()-
  // shaped object: left/top/right/bottom).
  //
  // Hit test is left/top-inclusive, right/bottom-exclusive — the same
  // half-open convention getBoundingClientRect()'s own box implies (a point
  // exactly on the right or bottom edge belongs to whatever sits just past
  // it, not this box). Pinned here as the one rule both this function and
  // its tests use.
  //
  // Returns 'ignore' (position missing, or outside the rect — drops
  // elsewhere in the window are silently ignored per spec), 'no-session'
  // (inside the rect, but the remote pane has no active session — caller
  // shows the existing "not connected" toast), or 'accept' (inside the rect,
  // session active — caller routes the drop).
  function resolveNativeDrop(position, paneRect, sessionActive) {
    if (!position || !paneRect) return 'ignore';
    const { x, y } = position;
    if (typeof x !== 'number' || typeof y !== 'number') return 'ignore';
    const hit = x >= paneRect.left && x < paneRect.right
      && y >= paneRect.top && y < paneRect.bottom;
    if (!hit) return 'ignore';
    return sessionActive ? 'accept' : 'no-session';
  }

  // Route each dropped OS path into the remote pane: stat it to tell file
  // from directory, then hand it to the matching transfer dep exactly like
  // the equivalent row-menu/DOM-drop action would (files-panel.js's
  // doUpload / doUploadFolder / onDropEntries) — directory -> transferRecursive
  // with the destination CONTAINER as-is (the backend appends the source's
  // own basename); file -> transferUpload with the destination filename
  // pre-joined, mirroring the single-file path exactly.
  //
  // `deps`:
  //   statPath(path)              -> Promise<{ isDir }>
  //   transferRecursive(paneId, sourcePath, destPath) -> Promise
  //   transferUpload(paneId, sourcePath, destPath)     -> Promise
  //   targetPaneId, targetPath    -> the remote pane's id / current directory
  //   toast                       -> { error, info } (both optional)
  //
  // Paths are routed sequentially and independently: a stat failure or a
  // rejected transfer for one path reports via toast.error and moves on to
  // the next path rather than aborting the whole drop.
  async function routeNativeDropPaths(paths, deps) {
    const list = Array.isArray(paths) ? paths : [];
    const opts = deps || {};
    const { statPath, transferRecursive, transferUpload, targetPaneId, targetPath, toast } = opts;

    for (let i = 0; i < list.length; i += 1) {
      const sourcePath = list[i];
      const name = basenameOfPath(sourcePath);
      let isDir = false;
      try {
        const stat = typeof statPath === 'function'
          ? await statPath(sourcePath)
          : await Promise.reject(new Error('native-drop: no statPath dep supplied'));
        isDir = !!(stat && stat.isDir);
      } catch (err) {
        if (toast && typeof toast.error === 'function') {
          toast.error('Upload Failed', `${name}: ${String(err && err.message ? err.message : err)}`);
        }
        continue; // eslint-disable-line no-continue
      }

      try {
        if (isDir) {
          if (typeof transferRecursive !== 'function') {
            throw new Error('native-drop: no transferRecursive dep supplied');
          }
          await transferRecursive(targetPaneId, sourcePath, targetPath);
          if (toast && typeof toast.info === 'function') {
            toast.info('Folder transfer started', name);
          }
        } else {
          if (typeof transferUpload !== 'function') {
            throw new Error('native-drop: no transferUpload dep supplied');
          }
          const destPath = joinPath(targetPath, name);
          await transferUpload(targetPaneId, sourcePath, destPath);
        }
      } catch (err) {
        if (toast && typeof toast.error === 'function') {
          toast.error('Upload Failed', String(err && err.message ? err.message : err));
        }
      }
    }
  }

  global.termlabNativeDrop = {
    resolveNativeDrop,
    routeNativeDropPaths,
  };
})(window);

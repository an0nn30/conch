// OS file drops (Task 8, plus the Task 8 fix-round terminal-collision fix).
//
// Tauri v2 delivers native (Finder/Explorer) file drops as window-level
// events carrying real filesystem paths — `tauri://drag-drop` (paths +
// position), plus `tauri://drag-enter`/`drag-over`/`drag-leave` for the
// hover feedback. The DOM's own drag events never see real paths for an OS
// drop (that's what Task 7's pane-view.js intra-app drag/drop, with its
// synthetic `application/x-termlab-entry` payload, is for) — this module is
// the OS-drop counterpart.
//
// Two consumers share this module's position/hit-test helpers:
// files-panel.js (the remote pane's own drop target) and
// core/dragdrop-runtime.js's window-level `onDragDropEvent` terminal
// handler (which must hit-test against the terminal host so an OS drop
// elsewhere in the window — e.g. onto the SFTP remote pane — doesn't ALSO
// paste the dropped paths into a live shell prompt). Both load this script
// first (see index.html's script order), so both reuse the SAME
// scaleNativeDropPosition/pointInRect rather than keeping their own copies.
//
// Every export here is a pure function of its arguments (no DOM beyond the
// rect-shaped object passed in, no `invoke`, no module-level state) so all
// of it can be exercised in a bare VM context — see
// scripts/tests/test_files_dnd.mjs's native-drop section, which loads only
// this file.
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

  // Tauri v2 reports drag-drop positions in PHYSICAL pixels, while
  // getBoundingClientRect() (what every hit-test here compares against) is
  // in LOGICAL/CSS pixels — on a retina display (devicePixelRatio 2) a raw
  // physical position would land roughly twice as far right/down as it
  // should. Every consumer must scale through this before hit-testing.
  // Returns null when `position` isn't a well-formed {x, y} pair.
  function scaleNativeDropPosition(position) {
    if (!position || typeof position.x !== 'number' || typeof position.y !== 'number') return null;
    const ratio = (typeof global !== 'undefined' && global.devicePixelRatio) || 1;
    return { x: position.x / ratio, y: position.y / ratio };
  }

  // The raw rectangle hit-test, shared by resolveNativeDrop below and by
  // any other caller (dragdrop-runtime.js) that just needs "is this
  // (already-scaled, logical-px) point inside this
  // getBoundingClientRect()-shaped rect", without the SFTP-specific
  // accept/no-session/ignore vocabulary resolveNativeDrop layers on top.
  //
  // Left/top-inclusive, right/bottom-exclusive — the same half-open
  // convention getBoundingClientRect()'s own box implies (a point exactly on
  // the right or bottom edge belongs to whatever sits just past it, not this
  // box). Pinned here as the one rule every hit-test in this module, and
  // every caller's tests, uses.
  function pointInRect(position, rect) {
    if (!position || !rect) return false;
    const { x, y } = position;
    if (typeof x !== 'number' || typeof y !== 'number') return false;
    return x >= rect.left && x < rect.right && y >= rect.top && y < rect.bottom;
  }

  // Decide what a native drag/drop event at `position` (already scaled to
  // LOGICAL px via scaleNativeDropPosition above) should do against the
  // remote pane root's `paneRect` (a getBoundingClientRect()-shaped object).
  //
  // Returns 'ignore' (position missing, or outside the rect — drops
  // elsewhere in the window are silently ignored per spec), 'no-session'
  // (inside the rect, but the remote pane has no active session — caller
  // shows the existing "not connected" toast), or 'accept' (inside the rect,
  // session active — caller routes the drop).
  function resolveNativeDrop(position, paneRect, sessionActive) {
    if (!pointInRect(position, paneRect)) return 'ignore';
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

  // Fan one `onDragDropEvent` stream out to the hover/leave/drop handlers.
  // Tauri v2 delivers OS drag events ONLY through onDragDropEvent — a plain
  // window `listen('tauri://drag-*')` registers against a different event
  // target and never fires (the live-app bug that motivated this seam). The
  // event object passes through untouched so handlers keep reading
  // `payload.position` / `payload.paths`.
  function dispatchNativeDragDropEvent(event, handlers) {
    const type = event && event.payload ? event.payload.type : null;
    if (!handlers || !type) return undefined;
    // The handler's return value passes through so async drop routing stays
    // awaitable by callers (the test harness awaits it; production ignores it).
    if ((type === 'enter' || type === 'over') && typeof handlers.hover === 'function') {
      return handlers.hover(event);
    }
    if (type === 'leave' && typeof handlers.leave === 'function') {
      return handlers.leave(event);
    }
    if (type === 'drop' && typeof handlers.drop === 'function') {
      return handlers.drop(event);
    }
    return undefined;
  }

  global.termlabNativeDrop = {
    scaleNativeDropPosition,
    pointInRect,
    resolveNativeDrop,
    routeNativeDropPaths,
    dispatchNativeDragDropEvent,
  };
})(window);

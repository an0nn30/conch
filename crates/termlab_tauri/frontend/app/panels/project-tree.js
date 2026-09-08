// The project tree — a single-pane, lazily-listed view of one directory.
//
// Its own module rather than more code in the already-large files panel: the
// panel decides WHICH view a window gets (project tree or the dual-pane
// local+SFTP explorer), and this decides what a tree is.
//
// Listing goes through the existing `local_list_dir` command and its
// `FileEntry` shape — there is no new listing backend. Lazy: a directory is
// listed the first time it is expanded and the listing is then cached, so
// collapsing and re-expanding costs nothing. There is no filesystem watcher
// in v1; freshness comes from explicit refresh triggers the panel owns.
//
// Every name that came off the filesystem is written with textContent. The
// only innerHTML in this file is the icon markup from window.fileIcons, which
// is repo-authored SVG.
(function initTermLabProjectTree(global) {
  'use strict';

  function el(tag, className) {
    const node = global.document.createElement(tag);
    if (className) node.className = className;
    return node;
  }

  function joinPath(base, name) {
    if (!base || base === '/') return '/' + name;
    return base.endsWith('/') ? base + name : base + '/' + name;
  }

  // Directories first, then alphabetical, case-insensitive. Uses the same
  // toLowerCase()+localeCompare() form as features/files/pane-store.js's
  // sortEntries so an accented file name orders identically in the project
  // tree and in the dual-pane explorer, rather than drifting because one
  // pane used code-unit comparison and the other used locale collation.
  function sortEntries(entries, showHidden) {
    const visible = (entries || []).filter(
      (item) => showHidden || !String((item && item.name) || '').startsWith('.'),
    );
    return visible.slice().sort((a, b) => {
      if (!!a.is_dir !== !!b.is_dir) return a.is_dir ? -1 : 1;
      return String(a.name || '').toLowerCase().localeCompare(String(b.name || '').toLowerCase());
    });
  }

  function iconHtml(name, isDir) {
    return global.fileIcons && typeof global.fileIcons.iconFor === 'function'
      ? global.fileIcons.iconFor(name, isDir, false)
      : '';
  }

  // Whether `node` is `ancestor` or is nested inside it — used to tell
  // whether keyboard focus currently lives inside the tree's list, since a
  // DOM replace (see render()) would otherwise silently drop focus to
  // <body> and keyboard navigation would stop reaching the container.
  function isWithin(node, ancestor) {
    let current = node;
    while (current) {
      if (current === ancestor) return true;
      current = current.parentNode;
    }
    return false;
  }

  function create(options) {
    const opts = options || {};
    const invoke = opts.invoke;
    const root = String(opts.root || '');
    const onOpenFile = typeof opts.onOpenFile === 'function'
      ? opts.onOpenFile
      : (filePath) => {
        const service = global.termlabEditorService;
        if (service && typeof service.openLocalFile === 'function') service.openLocalFile(filePath);
      };
    const onContextMenu = typeof opts.onContextMenu === 'function' ? opts.onContextMenu : () => {};
    const toastError = typeof opts.toastError === 'function'
      ? opts.toastError
      : (title, body) => { if (global.toast) global.toast.error(title, body); };

    let showHidden = opts.showHidden === true;
    let missing = false;
    let gitStatus = null;
    let activePath = null;
    let pending = Promise.resolve();
    let destroyed = false;

    // path -> FileEntry[] for every directory currently known-good, the set
    // of directories currently open, and the set of directories whose most
    // recent listing attempt failed (rendered distinctly so an error is
    // never confused with a genuinely empty folder). A failed listing is
    // deliberately NOT cached in `listings` — the next expand retries
    // instead of being stuck on a permanently-empty directory.
    const listings = new Map();
    const expanded = new Set();
    const errored = new Set();
    // dirPath -> in-flight listDir() promise, so that several expands of the
    // same directory issued in the same tick (e.g. a fast double-click, or
    // expand() called from both a click and a keyboard handler) share one
    // `local_list_dir` call instead of each firing its own.
    const inFlight = new Map();
    let rowNodes = [];

    const element = el('div', 'tl-project-tree');
    const toolbar = el('div', 'tl-project-tree__toolbar');
    const title = el('span', 'tl-project-tree__title');
    title.textContent = root;
    title.title = root;
    const refreshButton = el('button', 'tl-project-tree__button');
    refreshButton.type = 'button';
    refreshButton.textContent = 'Refresh';
    refreshButton.setAttribute('aria-label', 'Refresh the project tree');
    refreshButton.addEventListener('click', () => { refreshAllTracked(); });
    const hiddenButton = el('button', 'tl-project-tree__button');
    hiddenButton.type = 'button';
    hiddenButton.textContent = 'Hidden';
    hiddenButton.setAttribute('aria-pressed', showHidden ? 'true' : 'false');
    hiddenButton.addEventListener('click', () => {
      showHidden = !showHidden;
      hiddenButton.setAttribute('aria-pressed', showHidden ? 'true' : 'false');
      render();
    });
    toolbar.appendChild(title);
    toolbar.appendChild(hiddenButton);
    toolbar.appendChild(refreshButton);

    // A fixed slot above the list so the banner (Task 7) and the missing-root
    // state can appear and disappear without the tree reordering anything.
    // `missingHost` is a permanent child of `noticeHost`, mounted once here:
    // render() only ever replaces missingHost's OWN children, never touches
    // noticeHost directly, so whatever Task 7 mounts alongside it (e.g. a
    // trust banner) survives every expand/collapse/refresh/toggle.
    const noticeHost = el('div', 'tl-project-tree__notice-host');
    const missingHost = el('div', 'tl-project-tree__missing-slot');
    noticeHost.appendChild(missingHost);

    const list = el('div', 'tl-project-tree__list tl-scroll');
    list.setAttribute('role', 'tree');
    list.setAttribute('aria-label', 'Project files');
    list.setAttribute('tabindex', '0');

    element.appendChild(toolbar);
    element.appendChild(noticeHost);
    element.appendChild(list);

    // Fetches one directory's listing, de-duplicating concurrent callers and
    // never caching a failure. Returns a Promise<boolean> rather than being
    // declared `async` so a same-tick second caller can be handed the exact
    // in-flight promise from `inFlight` before the first `invoke` call has
    // even resolved.
    function listDir(dirPath) {
      if (destroyed) return Promise.resolve(false);
      if (inFlight.has(dirPath)) return inFlight.get(dirPath);
      const attempt = (async () => {
        try {
          const entries = await invoke('local_list_dir', { path: dirPath });
          // Re-checked after the await: destroy() may have run while this
          // call was in flight. Its effects (toasting, and writing into
          // listings/errored/expanded) must not land on a tree that has
          // already declared those collections cleared and final.
          if (destroyed) return false;
          listings.set(dirPath, Array.isArray(entries) ? entries : []);
          errored.delete(dirPath);
          return true;
        } catch (error) {
          if (destroyed) return false;
          // Toast, collapse, and do not cache the empty result: the rest of
          // the tree keeps working, and the next expand of this directory
          // retries rather than being stuck showing "empty" forever.
          toastError('Cannot Read Folder', dirPath + ': ' + String(error));
          listings.delete(dirPath);
          expanded.delete(dirPath);
          errored.add(dirPath);
          return false;
        } finally {
          inFlight.delete(dirPath);
        }
      })();
      inFlight.set(dirPath, attempt);
      return attempt;
    }

    // Queues `promise` onto the tree's single settled()-observable chain.
    // Once destroy() has run there is nothing left to observe or act on, so
    // a post-destroy caller is handed its promise back unqueued rather than
    // reviving `pending`.
    function track(promise) {
      if (destroyed) return promise;
      pending = pending.then(() => promise).catch(() => {});
      return promise;
    }

    async function expand(dirPath) {
      if (!listings.has(dirPath)) {
        const ok = await listDir(dirPath);
        if (destroyed || !ok) { render(); return; }
      }
      // Belt-and-braces: destroy() cannot actually interleave here (nothing
      // above yields once listings.has(dirPath) was already true), but the
      // guard keeps expanded.add() from ever running on a torn-down tree
      // regardless of how this function is reshaped later.
      if (destroyed) { render(); return; }
      expanded.add(dirPath);
      render();
    }

    function collapse(dirPath) {
      expanded.delete(dirPath);
      render();
    }

    async function refresh(dirPath) {
      if (!listings.has(dirPath) && !errored.has(dirPath)) return;
      await listDir(dirPath);
      render();
    }

    async function refreshAll() {
      const targets = listings.size ? Array.from(listings.keys()) : [root];
      for (const dirPath of targets) {
        await listDir(dirPath);
      }
      render();
    }

    // The toolbar button and the public handle share this one path so that
    // settled() actually covers a user-initiated refresh (rather than the
    // button firing an untracked, unobserved refreshAll() whose rejection
    // would go unhandled).
    function refreshAllTracked() {
      return track(refreshAll());
    }

    // The flattened, currently-visible rows, depth-first in display order.
    function buildNodes() {
      const out = [];
      const walk = (dirPath, depth) => {
        for (const item of sortEntries(listings.get(dirPath) || [], showHidden)) {
          const nodePath = joinPath(dirPath, item.name);
          out.push({
            path: nodePath,
            name: item.name,
            isDir: !!item.is_dir,
            parentPath: dirPath,
            depth,
          });
          if (item.is_dir && expanded.has(nodePath)) walk(nodePath, depth + 1);
        }
      };
      walk(root, 0);
      return out;
    }

    function gitStateFor(node) {
      const git = global.termlabProjectGit;
      if (!gitStatus || !git || typeof git.stateForPath !== 'function') return null;
      return git.stateForPath(gitStatus, root, node.path, node.isDir);
    }

    function renderRow(node) {
      const row = el('div', 'tl-project-tree__row');
      row.setAttribute('role', 'treeitem');
      row.setAttribute('data-tree-path', node.path);
      row.setAttribute('aria-level', String(node.depth + 1));
      row.setAttribute('tabindex', '-1');
      row.style.paddingLeft = (node.depth * 12 + 4) + 'px';

      const twisty = el('span', 'tl-project-tree__twisty');
      twisty.setAttribute('aria-hidden', 'true');
      const hasError = node.isDir && errored.has(node.path);
      if (node.isDir) {
        const open = expanded.has(node.path);
        twisty.textContent = open ? '▾' : '▸';
        row.setAttribute('aria-expanded', open ? 'true' : 'false');
        if (hasError) row.setAttribute('data-state', 'error');
      } else {
        twisty.textContent = '';
      }

      const icon = el('span', 'tl-project-tree__icon');
      icon.setAttribute('aria-hidden', 'true');
      icon.innerHTML = iconHtml(node.name, node.isDir);

      const label = el('span', 'tl-project-tree__label');
      label.textContent = node.name;

      // The git-state and error suffixes both compose into one aria-label:
      // the failure is a spoken word ("failed to load"), not just the
      // data-state/colour pairing, so a screen reader can tell a directory
      // that errored apart from one that's simply empty.
      const gitState = gitStateFor(node);
      const labelParts = [node.name];
      if (gitState) {
        row.setAttribute('data-git-state', gitState);
        labelParts.push(gitState);
      }
      if (hasError) labelParts.push('failed to load');
      if (labelParts.length > 1) row.setAttribute('aria-label', labelParts.join(', '));

      row.appendChild(twisty);
      row.appendChild(icon);
      row.appendChild(label);
      row.title = node.path;
      row._node = node;
      return row;
    }

    function renderMissing() {
      const notice = el('div', 'tl-project-tree__missing');
      notice.setAttribute('role', 'note');
      const text = el('span', 'tl-project-tree__missing-text');
      text.textContent = 'This project folder is missing: ' + root;
      const reopen = el('button', 'tl-project-tree__button');
      reopen.type = 'button';
      reopen.textContent = 'Choose another folder…';
      reopen.setAttribute('data-tree-action', 'reopen');
      reopen.addEventListener('click', () => {
        if (typeof opts.onReopen === 'function') opts.onReopen();
      });
      notice.appendChild(text);
      notice.appendChild(reopen);
      return notice;
    }

    function render() {
      if (destroyed) return;
      // A real DOM drops focus to <body> when the focused node is removed.
      // Both replaced regions below need the same before/after focus check:
      // list.replaceChildren() removes every row, and missingHost's rebuild
      // removes the "Choose another folder…" button even when `missing`
      // stays true across an unrelated render (setShowHidden, a git-status
      // poll) — so a row OR that reopen button holding focus must be tracked
      // here and re-focused on its replacement afterward.
      const hadFocus = isWithin(global.document.activeElement, list);
      const hadMissingFocus = isWithin(global.document.activeElement, missingHost);

      const missingNotice = missing ? renderMissing() : null;
      missingHost.replaceChildren(...(missingNotice ? [missingNotice] : []));

      const nodes = missing ? [] : buildNodes();
      const built = nodes.map(renderRow);
      list.replaceChildren(...built);
      rowNodes = nodes;
      if (activePath && !nodes.some((n) => n.path === activePath)) activePath = null;
      applyActive(built);

      if (hadFocus) {
        const idx = rowNodes.findIndex((n) => n.path === activePath);
        const target = idx >= 0 ? built[idx] : list;
        if (target && typeof target.focus === 'function') target.focus();
      }
      if (hadMissingFocus && missingNotice) {
        const reopen = missingNotice.querySelector('[data-tree-action="reopen"]');
        if (reopen && typeof reopen.focus === 'function') reopen.focus();
      }
    }

    function applyActive(built) {
      const rows = built || Array.from(list.children);
      rows.forEach((row) => {
        const isActive = !!row._node && row._node.path === activePath;
        row.setAttribute('tabindex', isActive ? '0' : '-1');
        row.classList.toggle('is-active', isActive);
      });
    }

    // Moves by row INDEX rather than building a `[data-tree-path="..."]`
    // selector from the (untrusted) file name: a name containing `"` would
    // both mismatch and throw a DOMException out of the keydown handler in
    // a real browser. `list.children` is rebuilt in the same order as
    // `rowNodes` on every render(), so indexing is exact and never touches
    // a selector string built from file-system data.
    function moveTo(index) {
      if (!rowNodes.length) return;
      const clamped = Math.min(Math.max(index, 0), rowNodes.length - 1);
      activePath = rowNodes[clamped].path;
      applyActive();
      const row = list.children[clamped];
      if (row && typeof row.focus === 'function') row.focus();
    }

    function indexOfActive() {
      return rowNodes.findIndex((n) => n.path === activePath);
    }

    function activate(node) {
      if (!node) return;
      activePath = node.path;
      // Applied here (not left to render()) so a click on a plain FILE row
      // — which never triggers a render() — still highlights and becomes
      // the roving tabindex stop.
      applyActive();
      if (node.isDir) {
        if (expanded.has(node.path)) collapse(node.path);
        else track(expand(node.path));
        return;
      }
      onOpenFile(node.path);
    }

    // Capture-phase discipline lives with the panel's router registration; the
    // tree owns only what happens once a key reaches its list.
    list.addEventListener('keydown', (event) => {
      const at = indexOfActive();
      const key = event.key;
      if (key === 'ArrowDown') { moveTo(at < 0 ? 0 : at + 1); event.preventDefault(); return; }
      if (key === 'ArrowUp') { moveTo(at < 0 ? 0 : at - 1); event.preventDefault(); return; }
      if (key === 'Home') { moveTo(0); event.preventDefault(); return; }
      if (key === 'End') { moveTo(rowNodes.length - 1); event.preventDefault(); return; }
      if (key === 'ArrowRight') {
        const node = rowNodes[at < 0 ? 0 : at];
        if (node && node.isDir && !expanded.has(node.path)) { activePath = node.path; track(expand(node.path)); }
        else moveTo(at < 0 ? 0 : at + 1);
        event.preventDefault();
        return;
      }
      if (key === 'ArrowLeft') {
        const node = rowNodes[at < 0 ? 0 : at];
        if (node && node.isDir && expanded.has(node.path)) {
          activePath = node.path;
          collapse(node.path);
        } else if (node && node.parentPath && node.parentPath !== root) {
          // ARIA tree pattern: Left on a collapsed directory or a leaf moves
          // to its PARENT row, not merely the previous visible row (which,
          // for anything past a folder's first child, is a sibling).
          const parentIndex = rowNodes.findIndex((n) => n.path === node.parentPath);
          if (parentIndex >= 0) moveTo(parentIndex);
        }
        event.preventDefault();
        return;
      }
      if (key === 'Enter') {
        activate(rowNodes[at < 0 ? 0 : at]);
        event.preventDefault();
      }
    });

    list.addEventListener('click', (event) => {
      const row = event.target && typeof event.target.closest === 'function'
        ? event.target.closest('[data-tree-path]')
        : event.target;
      if (!row || !row._node) return;
      activate(row._node);
    });

    list.addEventListener('contextmenu', (event) => {
      const row = event.target && typeof event.target.closest === 'function'
        ? event.target.closest('[data-tree-path]')
        : event.target;
      if (!row || !row._node) return;
      if (typeof event.preventDefault === 'function') event.preventDefault();
      activePath = row._node.path;
      applyActive();
      const node = row._node;
      onContextMenu(event, {
        path: node.path, name: node.name, isDir: node.isDir, parentPath: node.parentPath,
      });
    });

    return {
      element,
      expand: (p) => track(expand(p)),
      collapse,
      refresh: (p) => track(refresh(p)),
      refreshAll: refreshAllTracked,
      settled: () => pending,
      activePath: () => activePath,
      rows: () => rowNodes.slice(),
      setGitStatus(snapshot) { gitStatus = snapshot || null; render(); },
      setMissing(value) { missing = value === true; render(); },
      setShowHidden(value) {
        showHidden = value === true;
        hiddenButton.setAttribute('aria-pressed', showHidden ? 'true' : 'false');
        render();
      },
      focus() { if (typeof list.focus === 'function') list.focus(); },
      noticeHost,
      // Terminal: once destroyed, render()/listDir()/track() all become
      // no-ops (or hand back an unqueued promise), so a stray reference held
      // by a caller (e.g. a pane switch that raced a window close) cannot
      // reach into IPC or repaint a detached tree.
      destroy() {
        destroyed = true;
        listings.clear();
        expanded.clear();
        errored.clear();
        inFlight.clear();
        rowNodes = [];
        if (element.parentNode) element.remove();
      },
    };
  }

  global.termlabProjectTree = { create, sortEntries, joinPath };
})(window);

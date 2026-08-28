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

  // Directories first, then alphabetical, case-insensitive — the same rule
  // local_fs.rs::sort_entries applies server-side, restated here because the
  // hidden-files filter runs in the same pass.
  function sortEntries(entries, showHidden) {
    const visible = (entries || []).filter(
      (item) => showHidden || !String((item && item.name) || '').startsWith('.'),
    );
    return visible.slice().sort((a, b) => {
      if (!!a.is_dir !== !!b.is_dir) return a.is_dir ? -1 : 1;
      const an = String(a.name || '').toLowerCase();
      const bn = String(b.name || '').toLowerCase();
      if (an < bn) return -1;
      if (an > bn) return 1;
      return 0;
    });
  }

  function iconHtml(name, isDir) {
    return global.fileIcons && typeof global.fileIcons.iconFor === 'function'
      ? global.fileIcons.iconFor(name, isDir, false)
      : '';
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

    // path -> FileEntry[] for every directory ever listed, and the set of
    // directories currently open. Two maps rather than one node graph: the
    // flat pair is what makes refreshAll a loop over `listings.keys()`.
    const listings = new Map();
    const expanded = new Set();
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
    refreshButton.addEventListener('click', () => { refreshAll(); });
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
    const noticeHost = el('div', 'tl-project-tree__notice-host');

    const list = el('div', 'tl-project-tree__list tl-scroll');
    list.setAttribute('role', 'tree');
    list.setAttribute('aria-label', 'Project files');
    list.setAttribute('tabindex', '0');

    element.appendChild(toolbar);
    element.appendChild(noticeHost);
    element.appendChild(list);

    async function listDir(dirPath) {
      try {
        const entries = await invoke('local_list_dir', { path: dirPath });
        listings.set(dirPath, Array.isArray(entries) ? entries : []);
        return true;
      } catch (error) {
        // Toast and collapse: the rest of the tree keeps working, which is the
        // whole point of listing lazily and per-directory.
        toastError('Cannot Read Folder', dirPath + ': ' + String(error));
        listings.set(dirPath, []);
        expanded.delete(dirPath);
        return false;
      }
    }

    function track(promise) {
      pending = pending.then(() => promise).catch(() => {});
      return promise;
    }

    async function expand(dirPath) {
      if (!listings.has(dirPath)) {
        const ok = await listDir(dirPath);
        if (!ok) { render(); return; }
      }
      expanded.add(dirPath);
      render();
    }

    function collapse(dirPath) {
      expanded.delete(dirPath);
      render();
    }

    async function refresh(dirPath) {
      if (!listings.has(dirPath)) return;
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
      if (node.isDir) {
        const open = expanded.has(node.path);
        twisty.textContent = open ? '▾' : '▸';
        row.setAttribute('aria-expanded', open ? 'true' : 'false');
      } else {
        twisty.textContent = '';
      }

      const icon = el('span', 'tl-project-tree__icon');
      icon.setAttribute('aria-hidden', 'true');
      icon.innerHTML = iconHtml(node.name, node.isDir);

      const label = el('span', 'tl-project-tree__label');
      label.textContent = node.name;

      const state = gitStateFor(node);
      if (state) {
        row.setAttribute('data-git-state', state);
        row.setAttribute('aria-label', node.name + ', ' + state);
      }

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
      const notices = [];
      if (missing) notices.push(renderMissing());
      noticeHost.replaceChildren(...notices);

      const nodes = missing ? [] : buildNodes();
      const built = nodes.map(renderRow);
      list.replaceChildren(...built);
      rowNodes = nodes;
      if (activePath && !nodes.some((n) => n.path === activePath)) activePath = null;
      applyActive(built);
    }

    function applyActive(built) {
      const rows = built || Array.from(list.children);
      rows.forEach((row) => {
        const isActive = !!row._node && row._node.path === activePath;
        row.setAttribute('tabindex', isActive ? '0' : '-1');
        row.classList.toggle('is-active', isActive);
      });
    }

    function moveTo(index) {
      if (!rowNodes.length) return;
      const clamped = Math.min(Math.max(index, 0), rowNodes.length - 1);
      activePath = rowNodes[clamped].path;
      applyActive();
      const row = list.querySelector('[data-tree-path="' + activePath + '"]');
      if (row && typeof row.focus === 'function') row.focus();
    }

    function indexOfActive() {
      return rowNodes.findIndex((n) => n.path === activePath);
    }

    function activate(node) {
      if (!node) return;
      activePath = node.path;
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
        if (node && node.isDir && expanded.has(node.path)) { activePath = node.path; collapse(node.path); }
        else moveTo(at < 0 ? 0 : at - 1);
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
      refreshAll: () => track(refreshAll()),
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
      destroy() {
        listings.clear();
        expanded.clear();
        rowNodes = [];
        if (element.parentNode) element.remove();
      },
    };
  }

  global.termlabProjectTree = { create, sortEntries, joinPath };
})(window);

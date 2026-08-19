// The file chooser VIEW — every pixel of the chooser and every listing call
// it makes, with no knowledge of the surface it is rendered on.
//
// It is deliberately host-agnostic. Two hosts exist: `features/editor/
// file-dialog.js` mounts it inside a tl-dialog in the main window, and (from
// the chooser-window work) `chooser.html` mounts it as the whole content of a
// window of its own. Nothing below asks which — in particular:
//   - it never asks for its own window label. The label decides which SSH
//     sessions are addressable (see buildScopes), and in a chooser window the
//     answer is the PARENT window's label, not this one's. It arrives as
//     `deps.parentWindowLabel`.
//   - it never registers with the close-guard / window-close machinery. It
//     reports exactly one answer, through `deps.onResolve`, and the host
//     decides what that means for its window.
//
// Layering (unchanged by the move):
//   - `features/editor/file-dialog-model.js` owns the pure arithmetic
//     (sort/filter/breadcrumbs/join/parent). This file owns the DOM and the IO.
//   - the data-service object handed in as `deps.data` owns every invoke this
//     needs (local_list_dir / sftp_list_dir / sftp_realpath /
//     remote_get_sessions / get_home_dir). No raw invoke names appear below.
//
// The DOM is built with createElement rather than innerHTML throughout. That
// is deliberate: file names are attacker-controlled-ish text from a remote
// host, and it also keeps the module testable against a minimal DOM stub.
(function initTermLabFileDialogView(global) {
  'use strict';

  const LOCAL_SCOPE_LABEL = 'This Mac';

  // ---------------------------------------------------------------------------
  // Ambient lookups (resolved lazily — this script loads before main-runtime
  // publishes termlabServices, exactly like editor-service.js).
  // ---------------------------------------------------------------------------

  // The transport the data-service functions are called WITH. `deps.data` is
  // the data service itself (its functions all take `invoke` first), so the
  // one thing the view still resolves from the ambient globals is the client
  // — the same lookup file-dialog.js does, and the same one every other
  // feature module does.
  function invoke(command, args) {
    const client = global.termlabServices && global.termlabServices.tauriClient;
    if (!client || typeof client.invoke !== 'function') {
      return Promise.reject(new Error('tauri client unavailable'));
    }
    return client.invoke(command, args);
  }

  function fileModel() {
    return global.termlabFileDialogModel || null;
  }

  function errorText(error) {
    if (!error) return 'Unknown error';
    if (error.message) return String(error.message);
    return String(error);
  }

  // Size and date formatting live in file-dialog-model.js (pure, injected
  // `now`), not in window.utils: the listing's column rules — '—' for a
  // directory, 'Today'/'Yesterday', no leading zero on the day — are this
  // dialog's, and utils.formatDate has its own callers to answer to.

  // ---------------------------------------------------------------------------
  // Session -> scope derivation (pure; exported for tests)
  // ---------------------------------------------------------------------------

  // `remote_get_sessions` returns `ActiveSession { key, host, user, port }`
  // (crates/termlab_tauri/src/remote/server_commands.rs). There is no paneId
  // field and no label field: `key` is `"{window_label}:{pane_id}"`, and the
  // label is composed from user/host/port.
  //
  // The label is NOT composed here. It is hashed into the temp path by
  // `editor_temp_path`, which makes it the editor's identity for a remote
  // file, so it has to be the byte-identical string files-panel.js produces
  // for the same session — otherwise the same file opens in two tabs
  // depending on which surface opened it. The single formula lives in
  // features/files/data-service.js as `sessionHostLabel`; this is only the
  // lookup of it.
  //
  // Returns null when the shared helper is unavailable. There is deliberately
  // no local fallback formula: a second copy is exactly the failure this
  // indirection exists to prevent, so a build that cannot reach the helper
  // offers no remote scopes rather than inventing labels for them.
  function hostLabelFor(data, session, paneId) {
    if (!data || typeof data.sessionHostLabel !== 'function') return null;
    return data.sessionHostLabel(session, paneId);
  }

  // Sessions are global to the app, but `sftp_list_dir` resolves its handle
  // with `get_ssh_handle(state, window.label(), pane_id)` — a pane id from
  // another window would either miss or, worse, hit an unrelated pane of the
  // same number. So only the OWNING window's sessions can be offered, and in
  // a chooser window the owner is the parent that asked for the chooser.
  function paneIdFromSessionKey(key, windowLabel) {
    if (!windowLabel) return null;
    const text = String(key == null ? '' : key);
    const prefix = `${windowLabel}:`;
    if (text.indexOf(prefix) !== 0) return null;
    const tail = text.slice(prefix.length);
    if (!/^\d+$/.test(tail)) return null;
    return Number(tail);
  }

  // Local first, then remote hosts in a stable order. `remote_get_sessions`
  // walks a HashMap, so without the sort the buttons would shuffle between
  // openings.
  //
  // `hostLabel` is what goes to openRemoteFile (see above). `label` is what
  // the button shows: when two panes are on the same host the bare label is
  // ambiguous, so the *display* gets a pane suffix while `hostLabel` stays
  // clean.
  function buildScopes(data, sessions, windowLabel, localStart) {
    const scopes = [{
      id: 'local',
      kind: 'local',
      label: LOCAL_SCOPE_LABEL,
      hostLabel: null,
      paneId: null,
      start: localStart || '/',
    }];

    const remote = [];
    for (const session of (Array.isArray(sessions) ? sessions : [])) {
      const paneId = paneIdFromSessionKey(session && session.key, windowLabel);
      if (paneId == null) continue;
      const hostLabel = hostLabelFor(data, session, paneId);
      if (hostLabel == null) continue;
      remote.push({
        id: `remote:${paneId}`,
        kind: 'remote',
        label: hostLabel,
        hostLabel,
        paneId,
        start: null,
      });
    }
    remote.sort((a, b) => {
      const al = a.hostLabel.toLowerCase();
      const bl = b.hostLabel.toLowerCase();
      if (al < bl) return -1;
      if (al > bl) return 1;
      return a.paneId - b.paneId;
    });

    const seen = new Map();
    for (const scope of remote) seen.set(scope.hostLabel, (seen.get(scope.hostLabel) || 0) + 1);
    for (const scope of remote) {
      if (seen.get(scope.hostLabel) > 1) scope.label = `${scope.hostLabel} (pane ${scope.paneId})`;
    }

    return scopes.concat(remote);
  }

  // ---------------------------------------------------------------------------
  // Listing IO — everything goes through the injected data service
  // ---------------------------------------------------------------------------

  // Does this path already exist? Resolves with the FileEntry, rejects when it
  // does not exist (both backends return a bare String there). Save As reads
  // the rejection as "free to write" and the resolution as "ask first".
  function statScopePath(data, scope, path) {
    if (!data) return Promise.reject(new Error('Files data service unavailable'));
    if (scope.kind === 'local') {
      if (typeof data.statLocal !== 'function') {
        return Promise.reject(new Error('Files data service unavailable: statLocal'));
      }
      return data.statLocal(invoke, path);
    }
    if (typeof data.statRemote !== 'function') {
      return Promise.reject(new Error('Files data service unavailable: statRemote'));
    }
    return data.statRemote(invoke, scope.paneId, path);
  }

  function mkdirInScope(data, scope, path) {
    if (!data) return Promise.reject(new Error('Files data service unavailable'));
    if (scope.kind === 'local') {
      if (typeof data.localMkdir !== 'function') {
        return Promise.reject(new Error('Files data service unavailable: localMkdir'));
      }
      return data.localMkdir(invoke, path);
    }
    if (typeof data.remoteMkdir !== 'function') {
      return Promise.reject(new Error('Files data service unavailable: remoteMkdir'));
    }
    return data.remoteMkdir(invoke, scope.paneId, path);
  }

  function listScopeDir(data, scope, path) {
    if (!data) return Promise.reject(new Error('Files data service unavailable'));
    if (scope.kind === 'local') {
      if (typeof data.listLocalDir !== 'function') {
        return Promise.reject(new Error('Files data service unavailable: listLocalDir'));
      }
      return data.listLocalDir(invoke, path);
    }
    if (typeof data.listRemoteDir !== 'function') {
      return Promise.reject(new Error('Files data service unavailable: listRemoteDir'));
    }
    return data.listRemoteDir(invoke, scope.paneId, path);
  }

  // '.' rather than '~': SFTP realpath resolves it server-side against the
  // session's own home, which is the only thing that knows what `~` means on
  // that host. Same call and same argument as files-panel.js:201.
  function resolveScopeStart(data, scope) {
    if (scope.kind === 'local') return Promise.resolve(scope.start || '/');
    if (!data || typeof data.getRemoteRealPath !== 'function') {
      return Promise.reject(new Error('Files data service unavailable: getRemoteRealPath'));
    }
    return data.getRemoteRealPath(invoke, scope.paneId, '.');
  }

  // `windowLabel` is the PARENT window's label, handed in by the host. The
  // view never calls getCurrentWindowLabel: in a chooser window that would
  // answer with the chooser's own label and every remote scope would silently
  // vanish from the sidebar.
  async function loadScopes(data, windowLabel) {
    if (!data) throw new Error('Files data service unavailable');
    const [home, sessions] = await Promise.all([
      typeof data.getHomeDir === 'function' ? data.getHomeDir(invoke).catch(() => '/') : Promise.resolve('/'),
      typeof data.getSessions === 'function' ? data.getSessions(invoke).catch(() => []) : Promise.resolve([]),
    ]);
    return buildScopes(data, sessions, windowLabel, home || '/');
  }

  // ---------------------------------------------------------------------------
  // DOM helpers
  // ---------------------------------------------------------------------------

  function el(tag, className, text) {
    const node = document.createElement(tag);
    if (className) node.className = className;
    if (text != null) node.textContent = String(text);
    return node;
  }

  function clearChildren(node) {
    if (!node) return;
    while (node.lastChild) node.removeChild(node.lastChild);
  }

  // Decoration, never content: `tlIcon` is a separate script (app/ui/tl-icon.js)
  // and its dark-variant lookup reads document.documentElement, so a build (or
  // a test harness) without it must lose the glyph and nothing else. Same
  // guard shape as ui/tl-combo.js:57 and ui/tl-spinner.js:29.
  function appendIcon(parent, name, extraClass) {
    const icons = global.tlIcon;
    if (!parent || !icons || typeof icons.create !== 'function') return null;
    let img = null;
    try {
      img = icons.create(name, { size: 16, alt: '' });
    } catch (_) {
      return null;
    }
    if (!img) return null;
    if (extraClass) img.className = `${img.className} ${extraClass}`;
    parent.appendChild(img);
    return img;
  }

  function setDisabled(button, disabled) {
    if (!button) return;
    // aria-disabled as well as the property: the click gate below reads the
    // live `disabled` property, and a screen reader reads the attribute.
    // Setting both keeps them in step (same fix as settings'
    // wireApplyDirtyTracking).
    button.disabled = !!disabled;
    if (typeof button.setAttribute === 'function') {
      button.setAttribute('aria-disabled', disabled ? 'true' : 'false');
    }
  }

  // The footer's own buttons. tl-dialog used to build these (and this is the
  // shape it built): a `.tl-btn`, primary or not, whose click is gated on the
  // live `disabled` property rather than on whatever it was at build time.
  function footerButton(label, primary, onSelect) {
    const btn = el('button', primary ? 'tl-btn tl-btn--primary' : 'tl-btn', label);
    btn.type = 'button';
    btn.addEventListener('click', () => {
      if (!btn.disabled && typeof onSelect === 'function') onSelect();
    });
    return btn;
  }

  // ---------------------------------------------------------------------------
  // Nested prompts (Save As only)
  // ---------------------------------------------------------------------------
  //
  // Both stack ON TOP of the chooser rather than replacing it: answering
  // "no" has to leave the user where they were, with the same directory and
  // the same typed name, free to choose somewhere else. tl-dialog supports
  // the nesting (its z-index/Escape/Tab-trap handling is all depth-aware);
  // the `done` latch is dialog-service.js's confirmSave shape, so the onClose
  // that close() itself fires cannot overwrite an answer already given.
  //
  // These are the ONE thing the view still takes from tl-dialog, and they are
  // small modals over whatever host is on screen — tl-dialog is loaded by both
  // index.html and chooser.html for exactly this.

  function confirmOverwrite(name) {
    return new Promise((resolve) => {
      if (!global.tlDialog || typeof global.tlDialog.open !== 'function') {
        resolve(false);
        return;
      }
      let done = false;
      let handle = null;
      const finish = (accepted) => {
        if (done) return;
        done = true;
        resolve(accepted);
        if (handle) handle.close(accepted ? 'overwrite' : 'cancel');
      };
      handle = global.tlDialog.open({
        title: 'Overwrite File?',
        ariaLabel: 'Overwrite file',
        size: 'sm',
        body: (bodyEl) => {
          const message = el('div', 'tl-dialog-message');
          message.textContent = `"${name}" already exists. Replace it?`;
          bodyEl.appendChild(message);
        },
        // Cancel first, so it is the button tl-dialog focuses: the destructive
        // answer must never be the one a stray Return picks.
        buttons: [
          { label: 'Cancel', onSelect: () => finish(false) },
          { label: 'Overwrite', primary: true, danger: true, onSelect: () => finish(true) },
        ],
        onClose: () => finish(false),
      });
    });
  }

  // Resolves with the typed name, or null. Validation of what the name means
  // (and every error from the mkdir itself) belongs to the caller.
  function promptForFolderName() {
    return new Promise((resolve) => {
      if (!global.tlDialog || typeof global.tlDialog.open !== 'function') {
        resolve(null);
        return;
      }
      let done = false;
      let handle = null;
      const input = el('input', 'tl-input tl-filedlg__prompt-input');
      input.type = 'text';
      input.spellcheck = false;
      input.setAttribute('aria-label', 'Folder name');
      input.setAttribute('placeholder', 'Folder name');
      const finish = (value) => {
        if (done) return;
        done = true;
        resolve(value);
        if (handle) handle.close(value ? 'create' : 'cancel');
      };
      const submit = () => {
        const typed = String(input.value || '').trim();
        finish(typed || null);
      };
      input.addEventListener('keydown', (event) => {
        if (event.key !== 'Enter') return;
        if (typeof event.preventDefault === 'function') event.preventDefault();
        submit();
      });
      handle = global.tlDialog.open({
        title: 'New Folder',
        ariaLabel: 'New folder',
        size: 'sm',
        body: (bodyEl) => { bodyEl.appendChild(input); },
        buttons: [
          { label: 'Cancel', onSelect: () => finish(null) },
          { label: 'Create', primary: true, onSelect: () => submit() },
        ],
        onClose: () => finish(null),
      });
    });
  }

  // ---------------------------------------------------------------------------
  // The chooser
  // ---------------------------------------------------------------------------

  /**
   * Render the chooser into `root` and answer exactly once through
   * `deps.onResolve`:
   *   { scope, path, entry }   — a file the user picked (in save mode `entry`
   *                              is the existing file being replaced, or null)
   *   null                     — cancelled (Cancel, Escape, or a failure to
   *                              even build the scope list)
   *
   * deps = { data, mode, filename, selectFilename, parentWindowLabel, onResolve }
   *   `data`   the features/files/data-service.js object (every IO call the
   *            view makes goes through it).
   *   `mode`   'save' adds the filename field (pre-filled with `filename`),
   *            the New Folder button and the existence check; the primary
   *            button reads Save. Anything else is an open chooser.
   *   `selectFilename` focuses that field with its text selected — for a
   *            placeholder name (an untitled buffer's "Untitled-2") that is
   *            there to be typed over rather than edited.
   *   `parentWindowLabel` the window whose SSH sessions may be offered. See
   *            paneIdFromSessionKey.
   *
   * Returns { focusInitial }, which the host calls once the view is on screen.
   */
  function build(root, deps) {
    const opts = deps || {};
    const data = opts.data || null;
    const saveMode = opts.mode === 'save';
    const parentWindowLabel = opts.parentWindowLabel || null;

    // The latch. Escape, Cancel and a successful pick all reach here, and so
    // does a host that tears the view down; whichever arrives first is the
    // answer (same shape as dialog-service.js's confirmSave).
    let done = false;
    function finish(result) {
      if (done) return;
      done = true;
      if (typeof opts.onResolve === 'function') opts.onResolve(result || null);
    }

    const model = fileModel();
    if (!model) {
      // Nothing can be rendered without the formatters/sort. The host owns the
      // user-facing report (a toast in the main window); the view just answers.
      finish(null);
      return { focusInitial: () => {} };
    }

    // ----- state -----
    let scopes = [];
    let scope = null;
    let cwd = '/';
    let entries = [];
    let visible = [];
    let selectedIndex = -1;
    let showHidden = false;
    let filterQuery = '';
    // Sort lives beside the filter because it is applied in the same place the
    // filter is (renderRows), and it is deliberately per-dialog-open: every
    // chooser starts at name/ascending rather than restoring a previous
    // choice (spec, "Known limitations").
    let sortKey = 'name';
    let sortDir = 'asc';
    // Bumped by every navigation. A listing whose token is stale when it
    // lands is dropped: the user has already gone somewhere else, and
    // painting it would put the wrong rows under the current breadcrumb.
    let navToken = 0;

    // ----- elements -----
    //
    // Two regions above a footer: a fixed sidebar of places (This Mac + every
    // connected host) and the main column (path bar, listing, inline error).
    // The footer's left slot carries the Hidden toggle and, in save mode, the
    // filename controls — see footerCtl below; its right slot carries Cancel
    // and the primary button.
    const view = el('div', 'tl-filedlg__view');
    const body = el('div', 'tl-filedlg');
    view.appendChild(body);

    const sidebar = el('div', 'tl-filedlg__sidebar');
    sidebar.setAttribute('role', 'group');
    sidebar.setAttribute('aria-label', 'Location');
    body.appendChild(sidebar);

    const mainCol = el('div', 'tl-filedlg__main');
    body.appendChild(mainCol);

    const pathBar = el('div', 'tl-filedlg__pathbar');
    const upBtn = el('button', 'tl-filedlg__up', '↑');
    upBtn.type = 'button';
    upBtn.setAttribute('aria-label', 'Parent directory');
    upBtn.title = 'Parent directory';
    pathBar.appendChild(upBtn);
    const crumbBar = el('div', 'tl-filedlg__crumbs');
    crumbBar.setAttribute('aria-label', 'Breadcrumbs');
    pathBar.appendChild(crumbBar);
    const pathInput = el('input', 'tl-input tl-filedlg__path');
    pathInput.type = 'text';
    pathInput.spellcheck = false;
    pathInput.setAttribute('aria-label', 'Path');
    pathInput.setAttribute('placeholder', '/path/to/directory');
    pathBar.appendChild(pathInput);
    const filterBox = el('div', 'tl-filedlg__filterbox');
    appendIcon(filterBox, 'search', 'tl-filedlg__filtericon');
    const filterInput = el('input', 'tl-input tl-filedlg__filter');
    filterInput.type = 'search';
    filterInput.setAttribute('aria-label', 'Filter by name');
    filterInput.setAttribute('placeholder', 'Filter…');
    filterBox.appendChild(filterInput);
    pathBar.appendChild(filterBox);
    mainCol.appendChild(pathBar);

    const box = el('div', 'tl-picker__box tl-scroll tl-filedlg__box');
    const head = el('div', 'tl-filedlg__head');
    head.setAttribute('role', 'row');
    // key -> { btn, arrow, variant }. The header cells are buttons so the
    // sort is reachable by keyboard and activatable with Space/Enter — both
    // native button behaviors that survive the role override below, not
    // role-derived ones. `role` is overridden to columnheader so `aria-sort`
    // is on the element ARIA expects to find it on; that override REMOVES the
    // implicit button role, so the cells are no longer announced as
    // pressable.
    const headCells = {};
    function buildHeadCell(key, variant, label) {
      const btn = el('button', `tl-filedlg__col tl-filedlg__col--${variant}`);
      btn.type = 'button';
      btn.setAttribute('role', 'columnheader');
      btn.appendChild(el('span', 'tl-filedlg__col-label', label));
      const arrow = el('span', 'tl-filedlg__sort');
      btn.appendChild(arrow);
      btn.addEventListener('click', () => sortBy(key));
      head.appendChild(btn);
      headCells[key] = { btn, arrow, variant };
    }
    buildHeadCell('name', 'name', 'Name');
    buildHeadCell('size', 'size', 'Size');
    buildHeadCell('modified', 'time', 'Modified');
    box.appendChild(head);
    const list = el('div', 'tl-filedlg__list');
    list.setAttribute('role', 'listbox');
    list.setAttribute('aria-label', 'Files');
    list.tabIndex = 0;
    box.appendChild(list);
    const emptyEl = el('div', 'tl-filedlg__empty', 'No matches');
    emptyEl.hidden = true;
    box.appendChild(emptyEl);
    mainCol.appendChild(box);

    // Inline, in the body, next to the list it describes — deliberately NOT a
    // toast. A toast for "cannot list /root" would fly away over the terminal
    // while the dialog sat there showing nothing and explaining nothing.
    const errorEl = el('div', 'tl-filedlg__error');
    errorEl.setAttribute('role', 'alert');
    errorEl.hidden = true;
    mainCol.appendChild(errorEl);

    // ----- footer -----
    //
    // The view's own footer row, not the host's: it is the same row in both
    // hosts, and the window host has no dialog chrome to borrow one from.
    const footer = el('div', 'tl-filedlg__footer');
    const footerStart = el('div', 'tl-filedlg__footer-start');
    const footerEnd = el('div', 'tl-filedlg__footer-end');
    footer.appendChild(footerStart);
    footer.appendChild(footerEnd);
    view.appendChild(footer);

    // The footer's left slot: options about the whole dialog rather than
    // about the directory on screen.
    const footerCtl = el('div', 'tl-filedlg__footctl');
    const hiddenLabel = el('label', 'tl-check tl-filedlg__hidden');
    const hiddenBox = el('input');
    hiddenBox.type = 'checkbox';
    hiddenLabel.appendChild(hiddenBox);
    hiddenLabel.appendChild(el('span', null, 'Hidden'));
    footerCtl.appendChild(hiddenLabel);
    footerStart.appendChild(footerCtl);

    // Save mode only: the name to write, plus the one directory-creating
    // affordance a save dialog needs. Built only in save mode so the open
    // chooser offers neither.
    let nameInput = null;
    let newFolderBtn = null;
    if (saveMode) {
      footerCtl.appendChild(el('label', 'tl-filedlg__namelabel', 'Save As:'));
      nameInput = el('input', 'tl-input tl-filedlg__name');
      nameInput.type = 'text';
      nameInput.spellcheck = false;
      nameInput.setAttribute('aria-label', 'File name');
      nameInput.setAttribute('placeholder', 'File name');
      nameInput.value = String(opts.filename || '');
      footerCtl.appendChild(nameInput);
      newFolderBtn = el('button', 'tl-btn tl-filedlg__newfolder', 'New Folder');
      newFolderBtn.type = 'button';
      footerCtl.appendChild(newFolderBtn);
    }

    // Cancel first, then "Open" or "Save" depending on the mode.
    const cancelButton = footerButton('Cancel', false, () => finish(null));
    footerEnd.appendChild(cancelButton);
    const primaryButton = footerButton(
      saveMode ? 'Save' : 'Open',
      true,
      () => (saveMode ? attemptSave() : activate()),
    );
    footerEnd.appendChild(primaryButton);

    // ----- rendering -----

    // One row per scope, in the order buildScopes produced (local first, then
    // hosts), split into the two labelled sections the sidebar shows.
    function buildScopeRow(candidate) {
      const btn = el('button', 'tl-filedlg__scope');
      btn.type = 'button';
      const isActive = !!(scope && candidate.id === scope.id);
      if (isActive) btn.className = 'tl-filedlg__scope is-active';
      btn.setAttribute('aria-pressed', isActive ? 'true' : 'false');
      // 'sftp' is the glyph the SFTP tool window already uses for a remote
      // host (tool-window-runtime.js:238); no new asset is added for this.
      appendIcon(btn, candidate.kind === 'remote' ? 'sftp' : 'folder', 'tl-filedlg__scope-icon');
      btn.appendChild(el('span', 'tl-filedlg__scope-label', candidate.label));
      if (candidate.kind === 'remote') {
        btn.title = candidate.hostLabel;
        // Affordance, not state machinery: every host in this list is
        // connected by construction — buildScopes only ever sees sessions
        // that are already up.
        btn.appendChild(el('span', 'tl-filedlg__scope-dot'));
      }
      btn.addEventListener('click', () => {
        // `enterScope` sets `scope = candidate` BEFORE `resolveScopeStart`
        // runs, so a scope whose start failed to resolve is left "active"
        // with `start` still null. Guarding on id alone would make that
        // button permanently dead — the same-scope click that should retry
        // it would instead no-op forever. Only skip re-entering a scope
        // that is both active AND already has a resolved start.
        if (scope && candidate.id === scope.id && candidate.start != null) return;
        enterScope(candidate);
      });
      return btn;
    }

    function appendScopeSection(label, members) {
      if (!members.length) return;
      const section = el('div', 'tl-filedlg__section');
      section.appendChild(el('div', 'tl-filedlg__section-label', label));
      for (const candidate of members) section.appendChild(buildScopeRow(candidate));
      sidebar.appendChild(section);
    }

    function renderSidebar() {
      clearChildren(sidebar);
      // A "Hosts" heading over nothing would read as a broken feature rather
      // than an empty one, so the section only exists when a session does.
      appendScopeSection('Places', scopes.filter((s) => s.kind !== 'remote'));
      appendScopeSection('Hosts', scopes.filter((s) => s.kind === 'remote'));
    }

    // The sort indicator, on the one column that owns the order.
    function renderHead() {
      for (const key of Object.keys(headCells)) {
        const cellDef = headCells[key];
        const isActive = key === sortKey;
        cellDef.btn.className = `tl-filedlg__col tl-filedlg__col--${cellDef.variant}`
          + (isActive ? ' is-active' : '');
        cellDef.btn.setAttribute('aria-sort',
          isActive ? (sortDir === 'asc' ? 'ascending' : 'descending') : 'none');
        cellDef.arrow.textContent = isActive ? (sortDir === 'asc' ? '▲' : '▼') : '';
      }
    }

    // A header click: a new column starts ascending, the current column
    // flips. The selection follows the ENTRY, not its row number — the file
    // the user picked must not become a different file because the order
    // changed under it.
    function sortBy(key) {
      if (sortKey === key) sortDir = sortDir === 'asc' ? 'desc' : 'asc';
      else { sortKey = key; sortDir = 'asc'; }
      const held = selectedEntry();
      renderHead();
      renderRows(held ? held.name : null);
      // Return focus to the list: the header button that was just clicked is
      // a SIBLING of `list`, not a descendant, so the keydown handler below
      // (registered on `list` itself) never sees an event while focus stays
      // on the button — arrows/Home/End would go dead and Enter would re-fire
      // the header's own click instead of opening the selection.
      if (typeof list.focus === 'function') list.focus();
    }

    function renderCrumbs() {
      clearChildren(crumbBar);
      // splitBreadcrumbs('') is the root crumb, never an empty array, so the
      // bar always has at least the '/' button to click back to.
      const crumbs = model.splitBreadcrumbs(cwd);
      crumbs.forEach((crumb, index) => {
        const btn = el('button', 'tl-filedlg__crumb', crumb.label);
        btn.type = 'button';
        if (index === crumbs.length - 1) btn.className = 'tl-filedlg__crumb is-current';
        btn.addEventListener('click', () => navigate(crumb.path));
        crumbBar.appendChild(btn);
      });
    }

    function selectedEntry() {
      return selectedIndex >= 0 && selectedIndex < visible.length ? visible[selectedIndex] : null;
    }

    // Open gates on a FILE being selected (a highlighted directory leaves it
    // disabled — Enter descends instead). Save gates on a non-empty name: the
    // whole point is to write somewhere that does not exist yet, so there is
    // nothing in the listing to select.
    function syncPrimaryButton() {
      if (saveMode) {
        setDisabled(primaryButton, !String(nameInput.value || '').trim());
        return;
      }
      const entry = selectedEntry();
      setDisabled(primaryButton, !entry || !!entry.is_dir);
    }

    // `preserveName` re-finds the selection by entry name in the NEW order
    // (used by the sort headers); it drops the selection when that entry is
    // no longer visible. Omitted, the selected index is left where it is —
    // which is what every other caller wants, including the navigations that
    // deliberately cleared it a moment earlier.
    function renderRows(preserveName) {
      clearChildren(list);
      visible = model.sortEntries(
        model.filterEntries(entries, filterQuery, showHidden), sortKey, sortDir,
      );
      if (preserveName != null) {
        selectedIndex = visible.findIndex((entry) => entry && entry.name === preserveName);
      }
      if (selectedIndex >= visible.length) selectedIndex = -1;
      // Read once per render, not once per row, so every row in one painting
      // dates itself against the same instant.
      const nowSeconds = Math.floor(Date.now() / 1000);
      visible.forEach((entry, index) => {
        const row = el('div', 'tl-filedlg__row' + (index === selectedIndex ? ' is-selected' : ''));
        row.setAttribute('role', 'option');
        row.setAttribute('aria-selected', index === selectedIndex ? 'true' : 'false');
        row.tabIndex = -1;
        appendIcon(row, entry.is_dir ? 'folder' : 'file', 'tl-filedlg__rowicon');
        const name = el('span', 'tl-filedlg__cell tl-filedlg__cell--name', entry.name);
        if (entry.is_dir) name.className = 'tl-filedlg__cell tl-filedlg__cell--name is-dir';
        row.appendChild(name);
        // A directory's `size` is its inode's, which means nothing to the
        // person reading the column — hence '—' here rather than a number.
        // The formatter never produces '—' itself; that call is this
        // renderer's.
        row.appendChild(el('span', 'tl-filedlg__cell tl-filedlg__cell--size',
          entry.is_dir ? '—' : model.formatSize(entry.size)));
        row.appendChild(el('span', 'tl-filedlg__cell tl-filedlg__cell--time',
          model.formatModified(entry.modified, nowSeconds)));
        row.addEventListener('click', () => select(index));
        row.addEventListener('dblclick', () => { select(index); activate(); });
        list.appendChild(row);
      });
      // A re-sort (the only caller that passes `preserveName`) can carry the
      // held selection to a new row position that is off-screen. `select()`
      // is deliberately NOT the fix here — in save mode it writes
      // `nameInput.value`, which would clobber a filename the user already
      // typed after clicking that row. Scroll the row directly instead.
      if (preserveName != null && selectedIndex >= 0) {
        const selectedRow = list.children[selectedIndex];
        if (selectedRow && typeof selectedRow.scrollIntoView === 'function') {
          selectedRow.scrollIntoView({ block: 'nearest' });
        }
      }
      emptyEl.hidden = visible.length !== 0 || !errorEl.hidden;
      syncPrimaryButton();
    }

    function select(index) {
      selectedIndex = index;
      // In save mode, picking a row means "use that name" — the Finder/GTK
      // behaviour. It fills the field rather than saving, so the overwrite
      // prompt is still one deliberate click away.
      if (saveMode) {
        const picked = index >= 0 && index < visible.length ? visible[index] : null;
        if (picked && !picked.is_dir) nameInput.value = picked.name;
      }
      const rows = list.children;
      for (let i = 0; i < rows.length; i++) {
        const isSelected = i === index;
        rows[i].className = 'tl-filedlg__row' + (isSelected ? ' is-selected' : '');
        rows[i].setAttribute('aria-selected', isSelected ? 'true' : 'false');
        if (isSelected && typeof rows[i].scrollIntoView === 'function') {
          rows[i].scrollIntoView({ block: 'nearest' });
        }
      }
      syncPrimaryButton();
    }

    function showError(message) {
      errorEl.textContent = message;
      errorEl.hidden = false;
      emptyEl.hidden = true;
    }

    function clearError() {
      errorEl.textContent = '';
      errorEl.hidden = true;
    }

    // ----- navigation -----

    // Selection is dropped SYNCHRONOUSLY here, before the listing is even
    // requested. That is what stops the double-enter race: Enter on a
    // directory starts a navigation, and a second Enter landing before the
    // listing resolves finds no selection and does nothing — rather than
    // re-firing `activate()` on the file that happened to be selected a
    // moment ago and opening it behind the descent.
    async function navigate(path) {
      if (!scope) return;
      const token = ++navToken;
      cwd = path;
      selectedIndex = -1;
      entries = [];
      visible = [];
      clearError();
      clearChildren(list);
      syncPrimaryButton();
      pathInput.value = path;
      renderCrumbs();
      list.setAttribute('aria-busy', 'true');
      try {
        const listed = await listScopeDir(data, scope, path);
        if (token !== navToken) return;
        entries = Array.isArray(listed) ? listed : [];
        renderRows();
      } catch (error) {
        if (token !== navToken) return;
        entries = [];
        renderRows();
        // The scope bar stays live: a host that dropped mid-browse leaves the
        // user looking at this message with every other scope still one click
        // away, which is the whole point of showing it here.
        showError(errorText(error));
      } finally {
        if (token === navToken) list.setAttribute('aria-busy', 'false');
      }
    }

    async function enterScope(next) {
      const token = ++navToken;
      scope = next;
      cwd = '/';
      entries = [];
      visible = [];
      selectedIndex = -1;
      filterQuery = '';
      filterInput.value = '';
      clearError();
      clearChildren(list);
      syncPrimaryButton();
      renderSidebar();
      renderCrumbs();

      // Resolved once per scope and cached on it, so switching back and forth
      // does not re-round-trip.
      if (next.start == null) {
        list.setAttribute('aria-busy', 'true');
        let start;
        try {
          start = await resolveScopeStart(data, next);
        } catch (error) {
          if (token !== navToken) return;
          list.setAttribute('aria-busy', 'false');
          showError(errorText(error));
          return;
        }
        if (token !== navToken) return;
        next.start = start || '/';
      }
      await navigate(next.start);
    }

    // Enter / double-click. A directory descends; a file finishes (in save
    // mode select() has already put its name in the field, so "finishing" is
    // the same attempt the Save button makes, overwrite prompt and all).
    function activate() {
      const entry = selectedEntry();
      if (!entry) {
        if (saveMode) attemptSave();
        return;
      }
      if (entry.is_dir) {
        navigate(model.joinPath(cwd, entry.name));
        return;
      }
      if (saveMode) {
        attemptSave();
        return;
      }
      finish({ scope, path: model.joinPath(cwd, entry.name), entry });
    }

    // ----- save mode -----

    // The typed name resolved against the current directory. An absolute name
    // is taken as written (pasting a full path into the name field is a real
    // habit); anything else hangs off `cwd`.
    function targetPathFor(name) {
      return name.charAt(0) === '/' ? name : model.joinPath(cwd, name);
    }

    // Runs only while a Save is being decided, so a second click cannot open
    // two overwrite prompts for the same file.
    let saveInProgress = false;

    async function attemptSave() {
      if (!saveMode || saveInProgress || !scope) return;
      const name = String(nameInput.value || '').trim();
      if (!name) return;                       // the button is disabled too
      const path = targetPathFor(name);
      saveInProgress = true;
      setDisabled(primaryButton, true);
      clearError();
      try {
        let existing = null;
        try {
          existing = await statScopePath(data, scope, path);
        } catch (_) {
          // Every stat rejection is read as "does not exist" — the ordinary
          // case for a Save As, and the reason this is a caught rejection
          // rather than an error to report. That is a real trade, not a free
          // simplification: `local_stat`/`sftp_stat` both return a bare
          // `Result<FileEntry, String>` (sftp_commands.rs), so a permission
          // error or a dropped SSH connection looks identical here to a
          // missing file, and silently skips the overwrite prompt instead of
          // surfacing the real problem. Telling them apart needs typed errors
          // from both backends, not a JS-side fix.
          existing = null;
        }
        if (existing && existing.is_dir) {
          showError(`"${name}" is a directory.`);
          return;
        }
        if (existing && !(await confirmOverwrite(name))) return;
        finish({ scope, path, entry: existing });
      } finally {
        saveInProgress = false;
        syncPrimaryButton();
      }
    }

    async function createFolder() {
      if (!scope) return;
      const name = await promptForFolderName();
      if (!name) return;
      try {
        await mkdirInScope(data, scope, targetPathFor(name));
      } catch (error) {
        // Inline, next to the listing it failed in — same reasoning as a
        // listing error, and a toast would fly away over the terminal.
        showError(errorText(error));
        return;
      }
      await navigate(cwd);
    }

    // Enter in the path field. A directory is jumped to; a full file path is
    // accepted by listing its parent and matching the basename, so pasting
    // "/etc/hosts" works as well as pasting "/etc".
    async function jumpToTypedPath() {
      const typed = String(pathInput.value || '').trim();
      if (!typed || !scope) return;
      const token = ++navToken;
      selectedIndex = -1;
      clearError();
      syncPrimaryButton();
      list.setAttribute('aria-busy', 'true');
      let dirError = null;
      try {
        const listed = await listScopeDir(data, scope, typed);
        if (token !== navToken) return;
        cwd = typed;
        entries = Array.isArray(listed) ? listed : [];
        renderCrumbs();
        renderRows();
        list.setAttribute('aria-busy', 'false');
        return;
      } catch (error) {
        if (token !== navToken) return;
        dirError = error;
      }

      const parent = model.parentPath(typed);
      const base = typed.replace(/\/+$/, '').split('/').pop();
      try {
        const listed = await listScopeDir(data, scope, parent);
        if (token !== navToken) return;
        const match = (Array.isArray(listed) ? listed : []).find((e) => e && e.name === base && !e.is_dir);
        if (match) {
          finish({ scope, path: model.joinPath(parent, base), entry: match });
          return;
        }
        cwd = parent;
        entries = Array.isArray(listed) ? listed : [];
        pathInput.value = parent;
        renderCrumbs();
        renderRows();
        showError(`No such file or directory: ${typed}`);
      } catch (_) {
        if (token !== navToken) return;
        renderRows();
        showError(errorText(dirError));
      } finally {
        if (token === navToken) list.setAttribute('aria-busy', 'false');
      }
    }

    // ----- listeners (all element-scoped; a document-level keydown would
    // both break the boundary check and fight the keyboard router) -----

    upBtn.addEventListener('click', () => navigate(model.parentPath(cwd)));

    filterInput.addEventListener('input', () => {
      filterQuery = filterInput.value;
      selectedIndex = -1;
      renderRows();
    });

    hiddenBox.addEventListener('change', () => {
      showHidden = !!hiddenBox.checked;
      selectedIndex = -1;
      renderRows();
    });

    pathInput.addEventListener('keydown', (event) => {
      if (event.key !== 'Enter') return;
      if (typeof event.preventDefault === 'function') event.preventDefault();
      jumpToTypedPath();
    });

    if (saveMode) {
      nameInput.addEventListener('input', () => syncPrimaryButton());
      nameInput.addEventListener('keydown', (event) => {
        if (event.key !== 'Enter') return;
        if (typeof event.preventDefault === 'function') event.preventDefault();
        attemptSave();
      });
      newFolderBtn.addEventListener('click', () => { createFolder(); });
    }

    list.addEventListener('keydown', (event) => {
      const key = event.key;
      if (key === 'ArrowDown' || key === 'ArrowUp' || key === 'Home' || key === 'End') {
        if (!visible.length) return;
        if (typeof event.preventDefault === 'function') event.preventDefault();
        let next;
        if (key === 'Home') next = 0;
        else if (key === 'End') next = visible.length - 1;
        else if (key === 'ArrowDown') next = selectedIndex < 0 ? 0 : Math.min(visible.length - 1, selectedIndex + 1);
        else next = selectedIndex <= 0 ? 0 : selectedIndex - 1;
        select(next);
        return;
      }
      if (key === 'Enter') {
        if (typeof event.preventDefault === 'function') event.preventDefault();
        activate();
      }
    });

    // Escape is the view's OWN, on the root it was handed: in the window host
    // there is no tl-dialog to inherit an Escape registration from, and in the
    // tl-dialog host the two both land on `finish`, which answers once.
    // Element-scoped, never `document` — see the note above the listeners.
    if (root && typeof root.addEventListener === 'function') {
      root.addEventListener('keydown', (event) => {
        if (!event || event.key !== 'Escape') return;
        if (typeof event.preventDefault === 'function') event.preventDefault();
        finish(null);
      });
    }

    // ----- lifecycle -----

    // Put the caret in the name field with the placeholder selected, so the
    // first keystroke replaces "Untitled-2" instead of appending to it.
    //
    // Scheduled rather than done inline: the tl-dialog host focuses the first
    // focusable element in the panel from a requestAnimationFrame callback it
    // queued BEFORE it calls this, so a plain focus() here would be undone one
    // frame later. rAF callbacks run in registration order, so ours lands
    // after it. With no rAF (the test sandboxes) that focus already happened
    // synchronously, and running now is correct.
    function selectNameField() {
      const apply = () => {
        if (!nameInput) return;
        if (typeof nameInput.focus === 'function') nameInput.focus();
        if (typeof nameInput.select === 'function') nameInput.select();
      };
      if (typeof global.requestAnimationFrame === 'function') global.requestAnimationFrame(apply);
      else apply();
    }

    // The header carries the sort indicator from the first paint, before the
    // listing it labels has even been requested.
    renderHead();
    // Save starts enabled when the pane's current name was pre-filled, so
    // ⌘⇧S → Return saves under the same name in a new place.
    syncPrimaryButton();
    if (root && typeof root.appendChild === 'function') root.appendChild(view);

    // Scope discovery is async, so the chooser is already on screen when it
    // lands — an empty sidebar for a few ms beats a ⌘O that appears to do
    // nothing while sessions are enumerated.
    loadScopes(data, parentWindowLabel).then((built) => {
      if (done) return;
      scopes = built;
      renderSidebar();
      return enterScope(scopes[0]);
    }).catch((error) => {
      if (done) return;
      showError(errorText(error));
    });

    return {
      // Called by the host once the view is on screen. In save mode with a
      // placeholder name that means the name field, selected; otherwise the
      // listing, so the arrow keys work without a click first.
      focusInitial: () => {
        if (saveMode && opts.selectFilename && nameInput) {
          selectNameField();
          return;
        }
        if (typeof list.focus === 'function') list.focus();
      },
    };
  }

  global.termlabFileDialogView = {
    build,
    // Exposed for scripts/tests/test_file_dialog.mjs — pure derivations that
    // need no DOM (matches tl-dialog.js's _zIndexForDepth precedent). The host
    // label formula is deliberately NOT among them: it belongs to
    // features/files/data-service.js and is pinned by a test there, so that
    // one test covers both this module and files-panel.js.
    _paneIdFromSessionKey: paneIdFromSessionKey,
    _buildScopes: buildScopes,
  };
})(window);

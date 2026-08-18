// Unified file chooser for the editor — "This Mac" plus every SSH host this
// window is already connected to, in one tl-dialog.
//
// Nothing here connects, disconnects or authenticates: the scope bar is built
// from `remote_get_sessions`, which lists sessions that are ALREADY up. A host
// that is not connected simply is not offered.
//
// Layering:
//   - `features/editor/file-dialog-model.js` owns the pure arithmetic
//     (sort/filter/breadcrumbs/join/parent). This file owns the DOM and the IO.
//   - `features/files/data-service.js` owns every invoke this needs
//     (local_list_dir / sftp_list_dir / sftp_realpath / remote_get_sessions /
//     current_window_label / get_home_dir). No raw invoke names appear below.
//   - `termlabEditorService.openLocalFile` / `.openRemoteFile` are the only
//     open paths. This file adds none: the size cap, the binary/extension
//     blocklist and the same-file-focus behaviour all come free from them.
//
// The DOM is built with createElement rather than innerHTML throughout. That
// is deliberate: file names are attacker-controlled-ish text from a remote
// host, and it also keeps the module testable against a minimal DOM stub.
(function initTermLabFileDialog(global) {
  'use strict';

  const LOCAL_SCOPE_LABEL = 'This Mac';

  // ---------------------------------------------------------------------------
  // Ambient lookups (resolved lazily — this script loads before main-runtime
  // publishes termlabServices, exactly like editor-service.js).
  // ---------------------------------------------------------------------------

  function invoke(command, args) {
    const client = global.termlabServices && global.termlabServices.tauriClient;
    if (!client || typeof client.invoke !== 'function') {
      return Promise.reject(new Error('tauri client unavailable'));
    }
    return client.invoke(command, args);
  }

  function filesData() {
    return global.termlabFilesFeatureDataService || null;
  }

  function fileModel() {
    return global.termlabFileDialogModel || null;
  }

  function toastError(title, body) {
    if (global.toast && typeof global.toast.error === 'function') {
      global.toast.error(title, body);
      return;
    }
    console.error(`${title}: ${body}`);
  }

  function errorText(error) {
    if (!error) return 'Unknown error';
    if (error.message) return String(error.message);
    return String(error);
  }

  function formatSize(bytes) {
    const utils = global.utils;
    if (utils && typeof utils.formatSize === 'function') return utils.formatSize(bytes);
    return bytes == null ? '' : String(bytes);
  }

  function formatDate(epoch) {
    const utils = global.utils;
    if (utils && typeof utils.formatDate === 'function') return utils.formatDate(epoch);
    return '';
  }

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
  function hostLabelFor(session, paneId) {
    const data = filesData();
    if (!data || typeof data.sessionHostLabel !== 'function') return null;
    return data.sessionHostLabel(session, paneId);
  }

  // Sessions are global to the app, but `sftp_list_dir` resolves its handle
  // with `get_ssh_handle(state, window.label(), pane_id)` — a pane id from
  // another window would either miss or, worse, hit an unrelated pane of the
  // same number. So only this window's sessions can be offered.
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
  function buildScopes(sessions, windowLabel, localStart) {
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
      const hostLabel = hostLabelFor(session, paneId);
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
  // Listing IO — everything goes through features/files/data-service.js
  // ---------------------------------------------------------------------------

  function listScopeDir(scope, path) {
    const data = filesData();
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
  function resolveScopeStart(scope) {
    if (scope.kind === 'local') return Promise.resolve(scope.start || '/');
    const data = filesData();
    if (!data || typeof data.getRemoteRealPath !== 'function') {
      return Promise.reject(new Error('Files data service unavailable: getRemoteRealPath'));
    }
    return data.getRemoteRealPath(invoke, scope.paneId, '.');
  }

  async function loadScopes() {
    const data = filesData();
    if (!data) throw new Error('Files data service unavailable');
    const [home, windowLabel, sessions] = await Promise.all([
      typeof data.getHomeDir === 'function' ? data.getHomeDir(invoke).catch(() => '/') : Promise.resolve('/'),
      typeof data.getCurrentWindowLabel === 'function'
        ? data.getCurrentWindowLabel(invoke).catch(() => null)
        : Promise.resolve(null),
      typeof data.getSessions === 'function' ? data.getSessions(invoke).catch(() => []) : Promise.resolve([]),
    ]);
    return buildScopes(sessions, windowLabel, home || '/');
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

  function findFooterButton(panel, label) {
    if (!panel || typeof panel.querySelectorAll !== 'function') return null;
    const buttons = Array.prototype.slice.call(panel.querySelectorAll('.tl-dialog__footer .tl-btn'));
    return buttons.find((btn) => btn.textContent === label) || null;
  }

  function setDisabled(button, disabled) {
    if (!button) return;
    // aria-disabled as well as the property: tl-dialog's buildFooterButton
    // only writes aria-disabled once, at build time, and its click gate reads
    // the live `disabled` property. Setting both keeps them in step (same fix
    // as settings' wireApplyDirtyTracking).
    button.disabled = !!disabled;
    if (typeof button.setAttribute === 'function') {
      button.setAttribute('aria-disabled', disabled ? 'true' : 'false');
    }
  }

  // ---------------------------------------------------------------------------
  // The chooser
  // ---------------------------------------------------------------------------

  // One dialog at a time. A second ⌘O while the chooser is up should surface
  // the dialog that is already open, not stack a second modal over it.
  let activeChoice = null;

  /**
   * Show the chooser. Resolves with
   *   { scope, path, entry }   — a file the user picked
   *   null                     — cancelled (Cancel, Escape, backdrop, or a
   *                              failure to even build the scope list)
   * It never rejects: a cancelled chooser is not an error.
   */
  function chooseFile() {
    if (activeChoice) return activeChoice.promise;

    const model = fileModel();
    if (!model) {
      toastError('Cannot Open File', 'The file dialog model is unavailable.');
      return Promise.resolve(null);
    }
    if (!global.tlDialog || typeof global.tlDialog.open !== 'function') {
      toastError('Cannot Open File', 'The dialog stack is unavailable.');
      return Promise.resolve(null);
    }

    let resolveChoice = null;
    const promise = new Promise((resolve) => { resolveChoice = resolve; });
    const session = { promise };
    activeChoice = session;

    // ----- state -----
    let scopes = [];
    let scope = null;
    let cwd = '/';
    let entries = [];
    let visible = [];
    let selectedIndex = -1;
    let showHidden = false;
    let filterQuery = '';
    // Bumped by every navigation. A listing whose token is stale when it
    // lands is dropped: the user has already gone somewhere else, and
    // painting it would put the wrong rows under the current breadcrumb.
    let navToken = 0;

    // ----- elements -----
    const root = el('div', 'tl-filedlg');

    const scopeBar = el('div', 'tl-filedlg__scopes');
    scopeBar.setAttribute('role', 'group');
    scopeBar.setAttribute('aria-label', 'Location');
    root.appendChild(scopeBar);

    const navRow = el('div', 'tl-filedlg__nav');
    const upBtn = el('button', 'tl-filedlg__up', '↑');
    upBtn.type = 'button';
    upBtn.setAttribute('aria-label', 'Parent directory');
    upBtn.title = 'Parent directory';
    navRow.appendChild(upBtn);
    const crumbBar = el('div', 'tl-filedlg__crumbs');
    crumbBar.setAttribute('aria-label', 'Breadcrumbs');
    navRow.appendChild(crumbBar);
    root.appendChild(navRow);

    const controls = el('div', 'tl-filedlg__controls');
    const pathInput = el('input', 'tl-input tl-filedlg__path');
    pathInput.type = 'text';
    pathInput.spellcheck = false;
    pathInput.setAttribute('aria-label', 'Path');
    pathInput.setAttribute('placeholder', '/path/to/directory');
    controls.appendChild(pathInput);
    const filterInput = el('input', 'tl-input tl-filedlg__filter');
    filterInput.type = 'search';
    filterInput.setAttribute('aria-label', 'Filter by name');
    filterInput.setAttribute('placeholder', 'Filter…');
    controls.appendChild(filterInput);
    const hiddenLabel = el('label', 'tl-check tl-filedlg__hidden');
    const hiddenBox = el('input');
    hiddenBox.type = 'checkbox';
    hiddenLabel.appendChild(hiddenBox);
    hiddenLabel.appendChild(el('span', null, 'Hidden'));
    controls.appendChild(hiddenLabel);
    root.appendChild(controls);

    const box = el('div', 'tl-picker__box tl-scroll tl-filedlg__box');
    const head = el('div', 'tl-filedlg__head');
    head.appendChild(el('span', 'tl-filedlg__col tl-filedlg__col--name', 'Name'));
    head.appendChild(el('span', 'tl-filedlg__col tl-filedlg__col--size', 'Size'));
    head.appendChild(el('span', 'tl-filedlg__col tl-filedlg__col--time', 'Modified'));
    box.appendChild(head);
    const list = el('div', 'tl-filedlg__list');
    list.setAttribute('role', 'listbox');
    list.setAttribute('aria-label', 'Files');
    list.tabIndex = 0;
    box.appendChild(list);
    const emptyEl = el('div', 'tl-filedlg__empty', 'No matches');
    emptyEl.hidden = true;
    box.appendChild(emptyEl);
    root.appendChild(box);

    // Inline, in the body, next to the list it describes — deliberately NOT a
    // toast. A toast for "cannot list /root" would fly away over the terminal
    // while the dialog sat there showing nothing and explaining nothing.
    const errorEl = el('div', 'tl-filedlg__error');
    errorEl.setAttribute('role', 'alert');
    errorEl.hidden = true;
    root.appendChild(errorEl);

    let openButton = null;

    // ----- rendering -----

    function renderScopeBar() {
      clearChildren(scopeBar);
      for (const candidate of scopes) {
        const btn = el('button', 'tl-filedlg__scope', candidate.label);
        btn.type = 'button';
        const isActive = scope && candidate.id === scope.id;
        if (isActive) btn.className = 'tl-filedlg__scope is-active';
        btn.setAttribute('aria-pressed', isActive ? 'true' : 'false');
        if (candidate.kind === 'remote') btn.title = candidate.hostLabel;
        btn.addEventListener('click', () => {
          if (scope && candidate.id === scope.id) return;
          enterScope(candidate);
        });
        scopeBar.appendChild(btn);
      }
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

    function syncOpenButton() {
      const entry = selectedEntry();
      setDisabled(openButton, !entry || !!entry.is_dir);
    }

    function renderRows() {
      clearChildren(list);
      visible = model.sortEntries(model.filterEntries(entries, filterQuery, showHidden));
      if (selectedIndex >= visible.length) selectedIndex = -1;
      visible.forEach((entry, index) => {
        const row = el('div', 'tl-filedlg__row' + (index === selectedIndex ? ' is-selected' : ''));
        row.setAttribute('role', 'option');
        row.setAttribute('aria-selected', index === selectedIndex ? 'true' : 'false');
        row.tabIndex = -1;
        const name = el('span', 'tl-filedlg__cell tl-filedlg__cell--name', entry.name);
        if (entry.is_dir) name.className = 'tl-filedlg__cell tl-filedlg__cell--name is-dir';
        row.appendChild(name);
        row.appendChild(el('span', 'tl-filedlg__cell tl-filedlg__cell--size', entry.is_dir ? '' : formatSize(entry.size)));
        row.appendChild(el('span', 'tl-filedlg__cell tl-filedlg__cell--time', formatDate(entry.modified)));
        row.addEventListener('click', () => select(index));
        row.addEventListener('dblclick', () => { select(index); activate(); });
        list.appendChild(row);
      });
      emptyEl.hidden = visible.length !== 0 || !errorEl.hidden;
      syncOpenButton();
    }

    function select(index) {
      selectedIndex = index;
      const rows = list.children;
      for (let i = 0; i < rows.length; i++) {
        const isSelected = i === index;
        rows[i].className = 'tl-filedlg__row' + (isSelected ? ' is-selected' : '');
        rows[i].setAttribute('aria-selected', isSelected ? 'true' : 'false');
        if (isSelected && typeof rows[i].scrollIntoView === 'function') {
          rows[i].scrollIntoView({ block: 'nearest' });
        }
      }
      syncOpenButton();
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
      syncOpenButton();
      pathInput.value = path;
      renderCrumbs();
      list.setAttribute('aria-busy', 'true');
      try {
        const listed = await listScopeDir(scope, path);
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
      syncOpenButton();
      renderScopeBar();
      renderCrumbs();

      // Resolved once per scope and cached on it, so switching back and forth
      // does not re-round-trip.
      if (next.start == null) {
        list.setAttribute('aria-busy', 'true');
        let start;
        try {
          start = await resolveScopeStart(next);
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

    // Enter / double-click. A directory descends; a file finishes.
    function activate() {
      const entry = selectedEntry();
      if (!entry) return;
      if (entry.is_dir) {
        navigate(model.joinPath(cwd, entry.name));
        return;
      }
      finish({ scope, path: model.joinPath(cwd, entry.name), entry });
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
      syncOpenButton();
      list.setAttribute('aria-busy', 'true');
      let dirError = null;
      try {
        const listed = await listScopeDir(scope, typed);
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
        const listed = await listScopeDir(scope, parent);
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

    // ----- lifecycle -----

    // The latch. Escape and the backdrop both reach here through tl-dialog's
    // onClose, and so does the Cancel button and a successful pick; whichever
    // arrives first is the answer, and handle.close()'s own onClose cannot
    // overwrite it (same shape as dialog-service.js's confirmSave).
    let done = false;
    let handle = null;
    function finish(result) {
      if (done) return;
      done = true;
      activeChoice = null;
      resolveChoice(result || null);
      if (handle) handle.close(result ? 'open' : 'cancel');
    }

    handle = global.tlDialog.open({
      title: 'Open File',
      ariaLabel: 'Open file',
      size: 'lg',
      body: (bodyEl) => { bodyEl.appendChild(root); },
      buttons: [
        { label: 'Cancel', onSelect: () => finish(null) },
        { label: 'Open', primary: true, disabled: true, onSelect: () => activate() },
      ],
      onOpen: (panel) => {
        openButton = findFooterButton(panel, 'Open');
        setDisabled(openButton, true);
      },
      onClose: () => finish(null),
    });

    // Scope discovery is async, so the dialog is already on screen when it
    // lands — an empty scope bar for a few ms beats a ⌘O that appears to do
    // nothing while sessions are enumerated.
    loadScopes().then((built) => {
      if (done) return;
      scopes = built;
      renderScopeBar();
      return enterScope(scopes[0]);
    }).catch((error) => {
      if (done) return;
      showError(errorText(error));
    });

    return promise;
  }

  // ---------------------------------------------------------------------------
  // Public entry points
  // ---------------------------------------------------------------------------

  /**
   * ⌘O. Shows the chooser and routes the pick through the editor service's
   * existing open paths — no new open path, so the size cap, the extension
   * blocklist, the binary sniff and the focus-the-tab-you-already-have
   * behaviour all apply unchanged.
   */
  async function openForOpen() {
    const choice = await chooseFile();
    if (!choice) return null;
    const editor = global.termlabEditorService;
    if (!editor) {
      toastError('Editor Unavailable', 'The editor service is not loaded.');
      return null;
    }
    if (choice.scope.kind === 'local') {
      await editor.openLocalFile(choice.path);
      return choice;
    }
    await editor.openRemoteFile({
      paneId: choice.scope.paneId,
      remotePath: choice.path,
      hostLabel: choice.scope.hostLabel,
      size: choice.entry ? choice.entry.size : 0,
    });
    return choice;
  }

  /**
   * Save As. NOT IMPLEMENTED HERE — Task 6 of the editor-polish plan
   * implements it on top of the same chooser. It rejects rather than
   * resolving so a caller wired up early fails loudly instead of silently
   * appearing to have saved.
   */
  function openForSave(pane) {
    void pane;
    return Promise.reject(new Error('Save As is not implemented yet (Task 6 implements openForSave).'));
  }

  global.termlabFileDialog = {
    openForOpen,
    openForSave,
    // Exposed for scripts/tests/test_file_dialog.mjs — pure derivations that
    // need no DOM (matches tl-dialog.js's _zIndexForDepth precedent). The host
    // label formula is deliberately NOT among them: it belongs to
    // features/files/data-service.js and is pinned by a test there, so that
    // one test covers both this module and files-panel.js.
    _paneIdFromSessionKey: paneIdFromSessionKey,
    _buildScopes: buildScopes,
    _chooseFile: chooseFile,
  };
})(window);

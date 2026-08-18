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

  // Does this path already exist? Resolves with the FileEntry, rejects when it
  // does not exist (both backends return a bare String there). Save As reads
  // the rejection as "free to write" and the resolution as "ask first".
  function statScopePath(scope, path) {
    const data = filesData();
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

  function mkdirInScope(scope, path) {
    const data = filesData();
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
  // Nested prompts (Save As only)
  // ---------------------------------------------------------------------------
  //
  // Both stack ON TOP of the chooser rather than replacing it: answering
  // "no" has to leave the user where they were, with the same directory and
  // the same typed name, free to choose somewhere else. tl-dialog supports
  // the nesting (its z-index/Escape/Tab-trap handling is all depth-aware);
  // the `done` latch is dialog-service.js's confirmSave shape, so the onClose
  // that close() itself fires cannot overwrite an answer already given.

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

  // One dialog at a time. A second ⌘O while the chooser is up should surface
  // the dialog that is already open, not stack a second modal over it.
  let activeChoice = null;

  /**
   * Show the chooser. Resolves with
   *   { scope, path, entry }   — a file the user picked (in save mode `entry`
   *                              is the existing file being replaced, or null)
   *   null                     — cancelled (Cancel, Escape, backdrop, or a
   *                              failure to even build the scope list)
   * It never rejects: a cancelled chooser is not an error.
   *
   * options = { mode: 'open' | 'save', filename, selectFilename }
   *   'save' adds the filename field (pre-filled with `filename`), the New
   *   Folder button and the existence check; the primary button reads Save.
   *   `selectFilename` focuses that field with its text selected — for a
   *   placeholder name (an untitled buffer's "Untitled-2") that is there to be
   *   typed over rather than edited.
   */
  function chooseFile(options) {
    if (activeChoice) return activeChoice.promise;

    const opts = options || {};
    const saveMode = opts.mode === 'save';
    const failTitle = saveMode ? 'Cannot Save File' : 'Cannot Open File';

    const model = fileModel();
    if (!model) {
      toastError(failTitle, 'The file dialog model is unavailable.');
      return Promise.resolve(null);
    }
    if (!global.tlDialog || typeof global.tlDialog.open !== 'function') {
      toastError(failTitle, 'The dialog stack is unavailable.');
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

    // Save mode only: the name to write, plus the one directory-creating
    // affordance a save dialog needs. Built (and appended) only in save mode
    // so the open chooser is byte-for-byte the dialog Task 5 shipped.
    let nameInput = null;
    let newFolderBtn = null;
    if (saveMode) {
      const saveRow = el('div', 'tl-filedlg__save');
      const nameLabel = el('label', 'tl-filedlg__namelabel', 'Save As:');
      saveRow.appendChild(nameLabel);
      nameInput = el('input', 'tl-input tl-filedlg__name');
      nameInput.type = 'text';
      nameInput.spellcheck = false;
      nameInput.setAttribute('aria-label', 'File name');
      nameInput.setAttribute('placeholder', 'File name');
      nameInput.value = String(opts.filename || '');
      saveRow.appendChild(nameInput);
      newFolderBtn = el('button', 'tl-btn tl-filedlg__newfolder', 'New Folder');
      newFolderBtn.type = 'button';
      saveRow.appendChild(newFolderBtn);
      root.appendChild(saveRow);
    }

    // Inline, in the body, next to the list it describes — deliberately NOT a
    // toast. A toast for "cannot list /root" would fly away over the terminal
    // while the dialog sat there showing nothing and explaining nothing.
    const errorEl = el('div', 'tl-filedlg__error');
    errorEl.setAttribute('role', 'alert');
    errorEl.hidden = true;
    root.appendChild(errorEl);

    // "Open" or "Save", depending on the mode.
    let primaryButton = null;

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
          // `enterScope` sets `scope = candidate` BEFORE `resolveScopeStart`
          // runs, so a scope whose start failed to resolve is left "active"
          // with `start` still null. Guarding on id alone would make that
          // button permanently dead — the same-scope click that should retry
          // it would instead no-op forever. Only skip re-entering a scope
          // that is both active AND already has a resolved start.
          if (scope && candidate.id === scope.id && candidate.start != null) return;
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
      syncPrimaryButton();
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
          existing = await statScopePath(scope, path);
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
        await mkdirInScope(scope, targetPathFor(name));
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

    // ----- lifecycle -----

    // Put the caret in the name field with the placeholder selected, so the
    // first keystroke replaces "Untitled-2" instead of appending to it.
    //
    // Scheduled rather than done inline: tl-dialog focuses the first focusable
    // element in the panel from a requestAnimationFrame callback it queued
    // BEFORE onOpen runs, so a plain focus() here would be undone one frame
    // later. rAF callbacks run in registration order, so ours lands after it.
    // With no rAF (the test sandboxes) that focus already happened
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

    const primaryLabel = saveMode ? 'Save' : 'Open';
    handle = global.tlDialog.open({
      title: saveMode ? 'Save File As' : 'Open File',
      ariaLabel: saveMode ? 'Save file as' : 'Open file',
      size: 'lg',
      body: (bodyEl) => { bodyEl.appendChild(root); },
      buttons: [
        { label: 'Cancel', onSelect: () => finish(null) },
        {
          label: primaryLabel,
          primary: true,
          disabled: true,
          onSelect: () => (saveMode ? attemptSave() : activate()),
        },
      ],
      onOpen: (panel) => {
        primaryButton = findFooterButton(panel, primaryLabel);
        // Save starts enabled when the pane's current name was pre-filled,
        // so ⌘⇧S → Return saves under the same name in a new place.
        syncPrimaryButton();
        if (saveMode && opts.selectFilename && nameInput) selectNameField();
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

  function basename(p) {
    const segments = String(p || '').split('/').filter(Boolean);
    return segments.length ? segments[segments.length - 1] : '';
  }

  /**
   * ⌘⇧S. Shows the same chooser in save mode and routes the chosen target
   * through `termlabEditorService.saveAs`, which owns the write, the upload
   * and the pane rebind — this file never touches pane identity.
   *
   * Resolves with the chooser's result, or null when it was cancelled or the
   * save failed. It does not reject: `saveAs` has already told the user what
   * went wrong (and left the pane on its old binding), so a second report
   * here would only say it twice.
   */
  async function openForSave(pane) {
    if (!pane || pane.kind !== 'editor') return null;

    // A chooser is already on screen. chooseFile's `activeChoice`
    // short-circuit would hand this caller THAT dialog's answer — and a path
    // the user picked for one pane is never where another pane's buffer
    // belongs: both tabs would rebind to one file and the second write would
    // bury the first, with nothing on screen to say so. Refuse instead. The
    // dialog they are looking at is the one they are answering, and savePane
    // reads a null here as a quiet not-saved (no toast, no close).
    //
    // Same-pane re-entry does not reach this: editor-service parks a
    // per-pane entry for the duration of the await and joins it. This is the
    // backstop for every other route, including a window-close sweep that
    // walks a second dirty pane while the first pane's chooser is up.
    if (activeChoice) return null;
    // The name the USER knows this file by. For a remote pane that is the
    // remote basename — pane.filePath is the local temp file it was
    // downloaded into, which they have never seen.
    // An untitled buffer has no path to take a basename from. Its tab label
    // ("Untitled-2") is the only name it has, so that is what the field is
    // seeded with — selected, because it is a placeholder to type over rather
    // than a name to extend.
    const untitled = !pane.filePath && !pane.remote;
    const labels = global.termlabEditorTabLabel;
    let currentName;
    if (pane.remote) {
      currentName = basename(pane.remote.remotePath);
    } else if (untitled && labels && typeof labels.editorTabLabel === 'function') {
      currentName = labels.editorTabLabel(pane).label;
    } else {
      currentName = basename(pane.filePath);
    }

    const choice = await chooseFile({
      mode: 'save',
      filename: currentName,
      selectFilename: untitled,
    });
    if (!choice) return null;

    const editor = global.termlabEditorService;
    if (!editor || typeof editor.saveAs !== 'function') {
      toastError('Editor Unavailable', 'The editor service is not loaded.');
      return null;
    }

    const target = choice.scope.kind === 'local'
      ? { scope: 'local', path: choice.path }
      : {
        scope: 'remote',
        paneId: choice.scope.paneId,
        // The CLEAN identity string, never the button's disambiguated
        // caption: editor_temp_path hashes this into the file's temp path,
        // so a " (pane N)" suffix here would split one remote file across
        // two tabs.
        hostLabel: choice.scope.hostLabel,
        remotePath: choice.path,
      };

    try {
      await editor.saveAs(pane, target);
    } catch (_) {
      return null;
    }
    return choice;
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

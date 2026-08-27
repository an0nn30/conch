// Next/Previous Problem (F8 / Shift-F8) and row activation.
//
// Both entry points — the keyboard and a click in the Problems list — end up
// here, so "open the file, focus the owner, select the range" has exactly one
// implementation and the panel cannot drift from the shortcut.
//
// It opens files through `termlabEditorService.openLocalFile` and nothing
// else. That call owns the app-wide ownership protocol: it focuses an already
// open tab, or reserves the document and, when another WINDOW already owns it,
// hands the focus request to Rust instead of opening a second view of the same
// bytes. Re-implementing any of that here would be how two windows end up
// editing one file.
//
// It registers no key listeners. `editor_next_problem`/`editor_previous_problem`
// are configurable bindings the keyboard router already owns; shortcut-runtime
// dispatches them as `termlab:editor-next-problem` window events after it has
// decided the pane scope, which is why F8 in a terminal pane still reaches the
// shell.
(function initTermLabProblemsNavigation(global) {
  'use strict';

  const LIVE_REGION_ID = 'problems-live-region';

  let paneAccess = null;
  let announceHook = null;
  let shortcutHandlers = null;

  function store() {
    return global.termlabProblemsStore || null;
  }

  function model() {
    return global.termlabProblemsModel || null;
  }

  function editorService() {
    return global.termlabEditorService || null;
  }

  // A popped-out Problems window has no editor of its own: editor-service's
  // createEditorTab throws unless manager-compose-runtime has run in THIS
  // window, and a panel-host boot skips that module entirely.
  // `__termlabCreateEditorTab` is exactly what createEditorTab gates on, so
  // it is the true test — `window.termlabEditorService` is not, because its
  // <script> tag loads in every window index.html serves. The same test and
  // the same escape hatch as files-panel.js's openInEditor.
  function canOpenLocally() {
    return typeof global.__termlabCreateEditorTab === 'function';
  }

  let hostActionBridge;
  function hostActions() {
    if (hostActionBridge !== undefined) return hostActionBridge;
    const client = global.termlabServices && global.termlabServices.tauriClient;
    const factory = global.termlabPanelHostBridge;
    hostActionBridge = (client && typeof client.invoke === 'function'
      && factory && typeof factory.create === 'function')
      ? factory.create({ invoke: (command, args) => client.invoke(command, args) })
      : null;
    return hostActionBridge;
  }

  // --- announcements ----------------------------------------------------------
  //
  // A polite live region, never focused: the point is to tell a screen reader
  // where the caret just went without taking focus away from the editor the
  // reveal just scrolled.

  function liveRegion() {
    const doc = global.document;
    if (!doc || !doc.body) return null;
    let region = doc.getElementById(LIVE_REGION_ID);
    if (region) return region;
    region = doc.createElement('div');
    region.id = LIVE_REGION_ID;
    region.className = 'tl-visually-hidden';
    region.setAttribute('aria-live', 'polite');
    region.setAttribute('aria-atomic', 'true');
    region.setAttribute('role', 'status');
    doc.body.appendChild(region);
    return region;
  }

  function announce(text) {
    if (typeof announceHook === 'function') {
      announceHook(text);
      return;
    }
    const region = liveRegion();
    if (region) region.textContent = String(text || '');
  }

  function announcementFor(item, index, total) {
    const builder = model();
    const severity = builder ? builder.severityLabel(item.severity) : item.severity;
    const where = `${item.fileName}:${item.line}:${item.column}`;
    return `${index + 1} of ${total}: ${severity} in ${where}. ${item.message}`;
  }

  // --- reveal -----------------------------------------------------------------

  function paneForPath(filePath) {
    if (!paneAccess || typeof paneAccess.allPanes !== 'function') return null;
    const panes = paneAccess.allPanes();
    if (!panes || typeof panes.values !== 'function') return null;
    for (const pane of panes.values()) {
      if (pane && pane.kind === 'editor' && !pane.remote && pane.filePath === filePath) return pane;
    }
    return null;
  }

  function offsetAt(doc, position) {
    const line = Number(position && position.line);
    const character = Number(position && position.character);
    const wanted = Number.isFinite(line) ? line + 1 : 1;
    const entry = doc.line(Math.min(Math.max(wanted, 1), doc.lines));
    const column = Number.isFinite(character) && character > 0 ? character : 0;
    return Math.min(entry.from + column, entry.to);
  }

  function reveal(pane, item, options) {
    const CM = global.CM6;
    if (!pane || !pane.view || !pane.view.state || !item.range) return false;
    const doc = pane.view.state.doc;
    const anchor = offsetAt(doc, item.range.start);
    const head = Math.max(offsetAt(doc, item.range.end), anchor);
    const spec = { selection: { anchor, head } };
    if (CM && CM.EditorView && typeof CM.EditorView.scrollIntoView === 'function') {
      spec.effects = CM.EditorView.scrollIntoView(anchor, { y: 'center' });
    } else {
      spec.scrollIntoView = true;
    }
    pane.view.dispatch(spec);
    if (!options || options.focus !== false) {
      if (typeof pane.view.focus === 'function') pane.view.focus();
    }
    return true;
  }

  // --- activation -------------------------------------------------------------

  async function activate(item, options) {
    if (!item || !item.path) return false;
    if (!canOpenLocally()) {
      // Hand the open to the parent window, which has the editor. The range
      // does not travel: `open-in-editor` carries a path, and widening that
      // closed action list is a change to the panel-host protocol, not to
      // this window. The row still takes the user to the right file.
      const bridge = hostActions();
      if (bridge && typeof bridge.publishAction === 'function') {
        bridge.publishAction('open-in-editor', { kind: 'local', path: item.path });
      }
      return false;
    }
    const service = editorService();
    if (!service || typeof service.openLocalFile !== 'function') return false;
    try {
      await service.openLocalFile(item.path);
    } catch (error) {
      // A stale or deleted target is reported by the open flow's own toast.
      // The diagnostic itself stays put: only the server may withdraw it.
      console.warn('Problems: could not open the diagnostic target', error);
      return false;
    }
    const pane = paneForPath(item.path);
    // No local pane means another window owns the document; `openLocalFile`
    // has already asked Rust to focus that owner, and reaching further would
    // be this window claiming a document it does not hold.
    if (!pane) return false;
    if (paneAccess) {
      if (typeof paneAccess.activateTab === 'function') paneAccess.activateTab(pane.tabId);
      if (typeof paneAccess.setFocusedPane === 'function') paneAccess.setFocusedPane(pane.paneId);
    }
    return reveal(pane, item, options);
  }

  // --- traversal --------------------------------------------------------------

  async function step(delta) {
    const current = store();
    const builder = model();
    if (!current || !builder) return null;
    const list = current.orderedItems();
    if (!list.length) {
      announce('No problems.');
      return null;
    }
    const selectedId = current.getSelectedId();
    const at = list.findIndex((entry) => entry.id === selectedId);
    const index = builder.stepIndex(list.length, at, delta);
    const item = list[index];
    current.select(item.id);
    // Focus follows the reveal: F8 is pressed in the editor, and the editor is
    // where the user expects to keep typing.
    await activate(item, { focus: true });
    announce(announcementFor(item, index, list.length));
    return item;
  }

  function next() { return step(1); }
  function previous() { return step(-1); }

  // --- lifecycle --------------------------------------------------------------

  function configure(options) {
    const opts = options || {};
    if (opts.paneAccess) paneAccess = opts.paneAccess;
    else if (!paneAccess) paneAccess = global.__termlabPaneAccess || null;
    if (typeof opts.announce === 'function') announceHook = opts.announce;
    if (!shortcutHandlers && typeof global.addEventListener === 'function') {
      shortcutHandlers = {
        'termlab:editor-next-problem': () => { next(); },
        'termlab:editor-previous-problem': () => { previous(); },
      };
      for (const name of Object.keys(shortcutHandlers)) {
        global.addEventListener(name, shortcutHandlers[name]);
      }
    }
  }

  function dispose() {
    if (shortcutHandlers && typeof global.removeEventListener === 'function') {
      for (const name of Object.keys(shortcutHandlers)) {
        global.removeEventListener(name, shortcutHandlers[name]);
      }
    }
    shortcutHandlers = null;
  }

  global.termlabProblemsNavigation = {
    configure,
    dispose,
    next,
    previous,
    activate,
    announce,
    announcementFor,
  };
})(window);

// Go to Definition, the multiple-definition chooser, and this window's
// back/forward navigation history.
//
// Three responsibilities, one small module:
//
//   * ask for a definition (through editor-service's requestFeature, which owns
//     the flush/version barrier — never through Tauri directly);
//   * take the user there THROUGH editor-service, which is the app-wide
//     ownership authority: it focuses an already open tab, reserves an unopened
//     file, and hands a document another WINDOW owns to Rust to focus instead
//     of opening a second editable view of the same bytes. Nothing here opens,
//     reads or reserves a file itself;
//   * remember where the jump started, so Back and Forward can restore the tab,
//     the caret and the selection.
//
// Two halves live next door, because neither is about deciding where to go:
// lsp-navigation-history.js owns the bounded per-window stacks and what a
// location is, and lsp-navigation-chooser.js owns the candidate list's
// CodeMirror field and DOM. This file is the orchestration between them, the
// definition request, and editor-service.
//
// Server URIs are converted by lsp-uri.js and by nothing else, and only
// `file:` targets are accepted: a `jdt://` or `untitled:` target is reported as
// unsupported rather than turned into a nonsense path.
//
// The chooser is a CodeMirror tooltip anchored at the requested position, so
// CodeMirror places it and maps it; its keys are a Prec.highest domEventHandler
// because vim is a ViewPlugin whose own keydown handler runs ahead of every
// keymap, and Enter/Escape/arrows have to reach an open chooser first. With no
// chooser open the handler returns false and vim is untouched.
(function initTermLabLspNavigation(global) {
  'use strict';

  const PREVIEW_LIMIT = 120;

  let paneForViewHook = null;
  let currentPaneHook = null;
  let allPanesHook = null;
  let requestFeatureHook = null;
  let openLocalFileAtHook = null;
  let windowLabel = null;
  let windowHandlers = null;

  // view -> { sequence }. Bumping the sequence cancels an in-flight request:
  // its answer describes a caret the user has already left.
  const entries = new WeakMap();

  // Built once, on first use: two mounts of extensions() must install the same
  // handlers, not two competing sets.
  let mounted = null;

  function cm() {
    return global.CM6 || null;
  }

  function doc() {
    return global.document;
  }

  function uriModule() {
    return global.termlabLspUri || null;
  }

  function editorService() {
    return global.termlabEditorService || null;
  }

  // --- pane and document lookup ------------------------------------------------

  function paneForView(view) {
    if (typeof paneForViewHook === 'function') return paneForViewHook(view) || null;
    const access = global.__termlabPaneAccess;
    if (!access || typeof access.allPanes !== 'function') return null;
    const panes = access.allPanes();
    if (!panes || typeof panes.values !== 'function') return null;
    for (const pane of panes.values()) {
      if (pane && pane.view === view) return pane;
    }
    return null;
  }

  function currentPane() {
    if (typeof currentPaneHook === 'function') return currentPaneHook() || null;
    const access = global.__termlabPaneAccess;
    return access && typeof access.currentPane === 'function' ? access.currentPane() : null;
  }

  function allPanes() {
    if (typeof allPanesHook === 'function') return allPanesHook() || null;
    const access = global.__termlabPaneAccess;
    return access && typeof access.allPanes === 'function' ? access.allPanes() : null;
  }

  // A pane that may be asked for a definition: a local editor with a committed
  // document whose session advertises the feature. Remote panes are excluded
  // here as well as in Rust — a remote buffer never enters the registry.
  function documentStateFor(pane) {
    if (!pane || pane.kind !== 'editor' || !pane.view || pane.remote) return null;
    const store = global.termlabLspState;
    if (!store || typeof store.get !== 'function') return null;
    const state = store.get(pane);
    if (!state || !state.documentId) return null;
    const capabilities = state.capabilities || (state.status && state.status.capabilities) || {};
    if (capabilities.definition !== true) return null;
    return state;
  }

  // --- status ---------------------------------------------------------------------
  //
  // Every failure here is non-blocking by design: the current editor does not
  // move, and the user is told why in a toast rather than in a dialog.

  function status(title, body) {
    if (global.toast && typeof global.toast.info === 'function') {
      global.toast.info(title, body);
    }
  }

  // --- history ------------------------------------------------------------------------
  //
  // The stacks, the bound, and what a location is: lsp-navigation-history.js.

  function history() {
    return global.termlabLspNavigationHistory || null;
  }

  function captureLocation(pane, pos) {
    const store = history();
    return store ? store.capture(pane, pos, windowLabel) : null;
  }

  function historyState() {
    const store = history();
    return store ? store.state() : { back: [], forward: [] };
  }

  // --- targets ---------------------------------------------------------------------------

  function clamp(value, low, high) {
    return Math.min(Math.max(value, low), high);
  }

  function positionAt(document, offset) {
    const helper = global.termlabLspPosition;
    return helper ? helper.positionAt(document, offset) : { line: 0, character: 0 };
  }

  function isFileUri(value) {
    const uri = uriModule();
    return !!(uri && typeof uri.isFileUri === 'function' && uri.isFileUri(value));
  }

  function basename(value) {
    const at = String(value).lastIndexOf('/');
    return at < 0 ? String(value) : String(value).slice(at + 1);
  }

  function dirname(value) {
    const at = String(value).lastIndexOf('/');
    return at <= 0 ? '/' : String(value).slice(0, at);
  }

  // The line the target sits on, when this window already has that file open.
  // A file it has NOT opened gets no preview: reading arbitrary files off disk
  // to decorate a list is a cost the chooser does not need to pay, and the row
  // still says where the target lives.
  function previewFor(filePath, line) {
    const panes = allPanes();
    if (!panes || typeof panes.values !== 'function') return null;
    const helper = global.termlabLspPosition;
    if (!helper) return null;
    for (const pane of panes.values()) {
      if (!pane || pane.kind !== 'editor' || pane.remote || pane.filePath !== filePath) continue;
      const state = pane.view && pane.view.state;
      if (!state || !state.doc) return null;
      const text = helper.lineTextAt(state.doc, line).trim();
      return text ? text.slice(0, PREVIEW_LIMIT) : null;
    }
    return null;
  }

  // Server locations -> what the chooser and the jump both consume. Rust has
  // already collapsed `LocationLink` onto its target selection range, so both
  // result shapes arrive here as plain locations.
  function normalizeLocations(response) {
    const raw = (response && Array.isArray(response.locations)) ? response.locations : [];
    const uri = uriModule();
    const targets = [];
    let rejected = 0;
    for (const entry of raw) {
      if (!entry || typeof entry.uri !== 'string' || !entry.range) continue;
      if (!isFileUri(entry.uri)) {
        rejected += 1;
        continue;
      }
      const filePath = uri.uriToPath(entry.uri);
      const line = Number.isInteger(entry.range.start && entry.range.start.line)
        ? entry.range.start.line
        : 0;
      const character = Number.isInteger(entry.range.start && entry.range.start.character)
        ? entry.range.start.character
        : 0;
      targets.push({
        uri: entry.uri,
        path: filePath,
        range: entry.range,
        name: basename(filePath),
        context: dirname(filePath),
        line: line + 1,
        column: character + 1,
        preview: previewFor(filePath, line),
      });
    }
    return { targets, rejected };
  }

  // --- the jump -------------------------------------------------------------------------
  //
  // One route to a target, for definitions and for both history directions.
  // Everything goes through editor-service: it focuses an open tab, opens an
  // unopened file under a reservation, or reports that another window owns the
  // document (in which case Rust has already focused that window and this one
  // must not open a second view).

  function openLocalFileAt(filePath, range) {
    if (typeof openLocalFileAtHook === 'function') {
      return Promise.resolve(openLocalFileAtHook(filePath, range, { focus: true }));
    }
    const service = editorService();
    if (!service || typeof service.openLocalFileAt !== 'function') {
      return Promise.resolve({ status: 'unavailable' });
    }
    return Promise.resolve(service.openLocalFileAt(filePath, range, { focus: true }));
  }

  async function jumpTo(target) {
    let result = null;
    try {
      result = await openLocalFileAt(target.path, target.range);
    } catch (error) {
      status('Cannot Navigate', String(error));
      return 'failed';
    }
    const outcome = result && result.status;
    if (outcome === 'ownerElsewhere') return 'elsewhere';
    if (outcome === 'focused' || outcome === 'opened') {
      // The file is open, but only a completed reveal is a jump. A degraded
      // open (no ownership bridge, so no pane to select in) reports `revealed`
      // as false or not at all, and recording that as history would give Back
      // a step that returns to a place the user was never taken to. No toast:
      // nothing failed that the user asked for.
      return result.revealed === true ? 'navigated' : 'unrevealed';
    }
    status(
      'Cannot Navigate',
      result && result.error
        ? String(result.error)
        : `${target.name || target.path} could not be opened.`,
    );
    return 'failed';
  }

  // --- the chooser ------------------------------------------------------------------------
  //
  // Its CodeMirror field and its DOM: lsp-navigation-chooser.js.

  let chooserConfigured = false;

  function chooser() {
    const list = global.termlabLspNavigationChooser || null;
    if (list && !chooserConfigured) {
      chooserConfigured = true;
      // What a picked row MEANS — jump there, and record where the jump began —
      // is this file's business, not the list's.
      list.configure({ onChoose: (view, index) => { choose(view, index); } });
    }
    return list;
  }

  function chooserState(view) {
    const list = chooser();
    return list ? list.state(view) : { open: false, index: 0, items: [] };
  }

  function closeChooser(view) {
    const list = chooser();
    if (list) list.close(view);
  }

  // Asked by lsp-tooltips before it opens anything of its own, the same way it
  // asks CodeMirror whether the completion popup is up. The one-overlay rule
  // has to hold in both directions: opening the chooser dismisses hover and
  // signature help, and while it is open they stand down.
  function chooserOpen(view) {
    const list = chooser();
    return !!list && list.isOpen(view);
  }

  function renderChooser(value, view) {
    const list = chooser();
    return list ? list.render(value, view) : doc().createElement('div');
  }

  function openChooser(view, targets, pos, origin) {
    const list = chooser();
    return !!list && list.open(view, targets, pos, origin);
  }

  function moveChooser(view, delta) {
    const list = chooser();
    return !!list && list.move(view, delta);
  }

  async function choose(view, index) {
    const list = chooser();
    const value = list ? list.valueOf(view) : null;
    if (!value || !value.items.length) return 'none';
    const at = Number.isInteger(index) ? index : value.index;
    const target = value.items[at];
    const origin = value.origin;
    closeChooser(view);
    if (!target) return 'none';
    const outcome = await jumpTo(target);
    if (outcome === 'navigated') record(origin);
    return outcome;
  }

  // --- requests ----------------------------------------------------------------------------

  function entryFor(view) {
    let entry = entries.get(view);
    if (!entry) {
      entry = { sequence: 0 };
      entries.set(view, entry);
    }
    return entry;
  }

  function requestFeature(pane, position) {
    if (typeof requestFeatureHook === 'function') {
      return Promise.resolve(requestFeatureHook(pane, 'definition', position, null));
    }
    const service = editorService();
    if (!service || typeof service.requestFeature !== 'function') return Promise.resolve(null);
    return Promise.resolve(service.requestFeature(pane, 'definition', position, null));
  }

  // Only a jump that actually moved THIS window is history. A target another
  // window owns leaves this editor exactly where it was, so recording it would
  // give Back a step that undoes nothing.
  function record(origin) {
    const store = history();
    if (store) store.record(origin);
  }

  function viewOrCurrent(view) {
    if (view) return view;
    const pane = currentPane();
    return pane && pane.kind === 'editor' ? pane.view : null;
  }

  async function goToDefinition(view, pos) {
    const target = viewOrCurrent(view);
    if (!target || !target.state) return 'unavailable';
    const pane = paneForView(target) || currentPane();
    const state = documentStateFor(pane);
    if (!state) return 'unavailable';
    const at = Number.isInteger(pos)
      ? clamp(pos, 0, target.state.doc.length)
      : target.state.selection.main.head;
    const origin = captureLocation(pane, at);

    const entry = entryFor(target);
    entry.sequence += 1;
    const sequence = entry.sequence;
    const documentId = state.documentId;

    let response = null;
    try {
      response = await requestFeature(pane, positionAt(target.state.doc, at));
    } catch (error) {
      status('Cannot Navigate', String(error));
      return 'failed';
    }
    // A newer request has been made from this view; this answer describes a
    // caret the user has left.
    if (entryFor(target).sequence !== sequence) return 'stale';
    if (!response || response.documentId !== documentId) return 'none';

    const { targets, rejected } = normalizeLocations(response);
    if (!targets.length) {
      if (rejected) {
        status(
          'Definition Not Available',
          'The definition is outside the local filesystem, so it cannot be opened here.',
        );
        return 'unsupported';
      }
      status('No Definition Found', 'The language server reported no definition for this symbol.');
      return 'none';
    }
    if (targets.length > 1) {
      if (openChooser(target, targets, at, origin)) return 'chooser';
    }
    const outcome = await jumpTo(targets[0]);
    if (outcome === 'navigated') record(origin);
    return outcome;
  }

  // --- back and forward -----------------------------------------------------------------------
  //
  // Two outcomes consume the entry, and the policy is deliberate:
  //
  //   'navigated'  — this window went there.
  //   'elsewhere'  — the document has moved to another WINDOW since the entry
  //                  was recorded (a Save As, or an owner that opened later),
  //                  and that window has just been focused. That IS the
  //                  navigation the user asked for, so the step completes.
  //                  Leaving the entry on top instead would make every further
  //                  Back focus the same window forever, with every older
  //                  entry permanently out of reach.
  //
  // A genuine failure — the file was deleted, or nothing could be selected —
  // consumes nothing: the file may come back, and a second press must not
  // silently become a jump two places back.
  async function step(direction) {
    const store = history();
    const entry = store ? store.peek(direction) : null;
    if (!entry) return 'none';
    const here = captureLocation(currentPane(), null);
    const outcome = await jumpTo(store.entryTarget(entry));
    if (outcome !== 'navigated' && outcome !== 'elsewhere') return outcome;
    store.advance(direction, here);
    return outcome;
  }

  function navigateBack() {
    return step('back');
  }

  function navigateForward() {
    return step('forward');
  }

  // --- events -----------------------------------------------------------------------------------

  function chooserKey(event) {
    if (!event || event.metaKey || event.ctrlKey || event.altKey || event.shiftKey) return null;
    const key = event.key;
    if (key === 'ArrowDown' || key === 'ArrowUp' || key === 'Enter') return key;
    if (key === 'Escape' || key === 'Esc') return 'Escape';
    return null;
  }

  function handleKeydown(event, view) {
    const key = chooserKey(event);
    if (!key) return false;
    if (!chooserOpen(view)) return false;
    if (key === 'ArrowDown') return moveChooser(view, 1);
    if (key === 'ArrowUp') return moveChooser(view, -1);
    if (key === 'Escape') {
      closeChooser(view);
      return true;
    }
    choose(view);
    return true;
  }

  // Command-click. Deliberately NOT Ctrl-click: on macOS that gesture is the
  // context menu. The click is not consumed — the caret still lands where the
  // user clicked, and the definition request rides along.
  function handleMousedown(event, view) {
    if (!event || !event.metaKey || event.altKey || event.ctrlKey) return false;
    if (Number.isInteger(event.button) && event.button !== 0) return false;
    if (!view || typeof view.posAtCoords !== 'function') return false;
    const pos = view.posAtCoords({ x: event.clientX, y: event.clientY });
    if (pos === null || pos === undefined) return false;
    goToDefinition(view, pos);
    return false;
  }

  // --- extensions -------------------------------------------------------------------------------

  function extensions() {
    const CM = cm();
    const module = chooser();
    const field = module ? module.field() : null;
    if (!CM || !field) return [];
    if (mounted) return mounted;
    const list = [field];
    if (
      CM.Prec && typeof CM.Prec.highest === 'function'
      && CM.EditorView && typeof CM.EditorView.domEventHandlers === 'function'
    ) {
      list.push(CM.Prec.highest(CM.EditorView.domEventHandlers({
        keydown: (event, view) => handleKeydown(event, view),
        mousedown: (event, view) => handleMousedown(event, view),
      })));
    }
    mounted = list;
    return mounted;
  }

  // --- lifecycle ---------------------------------------------------------------------------------

  // F12, Ctrl-minus and Ctrl-Shift-minus arrive as window events
  // (shortcut-runtime dispatches `termlab:editor-go-to-definition`,
  // `termlab:editor-navigate-back` and `termlab:editor-navigate-forward` after
  // it has decided the pane scope), so a terminal pane keeps those keys.
  function installWindowHandlers() {
    if (windowHandlers || typeof global.addEventListener !== 'function') return;
    windowHandlers = {
      'termlab:editor-go-to-definition': () => { goToDefinition(); },
      'termlab:editor-navigate-back': () => { navigateBack(); },
      'termlab:editor-navigate-forward': () => { navigateForward(); },
    };
    for (const name of Object.keys(windowHandlers)) {
      global.addEventListener(name, windowHandlers[name]);
    }
  }

  function configure(options) {
    const opts = options || {};
    if (typeof opts.paneForView === 'function') paneForViewHook = opts.paneForView;
    if (typeof opts.currentPane === 'function') currentPaneHook = opts.currentPane;
    if (typeof opts.allPanes === 'function') allPanesHook = opts.allPanes;
    if (typeof opts.requestFeature === 'function') requestFeatureHook = opts.requestFeature;
    if (typeof opts.openLocalFileAt === 'function') openLocalFileAtHook = opts.openLocalFileAt;
    if (opts.windowLabel) windowLabel = String(opts.windowLabel);
    installWindowHandlers();
  }

  function dispose() {
    if (windowHandlers && typeof global.removeEventListener === 'function') {
      for (const name of Object.keys(windowHandlers)) {
        global.removeEventListener(name, windowHandlers[name]);
      }
    }
    windowHandlers = null;
  }

  global.termlabLspNavigation = {
    configure,
    dispose,
    extensions,
    goToDefinition,
    navigateBack,
    navigateForward,
    historyState,
    chooserState,
    chooserOpen,
    closeChooser,
    moveChooser,
    choose,
    handleKeydown,
    handleMousedown,
    renderChooser,
    normalizeLocations,
    captureLocation,
  };
})(window);

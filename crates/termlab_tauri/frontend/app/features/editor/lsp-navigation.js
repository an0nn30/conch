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
//   * remember where the jump started, bounded, so Back and Forward can restore
//     the tab, the caret and the selection.
//
// The history is per webview window on purpose. Ownership is app-wide, but
// "where I was" is not: two windows are two reading positions, and a shared
// stack would make Back in one window yank the other one's editor around.
// Entries therefore carry the owner (window label and pane) they were recorded
// against as a preference, while the actual routing decision stays with
// editor-service on every jump — the document may have moved windows since.
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

  // Bounded per window. Deep enough that Back keeps working across a long
  // reading session, small enough that it cannot grow without limit.
  const MAX_HISTORY = 100;
  const PREVIEW_LIMIT = 120;

  let paneForViewHook = null;
  let currentPaneHook = null;
  let allPanesHook = null;
  let requestFeatureHook = null;
  let openLocalFileAtHook = null;
  let windowLabel = null;
  let windowHandlers = null;

  const backStack = [];
  const forwardStack = [];

  // view -> { sequence }. Bumping the sequence cancels an in-flight request:
  // its answer describes a caret the user has already left.
  const entries = new WeakMap();

  // Built once, on first use: two mounts of extensions() must install the same
  // field, not two competing ones.
  let chooserEffect = null;
  let chooserField = null;
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

  // --- positions --------------------------------------------------------------------

  function lspPositionAt(document, offset) {
    const line = document.lineAt(offset);
    return { line: line.number - 1, character: offset - line.from };
  }

  function clamp(value, low, high) {
    return Math.min(Math.max(value, low), high);
  }

  function samePosition(position) {
    return { line: position.line, character: position.character };
  }

  // --- history ------------------------------------------------------------------------

  function pushBounded(stack, entry) {
    stack.push(entry);
    while (stack.length > MAX_HISTORY) stack.shift();
  }

  // Where a jump is starting from. `pos` is the position the request was made
  // at — the caret for F12, the clicked character for Command-click — because
  // that, not wherever the caret drifts to while the server thinks, is where
  // Back should return to. A non-empty selection is preserved as the range so
  // Back restores it too.
  function captureLocation(pane, pos) {
    const uri = uriModule();
    if (!pane || pane.kind !== 'editor' || !pane.view || pane.remote || !pane.filePath) return null;
    if (!uri || typeof uri.pathToUri !== 'function') return null;
    const state = pane.view.state;
    if (!state || !state.doc) return null;
    const document = state.doc;
    const main = state.selection.main;
    const at = Number.isInteger(pos) ? clamp(pos, 0, document.length) : main.head;
    const position = lspPositionAt(document, at);
    const range = main.empty
      ? { start: samePosition(position), end: samePosition(position) }
      : { start: lspPositionAt(document, main.from), end: lspPositionAt(document, main.to) };
    return {
      uri: uri.pathToUri(pane.filePath),
      position,
      range,
      owner: { windowLabel, paneId: String(pane.paneId) },
    };
  }

  function entryTarget(entry) {
    const uri = uriModule();
    return {
      uri: entry.uri,
      path: uri && typeof uri.uriToPath === 'function' ? uri.uriToPath(entry.uri) : entry.uri,
      range: entry.range,
    };
  }

  function historyState() {
    return { back: backStack.slice(), forward: forwardStack.slice() };
  }

  // --- targets ---------------------------------------------------------------------------

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
    for (const pane of panes.values()) {
      if (!pane || pane.kind !== 'editor' || pane.remote || pane.filePath !== filePath) continue;
      const state = pane.view && pane.view.state;
      const document = state && state.doc;
      if (!document || typeof document.line !== 'function') return null;
      try {
        const wanted = clamp(line + 1, 1, document.lines);
        const text = String(document.line(wanted).text || '').trim();
        return text ? text.slice(0, PREVIEW_LIMIT) : null;
      } catch (_) {
        return null;
      }
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

  // --- the chooser field ------------------------------------------------------------------

  function ensureField() {
    if (chooserField) return chooserField;
    const CM = cm();
    if (
      !CM || !CM.StateField || typeof CM.StateField.define !== 'function'
      || !CM.StateEffect || typeof CM.StateEffect.define !== 'function'
      || !CM.showTooltip || typeof CM.showTooltip.from !== 'function'
    ) return null;
    chooserEffect = CM.StateEffect.define();
    chooserField = CM.StateField.define({
      create: () => null,
      update(value, tr) {
        let replaced = false;
        for (const effect of tr.effects) {
          if (effect.is(chooserEffect)) {
            value = effect.value;
            replaced = true;
          }
        }
        if (!value) return null;
        // An edit invalidates the origin the chooser was opened from — the
        // symbol under the caret has moved or changed — so the list goes with
        // it rather than sending the user somewhere on stale evidence.
        if (!replaced && tr.docChanged) return null;
        return value;
      },
      provide: (field) => CM.showTooltip.from(field, (value) => (value ? {
        pos: value.anchor,
        above: false,
        create: (view) => ({ dom: renderChooser(value, view) }),
      } : null)),
    });
    return chooserField;
  }

  function chooserOf(view) {
    if (!chooserField || !view || !view.state || typeof view.state.field !== 'function') return null;
    return view.state.field(chooserField, false) || null;
  }

  function setChooser(view, value) {
    if (!chooserField || !view || typeof view.dispatch !== 'function') return;
    if (!value && !chooserOf(view)) return;
    view.dispatch({ effects: chooserEffect.of(value) });
  }

  function chooserState(view) {
    const value = chooserOf(view);
    if (!value) return { open: false, index: 0, items: [] };
    return { open: true, index: value.index, items: value.items };
  }

  function closeChooser(view) {
    setChooser(view, null);
  }

  // Asked by lsp-tooltips before it opens anything of its own, the same way it
  // asks CodeMirror whether the completion popup is up. The one-overlay rule
  // has to hold in both directions: opening the chooser dismisses hover and
  // signature help, and while it is open they stand down.
  function chooserOpen(view) {
    return chooserOf(view) !== null;
  }

  // --- rendering --------------------------------------------------------------------------
  //
  // Paths and previews are file contents and file names — never markup. Every
  // string goes in through textContent, the same rule the other LSP surfaces
  // follow.

  function part(className, text) {
    const node = doc().createElement('span');
    node.className = className;
    node.textContent = String(text);
    return node;
  }

  function renderChooser(value, view) {
    const root = doc().createElement('div');
    root.className = 'tl-definition-chooser';
    root.setAttribute('role', 'listbox');
    root.setAttribute('aria-label', 'Definitions');
    const items = (value && value.items) || [];
    const active = Number.isInteger(value && value.index) ? value.index : 0;
    items.forEach((item, index) => {
      const row = doc().createElement('div');
      const selected = index === active;
      row.className = selected
        ? 'tl-definition-chooser__item tl-definition-chooser__item--active'
        : 'tl-definition-chooser__item';
      row.setAttribute('role', 'option');
      row.setAttribute('aria-selected', selected ? 'true' : 'false');
      row.appendChild(part('tl-definition-chooser__where', `${item.name}:${item.line}`));
      row.appendChild(part(
        'tl-definition-chooser__preview',
        item.preview === null || item.preview === undefined ? item.context : item.preview,
      ));
      if (typeof row.addEventListener === 'function') {
        row.addEventListener('mousedown', (event) => {
          if (event && Number.isInteger(event.button) && event.button !== 0) return;
          // The editor keeps the keyboard: a chooser row is a target list, not
          // a focusable surface of its own.
          if (event && typeof event.preventDefault === 'function') event.preventDefault();
          choose(view || null, index);
        });
      }
      root.appendChild(row);
    });
    return root;
  }

  // --- opening and choosing -------------------------------------------------------------

  function openChooser(view, targets, pos, origin) {
    if (!ensureField()) return false;
    // One overlay at a time: a hover or signature tooltip is about the symbol
    // the user is leaving, and two boxes at the same anchor is nobody's design.
    const tooltips = global.termlabLspTooltips;
    if (tooltips && typeof tooltips.dismiss === 'function') tooltips.dismiss(view);
    setChooser(view, { items: targets, index: 0, anchor: pos, origin });
    return true;
  }

  function moveChooser(view, delta) {
    const value = chooserOf(view);
    if (!value || !value.items.length) return false;
    const count = value.items.length;
    const index = ((value.index + delta) % count + count) % count;
    setChooser(view, { ...value, index });
    return true;
  }

  async function choose(view, index) {
    const value = chooserOf(view);
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
    if (!origin) return;
    pushBounded(backStack, origin);
    // A new branch discards the one the user abandoned, exactly as a browser
    // does: Forward from here would lead somewhere they chose to leave.
    forwardStack.length = 0;
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
      response = await requestFeature(pane, lspPositionAt(target.state.doc, at));
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
  async function step(from, to) {
    if (!from.length) return 'none';
    const entry = from[from.length - 1];
    const here = captureLocation(currentPane(), null);
    const outcome = await jumpTo(entryTarget(entry));
    if (outcome !== 'navigated' && outcome !== 'elsewhere') return outcome;
    from.pop();
    if (here) pushBounded(to, here);
    return outcome;
  }

  function navigateBack() {
    return step(backStack, forwardStack);
  }

  function navigateForward() {
    return step(forwardStack, backStack);
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
    if (!chooserOf(view)) return false;
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
    const field = ensureField();
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

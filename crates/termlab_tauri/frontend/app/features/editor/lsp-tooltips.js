// Hover and signature help: ONE overlay controller per editor view.
//
// Both surfaces share a single state machine, because they share a single
// place on screen:
//
//   closed | pending(kind, request, version) | visible(kind, anchor, payload)
//
// `pending` is bookkeeping this module owns (a per-view sequence number and
// the document version the request was made at); `visible` lives in a
// CodeMirror StateField, so the tooltip is placed and mapped by CodeMirror
// rather than by absolute coordinates this module would have to recompute on
// every scroll. Only one overlay is ever visible: starting a request of the
// other kind closes what is on screen.
//
// Server text is NEVER injected as markup: Markdown is normalized to plain
// text and fenced-code segments by lsp-markdown.js and inserted with
// textContent — the same rule lsp-completion.js and lsp-diagnostics.js follow,
// for the same reason.
//
// It reaches the backend only through editor-service's requestFeature, which
// owns the flush/barrier/staleness protocol, and from there through
// lsp-bridge. It reaches Tauri through no seam of its own, and its one
// keyboard hook is a CodeMirror domEventHandler on the editor itself, so a
// focused terminal pane is untouched.
(function initTermLabLspTooltips(global) {
  'use strict';

  const DEFAULT_HOVER_DELAY_MS = 320;

  let paneForViewHook = null;
  let currentPaneHook = null;
  let requestFeatureHook = null;
  let hoverDelayMs = DEFAULT_HOVER_DELAY_MS;
  let windowHandlers = null;

  // view -> { sequence, pending, timer }
  const entries = new WeakMap();

  // Built once, on first use: two mounts of extensions() must install the same
  // field, not two competing ones.
  let overlayEffect = null;
  let overlayField = null;
  let mounted = null;

  function cm() {
    return global.CM6 || null;
  }

  function doc() {
    return global.document;
  }

  // --- pane and document lookup ---------------------------------------------

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

  // A pane that may be asked for `feature`: a local editor with a committed
  // document whose session advertises it. Remote panes are excluded here as
  // well as in Rust — a remote buffer never enters the registry.
  function documentStateFor(pane, feature) {
    if (!pane || pane.kind !== 'editor' || !pane.view || pane.remote) return null;
    const store = global.termlabLspState;
    if (!store || typeof store.get !== 'function') return null;
    const state = store.get(pane);
    if (!state || !state.documentId) return null;
    const capabilities = state.capabilities || (state.status && state.status.capabilities) || {};
    if (capabilities[feature] !== true) return null;
    return state;
  }

  function isReady(state) {
    return !!(state && state.status && state.status.state === 'ready');
  }

  // --- trigger characters ----------------------------------------------------
  //
  // Rust normalizes these (curated list merged with what the server
  // advertised, per adapter policy) and puts them on the status payload. An
  // older backend that sends neither leaves both lists empty, which correctly
  // means "no automatic signature help", not a crash.

  function stringList(value) {
    return Array.isArray(value) ? value.filter((entry) => typeof entry === 'string' && entry) : [];
  }

  function signatureTriggersFor(state) {
    const status = (state && state.status) || {};
    return {
      trigger: stringList(status.signatureHelpTriggerCharacters),
      retrigger: stringList(status.signatureHelpRetriggerCharacters),
    };
  }

  // What a just-typed character means for signature help. Automatic requests
  // happen only while the session is ready; a retrigger character re-asks only
  // when an overlay is already open, which is what keeps `,` from opening a
  // tooltip in the middle of ordinary prose.
  function signatureActionFor(state, character, open) {
    if (!state || !character) return null;
    if (!isReady(state)) return null;
    const triggers = signatureTriggersFor(state);
    if (triggers.trigger.indexOf(character) >= 0) return 'trigger';
    if (open && triggers.retrigger.indexOf(character) >= 0) return 'retrigger';
    return null;
  }

  // --- position conversion ---------------------------------------------------
  //
  // LSP positions and CodeMirror offsets both count UTF-16 code units, so the
  // conversion is arithmetic — and the clamping around it (a server that has
  // raced ahead can name a line past the end, where CodeMirror throws rather
  // than saturating) lives in lsp-position.js so that every surface places a
  // server range the same way.

  function lspPositionAt(document, offset) {
    const helper = global.termlabLspPosition;
    return helper ? helper.positionAt(document, offset) : { line: 0, character: 0 };
  }

  function anchorFor(view, range, fallback) {
    const helper = global.termlabLspPosition;
    if (!range || !range.start || !range.end || !helper) {
      return { from: fallback, to: fallback };
    }
    return helper.spanOf(view.state.doc, range);
  }

  // --- Markdown --------------------------------------------------------------
  //
  // Normalizing server Markdown to safe segments, and rendering those segments
  // as text nodes, is lsp-markdown.js's job. It is pure and has nothing to do
  // with the overlay state machine this file exists for.

  function markdown() {
    return global.termlabLspMarkdown || null;
  }

  function markdownSegments(blocks) {
    const module = markdown();
    return module ? module.markdownSegments(blocks) : [];
  }

  function appendSegments(root, segments, textClass, codeClass) {
    const module = markdown();
    if (module) module.appendSegments(root, segments, textClass, codeClass);
  }

  function block(className, text) {
    const module = markdown();
    return module ? module.block(className, text) : doc().createElement('div');
  }

  // --- payload normalization --------------------------------------------------

  function normalizeHover(response) {
    const segments = markdownSegments(response && response.blocks);
    return segments.length ? { segments } : null;
  }

  function boundedIndex(value, length) {
    return Number.isInteger(value) && value >= 0 && value < length ? value : 0;
  }

  function normalizeSignature(response) {
    const raw = (response && Array.isArray(response.signatures)) ? response.signatures : [];
    const signatures = raw
      .filter((signature) => signature && typeof signature.label === 'string' && signature.label)
      .map((signature) => ({
        label: String(signature.label),
        documentation: markdownSegments(signature.documentation),
        parameters: (Array.isArray(signature.parameters) ? signature.parameters : [])
          .filter(Boolean)
          .map((parameter) => ({
            label: String(parameter.label || ''),
            start: Number.isInteger(parameter.labelStartUtf16) ? parameter.labelStartUtf16 : null,
            end: Number.isInteger(parameter.labelEndUtf16) ? parameter.labelEndUtf16 : null,
            documentation: markdownSegments(parameter.documentation),
          })),
        activeParameter: Number.isInteger(signature.activeParameter)
          ? signature.activeParameter
          : null,
      }));
    if (!signatures.length) return null;
    return {
      signatures,
      activeSignature: boundedIndex(response.activeSignature, signatures.length),
      activeParameter: Number.isInteger(response.activeParameter) ? response.activeParameter : 0,
    };
  }

  // --- rendering ---------------------------------------------------------------

  function renderHover(payload) {
    const root = doc().createElement('div');
    root.className = 'tl-hover';
    appendSegments(root, payload && payload.segments, 'tl-hover__text', 'tl-hover__code');
    return root;
  }

  // Where the active parameter sits inside the signature label. The server's
  // own offsets win; a label-substring search is the documented fallback. A
  // parameter the label does not contain is left unhighlighted rather than
  // guessed — highlighting the wrong span is worse than highlighting none.
  function parameterRange(signature, index) {
    const parameter = signature.parameters[index];
    if (!parameter) return null;
    const length = signature.label.length;
    if (
      Number.isInteger(parameter.start) && Number.isInteger(parameter.end)
      && parameter.start >= 0 && parameter.end > parameter.start && parameter.end <= length
    ) {
      return { from: parameter.start, to: parameter.end };
    }
    if (!parameter.label) return null;
    const at = signature.label.indexOf(parameter.label);
    if (at < 0) return null;
    return { from: at, to: at + parameter.label.length };
  }

  function labelPart(className, text) {
    const node = doc().createElement('span');
    node.className = className;
    node.textContent = text;
    return node;
  }

  function renderSignature(payload) {
    const root = doc().createElement('div');
    root.className = 'tl-signature';
    const signatures = (payload && payload.signatures) || [];
    const index = boundedIndex(payload && payload.activeSignature, signatures.length);
    const signature = signatures[index];
    if (!signature) return root;
    if (signatures.length > 1) {
      root.appendChild(block('tl-signature__count', `${index + 1} of ${signatures.length}`));
    }
    // A per-signature activeParameter wins over the response-level one: that
    // is the order the protocol specifies, and overload sets rely on it.
    const active = Number.isInteger(signature.activeParameter)
      ? signature.activeParameter
      : payload.activeParameter;
    const label = doc().createElement('div');
    label.className = 'tl-signature__label';
    const range = parameterRange(signature, active);
    if (range) {
      label.appendChild(labelPart('tl-signature__param', signature.label.slice(0, range.from)));
      label.appendChild(labelPart(
        'tl-signature__param--active', signature.label.slice(range.from, range.to),
      ));
      label.appendChild(labelPart('tl-signature__param', signature.label.slice(range.to)));
    } else {
      label.textContent = signature.label;
    }
    root.appendChild(label);
    const parameter = signature.parameters[active];
    if (parameter) {
      appendSegments(root, parameter.documentation, 'tl-signature__doc', 'tl-hover__code');
    }
    appendSegments(root, signature.documentation, 'tl-signature__doc', 'tl-hover__code');
    return root;
  }

  // --- the CodeMirror field -----------------------------------------------------

  // True when the transaction deleted text spanning the overlay's anchor — the
  // call the hints describe was removed, rather than edited. An insertion
  // (`toA === fromA`) is never this, and neither is a backspace that stops AT
  // the anchor: only a deletion that strictly crosses it counts, so editing
  // arguments keeps the hints and deleting the call closes them.
  function deletedAcross(changes, anchor) {
    // Strict containment on BOTH sides, and the same test whatever the anchor's
    // width. The anchor does not stay a caret: the carry-forward below maps its
    // ends with opposite associativity, so every character typed at the caret
    // widens it over what has been typed since the overlay opened. An overlap
    // test would therefore call an ordinary backspace "the call was deleted" —
    // the most common in-call gesture there is — so the deletion has to swallow
    // the anchor whole before this counts.
    //
    // The cost is a deletion that begins exactly AT the anchor (select from the
    // caret to the end of the line and delete): the overlay survives it, until
    // Escape, blur or scroll. That is the better trade — the alternative closes
    // on a plain forward-delete inside the call.
    let removed = false;
    changes.iterChanges((fromA, toA) => {
      if (toA <= fromA) return;
      removed = removed || (fromA < anchor.from && toA > anchor.to);
    });
    return removed;
  }

  function ensureField() {
    if (overlayField) return overlayField;
    const CM = cm();
    if (
      !CM || !CM.StateField || typeof CM.StateField.define !== 'function'
      || !CM.StateEffect || typeof CM.StateEffect.define !== 'function'
      || !CM.showTooltip || typeof CM.showTooltip.from !== 'function'
    ) return null;
    overlayEffect = CM.StateEffect.define();
    overlayField = CM.StateField.define({
      create: () => null,
      update(value, tr) {
        let replaced = false;
        for (const effect of tr.effects) {
          if (effect.is(overlayEffect)) {
            value = effect.value;
            replaced = true;
          }
        }
        if (!value) return null;
        if (!replaced && tr.docChanged) {
          // An edit invalidates a hover outright: the text it described has
          // moved or changed. Signature help is the opposite — typing inside
          // a call is exactly when it should stay — so it is carried forward
          // with its anchor mapped through the change.
          if (value.kind !== 'signatureHelp') return null;
          // Unless the edit REMOVED the call. A mapped anchor survives a
          // deletion that swallowed it (it collapses onto the deletion point),
          // which used to leave the hints for a call that no longer exists,
          // up until Escape, blur or scroll.
          if (deletedAcross(tr.changes, value.anchor)) return null;
          value = {
            ...value,
            anchor: {
              from: tr.changes.mapPos(value.anchor.from, -1),
              to: tr.changes.mapPos(value.anchor.to, 1),
            },
          };
        }
        if (!replaced && tr.selection && value.kind === 'hover') return null;
        return value;
      },
      provide: (field) => CM.showTooltip.from(field, (value) => (value ? {
        pos: value.anchor.from,
        end: value.anchor.to,
        above: true,
        create: () => ({
          dom: value.kind === 'hover'
            ? renderHover(value.payload)
            : renderSignature(value.payload),
        }),
      } : null)),
    });
    return overlayField;
  }

  function visibleOf(view) {
    if (!overlayField || !view || !view.state || typeof view.state.field !== 'function') return null;
    return view.state.field(overlayField, false) || null;
  }

  function setVisible(view, value) {
    if (!overlayField || !view || typeof view.dispatch !== 'function') return;
    if (!value && !visibleOf(view)) return;
    view.dispatch({ effects: overlayEffect.of(value) });
  }

  // --- per-view bookkeeping -------------------------------------------------------

  function entryFor(view) {
    let entry = entries.get(view);
    if (!entry) {
      entry = { sequence: 0, pending: null, timer: null };
      entries.set(view, entry);
    }
    return entry;
  }

  function clearTimer(entry) {
    if (entry.timer !== null) {
      global.clearTimeout(entry.timer);
      entry.timer = null;
    }
  }

  // Bumping the sequence is the cancellation: a response that arrives after it
  // no longer matches the pending request and is dropped on the floor.
  function cancelPending(view) {
    const entry = entryFor(view);
    clearTimer(entry);
    if (entry.pending) {
      entry.sequence += 1;
      entry.pending = null;
    }
  }

  function isOpen(view) {
    return entryFor(view).pending !== null || visibleOf(view) !== null;
  }

  function dismiss(view) {
    if (!view) return;
    cancelPending(view);
    setVisible(view, null);
  }

  function stateOf(view) {
    if (!view) return { phase: 'closed' };
    const entry = entryFor(view);
    const visible = visibleOf(view);
    const snapshot = {
      phase: entry.pending ? 'pending' : (visible ? 'visible' : 'closed'),
      kind: entry.pending ? entry.pending.kind : (visible ? visible.kind : null),
      request: entry.pending ? entry.pending.request : null,
      version: entry.pending ? entry.pending.version : null,
      anchor: visible ? visible.anchor : null,
      payload: visible ? visible.payload : null,
    };
    return snapshot;
  }

  // --- requests --------------------------------------------------------------------

  function completionOpen(view) {
    const CM = cm();
    if (!CM || typeof CM.completionStatus !== 'function' || !view || !view.state) return false;
    try {
      return CM.completionStatus(view.state) !== null;
    } catch (_) {
      return false;
    }
  }

  // Go to Definition's chooser is the third surface that wants this anchor,
  // and unlike the other two it is keyboard-modal while open: Enter and the
  // arrows belong to it. So neither an automatic dwell nor an explicit Show
  // Hover may render on top of it — Escape closes the chooser, and hover is
  // available again immediately after. Asked of the owning module, exactly the
  // way completionOpen asks CodeMirror.
  function chooserOpen(view) {
    const navigation = global.termlabLspNavigation;
    if (!view || !navigation || typeof navigation.chooserOpen !== 'function') return false;
    try {
      return navigation.chooserOpen(view) === true;
    } catch (_) {
      return false;
    }
  }

  function requestFeature(pane, kind, position, trigger) {
    if (typeof requestFeatureHook === 'function') {
      return Promise.resolve(requestFeatureHook(pane, kind, position, trigger));
    }
    const service = global.termlabEditorService;
    if (!service || typeof service.requestFeature !== 'function') return Promise.resolve(null);
    return Promise.resolve(service.requestFeature(pane, kind, position, trigger));
  }

  // The one request path. `manual` marks an explicit invocation, which skips
  // the automatic gates (session readiness and the completion collision) but
  // never the capability check.
  //
  // There is deliberately NO version comparison here. requestFeature's barrier
  // captures the version AFTER flushing the pending change batch and rejects
  // on its own admission sequence, attachment generation, document id and
  // version; a second check outside it, against a version read BEFORE the
  // flush, only ever discards good results.
  async function requestOverlay(view, kind, pos, options) {
    const opts = options || {};
    if (!view || !view.state) return false;
    const field = ensureField();
    if (!field) return false;
    const pane = paneForView(view);
    const feature = kind === 'hover' ? 'hover' : 'signatureHelp';
    const state = documentStateFor(pane, feature);
    if (!state) {
      dismiss(view);
      return false;
    }
    // The definition chooser wins over both kinds, manual invocation included:
    // it is on screen at the same anchor and owns the keyboard until Escape.
    if (chooserOpen(view)) return false;
    if (!opts.manual) {
      // Completion wins the collision; signature help is available again the
      // moment its popup closes.
      if (completionOpen(view)) return false;
      if (!isReady(state)) return false;
    }

    const entry = entryFor(view);
    clearTimer(entry);
    entry.sequence += 1;
    const request = entry.sequence;
    const documentId = state.documentId;
    entry.pending = { kind, request, version: state.version };
    // Only one overlay may be on screen. A request of the OTHER kind closes
    // what is showing immediately; a retrigger of the same kind leaves it up
    // so the tooltip does not flicker on every keystroke inside a call.
    const visible = visibleOf(view);
    if (visible && visible.kind !== kind) setVisible(view, null);

    let response = null;
    try {
      response = await requestFeature(
        pane, kind, lspPositionAt(view.state.doc, pos), opts.trigger || null,
      );
    } catch (_) {
      if (entry.pending && entry.pending.request === request) entry.pending = null;
      return false;
    }
    if (!entry.pending || entry.pending.request !== request) return false;
    entry.pending = null;

    const current = documentStateFor(pane, feature);
    if (!current || current.documentId !== documentId) {
      setVisible(view, null);
      return false;
    }
    if (!response || response.documentId !== documentId) {
      setVisible(view, null);
      return false;
    }
    const payload = kind === 'hover' ? normalizeHover(response) : normalizeSignature(response);
    if (!payload) {
      setVisible(view, null);
      return false;
    }
    const anchor = kind === 'hover'
      ? anchorFor(view, response.range, pos)
      : { from: pos, to: pos };
    setVisible(view, { kind, anchor, payload });
    return true;
  }

  function caretOf(view) {
    return view && view.state ? view.state.selection.main.head : 0;
  }

  function viewOrCurrent(view) {
    if (view) return view;
    const pane = currentPane();
    return pane && pane.kind === 'editor' ? pane.view : null;
  }

  function showHover(view, pos) {
    const target = viewOrCurrent(view);
    if (!target) return Promise.resolve(false);
    const at = Number.isInteger(pos) ? pos : caretOf(target);
    return requestOverlay(target, 'hover', at, { manual: true });
  }

  function showSignatureHelp(view, pos) {
    const target = viewOrCurrent(view);
    if (!target) return Promise.resolve(false);
    const at = Number.isInteger(pos) ? pos : caretOf(target);
    return requestOverlay(target, 'signatureHelp', at, { manual: true });
  }

  // --- events ---------------------------------------------------------------------

  function handleKeydown(event, view) {
    if (!event || event.metaKey || event.ctrlKey || event.altKey || event.shiftKey) return false;
    if (event.key !== 'Escape' && event.key !== 'Esc') return false;
    if (!isOpen(view)) return false;
    dismiss(view);
    return true;
  }

  function handleBlur(view) {
    dismiss(view);
  }

  function handleScroll(view) {
    dismiss(view);
  }

  function handlePointerLeave(view) {
    dismiss(view);
  }

  function handlePointerMove(event, view) {
    if (!view || !event) return;
    const entry = entryFor(view);
    const pos = typeof view.posAtCoords === 'function'
      ? view.posAtCoords({ x: event.clientX, y: event.clientY })
      : null;
    if (pos === null || pos === undefined) {
      clearTimer(entry);
      return;
    }
    const visible = visibleOf(view);
    // Signature help outranks the automatic dwell. The pointer wandering over
    // the code while the user types arguments is not a request for anything,
    // and letting the dwell fire would swap the parameter hints for a hover
    // through the one-overlay rule. An explicit Show Hover still wins — that is
    // a gesture, not a side effect of where the mouse happens to rest.
    if (visible && visible.kind === 'signatureHelp') {
      clearTimer(entry);
      return;
    }
    // Still over the text the tooltip describes: leave it alone and do not
    // restart the dwell. Anywhere else invalidates it.
    if (visible && visible.kind === 'hover') {
      if (pos >= visible.anchor.from && pos <= visible.anchor.to) return;
      dismiss(view);
    }
    clearTimer(entry);
    entry.timer = global.setTimeout(() => {
      entry.timer = null;
      requestOverlay(view, 'hover', pos, {});
    }, hoverDelayMs);
  }

  // Called from the update listener. An edit cancels any in-flight request
  // (its answer describes text that no longer exists) and, when the edit came
  // from typing, may open or refresh signature help. Automatic triggers fire
  // only on a user INPUT transaction: with vim enabled that is exactly insert
  // mode, and a programmatic document mutation never counts as editing.
  function noteDocumentChange(view, update) {
    if (!view || !update || !update.docChanged) return;
    const openBefore = isOpen(view);
    cancelPending(view);
    const transactions = update.transactions || [];
    const typed = transactions.some(
      (tr) => tr && typeof tr.isUserEvent === 'function' && tr.isUserEvent('input'),
    );
    if (!typed) return;
    const state = update.state;
    if (!state) return;
    const pane = paneForView(view);
    const documentState = documentStateFor(pane, 'signatureHelp');
    if (!documentState) return;
    const pos = state.selection.main.head;
    const character = state.doc.sliceString(Math.max(0, pos - 1), pos);
    if (!signatureActionFor(documentState, character, openBefore)) return;
    requestOverlay(view, 'signatureHelp', pos, { trigger: character });
  }

  // --- extensions ---------------------------------------------------------------------
  //
  // The keydown hook has to beat @replit/codemirror-vim, and a keymap cannot:
  // every keymap in a state is served by ONE shared DOM handler that
  // @codemirror/view registers at Prec.default, while vim is a ViewPlugin with
  // its own keydown handler that runs ahead of it. So this is a
  // domEventHandler of our own raised with Prec.highest, and it returns false
  // — no preventDefault — whenever no overlay is open, which is what keeps
  // Escape leaving insert mode exactly as before.

  function extensions() {
    const CM = cm();
    const field = ensureField();
    if (!CM || !field) return [];
    if (mounted) return mounted;
    const list = [field];
    if (CM.EditorView && typeof CM.EditorView.updateListener === 'object') {
      list.push(CM.EditorView.updateListener.of((update) => {
        try {
          noteDocumentChange(update.view, update);
        } catch (error) {
          console.error('LSP tooltips could not handle a document change', error);
        }
      }));
    }
    if (
      CM.Prec && typeof CM.Prec.highest === 'function'
      && CM.EditorView && typeof CM.EditorView.domEventHandlers === 'function'
    ) {
      list.push(CM.Prec.highest(CM.EditorView.domEventHandlers({
        keydown: (event, view) => handleKeydown(event, view),
        mousemove: (event, view) => { handlePointerMove(event, view); return false; },
        mouseleave: (event, view) => { handlePointerLeave(view); return false; },
        blur: (event, view) => { handleBlur(view); return false; },
        scroll: (event, view) => { handleScroll(view); return false; },
      })));
    }
    mounted = list;
    return mounted;
  }

  // --- lifecycle -------------------------------------------------------------------------

  // The configured editor_signature_help shortcut and the palette's Show Hover
  // action arrive as window events (shortcut-runtime dispatches
  // `termlab:editor-signature-help`; the palette dispatches
  // `termlab:editor-show-hover`). These are application events, not key
  // handlers: the router already decided the pane scope.
  function installWindowHandlers() {
    if (windowHandlers || typeof global.addEventListener !== 'function') return;
    windowHandlers = {
      'termlab:editor-show-hover': () => { showHover(); },
      'termlab:editor-signature-help': () => { showSignatureHelp(); },
    };
    for (const name of Object.keys(windowHandlers)) {
      global.addEventListener(name, windowHandlers[name]);
    }
  }

  function configure(options) {
    const opts = options || {};
    if (typeof opts.paneForView === 'function') paneForViewHook = opts.paneForView;
    if (typeof opts.currentPane === 'function') currentPaneHook = opts.currentPane;
    if (typeof opts.requestFeature === 'function') requestFeatureHook = opts.requestFeature;
    if (Number.isFinite(opts.hoverDelayMs) && opts.hoverDelayMs >= 0) {
      hoverDelayMs = opts.hoverDelayMs;
    }
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

  global.termlabLspTooltips = {
    configure,
    dispose,
    extensions,
    stateOf,
    dismiss,
    showHover,
    showSignatureHelp,
    handleKeydown,
    handleBlur,
    handleScroll,
    handlePointerMove,
    handlePointerLeave,
    noteDocumentChange,
    signatureTriggersFor,
    signatureActionFor,
    markdownSegments,
    renderHover,
    renderSignature,
  };
})(window);

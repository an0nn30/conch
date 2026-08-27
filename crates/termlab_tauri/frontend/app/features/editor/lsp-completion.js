// LSP-backed completion for CodeMirror editor panes.
//
// Two jobs; the third — turning an item into a transaction — lives next door
// in lsp-completion-apply.js.
//
//   1. an async completion source that asks the language service for the
//      caret's position and drops anything that comes back for a document
//      that has since changed identity;
//   2. keyboard ownership of the popup that outranks vim while it is open and
//      hands every key straight back when it is not.
//
// It reaches the backend only through editor-service's requestFeature, which
// owns the flush/barrier/staleness protocol, and from there through
// lsp-bridge. It never calls invoke(), and it registers no document- or
// window-level KEY handlers: the one keyboard hook is a CodeMirror
// domEventHandler on the editor itself, so a focused terminal pane is
// untouched.
(function initTermLabLspCompletion(global) {
  'use strict';

  // What a completion item's `kind` means to CodeMirror's own icon set.
  // Unknown and absent both fall to 'text' rather than to no icon at all.
  const KIND_TYPES = {
    text: 'text',
    method: 'method',
    function: 'function',
    constructor: 'class',
    field: 'property',
    variable: 'variable',
    class: 'class',
    interface: 'interface',
    module: 'namespace',
    property: 'property',
    unit: 'constant',
    value: 'constant',
    enum: 'enum',
    keyword: 'keyword',
    snippet: 'text',
    color: 'constant',
    file: 'text',
    reference: 'variable',
    folder: 'text',
    enumMember: 'enum',
    constant: 'constant',
    struct: 'class',
    event: 'variable',
    operator: 'keyword',
    typeParameter: 'type',
  };

  const IDENTIFIER_CHARACTER = /[A-Za-z0-9_$]/;
  const WORD_BEFORE_CARET = /[A-Za-z0-9_$]*$/;

  let paneForViewHook = null;
  let currentPaneHook = null;
  let requestCompletionHook = null;
  let suggestionsWhileTyping = true;
  let shortcutHandler = null;

  function cm() {
    return global.CM6 || null;
  }

  function apply() {
    return global.termlabLspCompletionApply || null;
  }

  function record(kind, message) {
    const module = apply();
    if (module && typeof module.record === 'function') module.record(kind, message);
  }

  // --- pane and document lookup --------------------------------------------

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

  // A pane that may talk to a language server: a local editor with a committed
  // document whose session says it can complete. Remote panes are excluded
  // here as well as in Rust — a remote buffer never enters the registry, and
  // asking anyway would be a request built on a document nobody owns.
  function documentStateFor(pane) {
    if (!pane || pane.kind !== 'editor' || !pane.view) return null;
    if (pane.remote) return null;
    const store = global.termlabLspState;
    if (!store || typeof store.get !== 'function') return null;
    const state = store.get(pane);
    if (!state || !state.documentId) return null;
    const capabilities = state.capabilities || (state.status && state.status.capabilities) || {};
    if (capabilities.completion !== true) return null;
    return state;
  }

  function completableView(view) {
    return documentStateFor(paneForView(view)) !== null;
  }

  function triggerCharactersFor(state) {
    const status = state && state.status;
    const characters = status && status.completionTriggerCharacters;
    return Array.isArray(characters) ? characters : [];
  }

  // --- position conversion --------------------------------------------------

  function lspPositionAt(doc, offset) {
    const line = doc.lineAt(offset);
    return { line: line.number - 1, character: offset - line.from };
  }

  function textBeforeCaret(context) {
    const line = context.state.doc.lineAt(context.pos);
    return line.text.slice(0, context.pos - line.from);
  }

  function wordStart(context) {
    const before = textBeforeCaret(context);
    const match = WORD_BEFORE_CARET.exec(before);
    return context.pos - (match ? match[0].length : 0);
  }

  // Null means "do not wake the server". Explicit invocation always requests;
  // otherwise a server trigger character requests and names itself (LSP's
  // triggerCharacter), and ordinary identifier typing requests anonymously.
  function triggerFor(context, state) {
    if (context.explicit) return { request: true, character: null };
    if (!suggestionsWhileTyping) return null;
    const before = textBeforeCaret(context).slice(-1);
    if (!before) return null;
    if (triggerCharactersFor(state).indexOf(before) >= 0) {
      return { request: true, character: before };
    }
    if (IDENTIFIER_CHARACTER.test(before)) return { request: true, character: null };
    return null;
  }

  // --- documentation --------------------------------------------------------
  //
  // Server text is inserted as textContent, never as markup. Markdown blocks
  // are shown as their source rather than rendered: this phase has no
  // sanitizing renderer, and showing the source is honest where interpreting
  // it would be dangerous.

  function paragraph(className, text) {
    const node = global.document.createElement('div');
    node.className = className;
    node.textContent = String(text);
    return node;
  }

  function renderInfo(item, resolved) {
    const module = apply();
    const source = resolved || item;
    const node = global.document.createElement('div');
    node.className = 'tl-completion-info';
    if (source.detail) node.appendChild(paragraph('tl-completion-info-detail', source.detail));
    const blocks = Array.isArray(source.documentation) ? source.documentation : [];
    for (const block of blocks) {
      if (!block || !block.value) continue;
      node.appendChild(paragraph('tl-completion-info-doc', block.value));
    }
    if (source.deprecated) {
      node.appendChild(paragraph('tl-completion-info-note', 'Deprecated.'));
    }
    // The unsupported metadata has to be VISIBLE, not just logged: an item
    // whose workspace edit silently does not happen is worse than one that
    // says so.
    for (const effect of (item.unsupportedEffects || [])) {
      const text = module && typeof module.unsupportedEffectText === 'function'
        ? module.unsupportedEffectText(effect)
        : null;
      if (text) node.appendChild(paragraph('tl-completion-info-note', text));
    }
    return node;
  }

  // completionItem/resolve. Resolved once per item and cached; a failure
  // degrades to what the item already carried.
  function makeInfoRenderer(item, documentId) {
    let pending = null;
    let resolved = null;
    return function info() {
      if (!pending) {
        const bridge = global.termlabLspBridge;
        if (!bridge || typeof bridge.resolveCompletionItem !== 'function') {
          return renderInfo(item, null);
        }
        pending = Promise.resolve(bridge.resolveCompletionItem(documentId, item.id))
          .then((value) => { resolved = value || null; })
          .catch((error) => { record('resolve', `completion resolve failed: ${error}`); });
      }
      return pending.then(() => renderInfo(item, resolved));
    };
  }

  // --- option translation ---------------------------------------------------

  // LSP leaves ranking to the client: items are ordered by `sortText`, or by
  // `label` where the server gave none. Rust hands them over in the order the
  // server sent them, which is not the same thing, so the ordering happens
  // here — stably, so the server's own order still breaks ties.
  function orderedItems(items) {
    return items
      .map((entry, index) => ({ entry, index }))
      .sort((left, right) => {
        const a = String(left.entry.sortText || left.entry.label || '');
        const b = String(right.entry.sortText || right.entry.label || '');
        if (a < b) return -1;
        if (a > b) return 1;
        return left.index - right.index;
      })
      .map((wrapped) => wrapped.entry);
  }

  // `requestPos` travels with every option because the item's edit ranges were
  // measured against the document at that caret; see lsp-completion-apply.js.
  function toOption(item, index, requestPos, documentId) {
    const option = {
      // CodeMirror matches on `label`, so filterText — which is what the
      // server said to match on — has to be the label, with the human-facing
      // text moved to displayLabel.
      label: String(item.filterText || item.label || ''),
      type: KIND_TYPES[item.kind] || 'text',
      // Descending, so the server's ranking survives CodeMirror's own sort.
      // Clamped because CodeMirror documents boost as -99..99 and a long list
      // would otherwise run straight out of that range.
      boost: Math.max(-99, 99 - index),
      info: makeInfoRenderer(item, documentId),
      apply: (view, completion, from, to) => {
        const module = apply();
        if (module) module.applyItem(item, view, from, to, requestPos);
      },
    };
    if (item.filterText && item.filterText !== item.label) option.displayLabel = String(item.label);
    if (item.detail) option.detail = String(item.detail);
    if (Array.isArray(item.commitCharacters) && item.commitCharacters.length) {
      option.commitCharacters = item.commitCharacters.slice();
    }
    return option;
  }

  // --- the source -----------------------------------------------------------

  async function completionSource(context) {
    if (!context || !context.state) return null;
    const view = context.view;
    const pane = paneForView(view);
    const state = documentStateFor(pane);
    if (!state) return null;
    const trigger = triggerFor(context, state);
    if (!trigger) return null;

    const documentId = state.documentId;
    const requestPos = context.pos;
    const position = lspPositionAt(context.state.doc, requestPos);
    const from = wordStart(context);

    let response = null;
    try {
      response = await requestCompletion(pane, position, trigger.character);
    } catch (error) {
      record('completion', `completion request failed: ${error}`);
      return null;
    }
    if (!response || !Array.isArray(response.items) || !response.items.length) return null;
    // The version guard belongs to editor-service's barrier, which captures it
    // AFTER flushing the pending change batch and rejects on its own admission
    // sequence, attachment generation, document id and version. Re-checking a
    // version captured BEFORE the flush here only ever discarded good results,
    // because a batched keystroke bumps the version between the two. What is
    // left is the guard the barrier cannot make: that this source's own
    // context and pane still refer to the same document.
    if (context.aborted) return null;
    const current = documentStateFor(pane);
    if (!current || current.documentId !== documentId) return null;
    if (response.documentId !== documentId) return null;

    return {
      from,
      options: orderedItems(response.items)
        .map((entry, index) => toOption(entry, index, requestPos, documentId)),
      // An incomplete list has to be re-requested as the prefix grows.
      validFor: response.isIncomplete ? undefined : /^[A-Za-z0-9_$]*$/,
    };
  }

  function requestCompletion(pane, position, character) {
    if (typeof requestCompletionHook === 'function') {
      return Promise.resolve(requestCompletionHook(pane, position, character));
    }
    const service = global.termlabEditorService;
    if (!service || typeof service.requestFeature !== 'function') return Promise.resolve(null);
    return Promise.resolve(service.requestFeature(pane, 'completion', position, character));
  }

  // --- keyboard -------------------------------------------------------------
  //
  // This has to beat @replit/codemirror-vim, and a keymap cannot.
  //
  // Every CodeMirror keymap in a state — whatever its precedence — is served
  // by ONE shared DOM handler that @codemirror/view registers at Prec.default
  // (`handleKeyEvents`); precedence inside the keymap facet only orders
  // bindings against each other, behind that single handler. vim is not a
  // keymap at all: it is a ViewPlugin with an `eventHandlers.keydown` that
  // preventDefaults whatever it consumes, and the view runs plugin handlers in
  // plugin order, stopping at the first one that returns true or has already
  // called preventDefault. A vim plugin at default precedence therefore beats
  // `Prec.highest(keymap.of(...))` every time, and Escape would leave insert
  // mode instead of closing the popup.
  //
  // So the hook is a domEventHandler of our own, raised with Prec.highest,
  // which lands ahead of vim's plugin. It returns false — no preventDefault —
  // for every key it does not own and whenever the popup is closed, so vim and
  // the default keymap see those keys exactly as before.

  function popupOpen(view) {
    const CM = cm();
    if (!CM || typeof CM.completionStatus !== 'function' || !view || !view.state) return false;
    try {
      return CM.completionStatus(view.state) !== null;
    } catch (_) {
      return false;
    }
  }

  function whenOpen(command) {
    return (view) => {
      if (!popupOpen(view) || typeof command !== 'function') return false;
      return command(view) !== false;
    };
  }

  function keyBindings() {
    const CM = cm();
    if (!CM) return [];
    // moveCompletionSelection is a Command FACTORY: called once here, not once
    // per keystroke.
    const move = (forward, by) => (typeof CM.moveCompletionSelection === 'function'
      ? CM.moveCompletionSelection(forward, by)
      : null);
    return [
      {
        key: 'Ctrl-Space',
        // Guarded like the rest: on a remote or plain-text pane there is no
        // source to open, and swallowing the key for nothing is worse than
        // letting it through.
        run: (view) => (completableView(view) && typeof CM.startCompletion === 'function'
          ? CM.startCompletion(view) !== false
          : false),
      },
      { key: 'Escape', run: whenOpen(CM.closeCompletion) },
      { key: 'Enter', run: whenOpen(CM.acceptCompletion) },
      { key: 'Tab', run: whenOpen(CM.acceptCompletion) },
      // With autocompletion's own keymap off, these are the only thing
      // navigating the list — and while the popup is closed they are still
      // vim's and the default keymap's arrows.
      { key: 'ArrowDown', run: whenOpen(move(true)) },
      { key: 'ArrowUp', run: whenOpen(move(false)) },
      { key: 'PageDown', run: whenOpen(move(true, 'page')) },
      { key: 'PageUp', run: whenOpen(move(false, 'page')) },
    ];
  }

  // KeyboardEvent -> the name of the binding it could match, or null. Anything
  // carrying Meta or Alt is somebody else's (and Ctrl is ours only for the
  // manual trigger), so vim's own Ctrl-/Alt- bindings are never intercepted.
  function eventKey(event) {
    if (!event || event.altKey || event.metaKey) return null;
    const key = event.key;
    if (event.ctrlKey) return key === ' ' || key === 'Spacebar' ? 'Ctrl-Space' : null;
    if (key === 'Escape' || key === 'Esc') return 'Escape';
    if (key === 'Enter') return 'Enter';
    if (key === 'Tab') return event.shiftKey ? null : 'Tab';
    if (key === 'ArrowDown' || key === 'ArrowUp' || key === 'PageDown' || key === 'PageUp') {
      return key;
    }
    return null;
  }

  function handleKeydown(event, view) {
    const key = eventKey(event);
    if (!key) return false;
    const binding = keyBindings().find((candidate) => candidate.key === key);
    return binding ? binding.run(view) === true : false;
  }

  // What editor-pane mounts.
  function extensions() {
    const CM = cm();
    if (!CM || typeof CM.autocompletion !== 'function') return [];
    const list = [
      CM.autocompletion({
        override: [completionSource],
        activateOnTyping: true,
        // One handler owns these keys. Two competing ones is exactly how
        // Escape starts needing two presses in insert mode.
        defaultKeymap: false,
        closeOnBlur: true,
        maxRenderedOptions: 100,
      }),
    ];
    if (
      CM.Prec && typeof CM.Prec.highest === 'function'
      && CM.EditorView && typeof CM.EditorView.domEventHandlers === 'function'
    ) {
      list.push(CM.Prec.highest(CM.EditorView.domEventHandlers({
        keydown: (event, view) => handleKeydown(event, view),
      })));
    }
    return list;
  }

  // The configured editor_completion shortcut (shortcut-runtime dispatches
  // `termlab:editor-completion` on the window). This is an application event,
  // not a key handler: the router already decided the pane scope.
  function triggerManualCompletion() {
    const CM = cm();
    const pane = currentPane();
    if (!CM || typeof CM.startCompletion !== 'function') return false;
    if (!pane || pane.kind !== 'editor' || !pane.view) return false;
    if (!documentStateFor(pane)) return false;
    return CM.startCompletion(pane.view) === true;
  }

  function configure(options) {
    const opts = options || {};
    if (typeof opts.paneForView === 'function') paneForViewHook = opts.paneForView;
    if (typeof opts.currentPane === 'function') currentPaneHook = opts.currentPane;
    if (typeof opts.requestCompletion === 'function') requestCompletionHook = opts.requestCompletion;
    if (typeof opts.suggestionsWhileTyping === 'boolean') {
      suggestionsWhileTyping = opts.suggestionsWhileTyping;
    }
    if (!shortcutHandler && typeof global.addEventListener === 'function') {
      shortcutHandler = () => { triggerManualCompletion(); };
      global.addEventListener('termlab:editor-completion', shortcutHandler);
    }
  }

  function dispose() {
    if (shortcutHandler && typeof global.removeEventListener === 'function') {
      global.removeEventListener('termlab:editor-completion', shortcutHandler);
    }
    shortcutHandler = null;
  }

  global.termlabLspCompletion = {
    configure,
    dispose,
    extensions,
    keyBindings,
    eventKey,
    handleKeydown,
    completionSource,
    triggerManualCompletion,
    setSuggestionsWhileTyping: (enabled) => { suggestionsWhileTyping = enabled !== false; },
    sessionLog: () => {
      const module = apply();
      return module && typeof module.sessionLog === 'function' ? module.sessionLog() : [];
    },
  };
})(window);

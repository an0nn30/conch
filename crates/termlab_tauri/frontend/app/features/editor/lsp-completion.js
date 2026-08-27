// LSP-backed completion for CodeMirror editor panes.
//
// Three jobs and no more:
//
//   1. an async completion source that asks the language service for the
//      caret's position and drops anything that comes back for a document or
//      a version that has since moved on;
//   2. a translation from normalized completion items into CodeMirror options,
//      including an `apply` that lands the primary edit, the snippet and every
//      supported same-document additional edit in ONE transaction — undo,
//      dirty tracking and document synchronisation all have to see one change,
//      not three;
//   3. a dedicated keymap that outranks vim while the popup is open and hands
//      every key straight back when it is not.
//
// It reaches the backend only through editor-service's requestFeature, which
// owns the flush/barrier/staleness protocol, and therefore only through
// lsp-bridge. It never calls invoke() and it registers NO document- or
// window-level KEY handlers: everything keyboard-shaped is a CodeMirror
// binding, so a focused terminal pane and vim normal mode are untouched.
(function initTermLabLspCompletion(global) {
  'use strict';

  // Bounded because it is fed by server data on a hot path: a server that
  // returns an unsupported workspace edit on every keystroke must not grow
  // this without limit.
  const SESSION_LOG_LIMIT = 200;
  const sessionLogEntries = [];

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

  const UNSUPPORTED_EFFECT_TEXT = {
    workspaceEdit: 'This suggestion also edits other files. Those edits were not applied.',
    command: 'This suggestion asks to run a server command, which is not supported yet.',
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

  function record(kind, message) {
    sessionLogEntries.push({ at: Date.now(), kind: String(kind), message: String(message) });
    while (sessionLogEntries.length > SESSION_LOG_LIMIT) sessionLogEntries.shift();
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

  function triggerCharactersFor(state) {
    const status = state && state.status;
    const characters = status && status.completionTriggerCharacters;
    return Array.isArray(characters) ? characters : [];
  }

  // --- position conversion --------------------------------------------------
  //
  // CodeMirror columns are already UTF-16 code units, which is what LSP
  // positions count, so both directions are pure arithmetic. Rust owns the
  // conversion for everything it sends; this owns only the caret.

  function lspPositionAt(doc, offset) {
    const line = doc.lineAt(offset);
    return { line: line.number - 1, character: offset - line.from };
  }

  function offsetAt(doc, position) {
    if (!position) return null;
    const lineNumber = Math.min(Math.max(Number(position.line) + 1, 1), doc.lines);
    const line = doc.line(lineNumber);
    const character = Math.max(Number(position.character) || 0, 0);
    return Math.min(line.from + character, line.to);
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
      const text = UNSUPPORTED_EFFECT_TEXT[effect];
      if (text) node.appendChild(paragraph('tl-completion-info-note', text));
    }
    return node;
  }

  // completionItem/resolve when the bridge exposes it. Resolved once per item
  // and cached; a failure degrades to what the item already carried.
  function makeInfoRenderer(item) {
    let pending = null;
    let resolved = null;
    return function info() {
      if (!pending) {
        const bridge = global.termlabLspBridge;
        if (!bridge || typeof bridge.resolveCompletionItem !== 'function') {
          return renderInfo(item, null);
        }
        pending = Promise.resolve(bridge.resolveCompletionItem(item.id))
          .then((value) => { resolved = value || null; })
          .catch((error) => { record('resolve', `completion resolve failed: ${error}`); });
      }
      return pending.then(() => renderInfo(item, resolved));
    };
  }

  // --- applying an item -----------------------------------------------------

  function primaryEdit(doc, item, from, to) {
    const edit = item.textEdit;
    if (edit && edit.kind === 'textEdit' && edit.range) {
      return {
        from: offsetAt(doc, edit.range.start),
        to: offsetAt(doc, edit.range.end),
        insert: String(edit.newText || ''),
      };
    }
    if (edit && edit.kind === 'insertReplaceEdit' && edit.insert) {
      // Insert, never replace: this phase does not eat text to the right of
      // the caret on the strength of a suggestion.
      return {
        from: offsetAt(doc, edit.insert.start),
        to: offsetAt(doc, edit.insert.end),
        insert: String(edit.newText || ''),
      };
    }
    const insert = typeof item.insertText === 'string' && item.insertText
      ? item.insertText
      : String(item.label || '');
    return { from, to, insert };
  }

  function additionalChanges(doc, item, primary) {
    const edits = Array.isArray(item.additionalTextEdits) ? item.additionalTextEdits : [];
    if (!edits.length) return [];
    const changes = edits
      .filter((edit) => edit && edit.range)
      .map((edit) => ({
        from: offsetAt(doc, edit.range.start),
        to: offsetAt(doc, edit.range.end),
        insert: String(edit.newText || ''),
      }));
    const ordered = changes.concat([primary]).sort((a, b) => a.from - b.from || a.to - b.to);
    for (let index = 1; index < ordered.length; index += 1) {
      if (ordered[index].from < ordered[index - 1].to) {
        // All or nothing for the extras: a half-applied import block is worse
        // than none, and the primary edit is what the user actually asked for.
        record(
          'additionalEdits',
          `refused ${changes.length} additional edit(s) for "${item.label}": ranges overlap`,
        );
        return [];
      }
    }
    return changes;
  }

  function noteUnsupportedEffects(item) {
    for (const effect of (item.unsupportedEffects || [])) {
      const text = UNSUPPORTED_EFFECT_TEXT[effect];
      if (text) record('unsupportedEffect', `"${item.label}": ${effect} not applied — ${text}`);
    }
  }

  // Run CodeMirror's snippet applier without letting its transaction land, so
  // the additional edits can ride in it.
  //
  // The applier takes a duck-typed {state, dispatch} editor and ends in
  // `editor.dispatch(editor.state.update(spec))` — a Transaction, which
  // nothing can be merged into. Handing it a state whose `update` returns the
  // spec unchanged is what turns that into something mergeable. Returns null
  // if anything about that assumption stops holding (a Transaction arrives
  // anyway, or the applier throws), so the caller can fall back rather than
  // dispatch something malformed.
  function captureSnippetSpec(view, template, item, from, to) {
    const CM = cm();
    if (!CM || typeof CM.snippet !== 'function') return null;
    let captured = null;
    try {
      const proxyState = Object.create(view.state);
      proxyState.update = (spec) => spec;
      CM.snippet(template)(
        { state: proxyState, dispatch: (value) => { captured = value; } },
        item,
        from,
        to,
      );
    } catch (error) {
      record('snippet', `snippet expansion failed for "${item.label}": ${error}`);
      return null;
    }
    // A Transaction carries the resulting state; a spec does not.
    if (!captured || captured.state !== undefined) return null;
    return captured;
  }

  // The one transaction. Undo, the dirty flag and the versioned change stream
  // sent to Rust must all see a single coherent change, never a primary edit
  // followed by a separate import insertion.
  function applyItem(item, view, from, to) {
    const CM = cm();
    if (!CM || !view) return;
    noteUnsupportedEffects(item);
    const doc = view.state.doc;
    const primary = primaryEdit(doc, item, from, to);
    const extras = additionalChanges(doc, item, primary);

    if (item.isSnippet && typeof CM.snippet === 'function') {
      // Nothing to merge: let the applier dispatch its own transaction, which
      // is already exactly one and exactly what CodeMirror intends.
      if (!extras.length) {
        CM.snippet(primary.insert)(view, item, primary.from, primary.to);
        return;
      }
      const spec = captureSnippetSpec(view, primary.insert, item, primary.from, primary.to);
      if (spec) {
        // Array entries are all relative to the pre-change document, which is
        // what both the server's ranges and the snippet's own range are.
        view.dispatch({ ...spec, changes: [spec.changes].concat(extras) });
        return;
      }
      record(
        'snippet',
        `"${item.label}": expanded as plain text so its additional edits could stay in one change`,
      );
    }

    const changes = extras.concat([primary]).sort((a, b) => a.from - b.from);
    view.dispatch({ changes });
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

  function toOption(item, index) {
    const option = {
      // CodeMirror matches on `label`, so filterText — which is what the
      // server said to match on — has to be the label, with the human-facing
      // text moved to displayLabel.
      label: String(item.filterText || item.label || ''),
      type: KIND_TYPES[item.kind] || 'text',
      // Descending, so the server's ranking survives CodeMirror's own sort.
      // Clamped because CodeMirror documents boost as -99..99 and a long list
      // would otherwise run straight out of that range; past the first ~200
      // items the ranking no longer decides anything a user sees.
      boost: Math.max(-99, 99 - index),
      info: makeInfoRenderer(item),
      apply: (view, completion, from, to) => applyItem(item, view, from, to),
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
    const version = state.version;
    const position = lspPositionAt(context.state.doc, context.pos);
    const from = wordStart(context);

    let response = null;
    try {
      response = await requestCompletion(pane, position, trigger.character);
    } catch (error) {
      record('completion', `completion request failed: ${error}`);
      return null;
    }
    if (!response || !Array.isArray(response.items) || !response.items.length) return null;
    // Everything below is the stale-result guard. requestFeature applies its
    // own; this one is what makes the guard true of THIS source's context too.
    if (context.aborted) return null;
    const current = documentStateFor(pane);
    if (!current || current.documentId !== documentId) return null;
    if (response.documentId !== documentId) return null;
    if (response.sourceVersion !== version || current.version !== version) return null;

    return {
      from,
      options: orderedItems(response.items).map(toOption),
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
  // Every binding below asks completionStatus first and returns false when the
  // popup is not open, which is the whole reason this can outrank vim: with no
  // popup, Escape still leaves insert mode in one press, Enter still opens a
  // line and Tab still indents.

  function popupOpen(view) {
    const CM = cm();
    if (!CM || typeof CM.completionStatus !== 'function' || !view) return false;
    return CM.completionStatus(view.state) !== null;
  }

  function whenOpen(command) {
    return (view) => {
      if (!popupOpen(view) || typeof command !== 'function') return false;
      return command(view) !== false;
    };
  }

  function keymapBindings() {
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
        run: (view) => (typeof CM.startCompletion === 'function' ? CM.startCompletion(view) : false),
      },
      { key: 'Escape', run: whenOpen(CM.closeCompletion) },
      { key: 'Enter', run: whenOpen(CM.acceptCompletion) },
      { key: 'Tab', run: whenOpen(CM.acceptCompletion) },
      // With autocompletion's own keymap off, these are the only thing
      // navigating the list — and while the popup is closed they are still
      // vim's (and the default keymap's) arrows.
      { key: 'ArrowDown', run: whenOpen(move(true)) },
      { key: 'ArrowUp', run: whenOpen(move(false)) },
      { key: 'PageDown', run: whenOpen(move(true, 'page')) },
      { key: 'PageUp', run: whenOpen(move(false, 'page')) },
    ];
  }

  // What editor-pane mounts. The keymap is raised with Prec.highest rather
  // than moved to the front of the extension list: vim's own keymap has to
  // stay the FIRST extension (test_editor_vim_glue.mjs pins that), and
  // precedence is the only other way to be heard before it.
  function extensions() {
    const CM = cm();
    if (!CM || typeof CM.autocompletion !== 'function') return [];
    const list = [
      CM.autocompletion({
        override: [completionSource],
        activateOnTyping: true,
        // One keymap owns these keys. Two competing ones is exactly how
        // Escape starts needing two presses in insert mode.
        defaultKeymap: false,
        closeOnBlur: true,
        maxRenderedOptions: 100,
      }),
    ];
    if (CM.Prec && typeof CM.Prec.highest === 'function' && CM.keymap) {
      list.push(CM.Prec.highest(CM.keymap.of(keymapBindings())));
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
    keymapBindings,
    completionSource,
    triggerManualCompletion,
    setSuggestionsWhileTyping: (enabled) => { suggestionsWhileTyping = enabled !== false; },
    sessionLog: () => sessionLogEntries.slice(),
  };
})(window);

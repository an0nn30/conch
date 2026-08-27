// Turning one normalized completion item into one CodeMirror transaction.
//
// Split out of lsp-completion.js because this half is where the hard parts
// live: the server's ranges were measured against the document as it was when
// the request went out, CodeMirror's are measured now, and the snippet
// applier's selection and placeholder effects are measured against a document
// that has had only the snippet applied to it. Getting an import insertion and
// a snippet into a single transaction means reconciling all three.
//
// Nothing here talks to the language service or to a pane. It is given an
// item, a view and a range, and it dispatches exactly once.
(function initTermLabLspCompletionApply(global) {
  'use strict';

  // Bounded because server data feeds it on a hot path: an item that carries
  // an unsupported workspace edit on every keystroke must not grow this.
  const SESSION_LOG_LIMIT = 200;
  const sessionLogEntries = [];

  const UNSUPPORTED_EFFECT_TEXT = {
    workspaceEdit: 'This suggestion also edits other files. Those edits were not applied.',
    command: 'This suggestion asks to run a server command, which is not supported yet.',
  };

  function cm() {
    return global.CM6 || null;
  }

  function record(kind, message) {
    sessionLogEntries.push({ at: Date.now(), kind: String(kind), message: String(message) });
    while (sessionLogEntries.length > SESSION_LOG_LIMIT) sessionLogEntries.shift();
  }

  // --- position conversion --------------------------------------------------
  //
  // CodeMirror columns are already UTF-16 code units, which is what LSP
  // positions count, so this is pure arithmetic.

  function offsetAt(doc, position) {
    if (!position) return null;
    const lineNumber = Math.min(Math.max(Number(position.line) + 1, 1), doc.lines);
    const line = doc.line(lineNumber);
    const character = Math.max(Number(position.character) || 0, 0);
    return Math.min(line.from + character, line.to);
  }

  // --- the primary edit -----------------------------------------------------

  function editRangeOf(item) {
    const edit = item.textEdit;
    if (!edit) return null;
    if (edit.kind === 'textEdit') return edit.range || null;
    // Insert, never replace: this phase does not eat text to the right of the
    // caret on the strength of a suggestion.
    if (edit.kind === 'insertReplaceEdit') return edit.insert || null;
    return null;
  }

  function insertTextOf(item) {
    const edit = item.textEdit;
    if (edit && typeof edit.newText === 'string') return edit.newText;
    if (typeof item.insertText === 'string' && item.insertText) return item.insertText;
    return String(item.label || '');
  }

  // The range to replace, in CURRENT document offsets.
  //
  // The server's range was measured when the request went out. CodeMirror
  // keeps a completion alive while the user goes on typing (validFor), mapping
  // the result's end forward — `to` here is that live end (autocomplete's
  // ActiveResult.map does `changes.mapPos(this.to, 1)`), while the server's
  // end still points at where the caret used to be. Applying the server's end
  // verbatim leaves the extra keystrokes behind: "va" + "l" + an item whose
  // edit says [17,19) produces "valuel".
  //
  // So the server's end is shifted by however far the caret has travelled
  // since the request, which is what VS Code does. `from` stays the server's,
  // because that is the one end the server is authoritative about and the one
  // CodeMirror never moves.
  function primaryEdit(state, item, from, to, requestPos) {
    const doc = state.doc;
    const insert = insertTextOf(item);
    const range = editRangeOf(item);
    if (!range) return { from, to, insert };
    const start = offsetAt(doc, range.start);
    const end = offsetAt(doc, range.end);
    const drift = Number.isInteger(requestPos) && Number.isInteger(to) ? to - requestPos : 0;
    const shifted = Math.min(Math.max(end + drift, start), doc.length);
    return { from: start, to: shifted, insert };
  }

  // --- additional edits -----------------------------------------------------

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

  // --- snippets -------------------------------------------------------------

  // What a snippet template says when nothing can expand it. `${1:value}`
  // keeps its default, `$1` and `${1}` are tab stops with no text. Escaped
  // braces and dollars are left alone and then unescaped, which is why the
  // placeholder patterns all refuse a preceding backslash.
  function snippetPlainText(template) {
    return String(template)
      .replace(/(?<!\\)\$\{\d+:([^}]*)\}/g, '$1')
      .replace(/(?<!\\)\$\{\d+\}/g, '')
      .replace(/(?<!\\)\$\d+/g, '')
      .replace(/\\([${}])/g, '$1');
  }

  // CodeMirror's own snippet applier, run against the real state but with its
  // transaction caught instead of dispatched, so the additional edits can be
  // merged into it. The applier takes a duck-typed {state, dispatch} editor
  // precisely so callers can do this; nothing is patched or shadowed.
  function captureSnippet(state, item, primary) {
    const CM = cm();
    if (!CM || typeof CM.snippet !== 'function') return null;
    let captured = null;
    try {
      CM.snippet(primary.insert)(
        { state, dispatch: (transaction) => { captured = transaction; } },
        item,
        primary.from,
        primary.to,
      );
    } catch (error) {
      record('snippet', `snippet expansion failed for "${item.label}": ${error}`);
      return null;
    }
    if (!captured || !captured.changes) return null;
    return {
      changes: captured.changes,
      selection: captured.selection || undefined,
      effects: captured.effects && captured.effects.length ? captured.effects.slice() : undefined,
      scrollIntoView: true,
      userEvent: 'input.complete',
    };
  }

  // --- merging --------------------------------------------------------------

  function plainSpec(state, primary) {
    const CM = cm();
    if (CM && typeof CM.insertCompletionText === 'function') {
      // The canonical helper: it also places the caret after the inserted text
      // and tags the transaction as `input.complete`, neither of which a bare
      // change spec does.
      return CM.insertCompletionText(state, primary.insert, primary.from, primary.to);
    }
    return { changes: [primary], scrollIntoView: true, userEvent: 'input.complete' };
  }

  // Fold the additional edits into a spec whose selection and effects are
  // already expressed in POST-change coordinates.
  //
  // Adding an import above the caret shifts everything below it, so a snippet
  // field recorded at "after the snippet ran" is wrong by exactly the length
  // that got inserted before it — the placeholder lands on "og(va" instead of
  // "value". Rather than compute that offset by hand, the extras are mapped
  // over the base change set and the selection and effects are mapped through
  // the result: CodeMirror's own position mapping, so multi-line and
  // multi-field cases come out right too.
  function mergeExtras(state, base, extras) {
    const CM = cm();
    if (!extras.length) return base;
    const canMap = CM && CM.ChangeSet && typeof CM.ChangeSet.of === 'function'
      && CM.StateEffect && typeof CM.StateEffect.mapEffects === 'function'
      && typeof state.changes === 'function';
    if (!canMap) {
      // Still one transaction, but a selection that cannot be corrected is
      // dropped rather than placed somewhere wrong.
      record('additionalEdits', 'merged additional edits without position mapping');
      return {
        ...base,
        changes: [base.changes].concat(extras),
        selection: undefined,
        effects: undefined,
      };
    }
    const baseChanges = state.changes(base.changes);
    const overBase = CM.ChangeSet.of(extras, state.doc.length).map(baseChanges);
    const merged = { ...base, changes: baseChanges.compose(overBase) };
    if (base.selection && typeof base.selection.map === 'function') {
      merged.selection = base.selection.map(overBase);
    }
    if (base.effects && base.effects.length) {
      merged.effects = CM.StateEffect.mapEffects(base.effects, overBase);
    }
    return merged;
  }

  // --- the one dispatch -----------------------------------------------------
  //
  // Undo, the dirty flag and the versioned change stream sent to Rust all have
  // to see a single coherent change, never a primary edit followed by a
  // separate import insertion.
  function applyItem(item, view, from, to, requestPos) {
    const CM = cm();
    if (!CM || !view || !view.state) return;
    noteUnsupportedEffects(item);
    const state = view.state;
    let primary = primaryEdit(state, item, from, to, requestPos);
    const extras = additionalChanges(state.doc, item, primary);

    let base = null;
    if (item.isSnippet && typeof CM.snippet === 'function') {
      base = captureSnippet(state, item, primary);
      if (!base) {
        record(
          'snippet',
          `"${item.label}": expanded as plain text so its edits could stay in one change`,
        );
        primary = { ...primary, insert: snippetPlainText(primary.insert) };
      }
    }
    if (!base) base = plainSpec(state, primary);
    view.dispatch(mergeExtras(state, base, extras));
  }

  global.termlabLspCompletionApply = {
    applyItem,
    snippetPlainText,
    unsupportedEffectText: (effect) => UNSUPPORTED_EFFECT_TEXT[effect] || null,
    record,
    sessionLog: () => sessionLogEntries.slice(),
  };
})(window);

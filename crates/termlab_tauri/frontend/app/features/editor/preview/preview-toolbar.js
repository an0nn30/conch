// The Editor / Split / Preview control for one markdown editor pane.
//
// The preview shipped reachable only by cmd+shift+y, which for anyone who has
// not read the keymap is the same as not shipping it. This is the visible
// route to the same three modes.
//
// It is PARENT-DOCUMENT chrome, deliberately: it renders into the pane
// element, never into the preview. That frame is sandboxed without
// allow-scripts and could not run a control at all — which is also why its
// styles live in components/editor.css and not in markdown-preview.css, the
// stylesheet that gets inlined into the frame.
//
// editor-pane.js owns WHEN this exists (only for a pane that has a preview)
// and what a click MEANS; this owns the buttons and their pressed state, and
// nothing else. Dependencies arrive through createToolbar rather than being
// read off globals, which is what makes it testable with no EditorView.
(function initTermLabPreviewToolbar(global) {
  'use strict';

  // Author-written, never interpolated — no user or document text reaches
  // these, so there is nothing here for utils.esc() to escape. Inline rather
  // than PNG assets so the glyphs take the button's `currentColor` and follow
  // the theme, and so the control has no file to fail to load.
  const FRAME = '<rect x="0.5" y="1.5" width="13" height="11" rx="1" fill="none" stroke="currentColor"/>';
  const ICONS = {
    editor: `<svg width="14" height="14" viewBox="0 0 14 14" aria-hidden="true" focusable="false">${FRAME}`
      + '<path d="M3 5h5M3 7.5h8M3 10h6" stroke="currentColor" stroke-linecap="round"/></svg>',
    split: `<svg width="14" height="14" viewBox="0 0 14 14" aria-hidden="true" focusable="false">${FRAME}`
      + '<path d="M7 1.5v11" stroke="currentColor"/>'
      + '<path d="M2.5 5h3M2.5 7.5h2" stroke="currentColor" stroke-linecap="round"/>'
      + '<path d="M8.5 4.5h3" stroke="currentColor" stroke-width="2" stroke-linecap="round"/>'
      + '<path d="M8.5 7.5h3M8.5 10h2" stroke="currentColor" stroke-linecap="round"/></svg>',
    preview: `<svg width="14" height="14" viewBox="0 0 14 14" aria-hidden="true" focusable="false">${FRAME}`
      + '<path d="M3 4.5h5" stroke="currentColor" stroke-width="2" stroke-linecap="round"/>'
      + '<path d="M3 7.5h8M3 10h6" stroke="currentColor" stroke-linecap="round"/></svg>',
  };

  // Order matches preview-mode.js's MODES, so the control reads left-to-right
  // as the same three states cmd+shift+y walks between. The label is both the
  // tooltip and the accessible name: an icon button with no text has no other
  // name to offer.
  const BUTTONS = [
    { mode: 'editor', label: 'Editor only' },
    { mode: 'split', label: 'Editor and preview' },
    { mode: 'preview', label: 'Preview only' },
  ];

  const ACTIVE_CLASS = 'is-active';

  /**
   * Build the control inside `hostEl`.
   *
   *   deps = { onSelect(mode), readMode() -> mode, document? }
   *
   * `onSelect` is the pane's own mode-change path — the one the shortcut and
   * the View menu also take — so the two routes cannot drift apart. `readMode`
   * is read at click time rather than trusted from the last setMode, so a mode
   * changed by any other route still suppresses a click on the active button.
   *
   * Returns { element, setMode, destroy }, or null when a dependency is
   * missing — the same quiet degradation the rest of the preview uses, so a
   * pane never ends up with a control that cannot act.
   */
  function createToolbar(hostEl, deps) {
    const options = deps || {};
    const doc = options.document || global.document;
    const onSelect = typeof options.onSelect === 'function' ? options.onSelect : null;
    const readMode = typeof options.readMode === 'function' ? options.readMode : null;
    if (!hostEl || !onSelect || !readMode) return null;
    if (!doc || typeof doc.createElement !== 'function') return null;

    const root = doc.createElement('div');
    root.className = 'md-preview-toolbar';
    root.setAttribute('role', 'group');
    root.setAttribute('aria-label', 'Markdown view mode');

    const buttons = [];
    // Every listener bound is recorded so destroy() removes exactly the set it
    // added. A pane is opened and closed many times in a session; one listener
    // surviving each close is a leak that grows with use.
    const bound = [];

    function bind(el, type, fn) {
      el.addEventListener(type, fn);
      bound.push({ el, type, fn });
    }

    for (const spec of BUTTONS) {
      const btn = doc.createElement('button');
      btn.className = 'tl-icon-btn md-preview-toolbar__btn';
      btn.setAttribute('type', 'button');
      btn.setAttribute('title', spec.label);
      btn.setAttribute('aria-label', spec.label);
      // A segmented control is a set of toggles, not a set of commands: pressed
      // state is how a screen reader user learns which mode the pane is in.
      btn.setAttribute('aria-pressed', 'false');
      btn.setAttribute('data-preview-mode', spec.mode);
      btn.innerHTML = ICONS[spec.mode] || '';

      bind(btn, 'click', () => {
        // Re-selecting the mode the pane is already in does nothing. The
        // shortcut cycles because it has one key for three states; a segmented
        // control that also moved on a second click of the same segment would
        // be a different control from the one it looks like.
        if (spec.mode === readMode()) return;
        onSelect(spec.mode);
      });
      // Cancelling mousedown's default keeps the caret in the document: the
      // shortcut leaves focus in the editor, and a button that pulled it out
      // would make the two routes behave differently for the very common
      // "switch to split, keep typing" case. Click still fires, and the button
      // is still reachable (and operable) by Tab.
      bind(btn, 'mousedown', (event) => {
        if (event && typeof event.preventDefault === 'function') event.preventDefault();
      });

      root.appendChild(btn);
      buttons.push({ mode: spec.mode, el: btn });
    }

    function setMode(mode) {
      for (const button of buttons) {
        const active = button.mode === mode;
        button.el.setAttribute('aria-pressed', active ? 'true' : 'false');
        if (active) button.el.classList.add(ACTIVE_CLASS);
        else button.el.classList.remove(ACTIVE_CLASS);
      }
      return mode;
    }

    function destroy() {
      for (const entry of bound) {
        if (typeof entry.el.removeEventListener === 'function') {
          entry.el.removeEventListener(entry.type, entry.fn);
        }
      }
      bound.length = 0;
      if (root.parentNode) root.parentNode.removeChild(root);
    }

    hostEl.appendChild(root);
    setMode(readMode());
    return { element: root, setMode, destroy };
  }

  global.termlabPreviewToolbar = { createToolbar };
})(window);

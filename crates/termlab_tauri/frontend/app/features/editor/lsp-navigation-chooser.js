// The multiple-definition chooser: its CodeMirror field and its DOM.
//
// Split out of lsp-navigation.js, which decides WHAT to navigate to; this half
// only shows a list of candidates and reports which one was picked.
//
// It is a CodeMirror tooltip anchored at the position the definition was
// requested from, so CodeMirror places it, maps it and tears it down — no
// absolute coordinates to recompute on scroll, and no focusable surface of its
// own: the editor keeps the keyboard the whole time, which is why the rows
// carry no tab stops and a click on one preventDefaults.
//
// The list is data from a language server (paths, and lines of somebody's
// source), so every string goes in through textContent. Nothing here ever
// assigns markup.
(function initTermLabLspNavigationChooser(global) {
  'use strict';

  let chooseHook = null;

  // Built once, on first use: two mounts of the extension must install the same
  // field, not two competing ones.
  let chooserEffect = null;
  let chooserField = null;

  function cm() {
    return global.CM6 || null;
  }

  function doc() {
    return global.document;
  }

  // --- the field ---------------------------------------------------------------

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
        create: (view) => ({ dom: render(value, view) }),
      } : null)),
    });
    return chooserField;
  }

  function valueOf(view) {
    if (!chooserField || !view || !view.state || typeof view.state.field !== 'function') return null;
    return view.state.field(chooserField, false) || null;
  }

  function setValue(view, value) {
    if (!chooserField || !view || typeof view.dispatch !== 'function') return;
    if (!value && !valueOf(view)) return;
    view.dispatch({ effects: chooserEffect.of(value) });
  }

  // --- rendering -----------------------------------------------------------------

  function part(className, text) {
    const node = doc().createElement('span');
    node.className = className;
    node.textContent = String(text);
    return node;
  }

  function render(value, view) {
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
          if (typeof chooseHook === 'function') chooseHook(view || null, index);
        });
      }
      root.appendChild(row);
    });
    return root;
  }

  // --- the surface lsp-navigation drives --------------------------------------------

  function open(view, items, anchor, origin) {
    if (!ensureField()) return false;
    // One overlay at a time: a hover or signature tooltip is about the symbol
    // the user is leaving, and two boxes at the same anchor is nobody's design.
    const tooltips = global.termlabLspTooltips;
    if (tooltips && typeof tooltips.dismiss === 'function') tooltips.dismiss(view);
    setValue(view, {
      items, index: 0, anchor, origin,
    });
    return true;
  }

  function close(view) {
    setValue(view, null);
  }

  function move(view, delta) {
    const value = valueOf(view);
    if (!value || !value.items.length) return false;
    const count = value.items.length;
    const index = ((value.index + delta) % count + count) % count;
    setValue(view, { ...value, index });
    return true;
  }

  function state(view) {
    const value = valueOf(view);
    if (!value) return { open: false, index: 0, items: [] };
    return { open: true, index: value.index, items: value.items };
  }

  function isOpen(view) {
    return valueOf(view) !== null;
  }

  function configure(options) {
    const opts = options || {};
    if (typeof opts.onChoose === 'function') chooseHook = opts.onChoose;
  }

  global.termlabLspNavigationChooser = {
    configure,
    field: ensureField,
    open,
    close,
    move,
    state,
    isOpen,
    valueOf,
    render,
  };
})(window);

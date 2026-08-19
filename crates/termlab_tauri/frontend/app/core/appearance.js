// The single owner of `data-tl-appearance` — the attribute `tokens-light.css`
// gates on (crates/termlab_tauri/frontend/styles/design-system/tokens-light.css:1)
// and `tl-icon.js` reads to pick icon variants (app/ui/tl-icon.js:19-20).
// Nothing else in the app may set or clear this attribute: every consumer
// (token CSS, icon variants, and — indirectly, via --tl-bg — the editor's
// isDarkTheme() luminance check) works for free once this module runs.
//
// `apply(mode)` maps an AppearanceMode string ('dark' | 'light' | 'system',
// case-insensitive; anything else falls back to 'dark', matching the Rust
// default in termlab_core::config::colors::AppearanceMode) onto the
// attribute:
//   - 'dark'   -> attribute removed (dark is the no-attribute baseline)
//   - 'light'  -> attribute set to "light"
//   - 'system' -> resolved live via matchMedia('(prefers-color-scheme: dark)'),
//                 with exactly one 'change' listener kept registered so an OS
//                 flip re-resolves without a reload. Calling apply() again
//                 with a non-system mode tears that listener down; calling
//                 apply('system') again while already in system mode is a
//                 no-op on the listener (idempotent — it does not stack).
//
// `current()` returns the last RESOLVED value ('dark' | 'light'), never
// 'system' — callers that need "is the app dark right now" get a real answer
// instead of the mode string.
//
// Every RESOLVED change (an explicit apply() that lands on a different value,
// or an OS flip while in 'system') is announced as a `tl-appearance-changed`
// CustomEvent dispatched on `document`, carrying
// `detail.resolved` ('dark' | 'light'). This is the one notification the
// baked-at-create-time consumers subscribe to: icon `src` variants
// (app/ui/tl-icon.js) and the editor's CodeMirror theme
// (app/config-runtime.js -> termlabEditorPane.refreshTheme). A DOM event
// rather than a subscriber list because this frontend has no bundler and no
// module graph — three separate HTML entrypoints load these files as plain
// classic scripts in whatever order the page lists them, so a consumer must
// be able to attach without appearance.js knowing it exists, and without a
// load-order contract. `document` is the only object all of them already
// share.
//
// The event fires ONLY on an actual change of the resolved value, so a
// dark-only app (apply('dark') over and over from the config-changed
// handler) never dispatches anything at all.
(function initTermLabAppearance(global) {
  'use strict';

  const ATTR = 'data-tl-appearance';
  const CHANGED_EVENT = 'tl-appearance-changed';

  let currentMode = null; // 'dark' | 'light' | 'system' | null (never applied)
  let currentResolved = 'dark';
  let mql = null;
  let mqlListener = null;

  function detachListener() {
    if (mql && mqlListener) {
      if (typeof mql.removeEventListener === 'function') {
        mql.removeEventListener('change', mqlListener);
      } else if (typeof mql.removeListener === 'function') {
        // Safari < 14 / older WebKitGTK MediaQueryList API.
        mql.removeListener(mqlListener);
      }
    }
    mql = null;
    mqlListener = null;
  }

  // Announce a resolved change to whoever is listening. Deliberately fired
  // AFTER currentResolved is updated, so a handler calling current() (or
  // reading the attribute) sees the new value, not the one being replaced.
  function notifyChanged(doc, resolved) {
    if (!doc || typeof doc.dispatchEvent !== 'function') return;
    const CustomEventCtor = global.CustomEvent;
    if (typeof CustomEventCtor !== 'function') return;
    doc.dispatchEvent(new CustomEventCtor(CHANGED_EVENT, { detail: { resolved } }));
  }

  function setResolved(doc, resolved) {
    const root = doc.documentElement;
    if (resolved === 'light') {
      root.setAttribute(ATTR, 'light');
    } else {
      root.removeAttribute(ATTR);
    }
    const changed = resolved !== currentResolved;
    currentResolved = resolved;
    if (changed) notifyChanged(doc, resolved);
  }

  function attachSystemListener(doc, matchMediaFn) {
    mql = matchMediaFn('(prefers-color-scheme: dark)');
    mqlListener = () => setResolved(doc, mql.matches ? 'dark' : 'light');
    if (typeof mql.addEventListener === 'function') {
      mql.addEventListener('change', mqlListener);
    } else if (typeof mql.addListener === 'function') {
      mql.addListener(mqlListener);
    }
  }

  function apply(mode, deps) {
    const doc = (deps && deps.doc) || global.document;
    const matchMediaFn = (deps && deps.matchMedia) || global.matchMedia;
    const normalized = String(mode || '').toLowerCase();

    if (normalized !== 'system') {
      detachListener();
      currentMode = normalized === 'light' ? 'light' : 'dark';
      setResolved(doc, currentMode);
      return;
    }

    // 'system': reuse the existing listener if one is already registered for
    // it (double-apply must not stack a second listener); otherwise tear
    // down whatever was there (a dark/light apply leaves nothing to tear
    // down) and register the one listener this mode ever needs.
    if (currentMode !== 'system' || !mql) {
      detachListener();
      if (typeof matchMediaFn === 'function') {
        attachSystemListener(doc, matchMediaFn);
      }
    }
    currentMode = 'system';
    // No matchMedia at all (no mql) means the OS preference is unresolvable,
    // which resolves to 'dark' — the same convention as the falsy/unknown
    // mode above, as theme.js:isDarkTheme()'s unparseable --tl-bg, and as the
    // Rust-side AppearanceMode default. Only 'light' is ever an affirmative
    // answer.
    setResolved(doc, !mql || mql.matches ? 'dark' : 'light');
  }

  function current() {
    return currentResolved;
  }

  global.termlabAppearance = { apply, current, CHANGED_EVENT };
})(window);

// One trust ask per project, as a non-blocking banner in the project tree's
// header area — never a modal. Editing is never blocked either way: this
// decides whether language servers start, not whether the file opens.
//
// "Trust project" calls the existing lsp_set_project_trust, whose persistence
// is what makes the banner never return for that project. "Not now" dismisses
// for this window's lifetime only and writes nothing — the per-file status
// strip (features/editor/project-context.js) stays the way to trust later.
(function initTermLabProjectTrustBanner(global) {
  'use strict';

  // "Not now" dismissals live here, keyed by canonical project root, for the
  // WINDOW's lifetime — not in a trust record (Not now must write nothing,
  // per spec) and not inside a single mount()'s closure. files-panel.js's
  // project tree handle (and its noticeHost, this banner's mount point) does
  // NOT survive a dual-pane <-> project mode toggle: setProjectMode(true)
  // destroys the old handle and builds a fresh one, calling mount() again
  // with a brand-new closure. This module-level set is what survives that
  // and keeps a dismissed banner from reappearing on the next toggle back
  // into project mode.
  const dismissedRoots = new Set();

  // A project with ANY recorded decision is settled: a recorded denial is a
  // decision too, and re-asking would be nagging.
  function decide(trustedProjects, root) {
    const records = Array.isArray(trustedProjects) ? trustedProjects : [];
    const settled = records.some((record) => record && String(record.root) === String(root));
    return settled ? 'settled' : 'ask';
  }

  function el(tag, className) {
    const node = global.document.createElement(tag);
    if (className) node.className = className;
    return node;
  }

  function mount(options) {
    const opts = options || {};
    const host = opts.host;
    const root = String(opts.root || '');
    const bridge = opts.bridge || global.termlabLspBridge || null;
    const onDecision = typeof opts.onDecision === 'function' ? opts.onDecision : () => {};

    let element = null;
    let pending = Promise.resolve();
    // The root-level guard: both buttons check and set this before doing
    // anything else, so a rapid double click on Trust (fired again before its
    // IPC round trip resolves) or a Trust-then-Not-now cross-click cannot
    // send two decisions for the same banner. A failed Trust call resets it
    // so the user can retry.
    let decided = false;

    function remove() {
      if (element && element.parentNode) element.remove();
      element = null;
    }

    function build() {
      const banner = el('div', 'tl-project-banner');
      banner.setAttribute('role', 'note');
      const text = el('span', 'tl-project-banner__text');
      text.textContent = 'Trust this project and start language servers?';
      const trust = el('button', 'tl-project-tree__button');
      trust.type = 'button';
      trust.textContent = 'Trust project';
      trust.setAttribute('data-project-trust', 'trust');
      trust.addEventListener('click', () => {
        if (decided) return;
        decided = true;
        // adapterId is null: the decision is about the PROJECT, not one
        // language server in it, which is the whole point of asking once.
        pending = Promise.resolve(bridge.setProjectTrust(root, null, 'trusted'))
          .then(() => { remove(); onDecision('trusted'); })
          .catch((error) => {
            decided = false;
            if (global.toast && typeof global.toast.error === 'function') {
              global.toast.error('Trust Failed', String(error));
            }
          });
      });
      const later = el('button', 'tl-project-tree__button');
      later.type = 'button';
      later.textContent = 'Not now';
      later.setAttribute('data-project-trust', 'later');
      later.addEventListener('click', () => {
        if (decided) return;
        decided = true;
        dismissedRoots.add(root);
        remove();
        onDecision('later');
      });
      banner.appendChild(text);
      banner.appendChild(trust);
      banner.appendChild(later);
      return banner;
    }

    const ready = (async () => {
      if (!host || !bridge || typeof bridge.trustedProjects !== 'function') return;
      if (dismissedRoots.has(root)) return;
      let records = [];
      try {
        records = await bridge.trustedProjects();
      } catch (error) {
        // Unreadable trust store: do not ask, and do not claim trust either.
        console.warn('project trust banner: could not read trusted projects', error);
        return;
      }
      if (decide(records, root) !== 'ask') return;
      // Re-check: a "Not now" click on an earlier mount for this same root
      // could have landed while this trustedProjects() call was in flight.
      if (dismissedRoots.has(root)) return;
      element = build();
      host.appendChild(element);
    })();

    return {
      ready,
      settled: () => pending,
      get element() { return element; },
      destroy: remove,
    };
  }

  global.termlabProjectTrustBanner = { decide, mount };
})(window);

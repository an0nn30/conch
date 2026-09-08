// One trust ask per project, as a non-blocking banner in the project tree's
// header area — never a modal. Editing is never blocked either way: this
// decides whether language servers start, not whether the file opens.
//
// "Trust project" calls the existing lsp_set_project_trust, whose persistence
// is what makes the banner never return for that project. "Not now" dismisses
// for this window's lifetime only and writes nothing — the per-file status
// strip (features/editor/project-context.js) stays the way to trust later.
//
// The grant this banner offers is strictly broader than the per-file trust
// dialog (project-context.js's trustProject()): one click here trusts every
// language server this project will ever run, not one adapter at a time —
// see decide() below, which only ever treats a PROJECT-WIDE record (no
// adapterId) as a settled decision. A per-adapter-only trust record (from
// the per-file dialog) does not settle this banner; it still offers the
// broader grant once. The copy has to disclose that breadth honestly, which
// is why it names the project and states the "every language server" scope
// rather than the generic wording an earlier draft used.
//
// A project-wide record is settled regardless of its decision: a revoked
// project stays revoked here too — this banner never re-asks a project the
// user has already made a project-wide call on, trusted OR not.
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

  // Settled means: a PROJECT-WIDE record exists for this root (adapterId
  // null/undefined), whatever its decision — trusted, denied or revoked all
  // count, since every one of them is a real answer to "does the whole
  // project get to run language servers", and re-asking any of them would be
  // nagging. A record scoped to one adapterId does NOT settle this banner:
  // it answers a narrower question (one language server) than the one this
  // banner asks (every language server this project will ever run), so the
  // broader grant is still worth offering once.
  function decide(trustedProjects, root) {
    const records = Array.isArray(trustedProjects) ? trustedProjects : [];
    const settled = records.some((record) => (
      record
      && String(record.root) === String(root)
      && (record.adapterId === null || record.adapterId === undefined)
    ));
    return settled ? 'settled' : 'ask';
  }

  function el(tag, className) {
    const node = global.document.createElement(tag);
    if (className) node.className = className;
    return node;
  }

  // editor.lsp.enabled, read the way the Settings dialog itself reads it
  // (settings/store.js, sections-editor.js): the full settings document from
  // get_all_settings, at `.editor.lsp.enabled`. get_app_config's flattened
  // snapshot does not carry this field (only
  // editor_lsp_suggestions_while_typing does), so that shortcut is not
  // available here. Never fatal, and permissive on any read failure or
  // missing field — LspConfig::default() has `enabled: true` on the Rust
  // side, so "unknown" and "on" mean the same thing; only an explicit
  // `false` suppresses the banner.
  async function lspGloballyEnabled(invoke) {
    if (typeof invoke !== 'function') return true;
    try {
      const settings = await invoke('get_all_settings');
      const enabled = settings && settings.editor && settings.editor.lsp && settings.editor.lsp.enabled;
      return enabled !== false;
    } catch (error) {
      console.warn('project trust banner: could not read editor.lsp.enabled', error);
      return true;
    }
  }

  function mount(options) {
    const opts = options || {};
    const host = opts.host;
    const root = String(opts.root || '');
    const name = opts.name ? String(opts.name) : '';
    const bridge = opts.bridge || global.termlabLspBridge || null;
    const invoke = typeof opts.invoke === 'function' ? opts.invoke : null;
    const onDecision = typeof opts.onDecision === 'function' ? opts.onDecision : () => {};

    let element = null;
    let pending = Promise.resolve();
    // The root-level guard: both buttons check and set this before doing
    // anything else, so a rapid double click on Trust (fired again before its
    // IPC round trip resolves) or a Trust-then-Not-now cross-click cannot
    // send two decisions for the same banner. A failed Trust call — whether
    // the IPC promise rejects OR the call throws synchronously (e.g. a
    // bridge missing the method entirely) — resets it so the user can retry
    // instead of the banner being permanently stuck.
    let decided = false;

    function remove() {
      if (element && element.parentNode) element.remove();
      element = null;
    }

    function resetOnFailure(error) {
      decided = false;
      if (global.toast && typeof global.toast.error === 'function') {
        global.toast.error('Trust Failed', String(error));
      }
    }

    function build() {
      const banner = el('div', 'tl-project-banner');
      banner.setAttribute('role', 'note');
      // Neither the trust decision nor the DOM insertion is user-initiated —
      // the banner just appears once the project tree has finished asking
      // its own questions — so role="note" alone (announced only when
      // focused) would leave a screen-reader user with no idea it showed up.
      banner.setAttribute('aria-live', 'polite');
      const copy = el('div', 'tl-project-banner__copy');
      const title = el('div', 'tl-project-banner__title');
      // Name the root: the project name if one was supplied, alongside the
      // full canonical path — the whole point of F3 is that this grant is
      // broader than the per-file dialog, so it must disclose at least as
      // much as that dialog's "will run for {canonicalRoot}" line.
      // textContent (not innerHTML), so this needs no HTML escaping — a
      // path or name containing "&"/"<" renders as literal text, exactly as
      // typed, the same way project-context.js's own trust dialog already
      // renders its disclosure line.
      title.textContent = `Trust ${name ? `${name} (${root})` : root}?`;
      const detail = el('div', 'tl-project-banner__text');
      detail.textContent = 'Language servers will run for every file in this project.';
      copy.appendChild(title);
      copy.appendChild(detail);

      const trust = el('button', 'tl-project-tree__button');
      trust.type = 'button';
      trust.textContent = 'Trust project';
      trust.setAttribute('data-project-trust', 'trust');
      trust.addEventListener('click', () => {
        if (decided) return;
        decided = true;
        try {
          // adapterId is null: the decision is about the PROJECT, not one
          // language server in it, which is the whole point of asking once.
          pending = Promise.resolve(bridge.setProjectTrust(root, null, 'trusted'))
            .then(() => { remove(); onDecision('trusted'); })
            .catch(resetOnFailure);
        } catch (error) {
          // A synchronous throw (e.g. bridge.setProjectTrust is not a
          // function) never reaches the .catch above — Promise.resolve()'s
          // argument is evaluated eagerly, before the Promise machinery
          // exists to catch anything. Without this try/catch, `decided`
          // would stay true forever and both buttons would be permanently
          // dead — the bridge failing is not a reason to brick the banner.
          resetOnFailure(error);
        }
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
      banner.appendChild(copy);
      banner.appendChild(trust);
      banner.appendChild(later);
      return banner;
    }

    const ready = (async () => {
      if (!host || !bridge || typeof bridge.trustedProjects !== 'function') return;
      if (dismissedRoots.has(root)) return;
      if (!(await lspGloballyEnabled(invoke))) return;
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
      // could have landed while the awaits above were in flight.
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

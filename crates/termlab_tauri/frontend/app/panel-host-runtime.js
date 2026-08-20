// The panel host WINDOW's boot runtime — the branch `app/main-runtime.js`
// takes when its own window label starts with `panelhost-`, instead of
// composing the terminal app.
//
// A panel host is one tool window rendered in its own OS window (see
// `src/panel_host.rs`). It loads `index.html`, exactly like a main window
// does — Rust builds it with `WebviewUrl::App("index.html")` so there is one
// script graph to keep in step, not two — which is precisely why the branch
// has to happen this early: everything main-runtime.js does after it (the
// compose runtime, the tab/pane managers, the event wiring, the first
// `createTab`, the shortcut table) belongs to a terminal window and must
// never run here.
//
// What this window IS, in full:
//   * appearance + theme CSS, mirroring settings.html's boot block
//     (settings.html:104-141) — the same three steps, in the same order, for
//     the same reason: the theme fetch carries the RESOLVED appearance, and
//     `termlabAppearance.apply` is what resolves it;
//   * `pluginWidgets.init({ invoke, listen })` with NO terminal callbacks, so
//     a plugin's tab/pty actions are inert here (every one of them sits
//     behind an `if (opts.xxx)` guard in app/panels/plugin-widgets.js);
//   * the tool-window REGISTRATIONS only — see
//     `termlabToolWindowRuntime.registerAll` — with no zones, strips, rails
//     or layout of any kind;
//   * a slim chrome (title + dock-back) around the ONE registration this
//     window was built for, mounted through the same `renderFn` the docked
//     panel would have used.
//
// This file never reads its own window label, for the same reason
// app/chooser-window-runtime.js does not: labels are unique per request
// (`panelhost-<parent>-<reqId>`), so nothing useful can be parsed out of one,
// and the only place a parent label matters arrives pre-resolved as
// `request.parentLabel`.
(function initTermLabPanelHostRuntime(global) {
  'use strict';

  // Applied to <body> so the stylesheet can stand down the main window's
  // shell (#app, and the strips/sidebars inside it) without this runtime
  // having to tear the markup out of a document it shares with main-runtime.
  const HOST_BODY_CLASS = 'tl-panelhost-window';

  // ---- The `panel-host-event` seam (Task 5 owns the consumers) -------------
  //
  // Rust re-dispatches a parent's window-state events to every live host of
  // that parent as `panel-host-event { event, payload }`
  // (`panel_host::panel_host_broadcast`). Re-dispatching those INTO this
  // window's own listeners is Task 5's job; all this boot does is make sure
  // none of them is lost in the meantime. Events that arrive before a sink is
  // installed are queued, and installing one drains the queue in arrival
  // order — so Task 5 can wire consumers whenever it likes without racing the
  // boot.
  const pendingEvents = [];
  let eventSink = null;

  function receivePanelHostEvent(payload) {
    if (eventSink) {
      eventSink(payload);
      return;
    }
    pendingEvents.push(payload);
  }

  function setEventSink(fn) {
    eventSink = typeof fn === 'function' ? fn : null;
    if (!eventSink) return;
    while (pendingEvents.length > 0) eventSink(pendingEvents.shift());
  }

  function getPendingEvents() {
    return pendingEvents.slice();
  }

  // ---- Self-close ----------------------------------------------------------

  // `abort_panel_host` is THE self-close for a REGISTERED host, not a
  // convenience: `open_panel_host` registers the entry before the window even
  // exists, so a plain `close()` from inside one would hit
  // `on_panel_host_close_requested`'s registered branch and be intercepted
  // into a hide — a window hidden forever with nothing in it. Abort removes
  // the entry, tells the parent (`panel-host-aborted`, which resets the tool
  // window's view mode without expecting a remount), and destroys the window.
  //
  // `currentWindow.close()` is only the fallback for the case where there is
  // no entry to abort — an unregistered `panelhost-*` window, which that same
  // hook lets close normally.
  function closeThisWindow(currentWindow) {
    if (currentWindow && typeof currentWindow.close === 'function') {
      currentWindow.close();
      return;
    }
    const tauri = global.__TAURI__;
    if (!tauri || !tauri.window || typeof tauri.window.getCurrentWindow !== 'function') return;
    const win = tauri.window.getCurrentWindow();
    if (win && typeof win.close === 'function') win.close();
  }

  function abortThisHost(invoke, currentWindow) {
    return Promise.resolve()
      .then(() => invoke('abort_panel_host'))
      .catch(() => {
        // No entry to abort (or the command is gone): fall back to a plain
        // close, which proceeds for an unregistered host.
        closeThisWindow(currentWindow);
      });
  }

  // Why the no-request path still calls the above, when the abort is expected
  // to FAIL: `get_panel_host_request` rejects only when the registry has no
  // entry for this window's label, and `abort_panel_host` requires exactly
  // that entry — so on today's Rust the abort always fails and the close
  // fallback is what does the work. It is called anyway because the two
  // commands do not have to keep failing for the same reason forever: should
  // `get_panel_host_request` ever reject for anything else (a transient IPC
  // or deserialization failure, say), the entry WOULD still exist, and a
  // plain `close()` on a REGISTERED host is intercepted into a hide — a
  // permanently invisible window with nothing in it. One failed IPC on a path
  // that is already terminal buys immunity from that.

  // ---- Appearance / theme --------------------------------------------------

  // A mirror of settings.html:104-141, not a re-invention: appearance first
  // (it resolves `auto`), then the theme CSS that carries the resolved value,
  // then the `config-changed` sync so a save made in any other window lands
  // here too. `applyUiConfig: true` follows settings.html rather than
  // chooser.html — a host renders real tool-window panels, so the UI font
  // sizes, the animation switch and the native frame tint all matter.
  async function applyAppearanceAndTheme(invoke, listenOnCurrentWindow) {
    const configService = global.termlabConfigService || {};

    try {
      const appCfg = await invoke('GET_APP_CONFIG');
      if (global.termlabAppearance && typeof global.termlabAppearance.apply === 'function') {
        global.termlabAppearance.apply(appCfg && appCfg.appearance_mode);
      }
      if (typeof configService.applyUiConfig === 'function') {
        configService.applyUiConfig(appCfg);
      }
    } catch (error) {
      console.warn('Failed to apply appearance in panel host window:', error);
    }

    try {
      const themeColors = await invoke('GET_THEME_COLORS', {
        resolvedAppearance: global.termlabAppearance
          && typeof global.termlabAppearance.current === 'function'
          ? global.termlabAppearance.current()
          : 'dark',
      });
      if (typeof configService.applyThemeCss === 'function') {
        configService.applyThemeCss(themeColors);
      }
    } catch (error) {
      console.warn('Failed to load theme colors in panel host window:', error);
    }

    if (global.termlabAppearanceSync && typeof global.termlabAppearanceSync.create === 'function') {
      global.termlabAppearanceSync.create({
        invoke,
        listen: listenOnCurrentWindow,
        applyUiConfig: true,
        label: 'panel host',
      }).init();
    }
  }

  // ---- Chrome --------------------------------------------------------------

  // A slim header and a content root, and nothing else. Deliberately NOT the
  // zone header the docked panel gets (`.tl-toolwindow__header`, built by
  // tool-window-manager.js): that one carries the zone's own affordances
  // (split, move, hide), none of which mean anything to a window that IS the
  // panel.
  function buildChrome(title, onDock) {
    const rootEl = document.createElement('div');
    rootEl.className = 'tl-panelhost';

    const headerEl = document.createElement('div');
    headerEl.className = 'tl-panelhost__header';
    // Windows/Linux hosts are built without native decorations
    // (`create_panel_host_window`'s `use_custom_titlebar`), so the header is
    // the only thing left to drag the window by.
    headerEl.setAttribute('data-tauri-drag-region', '');

    const titleEl = document.createElement('div');
    titleEl.className = 'tl-panelhost__title';
    titleEl.setAttribute('data-tauri-drag-region', '');
    titleEl.textContent = title;
    headerEl.appendChild(titleEl);

    const actionsEl = document.createElement('div');
    actionsEl.className = 'tl-panelhost__actions';
    const dockButtonEl = document.createElement('button');
    dockButtonEl.className = 'tl-panelhost__action';
    dockButtonEl.type = 'button';
    dockButtonEl.dataset.action = 'dock';
    dockButtonEl.textContent = 'Dock';
    dockButtonEl.title = 'Dock back into the main window';
    dockButtonEl.setAttribute('aria-label', 'Dock back into the main window');
    dockButtonEl.addEventListener('click', onDock);
    actionsEl.appendChild(dockButtonEl);
    headerEl.appendChild(actionsEl);

    const contentRootEl = document.createElement('div');
    contentRootEl.className = 'tl-panelhost__content';

    rootEl.appendChild(headerEl);
    rootEl.appendChild(contentRootEl);
    return { rootEl, headerEl, titleEl, dockButtonEl, contentRootEl };
  }

  // The element pair `tool-window-manager.js`'s `ensureWindowElement` builds
  // for a docked panel, rebuilt here verbatim (class names included) so a
  // renderFn cannot tell which side of the pop-out it is running on and the
  // shared `.tool-window-content` / `.tool-window-scroll-viewport` rules in
  // styles/tool-windows.css apply unchanged.
  function mountRegistration(contentRootEl, registration) {
    const panelEl = document.createElement('div');
    panelEl.className = 'tool-window-content';
    panelEl.dataset.toolWindow = registration.id;
    const renderRootEl = document.createElement('div');
    renderRootEl.className = 'tool-window-scroll-viewport';
    panelEl.appendChild(renderRootEl);
    contentRootEl.appendChild(panelEl);
    registration.renderFn(renderRootEl);
    return { panelEl, renderRootEl };
  }

  // ---- Boot ----------------------------------------------------------------

  async function boot(deps) {
    const options = deps || {};
    const invoke = options.invoke;
    const listen = options.listen;
    const listenOnCurrentWindow = options.listenOnCurrentWindow;
    const currentWindow = options.currentWindow || null;

    // "A host with no request must not linger", the panel-host twin of the
    // chooser's rule: a rejection means Rust has no entry for THIS window's
    // label — displaced out from under it, or already torn down. Abort, then
    // close; see abortThisHost's second comment for why the abort is issued
    // even though this particular path expects it to fail.
    let request;
    try {
      request = await invoke('get_panel_host_request');
    } catch (_) {
      await abortThisHost(invoke, currentWindow);
      return { status: 'no-request' };
    }

    if (document.body && document.body.classList) {
      document.body.classList.add(HOST_BODY_CLASS);
    }

    await applyAppearanceAndTheme(invoke, listenOnCurrentWindow);

    // No terminal callbacks, deliberately: `createTab`, `renameActiveTab`,
    // `renameTabById`, `focusTabById` and `writeToActivePty` all sit behind
    // `if (opts.xxx)` guards in app/panels/plugin-widgets.js, so leaving them
    // out makes a plugin's tab and pty actions inert in this window rather
    // than half-working against tabs that do not exist here.
    if (global.pluginWidgets && typeof global.pluginWidgets.init === 'function') {
      global.pluginWidgets.init({ invoke, listen });
    }

    if (global.termlabToolWindowRuntime && typeof global.termlabToolWindowRuntime.create === 'function') {
      const toolWindowRuntime = global.termlabToolWindowRuntime.create({
        invoke,
        listen,
        listenOnCurrentWindow,
        // A host owns no sidebars and no zones, so it has nothing truthful to
        // say about the saved layout — and a write from here would clobber
        // the widths and visibility flags the PARENT is keeping. Reads answer
        // empty for the same reason.
        layoutService: {
          getSavedLayout: () => Promise.resolve({}),
          saveLayout: () => {},
          savePartialLayout: () => {},
        },
        debouncedFitAndResize: () => {},
        getCurrentTab: () => null,
        getCurrentPane: () => null,
      });
      await toolWindowRuntime.registerAll({ registrationsOnly: true });
    }

    const manager = global.toolWindowManager;
    const registration = manager && typeof manager.getRegistration === 'function'
      ? manager.getRegistration(request.toolWindowId)
      : null;

    // Nothing registered under this id — a plugin that was removed between
    // the pop-out and this boot, or an id from a stale saved layout. There is
    // no panel to host, so the window takes itself out of the registry and
    // dies; `panel-host-aborted` tells the parent to reset the view mode
    // WITHOUT waiting for a remount that is never coming.
    if (!registration || typeof registration.renderFn !== 'function') {
      await abortThisHost(invoke, currentWindow);
      return { status: 'aborted', request };
    }

    const chrome = buildChrome(
      request.title || registration.title || request.toolWindowId,
      () => {
        invoke('dock_panel_host', { toolWindowId: request.toolWindowId }).catch(() => {});
      },
    );
    document.body.appendChild(chrome.rootEl);

    const mounted = mountRegistration(chrome.contentRootEl, registration);

    // Subscribed BEFORE `panel_host_ready`: the parent may broadcast the
    // moment this window becomes visible, and an event that arrives before
    // Task 5 installs its sink is queued rather than dropped.
    if (typeof listenOnCurrentWindow === 'function') {
      const registrationPromise = listenOnCurrentWindow('panel-host-event', (event) => {
        receivePanelHostEvent(event && event.payload ? event.payload : null);
      });
      if (registrationPromise && typeof registrationPromise.catch === 'function') {
        registrationPromise.catch(() => {});
      }
    }

    // Show + focus, only now that the panel is actually on screen — the
    // panel-host twin of `app_ready` (and of the chooser's `chooser_ready`).
    try {
      await invoke('panel_host_ready');
    } catch (error) {
      console.warn('Failed to show panel host window:', error);
    }

    return {
      status: 'mounted',
      request,
      registration,
      rootEl: chrome.rootEl,
      headerEl: chrome.headerEl,
      titleEl: chrome.titleEl,
      dockButtonEl: chrome.dockButtonEl,
      contentRootEl: chrome.contentRootEl,
      panelEl: mounted.panelEl,
      renderRootEl: mounted.renderRootEl,
    };
  }

  global.termlabPanelHostRuntime = {
    boot,
    setEventSink,
    getPendingEvents,
    HOST_BODY_CLASS,
  };
})(window);

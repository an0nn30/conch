// Keeps a SECONDARY window (settings, chooser) in step with an appearance or
// theme change made somewhere else.
//
// save_settings writes the config and broadcasts `config-changed` to every
// window (crates/termlab_tauri/src/settings.rs), but a broadcast only helps
// windows that listen, and until this module existed the only listener lived
// in app/config-runtime.js — which index.html alone loads. So the standalone
// settings window flipped the MAIN window from its own Apply button and left
// itself on the old appearance, and a chooser open across a save stayed stale
// for its whole life.
//
// Not folded into config-runtime.js: that module also rebuilds terminal
// themes, fonts, vim mode and pane state, none of which exists in these
// windows. This is the small subset a chrome-only window needs, matching what
// each of those windows already does at boot.
//
// `applyUiConfig: true` mirrors settings.html's boot, which calls
// applyUiConfig (UI font sizes, animations, and — via applyNativeWindowTheme
// — the macOS frame tint). chooser.html's boot does not call it, so its sync
// does not either: this module re-runs a window's boot appearance work, it
// does not add work that window never did.
(function initTermLabAppearanceSync(global) {
  'use strict';

  function create(deps) {
    const invoke = deps && deps.invoke;
    const listen = deps && deps.listen;
    const withUiConfig = !!(deps && deps.applyUiConfig);
    const label = (deps && deps.label) || 'window';

    // Appearance BEFORE theme CSS, the same ordering app/config-runtime.js
    // uses: the token-CSS switch and the terminal accent vars applyThemeCss
    // writes then land together instead of a frame apart.
    //
    // Two independent try blocks, not one: the appearance re-apply and the
    // theme CSS have no dependency on each other, and a failing GET_APP_CONFIG
    // must not leave the window on stale terminal colours as well as a stale
    // appearance.
    async function reapply() {
      const configService = global.termlabConfigService || {};

      try {
        const appCfg = await invoke('GET_APP_CONFIG');
        if (global.termlabAppearance && typeof global.termlabAppearance.apply === 'function') {
          global.termlabAppearance.apply(appCfg && appCfg.appearance_mode);
        }
        if (withUiConfig && typeof configService.applyUiConfig === 'function') {
          configService.applyUiConfig(appCfg);
        }
      } catch (error) {
        console.warn(`Failed to re-apply appearance in ${label} window:`, error);
      }

      try {
        const tc = await invoke('GET_THEME_COLORS');
        if (typeof configService.applyThemeCss === 'function') {
          configService.applyThemeCss(tc);
        }
      } catch (error) {
        console.warn(`Failed to reload theme colors in ${label} window:`, error);
      }
    }

    function init() {
      if (typeof listen === 'function') {
        // The handler returns reapply()'s promise. Tauri ignores it; a test
        // awaits it rather than racing the two invokes.
        const registration = listen('config-changed', () => reapply());
        // listenOnCurrentWindow rejects when the Tauri event API is missing;
        // an unhandled rejection there would be noise, not a failure this
        // window can do anything about.
        if (registration && typeof registration.catch === 'function') {
          registration.catch(() => {});
        }
      }
      return { reapply };
    }

    return { init, reapply };
  }

  global.termlabAppearanceSync = { create };
})(window);

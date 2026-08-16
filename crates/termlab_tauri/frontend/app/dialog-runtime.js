(function initTermLabDialogRuntime(global) {
  function create(deps) {
    const invoke = deps.invoke;
    const esc = deps.esc;
    const refocusActiveTerminal = deps.refocusActiveTerminal;
    const isCommandPaletteOpen = deps.isCommandPaletteOpen;
    const keyboardRouter = global.termlabKeyboardRouter;

    function initOverlayFocusHandlers() {
      // True while ANY dialog is open — either a legacy .ssh-overlay (still
      // used by command-palette-runtime.js, core/dialog-service.js, and
      // features/settings/renderers.js; migrating those is Phase 5b's job,
      // not design-system-phase-5a's) or a tl-dialog-shell dialog
      // (app/ui/tl-dialog.js; everything this phase migrated onto it: SSH,
      // keygen, files, plugin, About, restart, tunnels, vault).
      const isAnyDialogOpen = () =>
        !!document.querySelector('.ssh-overlay') ||
        !!(global.tlDialog && typeof global.tlDialog.count === 'function' && global.tlDialog.count() > 0);

      const handleEscape = (event) => {
        if (event.key !== 'Escape') return false;

        if (isAnyDialogOpen()) return false;

        // Popup menus (window.tlMenu) own their own Escape dismissal via the
        // keyboard router at a higher priority than this handler, so there is
        // no longer a menu class to check for here.
        refocusActiveTerminal();
        return false;
      };
      if (keyboardRouter && typeof keyboardRouter.register === 'function') {
        keyboardRouter.register({
          name: 'overlay-focus-handler',
          priority: 100,
          onKeyDown: (event) => handleEscape(event),
        });
      } else {
        console.warn('dialog-runtime: keyboard router unavailable, overlay focus escape handler not registered');
      }

      function scheduleRefocusAfterOverlayClose() {
        setTimeout(() => {
          if (isAnyDialogOpen()) return;
          if (isCommandPaletteOpen()) return;
          const active = document.activeElement;
          if (active) {
            const tag = String(active.tagName || '').toLowerCase();
            if (tag === 'input' || tag === 'textarea' || tag === 'select' || active.isContentEditable) return;
          }
          refocusActiveTerminal();
        }, 0);
      }

      // Legacy dialogs (command-palette-runtime.js, core/dialog-service.js,
      // features/settings/renderers.js — Phase 5b's job to migrate onto
      // tl-dialog) still build a plain .ssh-overlay div and el.remove() it
      // with no lifecycle hook, so a MutationObserver is still needed to
      // notice when the last one of those closes.
      let previousOverlayCount = document.querySelectorAll('.ssh-overlay').length;
      const overlayFocusObserver = new MutationObserver(() => {
        const overlayCount = document.querySelectorAll('.ssh-overlay').length;
        if (previousOverlayCount > 0 && overlayCount === 0) {
          scheduleRefocusAfterOverlayClose();
        }
        previousOverlayCount = overlayCount;
      });
      overlayFocusObserver.observe(document.body, { childList: true, subtree: true });

      // Migrated dialogs (tl-dialog shell) get an explicit "stack emptied"
      // callback instead of being inferred from a MutationObserver.
      if (global.tlDialog && typeof global.tlDialog.onAllClosed === 'function') {
        global.tlDialog.onAllClosed(scheduleRefocusAfterOverlayClose);
      }
    }

    async function showAboutDialog() {
      let info;
      try { info = await invoke('get_about_info'); } catch (_) { info = {}; }
      const ver = info.version || '?';
      const commit = (info.commit || 'dev').substring(0, 7);
      const rawDate = info.build_date || '';
      const buildDate = rawDate
        ? new Date(rawDate).toLocaleDateString(undefined, { year: 'numeric', month: 'long', day: 'numeric' })
        : 'unknown';
      const platform = (info.platform || '?') + ' ' + (info.arch || '');

      if (!global.tlDialog || typeof global.tlDialog.open !== 'function') return;

      let handle = null;
      handle = global.tlDialog.open({
        title: 'About TermLab',
        ariaLabel: 'About TermLab',
        size: 'md',
        body: (bodyEl) => {
          bodyEl.style.cssText = 'padding-top:6px;display:flex;gap:20px;align-items:flex-start';
          bodyEl.innerHTML = `
            <img src="icons/app-icon.png" style="width:64px;height:64px;flex-shrink:0;border-radius:12px" />
            <div style="flex:1;min-width:0">
              <div style="font-size:18px;font-weight:700;margin-bottom:4px">TermLab ${esc(ver)}</div>
              <div style="color:var(--text-secondary);font-size:12px;margin-bottom:12px">Build #${esc(commit)}, built on ${esc(buildDate)}</div>
              <div style="color:var(--text-secondary);font-size:12px;margin-bottom:12px">Platform: ${esc(platform)}</div>
              <div style="color:var(--fg);font-size:12px;line-height:1.6;margin-bottom:12px">A terminal-native workstation for SSH-heavy engineering workflows.</div>
              <div style="color:var(--text-secondary);font-size:11px;line-height:1.6;margin-bottom:12px">TermLab unifies terminal, remote sessions, files, tunnels, credentials, and plugins in one cross-platform app.</div>
              <div style="color:var(--text-secondary);font-size:11px;line-height:1.6">
                Licensed under <a href="#" style="color:var(--blue)" onclick="event.preventDefault();if(window.__TAURI__&&window.__TAURI__.shell)window.__TAURI__.shell.open('https://www.apache.org/licenses/LICENSE-2.0')">Apache License 2.0</a><br>
                Icons: <a href="#" style="color:var(--blue)" onclick="event.preventDefault();if(window.__TAURI__&&window.__TAURI__.shell)window.__TAURI__.shell.open('https://github.com/snwh/paper-icon-theme')">Paper Icon Theme</a> by Sam Hewitt (<a href="#" style="color:var(--blue)" onclick="event.preventDefault();if(window.__TAURI__&&window.__TAURI__.shell)window.__TAURI__.shell.open('https://creativecommons.org/licenses/by-sa/4.0/')">CC BY-SA 4.0</a>)<br><br>
                <a href="#" style="color:var(--blue)" onclick="event.preventDefault();if(window.__TAURI__&&window.__TAURI__.shell)window.__TAURI__.shell.open('https://github.com/an0nn30/conch')">github.com/an0nn30/conch</a>
              </div>
            </div>
          `;
        },
        buttons: [
          { label: 'Close', onSelect: () => handle.close() },
          { label: 'Copy Info', primary: true, onSelect: () => {
            const text = 'TermLab ' + ver + '\nBuild #' + commit + ', built on ' + buildDate + '\nPlatform: ' + platform;
            navigator.clipboard.writeText(text).then(() => {
              global.toast.success('Copied', 'Build info copied to clipboard.');
            });
            handle.close();
          } },
        ],
      });
    }

    return {
      initOverlayFocusHandlers,
      showAboutDialog,
    };
  }

  global.termlabDialogRuntime = {
    create,
  };
})(window);

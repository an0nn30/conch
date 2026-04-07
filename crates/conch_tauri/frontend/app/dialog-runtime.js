(function initConchDialogRuntime(global) {
  function create(deps) {
    const invoke = deps.invoke;
    const esc = deps.esc;
    const refocusActiveTerminal = deps.refocusActiveTerminal;
    const isCommandPaletteOpen = deps.isCommandPaletteOpen;
    const keyboardRouter = global.conchKeyboardRouter;

    function initOverlayFocusHandlers() {
      const handleEscape = (event) => {
        if (event.key !== 'Escape') return false;

        const overlay = document.querySelector('.ssh-overlay');
        if (overlay) return false;

        const ctxMenu = document.querySelector('.ssh-context-menu');
        if (ctxMenu) {
          ctxMenu.remove();
          return true;
        }

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

      let previousOverlayCount = document.querySelectorAll('.ssh-overlay').length;
      function scheduleRefocusAfterOverlayClose() {
        setTimeout(() => {
          if (document.querySelector('.ssh-overlay')) return;
          if (isCommandPaletteOpen()) return;
          const active = document.activeElement;
          if (active) {
            const tag = String(active.tagName || '').toLowerCase();
            if (tag === 'input' || tag === 'textarea' || tag === 'select' || active.isContentEditable) return;
          }
          refocusActiveTerminal();
        }, 0);
      }

      const overlayFocusObserver = new MutationObserver(() => {
        const overlayCount = document.querySelectorAll('.ssh-overlay').length;
        if (previousOverlayCount > 0 && overlayCount === 0) {
          scheduleRefocusAfterOverlayClose();
        }
        previousOverlayCount = overlayCount;
      });
      overlayFocusObserver.observe(document.body, { childList: true, subtree: true });
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

      const overlay = document.createElement('div');
      overlay.className = 'ssh-overlay';
      overlay.id = 'about-overlay';

      const dialog = document.createElement('div');
      dialog.className = 'ssh-form';
      dialog.style.width = '460px';

      const title = document.createElement('div');
      title.className = 'ssh-form-title';
      title.textContent = 'About Conch';
      dialog.appendChild(title);

      const body = document.createElement('div');
      body.style.cssText = 'padding:20px 24px;color:var(--fg);font-size:13px;line-height:1.7;display:flex;gap:20px;align-items:flex-start';

      const icon = document.createElement('img');
      icon.src = 'icons/app-icon.png';
      icon.style.cssText = 'width:64px;height:64px;flex-shrink:0;border-radius:12px';
      body.appendChild(icon);

      const content = document.createElement('div');
      content.style.cssText = 'flex:1;min-width:0';

      const heading = document.createElement('div');
      heading.style.cssText = 'font-size:18px;font-weight:700;margin-bottom:4px';
      heading.textContent = 'Conch ' + esc(ver);
      content.appendChild(heading);

      const build = document.createElement('div');
      build.style.cssText = 'color:var(--text-secondary);font-size:12px;margin-bottom:12px';
      build.textContent = 'Build #' + esc(commit) + ', built on ' + esc(buildDate);
      content.appendChild(build);

      const runtime = document.createElement('div');
      runtime.style.cssText = 'color:var(--text-secondary);font-size:12px;margin-bottom:12px';
      runtime.textContent = 'Platform: ' + esc(platform);
      content.appendChild(runtime);

      const blurb = document.createElement('div');
      blurb.style.cssText = 'color:var(--fg);font-size:12px;line-height:1.6;margin-bottom:12px';
      blurb.textContent = 'A terminal-native workstation for SSH-heavy engineering workflows.';
      content.appendChild(blurb);

      const position = document.createElement('div');
      position.style.cssText = 'color:var(--text-secondary);font-size:11px;line-height:1.6;margin-bottom:12px';
      position.textContent = 'Conch unifies terminal, remote sessions, files, tunnels, credentials, and plugins in one cross-platform app.';
      content.appendChild(position);

      const license = document.createElement('div');
      license.style.cssText = 'color:var(--text-secondary);font-size:11px;line-height:1.6';
      license.innerHTML = 'Licensed under <a href="#" style="color:var(--blue)" onclick="event.preventDefault();if(window.__TAURI__&&window.__TAURI__.shell)window.__TAURI__.shell.open(\'https://www.apache.org/licenses/LICENSE-2.0\')">Apache License 2.0</a><br>'
        + 'Icons: <a href="#" style="color:var(--blue)" onclick="event.preventDefault();if(window.__TAURI__&&window.__TAURI__.shell)window.__TAURI__.shell.open(\'https://github.com/snwh/paper-icon-theme\')">Paper Icon Theme</a> by Sam Hewitt (<a href="#" style="color:var(--blue)" onclick="event.preventDefault();if(window.__TAURI__&&window.__TAURI__.shell)window.__TAURI__.shell.open(\'https://creativecommons.org/licenses/by-sa/4.0/\')">CC BY-SA 4.0</a>)<br><br>'
        + '<a href="#" style="color:var(--blue)" onclick="event.preventDefault();if(window.__TAURI__&&window.__TAURI__.shell)window.__TAURI__.shell.open(\'https://github.com/an0nn30/conch\')">github.com/an0nn30/conch</a>';
      content.appendChild(license);

      body.appendChild(content);
      dialog.appendChild(body);

      const buttons = document.createElement('div');
      buttons.className = 'ssh-form-buttons';

      const closeBtn = document.createElement('button');
      closeBtn.className = 'ssh-form-btn';
      closeBtn.textContent = 'Close';
      closeBtn.addEventListener('click', () => overlay.remove());

      const copyBtn = document.createElement('button');
      copyBtn.className = 'ssh-form-btn primary';
      copyBtn.textContent = 'Copy Info';
      copyBtn.addEventListener('click', () => {
        const text = 'Conch ' + ver + '\nBuild #' + commit + ', built on ' + buildDate + '\nPlatform: ' + platform;
        navigator.clipboard.writeText(text).then(() => {
          global.toast.success('Copied', 'Build info copied to clipboard.');
        });
        overlay.remove();
      });

      buttons.appendChild(closeBtn);
      buttons.appendChild(copyBtn);
      dialog.appendChild(buttons);

      overlay.appendChild(dialog);
      overlay.addEventListener('mousedown', (event) => { if (event.target === overlay) overlay.remove(); });
      document.body.appendChild(overlay);
    }

    return {
      initOverlayFocusHandlers,
      showAboutDialog,
    };
  }

  global.conchDialogRuntime = {
    create,
  };
})(window);

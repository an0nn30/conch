// Terminal view — xterm.js + SSH session management + accessory bar.

(function (exports) {
  'use strict';

  let terminal = null;
  let fitAddon = null;
  let currentSessionId = null;
  let ctrlSticky = false;
  let altSticky = false;

  const ACCESSORY_KEYS = [
    { label: 'Esc',  send: '\x1b' },
    { label: 'Tab',  send: '\t' },
    { label: 'Ctrl', ctrl: true },
    { label: 'Alt',  alt: true },
    { label: '↑',    send: '\x1b[A' },
    { label: '↓',    send: '\x1b[B' },
    { label: '→',    send: '\x1b[C' },
    { label: '←',    send: '\x1b[D' },
    { label: '|',    send: '|' },
    { label: '/',    send: '/' },
    { label: '~',    send: '~' },
    { label: '-',    send: '-' },
  ];

  /** Open a terminal session. */
  async function connect(spec, password) {
    const view = document.getElementById('terminal-view');
    const titleEl = document.getElementById('terminal-title');
    view.classList.add('active');
    titleEl.textContent = 'Connecting to ' + spec + '...';

    // Create xterm.js instance
    if (!terminal) {
      createTerminal();
    } else {
      terminal.clear();
    }

    // Wait for the view to be fully laid out before fitting,
    // then use the fitted dimensions for the SSH connection.
    await new Promise(r => setTimeout(r, 100));
    if (fitAddon) fitAddon.fit();

    try {
      const sessionId = await window.__TAURI__.core.invoke('ssh_quick_connect', {
        spec,
        cols: terminal.cols,
        rows: terminal.rows,
        password: password || null,
      });

      currentSessionId = sessionId;
      titleEl.textContent = spec;

      // Fit again now that we're connected (in case layout shifted)
      requestAnimationFrame(() => {
        if (fitAddon) fitAddon.fit();
        terminal.focus();
      });
    } catch (err) {
      titleEl.textContent = 'Connection failed';
      window.toast.error('SSH Error', err);
      setTimeout(() => close(), 2000);
    }
  }

  /** Create the xterm.js terminal instance. */
  function createTerminal() {
    const container = document.getElementById('terminal-container');
    container.innerHTML = '';

    terminal = new window.Terminal({
      cursorBlink: true,
      fontSize: 14,
      fontFamily: "'SF Mono', Menlo, 'Fira Code', monospace",
      theme: {
        background: '#282a36',
        foreground: '#f8f8f2',
        cursor: '#f8f8f2',
        selectionBackground: '#44475a',
        black: '#21222c',
        red: '#ff5555',
        green: '#50fa7b',
        yellow: '#f1fa8c',
        blue: '#bd93f9',
        magenta: '#ff79c6',
        cyan: '#8be9fd',
        white: '#f8f8f2',
        brightBlack: '#6272a4',
        brightRed: '#ff6e6e',
        brightGreen: '#69ff94',
        brightYellow: '#ffffa5',
        brightBlue: '#d6acff',
        brightMagenta: '#ff92df',
        brightCyan: '#a4ffff',
        brightWhite: '#ffffff',
      },
      scrollback: 1000,
      allowProposedApi: true,
    });

    fitAddon = new window.FitAddon.FitAddon();
    terminal.loadAddon(fitAddon);

    // Use canvas renderer for iOS WKWebView performance (WebGL is unreliable)
    if (window.CanvasAddon) {
      terminal.loadAddon(new window.CanvasAddon.CanvasAddon());
    }

    terminal.open(container);

    // Fit after a frame so the container has dimensions
    requestAnimationFrame(() => {
      fitAddon.fit();
    });

    // Send user input to SSH
    terminal.onData((data) => {
      if (!currentSessionId) return;

      // Handle Ctrl sticky mode
      if (ctrlSticky) {
        ctrlSticky = false;
        updateModifierButtons();
        // Convert to Ctrl character (ASCII 1-26)
        const ch = data.toUpperCase().charCodeAt(0);
        if (ch >= 65 && ch <= 90) {
          data = String.fromCharCode(ch - 64);
        }
      }

      // Handle Alt sticky mode (sends ESC prefix)
      if (altSticky) {
        altSticky = false;
        updateModifierButtons();
        data = '\x1b' + data;
      }

      const bytes = new TextEncoder().encode(data);
      window.__TAURI__.core.invoke('ssh_write', {
        sessionId: currentSessionId,
        data: Array.from(bytes),
      }).catch(() => {});
    });

    // Handle resize
    terminal.onResize(({ cols, rows }) => {
      if (!currentSessionId) return;
      window.__TAURI__.core.invoke('ssh_resize', {
        sessionId: currentSessionId,
        cols, rows,
      }).catch(() => {});
    });

    // Refit on window resize / orientation change / iOS keyboard show/hide
    window.addEventListener('resize', () => {
      if (fitAddon && terminal) fitAddon.fit();
    });
    // visualViewport is more reliable for iOS keyboard events
    window.visualViewport?.addEventListener('resize', () => {
      if (fitAddon && terminal) fitAddon.fit();
    });

    // Build accessory bar
    buildAccessoryBar();
  }

  /** Build the accessory key bar. */
  function buildAccessoryBar() {
    const bar = document.getElementById('accessory-bar');
    bar.innerHTML = '';

    ACCESSORY_KEYS.forEach(key => {
      const btn = document.createElement('button');
      btn.className = 'accessory-key';
      btn.textContent = key.label;
      if (key.ctrl) btn.id = 'ctrl-key';
      if (key.alt) btn.id = 'alt-key';

      btn.addEventListener('click', () => {
        if (key.ctrl) {
          ctrlSticky = !ctrlSticky;
          updateCtrlButton();
          terminal.focus();
          return;
        }
        if (key.alt) {
          altSticky = !altSticky;
          updateModifierButtons();
          terminal.focus();
          return;
        }
        if (key.send && currentSessionId) {
          const bytes = new TextEncoder().encode(key.send);
          window.__TAURI__.core.invoke('ssh_write', {
            sessionId: currentSessionId,
            data: Array.from(bytes),
          }).catch(() => {});
        }
        terminal.focus();
      });

      bar.appendChild(btn);
    });
  }

  function updateModifierButtons() {
    const ctrlBtn = document.getElementById('ctrl-key');
    const altBtn = document.getElementById('alt-key');
    if (ctrlBtn) ctrlBtn.classList.toggle('sticky', ctrlSticky);
    if (altBtn) altBtn.classList.toggle('sticky', altSticky);
  }

  /** Close the terminal view and disconnect. */
  function close() {
    const view = document.getElementById('terminal-view');
    view.classList.remove('active');

    if (currentSessionId) {
      window.__TAURI__.core.invoke('ssh_disconnect', {
        sessionId: currentSessionId,
      }).catch(() => {});
      currentSessionId = null;
    }
  }

  /** Initialize event listeners. */
  function init() {
    // Back button
    document.getElementById('terminal-back')
      .addEventListener('click', close);

    // Disconnect button
    document.getElementById('terminal-disconnect')
      .addEventListener('click', close);

    // Listen for SSH output
    if (window.__TAURI__) {
      window.__TAURI__.event.listen('pty-output', (event) => {
        const { session_id, data } = event.payload;
        if (session_id === currentSessionId && terminal) {
          terminal.write(data);
        }
      });

      // Listen for SSH session exit
      window.__TAURI__.event.listen('pty-exit', (event) => {
        const { session_id } = event.payload;
        if (session_id === currentSessionId) {
          window.toast.info('Disconnected', 'Session closed by server.');
          setTimeout(() => close(), 1000);
        }
      });

      // Listen for host key prompts
      window.__TAURI__.event.listen('ssh-host-key-prompt', (event) => {
        showHostKeyPrompt(event.payload);
      });

      // Listen for password prompts
      window.__TAURI__.event.listen('ssh-password-prompt', (event) => {
        showPasswordPrompt(event.payload);
      });
    }
  }

  // ---------------------------------------------------------------------------
  // Auth prompt dialogs
  // ---------------------------------------------------------------------------

  function showHostKeyPrompt({ prompt_id, message, detail }) {
    const overlay = document.createElement('div');
    overlay.className = 'auth-overlay';
    overlay.innerHTML = `
      <div class="auth-dialog">
        <h3>Host Key Verification</h3>
        <p>${window.utils.esc(message)}</p>
        <p style="font-family:monospace;font-size:11px;">${window.utils.esc(detail)}</p>
        <div class="auth-dialog-buttons">
          <button class="auth-btn-cancel" id="hk-reject">Reject</button>
          <button class="auth-btn-confirm" id="hk-accept">Accept</button>
        </div>
      </div>
    `;
    document.body.appendChild(overlay);

    overlay.querySelector('#hk-accept').addEventListener('click', () => {
      window.__TAURI__.core.invoke('auth_respond_host_key', {
        promptId: prompt_id, accepted: true,
      });
      overlay.remove();
    });
    overlay.querySelector('#hk-reject').addEventListener('click', () => {
      window.__TAURI__.core.invoke('auth_respond_host_key', {
        promptId: prompt_id, accepted: false,
      });
      overlay.remove();
    });
  }

  function showPasswordPrompt({ prompt_id, message }) {
    const overlay = document.createElement('div');
    overlay.className = 'auth-overlay';
    overlay.innerHTML = `
      <div class="auth-dialog">
        <h3>Password Required</h3>
        <p>${window.utils.esc(message)}</p>
        <input type="password" id="pw-input" placeholder="Password"
               autocomplete="off" autocorrect="off" autocapitalize="none">
        <div class="auth-dialog-buttons">
          <button class="auth-btn-cancel" id="pw-cancel">Cancel</button>
          <button class="auth-btn-confirm" id="pw-submit">Connect</button>
        </div>
      </div>
    `;
    document.body.appendChild(overlay);

    const input = overlay.querySelector('#pw-input');
    input.focus();

    overlay.querySelector('#pw-submit').addEventListener('click', () => {
      window.__TAURI__.core.invoke('auth_respond_password', {
        promptId: prompt_id, password: input.value || null,
      });
      overlay.remove();
    });
    overlay.querySelector('#pw-cancel').addEventListener('click', () => {
      window.__TAURI__.core.invoke('auth_respond_password', {
        promptId: prompt_id, password: null,
      });
      overlay.remove();
    });
    input.addEventListener('keydown', (e) => {
      if (e.key === 'Enter') {
        overlay.querySelector('#pw-submit').click();
      }
    });
  }

  exports.terminalView = { init, connect, close };
})(window);

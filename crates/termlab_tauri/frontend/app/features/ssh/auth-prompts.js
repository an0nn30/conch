(function initTermLabSshAuthPrompts(global) {
  'use strict';

  // Dialogs currently open in THIS window, keyed by prompt id. The backend
  // broadcasts prompts to every window; when any window answers, it
  // broadcasts the resolution back so the other windows' copies close
  // without sending a second (already-consumed) response.
  const RESOLVED_EVENT = 'ssh-auth-prompt-resolved';
  const openPrompts = new Map();
  let installed = false;

  function trackPrompt(promptId, close) {
    openPrompts.set(promptId, close);
  }

  function closeResolvedPrompt(promptId) {
    if (!promptId) return;
    const close = openPrompts.get(promptId);
    if (!close) return;
    openPrompts.delete(promptId);
    close();
  }

  function showHostKeyPrompt(event, deps) {
    const d = deps || {};
    const payload = event && event.payload ? event.payload : {};
    const promptId = payload.prompt_id;
    const message = payload.message;
    const detail = payload.detail;
    if (!promptId) return false;

    const esc = typeof d.esc === 'function' ? d.esc : (value) => String(value == null ? '' : value);
    const invoke = typeof d.invoke === 'function' ? d.invoke : null;
    if (!invoke) return false;
    if (!global.tlDialog || typeof global.tlDialog.open !== 'function') return false;

    let handle = null;
    let done = false;
    const respond = (accepted) => {
      if (done) return;
      done = true;
      openPrompts.delete(promptId);
      invoke('auth_respond_host_key', { promptId, accepted }).catch(() => {});
      if (handle) handle.close();
    };
    trackPrompt(promptId, () => {
      done = true;
      if (handle) handle.close();
    });

    handle = global.tlDialog.open({
      title: 'SSH Host Key Verification',
      ariaLabel: 'SSH host key verification',
      size: 'md',
      body: (bodyEl) => {
        bodyEl.innerHTML = `
          <div class="ssh-auth-message">${esc(message)}</div>
          <pre class="ssh-auth-detail">${esc(detail)}</pre>
          <div class="ssh-auth-question">Do you want to continue connecting and save this key?</div>
        `;
      },
      buttons: [
        { label: 'Reject', onSelect: () => respond(false) },
        { label: 'Accept & Save', primary: true, onSelect: () => respond(true) },
      ],
      // Escape/backdrop dismiss = reject, same as the old overlay.
      onClose: () => respond(false),
      // The old overlay-level key handler treated Enter as "accept"
      // regardless of which element had focus (document-level capture, so
      // it fired before a focused button's native Enter-triggers-click
      // could). This body has no input to focus, so the shell's default
      // focus lands on the first footer button ("Reject") — a plain
      // body-scoped bubble listener would miss Enter entirely there, since
      // the footer isn't a descendant of the body, and "Reject" would fire
      // instead. Listening on the fully-built panel reproduces "always
      // accept on Enter, whichever element has focus".
      onOpen: (panelEl) => {
        panelEl.addEventListener('keydown', (keyEvent) => {
          if (keyEvent.key !== 'Enter') return;
          keyEvent.preventDefault();
          respond(true);
        });
      },
    });
    return true;
  }

  function showPasswordPrompt(event, deps) {
    const d = deps || {};
    const payload = event && event.payload ? event.payload : {};
    const promptId = payload.prompt_id;
    const message = payload.message;
    if (!promptId) return false;

    const esc = typeof d.esc === 'function' ? d.esc : (value) => String(value == null ? '' : value);
    const invoke = typeof d.invoke === 'function' ? d.invoke : null;
    if (!invoke) return false;
    if (!global.tlDialog || typeof global.tlDialog.open !== 'function') return false;

    let handle = null;
    let done = false;
    const respond = (password) => {
      if (done) return;
      done = true;
      openPrompts.delete(promptId);
      invoke('auth_respond_password', { promptId, password }).catch(() => {});
      if (handle) handle.close();
    };
    trackPrompt(promptId, () => {
      done = true;
      if (handle) handle.close();
    });

    handle = global.tlDialog.open({
      title: 'SSH Authentication',
      ariaLabel: 'SSH authentication',
      size: 'sm',
      body: (bodyEl) => {
        bodyEl.innerHTML = `
          <div class="ssh-auth-message">${esc(message)}</div>
          <div class="tl-field">
            <span class="tl-field__label">Password</span>
            <input type="password" class="tl-input" id="pw-input" spellcheck="false" autocomplete="off" />
          </div>
        `;
        const input = bodyEl.querySelector('#pw-input');
        if (input) {
          input.addEventListener('keydown', (keyEvent) => {
            if (keyEvent.key !== 'Enter') return;
            respond(input.value || null);
          });
          setTimeout(() => input.focus(), 50);
        }
      },
      buttons: [
        { label: 'Cancel', onSelect: () => respond(null) },
        { label: 'Connect', primary: true, onSelect: () => {
          const input = handle.el.querySelector('#pw-input');
          respond(input ? (input.value || null) : null);
        } },
      ],
      // Escape/backdrop dismiss = cancel (password null), same as the old overlay.
      onClose: () => respond(null),
    });
    return true;
  }

  // Window-wide prompt wiring. The SSH handler blocks on these prompts, so
  // their listeners must exist from boot in every window — registering them
  // only when some panel happens to render leaves an early connection
  // hanging forever with no dialog anywhere.
  function install(deps) {
    const d = deps || {};
    if (typeof d.listen !== 'function' || typeof d.invoke !== 'function') {
      throw new TypeError('SSH auth prompts require listen and invoke');
    }
    if (installed) return;
    installed = true;
    const promptDeps = { invoke: d.invoke, esc: d.esc };
    d.listen('ssh-host-key-prompt', (event) => showHostKeyPrompt(event, promptDeps));
    d.listen('ssh-password-prompt', (event) => showPasswordPrompt(event, promptDeps));
    d.listen(RESOLVED_EVENT, (event) => {
      const payload = event && event.payload ? event.payload : {};
      closeResolvedPrompt(payload.prompt_id);
    });
  }

  global.termlabSshAuthPrompts = {
    install,
    showHostKeyPrompt,
    showPasswordPrompt,
  };
})(window);

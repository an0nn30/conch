(function initTermLabSshAuthPrompts(global) {
  'use strict';

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
      invoke('auth_respond_host_key', { promptId, accepted }).catch(() => {});
      if (handle) handle.close();
    };

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
      invoke('auth_respond_password', { promptId, password }).catch(() => {});
      if (handle) handle.close();
    };

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

  global.termlabSshAuthPrompts = {
    showHostKeyPrompt,
    showPasswordPrompt,
  };
})(window);

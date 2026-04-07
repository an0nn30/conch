(function initConchSshAuthPrompts(global) {
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

    const overlay = document.createElement('div');
    overlay.className = 'ssh-overlay';
    overlay.style.zIndex = '5000';
    if (typeof d.setOverlayDialogAttributes === 'function') {
      d.setOverlayDialogAttributes(overlay, 'SSH host key verification');
    }
    overlay.innerHTML = `
      <div class="ssh-form" style="max-width:520px">
        <div class="ssh-form-title">SSH Host Key Verification</div>
        <div class="ssh-form-body">
          <div class="ssh-auth-message">${esc(message)}</div>
          <pre class="ssh-auth-detail">${esc(detail)}</pre>
          <div class="ssh-auth-question">Do you want to continue connecting and save this key?</div>
        </div>
        <div class="ssh-form-buttons">
          <button class="ssh-form-btn" id="hk-reject">Reject</button>
          <button class="ssh-form-btn primary" id="hk-accept">Accept & Save</button>
        </div>
      </div>
    `;

    document.body.appendChild(overlay);

    let done = false;
    const respond = (accepted) => {
      if (done) return;
      done = true;
      if (typeof unregisterKeys === 'function') unregisterKeys();
      if (overlay.isConnected) overlay.remove();
      invoke('auth_respond_host_key', { promptId, accepted }).catch(() => {});
    };
    const unregisterKeys = typeof d.registerOverlayKeys === 'function'
      ? d.registerOverlayKeys(overlay, 'ssh-host-key-prompt', (keyEvent) => {
        if (keyEvent.key === 'Escape') {
          respond(false);
          return true;
        }
        if (keyEvent.key === 'Enter') {
          respond(true);
          return true;
        }
        return false;
      })
      : null;

    overlay.querySelector('#hk-reject').addEventListener('click', () => respond(false));
    overlay.querySelector('#hk-accept').addEventListener('click', () => respond(true));
    overlay.addEventListener('mousedown', (mouseEvent) => {
      if (mouseEvent.target === overlay) respond(false);
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

    const overlay = document.createElement('div');
    overlay.className = 'ssh-overlay';
    overlay.style.zIndex = '5000';
    if (typeof d.setOverlayDialogAttributes === 'function') {
      d.setOverlayDialogAttributes(overlay, 'SSH authentication');
    }
    overlay.innerHTML = `
      <div class="ssh-form ssh-form-small">
        <div class="ssh-form-title">SSH Authentication</div>
        <div class="ssh-form-body">
          <div class="ssh-auth-message">${esc(message)}</div>
          <label class="ssh-form-label">Password
            <input type="password" id="pw-input" spellcheck="false" autocomplete="off" />
          </label>
        </div>
        <div class="ssh-form-buttons">
          <button class="ssh-form-btn" id="pw-cancel">Cancel</button>
          <button class="ssh-form-btn primary" id="pw-connect">Connect</button>
        </div>
      </div>
    `;

    document.body.appendChild(overlay);
    setTimeout(() => {
      const input = overlay.querySelector('#pw-input');
      if (input) input.focus();
    }, 50);

    let done = false;
    const respond = (password) => {
      if (done) return;
      done = true;
      if (typeof unregisterKeys === 'function') unregisterKeys();
      if (overlay.isConnected) overlay.remove();
      invoke('auth_respond_password', { promptId, password }).catch(() => {});
    };
    const unregisterKeys = typeof d.registerOverlayKeys === 'function'
      ? d.registerOverlayKeys(overlay, 'ssh-password-prompt', (keyEvent) => {
        if (keyEvent.key !== 'Escape') return false;
        respond(null);
        return true;
      })
      : null;

    overlay.querySelector('#pw-cancel').addEventListener('click', () => respond(null));
    overlay.querySelector('#pw-connect').addEventListener('click', () => {
      const input = overlay.querySelector('#pw-input');
      respond(input ? (input.value || null) : null);
    });
    const input = overlay.querySelector('#pw-input');
    if (input) {
      input.addEventListener('keydown', (keyEvent) => {
        if (keyEvent.key !== 'Enter') return;
        respond(input.value || null);
      });
    }
    overlay.addEventListener('mousedown', (mouseEvent) => {
      if (mouseEvent.target === overlay) respond(null);
    });
    return true;
  }

  global.conchSshAuthPrompts = {
    showHostKeyPrompt,
    showPasswordPrompt,
  };
})(window);

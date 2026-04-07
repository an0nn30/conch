(function initConchSshDependencyPrompt(global) {
  'use strict';

  function showDependencyPrompt(missingDependencies, deps) {
    const d = deps || {};
    const esc = typeof d.esc === 'function'
      ? d.esc
      : (value) => String(value == null ? '' : value);
    if (typeof d.setOverlayDialogAttributes !== 'function') return null;
    if (typeof d.registerOverlayKeys !== 'function') return null;

    return new Promise((resolve) => {
      const existing = document.querySelector('.ssh-overlay.dep-prompt');
      if (existing) existing.remove();

      const overlay = document.createElement('div');
      overlay.className = 'ssh-overlay dep-prompt';
      d.setOverlayDialogAttributes(overlay, 'Export dependency servers');

      let listHtml = '';
      for (const dep of missingDependencies || []) {
        const dependencyLabel = `${dep.server.label} (${dep.server.user}@${dep.server.host}:${dep.server.port})`;
        const reasonText = dep.reason === 'proxy_jump'
          ? `${dep.sourceLabel} uses ProxyJump`
          : dep.sourceLabel;
        listHtml += `<div class="ssh-export-item" style="padding:2px 0;">
          <span>${esc(reasonText)}</span>
          <span class="ssh-export-dim">\u2192 ${esc(dependencyLabel)}</span>
        </div>`;
      }

      overlay.innerHTML = `
        <div class="ssh-form" style="min-width:400px;">
          <div class="ssh-form-title">Include Dependency Servers?</div>
          <div class="ssh-form-body">
            <div style="margin-bottom:8px;font-size:12px;color:var(--fg);">
              The following selections depend on server connections that are not in your export:
            </div>
            ${listHtml}
            <div style="margin-top:10px;font-size:11px;color:var(--dim-fg);">
              Without these servers, imported connections may fail on another machine.
            </div>
          </div>
          <div class="ssh-form-buttons">
            <button class="ssh-form-btn" id="dep-cancel">Cancel</button>
            <button class="ssh-form-btn" id="dep-skip">Export Without</button>
            <button class="ssh-form-btn primary" id="dep-include">Include Servers</button>
          </div>
        </div>
      `;

      document.body.appendChild(overlay);

      let done = false;
      const finish = (result) => {
        if (done) return;
        done = true;
        if (typeof unregisterKeys === 'function') unregisterKeys();
        if (overlay.isConnected) overlay.remove();
        resolve(result);
      };

      const unregisterKeys = d.registerOverlayKeys(overlay, 'ssh-export-dependency-dialog', (event) => {
        if (event.key !== 'Escape') return false;
        finish(null);
        return true;
      });

      overlay.addEventListener('mousedown', (event) => {
        if (event.target === overlay) finish(null);
      });
      overlay.querySelector('#dep-cancel').addEventListener('click', () => finish(null));
      overlay.querySelector('#dep-skip').addEventListener('click', () => finish(false));
      overlay.querySelector('#dep-include').addEventListener('click', () => finish(true));
    });
  }

  global.conchSshDependencyPrompt = {
    showDependencyPrompt,
  };
})(window);

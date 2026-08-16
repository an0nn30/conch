(function initTermLabSshDependencyPrompt(global) {
  'use strict';

  // Only one dependency prompt makes sense at a time (it's always raised
  // from inside the export dialog's Export click handler); track the
  // in-flight handle locally instead of querying the DOM for a stale
  // .ssh-overlay.dep-prompt node, which no longer exists once this dialog
  // renders through the tl-dialog shell.
  let activeHandle = null;

  function showDependencyPrompt(missingDependencies, deps) {
    const d = deps || {};
    const esc = typeof d.esc === 'function'
      ? d.esc
      : (value) => String(value == null ? '' : value);
    if (!global.tlDialog || typeof global.tlDialog.open !== 'function') return null;

    return new Promise((resolve) => {
      if (activeHandle) {
        activeHandle.close();
        activeHandle = null;
      }

      let listHtml = '';
      for (const dep of missingDependencies || []) {
        const dependencyLabel = `${dep.server.label} (${dep.server.user}@${dep.server.host}:${dep.server.port})`;
        const reasonText = dep.reason === 'proxy_jump'
          ? `${dep.sourceLabel} uses ProxyJump`
          : dep.sourceLabel;
        listHtml += `<div class="ssh-export-item" style="padding:2px 0;">
          <span>${esc(reasonText)}</span>
          <span class="ssh-export-dim">→ ${esc(dependencyLabel)}</span>
        </div>`;
      }

      let done = false;
      let handle = null;
      // Buttons resolve through here (and close the dialog themselves);
      // onClose is only the fallback for Escape/backdrop-dismiss, which
      // must resolve null ("cancelled") same as the old code — not
      // whatever internal close-reason string tl-dialog passes it.
      const finish = (result) => {
        if (done) return;
        done = true;
        activeHandle = null;
        resolve(result);
        if (handle) handle.close();
      };

      handle = global.tlDialog.open({
        title: 'Include Dependency Servers?',
        ariaLabel: 'Export dependency servers',
        size: 'md',
        body: (bodyEl) => {
          bodyEl.innerHTML = `
            <div style="margin-bottom:8px;font-size:12px;color:var(--fg);">
              The following selections depend on server connections that are not in your export:
            </div>
            ${listHtml}
            <div style="margin-top:10px;font-size:11px;color:var(--dim-fg);">
              Without these servers, imported connections may fail on another machine.
            </div>
          `;
        },
        buttons: [
          { label: 'Cancel', onSelect: () => finish(null) },
          { label: 'Export Without', onSelect: () => finish(false) },
          { label: 'Include Servers', primary: true, onSelect: () => finish(true) },
        ],
        onClose: () => finish(null),
      });
      activeHandle = handle;
    });
  }

  global.termlabSshDependencyPrompt = {
    showDependencyPrompt,
  };
})(window);

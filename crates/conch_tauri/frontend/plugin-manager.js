// Plugin Manager — scan, enable/disable plugins.

(function (exports) {
  'use strict';

  let invoke = null;

  function init(opts) {
    invoke = opts.invoke;
  }

  async function show() {
    removeOverlay();

    let plugins = [];
    try {
      plugins = await invoke('scan_plugins');
    } catch (e) {
      if (window.toast) window.toast.error('Plugin Scan Failed', String(e));
      return;
    }

    const overlay = document.createElement('div');
    overlay.className = 'ssh-overlay';
    overlay.id = 'plugin-manager-overlay';

    overlay.innerHTML = `
      <div class="ssh-form plugin-manager-dialog">
        <div class="ssh-form-title">Plugin Manager</div>
        <div class="pm-body">
          <table class="pm-table">
            <thead>
              <tr>
                <th>Status</th>
                <th>Name</th>
                <th>Version</th>
                <th>Type</th>
                <th>Source</th>
                <th class="pm-th-path">Path</th>
                <th></th>
              </tr>
            </thead>
            <tbody id="pm-tbody"></tbody>
          </table>
          ${plugins.length === 0 ? '<div class="pm-empty">No plugins found in search paths</div>' : ''}
        </div>
        <div class="ssh-form-buttons">
          <button class="ssh-form-btn" id="pm-rescan">Rescan</button>
          <button class="ssh-form-btn" id="pm-close">Close</button>
        </div>
      </div>
    `;

    document.body.appendChild(overlay);

    const tbody = overlay.querySelector('#pm-tbody');
    for (const p of plugins) {
      tbody.appendChild(createPluginRow(p));
    }

    overlay.addEventListener('mousedown', (e) => { if (e.target === overlay) removeOverlay(); });
    overlay.querySelector('#pm-close').addEventListener('click', removeOverlay);
    overlay.querySelector('#pm-rescan').addEventListener('click', () => show());

    const onKey = (e) => {
      if (e.key === 'Escape') { removeOverlay(); document.removeEventListener('keydown', onKey); }
    };
    document.addEventListener('keydown', onKey);
  }

  function createPluginRow(plugin) {
    const tr = document.createElement('tr');
    tr.className = 'pm-row';

    const statusDot = plugin.loaded
      ? '<span class="tunnel-dot active"></span> Loaded'
      : '<span class="tunnel-dot inactive"></span> Stopped';

    tr.innerHTML =
      `<td class="pm-col-status">${statusDot}</td>` +
      `<td class="pm-col-name">${esc(plugin.name)}</td>` +
      `<td class="pm-col-version">${esc(plugin.version)}</td>` +
      `<td>${esc(plugin.plugin_type)}</td>` +
      `<td>${esc(plugin.source)}</td>` +
      `<td class="pm-col-path" title="${attr(plugin.path)}">${esc(shortPath(plugin.path))}</td>`;

    const actionTd = document.createElement('td');
    actionTd.className = 'pm-col-action';

    if (plugin.loaded) {
      const btn = document.createElement('button');
      btn.className = 'tunnel-action-btn';
      btn.textContent = 'Disable';
      btn.addEventListener('click', async (e) => {
        e.stopPropagation();
        btn.disabled = true;
        try {
          await invoke('disable_plugin', { name: plugin.name, source: plugin.source });
          await invoke('rebuild_menu').catch(() => {});
          if (window.toast) window.toast.info('Plugin Disabled', plugin.name);
        } catch (err) {
          if (window.toast) window.toast.error('Disable Failed', String(err));
        }
        show();
      });
      actionTd.appendChild(btn);
    } else {
      const btn = document.createElement('button');
      btn.className = 'tunnel-action-btn';
      btn.textContent = 'Enable';
      btn.addEventListener('click', async (e) => {
        e.stopPropagation();
        btn.disabled = true;
        try {
          await invoke('enable_plugin', { name: plugin.name, source: plugin.source, path: plugin.path });
          // Rebuild the native menu to include any menu items the plugin registered.
          await invoke('rebuild_menu').catch(() => {});
          if (window.toast) window.toast.success('Plugin Enabled', plugin.name);
        } catch (err) {
          if (window.toast) window.toast.error('Enable Failed', String(err));
        }
        show();
      });
      actionTd.appendChild(btn);
    }

    tr.appendChild(actionTd);
    return tr;
  }

  function removeOverlay() {
    const el = document.getElementById('plugin-manager-overlay');
    if (el) el.remove();
  }

  function shortPath(p) {
    const parts = p.split('/');
    if (parts.length <= 3) return p;
    return '.../' + parts.slice(-2).join('/');
  }

  function esc(str) {
    const el = document.createElement('span');
    el.textContent = str || '';
    return el.innerHTML;
  }

  function attr(str) {
    return String(str || '').replace(/"/g, '&quot;');
  }

  exports.pluginManager = { init, show };
})(window);

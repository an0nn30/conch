// Plugin Widget Renderer — converts Widget JSON trees to HTML.
// Also handles widget interaction events back to the backend.

(function (exports) {
  'use strict';

  let invoke = null;
  let listen = null;
  const pluginMenuItems = [];
  const dockedViewRefreshTimers = new Map();
  // Tracks plugins whose dialog was recently dismissed to reject queued duplicates.
  const _dialogCooldown = new Set();
  // Tracks handles for panels registered at the bottom location.
  // Maps handle (number) → plugin name (string).
  const bottomPanelHandles = new Map();

  function log(msg) { console.log('[plugin-widgets] ' + msg); }

  function init(opts) {
    invoke = opts.invoke;
    listen = opts.listen;

    // Track panel registrations so we know which handles belong to bottom panels.
    listen('plugin-panel-registered', (event) => {
      const { handle, plugin, name, location } = event.payload;
      if (location === 'bottom') {
        bottomPanelHandles.set(handle, plugin);
        if (window.notificationPanel) {
          window.notificationPanel.addPluginTab(
            'plugin-' + plugin,
            name || plugin,
            (container) => {
              renderWidgets(container, '[]', plugin);
            }
          );
        }
      }
    });

    // Listen for plugin panel removal (batch event) and clean up bottom panel tabs.
    listen('plugin-panels-removed', (event) => {
      const { plugin, handles } = event.payload;
      for (const handle of handles) {
        if (bottomPanelHandles.has(handle)) {
          bottomPanelHandles.delete(handle);
        }
      }
      if (window.notificationPanel) {
        window.notificationPanel.removePluginTab('plugin-' + plugin);
      }
    });

    // Listen for widget updates from plugins.
    listen('plugin-widgets-updated', (event) => {
      const { handle, plugin, widgets_json } = event.payload;
      if (bottomPanelHandles.has(handle)) {
        // Route bottom-panel plugin widgets to the notification panel tab system.
        if (window.notificationPanel) {
          window.notificationPanel.updatePluginTab(
            'plugin-' + plugin,
            (container) => {
              renderWidgets(container, widgets_json, plugin);
            }
          );
        }
        return;
      }
      const container = document.querySelector(`[data-plugin-handle="${handle}"]`);
      if (container) {
        renderWidgets(container, widgets_json, plugin);
      }
    });

    // Listen for plugin menu item registrations → store and add to Tools menu area.
    listen('plugin-menu-item', (event) => {
      const item = event.payload;
      if (!item || !item.plugin || !item.action) return;
      pluginMenuItems.push(item);
      // Emit a custom DOM event so the menu-action handler can pick it up.
      log('Plugin registered menu item: ' + item.label + ' (' + item.plugin + ')');
    });

    // Listen for plugin dialog requests.
    listen('plugin-form-dialog', handleFormDialog);
    listen('plugin-prompt-dialog', handlePromptDialog);
    listen('plugin-confirm-dialog', handleConfirmDialog);

    // Listen for plugin notifications → route to toast system.
    listen('plugin-notification', (event) => {
      const { plugin, json } = event.payload;
      try {
        const data = JSON.parse(json);
        const level = data.level || 'info';
        const title = data.title || plugin;
        const body = data.body || '';
        if (window.toast) window.toast[level === 'error' ? 'error' : level === 'warn' ? 'warn' : level === 'success' ? 'success' : 'info'](title, body);
      } catch (_) {}
    });

    // Listen for write-to-pty events from plugins.
    listen('plugin-write-pty', (event) => {
      if (opts.writeToActivePty) opts.writeToActivePty(event.payload);
    });
  }

  // ---------------------------------------------------------------------------
  // Widget rendering
  // ---------------------------------------------------------------------------

  function renderWidgets(container, widgetsJson, pluginName, viewId) {
    let widgets;
    try {
      widgets = typeof widgetsJson === 'string' ? JSON.parse(widgetsJson) : widgetsJson;
    } catch (e) {
      container.innerHTML = '<div class="pw-error">Invalid widget JSON</div>';
      return;
    }

    if (!Array.isArray(widgets)) widgets = [widgets];

    const frag = document.createDocumentFragment();
    for (const w of widgets) {
      const el = renderWidget(w, pluginName, viewId);
      if (el) frag.appendChild(el);
    }
    container.innerHTML = '';
    container.appendChild(frag);
  }

  function renderWidget(w, pluginName, viewId) {
    if (!w || !w.type) return null;

    switch (w.type) {
      case 'heading': return renderHeading(w);
      case 'label': return renderLabel(w);
      case 'text': return renderText(w);
      case 'scroll_text': return renderScrollText(w);
      case 'key_value': return renderKeyValue(w);
      case 'separator': return renderSeparator();
      case 'spacer': return renderSpacer(w);
      case 'icon_label': return renderIconLabel(w);
      case 'badge': return renderBadge(w);
      case 'progress': return renderProgress(w);
      case 'button': return renderButton(w, pluginName, viewId);
      case 'text_input': return renderTextInput(w, pluginName, viewId);
      case 'text_edit': return renderTextEdit(w, pluginName, viewId);
      case 'checkbox': return renderCheckbox(w, pluginName, viewId);
      case 'combo_box': return renderComboBox(w, pluginName, viewId);
      case 'toolbar': return renderToolbar(w, pluginName, viewId);
      case 'tree_view': return renderTreeView(w, pluginName, viewId);
      case 'table': return renderTable(w, pluginName, viewId);
      case 'horizontal': return renderHorizontal(w, pluginName, viewId);
      case 'vertical': return renderVertical(w, pluginName, viewId);
      case 'scroll_area': return renderScrollArea(w, pluginName, viewId);
      case 'tabs': return renderTabs(w, pluginName, viewId);
      case 'html': return renderHtmlWidget(w, pluginName, viewId);
      default:
        const el = document.createElement('div');
        el.className = 'pw-unknown';
        el.textContent = `[unknown widget: ${w.type}]`;
        return el;
    }
  }

  // -- Layout --

  function renderHorizontal(w, pn, viewId) {
    const el = document.createElement('div');
    el.className = 'pw-horizontal';
    if (w.spacing) el.style.gap = w.spacing + 'px';
    if (w.centered) el.style.justifyContent = 'center';
    for (const child of (w.children || [])) {
      const c = renderWidget(child, pn, viewId);
      if (c) el.appendChild(c);
    }
    return el;
  }

  function renderVertical(w, pn, viewId) {
    const el = document.createElement('div');
    el.className = 'pw-vertical';
    if (w.spacing) el.style.gap = w.spacing + 'px';
    for (const child of (w.children || [])) {
      const c = renderWidget(child, pn, viewId);
      if (c) el.appendChild(c);
    }
    return el;
  }

  function renderScrollArea(w, pn, viewId) {
    const el = document.createElement('div');
    el.className = 'pw-scroll-area';
    if (w.max_height) el.style.maxHeight = w.max_height + 'px';
    for (const child of (w.children || [])) {
      const c = renderWidget(child, pn, viewId);
      if (c) el.appendChild(c);
    }
    return el;
  }

  function renderTabs(w, pn, viewId) {
    const el = document.createElement('div');
    el.className = 'pw-tabs';
    const bar = document.createElement('div');
    bar.className = 'pw-tabs-bar';
    const content = document.createElement('div');
    content.className = 'pw-tabs-content';

    (w.tabs || []).forEach((tab, i) => {
      const btn = document.createElement('button');
      btn.className = 'pw-tab-btn' + (i === w.active ? ' active' : '');
      btn.textContent = tab.label;
      btn.addEventListener('click', () => {
        sendEvent(pn, { type: 'tab_changed', id: w.id, active: i }, viewId);
      });
      bar.appendChild(btn);

      if (i === w.active) {
        for (const child of (tab.children || [])) {
          const c = renderWidget(child, pn, viewId);
          if (c) content.appendChild(c);
        }
      }
    });

    el.appendChild(bar);
    el.appendChild(content);
    return el;
  }

  // -- Data Display --

  function renderHeading(w) {
    const el = document.createElement('h3');
    el.className = 'pw-heading';
    el.textContent = w.text;
    return el;
  }

  function renderLabel(w) {
    const el = document.createElement('span');
    el.className = 'pw-label' + (w.style ? ' pw-style-' + w.style : '');
    el.textContent = w.text;
    return el;
  }

  function renderText(w) {
    const el = document.createElement('pre');
    el.className = 'pw-text';
    el.textContent = w.text;
    return el;
  }

  function renderScrollText(w) {
    const el = document.createElement('pre');
    el.className = 'pw-scroll-text';
    if (w.max_height) el.style.maxHeight = w.max_height + 'px';
    el.textContent = w.text;
    // Auto-scroll to bottom.
    requestAnimationFrame(() => { el.scrollTop = el.scrollHeight; });
    return el;
  }

  function renderKeyValue(w) {
    const el = document.createElement('div');
    el.className = 'pw-kv';
    el.innerHTML = `<span class="pw-kv-key">${esc(w.key)}</span><span class="pw-kv-value">${esc(w.value)}</span>`;
    return el;
  }

  function renderSeparator() {
    const el = document.createElement('hr');
    el.className = 'pw-separator';
    return el;
  }

  function renderSpacer(w) {
    const el = document.createElement('div');
    el.className = 'pw-spacer';
    if (w.size) el.style.height = w.size + 'px';
    else el.style.flex = '1';
    return el;
  }

  function renderIconLabel(w) {
    const el = document.createElement('span');
    el.className = 'pw-icon-label' + (w.style ? ' pw-style-' + w.style : '');
    if (w.icon) el.innerHTML = iconHtml(w.icon, 14) + esc(w.text);
    else el.textContent = w.text;
    return el;
  }

  function renderBadge(w) {
    const el = document.createElement('span');
    el.className = 'pw-badge pw-badge-' + (w.variant || 'info');
    el.textContent = w.text;
    return el;
  }

  function renderProgress(w) {
    const el = document.createElement('div');
    el.className = 'pw-progress';
    const pct = Math.round((w.fraction || 0) * 100);
    el.innerHTML = `<div class="pw-progress-bar" style="width:${pct}%"></div>`;
    if (w.label) {
      const lbl = document.createElement('span');
      lbl.className = 'pw-progress-label';
      lbl.textContent = w.label;
      el.appendChild(lbl);
    }
    return el;
  }

  // -- Interactive --

  function renderButton(w, pn, viewId) {
    const el = document.createElement('button');
    el.className = 'pw-button';
    if (w.icon) el.innerHTML = iconHtml(w.icon, 14) + esc(w.label);
    else el.textContent = w.label;
    if (w.enabled === false) el.disabled = true;
    el.addEventListener('click', () => sendEvent(pn, { type: 'button_click', id: w.id }, viewId));
    return el;
  }

  function renderTextInput(w, pn, viewId) {
    const el = document.createElement('input');
    el.className = 'pw-text-input';
    el.type = 'text';
    el.value = w.value || '';
    if (w.hint) el.placeholder = w.hint;
    el.spellcheck = false;
    let debounce = null;
    el.addEventListener('input', () => {
      clearTimeout(debounce);
      debounce = setTimeout(() => {
        sendEvent(pn, { type: 'text_input_changed', id: w.id, value: el.value }, viewId);
      }, 200);
    });
    el.addEventListener('keydown', (e) => {
      if (e.key === 'Enter') sendEvent(pn, { type: 'text_input_submit', id: w.id, value: el.value }, viewId);
      if (e.key === 'ArrowDown') sendEvent(pn, { type: 'text_input_arrow_down', id: w.id }, viewId);
      if (e.key === 'ArrowUp') sendEvent(pn, { type: 'text_input_arrow_up', id: w.id }, viewId);
    });
    if (w.request_focus) setTimeout(() => el.focus(), 50);
    return el;
  }

  function renderTextEdit(w, pn, viewId) {
    const el = document.createElement('textarea');
    el.className = 'pw-text-edit';
    el.value = w.value || '';
    if (w.hint) el.placeholder = w.hint;
    if (w.lines) el.rows = w.lines;
    el.addEventListener('input', () => {
      sendEvent(pn, { type: 'text_edit_changed', id: w.id, value: el.value }, viewId);
    });
    return el;
  }

  function renderCheckbox(w, pn, viewId) {
    const el = document.createElement('label');
    el.className = 'pw-checkbox';
    const input = document.createElement('input');
    input.type = 'checkbox';
    input.checked = w.checked;
    input.addEventListener('change', () => {
      sendEvent(pn, { type: 'checkbox_changed', id: w.id, checked: input.checked }, viewId);
    });
    el.appendChild(input);
    el.appendChild(document.createTextNode(' ' + w.label));
    return el;
  }

  function renderComboBox(w, pn, viewId) {
    const el = document.createElement('select');
    el.className = 'pw-combo-box';
    for (const opt of (w.options || [])) {
      const o = document.createElement('option');
      o.value = opt.value;
      o.textContent = opt.label;
      if (opt.value === w.selected) o.selected = true;
      el.appendChild(o);
    }
    el.addEventListener('change', () => {
      sendEvent(pn, { type: 'combo_box_changed', id: w.id, value: el.value }, viewId);
    });
    return el;
  }

  // -- Toolbar --

  function renderToolbar(w, pn, viewId) {
    const el = document.createElement('div');
    el.className = 'pw-toolbar';
    for (const item of (w.items || [])) {
      if (item.type === 'separator') {
        const sep = document.createElement('div');
        sep.className = 'pw-toolbar-sep';
        el.appendChild(sep);
      } else if (item.type === 'spacer') {
        const sp = document.createElement('div');
        sp.className = 'pw-toolbar-spacer';
        el.appendChild(sp);
      } else if (item.type === 'button') {
        const btn = document.createElement('button');
        btn.className = 'pw-toolbar-btn';
        btn.textContent = item.label || '';
        if (item.tooltip) btn.title = item.tooltip;
        if (item.enabled === false) btn.disabled = true;
        btn.addEventListener('click', () => sendEvent(pn, { type: 'button_click', id: item.id }, viewId));
        el.appendChild(btn);
      } else if (item.type === 'text_input') {
        const input = document.createElement('input');
        input.className = 'pw-toolbar-input';
        input.type = 'text';
        input.value = item.value || '';
        if (item.hint) input.placeholder = item.hint;
        input.addEventListener('keydown', (e) => {
          if (e.key === 'Enter') sendEvent(pn, { type: 'toolbar_input_submit', id: item.id, value: input.value }, viewId);
        });
        el.appendChild(input);
      }
    }
    return el;
  }

  // -- Tree View --

  function renderTreeView(w, pn, viewId) {
    const el = document.createElement('div');
    el.className = 'pw-tree';
    for (const node of (w.nodes || [])) {
      el.appendChild(renderTreeNode(node, w.id, w.selected, pn, viewId));
    }
    return el;
  }

  function renderTreeNode(node, treeId, selectedId, pn, viewId) {
    const el = document.createElement('div');
    el.className = 'pw-tree-node';

    const row = document.createElement('div');
    row.className = 'pw-tree-row' + (node.id === selectedId ? ' selected' : '');
    if (node.bold) row.classList.add('bold');

    const hasChildren = node.children && node.children.length > 0;
    const expanded = node.expanded !== false;

    if (hasChildren) {
      const arrow = document.createElement('span');
      arrow.className = 'pw-tree-arrow';
      arrow.textContent = expanded ? '▼' : '▶';
      arrow.addEventListener('click', (e) => {
        e.stopPropagation();
        sendEvent(pn, { type: 'tree_toggle', id: treeId, node_id: node.id, expanded: !expanded }, viewId);
      });
      row.appendChild(arrow);
    } else {
      const sp = document.createElement('span');
      sp.className = 'pw-tree-arrow-placeholder';
      row.appendChild(sp);
    }

    if (node.icon) {
      const iconEl = document.createElement('span');
      iconEl.innerHTML = iconHtml(node.icon, 14);
      row.appendChild(iconEl);
    }

    const label = document.createElement('span');
    label.className = 'pw-tree-label';
    label.textContent = node.label;
    row.appendChild(label);

    if (node.badge) {
      const badge = document.createElement('span');
      badge.className = 'pw-tree-badge';
      badge.textContent = node.badge;
      row.appendChild(badge);
    }

    row.addEventListener('click', () => {
      sendEvent(pn, { type: 'tree_select', id: treeId, node_id: node.id }, viewId);
    });
    row.addEventListener('dblclick', () => {
      sendEvent(pn, { type: 'tree_activate', id: treeId, node_id: node.id }, viewId);
    });

    el.appendChild(row);

    if (hasChildren && expanded) {
      const childContainer = document.createElement('div');
      childContainer.className = 'pw-tree-children';
      for (const child of node.children) {
        childContainer.appendChild(renderTreeNode(child, treeId, selectedId, pn, viewId));
      }
      el.appendChild(childContainer);
    }

    return el;
  }

  // -- Table --

  function renderTable(w, pn, viewId) {
    const el = document.createElement('div');
    el.className = 'pw-table-wrap';

    const table = document.createElement('table');
    table.className = 'pw-table';

    // Header
    const thead = document.createElement('thead');
    const headerRow = document.createElement('tr');
    for (const col of (w.columns || [])) {
      if (col.visible === false) continue;
      const th = document.createElement('th');
      th.textContent = col.label;
      if (col.width) th.style.width = col.width + 'px';
      if (col.sortable) {
        th.style.cursor = 'pointer';
        if (w.sort_column === col.id) {
          th.textContent += w.sort_ascending ? ' \u25B4' : ' \u25BE';
        }
        th.addEventListener('click', () => {
          const asc = w.sort_column === col.id ? !w.sort_ascending : true;
          sendEvent(pn, { type: 'table_sort', id: w.id, column: col.id, ascending: asc }, viewId);
        });
      }
      headerRow.appendChild(th);
    }
    thead.appendChild(headerRow);
    table.appendChild(thead);

    // Body
    const tbody = document.createElement('tbody');
    for (const row of (w.rows || [])) {
      const tr = document.createElement('tr');
      tr.className = 'pw-table-row' + (row.id === w.selected_row ? ' selected' : '');
      for (let i = 0; i < (w.columns || []).length; i++) {
        const col = w.columns[i];
        if (col.visible === false) continue;
        const cell = row.cells[i];
        const td = document.createElement('td');
        if (typeof cell === 'string') {
          td.textContent = cell;
        } else if (cell && typeof cell === 'object') {
          if (cell.icon) td.innerHTML = iconHtml(cell.icon, 14) + esc(cell.text || '');
          else td.textContent = cell.text || '';
        }
        tr.appendChild(td);
      }
      tr.addEventListener('click', () => {
        sendEvent(pn, { type: 'table_select', id: w.id, row_id: row.id }, viewId);
      });
      tr.addEventListener('dblclick', () => {
        sendEvent(pn, { type: 'table_activate', id: w.id, row_id: row.id }, viewId);
      });
      tbody.appendChild(tr);
    }
    table.appendChild(tbody);
    el.appendChild(table);
    return el;
  }

  // ---------------------------------------------------------------------------
  // Event dispatch
  // ---------------------------------------------------------------------------

  function refreshDockedView(pluginName, viewId) {
    if (!invoke || !pluginName || !viewId) return;
    const key = pluginName + '::' + viewId;
    if (dockedViewRefreshTimers.has(key)) return;

    const eventType = widgetEventTypeByViewKey.get(key);
    const delayMs = eventType === 'text_input_changed' || eventType === 'text_edit_changed' ? 180 : 0;
    const timer = setTimeout(async () => {
      dockedViewRefreshTimers.delete(key);
      const container = document.querySelector(`.plugin-panel-content[data-plugin-view-id="${viewId}"]`);
      if (!container) return;
      try {
        const result = await invoke('request_plugin_view_render', { pluginName, viewId });
        if (result != null) renderWidgets(container, result, pluginName, viewId);
      } catch (e) {
        console.error('request_plugin_view_render error:', e);
      } finally {
        widgetEventTypeByViewKey.delete(key);
      }
    }, delayMs);

    dockedViewRefreshTimers.set(key, timer);
  }

  const widgetEventTypeByViewKey = new Map();

  function sendEvent(pluginName, widgetEvent, viewId) {
    if (!invoke || !pluginName) return;
    const payload = { kind: 'widget', ...widgetEvent };
    if (viewId) payload.view_id = viewId;
    const eventJson = JSON.stringify(payload);
    invoke('plugin_widget_event', { pluginName, eventJson })
      .then(() => {
        if (viewId) {
          const key = pluginName + '::' + viewId;
          widgetEventTypeByViewKey.set(key, widgetEvent && widgetEvent.type ? widgetEvent.type : '');
          refreshDockedView(pluginName, viewId);
        } else {
          refreshPanelPlugin(pluginName);
        }
      })
      .catch((e) => {
        console.error('plugin_widget_event error:', e);
      });
  }

  /** Re-render a panel plugin by requesting fresh widgets from the backend. */
  async function refreshPanelPlugin(pluginName) {
    const container = document.querySelector(`.plugin-panel-content[data-plugin-name="${CSS.escape(pluginName)}"]`);
    if (!container) return;
    try {
      const result = await invoke('request_plugin_render', { pluginName });
      if (result != null) renderWidgets(container, result, pluginName);
    } catch (e) {
      console.error('refreshPanelPlugin error:', e);
    }
  }

  // ---------------------------------------------------------------------------
  // Helpers
  // ---------------------------------------------------------------------------

  const esc = window.utils.esc;
  const attr = window.utils.attr;

  // ---------------------------------------------------------------------------
  // HTML widget (Shadow DOM)
  // ---------------------------------------------------------------------------

  // CSS custom properties forwarded into each shadow root.
  const _themeProps = [
    '--bg', '--fg', '--dim-fg', '--panel-bg', '--tab-bar-bg', '--tab-border',
    '--active-highlight', '--red', '--green', '--yellow', '--blue', '--cyan',
    '--magenta', '--input-bg', '--hover-bg', '--text-secondary', '--text-muted',
    '--ui-font-small', '--ui-font-list', '--ui-font-normal',
  ];

  function renderHtmlWidget(w, pluginName, viewId) {
    const host = document.createElement('div');
    host.className = 'pw-html-host';
    const shadow = host.attachShadow({ mode: 'open' });

    // Inherit theme variables from the document root.
    const rootStyle = getComputedStyle(document.documentElement);
    let vars = ':host {';
    for (const p of _themeProps) {
      const v = rootStyle.getPropertyValue(p).trim();
      if (v) vars += ` ${p}: ${v};`;
    }
    vars += ' font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif;';
    vars += ' font-size: var(--ui-font-normal, 14px);';
    vars += ' color: var(--fg); }';

    const style = document.createElement('style');
    style.textContent = vars + '\n' + (w.css || '');
    shadow.appendChild(style);

    const container = document.createElement('div');
    container.innerHTML = w.content;
    shadow.appendChild(container);

    // Wire up data-action click events.
    shadow.addEventListener('click', (e) => {
      const actionEl = e.target.closest('[data-action]');
      if (actionEl) {
        const action = actionEl.getAttribute('data-action');
        sendEvent(pluginName, { type: 'button_click', id: action }, viewId);
      }
    });

    return host;
  }

  // ---------------------------------------------------------------------------
  // Plugin dialogs
  // ---------------------------------------------------------------------------

  function handleFormDialog(event) {
    const { prompt_id, json } = event.payload;
    const pluginName = prompt_id.split('\0')[0];
    if (_dialogCooldown.has(pluginName) ||
        document.querySelector(`.ssh-overlay[data-plugin-dialog="${CSS.escape(pluginName)}"]`)) {
      invoke('dialog_respond_form', { promptId: prompt_id, result: null }).catch(() => {});
      return;
    }
    let desc;
    try { desc = typeof json === 'string' ? JSON.parse(json) : json; } catch (_) { desc = {}; }

    const overlay = document.createElement('div');
    overlay.className = 'ssh-overlay';
    overlay.setAttribute('data-plugin-dialog', pluginName);
    overlay.style.zIndex = '4000';

    const title = desc.title || 'Form';
    const fields = desc.fields || [];
    const buttons = desc.buttons || [{ id: 'cancel', label: 'Cancel' }, { id: 'ok', label: 'OK' }];

    let fieldsHtml = '';
    for (const f of fields) {
      if (f.type === 'separator') { fieldsHtml += '<hr class="pw-separator">'; continue; }
      if (f.type === 'label') { fieldsHtml += `<div class="pw-label">${esc(f.text || '')}</div>`; continue; }
      const label = f.label || f.id || '';
      const hint = f.hint ? ` placeholder="${attr(f.hint)}"` : '';
      const val = f.value != null ? ` value="${attr(String(f.value))}"` : '';
      if (f.type === 'text') {
        fieldsHtml += `<label class="ssh-form-label">${esc(label)}<input type="text" data-field="${attr(f.id)}"${val}${hint} spellcheck="false"></label>`;
      } else if (f.type === 'password') {
        fieldsHtml += `<label class="ssh-form-label">${esc(label)}<input type="password" data-field="${attr(f.id)}"${val}${hint}></label>`;
      } else if (f.type === 'number') {
        fieldsHtml += `<label class="ssh-form-label">${esc(label)}<input type="number" data-field="${attr(f.id)}"${val}></label>`;
      } else if (f.type === 'combo') {
        const opts = (f.options || []).map(o => `<option value="${attr(o)}" ${o === f.value ? 'selected' : ''}>${esc(o)}</option>`).join('');
        fieldsHtml += `<label class="ssh-form-label">${esc(label)}<select data-field="${attr(f.id)}">${opts}</select></label>`;
      } else if (f.type === 'checkbox') {
        const checked = f.value ? 'checked' : '';
        fieldsHtml += `<label class="pw-checkbox"><input type="checkbox" data-field="${attr(f.id)}" ${checked}> ${esc(label)}</label>`;
      } else if (f.type === 'host_port') {
        fieldsHtml += `<div class="ssh-form-row"><label class="ssh-form-label" style="flex:1">${esc(label)}<input type="text" data-field="${attr(f.host_id || 'host')}" value="${attr(f.host_value || '')}" spellcheck="false"></label>`;
        fieldsHtml += `<label class="ssh-form-label" style="width:80px">Port<input type="number" data-field="${attr(f.port_id || 'port')}" value="${attr(f.port_value || '22')}"></label></div>`;
      } else if (f.type === 'file_picker') {
        fieldsHtml += `<label class="ssh-form-label">${esc(label)}<input type="text" data-field="${attr(f.id)}"${val}${hint} spellcheck="false"></label>`;
      }
    }

    let buttonsHtml = '';
    for (const b of buttons) {
      const primary = b.id === 'ok' || b.id === 'save' || b.id === 'save_connect' ? ' primary' : '';
      buttonsHtml += `<button class="ssh-form-btn${primary}" data-action="${attr(b.id)}">${esc(b.label)}</button>`;
    }

    overlay.innerHTML = `<div class="ssh-form"><div class="ssh-form-title">${esc(title)}</div><div class="ssh-form-body">${fieldsHtml}</div><div class="ssh-form-buttons">${buttonsHtml}</div></div>`;
    document.body.appendChild(overlay);

    // Keyboard-first UX: focus the first editable field automatically.
    setTimeout(() => {
      const firstInput = overlay.querySelector(
        '.ssh-form-body input[type="text"], .ssh-form-body input[type="password"], .ssh-form-body input[type="number"], .ssh-form-body select, .ssh-form-body textarea'
      );
      if (firstInput && typeof firstInput.focus === 'function') {
        firstInput.focus();
        if (firstInput.tagName === 'INPUT' && typeof firstInput.select === 'function') {
          firstInput.select();
        }
      }
    }, 30);

    const dismiss = (result) => {
      _dialogCooldown.add(pluginName);
      setTimeout(() => _dialogCooldown.delete(pluginName), 600);
      overlay.remove();
      invoke('dialog_respond_form', { promptId: prompt_id, result }).catch(() => {});
    };

    overlay.addEventListener('mousedown', (e) => { if (e.target === overlay) dismiss(null); });
    overlay.querySelectorAll('.ssh-form-btn').forEach(btn => {
      btn.addEventListener('click', () => {
        const action = btn.dataset.action;
        if (action === 'cancel') { dismiss(null); return; }
        // Collect field values.
        const values = { _action: action };
        overlay.querySelectorAll('[data-field]').forEach(el => {
          const id = el.dataset.field;
          if (el.type === 'checkbox') values[id] = el.checked;
          else values[id] = el.value;
        });
        dismiss(JSON.stringify(values));
      });
    });

    const onKey = (e) => { if (e.key === 'Escape') { e.stopPropagation(); dismiss(null); document.removeEventListener('keydown', onKey, true); } };
    document.addEventListener('keydown', onKey, true);
  }

  function handlePromptDialog(event) {
    const { prompt_id, message, default_value } = event.payload;
    const pluginName = prompt_id.split('\0')[0];
    if (_dialogCooldown.has(pluginName) ||
        document.querySelector(`.ssh-overlay[data-plugin-dialog="${CSS.escape(pluginName)}"]`)) {
      invoke('dialog_respond_prompt', { promptId: prompt_id, value: null }).catch(() => {});
      return;
    }
    const overlay = document.createElement('div');
    overlay.className = 'ssh-overlay';
    overlay.setAttribute('data-plugin-dialog', pluginName);
    overlay.style.zIndex = '4000';
    overlay.innerHTML = `<div class="ssh-form ssh-form-small"><div class="ssh-form-title">Prompt</div><div class="ssh-form-body"><div class="pw-label">${esc(message)}</div><input class="pw-text-input" id="pd-input" type="text" value="${attr(default_value || '')}" spellcheck="false"></div><div class="ssh-form-buttons"><button class="ssh-form-btn" id="pd-cancel">Cancel</button><button class="ssh-form-btn primary" id="pd-ok">OK</button></div></div>`;
    document.body.appendChild(overlay);
    setTimeout(() => overlay.querySelector('#pd-input').focus(), 50);

    const dismiss = (val) => {
      _dialogCooldown.add(pluginName);
      setTimeout(() => _dialogCooldown.delete(pluginName), 600);
      overlay.remove();
      invoke('dialog_respond_prompt', { promptId: prompt_id, value: val }).catch(() => {});
    };

    overlay.querySelector('#pd-cancel').addEventListener('click', () => dismiss(null));
    overlay.querySelector('#pd-ok').addEventListener('click', () => dismiss(overlay.querySelector('#pd-input').value));
    overlay.querySelector('#pd-input').addEventListener('keydown', (e) => { if (e.key === 'Enter') dismiss(overlay.querySelector('#pd-input').value); });
    overlay.addEventListener('mousedown', (e) => { if (e.target === overlay) dismiss(null); });
    const onKey = (e) => { if (e.key === 'Escape') { e.stopPropagation(); dismiss(null); document.removeEventListener('keydown', onKey, true); } };
    document.addEventListener('keydown', onKey, true);
  }

  function handleConfirmDialog(event) {
    const { prompt_id, message } = event.payload;
    const pluginName = prompt_id.split('\0')[0];
    if (_dialogCooldown.has(pluginName) ||
        document.querySelector(`.ssh-overlay[data-plugin-dialog="${CSS.escape(pluginName)}"]`)) {
      invoke('dialog_respond_confirm', { promptId: prompt_id, accepted: false }).catch(() => {});
      return;
    }
    const overlay = document.createElement('div');
    overlay.className = 'ssh-overlay';
    overlay.setAttribute('data-plugin-dialog', pluginName);
    overlay.style.zIndex = '4000';
    overlay.innerHTML = `<div class="ssh-form ssh-form-small"><div class="ssh-form-title">Confirm</div><div class="ssh-form-body"><div class="pw-label">${esc(message)}</div></div><div class="ssh-form-buttons"><button class="ssh-form-btn" id="cd-no">No</button><button class="ssh-form-btn primary" id="cd-yes">Yes</button></div></div>`;
    document.body.appendChild(overlay);

    const dismiss = (val) => {
      _dialogCooldown.add(pluginName);
      setTimeout(() => _dialogCooldown.delete(pluginName), 600);
      overlay.remove();
      invoke('dialog_respond_confirm', { promptId: prompt_id, accepted: val }).catch(() => {});
    };

    overlay.querySelector('#cd-no').addEventListener('click', () => dismiss(false));
    overlay.querySelector('#cd-yes').addEventListener('click', () => dismiss(true));
    overlay.addEventListener('mousedown', (e) => { if (e.target === overlay) dismiss(false); });
    const onKey = (e) => { if (e.key === 'Escape') { e.stopPropagation(); dismiss(false); document.removeEventListener('keydown', onKey, true); } };
    document.addEventListener('keydown', onKey, true);
  }

  /// Map a plugin icon name to an <img> tag using the PNG icon set.
  function iconHtml(name, size) {
    if (!name) return '';
    size = size || 14;
    // Map icon names to filenames (dark variants for dark theme).
    const map = {
      'file': 'file-dark', 'folder': 'folder', 'folder-open': 'folder-open',
      'server': 'server', 'network-server': 'network-server', 'terminal': 'terminal',
      'go-home': 'go-home-dark', 'go-next': 'go-next-dark', 'go-previous': 'go-previous-dark',
      'refresh': 'view-refresh-dark', 'folder-new': 'folder-new-dark',
      'transfer-up': 'transfer-up-dark', 'transfer-down': 'transfer-down-dark',
      'tab-close': 'tab-close-dark', 'computer': 'computer-dark',
      'locked': 'locked-dark', 'unlocked': 'unlocked-dark', 'eye': 'eye-dark',
    };
    const file = map[name] || name;
    return `<img src="icons/${file}.png" width="${size}" height="${size}" style="vertical-align:middle;margin-right:3px">`;
  }

  function getMenuItems() { return pluginMenuItems.slice(); }

  function triggerMenuAction(pluginName, action) {
    if (!invoke) return;
    invoke('trigger_plugin_menu_action', { pluginName, action }).catch((e) => {
      console.error('trigger_plugin_menu_action error:', e);
    });
  }

  exports.pluginWidgets = { init, renderWidgets, getMenuItems, triggerMenuAction };
})(window);

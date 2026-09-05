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
  // Tracks when a plugin last pushed panel widget updates via host events.
  const panelUpdateAt = new Map();

  function log(msg) { console.log('[plugin-widgets] ' + msg); }

  // Duplicate-dialog guard for handleFormDialog/handlePromptDialog/
  // handleConfirmDialog below: each open dialog tags its tl-dialog panel
  // with data-plugin-dialog="<pluginName>" (see the onOpen hooks) so a
  // second prompt from the same plugin while one is already open can be
  // detected without depending on the removed .ssh-overlay class.
  function findOpenPluginDialog(pluginName) {
    return document.querySelector(`[data-plugin-dialog="${CSS.escape(pluginName)}"]`);
  }

  function init(opts) {
    invoke = opts.invoke;
    listen = opts.listen;

    // Listen for widget updates from plugins. Bottom-location panels now
    // register as ordinary tool windows (see tool-window-runtime.js's
    // registerPluginToolWindow), so there is no separate bottom-panel routing
    // here anymore — every plugin panel, regardless of location, renders into
    // a `.plugin-panel-content[data-plugin-name]` container.
    listen('plugin-widgets-updated', (event) => {
      const { handle, plugin, widgets_json } = event.payload;
      if (plugin) panelUpdateAt.set(plugin, Date.now());
      let container = document.querySelector(`[data-plugin-handle="${handle}"]`);
      if (!container && plugin) {
        container = document.querySelector(`.plugin-panel-content[data-plugin-name="${CSS.escape(plugin)}"]`);
      }
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

    // Listen for plugin requests to create a new local tab, then optionally
    // write initial content into the newly focused terminal.
    listen('plugin-new-tab', async (event) => {
      const payload = event.payload || {};
      if (!opts.createTab) return;
      try {
        const createdTabId = await opts.createTab({ plainShell: payload.plain === true });
        if (payload.request_id && invoke) {
          invoke('plugin_respond_new_tab', {
            requestId: payload.request_id,
            tabId: createdTabId != null ? String(createdTabId) : null,
          }).catch(() => {});
        }
        if (payload.tab_title && opts.renameActiveTab) {
          if (payload.request_id && createdTabId != null && opts.renameTabById) {
            opts.renameTabById(String(createdTabId), payload.tab_title);
          } else {
            opts.renameActiveTab(payload.tab_title);
          }
        }
        if (payload.command && opts.writeToActivePty) {
          setTimeout(() => {
            opts.writeToActivePty(payload.command);
          }, 120);
        }
      } catch (e) {
        if (payload.request_id && invoke) {
          invoke('plugin_respond_new_tab', {
            requestId: payload.request_id,
            tabId: null,
          }).catch(() => {});
        }
        console.error('plugin-new-tab error:', e);
      }
    });

    // Listen for plugin requests to rename the active tab.
    listen('plugin-rename-tab', (event) => {
      const payload = event.payload || {};
      const title = typeof payload === 'string' ? payload : payload.title;
      if (!title) return;
      if (payload && payload.tab_id && opts.renameTabById) {
        opts.renameTabById(String(payload.tab_id), title);
        return;
      }
      if (!opts.renameActiveTab) return;
      opts.renameActiveTab(title);
    });

    // Listen for plugin requests to focus an existing tab by id.
    listen('plugin-focus-tab', (event) => {
      const payload = event.payload || {};
      if (!payload || payload.tab_id == null) return;
      if (!opts.focusTabById) return;
      opts.focusTabById(String(payload.tab_id));
    });
  }

  // ---------------------------------------------------------------------------
  // Widget rendering
  // ---------------------------------------------------------------------------

  function renderWidgets(container, widgetsJson, pluginName, viewId) {
    const focusState = captureFocusState(container);
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
    restoreFocusState(container, focusState);
  }

  function captureFocusState(container) {
    const active = document.activeElement;
    if (!active || !container || !container.contains(active)) return null;
    const widgetId = active.getAttribute && active.getAttribute('data-pw-id');
    const widgetKind = active.getAttribute && active.getAttribute('data-pw-kind');
    if (!widgetId || !widgetKind) return null;
    return {
      widgetId,
      widgetKind,
      selectionStart: typeof active.selectionStart === 'number' ? active.selectionStart : null,
      selectionEnd: typeof active.selectionEnd === 'number' ? active.selectionEnd : null,
      scrollTop: typeof active.scrollTop === 'number' ? active.scrollTop : null,
      scrollLeft: typeof active.scrollLeft === 'number' ? active.scrollLeft : null,
    };
  }

  function restoreFocusState(container, focusState) {
    if (!container || !focusState) return;
    const selector = `[data-pw-kind="${CSS.escape(focusState.widgetKind)}"][data-pw-id="${CSS.escape(focusState.widgetId)}"]`;
    const next = container.querySelector(selector);
    if (!next || typeof next.focus !== 'function') return;
    next.focus({ preventScroll: true });
    if (focusState.scrollTop != null) next.scrollTop = focusState.scrollTop;
    if (focusState.scrollLeft != null) next.scrollLeft = focusState.scrollLeft;
    if (
      focusState.selectionStart != null &&
      focusState.selectionEnd != null &&
      typeof next.setSelectionRange === 'function'
    ) {
      const max = typeof next.value === 'string' ? next.value.length : focusState.selectionEnd;
      const start = Math.min(focusState.selectionStart, max);
      const end = Math.min(focusState.selectionEnd, max);
      next.setSelectionRange(start, end);
    }
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
    const wrap = document.createElement('div');
    wrap.className = 'pw-text-input-wrap';
    wrap.setAttribute('data-plugin-setting-id', w.id || '');

    const el = document.createElement('input');
    el.className = 'pw-text-input';
    el.type = 'text';
    el.setAttribute('data-pw-kind', 'text_input');
    el.setAttribute('data-pw-id', w.id || '');
    el.value = w.value || '';
    if (w.hint) el.placeholder = w.hint;
    el.spellcheck = false;
    if (w.enabled === false) {
      el.disabled = true;
      el.setAttribute('aria-disabled', 'true');
    }
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
    if (w.request_focus && !el.disabled) setTimeout(() => el.focus(), 50);
    wrap.appendChild(el);
    return wrap;
  }

  function renderTextEdit(w, pn, viewId) {
    const el = document.createElement('textarea');
    el.className = 'pw-text-edit';
    el.setAttribute('data-pw-kind', 'text_edit');
    el.setAttribute('data-pw-id', w.id || '');
    el.value = w.value || '';
    if (w.hint) el.placeholder = w.hint;
    if (w.lines) el.rows = w.lines;
    el.addEventListener('input', () => {
      sendEvent(pn, { type: 'text_edit_changed', id: w.id, value: el.value }, viewId);
    });
    return el;
  }

  function renderCheckbox(w, pn, viewId) {
    const textLabel = String(w.label || '').trim();
    const el = document.createElement('label');
    const input = document.createElement('input');
    input.type = 'checkbox';
    input.checked = !!w.checked;
    input.addEventListener('change', () => {
      sendEvent(pn, { type: 'checkbox_changed', id: w.id, checked: input.checked }, viewId);
    });

    if (!textLabel) {
      el.className = 'pw-checkbox-switch';
      const slider = document.createElement('span');
      slider.className = 'pw-checkbox-switch-slider';
      el.appendChild(input);
      el.appendChild(slider);
      return el;
    }

    el.className = 'pw-checkbox';
    el.appendChild(input);
    el.appendChild(document.createTextNode(' ' + textLabel));
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

  function sendEvent(pluginName, widgetEvent, viewId, options) {
    if (!invoke || !pluginName) return;
    const skipRefresh = !!(options && options.skipRefresh === true);
    const payload = { kind: 'widget', ...widgetEvent };
    if (viewId) payload.view_id = viewId;
    const eventJson = JSON.stringify(payload);
    const sentAt = Date.now();
    invoke('plugin_widget_event', { pluginName, eventJson })
      .then(() => {
        if (skipRefresh) return;
        if (viewId) {
          refreshPluginView(pluginName, viewId);
          return;
        }
        // Panel views are usually push-updated by plugin host events. To avoid
        // duplicate pull renders (which can cause UI flicker), only fall back
        // to request_plugin_render when no push update arrived after this event.
        setTimeout(() => {
          const lastUpdateAt = panelUpdateAt.get(pluginName) || 0;
          if (lastUpdateAt >= sentAt) return;
          refreshPluginView(pluginName, viewId);
        }, 120);
      })
      .catch((e) => {
        console.error('plugin_widget_event error:', e);
      });
  }

  /** Re-render plugin widgets in whichever host surface dispatched the event. */
  async function refreshPluginView(pluginName, viewId) {
    if (viewId) {
      const selector = `.plugin-settings-content[data-plugin-name="${CSS.escape(pluginName)}"][data-plugin-view-id="${CSS.escape(viewId)}"]`;
      const settingsContainer = document.querySelector(selector);
      if (settingsContainer) {
        try {
          const result = await invoke('request_plugin_render', { pluginName, viewId });
          if (result != null) renderWidgets(settingsContainer, result, pluginName, viewId);
        } catch (e) {
          console.error('refreshPluginView(settings) error:', e);
        }
        return;
      }

      // Docked plugin views render into split-tree pane containers keyed by
      // view id; route view-scoped renders there before the panel fallback.
      const dockedContainer = document.querySelector(
        `.plugin-panel-content[data-plugin-view-id="${CSS.escape(viewId)}"]`
      );
      if (dockedContainer) {
        try {
          const result = await invoke('request_plugin_view_render', { pluginName, viewId });
          if (result != null) renderWidgets(dockedContainer, result, pluginName, viewId);
        } catch (e) {
          console.error('refreshPluginView(docked) error:', e);
        }
        return;
      }
    }

    const panelContainer = document.querySelector(`.plugin-panel-content[data-plugin-name="${CSS.escape(pluginName)}"]`);
    if (!panelContainer) return;
    try {
      const result = await invoke('request_plugin_render', { pluginName });
      if (result != null) renderWidgets(panelContainer, result, pluginName);
    } catch (e) {
      console.error('refreshPluginView(panel) error:', e);
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

    // Plugin HTML is untrusted — see html-sanitizer.js. Never assign it to
    // innerHTML: the shadow root isolates styles, not scripts, and an inline
    // event handler here would run with access to the Tauri invoke bridge.
    const container = document.createElement('div');
    container.appendChild(window.htmlSanitizer.sanitizeToFragment(w.content, document));
    shadow.appendChild(container);

    // Wire up data-action click events.
    shadow.addEventListener('click', (e) => {
      const actionEl = e.target.closest('[data-action]');
      if (actionEl) {
        const action = actionEl.getAttribute('data-action');
        sendEvent(pluginName, { type: 'button_click', id: action }, viewId);
      }
    });

    // Wire up data-dbl-action double-click events.
    shadow.addEventListener('dblclick', (e) => {
      const actionEl = e.target.closest('[data-dbl-action]');
      if (actionEl) {
        const action = actionEl.getAttribute('data-dbl-action');
        sendEvent(pluginName, { type: 'button_click', id: action }, viewId);
      }
    });

    // Wire up right-click actions for plugin HTML UIs.
    shadow.addEventListener('contextmenu', (e) => {
      const actionEl = e.target.closest('[data-context-action]');
      if (!actionEl) return;
      e.preventDefault();
      e.stopPropagation();
      const action = actionEl.getAttribute('data-context-action');
      sendEvent(pluginName, {
        type: 'button_click',
        id: action,
        x: Math.round(e.clientX || 0),
        y: Math.round(e.clientY || 0),
      }, viewId);
    });

    return host;
  }

  // ---------------------------------------------------------------------------
  // Plugin dialogs
  // ---------------------------------------------------------------------------

  function handleFormDialog(event) {
    const { prompt_id, json } = event.payload;
    const pluginName = prompt_id.split('\0')[0];
    if (_dialogCooldown.has(pluginName) || findOpenPluginDialog(pluginName)) {
      invoke('dialog_respond_form', { promptId: prompt_id, result: null }).catch(() => {});
      return;
    }
    if (!window.tlDialog || typeof window.tlDialog.open !== 'function') {
      invoke('dialog_respond_form', { promptId: prompt_id, result: null }).catch(() => {});
      return;
    }
    let desc;
    try { desc = typeof json === 'string' ? JSON.parse(json) : json; } catch (_) { desc = {}; }

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
        fieldsHtml += `<div class="tl-field"><span class="tl-field__label">${esc(label)}</span><input type="text" class="tl-input" data-field="${attr(f.id)}"${val}${hint} spellcheck="false"></div>`;
      } else if (f.type === 'password') {
        fieldsHtml += `<div class="tl-field"><span class="tl-field__label">${esc(label)}</span><input type="password" class="tl-input" data-field="${attr(f.id)}"${val}${hint}></div>`;
      } else if (f.type === 'number') {
        fieldsHtml += `<div class="tl-field"><span class="tl-field__label">${esc(label)}</span><input type="number" class="tl-input tl-spinner-target" data-field="${attr(f.id)}"${val}></div>`;
      } else if (f.type === 'combo') {
        const opts = (f.options || []).map(o => `<option value="${attr(o)}" ${o === f.value ? 'selected' : ''}>${esc(o)}</option>`).join('');
        fieldsHtml += `<div class="tl-field"><span class="tl-field__label">${esc(label)}</span><select class="tl-combo-select" data-field="${attr(f.id)}">${opts}</select></div>`;
      } else if (f.type === 'checkbox') {
        const checked = f.value ? 'checked' : '';
        fieldsHtml += `<label class="tl-check"><input type="checkbox" data-field="${attr(f.id)}" ${checked}> ${esc(label)}</label>`;
      } else if (f.type === 'host_port') {
        fieldsHtml += `<div class="tl-field"><span class="tl-field__label">${esc(label)}</span><input type="text" class="tl-input" data-field="${attr(f.host_id || 'host')}" value="${attr(f.host_value || '')}" spellcheck="false"></div>`;
        fieldsHtml += `<div class="tl-field"><span class="tl-field__label">Port</span><input type="number" class="tl-input tl-spinner-target" data-field="${attr(f.port_id || 'port')}" value="${attr(f.port_value || '22')}"></div>`;
      } else if (f.type === 'file_picker') {
        fieldsHtml += `<div class="tl-field"><span class="tl-field__label">${esc(label)}</span><input type="text" class="tl-input" data-field="${attr(f.id)}"${val}${hint} spellcheck="false"></div>`;
      }
    }

    let handle = null;
    let done = false;
    const dismiss = (result) => {
      if (done) return;
      done = true;
      _dialogCooldown.add(pluginName);
      setTimeout(() => _dialogCooldown.delete(pluginName), 600);
      invoke('dialog_respond_form', { promptId: prompt_id, result }).catch(() => {});
      if (handle) handle.close();
    };

    const dialogButtons = buttons.map((b) => ({
      label: b.label,
      primary: b.id === 'ok' || b.id === 'save' || b.id === 'save_connect',
      onSelect: () => {
        if (b.id === 'cancel') { dismiss(null); return; }
        // Collect field values.
        const values = { _action: b.id };
        handle.el.querySelectorAll('[data-field]').forEach(el => {
          const id = el.dataset.field;
          if (el.type === 'checkbox') values[id] = el.checked;
          else values[id] = el.value;
        });
        dismiss(JSON.stringify(values));
      },
    }));

    handle = window.tlDialog.open({
      title,
      ariaLabel: title || 'Plugin form',
      size: 'md',
      body: (bodyEl) => {
        bodyEl.innerHTML = fieldsHtml;
        bodyEl.querySelectorAll('select.tl-combo-select').forEach((select) => {
          if (window.tlCombo && typeof window.tlCombo.attach === 'function') window.tlCombo.attach(select);
        });
        // 'number' fields and host_port's port field keep the native input
        // as the source of truth (tlSpinner.attach only adds a stepper
        // column next to it, same contract as tlCombo.attach above).
        bodyEl.querySelectorAll('input.tl-spinner-target').forEach((input) => {
          if (window.tlSpinner && typeof window.tlSpinner.attach === 'function') window.tlSpinner.attach(input);
        });
        // Keyboard-first UX: focus the first editable field automatically.
        setTimeout(() => {
          let firstInput = bodyEl.querySelector(
            'input[type="text"], input[type="password"], input[type="number"], select, textarea'
          );
          // tlCombo.attach() (above) hides 'combo' fields' native <select>
          // and shows a .tl-combo button in its place, so the select itself
          // can no longer receive focus — focus the visible button instead.
          if (firstInput && firstInput.tagName === 'SELECT') {
            firstInput = (firstInput._tlCombo && firstInput._tlCombo.button) || null;
          }
          if (firstInput && typeof firstInput.focus === 'function') {
            firstInput.focus();
            if (firstInput.tagName === 'INPUT' && typeof firstInput.select === 'function') {
              firstInput.select();
            }
          }
        }, 30);
      },
      buttons: dialogButtons,
      onClose: () => dismiss(null),
      onOpen: (panelEl) => { panelEl.setAttribute('data-plugin-dialog', pluginName); },
    });
  }

  function handlePromptDialog(event) {
    const { prompt_id, message, default_value } = event.payload;
    const pluginName = prompt_id.split('\0')[0];
    if (_dialogCooldown.has(pluginName) || findOpenPluginDialog(pluginName)) {
      invoke('dialog_respond_prompt', { promptId: prompt_id, value: null }).catch(() => {});
      return;
    }
    if (!window.tlDialog || typeof window.tlDialog.open !== 'function') {
      invoke('dialog_respond_prompt', { promptId: prompt_id, value: null }).catch(() => {});
      return;
    }

    let handle = null;
    let done = false;
    const dismiss = (val) => {
      if (done) return;
      done = true;
      _dialogCooldown.add(pluginName);
      setTimeout(() => _dialogCooldown.delete(pluginName), 600);
      invoke('dialog_respond_prompt', { promptId: prompt_id, value: val }).catch(() => {});
      if (handle) handle.close();
    };

    handle = window.tlDialog.open({
      title: 'Prompt',
      ariaLabel: 'Plugin prompt',
      size: 'sm',
      body: (bodyEl) => {
        bodyEl.innerHTML = `<div class="pw-label">${esc(message)}</div><input class="tl-input" id="pd-input" type="text" value="${attr(default_value || '')}" spellcheck="false">`;
        const input = bodyEl.querySelector('#pd-input');
        input.addEventListener('keydown', (e) => { if (e.key === 'Enter') dismiss(input.value); });
        setTimeout(() => input.focus(), 50);
      },
      buttons: [
        { label: 'Cancel', onSelect: () => dismiss(null) },
        { label: 'OK', primary: true, onSelect: () => dismiss(handle.el.querySelector('#pd-input').value) },
      ],
      onClose: () => dismiss(null),
      onOpen: (panelEl) => { panelEl.setAttribute('data-plugin-dialog', pluginName); },
    });
  }

  function handleConfirmDialog(event) {
    const { prompt_id, message } = event.payload;
    const pluginName = prompt_id.split('\0')[0];
    if (_dialogCooldown.has(pluginName) || findOpenPluginDialog(pluginName)) {
      invoke('dialog_respond_confirm', { promptId: prompt_id, accepted: false }).catch(() => {});
      return;
    }
    if (!window.tlDialog || typeof window.tlDialog.open !== 'function') {
      invoke('dialog_respond_confirm', { promptId: prompt_id, accepted: false }).catch(() => {});
      return;
    }

    let handle = null;
    let done = false;
    const dismiss = (val) => {
      if (done) return;
      done = true;
      _dialogCooldown.add(pluginName);
      setTimeout(() => _dialogCooldown.delete(pluginName), 600);
      invoke('dialog_respond_confirm', { promptId: prompt_id, accepted: val }).catch(() => {});
      if (handle) handle.close();
    };

    handle = window.tlDialog.open({
      title: 'Confirm',
      ariaLabel: 'Plugin confirmation',
      size: 'sm',
      body: (bodyEl) => {
        bodyEl.innerHTML = `<div class="pw-label">${esc(message)}</div>`;
      },
      buttons: [
        { label: 'No', onSelect: () => dismiss(false) },
        { label: 'Yes', primary: true, onSelect: () => dismiss(true) },
      ],
      onClose: () => dismiss(false),
      onOpen: (panelEl) => { panelEl.setAttribute('data-plugin-dialog', pluginName); },
    });
  }

  /// Map a plugin icon name to an <img> tag using the PNG icon set.
  /** Delegates to widget-icons.js, which validates plugin-supplied names. */
  function iconHtml(name, size) {
    return window.widgetIcons.iconHtml(name, size);
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

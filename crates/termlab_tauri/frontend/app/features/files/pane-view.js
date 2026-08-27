(function initTermLabFilesPaneView(global) {
  'use strict';

  // Drag-and-drop between panes (Task 7). One MIME type carries the whole
  // payload as JSON — paneId/path/isDir are what files-panel.js's
  // onDropEntries needs to route the transfer, read at `drop` time (data is
  // universally readable there).
  const DND_ENTRY_MIME = 'application/x-termlab-entry';

  // A second, empty-valued MIME type per kind, set alongside the JSON
  // payload at dragstart. `dragover` accept/reject is decided from this
  // ALONE — `dataTransfer.types` is reliably readable during `dragover`
  // across engines, but `getData` is not (WebKitGTK-class engines return
  // '' for it mid-drag): deciding from `types` keeps the drop-target
  // highlight working everywhere instead of silently going dead on an
  // engine that restricts `getData` at that stage.
  const DND_KIND_MIME_PREFIX = 'application/x-termlab-entry-kind-';
  function dndKindMime(kind) { return `${DND_KIND_MIME_PREFIX}${kind}`; }

  function defaultJoinPath(base, name) {
    const trimmed = String(base || '').replace(/\/+$/, '');
    return `${trimmed || ''}/${name}`;
  }

  function dndTypesOf(dataTransfer) {
    if (!dataTransfer || !dataTransfer.types) return [];
    return Array.isArray(dataTransfer.types) ? dataTransfer.types : Array.from(dataTransfer.types);
  }

  // `dataTransfer.types` is readable during `dragover` even where `getData`
  // is restricted (browser-dependent); checking it first means a foreign
  // drag (an OS file drop, or anything without our MIME) is recognized —
  // and left alone — without ever attempting to read its data.
  function dndHasEntryMime(dataTransfer) {
    return dndTypesOf(dataTransfer).indexOf(DND_ENTRY_MIME) !== -1;
  }

  // The dragged entry's pane kind, from `types` alone — no `getData` call.
  // Used by `dragover`, which must never depend on `getData` succeeding.
  function dndSourceKindFromTypes(dataTransfer) {
    const types = dndTypesOf(dataTransfer);
    if (types.indexOf(dndKindMime('local')) !== -1) return 'local';
    if (types.indexOf(dndKindMime('remote')) !== -1) return 'remote';
    return null;
  }

  // Parses the JSON payload a row's dragstart wrote, or null if it is
  // missing/malformed. Never throws — a drag this module doesn't recognize
  // must fall through untouched, not crash the handler. Only called from
  // `drop`, where reading data is universally permitted.
  function dndReadEntryPayload(dataTransfer) {
    if (!dndHasEntryMime(dataTransfer) || typeof dataTransfer.getData !== 'function') return null;
    let raw;
    try {
      raw = dataTransfer.getData(DND_ENTRY_MIME);
    } catch (err) {
      return null;
    }
    if (!raw) return null;
    try {
      const parsed = JSON.parse(raw);
      if (!parsed || (parsed.paneKind !== 'local' && parsed.paneKind !== 'remote')) return null;
      return parsed;
    } catch (err) {
      return null;
    }
  }

  function transferDirection(status) {
    return status && status.direction === 'download' ? 'download' : 'upload';
  }

  function transferPercentText(percent) {
    const value = Number(percent) || 0;
    if (value > 0 && value < 1) return '&lt;1%';
    return `${Math.max(0, Math.min(100, Math.round(value)))}%`;
  }

  function transferBadgeHtml(status) {
    if (!status) return '';
    if (status.status === 'attention') {
      if (status.transferId) {
        const transferId = String(status.transferId)
          .replace(/&/g, '&amp;')
          .replace(/"/g, '&quot;')
          .replace(/</g, '&lt;')
          .replace(/>/g, '&gt;');
        return `<button type="button" class="fp-transfer-attention" data-transfer-id="${transferId}" aria-label="Resolve transfer issue">Needs attention</button>`;
      }
      return '<span class="fp-transfer-attention" role="status">Needs attention</span>';
    }
    if (status.status === 'preparing' || status.status === 'starting') {
      const phase = status.status === 'preparing' ? 'Preparing' : 'Starting';
      return `<span class="fp-transfer-phase" role="status">${phase} ${transferDirection(status)}…</span>`;
    }
    if (status.status === 'waiting') {
      return `<span class="fp-transfer-waiting" role="status">Waiting to retry ${transferDirection(status)}…</span>`;
    }
    if (status.status === 'in_progress') {
      return `<span class="fp-transfer-pct">${transferPercentText(status.percent)}</span>`;
    }
    return '';
  }

  function transferActivityText(transferStatus) {
    const active = Object.entries(transferStatus || {}).filter(([, status]) => (
      status
      && ['preparing', 'starting', 'in_progress'].includes(status.status)
    ));
    if (active.length === 0) return '';
    if (active.length > 1) return `${active.length} transfers active`;

    const [fileName, status] = active[0];
    const direction = transferDirection(status);
    if (status.status === 'preparing') return `Preparing ${direction}: ${fileName}`;
    if (status.status === 'starting') return `Starting ${direction}: ${fileName}`;
    const action = direction === 'download' ? 'Downloading' : 'Uploading';
    return `${action} ${fileName}: ${transferPercentText(status.percent).replace('&lt;', '<')}`;
  }

  function activateTransferBadge(status, invoker, deps) {
    if (!status || status.status !== 'attention' || !status.transferId) return;
    if (deps && typeof deps.onTransferAttention === 'function') {
      deps.onTransferAttention(status.transferId, invoker);
    }
  }

  // The ⋮ overflow menu holds the low-frequency actions the toolbar no longer
  // shows inline: Go home and the checkable hidden-files toggle. Anchored to
  // the button's rect (keyboard activation synthesizes 0,0 click coords).
  function openMoreMenu(btn, pane, d) {
    if (!global.tlMenu || typeof global.tlMenu.open !== 'function') return;
    const rect = typeof btn.getBoundingClientRect === 'function'
      ? btn.getBoundingClientRect()
      : { left: 0, bottom: 0 };
    global.tlMenu.open({
      x: rect.left,
      y: rect.bottom + 2,
      ariaLabel: 'More file actions',
      items: [
        {
          label: 'Go home',
          disabled: !pane.isLocal && !d.activeRemotePaneId,
          onSelect: () => { if (typeof d.onHome === 'function') d.onHome(); },
        },
        {
          label: 'Show hidden files',
          checked: !!pane.showHidden,
          onSelect: () => { if (typeof d.onToggleHidden === 'function') d.onToggleHidden(); },
        },
      ],
    });
  }

  function renderPane(pane, el, deps) {
    if (!el || !pane) return;
    const d = deps || {};

    const isRemote = !pane.isLocal;
    const noSession = isRemote && !d.activeRemotePaneId;

    const esc = typeof d.esc === 'function' ? d.esc : (value) => String(value == null ? '' : value);
    const attr = typeof d.attr === 'function' ? d.attr : esc;
    const extOf = typeof d.extOf === 'function' ? d.extOf : () => '';
    const formatSize = typeof d.formatSize === 'function' ? d.formatSize : (value) => String(value || 0);
    const formatDate = typeof d.formatDate === 'function' ? d.formatDate : (value) => String(value || '');
    const fileIcons = d.fileIcons && typeof d.fileIcons.iconFor === 'function' ? d.fileIcons : null;
    const sortArrow = typeof d.sortArrow === 'function' ? d.sortArrow : () => '';

    const visibleEntries = (Array.isArray(pane.entries) ? pane.entries : [])
      .filter((entry) => pane.showHidden || !String(entry.name || '').startsWith('.'));
    const hiddenCount = (pane.entries || []).length - visibleEntries.length;
    const footerText = noSession
      ? 'Not connected'
      : (hiddenCount > 0
        ? `${visibleEntries.length} items (${hiddenCount} hidden)`
        : `${visibleEntries.length} items`);
    const activityText = transferActivityText(pane.transferStatus);

    // Breadcrumb path bar: default view of the pane's location. The text
    // input stays in the DOM (hidden) for click-to-edit; without a session or
    // without the breadcrumbs module the plain input renders as before.
    const crumbsApi = global.termlabFilesBreadcrumbs;
    const crumbPath = pane.currentPath || pane.pathInput || '';
    const useCrumbs = !!(crumbsApi && crumbPath && !noSession);
    let hiddenCrumbSegments = [];
    let pathBarHtml = `<input class="fp-path-input${useCrumbs ? ' fp-path-input--hidden' : ''}" type="text" value="${attr(pane.pathInput)}" spellcheck="false" ${noSession ? 'disabled' : ''} />`;
    if (useCrumbs) {
      const folded = crumbsApi.collapse(crumbsApi.segments(crumbPath), 4);
      hiddenCrumbSegments = folded.hidden;
      const ordered = [folded.head, ...folded.tail].filter(Boolean);
      const lastIndex = ordered.length - 1;
      const crumbHtml = (seg, isCurrent) => (isCurrent
        ? `<span class="fp-crumb is-current" title="${attr(seg.path)}">${esc(seg.label)}</span>`
        : `<button type="button" class="fp-crumb" data-crumb-path="${attr(seg.path)}" title="${attr(seg.path)}">${esc(seg.label)}</button>`);
      const sep = '<span class="fp-crumb-sep">/</span>';
      const parts = [];
      ordered.forEach((seg, index) => {
        if (index === 1 && folded.hidden.length > 0) {
          if (ordered[0].label !== '/') parts.push(sep);
          parts.push('<button type="button" class="fp-crumb fp-crumb-overflow" data-action="crumb-overflow" title="Show folded folders">…</button>');
          parts.push(sep);
        } else if (index > 0 && !(index === 1 && ordered[0].label === '/')) {
          parts.push(sep);
        }
        parts.push(crumbHtml(seg, index === lastIndex));
      });
      pathBarHtml = `<div class="fp-crumbs" role="navigation" aria-label="Current path">${parts.join('')}</div>${pathBarHtml}`;
    }

    el.innerHTML = `
      ${isRemote ? `
      <div class="fp-host-strip">
        <span class="fp-host-status${noSession ? '' : ' is-connected'}" title="${noSession ? 'Not connected' : 'Connected'}"></span>
        <span class="fp-host-combo-slot"></span>
      </div>` : ''}
      <div class="fp-toolbar">
        <span class="fp-tb-group fp-tb-nav">
          <button class="fp-tb-btn" data-action="back" ${pane.backStack.length === 0 ? 'disabled' : ''} title="Back">${d.iconBack || ''}</button>
          <button class="fp-tb-btn" data-action="forward" ${pane.forwardStack.length === 0 ? 'disabled' : ''} title="Forward">${d.iconForward || ''}</button>
          ${pathBarHtml}
          <button class="fp-tb-btn" data-action="refresh" title="Refresh" ${noSession ? 'disabled' : ''}>${d.iconRefresh || ''}</button>
          <button class="fp-tb-btn" data-action="more" title="More actions" aria-haspopup="menu">${d.iconMore || '⋮'}</button>
        </span>
      </div>
      ${pane.error ? `<div class="fp-error">${esc(pane.error)}</div>` : ''}
      <div class="fp-table-wrap">
        <table class="fp-table">
          <thead><tr>
            <th class="fp-th-name" data-col="name">Name ${sortArrow(pane, 'name')}</th>
            ${pane.colExt ? `<th class="fp-th-ext" data-col="ext">Ext ${sortArrow(pane, 'ext')}</th>` : ''}
            ${pane.colSize ? `<th class="fp-th-size" data-col="size">Size ${sortArrow(pane, 'size')}</th>` : ''}
            ${pane.colModified ? `<th class="fp-th-mod" data-col="modified">Modified ${sortArrow(pane, 'modified')}</th>` : ''}
          </tr></thead>
          <tbody></tbody>
        </table>
      </div>
      <div class="fp-footer">
        <span>${esc(footerText)}</span>
        ${activityText ? `<span class="fp-footer__activity" role="status" aria-live="polite">${esc(activityText)}</span>` : ''}
      </div>
    `;

    const joinPathFn = typeof d.joinPath === 'function' ? d.joinPath : defaultJoinPath;

    const tbody = el.querySelector('tbody');
    for (const entry of visibleEntries) {
      const tr = document.createElement('tr');
      tr.className = 'fp-row';
      tr.tabIndex = 0;
      tr.draggable = true;
      tr.setAttribute('aria-label', entry.is_dir ? `Folder ${entry.name}` : `File ${entry.name}`);
      const ts = pane.transferStatus && pane.transferStatus[entry.name];
      if (ts) {
        if (ts.status === 'completed') tr.classList.add('fp-transferred');
        else if (['preparing', 'starting', 'in_progress'].includes(ts.status)) tr.classList.add('fp-transferring');
      }
      tr.dataset.name = entry.name;

      const icon = fileIcons ? fileIcons.iconFor(entry.name, entry.is_dir, !pane.isLocal) : '';
      let cells = `<td class="fp-cell-name">${icon} <span>${esc(entry.name)}</span>`;
      cells += transferBadgeHtml(ts);
      cells += '</td>';
      if (pane.colExt) cells += `<td class="fp-cell-ext">${esc(extOf(entry.name))}</td>`;
      if (pane.colSize) cells += `<td class="fp-cell-size">${entry.is_dir ? '' : formatSize(entry.size)}</td>`;
      if (pane.colModified) cells += `<td class="fp-cell-mod">${entry.modified ? formatDate(entry.modified) : ''}</td>`;
      tr.innerHTML = cells;

      const attentionControl = tr.querySelector('.fp-transfer-attention[data-transfer-id]');
      if (attentionControl) {
        attentionControl.addEventListener('click', (event) => {
          event.preventDefault();
          event.stopPropagation();
          activateTransferBadge(ts, attentionControl, d);
        });
        attentionControl.addEventListener('dblclick', (event) => {
          event.preventDefault();
          event.stopPropagation();
        });
      }

      tr.addEventListener('dragstart', (event) => {
        const dataTransfer = event && event.dataTransfer;
        if (!dataTransfer || typeof dataTransfer.setData !== 'function') return;
        const kind = isRemote ? 'remote' : 'local';
        const payload = {
          paneKind: kind,
          // Local browsing has no per-session pane id in this dual-pane
          // model (there is exactly one local pane); the remote side's id
          // is the pinned/active SSH session pane the row belongs to.
          paneId: isRemote ? (d.activeRemotePaneId != null ? d.activeRemotePaneId : null) : null,
          path: joinPathFn(pane.currentPath, entry.name),
          isDir: !!entry.is_dir,
        };
        dataTransfer.setData(DND_ENTRY_MIME, JSON.stringify(payload));
        // Empty-valued marker type, checked (never read) by dragover — see
        // dndSourceKindFromTypes above.
        dataTransfer.setData(dndKindMime(kind), '');
        // Record the in-flight payload with files-panel: on platforms where
        // native drag interception swallows DOM dragover/drop (macOS), the
        // drop completes over the native channel and needs this record.
        if (typeof d.onDragStart === 'function') d.onDragStart(payload);
      });
      // Source-side safety net: an engine that cancels a drag mid-flight
      // (Escape, dropping outside any target) fires `dragend` on the
      // dragged row without necessarily firing `dragleave` on whichever
      // pane's drop-target highlight is lit. Clearing both panes' roots
      // here — rather than trying to track which one is lit — is simplest
      // and idempotent; files-panel.js's onDragEnd dep is the one with
      // handles to both #fp-local and #fp-remote.
      tr.addEventListener('dragend', () => {
        if (typeof d.onDragEnd === 'function') d.onDragEnd();
      });
      tr.addEventListener('dblclick', () => {
        if (typeof d.onActivateEntry === 'function') d.onActivateEntry(entry);
      });
      tr.addEventListener('click', () => {
        el.querySelectorAll('.fp-row.selected').forEach((row) => row.classList.remove('selected'));
        tr.classList.add('selected');
        if (typeof d.onSelectEntry === 'function') d.onSelectEntry(entry.name);
      });
      tr.addEventListener('contextmenu', (event) => {
        event.preventDefault();
        el.querySelectorAll('.fp-row.selected').forEach((row) => row.classList.remove('selected'));
        tr.classList.add('selected');
        if (typeof d.onSelectEntry === 'function') d.onSelectEntry(entry.name);
        if (typeof d.onOpenRowMenu === 'function') d.onOpenRowMenu(event, entry);
      });
      tr.addEventListener('keydown', (event) => {
        if (event.target !== tr) return;
        if (event.key === 'Enter') {
          event.preventDefault();
          if (typeof d.onActivateEntry === 'function') d.onActivateEntry(entry);
          return;
        }
        if (event.key === ' ') {
          event.preventDefault();
          el.querySelectorAll('.fp-row.selected').forEach((row) => row.classList.remove('selected'));
          tr.classList.add('selected');
          if (typeof d.onSelectEntry === 'function') d.onSelectEntry(entry.name);
        }
      });
      tbody.appendChild(tr);
    }

    el.querySelectorAll('.fp-tb-btn').forEach((btn) => {
      btn.addEventListener('click', () => {
        const action = btn.dataset.action;
        if (action === 'back' && typeof d.onBack === 'function') d.onBack();
        else if (action === 'forward' && typeof d.onForward === 'function') d.onForward();
        else if (action === 'refresh' && typeof d.onRefresh === 'function') d.onRefresh();
        else if (action === 'more') openMoreMenu(btn, pane, d);
      });
    });

    const pathInput = el.querySelector('.fp-path-input');
    if (pathInput) {
      pathInput.addEventListener('keydown', (event) => {
        if (event.key !== 'Enter') return;
        const value = pathInput.value.trim();
        if (!value) return;
        if (typeof d.onNavigate === 'function') d.onNavigate(value);
      });
    }

    // Crumb bar: ancestor crumbs navigate, the … crumb lists folded folders,
    // and a click on the bar's empty area swaps to the text input for direct
    // path entry (Escape or blur returns to crumbs without navigating).
    const crumbsEl = el.querySelector('.fp-crumbs');
    if (crumbsEl && pathInput) {
      const exitEdit = () => {
        pathInput.classList.add('fp-path-input--hidden');
        crumbsEl.style.display = '';
      };
      const enterEdit = () => {
        crumbsEl.style.display = 'none';
        pathInput.classList.remove('fp-path-input--hidden');
        if (typeof pathInput.focus === 'function') pathInput.focus();
        if (typeof pathInput.select === 'function') pathInput.select();
      };
      crumbsEl.addEventListener('click', (event) => {
        const target = event.target;
        const closest = target && typeof target.closest === 'function'
          ? (sel) => target.closest(sel)
          : () => null;
        const crumbBtn = closest('[data-crumb-path]');
        if (crumbBtn) {
          const crumbPathValue = crumbBtn.getAttribute('data-crumb-path');
          if (crumbPathValue && typeof d.onNavigate === 'function') d.onNavigate(crumbPathValue);
          return;
        }
        const overflowBtn = closest('[data-action="crumb-overflow"]');
        if (overflowBtn) {
          if (global.tlMenu && typeof global.tlMenu.open === 'function') {
            const rect = typeof overflowBtn.getBoundingClientRect === 'function'
              ? overflowBtn.getBoundingClientRect()
              : { left: 0, bottom: 0 };
            global.tlMenu.open({
              x: rect.left,
              y: rect.bottom + 2,
              ariaLabel: 'Folded folders',
              items: hiddenCrumbSegments.map((seg) => ({
                label: seg.path,
                onSelect: () => { if (typeof d.onNavigate === 'function') d.onNavigate(seg.path); },
              })),
            });
          }
          return;
        }
        enterEdit();
      });
      pathInput.addEventListener('keydown', (event) => {
        if (event.key === 'Escape') exitEdit();
      });
      pathInput.addEventListener('blur', exitEdit);
    }

    el.querySelectorAll('th[data-col]').forEach((th) => {
      th.style.cursor = 'pointer';
      th.tabIndex = 0;
      th.setAttribute('role', 'button');
      th.addEventListener('click', () => {
        if (typeof d.onSort !== 'function') return;
        d.onSort(th.dataset.col);
      });
      th.addEventListener('keydown', (event) => {
        if (event.key !== 'Enter' && event.key !== ' ') return;
        event.preventDefault();
        if (typeof d.onSort !== 'function') return;
        d.onSort(th.dataset.col);
      });
      th.addEventListener('contextmenu', (event) => {
        event.preventDefault();
        if (typeof d.onOpenColumnMenu === 'function') {
          d.onOpenColumnMenu(event);
        }
      });
    });

    // Drop target (Task 7) — bound on the .fp-pane ROOT (`el`): the element
    // that carries the is-drop-target highlight styles/panels.css lights up
    // edge to edge, and the same element the native (OS) drop path in
    // files-panel.js hit-tests. Listening on the inner .fp-table-wrap
    // instead left a highlighted pane ignoring releases over its own
    // toolbar or footer.
    //
    // `el` persists across renders (only its innerHTML is rebuilt), so the
    // listeners are attached exactly once and read their live inputs — the
    // pane's current path, this render's onDropEntries — from a context
    // object refreshed on every call. Re-binding per render would stack
    // duplicate listeners; capturing this render's values would go stale.
    //
    // Same-kind drags (local-on-local, remote-on-remote) and foreign drags
    // (no entry MIME — OS file drops are Task 8's territory) fall through
    // untouched: no preventDefault, so the browser's/Tauri's own handling
    // still applies.
    const dropContext = el.__termlabPaneDropContext || {};
    dropContext.targetPaneKind = isRemote ? 'remote' : 'local';
    dropContext.currentPath = pane.currentPath;
    dropContext.onDropEntries = d.onDropEntries;
    if (!el.__termlabPaneDropContext) {
      el.__termlabPaneDropContext = dropContext;
      el.addEventListener('dragover', (event) => {
        // Deliberately types-only — never getData — so the accept decision
        // works on engines that restrict getData during dragover. See
        // dndSourceKindFromTypes's comment above.
        const sourceKind = dndSourceKindFromTypes(event.dataTransfer);
        if (!sourceKind || sourceKind === dropContext.targetPaneKind) return;
        event.preventDefault();
        el.classList.add('is-drop-target');
      });
      el.addEventListener('dragleave', () => {
        el.classList.remove('is-drop-target');
      });
      el.addEventListener('drop', (event) => {
        el.classList.remove('is-drop-target');
        const source = dndReadEntryPayload(event.dataTransfer);
        if (!source || source.paneKind === dropContext.targetPaneKind) return;
        event.preventDefault();
        if (typeof dropContext.onDropEntries === 'function') {
          dropContext.onDropEntries({
            source,
            targetPaneKind: dropContext.targetPaneKind,
            targetPath: dropContext.currentPath,
          });
        }
      });
    }

    // Host combo (remote pane only) — built with the DOM API rather than
    // interpolated into the innerHTML template above, because the
    // <select>'s live identity matters: tlCombo.attach() below must run
    // against THIS render's element, and el.innerHTML just wiped whatever
    // combo the previous render attached. Every renderPane call is a fresh
    // trap for a one-time attach() — see files-panel.js's onComboMount,
    // which is invoked here, every time, for exactly this reason.
    if (isRemote) {
      const slot = el.querySelector('.fp-host-combo-slot');
      if (slot) {
        const select = document.createElement('select');
        select.className = 'fp-host-select';
        (Array.isArray(d.hostOptions) ? d.hostOptions : []).forEach((opt) => {
          const optionEl = document.createElement('option');
          optionEl.value = opt.value;
          optionEl.textContent = opt.label;
          if (opt.disabled) optionEl.disabled = true;
          select.appendChild(optionEl);
        });
        select.value = d.hostComboValue || '';
        select.disabled = !!d.hostComboBusy;
        select.addEventListener('change', () => {
          if (typeof d.onHostComboChange === 'function') d.onHostComboChange(select.value);
        });
        slot.appendChild(select);

        if (d.showDisconnect) {
          const eject = document.createElement('button');
          eject.type = 'button';
          eject.className = 'fp-tb-btn fp-host-disconnect';
          eject.title = 'Disconnect';
          eject.textContent = '⏏';
          eject.addEventListener('click', () => {
            if (typeof d.onDisconnect === 'function') d.onDisconnect();
          });
          slot.appendChild(eject);
        }

        if (typeof d.onComboMount === 'function') d.onComboMount(select);
      }
    }
  }

  // Column-visibility toggle menu — renders through the shared window.tlMenu
  // component. Items are checkable (tlMenu's `checked` item property renders
  // a "✓" glyph in the icon gutter); clicking one toggles the column and
  // closes the menu, same as before.
  function showColumnMenu(event, pane, deps) {
    const d = deps || {};
    if (!global.tlMenu || typeof global.tlMenu.open !== 'function') {
      console.error('files-pane-view: window.tlMenu is unavailable');
      return null;
    }

    const cols = [
      { key: 'colExt', label: 'Extension' },
      { key: 'colSize', label: 'Size' },
      { key: 'colModified', label: 'Modified' },
    ];

    const items = cols.map((col) => ({
      label: col.label,
      checked: !!pane[col.key],
      onSelect: () => {
        if (typeof d.onToggleColumn === 'function') d.onToggleColumn(col.key);
      },
    }));

    return global.tlMenu.open({
      x: event.clientX,
      y: event.clientY,
      items,
      ariaLabel: 'File columns',
      routerName: 'fp-col-context-menu',
      routerPriority: 220,
    });
  }

  // ---------------------------------------------------------------------------
  // Row context menu (right-click on a file/dir row) — renders through the
  // shared window.tlMenu component (styles/design-system/components/menu.css,
  // app/ui/tl-menu.js), which owns positioning, dismissal, and single-instance
  // behavior. Items are pre-built by the caller (files-panel.js), which owns
  // the business logic (invoke, session state); this module only translates
  // that item shape ({type: 'separator'} / {action}) into tlMenu's
  // ({separator: true} / {onSelect}).
  function showRowContextMenu(event, items) {
    if (!global.tlMenu || typeof global.tlMenu.open !== 'function') {
      console.error('files-pane-view: window.tlMenu is unavailable');
      return null;
    }

    const menuItems = (Array.isArray(items) ? items : []).map((item) => {
      if (item.type === 'separator') return { separator: true };
      return {
        label: item.label,
        icon: item.icon,
        disabled: item.disabled,
        danger: item.danger,
        title: item.title,
        onSelect: item.action,
      };
    });

    return global.tlMenu.open({
      x: event.clientX,
      y: event.clientY,
      items: menuItems,
      ariaLabel: 'File actions',
      routerName: 'fp-row-context-menu',
      routerPriority: 220,
    });
  }

  global.termlabFilesPaneView = {
    renderPane,
    transferBadgeHtml,
    transferActivityText,
    activateTransferBadge,
    showColumnMenu,
    showRowContextMenu,
  };
})(window);

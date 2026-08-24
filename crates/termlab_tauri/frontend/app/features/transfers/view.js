(function initTermLabTransferCenterView(global) {
  'use strict';

  const TERMINAL_KINDS = new Set(['completed', 'failed', 'cancelled']);
  const COLUMNS = [
    'File / Direction',
    'Host',
    'Destination',
    'Status / Progress',
    'Speed / ETA',
    'Actions',
  ];

  function append(parent, tag, className, text) {
    const element = document.createElement(tag);
    if (className) element.className = className;
    if (text !== undefined) element.textContent = text;
    parent.appendChild(element);
    return element;
  }

  function setData(element, name, value) {
    element.setAttribute(`data-${name}`, value);
  }

  function titleCase(value) {
    const text = String(value || '');
    return text ? text.charAt(0).toUpperCase() + text.slice(1) : 'Unknown';
  }

  function isHistoryJob(job) {
    return TERMINAL_KINDS.has(job && job.state ? job.state.kind : '');
  }

  function jobsFor(snapshot, viewMode) {
    const jobs = snapshot && Array.isArray(snapshot.jobs) ? snapshot.jobs : [];
    const history = viewMode === 'history';
    return jobs.filter((job) => isHistoryJob(job) === history);
  }

  function endpointLabel(endpoint) {
    if (!endpoint) return 'Unknown host';
    if (endpoint.kind === 'configured') return endpoint.label || endpoint.serverEntryId || 'Configured host';
    if (endpoint.kind === 'adHoc') {
      const user = endpoint.user ? `${endpoint.user}@` : '';
      const port = endpoint.port ? `:${endpoint.port}` : '';
      return `${user}${endpoint.host || 'Unknown host'}${port}`;
    }
    return endpoint.label || endpoint.host || 'Unknown host';
  }

  function transferPaths(job) {
    const upload = job && job.direction === 'upload';
    return {
      source: upload ? job.localPath : job.remotePath,
      destination: upload ? job.remotePath : job.localPath,
    };
  }

  function percentFor(job) {
    const total = Number(job && job.totalBytes);
    if (!(total > 0)) return null;
    const transferred = Number(job.bytesTransferred) || 0;
    return Math.max(0, Math.min(100, Math.round((transferred / total) * 100)));
  }

  function statusFor(job) {
    const state = (job && job.state) || {};
    switch (state.kind) {
      case 'queued': return 'Queued';
      case 'connecting': return 'Connecting';
      case 'checking': return 'Checking';
      case 'running': return 'Running';
      case 'paused': return 'Paused';
      case 'needsConnection': return state.message ? `Needs connection — ${state.message}` : 'Needs connection';
      case 'needsAttention': return 'Needs attention';
      case 'retryWaiting': return `Retry waiting${state.attempt ? ` — attempt ${state.attempt}` : ''}`;
      case 'completed': return state.result === 'skipped' ? 'Completed — Skipped' : 'Completed';
      case 'failed': return state.error ? `Failed — ${state.error}` : 'Failed';
      case 'cancelled': return state.cleanupError ? `Cancelled — ${state.cleanupError}` : 'Cancelled';
      default: return titleCase(state.kind);
    }
  }

  function formatEta(seconds) {
    const value = Math.max(0, Math.round(Number(seconds) || 0));
    if (value < 60) return `${value}s remaining`;
    const minutes = Math.floor(value / 60);
    const remainder = value % 60;
    return remainder ? `${minutes}m ${remainder}s remaining` : `${minutes}m remaining`;
  }

  function actionsFor(job) {
    const kind = job && job.state ? job.state.kind : '';
    switch (kind) {
      case 'running': return [['pause', 'Pause'], ['cancel', 'Cancel']];
      case 'paused': return [['resume', 'Resume'], ['cancel', 'Cancel']];
      case 'failed': return [['retry', 'Retry'], ['details', 'Details']];
      case 'needsConnection': return [['connect', 'Connect'], ['cancel', 'Cancel']];
      case 'needsAttention': return [['resolve', 'Resolve'], ['cancel', 'Cancel']];
      case 'queued': return [
        ['pause', 'Pause'],
        ['toggle-priority', job.priority === 'interactive' ? 'Normal priority' : 'Prioritize'],
        ['move-up', 'Move up'],
        ['move-down', 'Move down'],
        ['cancel', 'Cancel'],
      ];
      case 'completed':
      case 'cancelled':
        return [['details', 'Details']];
      default:
        return [];
    }
  }

  function create(options) {
    const opts = options || {};
    const panelEl = opts.panelEl;
    if (!panelEl) throw new Error('Transfer Center view requires panelEl');
    const formatSize = typeof opts.formatSize === 'function'
      ? opts.formatSize
      : (value) => `${Number(value) || 0} B`;
    const onViewChange = typeof opts.onViewChange === 'function' ? opts.onViewChange : () => {};
    const onSelect = typeof opts.onSelect === 'function' ? opts.onSelect : () => {};
    const onAction = typeof opts.onAction === 'function' ? opts.onAction : () => {};

    panelEl.classList.add('tl-transfer-center');
    panelEl.setAttribute('tabindex', '-1');

    const toolbarEl = append(panelEl, 'div', 'tl-transfer-center__toolbar');
    toolbarEl.setAttribute('role', 'toolbar');
    toolbarEl.setAttribute('aria-label', 'Transfer controls');
    const tabsEl = append(toolbarEl, 'div', 'tl-transfer-center__tabs');
    tabsEl.setAttribute('role', 'tablist');
    tabsEl.setAttribute('aria-label', 'Transfer view');
    const activeButtonEl = append(tabsEl, 'button', 'tl-transfer-center__tab');
    activeButtonEl.setAttribute('type', 'button');
    activeButtonEl.setAttribute('role', 'tab');
    setData(activeButtonEl, 'transfer-view', 'active');
    const historyButtonEl = append(tabsEl, 'button', 'tl-transfer-center__tab');
    historyButtonEl.setAttribute('type', 'button');
    historyButtonEl.setAttribute('role', 'tab');
    setData(historyButtonEl, 'transfer-view', 'history');

    const summaryEl = append(toolbarEl, 'div', 'tl-transfer-center__summary');
    summaryEl.setAttribute('aria-live', 'polite');
    summaryEl.setAttribute('aria-atomic', 'true');

    const toolbarActionsEl = append(toolbarEl, 'div', 'tl-transfer-center__toolbar-actions');
    const queueButtonEl = append(toolbarActionsEl, 'button', 'tl-btn tl-transfer-center__toolbar-button');
    queueButtonEl.setAttribute('type', 'button');
    const clearButtonEl = append(toolbarActionsEl, 'button', 'tl-btn tl-transfer-center__toolbar-button', 'Clear completed');
    clearButtonEl.setAttribute('type', 'button');
    clearButtonEl.setAttribute('aria-label', 'Clear completed transfer history');
    setData(clearButtonEl, 'transfer-action', 'clear-completed');
    const concurrencyButtonEl = append(toolbarActionsEl, 'button', 'tl-btn tl-transfer-center__toolbar-button', 'Concurrency');
    concurrencyButtonEl.setAttribute('type', 'button');
    concurrencyButtonEl.setAttribute('aria-label', 'Configure transfer concurrency');
    setData(concurrencyButtonEl, 'transfer-action', 'concurrency');

    const recoveryEl = append(panelEl, 'div', 'tl-transfer-center__recovery');
    recoveryEl.setAttribute('role', 'alert');
    setData(recoveryEl, 'transfer-state', 'recovery-error');
    recoveryEl.hidden = true;

    const stateEl = append(panelEl, 'div', 'tl-transfer-center__state');
    setData(stateEl, 'transfer-state', 'loading');
    stateEl.textContent = 'Loading transfers…';

    const tableViewportEl = append(panelEl, 'div', 'tl-transfer-center__table-viewport');
    tableViewportEl.hidden = true;
    const tableEl = append(tableViewportEl, 'table', 'tl-transfer-center__table');
    tableEl.setAttribute('aria-label', 'Transfers');
    const theadEl = append(tableEl, 'thead');
    const headerRowEl = append(theadEl, 'tr');
    for (const column of COLUMNS) {
      const cell = append(headerRowEl, 'th', '', column);
      cell.setAttribute('scope', 'col');
    }
    const tbodyEl = append(tableEl, 'tbody');

    let currentIds = [];
    let rowsById = new Map();
    let latestSnapshot = null;
    let latestState = { viewMode: 'active', selectedId: null };
    let lastSummaryText = null;

    function patchIdentity(rowRecord, job) {
      const fileCell = rowRecord.cells.file;
      fileCell.replaceChildren();
      append(fileCell, 'span', 'tl-transfer-center__file-name', job.fileName || 'Unnamed transfer');
      append(fileCell, 'span', 'tl-transfer-center__direction', titleCase(job.direction));

      rowRecord.cells.host.textContent = endpointLabel(job.endpoint);
      const paths = transferPaths(job);
      const destinationCell = rowRecord.cells.destination;
      destinationCell.replaceChildren();
      const sourceEl = append(destinationCell, 'span', 'tl-transfer-center__path', paths.source || '—');
      sourceEl.setAttribute('title', paths.source || '');
      append(destinationCell, 'span', 'tl-transfer-center__path-arrow', '→');
      const destinationEl = append(destinationCell, 'span', 'tl-transfer-center__path', paths.destination || '—');
      destinationEl.setAttribute('title', paths.destination || '');
    }

    function patchProgress(rowRecord, job) {
      const statusCell = rowRecord.cells.status;
      statusCell.replaceChildren();
      const paths = transferPaths(job);
      const percentage = percentFor(job);
      const status = statusFor(job);
      append(
        statusCell,
        'span',
        `tl-transfer-center__status tl-transfer-center__status--${job.state && job.state.kind ? job.state.kind : 'unknown'}`,
        percentage === null ? status : `${status} — ${percentage}%`,
      );
      if (percentage !== null) {
        const progressEl = append(statusCell, 'progress', 'tl-transfer-center__progress');
        progressEl.value = Number(job.bytesTransferred) || 0;
        progressEl.max = Number(job.totalBytes) || 1;
        const progressName = job.fileName || paths.destination || paths.source || 'Transfer';
        progressEl.setAttribute('aria-label', `${progressName} progress: ${percentage}%`);
      }

      const speedCell = rowRecord.cells.speed;
      speedCell.replaceChildren();
      const speed = Number(job.speedBytesPerSecond) || 0;
      append(speedCell, 'span', 'tl-transfer-center__speed', speed > 0 ? `${formatSize(speed)}/s` : '—');
      if (job.etaSeconds !== null && job.etaSeconds !== undefined) {
        append(speedCell, 'span', 'tl-transfer-center__eta', formatEta(job.etaSeconds));
      }
    }

    function patchActions(rowRecord, job) {
      const actionsCell = rowRecord.cells.actions;
      actionsCell.replaceChildren();
      for (const [action, label] of actionsFor(job)) {
        const button = append(actionsCell, 'button', 'tl-transfer-center__action', label);
        button.setAttribute('type', 'button');
        button.setAttribute('aria-label', `${label} ${job.fileName || 'transfer'}`);
        setData(button, 'transfer-action', action);
        setData(button, 'job-id', job.id);
      }
    }

    function signatures(job) {
      return {
        identity: JSON.stringify([
          job.fileName,
          job.direction,
          job.endpoint,
          job.localPath,
          job.remotePath,
        ]),
        progress: JSON.stringify([
          job.state,
          job.bytesTransferred,
          job.totalBytes,
          job.speedBytesPerSecond,
          job.etaSeconds,
        ]),
        actions: JSON.stringify([job.state && job.state.kind, job.priority]),
      };
    }

    function patchRow(rowRecord, job, selected) {
      const next = signatures(job);
      const previous = rowRecord.signatures;
      if (!previous || previous.identity !== next.identity) patchIdentity(rowRecord, job);
      if (!previous || previous.progress !== next.progress) patchProgress(rowRecord, job);
      if (!previous || previous.actions !== next.actions) patchActions(rowRecord, job);
      rowRecord.signatures = next;
      rowRecord.element.setAttribute('aria-selected', selected ? 'true' : 'false');
      rowRecord.element.classList.toggle('is-selected', selected);
    }

    function createRow(job, selected) {
      const rowEl = document.createElement('tr');
      rowEl.className = 'tl-transfer-center__row';
      setData(rowEl, 'job-id', job.id);
      rowEl.setAttribute('tabindex', '0');
      const cells = {};
      for (const name of ['file', 'host', 'destination', 'status', 'speed', 'actions']) {
        cells[name] = append(rowEl, 'td', `tl-transfer-center__cell tl-transfer-center__cell--${name}`);
        setData(cells[name], 'transfer-cell', name);
      }
      const record = { element: rowEl, cells, signatures: null };
      patchRow(record, job, selected);
      return record;
    }

    function updateSummary(snapshot, state) {
      const summary = snapshot && snapshot.summary ? snapshot.summary : {};
      const active = Number(summary.active) || 0;
      const history = Number(summary.history) || 0;
      activeButtonEl.textContent = `Active ${active}`;
      historyButtonEl.textContent = `History ${history}`;
      const historySelected = state.viewMode === 'history';
      activeButtonEl.setAttribute('aria-selected', historySelected ? 'false' : 'true');
      historyButtonEl.setAttribute('aria-selected', historySelected ? 'true' : 'false');
      activeButtonEl.classList.toggle('is-active', !historySelected);
      historyButtonEl.classList.toggle('is-active', historySelected);

      const parts = [
        `${active} active`,
        `${Number(summary.running) || 0} running`,
        `${Number(summary.queued) || 0} queued`,
        `${Number(summary.paused) || 0} paused`,
        `${Number(summary.attention) || 0} need attention`,
        `${Number(summary.failed) || 0} failed`,
      ];
      const summaryText = `${snapshot.queuePaused ? 'Queue paused · ' : ''}${parts.join(' · ')}`;
      if (summaryText !== lastSummaryText) {
        summaryEl.textContent = summaryText;
        lastSummaryText = summaryText;
      }

      const paused = !!snapshot.queuePaused;
      queueButtonEl.textContent = paused ? 'Resume all' : 'Pause all';
      queueButtonEl.setAttribute('aria-label', paused ? 'Resume all eligible transfers' : 'Pause all active transfers');
      setData(queueButtonEl, 'transfer-action', paused ? 'resume-all' : 'pause-all');
      clearButtonEl.disabled = history === 0;
    }

    function render(snapshot, state) {
      latestSnapshot = snapshot;
      latestState = { viewMode: state && state.viewMode === 'history' ? 'history' : 'active', selectedId: state && state.selectedId };

      if (!snapshot) {
        recoveryEl.hidden = true;
        tableViewportEl.hidden = true;
        stateEl.hidden = false;
        setData(stateEl, 'transfer-state', 'loading');
        stateEl.textContent = 'Loading transfers…';
        return;
      }

      updateSummary(snapshot, latestState);
      recoveryEl.hidden = !snapshot.recoveryError;
      recoveryEl.textContent = snapshot.recoveryError ? `Transfer recovery error: ${snapshot.recoveryError}` : '';

      const jobs = jobsFor(snapshot, latestState.viewMode);
      const nextIds = jobs.map((job) => job.id);
      const membershipChanged = nextIds.length !== currentIds.length
        || nextIds.some((id, index) => id !== currentIds[index]);

      if (membershipChanged) {
        tbodyEl.replaceChildren();
        rowsById = new Map();
        for (const job of jobs) {
          const record = createRow(job, latestState.selectedId === job.id);
          rowsById.set(job.id, record);
          tbodyEl.appendChild(record.element);
        }
        currentIds = nextIds;
      } else {
        for (const job of jobs) {
          const record = rowsById.get(job.id);
          if (record) patchRow(record, job, latestState.selectedId === job.id);
        }
      }

      const empty = jobs.length === 0;
      tableViewportEl.hidden = empty;
      stateEl.hidden = !empty;
      if (empty) {
        setData(stateEl, 'transfer-state', 'empty');
        stateEl.textContent = latestState.viewMode === 'history' ? 'No transfer history' : 'No active transfers';
      }
    }

    function setSelection(selectedId) {
      latestState.selectedId = selectedId;
      for (const [id, record] of rowsById) {
        const selected = id === selectedId;
        record.element.setAttribute('aria-selected', selected ? 'true' : 'false');
        record.element.classList.toggle('is-selected', selected);
      }
    }

    function onClick(event) {
      const target = event && event.target;
      if (!target || typeof target.closest !== 'function') return;
      const viewButton = target.closest('[data-transfer-view]');
      if (viewButton) {
        onViewChange(viewButton.getAttribute('data-transfer-view'));
        return;
      }

      const actionButton = target.closest('[data-transfer-action]');
      if (actionButton) {
        const jobId = actionButton.getAttribute('data-job-id');
        if (jobId) onSelect(jobId);
        onAction({ action: actionButton.getAttribute('data-transfer-action'), jobId, invoker: actionButton });
        return;
      }

      const row = target.closest('tr[data-job-id]');
      if (row) onSelect(row.getAttribute('data-job-id'));
    }

    function onKeyDown(event) {
      const target = event && event.target;
      if (!target || typeof target.closest !== 'function') return;
      const tag = String(target.tagName || '').toUpperCase();
      if (tag === 'BUTTON' || tag === 'INPUT' || tag === 'SELECT' || tag === 'TEXTAREA'
          || target.getAttribute('contenteditable') === 'true') return;

      const key = event.key;
      const row = target.closest('tr[data-job-id]');
      const currentId = row ? row.getAttribute('data-job-id') : latestState.selectedId;
      if (key === 'ArrowUp' || key === 'ArrowDown') {
        if (currentIds.length === 0) return;
        const currentIndex = Math.max(0, currentIds.indexOf(currentId));
        const delta = key === 'ArrowUp' ? -1 : 1;
        const nextIndex = Math.max(0, Math.min(currentIds.length - 1, currentIndex + delta));
        const nextId = currentIds[nextIndex];
        if (typeof event.preventDefault === 'function') event.preventDefault();
        onSelect(nextId);
        const nextRow = rowsById.get(nextId);
        if (nextRow && typeof nextRow.element.focus === 'function') nextRow.element.focus();
        return;
      }

      const jobId = currentId || latestState.selectedId;
      const jobs = latestSnapshot && Array.isArray(latestSnapshot.jobs) ? latestSnapshot.jobs : [];
      const job = jobs.find((item) => item.id === jobId);
      const kind = job && job.state ? job.state.kind : '';
      let action = null;
      if (key === ' ' || key === 'Spacebar') {
        if (kind === 'running' || kind === 'queued') action = 'pause';
        else if (kind === 'paused') action = 'resume';
      } else if (key === 'Enter') {
        if (kind === 'needsAttention') action = 'resolve';
        else if (actionsFor(job).some(([name]) => name === 'details')) action = 'details';
      } else if (key === 'Delete' && actionsFor(job).some(([name]) => name === 'cancel')) {
        action = 'cancel';
      }
      if (!action) return;
      if (typeof event.preventDefault === 'function') event.preventDefault();
      if (currentId && latestState.selectedId !== currentId) onSelect(currentId);
      const invoker = rowsById.get(jobId);
      onAction({ action, jobId, invoker: invoker ? invoker.element : row });
    }

    panelEl.addEventListener('click', onClick);
    panelEl.addEventListener('keydown', onKeyDown);

    return {
      render,
      setSelection,
      destroy() {
        panelEl.removeEventListener('click', onClick);
        panelEl.removeEventListener('keydown', onKeyDown);
      },
      getSnapshot: () => latestSnapshot,
    };
  }

  global.termlabTransferCenterView = { create, jobsFor, isHistoryJob };
})(window);

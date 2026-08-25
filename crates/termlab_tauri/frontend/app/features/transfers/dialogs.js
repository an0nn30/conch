(function initTermLabTransferDialogs(global) {
  'use strict';

  function append(parent, tag, className, text) {
    const element = document.createElement(tag);
    if (className) element.className = className;
    if (text !== undefined) element.textContent = String(text);
    parent.appendChild(element);
    return element;
  }

  function connected(element) {
    return !!element && element.isConnected !== false;
  }

  function createFocusReturn(invoker) {
    const transferRow = invoker && typeof invoker.closest === 'function'
      ? invoker.closest('tr[data-job-id]')
      : null;
    const fileRow = !transferRow && invoker && typeof invoker.closest === 'function'
      ? invoker.closest('tr[data-name]')
      : null;
    const row = transferRow || fileRow;
    const rowSelector = transferRow ? 'tr[data-job-id]' : 'tr[data-name]';
    const keyAttribute = transferRow ? 'data-job-id' : 'data-name';
    const panelSelector = transferRow ? '.tl-transfer-center' : '.fp-pane';
    const panel = row && typeof row.closest === 'function' ? row.closest(panelSelector) : null;
    const rowKey = row ? row.getAttribute(keyAttribute) : null;
    const rowsAtOpen = panel ? Array.from(panel.querySelectorAll(rowSelector)) : [];
    const rowIndex = Math.max(0, rowsAtOpen.indexOf(row));

    return () => {
      if (connected(invoker) && typeof invoker.focus === 'function') {
        invoker.focus();
        return;
      }
      if (!connected(panel)) return;
      const currentRows = Array.from(panel.querySelectorAll(rowSelector));
      const keyedRow = currentRows.find((candidate) => candidate.getAttribute(keyAttribute) === rowKey);
      let fallback = keyedRow || currentRows[Math.min(rowIndex, Math.max(0, currentRows.length - 1))] || panel;
      if (fileRow && keyedRow) {
        fallback = keyedRow.querySelector('.fp-transfer-attention[data-transfer-id]') || keyedRow;
      }
      if (fallback && typeof fallback.focus === 'function') fallback.focus();
    };
  }

  function openDialog(options, invoker, clearInputs) {
    if (!global.tlDialog || typeof global.tlDialog.open !== 'function') return null;
    const inputs = Array.isArray(clearInputs) ? clearInputs : [];
    const returnFocus = createFocusReturn(invoker);
    const opts = { ...options };
    const callerOnClose = opts.onClose;
    opts.onClose = (result) => {
      for (const input of inputs) input.value = '';
      if (typeof callerOnClose === 'function') callerOnClose(result);
      returnFocus();
    };
    return global.tlDialog.open(opts);
  }

  function endpointLabel(endpoint) {
    if (!endpoint) return 'Unknown endpoint';
    if (endpoint.kind === 'configured') return endpoint.label || endpoint.serverEntryId || 'Configured host';
    if (endpoint.kind === 'adHoc') {
      const user = endpoint.user ? `${endpoint.user}@` : '';
      const port = endpoint.port ? `:${endpoint.port}` : '';
      return `${user}${endpoint.host || 'Unknown host'}${port}`;
    }
    return endpoint.label || endpoint.host || 'Unknown endpoint';
  }

  function addDetail(bodyEl, label, value) {
    if (value === null || value === undefined || value === '') return;
    const row = append(bodyEl, 'div', 'tl-field');
    append(row, 'span', 'tl-field__label', label);
    append(row, 'div', 'tl-transfer-dialog__value', value);
  }

  function closeAfter(handleRef, callback, value) {
    return Promise.resolve(typeof callback === 'function' ? callback(value) : undefined)
      .then(() => {
        if (handleRef.current) handleRef.current.close('confirm');
      });
  }

  function showCancel(job, invoker, onConfirm) {
    const handleRef = { current: null };
    handleRef.current = openDialog({
      title: 'Cancel transfer?',
      ariaLabel: 'Confirm cancel transfer',
      size: 'sm',
      body(bodyEl) {
        append(bodyEl, 'p', '', 'This stops the transfer and may remove its managed partial file.');
        append(bodyEl, 'p', 'tl-transfer-dialog__filename', job && job.fileName ? job.fileName : 'Unnamed transfer');
      },
      buttons: [
        { label: 'Keep transfer', onSelect: () => handleRef.current && handleRef.current.close('cancel') },
        { label: 'Cancel transfer', danger: true, onSelect: () => closeAfter(handleRef, onConfirm) },
      ],
    }, invoker);
    return handleRef.current;
  }

  function showDetails(job, invoker) {
    const handleRef = { current: null };
    const state = (job && job.state) || {};
    handleRef.current = openDialog({
      title: 'Transfer details',
      ariaLabel: 'Transfer details',
      size: 'md',
      body(bodyEl) {
        addDetail(bodyEl, 'Endpoint', endpointLabel(job && job.endpoint));
        addDetail(bodyEl, 'Local path', job && job.localPath);
        addDetail(bodyEl, 'Remote path', job && job.remotePath);
        addDetail(bodyEl, 'Created', job && job.createdAtMs);
        addDetail(bodyEl, 'Updated', job && job.updatedAtMs);
        addDetail(bodyEl, 'Checkpoint', job && job.durableCheckpoint);
        addDetail(bodyEl, 'Error', state.error || state.cleanupError || state.message);
      },
      buttons: [{ label: 'Close', primary: true, onSelect: () => handleRef.current && handleRef.current.close('close') }],
    }, invoker);
    return handleRef.current;
  }

  function attentionPresentation(reason) {
    switch (reason && reason.kind) {
      case 'destinationConflict':
        return {
          title: 'Destination conflict',
          ariaLabel: 'Resolve destination conflict',
          description: 'The destination already exists. Overwrite permanently replaces it.',
          restartable: false,
        };
      case 'sourceChanged':
        return {
          title: 'Source changed',
          ariaLabel: 'Resolve changed source',
          description: 'The source changed after this transfer started. Restart from the beginning or skip it.',
          restartable: true,
        };
      case 'sourceCannotResume':
        return {
          title: 'Resume unavailable',
          ariaLabel: 'Resolve transfer that cannot resume',
          description: 'The source identity cannot be verified, so this transfer cannot be safely resumed.',
          restartable: true,
        };
      case 'sourceMissing':
        return {
          title: 'Source missing',
          ariaLabel: 'Resolve missing transfer source',
          description: 'The source path no longer exists. Restore it and restart from the beginning, or skip this transfer.',
          restartable: true,
        };
      case 'missingPartial':
        return {
          title: 'Partial file missing',
          ariaLabel: 'Resolve missing transfer partial',
          description: 'The managed partial file is missing. Restart from the beginning or skip this transfer.',
          restartable: true,
        };
      case 'commitRecovery':
        return {
          title: 'Commit recovery required',
          ariaLabel: 'Resolve transfer commit recovery',
          description: 'TermLab found an interrupted destination commit that needs an explicit decision.',
          restartable: true,
        };
      case 'cleanup':
        return {
          title: 'Cleanup required',
          ariaLabel: 'Resolve transfer cleanup',
          description: 'TermLab could not finish cleaning up managed transfer artifacts.',
          restartable: true,
        };
      default:
        return {
          title: 'Transfer needs attention',
          ariaLabel: 'Transfer needs attention',
          description: 'This transfer reported an unsupported attention reason. Review Details before continuing.',
          restartable: false,
        };
    }
  }

  function showConflict(job, invoker, onResolve) {
    const handleRef = { current: null };
    const inputs = [];
    const reason = job && job.state && job.state.reason ? job.state.reason : {};
    const destinationConflict = reason.kind === 'destinationConflict';
    const presentation = attentionPresentation(reason);
    let renameInput = null;
    let renameError = null;

    const resolveAndClose = (resolution) => closeAfter(handleRef, onResolve, resolution);
    const buttons = [{ label: 'Cancel', onSelect: () => handleRef.current && handleRef.current.close('cancel') }];
    if (destinationConflict) {
      buttons.push({ label: 'Skip', onSelect: () => resolveAndClose({ kind: 'skip' }) });
      if (reason.resumeAvailable === true) {
        buttons.push({ label: 'Resume', onSelect: () => resolveAndClose({ kind: 'resume' }) });
      }
      buttons.push({
        label: 'Rename',
        onSelect: () => {
          const destination = renameInput ? String(renameInput.value || '').trim() : '';
          if (!destination) {
            if (renameError) renameError.hidden = false;
            if (renameInput && typeof renameInput.focus === 'function') renameInput.focus();
            return Promise.resolve();
          }
          if (renameError) renameError.hidden = true;
          return resolveAndClose({ kind: 'rename', destination });
        },
      });
      buttons.push({ label: 'Overwrite', danger: true, onSelect: () => resolveAndClose({ kind: 'overwrite' }) });
    } else if (presentation.restartable) {
      buttons.push({ label: 'Skip', onSelect: () => resolveAndClose({ kind: 'skip' }) });
      buttons.push({ label: 'Restart', primary: true, onSelect: () => resolveAndClose({ kind: 'restart' }) });
    }

    handleRef.current = openDialog({
      title: presentation.title,
      ariaLabel: presentation.ariaLabel,
      size: 'md',
      body(bodyEl) {
        append(bodyEl, 'p', '', presentation.description);
        if (destinationConflict) {
          addDetail(bodyEl, 'Destination', job && (job.direction === 'upload' ? job.remotePath : job.localPath));
          const field = append(bodyEl, 'label', 'tl-field');
          append(field, 'span', 'tl-field__label', 'Rename destination');
          renameInput = append(field, 'input', 'tl-input');
          renameInput.setAttribute('type', 'text');
          renameInput.setAttribute('data-transfer-field', 'rename');
          renameInput.setAttribute('autocomplete', 'off');
          inputs.push(renameInput);
          renameError = append(field, 'span', 'tl-field__error', 'Enter a non-empty destination path.');
          renameError.setAttribute('data-transfer-error', 'rename');
          renameError.setAttribute('role', 'alert');
          renameError.hidden = true;
        }
        if (reason.kind === 'sourceChanged') {
          addDetail(bodyEl, 'Expected size', reason.expected && reason.expected.size);
          addDetail(bodyEl, 'Current size', reason.actual && reason.actual.size);
        }
        if (reason.kind === 'commitRecovery' || reason.kind === 'cleanup') {
          addDetail(bodyEl, 'Backend message', reason.message);
        }
      },
      buttons,
    }, invoker, inputs);
    return handleRef.current;
  }

  function showConcurrency(settings, invoker, onSave) {
    const handleRef = { current: null };
    const inputs = [];
    let globalInput = null;
    let hostInput = null;
    let depthInput = null;
    let chunkInput = null;
    let errorEl = null;

    function limitField(bodyEl, label, dataName, value, min, max) {
      const field = append(bodyEl, 'label', 'tl-field');
      append(field, 'span', 'tl-field__label', label);
      const input = append(field, 'input', 'tl-input');
      input.setAttribute('type', 'number');
      input.setAttribute('min', String(min));
      input.setAttribute('max', String(max));
      input.setAttribute('step', '1');
      input.setAttribute('data-transfer-field', dataName);
      input.value = String(value);
      inputs.push(input);
      return input;
    }

    function integerLimit(input, min, max) {
      const text = String(input && input.value !== undefined ? input.value : '').trim();
      const value = Number(text);
      return text !== '' && Number.isInteger(value) && value >= min && value <= max ? value : null;
    }

    handleRef.current = openDialog({
      title: 'Transfer concurrency',
      ariaLabel: 'Transfer concurrency settings',
      size: 'sm',
      body(bodyEl) {
        const current = settings || {};
        globalInput = limitField(bodyEl, 'All hosts', 'global-limit', current.globalLimit || 1, 1, 32);
        hostInput = limitField(bodyEl, 'Per host', 'per-host-limit', current.perHostLimit || 1, 1, 32);
        depthInput = limitField(bodyEl, 'Pipeline depth', 'pipeline-depth',
          current.pipelineDepth || 16, 1, 64);
        // The engine clamps chunks to the raw SFTP cap (255 KiB) unless the
        // server advertises a higher limit, so the field says so rather than
        // silently accepting a value it will not use.
        chunkInput = limitField(bodyEl, 'Chunk size (KiB, servers may cap at 255)', 'pipeline-chunk-kib',
          Math.round((current.pipelineChunkBytes || 262144) / 1024), 32, 1024);
        errorEl = append(bodyEl, 'div', 'tl-field__error', "Enter whole numbers within each field's range.");
        errorEl.setAttribute('data-transfer-error', 'concurrency');
        errorEl.setAttribute('role', 'alert');
        errorEl.hidden = true;
      },
      buttons: [
        { label: 'Cancel', onSelect: () => handleRef.current && handleRef.current.close('cancel') },
        {
          label: 'Save',
          primary: true,
          onSelect: () => {
            const globalLimit = integerLimit(globalInput, 1, 32);
            const perHostLimit = integerLimit(hostInput, 1, 32);
            const pipelineDepth = integerLimit(depthInput, 1, 64);
            const chunkKib = integerLimit(chunkInput, 32, 1024);
            if (globalLimit === null || perHostLimit === null || pipelineDepth === null || chunkKib === null) {
              if (errorEl) errorEl.hidden = false;
              return Promise.resolve();
            }
            if (errorEl) errorEl.hidden = true;
            return closeAfter(handleRef, onSave, {
              globalLimit, perHostLimit,
              pipelineDepth,
              pipelineChunkBytes: chunkKib * 1024,
            });
          },
        },
      ],
    }, invoker, inputs);
    return handleRef.current;
  }

  function showAdHocReconnect(job, invoker, onRequeue) {
    const handleRef = { current: null };
    const endpoint = (job && job.endpoint) || {};
    const match = `${endpoint.user || '<user>'}@${endpoint.host || '<host>'}:${endpoint.port || 22}`;
    handleRef.current = openDialog({
      title: 'Reconnect transfer host',
      ariaLabel: 'Reconnect ad-hoc transfer host',
      size: 'sm',
      body(bodyEl) {
        append(bodyEl, 'p', '', `Reconnect a matching ${match} session, then choose Requeue transfer.`);
        append(bodyEl, 'p', '', 'TermLab will not store credentials or reconnect an ad-hoc endpoint automatically.');
      },
      buttons: [
        { label: 'Close', onSelect: () => handleRef.current && handleRef.current.close('close') },
        {
          label: 'Requeue transfer',
          primary: true,
          onSelect: () => closeAfter(handleRef, onRequeue),
        },
      ],
    }, invoker);
    return handleRef.current;
  }

  function showConnectError(job, error, invoker) {
    const handleRef = { current: null };
    handleRef.current = openDialog({
      title: 'Could not reconnect',
      ariaLabel: 'Transfer reconnect error',
      size: 'sm',
      body(bodyEl) {
        addDetail(bodyEl, 'Endpoint', endpointLabel(job && job.endpoint));
        addDetail(bodyEl, 'Error', error && error.message ? error.message : String(error || 'Unknown error'));
      },
      buttons: [{ label: 'Close', primary: true, onSelect: () => handleRef.current && handleRef.current.close('close') }],
    }, invoker);
    return handleRef.current;
  }

  global.termlabTransferDialogs = {
    showCancel,
    showDetails,
    showConflict,
    showConcurrency,
    showAdHocReconnect,
    showConnectError,
  };
})(window);

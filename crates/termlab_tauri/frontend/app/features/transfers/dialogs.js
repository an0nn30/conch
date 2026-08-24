(function initTermLabTransferDialogs(global) {
  'use strict';

  function append(parent, tag, className, text) {
    const element = document.createElement(tag);
    if (className) element.className = className;
    if (text !== undefined) element.textContent = String(text);
    parent.appendChild(element);
    return element;
  }

  function restoreFocus(invoker) {
    if (invoker && typeof invoker.focus === 'function') invoker.focus();
  }

  function openDialog(options, invoker, clearInputs) {
    if (!global.tlDialog || typeof global.tlDialog.open !== 'function') return null;
    const inputs = Array.isArray(clearInputs) ? clearInputs : [];
    const opts = { ...options };
    const callerOnClose = opts.onClose;
    opts.onClose = (result) => {
      for (const input of inputs) input.value = '';
      if (typeof callerOnClose === 'function') callerOnClose(result);
      restoreFocus(invoker);
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

  function showConflict(job, invoker, onResolve) {
    const handleRef = { current: null };
    const inputs = [];
    const reason = job && job.state && job.state.reason ? job.state.reason : {};
    const destinationConflict = reason.kind === 'destinationConflict';
    let renameInput = null;
    let renameError = null;

    const resolveAndClose = (resolution) => closeAfter(handleRef, onResolve, resolution);
    const buttons = [{ label: 'Cancel', onSelect: () => handleRef.current && handleRef.current.close('cancel') }];
    buttons.push({ label: 'Skip', onSelect: () => resolveAndClose({ kind: 'skip' }) });
    if (destinationConflict) {
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
    } else {
      buttons.push({ label: 'Restart', primary: true, onSelect: () => resolveAndClose({ kind: 'restart' }) });
    }

    handleRef.current = openDialog({
      title: destinationConflict ? 'Destination conflict' : 'Source changed',
      ariaLabel: destinationConflict ? 'Resolve destination conflict' : 'Resolve changed source',
      size: 'md',
      body(bodyEl) {
        if (destinationConflict) {
          append(bodyEl, 'p', '', 'The destination already exists. Overwrite permanently replaces it.');
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
        } else {
          append(bodyEl, 'p', '', 'The source changed after this transfer started. Restart from the beginning or skip it.');
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
    let errorEl = null;

    function limitField(bodyEl, label, dataName, value) {
      const field = append(bodyEl, 'label', 'tl-field');
      append(field, 'span', 'tl-field__label', label);
      const input = append(field, 'input', 'tl-input');
      input.setAttribute('type', 'number');
      input.setAttribute('min', '1');
      input.setAttribute('max', '32');
      input.setAttribute('step', '1');
      input.setAttribute('data-transfer-field', dataName);
      input.value = String(value);
      inputs.push(input);
      return input;
    }

    function integerLimit(input) {
      const text = String(input && input.value !== undefined ? input.value : '').trim();
      const value = Number(text);
      return text !== '' && Number.isInteger(value) && value >= 1 && value <= 32 ? value : null;
    }

    handleRef.current = openDialog({
      title: 'Transfer concurrency',
      ariaLabel: 'Transfer concurrency settings',
      size: 'sm',
      body(bodyEl) {
        const current = settings || {};
        globalInput = limitField(bodyEl, 'All hosts', 'global-limit', current.globalLimit || 1);
        hostInput = limitField(bodyEl, 'Per host', 'per-host-limit', current.perHostLimit || 1);
        errorEl = append(bodyEl, 'div', 'tl-field__error', 'Enter whole numbers from 1 to 32 for both limits.');
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
            const globalLimit = integerLimit(globalInput);
            const perHostLimit = integerLimit(hostInput);
            if (globalLimit === null || perHostLimit === null) {
              if (errorEl) errorEl.hidden = false;
              return Promise.resolve();
            }
            if (errorEl) errorEl.hidden = true;
            return closeAfter(handleRef, onSave, { globalLimit, perHostLimit });
          },
        },
      ],
    }, invoker, inputs);
    return handleRef.current;
  }

  function showAdHocReconnect(job, invoker) {
    const handleRef = { current: null };
    const endpoint = (job && job.endpoint) || {};
    const match = `${endpoint.user || '<user>'}@${endpoint.host || '<host>'}:${endpoint.port || 22}`;
    handleRef.current = openDialog({
      title: 'Reconnect transfer host',
      ariaLabel: 'Reconnect ad-hoc transfer host',
      size: 'sm',
      body(bodyEl) {
        append(bodyEl, 'p', '', `Reconnect a matching ${match} session, then choose Resume for this transfer.`);
        append(bodyEl, 'p', '', 'TermLab will not store credentials or reconnect an ad-hoc endpoint automatically.');
      },
      buttons: [{ label: 'Close', primary: true, onSelect: () => handleRef.current && handleRef.current.close('close') }],
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

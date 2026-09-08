// Slim, non-modal project context/status control for editor panes.
(function initTermLabProjectContext(global) {
  'use strict';

  const EXECUTABLE_FAMILY = {
    typescript: 'typescript-language-server',
    json: 'vscode-json-languageserver',
    python: 'pyright',
    rust: 'rust-analyzer',
    go: 'gopls',
    clangd: 'clangd',
    java: 'Eclipse JDT Language Server',
  };
  const controls = new WeakMap();

  function statusOf(state) {
    return state && state.status ? state.status : { state: 'disabled' };
  }

  function clearCandidate(state) {
    const candidates = state && Array.isArray(state.projectCandidates) ? state.projectCandidates : [];
    const evidence = candidates.filter((candidate) => !candidate.isFallback);
    if (evidence.length === 1) return evidence[0];
    if (evidence.length === 0 && candidates.length === 1) return candidates[0];
    return null;
  }

  function presentation(state) {
    if (!state) return { text: 'Plain text', detail: 'Language features are not attached', tone: 'quiet' };
    const status = statusOf(state);
    switch (status.state) {
      case 'choosingProject': {
        const candidates = state.projectCandidates || [];
        if (!candidates.length) return { text: 'Loose file', detail: 'No project selected', tone: 'quiet' };
        const inferred = clearCandidate(state);
        if (inferred) return { text: inferred.displayName, detail: inferred.reason, tone: 'attention' };
        return { text: 'Choose project…', detail: 'Several project roots match this file', tone: 'attention' };
      }
      case 'disabled':
        return { text: 'Project features off', detail: 'Editing without language features', tone: 'quiet' };
      case 'untrusted':
        return { text: 'Trust required', detail: 'Review this project before starting its language server', tone: 'attention' };
      case 'starting':
        return { text: 'Starting language server…', detail: status.message || 'Starting', tone: 'progress' };
      case 'indexing':
        return { text: 'Indexing project…', detail: status.message || 'Language server is indexing', tone: 'progress' };
      case 'ready':
        return { text: 'Language features ready', detail: status.message || 'Ready', tone: 'ready' };
      case 'failed':
        return { text: 'Language server failed', detail: status.message || 'Use Retry to start it again', tone: 'error' };
      case 'unavailable':
        return { text: 'Language server unavailable', detail: status.message || 'This adapter is unavailable in this build', tone: 'error' };
      default:
        return { text: 'Plain text', detail: 'Editing continues without language features', tone: 'quiet' };
    }
  }

  function projectChoices(state) {
    const candidates = state && Array.isArray(state.projectCandidates) ? state.projectCandidates : [];
    const choices = candidates.filter((candidate) => !candidate.isFallback).map((candidate) => ({
      label: candidate.displayName,
      reason: candidate.reason,
      context: { kind: 'root', root: candidate.canonicalPath },
      candidate,
    }));
    const fallback = candidates.find((candidate) => candidate.isFallback) || null;
    choices.push({
      label: 'This folder',
      reason: 'Use this file’s containing folder',
      context: fallback ? { kind: 'root', root: fallback.canonicalPath } : { kind: 'deferForSession' },
      candidate: fallback,
    });
    choices.push({
      label: 'No project features',
      reason: 'Keep editing without language features',
      context: { kind: 'disabled' },
      candidate: null,
    });
    return choices;
  }

  function canonicalRoot(state) {
    if (!state) return null;
    const status = statusOf(state);
    const candidates = state.projectCandidates || [];
    const match = candidates.find((candidate) => (
      candidate.rootUri === status.projectRootUri
      || candidate.canonicalPath === state.selectedRoot
    ));
    if (match) return match.canonicalPath;
    const inferred = clearCandidate(state);
    return inferred ? inferred.canonicalPath : state.selectedRoot;
  }

  function trustDisclosure(state) {
    const status = statusOf(state);
    return {
      executableFamily: EXECUTABLE_FAMILY[status.adapterId] || status.adapterId || 'language server',
      canonicalRoot: canonicalRoot(state),
    };
  }

  function actions(state) {
    const status = statusOf(state);
    switch (status.state) {
      case 'untrusted':
        return [
          { label: 'Trust and enable', id: 'trust' },
          { label: 'Not now', id: 'defer' },
          { label: 'Edit without language features', id: 'disable' },
          { label: 'Change project…', id: 'change' },
        ];
      case 'ready':
      case 'indexing':
        return [
          { label: 'Change project…', id: 'change' },
          { label: 'Restart language server', id: 'restart' },
          { label: 'View server logs', id: 'logs' },
          { label: 'Revoke trust', id: 'revoke' },
        ];
      case 'failed':
        return [
          { label: 'Retry', id: 'restart' },
          { label: 'View server logs', id: 'logs' },
          { label: 'Change project…', id: 'change' },
          { label: 'Revoke trust', id: 'revoke' },
        ];
      case 'starting':
        return [
          { label: 'Edit without language features', id: 'disable' },
          { label: 'Change project…', id: 'change' },
        ];
      case 'unavailable':
        return [
          { label: 'Edit without language features', id: 'disable' },
          { label: 'Change project…', id: 'change' },
        ];
      default:
        return [{ label: 'Change project…', id: 'change' }];
    }
  }

  function currentState(pane) {
    const store = global.termlabLspState;
    return store && typeof store.get === 'function' ? store.get(pane) : null;
  }

  async function bridgeCall(name, args) {
    const bridge = global.termlabLspBridge;
    if (!bridge || typeof bridge[name] !== 'function') {
      const error = { message: 'Language features are unavailable; editing continues.' };
      if (global.toast && typeof global.toast.error === 'function') {
        global.toast.error('Language Features', error.message);
      }
      return { ok: false, error };
    }
    try {
      return { ok: true, value: await bridge[name](...args) };
    } catch (cause) {
      console.warn(`Project context: ${name} failed`, cause);
      const error = typeof bridge.normalizeError === 'function'
        ? bridge.normalizeError(cause, name)
        : { message: 'Language features are unavailable; editing continues.', detail: String(cause) };
      if (global.toast && typeof global.toast.error === 'function') {
        global.toast.error('Language Features', `${error.message} ${error.detail || ''}`.trim());
      }
      return { ok: false, error };
    }
  }

  function chooseProject(pane, anchor) {
    const state = currentState(pane);
    if (!state || !global.tlMenu) return;
    const rect = anchor.getBoundingClientRect();
    const items = projectChoices(state).map((choice) => ({
      label: `${choice.label} — ${choice.reason}`,
      title: choice.reason,
      onSelect: () => bridgeCall('setProjectContext', [state.documentId, choice.context]),
    }));
    global.tlMenu.open({
      x: rect.left,
      y: rect.bottom,
      ariaLabel: 'Choose project context',
      routerName: 'editor-project-context',
      items,
    });
  }

  function trustProject(pane) {
    const state = currentState(pane);
    if (!state || !global.tlDialog) return;
    const disclosure = trustDisclosure(state);
    const status = statusOf(state);
    let handle = null;
    handle = global.tlDialog.open({
      title: 'Trust this project?',
      size: 'sm',
      body(body) {
        const summary = document.createElement('div');
        summary.className = 'editor-project-trust';
        summary.textContent = `${disclosure.executableFamily} will run for ${disclosure.canonicalRoot}.`;
        body.appendChild(summary);
      },
      buttons: [
        { label: 'Not now', onSelect: () => handle.close('defer') },
        {
          label: 'Trust and enable', primary: true,
          onSelect: async () => {
            const result = await bridgeCall('setProjectTrust', [disclosure.canonicalRoot, status.adapterId, 'trusted']);
            if (result.ok) handle.close('trusted');
          },
        },
      ],
    });
  }

  function runAction(pane, action, anchor) {
    const state = currentState(pane);
    if (!state) return;
    const status = statusOf(state);
    const root = canonicalRoot(state);
    if (action.id === 'change') chooseProject(pane, anchor);
    else if (action.id === 'trust') trustProject(pane);
    else if (action.id === 'defer') bridgeCall('setProjectContext', [state.documentId, { kind: 'deferForSession' }]);
    else if (action.id === 'disable') bridgeCall('setProjectContext', [state.documentId, { kind: 'disabled' }]);
    else if (action.id === 'restart') bridgeCall('restartSession', [status.adapterId, root]);
    else if (action.id === 'revoke') bridgeCall('revokeProjectTrust', [root, status.adapterId]);
    else if (action.id === 'logs') {
      bridgeCall('sessionLogs', [status.adapterId, root]).then((result) => {
        if (!result.ok) return;
        const logs = result.value;
        const text = Array.isArray(logs) && logs.length
          ? logs.map((entry) => `${entry.kind}: ${entry.message}`).join('\n')
          : 'No language server logs are available.';
        if (global.toast && typeof global.toast.info === 'function') global.toast.info('Language Server Logs', text);
      });
    }
  }

  function openActions(pane, button) {
    const state = currentState(pane);
    if (!state || !global.tlMenu) return;
    const rect = button.getBoundingClientRect();
    global.tlMenu.open({
      x: rect.left,
      y: rect.bottom,
      ariaLabel: 'Project actions',
      routerName: 'editor-project-actions',
      items: actions(state).map((action) => ({
        label: action.label,
        onSelect: () => runAction(pane, action, button),
      })),
    });
  }

  function refresh(pane) {
    const entry = controls.get(pane);
    if (!entry) return;
    const shown = presentation(currentState(pane));
    entry.button.textContent = shown.text;
    entry.button.title = shown.detail;
    entry.button.dataset.state = shown.tone;
    entry.button.setAttribute('aria-label', `Project context: ${shown.text}`);
  }

  function mount(host, pane) {
    if (!host || !pane) return null;
    const wrap = document.createElement('div');
    wrap.className = 'editor-project-status';
    const button = document.createElement('button');
    button.type = 'button';
    button.className = 'editor-project-status__button';
    button.addEventListener('click', () => {
      const shown = presentation(currentState(pane));
      if (shown.text === 'Choose project…' || shown.text === 'Loose file') chooseProject(pane, button);
      else openActions(pane, button);
    });
    wrap.appendChild(button);
    host.appendChild(wrap);
    const store = global.termlabLspState;
    const unsubscribe = store && typeof store.subscribe === 'function'
      ? store.subscribe((changed) => { if (changed === pane) refresh(pane); })
      : function () {};
    controls.set(pane, { wrap, button, unsubscribe });
    refresh(pane);
    return button;
  }

  function unmount(pane) {
    const entry = controls.get(pane);
    if (!entry) return;
    entry.unsubscribe();
    entry.wrap.remove();
    controls.delete(pane);
  }

  function formatLastUsed(timestamp) {
    if (!timestamp) return 'Never used';
    try { return new Date(timestamp).toLocaleString(); } catch (_) { return 'Unknown'; }
  }

  async function renderTrustedProjects(container, helpers) {
    const h = helpers || {};
    if (!container || typeof h.addSectionLabel !== 'function' || typeof h.addRow !== 'function') return;
    h.addSectionLabel(container, 'Trusted Projects');
    const result = await bridgeCall('trustedProjects', []);
    if (!result.ok) {
      const unavailable = document.createElement('span');
      unavailable.className = 'tl-settings__row-desc';
      unavailable.textContent = 'Trusted projects unavailable';
      h.addRow(container, 'Project trust', 'Could not load trusted projects; reopen Settings to retry', unavailable);
      return;
    }
    const projects = result.value;
    if (!Array.isArray(projects) || !projects.length) {
      const empty = document.createElement('span');
      empty.className = 'tl-settings__row-desc';
      empty.textContent = 'No trusted projects';
      h.addRow(container, 'Project trust', 'Projects appear here after an explicit trust decision', empty);
      return;
    }
    for (const project of projects) {
      const button = document.createElement('button');
      button.type = 'button';
      button.className = 'tl-btn';
      button.textContent = 'Revoke trust';
      button.addEventListener('click', async () => {
        button.disabled = true;
        const revoked = await bridgeCall('revokeProjectTrust', [project.root, project.adapterId]);
        if (revoked.ok) {
          button.textContent = 'Revoked';
        } else {
          button.disabled = false;
          button.textContent = 'Revoke trust';
        }
      });
      const adapter = project.adapterId || 'All adapters';
      const context = project.root;
      const description = `Context: ${context} · Adapter: ${adapter} · ${project.decision} · Last used ${formatLastUsed(project.lastUsedAtMs)}`;
      h.addRow(container, project.root, description, button);
    }
  }

  global.termlabProjectContext = {
    presentation,
    projectChoices,
    actions,
    trustDisclosure,
    mount,
    unmount,
    refresh,
    renderTrustedProjects,
  };
})(window);

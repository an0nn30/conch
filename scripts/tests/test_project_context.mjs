// Run: node scripts/tests/test_project_context.mjs
import assert from 'node:assert';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';

const APP = path.resolve(import.meta.dirname, '../../crates/termlab_tauri/frontend/app');
let ran = 0;
let failures = 0;
async function check(name, fn) {
  ran += 1;
  try { await fn(); console.log(`  ok ${name}`); }
  catch (error) { failures += 1; console.error(`  FAIL ${name}\n       ${error.stack || error.message}`); }
}

class Element {
  constructor(tag) {
    this.tagName = String(tag).toUpperCase();
    this.children = [];
    this.dataset = {};
    this.style = {};
    this.listeners = {};
    this.textContent = '';
    this.className = '';
    this.title = '';
    this.disabled = false;
    this.isConnected = true;
  }
  appendChild(child) { this.children.push(child); child.parentNode = this; return child; }
  remove() { this.isConnected = false; if (this.parentNode) this.parentNode.children = this.parentNode.children.filter((c) => c !== this); }
  addEventListener(type, fn) { this.listeners[type] = fn; }
  setAttribute(name, value) { this[name] = String(value); }
  focus() { this.focused = true; }
  click() { if (this.listeners.click) this.listeners.click({ currentTarget: this }); }
  getBoundingClientRect() { return { left: 10, bottom: 20 }; }
}

function state(status, candidates = []) {
  return {
    documentId: 'doc-1', version: 1, projectCandidates: candidates,
    selectedRoot: status.projectRootUri || null, trust: null,
    capabilities: status.capabilities || {}, status, diagnosticsRevision: 0,
  };
}

function makeSandbox(options = {}) {
  const menuCalls = [];
  const dialogCalls = [];
  const bridgeCalls = [];
  const toasts = { error: [], info: [] };
  const closedDialogs = [];
  const sandbox = {
    console, setTimeout, clearTimeout,
    document: { createElement: (tag) => new Element(tag), body: new Element('body') },
    innerWidth: 1000,
    tlMenu: { open: (opts) => { menuCalls.push(opts); return new Element('div'); } },
    toast: {
      error: (title, body) => toasts.error.push([title, body]),
      info: (title, body) => toasts.info.push([title, body]),
    },
    tlDialog: {
      open: (opts) => {
        dialogCalls.push(opts);
        return { close(reason) { closedDialogs.push(reason); } };
      },
    },
    termlabServices: {
      tauriClient: {
        async invoke(command, args) {
          bridgeCalls.push([command, args]);
          if (options.rejectCommand === command) throw new Error(`failed ${command}`);
          if (command === 'lsp_trusted_projects') return [{
            root: '/repo', rootUri: 'file:///repo', adapterId: 'typescript', decision: 'trusted',
            updatedAtMs: 1, lastUsedAtMs: 1700000000000,
          }];
          if (command === 'lsp_session_logs') return [];
          return null;
        },
        listen() { return Promise.resolve(() => {}); },
      },
    },
  };
  sandbox.window = sandbox;
  vm.createContext(sandbox);
  for (const relative of ['features/editor/lsp-state.js', 'features/editor/lsp-bridge.js']) {
    const dependency = path.join(APP, relative);
    vm.runInContext(fs.readFileSync(dependency, 'utf8'), sandbox, { filename: dependency });
  }
  const filename = path.join(APP, 'features/editor/project-context.js');
  vm.runInContext(fs.readFileSync(filename, 'utf8'), sandbox, { filename });
  return {
    sandbox, api: sandbox.termlabProjectContext, store: sandbox.termlabLspState,
    menuCalls, dialogCalls, bridgeCalls, toasts, closedDialogs,
  };
}

function attachPane(store, pane, paneState) {
  store.attach(pane, {
    documentId: paneState.documentId,
    version: paneState.version,
    projectCandidates: paneState.projectCandidates,
    status: paneState.status,
  });
}

const clearCandidate = {
  rootUri: 'file:///repo', canonicalPath: '/repo', displayName: 'repo', marker: 'package.json',
  reason: 'package.json identifies a JavaScript project', confidence: 90, isFallback: false,
};
const nestedCandidate = {
  rootUri: 'file:///repo/packages/app', canonicalPath: '/repo/packages/app', displayName: 'app', marker: 'tsconfig.json',
  reason: 'tsconfig.json identifies a TypeScript project', confidence: 100, isFallback: false,
};
const folderFallback = {
  rootUri: 'file:///repo/packages/app/src', canonicalPath: '/repo/packages/app/src', displayName: 'src', marker: 'This folder',
  reason: 'Use this file’s parent folder', confidence: 20, isFallback: true,
};
const capabilities = { completion: true, hover: true, signatureHelp: true, definition: true, diagnostics: true };

await check('status text covers every non-blocking project/server state', async () => {
  const { api } = makeSandbox();
  const cases = [
    [state({ state: 'choosingProject', adapterId: 'typescript', capabilities }, []), 'Loose file'],
    [state({ state: 'choosingProject', adapterId: 'typescript', capabilities }, [clearCandidate]), 'repo'],
    [state({ state: 'choosingProject', adapterId: 'typescript', capabilities }, [clearCandidate, nestedCandidate]), 'Choose project…'],
    [state({ state: 'disabled', adapterId: 'typescript', capabilities }), 'Project features off'],
    [state({ state: 'untrusted', adapterId: 'typescript', projectRootUri: 'file:///repo', capabilities }, [clearCandidate]), 'Trust required'],
    [state({ state: 'starting', adapterId: 'typescript', projectRootUri: 'file:///repo', capabilities }), 'Starting language server…'],
    [state({ state: 'ready', adapterId: 'typescript', projectRootUri: 'file:///repo', capabilities }), 'Language features ready'],
    [state({ state: 'failed', adapterId: 'typescript', projectRootUri: 'file:///repo', capabilities }), 'Language server failed'],
    [state({ state: 'unavailable', adapterId: 'java', unavailableReason: { kind: 'notBundledYet', adapterId: 'java' }, capabilities }), 'Language server unavailable'],
  ];
  for (const [input, expected] of cases) assert.equal(api.presentation(input).text, expected);
});

await check('chooser preserves reasoned candidates and always ends with explicit fallbacks', async () => {
  const { api } = makeSandbox();
  const choices = api.projectChoices(state(
    { state: 'choosingProject', adapterId: 'typescript', capabilities },
    [nestedCandidate, clearCandidate, folderFallback],
  ));
  assert.deepEqual(JSON.parse(JSON.stringify(choices.map((item) => ({ label: item.label, reason: item.reason })))), [
    { label: 'app', reason: 'tsconfig.json identifies a TypeScript project' },
    { label: 'repo', reason: 'package.json identifies a JavaScript project' },
    { label: 'This folder', reason: 'Use this file’s containing folder' },
    { label: 'No project features', reason: 'Keep editing without language features' },
  ]);
});

await check('state action menus expose the required exact desktop actions', async () => {
  const { api } = makeSandbox();
  const labels = (status) => api.actions(state(status, [clearCandidate])).map((item) => item.label);
  assert.deepEqual(labels({ state: 'untrusted', adapterId: 'typescript', projectRootUri: 'file:///repo', capabilities }), [
    'Trust and enable', 'Not now', 'Edit without language features', 'Change project…',
  ]);
  assert.deepEqual(labels({ state: 'ready', adapterId: 'typescript', projectRootUri: 'file:///repo', capabilities }), [
    'Change project…', 'Restart language server', 'View server logs', 'Revoke trust',
  ]);
  assert.deepEqual(labels({ state: 'failed', adapterId: 'typescript', projectRootUri: 'file:///repo', capabilities }), [
    'Retry', 'View server logs', 'Change project…', 'Revoke trust',
  ]);
  assert.deepEqual(labels({ state: 'unavailable', adapterId: 'java', capabilities }), [
    'Edit without language features', 'Change project…',
  ]);
});

await check('trust disclosure names the executable family and canonical root', async () => {
  const { api } = makeSandbox();
  assert.deepEqual(JSON.parse(JSON.stringify(api.trustDisclosure(state({
    state: 'untrusted', adapterId: 'typescript', projectRootUri: 'file:///repo', capabilities,
  }, [clearCandidate])))), {
    executableFamily: 'typescript-language-server',
    canonicalRoot: '/repo',
  });
  const families = {
    json: 'vscode-json-languageserver', python: 'pyright', rust: 'rust-analyzer',
    go: 'gopls', clangd: 'clangd', java: 'Eclipse JDT Language Server',
  };
  for (const [adapterId, executableFamily] of Object.entries(families)) {
    assert.equal(api.trustDisclosure(state({
      state: 'untrusted', adapterId, projectRootUri: 'file:///repo', capabilities,
    }, [clearCandidate])).executableFamily, executableFamily);
  }
});

await check('mounting/opening a file stays non-modal and the control is keyboard reachable', async () => {
  const { api, store, dialogCalls } = makeSandbox();
  const pane = { paneId: 1, root: new Element('div') };
  attachPane(store, pane, state(
    { state: 'choosingProject', adapterId: 'typescript', capabilities },
    [clearCandidate, nestedCandidate],
  ));
  const control = api.mount(pane.root, pane);
  assert.equal(dialogCalls.length, 0);
  assert.equal(control.tagName, 'BUTTON');
  assert.equal(control.type, 'button');
  assert.equal(control.textContent, 'Choose project…');
  assert.equal(control['aria-label'], 'Project context: Choose project…');
});

await check('trusted-project management renders in Editor settings and revokes through the bridge', async () => {
  const { api, bridgeCalls } = makeSandbox();
  const container = new Element('div');
  const rows = [];
  await api.renderTrustedProjects(container, {
    addSectionLabel(parent, text) { rows.push(['section', text]); return parent.appendChild(new Element('div')); },
    addRow(parent, label, description, control) {
      rows.push(['row', label, description, control]);
      return parent.appendChild(new Element('div'));
    },
  });
  assert.equal(rows[0][1], 'Trusted Projects');
  assert.match(rows[1][2], /typescript · trusted · Last used/);
  rows[1][3].click();
  await new Promise((resolve) => setTimeout(resolve, 0));
  assert.deepEqual(bridgeCalls[0], ['lsp_trusted_projects', undefined]);
  assert.deepEqual(bridgeCalls[1], ['lsp_revoke_project_trust', { root: '/repo', adapterId: 'typescript' }]);
});

await check('failed trust keeps the disclosure open and renders a normalized actionable error', async () => {
  const { api, store, menuCalls, dialogCalls, closedDialogs, toasts } = makeSandbox({
    rejectCommand: 'lsp_set_project_trust',
  });
  const pane = { paneId: 2, root: new Element('div') };
  attachPane(store, pane, state({
    state: 'untrusted', adapterId: 'typescript', projectRootUri: 'file:///repo', capabilities,
  }, [clearCandidate]));
  const control = api.mount(pane.root, pane);
  control.click();
  menuCalls.at(-1).items.find((item) => item.label === 'Trust and enable').onSelect();
  const trustDialog = dialogCalls.at(-1);
  await trustDialog.buttons.find((button) => button.label === 'Trust and enable').onSelect();
  assert.deepEqual(closedDialogs, [], 'failed trust stays open for retry');
  assert.equal(toasts.error.length, 1);
  assert.match(toasts.error[0][1], /editing continues/i);
});

await check('failed revoke restores the settings action instead of claiming success', async () => {
  const { api, toasts } = makeSandbox({ rejectCommand: 'lsp_revoke_project_trust' });
  const rows = [];
  await api.renderTrustedProjects(new Element('div'), {
    addSectionLabel() {},
    addRow(_parent, label, description, control) { rows.push({ label, description, control }); },
  });
  const button = rows[0].control;
  button.click();
  await new Promise((resolve) => setTimeout(resolve, 0));
  assert.equal(button.textContent, 'Revoke trust');
  assert.equal(button.disabled, false);
  assert.equal(toasts.error.length, 1);
});

await check('failed log retrieval shows an error and never fabricates an empty-log success', async () => {
  const { api, store, menuCalls, toasts } = makeSandbox({ rejectCommand: 'lsp_session_logs' });
  const pane = { paneId: 3, root: new Element('div') };
  attachPane(store, pane, state({
    state: 'ready', adapterId: 'typescript', projectRootUri: 'file:///repo', capabilities,
  }, [clearCandidate]));
  const control = api.mount(pane.root, pane);
  control.click();
  menuCalls.at(-1).items.find((item) => item.label === 'View server logs').onSelect();
  await new Promise((resolve) => setTimeout(resolve, 0));
  assert.equal(toasts.error.length, 1);
  assert.deepEqual(toasts.info, []);
});

await check('real restart action sends the candidate filesystem root, never a file URI', async () => {
  const { api, store, menuCalls, bridgeCalls } = makeSandbox();
  const pane = { paneId: 4, root: new Element('div') };
  attachPane(store, pane, state({
    state: 'ready', adapterId: 'typescript', projectRootUri: 'file:///repo', capabilities,
  }, [clearCandidate]));
  const control = api.mount(pane.root, pane);
  control.click();
  menuCalls.at(-1).items.find((item) => item.label === 'Restart language server').onSelect();
  await new Promise((resolve) => setTimeout(resolve, 0));
  assert.deepEqual(bridgeCalls.at(-1), [
    'lsp_restart_session', { adapterId: 'typescript', root: '/repo' },
  ]);
});

console.log(`\n${ran - failures}/${ran} project context checks passed`);
if (failures) process.exit(1);

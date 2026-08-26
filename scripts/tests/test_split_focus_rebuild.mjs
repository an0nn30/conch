// Run: node scripts/tests/test_split_focus_rebuild.mjs
//
// rebuildTreeDOM detaches every pane root and reattaches them under a fresh
// split wrapper. WebKit (the Tauri webview) never fires `blur` on an element
// that is detached while focused, so xterm's internal focus flag stays stuck
// true and the old pane's cursor keeps rendering — and blinking — as focused
// forever. The rebuild must therefore blur the active element itself while it
// is still attached (so the event actually fires) and restore focus after the
// reattach so a rebuild is focus-neutral. Verified against real WebKit via a
// Playwright harness; this suite pins the DOM choreography with stubs.
import assert from 'node:assert';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';

const MODULE_PATH = path.resolve(
  import.meta.dirname,
  '../../crates/termlab_tauri/frontend/app/layout-runtime.js',
);

function loadRuntime(deps) {
  const sandbox = { console, setTimeout, clearTimeout };
  sandbox.window = sandbox;
  vm.createContext(sandbox);
  vm.runInContext(fs.readFileSync(MODULE_PATH, 'utf8'), sandbox, { filename: MODULE_PATH });
  return sandbox.termlabLayoutRuntime.create(deps);
}

// --- minimal DOM stand-ins ---------------------------------------------------
function makeNode(doc, name) {
  return { name, ownerDocument: doc, parentNode: null, childNodes: [] };
}
function append(parent, child) {
  detachNode(child);
  child.parentNode = parent;
  parent.childNodes.push(child);
}
function detachNode(child) {
  if (!child.parentNode) return;
  const siblings = child.parentNode.childNodes;
  siblings.splice(siblings.indexOf(child), 1);
  child.parentNode = null;
}
function isUnder(node, ancestor) {
  for (let cur = node; cur; cur = cur.parentNode) {
    if (cur === ancestor) return true;
  }
  return false;
}

function setup() {
  const doc = { activeElement: null };
  const docRoot = makeNode(doc, 'docRoot');
  const log = [];

  const containerEl = makeNode(doc, 'container');
  Object.defineProperty(containerEl, 'firstChild', {
    get() { return this.childNodes[0] || null; },
  });
  containerEl.removeChild = (child) => detachNode(child);
  containerEl.appendChild = (child) => append(containerEl, child);
  containerEl.contains = (node) => isUnder(node, containerEl);
  append(docRoot, containerEl);

  function makePaneRoot(name) {
    const root = makeNode(doc, `pane-${name}`);
    const textarea = makeNode(doc, `textarea-${name}`);
    append(root, textarea);
    Object.defineProperty(textarea, 'isConnected', {
      get() { return isUnder(textarea, docRoot); },
    });
    textarea.blur = () => {
      log.push(`blur:${name}:connected=${textarea.isConnected}`);
      if (doc.activeElement === textarea) doc.activeElement = null;
    };
    textarea.focus = () => {
      log.push(`focus:${name}:connected=${textarea.isConnected}`);
      doc.activeElement = textarea;
    };
    root.textarea = textarea;
    return root;
  }

  const paneA = makePaneRoot('a');
  const paneB = makePaneRoot('b');
  const panes = new Map([
    [1, { root: paneA }],
    [2, { root: paneB }],
  ]);
  const tab = {
    containerEl,
    treeRoot: { type: 'split', children: [{ type: 'leaf', paneId: 1 }, { type: 'leaf', paneId: 2 }] },
  };

  // renderTree stub: wrap both pane roots in a fresh split wrapper.
  const renderTree = (_tree, getPaneEl) => {
    const wrapper = makeNode(doc, 'wrapper');
    append(wrapper, getPaneEl(1));
    append(wrapper, getPaneEl(2));
    return wrapper;
  };

  const runtime = loadRuntime({
    invoke: async () => {},
    getPanes: () => panes,
    allPanesInTab: () => [1, 2],
    getCurrentTab: () => tab,
    renderTree,
  });

  // Start attached: both panes live under an old wrapper in the container.
  const oldWrapper = makeNode(doc, 'old-wrapper');
  append(oldWrapper, paneA);
  append(oldWrapper, paneB);
  append(containerEl, oldWrapper);

  return { doc, containerEl, log, paneA, paneB, tab, runtime };
}

// --- focused pane inside the container: blur fires while attached, focus restored ---
{
  const { doc, log, paneA, tab, runtime } = setup();
  doc.activeElement = paneA.textarea;
  runtime.rebuildTreeDOM(tab);
  assert.deepStrictEqual(
    log,
    ['blur:a:connected=true', 'focus:a:connected=true'],
    'active textarea is blurred while still attached, then refocused once reattached',
  );
  assert.strictEqual(doc.activeElement, paneA.textarea, 'rebuild is focus-neutral');
}

// --- focus outside the container (e.g. a dialog input): rebuild must not touch it ---
{
  const { doc, log, tab, runtime } = setup();
  const outside = {
    parentNode: null,
    blur: () => log.push('blur:outside'),
    focus: () => log.push('focus:outside'),
  };
  doc.activeElement = outside;
  runtime.rebuildTreeDOM(tab);
  assert.deepStrictEqual(log, [], 'focus outside the container is left alone');
  assert.strictEqual(doc.activeElement, outside);
}

// --- nothing focused at all: plain rebuild, no focus traffic ---
{
  const { doc, log, containerEl, paneA, paneB, tab, runtime } = setup();
  doc.activeElement = null;
  runtime.rebuildTreeDOM(tab);
  assert.deepStrictEqual(log, [], 'no active element: no blur/focus traffic');
  assert.strictEqual(containerEl.childNodes.length, 1, 'old children replaced with the rendered tree');
  assert.ok(containerEl.contains(paneA) && containerEl.contains(paneB), 'panes reattached under the new wrapper');
}

console.log('test_split_focus_rebuild: all assertions passed');

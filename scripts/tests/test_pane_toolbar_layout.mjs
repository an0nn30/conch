// Run: node scripts/tests/test_pane_toolbar_layout.mjs
//
// The pane toolbar is split into two groups so narrow panes (sidebar dock,
// skinny splits) can stack them via container query: a connection group (host
// picker + disconnect, remote panes only) and a navigation group (history,
// path, view controls). The connection group comes FIRST in the DOM — it is
// the pane's context and leads both the stacked layout and the tab order —
// while wide single-row layouts place it visually to the right with CSS
// `order`. These tests pin the markup contract that CSS relies on.

import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';

const PANE_VIEW_PATH = path.resolve(
  import.meta.dirname,
  '../../crates/termlab_tauri/frontend/app/features/files/pane-view.js',
);

function loadPaneView() {
  const sandbox = { console };
  sandbox.window = sandbox;
  vm.createContext(sandbox);
  vm.runInContext(fs.readFileSync(PANE_VIEW_PATH, 'utf8'), sandbox, { filename: PANE_VIEW_PATH });
  assert.ok(sandbox.termlabFilesPaneView, 'pane-view IIFE must expose window.termlabFilesPaneView');
  return sandbox.termlabFilesPaneView;
}

// renderPane guards every post-render query, so a recording element captures
// the markup without needing a DOM.
function recordingElement() {
  return {
    innerHTML: '',
    querySelector: () => null,
    querySelectorAll: () => [],
  };
}

function pane(isLocal) {
  return {
    isLocal,
    pathInput: isLocal ? '/home/user' : '/srv',
    backStack: [],
    forwardStack: [],
    entries: [],
    showHidden: false,
    colExt: false,
    colSize: true,
    colModified: false,
    error: null,
    transferStatus: null,
  };
}

// The local pane has no connection identity: navigation group only.
{
  const view = loadPaneView();
  const el = recordingElement();
  view.renderPane(pane(true), el, {});
  assert.ok(el.innerHTML.includes('fp-tb-nav'), 'local toolbar must render the navigation group');
  assert.ok(!el.innerHTML.includes('fp-tb-conn'), 'local toolbar must not render a connection group');
  assert.ok(!el.innerHTML.includes('fp-host-combo-slot'), 'local toolbar must not render a host slot');
}

// The remote pane renders the connection group before the navigation group,
// with the host slot inside it.
{
  const view = loadPaneView();
  const el = recordingElement();
  view.renderPane(pane(false), el, { activeRemotePaneId: 3 });
  const html = el.innerHTML;
  const connAt = html.indexOf('fp-tb-conn');
  const navAt = html.indexOf('fp-tb-nav');
  assert.ok(connAt >= 0, 'remote toolbar must render the connection group');
  assert.ok(navAt >= 0, 'remote toolbar must render the navigation group');
  assert.ok(connAt < navAt, 'connection group must precede navigation in the DOM');

  const slotAt = html.indexOf('fp-host-combo-slot');
  assert.ok(slotAt > connAt && slotAt < navAt, 'host slot must live inside the connection group');

  const toolbarAt = html.indexOf('fp-toolbar');
  assert.ok(toolbarAt >= 0 && toolbarAt < connAt, 'both groups must live inside the toolbar');

  for (const control of ['data-action="back"', 'data-action="forward"', 'fp-path-input',
    'data-action="home"', 'data-action="refresh"', 'data-action="hidden"']) {
    assert.ok(html.indexOf(control) > navAt, `${control} must live inside the navigation group`);
  }
}

console.log('pane toolbar layout: all assertions passed');

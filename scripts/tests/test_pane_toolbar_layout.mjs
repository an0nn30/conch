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

// The local pane has no connection identity: no host strip, and a slim nav
// row of back/forward, path, refresh, and the overflow menu button.
{
  const view = loadPaneView();
  const el = recordingElement();
  view.renderPane(pane(true), el, {});
  assert.ok(!el.innerHTML.includes('fp-host-strip'), 'local pane must not render a host strip');
  assert.ok(el.innerHTML.includes('fp-tb-nav'), 'local toolbar must render the navigation group');
  assert.ok(el.innerHTML.includes('data-action="more"'), 'local toolbar must render the overflow button');
  assert.ok(!el.innerHTML.includes('data-action="home"'), 'home moves into the overflow menu');
  assert.ok(!el.innerHTML.includes('data-action="hidden"'), 'hidden toggle moves into the overflow menu');
}

// The remote pane renders a host header strip ABOVE the toolbar — status dot,
// the host slot (tl-combo mounts there), and disconnect — then the same slim
// navigation row as the local pane.
{
  const view = loadPaneView();
  const el = recordingElement();
  view.renderPane(pane(false), el, { activeRemotePaneId: 3 });
  const html = el.innerHTML;
  const stripAt = html.indexOf('fp-host-strip');
  const toolbarAt = html.indexOf('fp-toolbar');
  assert.ok(stripAt >= 0, 'remote pane must render the host strip');
  assert.ok(toolbarAt >= 0, 'remote pane must render the toolbar');
  assert.ok(stripAt < toolbarAt, 'host strip must sit above the toolbar');

  const slotAt = html.indexOf('fp-host-combo-slot');
  assert.ok(slotAt > stripAt && slotAt < toolbarAt, 'host slot must live inside the strip');
  assert.ok(html.includes('fp-host-status'), 'strip must render a connection status dot');
  assert.ok(html.includes('is-connected'), 'an active session must mark the dot connected');

  for (const control of ['data-action="back"', 'data-action="forward"', 'fp-path-input',
    'data-action="refresh"', 'data-action="more"']) {
    assert.ok(html.indexOf(control) > toolbarAt, `${control} must live inside the toolbar`);
  }
  assert.ok(!html.includes('data-action="home"'), 'home moves into the overflow menu');
  assert.ok(!html.includes('data-action="hidden"'), 'hidden toggle moves into the overflow menu');
}

// Disconnected remote pane: the dot is not marked connected.
{
  const view = loadPaneView();
  const el = recordingElement();
  view.renderPane(pane(false), el, {});
  assert.ok(el.innerHTML.includes('fp-host-status'), 'strip renders its dot while disconnected');
  assert.ok(!el.innerHTML.includes('is-connected'), 'no active session means no connected marker');
}

console.log('pane toolbar layout: all assertions passed');

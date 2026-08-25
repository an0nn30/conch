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
const BREADCRUMBS_PATH = path.resolve(
  import.meta.dirname,
  '../../crates/termlab_tauri/frontend/app/features/files/breadcrumbs.js',
);

function loadPaneView() {
  const sandbox = { console };
  sandbox.window = sandbox;
  vm.createContext(sandbox);
  vm.runInContext(fs.readFileSync(BREADCRUMBS_PATH, 'utf8'), sandbox, { filename: BREADCRUMBS_PATH });
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
    pathInput: isLocal ? '/home/user' : '/srv/deep',
    currentPath: isLocal ? '/home/user' : '/srv/deep',
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

// Breadcrumbs replace the visible path box: each ancestor is a clickable
// crumb, the current directory is inert, and the text input stays in the DOM
// (hidden) for click-to-edit. A connected remote pane gets the same bar.
{
  const view = loadPaneView();
  const el = recordingElement();
  view.renderPane(pane(true), el, {});
  const html = el.innerHTML;
  assert.ok(html.includes('fp-crumbs'), 'toolbar must render the crumb bar');
  assert.ok(html.includes('data-crumb-path="/"'), 'root crumb navigates to /');
  assert.ok(html.includes('data-crumb-path="/home"'), 'ancestor crumb navigates to its path');
  assert.ok(!html.includes('data-crumb-path="/home/user"'), 'current directory is not a navigation target');
  assert.ok(html.includes('is-current'), 'current directory renders as the inert current crumb');
  assert.ok(html.includes('fp-path-input'), 'text input stays in the DOM for click-to-edit');
}

// Deep paths fold their middle into a crumb-overflow control.
{
  const view = loadPaneView();
  const el = recordingElement();
  const deep = pane(true);
  deep.currentPath = '/a/b/c/d/e/f';
  deep.pathInput = '/a/b/c/d/e/f';
  view.renderPane(deep, el, {});
  assert.ok(el.innerHTML.includes('data-action="crumb-overflow"'), 'deep paths render the overflow crumb');
  assert.ok(!el.innerHTML.includes('data-crumb-path="/a/b"'), 'folded ancestors are not inline crumbs');
}

// A disconnected remote pane has no path to crumb: it keeps the plain
// (disabled) input rather than an empty bar.
{
  const view = loadPaneView();
  const el = recordingElement();
  const disconnected = pane(false);
  disconnected.currentPath = '';
  disconnected.pathInput = '';
  view.renderPane(disconnected, el, {});
  assert.ok(!el.innerHTML.includes('fp-crumbs'), 'no crumb bar without a session');
  assert.ok(el.innerHTML.includes('fp-path-input'), 'plain input remains while disconnected');
}

console.log('pane toolbar layout: all assertions passed');

# Design System Phase 2: Right Side — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the right side of the window match the JVM TermLab reference: IntelliJ-style tool-window chrome, Hosts (top) and Tunnels (bottom) stacked tool windows with icon toolbars, restyled edge strips, and Notifications as a strip-accessible tool window.

**Architecture:** Restyle the existing tool-window-manager chrome and strips onto Phase 1's tokens/components; slim the Sessions panel into a Hosts window; extract the tunnels section into a new Tunnels tool window; migrate the notification history from the fixed bottom bar into a right-zone tool window. No Rust changes.

**Tech Stack:** Vanilla IIFE JS + CSS custom properties (Phase 1 tokens); IntelliJ icons via `window.tlIcon`.

## Global Constraints

- Spec: `docs/superpowers/specs/2026-08-14-termlab-design-system-design.md` (Phase 2 scope). Reference look: the JVM TermLab screenshot — tool window = title header row (title left; gear + minimize icons right) + small-icon toolbar row + content; empty states say `Nothing to show`; right strip shows rotated labels.
- Phase 1 products to consume (do not redefine): semantic vars `--tl-bg, --tl-panel-bg, --tl-fg, --tl-fg-muted, --tl-border, --tl-accent, --tl-selection-bg, --tl-selection-fg, --tl-row-hover, --tl-header-h(28px), --tl-toolbar-h(26px), --tl-row-h(24px), --tl-space-1..4, --tl-radius, --tl-font-ui, --tl-font-size-ui, --tl-scrollbar-thumb(-hover)`; classes `tl-btn, tl-btn--primary, tl-icon-btn, tl-input, tl-toolwindow__header, tl-toolwindow__toolbar, tl-empty-state, tl-scroll`; icon helper `window.tlIcon.create(name, {size, alt})` with vendored names `add, edit, remove, refresh, web, settings, hideToolWindow, close, search, chevronDown, chevronRight, folder, file, notifications, moreVertical`.
- NO raw hex in any CSS this phase touches — use `var(--tl-*)` (legacy vars like `var(--blue)` being replaced count as violations too in edited rules). `styles/tool-windows.css` and `styles/panels.css` are being migrated in place; edited rules must be token-based.
- All command/event calls via the injected `invoke` / `window.termlabTauriClient`; keyboard via `window.termlabKeyboardRouter`; layout persistence via the existing layout-service flow (`tool_window_zones` etc. — do not rename persisted keys; keep tool-window ids stable: `ssh-sessions` keeps its id, only its title changes).
- Vanilla IIFE modules; script tags in `index.html` in dependency order.
- `rg` is NOT installed — verify with plain `grep`. `node --check` every touched JS file.
- Frequent commits: commit message given per task.
- Key file/line anchors (verified): `app/layout/tool-window-manager.js` — `register` L127, `ensureWindowElement` L213, `moveTo` L289, `updateZone` L341, `updateStrips` L701, `makeStripBtn` L733; `app/tool-window-runtime.js` — ssh-sessions registration L153, tunnelManager init L248; `app/panels/ssh-panel.js` — panel innerHTML L84–104; `app/features/ssh/view.js` — `makeSectionHeader` L4, `createServerNode` L11, `createFolderNode` L36, `renderServerList` L73, `renderSessions` L122, `createTunnelNode` L143, `renderTunnels` L215; `app/ui/notification-panel.js` — `init` L12, `addTab` L36, `renderNotifications` L71; `app/startup-runtime.js` L123 (notificationPanel.init), L140–145 (bottom panel restore); `app/menu-actions.js` L231 (toggle-bottom-panel).

---

### Task 1: Tool-window chrome and strip restyle

**Files:**
- Modify: `crates/termlab_tauri/frontend/app/layout/tool-window-manager.js` (updateZone L341, makeStripBtn L733)
- Create: `crates/termlab_tauri/frontend/styles/design-system/components/strips.css`
- Modify: `crates/termlab_tauri/frontend/styles/tool-windows.css` (zone-header/strip rules)
- Modify: `crates/termlab_tauri/frontend/index.html` + `settings.html` (one stylesheet link)

**Interfaces:**
- Consumes: `tl-toolwindow__header` etc. from Phase 1; `window.tlIcon`
- Produces: zone headers rendered as `div.zone-header.tl-toolwindow__header` containing `span.zone-header-title` + `span.tl-toolwindow__header-actions` with two `button.tl-icon-btn` (gear → existing context menu; hide → `deactivate`); strip buttons `button.strip-btn` optionally containing a `.tl-icon` img before the label. Later tasks rely on `register(id, {icon})` now rendering that icon on the strip button.

- [ ] **Step 1: Rework `updateZone` header creation**

In `updateZone` (L341), where `div.zone-header > span.zone-header-title` is built, replace the header construction with:

```js
      const header = document.createElement('div');
      header.className = 'zone-header tl-toolwindow__header';
      const titleEl = document.createElement('span');
      titleEl.className = 'zone-header-title';
      titleEl.textContent = activeWindow.title;
      const actionsEl = document.createElement('span');
      actionsEl.className = 'tl-toolwindow__header-actions';
      const gearBtn = document.createElement('button');
      gearBtn.className = 'tl-icon-btn';
      gearBtn.title = 'Options';
      gearBtn.appendChild(global.tlIcon.create('settings', { size: 16, alt: 'Options' }));
      gearBtn.addEventListener('click', (e) => {
        e.stopPropagation();
        showContextMenu(activeWindow.id, zoneId, e.clientX, e.clientY);
      });
      const hideBtn = document.createElement('button');
      hideBtn.className = 'tl-icon-btn';
      hideBtn.title = 'Hide';
      hideBtn.appendChild(global.tlIcon.create('hideToolWindow', { size: 16, alt: 'Hide' }));
      hideBtn.addEventListener('click', (e) => {
        e.stopPropagation();
        deactivate(activeWindow.id);
      });
      actionsEl.appendChild(gearBtn);
      actionsEl.appendChild(hideBtn);
      header.appendChild(titleEl);
      header.appendChild(actionsEl);
```

Match the exact local variable names in the current function (read it first — if the active window variable is named differently, e.g. `tw`, use that; `showContextMenu(id, zone, x, y)`'s real signature is at its definition — align the call). Keep the header appended where the old one was.

- [ ] **Step 2: Render icons on strip buttons**

In `makeStripBtn` (L733), before setting the label, add:

```js
    if (tw.icon && global.tlIcon) {
      btn.appendChild(global.tlIcon.create(tw.icon, { size: 16, alt: '' }));
    }
    const labelSpan = document.createElement('span');
    labelSpan.className = 'strip-btn-label';
    labelSpan.textContent = tw.title;
    btn.appendChild(labelSpan);
```

replacing the bare `btn.textContent = tw.title;` (keep every existing listener/attr).

- [ ] **Step 3: Write strips.css and retoken zone chrome**

`styles/design-system/components/strips.css`:

```css
.side-strip {
  background: var(--tl-panel-bg);
  width: 26px;
}
.strip-btn {
  writing-mode: vertical-rl;
  text-orientation: mixed;
  display: flex;
  align-items: center;
  gap: var(--tl-space-1);
  padding: var(--tl-space-2) 0;
  font: 500 11px var(--tl-font-ui);
  color: var(--tl-fg-muted);
  background: transparent;
  border: none;
  border-left: 2px solid transparent;
}
.strip-btn:hover { color: var(--tl-fg); background: var(--tl-row-hover); }
.strip-btn.active {
  color: var(--tl-fg);
  background: var(--tl-selection-bg);
  border-left-color: var(--tl-accent);
}
#left-strip .strip-btn { transform: rotate(180deg); }
.strip-btn .tl-icon { transform: rotate(90deg); }
#left-strip .strip-btn .tl-icon { transform: rotate(-90deg); }
```

In `styles/tool-windows.css`: DELETE the now-superseded `.strip-btn` rules (L130–141 block) and the `.side-strip` width/background declarations that strips.css owns (keep `.side-strip.hidden`, zen rules, borders using `var(--tl-border)` — replace their current color values with tokens). Retoken `.zone-header`/`.zone-header-title` (L228–238): the `tl-toolwindow__header` class now provides chrome; reduce the legacy rules to only what it doesn't cover and switch remaining colors to `var(--tl-*)`. Remove `text-transform: uppercase` (JVM headers are normal case).

- [ ] **Step 4: Link strips.css**

Add `<link rel="stylesheet" href="styles/design-system/components/strips.css" />` after the toolwindow.css link in BOTH `index.html` and `settings.html`.

- [ ] **Step 5: Verify**

```bash
node --check crates/termlab_tauri/frontend/app/layout/tool-window-manager.js
grep -n "#[0-9a-fA-F]\{3,8\}" crates/termlab_tauri/frontend/styles/design-system/components/strips.css   # expect empty
grep -n "var(--blue)\|uppercase" crates/termlab_tauri/frontend/styles/tool-windows.css                     # expect no hits in zone/strip rules
cargo build -p termlab_tauri && (./target/debug/termlab > /tmp/tl-p2-t1.log 2>&1 & sleep 6; kill %1) ; grep -ci error /tmp/tl-p2-t1.log  # expect 0
```

- [ ] **Step 6: Commit**

```bash
git add -A crates/termlab_tauri/frontend
git commit -m "feat(design-system): IntelliJ tool-window headers and strip styling"
```

---

### Task 2: Sessions panel becomes Hosts

**Files:**
- Modify: `crates/termlab_tauri/frontend/app/tool-window-runtime.js` (registration L153)
- Modify: `crates/termlab_tauri/frontend/app/panels/ssh-panel.js` (panel shell L84–104 + tunnel delegation removal)
- Modify: `crates/termlab_tauri/frontend/styles/panels.css` (header/quick-connect rules)

**Interfaces:**
- Consumes: Task 1 chrome; `tl-toolwindow__toolbar`, `tl-icon-btn`, `tl-input`, `tlIcon`
- Produces: tool window id `ssh-sessions` (UNCHANGED id — persisted zones keep working) titled `Hosts`, icon `web`, containing: toolbar row (add/edit/remove/refresh/web buttons + quick-connect input), active-sessions section, server tree. NO tunnels section (Task 3 owns tunnels; this task removes the section and its listeners from ssh-panel). Exposes `window.sshPanel.getSelectedServer()` returning the currently selected server node's data or null (Task 3+toolbars need selection; implement simple selection: click on `.ssh-server-node` marks it `.selected`, stores in module state).

- [ ] **Step 1: Update registration**

At tool-window-runtime.js L153: `register('ssh-sessions', { title: 'Hosts', icon: 'web', type: 'built-in', defaultZone: 'right-top', renderFn })` (title/icon changed only).

- [ ] **Step 2: Rework the panel shell**

In ssh-panel.js L84–104 innerHTML: remove `.ssh-panel-header` (the zone header now carries the title) and `.ssh-tunnels-section`; replace with:

```js
    panelEl.innerHTML = `
      <div class="tl-toolwindow__toolbar" id="hosts-toolbar">
        <button class="tl-icon-btn" id="ssh-add-new" title="New Host"></button>
        <button class="tl-icon-btn" id="ssh-edit-host" title="Edit Host"></button>
        <button class="tl-icon-btn" id="ssh-remove-host" title="Delete Host"></button>
        <button class="tl-icon-btn" id="ssh-refresh" title="Refresh"></button>
        <button class="tl-icon-btn" id="ssh-config-toggle" title="Show ~/.ssh/config hosts"></button>
        <input id="ssh-quick-connect-input" class="tl-input ssh-quick-connect-input"
               placeholder="Quick connect (user@host:port)" />
      </div>
      <div class="ssh-panel-body tl-scroll" id="ssh-panel-body">
        <div class="ssh-active-sessions" id="ssh-active-sessions"></div>
        <div class="ssh-server-list" id="ssh-server-list"></div>
      </div>`;
```

After setting innerHTML, populate the five buttons' icons: `document.getElementById('ssh-add-new').appendChild(tlIcon.create('add', {size:16}))` and likewise `edit`, `remove`, `refresh`, `web`. Rewire: `#ssh-add-new` keeps its existing click handler; `#ssh-refresh` keeps its existing handler; `#ssh-edit-host` calls the existing edit flow with `getSelectedServer()` (find the current edit entry point — the server context menu's Edit item — and reuse its function; disable the button via `disabled` when no selection); `#ssh-remove-host` same pattern with the delete flow (confirmation dialog included — reuse the context menu's delete path); `#ssh-config-toggle` toggles visibility of the ssh-config section (add a module-level boolean; re-render server list on toggle; button gets `.active` class via `classList.toggle`).

- [ ] **Step 3: Implement selection state**

In ssh-panel.js module scope add `let selectedServer = null;`. In the server-node click wiring (where `createServerNode` results get their handlers — the delegation lives in ssh-panel.js's render path), add: on click, clear previous `.selected` from the tree root, add `.selected` to the clicked node, set `selectedServer` to the node's server data, and update the edit/remove buttons' `disabled`. Export `getSelectedServer: () => selectedServer` on the returned `window.sshPanel` object. Double-click keeps its existing connect behavior.

- [ ] **Step 4: Remove tunnels from this panel**

Delete ssh-panel.js's tunnel delegation functions (`renderTunnels` L709 wrapper, `refreshTunnels` L695, `showTunnelContextMenu` L734) and any listeners/refresh calls that referenced `#ssh-tunnels-section` (grep the file for `tunnel` — every hit either moves to Task 3's panel or is deleted; record in your report which were deleted). Do NOT touch `app/features/ssh/view.js` tunnel functions (Task 3 consumes them) or tunnel data-service/actions.

- [ ] **Step 5: Retoken affected panels.css rules**

`.ssh-quick-connect` block: reduce to layout only (the input is now `tl-input`); `.ssh-panel-header`/`.ssh-panel-title`/`.ssh-panel-actions`/`.ssh-icon-btn` rules: delete if now unused (grep JS for each class first; `.ssh-icon-btn` is still used by other panels — check `grep -rn "ssh-icon-btn" crates/termlab_tauri/frontend/app` and keep if referenced, but switch its colors to tokens).

- [ ] **Step 6: Verify + commit**

```bash
node --check crates/termlab_tauri/frontend/app/panels/ssh-panel.js
grep -n "ssh-tunnels-section" crates/termlab_tauri/frontend/app/panels/ssh-panel.js  # expect empty
cargo build -p termlab_tauri && (./target/debug/termlab > /tmp/tl-p2-t2.log 2>&1 & sleep 6; kill %1) ; grep -ci error /tmp/tl-p2-t2.log
git add -A crates/termlab_tauri/frontend
git commit -m "feat(design-system): Sessions panel becomes Hosts with icon toolbar and selection"
```

---

### Task 3: Tunnels tool window

**Files:**
- Create: `crates/termlab_tauri/frontend/app/panels/tunnels-panel.js`
- Modify: `crates/termlab_tauri/frontend/app/tool-window-runtime.js` (register after ssh-sessions)
- Modify: `crates/termlab_tauri/frontend/index.html` (script tag)

**Interfaces:**
- Consumes: `sshView.renderTunnels`/`createTunnelNode` (app/features/ssh/view.js L143/L215 — reuse, don't duplicate), `sshData.getTunnels` (features/ssh/data-service.js L11), `sshActions.startTunnel/stopTunnel/deleteTunnel` (features/ssh/actions.js L20-28), `window.tunnelManager.show/showNewTunnelForm/showEdit` (app/panels/tunnel-manager.js), Task 1 chrome classes, `tl-empty-state`
- Produces: `window.tunnelsPanel = { init, refresh }`; tool window id `tunnels`, title `Tunnels`, icon `moreVertical` is WRONG — use no icon (pass `icon: null`; the JVM strip shows a plug-like icon we don't have vendored — the label suffices; note it as a Phase 2 known gap), `defaultZone: 'right-bottom'`.

- [ ] **Step 1: Write tunnels-panel.js**

```js
(function (global) {
  'use strict';

  function create() {
    let panelEl = null;
    let invoke = null;
    let listen = null;
    let refreshTimer = null;

    async function refresh() {
      if (!panelEl || !invoke) return;
      const listEl = panelEl.querySelector('#tunnels-list');
      if (!listEl) return;
      let tunnels = [];
      try {
        tunnels = await global.sshData.getTunnels(invoke);
      } catch (e) {
        console.error('tunnels refresh failed:', e);
        return;
      }
      listEl.innerHTML = '';
      if (!tunnels.length) {
        const empty = document.createElement('div');
        empty.className = 'tl-empty-state';
        empty.textContent = 'Nothing to show';
        listEl.appendChild(empty);
        return;
      }
      for (const tunnel of tunnels) {
        listEl.appendChild(global.sshView.createTunnelNode(tunnel, {
          onStart: (t) => global.sshActions.startTunnel(invoke, t).then(refresh),
          onStop: (t) => global.sshActions.stopTunnel(invoke, t).then(refresh),
          onMenu: (t, x, y) => global.tunnelManager.show(),
        }));
      }
    }

    function init(deps) {
      invoke = deps.invoke;
      listen = deps.listen;
      panelEl = deps.panelEl;
      panelEl.innerHTML = `
        <div class="tl-toolwindow__toolbar" id="tunnels-toolbar">
          <button class="tl-icon-btn" id="tunnel-add" title="New Tunnel"></button>
          <button class="tl-icon-btn" id="tunnel-manage" title="Edit Tunnels"></button>
          <button class="tl-icon-btn" id="tunnel-refresh" title="Refresh"></button>
        </div>
        <div class="tunnels-list tl-scroll" id="tunnels-list"></div>`;
      panelEl.querySelector('#tunnel-add').appendChild(global.tlIcon.create('add', { size: 16 }));
      panelEl.querySelector('#tunnel-manage').appendChild(global.tlIcon.create('edit', { size: 16 }));
      panelEl.querySelector('#tunnel-refresh').appendChild(global.tlIcon.create('refresh', { size: 16 }));
      panelEl.querySelector('#tunnel-add').addEventListener('click', () => {
        global.tunnelManager.showNewTunnelForm ? global.tunnelManager.showNewTunnelForm() : global.tunnelManager.show();
      });
      panelEl.querySelector('#tunnel-manage').addEventListener('click', () => global.tunnelManager.show());
      panelEl.querySelector('#tunnel-refresh').addEventListener('click', refresh);
      if (listen) {
        listen('tunnel-status-changed', () => refresh());
      }
      refresh();
      refreshTimer = setInterval(refresh, 15000);
    }

    return { init, refresh };
  }

  global.tunnelsPanel = create();
})(window);
```

IMPORTANT adaptation step: FIRST verify the actual global names exported by `app/features/ssh/{view,data-service,actions}.js` (grep each file for `global.` / `window.` assignments — the snippet assumes `sshView`/`sshData`/`sshActions` but the real names may differ, e.g. `termlabSshView`) and use the real ones throughout. Then: `createTunnelNode`'s REAL signature is at view.js L143 — read it and pass the callbacks/args it actually takes (the snippet above guesses an options object; align to reality, including how the sidebar previously wired start/stop/menu — mirror that wiring, minus the removed sidebar context). Same for `sshData.getTunnels`' signature (L11) and the actions (L20-28) — if they take `(invoke, tunnel)` vs `(tunnel)`, match. Check whether a `tunnel-status-changed` (or similarly named) event exists — grep `crates/termlab_tauri/src` for `emit.*tunnel`; if none exists, drop the `listen` block and rely on the interval + post-action refreshes. If `showNewTunnelForm` isn't exported from tunnel-manager (check its exports at L655: `{ init, show, showEdit, showError }`), fall back to `show()` for the add button and note it.

- [ ] **Step 2: Register + script tag**

In tool-window-runtime.js after the ssh-sessions registration block: register `'tunnels'` with `{ title: 'Tunnels', type: 'built-in', defaultZone: 'right-bottom', renderFn: (el) => { global.tunnelsPanel.init({ invoke, listen: listenOnCurrentWindow, panelEl: el }); } }` (match the surrounding registrations' dependency names exactly). Add `<script src="app/panels/tunnels-panel.js"></script>` in index.html next to the other `app/panels/*` scripts, BEFORE tool-window-runtime.js loads... verify tool-window-runtime.js is loaded after panels (it is — check and keep order).

- [ ] **Step 3: Verify + commit**

```bash
node --check crates/termlab_tauri/frontend/app/panels/tunnels-panel.js
cargo build -p termlab_tauri && (./target/debug/termlab > /tmp/tl-p2-t3.log 2>&1 & sleep 6; kill %1) ; grep -ci error /tmp/tl-p2-t3.log
git add -A crates/termlab_tauri/frontend
git commit -m "feat(design-system): Tunnels tool window in right-bottom zone"
```

---

### Task 4: Tree and section restyle (IntelliJ look for Hosts content)

**Files:**
- Modify: `crates/termlab_tauri/frontend/app/features/ssh/view.js` (chevron/folder icons)
- Modify: `crates/termlab_tauri/frontend/styles/panels.css` (tree/session/tunnel-node rules)

**Interfaces:**
- Consumes: `tlIcon` (chevronRight/chevronDown/folder/file), Phase 1 semantic vars
- Produces: same DOM class names (no JS API changes beyond icon elements inside existing nodes)

- [ ] **Step 1: Replace text arrows with icon chevrons**

In `createFolderNode` (view.js L36): the `.ssh-folder-arrow` element currently holds a text glyph — replace its text content with `tlIcon.create(expanded ? 'chevronDown' : 'chevronRight', { size: 16 })` and update the expand/collapse toggle to swap the img (replace child). Add `tlIcon.create('folder', { size: 16 })` before `.ssh-folder-name`. Guard: `if (global.tlIcon)` fallback to existing text glyph (settings.html contexts may not load tlIcon — verify; it does load it per Task 5 of Phase 1, so the guard is belt-and-braces).

- [ ] **Step 2: Retoken tree rules in panels.css**

Rewrite these rule blocks to tokens with IntelliJ metrics — keep selectors identical:

```css
.ssh-server-node, .ssh-session-node, .ssh-tunnel-node, .ssh-folder-header {
  height: var(--tl-row-h);
  display: flex; align-items: center;
  gap: var(--tl-space-1);
  padding: 0 var(--tl-space-2);
  font: 400 var(--tl-font-size-ui) var(--tl-font-ui);
  color: var(--tl-fg);
}
.ssh-server-node:hover, .ssh-folder-header:hover { background: var(--tl-row-hover); }
.ssh-server-node.selected { background: var(--tl-selection-bg); color: var(--tl-selection-fg); }
.ssh-server-detail, .ssh-folder-count { color: var(--tl-fg-muted); }
.ssh-section-header, .ssh-section-header-inline {
  color: var(--tl-fg-muted);
  font: 600 11px var(--tl-font-ui);
  padding: var(--tl-space-2) var(--tl-space-2) var(--tl-space-1);
}
```

Fold these into the existing blocks (replace color/size/padding declarations; keep any behavior-bearing declarations like cursor/overflow that exist today). Every edited rule: tokens only, no hex, no `var(--blue)`-era vars.

- [ ] **Step 3: Verify + commit**

```bash
node --check crates/termlab_tauri/frontend/app/features/ssh/view.js
grep -n "#[0-9a-fA-F]\{3,8\}" crates/termlab_tauri/frontend/styles/panels.css | head   # inspect: hits allowed ONLY in rules this task did not edit
cargo build -p termlab_tauri && (./target/debug/termlab > /tmp/tl-p2-t4.log 2>&1 & sleep 6; kill %1) ; grep -ci error /tmp/tl-p2-t4.log
git add -A crates/termlab_tauri/frontend
git commit -m "feat(design-system): IntelliJ tree styling and icons for Hosts content"
```

---

### Task 5: Notifications as a tool window

**Files:**
- Create: `crates/termlab_tauri/frontend/app/panels/notifications-panel.js`
- Modify: `crates/termlab_tauri/frontend/app/ui/notification-panel.js` (extract render, keep plugin-tab machinery)
- Modify: `crates/termlab_tauri/frontend/app/tool-window-runtime.js` (register), `app/menu-actions.js` L231 area (notifications menu item → activate tool window), `index.html` (script tag)

**Interfaces:**
- Consumes: notification store/render logic currently in `renderNotifications` (notification-panel.js L71)
- Produces: tool window id `notifications`, title `Notifications`, icon `notifications`, `defaultZone: 'right-bottom'`, registered INACTIVE (register after `tunnels` so tunnels stays the zone's active window; verify first-registrant-activates logic at tool-window-manager.js L145–179 and register in an order that leaves tunnels active). `window.notificationsPanel = { init, refresh }`. The `#bottom-panel` keeps plugin tabs; its built-in "Notifications" tab is removed.

- [ ] **Step 1: Extract shared render**

In notification-panel.js, export the entry-rendering logic (`renderNotifications` L71 internals) as `window.notificationPanel.renderInto(containerEl)` — parameterize the target element instead of the hardcoded bottom-panel content div; the existing bottom-panel path is DELETED for notifications (remove `addTab('Notifications'...)` from `init` L36 path; keep `addPluginTab` L120 and the rest). Grep for callers of anything you remove.

- [ ] **Step 2: New notifications-panel.js**

```js
(function (global) {
  'use strict';
  function create() {
    let panelEl = null;
    function init(deps) {
      panelEl = deps.panelEl;
      panelEl.classList.add('tl-scroll');
      refresh();
    }
    function refresh() {
      if (!panelEl) return;
      global.notificationPanel.renderInto(panelEl);
    }
    return { init, refresh };
  }
  global.notificationsPanel = create();
})(window);
```

Wire live updates: find where the bottom-panel version re-rendered on new notifications (grep notification-panel.js for the event/listener or the function `toast`/history hook that appended entries) and call `notificationsPanel.refresh()` from the same place (a direct call is fine — both modules are globals).

- [ ] **Step 3: Register + menu + script tag + empty state**

Register `'notifications'` after `'tunnels'` (same options shape; icon `'notifications'`). `renderInto` must render `div.tl-empty-state` with text `Nothing to show` when the history is empty (adjust the existing `.notif-empty` branch to use the class; keep `.notif-empty` as an additional class for any legacy rules). Update menu-actions.js: the notifications-related action (find it near toggle-bottom-panel L231) now calls `toolWindowManager.activate('notifications')` — read how menu-actions accesses the manager (grep for `toolWindowManager` or the runtime's exposed handle) and use the same path. Add the script tag next to other panels.

- [ ] **Step 4: Verify + commit**

```bash
node --check crates/termlab_tauri/frontend/app/ui/notification-panel.js crates/termlab_tauri/frontend/app/panels/notifications-panel.js
cargo build -p termlab_tauri && (./target/debug/termlab > /tmp/tl-p2-t5.log 2>&1 & sleep 6; kill %1) ; grep -ci error /tmp/tl-p2-t5.log
git add -A crates/termlab_tauri/frontend
git commit -m "feat(design-system): notifications history as a right-zone tool window"
```

---

### Task 6: Integration pass and visual self-check

**Files:**
- Modify: whatever the checks below surface
- Create: `docs/superpowers/specs/assets/phase2-right-side.png` (screenshot)

**Interfaces:** none new.

- [ ] **Step 1: Full launch + interaction smoke**

```bash
cargo build -p termlab_tauri
RUST_LOG=info ./target/debug/termlab > /tmp/tl-p2-final.log 2>&1 & sleep 8
```

With the app up, take a screenshot of the full window: `screencapture -x /tmp/tl-p2-shot.png` (macOS; captures the screen — acceptable). Then kill the app. Inspect the screenshot yourself (Read the png): right side must show two stacked tool windows — Hosts (toolbar with 5 icon buttons + quick connect; tree below) and Tunnels (3 icon buttons; `Nothing to show` if no tunnels) — with IntelliJ-style headers (title + gear + minimize), and the right strip showing Hosts/Tunnels/Notifications rotated labels. Compare against the JVM reference described in the spec. Fix obvious visual bugs (mis-sized icons, unstyled rows) before proceeding; note anything requiring judgment in your report.

- [ ] **Step 2: Copy screenshot into the repo**

```bash
mkdir -p docs/superpowers/specs/assets
cp /tmp/tl-p2-shot.png docs/superpowers/specs/assets/phase2-right-side.png
```

- [ ] **Step 3: Regression checks**

```bash
node --check $(git diff --name-only main -- 'crates/termlab_tauri/frontend/**/*.js' | tr '\n' ' ')  # every touched JS parses
grep -rn "#[0-9a-fA-F]\{3,8\}" crates/termlab_tauri/frontend/styles/design-system/components/*.css   # expect empty
cargo test --workspace 2>&1 | grep -cE "^test result: ok"   # expect 13
grep -ci "error\|panic" /tmp/tl-p2-final.log                # expect 0
```

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "feat(design-system): phase 2 integration pass with reference screenshot"
```

---

## Phase exit criteria

- App launches clean; Hosts + Tunnels stacked right with IntelliJ chrome; Notifications reachable from the right strip; screenshot checked in.
- All prior tests green (500 Rust, extractor, icon helper); no raw hex introduced in edited CSS.
- Human side-by-side against the JVM app is the final acceptance — the executing session hands the screenshot and the running app to the user at the end.

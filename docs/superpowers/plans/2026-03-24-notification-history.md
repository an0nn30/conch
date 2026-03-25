# Notification History & Bottom Panel Tabs Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a tabbed bottom panel with a built-in Notifications history tab and support for plugin bottom panel tabs.

**Architecture:** The bottom panel gets a tab bar with "Notifications" as the permanent first tab. `toast.js` records every notification in an in-memory array. A new `notification-panel.js` module renders the history and manages bottom panel tabs. Plugin widgets registered with `Bottom` location get their own tabs via the existing panel system.

**Tech Stack:** Frontend JS (IIFE modules), CSS custom properties, existing Tauri event system

**Spec:** `docs/superpowers/specs/2026-03-24-notification-history-design.md`

---

## File Structure

| Action | Path | Responsibility |
|--------|------|---------------|
| Modify | `crates/conch_tauri/frontend/toast.js` | Add history array, getHistory, onNotification, clearHistory |
| Create | `crates/conch_tauri/frontend/notification-panel.js` | Bottom panel tab management, notification history renderer |
| Modify | `crates/conch_tauri/frontend/index.html` | Bottom panel HTML restructure, CSS for tabs + history, script include, init call, wire up toggle + layout persistence |
| Modify | `crates/conch_tauri/src/lib.rs` | Add bottom_panel_visible to SavedLayout/WindowLayout |
| Modify | `crates/conch_tauri/frontend/plugin-widgets.js` | Route bottom-panel plugins to notification-panel tab API |

---

### Task 1: Add notification history to toast.js

**Files:**
- Modify: `crates/conch_tauri/frontend/toast.js`

- [ ] **Step 1: Add history array and notification callback support**

At the top of the IIFE (after the existing `let` declarations), add:

```javascript
const history = [];
let notificationListeners = [];
```

- [ ] **Step 2: Record every notification in the history array**

In the `show()` function, before the native notification check and in-app display, prepend:

```javascript
const record = {
  timestamp: new Date(),
  level: opts.level || 'info',
  title: opts.title || '',
  body: opts.body || '',
};
history.unshift(record);
for (const cb of notificationListeners) {
  try { cb(record); } catch (_) {}
}
```

- [ ] **Step 3: Add exported API functions**

Before the `exports.toast = { ... }` line, add:

```javascript
function getHistory() { return history; }
function onNotification(cb) { notificationListeners.push(cb); }
function clearHistory() {
  history.length = 0;
  for (const cb of notificationListeners) {
    try { cb(null); } catch (_) {} // null signals "cleared"
  }
}
```

- [ ] **Step 4: Update the exports**

```javascript
exports.toast = { show, showInApp, dismiss, configure, info, success, error, warn, getHistory, onNotification, clearHistory };
```

- [ ] **Step 5: Commit**

```bash
git add crates/conch_tauri/frontend/toast.js
git commit -m "Add notification history tracking to toast system"
```

---

### Task 2: Restructure bottom panel HTML and CSS for tabs

**Files:**
- Modify: `crates/conch_tauri/frontend/index.html`

- [ ] **Step 1: Replace bottom panel HTML**

Find the current bottom panel markup (around line 1085):

```html
<div id="bottom-panel" class="hidden">
  <div id="bottom-panel-header">Output</div>
  <div id="bottom-panel-content"></div>
</div>
```

Replace with:

```html
<div id="bottom-panel" class="hidden">
  <div id="bottom-panel-header">
    <div id="bottom-panel-tabs"></div>
    <div id="bottom-panel-actions"></div>
  </div>
  <div id="bottom-panel-content"></div>
</div>
```

- [ ] **Step 2: Update bottom panel CSS**

Replace the existing `#bottom-panel-header` CSS (around line 507) with tab-aware styles:

```css
#bottom-panel-header {
  display: flex; align-items: center; flex-shrink: 0;
  border-bottom: 1px solid var(--tab-border);
  padding: 0;
}
#bottom-panel-tabs {
  display: flex; flex: 1; overflow-x: auto; min-width: 0;
}
.bottom-tab {
  padding: 5px 14px; cursor: pointer; border: none; background: none;
  color: var(--text-muted); font-size: var(--ui-font-small);
  border-bottom: 2px solid transparent; white-space: nowrap;
}
.bottom-tab:hover { color: var(--fg); }
.bottom-tab.active { color: var(--fg); border-bottom-color: var(--blue); }
#bottom-panel-actions {
  display: flex; align-items: center; gap: 4px; padding: 0 8px; flex-shrink: 0;
}
.bottom-panel-action-btn {
  border: none; background: none; color: var(--text-muted); cursor: pointer;
  font-size: var(--ui-font-small); padding: 2px 6px; border-radius: 3px;
}
.bottom-panel-action-btn:hover { color: var(--fg); background: var(--active-highlight); }
```

Add notification history entry styles:

```css
.notif-entry {
  display: flex; align-items: flex-start; gap: 8px; padding: 3px 0;
  font-size: var(--ui-font-small); border-bottom: 1px solid var(--tab-border);
}
.notif-time { color: var(--text-muted); flex-shrink: 0; font-family: "JetBrains Mono", monospace; }
.notif-dot {
  width: 8px; height: 8px; border-radius: 50%; flex-shrink: 0; margin-top: 4px;
}
.notif-dot-info { background: var(--cyan); }
.notif-dot-success { background: var(--green); }
.notif-dot-warn { background: var(--yellow); }
.notif-dot-error { background: var(--red); }
.notif-text { flex: 1; min-width: 0; }
.notif-title { color: var(--fg); font-weight: 500; }
.notif-body { color: var(--text-muted); margin-left: 4px; }
.notif-empty { color: var(--text-muted); font-style: italic; padding: 12px 0; }
```

- [ ] **Step 3: Commit**

```bash
git add crates/conch_tauri/frontend/index.html
git commit -m "Restructure bottom panel HTML and CSS for tabbed layout"
```

---

### Task 3: Create notification-panel.js module

**Files:**
- Create: `crates/conch_tauri/frontend/notification-panel.js`

- [ ] **Step 1: Create the module**

```javascript
// Bottom panel with tabbed interface — built-in Notifications tab + plugin tabs.

(function (exports) {
  'use strict';

  let tabsEl = null;
  let actionsEl = null;
  let contentEl = null;
  let activeTabId = 'notifications';
  const pluginTabs = new Map(); // tabId -> { name, icon, renderFn }

  function init(opts) {
    tabsEl = document.getElementById('bottom-panel-tabs');
    actionsEl = document.getElementById('bottom-panel-actions');
    contentEl = document.getElementById('bottom-panel-content');

    // Build the permanent Notifications tab.
    addTab('notifications', 'Notifications');
    activateTab('notifications');

    // Add clear button to actions area.
    const clearBtn = document.createElement('button');
    clearBtn.className = 'bottom-panel-action-btn';
    clearBtn.textContent = 'Clear';
    clearBtn.title = 'Clear notification history';
    clearBtn.addEventListener('click', () => {
      if (window.toast && window.toast.clearHistory) window.toast.clearHistory();
    });
    actionsEl.appendChild(clearBtn);

    // Subscribe to new notifications for live updates.
    if (window.toast && window.toast.onNotification) {
      window.toast.onNotification((record) => {
        if (activeTabId === 'notifications') renderNotifications();
      });
    }
  }

  function addTab(id, label) {
    const btn = document.createElement('button');
    btn.className = 'bottom-tab';
    btn.textContent = label;
    btn.dataset.tabId = id;
    btn.addEventListener('click', () => activateTab(id));
    tabsEl.appendChild(btn);
  }

  function removeTab(id) {
    const btn = tabsEl.querySelector('[data-tab-id="' + id + '"]');
    if (btn) btn.remove();
    pluginTabs.delete(id);
    if (activeTabId === id) activateTab('notifications');
  }

  function activateTab(id) {
    activeTabId = id;
    // Update tab button states.
    for (const btn of tabsEl.querySelectorAll('.bottom-tab')) {
      btn.classList.toggle('active', btn.dataset.tabId === id);
    }
    // Show/hide clear button (only for notifications tab).
    if (actionsEl) {
      actionsEl.style.display = id === 'notifications' ? '' : 'none';
    }
    // Render content.
    if (id === 'notifications') {
      renderNotifications();
    } else {
      const plugin = pluginTabs.get(id);
      if (plugin && plugin.renderFn) {
        contentEl.innerHTML = '';
        plugin.renderFn(contentEl);
      }
    }
  }

  function renderNotifications() {
    if (!contentEl) return;
    contentEl.innerHTML = '';

    const history = (window.toast && window.toast.getHistory) ? window.toast.getHistory() : [];
    if (history.length === 0) {
      const empty = document.createElement('div');
      empty.className = 'notif-empty';
      empty.textContent = 'No notifications yet.';
      contentEl.appendChild(empty);
      return;
    }

    const frag = document.createDocumentFragment();
    for (const entry of history) {
      const row = document.createElement('div');
      row.className = 'notif-entry';

      const time = document.createElement('span');
      time.className = 'notif-time';
      const d = entry.timestamp;
      time.textContent = String(d.getHours()).padStart(2, '0') + ':' +
        String(d.getMinutes()).padStart(2, '0') + ':' +
        String(d.getSeconds()).padStart(2, '0');
      row.appendChild(time);

      const dot = document.createElement('span');
      dot.className = 'notif-dot notif-dot-' + (entry.level || 'info');
      row.appendChild(dot);

      const text = document.createElement('span');
      text.className = 'notif-text';
      const title = document.createElement('span');
      title.className = 'notif-title';
      title.textContent = entry.title || '';
      text.appendChild(title);
      if (entry.body) {
        const body = document.createElement('span');
        body.className = 'notif-body';
        body.textContent = entry.body;
        text.appendChild(body);
      }
      row.appendChild(text);

      frag.appendChild(row);
    }
    contentEl.appendChild(frag);
  }

  // --- Plugin tab API ---

  function addPluginTab(id, name, renderFn) {
    if (pluginTabs.has(id)) return;
    pluginTabs.set(id, { name, renderFn });
    addTab(id, name);
  }

  function removePluginTab(id) {
    removeTab(id);
  }

  function updatePluginTab(id, renderFn) {
    const plugin = pluginTabs.get(id);
    if (plugin) {
      plugin.renderFn = renderFn;
      if (activeTabId === id) activateTab(id);
    }
  }

  exports.notificationPanel = {
    init,
    activateTab,
    addPluginTab,
    removePluginTab,
    updatePluginTab,
  };
})(window);
```

- [ ] **Step 2: Add script tag in index.html**

After the `toast.js` script tag and before `ssh-panel.js`, add:

```html
<script src="notification-panel.js"></script>
```

- [ ] **Step 3: Initialize the panel in the startup code**

In the `start()` async function in `index.html`, after the toast configuration block and before the tab/terminal setup, add:

```javascript
// Initialize bottom panel tabs.
if (window.notificationPanel) {
  window.notificationPanel.init();
}
```

- [ ] **Step 4: Commit**

```bash
git add crates/conch_tauri/frontend/notification-panel.js crates/conch_tauri/frontend/index.html
git commit -m "Add notification panel module with tabbed bottom panel and history renderer"
```

---

### Task 4: Wire up bottom panel toggle with state persistence

**Files:**
- Modify: `crates/conch_tauri/src/lib.rs`
- Modify: `crates/conch_tauri/frontend/index.html`

- [ ] **Step 1: Add bottom_panel_visible to SavedLayout and WindowLayout**

In `lib.rs`, add to `SavedLayout` struct (around line 509):

```rust
bottom_panel_visible: bool,
```

Add to `get_saved_layout()` (around line 524):

```rust
bottom_panel_visible: state.layout.bottom_panel_visible,
```

Add to `WindowLayout` struct (around line 500):

```rust
bottom_panel_visible: Option<bool>,
```

Add to `save_window_layout()` (around line 537), alongside the other panel persistence:

```rust
if let Some(v) = layout.bottom_panel_visible {
    state.layout.bottom_panel_visible = v;
}
```

- [ ] **Step 2: Restore bottom panel visibility on startup**

In `index.html`, find where `get_saved_layout` is called and panel visibility is restored (search for `files_panel_visible` or `ssh_panel_visible`). Add alongside the existing panel restoration:

```javascript
if (layoutData.bottom_panel_visible === false) {
  document.getElementById('bottom-panel').classList.add('hidden');
} else {
  document.getElementById('bottom-panel').classList.remove('hidden');
}
```

- [ ] **Step 3: Update the toggle handler to persist state**

Replace the existing `toggle-bottom-panel` handler (around line 2066):

```javascript
if (action === 'toggle-bottom-panel') {
  const bp = document.getElementById('bottom-panel');
  if (bp) {
    bp.classList.toggle('hidden');
    setTimeout(() => fitAndResizeTab(currentTab()), 50);
    debouncedSaveLayout();
  }
  return;
}
```

The `debouncedSaveLayout()` function (or `saveLayoutState()`) already exists for the other panels. Make sure the bottom panel state is included in the layout data it sends. Find where `save_window_layout` is called with `ssh_panel_visible` and add:

```javascript
bottom_panel_visible: !document.getElementById('bottom-panel').classList.contains('hidden'),
```

- [ ] **Step 4: Verify it compiles**

Run: `cargo check -p conch_tauri`
Expected: Compiles

- [ ] **Step 5: Commit**

```bash
git add crates/conch_tauri/src/lib.rs crates/conch_tauri/frontend/index.html
git commit -m "Wire up bottom panel toggle with layout state persistence"
```

---

### Task 5: Route plugin bottom panels to bottom panel tabs

**Files:**
- Modify: `crates/conch_tauri/frontend/plugin-widgets.js`

- [ ] **Step 1: Detect bottom panel plugins and route to tab API**

In `plugin-widgets.js`, find where plugin panel widgets are updated (the `plugin-widgets-updated` event listener). When a panel's location is `"bottom"`, instead of rendering into a separate container, route it through the notification panel tab API:

```javascript
// Inside the plugin-widgets-updated handler, after getting panel info:
if (panel.location === 'bottom' && window.notificationPanel) {
  window.notificationPanel.addPluginTab(
    'plugin-' + panel.plugin_name,
    panel.name || panel.plugin_name,
    (container) => {
      renderWidgets(container, panel.widgets_json, panel.plugin_name);
    }
  );
  return; // Don't render in the default panel location
}
```

When a plugin is disabled, remove its tab:

```javascript
// In the plugin disable/unload handler:
if (window.notificationPanel) {
  window.notificationPanel.removePluginTab('plugin-' + pluginName);
}
```

This is the lightest-touch integration — the existing widget renderer is reused, just targeted at the bottom panel tab content area instead of a sidebar panel.

- [ ] **Step 2: Commit**

```bash
git add crates/conch_tauri/frontend/plugin-widgets.js
git commit -m "Route bottom-panel plugins to bottom panel tab system"
```

---

## Verification

After all tasks:

- [ ] `cargo check -p conch_tauri` — compiles
- [ ] `cargo test --workspace` — all tests pass
- [ ] Manual: toggle bottom panel → Notifications tab visible, empty state message shown
- [ ] Manual: trigger actions that produce toasts → entries appear in history with timestamp, level dot, title, body
- [ ] Manual: click Clear → history empties
- [ ] Manual: close panel, trigger more toasts, reopen → all entries present
- [ ] Manual: restart app → history is empty
- [ ] Manual: native notification (unfocused) → also logged in history
- [ ] Manual: toggle bottom panel → state persists across panel switches and sessions

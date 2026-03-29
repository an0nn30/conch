# Split Pane Support Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add nestable split-pane support to Conch, allowing any terminal pane to be split horizontally or vertically, with session inheritance (local shell or SSH).

**Architecture:** Hybrid JS tree + Rust sessions. JavaScript owns the binary split tree and DOM layout (nested flexbox). Rust owns session lifecycle (PTY/SSH), keyed by pane ID instead of tab ID. A new `split-tree.js` module provides pure tree manipulation functions. SSH splits reuse the existing connection by opening a new channel.

**Tech Stack:** Rust (Tauri v2 commands), JavaScript (xterm.js, DOM), CSS flexbox

**Spec:** `docs/superpowers/specs/2026-03-24-split-pane-design.md`

---

## File Structure

### New Files
- `crates/conch_tauri/frontend/split-tree.js` — Pure split tree data model and manipulation functions
- `crates/conch_tauri/frontend/split-pane.js` — Split pane DOM rendering, divider drag, focus management, context menu

### Modified Files
- `crates/conch_core/src/config/conch.rs` — Add split/close pane keyboard shortcut fields to `KeyboardConfig`
- `crates/conch_tauri/src/lib.rs` — Rename `tab_id` → `pane_id` in commands, events, `session_key`, `pty_reader_loop`; add menu items; expose new shortcuts
- `crates/conch_tauri/src/remote/mod.rs` — Rename `tab_id` → `pane_id` in SSH/SFTP commands, `SshSession`, `spawn_output_forwarder`; add `SshConnection` struct, `ssh_open_channel` command; update `ssh_disconnect` for ref-count cleanup
- `crates/conch_remote/src/ssh.rs` — Add `open_shell_channel` public function
- `crates/conch_tauri/frontend/index.html` — CSS migration; refactor tab system to use panes Map + tree roots; wire up split-tree.js and split-pane.js; update event listeners and invoke calls
- `crates/conch_tauri/frontend/files-panel.js` — `activeRemoteTabId` → `activeRemotePaneId`; `onTabChanged` → `onFocusChanged`; update all 6 `tabId` invoke call sites
- `crates/conch_tauri/frontend/plugin-widgets.js` — `writeToActivePty` routes through focused pane

### Implementation Notes

- **Atomic deploy:** Tasks 4 and 5 (Rust rename + frontend migration) must be deployed together. The app will not work with only one side done. Intermediate commits on the feature branch are acceptable.
- **Line numbers** are based on the code state at commit `b142938`. They may shift if other work lands first.
- **DOM preservation:** When re-rendering split trees after split/close, existing pane elements must NOT be removed via `innerHTML = ''`. Instead, remove only the split-containers and dividers, then re-append the preserved pane elements. This prevents xterm.js WebGL context loss.
- **Divider drag listeners:** The `setupDividerDrag` function uses event delegation on the container and must only be attached once per tab (during tab creation), not re-attached on every split/close.
- **connectionId:** The frontend constructs `connectionId` as `'conn:' + windowLabel + ':' + paneId` after a successful SSH connect, matching the Rust `connection_key()` format.

---

### Task 1: Split Tree Data Model (split-tree.js)

**Files:**
- Create: `crates/conch_tauri/frontend/split-tree.js`

This is a pure-function module with zero DOM dependency. All tree operations are testable standalone.

- [ ] **Step 1: Create split-tree.js with tree node constructors**

```js
// crates/conch_tauri/frontend/split-tree.js
// Split tree data model — pure functions, no DOM dependency.
(function () {
  'use strict';

  /** Create a leaf node. */
  function makeLeaf(paneId) {
    return { type: 'leaf', paneId };
  }

  /** Create a split node. */
  function makeSplit(direction, ratio, children) {
    return { type: 'split', direction, ratio, children };
  }

  /**
   * Split a leaf into a split node.
   * Returns the new tree root (or subtree root) with the original leaf as
   * children[0] and a new leaf (newPaneId) as children[1].
   */
  function splitLeaf(tree, targetPaneId, newPaneId, direction) {
    if (tree.type === 'leaf') {
      if (tree.paneId === targetPaneId) {
        return makeSplit(direction, 0.5, [
          makeLeaf(targetPaneId),
          makeLeaf(newPaneId),
        ]);
      }
      return tree;
    }
    // Recurse into split children.
    return makeSplit(tree.direction, tree.ratio, [
      splitLeaf(tree.children[0], targetPaneId, newPaneId, direction),
      splitLeaf(tree.children[1], targetPaneId, newPaneId, direction),
    ]);
  }

  /**
   * Remove a leaf by paneId and simplify the tree.
   * Returns the simplified tree, or null if the tree is now empty.
   */
  function removeLeaf(tree, paneId) {
    if (tree.type === 'leaf') {
      return tree.paneId === paneId ? null : tree;
    }
    const left = removeLeaf(tree.children[0], paneId);
    const right = removeLeaf(tree.children[1], paneId);
    if (left === null) return right;
    if (right === null) return left;
    return makeSplit(tree.direction, tree.ratio, [left, right]);
  }

  /** Collect all leaf pane IDs in the tree. */
  function allLeaves(tree) {
    if (tree.type === 'leaf') return [tree.paneId];
    return [...allLeaves(tree.children[0]), ...allLeaves(tree.children[1])];
  }

  /** Find the first leaf's paneId (depth-first). */
  function firstLeaf(tree) {
    if (tree.type === 'leaf') return tree.paneId;
    return firstLeaf(tree.children[0]);
  }

  /** Count leaf nodes. */
  function leafCount(tree) {
    if (tree.type === 'leaf') return 1;
    return leafCount(tree.children[0]) + leafCount(tree.children[1]);
  }

  /**
   * Find the parent split node of a given paneId and which child index (0 or 1).
   * Returns { parent, index } or null if not found.
   */
  function findParent(tree, paneId) {
    if (tree.type === 'leaf') return null;
    for (let i = 0; i < 2; i++) {
      const child = tree.children[i];
      if (child.type === 'leaf' && child.paneId === paneId) {
        return { parent: tree, index: i };
      }
      const found = findParent(child, paneId);
      if (found) return found;
    }
    return null;
  }

  /**
   * Update the ratio of the split node that is the parent of paneId.
   * Returns a new tree with the updated ratio (immutable).
   */
  function updateRatio(tree, paneId, newRatio) {
    if (tree.type === 'leaf') return tree;
    for (let i = 0; i < 2; i++) {
      if (tree.children[i].type === 'leaf' && tree.children[i].paneId === paneId) {
        return makeSplit(tree.direction, newRatio, tree.children);
      }
    }
    return makeSplit(tree.direction, tree.ratio, [
      updateRatio(tree.children[0], paneId, newRatio),
      updateRatio(tree.children[1], paneId, newRatio),
    ]);
  }

  window.splitTree = {
    makeLeaf,
    makeSplit,
    splitLeaf,
    removeLeaf,
    allLeaves,
    firstLeaf,
    leafCount,
    findParent,
    updateRatio,
  };
})();
```

- [ ] **Step 2: Verify the file is syntactically valid**

Run: `node --check crates/conch_tauri/frontend/split-tree.js`
Expected: No output (clean parse)

- [ ] **Step 3: Commit**

```bash
git add crates/conch_tauri/frontend/split-tree.js
git commit -m "Add split tree pure data model (split-tree.js)"
```

---

### Task 2: KeyboardConfig — Add Split Pane Shortcut Fields

**Files:**
- Modify: `crates/conch_core/src/config/conch.rs`
- Test: `crates/conch_core/src/config/conch.rs` (inline tests)

- [ ] **Step 1: Write failing tests for new keyboard config fields**

Add to the existing `#[cfg(test)] mod tests` at `conch.rs:161`:

```rust
#[test]
fn keyboard_config_split_pane_defaults() {
    let cfg = KeyboardConfig::default();
    assert_eq!(cfg.split_vertical, "cmd+d");
    assert_eq!(cfg.split_horizontal, "cmd+shift+d");
    assert_eq!(cfg.close_pane, "cmd+shift+w");
    assert_eq!(cfg.navigate_pane_up, "cmd+alt+up");
    assert_eq!(cfg.navigate_pane_down, "cmd+alt+down");
    assert_eq!(cfg.navigate_pane_left, "cmd+alt+left");
    assert_eq!(cfg.navigate_pane_right, "cmd+alt+right");
}

#[test]
fn keyboard_config_serde_default_fills_split_pane_fields() {
    let toml_str = r#"new_tab = "cmd+t""#;
    let cfg: KeyboardConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(cfg.split_vertical, "cmd+d");
    assert_eq!(cfg.close_pane, "cmd+shift+w");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p conch_core keyboard_config_split`
Expected: FAIL — fields don't exist yet

- [ ] **Step 3: Add fields to KeyboardConfig struct and Default impl**

In `conch.rs`, add to the `KeyboardConfig` struct (after `toggle_bottom_panel` at line 91):

```rust
pub split_vertical: String,
pub split_horizontal: String,
pub close_pane: String,
pub navigate_pane_up: String,
pub navigate_pane_down: String,
pub navigate_pane_left: String,
pub navigate_pane_right: String,
```

In the `Default` impl (after `toggle_bottom_panel` at line 104):

```rust
split_vertical: "cmd+d".into(),
split_horizontal: "cmd+shift+d".into(),
close_pane: "cmd+shift+w".into(),
navigate_pane_up: "cmd+alt+up".into(),
navigate_pane_down: "cmd+alt+down".into(),
navigate_pane_left: "cmd+alt+left".into(),
navigate_pane_right: "cmd+alt+right".into(),
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p conch_core keyboard_config_split`
Expected: PASS

- [ ] **Step 5: Run full conch_core tests**

Run: `cargo test -p conch_core`
Expected: All tests PASS (serde(default) ensures backward compat)

- [ ] **Step 6: Commit**

```bash
git add crates/conch_core/src/config/conch.rs
git commit -m "Add split pane keyboard shortcut fields to KeyboardConfig"
```

---

### Task 3: CSS for Split Layout

**Files:**
- Modify: `crates/conch_tauri/frontend/index.html` (CSS section only)

Add new CSS classes for the split layout system. These are additive — existing `.terminal-pane` styles are not changed yet (that happens in Task 6).

- [ ] **Step 1: Add split layout CSS classes**

Add the following CSS after the existing `.terminal-pane.active` rule block in `index.html`:

```css
/* --- Split pane layout --- */
.tab-tree-root {
  position: absolute;
  inset: 0;
  display: none;
}
.tab-tree-root.active {
  display: flex;
}
.split-container {
  display: flex;
  flex: 1;
  min-width: 0;
  min-height: 0;
  overflow: hidden;
}
.split-divider {
  flex-shrink: 0;
  background: var(--border);
  z-index: 1;
}
.split-divider.vertical {
  width: 4px;
  cursor: col-resize;
}
.split-divider.horizontal {
  height: 4px;
  cursor: row-resize;
}
.split-divider:hover {
  background: var(--blue);
}
.terminal-pane.focused {
  border-top: 2px solid var(--blue);
}
.terminal-pane:not(.focused) {
  border-top: 2px solid transparent;
}
```

- [ ] **Step 2: Verify build succeeds**

Run: `cargo build -p conch_tauri` (verifies index.html is valid and included)
Expected: PASS

- [ ] **Step 3: Commit**

```bash
git add crates/conch_tauri/frontend/index.html
git commit -m "Add CSS classes for split pane layout"
```

---

### Task 4: Rename tab_id → pane_id in Rust Backend

**Files:**
- Modify: `crates/conch_tauri/src/lib.rs`
- Modify: `crates/conch_tauri/src/remote/mod.rs`

This is a mechanical rename across all Rust commands, event structs, and helpers. The backend semantics don't change — only the parameter name.

- [ ] **Step 1: Write a test for the renamed session_key**

Add to `lib.rs` tests (after `resolved_shell_uses_configured_program_and_args` test):

```rust
#[test]
fn session_key_uses_pane_id() {
    let key = session_key("main", 42);
    assert_eq!(key, "main:42");
}
```

- [ ] **Step 2: Rename in lib.rs — session_key, event structs, commands**

In `lib.rs`:

1. `session_key` function (line 115): rename `tab_id` parameter to `pane_id`
2. `PtyOutputEvent` struct (line 88-92): rename `tab_id` field to `pane_id`
3. `PtyExitEvent` struct (line 94-98): rename `tab_id` field to `pane_id`
4. `spawn_shell` (line 119-160): rename `tab_id` parameter and all references to `pane_id`
5. `write_to_pty` (line 162-173): rename `tab_id` to `pane_id`
6. `resize_pty` (line 175-187): rename `tab_id` to `pane_id`
7. `close_pty` (line 189-193): rename `tab_id` to `pane_id`
8. `pty_reader_loop` (line 1261-1318): rename `tab_id` parameter and all references to `pane_id`

Use find-and-replace within each function scope. Keep the error messages consistent (e.g., `"Pane {pane_id} already exists on window {window_label}"`).

- [ ] **Step 3: Rename in remote/mod.rs — session_key, SSH commands, SFTP commands, spawn_output_forwarder**

In `remote/mod.rs`:

1. `session_key` (line 238): rename `tab_id` to `pane_id`
2. `spawn_output_forwarder` (line 110-136): rename `tab_id` to `pane_id`
3. `PtyOutputEvent` / `PtyExitEvent` references in the SSH channel loop and spawn_output_forwarder: update to use `pane_id` field
4. `ssh_connect` (line 244-362): rename `tab_id` to `pane_id` (parameter and all usages)
5. `ssh_quick_connect` (line 366-482): rename `tab_id` to `pane_id`
6. `ssh_write` (line 486-499): rename `tab_id` to `pane_id`
7. `ssh_resize` (line 503-517): rename `tab_id` to `pane_id`
8. `ssh_disconnect` (line 521-531): rename `tab_id` to `pane_id`
9. `get_ssh_handle` (line 887-898): rename `tab_id` to `pane_id`
10. All 8 SFTP commands (lines 901-1015): rename `tab_id` to `pane_id`
11. `transfer_download` and `transfer_upload`: rename `tab_id` to `pane_id`

- [ ] **Step 4: Run the full test suite**

Run: `cargo test --workspace`
Expected: All tests PASS (the rename is purely cosmetic at the Rust level)

- [ ] **Step 5: Commit**

```bash
git add crates/conch_tauri/src/lib.rs crates/conch_tauri/src/remote/mod.rs
git commit -m "Rename tab_id to pane_id across all Rust commands and events"
```

---

### Task 5: Frontend Data Model Migration

**Files:**
- Modify: `crates/conch_tauri/frontend/index.html`

Refactor the frontend tab system to use a `panes` Map alongside the existing `tabs` Map. Each tab gains a `treeRoot` and `focusedPaneId`. Event listeners and invoke calls change from `tabId` to `paneId`.

**IMPORTANT:** This task and Task 4 (Rust rename) must both be deployed together. After Task 4, the backend expects `pane_id`; after this task, the frontend sends `paneId`.

- [ ] **Step 1: Add panes Map and helper functions**

Near the existing `const tabs = new Map()` declaration, add:

```js
const panes = new Map();
let nextPaneId = 1;
let focusedPaneId = null;

function currentPane() {
  return panes.get(focusedPaneId) || null;
}

function getTabForPane(paneId) {
  const pane = panes.get(paneId);
  return pane ? tabs.get(pane.tabId) : null;
}

function allPanesInTab(tabId) {
  const tab = tabs.get(tabId);
  if (!tab) return [];
  return window.splitTree.allLeaves(tab.treeRoot);
}

function setFocusedPane(paneId) {
  if (focusedPaneId === paneId) return;
  // Remove old focus indicator.
  if (focusedPaneId != null) {
    const oldPane = panes.get(focusedPaneId);
    if (oldPane && oldPane.root) oldPane.root.classList.remove('focused');
  }
  focusedPaneId = paneId;
  const pane = panes.get(paneId);
  if (pane && pane.root) {
    pane.root.classList.add('focused');
    pane.term.focus();
    // Update the tab's last-focused pane.
    const tab = tabs.get(pane.tabId);
    if (tab) tab.focusedPaneId = paneId;
  }
}
```

- [ ] **Step 2: Refactor createTab() to use panes**

Modify `createTab()` to:
1. Allocate a `paneId` from `nextPaneId++`
2. Create a `.tab-tree-root` wrapper div instead of a bare `.terminal-pane`
3. Create the `.terminal-pane` inside the wrapper with `data-pane-id`
4. Store the pane in the `panes` Map
5. Set `tab.treeRoot = window.splitTree.makeLeaf(paneId)` on the tab object
6. Set `tab.focusedPaneId = paneId`
7. Store `tab.containerEl` = the `.tab-tree-root` div
8. Remove `tab.term`, `tab.fitAddon`, `tab.root`, `tab.spawned` — these are now on the pane
9. Call `invoke('spawn_shell', { paneId, cols, rows })` instead of `tabId`

- [ ] **Step 3: Refactor createSshTab() similarly**

Same pattern as createTab but calls `ssh_connect` / `ssh_quick_connect` with `paneId`.

- [ ] **Step 4: Refactor activateTab()**

Change from toggling `.terminal-pane.active` to toggling `.tab-tree-root.active`:
1. Hide previous tab's `.tab-tree-root` (remove `active` class)
2. Show new tab's `.tab-tree-root` (add `active` class)
3. Restore focus to `tab.focusedPaneId`
4. Call `fitAndResizePane` on the focused pane

- [ ] **Step 5: Update pty-output listener**

Change from:
```js
const tab = tabs.get(payload.tab_id);
if (tab) tab.term.write(payload.data);
```
To:
```js
const pane = panes.get(payload.pane_id);
if (pane && pane.term) pane.term.write(payload.data);
```

- [ ] **Step 6: Update pty-exit listener**

Change to look up the pane, determine its tab, then close the pane (or the tab if it's the last pane).

- [ ] **Step 7: Update term.onData handler**

Each pane's terminal gets its own `onData` callback:
```js
term.onData((data) => {
  if (!pane.spawned) return;
  const cmd = pane.type === 'ssh' ? 'ssh_write' : 'write_to_pty';
  invoke(cmd, { paneId: pane.paneId, data });
});
```

- [ ] **Step 8: Update all remaining invoke calls from tabId to paneId**

Search for `tabId` in invoke calls and replace with `paneId`, sourced from the appropriate pane object. Key call sites:
- `resize_pty` / `ssh_resize`
- `close_pty` / `ssh_disconnect`
- Drag-drop handler

- [ ] **Step 9: Update closeTab() to close all panes**

When closing a tab, walk the tree to get all pane IDs, dispose each pane's xterm instance and ResizeObserver, and close each backend session.

- [ ] **Step 10: Migrate the existing `.terminal-pane` CSS**

Change the existing `.terminal-pane` styles from `position: absolute; inset: 0` to:
```css
.terminal-pane {
  position: relative;
  min-width: 80px;
  min-height: 80px;
  overflow: hidden;
  flex: 1;
}
```

Remove the `.terminal-pane.active` rule (visibility is now controlled by `.tab-tree-root.active`).

- [ ] **Step 11: Add split-tree.js script tag to index.html**

Add `<script src="split-tree.js"></script>` before the main inline script block, ensuring `window.splitTree` is available.

- [ ] **Step 12: Verify the app builds and a single-pane tab works**

Run: `cargo build -p conch_tauri`
Expected: PASS — app compiles, single-pane tabs work identically to before

- [ ] **Step 13: Commit**

```bash
git add crates/conch_tauri/frontend/index.html
git commit -m "Refactor frontend tab system to pane-based data model"
```

---

### Task 6: Split Pane DOM Rendering and ResizeObserver

**Files:**
- Create: `crates/conch_tauri/frontend/split-pane.js`
- Modify: `crates/conch_tauri/frontend/index.html`

Build the DOM rendering engine that turns a split tree into nested flexbox containers, and migrate to per-pane ResizeObservers.

- [ ] **Step 1: Create split-pane.js with renderTree and per-pane resize**

```js
// crates/conch_tauri/frontend/split-pane.js
// Split pane DOM rendering, divider drag, focus management.
(function () {
  'use strict';

  /**
   * Render a split tree into a DOM element.
   * @param {Object} tree - split tree node
   * @param {Function} getPaneEl - (paneId) => HTMLElement for the pane's .terminal-pane div
   * @returns {HTMLElement} the root DOM element for this tree node
   */
  function renderTree(tree, getPaneEl) {
    if (tree.type === 'leaf') {
      return getPaneEl(tree.paneId);
    }

    const container = document.createElement('div');
    container.className = 'split-container';
    container.style.flexDirection = tree.direction === 'vertical' ? 'row' : 'column';

    const child0El = renderTree(tree.children[0], getPaneEl);
    const child1El = renderTree(tree.children[1], getPaneEl);

    child0El.style.flex = String(tree.ratio);
    child1El.style.flex = String(1 - tree.ratio);

    const divider = document.createElement('div');
    divider.className = 'split-divider ' + (tree.direction === 'vertical' ? 'vertical' : 'horizontal');
    divider.dataset.direction = tree.direction;

    container.appendChild(child0El);
    container.appendChild(divider);
    container.appendChild(child1El);

    return container;
  }

  /**
   * Set up a ResizeObserver for a pane that debounces fit + resize.
   * @param {Object} pane - pane object from panes Map
   * @param {Function} resizeFn - (pane) => void, called on resize
   * @returns {ResizeObserver}
   */
  function createPaneResizeObserver(pane, resizeFn) {
    const observer = new ResizeObserver(() => {
      clearTimeout(pane.debounceTimer);
      pane.debounceTimer = setTimeout(() => resizeFn(pane), 100);
    });
    observer.observe(pane.root);
    return observer;
  }

  /**
   * Set up divider drag on a tab container via event delegation.
   * IMPORTANT: Call this ONCE per tab (during tab creation). The delegated
   * listener handles all dividers, including those added by future splits.
   * Do NOT re-attach on split/close — the delegation handles new dividers
   * automatically.
   *
   * @param {HTMLElement} containerEl - the .tab-tree-root element
   * @param {Function} getTreeRoot - () => tree, returns the current tree root
   * @param {Function} setTreeRoot - (tree) => void, updates the tree root
   */
  function setupDividerDrag(containerEl, getTreeRoot, setTreeRoot) {
    containerEl.addEventListener('pointerdown', (e) => {
      if (!e.target.classList.contains('split-divider')) return;
      e.preventDefault();

      const divider = e.target;
      const parent = divider.parentElement;
      const direction = divider.dataset.direction;
      const child0 = divider.previousElementSibling;
      const child1 = divider.nextElementSibling;

      const parentRect = parent.getBoundingClientRect();
      const isVertical = direction === 'vertical';
      const totalSize = isVertical ? parentRect.width : parentRect.height;
      const minPx = 80;

      // Identify the split node: collect the immediate child pane IDs of
      // this split container (not deeply nested ones).
      const child0PaneId = getImmediatePaneId(child0);
      const child1PaneId = getImmediatePaneId(child1);

      divider.setPointerCapture(e.pointerId);

      function onMove(ev) {
        const pos = isVertical
          ? ev.clientX - parentRect.left
          : ev.clientY - parentRect.top;
        let ratio = pos / totalSize;
        const minRatio = minPx / totalSize;
        const maxRatio = 1 - minRatio;
        ratio = Math.max(minRatio, Math.min(maxRatio, ratio));

        child0.style.flex = String(ratio);
        child1.style.flex = String(1 - ratio);

        // Update the tree — use the known child pane IDs to find the correct
        // split node, even in nested trees.
        const tree = getTreeRoot();
        const updated = updateRatioByChildren(tree, child0PaneId, child1PaneId, ratio);
        setTreeRoot(updated);
      }

      function onUp() {
        divider.removeEventListener('pointermove', onMove);
        divider.removeEventListener('pointerup', onUp);
      }

      divider.addEventListener('pointermove', onMove);
      divider.addEventListener('pointerup', onUp);
    });
  }

  /**
   * Get the pane ID of a child element — if it's a .terminal-pane, return its
   * data-pane-id. If it's a .split-container, return the first leaf pane ID.
   */
  function getImmediatePaneId(el) {
    if (el.dataset && el.dataset.paneId) return parseInt(el.dataset.paneId, 10);
    const first = el.querySelector('[data-pane-id]');
    return first ? parseInt(first.dataset.paneId, 10) : null;
  }

  /**
   * Find a split node whose children contain the given pane IDs and update its ratio.
   * This correctly identifies the split node even in deeply nested trees.
   */
  function updateRatioByChildren(tree, child0PaneId, child1PaneId, newRatio) {
    if (tree.type === 'leaf') return tree;
    // Check if this split's children match.
    const left = window.splitTree.allLeaves(tree.children[0]);
    const right = window.splitTree.allLeaves(tree.children[1]);
    if (left.includes(child0PaneId) && right.includes(child1PaneId)) {
      return window.splitTree.makeSplit(tree.direction, newRatio, tree.children);
    }
    // Recurse.
    return window.splitTree.makeSplit(tree.direction, tree.ratio, [
      updateRatioByChildren(tree.children[0], child0PaneId, child1PaneId, newRatio),
      updateRatioByChildren(tree.children[1], child0PaneId, child1PaneId, newRatio),
    ]);
  }

  /**
   * Find the spatially adjacent pane in a given direction.
   * @param {number} currentPaneId
   * @param {'up'|'down'|'left'|'right'} direction
   * @param {HTMLElement} containerEl - the .tab-tree-root
   * @returns {number|null} the adjacent pane's ID, or null
   */
  function findAdjacentPane(currentPaneId, direction, containerEl) {
    const currentEl = containerEl.querySelector(`[data-pane-id="${currentPaneId}"]`);
    if (!currentEl) return null;

    const currentRect = currentEl.getBoundingClientRect();
    const cx = currentRect.left + currentRect.width / 2;
    const cy = currentRect.top + currentRect.height / 2;

    const allPaneEls = containerEl.querySelectorAll('[data-pane-id]');
    let bestId = null;
    let bestDist = Infinity;

    for (const el of allPaneEls) {
      const id = parseInt(el.dataset.paneId, 10);
      if (id === currentPaneId) continue;

      const r = el.getBoundingClientRect();
      const ex = r.left + r.width / 2;
      const ey = r.top + r.height / 2;

      // Check if the candidate is in the right direction.
      let valid = false;
      if (direction === 'left' && ex < cx) valid = true;
      if (direction === 'right' && ex > cx) valid = true;
      if (direction === 'up' && ey < cy) valid = true;
      if (direction === 'down' && ey > cy) valid = true;

      if (!valid) continue;

      const dist = Math.hypot(ex - cx, ey - cy);
      if (dist < bestDist) {
        bestDist = dist;
        bestId = id;
      }
    }

    return bestId;
  }

  window.splitPane = {
    renderTree,
    createPaneResizeObserver,
    setupDividerDrag,
    findAdjacentPane,
  };
})();
```

- [ ] **Step 2: Verify syntax**

Run: `node --check crates/conch_tauri/frontend/split-pane.js`
Expected: Clean parse

- [ ] **Step 3: Add script tag to index.html**

Add `<script src="split-pane.js"></script>` after the `split-tree.js` script tag.

- [ ] **Step 4: Remove the global ResizeObserver on #terminal-host**

Remove or replace the existing `new ResizeObserver(...)` that watches `#terminal-host`. Each pane now has its own observer set up via `createPaneResizeObserver`.

- [ ] **Step 5: Wire per-pane ResizeObserver into pane creation**

In `createTab()` and `createSshTab()`, after creating the pane's `.terminal-pane` element:

```js
pane.resizeObserver = window.splitPane.createPaneResizeObserver(pane, fitAndResizePane);
```

Create a `fitAndResizePane(pane)` function:

```js
function fitAndResizePane(pane) {
  if (!pane || !pane.term || !pane.fitAddon || !pane.spawned) return;
  const dims = pane.fitAddon.proposeDimensions();
  if (!dims || !dims.cols || !dims.rows) return;
  if (dims.cols === pane.lastCols && dims.rows === pane.lastRows) return;
  pane.lastCols = dims.cols;
  pane.lastRows = dims.rows;
  pane.fitAddon.fit();
  const cmd = pane.type === 'ssh' ? 'ssh_resize' : 'resize_pty';
  invoke(cmd, { paneId: pane.paneId, cols: dims.cols, rows: dims.rows });
}
```

- [ ] **Step 6: Verify build and single-pane tabs still work**

Run: `cargo build -p conch_tauri`
Expected: PASS

- [ ] **Step 7: Commit**

```bash
git add crates/conch_tauri/frontend/split-pane.js crates/conch_tauri/frontend/index.html
git commit -m "Add split pane DOM rendering and per-pane ResizeObserver"
```

---

### Task 7: Split Action — splitPane()

**Files:**
- Modify: `crates/conch_tauri/frontend/index.html`

Implement the `splitPane(direction)` function that splits the currently focused pane.

- [ ] **Step 1: Implement splitPane() for local shells**

```js
async function splitPane(direction) {
  const pane = currentPane();
  if (!pane) return;

  const tab = tabs.get(pane.tabId);
  if (!tab) return;

  const newPaneId = nextPaneId++;

  // Update the tree.
  tab.treeRoot = window.splitTree.splitLeaf(tab.treeRoot, pane.paneId, newPaneId, direction);

  // Create the new pane's terminal.
  const newPaneEl = document.createElement('div');
  newPaneEl.className = 'terminal-pane';
  newPaneEl.dataset.paneId = newPaneId;

  const { term, fitAddon } = initTerminal(newPaneEl);

  const newPane = {
    paneId: newPaneId,
    tabId: tab.id,
    type: pane.type,
    connectionId: pane.connectionId || null,
    term,
    fitAddon,
    root: newPaneEl,
    spawned: false,
    lastCols: 0,
    lastRows: 0,
    cleanupMouseBridge: null,
    resizeObserver: null,
    debounceTimer: null,
  };
  panes.set(newPaneId, newPane);

  // Re-render the tree into the tab's container.
  // IMPORTANT: Use rebuildTreeDOM() which preserves existing pane elements
  // (prevents xterm.js WebGL context loss). Only split-containers and
  // dividers are recreated; pane .terminal-pane divs are re-appended.
  rebuildTreeDOM(tab);

  // Divider drag is already set up via event delegation (once per tab).
  // No re-attachment needed — new dividers are handled automatically.

  // Set up ResizeObserver for the new pane.
  newPane.resizeObserver = window.splitPane.createPaneResizeObserver(newPane, fitAndResizePane);

  // Set up mouse bridge if needed.
  newPane.cleanupMouseBridge = setupTmuxRightClickBridge(term, newPaneEl);

  // Spawn the session.
  const dims = fitAddon.proposeDimensions() || { cols: 80, rows: 24 };

  if (pane.type === 'ssh' && pane.connectionId) {
    // SSH: open a new channel on the existing connection.
    try {
      await invoke('ssh_open_channel', {
        paneId: newPaneId,
        connectionId: pane.connectionId,
        cols: dims.cols,
        rows: dims.rows,
      });
      newPane.spawned = true;
    } catch (e) {
      window.toast.error('Failed to open SSH channel: ' + e);
    }
  } else {
    // Local shell.
    try {
      await invoke('spawn_shell', {
        paneId: newPaneId,
        cols: dims.cols,
        rows: dims.rows,
      });
      newPane.spawned = true;
    } catch (e) {
      window.toast.error('Failed to spawn shell: ' + e);
    }
  }

  // Wire up input handler.
  term.onData((data) => {
    if (!newPane.spawned) return;
    const cmd = newPane.type === 'ssh' ? 'ssh_write' : 'write_to_pty';
    invoke(cmd, { paneId: newPaneId, data });
  });

  // Focus the new pane.
  setFocusedPane(newPaneId);
}
```

- [ ] **Step 2: Implement rebuildTreeDOM() helper**

This function re-renders the split tree DOM without destroying existing pane elements:

```js
function rebuildTreeDOM(tab) {
  const containerEl = tab.containerEl;
  // Remove only split-containers and dividers, NOT pane elements.
  // Detach all children first (pane elements are preserved in panes Map).
  while (containerEl.firstChild) {
    containerEl.removeChild(containerEl.firstChild);
  }
  // Re-render from the tree, re-appending existing pane elements.
  const rendered = window.splitPane.renderTree(tab.treeRoot, (id) => {
    return panes.get(id).root;
  });
  containerEl.appendChild(rendered);
}
```

Note: `removeChild` detaches elements without destroying them. The pane's `.terminal-pane` div is still referenced by `panes.get(id).root`, so xterm.js's canvas and WebGL context survive the re-attachment.

- [ ] **Step 3: Set up divider drag ONCE during tab creation**

In `createTab()` and `createSshTab()`, after creating `tab.containerEl`:

```js
window.splitPane.setupDividerDrag(
  tab.containerEl,
  () => tab.treeRoot,
  (newTree) => { tab.treeRoot = newTree; }
);
```

This is event delegation — it handles all current and future dividers.

- [ ] **Step 4: Store connectionId on SSH panes**

In `createSshTab()`, after a successful `ssh_connect` / `ssh_quick_connect`, set:

```js
pane.connectionId = 'conn:' + windowLabel + ':' + pane.paneId;
```

This matches the Rust `connection_key()` format and is needed by `splitPane()` when spawning new SSH channels.

- [ ] **Step 5: Verify build**

Run: `cargo build -p conch_tauri`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add crates/conch_tauri/frontend/index.html
git commit -m "Implement splitPane() function for local and SSH sessions"
```

---

### Task 8: Pane Close and Tree Simplification

**Files:**
- Modify: `crates/conch_tauri/frontend/index.html`

- [ ] **Step 1: Implement closePane()**

```js
function closePane(paneId) {
  const pane = panes.get(paneId);
  if (!pane) return;

  const tab = tabs.get(pane.tabId);
  if (!tab) return;

  // If this is the last pane in the tab, close the tab.
  if (window.splitTree.leafCount(tab.treeRoot) <= 1) {
    closeTab(tab.id);
    return;
  }

  // Terminate the backend session.
  if (pane.type === 'ssh') {
    invoke('ssh_disconnect', { paneId }).catch(() => {});
  } else {
    invoke('close_pty', { paneId }).catch(() => {});
  }

  // Dispose xterm and observers.
  if (pane.cleanupMouseBridge) pane.cleanupMouseBridge();
  if (pane.resizeObserver) pane.resizeObserver.disconnect();
  pane.term.dispose();
  panes.delete(paneId);

  // Remove from tree and simplify.
  tab.treeRoot = window.splitTree.removeLeaf(tab.treeRoot, paneId);

  // Re-render (preserves existing pane elements, no innerHTML = '').
  rebuildTreeDOM(tab);
  // Divider drag is already delegated — no re-attachment needed.

  // Refocus.
  if (focusedPaneId === paneId) {
    const firstId = window.splitTree.firstLeaf(tab.treeRoot);
    setFocusedPane(firstId);
  }
}
```

- [ ] **Step 2: Update pty-exit listener to close panes**

When a `pty-exit` event arrives, call `closePane(pane_id)` instead of `closeTab`.

- [ ] **Step 3: Verify build**

Run: `cargo build -p conch_tauri`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add crates/conch_tauri/frontend/index.html
git commit -m "Implement pane close with tree simplification"
```

---

### Task 9: Focus Management and Keyboard Navigation

**Files:**
- Modify: `crates/conch_tauri/frontend/index.html`

- [ ] **Step 1: Add click-to-focus handler**

On each pane's `.terminal-pane` element, add a `mousedown` listener:

```js
newPaneEl.addEventListener('mousedown', () => setFocusedPane(paneId));
```

Add the same handler in `createTab()` for the initial pane.

- [ ] **Step 2: Add keyboard navigation handler**

In the global keydown handler (capture phase), detect `Cmd+Alt+Arrow`:

```js
document.addEventListener('keydown', (e) => {
  if ((e.metaKey || e.ctrlKey) && e.altKey && ['ArrowUp','ArrowDown','ArrowLeft','ArrowRight'].includes(e.key)) {
    e.preventDefault();
    e.stopPropagation();
    const dir = e.key.replace('Arrow', '').toLowerCase();
    const tab = tabs.get(activeTabId);
    if (!tab) return;
    const adj = window.splitPane.findAdjacentPane(focusedPaneId, dir, tab.containerEl);
    if (adj != null) setFocusedPane(adj);
  }
}, true);
```

- [ ] **Step 3: Verify build**

Run: `cargo build -p conch_tauri`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add crates/conch_tauri/frontend/index.html
git commit -m "Add click-to-focus and keyboard pane navigation"
```

---

### Task 10: SSH Multi-Channel Support

**Files:**
- Modify: `crates/conch_tauri/src/remote/mod.rs`
- Test: `crates/conch_tauri/src/remote/mod.rs` (inline tests)

- [ ] **Step 1: Write tests for SshConnection ref counting**

Add to the existing test module in `remote/mod.rs`:

```rust
#[test]
fn connection_key_format() {
    let key = connection_key("main", 1);
    assert_eq!(key, "conn:main:1");
}

#[test]
fn connection_key_differs_from_session_key() {
    let ck = connection_key("main", 1);
    let sk = session_key("main", 1);
    assert_ne!(ck, sk);
    assert!(ck.starts_with("conn:"));
}

#[test]
fn remote_state_new_has_no_connections() {
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let state = RemoteState::new(tx);
    assert!(state.connections.is_empty());
}
```

- [ ] **Step 2: Update the test_state_with helper**

The `test_state_with` helper at `remote/mod.rs:1568-1587` constructs `RemoteState` directly. Add the new `connections` field:

```rust
fn test_state_with(
    config: SshConfig,
    ssh_config_entries: Vec<ServerEntry>,
) -> RemoteState {
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    RemoteState {
        sessions: HashMap::new(),
        connections: HashMap::new(),  // NEW
        config,
        ssh_config_entries,
        pending_prompts: Arc::new(Mutex::new(PendingPrompts::new())),
        tunnel_manager: TunnelManager::new(),
        transfers: Arc::new(Mutex::new(TransferRegistry::new())),
        transfer_progress_tx: tx,
        paths: RemotePaths {
            known_hosts_file: std::path::PathBuf::from("/tmp/test_known_hosts"),
            config_dir: std::path::PathBuf::from("/tmp/test_config"),
            default_key_paths: vec![],
        },
    }
}
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p conch_tauri connection_key`
Expected: FAIL — `connection_key` doesn't exist yet

- [ ] **Step 3: Add SshConnection struct and connection_key helper**

```rust
/// A shared SSH connection that may serve multiple pane channels.
pub(crate) struct SshConnection {
    pub ssh_handle: Arc<conch_remote::russh::client::Handle<ConchSshHandler>>,
    pub host: String,
    pub user: String,
    pub port: u16,
    pub ref_count: u32,
}

fn connection_key(window_label: &str, pane_id: u32) -> String {
    format!("conn:{window_label}:{pane_id}")
}
```

Add `connections: HashMap<String, SshConnection>` to `RemoteState`:

```rust
pub connections: HashMap<String, SshConnection>,
```

Initialize in `RemoteState::new()`:

```rust
connections: HashMap::new(),
```

- [ ] **Step 4: Add `connection_id` field to SshSession**

```rust
pub(crate) struct SshSession {
    pub input_tx: mpsc::UnboundedSender<ChannelInput>,
    pub connection_id: String,
    pub host: String,
    pub user: String,
    pub port: u16,
}
```

Remove `ssh_handle` from `SshSession` — it now lives in `SshConnection`.

- [ ] **Step 5: Update ssh_connect to create SshConnection + SshSession**

In `ssh_connect`, after a successful connection:

```rust
let conn_key = connection_key(&window_label, pane_id);
{
    let mut state = remote_clone.lock();
    state.connections.insert(conn_key.clone(), SshConnection {
        ssh_handle: Arc::new(ssh_handle),
        host: server.host.clone(),
        user: credentials.username.clone(),
        port: server.port,
        ref_count: 1,
    });
    state.sessions.insert(key.clone(), SshSession {
        input_tx,
        connection_id: conn_key.clone(),
        host: server.host.clone(),
        user: credentials.username.clone(),
        port: server.port,
    });
}
```

Update the channel loop cleanup to decrement ref_count:

```rust
tokio::spawn(async move {
    let exited = conch_remote::ssh::channel_loop(channel, input_rx, output_tx).await;
    let mut state = remote_for_loop.lock();
    if let Some(session) = state.sessions.remove(&key_for_loop) {
        if let Some(conn) = state.connections.get_mut(&session.connection_id) {
            conn.ref_count -= 1;
            if conn.ref_count == 0 {
                state.connections.remove(&session.connection_id);
            }
        }
    }
    drop(state);
    if exited {
        let _ = app_handle.emit_to(&wl, "pty-exit", PtyExitEvent { window_label: wl.clone(), pane_id });
    }
});
```

- [ ] **Step 6: Update ssh_quick_connect similarly**

Same pattern as ssh_connect.

- [ ] **Step 7: Update get_ssh_handle to use connections**

```rust
fn get_ssh_handle(
    state: &RemoteState,
    window_label: &str,
    pane_id: u32,
) -> Result<Arc<conch_remote::russh::client::Handle<ConchSshHandler>>, String> {
    let key = session_key(window_label, pane_id);
    let session = state.sessions.get(&key)
        .ok_or_else(|| format!("No SSH session for {key}"))?;
    state.connections.get(&session.connection_id)
        .map(|c| Arc::clone(&c.ssh_handle))
        .ok_or_else(|| format!("No SSH connection for {}", session.connection_id))
}
```

- [ ] **Step 8: Add open_shell_channel to conch_remote (REQUIRED)**

This function does NOT exist yet. Add it to `crates/conch_remote/src/ssh.rs`:

```rust
/// Open a new shell channel on an existing SSH connection.
/// This mirrors the channel setup in `connect_and_open_shell` but skips
/// the connection/authentication step.
pub async fn open_shell_channel(
    session: &russh::client::Handle<ConchSshHandler>,
    cols: u16,
    rows: u16,
) -> Result<russh::Channel<russh::client::Msg>, String> {
    let channel = session
        .channel_open_session()
        .await
        .map_err(|e| format!("Channel open failed: {e}"))?;

    channel
        .request_pty(false, "xterm-256color", cols as u32, rows as u32, 0, 0, &[])
        .await
        .map_err(|e| format!("PTY request failed: {e}"))?;

    channel
        .request_shell(false)
        .await
        .map_err(|e| format!("Shell request failed: {e}"))?;

    Ok(channel)
}
```

Make sure this function is `pub` and exported from the crate.

- [ ] **Step 9: Add ssh_open_channel command**

```rust
#[tauri::command]
pub(crate) async fn ssh_open_channel(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    remote: tauri::State<'_, Arc<Mutex<RemoteState>>>,
    pane_id: u32,
    connection_id: String,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    let window_label = window.label().to_string();
    let key = session_key(&window_label, pane_id);

    let ssh_handle = {
        let state = remote.lock();
        let conn = state.connections.get(&connection_id)
            .ok_or_else(|| format!("SSH connection '{connection_id}' not found"))?;
        Arc::clone(&conn.ssh_handle)
    };

    let channel = conch_remote::ssh::open_shell_channel(&ssh_handle, cols, rows).await?;

    let (input_tx, input_rx) = mpsc::unbounded_channel();
    let (output_tx, output_rx) = mpsc::unbounded_channel::<Vec<u8>>();
    let _ = input_tx.send(ChannelInput::Resize { cols, rows });

    let (host, user, port) = {
        let state = remote.lock();
        let conn = state.connections.get(&connection_id).unwrap();
        (conn.host.clone(), conn.user.clone(), conn.port)
    };

    let remote_clone = Arc::clone(&*remote);
    {
        let mut state = remote_clone.lock();
        if let Some(conn) = state.connections.get_mut(&connection_id) {
            conn.ref_count += 1;
        }
        state.sessions.insert(key.clone(), SshSession {
            input_tx,
            connection_id: connection_id.clone(),
            host,
            user,
            port,
        });
    }

    let remote_for_loop = Arc::clone(&remote_clone);
    let key_for_loop = key.clone();
    let wl = window_label.clone();
    let conn_id = connection_id.clone();
    let app_handle = app.clone();
    tokio::spawn(async move {
        let exited = conch_remote::ssh::channel_loop(channel, input_rx, output_tx).await;
        let mut state = remote_for_loop.lock();
        state.sessions.remove(&key_for_loop);
        if let Some(conn) = state.connections.get_mut(&conn_id) {
            conn.ref_count -= 1;
            if conn.ref_count == 0 {
                state.connections.remove(&conn_id);
            }
        }
        drop(state);
        if exited {
            let _ = app_handle.emit_to(&wl, "pty-exit", PtyExitEvent { window_label: wl.clone(), pane_id });
        }
    });

    spawn_output_forwarder(&app, &window_label, pane_id, output_rx);
    Ok(())
}
```

- [ ] **Step 10: Register ssh_open_channel in the Tauri invoke_handler**

Add `remote::ssh_open_channel` to the `.invoke_handler(tauri::generate_handler![...])` list in `lib.rs`.

- [ ] **Step 11: Update ssh_disconnect to handle ref-count decrement**

The current `ssh_disconnect` removes the session from `sessions` AND sends `Shutdown`. But the channel loop cleanup also tries to remove the session. If `ssh_disconnect` removes it first, the channel loop's ref-count decrement is skipped, orphaning the connection.

**Fix:** Change `ssh_disconnect` to only signal shutdown and let the channel loop handle both session removal and ref-count decrement:

```rust
#[tauri::command]
pub(crate) fn ssh_disconnect(
    window: tauri::WebviewWindow,
    remote: tauri::State<'_, Arc<Mutex<RemoteState>>>,
    pane_id: u32,
) {
    let key = session_key(window.label(), pane_id);
    let state = remote.lock();
    // Only send shutdown — let the channel loop handle session removal
    // and connection ref-count decrement.
    if let Some(session) = state.sessions.get(&key) {
        let _ = session.input_tx.send(ChannelInput::Shutdown);
    }
}
```

- [ ] **Step 12: Run tests**

Run: `cargo test --workspace`
Expected: All tests PASS

- [ ] **Step 13: Commit**

```bash
git add crates/conch_tauri/src/remote/mod.rs crates/conch_tauri/src/lib.rs crates/conch_remote/src/ssh.rs
git commit -m "Add SSH multi-channel support with SshConnection ref counting"
```

---

### Task 11: Menu Items and Keyboard Shortcuts

**Files:**
- Modify: `crates/conch_tauri/src/lib.rs` (menu building, constants, event handler)
- Modify: `crates/conch_tauri/frontend/index.html` (menu action handler)

- [ ] **Step 1: Add menu constants**

In `lib.rs`, add new constants:

```rust
const MENU_SPLIT_VERTICAL_ID: &str = "shell.split_vertical";
const MENU_SPLIT_HORIZONTAL_ID: &str = "shell.split_horizontal";
const MENU_CLOSE_PANE_ID: &str = "shell.close_pane";
const MENU_ACTION_SPLIT_VERTICAL: &str = "split-vertical";
const MENU_ACTION_SPLIT_HORIZONTAL: &str = "split-horizontal";
const MENU_ACTION_CLOSE_PANE: &str = "close-pane";
```

- [ ] **Step 2: Add Shell submenu to build_app_menu()**

In `build_app_menu`, create a Shell submenu with split/close items between the File and Edit menus:

```rust
let split_v_accel = config_key_to_accelerator(&keyboard.split_vertical);
let split_v = MenuItem::with_id(app, MENU_SPLIT_VERTICAL_ID, "Split Pane Vertically", true, Some(&split_v_accel))?;
let split_h_accel = config_key_to_accelerator(&keyboard.split_horizontal);
let split_h = MenuItem::with_id(app, MENU_SPLIT_HORIZONTAL_ID, "Split Pane Horizontally", true, Some(&split_h_accel))?;
let close_pane_accel = config_key_to_accelerator(&keyboard.close_pane);
let close_pane = MenuItem::with_id(app, MENU_CLOSE_PANE_ID, "Close Pane", true, Some(&close_pane_accel))?;
let shell_menu = Submenu::with_items(app, "Shell", true, &[&split_v, &split_h, &PredefinedMenuItem::separator(app)?, &close_pane])?;
```

Add `&shell_menu` to the `Menu::with_items` calls (both macOS and non-macOS), placed after `&file_menu`.

- [ ] **Step 3: Add menu event handler for split actions**

In the `on_menu_event` handler in `lib.rs`, add cases:

```rust
MENU_SPLIT_VERTICAL_ID => emit_menu_action_to_focused_window(app, MENU_ACTION_SPLIT_VERTICAL),
MENU_SPLIT_HORIZONTAL_ID => emit_menu_action_to_focused_window(app, MENU_ACTION_SPLIT_HORIZONTAL),
MENU_CLOSE_PANE_ID => emit_menu_action_to_focused_window(app, MENU_ACTION_CLOSE_PANE),
```

- [ ] **Step 4: Do the same for build_app_menu_with_plugins()**

Add the same Shell menu to the plugin-augmented menu builder.

- [ ] **Step 5: Handle menu actions in frontend**

In the `menu-action` event listener in `index.html`:

```js
case 'split-vertical': splitPane('vertical'); break;
case 'split-horizontal': splitPane('horizontal'); break;
case 'close-pane': closePane(focusedPaneId); break;
```

- [ ] **Step 6: Expose new shortcuts to frontend**

Add split pane shortcuts to `KeyboardShortcuts` struct and `get_keyboard_shortcuts` command in `lib.rs`:

```rust
struct KeyboardShortcuts {
    // ... existing fields ...
    split_vertical: String,
    split_horizontal: String,
    close_pane: String,
}
```

- [ ] **Step 7: Verify build**

Run: `cargo build -p conch_tauri`
Expected: PASS

- [ ] **Step 8: Commit**

```bash
git add crates/conch_tauri/src/lib.rs crates/conch_tauri/frontend/index.html
git commit -m "Add Shell menu with split/close pane items and keyboard shortcuts"
```

---

### Task 12: Right-Click Terminal Context Menu

**Files:**
- Modify: `crates/conch_tauri/frontend/index.html`

- [ ] **Step 1: Add context menu HTML**

Add a context menu element to the HTML body:

```html
<div id="terminal-context-menu" class="ssh-overlay" style="display:none;">
  <div class="context-menu">
    <button class="context-item" data-action="split-vertical">Split Vertically</button>
    <button class="context-item" data-action="split-horizontal">Split Horizontally</button>
  </div>
</div>
```

- [ ] **Step 2: Add context menu CSS**

Style using the existing `ssh-context-menu` pattern — absolutely positioned, small shadow, dismissible.

- [ ] **Step 3: Add context menu event handler**

On `contextmenu` event on `.terminal-pane`:
1. Check if the terminal has mouse tracking enabled (use xterm's internal modes). If yes, let the tmux bridge handle it; return.
2. Prevent default browser context menu.
3. Position the context menu at cursor coordinates.
4. Show the menu.

On click of a `.context-item`:
1. Read `data-action`.
2. Call `splitPane(direction)`.
3. Hide the menu.

Dismiss on Escape or click outside.

- [ ] **Step 4: Verify build**

Run: `cargo build -p conch_tauri`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/conch_tauri/frontend/index.html
git commit -m "Add right-click terminal context menu for split actions"
```

---

### Task 13: Files Panel and Plugin Integration

**Files:**
- Modify: `crates/conch_tauri/frontend/files-panel.js`
- Modify: `crates/conch_tauri/frontend/plugin-widgets.js`
- Modify: `crates/conch_tauri/frontend/index.html` (callback wiring)

- [ ] **Step 1: Update files-panel.js**

1. Rename `activeRemoteTabId` → `activeRemotePaneId` (line 22, and all references at lines 159, 167, 168, 198, 294, 448, 469)
2. Change the `init()` options to accept `getActivePane` instead of `getActiveTab`:

```js
// In init(opts):
const getActivePane = opts.getActivePane;
```

3. Change the `onTabChanged` callback to `onFocusChanged(pane)`:

```js
function onFocusChanged(pane) {
  if (!pane || pane.type !== 'ssh' || !pane.spawned) {
    activeRemotePaneId = null;
    // ... deactivate remote pane
    return;
  }
  activeRemotePaneId = pane.paneId;
  // ... load SFTP for this pane
}
```

4. Update ALL 6 invoke calls from `tabId` to `paneId`:

```js
// Line 171: sftp_realpath
invoke('sftp_realpath', { paneId: tab.id, path: '.' })

// Line 204: sftp_list_dir
invoke('sftp_list_dir', { paneId: activeRemotePaneId, path: pane.currentPath })

// Line 456: transfer_download
invoke('transfer_download', { paneId: activeRemotePaneId, remotePath: ..., localPath: ... })

// Line 477: transfer_upload
invoke('transfer_upload', { paneId: activeRemotePaneId, localPath: ..., remotePath: ... })
```

Also update the guard checks at lines 448 and 469 from `activeRemoteTabId` to `activeRemotePaneId`.

- [ ] **Step 2: Update plugin-widgets.js writeToActivePty callback**

In `index.html` where `writeToActivePty` is set up:

```js
writeToActivePty: (payload) => {
  const pane = currentPane();
  if (!pane || !pane.spawned) return;
  const cmd = pane.type === 'ssh' ? 'ssh_write' : 'write_to_pty';
  invoke(cmd, { paneId: pane.paneId, data: payload.data || payload });
}
```

- [ ] **Step 3: Update index.html wiring**

Change the `filesPanel.init()` call to pass `getActivePane: () => currentPane()`.

Add a call to `filesPanel.onFocusChanged(pane)` inside `setFocusedPane()` whenever the focused pane changes.

- [ ] **Step 4: Verify build**

Run: `cargo build -p conch_tauri`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/conch_tauri/frontend/files-panel.js crates/conch_tauri/frontend/plugin-widgets.js crates/conch_tauri/frontend/index.html
git commit -m "Migrate files panel and plugin system to pane-based routing"
```

---

### Task 14: Final Integration Testing and Cleanup

**Files:**
- All modified files

- [ ] **Step 1: Run full Rust test suite**

Run: `cargo test --workspace`
Expected: All tests PASS

- [ ] **Step 2: Run cargo clippy**

Run: `cargo clippy --workspace -- -D warnings`
Expected: No warnings

- [ ] **Step 3: Verify all JS files parse cleanly**

Run: `for f in crates/conch_tauri/frontend/*.js; do node --check "$f"; done`
Expected: All pass

- [ ] **Step 4: Manual smoke test checklist**

- [ ] Open app, single tab works
- [ ] `Cmd+D` splits vertically, new shell spawns
- [ ] `Cmd+Shift+D` splits horizontally
- [ ] Click pane to focus, blue border appears
- [ ] `Cmd+Alt+Arrow` navigates between panes
- [ ] Drag divider to resize panes
- [ ] `Cmd+Shift+W` closes focused pane, tree simplifies
- [ ] `Cmd+W` closes entire tab (all panes)
- [ ] Split SSH tab: new channel opens, independent shell
- [ ] Close all SSH panes: connection disconnects
- [ ] Right-click context menu shows split options
- [ ] Files panel follows focused SSH pane
- [ ] Plugin writeToActivePty targets focused pane
- [ ] Multiple tabs with different split layouts, switching preserves layout
- [ ] Window resize: all panes refit correctly
- [ ] Nested splits (3+ levels): works correctly

- [ ] **Step 5: Commit any fixes**

```bash
git add crates/conch_tauri/src/ crates/conch_core/src/ crates/conch_remote/src/ crates/conch_tauri/frontend/
git commit -m "Fix integration issues from split pane testing"
```

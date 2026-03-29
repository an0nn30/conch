# Split Pane Support — Design Spec

## Overview

Add nestable split-pane support to Conch alongside the existing tab system. Any terminal pane can be split horizontally (top/bottom) or vertically (left/right), forming a recursive binary tree within each tab. When splitting an SSH session, the new pane opens a new channel on the existing SSH connection. Local shell splits spawn a fresh local PTY.

## Architecture

**Hybrid approach:** JavaScript owns the split tree and DOM layout (nested flexbox). Rust owns session lifecycle (PTY spawn, SSH channel open, write, resize, close). A unique `paneId` links JS tree leaf nodes to Rust backend sessions.

## Data Model

### Split Tree (JS)

Each tab maps to a split tree root. Two node types:

```js
// Leaf — a terminal pane
{ type: 'leaf', paneId: Number }

// Split — a container with two children
{ type: 'split', direction: 'horizontal' | 'vertical', ratio: Number, children: [node, node] }
```

A global `nextPaneId` counter (incrementing integer, never reused) assigns unique IDs across all tabs within a window.

A `panes` Map keyed by `paneId` holds pane-level data:

```js
panes.get(paneId) → {
  paneId: Number,
  tabId: Number,          // which tab this pane belongs to
  type: 'local' | 'ssh',
  connectionId: String,   // for SSH panes — see SSH section
  term: Terminal,         // xterm.js instance
  fitAddon: FitAddon,
  root: HTMLElement,      // the .terminal-pane div
  spawned: Boolean,
  lastCols: Number,
  lastRows: Number,
  cleanupMouseBridge: Function,
  resizeObserver: ResizeObserver,
  debounceTimer: Number   // per-pane debounce for resize
}
```

### Tab Object Refactoring (JS)

The existing `tabs` Map changes shape. Pane-level fields (`term`, `fitAddon`, `root`, `spawned`) move to the `panes` Map. The tab object becomes:

```js
tabs.get(tabId) → {
  id: Number,
  label: String,
  type: 'local' | 'ssh',   // type of the initial pane (for display)
  hasCustomTitle: Boolean,
  button: HTMLElement,       // tab bar button
  treeRoot: Object,          // split tree root node (leaf or split)
  containerEl: HTMLElement,  // the top-level DOM element for this tab's tree
  focusedPaneId: Number      // last-focused pane in this tab
}
```

Helper functions replace direct tab field access:
- `currentPane()` → returns the focused pane object (replaces `currentTab().term`)
- `getTabForPane(paneId)` → finds which tab a pane belongs to
- `allPanesInTab(tabId)` → walks the tree, returns all leaf pane IDs

### Backend Session Keys (Rust)

The `tab_id: u32` parameter in all Tauri commands is renamed to `pane_id: u32`. This is a semantic change — the backend no longer knows about tabs; it only manages sessions keyed by pane ID. The `TauriState::ptys` HashMap key changes from `window_label:tab_id` to `window_label:pane_id`.

Affected Rust command signatures (all change `tab_id` → `pane_id`):
- PTY: `spawn_shell`, `write_to_pty`, `resize_pty`, `close_pty`
- SSH: `ssh_connect`, `ssh_quick_connect`, `ssh_write`, `ssh_resize`, `ssh_disconnect`
- SFTP: `sftp_list_dir`, `sftp_stat`, `sftp_read_file`, `sftp_write_file`, `sftp_mkdir`, `sftp_rename`, `sftp_remove`, `sftp_realpath`
- New: `ssh_open_channel`

Frontend `invoke()` call sites requiring `tabId` → `paneId` migration:
- `index.html` (~11 sites): `spawn_shell`, `write_to_pty`/`ssh_write`, `resize_pty`/`ssh_resize`, `close_pty`/`ssh_disconnect`, `ssh_connect`/`ssh_quick_connect`, drag-drop handler, plugin write callback
- `files-panel.js` (2 sites): `sftp_realpath`, `sftp_list_dir`

The `session_key` helper changes accordingly:
```rust
fn session_key(window_label: &str, pane_id: u32) -> String {
    format!("{window_label}:{pane_id}")
}
```

### Event Payload Migration

The `PtyOutputEvent` and `PtyExitEvent` structs rename `tab_id` to `pane_id`:

```rust
#[derive(Clone, Serialize)]
struct PtyOutputEvent {
    window_label: String,
    pane_id: u32,   // was: tab_id
    data: String,
}

#[derive(Clone, Serialize)]
struct PtyExitEvent {
    window_label: String,
    pane_id: u32,   // was: tab_id
}
```

The frontend `pty-output` listener changes from `tabs.get(tabId)` to `panes.get(paneId)`:

```js
await listenOnCurrentWindow('pty-output', (event) => {
  const { pane_id, data } = event.payload;
  const pane = panes.get(pane_id);
  if (pane && pane.term) pane.term.write(data);
});
```

The `pty-exit` listener similarly looks up the pane, determines its tab, and handles cleanup (close pane, simplify tree, or close tab if last pane).

All frontend `invoke()` calls (approximately 15 call sites) change from `tabId` to `paneId`.

### Example

```
Tab "Tab 1"
└── SplitNode(vertical, 0.5)
    ├── Leaf(paneId: 1)        ← local shell
    └── SplitNode(horizontal, 0.6)
        ├── Leaf(paneId: 2)    ← local shell
        └── Leaf(paneId: 3)    ← local shell
```

## DOM Layout

Each tree node maps to a DOM element inside `#terminal-host`:

- **Leaf** → `div.terminal-pane` with `data-pane-id` attribute (contains xterm.js)
- **Split** → `div.split-container` with `flex-direction: row` (vertical split) or `column` (horizontal split)
- **Divider** → `div.split-divider` between the two children, 4px wide/tall

```html
<div class="split-container" style="flex-direction: row;">
  <div class="terminal-pane" style="flex: 0.5;" data-pane-id="1">
    <!-- xterm.js -->
  </div>
  <div class="split-divider vertical"></div>
  <div class="split-container" style="flex-direction: column; flex: 0.5;">
    <div class="terminal-pane" style="flex: 0.6;" data-pane-id="2">
      <!-- xterm.js -->
    </div>
    <div class="split-divider horizontal"></div>
    <div class="terminal-pane" style="flex: 0.4;" data-pane-id="3">
      <!-- xterm.js -->
    </div>
  </div>
</div>
```

### CSS Migration

The existing `.terminal-pane` CSS uses `position: absolute; inset: 0` with an `active` class toggle to show/hide panes per tab. This is incompatible with flexbox split layout. The CSS changes to:

```css
/* Each tab's tree root — only active tab is visible */
.tab-tree-root {
  position: absolute;
  inset: 0;
  display: none;
}
.tab-tree-root.active {
  display: flex;
}

/* Split containers — nested flexbox */
.split-container {
  display: flex;
  flex: 1;
  min-width: 0;
  min-height: 0;
  overflow: hidden;
}

/* Terminal panes — flex children, no absolute positioning */
.terminal-pane {
  position: relative;
  min-width: 80px;
  min-height: 80px;
  overflow: hidden;
}

/* Dividers */
.split-divider {
  flex-shrink: 0;
  background: var(--border);
}
.split-divider.vertical {
  width: 4px;
  cursor: col-resize;
}
.split-divider.horizontal {
  height: 4px;
  cursor: row-resize;
}

/* Focused pane indicator */
.terminal-pane.focused {
  border-top: 2px solid var(--blue);
}
```

**Tab visibility:** The `active` class toggle moves from individual `.terminal-pane` elements to a new `.tab-tree-root` wrapper. Each tab has one `.tab-tree-root` inside `#terminal-host`. Only the active tab's root is displayed.

**Single-pane tabs:** A tab with no splits has a tree root that is a single leaf node. The `.tab-tree-root` wraps a lone `.terminal-pane` directly — no split-container needed. When a split occurs, the DOM restructures to insert the split-container.

### ResizeObserver Architecture

The existing single `ResizeObserver` on `#terminal-host` is removed. Each pane gets its own `ResizeObserver` instance that watches the pane's `.terminal-pane` element. Each observer has its own debounce timer (stored in the pane object as `debounceTimer`) to coalesce rapid resize events independently.

When a divider is dragged, both adjacent panes' observers fire independently. Each pane debounces and sends its own `resize_pty` / `ssh_resize` command — no coordination needed between them.

Minimum pane size: 80px in either dimension, enforced during divider drag by clamping the ratio.

### Divider Interaction

- Dragging updates the `flex` values of the two sibling panes and the `ratio` in the tree node.
- The ratio is clamped such that neither child falls below 80px, calculated from the parent container's current pixel dimensions.
- No snap-to-equal or collapse-to-zero behavior — the divider simply clamps at the minimum. These could be added later.
- Cursor during drag: `col-resize` for vertical dividers, `row-resize` for horizontal.

## Splitting Flow

When a split is triggered:

1. Identify the active (focused) pane — the current leaf node.
2. Create a new split node replacing the leaf in its parent:
   - Original leaf becomes `children[0]`.
   - New leaf (fresh `paneId` from `nextPaneId++`) becomes `children[1]`.
   - Direction from user's choice. Initial ratio: `0.5`.
3. Rebuild the DOM subtree — wrap in `div.split-container`, insert divider, append both pane elements.
4. Create pane entry in the `panes` Map, initialize xterm.js + FitAddon + ResizeObserver.
5. Spawn the new session based on the originating pane's type:
   - **Local shell:** `spawn_shell(paneId, cols, rows)` — same as new tab creation.
   - **SSH:** `ssh_open_channel(paneId, connectionId, cols, rows)` — opens a new channel on the existing SSH connection, spawns a shell, wires up `pty-output`/`pty-exit` events scoped to the new `paneId`.
6. Wire up the new pane's `term.onData()` callback to call `invoke('write_to_pty', { paneId })` or `invoke('ssh_write', { paneId })`.
7. Focus the new pane.

### SSH Multi-Channel Support

A new Tauri command `ssh_open_channel` opens an additional channel on an already-established SSH connection.

**`connectionId`:** A string of the form `conn:window_label:pane_id` where `pane_id` is the original pane that established the SSH connection. The `conn:` prefix distinguishes connection keys from session keys (which use `window_label:pane_id` without prefix). This ID is stable for the lifetime of the connection.

**Rust data model changes:**

A new `SshConnection` struct separates the connection from individual channels:

```rust
struct SshConnection {
    ssh_handle: Arc<russh::client::Handle<ConchSshHandler>>,
    host: String,
    user: String,
    port: u16,
    ref_count: u32,   // number of active channels (Mutex-protected, no atomic needed)
}
```

`RemoteState` gains a `connections: Mutex<HashMap<String, SshConnection>>` map alongside the existing `sessions` map. Each `SshSession` gains a `connection_id: String` field and drops the `ssh_handle` (looked up via the connection).

When `ssh_connect` or `ssh_quick_connect` is called (original connection), it:
1. Creates an `SshConnection` entry with `ref_count: 1` and the `Arc`-wrapped `ssh_handle` (the `Arc` wrapping moves from per-session to per-connection).
2. Creates an `SshSession` entry with the `connection_id` and its own `input_tx`, but no `ssh_handle` (looked up through the connection).

When `ssh_open_channel` is called, it:
1. Looks up the `SshConnection` by `connectionId`.
2. Calls `ssh_handle.channel_open_session()` to open a new channel.
3. Increments `ref_count`.
4. Creates a new `SshSession` with the same `connection_id`, its own `input_tx`, and its own output forwarder task.

**SFTP routing:** The SFTP file browser panel uses the SSH connection of the **focused** SSH pane. The `files-panel.js` `onTabChanged` callback becomes `onFocusChanged(pane)` — it receives the focused pane object and checks `pane.type` and `pane.paneId` instead of tab-level fields. The stored `activeRemoteTabId` becomes `activeRemotePaneId`.

When focus changes:
- Focus moves to an SSH pane: SFTP panel activates, uses that pane's `connectionId`. Directory state is retained per-connection (if switching between panes on the same connection, the working directory is preserved).
- Focus moves to a local pane: SFTP panel deactivates (same as current behavior when switching to a local tab).
- Focus moves to an SSH pane on a different connection: SFTP panel switches to that connection, resets working directory to home.

The `get_ssh_handle()` helper is updated to accept a `pane_id`, look up the session's `connection_id`, and retrieve the `ssh_handle` from the connections map.

**Channel cleanup atomicity:** Channel close decrements the connection's `ref_count` atomically. When it reaches zero, the connection is removed from the `connections` map and disconnected. `Mutex` around the `connections` map ensures safe concurrent access if two channels close simultaneously.

## Focus Management

### Focus Tracking

- A global `focusedPaneId` variable tracks the active pane across the window.
- Each tab stores `focusedPaneId` — the last-focused pane in that tab.
- Clicking a pane's terminal area sets it as focused.
- Focused pane has a 2px accent border on the top edge (`var(--blue)`) via the `.focused` CSS class.
- Switching tabs restores focus to the tab's stored `focusedPaneId`.

### Keyboard Navigation

- `Cmd+Alt+Arrow` moves focus to the adjacent pane in that direction.
- Navigation is spatial — compares pane bounding rects to find the nearest neighbor in the arrow direction.
- No wrapping — if no pane exists in that direction, the shortcut is a no-op.

### Input Routing

- Only the focused pane receives keyboard input via its `term.onData()` callback.
- `write_to_pty` / `ssh_write` route via the focused pane's `paneId`.
- All panes independently resize via their own ResizeObserver.

### Plugin System Integration

The `writeToActivePty` callback used by the plugin widget system currently calls `currentTab()` to determine the write target. This changes to use `currentPane()` (the focused pane), routing plugin writes to the correct backend session. Similarly, any plugin API that references the "active terminal" must be updated to use pane-level resolution.

## Pane Close and Tree Simplification

When a pane is closed:

1. **Terminate session:** `close_pty(paneId)` or close the SSH channel for that pane.
2. **Dispose resources:** Call `term.dispose()` on the xterm.js instance (frees WebGL context if active), disconnect ResizeObserver, run `cleanupMouseBridge()`.
3. **Remove the leaf** from the tree and remove entry from `panes` Map.
4. **Simplify the tree:** The leaf's sibling replaces the parent split node, inheriting the parent's position and flex value. Divider DOM element is removed. If the sibling is a split node, its subtree is preserved.
5. **Reclaim space:** The remaining sibling expands to fill 100% of the former split area.
6. **Refocus:** If the closed pane was focused, focus moves to the sibling (or sibling's first leaf if it's a split node).
7. **Last pane in tab:** Closing the only remaining pane closes the entire tab.

### Tab Close with Splits

`Cmd+W` closes the entire tab, regardless of how many panes it contains. All panes in the tab are closed by walking the tree and terminating each session. This is distinct from `Cmd+Shift+W` which closes only the focused pane.

### SSH Connection Cleanup

- A reference count tracks panes per SSH connection via `SshConnection.ref_count`.
- Closing a pane decrements the count and closes only that channel.
- When the count reaches zero, the SSH connection itself is disconnected and cleaned up from `RemoteState`.
- If two channels close simultaneously, the `Mutex` on the connections map ensures only one thread performs the final disconnect.

## User Interaction

### Keyboard Shortcuts

Configurable via `[conch.keyboard]` in `config.toml`. New fields added to the `KeyboardConfig` struct in `crates/conch_core/src/config/conch.rs`:

```rust
pub struct KeyboardConfig {
    // ... existing fields ...
    pub split_vertical: String,      // default: "CmdOrCtrl+D"
    pub split_horizontal: String,    // default: "CmdOrCtrl+Shift+D"
    pub close_pane: String,          // default: "CmdOrCtrl+Shift+W"
    pub navigate_pane_up: String,    // default: "CmdOrCtrl+Alt+Up"
    pub navigate_pane_down: String,  // default: "CmdOrCtrl+Alt+Down"
    pub navigate_pane_left: String,  // default: "CmdOrCtrl+Alt+Left"
    pub navigate_pane_right: String, // default: "CmdOrCtrl+Alt+Right"
}
```

| Action | Default Shortcut |
|---|---|
| Split pane vertically (left/right) | `Cmd+D` |
| Split pane horizontally (top/bottom) | `Cmd+Shift+D` |
| Close pane | `Cmd+Shift+W` |
| Navigate to adjacent pane | `Cmd+Alt+Arrow` |

**`Cmd+W` behavior:** With multi-pane tabs, `Cmd+W` still closes the entire tab (all panes). `Cmd+Shift+W` closes only the focused pane. This mirrors iTerm2 behavior where Cmd+W closes the tab and Cmd+Shift+W closes the session/pane.

These shortcuts are registered as Tauri menu accelerators. The frontend keydown handler intercepts `Cmd+Alt+Arrow` for pane navigation (not suitable as a menu accelerator since direction matters).

### Menu Bar Items

Under the Shell menu:
- Split Pane Vertically (`Cmd+D`)
- Split Pane Horizontally (`Cmd+Shift+D`)
- Close Pane (`Cmd+Shift+W`)

### Right-Click Context Menu

A new terminal context menu is created (one does not exist today). It contains "Split Vertically" and "Split Horizontally" items.

- The context menu is triggered on `contextmenu` event on `.terminal-pane` elements.
- It only appears when the terminal does NOT have mouse tracking enabled (the existing tmux right-click bridge takes precedence when mouse mode is active).
- Implementation follows the same pattern as the existing `ssh-context-menu`: an absolutely positioned div with click handlers, dismissed on blur or Escape.

## State Persistence

**Not persisted across app restarts.** Each launch starts fresh with a single local shell tab. Split trees live in JS memory for the duration of the session — switching tabs preserves layout, but closing the app discards it.

No tree serialization, no session restore logic.

## Testing Strategy

### Rust Unit Tests

- Session key generation with pane IDs (`window:paneId` format).
- SSH multi-channel management: open/close channels, reference counting, cleanup on last channel close.
- Concurrent channel close safety (ref count atomicity).
- Existing PTY commands work unchanged with pane ID keys.
- `KeyboardConfig` new field defaults and serde round-trips.

### JS Pure Function Tests

Split tree operations extracted as pure functions (no DOM dependency):
- Splitting a leaf produces correct tree structure.
- Removing a leaf simplifies the parent correctly.
- Adjacent-pane spatial lookup returns correct neighbors.
- Ratio clamping respects minimum pane size constraints.
- `allPanesInTab` tree traversal returns all leaf pane IDs.

### Manual Test Scenarios

- Split local shell — new shell spawns, both accept input independently.
- Split SSH session — new channel opens on same connection, independent shells.
- Nested splits (3+ levels deep) — layout renders correctly, resize works.
- Close middle pane — tree simplifies, sibling expands.
- Close all panes in SSH connection — connection disconnects.
- Close tab with multiple panes (`Cmd+W`) — all sessions terminated.
- Divider drag — both adjacent panes resize, xterm fits correctly.
- Window resize with active splits — all panes refit proportionally.
- Tab switching with splits — layout preserved, focus restored.
- Plugin `writeToActivePty` — writes to focused pane, not a stale tab reference.
- WebGL context cleanup — open and close many panes, verify no context leak.
- Focus race — two SSH channels closing simultaneously, focus resolves cleanly.
- SFTP panel — reflects the focused SSH pane's connection.

## Follow-Up (Out of Scope)

- **SSH split behavior setting:** Add a preference in Settings to choose between reusing the existing SSH connection (new channel) or opening a new independent connection when splitting an SSH pane.

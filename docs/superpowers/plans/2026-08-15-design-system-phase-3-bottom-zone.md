# Design System Phase 3: Bottom Zone & SFTP — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give the window a real bottom tool-window zone with a horizontal strip, retire the legacy `#bottom-panel`, and move Files into it as a side-by-side SFTP dual pane matching the reference.

**Architecture:** The reference app has no bespoke bottom bar — its bottom area is a tool-window zone whose strip lists SFTP / Proxmox / SysInfo / Script Output. We finish the half-built `'bottom'` zone in the manager, migrate plugin bottom panels onto it, retire `#bottom-panel`, and restructure the Files panel.

**Tech Stack:** Vanilla IIFE JS + CSS tokens; no Rust changes (persistence keys are reused as-is).

## Global Constraints

- Spec: `docs/superpowers/specs/2026-08-14-termlab-design-system-design.md`. **Measured reference values: `docs/superpowers/specs/assets/reference/METRICS.md` — cite it rather than guessing; reference capture sits beside it.**
- Measurement loop (use it to verify, not eyeball): `scripts/ui-probe/README.md`. Build the three Swift helpers, `screencapture -x -o -l <windowId>`, then `scan`/`sample`. Capture drifts ±1 per channel.
- Tokens only in CSS (`var(--tl-*)`); no raw hex, no legacy `--blue`-era vars in edited rules. Legacy chrome vars are already token aliases — don't reintroduce hardcoded colors.
- Vanilla IIFE modules, `<script>` tags in `index.html` (and `settings.html` where applicable), no bundler.
- Commands/events via the injected `invoke` / `window.termlabTauriClient`; keyboard via `window.termlabKeyboardRouter`; layout persistence through the existing layout-service flow.
- **Do not change persisted key names.** `bottom_panel_visible` / `bottom_panel_height` (Rust: `crates/termlab_core/src/config/persistent.rs:37,43`; `crates/termlab_tauri/src/commands.rs:173-174,197-198`) are reused to mean the bottom *zone*. Tool-window ids stay stable: Files keeps id `file-explorer` (only its title/zone change).
- `rg` is not installed — verify with `grep`. `node --check` every touched JS file. Full suite: `cargo test --workspace` (expect 500 passing) plus `node scripts/tests/test_tl_icon.mjs` and `python3 scripts/tests/test_extract_tokens.py`.
- Verified anchors: `tool-window-manager.js` — `ZONE_IDS` L8, `zones` L13, `sidebars` L18, `panelState` L22, `strips` L27, `DRAGGABLE_ZONES` L28, `ZONE_LABELS` L29, `init` L67 (zone query L72), `register` L127, `ensureWindowElement` L213 (bails L216), `activate` L230, `moveTo` L289 (bails L292), `updateZone` L341 (bails L343), `updateSidebar` L420 (bottom guard L421), `getStripDropZoneRects` L515, `updateStrips` L730 (`['left','right']` L731), `makeStripBtn` L754, `showContextMenu` L792 (targets L804-809), `initZoneDivider` L910, `sideForZone` L971, `isPanelVisible` L977, `setPanelVisibility` L992, `togglePanel` L1012, `getSplitRatios` L1054 (`['left','right']` L1056), `listWindows` L1096. `tool-window-runtime.js` — `saveLayoutNow` L44 (bottom keys L65-66), restore L96-128, Files registration L149, bottom-panel resize handlers L253-296, plugin registration L411-439 (bottom early-return L416, zoneMap L418). `plugin-widgets.js` — bottom routing L54, `ensureBottomPanelTab` L52. `notification-panel.js` — `init` L18, `addTab` L29, `removeTab` L38, `activateTab` L53, `renderInto` L68, `addPluginTab` L117, `hasTabs` L159. `startup-runtime.js` — bottom restore L125-161. `menu-actions.js` — toggle L231-239. `files-panel.js` — shell innerHTML L105-114, dead resize L188-222, dead restore L238, dead save L225. `pane-view.js` — `renderPane` L4, label text L10-12, toolbar L31-38, table L40-50. `panels.css` — `.fp-pane-container` L5, `.fp-pane` L6, `.fp-transfer-bar` L52-56, bottom panel L170-239. `strips.css` — `.side-strip` width L1, `.strip-btn` L5, active L22.

---

### Task 1: Bottom zone infrastructure and strip parity

**Files:**
- Modify: `crates/termlab_tauri/frontend/index.html` (bottom zone markup + strip)
- Modify: `crates/termlab_tauri/frontend/app/layout/tool-window-manager.js`
- Modify: `crates/termlab_tauri/frontend/styles/design-system/components/strips.css`
- Modify: `crates/termlab_tauri/frontend/styles/tool-windows.css`

**Interfaces:**
- Produces: a working `'bottom'` zone — `register(id, {defaultZone: 'bottom'})` renders content, gets a strip tab, is toggleable, and persists. Tasks 2-3 depend on this.
- Produces: `#bottom-strip` containing `button.strip-btn.strip-btn--horizontal` entries; `.side-strip--horizontal` modifier for the strip container.

- [ ] **Step 1: Add the markup**

In `index.html`, immediately after the `#main-area` div closes and **before** the existing `#bottom-panel` block (which Task 2 removes), insert:

```html
    <div id="bottom-zone-wrap" class="bottom-zone-wrap hidden">
      <div id="bottom-zone-resize" class="bottom-zone-resize"></div>
      <div class="tool-zone" data-zone="bottom">
        <div class="zone-tab-strip hidden"></div>
        <div class="zone-content"></div>
      </div>
    </div>
    <div id="bottom-strip" class="side-strip side-strip--horizontal"></div>
```

`#bottom-strip` must be the last child of `#app` so it sits along the window's bottom edge.

- [ ] **Step 2: Teach the manager about the bottom side**

In `tool-window-manager.js`:

- L22 `panelState`: add `bottom: { visible: false }`.
- L27 `strips`: add `bottom: null`.
- L28 `DRAGGABLE_ZONES`: add `'bottom'`.
- L29 `ZONE_LABELS`: add `bottom: 'Bottom'`.
- In `init` (L67), after the left/right strip lookups (L90-91), add `strips.bottom = document.getElementById('bottom-strip');` and grab `const bottomWrapEl = document.getElementById('bottom-zone-wrap');` into a module-level `let bottomZoneWrapEl`.
- `updateStrips` (L730): keep the `['left','right']` loop for the vertical strips, then append a bottom pass that fills `strips.bottom` with one `makeStripBtn(wid, zones.bottom)` per `zones.bottom.windows`, adding the class `strip-btn--horizontal` to each button (pass a flag or add the class after creation — read `makeStripBtn` L754 and extend it with an optional `horizontal` argument rather than duplicating it).
- Add `function updateBottomZone()`: hides/shows `bottomZoneWrapEl` via the `hidden` class based on `panelState.bottom.visible && zones.bottom.activeId`, and calls `fitActiveTabFn()` so the terminal re-fits. Call it from `activate`, `deactivate`, `toggle`, and `moveTo` wherever `updateSidebar` is called for left/right (`updateSidebar` L420 keeps its `side === 'bottom'` early return — bottom is not a two-zone split).
- `setPanelVisibility` (L992) / `togglePanel` (L1012) / `isPanelVisible` (L977): these already key off `panelState[side]`, which now has a `bottom` entry — verify each works and calls `updateBottomZone()` for the bottom side.
- `showContextMenu` targets (L804-809): add a `'bottom'` entry so any tool window can be moved there.
- Leave `getSplitRatios` (L1054) and `initZoneDivider` (L910) left/right-only — the bottom zone holds no vertical split. Confirm no code path passes `'bottom'` into `initZoneDivider`/`setSplitRatio` (grep their call sites) so the latent `side + '-top'` TypeErrors stay unreachable.

- [ ] **Step 3: Strip CSS — horizontal variant plus measured parity fixes**

Rewrite `strips.css` to:

```css
.side-strip { background: var(--tl-panel-bg); width: 22px; }
.side-strip--horizontal { width: auto; height: 22px; }

.strip-btn {
  writing-mode: vertical-rl;
  text-orientation: mixed;
  display: flex; align-items: center;
  gap: var(--tl-space-1);
  padding: var(--tl-space-2) 0;
  font: 400 11px var(--tl-font-ui);
  color: var(--tl-fg-muted);
  background: transparent; border: none;
  cursor: pointer; user-select: none; white-space: nowrap;
}
.strip-btn:hover { color: var(--tl-fg); background: var(--tl-row-hover); }
/* Side-strip buttons show no fill when their tool window is open — measured
   off the reference; only bottom tabs get a selected fill. */
.strip-btn.active { color: var(--tl-fg); }

.strip-btn--horizontal {
  writing-mode: horizontal-tb;
  padding: 0 var(--tl-space-2);
  height: 22px;
}
.strip-btn--horizontal.active {
  background: var(--tl-ToolWindow-Button-selectedBackground);
  color: var(--tl-fg);
}

#left-strip .strip-btn { transform: rotate(180deg); }
.strip-btn .tl-icon { transform: rotate(90deg); }
#left-strip .strip-btn .tl-icon { transform: rotate(-90deg); }
.strip-btn--horizontal .tl-icon { transform: none; }
```

In `tool-windows.css`, add rules for the new wrap: `.bottom-zone-wrap { display: flex; flex-direction: column; flex-shrink: 0; height: 312px; min-height: 80px; max-height: 60vh; border-top: 1px solid var(--tl-border); }`, `.bottom-zone-wrap.hidden { display: none; }`, `.bottom-zone-resize { height: 4px; cursor: row-resize; flex-shrink: 0; }`, `#bottom-strip { border-top: 1px solid var(--tl-border); flex-direction: row; }` (the existing `.side-strip` rule at L111-115 sets `flex-direction: column`, so the horizontal modifier must override it), and `.side-strip--horizontal.hidden { height: 0; width: auto; }`.

- [ ] **Step 4: Verify**

```bash
node --check crates/termlab_tauri/frontend/app/layout/tool-window-manager.js
grep -n "#[0-9a-fA-F]\{3,8\}" crates/termlab_tauri/frontend/styles/design-system/components/strips.css   # expect empty
cargo build -p termlab_tauri && (RUST_LOG=info ./target/debug/termlab > /tmp/tl-p3-t1.log 2>&1 &) && sleep 8 && pkill -f target/debug/termlab; grep -ci "error\|panic" /tmp/tl-p3-t1.log   # expect 0
```

The bottom zone stays empty until Task 2/3 register windows there; the strips must still render left/right correctly and the app must look unchanged apart from 22px strips.

- [ ] **Step 5: Commit**

```bash
git add -A crates/termlab_tauri/frontend
git commit -m "feat(design-system): bottom tool-window zone with horizontal strip"
```

---

### Task 2: Retire #bottom-panel, migrate plugin bottom panels

**Files:**
- Modify: `crates/termlab_tauri/frontend/app/tool-window-runtime.js` (plugin routing, resize handler, restore/save)
- Modify: `crates/termlab_tauri/frontend/app/panels/plugin-widgets.js` (L52-66, L88, L99)
- Modify: `crates/termlab_tauri/frontend/app/ui/notification-panel.js` (drop tab machinery)
- Modify: `crates/termlab_tauri/frontend/app/startup-runtime.js` (L125-161), `app/menu-actions.js` (L231-239)
- Modify: `crates/termlab_tauri/frontend/index.html` (remove `#bottom-panel`), `styles/panels.css` (remove L170-239 bottom-panel rules)

**Interfaces:**
- Consumes: Task 1's bottom zone.
- Produces: plugins declaring `location: 'bottom'` register as tool windows `plugin:<name>` in zone `'bottom'`; `window.notificationPanel` keeps only `init`, `renderInto`, and the toast hook; `bottom_panel_visible`/`bottom_panel_height` now drive the bottom zone.

- [ ] **Step 1: Route bottom plugins to the zone**

`tool-window-runtime.js` L411-419: delete the `if (location === 'bottom') return;` early return (L416) and extend the map to `const zoneMap = { left: 'left-top', right: 'right-top', bottom: 'bottom' };`. In `plugin-widgets.js`, delete `ensureBottomPanelTab` (L52-66) and its call sites (L88 removePluginTab, L99 updatePluginTab) — the tool-window path already renders plugin widgets via `registerPluginToolWindow`'s `renderFn` (L427-439); confirm by reading both paths that nothing else is lost (report what you removed).

- [ ] **Step 2: Slim notification-panel.js**

Delete `addTab` (L29), `removeTab` (L38), `activateTab` (L53), `addPluginTab` (L117), `removePluginTab` (L144), `updatePluginTab` (L148), `hasTabs` (L159), and the `tabsEl`/`contentEl`/`activeTabId`/`pluginTabs` state (L13-16). Keep `init` (L18) — now only wiring `toast.onNotification` → `notificationsPanel.refresh()` — and `renderInto` (L68). Grep for every removed name across the frontend and fix callers (expect `startup-runtime.js`'s `hasTabs` check to go away with Step 3).

- [ ] **Step 3: Move visibility/height/resize to the bottom zone**

- `startup-runtime.js` L125-161: the restore block now applies `bottom_panel_visible` to the bottom **zone** — call `toolWindowManager.setPanelVisibility('bottom', visible)` (guarding for the manager not being ready yet; read how the surrounding code sequences manager init) and set `#bottom-zone-wrap`'s height from `bottom_panel_height` when `> 0`. Drop the `hasTabs()` gate: an empty zone is now impossible to show because visibility requires an active window.
- `tool-window-runtime.js` L253-296: repoint the resize handlers from `#bottom-panel-resize`/`#bottom-panel` to `#bottom-zone-resize`/`#bottom-zone-wrap`. Keep the inverted delta (drag up = taller) and widen the clamp to `Math.max(80, Math.min(window.innerHeight * 0.6, startHeight + delta))`. Keep the `saveLayoutNow()` calls on pointerup/cancel.
- `tool-window-runtime.js` `saveLayoutNow` L57-66: compute `bottom_panel_visible` from `toolWindowManager.isPanelVisible('bottom')` and `bottom_panel_height` from `#bottom-zone-wrap`'s `offsetHeight`.
- `menu-actions.js` L231-239: the `toggle-bottom-panel` action calls `toolWindowManager.togglePanel('bottom')` instead of toggling the `hidden` class.

- [ ] **Step 4: Delete the old markup and CSS**

Remove the `#bottom-panel` block from `index.html` (L62-69) and the bottom-panel rules from `panels.css` (L170-239) — **except** the `.notif-*` rules (L223-238), which the notifications tool window still uses. Leave `styles/skins.css`'s now-dead `#bottom-panel` selectors alone (Phase 6 cleanup) and note them in your report.

- [ ] **Step 5: Verify**

```bash
node --check $(git diff --name-only -- 'crates/termlab_tauri/frontend/**/*.js' | tr '\n' ' ')
grep -rn "bottom-panel" crates/termlab_tauri/frontend/app crates/termlab_tauri/frontend/index.html   # expect no live references (skins.css may retain dead selectors)
grep -rn "addPluginTab\|hasTabs\|removePluginTab\|updatePluginTab" crates/termlab_tauri/frontend/app  # expect empty
cargo build -p termlab_tauri && (RUST_LOG=info ./target/debug/termlab > /tmp/tl-p3-t2.log 2>&1 &) && sleep 8 && pkill -f target/debug/termlab; grep -ci "error\|panic" /tmp/tl-p3-t2.log
```

- [ ] **Step 6: Commit**

```bash
git add -A crates/termlab_tauri/frontend
git commit -m "feat(design-system): retire #bottom-panel, plugin bottom panels become tool windows"
```

---

### Task 3: Files becomes SFTP — bottom zone, side-by-side panes

**Files:**
- Modify: `crates/termlab_tauri/frontend/app/tool-window-runtime.js` (Files registration L149-170)
- Modify: `crates/termlab_tauri/frontend/app/panels/files-panel.js` (shell L105-114; delete dead code L188-222, L225-256, L170-186 fallbacks)
- Modify: `crates/termlab_tauri/frontend/app/features/files/pane-view.js` (labels L10-12)
- Modify: `crates/termlab_tauri/frontend/styles/panels.css` (`.fp-*` rules L1-105)

**Interfaces:**
- Consumes: Tasks 1-2.
- Produces: tool window id `file-explorer` (unchanged) titled `SFTP`, `defaultZone: 'bottom'`, laid out local-left / remote-right.

- [ ] **Step 1: Re-register**

`tool-window-runtime.js` L149: `register('file-explorer', { title: 'SFTP', icon: <icon>, type: 'built-in', defaultZone: 'bottom', renderFn })`. For `<icon>`: look for a transfer/remote-folder glyph in `/Users/dustin/projects/intellij-community/platform/icons/src` (candidates: `nodes/ftp.svg`, `general/remote.svg`, `actions/download.svg`); vendor it into `frontend/vendor/intellij-icons/` as `sftp.svg` (+ `_dark` if present), add it to `MANIFEST.md` and to `tl-icon.js`'s `darkVariants` set only if a dark file exists. If nothing suitable exists, pass `icon: 'folder'` and say so in your report.

- [ ] **Step 2: Flip the layout to side-by-side, local first**

`files-panel.js` L105-114 — reorder so local precedes remote (the reference puts local on the left):

```js
    panelEl.innerHTML = `
      <div class="fp-pane-container">
        <div class="fp-pane" id="fp-local"></div>
        <div class="fp-transfer-bar">
          <button class="fp-transfer-btn" id="fp-upload" title="Upload"></button>
          <button class="fp-transfer-btn" id="fp-download" title="Download"></button>
        </div>
        <div class="fp-pane" id="fp-remote"></div>
      </div>`;
```

Keep the existing SVG arrow markup for each button exactly as it is today (copy it across, including the `style="vertical-align:-2px"` attributes), and keep the existing click wiring at L116-117 pointing at the same ids — only the order changed. Note upload now sits above download, matching a vertical bar that points right/left toward the panes.

`panels.css` L5: `.fp-pane-container { flex: 1; display: flex; flex-direction: row; overflow: hidden; }`. L52-56 `.fp-transfer-bar`: `flex-direction: column; justify-content: center; flex-shrink: 0; border-left: 1px solid var(--tl-border); border-right: 1px solid var(--tl-border); border-top: none; border-bottom: none; gap: var(--tl-space-2); padding: 0 var(--tl-space-1);`. `.fp-pane` (L6) keeps `flex: 1` but swap `min-height: 0` for `min-width: 0` (add both — harmless and prevents overflow in either axis).

- [ ] **Step 3: Apply the measured metrics**

Per `METRICS.md`: `.fp-path-input` (panels.css L21-26) height `22px`; `.fp-table thead th` row height `24px`; `.fp-table` body background `var(--tl-Table-background)`; `.fp-row` height `var(--tl-row-h)`. Every edited declaration uses tokens. Do not touch `.fp-*` rules the metrics don't cover.

- [ ] **Step 4: Delete the dead code the tool-window manager superseded**

In `files-panel.js`: `initResize` and its handlers (L188-222), `saveLayoutState` (L225), `restoreLayout` (L238) and their call sites, plus the legacy fallback bodies after the `toolWindowManager` delegation lines in `isHidden`/`showPanel`/`hidePanel`/`togglePanel` (L170-186). Verify each is unreachable before removing (the manager is always present — `tool-window-runtime.js` registers this panel), and list what you deleted in your report. Keep the two cwd-polling intervals; note them as a follow-up rather than changing behavior here.

- [ ] **Step 5: Verify**

```bash
node --check crates/termlab_tauri/frontend/app/panels/files-panel.js crates/termlab_tauri/frontend/app/features/files/pane-view.js
cargo build -p termlab_tauri && (RUST_LOG=info ./target/debug/termlab > /tmp/tl-p3-t3.log 2>&1 &) && sleep 8 && pkill -f target/debug/termlab; grep -ci "error\|panic" /tmp/tl-p3-t3.log
```

- [ ] **Step 6: Commit**

```bash
git add -A crates/termlab_tauri/frontend
git commit -m "feat(design-system): SFTP dual-pane in the bottom zone"
```

---

### Task 4: Measured verification pass

**Files:**
- Modify: whatever the measurements show is off
- Create: `docs/superpowers/specs/assets/phase3-bottom-zone.png`

**Interfaces:** none new.

- [ ] **Step 1: Build the probe tools and capture**

```bash
cd scripts/ui-probe && swiftc -O winlist.swift -o /tmp/winlist && swiftc -O sample.swift -o /tmp/sample && swiftc -O scan.swift -o /tmp/scan && cd -
cargo build -p termlab_tauri && (RUST_LOG=info ./target/debug/termlab > /tmp/tl-p3-final.log 2>&1 &) && sleep 9
WID=$(/tmp/winlist | awk -F'\t' '$3=="termlab"{print $1; exit}')
screencapture -x -o -l $WID /tmp/p3.png
```

- [ ] **Step 2: Measure against the reference**

Scan a column through the SFTP area and the bottom strip, and compare every value to `METRICS.md`:

```bash
/tmp/scan /tmp/p3.png col 1200 <yStart> <yEnd>    # header 27+2, path field 22, col headers 24, table bg #292C33
/tmp/scan /tmp/p3.png row <yInStrip> 0 1200       # bottom strip: 22 tall, active tab filled #3E424A
/tmp/scan /tmp/p3.png row <yInPanes> 0 2600       # pane split ~50/50, divider 1px
```

Fix each deviation greater than 1px or 1 channel, rebuild, and re-measure until they match. Record the final before/after numbers in your report.

- [ ] **Step 3: Save the capture and run regressions**

```bash
cp /tmp/p3.png docs/superpowers/specs/assets/phase3-bottom-zone.png
cargo test --workspace 2>&1 | grep -cE "^test result: ok"   # expect 13
node scripts/tests/test_tl_icon.mjs && python3 scripts/tests/test_extract_tokens.py
grep -rn "#[0-9a-fA-F]\{3,8\}" crates/termlab_tauri/frontend/styles/design-system/components/*.css   # expect empty
```

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "feat(design-system): phase 3 verification pass with measured capture"
```

---

## Phase exit criteria

- Bottom zone hosts SFTP with a horizontal strip; plugin bottom panels register there; `#bottom-panel` is gone.
- SFTP renders local-left / remote-right with measured metrics matching `METRICS.md`.
- Strips are 22px; side-strip buttons have no active fill; bottom tabs do.
- All tests green; capture checked in. Human side-by-side is the final acceptance.

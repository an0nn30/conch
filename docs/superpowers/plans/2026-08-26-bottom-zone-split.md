# Bottom Zone Split Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Split the single `bottom` tool-window zone into an IntelliJ-style `bottom-left` / `bottom-right` pair so two tools can be open side by side in the bottom bar.

**Architecture:** Generalize the existing sidebar pair machinery (divider when both zones active, lone zone takes all space, remembered ratio) to the bottom, rotated 90°. `'bottom'` stays accepted forever as an alias for `'bottom-left'` at every zone-name entry point, so old persisted layouts and existing registrations keep working. Rust persistence changes are additive only.

**Tech Stack:** Vanilla JS IIFE frontend modules (`window.*` globals, VM-loaded node test suites in `scripts/tests/`), Rust (serde-persisted `state.toml` via `termlab_core`), Tauri v2 commands.

**Spec:** `docs/superpowers/specs/2026-08-26-bottom-zone-split-design.md`

## Global Constraints

- Branch: all work on `feat/bottom-zone-split` in `/Users/dustin/projects/conch` (verify with `git branch --show-current` before every commit; never commit to `main`).
- No Co-Authored-By lines in commits; imperative commit messages.
- Backward compat: old `state.toml` files (zone value `"bottom"`, `active_tool_windows` key `"bottom"`, missing `bottom_split_ratio`) must load with the same visible layout as before, tools landing in `bottom-left`.
- CSS: design tokens only (`var(--tl-*)`, `var(--dim-fg)` etc.), no raw hex in `styles/design-system/`; zone CSS lives in `styles/tool-windows.css`.
- Node suites run as `node scripts/tests/<file>.mjs`; VM sandbox realm — `JSON.parse(JSON.stringify(x))` before `deepStrictEqual` on arrays/objects that crossed the boundary.
- After every task: the task's suite passes AND `for f in scripts/tests/*.mjs; do node "$f" >/dev/null 2>&1 || echo "FAIL: $f"; done` prints nothing.

---

### Task 1: Zone list + `'bottom'` aliasing in the manager

**Files:**
- Modify: `crates/termlab_tauri/frontend/app/layout/tool-window-manager.js`
- Test: `scripts/tests/test_bottom_zone_split.mjs` (create)

**Interfaces:**
- Consumes: nothing new.
- Produces: `ZONE_IDS = ['left-top','left-bottom','right-top','right-bottom','bottom-left','bottom-right']`; exported `normalizeZoneName(name)` on `window.toolWindowManager` (returns `'bottom-left'` for `'bottom'`, the name itself for a known zone, `null` for anything else). All persisted-state setters and `register()`/`moveTo()` accept `'bottom'` and land on `'bottom-left'`. Later tasks rely on `zones['bottom-left']` / `zones['bottom-right']` existing and `sideForZone()` returning `'bottom'` for both.

- [ ] **Step 1: Write the failing test**

Create `scripts/tests/test_bottom_zone_split.mjs`. Copy the `makeElement`/`loadManager` harness verbatim from `scripts/tests/test_tool_window_closed_state.mjs` lines 39-95 (it stubs just enough DOM; `getElementById` returns null so every `update*` helper bails safely). Then:

```js
// --- 1. normalizeZoneName aliasing ------------------------------------------
{
  const twm = loadManager();
  assert.strictEqual(twm.normalizeZoneName('bottom'), 'bottom-left', 'legacy alias');
  assert.strictEqual(twm.normalizeZoneName('bottom-left'), 'bottom-left');
  assert.strictEqual(twm.normalizeZoneName('bottom-right'), 'bottom-right');
  assert.strictEqual(twm.normalizeZoneName('left-top'), 'left-top');
  assert.strictEqual(twm.normalizeZoneName('attic'), null, 'junk is rejected');
}

// --- 2. register() with legacy defaultZone lands in bottom-left --------------
{
  const twm = loadManager();
  twm.register('file-explorer', { title: 'SFTP', type: 'builtin', defaultZone: 'bottom', renderFn: () => null });
  assert.strictEqual(twm.getZoneForWindow('file-explorer'), 'bottom-left');
}

// --- 3. persisted legacy zone value and active key both migrate --------------
{
  const twm = loadManager();
  twm.setPersistedZones({ 'file-explorer': 'bottom' });
  twm.setPersistedActiveZoneWindows({ bottom: 'file-explorer' });
  twm.setPersistedPanelVisibility({ left: true, right: true, bottom: true });
  twm.setPanelVisibility('bottom', true, { save: false });
  twm.register('file-explorer', { title: 'SFTP', type: 'builtin', defaultZone: 'bottom', renderFn: () => null });
  assert.strictEqual(twm.getZoneForWindow('file-explorer'), 'bottom-left');
  assert.strictEqual(twm.isVisible('file-explorer'), true, 'saved-active window restores active');
  const active = JSON.parse(JSON.stringify(twm.getActiveZoneAssignments()));
  assert.strictEqual(active['bottom-left'], 'file-explorer', 'saves emit only new keys');
  assert.ok(!('bottom' in active), 'legacy key never re-emitted');
}

// --- 4. closed-state marker survives migration --------------------------------
{
  const twm = loadManager();
  twm.setPersistedZones({ 'file-explorer': 'bottom' });
  twm.setPersistedActiveZoneWindows({ bottom: '' });
  twm.setPersistedPanelVisibility({ left: true, right: true, bottom: true });
  twm.setPanelVisibility('bottom', true, { save: false });
  twm.register('file-explorer', { title: 'SFTP', type: 'builtin', defaultZone: 'bottom', renderFn: () => null });
  assert.strictEqual(twm.isVisible('file-explorer'), false, 'closed-while-bottom stays closed');
  assert.strictEqual(
    JSON.parse(JSON.stringify(twm.getActiveZoneAssignments()))['bottom-left'], '',
    'closed marker re-serialises under the new key',
  );
}

// --- 5. moveTo accepts both new names -----------------------------------------
{
  const twm = loadManager();
  twm.register('a', { title: 'A', type: 'builtin', defaultZone: 'bottom-left', renderFn: () => null });
  twm.register('b', { title: 'B', type: 'builtin', defaultZone: 'left-top', renderFn: () => null });
  twm.moveTo('b', 'bottom-right');
  assert.strictEqual(twm.getZoneForWindow('b'), 'bottom-right');
  twm.moveTo('b', 'bottom');           // legacy alias still routes
  assert.strictEqual(twm.getZoneForWindow('b'), 'bottom-left');
}

console.log('test_bottom_zone_split: zone aliasing assertions passed');
```

- [ ] **Step 2: Run it — expect FAIL** (`node scripts/tests/test_bottom_zone_split.mjs`): `normalizeZoneName is not a function`.

- [ ] **Step 3: Implement in `tool-window-manager.js`**

1. Line 8: `const ZONE_IDS = ['left-top', 'left-bottom', 'right-top', 'right-bottom', 'bottom-left', 'bottom-right'];` and update the file-header comment (line 2-3) to name six zones.
2. Line 29: same six names in `DRAGGABLE_ZONES`.
3. Lines 30-36 `ZONE_LABELS`: replace `bottom: 'Bottom'` with `'bottom-left': 'Bottom Left', 'bottom-right': 'Bottom Right'`.
4. Line 39: `const lastSplitRatios = { left: 0.5, right: 0.5, bottom: 0.5 };`
5. Add next to `sideForZone` (line ~1481):

```js
  // 'bottom' predates the bottom-left/right pair and stays accepted forever:
  // old state.toml zone values, plugin location strings, and defaultZone
  // registrations all still say it. Unknown names return null so callers
  // keep their existing fallback behaviour.
  function normalizeZoneName(name) {
    if (name === 'bottom') return 'bottom-left';
    return ZONE_IDS.includes(name) ? name : null;
  }
```

6. `setPersistedZones` (line 159): normalize values — `savedZoneAssignments` entries map through `normalizeZoneName`, dropping (deleting) entries that normalize to `null` (current behavior for junk is falling back to defaultZone; keep that by dropping the key).
7. `setPersistedActiveZoneWindows` (line 164): normalize KEYS the same way (a `bottom` key becomes `bottom-left`; on collision with an existing `bottom-left` key, the new-style key wins).
8. `register()` (line ~201): `const defaultZone = normalizeZoneName(opts.defaultZone) || 'right-bottom';` and the saved lookup already receives normalized values from step 6.
9. `moveTo(id, targetZone)`: first line `targetZone = normalizeZoneName(targetZone); if (!targetZone) return;` (find the function around line 730; it currently trusts the name).
10. `hasPersistedActiveForSide` (line ~190): replace the `if (side === 'bottom') return has('bottom');` arm with `if (side === 'bottom') return has('bottom-left') || has('bottom-right');` and update its comment (keys are normalized before this runs).
11. `updateBottomZone()` (line ~932): interim shim so the module still works before Task 2 — `zones.bottom` no longer exists, so change its `shouldShow` line to `const shouldShow = !zenActive && !!(panelState.bottom.visible && (zones['bottom-left'].activeId || zones['bottom-right'].activeId));`
12. Strip rendering (line ~1237-1247): interim shim — `const bottomZone = zones.bottom;` becomes `const bottomZone = zones['bottom-left'];` (Task 4 replaces this block wholesale). Also check for any other `zones.bottom` / `'bottom'` zone-name literal: `grep -n "zones.bottom\|'bottom'" crates/termlab_tauri/frontend/app/layout/tool-window-manager.js` — every hit must be either a SIDE usage (`panelState.bottom`, `strips.bottom`, `sideForZone` result, `savedPanelVisibility.bottom` — keep those) or covered by the edits above. The dock-rect map's `bottom:` entry (line ~995) keeps working untouched until Task 5 only if `DRAGGABLE_ZONES` no longer names it — since step 2 removed `bottom` from `DRAGGABLE_ZONES`, ALSO update the rect map now: replace the `bottom:` entry with two entries splitting the same band at the midpoint:

```js
      'bottom-left':  { left: pad, top: bottomBarY, width: Math.round((vw - pad * 2 - gap) / 2), height: Math.min(bottomBarH, Math.max(40, vh - bottomBarY - pad)) },
      'bottom-right': { left: pad + Math.round((vw - pad * 2 - gap) / 2) + gap, top: bottomBarY, width: Math.round((vw - pad * 2 - gap) / 2), height: Math.min(bottomBarH, Math.max(40, vh - bottomBarY - pad)) },
```

13. Context menu `buildContextMenuItems` (line ~1307): replace `{ zone: 'bottom', label: 'Bottom' }` with `{ zone: 'bottom-left', label: 'Bottom (Left)' }, { zone: 'bottom-right', label: 'Bottom (Right)' }`.
14. Export `normalizeZoneName` from the module's public object (line ~1666 area, alongside `setPersistedZones`).

- [ ] **Step 4: Run the new suite — expect PASS.** Then run the three existing manager suites: `node scripts/tests/test_tool_window_closed_state.mjs`, `node scripts/tests/test_tool_zone_markup.mjs`, `node scripts/tests/test_panel_host.mjs`. `test_tool_window_closed_state.mjs` asserts legacy `bottom` keys in `getActiveZoneAssignments()` output (lines ~113, 128, 149, 169) — update those assertions to expect the `bottom-left` key (the input fixtures may keep writing `bottom`; that is the point of the alias). `test_panel_host.mjs` checks every id gets menu entries — if it counts the move-to targets, adjust the count from 5 to 6.

- [ ] **Step 5: Run ALL suites** (loop from Global Constraints) — no failures.

- [ ] **Step 6: Commit** — `git add crates/termlab_tauri/frontend/app/layout/tool-window-manager.js scripts/tests/test_bottom_zone_split.mjs scripts/tests/test_tool_window_closed_state.mjs scripts/tests/test_panel_host.mjs && git commit -m "Split bottom tool zone into bottom-left/right with legacy alias"`

---

### Task 2: Bottom pair layout logic + generalized divider

**Files:**
- Modify: `crates/termlab_tauri/frontend/app/layout/tool-window-manager.js`
- Test: `scripts/tests/test_bottom_zone_split.mjs` (extend)

**Interfaces:**
- Consumes: Task 1's zones and `normalizeZoneName`.
- Produces: `updateBottomZone()` pair behavior; `initZoneDivider('bottom')` horizontal drag; `getSplitRatios()` returns `{ left, right, bottom }`; `setSplitRatio('bottom', ratio)` works. Task 6 calls `setSplitRatio('bottom', …)` from the load path.

- [ ] **Step 1: Write the failing test** — append to `test_bottom_zone_split.mjs`. The DOM-stub harness returns `null` from `getElementById`, so give this scenario a richer loader: copy `loadManager` into a `loadManagerWithBottomDom()` variant whose `document.getElementById` returns `makeElement('div')` for `'bottom-zone-wrap'` / `'bottom-zone-divider'`, and whose `document.querySelector` returns a fresh `makeElement('div')` (with a child `zone-tab-strip` element via `querySelector` returning another element) for each `[data-zone="…"]` selector, remembered per zone so the test can inspect them. Call `twm.initFromDom()` after loading. Then:

```js
// --- 6. pair layout: lone window takes full width, both restore ratio --------
{
  const { twm, zoneEls, dividerEl, wrapEl } = loadManagerWithBottomDom();
  twm.setPanelVisibility('bottom', true, { save: false });
  twm.register('a', { title: 'A', type: 'builtin', defaultZone: 'bottom-left', renderFn: () => null });
  assert.strictEqual(zoneEls['bottom-left'].style.flex, '1', 'lone left window gets all space');
  assert.strictEqual(zoneEls['bottom-right'].style.flex, '0');
  assert.ok(dividerEl.classList.contains('hidden'), 'no divider with one section');
  assert.ok(!wrapEl.classList.contains('hidden'), 'bar visible');

  twm.register('b', { title: 'B', type: 'builtin', defaultZone: 'bottom-right', renderFn: () => null });
  twm.activate('b');
  assert.ok(!dividerEl.classList.contains('hidden'), 'divider appears with both sections');
  const lf = parseFloat(zoneEls['bottom-left'].style.flex);
  const rf = parseFloat(zoneEls['bottom-right'].style.flex);
  assert.ok(Math.abs(lf - 0.5) < 0.01 && Math.abs(rf - 0.5) < 0.01, 'both active defaults to 50/50');

  twm.deactivate('b');
  assert.strictEqual(zoneEls['bottom-left'].style.flex, '1', 'closing right gives left all space');
  assert.ok(dividerEl.classList.contains('hidden'));

  twm.deactivate('a');
  assert.ok(wrapEl.classList.contains('hidden'), 'bar hides when both sections empty');
}

// --- 7. split ratio API grows a bottom arm ------------------------------------
{
  const { twm } = loadManagerWithBottomDom();
  twm.setPanelVisibility('bottom', true, { save: false });
  twm.register('a', { title: 'A', type: 'builtin', defaultZone: 'bottom-left', renderFn: () => null });
  twm.register('b', { title: 'B', type: 'builtin', defaultZone: 'bottom-right', renderFn: () => null });
  twm.activate('b'); twm.activate('a');
  twm.setSplitRatio('bottom', 0.3);
  const ratios = JSON.parse(JSON.stringify(twm.getSplitRatios()));
  assert.ok(Math.abs(ratios.bottom - 0.3) < 0.01, 'bottom ratio round-trips');
}

console.log('test_bottom_zone_split: pair layout assertions passed');
```

- [ ] **Step 2: Run — expect FAIL** (divider stays hidden / flex never set: `updateBottomZone` has no pair logic yet; `getSplitRatios()` has no bottom key).

- [ ] **Step 3: Implement**

1. Module state (near line 92): add `let bottomZoneDividerEl = null;`
2. `initFromDom` (~line 128-158): the per-zone `[data-zone]` query loop already picks up the two new zones. After the sidebar divider lookups add:

```js
    bottomZoneDividerEl = document.getElementById('bottom-zone-divider');
    initZoneDivider('bottom');
```

3. Replace `updateBottomZone()` (line ~932) with the pair logic (mirror of `updateSidebar`, horizontal):

```js
  function updateBottomZone() {
    if (!bottomZoneWrapEl) return;
    const appRoot = document.getElementById('app');
    const zenActive = !!(appRoot && appRoot.classList.contains('zen-mode'));
    const leftZone = zones['bottom-left'];
    const rightZone = zones['bottom-right'];
    const leftActive = leftZone.activeId !== null;
    const rightActive = rightZone.activeId !== null;
    const shouldShow = !zenActive && !!(panelState.bottom.visible && (leftActive || rightActive));
    bottomZoneWrapEl.classList.toggle('hidden', !shouldShow);

    if (bottomZoneDividerEl) {
      if (leftActive && rightActive) bottomZoneDividerEl.classList.remove('hidden');
      else bottomZoneDividerEl.classList.add('hidden');
    }

    if (leftZone.el && rightZone.el) {
      if (leftActive && !rightActive) {
        leftZone.el.style.flex = '1';
        rightZone.el.style.flex = '0';
      } else if (!leftActive && rightActive) {
        leftZone.el.style.flex = '0';
        rightZone.el.style.flex = '1';
      } else if (leftActive && rightActive) {
        const lf = parseFloat(leftZone.el.style.flex) || 0;
        const rf = parseFloat(rightZone.el.style.flex) || 0;
        if (lf < 0.1 || rf < 0.1) {
          const ratio = lastSplitRatios.bottom || 0.5;
          leftZone.el.style.flex = ratio.toString();
          rightZone.el.style.flex = (1 - ratio).toString();
        }
      }
    }
    if (fitActiveTabFn) fitActiveTabFn();
  }
```

4. Generalize `initZoneDivider(side)` (line ~1420). Head of the function becomes:

```js
  function initZoneDivider(side) {
    const horizontal = side === 'bottom';
    const dividerEl = horizontal ? bottomZoneDividerEl : sidebars[side].dividerEl;
    const firstZoneEl = horizontal ? zones['bottom-left'].el : zones[side + '-top'].el;
    const secondZoneEl = horizontal ? zones['bottom-right'].el : zones[side + '-bottom'].el;
    if (!dividerEl || !firstZoneEl || !secondZoneEl) return;
```

Then rename `topZoneEl`/`botZoneEl` → `firstZoneEl`/`secondZoneEl` throughout the function, and make the axis conditional: `startY = e.clientY` → `startPos = horizontal ? e.clientX : e.clientY`; `topH = topZoneEl.offsetHeight` → `firstSize = horizontal ? firstZoneEl.offsetWidth : firstZoneEl.offsetHeight` (same for second); container size `container.clientHeight - dividerEl.offsetHeight` → `horizontal ? container.clientWidth - dividerEl.offsetWidth : container.clientHeight - dividerEl.offsetHeight`; `delta = e.clientY - startY` → `(horizontal ? e.clientX : e.clientY) - startPos`; cursor `'row-resize'` → `horizontal ? 'col-resize' : 'row-resize'`. The ratio clamp (0.15–0.85), `lastSplitRatios[side] = newRatio`, `fitActiveTabFn`, and the pointerup/pointercancel handlers stay as they are.

5. `getSplitRatios()` (line ~1610): after the `['left','right']` loop add:

```js
    const blEl = zones['bottom-left'].el;
    const brEl = zones['bottom-right'].el;
    if (blEl && brEl) {
      const lf = parseFloat(blEl.style.flex) || 1;
      const rf = parseFloat(brEl.style.flex) || 1;
      ratios.bottom = lf / (lf + rf);
    }
```

6. `setSplitRatio(side, ratio)` (line ~1625): generalize the element lookup:

```js
  function setSplitRatio(side, ratio) {
    const firstEl = side === 'bottom' ? zones['bottom-left'].el : zones[side + '-top'].el;
    const secondEl = side === 'bottom' ? zones['bottom-right'].el : zones[side + '-bottom'].el;
    if (firstEl && secondEl && ratio > 0 && ratio < 1) {
      firstEl.style.flex = ratio.toString();
      secondEl.style.flex = (1 - ratio).toString();
      lastSplitRatios[side] = ratio;
    }
  }
```

- [ ] **Step 4: Run the suite — expect PASS.** Then all suites — no failures.

- [ ] **Step 5: Commit** — `git commit -m "Give the bottom zone pair the sidebar split behaviour"` (add the two touched files).

---

### Task 3: Markup + CSS for the bottom pair

**Files:**
- Modify: `crates/termlab_tauri/frontend/index.html` (lines 73-78)
- Modify: `crates/termlab_tauri/frontend/styles/tool-windows.css` (bottom-zone block, line ~124)
- Test: `scripts/tests/test_tool_zone_markup.mjs` (extend/update)

**Interfaces:**
- Consumes: Task 2's `initZoneDivider('bottom')` expecting `#bottom-zone-divider`, zone lookup expecting `[data-zone="bottom-left"]` / `[data-zone="bottom-right"]` each containing a `.zone-tab-strip`.
- Produces: the DOM contract above, plus `.bottom-zone-row` / `.zone-divider--vertical` CSS. Task 4 relies on `#bottom-strip` flex row with `space-between`.

- [ ] **Step 1: Extend the markup test.** Open `scripts/tests/test_tool_zone_markup.mjs` and read how it parses `index.html` (it validates zone structure). Add assertions: `index.html` contains no `data-zone="bottom"` (exact string with closing quote), contains exactly one `data-zone="bottom-left"` and one `data-zone="bottom-right"`, each followed by a `zone-tab-strip` child (match the existing zones' assertion style), and contains `id="bottom-zone-divider"`. Run — expect FAIL (markup still has the single zone).

- [ ] **Step 2: Update `index.html`** lines 73-78. Replace:

```html
    <div id="bottom-zone-wrap" class="bottom-zone-wrap hidden">
      <div id="bottom-zone-resize" class="bottom-zone-resize"></div>
      <div class="tool-zone empty" data-zone="bottom">
        <div class="zone-tab-strip hidden"></div>
      </div>
    </div>
```

with:

```html
    <div id="bottom-zone-wrap" class="bottom-zone-wrap hidden">
      <div id="bottom-zone-resize" class="bottom-zone-resize"></div>
      <div class="bottom-zone-row">
        <div class="tool-zone empty" data-zone="bottom-left">
          <div class="zone-tab-strip hidden"></div>
        </div>
        <div id="bottom-zone-divider" class="zone-divider zone-divider--vertical hidden"></div>
        <div class="tool-zone empty" data-zone="bottom-right">
          <div class="zone-tab-strip hidden"></div>
        </div>
      </div>
    </div>
```

- [ ] **Step 3: Update `styles/tool-windows.css`.** In the `/* -- Bottom tool-window zone -- */` block (line ~124) add:

```css
    .bottom-zone-row {
      display: flex;
      flex: 1;
      min-height: 0;
      min-width: 0;
    }
    /* Same affordance as the horizontal .zone-divider, rotated: drags the
       bottom-left/right split. */
    .zone-divider--vertical {
      width: 4px; height: auto; cursor: col-resize;
    }
    #bottom-strip { justify-content: space-between; }
```

(The base `.zone-divider` rule sets `height: 4px; cursor: row-resize;` — the `--vertical` modifier must appear AFTER it in the file so it wins; both classes are on the element.)

- [ ] **Step 4: Run** `node scripts/tests/test_tool_zone_markup.mjs` — expect PASS. All suites — no failures.

- [ ] **Step 5: Commit** — `git commit -m "Add bottom zone pair markup and divider styles"`.

---

### Task 4: Bottom strip split ends

**Files:**
- Modify: `crates/termlab_tauri/frontend/app/layout/tool-window-manager.js` (strip block, line ~1237)
- Modify: `crates/termlab_tauri/frontend/styles/tool-windows.css` (strip-section rules, line ~144)
- Test: `scripts/tests/test_bottom_zone_split.mjs` (extend)

**Interfaces:**
- Consumes: `zones['bottom-left']` / `zones['bottom-right']`, `makeStripBtn(wid, zone, horizontal, side)` (unchanged).
- Produces: bottom strip DOM = two `.strip-section.strip-section--row` children (second also `.strip-section--end`); left section holds bottom-left windows' buttons, right section bottom-right's.

- [ ] **Step 1: Write the failing test** — append to `test_bottom_zone_split.mjs`, using a loader variant whose `getElementById('bottom-strip')` returns a remembered element (the manager reads `strips.bottom` in `initFromDom` via `document.getElementById('bottom-strip')`):

```js
// --- 8. strip splits into left/right ends -------------------------------------
{
  const { twm, stripEl } = loadManagerWithBottomDom(); // extend the loader to expose bottom-strip
  twm.setPanelVisibility('bottom', true, { save: false });
  twm.register('a', { title: 'A', type: 'builtin', defaultZone: 'bottom-left', renderFn: () => null });
  twm.register('b', { title: 'B', type: 'builtin', defaultZone: 'bottom-right', renderFn: () => null });
  const sections = stripEl.children.filter((c) => String(c.className).includes('strip-section'));
  assert.strictEqual(sections.length, 2, 'two strip sections');
  assert.ok(String(sections[1].className).includes('strip-section--end'), 'second section is the right end');
  assert.strictEqual(sections[0].children.length, 1, 'left end holds the bottom-left window');
  assert.strictEqual(sections[1].children.length, 1, 'right end holds the bottom-right window');
}

console.log('test_bottom_zone_split: strip assertions passed');
```

(Note: the stub's `innerHTML = ''` — add an `innerHTML` setter to `makeElement` that clears `children` when assigned `''`, since the strip renderer starts with that. Keep it minimal: `set innerHTML(v) { if (v === '') this.children.length = 0; }`.)

- [ ] **Step 2: Run — expect FAIL** (single flat strip, no sections).

- [ ] **Step 3: Implement.** Replace the bottom-strip block in `updateStrips()` (line ~1237-1247) with:

```js
    const bottomStripEl = strips.bottom;
    if (bottomStripEl) {
      bottomStripEl.innerHTML = '';
      const leftZone = zones['bottom-left'];
      const rightZone = zones['bottom-right'];
      const totalWindows = leftZone.windows.length + rightZone.windows.length;
      bottomStripEl.classList.toggle('hidden', totalWindows === 0);
      if (totalWindows > 0) {
        // IntelliJ split ends: bottom-left windows at the strip's left end,
        // bottom-right windows at the right end (space-between in CSS).
        const leftSection = document.createElement('div');
        leftSection.className = 'strip-section strip-section--row';
        for (const wid of leftZone.windows) {
          leftSection.appendChild(makeStripBtn(wid, leftZone, true, 'bottom'));
        }
        const rightSection = document.createElement('div');
        rightSection.className = 'strip-section strip-section--row strip-section--end';
        for (const wid of rightZone.windows) {
          rightSection.appendChild(makeStripBtn(wid, rightZone, true, 'bottom'));
        }
        bottomStripEl.appendChild(leftSection);
        bottomStripEl.appendChild(rightSection);
      }
    }
```

In `tool-windows.css` next to `.strip-section` (line ~144):

```css
    .strip-section--row { flex-direction: row; }
    .strip-section--end { margin-left: auto; }
```

- [ ] **Step 4: Run the suite — PASS; all suites — no failures.**

- [ ] **Step 5: Commit** — `git commit -m "Render bottom strip as IntelliJ-style split ends"`.

---

### Task 5: Rust persistence — `bottom_split_ratio`

**Files:**
- Modify: `crates/termlab_core/src/config/persistent.rs` (LayoutConfig, line ~142 and Default impl ~181)
- Modify: `crates/termlab_tauri/src/commands.rs` (`SplitRatios` ~246, `SavedLayout` ~271, `saved_layout_from_state` ~296, `merge_window_layout` ~360)
- Modify: `crates/termlab_tauri/frontend/app/tool-window-runtime.js` (load path, line ~578)
- Test: existing `#[cfg(test)]` modules in both Rust files

**Interfaces:**
- Consumes: `getSplitRatios()` already emits `{ left, right, bottom }` (Task 2) through the save path at `tool-window-runtime.js:424` (`split_ratios: twm.getSplitRatios()`), so the frontend is already SENDING `bottom` — Rust currently drops it.
- Produces: `SavedLayout.bottom_split_ratio: f64` (default 0.5) delivered to the frontend; `setSplitRatio('bottom', …)` applied on load.

- [ ] **Step 1: Write the failing Rust tests.** In `persistent.rs`'s tests (mirror the existing `left_split_ratio` round-trip at line ~246/290): set `bottom_split_ratio: 0.25` on a `LayoutConfig`, save, reload, assert `0.25`; and a backward-compat test deserializing a TOML snippet WITHOUT the field asserting the default `0.5` (follow the file's existing compat-test pattern). In `commands.rs`'s tests: extend the existing `merge_window_layout` / `saved_layout_from_state` pair tests — a `WindowLayout` with `split_ratios: Some(SplitRatios { left: None, right: None, bottom: Some(0.3) })` merges into state as `0.3`, and `saved_layout_from_state` reads it back.

- [ ] **Step 2: Run** `cargo test -p termlab_core -p termlab_tauri --quiet` — expect compile FAIL (field missing), which is the red state for additive struct work.

- [ ] **Step 3: Implement.**

1. `persistent.rs` `LayoutConfig`: add `pub bottom_split_ratio: f32,` with exactly the same serde attributes as `left_split_ratio` (line 142 — if that field carries `#[serde(default = "...")]` copy it pointing at a `0.5` default fn; if the struct is blanket `#[serde(default)]`, the Default impl covers it). Default impl (~181): `bottom_split_ratio: 0.5,`.
2. `commands.rs` `SplitRatios`: add `bottom: Option<f64>,`. `SavedLayout`: add `bottom_split_ratio: f64,`. `saved_layout_from_state`: `bottom_split_ratio: layout.bottom_split_ratio as f64,`. `merge_window_layout` (~360): alongside the existing left/right arms add `if let Some(b) = ratios.bottom { state.bottom_split_ratio = b as f32; }`.
3. `tool-window-runtime.js` (~578-581): after the right arm add:

```js
          if (typeof initialLayoutData.bottom_split_ratio === 'number') {
            global.toolWindowManager.setSplitRatio('bottom', initialLayoutData.bottom_split_ratio);
          }
```

Match the exact guard style of the `left`/`right` lines above it (read them first; they may check for finite/positive — copy that).

- [ ] **Step 4: Run** `cargo test -p termlab_core -p termlab_tauri --quiet` — PASS. Also `cargo test --workspace --quiet` and the node-suite loop — no failures. If `SavedLayout` derives `TS` (it does — `#[derive(Serialize, TS)]`), run whatever regenerates TS bindings if the repo has a step for it (`grep -rn "ts-rs\|export_bindings" crates/termlab_tauri/src/ | head` — if bindings are generated into `frontend/types/`, regenerate per the pattern found; if tests auto-export, `cargo test` already did it. Commit any regenerated file).

- [ ] **Step 5: Commit** — `git commit -m "Persist the bottom zone split ratio"` (all touched Rust + JS + any regenerated types).

---

### Task 6: Full gate + manual checklist note

**Files:**
- Modify: `docs/superpowers/specs/2026-08-26-bottom-zone-split-design.md` (status line only)

**Interfaces:** none — verification task.

- [ ] **Step 1: Full test gate.** `cargo test --workspace --quiet` and the node loop over `scripts/tests/*.mjs` — zero failures. `bash scripts/check_frontend_boundaries.sh` — the ONLY failure allowed is the pre-existing `tl-dialog.js:334` keydown finding (verify no new findings mention files this plan touched).

- [ ] **Step 2: Static sanity.** `node --check` every modified JS file. `grep -n "data-zone=\"bottom\"" crates/termlab_tauri/frontend/index.html` — no hits. `grep -n "'bottom'" crates/termlab_tauri/frontend/app/layout/tool-window-manager.js` — every remaining hit is a side/panel usage (`panelState`, `strips`, `sideForZone` returns, `setPanelVisibility`, visibility maps) or inside `normalizeZoneName`, none a zone-map key.

- [ ] **Step 3: Update the spec status** to `Implemented (manual verification pending)` and append a short "Manual verification" list: move a tool bottom-left↔bottom-right via context menu and via drag; divider drag persists across restart; legacy layout migration (rename `~/.config/termlab/state.toml` aside is the rollback if anything looks wrong); zen mode hides the bar; popped-out bottom window restarts correctly.

- [ ] **Step 4: Commit** — `git commit -m "Mark bottom zone split spec implemented"`. Push the branch: `git push -u origin feat/bottom-zone-split`.

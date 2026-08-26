# Composite Panel Host Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Popping out the SFTP tool window opens a host window containing SFTP (main) + Transfers (bottom panel); docking returns both to their original zones.

**Architecture:** Registrations gain a declarative `companions` option. The parent tool-window manager suppresses companions (runtime-only state, nothing persisted) while a composite host lives; the host window mounts main + companions with a divider; `companion_ids` flows through the panel-host registry additively. One consumer: `file-explorer` → `transfer-center`.

**Tech Stack:** Vanilla JS IIFE frontend (VM-loaded node suites in `scripts/tests/`), Rust/Tauri v2 (`panel_host.rs` registry + window builder, serde camelCase commands).

**Spec:** `docs/superpowers/specs/2026-08-26-composite-panel-host-design.md`

## Global Constraints

- Branch: all work on `feat/composite-panel-host` in `/Users/dustin/projects/conch` (verify `git branch --show-current` before every commit; never commit to main).
- No Co-Authored-By; imperative commit messages.
- Additive persistence/IPC only: `companion_ids` everywhere carries `#[serde(default)]`; old callers, old saved layouts, and solo hosts behave byte-for-byte as today. Nothing new is persisted for companions.
- Suppression lifts on dock/abort/unregister — **never** on `hide_panel_host`.
- Node suites run as `node scripts/tests/<file>.mjs`; VM realm: JSON-roundtrip cross-realm objects before `deepStrictEqual`; harnesses must not define `sandbox.global` unless the module needs it (tool-window-manager.js binds `exports` and must never reference bare `global.`).
- After every task: the task's suite AND the full sweep `for f in scripts/tests/*.mjs; do node "$f" >/dev/null 2>&1 || echo "FAIL: $f"; done` (prints nothing) pass; Rust tasks also `cargo test -p termlab_tauri --quiet`.

---

### Task 1: Rust — `companion_ids` through the panel-host registry

**Files:**
- Modify: `crates/termlab_tauri/src/panel_host.rs` (`PanelHostRequest` ~line 54, `PanelHostEntry` ~140, `PanelHostRegistry::open` ~173, `open_panel_host` ~1160, `get_panel_host_request` ~1221, `create_panel_host_window` ~961)
- Test: existing `#[cfg(test)]` module in `panel_host.rs`

**Interfaces:**
- Consumes: nothing new.
- Produces: `open_panel_host(window, tool_window_id, title, companion_ids: Option<Vec<String>>)` (frontend sends `companionIds`; Tauri camelCases automatically); `PanelHostRequest.companion_ids: Vec<String>` returned by `get_panel_host_request`; hosts with companions and no persisted bounds open 40% taller (clamped to the work area).

- [ ] **Step 1: Write the failing tests.** In `panel_host.rs`'s test module, mirror the existing registry-test style:

```rust
#[test]
fn registry_open_stores_companion_ids() {
    let mut reg = PanelHostRegistry::default();
    let (_, entry) = reg.open(
        "main".into(),
        "file-explorer".into(),
        "SFTP".into(),
        vec!["transfer-center".into()],
    );
    assert_eq!(entry.companion_ids, vec!["transfer-center".to_string()]);
    let stored = reg.get("main", "file-explorer").expect("entry stored");
    assert_eq!(stored.companion_ids, vec!["transfer-center".to_string()]);
}

#[test]
fn panel_host_request_companion_ids_default_empty() {
    // Old serialized requests carry no companion_ids: deserialization must
    // default to empty, not fail.
    let json = r#"{"reqId":1,"toolWindowId":"a","parentLabel":"main","title":"A"}"#;
    let req: PanelHostRequest = serde_json::from_str(json).expect("deserializes");
    assert!(req.companion_ids.is_empty());
}
```

- [ ] **Step 2: Run** `cargo test -p termlab_tauri --quiet` — expect compile FAIL (missing field/param), the red state for additive struct work.

- [ ] **Step 3: Implement.**

1. `PanelHostRequest`: add `#[serde(default)] pub companion_ids: Vec<String>,`.
2. `PanelHostEntry`: add `pub companion_ids: Vec<String>,`.
3. `PanelHostRegistry::open(&mut self, parent_label, tool_window_id, title, companion_ids: Vec<String>)`: store on the entry (both the inserted and returned copy). Update every existing caller/test of `open()` to pass `vec![]` (grep `\.open(` in the file).
4. `open_panel_host` command: add param `companion_ids: Option<Vec<String>>`; `let companion_ids = companion_ids.unwrap_or_default();` pass into `registry.open(...)`, and thread `has_companions = !companion_ids.is_empty()` plus the ids into the main-thread closure → `create_panel_host_window(&handle, &build_parent_label, &build_label, &build_tool_window_id, &build_title, build_has_companions)`.
5. `get_panel_host_request`: map `companion_ids: e.companion_ids.clone(),`.
6. `create_panel_host_window(..., has_companions: bool)`: in the `None => (PANEL_HOST_DEFAULT_WIDTH, PANEL_HOST_DEFAULT_HEIGHT)` arm, when `has_companions` use `clamp_dimension(PANEL_HOST_DEFAULT_HEIGHT * 1.4, PANEL_HOST_MIN_HEIGHT, work_area.map(|(_, _, _, h)| h))` for the height (persisted bounds, when present, win unchanged — the user's remembered size is authoritative).

- [ ] **Step 4: Run** `cargo test -p termlab_tauri --quiet` — PASS; then `cargo test --workspace --quiet` — PASS.

- [ ] **Step 5: Commit** — `git add crates/termlab_tauri/src/panel_host.rs && git commit -m "Thread companion ids through the panel-host registry"`

---

### Task 2: Manager — `companions` registration + suppression core

**Files:**
- Modify: `crates/termlab_tauri/frontend/app/layout/tool-window-manager.js`
- Test: `scripts/tests/test_composite_host.mjs` (create)

**Interfaces:**
- Consumes: Task 1's `open_panel_host` accepting `companionIds` (the manager's `panelHostInvoke('open_panel_host', {...})` calls gain the field; Rust tolerates its absence, so this task is independently shippable).
- Produces (Task 3/5 rely on these exact names): `companionSuppressions: Map<companionId, {hostId, wasActive}>` (module state); `isCompanionSuppressed(id)` exported; `companionIdsFor(id)` internal (registered, non-self companion ids from `tw.companions`); `suppressCompanionsFor(hostId)` / `liftCompanionsFor(hostId)` internal; `register()` accepts `opts.companions` (array of `{id, position}`) stored as `tw.companions`.

- [ ] **Step 1: Write the failing test.** Create `scripts/tests/test_composite_host.mjs`. Copy the `makeElement`/`loadManager` harness from `scripts/tests/test_tool_window_closed_state.mjs` lines 39-95 verbatim, but do NOT set `sandbox.global` (guard against bare-global regressions), set `sandbox.window = sandbox` as there. Record `panelHostInvoke` traffic by stubbing what the manager uses: read the manager's `init(opts)` signature first — it receives `invoke` (grep `panelHostInvoke` to see how commands are sent) — and capture calls into `calls.invokes = [{cmd, args}]`, resolving with a fake reqId. Scenarios:

```js
const HOST = { title: 'SFTP', type: 'builtin', defaultZone: 'bottom-left', renderFn: () => null,
  companions: [{ id: 'transfer-center', position: 'bottom' }] };
const COMPANION = { title: 'Transfers', type: 'builtin', defaultZone: 'bottom-right', renderFn: () => null };

// --- 1. pop-out suppresses an ACTIVE companion; open carries companionIds ---
{
  const { twm, calls } = loadManager();
  twm.register('file-explorer', HOST);
  twm.register('transfer-center', COMPANION);
  twm.activate('transfer-center');
  twm.setViewMode('file-explorer', 'window');          // enterWindowMode path
  assert.strictEqual(twm.isCompanionSuppressed('transfer-center'), true);
  assert.strictEqual(twm.isVisible('transfer-center'), false, 'companion left its zone');
  const open = calls.invokes.find((c) => c.cmd === 'open_panel_host');
  assert.deepStrictEqual(JSON.parse(JSON.stringify(open.args.companionIds)), ['transfer-center']);
}

// --- 2. dock-back restores the companion, active state preserved -------------
{
  const { twm } = loadManager();
  twm.register('file-explorer', HOST);
  twm.register('transfer-center', COMPANION);
  twm.activate('transfer-center');
  twm.setViewMode('file-explorer', 'window');
  twm.setViewMode('file-explorer', 'dock');            // dockFromWindowMode path
  assert.strictEqual(twm.isCompanionSuppressed('transfer-center'), false);
  assert.strictEqual(twm.isVisible('transfer-center'), true, 'was active → active again');
}

// --- 3. a companion CLOSED before pop-out stays closed after dock ------------
{
  const { twm } = loadManager();
  twm.register('file-explorer', HOST);
  twm.register('transfer-center', COMPANION);
  // registered but never activated (or deactivate it if register auto-activates)
  if (twm.isVisible('transfer-center')) twm.deactivate('transfer-center');
  twm.setViewMode('file-explorer', 'window');
  twm.setViewMode('file-explorer', 'dock');
  assert.strictEqual(twm.isVisible('transfer-center'), false, 'was closed → stays closed');
}

// --- 4. activate() is inert while suppressed (auto-open guard) ---------------
{
  const { twm } = loadManager();
  twm.register('file-explorer', HOST);
  twm.register('transfer-center', COMPANION);
  twm.setViewMode('file-explorer', 'window');
  twm.activate('transfer-center');
  assert.strictEqual(twm.isVisible('transfer-center'), false, 'suppressed id cannot activate');
}

// --- 5. hide does NOT lift; abort does ---------------------------------------
{
  const { twm } = loadManager();
  twm.register('file-explorer', HOST);
  twm.register('transfer-center', COMPANION);
  twm.setViewMode('file-explorer', 'window');
  twm.toggle('file-explorer');                          // hides the host window
  assert.strictEqual(twm.isCompanionSuppressed('transfer-center'), true, 'hide keeps suppression');
  twm.notifyHostAborted('file-explorer');
  assert.strictEqual(twm.isCompanionSuppressed('transfer-center'), false, 'abort lifts');
}

// --- 6. companion popped out solo first gets absorbed -------------------------
{
  const { twm } = loadManager();
  twm.register('file-explorer', HOST);
  twm.register('transfer-center', COMPANION);
  twm.setViewMode('transfer-center', 'window');
  twm.setViewMode('file-explorer', 'window');
  assert.strictEqual(twm.isCompanionSuppressed('transfer-center'), true);
  assert.strictEqual(twm.getViewMode ? twm.getViewMode('transfer-center') : 'dock', 'dock',
    'solo host docked before suppression');
}

// --- 7. registration-order restore: host registers first, companion later ----
{
  const { twm } = loadManager();
  twm.setPersistedViewModes({ 'file-explorer': 'window' });
  twm.setPersistedZones({ 'file-explorer': 'bottom-left', 'transfer-center': 'bottom-right' });
  twm.setPersistedActiveZoneWindows({ 'bottom-left': 'file-explorer' });
  twm.register('file-explorer', HOST);
  twm.register('transfer-center', COMPANION);
  assert.strictEqual(twm.isCompanionSuppressed('transfer-center'), true,
    'companion registering after the popped-out host is suppressed on arrival');
}
```

Adjust the scenario mechanics to the manager's real public API after reading it: use `setViewMode(id, 'window'|'dock')` (exported — verify with grep; if pop-out goes through a different exported name, use that and note it in the report). Add exports needed by the test (`isCompanionSuppressed`, `getViewMode` if missing) as part of Step 3.

- [ ] **Step 2: Run** `node scripts/tests/test_composite_host.mjs` — expect FAIL (`isCompanionSuppressed is not a function`).

- [ ] **Step 3: Implement** in `tool-window-manager.js`:

1. Module state (near `viewModes`):

```js
  // companionId → { hostId, wasActive } while the companion rides inside a
  // live composite host; nothing here persists — dock/abort/unregister of
  // the host lifts it, restart re-derives it from the host's own summon.
  const companionSuppressions = new Map();
  // Registration order on boot: the host tool can enter window mode before
  // its companion's register() call arrives.
  const pendingCompanionSuppressions = new Map(); // companionId → hostId
```

2. `register()`: store `companions: Array.isArray(opts.companions) ? opts.companions.filter((c) => c && c.id) : []` on `tw`. After the existing window-mode restore branch decides the host is visible/popped (`savedViewModes[id] === VIEW_MODE_WINDOW && savedActiveId === id`), queue each of its companion ids: registered → `suppressCompanion(cid, id)`, else `pendingCompanionSuppressions.set(cid, id)`. And in the normal (non-window-mode) path: after the tw is created and pushed into its zone's window list as usual, check `pendingCompanionSuppressions.has(id)`; if so, take the hostId out of the map, skip every activate/restore branch, call `suppressCompanion(id, hostId)`, run the usual `updateZone/updateSidebar/updateBottomZone/updateStrips`, and return.
3. Helpers:

```js
  function companionIdsFor(id) {
    const tw = toolWindows.get(id);
    if (!tw || !Array.isArray(tw.companions)) return [];
    return tw.companions
      .map((c) => c.id)
      .filter((cid) => cid && cid !== id && toolWindows.has(cid));
  }

  function suppressCompanion(companionId, hostId) {
    if (companionSuppressions.has(companionId)) return;
    const tw = toolWindows.get(companionId);
    if (!tw) return;
    // A companion living in its own host window folds back first — the
    // composite absorbs it, two hosts for one pair would fight over it.
    if (getViewMode(companionId) === VIEW_MODE_WINDOW) {
      dockFromWindowMode(companionId);
    }
    companionSuppressions.set(companionId, { hostId, wasActive: !!tw.active });
    tw.active = false;
    if (tw.el) tw.el.style.display = 'none';
    const zone = zones[tw.zone];
    if (zone && zone.activeId === companionId) zone.activeId = null;
    updateZone(tw.zone);
    updateSidebar(sideForZone(tw.zone));
    updateBottomZone();
    updateStrips();
  }

  function suppressCompanionsFor(hostId) {
    for (const cid of companionIdsFor(hostId)) suppressCompanion(cid, hostId);
  }

  function liftCompanionsFor(hostId) {
    for (const [cid, info] of Array.from(companionSuppressions)) {
      if (info.hostId !== hostId) continue;
      companionSuppressions.delete(cid);
      const tw = toolWindows.get(cid);
      if (!tw) continue;
      if (info.wasActive && zones[tw.zone] && zones[tw.zone].activeId === null) {
        activate(cid);
      } else {
        updateZone(tw.zone);
        updateSidebar(sideForZone(tw.zone));
        updateBottomZone();
        updateStrips();
      }
    }
    for (const [cid, hid] of Array.from(pendingCompanionSuppressions)) {
      if (hid === hostId) pendingCompanionSuppressions.delete(cid);
    }
  }

  function isCompanionSuppressed(id) {
    return companionSuppressions.has(id);
  }
```

4. `activate(id)`: first line `if (companionSuppressions.has(id)) return;` (covers auto-open, palette, everything).
5. `enterWindowMode(tw)`: after `detachFromZone(tw)` and before `panelHostInvoke('open_panel_host', …)`, call `suppressCompanionsFor(id)`; change the invoke args to `{ toolWindowId: id, title: tw.title, companionIds: companionIdsFor(id) }`.
6. `summonWindowHost(id)`'s open-fallback: same args change, and call `suppressCompanionsFor(id)` right before the `open_panel_host` fallback fires (guarded — already-suppressed companions no-op via the `has` check).
7. `dockFromWindowMode(id, opts)`: after `resetToDock(id)`, call `liftCompanionsFor(id)` (this covers parent dock, host Dock button via `notifyHostDocked`, and the open-failure path).
8. `notifyHostAborted(id)`: add `liftCompanionsFor(id)` after `resetToDock(id)`.
9. `unregister(id)`: add `liftCompanionsFor(id)` and `companionSuppressions.delete(id); pendingCompanionSuppressions.delete(id);` (a removed companion must not linger in either map).
10. Export `isCompanionSuppressed` (and `getViewMode` if not already exported) from the public object.

- [ ] **Step 4: Run the suite — PASS; full sweep — no failures** (the existing `test_panel_host.mjs` must stay green: solo hosts send `companionIds: []`, which Rust defaults tolerate and the fake invokes ignore).

- [ ] **Step 5: Commit** — `git add crates/termlab_tauri/frontend/app/layout/tool-window-manager.js scripts/tests/test_composite_host.mjs && git commit -m "Suppress companion tools while their composite host is open"`

---

### Task 3: Strip "away" state, context-menu guards, SFTP declares its companion

**Files:**
- Modify: `crates/termlab_tauri/frontend/app/layout/tool-window-manager.js` (`makeStripBtn` ~1340, `buildContextMenuItems`)
- Modify: `crates/termlab_tauri/frontend/app/tool-window-runtime.js` (file-explorer registration ~line 110)
- Modify: `crates/termlab_tauri/frontend/styles/tool-windows.css`
- Test: `scripts/tests/test_composite_host.mjs` (extend)

**Interfaces:**
- Consumes: Task 2's `companionSuppressions` / `isCompanionSuppressed`.
- Produces: suppressed companions render `.strip-btn--away`; clicking one focuses the host (`focus_panel_host { toolWindowId: <hostId> }`, falling back to `summonWindowHost(hostId)`); their context-menu move/view/hide entries are disabled; `file-explorer` registers `companions: [{ id: 'transfer-center', position: 'bottom' }]`.

- [ ] **Step 1: Write the failing tests** (append to `test_composite_host.mjs`; the harness's strip elements need `children`-capable stubs — reuse the Task 2 loader, extending it to expose strip elements the way `test_bottom_zone_split.mjs`'s `loadManagerWithBottomDom` exposes `bottom-strip`):

```js
// --- 8. suppressed companion strip button: away class, click focuses host ----
{
  const { twm, calls, stripEls } = loadManagerWithStrips();
  twm.register('file-explorer', HOST);
  twm.register('transfer-center', COMPANION);
  twm.setViewMode('file-explorer', 'window');
  const btn = findStripBtn(stripEls, 'transfer-center'); // helper: walk sections' children by dataset.toolWindow
  assert.ok(String(btn.className).includes('strip-btn--away'), 'away styling');
  assert.ok(!String(btn.className).includes('active'), 'never lit while away');
  btn.click();                                           // stub: invoke stored click handler
  const focus = calls.invokes.find((c) => c.cmd === 'focus_panel_host');
  assert.strictEqual(focus.args.toolWindowId, 'file-explorer', 'click targets the HOST id');
}

// --- 9. context menu: move/view/hide disabled while suppressed ----------------
{
  const { twm } = loadManager();
  twm.register('file-explorer', HOST);
  twm.register('transfer-center', COMPANION);
  twm.setViewMode('file-explorer', 'window');
  const items = twm.buildContextMenuItems
    ? twm.buildContextMenuItems('transfer-center')
    : null; // if not exported, export it in Step 3 for testability (test_panel_host.mjs precedent)
  const actionable = items.filter((i) => !i.separator);
  assert.ok(actionable.every((i) => i.disabled === true),
    'every actionable entry disabled while suppressed');
}
```

(Adapt `btn.click()` to the harness: `makeElement` records `addEventListener` callbacks — extend it to store them and expose `fire('click')`. Check whether `buildContextMenuItems` is already exported — `test_panel_host.mjs` asserts menu entries, read how it reaches them and use the same route.)

- [ ] **Step 2: Run — expect FAIL** (no away class, focus not invoked, menu enabled).

- [ ] **Step 3: Implement.**

1. `makeStripBtn`: compute `const away = companionSuppressions.get(windowId) || null;` next to the existing `isActive` computation; force `isActive` to `false` when `away`; fold `(away ? ' strip-btn--away' : '')` into the button's class string; and add a first route in the existing click listener (before the toggle): `if (away) { panelHostInvoke('focus_panel_host', { toolWindowId: away.hostId }).catch(() => summonWindowHost(away.hostId)); return; }` — the tool lives in that window, so the rail button's job is to bring the window forward, not to toggle a dock panel.
2. `buildContextMenuItems(windowId)`: `const suppressed = companionSuppressions.has(windowId);` — set `disabled: isCurrent || suppressed` on the move targets, `disabled: … || suppressed` on both view-mode entries, and `disabled: suppressed` on Hide. Export `buildContextMenuItems` if it is not already reachable from tests.
3. `tool-window-runtime.js` file-explorer registration: add `companions: [{ id: 'transfer-center', position: 'bottom' }],` beside its `defaultZone`.
4. `tool-windows.css`, next to `.strip-btn.twm-strip-dragging`:

```css
    /* A companion riding inside a composite pop-out: present on the rail,
       but "away" — clicking focuses the host window instead of docking. */
    .strip-btn--away { opacity: 0.55; }
```

- [ ] **Step 4: Run the suite — PASS; full sweep — no failures.**

- [ ] **Step 5: Commit** — `git add crates/termlab_tauri/frontend/app/layout/tool-window-manager.js crates/termlab_tauri/frontend/app/tool-window-runtime.js crates/termlab_tauri/frontend/styles/tool-windows.css scripts/tests/test_composite_host.mjs && git commit -m "Show suppressed companions as away and route them to their host"`

---

### Task 4: Host window — composite layout with bottom section

**Files:**
- Modify: `crates/termlab_tauri/frontend/app/panel-host-runtime.js` (`boot()` ~363-470, `mountRegistration` ~340)
- Modify: `crates/termlab_tauri/frontend/styles/panels.css`
- Test: `scripts/tests/test_panel_host.mjs` (extend)

**Interfaces:**
- Consumes: Task 1's `request.companion_ids` (`get_panel_host_request` → camelCase `companionIds` on the wire — CHECK the actual field name the frontend sees: `PanelHostRequest` is `#[serde(rename_all = "camelCase")]`, so the frontend reads `request.companionIds`).
- Produces: a host whose request carries resolvable companion ids renders `chrome content (main) + .panel-host-bottom { .panel-host-divider + .panel-host-bottom-content (companion mounts) }`; every mount's disposer runs on `beforeunload`; unresolvable ids degrade to the solo layout.

- [ ] **Step 1: Read `test_panel_host.mjs`'s host-boot harness** (`makeHostSandbox` ~line 1297) to see how `boot()` is driven and asserted today. **Write the failing tests** in its style:

```js
// Composite: request with companionIds mounts main + companion behind a divider.
{
  const h = makeHostSandbox({
    request: { reqId: 9, toolWindowId: 'file-explorer', parentLabel: 'main',
               title: 'SFTP', companionIds: ['transfer-center'] },
    registrations: ['file-explorer', 'transfer-center'],   // adapt to harness's registration seeding
  });
  const result = await h.boot();
  assert.strictEqual(result.status, 'mounted');            // match harness's existing success status
  assert.strictEqual(h.renderCalls['file-explorer'], 1, 'main tool rendered');
  assert.strictEqual(h.renderCalls['transfer-center'], 1, 'companion rendered');
  assert.ok(h.bodyHasClass ? true : true);
  assert.ok(h.query('.panel-host-bottom'), 'bottom section exists');
  assert.ok(h.query('.panel-host-divider'), 'divider exists');
  h.fireBeforeUnload();
  assert.strictEqual(h.disposeCalls['file-explorer'], 1);
  assert.strictEqual(h.disposeCalls['transfer-center'], 1, 'both disposers ran');
}

// Degrade: unresolvable companion id → solo layout, no bottom section.
{
  const h = makeHostSandbox({
    request: { reqId: 10, toolWindowId: 'file-explorer', parentLabel: 'main',
               title: 'SFTP', companionIds: ['ghost-tool'] },
    registrations: ['file-explorer'],
  });
  await h.boot();
  assert.strictEqual(h.renderCalls['file-explorer'], 1);
  assert.ok(!h.query('.panel-host-bottom'), 'no bottom section for unresolvable companion');
}
```

The exact helper names (`renderCalls`, `query`, `fireBeforeUnload`) must be adapted to what the harness actually provides — extend the harness minimally where it lacks a hook, in the harness's own idiom. The two behaviors under test are non-negotiable: both renderFns called + both disposers on unload; degrade to solo.

- [ ] **Step 2: Run** `node scripts/tests/test_panel_host.mjs` — new scenarios FAIL (no `.panel-host-bottom`, companion never rendered).

- [ ] **Step 3: Implement** in `panel-host-runtime.js` `boot()`, after the existing `mountRegistration(chrome.contentRootEl, registration)`:

```js
    // Companions ride along in a bottom section — SFTP + Transfers reads as
    // one small app. Unresolvable ids degrade to today's solo host.
    const companionIds = Array.isArray(request.companionIds) ? request.companionIds : [];
    const companionMounts = [];
    const companionRegs = companionIds
      .filter((cid) => cid && cid !== request.toolWindowId)
      .map((cid) => manager.getRegistration(cid))
      .filter((reg) => reg && typeof reg.renderFn === 'function');
    if (companionRegs.length > 0) {
      const bottom = document.createElement('div');
      bottom.className = 'panel-host-bottom';
      const divider = document.createElement('div');
      divider.className = 'panel-host-divider';
      const bottomContent = document.createElement('div');
      bottomContent.className = 'panel-host-bottom-content';
      bottom.appendChild(divider);
      bottom.appendChild(bottomContent);
      chrome.rootEl.appendChild(bottom);
      for (const reg of companionRegs) {
        companionMounts.push(mountRegistration(bottomContent, reg));
      }
      initHostBottomDivider(divider, bottom, chrome.rootEl);
    }
```

Extend the existing `disposeMountedPanel` to also destroy every entry in `companionMounts` (same ordering: companions first, then the main mount — teardown mirrors mount order reversed). Add the divider drag helper next to `buildChrome`:

```js
  // Bottom-section resize: same pointer idiom as the zone dividers, clamped
  // 120px .. 70% of the window. Session-only — nothing persists.
  function initHostBottomDivider(dividerEl, bottomEl, rootEl) {
    let dragging = false;
    let startY = 0;
    let startHeight = 0;
    dividerEl.addEventListener('pointerdown', (e) => {
      e.preventDefault();
      dividerEl.setPointerCapture(e.pointerId);
      dragging = true;
      startY = e.clientY;
      startHeight = bottomEl.offsetHeight;
    });
    dividerEl.addEventListener('pointermove', (e) => {
      if (!dragging) return;
      const max = Math.round((rootEl.offsetHeight || window.innerHeight) * 0.7);
      const next = Math.max(120, Math.min(max, startHeight + (startY - e.clientY)));
      bottomEl.style.height = next + 'px';
    });
    const stop = () => { dragging = false; };
    dividerEl.addEventListener('pointerup', stop);
    dividerEl.addEventListener('pointercancel', stop);
  }
```

`panels.css` (next to the existing panel-host chrome styles — find them via `grep -n "panel-host\|host-body" crates/termlab_tauri/frontend/styles/panels.css`):

```css
    .panel-host-bottom {
      flex-shrink: 0;
      height: 35%;
      min-height: 120px;
      display: flex;
      flex-direction: column;
      border-top: 1px solid var(--tl-border);
    }
    .panel-host-divider {
      height: 4px;
      flex-shrink: 0;
      cursor: row-resize;
    }
    .panel-host-divider:hover { background: var(--dim-fg); }
    .panel-host-bottom-content {
      flex: 1;
      min-height: 0;
      display: flex;
      flex-direction: column;
      overflow: hidden;
    }
```

Check `chrome.rootEl`'s CSS is a flex column whose content area flexes (read `buildChrome` ~270 and its styles); if the content root lacks `min-height: 0`, add it so the bottom section can actually claim space.

- [ ] **Step 4: Run** `node scripts/tests/test_panel_host.mjs` — PASS (new and old scenarios); full sweep — no failures.

- [ ] **Step 5: Commit** — `git add crates/termlab_tauri/frontend/app/panel-host-runtime.js crates/termlab_tauri/frontend/styles/panels.css scripts/tests/test_panel_host.mjs && git commit -m "Mount companion tools in a bottom section of the panel host"`

---

### Task 5: Full gate + spec status

**Files:**
- Modify: `docs/superpowers/specs/2026-08-26-composite-panel-host-design.md` (status line + manual list)

- [ ] **Step 1:** `cargo test --workspace --quiet` and the node sweep — zero failures. `bash scripts/check_frontend_boundaries.sh` — only the pre-existing `tl-dialog.js:334` keydown finding.
- [ ] **Step 2:** `node --check` every modified JS file (`git diff --name-only main...HEAD -- '*.js'`).
- [ ] **Step 3:** Update the spec status to `Implemented (manual verification pending)` and append the spec's own Manual list as unchecked items (pop out SFTP → Transfers leaves + appears in host; divider drag; live transfer progress in host; dock restores both; restart while popped restores composite; hide + rail summon).
- [ ] **Step 4:** Commit (`git commit -m "Mark composite panel host spec implemented"`) and push `git push -u origin feat/composite-panel-host`.

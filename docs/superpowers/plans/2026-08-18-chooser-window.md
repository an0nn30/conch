# Chooser Window Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move the file chooser from an in-app `tl-dialog` overlay into its own window-modal OS window, extracting the chooser UI into `file-dialog-view.js` along the way.

**Architecture:** A Rust `ChooserRegistry` (managed state) owns one pending chooser per parent window and resolves it exactly once from any exit path, emitting `chooser-resolved` to the parent. The parent's `chooseFile` becomes a scrim-raising proxy with unchanged signature and guards; a new `chooser.html` window renders the extracted view, which talks to the existing listing commands directly.

**Tech Stack:** Tauri 2.10.3 (`WebviewWindowBuilder`, `emit_to`, managed state), no-bundler IIFE frontend, `vm`-sandbox `.mjs` tests, tokens-only CSS.

**Spec:** `docs/superpowers/specs/2026-08-18-chooser-window-design.md` (in this repo; amended by Task 5's spec-sync step)

## Global Constraints

- Branch `feat/chooser-window`; never commit to main; NO Co-Authored-By trailers; imperative commit messages (CLAUDE.md).
- `chooseFile(options)` keeps its exact signature and resolution shape `{scope, path, entry} | null`, never rejects; `activeChoice` same-mode sharing, cross-mode null refusal, and `cancelForPane` semantics are byte-for-byte (spec Goals 4).
- Exactly-once resolution through one registry method; late resolvers are no-ops (spec, Window & lifecycle).
- The scrim is lowered in a `finally` on every settle path including invoke failure (spec, Modality).
- The view keeps every `tl-filedlg__*` class from the 2026-08-18 redesign; `test_file_dialog.mjs` assertions may change WHERE they look, never WHAT they assert.
- Tokens-only CSS; boundary script's only allowed failure is the pre-existing `tl-dialog.js:334`.
- Frontend suite baseline: 29 suites green (`for f in scripts/tests/test_*.mjs; do node "$f" || echo FAIL $f; done` from repo root); cargo baseline 624 tests.
- Floor constants: **720 × 420 logical px** (Task 1 verifies against CSS sums; if off by more than 8px, correct the constant AND the spec line in the same commit).

## File Structure

- `crates/termlab_tauri/src/chooser_window.rs` — NEW: registry, request/outcome types, all six commands, builder, lifecycle listeners. One responsibility: the chooser window's lifetime.
- `crates/termlab_core/src/config/persistent.rs` — MODIFY: add `chooser_window: Option<ChooserWindowSize>`.
- `crates/termlab_tauri/src/lib.rs` — MODIFY: register commands + managed state + window-event hooks.
- `frontend/app/features/editor/file-dialog-view.js` — NEW: the UI, extracted.
- `frontend/app/features/editor/file-dialog.js` — MODIFY: shrinks to proxy + `openForOpen`/`openForSave`/`cancelForPane` (which stay, unchanged consumers of the resolved choice).
- `frontend/chooser.html`, `frontend/app/chooser-window-runtime.js` — NEW: window shell.
- `frontend/styles/design-system/components/file-dialog.css` — MODIFY: window-body context + view-owned footer.
- Tests: `scripts/tests/test_file_dialog.mjs` (repoint to view), NEW `scripts/tests/test_file_dialog_proxy.mjs`, harness edits in the four guard suites, Rust unit tests in `chooser_window.rs`.

---

### Task 1: Rust chooser window module (registry, commands, builder, persistence)

**Files:**
- Create: `crates/termlab_tauri/src/chooser_window.rs`
- Modify: `crates/termlab_core/src/config/persistent.rs` (add field), `crates/termlab_tauri/src/lib.rs` (register module/commands/state/hooks)
- Test: `#[cfg(test)]` in `chooser_window.rs`; existing persistence round-trip tests in `persistent.rs`'s test module

**Interfaces:**
- Consumes: `windows::appearance_to_theme` (`windows.rs:22`), `crate::arm_window_show_fallback` (`lib.rs:75`), `config::load_persistent_state`/`save_persistent_state` (`termlab_core config/mod.rs:253,266`), the `run_on_main_thread` dispatch rule (`windows.rs:42-51`), `emit_to` precedent (`pty.rs:303`).
- Produces (Tasks 3-4 rely on these exact names):
  - Commands: `open_file_chooser(window, mode: String, filename: Option<String>, select_filename: bool) -> Result<u64, String>` (returns `req_id`); `get_chooser_request(window) -> Result<ChooserRequest, String>`; `resolve_file_chooser(window, req_id: u64, choice: Option<serde_json::Value>) -> Result<(), String>`; `cancel_file_chooser(window) -> Result<(), String>`; `focus_file_chooser(window) -> Result<(), String>`; `chooser_ready(window) -> Result<(), String>`.
  - Event to parent: `chooser-resolved` with payload `{ reqId: u64, choice: serde_json::Value | null }` (camelCase via serde rename).
  - `ChooserRequest { req_id: u64, mode: String, filename: Option<String>, select_filename: bool, parent_label: String }` (serialized camelCase).
  - The chooser window label scheme: `format!("chooser-{parent_label}")`.
  - `PersistentState.chooser_window: Option<ChooserWindowSize>` where `ChooserWindowSize { width: f64, height: f64 }`.

Design decisions bound here: the outcome `choice` is an opaque `serde_json::Value` relayed verbatim — Rust never interprets the scope/entry shape, so frontend shape changes can't drift a typed mirror. `open_file_chooser` rejects callers whose own label starts with `chooser-` (`Err("chooser windows cannot open choosers")`). Registry key is the parent label; `req_id` from a `static NEXT_CHOOSER_REQ: AtomicU64` (pattern: `windows.rs:15`).

- [ ] **Step 1: Write the failing registry unit tests** (pure logic — the registry struct takes no Tauri handles; window/emit side effects live in thin command wrappers that the tests don't cover)

```rust
// in chooser_window.rs #[cfg(test)] mod tests
#[test]
fn open_registers_and_returns_ids() {
    let mut r = ChooserRegistry::default();
    let a = r.open("window-1".into(), req("open"));
    let b = r.open("window-2".into(), req("save"));
    assert_ne!(a.unwrap().req_id, b.unwrap().req_id);
}
#[test]
fn duplicate_open_returns_existing_req_id_not_new_entry() {
    let mut r = ChooserRegistry::default();
    let first = r.open("window-1".into(), req("open")).unwrap();
    let dup = r.open("window-1".into(), req("open"));
    assert!(matches!(dup, Err(AlreadyOpen { req_id }) if req_id == first.req_id));
}
#[test]
fn resolve_is_exactly_once() {
    let mut r = ChooserRegistry::default();
    let p = r.open("window-1".into(), req("open")).unwrap();
    assert!(r.resolve("window-1", p.req_id).is_some()); // first wins, returns entry
    assert!(r.resolve("window-1", p.req_id).is_none()); // late resolver: no-op
}
#[test]
fn resolve_with_stale_req_id_is_noop() {
    let mut r = ChooserRegistry::default();
    let p = r.open("window-1".into(), req("open")).unwrap();
    assert!(r.resolve("window-1", p.req_id + 999).is_none());
    assert!(r.resolve("window-1", p.req_id).is_some()); // real one still live
}
#[test]
fn parent_death_drains_only_that_parents_entry() {
    let mut r = ChooserRegistry::default();
    r.open("window-1".into(), req("open")).unwrap();
    let keep = r.open("window-2".into(), req("open")).unwrap();
    assert!(r.resolve_for_parent_death("window-1").is_some());
    assert!(r.resolve("window-2", keep.req_id).is_some());
}
```

`req(mode)` is a small helper building a `ChooserRequest` with that mode. Also add to `persistent.rs`'s existing test module: a round-trip test that a `PersistentState` with `chooser_window: Some(ChooserWindowSize { width: 800.0, height: 500.0 })` survives serialize→deserialize, and that deserializing a TOML string WITHOUT the key yields `None` (backward compat with existing state.toml files — the struct already has `#[serde(default)]`).

- [ ] **Step 2: Run to verify failure** — `cargo test -p termlab_tauri chooser_window` → compile error (module absent). `cargo test -p termlab_core chooser_window` → compile error (type absent).

- [ ] **Step 3: Implement.** `ChooserRegistry { pending: HashMap<String, PendingChooser> }`; `PendingChooser { req_id: u64, request: ChooserRequest }`. `open` inserts or returns `Err(AlreadyOpen { req_id })`; `resolve(parent_label, req_id)` removes and returns the entry only when both match; `resolve_for_parent_death(parent_label)` removes unconditionally by key. Managed as `Mutex<ChooserRegistry>` via `app.manage`. Then the command layer:
  - `open_file_chooser`: validate caller label; lock registry; on `AlreadyOpen`, `set_focus` the existing chooser window and return the existing `req_id`; else insert, then on the main thread (`run_on_main_thread`, the `windows.rs:42-51` deadlock rule) build the window: label `chooser-<parent>`, URL `chooser.html`, title `"Open"`/`"Save As"` by mode, `.inner_size(persisted.chooser_window clamped to ≥ floor and ≤ the parent monitor's work area per spec)`, `.min_inner_size(720.0, 420.0)`, `.resizable(true)`, `.visible(false)`, decorations/theme per `create_settings_window` (`windows.rs:82-91`), `.minimizable(false)` (spec: modal windows do not minimize), `.parent(&parent)` on macOS/Windows only (`#[cfg]`-gated; Linux relies on the focus bounce), centered on the parent (compute from parent `outer_position()`/`outer_size()` and the target inner size before build; never move after show), menu removed. Arm `crate::arm_window_show_fallback` (5s rescue, `lib.rs:75-94`). If the build fails, remove the registry entry and return `Err` — a registry entry with no window is a stuck scrim on the parent.
  - `get_chooser_request`: derive parent label by stripping the `chooser-` prefix from the calling window's label; return the pending request or `Err("no pending chooser")`.
  - `resolve_file_chooser`: registry `resolve`; if it returns the entry, `emit_to(parent_label, "chooser-resolved", payload)` then close the chooser window (close AFTER emit — the emit must not race window teardown). Persist the window's final inner size (logical px: divide physical by `scale_factor()`) into `chooser_window` via load-mutate-save of the whole `PersistentState` (`config/mod.rs:253,266` — whole-struct save is the only API).
  - `cancel_file_chooser` (called by the PARENT with its own label): resolve-as-cancel, same emit+close path, `choice: null`.
  - `chooser_ready`: `window.show()` + `window.set_focus()` (mirror of `app_ready`, `commands.rs:248-251`).
  - `focus_file_chooser`: `set_focus` on `chooser-<caller label>` if it exists.
  - Lifecycle hooks in `lib.rs`'s window-event handling: on `WindowEvent::Destroyed` for any label with a registry entry → `resolve_for_parent_death` + emit (no-op on dead parent) + close chooser; on `WindowEvent::CloseRequested` for a `chooser-*` label → treat as cancel through the same resolve path; on `WindowEvent::Focused(true)` for a label with a live chooser → `set_focus` the chooser (the modal bounce).

- [ ] **Step 4: Verify floor constants.** Sum the shipped CSS: sidebar `flex-basis` (`file-dialog.css`, `.tl-filedlg__sidebar`) + path-bar flex-basis floors + gaps/padding for width; path bar + 24px header + 8 × `--tl-row-h` + footer + padding for height. If the sum disagrees with 720 × 420 by more than 8px in either axis, change the constant in `chooser_window.rs` AND the spec's Sizing line AND this plan's Global Constraints line in this task's commit.

- [ ] **Step 5: Run tests** — `cargo test -p termlab_tauri && cargo test -p termlab_core` → all green including new ones; count ≥ 624 + new.

- [ ] **Step 6: Commit** — `git add -A && git commit -m "Add chooser window registry, commands, and persisted size"`

### Task 2: Extract `file-dialog-view.js`; repoint the dialog suite

**Files:**
- Create: `crates/termlab_tauri/frontend/app/features/editor/file-dialog-view.js`
- Modify: `crates/termlab_tauri/frontend/app/features/editor/file-dialog.js` (remove what moved), `crates/termlab_tauri/frontend/index.html` (script tag for the view, before file-dialog.js), `crates/termlab_tauri/frontend/styles/design-system/components/file-dialog.css` (footer styles)
- Test: `scripts/tests/test_file_dialog.mjs`

**Interfaces:**
- Consumes: `termlabFileDialogModel` (formatters/sort, unchanged), `features/files/data-service.js` functions (unchanged), `tlIcon`, `tlDialog` (overwrite confirm only).
- Produces: `global.termlabFileDialogView.build(root, deps) -> { focusInitial }` where `deps = { data, mode, filename, selectFilename, parentWindowLabel, onResolve }`. `data` is the data-service-shaped object (the harness already stubs exactly this shape, `test_file_dialog.mjs:225-237`); `parentWindowLabel` replaces every `getCurrentWindowLabel` call inside the view — the view NEVER asks for its own label. `onResolve(choiceOrNull)` fires exactly once; the `finish` latch moves into the view. Task 3 relies on: `chooseFile` no longer building any DOM; Task 4 relies on: `build` rendering the complete chooser INCLUDING a footer bar.

The mechanical rule for the extraction: everything currently inside `chooseFile`'s closure from `// ----- state -----` (`file-dialog.js:453`) through the keyboard wiring, plus the helpers it calls (`buildScopes`, `enterScope`, `renderRows`, `sortBy`, `confirmOverwrite`, `attemptSave`, `activate`, `loadScopes`…), moves to the view. What does NOT move: `activeChoice`, `chooseFile`, `openForOpen`, `openForSave`, `cancelForPane`, the toast failure paths.

The footer changes shape deliberately (the ONE intended DOM change): the view now renders its own footer row `div.tl-filedlg__footer` containing `div.tl-filedlg__footer-start` (Hidden toggle + save-mode `footerCtl` + New Folder — the exact nodes `onOpen` used to reparent, `file-dialog.js:1106-1112`) and `div.tl-filedlg__footer-end` (Cancel button + primary button as `tl-btn` elements the view owns directly — `findFooterButton` (`:271`) is deleted; `primaryButton` becomes a direct reference and `syncPrimaryButton` keeps its logic). `tlDialog.open` disappears from the chooser path entirely; `confirmOverwrite` (`file-dialog.js:298-330`) moves to the view verbatim — it still uses `global.tlDialog` for the small confirm, which remains available in both hosts (index.html and Task 4's chooser.html). Escape: the view exposes its cancel through `onResolve(null)` wired to a root-level `keydown` listener for Escape (the tl-dialog Escape registration it used to inherit is gone; the view must own it — checked in tests below). Add `.tl-filedlg__footer` styles to `file-dialog.css` mirroring `.tl-dialog__footer`'s layout (`dialog.css` — space-between, gap, padding, top border) with tokens only.

- [ ] **Step 1: Write the failing checks.** In `test_file_dialog.mjs`, replace the `tlDialog`-driven entry: the harness stops loading `ui/tl-dialog.js` for the main path (keep loading it — `confirmOverwrite` needs it) and instead of `dialog.openForOpen()` calls `sandbox.termlabFileDialogView.build(rootEl, { data: stubData, mode: 'open', parentWindowLabel: opts.windowLabel ?? 'main', onResolve: capture })`. Every existing assertion keeps its meaning; lookups that walked the tl-dialog panel (`panel.querySelector('.tl-dialog__footer-start')` etc.) repoint to `.tl-filedlg__footer-start` / `.tl-filedlg__footer-end`. New checks: (a) `onResolve` exactly-once — simulate pick then Escape, and double-Enter on a selected row: one call total, assert with a counter; (b) label fixture — `windowLabel: 'window-7'` in the stub data-service vs `parentWindowLabel: 'main'` passed to build: only sessions whose window filter matches `'main'` may appear (extend the existing sessions fixture with one session per label so the wrong-label bug renders a visibly different sidebar); (c) Escape on the view root resolves null; (d) the footer renders Cancel + primary with the primary disabled until selection (existing assertion, repointed).
- [ ] **Step 2: Run to verify failure** — `node scripts/tests/test_file_dialog.mjs` → fails on `termlabFileDialogView` undefined.
- [ ] **Step 3: Extract.** Move the code per the mechanical rule; thread `deps.data` where the moved code called module-level `fileModel()`/data-service lookups; replace both `getCurrentWindowLabel` call sites with `deps.parentWindowLabel`; build the footer; wire Escape. `file-dialog.js` temporarily calls the view inside a tl-dialog body to stay green (`body: (el) => view.build(el, {...})` with the old buttons removed) — this bridge lives only until Task 3 replaces it with the proxy.
- [ ] **Step 4: Run the full suite** — all 29 green; `node --check` both files.
- [ ] **Step 5: Commit** — `git commit -m "Extract file chooser UI into file-dialog-view with its own footer"`

### Task 3: The proxy — `chooseFile` over the window boundary

**Files:**
- Modify: `crates/termlab_tauri/frontend/app/features/editor/file-dialog.js`, `crates/termlab_tauri/frontend/app/core/tauri-client.js` (event name registration if `resolveName` requires it — read `resolveName` first; if it passes unknown names through, no edit), `crates/termlab_tauri/frontend/index.html` (remove the view script tag ONLY if the main window no longer renders the view — it does not after this task; the view script moves to chooser.html in Task 4, but keep the tag until Task 4 lands to avoid a window where neither host loads it — Task 4 removes it)
- Test: Create `scripts/tests/test_file_dialog_proxy.mjs`; modify harness setup (NOT assertions) in `test_editor_save_race.mjs`, `test_editor_save_inflight.mjs`, `test_shortcut_save_fallthrough.mjs`, `test_editor_remote_transfer.mjs`, `test_editor_chooser_teardown.mjs`, `test_editor_save_as.mjs`, `test_editor_untitled.mjs` as needed — these suites drive `chooseFile`/`openForSave`; they gain a stub transport (`invoke` handlers for `open_file_chooser`/`cancel_file_chooser`/`focus_file_chooser` + a `listenOnCurrentWindow('chooser-resolved')` stub they can fire) in their shared harness sections, following the existing `listen: () => Promise.resolve(() => {})` idiom (`test_editor_chooser_teardown.mjs:240`).

**Interfaces:**
- Consumes: Task 1's command names and `chooser-resolved` payload `{ reqId, choice }`; Task 2's view (no longer called here).
- Produces: `chooseFile` resolving `{scope, path, entry} | null` where `scope` is the plain object the view put in the choice (`{id, kind, label, hostLabel, paneId, start}` — `openForOpen`/`openForSave` read only `kind`/`paneId`/`hostLabel` (`file-dialog.js:1149-1254`) and they KEEP reading the resolved value exactly as today). The scrim element: `div.tl-chooser-scrim` appended to `document.body`, plus `aria-hidden="true"` and the `inert` attribute on `#app` (the app root — confirm the id in index.html; if it differs, use the actual root container). New CSS class `.tl-chooser-scrim` in `file-dialog.css`: `position: fixed; inset: 0; background: var(--tl-dialog-scrim); z-index: 4000;` (above tl-dialog's 3000-band).

`chooseFile`'s new body, structurally:

```js
function chooseFile(options) {
  // activeChoice guards: UNCHANGED lines (same-mode share, cross-mode null,
  // plus: on same-mode share also invoke('focus_file_chooser') fire-and-forget)
  let resolveChoice;
  const promise = new Promise((r) => { resolveChoice = r; });
  const session = { promise, mode, pane: opts.pane || null, cancel: doCancel };
  activeChoice = session;
  const scrim = raiseScrim();          // scrim + inert, idempotent
  let unlisten = null;
  let myReqId = null;
  const settle = (choice) => {          // the once-latch
    if (session.settled) return;
    session.settled = true;
    try { if (unlisten) unlisten(); } finally { lowerScrim(scrim); }
    if (activeChoice === session) activeChoice = null;
    resolveChoice(choice);
  };
  function doCancel() {
    invoke('cancel_file_chooser').catch(() => {}).finally(() => settle(null));
  }
  (async () => {
    try {
      unlisten = await listenOnCurrentWindow('chooser-resolved', (event) => {
        const p = event && event.payload;
        if (!p || p.reqId !== myReqId) return;   // stale-event protection
        settle(p.choice || null);
      });
      myReqId = await invoke('open_file_chooser', { mode, filename: opts.filename || null, selectFilename: !!opts.selectFilename });
    } catch (error) {
      toastError(failTitle, String(error));
      settle(null);
    }
  })();
  return promise;
}
```

The listener attaches BEFORE the invoke (a resolution cannot outrun a listener that already exists); events arriving before `myReqId` is assigned are dropped by the `p.reqId !== myReqId` check only after `myReqId` is set — guard with `myReqId === null` → buffer one event and re-check after the invoke returns (the invoke's return and the event both originate in Rust after registry insert, but the event can win the race on a fast cancel; the buffer is the fix, and the proxy test pins it).

- [ ] **Step 1: Write `test_file_dialog_proxy.mjs` (failing).** Harness: vm sandbox loading ONLY `file-dialog.js` (+ stub `termlabServices.tauriClient` with scripted `invoke` and a capturable `listenOnCurrentWindow`). Checks: (1) same-mode second call returns the SAME promise object and fired `focus_file_chooser`; (2) cross-mode second call resolves null without any invoke; (3) scrim: raised before `open_file_chooser`'s invoke handler runs (assert ordering via a log array), lowered after resolution event, lowered after invoke REJECTION (the finally), and `#app` has `inert` removed both times; (4) stale `reqId` event ignored, correct one settles; (5) event-before-reqId-assigned: fire the resolution synchronously from inside the `open_file_chooser` stub before it returns — promise still settles correctly (the buffer); (6) `cancelForPane(pane)` on the pane the chooser is for → `cancel_file_chooser` invoked and promise settles null; on a DIFFERENT pane → nothing (existing semantics, `file-dialog.js:1271-1275`); (7) `openForOpen`/`openForSave` still consume `scope.kind`/`paneId`/`hostLabel` off the settled choice — feed a remote-scope choice through the stub transport and assert the same downstream invokes fire that today's suite asserts (crib the expected invoke sequence from `test_editor_remote_transfer.mjs`'s existing assertions, do not weaken them there).
- [ ] **Step 2: Run to verify failure.**
- [ ] **Step 3: Implement the proxy; delete the Task-2 bridge and every now-dead helper from `file-dialog.js` (the file should shrink to roughly: invoke/toast helpers, `activeChoice`, `chooseFile`, scrim helpers, `openForOpen`, `openForSave`, `cancelForPane`, exports). Check `resolveName` in `tauri-client.js:181-194`: register `chooser-resolved` in the EVENTS map only if unknown names don't pass through.**
- [ ] **Step 4: Adapt the seven consuming suites' harnesses** with the stub transport where their existing flows now cross it; run each: every behavioral assertion unchanged and green. Full suite green.
- [ ] **Step 5: Commit** — `git commit -m "Route chooseFile through the chooser window with a scrim-guarded proxy"`

### Task 4: `chooser.html`, runtime, and window CSS

**Files:**
- Create: `crates/termlab_tauri/frontend/chooser.html`, `crates/termlab_tauri/frontend/app/chooser-window-runtime.js`
- Modify: `crates/termlab_tauri/frontend/index.html` (remove `file-dialog-view.js` script tag — after this task only chooser.html loads it; `file-dialog-model.js` stays in BOTH, the proxy does not need it but keep load order stable in chooser.html: model before view), `crates/termlab_tauri/frontend/styles/design-system/components/file-dialog.css` (`.tl-filedlg` window-body context)
- Test: extend `scripts/tests/test_file_dialog.mjs` with runtime checks (the runtime is a thin module — test it in the same harness with a stub invoke)

**Interfaces:**
- Consumes: Task 1's `get_chooser_request`/`resolve_file_chooser`/`chooser_ready`; Task 2's `build(root, deps)`.
- Produces: the shipped window. No later task.

`chooser.html` head: copy `settings.html`'s stylesheet list (`settings.html:7-27`) trimmed to what the chooser uses — tokens-dark, tokens-light, fonts, base, button, input, scrollbar, dialog (overwrite confirm), picker (listing box base), file-dialog — plus the Windows/Linux custom-titlebar markup block copied from `settings.html`'s `#settings-titlebar` with ids renamed `chooser-titlebar*`. Scripts, in order: `app/core/tauri-client.js` boot (inline module, mirroring `settings.html`'s: create client, `configService.applyThemeCss` for appearance — the scout confirmed `data-tl-appearance` is never set by anything; use `applyThemeCss` exactly as `settings.html` does), `app/ui/tl-icon.js`, `app/ui/tl-dialog.js` (+ whatever globals `tl-dialog.js` requires — read its header; the keyboard-router global it uses must be loaded or stubbed the way `settings.html` resolves it — copy settings.html's script set for this, do not guess), `app/features/files/data-service.js`, `app/features/editor/file-dialog-model.js`, `app/features/editor/file-dialog-view.js`, `app/chooser-window-runtime.js`.

`chooser-window-runtime.js`:

```js
(function initChooserWindowRuntime(global) {
  'use strict';
  async function boot() {
    const client = global.termlabServices && global.termlabServices.tauriClient;
    const invoke = client.invoke.bind(client);
    let request;
    try { request = await invoke('get_chooser_request'); }
    catch (e) { await invoke('close_current_window').catch(() => {}); return; }
    const root = document.getElementById('chooser-root');
    const view = global.termlabFileDialogView.build(root, {
      data: global.termlabFilesData,
      mode: request.mode,
      filename: request.filename,
      selectFilename: request.selectFilename,
      parentWindowLabel: request.parentLabel,
      onResolve: (choice) => {
        invoke('resolve_file_chooser', { reqId: request.reqId, choice }).catch(() => {});
      },
    });
    await invoke('chooser_ready');
    view.focusInitial();
  }
  boot();
})(window);
```

(If no `close_current_window` command exists, use `window.__TAURI__.window.getCurrentWindow().close()` — check `commands.rs` first and use whichever exists.) The no-request path closes the window (spec: "a chooser with no question must not linger"). Escape and the window close button both land in the same place: Escape via the view's root listener → `onResolve(null)`; the OS close button via Task 1's `CloseRequested` hook → registry cancel. CSS: `.tl-filedlg` gains a window-context rule — `html, body { height: 100% }` scoped to chooser.html via a `body.tl-chooser-body` class; `.tl-filedlg { height: 100% }` so the listing flexes and the chrome rows stay fixed; only `.tl-filedlg__box` may scroll (this is already true; the floor guarantees the rest).

- [ ] **Step 1: Write the failing runtime checks** in `test_file_dialog.mjs`'s harness: stub `invoke` for `get_chooser_request` (returns a save-mode request), `chooser_ready`, `resolve_file_chooser`; load the runtime; assert build was called with `parentWindowLabel === request.parentLabel` (NOT any window's own label), `chooser_ready` invoked after the view exists, and a view resolution invokes `resolve_file_chooser` with `{reqId, choice}`. Failure path: `get_chooser_request` rejects → the close path is invoked, view never builds.
- [ ] **Step 2: Run to verify failure. Step 3: Implement (html + runtime + CSS). Step 4: full frontend suite + boundary script green; `node --check` new JS.**
- [ ] **Step 5: Commit** — `git commit -m "Add the chooser window shell and runtime"`

### Task 5: Integration sweep, checklist section I, spec sync

**Files:**
- Modify: `docs/superpowers/notes/light-editor-manual-checklist.md` (section I, numbering continues after section H's last step), `docs/superpowers/specs/2026-08-18-chooser-window-design.md` (sync), any loose end the sweep finds
- Test: full suites, both stacks

Steps:
- [ ] **Step 1: Sweep.** `cargo test --workspace` AND the full frontend suite AND the boundary script — green (only `tl-dialog.js:334`). Grep for leftovers: `tlDialog.open` must not appear in `file-dialog.js`; `findFooterButton` must not exist; `getCurrentWindowLabel` must not be called from `file-dialog-view.js`; `index.html` must not load `file-dialog-view.js`.
- [ ] **Step 2: Checklist section I** (continue numbering from H's step 91), in the established voice, covering: chooser opens as a real window centered on its parent; parent grays out and rejects clicks/typing; clicking the parent re-fronts the chooser; ⌘O with a chooser open re-fronts it, ⌘O over a save chooser does nothing; resize stops at the content-fit floor and nothing but the listing ever scrolls; size remembered across opens; two app windows with two simultaneous choosers operating independently; `[SSH]` remote scopes incl. mid-browse disconnect → failed-scope retry; overwrite confirm appears inside the chooser window; save-mode filename flow incl. Escape; parent window closed with chooser open → chooser closes, no orphan; both themes; **the row-height/window-proportion comfort judgment at the floor size is explicitly the human's**.
- [ ] **Step 3: Spec sync.** Amend the spec where implementation deliberately diverged, at minimum: request has no `start_dir` (the view resolves `$HOME` itself via the data-service, as today); outcome relayed as opaque JSON, not a typed `ChooserChoice`; scope serialization is the view's existing plain-scope shape (`kind`/`paneId`/`hostLabel`/`label`/`id`/`start`) — plus anything Tasks 1-4 changed under review. Spec and code must not disagree at branch end.
- [ ] **Step 4: Commit** — `git commit -m "Add chooser-window manual checklist section and sync the spec"`

# Index HTML Modularization Plan (2026-04-01)

## Goal
Reduce `crates/conch_tauri/frontend/index.html` from a monolithic page into maintainable, testable modules with clear ownership boundaries.

## Current Problems
- File combines CSS, app bootstrap, pane/tab lifecycle, plugin integration, menu wiring, and window event routing.
- High merge conflict risk because unrelated work lands in one file.
- Hard to debug regressions because side effects are spread across one giant script scope.

## Target End State
- `index.html` is a shell: DOM skeleton + external CSS/JS includes + minimal bootstrap call.
- Runtime logic moved into focused JS modules under `crates/conch_tauri/frontend/app/`.
- Styles split into themed CSS chunks under `crates/conch_tauri/frontend/styles/`.

## Phases

### Phase 1: CSS Extraction (now)
- Move inline `<style>` block out of `index.html` into `frontend/styles/app.css`.
- Load with `<link rel="stylesheet" href="styles/app.css" />`.
- No runtime behavior changes.

### Phase 2: CSS Logical Split (no behavior changes)
- Split `styles/app.css` into:
  - `styles/base.css` (tokens/reset/typography)
  - `styles/layout.css` (app shell, tab bar, split host)
  - `styles/tool-windows.css` (zones, strips, DnD overlays)
  - `styles/panels.css` (files, ssh, notification, bottom panel)
  - `styles/dialogs.css` (settings/vault/forms/modals)
  - `styles/plugin-widgets.css` (pw-* classes)
- Keep class names stable.

### Phase 3: Bootstrap + Shared State Module
- Create `frontend/app/bootstrap.js` and `frontend/app/state.js`.
- Move top-level mutable maps/IDs (tabs, panes, plugin view maps) into `state.js` exports.
- Keep API surface compatible with existing modules (`window.*` fallback during migration).

### Phase 4: Pane/Tab Runtime Split
- Move tab/pane lifecycle into dedicated modules:
  - `app/tab-manager.js`
  - `app/pane-manager.js`
  - `app/split-runtime.js`
- `index.html` inline script becomes only wiring calls.

### Phase 5: Plugin + Tool Window Orchestration Split
- Move plugin registration/listener logic from `index.html` into:
  - `app/plugin-runtime.js`
  - `app/tool-window-runtime.js`
- Keep `plugin-widgets.js` focused on rendering only.

### Phase 6: Menus, Shortcuts, and Host Event Routing
- Move menu action dispatcher + keyboard routing to:
  - `app/menu-actions.js`
  - `app/shortcut-runtime.js`
  - `app/window-events-runtime.js`
  - `app/dialog-runtime.js`

### Phase 7: Final Shell Cleanup
- Keep `index.html` limited to:
  - structure markup
  - CSS links
  - JS includes
  - one `bootstrap()` call

## Progress
- Completed: Phase 1 (`index.html` inline CSS extracted).
- Completed: Phase 2 (`app.css` split into logical files and linked from `index.html`):
  - `styles/base.css`
  - `styles/layout.css`
  - `styles/tool-windows.css`
  - `styles/panels.css`
  - `styles/plugin-widgets.css`
  - `styles/dialogs.css`
- Completed: Phase 3 (`frontend/app/state.js` + `frontend/app/bootstrap.js` created and `index.html` startup/state initialization wired through module-backed entry points with fallback compatibility).
- In Progress: Phase 4/5 cleanup (`app/split-runtime.js`, `app/pane-manager.js`, `app/tab-manager.js`, `app/plugin-runtime.js`, and `app/tool-window-runtime.js` introduced; pane helpers/drop/focus/close/split and core tab runtime primitives now routed via module factory wrappers; plugin docked view orchestration moved into `plugin-runtime.js`; tool-window/layout/settings/plugin-widget bootstrap and related event wiring moved into `tool-window-runtime.js`; `index.html` wrappers for moved pane/tab/plugin lifecycle APIs are strict delegators).
- Completed: Phase 7 shell conversion of `index.html` (main startup/business script moved into `frontend/app/main-runtime.js`; `index.html` now serves as DOM shell + CSS/JS includes + minimal global contextmenu guard).
- In Progress: Post-Phase 7 cleanup: split `app/main-runtime.js` into smaller orchestration modules (startup/theme/app-config, manager composition, and runtime wiring layers) so no single runtime file regresses into a new monolith.
- Completed in this pass: extracted startup concerns into `app/startup-runtime.js` (status/error handling, runtime dependency checks, terminal config load, theme load+CSS apply, app config apply).
- Completed in this pass: extracted startup state/runtime composition into `app/compose-runtime.js` (DOM refs, initial state, input runtime, terminal runtime initialization/fallback, manager delegate runtime initialization/fallback).
- Completed in this pass: extracted manager assembly (`paneManager` + `tabManager` construction and delegate registration) into `app/manager-compose-runtime.js`.
- Completed in this pass: extracted runtime orchestration block (`pluginRuntime`/`paneDnd`/tool-window init/drag-drop init) into `app/orchestration-runtime.js`.
- Also fixed: `manager-compose-runtime` now correctly writes plugin view sizes into `pluginViewSizeMemory` (not `pluginViewPaneById`).
- Completed in this pass: extracted remaining event/menu/dialog/shortcut/config wiring into `app/event-wiring-runtime.js`.
- `app/main-runtime.js` now delegates startup, composition, manager assembly, orchestration, and event wiring to dedicated runtimes.
- Completed in this pass: extracted residual wrapper/util layer (layout delegates + clipboard + command palette facade) into `app/bridge-runtime.js`.
- `app/main-runtime.js` now primarily coordinates runtime composition and startup flow.
- Completed: final tidy pass on `app/main-runtime.js` (removed dead locals/wrappers, preserved behavior).
- Next: transition to commit/review or address follow-up product questions.

## Guardrails
- No feature work mixed with modularization PRs.
- Small, reviewable PRs (one phase per PR).
- Preserve behavior after each phase.
- Manual smoke checks after each phase:
  - tab create/split/close
  - ssh/files tool windows
  - plugin panel rendering
  - plugin docked view open/drag/close
  - menu actions + shortcuts

## Suggested PR Sequence
1. `refactor(frontend): extract index inline css into styles/app.css`
2. `refactor(frontend): split app.css into logical style modules`
3. `refactor(frontend): add bootstrap + state modules`
4. `refactor(frontend): extract tab and pane runtime`
5. `refactor(frontend): extract plugin/tool-window orchestration`
6. `refactor(frontend): extract menu and event routing`
7. `refactor(frontend): trim index.html to shell bootstrap`

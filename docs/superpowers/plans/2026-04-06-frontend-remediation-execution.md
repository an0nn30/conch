# Front-End Remediation Execution Plan (Tauri 2 Desktop UI)

Status: Active execution artifact
Date: 2026-04-06
Scope: `crates/conch_tauri/frontend`

## Execution Defaults
1. Mode: Stabilize then refactor.
2. Behavior policy: Strict behavior preservation.
3. Architecture constraint: Keep current non-framework architecture.
4. Delivery strategy: Incremental and merge-safe.

## Guardrails
1. New or edited frontend runtime code must use `window.conchTauriClient` as the command/event boundary.
2. New plugin permission prompts must use `window.conchDialogService.confirmPluginPermissions`.
3. New global keyboard handlers must register through `window.conchKeyboardRouter`.
4. New layout persistence work must route through `window.conchLayoutService`.
5. No new global context-menu suppression outside terminal-specific surfaces.

## Workstreams
1. WS-A Runtime/Infra: `app/core`, startup/main/orchestration runtime wiring.
2. WS-B UI primitives/theming: shared dialog styles/tokens and settings window integration.
3. WS-C Settings: plugin permission prompt dedupe, plugin list style entropy reduction.
4. WS-D SSH/Files/Vault: layout persistence migration to shared service.
5. WS-E Verification: static scan and behavior checklist deltas.

## Phase Breakdown

### Phase 0: Baseline and Guardrails
- [x] Identify frontend root and high-risk coupling points.
- [x] Add explicit guardrails to this execution doc.
- [x] Static and runtime baseline snapshots captured (`docs/superpowers/plans/2026-04-06-frontend-static-baseline.md`, `docs/superpowers/plans/2026-04-06-frontend-runtime-baseline.md`) including startup/palette timing baselines and render-churn hotspot signals.

### Phase 1: Core Stabilization
- [x] Add `app/core/tauri-client.js`.
- [x] Add `app/core/keyboard-router.js`.
- [x] Add `app/core/dialog-service.js`.
- [x] Add `app/core/layout-service.js`.
- [x] Add `app/core/config-service.js`.
- [x] Load services from `index.html` before dependent runtime scripts.
- [x] Wire `main-runtime.js` to consume `tauri-client` and publish shared services.
- [x] Wire startup/config paths to shared config service.

### Phase 2: Settings and UI-System Refactor
- [x] Migrate plugin permission confirmation in settings to shared dialog service.
- [x] Migrate plugin permission confirmation in command palette to shared dialog service.
- [x] Replace inline style-heavy settings plugin list rows with class-based styles.
- [x] Add shared plugin permission and settings plugin row style classes in `styles/dialogs.css`.
- [x] Extract settings constants/search-index into `app/features/settings/constants.js`.
- [x] Extract settings runtime load/plugin action flows into `app/features/settings/data-service.js`.
- [x] Extract settings sidebar/search renderer into `app/features/settings/sidebar.js` and delegate from `settings.js`.
- [x] Extract settings appearance section renderer into `app/features/settings/sections-appearance.js` and delegate from `settings.js`.
- [x] Extract settings basic section renderers (`advanced`, `files`) into `app/features/settings/sections-basic.js` and delegate from `settings.js`.
- [x] Extract settings keyboard section renderer into `app/features/settings/sections-keyboard.js` and delegate from `settings.js`.
- [x] Extract settings section renderers (`terminal`, `shell`, `cursor`) into `app/features/settings/sections-terminal.js` and delegate from `settings.js`.
- [x] Extract settings plugin section renderer into `app/features/settings/plugins-section.js`.
- [x] Align standalone settings window with shared style/system scripts (`settings-window.css` now limited to window-specific titlebar/layout chrome; shared settings/tokens/forms/styles sourced from `styles/{base,dialogs}.css`).

### Phase 3: SSH/Files/Vault Extraction Prep
- [x] Migrate files panel layout save/restore calls to shared layout service.
- [x] Migrate ssh panel layout save/restore calls to shared layout service.
- [x] Wire tool-window runtime save/restore through shared layout service and pass service to panels.
- [x] Extract SSH data access operations into `app/features/ssh/data-service.js` and wire refresh/export/import/session/tunnel callsites.
- [x] Extract SSH context-menu primitive into `app/features/ssh/context-menu.js` and delegate from `ssh-panel.js`.
- [x] Extract SSH auth prompt overlays into `app/features/ssh/auth-prompts.js` and delegate from `ssh-panel.js`.
- [x] Extract SSH export dependency prompt overlay into `app/features/ssh/dependency-prompt.js` and delegate from `ssh-panel.js`.
- [x] Extract SSH connection form orchestration into `app/features/ssh/connection-form.js` and delegate from `ssh-panel.js`.
- [x] Extract SSH folder/delete dialogs into `app/features/ssh/dialogs.js` and delegate from `ssh-panel.js`.
- [x] Extract Files data access operations into `app/features/files/data-service.js` and wire panel data/transfer callsites.
- [x] Extract Files pane-state ownership helpers into `app/features/files/pane-store.js` and wire `files-panel.js` (state shape, follow-path, sort/ext helpers).
- [x] Extract Files pane rendering/menu view layer into `app/features/files/pane-view.js` and delegate from `files-panel.js`.
- [x] Extract Files navigation/actions into `app/features/files/actions.js` and delegate from `files-panel.js`.
- [x] Extract Files transfer-progress/toast orchestration into `app/features/files/transfers.js` and delegate from `files-panel.js`.
- [x] Extract Vault data access operations into `app/features/vault/data-service.js` and wire `vault.js` command callsites.
- [x] Extract Vault setup/unlock dialogs into `app/features/vault/dialogs.js` and delegate from `vault.js`.
- [x] Extract Vault account form flow into `app/features/vault/account-form.js` and delegate from `vault.js`.
- [x] Extract Vault section rendering into `app/features/vault/sections.js` and keep `vault.js` focused on dialog orchestration.
- [x] Full module extraction (`store/view/actions`) for files/ssh/vault (Files/Vault complete; SSH store/action/view modules landed via `app/features/ssh/{store,actions,view}.js` with panel delegation across server/session/tunnel rendering and action callsites).

### Phase 4: UX Consistency and Accessibility Hardening
- [x] Remove global context-menu suppression from `index.html`.
- [x] Keep terminal-specific context-menu handling in terminal/runtime surfaces.
- [x] Route migrated escape/shortcut handlers through `keyboard-router` in startup/context/shortcut/dialog runtimes.
- [x] Route settings shortcut-recording key capture through shared keyboard handler (router-scoped registration only; document fallback removed).
- [x] SSH/Vault/Settings modal and standalone escape handlers migrated to keyboard-router scoped handlers with dialog ARIA semantics; expanded to plugin dialogs, tunnel manager overlays, keygen dialogs, and update-restart dialog surfaces.
- [x] Added roles/keyboard activation to custom SSH and Files context-menu controls, Files rows, sortable headers, and tunnel context-menu items.
- [x] Added roles/tab stops/Enter+Space activation for Settings and Vault sidebar custom items plus focus-visible styling for custom interactive controls.
- [x] Full keyboard contract normalization and ARIA sweep across migrated feature dialogs.

### Phase 5: Cleanup and Integration
- [x] Command palette indexing cache + invalidation optimization.
- [~] Dead-path deletion progressed (removed legacy inline SSH server/session/tunnel and auth dialog render fallbacks, Vault setup/unlock/section fallbacks, Settings sidebar/basic/terminal/appearance/keyboard fallback render paths, duplicate plugin-permission fallback dialog implementations in Settings/Command Palette, router-fallback document key handlers in startup/context/shortcut/dialog/clipboard/titlebar runtimes, and unused helper state in settings/tunnel modules); broader cleanup pending.
- [x] Boundary script expanded to guard raw layout persistence callsites (`save_window_layout`/`get_saved_layout`), inline `settings.html` style regressions, and direct document keydown listeners outside `keyboard-router`.
- [~] Final integration QA checklist prepared (`docs/superpowers/plans/2026-04-06-frontend-regression-checklist.md`) with automated checks complete; manual scenario sign-off pending.

## Implemented File Set (This Execution Slice)
- `crates/conch_tauri/frontend/index.html`
- `crates/conch_tauri/frontend/settings.html`
- `crates/conch_tauri/frontend/app/main-runtime.js`
- `crates/conch_tauri/frontend/app/startup-runtime.js`
- `crates/conch_tauri/frontend/app/config-runtime.js`
- `crates/conch_tauri/frontend/app/orchestration-runtime.js`
- `crates/conch_tauri/frontend/app/tool-window-runtime.js`
- `crates/conch_tauri/frontend/app/context-menu-runtime.js`
- `crates/conch_tauri/frontend/app/dialog-runtime.js`
- `crates/conch_tauri/frontend/app/shortcut-runtime.js`
- `crates/conch_tauri/frontend/app/command-palette-runtime.js`
- `crates/conch_tauri/frontend/app/window-events-runtime.js`
- `crates/conch_tauri/frontend/app/panels/settings.js`
- `crates/conch_tauri/frontend/app/panels/files-panel.js`
- `crates/conch_tauri/frontend/app/panels/ssh-panel.js`
- `crates/conch_tauri/frontend/app/panels/vault.js`
- `crates/conch_tauri/frontend/app/panels/plugin-widgets.js`
- `crates/conch_tauri/frontend/app/panels/tunnel-manager.js`
- `crates/conch_tauri/frontend/styles/dialogs.css`
- `crates/conch_tauri/frontend/styles/panels.css`
- `crates/conch_tauri/frontend/app/core/tauri-client.js`
- `crates/conch_tauri/frontend/app/core/keyboard-router.js`
- `crates/conch_tauri/frontend/app/core/dialog-service.js`
- `crates/conch_tauri/frontend/app/core/layout-service.js`
- `crates/conch_tauri/frontend/app/core/config-service.js`
- `crates/conch_tauri/frontend/app/core/keygen.js`
- `crates/conch_tauri/frontend/app/features/settings/constants.js`
- `crates/conch_tauri/frontend/app/features/settings/data-service.js`
- `crates/conch_tauri/frontend/app/features/settings/search.js`
- `crates/conch_tauri/frontend/app/features/settings/sidebar.js`
- `crates/conch_tauri/frontend/app/features/settings/sections-appearance.js`
- `crates/conch_tauri/frontend/app/features/settings/sections-basic.js`
- `crates/conch_tauri/frontend/app/features/settings/sections-keyboard.js`
- `crates/conch_tauri/frontend/app/features/settings/sections-terminal.js`
- `crates/conch_tauri/frontend/app/features/settings/plugins-section.js`
- `crates/conch_tauri/frontend/app/features/ssh/data-service.js`
- `crates/conch_tauri/frontend/app/features/ssh/store.js`
- `crates/conch_tauri/frontend/app/features/ssh/actions.js`
- `crates/conch_tauri/frontend/app/features/ssh/view.js`
- `crates/conch_tauri/frontend/app/features/ssh/context-menu.js`
- `crates/conch_tauri/frontend/app/features/ssh/auth-prompts.js`
- `crates/conch_tauri/frontend/app/features/ssh/dependency-prompt.js`
- `crates/conch_tauri/frontend/app/features/ssh/connection-form.js`
- `crates/conch_tauri/frontend/app/features/ssh/dialogs.js`
- `crates/conch_tauri/frontend/app/features/files/data-service.js`
- `crates/conch_tauri/frontend/app/features/files/pane-store.js`
- `crates/conch_tauri/frontend/app/features/files/actions.js`
- `crates/conch_tauri/frontend/app/features/files/pane-view.js`
- `crates/conch_tauri/frontend/app/features/files/transfers.js`
- `crates/conch_tauri/frontend/app/features/vault/data-service.js`
- `crates/conch_tauri/frontend/app/features/vault/dialogs.js`
- `crates/conch_tauri/frontend/app/features/vault/account-form.js`
- `crates/conch_tauri/frontend/app/features/vault/sections.js`
- `crates/conch_tauri/frontend/styles/settings-window.css`
- `crates/conch_tauri/frontend/app/bridge-runtime.js`
- `scripts/check_frontend_boundaries.sh`
- `scripts/snapshot_frontend_static_baseline.sh`
- `scripts/snapshot_frontend_runtime_baseline.sh`
- `docs/superpowers/plans/2026-04-06-frontend-static-baseline.md`
- `docs/superpowers/plans/2026-04-06-frontend-runtime-baseline.md`
- `docs/superpowers/plans/2026-04-06-frontend-regression-checklist.md`

## Open Risks
1. Full decomposition of `settings.js` remains outstanding.
2. Command strings are still present in many legacy call sites; boundary is now available and wired for incremental migration.
3. Standalone settings style alignment is complete; remaining UI-style risk is broader cross-surface token normalization outside settings-only scope.
4. Manual end-to-end regression scenarios are still pending completion in the Phase 5 checklist artifact.

## Next Execution Targets
1. Extract `settings.js` into `features/settings/{store,sections,search-index,actions,renderers}`.
2. Continue `settings.js` decomposition to reduce single-file ownership and enable dead-path deletion completion in Phase 5.
3. Execute checklist-driven manual regression verification for startup, tabs/splits, ssh/files/vault/settings/plugin flows using `docs/superpowers/plans/2026-04-06-frontend-regression-checklist.md`.
4. Close Phase 5 by final dead-path cleanup plus checklist sign-off package.

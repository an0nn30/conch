# Front-End Regression Checklist (Phase 5 Integration)

Date: 2026-04-06
Scope: `crates/conch_tauri/frontend`
Linked plan: `docs/superpowers/plans/2026-04-06-frontend-remediation-execution.md`

## Automated Validation (Completed)
- [x] `node --check` passes for migrated dialog/keyboard/runtime modules touched in remediation.
- [x] `scripts/check_frontend_boundaries.sh /Users/dustin/projects/rusty_conch_2` passes.
- [x] Direct keydown listener drift check passes (document keydown listeners centralized in `app/core/keyboard-router.js`).
- [x] Baseline snapshots refreshed:
  - `docs/superpowers/plans/2026-04-06-frontend-static-baseline.md`
  - `docs/superpowers/plans/2026-04-06-frontend-runtime-baseline.md`

## Manual Acceptance Scenarios
Status key: `[ ] pending`, `[x] verified`

1. Startup and restore
- [ ] App boot has no startup errors.
- [ ] Prior layout restores correctly.
- [ ] Zen mode restoration remains correct.

2. Terminal workflows
- [ ] Create tab.
- [ ] Split pane.
- [ ] Close pane/tab.
- [ ] Rename tab.
- [ ] Resize pane.
- [ ] Active pane tracking remains correct.

3. SSH workflows
- [ ] Quick connect.
- [ ] Saved server connect.
- [ ] Proxy jump.
- [ ] Import/export.
- [ ] Host-key prompt behavior.
- [ ] Password prompt behavior.

4. Files workflows
- [ ] Local browse.
- [ ] Remote browse.
- [ ] Follow-path sync.
- [ ] Upload/download progress, cancel, completion.

5. Settings workflows
- [ ] Open/close behavior.
- [ ] Sidebar search and jump.
- [ ] Apply/cancel behavior.
- [ ] Plugin enable/disable and permission prompt.

6. Plugin workflows
- [ ] Menu actions.
- [ ] Panel registration/removal.
- [ ] Widget event round-trips.
- [ ] Plugin dialogs (form/prompt/confirm).

7. Keyboard and focus
- [ ] Escape semantics across overlays/dialogs.
- [ ] Command palette shortcuts and navigation.
- [ ] Menu shortcuts.
- [ ] Focus return after overlays close.

8. Accessibility checks
- [ ] Focus visibility on custom controls.
- [ ] Keyboard-only operation for core flows.
- [ ] Role/label coverage on migrated custom controls.

9. Regression contract
- [ ] All Phase 0 baseline flows pass unchanged unless explicitly marked as approved consistency fix.

## Current Sign-Off State
Final integration sign-off is **not complete** yet. Automated checks are complete; manual scenario verification is pending.

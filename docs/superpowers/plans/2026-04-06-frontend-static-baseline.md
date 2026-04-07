# Frontend Static Baseline Snapshot

- Date: 2026-04-06 19:09:30 CDT
- Scope: `crates/conch_tauri/frontend/app`
- Method: static source scan (no runtime timing instrumentation)

## Summary

- JavaScript files: 75
- Total JS LOC: 18104
- `document.addEventListener('keydown')` occurrences: 1
- `contextmenu` occurrences (app + html entrypoints): 12
- `ssh-overlay` references: 41
- Raw layout invoke callsites (`save_window_layout/get_saved_layout`): 9

## Largest JS Files (LOC)

| LOC | File |
|---:|---|
| 1468 | `crates/conch_tauri/frontend/app/panels/settings.js` |
| 1008 | `crates/conch_tauri/frontend/app/panels/plugin-widgets.js` |
| 993 | `crates/conch_tauri/frontend/app/layout/tool-window-manager.js` |
| 981 | `crates/conch_tauri/frontend/app/panels/ssh-panel.js` |
| 656 | `crates/conch_tauri/frontend/app/panels/tunnel-manager.js` |
| 607 | `crates/conch_tauri/frontend/app/panels/files-panel.js` |
| 576 | `crates/conch_tauri/frontend/app/panels/vault.js` |
| 530 | `crates/conch_tauri/frontend/app/ui/titlebar.js` |
| 472 | `crates/conch_tauri/frontend/app/tab-manager.js` |
| 437 | `crates/conch_tauri/frontend/app/command-palette-runtime.js` |
| 415 | `crates/conch_tauri/frontend/app/main-runtime.js` |
| 398 | `crates/conch_tauri/frontend/app/features/ssh/connection-form.js` |
| 392 | `crates/conch_tauri/frontend/app/layout/pane-dnd.js` |
| 363 | `crates/conch_tauri/frontend/app/core/keygen.js` |
| 344 | `crates/conch_tauri/frontend/app/tool-window-runtime.js` |
| 344 | `crates/conch_tauri/frontend/app/shortcut-runtime.js` |
| 287 | `crates/conch_tauri/frontend/app/features/vault/account-form.js` |
| 286 | `crates/conch_tauri/frontend/app/features/settings/sections-terminal.js` |
| 279 | `crates/conch_tauri/frontend/app/features/settings/plugins-section.js` |
| 270 | `crates/conch_tauri/frontend/app/menu-actions.js` |

## Notes

- This snapshot is intended for remediation tracking and architectural drift detection.
- Runtime startup/palette baseline metrics are tracked in `docs/superpowers/plans/2026-04-06-frontend-runtime-baseline.md`.

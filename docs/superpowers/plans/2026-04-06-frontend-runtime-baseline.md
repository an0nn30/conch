# Frontend Runtime Baseline Snapshot

- Date: 2026-04-07T00:09:30.024Z
- Scope: `crates/conch_tauri/frontend/app`
- Method: simulated runtime microbench with mocked Tauri/document dependencies (agent-run reproducible script)

## Startup Metrics (ms)

| Metric | Samples | Avg | P95 | Min | Max |
|---|---:|---:|---:|---:|---:|
| `startup.loadTerminalConfig` | 30 | 0.017 | 0.016 | 0.001 | 0.422 |
| `startup.loadTheme` | 30 | 0.006 | 0.011 | 0.001 | 0.085 |
| `startup.applyAppConfig` | 15 | 0.013 | 0.127 | 0.002 | 0.127 |

## Command Palette Metrics (ms)

| Metric | Samples | Avg | P95 | Min | Max | Commands | Invoke Calls |
|---|---:|---:|---:|---:|---:|---:|---:|
| `commandPalette.buildCommands` | 40 | 0.010 | 0.016 | 0.003 | 0.197 | 8 | 160 |

## Render Churn Hotspot Signals (Static Heuristics)

| File | LOC | `render*(` calls | `.addEventListener(` | `.innerHTML =` |
|---|---:|---:|---:|---:|
| `crates/conch_tauri/frontend/app/panels/files-panel.js` | 608 | 10 | 6 | 2 |
| `crates/conch_tauri/frontend/app/panels/ssh-panel.js` | 982 | 17 | 12 | 2 |
| `crates/conch_tauri/frontend/app/panels/vault.js` | 577 | 14 | 5 | 3 |
| `crates/conch_tauri/frontend/app/panels/settings.js` | 1469 | 40 | 13 | 2 |

## Notes

- These numbers are a remediation baseline for trend comparison, not end-user wall-clock startup timings.
- Use this alongside manual QA for full desktop rendering/path correctness checks.

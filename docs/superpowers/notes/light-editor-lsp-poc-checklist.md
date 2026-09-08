# Light Editor LSP — POC Acceptance Checklist

> **Status: automated evidence recorded; manual matrices pending owner — the final
> acceptance commit (the plan's Step 7, `docs: record light editor LSP POC
> verification`) follows the owner's hands-on pass.** This document is committed
> as `docs: record automated POC acceptance evidence` because the plan reserves
> the Step 7 message for the point at which *every* required result, manual
> included, has been recorded.

Acceptance evidence for
[`docs/superpowers/plans/2026-08-24-light-editor-lsp-poc.md`](../plans/2026-08-24-light-editor-lsp-poc.md)
Task 16. This checklist is evidence, not a substitute for the tests themselves.

## Run header

| Field | Value |
|---|---|
| Date | 2026-08-27 |
| Machine | arm64 macOS, Darwin 25.6.0 (Apple Silicon) |
| Branch | `codex/lsp-completion` |
| Branch tip at run time | `303411c8ccf7a59a2dc8ce2c60c3164212f2bd70` (`303411c`) |
| App build identifier | `dist/TermLab.app` built from `303411c`; `CFBundleShortVersionString` 3.0.0, `CFBundleVersion` 3.0.0-rc.2 |
| App output path | `dist/TermLab.app` — the plan's Steps 2/7 say `build/TermLab.app`; the Makefile has always emitted to `dist/`, and the plan itself allows "use the actual app output path printed by `make app`" |
| Bundled servers | node 24.19.0, typescript-language-server 6.0.0, typescript 6.0.3, rust-analyzer 2026-08-24 |

## Automated evidence

Every command below was run verbatim from the repository root on the date above.
Two commands exit non-zero. Both are recorded as failures with their scope, not
described as passes.

| # | Command | Exit | Literal result | Verdict |
|---|---|---|---|---|
| 1 | `cargo fmt --all -- --check` | **1** | 177 diff hunks across 35 files | **FAIL — see "Formatting" below.** 170 hunks / 33 files are pre-existing on `main`; this branch adds 9 hunks in 3 files |
| 2 | `cargo test --workspace` | 0 | `1497 passed; 0 failed; 4 ignored` across 18 test binaries | PASS |
| 3 | `npm run build:vendor --prefix crates/termlab_tauri/frontend` | 0 | `vendor: wrote vendor/codemirror/codemirror.js` / `vendor check: 68 exports present` | PASS |
| 4 | `for test_file in scripts/tests/test_*.mjs; do node "$test_file"; done` | 0 | 63 files run, 63 exited 0, 0 failed | PASS |
| 5 | `scripts/lsp/fetch-macos-arm64.sh --verify-only packaging/lsp/dist/arm64` | 0 | `receipt: verified 144 files against …/packaging/lsp/dist/manifest.json` | PASS |
| 6 | `make app` | 0 | `Built dist/TermLab.app` | PASS |
| 7 | `scripts/lsp/smoke-macos-arm64.sh dist/TermLab.app` | 0 | `packaged LSP smoke test passed for …/dist/TermLab.app` | PASS |
| 8 | `codesign --verify --deep --strict --verbose=2 dist/TermLab.app` | 0 | `valid on disk` / `satisfies its Designated Requirement` | PASS |

Two additional gates belonging to this branch, which postdate the plan text:

| # | Command | Exit | Literal result | Verdict |
|---|---|---|---|---|
| 9 | `scripts/check_frontend_boundaries.sh` | **1** | `disallowed direct document keydown usage found outside keyboard-router: ./crates/termlab_tauri/frontend/app/ui/tl-dialog.js:334` | **FAIL — pre-existing.** `main` fails identically, same single violation, same line; no branch commit touches that file |
| 10 | `python3 scripts/tests/test_lsp_receipt.py` | 0 | `lsp receipt: all checks passed` (23 checks) | PASS |

### Smoke detail (command 7)

Both servers launched from inside the signed bundle with `PATH` scrubbed to
`/usr/bin:/bin:/usr/sbin:/sbin`, proving no host toolchain fallback:

```
lsp-smoke: node executable is arm64 and bundled: Contents/Resources/lsp/arm64/node/bin/node
lsp-smoke: rust-analyzer executable is arm64 and bundled: Contents/Resources/lsp/arm64/rust-analyzer/rust-analyzer
lsp-smoke: launching typescript-language-server with PATH=/usr/bin:/bin:/usr/sbin:/sbin
smoke: initialize answered by server
lsp-smoke: typescript-language-server answered initialize and shut down cleanly
lsp-smoke: launching rust-analyzer with PATH=/usr/bin:/bin:/usr/sbin:/sbin
smoke: initialize answered by rust-analyzer 0.3.3025-standalone (5c156cdfb0 2026-08-23)
lsp-smoke: rust-analyzer answered initialize and shut down cleanly
```

### Formatting (command 1) — recorded, not waived

`cargo fmt --all -- --check` exits 1. Measured against `main` as the baseline:

| Scope | Hunks | Files |
|---|---|---|
| Current branch | 177 | 35 |
| `main` (pre-existing) | 170 | 33 |
| **Attributable to this branch** | **9** | **3** |

The branch-attributable hunks are all in LSP files this branch authored:

- `crates/termlab_tauri/src/lib.rs` — 2 hunks
- `crates/termlab_tauri/src/lsp/catalog.rs` — 6 hunks
- `crates/termlab_tauri/src/lsp/commands.rs` — 1 hunk

These predated Task 15 (they arrived with the files in Tasks 1–14; Task 15's own
edits to `catalog.rs` were kept at that file's existing 6-hunk baseline). They
were pure formatting, no behavior.

**RESOLVED** in the formatting commit that follows this evidence commit: the
three files were formatted (`rustfmt --edition 2024`, applied to those files
only), `cargo fmt --all -- --check` now reports 168 hunks (all pre-existing on
`main`; the branch total sits 2 below `main`'s 170 because branch edits had
already incidentally cleaned 2 pre-existing hunks elsewhere) — zero hunks in
branch-authored files — and
`cargo test -p termlab_tauri` stayed at 961 passed / 0 failed. The remaining
hunks are `main`'s pre-existing dirt, tracked as a separate repo-wide cleanup.

### Warnings observed (recorded separately, per the plan)

Build warnings during `cargo test --workspace`, none newly introduced by
Task 15 (the `termlab_tauri` lib count has held at its baseline through this
branch's packaging work):

```
termlab_core   (lib) 1 warning
termlab_plugin (lib) 4 warnings
termlab_tauri  (lib) 16 warnings
termlab_tauri  (lib test) 10 warnings (9 duplicates)
```

---

## Manual matrices — PENDING OWNER

Nothing below has been executed. Each scenario needs a hands-on pass on Apple
Silicon with the app built above. Check the box and replace the Result line.

### Step 3 — TypeScript / JavaScript matrix

In a nested npm workspace on Apple Silicon:

- [ ] Open a file and confirm editing is immediate before any project choice.
      **Result: PENDING — owner**
- [ ] Choose between package and workspace roots, trust the selected root, and
      verify the choice persists.
      **Result: PENDING — owner**
- [ ] Verify automatic and manual completion, hover, signature help, F12,
      back/forward, diagnostics, Problems navigation, and Save As transfer.
      **Result: PENDING — owner**
- [ ] Open the same canonical file from a popped-out window and confirm the
      existing owner is focused.
      **Result: PENDING — owner**
- [ ] Revoke trust and confirm the editor remains usable while the server stops.
      > **Matrix note (trust-revocation group presentation).** Watch the Problems
      > groups here. The session-keyed group is deleted by the `stopped` status,
      > but the per-document `untrusted` statuses that follow carry no session id,
      > so they can re-create a group for the same project under its
      > adapter-keyed identity. Decide whether "revoked project still listed as
      > untrusted" is the presentation you want, or whether those post-stop
      > statuses should be suppressed in the Problems store. See Open Decision 5.
      **Result: PENDING — owner**
- [ ] Temporarily move host `node` / TypeScript tools off `PATH` and repeat
      initialization.
      > Automated evidence already covers the scrubbed-`PATH` case from inside
      > the signed bundle (command 7). This step confirms the same in the live app.
      **Result: PENDING — owner**

### Step 4 — Rust matrix

In a Cargo workspace with multiple member crates:

- [ ] Verify both crate and workspace candidates appear and the nested
      remembered root wins.
      **Result: PENDING — owner**
- [ ] Trust and start rust-analyzer.
      **Result: PENDING — owner**
- [ ] Exercise all five IDE interactions and workspace-wide Problems.
      **Result: PENDING — owner**
- [ ] Edit emoji / non-ASCII text before a diagnostic and verify ranges remain
      aligned.
      **Result: PENDING — owner**
- [ ] Close all attached documents, wait over two minutes, and confirm the
      server shuts down.
      **Result: PENDING — owner**
- [ ] Reopen and confirm a new shared session starts.
      **Result: PENDING — owner**

### Step 5 — Failure and plain-editor paths

Verify each state below. In every case confirm local open/save/close, dirty
guards, Vim mode, and remote editor behavior still work.

- [ ] Untrusted. **Result: PENDING — owner**
- [ ] Denied. **Result: PENDING — owner**
- [ ] Disabled. **Result: PENDING — owner**
- [ ] Server missing. **Result: PENDING — owner**
- [ ] Startup timeout. **Result: PENDING — owner**
- [ ] Crash. **Result: PENDING — owner**
- [ ] Malformed response. **Result: PENDING — owner**
- [ ] Unsupported curated language. **Result: PENDING — owner**

### Step 5a — manual checks still open

The Task 12/13 review items I-2, L-1, L-2, N-2, N-3 and L-3 are **closed in
code** by `7730f8e` and `f136b5a` and need no manual sign-off. What remains:

- [ ] Verify popped-out Problems activation focuses the owner and opens the file.
      > **Matrix note (popped-out range degradation).** A popped-out Problems
      > window opens the file *without* selecting the range: a panel host owns no
      > editor, so `problems-navigation.js` falls back to the `open-in-editor`
      > host action, which carries a path and no range. This is a recorded
      > degradation pending a `HOST_ACTION_EVENTS` protocol extension, not a new
      > bug. Widening that closed action list is a panel-host protocol change,
      > deliberately out of Task 12's scope.
      **Result: PENDING — owner**
- [ ] Judge the definition chooser's context previews in practice.
      > **N-1, by design.** A row shows the target LINE only when this window
      > already has that file open, and the containing directory otherwise. The
      > spec (design doc line 558) asks for a preview on every row; closing that
      > needs either a preview field on the `lsp_definition` payload (Rust) or a
      > bounded lazy read for the highlighted row only. See Open Decision 3.
      **Result: PENDING — owner**
- [ ] Judge cross-window Back behavior in practice.
      > **Matrix note (Back consumes cross-window entries).** Back toward a
      > document another WINDOW owns consumes its history entry: that window is
      > focused and the step completes. Deliberate — leaving the entry would make
      > every further Back focus the same window forever — but it is a one-line
      > flip in `lsp-navigation.js`'s `step` if it feels wrong in practice.
      **Result: PENDING — owner**

---

## Open decisions for the owner

1. **Reserved-close finding 5.** Should a controlled session stop after a
   reserved-close failure restart the server for the session's surviving
   documents, or leave them detached (current pinned behavior)?
2. **Release wiring.** CI never stages LSP resources: shipped DMGs have an empty
   `lsp/` (fails closed). Needs a release-pipeline decision, including the
   x86_64 slice of the universal build.
3. **N-1 chooser previews.** Which payload choice — a preview field on the
   `lsp_definition` payload, or a bounded lazy read for the highlighted row only?
4. **Bundle size.** 176 MB for two languages on one architecture; doubles for
   universal.
5. **Trust-revocation group presentation.** Should post-stop `untrusted`
   statuses be suppressed in the Problems store, or is "revoked project still
   listed as untrusted" the intended presentation?

---

## POC acceptance gate

Reproduced from the plan. "Automated" means the evidence above already
demonstrates it; "manual" means it awaits the matrices.

| Gate bullet | Supported by |
|---|---|
| A local TypeScript/JavaScript file and a local Rust file can use completion, diagnostics, hover, signature help, and go-to-definition from bundled servers with a scrubbed `PATH`. | **Partial.** Automated evidence proves both bundled servers initialize and shut down under a scrubbed `PATH` from inside the signed bundle (cmd 7), and the per-feature JS suites pass (cmd 4). The five live IDE interactions await Steps 3–4. |
| Ambiguous nested roots are offered without blocking file editing; remembered nested choices take precedence. | **Manual** — Steps 3 and 4. |
| No language server process starts without explicit trust, and revocation works without closing editor buffers. | **Manual** — Steps 3 and 5. |
| A canonical local URI has only one independently editable owner across all TermLab windows, including open and Save As races. | **Partial.** `test_lsp_editor_lifecycle.mjs`, `test_editor_save_race.mjs` and `test_editor_save_as.mjs` pass (cmd 4); cross-window behavior awaits Step 3. |
| Diagnostics appear both inline and in the app-wide Problems tool window and are removed by replacement/close/session-stop events. | **Partial.** `test_lsp_diagnostics.mjs` and `test_problems_panel.mjs` pass (cmd 4), and session-stop removal is covered by `7730f8e`'s Rust tests (cmd 2); live behavior awaits Steps 3–4. |
| Navigation history works across files and windows. | **Partial.** `test_lsp_navigation.mjs` passes (cmd 4); cross-window Back awaits Step 5a. |
| Remote buffers never attach to LSP and continue to behave as before. | **Partial.** `test_editor_remote_transfer.mjs` passes (cmd 4); live remote editing awaits Step 5. |
| The signed arm64 `.app` contains and launches only the pinned resource executables. | **Automated — MET.** Commands 5, 6, 7, 8: receipt verifies 144 files, both executables are arm64 Mach-O inside `Contents/Resources`, they answer LSP under a scrubbed `PATH`, and the signature is valid. |
| Full Rust and frontend test suites pass with no new warnings or regressions. | **MET** (as of the formatting commit after the evidence commit). `cargo test --workspace` 1497 passed / 0 failed; 63 JS files pass; zero branch-attributable fmt hunks remain (168 total, all pre-existing on `main` — see "Formatting" above). |

### Shippability caveat

Independent of the gate: the release pipeline does not stage these resources, so
today's shipped DMGs contain an empty `lsp/` and language features are silently
unavailable (failing closed) in released builds. Open Decision 2.

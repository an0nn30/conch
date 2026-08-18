# File Dialog Redesign Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Rebuild the file chooser's presentation — places sidebar, Name/Size/Modified columns with click-to-sort, IntelliJ icons, comfortable spacing — with zero behavior change to open/save semantics.

**Architecture:** Three additive layers: pure model formatters/sort (Task 1), the DOM restructure in `file-dialog.js` reusing every existing handler (Task 2), the CSS rewrite + manual checklist (Task 3). The behavior contract is the existing test suites; DOM class changes update selectors deliberately, never assertions.

**Tech Stack:** existing stack; zero new assets (vendored `folder.svg`/`file.svg`/`newFolder.svg` + `tlIcon` helper).

**Spec:** `docs/superpowers/specs/2026-08-18-file-dialog-redesign-design.md`

## Global Constraints

- Branch `feat/file-dialog-redesign`; NEVER commit to main; NO Co-Authored-By trailers (repo CLAUDE.md).
- Tokens only in CSS. Keyboard behavior byte-for-byte. All pinned behaviors survive: mode-refusal, teardown cancel, double-Enter guard, per-window session filter, clean hostLabel routing (the `(pane N)` suffix stays display-only), failed-scope retry.
- Existing suites are the contract: `test_file_dialog.mjs` (31), `test_editor_chooser_teardown.mjs` (6), the dialog harnesses inside `test_editor_save_as.mjs`/`test_editor_untitled.mjs`. Selector updates are deliberate and listed; ASSERTION changes are forbidden without a named reason in the report.
- `sortEntries()` with no args must behave exactly as today (other callers + tests depend on it).
- Discriminating fixtures; mutation-prove the sort-header wiring and one formatter boundary. Trace every mechanism claim; the code wins.

### Task 1: Model — formatters and parameterized sort

**Files:** modify `app/features/editor/file-dialog-model.js`; test `scripts/tests/test_file_dialog_model.mjs` (extend).
**Interfaces produced:** `formatSize(bytes)`, `formatModified(epochSeconds, nowEpochSeconds)`, `sortEntries(entries, key='name', direction='asc')` per the spec's Model additions — exact strings: `—` never produced by formatSize (caller's job for dirs); `Today`/`Yesterday`/`Mon DD`/`YYYY-MM-DD`.
**Steps:** failing tests first (boundaries per spec Testing; sort fixture where name/size/modified orders all differ; stability; no-arg identity with today's output on the SAME fixture the existing tests use); implement; mutation-prove one boundary (`<10` decimal rule) and dirs-first-under-desc; full suite; commit.

### Task 2: DOM restructure

**Files:** modify `app/features/editor/file-dialog.js`; update selectors in the four contract suites; extend `test_file_dialog.mjs` with the spec's new checks.
**Interfaces consumed:** Task 1's three functions; `tlIcon` (read `app/ui/tl-icon.js` for the real API before use); existing handlers — scope click (with retry), breadcrumbs, path field, filter, row activation, footer buttons, save-mode controls. NOTHING about `buildScopes`/session data changes.
**Steps:** (1) read the current `render`/`parts` structure end to end and write the mapping old-class → new-class into the report BEFORE editing; (2) restructure: sidebar region (Places/Hosts sections from the same scope array; active row = selection tokens; icon per spec), main column (path bar row, header row with sort state + `aria-sort`, rows via `formatSize`/`formatModified`/icons), footer per spec; (3) header-click sort: state lives beside the existing filter state, applied in the same place filtering is; re-render preserves selection by entry name where it survives; (4) update the four suites' selectors deliberately — list every change; (5) new checks incl. the sort-order proof and dir `—`; mutation-prove header wiring (break the click handler → sort check fails, parse-verified); (6) full frontend suite + boundary; commit.

### Task 3: CSS + checklist + spec-sync

**Files:** rewrite `styles/design-system/components/file-dialog.css`; append checklist section H to `docs/superpowers/notes/light-editor-manual-checklist.md`; sync the spec if anything shipped differently.
**Steps:** tokens-only CSS for the new structure (sidebar bg one step below panel like the app's tool strips — find the token the SFTP panel rail uses and match it; ~24px rows; sticky header row inside the scroll box; right-aligned numeric columns with tabular alignment; breadcrumb strip per existing overflow-x pattern; footer split layout); verify zero hex + boundary script; checklist H (both themes, host switch + failed retry, sort clicks, save footer, long-path scroll, row-height judgment); full suite; commit.

## Verification Summary
All suites green with only deliberate selector diffs in the four contract files; model additions exhaustively tested; no-arg sortEntries identical; boundary script clean (pre-existing tl-dialog.js:334 only); zero new assets; branch never touches main.

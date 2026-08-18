# Untitled Files Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace scratch files with Notepad-style untitled buffers — File → New File opens an in-memory buffer, and every save path routes through the Save As dialog until the file has a home.

**Architecture:** `savePane` becomes the single choke point: a pane with no `filePath` diverts to the existing `openForSave` dialog, so ⌘S, vim `:w`/`:wq`, and close-guard Save all inherit the diversion with no per-caller logic. Dialog cancel rejects with a `SaveCancelled` sentinel that every catch-site treats as quiet not-saved. The scratch machinery (commands, module, labels) is deleted; the on-disk scratches directory is left alone.

**Tech Stack:** existing stack, no new dependencies.

**Spec:** `docs/superpowers/specs/2026-08-18-untitled-files-design.md`

## Global Constraints

- Keymap field renames `new_scratch` → `new_file` with `#[serde(alias = "new_scratch")]`; action string `new-file`; default stays `cmd+n`. Exact.
- Cancel is not a failure: `SaveCancelled` must never produce an error toast, anywhere.
- The four pinned suites (`test_editor_save_race`, `test_editor_save_inflight`, `test_shortcut_save_fallthrough`, `test_editor_remote_transfer`) pass UNMODIFIED. `test_editor_save_as` may need its harness extended but its existing assertions must not weaken.
- `~/.config/termlab/scratches` on disk is never touched by this plan.
- Trace every mechanism claim against the code; verified accessor names below are from 2026-08-18 but the code wins. Test fixtures must be discriminating (this project has produced five that were not).
- Menu wiring recipe = `open_file`'s eight sites (keymap struct+default+test in `termlab.rs`; `coreShortcutActionByKey`; `menu-actions.js`; palette; `KEYBOARD_CORE_LABELS`/`KEYBOARD_CORE_GROUPS` in **store.js**; `config.example.toml`; macOS `menu.rs` BOTH build sites; `titlebar.js`). New File carries a native ⌘N accelerator like Open File's ⌘O — it is a global action, so native consumption changes nothing.

---

### Task 1: Rename to New File, add the menu item, delete the scratch commands

**Files:** `crates/termlab_core/src/config/termlab.rs` (field rename + alias + tests), `crates/termlab_tauri/src/editor_fs.rs` + `src/lib.rs` (remove `editor_scratch_dir`/`editor_scratch_list` commands + registrations + their tests), `src/menu.rs` (File → New File, both macOS app-menu build sites, accelerator from `keyboard.new_file`), `frontend/app/ui/titlebar.js`, `frontend/app/shortcut-runtime.js` (`new_file: 'new-file'`), `frontend/app/menu-actions.js` (`new-file` action; keep calling `openScratch` for now — Task 2 renames it), `frontend/app/command-palette-runtime.js` ("New File"), `frontend/app/features/settings/store.js` (labels/groups), `config.example.toml`, plus the `KeyboardShortcuts` ts-rs type if it lists `new_scratch` (regenerate as EP Task 6 did).

**Steps:** (1) failing Rust tests first: `new_file` default `cmd+n`, AND an alias test — a config snippet containing `new_scratch = "cmd+shift+u"` deserializes into `new_file`; run, watch fail; (2) implement rename with `#[serde(alias = "new_scratch")]`; (3) sweep every `new_scratch`/`new-scratch`/"New Scratch" reference (grep, list them all in the report) and rename/remove; (4) add the menu items following `open_file` end to end; (5) remove the two scratch commands + their registrations + `editor_scratch_list`'s test and the `scratch_dir` helper if now unconsumed; (6) `cargo test -p termlab_core -p termlab_tauri`, full frontend suite (expect `test_scratch_naming.mjs` still green — it dies in Task 2), boundary script; (7) commit.

### Task 2: Untitled buffers and the savePane choke point

**Files:** `frontend/app/features/editor/editor-service.js` (openUntitled replaces openScratch; savePane diversion + `SaveCancelled`; catch-site sweep), `tab-manager.js` (`createEditorTab` accepts `filePath: null` + `untitledSeq`), `features/editor/tab-label.js` (`Untitled`/`Untitled-N`, tooltip `Unsaved`), `file-dialog.js` (`openForSave` prefill from the tab label for untitled, text selected), delete `features/editor/scratch.js` + its `index.html` tag + `scripts/tests/test_scratch_naming.mjs`; new `scripts/tests/test_editor_untitled.mjs`.

**Interfaces (verified today):** `openForSave(pane)` resolves the saveAs result or **null when cancelled**; `savePane` currently rejects on failure and every caller treats resolve-as-saved (vim's `save()` returns true after await; `confirmDirtyPanes` catches → toast + return false). The diversion: `if (!pane.filePath) { const r = await global.termlabFileDialog.openForSave(pane); if (r == null) { const e = new Error('save cancelled'); e.name = 'SaveCancelled'; throw e; } return; }` — then sweep EVERY `savePane`/`saveActiveEditor` catch-site and suppress the toast for `error.name === 'SaveCancelled'` while still treating it as not-saved: `confirmDirtyPanes` (abort close, no toast), `saveActiveEditor`'s caller(s), vim's `save()` (already silent-false — verify), `writeOnce`/`saveAs` if reachable. Enumerate each site + its behavior in the report.

**Also:** `focusExistingEditor` and `pathHeldByAnotherPane` skip panes/args with null path (two untitleds are never "the same file"); untitled counter is per-window session state in editor-service; `openUntitled` needs no invoke at all — no bundleMissing short-circuit skipped though (keep the CM6 check).

**Tests (fixtures must discriminate):** untitled counter across creates; savePane diversion BOTH directions (untitled → dialog stub called; titled → NOT called — assert call counts, not truthiness); cancel → `SaveCancelled` rejection, zero error toasts recorded, pane still `filePath === null` and dirty; success → rebind fields (reuse `test_editor_save_as.mjs`'s real-module harness pattern); two untitled panes + `focusExistingEditor('/some/path')` and with the first untitled's "path" — no match; close-guard Save cancelled at dialog → close aborted, pane intact (drive the real `confirmDirtyPanes`). Mutation-prove the diversion (remove it → the untitled-⌘S case fails) and the toast suppression (drop the name check → the zero-toasts case fails); paste output.

### Task 3: Integration sweep, checklist, spec cross-reference

**Steps:** (1) repo-wide grep for `scratch` in code (not docs/history): nothing live may remain except the untouched-on-disk note; (2) add the superseded-by note atop the scratch section of `2026-08-17-light-editor-design.md` pointing at the new spec; (3) append checklist section G (spec's manual list, numbered on from F's last step, `[SSH]` marks where relevant); (4) run everything: `cargo test --workspace`, full frontend suite, `npm run build:vendor`, boundary script (only pre-existing `tl-dialog.js:334`); confirm via `git diff --name-only` the four pinned suites untouched across the whole plan; (5) commit.

## Verification Summary

All suites green; pinned suites untouched; `test_scratch_naming.mjs` gone; `test_editor_untitled.mjs` present with mutation-proven diversion and suppression; a `new_scratch = …` config line still binds New File; no live `scratch` references; checklist G exists.

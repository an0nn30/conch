# Untitled Files — Design

**Status:** Draft
**Date:** 2026-08-18
**Scope:** Replace scratch files with Notepad-style untitled buffers: File → New File opens an in-memory buffer, and every save path routes through the Save As dialog until the file has a home.
**Supersedes:** the "scratches are real files" decision in `2026-08-17-light-editor-design.md`. That choice was made to avoid IntelliJ's save-as-on-first-save dance before a save dialog existed; the editor-polish branch shipped one (`openForSave`), so the rationale is gone and the cost (a scratch directory silently accumulating files) remains.

## Behaviour

1. **New File** — ⌘N, File → New File (native menu + titlebar, native accelerator like Open File…), and the palette — opens a tab labelled `Untitled` (`Untitled-2`, `-3`… per-session counter). `pane.filePath = null`, `pane.remote = null`, nothing on disk, not dirty.
2. **First save routes through the dialog.** ⌘S, vim `:w`/`:wq`, and "Save" in any close prompt on an untitled pane open the Save As chooser (This Mac + connected hosts — a first save may go straight to a remote path). On success the existing Save As rebind gives the pane its identity; it is an ordinary file tab thereafter. Cancelling the dialog leaves the pane untitled and dirty, aborts any close in progress, and shows no error toast — cancel is not a failure.
3. **Close is Notepad's:** an untouched untitled closes silently; a modified one prompts Save / Don't Save / Cancel. (Modified means "has been edited", the existing dirty flag — typing and deleting everything still counts, as in Notepad.)
4. **The scratch machinery is removed:** `editor_scratch_dir` and `editor_scratch_list` commands, `scratch.js`, `test_scratch_naming.mjs`, and every "New Scratch" label. The `~/.config/termlab/scratches` directory on disk is left untouched — it simply stops being special. The keymap field renames `new_scratch` → `new_file` with `#[serde(alias = "new_scratch")]` so existing rebindings keep working; the action string becomes `new-file`.

## Mechanism

- **`savePane` is the single choke point.** Its first line: a pane with no `filePath` diverts to `termlabFileDialog.openForSave(pane)`. All four save paths (⌘S via `saveActiveEditor`'s delegation, `:w`, `:wq`, close-guard Save) inherit the diversion with no per-caller logic. `openForSave` resolving null (cancelled) makes `savePane` reject with a sentinel (`error.name === 'SaveCancelled'`); every `savePane` catch-site suppresses the toast for that name and treats it as not-saved, so `:wq` does not close, close guards abort, and nothing red flashes for a deliberate cancel.
- **Untitled labels** come from `editorTabLabel` (already returns a fallback for null `filePath`); it gains the per-session `Untitled-N` from a counter passed at creation (`pane.untitledSeq`). Tooltip: `Unsaved`.
- **`focusExistingEditor(path)` must skip untitled panes** — two nulls are not the same file. Same guard in `pathHeldByAnotherPane`.
- **Save As prefill** for untitled panes: the filename field seeds from the tab label (`Untitled`), selected so typing replaces it.
- The empty-never-prompts rule needs no new state: an untouched buffer has `dirty === false` and every guard already keys on dirty.

## Testing

`test_editor_untitled.mjs` (vm harness over the real editor-service/tab-label modules, invoke-IO stubbed): untitled naming counter across several creates; `savePane` diversion — untitled pane calls `openForSave`, titled pane does not (discriminating fixture: assert the dialog stub was or was not called, both directions); dialog-cancel → `savePane` rejects with `SaveCancelled`, no error toast recorded, pane still untitled and dirty; dialog-success → rebind fields set (reusing the Save As harness's real-shape stubs); `focusExistingEditor(null)` matches nothing with two untitled panes open; close-guard Save on untitled cancelled at the dialog → close aborted, pane intact. Existing suites must pass unmodified except those that named scratches, which are removed or updated deliberately (list them in the report).

Manual (checklist section G): New File from all three entry points; type, ⌘S → dialog → save local and remote; `:w` on untitled → dialog; `:wq` cancelled at dialog → tab stays; close prompts and both cancel layers abort; empty untitled closes silently; `Untitled-2` naming; a config with `new_scratch = "cmd+shift+u"` still binds New File (alias).

## Known limitations

- Untitled content lives only in memory: a crash loses it (Notepad's deal; the close guards cover every graceful path).
- `Untitled-N` numbering is per-session and per-window.

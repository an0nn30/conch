# Editor Polish, File Dialog, and Vim Mode Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Terminal font in the editor, host-labelled remote tabs, a unified local/remote file dialog (Open + Save As), and toggleable vim keybindings.

**Architecture:** Four additive features over the merged light editor. The dialog is pure frontend over commands that all exist today (`local_list_dir`/`local_stat`/`local_mkdir` beside their `sftp_*` twins, one shared `FileEntry`). Vim arrives through the established vendor pipeline as one more pinned package behind a settings toggle. The risky logic is Save As rebinding a pane's identity; it is isolated in `editor-service.js` with failure-ordering tests.

**Tech Stack:** Existing stack; one new npm package (`@replit/codemirror-vim`).

**Spec:** `docs/superpowers/specs/2026-08-17-editor-polish-chooser-vim-design.md`

## Global Constraints

- No bundler for app code: plain IIFEs on `window`, no `import`/`export`/`require` in app modules. Third-party deps go through the vendor pipeline (`package.json` + `vendor-entry.mjs` + `check-vendor.mjs`).
- Frontend tests are plain Node scripts (`node scripts/tests/test_<name>.mjs`), `node:assert` + `node:vm`, no jsdom. `deepStrictEqual` fails cross-realm — compare fields.
- CSS uses `--tl-*` tokens only; no raw hex in design-system component CSS.
- **Accessor names in this plan were verified against the code on 2026-08-17, but read the real module before using any of them** — this project lost four fix rounds to invented names. If a name here is wrong, the code wins; note it in your report.
- Test stubs must return what the real code returns (shapes verified per task below). A stub returning the one value that skips the bug has burned this project twice.
- New keybindings: `open_file: "cmd+o"`, `save_file_as: "cmd+shift+s"`, exact.
- Vim setting: `editor.vim_mode`, bool, default `false`, exact.
- Dialog lists only CONNECTED hosts (`remote_get_sessions`); no in-dialog connecting.
- Save As upload failure leaves the pane on its OLD binding, dirty. Never half-rebind.
- Run `cargo clean -p termlab_tauri` if a build reports a symbol missing that exists on disk.

---

## File Structure

**Created:**
| Path | Responsibility |
|---|---|
| `crates/termlab_tauri/frontend/app/features/editor/tab-label.js` | Pure label/tooltip composition for editor tabs. |
| `crates/termlab_tauri/frontend/app/features/editor/vim-mode.js` | Vim compartment glue: enable/disable, `:w`/`:q`/`:wq` ex-commands. |
| `crates/termlab_tauri/frontend/app/features/editor/file-dialog-model.js` | Pure dialog model: sort, filter, breadcrumbs, target paths. |
| `crates/termlab_tauri/frontend/app/features/editor/file-dialog.js` | The dialog UI on `tl-dialog`; open + save-as modes. |
| `crates/termlab_tauri/frontend/app/features/settings/sections-editor.js` | Settings → Editor section (vim toggle). |
| `crates/termlab_tauri/frontend/styles/design-system/components/file-dialog.css` | Dialog styles, tokens only. |
| `crates/termlab_core/src/config/editor.rs` | `[editor]` config table (`vim_mode`). |
| `scripts/tests/test_editor_tab_label.mjs`, `test_file_dialog_model.mjs`, `test_editor_save_as.mjs`, `test_editor_vim_glue.mjs` | Per-module tests. |

**Modified:** `theme.js`, `editor-pane.js` (vim + language compartments), `editor-service.js` (saveAs), `tab-manager.js` (label helper use), `main-runtime.js` (font global), `vendor-entry.mjs` + `package.json` (vim), `config/mod.rs` (editor table), `termlab.rs` (2 keymap fields), `shortcut-runtime.js`, `menu-actions.js`, `command-palette-runtime.js`, `store.js` + `renderers.js` + sidebar registration (Editor section), `config.example.toml`, `index.html`, `docs/superpowers/notes/light-editor-manual-checklist.md`.

---

### Task 1: Terminal Font in the Editor

**Files:**
- Modify: `crates/termlab_tauri/frontend/app/features/editor/theme.js:39`
- Modify: `crates/termlab_tauri/frontend/app/main-runtime.js` (everywhere `termFontFamily` is assigned)

**Interfaces:**
- Produces: `window.__termlabTermFontFamily` — the terminal's full CSS font stack, kept current by main-runtime; `theme.js` reads it at build time.

- [ ] **Step 1:** In `main-runtime.js`, find EVERY assignment to `termFontFamily` (declaration ~line 59, config-load `.then`, and any settings-change path — grep, do not assume the count) and mirror each into `window.__termlabTermFontFamily = termFontFamily;` immediately after. Add a one-line comment at the declaration: the editor theme reads this global so editor and terminal share one font source.
- [ ] **Step 2:** In `theme.js`, replace `'.cm-scroller': { fontFamily: 'inherit' }` with:
```js
      '.cm-scroller': {
        // The terminal's stack, not the UI font — an editor beside a terminal
        // shares its typeface. main-runtime keeps this global current; the
        // literal fallback matches its declaration for the pre-init window.
        fontFamily: global.__termlabTermFontFamily
          || '"JetBrains Mono", "Fira Code", "Cascadia Code", "Menlo", monospace',
      },
```
- [ ] **Step 3:** Verify live: open a scratch — glyphs are visibly monospace and identical to the terminal's. Change the terminal font in Settings — the editor follows (the config-runtime editor arm already calls `refreshTheme`, which rebuilds the theme and re-reads the global). Record both observations.
- [ ] **Step 4:** Commit: `git add -u && git commit -m "fix: editor uses the terminal font, not the UI font"`

### Task 2: Host Indicator on Remote Tabs

**Files:**
- Create: `crates/termlab_tauri/frontend/app/features/editor/tab-label.js`
- Test: `scripts/tests/test_editor_tab_label.mjs`
- Modify: `crates/termlab_tauri/frontend/app/tab-manager.js` (`createEditorTab`, ~line 613)
- Modify: `crates/termlab_tauri/frontend/index.html` (script tag before `editor-service.js`)

**Interfaces:**
- Produces: `window.termlabEditorTabLabel = { editorTabLabel }` where `editorTabLabel(pane) -> { label, tooltip }`. `pane` needs only `{ filePath, remote }` (`remote` is `null` or `{ remotePath, hostLabel }`). Task 6's rebind calls this same function.

- [ ] **Step 1: Failing test** — `scripts/tests/test_editor_tab_label.mjs`, vm-sandbox load per `test_scratch_naming.mjs`, cases: local `{filePath:'/a/b/notes.md', remote:null}` → label `notes.md`, tooltip `/a/b/notes.md`; remote `{filePath:'/tmp/x/nginx.conf', remote:{remotePath:'/etc/nginx.conf', hostLabel:'dustin@web1'}}` → label `nginx.conf — dustin@web1`, tooltip `dustin@web1:/etc/nginx.conf`; remote basename comes from `remotePath` not the temp `filePath`; missing/empty filePath → `untitled`; null pane → `{label:'untitled', tooltip:''}`. Field-by-field asserts.
- [ ] **Step 2:** Run, verify it fails (module missing).
- [ ] **Step 3:** Implement the IIFE: basename = last non-empty `/`-segment; remote label `${basename} — ${hostLabel}`, tooltip `${hostLabel}:${remotePath}`; local tooltip = `filePath`.
- [ ] **Step 4:** Run, verify pass.
- [ ] **Step 5:** In `createEditorTab`: replace the `fileName` derivation with `const { label, tooltip } = global.termlabEditorTabLabel.editorTabLabel(pane)` — note the pane object must exist first; restructure so the button is labelled after the pane literal is built (read the function; `makeTabButton(label, …)` then `button.title = tooltip`). The dirty-marker span logic is untouched.
- [ ] **Step 6:** Register the script tag; verify live: open a remote file → tab reads `name — user@host`, hover shows full path; local tab unchanged. Run `node scripts/tests/test_editor_tab_label.mjs` and the full frontend suite.
- [ ] **Step 7:** Commit: `feat: remote editor tabs name their host`

### Task 3: Vim Mode

**Files:**
- Modify: `crates/termlab_tauri/frontend/package.json` (+`@replit/codemirror-vim`), `vendor-entry.mjs` (+`export { vim, Vim, getCM } from '@replit/codemirror-vim';`)
- Create: `crates/termlab_core/src/config/editor.rs`; Modify: `config/mod.rs` (module + `pub editor: EditorConfig` on `UserConfig`, serde default)
- Create: `crates/termlab_tauri/frontend/app/features/editor/vim-mode.js`; Test: `scripts/tests/test_editor_vim_glue.mjs`
- Modify: `editor-pane.js` (vim compartment, FIRST in the extension list), `sections-editor.js` (create) + `renderers.js` + settings sidebar/store registration (follow `sections-window.js`'s full registration — read `renderers.js:827` and mirror every hook), `index.html`, `config.example.toml`

**Interfaces:**
- Produces: `window.termlabVimMode = { vimExtensions(enabled) -> Array, registerExCommands(deps) }` where `deps = { savePane(pane), closeTab(tabId), currentPane() }` — real accessors: `savePane` from `termlabEditorService`, `closeTab` via `managerDelegates`, `currentPane` via `__termlabPaneAccess`. `editor-pane.js` gains `setVimMode(view, enabled)` (compartment reconfigure) alongside `setFontSize`/`refreshTheme`.
- Rust: `UserConfig.editor.vim_mode: bool`, default false.

- [ ] **Step 1:** Rust first — `editor.rs` with `EditorConfig { vim_mode: bool }`, `Default` false, serde default, test `vim_mode_defaults_off`. Wire into `UserConfig` + `config.example.toml` (`[editor]\nvim_mode = false`). `cargo test -p termlab_core editor` → pass.
- [ ] **Step 2:** `cd crates/termlab_tauri/frontend && npm install @replit/codemirror-vim && npm run build:vendor` — the check must report the three new exports present (51 total). Record the bundle-size delta in your report.
- [ ] **Step 3: Failing glue test** — `test_editor_vim_glue.mjs`: stub `global.CM6 = { vim: () => ({marker:'vim-ext'}), Vim: { defineEx: (name, short, fn) => calls.push([name, short, fn]) }, Compartment: class {...} }` (mirror the real shapes: `vim()` returns an extension, `Vim.defineEx(name, prefix, handler)`). Assert: `vimExtensions(true)` returns `[vim()]`-shaped array, `vimExtensions(false)` returns `[]`; `registerExCommands` defines `write`/`quit`/`wq`; invoking the captured `write` handler calls `deps.savePane` with `deps.currentPane()`'s pane; `quit` calls `deps.closeTab` with that pane's `tabId` (the guarded path prompts on dirty — do NOT bypass it); `wq` awaits save then closes; handlers are no-ops when no editor pane is focused.
- [ ] **Step 4:** Run → fails. Implement `vim-mode.js`; run → passes.
- [ ] **Step 5:** `editor-pane.js`: add `vimComp` compartment as the FIRST entry in the extensions array (vim's keymap must outrank `defaultKeymap`), initialized from `global.termlabVimMode && cfg.vimMode ? vimExtensions(true) : []` — thread `vimMode` the same way font size reached `createEditorView` (read how `getTermFontSize` flows through `manager-compose-runtime.js`; mirror it for the vim flag from `get_all_settings`' `editor.vim_mode`). Add `setVimMode(view, enabled)`.
- [ ] **Step 6:** Settings → Editor section: checkbox "Vim keybindings" bound to `pendingSettings.editor.vim_mode`; on apply, call `eachEditorPane((p) => termlabEditorPane.setVimMode(p.view, enabled))` from the same place other settings propagate (read how the window section's values reach save — follow it, do not invent a channel). Register section in every place `sections-window.js` is registered (renderers, sidebar list, search index).
- [ ] **Step 7:** Verify live: toggle on → `i`/Escape/`dd` work in an open editor without reopening it; `:w` on a remote file uploads (toast); `:q` on a dirty editor prompts Save/Don't Save/Cancel; ⌘S still saves; toggle off → plain editing returns, still live. Escape interplay (a spec Risk): with vim on and the editor focused, Escape switches vim mode and must NOT close a dialog or exit zen; with focus in a terminal or dialog, Escape behaves exactly as before. Run all suites.
- [ ] **Step 8:** Commit: `feat: optional vim keybindings for the editor`

### Task 4: File Dialog Model (pure)

**Files:**
- Create: `app/features/editor/file-dialog-model.js`; Test: `scripts/tests/test_file_dialog_model.mjs`; register in `index.html`.

**Interfaces:**
- Produces `window.termlabFileDialogModel`: `sortEntries(entries)` (dirs first, then name, case-insensitive — input is `FileEntry[]`: `{name, is_dir, size, modified, permissions}`); `filterEntries(entries, query, showHidden)` (query: every whitespace term must appear in name, case-insensitive, per `matchesFilter` in `export-picker.js`; hidden = leading dot); `splitBreadcrumbs('/a/b/c') -> [{label:'/', path:'/'}, {label:'a', path:'/a'}, …]`; `joinPath(dir, name)` (single slash); `parentPath(path)` (`/a/b`→`/a`, `/a`→`/`, `/`→`/`).

- [ ] **Step 1: Failing test** covering: sort (dirs before files regardless of name; `B` before `a` case-insensitively); filter (multi-term, hidden off excludes dotfiles but never `..`-style entries — the model never emits those); breadcrumbs for `/`, `/etc`, `/etc/nginx/conf.d`; joinPath with and without trailing slash on dir; parentPath chain down to `/`; empty/null inputs return empty arrays / `/`, never throw.
- [ ] **Step 2:** Run → fail. **Step 3:** Implement. **Step 4:** Run → pass. **Step 5:** Commit `feat: file dialog model`.

### Task 5: File Dialog UI + Open Mode (⌘O)

**Files:**
- Create: `app/features/editor/file-dialog.js`, `styles/design-system/components/file-dialog.css`
- Modify: `termlab.rs` (`open_file: "cmd+o"` + default + test), `shortcut-runtime.js` (`open_file: 'open-file'` in `coreShortcutActionByKey`), `menu-actions.js` (`open-file` → `termlabFileDialog.openForOpen()`), `command-palette-runtime.js` (entry "Open File…"), `store.js` (Editor keyboard group + label), `config.example.toml`, `index.html`, menu File → Open… (`menu.rs` macOS + `titlebar.js` — follow how Rename Tab is wired end to end).

**Interfaces:**
- Consumes: model (Task 4); `remote_get_sessions` (host list — derive labels exactly as `files-panel.js` does, reuse its helper if exported); `sftp_list_dir(paneId, path)` / `local_list_dir(path)` / `sftp_realpath(paneId, path)`; `termlabEditorService.openLocalFile(path)` / `.openRemoteFile({paneId, remotePath, hostLabel, size})`.
- Produces: `window.termlabFileDialog = { openForOpen(), openForSave(pane) }` (save implemented in Task 6; stub returns rejected promise with a clear message until then — and say so in a comment).

- [ ] **Step 1:** Build the dialog on `tlDialog.open` (model: `dialog-service.js`'s `confirmSave` for the promise pattern; `export-picker.js` for list/filter chrome). Scope bar: `This Mac` + one button per session; listing renders `FileEntry`s through the model; double-click/Enter on `is_dir` descends (remote paths through `sftp_realpath` once at scope entry to resolve `~`); on a file, resolve the dialog. Path field: editable, Enter jumps (list errors render inline in the body, not toasts). Hidden toggle. Escape/backdrop cancels. Buttons: Cancel / Open (disabled until a file is selected).
- [ ] **Step 2:** Open flow: local → `openLocalFile(path)`; remote → `openRemoteFile({paneId: session.paneId, remotePath, hostLabel: session.label, size: entry.size})`. No new open path — the guards and same-file-focus come free. Read `remote_get_sessions`' actual return shape first and name the fields you found in your report.
- [ ] **Step 3:** Keybinding plumbing exactly as `save_file` was done (all seven touchpoints: keymap struct + default + test, action map, menu-actions, palette, store group, example toml, menus). `open-file` is always active — it may consume ⌘O in terminals (deliberate; note it).
- [ ] **Step 4:** CSS, tokens only; listing box per `.tl-picker__box` with sticky column header.
- [ ] **Step 5:** Verify live: ⌘O both scopes; browse, filter, hidden toggle, path-field jump; open remote file → tab with host label (Task 2); reject a `.png`; cancel leaves no tab; disconnected host absent from scope bar; disconnect mid-browse → inline error. Full suites + boundary check (no new violations).
- [ ] **Step 6:** Commit `feat: unified file-open dialog for local and connected hosts`.

### Task 6: Save As + Rebind

**Files:**
- Modify: `editor-service.js` (`saveAs(pane, target)`), `file-dialog.js` (save mode: filename field, New Folder, overwrite prompt), `editor-pane.js` (language compartment + `setLanguage(view, filename)`), `termlab.rs` (`save_file_as: "cmd+shift+s"` + the same seven touchpoints), `tab-manager.js` if the label refresh needs an exported hook (prefer calling `setTabLabel`/`button.title` through an existing export — read first).
- Test: `scripts/tests/test_editor_save_as.mjs`
- Modify: `docs/superpowers/notes/light-editor-manual-checklist.md` (append section F: this plan's manual checks, all four features).

**Interfaces:**
- Consumes: `sftp_stat`/`local_stat` (existence), `sftp_mkdir`/`local_mkdir` (New Folder), `editor_write_file`, `editor_temp_path`, `transfer_upload` via the existing `runTransfer`, `editorTabLabel` (Task 2), `savesInFlight` (existing).
- Produces: `saveAs(pane, target)` where `target = {scope:'local', path} | {scope:'remote', paneId, hostLabel, remotePath}`.

- [ ] **Step 1: Failing test first** — `test_editor_save_as.mjs` with stubs returning REAL shapes (stat resolves `FileEntry` or rejects with a string; `transfer_upload` resolves an id and completion arrives via the stubbed `transfer-progress` listener exactly as `test_editor_remote_transfer.mjs` stubs it — copy that harness's stub layer). Cases: (a) local target, not existing → write called, pane.filePath updated, `remote` cleared, label refreshed via `editorTabLabel`, dirty cleared; (b) existing target → overwrite prompt; decline → nothing changes; (c) remote target, upload succeeds → `remote` rebinds `{paneId, remotePath, hostLabel}`, temp path becomes filePath, old temp cleaned only if the pane owned one; (d) **remote upload FAILS → pane keeps OLD filePath/remote/label and stays dirty** — assert every field; (e) a save already in flight for the pane → saveAs waits or refuses per `savesInFlight` (match the existing guard's behaviour, do not invent a new queue); (f) language mode re-derived (assert `setLanguage` called with the new basename).
- [ ] **Step 2:** Run → fail. Implement: write-to-new-location FIRST, rebind ONLY after every fallible step succeeded (compute new temp path, write, upload, then mutate the pane in one synchronous block: filePath, remote, `editorTabLabel` → `setTabLabel` + `button.title`, `setLanguage`, `termlabResetDirty`). Old remote temp: `editor_temp_cleanup` the previous path after successful rebind, never before.
- [ ] **Step 3:** Run → pass; run `test_editor_save_race.mjs` + `test_editor_save_inflight.mjs` — both must still pass untouched.
- [ ] **Step 4:** Dialog save mode: filename field prefilled from current basename, New Folder (prompt for name → mkdir → refresh listing), primary button `Save`, existence check via stat before resolving. Wire `save_file_as` (guarded like `save_file`: editor-focused only, `coreHit = null` passthrough shape — read the existing `save-file` guard and mirror it exactly, including the fkey-table handling).
- [ ] **Step 5:** Verify live: ⌘⇧S scratch → remote host at a path that does not exist → created, tab shows `name — user@host`, `:w`/⌘S now upload to it; Save As onto an existing file → overwrite prompt; kill connection then ⌘⇧S to that host → failure toast, tab STILL points at the old location and is dirty; New Folder both scopes.
- [ ] **Step 6:** Append checklist section F; run every suite (`cargo test --workspace`, all `test_*.mjs`, boundary check — only `tl-dialog.js:334` may appear); commit `feat: save as across local and remote targets`.

---

## Verification Summary

- `cargo test --workspace`, all `scripts/tests/test_*.mjs`, `check_frontend_boundaries.sh` (no NEW violations beyond `tl-dialog.js:334`), `npm run build:vendor` (51 exports).
- The pinned regression suites (`test_editor_save_race`, `test_editor_save_inflight`, `test_shortcut_save_fallthrough`) pass unmodified.
- Bundle-size delta from vim recorded in the final report.
- Checklist section F exists and its live checks were run where no SSH host is required; remote checks listed as pending human verification otherwise.

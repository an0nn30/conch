# Editor Polish, File Dialog, and Vim Mode — Design

**Status:** Draft
**Date:** 2026-08-17
**Scope:** Four follow-ups to the light editor (merged earlier today): terminal font in the editor, host indicator on remote tabs, a unified local/remote file dialog, and optional vim keybindings.

## Goals

1. The editor renders in the terminal's monospace font, not the UI font.
2. A remote file's tab says which host it lives on.
3. ⌘O opens and ⌘⇧S saves-as through one in-app dialog that browses This Mac and every connected SFTP host — including saving to a remote path that does not exist yet.
4. Vim keybindings, off by default, toggleable in Settings.

## Non-Goals

- Connecting to a host from inside the file dialog. Hosts appear only while connected; connecting stays the SFTP/Hosts panels' job.
- Recents, favorites, or path bookmarks in the dialog.
- Vim customization (vimrc, custom mappings) beyond on/off.
- Changing how plain ⌘S works.

## 1. Terminal Font in the Editor

`app/features/editor/theme.js:39` sets `.cm-scroller { fontFamily: 'inherit' }`, which resolves to the UI font (Inter). It changes to the terminal stack — the same `'"JetBrains Mono", "Fira Code", "Cascadia Code"' + FONT_FALLBACKS` string `main-runtime.js:58-59` builds — passed into `buildTheme` rather than duplicated: `editor-pane.js`'s theme compartment already rebuilds on config change, and `setFontSize` already threads the size, so family rides the same path. A user-configured terminal font reaches the editor the same way it reaches xterm.

## 2. Host Indicator on Remote Tabs

`tab-manager.js:613` derives the label from the basename alone. For panes with a `remote` binding (already carrying `hostLabel`, e.g. `dustin@web1`), the label becomes `nginx.conf — dustin@web1`, and `button.title` (currently unset — a deferred minor from the branch review) becomes the full `user@host:/remote/path`. Local tabs: basename label, absolute path tooltip. The dirty marker span stays a sibling after the label, unaffected. Save As (§3) rebinds a pane and must refresh label and tooltip through the same helper, so the composition lives in one function, `editorTabLabel(pane)`, pure and unit-tested.

## 3. The File Dialog

One `tl-dialog`-based chooser, two modes, one new module `app/features/editor/file-dialog.js` plus `styles/design-system/components/file-dialog.css` (tokens only).

**Why custom for local too:** in Save As the destination is chosen inside the dialog. A native-local/custom-remote split would fork the UI on a decision the user has not made yet when the dialog opens — and "save this scratch to host X, at a path that does not exist there" must be one flow.

### Backing commands — all exist today, none are new

| Need | Command | Notes |
|---|---|---|
| List remote dir | `sftp_list_dir(paneId, path)` | wrapped by `features/files/data-service.js:29` |
| List local dir | `local_list_dir(path)` | `remote/sftp_commands.rs:178`, same `FileEntry` type |
| Stat / exists | `sftp_stat(paneId, path)` / `local_stat(path)` | overwrite prompt |
| New folder | `sftp_mkdir(paneId, path)` / `local_mkdir(path)` | |
| Connected hosts | `remote_get_sessions` | already feeds `files-panel.js`'s host labels |
| Read/write/guards | the editor's existing open/save paths | unchanged |

### Layout

- **Scope bar:** `This Mac` plus one entry per connected SFTP session (label `user@host[:port]`, same derivation `files-panel.js` uses). Disconnecting mid-browse surfaces the listing error in the dialog body; the dialog does not chase session state.
- **Path bar:** clickable breadcrumbs plus an editable text field (paste a path, Enter to jump — the typed value is passed to the listing call verbatim; see Known Limitations). Each scope's START directory, not the typed field, is what resolves `~`: it is looked up once at scope entry, locally via `get_home_dir` (wrapped by `features/files/data-service.js`'s `getHomeDir`), remotely via `sftp_realpath(paneId, '.')`.
- **Listing:** directories first then files, both sorted, type-ahead filter box (reusing the `.tl-picker__filter` pattern), double-click or Enter to descend/choose, hidden files toggle (off by default).
- **Save As adds:** filename field (pre-filled from the pane), **New Folder** button, primary button reads `Save`.
- Keyboard: arrows + Enter + Escape per `tl-dialog` conventions; the dialog registers at the standard dialog router priority (225).

### Open mode — ⌘O

Choosing a file routes through the exact existing entries: `openLocalFile(path)` or `openRemoteFile({paneId, remotePath, hostLabel, size})` with the size from the listing and the `paneId` from the chosen session's entry in `remote_get_sessions` (the same id the scope bar was built from). Every guard (5 MB cap, blocklist pre-check, binary sniff, same-file-focuses-existing-tab) applies unchanged because the dialog adds no second open path.

### Save As mode — ⌘⇧S

Target resolution, then:

1. **Target exists** (stat succeeds) → overwrite prompt (`tl-dialog` confirm, Cancel default).
2. **Local target:** `editor_write_file(path, contents)` — the temp-and-rename write.
3. **Remote target:** resolve `editor_temp_path(hostLabel, remotePath)`, write the temp locally, then the existing `transfer_upload` via `runTransfer` — creating the remote file if absent. Upload failure keeps the pane on its OLD binding, dirty, with the standard failure toast: a failed Save As must not half-rebind.
4. **On success, the pane rebinds:** `filePath`, `remote` (set, changed, or cleared for a local target), tab label + tooltip via `editorTabLabel`, language mode re-derived from the new name via the existing `languageKeyFor` + a compartment reconfigure. The previous file remains as last saved. In-flight guards: Save As respects `savesInFlight` for the pane like any save.

Save As is registered as keymap field `save_file_as` (default `cmd+shift+s`), menu File → Save As…, palette entry; ⌘O as `open_file`, File → Open…, palette. Both follow the `save_file` precedent: active only when relevant (Open always; Save As only with a focused editor), pass through otherwise, listed in Settings → Keyboard's Editor group.

The two menu items are not symmetric: File → Open… carries a native accelerator (⌘O), but File → Save As… deliberately ships with none. A native menu accelerator is consumed by AppKit before the webview sees the key — the same reason `save_file` has never had a menu item — so binding ⌘⇧S natively would steal the combo from every terminal pane instead of respecting Save As's editor-focused scoping, which lives entirely in `shortcut-runtime.js`'s fallback guard. The keystroke is still handled there, Settings → Keyboard still lists it, and the menu item (with no accelerator shown) is the discoverable, always-safe route; `menu-actions.js` re-checks the focused pane before acting either way.

## 4. Vim Mode

- `@replit/codemirror-vim` (pinned) joins `package.json`; `vendor-entry.mjs` exports `{ vim, Vim }` (`getCM` has no consumer in this app and is deliberately not re-exported); `check-vendor.mjs` covers them automatically. Measured bundle delta: +123,571 bytes (+11.71%), from 1,055,575 to 1,179,146 bytes.
- **Settings → Editor** (new section, `sections-editor.js`, following `sections-window.js`'s shape): checkbox "Vim keybindings", persisted in `UserConfig` as `editor.vim_mode: bool` (new tiny `[editor]` config table, default false).
- `editor-pane.js` gains a vim compartment, first in the extension list so vim's keymap outranks the default keymap. The Settings toggle reconfigures every open editor live from `config-runtime.js`'s `config-changed` handler, which loops `getPanes().values()` directly (not `editor-service.js`'s `eachEditorPane`) — the same shape as the two sibling live-apply arms already in that function.
- `:w` maps to the app's save for that pane (`savePane`), including remote upload; `:wq` saves then closes through `closeTab`'s guarded path; `:q` on a dirty pane routes to the same Save/Don't Save/Cancel prompt rather than vim's own error. ⌘S, ⌘W and every close guard behave identically with vim on.
- Escape handling: with vim enabled, Escape belongs to mode switching inside a focused editor; the keyboard router only sees it when the editor is not focused. This matches how xterm panes already swallow keys.

## Testing

Rust: `open_file`/`save_file_as` are real keymap fields on `KeyboardConfig` (`termlab_core/src/config/termlab.rs`, defaults `cmd+o` / `cmd+shift+s`), each covered by two tests (`{open_file,save_file_as}_fills_in_for_a_config_written_before_it_existed`, `{open_file,save_file_as}_round_trips_when_overridden`) plus the existing default-value assertion. `settings.rs` adds `changed_vim_mode_no_restart`, asserting `editor.vim_mode` is hot-reloadable. `menu.rs` adds two `MenuItem`s (`MENU_OPEN_FILE_ID`, `MENU_SAVE_FILE_AS_ID`); `lib.rs` adds their two dispatch arms.

Frontend (`node scripts/tests/…`, existing conventions — vm sandbox, field-by-field asserts):
- `test_editor_tab_label.mjs` — `editorTabLabel`: local basename, remote `name — user@host`, tooltip composition, dirty marker untouched, rebind cases (local→remote, remote→local, remote→other-host).
- `test_file_dialog_model.mjs` — the dialog's pure model: sort (dirs first), type-ahead filter, breadcrumb split/join for local and remote paths, target-path composition (dir + filename field), hidden-file filtering.
- `test_file_dialog.mjs` — the dialog UI layer, against the real `tl-dialog.js` and `file-dialog-model.js` with a minimal DOM stub: session→scope derivation (including the `{window_label}:{pane_id}` key parse and the shared host-label formula), listing/filter/hidden-toggle/Open-button gating, Enter/double-click descend vs. open with the double-Enter race closed, inline listing and scope-entry (`sftp_realpath`) failures — including retrying a scope whose entry failed by clicking it again — Escape/backdrop cancel via `tl-dialog`'s own router, routing into `termlabEditorService`, and the save-mode path field / `openForSave` stub (31 checks).
- `test_editor_save_as.mjs` — rebind logic against stubbed invokes returning real command shapes: success rebinds fully; remote upload failure leaves the OLD binding and dirty=true; overwrite prompt fires on existing stat; `savesInFlight` respected. Failure-injection stubs must return what the real commands return.
- `test_editor_vim_glue.mjs` — Vim: mode toggle reconfigures live panes (compartment call observed); `:w` invokes `savePane` for the right pane.

Manual (append to `docs/superpowers/notes/light-editor-manual-checklist.md`): editor font visibly monospace and tracks the terminal font setting; remote tab shows host; ⌘O both scopes; Save As local→remote with a nonexistent remote path creates it; overwrite prompt; upload-failure Save As leaves the old file intact and the tab dirty; vim toggle live-applies, `:w` uploads, `:q` prompts when dirty; with vim off nothing changed.

## Known Limitations

1. The dialog lists only connected hosts; no in-dialog connect.
2. No recents/favorites; the dialog does not open at the pane's current directory — each scope has a fixed start, resolved once at scope entry: local seeds at the home directory (`get_home_dir`), remote at `sftp_realpath(paneId, '.')`.
3. Vim is on/off only — no vimrc, no custom mappings.
4. Save As to remote inherits the editor's existing cross-window temp-path limitation.
5. The typed path field does no `~` expansion of its own — pasting `~/foo` and pressing Enter is passed to the listing call verbatim and fails to list. Only the scope's start directory (Known Limitation 2) resolves `~`.

## Risks

- **Save As rebind is the risky logic** — it mutates pane identity that the dirty guards, `focusExistingEditor`, `opensInFlight`, and temp cleanup all key on. `filePath` changes must move the pane's registrations atomically; the tests above pin the failure-ordering cases, and the whole-branch review should audit every `pane.filePath` consumer.
- **Vim/router Escape interplay** — verify Escape in a vim editor never leaks to the dialog/zen router while focused, and still works app-wide when the editor is not focused.
- **`@replit/codemirror-vim` version drift** against the pinned CodeMirror packages; the lockfile pins it, `check-vendor` catches missing exports.

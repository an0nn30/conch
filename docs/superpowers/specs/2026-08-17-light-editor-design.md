# Light Editor — Design

**Status:** Draft
**Date:** 2026-08-17
**Scope:** A light text editor surface in the Rust/Tauri app: scratch files, local files, and SFTP-triggered remote file editing.
**Ports:** `~/projects/TermLab/docs/superpowers/specs/2026-04-14-light-scratch-editor-design.md` (the JVM/IntelliJ design)

## Goals

1. Open a text file as a tab beside terminal tabs, edit it, and save it.
2. Create scratch files for quick notes and throwaway scripts.
3. Double-click a file in either SFTP pane — local or remote — to edit it; save writes back to disk or uploads to the remote.
4. Give the Script Runner port the entry point its own spec assumes: "the file in the active editor tab."

## Non-Goals

Carried over from the JVM spec, unchanged:

- Full IDE editing — completions, inspections, refactoring, go-to-definition.
- Running scripts. That is the Script Runner feature, built next, on top of this.
- Conflict detection for remote editing. If the remote file changes between open and save, we overwrite it.
- Auto-reload when a file changes underneath an open tab.
- A persistent scratches list / "Scratches & Consoles" surface.

## Deviations from the JVM Design

Three parts of the JVM design do not survive the port, each for a structural reason.

**No opt-in gate.** The JVM version ships the editor as a bundled-but-disabled plugin, with a first-launch notification, a settings checkbox, and a restart on every toggle — all of it machinery for keeping TextMate's memory cost off users who never asked for an editor. The Rust app has already settled the editor as **core**, not a Lua plugin, and CodeMirror's cost is a one-time bundle size rather than a per-launch class-loading cost. The feature is simply present. This removes the first-launch notification, the settings page, the `firstLaunchHandled` marker, `disabled_plugins.txt` seeding, and both restart prompts.

**Scratches are real files.** The JVM version keeps a scratch in memory as a `LightVirtualFile` and runs a Save-As dance on first save, which its own risk section flags as awkward — IntelliJ has no clean way to convert a tab from one file to another, so it closes and reopens with the caret restored. Here a scratch is a real file created immediately under `<config_dir>/scratches/`, which makes it an ordinary local-file tab from birth. Save is a plain write, reopening across launches works, and Script Runner gets a real path with no special case. The cost is files on disk the user did not explicitly ask for; the scratch directory is theirs to clean.

**Guards live in Rust only.** The JVM spec runs the size cap and extension blocklist on the EDT before download. Here they run in the Rust layer and the frontend calls `editor_can_open` before initiating a transfer. One implementation, one list, checked again at read time.

## Architecture

### 1. The Build Step

CodeMirror 6 is ESM-only and expects a bundler. The frontend has never had one: 93 script tags in `index.html`, IIFE modules, libraries vendored as plain UMD files under `frontend/vendor/`. This introduces a build step scoped strictly to third-party dependencies. The app's own modules are not touched and keep loading via script tags.

**`crates/termlab_tauri/frontend/package.json`** — new file, declaring `esbuild` and the CodeMirror packages as `devDependencies`, with:

```json
"scripts": { "build:vendor": "node build-vendor.mjs && node check-vendor.mjs" }
```

**`crates/termlab_tauri/frontend/build-vendor.mjs`** — new file. Runs esbuild over a generated entry module that re-exports the CodeMirror API, emitting:

- `format: 'iife'`, `globalName: 'CM6'`, `bundle: true`, `minify: true`, `target: 'es2020'`
- Output: `frontend/vendor/codemirror/codemirror.js`

CodeMirror injects its own styles from JavaScript via `style-mod`, so there is no companion CSS file to emit or register.

**Packages bundled** (exact set; anything not listed is out of scope):

| Purpose | Packages |
|---|---|
| Core | `@codemirror/state`, `@codemirror/view`, `@codemirror/commands`, `@codemirror/language`, `@codemirror/search` |
| Highlighting | `@lezer/highlight` |
| Languages | `@codemirror/lang-javascript`, `-json`, `-python`, `-markdown`, `-rust`, `-html`, `-css`, `-xml`, `-yaml`, `-sql`, `-java`, `-cpp`, `-go`, `-php` |
| Legacy modes | `@codemirror/legacy-modes` (shell, toml, dockerfile, lua, ruby, perl, powershell, nginx, properties, diff) |

No `@codemirror/autocomplete` — completions are an explicit non-goal, and leaving it out keeps the bundle honest.

**Wiring:**

- `tauri.conf.json` gains `"beforeBuildCommand"` and `"beforeDevCommand"`, both running `npm --prefix crates/termlab_tauri/frontend ci && npm --prefix crates/termlab_tauri/frontend run build:vendor`. It has neither key today; `frontendDist` stays `"frontend"`.
- `.gitignore` gains `crates/termlab_tauri/frontend/vendor/codemirror/` and `crates/termlab_tauri/frontend/node_modules/`.
- `package-lock.json` is committed. `npm ci` — not `npm install` — so versions are pinned by the lockfile.
- **CI (`ci.yml`) needs no change.** Its four jobs run only `cargo fmt`, `cargo test --workspace`, and `cargo clippy` — none of which build the frontend.
- **Release (`release.yml`) does.** Four jobs produce app binaries and each gains `actions/setup-node@v4`:
  - `macos` and `windows` run `cargo tauri build`, so `beforeBuildCommand` fires and only Node itself is needed.
  - `linux-amd64` and `linux-arm64` run `cargo build --release -p termlab_tauri` **without** the Tauri CLI, so `beforeBuildCommand` never fires. Each needs an explicit `npm ci && npm run build:vendor` step before the build, or it ships a binary whose `index.html` points at a bundle that was never generated.
- `index.html` gains `<script src="vendor/codemirror/codemirror.js"></script>` alongside the existing xterm tags. `settings.html` does **not**: it is a separate document with no panes and no editor, so loading a megabyte of CodeMirror into it would buy nothing.

**Accepted cost:** a fresh clone can no longer build without Node installed. This is the price of a build step over a committed blob, and it is deliberate — CodeMirror upgrades become a version bump instead of a manual re-vendor, and future npm dependencies follow the same path.

**Export check:** `crates/termlab_tauri/frontend/check-vendor.mjs` asserts that every name `vendor-entry.mjs` claims to export actually resolves on the built `CM6` global. A name that does not is otherwise silent — the language simply never highlights. `build:vendor` runs it after the build, so every path that produces a bundle (`beforeBuildCommand`, `beforeDevCommand`, and both Linux release jobs) fails loudly on a bad export instead of shipping one.

### 2. The Editor Pane Kind

Panes already carry a `kind`, and `plugin_view` is already a non-terminal kind (`plugin-runtime.js:121`) that participates in tabs, splits, drag-and-drop, and layout persistence. The editor follows that precedent exactly: `kind: 'editor'`.

**Pane shape:**

```js
{
  kind: 'editor',
  id, element,
  filePath,        // absolute local path; the scratch path for scratches
  view,            // CodeMirror EditorView
  dirty,           // boolean
  remote: null | { paneId, remotePath, hostLabel },
}
```

**Guards to audit.** Every existing `kind === 'terminal'` / `kind !== 'terminal'` test must be examined and given a deliberate answer for editor panes. They are not uniform: some are correctly terminal-only, some need an editor arm.

| Site | Required behavior |
|---|---|
| `window-events-runtime.js:89,99` | Terminal-only (PTY output). No change. |
| `clipboard-runtime.js:9` | Needs an editor arm — paste-into-editor must work. |
| `context-menu-runtime.js:30` | Terminal-only mouse-mode check; editors get the default context menu. |
| `shortcut-runtime.js:277` | Terminal-only. No change. |
| `config-runtime.js:37,59,66` | Font/theme reapplication — needs an editor arm so font-size changes reach editor panes. |
| `main-runtime.js:488,501` | Font/fit loop — editor arm: CodeMirror reflows itself, so this only sets the font. |
| `pane-manager.js:83,88,214,268,269,288` | Focus, close, and session-persistence. Editor arm: focus the `EditorView`; on close run the dirty guard; persist as an editor entry. |
| `tab-manager.js:375,376,379` | Close plumbing. Editor arm mirroring the `plugin_view` branch. |

**No session persistence.** The app does not restore tabs or panes across launches for any pane kind — there is no workspace-session mechanism to extend. Editor tabs are therefore in-memory like every other tab, and a scratch survives a restart only as a file on disk, to be reopened by hand. Building workspace restore is a separate feature; it is listed under Follow-ups.

### 3. Rust: File I/O and Guards

There are no filesystem commands in the app today. New module `crates/termlab_tauri/src/editor_fs.rs`:

```rust
pub enum OpenRejection {
    TooLarge { size: u64, max: u64 },
    BlockedExtension { ext: String },
    Binary { name: String },
}

pub fn guard_openable(name: &str, size: u64) -> Result<(), OpenRejection>
pub fn looks_binary(head: &[u8]) -> bool
```

- `MAX_EDIT_BYTES = 5 * 1024 * 1024`.
- Blocklist, case-insensitive, carried over verbatim from the JVM spec: `png jpg jpeg gif bmp ico webp svg zip tar gz tgz bz2 xz 7z rar jar war ear class exe dll so dylib pdf doc docx xls xlsx ppt pptx mp3 mp4 mov avi mkv wav flac pyc pyo`.
- `looks_binary` returns true if any byte in the first 8192 is `0x00`.

**Tauri commands** (registered in `lib.rs`'s `invoke_handler`):

| Command | Signature | Notes |
|---|---|---|
| `editor_can_open` | `(name: String, size: f64) -> Result<(), String>` | Pre-transfer check. No I/O. |
| `editor_read_file` | `(path: String) -> Result<String, String>` | Applies `guard_openable` against the on-disk size, then `looks_binary` on the head, then reads as UTF-8 lossy. |
| `editor_write_file` | `(path: String, contents: String) -> Result<(), String>` | Writes to `<path>.termlab-tmp` then renames, so a failed write never truncates the original. |
| `editor_scratch_dir` | `() -> Result<String, String>` | The scratch directory, created if absent. |
| `editor_scratch_list` | `() -> Result<Vec<String>, String>` | The file names already in the scratch directory, so `nextScratchName` can pick a free name without a round trip per candidate. Resolves the directory through the same helper as `editor_scratch_dir`: a divergence there would report a taken name as free and `editor_write_file` would truncate a scratch. |
| `editor_temp_path` | `(host_label: String, remote_path: String) -> Result<String, String>` | Resolves the remote-edit temp path (below) and creates its parent directories. |
| `editor_temp_cleanup` | `(path: String) -> Result<(), String>` | Deletes a temp file and any parent directories it leaves empty, refusing any path outside the temp root. |

`editor_fs.rs` also exposes `editor_temp_sweep()`, which deletes the whole temp root. It is **not** a Tauri command: its callers are app setup and `close_guard::finish_exit`, and handing the frontend "delete every remote edit in flight" is not a capability it needs.

Temp-path resolution lives in Rust rather than the frontend because that is where the guards live. The hash itself is no argument for it — it is a 6-line FNV-1a (below), equally trivial in JS. The argument is that the temp path, the size cap, the blocklist, and the cleanup's refusal to delete outside the temp root are one set of rules about one directory, and splitting them across the boundary would mean the frontend could compute a path the cleanup would then reject. One owner, one place to test.

Encoding is UTF-8. Files that are not valid UTF-8 but pass the binary sniff are read lossily; this is documented as a limitation rather than a silent corruption risk, because saving such a file rewrites the replacement characters.

### 4. SFTP Integration

`frontend/app/features/files/pane-view.js:76` already binds `dblclick` on file rows. Today it navigates directories and does nothing for files. The file branch becomes:

**Local pane:** open an editor pane on the path directly. There is no separate `editor_can_open` call — a local open goes straight to `editor_read_file`, which applies the same guards against the real file rather than against the listing's idea of it. That is strictly better here: no transfer is at stake, so there is nothing to save by rejecting early, and no window in which the listing can be stale. `editor_can_open` exists for the remote path, where the point is to reject *before* pulling bytes over SFTP.

**Remote pane:** call `editor_can_open(name, size)`; on success call `editor_temp_path(hostLabel, remotePath)`, then the existing `transfer_download(pane_id, remote_path, local_path)`, and await completion by listening to the existing `transfer-progress` event for the returned `transfer_id` — the payload carries `status` and `error`, and `files-panel.js:128` already demonstrates the listener pattern. On completion, open an editor pane on the temp path with `remote` populated.

**Temp path layout**, carried over from the JVM spec:

```
<editor_temp_dir>/<fnv1a(hostLabel)[..8]>/<fnv1a(remoteAbsolutePath)[..8]>/<basename>
```

The hash is FNV-1a, 64-bit, rendered as its first 8 hex characters. It disambiguates directory names on disk and is not security, so a cryptographic hash would be a dependency bought for nothing.

The basename is preserved so the language mapping picks the right mode. The hash prefixes keep same-named files on different hosts and paths apart. Opening the same remote file twice resolves to the same temp path, so the second double-click focuses the existing tab rather than opening a duplicate — within one window. See Known Limitations for what happens across two.

**Save on a remote-bound editor:** write the temp file via `editor_write_file`, then `transfer_upload(paneId, localPath, remotePath)`. Success shows a notification naming the host and path.

Failure shows an error notification naming the temp path and telling the user to save again — ⌘S *is* the retry, so there is no Retry button. That is the simpler design and it is the one the close guards already exercise: the upload happens **before** the dirty flag is cleared, so **a failed upload leaves the pane dirty**, every close guard refuses to discard the tab, and the temp file survives. Losing a user's edit to a dropped connection is the wrong side to be wrong on.

**Cleanup**, carried over: on tab close — and on closing a split editor pane — delete the temp file and any now-empty parent directories; on startup, sweep the temp root for orphans left by a crash, off the main thread. The temp root is also swept on a completed Quit or Restart poll, but not on every exit: see Known Limitation 9.

### 5. Dirty State and Close Guards

This is where the defects will be. Four paths tear down unsaved editors and all four must ask:

1. **Tab close** — `tab-manager.js`'s `closeTab`, already `async`, so it can await the dialog. Unconditional: there is no opt-out parameter, because an opt-out is a second door past the only tab-level guard. (Closing a *split* editor pane is guarded separately in `pane-manager.js`, which is the one close that does not route through `closeTab`.)
2. **Window close** — `lib.rs`'s `on_window_event` handles only `Focused` and `Destroyed` today; a `WindowEvent::CloseRequested` arm is new work. It calls `api.prevent_close()`, emits an event to that window, and closes only when the frontend answers that nothing is dirty or the user chose to discard.
3. **App quit** — one window at a time, not a broadcast, so a second window's unsaved buffer is not destroyed by a quit started in the first.
4. **The updater's "Restart Now"** — a restart destroys unsaved buffers exactly as thoroughly as a quit, and the user answered "apply the update?", not "discard your work?". It runs the same poll. Guarding it inside `close_guard::request_restart` rather than at the updater's toast covers every caller of `restart_app`, not just that button.

**Quit is a custom `MenuItem`, not `PredefinedMenuItem::quit`** — a user-visible change to the macOS app menu. The predefined item sends `[NSApp terminate:]`, which `tao` does not intercept (`applicationShouldTerminate:` is unimplemented) and which raises neither `WindowEvent::CloseRequested` nor `RunEvent::ExitRequested`, so there is no point at which unsaved editors could be checked. Routing quit through a menu id lets the poll run first. Known Limitation 9 is what remains of this problem.

Each shows a Save / Discard / Cancel dialog naming the file. Cancel aborts the close entirely. With several dirty editors, the dialog is per-file, in tab order, and Cancel on any one aborts the whole operation.

There is no general confirm dialog today — `dialog-service.js` only has plugin-permission confirms. A `confirmSave(fileName)` helper joins it, built on `tl-dialog`, returning `'save' | 'discard' | 'cancel'`. Dialog stacking and Escape priority follow the existing `tl-dialog` conventions; Escape maps to Cancel.

Dirty state is tracked from CodeMirror's `updateListener` on `docChanged`, and the tab label shows a modified marker.

### 6. Language Mapping and Theming

**`frontend/app/features/editor/language-map.js`** — pure, testable: `languageKeyFor(name)` returns a mode key or `null` for plain text. Maps by extension, with special cases for extensionless well-known names (`Dockerfile`, `Makefile`, `.bashrc`, `.zshrc`, `.gitignore`) and a lowercase comparison throughout. Being pure and table-driven, it is the natural home for the mapping and the easiest thing in this feature to test exhaustively.

**`frontend/app/features/editor/theme.js`** — builds a CodeMirror theme from the existing `--tl-*` design tokens read off the document element, plus a `HighlightStyle` mapping Lezer tags to token colors. It re-reads on theme change, driven by the same path `config-runtime.js` already uses to push terminal themes, so the editor follows skins with the rest of the UI. No raw hex: tokens only, per the design-system rule.

### 7. Frontend Module Layout

New files under `crates/termlab_tauri/frontend/app/features/editor/`:

| File | Responsibility |
|---|---|
| `language-map.js` | Filename → language mode. Pure. |
| `theme.js` | Design tokens → CodeMirror theme + highlight style. |
| `editor-pane.js` | Create/destroy an `EditorView` in a pane element; dirty tracking; focus. |
| `editor-service.js` | Open/save/close orchestration: local, scratch, and remote flows; upload-on-save. |
| `scratch.js` | Scratch naming (`scratch-1.txt`, first free number in the scratch dir) and creation. |

Each registered in `index.html`. `scratch.js`'s naming function is pure and testable.

### 8. Keyboard

Two new bindings in `termlab_core::config::termlab`'s keymap struct, alongside the existing fields, and therefore configurable in Settings → Keyboard like every other binding:

| Field | Default | Behavior |
|---|---|---|
| `new_scratch` | `cmd+n` | Creates and opens a new scratch tab. Free today — nothing binds `cmd+n`. |
| `save_file` | `cmd+s` | **Only active when the focused pane is an editor.** In a terminal pane the keystroke passes through untouched. |

`new_scratch` also appears in the command palette, which picks up keymap entries automatically.

Note the deviation from the app's `cmd+shift+*` convention: `cmd+s` for save is universal enough that shadowing it would be the surprising choice, and it is scoped to editor panes so it costs terminals nothing.

## Error Handling

| Failure | Surface | Recovery |
|---|---|---|
| File larger than 5 MB | Notification: "File too large (X MB). Maximum is 5 MB." | No transfer, no tab |
| Blocklisted extension | Notification: "Cannot edit binary file: {name}" | No transfer, no tab |
| Null byte in first 8 KB | Notification: "Binary file detected: {name}" | Temp file deleted, no tab |
| Download fails | Notification carrying the transfer error | No tab, temp file deleted |
| Upload fails | Error notification naming the temp path: "…save again to retry." | Pane stays dirty and the temp file is preserved; ⌘S retries the upload |
| Write fails (disk full, permissions) | Error notification | Original file intact — the write goes through a temp file and rename |
| Temp dir unwritable | Notification: "Cannot create temp file for editing" | No tab |
| File deleted between listing and open | Notification naming the path | No tab |

## Testing

**Rust (`cargo test`), in `editor_fs.rs`:**

- `guard_openable`: at the cap, one byte over, each blocklist entry, uppercase and mixed-case extensions, no extension, multiple dots, dotfiles.
- `looks_binary`: null at index 0, null at 8191, null at 8192 (must be missed — it is past the window), no nulls, empty input, input shorter than 8 KB.
- `editor_write_file`: the temp-and-rename path leaves the original intact when the write fails.
- `editor_temp_path`: the same filename on two hosts resolves differently; the same name at two remote paths resolves differently; basename and extension are preserved, including dotfiles and names with several dots.

**Frontend (`node scripts/tests/*.mjs`, matching the existing convention):**

All ten test files added by this branch:

- `test_language_map.mjs` — every mapped extension, case-insensitivity, the extensionless special cases, unknown extensions returning `null`, names that are only an extension (`.gitignore`).
- `test_scratch_naming.mjs` — first name in an empty directory, first free number with gaps, no collision with an existing file.
- `test_editor_close_guards.mjs` — guards refuse to close a dirty editor tab.
- `test_tab_close_dirty_guard.mjs` — tab-close flow returns 'cancel' when the user chooses not to discard.
- `test_window_close_answer.mjs` — window-close handler updates window state per the frontend's answer.
- `test_editor_unsaved_end_to_end.mjs` — the complete close flow: dirty check, dialog, decision routing.
- `test_editor_remote_transfer.mjs` — remote file transfer orchestration.
- `test_editor_save_inflight.mjs` — save state and re-entrancy on upload.
- `test_editor_save_race.mjs` — concurrent save requests.
- `test_shortcut_save_fallthrough.mjs` — `cmd+s` passes through in non-editor panes.

The four close-guard tests above drive the real `tl-dialog` through a hand-rolled DOM, giving them the most coverage of any feature module. The DOM-bound parts not covered — `editor-pane.js`, `theme.js` CodeMirror integration — have no automated tests; there is no jsdom in this repo, matching the precedent set by `test_tl_dialog.mjs`. They are covered by the manual pass below.

**Manual verification:**

1. `cmd+n` → type → `cmd+s` → close the tab → reopen the file from the SFTP local pane → the contents are there.
2. Double-click a local file in the SFTP local pane → edit → save → verify on disk.
3. Double-click a remote file → edit → save → verify on the remote from a terminal.
4. Kill the SSH connection, then save a remote-bound editor → error notification, and the tab stays marked dirty → reconnect → ⌘S again succeeds.
5. Double-click a 10 MB file → clean rejection, no transfer.
6. Double-click a `.jar` → blocklist rejection.
7. Double-click a file with a text extension but binary contents → rejection after download, temp file gone.
8. Modify an editor, then close the tab / close the window / quit — each prompts; Cancel aborts.
9. Split a terminal and an editor side by side; drag the editor tab to another position.
10. Change theme and font size in Settings → the editor follows.
11. Open the same remote file twice → the existing tab focuses; no duplicate.

## Known Limitations

1. No conflict detection — a remote file changed between open and save is overwritten.
2. No auto-reload for files changed underneath an open tab.
3. Non-UTF-8 files are read lossily; saving one rewrites the replacement characters.
4. Size cap and blocklist are hard-coded, not user-configurable.
5. Open tabs do not survive a restart — the app has no workspace-session restore for any pane kind.
6. No Save-All; each editor saves independently.
7. Scratch files accumulate on disk until the user deletes them.
8. Building the app now requires Node.
9. **The Dock's Quit and a system shutdown are not guarded on macOS.** Both send
   `terminate:` straight to `NSApp` without going through the app menu, so
   neither reaches the Quit menu item that the unsaved-changes poll hangs off,
   and unsaved editors are torn down without a prompt. This was equally true
   before the guards existed, but ⌘Q now asks and Dock → Quit still does not,
   which is a visible inconsistency. It appears unfixable at this layer: `tao`
   does not implement `applicationShouldTerminate:` (only
   `applicationWillTerminate:`, which fires after the decision is final), and
   Tauri raises `RunEvent::ExitRequested` only from `app.exit()` or the last
   window's destruction — never from `terminate:`. Closing it would mean
   patching or replacing the app delegate.

   The unsaved prompt is not the only casualty. `close_guard::finish_exit` is
   also where the remote-edit temp root is swept, so a Dock quit leaks the whole
   `termlab-sftp-edits` tree until the next launch's startup sweep clears it.
   Both losses come from the same bypass, and fixing the bypass fixes both.

10. **The same remote file opened in two windows shares one temp file, and
    closing either window breaks the other.** Both in-flight guards —
    `opensInFlight` and `focusExistingEditor` — are window-scoped, because each
    window is a separate JS context with its own copy of `editor-service.js`.
    `editor_temp_path` is a pure function of (host, path), so both windows
    resolve to the same file and both download onto it. Closing either runs
    `editor_temp_cleanup`, which deletes the file *and* climbs deleting the
    parent directories it empties, while `editor_write_file` does not recreate
    them — so the survivor's next save fails with "No such file or directory".
    Closing it needs the open-temp registry to live in Rust, where the path is
    computed: either a set of currently-open temp paths, or refcounting in
    `editor_temp_cleanup`.

11. **Editor transfers reuse the file explorer's progress bar and toasts.**
    Opening a remote file therefore raises a second "Transfer Complete" toast
    and briefly marks a same-named row in the local pane, as though the user had
    downloaded the file there.

12. **A dead host makes close and quit appear frozen** for up to 60 s per dirty
    remote pane: the guard cannot refuse until each pane's upload has timed out.

13. **The 5 MB pre-check trusts the directory listing's size.** A stale or lying
    listing gets the whole file downloaded before `editor_read_file` rejects it
    against the real bytes. The guard is not bypassed — only the saving of the
    transfer is.

14. **Scratch creation is not atomic.** There is no exclusive create, so a
    simultaneous ⌘N in two windows can have both pick the same free name and one
    truncate the other's scratch.

## Risks

- **Bundle size.** The full language set is roughly 1 MB minified. Measure after the first build; drop language packages if it is worse than expected.
- **The eight `kind === 'terminal'` guards.** The table above is from a grep, and a guard that needs an editor arm but does not get one fails silently rather than loudly — a font change that skips editor panes looks like nothing happened. Each needs a deliberate answer, not a pattern-matched one.
- **Close guards across four paths.** Window close, app quit, and the updater's restart go through Tauri, not the tab manager. An unguarded path silently discards a user's work, which is the worst failure this feature can have — and the restart path proved it, having shipped unguarded until it was caught.
- **A missing bundle fails silently.** `beforeBuildCommand` only fires under `cargo tauri build` / `cargo tauri dev`. Any path that builds with plain `cargo` — the two Linux release jobs, and a developer running `cargo run` — produces an app whose `index.html` references a bundle that does not exist, and the failure surfaces as an editor that quietly does nothing. `editor-service.js` must check for `window.CM6` at first use and show an explicit "editor bundle missing — run npm run build:vendor" notification rather than throwing into the void. The Makefile's DMG targets go through Tauri and are covered, but that needs confirming rather than assuming.

## Follow-ups

- Script Runner, built on this — the reason the editor was pulled ahead in the port order.
- Conflict detection (compare remote mtime before upload).
- Auto-reload for changed files.
- Configurable size cap and blocklist.
- A scratches browser.
- Workspace session restore — reopening tabs (terminal and editor alike) on launch.

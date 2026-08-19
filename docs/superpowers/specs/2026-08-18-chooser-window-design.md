# File Chooser as an Independent Window — Design

**Status:** Draft
**Date:** 2026-08-18
**Scope:** Move the unified file chooser out of the in-app `tl-dialog` overlay into its own OS window (`chooser.html`), window-modal over the requesting TermLab window, IntelliJ-save-dialog-style. Includes the extraction of the chooser UI from `file-dialog.js` that the 2026-08-18 whole-branch review mandated ("next feature here must extract, not append").

## Goals

1. The chooser is a real window: own titlebar, resizable with a content-fit floor, centered on its parent, remembered size.
2. Window-modal: the requesting TermLab window is grayed out and inert while its chooser is open; other TermLab windows are untouched and may run their own choosers concurrently.
3. No scrollable chrome: at any window size the sidebar, path bar, column header, and footer fit; the file listing is the only scrolling region. The size floor makes smaller impossible.
4. Zero call-site changes: `chooseFile(options)` keeps its exact signature and semantics; `savePane`, editor-service, `SaveCancelled`, and `choosersInFlight` are untouched.

## Non-Goals

- No native `NSOpenPanel`/`IFileDialog` (remote-host scopes require the custom UI — settled 2026-08-17).
- No changes to chooser behavior: sort, filter, hidden toggle, save-mode footer, overwrite confirm, `$HOME` start, no-default-extension all carry over as shipped.
- No pop-out tool windows in this project (they will reuse the window plumbing later; this spec does not design them).
- No app-modal option, no macOS sheets.

## Window & lifecycle (Rust: `chooser_window.rs`, registered from `lib.rs`)

- **Label:** `chooser-<parentLabel>-<reqId>` — unique per request, built once at registration and stored on the registry entry; the one-per-parent invariant lives only in the registry, never in the label scheme. (Unique because Tauri clears a destroyed window's label from its window map only when the `Destroyed` event round-trips the event loop, so rebuilding a replacement under the same label collides with `WindowLabelAlreadyExists`.) Every lookup and window-handle fetch uses the stored label; deriving the parent by parsing a `chooser-*` label is banned — parent labels contain dashes — so chooser-window-side callers (`get_chooser_request`, `resolve_file_chooser`, the CloseRequested hook) resolve themselves through the registry by exact window-label match. `open_file_chooser` called while an entry for that parent exists **cancels and recreates**: it takes the live entry out, resolves that session as cancelled through the normal path (`chooser-resolved { reqId: old, choice: null }` to the parent), registers a fresh entry with a fresh `req_id` and a fresh label, destroys the old chooser window (asynchronously — safe, since the replacement's label is different) and builds the fresh window immediately. It never hands the caller the existing `req_id`. *Why cancel-and-recreate:* every legitimate duplicate is already absorbed in the frontend — `activeChoice` shares the one promise for a same-mode repeat (invoking only `focus_file_chooser`) and refuses a cross-mode one — so an open that reaches Rust against a live entry is always an abnormal flow (the cancel/open IPC race, or a reloaded webview leaving a zombie chooser). Reusing the entry made such a caller adopt a window built for a different question: a save-mode request adopting an open-mode window gets a chooser with no filename field and no overwrite confirm, and `openForSave` then writes the pick straight over an existing file.
- **Command:** `open_file_chooser(window, mode: String, filename: Option<String>, select_filename: bool) -> Result<u64, String>` (as shipped — the request is built from these individual arguments plus the caller's own label, not a single `ChooserRequestArgs` struct), callable only from a main-app window (reject labels starting `chooser-`/`settings`). Builds on the main thread via `run_on_main_thread` (same deadlock rule as `open_new_window`, `windows.rs:42-51`).
- **Registry:** `ChooserRegistry` in managed state: `HashMap<parent_label, PendingChooser { req_id: u64, window_label: String, request: ChooserRequest }>`. `req_id` is a process-monotonic counter; every resolution event carries it, and the proxy drops events whose `req_id` is not the one it awaits (stale-event protection).
- **Builder:** `WebviewWindowBuilder::new(app, label, "chooser.html")`, `.parent(&parent_window)` (macOS/Windows owner relationship; on Linux it is best-effort and the focus bounce below covers it), `.title("Open")` / `"Save As"` by mode, `.inner_size(persisted or floor)`, `.min_inner_size(floor)`, `.resizable(true)`, `.decorations` and `.theme` per the settings-window rules (`windows.rs:78-98`), `.visible(false)`, menu removed, minimizable disabled where the platform supports it. Centered on the parent's current monitor position (computed from the parent's outer position/size before build; never moved after show).
- **Show:** the chooser frontend invokes `chooser_ready` when its first paint is committed; Rust then shows and focuses. `arm_window_show_fallback` (the existing rescue timer) guards a frontend that never reports.
- **Exactly-once resolution:** all exits funnel through the registry's `resolve(parent_label, req_id)` — user pick, Cancel/Escape, the window's close button (`CloseRequested`), parent `Destroyed`, and `cancel_file_chooser` (the `cancelForPane` path). `resolve` itself only removes and returns the matching entry (or `None` for a late/stale caller); the emit, size persistence and window close that follow a successful `resolve` are a separate completion step (`complete_chooser` as shipped) run by whichever caller won. First caller wins: it emits `chooser-resolved { reqId, choice }` to the parent window and closes the chooser window. Late callers find no entry and do nothing. `choice` is `Option<serde_json::Value>`, opaque to Rust (see Data flow, Outcome); `null`/absent is cancel. (The Share retry-duplication bug is the reason this is a spec requirement, not an implementation nicety.)
- **Parent death:** a `WindowEvent::Destroyed` listener for any label with a live chooser resolves that chooser as cancelled (the emit is a no-op on a dead parent) and closes the chooser window. The `.parent()` relationship is not trusted to do this on every platform.

## Modality (parent side)

- While its chooser is open the parent webview: raises a full-window scrim styled with the same tokens as the modal dialog scrim, and sets `inert` on the app root container — no clicks, no focus, no keyboard into the app content. The scrim is raised by the proxy before invoking `open_file_chooser` and lowered in a `finally` when the promise settles (including every error path — a stuck scrim is a locked app).
- Native menu accelerators (⌘O/⌘S/⌘⇧S) bypass the webview; they land in the existing `chooseFile` guards, which remain the modality's second wall: same mode → return the active promise, and the proxy additionally invokes `focus_file_chooser(parent_label)` so the answer window comes forward; different mode → `Promise.resolve(null)` refusal, unchanged.
- Focus bounce: while a chooser is registered, a `WindowEvent::Focused(true)` on its parent triggers `set_focus` on the chooser window (Rust side, works uniformly incl. Linux).
- Window close: the parent's close button while a chooser is open is allowed; parent death cancels the chooser (above) and the editor close guards behave exactly as if the chooser had been cancelled (`SaveCancelled` semantics unchanged).

## Frontend split

**`app/features/editor/file-dialog-view.js` (new)** — the entire chooser UI as shipped in the 2026-08-18 redesign, extracted verbatim-plus-reshaping from `file-dialog.js`: sidebar (Places/Hosts, `(pane N)` disambiguation, failed-scope retry), path bar, sortable columns, listing, footer controls, keyboard nav, sort-per-open, `navToken` staleness. Signature:

```
termlabFileDialogView.build(root, deps) -> { focusInitial() }
  deps = {
    data,                      // the files data-service object; every invoke the
                               // view makes goes through its functions, which all
                               // take the module's own ambient `invoke` first —
                               // no `invoke` is threaded through deps itself
    mode, filename, selectFilename,
    parentWindowLabel,         // session filter — the PARENT's label, from the request,
                               // never the chooser window's own label
    onResolve(choiceOrNull),   // called exactly once
  }
```

(As shipped, `confirm` is not a dep: `confirmOverwrite` reaches `global.tlDialog.open` directly, the same ambient-global pattern the view uses for `invoke` — both hosts load `tl-dialog.js`, so it is always present.)

The view builds into `root` (in the chooser window, `document.body`'s content root). It calls `onResolve` exactly once; `finish`-latch logic moves here with it. It contains no `tlDialog.open` call of its own for the chooser body (only for the overwrite confirm), no `activeChoice`, no scrim.

**`app/features/editor/file-dialog.js` (shrinks to the proxy)** — keeps `chooseFile(options)`, `activeChoice` (same-mode share / cross-mode refusal / `cancelForPane`), toast failure paths, and gains: scrim raise/lower, `open_file_chooser` invoke with the serialized request, one `chooser-resolved` listener filtered by `req_id`, rehydration of the outcome for callers. Resolves `{ scope, path, entry }` or `null`; never rejects. `cancelForPane` invokes `cancel_file_chooser` and resolves locally when the event echoes back (or immediately if the invoke fails — the scrim must always come down).

**`chooser.html` + `app/chooser-window-runtime.js` (new)** — head mirrors `settings.html` (tokens-dark/tokens-light, fonts, base, button/input/scrollbar/dialog/form/picker/file-dialog CSS; custom titlebar markup on Windows/Linux only, per the settings pattern, minus a minimize control — the window is `.minimizable(false)`, so a minimize button that can never work does not render). Scripts, in document order: `tauri-client.js` / `keyboard-router.js` / `config-service.js` (the same boot trio `settings.html` loads), an inline classic script that creates the Tauri client, applies theme colors, and activates the Windows/Linux custom titlebar (wiring its own maximize/close handlers directly — this inline script, not the runtime, owns the titlebar buttons), then `tl-icon.js`, `tl-dialog.js` (for the overwrite confirm only), `data-service.js`, `file-dialog-model.js`, `file-dialog-view.js`, and `chooser-window-runtime.js` last. The runtime itself is narrower than a first read of "wires the window" suggests: it invokes `get_chooser_request()` (Rust returns the `ChooserRequest` for this window's label), builds the view, awaits `chooser_ready`, and forwards the view's `onResolve` to `resolve_file_chooser(req_id, choice)`. Escape is the view's own root-scoped `keydown` listener (`file-dialog-view.js`), not something the runtime wires; the native OS close (traffic light) never touches JS at all — it raises `CloseRequested`, which Rust's `on_chooser_close_requested` hook answers directly. If `get_chooser_request` finds no entry (registry raced empty), the runtime closes the window itself — a chooser with no question must not linger.

## Data flow

- **Request:** `ChooserRequest { req_id, mode: "open"|"save", filename, select_filename, parent_label }` — as shipped, this carries no `start_dir`. `$HOME` is resolved chooser-side, inside `file-dialog-view.js`'s `loadScopes`, via the data-service's `getHomeDir` — the same rule the in-app dialog used, just invoked from the new host instead of threaded through the request. `pane` never crosses the boundary either way (it stays in the proxy's `activeChoice`, used only by `cancelForPane`).
- **Chooser-side data:** the view invokes `local_list_dir` / `sftp_list_dir` / `sftp_realpath` / `remote_get_sessions` directly. Session scopes are built by the existing `buildScopes` logic with `windowLabel = parentWindowLabel` — the current code reads its own window's label (`file-dialog.js:96-115` area); that read becomes a parameter. A session that dies mid-browse behaves exactly as a failed scope does today (error state, retry).
- **Outcome:** relayed as `Option<serde_json::Value>`, not a typed `ChooserChoice` — Rust never defines or parses the shape, only stores and forwards it verbatim (`resolve_file_chooser`'s `choice` parameter, `chooser_window.rs`), so a frontend shape change can't drift a Rust-side mirror out of sync. The value the view actually puts there is its existing plain-scope object: `{ kind: "local"|"remote", id, label, hostLabel, paneId, start }` for `scope`, plus `path` and `entry: FileEntry|null` alongside it — the same fields `buildScopes`/`enterScope` always produced, unchanged by the window move. The proxy passes this through to `openForOpen`/`openForSave` exactly as before; they still read only `scope.kind`/`scope.paneId`/`scope.hostLabel`.
- **Theme/appearance:** the chooser window gets `.theme()` at build; the runtime applies theme colors via `configService.applyThemeCss` the same way settings-window does. Neither window ever sets `data-tl-appearance` — nothing in the codebase does; `applyThemeCss`'s CSS custom properties are the whole mechanism on both hosts.

## Sizing & persistence

- **Floor (logical px): 720 × 420.** Derivation: width = sidebar 150 + path-bar flex-basis floors 478 + up button/gaps/padding ≈ 570 main; height = path bar + 24px header + 8 × 24px rows + footer + padding. The plan's first task re-derives both numbers by summing the actual CSS metrics and corrects the constant AND this spec line if they disagree; the floor is a named constant in one place on each side (Rust uses it for `min_inner_size`, CSS floors nothing — the window floor is the guarantee).
- **Persisted size:** `persistent state → chooser_window: { width, height }` (logical px), written on every chooser close from the window's final inner size, read at open, clamped to ≥ floor and ≤ the parent's monitor work area. No position persistence — always centered on parent.
- **Never resized after show** (the standing rule). The only scrolling region at any size is the listing; this is guaranteed by the floor, not by CSS clamps.

## Constraints

- CLAUDE.md: work on `feat/chooser-window`; no main commits; no Co-Authored-By trailers; unit tests required.
- Tokens-only CSS; boundary script passes (known pre-existing failure `tl-dialog.js:334` only).
- The view keeps every DOM class name from the redesign; `test_file_dialog.mjs`'s original 47 checks repoint their entry (build view directly instead of `chooseFile`) with zero behavioral assertions weakened, and the suite has since grown to 56 as shipped (Task 4's runtime checks added on top). The four race/guard suites (`test_editor_save_race`, `test_editor_save_inflight`, `test_shortcut_save_fallthrough`, `test_editor_remote_transfer`) exercise the proxy layer and must pass against the stubbed event transport without assertion changes.
- `file-dialog.css` adapts selectors from dialog-body context to window-body context but keeps class names; visual layout is unchanged except chrome ownership (window titlebar instead of tl-dialog header).

## Testing

- **View:** `test_file_dialog.mjs` drives `file-dialog-view.build` with a stub `invoke`/`confirm`; all 47 original checks preserved, 56 total as shipped; new checks — `onResolve` exactly-once under double-Enter and Escape-after-pick races; `parentWindowLabel` (not own label) reaches the session filter, pinned with a fixture where the two labels disagree and only the parent's sessions may appear; and the chooser-window-runtime checks from Task 4 (`get_chooser_request` → build → `chooser_ready` → `resolve_file_chooser` wiring, and the no-pending-request self-close path).
- **Proxy:** new `test_file_dialog_proxy.mjs` — same-mode promise sharing, cross-mode null refusal, scrim raised before invoke and lowered on resolve/cancel/invoke-failure (the `finally` is load-bearing), stale `req_id` events ignored, `cancelForPane` settles null and lowers the scrim.
- **Rust:** registry unit tests — exactly-once under racing resolvers, entry removed before emit, parent-death cleanup, a second open displacing the live entry and minting a fresh `req_id` AND a fresh `window_label` (never reusing either), window-label lookup finding the live entry by exact match and missing a displaced entry's stale label, a `cancel_file_chooser` carrying a stale `req_id` being a no-op that leaves the live entry alone, rejection of chooser-label callers.
- **Manual (checklist section I):** modality gray-out and focus bounce; accelerator behavior (⌘O twice, ⌘O over save chooser); resize floor stops at content-fit; size remembered across opens; two windows × two simultaneous choosers; SSH scopes incl. mid-browse disconnect; overwrite confirm inside the chooser window; both themes; parent close with chooser open.

## Known limitations

- Linux `.parent()` support is best-effort; the Rust focus bounce is the cross-platform guarantee of modal behavior.
- The scrim/`inert` disables the parent's webview content; native window chrome (traffic lights, resize) stays live — closing the parent is allowed and cancels the chooser (IntelliJ behaves the same way).
- Light theme remains approximate app-wide (token-pipeline gap, documented in the 2026-08-18 redesign spec); the chooser window inherits it equally.

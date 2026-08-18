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

- **Label:** `chooser-<parentLabel>` — the one-per-parent invariant is the label scheme. `open_file_chooser` called while that label exists focuses the existing window and returns the existing `req_id` (the frontend proxy has already handed back the shared promise; see Modality).
- **Command:** `open_file_chooser(window, req: ChooserRequestArgs) -> req_id`, callable only from a main-app window (reject labels starting `chooser-`/`settings`). Builds on the main thread via `run_on_main_thread` (same deadlock rule as `open_new_window`, `windows.rs:42-51`).
- **Registry:** `ChooserRegistry` in managed state: `HashMap<parent_label, PendingChooser { req_id: u64, request: ChooserRequest }>`. `req_id` is a process-monotonic counter; every resolution event carries it, and the proxy drops events whose `req_id` is not the one it awaits (stale-event protection).
- **Builder:** `WebviewWindowBuilder::new(app, label, "chooser.html")`, `.parent(&parent_window)` (macOS/Windows owner relationship; on Linux it is best-effort and the focus bounce below covers it), `.title("Open")` / `"Save As"` by mode, `.inner_size(persisted or floor)`, `.min_inner_size(floor)`, `.resizable(true)`, `.decorations` and `.theme` per the settings-window rules (`windows.rs:78-98`), `.visible(false)`, menu removed, minimizable disabled where the platform supports it. Centered on the parent's current monitor position (computed from the parent's outer position/size before build; never moved after show).
- **Show:** the chooser frontend invokes `chooser_ready` when its first paint is committed; Rust then shows and focuses. `arm_window_show_fallback` (the existing rescue timer) guards a frontend that never reports.
- **Exactly-once resolution:** all exits funnel through one registry method `resolve(parent_label, req_id, outcome)` — user pick, Cancel/Escape, the window's close button (`CloseRequested`), parent `Destroyed`, and `cancel_file_chooser` (the `cancelForPane` path). First caller wins: it removes the registry entry, emits `chooser:resolved { req_id, outcome }` to the parent window, and closes the chooser window. Late callers find no entry and do nothing. `outcome` is `{ choice: ChooserChoice | null }`; `null` is cancel. (The Share retry-duplication bug is the reason this is a spec requirement, not an implementation nicety.)
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
    invoke,                    // Tauri invoke
    mode, filename, selectFilename,
    parentWindowLabel,         // session filter — the PARENT's label, from the request,
                               // never the chooser window's own label
    confirm,                   // (title, body) -> Promise<boolean>, overwrite confirm
    onResolve(choiceOrNull),   // called exactly once
  }
```

The view builds into `root` (in the chooser window, `document.body`'s content root). It calls `onResolve` exactly once; `finish`-latch logic moves here with it. It contains no `tlDialog`, no `activeChoice`, no scrim.

**`app/features/editor/file-dialog.js` (shrinks to the proxy)** — keeps `chooseFile(options)`, `activeChoice` (same-mode share / cross-mode refusal / `cancelForPane`), toast failure paths, and gains: scrim raise/lower, `open_file_chooser` invoke with the serialized request, one `chooser:resolved` listener filtered by `req_id`, rehydration of the outcome for callers. Resolves `{ scope, path, entry }` or `null`; never rejects. `cancelForPane` invokes `cancel_file_chooser` and resolves locally when the event echoes back (or immediately if the invoke fails — the scrim must always come down).

**`chooser.html` + `app/chooser-window-runtime.js` (new)** — head mirrors `settings.html` (tokens, fonts, base, button/input/scrollbar/dialog/file-dialog CSS; custom titlebar markup on Windows/Linux only, per the settings pattern). Scripts: `tl-icon.js`, `tl-dialog.js` (for the overwrite confirm only), `file-dialog-model.js`, `file-dialog-view.js`, the runtime. The runtime: invokes `get_chooser_request()` (Rust returns the `ChooserRequest` for this window's label), builds the view, wires window-level Escape → resolve-null and the titlebar close, invokes `chooser_ready`, and forwards `onResolve` to `resolve_file_chooser(req_id, outcome)`. If `get_chooser_request` finds no entry (registry raced empty), the runtime invokes self-close — a chooser with no question must not linger.

## Data flow

- **Request:** `{ req_id, mode: "open"|"save", filename, select_filename, parent_label, start_dir }`. `start_dir` is `$HOME` resolved by the existing rule; `pane` never crosses the boundary (it stays in the proxy's `activeChoice`, used only by `cancelForPane`).
- **Chooser-side data:** the view invokes `local_list_dir` / `sftp_list_dir` / `sftp_realpath` / `remote_get_sessions` directly. Session scopes are built by the existing `buildScopes` logic with `windowLabel = parentWindowLabel` — the current code reads its own window's label (`file-dialog.js:96-115` area); that read becomes a parameter. A session that dies mid-browse behaves exactly as a failed scope does today (error state, retry).
- **Outcome:** `ChooserChoice { scope: { kind: "local"|"remote", session_key, label }, path, entry: FileEntry|null }` — plain data, serde on the Rust side, no live object crosses windows. The proxy rehydrates the scope shape `savePane`/open-file consume today (verified at plan time against the current scope object's consumed fields; the plan lists them explicitly).
- **Theme/appearance:** the chooser window gets `.theme()` at build; runtime applies `data-tl-appearance` the same way settings-window does.

## Sizing & persistence

- **Floor (logical px): 720 × 420.** Derivation: width = sidebar 150 + path-bar flex-basis floors 478 + up button/gaps/padding ≈ 570 main; height = path bar + 24px header + 8 × 24px rows + footer + padding. The plan's first task re-derives both numbers by summing the actual CSS metrics and corrects the constant AND this spec line if they disagree; the floor is a named constant in one place on each side (Rust uses it for `min_inner_size`, CSS floors nothing — the window floor is the guarantee).
- **Persisted size:** `persistent state → chooser_window: { width, height }` (logical px), written on every chooser close from the window's final inner size, read at open, clamped to ≥ floor and ≤ the parent's monitor work area. No position persistence — always centered on parent.
- **Never resized after show** (the standing rule). The only scrolling region at any size is the listing; this is guaranteed by the floor, not by CSS clamps.

## Constraints

- CLAUDE.md: work on `feat/chooser-window`; no main commits; no Co-Authored-By trailers; unit tests required.
- Tokens-only CSS; boundary script passes (known pre-existing failure `tl-dialog.js:334` only).
- The view keeps every DOM class name from the redesign; `test_file_dialog.mjs`'s 47 checks repoint their entry (build view directly instead of `chooseFile`) with zero behavioral assertions weakened. The four race/guard suites (`test_editor_save_race`, `test_editor_save_inflight`, `test_shortcut_save_fallthrough`, `test_editor_remote_transfer`) exercise the proxy layer and must pass against the stubbed event transport without assertion changes.
- `file-dialog.css` adapts selectors from dialog-body context to window-body context but keeps class names; visual layout is unchanged except chrome ownership (window titlebar instead of tl-dialog header).

## Testing

- **View:** `test_file_dialog.mjs` drives `file-dialog-view.build` with a stub `invoke`/`confirm`; all 47 checks preserved; new checks — `onResolve` exactly-once under double-Enter and Escape-after-pick races; `parentWindowLabel` (not own label) reaches the session filter, pinned with a fixture where the two labels disagree and only the parent's sessions may appear.
- **Proxy:** new `test_file_dialog_proxy.mjs` — same-mode promise sharing, cross-mode null refusal, scrim raised before invoke and lowered on resolve/cancel/invoke-failure (the `finally` is load-bearing), stale `req_id` events ignored, `cancelForPane` settles null and lowers the scrim.
- **Rust:** registry unit tests — exactly-once under racing resolvers, entry removed before emit, parent-death cleanup, `open_file_chooser` returning the live `req_id` for a duplicate request, rejection of chooser-label callers.
- **Manual (checklist section I):** modality gray-out and focus bounce; accelerator behavior (⌘O twice, ⌘O over save chooser); resize floor stops at content-fit; size remembered across opens; two windows × two simultaneous choosers; SSH scopes incl. mid-browse disconnect; overwrite confirm inside the chooser window; both themes; parent close with chooser open.

## Known limitations

- Linux `.parent()` support is best-effort; the Rust focus bounce is the cross-platform guarantee of modal behavior.
- The scrim/`inert` disables the parent's webview content; native window chrome (traffic lights, resize) stays live — closing the parent is allowed and cancels the chooser (IntelliJ behaves the same way).
- Light theme remains approximate app-wide (token-pipeline gap, documented in the 2026-08-18 redesign spec); the chooser window inherits it equally.

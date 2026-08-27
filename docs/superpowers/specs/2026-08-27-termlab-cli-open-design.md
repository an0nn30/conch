# `termlab` CLI Open + Install-to-PATH — Design

**Date:** 2026-08-27
**Status:** Approved in chat (spec pending user review)
**Owner request:** `termlab ~/file.txt` from a shell opens a text editor in a
new TermLab window. A command-palette entry installs `termlab` into PATH the
way VS Code's "Install 'code' command" does. Directories will be supported
later (LSP/workspace work in progress) — do not box the design into files.

## Summary

The existing app binary (already named `termlab`) becomes dual-mode: with
path arguments it acts as a CLI — if a running instance's IPC socket
answers, it forwards a generic `open-path` message and exits; otherwise it
boots as the app with the paths queued as pending opens. Both arrival paths
converge on one Rust function that opens a new app window and parks the path
in a per-window-label pending map the new window's frontend pulls during
startup (the panel-host request-pull pattern). Today the frontend routes
files to the editor and answers directories with a "not yet" toast; the
future directory/workspace route replaces only that frontend branch. A
palette command symlinks `/usr/local/bin/termlab` to the running binary,
escalating through the macOS administrator prompt when the direct write is
denied, with an uninstall twin.

## Current state (what this builds on)

- `crates/termlab_tauri/src/main.rs` ignores argv entirely; the binary is
  `[[bin]] name = "termlab"`.
- `crates/termlab_tauri/src/ipc.rs` listens on a Unix socket
  (`ipc_socket_path()`), parsing newline-delimited JSON `IpcMessage`
  variants `CreateWindow` / `CreateTab`. No CLI sender survives in-tree.
- `crates/termlab_tauri/src/windows.rs::create_new_window(app)` allocates
  labels `window-{id}` and builds an ordinary app window; it returns
  `tauri::Result<()>` today.
- `frontend/app/features/editor/editor-service.js` exposes
  `openLocalFile(filePath)` (reads the file via the backend, creates an
  editor tab, focuses an existing tab for the same path).
- The command palette adds entries via `buildPaletteCommands()`'s `add(id,
  title, subtitle, keywords, run, group)` in
  `frontend/app/command-palette-runtime.js`.
- The panel host demonstrates the request-pull boot pattern
  (`get_panel_host_request`): a new window asks Rust "what am I for?"
  instead of racing an event.

## Design

### CLI entry (main.rs + a new `cli` module)

`main()` calls `termlab_tauri::cli::run_cli_if_requested()` before any Tauri
setup (after `env_logger::init()`):

- **Arg parsing** (pure function, unit-tested): everything in
  `std::env::args().skip(1)` that is not a recognized subcommand or flag is
  a path. `msg …` (the documented IPC subcommand namespace) is reserved and
  left exactly as today (untouched — it is currently unimplemented in-tree
  and stays that way). `--help`/`-h` and `--version`/`-V` print and exit.
  Unknown `-`/`--` flags error to stderr, exit 2.
- **Path normalization** (pure, tested): each path is made absolute against
  the process cwd (`std::env::current_dir().join(p)` when relative), then
  lexically cleaned (no filesystem access — a nonexistent path must still
  travel to the app, which owns the error UX). Tilde is the shell's job.
- **Dispatch:** with zero paths, return `None` (boot the app normally).
  With paths: try `UnixStream::connect(ipc_socket_path())`. Connected →
  write one `{"type":"open-path","path":…}` line per path, exit 0.
  Not connected → return the paths; `main()` passes them into
  `termlab_tauri::run(user_config, pending_paths)` which seeds the pending
  queue before window creation, and the first window consumes the first
  path (see routing) rather than opening an extra empty window.
- Windows note: the IPC listener is unix-only today (`#[cfg(unix)]`); the
  CLI's socket fast-path is likewise unix-only, and on other platforms the
  boot-with-pending path is the only path. Same conditional structure
  `ipc.rs` already uses.

### IPC message (ipc.rs)

New variant, deliberately path-shaped rather than file-shaped:

```rust
#[serde(rename = "open-path")]
OpenPath { path: String },
```

Handler: `crate::open_path::open_in_new_window(&app, &path)` (below).
Serde round-trip and unknown-variant tolerance covered by tests alongside
the existing message tests.

### Open routing (new module `src/open_path.rs`)

One converging function pair, keyed by window label:

- `open_in_new_window(app, path)`: change `windows::create_new_window` to
  return `tauri::Result<String>` (the label it allocated; existing callers
  ignore the value), enqueue `(label → Vec<path>)` in a
  `Mutex<HashMap<String, Vec<String>>>` managed state, and rely on the pull
  below. Decision: multiple CLI paths in one invocation open one new window
  per path (the owner request is "opens … in a new window"; VS Code's
  one-window-many-tabs behavior is a possible later refinement, not v1).
- Boot-pending seeding: `termlab_tauri::run` puts the CLI paths into the
  same map under the FIRST window's label before that window is created,
  so the pull is identical for both arrival paths.
- `take_pending_open_paths` (Tauri command): returns and removes
  `Vec<String>` for the calling window's label (empty when none). Pull-only,
  no events — a window that boots slowly still gets its paths.

### Frontend routing

In the window startup path (same phase that initializes the editor service
— implementer anchors it where `orchestration-runtime`/`event-wiring`
finishes wiring `termlabEditorService`), call
`invoke('take_pending_open_paths')`; for each returned path:

- `stat` via the existing local-fs command surface: a regular file →
  `editorService.openLocalFile(path)`; missing → error toast naming the
  path; a directory → info toast "Directory workspaces aren't supported
  yet" (STRING LIVES IN ONE PLACE — this branch is the single seam the
  LSP/workspace work later replaces).
- The routing lives in a small new module
  `frontend/app/features/editor/open-path-routing.js` (pure decision logic
  exported for VM tests; DOM/service calls injected).

### Install to PATH (palette + `src/cli_install.rs`)

- Palette entries (group `Application`):
  - `Install 'termlab' Command in PATH` → `invoke('install_cli_symlink')`
  - `Uninstall 'termlab' Command from PATH` → `invoke('uninstall_cli_symlink')`
  Success/failure surfaces as toasts with the resolved paths in the body.
- `install_cli_symlink`: resolve `std::env::current_exe()` (canonicalized —
  inside the .app bundle on macOS this is
  `TermLab.app/Contents/MacOS/termlab`, which is exactly the right target);
  target link `/usr/local/bin/termlab`.
  1. If the link already resolves to the current exe → success (idempotent).
  2. Try direct: remove a stale link if it points into a TermLab bundle
     (never delete an unrelated file — error instead), then `symlink`.
  3. On `PermissionDenied` (macOS): one
     `osascript -e "do shell script \"ln -sf <exe> /usr/local/bin/termlab\" with administrator privileges"`
     (both paths shell-escaped); non-zero status → error with stderr.
     On Linux: report the `ln -sf` command for the user to run with sudo
     (no pkexec dependency in v1).
- `uninstall_cli_symlink`: remove the link only if it resolves into a
  TermLab bundle/exe; same direct-then-osascript escalation.
- Pure helpers (`symlink_decision(current_exe, link_target_state)` →
  Install/AlreadyInstalled/RefuseForeignFile, and the osascript
  command-string builder with escaping) are unit-tested; the commands are
  thin shells around them.

### Out of scope

- Opening directories (the frontend branch is the designed seam; nothing
  else changes later).
- Reusing an existing window / `--reuse-window`-style flags.
- Windows PATH install; Linux privilege escalation beyond printing the
  command.
- Shell completion, `termlab msg` sender implementation.

## Testing

- **Rust:** arg-parser table tests (paths vs flags vs `msg` passthrough,
  help/version, unknown flag exit); path normalization (relative→absolute,
  lexical clean, absolute untouched); `IpcMessage::OpenPath` serde
  round-trip + legacy messages unaffected; pending-map take-once semantics
  (take empties, unknown label → empty, per-label isolation);
  `symlink_decision` cases (fresh install, already-correct link, stale
  TermLab link, foreign file refusal) and osascript string escaping.
- **Frontend (VM):** `open-path-routing` decision logic (file→editor call,
  directory→coming-soon toast, missing→error toast); a wiring test that the
  startup path invokes `take_pending_open_paths` and routes results (harness
  precedent: the panel-host suites' invoke recorders).
- **Manual:** install command (admin prompt appears once, `which termlab`
  resolves); `termlab ~/file.txt` with app running (new window + editor) and
  with app closed (app launches into the file); relative path from a deep
  cwd; nonexistent path toast; directory toast; uninstall; `termlab` with
  no args from PATH launches the app.

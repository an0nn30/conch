# `termlab` CLI Open + Install-to-PATH Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `termlab <path>` opens the file in an editor in a new window (forwarding to a running instance over IPC, or booting the app with the open queued), and a palette command installs/uninstalls the `termlab` symlink in `/usr/local/bin` VS Code-style.

**Architecture:** The app binary goes dual-mode in `main()` via a new `cli` module. Both arrival paths (IPC `open_path` message, boot-pending CLI args) converge on `open_path::` state — a per-window-label pending map new windows drain with a pull command, mirroring the panel-host request pattern. The frontend's file-vs-directory routing branch is the single seam future directory/workspace support replaces.

**Tech Stack:** Rust/Tauri v2 (Unix-socket IPC in `ipc.rs`, commands in `lib.rs`'s `generate_handler`), vanilla JS IIFE frontend with VM-loaded node suites in `scripts/tests/`.

**Spec:** `docs/superpowers/specs/2026-08-27-termlab-cli-open-design.md`

## Global Constraints

- Branch: all work on `feat/termlab-cli-open` in `/Users/dustin/projects/conch` (`git branch --show-current` before every commit; never commit to main).
- No Co-Authored-By; imperative commit messages.
- Wire format correction to the spec: the `IpcMessage` enum carries `#[serde(tag = "type", rename_all = "snake_case")]`, so the new variant's wire tag is `open_path` (snake_case), not the spec's illustrative `open-path`. The enum's convention is authoritative; every producer/consumer in this plan uses `{"type":"open_path","path":…}`.
- Paths travel to the app even when nonexistent (the app owns the error UX); the CLI never stats them.
- The install/uninstall commands must never delete or overwrite a file that is not a TermLab symlink.
- The unix-only conditionals mirror `ipc.rs`'s existing `#[cfg(unix)]` structure; non-unix builds compile with the boot-pending path only.
- Node suites: `node scripts/tests/<file>.mjs`; VM realm — JSON-roundtrip before `deepStrictEqual`; harnesses must not define `sandbox.global`.
- After every task: task suite green, full node sweep (`for f in scripts/tests/*.mjs; do node "$f" >/dev/null 2>&1 || echo "FAIL: $f"; done` prints nothing), and for Rust tasks `cargo test -p termlab_tauri --quiet` green.

---

### Task 1: `open_path` module — pending map, pull command, new-window routing

**Files:**
- Create: `crates/termlab_tauri/src/open_path.rs`
- Modify: `crates/termlab_tauri/src/lib.rs` (module decl; `generate_handler` list ~line 794; manage the state in the builder where other `.manage(...)` calls live)
- Modify: `crates/termlab_tauri/src/windows.rs:100` (`create_new_window` returns the label)
- Test: `#[cfg(test)]` in `open_path.rs`

**Interfaces:**
- Consumes: `windows::create_new_window(app)` (return type changes here to `tauri::Result<String>` — the allocated `window-{id}` label; grep every existing caller and ignore the value with `let _ =` / `?; ` as each site's error handling already does).
- Produces (Tasks 2-3 rely on these exact names): `pub(crate) struct PendingOpens(Mutex<HashMap<String, Vec<String>>>)` managed via `app.manage(PendingOpens::default())`; `pub(crate) fn open_in_new_window<R: tauri::Runtime>(app: &tauri::AppHandle<R>, path: &str)`; `pub(crate) fn seed_for_label(pending: &PendingOpens, label: &str, paths: Vec<String>)`; Tauri command `take_pending_open_paths(window, state) -> Vec<String>`.

- [ ] **Step 1: Write the failing tests** in `open_path.rs`'s `#[cfg(test)]`:

```rust
#[test]
fn take_returns_and_clears_per_label() {
    let pending = PendingOpens::default();
    seed_for_label(&pending, "main", vec!["/a.txt".into(), "/b.txt".into()]);
    seed_for_label(&pending, "window-1", vec!["/c.txt".into()]);
    assert_eq!(pending.take("main"), vec!["/a.txt".to_string(), "/b.txt".to_string()]);
    assert!(pending.take("main").is_empty(), "take drains");
    assert_eq!(pending.take("window-1"), vec!["/c.txt".to_string()]);
}

#[test]
fn take_unknown_label_is_empty() {
    let pending = PendingOpens::default();
    assert!(pending.take("window-9").is_empty());
}

#[test]
fn seed_appends_rather_than_replaces() {
    let pending = PendingOpens::default();
    seed_for_label(&pending, "main", vec!["/a.txt".into()]);
    seed_for_label(&pending, "main", vec!["/b.txt".into()]);
    assert_eq!(pending.take("main").len(), 2);
}
```

- [ ] **Step 2: Run** `cargo test -p termlab_tauri --quiet` — compile FAIL (module missing), the red state for new-module work.

- [ ] **Step 3: Implement `open_path.rs`:**

```rust
//! CLI/IPC "open this path" routing: each request opens a new app window and
//! parks the path under that window's label until the window's frontend
//! pulls it with `take_pending_open_paths` — the same request-pull boot
//! pattern the panel host uses, so a slow-booting window cannot miss an
//! event that fired before it subscribed.

use std::collections::HashMap;
use std::sync::Mutex;

#[derive(Default)]
pub(crate) struct PendingOpens(Mutex<HashMap<String, Vec<String>>>);

impl PendingOpens {
    pub(crate) fn take(&self, label: &str) -> Vec<String> {
        self.0.lock().unwrap().remove(label).unwrap_or_default()
    }
}

pub(crate) fn seed_for_label(pending: &PendingOpens, label: &str, mut paths: Vec<String>) {
    pending
        .0
        .lock()
        .unwrap()
        .entry(label.to_string())
        .or_default()
        .append(&mut paths);
}

pub(crate) fn open_in_new_window<R: tauri::Runtime>(app: &tauri::AppHandle<R>, path: &str) {
    use tauri::Manager;
    match crate::windows::create_new_window(app) {
        Ok(label) => {
            let pending = app.state::<PendingOpens>();
            seed_for_label(&pending, &label, vec![path.to_string()]);
        }
        Err(e) => log::error!("open-path: could not create window for {path}: {e}"),
    }
}

#[tauri::command]
pub(crate) fn take_pending_open_paths(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, PendingOpens>,
) -> Vec<String> {
    state.take(window.label())
}
```

`windows.rs`: change `create_new_window`'s signature to `-> tauri::Result<String>`, and at the end return `Ok(label)` (the `label` local from line 101 — clone it before the builder consumes it if needed). Update every caller (`grep -rn "create_new_window" crates/termlab_tauri/src/`) to discard the value. In `lib.rs`: `mod open_path;` (alongside the other module decls), `.manage(open_path::PendingOpens::default())` in the builder chain, and `open_path::take_pending_open_paths` added to `generate_handler!`.

- [ ] **Step 4: Run** `cargo test -p termlab_tauri --quiet` — PASS; `cargo test --workspace --quiet` — PASS.

- [ ] **Step 5: Commit** — `git add crates/termlab_tauri/src/open_path.rs crates/termlab_tauri/src/lib.rs crates/termlab_tauri/src/windows.rs && git commit -m "Add pending open-path queue with per-window pull"`

---

### Task 2: CLI dual-mode entry + IPC `open_path` message

**Files:**
- Create: `crates/termlab_tauri/src/cli.rs`
- Modify: `crates/termlab_tauri/src/main.rs` (dispatch before Tauri boot)
- Modify: `crates/termlab_tauri/src/ipc.rs` (enum variant + handler arm)
- Modify: `crates/termlab_tauri/src/lib.rs:255` (`pub fn run(config: UserConfig, pending_paths: Vec<String>)`)
- Test: `#[cfg(test)]` in `cli.rs` and `ipc.rs`

**Interfaces:**
- Consumes: Task 1's `open_path::{seed_for_label, open_in_new_window, PendingOpens}`; `ipc::ipc_socket_path()`.
- Produces: `pub enum CliAction { RunApp { pending_paths: Vec<String> }, Exit(i32) }`; `pub fn evaluate(args: &[String], cwd: &Path) -> CliDecision` (pure — see below); `pub fn run_cli_if_requested() -> CliAction` (does the I/O); `run(config, pending_paths)` seeds label `"main"` before the first window is created.

- [ ] **Step 1: Write the failing tests** in `cli.rs`:

```rust
// The pure decision layer: what should the process do for these args?
// No sockets, no filesystem — I/O lives in run_cli_if_requested().
#[derive(Debug, PartialEq)]
pub enum CliDecision {
    RunApp,                                  // no paths → normal app boot
    ForwardOrRun { paths: Vec<String> },     // absolute paths to open
    PrintHelp,
    PrintVersion,
    ReservedSubcommand,                      // "msg …" — untouched namespace
    UnknownFlag(String),                     // exit 2 with stderr
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn eval(args: &[&str]) -> CliDecision {
        let owned: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        evaluate(&owned, Path::new("/home/dustin/proj"))
    }

    #[test]
    fn no_args_runs_app() {
        assert_eq!(eval(&[]), CliDecision::RunApp);
    }

    #[test]
    fn relative_path_becomes_absolute_against_cwd() {
        assert_eq!(
            eval(&["notes.md"]),
            CliDecision::ForwardOrRun { paths: vec!["/home/dustin/proj/notes.md".into()] }
        );
    }

    #[test]
    fn dot_segments_are_lexically_cleaned() {
        assert_eq!(
            eval(&["../other/./a.txt"]),
            CliDecision::ForwardOrRun { paths: vec!["/home/dustin/other/a.txt".into()] }
        );
    }

    #[test]
    fn absolute_path_untouched() {
        assert_eq!(
            eval(&["/tmp/x.txt"]),
            CliDecision::ForwardOrRun { paths: vec!["/tmp/x.txt".into()] }
        );
    }

    #[test]
    fn multiple_paths_keep_order() {
        let CliDecision::ForwardOrRun { paths } = eval(&["a.txt", "/b.txt"]) else { panic!() };
        assert_eq!(paths, vec!["/home/dustin/proj/a.txt".to_string(), "/b.txt".to_string()]);
    }

    #[test]
    fn help_version_msg_and_unknown_flags() {
        assert_eq!(eval(&["--help"]), CliDecision::PrintHelp);
        assert_eq!(eval(&["-h"]), CliDecision::PrintHelp);
        assert_eq!(eval(&["--version"]), CliDecision::PrintVersion);
        assert_eq!(eval(&["-V"]), CliDecision::PrintVersion);
        assert_eq!(eval(&["msg", "new-window"]), CliDecision::ReservedSubcommand);
        assert_eq!(eval(&["--reuse-window", "a.txt"]), CliDecision::UnknownFlag("--reuse-window".into()));
    }
}
```

And in `ipc.rs`'s tests (add a `#[cfg(test)]` module if none exists, matching workspace style):

```rust
#[test]
fn open_path_message_parses() {
    let msg: IpcMessage =
        serde_json::from_str(r#"{"type":"open_path","path":"/tmp/a.txt"}"#).unwrap();
    assert!(matches!(msg, IpcMessage::OpenPath { path } if path == "/tmp/a.txt"));
}

#[test]
fn legacy_messages_still_parse() {
    assert!(serde_json::from_str::<IpcMessage>(r#"{"type":"create_window"}"#).is_ok());
    assert!(serde_json::from_str::<IpcMessage>(r#"{"type":"create_tab"}"#).is_ok());
}
```

(If `IpcMessage` is private and untestable from a child mod, the test module lives inside `ipc.rs` itself, which sees it.)

- [ ] **Step 2: Run** `cargo test -p termlab_tauri --quiet` — compile FAIL.

- [ ] **Step 3: Implement.**

1. `cli.rs`: `evaluate(args, cwd)` — iterate args: first arg `msg` → `ReservedSubcommand` (leave everything after untouched); `--help|-h` / `--version|-V` anywhere before paths → print variants; any other arg starting with `-` → `UnknownFlag`; everything else joins `paths` after normalization: `if relative { cwd.join(p) } else { p }` then lexical clean (fold `.`/`..` components via `std::path::Component` iteration — no `canonicalize`, no fs access; `..` at root stays at root). Then:

```rust
pub fn run_cli_if_requested() -> CliAction {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("/"));
    match evaluate(&args, &cwd) {
        CliDecision::RunApp => CliAction::RunApp { pending_paths: vec![] },
        CliDecision::ReservedSubcommand => CliAction::RunApp { pending_paths: vec![] },
        CliDecision::PrintHelp => { println!("{HELP_TEXT}"); CliAction::Exit(0) }
        CliDecision::PrintVersion => { println!("termlab {}", env!("CARGO_PKG_VERSION")); CliAction::Exit(0) }
        CliDecision::UnknownFlag(f) => { eprintln!("termlab: unknown flag '{f}'"); CliAction::Exit(2) }
        CliDecision::ForwardOrRun { paths } => forward_or_run(paths),
    }
}
```

`forward_or_run` (unix): `UnixStream::connect(crate::ipc::ipc_socket_path())`; on success write one `{"type":"open_path","path":<json-escaped>}\n` per path (serialize with `serde_json::json!`, never string-format), flush, `CliAction::Exit(0)`; on connect failure `CliAction::RunApp { pending_paths: paths }`. Non-unix: `CliAction::RunApp { pending_paths: paths }` directly. `HELP_TEXT` is a `const &str` naming the one usage: `termlab [PATH ...]` plus the flags.
2. `main.rs`: after `env_logger::init()` and before `platform::init()`:

```rust
    let pending_paths = match termlab_tauri::cli::run_cli_if_requested() {
        termlab_tauri::cli::CliAction::Exit(code) => std::process::exit(code),
        termlab_tauri::cli::CliAction::RunApp { pending_paths } => pending_paths,
    };
```

and pass into `termlab_tauri::run(user_config, pending_paths)`. (`cli` and its types become `pub` in lib.rs's module tree for main.rs to reach.)
3. `lib.rs` `run(config: UserConfig, pending_paths: Vec<String>)`: after `.manage(open_path::PendingOpens::default())` exists on the app (Task 1), seed inside the setup closure before/where the `"main"` window is created: `open_path::seed_for_label(&app.state::<open_path::PendingOpens>(), "main", pending_paths)` (guard the call on non-empty to avoid a pointless lock). The `"main"` label is the first window's literal label (`lib.rs:363` shows `get_webview_window("main")`).
4. `ipc.rs`: add the variant `OpenPath { path: String },` and the handler arm:

```rust
                        Ok(IpcMessage::OpenPath { path }) => {
                            crate::open_path::open_in_new_window(&app, &path);
                        }
```

- [ ] **Step 4: Run** `cargo test -p termlab_tauri --quiet` then `cargo test --workspace --quiet` — PASS. `cargo build -p termlab_tauri` must also succeed for non-test code paths.

- [ ] **Step 5: Commit** — `git add crates/termlab_tauri/src/cli.rs crates/termlab_tauri/src/main.rs crates/termlab_tauri/src/ipc.rs crates/termlab_tauri/src/lib.rs && git commit -m "Make the termlab binary a dual-mode CLI that forwards open-path"`

---

### Task 3: Frontend routing — pull pending paths, route file vs directory

**Files:**
- Create: `crates/termlab_tauri/frontend/app/features/editor/open-path-routing.js`
- Modify: `crates/termlab_tauri/frontend/index.html` (script tag beside the other `features/editor/*` includes)
- Modify: the startup wiring site — locate with `grep -n "termlabEditorService" crates/termlab_tauri/frontend/app/orchestration-runtime.js crates/termlab_tauri/frontend/app/event-wiring-runtime.js` and hook where the editor service is known-initialized
- Test: `scripts/tests/test_open_path_routing.mjs` (create)

**Interfaces:**
- Consumes: Task 1's `take_pending_open_paths` command (returns `string[]` for the calling window); `invoke('local_stat', { path })` → resolves a `FileEntry` (READ `crates/termlab_tauri/src/remote/sftp_commands.rs:200` and the `FileEntry` struct in termlab_remote to learn the exact directory-flag field — likely `is_dir` or a `file_type` string — and use the real field; rejects when the path does not exist); `termlabEditorService.openLocalFile(path)`; `window.toast` (grep `toast.error`/`toast.info` call style in `frontend/app/ui/toast.js` and match it).
- Produces: `window.termlabOpenPathRouting = { create }`; `create(deps)` → `{ drainPendingOpens() }` where `deps = { invoke, openLocalFile, toastError(title, body), toastInfo(title, body) }`. The "directory not yet" copy lives in ONE exported constant `DIRECTORY_COMING_SOON` — the seam the LSP/workspace branch later replaces.

- [ ] **Step 1: Write the failing test** `scripts/tests/test_open_path_routing.mjs` (VM harness in the style of `test_tab_switcher_runtime.mjs` — `sandbox.window = sandbox`, no `sandbox.global`, JSON-roundtrip cross-realm values):

```js
// A fake invoke: take_pending_open_paths returns the queue once; local_stat
// resolves per a fixture map or rejects for unknown paths.
function load({ pending, statMap }) {
  // ...load open-path-routing.js into a vm sandbox...
  const calls = { opened: [], errors: [], infos: [] };
  let drained = false;
  const routing = sandbox.termlabOpenPathRouting.create({
    invoke: async (cmd, args) => {
      if (cmd === 'take_pending_open_paths') {
        if (drained) return [];
        drained = true;
        return pending;
      }
      if (cmd === 'local_stat') {
        if (!(args.path in statMap)) throw new Error('no such file');
        return statMap[args.path];
      }
      throw new Error('unexpected command ' + cmd);
    },
    openLocalFile: (p) => { calls.opened.push(p); },
    toastError: (title, body) => { calls.errors.push(body); },
    toastInfo: (title, body) => { calls.infos.push(body); },
  });
  return { routing, calls };
}

// file → editor; directory → coming-soon info; missing → error naming the path
{
  const { routing, calls } = load({
    pending: ['/tmp/a.txt', '/tmp/dir', '/tmp/ghost.txt'],
    statMap: {
      '/tmp/a.txt': { /* FileEntry-shaped: regular file — use the real field */ },
      '/tmp/dir': { /* FileEntry-shaped: directory */ },
    },
  });
  await routing.drainPendingOpens();
  assert.deepStrictEqual(JSON.parse(JSON.stringify(calls.opened)), ['/tmp/a.txt']);
  assert.strictEqual(calls.infos.length, 1, 'directory gets the coming-soon toast');
  assert.strictEqual(calls.errors.length, 1);
  assert.ok(calls.errors[0].includes('/tmp/ghost.txt'), 'error names the path');
}

// empty queue: no invokes beyond the pull, no toasts
{
  const { routing, calls } = load({ pending: [], statMap: {} });
  await routing.drainPendingOpens();
  assert.deepStrictEqual(calls.opened, []);
  assert.strictEqual(calls.errors.length + calls.infos.length, 0);
}
```

Fill the FileEntry fixtures with the REAL field shape found in Step 1's read of the struct — a fixture inventing a field the backend never sends is the classic false-green.

- [ ] **Step 2: Run** — FAIL (module missing).

- [ ] **Step 3: Implement** `open-path-routing.js` (IIFE, `window.termlabOpenPathRouting = { create, DIRECTORY_COMING_SOON }`): `drainPendingOpens()` pulls once, iterates sequentially (`for … of` with `await` — deterministic toast order), routes per stat result, catches per-path so one bad path never blocks the rest. Then wire the startup site: create the router with the real deps (`invoke`, `termlabEditorService.openLocalFile`, toast fns) and call `drainPendingOpens()` once after the editor service is initialized — fire-and-forget with a `.catch(() => {})`, boot must not hang on it. Add the script tag to `index.html`.

- [ ] **Step 4: Run the suite — PASS; full node sweep — no failures; `node --check` on both touched JS files.**

- [ ] **Step 5: Commit** — `git add` the new module, `index.html`, `scripts/tests/test_open_path_routing.mjs`, and whichever startup-wiring file Step 3 modified, then `git commit -m "Route pending CLI open paths to the editor in each new window"`.

---

### Task 4: Install/uninstall `termlab` in PATH

**Files:**
- Create: `crates/termlab_tauri/src/cli_install.rs`
- Modify: `crates/termlab_tauri/src/lib.rs` (module decl + two commands in `generate_handler!`)
- Modify: `crates/termlab_tauri/frontend/app/command-palette-runtime.js` (`buildPaletteCommands()`, beside the `core:settings` entry ~line 122)
- Test: `#[cfg(test)]` in `cli_install.rs`

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces: Tauri commands `install_cli_symlink() -> Result<String, String>` / `uninstall_cli_symlink() -> Result<String, String>` (Ok carries a human-readable success message for the toast; Err the failure text); pure `enum LinkState { Missing, PointsToUs, PointsToOtherTermLab(PathBuf), Foreign(PathBuf) }`, `fn classify_link(link_meta: Option<&Path>, current_exe: &Path) -> LinkState` and `fn admin_shell_command(action: &str) -> String` (the escaped osascript payload).

- [ ] **Step 1: Write the failing tests:**

```rust
#[test]
fn classify_missing_ours_stale_and_foreign() {
    let exe = Path::new("/Applications/TermLab.app/Contents/MacOS/termlab");
    assert!(matches!(classify_link(None, exe), LinkState::Missing));
    assert!(matches!(classify_link(Some(exe), exe), LinkState::PointsToUs));
    let stale = Path::new("/Applications/Old/TermLab.app/Contents/MacOS/termlab");
    assert!(matches!(classify_link(Some(stale), exe), LinkState::PointsToOtherTermLab(_)));
    let foreign = Path::new("/usr/local/bin/some-other-tool-target");
    assert!(matches!(classify_link(Some(foreign), exe), LinkState::Foreign(_)));
}

#[test]
fn admin_command_escapes_quotes() {
    let cmd = admin_shell_command("ln -sf '/Apps/Term \"Lab\".app/x' '/usr/local/bin/termlab'");
    // The osascript payload must survive both quoting layers: the inner shell
    // string and the AppleScript string literal around it.
    assert!(cmd.contains("with administrator privileges"));
    assert!(!cmd.contains("\"Lab\""), "raw inner double-quotes must be escaped for AppleScript");
}
```

`PointsToOtherTermLab` classification rule: the link target's path contains a component ending in `TermLab.app` or a final component `termlab`, but is not the current exe. Anything else is `Foreign`.

- [ ] **Step 2: Run** `cargo test -p termlab_tauri --quiet` — compile FAIL.

- [ ] **Step 3: Implement.**

1. Pure layer per the interface block. `admin_shell_command(action)` builds `osascript -e 'do shell script "<action-escaped>" with administrator privileges'` argument strings (return the full argv as `Vec<String>` if that is cleaner than one string — adjust the test accordingly and note it).
2. `install_cli_symlink`: `let exe = std::env::current_exe()` canonicalized; link path `/usr/local/bin/termlab`; read the existing symlink target (`fs::read_link`, `Ok→Some`, `NotFound→None`, a non-symlink existing file → treat its own path as the target so it classifies `Foreign`); match `classify_link`: `PointsToUs` → Ok("already installed"); `Foreign(p)` → Err naming `p` and refusing; `Missing`/`PointsToOtherTermLab` → attempt `std::os::unix::fs::symlink` (after `fs::remove_file` for the stale case); on `PermissionDenied` run the osascript escalation via `std::process::Command` (`ln -sf <exe> /usr/local/bin/termlab`, both shell-escaped) and map non-zero exit to Err with stderr. Non-macOS unix: skip osascript, Err with the exact `sudo ln -sf …` command for the user. `#[cfg(not(unix))]`: Err("not supported on this platform").
3. `uninstall_cli_symlink`: same classification; remove only `PointsToUs`/`PointsToOtherTermLab` (direct, then osascript `rm` escalation on macOS); `Missing` → Ok("was not installed"); `Foreign` → Err refusing.
4. Palette (`command-palette-runtime.js`, inside `buildPaletteCommands()` near the `core:settings` `add(...)`):

```js
      add('core:install-cli', "Install 'termlab' Command in PATH", 'Application',
        'install cli path shell command terminal termlab symlink',
        async () => {
          try {
            const msg = await invoke('install_cli_symlink');
            showStatus(String(msg));
          } catch (error) {
            showStatus('Install failed: ' + String(error));
          }
        });
      add('core:uninstall-cli', "Uninstall 'termlab' Command from PATH", 'Application',
        'uninstall remove cli path shell command termlab symlink',
        async () => {
          try {
            const msg = await invoke('uninstall_cli_symlink');
            showStatus(String(msg));
          } catch (error) {
            showStatus('Uninstall failed: ' + String(error));
          }
        });
```

(READ how nearby palette entries surface success/failure first — if they use the toast global rather than `showStatus`, match that idiom instead; the palette file's existing `catch` style is authoritative.)

- [ ] **Step 4: Run** `cargo test -p termlab_tauri --quiet`, `cargo test --workspace --quiet`, full node sweep, `node --check` on the palette file — all green.

- [ ] **Step 5: Commit** — `git add crates/termlab_tauri/src/cli_install.rs crates/termlab_tauri/src/lib.rs crates/termlab_tauri/frontend/app/command-palette-runtime.js && git commit -m "Add install/uninstall of the termlab PATH symlink"`

---

### Task 5: Full gate + spec status

**Files:**
- Modify: `docs/superpowers/specs/2026-08-27-termlab-cli-open-design.md`

- [ ] **Step 1:** `cargo test --workspace --quiet` and the full node sweep — zero failures. `bash scripts/check_frontend_boundaries.sh` — only the pre-existing `tl-dialog.js:334` keydown finding is allowed.
- [ ] **Step 2:** `node --check` every branch-modified JS file (`git diff --name-only main...HEAD -- '*.js'`); `cargo build -p termlab_tauri` release-shape sanity (`--release` NOT required).
- [ ] **Step 3:** Update the spec's `**Status:**` to `Implemented (manual verification pending)` and append a `## Manual verification` section with the spec's Manual list as `- [ ]` items, plus one extra row: "`termlab msg new-window` behaves exactly as before this branch (the sender is unimplemented in-tree; the invocation boots the app with the argument ignored)" — guards the reserved namespace against accidental capture by the new parser.
- [ ] **Step 4:** Commit (`git commit -m "Mark termlab CLI open spec implemented"`), push `git push -u origin feat/termlab-cli-open`.

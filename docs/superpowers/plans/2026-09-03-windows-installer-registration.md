# Windows Installer Registration & Build Script — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give TermLab a single committed Windows build script that both the dev VM and CI run, producing installers that register an Explorer "Open TermLab here" context menu and default-app entries.

**Architecture:** Registration is declared once per bundler — a WiX v3 fragment for the MSI and an NSIS hook for the setup.exe — and a Rust integration test asserts the two declare identical registry keys so they cannot drift. The context menu invokes a new `--working-directory` flag, which `main.rs` implements as a single `set_current_dir` before Tauri boots; the shell already inherits the process cwd, so no PTY changes are needed. `scripts/build-windows.ps1` becomes the only thing that invokes the bundler, and `release.yml` / `ci.yml` call it.

**Tech Stack:** Rust (Tauri 2.10.3, tauri-bundler), WiX v3 (via tauri-bundler), NSIS (via tauri-bundler), PowerShell 5.1, GitHub Actions.

**Spec:** `docs/superpowers/specs/2026-09-03-windows-installer-design.md`

## Global Constraints

Every task's requirements implicitly include these. Values are copied verbatim from the spec.

- **All registry keys are per-user, under `HKCU`.** No `HKLM` writes anywhere. Install scope is per-user, no admin, to `%LOCALAPPDATA%\TermLab` (verified in Task 1 — *not* `%LOCALAPPDATA%\Programs\TermLab`, which earlier drafts of this plan assumed).
- **The WiX fragment must use the WiX v3 schema** — `xmlns="http://schemas.microsoft.com/wix/2006/wi"`. Tauri v2's MSI bundler is WiX 3. The existing `packaging/windows/termlab.wxs` uses the v4 schema and cannot be reused.
- **The context-menu substitution is capital `%V`**, not lowercase `%v`. Capital `%V` is the one that resolves correctly for `Directory\shell`, `Directory\Background\shell`, and `Drive\shell` alike.
- **The context-menu label is exactly `Open TermLab here`**, set via the `MUIVerb` value.
- **The three context-menu roots are** `Software\Classes\Directory\shell\TermLab`, `Software\Classes\Directory\Background\shell\TermLab`, and `Software\Classes\Drive\shell\TermLab`.
- **The installed executable is `termlab.exe`, lowercase** (verified in Task 1). Tauri does not rename the Cargo binary to `productName` for this app. Every registry value, shortcut target and assertion uses `termlab.exe`.
- **Nothing hardcodes `x64`.** The dev VM is ARM64 Windows on Apple Silicon, so its artifacts are named `*_arm64-*` while CI's are `*_x64-*`. Scripts detect or glob; they never assume.
- **Release artifact names must not change.** `release.yml` uploads `TermLab_${VERSION}_x64-setup.exe` and `TermLab_${VERSION}_x64_en-US.msi` (plus `.sig` files), and `latest.json` / the auto-updater depend on the setup.exe name.
- **`bundle.active` stays `false` in `tauri.conf.json`.** Bundling is switched on per-invocation via `--config '{"bundle":{"active":true}}'`.
- **No `|| true`, no unchecked success messages.** Every build step is fatal on failure and verifies its own output exists before claiming success.
- **Target VM:** `dustin@192.168.1.125`, Windows 11 build 26200, repo at `C:\Users\dustin\conch`. It has `cargo`, `node`, `npm`, `git`, `python`. It does **not** have `javac`, `jar`, `wix`, or `make`.
- **Current version is `3.0.0-rc.2`.** MSI `ProductVersion` must be numeric (at most three dot-separated integers).
- Per `CLAUDE.md`: never commit to `main`; all work stays on `feat/windows-installer-registration`. Never add `Co-Authored-By` trailers other than the one this session was told to use. Every behavior change gets tests where testable without a GUI.

## Amendments after Task 1 (2026-09-03)

Task 1 measured the real Windows pipeline and disproved four of this plan's
assumptions. These amendments supersede the task text they name. Evidence for
every claim here is in `docs/superpowers/plans/task-1-baseline.md`.

**A1 — The installed exe is `termlab.exe`, not `TermLab.exe`.** Task 4's code
blocks have been corrected in place. Nothing further to do.

**A2 — The install directory is `%LOCALAPPDATA%\TermLab`.** Functionally this
changes nothing, because every registry value uses `[INSTALLDIR]` / `$INSTDIR`
rather than a literal path. Task 8's README text must state the real path.

**A3 — MSI packaging rejects the pre-release version; NSIS accepts it.** The
exact error is `optional pre-release identifier in app version must be
numeric-only and cannot be greater than 65535 for msi target`, and the whole
`cargo tauri build` aborts with no bundle directory produced at all. This
supersedes **Task 5 Step 2**, whose stop-and-ask was predicated on the only fix
being a global version strip that would rename the setup.exe and break the
updater. That is not the only fix. Task 5 instead bundles in two invocations:

- NSIS at the real version, so `TermLab_<version>_<arch>-setup.exe` and the
  updater artifact keep their exact current names.
- MSI in a separate invocation carrying a numeric-only `version` override, so an
  MSI is still produced. Its filename will read `3.0.0` where the setup.exe reads
  `3.0.0-rc.2`; MSI `ProductVersion` cannot encode a pre-release, so this is
  inherent, not a defect.

**A4 — `cargo tauri build` cannot run at all with the current config.**
tauri-cli 2.11.4 hard-errors with `The configured frontendDist includes the
["node_modules"] folder`, because `frontendDist: "frontend"` resolves to
`crates/termlab_tauri/frontend`, which is also where `npm ci` installs
`node_modules`. `release.yml` installs tauri-cli unpinned (`--version "^2"`), so
**both the `windows` and `macos` release jobs are broken today, independent of
this branch.** Task 5 must solve this or it cannot build anything. Preferred
approach, in order:

1. After the vendor build, stage a clean copy of the web assets (everything in
   `frontend/` except `node_modules`) into a git-ignored directory, and point
   `build.frontendDist` at it via the script's own `--config`. Non-destructive —
   it does not touch the developer's `node_modules`.
2. If staging breaks `bundle.resources` path resolution (`"frontend/themes/"` is
   relative to the crate directory), fall back to deleting
   `frontend/node_modules` after the vendor build and before bundling.

Either way the script must also override `build.beforeBuildCommand` to `""` for
the bundle invocation, or Tauri re-runs `npm ci` and recreates the very
directory the fix removed. Verify which approach works on the VM rather than
assuming.

**A5 — A signing key is NOT needed to build installers.** It is required only
because `bundle.createUpdaterArtifacts: true` asks for a signed `.sig`. The
installer is fully written to disk *before* the signing step fails, so a build
can exit non-zero with a perfectly good installer present. Task 5 therefore must
not treat a non-zero exit as proof of failure without first checking whether the
artifacts exist — and must not require a key from a developer who only wants an
installer. Do not edit `tauri.conf.json` to disable updater artifacts globally;
that would change what CI ships.

**A6 — Task 6 verifies the NSIS setup.exe as its primary target.** The context
menu hook lives in the NSIS installer, NSIS is what the updater ships, and an MSI
may not exist at a pre-release version. The MSI is a secondary check when
present. Task 6's registry assertions are unchanged — they are
architecture-independent.

**A7 — Task 7 must not upload an MSI unconditionally.** `release.yml` currently
does, which is part of why a release tag at the current version would fail. The
MSI upload becomes conditional on the artifact existing.

---

### Task 1: Establish the verified Windows baseline on the VM

This task exists because three facts the later tasks depend on cannot be known
from the source tree, and guessing any of them wrong invalidates Tasks 4–6:

1. Whether tauri-bundler accepts the pre-release version `3.0.0-rc.2` for the MSI's numeric `ProductVersion`, or fails.
2. The **installed executable's filename**. The Cargo binary is `termlab` (`crates/termlab_tauri/Cargo.toml:8-10`) but `productName` is `TermLab` (`crates/termlab_tauri/tauri.conf.json`), and Tauri v2 renames the main binary to `mainBinaryName`, which defaults to `productName`. Every registry value in Tasks 4 and 6 embeds this name.
3. The actual install directory the NSIS installer uses at `installMode: "currentUser"`.

**Files:**
- Create: `docs/superpowers/plans/task-1-baseline.md` (findings, committed so later tasks can cite it)

**Interfaces:**
- Produces: three verified constants used by every later task —
  - `INSTALLED_EXE_NAME` (expected `TermLab.exe`, must be confirmed — came back `termlab.exe`)
  - `INSTALL_DIR` (expected `%LOCALAPPDATA%\Programs\TermLab`, must be confirmed)
  - `MSI_VERSION_OK` (boolean: does `3.0.0-rc.2` bundle without error)

- [ ] **Step 1: Sync the branch to the VM**

```bash
ssh dustin@192.168.1.125 'powershell -NoProfile -Command "cd C:\Users\dustin\conch; git fetch origin; git checkout feat/windows-installer-registration; git reset --hard origin/feat/windows-installer-registration"'
```

Expected: the branch checks out. If the branch is not on origin yet, push it first from the worktree with `git push -u origin feat/windows-installer-registration`.

- [ ] **Step 2: Install the JDK the build needs**

The VM has no `javac`/`jar`, and without them the binary ships with no embedded Java SDK jar.

```bash
ssh dustin@192.168.1.125 'winget install --id Microsoft.OpenJDK.21 --accept-source-agreements --accept-package-agreements'
```

Expected: install succeeds. Confirm in a **new** shell (winget's PATH change does not apply to the current one):

```bash
ssh dustin@192.168.1.125 'powershell -NoProfile -Command "javac -version; jar --version"'
```

- [ ] **Step 3: Install the Tauri CLI on the VM**

```bash
ssh dustin@192.168.1.125 'powershell -NoProfile -Command "cargo install tauri-cli --version ^2 --locked"'
```

Expected: `cargo tauri --version` prints a 2.x version. This can take several minutes.

- [ ] **Step 4: Run a stock bundle to answer all three questions at once**

```bash
ssh dustin@192.168.1.125 'powershell -NoProfile -Command "cd C:\Users\dustin\conch; mkdir -Force java-sdk\build\classes; javac -d java-sdk\build\classes (Get-ChildItem java-sdk\src\termlab\plugin\*.java); jar cf java-sdk\build\termlab-plugin-sdk.jar -C java-sdk\build\classes .; cargo tauri build --config (Get-Content -Raw -Path nul) 2>&1"'
```

If quoting the inline `--config` JSON through SSH proves painful, write it to a file on the VM first and use `--config bundle-on.json` containing `{"bundle":{"active":true}}`. Do not fight the quoting; the file form is fine and is what the script in Task 5 will effectively do.

Expected outcomes to record:
- **If the build fails on version**, capture the exact error. `MSI_VERSION_OK = false`, and Task 5 gains an explicit numeric-version step.
- **If it succeeds**, list the outputs:

```bash
ssh dustin@192.168.1.125 'powershell -NoProfile -Command "Get-ChildItem C:\Users\dustin\conch\target\release\bundle -Recurse -File | Select-Object FullName, Length"'
```

- [ ] **Step 5: Install the stock MSI and record the real layout**

```bash
ssh dustin@192.168.1.125 'powershell -NoProfile -Command "Start-Process msiexec -ArgumentList \"/i\",\"C:\Users\dustin\conch\target\release\bundle\msi\TermLab_3.0.0-rc.2_x64_en-US.msi\",\"/qn\" -Wait; Get-ChildItem -Recurse \"$env:LOCALAPPDATA\Programs\TermLab\" -ErrorAction SilentlyContinue | Select-Object FullName"'
```

Adjust the MSI filename to whatever Step 4 actually produced.

Expected: a directory listing. **Record the exact `.exe` filename** — this is `INSTALLED_EXE_NAME`. If nothing is under `%LOCALAPPDATA%\Programs`, search Program Files too and record what you find as `INSTALL_DIR`.

- [ ] **Step 6: Uninstall, leaving the VM clean**

```bash
ssh dustin@192.168.1.125 'powershell -NoProfile -Command "Get-CimInstance Win32_Product -Filter \"Name LIKE ''TermLab%''\" | ForEach-Object { $_ | Invoke-CimMethod -MethodName Uninstall }"'
```

Expected: TermLab is gone from `%LOCALAPPDATA%\Programs`.

- [ ] **Step 7: Write the findings down**

Create `docs/superpowers/plans/task-1-baseline.md` recording, as plain statements with the commands that produced them:

```markdown
# Task 1 baseline — verified on the VM, 2026-09-03

- INSTALLED_EXE_NAME: <exact filename>
- INSTALL_DIR: <exact path>
- MSI_VERSION_OK: <yes / no, with the error text if no>
- Bundle output paths: <the listing from Step 4>
```

**RESOLVED:** `INSTALLED_EXE_NAME` came back as `termlab.exe` (lowercase). Task 4's code blocks have already been corrected in place; see Amendment A1.

- [ ] **Step 8: Commit**

```bash
git add docs/superpowers/plans/task-1-baseline.md
git commit -m "Record verified Windows bundle baseline from the dev VM

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 2: Add `--working-directory` to the CLI decision layer

`crates/termlab_tauri/src/cli.rs` is split into a pure decision layer
(`evaluate` / `plan`) and an I/O layer (`run_cli_if_requested`). All the new
logic goes in the pure layer, where it is unit-testable without sockets or a
GUI. This task is pure Rust and does not need the VM.

**Files:**
- Modify: `crates/termlab_tauri/src/cli.rs` — `HELP_TEXT` (line ~50), `CliDecision` (line ~64), `CliPlan` (line ~103), `plan` (line ~124), `evaluate` (line ~166), `CliAction` (line ~155), `run_cli_if_requested` (line ~236)
- Test: `crates/termlab_tauri/src/cli.rs` — the existing `#[cfg(test)] mod tests` at the bottom

**Interfaces:**
- Produces, for Task 3:
  - `CliAction::RunAppInDirectory { directory: String }` — a new variant `main.rs` must handle. `directory` is an absolute, lexically-cleaned path (it has been through `normalize_path`), but it is **not** guaranteed to exist on disk.
  - `CliDecision::RunAppInDirectory { directory: String }` and `CliPlan::RunAppInDirectory { directory: String }`
  - `CliDecision::UsageError(String)` and `CliPlan::UsageError(String)` — message is the human-readable reason, printed by the I/O layer as `termlab: {msg}` with exit code 2.
- Consumes: `INSTALLED_EXE_NAME` is not needed here. The flag spelling `--working-directory` is fixed by the Global Constraints and is what Task 4's registry command line must match exactly.

- [ ] **Step 1: Write the failing tests**

Add to the existing `#[cfg(test)] mod tests` block at the bottom of `crates/termlab_tauri/src/cli.rs`. The existing helper `native()` (already in that module) converts unix-style paths to native separators — use it, because `normalize_path` returns native separators and these assertions must pass on Windows too.

```rust
    // --- --working-directory ------------------------------------------------

    #[test]
    fn working_directory_flag_is_parsed_as_a_launch_directory() {
        let args = vec!["--working-directory".to_string(), "/home/dustin/proj".to_string()];
        assert_eq!(
            evaluate(&args, Path::new("/tmp")),
            CliDecision::RunAppInDirectory {
                directory: native("/home/dustin/proj"),
            },
            "--working-directory <DIR> should ask for a launch in <DIR>"
        );
    }

    #[test]
    fn working_directory_accepts_the_equals_form() {
        let args = vec!["--working-directory=/home/dustin/proj".to_string()];
        assert_eq!(
            evaluate(&args, Path::new("/tmp")),
            CliDecision::RunAppInDirectory {
                directory: native("/home/dustin/proj"),
            },
            "--working-directory=<DIR> should behave like the space-separated form"
        );
    }

    #[test]
    fn working_directory_is_resolved_against_the_cwd() {
        let args = vec!["--working-directory".to_string(), "proj".to_string()];
        assert_eq!(
            evaluate(&args, Path::new("/home/dustin")),
            CliDecision::RunAppInDirectory {
                directory: native("/home/dustin/proj"),
            },
            "a relative --working-directory should resolve against the process cwd"
        );
    }

    #[test]
    fn working_directory_without_a_value_is_a_usage_error() {
        let args = vec!["--working-directory".to_string()];
        assert!(
            matches!(evaluate(&args, Path::new("/tmp")), CliDecision::UsageError(_)),
            "--working-directory with no value should be a usage error, not a silent launch"
        );
    }

    #[test]
    fn empty_working_directory_value_is_a_usage_error() {
        let args = vec!["--working-directory=".to_string()];
        assert!(
            matches!(evaluate(&args, Path::new("/tmp")), CliDecision::UsageError(_)),
            "--working-directory= with an empty value should be a usage error"
        );
    }

    #[test]
    fn working_directory_combined_with_paths_is_a_usage_error() {
        let args = vec![
            "--working-directory".to_string(),
            "/home/dustin".to_string(),
            "notes.md".to_string(),
        ];
        assert!(
            matches!(evaluate(&args, Path::new("/tmp")), CliDecision::UsageError(_)),
            "--working-directory and path arguments express different intents and must not combine"
        );
    }

    #[test]
    fn help_wins_over_working_directory() {
        let args = vec![
            "--working-directory".to_string(),
            "/home/dustin".to_string(),
            "--help".to_string(),
        ];
        assert_eq!(
            evaluate(&args, Path::new("/tmp")),
            CliDecision::PrintHelp,
            "--help must keep precedence over --working-directory"
        );
    }

    #[test]
    fn version_wins_over_working_directory() {
        let args = vec!["--working-directory".to_string(), "/x".to_string(), "-V".to_string()];
        assert_eq!(
            evaluate(&args, Path::new("/tmp")),
            CliDecision::PrintVersion,
            "-V must keep precedence over --working-directory"
        );
    }

    #[test]
    fn working_directory_survives_the_detached_marker() {
        let args = vec!["--working-directory".to_string(), "/home/dustin".to_string()];
        let env = EnvMarkers::from_values(None, Some("1"));
        assert_eq!(
            plan(&args, Path::new("/tmp"), env),
            CliPlan::RunAppInDirectory {
                directory: native("/home/dustin"),
            },
            "there is no forwarding path for --working-directory, so DETACHED changes nothing"
        );
    }

    #[test]
    fn app_running_marker_ignores_working_directory() {
        let args = vec!["--working-directory".to_string(), "/home/dustin".to_string()];
        let env = EnvMarkers::from_values(Some("1"), None);
        assert_eq!(
            plan(&args, Path::new("/tmp"), env),
            CliPlan::RunApp { pending_paths: vec![] },
            "a Tauri restart re-exec inherits argv and must not be re-interpreted as a fresh request"
        );
    }

    #[test]
    fn help_text_documents_the_working_directory_flag() {
        assert!(
            HELP_TEXT.contains("--working-directory"),
            "the flag must be discoverable from --help"
        );
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p termlab_tauri --lib cli::tests 2>&1 | tail -30`

Expected: compile errors — `no variant named RunAppInDirectory found for enum CliDecision`, and the same for `UsageError` and `CliPlan::RunAppInDirectory`.

- [ ] **Step 3: Add the new enum variants**

In `crates/termlab_tauri/src/cli.rs`, add to `CliDecision`:

```rust
    /// `--working-directory <DIR>`: boot the app with `DIR` as its process
    /// working directory. The directory is absolute and lexically cleaned but
    /// is NOT guaranteed to exist — the I/O layer decides what to do about
    /// that, so this stays a pure decision.
    RunAppInDirectory { directory: String },
    /// Arguments parsed, but the combination is meaningless. The string is
    /// the human-readable reason; the I/O layer prints it and exits 2.
    UsageError(String),
```

Add the same two variants to `CliPlan`, with doc comments adapted:


```rust
    /// Boot the app in this process with `directory` as its working
    /// directory. There is no forwarding counterpart: Windows has no IPC
    /// listener (`ipc.rs` is `#[cfg(unix)]` throughout), and the context menu
    /// this serves wants a fresh window either way.
    RunAppInDirectory { directory: String },
    /// A usage error detected during parsing. Printed, then exit 2.
    UsageError(String),
```

And add **only** `RunAppInDirectory` to `CliAction` — a usage error never
reaches the caller as an action, it becomes `Exit(2)` in Step 7:

```rust
pub enum CliAction {
    RunApp { pending_paths: Vec<String> },
    /// Boot the app after moving the process into `directory`.
    RunAppInDirectory { directory: String },
    Exit(i32),
}
```

- [ ] **Step 4: Add the flag constant and update `HELP_TEXT`**

Replace the existing `HELP_TEXT` constant with:

```rust
/// The flag the Windows Explorer context menu invokes. Its spelling is
/// baked into `packaging/windows/registration.wxs` and
/// `packaging/windows/installer-hooks.nsh`; changing it here without
/// changing those ships a context-menu entry that exits 2.
const WORKING_DIRECTORY_FLAG: &str = "--working-directory";

const HELP_TEXT: &str = "\
termlab [PATH ...]
termlab --working-directory <DIR>

Open PATH(s) in a TermLab window. With no arguments, launches TermLab.

  --working-directory <DIR>
                   Launch TermLab with <DIR> as its working directory, so
                   terminals in the new window start there. Cannot be
                   combined with PATH arguments.
  -h, --help       Print this help message
  -V, --version    Print the version
";
```

- [ ] **Step 5: Teach `evaluate` the flag**

Replace the body of `evaluate` from `let mut paths = Vec::new();` through the closing `}` of the function with:

```rust
    let mut paths = Vec::new();
    let mut working_directory: Option<String> = None;
    let mut args_iter = args.iter();

    while let Some(arg) = args_iter.next() {
        if arg == "--help" || arg == "-h" {
            return CliDecision::PrintHelp;
        }
        if arg == "--version" || arg == "-V" {
            return CliDecision::PrintVersion;
        }
        if arg == WORKING_DIRECTORY_FLAG {
            match args_iter.next() {
                Some(value) if !value.is_empty() => {
                    working_directory = Some(normalize_path(value, cwd));
                }
                _ => {
                    return CliDecision::UsageError(format!(
                        "{WORKING_DIRECTORY_FLAG} requires a directory"
                    ));
                }
            }
            continue;
        }
        if let Some(value) = arg.strip_prefix(&format!("{WORKING_DIRECTORY_FLAG}=")) {
            if value.is_empty() {
                return CliDecision::UsageError(format!(
                    "{WORKING_DIRECTORY_FLAG} requires a directory"
                ));
            }
            working_directory = Some(normalize_path(value, cwd));
            continue;
        }
        if arg.starts_with("-psn_") {
            // macOS LaunchServices appends a process-serial-number flag
            // (`-psn_0_12345`) to Finder/Dock launches. Rejecting it would
            // exit(2) before Tauri init, so a double-launch would die
            // silently with no window and nothing on any console. Not a
            // path, not a flag we handle — skipped entirely.
            continue;
        }
        if let Some(rest) = arg.strip_prefix('-') {
            if !rest.is_empty() {
                return CliDecision::UnknownFlag(arg.clone());
            }
        }
        paths.push(normalize_path(arg, cwd));
    }

    match working_directory {
        Some(_) if !paths.is_empty() => CliDecision::UsageError(format!(
            "{WORKING_DIRECTORY_FLAG} cannot be combined with path arguments"
        )),
        Some(directory) => CliDecision::RunAppInDirectory { directory },
        None if paths.is_empty() => CliDecision::RunApp,
        None => CliDecision::ForwardOrRun { paths },
    }
}
```

- [ ] **Step 6: Map the new decisions through `plan`**

In `plan`, add these two arms to the `match evaluate(args, cwd)` block, alongside the existing ones:

```rust
        CliDecision::RunAppInDirectory { directory } => {
            // No forwarding counterpart on purpose: this is the Explorer
            // context-menu path, and Windows has no IPC listener to forward
            // to. `DETACHED` is therefore irrelevant here.
            CliPlan::RunAppInDirectory { directory }
        }
        CliDecision::UsageError(msg) => CliPlan::UsageError(msg),
```

- [ ] **Step 7: Handle the new plans in the I/O layer**

In `run_cli_if_requested`, add these two arms to the `match plan(...)` block:

```rust
        CliPlan::RunAppInDirectory { directory } => CliAction::RunAppInDirectory { directory },
        CliPlan::UsageError(msg) => {
            eprintln!("termlab: {msg}");
            CliAction::Exit(2)
        }
```

- [ ] **Step 8: Run the tests to verify they pass**

Run: `cargo test -p termlab_tauri --lib cli::tests 2>&1 | tail -20`

Expected: PASS, including every pre-existing test in the module. If an existing test now fails, the `evaluate` rewrite dropped behavior — re-read Step 5 against the original body rather than editing the test.

- [ ] **Step 9: Check the whole crate still compiles**

Run: `cargo check -p termlab_tauri --all-targets 2>&1 | tail -20`

Expected: a non-exhaustive-match error in `crates/termlab_tauri/src/main.rs`, because `CliAction` gained a variant. That is correct and is Task 3's job. No other errors.

- [ ] **Step 10: Commit**

```bash
git add crates/termlab_tauri/src/cli.rs
git commit -m "Add --working-directory to the CLI decision layer

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 3: Apply the working directory at startup

The whole runtime implementation is one `set_current_dir` call, because
`PtyBackend` spawns the shell with the inherited process cwd
(`crates/termlab_tauri/src/pty.rs`, `spawn_shell_for_pane` sets no cwd) and
`get_workspace_dir` already reports the launch cwd for tab titles. Both fall
out of moving the process before Tauri boots.

**Files:**
- Modify: `crates/termlab_tauri/src/main.rs:17-21` (the `run_cli_if_requested` match)

**Interfaces:**
- Consumes from Task 2: `CliAction::RunAppInDirectory { directory: String }`, where `directory` is absolute and lexically cleaned but may not exist on disk.
- Produces: nothing later tasks consume in code. Behaviorally it produces the guarantee Task 6 verifies — `termlab.exe --working-directory <DIR>` launches with `<DIR>` as the process cwd.

- [ ] **Step 1: Replace the CLI dispatch match**

In `crates/termlab_tauri/src/main.rs`, replace:

```rust
    let pending_paths = match termlab_tauri::cli::run_cli_if_requested() {
        termlab_tauri::cli::CliAction::Exit(code) => std::process::exit(code),
        termlab_tauri::cli::CliAction::RunApp { pending_paths } => pending_paths,
    };
```

with:

```rust
    let pending_paths = match termlab_tauri::cli::run_cli_if_requested() {
        termlab_tauri::cli::CliAction::Exit(code) => std::process::exit(code),
        termlab_tauri::cli::CliAction::RunApp { pending_paths } => pending_paths,
        termlab_tauri::cli::CliAction::RunAppInDirectory { directory } => {
            // The Explorer "Open TermLab here" verb. Moving the process is
            // the entire implementation: PtyBackend spawns the shell with the
            // inherited cwd, and get_workspace_dir reports the launch cwd for
            // tab titles, so both follow from this one call. It must happen
            // before platform::init(), which spawns environment probes.
            //
            // A directory that no longer exists must not be fatal: a stale
            // context-menu entry should still get the user an app window.
            if let Err(e) = std::env::set_current_dir(&directory) {
                log::warn!(
                    "startup: could not enter --working-directory {directory}: {e}; \
                     launching in the inherited directory instead"
                );
            } else {
                log::info!("startup: working directory set to {directory}");
            }
            Vec::new()
        }
    };
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo check -p termlab_tauri --all-targets 2>&1 | tail -20`

Expected: clean, no warnings about non-exhaustive matches.

- [ ] **Step 3: Verify the behavior end to end on this machine**

`set_current_dir` is process-global and cannot be unit-tested without racing every other test in the binary, so verify it by running the real binary. On macOS/Linux:

```bash
cargo run -p termlab_tauri -- --working-directory /tmp 2>&1 | grep "working directory"
```

Expected: a log line `startup: working directory set to /tmp`. Close the window that opens.

Then check the failure path is non-fatal:

```bash
cargo run -p termlab_tauri -- --working-directory /nonexistent-xyz 2>&1 | grep "could not enter"
```

Expected: a warning line, and the app still opens a window. Close it.

And the usage error:

```bash
cargo run -p termlab_tauri -- --working-directory /tmp notes.md; echo "exit=$?"
```

Expected: `termlab: --working-directory cannot be combined with path arguments` and `exit=2`.

- [ ] **Step 4: Run the full test suite**

Run: `cargo test --workspace 2>&1 | tail -20`

Expected: all tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/termlab_tauri/src/main.rs
git commit -m "Apply --working-directory to the process before Tauri boots

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 4: Declare the registration once, for both bundlers

**Files:**
- Create: `packaging/windows/registration.wxs`
- Create: `packaging/windows/installer-hooks.nsh`
- Create: `crates/termlab_tauri/tests/windows_registration_parity.rs`
- Modify: `crates/termlab_tauri/tauri.conf.json` (the `bundle` object)

**Interfaces:**
- Consumes from Task 1: `INSTALLED_EXE_NAME` = `termlab.exe` (lowercase), and `INSTALL_DIR` = `%LOCALAPPDATA%\TermLab`. The code blocks below already use the verified name — use them exactly as written and do not "correct" the casing to match `productName`.
- Consumes from Task 2: the flag spelling `--working-directory`.
- Produces: the registry key set that Task 6's verification script asserts, namely — `Software\Classes\Directory\shell\TermLab`, `Software\Classes\Directory\Background\shell\TermLab`, `Software\Classes\Drive\shell\TermLab` (each with a `\command` subkey), `Software\Clients\Terminal\TermLab\Capabilities`, `Software\RegisteredApplications`, and `Software\Microsoft\Windows\CurrentVersion\App Paths\termlab.exe`.

- [ ] **Step 1: Write the failing parity test**

Create `crates/termlab_tauri/tests/windows_registration_parity.rs`. This is a plain text-parsing test with no Windows dependency, so it runs in CI on Linux and macOS too — which is the point: drift gets caught on every PR, not only on a Windows box.

```rust
//! Guards the two Windows installer registration sources against drift.
//!
//! `packaging/windows/registration.wxs` (consumed by the MSI bundler) and
//! `packaging/windows/installer-hooks.nsh` (consumed by the NSIS bundler)
//! must register the same keys. Historically the project had two Windows
//! installer definitions and only one of them grew a context menu, so the
//! shipped installer had none. This test is what stops that recurring.

use std::collections::BTreeSet;
use std::path::PathBuf;

fn packaging_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("packaging")
        .join("windows")
}

fn read(name: &str) -> String {
    let path = packaging_dir().join(name);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
}

/// Pull the value of `name="..."` out of a single line of XML.
fn attribute(line: &str, name: &str) -> Option<String> {
    let needle = format!("{name}=\"");
    let start = line.find(&needle)? + needle.len();
    let rest = &line[start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

/// Every `Key` declared by a `<RegistryKey>` element in the WiX fragment.
fn wix_keys(text: &str) -> BTreeSet<String> {
    let mut keys = BTreeSet::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if !trimmed.starts_with("<RegistryKey") {
            continue;
        }
        assert!(
            trimmed.contains(r#"Root="HKCU""#),
            "registration must be per-user; found a non-HKCU key: {trimmed}"
        );
        let key = attribute(trimmed, "Key")
            .unwrap_or_else(|| panic!("<RegistryKey> without a Key attribute: {trimmed}"));
        keys.insert(key);
    }
    keys
}

/// Every key path targeted by `command` (e.g. `WriteRegStr`) in the NSIS hook.
fn nsis_keys(text: &str, command: &str) -> BTreeSet<String> {
    let mut keys = BTreeSet::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with(';') {
            continue;
        }
        let Some(rest) = trimmed.strip_prefix(command) else {
            continue;
        };
        let rest = rest.trim_start();
        let rest = rest.strip_prefix("HKCU").unwrap_or_else(|| {
            panic!("registration must be per-user; found a non-HKCU write: {trimmed}")
        });
        let rest = rest.trim_start();
        let rest = rest
            .strip_prefix('"')
            .unwrap_or_else(|| panic!("key path must be quoted: {trimmed}"));
        let end = rest
            .find('"')
            .unwrap_or_else(|| panic!("unterminated key path: {trimmed}"));
        keys.insert(rest[..end].to_string());
    }
    keys
}

#[test]
fn both_bundlers_register_the_same_keys() {
    let wix = wix_keys(&read("registration.wxs"));
    let nsis = nsis_keys(&read("installer-hooks.nsh"), "WriteRegStr");

    let only_in_wix: Vec<_> = wix.difference(&nsis).collect();
    let only_in_nsis: Vec<_> = nsis.difference(&wix).collect();

    assert!(
        only_in_wix.is_empty() && only_in_nsis.is_empty(),
        "the MSI and setup.exe must register identical keys.\n\
         only in registration.wxs: {only_in_wix:?}\n\
         only in installer-hooks.nsh: {only_in_nsis:?}"
    );
}

#[test]
fn uninstall_removes_every_key_the_install_wrote() {
    let hooks = read("installer-hooks.nsh");
    let written = nsis_keys(&hooks, "WriteRegStr");

    // Software\\RegisteredApplications is shared with every other installed
    // application, so uninstall removes our *value* from it rather than the
    // key. Both removal forms therefore count as cleanup.
    let mut deleted = nsis_keys(&hooks, "DeleteRegKey");
    deleted.extend(nsis_keys(&hooks, "DeleteRegValue"));

    for key in &written {
        let covered = deleted
            .iter()
            .any(|d| key == d || key.starts_with(&format!("{d}\\")));
        assert!(
            covered,
            "uninstall leaves {key} behind; no DeleteRegKey covers it"
        );
    }
}

#[test]
fn the_context_menu_invokes_the_flag_the_cli_actually_accepts() {
    for source in ["registration.wxs", "installer-hooks.nsh"] {
        let text = read(source);
        assert!(
            text.contains("--working-directory"),
            "{source} must invoke --working-directory; any other flag exits 2"
        );
        assert!(
            text.contains("%V"),
            "{source} must use capital %V, which is the substitution that \
             resolves for Directory, Directory\\Background and Drive alike"
        );
        assert!(
            !text.contains("\"%v\""),
            "{source} uses lowercase %v, which does not resolve for Directory\\shell"
        );
        assert!(
            text.contains("Open TermLab here"),
            "{source} must use the agreed context-menu label"
        );
    }
}

#[test]
fn all_three_context_menu_roots_are_registered() {
    let wix = wix_keys(&read("registration.wxs"));
    for root in [
        r"Software\Classes\Directory\shell\TermLab",
        r"Software\Classes\Directory\Background\shell\TermLab",
        r"Software\Classes\Drive\shell\TermLab",
    ] {
        assert!(
            wix.contains(root),
            "missing context-menu root {root}; the entry would not appear \
             for that kind of right-click"
        );
        assert!(
            wix.contains(&format!(r"{root}\command")),
            "missing command subkey for {root}; the entry would appear but do nothing"
        );
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p termlab_tauri --test windows_registration_parity 2>&1 | tail -20`

Expected: FAIL — `cannot read .../packaging/windows/registration.wxs: No such file or directory`.

- [ ] **Step 3: Write the WiX fragment**

Create `packaging/windows/registration.wxs`. Note the schema is WiX **v3** — `http://schemas.microsoft.com/wix/2006/wi` — because that is what tauri-bundler runs. `ForceDeleteOnUninstall="yes"` is what removes the subtree on uninstall.

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!--
  Windows registration for the MSI, consumed via bundle.windows.wix.fragmentPaths.

  Its twin is packaging/windows/installer-hooks.nsh, which does the same for the
  NSIS setup.exe. The two are asserted to declare identical keys by
  crates/termlab_tauri/tests/windows_registration_parity.rs — edit both or
  neither.

  All keys are HKCU: the installer is per-user and takes no UAC prompt.
-->
<Wix xmlns="http://schemas.microsoft.com/wix/2006/wi">
  <Fragment>
    <DirectoryRef Id="TARGETDIR">

      <!-- Explorer "Open TermLab here". Capital %V is required: it is the
           substitution that resolves for all three roots below. -->
      <Component Id="TermLabContextMenu" Guid="8f2c41d6-9b3e-4a17-8c05-6d1e2f7a9b34">
        <RegistryKey Root="HKCU" Key="Software\Classes\Directory\shell\TermLab" ForceDeleteOnUninstall="yes">
          <RegistryValue Name="MUIVerb" Type="string" Value="Open TermLab here" KeyPath="yes" />
          <RegistryValue Name="Icon" Type="string" Value="[INSTALLDIR]termlab.exe" />
        </RegistryKey>
        <RegistryKey Root="HKCU" Key="Software\Classes\Directory\shell\TermLab\command" ForceDeleteOnUninstall="yes">
          <RegistryValue Type="string" Value="&quot;[INSTALLDIR]termlab.exe&quot; --working-directory &quot;%V&quot;" />
        </RegistryKey>

        <RegistryKey Root="HKCU" Key="Software\Classes\Directory\Background\shell\TermLab" ForceDeleteOnUninstall="yes">
          <RegistryValue Name="MUIVerb" Type="string" Value="Open TermLab here" />
          <RegistryValue Name="Icon" Type="string" Value="[INSTALLDIR]termlab.exe" />
        </RegistryKey>
        <RegistryKey Root="HKCU" Key="Software\Classes\Directory\Background\shell\TermLab\command" ForceDeleteOnUninstall="yes">
          <RegistryValue Type="string" Value="&quot;[INSTALLDIR]termlab.exe&quot; --working-directory &quot;%V&quot;" />
        </RegistryKey>

        <RegistryKey Root="HKCU" Key="Software\Classes\Drive\shell\TermLab" ForceDeleteOnUninstall="yes">
          <RegistryValue Name="MUIVerb" Type="string" Value="Open TermLab here" />
          <RegistryValue Name="Icon" Type="string" Value="[INSTALLDIR]termlab.exe" />
        </RegistryKey>
        <RegistryKey Root="HKCU" Key="Software\Classes\Drive\shell\TermLab\command" ForceDeleteOnUninstall="yes">
          <RegistryValue Type="string" Value="&quot;[INSTALLDIR]termlab.exe&quot; --working-directory &quot;%V&quot;" />
        </RegistryKey>
      </Component>

      <!-- Default Programs: puts TermLab in Settings > Default apps and in
           "Open with". Note this is NOT the Windows 11 "Default terminal
           application" setting, which requires ConPTY handoff. -->
      <Component Id="TermLabDefaultApp" Guid="2b5e7c14-3f68-4d92-a0b7-51c9e8f36a2d">
        <RegistryKey Root="HKCU" Key="Software\Clients\Terminal\TermLab\Capabilities" ForceDeleteOnUninstall="yes">
          <RegistryValue Name="ApplicationName" Type="string" Value="TermLab" KeyPath="yes" />
          <RegistryValue Name="ApplicationDescription" Type="string" Value="A cross-platform terminal emulator and SSH client" />
          <RegistryValue Name="ApplicationIcon" Type="string" Value="[INSTALLDIR]termlab.exe,0" />
        </RegistryKey>
        <RegistryKey Root="HKCU" Key="Software\RegisteredApplications">
          <RegistryValue Name="TermLab" Type="string" Value="Software\Clients\Terminal\TermLab\Capabilities" />
        </RegistryKey>
      </Component>

      <!-- App Paths: makes Win+R "termlab" work without touching PATH. -->
      <Component Id="TermLabAppPaths" Guid="6d3a90f2-71c4-4e5b-9a38-b02f4c7d1e85">
        <RegistryKey Root="HKCU" Key="Software\Microsoft\Windows\CurrentVersion\App Paths\termlab.exe" ForceDeleteOnUninstall="yes">
          <RegistryValue Type="string" Value="[INSTALLDIR]termlab.exe" KeyPath="yes" />
          <RegistryValue Name="Path" Type="string" Value="[INSTALLDIR]" />
        </RegistryKey>
      </Component>

    </DirectoryRef>
  </Fragment>
</Wix>
```

- [ ] **Step 4: Write the NSIS hook**

Create `packaging/windows/installer-hooks.nsh`. `$INSTDIR` and `${MAINBINARYNAME}` are provided by Tauri's NSIS template. If Task 1 showed `${MAINBINARYNAME}` is not what produces the installed exe name, hardcode the verified name instead.

```nsis
; Windows registration for the NSIS setup.exe, consumed via
; bundle.windows.nsis.installerHooks.
;
; Its twin is packaging/windows/registration.wxs, which does the same for the
; MSI. The two are asserted to declare identical keys by
; crates/termlab_tauri/tests/windows_registration_parity.rs — edit both or
; neither.
;
; All keys are HKCU: the installer is per-user and takes no UAC prompt.

!macro NSIS_HOOK_POSTINSTALL
  ; Explorer "Open TermLab here". Capital %V is required: it is the
  ; substitution that resolves for all three roots below.
  WriteRegStr HKCU "Software\Classes\Directory\shell\TermLab" "MUIVerb" "Open TermLab here"
  WriteRegStr HKCU "Software\Classes\Directory\shell\TermLab" "Icon" "$INSTDIR\termlab.exe"
  WriteRegStr HKCU "Software\Classes\Directory\shell\TermLab\command" "" '"$INSTDIR\termlab.exe" --working-directory "%V"'

  WriteRegStr HKCU "Software\Classes\Directory\Background\shell\TermLab" "MUIVerb" "Open TermLab here"
  WriteRegStr HKCU "Software\Classes\Directory\Background\shell\TermLab" "Icon" "$INSTDIR\termlab.exe"
  WriteRegStr HKCU "Software\Classes\Directory\Background\shell\TermLab\command" "" '"$INSTDIR\termlab.exe" --working-directory "%V"'

  WriteRegStr HKCU "Software\Classes\Drive\shell\TermLab" "MUIVerb" "Open TermLab here"
  WriteRegStr HKCU "Software\Classes\Drive\shell\TermLab" "Icon" "$INSTDIR\termlab.exe"
  WriteRegStr HKCU "Software\Classes\Drive\shell\TermLab\command" "" '"$INSTDIR\termlab.exe" --working-directory "%V"'

  ; Default Programs: Settings > Default apps and "Open with".
  WriteRegStr HKCU "Software\Clients\Terminal\TermLab\Capabilities" "ApplicationName" "TermLab"
  WriteRegStr HKCU "Software\Clients\Terminal\TermLab\Capabilities" "ApplicationDescription" "A cross-platform terminal emulator and SSH client"
  WriteRegStr HKCU "Software\Clients\Terminal\TermLab\Capabilities" "ApplicationIcon" "$INSTDIR\termlab.exe,0"
  WriteRegStr HKCU "Software\RegisteredApplications" "TermLab" "Software\Clients\Terminal\TermLab\Capabilities"

  ; App Paths: makes Win+R "termlab" work without touching PATH.
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\App Paths\termlab.exe" "" "$INSTDIR\termlab.exe"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\App Paths\termlab.exe" "Path" "$INSTDIR"
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  DeleteRegKey HKCU "Software\Classes\Directory\shell\TermLab"
  DeleteRegKey HKCU "Software\Classes\Directory\Background\shell\TermLab"
  DeleteRegKey HKCU "Software\Classes\Drive\shell\TermLab"
  DeleteRegKey HKCU "Software\Clients\Terminal\TermLab"
  DeleteRegValue HKCU "Software\RegisteredApplications" "TermLab"
  DeleteRegKey HKCU "Software\Microsoft\Windows\CurrentVersion\App Paths\termlab.exe"
!macroend
```

Note the asymmetry on the last line of the uninstall macro:
`Software\RegisteredApplications` is shared with every other installed
application, so uninstall removes TermLab's **value** from it via
`DeleteRegValue`. Deleting the key itself would unregister every other
application on the machine.

- [ ] **Step 5: Run the parity test**

Run: `cargo test -p termlab_tauri --test windows_registration_parity 2>&1 | tail -30`

Expected: PASS, all four tests.

- [ ] **Step 6: Wire both files into `tauri.conf.json`**

In `crates/termlab_tauri/tauri.conf.json`, add a `windows` object inside `bundle`, leaving `active: false` alone:

```json
  "bundle": {
    "active": false,
    "createUpdaterArtifacts": true,
    "icon": [
      "icons/icon.icns",
      "icons/icon.ico",
      "icons/icon.png"
    ],
    "resources": {
      "frontend/themes/": "themes/"
    },
    "windows": {
      "wix": {
        "fragmentPaths": ["../../packaging/windows/registration.wxs"],
        "componentRefs": ["TermLabContextMenu", "TermLabDefaultApp", "TermLabAppPaths"]
      },
      "nsis": {
        "installMode": "currentUser",
        "installerHooks": "../../packaging/windows/installer-hooks.nsh"
      }
    }
  },
```

Paths are relative to `crates/termlab_tauri/` (where `tauri.conf.json` lives).

- [ ] **Step 7: Verify the config still parses**

Run: `python3 -c "import json; json.load(open('crates/termlab_tauri/tauri.conf.json')); print('valid json')"`

Expected: `valid json`.

- [ ] **Step 8: Commit**

```bash
git add packaging/windows/registration.wxs packaging/windows/installer-hooks.nsh crates/termlab_tauri/tests/windows_registration_parity.rs crates/termlab_tauri/tauri.conf.json
git commit -m "Declare Windows registration once for both bundlers

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 5: The build script

**Files:**
- Create: `scripts/build-windows.ps1`

**Interfaces:**
- Consumes from Task 1: `MSI_VERSION_OK`. If it was `false`, Step 3 below gains a numeric-version step; if `true`, skip that step.
- Consumes from Task 4: `tauri.conf.json` now references the fragment and hook, so `cargo tauri build` picks them up with no extra flags.
- Produces, for Task 7: the script's parameters — `-SkipTests`, `-SkipBundle`, `-Configuration <string>` (default `Release`), `-OutDir <string>` (default `dist`) — and its contract that it exits non-zero on any failure.

- [ ] **Step 1: Write the script**

Create `scripts/build-windows.ps1`:

```powershell
#Requires -Version 5.1
<#
.SYNOPSIS
    Builds TermLab's Windows artifacts: the NSIS setup.exe and the MSI.

.DESCRIPTION
    The single entry point for Windows builds. Both the developer VM and the
    GitHub Actions release job run this script, which is what keeps them from
    drifting: TermLab previously had one Windows installer path in CI and a
    different one in the Makefile, and only the unused one registered a
    context menu.

    Every step is fatal on failure and verifies its own output before
    reporting success.

.PARAMETER SkipTests
    Skip `cargo test`. Use only when tests ran in a separate step.

.PARAMETER SkipBundle
    Compile and test but do not produce installers. Used by CI on pull
    requests, where the goal is to catch a broken build early rather than to
    produce artifacts.

.PARAMETER Configuration
    Cargo profile. Defaults to Release.

.PARAMETER OutDir
    Where verified artifacts are copied. Defaults to dist.

.EXAMPLE
    pwsh scripts/build-windows.ps1
    pwsh scripts/build-windows.ps1 -SkipBundle
#>
[CmdletBinding()]
param(
    [switch]$SkipTests,
    [switch]$SkipBundle,
    [string]$Configuration = 'Release',
    [string]$OutDir = 'dist'
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

# Native commands do not trip $ErrorActionPreference, so every invocation is
# followed by an explicit exit-code check via Invoke-Checked.
function Invoke-Checked {
    param(
        [Parameter(Mandatory)][string]$What,
        [Parameter(Mandatory)][scriptblock]$Command
    )
    Write-Host "==> $What" -ForegroundColor Cyan
    & $Command
    if ($LASTEXITCODE -ne 0) {
        throw "$What failed with exit code $LASTEXITCODE"
    }
}

function Assert-Tool {
    param(
        [Parameter(Mandatory)][string]$Name,
        [Parameter(Mandatory)][string]$Remedy
    )
    if (-not (Get-Command $Name -ErrorAction SilentlyContinue)) {
        throw "required tool '$Name' is not on PATH. $Remedy"
    }
}

$RepoRoot = Split-Path -Parent $PSScriptRoot
Set-Location $RepoRoot
Write-Host "Repository: $RepoRoot"

# --- 1. Preflight ---------------------------------------------------------
# WiX and NSIS are downloaded by tauri-bundler itself and need no handling.
Write-Host "==> Preflight" -ForegroundColor Cyan
Assert-Tool cargo 'Install Rust from https://rustup.rs'
Assert-Tool node  'Install Node 20+ from https://nodejs.org'
Assert-Tool npm   'Install Node 20+ from https://nodejs.org'
Assert-Tool git   'Install Git from https://git-scm.com'
Assert-Tool javac 'Install a JDK: winget install Microsoft.OpenJDK.21'
Assert-Tool jar   'Install a JDK: winget install Microsoft.OpenJDK.21'

if (-not $SkipBundle) {
    cargo tauri --version *> $null
    if ($LASTEXITCODE -ne 0) {
        Invoke-Checked 'Install the Tauri CLI' { cargo install tauri-cli --version '^2' --locked }
    }
}

# --- 2. Frontend vendor bundle -------------------------------------------
# Tauri's beforeBuildCommand also runs this, so in the bundling path it runs
# twice. That redundancy is deliberate: -SkipBundle never invokes Tauri, and
# a missing bundle ships an editor that shows the "bundle missing" toast.
$FrontendDir = Join-Path $RepoRoot 'crates\termlab_tauri\frontend'
Invoke-Checked 'Install frontend dependencies' { npm --prefix $FrontendDir ci }
Invoke-Checked 'Build frontend vendor bundles' { npm --prefix $FrontendDir run build:vendor }

# --- 3. Java Plugin SDK jar (embedded in the binary) ----------------------
$ClassesDir = Join-Path $RepoRoot 'java-sdk\build\classes'
New-Item -ItemType Directory -Force -Path $ClassesDir | Out-Null
$JavaSources = Get-ChildItem (Join-Path $RepoRoot 'java-sdk\src\termlab\plugin\*.java')
if ($JavaSources.Count -eq 0) {
    throw 'no Java SDK sources found under java-sdk/src/termlab/plugin/'
}
Invoke-Checked 'Compile the Java Plugin SDK' { javac -d $ClassesDir $JavaSources.FullName }
$SdkJar = Join-Path $RepoRoot 'java-sdk\build\termlab-plugin-sdk.jar'
Invoke-Checked 'Package the Java Plugin SDK jar' { jar cf $SdkJar -C $ClassesDir . }
if (-not (Test-Path $SdkJar)) {
    throw "the Java SDK jar was not produced at $SdkJar"
}

# --- 4. Test --------------------------------------------------------------
if ($SkipTests) {
    Write-Host '==> Tests skipped (-SkipTests)' -ForegroundColor Yellow
} elseif ($Configuration -eq 'Release') {
    Invoke-Checked 'Run the test suite' { cargo test --release --workspace }
} else {
    Invoke-Checked 'Run the test suite' { cargo test --workspace }
}

# --- 5. Bundle ------------------------------------------------------------
if ($SkipBundle) {
    if ($Configuration -eq 'Release') {
        Invoke-Checked 'Compile (no bundle)' { cargo build --release -p termlab_tauri }
    } else {
        Invoke-Checked 'Compile (no bundle)' { cargo build -p termlab_tauri }
    }
    Write-Host '==> Bundling skipped (-SkipBundle)' -ForegroundColor Yellow
    exit 0
}

Invoke-Checked 'Bundle installers' { cargo tauri build --config '{"bundle":{"active":true}}' }

# --- 6. Verify and collect ------------------------------------------------
$BundleRoot = Join-Path $RepoRoot 'target\release\bundle'
$Setup = Get-ChildItem -Path (Join-Path $BundleRoot 'nsis') -Filter '*-setup.exe' -ErrorAction SilentlyContinue | Select-Object -First 1
$Msi   = Get-ChildItem -Path (Join-Path $BundleRoot 'msi')  -Filter '*.msi'       -ErrorAction SilentlyContinue | Select-Object -First 1

if (-not $Setup) { throw "no NSIS setup.exe under $BundleRoot\nsis — the bundler reported success but produced nothing" }
if (-not $Msi)   { throw "no MSI under $BundleRoot\msi — the bundler reported success but produced nothing" }

# A truncated or stub installer is worse than a missing one, because it looks
# like a successful build. TermLab's real installers are tens of megabytes.
$MinBytes = 5MB
foreach ($artifact in @($Setup, $Msi)) {
    if ($artifact.Length -lt $MinBytes) {
        throw "$($artifact.Name) is only $($artifact.Length) bytes, well under the $MinBytes floor — treating as a failed build"
    }
}

$OutPath = Join-Path $RepoRoot $OutDir
New-Item -ItemType Directory -Force -Path $OutPath | Out-Null
Copy-Item $Setup.FullName -Destination $OutPath -Force
Copy-Item $Msi.FullName   -Destination $OutPath -Force

Write-Host ''
Write-Host 'Build complete.' -ForegroundColor Green
Write-Host "  $($Setup.Name)  ($([math]::Round($Setup.Length / 1MB, 1)) MB)"
Write-Host "  $($Msi.Name)  ($([math]::Round($Msi.Length / 1MB, 1)) MB)"
Write-Host "  copied to $OutPath"
```

- [ ] **Step 2: If Task 1 recorded `MSI_VERSION_OK: no`, add a numeric-version step**

Only if the baseline showed tauri-bundler rejects `3.0.0-rc.2`. Insert before
`# --- 5. Bundle`:

```powershell
# --- 4b. Numeric MSI version ---------------------------------------------
# MSI ProductVersion must be at most three dot-separated integers, so a
# pre-release suffix is stripped for the bundle only. Cargo.toml is not
# modified; the value is passed through --config.
$CargoVersion = (Select-String -Path (Join-Path $RepoRoot 'Cargo.toml') -Pattern '^version = "(.+)"' | Select-Object -First 1).Matches[0].Groups[1].Value
$NumericVersion = ($CargoVersion -split '-')[0]
Write-Host "==> Cargo version $CargoVersion, MSI ProductVersion $NumericVersion" -ForegroundColor Cyan
```

and change the bundle invocation to:

```powershell
Invoke-Checked 'Bundle installers' { cargo tauri build --config "{`"version`":`"$NumericVersion`",`"bundle`":{`"active`":true}}" }
```

**Warning:** this changes the artifact filenames from `TermLab_3.0.0-rc.2_x64-setup.exe` to `TermLab_3.0.0_x64-setup.exe`, which breaks the Global Constraint that release artifact names must not change. If this step is needed, stop and raise it — `release.yml`'s upload steps and `latest.json` would both need updating, and that is a decision, not an implementation detail.

- [ ] **Step 3: Verify the script is syntactically valid**

On any machine with PowerShell (macOS included, via `pwsh`):

```bash
pwsh -NoProfile -Command '$null = [System.Management.Automation.Language.Parser]::ParseFile("scripts/build-windows.ps1", [ref]$null, [ref]$errors); if ($errors) { $errors; exit 1 } else { "syntax ok" }'
```

Expected: `syntax ok`. If `pwsh` is not installed locally, run the same check on the VM over SSH with `powershell -NoProfile -Command`.

- [ ] **Step 4: Run it on the VM, end to end**

```bash
ssh dustin@192.168.1.125 'powershell -NoProfile -Command "cd C:\Users\dustin\conch; git fetch origin; git reset --hard origin/feat/windows-installer-registration; powershell -NoProfile -File scripts\build-windows.ps1"'
```

Expected: the run ends with `Build complete.` and both artifact lines. If it fails, the error names the failing step — fix and re-run. Do not add `|| true` or soften a check to get past a failure.

- [ ] **Step 5: Commit**

```bash
git add scripts/build-windows.ps1
git commit -m "Add the single Windows build entry point

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 6: Install verification on the VM

**Files:**
- Create: `scripts/tests/verify-windows-install.ps1`

**Interfaces:**
- Consumes from Task 1: `INSTALLED_EXE_NAME`, `INSTALL_DIR`.
- Consumes from Task 4: the exact registry key set.
- Consumes from Task 5: the artifacts in `dist/`.

- [ ] **Step 1: Write the verification script**

Create `scripts/tests/verify-windows-install.ps1`:

```powershell
#Requires -Version 5.1
<#
.SYNOPSIS
    Installs the built MSI, asserts TermLab registered itself correctly,
    then uninstalls and asserts it cleaned up.

.DESCRIPTION
    Run on Windows after scripts/build-windows.ps1. This is committed rather
    than performed ad hoc so the check is repeatable on every release.

.PARAMETER MsiPath
    The MSI to verify. Defaults to the newest .msi in dist/.
#>
[CmdletBinding()]
param(
    [string]$MsiPath
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$RepoRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
if (-not $MsiPath) {
    $candidate = Get-ChildItem (Join-Path $RepoRoot 'dist') -Filter '*.msi' -ErrorAction SilentlyContinue |
        Sort-Object LastWriteTime -Descending | Select-Object -First 1
    if (-not $candidate) { throw 'no MSI in dist/. Run scripts/build-windows.ps1 first.' }
    $MsiPath = $candidate.FullName
}
Write-Host "Verifying $MsiPath"

$Failures = New-Object System.Collections.Generic.List[string]

function Assert-True {
    param([Parameter(Mandatory)][bool]$Condition, [Parameter(Mandatory)][string]$Message)
    if ($Condition) {
        Write-Host "  PASS  $Message" -ForegroundColor Green
    } else {
        Write-Host "  FAIL  $Message" -ForegroundColor Red
        $script:Failures.Add($Message)
    }
}

# Every key the installers are supposed to write.
$ContextMenuRoots = @(
    'Software\Classes\Directory\shell\TermLab',
    'Software\Classes\Directory\Background\shell\TermLab',
    'Software\Classes\Drive\shell\TermLab'
)
$OtherKeys = @(
    'Software\Clients\Terminal\TermLab\Capabilities',
    'Software\Microsoft\Windows\CurrentVersion\App Paths\termlab.exe'
)

function Invoke-Msi {
    param([Parameter(Mandatory)][string]$Arguments)
    $p = Start-Process msiexec -ArgumentList $Arguments -Wait -PassThru
    if ($p.ExitCode -ne 0) { throw "msiexec $Arguments failed with exit code $($p.ExitCode)" }
}

# --- Install --------------------------------------------------------------
Write-Host "`n== Installing ==" -ForegroundColor Cyan
Invoke-Msi "/i `"$MsiPath`" /qn"

Write-Host "`n== Registration ==" -ForegroundColor Cyan
foreach ($root in $ContextMenuRoots) {
    $key = "HKCU:\$root"
    Assert-True (Test-Path $key) "$root exists"
    if (Test-Path $key) {
        $label = (Get-ItemProperty $key -Name 'MUIVerb' -ErrorAction SilentlyContinue).MUIVerb
        Assert-True ($label -eq 'Open TermLab here') "$root has the label 'Open TermLab here' (got '$label')"
    }

    $cmdKey = "HKCU:\$root\command"
    Assert-True (Test-Path $cmdKey) "$root\command exists"
    if (Test-Path $cmdKey) {
        $cmd = (Get-ItemProperty $cmdKey -Name '(default)' -ErrorAction SilentlyContinue).'(default)'
        Assert-True ($cmd -like '*--working-directory*') "$root command passes --working-directory (got '$cmd')"
        Assert-True ($cmd -like '*%V*') "$root command uses capital %V (got '$cmd')"

        # The command must point at a file that actually exists, which is what
        # catches a wrong INSTALLDIR or a renamed binary.
        if ($cmd -match '^"([^"]+)"') {
            $exe = $Matches[1]
            Assert-True (Test-Path $exe) "$root command target exists on disk: $exe"
        } else {
            Assert-True $false "$root command is not a quoted path: $cmd"
        }
    }
}

foreach ($key in $OtherKeys) {
    Assert-True (Test-Path "HKCU:\$key") "$key exists"
}

$registered = (Get-ItemProperty 'HKCU:\Software\RegisteredApplications' -Name 'TermLab' -ErrorAction SilentlyContinue).TermLab
Assert-True ($registered -eq 'Software\Clients\Terminal\TermLab\Capabilities') `
    "RegisteredApplications\TermLab points at the Capabilities key (got '$registered')"

# --- Launch ---------------------------------------------------------------
Write-Host "`n== Launch ==" -ForegroundColor Cyan
$appPath = (Get-ItemProperty 'HKCU:\Software\Microsoft\Windows\CurrentVersion\App Paths\termlab.exe' -Name '(default)' -ErrorAction SilentlyContinue).'(default)'
if ($appPath -and (Test-Path $appPath)) {
    $proc = Start-Process $appPath -ArgumentList '--working-directory', 'C:\Windows' -PassThru
    Start-Sleep -Seconds 8
    $alive = -not $proc.HasExited
    Assert-True $alive "termlab --working-directory C:\Windows is still running after 8s (exit code: $(if ($proc.HasExited) { $proc.ExitCode } else { 'n/a' }))"
    if ($alive) { $proc.Kill(); $proc.WaitForExit() }
} else {
    Assert-True $false "App Paths entry points at a real file (got '$appPath')"
}

# --- Uninstall ------------------------------------------------------------
Write-Host "`n== Uninstalling ==" -ForegroundColor Cyan
Invoke-Msi "/x `"$MsiPath`" /qn"

Write-Host "`n== Cleanup ==" -ForegroundColor Cyan
foreach ($root in $ContextMenuRoots + $OtherKeys) {
    Assert-True (-not (Test-Path "HKCU:\$root")) "$root removed on uninstall"
}
$leftover = (Get-ItemProperty 'HKCU:\Software\RegisteredApplications' -Name 'TermLab' -ErrorAction SilentlyContinue)
Assert-True ($null -eq $leftover) 'RegisteredApplications\TermLab removed on uninstall'

# --- Result ---------------------------------------------------------------
Write-Host ''
if ($Failures.Count -gt 0) {
    Write-Host "$($Failures.Count) check(s) failed:" -ForegroundColor Red
    $Failures | ForEach-Object { Write-Host "  - $_" -ForegroundColor Red }
    exit 1
}
Write-Host 'All install verification checks passed.' -ForegroundColor Green
```

- [ ] **Step 2: Run it on the VM**

```bash
ssh dustin@192.168.1.125 'powershell -NoProfile -Command "cd C:\Users\dustin\conch; git fetch origin; git reset --hard origin/feat/windows-installer-registration; powershell -NoProfile -File scripts\tests\verify-windows-install.ps1"'
```

Expected: every check PASS and `All install verification checks passed.`

Common real failures and what they mean:
- *command target does not exist on disk* — `[INSTALLDIR]termlab.exe` resolved wrongly. Re-read Task 1's `INSTALLED_EXE_NAME` and fix `registration.wxs`.
- *key does not exist* — the `componentRefs` in `tauri.conf.json` do not match the `Component Id`s in `registration.wxs`.
- *process exited immediately* — the flag reached a binary that does not understand it. Confirm Task 2 and 3 are in the build being installed.

- [ ] **Step 3: Verify the NSIS installer the same way, manually**

The script targets the MSI. Confirm the setup.exe registers identically:

```bash
ssh dustin@192.168.1.125 'powershell -NoProfile -Command "cd C:\Users\dustin\conch; Start-Process (Get-ChildItem dist\*-setup.exe | Select-Object -First 1).FullName -ArgumentList \"/S\" -Wait; Get-ChildItem HKCU:\Software\Classes\Directory\shell\TermLab"'
```

Expected: the key exists. Then uninstall via the uninstaller in the install directory and confirm it is gone.

- [ ] **Step 4: Manual Explorer check**

This cannot be automated without driving the shell UI. On the VM's desktop, with the MSI installed:

1. Right-click a folder → **Show more options** → confirm "Open TermLab here" appears and opens a terminal in that folder.
2. Right-click the empty background inside an open folder → same.
3. Right-click a drive in This PC → same.

Record the result in the commit message or the PR body, including that the entry is under "Show more options" on Windows 11 — that is expected and documented, not a defect.

- [ ] **Step 5: Commit**

```bash
git add scripts/tests/verify-windows-install.ps1
git commit -m "Add repeatable Windows install verification

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 7: Point CI at the script

**Files:**
- Modify: `.github/workflows/release.yml:131-147` (the `windows` job's Tauri CLI, Java SDK, Test, and Build steps)
- Modify: `.github/workflows/ci.yml:76-98` (the `windows` job)

**Interfaces:**
- Consumes from Task 5: `scripts/build-windows.ps1` and its `-SkipBundle` switch.
- Produces: no change to artifact paths or names, so `latest.json` and the updater stay working.

- [ ] **Step 1: Replace the release job's build steps**

In `.github/workflows/release.yml`, in the `windows` job, replace these four steps:

```yaml
      - name: Install Tauri CLI
        run: cargo install tauri-cli --version "^2"

      - name: Build Java SDK JAR (embedded in binary)
        run: |
          mkdir -p java-sdk/build/classes
          javac -d java-sdk/build/classes java-sdk/src/termlab/plugin/*.java
          jar cf java-sdk/build/termlab-plugin-sdk.jar -C java-sdk/build/classes .

      - name: Test
        run: cargo test --release

      - name: Build with Tauri
        env:
          TAURI_SIGNING_PRIVATE_KEY: ${{ secrets.TAURI_SIGNING_PRIVATE_KEY }}
          TAURI_SIGNING_PRIVATE_KEY_PASSWORD: ${{ secrets.TAURI_SIGNING_PRIVATE_KEY_PASSWORD }}
        run: cargo tauri build --config '{"bundle":{"active":true}}'
```

with this single step:

```yaml
      # Everything Windows-specific lives in the script so this job and the
      # developer VM cannot drift. See scripts/build-windows.ps1.
      - name: Build Windows artifacts
        shell: pwsh
        env:
          TAURI_SIGNING_PRIVATE_KEY: ${{ secrets.TAURI_SIGNING_PRIVATE_KEY }}
          TAURI_SIGNING_PRIVATE_KEY_PASSWORD: ${{ secrets.TAURI_SIGNING_PRIVATE_KEY_PASSWORD }}
        run: ./scripts/build-windows.ps1
```

The `shell: pwsh` override is required: the job sets `defaults.run.shell: bash`.
Leave every other step in the job, including all four upload steps, exactly as
they are — they read from `target/release/bundle/`, which the script does not
move.

- [ ] **Step 2: Replace the CI job's build steps**

In `.github/workflows/ci.yml`, in the `windows` job, replace:

```yaml
      - name: Build Java SDK JAR (embedded in binary)
        run: |
          mkdir -p java-sdk/build/classes
          javac -d java-sdk/build/classes java-sdk/src/termlab/plugin/*.java
          jar cf java-sdk/build/termlab-plugin-sdk.jar -C java-sdk/build/classes .

      - name: Test
        run: cargo test --workspace
```

with:

```yaml
      # Same script the release job and the dev VM run, minus the bundling.
      # This is what catches a broken WiX fragment or NSIS hook at PR time.
      - name: Build and test
        shell: pwsh
        run: ./scripts/build-windows.ps1 -SkipBundle -Configuration Debug
```

Leave the `Clippy` step as it is.

- [ ] **Step 3: Add a Node setup step to the CI windows job**

The script runs `npm ci`, and `ci.yml`'s `windows` job has no `setup-node`.
Add it after the checkout step, matching the release job:

```yaml
      - uses: actions/setup-node@v4
        with:
          node-version: '20'
```

- [ ] **Step 4: Validate both workflow files parse**

```bash
python3 -c "
import yaml
for f in ['.github/workflows/ci.yml', '.github/workflows/release.yml']:
    yaml.safe_load(open(f))
    print(f, 'valid')
"
```

Expected: both print `valid`. If PyYAML is missing, `pip3 install pyyaml` first.

- [ ] **Step 5: Confirm the artifact paths the upload steps use are untouched**

```bash
grep -n "bundle/nsis\|bundle/msi" .github/workflows/release.yml
```

Expected: the same four `target/release/bundle/...` paths as before this task.
If any changed, revert that — the updater depends on them.

- [ ] **Step 6: Commit**

```bash
git add .github/workflows/release.yml .github/workflows/ci.yml
git commit -m "Build Windows artifacts through the shared script in CI

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 8: Remove the superseded path and document the new one

This is last on purpose: nothing gets deleted until Tasks 5–7 have proven the
replacement works.

**Files:**
- Delete: `packaging/windows/termlab.wxs`
- Delete: `scripts/set_msi_version.py`
- Modify: `Makefile` — remove the `msi` and `exe` targets, their `help` lines, and the two `termlab.wxs` assertions in `bump`
- Modify: `README.md` — Windows install and build instructions

**Interfaces:**
- Consumes: nothing. This task only removes what Tasks 4–7 replaced.

- [ ] **Step 1: Confirm nothing still references the files being deleted**

```bash
grep -rn "termlab.wxs\|set_msi_version" --exclude-dir=.git --exclude-dir=target --exclude-dir=node_modules --exclude-dir=.worktrees .
```

Expected: hits only in `Makefile`, `docs/superpowers/specs/2026-09-03-windows-installer-design.md`, and this plan. If anything else references them, stop and handle it before deleting.

- [ ] **Step 2: Delete the superseded files**

```bash
git rm packaging/windows/termlab.wxs scripts/set_msi_version.py
```

Note: `packaging/windows/license.rtf` stays — Tauri's WiX bundler can use it.

- [ ] **Step 3: Remove the Makefile's Windows targets**

Delete these two blocks from `Makefile` in their entirety, including their
`# ---` comment headers:

```make
.PHONY: msi
msi: build
	...

.PHONY: exe
exe: build
	...
```

And remove these two lines from the `help` target:

```make
	@echo "  msi            Build .msi installer (run on Windows)"
	@echo "  exe            Build portable .exe (run on Windows)"
```

Add one line in their place, under the "Local builds" heading:

```make
	@echo "  (Windows)      Run scripts/build-windows.ps1 on Windows"
```

- [ ] **Step 4: Remove the Makefile's `termlab.wxs` assertions from `bump`**

In the `bump` target, delete the `set_msi_version.py` invocation:

```make
	python3 scripts/set_msi_version.py "$(V_NUMERIC)"
```

and the two assertions that read the deleted file:

```make
	@grep -q 'Codepage="1252" Version="$(V_NUMERIC)"' packaging/windows/termlab.wxs \
		|| { echo "error: termlab.wxs Package Version was not updated to $(V_NUMERIC)"; exit 1; }
	@grep -q 'InstallerVersion="200"' packaging/windows/termlab.wxs \
		|| { echo "error: InstallerVersion was clobbered — it declares the minimum Windows Installer version and must stay 200"; exit 1; }
```

Also remove `packaging/windows/termlab.wxs` from the `git add` line in the
`release` target.

- [ ] **Step 5: Verify the Makefile still works**

```bash
make help
```

Expected: the help text prints with no `msi`/`exe` lines and no errors.

```bash
make -n bump V=9.9.9
```

Expected: the recipe prints without referencing `set_msi_version.py` or `termlab.wxs`.

- [ ] **Step 6: Document the Windows story in the README**

Add a Windows subsection to `README.md`'s install/build area. Place it beside
the existing macOS and Linux instructions, matching their heading level.

```markdown
### Windows

Download `TermLab_<version>_x64-setup.exe` (or the `.msi`) from the
[releases page](https://github.com/an0nn30/conch/releases). The installer is
per-user and needs no administrator prompt; it installs to
`%LOCALAPPDATA%\Programs\TermLab`.

The installer registers TermLab with Windows:

- **"Open TermLab here"** in the Explorer context menu, for folders, folder
  backgrounds, and drives.
- **Settings > Default apps**, so TermLab appears alongside other terminals.
- **`Win+R` → `termlab`**, via an App Paths entry.

Uninstalling removes all of these.

Two current limitations:

- On Windows 11 the context-menu entry is under **"Show more options"**
  (or `Shift`+`F10`), not the top-level menu. A top-level entry requires a
  signed sparse MSIX package with an `IExplorerCommand` handler, which
  TermLab does not ship yet.
- TermLab does not appear in Windows 11's **"Default terminal application"**
  dropdown. That setting requires implementing ConPTY handoff
  (`ITerminalHandoff3`), which is separate from the default-app registration
  above.

#### Building on Windows

Requires Rust, Node 20+, Git, and a JDK
(`winget install Microsoft.OpenJDK.21`). Then:

```powershell
pwsh scripts/build-windows.ps1
```

This produces the setup.exe and the MSI in `dist/`. It is the same script CI
runs, so a local build matches a release build. To verify the installer
registers and cleans up correctly:

```powershell
pwsh scripts/tests/verify-windows-install.ps1
```
```

- [ ] **Step 7: Run the full test suite**

```bash
cargo test --workspace 2>&1 | tail -20
```

Expected: all tests pass, including `windows_registration_parity`.

- [ ] **Step 8: Confirm the plugin SDK docs need no change**

Per `CLAUDE.md`, any change to the Lua or Java plugin surface must update
`docs/plugin-sdk.md` in the same commit. This branch touches no plugin API:

```bash
git diff --stat origin/main...HEAD -- docs/plugin-sdk.md docs/plugin-security-model.md java-sdk/ crates/termlab_plugin_sdk/
```

Expected: empty output. If not, the branch grew scope that needs documenting.

- [ ] **Step 9: Commit**

```bash
git add -A
git commit -m "Remove the superseded Windows installer path and document the new one

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

- [ ] **Step 10: Push the branch**

```bash
git push -u origin feat/windows-installer-registration
```

Per `CLAUDE.md`, do not open a PR unless asked.

---

## Verification Summary

What each layer is actually covered by, so nothing is claimed without evidence:

| Behavior | Covered by |
|---|---|
| `--working-directory` parsing, precedence, usage errors | `cargo test -p termlab_tauri --lib cli::tests` (Task 2) |
| The flag actually moves the process | Manual binary run, Task 3 Step 3 |
| MSI and setup.exe register identical keys | `cargo test -p termlab_tauri --test windows_registration_parity` (Task 4), runs on every platform in CI |
| Registry keys are correct after a real install | `scripts/tests/verify-windows-install.ps1` on the VM (Task 6) |
| Keys are removed on uninstall | Same script, cleanup section (Task 6) |
| The context menu appears and works in Explorer | **Manual only** (Task 6 Step 4) — cannot be automated without driving the shell UI |
| The build script produces real artifacts | Size and existence checks inside the script (Task 5), exercised on the VM |
| CI and the VM cannot drift | Both invoke `scripts/build-windows.ps1` (Task 7) |

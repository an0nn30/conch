# Windows Installer, Registration, and Build Script — Design

Date: 2026-09-03
Branch: `feat/windows-installer-registration`

## Problem

TermLab has no reproducible Windows build entry point, and its Windows
installers do not register the app with the OS.

Three concrete defects motivate this work:

1. **Two divergent installer paths.** `.github/workflows/release.yml` builds
   Windows artifacts with `cargo tauri build`, producing an NSIS
   `TermLab_<version>_x64-setup.exe` and Tauri's own MSI. Neither registers
   anything beyond a Start menu shortcut. Separately, the `Makefile` has `msi`
   and `exe` targets that build a hand-written `packaging/windows/termlab.wxs`
   which *does* declare an Explorer context menu — but CI never runs those
   targets, so every shipped release lacks the context menu.

2. **The hand-written context menu could not work even if it shipped.** Its
   command is `termlab.exe --working-directory "%v"`, and `crates/termlab_tauri/src/cli.rs`
   accepts only `[PATH ...]` — an unrecognized flag takes the `UnknownFlag`
   branch and exits 2. Passing a bare directory instead would also fail:
   `crates/termlab_tauri/frontend/app/features/editor/open-path-routing.js`
   answers directories with a "not supported yet" toast. It also registers only
   `Directory\Background` (right-click empty space), not `Directory` or `Drive`,
   and uses lowercase `%v`, which does not resolve correctly for
   `Directory\shell`.

3. **No OS registration.** TermLab does not appear in Settings > Default apps,
   is not reachable from `Win+R`, and has no "Open with" presence.

## Decisions

These were settled during brainstorming and are not open questions:

| Question | Decision |
|---|---|
| What "register as a Terminal" means | Default-app registration: `Software\Clients\Terminal`, `RegisteredApplications`, and `App Paths`. **Not** the Windows 11 ConPTY handoff (`ITerminalHandoff3`), which is deferred to its own project. |
| How "Open TermLab here" works | A real `--working-directory <dir>` flag on the app, not a registry-level `cd &&` wrapper. |
| Which installer survives | Consolidate on the Tauri bundler. Registration is defined once and consumed by both the MSI (WiX fragment) and the setup.exe (NSIS hook). |
| Install scope | Per-user, no admin. `%LOCALAPPDATA%\Programs\TermLab`, all keys under `HKCU`. |
| Windows 11 menu placement | Legacy registry verbs only. The entry appears under "Show more options"; the sparse-MSIX `IExplorerCommand` handler that would make it top-level is deferred. |

## Architecture

Four components, each independently testable:

```
scripts/build-windows.ps1              the single build entry point (VM + CI)
packaging/windows/registration.wxs     registration for the MSI      \ one definition,
packaging/windows/installer-hooks.nsh  registration for the setup.exe / two consumers
crates/termlab_tauri/src/cli.rs        the --working-directory flag
.github/workflows/{release,ci}.yml     call the build script, nothing else
```

Nothing else calls the bundlers. The VM and CI run the identical script, which
is what keeps them from drifting again.

### Component 1 — `scripts/build-windows.ps1`

Parameters: `-SkipTests`, `-SkipBundle`, `-Configuration Release`, `-OutDir dist`.

Steps, each fatal on failure (no `|| true`, no unchecked success messages):

1. **Preflight.** Assert `cargo`, `node`, `npm`, `git` are on PATH. Assert a JDK
   is present (`javac` and `jar`); the target VM has none, so the failure
   message must name the fix (`winget install Microsoft.OpenJDK.21`) rather
   than let the build silently produce a binary with no embedded Java SDK jar.
   Install `cargo tauri` on demand if absent. WiX and NSIS are downloaded by
   tauri-bundler itself and need no handling.
2. **Frontend vendor bundle.** `npm ci` then `npm run build:vendor` in
   `crates/termlab_tauri/frontend`. Skipping this ships an editor that shows the
   "bundle missing" toast — the same gap the `Makefile` comments call out.
3. **Java SDK jar.** `javac` + `jar`, mirroring `release.yml` steps 134–138.
4. **Test.** `cargo test --release --workspace`, unless `-SkipTests`.
5. **Bundle.** `cargo tauri build --config '{"bundle":{"active":true}}'`, unless
   `-SkipBundle`.
6. **Verify and collect.** Assert the setup.exe and the .msi exist and are
   non-trivially sized, then copy both into `-OutDir` with the version in the
   filename.

`-SkipBundle` exists so `ci.yml` can compile and test on PRs without paying for
a full bundle, while still catching a Rust or frontend break early.

### Component 2 — registration, defined once

Two files, asserted by tests to declare the same key set:

- `packaging/windows/registration.wxs` — **WiX v3 schema**
  (`http://schemas.microsoft.com/wix/2006/wi`). Tauri v2's MSI bundler is WiX 3;
  the existing `termlab.wxs` uses the v4 schema and cannot be reused as a
  fragment.
- `packaging/windows/installer-hooks.nsh` — defines `NSIS_HOOK_POSTINSTALL` and
  `NSIS_HOOK_POSTUNINSTALL`.

Keys written, all under `HKCU`:

**Context menu** — three roots, so the entry appears when right-clicking a
folder, the background of an open folder, and a drive:

- `Software\Classes\Directory\shell\TermLab`
- `Software\Classes\Directory\Background\shell\TermLab`
- `Software\Classes\Drive\shell\TermLab`

Each carries `MUIVerb = "Open TermLab here"` and `Icon = <installed exe>`, with
a `command` subkey of:

```
"<installed exe>" --working-directory "%V"
```

Capital `%V` is required: it is the one substitution that resolves correctly for
all three roots.

**Default-app registration:**

- `Software\Clients\Terminal\TermLab\Capabilities` with `ApplicationName`,
  `ApplicationDescription`, and `ApplicationIcon`.
- `Software\RegisteredApplications` value `TermLab` pointing at that
  `Capabilities` path. This is what surfaces TermLab in Settings > Default apps.

**App Paths:**

- `Software\Microsoft\Windows\CurrentVersion\App Paths\termlab.exe`, default
  value = full exe path, so `Win+R -> termlab` works.

Uninstall removes every key above. WiX components remove on uninstall by
construction; the NSIS side does it explicitly in `NSIS_HOOK_POSTUNINSTALL`.

**`tauri.conf.json` changes.** `bundle.active` stays `false` (CI and the script
pass it via `--config`). Added:

- `bundle.windows.wix.fragmentPaths` and `bundle.windows.wix.componentRefs`
- `bundle.windows.nsis.installerHooks`
- `bundle.windows.nsis.installMode: "currentUser"`

**Removed.** `packaging/windows/termlab.wxs`, `scripts/set_msi_version.py`, the
`Makefile` `msi` and `exe` targets, and the two `termlab.wxs` assertions in the
`Makefile` `bump` target. These are the divergence; leaving them means leaving
two places to keep correct.

### Component 3 — `--working-directory` in the app

The flag is added to the pure decision layer in `crates/termlab_tauri/src/cli.rs`:
`CliDecision` and `CliPlan` each carry `working_directory: Option<String>`.

The implementation is small because the existing code already does the work:

- On Windows, `forward_or_detach` is a `#[cfg(not(unix))]` stub that boots the
  app in-process. No IPC is needed — and none exists on Windows, since
  `crates/termlab_tauri/src/ipc.rs` is `#[cfg(unix)]` throughout. Each
  right-click therefore launches a fresh TermLab window, which is the intended
  behavior for v1.
- `PtyBackend` spawns the shell with the *inherited* process cwd
  (`crates/termlab_tauri/src/pty.rs`, `spawn_shell_for_pane` sets no cwd).
- `get_workspace_dir` already reports the launch cwd, which drives tab titles.

So `main.rs` calls `std::env::set_current_dir(dir)` before `platform::init()`,
and that single call yields both a shell in the right directory and a correct
tab title. `set_current_dir` is the right semantic here rather than a blunt
instrument: it means "this instance was launched in this directory", which is
exactly what the context menu is expressing.

Error handling:

- A nonexistent or non-directory argument logs a warning and falls through to a
  normal launch. A broken context-menu entry must not leave the user with an app
  that refuses to start.
- `--working-directory` together with positional path arguments is a usage
  error (exit 2). The two express different intents and combining them has no
  defined meaning.
- `--help` and `--version` keep precedence over the flag, matching the existing
  precedence rules.
- `HELP_TEXT` gains the flag.

### Component 4 — CI wiring

`release.yml`, `windows` job: the Java SDK, test, and build steps (134–147)
collapse into a single `pwsh scripts/build-windows.ps1` invocation. Upload steps
and artifact names are unchanged, so `latest.json` and the auto-updater — which
pulls `TermLab_${VERSION}_x64-setup.exe` — are untouched.

`ci.yml` gains a `windows` job running `pwsh scripts/build-windows.ps1 -SkipBundle`,
so a broken WiX fragment or NSIS hook fails at PR time rather than at tag time.

## Testing

**Rust unit tests** (`cli.rs`, extending the existing `#[cfg(test)] mod tests`):

- `--working-directory <dir>` parses into `CliPlan` with the directory set.
- `--working-directory` with no value is a usage error.
- `--working-directory` combined with path arguments is a usage error.
- `--help` and `--version` still win over the flag.
- The `APP_RUNNING` / `DETACHED` env markers interact with the flag the same way
  they do with paths.

**Registration parity test.** A test asserts that `registration.wxs` and
`installer-hooks.nsh` declare the same set of registry key paths, so the two
consumers cannot drift.

**VM install verification** — `scripts/tests/verify-windows-install.ps1`, run
over SSH against `dustin@192.168.1.125` (repo at `C:\Users\dustin\conch`):

1. `msiexec /i <msi> /qn`
2. Assert every key above exists, and that each `command` value points at the
   real installed exe path.
3. Launch `termlab.exe --working-directory C:\Windows`; confirm the process
   starts.
4. `msiexec /x <msi> /qn`
5. Assert every key is gone.

This is a committed script, not a one-off session, so it is repeatable.

**Manual.** Right-clicking a folder, a folder background, and a drive in
Explorer, confirming the entry appears and opens a shell in that directory. This
cannot be automated without driving the shell UI; the result will be reported
explicitly.

## Documentation

`README.md` gains Windows install and build instructions. Neither
`docs/plugin-sdk.md` nor `docs/plugin-security-model.md` is affected — this
change touches no plugin API surface.

## Known limitations, to be documented in the README

- On Windows 11 the context-menu entry appears under **"Show more options"**
  (Shift+F10), not at the top level. Top-level placement requires a sparse MSIX
  package with an `IExplorerCommand` handler and a code-signing certificate;
  deferred.
- `Software\Clients\Terminal` is not a capability class Windows itself consumes
  for the "Default terminal application" setting. That setting requires ConPTY
  handoff via `ITerminalHandoff3`; deferred. The registration delivered here
  puts TermLab in Settings > Default apps and "Open with".
- Each context-menu invocation launches a new TermLab instance rather than a new
  window in a running one, because IPC is Unix-only. Windows IPC is out of scope
  for this branch.

## Open risk

The current version is `3.0.0-rc.2`, and MSI `ProductVersion` must be numeric
(at most three dot-separated integers). Whether tauri-bundler drops the
pre-release suffix or fails outright must be verified on the VM early. If it
fails, the build script gains an explicit numeric-version step before bundling.

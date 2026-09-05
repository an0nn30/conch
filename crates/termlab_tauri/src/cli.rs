//! Dual-mode CLI entry point.
//!
//! `termlab` with no arguments boots the GUI as normal. `termlab <path...>`
//! forwards the paths to an already-running instance over the IPC socket (so
//! `termlab notes.md` from a terminal opens a new window in the running app
//! instead of launching a second process), falling back to booting a fresh
//! app instance carrying the paths as pending opens when no instance is
//! listening.
//!
//! When nothing is listening on the socket, the CLI does NOT become the app
//! itself: it re-spawns the executable detached (own session, null stdio) and
//! exits, so the shell prompt comes straight back and closing that terminal
//! can't SIGHUP a GUI full of unsaved buffers. Two env markers keep that
//! re-spawn (and Tauri's own restart re-exec) from looping back through the
//! CLI layer — see [`EnvMarkers`].
//!
//! Split into a pure decision layer (`evaluate`/`plan`) and an I/O layer
//! (`run_cli_if_requested`) so the argument-parsing, path-normalization, and
//! marker-precedence logic is unit-testable without touching sockets, the
//! process table, or the filesystem.

use std::path::{Component, Path, PathBuf};

/// Set on the detached re-spawn this module performs when no running
/// instance answered the socket. Tells that child "you ARE the app — boot
/// with argv's paths pending, don't try the socket, don't re-spawn again".
/// Without it the child would take the same not-connected branch its parent
/// did and fork forever.
pub const DETACHED_ENV: &str = "TERMLAB_CLI_DETACHED";

/// Set by the app itself once it is genuinely booting (see
/// `crate::mark_app_running`). Tauri's restart-required flow re-execs the
/// process with the ORIGINAL argv — which on this branch may carry paths —
/// and that re-exec inherits the environment. Seeing this marker means "this
/// is a restart, not a CLI invocation": ignore argv entirely and come back as
/// a plain app, rather than re-opening phantom files or forwarding them into
/// the parent that is in the middle of dying.
pub const APP_RUNNING_ENV: &str = "TERMLAB_APP_RUNNING";

/// Every marker [`EnvMarkers::from_env`] reads. These are private signalling
/// between the CLI layer and the app instance it launched, and must be
/// stripped from any process the app spawns on the user's behalf — above all
/// a PTY shell, which is precisely where someone types `termlab notes.md`.
/// Left inherited, that invocation would see `APP_RUNNING` and boot a second
/// blank app instead of forwarding to the instance hosting the terminal it
/// was typed into.
pub const INTERNAL_MARKER_VARS: [&str; 2] = [APP_RUNNING_ENV, DETACHED_ENV];

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

/// What the process should do, given its CLI arguments and current working
/// directory. Pure — no sockets, no filesystem access beyond the `cwd`
/// already handed in. I/O lives in `run_cli_if_requested`.
#[derive(Debug, PartialEq)]
pub enum CliDecision {
    RunApp,
    ForwardOrRun {
        paths: Vec<String>,
    },
    PrintHelp,
    PrintVersion,
    ReservedSubcommand,
    UnknownFlag(String),
    /// `--working-directory <DIR>`: boot the app with `DIR` as its process
    /// working directory. The directory is absolute and lexically cleaned but
    /// is NOT guaranteed to exist — the I/O layer decides what to do about
    /// that, so this stays a pure decision.
    RunAppInDirectory {
        directory: String,
    },
    /// Arguments parsed, but the combination is meaningless. The string is
    /// the human-readable reason; the I/O layer prints it and exits 2.
    UsageError(String),
}

/// The two env markers that override the plain argument reading, hoisted
/// off `std::env` into a value so [`plan`] stays a pure function.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct EnvMarkers {
    /// [`APP_RUNNING_ENV`] is set: we are Tauri's restart re-exec.
    pub app_running: bool,
    /// [`DETACHED_ENV`] is set: we are the detached re-spawn of a CLI
    /// invocation and are the instance that must actually boot.
    pub detached: bool,
}

impl EnvMarkers {
    /// Build from raw env values. Only the exact string `"1"` counts, so a
    /// stray empty or `0` value in an inherited environment is inert.
    pub fn from_values(app_running: Option<&str>, detached: Option<&str>) -> Self {
        Self {
            app_running: app_running == Some("1"),
            detached: detached == Some("1"),
        }
    }

    fn from_env() -> Self {
        let app_running = std::env::var(APP_RUNNING_ENV).ok();
        let detached = std::env::var(DETACHED_ENV).ok();
        Self::from_values(app_running.as_deref(), detached.as_deref())
    }
}

/// The argument decision with the env markers folded in — what
/// `run_cli_if_requested` will actually carry out. Pure counterpart of
/// [`CliAction`]: the only thing left after this is the I/O.
#[derive(Debug, PartialEq)]
pub enum CliPlan {
    /// Boot the app in this process, seeding these paths as pending opens.
    RunApp {
        pending_paths: Vec<String>,
    },
    /// Try to hand these paths to a running instance over the IPC socket;
    /// if nothing answers, re-spawn detached and exit.
    ForwardOrDetach {
        paths: Vec<String>,
    },
    PrintHelp,
    PrintVersion,
    UnknownFlag(String),
    /// Boot the app in this process with `directory` as its working
    /// directory. There is no forwarding counterpart: Windows has no IPC
    /// listener (`ipc.rs` is `#[cfg(unix)]` throughout), and the context menu
    /// this serves wants a fresh window either way.
    RunAppInDirectory {
        directory: String,
    },
    /// A usage error detected during parsing. Printed, then exit 2.
    UsageError(String),
}

/// Fold the env markers into [`evaluate`]'s decision. Pure.
///
/// Precedence is strict and load-bearing: `APP_RUNNING` beats `DETACHED`
/// beats the socket-forward path. A restart of a process that was itself a
/// detached CLI spawn inherits BOTH markers, and must come back as a plain
/// app rather than re-opening the file the user opened an hour ago.
pub fn plan(args: &[String], cwd: &Path, env: EnvMarkers) -> CliPlan {
    if env.app_running {
        // A restart: argv is a stale artifact of how the process was first
        // launched, not a request. Do not even parse it.
        return CliPlan::RunApp {
            pending_paths: vec![],
        };
    }

    match evaluate(args, cwd) {
        CliDecision::RunApp | CliDecision::ReservedSubcommand => CliPlan::RunApp {
            pending_paths: vec![],
        },
        CliDecision::PrintHelp => CliPlan::PrintHelp,
        CliDecision::PrintVersion => CliPlan::PrintVersion,
        CliDecision::UnknownFlag(f) => CliPlan::UnknownFlag(f),
        CliDecision::RunAppInDirectory { directory } => {
            // No forwarding counterpart on purpose: this is the Explorer
            // context-menu path, and Windows has no IPC listener to forward
            // to. `DETACHED` is therefore irrelevant here.
            CliPlan::RunAppInDirectory { directory }
        }
        CliDecision::UsageError(msg) => CliPlan::UsageError(msg),
        CliDecision::ForwardOrRun { paths } => {
            if env.detached {
                // We are the re-spawn. Boot; never loop back to the socket.
                CliPlan::RunApp {
                    pending_paths: paths,
                }
            } else {
                CliPlan::ForwardOrDetach { paths }
            }
        }
    }
}

/// What `run_cli_if_requested` decided the process should actually do, after
/// performing whatever I/O `evaluate`'s decision called for (e.g. trying to
/// forward paths over the IPC socket).
#[derive(Debug, PartialEq)]
pub enum CliAction {
    RunApp {
        pending_paths: Vec<String>,
    },
    /// Boot the app after moving the process into `directory`.
    RunAppInDirectory {
        directory: String,
    },
    Exit(i32),
}

/// Decide what to do with the process's CLI arguments. Pure function: given
/// the same `args` and `cwd` it always returns the same decision.
pub fn evaluate(args: &[String], cwd: &Path) -> CliDecision {
    if let Some(first) = args.first() {
        if first == "msg" {
            // Untouched namespace — `ipc.rs` / the existing `termlab msg
            // new-window` flow owns this, not path-opening.
            return CliDecision::ReservedSubcommand;
        }
    }

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
                    working_directory = Some(normalize_path(strip_trailing_quote(value), cwd));
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
            working_directory = Some(normalize_path(strip_trailing_quote(value), cwd));
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

/// Strip a single trailing `"` from a `--working-directory` value.
///
/// The Explorer verb template quotes `%V` (see
/// `packaging/windows/registration.wxs` and
/// `packaging/windows/installer-hooks.nsh`): `"<exe>" --working-directory
/// "%V"`. For a drive root (always for `Drive\shell`, and for
/// `Directory\Background\shell` at a drive root), `%V` expands to `C:\`, so
/// the quoted value is `"C:\"`. Windows command-line argv parsing treats a
/// single backslash immediately before a closing quote as escaping that
/// quote rather than ending the quoted section, so the backslash is
/// consumed and this process receives the value `C:"` — not `C:\`. Left
/// alone, that trailing quote becomes part of the path and the directory
/// change silently fails (see `main.rs`'s `set_current_dir` warning).
///
/// A double quote can never legally appear in a Windows path, so stripping
/// one trailing quote here is always safe, and makes this robust regardless
/// of exactly what Windows hands over — rather than patching the `.wxs`/
/// `.nsh` command strings, which are asserted byte-identical to each other
/// by `windows_registration_parity.rs`.
fn strip_trailing_quote(value: &str) -> &str {
    value.strip_suffix('"').unwrap_or(value)
}

/// Resolve `p` against `cwd` if it's relative, then lexically clean `.` and
/// `..` components. No `canonicalize` and no filesystem access — this must
/// work for paths that don't exist yet. `..` at the root stays at the root.
fn normalize_path(p: &str, cwd: &Path) -> String {
    let path = Path::new(p);
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    };

    let mut cleaned = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                // Pop the last real segment, but `..` at the root is inert
                // rather than escaping it or accumulating a dangling parent.
                match cleaned.components().next_back() {
                    Some(Component::RootDir) | None => {}
                    _ => {
                        cleaned.pop();
                    }
                }
            }
            other => cleaned.push(other.as_os_str()),
        }
    }

    cleaned.to_string_lossy().into_owned()
}

/// Entry point called from `main` before the Tauri app boots. Performs the
/// actual I/O (env args, cwd, and — for `ForwardOrRun` — the IPC socket
/// connection attempt) that `evaluate` deliberately stays free of.
pub fn run_cli_if_requested() -> CliAction {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
    match plan(&args, &cwd, EnvMarkers::from_env()) {
        CliPlan::RunApp { pending_paths } => CliAction::RunApp { pending_paths },
        CliPlan::PrintHelp => {
            println!("{HELP_TEXT}");
            CliAction::Exit(0)
        }
        CliPlan::PrintVersion => {
            println!("termlab {}", env!("CARGO_PKG_VERSION"));
            CliAction::Exit(0)
        }
        CliPlan::UnknownFlag(f) => {
            eprintln!("termlab: unknown flag '{f}'");
            CliAction::Exit(2)
        }
        CliPlan::RunAppInDirectory { directory } => CliAction::RunAppInDirectory { directory },
        CliPlan::UsageError(msg) => {
            eprintln!("termlab: {msg}");
            CliAction::Exit(2)
        }
        CliPlan::ForwardOrDetach { paths } => forward_or_detach(paths),
    }
}

/// Try to forward `paths` to an already-running instance over the IPC
/// socket. On success the running instance opens the windows and this
/// process exits.
///
/// On failure (nothing listening) this process does NOT become the app:
/// `/usr/local/bin/termlab notes.md` is invoked from a shell, and a GUI that
/// runs as a foreground child of that shell wedges the terminal until the app
/// quits — and takes a SIGHUP, unsaved buffers and all, if the terminal is
/// closed first. Instead the executable is re-spawned detached (own session,
/// null stdio, [`DETACHED_ENV`] set so the child boots directly instead of
/// coming back through here) and this process exits 0 immediately.
///
/// Only the re-spawn *itself* failing falls back to booting in-process —
/// better a wedged terminal than losing the user's request entirely.
#[cfg(unix)]
fn forward_or_detach(paths: Vec<String>) -> CliAction {
    use std::io::Write;
    use std::os::unix::net::UnixStream;

    match UnixStream::connect(crate::ipc::ipc_socket_path()) {
        Ok(mut stream) => {
            for path in &paths {
                let msg = serde_json::json!({"type": "open_path", "path": path});
                if let Err(e) = writeln!(stream, "{msg}") {
                    log::error!("termlab: failed to forward path over IPC socket: {e}");
                    return CliAction::RunApp {
                        pending_paths: paths,
                    };
                }
            }
            let _ = stream.flush();
            CliAction::Exit(0)
        }
        Err(_) => match spawn_detached(&paths) {
            Ok(()) => CliAction::Exit(0),
            Err(e) => {
                log::error!("termlab: could not launch a detached instance: {e}");
                CliAction::RunApp {
                    pending_paths: paths,
                }
            }
        },
    }
}

/// Re-launch this executable as a detached background process carrying
/// `paths` (already normalized to absolute, so the child's cwd is
/// irrelevant) and the [`DETACHED_ENV`] marker.
#[cfg(unix)]
fn spawn_detached(paths: &[String]) -> std::io::Result<()> {
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};

    let exe = std::env::current_exe()?.canonicalize()?;
    let mut cmd = Command::new(exe);
    cmd.args(paths)
        .env(DETACHED_ENV, "1")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    // SAFETY: `pre_exec` runs in the forked child between fork and exec,
    // where only async-signal-safe calls are legal. `setsid(2)` is one, and
    // it is the only thing done here. It puts the child in a new session
    // with no controlling terminal, which is the entire point: the app must
    // outlive the shell that launched it rather than being SIGHUPped when
    // that terminal closes. A failing setsid (only possible if we are
    // somehow already a session leader) is not worth aborting the launch
    // over, so its result is deliberately ignored.
    unsafe {
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }

    cmd.spawn()?;
    Ok(())
}

#[cfg(not(unix))]
fn forward_or_detach(paths: Vec<String>) -> CliAction {
    CliAction::RunApp {
        pending_paths: paths,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    /// Render a unix-style path the way `PathBuf` does on this platform.
    ///
    /// `evaluate` normalizes through `PathBuf`, so its output carries native
    /// separators — `\home\dustin\a.txt` on Windows. These assertions stay
    /// readable as unix paths and get converted, rather than being duplicated
    /// per platform.
    fn native(path: &str) -> String {
        path.replace('/', std::path::MAIN_SEPARATOR_STR)
    }

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
            CliDecision::ForwardOrRun {
                paths: vec![native("/home/dustin/proj/notes.md")]
            }
        );
    }

    #[test]
    fn dot_segments_are_lexically_cleaned() {
        assert_eq!(
            eval(&["../other/./a.txt"]),
            CliDecision::ForwardOrRun {
                paths: vec![native("/home/dustin/other/a.txt")]
            }
        );
    }

    #[test]
    fn absolute_path_untouched() {
        assert_eq!(
            eval(&["/tmp/x.txt"]),
            CliDecision::ForwardOrRun {
                paths: vec![native("/tmp/x.txt")]
            }
        );
    }

    #[test]
    fn multiple_paths_keep_order() {
        let CliDecision::ForwardOrRun { paths } = eval(&["a.txt", "/b.txt"]) else {
            panic!()
        };
        assert_eq!(
            paths,
            vec![native("/home/dustin/proj/a.txt"), native("/b.txt")]
        );
    }

    #[test]
    fn help_version_msg_and_unknown_flags() {
        assert_eq!(eval(&["--help"]), CliDecision::PrintHelp);
        assert_eq!(eval(&["-h"]), CliDecision::PrintHelp);
        assert_eq!(eval(&["--version"]), CliDecision::PrintVersion);
        assert_eq!(eval(&["-V"]), CliDecision::PrintVersion);
        assert_eq!(
            eval(&["msg", "new-window"]),
            CliDecision::ReservedSubcommand
        );
        assert_eq!(
            eval(&["--reuse-window", "a.txt"]),
            CliDecision::UnknownFlag("--reuse-window".into())
        );
    }

    #[test]
    fn dot_dot_at_root_stays_at_root() {
        assert_eq!(
            eval(&["/../etc/passwd"]),
            CliDecision::ForwardOrRun {
                paths: vec![native("/etc/passwd")]
            }
        );
    }

    #[test]
    fn launch_services_process_serial_number_flag_is_ignored() {
        // macOS LaunchServices historically appends `-psn_0_NNNNN` when the
        // app is launched from Finder/Dock. Treating it as an unknown flag
        // would exit(2) before Tauri ever initializes, so a double-click
        // would just silently do nothing.
        assert_eq!(eval(&["-psn_0_123456"]), CliDecision::RunApp);
        assert_eq!(
            eval(&["-psn_0_123456", "notes.md"]),
            CliDecision::ForwardOrRun {
                paths: vec![native("/home/dustin/proj/notes.md")]
            }
        );
        // Real typos must still fail loudly.
        assert_eq!(eval(&["-psn"]), CliDecision::UnknownFlag("-psn".into()));
    }

    fn plan_for(args: &[&str], env: EnvMarkers) -> CliPlan {
        let owned: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        plan(&owned, Path::new("/home/dustin/proj"), env)
    }

    #[test]
    fn without_markers_paths_forward_or_detach() {
        assert_eq!(
            plan_for(&["a.txt"], EnvMarkers::default()),
            CliPlan::ForwardOrDetach {
                paths: vec![native("/home/dustin/proj/a.txt")]
            }
        );
        assert_eq!(
            plan_for(&[], EnvMarkers::default()),
            CliPlan::RunApp {
                pending_paths: vec![]
            }
        );
    }

    #[test]
    fn detached_marker_boots_directly_with_argv_paths_pending() {
        // The detached re-spawn must NOT try the socket again and must NOT
        // re-spawn itself — that would recurse forever.
        assert_eq!(
            plan_for(
                &["a.txt"],
                EnvMarkers {
                    app_running: false,
                    detached: true
                }
            ),
            CliPlan::RunApp {
                pending_paths: vec![native("/home/dustin/proj/a.txt")]
            }
        );
    }

    #[test]
    fn app_running_marker_ignores_argv_entirely() {
        // Tauri's restart re-execs with the ORIGINAL argv, which may still
        // carry paths. A restart must come back as a plain app.
        assert_eq!(
            plan_for(
                &["a.txt", "b.txt"],
                EnvMarkers {
                    app_running: true,
                    detached: false
                }
            ),
            CliPlan::RunApp {
                pending_paths: vec![]
            }
        );
    }

    #[test]
    fn marker_precedence_app_running_beats_detached_beats_forward() {
        let paths = ["a.txt"];
        // APP_RUNNING wins over DETACHED.
        assert_eq!(
            plan_for(
                &paths,
                EnvMarkers {
                    app_running: true,
                    detached: true
                }
            ),
            CliPlan::RunApp {
                pending_paths: vec![]
            }
        );
        // DETACHED wins over the socket-forward path.
        assert_eq!(
            plan_for(
                &paths,
                EnvMarkers {
                    app_running: false,
                    detached: true
                }
            ),
            CliPlan::RunApp {
                pending_paths: vec![native("/home/dustin/proj/a.txt")]
            }
        );
        // Neither marker: forward (or detach).
        assert_eq!(
            plan_for(
                &paths,
                EnvMarkers {
                    app_running: false,
                    detached: false
                }
            ),
            CliPlan::ForwardOrDetach {
                paths: vec![native("/home/dustin/proj/a.txt")]
            }
        );
    }

    #[test]
    fn app_running_marker_swallows_help_and_unknown_flags_too() {
        // "ignore argv entirely" is literal: a restart must never print help
        // and exit, whatever the original argv was.
        assert_eq!(
            plan_for(
                &["--help"],
                EnvMarkers {
                    app_running: true,
                    detached: false
                }
            ),
            CliPlan::RunApp {
                pending_paths: vec![]
            }
        );
        assert_eq!(
            plan_for(
                &["--bogus"],
                EnvMarkers {
                    app_running: true,
                    detached: false
                }
            ),
            CliPlan::RunApp {
                pending_paths: vec![]
            }
        );
    }

    #[test]
    fn help_and_version_still_short_circuit_without_markers() {
        assert_eq!(
            plan_for(&["--help"], EnvMarkers::default()),
            CliPlan::PrintHelp
        );
        assert_eq!(
            plan_for(&["-V"], EnvMarkers::default()),
            CliPlan::PrintVersion
        );
        assert_eq!(
            plan_for(&["--bogus"], EnvMarkers::default()),
            CliPlan::UnknownFlag("--bogus".into())
        );
        assert_eq!(
            plan_for(&["msg", "new-window"], EnvMarkers::default()),
            CliPlan::RunApp {
                pending_paths: vec![]
            }
        );
    }

    #[test]
    fn internal_markers_are_scrubbed_from_child_environments() {
        // Both markers live in the app's own environment for the whole
        // session (APP_RUNNING has to, so Tauri's restart re-exec inherits
        // it). Any process the app spawns for the user would inherit them
        // too — and a PTY shell is exactly where someone types `termlab
        // notes.md`. That invocation must forward to this running instance
        // like any other, not see APP_RUNNING and boot a second blank app.
        assert!(INTERNAL_MARKER_VARS.contains(&APP_RUNNING_ENV));
        assert!(INTERNAL_MARKER_VARS.contains(&DETACHED_ENV));
        assert_eq!(
            INTERNAL_MARKER_VARS.len(),
            2,
            "every marker `EnvMarkers::from_env` reads must also be scrubbed"
        );
    }

    #[test]
    fn env_markers_read_only_the_exact_marker_value() {
        assert_eq!(
            EnvMarkers::from_values(Some("1"), Some("1")),
            EnvMarkers {
                app_running: true,
                detached: true
            }
        );
        assert_eq!(EnvMarkers::from_values(None, None), EnvMarkers::default());
        // Anything other than "1" is not the marker.
        assert_eq!(
            EnvMarkers::from_values(Some("0"), Some("")),
            EnvMarkers::default()
        );
    }

    // --- --working-directory ------------------------------------------------

    #[test]
    fn working_directory_flag_is_parsed_as_a_launch_directory() {
        let args = vec![
            "--working-directory".to_string(),
            "/home/dustin/proj".to_string(),
        ];
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
            matches!(
                evaluate(&args, Path::new("/tmp")),
                CliDecision::UsageError(_)
            ),
            "--working-directory with no value should be a usage error, not a silent launch"
        );
    }

    #[test]
    fn empty_working_directory_value_is_a_usage_error() {
        let args = vec!["--working-directory=".to_string()];
        assert!(
            matches!(
                evaluate(&args, Path::new("/tmp")),
                CliDecision::UsageError(_)
            ),
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
            matches!(
                evaluate(&args, Path::new("/tmp")),
                CliDecision::UsageError(_)
            ),
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
        let args = vec![
            "--working-directory".to_string(),
            "/x".to_string(),
            "-V".to_string(),
        ];
        assert_eq!(
            evaluate(&args, Path::new("/tmp")),
            CliDecision::PrintVersion,
            "-V must keep precedence over --working-directory"
        );
    }

    #[test]
    fn working_directory_survives_the_detached_marker() {
        let args = vec![
            "--working-directory".to_string(),
            "/home/dustin".to_string(),
        ];
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
        let args = vec![
            "--working-directory".to_string(),
            "/home/dustin".to_string(),
        ];
        let env = EnvMarkers::from_values(Some("1"), None);
        assert_eq!(
            plan(&args, Path::new("/tmp"), env),
            CliPlan::RunApp {
                pending_paths: vec![]
            },
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

    // --- trailing-quote stripping (drive-root %V corruption) ----------------

    #[test]
    fn working_directory_strips_a_trailing_quote_left_by_a_corrupted_drive_root() {
        // Explorer's "Open TermLab here" verb on a drive (or a background
        // click at a drive root) expands %V to `C:\`, and Windows argv
        // parsing eats the backslash immediately before the closing quote,
        // so this process actually receives the value `C:"` rather than
        // `C:\`. The fix must strip that stray quote before normalizing;
        // compute the expected value the same way normalize_path would
        // (cwd.join) rather than hardcoding platform-specific separators.
        let cwd = Path::new("/tmp");
        let args = vec!["--working-directory".to_string(), "C:\"".to_string()];
        let expected = cwd.join("C:").to_string_lossy().into_owned();
        assert_eq!(
            evaluate(&args, cwd),
            CliDecision::RunAppInDirectory {
                directory: expected
            },
            "a trailing quote left by Windows argv parsing must be stripped, \
             not carried into the launch directory"
        );
    }

    #[test]
    fn working_directory_normal_path_has_no_quote_to_strip() {
        let args = vec![
            "--working-directory".to_string(),
            "/home/dustin/proj".to_string(),
        ];
        assert_eq!(
            evaluate(&args, Path::new("/tmp")),
            CliDecision::RunAppInDirectory {
                directory: native("/home/dustin/proj"),
            },
            "an ordinary path with no trailing quote must be unaffected"
        );
    }

    #[test]
    fn working_directory_trailing_backslash_without_a_quote_is_left_alone() {
        // Only a trailing double quote is stripped. A path that legitimately
        // ends in a backslash (a normal, uncorrupted Windows path, not one
        // that went through the Explorer-verb quoting bug) must come out
        // exactly as `normalize_path` alone would produce it -- the quote fix
        // must add nothing on top.
        //
        // The expected value goes through `normalize_path` itself rather than
        // a bare `cwd.join`: on Windows `Path::components()` drops a trailing
        // separator, so `C:\Users\dustin\` normalizes to `C:\Users\dustin`,
        // while `cwd.join` keeps the trailing `\`. A literal or join-based
        // expectation passes on macOS (where `\` is not a separator) and fails
        // on Windows -- which is what shipped and broke the Windows build.
        let cwd = Path::new("/tmp");
        let value = "C:\\Users\\dustin\\";
        let args = vec!["--working-directory".to_string(), value.to_string()];
        let expected = normalize_path(value, cwd);
        assert_eq!(
            evaluate(&args, cwd),
            CliDecision::RunAppInDirectory {
                directory: expected.clone()
            },
            "a trailing backslash with no quote must not be altered by the fix"
        );
        // Deliberately NOT asserted: that the trailing- and non-trailing-
        // backslash forms resolve identically. That is only true where `\`
        // is a path separator (Windows); on macOS/Linux it is an ordinary
        // character and the two are genuinely different paths. Asserting it
        // here would simply move the platform trap from Windows to macOS.
        assert!(
            !expected.contains('"'),
            "a normalized directory must never carry a quote character"
        );
    }
}

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

const HELP_TEXT: &str = "\
termlab [PATH ...]

Open PATH(s) in a TermLab window. With no arguments, launches TermLab.

  -h, --help       Print this help message
  -V, --version    Print the version
";

/// What the process should do, given its CLI arguments and current working
/// directory. Pure — no sockets, no filesystem access beyond the `cwd`
/// already handed in. I/O lives in `run_cli_if_requested`.
#[derive(Debug, PartialEq)]
pub enum CliDecision {
    RunApp,
    ForwardOrRun { paths: Vec<String> },
    PrintHelp,
    PrintVersion,
    ReservedSubcommand,
    UnknownFlag(String),
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
    RunApp { pending_paths: Vec<String> },
    /// Try to hand these paths to a running instance over the IPC socket;
    /// if nothing answers, re-spawn detached and exit.
    ForwardOrDetach { paths: Vec<String> },
    PrintHelp,
    PrintVersion,
    UnknownFlag(String),
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
        return CliPlan::RunApp { pending_paths: vec![] };
    }

    match evaluate(args, cwd) {
        CliDecision::RunApp | CliDecision::ReservedSubcommand => {
            CliPlan::RunApp { pending_paths: vec![] }
        }
        CliDecision::PrintHelp => CliPlan::PrintHelp,
        CliDecision::PrintVersion => CliPlan::PrintVersion,
        CliDecision::UnknownFlag(f) => CliPlan::UnknownFlag(f),
        CliDecision::ForwardOrRun { paths } => {
            if env.detached {
                // We are the re-spawn. Boot; never loop back to the socket.
                CliPlan::RunApp { pending_paths: paths }
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
    RunApp { pending_paths: Vec<String> },
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
    for arg in args {
        if arg == "--help" || arg == "-h" {
            return CliDecision::PrintHelp;
        }
        if arg == "--version" || arg == "-V" {
            return CliDecision::PrintVersion;
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

    if paths.is_empty() {
        CliDecision::RunApp
    } else {
        CliDecision::ForwardOrRun { paths }
    }
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
                    return CliAction::RunApp { pending_paths: paths };
                }
            }
            let _ = stream.flush();
            CliAction::Exit(0)
        }
        Err(_) => match spawn_detached(&paths) {
            Ok(()) => CliAction::Exit(0),
            Err(e) => {
                log::error!("termlab: could not launch a detached instance: {e}");
                CliAction::RunApp { pending_paths: paths }
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
    CliAction::RunApp { pending_paths: paths }
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

    #[test]
    fn dot_dot_at_root_stays_at_root() {
        assert_eq!(
            eval(&["/../etc/passwd"]),
            CliDecision::ForwardOrRun { paths: vec!["/etc/passwd".into()] }
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
            CliDecision::ForwardOrRun { paths: vec!["/home/dustin/proj/notes.md".into()] }
        );
        // Real typos must still fail loudly.
        assert_eq!(
            eval(&["-psn"]),
            CliDecision::UnknownFlag("-psn".into())
        );
    }

    fn plan_for(args: &[&str], env: EnvMarkers) -> CliPlan {
        let owned: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        plan(&owned, Path::new("/home/dustin/proj"), env)
    }

    #[test]
    fn without_markers_paths_forward_or_detach() {
        assert_eq!(
            plan_for(&["a.txt"], EnvMarkers::default()),
            CliPlan::ForwardOrDetach { paths: vec!["/home/dustin/proj/a.txt".into()] }
        );
        assert_eq!(
            plan_for(&[], EnvMarkers::default()),
            CliPlan::RunApp { pending_paths: vec![] }
        );
    }

    #[test]
    fn detached_marker_boots_directly_with_argv_paths_pending() {
        // The detached re-spawn must NOT try the socket again and must NOT
        // re-spawn itself — that would recurse forever.
        assert_eq!(
            plan_for(&["a.txt"], EnvMarkers { app_running: false, detached: true }),
            CliPlan::RunApp { pending_paths: vec!["/home/dustin/proj/a.txt".into()] }
        );
    }

    #[test]
    fn app_running_marker_ignores_argv_entirely() {
        // Tauri's restart re-execs with the ORIGINAL argv, which may still
        // carry paths. A restart must come back as a plain app.
        assert_eq!(
            plan_for(&["a.txt", "b.txt"], EnvMarkers { app_running: true, detached: false }),
            CliPlan::RunApp { pending_paths: vec![] }
        );
    }

    #[test]
    fn marker_precedence_app_running_beats_detached_beats_forward() {
        let paths = ["a.txt"];
        // APP_RUNNING wins over DETACHED.
        assert_eq!(
            plan_for(&paths, EnvMarkers { app_running: true, detached: true }),
            CliPlan::RunApp { pending_paths: vec![] }
        );
        // DETACHED wins over the socket-forward path.
        assert_eq!(
            plan_for(&paths, EnvMarkers { app_running: false, detached: true }),
            CliPlan::RunApp { pending_paths: vec!["/home/dustin/proj/a.txt".into()] }
        );
        // Neither marker: forward (or detach).
        assert_eq!(
            plan_for(&paths, EnvMarkers { app_running: false, detached: false }),
            CliPlan::ForwardOrDetach { paths: vec!["/home/dustin/proj/a.txt".into()] }
        );
    }

    #[test]
    fn app_running_marker_swallows_help_and_unknown_flags_too() {
        // "ignore argv entirely" is literal: a restart must never print help
        // and exit, whatever the original argv was.
        assert_eq!(
            plan_for(&["--help"], EnvMarkers { app_running: true, detached: false }),
            CliPlan::RunApp { pending_paths: vec![] }
        );
        assert_eq!(
            plan_for(&["--bogus"], EnvMarkers { app_running: true, detached: false }),
            CliPlan::RunApp { pending_paths: vec![] }
        );
    }

    #[test]
    fn help_and_version_still_short_circuit_without_markers() {
        assert_eq!(plan_for(&["--help"], EnvMarkers::default()), CliPlan::PrintHelp);
        assert_eq!(plan_for(&["-V"], EnvMarkers::default()), CliPlan::PrintVersion);
        assert_eq!(
            plan_for(&["--bogus"], EnvMarkers::default()),
            CliPlan::UnknownFlag("--bogus".into())
        );
        assert_eq!(
            plan_for(&["msg", "new-window"], EnvMarkers::default()),
            CliPlan::RunApp { pending_paths: vec![] }
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
            EnvMarkers { app_running: true, detached: true }
        );
        assert_eq!(EnvMarkers::from_values(None, None), EnvMarkers::default());
        // Anything other than "1" is not the marker.
        assert_eq!(EnvMarkers::from_values(Some("0"), Some("")), EnvMarkers::default());
    }
}

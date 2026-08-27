//! Dual-mode CLI entry point.
//!
//! `termlab` with no arguments boots the GUI as normal. `termlab <path...>`
//! forwards the paths to an already-running instance over the IPC socket (so
//! `termlab notes.md` from a terminal opens a new window in the running app
//! instead of launching a second process), falling back to booting a fresh
//! app instance carrying the paths as pending opens when no instance is
//! listening.
//!
//! Split into a pure decision layer (`evaluate`) and an I/O layer
//! (`run_cli_if_requested`) so the argument-parsing and path-normalization
//! logic is unit-testable without touching sockets or the filesystem.

use std::path::{Component, Path, PathBuf};

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
    match evaluate(&args, &cwd) {
        CliDecision::RunApp => CliAction::RunApp { pending_paths: vec![] },
        CliDecision::ReservedSubcommand => CliAction::RunApp { pending_paths: vec![] },
        CliDecision::PrintHelp => {
            println!("{HELP_TEXT}");
            CliAction::Exit(0)
        }
        CliDecision::PrintVersion => {
            println!("termlab {}", env!("CARGO_PKG_VERSION"));
            CliAction::Exit(0)
        }
        CliDecision::UnknownFlag(f) => {
            eprintln!("termlab: unknown flag '{f}'");
            CliAction::Exit(2)
        }
        CliDecision::ForwardOrRun { paths } => forward_or_run(paths),
    }
}

/// Try to forward `paths` to an already-running instance over the IPC
/// socket. On success the running instance opens the windows and this
/// process exits; on failure (nothing listening) the paths are handed back
/// so this process can boot the app itself and open them locally.
#[cfg(unix)]
fn forward_or_run(paths: Vec<String>) -> CliAction {
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
        Err(_) => CliAction::RunApp { pending_paths: paths },
    }
}

#[cfg(not(unix))]
fn forward_or_run(paths: Vec<String>) -> CliAction {
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
}

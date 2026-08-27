//! Install/uninstall the `termlab` CLI entry point at `/usr/local/bin/termlab`.
//!
//! Split the same way `cli.rs` is: a pure classification layer
//! (`classify_link`, `LinkState`, `admin_shell_command`) that is fully unit
//! tested, and thin Tauri commands (`install_cli_symlink`,
//! `uninstall_cli_symlink`) that do the actual filesystem/process I/O.
//!
//! HARD SAFETY CONSTRAINT: install/uninstall must never delete or overwrite a
//! file at the link path that isn't a TermLab symlink. `classify_link` is the
//! single place that decides this, and both commands route every existing
//! file through it before touching anything — in *program order* there is no
//! "remove first, classify later" shortcut anywhere below. This is not a
//! claim of atomicity: classify-then-remove has a TOCTOU window (something
//! else could replace the file between the classify and the `remove_file`
//! call), which is accepted here rather than guarded against — a concurrent
//! writer to `/usr/local/bin` is inside the user's own trust boundary, not
//! an adversary this module defends against.

use std::path::{Path, PathBuf};

/// Where the CLI entry point is installed. Fixed rather than configurable —
/// this is the one directory every shell's default `PATH` already includes
/// on macOS and Linux without editing `.zshrc`/`.bashrc`.
#[cfg(unix)]
const LINK_PATH: &str = "/usr/local/bin/termlab";

/// What already exists at the link path, relative to the running app's own
/// executable.
#[derive(Debug, PartialEq)]
pub(crate) enum LinkState {
    /// Nothing exists at the link path.
    Missing,
    /// The link already points at this exact running executable — nothing to
    /// do.
    PointsToUs,
    /// The link points at a *different* TermLab install (e.g. the app was
    /// moved, or another copy is installed elsewhere). Safe to relink: it's
    /// still ours, just stale.
    PointsToOtherTermLab(PathBuf),
    /// The path exists and doesn't look like any TermLab install. Never
    /// touched — install/uninstall both refuse and report this back to the
    /// user instead.
    Foreign(PathBuf),
}

/// What's already at the link path, as read directly off the filesystem —
/// exactly as much detail as `classify_link` needs, and no more. The
/// symlink/non-symlink distinction is carried explicitly (rather than
/// folded into "what path do we have") so a non-symlink file can never be
/// run through the name-based heuristic below: whether something happens to
/// be *named* `termlab` is never, by itself, evidence that it *is* ours.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum LinkProbe {
    /// Nothing exists at the link path.
    Missing,
    /// A symlink exists at the link path, resolving to this target.
    Symlink(PathBuf),
    /// Something exists at the link path but is NOT a symlink (a regular
    /// file, a directory, a hardlink, ...). Always `Foreign` in
    /// `classify_link` — never inspected further, regardless of its name.
    Other,
}

/// Classify the existing entry at the link path, without touching the
/// filesystem itself. `link_path` is used only to name the offending file
/// in the `Foreign` case for a non-symlink `probe` (there is no "target" to
/// report for those — the file itself is what's foreign).
///
/// A symlink's target classifies as `PointsToOtherTermLab` when it contains
/// a path component ending in `TermLab.app`, or its final component is
/// exactly `termlab` — and it isn't already `current_exe`. Any other
/// symlink target is `Foreign`. A non-symlink entry (`LinkProbe::Other`) is
/// unconditionally `Foreign`, regardless of what the link path is named.
pub(crate) fn classify_link(
    probe: &LinkProbe,
    current_exe: &Path,
    link_path: &Path,
) -> LinkState {
    let target = match probe {
        LinkProbe::Missing => return LinkState::Missing,
        LinkProbe::Other => return LinkState::Foreign(link_path.to_path_buf()),
        LinkProbe::Symlink(target) => target,
    };

    if target == current_exe {
        return LinkState::PointsToUs;
    }

    let looks_like_termlab = target.components().any(|c| {
        c.as_os_str()
            .to_str()
            .is_some_and(|s| s.ends_with("TermLab.app"))
    }) || target
        .file_name()
        .and_then(|f| f.to_str())
        .is_some_and(|f| f == "termlab");

    if looks_like_termlab {
        LinkState::PointsToOtherTermLab(target.to_path_buf())
    } else {
        LinkState::Foreign(target.to_path_buf())
    }
}

/// Should a failed `symlink()` at the link path be retried with elevated
/// privileges?
///
/// The naive answer is "only `PermissionDenied`", and that is wrong: the two
/// most common real-world failures against a root-owned `/usr/local/bin`
/// surface as a *different* errno.
///
/// - `PermissionDenied` (EACCES): the directory isn't writable by this user.
/// - `AlreadyExists` (EEXIST): a stale root-owned TermLab symlink is sitting
///   there. `install()` classified it as safe to replace and called
///   `remove_file`, which failed EACCES and was deliberately discarded (it's
///   best-effort), so `symlink()` then failed EEXIST. Escalated `ln -sf`
///   replaces it in one step.
/// - `NotFound` (ENOENT): `/usr/local/bin` doesn't exist at all — common on
///   a clean macOS install that has never had Homebrew. The escalated
///   command creates it first (see [`install_escalation_command`]).
///
/// Everything else is a genuine error the user should see, not something an
/// admin password fixes.
#[cfg(unix)]
pub(crate) fn should_escalate(kind: std::io::ErrorKind) -> bool {
    matches!(
        kind,
        std::io::ErrorKind::PermissionDenied
            | std::io::ErrorKind::AlreadyExists
            | std::io::ErrorKind::NotFound
    )
}

/// The same question for the uninstall path's `remove_file`, where the
/// answer really is "only `PermissionDenied`".
///
/// `remove_file` cannot fail `AlreadyExists`, and its `NotFound` is
/// uninstall's *success* condition, not a failure: it means the link is
/// already gone (something removed it in the window between `classify_link`
/// and here). Escalating there would pop an admin prompt to delete a file
/// that does not exist. `unlink` handles it as success instead.
#[cfg(unix)]
pub(crate) fn should_escalate_unlink(kind: std::io::ErrorKind) -> bool {
    matches!(kind, std::io::ErrorKind::PermissionDenied)
}

/// Single-quote `path` for use as one argument in a shell command line,
/// escaping any embedded single quotes with the standard
/// close-quote/escaped-quote/reopen-quote trick.
#[cfg(unix)]
pub(crate) fn shell_quote(path: &Path) -> String {
    let raw = path.to_string_lossy();
    format!("'{}'", raw.replace('\'', "'\\''"))
}

/// The privileged shell command that repairs the install, given why the
/// unprivileged `symlink()` failed.
///
/// `ln -sf` covers both `PermissionDenied` and `AlreadyExists`: `-f` removes
/// an existing entry at the destination, and running as root makes the
/// directory writable. `NotFound` additionally needs the parent directory
/// created, since `ln` alone would fail ENOENT again even as root.
#[cfg(unix)]
pub(crate) fn install_escalation_command(
    kind: std::io::ErrorKind,
    exe: &Path,
    link_path: &Path,
) -> String {
    let link = format!("ln -sf {} {}", shell_quote(exe), shell_quote(link_path));
    match (kind, link_path.parent()) {
        (std::io::ErrorKind::NotFound, Some(parent)) => {
            format!("mkdir -p {} && {link}", shell_quote(parent))
        }
        _ => link,
    }
}

/// Escape `action` (a fully-formed shell command line, already
/// single-quoted per-argument by the caller) for use as the string literal
/// inside an AppleScript `do shell script "..." with administrator
/// privileges` command, and return that AppleScript source.
///
/// Two quoting layers are in play: `action`'s own shell quoting (the
/// caller's responsibility — see `shell_quote` below) and AppleScript's
/// double-quoted string literal around the whole thing (this function's
/// responsibility). AppleScript string literals treat `\` and `"` specially,
/// so both are escaped here; nothing else needs it since `action` uses only
/// single quotes for its own shell-level quoting.
#[cfg(target_os = "macos")]
pub(crate) fn admin_shell_command(action: &str) -> String {
    let escaped = action.replace('\\', "\\\\").replace('"', "\\\"");
    format!("do shell script \"{escaped}\" with administrator privileges")
}

#[cfg(unix)]
mod unix_impl {
    use super::*;

    /// Read what's at `link_path` into a `LinkProbe`. `fs::read_link` fails
    /// both when nothing exists AND when something exists but isn't a
    /// symlink, so those two are told apart by error kind: `NotFound` means
    /// nothing's there, any other error (`InvalidInput` for a non-symlink
    /// file, on most platforms) means something's there that isn't a
    /// symlink — `LinkProbe::Other`, never inspected for its name.
    fn read_link_target(link_path: &Path) -> LinkProbe {
        match std::fs::read_link(link_path) {
            Ok(target) => LinkProbe::Symlink(target),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => LinkProbe::Missing,
            Err(_) => LinkProbe::Other,
        }
    }

    fn current_exe() -> Result<PathBuf, String> {
        std::env::current_exe()
            .and_then(|p| p.canonicalize())
            .map_err(|e| format!("Could not determine the running executable's path: {e}"))
    }

    #[cfg(target_os = "macos")]
    fn run_admin_shell(action: &str) -> Result<(), String> {
        let payload = admin_shell_command(action);
        let output = std::process::Command::new("osascript")
            .arg("-e")
            .arg(&payload)
            .output()
            .map_err(|e| format!("Failed to launch osascript: {e}"))?;
        if output.status.success() {
            Ok(())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(format!("Administrator command failed: {}", stderr.trim()))
        }
    }

    #[cfg(target_os = "macos")]
    fn escalate(action_desc: &str, action: String) -> Result<(), String> {
        run_admin_shell(&action).map_err(|e| format!("{action_desc}: {e}"))
    }

    /// Non-macOS unix (Linux, BSD, ...): there's no OS-native GUI privilege
    /// escalation prompt to shell out to, so hand the user the exact command
    /// to run themselves instead of silently failing.
    #[cfg(not(target_os = "macos"))]
    fn escalate(_action_desc: &str, action: String) -> Result<(), String> {
        Err(format!(
            "Permission denied. Run this yourself to finish the operation:\n  sudo {action}"
        ))
    }

    pub(crate) fn install() -> Result<String, String> {
        let exe = current_exe()?;
        let link_path = Path::new(LINK_PATH);
        let probe = read_link_target(link_path);

        match classify_link(&probe, &exe, link_path) {
            LinkState::PointsToUs => {
                Ok(format!("'termlab' is already installed at {LINK_PATH}"))
            }
            LinkState::Foreign(p) => Err(format!(
                "{LINK_PATH} already exists and isn't a TermLab install ({}); refusing to overwrite it",
                p.display()
            )),
            LinkState::Missing => link(&exe, link_path),
            LinkState::PointsToOtherTermLab(_) => {
                // Stale TermLab symlink from another install location —
                // known-safe to remove because it just passed classify_link.
                let _ = std::fs::remove_file(link_path);
                link(&exe, link_path)
            }
        }
    }

    fn link(exe: &Path, link_path: &Path) -> Result<String, String> {
        match std::os::unix::fs::symlink(exe, link_path) {
            Ok(()) => Ok(format!("Installed 'termlab' to {}", link_path.display())),
            Err(e) if should_escalate(e.kind()) => {
                let action = install_escalation_command(e.kind(), exe, link_path);
                escalate("Installing 'termlab'", action)?;
                Ok(format!(
                    "Installed 'termlab' to {} (with administrator privileges)",
                    link_path.display()
                ))
            }
            Err(e) => Err(format!(
                "Failed to create symlink at {}: {e}",
                link_path.display()
            )),
        }
    }

    pub(crate) fn uninstall() -> Result<String, String> {
        let exe = current_exe()?;
        let link_path = Path::new(LINK_PATH);
        let probe = read_link_target(link_path);

        match classify_link(&probe, &exe, link_path) {
            LinkState::Missing => Ok(format!("'termlab' was not installed at {LINK_PATH}")),
            LinkState::Foreign(p) => Err(format!(
                "{LINK_PATH} isn't a TermLab install ({}); refusing to remove it",
                p.display()
            )),
            LinkState::PointsToUs | LinkState::PointsToOtherTermLab(_) => unlink(link_path),
        }
    }

    fn unlink(link_path: &Path) -> Result<String, String> {
        match std::fs::remove_file(link_path) {
            Ok(()) => Ok(format!("Removed 'termlab' from {}", link_path.display())),
            // Already gone — something removed it between classify_link and
            // here. That is the outcome uninstall wanted, so report success
            // rather than escalating (see `should_escalate_unlink`).
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                Ok(format!("'termlab' was not installed at {}", link_path.display()))
            }
            Err(e) if should_escalate_unlink(e.kind()) => {
                let action = format!("rm -f {}", shell_quote(link_path));
                escalate("Uninstalling 'termlab'", action)?;
                Ok(format!(
                    "Removed 'termlab' from {} (with administrator privileges)",
                    link_path.display()
                ))
            }
            Err(e) => Err(format!(
                "Failed to remove {}: {e}",
                link_path.display()
            )),
        }
    }
}

/// Install the `termlab` CLI entry point by symlinking it into
/// `/usr/local/bin/termlab`. Refuses to touch anything at that path that
/// isn't already a TermLab symlink (see `classify_link`).
#[cfg(unix)]
#[tauri::command]
pub(crate) fn install_cli_symlink() -> Result<String, String> {
    unix_impl::install()
}

/// Uninstall the `termlab` CLI entry point, removing
/// `/usr/local/bin/termlab` only if it's a TermLab symlink (see
/// `classify_link`).
#[cfg(unix)]
#[tauri::command]
pub(crate) fn uninstall_cli_symlink() -> Result<String, String> {
    unix_impl::uninstall()
}

#[cfg(not(unix))]
#[tauri::command]
pub(crate) fn install_cli_symlink() -> Result<String, String> {
    Err("Installing the 'termlab' command is not supported on this platform".to_string())
}

#[cfg(not(unix))]
#[tauri::command]
pub(crate) fn uninstall_cli_symlink() -> Result<String, String> {
    Err("Uninstalling the 'termlab' command is not supported on this platform".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_missing_ours_stale_and_foreign() {
        let exe = Path::new("/Applications/TermLab.app/Contents/MacOS/termlab");
        let link_path = Path::new("/usr/local/bin/termlab");
        assert!(matches!(
            classify_link(&LinkProbe::Missing, exe, link_path),
            LinkState::Missing
        ));
        assert!(matches!(
            classify_link(&LinkProbe::Symlink(exe.to_path_buf()), exe, link_path),
            LinkState::PointsToUs
        ));
        let stale = PathBuf::from("/Applications/Old/TermLab.app/Contents/MacOS/termlab");
        assert!(matches!(
            classify_link(&LinkProbe::Symlink(stale), exe, link_path),
            LinkState::PointsToOtherTermLab(_)
        ));
        let foreign = PathBuf::from("/usr/local/bin/some-other-tool-target");
        assert!(matches!(
            classify_link(&LinkProbe::Symlink(foreign), exe, link_path),
            LinkState::Foreign(_)
        ));
    }

    #[test]
    fn classify_non_symlink_file_named_termlab_must_not_be_mistaken_for_ours() {
        // Regression: a plain (non-symlink) file at the link path must
        // always be Foreign, regardless of its name. Before this was fixed,
        // read_link_target's non-symlink fallback handed classify_link the
        // link path itself as if it were a symlink's "target", and since
        // LINK_PATH's basename is literally "termlab", the name-based
        // heuristic matched it and misclassified ANY plain file sitting at
        // the link path as PointsToOtherTermLab -- which install()/
        // uninstall() would then delete. LinkProbe::Other now bypasses the
        // name-based heuristic entirely, structurally, so this can't
        // regress silently.
        let exe = Path::new("/Applications/TermLab.app/Contents/MacOS/termlab");
        let link_path = Path::new("/usr/local/bin/termlab");
        let state = classify_link(&LinkProbe::Other, exe, link_path);
        assert!(
            matches!(state, LinkState::Foreign(_)),
            "a non-symlink file at the link path must classify Foreign regardless \
             of its name, got {state:?}"
        );
    }

    #[test]
    fn classify_final_component_termlab_without_dot_app_is_also_ours() {
        // The rule is "final component exactly `termlab`" OR "a component
        // ending in `TermLab.app`" — a bare `termlab` binary dropped
        // somewhere outside a `.app` bundle (e.g. a dev build) still counts,
        // as long as it's an actual symlink target (not the non-symlink
        // case covered above).
        let exe = Path::new("/Applications/TermLab.app/Contents/MacOS/termlab");
        let link_path = Path::new("/usr/local/bin/termlab");
        let dev_build = PathBuf::from("/Users/me/conch/target/debug/termlab");
        assert!(matches!(
            classify_link(&LinkProbe::Symlink(dev_build), exe, link_path),
            LinkState::PointsToOtherTermLab(_)
        ));
    }

    #[test]
    fn classify_similarly_named_foreign_binary_is_not_mistaken_for_ours() {
        let exe = Path::new("/Applications/TermLab.app/Contents/MacOS/termlab");
        let link_path = Path::new("/usr/local/bin/termlab");
        // "termlab-old" is not an exact match on the final component, and
        // doesn't contain a `TermLab.app` component either.
        let similar = PathBuf::from("/usr/local/bin/termlab-old");
        assert!(matches!(
            classify_link(&LinkProbe::Symlink(similar), exe, link_path),
            LinkState::Foreign(_)
        ));
    }

    #[cfg(unix)]
    #[test]
    fn escalate_table_for_symlink_creation() {
        use std::io::ErrorKind::*;
        // The three failures that actually happen against a root-owned
        // /usr/local/bin, and which `ln -sf` (as root) fixes:
        //   PermissionDenied — the directory isn't writable by this user
        //   AlreadyExists    — a root-owned stale link is there; our
        //                      best-effort remove_file failed EACCES and was
        //                      discarded, so symlink() then failed EEXIST
        //   NotFound         — /usr/local/bin doesn't exist at all
        for kind in [PermissionDenied, AlreadyExists, NotFound] {
            assert!(
                should_escalate(kind),
                "{kind:?} must escalate — it's a permission problem wearing a \
                 different errno"
            );
        }
        // Anything else is a real error to report, not something a prompt
        // for the admin password would fix.
        for kind in [InvalidInput, Interrupted, Unsupported, InvalidData] {
            assert!(!should_escalate(kind), "{kind:?} must not escalate");
        }
    }

    #[cfg(unix)]
    #[test]
    fn escalate_table_for_unlink_is_narrower() {
        use std::io::ErrorKind::*;
        assert!(should_escalate_unlink(PermissionDenied));
        // `rm` has nothing to do if the file is already gone: NotFound is
        // uninstall's *success* condition (something removed the link
        // between classify_link and remove_file), so prompting for an admin
        // password to remove a file that no longer exists would be absurd.
        assert!(!should_escalate_unlink(NotFound));
        assert!(!should_escalate_unlink(AlreadyExists));
        assert!(!should_escalate_unlink(InvalidInput));
    }

    #[cfg(unix)]
    #[test]
    fn install_escalation_command_creates_the_directory_only_when_missing() {
        use std::io::ErrorKind::*;
        let exe = Path::new("/Applications/TermLab.app/Contents/MacOS/termlab");
        let link = Path::new("/usr/local/bin/termlab");

        let denied = install_escalation_command(PermissionDenied, exe, link);
        assert_eq!(
            denied,
            "ln -sf '/Applications/TermLab.app/Contents/MacOS/termlab' '/usr/local/bin/termlab'"
        );
        // `-f` is what makes the AlreadyExists case work at all.
        assert_eq!(install_escalation_command(AlreadyExists, exe, link), denied);

        // NotFound means the parent directory itself is absent, so `ln`
        // alone would fail again even as root.
        assert_eq!(
            install_escalation_command(NotFound, exe, link),
            "mkdir -p '/usr/local/bin' && ln -sf \
             '/Applications/TermLab.app/Contents/MacOS/termlab' '/usr/local/bin/termlab'"
        );
    }

    #[cfg(unix)]
    #[test]
    fn shell_quote_escapes_embedded_single_quotes() {
        assert_eq!(shell_quote(Path::new("/Apps/it's here/termlab")), "'/Apps/it'\\''s here/termlab'");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn admin_command_escapes_quotes() {
        let cmd = admin_shell_command(
            "ln -sf '/Apps/Term \"Lab\".app/x' '/usr/local/bin/termlab'",
        );
        // The osascript payload must survive both quoting layers: the inner
        // shell string and the AppleScript string literal around it.
        assert!(cmd.contains("with administrator privileges"));
        assert!(
            !cmd.contains("\"Lab\""),
            "raw inner double-quotes must be escaped for AppleScript"
        );
        // And the escaped form must actually be present and well-formed.
        assert!(cmd.contains("\\\"Lab\\\""));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn admin_command_escapes_backslashes_before_quotes() {
        // Escaping order matters: if `"` were escaped before `\`, the
        // backslash just inserted in front of a `"` would itself get
        // re-escaped, doubling up incorrectly.
        let cmd = admin_shell_command(r#"echo "a\b""#);
        assert_eq!(
            cmd,
            "do shell script \"echo \\\"a\\\\b\\\"\" with administrator privileges"
        );
    }
}

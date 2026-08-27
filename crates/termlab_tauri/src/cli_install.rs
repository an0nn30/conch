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
//! file through it before touching anything — there is no "remove first,
//! classify later" shortcut anywhere below.

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

/// Classify the existing entry at the link path, without touching the
/// filesystem itself.
///
/// `link_meta` is:
/// - `None` when nothing exists at the link path.
/// - `Some(target)` when something exists — `target` is the symlink's
///   resolved target, or (for a non-symlink file already sitting there) the
///   link path itself, which guarantees it can never accidentally equal
///   `current_exe` and always classifies as `Foreign`.
///
/// A target classifies as `PointsToOtherTermLab` when it contains a path
/// component ending in `TermLab.app`, or its final component is exactly
/// `termlab` — and it isn't already `current_exe`. Anything else is
/// `Foreign`.
pub(crate) fn classify_link(link_meta: Option<&Path>, current_exe: &Path) -> LinkState {
    let Some(target) = link_meta else {
        return LinkState::Missing;
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

    /// Read what's at `link_path`, normalized so every case classify_link
    /// needs is representable:
    /// - doesn't exist -> `None`
    /// - a symlink -> `Some(target)`
    /// - anything else (a regular file, a directory, ...) -> `Some(link_path
    ///   itself)`, which `classify_link` always reads as `Foreign` since it
    ///   can never equal `current_exe`.
    fn read_link_target(link_path: &Path) -> Option<PathBuf> {
        match std::fs::read_link(link_path) {
            Ok(target) => Some(target),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(_) => Some(link_path.to_path_buf()),
        }
    }

    /// Single-quote `path` for use as one argument in a shell command line,
    /// escaping any embedded single quotes with the standard
    /// close-quote/escaped-quote/reopen-quote trick.
    fn shell_quote(path: &Path) -> String {
        let raw = path.to_string_lossy();
        format!("'{}'", raw.replace('\'', "'\\''"))
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
        let existing = read_link_target(link_path);

        match classify_link(existing.as_deref(), &exe) {
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
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                let action = format!("ln -sf {} {}", shell_quote(exe), shell_quote(link_path));
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
        let existing = read_link_target(link_path);

        match classify_link(existing.as_deref(), &exe) {
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
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
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
        assert!(matches!(classify_link(None, exe), LinkState::Missing));
        assert!(matches!(
            classify_link(Some(exe), exe),
            LinkState::PointsToUs
        ));
        let stale = Path::new("/Applications/Old/TermLab.app/Contents/MacOS/termlab");
        assert!(matches!(
            classify_link(Some(stale), exe),
            LinkState::PointsToOtherTermLab(_)
        ));
        let foreign = Path::new("/usr/local/bin/some-other-tool-target");
        assert!(matches!(
            classify_link(Some(foreign), exe),
            LinkState::Foreign(_)
        ));
    }

    #[test]
    fn classify_final_component_termlab_without_dot_app_is_also_ours() {
        // The rule is "final component exactly `termlab`" OR "a component
        // ending in `TermLab.app`" — a bare `termlab` binary dropped
        // somewhere outside a `.app` bundle (e.g. a dev build) still counts.
        let exe = Path::new("/Applications/TermLab.app/Contents/MacOS/termlab");
        let dev_build = Path::new("/Users/me/conch/target/debug/termlab");
        assert!(matches!(
            classify_link(Some(dev_build), exe),
            LinkState::PointsToOtherTermLab(_)
        ));
    }

    #[test]
    fn classify_similarly_named_foreign_binary_is_not_mistaken_for_ours() {
        let exe = Path::new("/Applications/TermLab.app/Contents/MacOS/termlab");
        // "termlab-old" is not an exact match on the final component, and
        // doesn't contain a `TermLab.app` component either.
        let similar = Path::new("/usr/local/bin/termlab-old");
        assert!(matches!(
            classify_link(Some(similar), exe),
            LinkState::Foreign(_)
        ));
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

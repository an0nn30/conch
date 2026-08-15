//! Platform-specific environment initialisation.
//!
//! When launched from a desktop environment (macOS Finder, Linux desktop entry,
//! Windows Start Menu) the process inherits a minimal environment that may lack
//! variables like `LANG`, `SSH_AUTH_SOCK`, or a complete `PATH`.
//!
//! Must be called early in `main()`, before any child processes are spawned.

/// Perform platform-specific environment setup.
pub fn init() {
    #[cfg(target_os = "macos")]
    macos_init();

    #[cfg(target_os = "linux")]
    linux_init();

    // Windows inherits the full system environment by default.
}

#[cfg(target_os = "macos")]
fn macos_init() {
    set_ssh_auth_sock();
    set_locale();
    fix_path();
}

#[cfg(target_os = "linux")]
fn linux_init() {
    // Linux desktop sessions typically inherit a full environment.
    // Ensure LANG is set for Unicode support.
    if std::env::var("LANG").is_err() {
        unsafe { std::env::set_var("LANG", "en_US.UTF-8") };
    }
}

/// Run an environment-probe command, giving up after `timeout`.
///
/// These probes run on the main thread before the window exists, so a probe
/// that never returns is an invisible launch hang. `Command::output()` has no
/// deadline and waits for stdout to reach EOF — which never happens if the
/// child leaves a grandchild holding the pipe — so it is not safe here.
///
/// Returns the trimmed stdout, or `None` if the command failed, could not be
/// spawned, or outlived the deadline. Every caller must be able to carry on
/// without the value.
#[cfg(target_os = "macos")]
fn run_with_timeout(
    program: &str,
    args: &[&str],
    timeout: std::time::Duration,
) -> Option<String> {
    use std::process::{Command, Stdio};
    use std::sync::mpsc;

    let child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| log::warn!("env probe: failed to spawn {program}: {e}"))
        .ok()?;

    let pid = child.id();
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("env-probe".into())
        .spawn(move || {
            let _ = tx.send(child.wait_with_output());
        })
        .ok()?;

    match rx.recv_timeout(timeout) {
        Ok(Ok(out)) if out.status.success() => {
            Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
        }
        Ok(Ok(out)) => {
            log::warn!("env probe: {program} exited with {}", out.status);
            None
        }
        Ok(Err(e)) => {
            log::warn!("env probe: {program} failed: {e}");
            None
        }
        Err(_) => {
            // Kill the child so it can't linger. The reader thread may stay
            // parked if a grandchild still holds the pipe, but startup is
            // free to continue without it.
            log::warn!("env probe: {program} timed out after {timeout:?}; continuing without it");
            unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) };
            None
        }
    }
}

/// Discover `SSH_AUTH_SOCK` from the launchd environment if not already set.
#[cfg(target_os = "macos")]
fn set_ssh_auth_sock() {
    if std::env::var("SSH_AUTH_SOCK").is_ok() {
        return;
    }

    let path = run_with_timeout(
        "launchctl",
        &["getenv", "SSH_AUTH_SOCK"],
        std::time::Duration::from_secs(2),
    );

    match path {
        Some(path) if !path.is_empty() => {
            log::debug!("Discovered SSH_AUTH_SOCK from launchd: {path}");
            unsafe { std::env::set_var("SSH_AUTH_SOCK", &path) };
        }
        _ => {
            log::debug!("SSH_AUTH_SOCK not available from launchd");
        }
    }
}

/// Set locale via NSLocale when the environment doesn't provide one.
///
/// When launched from Finder, LANG/LC_ALL are typically unset, defaulting to
/// the "C" locale which breaks Unicode rendering in child processes (shells,
/// CLI tools). This queries the system locale and sets LC_ALL accordingly.
#[cfg(target_os = "macos")]
fn set_locale() {
    use std::ffi::{CStr, CString};

    let Ok(env_locale_c) = CString::new("") else {
        return;
    };
    let env_locale_ptr = unsafe { libc::setlocale(libc::LC_ALL, env_locale_c.as_ptr()) };
    if !env_locale_ptr.is_null() {
        let env_locale = unsafe { CStr::from_ptr(env_locale_ptr).to_string_lossy() };
        if env_locale != "C" {
            log::debug!("Using environment locale: {}", env_locale);
            return;
        }
    }

    // Query system locale via NSLocale.
    let system_locale = macos_system_locale();
    let system_locale_c = CString::new(system_locale.clone()).unwrap_or_default();
    let lc_all = unsafe { libc::setlocale(libc::LC_ALL, system_locale_c.as_ptr()) };

    if lc_all.is_null() {
        log::debug!("Using fallback locale: UTF-8");
        let Ok(fallback) = CString::new("UTF-8") else {
            return;
        };
        unsafe { libc::setlocale(libc::LC_CTYPE, fallback.as_ptr()) };
        unsafe { std::env::set_var("LC_CTYPE", "UTF-8") };
    } else {
        log::debug!("Using system locale: {}", system_locale);
        unsafe { std::env::set_var("LC_ALL", &system_locale) };
    }
}

/// Query the system locale from NSLocale (macOS).
#[cfg(target_os = "macos")]
fn macos_system_locale() -> String {
    use objc2::sel;
    use objc2_foundation::{NSLocale, NSObjectProtocol};

    let locale = NSLocale::currentLocale();

    let has_language = locale.respondsToSelector(sel!(languageCode));
    let has_country = locale.respondsToSelector(sel!(countryCode));

    if has_language && has_country {
        let language = locale.languageCode();
        #[allow(deprecated)]
        if let Some(country) = locale.countryCode() {
            return format!("{}_{}.UTF-8", language, country);
        }
    }

    // Fallback: use localeIdentifier.
    locale.localeIdentifier().to_string() + ".UTF-8"
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::run_with_timeout;
    use std::time::{Duration, Instant};

    #[test]
    fn returns_trimmed_stdout_on_success() {
        let out = run_with_timeout("/bin/sh", &["-c", "echo hello"], Duration::from_secs(5));
        assert_eq!(out.as_deref(), Some("hello"));
    }

    #[test]
    fn returns_none_when_command_fails() {
        let out = run_with_timeout("/bin/sh", &["-c", "exit 3"], Duration::from_secs(5));
        assert_eq!(out, None);
    }

    #[test]
    fn returns_none_when_program_is_missing() {
        let out = run_with_timeout("/nonexistent/binary", &[], Duration::from_secs(5));
        assert_eq!(out, None);
    }

    /// The whole point: a probe that never returns must not stall startup.
    #[test]
    fn gives_up_on_a_hanging_command() {
        let start = Instant::now();
        let out = run_with_timeout("/bin/sh", &["-c", "sleep 60"], Duration::from_millis(300));
        let elapsed = start.elapsed();

        assert_eq!(out, None);
        assert!(
            elapsed < Duration::from_secs(5),
            "timed-out probe should return promptly, took {elapsed:?}"
        );
    }

    /// A child that exits while a grandchild still holds the stdout pipe open
    /// never reaches EOF — the classic `Command::output()` hang. The deadline
    /// must cover that case too, not just a slow direct child.
    #[test]
    fn gives_up_when_a_grandchild_holds_the_pipe_open() {
        let start = Instant::now();
        let out = run_with_timeout(
            "/bin/sh",
            &["-c", "sleep 60 & echo done"],
            Duration::from_millis(300),
        );
        let elapsed = start.elapsed();

        assert_eq!(out, None);
        assert!(
            elapsed < Duration::from_secs(5),
            "pipe-held probe should return promptly, took {elapsed:?}"
        );
    }
}

/// Ensure PATH includes common directories when launched from Finder.
///
/// Finder-launched apps inherit a minimal PATH from launchd that may not
/// include /usr/local/bin, /opt/homebrew/bin, etc.
#[cfg(target_os = "macos")]
fn fix_path() {
    if let Ok(current_path) = std::env::var("PATH") {
        // If PATH already looks complete (has homebrew or local bin), skip.
        if current_path.contains("/usr/local/bin") || current_path.contains("/opt/homebrew/bin") {
            return;
        }
    }

    // Source the user's shell profile to get the full PATH. A login shell runs
    // arbitrary user startup files (version managers, network mounts), so this
    // gets a deadline — an unbounded wait here is a launch hang with no window.
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string());
    let full_path = run_with_timeout(
        &shell,
        &["-l", "-c", "echo $PATH"],
        std::time::Duration::from_secs(5),
    );

    if let Some(full_path) = full_path {
        if !full_path.is_empty() {
            log::debug!(
                "Expanded PATH from login shell: {}",
                &full_path[..full_path.len().min(200)]
            );
            unsafe { std::env::set_var("PATH", &full_path) };
        }
    }
}

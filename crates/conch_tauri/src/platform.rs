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

/// Discover `SSH_AUTH_SOCK` from the launchd environment if not already set.
#[cfg(target_os = "macos")]
fn set_ssh_auth_sock() {
    if std::env::var("SSH_AUTH_SOCK").is_ok() {
        return;
    }

    let output = std::process::Command::new("launchctl")
        .args(["getenv", "SSH_AUTH_SOCK"])
        .output();

    match output {
        Ok(out) if out.status.success() => {
            let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !path.is_empty() {
                log::debug!("Discovered SSH_AUTH_SOCK from launchd: {path}");
                unsafe { std::env::set_var("SSH_AUTH_SOCK", &path) };
            }
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

    let env_locale_c = CString::new("").unwrap();
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
        let fallback = CString::new("UTF-8").unwrap();
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

    // Source the user's shell profile to get the full PATH.
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string());
    let output = std::process::Command::new(&shell)
        .args(["-l", "-c", "echo $PATH"])
        .output();

    if let Ok(out) = output {
        if out.status.success() {
            let full_path = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !full_path.is_empty() {
                log::debug!(
                    "Expanded PATH from login shell: {}",
                    &full_path[..full_path.len().min(200)]
                );
                unsafe { std::env::set_var("PATH", &full_path) };
            }
        }
    }
}

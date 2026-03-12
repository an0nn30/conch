//! Platform capability detection.
//!
//! Instead of scattering `cfg!(target_os = ...)` checks throughout the UI code,
//! this module describes what the current platform supports in a single struct.
//! UI code queries capabilities rather than testing OS names.

use conch_core::config::WindowDecorations;

/// Describes UI capabilities of the current platform.
#[derive(Debug, Clone)]
pub struct PlatformCapabilities {
    /// Whether the platform supports `fullsize_content_view` (content extends behind title bar).
    pub fullsize_content_view: bool,
    /// Whether the platform supports transparent window backgrounds.
    pub transparent_windows: bool,
    /// Whether the platform supports completely hiding window decorations
    /// while remaining usable (buttonless mode needs a drag region fallback).
    pub buttonless_decorations: bool,
    /// Whether a native global menu bar is available (macOS).
    pub native_global_menu: bool,
}

impl PlatformCapabilities {
    /// Detect capabilities for the current platform.
    pub fn current() -> Self {
        Self {
            fullsize_content_view: cfg!(target_os = "macos"),
            transparent_windows: cfg!(any(target_os = "macos", target_os = "linux")),
            buttonless_decorations: cfg!(target_os = "macos"),
            native_global_menu: cfg!(target_os = "macos"),
        }
    }

    /// Validate and clamp a user-chosen decoration style to what the platform
    /// actually supports, falling back to `Full` for unsupported modes.
    pub fn effective_decorations(&self, requested: WindowDecorations) -> WindowDecorations {
        match requested {
            WindowDecorations::Buttonless if !self.buttonless_decorations => {
                log::warn!("Buttonless decorations not supported on this platform, using Full");
                WindowDecorations::Full
            }
            WindowDecorations::Transparent if !self.transparent_windows => {
                log::warn!("Transparent decorations not supported on this platform, using Full");
                WindowDecorations::Full
            }
            other => other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all_capable() -> PlatformCapabilities {
        PlatformCapabilities {
            fullsize_content_view: true,
            transparent_windows: true,
            buttonless_decorations: true,
            native_global_menu: true,
        }
    }

    fn no_capabilities() -> PlatformCapabilities {
        PlatformCapabilities {
            fullsize_content_view: false,
            transparent_windows: false,
            buttonless_decorations: false,
            native_global_menu: false,
        }
    }

    #[test]
    fn effective_full_always_allowed() {
        let p = no_capabilities();
        assert_eq!(p.effective_decorations(WindowDecorations::Full), WindowDecorations::Full);
    }

    #[test]
    fn effective_none_always_allowed() {
        let p = no_capabilities();
        assert_eq!(p.effective_decorations(WindowDecorations::None), WindowDecorations::None);
    }

    #[test]
    fn effective_buttonless_falls_back_when_unsupported() {
        let p = no_capabilities();
        assert_eq!(p.effective_decorations(WindowDecorations::Buttonless), WindowDecorations::Full);
    }

    #[test]
    fn effective_buttonless_allowed_when_supported() {
        let p = all_capable();
        assert_eq!(p.effective_decorations(WindowDecorations::Buttonless), WindowDecorations::Buttonless);
    }

    #[test]
    fn effective_transparent_falls_back_when_unsupported() {
        let p = no_capabilities();
        assert_eq!(p.effective_decorations(WindowDecorations::Transparent), WindowDecorations::Full);
    }

    #[test]
    fn effective_transparent_allowed_when_supported() {
        let p = all_capable();
        assert_eq!(p.effective_decorations(WindowDecorations::Transparent), WindowDecorations::Transparent);
    }

    #[test]
    fn current_returns_valid_struct() {
        // Smoke test — just ensure it doesn't panic.
        let _ = PlatformCapabilities::current();
    }
}

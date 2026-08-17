//! Window configuration: decorations and initial dimensions.

use serde::{Deserialize, Serialize};

/// Window decoration style (mirrors Alacritty `window.decorations`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Default)]
pub enum WindowDecorations {
    #[default]
    Full,
    Transparent,
    Buttonless,
    None,
}

impl<'de> Deserialize<'de> for WindowDecorations {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        match s.to_lowercase().as_str() {
            "full" => Ok(Self::Full),
            "transparent" => Ok(Self::Transparent),
            "buttonless" => Ok(Self::Buttonless),
            "none" => Ok(Self::None),
            _ => Err(serde::de::Error::unknown_variant(
                &s,
                &["Full", "Transparent", "Buttonless", "None"],
            )),
        }
    }
}

/// Default window size in terminal cells, as other terminals express it.
///
/// The Rust side can only estimate the pixel size for these (cell metrics
/// depend on the font, which lives in the webview), so it opens the window at
/// an approximation and the frontend corrects it to the exact cell count once
/// the terminal has measured itself. `0` means "leave the window as the OS
/// sized it" and suppresses that correction.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowDimensions {
    pub columns: u16,
    pub lines: u16,
}

impl Default for WindowDimensions {
    fn default() -> Self {
        Self {
            columns: 102,
            lines: 46,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowConfig {
    pub dimensions: WindowDimensions,
    pub decorations: WindowDecorations,
    /// Open additional windows (Cmd+Shift+N) in zen mode regardless of the
    /// saved layout's own zen state.
    ///
    /// The common shape is one main window with the panels showing and extra
    /// windows used as bare terminals, so this defaults on. A window opened
    /// this way does NOT persist zen back into the shared layout — see
    /// tool-window-runtime.js's save path — otherwise the main window would
    /// inherit it on next launch.
    pub new_window_zen_mode: bool,
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self {
            dimensions: WindowDimensions::default(),
            decorations: WindowDecorations::default(),
            new_window_zen_mode: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Deserialize)]
    struct DecWrapper {
        decorations: WindowDecorations,
    }

    fn parse_dec(toml_str: &str) -> Result<WindowDecorations, toml::de::Error> {
        let w: DecWrapper = toml::from_str(toml_str)?;
        Ok(w.decorations)
    }

    #[test]
    fn new_window_zen_mode_defaults_on_and_round_trips() {
        assert!(WindowConfig::default().new_window_zen_mode);
        let parsed: WindowConfig = toml::from_str("new_window_zen_mode = false").unwrap();
        assert!(!parsed.new_window_zen_mode);
        // Absent from an existing config file: the default still applies.
        let empty: WindowConfig = toml::from_str("").unwrap();
        assert!(empty.new_window_zen_mode);
    }

    #[test]
    fn decorations_default_is_full() {
        assert_eq!(WindowDecorations::default(), WindowDecorations::Full);
    }

    #[test]
    fn decorations_deserialize_full() {
        assert_eq!(
            parse_dec(r#"decorations = "Full""#).unwrap(),
            WindowDecorations::Full
        );
    }

    #[test]
    fn decorations_deserialize_case_insensitive() {
        assert_eq!(
            parse_dec(r#"decorations = "transparent""#).unwrap(),
            WindowDecorations::Transparent
        );
        assert_eq!(
            parse_dec(r#"decorations = "BUTTONLESS""#).unwrap(),
            WindowDecorations::Buttonless
        );
    }

    #[test]
    fn decorations_deserialize_none() {
        assert_eq!(
            parse_dec(r#"decorations = "none""#).unwrap(),
            WindowDecorations::None
        );
    }

    #[test]
    fn decorations_deserialize_buttonless() {
        assert_eq!(
            parse_dec(r#"decorations = "buttonless""#).unwrap(),
            WindowDecorations::Buttonless
        );
    }

    #[test]
    fn decorations_invalid_value_errors() {
        assert!(parse_dec(r#"decorations = "fancy""#).is_err());
    }

    #[test]
    fn dimensions_default() {
        let d = WindowDimensions::default();
        assert_eq!(d.columns, 102);
        assert_eq!(d.lines, 46);
    }

    #[test]
    fn dimensions_zero_is_preserved_as_the_leave_it_alone_escape_hatch() {
        // 0 means "let the OS size the window"; the frontend skips its
        // correction on it, so it must survive a round trip rather than being
        // normalised away by the Default impl.
        let parsed: WindowDimensions = toml::from_str("columns = 0\nlines = 0").unwrap();
        assert_eq!(parsed.columns, 0);
        assert_eq!(parsed.lines, 0);
    }

    #[test]
    fn window_config_roundtrip() {
        let cfg = WindowConfig::default();
        let toml_str = toml::to_string(&cfg).unwrap();
        let parsed: WindowConfig = toml::from_str(&toml_str).unwrap();
        assert_eq!(parsed.dimensions, cfg.dimensions);
    }
}

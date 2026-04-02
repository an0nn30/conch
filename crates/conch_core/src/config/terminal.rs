//! Terminal configuration: shell, cursor, scroll, and environment.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use super::FontConfig;

/// Terminal backend mode.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TerminalBackend {
    #[default]
    Local,
    Tmux,
}

/// Tmux-specific configuration. Only used when `backend = "tmux"`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TmuxConfig {
    /// Path to tmux binary. Empty string means search `$PATH`.
    pub binary: String,
    /// What to do when a window opens in tmux mode.
    pub startup_behavior: TmuxStartupBehavior,
    /// What "New Tab" does in tmux mode.
    pub new_tab_behavior: TmuxNewTabBehavior,
    /// What "New Window" does in tmux mode.
    pub new_window_behavior: TmuxNewWindowBehavior,
}

impl Default for TmuxConfig {
    fn default() -> Self {
        Self {
            binary: String::new(),
            startup_behavior: TmuxStartupBehavior::default(),
            new_tab_behavior: TmuxNewTabBehavior::default(),
            new_window_behavior: TmuxNewWindowBehavior::default(),
        }
    }
}

impl TmuxConfig {
    /// Returns the tmux binary path, defaulting to `"tmux"` if empty.
    pub fn resolved_binary(&self) -> &str {
        let b = self.binary.trim();
        if b.is_empty() { "tmux" } else { b }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TmuxStartupBehavior {
    #[default]
    AttachLastSession,
    ShowSessionPicker,
    CreateNewSession,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TmuxNewTabBehavior {
    #[default]
    NewTmuxWindow,
    SessionPicker,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TmuxNewWindowBehavior {
    #[default]
    AttachSameSession,
    ShowSessionPicker,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TerminalConfig {
    pub backend: TerminalBackend,
    pub tmux: TmuxConfig,
    pub shell: TerminalShell,
    pub env: HashMap<String, String>,
    pub cursor: CursorConfig,
    pub scroll_sensitivity: f32,
    pub font: FontConfig,
}

impl Default for TerminalConfig {
    fn default() -> Self {
        Self {
            backend: TerminalBackend::default(),
            tmux: TmuxConfig::default(),
            shell: TerminalShell::default(),
            env: HashMap::new(),
            cursor: CursorConfig::default(),
            scroll_sensitivity: 0.15,
            font: FontConfig::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TerminalShell {
    pub program: String,
    pub args: Vec<String>,
}

impl Default for TerminalShell {
    fn default() -> Self {
        Self {
            program: String::new(),
            args: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CursorConfig {
    pub style: CursorStyleConfig,
    pub vi_mode_style: Option<CursorStyleConfig>,
}

impl Default for CursorConfig {
    fn default() -> Self {
        Self {
            style: CursorStyleConfig::default(),
            vi_mode_style: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CursorStyleConfig {
    pub shape: String,
    #[serde(deserialize_with = "deserialize_blinking")]
    pub blinking: bool,
}

impl Default for CursorStyleConfig {
    fn default() -> Self {
        Self {
            shape: "Block".into(),
            blinking: true,
        }
    }
}

fn deserialize_blinking<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<bool, D::Error> {
    use serde::de;

    struct BlinkingVisitor;

    impl<'de> de::Visitor<'de> for BlinkingVisitor {
        type Value = bool;

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("a boolean or one of \"Never\", \"Off\", \"On\", \"Always\"")
        }

        fn visit_bool<E: de::Error>(self, v: bool) -> Result<bool, E> {
            Ok(v)
        }

        fn visit_str<E: de::Error>(self, v: &str) -> Result<bool, E> {
            match v.to_lowercase().as_str() {
                "always" | "on" => Ok(true),
                "never" | "off" => Ok(false),
                _ => Err(de::Error::unknown_variant(
                    v,
                    &["Never", "Off", "On", "Always"],
                )),
            }
        }
    }

    deserializer.deserialize_any(BlinkingVisitor)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_style_default() {
        let s = CursorStyleConfig::default();
        assert_eq!(s.shape, "Block");
        assert!(s.blinking);
    }

    #[test]
    fn terminal_config_default_scroll() {
        assert_eq!(TerminalConfig::default().scroll_sensitivity, 0.15);
    }

    #[test]
    fn blinking_deserialize_true() {
        let cfg: CursorStyleConfig = toml::from_str(
            r#"shape = "Block"
blinking = true"#,
        )
        .unwrap();
        assert!(cfg.blinking);
    }

    #[test]
    fn blinking_deserialize_false() {
        let cfg: CursorStyleConfig = toml::from_str(
            r#"shape = "Block"
blinking = false"#,
        )
        .unwrap();
        assert!(!cfg.blinking);
    }

    #[test]
    fn blinking_deserialize_always_string() {
        let cfg: CursorStyleConfig = toml::from_str(
            r#"shape = "Block"
blinking = "Always""#,
        )
        .unwrap();
        assert!(cfg.blinking);
    }

    #[test]
    fn blinking_deserialize_on_string() {
        let cfg: CursorStyleConfig = toml::from_str(
            r#"shape = "Block"
blinking = "On""#,
        )
        .unwrap();
        assert!(cfg.blinking);
    }

    #[test]
    fn blinking_deserialize_never_string() {
        let cfg: CursorStyleConfig = toml::from_str(
            r#"shape = "Block"
blinking = "Never""#,
        )
        .unwrap();
        assert!(!cfg.blinking);
    }

    #[test]
    fn blinking_deserialize_off_string() {
        let cfg: CursorStyleConfig = toml::from_str(
            r#"shape = "Block"
blinking = "off""#,
        )
        .unwrap();
        assert!(!cfg.blinking);
    }

    #[test]
    fn blinking_deserialize_invalid_string_errors() {
        let result: Result<CursorStyleConfig, _> = toml::from_str(
            r#"shape = "Block"
blinking = "maybe""#,
        );
        assert!(result.is_err());
    }

    #[test]
    fn shell_default_empty() {
        let s = TerminalShell::default();
        assert!(s.program.is_empty());
        assert!(s.args.is_empty());
    }

    #[test]
    fn terminal_config_roundtrip() {
        let cfg = TerminalConfig::default();
        let toml_str = toml::to_string(&cfg).unwrap();
        let parsed: TerminalConfig = toml::from_str(&toml_str).unwrap();
        assert_eq!(parsed.scroll_sensitivity, cfg.scroll_sensitivity);
        assert_eq!(parsed.shell, cfg.shell);
    }

    #[test]
    fn default_backend_is_local() {
        let cfg = TerminalConfig::default();
        assert_eq!(cfg.backend, TerminalBackend::Local);
    }

    #[test]
    fn parse_tmux_backend() {
        let cfg: TerminalConfig = toml::from_str(r#"backend = "tmux""#).unwrap();
        assert_eq!(cfg.backend, TerminalBackend::Tmux);
    }

    #[test]
    fn tmux_config_defaults() {
        let cfg: TerminalConfig = toml::from_str("").unwrap();
        assert_eq!(cfg.tmux.binary, "");
        assert_eq!(cfg.tmux.startup_behavior, TmuxStartupBehavior::AttachLastSession);
        assert_eq!(cfg.tmux.new_tab_behavior, TmuxNewTabBehavior::NewTmuxWindow);
        assert_eq!(cfg.tmux.new_window_behavior, TmuxNewWindowBehavior::AttachSameSession);
    }

    #[test]
    fn tmux_config_serde_roundtrip() {
        let cfg = TmuxConfig {
            binary: "/opt/homebrew/bin/tmux".into(),
            startup_behavior: TmuxStartupBehavior::ShowSessionPicker,
            new_tab_behavior: TmuxNewTabBehavior::SessionPicker,
            new_window_behavior: TmuxNewWindowBehavior::ShowSessionPicker,
        };
        let s = toml::to_string(&cfg).unwrap();
        let parsed: TmuxConfig = toml::from_str(&s).unwrap();
        assert_eq!(cfg, parsed);
    }

    #[test]
    fn backward_compat_no_tmux_section() {
        let toml_str = r#"
            [shell]
            program = "/bin/zsh"
            args = ["-l"]
            scroll_sensitivity = 0.2
        "#;
        let cfg: TerminalConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(cfg.backend, TerminalBackend::Local);
        assert_eq!(cfg.shell.program, "/bin/zsh");
        assert_eq!(cfg.tmux, TmuxConfig::default());
    }

    #[test]
    fn resolved_binary_empty() {
        let cfg = TmuxConfig::default();
        assert_eq!(cfg.resolved_binary(), "tmux");
    }

    #[test]
    fn resolved_binary_explicit() {
        let cfg = TmuxConfig {
            binary: "/opt/homebrew/bin/tmux".into(),
            ..Default::default()
        };
        assert_eq!(cfg.resolved_binary(), "/opt/homebrew/bin/tmux");
    }

    #[test]
    fn resolved_binary_whitespace_only() {
        let cfg = TmuxConfig {
            binary: "   ".into(),
            ..Default::default()
        };
        assert_eq!(cfg.resolved_binary(), "tmux");
    }
}

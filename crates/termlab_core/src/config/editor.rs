//! Editor configuration: the light editor's own preferences.
//!
//! Deliberately separate from `[terminal]`: the editor borrows the terminal's
//! font, but its behaviour is its own. Vim keybindings remain opt-in, while
//! LSP support defaults on so existing config files gain the later editor
//! capabilities without a migration step.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct EditorConfig {
    /// Route the editor's keystrokes through vim's modal keymap.
    ///
    /// Off by default — `bool`'s own default, which is why `Default` is derived
    /// here rather than written out as the sibling config structs do. The value
    /// is read when an editor pane is created and re-applied live on
    /// `config-changed`, so toggling it does not need a restart.
    pub vim_mode: bool,
    /// Editor language-service preferences, separate from terminal settings.
    pub lsp: LspConfig,
}

impl Default for EditorConfig {
    fn default() -> Self {
        Self {
            vim_mode: false,
            lsp: LspConfig::default(),
        }
    }
}

/// Product-level language-service preferences. Server discovery and lifecycle
/// deliberately live elsewhere; this type only preserves user intent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LspConfig {
    pub enabled: bool,
    pub suggestions_while_typing: bool,
    pub languages: LspLanguageConfig,
}

impl Default for LspConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            suggestions_while_typing: true,
            languages: LspLanguageConfig::default(),
        }
    }
}

/// Stable per-language feature flags. TypeScript also covers JavaScript; the
/// clangd adapter covers C and C++.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LspLanguageConfig {
    pub typescript: bool,
    pub json: bool,
    pub python: bool,
    pub rust: bool,
    pub go: bool,
    pub clangd: bool,
    pub java: bool,
}

impl Default for LspLanguageConfig {
    fn default() -> Self {
        Self {
            typescript: true,
            json: true,
            python: true,
            rust: true,
            go: true,
            clangd: true,
            java: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vim_mode_defaults_off() {
        assert!(
            !EditorConfig::default().vim_mode,
            "modal editing must never be the default"
        );
    }

    #[test]
    fn vim_mode_round_trips() {
        let parsed: EditorConfig = toml::from_str("vim_mode = true").unwrap();
        assert!(parsed.vim_mode);
        let text = toml::to_string(&parsed).unwrap();
        let back: EditorConfig = toml::from_str(&text).unwrap();
        assert!(back.vim_mode);
    }

    #[test]
    fn absent_section_keeps_the_default() {
        // An existing config.toml written before this section existed.
        let empty: EditorConfig = toml::from_str("").unwrap();
        assert!(!empty.vim_mode);
    }
}

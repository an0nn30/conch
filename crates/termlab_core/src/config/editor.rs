//! Editor configuration: the light editor's own preferences.
//!
//! Deliberately separate from `[terminal]`: the editor borrows the terminal's
//! font, but its behaviour is its own. Today that is a single opt-in — vim
//! keybindings — which is off unless asked for, because turning modal editing
//! on for someone who did not ask makes every keystroke do the wrong thing.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct EditorConfig {
    /// Route the editor's keystrokes through vim's modal keymap.
    ///
    /// Off by default — `bool`'s own default, which is why `Default` used to
    /// be derived here rather than written out as the sibling config structs
    /// do. The value is read when an editor pane is created and re-applied
    /// live on `config-changed`, so toggling it does not need a restart.
    pub vim_mode: bool,

    /// Which view mode a markdown file opens in: "editor", "split" or
    /// "preview". Defaults to "editor" so nothing changes for anyone who has
    /// not asked for a preview. Unrecognised values are treated as "editor"
    /// by the frontend rather than rejected, so a typo degrades to today's
    /// behaviour instead of failing the whole config load.
    pub preview_default_mode: String,
}

impl Default for EditorConfig {
    fn default() -> Self {
        Self {
            vim_mode: false,
            preview_default_mode: "editor".to_string(),
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

    #[test]
    fn preview_default_mode_defaults_to_editor() {
        assert_eq!(
            EditorConfig::default().preview_default_mode,
            "editor",
            "preview must be opt-in, never the default view"
        );
    }

    #[test]
    fn preview_default_mode_round_trips() {
        let parsed: EditorConfig = toml::from_str(r#"preview_default_mode = "split""#).unwrap();
        assert_eq!(parsed.preview_default_mode, "split");
        let text = toml::to_string(&parsed).unwrap();
        let back: EditorConfig = toml::from_str(&text).unwrap();
        assert_eq!(back.preview_default_mode, "split");
    }

    #[test]
    fn editor_config_without_preview_key_still_parses() {
        // Backward compatibility: configs written before this field existed.
        let parsed: EditorConfig = toml::from_str("vim_mode = true").unwrap();
        assert!(parsed.vim_mode);
        assert_eq!(parsed.preview_default_mode, "editor");
    }
}

# Theme Preview Box — Design Spec

## Summary

Add an inline theme preview box to the Settings > Appearance section, below the theme dropdown. The preview shows a static terminal mockup (prompt, file listing, commands) colored with the selected theme's palette, plus two rows of ANSI color swatches. The preview updates on dropdown selection — before the user clicks Apply — via a new backend command that resolves theme colors without changing the active config.

## Motivation

Currently, users must select a theme and click Apply to see what it looks like. There's no way to preview a theme before committing. This makes theme exploration tedious, especially with many installed themes.

## Design Decisions

- **Inline placement** (below dropdown, not side panel): compact, always visible when Appearance section is open, no layout changes to the settings dialog.
- **Update on dropdown selection** (not on Apply, not on hover): gives a clear preview before committing. Requires a backend command to resolve colors without applying.
- **Static HTML mockup** (not real terminal content, not configurable): simple, predictable, no backend dependency for content. Purpose is showing color mapping, not replicating real output.
- **Normal + bright ANSI swatches** (two rows): themes often differentiate significantly between normal and bright variants; two rows add minimal height.

## Backend Changes

### New helper: `resolve_theme_colors_from_scheme`

**File:** `crates/conch_tauri/src/theme.rs`

Extract the color derivation logic from the existing `resolve_theme_colors(config)` into a standalone function:

```rust
pub fn resolve_theme_colors_from_scheme(scheme: &ColorScheme) -> ThemeColors
```

The existing `resolve_theme_colors(config)` calls this internally — no behavior change for existing callers.

### New Tauri command: `preview_theme_colors`

**File:** `crates/conch_tauri/src/settings.rs`

```rust
#[tauri::command]
pub fn preview_theme_colors(name: String) -> Result<ThemeColors, String>
```

- Resolves theme by name via `conch_core::color_scheme::resolve_theme(&name)`
- Converts to `ThemeColors` via `theme::resolve_theme_colors_from_scheme()`
- Returns the full `ThemeColors` JSON (same shape the frontend already consumes from `get_theme_colors`)
- Does **not** modify active config, update state, or emit `config-changed`
- Falls back to Dracula if theme not found (matches existing `resolve_theme` behavior)

Register in the Tauri command handler in `lib.rs`.

## Frontend Changes

### Preview box in settings.js

**Location:** Appearance section, directly below the theme dropdown `<select>`.

**HTML structure:**
- Container `div.theme-preview` with:
  - "PREVIEW" label (styled like other section labels)
  - Terminal mockup area with static content:
    - Prompt line: `user@conch:~/projects $` with colored segments
    - `ls -la` output showing ~8 entries: directories (blue), executables (red), config files (green), dotfiles (yellow), regular files (foreground)
    - `echo "hello world"` command with magenta keyword, yellow string
    - Cursor block (inverted bg/fg or cursor colors if available)
  - Two rows of color swatches:
    - Row 1: 8 normal ANSI colors (black, red, green, yellow, blue, magenta, cyan, white)
    - Row 2: 8 bright ANSI colors

**Styling:**
- Container: themed background from preview data, 1px border, rounded corners, monospace font
- The mockup uses inline styles set from `ThemeColors` data (not CSS variables, since it shows a different theme than the active one)
- The container border and label use existing CSS variables (part of the settings dialog chrome)

### Update function: `updateThemePreview(themeColors)`

Sets inline styles on all colored elements in the preview using values from the `ThemeColors` object:

| Element | Color source |
|---------|-------------|
| Mockup background | `background` |
| Regular text | `foreground` |
| Prompt hostname | `green` |
| Prompt path | `blue` |
| Directories | `blue` |
| Executables | `red` |
| Config files (Cargo.toml) | `green` |
| Dotfiles (.gitignore) | `yellow` |
| Plain files (README.md) | `foreground` |
| Numbers/sizes | `cyan` |
| Permissions | `dim_fg` |
| Command keywords | `magenta` |
| String literals | `yellow` |
| Cursor block | `foreground` bg, `background` fg (inverted) |
| Normal swatch row | `black`, `red`, `green`, `yellow`, `blue`, `magenta`, `cyan`, `white` |
| Bright swatch row | `bright_black` through `bright_white` |

### Trigger behavior

1. **On settings dialog open:** call `invoke('get_theme_colors')` → `updateThemePreview()` with current active theme colors.
2. **On dropdown `change` event:** call `invoke('preview_theme_colors', { name: selectedValue })` → `updateThemePreview()` with returned colors.
3. **On Apply:** normal `save_settings` flow unchanged. The preview already shows the selected theme.
4. **On Cancel:** preview is discarded with the dialog. No cleanup needed.

## Testing

### Rust unit tests

- **`resolve_theme_colors_from_scheme` derivation:** pass a known `ColorScheme` with specific hex values, verify `ThemeColors` fields: `panel_bg` is darker than `background`, `tab_bar_bg` is darker than `panel_bg`, `input_bg` is lighter than `background`, `active_highlight` is lighter than `input_bg`.
- **`preview_theme_colors` with valid theme:** call with "dracula" (built-in fallback), verify it returns non-empty color strings.
- **`preview_theme_colors` with invalid theme:** call with a nonexistent theme name, verify it returns Dracula defaults (graceful fallback, no error).

### Manual verification

- Preview renders correctly when settings dialog opens
- Preview updates when a different theme is selected from dropdown
- Preview colors match the selected theme (compare with theme file)
- Selecting a theme in the dropdown does NOT change the active app theme
- Clicking Apply changes the active theme as before
- Clicking Cancel discards the preview without side effects

## Scope exclusions

- No hover-to-preview on dropdown options
- No user-configurable preview content
- No canvas-based rendering
- No new JS files — all changes contained in `settings.js`
- No changes to the existing theme application pipeline or hot-reload system

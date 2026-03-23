# Theme Preview Box Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add an inline theme preview box to Settings > Appearance that shows a terminal mockup colored with the selected theme's palette, updating on dropdown selection before Apply.

**Architecture:** Extract a `resolve_theme_colors_from_scheme()` helper from the existing theme pipeline so colors can be resolved without mutating config. Add a `preview_theme_colors` Tauri command. Build the preview box HTML and update logic in `settings.js`.

**Tech Stack:** Rust (Tauri v2 commands), vanilla JS (frontend)

**Spec:** `docs/superpowers/specs/2026-03-22-theme-preview-design.md`

---

### Task 1: Extract `resolve_theme_colors_from_scheme` helper

**Files:**
- Modify: `crates/conch_tauri/src/theme.rs:43-81`
- Test: `crates/conch_tauri/src/theme.rs` (new `#[cfg(test)] mod tests`)

- [ ] **Step 1: Write failing tests for the new helper**

Add a test module at the bottom of `theme.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use conch_core::color_scheme::ColorScheme;

    #[test]
    fn resolve_from_scheme_uses_primary_colors() {
        let scheme = ColorScheme::default(); // Dracula
        let tc = resolve_theme_colors_from_scheme(&scheme);
        assert_eq!(tc.background, "#282a36");
        assert_eq!(tc.foreground, "#f8f8f2");
    }

    #[test]
    fn resolve_from_scheme_derives_panel_colors() {
        let scheme = ColorScheme::default();
        let tc = resolve_theme_colors_from_scheme(&scheme);
        // panel_bg should be darker than background
        assert_ne!(tc.panel_bg, tc.background);
        // tab_bar_bg should be darker than panel_bg
        assert_ne!(tc.tab_bar_bg, tc.panel_bg);
        // input_bg should be lighter than background
        assert_ne!(tc.input_bg, tc.background);
    }

    #[test]
    fn resolve_from_scheme_maps_ansi_colors() {
        let scheme = ColorScheme::default();
        let tc = resolve_theme_colors_from_scheme(&scheme);
        assert_eq!(tc.red, "#ff5555");
        assert_eq!(tc.green, "#50fa7b");
        assert_eq!(tc.bright_red, "#ff6e6e");
        assert_eq!(tc.bright_green, "#69ff94");
    }

    #[test]
    fn resolve_from_scheme_handles_cursor_colors() {
        let scheme = ColorScheme::default(); // has cursor colors
        let tc = resolve_theme_colors_from_scheme(&scheme);
        assert_eq!(tc.cursor_text, "#282a36");
        assert_eq!(tc.cursor_color, "#f8f8f2");
    }

    #[test]
    fn resolve_from_scheme_fallback_when_no_cursor() {
        let mut scheme = ColorScheme::default();
        scheme.cursor = None;
        let tc = resolve_theme_colors_from_scheme(&scheme);
        // Falls back to bg/fg
        assert_eq!(tc.cursor_text, scheme.primary.background);
        assert_eq!(tc.cursor_color, scheme.primary.foreground);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p conch_tauri -- theme::tests --no-run 2>&1; cargo test -p conch_tauri -- theme::tests 2>&1`
Expected: compilation error — `resolve_theme_colors_from_scheme` doesn't exist yet.

- [ ] **Step 3: Extract the helper function**

In `crates/conch_tauri/src/theme.rs`, add `resolve_theme_colors_from_scheme` and refactor `resolve_theme_colors` to call it:

```rust
/// Resolve theme colors from a pre-loaded ColorScheme (no config needed).
pub(crate) fn resolve_theme_colors_from_scheme(scheme: &conch_core::color_scheme::ColorScheme) -> ThemeColors {
    let bg = &scheme.primary.background;
    let fg = &scheme.primary.foreground;
    let cursor = scheme.cursor.as_ref();
    let selection = scheme.selection.as_ref();

    ThemeColors {
        background: bg.clone(),
        foreground: fg.clone(),
        cursor_text: cursor.map(|c| c.text.clone()).unwrap_or_else(|| bg.clone()),
        cursor_color: cursor.map(|c| c.cursor.clone()).unwrap_or_else(|| fg.clone()),
        selection_text: selection.map(|s| s.text.clone()).unwrap_or_else(|| fg.clone()),
        selection_bg: selection.map(|s| s.background.clone()).unwrap_or_else(|| lighten(bg, 30)),
        black: scheme.normal.black.clone(),
        red: scheme.normal.red.clone(),
        green: scheme.normal.green.clone(),
        yellow: scheme.normal.yellow.clone(),
        blue: scheme.normal.blue.clone(),
        magenta: scheme.normal.magenta.clone(),
        cyan: scheme.normal.cyan.clone(),
        white: scheme.normal.white.clone(),
        bright_black: scheme.bright.black.clone(),
        bright_red: scheme.bright.red.clone(),
        bright_green: scheme.bright.green.clone(),
        bright_yellow: scheme.bright.yellow.clone(),
        bright_blue: scheme.bright.blue.clone(),
        bright_magenta: scheme.bright.magenta.clone(),
        bright_cyan: scheme.bright.cyan.clone(),
        bright_white: scheme.bright.white.clone(),
        dim_fg: scheme.primary.dim_foreground.clone().unwrap_or_else(|| lighten(bg, 60)),
        panel_bg: darken(bg, 8),
        tab_bar_bg: darken(bg, 14),
        tab_border: lighten(bg, 18),
        input_bg: lighten(bg, 10),
        active_highlight: lighten(bg, 28),
    }
}

pub(crate) fn resolve_theme_colors(config: &UserConfig) -> ThemeColors {
    let scheme = conch_core::color_scheme::resolve_theme(&config.colors.theme);
    resolve_theme_colors_from_scheme(&scheme)
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p conch_tauri -- theme::tests -v`
Expected: all 5 tests PASS.

- [ ] **Step 5: Run full workspace tests for no regressions**

Run: `cargo test --workspace`
Expected: all existing tests still pass.

- [ ] **Step 6: Commit**

```bash
git add crates/conch_tauri/src/theme.rs
git commit -m "Extract resolve_theme_colors_from_scheme helper for preview support"
```

---

### Task 2: Add `preview_theme_colors` Tauri command

**Files:**
- Modify: `crates/conch_tauri/src/settings.rs:1-5` (add import)
- Modify: `crates/conch_tauri/src/settings.rs:31` (add new command after `list_themes`)
- Modify: `crates/conch_tauri/src/lib.rs:1105` (register command)
- Test: `crates/conch_tauri/src/settings.rs` (add to existing `mod tests`)

- [ ] **Step 1: Write failing tests**

Add to the existing `mod tests` in `settings.rs`:

```rust
#[test]
fn preview_theme_colors_returns_dracula_defaults() {
    let tc = crate::theme::resolve_theme_colors_from_scheme(
        &conch_core::color_scheme::resolve_theme("dracula"),
    );
    assert_eq!(tc.background, "#282a36");
    assert_eq!(tc.red, "#ff5555");
}

#[test]
fn preview_theme_colors_unknown_falls_back_to_dracula() {
    let tc = crate::theme::resolve_theme_colors_from_scheme(
        &conch_core::color_scheme::resolve_theme("nonexistent_theme_xyz"),
    );
    // Should fall back to Dracula
    assert_eq!(tc.background, "#282a36");
}
```

- [ ] **Step 2: Run tests to verify they pass**

These tests call the already-implemented helper, so they should pass now. This verifies the integration path.

Run: `cargo test -p conch_tauri -- settings::tests -v`
Expected: all tests PASS (existing + new).

- [ ] **Step 3: Add the Tauri command**

In `crates/conch_tauri/src/settings.rs`, add the import at the top:

```rust
use crate::theme;
```

After the `list_themes` function (after line 31), add:

```rust
#[tauri::command]
pub(crate) fn preview_theme_colors(name: String) -> theme::ThemeColors {
    let scheme = conch_core::color_scheme::resolve_theme(&name);
    theme::resolve_theme_colors_from_scheme(&scheme)
}
```

- [ ] **Step 4: Register the command in lib.rs**

In `crates/conch_tauri/src/lib.rs`, add `settings::preview_theme_colors` to the `invoke_handler` list after `settings::list_themes` (around line 1105):

```rust
settings::list_themes,
settings::preview_theme_colors,
```

- [ ] **Step 5: Build and run tests**

Run: `cargo build -p conch_tauri && cargo test --workspace`
Expected: builds without errors, all tests pass.

- [ ] **Step 6: Commit**

```bash
git add crates/conch_tauri/src/settings.rs crates/conch_tauri/src/lib.rs
git commit -m "Add preview_theme_colors command for theme preview without applying"
```

---

### Task 3: Add theme preview box to settings.js

**Files:**
- Modify: `crates/conch_tauri/frontend/settings.js:218-239` (in `renderAppearance`)

- [ ] **Step 1: Add the preview box HTML builder and update function**

In `settings.js`, after the theme dropdown `addRow` call (line 239) and before the Appearance Mode section (line 241), insert the preview box. Add a helper function `buildThemePreview()` inside the IIFE that returns the preview container element, and an `updateThemePreview(container, tc)` function:

```javascript
function buildThemePreview() {
    const preview = document.createElement('div');
    preview.className = 'theme-preview';
    preview.style.cssText = 'border-radius: 6px; padding: 14px; margin: 8px 0 12px 0; font-family: "SF Mono", "Fira Code", "Cascadia Code", monospace; font-size: 12px; line-height: 1.5; border: 1px solid var(--tab-border);';

    preview.innerHTML = `
      <div style="font-size: 10px; text-transform: uppercase; letter-spacing: 0.5px; margin-bottom: 10px;" class="tp-label">PREVIEW</div>
      <div class="tp-prompt"><span class="tp-green tp-bold">user@conch</span><span class="tp-fg">:</span><span class="tp-blue tp-bold">~/projects</span><span class="tp-fg"> $ </span><span class="tp-fg">ls -la</span></div>
      <div class="tp-fg">total 42</div>
      <div><span class="tp-dim">drwxr-xr-x</span>  <span class="tp-cyan">12</span> <span class="tp-fg">user staff</span> <span class="tp-cyan">384</span> <span class="tp-fg">Mar 22</span> <span class="tp-blue tp-bold">.</span></div>
      <div><span class="tp-dim">drwxr-xr-x</span>   <span class="tp-cyan">5</span> <span class="tp-fg">user staff</span> <span class="tp-cyan">160</span> <span class="tp-fg">Mar 20</span> <span class="tp-blue tp-bold">..</span></div>
      <div><span class="tp-dim">drwxr-xr-x</span>   <span class="tp-cyan">8</span> <span class="tp-fg">user staff</span> <span class="tp-cyan">256</span> <span class="tp-fg">Mar 22</span> <span class="tp-blue tp-bold">src/</span></div>
      <div><span class="tp-dim">drwxr-xr-x</span>   <span class="tp-cyan">4</span> <span class="tp-fg">user staff</span> <span class="tp-cyan">128</span> <span class="tp-fg">Mar 21</span> <span class="tp-blue tp-bold">tests/</span></div>
      <div><span class="tp-dim">-rw-r--r--</span>   <span class="tp-cyan">1</span> <span class="tp-fg">user staff</span> <span class="tp-cyan">2.1K</span> <span class="tp-fg">Mar 22</span> <span class="tp-green">Cargo.toml</span></div>
      <div><span class="tp-dim">-rw-r--r--</span>   <span class="tp-cyan">1</span> <span class="tp-fg">user staff</span>  <span class="tp-cyan">847</span> <span class="tp-fg">Mar 20</span> <span class="tp-fg">README.md</span></div>
      <div><span class="tp-dim">-rwxr-xr-x</span>   <span class="tp-cyan">1</span> <span class="tp-fg">user staff</span> <span class="tp-cyan">4.2K</span> <span class="tp-fg">Mar 21</span> <span class="tp-red tp-bold">build.sh</span></div>
      <div><span class="tp-dim">-rw-r--r--</span>   <span class="tp-cyan">1</span> <span class="tp-fg">user staff</span>  <span class="tp-cyan">312</span> <span class="tp-fg">Mar 19</span> <span class="tp-yellow">.gitignore</span></div>
      <div style="margin-top: 4px;"><span class="tp-green tp-bold">user@conch</span><span class="tp-fg">:</span><span class="tp-blue tp-bold">~/projects</span><span class="tp-fg"> $ </span><span class="tp-magenta">echo</span> <span class="tp-yellow">"hello world"</span></div>
      <div class="tp-fg">hello world</div>
      <div style="margin-top: 4px;"><span class="tp-green tp-bold">user@conch</span><span class="tp-fg">:</span><span class="tp-blue tp-bold">~/projects</span><span class="tp-fg"> $ </span><span class="tp-cursor">&nbsp;</span></div>
      <div class="tp-swatches" style="display: flex; gap: 4px; margin-top: 12px; padding-top: 10px; border-top-width: 1px; border-top-style: solid;">
        <span class="tp-swatch-black"></span><span class="tp-swatch-red"></span><span class="tp-swatch-green"></span><span class="tp-swatch-yellow"></span><span class="tp-swatch-blue"></span><span class="tp-swatch-magenta"></span><span class="tp-swatch-cyan"></span><span class="tp-swatch-white"></span>
      </div>
      <div class="tp-swatches-bright" style="display: flex; gap: 4px; margin-top: 4px;">
        <span class="tp-swatch-bright-black"></span><span class="tp-swatch-bright-red"></span><span class="tp-swatch-bright-green"></span><span class="tp-swatch-bright-yellow"></span><span class="tp-swatch-bright-blue"></span><span class="tp-swatch-bright-magenta"></span><span class="tp-swatch-bright-cyan"></span><span class="tp-swatch-bright-white"></span>
      </div>
    `;

    // Style swatch spans
    for (const s of preview.querySelectorAll('[class^="tp-swatch"]')) {
      s.style.cssText = 'width: 16px; height: 16px; border-radius: 3px; display: inline-block;';
    }

    return preview;
}

function updateThemePreview(container, tc) {
    container.style.backgroundColor = tc.background;
    container.style.borderColor = tc.tab_border || tc.active_highlight;

    const set = (cls, color) => {
      for (const el of container.querySelectorAll('.' + cls)) el.style.color = color;
    };
    set('tp-fg', tc.foreground);
    set('tp-dim', tc.dim_fg);
    set('tp-label', tc.dim_fg);
    set('tp-green', tc.green);
    set('tp-blue', tc.blue);
    set('tp-cyan', tc.cyan);
    set('tp-red', tc.red);
    set('tp-yellow', tc.yellow);
    set('tp-magenta', tc.magenta);

    for (const el of container.querySelectorAll('.tp-bold')) el.style.fontWeight = 'bold';

    // Cursor block
    const cursor = container.querySelector('.tp-cursor');
    if (cursor) {
      cursor.style.backgroundColor = tc.cursor_color || tc.foreground;
      cursor.style.color = tc.cursor_text || tc.background;
    }

    // Swatch border color for swatches divider
    const swatchRow = container.querySelector('.tp-swatches');
    if (swatchRow) swatchRow.style.borderTopColor = tc.active_highlight;

    // Normal swatches
    const normalColors = ['black','red','green','yellow','blue','magenta','cyan','white'];
    for (const name of normalColors) {
      const el = container.querySelector('.tp-swatch-' + name);
      if (el) el.style.backgroundColor = tc[name];
    }
    // Bright swatches
    for (const name of normalColors) {
      const el = container.querySelector('.tp-swatch-bright-' + name);
      if (el) el.style.backgroundColor = tc['bright_' + name];
    }
}
```

- [ ] **Step 2: Wire the preview into renderAppearance**

In `renderAppearance`, after the `addRow(c, 'Theme', ...)` line (line 239), insert:

```javascript
    // Theme preview box
    const previewBox = buildThemePreview();
    c.appendChild(previewBox);

    // Load initial preview colors
    invoke('get_theme_colors').then(tc => updateThemePreview(previewBox, tc));

    // Update preview on theme selection change (before Apply)
    themeSelect.addEventListener('change', () => {
      invoke('preview_theme_colors', { name: themeSelect.value })
        .then(tc => updateThemePreview(previewBox, tc));
    });
```

Note: the existing `change` listener on `themeSelect` (line 236-238) sets `pendingSettings.colors.theme`. This new listener is a second listener on the same event — both fire. Do NOT remove the existing one.

- [ ] **Step 3: Build and manually test**

Run: `cargo tauri dev`

Verify:
1. Open Settings — preview box appears below the theme dropdown showing current theme colors
2. Change theme in dropdown — preview updates immediately with the new theme's colors
3. The app's actual theme does NOT change until Apply is clicked
4. Cancel discards — no side effects
5. Both swatch rows display (normal + bright)

- [ ] **Step 4: Commit**

```bash
git add crates/conch_tauri/frontend/settings.js
git commit -m "Add theme preview box to settings appearance section"
```

---

### Task 4: Final verification and cleanup

**Files:**
- All files from Tasks 1-3

- [ ] **Step 1: Run full workspace tests**

Run: `cargo test --workspace -v`
Expected: all tests pass, including the new `theme::tests` and `settings::tests`.

- [ ] **Step 2: Run clippy**

Run: `cargo clippy --workspace -- -D warnings`
Expected: no warnings.

- [ ] **Step 3: Verify no regressions in theme hot-reload**

Run: `cargo tauri dev`

Verify:
1. Edit a theme `.toml` file in `~/.config/conch/themes/` — app updates live (existing hot-reload still works)
2. Change theme via Settings > Apply — app theme changes as before
3. Open Settings again after applying — preview shows the newly active theme

- [ ] **Step 4: Final commit if any fixups were needed**

```bash
git add -A
git commit -m "Fix lint/test issues from theme preview implementation"
```

Only if fixups were required — skip if nothing changed.

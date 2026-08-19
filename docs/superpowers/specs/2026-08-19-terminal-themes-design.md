# Terminal-Only Themes — Design

**Status:** Draft
**Date:** 2026-08-19
**Scope:** User-facing terminal theme management, fully decoupled from app appearance: an `auto` default that tracks appearance via the two built-in TermLab palettes, a themes directory for drop-in Alacritty theme files, a settings picker, and an executable Alacritty-compatibility contract. Builds on TermLab Light (which ships the light built-in palette).

## Context (verified 2026-08-19)

- The machinery largely exists: `colors.theme: String` (default `"dracula"`) → `color_scheme::resolve_theme` loads a named Alacritty-format palette → `theme.rs` derives `ThemeColors` (26 fields) → `get_theme_colors` → xterm re-themed live on `config-changed` (`config-runtime.js:36-40`). The extractor already emits `themes/TermLab Dark.toml` in Alacritty format.
- MORE exists than first scouted (verified 2026-08-19 at main 210d1f7): `color_scheme.rs` already implements the user themes directory (`~/.config/termlab/themes/`, `color_scheme.rs:142`), the bundled dir (frontend `themes/`, `:126` — now holding BOTH `TermLab Dark.toml` and `TermLab Light.toml`), and `list_themes()` (`:181`) with a later-dir-wins collision rule (`:160,190` — user themes shadow built-ins).
- What does NOT exist: `auto`, any picker UI, a preview-carrying enumeration command for the frontend, appearance-awareness anywhere in resolution, and a verified compatibility contract (the parse schema is unaudited — plan task one audits it).

## Product rules (settled in brainstorm, 2026-08-19)

1. Fully decoupled: picking a terminal theme sticks regardless of app appearance.
2. Default `auto`: terminal palette = "TermLab Dark" / "TermLab Light" following the app's resolved appearance (including System flips) until the user picks a concrete theme.
3. Sources: the two built-ins + `~/.config/termlab/themes/*.toml` (or the platform-appropriate config dir the app already uses — same parent as `config.toml`). No bundled third-party set, no import UI.
4. The editor and all app chrome follow app appearance, never the terminal theme (the 7 terminal-derived accent vars in `applyThemeCss` are the sanctioned exception, unchanged).

## Design

### Config

`colors.theme` keeps its type (String) — `"auto"` becomes the new default and a reserved name. Existing configs naming `"dracula"` etc. keep exact current behavior (no migration). Resolution: `resolve_effective_theme(colors, resolved_appearance) -> palette`: `auto` → `TermLab Dark`|`TermLab Light` by appearance; any other name → today's `resolve_theme` lookup (built-ins, then themes dir by file stem, preserving current lookup semantics — audit and preserve, don't redesign). A missing/broken named theme falls back exactly as today (**Task 1 audit's ruling:** the existing fallback — silent log-and-fall-back-to-Dracula, inside `color_scheme::resolve_theme_in`'s name-lookup branch, reached in production through `commands::get_theme_colors` → `theme::resolve_theme_colors_for_appearance` → `resolve_effective_theme` → `resolve_theme` — is preserved as-is, not made loud; the picker's `{ name, error }` list entries, not a toast, are the designated loud path for a broken theme, since a resolution-time toast would fire on every terminal spawn while an editable entry sits in the picker for exactly this reason).

Saving settings from a config that predates the `auto` key **materializes `theme = "auto"` explicitly** into the written TOML: `save_settings` deserializes the whole `UserConfig` and re-serializes it, so every `#[serde(default)]` field — not just this one — becomes explicit on the next write, a pre-existing pattern `auto` did not introduce. This is deliberate and downgrade-safe: a build that predates the reserved name treats `"auto"` as an ordinary theme name, finds no `auto.toml`, and falls back to built-in Dracula — exactly what that older build already showed for the keyless config before the save, since `"dracula"` was the old serde default. The materialized key changes nothing for a binary that does not know about `auto`.

### Appearance coupling for `auto` only

When appearance resolves differently (mode change or OS flip in System), terminals under `auto` must re-theme. Wire: the appearance owner (`appearance.js`, from TermLab Light) triggers the same re-theme path `config-changed` uses, frontend-side (fetch `get_theme_colors` with the resolved appearance passed as an argument — new optional arg on the command — and apply). Rust must not guess appearance; the frontend resolved it (System lives in `matchMedia`, invisible to Rust).

### Theme enumeration

`<config_dir>/themes/` (created lazily). The directory machinery exists (`list_themes()`); what's added is a Tauri command `list_terminal_themes()` layering on top: `[{ name, source: builtin|user, palette_preview: {bg, fg, ansi[16]}, shadows_builtin: bool }]`, sorted built-ins first then user themes by name, rescanned per call (no watcher). Invalid TOML files appear as `{ name, error }` entries so the picker greys them with the parse error rather than hiding them. The EXISTING collision rule stays: user themes shadow built-ins (later-dirs-win, `color_scheme::list_themes_in`) — zero-behavior-change beats my earlier draft's built-ins-win idea; `shadows_builtin` surfaces that rule in the list entry itself (`true` when a user theme's name collides with a bundled built-in) rather than changing it, and the picker renders it as an "Overrides built-in" note.

**Reserved name in the themes directory:** a user file literally named `auto.toml` enumerates like any other user theme but can never be *selected* as itself — `effective_theme_name` intercepts the reserved name `"auto"` (case-insensitively) before any file lookup runs, so picking such an entry could only ever resolve to the real Auto entry, never to that file's own colors. Rather than silently filtering it out (which would drop a file the user placed there with no visible reason), the picker renders it as its own greyed, unselectable `reserved` kind with an explanatory note — following the same surface-don't-hide precedent as the `Broken` kind — whether or not the file also parses. A broken `auto.toml` classifies as `reserved`, not `broken`.

### Settings picker

In Appearance section: a "Terminal theme" row — a combo (existing `tl-combo`) listing Auto + built-ins + user themes, with a small palette strip preview per entry (16 ANSI swatches + bg/fg, rendered from `palette_preview`). Selecting writes `colors.theme` through the normal save path; `config-changed` re-themes live (existing machinery, zero new plumbing).

### Compatibility contract (the "100%")

Any valid Alacritty theme file parses and applies. Concretely, `color_scheme.rs` must accept the full current Alacritty schema: `colors.primary.{background,foreground,dim_foreground,bright_foreground}`, `colors.cursor.{text,cursor}`, `colors.vi_mode_cursor`, `colors.selection.{text,background}`, `colors.search`, `colors.hints`, `colors.line_indicator`, `colors.footer_bar`, `colors.normal.*`, `colors.bright.*`, `colors.dim.*`, `colors.indexed_colors[]`, `colors.transparent_background_colors`, `colors.draw_bold_text_with_bright_colors` — parse ALL without error; apply what has a terminal meaning here (primary, cursor, selection, normal, bright; indexed_colors into the xterm extended palette; `dim` is parsed and preserved but not applied — see Known limitations); ignore the rest by design, listed below. Both `#rrggbb` and `0x`-prefixed and `CellRgb` string forms Alacritty accepts must parse.

**Normalization boundary:** every accepted color form is canonicalized to `#rrggbb` at exactly one point, [`color_normalize::normalize_scheme`], called from `load_theme` — the single fallible file → `ColorScheme` funnel that both the path and name branches of theme resolution, and theme-list preview generation, go through. This matters because downstream consumers (`theme.rs`'s `darken`/`lighten`/`luminance`/`blend`, CSS custom properties, the xterm theme object) all slice the first six hex characters after stripping a leading `#`; left un-normalized, a `0x`-prefixed color like `"0x1e1e2e"` renders as `#001e1e`. `ColorScheme::default()` (built-in Dracula) is already canonical, so after `load_theme` there is no path to a non-canonical `ColorScheme`. `CellForeground`/`CellBackground` (`CellRgb`'s named-sentinel form) resolve against the theme's own `primary.foreground`/`primary.background` — the closest static equivalent available, since TermLab hands xterm a static palette with no per-cell context the way Alacritty's own cursor/selection rendering has.

**`indexed_colors` → `extendedAnsi`:** Alacritty's `colors.indexed_colors[]` is a sparse `{ index, color }` list that may skip indices freely; xterm.js's `ITheme.extendedAnsi` is a dense array whose element 0 is ANSI slot 16. TermLab emits a **sparse** array (`None`/`null` for every unlisted slot 16-255) rather than densifying it against xterm's own default table. Traced against the vendored bundle (`@xterm/xterm@5.5.0`): xterm seeds `i.ansi` from `DEFAULT_ANSI_COLORS` (the full 256-entry standard table) before ever reading `extendedAnsi`, and its overlay loop's per-entry parser only overwrites a slot when the source value is not `undefined` — so a hole is semantically identical to sending xterm's own default for that slot. Building a dense base in Rust would mean re-deriving xterm's default table, which is a drift risk for a byte-identical result the sparse array already gets for free.

**The contract is executable:** fixture tests vendor a representative set of REAL theme files verbatim from github.com/alacritty/alacritty-theme (a dozen spanning schema variety: with/without dim, indexed_colors, 0x colors, comments) under `crates/termlab_core/tests/fixtures/alacritty-themes/` with their upstream license note; a test parses every fixture and snapshots the resolved palettes. Adding a fixture that fails to parse is the regression alarm.

## Known limitations

- **`colors.dim` is parsed and preserved but never applied to the terminal renderer.** Traced against the vendored xterm bundle (`@xterm/xterm@5.5.0`): `_setTheme`'s theme object reads exactly `foreground`, `background`, `cursor`, `cursorAccent`, the two selection keys, `black`..`brightWhite`, and `extendedAnsi` — there is no dim key to hand it. Both xterm renderers derive dim instead of looking it up: the DOM renderer pushes an `xterm-dim` CSS class and halves the minimum-contrast ratio, applying dim as an opacity-style transform on whichever color was already chosen rather than a distinct palette entry. Forcing `colors.dim.*` onto xterm would mean overwriting the normal/bright slots with dim values outright, which is worse than not applying it, so the field round-trips through parsing (fixture-tested) with no terminal-visible effect.
- **Fields parsed but never applied by design**, matching Alacritty features TermLab's terminal surface has no equivalent for: `colors.vi_mode_cursor` (no vi mode), `colors.search`/`colors.hints`/`colors.line_indicator` (no corresponding overlay UI), `colors.footer_bar`, `colors.transparent_background_colors`, `colors.draw_bold_text_with_bright_colors` (window-compositing/rendering toggles TermLab doesn't expose per-theme). All parse without error per the compatibility contract; none affect anything on screen.

## Non-Goals

- No file watcher / hot reload of theme files (the picker rescans on settings open — every `list_terminal_themes` call re-scans the directories — not on save; a save does not change the themes directory, so nothing rescans there).
- No theme editing UI, no import button, no bundled third-party themes.
- No per-pane or per-host terminal themes.

## Constraints

- Branch `feat/terminal-themes` from post-TermLab-Light main; CLAUDE.md rules.
- Zero behavior change for configs that name a concrete existing theme.
- Suites/baselines hold (33 frontend + parity + extractor goldens, cargo 727 — 734 at Task 4 minus 7 tests deliberately deleted in Task 5 alongside their now-removed subject, the caller-less `preview_theme_colors`/`list_themes` stopgap commands; the behavior those tests covered — `auto`/appearance resolution, the Dracula fallback — is proven independently in `termlab_core::effective_theme`/`color_scheme`).

## Testing

- Rust: `auto` resolution both appearances; enumeration (valid, invalid, empty, missing dir); fixture-corpus parse+snapshot; name collision preserves the existing user-shadows-built-in rule with the shadowing surfaced in the list entry.
- Frontend: picker renders entries incl. greyed invalid ones; selection round-trip through save; auto re-theme on appearance flip (stubbed matchMedia + event).
- Manual checklist section K: drop a real downloaded theme in, pick it, verify live apply; break it, verify greyed error entry; auto tracking through Dark/Light/System OS flip.

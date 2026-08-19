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

`colors.theme` keeps its type (String) — `"auto"` becomes the new default and a reserved name. Existing configs naming `"dracula"` etc. keep exact current behavior (no migration). Resolution: `resolve_effective_theme(colors, resolved_appearance) -> palette`: `auto` → `TermLab Dark`|`TermLab Light` by appearance; any other name → today's `resolve_theme` lookup (built-ins, then themes dir by file stem, preserving current lookup semantics — audit and preserve, don't redesign). A missing/broken named theme falls back exactly as today (audit current fallback; keep it, make it loud via existing toast/log path if it is silent — decision recorded at plan time from the audit).

### Appearance coupling for `auto` only

When appearance resolves differently (mode change or OS flip in System), terminals under `auto` must re-theme. Wire: the appearance owner (`appearance.js`, from TermLab Light) triggers the same re-theme path `config-changed` uses, frontend-side (fetch `get_theme_colors` with the resolved appearance passed as an argument — new optional arg on the command — and apply). Rust must not guess appearance; the frontend resolved it (System lives in `matchMedia`, invisible to Rust).

### Theme enumeration

`<config_dir>/themes/` (created lazily). The directory machinery exists (`list_themes()`); what's added is a Tauri command `list_terminal_themes()` layering on top: `[{ name, source: builtin|user, palette_preview: {bg, fg, ansi[16]} }]`, sorted built-ins first then user themes by name, rescanned per call (no watcher). Invalid TOML files appear as `{ name, error }` entries so the picker greys them with the parse error rather than hiding them. The EXISTING collision rule stays: user themes shadow built-ins (later-dirs-win, `color_scheme.rs:160`) — zero-behavior-change beats my earlier draft's built-ins-win idea; the list marks shadowed entries.

### Settings picker

In Appearance section: a "Terminal theme" row — a combo (existing `tl-combo`) listing Auto + built-ins + user themes, with a small palette strip preview per entry (16 ANSI swatches + bg/fg, rendered from `palette_preview`). Selecting writes `colors.theme` through the normal save path; `config-changed` re-themes live (existing machinery, zero new plumbing).

### Compatibility contract (the "100%")

Any valid Alacritty theme file parses and applies. Concretely, `color_scheme.rs` must accept the full current Alacritty schema: `colors.primary.{background,foreground,dim_foreground,bright_foreground}`, `colors.cursor.{text,cursor}`, `colors.vi_mode_cursor`, `colors.selection.{text,background}`, `colors.search`, `colors.hints`, `colors.line_indicator`, `colors.footer_bar`, `colors.normal.*`, `colors.bright.*`, `colors.dim.*`, `colors.indexed_colors[]`, `colors.transparent_background_colors`, `colors.draw_bold_text_with_bright_colors` — parse ALL without error; apply what has a terminal meaning here (primary, cursor, selection, normal, bright, dim if the render path supports it — audit; indexed_colors into the xterm extended palette); ignore the rest by design, listed in the spec's Known limitations. Both `#rrggbb` and `0x`-prefixed and `CellRgb` string forms Alacritty accepts must parse.

**The contract is executable:** fixture tests vendor a representative set of REAL theme files verbatim from github.com/alacritty/alacritty-theme (a dozen spanning schema variety: with/without dim, indexed_colors, 0x colors, comments) under `crates/termlab_core/tests/fixtures/alacritty-themes/` with their upstream license note; a test parses every fixture and snapshots the resolved palettes. Adding a fixture that fails to parse is the regression alarm.

## Non-Goals

- No file watcher / hot reload of theme files (rescan on settings open and on save).
- No theme editing UI, no import button, no bundled third-party themes.
- No per-pane or per-host terminal themes.

## Constraints

- Branch `feat/terminal-themes` from post-TermLab-Light main; CLAUDE.md rules.
- Zero behavior change for configs that name a concrete existing theme.
- Suites/baselines hold (32 frontend + parity + extractor goldens, cargo ≥ 648 + new).

## Testing

- Rust: `auto` resolution both appearances; enumeration (valid, invalid, empty, missing dir); fixture-corpus parse+snapshot; name collision preserves the existing user-shadows-built-in rule with the shadowing surfaced in the list entry.
- Frontend: picker renders entries incl. greyed invalid ones; selection round-trip through save; auto re-theme on appearance flip (stubbed matchMedia + event).
- Manual checklist section K: drop a real downloaded theme in, pick it, verify live apply; break it, verify greyed error entry; auto tracking through Dark/Light/System OS flip.

# Terminal-Only Themes Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** User-facing terminal theme management: an `auto` default tracking app appearance via the built-in TermLab pair, a drop-in themes directory of Alacritty-format files, a settings picker with palette previews, and an executable Alacritty-compatibility contract.

**Architecture:** Rust owns discovery/parsing/resolution (`color_scheme.rs` + a new `theme_dir` module); `auto` is resolved frontend-side (only the frontend knows the System-resolved appearance) by passing the resolved appearance to `get_theme_colors`; the existing `config-changed` → live re-theme path carries everything else unchanged.

**Tech Stack:** Rust serde/toml, existing `ThemeColors` derivation, `tl-combo` settings control, vm-sandbox `.mjs` tests, vendored Alacritty theme fixtures.

**Spec:** `docs/superpowers/specs/2026-08-19-terminal-themes-design.md`

## Global Constraints

- Branch `feat/terminal-themes`; never commit to main; NO Co-Authored-By trailers; imperative commits (CLAUDE.md).
- Zero behavior change for configs naming a concrete existing theme (e.g. `theme = "dracula"`).
- `"auto"` is a reserved name: new default, resolves to `TermLab Dark` / `TermLab Light` (Task 3 of the TermLab-Light plan registered that exact string) by RESOLVED appearance passed from the frontend — Rust never guesses System.
- Compatibility contract: every fixture in `crates/termlab_core/tests/fixtures/alacritty-themes/` parses; unmapped Alacritty keys are accepted-and-ignored by design.
- Baselines: 32 frontend suites, 648 cargo, boundary `tl-dialog.js:334` only, token parity + extractor goldens green. Branch base main@210d1f7.

## Tasks (outline — briefs finalized at branch time)

### Task 1: Audit + schema completion in `color_scheme.rs`
Audit what the current parser accepts (the extractor-emitted TOML shape vs full Alacritty schema); extend the serde model to ACCEPT the full schema (`primary.{background,foreground,dim_foreground,bright_foreground}`, `cursor`, `vi_mode_cursor`, `selection`, `search`, `hints`, `line_indicator`, `footer_bar`, `normal`, `bright`, `dim`, `indexed_colors[]`, `transparent_background_colors`, `draw_bold_text_with_bright_colors`; `#rrggbb` and `0x` color forms) while APPLYING the mapped subset; vendor ~12 real alacritty-theme fixtures (license note) + parse/snapshot test; document current fallback behavior for missing/broken theme names (preserve it; make it loud if silent — report the finding, controller rules).

### Task 2: `list_terminal_themes` command over the EXISTING machinery
The dirs + `list_themes()` + later-dirs-win collision rule already exist (`color_scheme.rs:126,142,160,181`) — do NOT rebuild them. Add the Tauri command layering previews on top: `[{name, source, palette_preview:{bg,fg,ansi[16]}}]` + `{name, error}` entries for invalid TOML; built-ins first then user sorted; existing user-shadows-built-in rule preserved and surfaced on the entry. Unit tests: valid/invalid/empty/missing-dir/shadowing.

### Task 3: `auto` resolution + appearance re-theme wiring
`resolve_effective_theme(colors, resolved_appearance)`; `get_theme_colors` gains optional `resolved_appearance` arg (default: dark, back-compat); default `colors.theme` flips to `"auto"` (serde default; existing files naming themes unchanged); frontend: `appearance.js` flips trigger the re-theme fetch passing `termlabAppearance.current()`; `applyConfigChanged` passes it too. Tests: Rust auto-both-appearances; frontend stubbed-flip re-theme (extend `test_appearance.mjs` harness or new suite).

### Task 4: Settings picker
"Terminal theme" `tl-combo` row in Appearance section: Auto + built-ins + user themes, palette-strip preview (16 swatches + bg/fg from `palette_preview`), greyed error entries with the parse error; selection writes `colors.theme` through the normal save path (live apply arrives via existing `config-changed`). Settings-suite checks: rows render incl. error entry; selection round-trip.

### Task 5: Sweep + manual checklist section K + spec sync
Sweep greps; full suites; checklist K (drop in a real downloaded theme → appears + applies live; break it → greyed with error; auto tracks Dark/Light/System OS flip; concrete theme survives appearance flips; terminal-only promise — app chrome unchanged when picking Gruvbox); spec sync incl. the Task-1 audit's fallback ruling.

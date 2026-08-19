# TermLab Light — Design

**Status:** Draft
**Date:** 2026-08-19
**Scope:** Make TermLab Light a real, selectable app appearance: wire `AppearanceMode` to the (currently dead) `data-tl-appearance` attribute, regenerate complete light tokens, give the editor a genuine light/dark signal, ship the "TermLab Light" built-in terminal palette, and make System appearance follow the OS live. Skins are removed by the preceding `chore/remove-skins` branch; this spec assumes that base.

## Context (verified 2026-08-19)

- `tokens-light.css` is generated (extractor `scripts/extract_intellij_tokens.py`, sources in the JVM repo `../TermLab/core/resources/themes/`) and gated behind `:root[data-tl-appearance="light"]` — but NOTHING sets that attribute. `AppearanceMode` today only tints native window chrome (`windows.rs:20-29`, `terminal-runtime.js:411-438`).
- `tl-icon.js:19-20` reads the attribute (always absent → always dark icon variants). `theme.js:86-92` infers editor dark-vs-light from `--tl-bg` luminance — works unchanged once tokens actually switch.
- The extractor emits a terminal TOML only for dark (from `termlab-dark.xml`); the light theme JSON's `editorScheme` pointer still references the dark XML — there is no light terminal-palette source to extract.
- Token parity is guarded by `scripts/tests/test_token_parity.mjs` (in the standard suite glob) since commit 9ae2898.

## Goals

1. Settings → Appearance: Dark / Light / System all genuinely restyle the whole app — chrome, dialogs, chooser window, editor, icons — with no dark bleed-through under Light.
2. System mode follows the OS live (no relaunch) on every window including secondary ones (settings, chooser).
3. A "TermLab Light" terminal palette ships as a built-in, becoming the light half of the `auto` pair the terminal-themes spec consumes.
4. The dark experience is byte-identical to today when appearance is Dark.

## Design

### The switch (frontend, one owner)

A new small module `app/core/appearance.js` owns the attribute: `applyAppearance(mode)` sets `document.documentElement.dataset.tlAppearance = 'light' | removes it`, resolving `System` via `window.matchMedia('(prefers-color-scheme: dark)')` with a change listener (registered once; re-applies on OS flip). Called from: initial boot (all three HTML entries — index, settings, chooser — after `get_app_config`), and from the `config-changed` handler. The chooser window additionally receives the mode at boot the same way settings does (each window resolves independently from the same config — no cross-window event needed since `config-changed` already broadcasts).

Every consumer that keys off the attribute then works for free: `tokens-light.css` activates, `tl-icon.js` picks `_dark`-vs-plain variants (NOTE: naming is inverted in the vendored set — verify which variant is which and that light mode picks legible glyphs; the redesign flagged `file.svg`/`search.svg` as having no `_dark` variant), and `theme.js`'s luminance inference flips the CodeMirror `dark` flag. Editor panes re-theme via the existing `refreshTheme` path in `applyConfigChanged`.

### Tokens

Regenerate `tokens-light.css` from `TermLabLight.theme.json` (already supported by the extractor); fix any remaining gaps at the SOURCE theme JSON (in the JVM repo, which is the pipeline's input contract) rather than hand-editing generated CSS. The parity test is the gate. Visual gaps that parity can't catch (a token present in both sets but ugly in light) go to the manual checklist.

### Terminal palette

Hand-author `TermLab Light` as a built-in Alacritty-format palette in `termlab_core::color_scheme` beside the existing built-ins, seeded from the light theme JSON's colors (`background #E3E8EF`, `foreground #1F2933`, selection pair) plus a standard legible light ANSI 16 tuned against that background. It is NOT wired as any default in this spec — the terminal keeps following `colors.theme` exactly as today; the follow-up terminal-themes spec introduces `auto` and consumes this palette. (Deliberate: keeps this branch's terminal behavior at zero net change.)

### Native chrome

Already keyed off `AppearanceMode` — no change; verify the settings/chooser windows get `.theme()` consistently (chooser does, `chooser_window.rs:626`).

## Non-Goals

- No `auto` terminal-theme behavior, no theme picker changes, no themes directory (next spec).
- No redesign of light colors beyond what the source theme JSON says (taste fixes go through the JVM theme JSON + regeneration).
- No per-window appearance.

## Constraints

- Branch `feat/termlab-light` from the post-skins-removal main; CLAUDE.md rules (no main commits, no trailers).
- Dark-mode byte-identity: with appearance Dark, computed styles match today's (the attribute is simply absent — assert no CSS rule outside `tokens-light.css` keys on the attribute).
- Tokens-only CSS; boundary + parity tests green; 30-suite baseline + 645 cargo baseline hold.
- `applyThemeCss`'s 7 terminal-derived vars (`--tl-terminal-bg`, six ANSI accents) stay terminal-owned — light app chrome must not repaint them.

## Testing

- Unit (frontend): `appearance.js` — mode→attribute mapping incl. System via a stubbed matchMedia, change-listener re-application, and removal on Dark. `tl-icon.js` variant selection under both attribute states. Editor `isDarkTheme()` against light `--tl-bg`.
- Unit (Rust): built-in palette resolves by name via `resolve_theme("TermLab Light")`; snapshot of its 16+4 colors.
- Extractor golden: extend `test_extract_tokens.py` with a light-source assertion; parity test unchanged (it is the gate).
- Manual checklist section J: full surface walk in Light (chrome, tool windows, settings window, chooser window, editor incl. syntax colors, toasts, context menus), System-mode live OS flip, dark unchanged-spot-check.

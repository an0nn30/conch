# TermLab Light — Design

**Status:** Draft
**Date:** 2026-08-19
**Scope:** Make TermLab Light a real, selectable app appearance: wire `AppearanceMode` to the (currently dead) `data-tl-appearance` attribute, regenerate complete light tokens, give the editor a genuine light/dark signal, ship the "TermLab Light" built-in terminal palette, and make System appearance follow the OS live. Skins are removed by the preceding `chore/remove-skins` branch; this spec assumes that base.

## Context (verified 2026-08-19)

- `tokens-light.css` is generated (extractor `scripts/extract_intellij_tokens.py`, sources in the JVM repo `../TermLab/core/resources/themes/`) and gated behind `:root[data-tl-appearance="light"]` — but NOTHING sets that attribute. `AppearanceMode` today only tints native window chrome (`windows.rs:22-30`, `app/core/config-service.js:38-58` — `resolveNativeWindowTheme`/`applyNativeWindowTheme`; the `terminal-runtime.js:411-438` citation this line originally gave was already stale at spec-authoring time, a leftover from before the `1a2e5a8` frontend-module refactor moved this logic — `terminal-runtime.js` is 204 lines and has no appearance code at all).
- `tl-icon.js:19-20` reads the attribute (always absent → always dark icon variants). `theme.js:86-92` infers editor dark-vs-light from `--tl-bg` luminance. **Corrected in the fix round (F2):** the luminance flag alone is NOT enough — `theme.js:23` preferred `--tl-terminal-bg` for the editor background and took its syntax accents from the terminal-owned `--red/--green/...` vars, both of which this branch deliberately keeps dark. Under Light that painted `#1F2933` text on a near-black canvas (~1.3:1) while flagging itself `dark: false`. The editor now branches on `termlabAppearance.current()`: under Light every colour comes from app tokens; under Dark the code path is byte-identical to before the branch (pinned by `scripts/tests/fixtures/editor-theme-dark-base.mjs`).
- The extractor emits a terminal TOML only for dark (from `termlab-dark.xml`); the light theme JSON's `editorScheme` pointer still references the dark XML — there is no light terminal-palette source to extract.
- Token parity is guarded by `scripts/tests/test_token_parity.mjs` (in the standard suite glob) since commit 9ae2898.

## Goals

1. Settings → Appearance: Dark / Light / System all genuinely restyle the whole app — chrome, dialogs, chooser window, editor, icons — with no dark bleed-through under Light.
2. System mode follows the OS live (no relaunch) on every window including secondary ones (settings, chooser).
3. A "TermLab Light" terminal palette ships as a built-in, becoming the light half of the `auto` pair the terminal-themes spec consumes.
4. The dark experience is byte-identical to today when appearance is Dark.

## Design

### The switch (frontend, one owner)

A new small module `app/core/appearance.js` owns the attribute: `global.termlabAppearance = { apply(mode, deps), current() }`. `apply(mode)` sets `data-tl-appearance="light"` (via `setAttribute`) or removes the attribute entirely for dark, resolving `System` via `window.matchMedia('(prefers-color-scheme: dark)')` with a change listener (registered once; re-applies on OS flip; `deps` optionally injects `doc`/`matchMedia` for tests). Called from: initial boot (all three HTML entries — index, settings, chooser — after `get_app_config`), and from the `config-changed` handler. The chooser window additionally receives the mode at boot the same way settings does (each window resolves independently from the same config — no cross-window event needed since `config-changed` already broadcasts).

**Traced field path (verified during implementation, Task 1):** `get_app_config` (`crates/termlab_tauri/src/commands.rs:29`) returns `appCfg` as a **flat** JSON object — `"appearance_mode": format!("{:?}", cfg.colors.appearance_mode).to_lowercase()` — so the frontend consumes `appCfg.appearance_mode` directly (a lowercase `"dark"`/`"light"`/`"system"` string), NOT `appCfg.colors.appearance_mode`. The nesting under `colors` exists only on the Rust side (`termlab_core::config::colors::UserConfig.appearance_mode`, read by `save_settings`'s save path — `crates/termlab_tauri/src/settings.rs:66`, which deserializes the posted JSON straight into a `UserConfig` and calls `config::save_user_config` — and by `windows.rs`/`chooser_window.rs`'s native-chrome theming) and on the settings-form's in-memory `pendingSettings.colors.appearance_mode` used when writing changes back — `get_app_config`'s read path flattens it before it ever reaches the frontend.

Every consumer that keys off the attribute then works for free: `tokens-light.css` activates, `tl-icon.js` picks `_dark`-vs-plain variants (**confirmed non-inverted, Task 1:** `tl-icon.js:19-20`'s `isDarkAppearance()` is true whenever the attribute is anything other than `"light"`; checked the vendored SVG fills directly — `add.svg` fills `#6E6E6E` (dark grey, legible on light) while `add_dark.svg` fills `#AFB1B3` (light grey, legible on dark) — so `_dark` genuinely names the dark-appearance variant; the redesign's file-dialog spec separately flagged `file.svg`/`search.svg` as having no `_dark` variant at all — same asset both ways, unaffected by this), and `theme.js`'s luminance inference flips the CodeMirror `dark` flag. Editor panes re-theme via the existing `refreshTheme` path in `applyConfigChanged`.

**Amended by the fix round.** "Works for free" held for `tokens-light.css` only. Three consumers bake a value at build time and needed an explicit re-resolve, so `appearance.js` now also ANNOUNCES every resolved change as a `tl-appearance-changed` `CustomEvent` on `document` (`detail.resolved`, fired only when the resolved value actually changes — a dark-only install never dispatches):

- **Icons (F1).** `tl-icon.js` bakes the variant into `img.src` at `create()` time and nothing re-resolved it, so a live flip left the always-visible tool-window rail on `_dark` glyphs. `create()` now stamps `data-tl-icon` with the logical name, and ONE document-level listener drives `tlIcon.refreshAll(root)` over the stamped elements — one subscription for the whole app, not one per icon.
- **Editor (F2).** Colours are baked at `buildTheme()` time and only `applyConfigChanged` rebuilt them, so an OS flip in System mode left open editors entirely stale. `config-runtime.js` subscribes the same pane walk to the announcement.
- **Secondary windows (F3).** `config-changed` is broadcast to every window, but the only listener lived in `config-runtime.js`, which `index.html` alone loads — so the standalone settings window flipped every OTHER window from its own Apply button and stayed on the old appearance itself, and a chooser open across a save stayed stale for its whole life. `app/core/appearance-sync.js` is the small re-apply those two windows now register in their boot scripts (re-fetch `GET_APP_CONFIG` + `GET_THEME_COLORS`, re-apply what that window applies at boot).

### Tokens

Regenerate `tokens-light.css` from `TermLabLight.theme.json` (already supported by the extractor); fix any remaining gaps at the SOURCE theme JSON (in the JVM repo, which is the pipeline's input contract) rather than hand-editing generated CSS. The parity test is the gate. Visual gaps that parity can't catch (a token present in both sets but ugly in light) go to the manual checklist.

### Terminal palette

Hand-author `TermLab Light` as a bundled Alacritty-format TOML theme file, seeded from the light theme JSON's colors (`background #E3E8EF`, `foreground #1F2933`, selection pair) plus a standard legible light ANSI 16 tuned against that background. **Traced mechanism (Task 3):** there is no Rust-struct built-in for named themes other than the final-fallback `ColorScheme::default()` (Dracula) — "TermLab Dark" itself is not a hardcoded struct either, it is `crates/termlab_tauri/frontend/themes/TermLab Dark.toml`, discovered at runtime by `list_themes`/`list_themes_in` (`termlab_core::color_scheme` — `crates/termlab_core/src/color_scheme.rs:160-183`, scanning `bundled_themes_dir()`) and loaded by `resolve_theme_in` (`color_scheme.rs:191-237`) via an exact `HashMap` key lookup on the file stem. `TermLab Light` mirrors this exactly — a new `crates/termlab_tauri/frontend/themes/TermLab Light.toml` — no Rust struct, no new mechanism, no changes to `color_scheme.rs`'s discovery/resolution code. It is NOT wired as any default in this spec — the terminal keeps following `colors.theme` exactly as today; the follow-up terminal-themes spec introduces `auto` and consumes this palette. (Deliberate: keeps this branch's terminal behavior at zero net change.)

### Native chrome

Already keyed off `AppearanceMode` — no change; verify the settings/chooser windows get `.theme()` consistently (chooser does, `chooser_window.rs:626`).

## Non-Goals

- No `auto` terminal-theme behavior, no theme picker changes, no themes directory (next spec).
- No redesign of light colors beyond what the source theme JSON says (taste fixes go through the JVM theme JSON + regeneration).
- No per-window appearance.

## Constraints

- Branch `feat/termlab-light` from the post-skins-removal main; CLAUDE.md rules (no main commits, no trailers).
- Dark-mode byte-identity: with appearance Dark, computed styles match today's (the attribute is simply absent — assert no CSS rule outside `tokens-light.css` keys on the attribute).
- Tokens-only CSS; boundary + parity tests green; 31-suite baseline + 645 cargo baseline hold.
- `applyThemeCss`'s 7 terminal-derived vars (`--tl-terminal-bg`, six ANSI accents) stay terminal-owned — light app chrome must not repaint them.

## Testing

- Unit (frontend): `appearance.js` — mode→attribute mapping incl. System via a stubbed matchMedia, change-listener re-application, and removal on Dark. `tl-icon.js` variant selection under both attribute states. Editor `isDarkTheme()` against light `--tl-bg`.
- Unit (Rust): built-in palette resolves by name via `resolve_theme("TermLab Light")`; snapshot of its 16+4 colors.
- Extractor golden: extend `test_extract_tokens.py` with a light-source assertion; parity test unchanged (it is the gate).
- Manual checklist section J: full surface walk in Light (chrome, tool windows, settings window, chooser window, editor incl. syntax colors, toasts, context menus), System-mode live OS flip, dark unchanged-spot-check. Added — `docs/superpowers/notes/light-editor-manual-checklist.md`, steps 108-122; also carries the FG_MUTED contrast judgment call from Task 2, an icon-legibility pass now that the plain (non-`_dark`) variants render for the first time, a System-flip check with a chooser window open alongside its parent, and an explicit terminal-unchanged confirmation guarding this spec's Goal 4.

# TermLab Light Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make Settings → Appearance's Dark/Light/System genuinely restyle the whole app by wiring `AppearanceMode` to the `data-tl-appearance` attribute, completing the light token set, and shipping the "TermLab Light" built-in terminal palette.

**Architecture:** One frontend module (`app/core/appearance.js`) owns the attribute; every existing consumer (tokens-light.css, tl-icon variants, the editor's luminance inference) activates for free. Tokens regenerate from the JVM-repo source theme through the existing extractor. The light terminal palette is a hand-authored built-in beside the existing ones — wired to nothing yet (the terminal-themes plan consumes it).

**Tech Stack:** Vanilla IIFE frontend + matchMedia; python token extractor (`scripts/extract_intellij_tokens.py`); Rust `color_scheme.rs` built-ins.

**Spec:** `docs/superpowers/specs/2026-08-19-termlab-light-design.md`

## Global Constraints

- Branch `feat/termlab-light`; never commit to main; NO Co-Authored-By trailers; imperative commit messages (CLAUDE.md).
- Dark byte-identity: with appearance Dark the attribute is ABSENT and no stylesheet outside `tokens-light.css` keys on `data-tl-appearance` (grep-assert in Task 4).
- `applyThemeCss`'s 7 terminal-derived vars stay terminal-owned — never set from tokens.
- Tokens-only CSS; boundary script's only allowed failure `tl-dialog.js:334`; token parity test green; baselines: 31 frontend suites, 645 cargo.
- Generated files (`tokens-*.css`) are never hand-edited — gaps are fixed in the SOURCE theme JSON in the JVM repo (`../TermLab/core/resources/themes/TermLabLight.theme.json`) and regenerated. JVM-repo edits are reported, not committed there (separate repo, user's call).

## File Structure

- `frontend/app/core/appearance.js` — NEW: sole owner of `data-tl-appearance` + System matchMedia tracking.
- `frontend/index.html`, `settings.html`, `chooser.html` — boot wiring; `frontend/app/config-runtime.js` — re-apply on `config-changed`.
- `../TermLab/core/resources/themes/TermLabLight.theme.json` — SOURCE fixes (other repo, report-only).
- `frontend/styles/design-system/tokens-light.css` — regenerated output.
- `crates/termlab_core/src/color_scheme.rs` — the built-in light palette.
- `scripts/tests/test_extract_tokens.py`, `scripts/tests/test_appearance.mjs` (NEW), checklist note.

---

### Task 1: The appearance owner

**Files:**
- Create: `crates/termlab_tauri/frontend/app/core/appearance.js`
- Modify: `crates/termlab_tauri/frontend/index.html`, `settings.html`, `chooser.html` (script tag + boot call), `crates/termlab_tauri/frontend/app/config-runtime.js` (re-apply in `applyConfigChanged`)
- Test: Create `scripts/tests/test_appearance.mjs`

**Interfaces:**
- Produces: `global.termlabAppearance = { apply(mode), current() }`. `apply('dark')` removes the attribute; `apply('light')` sets `data-tl-appearance="light"`; `apply('system')` resolves via `matchMedia('(prefers-color-scheme: dark)')` AND registers exactly one change listener that re-resolves on OS flips (re-`apply` with another mode unregisters it — one listener max, idempotent apply). `current()` returns `'dark'|'light'` (the RESOLVED value). Injected document/matchMedia for tests: `apply(mode, { doc, matchMedia })` optional deps defaulting to globals.
- Consumes: `appCfg.colors.appearance_mode` — verify the exact field path the frontend receives from `get_app_config` (scout confirmed `appCfg.appearance_mode` at `terminal-runtime.js:424-438` — trace which shape it is and cite; the serialized name may be snake_case at a different nesting).

- [ ] **Step 1: Write failing tests** in `test_appearance.mjs` (vm-sandbox harness like siblings): dark→attribute absent; light→attribute `"light"`; system+stub matchMedia(dark)→absent; system+stub(light)→set; OS flip via stub's change listener re-resolves; switching system→dark removes the listener (stub counts removals); double-apply registers one listener. Also: `tl-icon.js` loaded into the same sandbox picks the light-variant name when the attribute is set (read tl-icon's actual variant logic first — the redesign noted the `_dark` suffix naming; assert on whatever the real mapping is, cite lines). Also: `features/editor/theme.js` loaded with a stubbed `getComputedStyle` returning the light `--tl-bg` (`#E3E8EF`) reports non-dark through its luminance path, and dark `--tl-bg` reports dark (read `isDarkTheme`'s actual access pattern first and stub exactly that).
- [ ] **Step 2: Run — fails (module absent).** `node scripts/tests/test_appearance.mjs`
- [ ] **Step 3: Implement `appearance.js`** per the interface. Boot wiring: in each HTML entry's boot script, after the config fetch that already happens there, call `termlabAppearance.apply(<mode from config>)`; in `applyConfigChanged` (`config-runtime.js`), re-apply BEFORE `applyThemeCss` so token switch and terminal accents land in one frame. Script-tag ordering: classic script before dependents (the module-defer trap from the chooser task — appearance.js before config-runtime.js in index.html, before the inline boots elsewhere).
- [ ] **Step 4: Full frontend suite green (now 32 suites).** `node --check` touched files.
- [ ] **Step 5: Commit** — `git commit -m "Add the appearance owner that finally sets data-tl-appearance"`

### Task 2: Complete the light tokens from the source theme

**Files:**
- Modify: `../TermLab/core/resources/themes/TermLabLight.theme.json` (source fixes — NOT committed there; changes reported), `crates/termlab_tauri/frontend/styles/design-system/tokens-light.css` (regenerated only), `scripts/tests/test_extract_tokens.py` (light golden assertions)
- Test: `python3 scripts/tests/test_extract_tokens.py`, `node scripts/tests/test_token_parity.mjs`

**Interfaces:** none produced; consumes the extractor CLI (`python3 scripts/extract_intellij_tokens.py --termlab-repo ../TermLab --out-dir crates/termlab_tauri/frontend`).

- [ ] **Step 1:** Run the parity test and the extractor dry (regenerate, `git diff` tokens-light.css) — enumerate every token in `tokens-dark.css` missing or visually-suspect in light output. The JVM source's `ui` map is the fix surface: add missing component entries mirroring the dark JSON's key set (open `TermLabDark.theme.json` beside it — every `ui.*` key it defines that the light one lacks is a candidate; add with sensible light values derived from the light palette block). NOTE the source file already has uncommitted modifications in the JVM repo (from the token-gap session) — build on them, do not revert.
- [ ] **Step 2:** Extend `test_extract_tokens.py` with a light-source golden: a minimal light theme JSON fixture → assert exact `--tl-base-background: #E3E8EF;`-style outputs and the `:root[data-tl-appearance="light"]` wrapper.
- [ ] **Step 3:** Regenerate; parity test green; full frontend suite green (tokens-light.css is load-bearing for nothing until Task 1's attribute, but parity + boundary still gate).
- [ ] **Step 4: Commit** (conch side only) — `git commit -m "Regenerate complete light tokens from the light source theme"` — the commit message body lists the JVM-repo source edits verbatim for the user to commit over there.

### Task 3: The TermLab Light built-in terminal palette

**Files:**
- Modify: `crates/termlab_core/src/color_scheme.rs` (add built-in beside existing ones — read how "dracula"/"TermLab Dark" are registered first and mirror exactly)
- Test: `#[cfg(test)]` beside the existing color_scheme tests

**Interfaces:**
- Produces: `resolve_theme("TermLab Light")` returns the palette (the terminal-themes plan consumes this exact name).

Palette (seeded from the light source theme; hand-tuned ANSI legible on `#E3E8EF`):
background `#E3E8EF`, foreground `#1F2933`, cursor `#1F2933`/text `#E3E8EF`, selection bg `#CAD4E2`/fg `#1F2933`;
normal: black `#1F2933`, red `#B3261E`, green `#1E7B34`, yellow `#9A6700`, blue `#1D4ED8`, magenta `#8E24AA`, cyan `#0E7490`, white `#D5DBE3`;
bright: black `#52606D`, red `#D93025`, green `#2E9E4C`, yellow `#B8860B`, blue `#3B82F6`, magenta `#AB47BC`, cyan `#0891B2`, white `#F4F7FA`.

- [ ] **Step 1:** Failing tests: `resolve_theme("TermLab Light")` resolves; snapshot every field of the resolved palette (exact hex per the table above); the existing built-in list/enumeration includes it exactly once.
- [ ] **Step 2:** Run — fail. **Step 3:** Implement. **Step 4:** `cargo test -p termlab_core` green, full workspace green. **Step 5: Commit** — `git commit -m "Add the TermLab Light built-in terminal palette"`

### Task 4: Integration sweep, checklist section J, spec sync

**Files:**
- Modify: `docs/superpowers/notes/light-editor-manual-checklist.md` (section J after section I's last step), spec (sync), anything the sweep finds
- Test: full suites both stacks

- [ ] **Step 1: Sweep.** Grep-assert dark byte-identity: `grep -rn "data-tl-appearance" crates/termlab_tauri/frontend --include="*.css"` hits ONLY `tokens-light.css`; JS hits only `appearance.js` + `tl-icon.js`. Full frontend suite (32), cargo (645 + Task 3's), boundary, parity, `test_extract_tokens.py`.
- [ ] **Step 2: Checklist section J** (numbering continues after section I's step 107): Light walk of every surface — titlebar/tool strips, Hosts/Tunnels/Notifications, SFTP pane, settings modal AND settings window, chooser window (both modes), editor with syntax highlighting on a real file, toasts, context menus, command palette; System-mode live OS flip both directions with app open; Dark spot-check (unchanged); icon legibility in light (the vendored variant question); **judgment call-outs** for any token that parity passes but taste fails — fix loops go through the SOURCE theme JSON.
- [ ] **Step 3: Spec sync** against HEAD (incl. the actual appearance-field path Task 1 traced). Plan superseded-notes if execution diverged.
- [ ] **Step 4: Commit** — `git commit -m "Add TermLab Light manual checklist section and sync the spec"`

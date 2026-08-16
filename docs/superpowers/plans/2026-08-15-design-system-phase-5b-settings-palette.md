# Design System Phase 5b: Settings and Command Palette — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Rebuild the Settings window and the command palette on the design system, migrate the last dialog holdout, and delete what remains of the legacy dialog stylesheet.

**Architecture:** Settings moves onto `tl-dialog` and the shared controls, gaining the reference's sidebar tree / breadcrumb / footer layout. The command palette becomes IntelliJ's Search Everywhere. The plugin-permissions dialog — the last `.ssh-overlay` producer besides those two — moves too, which finally lets `styles/dialogs.css` and `dialog-runtime.js`'s MutationObserver go.

**Tech Stack:** Vanilla IIFE modules + CSS custom properties; no build step; no Rust changes.

## Global Constraints

- Spec: `docs/superpowers/specs/2026-08-14-termlab-design-system-design.md`. **Measured targets: `docs/superpowers/specs/assets/reference/METRICS.md`, "Settings window" section, with the capture `assets/reference/jvm-termlab-settings.png`.** Cite them; never eyeball.
- Measured values that bind this phase: sidebar/content divider at **~20% of window width**; sidebar selected row fill **`#627190`**, height **24px**; checkbox **14px square**, 1px `#424854` border, `#292C33` fill, tick in the **foreground** colour (already implemented as `.tl-check`); primary button fill **`#6F7F9E`** with a **white** label (already `.tl-btn--primary`); panel background `#22252A`; section headers are plain-case labels followed by a 1px rule to the right edge; footer has `?` bottom-left and `Cancel` / `Apply` (disabled until dirty) / `OK` bottom-right.
- **Two measurement rules, learned the hard way (both in METRICS.md):** capture the reference app **focused**, and put **both windows on the same display** before comparing colours.
- Build on the existing shared components — never new bespoke CSS: `tl-dialog` (`app/ui/tl-dialog.js`), `tl-menu`, `tl-combo`, `tl-spinner`, `.tl-check`, `.tl-radio`, `.tl-field`/`.tl-field__label`, `.tl-details`, `.tl-btn`/`.tl-btn--primary`/`.is-danger`, `.tl-input`, `.tl-empty-state`, `.tl-scroll`. Menus opened inside a dialog already register Escape at 230 so they close before the dialog (see `tl-menu.js` `escapePriority()`).
- Tokens only in CSS (`var(--tl-*)`); hex-in-`var()`-fallback is allowed **only** in `styles/design-system/base.css`.
- Vanilla IIFE modules; script/link tags in `index.html` **and** `settings.html`; commands/events via `window.termlabTauriClient`; keyboard via `window.termlabKeyboardRouter`.
- `rg` is not installed — verify with `grep`. `node --check` every touched JS. Suites: `cargo test --workspace` (505 expected), `node scripts/tests/test_tl_icon.mjs`, `test_tl_dialog.mjs`, `test_tl_combo.mjs`, `python3 scripts/tests/test_extract_tokens.py`.
- **Do not run `screencapture`** — the controller performs all visual measurement, and will ask the user to open surfaces since clicks cannot be automated.
- Another session owns `crates/termlab_tauri/src/platform.rs` and `src/main.rs` — do not touch them.
- **Skins**: `styles/skins.css` re-skins the legacy settings/palette classes and has no `tl-*` rules; converting these surfaces makes them render unskinned under metal/win95 etc. That is accepted and is Phase 6's job — list what goes dead in your report, leave `skins.css` untouched.
- Verified anchors. **Settings**: `app/panels/settings.js` — `open` :152, `openInWindow` :167, `renderStandalone` :183, `renderDialog` :211, `close` :242, `selectSection` :274, `renderCurrentSection` :281, Escape registrations at :196-205 (standalone) and :225-235 (modal, priority 210). `app/features/settings/renderers.js` — `addSectionLabel` :11-16, `addDivider` :18-22, `addRow` :24-44, `addSearchInput` :65-77, `makeInput` :81-92, `makeToggleGroup` :94-126, `makeSwitch` :128-140, `buildThemePreview` :160-261, `updateThemePreview` :263-337, `createShortcutRecorder` :383-454, `renderDialogShell` :513-593, `renderStandaloneShell` :599-662, `createSectionRenderers` :673-945, `renderSection` :924-942. `app/features/settings/sidebar.js` — `attachActivatableItem` :4-14, `renderSidebarInto` :16-227. `app/features/settings/constants.js` — section defs :5-20 (4 groups, 8 sections), searchable index :24-55. Settings `<select>` sites: `sections-appearance.js` :38 theme, :55 skin, :178 window decorations, :212 UI font; `sections-terminal.js` :22 terminal font. `settings.html` is 129 lines (custom titlebar :106-120, Windows/Linux only); `styles/settings-window.css` is 91 lines (2 raw hex); settings CSS blocks live at `styles/dialogs.css` :282-505. **Palette**: `app/command-palette-runtime.js` — `MAX_QUICK_RESULTS` :12, cache TTL :13, `fuzzyScore` :46-61, `quickPickIndexFromKey` :236-241, `renderPaletteResults` :243-276, `openCommandPalette` :309-422 (overlay :312-313), `onKeyDown` :342-387 (router priority 260); CSS at `styles/dialogs.css` :67-132. **Last legacy overlay producers**: `command-palette-runtime.js:313`, `core/dialog-service.js:49` (`confirmPluginPermissions` :108-145, consumers `command-palette-runtime.js:81` and `features/settings/actions.js:21`), `features/settings/renderers.js:517`. `app/dialog-runtime.js` :31-38 / :60-64 keeps a `.ssh-overlay` MutationObserver solely for those three.

## A product decision this phase forces — SETTLED

Our settings use **iOS-style switches** (`makeSwitch`, 11 uses) for booleans; the reference uses **checkboxes** everywhere and has no switch control at all. **Decision (user, 2026-08-15): convert every switch to `.tl-check`.** `makeSwitch` and the `.tl-switch` CSS are deleted rather than restyled — no switch control survives this phase. Toggle groups (`makeToggleGroup`, 5 uses) have no reference equivalent either, but they encode a 3-way choice rather than a boolean, so they stay, restyled on tokens.

---

### Task 1: Settings shell — dialog, sidebar, breadcrumb, footer

**Files:**
- Modify: `crates/termlab_tauri/frontend/app/features/settings/renderers.js` (`renderDialogShell` :513-593, `renderStandaloneShell` :599-662)
- Modify: `crates/termlab_tauri/frontend/app/panels/settings.js` (:196-205, :211, :225-235, :242)
- Modify: `crates/termlab_tauri/frontend/app/features/settings/sidebar.js`
- Create: `crates/termlab_tauri/frontend/styles/design-system/components/settings.css`
- Modify: `crates/termlab_tauri/frontend/settings.html`, `index.html`

**Interfaces:**
- Produces: the modal shell rendered by `tlDialog.open({ size: 'lg' })` with a body split into `.tl-settings__sidebar` + `.tl-settings__content`, and a footer carrying `?` / `Cancel` / `Apply` / `OK`. The standalone window keeps its own shell (no overlay) but shares every inner class.
- Produces classes: `.tl-settings`, `.tl-settings__sidebar`, `.tl-settings__search`, `.tl-settings__group`, `.tl-settings__item` (`.is-active`), `.tl-settings__content`, `.tl-settings__breadcrumb`, `.tl-settings__section`, `.tl-settings__rule`, `.tl-settings__row`, `.tl-settings__row-label`, `.tl-settings__row-desc`, `.tl-settings__link`.

- [ ] **Step 1: Move the modal onto tl-dialog**

Replace `renderDialogShell`'s hand-built `.ssh-overlay#settings-overlay` + `.ssh-form.settings-dialog` with `tlDialog.open({ title: 'Settings', size: 'lg', body, buttons })`. Buttons, in the reference's order: `Cancel` (secondary), `Apply` (secondary, **disabled until there are pending changes** — wire it to the existing dirty state in `features/settings/store.js`), `OK` (primary). The `?` help button sits bottom-left; the shell's footer is right-aligned, so add a `.tl-dialog__footer` left-slot for it in `components/dialog.css` (a small, general addition — the reference's dialogs use it too) rather than a settings-only hack. Delete **only** the modal's priority-210 Escape registration at `settings.js` :225-235 — `tl-dialog` owns Escape for that path. The standalone window's registration at :196-205 **stays**: that window has no dialog shell, so nothing else would close it. Keep `openInWindow`'s standalone path working: it has no overlay, so it keeps its own shell but must use the same inner markup and classes.

- [ ] **Step 2: Sidebar per the measurement**

`sidebar.js` keeps its data flow (groups, fuzzy search, keyboard activation) but renders to the new classes. Measured: sidebar occupies **~20% of the width**; the selected row is filled **`#627190`** and is **24px** tall. Add a semantic alias in `base.css` (e.g. `--tl-settings-selection: var(--tl-List-selectionBackground, #627190)` — check first whether a generated token already carries that value; grep `tokens-dark.css` for `627190` and prefer the real token) and consume it from `settings.css`. Rows keep `role="button"`/`tabindex=0` but should become real `<button>` elements unless that breaks the existing keyboard handling — say which you did. Group headers and the disclosure arrows use the vendored `chevronRight`/`chevronDown` icons via `window.tlIcon`.

- [ ] **Step 3: Content header and section rules**

Add the breadcrumb (`<group> › <section>`) above the content, matching the reference. `addSectionLabel` (:11-16) and `addDivider` (:18-22) become one construct: a plain-case label followed by a 1px rule running to the right edge (the reference draws the rule *beside* the label, not under it — check `jvm-termlab-settings.png` and match what you see). Helper text under a row is muted; inline links (`Reset to default`, `How it works`) use the accent colour.

- [ ] **Step 4: Verify + commit**

```bash
node --check crates/termlab_tauri/frontend/app/features/settings/renderers.js crates/termlab_tauri/frontend/app/features/settings/sidebar.js crates/termlab_tauri/frontend/app/panels/settings.js
grep -n "#[0-9a-fA-F]\{3,8\}" crates/termlab_tauri/frontend/styles/design-system/components/settings.css   # expect empty
cargo build -p termlab_tauri && (RUST_LOG=info ./target/debug/termlab > /tmp/tl-p5b-t1.log 2>&1 &) && sleep 8 && pkill -f target/debug/termlab; grep -ci "error\|panic" /tmp/tl-p5b-t1.log
git add -A crates/termlab_tauri/frontend
git commit -m "feat(design-system): settings shell on tl-dialog with reference sidebar and footer"
```

Both entry points must still work: the in-app dialog (`open`) and the standalone window (`openInWindow`).

---

### Task 2: Settings controls and sections

**Files:**
- Modify: `app/features/settings/renderers.js` (control factories + section renderers)
- Modify: `app/features/settings/sections-{appearance,basic,keyboard,terminal}.js`, `plugins-section.js`
- Modify: `styles/design-system/components/settings.css`, `form.css` (only if a control genuinely needs a new shared rule)

**Interfaces:** consumes Task 1's classes and the Phase-5a controls.

- [ ] **Step 1: Migrate the control factories**

`makeInput` (:81-92) → `.tl-input`; every `<select>` (5 sites listed in the anchors) → `tlCombo.attach`; numeric inputs → `tlSpinner.attach`; `addRow` (:24-44) → `.tl-settings__row` with the label column matching the reference. `makeSwitch` (:128-140, 11 uses) → `.tl-check`, per the settled decision above; delete `makeSwitch` and every `.tl-switch` rule once the last caller is converted. `makeToggleGroup` (:94-126) keeps its 3-way semantics, restyled on tokens.

- [ ] **Step 2: The special controls**

`createShortcutRecorder` (:383-454) keeps its recording behaviour; restyle `.settings-shortcut-key` (currently uses a raw `#7aa2f7`) onto tokens. `buildThemePreview`/`updateThemePreview` (:160-337) applies colours through inline styles by design (it previews arbitrary terminal palettes) — leave that mechanism, but move its chrome (borders, labels, container) onto tokens. The plugin rows in `plugins-section.js` keep their badges; retoken them (they carry raw hex today).

- [ ] **Step 3: Sweep raw hex out of the settings surface**

`styles/dialogs.css` :282-505 and `styles/settings-window.css` carry raw hex (`#7aa2f7` ×several, `#e81123`, `#fff`, plus two URL-encoded `%236272a4` select arrows). Everything that survives into the new settings CSS must be token-based; the arrows disappear with the native selects.

- [ ] **Step 4: Verify + commit**

```bash
node --check $(git diff --name-only -- 'crates/termlab_tauri/frontend/**/*.js' | tr '\n' ' ')
grep -rn "#[0-9a-fA-F]\{3,8\}" crates/termlab_tauri/frontend/styles/design-system/components/*.css   # expect empty
cargo build -p termlab_tauri && (RUST_LOG=info ./target/debug/termlab > /tmp/tl-p5b-t2.log 2>&1 &) && sleep 8 && pkill -f target/debug/termlab; grep -ci "error\|panic" /tmp/tl-p5b-t2.log
git add -A crates/termlab_tauri/frontend
git commit -m "feat(design-system): settings controls on the shared form components"
```

Every setting must still read and write the same value — this is a presentation change. Report anything whose behaviour genuinely could not be preserved.

---

### Task 3: Command palette as Search Everywhere

**Files:**
- Modify: `app/command-palette-runtime.js` (:243-276, :309-422)
- Create: `styles/design-system/components/palette.css`
- Modify: `index.html`

**Interfaces:** produces `.tl-palette`, `.tl-palette__input`, `.tl-palette__group`, `.tl-palette__item` (`.is-active`), `.tl-palette__icon`, `.tl-palette__title`, `.tl-palette__subtitle`, `.tl-palette__shortcut`, `.tl-palette__empty`.

- [ ] **Step 1: Capture the reference first**

The repo has no Search Everywhere capture. **Ask the controller to obtain one** (the user opens Shift-Shift in the JVM app, on the same display, focused) and add its measurements to METRICS.md before styling. If the controller reports it is unavailable, build from the tokens plus the existing palette structure and record in your report that the palette is unverified against a reference.

- [ ] **Step 2: Restyle and extend**

Keep the fuzzy scorer (:46-61), the 45s command cache (:13), the digit-1-5 quick pick (:236-241) and the arrow/enter/escape handling (:342-387). Change: render on `tl-dialog` (top-anchored — add a `--top` modifier to `dialog.css` rather than a palette-only overlay), add **grouped results with headers** and a **per-result icon** (via `window.tlIcon`), and raise the result cap above 5 with the digit shortcuts still applying to the first five. Preserve the router priority relationship: the palette currently registers at 260, above the shell's 225, so it wins Escape — keep that.

- [ ] **Step 3: Verify + commit**

```bash
node --check crates/termlab_tauri/frontend/app/command-palette-runtime.js
grep -n "#[0-9a-fA-F]\{3,8\}" crates/termlab_tauri/frontend/styles/design-system/components/palette.css   # expect empty
cargo build -p termlab_tauri && (RUST_LOG=info ./target/debug/termlab > /tmp/tl-p5b-t3.log 2>&1 &) && sleep 8 && pkill -f target/debug/termlab; grep -ci "error\|panic" /tmp/tl-p5b-t3.log
git add -A crates/termlab_tauri/frontend
git commit -m "feat(design-system): command palette as Search Everywhere"
```

---

### Task 4: The last holdout, then delete the legacy stylesheet

**Files:**
- Modify: `app/core/dialog-service.js` (`confirmPluginPermissions` :108-145; consumers `command-palette-runtime.js:81`, `features/settings/actions.js:21`)
- Modify: `app/dialog-runtime.js` (:31-38, :60-64)
- Modify/Delete: `styles/dialogs.css`, `styles/settings-window.css`
- Modify: `index.html`, `settings.html`

**Interfaces:** after this task, nothing in `app/` produces `.ssh-overlay`.

- [ ] **Step 1: Migrate the plugin-permissions dialog**

Move `confirmPluginPermissions` onto `tlDialog.open` (it returns a promise resolving to the user's choice — preserve that contract exactly for both callers). Then `dialog-service.js`'s own `open`/`detachOverlay`/`registerEscape` (:15-106) are dead: delete them if nothing else calls them (grep first), or reduce the module to just the permissions helper.

- [ ] **Step 2: Remove the MutationObserver**

With no `.ssh-overlay` producers left, `dialog-runtime.js`'s observer (:31-38, :60-64) can be replaced entirely by `tlDialog.onAllClosed()`. Verify the terminal still refocuses after every dialog closes, including the palette and settings.

- [ ] **Step 3: Delete what is dead**

Grep each remaining block in `styles/dialogs.css` and `styles/settings-window.css`; delete every selector no longer referenced from `app/` or the two HTML files. Report the before/after line counts (dialogs.css was 689 lines at the start of Phase 5, 668 after 5a) and list the `skins.css` selectors that are now dead. If the whole of `dialogs.css` goes, remove its `<link>` tags too.

- [ ] **Step 4: Verify + commit**

```bash
grep -rn "ssh-overlay\|ssh-form" crates/termlab_tauri/frontend/app crates/termlab_tauri/frontend/*.html   # expect empty
node --check $(git diff --name-only -- 'crates/termlab_tauri/frontend/**/*.js' | tr '\n' ' ')
cargo test --workspace 2>&1 | grep -cE "^test result: ok"   # expect 13
cargo build -p termlab_tauri && (RUST_LOG=info ./target/debug/termlab > /tmp/tl-p5b-t4.log 2>&1 &) && sleep 8 && pkill -f target/debug/termlab; grep -ci "error\|panic" /tmp/tl-p5b-t4.log
git add -A crates/termlab_tauri/frontend
git commit -m "refactor(design-system): migrate plugin-permissions dialog, delete legacy dialog CSS"
```

---

### Task 5: Measured verification pass (controller-run)

The controller runs this — it needs screen capture and user interaction, which subagents cannot do.

- [ ] **Step 1: Capture and compare**

With both windows on the **same display** and each app **focused** for its own capture, compare our Settings against `jvm-termlab-settings.png`: sidebar width ratio, selected-row fill `#627190` and 24px height, checkbox geometry, section label + rule, footer button order and the disabled `Apply`, primary fill `#6F7F9E` with a white label. Do the same for the palette if a reference was obtained in Task 3.

- [ ] **Step 2: Fix deviations, run regressions, commit**

```bash
cargo test --workspace 2>&1 | grep -cE "^test result: ok"
node scripts/tests/test_tl_icon.mjs && node scripts/tests/test_tl_dialog.mjs && node scripts/tests/test_tl_combo.mjs && python3 scripts/tests/test_extract_tokens.py
cp <capture> docs/superpowers/specs/assets/phase5b-settings.png
git add -A && git commit -m "feat(design-system): phase 5b verification pass"
```

---

## Phase exit criteria

- Settings (both the in-app dialog and the standalone window) and the command palette run on the design system, matching the measured reference.
- No `.ssh-overlay` producers remain; `styles/dialogs.css` is deleted or reduced to nothing the app references; `dialog-runtime.js` no longer needs a MutationObserver.
- All tests green; capture checked in. Human side-by-side is the final acceptance.
- Phase 6 inherits: porting `skins.css` onto the design system (every converted surface currently renders unskinned under the retro themes) and the residual measurement gap for the Add SSH Host dialog's field heights.

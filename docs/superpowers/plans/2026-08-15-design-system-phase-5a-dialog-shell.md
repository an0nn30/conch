# Design System Phase 5a: Dialog Shell and Form Controls — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace 26 hand-rolled dialogs with one shared dialog shell and a real set of form controls that match the reference, fixing the focus and teardown defects those dialogs carry today.

**Architecture:** A `tl-dialog` component owns the overlay, panel, focus trap, stacking and dismissal; a control layer adds checkbox, radio, combo box (a popup built on the existing `tl-menu` primitive), spinner and switch. Every dialog is then migrated onto them in two batches. Settings and the command palette are deliberately out of scope — they are Phase 5b.

**Tech Stack:** Vanilla IIFE modules + CSS custom properties; no build step; no Rust changes.

## Global Constraints

- Spec: `docs/superpowers/specs/2026-08-14-termlab-design-system-design.md`. **Measured targets: `docs/superpowers/specs/assets/reference/METRICS.md` — the "Settings window" and "Dialogs and settings" sections. Cite them; never eyeball.** Reference captures sit beside it (`jvm-termlab-settings.png` and the dialog screenshots described there).
- **Dialogs stay in-page overlays** styled to look like the reference's OS windows (user decision). Do not spawn Tauri windows.
- Measured values that matter here: checkbox **14px square**, 1px border `#424854`, fill `#292C33`, tick drawn in the **foreground** colour `#ACB2BE` (**not** accent-filled); primary button fill `#6F7F9E` with a **white** label; dialog/panel background `#22252A`; sidebar-style selection `#627190`.
- Tokens only in CSS (`var(--tl-*)`). The JVM theme defines **no** CheckBox/RadioButton/Dialog/Separator tokens, so add semantic aliases in `styles/design-system/base.css` (the only file where hex-in-`var()`-fallback is allowed) and consume those from component files.
- Vanilla IIFE modules; `<script>`/`<link>` tags in `index.html` **and** `settings.html` where the module is used; commands/events via `window.termlabTauriClient`; keyboard via `window.termlabKeyboardRouter`.
- `rg` is not installed — verify with `grep`. `node --check` every touched JS. Full suite: `cargo test --workspace` (505 expected) plus `node scripts/tests/test_tl_icon.mjs` and `python3 scripts/tests/test_extract_tokens.py`.
- **Do not run `screencapture` or any screen-capture tool** — the controller performs all visual measurement.
- Another session is editing `crates/termlab_tauri/src/platform.rs` and `src/main.rs`. Do not touch those files.
- **Skins**: `styles/skins.css` re-skins the legacy dialog classes for metal/win95 (`~:182-320`, `~:450-500+`). Renaming classes breaks those skins until Phase 6 ports them — that is accepted (same as the `tl-menu` conversion); list what goes dead in your report, and leave `skins.css` itself untouched.
- Verified anchors (from a full inventory): the single overlay primitive is `.ssh-overlay` + `.ssh-form` at `styles/dialogs.css:2-66` (`.ssh-form-title` :15-18, `-body` :19, `-row` :20, `-label` :21-42 which also styles nested `input`/`select`, `-advanced` :43-54, `-buttons` :55-58, `-btn`/`.primary` :59-66). `app/core/dialog-service.js` (151 lines): `detachOverlay` :15-26, `registerEscape` :28-44, `open` :46-106, `confirmPluginPermissions` :108-145 — its only consumers are `app/command-palette-runtime.js:81` and `app/features/settings/actions.js:21`. Dialog inventory: `features/ssh/connection-form.js` 1 (`showConnectionForm` :157), `features/ssh/dialogs.js` 3 (:4, :77, :158), `features/ssh/auth-prompts.js` 2 (:4, :69), `features/ssh/dependency-prompt.js` 1 (:4), `panels/ssh-panel.js` 1 (`exportConfig` :400), `panels/tunnel-manager.js` 5 (:61, :246, :320, :445, :562), `features/vault/dialogs.js` 2 (:4, :111), `features/vault/account-form.js` 1 (:4), `panels/vault.js` 1 (:254), `core/keygen.js` 2 (:63, :262), `panels/files-panel.js` 2 (:723, :773), `panels/plugin-widgets.js` 3 (:834, :936, :971), `dialog-runtime.js` 1 (`showAboutDialog` :56), `window-events-runtime.js` 1 (`showRestartDialog` :14). Ad-hoc z-indexes today: 3000 base, 3100 nested, 3400 palette, 4000 plugin, 4500 files, 5000 auth prompts; `tl-menu` sits at 3200. Escape priorities: 210 settings, 220 most, 230 vault, 260 palette. **Landmines:** `app/dialog-runtime.js:32-53` runs a MutationObserver counting `.ssh-overlay` nodes to refocus the terminal; `app/panels/ssh-panel.js:950` and `app/panels/tunnel-manager.js:619` both do `querySelectorAll('.ssh-overlay').forEach(el => el.remove())`, which destroys unrelated open dialogs; no dialog anywhere traps focus.

---

### Task 1: The `tl-dialog` shell

**Files:**
- Create: `crates/termlab_tauri/frontend/styles/design-system/components/dialog.css`
- Create: `crates/termlab_tauri/frontend/app/ui/tl-dialog.js`
- Create: `scripts/tests/test_tl_dialog.mjs`
- Modify: `crates/termlab_tauri/frontend/index.html`, `settings.html` (link + script tags)
- Modify: `crates/termlab_tauri/frontend/styles/design-system/base.css` (semantic aliases)

**Interfaces:**
- Produces: `window.tlDialog.open({ title, body, buttons, size, ariaLabel, onClose })` → a handle `{ el, close(result) }`; `window.tlDialog.closeTop()`; `window.tlDialog.count()`.
  - `body` accepts an element or a builder `(bodyEl) => void`.
  - `buttons`: array of `{ label, primary?, danger?, disabled?, onSelect }` rendered bottom-right in order (secondary first, primary last, matching the reference).
  - Stacking: each open dialog gets `z-index = 3000 + depth*10`; `tl-menu` must always sit above the topmost dialog — compute its z-index from `tlDialog.count()` rather than the current hardcoded 3200.
- Produces classes: `.tl-dialog__overlay`, `.tl-dialog`, `.tl-dialog__title`, `.tl-dialog__body`, `.tl-dialog__footer`, `.tl-dialog--sm|--md|--lg`.

- [ ] **Step 1: Add semantic aliases**

In `styles/design-system/base.css`, add (hex fallbacks are measured values from METRICS.md):

```css
  /* Dialog + control tokens: the JVM theme defines none of these, so the
     values below are measured from the reference app (see METRICS.md). */
  --tl-dialog-bg: var(--tl-Panel-background);
  --tl-dialog-border: var(--tl-base-borderColor);
  --tl-dialog-scrim: rgba(0, 0, 0, 0.5);
  --tl-control-border: var(--tl-Component-borderColor, #424854);
  --tl-control-bg: var(--tl-TextField-background);
  --tl-control-tick: var(--tl-base-foreground);
  --tl-primary-fg: #ffffff;
  --tl-z-dialog: 3000;
```

- [ ] **Step 2: Write dialog.css**

```css
.tl-dialog__overlay {
  position: fixed; inset: 0;
  display: flex; align-items: center; justify-content: center;
  background: var(--tl-dialog-scrim);
}
.tl-dialog {
  display: flex; flex-direction: column;
  max-height: 84vh;
  background: var(--tl-dialog-bg);
  border: 1px solid var(--tl-dialog-border);
  border-radius: var(--tl-radius);
  box-shadow: 0 8px 32px rgba(0, 0, 0, 0.5);
}
.tl-dialog--sm { width: 380px; }
.tl-dialog--md { width: 520px; }
.tl-dialog--lg { width: 720px; }
.tl-dialog__title {
  flex-shrink: 0;
  text-align: center;
  font: 600 var(--tl-font-size-ui) var(--tl-font-ui);
  color: var(--tl-fg);
  padding: var(--tl-space-3) var(--tl-space-4);
}
.tl-dialog__body {
  flex: 1; min-height: 0; overflow-y: auto;
  padding: 0 var(--tl-space-4) var(--tl-space-3);
}
.tl-dialog__footer {
  flex-shrink: 0;
  display: flex; justify-content: flex-end; gap: var(--tl-space-2);
  padding: var(--tl-space-3) var(--tl-space-4);
}
```

The reference centres the dialog title (it is a native window title bar); `--sm/--md/--lg` replace the ad-hoc `width: 320px/460px` in `dialogs.css`.

- [ ] **Step 3: Write tl-dialog.js**

Implement, following `app/ui/tl-menu.js`'s conventions (IIFE, single global, router registration):

- A module-level `stack` of open dialogs. `open()` pushes; `close()` pops and removes only its own overlay — never a global `querySelectorAll` sweep.
- **Focus trap**: on open, remember `document.activeElement`; focus the first focusable element in the panel (or the panel itself with `tabindex="-1"`); register a capture-phase `keydown` on the overlay that intercepts `Tab`/`Shift+Tab` and cycles within the panel's focusable set (`a[href], button, input, select, textarea, [tabindex]:not([tabindex="-1"])`, filtered for `:not(:disabled)` and non-zero size). On close, restore focus to the remembered element when it is still `isConnected`.
- **Escape**: register through `window.termlabKeyboardRouter` with a priority above the dialogs it replaces, and `isActive` true only for the topmost dialog (see `dialog-service.js:28-44` for the existing pattern to follow).
- **Backdrop dismiss**: `mousedown` where `event.target === overlay`, matching today's behaviour.
- **ARIA**: `role="dialog"`, `aria-modal="true"`, `aria-label` from `title`/`ariaLabel`; mark the rest of the app `aria-hidden` while a dialog is open (and restore it), since a real modal should hide the background from assistive tech.
- Expose `count()` so `tl-menu.js` can compute a z-index above the topmost dialog; update `tl-menu.js` accordingly (its current hardcoded 3200 sits *below* the 4000/4500/5000 dialogs, so menus opened from those dialogs render behind them today — fix that as part of this task and say so in your report).

- [ ] **Step 4: Write the test**

`scripts/tests/test_tl_dialog.mjs`, following the shape of `scripts/tests/test_tl_icon.mjs` (a `window`/`document` shim, no jsdom available). Cover the pure logic that does not need a real layout engine:
- `z-index` for depth 0/1/2 is 3000/3010/3020.
- The focusable-candidate filter excludes `disabled` and `tabindex="-1"` elements and includes the standard set.
- `close()` removes only the top entry and leaves lower dialogs in the stack.
Stub the DOM pieces you need (`createElement` returning objects with `appendChild`/`classList`/`setAttribute`), the same way the icon test stubs `document`.

Run: `node scripts/tests/test_tl_dialog.mjs` — expect `ok`.

- [ ] **Step 5: Wire and verify**

Add the `<link>` for `dialog.css` after `menu.css` and the `<script>` for `tl-dialog.js` next to `tl-menu.js` in **both** `index.html` and `settings.html`.

```bash
node --check crates/termlab_tauri/frontend/app/ui/tl-dialog.js
node scripts/tests/test_tl_dialog.mjs
grep -n "#[0-9a-fA-F]\{3,8\}" crates/termlab_tauri/frontend/styles/design-system/components/dialog.css   # expect empty
cargo build -p termlab_tauri && (RUST_LOG=info ./target/debug/termlab > /tmp/tl-p5a-t1.log 2>&1 &) && sleep 8 && pkill -f target/debug/termlab; grep -ci "error\|panic" /tmp/tl-p5a-t1.log
```

Nothing consumes the shell yet, so the app must look and behave exactly as before.

- [ ] **Step 6: Commit**

```bash
git add -A crates/termlab_tauri/frontend scripts/tests
git commit -m "feat(design-system): tl-dialog shell with focus trap and stacking"
```

---

### Task 2: Form controls

**Files:**
- Create: `crates/termlab_tauri/frontend/styles/design-system/components/form.css`
- Create: `crates/termlab_tauri/frontend/app/ui/tl-combo.js`
- Modify: `crates/termlab_tauri/frontend/index.html`, `settings.html`
- Modify: `crates/termlab_tauri/frontend/app/ui/tl-menu.js` (arrow-key navigation)

**Interfaces:**
- Consumes: Task 1's tokens; `tl-menu` for the combo popup.
- Produces: classes `.tl-check`, `.tl-radio`, `.tl-field`, `.tl-field__label`, `.tl-spinner`, `.tl-switch`; and `window.tlCombo.attach(selectEl)` which hides a native `<select>` and renders an IntelliJ-style button + popup driven by the select's own options, keeping the `<select>` as the source of truth so existing `change` handlers keep working.

- [ ] **Step 1: Add arrow-key navigation to tl-menu**

`app/ui/tl-menu.js` currently supports Tab/Enter/Space only. Add `ArrowDown`/`ArrowUp` to move between enabled items (wrapping), `Home`/`End` to jump, and keep Escape. This is needed for combo popups and is a shared-component improvement — do not fork the module.

- [ ] **Step 2: Write form.css**

Measured values (METRICS.md): checkbox is a **14px square**, 1px `--tl-control-border`, fill `--tl-control-bg`, tick in `--tl-control-tick` — **no accent fill**. Radios are the same but circular. Labels sit to the right with `--tl-space-2` gap; the two-column checkbox layout in the reference is the caller's job, not the component's.

```css
.tl-check, .tl-radio {
  display: inline-flex; align-items: center; gap: var(--tl-space-2);
  font: 400 var(--tl-font-size-ui) var(--tl-font-ui);
  color: var(--tl-fg);
  cursor: pointer;
}
.tl-check input, .tl-radio input {
  appearance: none;
  width: 14px; height: 14px; margin: 0;
  flex-shrink: 0;
  background: var(--tl-control-bg);
  border: 1px solid var(--tl-control-border);
  border-radius: var(--tl-radius);
}
.tl-radio input { border-radius: 50%; }
.tl-check input:checked, .tl-radio input:checked { border-color: var(--tl-accent); }
.tl-check input:checked::after {
  content: ''; display: block;
  width: 4px; height: 8px; margin: 0 auto;
  border: solid var(--tl-control-tick);
  border-width: 0 2px 2px 0;
  transform: rotate(45deg) translate(-1px, -1px);
}
.tl-radio input:checked::after {
  content: ''; display: block;
  width: 6px; height: 6px; margin: 3px auto;
  border-radius: 50%;
  background: var(--tl-control-tick);
}
.tl-check input:focus-visible, .tl-radio input:focus-visible {
  outline: 1px solid var(--tl-accent); outline-offset: 1px;
}
.tl-field { display: flex; align-items: center; gap: var(--tl-space-3); margin-bottom: var(--tl-space-2); }
.tl-field__label { flex: 0 0 auto; min-width: 120px; text-align: right; color: var(--tl-fg); font: 400 var(--tl-font-size-ui) var(--tl-font-ui); }
.tl-field > .tl-input, .tl-field > .tl-combo { flex: 1 1 auto; min-width: 0; }
```

The reference's two-column form (right-aligned label, control filling the rest) is `.tl-field`; use it for every migrated dialog form row.

- [ ] **Step 3: Write tl-combo.js**

`attach(selectEl)` should: hide the native select (`display: none`, keep it in the DOM), insert a `button.tl-combo` showing the selected option's text plus a chevron icon (`window.tlIcon.create('chevronDown', {size: 16})`), and on click open a `tlMenu` anchored to the button's rect with one item per option — the current option marked `checked`. Selecting an item sets `selectEl.value` and dispatches `new Event('change', { bubbles: true })` so existing handlers fire unchanged. Re-read the options each time the popup opens (several selects are repopulated at runtime — see `populateAccountPicker` in `features/ssh/connection-form.js:4-39`). Keep the button's `disabled` state in sync with the select's.

Style `.tl-combo` in `form.css` to match the reference: full width of its field, panel-coloured button with a 1px border, label left, chevron right, accent border on focus.

- [ ] **Step 4: Verify and commit**

```bash
node --check crates/termlab_tauri/frontend/app/ui/tl-combo.js crates/termlab_tauri/frontend/app/ui/tl-menu.js
grep -n "#[0-9a-fA-F]\{3,8\}" crates/termlab_tauri/frontend/styles/design-system/components/form.css   # expect empty
cargo build -p termlab_tauri && (RUST_LOG=info ./target/debug/termlab > /tmp/tl-p5a-t2.log 2>&1 &) && sleep 8 && pkill -f target/debug/termlab; grep -ci "error\|panic" /tmp/tl-p5a-t2.log
git add -A crates/termlab_tauri/frontend
git commit -m "feat(design-system): checkbox, radio, field and combo-box controls"
```

---

### Task 3: Migrate batch A — SSH, keygen, files, plugins, about, restart

**Files:**
- Modify: `app/features/ssh/{connection-form,dialogs,auth-prompts,dependency-prompt}.js`, `app/panels/ssh-panel.js` (`exportConfig` :400, `removeOverlay` :950)
- Modify: `app/core/keygen.js`, `app/panels/files-panel.js` (:723, :773, `removeFilesOverlay` :698)
- Modify: `app/panels/plugin-widgets.js` (:834, :936, :971)
- Modify: `app/dialog-runtime.js` (:56 About, and the MutationObserver at :32-53)
- Modify: `app/window-events-runtime.js` (:14)

**Interfaces:** consumes Tasks 1-2; produces no new API.

- [ ] **Step 1: Migrate, dialog by dialog**

For each: replace the hand-built overlay/panel with `tlDialog.open({...})`, move form rows onto `.tl-field`, inputs onto `.tl-input`, buttons into the shell's `buttons` array (`.ssh-form-btn.primary` → `{ primary: true }`; the inline `style="background:var(--red);border-color:var(--red)"` danger buttons at `features/ssh/dialogs.js:182`, `tunnel-manager.js:259` and `files-panel.js:783` → `{ danger: true }`), checkboxes onto `.tl-check`, radios onto `.tl-radio`, and each native `<select>` through `tlCombo.attach`. Delete each file's bespoke Escape/teardown helper — the shell owns both. Where a dialog maps Enter to its primary action (`features/ssh/dialogs.js:58-68`, `:139-149`, `auth-prompts.js` host-key, `files-panel.js` text prompt), keep that behaviour by wiring it in the body's `keydown`.

**Preserve behaviour exactly**: every field, default value, validation, and callback. This is a presentation change. Call out in your report any place where the old markup and the new shell genuinely cannot behave identically.

- [ ] **Step 2: Fix the two landmines**

- `app/panels/ssh-panel.js:950` and `app/panels/tunnel-manager.js:619` (`removeOverlay`) currently remove **all** `.ssh-overlay` nodes. After migration they must close only their own dialog — use the handle returned by `tlDialog.open` (tunnel-manager is batch B, so in this task fix ssh-panel and leave a note).
- `app/dialog-runtime.js:32-53` observes `.ssh-overlay` node counts to refocus the terminal. Repoint it at the shell (prefer an explicit callback from `tl-dialog` on "last dialog closed" over a MutationObserver; if you keep the observer, watch the new class).

- [ ] **Step 3: Verify and commit**

```bash
node --check $(git diff --name-only -- 'crates/termlab_tauri/frontend/**/*.js' | tr '\n' ' ')
grep -rn "ssh-overlay" crates/termlab_tauri/frontend/app | grep -v tunnel-manager   # expect only batch-B leftovers
cargo build -p termlab_tauri && (RUST_LOG=info ./target/debug/termlab > /tmp/tl-p5a-t3.log 2>&1 &) && sleep 8 && pkill -f target/debug/termlab; grep -ci "error\|panic" /tmp/tl-p5a-t3.log
git add -A crates/termlab_tauri/frontend
git commit -m "refactor(design-system): migrate ssh, keygen, files and plugin dialogs to tl-dialog"
```

---

### Task 4: Migrate batch B — vault and tunnels, then delete the legacy CSS

**Files:**
- Modify: `app/features/vault/{dialogs,account-form}.js`, `app/panels/vault.js` (:254)
- Modify: `app/panels/tunnel-manager.js` (5 dialogs; `removeOverlay` :619)
- Modify: `styles/dialogs.css` (delete the dead legacy blocks)

**Interfaces:** consumes Tasks 1-2.

- [ ] **Step 1: Migrate the vault and tunnel dialogs**

Same rules as Task 3. These are the most complex: `panels/vault.js:254` is a full list UI with a sidebar, and `tunnel-manager.js:61` renders a table. Keep their internal layout markup — only the shell, buttons, fields and controls change. The vault dialogs currently register Escape at priority 230 and use a fixed `id="vault-overlay"`; both go away with the shell.

- [ ] **Step 2: Delete dead legacy CSS**

Once nothing references them, remove from `styles/dialogs.css`: `.ssh-overlay`, `.ssh-form*` (title/body/row/label/buttons/btn/small/advanced), and any other block whose selectors no longer appear in `app/`. **Grep before each deletion.** Keep everything still in use by Phase 5b (settings and command palette classes) — those are `.settings-*`, `.command-palette*`, and `.plugin-permissions-*` if `dialog-service` still renders it. Report the line-count before/after and list which `skins.css` selectors are now dead (leave that file alone).

- [ ] **Step 3: Verify and commit**

```bash
node --check $(git diff --name-only -- 'crates/termlab_tauri/frontend/**/*.js' | tr '\n' ' ')
grep -rn "ssh-overlay\|ssh-form" crates/termlab_tauri/frontend/app crates/termlab_tauri/frontend/index.html   # expect empty
cargo test --workspace 2>&1 | grep -cE "^test result: ok"   # expect 13
cargo build -p termlab_tauri && (RUST_LOG=info ./target/debug/termlab > /tmp/tl-p5a-t4.log 2>&1 &) && sleep 8 && pkill -f target/debug/termlab; grep -ci "error\|panic" /tmp/tl-p5a-t4.log
git add -A crates/termlab_tauri/frontend
git commit -m "refactor(design-system): migrate vault and tunnel dialogs, drop legacy dialog CSS"
```

---

### Task 5: Measured verification pass (controller-run)

The controller runs this task — it needs screen capture, which subagents must not use.

- [ ] **Step 1: Capture and compare**

Launch, open a representative dialog set (the controller will ask the user to open them, since clicks cannot be automated), capture each by window id, and compare against `METRICS.md`: checkbox 14px square with the measured border/fill/tick colours, primary button `#6F7F9E` with a white label, dialog background `#22252A`, right-aligned label column, buttons bottom-right in secondary→primary order.

- [ ] **Step 2: Fix deviations, then regressions**

```bash
cargo test --workspace 2>&1 | grep -cE "^test result: ok"
node scripts/tests/test_tl_icon.mjs && node scripts/tests/test_tl_dialog.mjs && python3 scripts/tests/test_extract_tokens.py
grep -rn "#[0-9a-fA-F]\{3,8\}" crates/termlab_tauri/frontend/styles/design-system/components/*.css   # expect empty
```

- [ ] **Step 3: Commit**

```bash
git add -A && git commit -m "feat(design-system): phase 5a verification pass"
```

---

## Phase exit criteria

- One dialog shell; all 26 dialogs (minus settings and the command palette, which are 5b) migrated onto it.
- Focus is trapped and restored; Escape and backdrop dismissal work per dialog; nested dialogs stack correctly and menus render above them.
- No dialog can destroy another dialog's overlay.
- Checkbox/radio/field/combo controls match the measured reference; all 17 native selects go through `tlCombo`.
- Legacy `.ssh-overlay`/`.ssh-form*` CSS deleted; tests green; human side-by-side is the final acceptance.

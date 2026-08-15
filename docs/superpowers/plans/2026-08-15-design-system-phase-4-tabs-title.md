# Design System Phase 4: Tabs, Terminal Chrome, Title Bar — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the editor area match the reference: an IntelliJ-style tab strip that sits over the terminal only, a `<workspace> – <tab>` window title, and terminal chrome flush to its borders.

**Architecture:** The tab bar moves out of the full-width app shell into the editor column, is restyled to the measured reference (unfilled tabs; the active one marked by a gap in the row's bottom border), and gains the icon/close/shortcut-hint details. The window title becomes derived state driven by the active tab.

**Tech Stack:** Vanilla IIFE JS + CSS tokens; Tauri window API for the title; no Rust changes required (the static `title` in `tauri.conf.json` becomes the boot value only).

## Global Constraints

- Spec: `docs/superpowers/specs/2026-08-14-termlab-design-system-design.md`. **Measured targets: `docs/superpowers/specs/assets/reference/METRICS.md` (Tab bar section) with the reference capture `assets/reference/jvm-termlab-tabs.png` — cite measurements, never guess.**
- Measurement loop: `scripts/ui-probe/README.md` (winlist → `screencapture -x -o -l <id>` → `scan`/`sample`). Capture drifts ±1 per channel; device px are 2× logical.
- Tokens only in CSS (`var(--tl-*)`), no raw hex in edited rules. Legacy chrome vars are token aliases already.
- Vanilla IIFE modules; `<script>`/`<link>` tags in `index.html`; commands/events via the injected client; keyboard via `window.termlabKeyboardRouter`.
- Behavior preservation: creating/closing/renaming/reordering tabs, Ctrl+N selection, tab context menu, pop-out, MultiExec-era wiring, and the tabs-hidden-at-one-tab rule must all keep working.
- `rg` is not installed — verify with `grep`. `node --check` every touched JS. Full suite `cargo test --workspace` (500 expected) plus `node scripts/tests/test_tl_icon.mjs` and `python3 scripts/tests/test_extract_tokens.py`.
- Verified anchors: `index.html:28` `<div id="tabbar"></div>` (currently a sibling ABOVE `#main-area`; the editor column is `#terminal-host` inside `#main-area`). `styles/layout.css:124-147` `#tabbar` (height 0 → 36px under `#app.tabs-visible`, `padding: 3px 0 0`, `border-bottom: 0 solid #3a3d4f` — a raw hex to remove), `:148+` `.tab-btn` (`flex: 1 1 0` — tabs currently stretch), `:181` `.tab-btn-label`, `:182-186` `.tab-btn-close`, `:187` `.tab-btn.active`. `app/tab-manager.js:45-60` builds the button (label span + `✕` close span, no icon), `:64` `setTabLabel`, `:76-78` rename, `:106/:157` title sync. Window title today: static `"title": "TermLab"` in `crates/termlab_tauri/tauri.conf.json:12`.

## Measured targets (from METRICS.md)

| Element | Target |
|---|---|
| Tab row | 27px content + 1px bottom border = 28 total; background `--tl-panel-bg` |
| Tab row extent | spans the editor column only — its bottom border lines up with the tool-window header borders |
| Tabs | content-width (~97px for "Terminal"), left-aligned, **not filled** |
| Active tab | the row's bottom border is **omitted beneath it** (merges with the terminal); no fill, no colored underline |
| Tab contents | 16px terminal icon, label, muted `×` close |
| Right end | keyboard hint (`⌘2`) in muted text |
| Terminal | background `#080A0E` starting immediately below the tab row border, flush to the window edge |

---

### Task 1: Move the tab bar into the editor column and restyle it

**Files:**
- Modify: `crates/termlab_tauri/frontend/index.html` (move `#tabbar`)
- Modify: `crates/termlab_tauri/frontend/styles/layout.css` (`#tabbar` and `.tab-btn*` rules, L124-190)
- Modify: `crates/termlab_tauri/frontend/app/tab-manager.js` (button construction L45-60)
- Modify: `crates/termlab_tauri/frontend/vendor/intellij-icons/` (+ `MANIFEST.md`, `app/ui/tl-icon.js`)

**Interfaces:**
- Produces: `#tabbar` nested inside the editor column above `#terminal-host`; `.tab-btn` gains a leading `.tl-icon`; `.tab-btn.active` marked by a border gap; logical icon name `terminal`.

- [ ] **Step 1: Vendor the terminal tab icon**

Find IntelliJ's terminal glyph (candidates, in order: `platform/icons/src/toolwindows/toolWindowTerminal.svg`, `platform/icons/src/general/console.svg`, `platform/icons/src/nodes/console.svg`) under `/Users/dustin/projects/intellij-community`. Copy it (and any `_dark` sibling) to `frontend/vendor/intellij-icons/terminal.svg` (+ `terminal_dark.svg`), add a `MANIFEST.md` line, and add `'terminal'` to `darkVariants` in `app/ui/tl-icon.js` **only if** a dark file exists. Compare the glyph against `assets/reference/jvm-termlab-tabs.png` (a small square with a `>`); if none match, use `folder` and say so in your report.

- [ ] **Step 2: Move the markup**

In `index.html`, delete `<div id="tabbar"></div>` from line 28 and re-insert it inside `#main-area` as the first child of the editor column, immediately before `#terminal-host`. Read the current `#main-area` structure first: `#terminal-host` is a flex sibling of the sidebars, so wrap them together — introduce `<div id="editor-column">` containing `#tabbar` and `#terminal-host`, and give it `flex: 1; display: flex; flex-direction: column; min-width: 0;` in `layout.css`. Preserve every existing id; other code queries `#terminal-host` directly.

- [ ] **Step 3: Restyle**

Replace the `#tabbar` rules (L124-147) and `.tab-btn*` rules with:

```css
    #tabbar {
      height: 0;
      display: flex;
      align-items: stretch;
      overflow: hidden;
      flex-shrink: 0;
      opacity: 0;
      background: var(--tl-panel-bg);
      border-bottom: 0 solid var(--tl-border);
      transition: height 180ms cubic-bezier(0.2, 0.8, 0.2, 1), opacity 160ms ease;
    }
    #app.tabs-visible #tabbar {
      height: 28px;
      border-bottom-width: 1px;
      opacity: 1;
    }
    .tab-btn {
      flex: 0 0 auto;
      display: inline-flex;
      align-items: center;
      gap: var(--tl-space-2);
      min-width: 0;
      max-width: 220px;
      height: 27px;
      padding: 0 var(--tl-space-2);
      background: transparent;
      border: none;
      color: var(--tl-fg);
      font: 400 var(--tl-font-size-ui) var(--tl-font-ui);
      cursor: pointer;
      position: relative;
    }
    .tab-btn:hover { background: var(--tl-row-hover); }
    /* The active tab is marked by punching a hole in the row's bottom border
       so it merges with the terminal below — measured off the reference,
       which uses no fill and no coloured underline. */
    .tab-btn.active::after {
      content: '';
      position: absolute;
      left: 0; right: 0; bottom: -1px;
      height: 1px;
      background: var(--tl-panel-bg);
    }
    .tab-btn-label { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
    .tab-btn-close {
      color: var(--tl-fg-muted);
      flex-shrink: 0;
      border-radius: var(--tl-radius);
      padding: 0 2px;
    }
    .tab-btn-close:hover { background: var(--tl-row-hover); color: var(--tl-fg); }
```

Delete any leftover `.tab-btn:first-child`/`.tab-btn.entering` rules that conflict (keep the entering animation only if it does not fight the new sizing — say which you kept). Remove the raw hex `#3a3d4f`.

- [ ] **Step 4: Add the icon to each tab button**

In `tab-manager.js` (L45-60), prepend the icon before the label span:

```js
      if (global.tlIcon) {
        button.appendChild(global.tlIcon.create('terminal', { size: 16, alt: '' }));
      }
```

(Use the module's IIFE global name — read the file's wrapper to get it right.) Keep `button._labelSpan` pointing at the label span so `setTabLabel` and the rename paths are untouched.

- [ ] **Step 5: Verify + commit**

```bash
node --check crates/termlab_tauri/frontend/app/tab-manager.js
grep -n "#[0-9a-fA-F]\{3,8\}" crates/termlab_tauri/frontend/styles/layout.css | sed -n '1,10p'   # the tabbar rules must contribute none
cargo build -p termlab_tauri && (RUST_LOG=info ./target/debug/termlab > /tmp/tl-p4-t1.log 2>&1 &) && sleep 8 && pkill -f target/debug/termlab; grep -ci "error\|panic" /tmp/tl-p4-t1.log
git add -A crates/termlab_tauri/frontend
git commit -m "feat(design-system): IntelliJ tab strip over the editor column"
```

---

### Task 2: Window title follows the active tab

**Files:**
- Modify: `crates/termlab_tauri/frontend/app/tab-manager.js` (activate/rename paths)
- Modify: `crates/termlab_tauri/frontend/app/main-runtime.js` or `startup-runtime.js` (initial title)

**Interfaces:**
- Produces: window title `"<workspace basename> – <active tab title>"` (en dash `–`, U+2013), updated on tab activate/rename/close, falling back to `TermLab` when no tab exists.

- [ ] **Step 1: Find the workspace name and the title API**

The workspace path arrives as the app's launch argument (see `crates/termlab_tauri/src/main.rs` / the `TERMLAB_WORKSPACE` flow) — check whether a command already exposes it (grep `get_cwd`, `workspace`, `home` in `src/commands.rs`); if one exists use it, otherwise derive from the initial local pane path already available in the frontend, and report which you used. For the title itself use `window.__TAURI__.window.getCurrentWindow().setTitle(...)`, matching how `currentWindow` is obtained elsewhere in the frontend.

- [ ] **Step 2: Implement**

Add a small helper (module-level in `tab-manager.js`) that composes and applies the title, and call it wherever the active tab changes or is renamed (the same places that already call `setTabLabel`/set `tab.button.title` at L106 and L157, plus tab activation and close). Guard every Tauri call in try/catch so a missing API can't break tab switching.

- [ ] **Step 3: Verify + commit**

```bash
node --check crates/termlab_tauri/frontend/app/tab-manager.js
cargo build -p termlab_tauri && (RUST_LOG=info ./target/debug/termlab > /tmp/tl-p4-t2.log 2>&1 &) && sleep 8 && pkill -f target/debug/termlab; grep -ci "error\|panic" /tmp/tl-p4-t2.log
git add -A crates/termlab_tauri/frontend
git commit -m "feat(design-system): window title follows workspace and active tab"
```

The controller verifies the rendered title against the reference in Task 4 (window titles are visible to `scripts/ui-probe/winlist`, so this is measurable, not subjective).

---

### Task 3: Terminal chrome flush to its borders

**Files:**
- Modify: `crates/termlab_tauri/frontend/styles/layout.css` (`#terminal-host`, `.terminal-pane` rules)

**Interfaces:** none new.

- [ ] **Step 1: Measure ours, then match**

Per METRICS.md the reference terminal background starts immediately below the tab row's 1px border with no gap, and runs flush to the window edge. Capture our window (`scripts/ui-probe`) and scan a column through the terminal top edge and a row through its left edge; if our terminal shows a gap, panel-coloured gutter, or padding that the reference lacks, remove it in `layout.css` (tokens only). Record before/after runs in your report. Do not change the terminal's internal padding if xterm owns it — report that instead.

- [ ] **Step 2: Verify + commit**

```bash
cargo build -p termlab_tauri && (RUST_LOG=info ./target/debug/termlab > /tmp/tl-p4-t3.log 2>&1 &) && sleep 8 && pkill -f target/debug/termlab; grep -ci "error\|panic" /tmp/tl-p4-t3.log
git add -A crates/termlab_tauri/frontend
git commit -m "feat(design-system): terminal chrome flush to editor borders"
```

---

### Task 4: Measured verification pass (controller-run)

**Files:**
- Modify: whatever the measurements show is off
- Create: `docs/superpowers/specs/assets/phase4-tabs-title.png`

- [ ] **Step 1: Capture with two tabs open and compare**

Launch, open a second tab, capture by window id, then scan:

```bash
/tmp/scan <shot> col <xInTab> 40 200      # tab row: 27 content + 1px border, panel bg
/tmp/scan <shot> row 112 0 620            # border gap beneath the active tab only
/tmp/winlist                              # window title must read "<workspace> – <tab>"
```

Compare each against METRICS.md's Tab bar table; fix deviations over 1px or 1 channel and re-measure.

- [ ] **Step 2: Regressions + asset + commit**

```bash
cargo test --workspace 2>&1 | grep -cE "^test result: ok"   # expect 13
node scripts/tests/test_tl_icon.mjs && python3 scripts/tests/test_extract_tokens.py
cp <shot> docs/superpowers/specs/assets/phase4-tabs-title.png
git add -A && git commit -m "feat(design-system): phase 4 verification pass with measured capture"
```

---

## Phase exit criteria

- Tab strip sits over the editor column at 28px, unfilled tabs with icon/label/close, active tab marked by the border gap, hidden at one tab.
- Window title reads `<workspace> – <active tab>` and tracks tab changes.
- Terminal chrome flush; all tests green; capture checked in. Human side-by-side is the final acceptance.

# Design System Phase 1: Foundation — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the token pipeline, vendored fonts/icons, and base component primitives that every later design-system phase builds on.

**Architecture:** A Python extractor reads the JVM TermLab's theme files and emits committed CSS custom-property files plus an Alacritty terminal theme. A hand-written semantic-alias layer and the first component CSS files (buttons, inputs, scrollbars, tool-window chrome) consume those tokens. Icons are the real IntelliJ SVGs, vendored with license; fonts are bundled JetBrains Mono + Inter.

**Tech Stack:** Python 3 (stdlib only) for the extractor; vanilla CSS + IIFE JS (no build step); Tauri static frontend.

## Global Constraints

- Spec: `docs/superpowers/specs/2026-08-14-termlab-design-system-design.md`
- JVM TermLab repo (token source): `/Users/dustin/projects/TermLab`; intellij-community checkout (icon source): `/Users/dustin/projects/intellij-community`
- Frontend is vanilla JS IIFE modules loaded via `<script>` tags in `crates/termlab_tauri/frontend/index.html` and `settings.html` — no bundler, no ES modules
- Generated token files are committed; never hand-edit them (hand edits go in the semantic alias layer)
- Component CSS uses `var(--tl-*)` only — no raw hex outside `tokens-*.css` and `base.css`
- All commands/events through `window.termlabTauriClient`; keyboard through `window.termlabKeyboardRouter` (not needed in this phase, but binding)
- `rg` (ripgrep) is NOT installed on this machine — use plain `grep` for local verification even where `scripts/check_frontend_boundaries.sh` uses rg
- Commit after each task with the message given in its final step

---

### Task 1: Token extractor with golden test

**Files:**
- Create: `scripts/extract_intellij_tokens.py`
- Create: `scripts/tests/test_extract_tokens.py`

**Interfaces:**
- Produces: CLI `python3 scripts/extract_intellij_tokens.py --termlab-repo <path> --out-dir crates/termlab_tauri/frontend` writing `styles/design-system/tokens-dark.css`, `styles/design-system/tokens-light.css`, `themes/TermLab Dark.toml`
- Produces (for Task 5+): CSS custom properties named `--tl-base-<prop>` (from the theme.json `"*"` section) and `--tl-<Component>-<prop>` with dots replaced by `-` (e.g. `--tl-ActionButton-hoverBackground`)

- [ ] **Step 1: Write the failing golden test**

`scripts/tests/test_extract_tokens.py`:

```python
"""Golden tests for the IntelliJ theme token extractor. Run directly:
python3 scripts/tests/test_extract_tokens.py"""
import json
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from extract_intellij_tokens import theme_to_css, scheme_to_alacritty

THEME = {
    "name": "TermLab Dark",
    "dark": True,
    "colors": {"accentColor": "#6B80A1", "backgroundColor": "#21252b"},
    "ui": {
        "*": {
            "background": "backgroundColor",
            "foreground": "#abb2bf",
            "selectionBackground": {"os.default": "#111111", "os.mac": "#323844"},
        },
        "ActionButton": {"hoverBackground": "#3d424b"},
        "ToolWindow": {"Header": {"background": "accentColor"}},
    },
}

SCHEME_XML = """<?xml version="1.0"?>
<scheme name="TermLab Dark" version="142">
  <colors>
    <option name="CONSOLE_BACKGROUND_KEY" value="070A0E" />
  </colors>
  <attributes>
    <option name="CONSOLE_BLACK_OUTPUT">
      <value><option name="FOREGROUND" value="3c4048" /></value>
    </option>
    <option name="CONSOLE_RED_OUTPUT">
      <value><option name="FOREGROUND" value="e06c75" /></value>
    </option>
  </attributes>
</scheme>"""


def test_theme_to_css():
    css = theme_to_css(THEME, selector=":root")
    assert "--tl-base-background: #21252b;" in css, css          # named ref resolved
    assert "--tl-base-foreground: #abb2bf;" in css               # literal passthrough
    assert "--tl-base-selectionBackground: #323844;" in css      # os.mac wins
    assert "--tl-ActionButton-hoverBackground: #3d424b;" in css  # component key
    assert "--tl-ToolWindow-Header-background: #6B80A1;" in css  # nested + named ref
    assert css.strip().startswith(":root {")
    assert "GENERATED FILE" in css                               # do-not-edit banner


def test_scheme_to_alacritty():
    toml_text = scheme_to_alacritty(SCHEME_XML)
    assert 'black = "#3c4048"' in toml_text
    assert 'red = "#e06c75"' in toml_text
    assert 'background = "#070A0E"' in toml_text


if __name__ == "__main__":
    test_theme_to_css()
    test_scheme_to_alacritty()
    print("ok")
```

- [ ] **Step 2: Run test to verify it fails**

Run: `python3 scripts/tests/test_extract_tokens.py`
Expected: `ModuleNotFoundError: No module named 'extract_intellij_tokens'`

- [ ] **Step 3: Write the extractor**

`scripts/extract_intellij_tokens.py`:

```python
#!/usr/bin/env python3
"""Generate design-system token CSS + terminal theme from the JVM TermLab repo.

Reads:
  <termlab-repo>/core/resources/themes/TermLabDark.theme.json
  <termlab-repo>/core/resources/themes/TermLabLight.theme.json
  <termlab-repo>/core/resources/termlab-dark.xml

Writes (under --out-dir, default crates/termlab_tauri/frontend):
  styles/design-system/tokens-dark.css   (:root)
  styles/design-system/tokens-light.css  (:root[data-tl-appearance="light"])
  themes/TermLab Dark.toml               (Alacritty-format terminal theme)

Generated files are committed. Never hand-edit them; semantic aliases live in
styles/design-system/base.css instead.
"""
import argparse
import json
import re
import xml.etree.ElementTree as ET
from pathlib import Path

BANNER = "/* GENERATED FILE — do not edit. Run scripts/extract_intellij_tokens.py */\n"


def _resolve(value, palette):
    """Resolve a theme.json value to a color string, or None to skip."""
    if isinstance(value, dict):
        value = value.get("os.mac", value.get("os.default"))
    if not isinstance(value, str):
        return None
    return palette.get(value, value)


def _flatten(ui, palette, prefix, out):
    for key, value in ui.items():
        name = "base" if key == "*" else key
        token = f"{prefix}-{name}" if prefix else name
        if isinstance(value, dict) and not ("os.default" in value or "os.mac" in value):
            _flatten(value, palette, token, out)
        else:
            resolved = _resolve(value, palette)
            if resolved is not None:
                out[token.replace(".", "-")] = resolved


def theme_to_css(theme, selector):
    palette = theme.get("colors", {})
    tokens = {}
    _flatten(theme.get("ui", {}), palette, "", tokens)
    lines = [BANNER, f"{selector} {{"]
    for name, value in sorted(tokens.items()):
        lines.append(f"  --tl-{name}: {value};")
    lines.append("}\n")
    return "\n".join(lines)


# ANSI slot -> (alacritty table, alacritty key)
_ANSI = {
    "CONSOLE_BLACK_OUTPUT": ("normal", "black"),
    "CONSOLE_RED_OUTPUT": ("normal", "red"),
    "CONSOLE_GREEN_OUTPUT": ("normal", "green"),
    "CONSOLE_YELLOW_OUTPUT": ("normal", "yellow"),
    "CONSOLE_BLUE_OUTPUT": ("normal", "blue"),
    "CONSOLE_MAGENTA_OUTPUT": ("normal", "magenta"),
    "CONSOLE_CYAN_OUTPUT": ("normal", "cyan"),
    "CONSOLE_GRAY_OUTPUT": ("normal", "white"),
    "CONSOLE_DARKGRAY_OUTPUT": ("bright", "black"),
    "CONSOLE_RED_BRIGHT_OUTPUT": ("bright", "red"),
    "CONSOLE_GREEN_BRIGHT_OUTPUT": ("bright", "green"),
    "CONSOLE_YELLOW_BRIGHT_OUTPUT": ("bright", "yellow"),
    "CONSOLE_BLUE_BRIGHT_OUTPUT": ("bright", "blue"),
    "CONSOLE_MAGENTA_BRIGHT_OUTPUT": ("bright", "magenta"),
    "CONSOLE_CYAN_BRIGHT_OUTPUT": ("bright", "cyan"),
    "CONSOLE_WHITE_OUTPUT": ("bright", "white"),
}


def _hex(value):
    value = value.strip().lstrip("#")
    return "#" + value.lower() if re.fullmatch(r"[0-9a-fA-F]{6}", value) else None


def scheme_to_alacritty(xml_text):
    root = ET.fromstring(xml_text)
    normal, bright = {}, {}
    background = foreground = None
    for opt in root.iter("option"):
        name = opt.get("name", "")
        if name == "CONSOLE_BACKGROUND_KEY" and opt.get("value"):
            background = _hex(opt.get("value"))
        if name in _ANSI:
            fg = opt.find("./value/option[@name='FOREGROUND']")
            if fg is not None and _hex(fg.get("value", "")):
                table, key = _ANSI[name]
                (normal if table == "normal" else bright)[key] = _hex(fg.get("value"))
    foreground = normal.get("white") or "#abb2bf"
    lines = ["# GENERATED — scripts/extract_intellij_tokens.py", "[colors.primary]"]
    if background:
        lines.append(f'background = "{background}"')
    lines.append(f'foreground = "{foreground}"')
    for table_name, table in (("normal", normal), ("bright", bright)):
        lines.append(f"[colors.{table_name}]")
        for key in ("black", "red", "green", "yellow", "blue", "magenta", "cyan", "white"):
            if key in table:
                lines.append(f'{key} = "{table[key]}"')
    return "\n".join(lines) + "\n"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--termlab-repo", default=str(Path(__file__).resolve().parents[2] / "TermLab"))
    ap.add_argument("--out-dir", default="crates/termlab_tauri/frontend")
    args = ap.parse_args()

    repo = Path(args.termlab_repo)
    out = Path(args.out_dir)
    themes_dir = repo / "core/resources/themes"

    ds = out / "styles/design-system"
    ds.mkdir(parents=True, exist_ok=True)
    dark = json.loads((themes_dir / "TermLabDark.theme.json").read_text())
    light = json.loads((themes_dir / "TermLabLight.theme.json").read_text())
    (ds / "tokens-dark.css").write_text(theme_to_css(dark, ":root"))
    (ds / "tokens-light.css").write_text(
        theme_to_css(light, ':root[data-tl-appearance="light"]'))

    theme_out = out / "themes"
    theme_out.mkdir(parents=True, exist_ok=True)
    xml_text = (repo / "core/resources/termlab-dark.xml").read_text()
    (theme_out / "TermLab Dark.toml").write_text(scheme_to_alacritty(xml_text))
    print("wrote tokens-dark.css, tokens-light.css, TermLab Dark.toml")


if __name__ == "__main__":
    main()
```

- [ ] **Step 4: Run test to verify it passes**

Run: `python3 scripts/tests/test_extract_tokens.py`
Expected: `ok`

- [ ] **Step 5: Commit**

```bash
git add scripts/extract_intellij_tokens.py scripts/tests/test_extract_tokens.py
git commit -m "feat(design-system): add IntelliJ theme token extractor with golden tests"
```

---

### Task 2: Generate real tokens and terminal theme

**Files:**
- Create (generated): `crates/termlab_tauri/frontend/styles/design-system/tokens-dark.css`, `tokens-light.css`, `crates/termlab_tauri/frontend/themes/TermLab Dark.toml`

**Interfaces:**
- Consumes: Task 1 CLI
- Produces: committed token files for Task 5; terminal theme discoverable by the existing theme loader (it scans the app `themes/` dir and `~/.config/termlab/themes/`; verify the frontend-bundled path is picked up — if the loader only reads the config dir, ALSO copy the toml there in this step and note it in the commit message)

- [ ] **Step 1: Run the extractor against the real repos**

Run: `python3 scripts/extract_intellij_tokens.py --termlab-repo /Users/dustin/projects/TermLab`
Expected: `wrote tokens-dark.css, tokens-light.css, TermLab Dark.toml`

- [ ] **Step 2: Sanity-check output values against known theme values**

Run: `grep -c -- "--tl-" crates/termlab_tauri/frontend/styles/design-system/tokens-dark.css && grep -- "--tl-base-background" crates/termlab_tauri/frontend/styles/design-system/tokens-dark.css`
Expected: a count > 100, and `--tl-base-background: #21252b;`

- [ ] **Step 3: Check how the theme loader discovers themes**

Run: `grep -rn "themes" crates/termlab_tauri/src/theme.rs | head -20`
If it loads only from the config dir, add `crates/termlab_tauri/frontend/themes/` to its scan paths OR document that the generated toml must be installed; prefer extending the scan path in `theme.rs` (small change, list the dir before the user dir so user overrides win).

- [ ] **Step 4: Commit**

```bash
git add crates/termlab_tauri/frontend/styles/design-system crates/termlab_tauri/frontend/themes crates/termlab_tauri/src/theme.rs
git commit -m "feat(design-system): generate TermLab Classic tokens and terminal theme"
```

---

### Task 3: Vendor fonts (JetBrains Mono + Inter)

**Files:**
- Create: `crates/termlab_tauri/frontend/vendor/fonts/` (woff2 files + `LICENSE-JetBrainsMono.txt`, `LICENSE-Inter.txt`)
- Create: `crates/termlab_tauri/frontend/styles/design-system/fonts.css`

**Interfaces:**
- Produces: font families `"JetBrains Mono"` and `"Inter"` available via `@font-face`; CSS vars `--tl-font-ui` and `--tl-font-mono` defined in `fonts.css`

- [ ] **Step 1: Confirm the JVM app's fonts**

Run: `grep -rn "Font\|font" /Users/dustin/projects/TermLab/core/src/com/termlab/core/settings/TermLabConsoleFontSettings.java | head -20`
Expected: console font default resolves to JetBrains Mono (IntelliJ platform default). If a different family is named, vendor that instead and adjust `--tl-font-mono`.

- [ ] **Step 2: Download pinned fonts**

```bash
cd crates/termlab_tauri/frontend/vendor/fonts
for w in Regular Bold Italic BoldItalic; do
  curl -fsSL -o JetBrainsMono-$w.woff2 \
    https://raw.githubusercontent.com/JetBrains/JetBrainsMono/v2.304/fonts/webfonts/JetBrainsMono-$w.woff2
done
curl -fsSL -o InterVariable.woff2 \
  https://raw.githubusercontent.com/rsms/inter/v4.1/docs/font-files/InterVariable.woff2
curl -fsSL -o LICENSE-JetBrainsMono.txt \
  https://raw.githubusercontent.com/JetBrains/JetBrainsMono/v2.304/OFL.txt
curl -fsSL -o LICENSE-Inter.txt https://raw.githubusercontent.com/rsms/inter/v4.1/LICENSE.txt
```

If any URL 404s: download the corresponding GitHub release zip (JetBrainsMono v2.304 / Inter v4.1), extract the woff2 files listed above, and record the actual source in the commit message.

- [ ] **Step 3: Write fonts.css**

```css
/* Design-system fonts: bundled so the look is identical on every machine. */
@font-face {
  font-family: "JetBrains Mono";
  src: url("../../vendor/fonts/JetBrainsMono-Regular.woff2") format("woff2");
  font-weight: 400; font-style: normal;
}
@font-face {
  font-family: "JetBrains Mono";
  src: url("../../vendor/fonts/JetBrainsMono-Bold.woff2") format("woff2");
  font-weight: 700; font-style: normal;
}
@font-face {
  font-family: "JetBrains Mono";
  src: url("../../vendor/fonts/JetBrainsMono-Italic.woff2") format("woff2");
  font-weight: 400; font-style: italic;
}
@font-face {
  font-family: "JetBrains Mono";
  src: url("../../vendor/fonts/JetBrainsMono-BoldItalic.woff2") format("woff2");
  font-weight: 700; font-style: italic;
}
@font-face {
  font-family: "Inter";
  src: url("../../vendor/fonts/InterVariable.woff2") format("woff2");
  font-weight: 100 900; font-style: normal;
}

:root {
  --tl-font-ui: "Inter", -apple-system, "Segoe UI", sans-serif;
  --tl-font-mono: "JetBrains Mono", "SF Mono", Menlo, monospace;
  --tl-font-size-ui: 13px;
}
```

- [ ] **Step 4: Verify files and commit**

Run: `ls -la crates/termlab_tauri/frontend/vendor/fonts/` — expect 5 woff2 + 2 licenses, each > 10KB (licenses smaller).

```bash
git add crates/termlab_tauri/frontend/vendor/fonts crates/termlab_tauri/frontend/styles/design-system/fonts.css
git commit -m "feat(design-system): vendor JetBrains Mono and Inter with licenses"
```

---

### Task 4: Vendor IntelliJ icon SVGs

**Files:**
- Create: `crates/termlab_tauri/frontend/vendor/intellij-icons/` (SVGs + `LICENSE.txt` + `MANIFEST.md`)

**Interfaces:**
- Produces: icons addressable as `vendor/intellij-icons/<name>.svg` and `<name>_dark.svg`, names listed below (used by Task 5's icon helper)

- [ ] **Step 1: Locate the icons in intellij-community**

The needed set for phases 1–2 (logical name → expected source name):
`add`, `edit`, `remove`, `refresh`, `web`, `settings` (gear), `hideToolWindow` (minimize —), `close`, `search`, `chevronDown`, `chevronRight`, `folder`, `file`, `notifications` (bell), `moreVertical`.

Run for each (example for `add`):
```bash
find /Users/dustin/projects/intellij-community/platform/icons/src -name "add.svg" -o -name "add_dark.svg" | head
```
Most live under `platform/icons/src/general/` or `platform/icons/src/actions/`. If a name is missing, search `find .../platform/icons/src -iname "*<word>*"` and pick the visually matching icon (compare with the reference screenshot); record the chosen path in MANIFEST.md.

- [ ] **Step 2: Copy with manifest**

```bash
mkdir -p crates/termlab_tauri/frontend/vendor/intellij-icons
cp /Users/dustin/projects/intellij-community/LICENSE.txt crates/termlab_tauri/frontend/vendor/intellij-icons/LICENSE.txt
```
Copy each icon (and its `_dark` variant when present) to `vendor/intellij-icons/<logicalName>.svg` / `<logicalName>_dark.svg`. Create `MANIFEST.md` with one line per icon: `logicalName ← <path relative to intellij-community> (Apache-2.0)`.

- [ ] **Step 3: Verify and commit**

Run: `ls crates/termlab_tauri/frontend/vendor/intellij-icons/ | wc -l` — expect ≥ 17 files (icons + license + manifest).

```bash
git add crates/termlab_tauri/frontend/vendor/intellij-icons
git commit -m "feat(design-system): vendor IntelliJ platform icons (Apache-2.0) with manifest"
```

---

### Task 5: Icon helper module

**Files:**
- Create: `crates/termlab_tauri/frontend/app/ui/tl-icon.js`
- Create: `scripts/tests/test_tl_icon.mjs`

**Interfaces:**
- Produces: `window.tlIcon.create(name, {size, alt})` → `<img>` element pointing at the themed variant; `window.tlIcon.resolve(name, isDark)` → path string (pure, tested)

- [ ] **Step 1: Write the failing test**

`scripts/tests/test_tl_icon.mjs`:

```js
// Run: node scripts/tests/test_tl_icon.mjs
import assert from 'node:assert';
const window = {};
globalThis.window = window;
const { readFileSync } = await import('node:fs');
eval(readFileSync('crates/termlab_tauri/frontend/app/ui/tl-icon.js', 'utf8'));

assert.equal(window.tlIcon.resolve('add', true), 'vendor/intellij-icons/add_dark.svg');
assert.equal(window.tlIcon.resolve('add', false), 'vendor/intellij-icons/add.svg');
// Icons without a dark variant fall back to the base file.
window.tlIcon._setDarkVariants(new Set(['add']));
assert.equal(window.tlIcon.resolve('web', true), 'vendor/intellij-icons/web.svg');
console.log('ok');
```

- [ ] **Step 2: Run test to verify it fails**

Run: `node scripts/tests/test_tl_icon.mjs`
Expected: failure (file not found / tlIcon undefined)

- [ ] **Step 3: Implement tl-icon.js**

```js
(function (global) {
  'use strict';
  // Icons that ship a *_dark.svg variant. Kept in JS (not fs-probed) because
  // the webview cannot stat files; regenerate with:
  //   ls crates/termlab_tauri/frontend/vendor/intellij-icons | grep _dark
  let darkVariants = new Set([
    'add', 'edit', 'remove', 'refresh', 'web', 'settings', 'hideToolWindow',
    'close', 'search', 'chevronDown', 'chevronRight', 'folder', 'file',
    'notifications', 'moreVertical',
  ]);

  function resolve(name, isDark) {
    if (isDark && darkVariants.has(name)) {
      return `vendor/intellij-icons/${name}_dark.svg`;
    }
    return `vendor/intellij-icons/${name}.svg`;
  }

  function isDarkAppearance() {
    return document.documentElement.getAttribute('data-tl-appearance') !== 'light';
  }

  function create(name, opts) {
    const img = document.createElement('img');
    img.className = 'tl-icon';
    img.draggable = false;
    img.width = (opts && opts.size) || 16;
    img.height = (opts && opts.size) || 16;
    img.alt = (opts && opts.alt) || '';
    img.src = resolve(name, isDarkAppearance());
    return img;
  }

  global.tlIcon = {
    create,
    resolve,
    _setDarkVariants: (set) => { darkVariants = set; },
  };
})(window);
```

After copying icons in Task 4, update the `darkVariants` set to the actual list (`ls ... | grep _dark`); keep the test's expectations aligned.

- [ ] **Step 4: Run test to verify it passes**

Run: `node scripts/tests/test_tl_icon.mjs`
Expected: `ok`

- [ ] **Step 5: Add script tag and commit**

In `crates/termlab_tauri/frontend/index.html`, add `<script src="app/ui/tl-icon.js"></script>` immediately before the existing `app/ui/toast.js` (or the first `app/ui/*` script if order differs); same in `settings.html` if it loads `app/ui` scripts.

```bash
git add crates/termlab_tauri/frontend/app/ui/tl-icon.js scripts/tests/test_tl_icon.mjs crates/termlab_tauri/frontend/index.html crates/termlab_tauri/frontend/settings.html
git commit -m "feat(design-system): themed icon helper with variant resolution"
```

---

### Task 6: Base layer and first component primitives

**Files:**
- Create: `crates/termlab_tauri/frontend/styles/design-system/base.css`
- Create: `crates/termlab_tauri/frontend/styles/design-system/components/button.css`
- Create: `crates/termlab_tauri/frontend/styles/design-system/components/input.css`
- Create: `crates/termlab_tauri/frontend/styles/design-system/components/scrollbar.css`
- Create: `crates/termlab_tauri/frontend/styles/design-system/components/toolwindow.css`
- Modify: `crates/termlab_tauri/frontend/index.html` and `settings.html` (stylesheet links)

**Interfaces:**
- Consumes: token vars from Task 2, fonts from Task 3
- Produces: semantic vars (`--tl-bg`, `--tl-panel-bg`, `--tl-fg`, `--tl-fg-muted`, `--tl-border`, `--tl-accent`, `--tl-selection-bg`, `--tl-selection-fg`, `--tl-row-hover`, `--tl-header-h`, `--tl-toolbar-h`, `--tl-row-h`); classes `tl-btn`, `tl-btn--primary`, `tl-icon-btn`, `tl-input`, `tl-toolwindow`, `tl-toolwindow__header`, `tl-toolwindow__toolbar`, `tl-empty-state` used by Phase 2

- [ ] **Step 1: Write base.css (semantic aliases + metrics)**

```css
/* Semantic alias layer: the only hand-written mapping from generated raw
   tokens to what components consume. Metrics measured against the JVM app. */
:root {
  --tl-bg: var(--tl-base-background);
  --tl-panel-bg: var(--tl-base-background);
  --tl-fg: var(--tl-base-foreground);
  --tl-fg-muted: var(--tl-base-infoForeground);
  --tl-border: var(--tl-base-borderColor);
  --tl-accent: var(--tl-ActionButton-hoverBorderColor, #6b80a1);
  --tl-selection-bg: var(--tl-base-selectionBackground);
  --tl-selection-fg: var(--tl-base-selectionForeground);
  --tl-row-hover: var(--tl-ActionButton-hoverBackground);

  --tl-header-h: 28px;   /* tool window title row */
  --tl-toolbar-h: 26px;  /* icon toolbar row */
  --tl-row-h: 24px;      /* table/tree compact row */
  --tl-space-1: 4px;
  --tl-space-2: 8px;
  --tl-space-3: 12px;
  --tl-space-4: 16px;
  --tl-radius: 3px;
}
```

If a `var(--tl-...)` referenced here does not exist in the generated
tokens-dark.css, pick the closest present key (check with
`grep -- "--tl-ActionButton" .../tokens-dark.css`) and note the substitution
in a comment. Verify each alias resolves before moving on.

- [ ] **Step 2: Write component CSS**

`components/button.css`:

```css
.tl-btn {
  font: 500 var(--tl-font-size-ui) var(--tl-font-ui);
  color: var(--tl-fg);
  background: var(--tl-Button-startBackground, var(--tl-panel-bg));
  border: 1px solid var(--tl-Button-startBorderColor, var(--tl-border));
  border-radius: var(--tl-radius);
  height: 26px;
  padding: 0 14px;
  cursor: default;
}
.tl-btn:hover { border-color: var(--tl-accent); }
.tl-btn:focus-visible { outline: 1px solid var(--tl-accent); outline-offset: 1px; }
.tl-btn--primary {
  background: var(--tl-accent);
  border-color: var(--tl-accent);
  color: var(--tl-selection-fg);
}
.tl-icon-btn {
  display: inline-flex; align-items: center; justify-content: center;
  width: 22px; height: 22px;
  background: transparent; border: 1px solid transparent;
  border-radius: var(--tl-radius);
}
.tl-icon-btn:hover { background: var(--tl-row-hover); }
```

`components/input.css`:

```css
.tl-input {
  font: 400 var(--tl-font-size-ui) var(--tl-font-ui);
  color: var(--tl-fg);
  background: var(--tl-TextField-background, var(--tl-bg));
  border: 1px solid var(--tl-Component-borderColor, var(--tl-border));
  border-radius: var(--tl-radius);
  height: 26px;
  padding: 0 var(--tl-space-2);
}
.tl-input:focus {
  outline: none;
  border-color: var(--tl-Component-focusedBorderColor, var(--tl-accent));
}
.tl-input::placeholder { color: var(--tl-fg-muted); }
```

`components/scrollbar.css` (WebKit — the Tauri webview):

```css
.tl-scroll::-webkit-scrollbar { width: 10px; height: 10px; }
.tl-scroll::-webkit-scrollbar-track { background: transparent; }
.tl-scroll::-webkit-scrollbar-thumb {
  background: var(--tl-ScrollBar-thumbColor, rgba(128, 134, 146, 0.4));
  border-radius: 5px;
  border: 2px solid transparent;
  background-clip: content-box;
}
.tl-scroll::-webkit-scrollbar-thumb:hover {
  background: var(--tl-ScrollBar-hoverThumbColor, rgba(128, 134, 146, 0.6));
  background-clip: content-box;
}
```

`components/toolwindow.css`:

```css
.tl-toolwindow { display: flex; flex-direction: column; background: var(--tl-panel-bg); }
.tl-toolwindow__header {
  display: flex; align-items: center; justify-content: space-between;
  height: var(--tl-header-h);
  padding: 0 var(--tl-space-2);
  font: 600 var(--tl-font-size-ui) var(--tl-font-ui);
  color: var(--tl-fg);
  border-bottom: 1px solid var(--tl-border);
}
.tl-toolwindow__header-actions { display: inline-flex; gap: var(--tl-space-1); }
.tl-toolwindow__toolbar {
  display: flex; align-items: center; gap: var(--tl-space-1);
  height: var(--tl-toolbar-h);
  padding: 0 var(--tl-space-1);
  border-bottom: 1px solid var(--tl-border);
}
.tl-empty-state {
  flex: 1; display: flex; align-items: center; justify-content: center;
  color: var(--tl-fg-muted);
  font: 400 var(--tl-font-size-ui) var(--tl-font-ui);
}
```

- [ ] **Step 3: Link stylesheets in dependency order**

In both `index.html` and `settings.html` `<head>`, before the existing `styles/base.css` link:

```html
<link rel="stylesheet" href="styles/design-system/tokens-dark.css" />
<link rel="stylesheet" href="styles/design-system/tokens-light.css" />
<link rel="stylesheet" href="styles/design-system/fonts.css" />
<link rel="stylesheet" href="styles/design-system/base.css" />
<link rel="stylesheet" href="styles/design-system/components/button.css" />
<link rel="stylesheet" href="styles/design-system/components/input.css" />
<link rel="stylesheet" href="styles/design-system/components/scrollbar.css" />
<link rel="stylesheet" href="styles/design-system/components/toolwindow.css" />
```

- [ ] **Step 4: Verify no raw hex leaked into component CSS**

Run: `grep -n "#[0-9a-fA-F]\{3,8\}" crates/termlab_tauri/frontend/styles/design-system/components/*.css`
Expected: no output. (`base.css` may contain hex only inside `var(..., fallback)` defaults; component files may not contain hex at all — move any fallback hex into `base.css` aliases.)

- [ ] **Step 5: Launch smoke test**

Run: `cargo build -p termlab_tauri && (./target/debug/termlab > /tmp/tl-ds-smoke.log 2>&1 & sleep 5; kill %1)`
Then: `grep -ci "error" /tmp/tl-ds-smoke.log` — expect 0. The app must look unchanged except font smoothing (nothing consumes the classes yet).

- [ ] **Step 6: Commit**

```bash
git add crates/termlab_tauri/frontend/styles/design-system crates/termlab_tauri/frontend/index.html crates/termlab_tauri/frontend/settings.html
git commit -m "feat(design-system): base alias layer and first component primitives"
```

---

### Task 7: Boundary check for the design system

**Files:**
- Modify: `scripts/check_frontend_boundaries.sh` (append one check)

**Interfaces:**
- Consumes: existing script structure (rg-based checks, `fail=1` accumulation)

- [ ] **Step 1: Append the check**

Following the existing pattern in the script, append before the final exit:

```bash
echo "frontend-boundary-check: scanning design-system component css for raw hex colors"
if rg -n "#[0-9a-fA-F]{3,8}" "$FRONTEND_DIR/styles/design-system/components" >/tmp/frontend-boundary-ds-hex.txt; then
  echo "frontend-boundary-check: raw hex found in design-system components (use tokens):" >&2
  cat /tmp/frontend-boundary-ds-hex.txt >&2
  fail=1
else
  echo "frontend-boundary-check: ok (design-system components use tokens only)"
fi
```

- [ ] **Step 2: Verify locally with grep (rg is not installed here)**

Run: `bash -n scripts/check_frontend_boundaries.sh && grep -n "#[0-9a-fA-F]\{3,8\}" crates/termlab_tauri/frontend/styles/design-system/components/*.css`
Expected: bash syntax OK; grep finds nothing.

- [ ] **Step 3: Commit**

```bash
git add scripts/check_frontend_boundaries.sh
git commit -m "chore(design-system): boundary check bans raw hex in component css"
```

---

## Phase exit criteria

- `python3 scripts/tests/test_extract_tokens.py` → ok
- `node scripts/tests/test_tl_icon.mjs` → ok
- App launches unchanged-looking; no console/log errors
- Tokens, fonts, icons, and primitives committed — Phase 2 (tool-window chrome,
  strips, Hosts/Tunnels) consumes them

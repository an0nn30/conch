# TermLab Design System — IntelliJ Classic Parity (Design Spec)

Date: 2026-08-14
Status: Approved design, pre-implementation
Scope: `crates/termlab_tauri/frontend`

## Goal

Rework the TermLab (Rust/Tauri) UI so that, side by side with the IntelliJ-based
TermLab, the two windows are visually indistinguishable at rest: same layout,
colors, control shapes and sizes, typography, and iconography. Deliver it as a
**design system** — a token + component layer that future skins are built from —
not a one-off restyle.

Reference implementation: the JVM TermLab repo (`~/projects/TermLab`), whose
look is defined in extractable form:

- `core/resources/themes/TermLabDark.theme.json` / `TermLabLight.theme.json`
  (IntelliJ theme format: component-keyed color tokens; dark base `#21252b`,
  foreground `#abb2bf`, accent `#6B80A1`, border `#333841`)
- `core/resources/termlab-dark.xml` (editor scheme: terminal ANSI colors)
- IntelliJ platform icon SVGs (Apache-2.0) in `~/projects/intellij-community`

Acceptance reference: the user's side-by-side screenshots (JVM TermLab main
window with Hosts/Tunnels right zones, SFTP bottom tool window, edge strips).

## Decisions (locked)

1. **Design system, not a skin.** Tokens + components + behaviors; the
   IntelliJ-classic look ("TermLab Classic") is the default skin built on it.
2. **Restyle & keep.** Conch-only surfaces (tab bar, quick-connect, zen mode,
   command palette) survive, rebuilt in the design system. At rest with one
   tab the windows match; extras appear only in use.
3. **Layout adopts the JVM structure** (see Layout section).
4. **Existing Alacritty terminal theming and hot-reload are untouched.** The
   skin system is re-expressed as token overrides.

## Architecture

### 1. Token pipeline (generated, checked in)

`scripts/extract_intellij_tokens.py` reads the JVM repo (default sibling path
`../TermLab`, overridable via `--termlab-repo`) and emits:

- `frontend/styles/design-system/tokens-dark.css`
- `frontend/styles/design-system/tokens-light.css`
- `frontend/themes/TermLab Dark.toml` (Alacritty-format terminal theme derived
  from `termlab-dark.xml` ANSI/console colors, so the terminal matches too)

Token naming:

- **Raw layer**: IntelliJ component keys, flattened:
  `--tl-ToolWindow-Header-background`, `--tl-Button-startBackground`, …
  Named color refs (`accentColor`, `selectionBackground`) and `os.mac`
  variants are resolved at generation time.
- **Semantic layer** (hand-maintained, small): `--tl-bg`, `--tl-panel-bg`,
  `--tl-fg`, `--tl-fg-muted`, `--tl-border`, `--tl-accent`,
  `--tl-selection-bg`, `--tl-selection-fg`, `--tl-row-hover`, plus spacing
  (`--tl-space-1..4`), radii, and control heights measured from the JVM app.
  Components consume semantic tokens first, raw tokens where fidelity needs it.

Generated files are committed. Regeneration is one command; CI does not need
the JVM repo present.

### 2. Component layer

CSS in `frontend/styles/design-system/components/` (one file per component),
behavior JS in `frontend/app/ui/` (vanilla IIFE modules, no build step).

Inventory (parity targets, matched to IntelliJ classic):

| Component | Notes |
|---|---|
| Buttons | default, primary (accent), icon-button (16px icon, hover square) |
| Text input / search field | IntelliJ focus ring (accent 1px + subtle outer) |
| Combo box | closed control + popup list; popup behavior in `app/ui/combo.js` |
| Checkbox | IntelliJ classic square, accent check |
| Tool window chrome | title header row; small-icon toolbar row; `Nothing to show` centered empty state |
| Table | compact rows, sortable headers, hover + selection rows |
| Tree | for Hosts folders; twisties matching IntelliJ |
| Tab strip | IntelliJ editor-tab style; hidden at 1 tab |
| Dialog shell | modal frame, title, button row (right-aligned, primary last) |
| Context menu / popup menu | shared popup primitive with menus |
| Scrollbars | thin overlay style |
| Splitters | 1px with hover grab zone |
| Status bar / toasts | IntelliJ notification balloon styling |

Icons: copy required SVGs (+ `_dark` variants) from `intellij-community` into
`frontend/vendor/intellij-icons/` with the Apache-2.0 LICENSE and a manifest
listing each icon's source path. Icon rendering helper picks variant by theme.

Fonts: terminal defaults to bundled JetBrains Mono (OFL, vendored with
license). UI font pinned during implementation to whatever the JVM app
resolves on macOS (from `TermLabConsoleFontSettings` / IntelliJ defaults —
bundled if not a system font). Sizes: UI 13px baseline; terminal size from
existing config.

### 3. Layout re-architecture

Adopt the JVM window structure:

- **Bottom tool window: SFTP** — side-by-side local/remote panes replacing the
  current left Files sidebar. Local pane: path bar (up, refresh, home, follow
  toggles) + Name/Modified table. Remote pane: Host combo + Connect/Disconnect
  buttons + Name/Size/Modified table + `Not connected` status line.
- **Right zone: Hosts (top) / Tunnels (bottom)** — stacked tool windows
  replacing the Sessions panel, each with header + icon toolbar
  (+ / edit / − / refresh; Hosts also gets the quick-connect affordance and
  ssh-config globe). Empty states show `Nothing to show`.
- **Edge strips**: right strip → Notifications, Hosts (rotated labels, active
  highlight); bottom strip → SFTP now; Proxmox / SysInfo / Script Output join
  as those features are ported.
- **Tab bar**: hidden at exactly one tab (matches reference screenshot);
  IntelliJ editor-tab styling at 2+.
- **Title bar**: native decorations retained; window title format matches the
  JVM app (`<user> — Terminal` style, driven by active session).
- Zen mode, command palette, notification history: kept, restyled (palette
  gets IntelliJ search-everywhere styling).

### 4. Skins on the token layer

A skin = one token-override CSS file (+ optional component tweak file),
registered in the existing skin picker. Default skin: **TermLab Classic**
(this design). The retro skin pack is ported to overrides in the final phase.
Alacritty terminal themes remain a separate, orthogonal axis.

## Migration plan (phases; each leaves the app working)

1. **Foundation** — extractor script + generated tokens, vendored fonts and
   icons, base primitives (buttons, inputs, scrollbars, headers, empty state).
2. **Right side** — tool-window chrome, edge strips, Sessions → Hosts +
   Tunnels split with toolbars.
3. **Bottom** — SFTP relayout to dual side-by-side panes; Files sidebar
   removed.
4. **Center chrome** — tab strip, terminal padding/background, title bar,
   status/toasts.
5. **Dialogs & settings** — dialog shell, menus, combo popups, settings
   window restyle.
6. **Skins + cleanup** — retro pack ported, legacy CSS deleted, boundary
   script extended to ban legacy style classes and raw hex in component CSS.

## Verification

- Per phase: `node --check` on touched JS; `bash scripts/check_frontend_boundaries.sh`.
- Token extractor gets unit-style checks (golden snippet of theme.json →
  expected CSS) runnable via `python3 -m pytest` or a plain assert script.
- Visual acceptance: side-by-side against the running JVM app; the user is
  the judge of "indistinguishable". Reference screenshots checked into
  `docs/superpowers/specs/assets/` for self-checks first.

## Risks

- **Details that live in Swing, not theme.json** (paddings, row heights,
  focus ring exactness): measured from reference screenshots and IntelliJ UI
  defaults; where ambiguous, the screenshot wins.
- **Icon coverage**: some JVM-side icons are TermLab-custom rather than
  platform icons; source them from the TermLab repo's resources instead.
- **Layout regression risk** in SFTP/Hosts refactors: mitigated by phase
  separation and the existing panel feature-module boundaries (data services
  unchanged; only view layers move).
- **Light theme drift**: generated from `TermLabLight.theme.json` in the same
  pipeline from day one; not hand-derived later.

## Out of scope

- Feature ports (Share, Script Runner, File Search, SysInfo, Proxmox, Light
  editor, MultiExec) — resume after this rework; ported features adopt the
  design system on arrival.
- Windows/Linux native-decoration differences beyond what the current app
  already does.
- New behaviors not present in either app.

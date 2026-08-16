# Reference metrics — JVM TermLab main window

Measured from `jvm-termlab-main-window.png` (2800x1960 device px = 1400x980
logical, retina 2x) with `scripts/ui-probe`. Device px are halved below.
Capture drifts colors ±1 per channel: measured `#22252A` == token `#21252b`.

## Colors by surface

| Surface | Measured | Token |
|---|---|---|
| Panel / tool window body | `#22252A` | `--tl-Panel-background` `#21252b` |
| Tool window header band (at rest) | `#333843` | `--tl-ToolWindow-Header-inactiveBackground` `#323844` |
| Header / panel borders | `#343840` | `--tl-base-borderColor` `#333841` |
| Table body background | `#292C33` | `--tl-Table-background` `#282c34` |
| Text field background | `#292C33` | `--tl-TextField-background` `#282c34` |
| Terminal | `#080A0E` | console scheme `#070A0E` |
| Bottom strip active tab | `#3E424A` | `--tl-ToolWindow-Button-selectedBackground` `#3d424b` |
| Side strip button (active or not) | `#22252A` | no background — see note |

Note: side-strip buttons show **no** active background in the reference, even
when their tool window is open. Bottom-strip tabs **do** get
`ToolWindow.Button.selectedBackground` when active.

## Metrics (logical px)

| Element | Value |
|---|---|
| Tool window header | 27 content + 1px border top and bottom = 29 total |
| Tool window icon toolbar row | 28 |
| Text field (path bar, quick connect) | 22 of background + 1px borders = 24 total |
| Table column-header row | 24.5 between borders (CSS height 22 + padding) |
| Side strip width | **22** (ours was 26) |
| Bottom strip height | **22** |
| Bottom strip tab | icon + label, ~67 wide for "SFTP", ~12 gap between tabs, ~21 left inset |
| SFTP bottom area total height | 312 |

## SFTP bottom tool window structure

One header (`SFTP`, full width, gear + minimize at right), then two panes
side by side split ~50/50 (divider at x=683 of 1378 content px):

- **Local pane (left)**: toolbar row (up / refresh / home / search icons +
  path field) → column headers `Name | Modified` → rows. Column divider
  between Name and Modified at ~389 of the pane's 683.
- **Remote pane (right)**: a taller row (~37) holding `Host:` combo +
  `Connect` + `Disconnect` buttons → path/search field row (22) → column
  headers `Name | Size | Modified` → rows → `Not connected` status line.

## Bottom strip

Tool-window tabs, left-aligned: `SFTP`, `Proxmox`, `SysInfo`, `Script Output`
— each an icon plus a label, active one filled. This is the same mechanism as
the side strips, laid out horizontally: the bottom area is a tool-window zone,
not a bespoke panel.

## Tab bar (measured from `jvm-termlab-tabs.png`, two tabs open)

Sits between the title bar and the terminal, spanning only the editor area
(not under the tool windows). Hidden entirely when a single tab is open.

| Element | Value |
|---|---|
| Tab row height | 28 (including its 1px bottom border) |
| Tab row background | `#22252A` — the panel background; tabs are NOT filled |
| Tab width | ~97 for the label "Terminal" (content-driven) |
| Bottom border | 1px `#343840` (`--tl-base-borderColor`) |
| **Active tab marker** | the bottom border is **omitted** beneath the active tab, so it merges with the terminal below — there is no fill and no colored underline |
| Tab contents | 16px terminal icon, label, then a muted `x` close button |
| Right end | keyboard hint (e.g. `⌘2`) in muted text |

Inactive and active labels render at the same brightness; only the border gap
distinguishes them.

## Dialogs and settings (structure from reference screenshots, 2026-08-15)

Pixel values still to be measured — the JVM theme defines **no** tokens for
CheckBox, RadioButton, Dialog or Separator, so those come from Swing defaults
and must be sampled from a live dialog window. Each dialog is its own OS
window (native title bar, centred title), not an in-page overlay.

### Modal dialog shell (`Add SSH Host`, `Add SSH Tunnel`, `Unlock Vault`)
- Native title bar with centred bold title; body on the panel background.
- Two-column form: right-aligned label column, control column filling the rest.
- Buttons bottom-right: secondary (`Cancel`) then primary (`Add` / `Unlock`)
  filled with the accent; generous padding above the button row.
- Controls seen: text field, spinner (numeric + up/down stepper), combo box
  (full width, arrow at right), radio group, file-picker field with a folder
  button, a full-width secondary toggle button (`Advanced >`).
- The focused field carries an accent border; unfocused fields are flat.

### Combo popup (open dropdown)
- Anchored under the control, same width, 1px border, panel background.
- Selected row filled with the accent (`#6B80A1`-family), white-ish text.
- Scrollbar appears when the list overflows; items left-aligned, single line.

### Settings window
- Left sidebar (~25% width) with a search field at top, a disclosure tree
  (`Appearance & Behavior` > children), and the selected row filled.
- Content pane: breadcrumb header (`Appearance & Behavior > Appearance`),
  then labelled rows; section headers (`Accessibility`, `UI Options`,
  `Tree Views`) are small caps-ish labels followed by a horizontal rule.
- Checkboxes: square, checked state filled with the accent and a white tick;
  two-column checkbox layout in `UI Options`.
- Inline blue links (`Reset to default`, `How it works`), muted helper text
  under some rows.
- Footer: `?` help button bottom-left; `Cancel`, `Apply`, `OK` bottom-right
  with `OK` filled (primary) and `Apply` disabled until changes exist.

## Settings window (measured from `jvm-termlab-settings.png`, 1934x1424 device = 967x712 logical)

Sidebar/content divider at x=386 device = **193 logical (~20% of width)**.

| Element | Measured | Notes |
|---|---|---|
| Checkbox box | 28 device = **14 logical** square, 1px border `#424854`, fill `#292C33` | unchecked = empty box, no accent |
| Checked checkbox | same box, tick drawn in `#ACB2BE` | **no accent fill** — the tick is foreground-coloured |
| Sidebar selected row | fill `#627190`, height 48 device = **24 logical** | a muted blue, lighter than `--tl-accent` `#6B80A1` |
| Primary button (`OK`) | fill `#6F7F9E`, label `#FFFFFF` | ≈ `--tl-accent`; white label, not `selectionForeground` |
| Panel background | `#22252A` | same as everywhere else |

Design notes not visible in tokens: checkboxes are **outline + foreground tick**, not accent-filled (my earlier read of the screenshot was wrong); section headers (`Accessibility`, `UI Options`, `Tree Views`) are plain-case labels followed by a 1px rule running to the right edge; helper text sits under its row in muted grey; inline links (`Reset to default`, `How it works`) render in the accent blue; the footer holds `?` bottom-left and `Cancel` / `Apply` (disabled until dirty) / `OK` bottom-right.

## Measurement caveat: both windows must be on the same display

Captured colors are display-profile dependent. On 2026-08-15 the same surface
read `#22252A` in the reference app (external display, 1x capture) and
`#1F2126` in ours (built-in Retina, 2x capture) **at the same moment** — a
~10% apparent difference that was entirely the profile, not the pixels.

Before comparing colors across the two apps:

1. Move both windows to the **same** display.
2. Sanity-check the capture scale: `sips -g pixelWidth` divided by the logical
   width from `scripts/ui-probe`'s `winlist` should be the same factor (1x or
   2x) for both captures.
3. If a comparison looks uniformly off by a few levels across *every* element,
   suspect the profile before suspecting the CSS — and confirm by sampling a
   surface both apps share (a tool-window panel) in the same session.

Within a single capture, comparisons are always valid: our dialog panel
matching our own tool-window panel proved the dialog was not being dimmed.

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

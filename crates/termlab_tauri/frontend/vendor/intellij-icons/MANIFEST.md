# IntelliJ Platform Icons — Vendor Manifest

Source: local checkout at `/Users/dustin/projects/intellij-community` (Apache-2.0, see `LICENSE.txt`
in this directory, copied verbatim from the repo root).

All icons are taken from the **classic** icon set (`platform/icons/src/...`), not the "new UI"
`expui/` set, per the design target (IntelliJ Classic look). Paths below are relative to
`platform/icons/src/` in the intellij-community checkout.

One line per logical name: `logicalName ← <path> (Apache-2.0)`. A trailing `[+ _dark]` marks names
that also have a `<name>_dark.svg` file in this directory; `[no dark variant]` marks names where the
classic set has no dark counterpart (verified by search — Task 5's icon helper must not assume a
`_dark.svg` exists for these).

- `add` ← `general/add.svg` (Apache-2.0) `[+ _dark]`
- `edit` ← `actions/edit.svg` (Apache-2.0) `[+ _dark]`
- `remove` ← `general/remove.svg` (Apache-2.0) `[+ _dark]`
- `refresh` ← `actions/refresh.svg` (Apache-2.0) `[+ _dark]`
- `web` ← `general/web.svg` (Apache-2.0) `[+ _dark]`
- `settings` ← `general/settings.svg` (Apache-2.0) `[+ _dark]`
- `hideToolWindow` ← `general/hideToolWindow.svg` (Apache-2.0) `[+ _dark]`
- `close` ← `actions/close.svg` (Apache-2.0) `[no dark variant]`
- `search` ← `actions/search.svg` (Apache-2.0) `[no dark variant]`
- `chevronDown` ← `general/chevron-down.svg` (Apache-2.0) `[no dark variant]`
- `chevronRight` ← `general/chevron-right.svg` (Apache-2.0) `[no dark variant]`
- `folder` ← `nodes/folder.svg` (Apache-2.0) `[no dark variant]`
- `file` ← `fileTypes/text.svg` (Apache-2.0) `[no dark variant]`
- `notifications` ← `toolwindows/notifications.svg` (Apache-2.0) `[+ _dark]`
- `moreVertical` ← `actions/more.svg` (Apache-2.0) `[+ _dark]`
- `sftp` ← `runConfigurations/remote.svg` (Apache-2.0) `[+ _dark]`

## Notes on non-exact-name choices

The brief lists an "expected source name" for each logical name; six of the fifteen did not have an
exact-name match in the classic set (only in `expui/`, which was intentionally avoided). For each,
the closest classic icon was chosen after reading the SVG source:

- **close**: no classic `close_dark.svg` exists outside `expui/` and the `linux/theme/*` window-chrome
  variants (which are OS-chrome close buttons, not toolbar icons — not appropriate here).
  `actions/close.svg` is the generic X-shaped close glyph used throughout the classic UI.
- **search**: `actions/search.svg` has no classic dark variant (only `expui/general/search_dark.svg`
  does); `actions/search.svg` is still the correct classic magnifying-glass glyph.
- **chevronDown` / `chevronRight`**: the classic set uses a hyphenated naming convention
  (`general/chevron-down.svg`, `general/chevron-right.svg`) rather than the camelCase
  `chevronDown.svg` / `chevronRight.svg` names, which only exist under `expui/general/`. Verified by
  reading the SVG paths that these are plain chevron glyphs (same visual family as the expui ones,
  just without the dark-mode-specific opacity/color split). Neither has a classic dark variant.
- **folder**: `nodes/folder.svg` is the standard classic folder glyph (used everywhere in the classic
  project tree). No classic `folder_dark.svg` exists (only `expui/nodes/folder_dark.svg` does).
- **file**: there is no `nodes/file.svg` in the classic set. `fileTypes/text.svg` was chosen as the
  generic "unknown/plain file" icon — a page-with-folded-corner glyph with generic text lines,
  visually pairing with `nodes/folder.svg` for tree rows (this is the classic
  `AllIcons.FileTypes.Text` icon, IntelliJ's default fallback file icon). `fileTypes/any_type.svg`
  (same silhouette, no text lines) was considered but rejected as less recognizable as "a file" on
  its own. Neither has a dark variant in the classic set.
- **moreVertical**: no classic icon is named `moreVertical`; `actions/more.svg` is a 3-dot vertical
  kebab glyph (`<circle>` stack) matching the intended "more actions" toolbar affordance, and it does
  have a classic `actions/more_dark.svg` counterpart.
- **sftp** (Task 3, `design-system-phase-3`): no `nodes/ftp.svg` or `general/remote.svg` exists in the
  classic set (the brief's candidate names). `runConfigurations/remote.svg` — the classic
  `AllIcons.RunConfigurations.Remote` two-monitors glyph — was chosen instead of
  `actions/download.svg` (a single-direction arrow, which misrepresents SFTP's two-way transfer) or
  `welcome/recentProjects/remoteProject.svg` (a window+chevron glyph read more as "open remote
  project/terminal" than "remote host connection"). `runConfigurations/remote.svg` has a classic
  `remote_dark.svg` counterpart and is the closest classic glyph to "connecting to a remote machine",
  which is what the SFTP tool window represents.

## Icons with NO dark variant

For Task 5's icon helper: the following 6 of 15 logical names have **no** `_dark.svg` file in this
directory (`<name>.svg` only) — `close`, `search`, `chevronDown`, `chevronRight`, `folder`, `file`.
The other 9 (`add`, `edit`, `remove`, `refresh`, `web`, `settings`, `hideToolWindow`, `notifications`,
`moreVertical`) each have both `<name>.svg` and `<name>_dark.svg`.
gear ← platform/icons/src/general/gearPlain.svg (Apache-2.0)

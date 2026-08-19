# File Dialog Redesign — Design

**Status:** Draft
**Date:** 2026-08-18
**Scope:** Visual and structural redesign of the unified file chooser (`app/features/editor/file-dialog.js` + `styles/design-system/components/file-dialog.css`): a places sidebar for scopes, detail columns in the listing, and a professional polish pass. Chosen from three mocked options (B's sidebar + A's columns).

## Goals

1. The dialog reads as designed, not assembled: proper row heights, aligned columns, contained path bar, native-feeling footer.
2. Host switching becomes the dialog's identity — a persistent sidebar (Places: This Mac; Hosts: each connected session, status-marked), replacing the pill row.
3. The listing earns "professional": Name / Size / Modified columns (the data already arrives in `FileEntry` and is currently discarded), file/folder icons from the vendored IntelliJ set, click-to-sort headers.

## Non-Goals

- No behavior changes to open/save semantics, guards, or keyboard flow — every existing pinned behavior survives (mode-refusal, teardown cancel, double-Enter guard, per-window session filter, clean hostLabel routing).
- No cross-directory fuzzy search (that is File Search, a future feature).
- No language-specific file icons — `folder.svg` / `file.svg` only.
- No in-dialog connecting; the sidebar lists connected hosts exactly as the pill bar did.

## Layout

Two-region dialog body (`tl-dialog` size stays as shipped or one step wider if cramped — measure):

- **Sidebar (150px, fixed):** section label `Places` → `This Mac`; section label `Hosts` → one row per connected session, `tlIcon` server-ish glyph (reuse an existing vendored icon; do not add assets), display label with the existing `(pane N)` disambiguation, and a dot via tokens for connected state (they are all connected by construction — the dot is affordance, not state machinery). The dot is `--tl-accent`, **not green**: the generated token sets carry exactly one green (`--tl-Label-successForeground`) and it exists in `tokens-dark.css` only, so a `var(green, fallback)` reference would resolve differently per theme — the phantom-correct trap `base.css` warns about — and minting a light-theme green is a token-pipeline decision with no reference measurement behind it. Sidebar fill is `--tl-panel-bg` (the tool-strip rail's own recessed fill), one step below the listing box's `--tl-control-bg`, separated from the dialog panel by a 1px border. Active scope row uses the selection tokens. Clicking = exactly the old scope-button handler (including the failed-scope retry semantics).
- **Main column:** path bar row (up button + contained breadcrumb strip + editable path field behavior unchanged + filter input, icon-decorated) — four controls in one row is tight at `lg`, so they are de-crowded with flex weights (crumbs 3, path 2, filter 1) and per-control flex-basis floors that let the row wrap to two lines at any narrower dialog rather than crushing all four; column header row (`Name` flexible, `Size` right-aligned fixed, `Modified` **left-aligned** fixed) — click toggles sort key/direction with an indicator; listing rows at `--tl-row-h` (24px) with icon + name (dirs first always), size (`—` for dirs), modified; empty/error states render in this region as today.

  `Modified` is left-aligned rather than right-aligned (an amendment to the original draft): its values are as often words (`Today`, `Yesterday`) as dates, and a right-aligned mix of the two reads ragged. Alignment between the dates themselves comes from `font-variant-numeric: tabular-nums` on the Size and Modified cells plus the fixed column widths — which is the same mechanism that lets `Jan 5` drop its leading zero.
- **Footer:** left side — Hidden-files toggle, and in save mode the `Save as` label + filename field + New Folder; right side — Cancel / primary (Open|Save). All existing footer behaviors (disabled-until-selection, overwrite confirm, prefill-selected) unchanged.

## Model additions (pure, `file-dialog-model.js`)

- `formatSize(bytes) -> string` — `—` for null/dirs handled by caller; `B`, `KB`, `MB`, `GB` with one decimal below 10, none above.
- `formatModified(epochSeconds, nowEpochSeconds) -> string` — `Today`, `Yesterday`, `Mon D` within the year (no leading zero on the day — Finder/VS Code convention; the column aligns via tabular numerals, not padding), `YYYY-MM-DD` otherwise; `now` injected for testability; null → `—`.
- `sortEntries(entries, key = 'name', direction = 'asc')` — extends the existing signature backward-compatibly (no-arg call keeps today's behavior exactly). Directories always sort before files regardless of key/direction; within each group, `name` (case-insensitive), `size` (numeric), `modified` (numeric, null last). Stable.

## Constraints

- Tokens only in CSS; the boundary script enforces it.
- The DOM class names the existing tests select on are a contract: `test_file_dialog.mjs`, `test_editor_chooser_teardown.mjs` (6), `test_editor_save_as.mjs`'s dialog harness, and `test_editor_untitled.mjs` all drive the real DOM. Renames/moves must update those tests deliberately WITHOUT weakening any behavioral assertion — diff discipline: assertion changes are review targets.
- Keyboard behavior byte-for-byte: arrows/Enter/Escape, type-ahead filter, path-field Enter, tl-dialog focus trap.
- `git` workflow per CLAUDE.md: this work lives on `feat/file-dialog-redesign`; no commits to main; no Co-Authored-By trailers.

## Testing

- Model: exhaustive unit tests for the three additions (size boundaries 999 B/1.0 KB/9.9→10 KB, year boundary, Today/Yesterday edges around midnight via injected now, sort stability, dirs-first invariance under every key/direction, null-modified ordering).
- Dialog: existing suites pass with deliberate selector updates only; new checks — sidebar renders Places+Hosts sections with the active row marked; header click cycles sort and the listing order proves it (discriminating fixture: an order where name-asc, size-asc and modified-asc all differ); a dir row shows `—` for size.
- Manual (checklist section H): visual pass in both themes; sidebar host switching incl. a failed host retry; sort clicks; save-mode footer; long-path breadcrumb scroll; the cramped-vs-comfortable row height judgment is explicitly human.

## Known limitations

- Sort choice is per-dialog-open, not persisted.
- Hosts appear only while connected (unchanged).
- ~~**Light theme is approximate, app-wide and not this dialog's doing.**~~ **Fixed.** `tokens-light.css` previously omitted `--tl-base-borderColor`, `--tl-base-infoForeground`, `--tl-base-selectionBackground` and `--tl-base-selectionForeground`, so `--tl-border`, `--tl-fg-muted`, `--tl-selection-bg` and `--tl-selection-fg` silently resolved to their dark values under `data-tl-appearance="light"`. The light source theme (`TermLabLight.theme.json` `ui.*`) now defines all four, and `scripts/tests/test_token_parity.mjs` fails loudly if any raw token consumed without a fallback by a `base.css` semantic alias is ever missing from either generated token set.
- The sidebar's scrollbar is the platform default: the custom `.tl-scroll` treatment is applied by class in JS, and the sidebar element does not carry it. Only visible with enough connected hosts to overflow.

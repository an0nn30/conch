# Phase 6 input: skin selectors orphaned by the design-system migration

`styles/skins.css` was deliberately left untouched through design-system Phases 1–5b. Every
surface migrated onto `tl-*` components therefore renders **unskinned** — it ignores the active
skin and shows default design-system styling. This was a decision, not an oversight: re-skinning
is Phase 6's job, and doing it per-phase would have meant rewriting the same rules five times.

This file is the inventory Phase 6 works from. Counts are selector occurrences in
`crates/termlab_tauri/frontend/styles/skins.css`, verified 2026-08-16 against the branch
`design-system-phase-5b` at commit `ca9b78b`.

## Dead — no producer anywhere in `app/` or the HTML files

| Selector | Occurrences | Killed by |
|---|---|---|
| `.settings-sidebar` (bare) | 2 | Phase 5b Task 1 |
| `.settings-sidebar-item` (incl. `.active`) | 23 | Phase 5b Task 1 |
| `.settings-sidebar-search` | 18 | Phase 5b Task 1 |
| `.settings-dialog` | 17 | Phase 5b Task 1 |
| `.settings-section-label` | 2 | Phase 5b Task 1 |
| `.settings-divider` | 2 | Phase 5b Task 4 |
| `.settings-select` (incl. `:focus`) | 18 | Phase 5b Task 2 |
| `.settings-toggle` (incl. `.active`) | 34 | Phase 5b Task 2 |
| `.settings-switch` (`.slider`, `input:checked + .slider`) | 4 | Phase 5b Task 2 |
| `.settings-shortcut-key` | 3 | Phase 5b Task 2 |
| `.command-palette` | 17 | Phase 5b Task 3 |
| `.command-palette-input` | 18 | Phase 5b Task 3 |
| `.command-palette-shortcut` | 2 | Phase 5b Task 3 |
| `.ssh-form` (bare container only) | 17 | Phase 5b Task 4 |

Roughly 177 selector occurrences in total, across the 17 built-in skins: metal, win95, win31,
winxp-luna, win2000-classic, mac-os9-platinum, mac-osx-panther, gnome2-clearlooks, kde3-keramik,
motif, nextstep, amiga-workbench, ibm-cua, terminal-glass, crt-amber, cyberdeck-industrial,
blueprint.

## Still live — do NOT delete

| Selector | Producer |
|---|---|
| `.settings-input` | `app/features/settings/renderers.js:96` (sidebar search field) |
| `.settings-search-highlight` | `app/features/settings/search.js:105` |
| `.settings-titlebar*` | `settings.html:29-31` (custom titlebar, Windows/Linux only) |
| `.ssh-form-btn`, `.ssh-form-label` | vault dialogs, `sections-terminal.js`, `plugins-section.js`, `ssh/dependency-prompt.js` |

Note the trap: `.ssh-form-btn` and `.ssh-form-label` survive while the bare `.ssh-form` container
does not, and they share comma-separated rule blocks with dead selectors in several skins. Deleting
by rule block rather than by selector will break live surfaces.

## What Phase 6 has to decide

The `tl-*` components are token-driven, so the cheapest port is to express each skin as a token
override set rather than as per-component rule blocks — that is what the design system was built
for. A skin that genuinely needs different *geometry* (win95's beveled borders, mac-os9's stripes)
still needs real rules, but those should hang off the `tl-*` class names, not the deleted ones.

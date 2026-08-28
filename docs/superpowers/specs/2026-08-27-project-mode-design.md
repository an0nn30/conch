# Project Mode — Design

**Date:** 2026-08-27
**Status:** Approved design, pre-implementation
**Depends on:** the `codex/lsp-completion` branch — LSP project context and trust
(`lsp_project_candidates` / `lsp_set_project_context` / `lsp_set_project_trust`),
`editor-service` ownership, the unified per-window jump trail
(`lsp-navigation-history`, commit `dfc8923`), and the CLI open-path routing seam
(`DIRECTORY_COMING_SOON` in `open-path-routing.js`).

## Purpose

Opening a directory turns a TermLab window into a lightweight IDE for that
directory: a project tree in the Files panel, one trust decision for the whole
project, project-wide text search, git status in the tree, and a recent-projects
list. The owner navigates by Vim (`gd`, `Ctrl-O`/`Ctrl-I`, `]d`); project mode
supplies the missing workspace shell around that navigation.

Explicitly **not** in v1 (deferred, in priority order): a fuzzy file finder
(Cmd-P) and the file index it requires; regex search; a filesystem watcher for
the tree; multi-root projects; remote (SFTP) projects.

## 1. Project lifecycle

A **project** is one absolute directory path, opened explicitly. There is no
auto-detection: opening a file never creates a project.

**Entry points:**
- CLI: `termlab <dir>`. `open_path.rs` / `cli.rs` already queue arbitrary
  paths; the frontend's `routeOne` currently answers directories with the
  `DIRECTORY_COMING_SOON` toast. That branch is replaced: a directory routes to
  "open as project."
- In-app: a File > Open Folder… menu item and a command-palette action
  ("Open Folder as Project…"), using the native directory picker. From inside a
  project window, Open Folder opens a **new** window; it never re-targets the
  current one.

**Registry (Rust).** New module `crates/termlab_tauri/src/project/`:

```
project/
  mod.rs        — ProjectRegistry: window label → ProjectState { root, opened_at };
                  Tauri commands; cleanup on window destroy
  git_status.rs — porcelain v2 parsing (section 4)
  search.rs     — walker + streaming matches (section 3)
```

Commands:
- `project_open(path) -> ProjectOpenResult` — canonicalizes, verifies it is a
  readable directory, and either creates a new project window bound to it or,
  if some window already holds the same canonical root, focuses that window and
  reports `{ focused_existing: true }`. One project per window; one window per
  project.
- `project_info() -> Option<ProjectInfo { root, name }>` — resolved for the
  calling window (same `currentWindow` threading as `panel_host`). `name` is
  the directory basename; it becomes the window title.
- Registry entries are removed on window destroy (same hook the window
  registry already uses).

**Window shape.** A project window opens with:
- the Files tool window visible in its zone, rendering the project tree;
- one terminal tab, cd'd to the project root, as the initial main content
  (editor tabs join it as files open);
- NOT zen mode. The existing CLI file-open behavior (zen, editor-only) is
  untouched for file arguments; only directory arguments take this path.

**Vanished root.** If the root disappears while the window is open, the tree
renders a "project folder is missing" state with a Reopen/Choose-again action;
open editor tabs are untouched. `project_open` on a nonexistent path is a toast
error, not a window.

## 2. Context-aware Files panel

`files-panel.js` becomes mode-aware at initialization and on demand:

- `project_info` returns a project → render **project mode**: a single-pane
  lazy tree rooted at the project. The dual-pane local+SFTP explorer code path
  is unchanged and still what every non-project window gets.
- A header toggle inside project windows switches the panel to the classic
  dual-pane view and back (state is per-window, not persisted in v1). SFTP
  therefore remains fully reachable from a project window.

**Tree.** New module `frontend/app/panels/project-tree.js` (IIFE global,
consumed by `files-panel.js`); the tree is its own module rather than more code
in the already-large files panel.

- Listing via the existing `local_fs` commands (`FileEntry` interface) — no new
  listing backend. Lazy: a directory lists when first expanded.
- Sort dirs-first, then alphabetical, case-insensitive. Dotfiles follow the
  files panel's existing hidden-files convention.
- Keyboard: arrows navigate, Right/Left expand/collapse, Enter opens, with the
  same capture-phase/router discipline as other panels. Clicking a row opens
  the file through `editor-service` (ownership rules hold; the open lands on
  the `Ctrl-O` jump trail via the `dfc8923` recorder — no extra work needed).
- Context menu: reuse the files panel's existing local operations (new file,
  new folder, rename, delete, reveal in Finder) against tree rows.
- Refresh: re-list on expand, refresh-all on window focus, manual refresh
  button. No filesystem watcher in v1.
- Unreadable directory → toast + collapsed row; the rest of the tree keeps
  working.

## 3. Project-wide text search

**Frontend.** New bottom-zone tool window "Search" (`frontend/app/panels/
project-search-panel.js` + a `project-search.css` component), registered only
when the window has a project, after the existing bottom-zone registrants (the
Problems-panel registration-order rule). Palette action "Search in Project" and
default shortcut `cmd+shift+f` in `[termlab.keyboard]`. Input row with a
case-sensitivity toggle; results grouped by file (path relative to root), one
row per match with line number and trimmed line preview; Enter/click opens the
file at the match through `editor-service` (jump-trail recorded). Empty,
searching, capped, and no-results states are explicit.

**Backend (`project/search.rs`).** Pure Rust; no dependence on `rg` existing on
the host:
- Walk with the `ignore` crate (new dependency): respects `.gitignore`,
  `.ignore`, global excludes; skips hidden VCS dirs.
- Literal substring match, case-sensitive or not (v1; regex is future work).
- Skip files that look binary (NUL probe in the first block) and files over a
  size cap (2 MB).
- Stream results to the calling window in batches via events
  (`project-search-results`), with a hard cap of 1000 matches; the terminal
  event says whether the cap was hit.
- `project_search(query, case_sensitive) -> search_id` starts a search and
  cancels any previous one for that window; `project_search_cancel()` stops
  outright. Cancellation is a flag the walker checks per file.

## 4. Git status in the tree

**Backend (`project/git_status.rs`).**
- `project_git_status() -> GitStatusSnapshot` runs
  `git -C <root> status --porcelain=v2 -z` with a timeout. Parsing lives in a
  pure function over the raw bytes → `path → GitFileState`
  (`modified | added | untracked | deleted | renamed | conflicted`), unit-tested
  against fixture porcelain output including renames and the `-z` framing.
- No `git` on `PATH`, not a repository, or a timeout → `GitStatusSnapshot::
  unavailable` and the feature is silently off. Never a toast.

**Frontend.** Tree rows tint by state using theme tokens (never hex), with
directory rollup: a folder shows the modified tint when anything beneath it has
a state (computed in JS by path prefix over the flat snapshot). Refresh
triggers: window focus, after an editor save in that window, and a 10-second
timer while the Files panel is visible in project mode. Snapshots are
replace-only (no merging).

## 5. Trust & LSP integration

- Opening a project **is** choosing the LSP root: when an editor pane in a
  project window attaches a file under the root, the frontend passes the
  project root as the chosen context (existing `lsp_set_project_context`
  path). The per-file root-candidate chooser never appears inside a project
  window for files under the root.
- **One trust ask per project.** On first render of a project window whose root
  is not already trusted, a non-blocking banner (project-tree header area, not
  a modal) offers: "Trust this project and start language servers?" —
  [Trust project] / [Not now]. Trust calls the existing
  `lsp_set_project_trust`; the existing trust persistence means the banner
  never returns for that project. Not-now dismisses for the window's lifetime;
  the existing per-file status strip remains the way to trust later. Editing
  is never blocked either way.
- Files **outside** the root opened in a project window (e.g. `gd` into std or
  a cargo-registry source) keep the loose-file behavior verified in commit
  `1a98d8f`: plain editable tab, no prompts, no LSP attach.

## 6. Recent projects & persistence

`state.toml` (via `termlab_core`, `#[serde(default)]` for back-compat):

- `recent_projects`: capped list (10) of `{ path, last_opened }`, most recent
  first, updated on every `project_open`. Feeds a File > Open Recent Project
  submenu and palette entries ("Reopen Project: <name>"). A recent whose path
  no longer exists is skipped in the menu and pruned on next update.
- `project_layouts`: map of project path → the same layout snapshot shape the
  per-window layout persistence already uses (panel visibility, sizes, bottom
  zone). Saved on project-window close; applied when that project reopens.
  Absent entry → the default project-window shape from section 1.

`termlab <dir>` for an already-open project routes through the existing
single-instance IPC and focuses the window (from `project_open`'s
`focused_existing` path).

## 7. Error handling summary

| Failure | Behavior |
|---|---|
| `project_open` on missing/unreadable path | toast error, no window |
| Root vanishes while open | tree shows missing-project state; tabs untouched |
| Unreadable subdirectory | toast, row collapsed, tree keeps working |
| Search cap hit | terminal event flags it; panel shows "first 1000 matches" |
| Search cancelled | silent; superseded by the new query's results |
| git absent / not a repo / timeout | status silently off |
| Recent project path gone | skipped in menu, pruned on next update |

## 8. Testing

**Rust (`#[cfg(test)]` per module):** registry insert/focus-existing/cleanup;
canonicalization (symlinked roots collapse to one project); porcelain v2
parsing fixtures (states, renames, `-z` framing, malformed input); search
walker (gitignore respected, binary/size skips, cap, cancellation flag,
case-insensitivity); CLI directory routing; `state.toml` round-trips and
back-compat for the two new fields.

**Frontend (VM harness rules: no `sandbox.global`, JSON-roundtrip cross-realm,
no lookbehind, no control bytes; guards extended to every new module):** panel
mode selection (project vs dual-pane vs toggle); tree render/sort/lazy-expand/
keyboard flow; open-through-editor-service and the jump-trail entry it records;
search panel states and result activation; trust banner flow (trust / not-now /
already-trusted); git tint rollup; registration order (Search never steals the
bottom zone).

**Manual (added to the POC checklist when implemented):** `termlab <dir>` cold
open; Open Folder; SFTP toggle; trust-then-`gd` flow; search under a dirty
tree; git tints while editing; recent-project reopen restoring layout.

## 9. Implementation order

Three sub-arcs, each independently shippable, one plan:

1. **Core** — `project/` registry + `project_open`/`project_info`, CLI
   directory routing, project window shape, context-aware Files panel with the
   tree, trust banner + LSP root integration.
2. **Search** — walker + streaming + Search tool window + shortcut.
3. **Git + recents** — porcelain parsing + tree tints; recent projects +
   per-project layout persistence.

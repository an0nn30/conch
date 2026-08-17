# Share: Import Conflict Resolution — Design

**Status:** Design
**Date:** 2026-08-17
**Scope:** Sub-project 2 of 2 — the four-status conflict model, a preview table with per-row Skip / Replace / Rename plus per-status bulk actions, and inline vault creation during import.
**Predecessor:** `docs/superpowers/specs/2026-08-16-share-bundle-design.md` (shipped; bundle format, encrypted export, UUID-preserving import).
**Source requirements:** `~/projects/TermLab/docs/plans/2026-04-14-share-export-import.md`, the JVM implementation being ported.

## Goals

- Show the user exactly what an import will do before it does it, per item.
- Let them override it per item, and per conflict class in bulk.
- Make a bundle work on a machine with no vault yet — the clean-install case that is the whole point of person-to-person sharing.
- Stay usable when a bundle carries a hundred-plus items.

## Non-Goals

- **No cross-file rollback.** Unchanged from sub-project 1: each file writes atomically, but a failure part-way through leaves partial state and reports it.
- **No field-level diff** of what a Replace would overwrite. That is a diff UI well past parity.
- **No modal on the first conflict.** The JVM interrupts with a Skip/Replace/Rename modal carrying an "apply to all remaining of this type" checkbox before the user has seen the table. **Decision (user, 2026-08-17):** deliver the same power as an in-table bulk control per status instead — see Preview below.
- No conflict handling for the legacy plaintext JSON path, which keeps its regenerate-UUIDs-and-append behaviour untouched.

## Decisions taken before this spec

Settled with the user on 2026-08-17 and not open for re-litigation during implementation:

1. **Table plus per-status bulk row; no first-conflict modal.**
2. **Inline vault creation** as a step in the import dialog, not a hand-off to the standalone vault dialog.
3. Defaults per status, below. In particular `SameId` continues to default to **Replace**, preserving the semantics sub-project 1 shipped; the improvement is that it is now visible and overridable rather than silent.

## Current state this builds on

- `crates/termlab_share/src/import_planner.rs` — `ItemAction { Add, Replace }` :23, `PlannedItem<T> { item, action }` :29, `PlannedFolder { id, name, expanded, entries }` :47, `ImportPlan { folders, servers, tunnels, accounts, keys, skipped }` :54, `plan(bundle, config, existing_account_ids)` :79. Existence is decided with `config.find_server`, which scans folders as well as ungrouped.
- `crates/termlab_share/src/import_executor.rs` — `execute` :46, `apply_folder` :269, `apply_server_entry` :319, `apply_tunnel` :333. **`execute` already switches on `planned.action`** rather than re-deriving it from live state, so a user-set action reaches the mutation directly. That was a deliberate fix in sub-project 1 for exactly this purpose.
- `crates/termlab_tauri/src/share_commands.rs` — `do_import` performs decode → plan → execute in one command, and passes a `VaultSink` only when a vault exists and is unlocked; otherwise credentials are held back and `credentials_held_back` is reported.
- `crates/termlab_tauri/frontend/app/panels/ssh-panel.js` — `importConfig` :827, `runImport` :864, `showImportPasswordDialog` :884, `showImportSummary` :950.
- `crates/termlab_vault/src/lib.rs` — `VaultManager::create(password)` :62, `unlock(password)` :79, `is_locked()` :44. Vault creation already exists and is exercised by `create_and_unlock_vault` :316; the Tauri layer wraps it as `vault_create` / `vault_unlock` (`vault_commands.rs` :122, :137).
- The export dialog's `.tl-picker` (`styles/design-system/components/picker.css`, `app/features/ssh/export-picker.js`) provides the bounded scroll box, filter and sticky group headers this table reuses.

## Architecture

### 1. The planner reports status; the user sets action

```rust
pub enum ConflictStatus {
    New,               // nothing local with this id or label
    SameId,            // an item with this id already exists locally
    LabelCollision,    // a different item already uses this label
    ReferenceBroken,   // a tunnel whose host is neither in the bundle nor local
}

pub enum ItemAction {
    Add,
    Replace,
    Skip,
    Rename(String),
}

pub struct PlannedItem<T> {
    pub item: T,
    pub status: ConflictStatus,
    pub action: ItemAction,   // the planner's default; the dialog may change it
}
```

`plan()` gains no new inputs — `config` and `existing_account_ids` already carry everything needed. `ImportPlan::skipped` stays for items the planner drops outright (a malformed row), distinct from an item the *user* set to `Skip`, which stays in the plan carrying that action so the summary can count it.

**Status precedence:** an id match wins over a label match. An item whose id already exists is the same item, whatever it is now called, so it is `SameId` even if its label also collides with a third item.

**Defaults per status:**

| Status | Default | Why |
|---|---|---|
| `New` | `Add` | Nothing to weigh. |
| `SameId` | `Replace` | Keeps what sub-project 1 shipped; a re-import of an updated bundle is the common case. Now visible and overridable. |
| `LabelCollision` | `Add` | Labels are not unique keys. Blocking on a coincidence would be irritating; Rename is offered. |
| `ReferenceBroken` | `Skip` | Importing a tunnel that points at nothing produces a row that cannot work. |

### 2. The executor gains two arms

`Skip` is a no-op for the item. `Rename(new_label)` writes the item with its **label changed and its id preserved** — id preservation is load-bearing, because tunnels inside the same bundle reference hosts by id, so renaming a host must not orphan its tunnels. `apply_folder`, `apply_server_entry` and `apply_tunnel` each learn both arms; nothing else in `execute` changes.

### 3. The dialog grows two steps

The import flow becomes: pick file → password (bundle only) → **vault step (conditional)** → **preview** → apply → summary.

**Vault step** runs only when the decoded bundle carries credentials. Three variants in one step:

- **No vault yet:** master password + confirm, with the line *"This bundle contains saved credentials. To store them you need a vault on this machine. Pick a master password — you'll use it to unlock the vault from now on."* Submits through the existing `vault_create`.
- **Vault exists but locked:** prompt for the existing master password, through `vault_unlock`.
- **Already unlocked:** skipped entirely.

Password rules are whatever `VaultManager::create` already enforces; this step must not invent a second set.

**Preview** is a table of Type / Label / Status / Action inside a `.tl-picker` scroll box, with the export picker's filter above it.

Rows cover **hosts, tunnels and credentials** (a bundled `VaultAccount`, labelled by its display name). Embedded **keys are not rows**: a key is not independently meaningful to a user and always follows the account that references it, so `ImportPlan::keys` keeps no action and is materialised for whichever accounts survive the user's decisions. This diverges from the JVM, whose table lists a Key type; a row a user cannot sensibly decide about is noise.

Per row, Action is a `tl-combo` offering only the actions valid for that status:

| Status | Actions offered |
|---|---|
| `New` | Add, Skip |
| `SameId` | Replace, Skip |
| `LabelCollision` | Add, Rename, Skip |
| `ReferenceBroken` | Skip only (disabled control, shown for transparency) |

`Rename` is offered for `LabelCollision` alone — renaming a `SameId` item is meaningless, since it *is* the local item and Replace already carries the bundle's label. Above the list, one bulk control per status group present in the plan — `3 already exist: [Replace all ▾]`. Rename edits inline and pre-fills a suggestion of the form `label (2)`, incrementing until it is locally unique.

A footer line reads `4 new, 2 replace, 1 skip, 1 rename`, recomputed as actions change, so the user can sanity-check the whole before committing.

### 4. Command shape

`do_import` splits, mirroring what the export side already does for its pre-write preview:

- `share_import_plan(path, password) -> ImportPreview` — decode, plan, and return the rows for display. Performs no mutation.
- `share_import_apply(path, password, decisions) -> ImportOutcome` — re-decode, re-plan, apply the user's decisions, mutate.

`decisions` is a list of `{ kind, id, action, label? }`. Re-planning on apply rather than holding the decoded bundle in backend state between calls keeps plaintext key material from living in a long-lived struct — the same reasoning the export preview used. Decisions are matched to re-planned rows by `(kind, id)`; a row that no longer exists on re-plan is ignored, and a row that appears is applied with its default.

## Error handling

| Situation | Behaviour |
|---|---|
| Wrong bundle password | "Incorrect password", prompt re-shown (unchanged) |
| Vault creation fails | Error shown in the vault step; the step stays open; nothing has been written |
| Vault unlock fails | "Incorrect master password", step stays open |
| User cancels at the vault step | Import aborts having written nothing |
| A decision references a row that vanished on re-plan | Ignored; the surviving rows still apply |
| Write failure mid-apply | Stop, report, accept partial state (unchanged) |

## Testing

**Unit, `crates/termlab_share`:**

- `import_planner` — table-driven over the status matrix: a new item; an id that exists ungrouped; an id that exists folder-nested; a label that collides with a different id; a tunnel whose host resolves neither in the bundle nor locally; and the precedence case where an item both matches an id and collides on label, asserting `SameId` wins. Each case also asserts the default action.
- `import_executor` — `Skip` writes nothing; `Rename` changes the label and **preserves the id**; a renamed host still resolves for a tunnel in the same bundle that references it by id; `Replace` on a folder-nested host updates in place rather than duplicating (the sub-project 1 regression, re-asserted here because the new arms touch the same functions).

**Integration:** a round trip where machine B is seeded with a conflicting id, a conflicting label and a tunnel whose host is absent — asserting the four statuses are produced, that the defaults apply, and that overriding each one changes the end state as expected.

**Frontend:** the decision-summary counter (`4 new, 2 replace…`) and the rename-suggestion generator are pure functions and get stub-DOM tests, following `scripts/tests/test_export_picker.mjs`.

## Security notes

- The apply step re-decodes rather than caching the decoded bundle, so plaintext key material is not held between two IPC calls.
- The master password entered in the vault step is zeroized after use, matching the bundle password's handling in `share_export` / `share_import`.
- Nothing about credentials-off bundles changes; they skip the vault step entirely.

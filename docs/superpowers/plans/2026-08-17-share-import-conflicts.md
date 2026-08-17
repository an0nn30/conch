# Share Import Conflict Resolution Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Show the user exactly what an import will do per item, let them override it, and let a recipient with no vault create one without leaving the import.

**Architecture:** The import planner starts reporting a `ConflictStatus` alongside a default `ItemAction`, and `ItemAction` widens to include `Skip` and `Rename`. The executor already switches on `planned.action`, so it needs only the two new arms. The single `share_import` command splits into a plan step and an apply step, and the dialog grows a conditional vault step plus a preview table built on the export picker's scroll box.

**Tech Stack:** Rust (serde, ts-rs, uuid), Tauri 2 commands, vanilla IIFE frontend on the `tl-dialog` / `tl-picker` / `tl-combo` design system.

**Spec:** `docs/superpowers/specs/2026-08-17-share-import-conflicts-design.md`

## Global Constraints

- The spec governs; this plan implements it. Read the spec's decision table before changing any default.
- **Defaults are settled and must not be "improved":** `New`→Add, `SameId`→Replace, `LabelCollision`→Add, `ReferenceBroken`→Skip.
- **Status precedence:** an id match wins over a label match. An item whose id already exists is `SameId` even when its label also collides with a third item.
- **Actions offered per status:** `New` → Add, Skip. `SameId` → Replace, Skip. `LabelCollision` → Add, Rename, Skip. `ReferenceBroken` → Skip only (control shown but disabled).
- **`Rename` preserves the item's id and changes only its label.** Tunnels reference hosts by id within a bundle, so a rename that changed the id would orphan them.
- **Keys are not rows.** `ImportPlan::keys` carries no action; keys materialise for whichever accounts survive the user's decisions.
- The apply step **re-decodes and re-plans** rather than caching the decoded bundle between IPC calls, so plaintext key material never lives in long-lived state. Decisions match re-planned rows by `(kind, id)`; a row that vanished on re-plan is ignored, a row that appeared applies its default.
- The legacy plaintext JSON import path is untouched — no statuses, no preview, keeps regenerate-UUIDs-and-append.
- Master and bundle passwords are zeroized after use, matching `share_export`/`share_import`.
- Frontend: vanilla IIFE, no bundler; new JS/CSS registered in `index.html` (and `settings.html` only if a design-system component); design-system components only, raw hex only in `styles/design-system/base.css`.
- **Cross-platform:** any Unix-only API behind `#[cfg(unix)]`. CI runs `cargo test --workspace` on `windows-latest`. Cross-check with `cargo check --target x86_64-pc-windows-msvc -p termlab_share -p termlab_vault -p termlab_remote -p termlab_core` (installed; `termlab_tauri` cannot cross-check from macOS).
- `rg` is NOT installed — use `grep`. Run `node --check` on touched JS; `cargo test --workspace` must stay green (16 `test result: ok` today).
- Do not touch `crates/termlab_tauri/src/platform.rs` or `src/main.rs`.
- **Never run `screencapture`** — no display access; the controller does visual checks.
- The working tree carries another session's untracked packaging files (`Packager.toml`, `icons/`, `crates/termlab_tauri/Packager.toml`). Never stage, commit or revert them; name your own paths explicitly, never `git add -A`.
- **If a build error names a symbol you can see on disk, suspect stale artifacts before suspecting the code.** This repo hit exactly that after a history rewrite; `cargo clean -p <crate>` on the affected crates is the fix.
- Every command output pasted into a report must come from a command actually executed.

### Verified anchors

- `crates/termlab_share/src/import_planner.rs` — `ItemAction { Add, Replace }` :23, `PlannedItem<T>` :29, `PlannedFolder` :47, `ImportPlan` :54, `plan()` :79, `action_for(bool)` :183, tests from :190. Note :142-160: an unresolvable tunnel is currently pushed to `skipped` and `continue`d — it must instead survive as a `ReferenceBroken` row.
- `crates/termlab_share/src/import_executor.rs` — `execute` :46, `apply_folder` :269, `apply_server_entry` :319, `apply_tunnel` :333.
- `crates/termlab_tauri/src/share_commands.rs` — `do_import`, `share_import`, and the export side's existing plan/write split (`share_export_preview` / `share_export`) to copy the shape from.
- `crates/termlab_tauri/frontend/app/panels/ssh-panel.js` — `importConfig` :827, `runImport` :864, `showImportPasswordDialog` :884, `showImportSummary` :950.
- `crates/termlab_vault/src/lib.rs` — `create(password)` :62, `unlock(password)` :79, `is_locked()` :44. Tauri wrappers `vault_create` :122 / `vault_unlock` :137 in `vault_commands.rs`.
- Reusable UI: `app/features/ssh/export-picker.js` (`matchesFilter`, `mount`), `styles/design-system/components/picker.css`, `app/ui/tl-combo.js`.

---

### Task 1: Planner reports conflict status

**Files:**
- Modify: `crates/termlab_share/src/import_planner.rs`

**Interfaces:**
- Produces: `ConflictStatus { New, SameId, LabelCollision, ReferenceBroken }`; `ItemAction { Add, Replace, Skip, Rename(String) }`; `PlannedItem<T> { item, status, action }`. Tasks 2-6 consume these.

- [ ] **Step 1: Write the failing tests**

Add to `import_planner.rs`'s existing `mod tests`. The fixture helpers there already build bundles and configs — reuse them rather than writing new ones.

```rust
    #[test]
    fn a_brand_new_server_is_new_and_defaults_to_add() {
        let (bundle, config) = fixture_new_server();
        let p = plan(&bundle, &config, &[]);
        assert_eq!(p.servers[0].status, ConflictStatus::New);
        assert_eq!(p.servers[0].action, ItemAction::Add);
    }

    #[test]
    fn an_existing_id_is_same_id_and_defaults_to_replace() {
        let (bundle, config) = fixture_server_id_exists_ungrouped();
        let p = plan(&bundle, &config, &[]);
        assert_eq!(p.servers[0].status, ConflictStatus::SameId);
        assert_eq!(p.servers[0].action, ItemAction::Replace);
    }

    #[test]
    fn an_existing_folder_nested_id_is_also_same_id() {
        // The pre-existing I2 trap: existence must be decided with
        // config.find_server, which scans folders, not just `ungrouped`.
        let (bundle, config) = fixture_server_id_exists_in_folder();
        let p = plan(&bundle, &config, &[]);
        assert_eq!(p.servers[0].status, ConflictStatus::SameId);
    }

    #[test]
    fn a_colliding_label_with_a_different_id_is_label_collision_and_defaults_to_add() {
        let (bundle, config) = fixture_label_collides_different_id();
        let p = plan(&bundle, &config, &[]);
        assert_eq!(p.servers[0].status, ConflictStatus::LabelCollision);
        assert_eq!(p.servers[0].action, ItemAction::Add);
    }

    #[test]
    fn an_id_match_outranks_a_label_collision() {
        // Same id as a local host AND the same label as a different local
        // host: it is the local item, so SameId wins.
        let (bundle, config) = fixture_id_match_and_label_collision();
        let p = plan(&bundle, &config, &[]);
        assert_eq!(p.servers[0].status, ConflictStatus::SameId);
        assert_eq!(p.servers[0].action, ItemAction::Replace);
    }

    #[test]
    fn an_unresolvable_tunnel_is_kept_as_a_reference_broken_row() {
        // It used to be dropped into ImportPlan::skipped. A dropped row is how
        // a user ends up wondering where a tunnel went, so it now survives with
        // status ReferenceBroken and action Skip.
        let (bundle, config) = fixture_tunnel_pointing_nowhere();
        let p = plan(&bundle, &config, &[]);
        assert_eq!(p.tunnels.len(), 1, "the row must survive, not be dropped");
        assert_eq!(p.tunnels[0].status, ConflictStatus::ReferenceBroken);
        assert_eq!(p.tunnels[0].action, ItemAction::Skip);
        assert!(p.skipped.is_empty(), "no longer reported via `skipped`");
    }

    #[test]
    fn a_tunnel_whose_host_is_in_the_bundle_resolves() {
        let (bundle, config) = fixture_tunnel_host_in_bundle();
        let p = plan(&bundle, &config, &[]);
        assert_eq!(p.tunnels[0].status, ConflictStatus::New);
        assert_eq!(p.tunnels[0].action, ItemAction::Add);
    }

    #[test]
    fn an_existing_account_id_is_same_id() {
        let (bundle, config, account_id) = fixture_account_exists();
        let p = plan(&bundle, &config, &[account_id]);
        assert_eq!(p.accounts[0].status, ConflictStatus::SameId);
        assert_eq!(p.accounts[0].action, ItemAction::Replace);
    }
```

Write the eight fixture helpers in the same module. Each returns a `(ShareBundle, SshConfig)` (plus a `Uuid` for the account case) built from the module's existing sample-bundle helper, varying only the piece under test. `ConflictStatus` and `ItemAction` need `#[derive(Debug, PartialEq)]` for these assertions.

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p termlab_share import_planner`
Expected: FAIL to compile — `ConflictStatus` does not exist.

- [ ] **Step 3: Implement**

```rust
// No serde/ts-rs derives: termlab_share is a pure domain crate with neither
// dependency, and the Tauri layer maps this to a string for the frontend
// (ImportPreviewRow::status in Task 3). Keep it that way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictStatus {
    New,
    SameId,
    LabelCollision,
    ReferenceBroken,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ItemAction {
    Add,
    Replace,
    Skip,
    Rename(String),
}

pub struct PlannedItem<T> {
    pub item: T,
    pub status: ConflictStatus,
    pub action: ItemAction,
}

/// Status and default action for an item, given whether its id already exists
/// locally and whether its label collides with a *different* local item.
///
/// An id match outranks a label collision: the item IS the local one, whatever
/// it is currently called.
fn classify(id_exists: bool, label_collides: bool) -> (ConflictStatus, ItemAction) {
    if id_exists {
        (ConflictStatus::SameId, ItemAction::Replace)
    } else if label_collides {
        (ConflictStatus::LabelCollision, ItemAction::Add)
    } else {
        (ConflictStatus::New, ItemAction::Add)
    }
}
```

Replace `action_for` with `classify` at all four call sites (folder entries, servers, tunnels, accounts). For each, compute `label_collides` by scanning the local collection for an item with the same label **and a different id** — servers via `config.all_servers()`, tunnels via `config.tunnels`, accounts via the caller-supplied list (which carries ids only, so accounts can never report `LabelCollision`; pass `false`).

For tunnels, replace the `skipped.push(...); continue;` at :142-160 with a `ReferenceBroken` row:

```rust
        let unresolvable = matches!(&item.server_entry_id, Some(host_id)
            if !bundle_host_ids.contains(host_id.as_str())
                && config.find_server(host_id).is_none());
        let (status, action) = if unresolvable {
            (ConflictStatus::ReferenceBroken, ItemAction::Skip)
        } else {
            let label_collides = config
                .tunnels
                .iter()
                .any(|t| t.label == item.label && t.id != item.id);
            classify(config.tunnels.iter().any(|t| t.id == item.id), label_collides)
        };
```

- [ ] **Step 4: Run tests and commit**

```bash
cargo test -p termlab_share 2>&1 | tail -5
cargo clippy -p termlab_share --all-targets 2>&1 | grep -E "^(warning|error)" | head
git add crates/termlab_share/src/import_planner.rs
git commit -m "feat(share): report conflict status per planned import item"
```

---

### Task 2: Executor honours Skip and Rename

**Files:**
- Modify: `crates/termlab_share/src/import_executor.rs`

**Interfaces:**
- Consumes: Task 1's `ItemAction` and `PlannedItem`.
- Produces: no new public API; `execute` handles all four arms.

- [ ] **Step 1: Write the failing tests**

```rust
    #[test]
    fn skip_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = SshConfig::default();
        let mut plan = plan_with_one_new_server();
        plan.servers[0].action = ItemAction::Skip;
        let out = execute(plan, &mut config, dir.path(), None).unwrap();
        assert!(config.ungrouped.is_empty(), "a skipped server must not be written");
        assert_eq!(out.servers, 0, "and must not be counted as imported");
    }

    #[test]
    fn rename_changes_the_label_and_preserves_the_id() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = SshConfig::default();
        let mut plan = plan_with_one_new_server(); // id "s1", label "prod"
        plan.servers[0].action = ItemAction::Rename("prod (2)".into());
        execute(plan, &mut config, dir.path(), None).unwrap();
        assert_eq!(config.ungrouped[0].label, "prod (2)");
        assert_eq!(config.ungrouped[0].id, "s1", "the id must survive a rename");
    }

    #[test]
    fn a_tunnel_still_resolves_to_a_host_that_was_renamed() {
        // Tunnels reference hosts by id within a bundle. A rename that changed
        // the id would orphan them, which is why the id is preserved above.
        let dir = tempfile::tempdir().unwrap();
        let mut config = SshConfig::default();
        let mut plan = plan_with_server_and_dependent_tunnel(); // tunnel -> "s1"
        plan.servers[0].action = ItemAction::Rename("renamed".into());
        execute(plan, &mut config, dir.path(), None).unwrap();
        assert_eq!(config.tunnels.len(), 1, "the tunnel must still import");
        assert_eq!(config.tunnels[0].server_entry_id.as_deref(), Some("s1"));
    }

    #[test]
    fn skip_on_a_folder_nested_entry_leaves_the_rest_of_the_folder_alone() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = SshConfig::default();
        let mut plan = plan_with_folder_of_two(); // entries "a" and "b"
        plan.folders[0].entries[0].action = ItemAction::Skip;
        execute(plan, &mut config, dir.path(), None).unwrap();
        let entries = &config.folders[0].entries;
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, "b");
    }
```

Write `plan_with_one_new_server`, `plan_with_server_and_dependent_tunnel` and `plan_with_folder_of_two` as fixtures in the same module, building `ImportPlan` values directly.

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p termlab_share import_executor`
Expected: FAIL. Note that Task 1 had to add `Skip => {}` and a `Rename(_)` stub to these matches already — widening the enum broke their exhaustiveness and the workspace would not compile otherwise. `Skip` is therefore already correct; the `Rename(_)` stub writes the item's **original** label, so `rename_changes_the_label_and_preserves_the_id` fails honestly rather than passing for the wrong reason. Your job is to replace that stub with a real implementation, not to add the arms from scratch.

- [ ] **Step 3: Implement**

In `apply_folder`, `apply_server_entry` and `apply_tunnel`, handle all four arms:

- `Add` — push (existing behaviour).
- `Replace` — find by id and overwrite in place, falling back to push if absent (existing behaviour).
- `Skip` — return without touching `config`, and do not increment the count.
- `Rename(label)` — set the item's `label` to that string, leave its `id` untouched, then apply it as `Add` when the id is absent locally or `Replace` when present. (A `LabelCollision` item by definition has no local id match, so this is `Add` in practice; the id check keeps it correct if a caller sets `Rename` on something else.)

`ImportOutcome`'s counts must reflect what was actually written, so a skipped item is excluded.

- [ ] **Step 4: Run tests and commit**

```bash
cargo test -p termlab_share 2>&1 | tail -5
cargo check --target x86_64-pc-windows-msvc -p termlab_share 2>&1 | tail -2
git add crates/termlab_share/src/import_executor.rs
git commit -m "feat(share): honour Skip and Rename in the import executor"
```

---

### Task 3: Split the import command into plan and apply

**Files:**
- Modify: `crates/termlab_tauri/src/share_commands.rs`, `crates/termlab_tauri/src/lib.rs`

**Interfaces:**
- Consumes: Tasks 1-2.
- Produces: commands `share_import_plan(path, password) -> ImportPreview` and `share_import_apply(path, password, decisions) -> ShareImportSummary`, plus:

```rust
#[derive(serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub(crate) struct ImportPreviewRow {
    pub kind: String,          // "host" | "tunnel" | "credential"
    pub id: String,
    pub label: String,
    pub detail: String,        // user@host:port for a host, L… → … for a tunnel
    pub status: String,        // "new" | "same_id" | "label_collision" | "reference_broken"
    pub default_action: String, // "add" | "replace" | "skip"
}

#[derive(serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub(crate) struct ImportPreview {
    pub rows: Vec<ImportPreviewRow>,
    pub includes_credentials: bool,
    pub vault_state: String,   // "absent" | "locked" | "unlocked"
}

#[derive(serde::Deserialize)]
pub(crate) struct ImportDecision {
    pub kind: String,
    pub id: String,
    pub action: String,        // "add" | "replace" | "skip" | "rename"
    pub label: Option<String>, // required when action == "rename"
}
```

Task 5 consumes `ImportPreview`/`ImportPreviewRow` and sends `ImportDecision`s.

- [ ] **Step 1: Implement `share_import_plan`**

Decode with the supplied password, plan against the current config and the vault's account ids, and flatten to rows: folder entries and ungrouped servers both as `kind: "host"`, tunnels as `"tunnel"`, accounts as `"credential"`. Keys produce no rows. Set `vault_state` from `VaultManager` — `"absent"` when no vault file exists, `"locked"`, or `"unlocked"`. Mutate nothing. Zeroize the password before returning.

- [ ] **Step 2: Implement `share_import_apply`**

Re-read the file, re-decode, re-plan, then overlay the decisions:

```rust
// Decisions are matched to re-planned rows by (kind, id). Re-planning rather
// than caching the decoded bundle between the two calls keeps plaintext key
// material out of long-lived state; the cost is that the plan may have moved
// under us, so a decision for a row that no longer exists is ignored and a row
// that appeared keeps its default.
```

Apply each decision by setting the matching `PlannedItem::action`, mapping `"rename"` to `ItemAction::Rename(label)` and rejecting a rename with no label as `Err("Rename requires a label")`. Then run `execute` exactly as `do_import` does today, including the `VaultSink` gating, `try_save_config`, and the `ssh-config-changed` emit. Delete the old `share_import` command and its registration once nothing calls it.

- [ ] **Step 3: Verify and commit**

```bash
cargo test --workspace 2>&1 | grep -cE "^test result: ok"
grep -rn "share_import\b" crates/termlab_tauri/src crates/termlab_tauri/frontend/app | grep -v "share_import_plan\|share_import_apply"   # expect empty
cargo build -p termlab_tauri 2>&1 | tail -1
git add crates/termlab_tauri/src/share_commands.rs crates/termlab_tauri/src/lib.rs crates/termlab_tauri/frontend/types
git commit -m "feat(share): split import into plan and apply commands"
```

---

### Task 4: Inline vault step in the import dialog

**Files:**
- Modify: `crates/termlab_tauri/frontend/app/panels/ssh-panel.js` (`importConfig` :827, `runImport` :864)

**Interfaces:**
- Consumes: `ImportPreview.vault_state` from Task 3.
- Produces: a vault step that resolves before the preview opens.

- [ ] **Step 1: Implement the step**

After `share_import_plan` returns and only when `includes_credentials` is true, branch on `vault_state`:

- `"unlocked"` — continue straight to the preview.
- `"locked"` — a `tl-dialog` with one password field and an Unlock button, calling the existing `vault_unlock` command; on `Incorrect master password` re-show with an inline error.
- `"absent"` — a `tl-dialog` with master password + confirm and the exact copy from the spec: *"This bundle contains saved credentials. To store them you need a vault on this machine. Pick a master password — you'll use it to unlock the vault from now on."* Submits through `vault_create`. Enable the button only when both fields are non-empty and equal, driving the button's live `disabled` property (tl-dialog's footer gate reads that property, not the value passed at open time).

Cancelling at this step aborts the import having written nothing. Do not implement password-strength rules here — whatever `vault_create` enforces is the rule.

- [ ] **Step 2: Verify and commit**

```bash
node --check crates/termlab_tauri/frontend/app/panels/ssh-panel.js
cargo build -p termlab_tauri && (RUST_LOG=info ./target/debug/termlab > /tmp/tl-t4.log 2>&1 &) && sleep 8 && pkill -f target/debug/termlab; grep -ci "error\|panic" /tmp/tl-t4.log
git add crates/termlab_tauri/frontend/app/panels/ssh-panel.js
git commit -m "feat(share): create or unlock the vault inline during import"
```

---

### Task 5: Preview table with per-row and bulk actions

**Files:**
- Create: `crates/termlab_tauri/frontend/app/features/ssh/import-preview.js`
- Create: `scripts/tests/test_import_preview.mjs`
- Modify: `crates/termlab_tauri/frontend/app/panels/ssh-panel.js`, `crates/termlab_tauri/frontend/index.html`
- Modify: `crates/termlab_tauri/frontend/styles/design-system/components/picker.css`

**Interfaces:**
- Consumes: `ImportPreview` rows from Task 3.
- Produces: `window.termlabImportPreview = { mount, summarise, suggestRename }`, where `mount(container, rows)` returns `{ decisions() }` yielding the `ImportDecision` array Task 3 expects. `summarise` and `suggestRename` are pure and tested.

- [ ] **Step 1: Write the failing tests for the pure helpers**

`scripts/tests/test_import_preview.mjs`, following `scripts/tests/test_export_picker.mjs`'s vm-sandbox pattern:

```js
const { summarise, suggestRename } = sandbox.termlabImportPreview;

// summarise counts by action, in the order the footer displays them.
assert.strictEqual(
  summarise([{ action: 'add' }, { action: 'add' }, { action: 'replace' }, { action: 'skip' }, { action: 'rename' }]),
  '2 new, 1 replace, 1 skip, 1 rename',
);
assert.strictEqual(summarise([{ action: 'add' }]), '1 new');
assert.strictEqual(summarise([]), 'Nothing to import');

// suggestRename appends a counter and keeps going until the name is free.
assert.strictEqual(suggestRename('prod', ['prod']), 'prod (2)');
assert.strictEqual(suggestRename('prod', ['prod', 'prod (2)']), 'prod (3)');
assert.strictEqual(suggestRename('prod', []), 'prod');
assert.strictEqual(suggestRename('prod', ['other']), 'prod');
```

- [ ] **Step 2: Run to verify failure, then implement the module**

Run: `node scripts/tests/test_import_preview.mjs` — fails, module absent.

`mount` renders one row per entry inside a `.tl-picker__box` with the export picker's filter above it (reuse `window.termlabExportPicker.matchesFilter` rather than reimplementing the matching rule). Each row shows Type, Label, Detail, a Status pill, and a `tl-combo` of the actions valid for that status per the Global Constraints table; a `reference_broken` row's control is present but `disabled`. Above the list, one bulk control per status present in the rows, labelled with its count — `3 already exist: [Replace all ▾]` — applying to the rows of that status that the filter currently shows. Choosing `rename` reveals an inline text input pre-filled with `suggestRename(label, existingLabels)`. A footer element shows `summarise(...)`, recomputed on every change.

Add a `.tl-picker__status` pill style to `picker.css` using tokens only.

- [ ] **Step 3: Wire it into the dialog**

`runImport` becomes: `share_import_plan` → vault step (Task 4) → preview dialog (`size: 'lg'`) → on confirm, `share_import_apply(path, password, preview.decisions())` → existing `showImportSummary`. Register the new script in `index.html`.

- [ ] **Step 4: Verify and commit**

```bash
node scripts/tests/test_import_preview.mjs
node --check crates/termlab_tauri/frontend/app/features/ssh/import-preview.js crates/termlab_tauri/frontend/app/panels/ssh-panel.js
grep -c "#[0-9a-fA-F]\{3,6\}" crates/termlab_tauri/frontend/styles/design-system/components/picker.css   # expect 0
cargo build -p termlab_tauri && (RUST_LOG=info ./target/debug/termlab > /tmp/tl-t5.log 2>&1 &) && sleep 8 && pkill -f target/debug/termlab; grep -ci "error\|panic" /tmp/tl-t5.log
git add crates/termlab_tauri/frontend/app/features/ssh/import-preview.js scripts/tests/test_import_preview.mjs crates/termlab_tauri/frontend/app/panels/ssh-panel.js crates/termlab_tauri/frontend/index.html crates/termlab_tauri/frontend/styles/design-system/components/picker.css
git commit -m "feat(share): import preview with per-row and bulk conflict actions"
```

---

### Task 6: End-to-end conflict round trip

**Files:**
- Modify: `crates/termlab_share/tests/round_trip.rs`

**Interfaces:** consumes Tasks 1-2.

- [ ] **Step 1: Write the test**

Seed a machine-B `SshConfig` that hits every status at once: a host with the same id as one in the bundle; a host whose label matches a *different* bundle host; and leave the bundle carrying a tunnel whose host is in neither place. Export from machine A with credentials, encode, decode, plan against machine B, and assert:

- the four statuses are each produced, on the rows expected;
- the defaults are `Add` / `Replace` / `Add` / `Skip` respectively;
- executing the plan unmodified leaves the `ReferenceBroken` tunnel unimported and the `SameId` host updated in place rather than duplicated;
- then re-plan, override the `SameId` row to `Skip` and the `LabelCollision` row to `Rename("prod (2)")`, execute, and assert the local host kept its original values while the renamed host landed with a new label and its bundle id.

- [ ] **Step 2: Run, then commit**

```bash
cargo test -p termlab_share --test round_trip 2>&1 | tail -5
cargo test --workspace 2>&1 | grep -cE "^test result: ok"
git add crates/termlab_share/tests/round_trip.rs
git commit -m "test(share): end-to-end import conflict round trip"
```

---

## Exit criteria

- Every planned item carries a status and a default action; the four statuses are produced by real fixtures.
- `Skip` writes nothing; `Rename` changes the label and preserves the id, and a dependent tunnel still resolves.
- Import is two commands; the apply step re-plans rather than caching a decoded bundle.
- A recipient with no vault can create one inside the import and receive credentials.
- The preview is bounded and filterable, with per-row and per-status bulk actions and a live summary.
- `cargo test --workspace` green; the four cross-checkable crates clean for `x86_64-pc-windows-msvc`.
- Human visual acceptance of the preview and vault steps is the final gate.

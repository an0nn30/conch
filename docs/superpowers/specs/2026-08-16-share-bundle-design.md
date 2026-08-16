# Share: Encrypted Export/Import Bundles — Design

**Status:** Design
**Date:** 2026-08-16
**Scope:** Sub-project 1 of 2 — the `.termlabshare` bundle format, encrypted export with credentials, and a round-trip import with simple apply semantics. Conflict resolution UI is sub-project 2.
**Source requirements:** `~/projects/TermLab/docs/plans/2026-04-14-share-export-import.md` (the JVM implementation this ports).

## Goals

- Export selected SSH hosts and tunnels, optionally with the credentials they reference, into one encrypted file.
- Import that file on another machine — including a machine with no vault — and end up with working connections.
- Make person-to-person sharing work, not just one user's machine-to-machine sync. A bundle must work on a clean install.
- Reuse the vault's crypto exactly. No new primitives, no second implementation.
- Stop the current silent duplication: importing the same data twice must not double it.

## Non-Goals

**Deferred to sub-project 2:** the four-status conflict model (new / same UUID / label collision / broken reference), the per-row preview table with Skip / Replace / Rename, "apply to all remaining conflicts", and the inline vault-creation step during import.

**Out of scope entirely (v1):** signing or authorship verification — the bundle password is the only trust mechanism; partial vault export of accounts no selected host references; cross-file rollback (each file writes atomically, but a failure part-way through leaves partial state); cloud sync or pairing; writing back to `~/.ssh/config`; wildcard or `Match` blocks when reading `~/.ssh/config`; editing a bundle after creation; key rotation; any unencrypted export path; reading bundle metadata without the password.

## Decisions taken before this spec

These were settled with the user on 2026-08-16 and are not open for re-litigation during implementation:

1. **Export is encrypted-only.** No plaintext export path survives. **Import still accepts the legacy plaintext `termlab-connections.json`**, so existing files stay usable.
2. **Bundles embed private key material.** Export reads the key file; import materialises it to disk. Without this, key-based hosts — nearly all of them — arrive broken.
3. **Import preserves UUIDs** rather than regenerating them, which is what makes conflict detection possible at all.
4. **Full JVM conflict parity is the target**, delivered in sub-project 2.

## Current state this builds on

- `remote_export` (`crates/termlab_tauri/src/remote/server_commands.rs:154`) writes **plaintext JSON** via `SshConfig::to_export_filtered`, which deliberately strips `vault_account_id` (see the `export_strips_vault_account_id` test). Credentials never leave today.
- `remote_import` (`:198`) reads that JSON and calls `SshConfig::merge_import` (`crates/termlab_remote/src/config.rs:347`), which **regenerates every UUID and appends** — so a second import duplicates everything, and tunnel→host links are repaired afterwards by value-matching in `resolve_imported_tunnel_keys`.
- The export selection dialog already exists in `app/panels/ssh-panel.js:400` (checkbox tree of folders, ungrouped hosts, `~/.ssh/config` entries, and tunnels) and already folds selected `~/.ssh/config` entries into the export as ungrouped servers. **The host half of the alias-conversion problem the JVM spec solves with an `SshConfigReader` is therefore already solved here** — no new config parser is needed for hosts. What still needs handling is a *tunnel* that points at a `~/.ssh/config` alias: `SavedTunnel` references its server through `server_entry_id` (new) or the legacy `session_key` string (`user@host:port`), so the export planner must pull the referenced alias in as a real `ServerEntry` and point the tunnel at it by id, the same way it pulls in an unselected internal host.
- The vault (`crates/termlab_vault/`) provides AES-256-GCM + Argon2id, an 8-byte magic (`TRMLBVLT`), 16-byte salt, 12-byte nonce, and `AuthMethod::{Password, Key { path, passphrase }, KeyAndPassword { key_path, passphrase, password }}`. **Key material is never stored — only paths.** `GeneratedKeyEntry` is likewise path-based.

## Architecture

A new crate, `crates/termlab_share`, depending on `termlab_remote` and `termlab_vault`. Five units with hard seams:

| Unit | Responsibility | I/O? |
|---|---|---|
| `bundle` | The `ShareBundle` type and its serde shape. Schema version, metadata, hosts, tunnels, credentials, embedded keys. | none |
| `codec` | `encode(&ShareBundle, password) -> Vec<u8>` and `decode(&[u8], password) -> Result<ShareBundle>`. Envelope framing only; delegates crypto. | none |
| `export_planner` | Selection → resolved `ShareBundle`: dependency pull, credential resolution, key embedding, warnings. | reads key files |
| `import_planner` | Decoded bundle + current state → an ordered apply plan. Pure; sub-project 2 extends its output with conflict statuses. | none |
| `import_executor` | Applies a plan. **The only unit that mutates user state.** | writes config + vault + keys |

The Tauri layer holds commands only — no logic. Everything but the executor is unit-testable without touching the filesystem.

### One supporting change in `termlab_vault`

`encrypt_vault`/`decrypt_vault` are typed to `Vault`, so the share codec cannot reuse them directly. Extract the framing and crypto into a generic pair in `termlab_vault::encryption`:

```rust
pub fn encrypt_blob(magic: &[u8; 8], version: u32, plaintext: &[u8], password: &[u8])
    -> Result<Vec<u8>, VaultError>;
pub fn decrypt_blob(
    expected_magic: &[u8; 8],
    legacy_magic: Option<&[u8; 8]>,
    data: &[u8],
    password: &[u8],
) -> Result<(u32, Vec<u8>), VaultError>;
```

The `legacy_magic` parameter exists because the vault must keep accepting its historical `CONCHVLT` magic; bundles pass `None`.

`encrypt_vault`/`decrypt_vault` become thin wrappers over these, keeping their existing behaviour and their legacy-magic (`CONCHVLT`) fallback. This is a refactor of working code in service of the current goal, not opportunistic cleanup: it is what keeps the crypto single-sourced.

## Bundle format

Extension `.termlabshare`. Layout mirrors the vault file so the framing is shared:

```
[ MAGIC        8 bytes  = "TRMLBSHR" ]
[ VERSION      u32 LE   = 1          ]
[ SALT        16 bytes               ]
[ NONCE       12 bytes               ]
[ CIPHERTEXT  AES-256-GCM, variable  ]
```

Note: the JVM spec specifies `MAGIC (8 bytes, "TERMLABSHR")`, but that literal is 10 bytes. We use `TRMLBSHR`, which is 8 and matches the vault's `TRMLBVLT` convention.

Decrypted, the ciphertext is one JSON document:

```json
{
  "schema_version": 1,
  "metadata": {
    "created_at": "2026-08-16T10:32:00Z",
    "source_host": "dustin-mbp",
    "termlab_version": "<crate version at export time>",
    "includes_credentials": true
  },
  "folders":  [ /* ServerFolder, existing serde shape */ ],
  "servers":  [ /* ServerEntry,  existing serde shape */ ],
  "tunnels":  [ /* SavedTunnel,  existing serde shape */ ],
  "vault": {
    "accounts": [ /* VaultAccount, existing serde shape */ ],
    "keys": [
      {
        "id": "uuid",
        "original_path": "~/.ssh/id_ed25519",
        "material": "<base64 of the private key bytes>",
        "public_material": "<base64, optional>",
        "passphrase": "optional string",
        "comment": "id_ed25519"
      }
    ]
  }
}
```

Rules:

- Reuse the existing serde shapes of `ServerFolder`, `ServerEntry`, `SavedTunnel`, `VaultAccount`. No parallel schema.
- `includes_credentials` is metadata for display. The authoritative test is whether `vault.accounts` and `vault.keys` are both empty.
- Argon2id parameters come from `termlab_vault`'s existing `derive_key`. Never re-specified here.
- `schema_version` 1. An unknown version decodes far enough to read the version and then fails with "This bundle was created by a newer version of TermLab" — the version lives inside the envelope header, not the ciphertext, so this check happens before decryption is attempted.
- Always encrypted; a password is always required at both ends.

## Export

**Entry points:** the existing Export item in the app menu, plus an Export action in the Hosts and Tunnels tool windows. All open one dialog, built on `tl-dialog` and the design-system controls.

**Dialog** — the existing selection tree from `ssh-panel.js:400`, plus:

- a checkbox: *"Include saved credentials (the recipient will receive passwords and private keys)"*, off by default;
- password and confirm fields, always required, with the hint *"Anyone with this password can read everything in the bundle."*;
- an Export button, disabled until at least one item is selected and the two passwords match and are non-empty.

**Planning** (`export_planner`, on confirm):

1. **Pull dependencies.** For each selected tunnel, pull in the host it references even if unselected — resolving `server_entry_id` first, then falling back to matching the legacy `session_key` against known hosts and `~/.ssh/config` entries. A tunnel whose host resolves to a `~/.ssh/config` alias exports that alias as a real `ServerEntry` and rewrites the tunnel's `server_entry_id` to it, in the bundle only. Record each auto-pull for the preview; a tunnel whose host cannot be resolved at all is a warning, and the tunnel exports with its `session_key` intact.
2. **Resolve credentials, if included.** Unlock the vault if locked (cancelling the unlock cancels the export with no side effects). For each selected host with a `vault_account_id`, copy that `VaultAccount` into the bundle.
3. **Embed key material.** For each copied account whose auth is `Key` or `KeyAndPassword`, read the file at its path, base64 it into `vault.keys`, and rewrite the account's path to reference the bundled key by id. Any stored passphrase travels with it. Also covers a host's legacy `key_path` field.
4. **Downgrade when credentials are excluded.** Strip `vault_account_id` from every host, exactly as `to_export_filtered` does today, so the recipient is prompted at connect time.
5. **Preview.** A modal listing what was auto-pulled, which keys were embedded, and every warning. The user confirms or cancels.

**Writing.** `codec::encode`, then a native save dialog defaulting to `termlab-share-YYYY-MM-DD.termlabshare`, written temp-file-then-rename so a partial bundle is never left behind.

**Warnings, never hard failures:** key file missing, unreadable, or not a recognised private key. The host still exports; the recipient fixes it or is prompted.

## Import (v1 semantics)

**Entry points:** the existing Import item and tool-window actions. The file picker offers `*.termlabshare` and `*.json`.

- **`.json`** → the existing legacy path, unchanged: `merge_import` with regenerated UUIDs.
- **`.termlabshare`** → password prompt → `codec::decode`. Wrong password, bad magic, and unknown schema version each re-show the prompt with a specific inline error.

**Apply semantics for v1** — deliberately simple, and replaced wholesale by sub-project 2's plan:

1. UUIDs are preserved as they appear in the bundle.
2. An item whose UUID already exists is **replaced**; an item whose UUID is new is **added**. This is what stops the current duplication.
3. Bundled keys are written to `~/.config/termlab/keys/<key-id>` with mode `0600` (Windows: the app config directory with default ACLs), and each account's path is rewritten to point there. An existing file at that path is left alone and reused — key ids are UUIDs, so a collision means the same key.
4. Vault items are applied before hosts, hosts before tunnels, so references resolve. A tunnel referencing a host that is neither in the bundle nor already present is skipped with a log line.
5. If the bundle carries credentials and no vault exists yet, v1 **stops with a clear message** telling the user to create a vault first. Inline creation is sub-project 2. Hosts and tunnels still import; only credentials are held back.
6. Each file keeps its existing atomic write. A mid-run failure stops and reports; partial state is accepted.

**Result:** a summary — *"Imported 5 hosts, 3 tunnels, 4 credentials. 1 skipped."* — after which both tool windows refresh.

## Error handling

| Situation | Behaviour |
|---|---|
| Wrong bundle password | "Incorrect password", prompt re-shown |
| Not a bundle / bad magic | "Not a valid TermLab share bundle" |
| `schema_version` > 1 | "This bundle was created by a newer version of TermLab" |
| Vault locked at export | Unlock prompt; cancel aborts the export cleanly |
| Key file missing/unreadable at export | Warning in preview; host still exported |
| Credentials present, no vault at import | Hosts and tunnels import; credentials reported as held back |
| Write failure mid-import | Stop, report, accept partial state |

## Testing

**Unit, in `crates/termlab_share`:**

- `codec` — round-trip with a known password; wrong password fails cleanly; truncated ciphertext, wrong magic, and schema versions 0 and 999 each fail with the right error; an encoded bundle decodes byte-equivalently.
- `export_planner` — tunnel pulls in its host; credentials-on copies the referenced account; key material is embedded and the account path rewritten; credentials-off strips `vault_account_id`; a missing key file produces a warning rather than an error. Table-driven, with the filesystem behind a trait so no real keys are touched.
- `import_planner` — new UUIDs add, existing UUIDs replace, a tunnel with an unresolvable host reference is skipped.

**Integration, against a temp directory:** full round-trip from a seeded machine-A state into an empty machine-B state, asserting hosts, tunnels, accounts and key files all arrive; re-importing the same bundle is idempotent rather than duplicating; a credentials bundle imported with no vault imports hosts and reports credentials held back.

**Frontend:** the export dialog's enable/disable rule (selection non-empty, passwords match) in the existing stubbed-DOM harness under `scripts/tests/`.

**Not tested:** the native file dialogs, and the vault's crypto primitives, which have their own tests.

## Security notes

- A bundle containing keys is as sensitive as `~/.ssh` itself. The password hint text says so plainly, and the credentials checkbox is off by default.
- Key material and passphrases live in memory only as long as the planner needs them; the bundle types implement `Zeroize` the way `AuthMethod` already does.
- Materialised keys are written `0600` before any content is written to them, never world-readable even briefly.
- No bundle metadata is readable without the password — everything except the envelope header is inside the ciphertext.

## Sub-project 2 (next spec)

The four-status conflict model, the preview table with per-row Skip / Replace / Rename and apply-to-all, and inline vault creation during import. It builds on this spec's `import_planner`, extending its output rather than replacing it.

# Light Editor LSP Apple Silicon Proof of Concept Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a bundled, local-file-only LSP proof of concept to TermLab's light editor on Apple Silicon macOS, proving TypeScript/JavaScript and Rust completion, diagnostics, hover, signature help, go-to-definition, navigation history, project-root/trust UX, and a workspace-wide Problems tool window.

**Architecture:** A Rust-owned, application-wide actor manages canonical document ownership, project context, trust, LSP server processes, document synchronization, and diagnostics. The plain-IIFE frontend is a thin CodeMirror adapter that reserves a local URI before reading it, streams versioned UTF-16 changes, renders normalized results, and exposes non-pushy project context controls. TypeScript and rust-analyzer run exclusively from verified app resources; no server or runtime is discovered on `PATH` or downloaded at runtime.

**Tech Stack:** Tauri 2, Rust/Tokio, `async-lsp` 0.2.4 using `async_lsp::lsp_types`, Ropey, TOML, plain JavaScript IIFEs, CodeMirror 6, npm/esbuild, macOS arm64 app resources.

**Spec:** [2026-08-24-light-editor-lsp-design.md](../specs/2026-08-24-light-editor-lsp-design.md)

## Global Constraints

- Preserve plain editing when LSP is disabled, untrusted, unavailable, starting, failed, or timing out.
- Local files only. Remote editor buffers never enter the ownership registry and never attach to an LSP session.
- Treat canonical local URI ownership as an application-wide invariant, independent of whether LSP is enabled or trusted.
- Never invoke an executable from `PATH`, install a runtime, or download a server while the app is running.
- Do not hold a mutex guard across an `.await`. The manager actor owns mutable state; session tasks communicate through channels.
- Use `async_lsp::lsp_types` everywhere. Do not add a direct, differently versioned `lsp-types` dependency.
- Convert between CodeMirror UTF-16 offsets and LSP positions in Rust. All non-ASCII and multiline conversions require tests.
- Keep source operations under the current 5 MiB editor limit and apply multi-change transactions from highest offset to lowest against a single pre-change snapshot.
- Use typed, normalized frontend payloads. Do not expose raw `lsp_types` values through Tauri events.
- Every server start is gated by both a remembered project root decision and explicit trust for that canonical root.
- The Problems store is authoritative in Rust and replaces diagnostics per `(session, URI, server revision)`; the UI does not merge stale arrays.
- This plan targets Apple Silicon (`aarch64-apple-darwin`) only. JSON, Python, Go, C/C++, Java, and universal macOS packaging are follow-up plans, not unfinished work in this one.

## File and Responsibility Map

| Area | Files | Responsibility |
| --- | --- | --- |
| Rust configuration | `crates/termlab_core/src/config/editor.rs`, `crates/termlab_core/src/config/termlab.rs` | LSP enablement, per-language flags, suggestion behavior, keyboard defaults |
| LSP protocol core | `crates/termlab_tauri/src/lsp/{mod,types,document,root,trust,ownership,catalog,client,session,diagnostics,manager,commands}.rs` | Protocol types, root/trust policy, URI ownership, process/session lifecycle, normalized diagnostics, actor API |
| Tauri wiring | `crates/termlab_tauri/src/lib.rs`, `crates/termlab_tauri/Cargo.toml` | Managed state, commands, events, dependencies |
| Editor integration | `crates/termlab_tauri/frontend/app/features/editor/{editor-pane,editor-service,lsp-bridge,lsp-state,lsp-completion,lsp-tooltips,lsp-diagnostics,lsp-navigation,project-context}.js`, `crates/termlab_tauri/frontend/app/{tab-manager,pane-manager,manager-compose-runtime}.js` | Thin CodeMirror adapter and local document lifecycle |
| Problems UI | `crates/termlab_tauri/frontend/app/panels/problems-panel.js`, `crates/termlab_tauri/frontend/app/tool-window-runtime.js` | Workspace diagnostics list, grouping, filtering, navigation |
| LSP styling | `crates/termlab_tauri/frontend/styles/design-system/components/editor.css`, `crates/termlab_tauri/frontend/styles/design-system/components/problems.css` | Token-based status strip, overlays, diagnostic surfaces, and Problems density/focus states |
| Settings/shortcuts | `crates/termlab_tauri/frontend/app/features/settings/{sections-editor,store,constants}.js`, `crates/termlab_tauri/frontend/app/shortcut-runtime.js` | LSP preferences, trust management, keyboard actions |
| Frontend loading/vendor | `crates/termlab_tauri/frontend/index.html`, `crates/termlab_tauri/frontend/package.json`, `crates/termlab_tauri/frontend/package-lock.json`, `crates/termlab_tauri/frontend/vendor-entry.mjs` | Deterministic script order and explicit CodeMirror completion/lint APIs |
| Packaging | `packaging/lsp/manifest.toml`, `packaging/lsp/node/{package,package-lock}.json`, `scripts/lsp/fetch-macos-arm64.sh`, `scripts/lsp/smoke-macos-arm64.sh`, `crates/termlab_tauri/tauri.conf.json`, `Makefile`, `.gitignore` | Verified arm64 Node/TypeScript/rust-analyzer resource assembly and smoke tests |
| Tests | Rust inline unit tests plus `scripts/tests/test_lsp_*.mjs` and `scripts/tests/test_problems_panel.mjs` | Fast contract, lifecycle, rendering, and interaction coverage |

---

### Task 1: Add stable configuration and keyboard contracts

**Files:**

- Modify: `crates/termlab_core/src/config/editor.rs`
- Modify: `crates/termlab_core/src/config/termlab.rs`
- Modify: `crates/termlab_core/src/config/mod.rs`
- Modify: `crates/termlab_tauri/frontend/app/features/settings/sections-editor.js`
- Modify: `crates/termlab_tauri/frontend/app/features/settings/store.js`
- Modify: `crates/termlab_tauri/frontend/app/features/settings/constants.js`
- Modify: `crates/termlab_tauri/frontend/app/shortcut-runtime.js`
- Test: `scripts/tests/test_lsp_settings.mjs`

- [ ] **Step 1: Write failing Rust configuration tests**

Add tests proving an old config with only `editor.vim_mode` deserializes with LSP enabled, all curated language flags enabled, and the approved shortcuts. Also prove a user's explicit false values survive a round trip.

```rust
#[test]
fn old_editor_config_gets_lsp_defaults() {
    let cfg: UserConfig = toml::from_str("[editor]\nvim_mode = true\n").unwrap();
    assert!(cfg.editor.lsp.enabled);
    assert!(cfg.editor.lsp.languages.typescript);
    assert!(cfg.editor.lsp.languages.rust);
    assert!(cfg.editor.lsp.languages.java);
    assert_eq!(cfg.termlab.keyboard.editor_completion, "ctrl+space");
    assert_eq!(cfg.termlab.keyboard.editor_go_to_definition, "f12");
}
```

- [ ] **Step 2: Run the focused Rust test and confirm it fails**

Run: `cargo test -p termlab_core old_editor_config_gets_lsp_defaults -- --nocapture`

Expected: compilation fails because the LSP and keyboard fields do not exist.

- [ ] **Step 3: Implement serde-defaulted configuration types**

Add `LspConfig` and `LspLanguageConfig` with explicit `Default` implementations. Include flags for TypeScript/JavaScript, JSON, Python, Rust, Go, C/C++, and Java now so persisted config remains stable when the later catalog plan lands. Add these exact keyboard fields and defaults:

```rust
editor_completion: "ctrl+space",
editor_signature_help: "cmd+shift+space",
editor_go_to_definition: "f12",
editor_navigate_back: "ctrl+-",
editor_navigate_forward: "ctrl+shift+-",
editor_next_problem: "f8",
editor_previous_problem: "shift+f8",
```

Keep hover command-palette-only by giving it no default chord.

- [ ] **Step 4: Write a failing frontend settings contract test**

Create a VM-based test following the existing settings tests. Assert that Editor settings expose a master LSP toggle, suggestions-while-typing, all curated language toggles, and the seven keyboard actions; assert no hover shortcut is registered.

- [ ] **Step 5: Run the frontend test and confirm it fails**

Run: `node scripts/tests/test_lsp_settings.mjs`

Expected: the LSP controls and shortcut actions are absent.

- [ ] **Step 6: Add settings controls, labels, search terms, and scoped actions**

Use the existing Editor section primitives. Add LSP actions to `KEYBOARD_CORE_LABELS` and the Editor keyboard group. Mark completion, signature, definition, history, and problem traversal as editor-scoped in `shortcut-runtime.js`; action handlers may dispatch `termlab:editor-*` custom events until their feature modules land.

- [ ] **Step 7: Run focused and regression tests**

Run:

```bash
cargo test -p termlab_core config -- --nocapture
node scripts/tests/test_lsp_settings.mjs
node scripts/tests/test_shortcut_save_fallthrough.mjs
```

Expected: all pass.

- [ ] **Step 8: Commit**

```bash
git add crates/termlab_core/src/config crates/termlab_tauri/frontend/app/features/settings crates/termlab_tauri/frontend/app/shortcut-runtime.js scripts/tests/test_lsp_settings.mjs
git commit -m "feat: add light editor LSP settings"
```

---

### Task 2: Build the versioned UTF-16 document model

**Files:**

- Modify: `crates/termlab_tauri/Cargo.toml`
- Create: `crates/termlab_tauri/src/lsp/mod.rs`
- Create: `crates/termlab_tauri/src/lsp/types.rs`
- Create: `crates/termlab_tauri/src/lsp/document.rs`
- Modify: `crates/termlab_tauri/src/lib.rs`

- [ ] **Step 1: Add failing offset and transaction tests**

Test ASCII, emoji, combining marks, CRLF, multiline positions, end-of-document, invalid boundaries, version mismatch, and multiple edits from one pre-change snapshot.

```rust
#[test]
fn utf16_offsets_round_trip_across_emoji() {
    let doc = DocumentText::new("a😀b\nç");
    assert_eq!(doc.position_at_utf16_offset(3).unwrap(), Position::new(0, 3));
    assert_eq!(doc.utf16_offset_at(Position::new(1, 1)).unwrap(), 6);
}

#[test]
fn applies_code_mirror_changes_high_to_low() {
    let mut doc = VersionedDocument::new("file:///tmp/a.ts", "abcdef", 1);
    doc.apply_batch(change_batch(1, 2, [(4, 6, "Z"), (1, 2, "XY")])).unwrap();
    assert_eq!(doc.text(), "aXYcdZ");
}
```

- [ ] **Step 2: Run and confirm compilation fails**

Run: `cargo test -p termlab_tauri lsp::document::tests -- --nocapture`

Expected: the `lsp` module and document types do not exist.

- [ ] **Step 3: Add protocol dependencies without a duplicate `lsp-types`**

Add:

```toml
async-lsp = { version = "0.2.4", default-features = false, features = ["omni-trait", "tokio"] }
futures = "0.3"
ropey = "1.6.1"
tower = "0.5"
toml = { workspace = true }
```

Use `async_lsp::lsp_types::{Position, Range, ...}` in every module.

- [ ] **Step 4: Implement normalized request/result types and the Ropey model**

Define serde camelCase types including `LspTextChange`, `LspChangeBatch`, `EditorPosition`, `EditorRange`, completion items, hover blocks, signature help, locations, diagnostics, project candidates, statuses, and command responses. Derive `ts_rs::TS` on the Tauri boundary types and add binding-export tests beside them. Keep every public field frontend-oriented and independent of raw protocol enums.

Implement `VersionedDocument::apply_batch` so it:

1. rejects a stale `base_version`;
2. validates all offsets against the unchanged pre-edit snapshot;
3. computes LSP ranges from that snapshot;
4. sorts changes descending by `from_utf16`;
5. applies them to the Rope;
6. advances exactly to `next_version`.

- [ ] **Step 5: Run focused tests and Clippy**

Run:

```bash
cargo test -p termlab_tauri lsp::document::tests -- --nocapture
cargo clippy -p termlab_tauri --lib -- -D warnings
```

Expected: tests pass. If existing crate warnings prevent `-D warnings`, record them and run `cargo clippy -p termlab_tauri --lib` while ensuring no new LSP warning is emitted.

- [ ] **Step 6: Commit**

```bash
git add crates/termlab_tauri/Cargo.toml Cargo.lock crates/termlab_tauri/src/lsp crates/termlab_tauri/src/lib.rs
git commit -m "feat: add versioned LSP document model"
```

---

### Task 3: Discover nested project-root candidates

**Files:**

- Create: `crates/termlab_tauri/src/lsp/root.rs`
- Modify: `crates/termlab_tauri/src/lsp/mod.rs`

- [ ] **Step 1: Write failing table-driven root tests**

Use temporary directory trees to cover:

- a Rust file below a crate nested in a Cargo workspace, returning both roots nearest-first;
- a TypeScript file below a package nested in a monorepo, returning `tsconfig.json`, `package.json`, and workspace markers without collapsing distinct ancestors;
- Python's `pyproject.toml`, Go's `go.work`/`go.mod`, C/C++'s `compile_commands.json`/CMake, Java's Maven/Gradle markers, and JSON's nearest project marker;
- symlink/canonical path normalization;
- malformed marker contents lowering confidence without aborting discovery;
- a loose file whose immediate parent remains the explicit fallback candidate.

```rust
#[test]
fn rust_returns_crate_and_workspace_candidates() {
    let roots = discover_project_roots(&file, LanguageId::Rust).unwrap();
    assert_eq!(roots.iter().map(|r| r.marker.as_str()).collect::<Vec<_>>(),
               ["Cargo.toml (package)", "Cargo.toml (workspace)"]);
}
```

- [ ] **Step 2: Run and confirm failure**

Run: `cargo test -p termlab_tauri lsp::root::tests -- --nocapture`

Expected: root discovery is undefined.

- [ ] **Step 3: Implement bounded ancestor discovery**

Walk every ancestor from the file's parent to the volume root, collecting all applicable markers. Parse only enough TOML/JSON to distinguish package versus workspace markers; do not recursively enumerate project contents. Deduplicate canonical paths while retaining the most specific reason, keep the immediate parent as `This folder`, and sort nearest-first with adapter precedence such as `go.work` over nested `go.mod`. Return candidates, not a silently selected root.

- [ ] **Step 4: Run tests**

Run: `cargo test -p termlab_tauri lsp::root::tests -- --nocapture`

Expected: all marker and nested-root cases pass.

- [ ] **Step 5: Commit**

```bash
git add crates/termlab_tauri/src/lsp
git commit -m "feat: discover nested LSP project roots"
```

---

### Task 4: Persist project decisions and trust safely

**Files:**

- Create: `crates/termlab_tauri/src/lsp/trust.rs`
- Modify: `crates/termlab_tauri/src/lsp/mod.rs`

- [ ] **Step 1: Write failing policy/store tests**

Cover longest-prefix nested bindings, an explicit disabled binding, trust separated from root selection, revocation, a missing store, malformed TOML failing closed, and persistence through reload.

```rust
#[test]
fn nested_binding_wins_over_parent_workspace() {
    let store = store_with_bindings([
        ("/repo", Root("/repo")),
        ("/repo/crates/api", Root("/repo/crates/api")),
    ]);
    assert_eq!(store.binding_for(Path::new("/repo/crates/api/src/lib.rs")),
               Some(Root(PathBuf::from("/repo/crates/api"))));
}
```

- [ ] **Step 2: Run and confirm failure**

Run: `cargo test -p termlab_tauri lsp::trust::tests -- --nocapture`

Expected: trust store types are missing.

- [ ] **Step 3: Implement the versioned TOML store**

Store records in `<config-dir>/lsp-projects.toml` with canonical paths and a schema version. Model root bindings (`root` or `disabled`) separately from trust (`trusted` or `denied`, timestamp, optional adapter id). Use `termlab_core::config::atomic_write`; never partially rewrite the file. A malformed store must produce a warning status and an empty, untrusted in-memory store, not start a server.

- [ ] **Step 4: Run tests**

Run: `cargo test -p termlab_tauri lsp::trust::tests -- --nocapture`

Expected: all pass, including atomic reload and nested precedence.

- [ ] **Step 5: Commit**

```bash
git add crates/termlab_tauri/src/lsp
git commit -m "feat: persist LSP project context and trust"
```

---

### Task 5: Enforce application-wide local URI ownership

**Files:**

- Create: `crates/termlab_tauri/src/lsp/ownership.rs`
- Modify: `crates/termlab_tauri/src/lsp/types.rs`
- Modify: `crates/termlab_tauri/src/lsp/mod.rs`

- [ ] **Step 1: Write failing reservation state-machine tests**

Test reserve/commit/release, same-window and cross-window conflicts, 30-second reservation expiry using a fake clock, close, atomic local-to-local transfer, transfer collision, local-to-remote release, and the rule that remote paths cannot be reserved.

```rust
#[test]
fn second_window_focuses_existing_owner() {
    let first = registry.reserve(local("/tmp/a.ts"), "main").unwrap();
    registry.commit(first.token(), "pane-1").unwrap();
    assert_eq!(registry.reserve(local("/tmp/a.ts"), "popup").unwrap(),
               ReserveResult::FocusOwner { window_label: "main".into(), pane_id: "pane-1".into() });
}
```

- [ ] **Step 2: Run and confirm failure**

Run: `cargo test -p termlab_tauri lsp::ownership::tests -- --nocapture`

Expected: ownership registry is absent.

- [ ] **Step 3: Implement explicit ownership states**

Use:

```rust
enum UriLease {
    Reserved { token: ReservationId, window_label: String, expires_at: Instant },
    Owned { document_id: DocumentId, window_label: String, pane_id: String },
}
```

Canonicalize before lookup. Reservation release only accepts an uncommitted token. Save As reserves the target before writing; transfer accepts that target reservation token, validates it, then moves ownership atomically after the write/rebind succeeds. Committed ownership is released only by document close or remote transfer.

- [ ] **Step 4: Run tests**

Run: `cargo test -p termlab_tauri lsp::ownership::tests -- --nocapture`

Expected: all state transitions pass and stale tokens cannot release an owner.

- [ ] **Step 5: Commit**

```bash
git add crates/termlab_tauri/src/lsp
git commit -m "feat: reserve local editor documents app wide"
```

---

### Task 6: Define the bundled server catalog and resource resolver

**Files:**

- Create: `crates/termlab_tauri/src/lsp/catalog.rs`
- Modify: `crates/termlab_tauri/src/lsp/types.rs`
- Modify: `crates/termlab_tauri/src/lsp/mod.rs`

- [ ] **Step 1: Write failing catalog/resolution tests**

Assert that JavaScript/TypeScript maps to one Node-backed adapter, Rust maps to rust-analyzer, unsupported POC languages report `notBundledYet`, commands are absolute and beneath the resolved resource root, `PATH` is never consulted, and production resolution does not fall back to a source directory.

```rust
#[test]
fn typescript_command_uses_bundled_node() {
    let cmd = catalog.resolve(LanguageId::TypeScript, Path::new("/App/Resources/lsp/arm64")).unwrap();
    assert_eq!(cmd.program, PathBuf::from("/App/Resources/lsp/arm64/node/bin/node"));
    assert!(cmd.args[0].ends_with("typescript/node_modules/typescript-language-server/lib/cli.mjs"));
    assert_eq!(cmd.args.last().unwrap(), "--stdio");
}
```

- [ ] **Step 2: Run and confirm failure**

Run: `cargo test -p termlab_tauri lsp::catalog::tests -- --nocapture`

Expected: no server catalog exists.

- [ ] **Step 3: Implement immutable adapter descriptors**

Describe language ids/extensions, root rules, executable-relative paths, arguments, initialization options, static workspace configuration, completion trigger normalization, per-root disposable cache/data paths, packaged version/license/notice metadata, supported POC state, and maximum startup time. Only TypeScript/JavaScript and Rust are runnable in this plan; keep the other curated language identities/config flags visible as `notBundledYet` so later plans extend the catalog without changing the frontend contract.

Resolution order must be explicit:

1. test-injected root;
2. packaged Tauri resource directory;
3. `TERMLAB_LSP_RESOURCE_DIR` only in debug/test builds;
4. checked-out `packaging/lsp/dist/arm64` only in debug builds.

Return a typed unavailable status if an expected file is missing or not executable.

- [ ] **Step 4: Run tests**

Run: `cargo test -p termlab_tauri lsp::catalog::tests -- --nocapture`

Expected: catalog and fail-closed resolution tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/termlab_tauri/src/lsp
git commit -m "feat: define bundled LSP server catalog"
```

---

### Task 7: Implement an `async-lsp` session around an injectable transport

**Files:**

- Create: `crates/termlab_tauri/src/lsp/client.rs`
- Create: `crates/termlab_tauri/src/lsp/session.rs`
- Create: `crates/termlab_tauri/src/lsp/test_support.rs`
- Modify: `crates/termlab_tauri/src/lsp/mod.rs`

- [ ] **Step 1: Write a failing in-memory protocol integration test**

Use Tokio duplex streams and an in-process mock language server. Verify initialize/initialized, didOpen, descending incremental didChange, full-document synchronization when requested, no didChange when synchronization is absent, didSave, completion plus completion-item resolve, hover, signature help, definition, publish and pull diagnostics, progress, workspace configuration, dynamic registration/unregistration, shutdown, and exit. The test must assert the advertised client capabilities match the features TermLab implements and unsupported server requests receive method-not-supported.

```rust
#[tokio::test]
async fn session_round_trips_editor_features() {
    let (launcher, observed) = MockServerLauncher::scripted(full_feature_script());
    let session = LspSession::start(test_descriptor(), root(), launcher, sink()).await.unwrap();
    session.did_open(test_document()).await.unwrap();
    assert_eq!(session.completion(position(0, 3)).await.unwrap()[0].label, "console");
    assert!(session.definition(position(0, 3)).await.unwrap().is_some());
    session.shutdown().await.unwrap();
    observed.assert_order(["initialize", "initialized", "textDocument/didOpen", "shutdown", "exit"]);
}
```

- [ ] **Step 2: Run and confirm failure**

Run: `cargo test -p termlab_tauri lsp::session::tests -- --nocapture`

Expected: session and launcher abstractions do not exist.

- [ ] **Step 3: Build the client router and session handle**

Use `async_lsp::MainLoop::new_client`, `Router::from_language_client`, `ConcurrencyLayer`, `CatchUnwindLayer`, and `ServiceBuilder`. Route publishDiagnostics, log/showMessage, and progress into a bounded event channel. Spawn a task that owns `mainloop.run_buffered(stdout, stdin)` and reports exit status.

Expose typed methods for open/change/save/close and the five request families, including completion-item resolve and pull-diagnostics helpers. Enforce:

- initialize: 60 seconds, adapter override up to 120 seconds;
- completion/hover/signature: 5 seconds;
- definition: 10 seconds;
- orderly shutdown: wait up to 3 seconds, then kill the child;
- cancellation by request id when the adapter supports it.

The production launcher uses `tokio::process::Command`, pipes only stdio, sets the canonical root as current directory, sanitizes environment variables that can redirect package/runtime loading, and accepts only an already-resolved absolute executable.

- [ ] **Step 4: Normalize server results at the boundary**

Convert completion edits/snippets/commit characters/additional edits, Markdown/plain hover contents, active signature/parameter, location/link variants, severity/source/code, and related diagnostic information into the frontend types in `types.rs`. Preserve cross-document completion edits as unsupported metadata rather than partially applying them. Reject locations outside `file:` URIs for this phase without failing the request.

- [ ] **Step 5: Run session tests and malformed-response tests**

Run:

```bash
cargo test -p termlab_tauri lsp::session::tests -- --nocapture
cargo test -p termlab_tauri lsp::client::tests -- --nocapture
```

Expected: full mock round trip, timeout, process exit, malformed payload, and shutdown escalation tests pass.

- [ ] **Step 6: Commit**

```bash
git add crates/termlab_tauri/src/lsp crates/termlab_tauri/Cargo.toml Cargo.lock
git commit -m "feat: add managed async LSP sessions"
```

---

### Task 8: Make diagnostics replacement and Problems snapshots deterministic

**Files:**

- Create: `crates/termlab_tauri/src/lsp/diagnostics.rs`
- Modify: `crates/termlab_tauri/src/lsp/types.rs`
- Modify: `crates/termlab_tauri/src/lsp/mod.rs`

- [ ] **Step 1: Write failing diagnostic-store tests**

Cover full replacement for one URI, independent sessions, stale revision rejection, clear-on-empty, clear-on-session-stop, severity totals, canonical URI sorting, and workspace-root filtering.

```rust
#[test]
fn newer_publish_replaces_uri_diagnostics() {
    let mut store = DiagnosticStore::default();
    store.replace(key("ts", "/repo/a.ts"), 4, vec![diag("old")]);
    store.replace(key("ts", "/repo/a.ts"), 5, vec![diag("new")]);
    assert_eq!(store.snapshot(None).items[0].message, "new");
}
```

- [ ] **Step 2: Run and confirm failure**

Run: `cargo test -p termlab_tauri lsp::diagnostics::tests -- --nocapture`

Expected: diagnostics store is missing.

- [ ] **Step 3: Implement authoritative replacement and snapshot APIs**

Key entries by `(session_id, canonical_uri)`, record the most recent accepted document/server revision, and emit monotonically increasing store revisions. Return both flat sorted items and aggregate error/warning/information/hint counts. Keep related information attached to each normalized item.

- [ ] **Step 4: Run tests**

Run: `cargo test -p termlab_tauri lsp::diagnostics::tests -- --nocapture`

Expected: replacements cannot leave stale diagnostics behind.

- [ ] **Step 5: Commit**

```bash
git add crates/termlab_tauri/src/lsp
git commit -m "feat: add workspace LSP diagnostics store"
```

---

### Task 9: Assemble the application-wide manager actor and Tauri API

**Files:**

- Create: `crates/termlab_tauri/src/lsp/manager.rs`
- Create: `crates/termlab_tauri/src/lsp/commands.rs`
- Modify: `crates/termlab_tauri/src/lsp/mod.rs`
- Modify: `crates/termlab_tauri/src/lib.rs`

- [ ] **Step 1: Write failing actor tests with a fake session factory**

Test these end-to-end manager policies without spawning a real process:

- reservation before file read and commit on open;
- focus-owner response for a duplicate open, including a pending reservation that waits for its commit/failure event before tab activation;
- clear root, ambiguous roots, remembered binding, session-only deferral, and disabled binding;
- no process before trust; exactly one shared session after trust;
- separate sessions for nested roots or adapters;
- global and per-language disablement detach documents and stop sessions without deleting trust;
- open/change/close ordering and version mismatch resync response;
- 40 ms maximum change batching;
- save and every interactive request flush pending changes, while a newer same-kind request cancels the older one;
- 2-minute idle shutdown using paused Tokio time;
- server crash clears diagnostics, reports status, and restarts only for an attached trusted document with bounded exponential backoff;
- three crashes within five minutes stop automatic restart until a manual restart command;
- bounded per-session stderr/protocol logs remain memory-only and routine logs never include source text;
- trust revocation detaches documents, clears diagnostics, stops the server, and removes only TermLab-owned disposable server caches;
- Save As ownership transfer and close release.

```rust
#[tokio::test(start_paused = true)]
async fn trusted_documents_share_session_by_adapter_and_root() {
    let harness = ManagerHarness::new();
    harness.trust("/repo", "typescript").await;
    harness.open("/repo/a.ts", "pane-a").await;
    harness.open("/repo/b.ts", "pane-b").await;
    assert_eq!(harness.launch_count("typescript", "/repo"), 1);
}
```

- [ ] **Step 2: Run and confirm failure**

Run: `cargo test -p termlab_tauri lsp::manager::tests -- --nocapture`

Expected: manager actor is missing.

- [ ] **Step 3: Implement the actor and managed handle**

Create a bounded `mpsc` command channel. The actor exclusively owns:

```rust
struct ManagerState {
    ownership: OwnershipRegistry,
    documents: HashMap<DocumentId, ManagedDocument>,
    sessions: HashMap<SessionKey, ManagedSession>,
    diagnostics: DiagnosticStore,
    projects: ProjectStore,
}
```

Session tasks send lifecycle and client notifications back through a separate actor input. Request callers receive typed results through `oneshot` channels. Never block the actor waiting for a server response; register a pending request and complete it from a later actor message. Each managed session owns a fixed-capacity in-memory log ring and crash timestamps; neither survives app exit. All emitted statuses and diagnostic updates carry monotonically increasing revisions plus document/session identity.

- [ ] **Step 4: Implement the typed Tauri command surface**

Expose and register:

```text
editor_reserve_document(path, windowLabel)
editor_release_document(reservationId)
editor_transfer_document(documentId, targetReservationId, windowLabel, paneId)
lsp_open_document(reservationId, paneId, contents, languageId)
lsp_apply_changes(documentId, batch)
lsp_resync_document(documentId, version, contents)
lsp_did_save(documentId)
lsp_close_document(documentId)
lsp_project_candidates(path, languageId)
lsp_set_project_context(documentId, root | disabled | deferForSession)
lsp_set_project_trust(root, adapterId, trusted | denied | revoked)
lsp_completion(documentId, position, trigger)
lsp_hover(documentId, position)
lsp_signature_help(documentId, position, trigger)
lsp_definition(documentId, position)
lsp_problems_snapshot(root?)
lsp_status_snapshot(documentId?)
lsp_restart_session(adapterId, root)
lsp_session_logs(adapterId, root)
lsp_trusted_projects()
lsp_revoke_project_trust(root, adapterId?)
```

Emit normalized `lsp-session-status`, `lsp-diagnostics-updated`, and `editor-document-owner-focused` events. Scope targeted events to the owning webview window; diagnostics may broadcast because Problems is app-wide.

- [ ] **Step 5: Wire shutdown into the Tauri lifecycle**

Manage `LspState` separately from the existing `TauriState`. On app exit, ask the manager to close documents and sessions, honor the 3-second per-process ceiling concurrently, then kill survivors. Do not delay window close for an idle server.

- [ ] **Step 6: Run actor, command serialization, and full Rust tests**

Run:

```bash
cargo test -p termlab_tauri lsp::manager::tests -- --nocapture
cargo test -p termlab_tauri lsp::commands::tests -- --nocapture
cargo test --workspace
```

Expected: all tests pass; existing unrelated warnings are unchanged.

- [ ] **Step 7: Commit**

```bash
git add crates/termlab_tauri/src/lsp crates/termlab_tauri/src/lib.rs
git commit -m "feat: manage app-wide LSP sessions"
```

---

### Task 10: Integrate reservation, project context, trust, and editor lifecycle

**Files:**

- Create: `crates/termlab_tauri/frontend/app/features/editor/lsp-bridge.js`
- Create: `crates/termlab_tauri/frontend/app/features/editor/lsp-state.js`
- Create: `crates/termlab_tauri/frontend/app/features/editor/project-context.js`
- Modify: `crates/termlab_tauri/frontend/app/features/editor/editor-pane.js`
- Modify: `crates/termlab_tauri/frontend/app/features/editor/editor-service.js`
- Modify: `crates/termlab_tauri/frontend/app/tab-manager.js`
- Modify: `crates/termlab_tauri/frontend/app/manager-compose-runtime.js`
- Modify: `crates/termlab_tauri/frontend/app/pane-manager.js`
- Modify: `crates/termlab_tauri/frontend/styles/design-system/components/editor.css`
- Modify: `crates/termlab_tauri/frontend/index.html`
- Test: `scripts/tests/test_lsp_editor_lifecycle.mjs`
- Test: `scripts/tests/test_project_context.mjs`

- [ ] **Step 1: Write failing local-open and Save As lifecycle tests**

Stub Tauri invoke/listen and assert:

- local open reserves before `editor_read_file`;
- focus-owner response skips the read and focuses the existing pane/window;
- read failure releases an uncommitted reservation;
- successful pane construction commits through `lsp_open_document`;
- local close sends `lsp_close_document` before CodeMirror destruction;
- local-to-local Save As reserves the target before writing and transfers ownership only after a successful write/rebind;
- cancelled or failed Save As releases only the target reservation and preserves source ownership;
- untitled-to-local and remote-to-local Save As reserve and commit the new local target after a successful write;
- local-to-remote Save As closes/releases the local document;
- remote open and remote-to-remote save never call any LSP/ownership command.

- [ ] **Step 2: Run and confirm failure**

Run: `node scripts/tests/test_lsp_editor_lifecycle.mjs`

Expected: editor service reads before reservation and has no LSP lifecycle.

- [ ] **Step 3: Add the single frontend bridge and document state model**

`lsp-bridge.js` is the only frontend file allowed to invoke `lsp_*` or `editor_*document` ownership commands. It owns listener unsubscription and maps command errors into stable statuses. `lsp-state.js` stores per-pane `documentId`, version, project candidates, selected root, trust, capabilities, status, and diagnostics revision without duplicating document text.

- [ ] **Step 4: Thread hooks through the editor lifecycle**

Add an `onDocumentTransaction` hook to `editor-pane.js` alongside the existing dirty listener. In `editor-service.js`, implement the reservation protocol before local reads, commit after pane creation, batch CodeMirror changes for at most 40 ms, flush before feature requests/save/close, and handle resync by sending the current full text with an incremented version. Ensure every dirty-close guard still executes before ownership release.

- [ ] **Step 5: Write failing context-control tests**

Cover status text and action menus for loose files, one clear candidate, ambiguous candidates, disabled project context, untrusted root, starting, ready, failed, and unavailable catalog entry. Assert file opening never blocks on a modal.

- [ ] **Step 6: Implement the slim project context control**

Mount it in the editor's status area and style it only with existing design tokens, including visible keyboard focus, narrow-pane truncation, and non-color status labels. A clear root may be displayed as the inferred choice; an ambiguous set shows a quiet `Choose project…` affordance. The chooser lists every reasoned candidate followed by `This folder` and `No project features`. Starting a server always requires an explicit trust action with the executable family and canonical root shown. Include `Not now`, `Edit without language features`, `Change project…`, `Restart language server`, `View server logs`, `Retry`, and `Revoke trust` where applicable.

- [ ] **Step 7: Add trusted-project management to Editor settings**

List canonical root, selected context, adapter, trust state, and last-used time. Revoke immediately through the bridge and update open panes from the status event. Revocation stops the affected session after attached documents are detached while leaving their editor buffers intact.

- [ ] **Step 8: Run focused tests**

Run:

```bash
node scripts/tests/test_lsp_editor_lifecycle.mjs
node scripts/tests/test_project_context.mjs
node scripts/tests/test_editor_unsaved_end_to_end.mjs
```

Expected: all pass, including local/remote boundaries and failure cleanup.

- [ ] **Step 9: Commit**

```bash
git add crates/termlab_tauri/frontend/app/features/editor crates/termlab_tauri/frontend/app/tab-manager.js crates/termlab_tauri/frontend/app/pane-manager.js crates/termlab_tauri/frontend/app/manager-compose-runtime.js crates/termlab_tauri/frontend/app/features/settings crates/termlab_tauri/frontend/styles/design-system/components/editor.css crates/termlab_tauri/frontend/index.html scripts/tests
git commit -m "feat: connect editor documents to LSP projects"
```

---

### Task 11: Add CodeMirror completion with Vim-safe keyboard behavior

**Files:**

- Modify: `crates/termlab_tauri/frontend/package.json`
- Modify: `crates/termlab_tauri/frontend/package-lock.json`
- Modify: `crates/termlab_tauri/frontend/vendor-entry.mjs`
- Create: `crates/termlab_tauri/frontend/app/features/editor/lsp-completion.js`
- Modify: `crates/termlab_tauri/frontend/app/features/editor/editor-pane.js`
- Modify: `crates/termlab_tauri/frontend/app/manager-compose-runtime.js`
- Modify: `crates/termlab_tauri/frontend/index.html`
- Test: `scripts/tests/test_lsp_completion.mjs`

- [ ] **Step 1: Make autocomplete and lint direct, bundled dependencies**

Add `@codemirror/autocomplete` and `@codemirror/lint` explicitly and export only the needed completion constructors/functions from `vendor-entry.mjs`: `autocompletion`, `startCompletion`, `closeCompletion`, `acceptCompletion`, `completionStatus`, and snippet support. Task 12 will add the lint exports; declaring both packages here makes the dependency contract explicit before either feature relies on a transitive install.

- [ ] **Step 2: Write failing completion tests**

Test manual `Ctrl-Space`, configured shortcut dispatch, optional identifier/server trigger characters, stale response cancellation, document-version rejection, prefix/filter text, plain `TextEdit`, `InsertReplaceEdit`, snippets, commit characters, completion-item resolve, same-document non-overlapping additional edits, visible unsupported metadata for cross-document edits, documentation/detail, completion-item kinds, and no request when the pane is remote/not ready.

Add explicit Vim assertions: insert mode can open/accept completion; normal-mode keys retain Vim behavior; Escape closes completion before leaving insert mode only when the popup is open.

- [ ] **Step 3: Run and confirm failure**

Run: `node scripts/tests/test_lsp_completion.mjs`

Expected: completion source and CodeMirror exports are absent.

- [ ] **Step 4: Implement an async CodeMirror completion source**

Flush pending changes, capture `{documentId, version, position}`, invoke the bridge, and discard the result if the pane/document/version changed. Translate normalized items into CodeMirror options; apply the primary edit, snippet, and supported same-document additional edits through one CodeMirror transaction so undo, dirty tracking, and LSP synchronization see one coherent change. Reject cross-document additional edits in this phase with a bounded session-log entry.

Create a dedicated completion keymap extension whose precedence is tested relative to Vim. Never register global DOM key handlers inside this module.

- [ ] **Step 5: Build vendor bundle and run tests**

Run:

```bash
npm run build:vendor --prefix crates/termlab_tauri/frontend
node scripts/tests/test_lsp_completion.mjs
```

Expected: bundle builds and all completion/Vim cases pass.

- [ ] **Step 6: Commit**

```bash
git add crates/termlab_tauri/frontend/package.json crates/termlab_tauri/frontend/package-lock.json crates/termlab_tauri/frontend/vendor-entry.mjs crates/termlab_tauri/frontend/vendor crates/termlab_tauri/frontend/app/features/editor crates/termlab_tauri/frontend/app/manager-compose-runtime.js crates/termlab_tauri/frontend/index.html scripts/tests/test_lsp_completion.mjs
git commit -m "feat: add LSP editor completion"
```

---

### Task 12: Render diagnostics and add the workspace Problems tool window

**Files:**

- Modify: `crates/termlab_tauri/frontend/vendor-entry.mjs`
- Create: `crates/termlab_tauri/frontend/app/features/editor/lsp-diagnostics.js`
- Create: `crates/termlab_tauri/frontend/app/panels/problems-panel.js`
- Create: `crates/termlab_tauri/frontend/styles/design-system/components/problems.css`
- Modify: `crates/termlab_tauri/frontend/app/tool-window-runtime.js`
- Modify: `crates/termlab_tauri/frontend/app/features/editor/editor-pane.js`
- Modify: `crates/termlab_tauri/frontend/index.html`
- Test: `scripts/tests/test_lsp_diagnostics.mjs`
- Test: `scripts/tests/test_problems_panel.mjs`

- [ ] **Step 1: Export CodeMirror lint primitives and write failing marker tests**

Export `linter`, `setDiagnostics`, and severity-compatible helpers from the vendor entry. Test severity mapping, zero-width ranges, multiline ranges, replacement by revision, stale-event rejection, clear events, and tooltip message/source/code rendering.

- [ ] **Step 2: Write failing Problems model/view tests**

Test grouping by project then file, severity filters/counts at global/project/file levels, stable severity/path/position sorting, empty/indexing/disconnected/failed states, session/root labels, keyboard next/previous wrapping, row activation opening/focusing a local file, and live replacement from a newer store revision.

- [ ] **Step 3: Run and confirm failure**

Run:

```bash
node scripts/tests/test_lsp_diagnostics.mjs
node scripts/tests/test_problems_panel.mjs
```

Expected: diagnostics renderer and Problems registration are absent.

- [ ] **Step 4: Implement editor diagnostics as a compartment**

Subscribe once through `lsp-bridge.js`, route URI updates to the owning pane, and dispatch `setDiagnostics` only when the event revision is newer. Do not keep a second merged diagnostics array in each view.

- [ ] **Step 5: Implement and register the Problems tool window**

Register `Problems` as a built-in bottom-zone tool window without auto-activating it on startup; it must inherit docking, resizing, hiding, and pop-out behavior from the normal tool-window manager. Render a dense keyboard-accessible tree/list with severity icon/text, file path relative to root, line/column, source/code, filters, counts, and polished empty/indexing/disconnected/failed states. Style it in `problems.css` with existing tokens and strong row/group focus states. Activation goes through the existing editor open/focus flow, preserving the app-wide ownership protocol.

- [ ] **Step 6: Wire F8 and Shift-F8**

Traverse the currently filtered deterministic Problems snapshot. If a target is in another file/window, focus that owner and reveal/select the diagnostic range. Announce navigation to assistive technology without stealing editor focus after reveal.

- [ ] **Step 7: Build and run tests**

Run:

```bash
npm run build:vendor --prefix crates/termlab_tauri/frontend
node scripts/tests/test_lsp_diagnostics.mjs
node scripts/tests/test_problems_panel.mjs
```

Expected: diagnostics replace cleanly and Problems navigation passes.

- [ ] **Step 8: Commit**

```bash
git add crates/termlab_tauri/frontend/vendor-entry.mjs crates/termlab_tauri/frontend/vendor crates/termlab_tauri/frontend/app crates/termlab_tauri/frontend/styles/design-system/components/problems.css crates/termlab_tauri/frontend/index.html scripts/tests
git commit -m "feat: add LSP diagnostics and Problems window"
```

---

### Task 13: Add hover and signature help without sticky overlays

**Files:**

- Create: `crates/termlab_tauri/frontend/app/features/editor/lsp-tooltips.js`
- Modify: `crates/termlab_tauri/frontend/app/features/editor/editor-pane.js`
- Modify: `crates/termlab_tauri/frontend/app/manager-compose-runtime.js`
- Modify: `crates/termlab_tauri/frontend/styles/design-system/components/editor.css`
- Modify: `crates/termlab_tauri/frontend/index.html`
- Test: `scripts/tests/test_lsp_tooltips.mjs`

- [ ] **Step 1: Write failing hover/signature interaction tests**

Cover hover delay and movement cancellation, manual hover action, Markdown rendered as safe text/allowed formatting, range anchoring, no-result dismissal, signature trigger/retrigger characters, active parameter highlighting, `Cmd-Shift-Space`, Escape, blur, document change, scroll, and stale response rejection.

Implemented behavior, recorded during Task 13 review so a later reader does not
"fix" it back: a document change is a dismissal event for HOVER and for PENDING
requests of either kind. It is deliberately NOT one for a VISIBLE signature
help overlay, which survives the edit with its anchor mapped through the
change — retrigger characters are typed inside the call, so a rule that closed
on every edit would make them unreachable. `docs/superpowers/specs/2026-08-24-light-editor-lsp-design.md:547`
is the controlling language: "dismisses stale content" for hover, alongside
"Signature help opens during calls".

- [ ] **Step 2: Run and confirm failure**

Run: `node scripts/tests/test_lsp_tooltips.mjs`

Expected: tooltip controller does not exist.

- [ ] **Step 3: Implement one overlay controller per editor view**

Use CodeMirror tooltip APIs where available and a single state machine:

```text
closed | pending(kind, request, version) | visible(kind, anchor, payload)
```

Only one LSP overlay may be visible at a time. Flush pending document changes before requests. Cancel pending work and close visible content on the dismissal events above — with the one implemented exception recorded in Step 1: an edit closes hover and cancels pending requests, while a visible signature help overlay is carried forward with its anchor mapped through the change. Sanitize/normalize Markdown; never inject server HTML.

- [ ] **Step 4: Wire automatic signature triggers and explicit actions**

Read trigger characters from server capabilities. Automatic requests occur only in insert/edit mode and when LSP is ready; manual signature help can request at any cursor position. Hover remains discoverable through the command palette and pointer dwell, with no default chord.

Carrying trigger characters to the frontend is this task's job for completion as well as signature help: extend the `LspCapabilities`/`LspStatus` payload with the normalized `completion_trigger_characters` the catalog already computes (`catalog.rs`), so `lsp-completion.js` — which already reads `status.completionTriggerCharacters` and falls back to identifier typing when it is absent — begins opening automatically after `.`, `::` and the rest without further frontend change.

- [ ] **Step 5: Run tests**

Run: `node scripts/tests/test_lsp_tooltips.mjs`

Expected: all cancellation, accessibility, and stale-result cases pass.

- [ ] **Step 6: Commit**

```bash
git add crates/termlab_tauri/frontend/app/features/editor crates/termlab_tauri/frontend/app/manager-compose-runtime.js crates/termlab_tauri/frontend/styles/design-system/components/editor.css crates/termlab_tauri/frontend/index.html scripts/tests/test_lsp_tooltips.mjs
git commit -m "feat: add LSP hover and signature help"
```

---

### Task 14: Add definition navigation and cross-window history

**Files:**

- Create: `crates/termlab_tauri/frontend/app/features/editor/lsp-navigation.js`
- Modify: `crates/termlab_tauri/frontend/app/features/editor/editor-service.js`
- Modify: `crates/termlab_tauri/frontend/app/manager-compose-runtime.js`
- Modify: `crates/termlab_tauri/frontend/index.html`
- Test: `scripts/tests/test_lsp_navigation.mjs`

- [ ] **Step 1: Write failing navigation tests**

Test single and multiple definitions, `LocationLink` selection ranges, current-location capture before navigation, same-file reveal, unopened local file open, existing owner in another window, Command-click, back/forward stacks, forward-stack truncation after a new branch, closed/missing file handling, no result, and rejection of non-file URIs.

- [ ] **Step 2: Run and confirm failure**

Run: `node scripts/tests/test_lsp_navigation.mjs`

Expected: navigation controller is missing.

- [ ] **Step 3: Implement a bounded per-window navigation history**

Store at most 100 entries per webview window of canonical URI, UTF-16 position/range, and preferred owner metadata. Route every target through `editor-service.js` so reservation/focus-owner behavior remains authoritative. Multiple definitions use a lightweight keyboard-navigable chooser with file, line, and context preview anchored near the cursor; never open all results.

- [ ] **Step 4: Wire shortcuts and reveal behavior**

Implement F12, Command-click, Ctrl-minus, and Ctrl-Shift-minus through CodeMirror and the existing shortcut runtime events. Reveal and select the server range, center it, focus the editor, and show a non-blocking status when a file disappeared or a URI is unsupported.

- [ ] **Step 5: Run tests**

Run:

```bash
node scripts/tests/test_lsp_navigation.mjs
node scripts/tests/test_lsp_editor_lifecycle.mjs
```

Expected: all navigation and ownership interactions pass.

- [ ] **Step 6: Commit**

```bash
git add crates/termlab_tauri/frontend/app/features/editor crates/termlab_tauri/frontend/app/manager-compose-runtime.js crates/termlab_tauri/frontend/index.html scripts/tests/test_lsp_navigation.mjs
git commit -m "feat: add LSP definition navigation"
```

---

### Task 15: Reproducibly package Node, TypeScript LS, and rust-analyzer for arm64

**Files:**

- Create: `packaging/lsp/manifest.toml`
- Create: `packaging/lsp/node/package.json`
- Create: `packaging/lsp/node/package-lock.json`
- Create: `scripts/lsp/fetch-macos-arm64.sh`
- Create: `scripts/lsp/smoke-macos-arm64.sh`
- Modify: `.gitignore`
- Modify: `crates/termlab_tauri/tauri.conf.json`
- Modify: `Makefile`
- Modify: `crates/termlab_tauri/src/lsp/catalog.rs`
- Test: `crates/termlab_tauri/src/lsp/catalog.rs`

- [ ] **Step 1: Commit the allowlisted artifact manifest**

Use exact immutable versions and SHA-256 values:

```toml
schema = 1
platform = "macos-arm64"

[[artifact]]
id = "node"
version = "24.19.0"
url = "https://nodejs.org/dist/v24.19.0/node-v24.19.0-darwin-arm64.tar.gz"
sha256 = "8294b7aa9b03997481c06babf1e8b270c859358f27da57a11509afe537ac381d"
license = "MIT"

[[artifact]]
id = "typescript-language-server"
version = "6.0.0"
url = "https://registry.npmjs.org/typescript-language-server/-/typescript-language-server-6.0.0.tgz"
sha256 = "6e23b48efc76af4e70928cdfe62ea6e6cfef67ab4c1e7579c4e82dd284fbdfd2"
license = "MIT"

[[artifact]]
id = "typescript"
version = "7.0.2"
url = "https://registry.npmjs.org/typescript/-/typescript-7.0.2.tgz"
sha256 = "da2513f4b95176d6dde8b51aab7afe8a927656c9d277369793f77f7e59371c08"
license = "Apache-2.0"

[[artifact]]
id = "rust-analyzer"
version = "2026-08-24"
url = "https://github.com/rust-lang/rust-analyzer/releases/download/2026-08-24/rust-analyzer-aarch64-apple-darwin.gz"
sha256 = "5f4557c2ea4d62f80f1ffeea2646d0d56fab7172a0db11f3065c4d246b763989"
license = "MIT OR Apache-2.0"
```

The Node package manifest pins exact `typescript-language-server` and `typescript` versions; generate and commit its lockfile. The fetch script may use the lockfile to include transitive npm dependencies, but it must independently verify the three declared npm package tarball hashes from the npm cache or fetched tarballs.

- [ ] **Step 2: Write the fetch script's failing verification mode first**

Add `--verify-only <resource-root>` that checks expected files, hashes recorded in a generated receipt, executable bits, and architecture using `file`/`lipo`. Run it against an empty temporary directory.

Run: `scripts/lsp/fetch-macos-arm64.sh --verify-only "$(mktemp -d)"`

Expected: nonzero exit naming missing Node and rust-analyzer artifacts.

- [ ] **Step 3: Implement deterministic staging**

The script must:

1. require `uname -m` to be `arm64` unless an explicit test fixture is supplied;
2. download to a `mktemp -d` directory with `curl --fail --location`;
3. verify SHA-256 before extraction;
4. run `npm ci --omit=dev --ignore-scripts` against the committed lockfile into staging;
5. stage Node beneath `node/`, npm packages beneath `typescript/node_modules/`, and rust-analyzer at `rust-analyzer/rust-analyzer`;
6. strip archives, caches, docs, and npm executable shims that are not needed;
7. generate the installed `manifest.json` receipt and `THIRD_PARTY_NOTICES.md` from the verified manifest and included license texts;
8. atomically replace only `packaging/lsp/dist` after all checks pass, yielding `dist/manifest.json`, `dist/THIRD_PARTY_NOTICES.md`, and `dist/arm64/...`.

Add `packaging/lsp/dist/` to `.gitignore`. Do not run the fetch script from `cargo build`, `build.rs`, or app startup.

- [ ] **Step 4: Fetch and verify the resource tree**

Run:

```bash
scripts/lsp/fetch-macos-arm64.sh
scripts/lsp/fetch-macos-arm64.sh --verify-only packaging/lsp/dist/arm64
packaging/lsp/dist/arm64/node/bin/node packaging/lsp/dist/arm64/typescript/node_modules/typescript-language-server/lib/cli.mjs --version
packaging/lsp/dist/arm64/rust-analyzer/rust-analyzer --version
```

Expected: TypeScript language server reports `6.0.0`; rust-analyzer reports the pinned release/build; verification succeeds.

- [ ] **Step 5: Add Tauri and manual app-bundle resource copying**

Map `packaging/lsp/dist/` to `lsp/` in `tauri.conf.json`. Add an `lsp-resources-arm64` Make target used by native arm64 `app`/`dmg-native`; copy the verified tree into `TermLab.app/Contents/Resources/lsp`, sign nested executables/runtime libraries first, and sign the outer app last. Keep universal targets out of this plan and fail with a clear message if this arm64-only resource target is mistakenly used for a universal artifact.

- [ ] **Step 6: Add a packaged-resource smoke script**

The script accepts an `.app`, verifies both executables are arm64 and inside `Contents/Resources`, launches each server with stdio, sends initialize/shutdown/exit framing, and asserts a valid JSON-RPC response. It must scrub `PATH` down to system utilities before server launch so accidental host discovery fails the test.

- [ ] **Step 7: Build and smoke the staged app**

Run:

```bash
make app
scripts/lsp/smoke-macos-arm64.sh build/TermLab.app
codesign --verify --deep --strict --verbose=2 build/TermLab.app
```

Use the actual app output path printed by `make app` if it differs.

Expected: both servers answer initialize from inside the signed app bundle; codesign verification passes.

- [ ] **Step 8: Commit**

```bash
git add packaging/lsp scripts/lsp .gitignore crates/termlab_tauri/tauri.conf.json crates/termlab_tauri/src/lsp/catalog.rs Makefile
git commit -m "build: bundle arm64 TypeScript and Rust LSP servers"
```

---

### Task 16: Complete automated and manual POC acceptance

**Files:**

- Create: `docs/superpowers/notes/light-editor-lsp-poc-checklist.md`
- Modify: `docs/superpowers/specs/2026-08-24-light-editor-lsp-design.md` only if implementation revealed an approved factual correction

- [ ] **Step 1: Run formatting and the complete automated suite**

Run:

```bash
cargo fmt --all -- --check
cargo test --workspace
npm run build:vendor --prefix crates/termlab_tauri/frontend
for test_file in scripts/tests/test_*.mjs; do node "$test_file"; done
scripts/lsp/fetch-macos-arm64.sh --verify-only packaging/lsp/dist/arm64
```

Expected: every command exits zero. Record any pre-existing warnings separately; do not describe a skipped or failing test as passing.

- [ ] **Step 2: Build and smoke the distributable app**

Run:

```bash
make app
scripts/lsp/smoke-macos-arm64.sh build/TermLab.app
codesign --verify --deep --strict --verbose=2 build/TermLab.app
```

Expected: the bundled Node/TypeScript server and rust-analyzer initialize with a scrubbed `PATH`, and the app signature remains valid.

- [ ] **Step 3: Perform the TypeScript/JavaScript manual matrix**

In a nested npm workspace on Apple Silicon:

- open a file and confirm editing is immediate before any project choice;
- choose between package and workspace roots, trust the selected root, and verify the choice persists;
- verify automatic/manual completion, hover, signature help, F12, back/forward, diagnostics, Problems navigation, and Save As transfer;
- open the same canonical file from a popped-out window and confirm the existing owner is focused;
- revoke trust and confirm the editor remains usable while the server stops;
- temporarily move host `node`/TypeScript tools off `PATH` and repeat initialization.

- [ ] **Step 4: Perform the Rust manual matrix**

In a Cargo workspace with multiple member crates:

- verify both crate and workspace candidates appear and the nested remembered root wins;
- trust and start rust-analyzer;
- exercise all five IDE interactions and workspace-wide Problems;
- edit emoji/non-ASCII text before a diagnostic and verify ranges remain aligned;
- close all attached documents, wait over two minutes, and confirm the server shuts down;
- reopen and confirm a new shared session starts.

- [ ] **Step 5: Exercise failure and plain-editor paths**

Verify untrusted, denied, disabled, server-missing, startup-timeout, crash, malformed response, and unsupported curated-language states. In every case confirm local open/save/close, dirty guards, Vim mode, and remote editor behavior still work.

- [ ] **Step 5a: Close the items recorded during Task 12 and Task 13 review**

- Verify popped-out Problems activation focuses the owner and opens the file; range reveal is a recorded degradation pending a `HOST_ACTION_EVENTS` protocol extension. A panel host owns no editor, so `app/features/problems/problems-navigation.js` falls back to the existing `open-in-editor` host action (the same escape hatch `app/panels/files-panel.js` uses), which carries a path and no range. Widening that closed action list is a panel-host protocol change, deliberately out of Task 12's scope.
- I-2 ghost project groups: a session whose last status was `failed`/`unavailable` keeps its group after stop, because no session-level terminal status is emitted (`crates/termlab_tauri/src/lsp/manager.rs:4511` emits per-document only) while `docs/superpowers/specs/2026-08-24-light-editor-lsp-design.md:620` requires the group to disappear. Needs either a session-level status event on stop (Rust) or a frontend prune in `problems-store.js`.
- L-1 signature help can outlive the call it describes: because a visible signature overlay is carried forward across edits with its anchor mapped, deleting the call leaves a zero-width mapped anchor and the overlay stays until Escape, blur, or scroll. Consider closing the overlay when the mapped anchor collapses on a deletion (`app/features/editor/lsp-tooltips.js`, the `overlayField` update).
- L-2 a pointer dwell over text closes an open signature-help overlay: the dwell issues a hover request, and the one-overlay rule closes the other kind. VS Code keeps the hints. Consider suppressing dwell-hover while signature help is visible (`handlePointerMove` in `app/features/editor/lsp-tooltips.js`).
- L-3 extract the Markdown normalize/render block (~190 lines, pure: `isFence`/`plainLine`/`markdownSegments`/`appendSegments`) out of `app/features/editor/lsp-tooltips.js` into its own module the next time that file is opened.

- [ ] **Step 6: Record evidence in the checklist**

For each automated command and manual scenario, record date, machine architecture, app build identifier, result, and any follow-up issue. The checklist is acceptance evidence, not a substitute for tests.

- [ ] **Step 7: Commit**

Commit the completed acceptance documentation after every required result is recorded.

```bash
git add docs/superpowers/notes/light-editor-lsp-poc-checklist.md
git commit -m "docs: record light editor LSP POC verification"
```

---

## POC Acceptance Gate

The proof of concept is complete only when all of the following are true:

- A local TypeScript/JavaScript file and a local Rust file can use completion, diagnostics, hover, signature help, and go-to-definition from bundled servers with a scrubbed `PATH`.
- Ambiguous nested roots are offered without blocking file editing; remembered nested choices take precedence.
- No language server process starts without explicit trust, and revocation works without closing editor buffers.
- A canonical local URI has only one independently editable owner across all TermLab windows, including open and Save As races.
- Diagnostics appear both inline and in the app-wide Problems tool window and are removed by replacement/close/session-stop events.
- Navigation history works across files and windows.
- Remote buffers never attach to LSP and continue to behave as before.
- The signed arm64 `.app` contains and launches only the pinned resource executables.
- Full Rust and frontend test suites pass with no new warnings or regressions.

## Explicit Follow-up Plans

These are intentionally outside this POC and must not be smuggled into its implementation:

1. Bundle and validate JSON, Python, Go, C/C++, and Java adapters, including their runtime/JDK/toolchain footprints and language-specific initialization behavior.
2. Produce universal macOS resources and signing/notarization strategy for both arm64 and x86_64, including per-architecture native servers and Node runtimes.
3. Add remote-file/project LSP transports and remote process lifecycle after the local ownership and trust model has production evidence.

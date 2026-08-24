# Light Editor Local LSP and IDE Features — Design

**Status:** Approved in design review
**Date:** 2026-08-24
**Scope:** Local-file language intelligence for the Tauri 2 light editor, backed by TermLab-bundled language servers on macOS.
**Supersedes:** The completion/IDE non-goal in `2026-08-17-light-editor-design.md` for local files only.

## Summary

TermLab will extend its CodeMirror 6 light editor with a Rust-owned Language
Server Protocol client. The first production-shaped slice supports completion,
diagnostics, hover, signature help, go-to-definition, navigation history, and a
workspace-wide Problems tool window for local files.

The feature remains additive. Opening a file is immediate, and the existing
editor continues to work when project features are disabled, a root is
ambiguous, a project is untrusted, or a server fails. Untitled buffers receive
project features after their first local save. SFTP-backed editor tabs are
explicitly excluded and never attach to a local language server in this phase.

TermLab owns the language-server installation and lifecycle. Users do not
install servers, configure executable paths, or depend on `PATH`. The proof of
concept targets Apple Silicon macOS with bundled TypeScript and Rust servers.
The approved language catalog then expands through the same adapter contract,
and a shipping universal macOS build includes both arm64 and x86_64 resources.

## Goals

1. Provide project-aware completion with automatic triggers, manual
   `Ctrl+Space`, documentation, snippets, and same-document additional edits.
2. Display server diagnostics inline and in one Problems tool window shared
   across active local projects.
3. Provide hover documentation, signature help, go-to-definition, and
   back/forward navigation.
4. Keep server processes, project trust, root selection, document versions,
   diagnostics, cancellation, and failure recovery in a testable Rust boundary.
5. Preserve the light editor's instant single-file workflow when project
   features are not wanted.
6. Bundle and version every supported language server and required runtime with
   TermLab so the installed feature is offline and requires no setup.
7. Establish an adapter contract that can add more languages and remote LSP in
   later phases without redesigning the editor surface.

## Non-Goals

- LSP for SFTP or other remote editor tabs.
- Downloading, updating, or discovering language servers at runtime.
- User-supplied server executables or arbitrary server command configuration.
- Rename, references, formatting, code actions, semantic tokens, inlay hints,
  call hierarchy, or arbitrary workspace edits.
- A general Open Folder/workspace-session model.
- Restoring editor or language-server sessions after app restart.
- Supporting Windows or Linux in this phase.
- Sending source code, diagnostics, usage data, or telemetry off the machine.

## Product Decisions

The following decisions were approved during design review:

- Phase one is local-file only. Remote LSP is a later feature phase.
- The IDE slice includes completion, diagnostics, hover, signature help,
  go-to-definition, navigation history, and a Problems tool window.
- TermLab bundles all runtimes and servers; there is no `PATH` fallback.
- The initial catalog is JavaScript/TypeScript, JSON, Python, Rust, Go, C/C++,
  and Java.
- Files open before project discovery or server startup completes.
- Root discovery considers every plausible ancestor, including nested projects.
- Ambiguous roots require an unobtrusive user choice; remembered choices apply
  automatically.
- A project must be trusted before any language server is spawned.
- macOS is the initial platform; the proof of concept begins on Apple Silicon.
- One canonical local file URI has one independently editable buffer app-wide.

## Architecture

### Ownership

The Tauri process owns one app-wide `LspManager` in managed Rust state:

```text
CodeMirror adapters ── Tauri commands ──▶ LspManager ── stdio ──▶ bundled LSPs
         ▲                                     │
         └──── responses and Tauri events ─────┤
Problems tool window ◀── diagnostic snapshots ─┘
```

The manager owns:

- the bundled-server catalog;
- remembered project-context and trust decisions;
- the app-wide canonical-URI document registry;
- server sessions keyed by `(server adapter, canonical project root)`;
- document text mirrors and monotonically increasing versions;
- in-flight requests and cancellation;
- normalized diagnostics;
- server status, bounded logs, crash history, and idle shutdown.

CodeMirror owns presentation and editor-local interaction only. It does not
spawn processes, frame JSON-RPC, decide trust, retain the diagnostic source of
truth, or infer project roots. `editor-service.js` remains the owner of file
open/save/Save As identity; it tells the LSP bridge when that identity opens,
closes, or atomically rebinds.

### Rust module shape

The Tauri crate gains an `lsp/` module with explicit responsibilities:

```text
lsp/
  mod.rs             managed state and public facade
  commands.rs        typed Tauri command boundary
  catalog.rs         bundled server descriptors and resource validation
  root.rs            language-aware candidate discovery and ranking
  trust.rs           persisted root choice and trust decisions
  manager.rs         document/session routing and app-wide invariants
  document.rs        text mirror, versions, edits, and position conversion
  session.rs         child process, initialization, requests, and shutdown
  client.rs          async-lsp client handlers for server-to-client traffic
  diagnostics.rs     normalized app-wide diagnostic store
  types.rs           serde/ts-rs frontend payloads
```

Use `async-lsp` 0.2.4 for the bidirectional LSP client loop and `lsp-types`
0.97.0 for protocol types. `tower-lsp` is not used because it is specialized
for implementing language servers rather than clients.

### Frontend module shape

The plain-IIFE frontend keeps app code unbundled. New focused modules are:

```text
app/features/editor/
  project-context.js   project/trust status control and chooser
  lsp-bridge.js        typed command/event facade and pane lifecycle
  lsp-codemirror.js    compartments and shared CM extension composition
  lsp-completion.js    completion source and completion-item application
  lsp-tooltips.js      hover and signature-help presentation
  lsp-diagnostics.js   CodeMirror diagnostic decoration adapter
  lsp-navigation.js    definition chooser and back/forward history

app/features/problems/
  problems-model.js    pure grouping, filtering, counts, and selection
  problems-panel.js    tool-window rendering and interaction
```

The CodeMirror vendor bundle adds explicit dependencies and exports for
`@codemirror/autocomplete` and `@codemirror/lint`, plus the view/state APIs
needed for hover and signature tooltips. Transitive installation does not count
as a dependency contract; packages used directly are declared directly.

## Document and Session Model

### URI reservation

Single-buffer ownership applies to every local editor, even when LSP is off or
the project is untrusted. It therefore begins before project attachment and
cannot be implemented as a frontend `focusExistingEditor` check.

Opening a local file uses a Rust reservation protocol:

1. `editor_reserve_document(path, window_label)` canonicalizes the path under
   the same filesystem rules used by editor I/O.
2. If the canonical URI is owned or reserved, Rust returns the existing owner
   and emits the focus request; the caller creates no tab.
3. Otherwise Rust returns an opaque, short-lived reservation id.
4. The frontend reads the file and creates the pane.
5. `lsp_open_document(reservation_id, pane_id, contents, language_id)` commits
   ownership and, independently, attaches project features when context and
   trust permit.
6. A read or pane-creation failure calls `editor_release_document` with the
   uncommitted reservation; abandoned reservations expire after 30 seconds.

Save As reserves the target URI before the existing write/rebind sequence. A
successful rebind atomically transfers ownership from the old URI to the
reserved target before releasing the old URI. A failed or cancelled Save As
releases only the target reservation and leaves the old ownership untouched.
This extends the existing same-window collision refusal to every window without
weakening its failure ordering.

Closing a pane calls `lsp_close_document` after its dirty/save guard has allowed
destruction; that command detaches LSP state and releases committed ownership
idempotently. Untitled and SFTP-backed panes own no canonical local URI until a
successful local Save As.

### Document identity

Every attached document has:

```text
document_id       opaque TermLab identifier
owner_window      Tauri window label
owner_pane        pane id
uri               canonical file:// URI
path              canonical local filesystem path
language_id       LSP language id
project_root      canonical selected root
server_key        adapter id + project root
version           monotonically increasing integer
text              Rust-side mirror
```

Untitled buffers and remote-backed panes have no LSP document identity. A local
Save As performs this ordering after reserving its target URI:

1. Finish the existing atomic file rebind.
2. Atomically transfer app-wide ownership to the reserved target URI.
3. Send `didClose` for the old URI, if attached.
4. Discover or apply context for the new path.
5. Send `didOpen` for the new URI only when its project is selected and trusted.

An LSP failure never rolls back a file save or rebind that already succeeded.
Project attachment is enhancement state, not file identity.

The ordering above describes a local-to-local, untitled-to-local, or
remote-to-local Save As. A successful local-to-remote Save As sends `didClose`
and releases the old local URI without attaching the remote temp path. A
remote-to-remote Save As remains outside LSP entirely.

### Single-buffer rule

LSP defines one open document state per URI within a client session. Two
independent TermLab buffers for the same URI would send conflicting versions
and already risk last-save-wins data loss. The Rust registry therefore grants
one app-wide owner to each canonical URI.

When a second window attempts to open an owned or reserved file, TermLab raises
and focuses the owning window, activates its tab when ownership has committed,
and selects its editor pane. It does not create a second buffer. A request that
finds a still-pending reservation focuses the reserving window and waits for
its success/failure event before attempting tab activation. Canonicalization,
reservation expiry, and ownership live in Rust so separate webview JavaScript
contexts cannot race.

### Server sessions

A session is keyed by the adapter and canonical selected root. Different files
and TermLab windows under the same root share the process. A process starts
only when its first trusted document attaches.

Initialization supplies one workspace folder, the selected root URI, supported
client capabilities, UTF-16 position encoding, adapter initialization options,
and a TermLab client identity. Server-advertised capabilities are stored on the
session and determine which UI features are enabled.

When its last document closes, a session enters a two-minute idle grace period.
A new document cancels idle shutdown. Expiry sends `shutdown`, then `exit`,
waits three seconds, and kills a child that does not exit. App quit performs the
same graceful sequence for every live session in parallel before applying the
same three-second final bound.

## Project Context and Trust

### Root candidates

Root discovery walks every ancestor of the file and returns ranked candidates,
not merely the nearest marker. Each candidate includes its canonical path,
display name, confidence, marker, and human-readable reason.

Language adapters contribute marker rules:

| Language family | Important candidates and precedence |
|---|---|
| JavaScript/TypeScript | `tsconfig.json`, `jsconfig.json`, package roots, `package.json` workspaces, `pnpm-workspace.yaml`, `lerna.json`, and `nx.json` |
| JSON | selected JavaScript/TypeScript context when present; otherwise repository or parent folder |
| Python | `pyproject.toml`, `setup.cfg`, `setup.py`, `tox.ini`, `Pipfile`, `poetry.lock`, `uv.lock`, and repository roots |
| Rust | crate `Cargo.toml` plus enclosing manifests containing `[workspace]` |
| Go | `go.work` ahead of nested `go.mod` |
| C/C++ | `compile_commands.json`, `.clangd`, CMake roots, repository roots |
| Java | Maven `pom.xml`, Gradle settings/build files, and Eclipse `.project` roots |

The immediate parent folder is always an explicit fallback candidate. Generic
repository roots may be candidates but do not erase a more specific language
root. Root parsing must be conservative: a malformed marker can lower
confidence but cannot stop the file from opening.

### Selection behavior

The file opens before discovery finishes. A narrow editor status strip contains
one project-context control:

- `Project: conch` for an active remembered context;
- `Trust conch to enable project features` for one clear but untrusted root;
- `Choose project context…` when multiple roots are plausible;
- `Project features off` when explicitly disabled;
- `Starting`, `Indexing`, `Ready`, or `Failed` server state when active.

The chooser lists every candidate and its reason, followed by `This folder`
and `No project features`. It is non-modal and never steals focus on open.

Selections use longest-prefix matching. A remembered nested choice overrides an
enclosing project choice; siblings remain independent. A selected root may be
changed from the status control at any time. Changing it detaches the document
from the old session and reattaches only after the new root is trusted.

### Trust persistence

No server process starts before trust. The project control explains that local
language tools may invoke compilers, build systems, build scripts, or project
code. `Trust and enable` is explicit.

Persist root bindings and trust decisions in a versioned, atomically written
`<config-dir>/lsp-projects.toml`, protected by a Rust lock. Records use
canonical paths and contain:

```text
binding scope -> selected root | disabled
canonical root -> trusted | denied
```

`Not now` affects only the current document attachment attempt and is not
persisted. A denial is persisted but reversible. Revoking trust immediately
detaches documents, stops sessions whose root is no longer trusted, clears
their diagnostics, and removes disposable per-server project caches. Source
files and user-owned build output are never deleted.

## Bundled Server Catalog

### Approved catalog

| Adapter id | Languages | Server/runtime |
|---|---|---|
| `typescript` | JavaScript, JSX, TypeScript, TSX | `typescript-language-server` + TypeScript on private Node |
| `json` | JSON and JSON-with-comments where supported | VS Code JSON language server on private Node |
| `python` | Python | Pyright on private Node |
| `rust` | Rust | `rust-analyzer` native binary |
| `go` | Go | `gopls` native binary |
| `clangd` | C and C++ | `clangd` native binary |
| `java` | Java | Eclipse JDT LS on a private JRE |

Node-based servers share one TermLab-owned Node runtime. JDT LS uses one
TermLab-owned JRE. No adapter falls back to a system Node, Java, or language
server. Toolchains used by the project itself remain project dependencies; for
example, bundling `rust-analyzer` does not bundle every Rust toolchain.

### Adapter descriptor

Each adapter is data plus a small typed hook surface:

- adapter id and display name;
- supported filenames, extensions, and LSP language ids;
- architecture-specific executable/runtime and arguments;
- initialization options and static client settings;
- root-marker rules and ranking hooks;
- completion trigger-character normalization where required;
- per-root cache/data-directory construction;
- environment additions limited to the child process;
- packaged version, upstream URL, license, and notices;
- initialize, shutdown, and smoke-test timeouts.

Server-specific conditionals belong in adapter modules. The manager never
switches on a language name for lifecycle or protocol behavior.

### macOS resources

Pinned upstream artifacts are fetched during packaging from an allowlisted
manifest with SHA-256 checksums. Generated binaries are not committed to Git.
The installed layout is:

```text
TermLab.app/Contents/Resources/lsp/
  manifest.json
  arm64/
    node/
    jre/
    typescript/
    json/
    pyright/
    rust-analyzer/
    gopls/
    clangd/
    jdtls/
  x86_64/             # required in the universal shipping build
    ...same adapter/runtime contract...
  THIRD_PARTY_NOTICES.md
```

The Apple Silicon proof of concept packages the arm64 TypeScript and Rust
resources first. The complete curated catalog follows through the same
manifest. A production universal macOS release includes both architecture
trees and chooses the matching one at runtime.

Nested executables and runtime libraries are signed before the outer app,
included in notarization validation, and verified by packaging smoke tests.
Server versions change only with a TermLab release. The installed feature makes
no network request and provides no individual server updater.

This distribution materially increases app size, particularly for `clangd`,
the private JRE/JDT LS, and two architecture trees. That is an accepted cost of
offline, zero-setup language support.

## Protocol and Synchronization

### Change transport

CodeMirror sends transactions as:

```text
document_id
base_version
next_version
changes[] = { from_utf16, to_utf16, inserted_text }
```

Offsets describe the pre-transaction document and are sorted from highest to
lowest before application. Applying high offsets first preserves the lower
offsets, allowing the Rust mirror to translate every change against a stable
pre-change snapshot and send a valid ordered incremental LSP change list.

The manager validates `base_version`. On a mismatch it rejects the incremental
batch with `resync_required`; the frontend supplies one full snapshot at the
next version. A mismatch never guesses or silently advances.

The Rust document layer owns UTF-16 offset, line/character, and Rust string
conversion. It advertises UTF-16 to servers and tests ASCII, combining marks,
BMP characters, surrogate pairs, emoji, CR/LF boundaries, and multi-edit
transactions. Position conversion is not duplicated in individual feature
handlers.

If a server requests full synchronization, the manager sends the current full
mirror. If it supports incremental synchronization, the manager sends the
validated changes. A server that advertises no synchronization receives no
`didChange` notifications.

### Ordering and cancellation

Normal typing may be batched for up to 40 milliseconds to reduce
webview-to-Rust IPC. Save and every interactive request first flush pending
changes. For completion, hover, signature help, and definition:

1. Flush document changes.
2. Cancel the older outstanding request of the same kind for that document.
3. Send the LSP request with the current document version.
4. Return a normalized response tagged with that source version.
5. Discard the response if the document version, URI, or project context has
   changed.

Rust sends `$/cancelRequest` when supported and always applies the local stale
result guard. Completion, hover, and signature requests time out after five
seconds; definition requests time out after ten seconds. Initialization uses a
60-second default, with an adapter-declared maximum of 120 seconds for JDT LS.
Timeouts cancel locally and never block editing.

### Supported protocol surface

The first phase handles:

- `initialize`, `initialized`, `shutdown`, and `exit`;
- `textDocument/didOpen`, `didChange`, `didSave`, and `didClose`;
- `textDocument/completion` and `completionItem/resolve`;
- `textDocument/publishDiagnostics` and pull diagnostics when advertised;
- `textDocument/hover`;
- `textDocument/signatureHelp`;
- `textDocument/definition` returning `Location` or `LocationLink`;
- progress creation and `$/progress`;
- server messages and log messages;
- dynamic capability registration/unregistration;
- `workspace/configuration` from adapter-owned settings;
- benign responses for supported server-to-client UI requests.

Unsupported requests receive a correct method-not-supported response. The
client does not claim capabilities it cannot honor.

Completion application supports plain text, snippets, `TextEdit`,
`InsertReplaceEdit`, commit characters, and non-overlapping additional edits
within the same document. Cross-document edits are preserved in the normalized
result but not applied; the item explains that its workspace edit is not yet
supported rather than partially mutating files.

Hover and completion documentation are sanitized before rendering. Definition
targets are limited to local `file://` URIs. A target outside the selected root
may open as a normal local editor, but it does not inherit trust or attach to a
new project without the ordinary context flow.

## Tauri Boundary

Commands use typed serde/`ts-rs` payloads. The initial boundary is:

| Command | Responsibility |
|---|---|
| `editor_reserve_document` | Canonicalize and reserve a local URI, or return/focus its existing owner |
| `editor_release_document` | Release an uncommitted reservation idempotently |
| `editor_transfer_document` | Atomically move ownership to a reserved Save As target |
| `lsp_project_candidates` | Return ranked roots for a canonical local path and language |
| `lsp_set_project_context` | Remember root selection, disabled state, or session-only deferral |
| `lsp_set_project_trust` | Trust, deny, or revoke a canonical root |
| `lsp_open_document` | Commit a reservation to its pane, attach project features when trusted, and return status |
| `lsp_apply_changes` | Validate and apply an incremental batch or request full resync |
| `lsp_resync_document` | Replace the mirror after an explicit version mismatch |
| `lsp_did_save` | Flush changes and notify the owning session after a successful save |
| `lsp_close_document` | Detach protocol state and release committed URI ownership idempotently |
| `lsp_completion` | Request normalized completion data |
| `lsp_hover` | Request normalized hover data |
| `lsp_signature_help` | Request normalized signature data |
| `lsp_definition` | Request normalized local definition targets |
| `lsp_problems_snapshot` | Return a revisioned diagnostic snapshot for active roots |
| `lsp_restart_session` | Restart one failed or active root/server session |
| `lsp_session_logs` | Return the bounded in-memory log for explicit inspection |

App-wide events are revisioned and contain enough identity to ignore stale
delivery:

- `lsp-diagnostics-updated`
- `lsp-session-status`
- `editor-document-owner-focused`

Frontend code calls these commands only through `lsp-bridge.js`. Components do
not scatter raw command strings or Tauri payload conventions.

## Editor Experience

### Status strip

A narrow strip below each editor is visually subordinate to the document. It
shows the project-context control, server state, and diagnostic counts. It does
not appear in terminal panes. Indexing and failures stay non-modal.

The server menu provides:

- restart language server;
- change project context;
- disable project features for this scope;
- view bounded server logs;
- revoke project trust.

### Completion

CodeMirror's autocomplete extension drives an asynchronous source backed by
`lsp_completion`. Automatic completion follows server trigger characters and
ordinary identifier typing. `Ctrl+Space` invokes it manually even when
suggestions-as-you-type are disabled.

The list shows label, kind, detail/source, and documentation. Selection is
keyboard-first. Snippets and supported same-document additional edits apply as
one CodeMirror transaction so undo restores the pre-completion document.

With Vim enabled, automatic completion occurs only while edits are being made.
Completion navigation has priority while its popup is open; Escape closes the
popup before performing any later modal action. This ordering is pinned by
integration and manual tests.

### Hover and signature help

Hover appears after a pointer dwell or keyboard command, uses sanitized
Markdown, and never takes editor focus. Moving the document or caret invalidates
the request and dismisses stale content.

Signature help opens during calls and highlights the active signature and
parameter. Completion wins when both surfaces would collide; signature help
remains available again after completion closes.

### Definition and navigation history

`F12` and Command-click request definitions. A single result opens immediately.
Multiple results use a keyboard-navigable chooser showing file, line, and a
short context preview. Local targets route through the existing editor service,
then select and center the returned range.

Each definition jump records the source location in a per-window navigation
history. Back and Forward commands restore tab, caret, and selection when the
target still exists. Peek definition is not part of this phase.

### Diagnostics

The active editor shows severity-specific underlines, optional gutter markers,
hover messages, and status counts. All visual diagnostics are projections of
the Rust diagnostic store. The frontend does not merge or retain an independent
authoritative list.

Versioned diagnostics older than the current document are rejected. Unversioned
diagnostics are accepted only from the currently attached session and replace
that session's previous publication for the URI. Closing an editor does not
delete valid workspace diagnostics for other files. Stopping or re-rooting a
session clears all records owned by that session.

### Keyboard commands

New configurable fields join the existing `termlab.keyboard` table:

| Field | macOS default | Scope |
|---|---|---|
| `editor_completion` | `ctrl+space` | Focused LSP-capable editor |
| `editor_signature_help` | `cmd+shift+space` | Focused LSP-capable editor |
| `editor_go_to_definition` | `f12` | Focused LSP-capable editor |
| `editor_navigate_back` | `ctrl+-` | Focused editor with history |
| `editor_navigate_forward` | `ctrl+shift+-` | Focused editor with forward history |
| `editor_next_problem` | `f8` | Focused editor with diagnostics |
| `editor_previous_problem` | `shift+f8` | Focused editor with diagnostics |

Show Hover is available in the command palette without a default chord because
the current shortcut router models single combinations, not VS Code-style
multi-step chords. Every editor-scoped binding passes through untouched in a
terminal pane. Native menu accelerators do not claim these keys ahead of the
focused-pane router.

## Problems Tool Window

Problems is registered as a normal TermLab tool window and inherits existing
docking, resizing, hiding, and pop-out behavior. It reads a revisioned snapshot
and subsequent diagnostic events from Rust.

The view provides:

- errors, warnings, information, and hints grouped by project then file;
- severity toggles and text filtering;
- counts at the global, project, and file levels;
- keyboard navigation and accessible group/item labels;
- file, range, source/server, diagnostic code, and sanitized message;
- explicit empty, indexing, server-disconnected, and server-failed states.

Activating a problem focuses the app-wide owner when the file is already open.
Otherwise it opens the local file in the requesting main window, then selects
and centers the range. A stale or deleted target reports a non-destructive
error and leaves the diagnostic visible until the server replaces it.

The window aggregates every active trusted project rather than naming one
global folder as the workspace. A project group disappears when its final
session stops and its diagnostic records are cleared.

## Settings

Settings expose product behavior rather than server paths:

```toml
[editor.lsp]
enabled = true
suggestions_while_typing = true

[editor.lsp.languages]
typescript = true
json = true
python = true
rust = true
go = true
clangd = true
java = true
```

All fields default to true for backward-compatible deserialization, but trust
still gates actual server startup. Disabling project features globally detaches
all documents and stops sessions without deleting remembered trust. Disabling a
language prevents new sessions for that adapter and stops it once its documents
detach. Manual completion remains available when only
`suggestions_while_typing` is false.

Trusted-project management lists canonical roots, selected context, last-used
time, and a revoke action. It never exposes server command-line editing.

## Failure Handling

| Failure | Behavior and recovery |
|---|---|
| Missing/corrupt bundled resource | Disable that adapter, show a specific status, keep plain editing available |
| Initialization timeout or error | Mark the session failed; expose logs and manual Restart |
| Interactive request timeout | Dismiss that response only; editor input remains uninterrupted |
| Server crash | Restart with bounded exponential backoff while preserving document mirrors |
| Repeated crashes | Stop after three crashes in five minutes; require manual Restart |
| Malformed server message | Log and reject the affected message; stop the session if framing is no longer trustworthy |
| Document version mismatch | Request explicit full resynchronization; never guess |
| Capability absent | Hide or disable only the unsupported interaction |
| Trust revoked | Detach, stop, clear diagnostics, and delete disposable TermLab-owned caches |
| Definition target missing | Report a quiet navigation error without changing the current editor |

Server stderr and protocol summaries are kept in a bounded in-memory ring per
session. They are not persisted automatically because they can contain local
paths or project-derived text. Viewing or copying logs is an explicit user
action. Raw source text is not added to routine application logs.

## Verification

### Rust unit tests

- root candidates, ranking, nested workspaces, malformed markers, and fallback;
- longest-prefix context selection and sibling isolation;
- trust canonicalization, persistence, denial, revocation, and atomic writes;
- manifest validation, architecture selection, missing resources, and adapter
  uniqueness;
- app-wide URI ownership and cross-window focus routing;
- incremental edits, resynchronization, versions, and full-sync fallback;
- UTF-16 conversions across ASCII, Unicode, surrogate pairs, and multiple
  changes;
- diagnostic normalization, replacement, version rejection, grouping revision,
  and session clearing;
- idle shutdown, graceful exit, timeouts, restart backoff, crash ceiling, and
  cache isolation.

### Deterministic mock-server integration tests

A small test server speaks real LSP framing over stdio and covers:

- initialize/initialized/shutdown/exit;
- ordered open/change/save/close;
- dynamic capabilities and workspace configuration;
- completion and resolve, including snippets and additional edits;
- push and pull diagnostics;
- hover, signature help, and both definition result shapes;
- cancellation and stale-result rejection;
- server progress and messages;
- malformed JSON-RPC, delayed responses, early exit, crash/restart, and hung
  shutdown.

These tests must not depend on a compiler, network access, or a developer's
installed language servers.

### Frontend tests

Follow the repository's existing VM-sandbox and pure-model conventions:

- project status and chooser state;
- completion normalization and atomic edit application;
- diagnostic-to-CodeMirror mapping and stale revision rejection;
- hover/signature precedence and sanitization;
- definition chooser and navigation history;
- Problems grouping, counts, filters, selection, and empty/failure states;
- shortcut routing, focused-pane scoping, and Vim popup/Escape ordering;
- Save As rebind and close teardown invoking the correct LSP lifecycle calls;
- vendor exports for every directly used CodeMirror API.

### macOS resource smoke tests

The Apple Silicon proof of concept launches the bundled TypeScript and Rust
servers from a staged `.app`, initializes each against a fixture project,
performs one completion or hover request, and shuts it down. The test runs with
a restricted `PATH` and network disabled to prove the resources are genuinely
self-contained.

Every later catalog entry must pass the same staged-app contract. Before a
universal release, the smoke suite runs against both architecture resource
trees and verifies nested signing/notarization inputs.

### Manual verification

1. Open an untrusted project file: editing works immediately and no server
   starts before trust.
2. Trust a clear root; observe start/index/ready without a modal.
3. Open a nested Rust crate and choose between crate and workspace roots;
   reopen and confirm the remembered choice.
4. Exercise automatic/manual completion, snippets, hover, signature help,
   definition, Back, and Forward in TypeScript and Rust fixtures.
5. Produce diagnostics in several files; confirm inline decorations and the
   grouped Problems window agree exactly.
6. Pop out Problems, activate an item, and confirm the owner window/tab focuses.
7. Open the same file from another window and confirm the original owner is
   focused instead of creating a duplicate buffer.
8. Toggle suggestions, a language adapter, and global project features live.
9. Revoke trust while a server is active and confirm processes, diagnostics,
   and disposable caches are removed while editing continues.
10. Crash and hang the mock server; confirm bounded recovery and useful status.
11. Use Vim insert/normal modes with completion, Escape, hover, and navigation.
12. Open a remote SFTP file and confirm it remains a plain light-editor tab with
    no local project attachment.

## Delivery Phases

### Phase 1: Protocol foundation

- Add typed protocol dependencies and the Rust manager/session/document core.
- Implement mock-server integration tests before real-server adapters.
- Add canonical URI ownership, root candidates, trust persistence, and the
  frontend bridge/status control.

### Phase 2: Apple Silicon vertical slice

- Package private Node + TypeScript server and native `rust-analyzer`.
- Implement completion, diagnostics, hover, signature, definition, navigation
  history, and Problems end to end.
- Pass staged-app offline smoke tests for both adapters.

### Phase 3: Curated catalog

- Add JSON and Pyright through the Node runtime.
- Add `gopls` and `clangd` native adapters.
- Add the private JRE and JDT LS adapter.
- Require the same fixture and staged-app contract for every adapter.

### Phase 4: Universal macOS readiness

- Build and package x86_64 resource variants.
- Integrate both trees with the existing universal Tauri release.
- Complete nested signing, notarization, license notices, size reporting, and
  both-architecture smoke validation.

### Later phase: Remote LSP

Remote support gets its own design. It must decide where servers run, how remote
roots and toolchains are discovered, how URI/path translation works, how one
SSH connection is shared, how trust applies to a host, and how diagnostics and
definitions cross that boundary. Phase one does not add dormant remote branches
or treat downloaded SFTP temp files as local projects.

## Accepted Costs and Risks

- Bundling Node, a JRE, `clangd`, and two macOS architecture trees may add
  hundreds of megabytes to the universal app.
- Java and C/C++ server startup/indexing can be slow; the UI must communicate
  progress without blocking.
- Language servers are trusted local tools but may invoke project build systems
  after the user trusts a root. The trust explanation must remain explicit.
- Different servers interpret workspace roots and capabilities differently.
  Adapter isolation and real-server smoke fixtures are required, not optional.
- Sharing sessions across windows requires the app-wide URI ownership rule and
  careful window-focus routing.
- CodeMirror/Vim keymap precedence is fragile and must be pinned by tests and a
  manual pass whenever autocomplete dependencies change.

These costs are accepted for a zero-setup, offline, project-aware editor. They
do not justify moving protocol state into the frontend or silently depending on
developer-installed tools.

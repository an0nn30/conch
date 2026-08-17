# Light Editor Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a light text editor to the app — scratch files, local files, and SFTP-triggered remote file editing — as tabs beside terminal tabs.

**Architecture:** CodeMirror 6 is bundled by esbuild into a single IIFE global (`window.CM6`), introducing the frontend's first build step, scoped strictly to third-party dependencies. The editor becomes a third pane kind (`kind: 'editor'`) alongside `terminal` and `plugin_view`, so it inherits tabs, splits, and drag-and-drop. All file guards (size cap, extension blocklist, binary sniff) and temp-path resolution live in Rust.

**Tech Stack:** Rust + Tauri 2, vanilla IIFE frontend (no bundler for app code), CodeMirror 6, esbuild, Node/npm (new dev dependency).

**Spec:** `docs/superpowers/specs/2026-08-17-light-editor-design.md`

## Global Constraints

- **The app's own frontend modules stay IIFE with `<script>` tags in `index.html`.** The build step covers third-party dependencies only. Do not convert app code to ESM, do not add imports between app modules.
- **`MAX_EDIT_BYTES = 5 * 1024 * 1024`** (5 MB), exact.
- **Extension blocklist, exact and case-insensitive:** `png jpg jpeg gif bmp ico webp svg zip tar gz tgz bz2 xz 7z rar jar war ear class exe dll so dylib pdf doc docx xls xlsx ppt pptx mp3 mp4 mov avi mkv wav flac pyc pyo`
- **Binary sniff:** any `0x00` byte in the **first 8192 bytes**.
- **Guards live in Rust only.** The frontend never carries a copy of the blocklist or the size cap.
- **CSS uses design tokens (`--tl-*`) only.** Raw hex is allowed nowhere except a `var()` fallback in `base.css`.
- **No `@codemirror/autocomplete`.** Completions are an explicit non-goal.
- **`npm ci`, never `npm install`,** in every scripted/CI context — the lockfile is the pin.
- **Frontend tests are plain Node scripts** run as `node scripts/tests/test_<name>.mjs`, using `node:assert` and `node:vm`, following `scripts/tests/test_window_size.mjs`. There is no jsdom and none is to be added.
- **`deepStrictEqual` fails on objects built inside a `node:vm` sandbox** (cross-realm prototypes). Compare fields individually.
- **Toast API is `window.toast.error(title, body)`** and `.warn` / `.success` / `.info`.
- **Dialogs use `window.tlDialog.open({ title, ariaLabel, size, body, buttons, onClose })`,** which returns a handle with `.close(reason)`.
- **Run `cargo clean -p termlab_tauri` if a build reports a symbol missing that exists on disk** — stale artifacts have bitten this repo before.

---

## File Structure

**Created:**

| Path | Responsibility |
|---|---|
| `crates/termlab_tauri/frontend/package.json` | Declares esbuild + CodeMirror devDependencies and the `build:vendor` script. |
| `crates/termlab_tauri/frontend/build-vendor.mjs` | esbuild driver producing the IIFE bundle. |
| `crates/termlab_tauri/frontend/vendor-entry.mjs` | The ESM entry re-exporting the CodeMirror surface the app uses. |
| `crates/termlab_tauri/src/editor_fs.rs` | Guards, binary sniff, temp-path resolution, read/write commands. |
| `crates/termlab_tauri/frontend/app/features/editor/language-map.js` | Filename → CodeMirror language key. Pure. |
| `crates/termlab_tauri/frontend/app/features/editor/theme.js` | Design tokens → CodeMirror theme + highlight style. |
| `crates/termlab_tauri/frontend/app/features/editor/editor-pane.js` | Owns an `EditorView` inside a pane element; dirty tracking; focus. |
| `crates/termlab_tauri/frontend/app/features/editor/editor-service.js` | Open/save orchestration for local, scratch, and remote files. |
| `crates/termlab_tauri/frontend/app/features/editor/scratch.js` | Scratch naming. Pure naming function + creation. |
| `crates/termlab_tauri/frontend/styles/design-system/components/editor.css` | Editor pane chrome. Tokens only. |
| `scripts/tests/test_language_map.mjs` | Tests for `language-map.js`. |
| `scripts/tests/test_scratch_naming.mjs` | Tests for `scratch.js`'s naming function. |

**Modified:**

| Path | Change |
|---|---|
| `crates/termlab_tauri/tauri.conf.json` | Add `beforeBuildCommand` / `beforeDevCommand`. |
| `.gitignore` | Ignore the generated bundle and `node_modules`. |
| `.github/workflows/release.yml` | `setup-node` in four jobs; explicit vendor step in the two Linux jobs. |
| `crates/termlab_tauri/frontend/index.html` | Script tags for the bundle and the new app modules. |
| `crates/termlab_tauri/src/lib.rs` | Register editor commands; add `WindowEvent::CloseRequested`. |
| `crates/termlab_core/src/config/termlab.rs` | Two new keymap fields. |
| `crates/termlab_tauri/frontend/app/tab-manager.js` | Editor tab creation; editor arm in `closeTab`; dirty guard. |
| `crates/termlab_tauri/frontend/app/pane-manager.js` | Editor arms in focus and close paths. |
| `crates/termlab_tauri/frontend/app/clipboard-runtime.js` | Editor arm for paste. |
| `crates/termlab_tauri/frontend/app/config-runtime.js` | Editor arm for font/theme reapplication. |
| `crates/termlab_tauri/frontend/app/main-runtime.js` | Editor arm in the font/fit loop. |
| `crates/termlab_tauri/frontend/app/shortcut-runtime.js` | Two new entries in `coreShortcutActionByKey`. |
| `crates/termlab_tauri/frontend/app/menu-actions.js` | `new-scratch` and `save-file` actions; quit guard. |
| `crates/termlab_tauri/frontend/app/command-palette-runtime.js` | Palette entry for New Scratch. |
| `crates/termlab_tauri/frontend/app/core/dialog-service.js` | `confirmSave(fileName)`. |
| `crates/termlab_tauri/frontend/app/features/files/pane-view.js:76` | File-row double-click opens an editor. |

---

## Task 1: The Build Step and the CodeMirror Bundle

**Files:**
- Create: `crates/termlab_tauri/frontend/package.json`
- Create: `crates/termlab_tauri/frontend/vendor-entry.mjs`
- Create: `crates/termlab_tauri/frontend/build-vendor.mjs`
- Modify: `crates/termlab_tauri/tauri.conf.json`
- Modify: `.gitignore`
- Modify: `.github/workflows/release.yml`
- Modify: `crates/termlab_tauri/frontend/index.html`

**Interfaces:**
- Produces: `window.CM6`, an object with the named exports listed in `vendor-entry.mjs` below. Every later task reads CodeMirror through this global and nothing else.

- [ ] **Step 1: Write the vendor entry module**

Create `crates/termlab_tauri/frontend/vendor-entry.mjs`. This is the complete list of CodeMirror surface the app may use — later tasks must not reach for anything absent here without adding it to this file.

```js
// The ESM entry esbuild bundles into vendor/codemirror/codemirror.js as the
// IIFE global `CM6`. This file is the app's entire CodeMirror API surface:
// if a module needs something not re-exported here, add it here first.
export { EditorState, Compartment } from '@codemirror/state';
export {
  EditorView, keymap, lineNumbers, highlightActiveLine,
  highlightActiveLineGutter, drawSelection, rectangularSelection,
  highlightSpecialChars,
} from '@codemirror/view';
export { defaultKeymap, history, historyKeymap, indentWithTab } from '@codemirror/commands';
export {
  syntaxHighlighting, HighlightStyle, StreamLanguage,
  bracketMatching, indentOnInput, foldGutter, foldKeymap,
} from '@codemirror/language';
export { searchKeymap, highlightSelectionMatches } from '@codemirror/search';
export { tags } from '@lezer/highlight';

export { javascript } from '@codemirror/lang-javascript';
export { json } from '@codemirror/lang-json';
export { python } from '@codemirror/lang-python';
export { markdown } from '@codemirror/lang-markdown';
export { rust } from '@codemirror/lang-rust';
export { html } from '@codemirror/lang-html';
export { css } from '@codemirror/lang-css';
export { xml } from '@codemirror/lang-xml';
export { yaml } from '@codemirror/lang-yaml';
export { sql } from '@codemirror/lang-sql';
export { java } from '@codemirror/lang-java';
export { cpp } from '@codemirror/lang-cpp';
export { go } from '@codemirror/lang-go';
export { php } from '@codemirror/lang-php';

export { shell } from '@codemirror/legacy-modes/mode/shell';
export { toml } from '@codemirror/legacy-modes/mode/toml';
export { dockerFile } from '@codemirror/legacy-modes/mode/dockerfile';
export { lua } from '@codemirror/legacy-modes/mode/lua';
export { ruby } from '@codemirror/legacy-modes/mode/ruby';
export { perl } from '@codemirror/legacy-modes/mode/perl';
export { powerShell } from '@codemirror/legacy-modes/mode/powershell';
export { nginx } from '@codemirror/legacy-modes/mode/nginx';
export { properties } from '@codemirror/legacy-modes/mode/properties';
export { diff } from '@codemirror/legacy-modes/mode/diff';
```

- [ ] **Step 2: Write `package.json`**

Create `crates/termlab_tauri/frontend/package.json`. `private: true` — this is never published.

```json
{
  "name": "termlab-frontend-vendor",
  "version": "0.0.0",
  "private": true,
  "description": "Bundles third-party frontend dependencies. The app's own modules are plain IIFE scripts and are NOT built.",
  "scripts": {
    "build:vendor": "node build-vendor.mjs"
  },
  "devDependencies": {
    "esbuild": "^0.24.0",
    "@codemirror/state": "^6.5.0",
    "@codemirror/view": "^6.35.0",
    "@codemirror/commands": "^6.7.0",
    "@codemirror/language": "^6.10.0",
    "@codemirror/search": "^6.5.0",
    "@codemirror/legacy-modes": "^6.4.0",
    "@lezer/highlight": "^1.2.0",
    "@codemirror/lang-javascript": "^6.2.0",
    "@codemirror/lang-json": "^6.0.0",
    "@codemirror/lang-python": "^6.1.0",
    "@codemirror/lang-markdown": "^6.3.0",
    "@codemirror/lang-rust": "^6.0.0",
    "@codemirror/lang-html": "^6.4.0",
    "@codemirror/lang-css": "^6.3.0",
    "@codemirror/lang-xml": "^6.1.0",
    "@codemirror/lang-yaml": "^6.1.0",
    "@codemirror/lang-sql": "^6.8.0",
    "@codemirror/lang-java": "^6.0.0",
    "@codemirror/lang-cpp": "^6.0.0",
    "@codemirror/lang-go": "^6.0.0",
    "@codemirror/lang-php": "^6.0.0"
  }
}
```

- [ ] **Step 3: Write the esbuild driver**

Create `crates/termlab_tauri/frontend/build-vendor.mjs`:

```js
// Bundles third-party frontend dependencies into a single IIFE global.
//
// The app's own modules are plain IIFE <script> files and are deliberately NOT
// built — this exists only so CodeMirror 6, which is ESM-only, can be consumed
// by a frontend with no module system.
import { build } from 'esbuild';
import { mkdirSync } from 'node:fs';
import path from 'node:path';

const here = import.meta.dirname;
const outDir = path.join(here, 'vendor', 'codemirror');
mkdirSync(outDir, { recursive: true });

await build({
  entryPoints: [path.join(here, 'vendor-entry.mjs')],
  outfile: path.join(outDir, 'codemirror.js'),
  bundle: true,
  format: 'iife',
  globalName: 'CM6',
  minify: true,
  target: 'es2020',
  legalComments: 'none',
});

console.log('vendor: wrote vendor/codemirror/codemirror.js');
```

- [ ] **Step 4: Install and build the bundle**

Run:
```bash
cd crates/termlab_tauri/frontend && npm install && npm run build:vendor
```
Expected: `vendor: wrote vendor/codemirror/codemirror.js`, and `package-lock.json` now exists.

Note this uses `npm install` **once**, deliberately, to create the lockfile. Every later invocation is `npm ci`.

- [ ] **Step 5: Check the bundle is a valid IIFE exposing the global**

Every export name in `vendor-entry.mjs` must actually resolve — a legacy-mode name that is subtly wrong (`powerShell` vs `powershell`, `dockerFile` vs `dockerfile`) produces `undefined` rather than an error, and the only symptom is a file type that silently gets no highlighting.

Create `crates/termlab_tauri/frontend/check-vendor.mjs`:

```js
// Asserts every name vendor-entry.mjs claims to export actually resolves in
// the built bundle. A missing name is otherwise silent: the language just
// never highlights.
import fs from 'node:fs';
import path from 'node:path';

const here = import.meta.dirname;
const entry = fs.readFileSync(path.join(here, 'vendor-entry.mjs'), 'utf8');
const expected = [...entry.matchAll(/export\s*\{([^}]*)\}/g)]
  .flatMap((m) => m[1].split(','))
  .map((s) => s.trim().split(/\s+as\s+/).pop().trim())
  .filter(Boolean);

globalThis.window = globalThis;
new Function(fs.readFileSync(path.join(here, 'vendor', 'codemirror', 'codemirror.js'), 'utf8'))();

const missing = expected.filter((name) => globalThis.CM6[name] === undefined);
if (!globalThis.CM6) throw new Error('CM6 global not defined — check globalName');
if (missing.length) throw new Error(`missing from bundle: ${missing.join(', ')}`);
console.log(`vendor check: ${expected.length} exports present`);
```

Run:
```bash
cd crates/termlab_tauri/frontend && node check-vendor.mjs
```
Expected: `vendor check: N exports present` with N at least 40. If it names missing exports, the package genuinely exports a different identifier — look it up in that package's own `dist` types and correct `vendor-entry.mjs`, then rebuild and re-run until clean. Report any name you had to correct.

- [ ] **Step 6: Record the bundle size**

Run:
```bash
ls -lh crates/termlab_tauri/frontend/vendor/codemirror/codemirror.js
```
Note the size in the task report. The spec's estimate is roughly 1 MB; anything above 2 MB should be reported as a concern rather than silently accepted.

- [ ] **Step 7: Ignore generated output**

Add to `.gitignore`:

```
crates/termlab_tauri/frontend/node_modules/
crates/termlab_tauri/frontend/vendor/codemirror/
```

Verify `package-lock.json` is NOT ignored:
```bash
cd /Users/dustin/projects/conch && git check-ignore -v crates/termlab_tauri/frontend/package-lock.json; echo "exit=$?"
```
Expected: `exit=1` (not ignored).

- [ ] **Step 8: Wire the Tauri build hooks**

In `crates/termlab_tauri/tauri.conf.json`, add both keys to the `build` object that currently holds only `"frontendDist": "frontend"`:

```json
  "build": {
    "frontendDist": "frontend",
    "beforeDevCommand": "npm --prefix crates/termlab_tauri/frontend ci && npm --prefix crates/termlab_tauri/frontend run build:vendor",
    "beforeBuildCommand": "npm --prefix crates/termlab_tauri/frontend ci && npm --prefix crates/termlab_tauri/frontend run build:vendor"
  }
```

These run from the repo root, which is why the `--prefix` is repo-relative.

- [ ] **Step 9: Add the script tag**

In `crates/termlab_tauri/frontend/index.html`, immediately after the existing xterm addon script tags (around line 89), add:

```html
  <!-- CodeMirror 6, bundled by frontend/build-vendor.mjs. Generated, git-ignored;
       run `npm run build:vendor` in frontend/ if this 404s. -->
  <script src="vendor/codemirror/codemirror.js"></script>
```

- [ ] **Step 10: Update the release workflow**

In `.github/workflows/release.yml`, add to the `macos` (line ~53) and `windows` (line ~107) jobs, after the `actions/checkout` step:

```yaml
      - uses: actions/setup-node@v4
        with:
          node-version: '20'
```

For `linux-amd64` (line ~239) and `linux-arm64` (line ~299), add the same `setup-node` step **and**, immediately before the `cargo build --release -p termlab_tauri` step, add:

```yaml
      - name: Build vendored frontend dependencies
        run: |
          npm --prefix crates/termlab_tauri/frontend ci
          npm --prefix crates/termlab_tauri/frontend run build:vendor
```

These two jobs build with plain `cargo`, so Tauri's `beforeBuildCommand` never fires — without this step they ship a binary referencing a bundle that was never generated.

Do **not** touch `.github/workflows/ci.yml`: its jobs run only `cargo fmt`, `cargo test --workspace`, and `cargo clippy`, none of which build the frontend.

- [ ] **Step 11: Verify the boundary check still passes**

Run:
```bash
cd /Users/dustin/projects/conch && ./scripts/check_frontend_boundaries.sh .
```
Expected: passes. If it flags anything inside `vendor/codemirror/`, add that directory to the script's exclusions the same way other vendor directories are handled, and re-run until clean. Report in the task report whether an exclusion was needed.

- [ ] **Step 12: Verify the app still builds and runs**

Run:
```bash
cd /Users/dustin/projects/conch && cargo build -p termlab_tauri
```
Expected: success. Then launch the app and confirm in the devtools console that `typeof window.CM6.EditorView` is `"function"`.

- [ ] **Step 13: Commit**

```bash
git add crates/termlab_tauri/frontend/package.json \
        crates/termlab_tauri/frontend/package-lock.json \
        crates/termlab_tauri/frontend/vendor-entry.mjs \
        crates/termlab_tauri/frontend/build-vendor.mjs \
        crates/termlab_tauri/frontend/check-vendor.mjs \
        crates/termlab_tauri/tauri.conf.json \
        crates/termlab_tauri/frontend/index.html \
        .gitignore .github/workflows/release.yml
git commit -m "build: bundle CodeMirror 6 via esbuild

Introduces the frontend's first build step, scoped to third-party
dependencies. App modules remain plain IIFE scripts."
```

---

## Task 2: Rust File Guards, Temp Paths, and Commands

**Files:**
- Create: `crates/termlab_tauri/src/editor_fs.rs`
- Modify: `crates/termlab_tauri/src/lib.rs` (add `mod editor_fs;` and register commands in `invoke_handler`, line ~511)

**Interfaces:**
- Produces, callable from the frontend via `invoke`:
  - `editor_can_open(name: String, size: f64) -> Result<(), String>`
  - `editor_read_file(path: String) -> Result<String, String>`
  - `editor_write_file(path: String, contents: String) -> Result<(), String>`
  - `editor_scratch_dir() -> Result<String, String>`
  - `editor_temp_path(hostLabel: String, remotePath: String) -> Result<String, String>`
  - `editor_temp_cleanup(path: String) -> Result<(), String>`
  - `editor_temp_sweep() -> Result<(), String>`

  Note the camelCase argument names on the frontend side: Tauri converts `host_label` to `hostLabel` in the JS `invoke` payload.

- [ ] **Step 1: Write the failing tests**

Create `crates/termlab_tauri/src/editor_fs.rs` with only the test module and the `use` line, so it compiles to a failing state:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn size_cap_is_five_megabytes() {
        assert!(guard_openable("a.txt", MAX_EDIT_BYTES).is_ok());
        assert!(matches!(
            guard_openable("a.txt", MAX_EDIT_BYTES + 1),
            Err(OpenRejection::TooLarge { .. })
        ));
    }

    #[test]
    fn blocklist_covers_every_listed_extension_case_insensitively() {
        for ext in BLOCKED_EXTENSIONS {
            let lower = format!("file.{ext}");
            let upper = format!("file.{}", ext.to_uppercase());
            assert!(
                matches!(guard_openable(&lower, 10), Err(OpenRejection::BlockedExtension { .. })),
                "{lower} should be blocked"
            );
            assert!(
                matches!(guard_openable(&upper, 10), Err(OpenRejection::BlockedExtension { .. })),
                "{upper} should be blocked"
            );
        }
    }

    #[test]
    fn ordinary_names_are_allowed() {
        for name in ["a.txt", "b.rs", "Makefile", ".gitignore", "a.tar.txt", "no_extension"] {
            assert!(guard_openable(name, 10).is_ok(), "{name} should be allowed");
        }
    }

    #[test]
    fn multi_dot_names_are_judged_by_the_last_extension() {
        // .tar.gz is blocked by `gz`; .gz.txt is not blocked at all.
        assert!(matches!(
            guard_openable("archive.tar.gz", 10),
            Err(OpenRejection::BlockedExtension { .. })
        ));
        assert!(guard_openable("notes.gz.txt", 10).is_ok());
    }

    #[test]
    fn binary_sniff_looks_at_the_first_8192_bytes_only() {
        assert!(looks_binary(&[0x00]));

        let mut at_8191 = vec![b'a'; 8192];
        at_8191[8191] = 0x00;
        assert!(looks_binary(&at_8191));

        // One byte past the window: must be missed.
        let mut at_8192 = vec![b'a'; 8193];
        at_8192[8192] = 0x00;
        assert!(!looks_binary(&at_8192));

        assert!(!looks_binary(b""));
        assert!(!looks_binary(b"short text"));
    }

    #[test]
    fn temp_paths_separate_hosts_and_paths_and_keep_the_basename() {
        let a = temp_path_parts("host-a", "/etc/nginx.conf");
        let b = temp_path_parts("host-b", "/etc/nginx.conf");
        let c = temp_path_parts("host-a", "/opt/nginx.conf");

        assert_ne!(a.0, b.0, "different hosts must not share a directory");
        assert_eq!(a.0, c.0, "same host must share its directory");
        assert_ne!(a.1, c.1, "different remote paths must not share a directory");
        assert_eq!(a.2, "nginx.conf");

        assert_eq!(temp_path_parts("h", "/a/.bashrc").2, ".bashrc");
        assert_eq!(temp_path_parts("h", "/a/x.tar.gz").2, "x.tar.gz");
    }

    #[test]
    fn write_uses_a_temp_file_so_a_failed_write_cannot_truncate_the_original() {
        let dir = std::env::temp_dir().join("termlab-editor-write-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("f.txt");
        std::fs::write(&target, "original").unwrap();

        // Writing to a path whose parent does not exist must fail without
        // touching the existing file.
        let bad = dir.join("missing-dir").join("f.txt");
        assert!(write_text_file(bad.to_str().unwrap(), "new").is_err());
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "original");

        write_text_file(target.to_str().unwrap(), "replaced").unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "replaced");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p termlab_tauri editor_fs`
Expected: compile errors — `guard_openable`, `MAX_EDIT_BYTES`, `BLOCKED_EXTENSIONS`, `OpenRejection`, `looks_binary`, `temp_path_parts`, `write_text_file` not found.

(If `cargo` reports missing symbols that you *have* written, run `cargo clean -p termlab_tauri` — stale artifacts have produced phantom failures in this repo.)

- [ ] **Step 3: Implement the module**

Prepend to `crates/termlab_tauri/src/editor_fs.rs`, above the test module:

```rust
//! Filesystem access for the light editor.
//!
//! Every guard the editor applies lives here and nowhere else: the frontend
//! asks this module whether a file may be opened rather than carrying its own
//! copy of the size cap or the blocklist.

use std::fs;
use std::path::{Path, PathBuf};

/// Files above this never open. Matches the JVM editor's cap.
pub const MAX_EDIT_BYTES: u64 = 5 * 1024 * 1024;

/// How much of a file the binary sniff inspects.
const SNIFF_BYTES: usize = 8192;

/// Extensions we refuse outright, so a mis-click on an image or an archive
/// does not pull megabytes over SFTP just to be rejected after the fact.
pub const BLOCKED_EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "bmp", "ico", "webp", "svg", "zip", "tar", "gz", "tgz", "bz2",
    "xz", "7z", "rar", "jar", "war", "ear", "class", "exe", "dll", "so", "dylib", "pdf", "doc",
    "docx", "xls", "xlsx", "ppt", "pptx", "mp3", "mp4", "mov", "avi", "mkv", "wav", "flac", "pyc",
    "pyo",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenRejection {
    TooLarge { size: u64, max: u64 },
    BlockedExtension { ext: String },
    Binary { name: String },
}

impl OpenRejection {
    /// The user-facing message. Formatted here so every caller says the same
    /// thing about the same rejection.
    pub fn message(&self) -> String {
        match self {
            OpenRejection::TooLarge { size, max } => format!(
                "File too large ({:.1} MB). Maximum is {} MB.",
                *size as f64 / (1024.0 * 1024.0),
                max / (1024 * 1024)
            ),
            OpenRejection::BlockedExtension { ext } => {
                format!("Cannot edit binary file: .{ext}")
            }
            OpenRejection::Binary { name } => format!("Binary file detected: {name}"),
        }
    }
}

/// Decide whether a file may be opened, from its name and size alone — no I/O,
/// so the remote path can reject before transferring a byte.
pub fn guard_openable(name: &str, size: u64) -> Result<(), OpenRejection> {
    if size > MAX_EDIT_BYTES {
        return Err(OpenRejection::TooLarge {
            size,
            max: MAX_EDIT_BYTES,
        });
    }
    if let Some(ext) = extension_of(name) {
        if BLOCKED_EXTENSIONS.contains(&ext.as_str()) {
            return Err(OpenRejection::BlockedExtension { ext });
        }
    }
    Ok(())
}

/// The lowercased characters after the final dot, when there is one that is
/// not the leading dot of a dotfile. `".bashrc"` has no extension by this
/// definition, which is what makes dotfiles editable.
fn extension_of(name: &str) -> Option<String> {
    let base = name.rsplit('/').next().unwrap_or(name);
    let idx = base.rfind('.')?;
    if idx == 0 {
        return None;
    }
    Some(base[idx + 1..].to_ascii_lowercase())
}

/// True if the head of a file contains a NUL, the cheap heuristic for "this is
/// not text". Only the first [`SNIFF_BYTES`] are examined, so a NUL past that
/// window is deliberately missed rather than costing a full scan.
pub fn looks_binary(head: &[u8]) -> bool {
    head.iter().take(SNIFF_BYTES).any(|b| *b == 0)
}

/// FNV-1a, 64-bit, rendered as 8 hex characters.
///
/// This disambiguates directory names on disk; it is not security, so a real
/// hash crate would be a dependency bought for nothing.
fn short_hash(input: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in input.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{:016x}", hash)[..8].to_string()
}

/// The three path components of a remote edit's temp location:
/// (host directory, path directory, basename).
///
/// Split out from [`temp_path`] so the naming rules are testable without
/// touching the filesystem.
pub fn temp_path_parts(host_label: &str, remote_path: &str) -> (String, String, String) {
    let basename = remote_path
        .rsplit('/')
        .find(|s| !s.is_empty())
        .unwrap_or("untitled")
        .to_string();
    (short_hash(host_label), short_hash(remote_path), basename)
}

pub fn temp_root() -> PathBuf {
    std::env::temp_dir().join("termlab-sftp-edits")
}

/// Write via a sibling temp file and a rename, so a write that fails partway
/// leaves the original file exactly as it was.
pub fn write_text_file(path: &str, contents: &str) -> Result<(), String> {
    let target = Path::new(path);
    let tmp = target.with_extension(format!(
        "{}termlab-tmp",
        target
            .extension()
            .map(|e| format!("{}.", e.to_string_lossy()))
            .unwrap_or_default()
    ));
    fs::write(&tmp, contents).map_err(|e| format!("Could not write {path}: {e}"))?;
    fs::rename(&tmp, target).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        format!("Could not replace {path}: {e}")
    })
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p termlab_tauri editor_fs`
Expected: 7 tests pass.

- [ ] **Step 5: Add the Tauri commands**

Append to `editor_fs.rs`, above the test module:

```rust
// ---------------------------------------------------------------------------
// Tauri commands
// ---------------------------------------------------------------------------

#[tauri::command]
pub(crate) fn editor_can_open(name: String, size: f64) -> Result<(), String> {
    let size = if size < 0.0 { 0 } else { size as u64 };
    guard_openable(&name, size).map_err(|r| r.message())
}

#[tauri::command]
pub(crate) fn editor_read_file(path: String) -> Result<String, String> {
    let p = Path::new(&path);
    let name = p
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.clone());
    let meta = fs::metadata(p).map_err(|e| format!("Could not open {path}: {e}"))?;
    guard_openable(&name, meta.len()).map_err(|r| r.message())?;

    let bytes = fs::read(p).map_err(|e| format!("Could not read {path}: {e}"))?;
    if looks_binary(&bytes) {
        return Err(OpenRejection::Binary { name }.message());
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

#[tauri::command]
pub(crate) fn editor_write_file(path: String, contents: String) -> Result<(), String> {
    write_text_file(&path, &contents)
}

#[tauri::command]
pub(crate) fn editor_scratch_dir() -> Result<String, String> {
    let dir = termlab_core::config::config_dir().join("scratches");
    fs::create_dir_all(&dir).map_err(|e| format!("Could not create scratch directory: {e}"))?;
    Ok(dir.to_string_lossy().into_owned())
}

#[tauri::command]
pub(crate) fn editor_temp_path(host_label: String, remote_path: String) -> Result<String, String> {
    let (host_dir, path_dir, basename) = temp_path_parts(&host_label, &remote_path);
    let dir = temp_root().join(host_dir).join(path_dir);
    fs::create_dir_all(&dir).map_err(|_| "Cannot create temp file for editing".to_string())?;
    Ok(dir.join(basename).to_string_lossy().into_owned())
}

/// Delete a temp file and any parent directories it leaves empty, without
/// escaping the temp root — a caller passing an arbitrary path must not be
/// able to delete outside it.
#[tauri::command]
pub(crate) fn editor_temp_cleanup(path: String) -> Result<(), String> {
    let root = temp_root();
    let target = PathBuf::from(&path);
    if !target.starts_with(&root) {
        return Err("Refusing to clean a path outside the editor temp root".into());
    }
    let _ = fs::remove_file(&target);
    let mut parent = target.parent().map(|p| p.to_path_buf());
    while let Some(dir) = parent {
        if dir == root || !dir.starts_with(&root) {
            break;
        }
        if fs::remove_dir(&dir).is_err() {
            break; // not empty — stop climbing
        }
        parent = dir.parent().map(|p| p.to_path_buf());
    }
    Ok(())
}

/// Delete everything under the temp root. Called at startup to clear orphans
/// left by a crash, and at shutdown.
#[tauri::command]
pub(crate) fn editor_temp_sweep() -> Result<(), String> {
    let _ = fs::remove_dir_all(temp_root());
    Ok(())
}
```

- [ ] **Step 6: Add a test for the cleanup guard**

Add to the `tests` module in `editor_fs.rs`:

```rust
    #[test]
    fn cleanup_refuses_paths_outside_the_temp_root() {
        let outside = std::env::temp_dir().join("termlab-not-an-edit.txt");
        std::fs::write(&outside, "keep me").unwrap();

        assert!(editor_temp_cleanup(outside.to_string_lossy().into_owned()).is_err());
        assert!(outside.exists(), "a path outside the root must survive");

        let _ = std::fs::remove_file(&outside);
    }
```

Run: `cargo test -p termlab_tauri editor_fs`
Expected: 8 tests pass.

- [ ] **Step 7: Register the module and commands**

In `crates/termlab_tauri/src/lib.rs`, add `mod editor_fs;` beside the other module declarations, and add these seven entries to the `tauri::generate_handler![...]` list starting at line ~511:

```rust
            editor_fs::editor_can_open,
            editor_fs::editor_read_file,
            editor_fs::editor_write_file,
            editor_fs::editor_scratch_dir,
            editor_fs::editor_temp_path,
            editor_fs::editor_temp_cleanup,
            editor_fs::editor_temp_sweep,
```

- [ ] **Step 8: Call the startup sweep**

In `lib.rs`'s setup, after the app handle is available, spawn the orphan sweep off the main thread so it never delays startup:

```rust
    std::thread::spawn(|| {
        let _ = editor_fs::editor_temp_sweep();
    });
```

- [ ] **Step 9: Verify the whole crate builds and tests pass**

Run: `cargo test -p termlab_tauri && cargo clippy -p termlab_tauri --all-targets`
Expected: all tests pass, no clippy warnings.

- [ ] **Step 10: Commit**

```bash
git add crates/termlab_tauri/src/editor_fs.rs crates/termlab_tauri/src/lib.rs
git commit -m "feat: editor filesystem guards and commands

Size cap, extension blocklist, binary sniff and temp-path resolution,
all in Rust so the frontend carries no copy of the rules."
```

---

## Task 3: Filename → Language Mapping

**Files:**
- Create: `crates/termlab_tauri/frontend/app/features/editor/language-map.js`
- Test: `scripts/tests/test_language_map.mjs`
- Modify: `crates/termlab_tauri/frontend/index.html`

**Interfaces:**
- Produces: `window.termlabEditorLanguageMap = { languageKeyFor }` where `languageKeyFor(filename: string) -> string | null`. The returned key is the name of an export in `vendor-entry.mjs` (e.g. `'javascript'`, `'shell'`), or `null` for plain text. Task 4 resolves the key against `window.CM6`.

- [ ] **Step 1: Write the failing test**

Create `scripts/tests/test_language_map.mjs`:

```js
// Run: node scripts/tests/test_language_map.mjs
//
// Filename → CodeMirror language key. Pure and table-driven, so it is tested
// exhaustively here rather than by opening files in the app.
import assert from 'node:assert';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';

const MODULE_PATH = path.resolve(
  import.meta.dirname,
  '../../crates/termlab_tauri/frontend/app/features/editor/language-map.js',
);

const sandbox = { console };
sandbox.window = sandbox;
vm.createContext(sandbox);
vm.runInContext(fs.readFileSync(MODULE_PATH, 'utf8'), sandbox, { filename: MODULE_PATH });

const { languageKeyFor } = sandbox.termlabEditorLanguageMap;

const cases = [
  ['app.js', 'javascript'], ['app.mjs', 'javascript'], ['app.cjs', 'javascript'],
  ['app.ts', 'javascript'], ['app.jsx', 'javascript'], ['app.tsx', 'javascript'],
  ['data.json', 'json'], ['main.py', 'python'], ['README.md', 'markdown'],
  ['lib.rs', 'rust'], ['index.html', 'html'], ['a.htm', 'html'],
  ['style.css', 'css'], ['pom.xml', 'xml'], ['ci.yml', 'yaml'], ['ci.yaml', 'yaml'],
  ['q.sql', 'sql'], ['A.java', 'java'], ['a.c', 'cpp'], ['a.cpp', 'cpp'],
  ['a.h', 'cpp'], ['a.hpp', 'cpp'], ['main.go', 'go'], ['i.php', 'php'],
  ['run.sh', 'shell'], ['run.bash', 'shell'], ['run.zsh', 'shell'],
  ['Cargo.toml', 'toml'], ['s.lua', 'lua'], ['s.rb', 'ruby'], ['s.pl', 'perl'],
  ['s.ps1', 'powerShell'], ['a.diff', 'diff'], ['a.patch', 'diff'],
  ['app.properties', 'properties'],
];

for (const [name, expected] of cases) {
  assert.strictEqual(languageKeyFor(name), expected, `${name} → ${expected}`);
}

// Case-insensitive on the extension.
assert.strictEqual(languageKeyFor('APP.JS'), 'javascript');
assert.strictEqual(languageKeyFor('Main.PY'), 'python');

// Extensionless names that are still recognisable.
assert.strictEqual(languageKeyFor('Dockerfile'), 'dockerFile');
assert.strictEqual(languageKeyFor('dockerfile'), 'dockerFile');
assert.strictEqual(languageKeyFor('.bashrc'), 'shell');
assert.strictEqual(languageKeyFor('.zshrc'), 'shell');
assert.strictEqual(languageKeyFor('.profile'), 'shell');
assert.strictEqual(languageKeyFor('nginx.conf'), 'nginx');

// A full path is accepted, and only the basename decides.
assert.strictEqual(languageKeyFor('/etc/nginx/nginx.conf'), 'nginx');
assert.strictEqual(languageKeyFor('/home/u/app.js'), 'javascript');

// Unknown and degenerate inputs are plain text, never a throw.
for (const name of ['notes.txt', 'Makefile', 'LICENSE', 'weird.zzz', '', null, undefined]) {
  assert.strictEqual(languageKeyFor(name), null, `${name} → null`);
}

console.log('language map: all assertions passed');
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `node scripts/tests/test_language_map.mjs`
Expected: FAIL — cannot read `languageKeyFor` of undefined (the module does not exist).

- [ ] **Step 3: Implement the module**

Create `crates/termlab_tauri/frontend/app/features/editor/language-map.js`:

```js
// Filename → CodeMirror language key.
//
// The value is the name of an export in frontend/vendor-entry.mjs; the editor
// pane resolves it against window.CM6. Keeping this a pure string→string map
// means the whole table is testable without a DOM or a CodeMirror instance.
(function initTermLabEditorLanguageMap(global) {
  'use strict';

  const BY_EXTENSION = {
    js: 'javascript', mjs: 'javascript', cjs: 'javascript',
    ts: 'javascript', jsx: 'javascript', tsx: 'javascript',
    json: 'json',
    py: 'python',
    md: 'markdown', markdown: 'markdown',
    rs: 'rust',
    html: 'html', htm: 'html',
    css: 'css',
    xml: 'xml',
    yml: 'yaml', yaml: 'yaml',
    sql: 'sql',
    java: 'java',
    c: 'cpp', h: 'cpp', cc: 'cpp', cpp: 'cpp', hpp: 'cpp', cxx: 'cpp',
    go: 'go',
    php: 'php',
    sh: 'shell', bash: 'shell', zsh: 'shell',
    toml: 'toml',
    lua: 'lua',
    rb: 'ruby',
    pl: 'perl', pm: 'perl',
    ps1: 'powerShell',
    diff: 'diff', patch: 'diff',
    properties: 'properties',
    conf: 'nginx',
  };

  // Files whose whole name identifies them, with no extension to go on.
  const BY_NAME = {
    dockerfile: 'dockerFile',
    '.bashrc': 'shell',
    '.bash_profile': 'shell',
    '.zshrc': 'shell',
    '.profile': 'shell',
  };

  function languageKeyFor(filename) {
    if (typeof filename !== 'string' || filename.length === 0) return null;
    const base = filename.split('/').pop().split('\\').pop();
    if (!base) return null;

    const byName = BY_NAME[base.toLowerCase()];
    if (byName) return byName;

    const dot = base.lastIndexOf('.');
    if (dot <= 0) return null; // no extension, or a dotfile we do not know
    const ext = base.slice(dot + 1).toLowerCase();
    return BY_EXTENSION[ext] || null;
  }

  global.termlabEditorLanguageMap = { languageKeyFor };
})(window);
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `node scripts/tests/test_language_map.mjs`
Expected: `language map: all assertions passed`

- [ ] **Step 5: Register the script**

In `crates/termlab_tauri/frontend/index.html`, alongside the other `app/features/` script tags, add:

```html
  <script src="app/features/editor/language-map.js"></script>
```

- [ ] **Step 6: Commit**

```bash
git add crates/termlab_tauri/frontend/app/features/editor/language-map.js \
        scripts/tests/test_language_map.mjs \
        crates/termlab_tauri/frontend/index.html
git commit -m "feat: filename to CodeMirror language mapping"
```

---

## Task 4: The Editor Pane and Its Theme

**Files:**
- Create: `crates/termlab_tauri/frontend/app/features/editor/theme.js`
- Create: `crates/termlab_tauri/frontend/app/features/editor/editor-pane.js`
- Create: `crates/termlab_tauri/frontend/styles/design-system/components/editor.css`
- Modify: `crates/termlab_tauri/frontend/index.html`

**Interfaces:**
- Consumes: `window.CM6` (Task 1); `window.termlabEditorLanguageMap.languageKeyFor` (Task 3).
- Produces:
  - `window.termlabEditorTheme = { buildTheme }` — `buildTheme() -> Array` of CodeMirror extensions derived from the current `--tl-*` tokens.
  - `window.termlabEditorPane = { createEditorView, destroyEditorView, setFontSize, refreshTheme }`
    - `createEditorView(hostEl, { doc, filename, onDirtyChange }) -> EditorView | null`
    - `destroyEditorView(view)` — `void`
    - `setFontSize(view, px)` — `void`
    - `refreshTheme(view)` — `void`

- [ ] **Step 1: Write the theme module**

Create `crates/termlab_tauri/frontend/app/features/editor/theme.js`:

```js
// A CodeMirror theme built from the app's design tokens.
//
// Reads the same --tl-* variables every other component uses, so the editor
// follows skins and light/dark without a second palette to keep in sync. No
// literal colours live here: an unset token yields an empty string and
// CodeMirror falls back to its own default rather than to a wrong hardcode.
(function initTermLabEditorTheme(global) {
  'use strict';

  function token(name, fallbackToken) {
    const styles = getComputedStyle(document.documentElement);
    const value = styles.getPropertyValue(name).trim();
    if (value) return value;
    return fallbackToken ? styles.getPropertyValue(fallbackToken).trim() : '';
  }

  function buildTheme() {
    const CM = global.CM6;
    if (!CM) return [];

    const bg = token('--tl-terminal-bg', '--tl-bg');
    const fg = token('--tl-fg');
    const muted = token('--tl-fg-muted');
    const accent = token('--tl-accent');
    const border = token('--tl-border');
    const selection = token('--tl-selection', '--tl-accent');
    const rowHover = token('--tl-row-hover');

    const theme = CM.EditorView.theme({
      '&': { backgroundColor: bg, color: fg, height: '100%' },
      '.cm-content': { caretColor: fg },
      '.cm-cursor, .cm-dropCursor': { borderLeftColor: fg },
      '&.cm-focused .cm-selectionBackground, .cm-selectionBackground, .cm-content ::selection':
        { backgroundColor: selection },
      '.cm-gutters': { backgroundColor: bg, color: muted, borderRight: `1px solid ${border}` },
      '.cm-activeLine': { backgroundColor: rowHover },
      '.cm-activeLineGutter': { backgroundColor: rowHover, color: fg },
      '.cm-selectionMatch': { backgroundColor: rowHover },
      '.cm-scroller': { fontFamily: 'inherit' },
    }, { dark: isDarkTheme() });

    const t = CM.tags;
    const highlight = CM.HighlightStyle.define([
      { tag: [t.keyword, t.controlKeyword, t.moduleKeyword], color: accent },
      { tag: [t.string, t.special(t.string)], color: token('--tl-success', '--tl-accent') },
      { tag: [t.comment, t.lineComment, t.blockComment], color: muted, fontStyle: 'italic' },
      { tag: [t.number, t.bool, t.null], color: token('--tl-warning', '--tl-accent') },
      { tag: [t.function(t.variableName), t.definition(t.variableName)], color: fg },
      { tag: [t.typeName, t.className], color: accent },
      { tag: t.propertyName, color: fg },
      { tag: t.operator, color: muted },
      { tag: t.invalid, color: token('--tl-danger') },
      { tag: [t.heading, t.strong], color: fg, fontWeight: 'bold' },
      { tag: t.emphasis, fontStyle: 'italic' },
      { tag: t.link, color: accent, textDecoration: 'underline' },
    ]);

    return [theme, CM.syntaxHighlighting(highlight)];
  }

  // The token pipeline emits a data-theme attribute; fall back to the
  // background's perceived lightness when it is absent.
  function isDarkTheme() {
    const attr = document.documentElement.getAttribute('data-theme');
    if (attr === 'dark') return true;
    if (attr === 'light') return false;
    const bg = token('--tl-bg');
    const m = /rgb[a]?\((\d+),\s*(\d+),\s*(\d+)/.exec(bg);
    if (!m) return true;
    const luma = (0.299 * +m[1] + 0.587 * +m[2] + 0.114 * +m[3]) / 255;
    return luma < 0.5;
  }

  global.termlabEditorTheme = { buildTheme };
})(window);
```

- [ ] **Step 2: Write the editor pane module**

Create `crates/termlab_tauri/frontend/app/features/editor/editor-pane.js`:

```js
// Owns a CodeMirror EditorView inside a pane element.
//
// Deliberately knows nothing about files, saving, or tabs — it renders a
// document and reports when it becomes dirty. editor-service.js supplies the
// meaning; this supplies the surface.
(function initTermLabEditorPane(global) {
  'use strict';

  // Compartments let the font size and theme be reconfigured on a live view
  // without rebuilding its state (which would discard undo history).
  const fontCompartments = new WeakMap();
  const themeCompartments = new WeakMap();

  function languageExtension(filename) {
    const CM = global.CM6;
    const map = global.termlabEditorLanguageMap;
    if (!CM || !map) return [];
    const key = map.languageKeyFor(filename);
    if (!key) return [];
    const entry = CM[key];
    if (!entry) return [];
    // Two shapes arrive here. The @codemirror/lang-* packages export a
    // FUNCTION returning a LanguageSupport. The legacy modes export a plain
    // StreamParser OBJECT, which has to be wrapped in StreamLanguage before
    // CodeMirror will take it. Discriminating on typeof is the whole trick —
    // treating the object as a factory silently yields no highlighting.
    if (typeof entry === 'function') return [entry()];
    return [CM.StreamLanguage.define(entry)];
  }

  function createEditorView(hostEl, options) {
    const CM = global.CM6;
    if (!CM || !hostEl) return null;
    const opts = options || {};
    const onDirtyChange = typeof opts.onDirtyChange === 'function' ? opts.onDirtyChange : () => {};

    const fontComp = new CM.Compartment();
    const themeComp = new CM.Compartment();
    const themeExtensions = global.termlabEditorTheme
      ? global.termlabEditorTheme.buildTheme()
      : [];

    let dirty = false;
    const dirtyWatcher = CM.EditorView.updateListener.of((update) => {
      if (!update.docChanged || dirty) return;
      dirty = true;
      onDirtyChange(true);
    });

    const view = new CM.EditorView({
      parent: hostEl,
      state: CM.EditorState.create({
        doc: typeof opts.doc === 'string' ? opts.doc : '',
        extensions: [
          CM.lineNumbers(),
          CM.highlightActiveLineGutter(),
          CM.highlightSpecialChars(),
          CM.history(),
          CM.foldGutter(),
          CM.drawSelection(),
          CM.rectangularSelection(),
          CM.indentOnInput(),
          CM.bracketMatching(),
          CM.highlightActiveLine(),
          CM.highlightSelectionMatches(),
          CM.keymap.of([
            ...CM.defaultKeymap,
            ...CM.historyKeymap,
            ...CM.searchKeymap,
            ...CM.foldKeymap,
            CM.indentWithTab,
          ]),
          languageExtension(opts.filename || ''),
          themeComp.of(themeExtensions),
          fontComp.of([]),
          dirtyWatcher,
        ],
      }),
    });

    fontCompartments.set(view, fontComp);
    themeCompartments.set(view, themeComp);
    // Callers clear dirty after a save; expose the reset without exposing state.
    view.termlabResetDirty = () => {
      dirty = false;
      onDirtyChange(false);
    };
    return view;
  }

  function destroyEditorView(view) {
    if (view && typeof view.destroy === 'function') view.destroy();
  }

  function setFontSize(view, px) {
    const CM = global.CM6;
    const comp = fontCompartments.get(view);
    if (!CM || !view || !comp || !px) return;
    view.dispatch({
      effects: comp.reconfigure(
        CM.EditorView.theme({ '&': { fontSize: `${px}px` } }),
      ),
    });
  }

  function refreshTheme(view) {
    const comp = themeCompartments.get(view);
    if (!view || !comp || !global.termlabEditorTheme) return;
    view.dispatch({ effects: comp.reconfigure(global.termlabEditorTheme.buildTheme()) });
  }

  global.termlabEditorPane = {
    createEditorView,
    destroyEditorView,
    setFontSize,
    refreshTheme,
  };
})(window);
```

- [ ] **Step 3: Write the stylesheet**

Create `crates/termlab_tauri/frontend/styles/design-system/components/editor.css`:

```css
/* Editor pane — the host element a CodeMirror view is mounted into.
   Tokens only; CodeMirror's own colours come from theme.js. */

.editor-pane-host {
  position: absolute;
  inset: 0;
  overflow: hidden;
  background: var(--tl-terminal-bg);
}

.editor-pane-host .cm-editor {
  height: 100%;
}

.editor-pane-host .cm-editor.cm-focused {
  outline: none;
}

/* The modified marker in the tab label. */
.tab-dirty-marker {
  margin-left: var(--tl-space-1);
  color: var(--tl-accent);
}
.tab-dirty-marker[hidden] {
  display: none;
}
```

Register it wherever the other `design-system/components/*.css` files are linked in `index.html`.

- [ ] **Step 4: Register the scripts**

In `crates/termlab_tauri/frontend/index.html`, after the `language-map.js` tag from Task 3:

```html
  <script src="app/features/editor/theme.js"></script>
  <script src="app/features/editor/editor-pane.js"></script>
```

- [ ] **Step 5: Verify in the running app**

Launch the app and run this in the devtools console:

```js
const host = document.createElement('div');
host.style.cssText = 'position:fixed;inset:100px;z-index:9999;';
document.body.appendChild(host);
const v = window.termlabEditorPane.createEditorView(host, {
  doc: 'function hi() {\n  return 1;\n}\n', filename: 'x.js',
  onDirtyChange: (d) => console.log('dirty:', d),
});
```

Expected: a syntax-highlighted editor appears with line numbers, colours matching the app theme. Type a character and confirm `dirty: true` logs exactly once. Then run `window.termlabEditorPane.setFontSize(v, 20)` and confirm the text grows, and `v.destroy(); host.remove();` to clean up.

Repeat with `filename: 'run.sh'` and confirm shell highlighting works — this exercises the `StreamLanguage` fallback path in `languageExtension`, which the `lang-*` path does not.

- [ ] **Step 6: Commit**

```bash
git add crates/termlab_tauri/frontend/app/features/editor/theme.js \
        crates/termlab_tauri/frontend/app/features/editor/editor-pane.js \
        crates/termlab_tauri/frontend/styles/design-system/components/editor.css \
        crates/termlab_tauri/frontend/index.html
git commit -m "feat: editor pane and token-driven CodeMirror theme"
```

---

## Task 5: The Editor Pane Kind

**Files:**
- Modify: `crates/termlab_tauri/frontend/app/tab-manager.js` (`createTab` ~line 424, `closeTab` ~line 361)
- Modify: `crates/termlab_tauri/frontend/app/pane-manager.js` (`setFocusedPane` ~line 83)
- Modify: `crates/termlab_tauri/frontend/app/clipboard-runtime.js:9`
- Modify: `crates/termlab_tauri/frontend/app/config-runtime.js:37,59,66`
- Modify: `crates/termlab_tauri/frontend/app/main-runtime.js:488,501`

**Interfaces:**
- Consumes: `window.termlabEditorPane` (Task 4).
- Produces: `createEditorTab({ filePath, contents, remote }) -> tabId`, exported from `tab-manager.js` on the same object its other functions are exported from. The pane object it creates has the shape:
  ```js
  { paneId, tabId, kind: 'editor', type: null, connectionId: null,
    term: null, fitAddon: null, root, spawned: false,
    lastCols: 0, lastRows: 0, cleanupMouseBridge: null,
    resizeObserver: null, debounceTimer: null,
    filePath, view, dirty: false, remote }
  ```
  `remote` is `null` for local files, or `{ paneId, remotePath, hostLabel }` for a file opened over SFTP. Task 8 populates it.

- [ ] **Step 1: Audit every `kind` guard and record the decision**

Run:
```bash
cd /Users/dustin/projects/conch/crates/termlab_tauri && grep -rn "kind === 'terminal'\|kind !== 'terminal'" frontend/app/
```

For **each** hit, write one line in the task report saying whether it stays terminal-only or gains an editor arm. This is the task's highest-risk step: a guard that needed an editor arm and did not get one fails silently — a font change that skips editor panes just looks like nothing happened.

The expected answers, to check your audit against:

| Site | Decision |
|---|---|
| `window-events-runtime.js:89,99` | Terminal-only — PTY output has no editor meaning. |
| `context-menu-runtime.js:30` | Terminal-only — a mouse-mode check; editors get the default menu. |
| `shortcut-runtime.js:277` | Terminal-only. |
| `clipboard-runtime.js:9` | **Editor arm** — paste must reach the editor. |
| `config-runtime.js:37,59,66` | **Editor arm** — font size and theme must reach editor panes. |
| `main-runtime.js:488,501` | **Editor arm** — font only; CodeMirror reflows itself, so no fit call. |
| `pane-manager.js:83,88` | **Editor arm** — focus the `EditorView`. |
| `pane-manager.js:214,268,269,288` | Terminal-only — these are split/spawn paths that already treat non-terminal panes as `'local'`. |
| `tab-manager.js:375,376,379` | **Editor arm** in the close loop — destroy the view. |

- [ ] **Step 2: Add `createEditorTab` to `tab-manager.js`**

Modelled on `createTab` (line 424) but with no terminal. Add beside it:

```js
    // An editor tab. Mirrors createTab's DOM and tab bookkeeping exactly —
    // same button, same tree-root container, same divider wiring — so editor
    // tabs participate in splits, drag-and-drop and activation with no
    // special cases downstream. The only difference is what lives in the pane.
    function createEditorTab(options) {
      const opts = options || {};
      const tabs = getTabs();
      const panes = getPanes();
      const tabId = allocateTabId();
      const paneId = allocatePaneId();
      const fileName = String(opts.filePath || 'untitled').split('/').pop();

      const button = makeTabButton(fileName, () => closeTab(tabId));
      button.dataset.tabId = String(tabId);
      button.classList.add('entering');

      const containerEl = document.createElement('div');
      containerEl.className = 'tab-tree-root';

      const paneEl = document.createElement('div');
      paneEl.className = 'terminal-pane';
      paneEl.dataset.paneId = paneId;
      containerEl.appendChild(paneEl);

      const hostEl = document.createElement('div');
      hostEl.className = 'editor-pane-host';
      paneEl.appendChild(hostEl);

      tabBarEl.appendChild(button);
      terminalHostEl.appendChild(containerEl);

      const pane = {
        paneId,
        tabId,
        kind: 'editor',
        type: null,
        connectionId: null,
        term: null,
        fitAddon: null,
        root: paneEl,
        spawned: false,
        lastCols: 0,
        lastRows: 0,
        cleanupMouseBridge: null,
        resizeObserver: null,
        debounceTimer: null,
        filePath: opts.filePath || null,
        view: null,
        dirty: false,
        remote: opts.remote || null,
      };
      panes.set(paneId, pane);

      const tab = {
        id: tabId,
        label: fileName,
        type: 'editor',
        hasCustomTitle: true,
        button,
        containerEl,
        treeRoot: makeLeaf(paneId),
        focusedPaneId: paneId,
      };
      tabs.set(tabId, tab);
      setupDividerDrag(
        containerEl,
        () => tab.treeRoot,
        (newTree) => { tab.treeRoot = newTree; },
      );
      updateTabBarVisibility();

      requestAnimationFrame(() => {
        requestAnimationFrame(() => button.classList.remove('entering'));
      });

      button.addEventListener('click', () => activateTab(tabId));
      paneEl.addEventListener('mousedown', () => setFocusedPane(paneId));

      // The modified marker is its own element, appended next to the tab's
      // label. Do NOT write button.textContent — makeTabButton builds child
      // elements (the label and the close affordance) and assigning
      // textContent destroys them. Read makeTabButton, find the element that
      // holds the label text, and insert the marker directly after it; append
      // to the button only if no such element exists.
      const dirtyMarker = document.createElement('span');
      dirtyMarker.className = 'tab-dirty-marker';
      dirtyMarker.textContent = '•';
      dirtyMarker.hidden = true;
      button.appendChild(dirtyMarker);

      pane.view = global.termlabEditorPane.createEditorView(hostEl, {
        doc: typeof opts.contents === 'string' ? opts.contents : '',
        filename: pane.filePath || '',
        onDirtyChange: (dirty) => {
          pane.dirty = dirty;
          dirtyMarker.hidden = !dirty;
        },
      });

      activateTab(tabId);
      setFocusedPane(paneId);
      return tabId;
    }
```

Export `createEditorTab` from `tab-manager.js` alongside its existing exports.

- [ ] **Step 3: Add the editor arm to `closeTab`**

In `closeTab` (line ~361), inside the pane loop, after the `plugin_view` branch at line 379, add:

```js
        } else if (pane.kind === 'editor') {
          if (pane.view) global.termlabEditorPane.destroyEditorView(pane.view);
        }
```

- [ ] **Step 4: Add the editor arm to `setFocusedPane`**

In `pane-manager.js`, in `setFocusedPane` (line ~83), beside the terminal focus call:

```js
        if (pane.kind === 'editor' && pane.view) {
          pane.view.focus();
        }
```

- [ ] **Step 5: Add the editor arm to paste**

In `clipboard-runtime.js:9`, the current guard rejects any non-terminal pane. Change it so an editor pane inserts at the cursor instead:

```js
      if (!pane || typeof text !== 'string' || text.length === 0) return false;
      if (pane.kind === 'editor' && pane.view) {
        pane.view.dispatch(pane.view.state.replaceSelection(text));
        return true;
      }
      if (pane.kind !== 'terminal' || !pane.spawned) return false;
```

- [ ] **Step 6: Add editor arms to font and theme reapplication**

In `config-runtime.js` at lines 37, 59 and 66, each loop currently skips non-terminal panes. Add an editor arm to each so the font size and theme reach editor panes:

```js
          if (pane.kind === 'editor' && pane.view) {
            global.termlabEditorPane.setFontSize(pane.view, fontSize);
            global.termlabEditorPane.refreshTheme(pane.view);
            continue;
          }
```

Use whatever local variable already holds the font size at each site; do not introduce a new source for it.

In `main-runtime.js` at lines 488 and 501, add the same font-size arm, but **no** fit call — CodeMirror reflows itself, and there is no `fitAddon` on an editor pane.

- [ ] **Step 7: Verify in the running app**

Launch the app and run in the devtools console:

```js
const tm = window.termlabTabManager || window.tabManager;
tm.createEditorTab({ filePath: '/tmp/demo.js', contents: 'const a = 1;\n' });
```

Then confirm, one at a time:
1. A tab named `demo.js` appears and is active, showing a highlighted editor.
2. Typing adds a `•` to the tab label.
3. `cmd+v` with something on the clipboard pastes into the editor.
4. Changing the font size in Settings resizes the editor text.
5. Changing the theme in Settings recolours the editor.
6. Splitting the tab puts a terminal beside the editor and both work.
7. Closing the tab removes it with no console errors.

Record each as pass/fail in the task report.

- [ ] **Step 8: Commit**

```bash
git add crates/termlab_tauri/frontend/app/tab-manager.js \
        crates/termlab_tauri/frontend/app/pane-manager.js \
        crates/termlab_tauri/frontend/app/clipboard-runtime.js \
        crates/termlab_tauri/frontend/app/config-runtime.js \
        crates/termlab_tauri/frontend/app/main-runtime.js
git commit -m "feat: editor pane kind alongside terminal and plugin_view"
```

---

## Task 6: Scratch Files, Save, and Keybindings

**Files:**
- Create: `crates/termlab_tauri/frontend/app/features/editor/scratch.js`
- Create: `crates/termlab_tauri/frontend/app/features/editor/editor-service.js`
- Test: `scripts/tests/test_scratch_naming.mjs`
- Modify: `crates/termlab_core/src/config/termlab.rs` (`KeyboardConfig` struct and its `Default`)
- Modify: `crates/termlab_tauri/frontend/app/shortcut-runtime.js:28-49`
- Modify: `crates/termlab_tauri/frontend/app/menu-actions.js:37`
- Modify: `crates/termlab_tauri/frontend/app/command-palette-runtime.js:115`
- Modify: `crates/termlab_tauri/frontend/index.html`

**Interfaces:**
- Consumes: `createEditorTab` (Task 5); the Rust commands from Task 2.
- Produces: `window.termlabEditorService = { openLocalFile, openScratch, saveActiveEditor, eachEditorPane, uploadRemote }`.
  - `openLocalFile(path) -> Promise<void>`
  - `openScratch() -> Promise<void>`
  - `saveActiveEditor() -> Promise<void>`
  - `eachEditorPane(fn)` — calls `fn(pane)` for every editor pane in the window. Task 7 builds its close guards on this.
  - `uploadRemote(pane) -> Promise<void>` — a no-op stub here; Task 8 replaces it with the real upload.
- Requires: `tab-manager.js` must export `activateTab`, and `pane-manager.js` must export `setFocusedPane` and `getFocusedPaneId`. Add them to those modules' exported objects if they are currently internal.
- Produces: `window.termlabEditorScratch = { nextScratchName }` — `nextScratchName(existing: string[]) -> string`.

- [ ] **Step 1: Write the failing scratch-naming test**

Create `scripts/tests/test_scratch_naming.mjs`:

```js
// Run: node scripts/tests/test_scratch_naming.mjs
//
// Scratch names must not collide with files already in the scratch directory,
// including ones the user renamed by hand.
import assert from 'node:assert';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';

const MODULE_PATH = path.resolve(
  import.meta.dirname,
  '../../crates/termlab_tauri/frontend/app/features/editor/scratch.js',
);

const sandbox = { console };
sandbox.window = sandbox;
vm.createContext(sandbox);
vm.runInContext(fs.readFileSync(MODULE_PATH, 'utf8'), sandbox, { filename: MODULE_PATH });

const { nextScratchName } = sandbox.termlabEditorScratch;

assert.strictEqual(nextScratchName([]), 'scratch-1.txt');
assert.strictEqual(nextScratchName(['scratch-1.txt']), 'scratch-2.txt');

// Gaps are filled rather than skipped — the first free number wins.
assert.strictEqual(nextScratchName(['scratch-1.txt', 'scratch-3.txt']), 'scratch-2.txt');

// Unrelated files in the directory are ignored.
assert.strictEqual(nextScratchName(['notes.md', 'scratch-1.txt']), 'scratch-2.txt');

// Never collides with an existing name, whatever the ordering.
assert.strictEqual(
  nextScratchName(['scratch-3.txt', 'scratch-1.txt', 'scratch-2.txt']),
  'scratch-4.txt',
);

// Degenerate input does not throw.
assert.strictEqual(nextScratchName(null), 'scratch-1.txt');
assert.strictEqual(nextScratchName(undefined), 'scratch-1.txt');

console.log('scratch naming: all assertions passed');
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `node scripts/tests/test_scratch_naming.mjs`
Expected: FAIL — cannot read `nextScratchName` of undefined.

- [ ] **Step 3: Implement `scratch.js`**

Create `crates/termlab_tauri/frontend/app/features/editor/scratch.js`:

```js
// Scratch file naming.
//
// Scratches are real files from the moment they are created, so the name has
// to be free on disk before the file is written — hence a pure function over
// the directory listing rather than a session counter.
(function initTermLabEditorScratch(global) {
  'use strict';

  function nextScratchName(existing) {
    const taken = new Set(Array.isArray(existing) ? existing : []);
    let n = 1;
    while (taken.has(`scratch-${n}.txt`)) n += 1;
    return `scratch-${n}.txt`;
  }

  global.termlabEditorScratch = { nextScratchName };
})(window);
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `node scripts/tests/test_scratch_naming.mjs`
Expected: `scratch naming: all assertions passed`

- [ ] **Step 5: Add a directory-listing command for scratches**

`nextScratchName` needs the existing names. Add to `crates/termlab_tauri/src/editor_fs.rs`, above the tests:

```rust
/// The file names already in the scratch directory, so the frontend can pick
/// a free scratch name without a round trip per candidate.
#[tauri::command]
pub(crate) fn editor_scratch_list() -> Result<Vec<String>, String> {
    let dir = termlab_core::config::config_dir().join("scratches");
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let entries =
        fs::read_dir(&dir).map_err(|e| format!("Could not read scratch directory: {e}"))?;
    Ok(entries
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect())
}
```

Register `editor_fs::editor_scratch_list` in `lib.rs`'s `invoke_handler`.

- [ ] **Step 6: Implement `editor-service.js`**

Create `crates/termlab_tauri/frontend/app/features/editor/editor-service.js`:

```js
// Open and save orchestration for the editor.
//
// The one place that knows a file has a location as well as contents: it
// reads through the Rust guards, hands the text to a tab, and writes it back.
(function initTermLabEditorService(global) {
  'use strict';

  function invoke(command, args) {
    const client = global.termlabServices && global.termlabServices.tauriClient;
    if (!client || typeof client.invoke !== 'function') {
      return Promise.reject(new Error('tauri client unavailable'));
    }
    return client.invoke(command, args);
  }

  // beforeBuildCommand only fires under `cargo tauri build`/`dev`, so a plain
  // `cargo run` yields an index.html pointing at a bundle that was never
  // generated. Say so instead of failing as an editor that does nothing.
  function bundleMissing() {
    if (global.CM6) return false;
    global.toast.error(
      'Editor unavailable',
      'The editor bundle is missing. Run "npm run build:vendor" in crates/termlab_tauri/frontend.',
    );
    return true;
  }

  function tabManager() {
    return global.termlabTabManager || global.tabManager;
  }

  function currentPane() {
    const panes = global.termlabPanes || (global.paneManager && global.paneManager.getPanes());
    const focused = global.paneManager && global.paneManager.getFocusedPaneId();
    return panes && focused != null ? panes.get(focused) : null;
  }

  // Opening a file that is already open focuses its tab instead of making a
  // second view of the same bytes — two editors on one path would each hold a
  // doc and the last save would silently win.
  function focusExistingEditor(filePath) {
    let found = null;
    eachEditorPane((pane) => {
      if (!found && pane.filePath === filePath) found = pane;
    });
    if (!found) return false;
    tabManager().activateTab(found.tabId);
    if (global.paneManager) global.paneManager.setFocusedPane(found.paneId);
    return true;
  }

  async function openLocalFile(filePath) {
    if (bundleMissing()) return;
    if (focusExistingEditor(filePath)) return;
    try {
      const contents = await invoke('editor_read_file', { path: filePath });
      tabManager().createEditorTab({ filePath, contents, remote: null });
    } catch (error) {
      global.toast.error('Cannot Open File', String(error));
    }
  }

  async function openScratch() {
    if (bundleMissing()) return;
    try {
      const [dir, existing] = await Promise.all([
        invoke('editor_scratch_dir'),
        invoke('editor_scratch_list'),
      ]);
      const name = global.termlabEditorScratch.nextScratchName(existing);
      const filePath = `${dir}/${name}`;
      await invoke('editor_write_file', { path: filePath, contents: '' });
      tabManager().createEditorTab({ filePath, contents: '', remote: null });
    } catch (error) {
      global.toast.error('Cannot Create Scratch', String(error));
    }
  }

  async function saveActiveEditor() {
    const pane = currentPane();
    if (!pane || pane.kind !== 'editor' || !pane.view) return;
    const contents = pane.view.state.doc.toString();
    try {
      await invoke('editor_write_file', { path: pane.filePath, contents });
      pane.view.termlabResetDirty();
      if (pane.remote) await uploadRemote(pane);
    } catch (error) {
      global.toast.error('Save Failed', String(error));
    }
  }

  // Replaced with the real implementation in Task 8; a local-only save has
  // nothing to upload.
  async function uploadRemote(_pane) {}

  function eachEditorPane(fn) {
    const panes = global.termlabPanes || (global.paneManager && global.paneManager.getPanes());
    if (!panes) return;
    for (const pane of panes.values()) {
      if (pane && pane.kind === 'editor') fn(pane);
    }
  }

  global.termlabEditorService = {
    openLocalFile,
    openScratch,
    saveActiveEditor,
    eachEditorPane,
    uploadRemote,
  };
})(window);
```

- [ ] **Step 7: Add the keymap fields**

In `crates/termlab_core/src/config/termlab.rs`, add to the `KeyboardConfig` struct, after `pub settings: String,`:

```rust
    pub new_scratch: String,
    pub save_file: String,
```

And to its `Default` impl, after `settings: "cmd+,".into(),`:

```rust
            new_scratch: "cmd+n".into(),
            save_file: "cmd+s".into(),
```

- [ ] **Step 8: Add a test for the new defaults**

In the same file's test module, add:

```rust
    #[test]
    fn editor_shortcuts_have_the_documented_defaults() {
        let k = KeyboardConfig::default();
        assert_eq!(k.new_scratch, "cmd+n");
        assert_eq!(k.save_file, "cmd+s");
    }
```

Run: `cargo test -p termlab_core keyboard`
Expected: PASS.

- [ ] **Step 9: Route the shortcuts to actions**

In `crates/termlab_tauri/frontend/app/shortcut-runtime.js`, add to `coreShortcutActionByKey` (line 28):

```js
      new_scratch: 'new-scratch',
      save_file: 'save-file',
```

In `menu-actions.js`'s `handleMenuAction` (line 37), add:

```js
      if (action === 'new-scratch') {
        global.termlabEditorService.openScratch();
        return;
      }
      if (action === 'save-file') {
        // Scoped to editor panes: in a terminal this must not swallow the
        // keystroke, so the service returns without acting and the event
        // continues to the terminal.
        global.termlabEditorService.saveActiveEditor();
        return;
      }
```

- [ ] **Step 10: Make `cmd+s` pass through in terminal panes**

`runShortcutFallbacks` returns `true` for any matched core action, which consumes the keystroke. `save-file` must only consume it when the focused pane is an editor. In `shortcut-runtime.js`, before the `coreHit` branch acts:

```js
        if (coreHit && coreHit.action === 'save-file') {
          const pane = getCurrentPane();
          if (!pane || pane.kind !== 'editor') return false;
        }
```

- [ ] **Step 11: Add the palette entry**

In `command-palette-runtime.js`, beside the existing `add('core:new-tab', ...)` calls at line 115:

```js
      add('core:new-scratch', 'New Scratch File', 'Editor', 'scratch file editor new note', () => handleMenuAction('new-scratch'));
```

- [ ] **Step 12: Register the scripts**

In `index.html`, after the editor tags from Task 4:

```html
  <script src="app/features/editor/scratch.js"></script>
  <script src="app/features/editor/editor-service.js"></script>
```

- [ ] **Step 13: Verify in the running app**

1. Press `cmd+n` → a `scratch-1.txt` tab opens.
2. Type text → the tab shows `•`.
3. Press `cmd+s` → the marker clears. Confirm on disk:
   ```bash
   ls ~/.config/termlab/scratches/ && cat ~/.config/termlab/scratches/scratch-1.txt
   ```
4. Press `cmd+n` again → `scratch-2.txt`, not a second `scratch-1.txt`.
5. Focus a **terminal** pane and press `cmd+s` → nothing happens in the app and the terminal receives the keystroke (no save toast, no console error).
6. Open the command palette and run "New Scratch File" → a scratch opens.
7. In Settings → Keyboard, confirm "New Scratch" and "Save File" appear and can be rebound.

- [ ] **Step 14: Commit**

```bash
git add crates/termlab_tauri/frontend/app/features/editor/scratch.js \
        crates/termlab_tauri/frontend/app/features/editor/editor-service.js \
        crates/termlab_tauri/src/editor_fs.rs \
        crates/termlab_tauri/src/lib.rs \
        crates/termlab_core/src/config/termlab.rs \
        crates/termlab_tauri/frontend/app/shortcut-runtime.js \
        crates/termlab_tauri/frontend/app/menu-actions.js \
        crates/termlab_tauri/frontend/app/command-palette-runtime.js \
        crates/termlab_tauri/frontend/index.html \
        scripts/tests/test_scratch_naming.mjs
git commit -m "feat: scratch files, save, and editor keybindings"
```

---

## Task 7: Unsaved-Changes Guards

**Files:**
- Modify: `crates/termlab_tauri/frontend/app/core/dialog-service.js`
- Modify: `crates/termlab_tauri/frontend/app/tab-manager.js` (`closeTab` ~line 361)
- Modify: `crates/termlab_tauri/frontend/app/features/editor/editor-service.js`
- Modify: `crates/termlab_tauri/src/lib.rs` (`on_window_event` ~line 471)
- Modify: `crates/termlab_tauri/frontend/app/menu-actions.js` (quit action)

**Interfaces:**
- Consumes: `hasDirtyEditors`, `eachEditorPane`, `saveActiveEditor` (Task 6).
- Produces:
  - `window.termlabDialogService.confirmSave(fileName) -> Promise<'save' | 'discard' | 'cancel'>`
  - `window.termlabEditorService.confirmAllDirty() -> Promise<boolean>` — `true` when it is safe to proceed.

- [ ] **Step 1: Add `confirmSave` to the dialog service**

In `crates/termlab_tauri/frontend/app/core/dialog-service.js`, beside `confirmPluginPermissions`:

```js
  // Three-way close prompt for a modified editor. Escape and the backdrop
  // resolve to 'cancel' — the safe answer, since losing an edit to a stray
  // keystroke is the failure this dialog exists to prevent.
  function confirmSave(fileName) {
    return new Promise((resolve) => {
      if (!global.tlDialog || typeof global.tlDialog.open !== 'function') {
        resolve('cancel');
        return;
      }

      let done = false;
      let handle = null;
      const finish = (choice) => {
        if (done) return;
        done = true;
        resolve(choice);
        if (handle) handle.close(choice);
      };

      handle = global.tlDialog.open({
        title: 'Unsaved Changes',
        ariaLabel: 'Unsaved changes',
        size: 'sm',
        body: (bodyEl) => {
          bodyEl.innerHTML =
            `<div class="tl-dialog-message">Save changes to "${escHtml(fileName)}" before closing?</div>`;
        },
        buttons: [
          { label: 'Cancel', onSelect: () => finish('cancel') },
          { label: "Don't Save", onSelect: () => finish('discard') },
          { label: 'Save', primary: true, onSelect: () => finish('save') },
        ],
        onClose: () => finish('cancel'),
      });
    });
  }
```

Add `confirmSave` to the exported object at the bottom of the file.

- [ ] **Step 2: Add `confirmAllDirty` and a per-pane save to the editor service**

In `editor-service.js`, add:

```js
  // Save a specific pane. saveActiveEditor covers the keyboard path; the close
  // guards need to save panes that are not focused.
  async function savePane(pane) {
    if (!pane || pane.kind !== 'editor' || !pane.view) return;
    const contents = pane.view.state.doc.toString();
    await invoke('editor_write_file', { path: pane.filePath, contents });
    pane.view.termlabResetDirty();
    if (pane.remote) await uploadRemote(pane);
  }

  // Walk every dirty editor and ask. Returns false the moment the user
  // cancels, which aborts the whole close rather than the one tab.
  async function confirmAllDirty() {
    const dirty = [];
    eachEditorPane((pane) => { if (pane.dirty) dirty.push(pane); });

    for (const pane of dirty) {
      const name = String(pane.filePath || 'untitled').split('/').pop();
      const choice = await global.termlabDialogService.confirmSave(name);
      if (choice === 'cancel') return false;
      if (choice === 'save') {
        try {
          await savePane(pane);
        } catch (error) {
          global.toast.error('Save Failed', String(error));
          return false; // a failed save must not be treated as consent to lose it
        }
      }
    }
    return true;
  }
```

Export `savePane` and `confirmAllDirty`.

- [ ] **Step 3: Guard tab close**

In `tab-manager.js`'s `closeTab` (already `async`), before the pane loop at line 370:

```js
      // Ask before discarding edits. Skipped when the caller is a close that
      // already asked (window close, quit), which passes skipDirtyCheck.
      if (!options.skipDirtyCheck) {
        const panes0 = getPanes();
        const dirtyPanes = allPanesInTab(tabId)
          .map((pid) => panes0.get(pid))
          .filter((p) => p && p.kind === 'editor' && p.dirty);
        for (const pane of dirtyPanes) {
          const name = String(pane.filePath || 'untitled').split('/').pop();
          const choice = await global.termlabDialogService.confirmSave(name);
          if (choice === 'cancel') return;
          if (choice === 'save') {
            try {
              await global.termlabEditorService.savePane(pane);
            } catch (error) {
              global.toast.error('Save Failed', String(error));
              return;
            }
          }
        }
      }
```

- [ ] **Step 4: Add the `CloseRequested` handler in Rust**

In `crates/termlab_tauri/src/lib.rs`'s `on_window_event` closure (line ~471), which currently handles only `Focused` and `Destroyed`, add:

```rust
            // The webview owns the answer to "is anything unsaved?", so stop
            // the close and ask. The frontend calls `confirm_window_close`
            // when it has one.
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if !window
                    .state::<CloseGuard>()
                    .is_confirmed(window.label())
                {
                    api.prevent_close();
                    let _ = window.emit_to(window.label(), "window-close-requested", ());
                }
            }
```

Add the guard state above the builder:

```rust
/// Tracks windows the frontend has cleared for closing, so the second
/// close attempt is allowed through instead of prompting forever.
#[derive(Default)]
pub(crate) struct CloseGuard(parking_lot::Mutex<std::collections::HashSet<String>>);

impl CloseGuard {
    fn is_confirmed(&self, label: &str) -> bool {
        self.0.lock().contains(label)
    }
    fn confirm(&self, label: &str) {
        self.0.lock().insert(label.to_string());
    }
}

#[tauri::command]
pub(crate) fn confirm_window_close(window: tauri::WebviewWindow, guard: tauri::State<'_, CloseGuard>) {
    guard.confirm(window.label());
    let _ = window.close();
}
```

Register `.manage(CloseGuard::default())` on the builder and `confirm_window_close` in the `invoke_handler`.

- [ ] **Step 5: Answer the close request in the frontend**

In `editor-service.js` (or wherever the app's other `listen` calls are wired — follow the existing pattern in `event-wiring-runtime.js`), add a listener:

```js
      await listenOnCurrentWindow('window-close-requested', async () => {
        const ok = await global.termlabEditorService.confirmAllDirty();
        if (ok) await invoke('confirm_window_close');
      });
```

- [ ] **Step 6: Guard quit**

In `menu-actions.js`'s `handleMenuAction`, find the `quit` action and make it await the same check before quitting:

```js
      if (action === 'quit') {
        (async () => {
          const ok = await global.termlabEditorService.confirmAllDirty();
          if (ok) await invoke('editor_temp_sweep').catch(() => {});
          if (ok) doQuit();
        })();
        return;
      }
```

Use the file's existing quit call in place of `doQuit()`.

- [ ] **Step 7: Verify all three paths**

Test each, recording pass/fail:

1. Open a scratch, type, **close the tab** → prompt appears. Cancel → tab stays, still dirty. Repeat → Don't Save → tab closes, file unchanged on disk. Repeat → Save → tab closes, file has the text.
2. Open two scratches, modify both, **close the window** → two prompts in sequence; Cancel on the second → the window stays open.
3. Modify a scratch and **quit** (`cmd+q`) → prompt appears; Cancel → the app stays running.
4. Modify a scratch, close the tab, choose Save, but make the save fail first:
   ```bash
   chmod 500 ~/.config/termlab/scratches
   ```
   → an error toast appears and the tab does **not** close. Then restore:
   ```bash
   chmod 700 ~/.config/termlab/scratches
   ```
5. A tab with **no** dirty editor closes with no prompt at all.

- [ ] **Step 8: Verify Rust builds clean**

Run: `cargo test -p termlab_tauri && cargo clippy -p termlab_tauri --all-targets`
Expected: pass, no warnings.

- [ ] **Step 9: Commit**

```bash
git add crates/termlab_tauri/frontend/app/core/dialog-service.js \
        crates/termlab_tauri/frontend/app/tab-manager.js \
        crates/termlab_tauri/frontend/app/features/editor/editor-service.js \
        crates/termlab_tauri/frontend/app/menu-actions.js \
        crates/termlab_tauri/src/lib.rs
git commit -m "feat: prompt before discarding unsaved editor changes

Covers tab close, window close (new CloseRequested handler) and quit."
```

---

## Task 8: SFTP Double-Click to Edit

**Files:**
- Modify: `crates/termlab_tauri/frontend/app/features/files/pane-view.js:76`
- Modify: `crates/termlab_tauri/frontend/app/features/editor/editor-service.js`
- Modify: `crates/termlab_tauri/frontend/app/tab-manager.js` (`closeTab` editor arm from Task 5)

**Interfaces:**
- Consumes: `editor_can_open`, `editor_temp_path`, `editor_temp_cleanup` (Task 2); `createEditorTab` with a `remote` argument (Task 5); `openLocalFile` (Task 6).
- Produces: `window.termlabEditorService.openRemoteFile({ paneId, remotePath, hostLabel, size })` and a real `uploadRemote(pane)` replacing Task 6's stub.

- [ ] **Step 1: Implement the remote open flow**

In `editor-service.js`, add:

```js
  // Wait for one transfer to finish. transfer_download/transfer_upload are
  // fire-and-forget and report through the shared 'transfer-progress' event,
  // so completion means watching for our own transfer_id to reach a terminal
  // status — the same pattern files-panel.js uses for its progress bars.
  function awaitTransfer(transferId) {
    return new Promise((resolve, reject) => {
      const client = global.termlabServices && global.termlabServices.tauriClient;
      let unlisten = null;
      let settled = false;
      const finish = (fn, arg) => {
        if (settled) return;
        settled = true;
        if (typeof unlisten === 'function') unlisten();
        fn(arg);
      };
      client.listen('transfer-progress', (event) => {
        const p = event && event.payload;
        if (!p || p.transfer_id !== transferId) return;
        if (p.status === 'completed') finish(resolve);
        else if (p.status === 'failed' || p.status === 'cancelled') {
          finish(reject, new Error(p.error || `Transfer ${p.status}`));
        }
      }).then((fn) => {
        unlisten = fn;
        if (settled) fn();
      });
    });
  }

  async function openRemoteFile(descriptor) {
    if (bundleMissing()) return;
    const { paneId, remotePath, hostLabel, size } = descriptor || {};
    const name = String(remotePath || '').split('/').pop();
    try {
      // Reject before a byte moves.
      await invoke('editor_can_open', { name, size: Number(size) || 0 });

      // The temp path is deterministic per (host, remote path), so this is
      // also how "the same remote file twice" resolves to one tab.
      const localPath = await invoke('editor_temp_path', { hostLabel, remotePath });
      if (focusExistingEditor(localPath)) return;

      const transferId = await invoke('transfer_download', {
        paneId,
        remotePath,
        localPath,
      });
      await awaitTransfer(transferId);

      // Guards again after download: the listing's size can be stale and only
      // now can the contents be sniffed.
      const contents = await invoke('editor_read_file', { path: localPath });
      tabManager().createEditorTab({
        filePath: localPath,
        contents,
        remote: { paneId, remotePath, hostLabel },
      });
    } catch (error) {
      // Anything that failed after the temp path existed leaves a stray file.
      global.toast.error('Cannot Open File', String(error));
    }
  }
```

- [ ] **Step 2: Replace the upload stub**

Replace Task 6's empty `uploadRemote` with:

```js
  async function uploadRemote(pane) {
    const { paneId, remotePath, hostLabel } = pane.remote;
    try {
      const transferId = await invoke('transfer_upload', {
        paneId,
        localPath: pane.filePath,
        remotePath,
      });
      await awaitTransfer(transferId);
      global.toast.success('Uploaded', `${hostLabel}:${remotePath}`);
    } catch (error) {
      // The temp file stays put so Retry has something to send. Losing an
      // edit to a dropped connection is the wrong side to be wrong on.
      global.toast.error('Upload Failed', `${String(error)} — reopen and save to retry.`);
      throw error;
    }
  }
```

- [ ] **Step 3: Wire the double-click**

In `crates/termlab_tauri/frontend/app/features/files/pane-view.js`, at the `dblclick` handler on line 76, the directory branch is unchanged. Add the file branch:

```js
        if (entry.is_dir) {
          navigateTo(entry.path);
          return;
        }
        if (isRemotePane) {
          global.termlabEditorService.openRemoteFile({
            paneId: currentPaneId,
            remotePath: entry.path,
            hostLabel: currentHostLabel,
            size: entry.size,
          });
        } else {
          global.termlabEditorService.openLocalFile(entry.path);
        }
```

Use the file's existing names for the entry object, the directory-navigation call, the remote/local discriminator, and the pane and host identifiers — read the surrounding function rather than assuming these match.

- [ ] **Step 4: Clean up temp files on tab close**

In `tab-manager.js`, extend the editor arm added in Task 5 Step 3:

```js
        } else if (pane.kind === 'editor') {
          if (pane.view) global.termlabEditorPane.destroyEditorView(pane.view);
          if (pane.remote && pane.filePath) {
            global.termlabServices.tauriClient
              .invoke('editor_temp_cleanup', { path: pane.filePath })
              .catch(() => {});
          }
        }
```

- [ ] **Step 5: Sweep temp files on quit**

Task 7 Step 6 already calls `editor_temp_sweep` before quitting. Confirm it is still there and runs before the quit call, not after.

- [ ] **Step 6: Verify the local path**

1. In the SFTP **local** pane, double-click a text file → it opens in an editor tab with the right syntax highlighting.
2. Edit it, `cmd+s`, then confirm on disk with `cat`.
3. Double-click that **same** file again → the existing tab focuses; no second tab, and any unsaved edit in it survives.
4. Double-click a `.png` → a rejection toast, no tab.
5. Double-click a file over 5 MB → a rejection toast naming the size, no tab.

- [ ] **Step 7: Verify the remote path**

Against a real SSH host:

1. Double-click a remote text file → downloads and opens.
2. Edit, `cmd+s` → an "Uploaded" toast; verify the change from a terminal on that host (`cat` the file).
3. Double-click the **same** file again → the existing tab focuses; no duplicate tab.
4. Double-click the same filename on a **different** host → a separate tab, and the two do not overwrite each other.
5. Double-click a 10 MB remote file → rejected with no transfer (watch that no progress bar appears).
6. Double-click a remote `.jar` → rejected with no transfer.
7. Double-click a remote file with a `.txt` extension but binary contents → downloads, then rejects; confirm the temp file is gone:
   ```bash
   find "$(node -e 'console.log(require("os").tmpdir())')/termlab-sftp-edits" -type f
   ```
8. Disconnect the SSH session, then `cmd+s` on an open remote editor → "Upload Failed" toast; the temp file still exists (same `find`), so nothing was lost.
9. Close a remote editor tab → its temp file and empty parent directories are gone.
10. Quit the app → the whole `termlab-sftp-edits` directory is gone.

- [ ] **Step 8: Run every test one more time**

```bash
cd /Users/dustin/projects/conch && cargo test --workspace && cargo clippy --all-targets \
  && node scripts/tests/test_language_map.mjs \
  && node scripts/tests/test_scratch_naming.mjs \
  && ./scripts/check_frontend_boundaries.sh .
```
Expected: everything passes.

- [ ] **Step 9: Commit**

```bash
git add crates/termlab_tauri/frontend/app/features/files/pane-view.js \
        crates/termlab_tauri/frontend/app/features/editor/editor-service.js \
        crates/termlab_tauri/frontend/app/tab-manager.js
git commit -m "feat: SFTP double-click opens files in the editor

Local files open directly; remote files download to a temp path, upload
on save, and clean up on close."
```

---

## Verification Summary

After all eight tasks, these must all hold:

- `cargo test --workspace` and `cargo clippy --all-targets` pass.
- `node scripts/tests/test_language_map.mjs` and `test_scratch_naming.mjs` pass.
- `./scripts/check_frontend_boundaries.sh .` passes.
- The eleven manual checks in the spec's Testing section have been run, each recorded pass or fail.
- The recorded CodeMirror bundle size is in the final report.
- No app module imports another app module — the build step covers third-party dependencies only.

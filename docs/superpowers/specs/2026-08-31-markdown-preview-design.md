# Markdown Preview for the Light Editor — Design

**Date:** 2026-08-31
**Status:** Designed (not yet implemented)
**Owner request:** Preview markdown files as rendered output the way VS Code
and IntelliJ do, inside the light editor.

## Summary

Markdown files opened in the light editor gain three view modes — **Editor**,
**Split**, **Preview** — toggled per pane. The rendered output is produced by
`markdown-it` (GFM), sanitized with an allowlist, and displayed inside a
sandboxed `<iframe>` that cannot execute scripts. Fenced code blocks are
highlighted by reusing the CodeMirror grammars the app already vendors, so
highlighting matches the editor and follows the active theme.

The preview is offline by design: local images are read through Rust and
inlined as `data:` URIs, images on remote (SFTP) files are fetched over the
existing SFTP session, and `http(s)` image loads are stripped during
sanitizing so they never issue a request.

Scroll sync is one-directional — the editor drives the preview — using source
line numbers that `markdown-it` attaches to every block token.

## Why the containment matters here

`crates/termlab_tauri/tauri.conf.json` sets `"csp": null`, and the webview has
`__TAURI__.invoke` available to it. A script that executes in this document can
read and write files, drive SSH sessions, and reach the credential vault.

The existing plugin renderer assigns raw plugin HTML straight to `innerHTML`
(`app/panels/plugin-widgets.js`, the `html` widget). That is a defensible trust
decision for plugins: a plugin is explicitly enabled by the user and its
capabilities are permission-gated.

**Markdown does not inherit that trust.** A `.md` file is untrusted content the
user merely opens — from a cloned repository, a downloaded archive, or an SFTP
host — with no enable step and no permission prompt. Opening a file must never
be able to execute code. This design therefore uses two independent barriers
rather than reusing the plugin path.

## Current state (what this builds on)

- `app/features/editor/editor-pane.js` owns a CodeMirror `EditorView` per pane
  and deliberately knows nothing about files, tabs, or saving. It uses
  compartments to reconfigure font, theme, vim, and language on a live view.
- `app/features/editor/editor-service.js` owns file meaning: open, save, Save
  As, remote binding, tab labels.
- `app/features/editor/language-map.js` already maps `md`/`markdown` to the
  `markdown` CodeMirror language.
- `frontend/vendor-entry.mjs` is the single ESM entry esbuild bundles into the
  `CM6` IIFE global. Anything the app needs from a third-party ESM package must
  be re-exported there first.
- Remote files are downloaded to a temp path and uploaded back on save. The
  pane's binding is `pane.remote = { paneId, remotePath, hostLabel }`.
- `sftp_read_file(paneId, path, offset, length)` returns **base64** data and
  caps each call at 1MB, so it is already binary-safe for images.
- `editor_fs.rs` exposes `editor_read_file`, which returns a `String` and is
  therefore text-only — not usable for images.

## Design

### View modes

A markdown pane has one of three modes:

| Mode | Layout |
|---|---|
| `editor` | CodeMirror only (current behavior) |
| `split` | CodeMirror left, preview right, within the same pane |
| `preview` | Preview only |

Mode is per-pane runtime state. It is offered only for files the language map
resolves to `markdown`; for every other file the toggle is inert and the pane
behaves exactly as it does today.

This is deliberately *not* a new pane kind in the split tree. Binding the
preview to its own document inside one pane makes it impossible to orphan a
preview or leave one pointing at a closed file, and it composes with the
existing split system — each split pane can independently be in preview mode.

### Modules

New, under `app/features/editor/preview/`:

| Module | Responsibility | Depends on |
|---|---|---|
| `markdown-renderer.js` | Pure: markdown string → `{ html, sourceMap }`. No DOM, no Tauri, no I/O. | vendored parser |
| `preview-frame.js` | Owns the sandboxed iframe: content injection, theme CSS, scroll-to-line, link interception | DOM |
| `preview-mode.js` | The three-mode state machine for one pane; owns the internal layout split | the two above |
| `image-resolver.js` | Image `src` → `data:` URI, with cache and per-render cancellation | Tauri commands |

`markdown-renderer.js` being pure and I/O-free is the load-bearing boundary of
this design: it is what lets the parser and the entire sanitizer XSS corpus run
as plain Node tests with no GUI context, alongside the existing
`scripts/tests/*.mjs` suites.

`editor-pane.js` is 165 lines today and should stay small. It gains only the
plumbing to mount and unmount a preview alongside its view; the preview logic
itself lives in the modules above.

### Render pipeline

```
.md text
  → markdown-it (GFM preset)      produces HTML + per-token source maps
  → data-src-line attributes      emitted on top-level blocks
  → DOMPurify (allowlist)         strips scripts, handlers, remote images
  → iframe srcdoc                 sandboxed, scripts disabled
```

**Parser: `markdown-it`.** Chosen over `marked` because its tokens carry
`.map` (`[startLine, endLine]`), which is exactly the data scroll sync needs.
`marked` is smaller but provides no line mapping, which would leave that to be
reverse-engineered. `markdown-it` is also what VS Code uses, and its plugin
model leaves the door open for Mermaid later without changing this pipeline.

**Code fences reuse CodeMirror.** A custom `highlight` hook runs fence contents
through the grammars already re-exported from `vendor-entry.mjs`, emitting
spans that carry the same highlight classes the editor uses. No second grammar
set, no `highlight.js`, and fence highlighting follows the active theme for
free.

### Sandbox configuration

```html
<iframe sandbox="allow-same-origin">
```

`allow-scripts` is deliberately **absent**. This combination is the point of
the design:

- Without `allow-scripts`, any `<script>` that survives sanitizing never
  executes, and no inline handler ever fires.
- With `allow-same-origin`, the **parent** can reach into the frame's DOM
  directly to drive scrolling and intercept link clicks.

Because the parent drives the frame by DOM access, there is no message
protocol at all. A `postMessage` design would need a script *inside* the frame
to receive messages, which would require `allow-scripts` and defeat the
sandbox.

DOMPurify remains the first barrier. Sanitizer bypasses are found regularly;
the sandbox is what makes a bypass a rendering bug instead of a compromise.

### Sanitizer policy

- Allow standard markdown-produced structure, plus the raw HTML elements real
  READMEs use: `<details>`, `<summary>`, `<img>`, `<br>`, `<kbd>`, `<sup>`,
  `<sub>`, and inline formatting.
- Strip `<script>`, `<style>`, `<iframe>`, `<object>`, `<embed>`, `<form>`,
  every `on*` attribute, and `javascript:` / `data:text/html` URLs.
- Allow `data-src-line` (scroll sync depends on it surviving).
- Rewrite `img[src]`: local and remote-file paths become resolver placeholders;
  `http(s)` sources are **dropped here**, so a blocked image never issues a
  request.
- `a[href]` is restricted to `http`, `https`, `mailto`, and relative paths.

### Images

**Local.** New `editor_read_image_base64(path)` command in `editor_fs.rs`,
mirroring the extension and size guards already in `editor_can_open`. Paths
resolve relative to the file's own directory.

**Remote.** Resolve relative paths against `dirname(pane.remote.remotePath)`,
then loop `sftp_read_file(paneId, path, offset, 1MB)` until a short read,
concatenating the base64 chunks. This reuses the pane's existing SFTP session;
no new backend command is required.

Two mechanisms keep the debounced re-render from becoming SFTP traffic:

- **Cache** — a `Map` keyed `${paneId}:${absolutePath}` holding the finished
  data URI, populated once per image and cleared when the pane closes. After
  the first render, subsequent renders cost no I/O.
- **Generation counter** — every render increments a counter; a fetch that
  resolves against a stale generation discards its result rather than writing
  into a frame that has moved on. This is what prevents an edit burst from
  queuing dozens of round-trips.

Images render as a sized placeholder and are swapped in on arrival, so a slow
link never blocks the text from displaying.

### Scroll sync

`markdown-it` attaches `.map` to every block token. The renderer emits
`data-src-line="<startLine>"` on each top-level block, and the sanitizer
allowlist preserves it.

On editor scroll (throttled with `requestAnimationFrame`), the parent reads the
topmost visible line from the CodeMirror view, finds the last frame element
whose `data-src-line` is `<=` that line, and scrolls the frame to it.

Sync is one-directional. Scrolling the preview does not move the editor, which
removes the need for loop-breaking guards and avoids the jitter bidirectional
sync is prone to when source lines and rendered blocks have very different
heights.

### Update timing

Re-render is debounced at ~150ms after the last document change. Parsing a
typical README is sub-millisecond; the debounce exists to avoid re-resolving
images and re-laying-out the frame on every keystroke.

### Config

```toml
[editor]
preview_default_mode = "editor"   # "editor" | "split" | "preview"

[termlab.keyboard]
toggle_preview = "cmd+shift+p"    # cycles editor -> split -> preview
```

Both fields are `#[serde(default)]`, so existing configs keep working
unchanged. `preview_default_mode` sets only the initial mode when a markdown
file is opened; the live mode is per-pane runtime state and is not persisted.

`cmd+shift+p` is free: it appears in the tree only as a test fixture in
`menu.rs` (`config_key_to_accelerator_cmdorctrl_uses_primary_modifier`), not as
a registered accelerator, and it is absent from `KeyboardConfig::default`. The
taken `cmd+shift+*` defaults are `[ ] d e j m n o r s t v w z`.

### Rust changes (additive only)

- `editor_fs.rs` — add `editor_read_image_base64`, with extension and size
  guards.
- `config/editor.rs` — add `preview_default_mode`.
- `config/termlab.rs` — add `toggle_preview` to `KeyboardConfig`.
- `lib.rs` — register the new command.

No changes to `termlab_remote`; the SFTP path is reused as-is.

### Files touched

**New**
- `app/features/editor/preview/markdown-renderer.js`
- `app/features/editor/preview/preview-frame.js`
- `app/features/editor/preview/preview-mode.js`
- `app/features/editor/preview/image-resolver.js`
- `styles/design-system/components/markdown-preview.css`
- `scripts/tests/test_markdown_render.mjs`
- `scripts/tests/test_markdown_sanitize.mjs`
- `scripts/tests/test_markdown_image_resolve.mjs`

**Modified**
- `frontend/vendor-entry.mjs` — re-export `markdown-it` and `DOMPurify`
- `frontend/package.json` — add both as devDependencies
- `frontend/index.html` — script tags for the four new modules
- `app/features/editor/editor-pane.js` — mount/unmount plumbing only
- `app/features/editor/editor-service.js` — offer the toggle for markdown panes
- `app/features/editor/language-map.js` — add an `isMarkdown(filename)` helper
- `crates/termlab_tauri/src/editor_fs.rs`
- `crates/termlab_core/src/config/editor.rs`
- `crates/termlab_core/src/config/termlab.rs`
- `crates/termlab_tauri/src/lib.rs`
- `config.example.toml`, `README.md`, `CLAUDE.md`

### Error handling

- Parse failure renders as an inline error block inside the frame, not a toast
  — the preview is the right place to report that the preview failed.
- A failed image becomes a broken-image placeholder showing its path.
- A missing vendor bundle falls back to Editor mode with the toggle disabled,
  matching the existing `bundleMissing()` pattern in `editor-service.js`.
- An SFTP fetch failure degrades to a placeholder; it never surfaces a modal or
  interrupts editing.

### Out of scope

- Mermaid diagrams and LaTeX math. The `markdown-it` plugin model leaves room
  for both without reworking this pipeline.
- Remote `http(s)` image loading. Deliberately blocked; revisit only behind an
  explicit opt-in setting.
- Bidirectional scroll sync.
- Exporting or printing the rendered output.
- A preview as a standalone pane in the split tree.
- Persisting per-pane mode across restarts.

## Testing

| Suite | Covers |
|---|---|
| `test_markdown_render.mjs` | GFM constructs → expected HTML; `data-src-line` mapping is present and monotonic |
| `test_markdown_sanitize.mjs` | XSS corpus: `<script>`, `onerror=`, `javascript:` hrefs, `<iframe>`, SVG payloads, `data:text/html`, and `http(s)` image stripping |
| `test_markdown_image_resolve.mjs` | Relative and absolute path resolution, cache hit behavior, stale-generation cancellation |
| `editor_fs.rs` `#[cfg(test)]` | Extension and size guards on `editor_read_image_base64` |
| `config/editor.rs` `#[cfg(test)]` | `preview_default_mode` default and serde round-trip |

The sanitizer corpus is non-negotiable given `csp: null`, and should be
extended with any bypass discovered later rather than being treated as
complete.

## Manual verification

1. Open a README with tables, task lists, fenced code, and a local image;
   confirm all four render and the fence highlighting matches the editor theme.
2. Toggle Editor → Split → Preview and back; confirm undo history and cursor
   position survive.
3. Type in Split mode; confirm the preview follows at roughly 150ms and does
   not flicker.
4. Scroll the editor in a long document; confirm the preview tracks.
5. Open a markdown file over SFTP with a relative image; confirm it loads once
   and that continued typing produces no further SFTP traffic.
6. Open a markdown file containing `<script>alert(1)</script>`,
   `<img src=x onerror=alert(1)>`, and an `https://` image; confirm nothing
   executes and the remote image does not load.
7. Switch theme with a preview open; confirm the preview restyles.

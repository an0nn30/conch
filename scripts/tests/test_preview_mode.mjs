// Run: node scripts/tests/test_preview_mode.mjs
//
// The mode machine is pure state — no DOM, no CodeMirror — so it is tested
// directly. What matters is that an unknown mode degrades to 'editor' rather
// than throwing: preview_default_mode comes from a hand-edited config file, and
// a typo there must not stop files from opening.
import assert from 'node:assert';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';

const ROOT = path.resolve(import.meta.dirname, '../../crates/termlab_tauri/frontend');
const sandbox = { window: {} };
sandbox.window = sandbox;
vm.createContext(sandbox);
vm.runInContext(fs.readFileSync(path.join(ROOT, 'app/features/editor/language-map.js'), 'utf8'), sandbox);
vm.runInContext(fs.readFileSync(path.join(ROOT, 'app/features/editor/preview/preview-mode.js'), 'utf8'), sandbox);

// --- markdown detection ----------------------------------------------------
const { isMarkdown } = sandbox.termlabEditorLanguageMap;
for (const name of ['a.md', 'B.MD', 'notes.markdown', '/p/to/README.md']) {
  assert.ok(isMarkdown(name), `${name} must be markdown`);
}
for (const name of ['a.txt', 'a.rs', 'markdown', '', null, undefined]) {
  assert.ok(!isMarkdown(name), `${JSON.stringify(name)} must not be markdown`);
}

// --- cycle order -----------------------------------------------------------
const { createModeState } = sandbox.termlabPreviewMode;
{
  const s = createModeState('editor');
  assert.strictEqual(s.mode(), 'editor');
  assert.strictEqual(s.cycle(), 'split');
  assert.strictEqual(s.cycle(), 'preview');
  assert.strictEqual(s.cycle(), 'editor', 'cycle must wrap back to editor');
}

// --- visibility derives from mode -----------------------------------------
{
  const s = createModeState('split');
  assert.ok(s.showsEditor() && s.showsPreview(), 'split shows both');
  s.set('preview');
  assert.ok(!s.showsEditor() && s.showsPreview(), 'preview hides the editor');
  s.set('editor');
  assert.ok(s.showsEditor() && !s.showsPreview(), 'editor hides the preview');
}

// --- bad input degrades ----------------------------------------------------
{
  assert.strictEqual(createModeState('nonsense').mode(), 'editor', 'unknown initial mode falls back');
  assert.strictEqual(createModeState(undefined).mode(), 'editor', 'absent initial mode falls back');
  const s = createModeState('editor');
  s.set('nope');
  assert.strictEqual(s.mode(), 'editor', 'set() ignores an unknown mode');
}

console.log('test_preview_mode: ok');

// Run: node scripts/tests/test_tl_icon.mjs
import assert from 'node:assert';
const window = {};
globalThis.window = window;
const { readFileSync } = await import('node:fs');
eval(readFileSync('crates/termlab_tauri/frontend/app/ui/tl-icon.js', 'utf8'));

assert.equal(window.tlIcon.resolve('add', true), 'vendor/intellij-icons/add_dark.svg');
assert.equal(window.tlIcon.resolve('add', false), 'vendor/intellij-icons/add.svg');
// Icons without a dark variant fall back to the base file.
window.tlIcon._setDarkVariants(new Set(['add']));
assert.equal(window.tlIcon.resolve('web', true), 'vendor/intellij-icons/web.svg');
console.log('ok');

// --- Files-panel toolbar glyphs + string API --------------------------------
// The SFTP toolbar renders through tlIcon (branded, appearance-aware) rather
// than the legacy baked-PNG assets that were invisible on dark themes. These
// names must resolve dark variants, and tlIcon.html() must produce a stamped
// <img> string usable inside innerHTML templates so refreshAll() heals it on
// appearance flips.
eval(readFileSync('crates/termlab_tauri/frontend/app/ui/tl-icon.js', 'utf8')); // fresh darkVariants
for (const name of ['back', 'forward', 'home', 'toggleVisibility']) {
  assert.equal(window.tlIcon.resolve(name, true), `vendor/intellij-icons/${name}_dark.svg`,
    `${name} must have a dark variant`);
  assert.equal(window.tlIcon.resolve(name, false), `vendor/intellij-icons/${name}.svg`);
}

// No document in this harness: html() must fall back to the dark appearance
// (the app default) instead of throwing.
const backHtml = window.tlIcon.html('back', { size: 12 });
assert.ok(backHtml.includes('data-tl-icon="back"'), 'html() stamps the logical name');
assert.ok(backHtml.includes('src="vendor/intellij-icons/back_dark.svg"'), 'html() resolves the current variant');
assert.ok(backHtml.includes('width="12"') && backHtml.includes('height="12"'), 'html() honors size');
assert.ok(backHtml.includes('class="tl-icon"'), 'html() renders the shared icon class');

// The vendored assets must actually exist — a name in darkVariants with no
// file on disk renders a broken image at runtime.
const { existsSync } = await import('node:fs');
for (const name of ['back', 'forward', 'home', 'toggleVisibility']) {
  for (const file of [`${name}.svg`, `${name}_dark.svg`]) {
    assert.ok(existsSync(`crates/termlab_tauri/frontend/vendor/intellij-icons/${file}`),
      `vendored asset ${file} must exist`);
  }
}
console.log('tl-icon toolbar glyphs: ok');

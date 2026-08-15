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

// Run: node scripts/tests/test_project_recents.mjs
//
// Recent projects and per-project layouts (Task 12): the frontend wiring
// around the Rust side implemented in crates/termlab_tauri/src/project/recents.rs
// and crates/termlab_tauri/src/commands.rs. This is pure source-text
// inspection — no jsdom in this repo (see test_tl_dialog.mjs for the
// precedent) — the same style test_project_git_tints.mjs uses for its
// wiring checks.
import assert from 'node:assert';
import fs from 'node:fs';
import path from 'node:path';

const ROOT = path.resolve(import.meta.dirname, '../../crates/termlab_tauri/frontend');
const APP = path.join(ROOT, 'app');

let ran = 0;
let failures = 0;
const queued = [];
function check(name, fn) { queued.push({ name, fn }); }

check('the palette offers a Reopen Project entry per recent', () => {
  const src = fs.readFileSync(path.join(APP, 'command-palette-runtime.js'), 'utf8');
  assert.ok(src.includes("invoke('project_recents')"), 'the palette reads the recents list');
  assert.ok(src.includes('Reopen Project: '), 'each recent is its own entry');
  assert.ok(src.includes("invoke('project_open'"), 'choosing one opens it');
});

check('the File menu carries Open Recent Project in both builders', () => {
  const menuRs = fs.readFileSync(
    path.resolve(import.meta.dirname, '../../crates/termlab_tauri/src/menu.rs'), 'utf8');
  assert.ok(menuRs.includes('MENU_RECENT_PROJECT_PREFIX'), 'recent items have an id prefix');
  assert.ok(menuRs.includes('"Open Recent Project"'), 'the submenu is titled');
  const occurrences = menuRs.split('MENU_RECENT_PROJECT_PREFIX').length - 1;
  assert.ok(occurrences >= 3, `both builders must carry it, saw ${occurrences} references`);
});

check('menu-actions opens a recent by path', () => {
  const src = fs.readFileSync(path.join(APP, 'menu-actions.js'), 'utf8');
  assert.ok(src.includes("open-recent-project:"), 'the prefixed action is handled');
});

check('the layout commands are project-aware', () => {
  const src = fs.readFileSync(
    path.resolve(import.meta.dirname, '../../crates/termlab_tauri/src/commands.rs'), 'utf8');
  assert.ok(src.includes('project_layouts'), 'save and restore go through the per-project map');
  assert.ok(src.includes('ProjectRegistry'), 'the calling window resolves its project');
});

for (const { name, fn } of queued) {
  ran += 1;
  try {
    await fn();
    console.log(`  ok   ${name}`);
  } catch (error) {
    failures += 1;
    console.log(`  FAIL ${name}`);
    console.log(`       ${(error && error.stack) || error}`);
  }
}
if (failures) {
  console.log(`project recents: ${failures} of ${ran} checks FAILED`);
  process.exitCode = 1;
} else {
  console.log(`project recents: all ${ran} checks passed`);
}

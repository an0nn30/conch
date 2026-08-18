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
  // Multi-dot basenames: only the characters after the *final* dot decide, so
  // an inner dot must not be mistaken for the extension boundary.
  ['archive.tar.gz', null],
  ['app.min.js', 'javascript'],
  ['config.local.yml', 'yaml'],
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

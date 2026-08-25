// Run: node scripts/tests/test_files_breadcrumbs.mjs
//
// Breadcrumb path model for the file panes: segments(path) turns an absolute
// path into clickable ancestors, collapse() folds deep paths into
// head + hidden-middle + tail so the bar never overflows. Pure logic, no DOM.

import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';

const MODULE_PATH = path.resolve(
  import.meta.dirname,
  '../../crates/termlab_tauri/frontend/app/features/files/breadcrumbs.js',
);

function load() {
  const sandbox = { console };
  sandbox.window = sandbox;
  vm.createContext(sandbox);
  vm.runInContext(fs.readFileSync(MODULE_PATH, 'utf8'), sandbox, { filename: MODULE_PATH });
  assert.ok(sandbox.termlabFilesBreadcrumbs, 'IIFE must expose window.termlabFilesBreadcrumbs');
  return sandbox.termlabFilesBreadcrumbs;
}

const plain = (v) => JSON.parse(JSON.stringify(v));

// POSIX segmentation: every segment carries the full path it navigates to.
{
  const bc = load();
  assert.deepEqual(plain(bc.segments('/home/dustin')), [
    { label: '/', path: '/' },
    { label: 'home', path: '/home' },
    { label: 'dustin', path: '/home/dustin' },
  ]);
  assert.deepEqual(plain(bc.segments('/')), [{ label: '/', path: '/' }]);
  assert.deepEqual(plain(bc.segments('/srv/')), [
    { label: '/', path: '/' },
    { label: 'srv', path: '/srv' },
  ], 'trailing slash is ignored');
}

// Windows drive paths keep the drive as the root segment.
{
  const bc = load();
  assert.deepEqual(plain(bc.segments('C:\\Users\\dustin')), [
    { label: 'C:', path: 'C:\\' },
    { label: 'Users', path: 'C:\\Users' },
    { label: 'dustin', path: 'C:\\Users\\dustin' },
  ]);
}

// Garbage in, something sane out: empty and relative paths become one
// non-navigating segment rather than throwing.
{
  const bc = load();
  assert.deepEqual(plain(bc.segments('')), []);
  assert.deepEqual(plain(bc.segments('relative/dir')), [{ label: 'relative/dir', path: 'relative/dir' }]);
}

// collapse() keeps the root and the last segments, folding the middle.
{
  const bc = load();
  const segs = bc.segments('/a/b/c/d/e/f');
  const short = bc.collapse(bc.segments('/home/dustin'), 4);
  assert.deepEqual(plain(short), {
    head: { label: '/', path: '/' },
    hidden: [],
    tail: [{ label: 'home', path: '/home' }, { label: 'dustin', path: '/home/dustin' }],
  }, 'short paths hide nothing');

  const folded = bc.collapse(segs, 4);
  assert.deepEqual(plain(folded.head), { label: '/', path: '/' });
  assert.deepEqual(plain(folded.tail), [
    { label: 'd', path: '/a/b/c/d' },
    { label: 'e', path: '/a/b/c/d/e' },
    { label: 'f', path: '/a/b/c/d/e/f' },
  ], 'tail keeps the last maxVisible-1 segments');
  assert.deepEqual(plain(folded.hidden), [
    { label: 'a', path: '/a' },
    { label: 'b', path: '/a/b' },
    { label: 'c', path: '/a/b/c' },
  ], 'middle segments fold into hidden');
}

console.log('files breadcrumbs: all assertions passed');

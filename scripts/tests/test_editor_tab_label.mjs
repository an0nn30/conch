// Run: node scripts/tests/test_editor_tab_label.mjs
//
// A remote file's tab must say which host it lives on: the SFTP panel that
// revealed the host closes as soon as the file opens, so the tab is the only
// place left to say where the file actually is.
import assert from 'node:assert';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';

const MODULE_PATH = path.resolve(
  import.meta.dirname,
  '../../crates/termlab_tauri/frontend/app/features/editor/tab-label.js',
);

const sandbox = { console };
sandbox.window = sandbox;
vm.createContext(sandbox);
vm.runInContext(fs.readFileSync(MODULE_PATH, 'utf8'), sandbox, { filename: MODULE_PATH });

const { editorTabLabel } = sandbox.termlabEditorTabLabel;

function checkLabel(actual, expected, description) {
  assert.strictEqual(actual.label, expected.label, `${description}: label`);
  assert.strictEqual(actual.tooltip, expected.tooltip, `${description}: tooltip`);
}

// Local file: label is the basename, tooltip is the full local path.
checkLabel(
  editorTabLabel({ filePath: '/a/b/notes.md', remote: null }),
  { label: 'notes.md', tooltip: '/a/b/notes.md' },
  'local file',
);

// Remote file: the basename comes from the *remote* path, not the local temp
// path the file was downloaded to — the temp filename is an implementation
// detail the user never sees.
checkLabel(
  editorTabLabel({
    filePath: '/tmp/x/nginx.conf',
    remote: { remotePath: '/etc/nginx.conf', hostLabel: 'dustin@web1' },
  }),
  { label: 'nginx.conf — dustin@web1', tooltip: 'dustin@web1:/etc/nginx.conf' },
  'remote file',
);

// Missing or empty filePath falls back to "untitled" rather than throwing or
// producing an empty label.
checkLabel(
  editorTabLabel({ filePath: undefined, remote: null }),
  { label: 'untitled', tooltip: '' },
  'missing filePath',
);
checkLabel(
  editorTabLabel({ filePath: '', remote: null }),
  { label: 'untitled', tooltip: '' },
  'empty filePath',
);

// No pane at all (e.g. called defensively) must not throw.
checkLabel(
  editorTabLabel(null),
  { label: 'untitled', tooltip: '' },
  'null pane',
);

console.log('editor tab label: all assertions passed');

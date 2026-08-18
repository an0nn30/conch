// Run: node scripts/tests/test_titlebar_accelerators.mjs
//
// The custom titlebar's File menu is not just a display of shortcuts. On
// Windows and Linux the native menu is hidden, so registerAccelerators binds
// every non-`noAccel` entry in the menu definition through the keyboard router
// at priority 115 — above the configurable shortcut table shortcut-runtime.js
// registers at 75/80. A hardcoded string in that table is therefore a LIVE key
// binding that WINS, and a user who rebinds the action in config.toml is left
// with a default they cannot free.
//
// That is what `new-file` was: a literal `${ctrl}+N`, while `new_file` has been
// a configurable key since the untitled-files work. The checks below pin the
// read-from-config behaviour for it and for `open-file` beside it, and pin the
// fallback for a backend too old to send the field.
import assert from 'node:assert';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';

const ROOT = path.resolve(import.meta.dirname, '../../crates/termlab_tauri/frontend');

let failures = 0;
let ran = 0;
function check(name, fn) {
  ran++;
  try {
    fn();
  } catch (error) {
    failures++;
    console.error(`  FAIL ${name}\n       ${error.message}`);
  }
}

// titlebar.js reads navigator.platform at load time to pick Cmd vs Ctrl. The
// bug this file guards is a Windows/Linux one, so load it as one of those.
function loadTitlebar(platform) {
  const sandbox = { console, navigator: { platform }, document: { addEventListener() {} } };
  sandbox.window = sandbox;
  vm.createContext(sandbox);
  vm.runInContext(
    fs.readFileSync(path.join(ROOT, 'app/ui/titlebar.js'), 'utf8'),
    sandbox,
    { filename: 'app/ui/titlebar.js' },
  );
  return sandbox.titlebar;
}

// A keydown as the router delivers it. `code` is the physical key, which is
// what matchesEvent prefers.
function keyEvent(spec) {
  return Object.assign({
    metaKey: false, ctrlKey: false, shiftKey: false, altKey: false, key: '', code: '',
  }, spec);
}

const CTRL_N = keyEvent({ ctrlKey: true, key: 'n', code: 'KeyN' });
const CTRL_SHIFT_U = keyEvent({ ctrlKey: true, shiftKey: true, key: 'u', code: 'KeyU' });

function bindingFor(titlebar, shortcuts, id) {
  const def = titlebar._buildMenuDef(shortcuts, false, []);
  return titlebar._acceleratorBindings(def).find((b) => b.id === id) || null;
}

console.log('titlebar accelerators: new-file is bound from config, not hardcoded');

check('New File is registered as a live accelerator at all', () => {
  // If this ever stops being true the rest of the file is vacuous — the whole
  // problem is that this entry IS a binding and not a caption.
  const titlebar = loadTitlebar('Win32');
  assert.ok(bindingFor(titlebar, {}, 'new-file'), 'new-file produces an accelerator binding');
});

check('a rebound new_file is what gets bound — and the default is NOT', () => {
  const titlebar = loadTitlebar('Win32');
  // The rebinding is deliberately not the default: if the field were ignored
  // and this fell back to Ctrl+N, an assertion against the default would still
  // pass by accident.
  const binding = bindingFor(titlebar, { new_file: 'cmd+shift+u' }, 'new-file');
  assert.ok(binding, 'new-file is still bound');
  assert.strictEqual(titlebar._matchesEvent(binding.combo, CTRL_SHIFT_U), true,
    'the rebound combo is the one the titlebar consumes');
  assert.strictEqual(titlebar._matchesEvent(binding.combo, CTRL_N), false,
    'and Ctrl+N is left free for the configurable table at priority 75/80');
});

check('no OTHER entry hard-binds Ctrl+N once new_file is rebound', () => {
  // The point of the fix is that the combo is free, not merely that one entry
  // moved. A second entry still matching it would leave the user exactly where
  // they started.
  const titlebar = loadTitlebar('Win32');
  const def = titlebar._buildMenuDef({ new_file: 'cmd+shift+u' }, false, []);
  // Rebuilt in this realm: the bindings array comes from the vm sandbox, and
  // deepStrictEqual compares prototypes across realms.
  const claimants = [];
  for (const b of titlebar._acceleratorBindings(def)) {
    if (titlebar._matchesEvent(b.combo, CTRL_N)) claimants.push(b.id);
  }
  assert.strictEqual(claimants.length, 0,
    `nothing in the titlebar menu may claim Ctrl+N, but these do: ${claimants.join(', ')}`);
});

check('an absent new_file still falls back to the platform default', () => {
  // get_keyboard_shortcuts is invoked inside a try/catch that leaves `shortcuts`
  // as {} on failure, so the fallback is a real runtime state, not a
  // hypothetical.
  const titlebar = loadTitlebar('Win32');
  const binding = bindingFor(titlebar, {}, 'new-file');
  assert.strictEqual(titlebar._matchesEvent(binding.combo, CTRL_N), true,
    'with no payload the menu still binds Ctrl+N');
});

check('open-file, the entry beside it, reads its config key the same way', () => {
  const titlebar = loadTitlebar('Win32');
  const rebound = bindingFor(titlebar, { open_file: 'cmd+shift+o' }, 'open-file');
  const ctrlO = keyEvent({ ctrlKey: true, key: 'o', code: 'KeyO' });
  const ctrlShiftO = keyEvent({ ctrlKey: true, shiftKey: true, key: 'o', code: 'KeyO' });
  assert.strictEqual(titlebar._matchesEvent(rebound.combo, ctrlShiftO), true);
  assert.strictEqual(titlebar._matchesEvent(rebound.combo, ctrlO), false);
});

check('save-file-as stays display-only — it is editor-scoped elsewhere', () => {
  // The counterpart: `noAccel` keeps it out of the binding list entirely, so
  // Ctrl+Shift+S still reaches the shell in a terminal pane. Pinned because the
  // fix above is "read the config key", and applying that alone to a noAccel
  // entry would be the wrong move.
  const titlebar = loadTitlebar('Win32');
  assert.strictEqual(bindingFor(titlebar, { save_file_as: 'cmd+shift+s' }, 'save-file-as'), null);
});

check('the same holds on macOS, where the combo is Cmd rather than Ctrl', () => {
  const titlebar = loadTitlebar('MacIntel');
  const binding = bindingFor(titlebar, { new_file: 'cmd+shift+u' }, 'new-file');
  const cmdShiftU = keyEvent({ metaKey: true, shiftKey: true, key: 'u', code: 'KeyU' });
  const cmdN = keyEvent({ metaKey: true, key: 'n', code: 'KeyN' });
  assert.strictEqual(titlebar._matchesEvent(binding.combo, cmdShiftU), true);
  assert.strictEqual(titlebar._matchesEvent(binding.combo, cmdN), false);
});

console.log('titlebar accelerators: the payload carries the field');

check('KeyboardShortcuts exposes new_file, and the command fills it from config', () => {
  // The frontend can only read what Rust sends. Both halves are pinned here
  // because either one alone silently reinstates the hardcoded default: an
  // absent field leaves `shortcuts.new_file` undefined and the `||` fallback
  // takes over, with every check above still green.
  const commands = fs.readFileSync(
    path.resolve(import.meta.dirname, '../../crates/termlab_tauri/src/commands.rs'), 'utf8');
  assert.ok(/struct KeyboardShortcuts \{[\s\S]*?\n    new_file: String,/.test(commands),
    'KeyboardShortcuts declares new_file');
  assert.ok(commands.includes('new_file: kb.new_file.clone(),'),
    'get_keyboard_shortcuts fills it from the keyboard config');

  const generated = fs.readFileSync(path.join(ROOT, 'types/KeyboardShortcuts.ts'), 'utf8');
  assert.ok(generated.includes('new_file: string'),
    'and the checked-in ts-rs export is not stale');
});

console.log('titlebar accelerators: new-tab, new-window, close-tab, settings read config too');

// The same bug as new-file, four more times: each of these ids has had a
// configurable KeyboardConfig field all along, but the menu entry carried a
// literal — so the default stayed hard-bound at priority 115 after a rebind.
const REBOUND_KEYS = [
  {
    id: 'new-tab', field: 'new_tab',
    rebound: 'cmd+shift+g', reboundEvent: keyEvent({ ctrlKey: true, shiftKey: true, key: 'g', code: 'KeyG' }),
    defaultEvent: keyEvent({ ctrlKey: true, key: 't', code: 'KeyT' }),
  },
  {
    id: 'new-window', field: 'new_window',
    rebound: 'cmd+alt+n', reboundEvent: keyEvent({ ctrlKey: true, altKey: true, key: 'n', code: 'KeyN' }),
    defaultEvent: keyEvent({ ctrlKey: true, shiftKey: true, key: 'n', code: 'KeyN' }),
  },
  {
    id: 'close-tab', field: 'close_tab',
    rebound: 'cmd+shift+x', reboundEvent: keyEvent({ ctrlKey: true, shiftKey: true, key: 'x', code: 'KeyX' }),
    defaultEvent: keyEvent({ ctrlKey: true, key: 'w', code: 'KeyW' }),
  },
  {
    id: 'settings', field: 'settings',
    rebound: 'cmd+alt+,', reboundEvent: keyEvent({ ctrlKey: true, altKey: true, key: ',', code: 'Comma' }),
    defaultEvent: keyEvent({ ctrlKey: true, key: ',', code: 'Comma' }),
  },
];

for (const { id, field, rebound, reboundEvent, defaultEvent } of REBOUND_KEYS) {
  check(`a rebound ${field} is what gets bound for ${id} — and the default is NOT`, () => {
    const titlebar = loadTitlebar('Win32');
    const binding = bindingFor(titlebar, { [field]: rebound }, id);
    assert.ok(binding, `${id} is still bound`);
    assert.strictEqual(titlebar._matchesEvent(binding.combo, reboundEvent), true,
      'the rebound combo is the one the titlebar consumes');
    assert.strictEqual(titlebar._matchesEvent(binding.combo, defaultEvent), false,
      'and the old default is left free for the configurable table at priority 75/80');
  });

  check(`no OTHER entry hard-binds the old default once ${field} is rebound`, () => {
    const titlebar = loadTitlebar('Win32');
    const def = titlebar._buildMenuDef({ [field]: rebound }, false, []);
    const claimants = [];
    for (const b of titlebar._acceleratorBindings(def)) {
      if (titlebar._matchesEvent(b.combo, defaultEvent)) claimants.push(b.id);
    }
    assert.strictEqual(claimants.length, 0,
      `nothing in the titlebar menu may claim ${id}'s old default, but these do: ${claimants.join(', ')}`);
  });

  check(`an absent ${field} still falls back to the platform default for ${id}`, () => {
    const titlebar = loadTitlebar('Win32');
    const binding = bindingFor(titlebar, {}, id);
    assert.ok(binding, `${id} is bound with an empty payload`);
    assert.strictEqual(titlebar._matchesEvent(binding.combo, defaultEvent), true,
      'with no payload the menu still binds the default combo');
  });
}

console.log('titlebar accelerators: id ↔ KeyboardConfig convention holds for every row');

// Every titlebar row id maps to its KeyboardConfig field by swapping dashes for
// underscores ('new-tab' ↔ 'new_tab'). This check derives the field list from
// the KeyboardConfig struct itself, so the NEXT menu item added with a literal
// while a config field exists fails here without anyone updating a list.
function keyboardConfigFields() {
  const source = fs.readFileSync(path.resolve(
    import.meta.dirname, '../../crates/termlab_core/src/config/termlab.rs'), 'utf8');
  const block = source.match(/pub struct KeyboardConfig \{([\s\S]*?)\n\}/);
  assert.ok(block, 'KeyboardConfig struct found in termlab.rs');
  const fields = [];
  for (const m of block[1].matchAll(/pub (\w+): String,/g)) fields.push(m[1]);
  assert.ok(fields.length >= 20, `parsed a plausible field list, got ${fields.length}`);
  return fields;
}

function flattenRows(menuDef) {
  const rows = [];
  function walk(items) {
    for (const item of items || []) {
      if (item.type === 'separator') continue;
      if (item.submenu) { walk(item.submenu); continue; }
      rows.push(item);
    }
  }
  for (const menu of menuDef) walk(menu.items);
  return rows;
}

check('every titlebar row with a configurable keymap field reads from shortcuts', () => {
  const titlebar = loadTitlebar('Win32');
  const fields = keyboardConfigFields();
  // Sentinel strings, not real combos: rows carry the raw config string, and
  // acceleratorBindings parses the binding from that same string — so string
  // equality pins display AND live binding in one assertion.
  const sentinels = {};
  for (const field of fields) sentinels[field] = `sentinel+${field}`;
  const rows = flattenRows(titlebar._buildMenuDef(sentinels, true, []));
  const offenders = [];
  const matched = [];
  for (const row of rows) {
    if (typeof row.id !== 'string') continue;
    const field = row.id.replace(/-/g, '_');
    if (!fields.includes(field)) continue;
    matched.push(row.id);
    if (row.shortcut !== sentinels[field]) offenders.push(`${row.id} shows "${row.shortcut}"`);
  }
  assert.strictEqual(offenders.length, 0,
    `these rows ignore their config field: ${offenders.join('; ')}`);
  // Guard the convention itself: if row ids drift away from field names the
  // loop above matches nothing and passes vacuously.
  for (const id of ['new-tab', 'new-window', 'close-tab', 'settings', 'new-file', 'open-file']) {
    assert.ok(matched.includes(id), `row ${id} is covered by the convention check`);
  }
});

check('every such field travels in the KeyboardShortcuts payload', () => {
  // The frontend can only read what Rust sends: a field the titlebar consumes
  // but commands.rs never fills leaves `shortcuts.<field>` undefined and the
  // `||` fallback silently reinstates the hardcoded default.
  const titlebar = loadTitlebar('Win32');
  const fields = keyboardConfigFields();
  const rowIds = new Set(flattenRows(titlebar._buildMenuDef({}, true, []))
    .map((r) => r.id).filter((id) => typeof id === 'string'));
  const commands = fs.readFileSync(
    path.resolve(import.meta.dirname, '../../crates/termlab_tauri/src/commands.rs'), 'utf8');
  const generated = fs.readFileSync(path.join(ROOT, 'types/KeyboardShortcuts.ts'), 'utf8');
  const missing = [];
  for (const field of fields) {
    if (!rowIds.has(field.replace(/_/g, '-'))) continue;
    // zen_mode reaches the titlebar via get_app_config's zen_mode_shortcut,
    // merged into shortcutsState in init() — not via KeyboardShortcuts.
    if (field === 'zen_mode') continue;
    if (!new RegExp(`struct KeyboardShortcuts \\{[\\s\\S]*?\\n    ${field}: String,`).test(commands)) {
      missing.push(`${field} not declared in KeyboardShortcuts`);
    }
    if (!commands.includes(`${field}: kb.${field}.clone(),`)) {
      missing.push(`${field} not filled from the keyboard config`);
    }
    if (!generated.includes(`${field}: string`)) {
      missing.push(`${field} missing from the checked-in ts-rs export`);
    }
  }
  assert.strictEqual(missing.length, 0, missing.join('; '));
});

if (failures) {
  console.error(`titlebar accelerators: ${failures} of ${ran} check(s) FAILED`);
  process.exit(1);
}
console.log(`titlebar accelerators: all ${ran} checks passed`);

// Run: node scripts/tests/test_titlebar_uninitialized_refresh.mjs
//
// titlebar.init() only runs where the custom titlebar exists: config-service.js
// sets `_initTitlebarPending` for Windows/Linux and event-wiring-runtime.js
// gates init() on it. But titlebar.refresh() is also called from bridge-runtime,
// tool-window-runtime, and the settings plugins-section — on every platform.
// On macOS that used to register the full accelerator table at router priority
// 115 with a null `menuActionHandler`: matched combos were consumed
// (preventDefault + stopPropagation) and then did nothing. Main windows were
// shielded only because shortcut-runtime's fallbacks sit at priority 120; any
// combo not covered there (Cmd+Shift+P, the zoom keys, Cmd+/) became a dead
// key the moment a plugin panel registered. These checks pin the fix: an
// uninitialized titlebar's refresh() is a complete no-op — no router
// registrations, no IPC — while the initialized (Windows/Linux) path still
// registers exactly as before.
import assert from 'node:assert';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';

const ROOT = path.resolve(import.meta.dirname, '../../crates/termlab_tauri/frontend');

let failures = 0;
let ran = 0;
async function check(name, fn) {
  ran++;
  try {
    await fn();
  } catch (error) {
    failures++;
    console.error(`  FAIL ${name}\n       ${error.message}`);
  }
}

// A DOM element stub with just enough surface for createTitlebar/init:
// innerHTML assignment, child insertion, listener wiring, and querySelector
// lookups that hand back more stubs.
function makeEl() {
  const el = {
    children: [],
    dataset: {},
    style: {},
    classList: { add() {}, remove() {} },
    innerHTML: '',
    textContent: '',
    className: '',
    firstChild: null,
    addEventListener() {},
    appendChild(child) { el.children.push(child); },
    insertBefore(child) { el.children.unshift(child); },
    querySelector() { return makeEl(); },
    querySelectorAll() { return []; },
    remove() {},
  };
  return el;
}

// Load titlebar.js into a fresh realm with a spying keyboard router and Tauri
// bridge, so each check observes exactly what refresh()/init() register.
function loadTitlebar(platform) {
  const registrations = [];
  const invokes = [];
  const sandbox = {
    console,
    navigator: { platform },
    document: {
      createElement: () => makeEl(),
      getElementById: () => makeEl(),
      addEventListener() {},
    },
    __TAURI__: {
      core: {
        invoke: async (cmd) => {
          invokes.push(cmd);
          if (cmd === 'get_keyboard_shortcuts') return {};
          if (cmd === 'get_app_config') return {};
          if (cmd === 'get_plugin_menu_items') return [];
          return null;
        },
      },
      window: {
        getCurrentWindow: () => ({
          minimize() {}, maximize() {}, unmaximize() {}, close() {},
          async isMaximized() { return false; },
        }),
      },
    },
    termlabKeyboardRouter: {
      register(options) {
        registrations.push(options);
        return function unregister() {
          const idx = registrations.indexOf(options);
          if (idx !== -1) registrations.splice(idx, 1);
        };
      },
    },
  };
  sandbox.window = sandbox;
  vm.createContext(sandbox);
  vm.runInContext(
    fs.readFileSync(path.join(ROOT, 'app/ui/titlebar.js'), 'utf8'),
    sandbox,
    { filename: 'app/ui/titlebar.js' },
  );
  return { titlebar: sandbox.titlebar, registrations, invokes };
}

console.log('titlebar refresh: uninitialized titlebar registers nothing');

await check('refresh() before init() registers zero router handlers', async () => {
  // The macOS runtime state: the module is loaded, __TAURI__ and the router
  // exist, but init() never ran because _initTitlebarPending was never set.
  const { titlebar, registrations } = loadTitlebar('MacIntel');
  await titlebar.refresh();
  const names = registrations.map((r) => r.name);
  assert.strictEqual(registrations.length, 0,
    `an uninitialized titlebar must not touch the keyboard router, but registered: ${names.join(', ')}`);
});

await check('refresh() before init() performs no IPC', async () => {
  // "No-op fully": the get_plugin_menu_items round-trip is only useful when
  // there is a menu to rebuild from it.
  const { titlebar, invokes } = loadTitlebar('MacIntel');
  await titlebar.refresh();
  assert.strictEqual(invokes.length, 0,
    `an uninitialized titlebar's refresh must not invoke commands, but called: ${invokes.join(', ')}`);
});

await check('refresh() before init() stays inert on Windows too', async () => {
  // Same gate on the custom-titlebar platforms: what authorizes registration
  // is init() having run, not the platform. A refresh that races ahead of the
  // pending init must not register a handlerless table there either.
  const { titlebar, registrations } = loadTitlebar('Win32');
  await titlebar.refresh();
  assert.strictEqual(registrations.length, 0,
    'a not-yet-initialized titlebar must not register accelerators on Windows either');
});

console.log('titlebar refresh: the initialized path is unchanged');

await check('init() still registers the accelerator table and menu escape', async () => {
  // Positive control: if the new guard were keyed on something always-false,
  // the zero-registration checks above would pass while silently killing the
  // real Windows/Linux accelerators. init() calls refresh() internally.
  const { titlebar, registrations } = loadTitlebar('Win32');
  await titlebar.init(() => {});
  const names = registrations.map((r) => r.name);
  assert.strictEqual(names.filter((n) => n === 'titlebar-accelerators').length, 1,
    `init must register the accelerator table exactly once, got: ${names.join(', ')}`);
  assert.strictEqual(names.filter((n) => n === 'titlebar-menu-escape').length, 1,
    `init must register the menu Escape handler, got: ${names.join(', ')}`);
});

await check('refresh() after init() re-registers instead of leaking', async () => {
  const { titlebar, registrations } = loadTitlebar('Win32');
  await titlebar.init(() => {});
  await titlebar.refresh();
  const names = registrations.map((r) => r.name);
  assert.strictEqual(names.filter((n) => n === 'titlebar-accelerators').length, 1,
    `a post-init refresh must swap the table, not stack a second one: ${names.join(', ')}`);
});

if (failures) {
  console.error(`titlebar uninitialized refresh: ${failures} of ${ran} check(s) FAILED`);
  process.exit(1);
}
console.log(`titlebar uninitialized refresh: all ${ran} checks passed`);

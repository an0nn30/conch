// Run: node scripts/tests/test_files_split.mjs
//
// The SFTP dual-pane splitter stores one orientation-agnostic ratio: panes get
// flex-GROW weights, never pixel sizes, so a 70/30 split survives moving the
// tool window between the bottom zone (side-by-side) and a sidebar (stacked).
// These tests load the real IIFE in a VM and drive attach() with fake DOM
// elements so the drag/reset/persist flow is exercised without a browser.

import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';

const SPLIT_PATH = path.resolve(
  import.meta.dirname,
  '../../crates/termlab_tauri/frontend/app/features/files/split.js',
);

function loadSplit() {
  const sandbox = { console };
  sandbox.window = sandbox;
  vm.createContext(sandbox);
  vm.runInContext(fs.readFileSync(SPLIT_PATH, 'utf8'), sandbox, { filename: SPLIT_PATH });
  assert.ok(sandbox.termlabFilesSplit, 'split IIFE must expose window.termlabFilesSplit');
  return sandbox.termlabFilesSplit;
}

function fakeElement(rect) {
  const listeners = new Map();
  return {
    style: {},
    captured: [],
    released: [],
    getBoundingClientRect: () => rect,
    addEventListener(type, handler) {
      if (!listeners.has(type)) listeners.set(type, new Set());
      listeners.get(type).add(handler);
    },
    removeEventListener(type, handler) {
      const set = listeners.get(type);
      if (set) set.delete(handler);
    },
    setPointerCapture(id) { this.captured.push(id); },
    releasePointerCapture(id) { this.released.push(id); },
    dispatch(type, event) {
      const set = listeners.get(type);
      if (!set || set.size === 0) return false;
      for (const handler of Array.from(set)) handler(event);
      return true;
    },
  };
}

function fakeStorage(initial = {}) {
  const map = new Map(Object.entries(initial));
  return {
    getItem: (key) => (map.has(key) ? map.get(key) : null),
    setItem: (key, value) => map.set(key, String(value)),
    map,
  };
}

const CONTAINER_RECT = { left: 100, top: 50, width: 800, height: 400 };

// clampRatio: bounds enforcement plus a defined fallback for garbage input,
// because the ratio round-trips through localStorage as a string.
{
  const split = loadSplit();
  assert.equal(split.clampRatio(0.5), 0.5);
  assert.equal(split.clampRatio(0.02), split.MIN_RATIO);
  assert.equal(split.clampRatio(0.99), split.MAX_RATIO);
  assert.equal(split.clampRatio(Number.NaN), 0.5);
  assert.equal(split.clampRatio('nonsense'), 0.5);
}

// ratioFromPointer maps the pointer's position across the container to a
// ratio on the axis the current orientation splits.
{
  const split = loadSplit();
  assert.equal(
    split.ratioFromPointer({ orientation: 'row', rect: CONTAINER_RECT, clientX: 300, clientY: 0 }),
    0.25,
    'row orientation must use the horizontal axis',
  );
  assert.equal(
    split.ratioFromPointer({ orientation: 'column', rect: CONTAINER_RECT, clientX: 0, clientY: 150 }),
    0.25,
    'column orientation must use the vertical axis',
  );
  assert.equal(
    split.ratioFromPointer({ orientation: 'row', rect: CONTAINER_RECT, clientX: 100 + 799, clientY: 0 }),
    split.MAX_RATIO,
    'pointer past the clamp boundary must clamp',
  );
}

// applyRatio assigns grow weights, never a fixed basis, to both panes.
{
  const split = loadSplit();
  const first = fakeElement(CONTAINER_RECT);
  const second = fakeElement(CONTAINER_RECT);
  split.applyRatio(first, second, 0.7);
  assert.equal(first.style.flex, '0.7 1 0px');
  assert.equal(second.style.flex, '0.3 1 0px');
}

// The persisted ratio survives a round-trip and garbage in storage falls back
// to an even split.
{
  const split = loadSplit();
  const storage = fakeStorage();
  split.saveRatio(storage, 0.65);
  assert.equal(split.loadRatio(storage), 0.65);
  assert.equal(split.loadRatio(fakeStorage({ [split.STORAGE_KEY]: 'junk' })), 0.5);
  assert.equal(split.loadRatio(fakeStorage()), 0.5);
}

// attach(): dragging the divider re-weights the panes live using the
// container's CURRENT orientation, and pointerup persists the final ratio.
{
  const split = loadSplit();
  const container = fakeElement(CONTAINER_RECT);
  container.currentFlexDirection = 'row';
  const first = fakeElement(CONTAINER_RECT);
  const second = fakeElement(CONTAINER_RECT);
  const divider = fakeElement(CONTAINER_RECT);
  const storage = fakeStorage({ [split.STORAGE_KEY]: '0.6' });

  split.attach({
    container,
    firstEl: first,
    secondEl: second,
    dividerEl: divider,
    storage,
    getOrientation: () => container.currentFlexDirection,
  });

  assert.equal(first.style.flex, '0.6 1 0px', 'attach must apply the persisted ratio');

  divider.dispatch('pointerdown', { pointerId: 7, clientX: 580, clientY: 0, preventDefault() {} });
  assert.deepEqual(divider.captured, [7], 'drag must capture the pointer');

  divider.dispatch('pointermove', { pointerId: 7, clientX: 300, clientY: 0 });
  assert.equal(first.style.flex, '0.25 1 0px', 'dragging must re-weight the first pane');
  assert.equal(second.style.flex, '0.75 1 0px', 'dragging must re-weight the second pane');

  divider.dispatch('pointerup', { pointerId: 7, clientX: 300, clientY: 0 });
  assert.equal(storage.getItem(split.STORAGE_KEY), '0.25', 'pointerup must persist the ratio');

  // After the panel moves to a sidebar the SAME divider drags on the vertical
  // axis — orientation is read per-drag, not captured at attach time.
  container.currentFlexDirection = 'column';
  divider.dispatch('pointerdown', { pointerId: 8, clientX: 0, clientY: 0, preventDefault() {} });
  divider.dispatch('pointermove', { pointerId: 8, clientX: 0, clientY: 50 + 320 });
  assert.equal(first.style.flex, '0.8 1 0px', 'column drags must use the vertical axis');
  divider.dispatch('pointerup', { pointerId: 8, clientX: 0, clientY: 50 + 320 });

  // Double-click resets to an even split and persists it.
  divider.dispatch('dblclick', {});
  assert.equal(first.style.flex, '0.5 1 0px');
  assert.equal(second.style.flex, '0.5 1 0px');
  assert.equal(storage.getItem(split.STORAGE_KEY), '0.5');
}

// Moves without a preceding pointerdown (or for a different pointer) are
// ignored — no re-weighting from stray hover events.
{
  const split = loadSplit();
  const container = fakeElement(CONTAINER_RECT);
  const first = fakeElement(CONTAINER_RECT);
  const second = fakeElement(CONTAINER_RECT);
  const divider = fakeElement(CONTAINER_RECT);

  split.attach({
    container,
    firstEl: first,
    secondEl: second,
    dividerEl: divider,
    storage: fakeStorage(),
    getOrientation: () => 'row',
  });
  const before = first.style.flex;

  divider.dispatch('pointermove', { pointerId: 1, clientX: 200, clientY: 0 });
  assert.equal(first.style.flex, before, 'a move without a drag must not re-weight');

  divider.dispatch('pointerdown', { pointerId: 2, clientX: 400, clientY: 0, preventDefault() {} });
  divider.dispatch('pointermove', { pointerId: 9, clientX: 200, clientY: 0 });
  assert.equal(first.style.flex, before, 'another pointer id must not steer the drag');
}

console.log('files split: all assertions passed');

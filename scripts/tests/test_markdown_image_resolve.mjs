// Run: node scripts/tests/test_markdown_image_resolve.mjs
//
// What this pins is the traffic behaviour, not the pixels: an image is fetched
// ONCE per pane, a debounced re-render costs nothing, and a fetch that lands
// after its render was superseded is discarded rather than written into a
// frame that has moved on. Over SFTP those three properties are the difference
// between a preview and a stalled connection.
import assert from 'node:assert';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';

const ROOT = path.resolve(import.meta.dirname, '../../crates/termlab_tauri/frontend');
const SRC = path.join(ROOT, 'app/features/editor/preview/image-resolver.js');

const sandbox = { window: {} };
sandbox.window = sandbox;
vm.createContext(sandbox);
vm.runInContext(fs.readFileSync(SRC, 'utf8'), sandbox);

function makeResolver() {
  const calls = [];
  const invoke = async (command, args) => {
    calls.push({ command, args });
    if (command === 'editor_read_image_base64') return 'TE9DQUw=';
    if (command === 'sftp_read_file') {
      // One short read: 4 base64 chars, fewer bytes than requested => done.
      return { data: 'UkVNT1RF', bytes_read: 6 };
    }
    throw new Error(`unexpected command ${command}`);
  };
  const resolver = sandbox.termlabPreviewImages.createResolver({
    invoke,
    mimeFor: () => 'image/png',
  });
  return { resolver, calls };
}

// --- local resolution ------------------------------------------------------
{
  const { resolver, calls } = makeResolver();
  const uri = await resolver.resolve('./img/a.png', null, resolver.currentGeneration(), '/home/u/doc.md');
  assert.match(uri, /^data:image\/png;base64,TE9DQUw=$/, 'local image becomes a data URI');
  assert.strictEqual(calls[0].command, 'editor_read_image_base64');
  assert.strictEqual(
    calls[0].args.path, '/home/u/img/a.png',
    'relative paths resolve against the document directory, not the cwd',
  );
}

// --- cache: the second resolve costs no I/O -------------------------------
{
  const { resolver, calls } = makeResolver();
  const gen = resolver.currentGeneration();
  await resolver.resolve('./a.png', null, gen, '/d/doc.md');
  await resolver.resolve('./a.png', null, gen, '/d/doc.md');
  assert.strictEqual(calls.length, 1, 'a cached image must not be fetched twice');
}

// --- cancellation: a stale generation discards its result -----------------
{
  const { resolver } = makeResolver();
  const stale = resolver.currentGeneration();
  resolver.nextGeneration();
  const out = await resolver.resolve('./a.png', null, stale, '/d/doc.md');
  assert.strictEqual(out, null, 'a result from a superseded render must be discarded');
}

// --- remote resolution -----------------------------------------------------
{
  const { resolver, calls } = makeResolver();
  const uri = await resolver.resolve(
    './img/b.png',
    { paneId: 7, remotePath: '/srv/docs/readme.md' },
    resolver.currentGeneration(),
    null,
  );
  assert.match(uri, /^data:image\/png;base64,UkVNT1RF$/, 'remote image becomes a data URI');
  assert.strictEqual(calls[0].command, 'sftp_read_file');
  assert.strictEqual(calls[0].args.paneId, 7, 'must reuse the pane SSH session');
  assert.strictEqual(
    calls[0].args.path, '/srv/docs/img/b.png',
    'remote relative paths resolve against the remote directory',
  );
}

// --- a failed fetch degrades, never throws --------------------------------
{
  const resolver = sandbox.termlabPreviewImages.createResolver({
    invoke: async () => { throw new Error('ENOENT'); },
    mimeFor: () => 'image/png',
  });
  const out = await resolver.resolve('./missing.png', null, resolver.currentGeneration(), '/d/doc.md');
  assert.strictEqual(out, null, 'a failed image resolves to null, it does not reject');
}

// --- absolute and remote-scheme sources -----------------------------------
{
  const { resolver, calls } = makeResolver();
  await resolver.resolve('/abs/x.png', null, resolver.currentGeneration(), '/d/doc.md');
  assert.strictEqual(calls[0].args.path, '/abs/x.png', 'absolute paths pass through unchanged');

  const skipped = await resolver.resolve('https://e.com/x.png', null, resolver.currentGeneration(), '/d/doc.md');
  assert.strictEqual(skipped, null, 'http(s) sources are never fetched');
}

// --- default MIME table: used only when the caller injects nothing --------
{
  const invoke = async (command) => {
    if (command === 'editor_read_image_base64') return 'AAAA';
    throw new Error(`unexpected command ${command}`);
  };
  const resolver = sandbox.termlabPreviewImages.createResolver({ invoke });
  const uri = await resolver.resolve('./pic.svg', null, resolver.currentGeneration(), '/d/doc.md');
  assert.match(
    uri, /^data:image\/svg\+xml;base64,AAAA$/,
    'the built-in MIME table is used when deps.mimeFor is absent, and maps .svg to image/svg+xml',
  );
}

console.log('test_markdown_image_resolve: ok');

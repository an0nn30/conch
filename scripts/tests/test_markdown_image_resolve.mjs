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
// The resolver rejoins multi-chunk remote reads through atob/btoa, which it
// reads off its injected global rather than assuming a browser. Supplying
// Node's is what lets the reassembly be exercised here at all.
sandbox.atob = atob;
sandbox.btoa = btoa;
vm.createContext(sandbox);
vm.runInContext(fs.readFileSync(SRC, 'utf8'), sandbox);

function makeResolver() {
  const calls = [];
  const invoke = async (command, args) => {
    calls.push({ command, args });
    if (command === 'editor_read_image_base64') return 'TE9DQUw=';
    if (command === 'sftp_read_file') {
      // The whole file in the first read, then the zero-byte read that ends
      // it. Zero bytes — not a short read — is the only EOF signal the Rust
      // side gives, since it performs one read() per call.
      if (args && args.offset > 0) return { data: '', bytes_read: 0 };
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

// --- Windows absolute paths: a drive letter is not a URL scheme -----------
{
  const { resolver, calls } = makeResolver();
  await resolver.resolve('C:\\Users\\bob\\img.png', null, resolver.currentGeneration(), '/d/doc.md');
  assert.strictEqual(
    calls[0].command, 'editor_read_image_base64',
    'a Windows drive letter must not be mistaken for an unsupported scheme like http:',
  );
  assert.strictEqual(
    calls[0].args.path, 'C:\\Users\\bob\\img.png',
    'a Windows absolute src is fetched with its path unchanged',
  );
}

// --- Windows relative resolution: joined against a drive-rooted dir -------
{
  const { resolver, calls } = makeResolver();
  await resolver.resolve('./img.png', null, resolver.currentGeneration(), 'C:\\Users\\bob\\doc.md');
  assert.strictEqual(
    calls[0].args.path, 'C:/Users/bob/img.png',
    'a relative src under a Windows docPath must join onto the drive, not get an extra leading slash',
  );
}

// --- remote bindings are always POSIX, regardless of docPath's shape ------
{
  const { resolver, calls } = makeResolver();
  const uri = await resolver.resolve(
    './img/b.png',
    { paneId: 7, remotePath: '/srv/docs/readme.md' },
    resolver.currentGeneration(),
    'C:\\fake\\windows.md',
  );
  assert.match(uri, /^data:image\/png;base64,UkVNT1RF$/, 'remote image still resolves');
  assert.strictEqual(calls[0].command, 'sftp_read_file');
  assert.strictEqual(
    calls[0].args.path, '/srv/docs/img/b.png',
    'a Windows-looking docPath must not leak drive-letter handling into the remote (always-POSIX) branch',
  );
}

// --- negative cache: a missing image is fetched exactly once per pane -----
{
  let attempts = 0;
  const invoke = async () => { attempts += 1; throw new Error('ENOENT'); };
  const resolver = sandbox.termlabPreviewImages.createResolver({ invoke, mimeFor: () => 'image/png' });
  const gen = resolver.currentGeneration();
  const a = await resolver.resolve('./missing.png', null, gen, '/d/doc.md');
  const b = await resolver.resolve('./missing.png', null, gen, '/d/doc.md');
  const c = await resolver.resolve('./missing.png', null, gen, '/d/doc.md');
  assert.strictEqual(a, null);
  assert.strictEqual(b, null);
  assert.strictEqual(c, null);
  assert.strictEqual(
    attempts, 1,
    'a broken reference must cost exactly one fetch attempt per pane, not one per debounce tick',
  );
}

// --- SFTP chunk ceiling: a still-not-exhausted file must not return a
//     silently truncated data URI ------------------------------------------
{
  let calls = 0;
  const invoke = async (command) => {
    if (command !== 'sftp_read_file') throw new Error(`unexpected command ${command}`);
    calls += 1;
    // Every read comes back full-length, as if the file were larger than the
    // ceiling: the loop must give up rather than hand back a partial image.
    return { data: 'AAAA', bytes_read: 1024 * 1024 };
  };
  const resolver = sandbox.termlabPreviewImages.createResolver({ invoke, mimeFor: () => 'image/png' });
  const out = await resolver.resolve(
    './big.png',
    { paneId: 1, remotePath: '/srv/docs/readme.md' },
    resolver.currentGeneration(),
    null,
  );
  assert.strictEqual(out, null, 'hitting the chunk ceiling with more data left must resolve to null, not a truncated URI');
  assert.ok(calls > 1, 'the ceiling must actually be exercised (more than one chunk requested)');
}

// --- the remote backstop is a BYTE budget, not a count of round trips -----
// russh-sftp caps every read at MAX_READ_LENGTH (261120) and the Rust command
// performs exactly one read() per call, so asking for 1MB still returns at most
// 255KiB. A ceiling counted in CALLS therefore cut remote images off at ~1.74MB
// while the comment beside it (and the local path) promised 8MB: anything in
// between resolved to null and rendered as nothing.
{
  const SFTP_READ = 261120;          // russh-sftp MAX_READ_LENGTH
  const whole = Buffer.alloc(3 * 1024 * 1024, 0x07);   // well past the old 8-call cap
  let reads = 0;
  const invoke = async (command, args) => {
    if (command !== 'sftp_read_file') throw new Error(`unexpected command ${command}`);
    reads += 1;
    const slice = whole.subarray(args.offset, args.offset + Math.min(args.length, SFTP_READ));
    return { data: slice.toString('base64'), bytes_read: slice.length };
  };
  const resolver = sandbox.termlabPreviewImages.createResolver({ invoke, mimeFor: () => 'image/png' });
  const uri = await resolver.resolve(
    './big.png',
    { paneId: 5, remotePath: '/srv/docs/readme.md' },
    resolver.currentGeneration(),
    null,
  );
  assert.ok(reads > 8, 'more round trips than the old call ceiling allowed');
  assert.strictEqual(
    uri, `data:image/png;base64,${whole.toString('base64')}`,
    'a 3MB remote image is under the 8MB budget and must resolve in full',
  );
}

// --- and the budget is what stops an oversized one ------------------------
// editor_read_image_base64 refuses anything over MAX_IMAGE_BYTES outright, so
// the remote path gives up at the same size rather than at some accidental
// multiple of the transport's read length.
{
  const SFTP_READ = 261120;
  const whole = Buffer.alloc(9 * 1024 * 1024, 0x08);   // over MAX_IMAGE_BYTES
  let reads = 0;
  const invoke = async (command, args) => {
    if (command !== 'sftp_read_file') throw new Error(`unexpected command ${command}`);
    reads += 1;
    const slice = whole.subarray(args.offset, args.offset + Math.min(args.length, SFTP_READ));
    return { data: slice.toString('base64'), bytes_read: slice.length };
  };
  const resolver = sandbox.termlabPreviewImages.createResolver({ invoke, mimeFor: () => 'image/png' });
  const out = await resolver.resolve(
    './huge.png',
    { paneId: 6, remotePath: '/srv/docs/readme.md' },
    resolver.currentGeneration(),
    null,
  );
  assert.strictEqual(out, null, 'over the byte budget must resolve to null, not a truncated URI');
  assert.ok(
    reads < 64,
    'the BYTE budget stopped it (8MB / 255KiB is ~33 reads), not the runaway call guard',
  );
}

// --- multi-chunk remote reads: the BYTES are rejoined, not the base64 -----
// sftp_read_file base64-encodes EACH chunk independently and the 1MB chunk
// size is not a multiple of 3, so every full chunk's encoding ends in `==`
// padding. Concatenating the encoded text buries that padding mid-payload and
// decodes to garbage — invisibly, and only for images over 1MB.
{
  const CHUNK = 1024 * 1024;
  const whole = Buffer.concat([
    Buffer.alloc(CHUNK, 0x01),   // exactly one full chunk: encoding ends in ==
    Buffer.alloc(1234, 0x02),    // a partial second chunk
  ]);
  let reads = 0;
  const invoke = async (command, args) => {
    if (command !== 'sftp_read_file') throw new Error(`unexpected command ${command}`);
    reads += 1;
    const slice = whole.subarray(args.offset, args.offset + args.length);
    return { data: slice.toString('base64'), bytes_read: slice.length };
  };
  const resolver = sandbox.termlabPreviewImages.createResolver({ invoke, mimeFor: () => 'image/png' });
  const uri = await resolver.resolve(
    './big.png',
    { paneId: 3, remotePath: '/srv/docs/readme.md' },
    resolver.currentGeneration(),
    null,
  );
  assert.ok(reads > 2, 'the multi-chunk path must actually be exercised');
  assert.strictEqual(
    uri, `data:image/png;base64,${whole.toString('base64')}`,
    'a multi-chunk remote image must decode to the original bytes, not to concatenated base64',
  );
}

// --- a throwing mimeFor must not make resolve() reject ---------------------
{
  const invoke = async (command) => {
    if (command === 'editor_read_image_base64') return 'AAAA';
    throw new Error(`unexpected command ${command}`);
  };
  const resolver = sandbox.termlabPreviewImages.createResolver({
    invoke,
    mimeFor: () => { throw new Error('mimeFor blew up'); },
  });
  const out = await resolver.resolve('./ok.png', null, resolver.currentGeneration(), '/d/doc.md');
  assert.strictEqual(out, null, 'a throwing mimeFor must degrade to null, never reject resolve()');
}

// --- a malformed sftp_read_file response must not make resolve() reject ---
{
  const invoke = async (command) => {
    if (command !== 'sftp_read_file') throw new Error(`unexpected command ${command}`);
    return { bytes_read: 4 }; // no `data` field
  };
  const resolver = sandbox.termlabPreviewImages.createResolver({ invoke, mimeFor: () => 'image/png' });
  const out = await resolver.resolve(
    './x.png',
    { paneId: 1, remotePath: '/srv/docs/readme.md' },
    resolver.currentGeneration(),
    null,
  );
  assert.strictEqual(out, null, 'a malformed sftp_read_file response must resolve to null, not reject');
}

console.log('test_markdown_image_resolve: ok');

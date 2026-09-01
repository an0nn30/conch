// Image src -> data: URI, for the markdown preview.
//
// Two mechanisms carry this module, and both exist because the preview
// re-renders on a 150ms debounce while you type:
//
//   cache      — an image is fetched once per pane. After the first render,
//                further renders cost no I/O at all. This includes failures:
//                a broken reference is cached too (see the WHY-comment at
//                the bottom of `resolve`), so it costs one attempt, not one
//                per debounce tick.
//   generation — every render bumps a counter. A fetch that returns against a
//                superseded generation drops its result instead of writing
//                into a frame that has moved on. Without it, an edit burst
//                queues a round-trip per keystroke per image, which over SFTP
//                is a stalled connection rather than a slow one.
//
// http(s) sources never reach here — the sanitizer drops them so no request is
// ever issued — but the guard is repeated below because this module must be
// safe to call directly.
(function initTermLabPreviewImages(global) {
  'use strict';

  // A URL scheme needs at least two characters before its colon (`http:`,
  // `data:`, `ftp:`). A single letter followed by `:` (`C:`) is a Windows
  // drive, not a scheme, so the `+` below (one-or-more, not zero-or-more) is
  // load-bearing: it is what keeps a Windows absolute path from being
  // mistaken for an unsupported remote scheme and silently dropped.
  const REMOTE_SCHEME = /^[a-z][a-z0-9+.-]+:/i;
  // `C:\` or `C:/` — a Windows drive root. Only meaningful for LOCAL paths;
  // remote paths live on the SSH host and are always POSIX, so callers pass
  // `posixOnly: true` to keep this out of that branch entirely.
  const WINDOWS_DRIVE = /^[a-z]:[\\/]/i;
  const SFTP_CHUNK = 1024 * 1024; // sftp_read_file caps each call at 1MB
  const MAX_REMOTE_CHUNKS = 8;    // ceiling of 8MB, matching MAX_IMAGE_BYTES

  // Mirrors the extension guard in editor_read_image_base64 (Rust). MIME
  // belongs on this side because this module is the one building the
  // `data:` URI; the Rust command only ever hands back a bare base64 payload.
  const DEFAULT_MIME_TABLE = {
    png: 'image/png',
    jpg: 'image/jpeg',
    jpeg: 'image/jpeg',
    gif: 'image/gif',
    webp: 'image/webp',
    svg: 'image/svg+xml',
    bmp: 'image/bmp',
    ico: 'image/x-icon',
  };

  function defaultMimeFor(filePath) {
    const path = typeof filePath === 'string' ? filePath : '';
    const dot = path.lastIndexOf('.');
    const ext = dot < 0 ? '' : path.slice(dot + 1).toLowerCase();
    return DEFAULT_MIME_TABLE[ext] || 'application/octet-stream';
  }

  // `posixOnly` forces pure `/`-separator handling for remote paths, which
  // never take drive letters or backslash separators regardless of what
  // platform TermLab itself is running on.
  function dirnameOf(filePath, posixOnly) {
    if (typeof filePath !== 'string' || !filePath) return '';
    const at = posixOnly
      ? filePath.lastIndexOf('/')
      : Math.max(filePath.lastIndexOf('/'), filePath.lastIndexOf('\\'));
    return at <= 0 ? '/' : filePath.slice(0, at);
  }

  // A path is absolute if it starts with `/` (POSIX), or — for local paths
  // only — if it is drive-rooted (`C:\`, `C:/`).
  function isAbsolutePath(p, posixOnly) {
    if (typeof p !== 'string' || !p) return false;
    if (p.startsWith('/')) return true;
    return !posixOnly && WINDOWS_DRIVE.test(p);
  }

  // Join and flatten `.`/`..` without a URL constructor, which would need a
  // base origin these paths do not have.
  function joinPath(dir, rel, posixOnly) {
    // Absolute paths — POSIX ones and, for local paths, Windows drive-rooted
    // ones — are intentionally NOT confined to the document's directory.
    // Supporting an absolute local image path was an explicit product
    // decision, not an oversight. `..` traversal reaches the exact same
    // place a literal absolute path would (e.g. `../../../etc/passwd.png`
    // from a doc under `/home/u/docs/` resolves to `/etc/passwd.png`), so
    // this note covers traversal too, not just literal absolute syntax. The
    // exposure this leaves is a hostile .md displaying a local image the
    // user did not mean to open; it stays bounded because the sanitizer
    // already strips http(s) image sources, so nothing resolved here can be
    // used to beacon data out.
    if (isAbsolutePath(rel, posixOnly)) return rel;

    const splitter = posixOnly ? /\/+/ : /[\\/]+/;
    const parts = `${dir}/${rel}`.split(splitter);
    const out = [];
    for (const part of parts) {
      if (part === '.' || part === '') continue;
      if (part === '..') { out.pop(); continue; }
      out.push(part);
    }
    // A Windows drive letter ("C:") ends up as the first segment when `dir`
    // was itself drive-rooted. Rebuilding it as `/C:/...` would turn a valid
    // Windows path into a broken POSIX-looking one, so keep the drive form
    // instead of always prepending a leading slash. This branch is skipped
    // for remote paths (posixOnly), which never have drive letters.
    if (!posixOnly && out.length && /^[a-z]:$/i.test(out[0])) {
      return out.join('/');
    }
    return `/${out.join('/')}`;
  }

  // sftp_read_file base64-encodes EACH CHUNK INDEPENDENTLY, and
  // SFTP_CHUNK % 3 === 1, so a full chunk's encoding always ends in `==`
  // padding. Concatenating the encoded TEXT therefore buries padding in the
  // middle of the payload and decodes to garbage — silently, and only for
  // images over 1MB, since anything smaller arrives in a single chunk. The
  // bytes have to be rejoined and encoded once, which is what these two do.
  //
  // atob/btoa are read off the module's injected global so this stays testable
  // outside a browser; without them a remote image degrades to no image, which
  // is the same outcome as any other failed fetch here.
  function decodeBase64(text) {
    if (typeof global.atob !== 'function') return null;
    try {
      return global.atob(text);
    } catch (err) {
      return null;
    }
  }

  function encodeBase64(binary) {
    if (typeof global.btoa !== 'function') return null;
    try {
      return global.btoa(binary);
    } catch (err) {
      return null;
    }
  }

  function createResolver(deps) {
    const options = deps || {};
    const invoke = options.invoke;
    const mimeFor = typeof options.mimeFor === 'function'
      ? options.mimeFor
      : defaultMimeFor;
    if (typeof invoke !== 'function') return null;

    const cache = new Map();
    let generation = 0;

    async function fetchLocal(absPath) {
      const b64 = await invoke('editor_read_image_base64', { path: absPath });
      return typeof b64 === 'string' ? b64 : null;
    }

    async function fetchRemote(paneId, absPath) {
      let binary = '';
      let offset = 0;
      for (let i = 0; i < MAX_REMOTE_CHUNKS; i += 1) {
        const res = await invoke('sftp_read_file', {
          paneId, path: absPath, offset, length: SFTP_CHUNK,
        });
        if (!res || typeof res.data !== 'string') break;
        const chunk = decodeBase64(res.data);
        if (chunk === null) return null;
        binary += chunk;
        const read = Number(res.bytes_read) || 0;
        // Zero bytes is the ONLY end-of-file signal the Rust side gives: it
        // performs one `read()` per call, and a short read is permitted at any
        // point in a stream. Treating a short read as EOF truncated images
        // whose transfer merely came back in smaller pieces.
        if (read <= 0) return binary ? encodeBase64(binary) : null;
        offset += read;
      }
      // Either the ceiling was hit before EOF (the file is not exhausted —
      // MAX_REMOTE_CHUNKS is a real ceiling, not just a loop bound) or a chunk
      // came back malformed. Handing back what was read so far in either case
      // would silently produce a truncated/corrupt
      // image; editor_read_image_base64 (the local counterpart) refuses
      // oversized files outright instead of reading a partial slice, so
      // mirror that here rather than returning bad output.
      return null;
    }

    // `docPath` is the local document path; `binding` is the remote one.
    async function resolve(src, binding, forGeneration, docPath) {
      if (typeof src !== 'string' || !src) return null;
      // Already inline, or a scheme this preview refuses to fetch.
      if (src.startsWith('data:') || REMOTE_SCHEME.test(src)) return null;

      const remote = binding && binding.remotePath ? binding : null;
      // Remote paths live on the SSH host and are always POSIX, independent
      // of the platform TermLab itself runs on — this flag is threaded
      // through dirnameOf/joinPath so drive-letter handling can never leak
      // into the remote branch.
      const posixOnly = !!remote;
      const baseDir = remote
        ? dirnameOf(remote.remotePath, true)
        : dirnameOf(docPath);
      const absPath = joinPath(baseDir, src, posixOnly);
      const key = `${remote ? remote.paneId : 'local'}:${absPath}`;

      if (cache.has(key)) return cache.get(key);

      let uri = null;
      try {
        const payload = remote
          ? await fetchRemote(remote.paneId, absPath)
          : await fetchLocal(absPath);
        // mimeFor is caller-supplied (deps.mimeFor) and must not be allowed
        // to make resolve() reject — a throwing mimeFor is just another
        // failure mode, same as a missing file, so it stays inside this
        // try block rather than running after it.
        uri = payload ? `data:${mimeFor(absPath)};base64,${payload}` : null;
      } catch (err) {
        // A missing or unreadable image (or a throwing mimeFor) is a
        // placeholder, never a toast and never a rejected render.
        uri = null;
      }

      // The render that asked for this has been superseded; drop the result
      // rather than caching work for a document state that no longer exists.
      if (forGeneration !== generation) return null;

      // Cache the outcome, including a failure (uri === null): a broken
      // reference should cost exactly one fetch attempt per pane, not one
      // per 150ms debounce tick — that repeat cost over SFTP is the exact
      // stalled connection the cache exists to prevent. Accepted trade-off:
      // an image that appears on disk mid-session is not picked up again
      // until the pane closes or the src/docPath changes — judged the
      // lesser harm.
      cache.set(key, uri);
      return uri;
    }

    return {
      resolve,
      currentGeneration: () => generation,
      nextGeneration: () => { generation += 1; return generation; },
      clear: () => cache.clear(),
    };
  }

  global.termlabPreviewImages = { createResolver };
})(window);

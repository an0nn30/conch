// Image src -> data: URI, for the markdown preview.
//
// Two mechanisms carry this module, and both exist because the preview
// re-renders on a 150ms debounce while you type:
//
//   cache      — an image is fetched once per pane. After the first render,
//                further renders cost no I/O at all.
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

  const REMOTE_SCHEME = /^[a-z][a-z0-9+.-]*:/i;
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

  function dirnameOf(filePath) {
    if (typeof filePath !== 'string' || !filePath) return '';
    const at = Math.max(filePath.lastIndexOf('/'), filePath.lastIndexOf('\\'));
    return at <= 0 ? '/' : filePath.slice(0, at);
  }

  // Join and flatten `.`/`..` without a URL constructor, which would need a
  // base origin these paths do not have.
  function joinPath(dir, rel) {
    // Absolute paths are intentionally NOT confined to the document's
    // directory — supporting an absolute local image path was an explicit
    // product decision, not an oversight. The exposure this leaves is a
    // hostile .md displaying a local image the user did not mean to open;
    // it stays bounded because the sanitizer already strips http(s) image
    // sources, so nothing resolved here can be used to beacon data out.
    if (rel.startsWith('/')) return rel;
    const parts = `${dir}/${rel}`.split(/[\\/]+/);
    const out = [];
    for (const part of parts) {
      if (part === '.' || part === '') continue;
      if (part === '..') { out.pop(); continue; }
      out.push(part);
    }
    return `/${out.join('/')}`;
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
      let out = '';
      let offset = 0;
      for (let i = 0; i < MAX_REMOTE_CHUNKS; i += 1) {
        const res = await invoke('sftp_read_file', {
          paneId, path: absPath, offset, length: SFTP_CHUNK,
        });
        if (!res || typeof res.data !== 'string') break;
        out += res.data;
        const read = Number(res.bytes_read) || 0;
        offset += read;
        // A short read means end of file. Anything else would loop forever on
        // a zero-byte response.
        if (read < SFTP_CHUNK) break;
      }
      return out || null;
    }

    // `docPath` is the local document path; `binding` is the remote one.
    async function resolve(src, binding, forGeneration, docPath) {
      if (typeof src !== 'string' || !src) return null;
      // Already inline, or a scheme this preview refuses to fetch.
      if (src.startsWith('data:') || REMOTE_SCHEME.test(src)) return null;

      const remote = binding && binding.remotePath ? binding : null;
      const baseDir = remote ? dirnameOf(remote.remotePath) : dirnameOf(docPath);
      const absPath = joinPath(baseDir, src);
      const key = `${remote ? remote.paneId : 'local'}:${absPath}`;

      if (cache.has(key)) return cache.get(key);

      let payload = null;
      try {
        payload = remote
          ? await fetchRemote(remote.paneId, absPath)
          : await fetchLocal(absPath);
      } catch (err) {
        // A missing or unreadable image is a placeholder, never a toast and
        // never a rejected render.
        payload = null;
      }

      // The render that asked for this has been superseded; drop the result
      // rather than caching work for a document state that no longer exists.
      if (forGeneration !== generation) return null;
      if (!payload) return null;

      const uri = `data:${mimeFor(absPath)};base64,${payload}`;
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

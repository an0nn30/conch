(function (global) {
  'use strict';
  // Icons that ship a *_dark.svg variant. Kept in JS (not fs-probed) because
  // the webview cannot stat files; regenerate with:
  //   ls crates/termlab_tauri/frontend/vendor/intellij-icons | grep _dark
  let darkVariants = new Set([
    'add', 'edit', 'remove', 'refresh', 'web', 'settings', 'gear',
    'hideToolWindow', 'notifications', 'moreVertical', 'sftp',
    'newFolder', 'copy', 'back', 'forward', 'home', 'toggleVisibility',
    'resume', 'suspend', 'gc',
  ]);

  function resolve(name, isDark) {
    if (isDark && darkVariants.has(name)) {
      return `vendor/intellij-icons/${name}_dark.svg`;
    }
    return `vendor/intellij-icons/${name}.svg`;
  }

  function isDarkAppearance() {
    // No document (string-building in a headless harness): the app's default
    // appearance is dark, so resolve the dark variant.
    if (typeof document === 'undefined' || !document.documentElement) return true;
    return document.documentElement.getAttribute('data-tl-appearance') !== 'light';
  }

  // The icon's logical name, stamped on the element so the variant can be
  // re-resolved later. `src` alone is not enough to recover it: `add.svg` and
  // `add_dark.svg` are two spellings of one name, and reversing the suffix
  // rule is guesswork the moment an icon is legitimately named `*_dark`.
  const NAME_ATTR = 'data-tl-icon';

  // String twin of create() for innerHTML templates (the files-panel toolbar
  // renders through one template string, not element-by-element). Carries the
  // same NAME_ATTR stamp so refreshAll() re-resolves it on appearance flips.
  // `name` is a known logical icon name, never user input.
  function html(name, opts) {
    const size = (opts && opts.size) || 16;
    const alt = (opts && opts.alt) || '';
    return `<img class="tl-icon" draggable="false" width="${size}" height="${size}" alt="${alt}" ${NAME_ATTR}="${name}" src="${resolve(name, isDarkAppearance())}">`;
  }

  function create(name, opts) {
    const img = document.createElement('img');
    img.className = 'tl-icon';
    img.draggable = false;
    img.width = (opts && opts.size) || 16;
    img.height = (opts && opts.size) || 16;
    img.alt = (opts && opts.alt) || '';
    img.setAttribute(NAME_ATTR, name);
    img.src = resolve(name, isDarkAppearance());
    return img;
  }

  // Re-resolve every icon under `root` against the CURRENT appearance.
  //
  // create() bakes the variant into `src`, so a live dark<->light flip would
  // otherwise leave every already-rendered icon on the stale variant — most
  // visibly the tool-window rail, which is built once at startup and never
  // rebuilt, so its `_dark` glyphs (#AFB1B3, a light grey) would sit
  // near-invisible on a light surface. Menus, the palette and the settings
  // sidebar rebuild per-open and self-heal; the rail, the tab bar and the
  // open panels do not.
  //
  // ONE delegated pass over the stamped elements, rather than a subscription
  // per icon: icons are created and discarded constantly (every menu open,
  // every ssh-tree render), and a per-icon listener would leak one document
  // registration for every icon the app has ever built.
  function refreshAll(root) {
    const scope = root || (typeof document !== 'undefined' ? document : null);
    if (!scope || typeof scope.querySelectorAll !== 'function') return 0;
    const isDark = isDarkAppearance();
    const nodes = scope.querySelectorAll(`img[${NAME_ATTR}]`);
    let refreshed = 0;
    for (const img of nodes) {
      const name = img.getAttribute(NAME_ATTR);
      if (!name) continue;
      const next = resolve(name, isDark);
      // Compared through getAttribute, not the `src` property: the property
      // reflects an absolute URL and would never equal the relative path,
      // making every pass a redundant write (and a fresh image request).
      if (img.getAttribute('src') !== next) img.setAttribute('src', next);
      refreshed += 1;
    }
    return refreshed;
  }

  // The single document-level subscription that drives refreshAll. Registered
  // here rather than from each window's boot script so all three entrypoints
  // (index/settings/chooser) get it just by loading this file. Guarded so a
  // test harness or a non-DOM host can load the module without a document.
  if (global.document && typeof global.document.addEventListener === 'function') {
    global.document.addEventListener('tl-appearance-changed', () => {
      refreshAll(global.document);
    });
  }

  global.tlIcon = {
    create,
    html,
    resolve,
    refreshAll,
    _setDarkVariants: (set) => { darkVariants = set; },
  };
})(window);

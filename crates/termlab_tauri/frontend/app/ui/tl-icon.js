(function (global) {
  'use strict';
  // Icons that ship a *_dark.svg variant. Kept in JS (not fs-probed) because
  // the webview cannot stat files; regenerate with:
  //   ls crates/termlab_tauri/frontend/vendor/intellij-icons | grep _dark
  let darkVariants = new Set([
    'add', 'edit', 'remove', 'refresh', 'web', 'settings', 'hideToolWindow',
    'notifications', 'moreVertical',
  ]);

  function resolve(name, isDark) {
    if (isDark && darkVariants.has(name)) {
      return `vendor/intellij-icons/${name}_dark.svg`;
    }
    return `vendor/intellij-icons/${name}.svg`;
  }

  function isDarkAppearance() {
    return document.documentElement.getAttribute('data-tl-appearance') !== 'light';
  }

  function create(name, opts) {
    const img = document.createElement('img');
    img.className = 'tl-icon';
    img.draggable = false;
    img.width = (opts && opts.size) || 16;
    img.height = (opts && opts.size) || 16;
    img.alt = (opts && opts.alt) || '';
    img.src = resolve(name, isDarkAppearance());
    return img;
  }

  global.tlIcon = {
    create,
    resolve,
    _setDarkVariants: (set) => { darkVariants = set; },
  };
})(window);

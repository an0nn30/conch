/**
 * Icon markup for plugin widgets.
 *
 * Icon names arrive from plugins and are interpolated into an `img` src, so
 * they are validated rather than escaped: a name that is not a bare filename
 * cannot address the bundled icon set and is far more likely to be an attempt
 * to break out of the attribute.
 */
(function () {
  'use strict';

  /** Friendly icon names mapped to bundled filenames (dark theme variants). */
  const ICON_FILES = {
    'file': 'file-dark', 'folder': 'folder', 'folder-open': 'folder-open',
    'server': 'server', 'network-server': 'network-server', 'terminal': 'terminal',
    'go-home': 'go-home-dark', 'go-next': 'go-next-dark', 'go-previous': 'go-previous-dark',
    'refresh': 'view-refresh-dark', 'folder-new': 'folder-new-dark',
    'transfer-up': 'transfer-up-dark', 'transfer-down': 'transfer-down-dark',
    'tab-close': 'tab-close-dark', 'computer': 'computer-dark',
    'locked': 'locked-dark', 'unlocked': 'unlocked-dark', 'eye': 'eye-dark',
  };

  /** A bare filename: no path separators, no quotes, no angle brackets. */
  const SAFE_FILENAME = /^[A-Za-z0-9._-]+$/;

  /**
   * Build an `img` tag for a named icon.
   *
   * @param {string} name Icon name, possibly supplied by a plugin.
   * @param {number} [size] Pixel size; defaults to 14.
   * @returns {string} HTML, or an empty string if the name is not usable.
   */
  function iconHtml(name, size) {
    if (!name) return '';
    const px = Number(size) || 14;
    const file = ICON_FILES[name] || String(name);
    if (!SAFE_FILENAME.test(file)) return '';
    return `<img src="icons/${file}.png" width="${px}" height="${px}" style="vertical-align:middle;margin-right:3px">`;
  }

  window.widgetIcons = { iconHtml };
})();

// Global toast notification system for Conch Mobile.

(function (exports) {
  'use strict';

  const esc = () => window.utils.esc;

  let toastContainer = null;

  function ensureContainer() {
    if (toastContainer) return;
    toastContainer = document.createElement('div');
    toastContainer.id = 'toast-container';
    document.body.appendChild(toastContainer);
  }

  /**
   * Show a toast notification.
   * @param {Object} opts
   * @param {string} opts.title    — Bold header text
   * @param {string} [opts.body]   — Optional detail text below the title
   * @param {'info'|'success'|'error'|'warn'} [opts.level='info']
   * @param {number} [opts.duration=4000] — Auto-dismiss ms (0 = sticky)
   * @returns {HTMLElement} the toast element
   */
  function show(opts) {
    ensureContainer();

    const level    = opts.level || 'info';
    const duration = opts.duration != null ? opts.duration : 4000;
    const escFn    = window.utils.esc;

    const iconMap = {
      success: '<svg width="14" height="14" viewBox="0 0 16 16" fill="currentColor"><path d="m 7.877 1.002 c -1.345 0.027 -2.695 0.446 -3.883 1.277 -3.167 2.217 -3.932 6.56 -1.715 9.727 2.217 3.167 6.558 3.932 9.725 1.715 2.277 -1.595 3.312 -4.282 2.891 -6.854 -0.098 -0.601 -0.285 -1.16 -0.527 -1.701 l -6.352 7.258 -0.688 0.813 -0.781 -0.75 -3 -3 c -0.376 -0.376 -0.376 -1.061 0 -1.438 0.376 -0.376 1.061 -0.376 1.438 0 l 2.25 2.25 6.023 -6.893 c -1.077 -1.241 -2.509 -2.03 -4.043 -2.301 -0.441 -0.078 -0.889 -0.112 -1.338 -0.104 z"/></svg>',
      error:   '<svg width="14" height="14" viewBox="0 0 16 16" fill="currentColor"><path d="m 8 1 c -3.866 0 -7 3.134 -7 7 0 3.866 3.134 7 7 7 3.866 0 7 -3.134 7 -7 0 -3.866 -3.134 -7 -7 -7 z m -1 2 h 2 V 9 h -2 z m 0 8 h 2 v 2 h -2 z"/></svg>',
      warn:    '<svg width="14" height="14" viewBox="0 0 16 16" fill="currentColor"><path d="m 7.955 0.667 c -0.343 0.015 -0.654 0.206 -0.824 0.504 l -7 12.338 c -0.379 0.667 0.103 1.494 0.869 1.494 h 14 c 0.767 0 1.248 -0.828 0.869 -1.494 l -7 -12.338 c -0.186 -0.326 -0.539 -0.521 -0.914 -0.504 z M 7 4 h 2 v 6 H 7 z m 0 7 h 2 v 2 H 7 z"/></svg>',
      info:    '<svg width="14" height="14" viewBox="0 0 16 16" fill="currentColor"><path d="m 8 1 c 0.552 0 1 0.448 1 1 0 0.033 -0.002 0.067 -0.006 0.1 2.29 0.459 4.006 2.471 4.006 4.9 l 0 2 v 2.5 l 2 2 v 0.5 H 13 3 1 v -0.5 l 2 -2 0 -4.5 c 0 -2.429 1.716 -4.442 4.006 -4.9 C 7.002 2.067 7 2.033 7 2 c 0 -0.552 0.448 -1 1 -1 z M 9.729 15 c -0.357 0.618 -1.015 0.999 -1.729 1 -0.714 -0.001 -1.373 -0.382 -1.73 -1 z"/></svg>',
    };

    const icon = iconMap[level] || iconMap.info;

    const toast = document.createElement('div');
    toast.className = 'conch-toast conch-toast-' + level;
    toast.innerHTML = `
      <div class="conch-toast-header">
        <span class="conch-toast-icon">${icon}</span>
        <span class="conch-toast-title">${escFn(opts.title)}</span>
        <button class="conch-toast-close">\u2715</button>
      </div>
      ${opts.body ? `<div class="conch-toast-body">${escFn(opts.body)}</div>` : ''}
    `;

    toast.querySelector('.conch-toast-close').addEventListener('click', () => dismiss(toast));
    toastContainer.appendChild(toast);
    requestAnimationFrame(() => toast.classList.add('visible'));

    if (duration > 0) {
      toast._timeout = setTimeout(() => dismiss(toast), duration);
    }

    return toast;
  }

  function dismiss(toast) {
    if (!toast || !toast.parentNode) return;
    clearTimeout(toast._timeout);
    toast.classList.remove('visible');
    setTimeout(() => toast.remove(), 300);
  }

  function info(title, body)    { return show({ title, body, level: 'info' }); }
  function success(title, body) { return show({ title, body, level: 'success' }); }
  function error(title, body)   { return show({ title, body, level: 'error', duration: 6000 }); }
  function warn(title, body)    { return show({ title, body, level: 'warn',  duration: 5000 }); }

  exports.toast = { show, dismiss, info, success, error, warn };
})(window);

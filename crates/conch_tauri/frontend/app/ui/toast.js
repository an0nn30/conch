// Global toast notification system.
// Supports configurable position (top/bottom) and native OS notifications
// when the app is out of focus.

(function (exports) {
  'use strict';

  let toastContainer = null;
  let position = 'bottom'; // 'top' or 'bottom'
  let nativeNotificationsEnabled = true;
  const history = [];
  let notificationListeners = [];

  function ensureContainer() {
    if (toastContainer) return;
    toastContainer = document.createElement('div');
    toastContainer.id = 'toast-container';
    applyPosition();
    document.body.appendChild(toastContainer);
  }

  function applyPosition() {
    if (!toastContainer) return;
    if (position === 'top') {
      toastContainer.style.bottom = '';
      toastContainer.style.top = '16px';
      toastContainer.style.flexDirection = 'column';
    } else {
      toastContainer.style.top = '';
      toastContainer.style.bottom = '16px';
      toastContainer.style.flexDirection = 'column-reverse';
    }
  }

  /**
   * Configure toast behavior.
   * @param {Object} opts
   * @param {'top'|'bottom'} [opts.position] — Where toasts appear
   * @param {boolean} [opts.nativeNotifications] — Use native OS notifications when unfocused
   */
  function configure(opts) {
    if (opts.position && (opts.position === 'top' || opts.position === 'bottom')) {
      position = opts.position;
      applyPosition();
    }
    if (typeof opts.nativeNotifications === 'boolean') {
      nativeNotificationsEnabled = opts.nativeNotifications;
    }
  }

  /**
   * Show a toast notification.
   * If native notifications are enabled and the window is not focused,
   * sends a native OS notification instead of an in-app toast.
   * @param {Object} opts
   * @param {string} opts.title    — Bold header text
   * @param {string} [opts.body]   — Optional detail text below the title
   * @param {'info'|'success'|'error'|'warn'} [opts.level='info']
   * @param {number} [opts.duration=4000] — Auto-dismiss ms (0 = sticky)
   * @param {Object} [opts.action] — Action button {label, callback}
   * @param {boolean} [opts.forceInApp=false] — Always show in-app (skip native)
   * @returns {HTMLElement|null} the toast element (null if sent as native notification)
   */
  function show(opts) {
    const record = {
      timestamp: new Date(),
      level: opts.level || 'info',
      title: opts.title || '',
      body: opts.body || '',
    };
    history.unshift(record);
    for (const cb of notificationListeners) {
      try { cb(record); } catch (_) {}
    }

    // Try native notification if window is not focused
    if (!opts.forceInApp && nativeNotificationsEnabled && !document.hasFocus()) {
      if (sendNativeNotification(opts.title, opts.body)) {
        return null;
      }
    }

    return showInApp(opts);
  }

  function showInApp(opts) {
    ensureContainer();

    const level = opts.level || 'info';
    const duration = opts.duration != null ? opts.duration : 4000;

    const toast = document.createElement('div');
    toast.className = 'conch-toast conch-toast-' + level;

    const icon = level === 'success' ? '<svg width="14" height="14" viewBox="0 0 16 16" fill="currentColor"><path d="m 7.877 1.002 c -1.345 0.027 -2.695 0.446 -3.883 1.277 -3.167 2.217 -3.932 6.56 -1.715 9.727 2.217 3.167 6.558 3.932 9.725 1.715 2.277 -1.595 3.312 -4.282 2.891 -6.854 -0.098 -0.601 -0.285 -1.16 -0.527 -1.701 l -6.352 7.258 -0.688 0.813 -0.781 -0.75 -3 -3 c -0.376 -0.376 -0.376 -1.061 0 -1.438 0.376 -0.376 1.061 -0.376 1.438 0 l 2.25 2.25 6.023 -6.893 c -1.077 -1.241 -2.509 -2.03 -4.043 -2.301 -0.441 -0.078 -0.889 -0.112 -1.338 -0.104 z"/></svg>'
      : level === 'error' ? '<svg width="14" height="14" viewBox="0 0 16 16" fill="currentColor"><path d="m 8 1 c -3.866 0 -7 3.134 -7 7 0 3.866 3.134 7 7 7 3.866 0 7 -3.134 7 -7 0 -3.866 -3.134 -7 -7 -7 z m -1 2 h 2 V 9 h -2 z m 0 8 h 2 v 2 h -2 z"/></svg>'
      : level === 'warn' ? '<svg width="14" height="14" viewBox="0 0 16 16" fill="currentColor"><path d="m 7.955 0.667 c -0.343 0.015 -0.654 0.206 -0.824 0.504 l -7 12.338 c -0.379 0.667 0.103 1.494 0.869 1.494 h 14 c 0.767 0 1.248 -0.828 0.869 -1.494 l -7 -12.338 c -0.186 -0.326 -0.539 -0.521 -0.914 -0.504 z M 7 4 h 2 v 6 H 7 z m 0 7 h 2 v 2 H 7 z"/></svg>'
      : '<svg width="14" height="14" viewBox="0 0 16 16" fill="currentColor"><path d="m 8 1 c 0.552 0 1 0.448 1 1 0 0.033 -0.002 0.067 -0.006 0.1 2.29 0.459 4.006 2.471 4.006 4.9 l 0 2 v 2.5 l 2 2 v 0.5 H 13 3 1 v -0.5 l 2 -2 0 -4.5 c 0 -2.429 1.716 -4.442 4.006 -4.9 C 7.002 2.067 7 2.033 7 2 c 0 -0.552 0.448 -1 1 -1 z M 9.729 15 c -0.357 0.618 -1.015 0.999 -1.729 1 -0.714 -0.001 -1.373 -0.382 -1.73 -1 z"/></svg>';

    let actionHtml = '';
    if (opts.action && opts.action.label) {
      actionHtml = `<div class="conch-toast-actions"><button class="conch-toast-action-btn">${esc(opts.action.label)}</button></div>`;
    }

    toast.innerHTML = `
      <div class="conch-toast-header">
        <span class="conch-toast-icon">${icon}</span>
        <span class="conch-toast-title">${esc(opts.title)}</span>
        <button class="conch-toast-close">\u2715</button>
      </div>
      ${opts.body ? `<div class="conch-toast-body">${esc(opts.body)}</div>` : ''}
      ${actionHtml}
    `;

    toast.querySelector('.conch-toast-close').addEventListener('click', () => dismiss(toast));

    if (opts.action && opts.action.callback) {
      const actionBtn = toast.querySelector('.conch-toast-action-btn');
      if (actionBtn) {
        actionBtn.addEventListener('click', () => {
          dismiss(toast);
          opts.action.callback();
        });
      }
    }

    toastContainer.appendChild(toast);
    requestAnimationFrame(() => toast.classList.add('visible'));

    if (duration > 0) {
      toast._timeout = setTimeout(() => dismiss(toast), duration);
    }

    return toast;
  }

  function sendNativeNotification(title, body) {
    try {
      const tauri = window.__TAURI__;
      if (tauri && tauri.notification) {
        tauri.notification.sendNotification({
          title: title || 'Conch',
          body: body || '',
        });
        return true;
      }
    } catch (_) {
      // Fall through to in-app toast
    }
    return false;
  }

  function dismiss(toast) {
    if (!toast || !toast.parentNode) return;
    clearTimeout(toast._timeout);
    toast.classList.remove('visible');
    setTimeout(() => toast.remove(), 300);
  }

  // Convenience methods
  function info(title, body, extra) { return show(Object.assign({ title, body, level: 'info' }, extra || {})); }
  function success(title, body) { return show({ title, body, level: 'success' }); }
  function error(title, body) { return show({ title, body, level: 'error', duration: 6000 }); }
  function warn(title, body) { return show({ title, body, level: 'warn', duration: 5000 }); }

  const esc = window.utils.esc;

  function getHistory() { return history; }
  function onNotification(cb) { notificationListeners.push(cb); }
  function clearHistory() {
    history.length = 0;
    for (const cb of notificationListeners) {
      try { cb(null); } catch (_) {}
    }
  }

  exports.toast = { show, showInApp, dismiss, configure, info, success, error, warn, getHistory, onNotification, clearHistory };
})(window);

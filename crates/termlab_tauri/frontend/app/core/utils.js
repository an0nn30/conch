// Shared utility functions used across all frontend modules.

(function (exports) {
  'use strict';

  /** HTML-escape a string for safe insertion into innerHTML. */
  function esc(str) {
    const el = document.createElement('span');
    el.textContent = str;
    return el.innerHTML;
  }

  /** Escape a string for use in an HTML attribute value. */
  function attr(str) {
    return String(str || '').replace(/"/g, '&quot;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
  }

  /** Format a byte count as a human-readable string. */
  function formatSize(bytes) {
    if (bytes == null) return '';
    if (bytes < 1024) return bytes + ' B';
    if (bytes < 1024 * 1024) return (bytes / 1024).toFixed(1) + ' KB';
    if (bytes < 1024 * 1024 * 1024) return (bytes / (1024 * 1024)).toFixed(1) + ' MB';
    return (bytes / (1024 * 1024 * 1024)).toFixed(2) + ' GB';
  }

  /** Format a Unix epoch timestamp as YYYY-MM-DD HH:MM. */
  function formatDate(epoch) {
    if (!epoch) return '';
    const d = new Date(epoch * 1000);
    const pad = (n) => String(n).padStart(2, '0');
    return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}`;
  }

  exports.utils = { esc, attr, formatSize, formatDate };
})(window);

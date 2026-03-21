// Shared utility functions for Conch Mobile.

(function (exports) {
  'use strict';

  /** HTML-escape a string for safe insertion into innerHTML. */
  function esc(str) {
    const el = document.createElement('span');
    el.textContent = str == null ? '' : String(str);
    return el.innerHTML;
  }

  /** Escape a string for use in an HTML attribute value. */
  function attr(str) {
    return String(str == null ? '' : str)
      .replace(/"/g, '&quot;')
      .replace(/</g, '&lt;')
      .replace(/>/g, '&gt;');
  }

  /** Format a byte count as a human-readable string. */
  function formatSize(bytes) {
    if (bytes == null) return '';
    if (bytes < 1024) return bytes + ' B';
    if (bytes < 1024 * 1024) return (bytes / 1024).toFixed(1) + ' KB';
    if (bytes < 1024 * 1024 * 1024) return (bytes / (1024 * 1024)).toFixed(1) + ' MB';
    return (bytes / (1024 * 1024 * 1024)).toFixed(2) + ' GB';
  }

  /**
   * Format a Unix epoch (seconds) or Date as a relative time string.
   * Examples: "just now", "5m ago", "2h ago", "3d ago"
   */
  function formatRelativeTime(epochOrDate) {
    if (!epochOrDate) return '';
    const ts = epochOrDate instanceof Date ? epochOrDate.getTime() : epochOrDate * 1000;
    const diff = Math.floor((Date.now() - ts) / 1000); // seconds
    if (diff < 60)  return 'just now';
    if (diff < 3600)  return Math.floor(diff / 60) + 'm ago';
    if (diff < 86400) return Math.floor(diff / 3600) + 'h ago';
    return Math.floor(diff / 86400) + 'd ago';
  }

  exports.utils = { esc, attr, formatSize, formatRelativeTime };
})(window);

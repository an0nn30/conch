(function initConchSettingsFeatureSearch(global) {
  'use strict';

  function normalizeSearchText(value) {
    return String(value || '').trim().toLowerCase();
  }

  function tokenizeSearchText(value) {
    return normalizeSearchText(value).split(/[\s:_-]+/).filter(Boolean);
  }

  function levenshteinDistance(a, b) {
    const left = String(a || '');
    const right = String(b || '');
    if (!left) return right.length;
    if (!right) return left.length;
    const prev = new Array(right.length + 1);
    const curr = new Array(right.length + 1);
    for (let j = 0; j <= right.length; j++) prev[j] = j;
    for (let i = 1; i <= left.length; i++) {
      curr[0] = i;
      for (let j = 1; j <= right.length; j++) {
        const cost = left.charCodeAt(i - 1) === right.charCodeAt(j - 1) ? 0 : 1;
        curr[j] = Math.min(
          prev[j] + 1,
          curr[j - 1] + 1,
          prev[j - 1] + cost
        );
      }
      for (let j = 0; j <= right.length; j++) prev[j] = curr[j];
    }
    return prev[right.length];
  }

  function getFuzzyMatchScore(query, haystack, extraTokens) {
    const q = normalizeSearchText(query);
    const text = normalizeSearchText(haystack);
    if (!q || !text) return Number.POSITIVE_INFINITY;
    if (text.includes(q)) return 0;

    const tokens = new Set([
      ...tokenizeSearchText(text),
      ...(Array.isArray(extraTokens) ? extraTokens.flatMap((item) => tokenizeSearchText(item)) : []),
    ]);
    if (tokens.size === 0) return Number.POSITIVE_INFINITY;

    let best = Number.POSITIVE_INFINITY;
    for (const token of tokens) {
      if (!token) continue;
      if (token.startsWith(q) || q.startsWith(token)) {
        best = Math.min(best, 1);
        continue;
      }
      if (token.includes(q) || q.includes(token)) {
        best = Math.min(best, 1);
        continue;
      }
      if (q.length >= 4 && token.length >= 4) {
        const distance = levenshteinDistance(q, token);
        if (distance <= 2) {
          best = Math.min(best, 2 + distance);
        }
      }
    }
    return best;
  }

  function isPrintableKeyEvent(event) {
    return !!(
      event &&
      !event.metaKey &&
      !event.ctrlKey &&
      !event.altKey &&
      typeof event.key === 'string' &&
      event.key.length === 1
    );
  }

  function isTextLikeElement(el) {
    if (!el) return false;
    const tag = String(el.tagName || '').toLowerCase();
    return tag === 'input' || tag === 'textarea' || tag === 'select' || el.isContentEditable;
  }

  function escapeRegExp(value) {
    return String(value || '').replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  }

  function appendHighlightedText(container, text, query) {
    const raw = String(text || '');
    const q = normalizeSearchText(query);
    if (!q) {
      container.textContent = raw;
      return;
    }
    const re = new RegExp('(' + escapeRegExp(q) + ')', 'ig');
    let lastIndex = 0;
    for (const match of raw.matchAll(re)) {
      const idx = match.index == null ? -1 : match.index;
      if (idx < 0) continue;
      if (idx > lastIndex) {
        container.appendChild(document.createTextNode(raw.slice(lastIndex, idx)));
      }
      const mark = document.createElement('mark');
      mark.className = 'settings-search-highlight';
      mark.textContent = raw.slice(idx, idx + match[0].length);
      container.appendChild(mark);
      lastIndex = idx + match[0].length;
    }
    if (lastIndex < raw.length) {
      container.appendChild(document.createTextNode(raw.slice(lastIndex)));
    }
  }

  global.conchSettingsFeatureSearch = {
    normalizeSearchText,
    tokenizeSearchText,
    levenshteinDistance,
    getFuzzyMatchScore,
    isPrintableKeyEvent,
    isTextLikeElement,
    escapeRegExp,
    appendHighlightedText,
  };
})(window);

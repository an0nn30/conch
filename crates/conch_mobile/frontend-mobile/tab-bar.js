// Tab bar bridge — listens for native iOS tab-changed events.
//
// The actual tab bar is a native UITabBar (see ios_native.rs).
// This module handles the content switching when the native bar emits events.
// Falls back to an HTML tab bar when running in a regular browser (dev/debug).

(function (exports) {
  'use strict';

  let contentEl = null;
  let activeTab = 'connections';

  // Registry for tab render functions: { tabId: () => HTMLElement }
  const renderers = {};

  /** Register a render function for a tab. */
  function register(tabId, renderFn) {
    renderers[tabId] = renderFn;
  }

  /** Switch to a tab by id. */
  function switchTo(tabId) {
    if (!renderers[tabId]) return;
    activeTab = tabId;

    // Render content
    if (contentEl) {
      contentEl.innerHTML = '';
      const page = renderers[tabId]();
      contentEl.appendChild(page);
    }

    // Update title
    const titles = { vault: 'Vault', connections: 'Connections', profile: 'Profile' };
    const titleEl = document.getElementById('screen-title');
    if (titleEl) titleEl.textContent = titles[tabId] || tabId;
  }

  /**
   * Initialize: listen for native tab events, or fall back to HTML tab bar.
   */
  function init() {
    contentEl = document.getElementById('tab-content');
    const tabBarEl = document.getElementById('tab-bar');

    // Listen for native tab changes from UITabBar (via Tauri events)
    if (window.__TAURI__) {
      window.__TAURI__.event.listen('tab-changed', (event) => {
        switchTo(event.payload);
      });

      // Listen for tab bar height so we can add bottom padding
      window.__TAURI__.event.listen('native-tab-bar-ready', (event) => {
        const height = event.payload;
        // Add padding so content scrolls above the floating native tab bar
        if (contentEl) {
          contentEl.style.paddingBottom = (height + 34) + 'px'; // tab height + safe area
        }
      });

      // Hide the HTML tab bar — native one is handling it
      if (tabBarEl) tabBarEl.style.display = 'none';
    } else {
      // Fallback: render HTML tab bar for browser debugging
      renderHtmlTabBar(tabBarEl);
    }

    // Show initial tab
    switchTo(activeTab);
  }

  /** Fallback HTML tab bar for browser-based development. */
  function renderHtmlTabBar(el) {
    if (!el) return;

    const ICONS = {
      vault: `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round">
        <rect x="3" y="11" width="18" height="11" rx="2" ry="2"/>
        <path d="M7 11V7a5 5 0 0 1 10 0v4"/>
        <circle cx="12" cy="16" r="1" fill="currentColor" stroke="none"/>
      </svg>`,
      connections: `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round">
        <rect x="2" y="3" width="20" height="14" rx="2"/>
        <path d="M8 21h8M12 17v4"/>
        <path d="M6 8 l2 2 -2 2" stroke-width="1.5"/>
        <line x1="11" y1="12" x2="15" y2="12" stroke-width="1.5"/>
      </svg>`,
      profile: `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round">
        <circle cx="12" cy="8" r="4"/>
        <path d="M4 20c0-4 3.6-7 8-7s8 3 8 7"/>
      </svg>`,
    };

    const LABELS = { vault: 'Vault', connections: 'Connect', profile: 'Profile' };

    el.innerHTML = '';
    ['vault', 'connections', 'profile'].forEach(tabId => {
      const btn = document.createElement('button');
      btn.className = 'tab-item' + (tabId === activeTab ? ' active' : '');
      btn.dataset.tab = tabId;
      btn.innerHTML = `${ICONS[tabId]}<span class="tab-label">${LABELS[tabId]}</span>`;
      btn.addEventListener('click', () => {
        switchTo(tabId);
        el.querySelectorAll('.tab-item').forEach(b =>
          b.classList.toggle('active', b.dataset.tab === tabId)
        );
      });
      el.appendChild(btn);
    });
  }

  /** Return the currently active tab id. */
  function getActive() {
    return activeTab;
  }

  exports.tabBar = { init, register, switchTo, getActive };
})(window);

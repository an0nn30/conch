// Tab bar component for Conch Mobile.
// Manages 3 tabs: vault, connections, profile.
// Default active tab: connections.

(function (exports) {
  'use strict';

  // SVG icon paths for each tab
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

  const LABELS = {
    vault:       'Vault',
    connections: 'Connect',
    profile:     'Profile',
  };

  let activeTab = 'connections';
  let tabBarEl  = null;
  let contentEl = null;

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

    // Update tab button states
    if (tabBarEl) {
      tabBarEl.querySelectorAll('.tab-item').forEach(btn => {
        btn.classList.toggle('active', btn.dataset.tab === tabId);
      });
    }

    // Render content
    if (contentEl) {
      contentEl.innerHTML = '';
      const page = renderers[tabId]();
      contentEl.appendChild(page);
    }
  }

  /**
   * Render and mount the tab bar into #tab-bar,
   * and wire up the #tab-content container.
   */
  function init() {
    tabBarEl  = document.getElementById('tab-bar');
    contentEl = document.getElementById('tab-content');

    if (!tabBarEl || !contentEl) {
      console.error('[tab-bar] Missing #tab-bar or #tab-content elements');
      return;
    }

    tabBarEl.innerHTML = '';

    ['vault', 'connections', 'profile'].forEach(tabId => {
      const btn = document.createElement('button');
      btn.className = 'tab-item' + (tabId === activeTab ? ' active' : '');
      btn.dataset.tab = tabId;
      btn.setAttribute('aria-label', LABELS[tabId]);
      btn.innerHTML = `
        ${ICONS[tabId]}
        <span class="tab-label">${LABELS[tabId]}</span>
      `;
      btn.addEventListener('click', () => switchTo(tabId));
      tabBarEl.appendChild(btn);
    });

    // Show initial tab
    switchTo(activeTab);
  }

  /** Return the currently active tab id. */
  function getActive() {
    return activeTab;
  }

  exports.tabBar = { init, register, switchTo, getActive };
})(window);

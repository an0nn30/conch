(function initTermLabTabSwitcherView(global) {
  'use strict';

  const PREVIEW_REFRESH_MS = 500;
  const KIND_LABELS = { local: 'Terminal', ssh: 'SSH', editor: 'Editor' };

  // Overlay for the ctrl+tab switcher: MRU tab list on the left, a scaled
  // DOM-clone preview of the selected tab on the right. Deliberately not a
  // tl-dialog — the switcher must never steal focus (keys keep flowing to
  // the capture-phase keyboard router while ctrl is held), and it lives
  // only as long as the modifier does.
  function create(deps) {
    const getTabContainerEl = deps.getTabContainerEl;
    const getStageSize = deps.getStageSize;
    const onPick = deps.onPick;

    let overlayEl = null;
    let listEl = null;
    let previewEl = null;
    let refreshTimer = null;
    let currentItems = [];
    let currentIndex = 0;

    function renderList() {
      listEl.textContent = '';
      currentItems.forEach((item, idx) => {
        const row = document.createElement('div');
        row.className = 'tl-tabswitcher__item' + (idx === currentIndex ? ' is-active' : '');
        const badge = document.createElement('div');
        badge.className = 'tl-tabswitcher__badge';
        badge.textContent = idx < 5 ? String(idx + 1) : '';
        const main = document.createElement('div');
        main.className = 'tl-tabswitcher__main';
        const title = document.createElement('div');
        title.className = 'tl-tabswitcher__title';
        title.textContent = item.label || 'Tab';
        const subtitle = document.createElement('div');
        subtitle.className = 'tl-tabswitcher__subtitle';
        subtitle.textContent = KIND_LABELS[item.kind] || 'Tab';
        main.appendChild(title);
        main.appendChild(subtitle);
        row.appendChild(badge);
        row.appendChild(main);
        row.addEventListener('mousedown', (event) => {
          event.preventDefault();
          if (typeof onPick === 'function') onPick(idx);
        });
        listEl.appendChild(row);
      });
    }

    function renderPreview() {
      const item = currentItems[currentIndex];
      previewEl.textContent = '';
      if (!item) return;
      const source = getTabContainerEl(item.id);
      if (!source) return;

      const stage = typeof getStageSize === 'function' ? getStageSize() : null;
      const srcW = stage && stage.width ? stage.width : 800;
      const srcH = stage && stage.height ? stage.height : 500;
      const box = previewEl.getBoundingClientRect();
      const scale = Math.min(
        box.width > 0 ? box.width / srcW : 0.3,
        box.height > 0 ? box.height / srcH : 0.3,
      ) || 0.3;

      const clone = source.cloneNode(true);
      clone.classList.add('tl-tabswitcher__clone');
      clone.classList.remove('active');
      const holder = document.createElement('div');
      holder.className = 'tl-tabswitcher__stage';
      holder.style.width = `${srcW * scale}px`;
      holder.style.height = `${srcH * scale}px`;
      const inner = document.createElement('div');
      inner.className = 'tl-tabswitcher__stage-inner';
      inner.style.width = `${srcW}px`;
      inner.style.height = `${srcH}px`;
      inner.style.transform = `scale(${scale})`;
      inner.appendChild(clone);
      holder.appendChild(inner);
      previewEl.appendChild(holder);
    }

    function open(items, selectedIndex) {
      close();
      currentItems = items;
      currentIndex = selectedIndex;

      overlayEl = document.createElement('div');
      overlayEl.className = 'tl-tabswitcher-overlay';
      const panel = document.createElement('div');
      panel.className = 'tl-tabswitcher';
      listEl = document.createElement('div');
      listEl.className = 'tl-tabswitcher__list tl-scroll';
      previewEl = document.createElement('div');
      previewEl.className = 'tl-tabswitcher__preview';
      // Match the preview box to the terminal host's shape so the scaled
      // clone fills it edge to edge; the panel has no fixed height and
      // wraps whatever this resolves to.
      const stage = typeof getStageSize === 'function' ? getStageSize() : null;
      if (stage && stage.width > 0 && stage.height > 0) {
        previewEl.style.aspectRatio = `${stage.width} / ${stage.height}`;
      }
      panel.appendChild(listEl);
      panel.appendChild(previewEl);
      overlayEl.appendChild(panel);
      document.body.appendChild(overlayEl);

      renderList();
      renderPreview();
      // A running command keeps ticking in the source DOM; re-cloning on a
      // slow beat keeps the preview honest without per-byte work.
      refreshTimer = setInterval(renderPreview, PREVIEW_REFRESH_MS);
    }

    function update(items, selectedIndex) {
      if (!overlayEl) return;
      currentItems = items;
      currentIndex = selectedIndex;
      renderList();
      renderPreview();
    }

    function close() {
      if (refreshTimer) {
        clearInterval(refreshTimer);
        refreshTimer = null;
      }
      if (overlayEl && overlayEl.parentNode) overlayEl.parentNode.removeChild(overlayEl);
      overlayEl = null;
      listEl = null;
      previewEl = null;
      currentItems = [];
    }

    return { open, update, close };
  }

  global.termlabTabSwitcherView = {
    create,
  };
})(window);

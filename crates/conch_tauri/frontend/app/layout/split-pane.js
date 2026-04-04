(function () {
  'use strict';

  /**
   * Render a split tree into DOM elements.
   * Leaf nodes return the pane's existing .terminal-pane element.
   * Split nodes create a new .split-container with a .split-divider between children.
   */
  function renderTree(tree, getPaneEl) {
    if (tree.type === 'leaf') {
      var el = getPaneEl(tree.paneId);
      el.style.flex = '1';
      return el;
    }

    const container = document.createElement('div');
    container.className = 'split-container';
    container.style.flexDirection = tree.direction === 'vertical' ? 'row' : 'column';

    const child0El = renderTree(tree.children[0], getPaneEl);
    const child1El = renderTree(tree.children[1], getPaneEl);

    child0El.style.flex = String(tree.ratio);
    child1El.style.flex = String(1 - tree.ratio);

    const divider = document.createElement('div');
    divider.className = 'split-divider ' + (tree.direction === 'vertical' ? 'vertical' : 'horizontal');
    divider.dataset.direction = tree.direction;

    container.appendChild(child0El);
    container.appendChild(divider);
    container.appendChild(child1El);

    return container;
  }

  /**
   * Set up a per-pane ResizeObserver with independent debounce.
   */
  function createPaneResizeObserver(pane, resizeFn) {
    const observer = new ResizeObserver(() => {
      clearTimeout(pane.debounceTimer);
      pane.debounceTimer = setTimeout(() => resizeFn(pane), 100);
    });
    observer.observe(pane.root);
    return observer;
  }

  /**
   * Set up divider drag via event delegation on the container.
   * Call this ONCE per tab during creation — handles all current and future dividers.
   */
  function setupDividerDrag(containerEl, getTreeRoot, setTreeRoot, onLayoutUpdated) {
    containerEl.addEventListener('pointerdown', (e) => {
      if (!e.target.classList.contains('split-divider')) return;
      e.preventDefault();

      const divider = e.target;
      const parent = divider.parentElement;
      const direction = divider.dataset.direction;
      const child0 = divider.previousElementSibling;
      const child1 = divider.nextElementSibling;

      const parentRect = parent.getBoundingClientRect();
      const isVertical = direction === 'vertical';
      const totalSize = isVertical ? parentRect.width : parentRect.height;
      const minPx = 80;

      const child0PaneId = getImmediatePaneId(child0);
      const child1PaneId = getImmediatePaneId(child1);

      divider.setPointerCapture(e.pointerId);

      function onMove(ev) {
        const pos = isVertical
          ? ev.clientX - parentRect.left
          : ev.clientY - parentRect.top;
        let ratio = pos / totalSize;
        const minRatio = minPx / totalSize;
        const maxRatio = 1 - minRatio;
        ratio = Math.max(minRatio, Math.min(maxRatio, ratio));

        child0.style.flex = String(ratio);
        child1.style.flex = String(1 - ratio);

        const tree = getTreeRoot();
        const updated = updateRatioByChildren(tree, child0PaneId, child1PaneId, ratio);
        setTreeRoot(updated);
        if (typeof onLayoutUpdated === 'function') onLayoutUpdated();
      }

      function onUp() {
        divider.removeEventListener('pointermove', onMove);
        divider.removeEventListener('pointerup', onUp);
        if (typeof onLayoutUpdated === 'function') onLayoutUpdated();
      }

      divider.addEventListener('pointermove', onMove);
      divider.addEventListener('pointerup', onUp);
    });
  }

  function getImmediatePaneId(el) {
    if (el.dataset && el.dataset.paneId) return parseInt(el.dataset.paneId, 10);
    const first = el.querySelector('[data-pane-id]');
    return first ? parseInt(first.dataset.paneId, 10) : null;
  }

  function updateRatioByChildren(tree, child0PaneId, child1PaneId, newRatio) {
    if (tree.type === 'leaf') return tree;
    const left = window.splitTree.allLeaves(tree.children[0]);
    const right = window.splitTree.allLeaves(tree.children[1]);
    if (left.includes(child0PaneId) && right.includes(child1PaneId)) {
      return window.splitTree.makeSplit(tree.direction, newRatio, tree.children);
    }
    return window.splitTree.makeSplit(tree.direction, tree.ratio, [
      updateRatioByChildren(tree.children[0], child0PaneId, child1PaneId, newRatio),
      updateRatioByChildren(tree.children[1], child0PaneId, child1PaneId, newRatio),
    ]);
  }

  /**
   * Find the spatially adjacent pane in a given direction.
   */
  function findAdjacentPane(currentPaneId, direction, containerEl) {
    const currentEl = containerEl.querySelector(`[data-pane-id="${currentPaneId}"]`);
    if (!currentEl) return null;

    const currentRect = currentEl.getBoundingClientRect();
    const cx = currentRect.left + currentRect.width / 2;
    const cy = currentRect.top + currentRect.height / 2;

    const allPaneEls = containerEl.querySelectorAll('[data-pane-id]');
    let bestId = null;
    let bestDist = Infinity;

    for (const el of allPaneEls) {
      const id = parseInt(el.dataset.paneId, 10);
      if (id === currentPaneId) continue;

      const r = el.getBoundingClientRect();
      const ex = r.left + r.width / 2;
      const ey = r.top + r.height / 2;

      let valid = false;
      if (direction === 'left' && ex < cx) valid = true;
      if (direction === 'right' && ex > cx) valid = true;
      if (direction === 'up' && ey < cy) valid = true;
      if (direction === 'down' && ey > cy) valid = true;

      if (!valid) continue;

      const dist = Math.hypot(ex - cx, ey - cy);
      if (dist < bestDist) {
        bestDist = dist;
        bestId = id;
      }
    }

    return bestId;
  }

  window.splitPane = {
    renderTree,
    createPaneResizeObserver,
    setupDividerDrag,
    findAdjacentPane,
  };
})();

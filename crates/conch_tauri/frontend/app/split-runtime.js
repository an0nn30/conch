(function initConchSplitRuntime(global) {
  function paneRatioInTree(tab, paneId) {
    if (!tab || !tab.treeRoot || !global.splitTree) return null;
    const parent = global.splitTree.findParent(tab.treeRoot, paneId);
    if (!parent || !parent.parent || parent.parent.type !== 'split') return null;
    const raw = Number(parent.parent.ratio);
    if (!Number.isFinite(raw)) return null;
    const ratio = parent.index === 0 ? raw : (1 - raw);
    if (!Number.isFinite(ratio)) return null;
    return Math.max(0.1, Math.min(0.9, ratio));
  }

  function insertAroundLeaf(tree, targetPaneId, newPaneId, direction, placeBefore) {
    if (!tree || !global.splitTree) return tree;
    if (tree.type === 'leaf') {
      if (tree.paneId !== targetPaneId) return tree;
      const first = placeBefore
        ? global.splitTree.makeLeaf(newPaneId)
        : global.splitTree.makeLeaf(targetPaneId);
      const second = placeBefore
        ? global.splitTree.makeLeaf(targetPaneId)
        : global.splitTree.makeLeaf(newPaneId);
      return global.splitTree.makeSplit(direction, 0.5, [first, second]);
    }

    return global.splitTree.makeSplit(tree.direction, tree.ratio, [
      insertAroundLeaf(tree.children[0], targetPaneId, newPaneId, direction, placeBefore),
      insertAroundLeaf(tree.children[1], targetPaneId, newPaneId, direction, placeBefore),
    ]);
  }

  global.conchSplitRuntime = {
    paneRatioInTree,
    insertAroundLeaf,
  };
})(window);

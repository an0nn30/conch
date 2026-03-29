(function () {
  'use strict';

  function makeLeaf(paneId) {
    return { type: 'leaf', paneId };
  }

  function makeSplit(direction, ratio, children) {
    return { type: 'split', direction, ratio, children };
  }

  function splitLeaf(tree, targetPaneId, newPaneId, direction) {
    if (tree.type === 'leaf') {
      if (tree.paneId === targetPaneId) {
        return makeSplit(direction, 0.5, [
          makeLeaf(targetPaneId),
          makeLeaf(newPaneId),
        ]);
      }
      return tree;
    }
    return makeSplit(tree.direction, tree.ratio, [
      splitLeaf(tree.children[0], targetPaneId, newPaneId, direction),
      splitLeaf(tree.children[1], targetPaneId, newPaneId, direction),
    ]);
  }

  function removeLeaf(tree, paneId) {
    if (tree.type === 'leaf') {
      return tree.paneId === paneId ? null : tree;
    }
    const left = removeLeaf(tree.children[0], paneId);
    const right = removeLeaf(tree.children[1], paneId);
    if (left === null) return right;
    if (right === null) return left;
    return makeSplit(tree.direction, tree.ratio, [left, right]);
  }

  function allLeaves(tree) {
    if (tree.type === 'leaf') return [tree.paneId];
    return [...allLeaves(tree.children[0]), ...allLeaves(tree.children[1])];
  }

  function firstLeaf(tree) {
    if (tree.type === 'leaf') return tree.paneId;
    return firstLeaf(tree.children[0]);
  }

  function leafCount(tree) {
    if (tree.type === 'leaf') return 1;
    return leafCount(tree.children[0]) + leafCount(tree.children[1]);
  }

  function findParent(tree, paneId) {
    if (tree.type === 'leaf') return null;
    for (let i = 0; i < 2; i++) {
      const child = tree.children[i];
      if (child.type === 'leaf' && child.paneId === paneId) {
        return { parent: tree, index: i };
      }
      const found = findParent(child, paneId);
      if (found) return found;
    }
    return null;
  }

  function updateRatio(tree, paneId, newRatio) {
    if (tree.type === 'leaf') return tree;
    for (let i = 0; i < 2; i++) {
      if (tree.children[i].type === 'leaf' && tree.children[i].paneId === paneId) {
        return makeSplit(tree.direction, newRatio, tree.children);
      }
    }
    return makeSplit(tree.direction, tree.ratio, [
      updateRatio(tree.children[0], paneId, newRatio),
      updateRatio(tree.children[1], paneId, newRatio),
    ]);
  }

  window.splitTree = {
    makeLeaf,
    makeSplit,
    splitLeaf,
    removeLeaf,
    allLeaves,
    firstLeaf,
    leafCount,
    findParent,
    updateRatio,
  };
})();

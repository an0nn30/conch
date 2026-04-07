(function initConchFilesPaneStore(global) {
  'use strict';

  function createPaneState(prefix, isLocal) {
    return {
      prefix,
      isLocal,
      followCwd: true,
      currentPath: '',
      pathInput: '',
      backStack: [],
      forwardStack: [],
      entries: [],
      sortColumn: 'name',
      sortAscending: true,
      showHidden: false,
      colExt: false,
      colSize: true,
      colModified: false,
      error: null,
      loading: false,
      // Transfer state per entry: { [name]: { status, percent } }
      transferStatus: {},
    };
  }

  function getFollowPathFromSettings(settings) {
    return settings
      && settings.conch
      && settings.conch.files
      && typeof settings.conch.files.follow_path === 'boolean'
      ? settings.conch.files.follow_path
      : true;
  }

  function applyFollowPathSetting(localPane, remotePane, enabled) {
    const follow = enabled !== false;
    if (localPane) localPane.followCwd = follow;
    if (remotePane) remotePane.followCwd = follow;
    return follow;
  }

  function extOf(name) {
    const text = String(name || '');
    const idx = text.lastIndexOf('.');
    return idx > 0 ? text.slice(idx + 1).toLowerCase() : '';
  }

  function sortEntries(pane) {
    if (!pane || !Array.isArray(pane.entries)) return;
    const col = pane.sortColumn;
    const asc = pane.sortAscending;
    pane.entries.sort((left, right) => {
      if (left.is_dir !== right.is_dir) return left.is_dir ? -1 : 1;
      let ord = 0;
      if (col === 'name') {
        ord = String(left.name || '').toLowerCase().localeCompare(String(right.name || '').toLowerCase());
      } else if (col === 'ext') {
        ord = extOf(left.name).localeCompare(extOf(right.name));
      } else if (col === 'size') {
        ord = (left.size || 0) - (right.size || 0);
      } else if (col === 'modified') {
        ord = (left.modified || 0) - (right.modified || 0);
      }
      return asc ? ord : -ord;
    });
  }

  global.conchFilesPaneStore = {
    createPaneState,
    getFollowPathFromSettings,
    applyFollowPathSetting,
    extOf,
    sortEntries,
  };
})(window);

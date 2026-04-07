(function initConchLayoutService(global) {
  'use strict';

  function create(deps) {
    const invoke = deps && deps.invoke;

    function getSavedLayout() {
      if (typeof invoke !== 'function') return Promise.resolve({});
      return invoke('get_saved_layout').catch(() => ({}));
    }

    function saveLayout(layout) {
      if (!layout || typeof layout !== 'object') return Promise.resolve();
      if (typeof invoke !== 'function') return Promise.resolve();
      return invoke('save_window_layout', { layout }).catch(() => {});
    }

    function savePartialLayout(patch) {
      if (!patch || typeof patch !== 'object') return Promise.resolve();
      return saveLayout(patch);
    }

    function createDebouncedSaver(delayMs) {
      const delay = typeof delayMs === 'number' && delayMs >= 0 ? delayMs : 150;
      let timer = null;
      return function debouncedSave(getLayout) {
        if (timer) clearTimeout(timer);
        timer = setTimeout(() => {
          timer = null;
          let next = null;
          try {
            next = typeof getLayout === 'function' ? getLayout() : null;
          } catch (_) {
            next = null;
          }
          if (!next) return;
          saveLayout(next);
        }, delay);
      };
    }

    return {
      getSavedLayout,
      saveLayout,
      savePartialLayout,
      createDebouncedSaver,
    };
  }

  global.conchLayoutService = {
    create,
  };
})(window);

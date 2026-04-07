(function initConchKeyboardRouter(global) {
  'use strict';

  let nextHandlerId = 1;
  const handlers = new Map();

  function toSortedHandlers() {
    return Array.from(handlers.values()).sort((left, right) => {
      if (left.priority !== right.priority) return right.priority - left.priority;
      return left.order - right.order;
    });
  }

  function dispatch(event, phase) {
    const sorted = toSortedHandlers();
    for (const entry of sorted) {
      if (typeof entry.isActive === 'function' && !entry.isActive()) continue;

      const fn = phase === 'keyup' ? entry.onKeyUp : entry.onKeyDown;
      if (typeof fn !== 'function') continue;

      let consumed = false;
      try {
        consumed = fn(event, entry) === true;
      } catch (error) {
        console.warn('Keyboard handler failed:', entry.name || entry.id, error);
      }

      if (consumed || event.defaultPrevented) {
        if (consumed && !event.defaultPrevented && event.cancelable) {
          event.preventDefault();
        }
        event.stopPropagation();
        if (typeof event.stopImmediatePropagation === 'function') {
          event.stopImmediatePropagation();
        }
        return true;
      }
    }
    return false;
  }

  function register(options) {
    const id = nextHandlerId++;
    handlers.set(id, {
      id,
      order: id,
      name: options && options.name ? String(options.name) : '',
      priority: options && typeof options.priority === 'number' ? options.priority : 0,
      isActive: options && typeof options.isActive === 'function' ? options.isActive : null,
      onKeyDown: options && typeof options.onKeyDown === 'function' ? options.onKeyDown : null,
      onKeyUp: options && typeof options.onKeyUp === 'function' ? options.onKeyUp : null,
    });

    return function unregister() {
      handlers.delete(id);
    };
  }

  function clearAll() {
    handlers.clear();
  }

  document.addEventListener('keydown', (event) => {
    dispatch(event, 'keydown');
  }, true);

  document.addEventListener('keyup', (event) => {
    dispatch(event, 'keyup');
  }, true);

  global.conchKeyboardRouter = {
    register,
    clearAll,
  };
})(window);

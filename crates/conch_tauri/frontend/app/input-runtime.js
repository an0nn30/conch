(function initConchInputRuntime(global) {
  function create() {
    function isTextInputTarget(el) {
      if (!el) return false;
      const tag = (el.tagName || '').toUpperCase();
      if (tag === 'TEXTAREA') {
        const cls = String(el.className || '');
        if (cls.includes('xterm-helper-textarea')) return false;
        return true;
      }
      if (tag === 'INPUT') {
        const type = (el.type || 'text').toLowerCase();
        return !['button', 'checkbox', 'color', 'file', 'hidden', 'image', 'radio', 'range', 'reset', 'submit'].includes(type);
      }
      if (typeof el.isContentEditable === 'boolean' && el.isContentEditable) return true;
      return false;
    }

    return {
      isTextInputTarget,
    };
  }

  global.conchInputRuntime = {
    create,
  };
})(window);

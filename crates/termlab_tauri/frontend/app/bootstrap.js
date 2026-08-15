(function initTermLabBootstrap(global) {
  function run(startFn) {
    return Promise.resolve()
      .then(() => startFn())
      .catch((error) => {
        console.error('App bootstrap failed:', error);
        if (typeof global.__termlabShowStatus === 'function') {
          global.__termlabShowStatus('Failed to bootstrap app: ' + String(error));
        }
      });
  }

  global.termlabBootstrap = {
    run,
  };
})(window);

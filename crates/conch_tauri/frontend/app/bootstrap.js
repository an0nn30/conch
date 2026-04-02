(function initConchBootstrap(global) {
  function run(startFn) {
    return Promise.resolve()
      .then(() => startFn())
      .catch((error) => {
        console.error('App bootstrap failed:', error);
        if (typeof global.__conchShowStatus === 'function') {
          global.__conchShowStatus('Failed to bootstrap app: ' + String(error));
        }
      });
  }

  global.conchBootstrap = {
    run,
  };
})(window);

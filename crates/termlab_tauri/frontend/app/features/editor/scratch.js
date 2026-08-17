// Scratch file naming.
//
// Scratches are real files from the moment they are created, so the name has
// to be free on disk before the file is written — hence a pure function over
// the directory listing rather than a session counter.
(function initTermLabEditorScratch(global) {
  'use strict';

  function nextScratchName(existing) {
    const taken = new Set(Array.isArray(existing) ? existing : []);
    let n = 1;
    while (taken.has(`scratch-${n}.txt`)) n += 1;
    return `scratch-${n}.txt`;
  }

  global.termlabEditorScratch = { nextScratchName };
})(window);

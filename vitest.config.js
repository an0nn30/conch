import { defineConfig } from 'vitest/config';

export default defineConfig({
  test: {
    // The frontend runtime is plain IIFE scripts that attach globals to
    // `window`, so tests import a file for its side effects and then read the
    // global it registers. That needs a DOM.
    environment: 'jsdom',
    include: ['crates/termlab_tauri/frontend/**/*.test.js'],
  },
});

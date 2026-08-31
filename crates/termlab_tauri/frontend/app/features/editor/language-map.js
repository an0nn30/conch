// Filename → CodeMirror language key.
//
// The value is the name of an export in frontend/vendor-entry.mjs; the editor
// pane resolves it against window.CM6. Keeping this a pure string→string map
// means the whole table is testable without a DOM or a CodeMirror instance.
(function initTermLabEditorLanguageMap(global) {
  'use strict';

  const BY_EXTENSION = {
    js: 'javascript', mjs: 'javascript', cjs: 'javascript',
    ts: 'javascript', jsx: 'javascript', tsx: 'javascript',
    json: 'json',
    py: 'python',
    md: 'markdown', markdown: 'markdown',
    rs: 'rust',
    // Markdown fences are tagged with the LANGUAGE NAME (```rust), not the
    // file extension (.rs) — fence-highlight.js builds a fake filename from
    // the fence tag to reuse this table, so the language name needs its own
    // entry alongside the real extension.
    rust: 'rust',
    html: 'html', htm: 'html',
    css: 'css',
    xml: 'xml',
    yml: 'yaml', yaml: 'yaml',
    sql: 'sql',
    java: 'java',
    c: 'cpp', h: 'cpp', cc: 'cpp', cpp: 'cpp', hpp: 'cpp', cxx: 'cpp',
    go: 'go',
    php: 'php',
    sh: 'shell', bash: 'shell', zsh: 'shell',
    toml: 'toml',
    lua: 'lua',
    rb: 'ruby',
    pl: 'perl', pm: 'perl',
    ps1: 'powerShell',
    diff: 'diff', patch: 'diff',
    properties: 'properties',
    conf: 'nginx',
  };

  // Files whose whole name identifies them, with no extension to go on.
  const BY_NAME = {
    dockerfile: 'dockerFile',
    '.bashrc': 'shell',
    '.bash_profile': 'shell',
    '.zshrc': 'shell',
    '.profile': 'shell',
  };

  function languageKeyFor(filename) {
    if (typeof filename !== 'string' || filename.length === 0) return null;
    const base = filename.split('/').pop().split('\\').pop();
    if (!base) return null;

    const byName = BY_NAME[base.toLowerCase()];
    if (byName) return byName;

    const dot = base.lastIndexOf('.');
    if (dot <= 0) return null; // no extension, or a dotfile we do not know
    const ext = base.slice(dot + 1).toLowerCase();
    return BY_EXTENSION[ext] || null;
  }

  global.termlabEditorLanguageMap = { languageKeyFor };
})(window);

#!/usr/bin/env bash
set -euo pipefail

ROOT="${1:-.}"
OUT="${2:-$ROOT/docs/superpowers/plans/2026-04-06-frontend-runtime-baseline.md}"

mkdir -p "$(dirname "$OUT")"

node - "$ROOT" "$OUT" <<'NODE'
const fs = require('fs');
const path = require('path');
const vm = require('vm');
const { performance } = require('perf_hooks');

const root = path.resolve(process.argv[2]);
const out = path.resolve(process.argv[3]);
const frontend = path.join(root, 'crates', 'conch_tauri', 'frontend');

function makeClassList() {
  const set = new Set();
  return {
    add: (...names) => names.forEach((name) => set.add(name)),
    remove: (...names) => names.forEach((name) => set.delete(name)),
    contains: (name) => set.has(name),
    toString: () => Array.from(set).join(' '),
  };
}

function makeElement(id) {
  return {
    id,
    style: {},
    textContent: '',
    classList: makeClassList(),
    addEventListener: () => {},
    removeEventListener: () => {},
  };
}

function loadScriptInContext(filePath, sandbox, codeTransform) {
  const src = fs.readFileSync(filePath, 'utf8');
  const code = typeof codeTransform === 'function' ? codeTransform(src) : src;
  vm.runInContext(code, sandbox, { filename: filePath });
}

function percentile(values, p) {
  if (!values.length) return 0;
  const sorted = [...values].sort((a, b) => a - b);
  const idx = Math.min(sorted.length - 1, Math.max(0, Math.ceil((p / 100) * sorted.length) - 1));
  return sorted[idx];
}

async function measureStartupRuntime() {
  const elMap = new Map();
  const getEl = (id) => {
    if (!elMap.has(id)) elMap.set(id, makeElement(id));
    return elMap.get(id);
  };
  const sandbox = {
    window: {},
    document: {
      getElementById: (id) => getEl(id),
      addEventListener: () => {},
      removeEventListener: () => {},
    },
    console,
    setTimeout,
    clearTimeout,
    Promise,
  };
  sandbox.window = sandbox.window || {};
  sandbox.window.document = sandbox.document;
  sandbox.window.console = console;
  sandbox.window.addEventListener = () => {};
  sandbox.window.notificationPanel = { init: () => {} };
  sandbox.window.conchKeyboardRouter = { register: () => () => {} };
  sandbox.window.conchConfigService = {
    toTerminalTheme: (_tc, fallbackTheme) => fallbackTheme,
    applyThemeCss: () => {},
    applyUiConfig: () => ({ borderlessMode: false }),
  };
  vm.createContext(sandbox);

  const startupRuntimePath = path.join(frontend, 'app', 'startup-runtime.js');
  loadScriptInContext(startupRuntimePath, sandbox);

  const runtimeFactory = sandbox.window.conchStartupRuntime && sandbox.window.conchStartupRuntime.create;
  if (typeof runtimeFactory !== 'function') {
    throw new Error('startup-runtime create() unavailable');
  }
  const runtime = runtimeFactory();

  const invoke = async (cmd) => {
    if (cmd === 'get_terminal_config') {
      return {
        font_family: 'JetBrains Mono',
        font_size: 14,
        cursor_style: 'block',
        cursor_blink: true,
        scroll_sensitivity: 1,
      };
    }
    if (cmd === 'get_theme_colors') {
      return {
        background: '#000000',
        foreground: '#ffffff',
        cursor_color: '#ffffff',
        cursor_text: '#000000',
        selection_bg: '#333333',
        selection_text: '#ffffff',
        black: '#000000',
        red: '#ff0000',
        green: '#00ff00',
        yellow: '#ffff00',
        blue: '#0000ff',
        magenta: '#ff00ff',
        cyan: '#00ffff',
        white: '#ffffff',
        bright_black: '#666666',
        bright_red: '#ff6666',
        bright_green: '#66ff66',
        bright_yellow: '#ffff66',
        bright_blue: '#6666ff',
        bright_magenta: '#ff66ff',
        bright_cyan: '#66ffff',
        bright_white: '#f5f5f5',
      };
    }
    if (cmd === 'get_app_config') {
      return { ui: { borderless: false } };
    }
    if (cmd === 'get_saved_layout') {
      return {
        zen_mode: false,
        files_panel_visible: true,
        ssh_panel_visible: true,
        bottom_panel_visible: true,
        bottom_panel_height: 220,
      };
    }
    return {};
  };

  const tCfgSamples = [];
  const themeSamples = [];
  const appCfgSamples = [];
  const fontFallbacks = ', monospace';
  const fallbackTheme = { background: '#000', foreground: '#fff' };

  for (let i = 0; i < 30; i += 1) {
    const t0 = performance.now();
    await runtime.loadTerminalConfig(invoke, fontFallbacks);
    tCfgSamples.push(performance.now() - t0);
  }
  for (let i = 0; i < 30; i += 1) {
    const t0 = performance.now();
    await runtime.loadTheme(invoke, fallbackTheme);
    themeSamples.push(performance.now() - t0);
  }
  for (let i = 0; i < 15; i += 1) {
    const t0 = performance.now();
    await runtime.applyAppConfig(invoke);
    appCfgSamples.push(performance.now() - t0);
  }

  return {
    terminalConfigMs: tCfgSamples,
    themeMs: themeSamples,
    appConfigMs: appCfgSamples,
  };
}

async function measureCommandPaletteBuild() {
  const sandbox = {
    window: {},
    document: {
      createElement: () => ({
        style: {},
        classList: { add: () => {}, remove: () => {} },
        addEventListener: () => {},
        appendChild: () => {},
        querySelector: () => ({
          addEventListener: () => {},
          focus: () => {},
          value: '',
        }),
        remove: () => {},
        innerHTML: '',
      }),
      querySelector: () => null,
      addEventListener: () => {},
      removeEventListener: () => {},
      body: { appendChild: () => {} },
      getElementById: () => null,
    },
    console,
    setTimeout,
    clearTimeout,
    Promise,
  };
  sandbox.window.document = sandbox.document;
  vm.createContext(sandbox);

  const palettePath = path.join(frontend, 'app', 'command-palette-runtime.js');
  loadScriptInContext(
    palettePath,
    sandbox,
    (src) => src.replace(
      'global.__conchInvalidateCommandPaletteCache = invalidateCommandCache;',
      'global.__conchInvalidateCommandPaletteCache = invalidateCommandCache; global.__conchDebugBuildPaletteCommands = buildPaletteCommands;'
    )
  );

  let callCount = 0;
  const invoke = async (cmd) => {
    callCount += 1;
    if (cmd === 'scan_plugins') return [];
    if (cmd === 'get_plugin_menu_items') return [];
    if (cmd === 'remote_get_servers') return { folders: [], ungrouped: [], ssh_config: [] };
    if (cmd === 'tunnel_get_all') return [];
    return [];
  };

  const runtimeFactory = sandbox.window.conchCommandPaletteRuntime && sandbox.window.conchCommandPaletteRuntime.create;
  if (typeof runtimeFactory !== 'function') {
    throw new Error('command-palette create() unavailable');
  }
  runtimeFactory({
    invoke,
    listen: () => Promise.resolve(() => {}),
    esc: (value) => String(value == null ? '' : value),
    handleMenuAction: () => {},
    createSshTab: () => {},
    getCurrentPane: () => null,
    showStatus: () => {},
    refreshTitlebar: () => {},
    refreshSshPanel: () => {},
  });

  const buildFn = sandbox.window.__conchDebugBuildPaletteCommands;
  if (typeof buildFn !== 'function') {
    throw new Error('debug palette build function unavailable');
  }

  const samples = [];
  let commandCount = 0;
  for (let i = 0; i < 40; i += 1) {
    const t0 = performance.now();
    const commands = await buildFn();
    samples.push(performance.now() - t0);
    commandCount = Array.isArray(commands) ? commands.length : 0;
  }

  return { samples, invokeCalls: callCount, commandCount };
}

function collectRenderHotspots() {
  const hotspotFiles = [
    path.join(frontend, 'app', 'panels', 'files-panel.js'),
    path.join(frontend, 'app', 'panels', 'ssh-panel.js'),
    path.join(frontend, 'app', 'panels', 'vault.js'),
    path.join(frontend, 'app', 'panels', 'settings.js'),
  ];
  return hotspotFiles.map((filePath) => {
    const source = fs.readFileSync(filePath, 'utf8');
    const rel = path.relative(root, filePath);
    return {
      file: rel,
      renderCalls: (source.match(/\brender[A-Za-z0-9_]*\s*\(/g) || []).length,
      eventListeners: (source.match(/\.addEventListener\(/g) || []).length,
      innerHtmlWrites: (source.match(/\.innerHTML\s*=/g) || []).length,
      loc: source.split('\n').length,
    };
  });
}

function summarize(name, samples) {
  const avg = samples.reduce((sum, value) => sum + value, 0) / Math.max(1, samples.length);
  return {
    name,
    n: samples.length,
    avg,
    p95: percentile(samples, 95),
    min: Math.min(...samples),
    max: Math.max(...samples),
  };
}

(async () => {
  const startup = await measureStartupRuntime();
  const palette = await measureCommandPaletteBuild();
  const hotspots = collectRenderHotspots();

  const startupTerminal = summarize('startup.loadTerminalConfig', startup.terminalConfigMs);
  const startupTheme = summarize('startup.loadTheme', startup.themeMs);
  const startupApp = summarize('startup.applyAppConfig', startup.appConfigMs);
  const paletteBuild = summarize('commandPalette.buildCommands', palette.samples);

  const lines = [];
  lines.push('# Frontend Runtime Baseline Snapshot');
  lines.push('');
  lines.push(`- Date: ${new Date().toISOString()}`);
  lines.push(`- Scope: \`crates/conch_tauri/frontend/app\``);
  lines.push('- Method: simulated runtime microbench with mocked Tauri/document dependencies (agent-run reproducible script)');
  lines.push('');
  lines.push('## Startup Metrics (ms)');
  lines.push('');
  lines.push('| Metric | Samples | Avg | P95 | Min | Max |');
  lines.push('|---|---:|---:|---:|---:|---:|');
  for (const row of [startupTerminal, startupTheme, startupApp]) {
    lines.push(`| \`${row.name}\` | ${row.n} | ${row.avg.toFixed(3)} | ${row.p95.toFixed(3)} | ${row.min.toFixed(3)} | ${row.max.toFixed(3)} |`);
  }
  lines.push('');
  lines.push('## Command Palette Metrics (ms)');
  lines.push('');
  lines.push('| Metric | Samples | Avg | P95 | Min | Max | Commands | Invoke Calls |');
  lines.push('|---|---:|---:|---:|---:|---:|---:|---:|');
  lines.push(`| \`${paletteBuild.name}\` | ${paletteBuild.n} | ${paletteBuild.avg.toFixed(3)} | ${paletteBuild.p95.toFixed(3)} | ${paletteBuild.min.toFixed(3)} | ${paletteBuild.max.toFixed(3)} | ${palette.commandCount} | ${palette.invokeCalls} |`);
  lines.push('');
  lines.push('## Render Churn Hotspot Signals (Static Heuristics)');
  lines.push('');
  lines.push('| File | LOC | `render*(` calls | `.addEventListener(` | `.innerHTML =` |');
  lines.push('|---|---:|---:|---:|---:|');
  for (const spot of hotspots) {
    lines.push(`| \`${spot.file}\` | ${spot.loc} | ${spot.renderCalls} | ${spot.eventListeners} | ${spot.innerHtmlWrites} |`);
  }
  lines.push('');
  lines.push('## Notes');
  lines.push('');
  lines.push('- These numbers are a remediation baseline for trend comparison, not end-user wall-clock startup timings.');
  lines.push('- Use this alongside manual QA for full desktop rendering/path correctness checks.');

  fs.writeFileSync(out, lines.join('\n') + '\n', 'utf8');
  process.stdout.write(`snapshot_frontend_runtime_baseline: wrote ${out}\n`);
})().catch((error) => {
  process.stderr.write(`snapshot_frontend_runtime_baseline: failed: ${error && error.stack ? error.stack : String(error)}\n`);
  process.exit(1);
});
NODE


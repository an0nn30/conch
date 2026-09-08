// A live, in-app trace of what happens when vim's <C-o>/<C-i> are pressed.
//
// Why this exists rather than a console.log: `gd` jumps in the built app but
// Ctrl-O/Ctrl-I do nothing, and every static explanation has been eliminated —
// the mapping resolves against the SHIPPED vendor bundle, no router, config or
// menu binding claims the combination, and the registration is unconditional.
// What is left can only be seen in a running WKWebView, and the owner does not
// open devtools. So the three places a press can die each raise a toast:
//
//   1. the capture-phase keyboard router receiving the keydown at all
//      (shortcut-runtime.js registers the observer). Reports `key` AND `code`,
//      because a WKWebView can deliver the keystroke as key "Process" or
//      "Dead" while `code` still says KeyO — and WebKit's own emacs-style
//      Ctrl-O editing binding could eat it before the app ever sees it;
//   2. vim matching the key to OUR action (vim-mode.js's mapped actions);
//   3. what navigateBack/navigateForward then did (lsp-navigation.js).
//
// Reading the toasts:
//   nothing at all      -> WebKit swallowed the key before the app;
//   1 only              -> the key reached the app but vim never handled it;
//   1 and 2, 3 "empty"  -> the keys work and the trail is empty (recorder);
//   1, 2 and 3          -> the walk ran, and 3 says what it did.
//
// The trace is a MODULE VARIABLE, defaults off, and is never persisted: it is
// a diagnostic for one sitting, not a setting. Nothing here changes what any
// key does — the observer in shortcut-runtime.js never consumes, and the two
// note points are called for their toast and their return value is ignored.
(function initTermLabVimJumpTrace(global) {
  'use strict';

  // Short: three of these can land in a row from a single keypress, and they
  // must not bury the app's own notifications.
  const TOAST_MS = 2600;

  let enabled = false;

  function toastInfo(title, body) {
    const toast = global.toast;
    if (toast && typeof toast.info === 'function') {
      toast.info(title, body, { duration: TOAST_MS });
      return true;
    }
    return false;
  }

  function isEnabled() {
    return enabled;
  }

  // The DURABLE half. Every note point below writes here as well as (maybe)
  // toasting, and this half is NOT gated on `enabled`: the toast toggle is for
  // watching a sequence live, but a bug report is written after the fact, and
  // the owner cannot have armed a trace for a press they had not yet made.
  // features/diagnostics/diag-log.js is fire-and-forget and swallows
  // everything, so this can never change what a key does.
  const CATEGORY = 'vim-nav';
  function record(message) {
    const diag = global.termlabDiag;
    if (diag && typeof diag.log === 'function') diag.log(CATEGORY, message);
  }

  function setEnabled(next) {
    enabled = next === true;
    return enabled;
  }

  // Matched on `code` as well as `key`, and that is the whole point: if the
  // webview delivers Ctrl-O as key "Process" or "Dead", a `key`-only test
  // would report nothing in exactly the case worth reporting. Modifier state
  // beyond Ctrl is not filtered — a Cmd-Ctrl-O that somehow arrives is still
  // evidence, and this only decides what to TOAST.
  function isJumpKeyEvent(event) {
    if (!event || event.ctrlKey !== true) return false;
    const key = String(event.key === undefined || event.key === null ? '' : event.key).toLowerCase();
    const code = String(event.code === undefined || event.code === null ? '' : event.code);
    return key === 'i' || key === 'o' || code === 'KeyI' || code === 'KeyO';
  }

  // Quoted, so an empty or whitespace key is visible as "" rather than as a
  // gap in the toast.
  function quoted(value) {
    return `"${String(value === undefined || value === null ? '' : value)}"`;
  }

  function describeKeyEvent(event) {
    if (!event) return 'no event';
    const parts = [`key=${quoted(event.key)}`, `code=${quoted(event.code)}`];
    const mods = [];
    if (event.ctrlKey) mods.push('ctrl');
    if (event.metaKey) mods.push('cmd');
    if (event.altKey) mods.push('alt');
    if (event.shiftKey) mods.push('shift');
    parts.push(`mods=${mods.length ? mods.join('+') : 'none'}`);
    if (event.isComposing === true) parts.push('composing');
    if (event.repeat === true) parts.push('repeat');
    if (event.defaultPrevented === true) parts.push('defaultPrevented');
    const target = event.target && event.target.tagName;
    if (target) parts.push(`target=${String(target).toLowerCase()}`);
    return parts.join(' ');
  }

  // Stage 1. Called for EVERY keydown the router sees, so it filters first and
  // never toasts for anything but Ctrl-I/Ctrl-O.
  function noteKey(event) {
    if (!isJumpKeyEvent(event)) return false;
    record(`1 router saw the key — ${describeKeyEvent(event)}`);
    if (!enabled) return false;
    return toastInfo('Vim trace 1: router saw the key', describeKeyEvent(event));
  }

  // Stage 2. Every mapped vim action reports here, not just the two jump ones:
  // seeing `gd` fire and Ctrl-O not is itself the answer.
  function noteVimAction(name) {
    record(`2 vim ran the mapping — ${String(name)}`);
    if (!enabled) return false;
    return toastInfo('Vim trace 2: vim ran the mapping', String(name));
  }

  // step() answers 'none' for "there was nothing to walk to", which is the
  // reading the owner needs spelled out.
  function normalizeOutcome(outcome) {
    const text = String(outcome === undefined || outcome === null ? '' : outcome);
    if (text === 'none' || text === '') return 'empty';
    return text;
  }

  // Stage 3.
  function noteNavigation(direction, outcome) {
    const body = `${normalizeOutcome(outcome)} — ${formatTrail(historyState())}`;
    record(`3 navigate ${String(direction)} — ${body}`);
    if (!enabled) return false;
    return toastInfo(`Vim trace 3: navigate ${String(direction)}`, body);
  }

  // Stage 0: what the trail RECORDER did. Stages 1-3 can only ever say the
  // trail was empty; this says why. File-only — it fires on ordinary file
  // opens, which is far too often to toast — and reports the pane identity as
  // well as the document, because "the trail is empty" and "the trail was
  // recorded against a pane that no longer exists" look identical from the
  // walk's end.
  function noteRecord(source, outcome, detail) {
    record(`0 record ${String(source)} — ${String(outcome)}${detail ? ` — ${String(detail)}` : ''}`);
    return false;
  }

  function historyState() {
    const navigation = global.termlabLspNavigation;
    if (!navigation || typeof navigation.historyState !== 'function') return null;
    try {
      return navigation.historyState();
    } catch (_) {
      return null;
    }
  }

  function baseName(value) {
    const text = String(value === undefined || value === null ? '' : value);
    const at = text.lastIndexOf('/');
    return at < 0 ? text : text.slice(at + 1);
  }

  // `file:line`, in the 1-based line numbers the editor's own gutter shows —
  // the history speaks LSP positions, which are 0-based.
  function entryLabel(entry) {
    if (!entry) return 'none';
    const uri = global.termlabLspUri;
    const filePath = uri && typeof uri.uriToPath === 'function' ? uri.uriToPath(entry.uri) : entry.uri;
    const line = entry.position && Number.isInteger(entry.position.line)
      ? entry.position.line + 1
      : '?';
    return `${baseName(filePath)}:${line}`;
  }

  function formatTrail(state) {
    if (!state) return 'no navigation history in this window';
    const back = Array.isArray(state.back) ? state.back : [];
    const forward = Array.isArray(state.forward) ? state.forward : [];
    const top = back.length ? entryLabel(back[back.length - 1]) : 'none';
    return `back ${back.length} / forward ${forward.length} — top back entry ${top}`;
  }

  // The palette's "Show Jump Trail". Unconditional: reading the trail is the
  // half of the diagnostic that answers "is the recorder working", and having
  // to arm the trace first would be a trap.
  function showTrail() {
    const summary = formatTrail(historyState());
    toastInfo('Vim jump trail', summary);
    return summary;
  }

  // The palette's "Trace Keys (toggle)". Always toasts, including when
  // switching OFF — otherwise the off press looks like a dead command.
  function toggleTrace() {
    const next = setEnabled(!enabled);
    toastInfo(
      'Vim navigation trace',
      next
        ? 'On. Press Ctrl-O or Ctrl-I in the editor; each stage that fires raises a toast.'
        : 'Off.',
    );
    return next;
  }

  global.termlabVimJumpTrace = {
    isEnabled,
    setEnabled,
    toggleTrace,
    isJumpKeyEvent,
    describeKeyEvent,
    normalizeOutcome,
    formatTrail,
    entryLabel,
    noteKey,
    noteVimAction,
    noteNavigation,
    noteRecord,
    showTrail,
  };
})(window);

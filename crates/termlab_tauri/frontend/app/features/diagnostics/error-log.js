// Uncaught frontend errors, routed into the on-disk diagnostic log.
//
// startup-runtime.js already listens for `error` and `unhandledrejection` and
// paints the message into the status banner. That banner is the only thing the
// owner sees in the built app, and it carries the MESSAGE and nothing else —
// no stack, no note of where in the app it happened, and it is gone the moment
// it is dismissed. "Frontend error: TypeError: undefined is not an object
// (evaluating 'n.isText')" is a real report we received, and the message alone
// named neither the caller nor the document it happened on.
//
// So the same two events also come here, and here they are written to
// ~/.config/termlab/logs/frontend.log at category `error` with:
//
//   * the message;
//   * the stack, when the event carries one (window `error` gives it on
//     `event.error`, a rejection on `event.reason`) — trimmed, because
//     diag_log.rs caps a record at 2000 characters and a deep vendor stack
//     would otherwise push the message itself out;
//   * where the app was: the focused pane's id, kind and path, which is the
//     difference between "it threw" and "it threw on the file I had just
//     jumped away from".
//
// The banner is unchanged: this is an additional reader of the same events,
// never a replacement. Nothing here can break the app — every accessor is
// wrapped, and a window with no diagnostic log at all just gets the banner it
// had before.
(function initTermLabErrorLog(global) {
  'use strict';

  // How much of a stack a record keeps. diag_log.rs truncates at 2000
  // characters, and the frames that matter are the innermost ones — a vendor
  // bundle can otherwise contribute forty frames of app bootstrap that say
  // nothing about the throw.
  const MAX_STACK_FRAMES = 12;

  // Belt and braces on top of the frame cap: one enormous single line (a
  // minified bundle has them) must not eat the whole record either.
  const MAX_STACK_CHARS = 1200;

  function text(value) {
    if (value === undefined) return 'undefined';
    if (value === null) return 'null';
    try {
      return typeof value === 'string' ? value : String(value);
    } catch (_) {
      return '<unprintable>';
    }
  }

  // The innermost `MAX_STACK_FRAMES` frames, on one line, so a record stays a
  // record. Returns '' when there is no usable stack — the caller then omits
  // the field rather than logging "stack=undefined".
  function formatStack(stack) {
    if (typeof stack !== 'string' || !stack) return '';
    const frames = stack.split('\n').map((line) => line.trim()).filter(Boolean);
    if (!frames.length) return '';
    const kept = frames.slice(0, MAX_STACK_FRAMES).join(' | ');
    return kept.length > MAX_STACK_CHARS ? `${kept.slice(0, MAX_STACK_CHARS)}…` : kept;
  }

  // Where the app was when it threw. Read through the same
  // `__termlabPaneAccess` seam every other feature uses, and never allowed to
  // throw on its own: a diagnostic that fails while describing a failure is
  // worse than no diagnostic. Returns '' when there is nothing to say.
  function describeContext() {
    try {
      const access = global.__termlabPaneAccess;
      if (!access || typeof access.currentPane !== 'function') return '';
      const pane = access.currentPane();
      if (!pane) return 'pane=none';
      const parts = [`pane=${text(pane.paneId)}`, `kind=${text(pane.kind)}`];
      parts.push(`path=${pane.filePath ? text(pane.filePath) : 'none'}`);
      if (!pane.view) parts.push('noView');
      return parts.join(' ');
    } catch (_) {
      return '';
    }
  }

  // The record, built from already-extracted parts. Pure, and the piece worth
  // testing: the shape of a line in frontend.log is the contract a bug report
  // is read against.
  //
  // `kind` is a prefix only when it says something the category does not. The
  // line in the file already reads `… [error] error: <message>`, so an
  // uncaught exception adds no prefix at all; a rejection does, because
  // "nobody caught this promise" is a different failure from "this threw".
  function describe(parts) {
    const source = parts || {};
    const kind = source.kind ? `${text(source.kind)}: ` : '';
    const pieces = [`${kind}${text(source.message)}`];
    const where = typeof source.context === 'string' ? source.context : '';
    if (where) pieces.push(`at ${where}`);
    const stack = formatStack(source.stack);
    if (stack) pieces.push(`stack: ${stack}`);
    return pieces.join(' — ');
  }

  // A window `error` event. The stack lives on `event.error`, which is absent
  // for a cross-origin script error — the message is still worth a record.
  function fromErrorEvent(event) {
    const source = event || {};
    const error = source.error;
    return {
      // No prefix: the record's category is already `error`, and the browser's
      // own message usually starts with the exception's type.
      kind: '',
      message: text(source.message),
      stack: error && typeof error.stack === 'string' ? error.stack : '',
      context: describeContext(),
    };
  }

  // An `unhandledrejection`. `reason` is whatever was rejected with, and is
  // very often not an Error at all, so the message is its string form and the
  // stack is only read when one is actually there.
  function fromRejectionEvent(event) {
    const source = event || {};
    const reason = source.reason;
    return {
      kind: 'unhandled rejection',
      message: text(reason && reason.message ? reason.message : reason),
      stack: reason && typeof reason.stack === 'string' ? reason.stack : '',
      context: describeContext(),
    };
  }

  function record(parts) {
    try {
      const diag = global.termlabDiag;
      if (!diag || typeof diag.log !== 'function') return false;
      diag.log('error', describe(parts), 'error');
      return true;
    } catch (_) {
      return false;
    }
  }

  function noteError(event) {
    return record(fromErrorEvent(event));
  }

  function noteRejection(event) {
    return record(fromRejectionEvent(event));
  }

  // Wire both events. Called from startup-runtime's initStatusController, next
  // to the banner's own listeners, so the two readers are registered together
  // and neither can exist without the other being obvious in review.
  //
  // Returns an uninstall function; a second install() is a no-op, because the
  // one thing worse than an unrecorded error is the same error recorded twice
  // with a different pane in each copy.
  let installed = false;
  function install(target) {
    const host = target || global;
    if (installed || !host || typeof host.addEventListener !== 'function') return () => {};
    installed = true;
    const onError = (event) => { noteError(event); };
    const onRejection = (event) => { noteRejection(event); };
    host.addEventListener('error', onError);
    host.addEventListener('unhandledrejection', onRejection);
    return () => {
      installed = false;
      if (typeof host.removeEventListener !== 'function') return;
      host.removeEventListener('error', onError);
      host.removeEventListener('unhandledrejection', onRejection);
    };
  }

  global.termlabErrorLog = {
    MAX_STACK_FRAMES,
    MAX_STACK_CHARS,
    describe,
    describeContext,
    formatStack,
    fromErrorEvent,
    fromRejectionEvent,
    noteError,
    noteRejection,
    install,
  };
})(window);

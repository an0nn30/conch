#!/usr/bin/env bash
set -euo pipefail

ROOT="${1:-.}"
FRONTEND_DIR="$ROOT/crates/conch_tauri/frontend"

if [[ ! -d "$FRONTEND_DIR" ]]; then
  echo "frontend-boundary-check: missing directory: $FRONTEND_DIR" >&2
  exit 2
fi

fail=0

echo "frontend-boundary-check: scanning for direct Tauri core.invoke usage"
if rg -n "__TAURI__\\.core\\.invoke|const \\{ invoke \\} = window\\.__TAURI__\\.core" "$FRONTEND_DIR" >/tmp/frontend-boundary-invoke.txt; then
  echo "frontend-boundary-check: disallowed direct invoke usage found:" >&2
  cat /tmp/frontend-boundary-invoke.txt >&2
  fail=1
else
  echo "frontend-boundary-check: ok (no direct core.invoke usage)"
fi

echo "frontend-boundary-check: scanning html entrypoints for global contextmenu suppression"
if rg -n "document\\.addEventListener\\('contextmenu'" \
  "$FRONTEND_DIR/index.html" \
  "$FRONTEND_DIR/settings.html" >/tmp/frontend-boundary-contextmenu.txt; then
  echo "frontend-boundary-check: disallowed html-level contextmenu suppression found:" >&2
  cat /tmp/frontend-boundary-contextmenu.txt >&2
  fail=1
else
  echo "frontend-boundary-check: ok (no html-level contextmenu suppression)"
fi

echo "frontend-boundary-check: scanning settings entrypoint for inline style blocks"
if rg -n "<style>" "$FRONTEND_DIR/settings.html" >/tmp/frontend-boundary-settings-style.txt; then
  echo "frontend-boundary-check: disallowed inline style block found in settings.html:" >&2
  cat /tmp/frontend-boundary-settings-style.txt >&2
  fail=1
else
  echo "frontend-boundary-check: ok (settings.html style loaded from shared stylesheet)"
fi

echo "frontend-boundary-check: scanning ssh/vault panels for ad hoc document keydown handlers"
overlay_keydown_hits="$(
  rg -n "document\\.addEventListener\\('keydown'" \
    "$FRONTEND_DIR/app/panels/ssh-panel.js" \
    "$FRONTEND_DIR/app/panels/vault.js" \
  | rg -v "onDocKey" \
  || true
)"
if [[ -n "$overlay_keydown_hits" ]]; then
  echo "frontend-boundary-check: disallowed ad hoc document keydown usage found in ssh/vault panels:" >&2
  printf '%s\n' "$overlay_keydown_hits" >&2
  fail=1
else
  echo "frontend-boundary-check: ok (ssh/vault keydown handlers are routed through scoped router helper)"
fi

echo "frontend-boundary-check: scanning frontend app for direct document keydown listeners outside keyboard router"
raw_doc_keydown_hits="$(
  rg -n "document\\.addEventListener\\('keydown'" "$FRONTEND_DIR/app" \
  | rg -v "app/core/keyboard-router\\.js" \
  || true
)"
if [[ -n "$raw_doc_keydown_hits" ]]; then
  echo "frontend-boundary-check: disallowed direct document keydown usage found outside keyboard-router:" >&2
  printf '%s\n' "$raw_doc_keydown_hits" >&2
  fail=1
else
  echo "frontend-boundary-check: ok (document keydown routing is centralized in keyboard-router)"
fi

echo "frontend-boundary-check: scanning for raw layout persistence callsites outside approved adapters"
raw_save_layout_hits="$(
  rg -n "invoke\\('save_window_layout'" "$FRONTEND_DIR/app" \
  | rg -v "app/core/layout-service\\.js|app/panels/files-panel\\.js|app/panels/ssh-panel\\.js|app/tool-window-runtime\\.js" \
  || true
)"
if [[ -n "$raw_save_layout_hits" ]]; then
  echo "frontend-boundary-check: disallowed raw save_window_layout usage found:" >&2
  printf '%s\n' "$raw_save_layout_hits" >&2
  fail=1
else
  echo "frontend-boundary-check: ok (save_window_layout usage constrained to approved adapters/fallbacks)"
fi

raw_get_layout_hits="$(
  rg -n "invoke\\('get_saved_layout'" "$FRONTEND_DIR/app" \
  | rg -v "app/core/layout-service\\.js|app/panels/files-panel\\.js|app/panels/ssh-panel\\.js|app/tool-window-runtime\\.js|app/startup-runtime\\.js" \
  || true
)"
if [[ -n "$raw_get_layout_hits" ]]; then
  echo "frontend-boundary-check: disallowed raw get_saved_layout usage found:" >&2
  printf '%s\n' "$raw_get_layout_hits" >&2
  fail=1
else
  echo "frontend-boundary-check: ok (get_saved_layout usage constrained to approved adapters/fallbacks)"
fi

if [[ "$fail" -ne 0 ]]; then
  echo "frontend-boundary-check: FAILED" >&2
  exit 1
fi

echo "frontend-boundary-check: PASSED"

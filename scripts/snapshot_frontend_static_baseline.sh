#!/usr/bin/env bash
set -euo pipefail

ROOT="${1:-.}"
OUT="${2:-$ROOT/docs/superpowers/plans/2026-04-06-frontend-static-baseline.md}"
FRONTEND="$ROOT/crates/conch_tauri/frontend"
APP_DIR="$FRONTEND/app"

if [[ ! -d "$APP_DIR" ]]; then
  echo "snapshot_frontend_static_baseline: missing app dir: $APP_DIR" >&2
  exit 2
fi

mkdir -p "$(dirname "$OUT")"

tmp_lines="$(mktemp)"
tmp_sorted="$(mktemp)"
trap 'rm -f "$tmp_lines" "$tmp_sorted"' EXIT

find "$APP_DIR" -type f -name '*.js' | while read -r f; do
  wc -l "$f" | awk '{print $1 "\t" $2}'
done > "$tmp_lines"
sort -rn "$tmp_lines" > "$tmp_sorted"

js_count="$(find "$APP_DIR" -type f -name '*.js' | wc -l | awk '{print $1}')"
total_lines="$(awk -F '\t' '{s += $1} END {print s+0}' "$tmp_sorted")"
keydown_count="$(rg -n "document\\.addEventListener\\('keydown'" "$APP_DIR" | wc -l | awk '{print $1}')"
contextmenu_count="$(rg -n "contextmenu" "$APP_DIR" "$FRONTEND/index.html" "$FRONTEND/settings.html" | wc -l | awk '{print $1}')"
overlay_count="$(rg -n "ssh-overlay" "$APP_DIR" | wc -l | awk '{print $1}')"
raw_layout_count="$(rg -n "invoke\\('save_window_layout'|invoke\\('get_saved_layout'" "$APP_DIR" | wc -l | awk '{print $1}')"

{
  echo "# Frontend Static Baseline Snapshot"
  echo
  echo "- Date: $(date '+%Y-%m-%d %H:%M:%S %Z')"
  echo "- Scope: \`crates/conch_tauri/frontend/app\`"
  echo "- Method: static source scan (no runtime timing instrumentation)"
  echo
  echo "## Summary"
  echo
  echo "- JavaScript files: $js_count"
  echo "- Total JS LOC: $total_lines"
  echo "- \`document.addEventListener('keydown')\` occurrences: $keydown_count"
  echo "- \`contextmenu\` occurrences (app + html entrypoints): $contextmenu_count"
  echo "- \`ssh-overlay\` references: $overlay_count"
  echo "- Raw layout invoke callsites (\`save_window_layout/get_saved_layout\`): $raw_layout_count"
  echo
  echo "## Largest JS Files (LOC)"
  echo
  echo "| LOC | File |"
  echo "|---:|---|"
  head -n 20 "$tmp_sorted" | while IFS=$'\t' read -r lines path; do
    rel="${path#$ROOT/}"
    echo "| $lines | \`$rel\` |"
  done
  echo
  echo "## Notes"
  echo
  echo "- This snapshot is intended for remediation tracking and architectural drift detection."
  echo "- Runtime startup/palette baseline metrics are tracked in \`docs/superpowers/plans/2026-04-06-frontend-runtime-baseline.md\`."
} > "$OUT"

echo "snapshot_frontend_static_baseline: wrote $OUT"

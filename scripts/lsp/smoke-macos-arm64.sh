#!/usr/bin/env bash
#
# Smoke-test the language servers packaged inside a built TermLab.app.
#
# This asserts the shipping contract rather than the build tree: both servers
# must live under Contents/Resources, be arm64 Mach-O executables, and answer a
# real initialize/shutdown/exit handshake over stdio. PATH is scrubbed down to
# the system utilities before each launch, so a server that quietly discovered a
# host Node or rust-analyzer would fail here instead of shipping broken.
#
# Usage:
#   scripts/lsp/smoke-macos-arm64.sh <path-to-TermLab.app>
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
HANDSHAKE_TOOL="$SCRIPT_DIR/lsp_handshake.py"
FETCH_TOOL="$SCRIPT_DIR/fetch-macos-arm64.sh"
SYSTEM_PATH="/usr/bin:/bin:/usr/sbin:/sbin"

log() {
  printf 'lsp-smoke: %s\n' "$1"
}

fail() {
  printf 'lsp-smoke: %s\n' "$1" >&2
  exit 1
}

[[ $# -eq 1 ]] || fail "usage: $(basename "${BASH_SOURCE[0]}") <path-to-TermLab.app>"

APP="$1"
[[ -d "$APP" ]] || fail "not an app bundle directory: $APP"
APP="$(cd "$APP" && pwd)"
RESOURCES="$APP/Contents/Resources"
[[ -d "$RESOURCES" ]] || fail "app bundle has no Contents/Resources: $APP"

LSP_ROOT="$RESOURCES/lsp"
ARCH_ROOT="$LSP_ROOT/arm64"
[[ -d "$ARCH_ROOT" ]] || fail "app bundle has no packaged LSP resources at $ARCH_ROOT"

# Prefer the system interpreter: the handshake runs under `env -i`, where a
# version-manager shim would lose the environment it needs to resolve itself.
if [[ -x /usr/bin/python3 ]]; then
  PYTHON3="/usr/bin/python3"
else
  PYTHON3="$(command -v python3)" || fail "python3 is required"
fi

# Reuse the packaging verifier so the bundled copy is checked against the same
# receipt the runtime will check, not just eyeballed for presence.
"$FETCH_TOOL" --verify-only "$ARCH_ROOT" \
  || fail "packaged resources inside $APP failed receipt verification"

assert_bundled_arm64_executable() {
  local label="$1" path="$2" resolved
  [[ -f "$path" ]] || fail "$label executable is missing: $path"
  [[ -x "$path" ]] || fail "$label executable is not executable: $path"
  resolved="$(cd "$(dirname "$path")" && pwd)/$(basename "$path")"
  case "$resolved" in
    "$RESOURCES"/*) ;;
    *) fail "$label executable is not inside Contents/Resources: $resolved" ;;
  esac
  file -b "$path" | grep -q 'Mach-O 64-bit executable arm64' \
    || fail "$label executable is not an arm64 Mach-O executable: $(file -b "$path")"
  [[ "$(lipo -archs "$path")" == "arm64" ]] \
    || fail "$label executable is not arm64-only: $(lipo -archs "$path")"
  log "$label executable is arm64 and bundled: ${resolved#"$APP/"}"
}

NODE="$ARCH_ROOT/node/bin/node"
TYPESCRIPT_CLI="$ARCH_ROOT/typescript/node_modules/typescript-language-server/lib/cli.mjs"
RUST_ANALYZER="$ARCH_ROOT/rust-analyzer/rust-analyzer"

assert_bundled_arm64_executable "node" "$NODE"
assert_bundled_arm64_executable "rust-analyzer" "$RUST_ANALYZER"
[[ -f "$TYPESCRIPT_CLI" ]] || fail "typescript-language-server entrypoint is missing: $TYPESCRIPT_CLI"

WORKSPACE="$(mktemp -d)"
SANDBOX_HOME="$(mktemp -d)"
trap 'rm -rf "$WORKSPACE" "$SANDBOX_HOME"' EXIT

printf 'export const answer = 42;\n' >"$WORKSPACE/index.ts"
printf '{ "compilerOptions": { "strict": true } }\n' >"$WORKSPACE/tsconfig.json"
printf '[package]\nname = "smoke"\nversion = "0.0.0"\nedition = "2021"\n' >"$WORKSPACE/Cargo.toml"
mkdir -p "$WORKSPACE/src"
printf 'fn main() {}\n' >"$WORKSPACE/src/main.rs"

# `env -i` plus a system-only PATH is the whole point: nothing the developer
# installed is reachable, so a server that fell back to a host toolchain cannot
# accidentally pass.
FAILED_SERVERS=()

run_handshake() {
  local label="$1"
  shift
  log "launching $label with PATH=$SYSTEM_PATH"
  if env -i \
    PATH="$SYSTEM_PATH" \
    HOME="$SANDBOX_HOME" \
    TMPDIR="$SANDBOX_HOME" \
    LANG=C \
    "$PYTHON3" "$HANDSHAKE_TOOL" --root "$WORKSPACE" --timeout 90 -- "$@"; then
    log "$label answered initialize and shut down cleanly"
  else
    # Keep going: a smoke test that stops at the first failure hides whether the
    # other server is also broken.
    FAILED_SERVERS+=("$label")
  fi
}

run_handshake "typescript-language-server" "$NODE" "$TYPESCRIPT_CLI" --stdio
run_handshake "rust-analyzer" "$RUST_ANALYZER"

if ((${#FAILED_SERVERS[@]} > 0)); then
  fail "stdio handshake failed for: ${FAILED_SERVERS[*]}"
fi

log "packaged LSP smoke test passed for $APP"

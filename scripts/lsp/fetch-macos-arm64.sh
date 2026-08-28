#!/usr/bin/env bash
#
# Reproducibly stage the bundled macOS arm64 language servers into
# packaging/lsp/dist.
#
# Every upstream artifact is pinned in packaging/lsp/manifest.toml with an
# immutable URL and a SHA-256 that is checked *before* anything is extracted.
# Nothing here is ever run from cargo build, build.rs, or app startup: bundling
# is an explicit packaging step, and the generated tree is git-ignored.
#
# Usage:
#   scripts/lsp/fetch-macos-arm64.sh                       fetch and stage
#   scripts/lsp/fetch-macos-arm64.sh --verify-only <root>   check a staged
#                                                           architecture root
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
PACKAGING_DIR="$REPO_ROOT/packaging/lsp"
MANIFEST_PATH="$PACKAGING_DIR/manifest.toml"
NODE_PROJECT_DIR="$PACKAGING_DIR/node"
DIST_DIR="$PACKAGING_DIR/dist"
RECEIPT_TOOL="$SCRIPT_DIR/receipt.py"

# Relative to the architecture root. Each entry is
# "<artifact-id>|<relative-path>|<executable?>".
REQUIRED_ENTRIES=(
  "node|node/bin/node|yes"
  "rust-analyzer|rust-analyzer/rust-analyzer|yes"
  "typescript-language-server|typescript/node_modules/typescript-language-server/lib/cli.mjs|no"
  "typescript|typescript/node_modules/typescript/lib/version.cjs|no"
  "typescript|typescript/node_modules/@typescript/typescript-darwin-arm64/lib/tsc|yes"
)

log() {
  printf 'lsp-fetch: %s\n' "$1"
}

# Scratch directories removed on exit. Tracked in a global so the EXIT trap
# never reaches for a variable that is local to a function that has returned.
CLEANUP_PATHS=()

cleanup() {
  local path
  for path in ${CLEANUP_PATHS[@]+"${CLEANUP_PATHS[@]}"}; do
    [[ -e "$path" ]] && rm -rf "$path"
  done
  return 0
}

fail() {
  printf 'lsp-fetch: %s\n' "$1" >&2
  exit 1
}

usage() {
  sed -n '3,14p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
}

require_tool() {
  command -v "$1" >/dev/null 2>&1 || fail "required tool not found: $1"
}

# ---------------------------------------------------------------------------
# Manifest
# ---------------------------------------------------------------------------

# Emits "id<TAB>version<TAB>url<TAB>sha256<TAB>license" per [[artifact]] table.
read_manifest() {
  [[ -f "$MANIFEST_PATH" ]] || fail "missing artifact manifest: $MANIFEST_PATH"
  awk '
    function value(line) {
      sub(/^[a-z0-9_]+[ \t]*=[ \t]*/, "", line)
      gsub(/^"|"$/, "", line)
      return line
    }
    function flush() {
      if (id != "") {
        printf "%s\t%s\t%s\t%s\t%s\n", id, version, url, sha256, license
      }
      id = ""; version = ""; url = ""; sha256 = ""; license = ""
    }
    /^\[\[artifact\]\]/ { flush(); next }
    /^id[ \t]*=/        { id = value($0); next }
    /^version[ \t]*=/   { version = value($0); next }
    /^url[ \t]*=/       { url = value($0); next }
    /^sha256[ \t]*=/    { sha256 = value($0); next }
    /^license[ \t]*=/   { license = value($0); next }
    END { flush() }
  ' "$MANIFEST_PATH"
}

manifest_field() {
  local wanted_id="$1" column="$2"
  read_manifest | awk -F '\t' -v id="$wanted_id" -v column="$column" \
    '$1 == id { print $column; found = 1 } END { exit found ? 0 : 1 }' \
    || fail "artifact not declared in manifest.toml: $wanted_id"
}

# Emits the --artifact id=version flags that pin the receipt to the manifest.
artifact_pin_flags() {
  read_manifest | awk -F '\t' '{ printf "--artifact\n%s=%s\n", $1, $2 }'
}

# ---------------------------------------------------------------------------
# Verification
# ---------------------------------------------------------------------------

verify_only() {
  local arch_root="$1"
  local problems=0

  require_tool file
  require_tool lipo
  require_tool codesign
  require_tool python3

  if [[ ! -d "$arch_root" ]]; then
    fail "not a staged architecture root: $arch_root"
  fi
  arch_root="$(cd "$arch_root" && pwd)"

  local lsp_root
  lsp_root="$(dirname "$arch_root")"

  local entry artifact_id relative_path executable full_path
  for entry in "${REQUIRED_ENTRIES[@]}"; do
    IFS='|' read -r artifact_id relative_path executable <<<"$entry"
    full_path="$arch_root/$relative_path"
    if [[ -L "$full_path" ]]; then
      printf 'lsp-fetch: %s artifact is a symlink: %s\n' "$artifact_id" "$relative_path" >&2
      problems=$((problems + 1))
      continue
    fi
    if [[ ! -f "$full_path" ]]; then
      printf 'lsp-fetch: missing %s artifact: %s\n' "$artifact_id" "$relative_path" >&2
      problems=$((problems + 1))
      continue
    fi
    if [[ ! -s "$full_path" ]]; then
      printf 'lsp-fetch: empty %s artifact: %s\n' "$artifact_id" "$relative_path" >&2
      problems=$((problems + 1))
      continue
    fi
    if [[ "$executable" == "yes" ]]; then
      if [[ ! -x "$full_path" ]]; then
        printf 'lsp-fetch: %s artifact is not executable: %s\n' \
          "$artifact_id" "$relative_path" >&2
        problems=$((problems + 1))
        continue
      fi
      if ! file -b "$full_path" | grep -q 'Mach-O 64-bit executable arm64'; then
        printf 'lsp-fetch: %s artifact is not an arm64 Mach-O executable: %s (%s)\n' \
          "$artifact_id" "$relative_path" "$(file -b "$full_path")" >&2
        problems=$((problems + 1))
        continue
      fi
      if [[ "$(lipo -archs "$full_path" 2>/dev/null)" != "arm64" ]]; then
        printf 'lsp-fetch: %s artifact is not arm64-only: %s (%s)\n' \
          "$artifact_id" "$relative_path" "$(lipo -archs "$full_path" 2>&1)" >&2
        problems=$((problems + 1))
        continue
      fi
      if ! codesign --verify --strict "$full_path" >/dev/null 2>&1; then
        printf 'lsp-fetch: %s artifact has no valid code signature: %s\n' \
          "$artifact_id" "$relative_path" >&2
        problems=$((problems + 1))
      fi
    fi
  done

  if [[ ! -f "$lsp_root/THIRD_PARTY_NOTICES.md" ]]; then
    printf 'lsp-fetch: missing third-party notices: %s\n' \
      "$lsp_root/THIRD_PARTY_NOTICES.md" >&2
    problems=$((problems + 1))
  fi

  if ((problems > 0)); then
    fail "$problems problem(s) found under $arch_root"
  fi

  local pin_flags=()
  while IFS= read -r flag; do
    pin_flags+=("$flag")
  done < <(artifact_pin_flags)

  python3 "$RECEIPT_TOOL" verify \
    --arch-root "$arch_root" \
    --receipt "$lsp_root/manifest.json" \
    "${pin_flags[@]}" \
    || fail "receipt verification failed for $arch_root"

  log "verified staged resources at $arch_root"
}

# ---------------------------------------------------------------------------
# Download and staging
# ---------------------------------------------------------------------------

download_and_verify() {
  local artifact_id="$1" destination="$2"
  local url expected actual
  url="$(manifest_field "$artifact_id" 3)"
  expected="$(manifest_field "$artifact_id" 4)"

  log "downloading $artifact_id from $url"
  curl --fail --location --silent --show-error \
    --proto '=https' --tlsv1.2 \
    --retry 3 --retry-delay 2 \
    --output "$destination" "$url" \
    || fail "download failed for $artifact_id ($url)"

  actual="$(shasum -a 256 "$destination" | awk '{ print $1 }')"
  if [[ "$actual" != "$expected" ]]; then
    fail "SHA-256 mismatch for $artifact_id ($url): expected $expected, got $actual"
  fi
  log "verified $artifact_id sha256 $actual"
}

stage_node() {
  local archive="$1" stage="$2" work="$3"
  local extracted
  mkdir -p "$work/node"
  tar -xzf "$archive" -C "$work/node"
  extracted="$(find "$work/node" -mindepth 1 -maxdepth 1 -type d | head -1)"
  [[ -n "$extracted" ]] || fail "node archive did not contain a top-level directory"
  mkdir -p "$stage/node/bin"
  cp "$extracted/bin/node" "$stage/node/bin/node"
  cp "$extracted/LICENSE" "$stage/node/LICENSE"
  chmod 0755 "$stage/node/bin/node"
  log "staged node runtime"
}

stage_rust_analyzer() {
  local archive="$1" stage="$2"
  mkdir -p "$stage/rust-analyzer"
  gunzip -c "$archive" >"$stage/rust-analyzer/rust-analyzer"
  chmod 0755 "$stage/rust-analyzer/rust-analyzer"
  log "staged rust-analyzer"
}

stage_typescript() {
  local stage="$1" work="$2"
  local npm_dir="$work/npm"
  mkdir -p "$npm_dir"
  cp "$NODE_PROJECT_DIR/package.json" "$NODE_PROJECT_DIR/package-lock.json" "$npm_dir/"
  (
    cd "$npm_dir"
    npm ci --omit=dev --ignore-scripts >/dev/null
  ) || fail "npm ci failed against the committed lockfile"

  local installed_tls installed_ts
  installed_tls="$(node_package_version "$npm_dir/node_modules/typescript-language-server/package.json")"
  installed_ts="$(node_package_version "$npm_dir/node_modules/typescript/package.json")"
  [[ "$installed_tls" == "$(manifest_field typescript-language-server 2)" ]] \
    || fail "npm ci installed typescript-language-server $installed_tls, manifest pins $(manifest_field typescript-language-server 2)"
  [[ "$installed_ts" == "$(manifest_field typescript 2)" ]] \
    || fail "npm ci installed typescript $installed_ts, manifest pins $(manifest_field typescript 2)"

  mkdir -p "$stage/typescript"
  mv "$npm_dir/node_modules" "$stage/typescript/node_modules"
  log "staged typescript-language-server $installed_tls with typescript $installed_ts"
}

node_package_version() {
  python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["version"])' "$1"
}

# Drop everything the runtime never opens: npm shims, sourcemaps, prose docs,
# and any archive that survived extraction. License and notice texts stay so
# THIRD_PARTY_NOTICES.md can quote what actually ships.
strip_staging() {
  local stage="$1"
  find "$stage" -type d \
    \( -name '.bin' -o -name '.github' -o -name 'doc' -o -name 'docs' -o -name 'man' \) \
    -prune -exec rm -rf {} +
  find "$stage" -type f \
    \( -name '*.map' -o -name '.npmignore' -o -name '.npmrc' -o -name '.package-lock.json' \) \
    -delete
  find "$stage" -type f \
    \( -name '*.md' -o -name '*.markdown' \) \
    ! -iname 'licen*' ! -iname 'notice*' ! -iname 'copying*' \
    -delete
  find "$stage" -type f \
    \( -name '*.tgz' -o -name '*.tar' -o -name '*.tar.gz' -o -name '*.gz' -o -name '*.zip' \) \
    -delete
  find "$stage" -type l -delete
  find "$stage" -type d -empty -delete
  log "stripped archives, caches, docs, and npm shims"
}

# Nested Mach-O executables must carry a valid signature before the outer app is
# signed, and their bytes must be final before the receipt records them — the
# runtime rejects any file whose SHA-256 drifted. Node and the TypeScript native
# compiler already ship Developer ID signatures and rust-analyzer ships an
# ad-hoc one, so this normally verifies and changes nothing; anything unsigned
# is ad-hoc signed here, where the receipt is still ahead of it.
sign_nested_executables() {
  local stage="$1" path signed=0 adhoc=0
  while IFS= read -r path; do
    file -b "$path" | grep -q 'Mach-O' || continue
    if codesign --verify --strict "$path" >/dev/null 2>&1; then
      signed=$((signed + 1))
      continue
    fi
    codesign --force --sign - "$path" \
      || fail "could not ad-hoc sign nested executable: ${path#"$stage/"}"
    codesign --verify --strict "$path" >/dev/null 2>&1 \
      || fail "nested executable is still unsigned after ad-hoc signing: ${path#"$stage/"}"
    adhoc=$((adhoc + 1))
  done < <(find "$stage" -type f -perm -u+x)
  log "nested executables: $signed already signed, $adhoc ad-hoc signed here"
}

write_notices() {
  local stage_parent="$1" stage="$2"
  local notices="$stage_parent/THIRD_PARTY_NOTICES.md"
  {
    printf '# Third-Party Notices\n\n'
    printf 'TermLab bundles the language servers and runtimes below. Each was\n'
    printf 'fetched from the pinned URL in `packaging/lsp/manifest.toml` and\n'
    printf 'verified by SHA-256 before staging.\n\n'
    local id version url sha256 license
    while IFS=$'\t' read -r id version url sha256 license; do
      printf '## %s %s\n\n' "$id" "$version"
      printf -- '- License: %s\n' "$license"
      printf -- '- Source: %s\n' "$url"
      printf -- '- SHA-256: `%s`\n\n' "$sha256"
    done < <(read_manifest)
    printf '## Included license texts\n\n'
    local license_file relative
    while IFS= read -r license_file; do
      relative="${license_file#"$stage/"}"
      printf '### `arm64/%s`\n\n```\n' "$relative"
      cat "$license_file"
      printf '\n```\n\n'
    done < <(find "$stage" -type f \
      \( -iname 'LICENSE' -o -iname 'LICENSE.txt' -o -iname 'NOTICE.txt' \) | sort)
  } >"$notices"
  log "wrote $(basename "$notices")"
}

fetch_and_stage() {
  require_tool curl
  require_tool shasum
  require_tool tar
  require_tool gunzip
  require_tool npm
  require_tool python3

  [[ "$(uname -s)" == "Darwin" ]] || fail "macOS-only packaging step (this is $(uname -s))"
  [[ "$(uname -m)" == "arm64" ]] \
    || fail "arm64-only packaging step (this host is $(uname -m)); use --verify-only to check an existing tree"

  local work downloads stage_parent stage
  trap cleanup EXIT
  work="$(mktemp -d)"
  CLEANUP_PATHS+=("$work")
  downloads="$work/downloads"
  mkdir -p "$downloads"

  # Stage inside packaging/lsp so the final replacement is a same-filesystem
  # rename rather than a copy that can be interrupted halfway.
  mkdir -p "$PACKAGING_DIR"
  stage_parent="$(mktemp -d "$PACKAGING_DIR/.staging.XXXXXX")"
  CLEANUP_PATHS+=("$stage_parent")
  stage="$stage_parent/arm64"
  mkdir -p "$stage"

  download_and_verify node "$downloads/node.tar.gz"
  download_and_verify rust-analyzer "$downloads/rust-analyzer.gz"
  download_and_verify typescript-language-server "$downloads/typescript-language-server.tgz"
  download_and_verify typescript "$downloads/typescript.tgz"

  stage_node "$downloads/node.tar.gz" "$stage" "$work"
  stage_rust_analyzer "$downloads/rust-analyzer.gz" "$stage"
  stage_typescript "$stage" "$work"
  strip_staging "$stage"
  sign_nested_executables "$stage"
  write_notices "$stage_parent" "$stage"

  local pin_flags=()
  while IFS= read -r flag; do
    pin_flags+=("$flag")
  done < <(artifact_pin_flags)
  # tauri.conf.json declares packaging/lsp/dist as a bundle resource, and
  # tauri-build refuses to compile when that path is missing. The placeholder is
  # tracked so a clean checkout builds before anyone runs this script, and it is
  # re-created here because the swap below replaces the whole directory.
  touch "$stage_parent/.gitkeep"

  python3 "$RECEIPT_TOOL" generate \
    --arch-root "$stage" \
    --output "$stage_parent/manifest.json" \
    "${pin_flags[@]}" \
    || fail "receipt generation failed"

  verify_only "$stage"

  # Replace packaging/lsp/dist only now that every check has passed: move the
  # old tree aside, rename the verified staging directory into place, then drop
  # the old tree. A crash mid-swap leaves a .previous.* directory behind rather
  # than a half-written dist/.
  local previous
  previous="$(mktemp -d "$PACKAGING_DIR/.previous.XXXXXX")"
  CLEANUP_PATHS+=("$previous")
  if [[ -d "$DIST_DIR" ]]; then
    mv "$DIST_DIR" "$previous/dist"
  fi
  mv "$stage_parent" "$DIST_DIR"
  # mktemp -d creates 0700; the tree is copied into an app bundle read by every
  # user of the machine.
  chmod 0755 "$DIST_DIR"

  log "staged bundled servers at $DIST_DIR"
}

main() {
  case "${1:-}" in
    --verify-only)
      [[ $# -eq 2 ]] || fail "usage: $(basename "${BASH_SOURCE[0]}") --verify-only <resource-root>"
      verify_only "$2"
      ;;
    -h | --help)
      usage
      ;;
    "")
      fetch_and_stage
      ;;
    *)
      usage >&2
      fail "unknown argument: $1"
      ;;
  esac
}

main "$@"

#!/bin/bash
#
# Generate a release changelog with:
#   1. An AI-generated summary (via Claude API)
#   2. A list of merged PRs with links
#
# Usage: generate_changelog.sh [PREVIOUS_TAG]
#
# Environment:
#   ANTHROPIC_API_KEY  — required for AI summary (skipped if unset)
#   GITHUB_REPOSITORY  — e.g. "an0nn30/rusty_conch" (auto-set in Actions)

set -euo pipefail

REPO="${GITHUB_REPOSITORY:-an0nn30/rusty_conch}"
CURRENT_TAG="$(git describe --tags --abbrev=0 2>/dev/null || true)"

# Determine previous tag.
if [ -n "${1:-}" ]; then
    PREV_TAG="$1"
else
    # Second-most-recent tag.
    PREV_TAG="$(git tag --sort=-v:refname | grep -E '^v[0-9]' | sed -n '2p' || true)"
fi

if [ -z "$PREV_TAG" ]; then
    echo "No previous tag found — generating changelog from all history." >&2
    RANGE=""
    RANGE_DISPLAY="the beginning"
else
    RANGE="${PREV_TAG}..HEAD"
    RANGE_DISPLAY="$PREV_TAG"
fi

echo "Generating changelog: ${RANGE_DISPLAY} → ${CURRENT_TAG:-HEAD}" >&2

# ---------------------------------------------------------------------------
# 1. Collect merged PRs since the previous tag
# ---------------------------------------------------------------------------
# Get merge commits in range, extract PR numbers.
if [ -n "$RANGE" ]; then
    MERGE_COMMITS=$(git log "$RANGE" --merges --oneline 2>/dev/null || true)
else
    MERGE_COMMITS=$(git log --merges --oneline 2>/dev/null || true)
fi

# Also get non-merge commits (direct pushes to main).
if [ -n "$RANGE" ]; then
    DIRECT_COMMITS=$(git log "$RANGE" --no-merges --oneline 2>/dev/null || true)
else
    DIRECT_COMMITS=$(git log --no-merges --oneline 2>/dev/null || true)
fi

# Extract PR numbers from merge commit messages like "Merge pull request #7"
PR_NUMBERS=$(echo "$MERGE_COMMITS" | grep -oE '#[0-9]+' | tr -d '#' | sort -un || true)

PR_LIST=""
if [ -n "$PR_NUMBERS" ]; then
    while IFS= read -r pr_num; do
        [ -z "$pr_num" ] && continue
        # Fetch PR title using gh CLI.
        pr_title=$(gh pr view "$pr_num" --repo "$REPO" --json title -q '.title' 2>/dev/null || echo "PR #${pr_num}")
        PR_LIST="${PR_LIST}- ${pr_title} ([#${pr_num}](https://github.com/${REPO}/pull/${pr_num}))"$'\n'
    done <<< "$PR_NUMBERS"
fi

# Collect direct (non-PR) commits for context.
COMMIT_LIST=""
if [ -n "$DIRECT_COMMITS" ]; then
    while IFS= read -r line; do
        [ -z "$line" ] && continue
        # Strip the short hash prefix.
        msg="${line#* }"
        # Skip release/merge commits.
        case "$msg" in
            release:*|Merge*) continue ;;
        esac
        COMMIT_LIST="${COMMIT_LIST}- ${msg}"$'\n'
    done <<< "$DIRECT_COMMITS"
fi

# ---------------------------------------------------------------------------
# 2. Generate AI summary (optional — skipped if no API key)
# ---------------------------------------------------------------------------
ALL_CHANGES="${PR_LIST}${COMMIT_LIST}"
AI_SUMMARY=""

if [ -n "${ANTHROPIC_API_KEY:-}" ] && [ -n "$ALL_CHANGES" ]; then
    echo "Generating AI summary..." >&2

    # Build the prompt.
    PROMPT="You are writing release notes for Conch, an open-source terminal emulator and SSH manager built with Rust and egui. Given the following list of changes since the last release, write a concise, user-friendly summary in 2-4 bullet points. Focus on what matters to users (new features, bug fixes, improvements). Do not use markdown headers — just bullet points. Do not mention PR numbers.

Changes:
${ALL_CHANGES}"

    # Escape for JSON.
    PROMPT_JSON=$(printf '%s' "$PROMPT" | python3 -c 'import sys,json; print(json.dumps(sys.stdin.read()))')

    RESPONSE=$(curl -s --max-time 30 \
        https://api.anthropic.com/v1/messages \
        -H "Content-Type: application/json" \
        -H "x-api-key: ${ANTHROPIC_API_KEY}" \
        -H "anthropic-version: 2023-06-01" \
        -d "{
            \"model\": \"claude-haiku-4-5-20251001\",
            \"max_tokens\": 512,
            \"messages\": [{\"role\": \"user\", \"content\": ${PROMPT_JSON}}]
        }" 2>/dev/null || true)

    if [ -n "$RESPONSE" ]; then
        AI_SUMMARY=$(echo "$RESPONSE" | python3 -c '
import sys, json
try:
    data = json.load(sys.stdin)
    print(data["content"][0]["text"])
except Exception:
    pass
' 2>/dev/null || true)
    fi

    if [ -z "$AI_SUMMARY" ]; then
        echo "Warning: AI summary generation failed, skipping." >&2
    fi
fi

# ---------------------------------------------------------------------------
# 3. Assemble the changelog
# ---------------------------------------------------------------------------
CHANGELOG=""

if [ -n "$AI_SUMMARY" ]; then
    CHANGELOG="## What's New\n\n${AI_SUMMARY}\n\n"
fi

if [ -n "$PR_LIST" ]; then
    CHANGELOG="${CHANGELOG}## Pull Requests\n\n${PR_LIST}\n"
fi

if [ -n "$COMMIT_LIST" ]; then
    CHANGELOG="${CHANGELOG}## Other Changes\n\n${COMMIT_LIST}\n"
fi

if [ -z "$CHANGELOG" ]; then
    CHANGELOG="No notable changes since ${RANGE_DISPLAY}.\n"
fi

# Print raw (with literal \n) for consumption by the caller.
printf '%b' "$CHANGELOG"

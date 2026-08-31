# TermLab — Agent Instructions

All AI coding agents working in this repository — Codex, Claude, and anything else —
follow the working agreement in **[CLAUDE.md](CLAUDE.md)**. Read it before making changes.
It is the single source of truth for architecture, commands, style, and workflow.

This file exists because Codex-style agents look for `AGENTS.md`. It intentionally stays
short so it cannot drift out of sync with CLAUDE.md.

## Non-negotiables

- **Never commit or push directly to `main`.** Every change goes on its own branch:
  `feat/` (features), `fix/` (bugs), `chore/` (docs, tooling, cleanup), `perf/` (performance).
- **Behavior changes require tests.** Add `#[cfg(test)] mod tests` alongside the code.
- **No monoliths.** Extract new functionality into focused modules rather than appending
  to large files.
- **Never `--force` push.** Never open PRs unless explicitly asked.
- **No Co-Authored-By lines** in commits. Imperative mood, one concern per branch.

## Verify before claiming done

```bash
cargo test --workspace
cargo clippy --all-targets
cargo fmt -- --check
```

## Orientation

TermLab is a Rust + Tauri v2 terminal workstation: terminal emulation, SSH/SFTP,
tunnels, an encrypted credential vault, a light editor, and a Lua/Java plugin system.
Seven crates under `crates/`, with a no-bundler JavaScript frontend in
`crates/termlab_tauri/frontend/`. See [CLAUDE.md](CLAUDE.md) for the full map and
`docs/` for plugin and design documentation.

Looking for the front-end architecture/UX auditor persona that used to live here?
It moved to [docs/frontend-audit-agent.md](docs/frontend-audit-agent.md).

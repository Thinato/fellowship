# Agent Guidelines

This file is the source of truth for AI coding agents working in this repo. `CLAUDE.md` is a symlink to this file.

## Project

Rust ratatui TUI ("tmux for agents"). 3 panes: workspaces (left, git worktrees), terminal (middle, embedded PTY), git status (right, diff + PR). Per-worktree shells kept alive on switch. `Ctrl+a` prefix keymap.

## Required workflow after every edit

Run **all four** commands in order before declaring a change complete:

```bash
cargo check
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
```

`cargo check` for fast type errors. `cargo fmt` to satisfy CI. `cargo clippy -D warnings` is the project lint gate (matches CI). `cargo test` to confirm no regression. **All four must pass.** No skipping. No `--no-verify`.

If any step fails, fix the root cause before continuing — do not paper over with `#[allow(...)]` unless the suppression is explicitly justified in a comment.

## Code conventions

- Rust 2024 edition.
- Errors propagate via `anyhow::Result` for app code; `thiserror` for library-style typed errors when needed.
- Keep the `[PaneId]` enum and pane modules in sync (`src/panes/{workspaces,terminal,gitstatus}.rs`).
- Spatial layout for focus motion lives in `src/layout.rs`. The visual `ui::render` is currently hardcoded to a 3-column horizontal split — keep them consistent if the visual layout ever changes.
- Keymap state machine in `src/keymap.rs`. Prefix is `Ctrl+a`, no timeout, `Esc` cancels prefix mode.
- Per-worktree terminals live in `App.terminals: HashMap<PathBuf, TerminalPane>`. Never kill on switch — only on quit.
- Worktrees go on disk at `$HOME/.fellowship/worktrees/<owner>/<repo>/<branch>` with branch slashes preserved. Logic in `git::worktree_path_for`.
- Config: `$HOME/.config/fellowship/config.toml` (global) merged with `<repo>/.fellowship/config.toml` (local). Local wins per field.

## Editing files

- **Always read a file before its first edit in the current session.** This avoids editing against stale assumptions and catches recent unrelated changes you'd otherwise clobber. Subsequent edits to the same file in the same loop don't need a re-read — the Edit tool will reject obviously stale state.
- Match existing indent / brace / import-grouping style of the file you're editing. Don't re-format unrelated lines.

## Pull requests

- **Direct pushes to `master` are blocked.** All changes land via PR.
- **Bump `Cargo.toml` version before opening a PR**, even for tiny changes. The version field is the contract install.sh + the release tarball depend on; treating it as load-bearing per-PR avoids the "we shipped agentic-ui under v0.1.0" bug class. Use semver: bug fix → patch, new feature → minor, breaking change → major. `Cargo.lock` updates with `cargo check` after the bump.
- After merge, tagging the new version (`git tag vX.Y.Z && git push origin vX.Y.Z`) triggers `.github/workflows/release.yml` to build the three-binary tarball.

## Don't

- Don't add new top-level modules without a clear need; prefer extending existing ones.
- Don't introduce dependencies without checking `Cargo.toml` for an existing crate that already covers the use case.
- Don't write integration tests that shell out to a fresh `git init` repo unless they use `tempfile` and clean up.
- Don't commit `target/` or `*.original.*` backup files.
- Don't add comments that just restate the code. Keep comments to non-obvious "why".

<!-- BEGIN BEADS INTEGRATION v:1 profile:minimal hash:ca08a54f -->
## Beads Issue Tracker

This project uses **bd (beads)** for issue tracking. Run `bd prime` to see full workflow context and commands.

### Quick Reference

```bash
bd ready              # Find available work
bd show <id>          # View issue details
bd update <id> --claim  # Claim work
bd close <id>         # Complete work
```

### Rules

- Use `bd` for ALL task tracking — do NOT use TodoWrite, TaskCreate, or markdown TODO lists
- Run `bd prime` for detailed command reference and session close protocol
- Use `bd remember` for persistent knowledge — do NOT use MEMORY.md files

## Session Completion

**When ending a work session**, you MUST complete ALL steps below. Work is NOT complete until `git push` succeeds.

**MANDATORY WORKFLOW:**

1. **File issues for remaining work** - Create issues for anything that needs follow-up
2. **Run quality gates** (if code changed) - Tests, linters, builds
3. **Update issue status** - Close finished work, update in-progress items
4. **PUSH TO REMOTE** - This is MANDATORY:
   ```bash
   git pull --rebase
   bd dolt push
   git push
   git status  # MUST show "up to date with origin"
   ```
5. **Clean up** - Clear stashes, prune remote branches
6. **Verify** - All changes committed AND pushed
7. **Hand off** - Provide context for next session

**CRITICAL RULES:**
- Work is NOT complete until `git push` succeeds
- NEVER stop before pushing - that leaves work stranded locally
- NEVER say "ready to push when you are" - YOU must push
- If push fails, resolve and retry until it succeeds
<!-- END BEADS INTEGRATION -->

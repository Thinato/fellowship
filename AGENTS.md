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

## Don't

- Don't add new top-level modules without a clear need; prefer extending existing ones.
- Don't introduce dependencies without checking `Cargo.toml` for an existing crate that already covers the use case.
- Don't write integration tests that shell out to a fresh `git init` repo unless they use `tempfile` and clean up.
- Don't commit `target/` or `*.original.*` backup files.
- Don't add comments that just restate the code. Keep comments to non-obvious "why".

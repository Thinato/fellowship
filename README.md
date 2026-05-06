# fellowship

A terminal UI for working with multiple git worktrees side by side — tmux for agents.

Three panes:

- **Left** — workspaces: every worktree of the current repo, plus a modal to create new ones.
- **Middle** — a real PTY shell, one per worktree, kept alive across switches. Run `claude`, `vim`, `htop`, anything.
- **Right** — git status: diff summary, untracked files, and the current branch's PR (via `gh`).

## Install

### Prebuilt binary (recommended)

```bash
curl --proto '=https' --tlsv1.2 -sSf \
  https://raw.githubusercontent.com/Thinato/fellowship/master/install.sh | sh
```

Installs to `~/.local/bin` by default. Override with `FELLOWSHIP_INSTALL_DIR=/usr/local/bin` or pin a tag with `FELLOWSHIP_VERSION=v0.1.0`.

Supported targets: Linux x86_64 / aarch64, macOS aarch64.

### From source

```bash
cargo install --path .
# or
cargo build --release && cp target/release/fellowship ~/.local/bin/
```

Requires `git` on `PATH`. `gh` is optional (PR pane shows "no PR" if missing or unauthed).

## Run

```bash
fellowship           # opens current directory
fellowship /path/to/repo
```

## Keybindings

Prefix is `Ctrl+a` (tmux-style; persists until follower or `Esc`). Inside the terminal pane every other key is forwarded to the PTY.

| Combo | Action |
|---|---|
| `Ctrl+a e` / `t` / `g` | Focus workspaces / terminal / git status |
| `Ctrl+a h` / `j` / `k` / `l` | Focus pane left / down / up / right |
| `Ctrl+a 1`..`9` | Switch to worktree by index |
| `Ctrl+a q` | Quit |
| `Ctrl+a ?` | Toggle help overlay |
| `Ctrl+a Ctrl+a` | Send literal `^A` to PTY (e.g. for shell `beginning-of-line`) |
| `Ctrl+a Esc` | Cancel prefix |
| `j` / `k` | Navigate workspaces list (when focused) |
| `n` | New worktree (workspaces pane) |
| `Enter` | Switch to selected worktree |

## Worktree layout on disk

New worktrees are created at:

```
$HOME/.fellowship/worktrees/<owner>/<repo>/<branch>
```

Branch slashes are preserved (`feat/x` → `.../<repo>/feat/x`). Owner/repo come from `remote.origin.url`; falls back to `local/<dir-name>` when no remote is set.

## Config

Two files, merged with **local taking precedence over global** per field:

- Global: `$HOME/.config/fellowship/config.toml`
- Local: `<repo>/.fellowship/config.toml`

Fields:

```toml
# Run this in every new worktree shell as soon as it spawns.
shell_startup_command = "claude"
```

## Build

```bash
cargo build
cargo test
cargo clippy -- -D warnings
```

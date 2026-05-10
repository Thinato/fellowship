//! Policy for the `safe-git` shim plus install helpers.
//!
//! Pure data-in / decision-out logic so the binary entry point in
//! `src/bin/safe-git.rs` stays trivial and the policy itself can be
//! exhaustively unit-tested.
//!
//! See `docs/plans/agentic-ui-v1.md` §3.7.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Git,
    Gh,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Allow,
    Block(String),
}

pub fn detect_tool(argv0: &str) -> Tool {
    let name = Path::new(argv0)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(argv0);
    match name {
        "git" => Tool::Git,
        "gh" => Tool::Gh,
        _ => Tool::Other,
    }
}

pub fn decide(tool: Tool, args: &[String]) -> Decision {
    match tool {
        Tool::Git => decide_git(args),
        Tool::Gh => decide_gh(args),
        Tool::Other => Decision::Allow,
    }
}

/// Walk past `git`'s leading global options (`-C <path>`, `-c k=v`,
/// `--git-dir <path>`, `--work-tree <path>`, `--exec-path[=...]`, etc.) to
/// find the subcommand. Conservative: unknown flags consume one arg.
fn first_subcommand(args: &[String]) -> Option<(usize, &String)> {
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        if a.starts_with("--") {
            // Long form. If it carries a value as a separate arg, consume it.
            // Heuristic: skip args matching known value-bearing flags.
            if matches!(
                a.as_str(),
                "--git-dir" | "--work-tree" | "--namespace" | "--super-prefix"
            ) {
                i += 2;
                continue;
            }
            // `--git-dir=...` / `--work-tree=...` etc. self-contained.
            i += 1;
            continue;
        }
        if let Some(rest) = a.strip_prefix('-') {
            // Short flag block. Some short opts take an argument.
            if rest == "C" || rest == "c" {
                i += 2;
                continue;
            }
            i += 1;
            continue;
        }
        return Some((i, a));
    }
    None
}

fn decide_git(args: &[String]) -> Decision {
    let Some((sub_idx, sub)) = first_subcommand(args) else {
        return Decision::Allow;
    };
    match sub.as_str() {
        "merge" => {
            Decision::Block("agents must not run `git merge` — humans review and merge PRs.".into())
        }
        "push" => decide_git_push(&args[sub_idx + 1..]),
        _ => Decision::Allow,
    }
}

fn decide_git_push(push_args: &[String]) -> Decision {
    if push_args
        .iter()
        .any(|a| matches!(a.as_str(), "--force" | "-f" | "--force-with-lease"))
    {
        return Decision::Block("agents must not force-push — pushes are append-only.".into());
    }
    let positional: Vec<&String> = push_args.iter().filter(|a| !a.starts_with('-')).collect();
    if let Some(refspec) = positional.get(1) {
        let target = refspec.split(':').next_back().unwrap_or(refspec.as_str());
        let bare = target.strip_prefix("refs/heads/").unwrap_or(target);
        if matches!(bare, "main" | "master") {
            return Decision::Block(format!(
                "agents must not push directly to `{bare}` — open a PR from a feature branch."
            ));
        }
    }
    Decision::Allow
}

fn decide_gh(args: &[String]) -> Decision {
    let Some((sub_idx, sub)) = first_subcommand(args) else {
        return Decision::Allow;
    };
    if sub != "pr" {
        return Decision::Allow;
    }
    // Look for the next non-flag arg AFTER `pr`.
    let after_pr = &args[sub_idx + 1..];
    let pr_sub = after_pr.iter().find(|a| !a.starts_with('-'));
    if pr_sub.map(|s| s.as_str()) == Some("merge") {
        return Decision::Block(
            "agents must not run `gh pr merge` — humans review and merge PRs.".into(),
        );
    }
    Decision::Allow
}

/// Path to the directory holding the `git` and `gh` shim symlinks.
/// Lives at `~/.fellowship/bin/`.
pub fn shim_dir() -> Result<PathBuf> {
    let home =
        std::env::var_os("HOME").ok_or_else(|| anyhow::anyhow!("HOME env var is not set"))?;
    Ok(PathBuf::from(home).join(".fellowship").join("bin"))
}

/// Locate the `safe-git` binary alongside the running fellowship binary.
/// Both are produced by the same cargo build, so they live in the same dir.
pub fn locate_safe_git() -> Result<PathBuf> {
    let exe = std::env::current_exe().context("reading current_exe")?;
    let dir = exe
        .parent()
        .ok_or_else(|| anyhow::anyhow!("current_exe has no parent dir: {}", exe.display()))?;
    let candidate = dir.join("safe-git");
    if !candidate.exists() {
        anyhow::bail!(
            "safe-git not found next to fellowship at {}. \
             Build with `cargo build` (or `cargo install --path .`) so both \
             binaries land in the same directory.",
            candidate.display()
        );
    }
    Ok(candidate)
}

/// Create or refresh `<shim_dir>/git` and `<shim_dir>/gh` symlinks pointing
/// at `safe_git`. Idempotent: if the symlinks already point at the right
/// target, nothing changes. If they exist with the wrong target, they are
/// replaced.
pub fn install_shims(shim_dir: &Path, safe_git: &Path) -> Result<()> {
    std::fs::create_dir_all(shim_dir)
        .with_context(|| format!("mkdir -p {}", shim_dir.display()))?;
    for name in ["git", "gh"] {
        let link = shim_dir.join(name);
        if let Ok(existing) = std::fs::read_link(&link) {
            if existing == safe_git {
                continue;
            }
            std::fs::remove_file(&link)
                .with_context(|| format!("remove stale symlink {}", link.display()))?;
        } else if link.exists() {
            // Non-symlink at the path — refuse to clobber.
            anyhow::bail!(
                "{} exists but is not a symlink; remove it manually so fellowship can install the shim",
                link.display()
            );
        }
        #[cfg(unix)]
        std::os::unix::fs::symlink(safe_git, &link)
            .with_context(|| format!("symlink {} -> {}", link.display(), safe_git.display()))?;
        #[cfg(not(unix))]
        anyhow::bail!("safe-git shim only supported on Unix");
    }
    Ok(())
}

/// Build a PATH value with `shim_dir` prepended to the current process PATH.
pub fn path_with_shim_prepended(shim_dir: &Path) -> Result<std::ffi::OsString> {
    let current = std::env::var_os("PATH").unwrap_or_default();
    let mut entries: Vec<PathBuf> = vec![shim_dir.to_path_buf()];
    entries.extend(std::env::split_paths(&current));
    let joined = std::env::join_paths(entries).context("composing shim-prefixed PATH")?;
    Ok(joined)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn detect_tool_recognizes_git_and_gh_via_basename() {
        assert_eq!(detect_tool("git"), Tool::Git);
        assert_eq!(detect_tool("/usr/bin/git"), Tool::Git);
        assert_eq!(detect_tool("/home/u/.fellowship/bin/gh"), Tool::Gh);
        assert_eq!(detect_tool("ls"), Tool::Other);
    }

    #[test]
    fn git_merge_is_blocked() {
        assert!(matches!(
            decide(Tool::Git, &args(&["merge", "feat/x"])),
            Decision::Block(_)
        ));
        assert!(matches!(
            decide(Tool::Git, &args(&["-C", "/tmp", "merge", "feat/x"])),
            Decision::Block(_)
        ));
    }

    #[test]
    fn git_push_force_is_blocked() {
        for flag in ["--force", "-f", "--force-with-lease"] {
            let r = decide(Tool::Git, &args(&["push", flag, "origin", "feat/x"]));
            assert!(matches!(r, Decision::Block(_)), "{flag} should be blocked");
        }
    }

    #[test]
    fn git_push_to_main_or_master_is_blocked() {
        for branch in ["main", "master", "refs/heads/main", "refs/heads/master"] {
            let r = decide(Tool::Git, &args(&["push", "origin", branch]));
            assert!(matches!(r, Decision::Block(_)), "push to {branch}");
        }
        // HEAD:main also blocked.
        let r = decide(Tool::Git, &args(&["push", "origin", "HEAD:main"]));
        assert!(matches!(r, Decision::Block(_)));
    }

    #[test]
    fn git_push_feature_branch_is_allowed() {
        let r = decide(Tool::Git, &args(&["push", "origin", "feat/bead-1"]));
        assert_eq!(r, Decision::Allow);
        let r = decide(Tool::Git, &args(&["push", "-u", "origin", "feat/bead-1"]));
        assert_eq!(r, Decision::Allow);
    }

    #[test]
    fn git_unrelated_subcommands_pass_through() {
        for cmd in ["status", "diff", "log", "commit", "fetch", "pull", "rebase"] {
            assert_eq!(decide(Tool::Git, &args(&[cmd])), Decision::Allow);
        }
    }

    #[test]
    fn gh_pr_merge_is_blocked() {
        let r = decide(Tool::Gh, &args(&["pr", "merge", "42"]));
        assert!(matches!(r, Decision::Block(_)));
        let r = decide(Tool::Gh, &args(&["pr", "merge", "--squash", "42"]));
        assert!(matches!(r, Decision::Block(_)));
    }

    #[test]
    fn gh_other_pr_subcommands_allowed() {
        for sub in ["create", "view", "list", "checkout", "diff", "comment"] {
            let r = decide(Tool::Gh, &args(&["pr", sub]));
            assert_eq!(r, Decision::Allow);
        }
    }

    #[test]
    fn gh_unrelated_subcommands_allowed() {
        let r = decide(Tool::Gh, &args(&["repo", "view"]));
        assert_eq!(r, Decision::Allow);
    }

    #[test]
    fn other_tool_is_allow_by_default() {
        let r = decide(Tool::Other, &args(&["anything"]));
        assert_eq!(r, Decision::Allow);
    }

    #[test]
    fn empty_argv_is_allow() {
        assert_eq!(decide(Tool::Git, &[]), Decision::Allow);
        assert_eq!(decide(Tool::Gh, &[]), Decision::Allow);
    }

    #[test]
    fn install_shims_creates_two_symlinks() {
        let tmp = tempfile::TempDir::new().unwrap();
        let shim_dir = tmp.path().join("bin");
        let safe_git = tmp.path().join("safe-git");
        std::fs::write(&safe_git, b"#!/bin/sh\nexit 0\n").unwrap();

        install_shims(&shim_dir, &safe_git).unwrap();
        let git_link = shim_dir.join("git");
        let gh_link = shim_dir.join("gh");
        assert!(git_link.is_symlink());
        assert!(gh_link.is_symlink());
        assert_eq!(std::fs::read_link(&git_link).unwrap(), safe_git);
        assert_eq!(std::fs::read_link(&gh_link).unwrap(), safe_git);
    }

    #[test]
    fn install_shims_idempotent_when_target_matches() {
        let tmp = tempfile::TempDir::new().unwrap();
        let shim_dir = tmp.path().join("bin");
        let safe_git = tmp.path().join("safe-git");
        std::fs::write(&safe_git, b"").unwrap();
        install_shims(&shim_dir, &safe_git).unwrap();
        // Second call must not error and must leave links intact.
        install_shims(&shim_dir, &safe_git).unwrap();
        assert_eq!(std::fs::read_link(shim_dir.join("git")).unwrap(), safe_git);
    }

    #[test]
    fn install_shims_replaces_stale_symlink() {
        let tmp = tempfile::TempDir::new().unwrap();
        let shim_dir = tmp.path().join("bin");
        std::fs::create_dir_all(&shim_dir).unwrap();
        let stale = tmp.path().join("old-safe-git");
        std::fs::write(&stale, b"").unwrap();
        std::os::unix::fs::symlink(&stale, shim_dir.join("git")).unwrap();
        std::os::unix::fs::symlink(&stale, shim_dir.join("gh")).unwrap();

        let fresh = tmp.path().join("new-safe-git");
        std::fs::write(&fresh, b"").unwrap();
        install_shims(&shim_dir, &fresh).unwrap();
        assert_eq!(std::fs::read_link(shim_dir.join("git")).unwrap(), fresh);
    }

    #[test]
    fn install_shims_refuses_to_clobber_real_file() {
        let tmp = tempfile::TempDir::new().unwrap();
        let shim_dir = tmp.path().join("bin");
        std::fs::create_dir_all(&shim_dir).unwrap();
        std::fs::write(shim_dir.join("git"), b"not a symlink").unwrap();
        let safe_git = tmp.path().join("safe-git");
        std::fs::write(&safe_git, b"").unwrap();
        let err = install_shims(&shim_dir, &safe_git).unwrap_err();
        assert!(
            err.to_string().contains("not a symlink"),
            "expected refusal, got {err}"
        );
    }

    #[test]
    fn path_with_shim_prepended_puts_shim_first() {
        let tmp = tempfile::TempDir::new().unwrap();
        let shim_dir = tmp.path().join("bin");
        // SAFETY: env mutation is unsafe in 2024 edition. Used in this single
        // test only; not relying on cross-test serialization.
        unsafe {
            std::env::set_var("PATH", "/usr/bin:/bin");
        }
        let new_path = path_with_shim_prepended(&shim_dir).unwrap();
        let parts: Vec<_> = std::env::split_paths(&new_path).collect();
        assert_eq!(parts[0], shim_dir);
        assert!(parts.iter().any(|p| p == Path::new("/usr/bin")));
    }
}

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use tokio::process::Command;

#[derive(Debug, Clone)]
pub struct Worktree {
    pub path: PathBuf,
    pub branch: Option<String>,
    #[allow(dead_code)]
    pub head: Option<String>,
    pub is_current: bool,
}

#[derive(Debug, Clone, Default)]
pub struct Diff {
    pub files: Vec<(FileStatus, PathBuf)>,
    pub insertions: u32,
    pub deletions: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileStatus {
    Added,
    Modified,
    Deleted,
    Renamed,
    Copied,
    Untracked,
    Other(char),
}

impl FileStatus {
    fn from_char(c: char) -> Self {
        match c {
            'A' => Self::Added,
            'M' => Self::Modified,
            'D' => Self::Deleted,
            'R' => Self::Renamed,
            'C' => Self::Copied,
            '?' => Self::Untracked,
            other => Self::Other(other),
        }
    }

    #[allow(dead_code)]
    pub fn symbol(&self) -> &str {
        match self {
            Self::Added => "A",
            Self::Modified => "M",
            Self::Deleted => "D",
            Self::Renamed => "R",
            Self::Copied => "C",
            Self::Untracked => "?",
            Self::Other(_) => "~",
        }
    }
}

pub async fn list_worktrees(repo_path: &Path) -> Result<Vec<Worktree>> {
    let output = Command::new("git")
        .args([
            "-C",
            repo_path.to_str().unwrap_or("."),
            "worktree",
            "list",
            "--porcelain",
        ])
        .output()
        .await
        .context("failed to run git worktree list")?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_worktree_porcelain(&stdout)
}

fn parse_worktree_porcelain(input: &str) -> Result<Vec<Worktree>> {
    let mut worktrees = Vec::new();
    let mut current_path: Option<PathBuf> = None;
    let mut current_branch: Option<String> = None;
    let mut current_head: Option<String> = None;
    let mut is_bare = false;
    let mut is_first = true;
    let mut is_current_wt = false;

    let flush = |worktrees: &mut Vec<Worktree>,
                 path: Option<PathBuf>,
                 branch: Option<String>,
                 head: Option<String>,
                 is_bare: bool,
                 is_current: bool| {
        if let Some(p) = path
            && !is_bare
        {
            worktrees.push(Worktree {
                path: p,
                branch,
                head,
                is_current,
            });
        }
    };

    for line in input.lines() {
        if line.is_empty() {
            flush(
                &mut worktrees,
                current_path.take(),
                current_branch.take(),
                current_head.take(),
                is_bare,
                is_current_wt,
            );
            is_bare = false;
            is_current_wt = false;
            is_first = false;
        } else if let Some(rest) = line.strip_prefix("worktree ") {
            current_path = Some(PathBuf::from(rest));
            is_current_wt = is_first;
        } else if let Some(rest) = line.strip_prefix("HEAD ") {
            current_head = Some(rest.to_string());
        } else if let Some(rest) = line.strip_prefix("branch ") {
            let branch = rest.strip_prefix("refs/heads/").unwrap_or(rest);
            current_branch = Some(branch.to_string());
        } else if line == "bare" {
            is_bare = true;
        }
    }

    // handle last entry when there is no trailing blank line
    flush(
        &mut worktrees,
        current_path,
        current_branch,
        current_head,
        is_bare,
        is_current_wt,
    );

    Ok(worktrees)
}

pub async fn add_worktree(repo_path: &Path, branch: &str) -> Result<PathBuf> {
    let wt_path = worktree_path_for(repo_path, branch).await?;

    if wt_path.exists() {
        anyhow::bail!("worktree path already exists: {}", wt_path.display());
    }
    if let Some(parent) = wt_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to mkdir -p {}", parent.display()))?;
    }

    let output = Command::new("git")
        .args([
            "-C",
            repo_path.to_str().unwrap_or("."),
            "worktree",
            "add",
            wt_path.to_str().unwrap_or("."),
            "-b",
            branch,
        ])
        .output()
        .await
        .context("failed to run git worktree add")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("git worktree add failed: {}", stderr.trim());
    }

    Ok(wt_path)
}

/// Compute target worktree path: `$HOME/.fellowship/worktrees/<owner>/<repo>/<branch>`.
/// Falls back to `local/<repo-dirname>/<branch>` when origin remote is absent.
/// Branch slashes are preserved (e.g. `feat/x` → `.../<repo>/feat/x`).
async fn worktree_path_for(repo_path: &Path, branch: &str) -> Result<PathBuf> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| anyhow::anyhow!("HOME env var not set"))?;
    let base = home.join(".fellowship").join("worktrees");

    let (owner, repo) = match remote_origin(repo_path).await? {
        Some(url) => parse_owner_repo(&url)
            .unwrap_or_else(|| ("local".to_string(), repo_dirname(repo_path).to_string())),
        None => ("local".to_string(), repo_dirname(repo_path).to_string()),
    };

    let mut path = base.join(owner).join(repo);
    for seg in branch.split('/').filter(|s| !s.is_empty()) {
        path.push(seg);
    }
    Ok(path)
}

fn repo_dirname(repo_path: &Path) -> &str {
    repo_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("repo")
}

pub async fn remote_origin(repo_path: &Path) -> Result<Option<String>> {
    let output = Command::new("git")
        .args([
            "-C",
            repo_path.to_str().unwrap_or("."),
            "config",
            "--get",
            "remote.origin.url",
        ])
        .output()
        .await
        .context("failed to run git config")?;
    if !output.status.success() {
        return Ok(None);
    }
    let url = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if url.is_empty() {
        Ok(None)
    } else {
        Ok(Some(url))
    }
}

/// Parse `owner/repo` from common remote URL shapes. Returns `None` on unknown formats.
/// Supports:
///   - `git@host:owner/repo(.git)?`
///   - `https://host/owner/repo(.git)?`
///   - `https://host/group/sub/repo(.git)?` → owner = `group-sub`
///   - `ssh://git@host/owner/repo(.git)?`
pub fn parse_owner_repo(url: &str) -> Option<(String, String)> {
    let url = url.trim().trim_end_matches('/');
    let url = url.strip_suffix(".git").unwrap_or(url);

    // SCP-style: git@github.com:owner/repo
    if let Some((_, path)) = url.split_once(':')
        && !url.starts_with("http://")
        && !url.starts_with("https://")
        && !url.starts_with("ssh://")
        && !path.is_empty()
    {
        return split_path_into_owner_repo(path);
    }

    // URL form: scheme://[user@]host/path
    if let Some(rest) = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .or_else(|| url.strip_prefix("ssh://"))
        && let Some((_, path)) = rest.split_once('/')
    {
        return split_path_into_owner_repo(path);
    }

    None
}

fn split_path_into_owner_repo(path: &str) -> Option<(String, String)> {
    let segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    match segs.as_slice() {
        [] | [_] => None,
        [owner, repo] => Some(((*owner).to_string(), (*repo).to_string())),
        rest => {
            // owner = all but last joined by '-', repo = last
            let last = *rest.last().unwrap();
            let owner = rest[..rest.len() - 1].join("-");
            Some((owner, last.to_string()))
        }
    }
}

pub async fn current_branch(repo_path: &Path) -> Result<Option<String>> {
    let output = Command::new("git")
        .args([
            "-C",
            repo_path.to_str().unwrap_or("."),
            "rev-parse",
            "--abbrev-ref",
            "HEAD",
        ])
        .output()
        .await
        .context("failed to run git rev-parse")?;

    if !output.status.success() {
        return Ok(None);
    }

    let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if branch == "HEAD" {
        Ok(None) // detached HEAD
    } else {
        Ok(Some(branch))
    }
}

pub async fn diff_summary(repo_path: &Path) -> Result<Diff> {
    let cwd = repo_path.to_str().unwrap_or(".");
    let (stat_out, name_out, untracked_out) = tokio::join!(
        Command::new("git")
            .args(["-C", cwd, "diff", "--shortstat", "HEAD"])
            .output(),
        Command::new("git")
            .args(["-C", cwd, "diff", "--name-status", "HEAD"])
            .output(),
        Command::new("git")
            .args(["-C", cwd, "ls-files", "--others", "--exclude-standard"])
            .output(),
    );

    let mut diff = Diff::default();

    if let Ok(out) = stat_out {
        let text = String::from_utf8_lossy(&out.stdout);
        parse_shortstat(text.as_ref(), &mut diff);
    }

    if let Ok(out) = name_out {
        let text = String::from_utf8_lossy(&out.stdout);
        for line in text.lines() {
            let mut parts = line.splitn(2, '\t');
            if let (Some(status_str), Some(path)) = (parts.next(), parts.next()) {
                let status_char = status_str.chars().next().unwrap_or('?');
                // For renames/copies the path field is "old\tnew" — take the new name.
                let actual_path = if status_char == 'R' || status_char == 'C' {
                    path.split_once('\t').map(|x| x.1).unwrap_or(path)
                } else {
                    path
                };
                diff.files.push((
                    FileStatus::from_char(status_char),
                    PathBuf::from(actual_path),
                ));
            }
        }
    }

    if let Ok(out) = untracked_out {
        let text = String::from_utf8_lossy(&out.stdout);
        for line in text.lines() {
            let path = line.trim();
            if !path.is_empty() {
                diff.files
                    .push((FileStatus::Untracked, PathBuf::from(path)));
            }
        }
    }

    Ok(diff)
}

fn parse_shortstat(text: &str, diff: &mut Diff) {
    // Example: " 3 files changed, 45 insertions(+), 12 deletions(-)"
    for part in text.split(',') {
        let part = part.trim();
        if part.contains("insertion")
            && let Some(n) = part.split_whitespace().next().and_then(|s| s.parse().ok())
        {
            diff.insertions = n;
        } else if part.contains("deletion")
            && let Some(n) = part.split_whitespace().next().and_then(|s| s.parse().ok())
        {
            diff.deletions = n;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_worktree_porcelain_basic() {
        let input = "\
worktree /home/user/repo
HEAD abc123
branch refs/heads/main

worktree /home/user/repo-feat
HEAD def456
branch refs/heads/feat/test

";
        let wts = parse_worktree_porcelain(input).unwrap();
        assert_eq!(wts.len(), 2);
        assert_eq!(wts[0].branch.as_deref(), Some("main"));
        assert!(wts[0].is_current);
        assert_eq!(wts[1].branch.as_deref(), Some("feat/test"));
        assert!(!wts[1].is_current);
    }

    #[test]
    fn test_parse_worktree_porcelain_bare_skipped() {
        let input = "\
worktree /home/user/repo
HEAD abc123
branch refs/heads/main

worktree /home/user/repo.git
HEAD 000000
bare

";
        let wts = parse_worktree_porcelain(input).unwrap();
        assert_eq!(wts.len(), 1);
        assert_eq!(wts[0].branch.as_deref(), Some("main"));
    }

    #[test]
    fn test_parse_worktree_porcelain_no_trailing_newline() {
        let input = "worktree /home/user/repo\nHEAD abc123\nbranch refs/heads/main";
        let wts = parse_worktree_porcelain(input).unwrap();
        assert_eq!(wts.len(), 1);
    }

    #[test]
    fn test_parse_shortstat() {
        let mut diff = Diff::default();
        parse_shortstat(
            " 3 files changed, 45 insertions(+), 12 deletions(-)",
            &mut diff,
        );
        assert_eq!(diff.insertions, 45);
        assert_eq!(diff.deletions, 12);
    }

    #[test]
    fn test_parse_shortstat_insertions_only() {
        let mut diff = Diff::default();
        parse_shortstat(" 1 file changed, 5 insertions(+)", &mut diff);
        assert_eq!(diff.insertions, 5);
        assert_eq!(diff.deletions, 0);
    }

    #[test]
    fn test_file_status_symbols() {
        assert_eq!(FileStatus::Added.symbol(), "A");
        assert_eq!(FileStatus::Modified.symbol(), "M");
        assert_eq!(FileStatus::Deleted.symbol(), "D");
        assert_eq!(FileStatus::Renamed.symbol(), "R");
    }

    #[test]
    fn parse_owner_repo_github_ssh() {
        assert_eq!(
            parse_owner_repo("git@github.com:foo/bar.git"),
            Some(("foo".into(), "bar".into()))
        );
    }

    #[test]
    fn parse_owner_repo_github_https() {
        assert_eq!(
            parse_owner_repo("https://github.com/foo/bar.git"),
            Some(("foo".into(), "bar".into()))
        );
    }

    #[test]
    fn parse_owner_repo_https_no_dot_git() {
        assert_eq!(
            parse_owner_repo("https://github.com/foo/bar"),
            Some(("foo".into(), "bar".into()))
        );
    }

    #[test]
    fn parse_owner_repo_gitlab_nested() {
        assert_eq!(
            parse_owner_repo("https://gitlab.com/group/sub/repo.git"),
            Some(("group-sub".into(), "repo".into()))
        );
    }

    #[test]
    fn parse_owner_repo_ssh_url() {
        assert_eq!(
            parse_owner_repo("ssh://git@github.com/foo/bar.git"),
            Some(("foo".into(), "bar".into()))
        );
    }

    #[test]
    fn parse_owner_repo_unknown_returns_none() {
        assert_eq!(parse_owner_repo(""), None);
        assert_eq!(parse_owner_repo("not-a-url"), None);
        assert_eq!(parse_owner_repo("git@host:onlyone"), None);
    }
}

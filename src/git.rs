use std::path::{Path, PathBuf};
use anyhow::{Context, Result};
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
        .args(["-C", repo_path.to_str().unwrap_or("."), "worktree", "list", "--porcelain"])
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
        if let Some(p) = path && !is_bare {
            worktrees.push(Worktree { path: p, branch, head, is_current });
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
    let repo_name = repo_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("repo");
    let slug = branch.replace('/', "-");
    let parent = repo_path.parent().unwrap_or(Path::new("."));
    let wt_path = parent.join(format!("{}-{}", repo_name, slug));

    if wt_path.exists() {
        anyhow::bail!("worktree path already exists: {}", wt_path.display());
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

pub async fn current_branch(repo_path: &Path) -> Result<Option<String>> {
    let output = Command::new("git")
        .args(["-C", repo_path.to_str().unwrap_or("."), "rev-parse", "--abbrev-ref", "HEAD"])
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
    let (stat_out, name_out) = tokio::join!(
        Command::new("git")
            .args(["-C", repo_path.to_str().unwrap_or("."), "diff", "--shortstat", "HEAD"])
            .output(),
        Command::new("git")
            .args(["-C", repo_path.to_str().unwrap_or("."), "diff", "--name-status", "HEAD"])
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
                diff.files.push((FileStatus::from_char(status_char), PathBuf::from(actual_path)));
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
        parse_shortstat(" 3 files changed, 45 insertions(+), 12 deletions(-)", &mut diff);
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
}

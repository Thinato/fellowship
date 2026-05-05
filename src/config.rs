use std::path::PathBuf;

#[allow(dead_code)]
pub fn shell() -> String {
    std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string())
}

#[allow(dead_code)]
pub fn worktree_sibling_path(repo_path: &std::path::Path, branch: &str) -> PathBuf {
    let repo_name = repo_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("repo");
    let branch_slug = branch.replace('/', "-");
    let parent = repo_path.parent().unwrap_or(repo_path);
    parent.join(format!("{}-{}", repo_name, branch_slug))
}

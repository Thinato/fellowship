use std::path::Path;
use anyhow::Result;
use serde::Deserialize;
use tokio::process::Command;

#[derive(Debug, Clone, Deserialize)]
pub struct PrInfo {
    pub number: u64,
    pub title: String,
    pub state: String,
    pub url: String,
}

/// Returns None when there is no PR, gh is absent, or not authenticated.
pub async fn current_pr(repo_path: &Path, branch: &str) -> Result<Option<PrInfo>> {
    if !gh_available().await {
        return Ok(None);
    }

    let output = Command::new("gh")
        .args([
            "pr",
            "view",
            branch,
            "--json",
            "number,title,state,url",
        ])
        .current_dir(repo_path)
        .output()
        .await?;

    if !output.status.success() {
        // Non-zero exit means no PR found or auth issue — treat as absent.
        return Ok(None);
    }

    let pr: PrInfo = serde_json::from_slice(&output.stdout)?;
    Ok(Some(pr))
}

async fn gh_available() -> bool {
    Command::new("gh")
        .arg("--version")
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pr_info_deserialize() {
        let json = r#"{"number":42,"title":"feat: add thing","state":"OPEN","url":"https://github.com/org/repo/pull/42"}"#;
        let pr: PrInfo = serde_json::from_str(json).unwrap();
        assert_eq!(pr.number, 42);
        assert_eq!(pr.state, "OPEN");
        assert_eq!(pr.title, "feat: add thing");
    }

    #[test]
    fn test_pr_info_deserialize_closed() {
        let json = r#"{"number":7,"title":"fix: bug","state":"CLOSED","url":"https://github.com/org/repo/pull/7"}"#;
        let pr: PrInfo = serde_json::from_str(json).unwrap();
        assert_eq!(pr.number, 7);
        assert_eq!(pr.state, "CLOSED");
    }
}

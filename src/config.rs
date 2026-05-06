use std::path::PathBuf;

use serde::Deserialize;

#[derive(Debug, Default, Clone, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub shell_startup_command: Option<String>,
}

impl Config {
    pub fn load() -> Self {
        let Some(path) = config_path() else {
            return Self::default();
        };
        let Ok(text) = std::fs::read_to_string(&path) else {
            return Self::default();
        };
        match toml::from_str::<Config>(&text) {
            Ok(cfg) => cfg,
            Err(e) => {
                eprintln!("fellowship: failed to parse {}: {e}", path.display());
                Self::default()
            }
        }
    }
}

pub fn config_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".config/fellowship/config.toml"))
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_shell_startup_command() {
        let toml_str = r#"shell_startup_command = "claude""#;
        let cfg: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(cfg.shell_startup_command.as_deref(), Some("claude"));
    }

    #[test]
    fn empty_config_yields_none() {
        let cfg: Config = toml::from_str("").unwrap();
        assert!(cfg.shell_startup_command.is_none());
    }
}

use std::path::{Path, PathBuf};

use serde::Deserialize;

#[derive(Debug, Default, Clone, Deserialize, PartialEq, Eq)]
pub struct Config {
    #[serde(default)]
    pub shell_startup_command: Option<String>,
}

impl Config {
    /// Load global config, then merge project-local config on top.
    /// Local fields override global fields where set.
    pub fn load(repo: &Path) -> Self {
        let global = read_config(global_config_path()).unwrap_or_default();
        let local = read_config(Some(local_config_path(repo))).unwrap_or_default();
        Self::merge(global, local)
    }

    fn merge(global: Self, local: Self) -> Self {
        Self {
            shell_startup_command: local.shell_startup_command.or(global.shell_startup_command),
        }
    }
}

fn read_config(path: Option<PathBuf>) -> Option<Config> {
    let path = path?;
    let text = std::fs::read_to_string(&path).ok()?;
    match toml::from_str::<Config>(&text) {
        Ok(cfg) => Some(cfg),
        Err(e) => {
            tracing::error!("failed to parse config {}: {e}", path.display());
            None
        }
    }
}

pub fn global_config_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".config/fellowship/config.toml"))
}

pub fn local_config_path(repo: &Path) -> PathBuf {
    repo.join(".fellowship/config.toml")
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

    #[test]
    fn local_overrides_global() {
        let global = Config {
            shell_startup_command: Some("global-cmd".into()),
        };
        let local = Config {
            shell_startup_command: Some("local-cmd".into()),
        };
        let merged = Config::merge(global, local);
        assert_eq!(merged.shell_startup_command.as_deref(), Some("local-cmd"));
    }

    #[test]
    fn local_unset_falls_back_to_global() {
        let global = Config {
            shell_startup_command: Some("global-cmd".into()),
        };
        let local = Config::default();
        let merged = Config::merge(global, local);
        assert_eq!(merged.shell_startup_command.as_deref(), Some("global-cmd"));
    }
}

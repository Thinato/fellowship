//! Beads integration. Shells out to `bd` (https://github.com/gastownhall/beads)
//! and converts its JSON output into the canonical [`Bead`] shape used by the
//! rest of fellowship.
//!
//! See `docs/plans/agentic-ui-v1.md` §3.2 for the bus design.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tokio::process::Command;

/// Raw `bd list --json` array element. Fields beyond what we read are tolerated
/// and ignored thanks to `#[serde(default)]` + `serde_json`'s lenient decoder.
/// Keep this struct close to the upstream schema; do downstream coercion in
/// [`Bead::from`].
#[derive(Debug, Clone, Deserialize, Default)]
struct BdRawIssue {
    #[serde(default)]
    id: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    status: String,
    #[serde(default)]
    assignee: Option<String>,
    #[serde(default)]
    labels: Vec<String>,
    #[serde(default)]
    priority: Option<i32>,
    #[serde(default, rename = "type")]
    issue_type: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Status {
    Open,
    InProgress,
    InReview,
    Closed,
    Other,
}

impl Status {
    fn parse(s: &str) -> Self {
        match s {
            "open" => Status::Open,
            "in_progress" | "in-progress" => Status::InProgress,
            "in_review" | "in-review" => Status::InReview,
            "closed" | "done" => Status::Closed,
            _ => Status::Other,
        }
    }
}

/// Canonical fellowship view of a bead. Insulated from upstream `bd` schema
/// drift via [`BdRawIssue`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Bead {
    pub id: String,
    pub title: String,
    pub status: Status,
    pub assignee: Option<String>,
    pub labels: Vec<String>,
    pub priority: Option<i32>,
    pub issue_type: Option<String>,
}

impl From<BdRawIssue> for Bead {
    fn from(raw: BdRawIssue) -> Self {
        Self {
            id: raw.id,
            title: raw.title,
            status: Status::parse(&raw.status),
            assignee: raw.assignee,
            labels: raw.labels,
            priority: raw.priority,
            issue_type: raw.issue_type,
        }
    }
}

/// Run `bd list --json` and parse the result into canonical beads.
/// `bd_path` lets tests inject a fake binary; production callers pass `"bd"`.
pub async fn list_beads_with(bd_path: &str) -> Result<Vec<Bead>> {
    let out = Command::new(bd_path)
        .args(["list", "--json"])
        .output()
        .await
        .with_context(|| format!("invoking `{} list --json`", bd_path))?;
    if !out.status.success() {
        anyhow::bail!(
            "{} list --json failed: {}",
            bd_path,
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    parse_bd_list(&out.stdout)
}

pub async fn list_beads() -> Result<Vec<Bead>> {
    list_beads_with("bd").await
}

/// Run `bd create --silent --title=… --description=… [--labels=…]` and return
/// the new bead id. Used by the watchdog to author `[fellowship-watchdog]`
/// beads on Stale transitions. `bd_path` is injectable for tests.
pub async fn create_bead_with(
    bd_path: &str,
    title: &str,
    description: &str,
    labels: &[&str],
) -> Result<String> {
    let mut args: Vec<String> = vec![
        "create".to_string(),
        "--silent".to_string(),
        "--title".to_string(),
        title.to_string(),
        "--description".to_string(),
        description.to_string(),
        "--type".to_string(),
        "task".to_string(),
    ];
    if !labels.is_empty() {
        args.push("--labels".to_string());
        args.push(labels.join(","));
    }
    let out = Command::new(bd_path)
        .args(&args)
        .output()
        .await
        .with_context(|| format!("invoking `{} create`", bd_path))?;
    if !out.status.success() {
        anyhow::bail!(
            "{} create failed: {}",
            bd_path,
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let id = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if id.is_empty() {
        anyhow::bail!("`{} create --silent` returned empty stdout", bd_path);
    }
    Ok(id)
}

pub async fn create_bead(title: &str, description: &str, labels: &[&str]) -> Result<String> {
    create_bead_with("bd", title, description, labels).await
}

fn parse_bd_list(stdout: &[u8]) -> Result<Vec<Bead>> {
    if stdout.iter().all(u8::is_ascii_whitespace) {
        return Ok(Vec::new());
    }
    let raw: Vec<BdRawIssue> = serde_json::from_slice(stdout)
        .context("parsing `bd list --json` output as a JSON array")?;
    Ok(raw.into_iter().map(Bead::from).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use tempfile::TempDir;

    fn write_fake_bd(dir: &std::path::Path, body: &str) -> std::path::PathBuf {
        // Tiny shell script that echoes the supplied JSON when called as
        // `bd list --json`. Argument matching is intentionally loose; the
        // script just dumps `body` to stdout regardless.
        let path = dir.join("bd");
        std::fs::write(&path, format!("#!/bin/sh\ncat <<'EOF'\n{}\nEOF\n", body)).unwrap();
        let mut perm = std::fs::metadata(&path).unwrap().permissions();
        perm.set_mode(0o755);
        std::fs::set_permissions(&path, perm).unwrap();
        path
    }

    fn write_failing_bd(dir: &std::path::Path) -> std::path::PathBuf {
        let path = dir.join("bd");
        std::fs::write(&path, "#!/bin/sh\necho 'boom' >&2\nexit 7\n").unwrap();
        let mut perm = std::fs::metadata(&path).unwrap().permissions();
        perm.set_mode(0o755);
        std::fs::set_permissions(&path, perm).unwrap();
        path
    }

    #[test]
    fn status_parse_recognizes_known_values() {
        assert_eq!(Status::parse("open"), Status::Open);
        assert_eq!(Status::parse("in_progress"), Status::InProgress);
        assert_eq!(Status::parse("in-progress"), Status::InProgress);
        assert_eq!(Status::parse("in_review"), Status::InReview);
        assert_eq!(Status::parse("closed"), Status::Closed);
        assert_eq!(Status::parse("done"), Status::Closed);
        assert_eq!(Status::parse("anything-else"), Status::Other);
    }

    #[test]
    fn parse_bd_list_handles_empty_array() {
        let out = parse_bd_list(b"[]").unwrap();
        assert!(out.is_empty());
    }

    #[test]
    fn parse_bd_list_handles_blank_stdout() {
        let out = parse_bd_list(b"").unwrap();
        assert!(out.is_empty());
        let out = parse_bd_list(b"   \n  ").unwrap();
        assert!(out.is_empty());
    }

    #[test]
    fn parse_bd_list_maps_known_fields() {
        let json = r#"[
            {"id":"bd-1","title":"Fix auth","status":"open","assignee":"engineer-1","labels":["role:engineer","priority:p1"],"priority":1,"type":"task"},
            {"id":"bd-2","title":"Design","status":"in_progress","labels":["role:architect"]}
        ]"#;
        let beads = parse_bd_list(json.as_bytes()).unwrap();
        assert_eq!(beads.len(), 2);
        assert_eq!(beads[0].id, "bd-1");
        assert_eq!(beads[0].status, Status::Open);
        assert_eq!(beads[0].assignee.as_deref(), Some("engineer-1"));
        assert_eq!(beads[0].priority, Some(1));
        assert_eq!(beads[0].issue_type.as_deref(), Some("task"));
        assert_eq!(beads[1].status, Status::InProgress);
        assert!(beads[1].assignee.is_none());
    }

    #[test]
    fn parse_bd_list_tolerates_unknown_fields() {
        let json = r#"[
            {"id":"bd-9","title":"x","status":"open","weird_extra":42,"more_junk":[1,2,3]}
        ]"#;
        let beads = parse_bd_list(json.as_bytes()).unwrap();
        assert_eq!(beads.len(), 1);
        assert_eq!(beads[0].id, "bd-9");
    }

    #[tokio::test]
    async fn list_beads_with_invokes_supplied_binary() {
        let tmp = TempDir::new().unwrap();
        let body = r#"[{"id":"bd-7","title":"hello","status":"open","labels":["role:engineer"]}]"#;
        let bd = write_fake_bd(tmp.path(), body);
        let beads = list_beads_with(bd.to_str().unwrap()).await.unwrap();
        assert_eq!(beads.len(), 1);
        assert_eq!(beads[0].id, "bd-7");
        assert_eq!(beads[0].labels, vec!["role:engineer".to_string()]);
    }

    fn write_capturing_bd(dir: &std::path::Path) -> std::path::PathBuf {
        // Echoes a fake id and writes its argv to a sidecar file so the test
        // can assert the exact arguments without a full bd install.
        let path = dir.join("bd");
        let argv_log = dir.join("argv.txt");
        std::fs::write(
            &path,
            format!(
                "#!/bin/sh\nfor arg in \"$@\"; do printf '%s\\n' \"$arg\"; done > '{}'\necho 'bd-fake-1'\n",
                argv_log.display()
            ),
        )
        .unwrap();
        let mut perm = std::fs::metadata(&path).unwrap().permissions();
        perm.set_mode(0o755);
        std::fs::set_permissions(&path, perm).unwrap();
        path
    }

    #[tokio::test]
    async fn create_bead_with_returns_id_and_passes_args() {
        let tmp = TempDir::new().unwrap();
        let bd = write_capturing_bd(tmp.path());
        let id = create_bead_with(
            bd.to_str().unwrap(),
            "the title",
            "the body",
            &["fellowship-watchdog", "phase-12"],
        )
        .await
        .unwrap();
        assert_eq!(id, "bd-fake-1");
        let argv = std::fs::read_to_string(tmp.path().join("argv.txt")).unwrap();
        let lines: Vec<&str> = argv.lines().collect();
        assert!(lines.contains(&"create"), "argv was: {argv}");
        assert!(lines.contains(&"--silent"), "argv was: {argv}");
        assert!(lines.contains(&"the title"), "argv was: {argv}");
        assert!(lines.contains(&"the body"), "argv was: {argv}");
        assert!(
            lines.contains(&"fellowship-watchdog,phase-12"),
            "argv was: {argv}"
        );
    }

    #[tokio::test]
    async fn create_bead_with_propagates_failure() {
        let tmp = TempDir::new().unwrap();
        let bd = write_failing_bd(tmp.path());
        let err = create_bead_with(bd.to_str().unwrap(), "t", "d", &[])
            .await
            .expect_err("non-zero exit should surface");
        let msg = format!("{:#}", err);
        assert!(msg.contains("boom"), "stderr should propagate: {msg}");
    }

    #[tokio::test]
    async fn list_beads_with_propagates_failure() {
        let tmp = TempDir::new().unwrap();
        let bd = write_failing_bd(tmp.path());
        let err = list_beads_with(bd.to_str().unwrap())
            .await
            .expect_err("expected non-zero exit to surface as error");
        let msg = format!("{:#}", err);
        assert!(msg.contains("boom"), "stderr should be in error: {msg}");
    }
}

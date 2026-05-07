//! `fellowship-ctl` — agent-facing helper CLI.
//!
//! Each subcommand either drops a JSON file under
//! `~/.fellowship/runtime/<session>/` (which fellowship watches via `notify`)
//! or shells out to a downstream tool (`gh`, `bd`).
//!
//! See `docs/plans/agentic-ui-v1.md` §3.3 for the design.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use uuid::Uuid;

use fellowship::runtime::{
    HeartbeatRecord, JOURNAL_FILE, JournalEntry, RELEASE_REQUEST_DIR, ReleaseRequest,
    SPAWN_REQUEST_DIR, STATE_DIR, SpawnRequest, ensure_subdir, now_ms, runtime_dir,
};

#[derive(Parser, Debug)]
#[command(
    name = "fellowship-ctl",
    about = "Agent-facing helper CLI for fellowship",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Record a heartbeat for an agent.
    Heartbeat {
        /// Agent id (e.g. `pm`, `orchestrator`, `engineer-1`).
        agent_id: String,
        /// One-line description of what the agent is currently doing.
        #[arg(long)]
        status: String,
    },
    /// Append a journal line for an agent.
    Log {
        /// Agent id.
        agent_id: String,
        /// Free-form message.
        message: String,
    },
    /// Request fellowship to spawn a new engineer in a fresh worktree.
    SpawnEngineer {
        /// Branch name to use for the new worktree (e.g. `feat/bead-123`).
        #[arg(long)]
        branch: Option<String>,
        /// Engineer should exit after one bead instead of looping.
        #[arg(long, default_value_t = false)]
        single_shot: bool,
    },
    /// Request fellowship to reap an engineer's PTY.
    ReleaseEngineer {
        /// Agent id to release (e.g. `engineer-1`).
        agent_id: String,
    },
    /// Fetch unresolved PR comments and emit JSONL on stdout. v1 helper for
    /// engineers; Phase 13 replaces the primary path with a beads message bus.
    PrComments {
        /// PR number.
        pr_number: u32,
        /// Override `<owner>/<repo>` (defaults to `gh`'s detected slug).
        #[arg(long)]
        repo: Option<String>,
    },
    /// Passthrough to the `bd` (beads) CLI so agent prompts can stay
    /// tool-agnostic. All arguments after `--` are forwarded verbatim.
    Bead {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
}

fn write_heartbeat(root: &Path, agent_id: &str, status: &str) -> Result<PathBuf> {
    let dir = ensure_subdir(root, STATE_DIR)?;
    let path = dir.join(format!("{}.json", agent_id));
    let record = HeartbeatRecord {
        agent_id: agent_id.to_string(),
        last_seen_ms: now_ms(),
        status: status.to_string(),
    };
    let json = serde_json::to_vec_pretty(&record)?;
    fs::write(&path, json).with_context(|| format!("write {}", path.display()))?;
    Ok(path)
}

fn append_journal(root: &Path, agent_id: &str, message: &str) -> Result<PathBuf> {
    fs::create_dir_all(root).with_context(|| format!("mkdir -p {}", root.display()))?;
    let path = root.join(JOURNAL_FILE);
    let entry = JournalEntry {
        ts_ms: now_ms(),
        agent_id: agent_id.to_string(),
        message: message.to_string(),
    };
    let mut line = serde_json::to_string(&entry)?;
    line.push('\n');
    let mut f = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("open {}", path.display()))?;
    f.write_all(line.as_bytes())?;
    Ok(path)
}

fn write_spawn_request(
    root: &Path,
    branch: Option<String>,
    single_shot: bool,
) -> Result<(PathBuf, String)> {
    let dir = ensure_subdir(root, SPAWN_REQUEST_DIR)?;
    let request_id = Uuid::new_v4().to_string();
    let path = dir.join(format!("{}.json", request_id));
    let req = SpawnRequest {
        request_id: request_id.clone(),
        branch,
        single_shot,
        requested_at_ms: now_ms(),
    };
    let json = serde_json::to_vec_pretty(&req)?;
    fs::write(&path, json).with_context(|| format!("write {}", path.display()))?;
    Ok((path, request_id))
}

fn write_release_request(root: &Path, agent_id: &str) -> Result<(PathBuf, String)> {
    let dir = ensure_subdir(root, RELEASE_REQUEST_DIR)?;
    let request_id = Uuid::new_v4().to_string();
    let path = dir.join(format!("{}.json", request_id));
    let req = ReleaseRequest {
        request_id: request_id.clone(),
        agent_id: agent_id.to_string(),
        requested_at_ms: now_ms(),
    };
    let json = serde_json::to_vec_pretty(&req)?;
    fs::write(&path, json).with_context(|| format!("write {}", path.display()))?;
    Ok((path, request_id))
}

/// Fetch unresolved PR comments via `gh api` (both `pulls` and `issues`
/// endpoints) and emit one JSONL record per comment on stdout.
fn run_pr_comments(pr_number: u32, repo: Option<String>) -> Result<()> {
    let mut endpoints: Vec<String> = Vec::new();
    let prefix = match repo {
        Some(r) => format!("repos/{}", r),
        None => {
            let out = Command::new("gh")
                .args([
                    "repo",
                    "view",
                    "--json",
                    "nameWithOwner",
                    "-q",
                    ".nameWithOwner",
                ])
                .output()
                .context("running `gh repo view` to resolve repo slug")?;
            if !out.status.success() {
                anyhow::bail!(
                    "gh repo view failed: {}",
                    String::from_utf8_lossy(&out.stderr).trim()
                );
            }
            let slug = String::from_utf8(out.stdout)?.trim().to_string();
            format!("repos/{}", slug)
        }
    };
    endpoints.push(format!("{}/pulls/{}/comments", prefix, pr_number));
    endpoints.push(format!("{}/issues/{}/comments", prefix, pr_number));

    for ep in endpoints {
        let out = Command::new("gh")
            .args(["api", "--paginate", &ep])
            .output()
            .with_context(|| format!("running `gh api {}`", ep))?;
        if !out.status.success() {
            anyhow::bail!(
                "gh api {} failed: {}",
                ep,
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        let value: serde_json::Value = serde_json::from_slice(&out.stdout)
            .with_context(|| format!("parsing gh api output for {}", ep))?;
        if let Some(arr) = value.as_array() {
            for item in arr {
                println!("{}", serde_json::to_string(item)?);
            }
        } else {
            println!("{}", serde_json::to_string(&value)?);
        }
    }
    Ok(())
}

fn run_bead_passthrough(args: &[String]) -> Result<()> {
    let status = Command::new("bd")
        .args(args)
        .status()
        .context("invoking `bd`")?;
    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }
    Ok(())
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let root = runtime_dir()?;

    match cli.command {
        Commands::Heartbeat { agent_id, status } => {
            let path = write_heartbeat(&root, &agent_id, &status)?;
            println!("heartbeat -> {}", path.display());
        }
        Commands::Log { agent_id, message } => {
            let path = append_journal(&root, &agent_id, &message)?;
            println!("journal +1 -> {}", path.display());
        }
        Commands::SpawnEngineer {
            branch,
            single_shot,
        } => {
            let (path, id) = write_spawn_request(&root, branch, single_shot)?;
            println!("spawn-request {} -> {}", id, path.display());
        }
        Commands::ReleaseEngineer { agent_id } => {
            let (path, id) = write_release_request(&root, &agent_id)?;
            println!("release-request {} -> {}", id, path.display());
        }
        Commands::PrComments { pr_number, repo } => {
            run_pr_comments(pr_number, repo)?;
        }
        Commands::Bead { args } => {
            run_bead_passthrough(&args)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn read_file(p: &Path) -> String {
        std::fs::read_to_string(p).unwrap()
    }

    #[test]
    fn heartbeat_writes_valid_json_at_expected_path() {
        let tmp = TempDir::new().unwrap();
        let path = write_heartbeat(tmp.path(), "engineer-1", "claiming bd-abc1").unwrap();
        assert!(path.ends_with("state/engineer-1.json"));
        let parsed: HeartbeatRecord = serde_json::from_str(&read_file(&path)).unwrap();
        assert_eq!(parsed.agent_id, "engineer-1");
        assert_eq!(parsed.status, "claiming bd-abc1");
        assert!(parsed.last_seen_ms > 0);
    }

    #[test]
    fn heartbeat_overwrites_previous_record() {
        let tmp = TempDir::new().unwrap();
        write_heartbeat(tmp.path(), "pm", "first").unwrap();
        let path = write_heartbeat(tmp.path(), "pm", "second").unwrap();
        let parsed: HeartbeatRecord = serde_json::from_str(&read_file(&path)).unwrap();
        assert_eq!(parsed.status, "second");
    }

    #[test]
    fn append_journal_creates_then_appends_ndjson() {
        let tmp = TempDir::new().unwrap();
        append_journal(tmp.path(), "pm", "creating bead bd-xyz").unwrap();
        append_journal(tmp.path(), "engineer-2", "opened PR #42").unwrap();
        let contents = read_file(&tmp.path().join(JOURNAL_FILE));
        let mut lines = contents.lines();
        let l1: JournalEntry = serde_json::from_str(lines.next().unwrap()).unwrap();
        let l2: JournalEntry = serde_json::from_str(lines.next().unwrap()).unwrap();
        assert!(lines.next().is_none(), "exactly two lines expected");
        assert_eq!(l1.agent_id, "pm");
        assert_eq!(l1.message, "creating bead bd-xyz");
        assert_eq!(l2.agent_id, "engineer-2");
        assert_eq!(l2.message, "opened PR #42");
    }

    #[test]
    fn spawn_request_records_branch_and_single_shot_flag() {
        let tmp = TempDir::new().unwrap();
        let (path, id) =
            write_spawn_request(tmp.path(), Some("feat/bead-99".into()), true).unwrap();
        assert!(path.starts_with(tmp.path().join(SPAWN_REQUEST_DIR)));
        assert!(path.to_string_lossy().contains(&id));
        let parsed: SpawnRequest = serde_json::from_str(&read_file(&path)).unwrap();
        assert_eq!(parsed.request_id, id);
        assert_eq!(parsed.branch.as_deref(), Some("feat/bead-99"));
        assert!(parsed.single_shot);
    }

    #[test]
    fn spawn_request_branch_is_optional() {
        let tmp = TempDir::new().unwrap();
        let (path, _) = write_spawn_request(tmp.path(), None, false).unwrap();
        let parsed: SpawnRequest = serde_json::from_str(&read_file(&path)).unwrap();
        assert!(parsed.branch.is_none());
        assert!(!parsed.single_shot);
    }

    #[test]
    fn release_request_carries_agent_id() {
        let tmp = TempDir::new().unwrap();
        let (path, id) = write_release_request(tmp.path(), "engineer-3").unwrap();
        assert!(path.starts_with(tmp.path().join(RELEASE_REQUEST_DIR)));
        let parsed: ReleaseRequest = serde_json::from_str(&read_file(&path)).unwrap();
        assert_eq!(parsed.request_id, id);
        assert_eq!(parsed.agent_id, "engineer-3");
    }

    /// Smoke check that the CLI parser accepts every documented invocation.
    #[test]
    fn cli_parses_all_subcommands() {
        let cases = [
            vec!["fellowship-ctl", "heartbeat", "pm", "--status", "alive"],
            vec!["fellowship-ctl", "log", "pm", "creating bead"],
            vec!["fellowship-ctl", "spawn-engineer"],
            vec![
                "fellowship-ctl",
                "spawn-engineer",
                "--branch",
                "feat/bead-1",
                "--single-shot",
            ],
            vec!["fellowship-ctl", "release-engineer", "engineer-1"],
            vec!["fellowship-ctl", "pr-comments", "42"],
            vec![
                "fellowship-ctl",
                "pr-comments",
                "42",
                "--repo",
                "owner/name",
            ],
            vec!["fellowship-ctl", "bead", "list"],
            vec!["fellowship-ctl", "bead", "update", "bd-1", "--claim"],
        ];
        for args in cases {
            assert!(
                Cli::try_parse_from(&args).is_ok(),
                "failed to parse: {:?}",
                args
            );
        }
    }
}

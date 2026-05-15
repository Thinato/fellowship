//! Shared runtime types and path resolution.
//!
//! Used by both `fellowship` (which watches the runtime dir via `notify`) and
//! `fellowship-ctl` (which writes into it). All JSON shapes here are the
//! cross-binary contract; changing them is a breaking change.
//!
//! See `docs/plans/agentic-ui-v1.md` §3.3 for the full design.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const STATE_DIR: &str = "state";
pub const SPAWN_REQUEST_DIR: &str = "spawn-requests";
pub const RELEASE_REQUEST_DIR: &str = "release-requests";
pub const JOURNAL_FILE: &str = "journal.ndjson";
/// Per-agent persistent state files (see `agents::state::AgentState`).
pub const AGENT_STATE_DIR: &str = "agent_state";
/// Bus-tick nudge files watched by `agents::watcher`. One file per recipient
/// agent, written by other agents after cross-agent `bd update` calls.
pub const BUS_TICK_DIR: &str = "bus-tick";

/// Per-agent heartbeat record. Written by `fellowship-ctl heartbeat`,
/// read by fellowship's watcher.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HeartbeatRecord {
    pub agent_id: String,
    pub last_seen_ms: u128,
    pub status: String,
}

/// Engineer-spawn request emitted by Orchestrator (or any caller).
/// Fellowship's watcher consumes one of these by allocating a fresh
/// `engineer-K`, creating the worktree, and spawning the PTY.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SpawnRequest {
    pub request_id: String,
    pub branch: Option<String>,
    pub single_shot: bool,
    pub requested_at_ms: u128,
}

/// Engineer-release request. Fellowship reaps the PTY (worktree stays).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReleaseRequest {
    pub request_id: String,
    pub agent_id: String,
    pub requested_at_ms: u128,
}

/// One line of the agent activity journal.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct JournalEntry {
    pub ts_ms: u128,
    pub agent_id: String,
    pub message: String,
}

pub fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

/// Resolve the fellowship runtime directory.
///
/// Order:
/// 1. `FELLOWSHIP_RUNTIME_DIR` — explicit override (used by tests).
/// 2. `~/.fellowship/runtime/$FELLOWSHIP_SESSION` — fellowship sets the env var
///    when spawning PTYs; agents inherit it.
/// 3. `~/.fellowship/runtime/$(read CURRENT_SESSION)` — fallback for shells
///    started outside fellowship (e.g. user running `fellowship-ctl` from a
///    second terminal). The running fellowship instance writes its uuid to
///    `~/.fellowship/runtime/CURRENT_SESSION` on boot.
/// 4. `~/.fellowship/runtime/default` — last-ditch fallback when no fellowship
///    is running.
pub fn runtime_dir() -> Result<PathBuf> {
    if let Some(p) = std::env::var_os("FELLOWSHIP_RUNTIME_DIR") {
        return Ok(PathBuf::from(p));
    }
    let home =
        std::env::var_os("HOME").ok_or_else(|| anyhow::anyhow!("HOME env var is not set"))?;
    let session = std::env::var("FELLOWSHIP_SESSION")
        .ok()
        .or_else(read_current_session)
        .unwrap_or_else(|| "default".to_string());
    Ok(PathBuf::from(home)
        .join(".fellowship")
        .join("runtime")
        .join(session))
}

/// Path to the marker file that records the currently-running fellowship
/// session uuid. Lives at `~/.fellowship/runtime/CURRENT_SESSION`.
pub fn current_session_marker_path() -> Result<PathBuf> {
    let home =
        std::env::var_os("HOME").ok_or_else(|| anyhow::anyhow!("HOME env var is not set"))?;
    Ok(PathBuf::from(home)
        .join(".fellowship")
        .join("runtime")
        .join("CURRENT_SESSION"))
}

pub fn read_current_session() -> Option<String> {
    let path = current_session_marker_path().ok()?;
    let text = std::fs::read_to_string(&path).ok()?;
    let s = text.trim().to_string();
    if s.is_empty() { None } else { Some(s) }
}

pub fn write_current_session(session: &str) -> Result<()> {
    let path = current_session_marker_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("mkdir -p {}", parent.display()))?;
    }
    std::fs::write(&path, session).with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

/// Remove the CURRENT_SESSION marker only if it still points at `expected`.
/// Avoids racing with a second fellowship instance that may have started
/// between our boot and quit.
pub fn clear_current_session(expected: &str) -> Result<()> {
    let path = current_session_marker_path()?;
    if let Ok(text) = std::fs::read_to_string(&path)
        && text.trim() == expected
    {
        let _ = std::fs::remove_file(&path);
    }
    Ok(())
}

pub fn ensure_subdir(root: &Path, name: &str) -> Result<PathBuf> {
    let p = root.join(name);
    fs::create_dir_all(&p).with_context(|| format!("mkdir -p {}", p.display()))?;
    Ok(p)
}

/// Directory holding per-agent persistent state JSON files. The directory
/// itself is created lazily by `AgentState::save`.
pub fn agent_state_dir(root: &Path) -> PathBuf {
    root.join(AGENT_STATE_DIR)
}

/// Path to one agent's state file. `member_label` is the stable label such as
/// `pm`, `architect`, `engineer-1` (see `MemberId::label`).
pub fn agent_state_path(root: &Path, member_label: &str) -> PathBuf {
    agent_state_dir(root).join(format!("{}.json", member_label))
}

/// Path to one agent's decision-log notes file. Lives next to the state JSON
/// but inside a per-agent subdirectory so future per-agent artifacts (logs,
/// cursors) have a home.
pub fn agent_notes_path(root: &Path, member_label: &str) -> PathBuf {
    agent_state_dir(root).join(member_label).join("notes.md")
}

/// Directory holding bus-tick nudge files. One file per recipient agent.
/// Writes here are observed by `agents::watcher` and converted into
/// `Event::AgentNudge`.
pub fn bus_tick_dir(root: &Path) -> PathBuf {
    root.join(BUS_TICK_DIR)
}

/// Path to one agent's bus-tick file. The recipient agent's PTY receives a
/// `[ping]` line when this file is modified.
pub fn bus_tick_path(root: &Path, member_label: &str) -> PathBuf {
    bus_tick_dir(root).join(format!("{}.tick", member_label))
}

/// Write a `SpawnRequest` JSON file under `<root>/spawn-requests/<uuid>.json`.
/// Returns `(path, request_id)`. Used by both `fellowship-ctl spawn-engineer`
/// and the native fellowship-side Orchestrator (Phase 12) to enqueue
/// engineer spawns; fellowship's runtime watcher consumes the file.
pub fn write_spawn_request(
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

pub fn journal_path(root: &Path) -> PathBuf {
    root.join(JOURNAL_FILE)
}

/// Read the journal file and parse every NDJSON line. Best-effort: malformed
/// lines are skipped silently. Returns an empty vec if the file is absent.
pub fn read_journal(root: &Path) -> Result<Vec<JournalEntry>> {
    let path = journal_path(root);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let bytes = fs::read(&path).with_context(|| format!("read {}", path.display()))?;
    let text = String::from_utf8_lossy(&bytes);
    Ok(text
        .lines()
        .filter_map(|line| serde_json::from_str::<JournalEntry>(line).ok())
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn runtime_dir_honors_explicit_override_env_var() {
        let tmp = TempDir::new().unwrap();
        // SAFETY: cargo runs each test in its own thread but tests within a
        // file may run in parallel. Env mutation is technically unsafe in
        // edition 2024; this single-test isolation is acceptable for now.
        unsafe {
            std::env::set_var("FELLOWSHIP_RUNTIME_DIR", tmp.path());
        }
        let resolved = runtime_dir().unwrap();
        assert_eq!(resolved, tmp.path());
        unsafe {
            std::env::remove_var("FELLOWSHIP_RUNTIME_DIR");
        }
    }

    #[test]
    fn ensure_subdir_creates_missing_path() {
        let tmp = TempDir::new().unwrap();
        let p = ensure_subdir(tmp.path(), "state").unwrap();
        assert!(p.is_dir());
    }

    #[test]
    fn agent_state_paths_are_under_runtime_root() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let state = agent_state_path(root, "engineer-1");
        let notes = agent_notes_path(root, "engineer-1");
        let dir = agent_state_dir(root);
        assert!(state.starts_with(root));
        assert!(notes.starts_with(root));
        assert!(dir.starts_with(root));
        assert_eq!(state.file_name().unwrap(), "engineer-1.json");
        assert_eq!(notes.file_name().unwrap(), "notes.md");
        assert!(
            notes.parent().unwrap().ends_with("agent_state/engineer-1"),
            "notes path should live inside per-agent subdir, got {}",
            notes.display()
        );
    }

    #[test]
    fn bus_tick_path_uses_recipient_label() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let tick = bus_tick_path(root, "pm");
        assert!(tick.starts_with(root));
        assert_eq!(tick.file_name().unwrap(), "pm.tick");
        assert!(tick.parent().unwrap().ends_with("bus-tick"));
    }

    #[test]
    fn heartbeat_record_roundtrips_through_json() {
        let record = HeartbeatRecord {
            agent_id: "engineer-1".into(),
            last_seen_ms: 12345,
            status: "claiming bd-1".into(),
        };
        let s = serde_json::to_string(&record).unwrap();
        let parsed: HeartbeatRecord = serde_json::from_str(&s).unwrap();
        assert_eq!(record, parsed);
    }
}

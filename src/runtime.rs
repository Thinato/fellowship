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

pub const STATE_DIR: &str = "state";
pub const SPAWN_REQUEST_DIR: &str = "spawn-requests";
pub const RELEASE_REQUEST_DIR: &str = "release-requests";
pub const JOURNAL_FILE: &str = "journal.ndjson";

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

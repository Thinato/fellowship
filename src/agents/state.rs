//! Per-agent persistent state for resurrection across PTY restarts.
//!
//! Each agent owns a JSON record at
//! `<runtime_dir>/agent_state/<member_label>.json`. The record captures the
//! claude/codex session id (so a restart can use `--resume <id>`), the list of
//! beads the agent is currently working on, and the exponential-backoff
//! schedule that the watchdog uses to decide when to re-spawn after a crash.
//!
//! The file is the source of truth across process boundaries. Writes go
//! through `save`, which performs an atomic temp-file + rename so that a
//! crash mid-write never leaves a torn JSON document.
//!
//! See `.omc/plans/agent-comms-resurrection.md` for the broader design.
//!
//! Note: this module intentionally stores `member_id` and `role` as plain
//! strings rather than the in-process `MemberId`/`Role` enums. That keeps the
//! on-disk contract decoupled from the live enum shape, which is important
//! because the state file is read across fellowship restarts.

use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::runtime;
use crate::surface::MemberId;

/// Backoff schedule (seconds) indexed by `restart_attempts`. Attempts beyond
/// the last entry stay at the final value (the 5-minute cap).
pub const BACKOFF_SECS: &[u64] = &[5, 15, 45, 120, 300];

/// One on-disk record per agent. Lives at
/// `agent_state/<member_label>.json` under the active runtime directory.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentState {
    /// Stable label such as `pm`, `architect`, `engineer-1`.
    pub member_id: String,
    /// Role label (`pm`, `architect`, `recon`, `engineer`, `orchestrator`).
    pub role: String,
    /// Which CLI is driving this agent (`claude`, `codex`, ...). Determines
    /// how the restart command rebuilds its argv with `--resume <id>`.
    pub cli: String,
    /// Captured from the CLI's startup banner. None until we observe it.
    pub session_id: Option<String>,
    /// Beads the agent currently has open. Used to rebuild a recovery prompt
    /// when `session_id` is unavailable.
    #[serde(default)]
    pub assigned_beads: Vec<String>,
    /// Highest bead-update timestamp the agent has already read. Used by the
    /// agent's polling loop to detect new notes since last cursor.
    #[serde(default)]
    pub last_bead_cursor_ms: Option<u64>,
    /// Monotonically increasing across restarts; never resets in this version.
    /// The plan deliberately drops the old `max_restarts: 3` hard cap.
    #[serde(default)]
    pub restart_attempts: u32,
    /// Earliest wall-clock millisecond at which the watchdog may re-spawn this
    /// agent. None means "respawn immediately on the next watchdog tick".
    #[serde(default)]
    pub next_restart_at_ms: Option<u64>,
    /// Epoch millis when the most recent live PTY was spawned.
    pub spawned_at_ms: u64,
}

impl AgentState {
    /// Build a fresh state for a newly spawned agent.
    pub fn new(member_id: &MemberId, cli: impl Into<String>, spawned_at_ms: u64) -> Self {
        Self {
            member_id: member_id.label(),
            role: member_id.role.as_str().to_string(),
            cli: cli.into(),
            session_id: None,
            assigned_beads: Vec::new(),
            last_bead_cursor_ms: None,
            restart_attempts: 0,
            next_restart_at_ms: None,
            spawned_at_ms,
        }
    }

    /// Load the on-disk record for `member_label` under `runtime_dir`.
    /// Returns `Ok(None)` when the file is absent — the caller decides whether
    /// to treat that as "never spawned" or "fresh slate".
    pub fn load(runtime_dir: &std::path::Path, member_label: &str) -> Result<Option<Self>> {
        let path = runtime::agent_state_path(runtime_dir, member_label);
        if !path.exists() {
            return Ok(None);
        }
        let bytes = fs::read(&path).with_context(|| format!("read {}", path.display()))?;
        let parsed: Self =
            serde_json::from_slice(&bytes).with_context(|| format!("parse {}", path.display()))?;
        Ok(Some(parsed))
    }

    /// Atomically persist this state. Creates the parent directory if missing.
    /// Writes to `<path>.tmp` then renames over the target so a crash mid-write
    /// cannot produce a torn JSON file. Rename is atomic on the same filesystem
    /// (which is always the case under the runtime dir).
    pub fn save(&self, runtime_dir: &std::path::Path) -> Result<PathBuf> {
        let path = runtime::agent_state_path(runtime_dir, &self.member_id);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("mkdir -p {}", parent.display()))?;
        }
        // Disambiguate concurrent writers within the same process by including
        // the thread id in the tmp filename. A single rename still wins.
        let tmp = path.with_extension(format!("tmp.{:?}", std::thread::current().id()));
        let bytes = serde_json::to_vec_pretty(self).context("serialize AgentState")?;
        fs::write(&tmp, &bytes).with_context(|| format!("write {}", tmp.display()))?;
        fs::rename(&tmp, &path)
            .with_context(|| format!("rename {} -> {}", tmp.display(), path.display()))?;
        Ok(path)
    }

    /// Increment the restart counter and stamp `next_restart_at_ms` according
    /// to the backoff schedule. Returns the delay in seconds that was applied.
    /// Caller is responsible for calling `save` afterwards.
    pub fn bump_restart(&mut self, now_ms: u64) -> u64 {
        self.restart_attempts = self.restart_attempts.saturating_add(1);
        let idx = (self.restart_attempts as usize).saturating_sub(1);
        let delay = BACKOFF_SECS
            .get(idx)
            .copied()
            .unwrap_or_else(|| *BACKOFF_SECS.last().expect("schedule non-empty"));
        self.next_restart_at_ms = Some(now_ms.saturating_add(delay.saturating_mul(1_000)));
        delay
    }

    /// Mark the agent as healthy: clear restart counter + pending backoff.
    /// Called once a freshly restarted agent emits its first heartbeat.
    pub fn clear_restart(&mut self) {
        self.restart_attempts = 0;
        self.next_restart_at_ms = None;
    }

    /// Record a freshly observed CLI session id.
    pub fn set_session_id(&mut self, session_id: impl Into<String>) {
        self.session_id = Some(session_id.into());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::surface::Role;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};
    use tempfile::TempDir;

    fn engineer_1() -> MemberId {
        MemberId::engineer(1)
    }

    #[test]
    fn round_trip_serialize_deserialize() {
        let tmp = TempDir::new().unwrap();
        let mut state = AgentState::new(&engineer_1(), "claude", 1_715_000_000_000);
        state.set_session_id("abc-123");
        state.assigned_beads.push("beads-42".into());
        state.last_bead_cursor_ms = Some(1_715_000_001_000);
        state.save(tmp.path()).unwrap();
        let loaded = AgentState::load(tmp.path(), "engineer-1").unwrap().unwrap();
        assert_eq!(loaded, state);
    }

    #[test]
    fn missing_file_returns_none() {
        let tmp = TempDir::new().unwrap();
        let got = AgentState::load(tmp.path(), "pm").unwrap();
        assert!(got.is_none());
    }

    #[test]
    fn save_creates_parent_dir() {
        let tmp = TempDir::new().unwrap();
        let state = AgentState::new(&MemberId::singleton(Role::Pm), "claude", 100);
        let path = state.save(tmp.path()).unwrap();
        assert!(path.exists(), "state file should exist after save");
        assert!(
            path.parent().unwrap().is_dir(),
            "parent dir should be created"
        );
    }

    #[test]
    fn bump_restart_follows_backoff_schedule() {
        let now = 1_000_000_u64;
        let mut state = AgentState::new(&engineer_1(), "claude", now);
        let mut delays = Vec::new();
        for _ in 0..6 {
            delays.push(state.bump_restart(now));
        }
        // First five attempts pull from the schedule; the sixth caps at 300 s.
        assert_eq!(delays, vec![5, 15, 45, 120, 300, 300]);
        // next_restart_at_ms is in millis from now.
        assert_eq!(
            state.next_restart_at_ms,
            Some(now + 300 * 1_000),
            "final next_restart_at_ms should reflect capped 300 s delay"
        );
        assert_eq!(state.restart_attempts, 6);
    }

    #[test]
    fn clear_restart_resets_counter_and_timer() {
        let mut state = AgentState::new(&engineer_1(), "claude", 0);
        state.bump_restart(0);
        state.bump_restart(0);
        assert_eq!(state.restart_attempts, 2);
        state.clear_restart();
        assert_eq!(state.restart_attempts, 0);
        assert!(state.next_restart_at_ms.is_none());
    }

    #[test]
    fn concurrent_saves_leave_a_valid_file() {
        // Two threads stamp updated states for the same agent at the same time.
        // The atomic temp+rename guarantees the final on-disk JSON is parseable
        // and equal to one of the two written states.
        let tmp = Arc::new(TempDir::new().unwrap());
        let counter = Arc::new(AtomicU32::new(0));
        let mut handles = Vec::new();
        for tid in 0..2u32 {
            let tmp = Arc::clone(&tmp);
            let counter = Arc::clone(&counter);
            handles.push(std::thread::spawn(move || {
                let mut state = AgentState::new(&engineer_1(), "claude", 1_000);
                // Distinct mutation per writer so we can verify which one won.
                state.assigned_beads.push(format!("beads-{}", tid));
                state.save(tmp.path()).unwrap();
                counter.fetch_add(1, Ordering::SeqCst);
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        let loaded = AgentState::load(tmp.path(), "engineer-1").unwrap().unwrap();
        assert_eq!(loaded.member_id, "engineer-1");
        assert!(
            loaded.assigned_beads == vec!["beads-0".to_string()]
                || loaded.assigned_beads == vec!["beads-1".to_string()],
            "final state should match one of the two writers, got {:?}",
            loaded.assigned_beads
        );
        assert_eq!(counter.load(Ordering::SeqCst), 2);
    }
}

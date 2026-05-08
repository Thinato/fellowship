//! In-memory mirror of the on-disk agent state. Phase 5 stores the latest
//! [`HeartbeatRecord`] per agent id; Phase 11 will layer the STALE/DEAD state
//! machine on top using elapsed time vs. configured thresholds.

use std::collections::HashMap;
use std::path::Path;

use anyhow::Result;

use crate::runtime::HeartbeatRecord;

/// Default heartbeat-age thresholds (Phase 11 hardcodes; Phase 10.5+ wires
/// from `Config::agents.heartbeat_*_sec`). Plan §3.6 specifies these values.
pub const HEARTBEAT_WARN_SEC: u64 = 60;
pub const HEARTBEAT_DEAD_SEC: u64 = 180;

/// Liveness derived from the most recent heartbeat for an agent id. Computed
/// per `liveness_for` call from `last_seen_ms` against the thresholds above.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Liveness {
    /// At least one heartbeat in the last `HEARTBEAT_WARN_SEC`.
    Live,
    /// Last heartbeat is older than `HEARTBEAT_WARN_SEC` but within
    /// `HEARTBEAT_DEAD_SEC`. Watchdog warns; no restart yet.
    Stale,
    /// Last heartbeat older than `HEARTBEAT_DEAD_SEC`. Watchdog restarts.
    Dead,
    /// No heartbeat record at all. Common at boot for placeholder/banner
    /// PTYs that don't write heartbeats; the watchdog treats this as
    /// "no telemetry — leave alone" rather than an alarm.
    Unknown,
}

#[derive(Debug, Default)]
pub struct AgentRegistry {
    records: HashMap<String, HeartbeatRecord>,
}

impl AgentRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn upsert(&mut self, record: HeartbeatRecord) {
        self.records.insert(record.agent_id.clone(), record);
    }

    pub fn get(&self, agent_id: &str) -> Option<&HeartbeatRecord> {
        self.records.get(agent_id)
    }

    /// Drop the heartbeat record for `agent_id`. Used by the watchdog after
    /// a restart so the new PTY isn't immediately re-judged "Dead" against
    /// the previous (stale) heartbeat — `liveness_for` will return
    /// `Unknown` until the restarted agent produces its first heartbeat.
    pub fn clear(&mut self, agent_id: &str) {
        self.records.remove(agent_id);
    }

    /// Derive `Liveness` for an agent given the current wall-clock time in
    /// epoch milliseconds. `now_ms` is supplied by the caller so tests can
    /// drive deterministic state transitions without waiting in real time.
    pub fn liveness_for(&self, agent_id: &str, now_ms: u128) -> Liveness {
        let Some(rec) = self.records.get(agent_id) else {
            return Liveness::Unknown;
        };
        let elapsed_sec = now_ms.saturating_sub(rec.last_seen_ms) / 1000;
        if elapsed_sec >= HEARTBEAT_DEAD_SEC as u128 {
            Liveness::Dead
        } else if elapsed_sec >= HEARTBEAT_WARN_SEC as u128 {
            Liveness::Stale
        } else {
            Liveness::Live
        }
    }

    /// Scan a state directory once and ingest every `*.json` file. Used at
    /// fellowship boot so the Members pane shows existing heartbeats before
    /// the watcher starts emitting events.
    pub fn load_from_state_dir(&mut self, state_dir: &Path) -> Result<usize> {
        let mut count = 0usize;
        if !state_dir.is_dir() {
            return Ok(count);
        }
        for entry in std::fs::read_dir(state_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            let bytes = std::fs::read(&path)?;
            if let Ok(record) = serde_json::from_slice::<HeartbeatRecord>(&bytes) {
                self.upsert(record);
                count += 1;
            }
        }
        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn record(agent_id: &str, status: &str, ts: u128) -> HeartbeatRecord {
        HeartbeatRecord {
            agent_id: agent_id.into(),
            last_seen_ms: ts,
            status: status.into(),
        }
    }

    #[test]
    fn upsert_replaces_existing_record() {
        let mut reg = AgentRegistry::new();
        reg.upsert(record("pm", "first", 100));
        reg.upsert(record("pm", "second", 200));
        let r = reg.get("pm").unwrap();
        assert_eq!(r.status, "second");
        assert_eq!(r.last_seen_ms, 200);
    }

    #[test]
    fn get_returns_none_for_unknown() {
        let reg = AgentRegistry::new();
        assert!(reg.get("ghost").is_none());
    }

    #[test]
    fn load_from_state_dir_ingests_existing_json_files() {
        let tmp = TempDir::new().unwrap();
        let state = tmp.path().join("state");
        std::fs::create_dir_all(&state).unwrap();
        for (id, status) in [("pm", "alive"), ("orchestrator", "routing")] {
            let r = record(id, status, 42);
            std::fs::write(
                state.join(format!("{}.json", id)),
                serde_json::to_vec(&r).unwrap(),
            )
            .unwrap();
        }
        // unrelated junk file ignored
        std::fs::write(state.join("notes.txt"), b"hi").unwrap();

        let mut reg = AgentRegistry::new();
        let n = reg.load_from_state_dir(&state).unwrap();
        assert_eq!(n, 2);
        assert_eq!(reg.get("pm").unwrap().status, "alive");
        assert_eq!(reg.get("orchestrator").unwrap().status, "routing");
    }

    #[test]
    fn load_from_state_dir_returns_zero_when_dir_absent() {
        let tmp = TempDir::new().unwrap();
        let mut reg = AgentRegistry::new();
        let n = reg.load_from_state_dir(&tmp.path().join("nope")).unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn clear_removes_record_so_liveness_becomes_unknown() {
        let mut reg = AgentRegistry::new();
        reg.upsert(record("pm", "alive", 0));
        // Before clear: would be Dead at distant `now_ms`.
        let dead_ms = (HEARTBEAT_DEAD_SEC as u128) * 1000;
        assert_eq!(reg.liveness_for("pm", dead_ms), Liveness::Dead);
        reg.clear("pm");
        assert!(reg.get("pm").is_none());
        assert_eq!(reg.liveness_for("pm", dead_ms), Liveness::Unknown);
        // Idempotent.
        reg.clear("pm");
        reg.clear("never-existed");
    }

    #[test]
    fn liveness_for_unknown_when_no_record() {
        let reg = AgentRegistry::new();
        assert_eq!(reg.liveness_for("ghost", 0), Liveness::Unknown);
    }

    #[test]
    fn liveness_for_live_when_within_warn_threshold() {
        let mut reg = AgentRegistry::new();
        // last_seen at 1_000_000 ms; now at 1_000_000 + 30_000 (30s elapsed).
        reg.upsert(record("pm", "alive", 1_000_000));
        assert_eq!(reg.liveness_for("pm", 1_030_000), Liveness::Live);
    }

    #[test]
    fn liveness_for_stale_at_warn_threshold() {
        let mut reg = AgentRegistry::new();
        reg.upsert(record("pm", "alive", 0));
        let warn_ms = (HEARTBEAT_WARN_SEC as u128) * 1000;
        assert_eq!(reg.liveness_for("pm", warn_ms), Liveness::Stale);
    }

    #[test]
    fn liveness_for_dead_at_dead_threshold() {
        let mut reg = AgentRegistry::new();
        reg.upsert(record("pm", "alive", 0));
        let dead_ms = (HEARTBEAT_DEAD_SEC as u128) * 1000;
        assert_eq!(reg.liveness_for("pm", dead_ms), Liveness::Dead);
        // Long past dead threshold also still Dead.
        assert_eq!(reg.liveness_for("pm", dead_ms * 10), Liveness::Dead);
    }
}

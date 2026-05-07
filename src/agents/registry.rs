//! In-memory mirror of the on-disk agent state. Phase 5 stores the latest
//! [`HeartbeatRecord`] per agent id; Phase 11 will layer the STALE/DEAD state
//! machine on top using elapsed time vs. configured thresholds.

use std::collections::HashMap;
use std::path::Path;

use anyhow::Result;

use crate::runtime::HeartbeatRecord;

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
}

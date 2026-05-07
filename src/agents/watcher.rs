//! Watches the fellowship runtime directory for heartbeats and spawn requests
//! and forwards them as fellowship events.
//!
//! Phase 5 wires only the heartbeat path; Phase 9 will consume spawn requests
//! by spawning engineer PTYs.

use std::path::Path;

use anyhow::{Context, Result};
use notify::{Event as NotifyEvent, EventKind, RecursiveMode, Watcher};
use tokio::sync::mpsc::UnboundedSender;

use crate::event::Event;
use crate::runtime::{HeartbeatRecord, STATE_DIR};

/// Spawn a notify watcher on `<runtime_root>/state/`. Each heartbeat write or
/// rewrite emits exactly one `Event::AgentHeartbeat` on `tx`.
///
/// Returns the watcher; drop it to stop watching. Callers should keep it
/// alive in `App` so it lives as long as the TUI session.
pub fn spawn_state_watcher(
    runtime_root: &Path,
    tx: UnboundedSender<Event>,
) -> Result<notify::RecommendedWatcher> {
    let state_dir = runtime_root.join(STATE_DIR);
    std::fs::create_dir_all(&state_dir)
        .with_context(|| format!("mkdir -p {}", state_dir.display()))?;

    let mut watcher = notify::recommended_watcher(move |res: notify::Result<NotifyEvent>| {
        let Ok(event) = res else { return };
        if !is_heartbeat_event(&event) {
            return;
        }
        for path in &event.paths {
            if let Some(record) = read_heartbeat(path)
                && tx.send(Event::AgentHeartbeat(record)).is_err()
            {
                // Receiver dropped; nothing to do.
                return;
            }
        }
    })
    .context("creating notify watcher")?;

    watcher
        .watch(&state_dir, RecursiveMode::NonRecursive)
        .with_context(|| format!("watching {}", state_dir.display()))?;

    Ok(watcher)
}

fn is_heartbeat_event(event: &NotifyEvent) -> bool {
    matches!(
        event.kind,
        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Any
    ) && event.paths.iter().any(|p| is_json_file(p))
}

fn is_json_file(path: &Path) -> bool {
    path.extension().and_then(|s| s.to_str()) == Some("json")
}

fn read_heartbeat(path: &Path) -> Option<HeartbeatRecord> {
    let bytes = std::fs::read(path).ok()?;
    serde_json::from_slice::<HeartbeatRecord>(&bytes).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::TempDir;

    #[test]
    fn read_heartbeat_returns_record_for_valid_json() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("pm.json");
        let record = HeartbeatRecord {
            agent_id: "pm".into(),
            last_seen_ms: 1,
            status: "alive".into(),
        };
        std::fs::write(&path, serde_json::to_vec(&record).unwrap()).unwrap();
        assert_eq!(read_heartbeat(&path), Some(record));
    }

    #[test]
    fn read_heartbeat_returns_none_for_garbage() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("junk.json");
        std::fs::write(&path, b"not json").unwrap();
        assert!(read_heartbeat(&path).is_none());
    }

    #[test]
    fn is_json_file_filters_extensions() {
        assert!(is_json_file(&PathBuf::from("/x/pm.json")));
        assert!(!is_json_file(&PathBuf::from("/x/pm.txt")));
        assert!(!is_json_file(&PathBuf::from("/x/pm")));
    }
}

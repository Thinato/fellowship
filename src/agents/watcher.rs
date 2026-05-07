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
use crate::runtime::{HeartbeatRecord, JOURNAL_FILE, STATE_DIR, journal_path, read_journal};

/// Spawn a notify watcher rooted at `runtime_root`. Watches:
/// - `<runtime_root>/state/` — every heartbeat JSON write fires
///   `Event::AgentHeartbeat`.
/// - `<runtime_root>/journal.ndjson` — every append fires
///   `Event::JournalSnapshot` with the full re-parsed contents.
///
/// Returns the watcher; drop it to stop watching. Callers should keep it
/// alive in `App` (or `main::run`) so it lives as long as the TUI session.
pub fn spawn_runtime_watcher(
    runtime_root: &Path,
    tx: UnboundedSender<Event>,
) -> Result<notify::RecommendedWatcher> {
    let state_dir = runtime_root.join(STATE_DIR);
    std::fs::create_dir_all(&state_dir)
        .with_context(|| format!("mkdir -p {}", state_dir.display()))?;
    // Touch the journal file so notify has something to watch from t=0.
    let journal = journal_path(runtime_root);
    if !journal.exists() {
        std::fs::write(&journal, b"").with_context(|| format!("touch {}", journal.display()))?;
    }

    let runtime_root_owned = runtime_root.to_path_buf();
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<NotifyEvent>| {
        let Ok(event) = res else { return };
        if !matches!(
            event.kind,
            EventKind::Create(_) | EventKind::Modify(_) | EventKind::Any
        ) {
            return;
        }
        let mut journal_dirty = false;
        for path in &event.paths {
            if path.ends_with(JOURNAL_FILE) {
                journal_dirty = true;
            } else if is_json_file(path)
                && let Some(record) = read_heartbeat(path)
                && tx.send(Event::AgentHeartbeat(record)).is_err()
            {
                return;
            }
        }
        if journal_dirty
            && let Ok(entries) = read_journal(&runtime_root_owned)
            && tx.send(Event::JournalSnapshot(entries)).is_err()
        {
            // Receiver dropped; nothing to do.
        }
    })
    .context("creating notify watcher")?;

    watcher
        .watch(&state_dir, RecursiveMode::NonRecursive)
        .with_context(|| format!("watching {}", state_dir.display()))?;
    watcher
        .watch(&journal, RecursiveMode::NonRecursive)
        .with_context(|| format!("watching {}", journal.display()))?;

    Ok(watcher)
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

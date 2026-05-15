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
use crate::runtime::{
    BUS_TICK_DIR, HeartbeatRecord, JOURNAL_FILE, RELEASE_REQUEST_DIR, ReleaseRequest,
    SPAWN_REQUEST_DIR, STATE_DIR, SpawnRequest, journal_path, read_journal,
};
use crate::surface::{MemberId, Role};

/// Spawn a notify watcher rooted at `runtime_root`. Watches:
/// - `<runtime_root>/state/` — heartbeats → `Event::AgentHeartbeat`.
/// - `<runtime_root>/journal.ndjson` — log appends → `Event::JournalSnapshot`.
/// - `<runtime_root>/spawn-requests/` — spawn intents →
///   `Event::SpawnRequestReceived`. The intent file is consumed (deleted)
///   before the event fires so duplicate notify events don't replay.
/// - `<runtime_root>/release-requests/` — release intents →
///   `Event::ReleaseRequestReceived` (consumed identically).
///
/// Returns the watcher; drop it to stop watching. Callers should keep it
/// alive in `App` (or `main::run`) so it lives as long as the TUI session.
pub fn spawn_runtime_watcher(
    runtime_root: &Path,
    tx: UnboundedSender<Event>,
) -> Result<notify::RecommendedWatcher> {
    let state_dir = runtime_root.join(STATE_DIR);
    let spawn_dir = runtime_root.join(SPAWN_REQUEST_DIR);
    let release_dir = runtime_root.join(RELEASE_REQUEST_DIR);
    let bus_tick_dir = runtime_root.join(BUS_TICK_DIR);
    for d in [&state_dir, &spawn_dir, &release_dir, &bus_tick_dir] {
        std::fs::create_dir_all(d).with_context(|| format!("mkdir -p {}", d.display()))?;
    }
    // Prune bus-tick files older than 10 minutes. Stale ticks left over from
    // a prior crash would otherwise fire AgentNudge storms on the next watcher
    // boot. Best-effort; failures here are logged-and-ignored.
    prune_stale_bus_ticks(&bus_tick_dir, std::time::Duration::from_secs(600));
    // Touch the journal file so notify has something to watch from t=0.
    let journal = journal_path(runtime_root);
    if !journal.exists() {
        std::fs::write(&journal, b"").with_context(|| format!("touch {}", journal.display()))?;
    }

    let runtime_root_owned = runtime_root.to_path_buf();
    let spawn_dir_owned = spawn_dir.clone();
    let release_dir_owned = release_dir.clone();
    let bus_tick_dir_owned = bus_tick_dir.clone();
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
                continue;
            }
            // Bus-tick files use `.tick` extension. Handle them before the
            // is_json_file gate so a fast-path emit doesn't require parsing.
            if path.starts_with(&bus_tick_dir_owned)
                && path.extension().and_then(|s| s.to_str()) == Some("tick")
                && let Some(member) = parse_member_label_from_tick_path(path)
                && tx.send(Event::AgentNudge(member)).is_err()
            {
                return;
            }
            if !is_json_file(path) {
                continue;
            }
            if path.starts_with(&spawn_dir_owned) {
                if let Some(req) = consume_spawn_request(path)
                    && tx.send(Event::SpawnRequestReceived(req)).is_err()
                {
                    return;
                }
            } else if path.starts_with(&release_dir_owned) {
                if let Some(req) = consume_release_request(path)
                    && tx.send(Event::ReleaseRequestReceived(req)).is_err()
                {
                    return;
                }
            } else if let Some(record) = read_heartbeat(path)
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
    watcher
        .watch(&spawn_dir, RecursiveMode::NonRecursive)
        .with_context(|| format!("watching {}", spawn_dir.display()))?;
    watcher
        .watch(&release_dir, RecursiveMode::NonRecursive)
        .with_context(|| format!("watching {}", release_dir.display()))?;
    watcher
        .watch(&bus_tick_dir, RecursiveMode::NonRecursive)
        .with_context(|| format!("watching {}", bus_tick_dir.display()))?;

    Ok(watcher)
}

/// Parse a tick filename like `engineer-1.tick`, `pm.tick`, `architect.tick`
/// back into a `MemberId`. Returns `None` for unknown role labels so the
/// watcher silently drops stray files (e.g. accidental touches).
fn parse_member_label_from_tick_path(path: &Path) -> Option<MemberId> {
    let stem = path.file_stem().and_then(|s| s.to_str())?;
    if let Some(rest) = stem.strip_prefix("engineer-") {
        let n: u32 = rest.parse().ok()?;
        return Some(MemberId::engineer(n));
    }
    let role = match stem {
        "pm" => Role::Pm,
        "architect" => Role::Architect,
        "recon" => Role::Recon,
        "orchestrator" => Role::Orchestrator,
        _ => return None,
    };
    Some(MemberId::singleton(role))
}

/// Read + delete a spawn-request file in one shot. Returns the parsed request
/// if both succeed. The delete-after-parse pattern guarantees a request is
/// processed at-most-once even if notify fires multiple events for the same
/// write (which it sometimes does on macOS).
fn consume_spawn_request(path: &Path) -> Option<SpawnRequest> {
    let bytes = std::fs::read(path).ok()?;
    let req: SpawnRequest = serde_json::from_slice(&bytes).ok()?;
    let _ = std::fs::remove_file(path);
    Some(req)
}

fn consume_release_request(path: &Path) -> Option<ReleaseRequest> {
    let bytes = std::fs::read(path).ok()?;
    let req: ReleaseRequest = serde_json::from_slice(&bytes).ok()?;
    let _ = std::fs::remove_file(path);
    Some(req)
}

fn is_json_file(path: &Path) -> bool {
    path.extension().and_then(|s| s.to_str()) == Some("json")
}

fn prune_stale_bus_ticks(dir: &Path, max_age: std::time::Duration) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let now = std::time::SystemTime::now();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("tick") {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        let Ok(modified) = meta.modified() else {
            continue;
        };
        if now
            .duration_since(modified)
            .map(|age| age > max_age)
            .unwrap_or(false)
        {
            let _ = std::fs::remove_file(&path);
        }
    }
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

    #[test]
    fn parse_tick_path_round_trips_singleton_and_engineer_labels() {
        let pm =
            parse_member_label_from_tick_path(&PathBuf::from("/runtime/bus-tick/pm.tick")).unwrap();
        assert_eq!(pm, MemberId::singleton(Role::Pm));

        let arch =
            parse_member_label_from_tick_path(&PathBuf::from("/runtime/bus-tick/architect.tick"))
                .unwrap();
        assert_eq!(arch, MemberId::singleton(Role::Architect));

        let eng1 =
            parse_member_label_from_tick_path(&PathBuf::from("/runtime/bus-tick/engineer-1.tick"))
                .unwrap();
        assert_eq!(eng1, MemberId::engineer(1));

        let eng42 =
            parse_member_label_from_tick_path(&PathBuf::from("/runtime/bus-tick/engineer-42.tick"))
                .unwrap();
        assert_eq!(eng42, MemberId::engineer(42));

        // Stray files silently dropped.
        assert!(parse_member_label_from_tick_path(&PathBuf::from("/x/junk.tick")).is_none());
        assert!(
            parse_member_label_from_tick_path(&PathBuf::from("/x/engineer-NaN.tick")).is_none()
        );
    }
}

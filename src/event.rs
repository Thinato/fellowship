use crate::beads::Bead;
use crate::gh::PrInfo;
use crate::git::{Diff, Worktree};
use crate::runtime::{HeartbeatRecord, ReleaseRequest, SpawnRequest};
use crate::surface::Surface;
use crossterm::event::KeyEvent;
use std::path::PathBuf;

#[derive(Debug)]
#[allow(dead_code)]
pub enum Event {
    Key(KeyEvent),
    Tick,
    GitRefresh,
    PtyOutput,
    SwitchWorkspace(PathBuf),
    /// Generic surface switch. Used by the Members pane to focus a specific
    /// agent. Workspace switching keeps using `SwitchWorkspace` because it
    /// carries extra side effects (git_status root, GitRefresh, list select).
    SwitchSurface(Surface),
    /// Emitted by the runtime watcher whenever a heartbeat JSON file under
    /// `<runtime>/state/` is written. Carries the parsed record.
    AgentHeartbeat(HeartbeatRecord),
    /// Periodic tick from `main`'s 5s watchdog interval. The handler walks
    /// the live members, compares each one's last_seen heartbeat against
    /// the warn/dead thresholds, and decides whether to restart, escalate,
    /// or do nothing.
    WatchdogTick,
    /// Emitted by the runtime watcher when a `spawn-requests/<uuid>.json`
    /// intent file is written. The watcher consumes (deletes) the file
    /// before sending so the request is processed at-most-once.
    SpawnRequestReceived(SpawnRequest),
    /// Emitted by the runtime watcher when a `release-requests/<uuid>.json`
    /// intent file is written. Same at-most-once semantics as Spawn.
    ReleaseRequestReceived(ReleaseRequest),
    /// Emitted by the runtime watcher whenever `<runtime>/journal.ndjson`
    /// changes. Carries the full re-parsed list (cheaper than tracking deltas).
    JournalSnapshot(Vec<crate::runtime::JournalEntry>),
    /// Emitted by the beads poller every ~3s with the latest snapshot.
    BeadsRefreshed(Vec<Bead>),
    CreateWorktree(String),
    PromptDeleteWorktree(PathBuf, String),
    DeleteWorktree(PathBuf),
    WorktreesRefreshed(Vec<Worktree>),
    DiffUpdated(Diff),
    PrUpdated(Option<PrInfo>),
    Redraw,
    Quit,
}

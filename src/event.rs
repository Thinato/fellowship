use crate::gh::PrInfo;
use crate::git::{Diff, Worktree};
use crate::runtime::HeartbeatRecord;
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
    CreateWorktree(String),
    PromptDeleteWorktree(PathBuf, String),
    DeleteWorktree(PathBuf),
    WorktreesRefreshed(Vec<Worktree>),
    DiffUpdated(Diff),
    PrUpdated(Option<PrInfo>),
    Redraw,
    Quit,
}

use std::path::PathBuf;
use crossterm::event::KeyEvent;
use crate::git::{Diff, Worktree};
use crate::gh::PrInfo;

#[derive(Debug)]
#[allow(dead_code)]
pub enum Event {
    Key(KeyEvent),
    Tick,
    GitRefresh,
    PtyOutput,
    SwitchWorkspace(PathBuf),
    CreateWorktree(String),
    WorktreesRefreshed(Vec<Worktree>),
    DiffUpdated(Diff),
    PrUpdated(Option<PrInfo>),
    Redraw,
    Quit,
}

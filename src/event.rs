use crate::gh::PrInfo;
use crate::git::{Diff, Worktree};
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
    CreateWorktree(String),
    PromptDeleteWorktree(PathBuf, String),
    DeleteWorktree(PathBuf),
    WorktreesRefreshed(Vec<Worktree>),
    DiffUpdated(Diff),
    PrUpdated(Option<PrInfo>),
    Redraw,
    Quit,
}

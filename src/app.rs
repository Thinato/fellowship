use std::path::PathBuf;
use std::time::Instant;

use anyhow::Result;
use crossterm::event::KeyEvent;
use tokio::sync::mpsc;

use crate::event::Event;
use crate::git;
use crate::gh;
use crate::keymap::{Action, InputMode, Keymap, default_bindings};
use crate::panes::gitstatus::GitStatusPane;
use crate::panes::terminal::TerminalPane;
use crate::panes::workspaces::WorkspacesPane;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneId {
    Workspaces,
    Terminal,
    GitStatus,
}

pub struct App {
    pub focus: PaneId,
    pub input_mode: InputMode,
    pub workspaces: WorkspacesPane,
    pub terminal: TerminalPane,
    pub git_status: GitStatusPane,
    pub show_help: bool,
    pub should_quit: bool,
    pub active_path: PathBuf,
    pub event_tx: mpsc::UnboundedSender<Event>,
    keymap: Keymap,
}

impl App {
    pub fn new(
        root_path: PathBuf,
        terminal: TerminalPane,
        event_tx: mpsc::UnboundedSender<Event>,
    ) -> Self {
        Self {
            focus: PaneId::Terminal,
            input_mode: InputMode::Normal,
            workspaces: WorkspacesPane::new(root_path.clone()),
            terminal,
            git_status: GitStatusPane::new(root_path.clone()),
            show_help: false,
            should_quit: false,
            active_path: root_path,
            event_tx,
            keymap: Keymap::new(default_bindings()),
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Result<()> {
        let now = Instant::now();
        let action = self.keymap.handle(&mut self.input_mode, key, self.focus, now);

        match action {
            Action::FocusPane(pane) => {
                self.focus = pane;
            }
            Action::Quit => {
                self.should_quit = true;
            }
            Action::ToggleHelp => {
                self.show_help = !self.show_help;
            }
            Action::SendLiteralPrefix => {
                if self.focus == PaneId::Terminal {
                    use crate::panes::terminal::key_to_bytes;
                    use crossterm::event::{KeyCode, KeyModifiers};
                    let ctrl_space =
                        KeyEvent::new(KeyCode::Char(' '), KeyModifiers::CONTROL);
                    let _ = self.terminal.write_keys(&key_to_bytes::encode(ctrl_space));
                }
            }
            Action::PassThrough => {
                self.dispatch_key_to_focused_pane(key)?;
            }
            Action::Consume => {}
        }
        Ok(())
    }

    fn dispatch_key_to_focused_pane(&mut self, key: KeyEvent) -> Result<()> {
        match self.focus {
            PaneId::Terminal => {
                use crate::panes::terminal::key_to_bytes;
                let bytes = key_to_bytes::encode(key);
                if !bytes.is_empty() {
                    self.terminal.write_keys(&bytes)?;
                }
            }
            PaneId::Workspaces => {
                if let Some(event) = self.workspaces.handle_key(key) {
                    let _ = self.event_tx.send(event);
                }
            }
            PaneId::GitStatus => {
                // git status pane has no key handling currently
            }
        }
        Ok(())
    }

    pub async fn handle_event(&mut self, event: Event) -> Result<()> {
        match event {
            Event::Key(key) => {
                self.handle_key(key)?;
            }
            Event::SwitchWorkspace(path) => {
                self.active_path = path.clone();
                self.git_status.root_path = path.clone();
                let size = self.terminal.size();
                self.terminal.restart(size.0, size.1, &path, self.event_tx.clone())?;
                // Trigger immediate git + PR refresh
                let _ = self.event_tx.send(Event::GitRefresh);
            }
            Event::CreateWorktree(branch) => {
                let root = self.active_path.clone();
                let tx = self.event_tx.clone();
                tokio::spawn(async move {
                    match git::add_worktree(&root, &branch).await {
                        Ok(_) => {
                            // Refresh worktree list
                            if let Ok(wts) = git::list_worktrees(&root).await {
                                let _ = tx.send(Event::WorktreesRefreshed(wts));
                            }
                        }
                        Err(e) => {
                            eprintln!("git worktree add failed: {e}");
                        }
                    }
                });
            }
            Event::WorktreesRefreshed(worktrees) => {
                self.workspaces.set_worktrees(worktrees);
            }
            Event::GitRefresh => {
                let path = self.active_path.clone();
                let tx = self.event_tx.clone();
                tokio::spawn(async move {
                    let diff = git::diff_summary(&path).await.unwrap_or_default();
                    let branch = git::current_branch(&path).await.unwrap_or(None);
                    let pr = if let Some(ref b) = branch {
                        gh::current_pr(&path, b).await.unwrap_or(None)
                    } else {
                        None
                    };
                    let wts = git::list_worktrees(&path).await.unwrap_or_default();
                    let _ = tx.send(Event::DiffUpdated(diff));
                    let _ = tx.send(Event::PrUpdated(pr));
                    let _ = tx.send(Event::WorktreesRefreshed(wts));
                });
            }
            Event::DiffUpdated(diff) => {
                self.git_status.update_diff(diff);
            }
            Event::PrUpdated(pr) => {
                self.git_status.update_pr(pr);
            }
            Event::Redraw | Event::Tick => {}
            Event::Quit => {
                self.should_quit = true;
            }
            Event::PtyOutput => {}
        }
        Ok(())
    }

    pub fn resize_terminal(&mut self, area: ratatui::layout::Rect) {
        // area is the inner area for the terminal pane (after border)
        let rows = area.height;
        let cols = area.width;
        let _ = self.terminal.resize(rows, cols);
    }
}

use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::Result;
use crossterm::event::KeyEvent;
use tokio::sync::mpsc;

use crate::event::Event;
use crate::gh;
use crate::git;
use crate::keymap::{Action, InputMode, Keymap, default_bindings};
use crate::layout::PaneLayout;
use crate::panes::gitstatus::GitStatusPane;
use crate::panes::members::MembersPane;
use crate::panes::terminal::TerminalPane;
use crate::panes::workspaces::WorkspacesPane;
use crate::surface::Surface;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneId {
    Members,
    Workspaces,
    Terminal,
    GitStatus,
}

pub struct App {
    pub focus: PaneId,
    pub input_mode: InputMode,
    pub members: MembersPane,
    pub workspaces: WorkspacesPane,
    pub terminals: HashMap<Surface, TerminalPane>,
    pub git_status: GitStatusPane,
    pub show_help: bool,
    pub should_quit: bool,
    pub pending_delete: Option<(PathBuf, String)>,
    /// Surface currently bound to the Terminal pane. In Phase 2 this is always
    /// `Surface::Workspace(...)`; Phase 3 introduces `Surface::Member(...)`.
    pub active_surface: Surface,
    /// Path of the most recently active workspace. Workspace-bound operations
    /// (git diff, worktree create/delete) always operate against this path,
    /// even when the active surface is a Member.
    pub last_workspace_path: PathBuf,
    pub last_term_size: (u16, u16),
    pub event_tx: mpsc::UnboundedSender<Event>,
    pub layout: PaneLayout,
    pub startup_cmd: Option<String>,
    keymap: Keymap,
}

impl App {
    pub fn new(
        root_path: PathBuf,
        terminal: TerminalPane,
        event_tx: mpsc::UnboundedSender<Event>,
        startup_cmd: Option<String>,
    ) -> Self {
        let last_term_size = terminal.size();
        let mut terminals = HashMap::new();
        let initial_surface = Surface::Workspace(root_path.clone());
        terminals.insert(initial_surface.clone(), terminal);
        Self {
            focus: PaneId::Terminal,
            input_mode: InputMode::Normal,
            members: MembersPane::new(),
            workspaces: WorkspacesPane::new(root_path.clone()),
            terminals,
            git_status: GitStatusPane::new(root_path.clone()),
            show_help: false,
            should_quit: false,
            pending_delete: None,
            active_surface: initial_surface,
            last_workspace_path: root_path,
            last_term_size,
            event_tx,
            layout: PaneLayout::default_horizontal(),
            startup_cmd,
            keymap: Keymap::new(default_bindings()),
        }
    }

    fn execute_command(&mut self, cmd: &str) {
        match cmd.trim() {
            "" => {}
            "q" | "quit" => {
                self.should_quit = true;
            }
            _ => {}
        }
    }

    pub fn active_terminal_mut(&mut self) -> Option<&mut TerminalPane> {
        let key = self.active_surface.clone();
        self.terminals.get_mut(&key)
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Result<()> {
        if self.pending_delete.is_some() {
            use crossterm::event::KeyCode;
            match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                    if let Some((path, _)) = self.pending_delete.take() {
                        let _ = self.event_tx.send(Event::DeleteWorktree(path));
                    }
                }
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                    self.pending_delete = None;
                }
                _ => {}
            }
            return Ok(());
        }

        let action = self.keymap.handle(&mut self.input_mode, key, self.focus);

        match action {
            Action::FocusPane(pane) => {
                self.focus = pane;
            }
            Action::FocusDir(dir) => {
                if let Some(target) = self.layout.neighbor(self.focus, dir) {
                    self.focus = target;
                }
            }
            Action::SwitchWorktree(idx) => {
                if let Some(wt) = self.workspaces.worktrees.get(idx) {
                    let path = wt.path.clone();
                    let _ = self.event_tx.send(Event::SwitchWorkspace(path));
                }
            }
            Action::Quit => {
                self.should_quit = true;
            }
            Action::CyclePane => {
                self.focus = match self.focus {
                    PaneId::Members => PaneId::Workspaces,
                    PaneId::Workspaces => PaneId::Terminal,
                    PaneId::Terminal => PaneId::GitStatus,
                    PaneId::GitStatus => PaneId::Members,
                };
            }
            Action::ToggleHelp => {
                self.show_help = !self.show_help;
            }
            Action::EnterCommandMode => {}
            Action::ExecuteCommand(cmd) => {
                self.execute_command(&cmd);
            }
            Action::SendLiteralPrefix => {
                if self.focus == PaneId::Terminal {
                    use crate::panes::terminal::key_to_bytes;
                    use crossterm::event::{KeyCode, KeyModifiers};
                    let ctrl_a = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL);
                    if let Some(t) = self.active_terminal_mut() {
                        let _ = t.write_keys(&key_to_bytes::encode(ctrl_a));
                    }
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
                if !bytes.is_empty()
                    && let Some(t) = self.active_terminal_mut()
                {
                    t.write_keys(&bytes)?;
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
            PaneId::Members => {
                self.members.handle_key(key);
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
                let surface = Surface::Workspace(path.clone());
                self.active_surface = surface.clone();
                self.last_workspace_path = path.clone();
                self.git_status.root_path = path.clone();
                self.workspaces.select_path(&path);
                self.focus = PaneId::Terminal;
                let (rows, cols) = self.last_term_size;
                if !self.terminals.contains_key(&surface) {
                    let pane = TerminalPane::spawn(
                        rows,
                        cols,
                        &path,
                        self.event_tx.clone(),
                        self.startup_cmd.as_deref(),
                    )?;
                    self.terminals.insert(surface, pane);
                } else if let Some(t) = self.terminals.get_mut(&surface) {
                    let _ = t.resize(rows, cols);
                }
                let _ = self.event_tx.send(Event::GitRefresh);
            }
            Event::PromptDeleteWorktree(path, name) => {
                if path == self.last_workspace_path {
                    // Refuse: cannot remove current worktree from inside it.
                } else {
                    self.pending_delete = Some((path, name));
                }
            }
            Event::DeleteWorktree(path) => {
                let root = self.last_workspace_path.clone();
                let tx = self.event_tx.clone();
                let target = path.clone();
                self.terminals.remove(&Surface::Workspace(path));
                tokio::spawn(async move {
                    match git::remove_worktree(&root, &target).await {
                        Ok(_) => {
                            if let Ok(wts) = git::list_worktrees(&root).await {
                                let _ = tx.send(Event::WorktreesRefreshed(wts));
                            }
                        }
                        Err(e) => {
                            eprintln!("git worktree remove failed: {e}");
                        }
                    }
                });
            }
            Event::CreateWorktree(branch) => {
                let root = self.last_workspace_path.clone();
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
                let path = self.last_workspace_path.clone();
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
        let rows = area.height;
        let cols = area.width;
        self.last_term_size = (rows, cols);
        if let Some(t) = self.active_terminal_mut() {
            let _ = t.resize(rows, cols);
        }
    }
}

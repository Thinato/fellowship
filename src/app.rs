use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::Result;
use crossterm::event::KeyEvent;
use tokio::sync::mpsc;
use tracing::{debug, error, info};

use crate::agents::registry::AgentRegistry;
use crate::beads::Bead;
use crate::event::Event;
use crate::gh;
use crate::git;
use crate::keymap::{Action, InputMode, Keymap, default_bindings};
use crate::layout::PaneLayout;
use crate::panes::gitstatus::GitStatusPane;
use crate::panes::members::MembersPane;
use crate::panes::status::StatusPane;
use crate::panes::terminal::TerminalPane;
use crate::panes::workspaces::WorkspacesPane;
use crate::runtime::STATE_DIR;
use crate::surface::{MemberId, Role, Surface};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneId {
    Members,
    Workspaces,
    Terminal,
    GitStatus,
}

/// Which sub-view the right column currently shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RightView {
    Git,
    Status,
}

pub struct App {
    pub focus: PaneId,
    pub input_mode: InputMode,
    pub members: MembersPane,
    pub workspaces: WorkspacesPane,
    pub terminals: HashMap<Surface, TerminalPane>,
    pub git_status: GitStatusPane,
    pub status: StatusPane,
    pub right_view: RightView,
    pub agent_registry: AgentRegistry,
    pub beads: Vec<Bead>,
    /// Per-session uuid set by `main` and propagated to spawned PTYs via
    /// `FELLOWSHIP_SESSION`. Surfaced in the status bar so the user knows
    /// which `~/.fellowship/runtime/<session>/` dir to point `fellowship-ctl`
    /// at when running it from a non-PTY shell.
    pub session_id: String,
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
        runtime_root: &std::path::Path,
        session_id: String,
        terminal: TerminalPane,
        event_tx: mpsc::UnboundedSender<Event>,
        startup_cmd: Option<String>,
    ) -> Result<Self> {
        let last_term_size = terminal.size();
        let (rows, cols) = last_term_size;
        let mut terminals = HashMap::new();
        let initial_surface = Surface::Workspace(root_path.clone());
        terminals.insert(initial_surface.clone(), terminal);

        // Phase 3: spawn one PTY per singleton role so switching to a Member
        // surface from the Members pane has something to display. The banner
        // command runs once and `exec bash` keeps the shell alive afterward.
        // Phase 10 replaces the banner with the real `claude` invocation.
        for role in [Role::Pm, Role::Orchestrator, Role::Architect, Role::Recon] {
            let id = MemberId::singleton(role);
            let banner = format!(
                "echo '[{}] placeholder — real prompt lands in Phase 10'; exec bash",
                role.as_str()
            );
            let pane =
                TerminalPane::spawn(rows, cols, &root_path, event_tx.clone(), Some(&banner))?;
            terminals.insert(Surface::Member(id), pane);
        }

        let mut agent_registry = AgentRegistry::new();
        // Best-effort initial scan: any heartbeats already on disk (e.g. left
        // over from a prior session under the same FELLOWSHIP_SESSION) get
        // ingested before the watcher takes over.
        let _ = agent_registry.load_from_state_dir(&runtime_root.join(STATE_DIR));

        Ok(Self {
            focus: PaneId::Terminal,
            input_mode: InputMode::Normal,
            members: MembersPane::new(),
            workspaces: WorkspacesPane::new(root_path.clone()),
            terminals,
            git_status: GitStatusPane::new(root_path.clone()),
            status: StatusPane::new(),
            right_view: RightView::Git,
            agent_registry,
            beads: Vec::new(),
            session_id,
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
        })
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
            Action::FocusGitView => {
                self.right_view = RightView::Git;
                self.focus = PaneId::GitStatus;
            }
            Action::FocusStatusView => {
                self.right_view = RightView::Status;
                self.focus = PaneId::GitStatus;
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
            PaneId::GitStatus => match self.right_view {
                RightView::Git => {
                    // git view has no key handling currently
                }
                RightView::Status => {
                    self.status.handle_key(key);
                }
            },
            PaneId::Members => {
                if let Some(event) = self.members.handle_key(key) {
                    let _ = self.event_tx.send(event);
                }
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
                self.members.set_active_member(None);
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
            Event::SwitchSurface(Surface::Workspace(path)) => {
                // Delegate to the dedicated workspace flow so its side effects
                // (git_status root, GitRefresh, list select) still fire.
                let _ = self.event_tx.send(Event::SwitchWorkspace(path));
            }
            Event::SwitchSurface(Surface::Member(id)) => {
                info!(member = %id.label(), "switch to member surface");
                let surface = Surface::Member(id);
                self.active_surface = surface.clone();
                self.members.set_active_member(Some(id));
                self.focus = PaneId::Terminal;
                let (rows, cols) = self.last_term_size;
                if let Some(t) = self.terminals.get_mut(&surface) {
                    let _ = t.resize(rows, cols);
                }
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
                            error!("git worktree remove failed: {e}");
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
                            error!("git worktree add failed: {e}");
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
            Event::AgentHeartbeat(record) => {
                debug!(agent = %record.agent_id, status = %record.status, "heartbeat");
                self.agent_registry.upsert(record);
            }
            Event::BeadsRefreshed(beads) => {
                self.beads = beads;
            }
            Event::JournalSnapshot(entries) => {
                self.status.replace_journal(entries);
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

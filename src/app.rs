use std::collections::{HashMap, HashSet, VecDeque};
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
use crate::runtime::{STATE_DIR, SpawnRequest};
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
    /// PATH override applied to every member-surface PTY. Includes the
    /// `~/.fellowship/bin/` shim dir as the first entry.
    pub agent_path: String,
    /// On-disk dir holding the role prompt files. Used to compute the path
    /// passed to `claude --append-system-prompt-file` for each member spawn.
    pub prompts_root: PathBuf,
    /// Whether the `claude` CLI was found on PATH at boot. Drives the
    /// real-vs-banner spawn shape in [`agents::spawn::plan_for`] and the
    /// PM-default-focus decision in `App::new`.
    pub claude_available: bool,
    /// Max engineer PTYs allowed concurrently (Phase 9 hardcodes 4; Phase 10
    /// pulls this from `Config::agents.max_engineers`).
    pub max_engineers: usize,
    /// Spawn requests received while at capacity. Drained on engineer release.
    pub spawn_queue: VecDeque<SpawnRequest>,
    /// Per-engineer worktree path, captured at spawn time so `restart_agent`
    /// can re-spawn into the same worktree without recreating it. Singletons
    /// don't need this (their cwd is `last_workspace_path`).
    pub engineer_worktrees: HashMap<MemberId, PathBuf>,
    /// Watchdog state machine — restarts taken so far per member. When this
    /// reaches `max_restarts`, the member is added to `failed_agents` and no
    /// further restarts are attempted (escalation banner shown in status bar).
    pub restart_counts: HashMap<MemberId, u32>,
    /// Members the watchdog has given up on (>= `max_restarts` restarts).
    pub failed_agents: HashSet<MemberId>,
    /// Max restarts per member before declaring failure (Phase 11 hardcodes
    /// 3; Phase 10.5+ wires `Config::agents.max_restarts`).
    pub max_restarts: u32,
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
    // Eight params is past clippy's default. The constructor accumulates
    // session-wide dependencies (workspace, runtime, agents, env). Bundling
    // them into a config struct is a future cleanup; out of scope for Phase 10.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        root_path: PathBuf,
        runtime_root: &std::path::Path,
        prompts_root: PathBuf,
        session_id: String,
        agent_path: &str,
        claude_available: bool,
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
            let prompt_path = crate::agents::spawn::prompt_path_for(&prompts_root, role);
            let action =
                crate::agents::spawn::plan_for(role, &id.label(), claude_available, &prompt_path);
            let base_env = vec![("PATH".to_string(), agent_path.to_string())];
            let pane = crate::agents::spawn::execute(
                &action,
                rows,
                cols,
                &root_path,
                event_tx.clone(),
                &base_env,
            )?;
            terminals.insert(Surface::Member(id), pane);
        }

        let mut agent_registry = AgentRegistry::new();
        // Best-effort initial scan: any heartbeats already on disk (e.g. left
        // over from a prior session under the same FELLOWSHIP_SESSION) get
        // ingested before the watcher takes over.
        let _ = agent_registry.load_from_state_dir(&runtime_root.join(STATE_DIR));

        // Q5 resolution: when agents are usable (claude_available), boot with
        // the user already focused on the PM member surface so they can start
        // chatting immediately. When claude is missing, keep the workspace
        // active so fellowship still works as a plain worktree TUI.
        let mut members_pane = MembersPane::new();
        let active_surface = if claude_available {
            let pm = MemberId::singleton(Role::Pm);
            members_pane.set_active_member(Some(pm));
            Surface::Member(pm)
        } else {
            initial_surface
        };

        Ok(Self {
            focus: PaneId::Terminal,
            input_mode: InputMode::Normal,
            members: members_pane,
            workspaces: WorkspacesPane::new(root_path.clone()),
            terminals,
            git_status: GitStatusPane::new(root_path.clone()),
            status: StatusPane::new(),
            right_view: RightView::Git,
            agent_registry,
            beads: Vec::new(),
            session_id,
            agent_path: agent_path.to_string(),
            prompts_root,
            claude_available,
            max_engineers: 4,
            spawn_queue: VecDeque::new(),
            engineer_worktrees: HashMap::new(),
            restart_counts: HashMap::new(),
            failed_agents: HashSet::new(),
            max_restarts: 3,
            show_help: false,
            should_quit: false,
            pending_delete: None,
            active_surface,
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
                // Receiving a heartbeat clears any restart count we'd been
                // accumulating against this agent — a healthy agent should
                // not be punished for a transient earlier silence.
                if let Some(id) = parse_engineer_id(&record.agent_id)
                    .or_else(|| singleton_id_for_label(&record.agent_id))
                {
                    self.restart_counts.remove(&id);
                }
                self.agent_registry.upsert(record);
            }
            Event::WatchdogTick => {
                self.run_watchdog().await;
            }
            Event::SpawnRequestReceived(req) => {
                self.handle_spawn_request(req).await;
            }
            Event::ReleaseRequestReceived(req) => {
                self.handle_release_request(req).await;
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

    /// Smallest engineer instance number not currently in use, starting at 1.
    fn next_engineer_instance(&self) -> u32 {
        let used: std::collections::HashSet<u32> =
            self.members.engineer_instances().into_iter().collect();
        (1u32..).find(|n| !used.contains(n)).unwrap_or(u32::MAX)
    }

    fn live_engineer_count(&self) -> usize {
        self.members.engineer_instances().len()
    }

    async fn handle_spawn_request(&mut self, req: SpawnRequest) {
        if self.live_engineer_count() >= self.max_engineers {
            info!(
                queued = self.spawn_queue.len() + 1,
                cap = self.max_engineers,
                "spawn-engineer queued — engineer pool at capacity"
            );
            self.spawn_queue.push_back(req);
            return;
        }
        if let Err(e) = self.spawn_engineer(req).await {
            error!("spawn-engineer failed: {e:#}");
        }
    }

    async fn spawn_engineer(&mut self, req: SpawnRequest) -> Result<()> {
        let Some(branch) = req.branch.as_ref() else {
            anyhow::bail!("spawn-engineer requested without --branch (Phase 9 requires it)");
        };
        let instance = self.next_engineer_instance();
        let id = MemberId::engineer(instance);

        // Create the worktree on the active workspace path.
        let repo = self.last_workspace_path.clone();
        let worktree_path = git::add_worktree(&repo, branch).await?;

        let agent_id_str = id.label();
        let prompt_path = crate::agents::spawn::prompt_path_for(&self.prompts_root, Role::Engineer);
        let action = crate::agents::spawn::plan_for(
            Role::Engineer,
            &agent_id_str,
            self.claude_available,
            &prompt_path,
        );

        // Base env: PATH (safe-git shim) + spawn request id for traceability.
        // The action's own env (AGENT_PROMPT / AGENT_ID / optional
        // AGENT_MODEL) is merged on top by `execute`.
        let base_env = vec![
            ("PATH".to_string(), self.agent_path.clone()),
            (
                "FELLOWSHIP_SPAWN_REQUEST_ID".to_string(),
                req.request_id.clone(),
            ),
        ];
        let (rows, cols) = self.last_term_size;
        let pane = crate::agents::spawn::execute(
            &action,
            rows,
            cols,
            &worktree_path,
            self.event_tx.clone(),
            &base_env,
        )?;
        self.terminals.insert(Surface::Member(id), pane);
        self.members.add_member(id);
        self.engineer_worktrees.insert(id, worktree_path.clone());

        info!(
            engineer = %id.label(),
            branch = %branch,
            worktree = %worktree_path.display(),
            "engineer spawned"
        );
        let _ = self.event_tx.send(Event::WorktreesRefreshed(
            git::list_worktrees(&repo).await.unwrap_or_default(),
        ));
        Ok(())
    }

    async fn handle_release_request(&mut self, req: crate::runtime::ReleaseRequest) {
        let Some(id) = parse_engineer_id(&req.agent_id) else {
            error!(agent_id = %req.agent_id, "release-engineer rejected: not an engineer id");
            return;
        };
        let surface = Surface::Member(id);
        if let Some(mut pane) = self.terminals.remove(&surface) {
            pane.shutdown();
        }
        self.members.remove_member(id);
        self.engineer_worktrees.remove(&id);
        self.restart_counts.remove(&id);
        self.failed_agents.remove(&id);
        info!(engineer = %id.label(), "engineer released");

        // Drain one queued spawn request if any (and we now have headroom).
        if self.live_engineer_count() < self.max_engineers
            && let Some(next) = self.spawn_queue.pop_front()
            && let Err(e) = self.spawn_engineer(next).await
        {
            error!("draining queued spawn-engineer failed: {e:#}");
        }
    }

    pub fn resize_terminal(&mut self, area: ratatui::layout::Rect) {
        let rows = area.height;
        let cols = area.width;
        self.last_term_size = (rows, cols);
        if let Some(t) = self.active_terminal_mut() {
            let _ = t.resize(rows, cols);
        }
    }

    /// Periodic watchdog pass. Splits cleanly into a pure decision pass
    /// (`decide_watchdog_actions`, fully unit-tested below) and the
    /// side-effect pass that applies each decision (counter bumps, PTY
    /// restart, escalation set).
    async fn run_watchdog(&mut self) {
        let now = crate::runtime::now_ms();
        let actions = decide_watchdog_actions(
            &self.members.members,
            &self.agent_registry,
            &self.restart_counts,
            &self.failed_agents,
            self.max_restarts,
            now,
        );
        for action in actions {
            match action {
                WatchdogAction::Restart { id, attempt } => {
                    self.restart_counts.insert(id, attempt);
                    info!(
                        agent = %id.label(),
                        attempt = attempt,
                        max = self.max_restarts,
                        "watchdog: agent dead, attempting restart"
                    );
                    if let Err(e) = self.restart_agent(id) {
                        error!(agent = %id.label(), "watchdog restart failed: {e:#}");
                    }
                }
                WatchdogAction::Escalate { id, restarts } => {
                    if self.failed_agents.insert(id) {
                        error!(
                            agent = %id.label(),
                            restarts = restarts,
                            "watchdog: agent failed after max restarts; not restarting again"
                        );
                    }
                }
                WatchdogAction::NoteStale(id) => {
                    debug!(agent = %id.label(), "watchdog: agent stale");
                }
            }
        }
    }

    /// Re-spawn `id`'s PTY in place. Singletons re-spawn at the current
    /// workspace root; engineers re-spawn into their original worktree
    /// (preserved across restarts). Caller is responsible for restart-count
    /// bookkeeping; this function only does the I/O.
    fn restart_agent(&mut self, id: MemberId) -> Result<()> {
        let surface = Surface::Member(id);
        if let Some(mut old) = self.terminals.remove(&surface) {
            old.shutdown();
        }
        // Drop the prior heartbeat so the new PTY isn't judged Dead against
        // the previous record. Liveness becomes Unknown (no badge) until the
        // restarted agent writes its first heartbeat.
        self.agent_registry.clear(&id.label());
        let cwd: PathBuf = match id.role {
            Role::Engineer => self.engineer_worktrees.get(&id).cloned().ok_or_else(|| {
                anyhow::anyhow!("no recorded worktree for engineer {}", id.label())
            })?,
            _ => self.last_workspace_path.clone(),
        };
        let prompt_path = crate::agents::spawn::prompt_path_for(&self.prompts_root, id.role);
        let action = crate::agents::spawn::plan_for(
            id.role,
            &id.label(),
            self.claude_available,
            &prompt_path,
        );
        let base_env = vec![("PATH".to_string(), self.agent_path.clone())];
        let (rows, cols) = self.last_term_size;
        let pane = crate::agents::spawn::execute(
            &action,
            rows,
            cols,
            &cwd,
            self.event_tx.clone(),
            &base_env,
        )?;
        self.terminals.insert(surface, pane);
        Ok(())
    }
}

/// One-tick output from [`decide_watchdog_actions`]. The pure decision
/// stays separate from `App`'s mutable side effects so it can be exercised
/// from `#[test]` without spinning up real PTYs.
#[derive(Debug, Clone, PartialEq, Eq)]
enum WatchdogAction {
    /// Restart this member. `attempt` is the new counter value (1-based)
    /// the side-effect pass should record.
    Restart { id: MemberId, attempt: u32 },
    /// Add this member to `failed_agents`. `restarts` is the count of
    /// failed attempts that triggered escalation.
    Escalate { id: MemberId, restarts: u32 },
    /// Member is Stale. Used purely for the debug log; no state change.
    NoteStale(MemberId),
}

/// Pure watchdog policy. Given the current member set and observed state,
/// returns the actions this tick should take. Idempotent; no I/O.
fn decide_watchdog_actions(
    members: &[MemberId],
    registry: &AgentRegistry,
    restart_counts: &HashMap<MemberId, u32>,
    failed_agents: &HashSet<MemberId>,
    max_restarts: u32,
    now_ms: u128,
) -> Vec<WatchdogAction> {
    use crate::agents::registry::Liveness;
    let mut out = Vec::new();
    for id in members {
        if failed_agents.contains(id) {
            continue;
        }
        match registry.liveness_for(&id.label(), now_ms) {
            Liveness::Dead => {
                let count = *restart_counts.get(id).unwrap_or(&0);
                if count >= max_restarts {
                    out.push(WatchdogAction::Escalate {
                        id: *id,
                        restarts: count,
                    });
                } else {
                    out.push(WatchdogAction::Restart {
                        id: *id,
                        attempt: count + 1,
                    });
                }
            }
            Liveness::Stale => out.push(WatchdogAction::NoteStale(*id)),
            Liveness::Live | Liveness::Unknown => {}
        }
    }
    out
}

/// Parse `engineer-<n>` into a `MemberId`. Returns `None` for any other shape
/// — singletons (pm, orchestrator, …) cannot be released this way.
fn parse_engineer_id(s: &str) -> Option<MemberId> {
    let n: u32 = s.strip_prefix("engineer-")?.parse().ok()?;
    Some(MemberId::engineer(n))
}

/// Parse a singleton label (`pm`, `orchestrator`, `architect`, `recon`) into
/// its canonical `MemberId`. Counterpart to `parse_engineer_id` so both
/// shapes can be resolved when looking up by string id (e.g. heartbeat
/// records that arrive identified by `agent_id` strings).
fn singleton_id_for_label(label: &str) -> Option<MemberId> {
    let role = match label {
        "pm" => Role::Pm,
        "orchestrator" => Role::Orchestrator,
        "architect" => Role::Architect,
        "recon" => Role::Recon,
        _ => return None,
    };
    Some(MemberId::singleton(role))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_engineer_id_accepts_valid_shape() {
        assert_eq!(parse_engineer_id("engineer-1"), Some(MemberId::engineer(1)));
        assert_eq!(
            parse_engineer_id("engineer-42"),
            Some(MemberId::engineer(42))
        );
    }

    #[test]
    fn parse_engineer_id_rejects_singleton_and_garbage() {
        assert!(parse_engineer_id("pm").is_none());
        assert!(parse_engineer_id("orchestrator").is_none());
        assert!(parse_engineer_id("engineer-").is_none());
        assert!(parse_engineer_id("engineer-x").is_none());
        assert!(parse_engineer_id("engineer").is_none());
        assert!(parse_engineer_id("").is_none());
    }

    #[test]
    fn singleton_id_for_label_recognizes_canonical_names() {
        assert_eq!(
            singleton_id_for_label("pm"),
            Some(MemberId::singleton(Role::Pm))
        );
        assert_eq!(
            singleton_id_for_label("orchestrator"),
            Some(MemberId::singleton(Role::Orchestrator))
        );
        assert_eq!(
            singleton_id_for_label("architect"),
            Some(MemberId::singleton(Role::Architect))
        );
        assert_eq!(
            singleton_id_for_label("recon"),
            Some(MemberId::singleton(Role::Recon))
        );
    }

    #[test]
    fn singleton_id_for_label_rejects_non_singletons_and_garbage() {
        assert!(singleton_id_for_label("engineer-1").is_none());
        assert!(singleton_id_for_label("PM").is_none()); // case-sensitive
        assert!(singleton_id_for_label("").is_none());
        assert!(singleton_id_for_label("anything-else").is_none());
    }

    use crate::agents::registry::{AgentRegistry, HEARTBEAT_DEAD_SEC, HEARTBEAT_WARN_SEC};
    use crate::runtime::HeartbeatRecord;

    fn fresh_registry(records: &[(&str, u128)]) -> AgentRegistry {
        let mut reg = AgentRegistry::new();
        for (id, last_seen_ms) in records {
            reg.upsert(HeartbeatRecord {
                agent_id: (*id).to_string(),
                last_seen_ms: *last_seen_ms,
                status: "test".into(),
            });
        }
        reg
    }

    #[test]
    fn watchdog_no_actions_when_all_live() {
        let pm = MemberId::singleton(Role::Pm);
        let now = 1_000_000;
        let warn_ms = (HEARTBEAT_WARN_SEC as u128) * 1000;
        let reg = fresh_registry(&[("pm", now - warn_ms / 2)]);
        let actions =
            decide_watchdog_actions(&[pm], &reg, &HashMap::new(), &HashSet::new(), 3, now);
        assert!(actions.is_empty(), "live member should not trigger actions");
    }

    #[test]
    fn watchdog_emits_note_stale_for_stale_members() {
        let pm = MemberId::singleton(Role::Pm);
        let now = 1_000_000;
        let warn_ms = (HEARTBEAT_WARN_SEC as u128) * 1000;
        let reg = fresh_registry(&[("pm", now - warn_ms - 1000)]);
        let actions =
            decide_watchdog_actions(&[pm], &reg, &HashMap::new(), &HashSet::new(), 3, now);
        assert_eq!(actions, vec![WatchdogAction::NoteStale(pm)]);
    }

    #[test]
    fn watchdog_skips_unknown_no_telemetry_members() {
        let arch = MemberId::singleton(Role::Architect);
        // No record for architect.
        let actions = decide_watchdog_actions(
            &[arch],
            &AgentRegistry::new(),
            &HashMap::new(),
            &HashSet::new(),
            3,
            1_000_000,
        );
        assert!(actions.is_empty(), "Unknown liveness must be a no-op");
    }

    #[test]
    fn watchdog_restarts_dead_member_under_cap() {
        let pm = MemberId::singleton(Role::Pm);
        let now = 1_000_000;
        let dead_ms = (HEARTBEAT_DEAD_SEC as u128) * 1000;
        let reg = fresh_registry(&[("pm", now - dead_ms - 1000)]);
        let actions =
            decide_watchdog_actions(&[pm], &reg, &HashMap::new(), &HashSet::new(), 3, now);
        assert_eq!(
            actions,
            vec![WatchdogAction::Restart { id: pm, attempt: 1 }]
        );
    }

    #[test]
    fn watchdog_increments_attempt_with_existing_count() {
        let pm = MemberId::singleton(Role::Pm);
        let now = 1_000_000;
        let dead_ms = (HEARTBEAT_DEAD_SEC as u128) * 1000;
        let reg = fresh_registry(&[("pm", now - dead_ms - 1)]);
        let mut counts = HashMap::new();
        counts.insert(pm, 2);
        let actions = decide_watchdog_actions(&[pm], &reg, &counts, &HashSet::new(), 3, now);
        assert_eq!(
            actions,
            vec![WatchdogAction::Restart { id: pm, attempt: 3 }]
        );
    }

    #[test]
    fn watchdog_escalates_at_max_restarts() {
        let pm = MemberId::singleton(Role::Pm);
        let now = 1_000_000;
        let dead_ms = (HEARTBEAT_DEAD_SEC as u128) * 1000;
        let reg = fresh_registry(&[("pm", now - dead_ms - 1)]);
        let mut counts = HashMap::new();
        counts.insert(pm, 3); // already at max
        let actions = decide_watchdog_actions(&[pm], &reg, &counts, &HashSet::new(), 3, now);
        assert_eq!(
            actions,
            vec![WatchdogAction::Escalate {
                id: pm,
                restarts: 3
            }]
        );
    }

    #[test]
    fn watchdog_skips_already_failed_members() {
        let pm = MemberId::singleton(Role::Pm);
        let now = 1_000_000;
        let dead_ms = (HEARTBEAT_DEAD_SEC as u128) * 1000;
        let reg = fresh_registry(&[("pm", now - dead_ms - 1)]);
        let mut failed = HashSet::new();
        failed.insert(pm);
        let actions = decide_watchdog_actions(&[pm], &reg, &HashMap::new(), &failed, 3, now);
        assert!(
            actions.is_empty(),
            "failed members must produce no further actions"
        );
    }

    #[test]
    fn watchdog_handles_mixed_member_set_in_one_pass() {
        let pm = MemberId::singleton(Role::Pm);
        let arch = MemberId::singleton(Role::Architect);
        let recon = MemberId::singleton(Role::Recon);
        let eng = MemberId::engineer(1);
        let now = 1_000_000;
        let warn_ms = (HEARTBEAT_WARN_SEC as u128) * 1000;
        let dead_ms = (HEARTBEAT_DEAD_SEC as u128) * 1000;
        // pm = live, arch = stale, recon = unknown (no record), eng = dead.
        let reg = fresh_registry(&[
            ("pm", now - 5_000),
            ("architect", now - warn_ms - 5_000),
            ("engineer-1", now - dead_ms - 5_000),
        ]);
        let actions = decide_watchdog_actions(
            &[pm, arch, recon, eng],
            &reg,
            &HashMap::new(),
            &HashSet::new(),
            3,
            now,
        );
        assert_eq!(
            actions,
            vec![
                WatchdogAction::NoteStale(arch),
                WatchdogAction::Restart {
                    id: eng,
                    attempt: 1
                },
            ]
        );
    }
}

use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture, EventStream},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use futures::StreamExt;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use tokio::sync::mpsc;
use tokio::time;

use fellowship::agents::{orchestrator, spawn as agent_spawn, watcher};
use fellowship::app::App;
use fellowship::beads;
use fellowship::config;
use fellowship::debug_log;
use fellowship::event::Event;
use fellowship::guard;
use fellowship::panes::terminal::TerminalPane;
use fellowship::runtime;
use fellowship::ui;
use tracing::{error, info, warn};

#[tokio::main]
async fn main() -> Result<()> {
    let root_path = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().expect("cannot read cwd"));

    // Allocate a per-session uuid so multiple fellowship instances don't collide
    // on the runtime dir, and propagate it to spawned PTYs so agents inherit it.
    let session_id = uuid::Uuid::new_v4().to_string();
    // SAFETY: env mutation is unsafe in edition 2024. Done once before any
    // PTY spawn so child processes inherit the value.
    unsafe {
        std::env::set_var("FELLOWSHIP_SESSION", &session_id);
    }
    let runtime_root = runtime::runtime_dir()?;
    runtime::ensure_subdir(&runtime_root, runtime::STATE_DIR)?;
    runtime::ensure_subdir(&runtime_root, runtime::SPAWN_REQUEST_DIR)?;
    runtime::ensure_subdir(&runtime_root, runtime::RELEASE_REQUEST_DIR)?;

    let log_path = debug_log::init(&runtime_root)?;
    info!(
        session = %session_id,
        runtime_root = %runtime_root.display(),
        log = %log_path.display(),
        "fellowship boot"
    );

    // Publish CURRENT_SESSION marker so `fellowship-ctl` invocations from
    // shells outside this fellowship instance pick the right runtime dir.
    if let Err(e) = runtime::write_current_session(&session_id) {
        error!("failed to write CURRENT_SESSION marker: {e}");
    }

    // Install the safe-git shim and compute the PATH override for member
    // surfaces. Boot fails loudly if the shim cannot be installed — agents
    // running with --dangerously-skip-permissions and an unrestricted PATH
    // could push directly to main, which is the exact failure mode we are
    // guarding against.
    let shim_dir = guard::shim_dir()?;
    let safe_git = guard::locate_safe_git()?;
    guard::install_shims(&shim_dir, &safe_git)?;
    let agent_path_os = guard::path_with_shim_prepended(&shim_dir)?;
    let agent_path = agent_path_os
        .into_string()
        .map_err(|_| anyhow::anyhow!("PATH contained non-UTF8 bytes; refuse to spawn agents"))?;
    info!(
        shim_dir = %shim_dir.display(),
        safe_git = %safe_git.display(),
        "safe-git shims installed for agent surfaces"
    );

    // Materialize the role prompts on disk so `claude --append-system-prompt-file`
    // can read them. Idempotent; safe to call every boot.
    let prompts_root = runtime_root.join("prompts");
    agent_spawn::write_prompt_files(&prompts_root)?;
    info!(
        prompts_root = %prompts_root.display(),
        "agent role prompts materialized"
    );

    // Probe `claude` once at boot. When absent, member surfaces fall back to
    // a placeholder banner that drops the user into a normal bash so
    // fellowship still runs in dev environments without claude installed.
    let claude_available = agent_spawn::claude_available_at_boot();
    if claude_available {
        info!("claude CLI detected — agent surfaces will exec real claude");
    } else {
        warn!(
            "claude CLI not found on PATH — agent surfaces will boot a placeholder bash. \
             Install claude code (https://claude.com/claude-code) and restart fellowship."
        );
    }

    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = run(
        &mut terminal,
        root_path,
        runtime_root,
        prompts_root,
        session_id.clone(),
        agent_path,
        claude_available,
    )
    .await;

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    if let Err(e) = runtime::clear_current_session(&session_id) {
        error!("failed to clear CURRENT_SESSION marker: {e}");
    }
    info!(session = %session_id, "fellowship shutdown");

    result
}

async fn run(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    root_path: PathBuf,
    runtime_root: PathBuf,
    prompts_root: PathBuf,
    session_id: String,
    agent_path: String,
    claude_available: bool,
) -> Result<()> {
    let (event_tx, mut event_rx) = mpsc::unbounded_channel::<Event>();

    // Estimate initial PTY size from terminal dimensions
    let size = terminal.size()?;
    // Layout: left=28 + 2borders, right=40 + 2borders, mid borders=2, status=1
    let mid_cols = size.width.saturating_sub(28 + 40 + 6).max(20);
    let mid_rows = size.height.saturating_sub(3).max(5);

    let cfg = config::Config::load(&root_path);
    let startup_cmd = cfg.shell_startup_command.clone();
    let pty_pane = TerminalPane::spawn(
        mid_rows,
        mid_cols,
        &root_path,
        event_tx.clone(),
        startup_cmd.as_deref(),
    )?;
    let mut app = App::new(
        root_path.clone(),
        &runtime_root,
        prompts_root,
        session_id,
        &agent_path,
        claude_available,
        pty_pane,
        event_tx.clone(),
        startup_cmd,
    )?;

    // Watcher must outlive the run loop or notify drops the underlying watch.
    let _watcher = watcher::spawn_runtime_watcher(&runtime_root, event_tx.clone())?;

    // Initial data load
    let _ = event_tx.send(Event::GitRefresh);

    // Background git refresh tick every 2 seconds
    let tick_tx = event_tx.clone();
    tokio::spawn(async move {
        let mut interval = time::interval(Duration::from_secs(2));
        interval.tick().await; // skip immediate first tick (already sent GitRefresh above)
        loop {
            interval.tick().await;
            if tick_tx.send(Event::GitRefresh).is_err() {
                break;
            }
        }
    });

    // Phase 12: native Orchestrator loop. Polls `bd list --json` and writes
    // a `SpawnRequest` for each open `role:engineer` bead with no
    // assignee. Replaces the LLM Orchestrator PTY which never auto-ticked.
    let orchestrator_runtime_root = runtime_root.clone();
    tokio::spawn(async move {
        if let Err(e) = orchestrator::run(
            orchestrator_runtime_root,
            app.max_engineers,
            Duration::from_secs(orchestrator::DEFAULT_POLL_SECS),
        )
        .await
        {
            error!("orchestrator loop exited: {e:#}");
        }
    });

    // Background watchdog tick every 5 seconds. The handler walks the live
    // members and decides whether to restart based on heartbeat age. Plan
    // §3.6 specifies this cadence and the warn/dead thresholds.
    let watchdog_tx = event_tx.clone();
    tokio::spawn(async move {
        let mut interval = time::interval(Duration::from_secs(5));
        loop {
            interval.tick().await;
            if watchdog_tx.send(Event::WatchdogTick).is_err() {
                break;
            }
        }
    });

    // Background beads poll every 3 seconds. `bd` errors (e.g. not initialized
    // in the repo, binary missing) are logged once-per-failure-type and the
    // tick continues so a later `bd init` recovers without restarting.
    let beads_tx = event_tx.clone();
    tokio::spawn(async move {
        let mut interval = time::interval(Duration::from_secs(3));
        let mut last_error: Option<String> = None;
        loop {
            interval.tick().await;
            match beads::list_beads().await {
                Ok(beads) => {
                    if last_error.is_some() {
                        info!(count = beads.len(), "beads recovered");
                        last_error = None;
                    }
                    if beads_tx.send(Event::BeadsRefreshed(beads)).is_err() {
                        break;
                    }
                }
                Err(e) => {
                    let msg = format!("{e:#}");
                    if last_error.as_deref() != Some(msg.as_str()) {
                        error!(error = %msg, "bd list --json failed");
                        last_error = Some(msg);
                    }
                }
            }
        }
    });

    // Crossterm key input reader
    let input_tx = event_tx.clone();
    tokio::spawn(async move {
        let mut stream = EventStream::new();
        while let Some(Ok(ev)) = stream.next().await {
            if let crossterm::event::Event::Key(key) = ev
                && input_tx.send(Event::Key(key)).is_err()
            {
                break;
            }
        }
    });

    loop {
        // Resize PTY to match current frame's terminal pane inner area
        let frame_size = terminal.size()?;
        let mid_cols = frame_size.width.saturating_sub(28 + 40 + 6).max(1);
        let mid_rows = frame_size.height.saturating_sub(3).max(1);
        app.resize_terminal(ratatui::layout::Rect::new(0, 0, mid_cols, mid_rows));

        terminal.draw(|f| ui::render(f, &mut app))?;

        if app.should_quit {
            for t in app.terminals.values_mut() {
                t.shutdown();
            }
            break;
        }

        match tokio::time::timeout(Duration::from_millis(16), event_rx.recv()).await {
            Ok(Some(event)) => {
                app.handle_event(event).await?;
            }
            Ok(None) => break, // channel closed
            Err(_) => {}       // timeout — redraw
        }
    }

    Ok(())
}

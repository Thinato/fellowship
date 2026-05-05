mod app;
mod config;
mod event;
mod gh;
mod git;
mod keymap;
mod layout;
mod panes;
mod ui;

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

use app::App;
use event::Event;
use panes::terminal::TerminalPane;

#[tokio::main]
async fn main() -> Result<()> {
    let root_path = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().expect("cannot read cwd"));

    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = run(&mut terminal, root_path).await;

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    result
}

async fn run(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    root_path: PathBuf,
) -> Result<()> {
    let (event_tx, mut event_rx) = mpsc::unbounded_channel::<Event>();

    // Estimate initial PTY size from terminal dimensions
    let size = terminal.size()?;
    // Layout: left=28 + 2borders, right=40 + 2borders, mid borders=2, status=1
    let mid_cols = size.width.saturating_sub(28 + 40 + 6).max(20);
    let mid_rows = size.height.saturating_sub(3).max(5);

    let pty_pane = TerminalPane::spawn(mid_rows, mid_cols, &root_path, event_tx.clone())?;
    let mut app = App::new(root_path.clone(), pty_pane, event_tx.clone());

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

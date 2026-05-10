//! Per-session debug log. Written to `<runtime_root>/fellowship.log`.
//!
//! The TUI cannot use stderr (crossterm owns the alternate screen), so any
//! diagnostic that previously went to `eprintln!` writes here instead. Tail
//! the file with `tail -F ~/.fellowship/runtime/<session>/fellowship.log`
//! while fellowship is running.

use std::path::Path;
use std::sync::Mutex;

use anyhow::{Context, Result};

const LOG_FILE: &str = "fellowship.log";

/// Initialize a global tracing subscriber that writes to the session log file.
/// Safe to call exactly once per process; subsequent calls are no-ops.
pub fn init(runtime_root: &Path) -> Result<std::path::PathBuf> {
    let path = runtime_root.join(LOG_FILE);
    std::fs::create_dir_all(runtime_root)
        .with_context(|| format!("mkdir -p {}", runtime_root.display()))?;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("open {}", path.display()))?;
    let writer = Mutex::new(file);
    // `try_init` returns Err if a subscriber is already installed; that's fine
    // — we only need one global subscriber per process.
    let _ = tracing_subscriber::fmt()
        .with_writer(writer)
        .with_ansi(false)
        .with_target(false)
        .with_max_level(tracing::Level::DEBUG)
        .try_init();
    Ok(path)
}

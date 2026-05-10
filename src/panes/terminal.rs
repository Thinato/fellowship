use std::io::Write;
use std::path::Path;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};

/// `portable_pty::CommandBuilder` starts with an empty env, so spawned
/// processes don't see HOME, USER, $PATH, claude/anthropic auth state, etc.
/// Forward the parent process env explicitly. Callers may override individual
/// vars afterwards (TERM, PATH for the safe-git shim, AGENT_*).
///
/// `TERM` is explicitly skipped because every caller sets `xterm-256color`.
fn inherit_parent_env(cmd: &mut CommandBuilder) {
    for (k, v) in std::env::vars_os() {
        if k == "TERM" {
            continue;
        }
        cmd.env(&k, &v);
    }
}
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;
use tokio::sync::mpsc::UnboundedSender;

use crate::event::Event;

pub mod key_to_bytes {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    pub fn encode(key: KeyEvent) -> Vec<u8> {
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let mut bytes = encode_inner(key);
        if alt && !bytes.is_empty() {
            let mut out = vec![0x1b];
            out.extend_from_slice(&bytes);
            bytes = out;
        }
        bytes
    }

    fn encode_inner(key: KeyEvent) -> Vec<u8> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Char(c) => {
                if ctrl {
                    let lower = c.to_ascii_lowercase();
                    if lower.is_ascii_alphabetic() {
                        return vec![(lower as u8) & 0x1f];
                    }
                    // Ctrl+Space → 0x00
                    if c == ' ' {
                        return vec![0x00];
                    }
                }
                let mut buf = [0u8; 4];
                c.encode_utf8(&mut buf).as_bytes().to_vec()
            }
            KeyCode::Enter => b"\r".to_vec(),
            KeyCode::Backspace => b"\x7f".to_vec(),
            KeyCode::Tab => b"\t".to_vec(),
            KeyCode::Esc => b"\x1b".to_vec(),
            KeyCode::Up => b"\x1b[A".to_vec(),
            KeyCode::Down => b"\x1b[B".to_vec(),
            KeyCode::Right => b"\x1b[C".to_vec(),
            KeyCode::Left => b"\x1b[D".to_vec(),
            KeyCode::Home => b"\x1b[H".to_vec(),
            KeyCode::End => b"\x1b[F".to_vec(),
            KeyCode::PageUp => b"\x1b[5~".to_vec(),
            KeyCode::PageDown => b"\x1b[6~".to_vec(),
            KeyCode::Delete => b"\x1b[3~".to_vec(),
            KeyCode::Insert => b"\x1b[2~".to_vec(),
            KeyCode::F(n) => match n {
                1 => b"\x1bOP".to_vec(),
                2 => b"\x1bOQ".to_vec(),
                3 => b"\x1bOR".to_vec(),
                4 => b"\x1bOS".to_vec(),
                5 => b"\x1b[15~".to_vec(),
                6 => b"\x1b[17~".to_vec(),
                7 => b"\x1b[18~".to_vec(),
                8 => b"\x1b[19~".to_vec(),
                9 => b"\x1b[20~".to_vec(),
                10 => b"\x1b[21~".to_vec(),
                11 => b"\x1b[23~".to_vec(),
                12 => b"\x1b[24~".to_vec(),
                _ => vec![],
            },
            _ => vec![],
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

        fn key(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
            KeyEvent::new(code, mods)
        }

        #[test]
        fn arrow_up() {
            assert_eq!(encode(key(KeyCode::Up, KeyModifiers::NONE)), b"\x1b[A");
        }

        #[test]
        fn arrow_down() {
            assert_eq!(encode(key(KeyCode::Down, KeyModifiers::NONE)), b"\x1b[B");
        }

        #[test]
        fn arrow_right() {
            assert_eq!(encode(key(KeyCode::Right, KeyModifiers::NONE)), b"\x1b[C");
        }

        #[test]
        fn arrow_left() {
            assert_eq!(encode(key(KeyCode::Left, KeyModifiers::NONE)), b"\x1b[D");
        }

        #[test]
        fn ctrl_c() {
            assert_eq!(
                encode(key(KeyCode::Char('c'), KeyModifiers::CONTROL)),
                vec![0x03]
            );
        }

        #[test]
        fn ctrl_space() {
            assert_eq!(
                encode(key(KeyCode::Char(' '), KeyModifiers::CONTROL)),
                vec![0x00]
            );
        }

        #[test]
        fn enter_is_cr() {
            assert_eq!(encode(key(KeyCode::Enter, KeyModifiers::NONE)), b"\r");
        }

        #[test]
        fn backspace_is_del() {
            assert_eq!(encode(key(KeyCode::Backspace, KeyModifiers::NONE)), b"\x7f");
        }

        #[test]
        fn f1_ss3() {
            assert_eq!(encode(key(KeyCode::F(1), KeyModifiers::NONE)), b"\x1bOP");
        }

        #[test]
        fn f4_ss3() {
            assert_eq!(encode(key(KeyCode::F(4), KeyModifiers::NONE)), b"\x1bOS");
        }

        #[test]
        fn f5_csi() {
            assert_eq!(encode(key(KeyCode::F(5), KeyModifiers::NONE)), b"\x1b[15~");
        }

        #[test]
        fn f12_csi() {
            assert_eq!(encode(key(KeyCode::F(12), KeyModifiers::NONE)), b"\x1b[24~");
        }

        #[test]
        fn alt_letter_prepends_esc() {
            let result = encode(key(KeyCode::Char('a'), KeyModifiers::ALT));
            assert_eq!(result, b"\x1ba");
        }

        #[test]
        fn plain_char() {
            assert_eq!(encode(key(KeyCode::Char('z'), KeyModifiers::NONE)), b"z");
        }

        #[test]
        fn page_up() {
            assert_eq!(encode(key(KeyCode::PageUp, KeyModifiers::NONE)), b"\x1b[5~");
        }

        #[test]
        fn page_down() {
            assert_eq!(
                encode(key(KeyCode::PageDown, KeyModifiers::NONE)),
                b"\x1b[6~"
            );
        }
    }
}

pub struct TerminalPane {
    parser: Arc<Mutex<vt100::Parser>>,
    master: Box<dyn MasterPty + Send>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
    size: (u16, u16),
}

impl TerminalPane {
    pub fn spawn(
        rows: u16,
        cols: u16,
        cwd: &Path,
        tx: UnboundedSender<Event>,
        startup_cmd: Option<&str>,
    ) -> Result<Self> {
        Self::spawn_with_env(rows, cols, cwd, tx, startup_cmd, &[])
    }

    /// Like [`Self::spawn`] but with extra environment variables added to
    /// the spawned shell. Member surfaces use this to inject a `PATH`
    /// override that points at the `safe-git` shim before the real `git`.
    pub fn spawn_with_env(
        rows: u16,
        cols: u16,
        cwd: &Path,
        tx: UnboundedSender<Event>,
        startup_cmd: Option<&str>,
        extra_env: &[(&str, &str)],
    ) -> Result<Self> {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string());
        let mut cmd = CommandBuilder::new(&shell);
        cmd.cwd(cwd);
        inherit_parent_env(&mut cmd);
        cmd.env("TERM", "xterm-256color");
        for (k, v) in extra_env {
            cmd.env(*k, *v);
        }
        Self::spawn_command(rows, cols, tx, cmd, startup_cmd)
    }

    /// Spawn an arbitrary program directly inside the PTY (no shell wrapper).
    /// Used by member surfaces that need to launch a specific binary with
    /// arguments passed as real argv (no shell quoting hazards).
    pub fn spawn_program_with_env(
        rows: u16,
        cols: u16,
        cwd: &Path,
        tx: UnboundedSender<Event>,
        program: &str,
        args: &[&str],
        extra_env: &[(&str, &str)],
    ) -> Result<Self> {
        let mut cmd = CommandBuilder::new(program);
        for a in args {
            cmd.arg(*a);
        }
        cmd.cwd(cwd);
        inherit_parent_env(&mut cmd);
        cmd.env("TERM", "xterm-256color");
        for (k, v) in extra_env {
            cmd.env(*k, *v);
        }
        Self::spawn_command(rows, cols, tx, cmd, None)
    }

    fn spawn_command(
        rows: u16,
        cols: u16,
        tx: UnboundedSender<Event>,
        cmd: CommandBuilder,
        startup_cmd: Option<&str>,
    ) -> Result<Self> {
        let pty_system = native_pty_system();
        let pair = pty_system.openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })?;

        let child = pair.slave.spawn_command(cmd)?;
        // Must drop slave so EOF propagates when child exits
        drop(pair.slave);

        let writer: Box<dyn Write + Send> = pair.master.take_writer()?;
        let writer = Arc::new(Mutex::new(writer));

        let parser = Arc::new(Mutex::new(vt100::Parser::new(rows, cols, 0)));

        let reader = pair.master.try_clone_reader()?;
        let parser_clone = Arc::clone(&parser);
        std::thread::spawn(move || {
            let mut reader = reader;
            let mut buf = vec![0u8; 4096];
            loop {
                match std::io::Read::read(&mut reader, &mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        parser_clone.lock().unwrap().process(&buf[..n]);
                        let _ = tx.send(Event::Redraw);
                    }
                }
            }
        });

        if let Some(cmd) = startup_cmd
            && !cmd.is_empty()
        {
            let mut line = cmd.to_string();
            line.push('\n');
            if let Ok(mut w) = writer.lock() {
                let _ = w.write_all(line.as_bytes());
                let _ = w.flush();
            }
        }

        Ok(Self {
            parser,
            master: pair.master,
            writer,
            child,
            size: (rows, cols),
        })
    }

    pub fn resize(&mut self, rows: u16, cols: u16) -> Result<()> {
        if self.size == (rows, cols) {
            return Ok(());
        }
        self.master.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })?;
        self.parser.lock().unwrap().set_size(rows, cols);
        self.size = (rows, cols);
        Ok(())
    }

    pub fn write_keys(&self, bytes: &[u8]) -> Result<()> {
        let mut w = self.writer.lock().unwrap();
        w.write_all(bytes)?;
        w.flush()?;
        Ok(())
    }

    pub fn render(&self, frame: &mut Frame, area: Rect, focused: bool) {
        use crate::ui::theme;
        use ratatui::widgets::{Block, Borders};

        let border_color = if focused {
            theme::FOCUSED_BORDER
        } else {
            theme::UNFOCUSED_BORDER
        };
        let title_style = if focused {
            Style::default()
                .fg(theme::TITLE_COLOR)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme::UNFOCUSED_BORDER)
        };

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border_color))
            .title(Span::styled(" Terminal ", title_style));

        let inner = block.inner(area);
        frame.render_widget(block, area);

        let parser = self.parser.lock().unwrap();
        let screen = parser.screen();
        let buf = frame.buffer_mut();

        for row_idx in 0..inner.height {
            for col_idx in 0..inner.width {
                let x = inner.x + col_idx;
                let y = inner.y + row_idx;

                let (ch, style) = if let Some(cell) = screen.cell(row_idx, col_idx) {
                    let ch = cell.contents();
                    let ch = if ch.is_empty() { " ".to_string() } else { ch };
                    let style = vt_style(cell);
                    (ch, style)
                } else {
                    (" ".to_string(), Style::default())
                };

                buf[(x, y)].set_symbol(&ch).set_style(style);
            }
        }

        if focused {
            let (cursor_row, cursor_col) = screen.cursor_position();
            let cursor_x = inner.x + cursor_col;
            let cursor_y = inner.y + cursor_row;
            if cursor_x < inner.x + inner.width && cursor_y < inner.y + inner.height {
                frame.set_cursor_position((cursor_x, cursor_y));
            }
        }
    }

    pub fn size(&self) -> (u16, u16) {
        self.size
    }

    pub fn shutdown(&mut self) {
        let _ = self.child.kill();
    }
}

fn vt_style(cell: &vt100::Cell) -> Style {
    let mut style = Style::default();

    if let Some(c) = vt_color(cell.fgcolor()) {
        style = style.fg(c);
    }
    if let Some(c) = vt_color(cell.bgcolor()) {
        style = style.bg(c);
    }
    if cell.bold() {
        style = style.add_modifier(Modifier::BOLD);
    }
    if cell.underline() {
        style = style.add_modifier(Modifier::UNDERLINED);
    }
    if cell.italic() {
        style = style.add_modifier(Modifier::ITALIC);
    }
    if cell.inverse() {
        style = style.add_modifier(Modifier::REVERSED);
    }
    style
}

fn vt_color(color: vt100::Color) -> Option<Color> {
    match color {
        vt100::Color::Default => None,
        vt100::Color::Idx(i) => Some(Color::Indexed(i)),
        vt100::Color::Rgb(r, g, b) => Some(Color::Rgb(r, g, b)),
    }
}

// Provide a no-op stub for the scaffold's `new()` call in app.rs.
// Once the event loop is wired (#9), App::new will call spawn() with real args.
impl TerminalPane {
    pub fn new() -> Self {
        // Intentionally panic at runtime if used without spawn — placeholder only.
        panic!("TerminalPane::new() is a scaffold stub; use TerminalPane::spawn() instead")
    }
}

impl Default for TerminalPane {
    fn default() -> Self {
        Self::new()
    }
}

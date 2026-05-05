use crate::app::{App, PaneId};
use crate::keymap::InputMode;
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
};

pub mod theme {
    use ratatui::style::Color;
    pub const FOCUSED_BORDER: Color = Color::Cyan;
    pub const UNFOCUSED_BORDER: Color = Color::DarkGray;
    pub const TITLE_COLOR: Color = Color::White;
    pub const STATUS_BG: Color = Color::DarkGray;
    pub const STATUS_FG: Color = Color::White;
    pub const PREFIX_INDICATOR: Color = Color::Yellow;
}

pub fn render(frame: &mut Frame, app: &mut App) {
    let area = frame.area();

    let [main_area, status_area] =
        Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(area);

    let [left_area, mid_area, right_area] = Layout::horizontal([
        Constraint::Length(28),
        Constraint::Min(40),
        Constraint::Length(40),
    ])
    .areas(main_area);

    render_workspaces_pane(frame, app, left_area);
    render_terminal_pane(frame, app, mid_area);
    render_gitstatus_pane(frame, app, right_area);
    render_status_bar(frame, app, status_area);

    if app.show_help {
        render_help_overlay(frame, area);
    }
}

fn render_workspaces_pane(frame: &mut Frame, app: &mut App, area: Rect) {
    let focused = app.focus == PaneId::Workspaces;
    app.workspaces.render(frame, area, focused);
}

fn render_terminal_pane(frame: &mut Frame, app: &mut App, area: Rect) {
    let focused = app.focus == PaneId::Terminal;
    let path = app.active_path.clone();
    if let Some(t) = app.terminals.get_mut(&path) {
        t.render(frame, area, focused);
    }
}

fn render_gitstatus_pane(frame: &mut Frame, app: &mut App, area: Rect) {
    let focused = app.focus == PaneId::GitStatus;
    app.git_status.render(frame, area, focused);
}

fn render_status_bar(frame: &mut Frame, app: &mut App, area: Rect) {
    let prefix_indicator = matches!(app.input_mode, InputMode::AwaitingPrefixFollower);

    let focus_label = match app.focus {
        PaneId::Workspaces => "WORKSPACES",
        PaneId::Terminal => "TERMINAL",
        PaneId::GitStatus => "GIT STATUS",
    };

    let mut spans = vec![
        Span::styled(
            format!(" {} ", focus_label),
            Style::default()
                .fg(theme::STATUS_FG)
                .bg(theme::STATUS_BG)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "  Ctrl+a: prefix  ?:help  q:quit",
            Style::default().fg(theme::STATUS_FG).bg(theme::STATUS_BG),
        ),
    ];

    if prefix_indicator {
        spans.push(Span::styled(
            "  [PREFIX]",
            Style::default()
                .fg(theme::PREFIX_INDICATOR)
                .bg(theme::STATUS_BG)
                .add_modifier(Modifier::BOLD),
        ));
    }

    let paragraph = Paragraph::new(Line::from(spans)).style(Style::default().bg(theme::STATUS_BG));
    frame.render_widget(paragraph, area);
}

fn render_help_overlay(frame: &mut Frame, area: Rect) {
    let help_width = 52u16;
    let help_height = 14u16;
    let x = area.x + area.width.saturating_sub(help_width) / 2;
    let y = area.y + area.height.saturating_sub(help_height) / 2;
    let popup_area = Rect::new(
        x,
        y,
        help_width.min(area.width),
        help_height.min(area.height),
    );

    let help_text = vec![
        Line::from(""),
        Line::from("  Ctrl+a e         Focus Workspaces"),
        Line::from("  Ctrl+a t         Focus Terminal"),
        Line::from("  Ctrl+a g         Focus Git Status"),
        Line::from("  Ctrl+a q         Quit"),
        Line::from("  Ctrl+a ?         Toggle help"),
        Line::from("  Ctrl+a Ctrl+a    Send literal ^A to PTY"),
        Line::from("  Ctrl+a Esc       Cancel prefix"),
        Line::from("  Ctrl+a 1..9      Switch worktree by index"),
        Line::from(""),
        Line::from("  j/k   Navigate list panes"),
        Line::from("  n     New worktree (Workspaces)"),
        Line::from("  Enter Switch to worktree"),
        Line::from(""),
    ];

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme::FOCUSED_BORDER))
        .title(Span::styled(
            " Help ",
            Style::default()
                .fg(theme::TITLE_COLOR)
                .add_modifier(Modifier::BOLD),
        ));

    frame.render_widget(Clear, popup_area);
    frame.render_widget(Paragraph::new(help_text).block(block), popup_area);
}

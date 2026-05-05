use crate::gh::PrInfo;
use crate::git::{Diff, FileStatus};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph},
};
use std::path::PathBuf;

pub struct GitStatusPane {
    pub root_path: PathBuf,
    pub diff: Diff,
    pub pr_info: Option<PrInfo>,
}

impl GitStatusPane {
    pub fn new(root_path: PathBuf) -> Self {
        Self {
            root_path,
            diff: Diff::default(),
            pr_info: None,
        }
    }

    pub fn update_diff(&mut self, diff: Diff) {
        self.diff = diff;
    }

    pub fn update_pr(&mut self, pr_info: Option<PrInfo>) {
        self.pr_info = pr_info;
    }

    pub fn render(&self, frame: &mut Frame, area: Rect, focused: bool) {
        let border_color = if focused {
            Color::Cyan
        } else {
            Color::DarkGray
        };
        let title_style = if focused {
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        };

        let outer_block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border_color))
            .title(Span::styled(" Git Status ", title_style));

        let inner = outer_block.inner(area);
        frame.render_widget(outer_block, area);

        // Split inner into PR section (top) and diff section (bottom).
        let pr_height = if self.pr_info.is_some() { 4u16 } else { 2u16 };
        let [pr_area, diff_area] =
            Layout::vertical([Constraint::Length(pr_height), Constraint::Min(1)]).areas(inner);

        self.render_pr_section(frame, pr_area);
        self.render_diff_section(frame, diff_area);
    }

    fn render_pr_section(&self, frame: &mut Frame, area: Rect) {
        let block = Block::default()
            .borders(Borders::BOTTOM)
            .border_style(Style::default().fg(Color::DarkGray))
            .title(Span::styled("PR", Style::default().fg(Color::Gray)));

        let inner = block.inner(area);
        frame.render_widget(block, area);

        let content = match &self.pr_info {
            None => vec![Line::from(Span::styled(
                "No open PR",
                Style::default().fg(Color::DarkGray),
            ))],
            Some(pr) => {
                let state_color = match pr.state.as_str() {
                    "OPEN" => Color::Green,
                    "CLOSED" => Color::Red,
                    "MERGED" => Color::Magenta,
                    _ => Color::Gray,
                };
                vec![
                    Line::from(vec![
                        Span::styled(
                            format!("#{} ", pr.number),
                            Style::default()
                                .fg(Color::Cyan)
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(
                            pr.state.clone(),
                            Style::default()
                                .fg(state_color)
                                .add_modifier(Modifier::BOLD),
                        ),
                    ]),
                    Line::from(Span::styled(
                        pr.title.clone(),
                        Style::default().fg(Color::White),
                    )),
                    Line::from(Span::styled(
                        pr.url.clone(),
                        Style::default().fg(Color::Blue),
                    )),
                ]
            }
        };

        frame.render_widget(Paragraph::new(content), inner);
    }

    fn render_diff_section(&self, frame: &mut Frame, area: Rect) {
        let ins = self.diff.insertions;
        let del = self.diff.deletions;
        let title = if ins == 0 && del == 0 {
            " Diff ".to_string()
        } else {
            format!(" Diff  +{}  -{} ", ins, del)
        };

        let block = Block::default()
            .borders(Borders::NONE)
            .title(Span::styled(title, Style::default().fg(Color::Gray)));

        let inner = block.inner(area);
        frame.render_widget(block, area);

        if self.diff.files.is_empty() {
            frame.render_widget(
                Paragraph::new(Span::styled(
                    "No changes",
                    Style::default().fg(Color::DarkGray),
                )),
                inner,
            );
            return;
        }

        let items: Vec<ListItem> = self
            .diff
            .files
            .iter()
            .map(|(status, path)| {
                let (sym, color) = status_style(status);
                let path_str = path.to_string_lossy();
                let line = Line::from(vec![
                    Span::styled(
                        format!("{} ", sym),
                        Style::default().fg(color).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(path_str.to_string(), Style::default().fg(Color::White)),
                ]);
                ListItem::new(line)
            })
            .collect();

        frame.render_widget(List::new(items), inner);
    }
}

fn status_style(status: &FileStatus) -> (&'static str, Color) {
    match status {
        FileStatus::Added => ("A", Color::Green),
        FileStatus::Modified => ("M", Color::Yellow),
        FileStatus::Deleted => ("D", Color::Red),
        FileStatus::Renamed => ("R", Color::Cyan),
        FileStatus::Copied => ("C", Color::Blue),
        FileStatus::Untracked => ("?", Color::Gray),
        FileStatus::Other(_) => ("~", Color::DarkGray),
    }
}

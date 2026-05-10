//! Status / Journal pane — second view of the right column.
//!
//! Two sub-views, toggled with `J`:
//! - **Beads** (default): kanban-style columns grouped by `Status`.
//! - **Journal**: tail of `<runtime>/journal.ndjson`, optional agent filter (`f`).
//!
//! See `docs/plans/agentic-ui-v1.md` §4.2 for the design.

use crossterm::event::{KeyCode, KeyModifiers};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph},
};

use crate::beads::{Bead, Status};
use crate::runtime::JournalEntry;

const JOURNAL_TAIL_MAX: usize = 200;
const KANBAN_COLUMN_LIMIT: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubView {
    Beads,
    Journal,
}

pub struct StatusPane {
    pub sub_view: SubView,
    /// Most recent journal lines (oldest first within the slice). Bounded by
    /// `JOURNAL_TAIL_MAX`.
    pub journal_tail: Vec<JournalEntry>,
    /// When `Some`, journal view shows only entries from this agent id.
    pub journal_filter: Option<String>,
}

impl Default for StatusPane {
    fn default() -> Self {
        Self::new()
    }
}

impl StatusPane {
    pub fn new() -> Self {
        Self {
            sub_view: SubView::Beads,
            journal_tail: Vec::new(),
            journal_filter: None,
        }
    }

    pub fn replace_journal(&mut self, entries: Vec<JournalEntry>) {
        let len = entries.len();
        let start = len.saturating_sub(JOURNAL_TAIL_MAX);
        self.journal_tail = entries.into_iter().skip(start).collect();
    }

    pub fn handle_key(&mut self, key: crossterm::event::KeyEvent) {
        match (key.code, key.modifiers) {
            (KeyCode::Char('J'), _) => {
                self.sub_view = match self.sub_view {
                    SubView::Beads => SubView::Journal,
                    SubView::Journal => SubView::Beads,
                };
            }
            (KeyCode::Char('f'), KeyModifiers::NONE) if self.sub_view == SubView::Journal => {
                // Phase 7: cycle filter through (None -> first known agent ->
                // next -> None). Simple: if any filter set, clear it; otherwise
                // pick the most recent journal entry's agent.
                if self.journal_filter.is_some() {
                    self.journal_filter = None;
                } else if let Some(last) = self.journal_tail.last() {
                    self.journal_filter = Some(last.agent_id.clone());
                }
            }
            _ => {}
        }
    }

    pub fn render(&self, frame: &mut Frame, area: Rect, focused: bool, beads: &[Bead]) {
        let border_color = if focused {
            Color::Cyan
        } else {
            Color::DarkGray
        };
        let title = match self.sub_view {
            SubView::Beads => " Status — Beads ",
            SubView::Journal => " Status — Journal ",
        };
        let title_style = if focused {
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        };

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border_color))
            .title(Span::styled(title, title_style));
        let inner = block.inner(area);
        frame.render_widget(block, area);

        match self.sub_view {
            SubView::Beads => self.render_kanban(frame, inner, beads),
            SubView::Journal => self.render_journal(frame, inner),
        }
    }

    fn render_kanban(&self, frame: &mut Frame, area: Rect, beads: &[Bead]) {
        if beads.is_empty() {
            let hint = vec![
                Line::from(""),
                Line::from(Span::styled(
                    "  No beads yet.",
                    Style::default().fg(Color::DarkGray),
                )),
                Line::from(""),
                Line::from(Span::styled(
                    "  Run `bd init` and `bd create \"…\"` in this repo,",
                    Style::default().fg(Color::DarkGray),
                )),
                Line::from(Span::styled(
                    "  or check `bd doctor` if you expected to see some.",
                    Style::default().fg(Color::DarkGray),
                )),
            ];
            frame.render_widget(Paragraph::new(hint), area);
            return;
        }

        let columns = [
            ("OPEN", Status::Open),
            ("IN-PROG", Status::InProgress),
            ("REVIEW", Status::InReview),
            ("DONE", Status::Closed),
        ];
        let constraints: Vec<Constraint> =
            columns.iter().map(|_| Constraint::Percentage(25)).collect();
        let column_areas = Layout::horizontal(constraints).split(area);

        for (i, (heading, status)) in columns.iter().enumerate() {
            let mut lines = vec![Line::from(Span::styled(
                *heading,
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ))];
            for b in beads
                .iter()
                .filter(|b| b.status == *status)
                .take(KANBAN_COLUMN_LIMIT)
            {
                let line = format!("{} {}", b.id, truncate(&b.title, 20));
                lines.push(Line::from(line));
            }
            frame.render_widget(Paragraph::new(lines), column_areas[i]);
        }
    }

    fn render_journal(&self, frame: &mut Frame, area: Rect) {
        if self.journal_tail.is_empty() {
            let hint = vec![
                Line::from(""),
                Line::from(Span::styled(
                    "  Journal is empty.",
                    Style::default().fg(Color::DarkGray),
                )),
                Line::from(""),
                Line::from(Span::styled(
                    "  Agents append entries via:",
                    Style::default().fg(Color::DarkGray),
                )),
                Line::from(Span::styled(
                    "    fellowship-ctl log <agent-id> \"<message>\"",
                    Style::default().fg(Color::DarkGray),
                )),
                Line::from(""),
                Line::from(Span::styled(
                    "  Press `J` to flip back to the beads view.",
                    Style::default().fg(Color::DarkGray),
                )),
            ];
            frame.render_widget(Paragraph::new(hint), area);
            return;
        }

        let filter_hint = match &self.journal_filter {
            Some(id) => format!(" [filter: {}]  ", id),
            None => String::from(" [no filter]  "),
        };
        let header = Line::from(vec![
            Span::styled(
                filter_hint,
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "press `f` to toggle, `J` to flip back to beads",
                Style::default().fg(Color::DarkGray),
            ),
        ]);

        let items: Vec<ListItem> = self
            .journal_tail
            .iter()
            .filter(|e| {
                self.journal_filter
                    .as_deref()
                    .is_none_or(|f| f == e.agent_id)
            })
            .map(|e| {
                let agent_color = color_for_agent(&e.agent_id);
                let line = Line::from(vec![
                    Span::styled(
                        format!("[{}] ", e.agent_id),
                        Style::default()
                            .fg(agent_color)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(e.message.clone()),
                ]);
                ListItem::new(line)
            })
            .collect();

        let [header_area, list_area] =
            Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).areas(area);
        frame.render_widget(Paragraph::new(header), header_area);
        frame.render_widget(List::new(items), list_area);
    }
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(n.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

fn color_for_agent(id: &str) -> Color {
    // Stable-but-cheap hash → palette pick. Same id always renders the same color.
    let mut h: u32 = 0;
    for b in id.as_bytes() {
        h = h.wrapping_mul(31).wrapping_add(*b as u32);
    }
    const PALETTE: &[Color] = &[
        Color::Cyan,
        Color::Magenta,
        Color::Green,
        Color::Yellow,
        Color::Blue,
        Color::LightRed,
        Color::LightGreen,
        Color::LightCyan,
    ];
    PALETTE[(h as usize) % PALETTE.len()]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyEvent;

    fn key(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, mods)
    }

    fn entry(agent: &str, msg: &str) -> JournalEntry {
        JournalEntry {
            ts_ms: 0,
            agent_id: agent.into(),
            message: msg.into(),
        }
    }

    #[test]
    fn capital_j_toggles_subview() {
        let mut p = StatusPane::new();
        assert_eq!(p.sub_view, SubView::Beads);
        p.handle_key(key(KeyCode::Char('J'), KeyModifiers::SHIFT));
        assert_eq!(p.sub_view, SubView::Journal);
        p.handle_key(key(KeyCode::Char('J'), KeyModifiers::SHIFT));
        assert_eq!(p.sub_view, SubView::Beads);
    }

    #[test]
    fn lowercase_f_only_filters_when_journal_active() {
        let mut p = StatusPane::new();
        p.replace_journal(vec![entry("pm", "hello"), entry("engineer-1", "wip")]);
        // In Beads view, `f` is ignored.
        p.handle_key(key(KeyCode::Char('f'), KeyModifiers::NONE));
        assert!(p.journal_filter.is_none());
        // Switch to Journal view.
        p.handle_key(key(KeyCode::Char('J'), KeyModifiers::SHIFT));
        // Filter latches onto the most recent entry's agent.
        p.handle_key(key(KeyCode::Char('f'), KeyModifiers::NONE));
        assert_eq!(p.journal_filter.as_deref(), Some("engineer-1"));
        // Pressing again clears the filter.
        p.handle_key(key(KeyCode::Char('f'), KeyModifiers::NONE));
        assert!(p.journal_filter.is_none());
    }

    #[test]
    fn replace_journal_caps_to_max_keeping_newest() {
        let mut p = StatusPane::new();
        let entries: Vec<_> = (0..JOURNAL_TAIL_MAX + 50)
            .map(|i| entry("pm", &format!("msg-{}", i)))
            .collect();
        p.replace_journal(entries);
        assert_eq!(p.journal_tail.len(), JOURNAL_TAIL_MAX);
        // First retained entry is offset 50 (we dropped the oldest 50).
        assert_eq!(p.journal_tail.first().unwrap().message, "msg-50");
        // Last retained is the very newest.
        assert_eq!(
            p.journal_tail.last().unwrap().message,
            format!("msg-{}", JOURNAL_TAIL_MAX + 50 - 1)
        );
    }

    #[test]
    fn truncate_short_passthrough_long_ellipsis() {
        assert_eq!(truncate("short", 10), "short");
        assert_eq!(truncate("abcdefghij", 10), "abcdefghij");
        assert_eq!(truncate("abcdefghijk", 10), "abcdefghi…");
    }

    #[test]
    fn color_for_agent_is_stable_per_id() {
        let a = color_for_agent("pm");
        let b = color_for_agent("pm");
        assert_eq!(a, b);
        let c = color_for_agent("orchestrator");
        // Not asserting inequality strictly (palette collisions OK), but the
        // function must be pure.
        let d = color_for_agent("orchestrator");
        assert_eq!(c, d);
    }
}

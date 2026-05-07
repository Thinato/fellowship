use crossterm::event::{KeyCode, KeyModifiers};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState},
};

/// Phase 1 placeholder: hardcoded role list. No PTYs behind these names yet.
/// Phase 2 generalizes the surface keying so `Member` entries can host PTYs.
const PLACEHOLDER_MEMBERS: &[&str] = &[
    "PM",
    "Orchestrator",
    "Architect",
    "Recon",
    "engineer-1",
    "engineer-2",
];

pub struct MembersPane {
    pub members: Vec<String>,
    pub selected: usize,
    pub active: usize,
    list_state: ListState,
}

impl MembersPane {
    pub fn new() -> Self {
        let members: Vec<String> = PLACEHOLDER_MEMBERS
            .iter()
            .map(|s| (*s).to_string())
            .collect();
        let mut list_state = ListState::default();
        list_state.select(Some(0));
        Self {
            members,
            selected: 0,
            active: 0,
            list_state,
        }
    }

    pub fn move_down(&mut self) {
        if !self.members.is_empty() {
            self.selected = (self.selected + 1).min(self.members.len() - 1);
            self.list_state.select(Some(self.selected));
        }
    }

    pub fn move_up(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
            self.list_state.select(Some(self.selected));
        }
    }

    /// Phase 1: Enter sets the "active" member visually. Real PTY swap arrives in Phase 3.
    pub fn handle_key(&mut self, key: crossterm::event::KeyEvent) {
        match (key.code, key.modifiers) {
            (KeyCode::Char('j'), KeyModifiers::NONE) | (KeyCode::Down, _) => self.move_down(),
            (KeyCode::Char('k'), KeyModifiers::NONE) | (KeyCode::Up, _) => self.move_up(),
            (KeyCode::Enter, _) => {
                self.active = self.selected;
            }
            _ => {}
        }
    }

    pub fn render(&mut self, frame: &mut Frame, area: Rect, focused: bool) {
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

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border_color))
            .title(Span::styled(" Members ", title_style));

        let inner = block.inner(area);
        frame.render_widget(block, area);

        let items: Vec<ListItem> = self
            .members
            .iter()
            .enumerate()
            .map(|(idx, name)| {
                let marker = if idx == self.active { "* " } else { "  " };
                let label = format!("{}{}", marker, name);
                let style = if idx == self.active {
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::White)
                };
                ListItem::new(Line::from(Span::styled(label, style)))
            })
            .collect();

        let list = List::new(items)
            .highlight_style(
                Style::default()
                    .bg(Color::Blue)
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol(">");

        frame.render_stateful_widget(list, inner, &mut self.list_state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyEvent;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn new_starts_at_zero_with_active_pm() {
        let m = MembersPane::new();
        assert_eq!(m.selected, 0);
        assert_eq!(m.active, 0);
        assert!(!m.members.is_empty());
        assert_eq!(m.members[0], "PM");
    }

    #[test]
    fn j_moves_down_clamped() {
        let mut m = MembersPane::new();
        for _ in 0..100 {
            m.handle_key(key(KeyCode::Char('j')));
        }
        assert_eq!(m.selected, m.members.len() - 1);
    }

    #[test]
    fn k_moves_up_clamped() {
        let mut m = MembersPane::new();
        m.handle_key(key(KeyCode::Char('j')));
        m.handle_key(key(KeyCode::Char('j')));
        m.handle_key(key(KeyCode::Char('k')));
        assert_eq!(m.selected, 1);
        for _ in 0..10 {
            m.handle_key(key(KeyCode::Char('k')));
        }
        assert_eq!(m.selected, 0);
    }

    #[test]
    fn enter_sets_active_to_selected() {
        let mut m = MembersPane::new();
        m.handle_key(key(KeyCode::Char('j')));
        m.handle_key(key(KeyCode::Char('j')));
        assert_eq!(m.active, 0);
        m.handle_key(key(KeyCode::Enter));
        assert_eq!(m.active, 2);
    }
}

use crossterm::event::{KeyCode, KeyModifiers};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState},
};

use crate::agents::registry::AgentRegistry;
use crate::event::Event;
use crate::surface::{MemberId, Role, Surface};

/// Roles that always exist as singleton members. Engineers are added dynamically
/// in Phase 9 and live alongside these in `MembersPane.members`.
const SINGLETON_ROLES: &[Role] = &[Role::Pm, Role::Orchestrator, Role::Architect, Role::Recon];

pub struct MembersPane {
    pub members: Vec<MemberId>,
    pub selected: usize,
    /// `Some(id)` when the Terminal pane is currently bound to a Member surface.
    /// `None` when bound to a Workspace surface.
    pub active: Option<MemberId>,
    list_state: ListState,
}

impl Default for MembersPane {
    fn default() -> Self {
        Self::new()
    }
}

impl MembersPane {
    pub fn new() -> Self {
        let members: Vec<MemberId> = SINGLETON_ROLES
            .iter()
            .map(|r| MemberId::singleton(*r))
            .collect();
        let mut list_state = ListState::default();
        list_state.select(Some(0));
        Self {
            members,
            selected: 0,
            active: None,
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

    /// Pane state mutator called by App after it processes a SwitchSurface event.
    /// Members pane never decides on its own which surface is active; the App is
    /// the single source of truth.
    pub fn set_active_member(&mut self, id: Option<MemberId>) {
        self.active = id;
    }

    pub fn selected_member(&self) -> Option<MemberId> {
        self.members.get(self.selected).copied()
    }

    /// Returns `Some(Event)` when the keypress should escalate to the App
    /// (currently only `Enter`, which requests a surface switch).
    pub fn handle_key(&mut self, key: crossterm::event::KeyEvent) -> Option<Event> {
        match (key.code, key.modifiers) {
            (KeyCode::Char('j'), KeyModifiers::NONE) | (KeyCode::Down, _) => {
                self.move_down();
                None
            }
            (KeyCode::Char('k'), KeyModifiers::NONE) | (KeyCode::Up, _) => {
                self.move_up();
                None
            }
            (KeyCode::Enter, _) => self
                .selected_member()
                .map(|id| Event::SwitchSurface(Surface::Member(id))),
            _ => None,
        }
    }

    pub fn render(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        focused: bool,
        registry: &AgentRegistry,
    ) {
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
            .map(|id| {
                let is_active = self.active == Some(*id);
                let marker = if is_active { "* " } else { "  " };
                let status_suffix = registry
                    .get(&id.label())
                    .map(|r| format!(" — {}", r.status))
                    .unwrap_or_default();
                let label = format!("{}{}{}", marker, id.label(), status_suffix);
                let style = if is_active {
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
    fn new_lists_four_singletons_no_active() {
        let m = MembersPane::new();
        assert_eq!(m.selected, 0);
        assert!(m.active.is_none());
        assert_eq!(m.members.len(), 4);
        assert_eq!(m.members[0], MemberId::singleton(Role::Pm));
        assert_eq!(m.members[3], MemberId::singleton(Role::Recon));
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
    fn enter_emits_switch_surface_event() {
        let mut m = MembersPane::new();
        m.handle_key(key(KeyCode::Char('j')));
        m.handle_key(key(KeyCode::Char('j')));
        let event = m.handle_key(key(KeyCode::Enter));
        match event {
            Some(Event::SwitchSurface(Surface::Member(id))) => {
                assert_eq!(id, MemberId::singleton(Role::Architect));
            }
            other => panic!("expected SwitchSurface(Member(architect)), got {:?}", other),
        }
    }

    #[test]
    fn set_active_member_drives_render_marker() {
        let mut m = MembersPane::new();
        assert!(m.active.is_none());
        let pm = MemberId::singleton(Role::Pm);
        m.set_active_member(Some(pm));
        assert_eq!(m.active, Some(pm));
        m.set_active_member(None);
        assert!(m.active.is_none());
    }
}

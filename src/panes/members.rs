use crossterm::event::{KeyCode, KeyModifiers};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState},
};

use std::collections::HashSet;

use crate::agents::registry::{AgentRegistry, Liveness};
use crate::event::Event;
use crate::surface::{MemberId, Role, Surface};

/// Roles that always exist as singleton members. Phase 12 removed
/// `Role::Orchestrator` from this list — orchestration is now a native
/// fellowship-side tokio loop (`crate::agents::orchestrator`) that does not
/// need an LLM PTY. The enum variant is kept for journal / heartbeat
/// continuity (a future configuration could re-enable an LLM Orchestrator).
const SINGLETON_ROLES: &[Role] = &[Role::Pm, Role::Architect, Role::Recon];

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

    /// Add an engineer to the bottom of the list if not already present.
    pub fn add_member(&mut self, id: MemberId) {
        if !self.members.contains(&id) {
            self.members.push(id);
        }
    }

    /// Remove a member from the list. If `selected` was on or past the
    /// removed entry it slides up to stay valid.
    pub fn remove_member(&mut self, id: MemberId) {
        if let Some(idx) = self.members.iter().position(|m| *m == id) {
            self.members.remove(idx);
            if self.active == Some(id) {
                self.active = None;
            }
            if self.selected >= self.members.len() && !self.members.is_empty() {
                self.selected = self.members.len() - 1;
            }
            self.list_state.select(Some(self.selected));
        }
    }

    /// All currently-tracked engineer ids (Role::Engineer).
    pub fn engineer_instances(&self) -> Vec<u32> {
        self.members
            .iter()
            .filter(|m| matches!(m.role, Role::Engineer))
            .map(|m| m.instance)
            .collect()
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
        now_ms: u128,
        failed_agents: &HashSet<MemberId>,
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
                let label_text = format!("{}{}{}", marker, id.label(), status_suffix);
                let label_style = if is_active {
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::White)
                };

                // Liveness badge — derived from the registry's heartbeat age
                // unless the watchdog has given up (failed_agents).
                let (badge_text, badge_color) = if failed_agents.contains(id) {
                    (" [DEAD]", Color::Red)
                } else {
                    match registry.liveness_for(&id.label(), now_ms) {
                        Liveness::Live => (" [WORK]", Color::Green),
                        Liveness::Stale => (" [STALE]", Color::Yellow),
                        Liveness::Dead => (" [DEAD]", Color::Red),
                        Liveness::Unknown => ("", Color::Reset),
                    }
                };
                let mut spans = vec![Span::styled(label_text, label_style)];
                if !badge_text.is_empty() {
                    spans.push(Span::styled(
                        badge_text.to_string(),
                        Style::default()
                            .fg(badge_color)
                            .add_modifier(Modifier::BOLD),
                    ));
                }
                ListItem::new(Line::from(spans))
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
    fn new_lists_three_singletons_no_active() {
        // Phase 12 dropped Orchestrator from the singleton list (now native).
        let m = MembersPane::new();
        assert_eq!(m.selected, 0);
        assert!(m.active.is_none());
        assert_eq!(m.members.len(), 3);
        assert_eq!(m.members[0], MemberId::singleton(Role::Pm));
        assert_eq!(m.members[1], MemberId::singleton(Role::Architect));
        assert_eq!(m.members[2], MemberId::singleton(Role::Recon));
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
        // Phase 12: Architect is now index 1 (PM=0, Architect=1, Recon=2).
        let mut m = MembersPane::new();
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

    #[test]
    fn add_member_appends_and_dedupes() {
        let mut m = MembersPane::new();
        let len_before = m.members.len();
        let e1 = MemberId::engineer(1);
        m.add_member(e1);
        assert_eq!(m.members.len(), len_before + 1);
        assert_eq!(*m.members.last().unwrap(), e1);
        // Idempotent: re-adding the same id is a no-op.
        m.add_member(e1);
        assert_eq!(m.members.len(), len_before + 1);
    }

    #[test]
    fn remove_member_clears_active_and_clamps_selection() {
        let mut m = MembersPane::new();
        let e1 = MemberId::engineer(1);
        m.add_member(e1);
        m.set_active_member(Some(e1));
        m.selected = m.members.len() - 1;
        m.remove_member(e1);
        assert!(m.active.is_none());
        assert!(m.selected < m.members.len());
        assert!(!m.members.contains(&e1));
    }

    #[test]
    fn engineer_instances_returns_only_engineer_role_ids() {
        let mut m = MembersPane::new();
        m.add_member(MemberId::engineer(1));
        m.add_member(MemberId::engineer(3));
        let mut got = m.engineer_instances();
        got.sort();
        assert_eq!(got, vec![1, 3]);
    }
}

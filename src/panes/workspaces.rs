use crossterm::event::{KeyCode, KeyModifiers};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph},
};
use std::path::PathBuf;
use tui_input::Input;
use tui_input::backend::crossterm::EventHandler;

use crate::event::Event;
use crate::git::Worktree;

pub struct WorkspacesPane {
    #[allow(dead_code)]
    pub root_path: PathBuf,
    pub worktrees: Vec<Worktree>,
    pub selected: usize,
    pub modal_open: bool,
    pub modal_input: Input,
    list_state: ListState,
}

impl WorkspacesPane {
    pub fn new(root_path: PathBuf) -> Self {
        let mut list_state = ListState::default();
        list_state.select(Some(0));
        Self {
            root_path,
            worktrees: vec![],
            selected: 0,
            modal_open: false,
            modal_input: Input::default(),
            list_state,
        }
    }

    pub fn set_worktrees(&mut self, worktrees: Vec<Worktree>) {
        self.worktrees = worktrees;
        if self.selected >= self.worktrees.len() && !self.worktrees.is_empty() {
            self.selected = self.worktrees.len() - 1;
        }
        self.list_state.select(Some(self.selected));
    }

    pub fn select_path(&mut self, path: &std::path::Path) {
        if let Some(idx) = self.worktrees.iter().position(|w| w.path == path) {
            self.selected = idx;
            self.list_state.select(Some(idx));
        }
    }

    pub fn move_down(&mut self) {
        if !self.worktrees.is_empty() {
            self.selected = (self.selected + 1).min(self.worktrees.len() - 1);
            self.list_state.select(Some(self.selected));
        }
    }

    pub fn move_up(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
            self.list_state.select(Some(self.selected));
        }
    }

    pub fn move_top(&mut self) {
        self.selected = 0;
        self.list_state.select(Some(0));
    }

    pub fn move_bottom(&mut self) {
        if !self.worktrees.is_empty() {
            self.selected = self.worktrees.len() - 1;
            self.list_state.select(Some(self.selected));
        }
    }

    pub fn open_modal(&mut self) {
        self.modal_input = Input::default();
        self.modal_open = true;
    }

    pub fn close_modal(&mut self) {
        self.modal_open = false;
        self.modal_input = Input::default();
    }

    pub fn selected_worktree(&self) -> Option<&Worktree> {
        self.worktrees.get(self.selected)
    }

    fn take_modal_branch(&mut self) -> String {
        let branch = self.modal_input.value().to_string();
        self.close_modal();
        branch
    }

    /// Handle a key event. Returns Some(Event) if an app-level event should fire.
    pub fn handle_key(&mut self, key: crossterm::event::KeyEvent) -> Option<Event> {
        if self.modal_open {
            match key.code {
                KeyCode::Esc => {
                    self.close_modal();
                    None
                }
                KeyCode::Enter => {
                    let branch = self.take_modal_branch();
                    if !branch.is_empty() {
                        Some(Event::CreateWorktree(branch))
                    } else {
                        None
                    }
                }
                _ => {
                    self.modal_input
                        .handle_event(&crossterm::event::Event::Key(key));
                    None
                }
            }
        } else {
            match (key.code, key.modifiers) {
                (KeyCode::Char('j'), KeyModifiers::NONE) | (KeyCode::Down, _) => {
                    self.move_down();
                    None
                }
                (KeyCode::Char('k'), KeyModifiers::NONE) | (KeyCode::Up, _) => {
                    self.move_up();
                    None
                }
                (KeyCode::Char('g'), KeyModifiers::NONE) => {
                    self.move_top();
                    None
                }
                (KeyCode::Char('G'), KeyModifiers::NONE) => {
                    self.move_bottom();
                    None
                }
                (KeyCode::Char('r'), KeyModifiers::NONE) => Some(Event::GitRefresh),
                (KeyCode::Char('n'), KeyModifiers::NONE) => {
                    self.open_modal();
                    None
                }
                (KeyCode::Enter, _) => self
                    .selected_worktree()
                    .map(|wt| Event::SwitchWorkspace(wt.path.clone())),
                _ => None,
            }
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
            .title(Span::styled(" Workspaces ", title_style));

        let inner = block.inner(area);
        frame.render_widget(block, area);

        let items: Vec<ListItem> = self
            .worktrees
            .iter()
            .map(|wt| {
                let branch = wt.branch.as_deref().unwrap_or("(detached)");
                let marker = if wt.is_current { "* " } else { "  " };
                let label = format!("{}{}", marker, branch);
                let style = if wt.is_current {
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

        if self.modal_open {
            self.render_modal(frame, area);
        }
    }

    fn render_modal(&self, frame: &mut Frame, parent_area: Rect) {
        let modal_width = 40u16;
        let modal_height = 5u16;
        let x = parent_area.x + parent_area.width.saturating_sub(modal_width) / 2;
        let y = parent_area.y + parent_area.height.saturating_sub(modal_height) / 2;
        let modal_area = Rect::new(
            x,
            y,
            modal_width.min(parent_area.width),
            modal_height.min(parent_area.height),
        );

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Yellow))
            .title(Span::styled(
                " New Worktree ",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ));

        let inner = block.inner(modal_area);
        frame.render_widget(Clear, modal_area);
        frame.render_widget(block, modal_area);

        let [prompt_area, input_area] =
            Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).areas(inner);

        frame.render_widget(
            Paragraph::new("Branch name:").style(Style::default().fg(Color::Gray)),
            prompt_area,
        );

        let display = format!("{}_", self.modal_input.value());
        frame.render_widget(
            Paragraph::new(display).style(Style::default().fg(Color::White)),
            input_area,
        );
    }
}

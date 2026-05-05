use std::collections::HashMap;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::PaneId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    FocusPane(PaneId),
    Quit,
    ToggleHelp,
    SendLiteralPrefix,
    PassThrough,
    Consume,
}

#[derive(Debug)]
pub enum InputMode {
    Normal,
    AwaitingPrefixFollower,
}

pub struct Keymap {
    pub bindings: HashMap<KeyEvent, Action>,
}

impl Keymap {
    pub fn new(bindings: HashMap<KeyEvent, Action>) -> Self {
        Self { bindings }
    }

    /// Process a key event given the current mode and focus pane.
    /// Prefix mode persists indefinitely; Esc exits without action.
    pub fn handle(&self, mode: &mut InputMode, key: KeyEvent, _focus: PaneId) -> Action {
        match mode {
            InputMode::Normal => {
                if key == prefix_key() {
                    *mode = InputMode::AwaitingPrefixFollower;
                    return Action::Consume;
                }
                Action::PassThrough
            }
            InputMode::AwaitingPrefixFollower => {
                *mode = InputMode::Normal;
                if key.code == KeyCode::Esc {
                    return Action::Consume;
                }
                self.bindings.get(&key).cloned().unwrap_or(Action::Consume)
            }
        }
    }
}

pub fn default_bindings() -> HashMap<KeyEvent, Action> {
    let mut map = HashMap::new();
    map.insert(
        KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE),
        Action::FocusPane(PaneId::Workspaces),
    );
    map.insert(
        KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE),
        Action::FocusPane(PaneId::Terminal),
    );
    map.insert(
        KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE),
        Action::FocusPane(PaneId::GitStatus),
    );
    map.insert(
        KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE),
        Action::Quit,
    );
    map.insert(
        KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE),
        Action::ToggleHelp,
    );
    map.insert(
        KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL),
        Action::SendLiteralPrefix,
    );
    map
}

pub fn prefix_key() -> KeyEvent {
    KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_keymap() -> Keymap {
        Keymap::new(default_bindings())
    }

    fn key(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, mods)
    }

    fn char_key(c: char) -> KeyEvent {
        key(KeyCode::Char(c), KeyModifiers::NONE)
    }

    fn ctrl_a() -> KeyEvent {
        key(KeyCode::Char('a'), KeyModifiers::CONTROL)
    }

    #[test]
    fn prefix_then_e_focuses_workspaces() {
        let km = make_keymap();
        let mut mode = InputMode::Normal;

        let a1 = km.handle(&mut mode, ctrl_a(), PaneId::GitStatus);
        assert_eq!(a1, Action::Consume);
        assert!(matches!(mode, InputMode::AwaitingPrefixFollower));

        let a2 = km.handle(&mut mode, char_key('e'), PaneId::GitStatus);
        assert_eq!(a2, Action::FocusPane(PaneId::Workspaces));
        assert!(matches!(mode, InputMode::Normal));
    }

    #[test]
    fn prefix_then_t_focuses_terminal() {
        let km = make_keymap();
        let mut mode = InputMode::Normal;

        km.handle(&mut mode, ctrl_a(), PaneId::Workspaces);
        let a = km.handle(&mut mode, char_key('t'), PaneId::Workspaces);
        assert_eq!(a, Action::FocusPane(PaneId::Terminal));
    }

    #[test]
    fn prefix_then_g_focuses_gitstatus() {
        let km = make_keymap();
        let mut mode = InputMode::Normal;

        km.handle(&mut mode, ctrl_a(), PaneId::Terminal);
        let a = km.handle(&mut mode, char_key('g'), PaneId::Terminal);
        assert_eq!(a, Action::FocusPane(PaneId::GitStatus));
    }

    #[test]
    fn prefix_then_q_quits() {
        let km = make_keymap();
        let mut mode = InputMode::Normal;

        km.handle(&mut mode, ctrl_a(), PaneId::Terminal);
        let a = km.handle(&mut mode, char_key('q'), PaneId::Terminal);
        assert_eq!(a, Action::Quit);
    }

    #[test]
    fn double_prefix_emits_send_literal_prefix() {
        let km = make_keymap();
        let mut mode = InputMode::Normal;

        km.handle(&mut mode, ctrl_a(), PaneId::Terminal);
        let a = km.handle(&mut mode, ctrl_a(), PaneId::Terminal);
        assert_eq!(a, Action::SendLiteralPrefix);
    }

    #[test]
    fn esc_cancels_prefix_mode() {
        let km = make_keymap();
        let mut mode = InputMode::Normal;

        km.handle(&mut mode, ctrl_a(), PaneId::Workspaces);
        assert!(matches!(mode, InputMode::AwaitingPrefixFollower));

        let a = km.handle(
            &mut mode,
            key(KeyCode::Esc, KeyModifiers::NONE),
            PaneId::Workspaces,
        );
        assert_eq!(a, Action::Consume);
        assert!(matches!(mode, InputMode::Normal));
    }

    #[test]
    fn unbound_follower_silently_dropped() {
        let km = make_keymap();
        let mut mode = InputMode::Normal;

        km.handle(&mut mode, ctrl_a(), PaneId::Terminal);
        let a = km.handle(&mut mode, char_key('z'), PaneId::Terminal);
        assert_eq!(a, Action::Consume);
        assert!(matches!(mode, InputMode::Normal));
    }

    #[test]
    fn terminal_focus_passes_through_normal_keys() {
        let km = make_keymap();
        let mut mode = InputMode::Normal;

        let a = km.handle(&mut mode, char_key('q'), PaneId::Terminal);
        assert_eq!(a, Action::PassThrough);

        let a2 = km.handle(
            &mut mode,
            key(KeyCode::Esc, KeyModifiers::NONE),
            PaneId::Terminal,
        );
        assert_eq!(a2, Action::PassThrough);
    }
}

use std::collections::HashMap;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::PaneId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dir {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    FocusPane(PaneId),
    FocusDir(Dir),
    SwitchWorktree(usize),
    Quit,
    CyclePane,
    ToggleHelp,
    SendLiteralPrefix,
    EnterCommandMode,
    ExecuteCommand(String),
    PassThrough,
    Consume,
}

#[derive(Debug)]
pub enum InputMode {
    Normal,
    AwaitingPrefixFollower,
    Command(String),
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
                let action = self.bindings.get(&key).cloned().unwrap_or(Action::Consume);
                if matches!(action, Action::EnterCommandMode) {
                    *mode = InputMode::Command(String::new());
                    return Action::Consume;
                }
                action
            }
            InputMode::Command(buf) => match key.code {
                KeyCode::Esc => {
                    *mode = InputMode::Normal;
                    Action::Consume
                }
                KeyCode::Enter => {
                    let cmd = std::mem::take(buf);
                    *mode = InputMode::Normal;
                    Action::ExecuteCommand(cmd)
                }
                KeyCode::Backspace => {
                    if buf.pop().is_none() {
                        *mode = InputMode::Normal;
                    }
                    Action::Consume
                }
                KeyCode::Char(c) => {
                    if key.modifiers.contains(KeyModifiers::CONTROL) && (c == 'c' || c == 'g') {
                        *mode = InputMode::Normal;
                        return Action::Consume;
                    }
                    buf.push(c);
                    Action::Consume
                }
                _ => Action::Consume,
            },
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
        KeyEvent::new(KeyCode::Char('m'), KeyModifiers::NONE),
        Action::FocusPane(PaneId::Members),
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
        KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE),
        Action::CyclePane,
    );
    map.insert(
        KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL),
        Action::SendLiteralPrefix,
    );
    map.insert(
        KeyEvent::new(KeyCode::Char(':'), KeyModifiers::NONE),
        Action::EnterCommandMode,
    );
    map.insert(
        KeyEvent::new(KeyCode::Char(':'), KeyModifiers::SHIFT),
        Action::EnterCommandMode,
    );
    map.insert(
        KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE),
        Action::FocusDir(Dir::Left),
    );
    map.insert(
        KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE),
        Action::FocusDir(Dir::Down),
    );
    map.insert(
        KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE),
        Action::FocusDir(Dir::Up),
    );
    map.insert(
        KeyEvent::new(KeyCode::Char('l'), KeyModifiers::NONE),
        Action::FocusDir(Dir::Right),
    );
    for n in 1..=9u8 {
        let c = char::from(b'0' + n);
        map.insert(
            KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE),
            Action::SwitchWorktree((n - 1) as usize),
        );
    }
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
    fn prefix_then_digit_switches_worktree() {
        let km = make_keymap();
        let mut mode = InputMode::Normal;

        km.handle(&mut mode, ctrl_a(), PaneId::Terminal);
        let a = km.handle(&mut mode, char_key('1'), PaneId::Terminal);
        assert_eq!(a, Action::SwitchWorktree(0));

        km.handle(&mut mode, ctrl_a(), PaneId::Terminal);
        let a = km.handle(&mut mode, char_key('9'), PaneId::Terminal);
        assert_eq!(a, Action::SwitchWorktree(8));
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
    fn prefix_then_h_focuses_dir_left() {
        let km = make_keymap();
        let mut mode = InputMode::Normal;
        km.handle(&mut mode, ctrl_a(), PaneId::Terminal);
        let a = km.handle(&mut mode, char_key('h'), PaneId::Terminal);
        assert_eq!(a, Action::FocusDir(Dir::Left));
    }

    #[test]
    fn prefix_then_j_focuses_dir_down() {
        let km = make_keymap();
        let mut mode = InputMode::Normal;
        km.handle(&mut mode, ctrl_a(), PaneId::Terminal);
        let a = km.handle(&mut mode, char_key('j'), PaneId::Terminal);
        assert_eq!(a, Action::FocusDir(Dir::Down));
    }

    #[test]
    fn prefix_then_k_focuses_dir_up() {
        let km = make_keymap();
        let mut mode = InputMode::Normal;
        km.handle(&mut mode, ctrl_a(), PaneId::Terminal);
        let a = km.handle(&mut mode, char_key('k'), PaneId::Terminal);
        assert_eq!(a, Action::FocusDir(Dir::Up));
    }

    #[test]
    fn prefix_then_l_focuses_dir_right() {
        let km = make_keymap();
        let mut mode = InputMode::Normal;
        km.handle(&mut mode, ctrl_a(), PaneId::Terminal);
        let a = km.handle(&mut mode, char_key('l'), PaneId::Terminal);
        assert_eq!(a, Action::FocusDir(Dir::Right));
    }

    #[test]
    fn prefix_then_m_focuses_members() {
        let km = make_keymap();
        let mut mode = InputMode::Normal;
        km.handle(&mut mode, ctrl_a(), PaneId::Terminal);
        let a = km.handle(&mut mode, char_key('m'), PaneId::Terminal);
        assert_eq!(a, Action::FocusPane(PaneId::Members));
    }

    #[test]
    fn prefix_then_o_cycles_pane() {
        let km = make_keymap();
        let mut mode = InputMode::Normal;
        km.handle(&mut mode, ctrl_a(), PaneId::Terminal);
        let a = km.handle(&mut mode, char_key('o'), PaneId::Terminal);
        assert_eq!(a, Action::CyclePane);
    }

    #[test]
    fn prefix_then_colon_enters_command_mode() {
        let km = make_keymap();
        let mut mode = InputMode::Normal;
        km.handle(&mut mode, ctrl_a(), PaneId::Terminal);
        let a = km.handle(&mut mode, char_key(':'), PaneId::Terminal);
        assert_eq!(a, Action::Consume);
        assert!(matches!(mode, InputMode::Command(ref s) if s.is_empty()));
    }

    #[test]
    fn command_mode_collects_chars_and_executes_on_enter() {
        let km = make_keymap();
        let mut mode = InputMode::Command(String::new());
        for c in "quit".chars() {
            let a = km.handle(&mut mode, char_key(c), PaneId::Terminal);
            assert_eq!(a, Action::Consume);
        }
        let a = km.handle(
            &mut mode,
            key(KeyCode::Enter, KeyModifiers::NONE),
            PaneId::Terminal,
        );
        assert_eq!(a, Action::ExecuteCommand("quit".into()));
        assert!(matches!(mode, InputMode::Normal));
    }

    #[test]
    fn command_mode_esc_cancels() {
        let km = make_keymap();
        let mut mode = InputMode::Command("foo".into());
        let a = km.handle(
            &mut mode,
            key(KeyCode::Esc, KeyModifiers::NONE),
            PaneId::Terminal,
        );
        assert_eq!(a, Action::Consume);
        assert!(matches!(mode, InputMode::Normal));
    }

    #[test]
    fn command_mode_backspace_deletes_then_exits_when_empty() {
        let km = make_keymap();
        let mut mode = InputMode::Command("ab".into());
        km.handle(
            &mut mode,
            key(KeyCode::Backspace, KeyModifiers::NONE),
            PaneId::Terminal,
        );
        assert!(matches!(mode, InputMode::Command(ref s) if s == "a"));
        km.handle(
            &mut mode,
            key(KeyCode::Backspace, KeyModifiers::NONE),
            PaneId::Terminal,
        );
        assert!(matches!(mode, InputMode::Command(ref s) if s.is_empty()));
        km.handle(
            &mut mode,
            key(KeyCode::Backspace, KeyModifiers::NONE),
            PaneId::Terminal,
        );
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

use std::collections::HashMap;
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::PaneId;

const PREFIX_TIMEOUT: Duration = Duration::from_secs(1);

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
    AwaitingPrefixFollower { since: Instant },
}

pub struct Keymap {
    pub bindings: HashMap<KeyEvent, Action>,
}

impl Keymap {
    pub fn new(bindings: HashMap<KeyEvent, Action>) -> Self {
        Self { bindings }
    }

    /// Process a key event given the current mode and focus pane.
    /// `now` is injectable for deterministic timeout testing.
    pub fn handle(&self, mode: &mut InputMode, key: KeyEvent, focus: PaneId, now: Instant) -> Action {
        match mode {
            InputMode::Normal => {
                if key == prefix_key() {
                    *mode = InputMode::AwaitingPrefixFollower { since: now };
                    return Action::Consume;
                }
                if focus == PaneId::Terminal {
                    return Action::PassThrough;
                }
                Action::PassThrough
            }
            InputMode::AwaitingPrefixFollower { since } => {
                let elapsed = now.duration_since(*since);
                *mode = InputMode::Normal;
                if elapsed > PREFIX_TIMEOUT {
                    // Timeout: re-dispatch key as if in Normal mode
                    return self.handle(mode, key, focus, now);
                }
                // Look up binding; unbound follower is silently dropped (Consume)
                self.bindings
                    .get(&key)
                    .cloned()
                    .unwrap_or(Action::Consume)
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
        KeyEvent::new(KeyCode::Char(' '), KeyModifiers::CONTROL),
        Action::SendLiteralPrefix,
    );
    map
}

pub fn prefix_key() -> KeyEvent {
    KeyEvent::new(KeyCode::Char(' '), KeyModifiers::CONTROL)
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

    fn ctrl_space() -> KeyEvent {
        key(KeyCode::Char(' '), KeyModifiers::CONTROL)
    }

    /// Returns an Instant that is `secs` seconds after the epoch-like base.
    /// We use Instant::now() as base and offset — since we pass `now` explicitly,
    /// we control elapsed by using a base in the past.
    fn instant_ago(secs: u64) -> Instant {
        Instant::now() - Duration::from_secs(secs)
    }

    #[test]
    fn prefix_then_e_focuses_workspaces() {
        let km = make_keymap();
        let mut mode = InputMode::Normal;
        let now = Instant::now();

        let a1 = km.handle(&mut mode, ctrl_space(), PaneId::GitStatus, now);
        assert_eq!(a1, Action::Consume);
        assert!(matches!(mode, InputMode::AwaitingPrefixFollower { .. }));

        let a2 = km.handle(&mut mode, char_key('e'), PaneId::GitStatus, now);
        assert_eq!(a2, Action::FocusPane(PaneId::Workspaces));
        assert!(matches!(mode, InputMode::Normal));
    }

    #[test]
    fn prefix_then_t_focuses_terminal() {
        let km = make_keymap();
        let mut mode = InputMode::Normal;
        let now = Instant::now();

        km.handle(&mut mode, ctrl_space(), PaneId::Workspaces, now);
        let a = km.handle(&mut mode, char_key('t'), PaneId::Workspaces, now);
        assert_eq!(a, Action::FocusPane(PaneId::Terminal));
    }

    #[test]
    fn prefix_then_g_focuses_gitstatus() {
        let km = make_keymap();
        let mut mode = InputMode::Normal;
        let now = Instant::now();

        km.handle(&mut mode, ctrl_space(), PaneId::Terminal, now);
        let a = km.handle(&mut mode, char_key('g'), PaneId::Terminal, now);
        assert_eq!(a, Action::FocusPane(PaneId::GitStatus));
    }

    #[test]
    fn prefix_then_q_quits() {
        let km = make_keymap();
        let mut mode = InputMode::Normal;
        let now = Instant::now();

        km.handle(&mut mode, ctrl_space(), PaneId::Terminal, now);
        let a = km.handle(&mut mode, char_key('q'), PaneId::Terminal, now);
        assert_eq!(a, Action::Quit);
    }

    #[test]
    fn double_prefix_emits_send_literal_prefix() {
        let km = make_keymap();
        let mut mode = InputMode::Normal;
        let now = Instant::now();

        km.handle(&mut mode, ctrl_space(), PaneId::Terminal, now);
        let a = km.handle(&mut mode, ctrl_space(), PaneId::Terminal, now);
        assert_eq!(a, Action::SendLiteralPrefix);
    }

    #[test]
    fn prefix_timeout_drops_state_and_redispatches() {
        let km = make_keymap();
        let mut mode = InputMode::Normal;

        // Simulate prefix pressed 2 seconds ago
        let prefix_time = instant_ago(2);
        km.handle(&mut mode, ctrl_space(), PaneId::Workspaces, prefix_time);
        assert!(matches!(mode, InputMode::AwaitingPrefixFollower { .. }));

        // now = current time; elapsed > 1s → timeout, key is re-dispatched as Normal
        let now = Instant::now();
        // 'e' in Normal mode with Workspaces focus → PassThrough (pane handles it)
        let a = km.handle(&mut mode, char_key('e'), PaneId::Workspaces, now);
        assert_eq!(a, Action::PassThrough);
        assert!(matches!(mode, InputMode::Normal));
    }

    #[test]
    fn unbound_follower_silently_dropped() {
        let km = make_keymap();
        let mut mode = InputMode::Normal;
        let now = Instant::now();

        km.handle(&mut mode, ctrl_space(), PaneId::Terminal, now);
        // 'z' is not bound
        let a = km.handle(&mut mode, char_key('z'), PaneId::Terminal, now);
        assert_eq!(a, Action::Consume);
        assert!(matches!(mode, InputMode::Normal));
    }

    #[test]
    fn terminal_focus_passes_through_normal_keys() {
        let km = make_keymap();
        let mut mode = InputMode::Normal;
        let now = Instant::now();

        // Any non-prefix key while focused on Terminal → PassThrough
        let a = km.handle(&mut mode, char_key('q'), PaneId::Terminal, now);
        assert_eq!(a, Action::PassThrough);

        let a2 = km.handle(&mut mode, key(KeyCode::Esc, KeyModifiers::NONE), PaneId::Terminal, now);
        assert_eq!(a2, Action::PassThrough);
    }
}

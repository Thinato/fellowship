use crate::app::PaneId;
use crate::keymap::Dir;

#[derive(Debug, Clone, Copy)]
pub struct Slot {
    pub pane: PaneId,
    pub col: u8,
    pub row: u8,
}

pub struct PaneLayout {
    pub slots: Vec<Slot>,
}

impl PaneLayout {
    /// Default three-column horizontal layout: Workspaces | Terminal | GitStatus
    pub fn default_horizontal() -> Self {
        Self {
            slots: vec![
                Slot {
                    pane: PaneId::Workspaces,
                    col: 0,
                    row: 0,
                },
                Slot {
                    pane: PaneId::Terminal,
                    col: 1,
                    row: 0,
                },
                Slot {
                    pane: PaneId::GitStatus,
                    col: 2,
                    row: 0,
                },
            ],
        }
    }

    pub fn slot(&self, pane: PaneId) -> Option<&Slot> {
        self.slots.iter().find(|s| s.pane == pane)
    }

    /// Return the nearest neighbor of `from` in direction `dir`, or `None` if no candidate exists.
    ///
    /// For horizontal moves (Left/Right): prefer same row, then closest col distance.
    /// For vertical moves (Up/Down): prefer same col, then closest row distance.
    /// Ties broken by smallest distance on the secondary axis.
    pub fn neighbor(&self, from: PaneId, dir: Dir) -> Option<PaneId> {
        let from_slot = self.slot(from)?;
        let (fc, fr) = (from_slot.col, from_slot.row);

        let candidates: Vec<&Slot> = self
            .slots
            .iter()
            .filter(|s| match dir {
                Dir::Left => s.col < fc,
                Dir::Right => s.col > fc,
                Dir::Up => s.row < fr,
                Dir::Down => s.row > fr,
            })
            .collect();

        if candidates.is_empty() {
            return None;
        }

        let best = candidates.into_iter().min_by_key(|s| {
            let col_dist = (s.col as i16 - fc as i16).unsigned_abs();
            let row_dist = (s.row as i16 - fr as i16).unsigned_abs();
            match dir {
                Dir::Left | Dir::Right => {
                    let same_row = u16::from(s.row != fr);
                    (same_row, col_dist, row_dist)
                }
                Dir::Up | Dir::Down => {
                    let same_col = u16::from(s.col != fc);
                    (same_col, row_dist, col_dist)
                }
            }
        })?;

        Some(best.pane)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default() -> PaneLayout {
        PaneLayout::default_horizontal()
    }

    // --- Default layout: Workspaces(0,0) | Terminal(1,0) | GitStatus(2,0) ---

    #[test]
    fn default_terminal_left_is_workspaces() {
        assert_eq!(
            default().neighbor(PaneId::Terminal, Dir::Left),
            Some(PaneId::Workspaces)
        );
    }

    #[test]
    fn default_terminal_right_is_gitstatus() {
        assert_eq!(
            default().neighbor(PaneId::Terminal, Dir::Right),
            Some(PaneId::GitStatus)
        );
    }

    #[test]
    fn default_terminal_up_is_none() {
        assert_eq!(default().neighbor(PaneId::Terminal, Dir::Up), None);
    }

    #[test]
    fn default_terminal_down_is_none() {
        assert_eq!(default().neighbor(PaneId::Terminal, Dir::Down), None);
    }

    #[test]
    fn default_workspaces_left_is_none() {
        assert_eq!(default().neighbor(PaneId::Workspaces, Dir::Left), None);
    }

    #[test]
    fn default_workspaces_right_is_terminal() {
        assert_eq!(
            default().neighbor(PaneId::Workspaces, Dir::Right),
            Some(PaneId::Terminal)
        );
    }

    #[test]
    fn default_workspaces_up_is_none() {
        assert_eq!(default().neighbor(PaneId::Workspaces, Dir::Up), None);
    }

    #[test]
    fn default_workspaces_down_is_none() {
        assert_eq!(default().neighbor(PaneId::Workspaces, Dir::Down), None);
    }

    #[test]
    fn default_gitstatus_left_is_terminal() {
        assert_eq!(
            default().neighbor(PaneId::GitStatus, Dir::Left),
            Some(PaneId::Terminal)
        );
    }

    #[test]
    fn default_gitstatus_right_is_none() {
        assert_eq!(default().neighbor(PaneId::GitStatus, Dir::Right), None);
    }

    #[test]
    fn default_gitstatus_up_is_none() {
        assert_eq!(default().neighbor(PaneId::GitStatus, Dir::Up), None);
    }

    #[test]
    fn default_gitstatus_down_is_none() {
        assert_eq!(default().neighbor(PaneId::GitStatus, Dir::Down), None);
    }

    // --- Custom layout: Workspaces(0,0) top-left, Terminal(1,0) top-right,
    //     GitStatus(0,1) bottom ---

    fn custom() -> PaneLayout {
        PaneLayout {
            slots: vec![
                Slot {
                    pane: PaneId::Workspaces,
                    col: 0,
                    row: 0,
                },
                Slot {
                    pane: PaneId::Terminal,
                    col: 1,
                    row: 0,
                },
                Slot {
                    pane: PaneId::GitStatus,
                    col: 0,
                    row: 1,
                },
            ],
        }
    }

    #[test]
    fn custom_workspaces_down_is_gitstatus() {
        assert_eq!(
            custom().neighbor(PaneId::Workspaces, Dir::Down),
            Some(PaneId::GitStatus)
        );
    }

    #[test]
    fn custom_terminal_down_is_gitstatus() {
        // GitStatus is the only candidate below; col differs but it's the only option.
        assert_eq!(
            custom().neighbor(PaneId::Terminal, Dir::Down),
            Some(PaneId::GitStatus)
        );
    }

    #[test]
    fn custom_gitstatus_up_is_workspaces() {
        // Workspaces (0,0) shares col=0; Terminal (1,0) does not. Same-col wins.
        assert_eq!(
            custom().neighbor(PaneId::GitStatus, Dir::Up),
            Some(PaneId::Workspaces)
        );
    }

    #[test]
    fn custom_gitstatus_right_is_terminal() {
        assert_eq!(
            custom().neighbor(PaneId::GitStatus, Dir::Right),
            Some(PaneId::Terminal)
        );
    }
}

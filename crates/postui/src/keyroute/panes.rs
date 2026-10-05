//! Pane geometry for `ctrl+w` (piece 4 spec §5.8): the sidebar on the
//! left, the editor over the response on the right. Pure: `App` feeds it
//! the focused pane and what it remembers.

use crate::action::PaneDir;
use crate::layout::PaneId;

/// What `<C-w>l` and `<C-w>p` remember (spec §5.8): the right-hand pane
/// last used, and the main pane the keyboard was on before this one.
/// `App::note_pane` feeds it at the start of each input event, key or
/// mouse (plan M4a Deviation 6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PaneMemory {
    seen: Option<PaneId>,
    right: PaneId,
    prev: Option<PaneId>,
}

impl Default for PaneMemory {
    fn default() -> Self {
        Self { seen: None, right: PaneId::Editor, prev: None }
    }
}

impl PaneMemory {
    /// The keyboard is on `now`.
    pub fn note(&mut self, now: PaneId) {
        if let Some(seen) = self.seen
            && seen != now
        {
            self.prev = Some(seen);
        }
        self.seen = Some(now);
        if matches!(now, PaneId::Editor | PaneId::Response) {
            self.right = now;
        }
    }
}

/// The main pane `dir` reaches from `from`, or `None` where there is none:
/// `h` from either right-hand pane is the sidebar, `l` from the sidebar is
/// the right-hand pane last used (the user's ruling, 2026-09-23), `j` and
/// `k` move between the editor and the response, `w` `W` cycle, `p` is the
/// pane before.
pub fn toward(from: PaneId, dir: PaneDir, mem: &PaneMemory) -> Option<PaneId> {
    let to = match (dir, from) {
        (PaneDir::Next, f) => f.next(),
        (PaneDir::Prev, f) => f.prev(),
        (PaneDir::Last, _) => mem.prev?,
        (PaneDir::Left, PaneId::Editor | PaneId::Response) => PaneId::Sidebar,
        (PaneDir::Right, PaneId::Sidebar) => mem.right,
        (PaneDir::Down, PaneId::Editor) => PaneId::Response,
        (PaneDir::Up, PaneId::Response) => PaneId::Editor,
        _ => return None,
    };
    (to != from).then_some(to)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::action::PaneDir::*;
    use crate::layout::PaneId::*;

    #[test]
    fn the_geometry() {
        let mem = PaneMemory::default();
        assert_eq!(toward(Response, Left, &mem), Some(Sidebar));
        assert_eq!(toward(Editor, Left, &mem), Some(Sidebar));
        assert_eq!(toward(Sidebar, Left, &mem), None, "nothing left of the sidebar");
        assert_eq!(toward(Editor, Down, &mem), Some(Response));
        assert_eq!(toward(Response, Up, &mem), Some(Editor));
        assert_eq!(toward(Sidebar, Down, &mem), None);
        assert_eq!(toward(Editor, Right, &mem), None, "the editor's right is the edge");
        assert_eq!(toward(Sidebar, Right, &mem), Some(Editor), "before any right pane: the editor");
        assert_eq!(toward(Response, Next, &mem), Some(Sidebar));
        assert_eq!(toward(Sidebar, Prev, &mem), Some(Response));
        assert_eq!(toward(Editor, Last, &mem), None, "no earlier pane");
    }

    #[test]
    fn memory_learns_the_right_pane_and_the_previous_one() {
        let mut mem = PaneMemory::default();
        mem.note(Response);
        assert_eq!(toward(Response, Last, &mem), None, "the first note has nothing before it");
        mem.note(Sidebar);
        assert_eq!(toward(Sidebar, Right, &mem), Some(Response), "the right pane last used");
        assert_eq!(toward(Sidebar, Last, &mem), Some(Response));
        mem.note(Sidebar);
        assert_eq!(toward(Sidebar, Last, &mem), Some(Response), "no change, no new memory");
    }
}

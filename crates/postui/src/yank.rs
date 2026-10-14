//! The row register: what `yy` (vim) / `y` (arrows) remembers until a
//! `p`/`P` spends it (piece 4 spec §5.1).
//!
//! It is an in-app register, not the system clipboard: nothing here
//! leaves the process, and `App` drops it on a project switch (a slug
//! names a request in the project being left). It holds exactly one row,
//! vim's unnamed register, because that is all `p` ever reads.
//!
//! The Variable Manager's options grid keeps its own stash
//! (`VarManager::stash`) rather than joining this one: an option row is
//! scoped to its selector, and the grid's yank and put are its copy and
//! paste (spec §5.4).

use crate::components::editor::EditorTab;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Register {
    #[default]
    Empty,
    /// A sidebar request, by slug. The put re-reads the file, so a yank
    /// never holds a stale copy of the request's bytes, and a source
    /// deleted between the `yy` and the `p` fails loudly instead.
    Request(String),
    /// A Params/Headers/Vars row. The tab is part of the register so a
    /// header can't be put into the params table: the three tables look
    /// alike (`String` → `Entry`) but mean different things on the wire.
    Row { tab: EditorTab, key: String, entry: postui_core::model::Entry },
}

impl Register {
    /// The request at `from` now lives at `to` (a rename, a move to
    /// another space, their undo or redo): a register naming it follows,
    /// as the session's cache does, so a put copies the request as it is
    /// now rather than failing on the old slug.
    pub fn follow(&mut self, from: &str, to: &str) {
        if let Self::Request(slug) = self
            && slug == from
        {
            *slug = to.to_string();
        }
    }

    /// Every request of the space `from` now lives in the space `to` (a
    /// space rename).
    pub fn follow_space(&mut self, from: &str, to: &str) {
        if let Self::Request(slug) = self
            && let Some(rest) = slug.strip_prefix(&format!("{from}/"))
        {
            *slug = format!("{to}/{rest}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_register_is_empty() {
        assert_eq!(Register::default(), Register::Empty);
    }

    /// The same key on two tabs is two different rows: `p` compares the
    /// whole register, tab included.
    #[test]
    fn a_row_carries_the_tab_it_came_from() {
        let entry = postui_core::model::Entry { value: "1".into(), enabled: true };
        let header =
            Register::Row { tab: EditorTab::Headers, key: "X".into(), entry: entry.clone() };
        let param = Register::Row { tab: EditorTab::Params, key: "X".into(), entry };
        assert_ne!(header, param);
    }
}

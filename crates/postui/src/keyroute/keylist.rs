//! The key list's rows (piece 4 spec §8.1): the same IDs as §6–§9 of
//! `docs/superpowers/specs/2026-09-27-vim-target-keys.md`, each with its
//! tier, its status and the milestone that ships it, so the matrix can
//! prove every shipped row has a case (plan M1 Deviation 12). A test keeps
//! this list and the doc in step.
//!
//! Test-only: the matrix (`app/key_matrix_tests.rs`) is its one reader.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    One,
    Two,
    Out,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// Ships: needs a vim case, and an arrows one where the verb exists
    /// in arrows (`Row::arrows`).
    Ship,
    /// Deliberately unsupported: needs a negative case, its keys then `j`.
    Out,
    /// A text key, compared against Vim by the engine's conformance
    /// harness rather than here (the spec's `Parked`, Deviation 26).
    Engine,
}

#[derive(Debug, Clone, Copy)]
pub struct Row {
    pub id: &'static str,
    pub tier: Tier,
    pub status: Status,
    /// The piece 4 milestone that ships it (3 for an engine row).
    pub milestone: u8,
    /// The verb also exists in the arrows profile.
    pub arrows: bool,
}

/// The milestone this build ships through: the coverage test checks the
/// rows up to it.
pub const MILESTONE: u8 = 3;

const fn row(id: &'static str, tier: Tier, status: Status, milestone: u8, arrows: bool) -> Row {
    Row { id, tier, status, milestone, arrows }
}

use Status::{Engine, Out as OutRow, Ship};
use Tier::{One, Out, Two};

pub static KEY_LIST: &[Row] = &[
    row("L.move", One, Ship, 1, true),
    row("L.hl", One, Ship, 1, true),
    row("L.open", One, Ship, 1, true),
    row("L.verbs", One, Ship, 2, true),
    row("L.dd", One, Ship, 2, true),
    row("L.Ndd", Two, Ship, 2, false),
    row("L.yank", One, Ship, 2, true),
    row("L.undo", One, Ship, 1, true),
    row("L.search", Out, OutRow, 1, false),
    row("L.fold", Two, Ship, 5, false),
    row("L.dot", Two, Ship, 5, false),
    row("L.hml", Out, OutRow, 1, false),
    row("R.move", One, Ship, 1, true),
    row("R.ctrl-d", One, Ship, 4, true),
    row("R.page", Two, Ship, 5, false),
    row("R.fold", One, Ship, 4, false),
    row("R.foldall", Two, Ship, 5, false),
    row("R.search", One, Ship, 1, true),
    row("R.star", Two, Ship, 5, false),
    row("R.yy", Two, Ship, 5, false),
    row("R.visual", Two, Ship, 5, false),
    row("P.ctrlw", One, Ship, 4, true),
    row("P.ctrlw-arrows", Two, Ship, 5, false),
    row("P.gt", One, Ship, 4, true),
    row("X.wq", One, Ship, 4, false),
    row("X.e", Two, Ship, 5, false),
    row("X.verbs", One, Ship, 4, false),
    row("X.edit", One, Ship, 4, false),
    row("X.history", Two, Ship, 5, false),
    row("X.unsupported", Out, OutRow, 4, false),
    row("X.noh", Two, Ship, 4, false),
    row("D.prompt", One, Ship, 3, true),
    row("D.picker", One, Ship, 3, true),
    row("D.confirm", One, Ship, 3, true),
    row("G.ZZ", One, Ship, 4, false),
    row("G.qa", Out, OutRow, 4, false),
    row("G.marks", Out, OutRow, 4, false),
    row("G.reg0", Two, Engine, 3, false),
    row("G.regs", Out, OutRow, 4, false),
    row("G.jumps", Out, OutRow, 4, false),
    row("G.unsupported", One, Ship, 1, false),
    row("G.echo", One, Ship, 1, false),
    row("G.mode", One, Ship, 1, false),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_key_list_doc_and_this_list_agree() {
        let doc = include_str!("../../../../docs/superpowers/specs/2026-09-27-vim-target-keys.md");
        let start = doc.find("## 6.").expect("the doc has §6");
        let end = doc[start..].find("\n---").map_or(doc.len(), |i| start + i);
        let ids: Vec<&str> = doc[start..end]
            .lines()
            .filter_map(|l| l.strip_prefix("| `"))
            .filter_map(|l| l.split('`').next())
            .collect();
        let listed: Vec<&str> = KEY_LIST.iter().map(|r| r.id).collect();
        assert_eq!(ids, listed, "§6–§9's IDs, in order");
    }

    #[test]
    fn out_rows_and_out_tiers_agree() {
        for r in KEY_LIST {
            assert_eq!(r.tier == Tier::Out, r.status == Status::Out, "{}", r.id);
        }
    }
}

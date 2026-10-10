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

/// The piece 4 milestone that ships a row (plan M4a Deviation 13): M4 is
/// split into M4a (panes, tabs, the response) and M4b (the command line
/// and the globals). M5 is split into M5a (the response viewer, `ctrl+w` +
/// arrows) and M5b (`.`, sidebar folds, command-line history) (plan M5a
/// Deviation 1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Milestone {
    M1,
    M2,
    M3,
    M4a,
    M4b,
    M5a,
    M5b,
}

#[derive(Debug, Clone, Copy)]
pub struct Row {
    pub id: &'static str,
    pub tier: Tier,
    pub status: Status,
    /// The piece 4 milestone that ships it (`M3` for an engine row).
    pub milestone: Milestone,
    /// The verb also exists in the arrows profile.
    pub arrows: bool,
}

/// The milestone this build ships through: the coverage test checks the
/// rows up to it.
pub const MILESTONE: Milestone = Milestone::M5a;

const fn row(id: &'static str, tier: Tier, status: Status, milestone: Milestone, arrows: bool) -> Row {
    Row { id, tier, status, milestone, arrows }
}

use Milestone::*;
use Status::{Engine, Out as OutRow, Ship};
use Tier::{One, Out, Two};

pub static KEY_LIST: &[Row] = &[
    row("L.move", One, Ship, M1, true),
    row("L.hl", One, Ship, M1, true),
    row("L.open", One, Ship, M1, true),
    row("L.verbs", One, Ship, M2, true),
    row("L.dd", One, Ship, M2, true),
    row("L.Ndd", Two, Ship, M2, false),
    row("L.yank", One, Ship, M2, true),
    row("L.undo", One, Ship, M1, true),
    row("L.search", Out, OutRow, M1, false),
    row("L.fold", Two, Ship, M5b, false),
    row("L.dot", Two, Ship, M5b, false),
    row("L.hml", Out, OutRow, M1, false),
    row("R.move", One, Ship, M1, true),
    row("R.ctrl-d", One, Ship, M4a, true),
    row("R.page", Two, Ship, M5a, true),
    row("R.fold", One, Ship, M4a, false),
    row("R.foldall", Two, Ship, M5a, false),
    row("R.search", One, Ship, M1, true),
    row("R.star", Two, Ship, M5a, false),
    row("R.yy", Two, Ship, M5a, false),
    row("R.visual", Two, Ship, M5a, false),
    row("P.ctrlw", One, Ship, M4a, true),
    row("P.ctrlw-arrows", Two, Ship, M5a, false),
    row("P.gt", One, Ship, M4a, true),
    row("X.wq", One, Ship, M4b, false),
    row("X.e", Two, Ship, M4b, false),
    row("X.verbs", One, Ship, M4b, false),
    row("X.edit", One, Ship, M4b, false),
    row("X.history", Two, Ship, M5b, false),
    row("X.unsupported", Out, OutRow, M4b, false),
    row("X.noh", Two, Ship, M4b, false),
    row("D.prompt", One, Ship, M3, true),
    row("D.picker", One, Ship, M3, true),
    row("D.confirm", One, Ship, M3, true),
    row("G.ZZ", One, Ship, M4b, false),
    row("G.qa", Out, OutRow, M4b, false),
    row("G.marks", Out, OutRow, M4b, false),
    row("G.reg0", Two, Engine, M3, false),
    row("G.regs", Out, OutRow, M4b, false),
    row("G.jumps", Out, OutRow, M4b, false),
    row("G.unsupported", One, Ship, M1, false),
    row("G.echo", One, Ship, M1, false),
    row("G.mode", One, Ship, M1, false),
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

//! The project's undo journal: entries of inverse operations, replayed by
//! `Project::undo`/`redo` through the same primitives that recorded them.
//! Pure data — no I/O here.

use crate::disk::{RelPath, Ticket};
use std::time::{Duration, Instant};

/// Keyboard reorders within this window merge into one entry (the
/// existing 2 s burst rule of `undo::History`).
pub const MERGE_WINDOW: Duration = Duration::from_secs(2);

/// Everything undo needs to reverse an operation, and nothing more.
/// Request ops carry paths and trash tickets; only the small fixed-count
/// documents carry text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Op {
    /// A file or directory moved. Inverse: move it back.
    Renamed { from: RelPath, to: RelPath },
    /// A file or directory was created. Inverse: trash it (so redo can
    /// restore it whatever it contains by then).
    Created { path: RelPath },
    /// A path went to the trash. Inverse: restore.
    Trashed { ticket: Ticket },
    /// A path came back from the trash. Inverse: retrash.
    Restored { ticket: Ticket },
    /// A document's text went from `before` to `after` (`None` = absent).
    /// Inverse: write `before` (or remove).
    Text {
        path: RelPath,
        before: Option<String>,
        after: Option<String>,
    },
}

impl Op {
    /// Every path this op's inverse will touch.
    pub fn touched(&self) -> Vec<&RelPath> {
        match self {
            Op::Renamed { from, to } => vec![from, to],
            Op::Created { path } => vec![path],
            Op::Trashed { ticket } | Op::Restored { ticket } => {
                vec![&ticket.original, &ticket.slot]
            }
            Op::Text { path, .. } => vec![path],
        }
    }
}

/// What the app must do after an entry is undone or redone.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EntryMeta {
    /// `(old, new)` slug pairs of every request the entry moved (rename,
    /// move to space, move all): how the open request follows.
    pub moves: Vec<(String, String)>,
    /// `(before, after)` active environment, when the entry switched it.
    pub active_env: Option<(Option<String>, Option<String>)>,
    /// The request this entry was made in, for the "jump back" toast.
    pub reopen: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeKey {
    RequestOrder { space: String, slug: String },
    SpaceOrder { name: String },
}

#[derive(Debug, Clone)]
pub struct Entry {
    pub label: String,
    pub ops: Vec<Op>,
    pub meta: EntryMeta,
    /// Set by keyboard reorders: consecutive entries with the same key
    /// within `MERGE_WINDOW` fold into one.
    pub merge: Option<(MergeKey, Instant)>,
}

impl Entry {
    /// Folds `next` into `self` when both are single `Text` ops on the
    /// same path: keep the first `before`, take the last `after`.
    fn try_merge(&mut self, next: &Entry) -> bool {
        let (Some((k1, _)), Some((k2, t2))) = (&self.merge, &next.merge) else {
            return false;
        };
        if k1 != k2 {
            return false;
        }
        let same_len = self.ops.len() == 1 && next.ops.len() == 1;
        let (Some(a), Some(b)) = (self.ops.first_mut(), next.ops.first()) else {
            return false;
        };
        match (a, b) {
            (
                Op::Text { path: p1, after, .. },
                Op::Text {
                    path: p2,
                    after: after2,
                    ..
                },
            ) if same_len && p1 == p2 => {
                *after = after2.clone();
                self.merge = Some((k2.clone(), *t2));
                true
            }
            _ => false,
        }
    }
}

pub const DEFAULT_CAP: usize = 200;

pub struct Journal {
    undo: Vec<Entry>,
    redo: Vec<Entry>,
    cap: usize,
}

impl Default for Journal {
    fn default() -> Self {
        Self::new()
    }
}

impl Journal {
    pub fn new() -> Journal {
        Journal::with_cap(DEFAULT_CAP)
    }

    pub fn with_cap(cap: usize) -> Journal {
        Journal {
            undo: Vec::new(),
            redo: Vec::new(),
            cap,
        }
    }

    /// Records a new entry: clears redo, merges a burst, drops the oldest
    /// past the cap.
    pub fn push(&mut self, entry: Entry) {
        self.redo.clear();
        if let (Some(last), Some((_, t))) = (self.undo.last_mut(), &entry.merge)
            && let Some((_, t_last)) = &last.merge
            && t.saturating_duration_since(*t_last) <= MERGE_WINDOW
            && last.try_merge(&entry)
        {
            return;
        }
        self.undo.push(entry);
        if self.undo.len() > self.cap {
            self.undo.remove(0);
        }
    }

    pub fn pop_undo(&mut self) -> Option<Entry> {
        self.undo.pop()
    }

    pub fn push_redo(&mut self, entry: Entry) {
        self.redo.push(entry);
    }

    pub fn pop_redo(&mut self) -> Option<Entry> {
        self.redo.pop()
    }

    /// A redo's product goes back on the undo stack without clearing redo.
    pub fn push_undo_replayed(&mut self, entry: Entry) {
        self.undo.push(entry);
        if self.undo.len() > self.cap {
            self.undo.remove(0);
        }
    }

    pub fn len(&self) -> usize {
        self.undo.len()
    }

    pub fn is_empty(&self) -> bool {
        self.undo.is_empty()
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::disk::RelPath;

    fn text_entry(label: &str, path: &str, before: &str, after: &str) -> Entry {
        Entry {
            label: label.to_string(),
            ops: vec![Op::Text {
                path: RelPath::new(path).unwrap(),
                before: Some(before.to_string()),
                after: Some(after.to_string()),
            }],
            meta: EntryMeta::default(),
            merge: None,
        }
    }

    #[test]
    fn push_clears_redo_and_pops_come_back_newest_first() {
        let mut j = Journal::new();
        j.push(text_entry("a", "project.toml", "", "1"));
        j.push(text_entry("b", "project.toml", "1", "2"));
        let b = j.pop_undo().unwrap();
        assert_eq!(b.label, "b");
        j.push_redo(b);
        assert!(j.can_redo());
        j.push(text_entry("c", "project.toml", "1", "3"));
        assert!(!j.can_redo(), "a new entry clears redo");
        assert_eq!(j.len(), 2);
    }

    #[test]
    fn the_cap_drops_the_oldest() {
        let mut j = Journal::with_cap(2);
        j.push(text_entry("a", "x", "", "1"));
        j.push(text_entry("b", "x", "1", "2"));
        j.push(text_entry("c", "x", "2", "3"));
        assert_eq!(j.len(), 2);
        assert_eq!(j.pop_undo().unwrap().label, "c");
        assert_eq!(j.pop_undo().unwrap().label, "b");
        assert!(j.pop_undo().is_none());
    }

    #[test]
    fn push_undo_replayed_also_respects_the_cap() {
        let mut j = Journal::with_cap(2);
        j.push(text_entry("a", "x", "", "1"));
        j.push(text_entry("b", "x", "1", "2"));
        let b = j.pop_undo().unwrap();
        j.push_undo_replayed(b);
        j.push(text_entry("c", "x", "2", "3"));
        assert_eq!(j.len(), 2);
        assert_eq!(j.pop_undo().unwrap().label, "c");
        assert_eq!(j.pop_undo().unwrap().label, "b");
        assert!(j.pop_undo().is_none());
    }

    #[test]
    fn a_burst_with_the_same_merge_key_folds_into_one_entry() {
        let mut j = Journal::new();
        let key = || MergeKey::SpaceOrder {
            name: "auth".to_string(),
        };
        let mut e1 = text_entry("move", "project.toml", "[a,b]", "[b,a]");
        e1.merge = Some((key(), std::time::Instant::now()));
        let mut e2 = text_entry("move", "project.toml", "[b,a]", "[b,a,c]");
        e2.merge = Some((key(), std::time::Instant::now()));
        j.push(e1);
        j.push(e2);
        assert_eq!(j.len(), 1);
        let e = j.pop_undo().unwrap();
        match &e.ops[..] {
            [Op::Text { before, after, .. }] => {
                assert_eq!(before.as_deref(), Some("[a,b]"), "first before");
                assert_eq!(after.as_deref(), Some("[b,a,c]"), "last after");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_different_key_or_an_old_burst_does_not_merge() {
        let mut j = Journal::new();
        let mut e1 = text_entry("move", "project.toml", "1", "2");
        e1.merge = Some((
            MergeKey::SpaceOrder {
                name: "a".to_string(),
            },
            std::time::Instant::now() - MERGE_WINDOW * 2,
        ));
        let mut e2 = text_entry("move", "project.toml", "2", "3");
        e2.merge = Some((
            MergeKey::SpaceOrder {
                name: "a".to_string(),
            },
            std::time::Instant::now(),
        ));
        j.push(e1);
        j.push(e2);
        assert_eq!(j.len(), 2, "outside the window");
        let mut e3 = text_entry("move", "project.toml", "3", "4");
        e3.merge = Some((
            MergeKey::SpaceOrder {
                name: "b".to_string(),
            },
            std::time::Instant::now(),
        ));
        j.push(e3);
        assert_eq!(j.len(), 3, "different key");
    }

    #[test]
    fn op_touched_names_every_path_an_op_affects() {
        let a = RelPath::new("requests/main/a.toml").unwrap();
        let b = RelPath::new("requests/auth/a.toml").unwrap();
        let op = Op::Renamed {
            from: a.clone(),
            to: b.clone(),
        };
        assert_eq!(op.touched(), vec![&a, &b]);
    }
}

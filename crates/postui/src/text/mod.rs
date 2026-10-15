//! Text shapes both keyboard profiles share: Vim's character classes, its
//! bracket matching and the mouse's words. Profile-neutral, so the arrows
//! profile reads them without naming the vim engine, and the engine reads
//! them from here (spec 2026-10-14, align the text mouse).

pub mod brackets;
pub mod class;
mod class_table;
pub mod mouse;

use std::borrow::Cow;

/// A position in chars, never bytes (vim engine spec §3.2). A one-line
/// buffer always has `row == 0`. Ordered by row, then column.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Pos {
    pub row: usize,
    pub col: usize,
}

impl Pos {
    pub const fn new(row: usize, col: usize) -> Self {
        Self { row, col }
    }
}

/// Text by lines: all the shapes here read.
pub trait Lines {
    /// At least 1: an empty text is one empty line.
    fn line_count(&self) -> usize;
    fn line(&self, row: usize) -> Cow<'_, [char]>;
}

/// A one-line field's text.
impl Lines for [char] {
    fn line_count(&self) -> usize {
        1
    }

    fn line(&self, _row: usize) -> Cow<'_, [char]> {
        Cow::Borrowed(self)
    }
}

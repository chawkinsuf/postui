//! Text shapes both keyboard profiles share: Vim's character classes, its
//! bracket matching, the mouse's words and a body's indentation.
//! Profile-neutral, so the arrows profile reads them without naming the vim
//! engine, and the engine reads them from here (spec 2026-10-14, align the
//! text mouse).

pub mod brackets;
pub mod class;
mod class_table;
pub mod indent;
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

/// The chars of the `{{token}}` that text put at char `at` of `line` would
/// land inside: one that starts before `at` and ends after it. A put just
/// before a token's first brace or just after its last lands outside it
/// (#149). The question is where the put lands, not which token the caret
/// is at: the caret tooltip (`Editor::caret_token`) also names the token a
/// caret just before it sits at, but a put there goes in front and nests
/// nothing. So vim Normal on a first brace replaces the token (its put
/// lands after the brace) except at column 0, where the put goes before.
pub fn token_around(line: &str, at: usize) -> Option<std::ops::Range<usize>> {
    let at = line.char_indices().nth(at).map_or(line.len(), |(byte, _)| byte);
    let token =
        postui_core::vars::find_tokens(line).into_iter().find(|t| t.start < at && at < t.end)?;
    let start = line[..token.start].chars().count();
    Some(start..start + line[token.start..token.end].chars().count())
}

/// Text by lines: all the shapes here read.
pub trait Lines {
    /// At least 1: an empty text is one empty line.
    fn line_count(&self) -> usize;
    fn line(&self, row: usize) -> Cow<'_, [char]>;
}

/// The request body's text: edtui's lines, read by the arrows body and by
/// the vim engine's `BodyBuf` alike.
impl Lines for edtui::Lines {
    fn line_count(&self) -> usize {
        self.len().max(1)
    }

    fn line(&self, row: usize) -> Cow<'_, [char]> {
        Cow::Borrowed(self.get(edtui::RowIndex::new(row)).map(Vec::as_slice).unwrap_or(&[]))
    }
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

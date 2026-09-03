//! A flattened, collapsible view of a JSON document.
//!
//! `JsonTree::parse` walks a `serde_json::Value` once and produces one
//! entry per pretty-printed line. Collapsing never rebuilds the lines: a
//! container line knows its closing line, so hiding its subtree is a range
//! skip. What a collapse does rebuild is the `visible` index — the
//! full-line index of every line currently on show — so that a frame, a
//! cursor move or a mouse drag on a million-line tree is `O(rows)`, not
//! `O(lines)` per row.
//!
//! The tree is built to be small: a multi-megabyte body flattens to
//! hundreds of thousands of lines, and the session keeps a tree per cached
//! response. Every line's rendered text (its content after the indent —
//! `"key": "value",`) lives back to back in one arena `String`, and a line
//! is a 24-byte record of where its text ends, where its key token ends,
//! which container it sits in and which (if any) it opens. Tokens, the
//! `{…} N keys` summary, the jq path and the ancestor chain are all
//! derived on demand from that — a per-row cost at draw time, in place of
//! dozens of heap allocations per line at parse time.
//!
//! The module is deliberately theme-agnostic: tokens carry a semantic
//! [`TokenKind`] and the caller maps that to its own palette.

use postui_core::jq::{PathSeg, render_path};
use std::borrow::Cow;

/// Semantic class of a rendered token, so the caller can color it with its
/// own theme tokens without this module knowing about themes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    /// An object key, including its quotes.
    Key,
    /// A string value, including its quotes.
    Str,
    /// A numeric value.
    Number,
    /// `null`, `true`, `false`, and the child-count in a collapsed summary.
    Literal,
    /// Braces, brackets, colons, commas, indentation.
    Punct,
}

/// A run of a line's text with one semantic class. Borrows the tree's
/// arena where it can; only a collapsed summary's child count is built.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token<'a> {
    pub text: Cow<'a, str>,
    pub kind: TokenKind,
}

impl<'a> Token<'a> {
    fn new(text: impl Into<Cow<'a, str>>, kind: TokenKind) -> Self {
        Self {
            text: text.into(),
            kind,
        }
    }
}

/// The container a line opens (objects and arrays with at least one child).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Container {
    /// Dense id, the index into the tree's container table.
    pub id: usize,
    pub children: usize,
    pub is_array: bool,
    /// Index of the line holding this container's closing bracket.
    pub end_line: usize,
}

/// What a line is, structurally.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
enum Kind {
    /// `"k": 1,` — a scalar value (with or without a key).
    Scalar,
    /// `"k": {` — opens a non-empty container.
    Open,
    /// `},` — closes one.
    Close,
    /// `"k": [],` — an empty container: one line, nothing to collapse.
    Empty,
}

const NONE: u32 = u32::MAX;

/// One line's record. `#[repr(C)]` keeps the layout at 24 bytes — five
/// `u32`s, a `u16` and two `u8`s — which is what makes a tree of a million
/// lines a few tens of megabytes rather than a gigabyte.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C)]
struct LineRec {
    /// Byte end of this line's text in the arena; it starts where the
    /// previous line's ends.
    end: u32,
    /// Bytes of the quoted key token at the start of the text, 0 for a
    /// keyless line (array element, root, closing bracket).
    key_len: u32,
    /// Id of the container this line sits in (`NONE` at the root). A
    /// closing line sits in the container it closes.
    parent: u32,
    /// This line's ordinal among its parent's children — its array index
    /// when the parent is an array. `NONE` for the root and closing lines.
    index: u32,
    /// Id of the container this line opens, `NONE` otherwise.
    container: u32,
    indent: u16,
    kind: Kind,
    /// Bit 0: a comma follows — in the text for every kind but `Open`,
    /// whose comma is drawn by its closing line when expanded and by its
    /// summary when collapsed. Bit 1: collapsed (only ever set on `Open`).
    flags: u8,
}

const COMMA: u8 = 1;
const COLLAPSED: u8 = 2;

impl LineRec {
    fn comma(&self) -> bool {
        self.flags & COMMA != 0
    }

    fn collapsed(&self) -> bool {
        self.flags & COLLAPSED != 0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ContainerRec {
    open_line: u32,
    end_line: u32,
    children: u32,
    is_array: bool,
}

/// A line, resolved against its tree: the borrowed view `JsonTree::line`
/// hands out. Cheap to build (a few field reads); the text-shaped
/// accessors derive their answers from the arena slice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeLine<'a> {
    pub indent: usize,
    /// `Some` when this line opens a non-empty object or array.
    pub container: Option<Container>,
    /// Only ever true on a line with a `container`.
    pub collapsed: bool,
    /// The line's text after its indent — `"key": "value",` in full.
    text: &'a str,
    key_len: usize,
    kind: Kind,
    /// A sibling follows. In the text unless this line opens a container.
    comma: bool,
}

impl<'a> TreeLine<'a> {
    /// The JSON text of a scalar line's value (`"a"`, `1`, `true`, `null`).
    /// `None` for container-opening, empty-container and closing lines.
    pub fn scalar_text(&self) -> Option<&'a str> {
        (self.kind == Kind::Scalar).then(|| self.value_text())
    }

    /// Whether the text ends with the sibling comma (an opening line's
    /// comma is not in its text — see `LineRec::flags`).
    fn comma_in_text(&self) -> bool {
        self.comma && self.kind != Kind::Open
    }

    /// The text after the key (and `: `) and before any trailing comma.
    fn value_text(&self) -> &'a str {
        let start = if self.key_len > 0 {
            self.key_len + 2
        } else {
            0
        };
        let end = self.text.len() - usize::from(self.comma_in_text());
        &self.text[start..end]
    }

    /// The fully-expanded tokens of this line, ignoring collapse state.
    pub fn tokens(&self) -> Vec<Token<'a>> {
        let mut out = Vec::with_capacity(4);
        self.push_prefix(&mut out);
        let value = self.value_text();
        let kind = match self.kind {
            Kind::Scalar => match value.as_bytes().first() {
                Some(b'"') => TokenKind::Str,
                Some(b'n' | b't' | b'f') => TokenKind::Literal,
                _ => TokenKind::Number,
            },
            Kind::Open | Kind::Close | Kind::Empty => TokenKind::Punct,
        };
        out.push(Token::new(value, kind));
        if self.comma_in_text() {
            out.push(Token::new(",", TokenKind::Punct));
        }
        out
    }

    /// The tokens to draw right now — the collapsed summary when this line
    /// opens a collapsed container, the full rendering otherwise.
    pub fn render_tokens(&self) -> Vec<Token<'a>> {
        let Some(c) = self.container.as_ref().filter(|_| self.collapsed) else {
            return self.tokens();
        };
        let mut out = Vec::with_capacity(5);
        self.push_prefix(&mut out);
        let (open, close) = if c.is_array { ("[", "]") } else { ("{", "}") };
        out.push(Token::new(format!("{open}…{close}"), TokenKind::Punct));
        out.push(Token::new(
            format!(" {}", child_count(c.children, c.is_array)),
            TokenKind::Literal,
        ));
        if self.comma {
            out.push(Token::new(",", TokenKind::Punct));
        }
        out
    }

    fn push_prefix(&self, out: &mut Vec<Token<'a>>) {
        if self.key_len > 0 {
            out.push(Token::new(&self.text[..self.key_len], TokenKind::Key));
            out.push(Token::new(": ", TokenKind::Punct));
        }
    }

    /// The text to draw right now, indentation included. Mirrors
    /// [`TreeLine::render_tokens`], so char offsets into this string line up
    /// with the rendered spans.
    pub fn plain_text(&self) -> String {
        let mut s = " ".repeat(self.indent);
        for t in self.render_tokens() {
            s.push_str(&t.text);
        }
        s
    }

    /// The fully-expanded text of this line, ignoring collapse state.
    pub fn expanded_text(&self) -> String {
        let mut s = String::with_capacity(self.indent + self.text.len());
        s.extend(std::iter::repeat_n(' ', self.indent));
        s.push_str(self.text);
        s
    }

    /// Display width of what [`TreeLine::plain_text`] would draw, without
    /// building it — what the widest-line measure over a whole tree runs.
    pub fn render_width(&self) -> usize {
        use unicode_width::UnicodeWidthStr;
        if self.collapsed && self.container.is_some() {
            return self.plain_text().width();
        }
        self.indent + self.text.width()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsonTree {
    /// Every line's text after its indent, back to back.
    text: String,
    lines: Vec<LineRec>,
    containers: Vec<ContainerRec>,
    /// Full-line indices of everything currently visible, in order.
    /// Rebuilt by whatever changes collapse state (`toggle`,
    /// `expand_ancestors`) — `O(lines)` there, so every read is `O(1)`.
    visible: Vec<u32>,
}

impl JsonTree {
    fn empty() -> Self {
        JsonTree {
            text: String::new(),
            lines: Vec::new(),
            containers: Vec::new(),
            visible: Vec::new(),
        }
    }

    /// Parses `text` as JSON and flattens it. Returns `None` when the text
    /// is not JSON at all. Synchronous and potentially slow on a large
    /// document: past `response::SYNC_PRETTY_BYTES` its caller runs it on a
    /// blocking worker rather than on the UI thread.
    pub fn parse(text: &str) -> Option<JsonTree> {
        let value: serde_json::Value = serde_json::from_str(text).ok()?;
        let mut tree = Self::empty();
        tree.walk(None, &value, 0, NONE, NONE, false);
        tree.rebuild_visible();
        Some(tree)
    }

    /// Several documents in one tree — a filter's outputs — one straight
    /// after another, exactly as `jq` prints a stream. Paths restart at `.`
    /// for every document. An empty list is an empty tree.
    pub fn parse_many(docs: &[String]) -> Option<JsonTree> {
        let mut tree = Self::empty();
        for doc in docs {
            let value: serde_json::Value = serde_json::from_str(doc).ok()?;
            tree.walk(None, &value, 0, NONE, NONE, false);
        }
        tree.rebuild_visible();
        Some(tree)
    }

    fn line_text(&self, full_index: usize) -> &str {
        let start = if full_index == 0 {
            0
        } else {
            self.lines[full_index - 1].end as usize
        };
        &self.text[start..self.lines[full_index].end as usize]
    }

    fn container_of(&self, rec: &LineRec) -> Option<Container> {
        (rec.container != NONE).then(|| {
            let c = &self.containers[rec.container as usize];
            Container {
                id: rec.container as usize,
                children: c.children as usize,
                is_array: c.is_array,
                end_line: c.end_line as usize,
            }
        })
    }

    pub fn line(&self, full_index: usize) -> TreeLine<'_> {
        let rec = &self.lines[full_index];
        TreeLine {
            indent: usize::from(rec.indent),
            container: self.container_of(rec),
            collapsed: rec.collapsed(),
            text: self.line_text(full_index),
            key_len: rec.key_len as usize,
            kind: rec.kind,
            comma: rec.comma(),
        }
    }

    pub fn full_index_of_visible(&self, visible_index: usize) -> Option<usize> {
        self.visible.get(visible_index).map(|&i| i as usize)
    }

    pub fn visible_line(&self, visible_index: usize) -> Option<TreeLine<'_>> {
        self.full_index_of_visible(visible_index)
            .map(|i| self.line(i))
    }

    /// The path segment `full_index` contributes: its key, or its index
    /// inside an array parent. `None` for the root and closing lines.
    fn own_seg(&self, full_index: usize) -> Option<PathSeg> {
        let rec = &self.lines[full_index];
        if rec.kind == Kind::Close || rec.parent == NONE {
            return None;
        }
        if rec.key_len > 0 {
            let quoted = &self.line_text(full_index)[..rec.key_len as usize];
            return Some(PathSeg::Key(unescape(quoted)));
        }
        self.containers[rec.parent as usize]
            .is_array
            .then_some(PathSeg::Index(rec.index as usize))
    }

    /// The container ids `full_index` lives inside, outermost first. A
    /// closing line lives inside the container it closes.
    fn parent_ids(&self, full_index: usize) -> Vec<usize> {
        let mut ids = Vec::new();
        let mut parent = self.lines[full_index].parent;
        while parent != NONE {
            ids.push(parent as usize);
            parent = self.lines[self.containers[parent as usize].open_line as usize].parent;
        }
        ids.reverse();
        ids
    }

    /// This line's jq path from the document root, derived by walking its
    /// ancestors — a closing line's path is its container's.
    fn path_of(&self, full_index: usize) -> Vec<PathSeg> {
        let mut segs = Vec::new();
        let rec = &self.lines[full_index];
        let mut at = if rec.kind == Kind::Close {
            // Its container's opening line carries the segment.
            Some(self.containers[rec.parent as usize].open_line as usize)
        } else {
            Some(full_index)
        };
        while let Some(i) = at {
            if let Some(seg) = self.own_seg(i) {
                segs.push(seg);
            }
            let parent = self.lines[i].parent;
            at = (parent != NONE).then(|| self.containers[parent as usize].open_line as usize);
        }
        segs.reverse();
        segs
    }

    pub fn jq_path_of(&self, full_index: usize) -> String {
        render_path(&self.path_of(full_index))
    }

    /// The keys of an array line's first element when that element is an object.
    pub fn first_element_keys(&self, full_index: usize) -> Vec<String> {
        let rec = &self.lines[full_index];
        if rec.container == NONE || !self.containers[rec.container as usize].is_array {
            return Vec::new();
        }
        let first = full_index + 1;
        let fc = self.lines[first].container;
        if fc == NONE || self.containers[fc as usize].is_array {
            return Vec::new();
        }
        let end = self.containers[fc as usize].end_line as usize;
        // Direct children of the first element: the lines whose parent is
        // that object (its closing line sits in it too — skipped).
        (first + 1..end)
            .filter(|&i| self.lines[i].parent == fc && self.lines[i].kind != Kind::Close)
            .filter_map(|i| match self.own_seg(i) {
                Some(PathSeg::Key(k)) => Some(k),
                _ => None,
            })
            .collect()
    }

    /// For a line inside an array element: the array's opening line index and the
    /// path from the element down to this line (empty when the line *is* the element).
    pub fn nearest_array_ancestor(&self, full_index: usize) -> Option<(usize, Vec<PathSeg>)> {
        let rec = &self.lines[full_index];
        let mut ids = self.parent_ids(full_index);
        // A closing line belongs to its container, which lives one level up.
        if rec.kind == Kind::Close {
            ids.pop();
        }
        let path = self.path_of(full_index);
        for &id in ids.iter().rev() {
            let c = &self.containers[id];
            if c.is_array {
                let open = c.open_line as usize;
                let array_len = self.path_of(open).len();
                // path = array path + [Index] + relative
                if path.len() < array_len + 1 {
                    return None;
                }
                return Some((open, path[array_len + 1..].to_vec()));
            }
        }
        None
    }

    /// Total number of lines, ignoring collapse state. Indices into this
    /// range are what [`JsonTree::expand_ancestors`] and
    /// [`JsonTree::visible_index_of`] speak.
    pub fn line_count(&self) -> usize {
        self.lines.len()
    }

    /// Full-line indices of everything currently visible, in order.
    pub fn visible_indices(&self) -> &[u32] {
        &self.visible
    }

    /// How many lines are visible right now.
    pub fn visible_len(&self) -> usize {
        self.visible.len()
    }

    /// Recomputes `visible` from the collapse flags: a walk over the lines
    /// that skips each collapsed container's subtree as a range.
    fn rebuild_visible(&mut self) {
        self.visible.clear();
        let mut i = 0;
        while i < self.lines.len() {
            self.visible.push(i as u32);
            let rec = &self.lines[i];
            if rec.collapsed() {
                i = self.containers[rec.container as usize].end_line as usize + 1;
            } else {
                i += 1;
            }
        }
    }

    /// The visible lines, in order. A collapsed container's opening line
    /// renders as its `{…} N keys` summary. Allocates a `Vec` the size of
    /// the visible set — for a whole-tree pass, not for a per-row lookup
    /// (that is `visible_line`).
    pub fn visible_lines(&self) -> Vec<TreeLine<'_>> {
        self.visible
            .iter()
            .map(|&i| self.line(i as usize))
            .collect()
    }

    /// Where `full_index` sits among the visible lines, or `None` if it is
    /// currently hidden inside a collapsed container.
    pub fn visible_index_of(&self, full_index: usize) -> Option<usize> {
        // `visible` is sorted (it is a subsequence of the line indices).
        u32::try_from(full_index)
            .ok()
            .and_then(|i| self.visible.binary_search(&i).ok())
    }

    /// Whether the line at `visible_index` opens a container (and so carries
    /// a `▸`/`▾` toggle glyph). `false` for an out-of-range index.
    pub fn is_container_at_visible(&self, visible_index: usize) -> bool {
        self.full_index_of_visible(visible_index)
            .is_some_and(|full| self.lines[full].container != NONE)
    }

    /// Collapses or expands the container opened by the line at
    /// `visible_index`. A no-op on lines that open no container (and on an
    /// out-of-range index).
    pub fn toggle(&mut self, visible_index: usize) {
        let Some(full) = self.full_index_of_visible(visible_index) else {
            return;
        };
        let rec = &mut self.lines[full];
        if rec.container != NONE {
            rec.flags ^= COLLAPSED;
            self.rebuild_visible();
        }
    }

    /// Expands every container that `full_index` lives inside, so that line
    /// becomes visible. Used when jumping to a search match.
    pub fn expand_ancestors(&mut self, full_index: usize) {
        if full_index >= self.lines.len() {
            return;
        }
        let mut changed = false;
        for id in self.parent_ids(full_index) {
            let opener = &mut self.lines[self.containers[id].open_line as usize];
            changed |= opener.collapsed();
            opener.flags &= !COLLAPSED;
        }
        if changed {
            self.rebuild_visible();
        }
    }

    /// Every line's fully-expanded text, ignoring collapse state — the
    /// corpus search runs over, so a match inside a collapsed container is
    /// still findable.
    pub fn full_text_lines(&self) -> Vec<String> {
        (0..self.lines.len())
            .map(|i| self.line(i).expanded_text())
            .collect()
    }

    /// Recursive flattening walk. `key` is the object key this value hangs
    /// off (`None` for the root and for array elements), `parent` the
    /// enclosing container's id, `index` the value's ordinal among its
    /// siblings, `comma` whether a sibling follows it.
    fn walk(
        &mut self,
        key: Option<&str>,
        value: &serde_json::Value,
        indent: u16,
        parent: u32,
        index: u32,
        comma: bool,
    ) {
        use serde_json::Value;

        let (open, close, len, is_array) = match value {
            Value::Object(map) => ("{", "}", map.len(), false),
            Value::Array(items) => ("[", "]", items.len(), true),
            scalar => {
                let key_len = self.push_key(key);
                push_scalar(&mut self.text, scalar);
                self.push_line(Kind::Scalar, key_len, indent, parent, index, NONE, comma);
                return;
            }
        };

        // An empty container is one line and cannot be collapsed — there is
        // nothing to hide, and `{…} 0 keys` is longer than `{}`.
        if len == 0 {
            let key_len = self.push_key(key);
            self.text.push_str(open);
            self.text.push_str(close);
            self.push_line(Kind::Empty, key_len, indent, parent, index, NONE, comma);
            return;
        }

        let id = self.containers.len() as u32;
        let open_line = self.lines.len() as u32;
        self.containers.push(ContainerRec {
            open_line,
            // Patched once the children have been emitted.
            end_line: open_line,
            children: len as u32,
            is_array,
        });
        let key_len = self.push_key(key);
        self.text.push_str(open);
        // The comma rides on the flag only: the closing line's text has it.
        self.push_line(Kind::Open, key_len, indent, parent, index, id, false);
        if comma {
            self.lines[open_line as usize].flags |= COMMA;
        }

        match value {
            Value::Object(map) => {
                for (i, (k, v)) in map.iter().enumerate() {
                    self.walk(Some(k), v, indent + 2, id, i as u32, i + 1 < len);
                }
            }
            Value::Array(items) => {
                for (i, v) in items.iter().enumerate() {
                    self.walk(None, v, indent + 2, id, i as u32, i + 1 < len);
                }
            }
            _ => unreachable!("only containers reach here"),
        }

        let end_line = self.lines.len() as u32;
        self.text.push_str(close);
        self.push_line(Kind::Close, 0, indent, id, NONE, NONE, comma);
        self.containers[id as usize].end_line = end_line;
    }

    /// Appends `"key": ` to the arena; returns the quoted key's byte length
    /// (0 when there is no key).
    fn push_key(&mut self, key: Option<&str>) -> u32 {
        let Some(k) = key else { return 0 };
        let before = self.text.len();
        push_escaped(&mut self.text, k);
        let key_len = (self.text.len() - before) as u32;
        self.text.push_str(": ");
        key_len
    }

    /// Closes the line whose text has just been appended: the trailing
    /// comma, then the record. `#[allow]`: a private helper mirroring
    /// `walk`'s own parameters one-for-one.
    #[allow(clippy::too_many_arguments)]
    fn push_line(
        &mut self,
        kind: Kind,
        key_len: u32,
        indent: u16,
        parent: u32,
        index: u32,
        container: u32,
        comma: bool,
    ) {
        if comma {
            self.text.push(',');
        }
        self.lines.push(LineRec {
            end: self.text.len() as u32,
            key_len,
            parent,
            index,
            container,
            indent,
            kind,
            flags: if comma { COMMA } else { 0 },
        });
    }
}

fn child_count(len: usize, is_array: bool) -> String {
    let noun = match (is_array, len == 1) {
        (true, true) => "item",
        (true, false) => "items",
        (false, true) => "key",
        (false, false) => "keys",
    };
    format!("{len} {noun}")
}

/// Appends the JSON string literal (quotes and escapes included) for `s`.
fn push_escaped(out: &mut String, s: &str) {
    out.push_str(&serde_json::Value::String(s.to_string()).to_string());
}

/// The key a quoted, escaped key token spells — the inverse of
/// `push_escaped`. Falls back to the token itself, quotes and all, should
/// the arena ever hold something that isn't a string literal.
fn unescape(quoted: &str) -> String {
    serde_json::from_str::<String>(quoted).unwrap_or_else(|_| quoted.to_string())
}

fn push_scalar(out: &mut String, value: &serde_json::Value) {
    use serde_json::Value;
    use std::fmt::Write;
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => write!(out, "{b}").expect("String never fails"),
        Value::Number(n) => write!(out, "{n}").expect("String never fails"),
        Value::String(s) => push_escaped(out, s),
        _ => unreachable!("containers are handled by the caller"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use postui_core::jq::render_path;

    #[test]
    fn tree_flattens_and_collapses() {
        let mut t = JsonTree::parse(r#"{"a": {"b": 1, "c": [1, 2]}, "d": null}"#).unwrap();
        let total = t.visible_lines().len();
        t.toggle(1);
        let collapsed = t.visible_lines().len();
        assert!(collapsed < total);
        let line_text = t.visible_lines()[1].plain_text();
        assert!(
            line_text.contains("2 keys"),
            "collapsed summary shows child count: {line_text}"
        );
        t.toggle(1);
        assert_eq!(t.visible_lines().len(), total, "re-expand restores");
    }

    #[test]
    fn nested_collapse_state_survives_an_outer_collapse_and_re_expand() {
        // {"outer": {"inner": {"leaf": 1}, "sibling": 2}}
        let mut t = JsonTree::parse(r#"{"outer": {"inner": {"leaf": 1}, "sibling": 2}}"#).unwrap();
        // visible: 0 "{", 1 "outer": {, 2 "inner": {, 3 "leaf": 1, 4 }, 5 "sibling": 2, 6 }, 7 }
        let inner_line = t
            .visible_lines()
            .iter()
            .position(|l| l.plain_text().contains("\"inner\""))
            .expect("inner line visible before any collapsing");
        t.toggle(inner_line); // collapse inner
        let inner_summary = t.visible_lines()[inner_line].plain_text();
        assert!(
            inner_summary.contains("1 key"),
            "inner collapsed summary: {inner_summary}"
        );

        let outer_line = t
            .visible_lines()
            .iter()
            .position(|l| l.plain_text().contains("\"outer\""))
            .expect("outer line visible");
        t.toggle(outer_line); // collapse outer (inner is now hidden, still collapsed)
        let outer_summary = t.visible_lines()[outer_line].plain_text();
        assert!(
            outer_summary.contains("2 keys"),
            "outer collapsed summary: {outer_summary}"
        );

        t.toggle(outer_line); // re-expand outer
        let inner_summary_again = t.visible_lines()[inner_line].plain_text();
        assert!(
            inner_summary_again.contains("1 key"),
            "inner must still be collapsed after outer re-expands: {inner_summary_again}"
        );
    }

    #[test]
    fn search_lines_cover_collapsed_content() {
        let mut t = JsonTree::parse(r#"{"outer": {"needle": "x"}}"#).unwrap();
        t.toggle(1); // collapse outer
        let text = t.full_text_lines().join("\n");
        assert!(
            text.contains("needle"),
            "search text ignores collapse state"
        );
    }

    #[test]
    fn parse_rejects_non_json() {
        assert!(JsonTree::parse("<html><body>hi</body></html>").is_none());
        assert!(JsonTree::parse("").is_none());
    }

    #[test]
    fn arrays_summarize_as_items() {
        let mut t = JsonTree::parse(r#"{"xs": [1, 2, 3]}"#).unwrap();
        t.toggle(1);
        let line = t.visible_lines()[1].plain_text();
        assert!(
            line.contains("3 items"),
            "array summary shows item count: {line}"
        );
    }

    #[test]
    fn single_child_containers_are_singular() {
        let mut t = JsonTree::parse(r#"{"xs": [1], "o": {"k": 1}}"#).unwrap();
        t.toggle(1);
        assert!(t.visible_lines()[1].plain_text().contains("1 item"));
        t.toggle(2);
        assert!(t.visible_lines()[2].plain_text().contains("1 key"));
    }

    #[test]
    fn expand_ancestors_makes_a_deep_line_visible() {
        let mut t = JsonTree::parse(r#"{"a": {"b": {"c": "deep"}}}"#).unwrap();
        let deep = t
            .full_text_lines()
            .iter()
            .position(|l| l.contains("deep"))
            .expect("the deep line exists in the search text");
        t.toggle(1); // collapse "a"
        assert!(
            t.visible_index_of(deep).is_none(),
            "the deep line is hidden while collapsed"
        );
        t.expand_ancestors(deep);
        let vis = t
            .visible_index_of(deep)
            .expect("expanding ancestors reveals the line");
        assert!(t.visible_lines()[vis].plain_text().contains("deep"));
    }

    #[test]
    fn plain_text_is_indented_and_covers_every_line() {
        let t = JsonTree::parse(r#"{"a": {"b": 1}}"#).unwrap();
        let lines = t.full_text_lines();
        assert_eq!(t.line_count(), lines.len());
        assert_eq!(lines[0], "{");
        assert_eq!(lines[1], "  \"a\": {");
        assert_eq!(lines[2], "    \"b\": 1");
        assert_eq!(lines[3], "  }");
        assert_eq!(lines[4], "}");
    }

    #[test]
    fn commas_separate_siblings_but_not_the_last_one() {
        let t = JsonTree::parse(r#"{"a": 1, "b": [2, 3], "c": true}"#).unwrap();
        assert_eq!(
            t.full_text_lines(),
            vec![
                "{",
                "  \"a\": 1,",
                "  \"b\": [",
                "    2,",
                "    3",
                "  ],",
                "  \"c\": true",
                "}"
            ]
        );
    }

    #[test]
    fn empty_containers_are_a_single_uncollapsible_line() {
        let t = JsonTree::parse(r#"{"a": {}, "b": []}"#).unwrap();
        assert_eq!(
            t.full_text_lines(),
            vec!["{", "  \"a\": {},", "  \"b\": []", "}"]
        );
        assert!(t.visible_lines()[1].container.is_none());
    }

    #[test]
    fn toggle_on_a_non_container_line_is_a_no_op() {
        let mut t = JsonTree::parse(r#"{"a": 1}"#).unwrap();
        let before = t.visible_lines().len();
        t.toggle(1);
        t.toggle(99);
        assert_eq!(t.visible_lines().len(), before);
    }

    #[test]
    fn token_kinds_classify_keys_strings_numbers_and_literals() {
        let t = JsonTree::parse(r#"{"k": "s", "n": 1, "b": null}"#).unwrap();
        let kinds = |i: usize| {
            t.line(i)
                .tokens()
                .iter()
                .map(|x| x.kind)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            kinds(1),
            vec![
                TokenKind::Key,
                TokenKind::Punct,
                TokenKind::Str,
                TokenKind::Punct
            ]
        );
        assert_eq!(kinds(2)[2], TokenKind::Number);
        assert_eq!(kinds(3)[2], TokenKind::Literal);
    }

    #[test]
    fn keys_and_strings_are_escaped() {
        let t = JsonTree::parse(r#"{"a\"b": "c\nd"}"#).unwrap();
        assert_eq!(t.full_text_lines()[1], r#"  "a\"b": "c\nd""#);
    }

    fn line_with(t: &JsonTree, needle: &str) -> usize {
        (0..t.line_count())
            .find(|&i| t.line(i).expanded_text().contains(needle))
            .unwrap_or_else(|| panic!("no line containing {needle:?}"))
    }

    #[test]
    fn every_line_carries_its_jq_path_and_closing_lines_carry_their_containers() {
        let t =
            JsonTree::parse(r#"{"data": {"items": [{"name": "a"}, 2], "odd key": true}}"#).unwrap();
        assert_eq!(t.jq_path_of(0), ".");
        assert_eq!(t.jq_path_of(line_with(&t, "\"items\"")), ".data.items");
        assert_eq!(
            t.jq_path_of(line_with(&t, "\"name\"")),
            ".data.items[0].name"
        );
        assert_eq!(t.jq_path_of(line_with(&t, "2")), ".data.items[1]");
        assert_eq!(
            t.jq_path_of(line_with(&t, "\"odd key\"")),
            r#".data["odd key"]"#
        );
        let items_open = line_with(&t, "\"items\"");
        let items_close = t.line(items_open).container.as_ref().unwrap().end_line;
        assert_eq!(t.jq_path_of(items_close), ".data.items");
    }

    #[test]
    fn scalar_lines_expose_their_json_value_text() {
        let t = JsonTree::parse(
            r#"{"s": "a\"b", "n": 1.5, "b": true, "z": null, "o": {}, "arr": [1]}"#,
        )
        .unwrap();
        assert_eq!(
            t.line(line_with(&t, "\"s\"")).scalar_text(),
            Some(r#""a\"b""#)
        );
        assert_eq!(t.line(line_with(&t, "\"n\"")).scalar_text(), Some("1.5"));
        assert_eq!(
            t.line(line_with(&t, "\"b\": true")).scalar_text(),
            Some("true")
        );
        assert_eq!(t.line(line_with(&t, "\"z\"")).scalar_text(), Some("null"));
        assert_eq!(
            t.line(line_with(&t, "\"o\"")).scalar_text(),
            None,
            "empty containers are not scalars"
        );
        assert_eq!(t.line(line_with(&t, "\"arr\"")).scalar_text(), None);
        assert_eq!(
            t.line(t.line_count() - 1).scalar_text(),
            None,
            "closing brace"
        );
    }

    #[test]
    fn first_element_keys_come_from_an_array_whose_first_element_is_an_object() {
        let t = JsonTree::parse(
            r#"{"items": [{"id": 1, "name": "a"}, {"id": 2}], "nums": [1, 2], "empty": []}"#,
        )
        .unwrap();
        assert_eq!(
            t.first_element_keys(line_with(&t, "\"items\"")),
            vec!["id", "name"]
        );
        assert!(t.first_element_keys(line_with(&t, "\"nums\"")).is_empty());
        assert!(t.first_element_keys(line_with(&t, "\"empty\"")).is_empty());
        assert!(
            t.first_element_keys(line_with(&t, "\"id\": 1")).is_empty(),
            "not an array"
        );
    }

    #[test]
    fn nearest_array_ancestor_gives_the_array_line_and_the_path_below_its_element() {
        let t = JsonTree::parse(r#"{"items": [{"meta": {"status": "on"}}], "top": 1}"#).unwrap();
        let items = line_with(&t, "\"items\"");
        let status = line_with(&t, "\"status\"");
        let (array_line, rel) = t
            .nearest_array_ancestor(status)
            .expect("status sits inside items[0]");
        assert_eq!(array_line, items);
        assert_eq!(render_path(&rel), ".meta.status");
        let element = line_with(&t, "\"meta\"") - 1; // the `{` opening items[0]
        assert_eq!(
            t.nearest_array_ancestor(element),
            Some((items, vec![])),
            "the element itself has an empty relative path"
        );
        assert_eq!(t.nearest_array_ancestor(line_with(&t, "\"top\"")), None);
    }

    #[test]
    fn parse_many_runs_documents_together_like_jq_and_restarts_paths() {
        let t = JsonTree::parse_many(&["{\"a\": 1}".to_string(), "[2]".to_string()]).unwrap();
        assert_eq!(
            t.full_text_lines(),
            vec!["{", "  \"a\": 1", "}", "[", "  2", "]"]
        );
        assert_eq!(t.jq_path_of(3), ".");
        assert_eq!(t.jq_path_of(4), ".[0]");
        assert!(JsonTree::parse_many(&["{".to_string()]).is_none());
        assert!(
            JsonTree::parse_many(&[]).is_some(),
            "no outputs is an empty tree, not a failure"
        );
    }

    #[test]
    fn a_line_record_stays_24_bytes() {
        // The whole point of the arena layout: a million-line tree is a
        // few tens of megabytes. Growing the record is a deliberate call.
        assert_eq!(std::mem::size_of::<LineRec>(), 24);
    }

    #[test]
    fn the_visible_index_tracks_every_collapse_change() {
        // {"a": {"b": {"c": 1}}, "d": 2}
        let mut t = JsonTree::parse(r#"{"a": {"b": {"c": 1}}, "d": 2}"#).unwrap();
        let all: Vec<u32> = (0..t.line_count() as u32).collect();
        assert_eq!(
            t.visible_indices(),
            &all[..],
            "everything visible after parse"
        );
        assert_eq!(t.visible_len(), all.len());
        t.toggle(1); // "a": {
        assert_eq!(t.visible_indices(), &[0, 1, 6, 7], "a's subtree is skipped");
        assert_eq!(t.visible_index_of(2), None, "b is hidden");
        assert_eq!(t.visible_index_of(6), Some(2), "d sits at visible row 2");
        assert!(!t.is_container_at_visible(2), "d is a scalar");
        t.toggle(0); // collapse the root too
        assert_eq!(t.visible_indices(), &[0]);
        t.expand_ancestors(3); // "c": 1, inside b inside a inside root
        assert_eq!(t.visible_indices(), &all[..], "every ancestor reopened");
        t.toggle(5); // a scalar-less closing line: no-op, index unchanged
        assert_eq!(t.visible_indices(), &all[..]);
        t.toggle(99);
        assert_eq!(t.visible_indices(), &all[..], "out of range is a no-op");
    }

    #[test]
    fn visible_line_maps_through_collapse_state() {
        let mut t = JsonTree::parse(r#"{"a": {"b": 1}, "c": 2}"#).unwrap();
        let a = line_with(&t, "\"a\"");
        t.toggle(a);
        // visible: 0 "{", 1 "a": {…}, 2 "c": 2, 3 "}"
        assert_eq!(t.full_index_of_visible(2), Some(line_with(&t, "\"c\"")));
        assert_eq!(
            t.visible_line(2).map(|l| l.expanded_text()),
            Some("  \"c\": 2".to_string())
        );
        assert_eq!(t.visible_line(9), None);
    }
}

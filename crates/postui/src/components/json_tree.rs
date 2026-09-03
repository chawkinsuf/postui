//! A flattened, collapsible view of a JSON document.
//!
//! `JsonTree::parse` walks a `serde_json::Value` once and produces a flat
//! `Vec<TreeLine>` — one entry per pretty-printed line. Collapsing never
//! rebuilds anything: a container line carries the index of its closing line,
//! so hiding its subtree is a range skip, and both the expanded and the
//! collapsed (`{…} 2 keys`) renderings of a line are precomputed at build
//! time.
//!
//! The module is deliberately theme-agnostic: tokens carry a semantic
//! [`TokenKind`] and the caller maps that to its own palette.

use postui_core::jq::{PathSeg, render_path};

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    pub text: String,
    pub kind: TokenKind,
}

impl Token {
    fn new(text: impl Into<String>, kind: TokenKind) -> Self {
        Self {
            text: text.into(),
            kind,
        }
    }
}

/// The container a line opens (objects and arrays with at least one child).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Container {
    /// Dense id, also the index into `JsonTree::container_lines`.
    pub id: usize,
    pub children: usize,
    pub is_array: bool,
    /// Index of the line holding this container's closing bracket.
    pub end_line: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeLine {
    pub indent: usize,
    /// The fully-expanded rendering of this line.
    pub tokens: Vec<Token>,
    /// The `{…} N keys` rendering, used when `collapsed`. Empty for lines
    /// that do not open a container.
    collapsed_tokens: Vec<Token>,
    /// `Some` when this line opens a non-empty object or array.
    pub container: Option<Container>,
    /// Ids of every container this line lives inside, outermost first.
    pub parent_ids: Vec<usize>,
    /// Only ever true on a line with a `container`.
    pub collapsed: bool,
    /// This line's jq path from the document root.
    pub path: Vec<PathSeg>,
    /// The JSON text of a scalar line's value (`"a"`, `1`, `true`, `null`).
    /// `None` for container-opening, closing and separator lines.
    scalar: Option<String>,
}

impl TreeLine {
    pub fn scalar_text(&self) -> Option<&str> {
        self.scalar.as_deref()
    }

    /// The blank line `parse_many` puts between documents.
    pub fn is_separator(&self) -> bool {
        self.tokens.is_empty()
    }

    /// The tokens to draw right now — the collapsed summary when this line
    /// opens a collapsed container, the full rendering otherwise.
    pub fn render_tokens(&self) -> &[Token] {
        if self.collapsed {
            &self.collapsed_tokens
        } else {
            &self.tokens
        }
    }

    /// The text to draw right now, indentation included. Mirrors
    /// [`TreeLine::render_tokens`], so char offsets into this string line up
    /// with the rendered spans.
    pub fn plain_text(&self) -> String {
        Self::join(self.indent, self.render_tokens())
    }

    /// The fully-expanded text of this line, ignoring collapse state.
    pub fn expanded_text(&self) -> String {
        Self::join(self.indent, &self.tokens)
    }

    fn join(indent: usize, tokens: &[Token]) -> String {
        let mut s = " ".repeat(indent);
        for t in tokens {
            s.push_str(&t.text);
        }
        s
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsonTree {
    lines: Vec<TreeLine>,
    /// Container id -> index of the line that opens it.
    container_lines: Vec<usize>,
}

impl JsonTree {
    /// Parses `text` as JSON and flattens it. Returns `None` when the text
    /// is not JSON at all. Synchronous and potentially slow on a large
    /// document: past `response::SYNC_PRETTY_BYTES` its caller runs it on a
    /// blocking worker rather than on the UI thread.
    pub fn parse(text: &str) -> Option<JsonTree> {
        let value: serde_json::Value = serde_json::from_str(text).ok()?;
        let mut tree = JsonTree {
            lines: Vec::new(),
            container_lines: Vec::new(),
        };
        tree.walk(None, &value, 0, &[], &[], false);
        Some(tree)
    }

    /// Several documents in one tree — a filter's outputs — separated by a
    /// blank line each, exactly as `jq` prints them. Paths restart at `.`
    /// for every document. An empty list is an empty tree.
    pub fn parse_many(docs: &[String]) -> Option<JsonTree> {
        let mut tree = JsonTree {
            lines: Vec::new(),
            container_lines: Vec::new(),
        };
        for (i, doc) in docs.iter().enumerate() {
            let value: serde_json::Value = serde_json::from_str(doc).ok()?;
            if i > 0 {
                tree.push(0, Vec::new(), Vec::new(), None, &[], &[], None);
            }
            tree.walk(None, &value, 0, &[], &[], false);
        }
        Some(tree)
    }

    pub fn line(&self, full_index: usize) -> &TreeLine {
        &self.lines[full_index]
    }

    pub fn full_index_of_visible(&self, visible_index: usize) -> Option<usize> {
        self.visible_indices().get(visible_index).copied()
    }

    pub fn visible_line(&self, visible_index: usize) -> Option<&TreeLine> {
        self.full_index_of_visible(visible_index).map(|i| &self.lines[i])
    }

    pub fn jq_path_of(&self, full_index: usize) -> String {
        render_path(&self.lines[full_index].path)
    }

    /// The keys of an array line's first element when that element is an object.
    pub fn first_element_keys(&self, full_index: usize) -> Vec<String> {
        let Some(c) = &self.lines[full_index].container else {
            return Vec::new();
        };
        if !c.is_array {
            return Vec::new();
        }
        let first = full_index + 1;
        let Some(fc) = &self.lines[first].container else {
            return Vec::new();
        };
        if fc.is_array {
            return Vec::new();
        }
        // Direct children of the first element: lines nested exactly one
        // container deeper whose path ends in a key.
        let depth = self.lines[first].parent_ids.len() + 1;
        (first + 1..fc.end_line)
            .filter(|&i| self.lines[i].parent_ids.len() == depth)
            .filter_map(|i| match self.lines[i].path.last() {
                Some(PathSeg::Key(k)) => Some(k.clone()),
                _ => None,
            })
            .collect()
    }

    /// For a line inside an array element: the array's opening line index and the
    /// path from the element down to this line (empty when the line *is* the element).
    pub fn nearest_array_ancestor(&self, full_index: usize) -> Option<(usize, Vec<PathSeg>)> {
        let line = &self.lines[full_index];
        // A closing line belongs to its container, which lives one level up.
        let path = &line.path;
        let mut ids = line.parent_ids.clone();
        if line.container.is_none() && line.tokens.first().is_some_and(|t| t.text == "}" || t.text == "]") {
            ids.pop();
        }
        for &id in ids.iter().rev() {
            let open = self.container_lines[id];
            if self.lines[open].container.as_ref().is_some_and(|c| c.is_array) {
                let array_len = self.lines[open].path.len();
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
    pub fn visible_indices(&self) -> Vec<usize> {
        let mut out = Vec::new();
        let mut i = 0;
        while i < self.lines.len() {
            out.push(i);
            let line = &self.lines[i];
            match (&line.container, line.collapsed) {
                (Some(c), true) => i = c.end_line + 1,
                _ => i += 1,
            }
        }
        out
    }

    /// The visible lines, in order. A collapsed container's opening line
    /// renders as its `{…} N keys` summary.
    pub fn visible_lines(&self) -> Vec<&TreeLine> {
        self.visible_indices()
            .into_iter()
            .map(|i| &self.lines[i])
            .collect()
    }

    /// Where `full_index` sits among the visible lines, or `None` if it is
    /// currently hidden inside a collapsed container.
    pub fn visible_index_of(&self, full_index: usize) -> Option<usize> {
        self.visible_indices().iter().position(|&i| i == full_index)
    }

    /// Whether the line at `visible_index` opens a container (and so carries
    /// a `▸`/`▾` toggle glyph). `false` for an out-of-range index.
    pub fn is_container_at_visible(&self, visible_index: usize) -> bool {
        self.visible_indices()
            .get(visible_index)
            .is_some_and(|&full| self.lines[full].container.is_some())
    }

    /// Collapses or expands the container opened by the line at
    /// `visible_index`. A no-op on lines that open no container (and on an
    /// out-of-range index).
    pub fn toggle(&mut self, visible_index: usize) {
        let Some(&full) = self.visible_indices().get(visible_index) else {
            return;
        };
        let line = &mut self.lines[full];
        if line.container.is_some() {
            line.collapsed = !line.collapsed;
        }
    }

    /// Expands every container that `full_index` lives inside, so that line
    /// becomes visible. Used when jumping to a search match.
    pub fn expand_ancestors(&mut self, full_index: usize) {
        let Some(line) = self.lines.get(full_index) else {
            return;
        };
        let parents = line.parent_ids.clone();
        for id in parents {
            let opener = self.container_lines[id];
            self.lines[opener].collapsed = false;
        }
    }

    /// Every line's fully-expanded text, ignoring collapse state — the
    /// corpus search runs over, so a match inside a collapsed container is
    /// still findable.
    pub fn full_text_lines(&self) -> Vec<String> {
        self.lines.iter().map(|l| l.expanded_text()).collect()
    }

    /// Recursive flattening walk. `key` is the object key this value hangs
    /// off (`None` for the root and for array elements), `comma` whether a
    /// sibling follows it.
    fn walk(
        &mut self,
        key: Option<&str>,
        value: &serde_json::Value,
        indent: usize,
        parents: &[usize],
        path: &[PathSeg],
        comma: bool,
    ) {
        use serde_json::Value;
        let prefix = || match key {
            Some(k) => vec![
                Token::new(escape(k), TokenKind::Key),
                Token::new(": ", TokenKind::Punct),
            ],
            None => Vec::new(),
        };

        let (open, close, len, is_array) = match value {
            Value::Object(map) => ("{", "}", map.len(), false),
            Value::Array(items) => ("[", "]", items.len(), true),
            scalar => {
                let mut tokens = prefix();
                let scalar = scalar_token(scalar);
                let text = scalar.text.clone();
                tokens.push(scalar);
                if comma {
                    tokens.push(Token::new(",", TokenKind::Punct));
                }
                self.push(indent, tokens, Vec::new(), None, parents, path, Some(text));
                return;
            }
        };

        // An empty container is one line and cannot be collapsed — there is
        // nothing to hide, and `{…} 0 keys` is longer than `{}`.
        if len == 0 {
            let mut tokens = prefix();
            tokens.push(Token::new(format!("{open}{close}"), TokenKind::Punct));
            if comma {
                tokens.push(Token::new(",", TokenKind::Punct));
            }
            self.push(indent, tokens, Vec::new(), None, parents, path, None);
            return;
        }

        let id = self.container_lines.len();
        let mut open_tokens = prefix();
        open_tokens.push(Token::new(open, TokenKind::Punct));

        let mut summary = prefix();
        summary.push(Token::new(format!("{open}…{close}"), TokenKind::Punct));
        summary.push(Token::new(
            format!(" {}", child_count(len, is_array)),
            TokenKind::Literal,
        ));
        if comma {
            summary.push(Token::new(",", TokenKind::Punct));
        }

        let open_line = self.lines.len();
        self.container_lines.push(open_line);
        // `end_line` is patched once the children have been emitted.
        let container = Container {
            id,
            children: len,
            is_array,
            end_line: open_line,
        };
        self.push(indent, open_tokens, summary, Some(container), parents, path, None);

        let mut inner_parents = parents.to_vec();
        inner_parents.push(id);
        match value {
            Value::Object(map) => {
                for (i, (k, v)) in map.iter().enumerate() {
                    self.walk(
                        Some(k),
                        v,
                        indent + 2,
                        &inner_parents,
                        &child_path(path, PathSeg::Key(k.clone())),
                        i + 1 < len,
                    );
                }
            }
            Value::Array(items) => {
                for (i, v) in items.iter().enumerate() {
                    self.walk(
                        None,
                        v,
                        indent + 2,
                        &inner_parents,
                        &child_path(path, PathSeg::Index(i)),
                        i + 1 < len,
                    );
                }
            }
            _ => unreachable!("only containers reach here"),
        }

        let mut close_tokens = vec![Token::new(close, TokenKind::Punct)];
        if comma {
            close_tokens.push(Token::new(",", TokenKind::Punct));
        }
        let end_line = self.lines.len();
        self.push(indent, close_tokens, Vec::new(), None, &inner_parents, path, None);
        if let Some(c) = &mut self.lines[open_line].container {
            c.end_line = end_line;
        }
    }

    // Private helper mirroring `walk`'s own parameters one-for-one; splitting
    // them into a struct would just move the same fields without reducing
    // anything a caller has to supply.
    #[allow(clippy::too_many_arguments)]
    fn push(
        &mut self,
        indent: usize,
        tokens: Vec<Token>,
        collapsed_tokens: Vec<Token>,
        container: Option<Container>,
        parents: &[usize],
        path: &[PathSeg],
        scalar: Option<String>,
    ) {
        self.lines.push(TreeLine {
            indent,
            tokens,
            collapsed_tokens,
            container,
            parent_ids: parents.to_vec(),
            collapsed: false,
            path: path.to_vec(),
            scalar,
        });
    }
}

fn child_path(path: &[PathSeg], seg: PathSeg) -> Vec<PathSeg> {
    let mut p = path.to_vec();
    p.push(seg);
    p
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

/// A JSON string literal (quotes and escapes included) for `s`.
fn escape(s: &str) -> String {
    serde_json::Value::String(s.to_string()).to_string()
}

fn scalar_token(value: &serde_json::Value) -> Token {
    use serde_json::Value;
    match value {
        Value::Null => Token::new("null", TokenKind::Literal),
        Value::Bool(b) => Token::new(b.to_string(), TokenKind::Literal),
        Value::Number(n) => Token::new(n.to_string(), TokenKind::Number),
        Value::String(s) => Token::new(escape(s), TokenKind::Str),
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
        let kinds = |i: usize| t.lines[i].tokens.iter().map(|x| x.kind).collect::<Vec<_>>();
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
        let t = JsonTree::parse(r#"{"data": {"items": [{"name": "a"}, 2], "odd key": true}}"#).unwrap();
        assert_eq!(t.jq_path_of(0), ".");
        assert_eq!(t.jq_path_of(line_with(&t, "\"items\"")), ".data.items");
        assert_eq!(t.jq_path_of(line_with(&t, "\"name\"")), ".data.items[0].name");
        assert_eq!(t.jq_path_of(line_with(&t, "2")), ".data.items[1]");
        assert_eq!(t.jq_path_of(line_with(&t, "\"odd key\"")), r#".data["odd key"]"#);
        let items_open = line_with(&t, "\"items\"");
        let items_close = t.line(items_open).container.as_ref().unwrap().end_line;
        assert_eq!(t.jq_path_of(items_close), ".data.items");
    }

    #[test]
    fn scalar_lines_expose_their_json_value_text() {
        let t = JsonTree::parse(r#"{"s": "a\"b", "n": 1.5, "b": true, "z": null, "o": {}, "arr": [1]}"#).unwrap();
        assert_eq!(t.line(line_with(&t, "\"s\"")).scalar_text(), Some(r#""a\"b""#));
        assert_eq!(t.line(line_with(&t, "\"n\"")).scalar_text(), Some("1.5"));
        assert_eq!(t.line(line_with(&t, "\"b\": true")).scalar_text(), Some("true"));
        assert_eq!(t.line(line_with(&t, "\"z\"")).scalar_text(), Some("null"));
        assert_eq!(t.line(line_with(&t, "\"o\"")).scalar_text(), None, "empty containers are not scalars");
        assert_eq!(t.line(line_with(&t, "\"arr\"")).scalar_text(), None);
        assert_eq!(t.line(t.line_count() - 1).scalar_text(), None, "closing brace");
    }

    #[test]
    fn first_element_keys_come_from_an_array_whose_first_element_is_an_object() {
        let t = JsonTree::parse(r#"{"items": [{"id": 1, "name": "a"}, {"id": 2}], "nums": [1, 2], "empty": []}"#).unwrap();
        assert_eq!(t.first_element_keys(line_with(&t, "\"items\"")), vec!["id", "name"]);
        assert!(t.first_element_keys(line_with(&t, "\"nums\"")).is_empty());
        assert!(t.first_element_keys(line_with(&t, "\"empty\"")).is_empty());
        assert!(t.first_element_keys(line_with(&t, "\"id\": 1")).is_empty(), "not an array");
    }

    #[test]
    fn nearest_array_ancestor_gives_the_array_line_and_the_path_below_its_element() {
        let t = JsonTree::parse(r#"{"items": [{"meta": {"status": "on"}}], "top": 1}"#).unwrap();
        let items = line_with(&t, "\"items\"");
        let status = line_with(&t, "\"status\"");
        let (array_line, rel) = t.nearest_array_ancestor(status).expect("status sits inside items[0]");
        assert_eq!(array_line, items);
        assert_eq!(render_path(&rel), ".meta.status");
        let element = line_with(&t, "\"meta\"") - 1; // the `{` opening items[0]
        assert_eq!(t.nearest_array_ancestor(element), Some((items, vec![])), "the element itself has an empty relative path");
        assert_eq!(t.nearest_array_ancestor(line_with(&t, "\"top\"")), None);
    }

    #[test]
    fn parse_many_separates_documents_with_a_blank_line_and_restarts_paths() {
        let t = JsonTree::parse_many(&["{\"a\": 1}".to_string(), "[2]".to_string()]).unwrap();
        assert_eq!(t.full_text_lines(), vec!["{", "  \"a\": 1", "}", "", "[", "  2", "]"]);
        assert!(t.line(3).is_separator());
        assert_eq!(t.jq_path_of(4), ".");
        assert_eq!(t.jq_path_of(5), ".[0]");
        assert!(JsonTree::parse_many(&["{".to_string()]).is_none());
        assert!(JsonTree::parse_many(&[]).is_some(), "no outputs is an empty tree, not a failure");
    }

    #[test]
    fn visible_line_maps_through_collapse_state() {
        let mut t = JsonTree::parse(r#"{"a": {"b": 1}, "c": 2}"#).unwrap();
        let a = line_with(&t, "\"a\"");
        t.toggle(a);
        // visible: 0 "{", 1 "a": {…}, 2 "c": 2, 3 "}"
        assert_eq!(t.full_index_of_visible(2), Some(line_with(&t, "\"c\"")));
        assert_eq!(t.visible_line(2).map(|l| l.expanded_text()), Some("  \"c\": 2".to_string()));
        assert_eq!(t.visible_line(9), None);
    }
}

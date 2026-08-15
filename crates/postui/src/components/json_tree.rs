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
        Self { text: text.into(), kind }
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

#[derive(Debug, Clone)]
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
}

impl TreeLine {
    /// The tokens to draw right now — the collapsed summary when this line
    /// opens a collapsed container, the full rendering otherwise.
    pub fn render_tokens(&self) -> &[Token] {
        if self.collapsed { &self.collapsed_tokens } else { &self.tokens }
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

pub struct JsonTree {
    lines: Vec<TreeLine>,
    /// Container id -> index of the line that opens it.
    container_lines: Vec<usize>,
}

impl JsonTree {
    /// Parses `text` as JSON and flattens it. Returns `None` when the text
    /// is not JSON at all. Callers apply their own size guard *before*
    /// calling this — parsing a huge body is exactly what the guard avoids.
    pub fn parse(text: &str) -> Option<JsonTree> {
        let value: serde_json::Value = serde_json::from_str(text).ok()?;
        let mut tree = JsonTree { lines: Vec::new(), container_lines: Vec::new() };
        tree.walk(None, &value, 0, &[], false);
        Some(tree)
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
        self.visible_indices().into_iter().map(|i| &self.lines[i]).collect()
    }

    /// Where `full_index` sits among the visible lines, or `None` if it is
    /// currently hidden inside a collapsed container.
    pub fn visible_index_of(&self, full_index: usize) -> Option<usize> {
        self.visible_indices().iter().position(|&i| i == full_index)
    }

    /// Collapses or expands the container opened by the line at
    /// `visible_index`. A no-op on lines that open no container (and on an
    /// out-of-range index).
    pub fn toggle(&mut self, visible_index: usize) {
        let Some(&full) = self.visible_indices().get(visible_index) else { return };
        let line = &mut self.lines[full];
        if line.container.is_some() {
            line.collapsed = !line.collapsed;
        }
    }

    /// Expands every container that `full_index` lives inside, so that line
    /// becomes visible. Used when jumping to a search match.
    pub fn expand_ancestors(&mut self, full_index: usize) {
        let Some(line) = self.lines.get(full_index) else { return };
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
                tokens.push(scalar_token(scalar));
                if comma {
                    tokens.push(Token::new(",", TokenKind::Punct));
                }
                self.push(indent, tokens, Vec::new(), None, parents);
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
            self.push(indent, tokens, Vec::new(), None, parents);
            return;
        }

        let id = self.container_lines.len();
        let mut open_tokens = prefix();
        open_tokens.push(Token::new(open, TokenKind::Punct));

        let mut summary = prefix();
        summary.push(Token::new(format!("{open}…{close}"), TokenKind::Punct));
        summary.push(Token::new(format!(" {}", child_count(len, is_array)), TokenKind::Literal));
        if comma {
            summary.push(Token::new(",", TokenKind::Punct));
        }

        let open_line = self.lines.len();
        self.container_lines.push(open_line);
        // `end_line` is patched once the children have been emitted.
        let container = Container { id, children: len, is_array, end_line: open_line };
        self.push(indent, open_tokens, summary, Some(container), parents);

        let mut inner_parents = parents.to_vec();
        inner_parents.push(id);
        match value {
            Value::Object(map) => {
                for (i, (k, v)) in map.iter().enumerate() {
                    self.walk(Some(k), v, indent + 2, &inner_parents, i + 1 < len);
                }
            }
            Value::Array(items) => {
                for (i, v) in items.iter().enumerate() {
                    self.walk(None, v, indent + 2, &inner_parents, i + 1 < len);
                }
            }
            _ => unreachable!("only containers reach here"),
        }

        let mut close_tokens = vec![Token::new(close, TokenKind::Punct)];
        if comma {
            close_tokens.push(Token::new(",", TokenKind::Punct));
        }
        let end_line = self.lines.len();
        self.push(indent, close_tokens, Vec::new(), None, &inner_parents);
        if let Some(c) = &mut self.lines[open_line].container {
            c.end_line = end_line;
        }
    }

    fn push(
        &mut self,
        indent: usize,
        tokens: Vec<Token>,
        collapsed_tokens: Vec<Token>,
        container: Option<Container>,
        parents: &[usize],
    ) {
        self.lines.push(TreeLine {
            indent,
            tokens,
            collapsed_tokens,
            container,
            parent_ids: parents.to_vec(),
            collapsed: false,
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

    #[test]
    fn tree_flattens_and_collapses() {
        let mut t = JsonTree::parse(r#"{"a": {"b": 1, "c": [1, 2]}, "d": null}"#).unwrap();
        let total = t.visible_lines().len();
        t.toggle(1);
        let collapsed = t.visible_lines().len();
        assert!(collapsed < total);
        let line_text = t.visible_lines()[1].plain_text();
        assert!(line_text.contains("2 keys"), "collapsed summary shows child count: {line_text}");
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
        assert!(inner_summary.contains("1 key"), "inner collapsed summary: {inner_summary}");

        let outer_line = t
            .visible_lines()
            .iter()
            .position(|l| l.plain_text().contains("\"outer\""))
            .expect("outer line visible");
        t.toggle(outer_line); // collapse outer (inner is now hidden, still collapsed)
        let outer_summary = t.visible_lines()[outer_line].plain_text();
        assert!(outer_summary.contains("2 keys"), "outer collapsed summary: {outer_summary}");

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
        assert!(text.contains("needle"), "search text ignores collapse state");
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
        assert!(line.contains("3 items"), "array summary shows item count: {line}");
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
        assert!(t.visible_index_of(deep).is_none(), "the deep line is hidden while collapsed");
        t.expand_ancestors(deep);
        let vis = t.visible_index_of(deep).expect("expanding ancestors reveals the line");
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
            vec!["{", "  \"a\": 1,", "  \"b\": [", "    2,", "    3", "  ],", "  \"c\": true", "}"]
        );
    }

    #[test]
    fn empty_containers_are_a_single_uncollapsible_line() {
        let t = JsonTree::parse(r#"{"a": {}, "b": []}"#).unwrap();
        assert_eq!(t.full_text_lines(), vec!["{", "  \"a\": {},", "  \"b\": []", "}"]);
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
        assert_eq!(kinds(1), vec![TokenKind::Key, TokenKind::Punct, TokenKind::Str, TokenKind::Punct]);
        assert_eq!(kinds(2)[2], TokenKind::Number);
        assert_eq!(kinds(3)[2], TokenKind::Literal);
    }

    #[test]
    fn keys_and_strings_are_escaped() {
        let t = JsonTree::parse(r#"{"a\"b": "c\nd"}"#).unwrap();
        assert_eq!(t.full_text_lines()[1], r#"  "a\"b": "c\nd""#);
    }
}

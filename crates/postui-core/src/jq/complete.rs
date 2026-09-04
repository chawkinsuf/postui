//! Completion for the response pane's jq bar: what the caret is typing
//! (`context`), which keys the body offers there (`keys_at`), jaq's own
//! builtin names (`builtins`), and the ghost text to draw for each
//! (`candidates`). Pure apart from `keys_at`; jaq's types never leave the
//! module.

use super::{Data, JqDocument, with_compiled};
use jaq_core::{Ctx, Vars};
use jaq_json::Val;
use std::collections::BTreeMap;
use std::sync::OnceLock;

/// How many outputs of the context expression are looked at for keys.
/// `.users[]` over a huge array stops here; a completion run can never
/// reach `OUTPUT_CAP`.
pub const COMPLETE_OUTPUTS: usize = 64;

/// Runs `input_expr` against `doc`, takes at most `COMPLETE_OUTPUTS`
/// outputs, and returns the keys of those that are objects in order of
/// first appearance, deduplicated. Any error — a prefix that does not
/// compile, a runtime error part-way — yields what was collected so far,
/// never an error: a half-typed filter is normal here.
pub fn keys_at(input_expr: &str, doc: &JqDocument) -> Vec<String> {
    let mut keys: Vec<String> = Vec::new();
    let _ = with_compiled(input_expr, |filter| {
        let ctx = Ctx::<Data>::new(&filter.lut, Vars::new([]));
        for item in filter
            .id
            .run((ctx, (*doc.0).clone()))
            .take(COMPLETE_OUTPUTS)
        {
            let Ok(val) = item else {
                break;
            };
            if let Val::Obj(map) = &val {
                for (k, _) in map.iter() {
                    if let Val::TStr(s) = k {
                        let key = String::from_utf8_lossy(s).into_owned();
                        if !keys.contains(&key) {
                            keys.push(key);
                        }
                    }
                }
            }
        }
        Ok(())
    });
    keys
}

/// What is being completed at the end of the bar text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    /// `.` followed by an identifier prefix (`quoted: false`), or `."`
    /// followed by any text — a quoted key being typed (`quoted: true`).
    Key { quoted: bool },
    /// A bare word not preceded by `.`, `$` or `@`: a builtin name.
    Word,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Context {
    pub kind: Kind,
    /// The characters already typed of the token being completed
    /// (`us` for `.us`, `sel` for `sel`, `my k` for `."my k`).
    pub partial: String,
    /// Byte offset in the text where the token starts — the `.` for a
    /// key, the first letter for a word.
    pub token_start: usize,
    /// For `Kind::Key`: the jq expression whose outputs the caret's `.`
    /// refers to. `None` for `Kind::Word`.
    pub input_expr: Option<String>,
}

fn is_ident_byte(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

fn is_ident_start(c: u8) -> bool {
    c.is_ascii_alphabetic() || c == b'_'
}

/// Byte index where the identifier run ending at the end of `s` starts
/// (`s.len()` when `s` does not end in an identifier character).
fn ident_run_start(s: &str) -> usize {
    let b = s.as_bytes();
    let mut i = b.len();
    while i > 0 && is_ident_byte(b[i - 1]) {
        i -= 1;
    }
    i
}

/// Byte index of the opening `"` of a string literal left unterminated
/// at the end of `s`, if any.
fn unterminated_string(s: &str) -> Option<usize> {
    let b = s.as_bytes();
    let mut open = None;
    let mut i = 0;
    while i < b.len() {
        match open {
            Some(_) => {
                if b[i] == b'\\' {
                    i += 2;
                    continue;
                }
                if b[i] == b'"' {
                    open = None;
                }
            }
            None => {
                if b[i] == b'"' {
                    open = Some(i);
                }
            }
        }
        i += 1;
    }
    open
}

/// Inspects the whole bar text (the caller guarantees the caret is at
/// its end) and says what is being completed, or `None` when the text
/// ends in something completion has nothing to offer for (whitespace, an
/// operator, `..`, a number, `$x`, `@fmt`, an ordinary string literal).
pub fn context(text: &str) -> Option<Context> {
    let b = text.as_bytes();
    if let Some(open) = unterminated_string(text) {
        // Only a string right after `.` is a key being typed.
        if open == 0 || b[open - 1] != b'.' {
            return None;
        }
        let partial = &text[open + 1..];
        if partial.contains('\\') {
            return None;
        }
        let token_start = open - 1;
        return Some(Context {
            kind: Kind::Key { quoted: true },
            partial: partial.to_string(),
            token_start,
            input_expr: Some(input_expr(&text[..token_start])),
        });
    }
    let start = ident_run_start(text);
    let word = &text[start..];
    let before = &text[..start];
    if before.ends_with('.') {
        let dot = start - 1;
        let prev = if dot > 0 { Some(b[dot - 1]) } else { None };
        // `..` (recurse), `1.5` (a number), `.5` (a number).
        if matches!(prev, Some(b'.')) || matches!(prev, Some(c) if c.is_ascii_digit()) {
            return None;
        }
        if word.bytes().next().is_some_and(|c| c.is_ascii_digit()) {
            return None;
        }
        return Some(Context {
            kind: Kind::Key { quoted: false },
            partial: word.to_string(),
            token_start: dot,
            input_expr: Some(input_expr(&text[..dot])),
        });
    }
    if word.is_empty() || !is_ident_start(word.as_bytes()[0]) {
        return None;
    }
    if matches!(before.as_bytes().last(), Some(b'$') | Some(b'@')) {
        return None;
    }
    Some(Context {
        kind: Kind::Word,
        partial: word.to_string(),
        token_start: start,
        input_expr: None,
    })
}

/// Builtins whose argument runs once per element of the input, so inside
/// `map(` the `.` is an element, not the array.
const PER_ELEMENT: [&str; 9] = [
    "map",
    "map_values",
    "sort_by",
    "group_by",
    "unique_by",
    "min_by",
    "max_by",
    "any",
    "all",
];

/// One forward pass over `s`: which bytes sit inside a string literal,
/// the bracket depth before each byte, where each closer (`)`, `]`, `}`
/// or a closing `"`) opened, and the openers still unclosed at the end.
/// Bytes are enough: every structural character is ASCII and UTF-8
/// continuation bytes never collide with them.
struct Scan {
    in_string: Vec<bool>,
    depth: Vec<usize>,
    open_of: Vec<Option<usize>>,
    unclosed: Vec<usize>,
}

fn scan(s: &str) -> Scan {
    let b = s.as_bytes();
    let n = b.len();
    let mut sc = Scan {
        in_string: vec![false; n],
        depth: vec![0; n],
        open_of: vec![None; n],
        unclosed: Vec::new(),
    };
    let mut string_open: Option<usize> = None;
    let mut i = 0;
    while i < n {
        sc.depth[i] = sc.unclosed.len();
        match string_open {
            Some(open) => {
                sc.in_string[i] = true;
                if b[i] == b'\\' {
                    if i + 1 < n {
                        sc.in_string[i + 1] = true;
                        sc.depth[i + 1] = sc.unclosed.len();
                    }
                    i += 2;
                    continue;
                }
                if b[i] == b'"' {
                    sc.open_of[i] = Some(open);
                    string_open = None;
                }
            }
            None => match b[i] {
                b'"' => {
                    string_open = Some(i);
                    sc.in_string[i] = true;
                }
                b'(' | b'[' | b'{' => sc.unclosed.push(i),
                b')' | b']' | b'}' => {
                    let want = match b[i] {
                        b')' => b'(',
                        b']' => b'[',
                        _ => b'{',
                    };
                    if sc.unclosed.last().is_some_and(|&o| b[o] == want) {
                        sc.open_of[i] = sc.unclosed.pop();
                    }
                }
                _ => {}
            },
        }
        i += 1;
    }
    sc
}

/// The jq expression whose outputs the `.` at the end of `head` refers
/// to: the outer expression (through any unclosed brackets), the stage
/// before the last top-level pipe, and the path chain right before the
/// caret, joined with ` | `; `.` when all are empty.
fn input_expr(head: &str) -> String {
    let parts = resolve(head);
    if parts.is_empty() {
        ".".into()
    } else {
        parts.join(" | ")
    }
}

/// The pieces of the input expression, outermost first.
fn resolve(head: &str) -> Vec<String> {
    let sc = scan(head);
    let b = head.as_bytes();
    let mut parts = Vec::new();
    let segment = match sc.unclosed.last().copied() {
        Some(pos) if b[pos] == b'(' => {
            let before = &head[..pos];
            let name_start = ident_run_start(before);
            let name = &before[name_start..];
            parts = resolve(&before[..name_start]);
            if PER_ELEMENT.contains(&name) {
                parts.push(".[]".into());
            }
            &head[pos + 1..]
        }
        Some(pos) => {
            parts = resolve(&head[..pos]);
            &head[pos + 1..]
        }
        None => head,
    };
    // `f(a; b)`: only the last argument is the caret's.
    let segment = match last_top_level(segment, b';') {
        Some(p) => &segment[p + 1..],
        None => segment,
    };
    let (stage, tail) = match last_top_level(segment, b'|') {
        Some(p) => (segment[..p].trim(), &segment[p + 1..]),
        None => ("", segment),
    };
    if !stage.is_empty() {
        parts.push(stage.to_string());
    }
    let chain = path_chain(tail);
    if !chain.is_empty() {
        parts.push(chain.to_string());
    }
    parts
}

/// Byte index of the last `what` in `s` outside strings and brackets. A
/// `|` directly followed by `=` is the update operator, not a pipe.
fn last_top_level(s: &str, what: u8) -> Option<usize> {
    let sc = scan(s);
    let b = s.as_bytes();
    (0..b.len()).rev().find(|&i| {
        b[i] == what
            && !sc.in_string[i]
            && sc.depth[i] == 0
            && !(what == b'|' && b.get(i + 1) == Some(&b'='))
    })
}

/// The path chain (`.a`, `."k"`, `[…]`, `?`, `$x`, a bare `.`) ending at
/// the end of `tail`, or `""` when it ends in anything else.
fn path_chain(tail: &str) -> &str {
    let t = tail.trim_end();
    let sc = scan(t);
    let b = t.as_bytes();
    let mut i = t.len();
    while i > 0 {
        let c = b[i - 1];
        if sc.in_string[i - 1] && c != b'"' {
            break;
        }
        let next = if c == b'?' {
            Some(i - 1)
        } else if c == b']' || c == b')' {
            // `.a[0]`, `.[0]`, `(.a | .b)` — a bracket group, with its
            // leading `.` when it has one; a function-call group like
            // `select(.a == 1)` extends to the start of its name.
            sc.open_of[i - 1].map(|o| {
                if o > 0 && b[o - 1] == b'.' {
                    o - 1
                } else if b[o] == b'(' && o > 0 && is_ident_byte(b[o - 1]) {
                    ident_run_start(&t[..o])
                } else {
                    o
                }
            })
        } else if c == b'"' {
            // `."my key"` — only a string right after `.` is a path step.
            sc.open_of[i - 1]
                .filter(|&o| o > 0 && b[o - 1] == b'.')
                .map(|o| o - 1)
        } else if is_ident_byte(c) {
            let j = ident_run_start(&t[..i]);
            match (j > 0).then(|| b[j - 1]) {
                Some(b'.') | Some(b'$') => Some(j - 1),
                _ => None,
            }
        } else if c == b'.' {
            Some(i - 1)
        } else {
            None
        };
        match next {
            Some(n) => i = n,
            None => break,
        }
    }
    &t[i..]
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Builtin {
    pub name: &'static str,
    pub arity: usize,
}

const KEYWORDS: [&str; 15] = [
    "if", "then", "elif", "else", "end", "and", "or", "not", "reduce", "foreach", "try", "catch",
    "as", "def", "label",
];

/// Every definition and native function jaq loads, deduplicated by name
/// (lowest arity kept), names starting with `_` dropped, plus jq's
/// keywords. Sorted. Built once.
pub fn builtins() -> &'static [Builtin] {
    static ALL: OnceLock<Vec<Builtin>> = OnceLock::new();
    ALL.get_or_init(|| {
        let mut by_name: BTreeMap<&'static str, usize> = BTreeMap::new();
        let mut note = |name: &'static str, arity: usize| {
            let e = by_name.entry(name).or_insert(arity);
            *e = (*e).min(arity);
        };
        for def in super::defs() {
            note(def.name, def.args.len());
        }
        for (name, binds, _) in super::funs() {
            note(name, binds.len());
        }
        for kw in KEYWORDS {
            note(kw, 0);
        }
        by_name
            .into_iter()
            .filter(|(name, _)| !name.starts_with('_'))
            .map(|(name, arity)| Builtin { name, arity })
            .collect()
    })
}

/// One thing the ghost can show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    /// What is drawn after the caret.
    pub ghost: String,
    /// What accepting inserts. Equal to `ghost` unless the token is
    /// rewritten (`replace_from`).
    pub insert: String,
    /// When `Some`, accepting first deletes from this byte offset of the
    /// bar text to the caret, then inserts `insert`: `.my` becomes
    /// `."my key"`.
    pub replace_from: Option<usize>,
}

fn is_identifier(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty() && is_ident_start(b[0]) && b.iter().all(|&c| is_ident_byte(c))
}

fn quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// The candidates for `ctx`: keys (for `Kind::Key`) or builtins (for
/// `Kind::Word`) that start with the partial and are longer than it —
/// a ghost is always a continuation — in body order for keys and
/// alphabetical for builtins.
pub fn candidates(ctx: &Context, keys: &[String]) -> Vec<Candidate> {
    let p = ctx.partial.as_str();
    let extends = |s: &str| s.starts_with(p) && s.len() > p.len();
    let plain = |rest: String| Candidate {
        ghost: rest.clone(),
        insert: rest,
        replace_from: None,
    };
    match ctx.kind {
        Kind::Word => builtins()
            .iter()
            .filter(|b| extends(b.name))
            .map(|b| {
                let mut rest = b.name[p.len()..].to_string();
                if b.arity > 0 {
                    rest.push('(');
                }
                plain(rest)
            })
            .collect(),
        Kind::Key { quoted: true } => keys
            .iter()
            .filter(|k| extends(k))
            .map(|k| plain(format!("{}\"", &k[p.len()..])))
            .collect(),
        Kind::Key { quoted: false } => keys
            .iter()
            .filter(|k| extends(k))
            .map(|k| {
                if is_identifier(k) {
                    plain(k[p.len()..].to_string())
                } else {
                    let q = quote(k);
                    Candidate {
                        insert: format!(".{q}"),
                        ghost: q,
                        replace_from: Some(ctx.token_start),
                    }
                }
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = r#"{"data":{"items":[{"id":1,"name":"a","status":"active"},{"id":2,"name":"b","status":"off","extra":true}],"total":2,"my key":0}}"#;

    fn doc() -> super::super::JqDocument {
        super::super::JqDocument::parse(DOC).expect("fixture is JSON")
    }

    #[test]
    fn keys_at_collects_object_keys_in_first_appearance_order() {
        assert_eq!(keys_at(".", &doc()), vec!["data"]);
        assert_eq!(keys_at(".data", &doc()), vec!["items", "total", "my key"]);
        assert_eq!(
            keys_at(".data.items[]", &doc()),
            vec!["id", "name", "status", "extra"],
            "keys of every output, deduplicated, in order"
        );
        assert_eq!(
            keys_at(".data.items | .[]", &doc()),
            vec!["id", "name", "status", "extra"]
        );
    }

    #[test]
    fn keys_at_is_empty_for_non_objects_errors_and_bad_filters() {
        assert!(
            keys_at(".data.items", &doc()).is_empty(),
            "an array has no keys"
        );
        assert!(keys_at(".data.total", &doc()).is_empty());
        assert!(
            keys_at(".data.items[] | .id | .x", &doc()).is_empty(),
            "runtime error"
        );
        assert!(keys_at("select(", &doc()).is_empty(), "does not compile");
        assert!(keys_at("$x", &doc()).is_empty(), "unbound variable");
    }

    #[test]
    fn keys_at_stops_at_the_output_cap() {
        // Unbounded outputs — the cap makes this return, not hang.
        let keys = keys_at("range(1e9) | {k: .}", &doc());
        assert_eq!(keys, vec!["k"]);
    }

    fn key(text: &str) -> (String, usize, bool) {
        let c = context(text).unwrap_or_else(|| panic!("{text:?} should complete a key"));
        let Kind::Key { quoted } = c.kind else {
            panic!("{text:?} should be a key, got {:?}", c.kind);
        };
        (c.partial, c.token_start, quoted)
    }

    #[test]
    fn a_dot_and_letters_complete_a_key() {
        assert_eq!(key(".us"), ("us".into(), 0, false));
        assert_eq!(key("."), ("".into(), 0, false));
        assert_eq!(key(".data.it"), ("it".into(), 5, false));
        assert_eq!(key(".data.items[] | .na"), ("na".into(), 16, false));
        assert_eq!(key(".a_b.c_1"), ("c_1".into(), 4, false));
    }

    #[test]
    fn a_quoted_key_being_typed_completes_with_its_text() {
        assert_eq!(key(".data.\"my k"), ("my k".into(), 5, true));
        assert_eq!(key(".\""), ("".into(), 0, true));
        assert!(context(".\"a\\\"b").is_none(), "escapes are not completed");
        assert!(context("\"abc").is_none(), "an ordinary string literal");
        assert!(context(".a | \"x").is_none());
    }

    #[test]
    fn a_bare_word_completes_a_builtin() {
        let c = context(".data.items | leng").unwrap();
        assert_eq!(c.kind, Kind::Word);
        assert_eq!(c.partial, "leng");
        assert_eq!(c.token_start, 14);
        assert!(c.input_expr.is_none());
        let c = context("sel").unwrap();
        assert_eq!(
            (c.kind, c.partial.as_str(), c.token_start),
            (Kind::Word, "sel", 0)
        );
    }

    #[test]
    fn nothing_is_offered_where_a_ghost_would_be_noise() {
        for text in [
            "", " ", ".data | ", ".a ==", "..", "1.", ".5", "1.5", "$x", "@base64", "$__loc",
            ".a[]", ".a?", "map(", "[.a]", ".a,", "42", "\"done\"",
        ] {
            assert!(context(text).is_none(), "{text:?} offers nothing");
        }
    }

    #[test]
    fn key_context_carries_an_input_expression_and_word_context_does_not() {
        assert!(context(".x").unwrap().input_expr.is_some());
        assert!(context("x").unwrap().input_expr.is_none());
    }

    fn expr(text: &str) -> String {
        context(text)
            .unwrap_or_else(|| panic!("{text:?} should complete a key"))
            .input_expr
            .unwrap_or_else(|| panic!("{text:?} should be a key context"))
    }

    #[test]
    fn the_input_expression_is_the_filter_up_to_the_caret_s_dot() {
        let cases = [
            (".us", "."),
            (".", "."),
            (".data.it", ".data"),
            (".data.items[].na", ".data.items[]"),
            (".data.items[0].", ".data.items[0]"),
            (".data.items[] | .na", ".data.items[]"),
            (".data.items[] | select(.st", ".data.items[]"),
            (".data.items | map(.na", ".data.items | .[]"),
            (
                ".data.items[] | select(.status == \"a\") | .i",
                ".data.items[] | select(.status == \"a\")",
            ),
            (".data.items[] | {name: .name, s: .st", ".data.items[]"),
            ("[.data.items[] | .na", ".data.items[]"),
            (".data.items[] | .id == .na", ".data.items[]"),
            (".data.\"my k", ".data"),
            (".data.\"my key\".na", ".data.\"my key\""),
            (".a.b?.c", ".a.b?"),
            (
                ".a | if .x then .b else .c end | .d",
                ".a | if .x then .b else .c end",
            ),
            ("limit(3; .data.items[] | .na", ".data.items[]"),
            (".a |= .b", "."),
            (".a as $x | $x.na", ".a as $x | $x"),
            ("$x.na", "$x"),
            (".[] | .na", ".[]"),
            (".. | .na", ".."),
            (".a | .b | .c | .na", ".a | .b | .c"),
            ("(.a | .b).na", "(.a | .b)"),
            (
                ".data.items | sort_by(.id) | .[0].na",
                ".data.items | sort_by(.id) | .[0]",
            ),
            (".data | (.items[] | .na", ".data | .items[]"),
            ("select(.a == 1).na", "select(.a == 1)"),
        ];
        for (text, want) in cases {
            assert_eq!(expr(text), want, "for {text:?}");
        }
    }

    #[test]
    fn context_never_panics() {
        // A tiny deterministic generator over the characters jq uses.
        let alphabet: Vec<char> = ".[]{}()|\"\\$@?;:,=<>+-*/ abz_09".chars().collect();
        let mut seed: u64 = 0x9e37_79b9_7f4a_7c15;
        for _ in 0..20_000 {
            let mut s = String::new();
            let len = (seed % 12) as usize;
            for _ in 0..len {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                s.push(alphabet[(seed % alphabet.len() as u64) as usize]);
            }
            let _ = context(&s);
        }
        let _ = context("é.ü");
        let _ = context(".\"é");
    }

    fn builtin(name: &str) -> Option<&'static Builtin> {
        builtins().iter().find(|b| b.name == name)
    }

    #[test]
    fn builtins_come_from_jaq_plus_keywords_sorted_without_internals() {
        assert_eq!(builtin("select").map(|b| b.arity), Some(1));
        assert_eq!(builtin("length").map(|b| b.arity), Some(0));
        assert_eq!(builtin("map").map(|b| b.arity), Some(1));
        assert_eq!(
            builtin("range").map(|b| b.arity),
            Some(1),
            "lowest arity wins"
        );
        assert_eq!(builtin("if").map(|b| b.arity), Some(0));
        assert!(builtin("reduce").is_some());
        assert!(builtins().iter().all(|b| !b.name.starts_with('_')));
        let names: Vec<&str> = builtins().iter().map(|b| b.name).collect();
        let mut sorted = names.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(names, sorted, "sorted and unique");
    }

    fn ghosts(text: &str, keys: &[&str]) -> Vec<String> {
        let keys: Vec<String> = keys.iter().map(|k| k.to_string()).collect();
        candidates(&context(text).unwrap(), &keys)
            .into_iter()
            .map(|c| c.ghost)
            .collect()
    }

    #[test]
    fn key_candidates_strictly_extend_the_partial_in_body_order() {
        assert_eq!(ghosts(".", &["id", "name"]), vec!["id", "name"]);
        assert_eq!(
            ghosts(".i", &["name", "ids", "id", "identity"]),
            vec!["ds", "d", "dentity"]
        );
        assert_eq!(
            ghosts(".id", &["id", "ids"]),
            vec!["s"],
            "an exact match adds nothing"
        );
        assert!(ghosts(".zz", &["id"]).is_empty());
        assert!(ghosts(".I", &["id"]).is_empty(), "case-sensitive");
    }

    #[test]
    fn keys_that_are_not_identifiers_are_offered_quoted() {
        let keys = vec!["my key".to_string(), "a-b".to_string(), "ok".to_string()];
        let cs = candidates(&context(".").unwrap(), &keys);
        assert_eq!(cs[0].ghost, "\"my key\"");
        assert_eq!(cs[0].insert, ".\"my key\"");
        assert_eq!(cs[0].replace_from, Some(0), "replaces from the '.'");
        assert_eq!(cs[1].ghost, "\"a-b\"");
        assert_eq!((cs[2].ghost.as_str(), cs[2].replace_from), ("ok", None));

        let cs = candidates(&context(".data.my").unwrap(), &keys);
        assert_eq!(cs.len(), 1);
        assert_eq!(cs[0].insert, ".\"my key\"");
        assert_eq!(cs[0].replace_from, Some(5));

        let cs = candidates(&context(".\"my k").unwrap(), &keys);
        assert_eq!(cs.len(), 1);
        assert_eq!((cs[0].ghost.as_str(), cs[0].replace_from), ("ey\"", None));

        let keys = vec!["say \"hi\"".to_string()];
        let cs = candidates(&context(".").unwrap(), &keys);
        assert_eq!(cs[0].insert, ".\"say \\\"hi\\\"\"");
    }

    #[test]
    fn word_candidates_are_builtins_with_a_paren_when_they_take_arguments() {
        let g = ghosts("sel", &[]);
        assert_eq!(g[0], "ect(");
        let g = ghosts("leng", &[]);
        assert_eq!(g, vec!["th"]);
        assert!(ghosts("length", &[]).is_empty(), "exact match adds nothing");
        let g = ghosts("ma", &[]);
        assert!(g.contains(&"p(".to_string()));
        assert!(g.contains(&"p_values(".to_string()));
    }
}

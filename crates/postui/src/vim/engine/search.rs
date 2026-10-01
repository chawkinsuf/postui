//! Body search (spec §3.14, plan 3c): the Vim `magic` subset translated to
//! the `regex` crate (open question 5, decided), Vim's `\<` and `\>` done
//! exactly with sentinel chars (Deviation 4), and `searchit()`'s candidate
//! chain under `cpoptions` `c`. The golden file is the judge.

use super::buf::{Pos, TextBuf};
use super::class::{class, is_space};
use regex::Regex;

/// `/` or `?` (spec §3.14).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dir {
    Forward,
    Backward,
}

impl Dir {
    /// The prompt's char, which also ends the pattern (an offset follows).
    pub(crate) fn delim(self) -> char {
        match self {
            Dir::Forward => '/',
            Dir::Backward => '?',
        }
    }
}

/// Word start (`\<`), word end (`\>`), both. Private-use chars that stand
/// in the haystack before the position they describe.
const BOW: char = '\u{E000}';
const EOW: char = '\u{E001}';
const BOTH: char = '\u{E002}';
/// What a text char that is one of the sentinels becomes.
const TEXT_SENTINEL: char = '\u{E003}';
const SKIP: &str = "[\\x{E000}-\\x{E002}]*";
const NOT_SENTINEL: &str = "\\x{E000}-\\x{E002}";
/// Vim's `META_flags[]` (regexp.c): the chars a backslash makes magic in
/// `magic` mode (or, for `* . [ ~`, literal). A backslash before any other
/// char, or at the pattern's end, is that char (`peekchr()`).
const META: &str = "%&()*+.123456789<=>?@ACDFHIKLMOPSUVWXZ[_acdfhiklmnopsuvwxz{|~";
/// Vim's `REGEXP_ABBR` letters that are not META: `\r \t \e \b` are control
/// chars, out of the subset.
const ABBR: &str = "rteb";

/// A compiled pattern.
pub(crate) struct Pattern {
    re: Regex,
    /// The haystack needs the sentinels (`\<` or `\>` was used).
    boundaries: bool,
}

/// Where a translated pattern is, for the `^`/`$`/`*` context rules.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Prev {
    /// The start, or just after `\(` or `\|`: `^` anchors, `*` is literal.
    Start,
    /// After `^`: `*` is literal.
    Anchor,
    /// After an atom a multi may follow.
    Atom,
    /// After a multi or a zero-width atom: another multi is refused.
    NoMulti,
}

/// Translates Vim `magic` syntax (the subset in the plan's table) to the
/// `regex` crate's. `Err` carries the atom outside the subset, as typed.
/// `source` is the pattern as stored ([`as_stored`]), so it reads the same
/// in either direction: `\?` is always the multi, `?` always literal.
pub(crate) fn compile(source: &str) -> Result<Pattern, String> {
    let src: Vec<char> = source.chars().collect();
    let mut out = String::new();
    let mut prev = Prev::Start;
    let mut depth = 0usize;
    let mut boundaries = false;
    let mut i = 0;
    // The last emitted zero-width run, to collapse `\<\>` into one class.
    let mut run: Option<(bool, bool)> = None;
    let flush_run = |out: &mut String, run: &mut Option<(bool, bool)>| {
        if let Some((b, e)) = run.take() {
            out.push_str(match (b, e) {
                (true, true) => "\\x{E002}",
                (true, false) => "[\\x{E000}\\x{E002}]",
                _ => "[\\x{E001}\\x{E002}]",
            });
        }
    };
    let atom = |out: &mut String, re: &str, prev: &mut Prev| {
        out.push_str("(?:");
        out.push_str(SKIP);
        out.push_str(re);
        out.push(')');
        *prev = Prev::Atom;
    };
    while i < src.len() {
        let c = src[i];
        let at_end = i + 1 == src.len();
        let before_alt = src.get(i + 1) == Some(&'\\') && matches!(src.get(i + 2), Some('|' | ')'));
        let zero_width = |b: bool, e: bool, run: &mut Option<(bool, bool)>, boundaries: &mut bool, prev: &mut Prev| {
            let (ob, oe) = run.unwrap_or((false, false));
            *run = Some((ob || b, oe || e));
            *boundaries = true;
            *prev = Prev::NoMulti;
        };
        if c != '\\' || !matches!(src.get(i + 1), Some('<' | '>')) {
            flush_run(&mut out, &mut run);
        }
        match c {
            '\\' => {
                let Some(&n) = src.get(i + 1) else {
                    // A trailing backslash is a literal one (`peekchr()`).
                    i += 1;
                    atom(&mut out, "\\\\", &mut prev);
                    continue;
                };
                i += 2;
                match n {
                    '<' => zero_width(true, false, &mut run, &mut boundaries, &mut prev),
                    '>' => zero_width(false, true, &mut run, &mut boundaries, &mut prev),
                    '+' | '=' | '?' | '{' if prev != Prev::Atom => return Err(format!("\\{n}")),
                    '+' => {
                        out.push('+');
                        prev = Prev::NoMulti;
                    }
                    '=' => {
                        out.push('?');
                        prev = Prev::NoMulti;
                    }
                    '?' => {
                        out.push('?');
                        prev = Prev::NoMulti;
                    }
                    '{' => {
                        let close = src[i..].iter().position(|&c| c == '}').ok_or("\\{")?;
                        let mut body: String = src[i..i + close].iter().collect();
                        if body.ends_with('\\') {
                            body.pop();
                        }
                        if body.starts_with('-') {
                            return Err("\\{-".into());
                        }
                        if !body.chars().all(|c| c.is_ascii_digit() || c == ',') || body.matches(',').count() > 1 {
                            return Err("\\{".into());
                        }
                        out.push_str(&match body.as_str() {
                            "" => "*".to_string(),
                            b if b.starts_with(',') => format!("{{0{b}}}"),
                            b => format!("{{{b}}}"),
                        });
                        i += close + 1;
                        prev = Prev::NoMulti;
                    }
                    '(' => {
                        out.push_str("(?:");
                        depth += 1;
                        prev = Prev::Start;
                    }
                    ')' => {
                        if depth == 0 {
                            return Err("\\)".into());
                        }
                        depth -= 1;
                        out.push(')');
                        prev = Prev::Atom;
                    }
                    '|' => {
                        out.push('|');
                        prev = Prev::Start;
                    }
                    's' => atom(&mut out, "[ \\t]", &mut prev),
                    'S' => atom(&mut out, &format!("[^ \\t{NOT_SENTINEL}]"), &mut prev),
                    'd' => atom(&mut out, "[0-9]", &mut prev),
                    'D' => atom(&mut out, &format!("[^0-9{NOT_SENTINEL}]"), &mut prev),
                    'w' => atom(&mut out, "[0-9A-Za-z_]", &mut prev),
                    'W' => atom(&mut out, &format!("[^0-9A-Za-z_{NOT_SENTINEL}]"), &mut prev),
                    '/' | '.' | '*' | '[' | ']' | '^' | '$' | '~' | '\\' => atom(&mut out, &regex::escape(&n.to_string()), &mut prev),
                    // `\zs`, `\ze`, `\z(`: the atom is the two letters.
                    'z' => return Err(src[i - 2..(i + 1).min(src.len())].iter().collect()),
                    // Vim's `peekchr()`: a char with no `META_flags` entry
                    // and no abbreviation is itself (`\q` is `q`).
                    other if !META.contains(other) && !ABBR.contains(other) => {
                        atom(&mut out, &regex::escape(&text_char(other).to_string()), &mut prev)
                    }
                    other => return Err(format!("\\{other}")),
                }
            }
            '.' => {
                i += 1;
                atom(&mut out, &format!("[^{NOT_SENTINEL}]"), &mut prev);
            }
            '*' => {
                i += 1;
                match prev {
                    Prev::Start | Prev::Anchor => atom(&mut out, "\\*", &mut prev),
                    Prev::Atom => {
                        out.push('*');
                        prev = Prev::NoMulti;
                    }
                    Prev::NoMulti => return Err("*".into()),
                }
            }
            '^' if prev == Prev::Start => {
                i += 1;
                out.push('^');
                prev = Prev::Anchor;
            }
            '$' if at_end || before_alt => {
                i += 1;
                out.push_str(SKIP);
                out.push('$');
                prev = Prev::NoMulti;
            }
            '[' => {
                let (re, used) = collection(&src[i..])?;
                i += used;
                atom(&mut out, &re, &mut prev);
            }
            '~' => return Err("~".into()),
            c => {
                i += 1;
                atom(&mut out, &regex::escape(&text_char(c).to_string()), &mut prev);
            }
        }
    }
    flush_run(&mut out, &mut run);
    if depth > 0 {
        return Err("\\(".into());
    }
    let re = Regex::new(&out).map_err(|_| source.to_string())?;
    Ok(Pattern { re, boundaries })
}

/// A `[…]` collection at `src[0]`. Returns the regex class and the chars
/// consumed, or `(\[, 1)` when no `]` closes it (Vim: a literal `[`).
fn collection(src: &[char]) -> Result<(String, usize), String> {
    let mut i = 1;
    let mut negate = false;
    if src.get(i) == Some(&'^') {
        negate = true;
        i += 1;
    }
    let mut items: Vec<char> = Vec::new();
    let mut ranges: Vec<(char, char)> = Vec::new();
    let mut first = true;
    loop {
        let Some(&c) = src.get(i) else { return Ok(("\\[".into(), 1)) };
        if c == ']' && !first {
            i += 1;
            break;
        }
        first = false;
        let item = if c == '\\' {
            let Some(&n) = src.get(i + 1) else { return Ok(("\\[".into(), 1)) };
            i += 2;
            match n {
                ']' | '\\' | '^' | '-' => n,
                't' => '\t',
                'e' => '\u{1b}',
                'r' => '\r',
                'b' => '\u{8}',
                // A line break: multi-line patterns are out of the subset.
                'n' => return Err("\\n".into()),
                // A char code (`\d123`, `\o40`, `\x20`, `€`) is out.
                'd' | 'o' | 'x' | 'u' | 'U' if src.get(i).is_some_and(|c| c.is_ascii_hexdigit()) => {
                    return Err(format!("\\{n}"));
                }
                // Before any other char the backslash is itself a member
                // and the char is read next (Vim's `[\.]` is `\` or `.`).
                _ => {
                    i -= 1;
                    '\\'
                }
            }
        } else if c == '[' && src.get(i + 1) == Some(&':') {
            // A char index, never a byte offset: the class name may be any text.
            let end = src[i..].windows(2).position(|w| w == [':', ']']).ok_or("[:")?;
            return Err(src[i..i + end + 2].iter().collect());
        } else {
            i += 1;
            c
        };
        if src.get(i) == Some(&'-') && src.get(i + 1).is_some_and(|&n| n != ']') {
            let hi = src[i + 1];
            let hi = if hi == '\\' { src.get(i + 2).copied().unwrap_or('\\') } else { hi };
            i += if src[i + 1] == '\\' { 3 } else { 2 };
            ranges.push((item, hi));
        } else {
            items.push(item);
        }
    }
    // The members as the haystack spells them (`text_char`): a range whose
    // end is a private-use char reaches U+E003 instead.
    let mut set = String::new();
    for c in items {
        set.push_str(&regex::escape(&text_char(c).to_string()));
    }
    for (lo, hi) in ranges {
        let (lo, hi) = (text_char(lo), text_char(hi));
        set.push_str(&format!("{}-{}", regex::escape(&lo.to_string()), regex::escape(&hi.to_string())));
    }
    // Either way the class never takes a sentinel.
    Ok((if negate { format!("[^{NOT_SENTINEL}{set}]") } else { format!("[[{set}]--[{NOT_SENTINEL}]]") }, i))
}

/// A text char as the haystack holds it: the four private-use chars
/// U+E000–U+E003 all read as U+E003, on the pattern's side too.
fn text_char(c: char) -> char {
    if (BOW..=TEXT_SENTINEL).contains(&c) { TEXT_SENTINEL } else { c }
}

/// Vim's `BOW`: a word-class char whose previous char has another class.
fn word_start(line: &[char], i: usize) -> bool {
    line.get(i).is_some_and(|&c| class(c) >= 2 && (i == 0 || class(line[i - 1]) != class(c)))
}

/// Vim's `EOW`: the previous char is a word-class char and this position's
/// char (the line's end counts as class 0) has another class.
fn word_end(line: &[char], i: usize) -> bool {
    i > 0 && class(line[i - 1]) >= 2 && line.get(i).is_none_or(|&c| class(c) != class(line[i - 1]))
}

/// One line as the regex sees it, with the sentinels in, and the way back.
struct Hay {
    text: String,
    /// For each byte offset of `text` (and its length), the real column.
    cols: Vec<usize>,
    /// For each real column (and the line's length), the byte offset where
    /// that column starts, its sentinel included.
    starts: Vec<usize>,
}

impl Hay {
    fn new(line: &[char], boundaries: bool) -> Hay {
        let mut text = String::with_capacity(line.len() * 2);
        let mut cols = Vec::with_capacity(line.len() * 3);
        let mut starts = Vec::with_capacity(line.len() + 1);
        let push = |ch: char, col: usize, text: &mut String, cols: &mut Vec<usize>| {
            for _ in 0..ch.len_utf8() {
                cols.push(col);
            }
            text.push(ch);
        };
        for i in 0..=line.len() {
            starts.push(text.len());
            if boundaries {
                match (word_start(line, i), word_end(line, i)) {
                    (true, true) => push(BOTH, i, &mut text, &mut cols),
                    (true, false) => push(BOW, i, &mut text, &mut cols),
                    (false, true) => push(EOW, i, &mut text, &mut cols),
                    (false, false) => {}
                }
            }
            if let Some(&c) = line.get(i) {
                push(text_char(c), i, &mut text, &mut cols);
            }
        }
        cols.push(line.len());
        Hay { text, cols, starts }
    }
}

impl Pattern {
    /// Vim's candidate chain on one line under `cpoptions` `c`: the matches
    /// found searching from column 0, then from each match's end (one char
    /// on from an empty match), stopping at the line's end. `(start, end)`
    /// in chars, `end` exclusive; an empty match has `start == end`.
    pub(crate) fn chain(&self, line: &[char]) -> Vec<(usize, usize)> {
        let hay = Hay::new(line, self.boundaries);
        let mut out = Vec::new();
        let mut from = 0usize;
        let mut first = true;
        loop {
            if !first && hay.cols[from] >= line.len() {
                break;
            }
            first = false;
            let Some(m) = self.re.find_at(&hay.text, from) else { break };
            let (s, e) = (hay.cols[m.start()], hay.cols[m.end()]);
            out.push((s, e));
            if s == e {
                if s >= line.len() {
                    break;
                }
                from = hay.starts[s + 1];
            } else {
                from = hay.starts[e];
            }
        }
        out
    }
}

/// Vim's `searchit()` for `count` hits from `from` under `wrapscan` and
/// `cpoptions` `c` (the chain). Forward: the first chain match on the
/// caret's line starting after the caret (one starting on the line's end
/// counts one column back), then down, then from the top to the caret's
/// line. Backward: the last chain match before the caret (the caret's line
/// is skipped when the caret is in column 0), then up, then from the
/// bottom. Returns where and whether a wrap happened.
pub(crate) fn search<B: TextBuf>(buf: &B, from: Pos, dir: Dir, count: usize, pat: &Pattern) -> Option<(Pos, bool)> {
    let lines = buf.line_count();
    let mut pos = from;
    let mut wrapped = false;
    for _ in 0..count.max(1) {
        let start = pos;
        let mut found = None;
        match dir {
            Dir::Forward => {
                let mut row = start.row;
                let mut first = true;
                for loop_ in 0..2 {
                    while row < lines {
                        let line = buf.line(row);
                        let hit = pat.chain(&line).into_iter().find(|&(s, _)| {
                            let s_adj = if s == line.len() && s > 0 { s - 1 } else { s };
                            !first || s_adj > start.col
                        });
                        if let Some((s, _)) = hit {
                            found = Some(Pos::new(row, s));
                            break;
                        }
                        first = false;
                        if loop_ == 1 && row == start.row {
                            break;
                        }
                        row += 1;
                    }
                    if found.is_some() || loop_ == 1 {
                        break;
                    }
                    wrapped = true;
                    row = 0;
                    first = false;
                }
            }
            Dir::Backward => {
                let mut first = start.col > 0;
                let mut row = if start.col == 0 { start.row as isize - 1 } else { start.row as isize };
                for loop_ in 0..2 {
                    while row >= 0 {
                        let r = row as usize;
                        let hit = pat.chain(&buf.line(r)).into_iter().rfind(|&(s, _)| !first || s < start.col);
                        if let Some((s, _)) = hit {
                            found = Some(Pos::new(r, s));
                            break;
                        }
                        first = false;
                        if loop_ == 1 && r == start.row {
                            break;
                        }
                        row -= 1;
                    }
                    if found.is_some() || loop_ == 1 {
                        break;
                    }
                    wrapped = true;
                    row = lines as isize - 1;
                    first = false;
                }
            }
        }
        pos = found?;
    }
    Some((pos, wrapped))
}

/// Vim's `find_ident_at_pos(FIND_IDENT | FIND_STRING)`: the keyword under
/// or after `col` (a run of one word class, class 2 and up), else the
/// non-blank string there. `[start, end)` in chars.
pub(crate) fn find_ident(line: &[char], col: usize) -> Option<(usize, usize)> {
    for pass in 0..2 {
        // 1. Skip to the start of a keyword (pass 0) or of any text (pass 1).
        let mut i = col;
        while i < line.len() {
            let k = class(line[i]);
            if k != 0 && (pass == 1 || k != 1) {
                break;
            }
            i += 1;
        }
        // 2. Back up over the same class.
        let this = line.get(i).map_or(0, |&c| class(c));
        while i > 0 && this != 0 && class(line[i - 1]) == this {
            i -= 1;
        }
        let this = if this > 2 { 2 } else { this };
        if pass == 1 || this == 2 {
            if i >= line.len() || (pass == 0 && this != 2) {
                return None;
            }
            // 3. The end: the same class (pass 0), or any non-blank (pass 1).
            let k = class(line[i]);
            let mut end = i;
            while end < line.len() && (if pass == 0 { class(line[end]) == k } else { class(line[end]) != 0 }) {
                end += 1;
            }
            return Some((i, end));
        }
    }
    None
}

/// Vim's `nv_ident()` pattern for `*` (`backward` false) and `#`: the text
/// with `/ . * ~ [ ^ $ \` escaped (`#` also `?`), `\<` before it when its
/// first char is a word char and `\>` after it when its last one is.
pub(crate) fn ident_pattern(line: &[char], start: usize, end: usize, backward: bool) -> String {
    let special: &[char] = if backward { &['/', '?', '.', '*', '~', '[', '^', '$', '\\'] } else { &['/', '.', '*', '~', '[', '^', '$', '\\'] };
    let mut pat = String::new();
    if class(line[start]) >= 2 {
        pat.push_str("\\<");
    }
    for &c in &line[start..end] {
        if special.contains(&c) {
            pat.push('\\');
        }
        pat.push(c);
    }
    if class(line[end - 1]) >= 2 {
        pat.push_str("\\>");
    }
    pat
}

/// The pattern as Vim's `do_search()` keeps it (`skip_regexp_ex()` with
/// the search's delimiter): in a `?` search `\?` becomes `?`, which is a
/// literal `?` in any direction. `*`'s and `#`'s patterns go through it
/// too, so `#` on `??` stores `??`.
pub(crate) fn as_stored(pat: &str, dir: Dir) -> String {
    let src: Vec<char> = pat.chars().collect();
    skip_regexp(&src, dir, false).0
}

/// Vim's `skip_regexp_ex()`: the pattern typed for `dir` as `do_search()`
/// keeps it (`\?` becomes `?` in a `?` search), and where it ends: at the
/// first `dir.delim()` that is neither escaped nor inside a `[…]` when
/// `stop` is set, else at the end. A `[` with no closing `]` takes the
/// rest of the line (`/[/` is the pattern `[/`). `\v` and `\V` switch
/// whether a bare `[` opens a collection, as in Vim.
fn skip_regexp(src: &[char], dir: Dir, stop: bool) -> (String, usize) {
    let delim = dir.delim();
    let mut out = String::with_capacity(src.len());
    // Vim's `mymagic >= MAGIC_ON`.
    let mut magic = true;
    let mut p = 0;
    while p < src.len() {
        let c = src[p];
        if stop && c == delim {
            break;
        }
        if (c == '[' && magic) || (c == '\\' && src.get(p + 1) == Some(&'[') && !magic) {
            // `skip_anyof(p + 1)`, as the C passes it in both cases.
            let close = skip_anyof(src, p + 1);
            if close >= src.len() {
                out.extend(&src[p..]);
                return (out, src.len());
            }
            out.extend(&src[p..=close]);
            p = close + 1;
        } else if c == '\\' && p + 1 < src.len() {
            // A backslash pair is one item (`\\?` keeps its `\\`).
            let n = src[p + 1];
            if !(dir == Dir::Backward && n == '?') {
                out.push('\\');
            }
            out.push(n);
            match n {
                'v' => magic = true,
                'V' => magic = false,
                _ => {}
            }
            p += 2;
        } else {
            out.push(c);
            p += 1;
        }
    }
    (out, p)
}

/// Vim's `skip_anyof()` from just after a `[`: the index of the `]` that
/// closes the collection, or `src.len()` when none does.
fn skip_anyof(src: &[char], mut p: usize) -> usize {
    const CLASSES: [&str; 19] = [
        "alnum", "alpha", "blank", "cntrl", "digit", "graph", "lower", "print", "punct", "space", "upper", "xdigit", "tab",
        "return", "backspace", "escape", "ident", "keyword", "fname",
    ];
    if src.get(p) == Some(&'^') {
        p += 1;
    }
    if matches!(src.get(p), Some(']' | '-')) {
        p += 1;
    }
    while p < src.len() && src[p] != ']' {
        let c = src[p];
        if c == '-' {
            p += 1;
            if p < src.len() && src[p] != ']' {
                p += 1;
            }
        } else if c == '\\' && src.get(p + 1).is_some_and(|n| "]^-n\\".contains(*n) || "nrtebdoxuU".contains(*n)) {
            p += 2;
        } else if c == '[' {
            // `[:name:]` with a known name, `[=x=]`, `[.x.]`: one item.
            let class = src.get(p + 1) == Some(&':')
                && src[p + 2..].windows(2).position(|w| w == [':', ']']).is_some_and(|end| {
                    let name: String = src[p + 2..p + 2 + end].iter().collect();
                    if CLASSES.contains(&name.as_str()) {
                        p += 2 + end + 2;
                        true
                    } else {
                        false
                    }
                });
            let equiv = !class
                && matches!(src.get(p + 1), Some('=' | '.'))
                && src.get(p + 3) == src.get(p + 1)
                && src.get(p + 4) == Some(&']');
            if equiv {
                p += 5;
            } else if !class {
                p += 1;
            }
        } else {
            p += 1;
        }
    }
    p
}

/// The `/` or `?` line while it is typed (spec §3.14, Deviation 16; Vim's
/// `getcmdline()`). `cursor` is a char index into `text`.
#[derive(Debug, Clone)]
pub(crate) struct Prompt {
    pub dir: Dir,
    pub text: Vec<char>,
    pub cursor: usize,
}

impl Prompt {
    pub(crate) fn new(dir: Dir) -> Self {
        Self { dir, text: Vec::new(), cursor: 0 }
    }

    pub(crate) fn insert(&mut self, c: char) {
        self.text.insert(self.cursor, c);
        self.cursor += 1;
    }

    /// `BS`: `false` when the line was empty (Vim then leaves the line).
    /// With the caret at the start of a non-empty line it does nothing.
    pub(crate) fn backspace(&mut self) -> bool {
        if self.text.is_empty() {
            return false;
        }
        if self.cursor > 0 {
            self.cursor -= 1;
            self.text.remove(self.cursor);
        }
        true
    }

    /// `Del`: the char under the caret, or the one before it at the end;
    /// `false` when the line was empty.
    pub(crate) fn delete(&mut self) -> bool {
        if self.text.is_empty() {
            return false;
        }
        if self.cursor < self.text.len() {
            self.cursor += 1;
        }
        self.backspace()
    }

    /// `ctrl+w` (Vim's `cmdline_erase_chars()`): blanks, then one char
    /// class. Never leaves the line.
    pub(crate) fn word_back(&mut self) {
        let mut p = self.cursor;
        while p > 0 && is_space(self.text[p - 1]) {
            p -= 1;
        }
        if p > 0 {
            let k = class(self.text[p - 1]);
            while p > 0 && !is_space(self.text[p - 1]) && class(self.text[p - 1]) == k {
                p -= 1;
            }
        }
        self.text.drain(p..self.cursor);
        self.cursor = p;
    }

    /// `ctrl+u`: everything before the caret.
    pub(crate) fn delete_to_start(&mut self) {
        self.text.drain(..self.cursor);
        self.cursor = 0;
    }

    /// The pattern and the offset: the text up to the first delimiter (`/`
    /// or `?` for this direction) that is neither escaped nor inside a
    /// `[…]` (Vim's `skip_regexp()`), and what follows it.
    pub(crate) fn split(&self) -> (String, String) {
        let (_, end) = skip_regexp(&self.text, self.dir, true);
        let pat = self.text[..end].iter().collect();
        let offset = self.text.get(end + 1..).map_or(String::new(), |rest| rest.iter().collect());
        (pat, offset)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chars(s: &str) -> Vec<char> {
        s.chars().collect()
    }

    fn chain(pat: &str, line: &str) -> Vec<(usize, usize)> {
        compile(pat).expect(pat).chain(&chars(line))
    }

    /// Vim 9.1's answers (plan 3c, batches p4 and p7). Three rows were
    /// re-probed on 2026-10-01: `x\?` and `[a-` (mis-transcribed from p4 and
    /// p7) and `?x` (in no batch). The refused atoms are the subset's edge.
    #[test]
    fn the_subset_translates_and_the_rest_is_refused() {
        for (pat, line, want) in [
            ("foo", "foo bar foo", vec![(0, 3), (8, 11)]),
            ("fo*", "foo bar foo", vec![(0, 3), (8, 11)]),
            ("f.o", "foo bar foo", vec![(0, 3), (8, 11)]),
            ("^baz", "baz foo", vec![(0, 3)]),
            ("foo$", "foo bar foo", vec![(8, 11)]),
            ("[fb]a", "foo bar foo", vec![(4, 6)]),
            ("\\(foo\\|baz\\)", "baz foo", vec![(0, 3), (4, 7)]),
            ("\\d\\+", "baz 42", vec![(4, 6)]),
            ("\\s", "a b\tc", vec![(1, 2), (3, 4)]),
            ("o\\+", "foo bar foo", vec![(1, 3), (9, 11)]),
            ("x\\=", "abc", vec![(0, 0), (1, 1), (2, 2)]),
            ("o\\{2}", "foo bar foo", vec![(1, 3), (9, 11)]),
            ("a\\{,2}b", "ab aab aaab", vec![(0, 2), (3, 6), (8, 11)]),
            ("a\\{}b", "ab aab", vec![(0, 2), (3, 6)]),
            ("\\.", "a.b", vec![(1, 2)]),
            ("a\\/b", "a/b", vec![(0, 3)]),
            ("*b", "a*b", vec![(1, 3)]),
            ("^*b", "*b", vec![(0, 2)]),
            ("c^d", "ab c^d", vec![(3, 6)]),
            ("b$c", "ab$c d", vec![(1, 4)]),
            ("a[]-]b", "a-b a]b", vec![(0, 3), (4, 7)]),
            ("a[\\]]b", "a-b a]b", vec![(4, 7)]),
            ("a[^X]b", "aXb ayb", vec![(4, 7)]),
            ("a[[]b", "a[b", vec![(0, 3)]),
            ("[", "foo [x", vec![(4, 5)]),
            ("[a-", "foo [a- foo", vec![(4, 7)]),
            ("[]", "a[]", vec![(1, 3)]),
            ("[^]", "a[^]", vec![(1, 4)]),
            ("b[\\t]r", "b\tr btr", vec![(0, 3)]),
            ("b[a^]r", "b^r", vec![(0, 3)]),
            // Golden `search/patterns-cases` `/b[\.]r`: the backslash is a member.
            ("b[\\.]r", "b.r b\\r bxr", vec![(0, 3), (4, 7)]),
            ("[\\d]", "a\\d", vec![(1, 2), (2, 3)]),
            // Golden `search/patterns` `/\q` and `/\`: an escape with no META
            // flag, and a trailing backslash, are literal (`peekchr()`).
            ("\\q", "aq", vec![(1, 2)]),
            ("\\", "a\\b", vec![(1, 2)]),
            ("foo\\", "foo\\ foo", vec![(0, 4)]),
            ("\\j\\0\\}\\N", "j0}N", vec![(0, 4)]),
            ("a\\Wb", "a_b a-b", vec![(4, 7)]),
            ("\\<foo\\>", "foobar foo", vec![(7, 10)]),
            ("a.*b\\>", "ab-cd ef", vec![(0, 2)]),
            ("\\<.*b", "-a b", vec![(1, 4)]),
            ("\\<x", "日本語x 日本", vec![(3, 4)]),
            ("b\\>", "ab日本語", vec![(1, 2)]),
            ("\\<y", "x×y x", vec![]),
            ("\\<\\>", "ab 日本語x", vec![(6, 6)]),
            ("\\>", "a  b", vec![(1, 1), (4, 4)]),
            ("\\<", "a  b", vec![(0, 0), (3, 3)]),
            ("$", "abc", vec![(3, 3)]),
            ("aa", "aaaa", vec![(0, 2), (2, 4)]),
            ("x*", "abc", vec![(0, 0), (1, 1), (2, 2)]),
            ("b\\|", "ab", vec![(0, 0), (1, 2)]),
            ("\\(x\\|r\\)*", "xxr", vec![(0, 3)]),
            ("\\(^f\\)", "ff", vec![(0, 1)]),
            ("\\(o$\\|x\\)", "foo", vec![(2, 3)]),
            ("foo?", "foo? x", vec![(0, 4)]),
            ("?x", "a?x", vec![(1, 3)]),
            ("foo/", "foo/ x", vec![(0, 4)]),
        ] {
            assert_eq!(chain(pat, line), want, "{pat:?} on {line:?}");
        }
        for (pat, atom) in [
            ("\\v foo", "\\v"), ("\\zsfoo", "\\zs"), ("\\bfoo", "\\b"), ("\\e", "\\e"), ("\\t", "\\t"),
            ("a\\nb", "\\n"), ("~", "~"), ("\\_s", "\\_"), ("\\a", "\\a"), ("\\%d", "\\%"), ("\\V.", "\\V"), ("\\c", "\\c"),
            ("\\u", "\\u"), ("b[[:upper:]]r", "[:upper:]"), ("b\\)", "\\)"), ("\\(b", "\\("), ("\\{2}b", "\\{"), ("\\+b", "\\+"),
            ("^\\+b", "\\+"), ("b**r", "*"), ("o\\{", "\\{"), ("a\\{-1,}b", "\\{-"), ("\\<*", "*"), ("\\1", "\\1"),
            ("[[:日本:]", "[:日本:]"), ("x[[:é:]]", "[:é:]"), ("b[\\n]r", "\\n"), ("[\\d65]", "\\d"), ("[\\x41]", "\\x"),
        ] {
            assert_eq!(compile(pat).err().as_deref(), Some(atom), "{pat:?}");
        }
        assert!(compile("\\?").is_err(), "a multi with nothing before it");
    }

    /// The sentinel haystack maps every position back, and a text char
    /// that is a sentinel is read as U+E003.
    #[test]
    fn sentinels_never_leak_into_positions() {
        assert_eq!(chain("\\<ab\\>", "ab ab\u{E000}ab"), vec![(0, 2)]);
        assert_eq!(chain(".", "a\u{E001}b")[1], (1, 2));
        assert_eq!(chain("\\<[^a]", "a b"), vec![(2, 3)]);
        // A collection never takes a sentinel, even when its range spans them.
        assert_eq!(chain("\\<a[a-\u{F000}]", "a b"), vec![]);
        assert_eq!(chain("\\<a[a-\u{F000}]", "ab"), vec![(0, 2)]);
    }

    /// U+E000–U+E003 in the pattern read as the haystack reads them in the
    /// text (Vim's class 2: part of a keyword). Vim: `*` on
    /// `ab\u{E000} x ab\u{E000}` from 1:1 lands on 1:7.
    #[test]
    fn private_use_chars_in_the_pattern_match_themselves() {
        let line = chars("ab\u{E000} x ab\u{E000}");
        let pat = ident_pattern(&line, 0, 3, false);
        assert_eq!(compile(&pat).expect(&pat).chain(&line), vec![(0, 3), (6, 9)]);
        assert_eq!(chain("\u{E002}", "a\u{E002}b"), vec![(1, 2)]);
        assert_eq!(chain("[\u{E001}]", "a\u{E000}b"), vec![(1, 2)]);
        assert_eq!(chain("[\u{E000}-\u{E001}]", "a\u{E003}b"), vec![(1, 2)]);
        assert_eq!(chain("[b-\u{E001}]", "a\u{E002}"), vec![(1, 2)]);
    }

    /// `find_ident_at_pos(FIND_IDENT | FIND_STRING)` as the C reads.
    #[test]
    fn find_ident_takes_the_keyword_else_the_string() {
        for (line, col, want) in [
            ("foo bar", 4, Some((4, 7))),
            ("a.b", 1, Some((2, 3))),
            ("... ---", 0, Some((0, 3))),
            ("日本語 abc", 0, Some((0, 3))),
            ("  ", 0, None),
            ("x ", 1, None),
            ("ab", 5, None),
        ] {
            assert_eq!(find_ident(&chars(line), col), want, "{line:?} at {col}");
        }
    }

    /// Probed: `#` on `??` stores `??`, and on `\?` stores `\\?`.
    #[test]
    fn a_question_search_stores_its_pattern_unescaped() {
        let line = chars("?? \\?");
        assert_eq!(as_stored(&ident_pattern(&line, 0, 2, true), Dir::Backward), "??");
        assert_eq!(as_stored(&ident_pattern(&line, 3, 5, true), Dir::Backward), "\\\\?");
        assert_eq!(as_stored("\\?x\\.", Dir::Forward), "\\?x\\.");
    }

    /// A typed `?` pattern is stored before it compiles, so `\?` is the
    /// literal `?` there and the multi in a `/` pattern (probed 2026-10-01:
    /// `?x\?` on `bar? x` from its end finds nothing; `?\?` on `a?` finds
    /// the `?`; `/b\?x` then `N` keeps the multi).
    #[test]
    fn a_stored_pattern_reads_the_same_in_either_direction() {
        let stored = |pat: &str, dir: Dir| chain(&as_stored(pat, dir), "bar? x a? bx");
        assert_eq!(as_stored("x\\?", Dir::Backward), "x?");
        assert_eq!(chain(&as_stored("x\\?", Dir::Backward), "bar? x"), vec![]);
        assert_eq!(chain(&as_stored("\\?", Dir::Backward), "a?"), vec![(1, 2)]);
        assert_eq!(stored("b\\?x", Dir::Forward), vec![(5, 6), (10, 12)]);
        assert_eq!(stored("b\\?x", Dir::Backward), vec![]);
    }

    /// The prompt's edits (Deviation 16) and its split at the delimiter.
    #[test]
    fn the_prompt_edits_and_splits() {
        let mut p = Prompt::new(Dir::Forward);
        for c in "ba r.x".chars() {
            p.insert(c);
        }
        p.word_back();
        assert_eq!(p.text.iter().collect::<String>(), "ba r.");
        p.word_back();
        assert_eq!(p.text.iter().collect::<String>(), "ba r");
        p.cursor = 1;
        assert!(p.delete());
        assert_eq!((p.text.iter().collect::<String>(), p.cursor), ("b r".to_string(), 1));
        p.delete_to_start();
        assert_eq!((p.text.iter().collect::<String>(), p.cursor), (" r".to_string(), 0));
        assert!(p.backspace(), "BS at the start of a non-empty line does nothing");
        p.cursor = 2;
        assert!(p.backspace() && p.backspace());
        assert!(!p.backspace() && !p.delete(), "an empty line is left");
        for c in "a\\/b/e".chars() {
            p.insert(c);
        }
        assert_eq!(p.split(), ("a\\/b".to_string(), "e".to_string()));
        let mut q = Prompt::new(Dir::Backward);
        for c in "a/b?".chars() {
            q.insert(c);
        }
        assert_eq!(q.split(), ("a/b".to_string(), String::new()));
    }

    /// Vim's `skip_regexp()` skips a `[…]`: a delimiter inside one does not
    /// end the pattern, an unclosed `[` takes the rest, and `\?` inside one
    /// is kept as typed.
    #[test]
    fn the_delimiter_inside_brackets_is_text() {
        let split = |dir: Dir, typed: &str| {
            let mut p = Prompt::new(dir);
            for c in typed.chars() {
                p.insert(c);
            }
            p.split()
        };
        let s = |a: &str, b: &str| (a.to_string(), b.to_string());
        assert_eq!(split(Dir::Forward, "a[/]b"), s("a[/]b", ""));
        assert_eq!(split(Dir::Forward, "a[/]b/e"), s("a[/]b", "e"));
        assert_eq!(split(Dir::Forward, "[/"), s("[/", ""));
        assert_eq!(split(Dir::Forward, "[]/]x/"), s("[]/]x", ""));
        assert_eq!(split(Dir::Forward, "[[:alpha:]/]/e"), s("[[:alpha:]/]", "e"));
        assert_eq!(split(Dir::Backward, "x[?]y"), s("x[?]y", ""));
        assert_eq!(split(Dir::Backward, "[?"), s("[?", ""));
        assert_eq!(split(Dir::Backward, "a\\[?b"), s("a\\[", "b"));
        assert_eq!(as_stored("\\?[\\?]", Dir::Backward), "?[\\?]");
        assert_eq!(chain(&as_stored("x[?]y", Dir::Backward), "x?y"), vec![(0, 3)]);
    }
}

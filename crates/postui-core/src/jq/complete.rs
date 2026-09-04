//! Completion for the response pane's jq bar: what the caret is typing
//! (`context`), which keys the body offers there (`keys_at`), jaq's own
//! builtin names (`builtins`), and the ghost text to draw for each
//! (`candidates`). Pure apart from `keys_at`; jaq's types never leave the
//! module.

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

/// The jq expression whose outputs the `.` at the end of `head` refers
/// to. Replaced by the real scanner in the next task.
fn input_expr(_head: &str) -> String {
    ".".into()
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!((c.kind, c.partial.as_str(), c.token_start), (Kind::Word, "sel", 0));
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

    #[test]
    fn context_never_panics() {
        // A tiny deterministic generator over the characters jq uses.
        let alphabet: Vec<char> =
            ".[]{}()|\"\\$@?;:,=<>+-*/ abz_09".chars().collect();
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
}

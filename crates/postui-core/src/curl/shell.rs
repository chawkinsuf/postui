//! Bash-flavoured splitting of a pasted curl command into commands and
//! words (spec §3.1), turning shell variables into `{{NAME}}` on the way
//! (§3.2). Not a shell: nothing runs, and nothing expands beyond what the
//! spec names.

use std::iter::Peekable;
use std::str::Chars;

/// One command of the paste: its words after quote removal, and what was
/// set aside on the way.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ShellCommand {
    pub words: Vec<String>,
    /// A pipeline tail or redirect after the command's own words
    /// (`| jq .`, `> out.json`), its words joined by spaces.
    pub tail: Option<String>,
    /// Shell variables converted to `{{NAME}}`, first-seen order, no repeats.
    pub converted: Vec<String>,
    /// `$(…)` and backtick substitutions, kept as literal text.
    pub substitutions: Vec<String>,
    /// One per word of `words`: what quote removal hid.
    pub marks: Vec<WordMark>,
    /// The command is its line, alone: nothing before it (`a && b`,
    /// `a; b`), ended by the line's end, not `;`, `&`, `&&` or `||`, and
    /// no tail. Only such a line can assign a value (spec R7).
    pub alone: bool,
    /// The text of the last `# @name …` comment line before this command
    /// and after the one before it (spec §3.1).
    pub name: Option<String>,
}

/// What a word was before quote removal, for reading assignments (spec R7).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct WordMark {
    /// The word opens with an unquoted `NAME=` or `NAME+=`: bash's
    /// assignment word. `"T"=1` is a command, not an assignment.
    pub assignment: bool,
    /// The word is the text written, quotes aside: no `$` outside single
    /// quotes (`$'…'` and `$"…"` are quotes too), no backticks, and no
    /// unquoted `~` or `{` bash might expand.
    pub literal: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ShellError {
    /// A quote, `${` or substitution never closed, in the paste's command
    /// `command` (1-based, counting non-empty commands).
    Unclosed { command: usize },
    /// Windows cmd quoting (`^"` plus ` ^` line continuations).
    CmdStyle,
}

pub(crate) fn split(text: &str) -> Result<Vec<ShellCommand>, ShellError> {
    let text = text.replace("\r\n", "\n");
    Splitter {
        chars: text.chars().peekable(),
        commands: Vec::new(),
        current: ShellCommand::default(),
        word: String::new(),
        in_word: false,
        quoted: false,
        mark: WordMark::default(),
        expanded: false,
        in_tail: false,
        tail_words: Vec::new(),
        line_start: true,
        after_and_or: false,
        name: None,
    }
    .run()
}

/// A shell variable name: `[A-Za-z_][A-Za-z0-9_]*` — always a valid
/// postui variable name too.
pub(super) fn is_name(s: &str) -> bool {
    let mut chars = s.chars();
    chars.next().is_some_and(|c| c == '_' || c.is_ascii_alphabetic())
        && chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
}

/// `$'…'` bytes as text: valid UTF-8 runs stay as they are, and each byte
/// of an invalid sequence becomes the Latin-1 char U+00XX (Firefox writes
/// `é` as `\xe9`).
fn decode_bytes(mut bytes: &[u8]) -> String {
    let mut out = String::new();
    loop {
        match std::str::from_utf8(bytes) {
            Ok(s) => {
                out.push_str(s);
                return out;
            }
            Err(e) => {
                let (valid, rest) = bytes.split_at(e.valid_up_to());
                out.push_str(std::str::from_utf8(valid).unwrap_or_default());
                let bad = e.error_len().unwrap_or(rest.len());
                out.extend(rest[..bad].iter().map(|&b| char::from(b)));
                bytes = &rest[bad..];
            }
        }
    }
}

struct Splitter<'a> {
    chars: Peekable<Chars<'a>>,
    commands: Vec<ShellCommand>,
    current: ShellCommand,
    word: String,
    /// A word has begun, even an empty quoted one (`''`).
    in_word: bool,
    /// Something quoted, escaped or expanded is in the word so far.
    quoted: bool,
    mark: WordMark,
    /// The word so far is not `WordMark::literal`.
    expanded: bool,
    /// Past a `|` or a redirect: words go to the tail.
    in_tail: bool,
    tail_words: Vec<String>,
    /// The command being read began its line.
    line_start: bool,
    /// The last command was ended by `&&` or `||`.
    after_and_or: bool,
    /// A `# @name …` line waiting for the next command.
    name: Option<String>,
}

impl Splitter<'_> {
    /// Browsers' "Copy as cURL (cmd)" ends each continued line with ` ^` and
    /// quotes with `^"`. Either signal refuses the paste: a bare `^` word
    /// ending a line (`cmd_continuation`), or a word opening with `^"`. Both
    /// are seen outside quotes only, so a bash body holding them still
    /// parses.
    fn run(mut self) -> Result<Vec<ShellCommand>, ShellError> {
        while let Some(c) = self.chars.next() {
            match c {
                ' ' | '\t' => self.end_word(),
                '\n' if self.line_continues() => {}
                '\n' => {
                    if self.cmd_continuation() {
                        return Err(ShellError::CmdStyle);
                    }
                    self.end_command(true);
                }
                ';' => self.end_command(false),
                '&' => {
                    // `&&` and a lone `&` (background) both end the command.
                    let and = self.chars.peek() == Some(&'&');
                    if and {
                        self.chars.next();
                    }
                    self.end_command(false);
                    self.after_and_or = and;
                }
                '|' => {
                    if self.chars.peek() == Some(&'|') {
                        self.chars.next();
                        self.end_command(false);
                        self.after_and_or = true;
                    } else {
                        self.end_word();
                        self.start_tail("|".to_string());
                    }
                }
                '>' | '<' => self.redirect(c),
                '#' if !self.in_word => self.skip_comment(),
                '\\' => match self.chars.next() {
                    Some('\n') => {} // a continuation
                    Some(n) => {
                        self.quoted = true;
                        self.push(n);
                    }
                    None => self.push('\\'),
                },
                '\'' => {
                    self.in_word = true;
                    self.quoted = true;
                    self.single_quoted()?;
                }
                '"' => {
                    // `^"` opening a word is Windows cmd quoting.
                    if self.in_word && self.word == "^" {
                        return Err(ShellError::CmdStyle);
                    }
                    self.in_word = true;
                    self.quoted = true;
                    self.double_quoted()?;
                }
                '`' => {
                    self.in_word = true;
                    self.quoted = true;
                    self.backtick()?;
                }
                '$' => {
                    self.quoted = true;
                    self.dollar(false)?;
                }
                c => self.push_unquoted(c),
            }
        }
        if self.cmd_continuation() {
            return Err(ShellError::CmdStyle);
        }
        self.end_command(true);
        Ok(self.commands)
    }

    /// The line so far ends in `&&`, `||` or `|`, so bash reads on into
    /// the next line, blank lines and comments aside, as the same command
    /// line.
    fn line_continues(&self) -> bool {
        if self.in_word {
            false
        } else if self.in_tail {
            self.tail_words.last().is_some_and(|w| w == "|")
        } else {
            self.after_and_or && self.current.words.is_empty()
        }
    }

    /// The line ends in cmd's ` ^` continuation: its last word is `^`.
    fn cmd_continuation(&mut self) -> bool {
        self.end_word();
        !self.in_tail && self.current.words.last().is_some_and(|w| w == "^")
    }

    fn unclosed(&self) -> ShellError {
        ShellError::Unclosed { command: self.commands.len() + 1 }
    }

    fn push(&mut self, c: char) {
        self.in_word = true;
        self.word.push(c);
    }

    /// A char outside any quote: it may make the word an assignment, or
    /// be a tilde or brace bash might expand.
    fn push_unquoted(&mut self, c: char) {
        let name = self.word.strip_suffix('+').unwrap_or(&self.word);
        if c == '=' && !self.quoted && !self.mark.assignment && is_name(name) {
            self.mark.assignment = true;
        }
        if c == '~' || c == '{' {
            self.expanded = true;
        }
        self.push(c);
    }

    fn end_word(&mut self) {
        if !self.in_word {
            return;
        }
        let w = std::mem::take(&mut self.word);
        let mut mark = std::mem::take(&mut self.mark);
        mark.literal = !std::mem::take(&mut self.expanded);
        if self.in_tail {
            self.tail_words.push(w);
        } else {
            self.current.words.push(w);
            self.current.marks.push(mark);
        }
        self.in_word = false;
        self.quoted = false;
    }

    fn start_tail(&mut self, op: String) {
        self.in_tail = true;
        self.tail_words.push(op);
    }

    /// `ends_line`: a line break or the paste's end ended it.
    fn end_command(&mut self, ends_line: bool) {
        self.end_word();
        if self.in_tail {
            self.current.tail = Some(self.tail_words.join(" "));
            self.tail_words.clear();
            self.in_tail = false;
        }
        let mut cmd = std::mem::take(&mut self.current);
        cmd.alone = self.line_start && ends_line && cmd.tail.is_none();
        // A blank command (`a;;`) still leaves the rest of the line shared.
        self.line_start = ends_line;
        self.after_and_or = false;
        if matches!(cmd.words.first().map(String::as_str), Some("$" | "%")) {
            cmd.words.remove(0);
            cmd.marks.remove(0);
        }
        if !cmd.words.is_empty() {
            cmd.name = self.name.take();
            self.commands.push(cmd);
        }
    }

    /// `>`, `>>`, `<`, `2>`, `2>&1`: the curl part ends here and the rest
    /// of the command is tail. `&>` and `|&` are not seen here: the `&` ends
    /// the command first, and the redirect after it is dropped unlisted.
    fn redirect(&mut self, first: char) {
        let mut op = String::new();
        // A bare fd number glued to the operator (`2>`) belongs to it.
        if self.in_word && !self.word.is_empty() && self.word.chars().all(|d| d.is_ascii_digit()) {
            op = std::mem::take(&mut self.word);
            self.in_word = false;
            self.quoted = false;
            self.mark = WordMark::default();
            self.expanded = false;
        } else {
            self.end_word();
        }
        op.push(first);
        while let Some(&n) = self.chars.peek() {
            let after_amp = op.ends_with('&');
            if matches!(n, '>' | '<' | '&') || (after_amp && (n.is_ascii_digit() || n == '-')) {
                op.push(n);
                self.chars.next();
            } else {
                break;
            }
        }
        self.start_tail(op);
    }

    /// A comment that is its own line and reads `# @name …` names the next
    /// command.
    fn skip_comment(&mut self) {
        let mut text = String::new();
        while let Some(&n) = self.chars.peek() {
            if n == '\n' {
                break;
            }
            text.push(n);
            self.chars.next();
        }
        let own_line = self.line_start && self.current.words.is_empty() && !self.in_tail;
        let name = text.trim_start().strip_prefix("@name").filter(|r| r.starts_with([' ', '\t']));
        if let Some(name) = name.map(str::trim).filter(|n| !n.is_empty() && own_line) {
            self.name = Some(name.to_string());
        }
    }

    fn single_quoted(&mut self) -> Result<(), ShellError> {
        loop {
            match self.chars.next() {
                None => return Err(self.unclosed()),
                Some('\'') => return Ok(()),
                Some(c) => self.word.push(c),
            }
        }
    }

    fn double_quoted(&mut self) -> Result<(), ShellError> {
        loop {
            match self.chars.next() {
                None => return Err(self.unclosed()),
                Some('"') => return Ok(()),
                Some('\\') => match self.chars.next() {
                    Some(n @ ('"' | '\\' | '$' | '`')) => self.word.push(n),
                    Some('\n') => {}
                    Some(n) => {
                        self.word.push('\\');
                        self.word.push(n);
                    }
                    None => return Err(self.unclosed()),
                },
                Some('$') => self.dollar(true)?,
                Some('`') => self.backtick()?,
                Some(c) => self.word.push(c),
            }
        }
    }

    /// `$'…'` (bash ANSI-C quoting, what browsers emit): escapes decode to
    /// bytes, and the bytes decode as UTF-8 once the quote closes; bytes
    /// that are not valid UTF-8 are Latin-1 chars (`decode_bytes`). A
    /// `😀` surrogate pair is one char; a lone surrogate stays
    /// literal text.
    fn ansi_c(&mut self) -> Result<(), ShellError> {
        let mut bytes: Vec<u8> = Vec::new();
        loop {
            match self.chars.next() {
                None => return Err(self.unclosed()),
                Some('\'') => break,
                Some('\\') => {
                    let Some(e) = self.chars.next() else {
                        return Err(self.unclosed());
                    };
                    match e {
                        'n' => bytes.push(b'\n'),
                        't' => bytes.push(b'\t'),
                        'r' => bytes.push(b'\r'),
                        'a' => bytes.push(0x07),
                        'b' => bytes.push(0x08),
                        'e' | 'E' => bytes.push(0x1b),
                        'f' => bytes.push(0x0c),
                        'v' => bytes.push(0x0b),
                        '\\' | '\'' | '"' | '?' => bytes.push(e as u8),
                        'x' => {
                            let digits = self.take_digits(16, 2);
                            match u8::from_str_radix(&digits, 16) {
                                Ok(b) if !digits.is_empty() => bytes.push(b),
                                _ => bytes.extend_from_slice(b"\\x"),
                            }
                        }
                        'u' | 'U' => {
                            let max = if e == 'u' { 4 } else { 8 };
                            let digits = self.take_digits(16, max);
                            let mut code = u32::from_str_radix(&digits, 16).ok();
                            // Firefox writes an emoji as two `\u` surrogates.
                            if e == 'u'
                                && code.is_some_and(|c| (0xD800..0xDC00).contains(&c))
                                && let Some(low) = self.low_surrogate()
                            {
                                code =
                                    code.map(|h| 0x10000 + ((h - 0xD800) << 10) + (low - 0xDC00));
                            }
                            match code.and_then(char::from_u32) {
                                Some(c) if !digits.is_empty() => {
                                    let mut buf = [0u8; 4];
                                    bytes.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
                                }
                                _ => {
                                    bytes.push(b'\\');
                                    bytes.push(e as u8);
                                    bytes.extend_from_slice(digits.as_bytes());
                                }
                            }
                        }
                        '0'..='7' => {
                            let mut digits = e.to_string();
                            digits.push_str(&self.take_digits(8, 2));
                            let value = u32::from_str_radix(&digits, 8).unwrap_or(0);
                            bytes.push((value & 0xff) as u8);
                        }
                        other => {
                            bytes.push(b'\\');
                            let mut buf = [0u8; 4];
                            bytes.extend_from_slice(other.encode_utf8(&mut buf).as_bytes());
                        }
                    }
                }
                Some(c) => {
                    let mut buf = [0u8; 4];
                    bytes.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
                }
            }
        }
        self.word.push_str(&decode_bytes(&bytes));
        Ok(())
    }

    /// Consumes a directly following `\uDC00`..`\uDFFF` escape and returns
    /// its value; consumes nothing otherwise.
    fn low_surrogate(&mut self) -> Option<u32> {
        let mut ahead = self.chars.clone();
        if ahead.next() != Some('\\') || ahead.next() != Some('u') {
            return None;
        }
        let digits: String = ahead.by_ref().take(4).collect();
        let value = u32::from_str_radix(&digits, 16).ok()?;
        if digits.len() != 4 || !(0xDC00..0xE000).contains(&value) {
            return None;
        }
        self.chars = ahead;
        Some(value)
    }

    fn take_digits(&mut self, radix: u32, max: usize) -> String {
        let mut digits = String::new();
        while digits.len() < max {
            match self.chars.peek() {
                Some(&d) if d.is_digit(radix) => {
                    digits.push(d);
                    self.chars.next();
                }
                _ => break,
            }
        }
        digits
    }

    /// A `$` outside single quotes (spec §3.2).
    fn dollar(&mut self, in_double: bool) -> Result<(), ShellError> {
        self.in_word = true;
        match self.chars.peek().copied() {
            Some('\'') if !in_double => {
                self.chars.next();
                self.ansi_c()
            }
            // `$"…"` is bash's locale string: plain double quotes here.
            Some('"') if !in_double => {
                self.chars.next();
                self.double_quoted()
            }
            _ => {
                self.expanded = true;
                self.expansion()
            }
        }
    }

    /// A `$` that expands: a variable, a substitution, `$1` and the like.
    fn expansion(&mut self) -> Result<(), ShellError> {
        match self.chars.peek().copied() {
            Some('{') => {
                self.chars.next();
                let mut inner = String::new();
                loop {
                    match self.chars.next() {
                        Some('}') => break,
                        Some(c) => inner.push(c),
                        None => return Err(self.unclosed()),
                    }
                }
                if is_name(&inner) {
                    self.convert(inner);
                } else {
                    // `${X:-y}` and other operator forms stay as written.
                    self.word.push_str(&format!("${{{inner}}}"));
                }
                Ok(())
            }
            Some('(') => {
                self.chars.next();
                let inner = self.substitution_body()?;
                let literal = format!("$({inner})");
                self.word.push_str(&literal);
                self.current.substitutions.push(literal);
                Ok(())
            }
            Some(c) if c == '_' || c.is_ascii_alphabetic() => {
                let mut name = String::new();
                while let Some(&n) = self.chars.peek() {
                    if n == '_' || n.is_ascii_alphanumeric() {
                        name.push(n);
                        self.chars.next();
                    } else {
                        break;
                    }
                }
                self.convert(name);
                Ok(())
            }
            // `$$` (the pid) is literal, and so is the name after it.
            Some('$') => {
                self.chars.next();
                self.word.push_str("$$");
                Ok(())
            }
            // `$1`, `$?`, `$@`, a lone `$`: literal.
            _ => {
                self.word.push('$');
                Ok(())
            }
        }
    }

    /// The text of a `$(…)` up to its closing `)`, as written. Parentheses
    /// nest, and a `)` that is quoted or escaped inside does not close it.
    fn substitution_body(&mut self) -> Result<String, ShellError> {
        let mut depth = 1usize;
        let mut inner = String::new();
        loop {
            let Some(c) = self.chars.next() else {
                return Err(self.unclosed());
            };
            match c {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        return Ok(inner);
                    }
                }
                '\\' => {
                    inner.push(c);
                    match self.chars.next() {
                        Some(n) => inner.push(n),
                        None => return Err(self.unclosed()),
                    }
                    continue;
                }
                '\'' | '"' => {
                    inner.push(c);
                    loop {
                        match self.chars.next() {
                            None => return Err(self.unclosed()),
                            Some('\\') if c == '"' => {
                                inner.push('\\');
                                match self.chars.next() {
                                    Some(n) => inner.push(n),
                                    None => return Err(self.unclosed()),
                                }
                            }
                            Some(q) => {
                                inner.push(q);
                                if q == c {
                                    break;
                                }
                            }
                        }
                    }
                    continue;
                }
                _ => {}
            }
            inner.push(c);
        }
    }

    fn convert(&mut self, name: String) {
        self.word.push_str(&format!("{{{{{name}}}}}"));
        if !self.current.converted.contains(&name) {
            self.current.converted.push(name);
        }
    }

    fn backtick(&mut self) -> Result<(), ShellError> {
        let mut inner = String::new();
        loop {
            match self.chars.next() {
                None => return Err(self.unclosed()),
                Some('`') => break,
                Some('\\') => {
                    inner.push('\\');
                    if let Some(n) = self.chars.next() {
                        inner.push(n);
                    }
                }
                Some(c) => inner.push(c),
            }
        }
        let literal = format!("`{inner}`");
        self.word.push_str(&literal);
        self.expanded = true;
        self.current.substitutions.push(literal);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(text: &str) -> Vec<String> {
        let mut cmds = split(text).unwrap();
        assert_eq!(cmds.len(), 1, "{cmds:?}");
        cmds.remove(0).words
    }

    #[test]
    fn whitespace_separates_words() {
        assert_eq!(words("curl  -X\tPOST https://x"), ["curl", "-X", "POST", "https://x"]);
    }

    #[test]
    fn quotes_and_escapes_join_into_one_word() {
        assert_eq!(words(r#"curl 'a b' "c d" e\ f"#), ["curl", "a b", "c d", "e f"]);
        assert_eq!(words(r#"curl -H'A: '"b""#), ["curl", "-HA: b"]);
        assert_eq!(words("curl ''"), ["curl", ""]);
    }

    #[test]
    fn backslash_newline_continues_the_command() {
        assert_eq!(
            words("curl \\\n  -X POST \\\r\n  https://x"),
            ["curl", "-X", "POST", "https://x"]
        );
    }

    #[test]
    fn double_quotes_keep_only_their_own_escapes() {
        assert_eq!(words(r#"curl "a\"b\\c\$d\`e\q""#), ["curl", "a\"b\\c$d`e\\q"]);
    }

    #[test]
    fn ansi_c_escapes_decode() {
        assert_eq!(
            words(concat!(r"curl $'a\nb\'c\x41\", "u00e9", r"\101\\\", "u0021'")),
            ["curl", "a\nb'cAéA\\!"]
        );
        // Bytes from `\x` decode as UTF-8 together.
        assert_eq!(words(r"curl $'\xc3\xa9'"), ["curl", "é"]);
    }

    #[test]
    fn ansi_c_latin_1_bytes_and_surrogate_pairs_decode_as_firefox_meant() {
        // A lone Latin-1 byte (Firefox's escape for e-acute) is U+00E9.
        assert_eq!(words(concat!(r"curl $'Jos\", "xe9'")), ["curl", "José"]);
        // Valid UTF-8 byte pairs still decode as UTF-8.
        assert_eq!(words(concat!(r"curl $'\", "xc3", r"\", "xa9'")), ["curl", "é"]);
        // Valid runs stay, only the invalid byte is Latin-1.
        assert_eq!(words(concat!(r"curl $'é\", "xe9", r"\", "xc3", r"\", "xa9'")), ["curl", "ééé"]);
        // A surrogate pair joins into one char.
        assert_eq!(words(concat!(r"curl $'\", "ud83d", r"\", "ude00'")), ["curl", "\u{1F600}"]);
        // A lone surrogate stays literal text.
        assert_eq!(words(concat!(r"curl $'\", "ud83d'")), ["curl", concat!(r"\", "ud83d")]);
        assert_eq!(words(concat!(r"curl $'\", "ude00'")), ["curl", concat!(r"\", "ude00")]);
    }

    #[test]
    fn line_breaks_semicolons_and_logic_operators_end_commands() {
        let cmds = split("curl a\ncurl b; curl c && curl d || curl e & curl f").unwrap();
        let firsts: Vec<&str> = cmds.iter().map(|c| c.words[1].as_str()).collect();
        assert_eq!(firsts, ["a", "b", "c", "d", "e", "f"]);
    }

    #[test]
    fn a_command_is_alone_on_a_line_of_its_own_with_no_tail() {
        let alone = |t| split(t).unwrap().iter().map(|c| c.alone).collect::<Vec<_>>();
        assert_eq!(alone("a\\\n  b # note\n\nc\n"), [true, true]);
        assert_eq!(
            alone("a; b\nc && d\ne || f\ng &\nh"),
            [false, false, false, false, false, false, false, true]
        );
        assert_eq!(alone("a | b\nc > f\nd"), [false, false, true]);
    }

    #[test]
    fn a_line_ending_in_and_or_or_a_pipe_goes_on_into_the_next() {
        let cmds = split("a &&\n  b\nc ||  # note\n\n  d\ne |\n  jq .x\nf &\ng").unwrap();
        let firsts: Vec<&str> = cmds.iter().map(|c| c.words[0].as_str()).collect();
        assert_eq!(firsts, ["a", "b", "c", "d", "e", "f", "g"]);
        let alone: Vec<bool> = cmds.iter().map(|c| c.alone).collect();
        assert_eq!(alone, [false, false, false, false, false, false, true]);
        assert_eq!(cmds[4].tail.as_deref(), Some("| jq .x"));
        // A word after the operator ends the line as usual.
        let cmds = split("a | b\nc").unwrap();
        assert_eq!((cmds.len(), cmds[1].alone), (2, true));
    }

    #[test]
    fn blank_lines_and_comments_are_not_commands() {
        let cmds = split("# fetch it\n\ncurl https://x/#frag # trailing\n").unwrap();
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].words, ["curl", "https://x/#frag"]);
    }

    #[test]
    fn a_name_comment_names_the_next_command() {
        let names = |t| split(t).unwrap().into_iter().map(|c| c.name).collect::<Vec<_>>();
        let named = |n: &str| Some(n.to_string());
        // Comments and blank lines may sit between; a later name replaces.
        assert_eq!(
            names("# @name List users\n# note\n\ncurl a\ncurl b"),
            [named("List users"), None]
        );
        assert_eq!(names("# @name x\n  #@name   y  \ncurl a"), [named("y")]);
        // Another command takes it, even one that is not curl.
        assert_eq!(names("# @name x\nexport T=1\ncurl a"), [named("x"), None]);
        // Only a comment that is its own line counts.
        assert_eq!(names("curl a # @name x\ncurl b"), [None, None]);
        assert_eq!(names("curl a; # @name x\ncurl b"), [None, None]);
        assert_eq!(names("curl a \\\n# @name x\ncurl b"), [None, None]);
        // An empty name, another tag, or a name left at the end names nothing.
        assert_eq!(names("# @name\ncurl a\n# @names x\ncurl b\n# @name z"), [None, None]);
        // The text is kept as written: a `/` is a folder for the prompt.
        assert_eq!(names("# @name users/List all\ncurl a"), [named("users/List all")]);
    }

    #[test]
    fn a_pipeline_tail_or_redirect_is_set_aside() {
        let cmds = split("curl a | jq '.x | .y'\ncurl b > out.json 2>&1").unwrap();
        assert_eq!(cmds[0].words, ["curl", "a"]);
        assert_eq!(cmds[0].tail.as_deref(), Some("| jq .x | .y"));
        assert_eq!(cmds[1].words, ["curl", "b"]);
        assert_eq!(cmds[1].tail.as_deref(), Some("> out.json 2>&1"));
    }

    #[test]
    fn a_shell_prompt_is_stripped() {
        assert_eq!(words("$ curl https://x"), ["curl", "https://x"]);
        assert_eq!(words("% curl https://x"), ["curl", "https://x"]);
    }

    #[test]
    fn shell_variables_become_postui_variables() {
        let cmds = split(r#"curl -H "Authorization: Bearer $TOKEN" ${BASE}/x $TOKEN"#).unwrap();
        assert_eq!(
            cmds[0].words,
            ["curl", "-H", "Authorization: Bearer {{TOKEN}}", "{{BASE}}/x", "{{TOKEN}}"]
        );
        assert_eq!(cmds[0].converted, ["TOKEN", "BASE"]);
    }

    #[test]
    fn specials_and_single_quotes_stay_literal() {
        let cmds = split(r#"curl "$1 $? $$ ${X:-y} $" '$NOT' $'$ALSO' -u 'me:p@$$w0rd'"#).unwrap();
        assert_eq!(
            cmds[0].words,
            ["curl", "$1 $? $$ ${X:-y} $", "$NOT", "$ALSO", "-u", "me:p@$$w0rd"]
        );
        assert!(cmds[0].converted.is_empty());
    }

    #[test]
    fn a_double_dollar_keeps_the_name_after_it_literal() {
        let cmds = split(r#"curl -u "me:p@$$w0rd" -H p@$$w0rd https://x"#).unwrap();
        assert_eq!(cmds[0].words, ["curl", "-u", "me:p@$$w0rd", "-H", "p@$$w0rd", "https://x"]);
        assert!(cmds[0].converted.is_empty());
    }

    #[test]
    fn command_substitution_stays_literal_and_is_noted() {
        let cmds = split(r#"curl -H "X-Date: $(date +%s)" `id -u`"#).unwrap();
        assert_eq!(cmds[0].words, ["curl", "-H", "X-Date: $(date +%s)", "`id -u`"]);
        assert_eq!(cmds[0].substitutions, ["$(date +%s)", "`id -u`"]);
    }

    #[test]
    fn a_quoted_or_escaped_paren_does_not_close_a_substitution() {
        let cmds = split(r#"curl -H "X: $(echo ')' ")" \) (x))" https://x"#).unwrap();
        assert_eq!(cmds[0].words, ["curl", "-H", r#"X: $(echo ')' ")" \) (x))"#, "https://x"]);
        assert_eq!(cmds[0].substitutions, [r#"$(echo ')' ")" \) (x))"#]);
        assert_eq!(split("curl $(echo ')"), Err(ShellError::Unclosed { command: 1 }));
    }

    #[test]
    fn an_unclosed_quote_names_its_command() {
        assert_eq!(split("curl a\ncurl 'b"), Err(ShellError::Unclosed { command: 2 }));
        assert_eq!(split("curl \"a"), Err(ShellError::Unclosed { command: 1 }));
        assert_eq!(split("curl $'a"), Err(ShellError::Unclosed { command: 1 }));
    }

    fn marks(text: &str) -> Vec<(bool, bool)> {
        let cmds = split(text).unwrap();
        cmds[0].marks.iter().map(|m| (m.assignment, m.literal)).collect()
    }

    #[test]
    fn an_assignment_word_opens_with_an_unquoted_name_and_equals() {
        let m = |t| marks(t).into_iter().map(|(a, _)| a).collect::<Vec<_>>();
        assert_eq!(
            m("A=1 B='x y' C= \"D\"=1 E\\=1 F$X=1 =1 1A=2"),
            [true, true, true, false, false, false, false, false]
        );
        assert_eq!(m("A+=1 \"A\"+=1 A\\+=1 +=1 A++=1"), [true, false, false, false, false]);
        // Quotes after the `=` are part of the value.
        assert_eq!(m("A=\"$x\"=y"), [true]);
        assert_eq!(m("$ A=1 curl"), [true, false], "the prompt's mark goes with it");
    }

    #[test]
    fn a_word_is_literal_with_nothing_bash_might_expand() {
        let l = |t| marks(t).into_iter().map(|(_, l)| l).collect::<Vec<_>>();
        assert_eq!(
            l("A=$X A=\"${X}\" A=$(id) A=`id` A=\"`id`\" A=$1 A=a$ A=~/x A=a~b A={a,b} A=x{"),
            [false; 11]
        );
        assert_eq!(
            l("A=abc A='$X' A='{a,b}' A=\\{a A=\"{\" A=\"~\" A=$'a\\tb' A=$\"x\" A=http://x?a=1"),
            [true; 9]
        );
    }

    #[test]
    fn windows_cmd_quoting_is_refused() {
        assert_eq!(split("curl ^\"https://x^\" ^\n  -H ^\"A: b^\""), Err(ShellError::CmdStyle));
        // A bash body that merely contains `^"` still parses.
        assert!(split("curl -d '^\"x' https://x").is_ok());
        assert!(split("curl -d 'a ^\"x' https://x").is_ok());
        // One line is enough when a word opens with `^"`.
        assert_eq!(split("curl ^\"https://x/a^\""), Err(ShellError::CmdStyle));
        // Unquoted cmd: only the ` ^` continuation gives it away.
        assert_eq!(split("curl https://x ^  \r\n  -k"), Err(ShellError::CmdStyle));
        assert_eq!(split("curl https://x ^"), Err(ShellError::CmdStyle));
        // A quoted bash line that ends in ` ^` is body text.
        assert!(split("curl -d 'regex: a ^\nb' https://x").is_ok());
        assert!(split("curl -d \"a ^\nb\" https://x").is_ok());
    }
}

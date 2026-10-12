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
    if is_cmd_style(text) {
        return Err(ShellError::CmdStyle);
    }
    let text = text.replace("\r\n", "\n");
    Splitter {
        chars: text.chars().peekable(),
        commands: Vec::new(),
        current: ShellCommand::default(),
        word: String::new(),
        in_word: false,
        in_tail: false,
        tail_words: Vec::new(),
    }
    .run()
}

/// Browsers' "Copy as cURL (cmd)" quotes with `^"` and ends each
/// continued line with ` ^`. Both must show up, so a bash body that merely
/// contains `^"` still parses.
fn is_cmd_style(text: &str) -> bool {
    text.contains("^\"") && text.lines().any(|l| l.trim_end().ends_with(" ^"))
}

/// A shell variable name: `[A-Za-z_][A-Za-z0-9_]*` — always a valid
/// postui variable name too.
fn is_name(s: &str) -> bool {
    let mut chars = s.chars();
    chars.next().is_some_and(|c| c == '_' || c.is_ascii_alphabetic())
        && chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
}

struct Splitter<'a> {
    chars: Peekable<Chars<'a>>,
    commands: Vec<ShellCommand>,
    current: ShellCommand,
    word: String,
    /// A word has begun, even an empty quoted one (`''`).
    in_word: bool,
    /// Past a `|` or a redirect: words go to the tail.
    in_tail: bool,
    tail_words: Vec<String>,
}

impl Splitter<'_> {
    fn run(mut self) -> Result<Vec<ShellCommand>, ShellError> {
        while let Some(c) = self.chars.next() {
            match c {
                ' ' | '\t' => self.end_word(),
                '\n' | ';' => self.end_command(),
                '&' => {
                    // `&&` and a lone `&` (background) both end the command.
                    if self.chars.peek() == Some(&'&') {
                        self.chars.next();
                    }
                    self.end_command();
                }
                '|' => {
                    if self.chars.peek() == Some(&'|') {
                        self.chars.next();
                        self.end_command();
                    } else {
                        self.end_word();
                        self.start_tail("|".to_string());
                    }
                }
                '>' | '<' => self.redirect(c),
                '#' if !self.in_word => self.skip_comment(),
                '\\' => match self.chars.next() {
                    Some('\n') => {} // a continuation
                    Some(n) => self.push(n),
                    None => self.push('\\'),
                },
                '\'' => {
                    self.in_word = true;
                    self.single_quoted()?;
                }
                '"' => {
                    self.in_word = true;
                    self.double_quoted()?;
                }
                '`' => {
                    self.in_word = true;
                    self.backtick()?;
                }
                '$' => self.dollar(false)?,
                c => self.push(c),
            }
        }
        self.end_command();
        Ok(self.commands)
    }

    fn unclosed(&self) -> ShellError {
        ShellError::Unclosed {
            command: self.commands.len() + 1,
        }
    }

    fn push(&mut self, c: char) {
        self.in_word = true;
        self.word.push(c);
    }

    fn end_word(&mut self) {
        if !self.in_word {
            return;
        }
        let w = std::mem::take(&mut self.word);
        if self.in_tail {
            self.tail_words.push(w);
        } else {
            self.current.words.push(w);
        }
        self.in_word = false;
    }

    fn start_tail(&mut self, op: String) {
        self.in_tail = true;
        self.tail_words.push(op);
    }

    fn end_command(&mut self) {
        self.end_word();
        if self.in_tail {
            self.current.tail = Some(self.tail_words.join(" "));
            self.tail_words.clear();
            self.in_tail = false;
        }
        let mut cmd = std::mem::take(&mut self.current);
        if matches!(cmd.words.first().map(String::as_str), Some("$" | "%")) {
            cmd.words.remove(0);
        }
        if !cmd.words.is_empty() {
            self.commands.push(cmd);
        }
    }

    /// `>`, `>>`, `<`, `2>`, `2>&1`, `&>`…: the curl part ends here and
    /// the rest of the command is tail.
    fn redirect(&mut self, first: char) {
        let mut op = String::new();
        // A bare fd number glued to the operator (`2>`) belongs to it.
        if self.in_word && !self.word.is_empty() && self.word.chars().all(|d| d.is_ascii_digit()) {
            op = std::mem::take(&mut self.word);
            self.in_word = false;
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

    fn skip_comment(&mut self) {
        while let Some(&n) = self.chars.peek() {
            if n == '\n' {
                break;
            }
            self.chars.next();
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
    /// bytes, and the bytes decode as UTF-8 once the quote closes.
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
                            match u32::from_str_radix(&digits, 16).ok().and_then(char::from_u32) {
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
        self.word.push_str(&String::from_utf8_lossy(&bytes));
        Ok(())
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
                let mut depth = 1usize;
                let mut inner = String::new();
                loop {
                    match self.chars.next() {
                        None => return Err(self.unclosed()),
                        Some('(') => {
                            depth += 1;
                            inner.push('(');
                        }
                        Some(')') => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                            inner.push(')');
                        }
                        Some(c) => inner.push(c),
                    }
                }
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
            // `$1`, `$?`, `$$`, `$@`, a lone `$`: literal.
            _ => {
                self.word.push('$');
                Ok(())
            }
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
        assert_eq!(words("curl \\\n  -X POST \\\r\n  https://x"), ["curl", "-X", "POST", "https://x"]);
    }

    #[test]
    fn double_quotes_keep_only_their_own_escapes() {
        assert_eq!(words(r#"curl "a\"b\\c\$d\`e\q""#), ["curl", "a\"b\\c$d`e\\q"]);
    }

    #[test]
    fn ansi_c_escapes_decode() {
        assert_eq!(
            words(r"curl $'a\nb\'c\x41é\101\\!'"),
            ["curl", "a\nb'cAéA\\!"]
        );
        // Bytes from `\x` decode as UTF-8 together.
        assert_eq!(words(r"curl $'\xc3\xa9'"), ["curl", "é"]);
    }

    #[test]
    fn line_breaks_semicolons_and_logic_operators_end_commands() {
        let cmds = split("curl a\ncurl b; curl c && curl d || curl e & curl f").unwrap();
        let firsts: Vec<&str> = cmds.iter().map(|c| c.words[1].as_str()).collect();
        assert_eq!(firsts, ["a", "b", "c", "d", "e", "f"]);
    }

    #[test]
    fn blank_lines_and_comments_are_not_commands() {
        let cmds = split("# fetch it\n\ncurl https://x/#frag # trailing\n").unwrap();
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].words, ["curl", "https://x/#frag"]);
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
        assert_eq!(cmds[0].words, ["curl", "-H", "Authorization: Bearer {{TOKEN}}", "{{BASE}}/x", "{{TOKEN}}"]);
        assert_eq!(cmds[0].converted, ["TOKEN", "BASE"]);
    }

    #[test]
    fn specials_and_single_quotes_stay_literal() {
        let cmds = split(r#"curl "$1 $? $$ ${X:-y} $" '$NOT' $'$ALSO' -u 'me:p@$$w0rd'"#).unwrap();
        assert_eq!(cmds[0].words, ["curl", "$1 $? $$ ${X:-y} $", "$NOT", "$ALSO", "-u", "me:p@$$w0rd"]);
        assert!(cmds[0].converted.is_empty());
    }

    #[test]
    fn command_substitution_stays_literal_and_is_noted() {
        let cmds = split(r#"curl -H "X-Date: $(date +%s)" `id -u`"#).unwrap();
        assert_eq!(cmds[0].words, ["curl", "-H", "X-Date: $(date +%s)", "`id -u`"]);
        assert_eq!(cmds[0].substitutions, ["$(date +%s)", "`id -u`"]);
    }

    #[test]
    fn an_unclosed_quote_names_its_command() {
        assert_eq!(split("curl a\ncurl 'b"), Err(ShellError::Unclosed { command: 2 }));
        assert_eq!(split("curl \"a"), Err(ShellError::Unclosed { command: 1 }));
        assert_eq!(split("curl $'a"), Err(ShellError::Unclosed { command: 1 }));
    }

    #[test]
    fn windows_cmd_quoting_is_refused() {
        assert_eq!(split("curl ^\"https://x^\" ^\n  -H ^\"A: b^\""), Err(ShellError::CmdStyle));
        // A bash body that merely contains `^"` still parses.
        assert!(split("curl -d '^\"x' https://x").is_ok());
    }
}

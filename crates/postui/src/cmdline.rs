//! The `:` line's history (key list §8 `X.history`; plan M5b Task 5):
//! Vim's command-line history, kept across launches in `ui.toml` as Vim
//! keeps it in viminfo (the user's decision, 2026-10-09). `Config` reads
//! and writes the file; this is what is in it.

/// How many lines the history keeps: Vim's `'history'` as `defaults.vim`
/// sets it (plan M5b Deviation 15; the compiled default is 50).
pub const MAX_LINES: usize = 200;

/// The lines the `:` line closed with, oldest first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CmdlineHistory {
    lines: Vec<String>,
    /// False when `ui.toml`, or its `[history]`, did not read at startup:
    /// it was reported then and is never written over (the user's rule,
    /// parse errors never default).
    writable: bool,
}

impl Default for CmdlineHistory {
    fn default() -> Self {
        Self { lines: Vec::new(), writable: true }
    }
}

impl CmdlineHistory {
    /// An empty history that is never written: `ui.toml` could not be read
    /// at all, so what is on disk is unknown and a quit save must not
    /// replace it.
    pub fn unreadable() -> Self {
        Self { lines: Vec::new(), writable: false }
    }

    /// The history in `ui.toml`'s text: `[history]`'s `cmdline`, a list of
    /// strings, oldest first. No table or no key is no history. A table or
    /// key that does not read gives an empty history that is never written,
    /// and the warning to show. A file that does not parse at all is never
    /// written either; `UsageStore::parse` words its warning, so none comes
    /// from here.
    pub fn parse(text: &str) -> (Self, Option<String>) {
        let Ok(value) = toml::from_str::<toml::Value>(text) else {
            return (Self { lines: Vec::new(), writable: false }, None);
        };
        let Some(history) = value.get("history") else {
            return (Self::default(), None);
        };
        let lines = history.as_table().and_then(|t| match t.get("cmdline") {
            None => Some(Vec::new()),
            Some(v) => v.as_array().and_then(|a| a.iter().map(|l| l.as_str().map(str::to_string)).collect()),
        });
        match lines {
            Some(mut lines) => {
                let extra = lines.len().saturating_sub(MAX_LINES);
                lines.drain(..extra);
                (Self { lines, writable: true }, None)
            }
            None => (
                Self { lines: Vec::new(), writable: false },
                Some("[history] cmdline is not a list of strings; the command-line history will not be saved until it is fixed".into()),
            ),
        }
    }

    /// Writes the lines into `doc`'s `[history]` table as `cmdline`, every
    /// other key left alone, the table made when there is none. A `history`
    /// that is not a table is the user's and is left as it is (the user's
    /// rule, parse errors never default). The I/O is
    /// `Config::save_cmdline_history`'s.
    pub fn write_into(&self, doc: &mut toml_edit::DocumentMut) {
        let mut lines = toml_edit::Array::new();
        for line in &self.lines {
            lines.push(line.as_str());
        }
        let history = doc
            .entry("history")
            .or_insert(toml_edit::Item::Table(toml_edit::Table::new()));
        if let Some(table) = history.as_table_like_mut() {
            table.insert("cmdline", toml_edit::value(lines));
        }
    }

    /// A line the `:` line closed with, Enter or Esc (Vim's
    /// `add_to_history()`; probes H6, H7, H15, H19): kept as typed, an
    /// equal line moved to the newest place, the oldest dropped past
    /// [`MAX_LINES`]. An empty line is not kept (H5).
    pub fn add(&mut self, line: &str) {
        if line.is_empty() {
            return;
        }
        self.lines.retain(|l| l != line);
        self.lines.push(line.to_string());
        let extra = self.lines.len().saturating_sub(MAX_LINES);
        self.lines.drain(..extra);
    }

    /// Oldest first.
    pub fn lines(&self) -> &[String] {
        &self.lines
    }

    /// Whether quitting may write it (see the field).
    pub fn writable(&self) -> bool {
        self.writable
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Probes H5, H6, H7, H19: a line is kept as typed, an equal one moves
    /// to the newest place, an empty one is not kept.
    #[test]
    fn lines_are_kept_as_vim_keeps_them() {
        let mut h = CmdlineHistory::default();
        for line in ["echo 1", "let g:a=1", "echo 22", "", "echo 1", "  echo 1"] {
            h.add(line);
        }
        assert_eq!(h.lines(), ["let g:a=1", "echo 22", "echo 1", "  echo 1"]);
    }

    /// Deviation 15: the newest `MAX_LINES` lines are kept.
    #[test]
    fn the_oldest_lines_go_past_the_cap() {
        let mut h = CmdlineHistory::default();
        for i in 0..MAX_LINES + 5 {
            h.add(&i.to_string());
        }
        assert_eq!(h.lines().len(), MAX_LINES);
        assert_eq!(h.lines()[0], "5");
    }

    /// Deviation 16: `[history]`'s `cmdline`, every other key and comment
    /// left as it was.
    #[test]
    fn it_reads_and_writes_its_key_only() {
        let text = "# mine\n[palette.usage]\n\n[history]\ncmdline = [\"w\", \"q\"]\nother = 1\n";
        let (mut h, warning) = CmdlineHistory::parse(text);
        assert_eq!(warning, None);
        assert_eq!(h.lines(), ["w", "q"]);
        h.add("send");
        let mut doc: toml_edit::DocumentMut = text.parse().unwrap();
        h.write_into(&mut doc);
        let out = doc.to_string();
        assert!(out.starts_with("# mine\n"), "{out}");
        assert!(out.contains("other = 1"), "{out}");
        assert_eq!(CmdlineHistory::parse(&out).0.lines(), ["w", "q", "send"]);
        assert_eq!(CmdlineHistory::parse("").0, CmdlineHistory::default(), "no file, no history");
        assert_eq!(CmdlineHistory::parse("[palette]\n").0, CmdlineHistory::default(), "no table, no history");
    }

    /// Review Focus 4 (the user's rule): a `[history]` that does not read
    /// is reported and never written.
    #[test]
    fn a_history_that_does_not_read_is_not_writable() {
        for text in ["[history]\ncmdline = 5\n", "[history]\ncmdline = [1, \"w\"]\n", "history = 3\n"] {
            let (h, warning) = CmdlineHistory::parse(text);
            assert!(warning.is_some(), "{text}");
            assert!(!h.writable(), "{text}");
            assert!(h.lines().is_empty(), "{text}");
        }
    }

    /// Amendment A5.3: a file that does not parse at all is not writable
    /// either. `UsageStore::parse` words the warning, so none comes from
    /// here.
    #[test]
    fn a_file_that_does_not_parse_is_not_writable() {
        let (h, warning) = CmdlineHistory::parse("not = [toml");
        assert_eq!(warning, None);
        assert!(!h.writable());
        assert!(h.lines().is_empty());
    }

    /// Amendment A5.2: a `ui.toml` with no `[history]` gets a real table,
    /// with its own header, not an inline `history = { … }`.
    #[test]
    fn a_fresh_file_gets_a_history_table() {
        let mut h = CmdlineHistory::default();
        h.add("w");
        let mut doc: toml_edit::DocumentMut = "[palette.usage]\n".parse().unwrap();
        h.write_into(&mut doc);
        let out = doc.to_string();
        assert!(out.contains("\n[history]\ncmdline = [\"w\"]\n"), "{out}");
        assert_eq!(CmdlineHistory::parse(&out).0.lines(), ["w"]);
        // An inline table the user wrote is a table too: it keeps its form.
        let mut doc: toml_edit::DocumentMut = "history = { other = 1 }\n".parse().unwrap();
        h.write_into(&mut doc);
        let out = doc.to_string();
        assert!(out.starts_with("history = { other = 1"), "{out}");
        assert_eq!(CmdlineHistory::parse(&out).0.lines(), ["w"]);
    }

    /// Amendment A5.2: a `history` that is not a table is never written
    /// over, and writing into it never panics.
    #[test]
    fn a_history_that_is_not_a_table_is_left_byte_for_byte() {
        let mut h = CmdlineHistory::default();
        h.add("w");
        for text in ["history = 3\n", "# mine\nhistory = [\"x\"]\n"] {
            let mut doc: toml_edit::DocumentMut = text.parse().unwrap();
            h.write_into(&mut doc);
            assert_eq!(doc.to_string(), text);
        }
    }
}

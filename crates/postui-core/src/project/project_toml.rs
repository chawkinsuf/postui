//! The project's own settings in `project.toml`: its display `name` and
//! its `[default_headers]` (spec 2026-11-15 project settings). Each write
//! is one journaled entry, as the Manage screen's other `project.toml`
//! edits are.

use super::{Error, Project};
use crate::journal::EntryMeta;
use crate::model::Entry;
use indexmap::IndexMap;

/// The top-level table the default headers live in.
const DEFAULT_HEADERS: &str = "default_headers";

impl Project {
    /// Sets the project's display name: trimmed, and `None` or blank
    /// removes `name`, so the project shows its folder name again. A name
    /// the file already holds writes nothing. A new name keeps the old
    /// one's comments.
    pub fn set_project_name(&mut self, name: Option<&str>) -> Result<(), Error> {
        let name = name.map(str::trim).filter(|n| !n.is_empty()).map(str::to_string);
        self.transaction("set project name", EntryMeta::default(), |p| {
            p.edit_project_toml(|doc| match &name {
                Some(n) => {
                    doc["name"] = with_decor_of(doc.get("name"), toml_edit::value(n.as_str()));
                }
                None => {
                    doc.remove("name");
                }
            })
        })
    }

    /// Writes `headers` as the whole `[default_headers]` table, in its
    /// order: a header the file holds that `headers` lacks goes, even one
    /// added by hand since the last read (spec 2026-11-15 R17, the user's
    /// ruling; undo puts it back). Every header is written as a value on
    /// its own line, so the file holds them in `headers`' order: a value the
    /// table already holds as it is stays as written, a changed one under a
    /// name the table holds keeps that key's comments, and any other form
    /// (a `[default_headers.X]` sub-table, a dotted `X.value`), which would
    /// print in another place, is written in the plain form. An empty map
    /// removes the table. An inline `default_headers = { … }` is written as
    /// a `[default_headers]` table.
    pub fn set_default_headers(&mut self, headers: &IndexMap<String, Entry>) -> Result<(), Error> {
        self.transaction("set default headers", EntryMeta::default(), |p| {
            p.edit_project_toml(|doc| {
                if headers.is_empty() {
                    doc.remove(DEFAULT_HEADERS);
                    return;
                }
                let old = doc.get(DEFAULT_HEADERS).and_then(toml_edit::Item::as_table).cloned();
                // The old table's own decor and place in the file stay.
                let mut table = old.clone().unwrap_or_default();
                table.clear();
                table.set_implicit(false);
                table.set_dotted(false);
                for (name, entry) in headers {
                    let held = old.as_ref().and_then(|t| t.get_key_value(name));
                    match held.filter(|(_, item)| item.is_value()) {
                        Some((key, item)) if entry_of(item).as_ref() == Some(entry) => {
                            table.insert_formatted(key, item.clone());
                        }
                        Some((key, item)) => {
                            table.insert_formatted(key, with_decor_of(Some(item), entry.to_item()));
                        }
                        None => {
                            table.insert(name, entry.to_item());
                        }
                    }
                }
                if old.is_none() {
                    // Not a table (an inline one): its key goes too, with
                    // its inline spacing.
                    doc.remove(DEFAULT_HEADERS);
                }
                doc[DEFAULT_HEADERS] = toml_edit::Item::Table(table);
            })
        })
    }
}

/// The entry a value of `[default_headers]` holds, read by `Entry`'s own
/// `Deserialize`, as the file is read; `None` for one it doesn't read.
fn entry_of(item: &toml_edit::Item) -> Option<Entry> {
    use serde::Deserialize;
    let mut value = item.as_value()?.clone();
    value.decor_mut().clear();
    let raw = value.to_string();
    Entry::deserialize(toml::de::ValueDeserializer::parse(&raw).ok()?).ok()
}

/// `new` with `old`'s value decor (the comment after it on its line), when
/// both are values.
fn with_decor_of(old: Option<&toml_edit::Item>, mut new: toml_edit::Item) -> toml_edit::Item {
    if let (Some(old), Some(value)) = (old.and_then(toml_edit::Item::as_value), new.as_value_mut())
    {
        *value.decor_mut() = old.decor().clone();
    }
    new
}

#[cfg(test)]
mod tests {
    use super::super::tests::{fixture, read};
    use super::*;

    fn on(v: &str) -> Entry {
        Entry { value: v.to_string(), enabled: true }
    }

    fn off(v: &str) -> Entry {
        Entry { value: v.to_string(), enabled: false }
    }

    fn reopen(dir: &tempfile::TempDir) -> Project {
        Project::open(dir.path().to_path_buf()).unwrap().0
    }

    #[test]
    fn the_name_is_trimmed_written_and_undone() {
        let (dir, mut p) = fixture();
        p.set_project_name(Some("  Orders API ")).unwrap();
        assert_eq!(p.display_name(), "Orders API");
        assert!(read(&dir, "project.toml").unwrap().contains("name = \"Orders API\""));
        assert_eq!(p.journal_len(), 1);
        p.undo().unwrap();
        assert_eq!(p.display_name(), "Demo");
        assert_eq!(reopen(&dir).display_name(), "Demo");
    }

    #[test]
    fn a_blank_name_removes_the_key_and_the_folder_name_shows() {
        let (dir, mut p) = fixture();
        let folder = dir.path().file_name().unwrap().to_string_lossy().to_string();
        p.set_project_name(Some("   ")).unwrap();
        assert_eq!(p.meta().name, None);
        assert_eq!(p.display_name(), folder);
        assert!(!read(&dir, "project.toml").unwrap().contains("name = \"Demo\""));
        p.set_project_name(Some("Back")).unwrap();
        p.set_project_name(None).unwrap();
        assert_eq!(p.display_name(), folder);
        assert_eq!(p.journal_len(), 3);
    }

    #[test]
    fn the_name_already_there_writes_nothing() {
        let (dir, mut p) = fixture();
        let before = read(&dir, "project.toml").unwrap();
        p.set_project_name(Some("Demo")).unwrap();
        assert_eq!(read(&dir, "project.toml").unwrap(), before);
        assert_eq!(p.journal_len(), 0);
    }

    #[test]
    fn default_headers_write_plain_and_inline_in_order() {
        let (dir, mut p) = fixture();
        let mut h = IndexMap::new();
        h.insert("X-Client".to_string(), on("postui"));
        h.insert("X-Off".to_string(), off("x"));
        h.insert("Accept".to_string(), on("{{type}}"));
        p.set_default_headers(&h).unwrap();
        assert_eq!(p.meta().default_headers, h);
        assert_eq!(reopen(&dir).meta().default_headers, h);
        let text = read(&dir, "project.toml").unwrap();
        assert!(text.contains("X-Client = \"postui\""), "{text}");
        assert!(text.contains("X-Off = { value = \"x\", enabled = false }"), "{text}");
        assert!(text.contains("name = \"Demo\""), "the rest of the file stays: {text}");
        assert_eq!(p.journal_len(), 1);
        p.undo().unwrap();
        assert!(p.meta().default_headers.is_empty());
        assert!(!read(&dir, "project.toml").unwrap().contains("default_headers"));
    }

    #[test]
    fn an_unchanged_header_keeps_its_comment_and_a_change_rewrites_the_rest() {
        let (dir, _p) = fixture();
        let text = read(&dir, "project.toml").unwrap();
        std::fs::write(
            dir.path().join("project.toml"),
            format!(
                "{text}\n[default_headers]\n# who calls\nX-Client = \"postui\"\nX-Trace = \"1\"\n"
            ),
        )
        .unwrap();
        let mut p = reopen(&dir);
        let mut h = p.meta().default_headers.clone();
        h.get_mut("X-Trace").unwrap().enabled = false;
        p.set_default_headers(&h).unwrap();
        let text = read(&dir, "project.toml").unwrap();
        assert!(text.contains("# who calls\nX-Client = \"postui\""), "{text}");
        assert!(text.contains("X-Trace = { value = \"1\", enabled = false }"), "{text}");
        assert_eq!(p.meta().default_headers, h);
    }

    #[test]
    fn a_changed_header_keeps_its_comments() {
        let (dir, _p) = fixture();
        let text = read(&dir, "project.toml").unwrap();
        std::fs::write(
            dir.path().join("project.toml"),
            format!(
                "{text}\n[default_headers]\n# who calls\nX-Client = \"postui\"  # set by ops\n"
            ),
        )
        .unwrap();
        let mut p = reopen(&dir);
        let mut h = p.meta().default_headers.clone();
        h.get_mut("X-Client").unwrap().enabled = false;
        p.set_default_headers(&h).unwrap();
        let text = read(&dir, "project.toml").unwrap();
        assert!(
            text.contains(
                "# who calls\nX-Client = { value = \"postui\", enabled = false }  # set by ops\n"
            ),
            "{text}"
        );
        h.get_mut("X-Client").unwrap().value = "cli".to_string();
        h.get_mut("X-Client").unwrap().enabled = true;
        p.set_default_headers(&h).unwrap();
        let text = read(&dir, "project.toml").unwrap();
        assert!(text.contains("# who calls\nX-Client = \"cli\"  # set by ops\n"), "{text}");
        assert_eq!(reopen(&dir).meta().default_headers, h);
    }

    #[test]
    fn a_new_name_keeps_the_old_ones_comments() {
        let (dir, _p) = fixture();
        let text = read(&dir, "project.toml").unwrap();
        let commented =
            text.replace("name = \"Demo\"", "# the chip\nname = \"Demo\"  # shown in the header");
        assert_ne!(commented, text);
        std::fs::write(dir.path().join("project.toml"), commented).unwrap();
        let mut p = reopen(&dir);
        p.set_project_name(Some("Orders API")).unwrap();
        let text = read(&dir, "project.toml").unwrap();
        assert!(
            text.contains("# the chip\nname = \"Orders API\"  # shown in the header"),
            "{text}"
        );
    }

    #[test]
    fn an_inline_table_is_written_as_a_table_with_every_header() {
        let (dir, _p) = fixture();
        let text = read(&dir, "project.toml").unwrap();
        let inline = format!("default_headers = {{ a = \"1\", b = \"2\" }}  # note\n{text}");
        std::fs::write(dir.path().join("project.toml"), inline).unwrap();
        let mut p = reopen(&dir);
        let mut h = p.meta().default_headers.clone();
        h.get_mut("b").unwrap().enabled = false;
        p.set_default_headers(&h).unwrap();
        let text = read(&dir, "project.toml").unwrap();
        assert!(text.contains("[default_headers]\na = \"1\"\n"), "{text}");
        assert_eq!(reopen(&dir).meta().default_headers, h);
        assert_eq!(reopen(&dir).display_name(), "Demo");
    }

    #[test]
    fn a_renamed_header_keeps_its_place() {
        let (_dir, mut p) = fixture();
        let mut h = IndexMap::new();
        h.insert("a".to_string(), on("1"));
        h.insert("b".to_string(), on("2"));
        h.insert("c".to_string(), on("3"));
        p.set_default_headers(&h).unwrap();
        h.shift_remove("b");
        h.shift_insert(1, "B".to_string(), on("2"));
        p.set_default_headers(&h).unwrap();
        let keys: Vec<&str> = p.meta().default_headers.keys().map(String::as_str).collect();
        assert_eq!(keys, ["a", "B", "c"]);
    }

    /// A header written as a `[default_headers.X]` sub-table or a dotted
    /// `X.value` would print after the plain values, or keep a form the
    /// tab didn't write: each is written as a plain value, so the file
    /// holds the headers in the order written, and reads back in it.
    #[test]
    fn every_form_is_written_in_the_order_given() {
        let (dir, _p) = fixture();
        let text = read(&dir, "project.toml").unwrap();
        std::fs::write(
            dir.path().join("project.toml"),
            format!(
                "{text}\n[default_headers]\na = \"1\"\nd.value = \"4\"\nc = \"3\"\n\n\
                 [default_headers.b]\nvalue = \"2\"\nenabled = false\n"
            ),
        )
        .unwrap();
        let mut p = reopen(&dir);
        let read_order: Vec<&str> = p.meta().default_headers.keys().map(String::as_str).collect();
        assert_eq!(read_order, ["a", "d", "c", "b"]);
        // The tab adds `e` last.
        let mut h = p.meta().default_headers.clone();
        h.insert("e".to_string(), on("5"));
        p.set_default_headers(&h).unwrap();
        for p in [&p, &reopen(&dir)] {
            let keys: Vec<&str> = p.meta().default_headers.keys().map(String::as_str).collect();
            assert_eq!(keys, ["a", "d", "c", "b", "e"]);
            assert_eq!(p.meta().default_headers, h);
        }
        let text = read(&dir, "project.toml").unwrap();
        assert!(
            text.contains(
                "[default_headers]\na = \"1\"\nd = \"4\"\nc = \"3\"\n\
                 b = { value = \"2\", enabled = false }\ne = \"5\"\n"
            ),
            "{text}"
        );
    }

    /// Headers held only as sub-tables, under no `[default_headers]` line
    /// of their own, are written under one.
    #[test]
    fn sub_tables_alone_are_written_under_a_table() {
        let (dir, _p) = fixture();
        let text = read(&dir, "project.toml").unwrap();
        std::fs::write(
            dir.path().join("project.toml"),
            format!("{text}\n[default_headers.a]\nvalue = \"1\"\n"),
        )
        .unwrap();
        let mut p = reopen(&dir);
        let mut h = p.meta().default_headers.clone();
        h.insert("b".to_string(), on("2"));
        p.set_default_headers(&h).unwrap();
        let text = read(&dir, "project.toml").unwrap();
        assert!(text.contains("[default_headers]\na = \"1\"\nb = \"2\"\n"), "{text}");
        assert_eq!(reopen(&dir).meta().default_headers, h);
    }

    /// An entry is compared by `Entry`'s own reading of it: a value the
    /// tab didn't change keeps the form it was written in.
    #[test]
    fn an_unchanged_value_keeps_its_written_form() {
        let (dir, _p) = fixture();
        let text = read(&dir, "project.toml").unwrap();
        std::fs::write(
            dir.path().join("project.toml"),
            format!(
                "{text}\n[default_headers]\na = {{ value = 'x', enabled = true }}  # kept\nb = \"2\"\n"
            ),
        )
        .unwrap();
        let mut p = reopen(&dir);
        let mut h = p.meta().default_headers.clone();
        h.get_mut("b").unwrap().enabled = false;
        p.set_default_headers(&h).unwrap();
        let text = read(&dir, "project.toml").unwrap();
        assert!(text.contains("a = { value = 'x', enabled = true }  # kept\n"), "{text}");
        assert_eq!(reopen(&dir).meta().default_headers, h);
    }

    #[test]
    fn emptying_the_headers_removes_the_table_and_no_change_records_nothing() {
        let (dir, mut p) = fixture();
        let mut h = IndexMap::new();
        h.insert("a".to_string(), on("1"));
        p.set_default_headers(&h).unwrap();
        p.set_default_headers(&h).unwrap();
        assert_eq!(p.journal_len(), 1, "the same headers again write nothing");
        p.set_default_headers(&IndexMap::new()).unwrap();
        assert!(!read(&dir, "project.toml").unwrap().contains("default_headers"));
        assert!(p.meta().default_headers.is_empty());
        assert_eq!(p.journal_len(), 2);
    }
}

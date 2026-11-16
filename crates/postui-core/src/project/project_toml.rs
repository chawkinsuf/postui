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
    /// ruling; undo puts it back). An empty map removes the table.
    ///
    /// The table keeps its form (a `[default_headers]` table, an inline
    /// `default_headers = { … }`, dotted `default_headers.X` keys) and its
    /// comments. A header whose value `headers` doesn't change stays as
    /// written, in its form and with its comments, except a
    /// `[default_headers.X]` sub-table that can't stay where it is (see
    /// `kept_sub_tables`). A changed header, and such a sub-table, is
    /// written in the plain form and keeps its comments.
    pub fn set_default_headers(&mut self, headers: &IndexMap<String, Entry>) -> Result<(), Error> {
        self.transaction("set default headers", EntryMeta::default(), |p| {
            p.edit_project_toml(|doc| write_default_headers(doc, headers))
        })
    }
}

/// `set_default_headers`' edit of the document.
fn write_default_headers(doc: &mut toml_edit::DocumentMut, headers: &IndexMap<String, Entry>) {
    use toml_edit::{Item, Value};
    if headers.is_empty() {
        doc.remove(DEFAULT_HEADERS);
        return;
    }
    match doc.get_mut(DEFAULT_HEADERS) {
        // Cleared in place, so its decor, place and form stay.
        Some(Item::Table(table)) => {
            let old = table.clone();
            let kept = kept_sub_tables(&old, headers);
            table.clear();
            for (i, (name, entry)) in headers.iter().enumerate() {
                let (key, item) = header_item(&old, name, entry, i >= kept);
                table.insert_formatted(&key, item);
            }
        }
        Some(Item::Value(Value::InlineTable(inline))) => {
            let old = inline.clone();
            inline.clear();
            for (name, entry) in headers {
                let (key, item) = header_item(&old, name, entry, true);
                if let Ok(value) = item.into_value() {
                    inline.insert_formatted(&key, value);
                }
            }
            // An inline table holds no comments; its spacing depends on
            // each value's place in it, so it is laid out afresh.
            inline.fmt();
        }
        _ => {
            let mut table = toml_edit::Table::new();
            for (name, entry) in headers {
                table.insert(name, entry.to_item());
            }
            doc[DEFAULT_HEADERS] = Item::Table(table);
        }
    }
}

/// Where the run of `[default_headers.X]` sub-tables kept as they are
/// starts in `headers`. A sub-table prints after the table's own values,
/// at its place in the file, so it stays only when it is unchanged, every
/// header after it stays such a sub-table further down the file, and,
/// when the table holds values, it lies below the table's own place;
/// otherwise the file would read the headers in another order (R10).
fn kept_sub_tables(old: &toml_edit::Table, headers: &IndexMap<String, Entry>) -> usize {
    let mut run: Vec<isize> = Vec::new();
    for (name, entry) in headers.iter().rev() {
        let pos = old
            .get(name)
            .and_then(toml_edit::Item::as_table)
            .filter(|t| !t.is_dotted())
            .and_then(toml_edit::Table::position)
            .filter(|_| old.get(name).and_then(entry_of).as_ref() == Some(entry));
        match pos {
            Some(pos) if run.last().is_none_or(|&next| pos < next) => run.push(pos),
            _ => break,
        }
    }
    if run.len() < headers.len() {
        run.retain(|&pos| old.position().is_some_and(|top| top < pos));
    }
    headers.len() - run.len()
}

/// The key and item `entry` is written as under `name`, given what `old`
/// holds there: the held one as it is when its value is unchanged (a
/// sub-table only when `sub_table_stays`); otherwise `entry`'s plain form,
/// with the held one's comments.
fn header_item(
    old: &dyn toml_edit::TableLike,
    name: &str,
    entry: &Entry,
    sub_table_stays: bool,
) -> (toml_edit::Key, toml_edit::Item) {
    use toml_edit::Item;
    let Some((key, held)) = old.get_key_value(name) else {
        return (toml_edit::Key::new(name), entry.to_item());
    };
    let stays = match held {
        Item::Table(table) if !table.is_dotted() => sub_table_stays,
        _ => true,
    };
    if stays && entry_of(held).as_ref() == Some(entry) {
        return (key.clone(), held.clone());
    }
    let mut new = entry.to_item();
    match held {
        Item::Table(table) => {
            // A sub-table or dotted keys: the comments above and after
            // its lines go above and after the plain line.
            let (above, after) = comments_of(table);
            let mut key = toml_edit::Key::new(name);
            key.leaf_decor_mut().set_prefix(above);
            if let (false, Some(value)) = (after.is_empty(), new.as_value_mut()) {
                value.decor_mut().set_suffix(format!(" {after}"));
            }
            (key, new)
        }
        _ => (key.clone(), with_decor_of(Some(held), new)),
    }
}

/// The comment lines above a sub-table's or dotted header's lines, and
/// the comments after them, joined.
fn comments_of(table: &toml_edit::Table) -> (String, String) {
    fn text(raw: Option<&toml_edit::RawString>) -> &str {
        raw.and_then(toml_edit::RawString::as_str).unwrap_or("")
    }
    let mut above = String::new();
    let mut after: Vec<String> = Vec::new();
    let mut take = |prefix: &str, suffix: &str| {
        for line in prefix.lines().map(str::trim).filter(|l| l.starts_with('#')) {
            above.push_str(line);
            above.push('\n');
        }
        let suffix = suffix.trim();
        if suffix.starts_with('#') {
            after.push(suffix.to_string());
        }
    };
    take(text(table.decor().prefix()), text(table.decor().suffix()));
    for (path, value) in table.get_values() {
        let key = path.last().map(|k| text(k.leaf_decor().prefix())).unwrap_or("");
        take(key, text(value.decor().suffix()));
    }
    (above, after.join(" "))
}

/// The entry a header of `[default_headers]` holds, in any form, read by
/// `Entry`'s own `Deserialize`, as the file is read; `None` for one it
/// doesn't read.
fn entry_of(item: &toml_edit::Item) -> Option<Entry> {
    use serde::Deserialize;
    let mut value = item.clone().into_value().ok()?;
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
    fn an_inline_table_stays_inline_with_its_comment() {
        let (dir, _p) = fixture();
        let text = read(&dir, "project.toml").unwrap();
        let inline = format!(
            "# sent with every request\ndefault_headers = {{ a = 'kept', b = \"2\" }}  # note\n{text}"
        );
        std::fs::write(dir.path().join("project.toml"), inline).unwrap();
        let mut p = reopen(&dir);
        let mut h = p.meta().default_headers.clone();
        h.get_mut("b").unwrap().enabled = false;
        h.insert("c".to_string(), on("3"));
        p.set_default_headers(&h).unwrap();
        let text = read(&dir, "project.toml").unwrap();
        assert!(
            text.starts_with(
                "# sent with every request\ndefault_headers = { a = 'kept', \
                 b = { value = \"2\", enabled = false }, c = \"3\" }  # note\n"
            ),
            "{text}"
        );
        assert_eq!(reopen(&dir).meta().default_headers, h);
        assert_eq!(reopen(&dir).display_name(), "Demo");
    }

    /// Dotted `default_headers.X` keys at the top of the file stay dotted.
    #[test]
    fn dotted_table_keys_stay_dotted() {
        let (dir, _p) = fixture();
        let text = read(&dir, "project.toml").unwrap();
        let dotted = format!("# who calls\ndefault_headers.a = \"1\"  # ops\n{text}");
        std::fs::write(dir.path().join("project.toml"), dotted).unwrap();
        let mut p = reopen(&dir);
        let mut h = p.meta().default_headers.clone();
        h.insert("b".to_string(), off("2"));
        p.set_default_headers(&h).unwrap();
        let text = read(&dir, "project.toml").unwrap();
        assert!(
            text.starts_with(
                "# who calls\ndefault_headers.a = \"1\"  # ops\n\
                 default_headers.b = { value = \"2\", enabled = false }\n"
            ),
            "{text}"
        );
        assert_eq!(reopen(&dir).meta().default_headers, h);
    }

    /// A change to one header leaves an untouched `[default_headers.X]`
    /// sub-table and an untouched dotted `X.value` as written, comments
    /// included.
    #[test]
    fn untouched_sub_tables_and_dotted_headers_stay_as_written() {
        let (dir, _p) = fixture();
        let text = read(&dir, "project.toml").unwrap();
        let tail = "[default_headers]\nX-Client = \"postui\"\n# per call\nX-Id.value = \"7\"  # id\n\n\
                    # staging only\n[default_headers.X-Trace]  # trace\nvalue = \"1\"\n";
        std::fs::write(dir.path().join("project.toml"), format!("{text}\n{tail}")).unwrap();
        let mut p = reopen(&dir);
        let mut h = p.meta().default_headers.clone();
        h.get_mut("X-Client").unwrap().enabled = false;
        p.set_default_headers(&h).unwrap();
        let text = read(&dir, "project.toml").unwrap();
        assert!(
            text.contains(
                "[default_headers]\nX-Client = { value = \"postui\", enabled = false }\n\
                 # per call\nX-Id.value = \"7\"  # id\n\n\
                 # staging only\n[default_headers.X-Trace]  # trace\nvalue = \"1\"\n"
            ),
            "{text}"
        );
        assert_eq!(reopen(&dir).meta().default_headers, h);
    }

    /// A sub-table that a header is now written after takes the plain
    /// form, so the file keeps the order, and keeps its comments.
    #[test]
    fn a_sub_table_written_plain_keeps_its_comments() {
        let (dir, _p) = fixture();
        let text = read(&dir, "project.toml").unwrap();
        let tail = "[default_headers]\na = \"1\"\n\n# staging only\n[default_headers.X-Trace]  # trace\n\
                    # the id\nvalue = \"1\"  # one\n";
        std::fs::write(dir.path().join("project.toml"), format!("{text}\n{tail}")).unwrap();
        let mut p = reopen(&dir);
        let mut h = p.meta().default_headers.clone();
        h.insert("b".to_string(), on("2"));
        p.set_default_headers(&h).unwrap();
        let text = read(&dir, "project.toml").unwrap();
        assert!(
            text.contains(
                "[default_headers]\na = \"1\"\n# staging only\n# the id\n\
                 X-Trace = \"1\" # trace # one\nb = \"2\"\n"
            ),
            "{text}"
        );
        assert!(!text.contains("[default_headers.X-Trace]"), "{text}");
        let keys: Vec<String> = reopen(&dir).meta().default_headers.keys().cloned().collect();
        assert_eq!(keys, ["a", "X-Trace", "b"]);
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

    /// A `[default_headers.X]` sub-table prints after the table's values:
    /// with a header added after it, it is written as a plain value, so
    /// the file holds the headers in the order written and reads back in
    /// it. A dotted `X.value` prints in its place and stays.
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
                "[default_headers]\na = \"1\"\nd.value = \"4\"\nc = \"3\"\n\
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

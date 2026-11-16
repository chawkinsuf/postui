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
    /// one's comments; a removed one leaves the comments above it
    /// (`remove_keeping_comments`).
    pub fn set_project_name(&mut self, name: Option<&str>) -> Result<(), Error> {
        let name = name.map(str::trim).filter(|n| !n.is_empty()).map(str::to_string);
        self.transaction("set project name", EntryMeta::default(), |p| {
            p.edit_project_toml(|doc| match &name {
                Some(n) => {
                    doc["name"] = with_decor_of(doc.get("name"), toml_edit::value(n.as_str()));
                }
                None => remove_keeping_comments(doc, "name"),
            })
        })
    }

    /// Writes `headers` as the whole `[default_headers]` table, in its
    /// order: a header the file holds that `headers` lacks goes, even one
    /// added by hand since the last read (spec 2026-11-15 R17, the user's
    /// ruling; undo puts it back). An empty map removes the table.
    ///
    /// One rule for the forms the app never writes (`mine`, spec §3): a
    /// header `headers` doesn't change stays as written, in its form and
    /// with its comments, except a `[default_headers.X]` sub-table. A
    /// changed or new header, and every sub-table, is written as a plain
    /// line in the table's body; a held header's comments stay above
    /// that line. So every header is a line of the table's body, and the
    /// file reads them back in `headers`' order (R10). The table keeps its
    /// form: a `[default_headers]` table, an inline `default_headers = {
    /// … }`, or dotted `default_headers.X` keys.
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
        remove_keeping_comments(doc, DEFAULT_HEADERS);
        return;
    }
    match doc.get_mut(DEFAULT_HEADERS) {
        // Cleared in place, so its decor, place and form stay.
        Some(Item::Table(table)) => {
            let old = table.clone();
            table.clear();
            for (name, entry) in headers {
                let (key, item) = header_item(&old, name, entry);
                table.insert_formatted(&key, item);
            }
        }
        Some(Item::Value(Value::InlineTable(inline))) => {
            let old = inline.clone();
            inline.clear();
            for (name, entry) in headers {
                let (key, item) = header_item(&old, name, entry);
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

/// The key and item `entry` is written as under `name`, given what `old`
/// holds there: the held item as it is when its value is unchanged and it
/// is not a sub-table; otherwise `entry`'s plain form. A held plain or
/// inline value lends the new line its key's and value's decor; a held
/// sub-table or dotted header puts each of its comments on a line of its
/// own above the new line.
fn header_item(
    old: &dyn toml_edit::TableLike,
    name: &str,
    entry: &Entry,
) -> (toml_edit::Key, toml_edit::Item) {
    use toml_edit::Item;
    let Some((key, held)) = old.get_key_value(name) else {
        return (toml_edit::Key::new(name), entry.to_item());
    };
    let sub_table = matches!(held, Item::Table(t) if !t.is_dotted());
    if !sub_table && entry_of(held).as_ref() == Some(entry) {
        return (key.clone(), held.clone());
    }
    match held {
        Item::Value(_) => (key.clone(), with_decor_of(Some(held), entry.to_item())),
        _ => {
            let mut new_key = toml_edit::Key::new(name);
            new_key.leaf_decor_mut().set_prefix(comment_lines(key, held));
            (new_key, entry.to_item())
        }
    }
}

/// Every comment on `key` and in `item`, each on a line of its own, in
/// the order they are written.
fn comment_lines(key: &toml_edit::Key, item: &toml_edit::Item) -> String {
    fn push(out: &mut String, decor: &toml_edit::Decor) {
        for raw in [decor.prefix(), decor.suffix()] {
            let text = raw.and_then(toml_edit::RawString::as_str).unwrap_or("");
            for line in text.lines().map(str::trim).filter(|l| l.starts_with('#')) {
                out.push_str(line);
                out.push('\n');
            }
        }
    }
    let mut out = String::new();
    push(&mut out, key.leaf_decor());
    if let toml_edit::Item::Table(table) = item {
        push(&mut out, table.decor());
        for (path, value) in table.get_values() {
            for k in path {
                push(&mut out, k.leaf_decor());
            }
            push(&mut out, value.decor());
        }
    }
    out
}

/// Removes `name` from the top level of `doc`. The comment lines above it
/// may be the file's own or describe what follows, so they stay: they
/// move above what prints next, the next top-level value, else the next
/// table, else the end of the file. Its own comments inside or after it
/// go with it.
fn remove_keeping_comments(doc: &mut toml_edit::DocumentMut, name: &str) {
    use toml_edit::Item;
    let Some(at) = doc.iter().position(|(k, _)| k == name) else { return };
    let Some((key, item)) = doc.remove_entry(name) else { return };
    let above = text_above(&key, &item);
    if !above.contains('#') {
        return;
    }
    // A value or dotted keys print in the top-level body, before every
    // table; a table prints at its place.
    let from = match &item {
        Item::Table(t) if !t.is_dotted() => t.position(),
        Item::ArrayOfTables(a) => a.get(0).and_then(toml_edit::Table::position),
        _ => {
            let next = doc.iter().skip(at).find(|(_, i)| in_body(i)).map(|(k, _)| k.to_string());
            if let Some(next) = next {
                prepend_above(doc.as_table_mut(), &next, &above);
                return;
            }
            None
        }
    };
    let mut next = None;
    next_table(doc.as_table(), from, &mut next);
    match next {
        Some(pos) => {
            table_at(doc.as_table_mut(), pos, &mut |t: &mut toml_edit::Table| {
                let old = t.decor().prefix().and_then(toml_edit::RawString::as_str).unwrap_or("");
                let new = format!("{above}{old}");
                t.decor_mut().set_prefix(new);
            });
        }
        None => {
            let old = doc.trailing().as_str().unwrap_or("").to_string();
            doc.set_trailing(format!("{above}{old}"));
        }
    }
}

/// Whether `item` prints in its table's body: a value, or dotted keys.
fn in_body(item: &toml_edit::Item) -> bool {
    match item {
        toml_edit::Item::Value(_) => true,
        toml_edit::Item::Table(t) => t.is_dotted(),
        _ => false,
    }
}

/// The text printed above `item`'s first line under `key`.
fn text_above(key: &toml_edit::Key, item: &toml_edit::Item) -> String {
    use toml_edit::Item;
    let raw = |d: &toml_edit::Decor| {
        d.prefix().and_then(toml_edit::RawString::as_str).unwrap_or("").to_string()
    };
    match item {
        // Dotted keys print their first line's key last, with its decor.
        Item::Table(t) if t.is_dotted() => {
            t.iter().next().and_then(|(k, i)| Some(text_above(t.key(k)?, i))).unwrap_or_default()
        }
        Item::Table(t) => raw(t.decor()),
        Item::ArrayOfTables(a) => a.get(0).map(|t| raw(t.decor())).unwrap_or_default(),
        _ => raw(key.leaf_decor()),
    }
}

/// Puts `text` before what prints above the body item `name` of `table`.
fn prepend_above(table: &mut toml_edit::Table, name: &str, text: &str) {
    use toml_edit::Item;
    let Some((mut key, item)) = table.get_key_value_mut(name) else { return };
    let decor = match item {
        Item::Table(t) => {
            let Some(first) = t.iter().next().map(|(k, _)| k.to_string()) else { return };
            return prepend_above(t, &first, text);
        }
        _ => key.leaf_decor_mut(),
    };
    let old = decor.prefix().and_then(toml_edit::RawString::as_str).unwrap_or("");
    let new = format!("{text}{old}");
    decor.set_prefix(new);
}

/// The place of the first table with a header line printed after place
/// `from` (after none: the first of all), into `best`.
fn next_table(table: &toml_edit::Table, from: Option<isize>, best: &mut Option<isize>) {
    for (_, item) in table.iter() {
        let tables: Vec<&toml_edit::Table> = match item {
            toml_edit::Item::Table(t) => vec![t],
            toml_edit::Item::ArrayOfTables(a) => a.iter().collect(),
            _ => continue,
        };
        for t in tables {
            let printed = !t.is_dotted() && !(t.is_implicit() && t.get_values().is_empty());
            if let Some(pos) = t.position().filter(|_| printed)
                && from.is_none_or(|f| pos > f)
                && best.is_none_or(|b| pos < b)
            {
                *best = Some(pos);
            }
            next_table(t, from, best);
        }
    }
}

/// Runs `f` on the table with a header line at place `pos`.
fn table_at(table: &mut toml_edit::Table, pos: isize, f: &mut dyn FnMut(&mut toml_edit::Table)) {
    for (_, item) in table.iter_mut() {
        let tables: Vec<&mut toml_edit::Table> = match item {
            toml_edit::Item::Table(t) => vec![t],
            toml_edit::Item::ArrayOfTables(a) => a.iter_mut().collect(),
            _ => continue,
        };
        for t in tables {
            if t.position() == Some(pos) && !t.is_dotted() {
                f(t);
                return;
            }
            table_at(t, pos, f);
        }
    }
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

    /// A change to one header leaves an untouched dotted `X.value` as
    /// written, comments included; a `[default_headers.X]` sub-table is
    /// written as a plain line, its comments each on a line above it.
    #[test]
    fn untouched_dotted_headers_stay_and_sub_tables_go_plain() {
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
            text.ends_with(
                "[default_headers]\nX-Client = { value = \"postui\", enabled = false }\n\
                 # per call\nX-Id.value = \"7\"  # id\n\
                 # staging only\n# trace\nX-Trace = \"1\"\n"
            ),
            "{text}"
        );
        assert_eq!(reopen(&dir).meta().default_headers, h);
    }

    /// A sub-table written plain keeps every comment, each on its own
    /// line above it, and a later write leaves them as they are.
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
        let written = "[default_headers]\na = \"1\"\n# staging only\n# trace\n# the id\n# one\n\
                       X-Trace = \"1\"\nb = \"2\"\n";
        let text = read(&dir, "project.toml").unwrap();
        assert!(text.ends_with(written), "{text}");
        h.get_mut("b").unwrap().enabled = false;
        p.set_default_headers(&h).unwrap();
        let text = read(&dir, "project.toml").unwrap();
        let again = written.replace("b = \"2\"", "b = { value = \"2\", enabled = false }");
        assert!(text.ends_with(&again), "{text}");
        let keys: Vec<String> = reopen(&dir).meta().default_headers.keys().cloned().collect();
        assert_eq!(keys, ["a", "X-Trace", "b"]);
    }

    /// A changed dotted header is written plain under the comments that
    /// were above it and after it.
    #[test]
    fn a_changed_dotted_header_keeps_its_comments() {
        let (dir, _p) = fixture();
        let text = read(&dir, "project.toml").unwrap();
        let tail = "[default_headers]\n# per call\nX-Id.value = \"7\"  # id\n";
        std::fs::write(dir.path().join("project.toml"), format!("{text}\n{tail}")).unwrap();
        let mut p = reopen(&dir);
        let mut h = p.meta().default_headers.clone();
        h.get_mut("X-Id").unwrap().value = "8".to_string();
        p.set_default_headers(&h).unwrap();
        let text = read(&dir, "project.toml").unwrap();
        assert!(text.ends_with("[default_headers]\n# per call\n# id\nX-Id = \"8\"\n"), "{text}");
        assert_eq!(reopen(&dir).meta().default_headers, h);
    }

    /// Whatever forms the file holds the headers in, a write reads back
    /// in the order written: sub-tables under no `[default_headers]` line,
    /// with other tables between them, and a sub-table under dotted keys.
    #[test]
    fn the_file_reads_back_in_the_order_written() {
        let implicit = "name = \"Demo\"\nspaces = [\"main\", \"auth\"]\n\n\
                        [default_headers.b]\nvalue = \"2\"\n\n[space.auth]\nname = \"Auth\"\n\n\
                        [default_headers.a]\nvalue = \"1\"\nenabled = false\n\n\
                        [environment.dev]\nname = \"Dev\"\n";
        let dotted = "default_headers.b = \"2\"\nname = \"Demo\"\nspaces = [\"main\", \"auth\"]\n\n\
                      [space.auth]\nname = \"Auth\"\n\n\
                      [default_headers.a]\nvalue = \"1\"\nenabled = false\n";
        for file in [implicit, dotted] {
            let (dir, _p) = fixture();
            std::fs::write(dir.path().join("project.toml"), file).unwrap();
            let mut p = reopen(&dir);
            let read_order: Vec<String> = p.meta().default_headers.keys().cloned().collect();
            assert_eq!(read_order, ["b", "a"], "{file}");
            let mut h = p.meta().default_headers.clone();
            h.get_mut("b").unwrap().enabled = false;
            p.set_default_headers(&h).unwrap();
            h.insert("c".to_string(), on("3"));
            p.set_default_headers(&h).unwrap();
            for p in [&p, &reopen(&dir)] {
                let keys: Vec<&str> = p.meta().default_headers.keys().map(String::as_str).collect();
                assert_eq!(keys, ["b", "a", "c"], "{}", read(&dir, "project.toml").unwrap());
                assert_eq!(p.meta().default_headers, h);
            }
            assert_eq!(reopen(&dir).display_name(), "Demo");
        }
    }

    /// Removing the name keeps the comment lines above it: they belong
    /// to the file or to what follows.
    #[test]
    fn a_removed_name_leaves_the_comments_above_it() {
        let (dir, _p) = fixture();
        let text = read(&dir, "project.toml").unwrap();
        // First in the file: they go above the next value.
        std::fs::write(dir.path().join("project.toml"), format!("# Orders kit\n{text}")).unwrap();
        let mut p = reopen(&dir);
        p.set_project_name(None).unwrap();
        let after = read(&dir, "project.toml").unwrap();
        assert!(after.starts_with("# Orders kit\nspaces = "), "{after}");
        // Last of the values: they go above the first table.
        let last = "spaces = [\"main\", \"auth\"]\n# see README\nname = \"Demo\"\n\n\
                    [space.auth]\nname = \"Auth\"\n";
        std::fs::write(dir.path().join("project.toml"), last).unwrap();
        let mut p = reopen(&dir);
        p.set_project_name(None).unwrap();
        let after = read(&dir, "project.toml").unwrap();
        assert_eq!(
            after,
            "spaces = [\"main\", \"auth\"]\n# see README\n\n[space.auth]\nname = \"Auth\"\n"
        );
        p.undo().unwrap();
        assert_eq!(read(&dir, "project.toml").unwrap(), last);
    }

    /// Emptying the headers keeps the comment lines above the table: they
    /// go above what follows, or stay at the end of the file.
    #[test]
    fn emptied_headers_leave_the_comments_above_the_table() {
        let middle = "name = \"Demo\"\n\n# sent with every request\n[default_headers]\na = \"1\"\n\n\
                      [space.auth]\nname = \"Auth\"\n";
        let last = "name = \"Demo\"\n\n[space.auth]\nname = \"Auth\"\n\n\
                    # sent with every request\n[default_headers]\na = \"1\"\n# end\n";
        let inline =
            "# sent with every request\ndefault_headers = { a = \"1\" }\nname = \"Demo\"\n";
        let dotted = "# sent with every request\ndefault_headers.a = \"1\"\nname = \"Demo\"\n";
        for (file, want) in [
            (
                middle,
                "name = \"Demo\"\n\n# sent with every request\n\n[space.auth]\nname = \"Auth\"\n",
            ),
            (
                last,
                "name = \"Demo\"\n\n[space.auth]\nname = \"Auth\"\n\n# sent with every request\n# end\n",
            ),
            (inline, "# sent with every request\nname = \"Demo\"\n"),
            (dotted, "# sent with every request\nname = \"Demo\"\n"),
        ] {
            let (dir, _p) = fixture();
            std::fs::write(dir.path().join("project.toml"), file).unwrap();
            let mut p = reopen(&dir);
            p.set_default_headers(&IndexMap::new()).unwrap();
            assert_eq!(read(&dir, "project.toml").unwrap(), want, "{file}");
            assert!(p.meta().default_headers.is_empty());
        }
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

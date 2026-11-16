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
    /// ruling; undo puts it back). An entry the table already holds as it
    /// is keeps its own item; a changed entry under a name the table holds
    /// keeps that key's comments; an empty map removes the table.
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
                for (name, entry) in headers {
                    match old.as_ref().and_then(|t| t.get_key_value(name)) {
                        Some((key, item)) if entry_of(item).as_ref() == Some(entry) => {
                            table.insert_formatted(key, item.clone());
                        }
                        Some((key, item)) => {
                            table.insert_formatted(
                                key,
                                with_decor_of(Some(item), entry_item(entry)),
                            );
                        }
                        None => {
                            table.insert(name, entry_item(entry));
                        }
                    }
                }
                doc[DEFAULT_HEADERS] = toml_edit::Item::Table(table);
            })
        })
    }
}

/// The entry an item of `[default_headers]` holds: a plain string, or a
/// table with `value` and an optional `enabled` (the forms `Entry` reads).
fn entry_of(item: &toml_edit::Item) -> Option<Entry> {
    if let Some(value) = item.as_str() {
        return Some(Entry { value: value.to_string(), enabled: true });
    }
    let t = item.as_table_like()?;
    let value = t.get("value")?.as_str()?.to_string();
    let enabled = match t.get("enabled") {
        Some(e) => e.as_bool()?,
        None => true,
    };
    Some(Entry { value, enabled })
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

/// `entry` as `HttpRequest::to_toml_string` writes a table row: enabled as
/// a plain string, disabled as `{ value, enabled = false }`.
fn entry_item(entry: &Entry) -> toml_edit::Item {
    if entry.enabled {
        return toml_edit::value(entry.value.as_str());
    }
    let mut inline = toml_edit::InlineTable::new();
    inline.insert("value", toml_edit::Value::from(entry.value.as_str()));
    inline.insert("enabled", toml_edit::Value::from(false));
    toml_edit::Item::Value(toml_edit::Value::InlineTable(inline))
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

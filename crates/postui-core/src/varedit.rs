//! Surgical `toml_edit` mutations for `variables.toml` and
//! `environments/<env>.toml` (spec §5, §7).
//!
//! Every function here is pure text -> text: parse the document with
//! `toml_edit::DocumentMut`, mutate only the addressed item, and return
//! `doc.to_string()`. Write fidelity — comments, blank lines, ordering, and
//! unrelated options survive untouched — is the whole point (spec §7); see
//! the round-trip fixture tests below for the contract in practice.

use indexmap::IndexMap;
use toml_edit::{Array, DocumentMut, Item, Key, RawString, Table, Value, value};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditError {
    Parse(String),
    NotFound(String),
    Conflict(String),
}

impl std::fmt::Display for EditError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EditError::Parse(m) | EditError::NotFound(m) | EditError::Conflict(m) => {
                write!(f, "{m}")
            }
        }
    }
}

impl std::error::Error for EditError {}

fn parse(doc: &str) -> Result<DocumentMut, EditError> {
    doc.parse::<DocumentMut>()
        .map_err(|e| EditError::Parse(e.to_string()))
}

fn not_found(msg: impl Into<String>) -> EditError {
    EditError::NotFound(msg.into())
}

/// Gets-or-creates a nested table at `key` inside `parent`, as a real
/// (non-inline) `Table`. Brand-new *container* tables should pass
/// `implicit_if_new = true` so an empty ancestor (e.g. `options` or
/// `options.<selector>`) never prints its own header — only its leaves do,
/// matching the `[options.user."user 1"]` style. A table that is itself
/// the addressed target (e.g. `[base_url]`, `[options.user."user 1"]`)
/// should pass `false` so it always renders, even with no fields yet.
fn table_mut<'a>(
    parent: &'a mut Table,
    key: &str,
    implicit_if_new: bool,
) -> Result<&'a mut Table, EditError> {
    if !parent.contains_key(key) {
        let mut t = Table::new();
        t.set_implicit(implicit_if_new);
        parent.insert(key, Item::Table(t));
    }
    parent
        .get_mut(key)
        .and_then(Item::as_table_mut)
        .ok_or_else(|| EditError::Parse(format!("\"{key}\" exists but is not a table")))
}

/// Order- and decor-preserving rename of a key within `parent`. Works both
/// for a top-level table header (variable/selector/option rename — table header
/// *print* order in toml_edit follows each table's own recorded
/// `doc_position`, not map order, so this is a no-op risk there) and for a
/// flat `key = value` field inside a table (field key rename inside an
/// option row — flat field print order *is* map order, so this matters
/// there). Rebuilding unconditionally keeps both cases correct without
/// needing to special-case which situation we're in.
pub(crate) fn rename_key(parent: &mut Table, from: &str, to: &str) {
    let implicit = parent.is_implicit();
    let decor = parent.decor().clone();
    let position = parent.position();
    let order: Vec<String> = parent.iter().map(|(k, _)| k.to_string()).collect();

    let mut rebuilt = Table::new();
    rebuilt.set_implicit(implicit);
    *rebuilt.decor_mut() = decor;
    if let Some(pos) = position {
        rebuilt.set_position(pos);
    }

    for k in order {
        let (key, item) = parent.remove_entry(&k).expect("key came from iter");
        if k == from {
            let new_key = Key::new(to)
                .with_leaf_decor(key.leaf_decor().clone())
                .with_dotted_decor(key.dotted_decor().clone());
            rebuilt.insert_formatted(&new_key, item);
        } else {
            rebuilt.insert_formatted(&key, item);
        }
    }
    *parent = rebuilt;
}

/// Moves an existing `key` to the front of `parent`'s map order, keeping
/// every key's decor. Flat `key = value` pairs print in map order, so this
/// is what puts an option's `description` above its field values even when
/// the description is added to an option that already has values.
fn move_key_first(parent: &mut Table, key: &str) {
    let already_first = parent.iter().next().is_some_and(|(k, _)| k == key);
    if already_first || !parent.contains_key(key) {
        return;
    }
    let implicit = parent.is_implicit();
    let decor = parent.decor().clone();
    let position = parent.position();
    let mut order: Vec<String> = parent.iter().map(|(k, _)| k.to_string()).collect();
    order.retain(|k| k != key);
    order.insert(0, key.to_string());

    let mut rebuilt = Table::new();
    rebuilt.set_implicit(implicit);
    *rebuilt.decor_mut() = decor;
    if let Some(pos) = position {
        rebuilt.set_position(pos);
    }
    for k in order {
        let (key, item) = parent.remove_entry(&k).expect("key came from iter");
        rebuilt.insert_formatted(&key, item);
    }
    *parent = rebuilt;
}

/// Whether `name` already occupies either namespace a variable/selector
/// declaration could collide with: a top-level key, or a `[selectors.<name>]`
/// option — variable and selector names share one namespace (spec §1), so a
/// rename target must be checked against both.
fn name_exists(root: &Table, name: &str) -> bool {
    root.contains_key(name)
        || root
            .get("selectors")
            .and_then(Item::as_table)
            .is_some_and(|g| g.contains_key(name))
}

/// Splits a leading-decor prefix at its LAST blank line into `(file_header,
/// own_comment)`. `own_comment` is the block contiguous with the item the
/// prefix belongs to (from just after that blank line through the end of
/// `prefix`) — it's specific to that item and leaves with it on delete.
/// `file_header` is everything up to and including the blank line — the
/// part that predates the item and should survive it. Returns `None` when
/// `prefix` has no blank line at all, i.e. it's *only* the item's own
/// contiguous comment with nothing to transfer.
fn split_leading_decor(prefix: &str) -> Option<(&str, &str)> {
    let idx = prefix.rfind("\n\n")?;
    let split_at = idx + 2;
    Some((&prefix[..split_at], &prefix[split_at..]))
}

/// Joins a transferred file header onto the new first item's own existing
/// leading decor, collapsing any run of 3+ consecutive newlines down to a
/// single blank line (2). Both halves independently end their own block
/// with a blank-line separator — the header's trailing blank line and the
/// new-first-item's former separator-from-the-deleted-item are the same
/// logical blank line once merged, so naive concatenation would otherwise
/// double it up.
fn join_decor_prefix(header: &str, existing: &str) -> String {
    let mut out = String::with_capacity(header.len() + existing.len());
    let mut newline_run = 0usize;
    for ch in header.chars().chain(existing.chars()) {
        if ch == '\n' {
            newline_run += 1;
            if newline_run <= 2 {
                out.push(ch);
            }
        } else {
            newline_run = 0;
            out.push(ch);
        }
    }
    out
}

/// Prepends `header` onto the leading decor of whatever will actually
/// *print first* in `parent` after a deletion. Table headers keep their
/// leading decor on the `Table` itself; flat `key = value` pairs keep it on
/// their `Key`'s `leaf_decor`. An implicit ancestor table with no flat
/// children of its own (e.g. `options`, kept invisible on purpose so only
/// its leaves like `[options.user."user 1"]` print) never prints its own
/// decor, so this recurses into such a table's first child to find the
/// option that will actually be first on the page.
fn attach_header_to_new_first(parent: &mut Table, header: &str) {
    let Some(key) = parent.iter().next().map(|(k, _)| k.to_string()) else {
        return;
    };
    let recurse_into_child = matches!(
        parent.get(&key),
        Some(Item::Table(t)) if t.is_implicit() && !t.is_empty() && t.get_values().is_empty()
    );
    if recurse_into_child {
        if let Some(child) = parent.get_mut(&key).and_then(Item::as_table_mut) {
            attach_header_to_new_first(child, header);
        }
        return;
    }
    match parent.get_mut(&key) {
        Some(Item::Table(t)) => {
            let existing = t
                .decor()
                .prefix()
                .and_then(RawString::as_str)
                .unwrap_or_default()
                .to_string();
            t.decor_mut()
                .set_prefix(join_decor_prefix(header, &existing));
        }
        _ => {
            if let Some(mut km) = parent.key_mut(&key) {
                let existing = km
                    .leaf_decor()
                    .prefix()
                    .and_then(RawString::as_str)
                    .unwrap_or_default()
                    .to_string();
                km.leaf_decor_mut()
                    .set_prefix(join_decor_prefix(header, &existing));
            }
        }
    }
}

/// Removes `key` from `parent`. If `key` was the first item, the file-header
/// portion of its own leading decor (see `split_leading_decor`) is
/// transferred onto whatever becomes the new first-printed item, so the
/// file header outlives the deleted item even though that item's own
/// contiguous comment leaves with it (spec §7 write fidelity).
fn remove_transferring_header(parent: &mut Table, key: &str) -> Option<Item> {
    let is_first = parent.iter().next().is_some_and(|(k, _)| k == key);
    let header = is_first
        .then(|| match parent.get(key) {
            Some(Item::Table(t)) => t.decor().prefix().and_then(RawString::as_str),
            _ => parent
                .key(key)
                .and_then(|k| k.leaf_decor().prefix())
                .and_then(RawString::as_str),
        })
        .flatten()
        .and_then(split_leading_decor)
        .map(|(header, _own_comment)| header.to_string());

    let removed = parent.remove(key);

    if let Some(header) = header {
        attach_header_to_new_first(parent, &header);
    }

    removed
}

// ---------------------------------------------------------------------
// variables.toml verbs
// ---------------------------------------------------------------------

/// Creates the variable's table if absent, and sets whichever of
/// `description`/`default` are `Some`. Fields left `None` are untouched.
pub fn upsert_var(
    doc: &str,
    name: &str,
    description: Option<&str>,
    default: Option<&str>,
) -> Result<String, EditError> {
    let mut doc = parse(doc)?;
    let root = doc.as_table_mut();
    let t = table_mut(root, name, false)?;
    if let Some(d) = description {
        t["description"] = value(d);
    }
    if let Some(d) = default {
        t["default"] = value(d);
    }
    Ok(doc.to_string())
}

/// Removes a variable's `default` line, leaving the rest of its table
/// (description, secret) untouched. A variable without a default passes
/// through unchanged; `NotFound` when the variable itself is missing.
pub fn clear_default(doc: &str, name: &str) -> Result<String, EditError> {
    let mut doc = parse(doc)?;
    let root = doc.as_table_mut();
    let t = root
        .get_mut(name)
        .and_then(Item::as_table_mut)
        .ok_or_else(|| not_found(format!("variable \"{name}\" not found")))?;
    t.remove("default");
    Ok(doc.to_string())
}

/// Sets/clears `secret = true` on a variable. Turning secret on removes any
/// `default` (a default would commit a secret value into the shared file).
pub fn set_secret_flag(doc: &str, name: &str, secret: bool) -> Result<String, EditError> {
    let mut doc = parse(doc)?;
    let root = doc.as_table_mut();
    let t = root
        .get_mut(name)
        .and_then(Item::as_table_mut)
        .ok_or_else(|| not_found(format!("variable \"{name}\" not found")))?;
    if secret {
        t.remove("default");
        t["secret"] = value(true);
    } else {
        t.remove("secret");
    }
    Ok(doc.to_string())
}

/// Renames a variable's table header, cascading into every selector's
/// `fields` array. The environment-side half of a field rename — the key
/// inside each `[options.<selector>."<option>"]` row — is
/// [`rename_option_field`], which the caller loops across every
/// environment.
pub fn rename_var(doc: &str, from: &str, to: &str) -> Result<String, EditError> {
    let mut doc = parse(doc)?;
    let root = doc.as_table_mut();
    if !root.contains_key(from) {
        return Err(not_found(format!("variable \"{from}\" not found")));
    }
    if from != to && name_exists(root, to) {
        return Err(EditError::Conflict(format!(
            "\"{to}\" already exists; rename would merge two declarations into one"
        )));
    }
    rename_key(root, from, to);

    if let Some(selectors_table) = root.get_mut("selectors").and_then(Item::as_table_mut) {
        let selector_names: Vec<String> =
            selectors_table.iter().map(|(k, _)| k.to_string()).collect();
        for gname in selector_names {
            let Some(selector_table) = selectors_table.get_mut(&gname).and_then(Item::as_table_mut)
            else {
                continue;
            };

            if let Some(arr) = selector_table
                .get_mut("fields")
                .and_then(Item::as_array_mut)
            {
                for v in arr.iter_mut() {
                    if v.as_str() == Some(from) {
                        let decor = v.decor().clone();
                        *v = Value::from(to);
                        *v.decor_mut() = decor;
                    }
                }
            }
        }
    }

    Ok(doc.to_string())
}

/// Deletes a variable's table. `Conflict` if it's still a field of any
/// selector (remove it from the selector first).
pub fn delete_var(doc: &str, name: &str) -> Result<String, EditError> {
    let mut doc = parse(doc)?;
    let root = doc.as_table_mut();
    if !root.contains_key(name) {
        return Err(not_found(format!("variable \"{name}\" not found")));
    }
    if let Some(selectors_table) = root.get("selectors").and_then(Item::as_table) {
        for (gname, gitem) in selectors_table.iter() {
            let Some(fields) = gitem
                .as_table()
                .and_then(|t| t.get("fields"))
                .and_then(Item::as_array)
            else {
                continue;
            };
            if fields.iter().any(|v| v.as_str() == Some(name)) {
                return Err(EditError::Conflict(format!(
                    "variable \"{name}\" is a field of selector \"{gname}\"; remove it from the selector first"
                )));
            }
        }
    }
    remove_transferring_header(root, name);
    Ok(doc.to_string())
}

/// Creates the selector's table if absent, sets `description` when given, and
/// always sets `fields` to the given list.
pub fn upsert_selector(
    doc: &str,
    name: &str,
    description: Option<&str>,
    fields: &[String],
) -> Result<String, EditError> {
    if fields.is_empty() {
        return Err(EditError::Conflict(format!(
            "a selector needs at least one field; add a field before saving \"{name}\""
        )));
    }
    let mut doc = parse(doc)?;
    let root = doc.as_table_mut();
    let selectors = table_mut(root, "selectors", true)?;
    let g = table_mut(selectors, name, false)?;
    if let Some(d) = description {
        g["description"] = value(d);
    }
    let mut arr = Array::new();
    for f in fields {
        arr.push(f.as_str());
    }
    g["fields"] = Item::Value(Value::Array(arr));
    Ok(doc.to_string())
}

/// Sets or removes `shared = true` on an existing `[selectors.<name>]`
/// declaration (false removes the key — absent is the default). `NotFound`
/// when no such selector is declared.
pub fn set_selector_shared(doc: &str, name: &str, shared: bool) -> Result<String, EditError> {
    let mut doc = parse(doc)?;
    let root = doc.as_table_mut();
    let g = root
        .get_mut("selectors")
        .and_then(Item::as_table_mut)
        .and_then(|selectors| selectors.get_mut(name))
        .and_then(Item::as_table_mut)
        .ok_or_else(|| EditError::NotFound(format!("selector \"{name}\" not found")))?;
    if shared {
        g["shared"] = value(true);
    } else {
        g.remove("shared");
    }
    Ok(doc.to_string())
}

/// Renames a selector's `[selectors.<from>]` table header, decor and position
/// preserved. `NotFound` if there is no such selector; `Conflict` if `to`
/// isn't a valid name, or already occupies either namespace a declaration
/// can collide with (a top-level variable table or another selector) — a
/// rename that merged two declarations would silently lose one.
///
/// The environment-side half — each environment's `[options.<from>]`
/// subtree — is [`rename_selector_options`], which the caller loops across
/// every environment. Both halves have to land together: an environment
/// holding options for a selector the model no longer declares (and a selector
/// whose options are still filed under the old name) both fail
/// `validate_env`.
pub fn rename_selector(doc: &str, from: &str, to: &str) -> Result<String, EditError> {
    if !crate::vars::is_valid_var_name(to) {
        return Err(EditError::Conflict(format!(
            "\"{to}\" is not a valid selector name"
        )));
    }
    let mut doc = parse(doc)?;
    let root = doc.as_table_mut();
    if from != to && name_exists(root, to) {
        return Err(EditError::Conflict(format!(
            "\"{to}\" already exists; rename would merge two declarations into one"
        )));
    }
    let selectors = root
        .get_mut("selectors")
        .and_then(Item::as_table_mut)
        .filter(|g| g.contains_key(from))
        .ok_or_else(|| not_found(format!("selector \"{from}\" not found")))?;
    rename_key(selectors, from, to);
    Ok(doc.to_string())
}

/// Deletes a selector's table. `NotFound` if there is no such selector. The
/// environment-side half — the selector's `[options.<name>]` subtree — is
/// [`delete_selector_options`], which the caller loops across every
/// environment.
pub fn delete_selector(doc: &str, name: &str) -> Result<String, EditError> {
    let mut doc = parse(doc)?;
    let root = doc.as_table_mut();
    let removed = root
        .get_mut("selectors")
        .and_then(Item::as_table_mut)
        .and_then(|g| remove_transferring_header(g, name));
    if removed.is_none() {
        return Err(not_found(format!("selector \"{name}\" not found")));
    }
    Ok(doc.to_string())
}

// ---------------------------------------------------------------------
// environments/<env>.toml verbs
// ---------------------------------------------------------------------

/// Sets or removes a flat `name = "value"` pair. `None` removes the pair
/// (`NotFound` if it wasn't there).
pub fn set_env_value(doc: &str, name: &str, val: Option<&str>) -> Result<String, EditError> {
    let mut doc = parse(doc)?;
    let root = doc.as_table_mut();
    match val {
        Some(v) => {
            // Index-assignment updates the existing `Item` in place and
            // keeps the existing `Key` (and its decor/comment) when the
            // pair already exists; `Table::insert` would instead build a
            // fresh, decor-less `Key`, silently dropping any comment
            // attached to it.
            root[name] = value(v);
        }
        None => {
            if remove_transferring_header(root, name).is_none() {
                return Err(not_found(format!("\"{name}\" not found")));
            }
        }
    }
    Ok(doc.to_string())
}

/// `description` inside an options table is an option's own description, so
/// no option may be named that — the model would read the option back as a
/// malformed description rather than a record.
fn check_option_name(name: &str) -> Result<(), EditError> {
    if name == crate::varmodel::OPTION_DESCRIPTION {
        return Err(EditError::Conflict(format!(
            "\"{name}\" is reserved for an option's own description and can't be used as an option name"
        )));
    }
    Ok(())
}

/// This environment's `[options.<selector>]` table, if it has one.
fn options_selector_mut<'a>(root: &'a mut Table, selector: &str) -> Option<&'a mut Table> {
    root.get_mut("options")
        .and_then(Item::as_table_mut)
        .and_then(|options| options.get_mut(selector))
        .and_then(Item::as_table_mut)
}

/// Creates or updates `[options.<selector>."<option>"]`, setting `description`
/// (written above the field values) when given plus every field in
/// `values`. Fields already in the option but absent from `values` are left
/// alone — [`strip_option_field`] is how a field leaves an option.
pub fn upsert_option(
    doc: &str,
    selector: &str,
    option: &str,
    description: Option<&str>,
    values: &IndexMap<String, String>,
) -> Result<String, EditError> {
    check_option_name(option)?;
    let mut doc = parse(doc)?;
    let root = doc.as_table_mut();
    let options = table_mut(root, "options", true)?;
    let selector_table = table_mut(options, selector, true)?;
    let t = table_mut(selector_table, option, false)?;
    if let Some(d) = description {
        t["description"] = value(d);
        move_key_first(t, "description");
    }
    for (k, v) in values {
        t[k.as_str()] = value(v.as_str());
    }
    Ok(doc.to_string())
}

/// Renames one option of `selector`. `NotFound` if the selector or the option
/// isn't in this environment; `Conflict` if `to` already exists (the
/// rename would merge two records into one).
pub fn rename_option(doc: &str, selector: &str, from: &str, to: &str) -> Result<String, EditError> {
    check_option_name(to)?;
    let mut doc = parse(doc)?;
    let root = doc.as_table_mut();
    let g = options_selector_mut(root, selector)
        .ok_or_else(|| not_found(format!("selector \"{selector}\" has no options here")))?;
    if !g.contains_key(from) {
        return Err(not_found(format!(
            "option \"{from}\" not found in selector \"{selector}\""
        )));
    }
    if from != to && g.contains_key(to) {
        return Err(EditError::Conflict(format!(
            "option \"{to}\" already exists in selector \"{selector}\"; rename would merge two options into one"
        )));
    }
    rename_key(g, from, to);
    Ok(doc.to_string())
}

/// Removes one option's `description` key, leaving its values untouched —
/// the write half of clearing a description in the option Edit prompt
/// ([`upsert_option`]'s `None` deliberately preserves an existing
/// description, so a clear needs its own verb). A no-op when the option
/// has no description; `NotFound` if the selector or option isn't in this
/// environment.
pub fn remove_option_description(
    doc: &str,
    selector: &str,
    option: &str,
) -> Result<String, EditError> {
    let mut doc = parse(doc)?;
    let root = doc.as_table_mut();
    let t = options_selector_mut(root, selector)
        .and_then(|g| g.get_mut(option))
        .and_then(Item::as_table_mut)
        .ok_or_else(|| {
            not_found(format!(
                "option \"{option}\" not found in selector \"{selector}\""
            ))
        })?;
    t.remove("description");
    Ok(doc.to_string())
}

/// Removes one option of `selector`. `NotFound` if it isn't there.
pub fn delete_option(doc: &str, selector: &str, option: &str) -> Result<String, EditError> {
    let mut doc = parse(doc)?;
    let root = doc.as_table_mut();
    let removed =
        options_selector_mut(root, selector).and_then(|g| remove_transferring_header(g, option));
    if removed.is_none() {
        return Err(not_found(format!(
            "option \"{option}\" not found in selector \"{selector}\""
        )));
    }
    Ok(doc.to_string())
}

/// Removes the whole `[options.<selector>]` subtree — the environment-side
/// half of [`delete_selector`]. No-op (never an error) when this environment
/// has no options for the selector, since the caller loops it across every
/// environment.
pub fn delete_selector_options(doc: &str, selector: &str) -> Result<String, EditError> {
    let mut doc = parse(doc)?;
    let root = doc.as_table_mut();
    if let Some(options) = root.get_mut("options").and_then(Item::as_table_mut) {
        remove_transferring_header(options, selector);
    }
    Ok(doc.to_string())
}

/// Renames this environment's `[options.<from>]` subtree to
/// `[options.<to>]` — the environment-side half of [`rename_selector`]. A
/// no-op (never an error) when this environment has no options for the
/// selector, since the caller loops it across every environment. `Conflict`
/// when `to` already has its own options table here: the rename would
/// merge two selectors' records into one.
pub fn rename_selector_options(doc: &str, from: &str, to: &str) -> Result<String, EditError> {
    let mut doc = parse(doc)?;
    let root = doc.as_table_mut();
    let Some(options) = root.get_mut("options").and_then(Item::as_table_mut) else {
        return Ok(doc.to_string());
    };
    if !options.contains_key(from) {
        return Ok(doc.to_string());
    }
    if from != to && options.contains_key(to) {
        return Err(EditError::Conflict(format!(
            "\"{to}\" already has options in this environment; rename would merge two selectors into one"
        )));
    }
    rename_key(options, from, to);
    Ok(doc.to_string())
}

/// Gives every option of `selector` that lacks `field` an empty value for it —
/// what a field *joining* its selector does to the records already written
/// for it, and the mirror image of [`strip_option_field`]. Every option of a
/// selector must supply every declared field (`validate_env`), so a field
/// addition has to land in the environment files in the same breath as the
/// declaration. Entries that already set the field keep their value;
/// environments without the selector pass through untouched.
pub fn ensure_option_field(doc: &str, selector: &str, field: &str) -> Result<String, EditError> {
    let mut doc = parse(doc)?;
    let root = doc.as_table_mut();
    if let Some(g) = options_selector_mut(root, selector) {
        for (_, option) in g.iter_mut() {
            if let Some(t) = option.as_table_mut()
                && !t.contains_key(field)
            {
                t[field] = value("");
            }
        }
    }
    Ok(doc.to_string())
}

/// Renames a field key inside every option of `selector` — the
/// environment-side half of [`rename_var`] for a selector field. Entries
/// without the field (and environments without the selector) pass through
/// untouched.
pub fn rename_option_field(
    doc: &str,
    selector: &str,
    from: &str,
    to: &str,
) -> Result<String, EditError> {
    let mut doc = parse(doc)?;
    let root = doc.as_table_mut();
    if let Some(g) = options_selector_mut(root, selector) {
        let option_names: Vec<String> = g.iter().map(|(k, _)| k.to_string()).collect();
        for name in option_names {
            if let Some(option) = g.get_mut(&name).and_then(Item::as_table_mut)
                && option.contains_key(from)
            {
                rename_key(option, from, to);
            }
        }
    }
    Ok(doc.to_string())
}

/// Removes a field key from every option of `selector` — what a field leaving
/// its selector does to the values already recorded for it. Entries without
/// the field (and environments without the selector) pass through untouched.
pub fn strip_option_field(doc: &str, selector: &str, field: &str) -> Result<String, EditError> {
    let mut doc = parse(doc)?;
    let root = doc.as_table_mut();
    if let Some(g) = options_selector_mut(root, selector) {
        for (_, option) in g.iter_mut() {
            if let Some(t) = option.as_table_mut() {
                t.remove(field);
            }
        }
    }
    Ok(doc.to_string())
}

/// Renames `from` to `to` wherever `from` appears in an environment file:
/// its flat `from = "value"` pair (if any) and its `[options.from]`
/// table (if any, i.e. when `from` is a selector) — the cascade `rename_var`
/// itself doesn't (and can't;
/// `rename_var` only ever sees `variables.toml`) do, so the Manager's
/// rename op loops this across every environment after `rename_var`
/// succeeds. Neither being present is a no-op (returns `doc` unchanged,
/// not `NotFound`) — most environments won't have anything to rename for
/// a given variable, and the caller's per-env loop reads far simpler
/// unconditionally calling this than having to first check which
/// environments need it.
pub fn rename_env_var(doc: &str, from: &str, to: &str) -> Result<String, EditError> {
    let mut doc = parse(doc)?;
    let root = doc.as_table_mut();
    let has_flat = root.contains_key(from);
    let has_options = root
        .get("options")
        .and_then(Item::as_table)
        .is_some_and(|e| e.contains_key(from));
    if !has_flat && !has_options {
        return Ok(doc.to_string());
    }
    // Conflict check up front, before any mutation: a `to` that already
    // occupies the same slot `from` is being moved into would otherwise
    // silently merge two options into one (`rename_key`'s rebuild would
    // encounter `to` twice and the second insert wins, dropping the first).
    if from != to {
        let flat_conflict = has_flat && root.contains_key(to);
        let options_conflict = has_options
            && root
                .get("options")
                .and_then(Item::as_table)
                .is_some_and(|e| e.contains_key(to));
        if flat_conflict || options_conflict {
            return Err(EditError::Conflict(format!(
                "\"{to}\" already exists in this environment; rename would merge two options into one"
            )));
        }
    }
    if has_flat {
        rename_key(root, from, to);
    }
    if has_options {
        let options = root
            .get_mut("options")
            .and_then(Item::as_table_mut)
            .expect("has_options confirmed the table exists above");
        rename_key(options, from, to);
    }
    Ok(doc.to_string())
}

// ---------------------------------------------------------------------
// Manager integration: usage scan + promote (spec §4)
// ---------------------------------------------------------------------

/// Slugs (sorted, matching [`crate::storage::list_requests`] order) of
/// saved requests whose raw file text contains a well-formed `{{name}}`
/// token — url, params, headers, body, and `[variables]` values are all
/// plain TOML string values, so a raw-text [`crate::vars::find_tokens`]
/// scan is exact and cheap; no need to parse each file's fields
/// individually.
pub fn scan_usage(root: &std::path::Path, name: &str) -> Vec<String> {
    let (listings, _walk_err) = crate::storage::list_requests(root);
    listings
        .into_iter()
        .filter(|listing| {
            let path = root.join("requests").join(format!("{}.toml", listing.slug));
            std::fs::read_to_string(&path)
                .map(|text| {
                    crate::vars::find_tokens(&text)
                        .iter()
                        .any(|t| t.name == name)
                })
                .unwrap_or(false)
        })
        .map(|listing| listing.slug)
        .collect()
}

/// Where a promoted request-scope value lands in the project.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromoteTarget {
    /// The declaration's shared `default`.
    Default,
    /// A flat pair in the active environment, plus a bare declaration
    /// (name only, no fields) so the variable exists project-wide.
    Env,
}

/// Promotes a request-scope `{{name}} = value` into the project:
/// `Default` writes `value` as the declaration's `default`; `Env` writes a
/// flat `name = value` pair into `env_doc` and a bare declaration (no
/// fields) into `vars_doc`. `Conflict` if `name` collides with anything
/// already occupying that name in `vars_doc`:
/// - a secret variable — promoting a plain value onto it would either
///   commit a secret or make the declaration invalid;
/// - an existing selector's own name — selector and variable names share one
///   namespace (spec §1), so `upsert_var` would otherwise create a
///   colliding top-level variable table alongside `[selectors.<name>]`;
/// - an existing selector's field — that name's value comes from the selector's
///   selected option, so writing a `default` onto it would be dead and
///   misleading.
///
/// In every conflict case the caller should offer a rename instead. The
/// caller is responsible for removing the request's own `[variables]`
/// option.
pub fn promote_var(
    vars_doc: &str,
    env_doc: Option<&str>,
    name: &str,
    value: &str,
    target: PromoteTarget,
) -> Result<(String, Option<String>), EditError> {
    let existing = parse(vars_doc)?;
    let root = existing.as_table();

    if root
        .get(name)
        .and_then(Item::as_table)
        .and_then(|t| t.get("secret"))
        .and_then(Item::as_bool)
        .unwrap_or(false)
    {
        return Err(EditError::Conflict(format!(
            "variable \"{name}\" is secret; promoting a plain value onto it would either commit a secret to variables.toml or make the declaration invalid"
        )));
    }

    if let Some(selectors_table) = root.get("selectors").and_then(Item::as_table) {
        if selectors_table.contains_key(name) {
            return Err(EditError::Conflict(format!(
                "\"{name}\" is already a selector name; selector and variable names share one namespace"
            )));
        }
        for (gname, gitem) in selectors_table.iter() {
            let is_field = gitem
                .as_table()
                .and_then(|t| t.get("fields"))
                .and_then(Item::as_array)
                .is_some_and(|fields| fields.iter().any(|v| v.as_str() == Some(name)));
            if is_field {
                return Err(EditError::Conflict(format!(
                    "variable \"{name}\" is a field of selector \"{gname}\"; its value comes from the selector's selected option"
                )));
            }
        }
    }

    match target {
        PromoteTarget::Default => {
            let new_vars = upsert_var(vars_doc, name, None, Some(value))?;
            Ok((new_vars, None))
        }
        PromoteTarget::Env => {
            let new_vars = upsert_var(vars_doc, name, None, None)?;
            let env_doc = env_doc.ok_or_else(|| {
                not_found("no active environment file to promote into".to_string())
            })?;
            let new_env = set_env_value(env_doc, name, Some(value))?;
            Ok((new_vars, Some(new_env)))
        }
    }
}

/// Removes every environment-level trace of `name`: its flat `name =
/// "value"` pair (if any) and its whole `[options.name]` subtree (if any,
/// i.e. when `name` is a selector). No-op (returns `doc` unchanged) when
/// neither exists — mirrors `rename_env_var`'s per-env-loop-friendly
/// behavior, since the caller (the Manager's delete cascade) loops this
/// across every environment unconditionally, most of which won't have
/// anything for a given name.
pub fn delete_env_var(doc: &str, name: &str) -> Result<String, EditError> {
    let mut doc = parse(doc)?;
    let root = doc.as_table_mut();
    remove_transferring_header(root, name);
    if let Some(options) = root.get_mut("options").and_then(Item::as_table_mut) {
        remove_transferring_header(options, name);
    }
    Ok(doc.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fixture mirrors spec §3.2's `variables.toml` example, plus a
    /// one-field selector (what a migrated enumerated variable becomes) so
    /// both selector shapes are exercised.
    const VARS: &str = r#"# variables.toml

[base_url]
description = "API root"
default = "http://localhost:8080"

[api_key]
description = "service API key"
secret = true

[selectors.tier]
description = "pricing tier"
fields = ["tier"]

[selectors.test-user]
description = "user with linked customer"
fields = ["user_id", "customer_id"]
"#;

    /// Fixture mirrors spec §3.2's `environments/<env>.toml` example.
    const ENV: &str = r#"# environments/qa.toml

base_url = "https://qa.example.com"

[options.tier.gold]
description = "the good one"
tier = "g-1"

[options.test-user."user 1"]
user_id = "1001"
customer_id = "c-77"

[options.test-user."user 2"]
user_id = "1002"
customer_id = "c-91"
"#;

    /// Every environment verb's output must still parse as an environment
    /// document; returns the parsed data so callers can assert on it.
    fn reparses_env(s: &str) -> crate::varmodel::EnvData {
        crate::varmodel::parse_environment(s)
            .unwrap_or_else(|e| panic!("output must reparse as an environment: {e}\n---\n{s}"))
    }

    fn reparses_vars(s: &str) -> crate::varmodel::VarModel {
        crate::varmodel::parse_variables(s)
            .unwrap_or_else(|e| panic!("output must reparse as variables: {e}\n---\n{s}"))
    }

    fn vals(pairs: &[(&str, &str)]) -> IndexMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    // -------------------------------------------------------------
    // variables.toml verbs
    // -------------------------------------------------------------

    #[test]
    fn upsert_var_updates_only_the_given_field_on_an_existing_var() {
        let out = upsert_var(VARS, "base_url", None, Some("http://localhost:9090")).unwrap();
        assert_eq!(
            out,
            r#"# variables.toml

[base_url]
description = "API root"
default = "http://localhost:9090"

[api_key]
description = "service API key"
secret = true

[selectors.tier]
description = "pricing tier"
fields = ["tier"]

[selectors.test-user]
description = "user with linked customer"
fields = ["user_id", "customer_id"]
"#
        );
    }

    #[test]
    fn upsert_var_creates_a_new_var_table_at_the_end() {
        let out = upsert_var(VARS, "timeout", Some("request timeout, ms"), Some("30000")).unwrap();
        assert!(out.starts_with("# variables.toml\n"));
        assert!(
            out.ends_with(
                "[timeout]\ndescription = \"request timeout, ms\"\ndefault = \"30000\"\n"
            )
        );
        let m = reparses_vars(&out);
        assert_eq!(m.vars["timeout"].default.as_deref(), Some("30000"));
    }

    #[test]
    fn set_secret_flag_true_strips_default_and_sets_secret() {
        let out = set_secret_flag(VARS, "base_url", true).unwrap();
        assert!(out.contains("[base_url]\ndescription = \"API root\"\nsecret = true\n"));
        assert!(!out.contains("localhost:8080"), "the default is gone");
        assert!(reparses_vars(&out).vars["base_url"].secret);
    }

    #[test]
    fn set_secret_flag_false_removes_the_secret_line() {
        let out = set_secret_flag(VARS, "api_key", false).unwrap();
        assert!(out.contains("[api_key]\ndescription = \"service API key\"\n\n"));
        assert!(!reparses_vars(&out).vars["api_key"].secret);
    }

    #[test]
    fn clear_default_removes_only_the_default_line() {
        let out = clear_default(VARS, "base_url").unwrap();
        assert!(!out.contains("localhost:8080"), "{out}");
        assert!(
            out.contains("[base_url]\ndescription = \"API root\"\n"),
            "the description stays: {out}"
        );
        assert!(
            reparses_vars(&out).vars["base_url"].default.is_none(),
            "{out}"
        );
        // A variable with no default passes through unchanged...
        let none = clear_default(&out, "base_url").unwrap();
        assert_eq!(none, out);
        // ...and a missing variable is NotFound.
        assert_eq!(
            clear_default(VARS, "nope").unwrap_err(),
            EditError::NotFound("variable \"nope\" not found".to_string())
        );
    }

    #[test]
    fn set_secret_flag_not_found_on_missing_var() {
        let err = set_secret_flag(VARS, "nope", true).unwrap_err();
        assert_eq!(
            err,
            EditError::NotFound("variable \"nope\" not found".to_string())
        );
    }

    #[test]
    fn rename_var_renames_the_header_in_place_no_other_lines_move() {
        let out = rename_var(VARS, "base_url", "root_url").unwrap();
        assert_eq!(out, VARS.replace("[base_url]", "[root_url]"));
        assert!(reparses_vars(&out).vars.contains_key("root_url"));
    }

    #[test]
    fn rename_var_cascades_into_selector_fields() {
        // The model forbids a field from also being a declared variable,
        // but `varedit` is text -> text and never parses the model, so a
        // hand-edited file carrying both must not come back half-renamed.
        let vars = "[user_id]\ndescription = \"x\"\n\n[selectors.test-user]\nfields = [\"user_id\", \"customer_id\"]\n";
        let out = rename_var(vars, "user_id", "uid").unwrap();
        assert!(out.contains("[uid]"), "{out}");
        assert!(
            out.contains("fields = [\"uid\", \"customer_id\"]"),
            "the selector's field list follows the rename:\n{out}"
        );
    }

    #[test]
    fn rename_var_conflict_when_to_already_exists_as_a_variable() {
        let err = rename_var(VARS, "base_url", "api_key").unwrap_err();
        assert!(matches!(err, EditError::Conflict(_)));
    }

    #[test]
    fn rename_var_conflict_when_to_already_exists_as_a_selector() {
        let err = rename_var(VARS, "base_url", "test-user").unwrap_err();
        assert!(matches!(err, EditError::Conflict(_)));
    }

    #[test]
    fn rename_var_not_found_on_missing_var() {
        let err = rename_var(VARS, "nope", "x").unwrap_err();
        assert_eq!(
            err,
            EditError::NotFound("variable \"nope\" not found".to_string())
        );
    }

    #[test]
    fn delete_var_removes_the_table() {
        let out = delete_var(VARS, "api_key").unwrap();
        assert!(!out.contains("[api_key]"));
        assert!(!reparses_vars(&out).vars.contains_key("api_key"));
    }

    #[test]
    fn delete_var_of_first_item_keeps_the_file_header_but_drops_its_own_comment() {
        // Regression: deleting the FIRST top-level item used to drop the
        // file's leading header comment along with it, because the header
        // lives in that item's own leading decor. The fix splits that decor
        // at its last blank line: the file header (above the blank line)
        // transfers to the new first item; the deleted item's own
        // contiguous comment (below the blank line, right against the
        // item) leaves with it, same as before.
        let vars = r#"# variables.toml

# base_url is the API root, override per-environment as needed
[base_url]
description = "API root"
default = "http://localhost:8080"

[api_key]
description = "service API key"
secret = true
"#;
        let out = delete_var(vars, "base_url").unwrap();
        assert_eq!(
            out,
            r#"# variables.toml

[api_key]
description = "service API key"
secret = true
"#
        );
    }

    #[test]
    fn delete_var_conflict_when_still_a_selector_field() {
        let vars =
            "[user_id]\ndescription = \"x\"\n\n[selectors.test-user]\nfields = [\"user_id\"]\n";
        let err = delete_var(vars, "user_id").unwrap_err();
        assert_eq!(
            err,
            EditError::Conflict(
                "variable \"user_id\" is a field of selector \"test-user\"; remove it from the selector first"
                    .to_string()
            )
        );
    }

    #[test]
    fn delete_var_not_found_on_missing_var() {
        let err = delete_var(VARS, "nope").unwrap_err();
        assert_eq!(
            err,
            EditError::NotFound("variable \"nope\" not found".to_string())
        );
    }

    #[test]
    fn upsert_selector_writes_fields_and_round_trips() {
        let out = upsert_selector(
            "[base_url]\ndefault = \"x\"\n",
            "user",
            Some("linked pair"),
            &["user_id".to_string(), "customer_id".to_string()],
        )
        .unwrap();
        assert!(
            out.contains("fields = [\"user_id\", \"customer_id\"]"),
            "{out}"
        );
        let m = reparses_vars(&out);
        assert_eq!(m.selectors["user"].fields, ["user_id", "customer_id"]);
        assert_eq!(
            m.selectors["user"].description.as_deref(),
            Some("linked pair")
        );
    }

    #[test]
    fn set_selector_shared_writes_and_removes_the_flag() {
        let doc = upsert_selector("", "locale", None, &["lang".to_string()]).unwrap();
        let out = set_selector_shared(&doc, "locale", true).unwrap();
        let m = reparses_vars(&out);
        assert!(m.selectors["locale"].shared);
        let out = set_selector_shared(&out, "locale", false).unwrap();
        assert!(
            !out.contains("shared"),
            "flag removed, not set false: {out}"
        );
        assert!(!reparses_vars(&out).selectors["locale"].shared);

        let err = set_selector_shared("", "nope", true).unwrap_err();
        assert!(matches!(err, EditError::NotFound(_)), "{err:?}");
    }

    #[test]
    fn upsert_selector_with_no_fields_is_refused() {
        // A selector needs at least one field (`varmodel::EmptyFields`), so
        // writing an empty list would produce a file that no longer loads.
        let err = upsert_selector(VARS, "empty", None, &[]).unwrap_err();
        assert_eq!(
            err,
            EditError::Conflict(
                "a selector needs at least one field; add a field before saving \"empty\""
                    .to_string()
            )
        );
    }

    #[test]
    fn upsert_selector_replaces_fields_on_an_existing_selector() {
        let out = upsert_selector(VARS, "test-user", None, &["user_id".to_string()]).unwrap();
        assert_eq!(
            reparses_vars(&out).selectors["test-user"].fields,
            ["user_id"]
        );
        assert_eq!(
            reparses_vars(&out).selectors["test-user"]
                .description
                .as_deref(),
            Some("user with linked customer"),
            "a description left as None is untouched"
        );
    }

    #[test]
    fn delete_selector_removes_the_selector() {
        let out = delete_selector(VARS, "test-user").unwrap();
        assert!(!out.contains("test-user"));
        assert!(!reparses_vars(&out).selectors.contains_key("test-user"));
        assert!(reparses_vars(&out).selectors.contains_key("tier"));
    }

    #[test]
    fn delete_selector_not_found_on_missing_selector() {
        let err = delete_selector(VARS, "nope").unwrap_err();
        assert_eq!(
            err,
            EditError::NotFound("selector \"nope\" not found".to_string())
        );
    }

    // -------------------------------------------------------------
    // environments/<env>.toml verbs
    // -------------------------------------------------------------

    #[test]
    fn set_env_value_adds_a_new_flat_pair() {
        let out = set_env_value(ENV, "region", Some("us-east")).unwrap();
        assert!(out.contains("base_url = \"https://qa.example.com\"\nregion = \"us-east\"\n"));
        assert_eq!(reparses_env(&out).values["region"], "us-east");
    }

    #[test]
    fn set_env_value_updates_an_existing_flat_pair() {
        let out = set_env_value(ENV, "base_url", Some("https://qa2.example.com")).unwrap();
        assert_eq!(
            out,
            ENV.replace("https://qa.example.com", "https://qa2.example.com")
        );
        assert_eq!(
            reparses_env(&out).values["base_url"],
            "https://qa2.example.com"
        );
    }

    #[test]
    fn set_env_value_update_preserves_the_keys_own_leading_comment() {
        // Regression: `set_env_value(Some(v))` on an EXISTING, non-first key
        // used to build a fresh, decor-less `Key` on every update
        // (`Table::insert`), silently dropping any comment attached to that
        // key. Index-assignment instead reuses the existing `Key`.
        let env = r#"# environments/qa.toml

base_url = "https://qa.example.com"
# staging region override, remove once qa2 is retired
region = "eu-west"

[options.tier.gold]
tier = "g-1"
"#;
        let out = set_env_value(env, "region", Some("us-east")).unwrap();
        assert_eq!(
            out,
            r#"# environments/qa.toml

base_url = "https://qa.example.com"
# staging region override, remove once qa2 is retired
region = "us-east"

[options.tier.gold]
tier = "g-1"
"#
        );
    }

    #[test]
    fn set_env_value_none_removes_the_flat_pair() {
        let out = set_env_value(ENV, "base_url", None).unwrap();
        assert!(!out.contains("base_url"));
        assert!(reparses_env(&out).values.is_empty());
    }

    #[test]
    fn set_env_value_none_not_found_when_pair_absent() {
        let err = set_env_value(ENV, "nope", None).unwrap_err();
        assert_eq!(err, EditError::NotFound("\"nope\" not found".to_string()));
    }

    #[test]
    fn upsert_option_creates_and_updates_preserving_other_options() {
        let doc = "base_url = \"x\"\n";
        let mut vals = IndexMap::new();
        vals.insert("user_id".to_string(), "1001".to_string());
        vals.insert("customer_id".to_string(), "cust-77".to_string());
        let out = upsert_option(doc, "user", "user 1", None, &vals).unwrap();
        assert!(out.contains("[options.user.\"user 1\"]"));
        let mut vals2 = vals.clone();
        vals2.insert("user_id".to_string(), "9999".to_string());
        let out2 = upsert_option(&out, "user", "user 1", Some("admin"), &vals2).unwrap();
        assert!(out2.contains("9999") && out2.contains("description = \"admin\""));
        assert_eq!(out2.matches("[options.user.").count(), 1);
        let e = reparses_env(&out2);
        assert_eq!(e.options["user"]["user 1"].values["user_id"], "9999");
        assert_eq!(
            e.options["user"]["user 1"].description.as_deref(),
            Some("admin")
        );
        assert_eq!(e.values["base_url"], "x");
    }

    #[test]
    fn upsert_option_leaves_sibling_options_alone() {
        let out = upsert_option(
            ENV,
            "test-user",
            "user 3",
            None,
            &vals(&[("user_id", "1003"), ("customer_id", "c-03")]),
        )
        .unwrap();
        let e = reparses_env(&out);
        assert_eq!(e.options["test-user"].len(), 3);
        assert_eq!(e.options["test-user"]["user 1"].values["user_id"], "1001");
        assert_eq!(
            e.options["test-user"]["user 3"].values["customer_id"],
            "c-03"
        );
        assert_eq!(e.options["tier"]["gold"].values["tier"], "g-1");
    }

    #[test]
    fn upsert_option_quotes_awkward_option_names() {
        let out = upsert_option(
            "",
            "user",
            "the \"big\" one, v2",
            None,
            &vals(&[("user_id", "1")]),
        )
        .unwrap();
        let e = reparses_env(&out);
        assert_eq!(
            e.options["user"]["the \"big\" one, v2"].values["user_id"],
            "1"
        );
    }

    #[test]
    fn remove_option_description_removes_only_that_key() {
        let out = remove_option_description(ENV, "tier", "gold").unwrap();
        let e = reparses_env(&out);
        assert_eq!(e.options["tier"]["gold"].description, None);
        assert_eq!(e.options["tier"]["gold"].values["tier"], "g-1");
        assert_eq!(e.options["test-user"].len(), 2, "siblings untouched");
        assert_eq!(e.values["base_url"], "https://qa.example.com");
    }

    #[test]
    fn remove_option_description_is_a_noop_without_one_and_errors_on_missing_option() {
        let out = remove_option_description(ENV, "test-user", "user 1").unwrap();
        reparses_env(&out);
        assert!(out.contains("[options.test-user.\"user 1\"]"));
        assert!(matches!(
            remove_option_description(ENV, "tier", "nope"),
            Err(EditError::NotFound(_))
        ));
        assert!(matches!(
            remove_option_description(ENV, "ghost", "gold"),
            Err(EditError::NotFound(_))
        ));
    }

    #[test]
    fn upsert_option_writes_description_above_the_field_values() {
        let out = upsert_option("", "user", "u1", None, &vals(&[("user_id", "1")])).unwrap();
        let out = upsert_option(&out, "user", "u1", Some("later"), &vals(&[])).unwrap();
        assert!(
            out.contains("description = \"later\"\nuser_id = \"1\""),
            "description goes first even when added after the values:\n{out}"
        );
        reparses_env(&out);
    }

    #[test]
    fn an_option_may_not_be_named_description() {
        // `description` inside an options table is an option's own
        // description, so writing one under that name would produce a
        // document the model rejects.
        let err = upsert_option(
            ENV,
            "test-user",
            "description",
            None,
            &vals(&[("user_id", "1")]),
        )
        .unwrap_err();
        assert_eq!(
            err,
            EditError::Conflict(
                "\"description\" is reserved for an option's own description and can't be used as an option name"
                    .to_string()
            )
        );
        assert!(matches!(
            rename_option(ENV, "test-user", "user 1", "description"),
            Err(EditError::Conflict(_))
        ));
    }

    #[test]
    fn rename_option_preserves_value_order_and_unrelated_comments() {
        let out = rename_option(ENV, "test-user", "user 1", "the first user").unwrap();
        assert!(out.starts_with("# environments/qa.toml\n"));
        assert!(out.contains(
            "[options.test-user.\"the first user\"]\nuser_id = \"1001\"\ncustomer_id = \"c-77\"\n"
        ));
        let e = reparses_env(&out);
        assert_eq!(
            e.options["test-user"].keys().collect::<Vec<_>>(),
            ["the first user", "user 2"]
        );
    }

    #[test]
    fn rename_option_conflict_when_target_exists() {
        let err = rename_option(ENV, "test-user", "user 1", "user 2").unwrap_err();
        assert_eq!(
            err,
            EditError::Conflict(
                "option \"user 2\" already exists in selector \"test-user\"; rename would merge two options into one"
                    .to_string()
            )
        );
    }

    #[test]
    fn rename_option_not_found_for_missing_option_or_selector() {
        assert_eq!(
            rename_option(ENV, "test-user", "nope", "x").unwrap_err(),
            EditError::NotFound("option \"nope\" not found in selector \"test-user\"".to_string())
        );
        assert_eq!(
            rename_option(ENV, "ghost", "a", "b").unwrap_err(),
            EditError::NotFound("selector \"ghost\" has no options here".to_string())
        );
    }

    #[test]
    fn delete_option_removes_only_that_option() {
        let out = delete_option(ENV, "test-user", "user 1").unwrap();
        let e = reparses_env(&out);
        assert_eq!(
            e.options["test-user"].keys().collect::<Vec<_>>(),
            ["user 2"]
        );
        assert!(e.options.contains_key("tier"));
    }

    #[test]
    fn delete_option_not_found_when_absent() {
        assert_eq!(
            delete_option(ENV, "test-user", "nope").unwrap_err(),
            EditError::NotFound("option \"nope\" not found in selector \"test-user\"".to_string())
        );
        assert_eq!(
            delete_option(ENV, "ghost", "nope").unwrap_err(),
            EditError::NotFound("option \"nope\" not found in selector \"ghost\"".to_string())
        );
    }

    #[test]
    fn delete_selector_options_removes_the_whole_subtree() {
        let out = delete_selector_options(ENV, "test-user").unwrap();
        assert!(!out.contains("test-user"));
        let e = reparses_env(&out);
        assert!(!e.options.contains_key("test-user"));
        assert_eq!(e.options["tier"]["gold"].values["tier"], "g-1");
    }

    #[test]
    fn delete_selector_options_is_a_no_op_when_absent() {
        assert_eq!(delete_selector_options(ENV, "ghost").unwrap(), ENV);
        assert_eq!(
            delete_selector_options("base_url = \"x\"\n", "ghost").unwrap(),
            "base_url = \"x\"\n"
        );
    }

    #[test]
    fn rename_option_field_touches_every_option_of_the_selector() {
        let out = rename_option_field(ENV, "test-user", "user_id", "uid").unwrap();
        let e = reparses_env(&out);
        assert_eq!(e.options["test-user"]["user 1"].values["uid"], "1001");
        assert_eq!(e.options["test-user"]["user 2"].values["uid"], "1002");
        assert_eq!(
            e.options["test-user"]["user 1"]
                .values
                .keys()
                .collect::<Vec<_>>(),
            ["uid", "customer_id"],
            "the renamed key keeps its position"
        );
        assert_eq!(e.options["tier"]["gold"].values["tier"], "g-1");
    }

    #[test]
    fn rename_option_field_is_a_no_op_when_the_selector_or_field_is_absent() {
        assert_eq!(rename_option_field(ENV, "ghost", "a", "b").unwrap(), ENV);
        assert_eq!(rename_option_field(ENV, "tier", "ghost", "b").unwrap(), ENV);
    }

    #[test]
    fn strip_option_field_removes_it_from_every_option() {
        let out = strip_option_field(ENV, "test-user", "customer_id").unwrap();
        let e = reparses_env(&out);
        assert!(
            !e.options["test-user"]["user 1"]
                .values
                .contains_key("customer_id")
        );
        assert!(
            !e.options["test-user"]["user 2"]
                .values
                .contains_key("customer_id")
        );
        assert_eq!(e.options["test-user"]["user 1"].values["user_id"], "1001");
    }

    #[test]
    fn strip_option_field_is_a_no_op_when_the_selector_or_field_is_absent() {
        assert_eq!(strip_option_field(ENV, "ghost", "a").unwrap(), ENV);
        assert_eq!(strip_option_field(ENV, "tier", "ghost").unwrap(), ENV);
    }

    #[test]
    fn ensure_option_field_fills_only_the_options_that_lack_it() {
        let out = ensure_option_field(ENV, "test-user", "region").unwrap();
        let env = reparses_env(&out);
        let options = &env.options["test-user"];
        assert_eq!(options["user 1"].values["region"], "");
        assert_eq!(options["user 2"].values["region"], "");
        // Existing values are never overwritten…
        assert_eq!(options["user 1"].values["user_id"], "1001");
        let again = ensure_option_field(&out, "test-user", "user_id").unwrap();
        assert_eq!(
            reparses_env(&again).options["test-user"]["user 1"].values["user_id"],
            "1001"
        );
        // …and another selector's options are untouched.
        assert!(!env.options["tier"]["gold"].values.contains_key("region"));
    }

    #[test]
    fn ensure_option_field_is_a_no_op_when_the_selector_has_no_options_here() {
        assert_eq!(ensure_option_field(ENV, "ghost", "a").unwrap(), ENV);
        assert_eq!(ensure_option_field("", "test-user", "a").unwrap(), "");
    }

    // -------------------------------------------------------------
    // selector rename: both halves
    // -------------------------------------------------------------

    #[test]
    fn rename_selector_moves_the_header_and_keeps_its_decor() {
        let out = rename_selector(VARS, "test-user", "customer").unwrap();
        assert!(out.contains("[selectors.customer]"), "{out}");
        assert!(!out.contains("[selectors.test-user]"), "{out}");
        assert!(
            out.contains(
                "[selectors.customer]\ndescription = \"user with linked customer\"\nfields = [\"user_id\", \"customer_id\"]\n"
            ),
            "description, fields and their formatting survive: {out}"
        );
        // Everything else in the file is byte-identical.
        assert!(out.starts_with("# variables.toml\n"), "{out}");
        assert!(out.contains("[selectors.tier]"), "{out}");
        let m = reparses_vars(&out);
        assert_eq!(
            m.selectors["customer"].fields,
            vec!["user_id", "customer_id"]
        );
        assert!(!m.selectors.contains_key("test-user"));
    }

    #[test]
    fn rename_selector_refuses_a_missing_selector_a_taken_name_and_a_bad_name() {
        assert_eq!(
            rename_selector(VARS, "ghost", "x").unwrap_err(),
            EditError::NotFound("selector \"ghost\" not found".to_string())
        );
        // A variable already holds the name…
        assert!(matches!(
            rename_selector(VARS, "test-user", "base_url").unwrap_err(),
            EditError::Conflict(_)
        ));
        // …and so does another selector.
        assert!(matches!(
            rename_selector(VARS, "test-user", "tier").unwrap_err(),
            EditError::Conflict(_)
        ));
        assert!(matches!(
            rename_selector(VARS, "test-user", "not a name").unwrap_err(),
            EditError::Conflict(_)
        ));
        // Renaming to itself is allowed (and inert).
        assert_eq!(rename_selector(VARS, "tier", "tier").unwrap(), VARS);
    }

    #[test]
    fn rename_selector_options_moves_the_whole_subtree() {
        let out = rename_selector_options(ENV, "test-user", "customer").unwrap();
        assert!(out.contains("[options.customer.\"user 1\"]"), "{out}");
        assert!(out.contains("[options.customer.\"user 2\"]"), "{out}");
        assert!(!out.contains("test-user"), "{out}");
        let env = reparses_env(&out);
        assert_eq!(env.options["customer"]["user 1"].values["user_id"], "1001");
        assert!(env.options.contains_key("tier"), "other selectors stay put");
        assert_eq!(env.values["base_url"], "https://qa.example.com");
    }

    #[test]
    fn rename_selector_options_no_ops_without_the_selector_and_refuses_a_merge() {
        assert_eq!(rename_selector_options(ENV, "ghost", "x").unwrap(), ENV);
        assert_eq!(rename_selector_options("", "test-user", "x").unwrap(), "");
        assert!(matches!(
            rename_selector_options(ENV, "test-user", "tier").unwrap_err(),
            EditError::Conflict(_)
        ));
    }

    #[test]
    fn rename_env_var_renames_the_flat_pair_preserving_its_own_comment() {
        let env = r#"# environments/qa.toml

base_url = "https://qa.example.com"
# staging region override, remove once qa2 is retired
region = "eu-west"

[options.tier.gold]
tier = "g-1"
"#;
        let out = rename_env_var(env, "region", "aws_region").unwrap();
        assert_eq!(
            out,
            r#"# environments/qa.toml

base_url = "https://qa.example.com"
# staging region override, remove once qa2 is retired
aws_region = "eu-west"

[options.tier.gold]
tier = "g-1"
"#
        );
    }

    #[test]
    fn rename_env_var_renames_the_options_table_key() {
        let out = rename_env_var(ENV, "test-user", "person").unwrap();
        let e = reparses_env(&out);
        assert!(!e.options.contains_key("test-user"));
        assert_eq!(e.options["person"]["user 1"].values["user_id"], "1001");
    }

    #[test]
    fn rename_env_var_renames_both_the_flat_pair_and_its_options_table() {
        // The Rename op doesn't know which shape each environment uses
        // ahead of time (a name can have a flat pair here and options
        // there), so it must handle either — or, as here, both — without
        // erroring.
        let env = r#"shard = "d-1"

[options.shard.east]
shard = "e-1"
"#;
        let out = rename_env_var(env, "shard", "region").unwrap();
        assert_eq!(
            out,
            r#"region = "d-1"

[options.region.east]
shard = "e-1"
"#
        );
    }

    #[test]
    fn rename_env_var_conflict_when_to_flat_pair_already_exists() {
        let env = "base_url = \"https://qa.example.com\"\nregion = \"us-east\"\n";
        let err = rename_env_var(env, "base_url", "region").unwrap_err();
        assert_eq!(
            err,
            EditError::Conflict(
                "\"region\" already exists in this environment; rename would merge two options into one"
                    .to_string()
            )
        );
    }

    #[test]
    fn rename_env_var_conflict_when_to_options_table_already_exists() {
        let err = rename_env_var(ENV, "tier", "test-user").unwrap_err();
        assert!(matches!(err, EditError::Conflict(_)));
    }

    #[test]
    fn rename_env_var_is_a_no_op_when_the_name_is_absent() {
        let out = rename_env_var(ENV, "nope", "still-nope").unwrap();
        assert_eq!(
            out, ENV,
            "an environment with nothing to rename must come back unchanged, not error"
        );
    }

    #[test]
    fn delete_env_var_removes_the_flat_pair_and_the_options_table() {
        let env = r#"base_url = "https://qa.example.com"
shard = "d-1"

[options.shard.east]
shard = "e-1"

[options.tier.gold]
tier = "g-1"
"#;
        let out = delete_env_var(env, "shard").unwrap();
        let e = reparses_env(&out);
        assert!(!e.values.contains_key("shard"));
        assert!(!e.options.contains_key("shard"));
        assert_eq!(e.options["tier"]["gold"].values["tier"], "g-1");
    }

    #[test]
    fn delete_env_var_flat_pair_only() {
        let out = delete_env_var(ENV, "base_url").unwrap();
        let e = reparses_env(&out);
        assert!(e.values.is_empty());
        assert_eq!(e.options.len(), 2);
    }

    #[test]
    fn delete_env_var_options_table_only() {
        let out = delete_env_var(ENV, "tier").unwrap();
        let e = reparses_env(&out);
        assert!(!e.options.contains_key("tier"));
        assert_eq!(e.values["base_url"], "https://qa.example.com");
    }

    #[test]
    fn delete_env_var_is_a_no_op_when_the_name_is_absent() {
        let out = delete_env_var(ENV, "nope").unwrap();
        assert_eq!(out, ENV);
    }

    // -------------------------------------------------------------
    // scan_usage / promote_var
    // -------------------------------------------------------------

    fn req_with(
        url: &str,
        params: &[(&str, &str)],
        headers: &[(&str, &str)],
        variables: &[(&str, &str)],
        body: Option<&str>,
    ) -> crate::model::HttpRequest {
        use crate::model::{Body, Entry, Method};
        let option = |v: &str| Entry {
            value: v.to_string(),
            enabled: true,
        };
        crate::model::HttpRequest {
            name: None,
            method: Method::Get,
            url: url.to_string(),
            substitute_body: false,
            insecure: false,
            jq: None,
            jq_enabled: true,
            params: params
                .iter()
                .map(|(k, v)| (k.to_string(), option(v)))
                .collect(),
            headers: headers
                .iter()
                .map(|(k, v)| (k.to_string(), option(v)))
                .collect(),
            variables: variables
                .iter()
                .map(|(k, v)| (k.to_string(), option(v)))
                .collect(),
            body: body.map(|t| Body::Json {
                text: t.to_string(),
            }),
        }
    }

    #[test]
    fn scan_usage_finds_tokens_in_every_field_and_ignores_other_names() {
        let dir = tempfile::tempdir().unwrap();
        crate::storage::ensure_project(dir.path()).unwrap();

        // url
        crate::storage::save_request(
            dir.path(),
            "main/in-url",
            &req_with("https://x.test/{{base_url}}", &[], &[], &[], None),
        )
        .unwrap();
        // params
        crate::storage::save_request(
            dir.path(),
            "main/in-params",
            &req_with("https://x.test", &[("q", "{{base_url}}")], &[], &[], None),
        )
        .unwrap();
        // headers
        crate::storage::save_request(
            dir.path(),
            "main/in-headers",
            &req_with(
                "https://x.test",
                &[],
                &[("X-Auth", "{{base_url}}")],
                &[],
                None,
            ),
        )
        .unwrap();
        // [variables] value
        crate::storage::save_request(
            dir.path(),
            "main/in-variables",
            &req_with(
                "https://x.test",
                &[],
                &[],
                &[("local", "{{base_url}}")],
                None,
            ),
        )
        .unwrap();
        // body
        crate::storage::save_request(
            dir.path(),
            "main/in-body",
            &req_with(
                "https://x.test",
                &[],
                &[],
                &[],
                Some(r#"{"root": "{{base_url}}"}"#),
            ),
        )
        .unwrap();
        // a different token only: must be ignored
        crate::storage::save_request(
            dir.path(),
            "main/unrelated",
            &req_with("https://x.test/{{other}}", &[], &[], &[], None),
        )
        .unwrap();
        // no token at all
        crate::storage::save_request(
            dir.path(),
            "main/none",
            &req_with("https://x.test", &[], &[], &[], None),
        )
        .unwrap();

        let mut hits = scan_usage(dir.path(), "base_url");
        hits.sort();
        assert_eq!(
            hits,
            [
                "main/in-body",
                "main/in-headers",
                "main/in-params",
                "main/in-url",
                "main/in-variables",
            ]
        );
    }

    #[test]
    fn scan_usage_empty_when_no_requests_reference_the_name() {
        let dir = tempfile::tempdir().unwrap();
        crate::storage::ensure_project(dir.path()).unwrap();
        crate::storage::save_request(
            dir.path(),
            "main/a",
            &req_with("https://x.test/{{other}}", &[], &[], &[], None),
        )
        .unwrap();
        assert!(scan_usage(dir.path(), "base_url").is_empty());
    }

    #[test]
    fn promote_var_to_default_writes_the_declaration_default() {
        let (vars_out, env_out) =
            promote_var(VARS, Some(ENV), "new_var", "hello", PromoteTarget::Default).unwrap();
        assert!(env_out.is_none());
        assert!(vars_out.ends_with("[new_var]\ndefault = \"hello\"\n"));
        assert_eq!(
            reparses_vars(&vars_out).vars["new_var"].default.as_deref(),
            Some("hello")
        );
    }

    #[test]
    fn promote_var_to_env_writes_flat_pair_and_bare_declaration() {
        let (vars_out, env_out) =
            promote_var(VARS, Some(ENV), "new_var", "hello", PromoteTarget::Env).unwrap();
        let m = reparses_vars(&vars_out);
        assert_eq!(m.vars["new_var"], crate::varmodel::VarDecl::default());
        let e = reparses_env(&env_out.unwrap());
        assert_eq!(e.values["new_var"], "hello");
    }

    #[test]
    fn promote_var_onto_existing_selector_name_is_conflict() {
        let err =
            promote_var(VARS, Some(ENV), "test-user", "x", PromoteTarget::Default).unwrap_err();
        assert!(matches!(err, EditError::Conflict(_)));
    }

    #[test]
    fn promote_var_onto_existing_secret_name_is_conflict() {
        let err = promote_var(VARS, Some(ENV), "api_key", "x", PromoteTarget::Default).unwrap_err();
        assert!(matches!(err, EditError::Conflict(_)));
    }

    #[test]
    fn promote_var_onto_existing_selector_field_is_conflict() {
        let err = promote_var(VARS, Some(ENV), "user_id", "x", PromoteTarget::Default).unwrap_err();
        assert_eq!(
            err,
            EditError::Conflict(
                "variable \"user_id\" is a field of selector \"test-user\"; its value comes from the selector's selected option"
                    .to_string()
            )
        );
    }
}

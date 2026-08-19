//! Surgical `toml_edit` mutations for `variables.toml` and
//! `environments/<env>.toml` (spec §5, §7).
//!
//! Every function here is pure text -> text: parse the document with
//! `toml_edit::DocumentMut`, mutate only the addressed item, and return
//! `doc.to_string()`. Write fidelity — comments, blank lines, ordering, and
//! unrelated entries survive untouched — is the whole point (spec §7); see
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
/// `implicit_if_new = true` so an empty ancestor (e.g. `options`) never
/// prints its own header — only its leaves do, matching spec §1.1's
/// `[user.options.alice]` style. A table that is itself the addressed
/// target (e.g. `[base_url]`, `[groups.g.options.alice]`) should pass
/// `false` so it always renders, even with no fields yet.
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
/// for a top-level table header (variable/group rename — table header
/// *print* order in toml_edit follows each table's own recorded
/// `doc_position`, not map order, so this is a no-op risk there) and for a
/// flat `key = value` field inside a table (member key rename inside a
/// group option row — flat field print order *is* map order, so this
/// matters there). Rebuilding unconditionally keeps both cases correct
/// without needing to special-case which situation we're in.
fn rename_key(parent: &mut Table, from: &str, to: &str) {
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

/// Whether `name` already occupies either namespace a variable/group
/// declaration could collide with: a top-level key, or a `[groups.<name>]`
/// entry — variable and group names share one namespace (spec §1), so a
/// rename target must be checked against both.
fn name_exists(root: &Table, name: &str) -> bool {
    root.contains_key(name)
        || root
            .get("groups")
            .and_then(Item::as_table)
            .is_some_and(|g| g.contains_key(name))
}

/// Finds the table that owns shared options: either a top-level variable
/// table `[owner]`, or a group table `[groups.owner]`.
fn locate_owner_table<'a>(root: &'a mut Table, owner: &str) -> Result<&'a mut Table, EditError> {
    if root.contains_key(owner) {
        return root
            .get_mut(owner)
            .and_then(Item::as_table_mut)
            .ok_or_else(|| EditError::Parse(format!("\"{owner}\" exists but is not a table")));
    }
    if let Some(groups_table) = root.get_mut("groups").and_then(Item::as_table_mut)
        && groups_table.contains_key(owner)
    {
        return groups_table
            .get_mut(owner)
            .and_then(Item::as_table_mut)
            .ok_or_else(|| EditError::Parse(format!("\"{owner}\" exists but is not a table")));
    }
    Err(not_found(format!(
        "\"{owner}\" is not a declared variable or group"
    )))
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
/// children of its own (e.g. `options`, kept invisible on purpose per spec
/// §1.1 so only its leaves like `[user.options.alice]` print) never prints
/// its own decor, so this recurses into such a table's first child to find
/// the entry that will actually be first on the page.
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

/// Sets/clears `secret = true` on a variable. Turning secret on removes any
/// `default` (a default would commit a secret value into the shared file)
/// and is a `Conflict` if the variable declares `options`.
pub fn set_secret_flag(doc: &str, name: &str, secret: bool) -> Result<String, EditError> {
    let mut doc = parse(doc)?;
    let root = doc.as_table_mut();
    let t = root
        .get_mut(name)
        .and_then(Item::as_table_mut)
        .ok_or_else(|| not_found(format!("variable \"{name}\" not found")))?;
    if secret {
        if t.get("options").is_some() {
            return Err(EditError::Conflict(format!(
                "variable \"{name}\" has options; remove them before marking it secret"
            )));
        }
        t.remove("default");
        t["secret"] = value(true);
    } else {
        t.remove("secret");
    }
    Ok(doc.to_string())
}

/// Renames a variable's table header, cascading into every group's
/// `members` array and, for groups the variable belongs to, the
/// corresponding member field key inside each `[groups.<g>.options.<key>]`
/// row.
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

    if let Some(groups_table) = root.get_mut("groups").and_then(Item::as_table_mut) {
        let group_names: Vec<String> = groups_table.iter().map(|(k, _)| k.to_string()).collect();
        for gname in group_names {
            let Some(group_table) = groups_table.get_mut(&gname).and_then(Item::as_table_mut)
            else {
                continue;
            };

            let mut is_member = false;
            if let Some(arr) = group_table.get_mut("members").and_then(Item::as_array_mut) {
                for v in arr.iter_mut() {
                    if v.as_str() == Some(from) {
                        let decor = v.decor().clone();
                        *v = Value::from(to);
                        *v.decor_mut() = decor;
                        is_member = true;
                    }
                }
            }

            if is_member
                && let Some(options_table) =
                    group_table.get_mut("options").and_then(Item::as_table_mut)
            {
                let option_keys: Vec<String> =
                    options_table.iter().map(|(k, _)| k.to_string()).collect();
                for okey in option_keys {
                    if let Some(opt_table) =
                        options_table.get_mut(&okey).and_then(Item::as_table_mut)
                        && opt_table.contains_key(from)
                    {
                        rename_key(opt_table, from, to);
                    }
                }
            }
        }
    }

    Ok(doc.to_string())
}

/// Deletes a variable's table. `Conflict` if it's still a member of any
/// group (remove it from the group first).
pub fn delete_var(doc: &str, name: &str) -> Result<String, EditError> {
    let mut doc = parse(doc)?;
    let root = doc.as_table_mut();
    if !root.contains_key(name) {
        return Err(not_found(format!("variable \"{name}\" not found")));
    }
    if let Some(groups_table) = root.get("groups").and_then(Item::as_table) {
        for (gname, gitem) in groups_table.iter() {
            let Some(members) = gitem
                .as_table()
                .and_then(|t| t.get("members"))
                .and_then(Item::as_array)
            else {
                continue;
            };
            if members.iter().any(|v| v.as_str() == Some(name)) {
                return Err(EditError::Conflict(format!(
                    "variable \"{name}\" is a member of group \"{gname}\"; remove it from the group first"
                )));
            }
        }
    }
    remove_transferring_header(root, name);
    Ok(doc.to_string())
}

/// Creates or updates a keyed shared option under a variable or group:
/// `[owner.options.key]` for a variable (`value_or_members` is `{"value":
/// v}`), `[groups.owner.options.key]` for a group (`value_or_members` is
/// the member-name -> value map).
pub fn upsert_shared_option(
    doc: &str,
    owner: &str,
    key: &str,
    description: Option<&str>,
    value_or_members: &IndexMap<String, String>,
) -> Result<String, EditError> {
    let mut doc = parse(doc)?;
    let root = doc.as_table_mut();
    let owner_table = locate_owner_table(root, owner)?;
    let options = table_mut(owner_table, "options", true)?;
    let opt = table_mut(options, key, false)?;
    if let Some(d) = description {
        opt["description"] = value(d);
    }
    for (k, v) in value_or_members {
        opt[k.as_str()] = value(v.as_str());
    }
    Ok(doc.to_string())
}

/// Removes a keyed shared option from a variable or group.
pub fn delete_shared_option(doc: &str, owner: &str, key: &str) -> Result<String, EditError> {
    let mut doc = parse(doc)?;
    let root = doc.as_table_mut();
    let owner_table = locate_owner_table(root, owner)?;
    let removed = owner_table
        .get_mut("options")
        .and_then(Item::as_table_mut)
        .and_then(|t| remove_transferring_header(t, key));
    if removed.is_none() {
        return Err(not_found(format!(
            "option \"{key}\" not found on \"{owner}\""
        )));
    }
    Ok(doc.to_string())
}

/// Creates the group's table if absent, sets `description` when given, and
/// always sets `members` to the given list.
pub fn upsert_group(
    doc: &str,
    name: &str,
    description: Option<&str>,
    members: &[String],
) -> Result<String, EditError> {
    let mut doc = parse(doc)?;
    let root = doc.as_table_mut();
    let groups = table_mut(root, "groups", true)?;
    let g = table_mut(groups, name, false)?;
    if let Some(d) = description {
        g["description"] = value(d);
    }
    let mut arr = Array::new();
    for m in members {
        arr.push(m.as_str());
    }
    g["members"] = Item::Value(Value::Array(arr));
    Ok(doc.to_string())
}

/// Removes `member` from `group`'s member list and strips its value from
/// every `[groups.<group>.options.*]` table — a member's per-option values
/// must not outlive its membership (the model rejects options naming a
/// non-member). `NotFound` if the group doesn't exist or `member` isn't in
/// its list.
pub fn remove_group_member(doc: &str, group: &str, member: &str) -> Result<String, EditError> {
    let mut doc = parse(doc)?;
    let root = doc.as_table_mut();
    let g = root
        .get_mut("groups")
        .and_then(Item::as_table_mut)
        .and_then(|groups| groups.get_mut(group))
        .and_then(Item::as_table_mut)
        .ok_or_else(|| not_found(format!("group \"{group}\" not found")))?;
    let members = g
        .get_mut("members")
        .and_then(Item::as_value_mut)
        .and_then(Value::as_array_mut)
        .ok_or_else(|| not_found(format!("group \"{group}\" has no members list")))?;
    let before = members.len();
    members.retain(|v| v.as_str() != Some(member));
    if members.len() == before {
        return Err(not_found(format!(
            "\"{member}\" is not a member of \"{group}\""
        )));
    }
    if let Some(options) = g.get_mut("options").and_then(Item::as_table_mut) {
        for (_, opt) in options.iter_mut() {
            if let Some(t) = opt.as_table_mut() {
                t.remove(member);
            }
        }
    }
    Ok(doc.to_string())
}

/// Deletes a group's table (and everything nested under it, including its
/// options). `NotFound` if there is no such group.
pub fn delete_group(doc: &str, name: &str) -> Result<String, EditError> {
    let mut doc = parse(doc)?;
    let root = doc.as_table_mut();
    let removed = root
        .get_mut("groups")
        .and_then(Item::as_table_mut)
        .and_then(|g| remove_transferring_header(g, name));
    if removed.is_none() {
        return Err(not_found(format!("group \"{name}\" not found")));
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

/// Creates or updates `[options.owner.key]`, setting every field given in
/// `fields` (e.g. `description`, `value`, or per-member values for a
/// group's option row).
pub fn upsert_env_option(
    doc: &str,
    owner: &str,
    key: &str,
    fields: &IndexMap<String, String>,
) -> Result<String, EditError> {
    let mut doc = parse(doc)?;
    let root = doc.as_table_mut();
    let options = table_mut(root, "options", true)?;
    let owner_table = table_mut(options, owner, true)?;
    let opt = table_mut(owner_table, key, false)?;
    for (k, v) in fields {
        opt[k.as_str()] = value(v.as_str());
    }
    Ok(doc.to_string())
}

/// Renames `from` to `to` wherever `from` appears in an environment file:
/// its flat `from = "value"` pair (if any) and its `[options.from.*]`
/// table (if any) — the cascade `rename_var` itself doesn't (and can't;
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
        .is_some_and(|o| o.contains_key(from));
    if !has_flat && !has_options {
        return Ok(doc.to_string());
    }
    // Conflict check up front, before any mutation: a `to` that already
    // occupies the same slot `from` is being moved into would otherwise
    // silently merge two entries into one (`rename_key`'s rebuild would
    // encounter `to` twice and the second insert wins, dropping the first).
    if from != to {
        let flat_conflict = has_flat && root.contains_key(to);
        let options_conflict = has_options
            && root
                .get("options")
                .and_then(Item::as_table)
                .is_some_and(|o| o.contains_key(to));
        if flat_conflict || options_conflict {
            return Err(EditError::Conflict(format!(
                "\"{to}\" already exists in this environment; rename would merge two entries into one"
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
/// - an enumerated variable (one with an `options` table) — writing over
///   it would either clobber its options or leave promote's meaning
///   ambiguous;
/// - an existing group's own name — group and variable names share one
///   namespace (spec §1), so `upsert_var` would otherwise create a
///   colliding top-level variable table alongside `[groups.<name>]`;
/// - an existing group's member — that name's value comes from the
///   group's selected option, so writing a `default` onto it would be
///   dead and misleading.
///
/// In every conflict case the caller should offer a rename instead. The
/// caller is responsible for removing the request's own `[variables]`
/// entry.
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
        .is_some_and(|t| t.contains_key("options"))
    {
        return Err(EditError::Conflict(format!(
            "variable \"{name}\" is enumerated; promoting would overwrite its options"
        )));
    }

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

    if let Some(groups_table) = root.get("groups").and_then(Item::as_table) {
        if groups_table.contains_key(name) {
            return Err(EditError::Conflict(format!(
                "\"{name}\" is already a group name; group and variable names share one namespace"
            )));
        }
        for (gname, gitem) in groups_table.iter() {
            let is_member = gitem
                .as_table()
                .and_then(|t| t.get("members"))
                .and_then(Item::as_array)
                .is_some_and(|members| members.iter().any(|v| v.as_str() == Some(name)));
            if is_member {
                return Err(EditError::Conflict(format!(
                    "variable \"{name}\" is a member of group \"{gname}\"; its value comes from the group's selected option"
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
/// "value"` pair (if any) and its whole `[options.name.*]` table (if any).
/// No-op (returns `doc` unchanged) when neither exists — mirrors
/// `rename_env_var`'s per-env-loop-friendly behavior, since the caller
/// (the Manager's delete cascade) loops this across every environment
/// unconditionally, most of which won't have anything for a given name.
pub fn delete_env_var(doc: &str, name: &str) -> Result<String, EditError> {
    let mut doc = parse(doc)?;
    let root = doc.as_table_mut();
    remove_transferring_header(root, name);
    if let Some(options) = root.get_mut("options").and_then(Item::as_table_mut) {
        remove_transferring_header(options, name);
    }
    Ok(doc.to_string())
}

/// Removes `[options.owner.key]`. `NotFound` if it wasn't there.
/// Strips `member`'s value from every `[options.<group>.*]` table in an
/// environment doc — the env-file half of [`remove_group_member`]. A doc
/// with nothing to strip passes through unchanged (never an error: most
/// envs won't override the group at all).
pub fn strip_env_group_member(doc: &str, group: &str, member: &str) -> Result<String, EditError> {
    let mut doc = parse(doc)?;
    let root = doc.as_table_mut();
    if let Some(keys) = root
        .get_mut("options")
        .and_then(Item::as_table_mut)
        .and_then(|o| o.get_mut(group))
        .and_then(Item::as_table_mut)
    {
        for (_, opt) in keys.iter_mut() {
            if let Some(t) = opt.as_table_mut() {
                t.remove(member);
            }
        }
    }
    Ok(doc.to_string())
}

pub fn delete_env_option(doc: &str, owner: &str, key: &str) -> Result<String, EditError> {
    let mut doc = parse(doc)?;
    let root = doc.as_table_mut();
    let removed = root
        .get_mut("options")
        .and_then(Item::as_table_mut)
        .and_then(|o| o.get_mut(owner))
        .and_then(Item::as_table_mut)
        .and_then(|t| remove_transferring_header(t, key));
    if removed.is_none() {
        return Err(not_found(format!("[options.{owner}.{key}] not found")));
    }
    Ok(doc.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fixture mirrors spec §1.1's example almost verbatim, plus a
    /// `[user_id]` top-level table so a group member has both a shared
    /// declaration *and* group membership to exercise the rename cascade.
    const VARS: &str = r#"# variables.toml

[base_url]
description = "API root"
default = "http://localhost:8080"

[api_key]
description = "service API key"
secret = true

[user]
description = "seeded test user"
[user.options.alice]
description = "admin, active sub"
value = "1001"
[user.options.bob]
description = "expired trial"
value = "2002"

[user_id]
description = "linked user id"

[groups.test-user]
description = "user with linked customer"
members = ["user_id", "customer_id"]
[groups.test-user.options.alice]
description = "admin, active sub"
user_id = "1001"
customer_id = "c-77"
"#;

    /// Fixture mirrors spec §1.2's example.
    const ENV: &str = r#"# environments/qa.toml

base_url = "https://qa.example.com"

[options.user.alice]
value = "9001"
[options.user.qa-only]
description = "exists only in qa"
value = "3003"
[options.test-user.alice]
user_id = "9001"
"#;

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

[user]
description = "seeded test user"
[user.options.alice]
description = "admin, active sub"
value = "1001"
[user.options.bob]
description = "expired trial"
value = "2002"

[user_id]
description = "linked user id"

[groups.test-user]
description = "user with linked customer"
members = ["user_id", "customer_id"]
[groups.test-user.options.alice]
description = "admin, active sub"
user_id = "1001"
customer_id = "c-77"
"#
        );
    }

    #[test]
    fn upsert_var_creates_a_new_var_table_at_the_end() {
        let out = upsert_var(VARS, "timeout", Some("request timeout, ms"), Some("30000")).unwrap();
        assert_eq!(
            out,
            r#"# variables.toml

[base_url]
description = "API root"
default = "http://localhost:8080"

[api_key]
description = "service API key"
secret = true

[user]
description = "seeded test user"
[user.options.alice]
description = "admin, active sub"
value = "1001"
[user.options.bob]
description = "expired trial"
value = "2002"

[user_id]
description = "linked user id"

[groups.test-user]
description = "user with linked customer"
members = ["user_id", "customer_id"]
[groups.test-user.options.alice]
description = "admin, active sub"
user_id = "1001"
customer_id = "c-77"

[timeout]
description = "request timeout, ms"
default = "30000"
"#
        );
    }

    #[test]
    fn set_secret_flag_true_strips_default_and_sets_secret() {
        let out = set_secret_flag(VARS, "base_url", true).unwrap();
        assert_eq!(
            out,
            r#"# variables.toml

[base_url]
description = "API root"
secret = true

[api_key]
description = "service API key"
secret = true

[user]
description = "seeded test user"
[user.options.alice]
description = "admin, active sub"
value = "1001"
[user.options.bob]
description = "expired trial"
value = "2002"

[user_id]
description = "linked user id"

[groups.test-user]
description = "user with linked customer"
members = ["user_id", "customer_id"]
[groups.test-user.options.alice]
description = "admin, active sub"
user_id = "1001"
customer_id = "c-77"
"#
        );
    }

    #[test]
    fn set_secret_flag_true_is_conflict_when_var_has_options() {
        let err = set_secret_flag(VARS, "user", true).unwrap_err();
        assert_eq!(
            err,
            EditError::Conflict(
                "variable \"user\" has options; remove them before marking it secret".to_string()
            )
        );
    }

    #[test]
    fn set_secret_flag_false_removes_the_secret_line() {
        let out = set_secret_flag(VARS, "api_key", false).unwrap();
        assert_eq!(
            out,
            r#"# variables.toml

[base_url]
description = "API root"
default = "http://localhost:8080"

[api_key]
description = "service API key"

[user]
description = "seeded test user"
[user.options.alice]
description = "admin, active sub"
value = "1001"
[user.options.bob]
description = "expired trial"
value = "2002"

[user_id]
description = "linked user id"

[groups.test-user]
description = "user with linked customer"
members = ["user_id", "customer_id"]
[groups.test-user.options.alice]
description = "admin, active sub"
user_id = "1001"
customer_id = "c-77"
"#
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
        assert_eq!(
            out,
            r#"# variables.toml

[root_url]
description = "API root"
default = "http://localhost:8080"

[api_key]
description = "service API key"
secret = true

[user]
description = "seeded test user"
[user.options.alice]
description = "admin, active sub"
value = "1001"
[user.options.bob]
description = "expired trial"
value = "2002"

[user_id]
description = "linked user id"

[groups.test-user]
description = "user with linked customer"
members = ["user_id", "customer_id"]
[groups.test-user.options.alice]
description = "admin, active sub"
user_id = "1001"
customer_id = "c-77"
"#
        );
    }

    #[test]
    fn rename_var_cascades_into_group_members_and_group_option_keys() {
        let out = rename_var(VARS, "user_id", "account_id").unwrap();
        assert_eq!(
            out,
            r#"# variables.toml

[base_url]
description = "API root"
default = "http://localhost:8080"

[api_key]
description = "service API key"
secret = true

[user]
description = "seeded test user"
[user.options.alice]
description = "admin, active sub"
value = "1001"
[user.options.bob]
description = "expired trial"
value = "2002"

[account_id]
description = "linked user id"

[groups.test-user]
description = "user with linked customer"
members = ["account_id", "customer_id"]
[groups.test-user.options.alice]
description = "admin, active sub"
account_id = "1001"
customer_id = "c-77"
"#
        );
    }

    #[test]
    fn rename_var_conflict_when_to_already_exists_as_a_variable() {
        let err = rename_var(VARS, "base_url", "api_key").unwrap_err();
        assert_eq!(
            err,
            EditError::Conflict(
                "\"api_key\" already exists; rename would merge two declarations into one"
                    .to_string()
            )
        );
        // Nothing changed on a rejected rename.
        assert!(rename_var(VARS, "base_url", "api_key").is_err());
    }

    #[test]
    fn rename_var_conflict_when_to_already_exists_as_a_group() {
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
        let out = delete_var(VARS, "base_url").unwrap();
        assert_eq!(
            out,
            r#"# variables.toml

[api_key]
description = "service API key"
secret = true

[user]
description = "seeded test user"
[user.options.alice]
description = "admin, active sub"
value = "1001"
[user.options.bob]
description = "expired trial"
value = "2002"

[user_id]
description = "linked user id"

[groups.test-user]
description = "user with linked customer"
members = ["user_id", "customer_id"]
[groups.test-user.options.alice]
description = "admin, active sub"
user_id = "1001"
customer_id = "c-77"
"#
        );
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
    fn delete_var_conflict_when_still_a_group_member() {
        let err = delete_var(VARS, "user_id").unwrap_err();
        assert_eq!(
            err,
            EditError::Conflict(
                "variable \"user_id\" is a member of group \"test-user\"; remove it from the group first"
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
    fn upsert_shared_option_adds_a_new_keyed_option_on_a_var() {
        let mut fields = IndexMap::new();
        fields.insert("value".to_string(), "3003".to_string());
        let out = upsert_shared_option(VARS, "user", "carol", Some("qa only"), &fields).unwrap();
        assert_eq!(
            out,
            r#"# variables.toml

[base_url]
description = "API root"
default = "http://localhost:8080"

[api_key]
description = "service API key"
secret = true

[user]
description = "seeded test user"
[user.options.alice]
description = "admin, active sub"
value = "1001"
[user.options.bob]
description = "expired trial"
value = "2002"

[user.options.carol]
description = "qa only"
value = "3003"

[user_id]
description = "linked user id"

[groups.test-user]
description = "user with linked customer"
members = ["user_id", "customer_id"]
[groups.test-user.options.alice]
description = "admin, active sub"
user_id = "1001"
customer_id = "c-77"
"#
        );
    }

    #[test]
    fn upsert_shared_option_updates_an_existing_option_value() {
        let mut fields = IndexMap::new();
        fields.insert("value".to_string(), "9999".to_string());
        let out = upsert_shared_option(VARS, "user", "alice", None, &fields).unwrap();
        assert_eq!(
            out,
            r#"# variables.toml

[base_url]
description = "API root"
default = "http://localhost:8080"

[api_key]
description = "service API key"
secret = true

[user]
description = "seeded test user"
[user.options.alice]
description = "admin, active sub"
value = "9999"
[user.options.bob]
description = "expired trial"
value = "2002"

[user_id]
description = "linked user id"

[groups.test-user]
description = "user with linked customer"
members = ["user_id", "customer_id"]
[groups.test-user.options.alice]
description = "admin, active sub"
user_id = "1001"
customer_id = "c-77"
"#
        );
    }

    #[test]
    fn upsert_shared_option_adds_a_new_option_row_on_a_group() {
        let mut members = IndexMap::new();
        members.insert("user_id".to_string(), "3003".to_string());
        members.insert("customer_id".to_string(), "c-99".to_string());
        let out =
            upsert_shared_option(VARS, "test-user", "carol", Some("qa only"), &members).unwrap();
        assert_eq!(
            out,
            r#"# variables.toml

[base_url]
description = "API root"
default = "http://localhost:8080"

[api_key]
description = "service API key"
secret = true

[user]
description = "seeded test user"
[user.options.alice]
description = "admin, active sub"
value = "1001"
[user.options.bob]
description = "expired trial"
value = "2002"

[user_id]
description = "linked user id"

[groups.test-user]
description = "user with linked customer"
members = ["user_id", "customer_id"]
[groups.test-user.options.alice]
description = "admin, active sub"
user_id = "1001"
customer_id = "c-77"

[groups.test-user.options.carol]
description = "qa only"
user_id = "3003"
customer_id = "c-99"
"#
        );
    }

    #[test]
    fn upsert_shared_option_not_found_on_missing_owner() {
        let mut fields = IndexMap::new();
        fields.insert("value".to_string(), "1".to_string());
        let err = upsert_shared_option(VARS, "nope", "k", None, &fields).unwrap_err();
        assert_eq!(
            err,
            EditError::NotFound("\"nope\" is not a declared variable or group".to_string())
        );
    }

    #[test]
    fn delete_shared_option_removes_the_keyed_option() {
        let out = delete_shared_option(VARS, "user", "bob").unwrap();
        assert_eq!(
            out,
            r#"# variables.toml

[base_url]
description = "API root"
default = "http://localhost:8080"

[api_key]
description = "service API key"
secret = true

[user]
description = "seeded test user"
[user.options.alice]
description = "admin, active sub"
value = "1001"

[user_id]
description = "linked user id"

[groups.test-user]
description = "user with linked customer"
members = ["user_id", "customer_id"]
[groups.test-user.options.alice]
description = "admin, active sub"
user_id = "1001"
customer_id = "c-77"
"#
        );
    }

    #[test]
    fn delete_shared_option_not_found_on_missing_key() {
        let err = delete_shared_option(VARS, "user", "nope").unwrap_err();
        assert_eq!(
            err,
            EditError::NotFound("option \"nope\" not found on \"user\"".to_string())
        );
    }

    #[test]
    fn remove_group_member_strips_the_list_and_every_options_value() {
        let out = remove_group_member(VARS, "test-user", "user_id").unwrap();
        let model = crate::varmodel::parse_variables(&out).unwrap();
        let g = model.groups.get("test-user").unwrap();
        assert_eq!(g.members, vec!["customer_id".to_string()]);
        assert!(
            !g.options
                .get("alice")
                .unwrap()
                .values
                .contains_key("user_id"),
            "the removed member's per-option values go too: {out}"
        );
        assert!(
            g.options
                .get("alice")
                .unwrap()
                .values
                .contains_key("customer_id"),
            "other members' values stay"
        );
        // the standalone [user_id] declaration is untouched
        assert!(model.vars.contains_key("user_id"));

        assert!(remove_group_member(VARS, "test-user", "nope").is_err());
        assert!(remove_group_member(VARS, "no-such-group", "user_id").is_err());
    }

    #[test]
    fn strip_env_group_member_removes_only_that_members_option_values() {
        let out = strip_env_group_member(ENV, "test-user", "user_id").unwrap();
        assert!(
            !out.contains("[options.test-user.alice]") || !out.contains("user_id = \"9001\""),
            "{out}"
        );
        // untouched: the plain var's options and the flat value
        assert!(out.contains("[options.user.alice]"), "{out}");
        assert!(out.contains("base_url = "), "{out}");
        let parsed = crate::varmodel::parse_environment(&out).unwrap();
        assert!(
            parsed
                .options
                .get("test-user")
                .and_then(|t| t.get("alice"))
                .is_none_or(|o| !o.contains_key("user_id")),
            "{out}"
        );
        // a doc with nothing to strip passes through unchanged
        let untouched = strip_env_group_member("x = \"1\"\n", "test-user", "user_id").unwrap();
        assert_eq!(untouched, "x = \"1\"\n");
    }

    #[test]
    fn upsert_group_with_no_members_writes_an_empty_list_that_reparses() {
        let out = upsert_group(VARS, "billing-pair", None, &[]).unwrap();
        assert!(out.contains("[groups.billing-pair]"), "{out}");
        assert!(out.contains("members = []"), "{out}");
        let model = crate::varmodel::parse_variables(&out).unwrap();
        assert!(
            model.groups.get("billing-pair").unwrap().members.is_empty(),
            "an empty group parses back"
        );
    }

    #[test]
    fn upsert_group_creates_a_new_group_table() {
        let members = vec!["order_id".to_string(), "invoice_id".to_string()];
        let out = upsert_group(VARS, "billing-pair", Some("billing linkage"), &members).unwrap();
        assert_eq!(
            out,
            r#"# variables.toml

[base_url]
description = "API root"
default = "http://localhost:8080"

[api_key]
description = "service API key"
secret = true

[user]
description = "seeded test user"
[user.options.alice]
description = "admin, active sub"
value = "1001"
[user.options.bob]
description = "expired trial"
value = "2002"

[user_id]
description = "linked user id"

[groups.test-user]
description = "user with linked customer"
members = ["user_id", "customer_id"]
[groups.test-user.options.alice]
description = "admin, active sub"
user_id = "1001"
customer_id = "c-77"

[groups.billing-pair]
description = "billing linkage"
members = ["order_id", "invoice_id"]
"#
        );
    }

    #[test]
    fn upsert_group_replaces_members_on_an_existing_group() {
        let members = vec![
            "user_id".to_string(),
            "customer_id".to_string(),
            "region".to_string(),
        ];
        let out = upsert_group(VARS, "test-user", None, &members).unwrap();
        assert_eq!(
            out,
            r#"# variables.toml

[base_url]
description = "API root"
default = "http://localhost:8080"

[api_key]
description = "service API key"
secret = true

[user]
description = "seeded test user"
[user.options.alice]
description = "admin, active sub"
value = "1001"
[user.options.bob]
description = "expired trial"
value = "2002"

[user_id]
description = "linked user id"

[groups.test-user]
description = "user with linked customer"
members = ["user_id", "customer_id", "region"]
[groups.test-user.options.alice]
description = "admin, active sub"
user_id = "1001"
customer_id = "c-77"
"#
        );
    }

    #[test]
    fn delete_group_removes_the_group_and_its_options() {
        let out = delete_group(VARS, "test-user").unwrap();
        assert_eq!(
            out,
            r#"# variables.toml

[base_url]
description = "API root"
default = "http://localhost:8080"

[api_key]
description = "service API key"
secret = true

[user]
description = "seeded test user"
[user.options.alice]
description = "admin, active sub"
value = "1001"
[user.options.bob]
description = "expired trial"
value = "2002"

[user_id]
description = "linked user id"
"#
        );
    }

    #[test]
    fn delete_group_not_found_on_missing_group() {
        let err = delete_group(VARS, "nope").unwrap_err();
        assert_eq!(
            err,
            EditError::NotFound("group \"nope\" not found".to_string())
        );
    }

    #[test]
    fn set_env_value_adds_a_new_flat_pair() {
        let out = set_env_value(ENV, "region", Some("us-east")).unwrap();
        assert_eq!(
            out,
            r#"# environments/qa.toml

base_url = "https://qa.example.com"
region = "us-east"

[options.user.alice]
value = "9001"
[options.user.qa-only]
description = "exists only in qa"
value = "3003"
[options.test-user.alice]
user_id = "9001"
"#
        );
    }

    #[test]
    fn set_env_value_updates_an_existing_flat_pair() {
        let out = set_env_value(ENV, "base_url", Some("https://qa2.example.com")).unwrap();
        assert_eq!(
            out,
            r#"# environments/qa.toml

base_url = "https://qa2.example.com"

[options.user.alice]
value = "9001"
[options.user.qa-only]
description = "exists only in qa"
value = "3003"
[options.test-user.alice]
user_id = "9001"
"#
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

[options.user.alice]
value = "9001"
"#;
        let out = set_env_value(env, "region", Some("us-east")).unwrap();
        assert_eq!(
            out,
            r#"# environments/qa.toml

base_url = "https://qa.example.com"
# staging region override, remove once qa2 is retired
region = "us-east"

[options.user.alice]
value = "9001"
"#
        );
    }

    #[test]
    fn set_env_value_none_removes_the_flat_pair() {
        let out = set_env_value(ENV, "base_url", None).unwrap();
        assert_eq!(
            out,
            r#"# environments/qa.toml

[options.user.alice]
value = "9001"
[options.user.qa-only]
description = "exists only in qa"
value = "3003"
[options.test-user.alice]
user_id = "9001"
"#
        );
    }

    #[test]
    fn set_env_value_none_not_found_when_pair_absent() {
        let err = set_env_value(ENV, "nope", None).unwrap_err();
        assert_eq!(err, EditError::NotFound("\"nope\" not found".to_string()));
    }

    #[test]
    fn rename_env_var_renames_the_flat_pair_preserving_its_own_comment() {
        let env = r#"# environments/qa.toml

base_url = "https://qa.example.com"
# staging region override, remove once qa2 is retired
region = "eu-west"

[options.user.alice]
value = "9001"
"#;
        let out = rename_env_var(env, "region", "aws_region").unwrap();
        assert_eq!(
            out,
            r#"# environments/qa.toml

base_url = "https://qa.example.com"
# staging region override, remove once qa2 is retired
aws_region = "eu-west"

[options.user.alice]
value = "9001"
"#
        );
    }

    #[test]
    fn rename_env_var_renames_the_options_table_key() {
        let out = rename_env_var(ENV, "user", "person").unwrap();
        assert_eq!(
            out,
            r#"# environments/qa.toml

base_url = "https://qa.example.com"

[options.person.alice]
value = "9001"
[options.person.qa-only]
description = "exists only in qa"
value = "3003"
[options.test-user.alice]
user_id = "9001"
"#
        );
    }

    #[test]
    fn rename_env_var_renames_both_the_flat_pair_and_its_options_table() {
        // A variable can be simple in one env and enumerated in another
        // (spec §1.2), so a single env file can legitimately carry both a
        // flat pair AND an `[options.<name>]` table for two DIFFERENT
        // names — but this fixture exercises the same name appearing as
        // both in one file (the `Rename` op doesn't know which shape each
        // environment uses ahead of time, so it must handle either, or
        // both, without erroring).
        let env = r#"base_url = "https://qa.example.com"

[options.base_url.primary]
value = "https://qa2.example.com"
"#;
        let out = rename_env_var(env, "base_url", "root_url").unwrap();
        assert_eq!(
            out,
            r#"root_url = "https://qa.example.com"

[options.root_url.primary]
value = "https://qa2.example.com"
"#
        );
    }

    #[test]
    fn rename_env_var_conflict_when_to_flat_pair_already_exists() {
        // ENV has both a flat "base_url" pair and options for "user"/
        // "test-user"; renaming "base_url" onto the existing flat-pair-less
        // but options-bearing "user" is fine (no flat "user" pair), so use
        // a fixture where the target name already has a flat pair too.
        let env = r#"base_url = "https://qa.example.com"
region = "us-east"
"#;
        let err = rename_env_var(env, "base_url", "region").unwrap_err();
        assert_eq!(
            err,
            EditError::Conflict(
                "\"region\" already exists in this environment; rename would merge two entries into one"
                    .to_string()
            )
        );
    }

    #[test]
    fn rename_env_var_conflict_when_to_options_table_already_exists() {
        let err = rename_env_var(ENV, "user", "test-user").unwrap_err();
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
    fn upsert_env_option_updates_an_existing_option_field() {
        let mut fields = IndexMap::new();
        fields.insert("value".to_string(), "5005".to_string());
        let out = upsert_env_option(ENV, "user", "alice", &fields).unwrap();
        assert_eq!(
            out,
            r#"# environments/qa.toml

base_url = "https://qa.example.com"

[options.user.alice]
value = "5005"
[options.user.qa-only]
description = "exists only in qa"
value = "3003"
[options.test-user.alice]
user_id = "9001"
"#
        );
    }

    #[test]
    fn upsert_env_option_creates_a_new_owner_and_key() {
        let mut fields = IndexMap::new();
        fields.insert("description".to_string(), "new option".to_string());
        fields.insert("value".to_string(), "7007".to_string());
        let out = upsert_env_option(ENV, "test-user", "bob", &fields).unwrap();
        assert_eq!(
            out,
            r#"# environments/qa.toml

base_url = "https://qa.example.com"

[options.user.alice]
value = "9001"
[options.user.qa-only]
description = "exists only in qa"
value = "3003"
[options.test-user.alice]
user_id = "9001"

[options.test-user.bob]
description = "new option"
value = "7007"
"#
        );
    }

    #[test]
    fn delete_env_option_removes_the_keyed_option() {
        let out = delete_env_option(ENV, "user", "alice").unwrap();
        assert_eq!(
            out,
            r#"# environments/qa.toml

base_url = "https://qa.example.com"
[options.user.qa-only]
description = "exists only in qa"
value = "3003"
[options.test-user.alice]
user_id = "9001"
"#
        );
    }

    #[test]
    fn delete_env_option_not_found_on_missing_key() {
        let err = delete_env_option(ENV, "user", "nope").unwrap_err();
        assert_eq!(
            err,
            EditError::NotFound("[options.user.nope] not found".to_string())
        );
    }

    #[test]
    fn delete_env_var_removes_the_flat_pair_and_the_options_table() {
        let env = r#"base_url = "https://qa.example.com"
shard = "d-1"

[options.shard.east]
value = "e-1"
[options.user.alice]
value = "9001"
"#;
        let out = delete_env_var(env, "shard").unwrap();
        assert_eq!(
            out,
            r#"base_url = "https://qa.example.com"
[options.user.alice]
value = "9001"
"#
        );
    }

    #[test]
    fn delete_env_var_flat_pair_only() {
        let out = delete_env_var(ENV, "base_url").unwrap();
        assert_eq!(
            out,
            r#"# environments/qa.toml

[options.user.alice]
value = "9001"
[options.user.qa-only]
description = "exists only in qa"
value = "3003"
[options.test-user.alice]
user_id = "9001"
"#
        );
    }

    #[test]
    fn delete_env_var_options_table_only() {
        let out = delete_env_var(ENV, "user").unwrap();
        assert_eq!(
            out,
            r#"# environments/qa.toml

base_url = "https://qa.example.com"
[options.test-user.alice]
user_id = "9001"
"#
        );
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
        let entry = |v: &str| Entry {
            value: v.to_string(),
            enabled: true,
        };
        crate::model::HttpRequest {
            method: Method::Get,
            url: url.to_string(),
            substitute_body: false,
            params: params
                .iter()
                .map(|(k, v)| (k.to_string(), entry(v)))
                .collect(),
            headers: headers
                .iter()
                .map(|(k, v)| (k.to_string(), entry(v)))
                .collect(),
            variables: variables
                .iter()
                .map(|(k, v)| (k.to_string(), entry(v)))
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
            "in-url",
            &req_with("https://x.test/{{base_url}}", &[], &[], &[], None),
        )
        .unwrap();
        // params
        crate::storage::save_request(
            dir.path(),
            "in-params",
            &req_with("https://x.test", &[("q", "{{base_url}}")], &[], &[], None),
        )
        .unwrap();
        // headers
        crate::storage::save_request(
            dir.path(),
            "in-headers",
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
            "in-variables",
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
            "in-body",
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
            "unrelated",
            &req_with("https://x.test/{{other}}", &[], &[], &[], None),
        )
        .unwrap();
        // no token at all
        crate::storage::save_request(
            dir.path(),
            "none",
            &req_with("https://x.test", &[], &[], &[], None),
        )
        .unwrap();

        let mut hits = scan_usage(dir.path(), "base_url");
        hits.sort();
        assert_eq!(
            hits,
            [
                "in-body",
                "in-headers",
                "in-params",
                "in-url",
                "in-variables",
            ]
        );
    }

    #[test]
    fn scan_usage_empty_when_no_requests_reference_the_name() {
        let dir = tempfile::tempdir().unwrap();
        crate::storage::ensure_project(dir.path()).unwrap();
        crate::storage::save_request(
            dir.path(),
            "a",
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
        assert_eq!(
            vars_out,
            r#"# variables.toml

[base_url]
description = "API root"
default = "http://localhost:8080"

[api_key]
description = "service API key"
secret = true

[user]
description = "seeded test user"
[user.options.alice]
description = "admin, active sub"
value = "1001"
[user.options.bob]
description = "expired trial"
value = "2002"

[user_id]
description = "linked user id"

[groups.test-user]
description = "user with linked customer"
members = ["user_id", "customer_id"]
[groups.test-user.options.alice]
description = "admin, active sub"
user_id = "1001"
customer_id = "c-77"

[new_var]
default = "hello"
"#
        );
    }

    #[test]
    fn promote_var_to_env_writes_flat_pair_and_bare_declaration() {
        let (vars_out, env_out) =
            promote_var(VARS, Some(ENV), "new_var", "hello", PromoteTarget::Env).unwrap();
        assert_eq!(
            vars_out,
            r#"# variables.toml

[base_url]
description = "API root"
default = "http://localhost:8080"

[api_key]
description = "service API key"
secret = true

[user]
description = "seeded test user"
[user.options.alice]
description = "admin, active sub"
value = "1001"
[user.options.bob]
description = "expired trial"
value = "2002"

[user_id]
description = "linked user id"

[groups.test-user]
description = "user with linked customer"
members = ["user_id", "customer_id"]
[groups.test-user.options.alice]
description = "admin, active sub"
user_id = "1001"
customer_id = "c-77"

[new_var]
"#
        );
        assert_eq!(
            env_out.unwrap(),
            r#"# environments/qa.toml

base_url = "https://qa.example.com"
new_var = "hello"

[options.user.alice]
value = "9001"
[options.user.qa-only]
description = "exists only in qa"
value = "3003"
[options.test-user.alice]
user_id = "9001"
"#
        );
    }

    #[test]
    fn promote_var_onto_existing_enumerated_name_is_conflict() {
        let err =
            promote_var(VARS, Some(ENV), "user", "carol", PromoteTarget::Default).unwrap_err();
        assert!(
            matches!(err, EditError::Conflict(_)),
            "expected Conflict, got {err:?}"
        );
    }

    #[test]
    fn promote_var_onto_existing_group_name_is_conflict() {
        // "test-user" is a group name (`[groups.test-user]`), not a
        // variable — group and variable names share one namespace.
        let err = promote_var(
            VARS,
            Some(ENV),
            "test-user",
            "whatever",
            PromoteTarget::Default,
        )
        .unwrap_err();
        assert!(
            matches!(err, EditError::Conflict(_)),
            "expected Conflict, got {err:?}"
        );
    }

    #[test]
    fn promote_var_onto_existing_secret_name_is_conflict() {
        // "api_key" is `secret = true` with no options — the enumerated
        // check above doesn't catch it, so this exercises the dedicated
        // secret guard (review finding: this used to fall through to
        // `upsert_var` writing a `default` alongside `secret = true`,
        // producing a `variables.toml` that fails to parse on next load).
        let err = promote_var(
            VARS,
            Some(ENV),
            "api_key",
            "whatever",
            PromoteTarget::Default,
        )
        .unwrap_err();
        assert!(
            matches!(err, EditError::Conflict(_)),
            "expected Conflict, got {err:?}"
        );
    }

    #[test]
    fn promote_var_onto_existing_group_member_is_conflict() {
        // "user_id" is a member of the "test-user" group; its resolved
        // value comes from the group's selected option, not a plain
        // default.
        let err =
            promote_var(VARS, Some(ENV), "user_id", "1001", PromoteTarget::Default).unwrap_err();
        assert!(
            matches!(err, EditError::Conflict(_)),
            "expected Conflict, got {err:?}"
        );
    }
}

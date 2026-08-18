//! Surgical `toml_edit` mutations for `variables.toml` and
//! `environments/<env>.toml` (spec §5, §7).
//!
//! Every function here is pure text -> text: parse the document with
//! `toml_edit::DocumentMut`, mutate only the addressed item, and return
//! `doc.to_string()`. Write fidelity — comments, blank lines, ordering, and
//! unrelated entries survive untouched — is the whole point (spec §7); see
//! the round-trip fixture tests below for the contract in practice.

use indexmap::IndexMap;
use toml_edit::{Array, DocumentMut, Item, Key, Table, Value, value};

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
    root.remove(name);
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
        .and_then(|t| t.remove(key));
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

/// Deletes a group's table (and everything nested under it, including its
/// options). `NotFound` if there is no such group.
pub fn delete_group(doc: &str, name: &str) -> Result<String, EditError> {
    let mut doc = parse(doc)?;
    let root = doc.as_table_mut();
    let removed = root
        .get_mut("groups")
        .and_then(Item::as_table_mut)
        .and_then(|g| g.remove(name));
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
            root.insert(name, value(v));
        }
        None => {
            if root.remove(name).is_none() {
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

/// Removes `[options.owner.key]`. `NotFound` if it wasn't there.
pub fn delete_env_option(doc: &str, owner: &str, key: &str) -> Result<String, EditError> {
    let mut doc = parse(doc)?;
    let root = doc.as_table_mut();
    let removed = root
        .get_mut("options")
        .and_then(Item::as_table_mut)
        .and_then(|o| o.get_mut(owner))
        .and_then(Item::as_table_mut)
        .and_then(|t| t.remove(key));
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
            r#"
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
            r#"base_url = "https://qa2.example.com"

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
    fn set_env_value_none_removes_the_flat_pair() {
        let out = set_env_value(ENV, "base_url", None).unwrap();
        assert_eq!(
            out,
            r#"
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
}

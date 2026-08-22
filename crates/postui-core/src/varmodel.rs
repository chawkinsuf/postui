//! Stage-7 variable model: parsing `variables.toml` (variable declarations
//! and group declarations listing their `fields`) and
//! `environments/<env>.toml` (flat values for simple variables plus that
//! environment's group `entries`).
//!
//! A group is a set of linked fields with named entries — records, not a
//! switcher (spec §3.1). Picking an entry fills every field of the group at
//! once, and entries belong to one specific environment.

use indexmap::IndexMap;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct VarDecl {
    pub description: Option<String>,
    pub default: Option<String>,
    pub secret: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GroupDecl {
    pub description: Option<String>,
    /// Ordered field names; every entry of the group supplies all of them.
    pub fields: Vec<String>,
}

/// One named record of a group, in one environment.
#[derive(Debug, Clone, PartialEq)]
pub struct EntryDecl {
    pub description: Option<String>,
    /// field name → value
    pub values: IndexMap<String, String>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct VarModel {
    pub vars: IndexMap<String, VarDecl>,
    pub groups: IndexMap<String, GroupDecl>,
}

/// Flat values for simple variables plus this environment's group entries.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EnvData {
    pub values: IndexMap<String, String>,
    /// group name → entry name → entry
    pub entries: IndexMap<String, IndexMap<String, EntryDecl>>,
}

/// One env's `group name → selected entry name`. Legacy per-variable
/// selections use the same map (a migrated enumerated variable becomes a
/// one-field group of the same name, so its key carries over unchanged).
pub type Selections = IndexMap<String, String>;

/// One env's `name → secret value`.
pub type SecretValues = IndexMap<String, String>;

/// Why a resolved (or unresolved) name has the value it has.
#[derive(Debug, Clone, PartialEq)]
pub enum VarMeta {
    Simple,
    GroupMember {
        group: String,
        selected: String,
    },
    Secret,
    /// A group field whose group has no (or a stale) selection.
    NeedsSelection,
    /// Secret with no value for this env.
    MissingSecret,
}

/// The result of resolving a `VarModel` against one environment, its
/// selections, and its secrets.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Resolved {
    /// Names needing a selection or secret are omitted here.
    pub values: IndexMap<String, String>,
    /// Every declared name (vars + group fields) has an entry.
    pub meta: IndexMap<String, VarMeta>,
}

#[derive(thiserror::Error, Debug, Clone, PartialEq)]
pub enum ModelError {
    #[error("could not parse TOML: {0}")]
    Toml(String),
    #[error(
        "\"{0}\" is a reserved name and can't be used for a variable or group; rename it to something else"
    )]
    ReservedName(String),
    #[error(
        "\"{0}\" is not a valid name; use only ASCII letters, digits, `_`, and `-`, and make it non-empty"
    )]
    InvalidName(String),
    #[error("\"{0}\" is declared as both a variable and a group; rename one so the name is unique")]
    NameCollision(String),
    #[error("\"{0}\" must be a table, e.g. [{0}]")]
    NotATable(String),
    #[error("[{table}] has unknown field \"{field}\"; remove it or fix the typo")]
    UnknownField { table: String, field: String },
    #[error("[{table}] field \"{field}\" must be a string")]
    NotAString { table: String, field: String },
    #[error("[{table}] field \"{field}\" must be a boolean (true or false)")]
    NotABool { table: String, field: String },
    #[error("[groups.{group}] field \"fields\" must be an array of variable names")]
    FieldsNotArray { group: String },
    #[error("[groups.{group}] is missing required field \"fields\"")]
    MissingFields { group: String },
    #[error(
        "variable \"{0}\" is secret = true and also sets a default; remove default so no secret value is committed to variables.toml"
    )]
    SecretWithDefault(String),
    #[error(
        "group \"{group}\" lists field \"{field}\", which is declared secret = true; a secret can't be a group field"
    )]
    FieldIsSecret { group: String, field: String },
    #[error(
        "group \"{group}\" lists field \"{field}\", which is also declared as a variable; a group field's value comes from the group's entries, so remove the [{field}] declaration"
    )]
    FieldCollidesWithVar { group: String, field: String },
    #[error(
        "\"{field}\" is a field of both group \"{first}\" and group \"{second}\"; a field can only belong to one group"
    )]
    FieldInMultipleGroups {
        field: String,
        first: String,
        second: String,
    },
    #[error("[entries.{0}] must be a table (e.g. [entries.{0}.\"entry name\"])")]
    EntriesNotTable(String),
    #[error("[entries.{group}.\"{entry}\"] must be a table of field values")]
    EntryNotTable { group: String, entry: String },
    #[error("[entries.{group}.\"{entry}\"] field \"{field}\" must be a string")]
    EntryFieldNotString {
        group: String,
        entry: String,
        field: String,
    },
    #[error(
        "[entries.{0}] does not match a declared group; declare [groups.{0}] in variables.toml or fix the typo"
    )]
    EntryForUndeclaredGroup(String),
    #[error(
        "[entries.{group}.\"{entry}\"] is missing field \"{field}\"; every entry must supply all of the group's fields"
    )]
    EntryMissingField {
        group: String,
        entry: String,
        field: String,
    },
    #[error(
        "[entries.{group}.\"{entry}\"] sets \"{field}\", which is not a field of group \"{group}\"; fix the typo or add \"{field}\" to the group's fields"
    )]
    EntryUnknownField {
        group: String,
        entry: String,
        field: String,
    },
    #[error("[entries.{group}] has an entry with an empty name; give every entry a name")]
    EntryEmptyName { group: String },
    #[error(
        "[entries.{group}] has an entry named \"description\"; that name is reserved for an entry's own description, so rename the entry"
    )]
    EntryNameReserved { group: String },
    #[error(
        "environment sets a flat value for secret variable \"{0}\"; secrets can't be committed to environments/<env>.toml"
    )]
    EnvValueForSecret(String),
    #[error(
        "environment sets a flat value for \"{0}\", which is a group name; groups don't take a flat value, use [entries.{0}.\"<entry>\"] instead"
    )]
    EnvValueForGroup(String),
    #[error(
        "environment sets a flat value for \"{name}\", which is a field of group \"{group}\"; its value comes from the group's selected entry, not a flat value"
    )]
    EnvValueForField { name: String, group: String },
}

// ---------------------------------------------------------------------
// variables.toml
// ---------------------------------------------------------------------

const RESERVED_NAMES: [&str; 3] = ["options", "groups", "entries"];

/// The one key inside an entry table that is the entry's own description
/// rather than a field value — and so the one name an entry may not have.
pub const ENTRY_DESCRIPTION: &str = "description";

fn is_reserved(name: &str) -> bool {
    RESERVED_NAMES.contains(&name)
}

fn check_name(name: &str) -> Result<(), ModelError> {
    if is_reserved(name) {
        return Err(ModelError::ReservedName(name.to_string()));
    }
    if !crate::vars::is_valid_var_name(name) {
        return Err(ModelError::InvalidName(name.to_string()));
    }
    Ok(())
}

fn as_table<'a>(
    value: &'a toml::Value,
    table_path: &str,
) -> Result<&'a toml::map::Map<String, toml::Value>, ModelError> {
    value
        .as_table()
        .ok_or_else(|| ModelError::NotATable(table_path.to_string()))
}

fn get_string(
    table: &toml::map::Map<String, toml::Value>,
    field: &str,
    table_path: &str,
) -> Result<Option<String>, ModelError> {
    match table.get(field) {
        None => Ok(None),
        Some(v) => v
            .as_str()
            .map(|s| Some(s.to_string()))
            .ok_or_else(|| ModelError::NotAString {
                table: table_path.to_string(),
                field: field.to_string(),
            }),
    }
}

fn get_bool(
    table: &toml::map::Map<String, toml::Value>,
    field: &str,
    table_path: &str,
) -> Result<bool, ModelError> {
    match table.get(field) {
        None => Ok(false),
        Some(v) => v.as_bool().ok_or_else(|| ModelError::NotABool {
            table: table_path.to_string(),
            field: field.to_string(),
        }),
    }
}

fn check_unknown_fields(
    table: &toml::map::Map<String, toml::Value>,
    allowed: &[&str],
    table_path: &str,
) -> Result<(), ModelError> {
    for key in table.keys() {
        if !allowed.contains(&key.as_str()) {
            return Err(ModelError::UnknownField {
                table: table_path.to_string(),
                field: key.clone(),
            });
        }
    }
    Ok(())
}

fn parse_var_decl(value: &toml::Value, var_name: &str) -> Result<VarDecl, ModelError> {
    let table = as_table(value, var_name)?;
    check_unknown_fields(table, &["description", "default", "secret"], var_name)?;
    let description = get_string(table, "description", var_name)?;
    let default = get_string(table, "default", var_name)?;
    let secret = get_bool(table, "secret", var_name)?;
    if secret && default.is_some() {
        return Err(ModelError::SecretWithDefault(var_name.to_string()));
    }
    Ok(VarDecl {
        description,
        default,
        secret,
    })
}

fn parse_group_decl(value: &toml::Value, group_name: &str) -> Result<GroupDecl, ModelError> {
    let table_path = format!("groups.{group_name}");
    let table = as_table(value, &table_path)?;
    check_unknown_fields(table, &["description", "fields"], &table_path)?;
    let description = get_string(table, "description", &table_path)?;
    let fields_value = table
        .get("fields")
        .ok_or_else(|| ModelError::MissingFields {
            group: group_name.to_string(),
        })?;
    let fields_array = fields_value
        .as_array()
        .ok_or_else(|| ModelError::FieldsNotArray {
            group: group_name.to_string(),
        })?;
    let mut fields = Vec::new();
    for f in fields_array {
        let name = f.as_str().ok_or_else(|| ModelError::FieldsNotArray {
            group: group_name.to_string(),
        })?;
        check_name(name)?;
        fields.push(name.to_string());
    }
    Ok(GroupDecl {
        description,
        fields,
    })
}

pub fn parse_variables(s: &str) -> Result<VarModel, ModelError> {
    let top: IndexMap<String, toml::Value> =
        toml::from_str(s).map_err(|e| ModelError::Toml(e.to_string()))?;

    let mut vars = IndexMap::new();
    for (name, value) in &top {
        if name == "groups" {
            continue;
        }
        check_name(name)?;
        vars.insert(name.clone(), parse_var_decl(value, name)?);
    }

    let mut groups = IndexMap::new();
    // field name -> group name that has already claimed it
    let mut field_owner: IndexMap<String, String> = IndexMap::new();
    if let Some(groups_value) = top.get("groups") {
        let groups_table = as_table(groups_value, "groups")?;
        for (group_name, value) in groups_table {
            // A flat (non-table) entry under `[groups]` means someone tried
            // to use the reserved name "groups" as a plain variable table
            // (e.g. `[groups]\ndefault = "x"`) rather than declaring a
            // nested `[groups.<name>]`.
            if value.as_table().is_none() {
                return Err(ModelError::ReservedName("groups".to_string()));
            }
            check_name(group_name)?;
            if vars.contains_key(group_name) {
                return Err(ModelError::NameCollision(group_name.clone()));
            }
            let decl = parse_group_decl(value, group_name)?;
            for field in &decl.fields {
                if let Some(existing) = field_owner.get(field) {
                    return Err(ModelError::FieldInMultipleGroups {
                        field: field.clone(),
                        first: existing.clone(),
                        second: group_name.clone(),
                    });
                }
                field_owner.insert(field.clone(), group_name.clone());
                // A group MAY share its name with one of its own fields
                // (the one-field group a migrated enumerated variable
                // becomes); it may not share it with a *variable*, which
                // the NameCollision check above already rejected.
                if let Some(var_decl) = vars.get(field) {
                    if var_decl.secret {
                        return Err(ModelError::FieldIsSecret {
                            group: group_name.clone(),
                            field: field.clone(),
                        });
                    }
                    return Err(ModelError::FieldCollidesWithVar {
                        group: group_name.clone(),
                        field: field.clone(),
                    });
                }
            }
            groups.insert(group_name.clone(), decl);
        }
    }

    Ok(VarModel { vars, groups })
}

// ---------------------------------------------------------------------
// environments/<env>.toml
// ---------------------------------------------------------------------

fn parse_entry(value: &toml::Value, group: &str, entry: &str) -> Result<EntryDecl, ModelError> {
    let table = value.as_table().ok_or_else(|| ModelError::EntryNotTable {
        group: group.to_string(),
        entry: entry.to_string(),
    })?;
    let mut description = None;
    let mut values = IndexMap::new();
    for (field, field_value) in table {
        let s = field_value
            .as_str()
            .ok_or_else(|| ModelError::EntryFieldNotString {
                group: group.to_string(),
                entry: entry.to_string(),
                field: field.clone(),
            })?;
        if field == ENTRY_DESCRIPTION {
            description = Some(s.to_string());
            continue;
        }
        values.insert(field.clone(), s.to_string());
    }
    Ok(EntryDecl {
        description,
        values,
    })
}

pub fn parse_environment(s: &str) -> Result<EnvData, ModelError> {
    let top: IndexMap<String, toml::Value> =
        toml::from_str(s).map_err(|e| ModelError::Toml(e.to_string()))?;

    let mut values = IndexMap::new();
    let mut entries: IndexMap<String, IndexMap<String, EntryDecl>> = IndexMap::new();

    for (key, value) in &top {
        if key == "entries" {
            let entries_table = as_table(value, "entries")?;
            for (group, group_value) in entries_table {
                let group_table = group_value
                    .as_table()
                    .ok_or_else(|| ModelError::EntriesNotTable(group.clone()))?;
                let mut per_group = IndexMap::new();
                for (entry_name, entry_value) in group_table {
                    if entry_name.is_empty() {
                        return Err(ModelError::EntryEmptyName {
                            group: group.clone(),
                        });
                    }
                    // `description` inside a group's entries table is an
                    // entry's own description, never an entry name.
                    if entry_name == ENTRY_DESCRIPTION {
                        return Err(ModelError::EntryNameReserved {
                            group: group.clone(),
                        });
                    }
                    per_group.insert(
                        entry_name.clone(),
                        parse_entry(entry_value, group, entry_name)?,
                    );
                }
                entries.insert(group.clone(), per_group);
            }
            continue;
        }
        let s = value.as_str().ok_or_else(|| ModelError::NotAString {
            table: "<root>".to_string(),
            field: key.clone(),
        })?;
        values.insert(key.clone(), s.to_string());
    }

    Ok(EnvData { values, entries })
}

/// This environment's entries for `group`, or `None` when the group has no
/// entries here.
pub fn group_entries<'a>(env: &'a EnvData, group: &str) -> Option<&'a IndexMap<String, EntryDecl>> {
    env.entries.get(group)
}

// ---------------------------------------------------------------------
// env validation
// ---------------------------------------------------------------------

/// Friendly errors: a flat value naming a secret, a group, or a group
/// field; an `[entries.<group>]` table for an undeclared group; an entry
/// missing one of its group's fields or setting a field the group doesn't
/// declare.
pub fn validate_env(model: &VarModel, env: &EnvData) -> Result<(), ModelError> {
    for key in env.values.keys() {
        if let Some(decl) = model.vars.get(key) {
            if decl.secret {
                return Err(ModelError::EnvValueForSecret(key.clone()));
            }
            continue;
        }
        if model.groups.contains_key(key) {
            return Err(ModelError::EnvValueForGroup(key.clone()));
        }
        if let Some(group_name) = model
            .groups
            .iter()
            .find(|(_, g)| g.fields.contains(key))
            .map(|(gname, _)| gname.clone())
        {
            return Err(ModelError::EnvValueForField {
                name: key.clone(),
                group: group_name,
            });
        }
    }

    for (group, group_entries) in &env.entries {
        let decl = model
            .groups
            .get(group)
            .ok_or_else(|| ModelError::EntryForUndeclaredGroup(group.clone()))?;
        for (entry_name, entry) in group_entries {
            for field in &decl.fields {
                if !entry.values.contains_key(field) {
                    return Err(ModelError::EntryMissingField {
                        group: group.clone(),
                        entry: entry_name.clone(),
                        field: field.clone(),
                    });
                }
            }
            for field in entry.values.keys() {
                if !decl.fields.contains(field) {
                    return Err(ModelError::EntryUnknownField {
                        group: group.clone(),
                        entry: entry_name.clone(),
                        field: field.clone(),
                    });
                }
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------
// resolve_env
// ---------------------------------------------------------------------

/// Resolve a `VarModel` against one environment's data, selections, and
/// secrets. Pure: no I/O, no file system.
///
/// Precedence per name, first hit wins (layers 1–2 — request overlay and
/// script-set values — are applied elsewhere): secret value for the active
/// env → the selected entry's value for a group field → flat env value →
/// declaration default. Names needing a selection or a secret are omitted
/// from `values` but still get a `meta` entry. Undeclared env values pass
/// through into `values` with no `meta` entry (stage-3 leniency).
pub fn resolve_env(
    model: &VarModel,
    env: &EnvData,
    selections: &Selections,
    secrets: &SecretValues,
) -> Resolved {
    let mut values = IndexMap::new();
    let mut meta = IndexMap::new();

    for (name, decl) in &model.vars {
        if decl.secret {
            match secrets.get(name) {
                Some(value) => {
                    values.insert(name.clone(), value.clone());
                    meta.insert(name.clone(), VarMeta::Secret);
                }
                None => {
                    meta.insert(name.clone(), VarMeta::MissingSecret);
                }
            }
            continue;
        }

        if let Some(value) = env.values.get(name) {
            values.insert(name.clone(), value.clone());
        } else if let Some(default) = &decl.default {
            values.insert(name.clone(), default.clone());
        }
        meta.insert(name.clone(), VarMeta::Simple);
    }

    for (group_name, group_decl) in &model.groups {
        let selected = selections.get(group_name).and_then(|name| {
            env.entries
                .get(group_name)
                .and_then(|entries| entries.get(name))
                .map(|entry| (name, entry))
        });

        for field in &group_decl.fields {
            match selected {
                Some((name, entry)) => {
                    if let Some(value) = entry.values.get(field) {
                        values.insert(field.clone(), value.clone());
                    } else {
                        values.shift_remove(field);
                    }
                    meta.insert(
                        field.clone(),
                        VarMeta::GroupMember {
                            group: group_name.clone(),
                            selected: name.clone(),
                        },
                    );
                }
                None => {
                    values.shift_remove(field);
                    meta.insert(field.clone(), VarMeta::NeedsSelection);
                }
            }
        }
    }

    let declared_fields: std::collections::HashSet<&str> = model
        .groups
        .values()
        .flat_map(|g| g.fields.iter().map(String::as_str))
        .collect();
    for (name, value) in &env.values {
        if model.vars.contains_key(name) || declared_fields.contains(name.as_str()) {
            continue;
        }
        values.insert(name.clone(), value.clone());
    }

    Resolved { values, meta }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------
    // variables.toml
    // -----------------------------------------------------------------

    #[test]
    fn parses_vars_and_groups_with_fields() {
        let m = parse_variables(
            r#"
[base_url]
description = "API root"
default = "http://localhost:8080"

[api_key]
secret = true

[groups.user]
description = "Linked user/customer pair"
fields = ["user_id", "customer_id"]
"#,
        )
        .unwrap();
        assert_eq!(m.groups["user"].fields, ["user_id", "customer_id"]);
        assert_eq!(
            m.groups["user"].description.as_deref(),
            Some("Linked user/customer pair")
        );
        assert!(m.vars["api_key"].secret);
        assert_eq!(
            m.vars["base_url"].default.as_deref(),
            Some("http://localhost:8080")
        );
        assert_eq!(m.vars["base_url"].description.as_deref(), Some("API root"));
        assert!(!m.vars["base_url"].secret);
    }

    #[test]
    fn variable_named_entries_is_rejected_as_reserved() {
        let err = parse_variables("[entries]\ndefault = \"x\"\n").unwrap_err();
        assert!(
            err.to_string().contains("\"entries\" is a reserved name"),
            "{err}"
        );
    }

    #[test]
    fn variable_named_options_is_rejected_as_reserved() {
        let err = parse_variables("[options]\ndefault = \"x\"\n").unwrap_err();
        assert!(
            err.to_string().contains("\"options\" is a reserved name"),
            "{err}"
        );
    }

    #[test]
    fn variable_named_groups_is_rejected_as_reserved() {
        // `[groups]` is always the group container; a flat field under it
        // (as if declaring a variable named "groups") is rejected.
        let err = parse_variables("[groups]\ndefault = \"x\"\n").unwrap_err();
        assert!(
            err.to_string().contains("\"groups\" is a reserved name"),
            "{err}"
        );
    }

    #[test]
    fn invalid_variable_name_is_rejected() {
        let err = parse_variables("[\"has space\"]\ndefault = \"x\"\n").unwrap_err();
        assert!(err.to_string().contains("is not a valid name"), "{err}");
    }

    #[test]
    fn group_colliding_with_a_variable_name_is_rejected() {
        let err = parse_variables(
            r#"
[user]
default = "x"

[groups.user]
fields = ["user_id"]
"#,
        )
        .unwrap_err();
        assert!(
            err.to_string()
                .contains("\"user\" is declared as both a variable and a group"),
            "{err}"
        );
    }

    #[test]
    fn field_in_two_groups_is_rejected() {
        let err = parse_variables(
            r#"
[groups.a]
fields = ["shared"]

[groups.b]
fields = ["shared"]
"#,
        )
        .unwrap_err();
        assert!(
            err.to_string()
                .contains("\"shared\" is a field of both group \"a\" and group \"b\""),
            "{err}"
        );
    }

    #[test]
    fn field_that_is_also_a_declared_variable_is_rejected() {
        let err = parse_variables(
            r#"
[user_id]
default = "1"

[groups.user]
fields = ["user_id"]
"#,
        )
        .unwrap_err();
        assert!(
            err.to_string()
                .contains("which is also declared as a variable"),
            "{err}"
        );
    }

    #[test]
    fn field_declared_secret_is_rejected() {
        let err = parse_variables(
            r#"
[token]
secret = true

[groups.user]
fields = ["token"]
"#,
        )
        .unwrap_err();
        assert!(
            err.to_string()
                .contains("which is declared secret = true; a secret can't be a group field"),
            "{err}"
        );
    }

    #[test]
    fn group_may_share_its_name_with_its_own_single_field() {
        let m = parse_variables("[groups.tier]\nfields = [\"tier\"]\n").unwrap();
        assert_eq!(m.groups["tier"].fields, ["tier"]);
    }

    #[test]
    fn group_without_fields_is_rejected() {
        let err = parse_variables("[groups.user]\ndescription = \"x\"\n").unwrap_err();
        assert!(
            err.to_string()
                .contains("[groups.user] is missing required field \"fields\""),
            "{err}"
        );
    }

    #[test]
    fn group_fields_must_be_an_array_of_strings() {
        let err = parse_variables("[groups.user]\nfields = \"user_id\"\n").unwrap_err();
        assert!(
            err.to_string()
                .contains("field \"fields\" must be an array of variable names"),
            "{err}"
        );
        let err = parse_variables("[groups.user]\nfields = [1]\n").unwrap_err();
        assert!(
            err.to_string()
                .contains("field \"fields\" must be an array of variable names"),
            "{err}"
        );
    }

    #[test]
    fn secret_with_default_is_rejected() {
        let err = parse_variables("[api_key]\nsecret = true\ndefault = \"oops\"\n").unwrap_err();
        assert!(
            err.to_string()
                .contains("is secret = true and also sets a default"),
            "{err}"
        );
    }

    #[test]
    fn unknown_declaration_field_is_rejected() {
        let err = parse_variables("[base_url]\nvalue = \"x\"\n").unwrap_err();
        assert!(
            err.to_string()
                .contains("[base_url] has unknown field \"value\""),
            "{err}"
        );
        let err = parse_variables("[groups.user]\nfields = []\nmembers = []\n").unwrap_err();
        assert!(
            err.to_string()
                .contains("[groups.user] has unknown field \"members\""),
            "{err}"
        );
    }

    // -----------------------------------------------------------------
    // environments/<env>.toml
    // -----------------------------------------------------------------

    #[test]
    fn parses_env_entries_and_resolves_selection() {
        let m =
            parse_variables("[groups.user]\nfields = [\"user_id\", \"customer_id\"]\n").unwrap();
        let e = parse_environment(
            r#"
base_url = "https://stg.example.com"

[entries.user."user 1"]
user_id = "1001"
customer_id = "cust-77"

[entries.user."user 2"]
description = "the premium one"
user_id = "1002"
customer_id = "cust-91"
"#,
        )
        .unwrap();
        assert_eq!(e.values["base_url"], "https://stg.example.com");
        assert_eq!(e.entries["user"]["user 2"].values["customer_id"], "cust-91");
        assert_eq!(
            e.entries["user"]["user 2"].description.as_deref(),
            Some("the premium one")
        );
        assert!(
            !e.entries["user"]["user 2"]
                .values
                .contains_key("description"),
            "an entry's description is not one of its fields"
        );
        validate_env(&m, &e).unwrap();

        let mut sel = Selections::new();
        sel.insert("user".into(), "user 2".into());
        let r = resolve_env(&m, &e, &sel, &SecretValues::new());
        assert_eq!(r.values["user_id"], "1002");
        assert_eq!(
            r.meta["customer_id"],
            VarMeta::GroupMember {
                group: "user".into(),
                selected: "user 2".into()
            }
        );
    }

    #[test]
    fn group_entries_looks_up_one_groups_entries() {
        let e = parse_environment("[entries.user.\"user 1\"]\nuser_id = \"1\"\n").unwrap();
        assert_eq!(group_entries(&e, "user").unwrap().len(), 1);
        assert!(group_entries(&e, "nope").is_none());
    }

    #[test]
    fn entry_table_for_an_undeclared_group_is_rejected() {
        let m = parse_variables("").unwrap();
        let e = parse_environment("[entries.ghost.\"a\"]\nx = \"1\"\n").unwrap();
        let err = validate_env(&m, &e).unwrap_err();
        assert!(
            err.to_string()
                .contains("[entries.ghost] does not match a declared group"),
            "{err}"
        );
    }

    #[test]
    fn entry_missing_a_field_is_rejected() {
        let m =
            parse_variables("[groups.user]\nfields = [\"user_id\", \"customer_id\"]\n").unwrap();
        let e = parse_environment("[entries.user.\"user 1\"]\nuser_id = \"1\"\n").unwrap();
        let err = validate_env(&m, &e).unwrap_err();
        assert!(
            err.to_string()
                .contains("[entries.user.\"user 1\"] is missing field \"customer_id\""),
            "{err}"
        );
    }

    #[test]
    fn entry_with_an_extra_field_is_rejected() {
        let m = parse_variables("[groups.user]\nfields = [\"user_id\"]\n").unwrap();
        let e = parse_environment("[entries.user.\"user 1\"]\nuser_id = \"1\"\nnope = \"2\"\n")
            .unwrap();
        let err = validate_env(&m, &e).unwrap_err();
        assert!(
            err.to_string().contains(
                "[entries.user.\"user 1\"] sets \"nope\", which is not a field of group \"user\""
            ),
            "{err}"
        );
    }

    #[test]
    fn entry_field_must_be_a_string() {
        let err = parse_environment("[entries.user.\"user 1\"]\nuser_id = 1001\n").unwrap_err();
        assert!(
            err.to_string()
                .contains("[entries.user.\"user 1\"] field \"user_id\" must be a string"),
            "{err}"
        );
    }

    #[test]
    fn entry_must_be_a_table() {
        let err = parse_environment("[entries.user]\n\"user 1\" = \"nope\"\n").unwrap_err();
        assert!(
            err.to_string()
                .contains("[entries.user.\"user 1\"] must be a table of field values"),
            "{err}"
        );
    }

    #[test]
    fn entries_group_must_be_a_table() {
        let err = parse_environment("[entries]\nuser = \"nope\"\n").unwrap_err();
        assert!(
            err.to_string().contains("[entries.user] must be a table"),
            "{err}"
        );
    }

    #[test]
    fn empty_entry_name_is_rejected() {
        let err = parse_environment("[entries.user.\"\"]\nuser_id = \"1\"\n").unwrap_err();
        assert!(
            err.to_string()
                .contains("[entries.user] has an entry with an empty name"),
            "{err}"
        );
    }

    #[test]
    fn an_entry_named_description_is_rejected() {
        // `description` in an entries table is an entry's own description,
        // so it can't double as an entry name.
        let err = parse_environment("[entries.user.description]\nuser_id = \"1\"\n").unwrap_err();
        assert!(
            err.to_string()
                .contains("[entries.user] has an entry named \"description\""),
            "{err}"
        );
    }

    #[test]
    fn flat_env_value_for_a_group_is_rejected() {
        let m = parse_variables("[groups.user]\nfields = [\"user_id\"]\n").unwrap();
        let e = parse_environment("user = \"x\"\n").unwrap();
        let err = validate_env(&m, &e).unwrap_err();
        assert!(err.to_string().contains("which is a group name"), "{err}");
    }

    #[test]
    fn flat_env_value_for_a_group_field_is_rejected() {
        let m = parse_variables("[groups.user]\nfields = [\"user_id\"]\n").unwrap();
        let e = parse_environment("user_id = \"x\"\n").unwrap();
        let err = validate_env(&m, &e).unwrap_err();
        assert!(
            err.to_string()
                .contains("which is a field of group \"user\""),
            "{err}"
        );
    }

    #[test]
    fn flat_env_value_for_a_secret_is_rejected() {
        let m = parse_variables("[api_key]\nsecret = true\n").unwrap();
        let e = parse_environment("api_key = \"leak\"\n").unwrap();
        let err = validate_env(&m, &e).unwrap_err();
        assert!(
            err.to_string()
                .contains("environment sets a flat value for secret variable \"api_key\""),
            "{err}"
        );
    }

    // -----------------------------------------------------------------
    // resolve_env
    // -----------------------------------------------------------------

    fn user_model() -> VarModel {
        parse_variables("[groups.user]\nfields = [\"user_id\", \"customer_id\"]\n").unwrap()
    }

    fn user_env() -> EnvData {
        parse_environment(
            "[entries.user.\"user 1\"]\nuser_id = \"1001\"\ncustomer_id = \"cust-77\"\n",
        )
        .unwrap()
    }

    #[test]
    fn no_selection_leaves_every_field_needing_one() {
        let r = resolve_env(
            &user_model(),
            &user_env(),
            &Selections::new(),
            &SecretValues::new(),
        );
        assert_eq!(r.meta["user_id"], VarMeta::NeedsSelection);
        assert_eq!(r.meta["customer_id"], VarMeta::NeedsSelection);
        assert!(r.values.is_empty());
    }

    #[test]
    fn stale_selection_degrades_to_needs_selection() {
        let mut sel = Selections::new();
        sel.insert("user".into(), "deleted entry".into());
        let r = resolve_env(&user_model(), &user_env(), &sel, &SecretValues::new());
        assert_eq!(r.meta["user_id"], VarMeta::NeedsSelection);
        assert!(r.values.get("user_id").is_none());
    }

    #[test]
    fn secret_value_wins_over_a_flat_env_value() {
        let m = parse_variables("[api_key]\nsecret = true\n").unwrap();
        let e = parse_environment("api_key = \"from-env\"\n").unwrap();
        let mut secrets = SecretValues::new();
        secrets.insert("api_key".into(), "s3cret".into());
        let r = resolve_env(&m, &e, &Selections::new(), &secrets);
        assert_eq!(r.values["api_key"], "s3cret");
        assert_eq!(r.meta["api_key"], VarMeta::Secret);
    }

    #[test]
    fn missing_secret_is_reported_and_omitted_from_values() {
        let m = parse_variables("[api_key]\nsecret = true\n").unwrap();
        let r = resolve_env(
            &m,
            &EnvData::default(),
            &Selections::new(),
            &SecretValues::new(),
        );
        assert_eq!(r.meta["api_key"], VarMeta::MissingSecret);
        assert!(r.values.get("api_key").is_none());
    }

    #[test]
    fn env_value_beats_declaration_default_which_is_the_fallback() {
        let m = parse_variables("[base_url]\ndefault = \"http://localhost\"\n").unwrap();
        let r = resolve_env(
            &m,
            &EnvData::default(),
            &Selections::new(),
            &SecretValues::new(),
        );
        assert_eq!(r.values["base_url"], "http://localhost");
        assert_eq!(r.meta["base_url"], VarMeta::Simple);

        let e = parse_environment("base_url = \"https://stg\"\n").unwrap();
        let r = resolve_env(&m, &e, &Selections::new(), &SecretValues::new());
        assert_eq!(r.values["base_url"], "https://stg");
    }

    #[test]
    fn undeclared_env_values_pass_through_without_meta() {
        let r = resolve_env(
            &VarModel::default(),
            &parse_environment("stray = \"v\"\n").unwrap(),
            &Selections::new(),
            &SecretValues::new(),
        );
        assert_eq!(r.values["stray"], "v");
        assert!(r.meta.get("stray").is_none());
    }

    #[test]
    fn one_field_group_sharing_its_name_resolves_through_the_selection() {
        let m = parse_variables("[groups.tier]\nfields = [\"tier\"]\n").unwrap();
        let e = parse_environment(
            "[entries.tier.gold]\ntier = \"g-1\"\n[entries.tier.free]\ntier = \"f-1\"\n",
        )
        .unwrap();
        validate_env(&m, &e).unwrap();
        let mut sel = Selections::new();
        sel.insert("tier".into(), "gold".into());
        let r = resolve_env(&m, &e, &sel, &SecretValues::new());
        assert_eq!(r.values["tier"], "g-1");
        assert_eq!(
            r.meta["tier"],
            VarMeta::GroupMember {
                group: "tier".into(),
                selected: "gold".into()
            }
        );
    }
}

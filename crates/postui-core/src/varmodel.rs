//! Stage-6 variable model: parsing `variables.toml` (declarations, options,
//! groups) and `environments/<env>.toml` (flat values + keyed option
//! overrides). Pure parsing only — merging declarations against an
//! environment happens in a later module.

use indexmap::IndexMap;

#[derive(Debug, Clone, PartialEq)]
pub struct OptionDecl {
    pub description: Option<String>,
    pub value: String,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct VarDecl {
    pub description: Option<String>,
    pub default: Option<String>,
    pub secret: bool,
    pub options: IndexMap<String, OptionDecl>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GroupOption {
    pub description: Option<String>,
    /// member name → value
    pub values: IndexMap<String, String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GroupDecl {
    pub description: Option<String>,
    pub members: Vec<String>,
    pub options: IndexMap<String, GroupOption>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct VarModel {
    pub vars: IndexMap<String, VarDecl>,
    pub groups: IndexMap<String, GroupDecl>,
}

/// One env's `name → selected option key` (variables and groups share the
/// namespace, so a single map serves both).
pub type Selections = IndexMap<String, String>;

/// One env's `name → secret value`.
pub type SecretValues = IndexMap<String, String>;

/// Why a resolved (or unresolved) name has the value it has.
#[derive(Debug, Clone, PartialEq)]
pub enum VarMeta {
    Simple,
    Enumerated {
        selected: String,
    },
    GroupMember {
        group: String,
        selected: String,
    },
    Secret,
    /// Enumerated/group with no (or stale) selection.
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
    /// Every declared name (vars + group members) has an entry.
    pub meta: IndexMap<String, VarMeta>,
}

/// Flat env values plus raw option override tables, interpreted against a
/// `VarModel` elsewhere.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EnvData {
    pub values: IndexMap<String, String>,
    /// name → key → field/member → string
    pub options: IndexMap<String, IndexMap<String, IndexMap<String, String>>>,
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
    #[error("[{table}] field \"members\" must be an array of variable names")]
    MembersNotArray { table: String },
    #[error("[groups.{group}] is missing required field \"members\"")]
    MissingMembers { group: String },
    #[error("[{table}] is missing required field \"value\"")]
    MissingValue { table: String },
    #[error(
        "variable \"{0}\" is secret = true and also sets a default; remove default so no secret value is committed to variables.toml"
    )]
    SecretWithDefault(String),
    #[error(
        "variable \"{0}\" is secret = true and also declares options; remove options so no secret value is committed to variables.toml"
    )]
    SecretWithOptions(String),
    #[error(
        "group \"{group}\" member \"{member}\" declares its own [options]; give the option values on the group instead"
    )]
    MemberHasOptions { group: String, member: String },
    #[error(
        "group \"{group}\" member \"{member}\" is declared secret = true; a secret can't be a group member"
    )]
    MemberIsSecret { group: String, member: String },
    #[error(
        "variable \"{var}\" belongs to both group \"{first}\" and group \"{second}\"; a variable can only belong to one group"
    )]
    VariableInMultipleGroups {
        var: String,
        first: String,
        second: String,
    },
    #[error("[options.{0}] must be a table (e.g. [options.{0}.<key>]); found a non-table value")]
    OptionsNotTable(String),
    #[error(
        "environment sets a flat value for \"{0}\", but \"{0}\" has options in this environment; remove the flat value or the options"
    )]
    EnvValueForEnumerated(String),
    #[error(
        "environment sets a flat value for secret variable \"{0}\"; secrets can't be committed to environments/<env>.toml"
    )]
    EnvValueForSecret(String),
    #[error(
        "environment sets a flat value for \"{0}\", which is a group name; groups don't take a flat value, use [options.{0}.<key>] instead"
    )]
    EnvValueForGroup(String),
    #[error(
        "environment sets a flat value for \"{name}\", which is a member of group \"{group}\"; its value comes from the group's selected option, not a flat value"
    )]
    EnvValueForGroupMember { name: String, group: String },
    #[error(
        "[options.{0}] does not match a declared variable or group; declare it in variables.toml or fix the typo"
    )]
    EnvOptionsUndeclared(String),
    #[error("[options.{0}] is secret = true; secrets can't have options set in an environment")]
    EnvOptionsForSecret(String),
    #[error(
        "[options.{group}.{key}] sets \"{member}\", which is not a member of group \"{group}\"; fix the typo or add \"{member}\" to members"
    )]
    EnvGroupOptionNonMember {
        group: String,
        key: String,
        member: String,
    },
    #[error(
        "[options.{name}.{key}] is a new option not declared in variables.toml and is missing required field \"value\""
    )]
    EnvOptionMissingValue { name: String, key: String },
}

// ---------------------------------------------------------------------
// variables.toml
// ---------------------------------------------------------------------

const RESERVED_NAMES: [&str; 2] = ["options", "groups"];

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

fn parse_option_decl(value: &toml::Value, table_path: &str) -> Result<OptionDecl, ModelError> {
    let table = as_table(value, table_path)?;
    check_unknown_fields(table, &["description", "value"], table_path)?;
    let description = get_string(table, "description", table_path)?;
    let value =
        get_string(table, "value", table_path)?.ok_or_else(|| ModelError::MissingValue {
            table: table_path.to_string(),
        })?;
    Ok(OptionDecl { description, value })
}

fn parse_var_options(
    value: &toml::Value,
    var_name: &str,
) -> Result<IndexMap<String, OptionDecl>, ModelError> {
    let table_path = format!("{var_name}.options");
    let table = as_table(value, &table_path)?;
    let mut out = IndexMap::new();
    for (key, v) in table {
        check_name(key)?;
        let entry_path = format!("{table_path}.{key}");
        out.insert(key.clone(), parse_option_decl(v, &entry_path)?);
    }
    Ok(out)
}

fn parse_var_decl(value: &toml::Value, var_name: &str) -> Result<VarDecl, ModelError> {
    let table = as_table(value, var_name)?;
    check_unknown_fields(
        table,
        &["description", "default", "secret", "options"],
        var_name,
    )?;
    let description = get_string(table, "description", var_name)?;
    let default = get_string(table, "default", var_name)?;
    let secret = get_bool(table, "secret", var_name)?;
    let options = match table.get("options") {
        None => IndexMap::new(),
        Some(v) => parse_var_options(v, var_name)?,
    };
    if secret && default.is_some() {
        return Err(ModelError::SecretWithDefault(var_name.to_string()));
    }
    if secret && !options.is_empty() {
        return Err(ModelError::SecretWithOptions(var_name.to_string()));
    }
    Ok(VarDecl {
        description,
        default,
        secret,
        options,
    })
}

fn parse_group_option(value: &toml::Value, table_path: &str) -> Result<GroupOption, ModelError> {
    let table = as_table(value, table_path)?;
    let description = get_string(table, "description", table_path)?;
    let mut values = IndexMap::new();
    for (key, v) in table {
        if key == "description" {
            continue;
        }
        let value = v.as_str().ok_or_else(|| ModelError::NotAString {
            table: table_path.to_string(),
            field: key.clone(),
        })?;
        values.insert(key.clone(), value.to_string());
    }
    Ok(GroupOption {
        description,
        values,
    })
}

fn parse_group_decl(value: &toml::Value, group_name: &str) -> Result<GroupDecl, ModelError> {
    let table_path = format!("groups.{group_name}");
    let table = as_table(value, &table_path)?;
    check_unknown_fields(table, &["description", "members", "options"], &table_path)?;
    let description = get_string(table, "description", &table_path)?;
    let members_value = table
        .get("members")
        .ok_or_else(|| ModelError::MissingMembers {
            group: group_name.to_string(),
        })?;
    let members_array = members_value
        .as_array()
        .ok_or_else(|| ModelError::MembersNotArray {
            table: table_path.clone(),
        })?;
    let mut members = Vec::new();
    for m in members_array {
        let name = m.as_str().ok_or_else(|| ModelError::MembersNotArray {
            table: table_path.clone(),
        })?;
        check_name(name)?;
        members.push(name.to_string());
    }
    let mut options = IndexMap::new();
    if let Some(opts_value) = table.get("options") {
        let opts_table = as_table(opts_value, &format!("{table_path}.options"))?;
        for (key, v) in opts_table {
            check_name(key)?;
            let entry_path = format!("{table_path}.options.{key}");
            options.insert(key.clone(), parse_group_option(v, &entry_path)?);
        }
    }
    Ok(GroupDecl {
        description,
        members,
        options,
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
    // member name -> group name that has already claimed it
    let mut member_owner: IndexMap<String, String> = IndexMap::new();
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
            for member in &decl.members {
                if let Some(existing) = member_owner.get(member) {
                    return Err(ModelError::VariableInMultipleGroups {
                        var: member.clone(),
                        first: existing.clone(),
                        second: group_name.clone(),
                    });
                }
                member_owner.insert(member.clone(), group_name.clone());
                if let Some(var_decl) = vars.get(member) {
                    if !var_decl.options.is_empty() {
                        return Err(ModelError::MemberHasOptions {
                            group: group_name.clone(),
                            member: member.clone(),
                        });
                    }
                    if var_decl.secret {
                        return Err(ModelError::MemberIsSecret {
                            group: group_name.clone(),
                            member: member.clone(),
                        });
                    }
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

pub fn parse_environment(s: &str) -> Result<EnvData, ModelError> {
    let top: IndexMap<String, toml::Value> =
        toml::from_str(s).map_err(|e| ModelError::Toml(e.to_string()))?;

    let mut values = IndexMap::new();
    let mut options = IndexMap::new();

    for (key, value) in &top {
        if key == "options" {
            let opts_table = as_table(value, "options")?;
            for (name, name_value) in opts_table {
                let name_table = name_value
                    .as_table()
                    .ok_or_else(|| ModelError::OptionsNotTable(name.clone()))?;
                let mut per_name = IndexMap::new();
                for (opt_key, opt_value) in name_table {
                    let entry_path = format!("options.{name}.{opt_key}");
                    let entry_table = as_table(opt_value, &entry_path)?;
                    let mut fields = IndexMap::new();
                    for (field, field_value) in entry_table {
                        let s = field_value.as_str().ok_or_else(|| ModelError::NotAString {
                            table: entry_path.clone(),
                            field: field.clone(),
                        })?;
                        fields.insert(field.clone(), s.to_string());
                    }
                    per_name.insert(opt_key.clone(), fields);
                }
                options.insert(name.clone(), per_name);
            }
            continue;
        }
        let s = value.as_str().ok_or_else(|| ModelError::NotAString {
            table: "<root>".to_string(),
            field: key.clone(),
        })?;
        values.insert(key.clone(), s.to_string());
    }

    Ok(EnvData { values, options })
}

// ---------------------------------------------------------------------
// merging + env validation
// ---------------------------------------------------------------------

/// Env option tables merged by key onto the shared list; env-only lists
/// come through wholesale. Empty map = simple in this env.
pub fn merged_var_options(
    model: &VarModel,
    env: &EnvData,
    name: &str,
) -> IndexMap<String, OptionDecl> {
    let mut out = model
        .vars
        .get(name)
        .map(|decl| decl.options.clone())
        .unwrap_or_default();

    if let Some(env_opts) = env.options.get(name) {
        for (key, fields) in env_opts {
            match out.get_mut(key) {
                Some(existing) => {
                    if let Some(description) = fields.get("description") {
                        existing.description = Some(description.clone());
                    }
                    if let Some(value) = fields.get("value") {
                        existing.value = value.clone();
                    }
                }
                None => {
                    out.insert(
                        key.clone(),
                        OptionDecl {
                            description: fields.get("description").cloned(),
                            value: fields.get("value").cloned().unwrap_or_default(),
                        },
                    );
                }
            }
        }
    }

    out
}

/// Env option tables merged by key onto the shared list; env-only lists
/// come through wholesale. Empty map = simple in this env.
pub fn merged_group_options(
    model: &VarModel,
    env: &EnvData,
    group: &str,
) -> IndexMap<String, GroupOption> {
    let mut out = model
        .groups
        .get(group)
        .map(|decl| decl.options.clone())
        .unwrap_or_default();

    if let Some(env_opts) = env.options.get(group) {
        for (key, fields) in env_opts {
            let description = fields.get("description").cloned();
            let member_values = fields
                .iter()
                .filter(|(field, _)| field.as_str() != "description")
                .map(|(field, value)| (field.clone(), value.clone()));

            match out.get_mut(key) {
                Some(existing) => {
                    if description.is_some() {
                        existing.description = description;
                    }
                    for (member, value) in member_values {
                        existing.values.insert(member, value);
                    }
                }
                None => {
                    out.insert(
                        key.clone(),
                        GroupOption {
                            description,
                            values: member_values.collect(),
                        },
                    );
                }
            }
        }
    }

    out
}

/// Friendly errors: flat value for a var enumerated *in this env*; flat
/// value for a secret var; [options.<name>] where <name> is undeclared or
/// secret; group option row naming a non-member.
pub fn validate_env(model: &VarModel, env: &EnvData) -> Result<(), ModelError> {
    for key in env.values.keys() {
        if let Some(decl) = model.vars.get(key) {
            if decl.secret {
                return Err(ModelError::EnvValueForSecret(key.clone()));
            }
            if !merged_var_options(model, env, key).is_empty() {
                return Err(ModelError::EnvValueForEnumerated(key.clone()));
            }
            continue;
        }
        if model.groups.contains_key(key) {
            return Err(ModelError::EnvValueForGroup(key.clone()));
        }
        if let Some(group_name) = model
            .groups
            .iter()
            .find(|(_, g)| g.members.contains(key))
            .map(|(gname, _)| gname.clone())
        {
            return Err(ModelError::EnvValueForGroupMember {
                name: key.clone(),
                group: group_name,
            });
        }
    }

    for (name, entries) in &env.options {
        if let Some(decl) = model.vars.get(name) {
            if decl.secret {
                return Err(ModelError::EnvOptionsForSecret(name.clone()));
            }
            for (key, fields) in entries {
                if !decl.options.contains_key(key) && !fields.contains_key("value") {
                    return Err(ModelError::EnvOptionMissingValue {
                        name: name.clone(),
                        key: key.clone(),
                    });
                }
            }
        } else if let Some(group_decl) = model.groups.get(name) {
            for (key, fields) in entries {
                for member in fields.keys() {
                    if member == "description" {
                        continue;
                    }
                    if !group_decl.members.contains(member) {
                        return Err(ModelError::EnvGroupOptionNonMember {
                            group: name.clone(),
                            key: key.clone(),
                            member: member.clone(),
                        });
                    }
                }
            }
        } else {
            return Err(ModelError::EnvOptionsUndeclared(name.clone()));
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
/// Precedence per name, first hit wins (spec §2, layers 3–6; layers 1–2 —
/// request overlay and script-set values — are applied elsewhere):
/// secret value for the active env → selected option's value from the
/// env-merged list (enumerated/group) → simple env value → declaration
/// default. Names needing a selection or secret are omitted from `values`
/// but still get a `meta` entry. Undeclared env values pass through into
/// `values` with no `meta` entry (stage-3 leniency).
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

        let merged_options = merged_var_options(model, env, name);
        if !merged_options.is_empty() {
            match selections
                .get(name)
                .and_then(|key| merged_options.get(key).map(|opt| (key, opt)))
            {
                Some((key, opt)) => {
                    values.insert(name.clone(), opt.value.clone());
                    meta.insert(
                        name.clone(),
                        VarMeta::Enumerated {
                            selected: key.clone(),
                        },
                    );
                }
                None => {
                    meta.insert(name.clone(), VarMeta::NeedsSelection);
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
        let merged_options = merged_group_options(model, env, group_name);
        let selected = selections
            .get(group_name)
            .and_then(|key| merged_options.get(key).map(|opt| (key, opt)));

        for member in &group_decl.members {
            match selected {
                Some((key, opt)) => {
                    if let Some(value) = opt.values.get(member) {
                        values.insert(member.clone(), value.clone());
                    } else {
                        values.shift_remove(member);
                    }
                    meta.insert(
                        member.clone(),
                        VarMeta::GroupMember {
                            group: group_name.clone(),
                            selected: key.clone(),
                        },
                    );
                }
                None => {
                    values.shift_remove(member);
                    meta.insert(member.clone(), VarMeta::NeedsSelection);
                }
            }
        }
    }

    let declared_members: std::collections::HashSet<&str> = model
        .groups
        .values()
        .flat_map(|g| g.members.iter().map(String::as_str))
        .collect();
    for (name, value) in &env.values {
        if model.vars.contains_key(name) || declared_members.contains(name.as_str()) {
            continue;
        }
        values.insert(name.clone(), value.clone());
    }

    Resolved { values, meta }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_simple_secret_enumerated_and_group() {
        let m = parse_variables(
            r#"
[base_url]
description = "API root"
default = "http://localhost:8080"

[api_key]
secret = true

[user]
[user.options.alice]
description = "admin"
value = "1001"
[user.options.bob]
value = "2002"

[groups.test-user]
members = ["user_id", "customer_id"]
[groups.test-user.options.alice]
user_id = "1001"
customer_id = "c-77"
"#,
        )
        .unwrap();
        assert_eq!(
            m.vars["user"].options.keys().collect::<Vec<_>>(),
            ["alice", "bob"]
        );
        assert!(m.vars["api_key"].secret);
        assert_eq!(m.groups["test-user"].members, ["user_id", "customer_id"]);
        assert_eq!(
            m.groups["test-user"].options["alice"].values["customer_id"],
            "c-77"
        );
    }

    #[test]
    fn base_url_decl_fields_are_captured() {
        let m = parse_variables(
            r#"
[base_url]
description = "API root"
default = "http://localhost:8080"
"#,
        )
        .unwrap();
        let decl = &m.vars["base_url"];
        assert_eq!(decl.description.as_deref(), Some("API root"));
        assert_eq!(decl.default.as_deref(), Some("http://localhost:8080"));
        assert!(!decl.secret);
        assert!(decl.options.is_empty());
    }

    #[test]
    fn variable_named_options_is_rejected() {
        let err = parse_variables(
            r#"
[options]
default = "x"
"#,
        )
        .unwrap_err();
        assert_eq!(
            err.to_string(),
            ModelError::ReservedName("options".into()).to_string()
        );
        assert!(err.to_string().contains("reserved"));
    }

    #[test]
    fn variable_named_groups_is_rejected() {
        // `[groups]` is always the group container; a flat field under it
        // (as if declaring a variable named "groups") is rejected the same
        // way as `[options]` above.
        let err = parse_variables(
            r#"
[groups]
default = "x"
"#,
        )
        .unwrap_err();
        assert_eq!(
            err.to_string(),
            ModelError::ReservedName("groups".into()).to_string()
        );
        assert!(err.to_string().contains("reserved"));
    }

    #[test]
    fn group_name_colliding_with_variable_name_is_rejected() {
        let err = parse_variables(
            r#"
[widget]
default = "x"

[groups.widget]
members = ["thing"]
"#,
        )
        .unwrap_err();
        assert_eq!(err, ModelError::NameCollision("widget".into()));
        assert!(err.to_string().contains("widget"));
        assert!(err.to_string().to_lowercase().contains("both"));
    }

    #[test]
    fn member_with_its_own_options_is_rejected() {
        let err = parse_variables(
            r#"
[session_id]
[session_id.options.x]
value = "1"

[groups.auth_group]
members = ["session_id"]
"#,
        )
        .unwrap_err();
        assert_eq!(
            err,
            ModelError::MemberHasOptions {
                group: "auth_group".into(),
                member: "session_id".into(),
            }
        );
    }

    #[test]
    fn member_declared_secret_is_rejected() {
        let err = parse_variables(
            r#"
[api_key]
secret = true

[groups.creds]
members = ["api_key"]
"#,
        )
        .unwrap_err();
        assert_eq!(
            err,
            ModelError::MemberIsSecret {
                group: "creds".into(),
                member: "api_key".into(),
            }
        );
    }

    #[test]
    fn variable_in_two_groups_is_rejected() {
        let err = parse_variables(
            r#"
[groups.team_a]
members = ["shared_id"]

[groups.team_b]
members = ["shared_id"]
"#,
        )
        .unwrap_err();
        assert_eq!(
            err,
            ModelError::VariableInMultipleGroups {
                var: "shared_id".into(),
                first: "team_a".into(),
                second: "team_b".into(),
            }
        );
    }

    #[test]
    fn secret_with_default_is_rejected() {
        let err = parse_variables(
            r#"
[token]
secret = true
default = "x"
"#,
        )
        .unwrap_err();
        assert_eq!(err, ModelError::SecretWithDefault("token".into()));
        assert!(err.to_string().contains("token"));
        assert!(err.to_string().contains("default"));
    }

    #[test]
    fn secret_with_options_is_rejected() {
        let err = parse_variables(
            r#"
[token]
secret = true
[token.options.x]
value = "1"
"#,
        )
        .unwrap_err();
        assert_eq!(err, ModelError::SecretWithOptions("token".into()));
        assert!(err.to_string().contains("token"));
        assert!(err.to_string().contains("options"));
    }

    #[test]
    fn invalid_variable_name_is_rejected() {
        let err = parse_variables(
            r#"
["bad name!"]
default = "x"
"#,
        )
        .unwrap_err();
        assert_eq!(err, ModelError::InvalidName("bad name!".into()));
        assert!(err.to_string().contains("bad name!"));
    }

    #[test]
    fn invalid_option_key_name_is_rejected() {
        let err = parse_variables(
            r#"
[widget]
[widget.options."bad key!"]
value = "1"
"#,
        )
        .unwrap_err();
        assert_eq!(err, ModelError::InvalidName("bad key!".into()));
        assert!(err.to_string().contains("bad key!"));
    }

    #[test]
    fn unknown_field_in_variable_table_is_rejected() {
        let err = parse_variables(
            r#"
[widget]
color = "blue"
"#,
        )
        .unwrap_err();
        assert_eq!(
            err,
            ModelError::UnknownField {
                table: "widget".into(),
                field: "color".into(),
            }
        );
    }

    #[test]
    fn env_flat_pairs_and_options_tables_parse() {
        let e = parse_environment(
            r#"
base_url = "https://qa.example.com"

[options.user.alice]
value = "9001"
[options.user.qa-only]
description = "exists only in qa"
value = "3003"
[options.test-user.alice]
user_id = "9001"
"#,
        )
        .unwrap();
        assert_eq!(e.values["base_url"], "https://qa.example.com");
        assert_eq!(e.options["user"]["alice"]["value"], "9001");
        assert_eq!(
            e.options["user"]["qa-only"]["description"],
            "exists only in qa"
        );
        assert_eq!(e.options["test-user"]["alice"]["user_id"], "9001");
    }

    #[test]
    fn env_non_table_under_options_errors() {
        let err = parse_environment(
            r#"
[options]
user = "not-a-table"
"#,
        )
        .unwrap_err();
        assert_eq!(err, ModelError::OptionsNotTable("user".into()));
        assert!(err.to_string().contains("options.user"));
    }

    #[test]
    fn env_non_string_flat_value_errors() {
        let err = parse_environment(
            r#"
base_url = 123
"#,
        )
        .unwrap_err();
        assert_eq!(
            err,
            ModelError::NotAString {
                table: "<root>".into(),
                field: "base_url".into(),
            }
        );
        assert!(err.to_string().contains("base_url"));
    }

    // -------------------------------------------------------------
    // merge + validate_env
    // -------------------------------------------------------------

    #[test]
    fn merged_var_options_overrides_value_keeps_shared_description() {
        let m = parse_variables(
            r#"
[user]
[user.options.alice]
description = "admin"
value = "1001"
[user.options.bob]
value = "2002"
"#,
        )
        .unwrap();
        let e = parse_environment(
            r#"
[options.user.alice]
value = "9001"
[options.user.qa-only]
description = "exists only in qa"
value = "3003"
"#,
        )
        .unwrap();
        let merged = merged_var_options(&m, &e, "user");
        assert_eq!(
            merged.keys().collect::<Vec<_>>(),
            ["alice", "bob", "qa-only"]
        );
        assert_eq!(merged["alice"].value, "9001");
        assert_eq!(merged["alice"].description.as_deref(), Some("admin"));
        assert_eq!(merged["bob"].value, "2002");
        assert_eq!(merged["qa-only"].value, "3003");
        assert_eq!(
            merged["qa-only"].description.as_deref(),
            Some("exists only in qa")
        );
    }

    #[test]
    fn merged_var_options_wholesale_when_no_shared_options() {
        let m = parse_variables(
            r#"
[user]
"#,
        )
        .unwrap();
        let e = parse_environment(
            r#"
[options.user.alice]
value = "9001"
[options.user.qa-only]
value = "3003"
"#,
        )
        .unwrap();
        let merged = merged_var_options(&m, &e, "user");
        assert_eq!(merged.keys().collect::<Vec<_>>(), ["alice", "qa-only"]);
        assert_eq!(merged["alice"].value, "9001");
        assert!(merged["alice"].description.is_none());
        assert_eq!(merged["qa-only"].value, "3003");
    }

    #[test]
    fn merged_group_options_member_value_override() {
        let m = parse_variables(
            r#"
[groups.test-user]
members = ["user_id", "customer_id"]
[groups.test-user.options.alice]
user_id = "1001"
customer_id = "c-77"
"#,
        )
        .unwrap();
        let e = parse_environment(
            r#"
[options.test-user.alice]
user_id = "9001"
"#,
        )
        .unwrap();
        let merged = merged_group_options(&m, &e, "test-user");
        assert_eq!(merged["alice"].values["user_id"], "9001");
        assert_eq!(merged["alice"].values["customer_id"], "c-77");
    }

    #[test]
    fn validate_env_flat_value_for_var_enumerated_in_this_env_errors() {
        let m = parse_variables(
            r#"
[user]
"#,
        )
        .unwrap();
        let e = parse_environment(
            r#"
user = "alice"
[options.user.alice]
value = "9001"
"#,
        )
        .unwrap();
        let err = validate_env(&m, &e).unwrap_err();
        assert_eq!(err, ModelError::EnvValueForEnumerated("user".into()));
    }

    #[test]
    fn validate_env_flat_value_for_var_not_enumerated_in_other_env_is_ok() {
        let m = parse_variables(
            r#"
[user]
"#,
        )
        .unwrap();
        let e = parse_environment(
            r#"
user = "alice"
"#,
        )
        .unwrap();
        validate_env(&m, &e).unwrap();
    }

    #[test]
    fn validate_env_flat_value_for_secret_var_errors() {
        let m = parse_variables(
            r#"
[api_key]
secret = true
"#,
        )
        .unwrap();
        let e = parse_environment(
            r#"
api_key = "sekret"
"#,
        )
        .unwrap();
        let err = validate_env(&m, &e).unwrap_err();
        assert_eq!(err, ModelError::EnvValueForSecret("api_key".into()));
    }

    #[test]
    fn validate_env_flat_value_naming_a_group_errors() {
        let m = parse_variables(
            r#"
[groups.test-user]
members = ["user_id", "customer_id"]
"#,
        )
        .unwrap();
        let e = parse_environment(
            r#"
test-user = "alice"
"#,
        )
        .unwrap();
        let err = validate_env(&m, &e).unwrap_err();
        assert_eq!(err, ModelError::EnvValueForGroup("test-user".into()));
        assert!(err.to_string().contains("test-user"));
    }

    #[test]
    fn validate_env_flat_value_naming_a_group_member_errors() {
        let m = parse_variables(
            r#"
[groups.test-user]
members = ["user_id", "customer_id"]
"#,
        )
        .unwrap();
        let e = parse_environment(
            r#"
user_id = "1001"
"#,
        )
        .unwrap();
        let err = validate_env(&m, &e).unwrap_err();
        assert_eq!(
            err,
            ModelError::EnvValueForGroupMember {
                name: "user_id".into(),
                group: "test-user".into(),
            }
        );
        assert!(err.to_string().contains("user_id"));
        assert!(err.to_string().contains("test-user"));
    }

    #[test]
    fn validate_env_options_table_for_undeclared_name_errors() {
        let m = parse_variables(
            r#"
[base_url]
default = "x"
"#,
        )
        .unwrap();
        let e = parse_environment(
            r#"
[options.nope.alice]
value = "1"
"#,
        )
        .unwrap();
        let err = validate_env(&m, &e).unwrap_err();
        assert_eq!(err, ModelError::EnvOptionsUndeclared("nope".into()));
    }

    #[test]
    fn validate_env_options_table_for_secret_var_errors() {
        let m = parse_variables(
            r#"
[api_key]
secret = true
"#,
        )
        .unwrap();
        let e = parse_environment(
            r#"
[options.api_key.x]
value = "1"
"#,
        )
        .unwrap();
        let err = validate_env(&m, &e).unwrap_err();
        assert_eq!(err, ModelError::EnvOptionsForSecret("api_key".into()));
    }

    #[test]
    fn validate_env_group_option_row_naming_non_member_errors() {
        let m = parse_variables(
            r#"
[groups.test-user]
members = ["user_id", "customer_id"]
"#,
        )
        .unwrap();
        let e = parse_environment(
            r#"
[options.test-user.alice]
user_id = "1001"
bogus_field = "x"
"#,
        )
        .unwrap();
        let err = validate_env(&m, &e).unwrap_err();
        assert_eq!(
            err,
            ModelError::EnvGroupOptionNonMember {
                group: "test-user".into(),
                key: "alice".into(),
                member: "bogus_field".into(),
            }
        );
    }

    #[test]
    fn validate_env_missing_value_for_new_var_option_key_errors() {
        let m = parse_variables(
            r#"
[user]
[user.options.alice]
value = "1001"
"#,
        )
        .unwrap();
        let e = parse_environment(
            r#"
[options.user.qa-only]
description = "no value here"
"#,
        )
        .unwrap();
        let err = validate_env(&m, &e).unwrap_err();
        assert_eq!(
            err,
            ModelError::EnvOptionMissingValue {
                name: "user".into(),
                key: "qa-only".into(),
            }
        );
    }

    #[test]
    fn validate_env_ok_for_valid_env() {
        let m = parse_variables(
            r#"
[base_url]
default = "http://localhost"

[api_key]
secret = true

[user]
[user.options.alice]
value = "1001"

[groups.test-user]
members = ["user_id", "customer_id"]
[groups.test-user.options.alice]
user_id = "1001"
customer_id = "c-77"
"#,
        )
        .unwrap();
        let e = parse_environment(
            r#"
base_url = "https://qa.example.com"

[options.user.alice]
value = "9001"
[options.user.qa-only]
value = "3003"
[options.test-user.alice]
user_id = "9001"
"#,
        )
        .unwrap();
        validate_env(&m, &e).unwrap();
    }

    // -------------------------------------------------------------
    // resolve_env
    // -------------------------------------------------------------

    #[test]
    fn resolve_secret_present_yields_secret_meta_and_value() {
        let m = parse_variables(
            r#"
[api_key]
secret = true
"#,
        )
        .unwrap();
        let e = parse_environment("").unwrap();
        let selections = Selections::new();
        let mut secrets = SecretValues::new();
        secrets.insert("api_key".into(), "sk-qa-123".into());
        let r = resolve_env(&m, &e, &selections, &secrets);
        assert_eq!(r.values["api_key"], "sk-qa-123");
        assert_eq!(r.meta["api_key"], VarMeta::Secret);
    }

    #[test]
    fn resolve_secret_absent_yields_missing_secret_and_omitted() {
        let m = parse_variables(
            r#"
[api_key]
secret = true
"#,
        )
        .unwrap();
        let e = parse_environment("").unwrap();
        let r = resolve_env(&m, &e, &Selections::new(), &SecretValues::new());
        assert!(!r.values.contains_key("api_key"));
        assert_eq!(r.meta["api_key"], VarMeta::MissingSecret);
    }

    #[test]
    fn resolve_enumerated_var_with_selection_resolves_value() {
        let m = parse_variables(
            r#"
[user]
[user.options.alice]
value = "1001"
[user.options.bob]
value = "2002"
"#,
        )
        .unwrap();
        let e = parse_environment("").unwrap();
        let mut selections = Selections::new();
        selections.insert("user".into(), "bob".into());
        let r = resolve_env(&m, &e, &selections, &SecretValues::new());
        assert_eq!(r.values["user"], "2002");
        assert_eq!(
            r.meta["user"],
            VarMeta::Enumerated {
                selected: "bob".into()
            }
        );
    }

    #[test]
    fn resolve_enumerated_var_without_selection_needs_selection_and_omitted() {
        let m = parse_variables(
            r#"
[user]
[user.options.alice]
value = "1001"
"#,
        )
        .unwrap();
        let e = parse_environment("").unwrap();
        let r = resolve_env(&m, &e, &Selections::new(), &SecretValues::new());
        assert!(!r.values.contains_key("user"));
        assert_eq!(r.meta["user"], VarMeta::NeedsSelection);
    }

    #[test]
    fn resolve_enumerated_var_with_stale_selection_needs_selection_and_omitted() {
        let m = parse_variables(
            r#"
[user]
[user.options.alice]
value = "1001"
"#,
        )
        .unwrap();
        let e = parse_environment("").unwrap();
        let mut selections = Selections::new();
        selections.insert("user".into(), "ghost".into());
        let r = resolve_env(&m, &e, &selections, &SecretValues::new());
        assert!(!r.values.contains_key("user"));
        assert_eq!(r.meta["user"], VarMeta::NeedsSelection);
    }

    #[test]
    fn resolve_simple_env_value_used_over_default() {
        let m = parse_variables(
            r#"
[base_url]
default = "http://localhost:8080"
"#,
        )
        .unwrap();
        let e = parse_environment(
            r#"
base_url = "https://qa.example.com"
"#,
        )
        .unwrap();
        let r = resolve_env(&m, &e, &Selections::new(), &SecretValues::new());
        assert_eq!(r.values["base_url"], "https://qa.example.com");
        assert_eq!(r.meta["base_url"], VarMeta::Simple);
    }

    #[test]
    fn resolve_falls_back_to_default_when_no_env_value() {
        let m = parse_variables(
            r#"
[base_url]
default = "http://localhost:8080"
"#,
        )
        .unwrap();
        let e = parse_environment("").unwrap();
        let r = resolve_env(&m, &e, &Selections::new(), &SecretValues::new());
        assert_eq!(r.values["base_url"], "http://localhost:8080");
        assert_eq!(r.meta["base_url"], VarMeta::Simple);
    }

    #[test]
    fn resolve_group_member_from_selected_option() {
        let m = parse_variables(
            r#"
[groups.test-user]
members = ["user_id", "customer_id"]
[groups.test-user.options.alice]
user_id = "1001"
customer_id = "c-77"
"#,
        )
        .unwrap();
        let e = parse_environment("").unwrap();
        let mut selections = Selections::new();
        selections.insert("test-user".into(), "alice".into());
        let r = resolve_env(&m, &e, &selections, &SecretValues::new());
        assert_eq!(r.values["user_id"], "1001");
        assert_eq!(r.values["customer_id"], "c-77");
        assert_eq!(
            r.meta["user_id"],
            VarMeta::GroupMember {
                group: "test-user".into(),
                selected: "alice".into()
            }
        );
        assert_eq!(
            r.meta["customer_id"],
            VarMeta::GroupMember {
                group: "test-user".into(),
                selected: "alice".into()
            }
        );
    }

    #[test]
    fn resolve_group_member_without_selection_needs_selection_and_omitted() {
        let m = parse_variables(
            r#"
[groups.test-user]
members = ["user_id", "customer_id"]
[groups.test-user.options.alice]
user_id = "1001"
customer_id = "c-77"
"#,
        )
        .unwrap();
        let e = parse_environment("").unwrap();
        let r = resolve_env(&m, &e, &Selections::new(), &SecretValues::new());
        assert!(!r.values.contains_key("user_id"));
        assert!(!r.values.contains_key("customer_id"));
        assert_eq!(r.meta["user_id"], VarMeta::NeedsSelection);
        assert_eq!(r.meta["customer_id"], VarMeta::NeedsSelection);
    }

    #[test]
    fn resolve_group_member_with_stale_selection_needs_selection_and_omitted() {
        let m = parse_variables(
            r#"
[groups.test-user]
members = ["user_id", "customer_id"]
[groups.test-user.options.alice]
user_id = "1001"
customer_id = "c-77"
"#,
        )
        .unwrap();
        let e = parse_environment("").unwrap();
        let mut selections = Selections::new();
        selections.insert("test-user".into(), "ghost".into());
        let r = resolve_env(&m, &e, &selections, &SecretValues::new());
        assert!(!r.values.contains_key("user_id"));
        assert_eq!(r.meta["user_id"], VarMeta::NeedsSelection);
    }

    #[test]
    fn resolve_per_env_enumerated_var_resolves_in_that_env_and_simple_in_another() {
        let m = parse_variables(
            r#"
[user]
"#,
        )
        .unwrap();
        let qa = parse_environment(
            r#"
[options.user.alice]
value = "9001"
"#,
        )
        .unwrap();
        let mut selections = Selections::new();
        selections.insert("user".into(), "alice".into());
        let r_qa = resolve_env(&m, &qa, &selections, &SecretValues::new());
        assert_eq!(r_qa.values["user"], "9001");
        assert_eq!(
            r_qa.meta["user"],
            VarMeta::Enumerated {
                selected: "alice".into()
            }
        );

        let dev = parse_environment(
            r#"
user = "plain-value"
"#,
        )
        .unwrap();
        let r_dev = resolve_env(&m, &dev, &Selections::new(), &SecretValues::new());
        assert_eq!(r_dev.values["user"], "plain-value");
        assert_eq!(r_dev.meta["user"], VarMeta::Simple);
    }

    #[test]
    fn resolve_undeclared_env_value_passes_through_with_no_meta() {
        let m = parse_variables(
            r#"
[base_url]
default = "http://localhost:8080"
"#,
        )
        .unwrap();
        let e = parse_environment(
            r#"
base_url = "https://qa.example.com"
legacy_flag = "on"
"#,
        )
        .unwrap();
        let r = resolve_env(&m, &e, &Selections::new(), &SecretValues::new());
        assert_eq!(r.values["legacy_flag"], "on");
        assert!(!r.meta.contains_key("legacy_flag"));
    }
}

//! The variable model: parsing `variables.toml` (variable declarations
//! and selector declarations listing their `fields`) and
//! `environments/<env>.toml` (flat values for simple variables plus that
//! environment's selector `options`).
//!
//! A selector is a set of linked fields with named options — records you
//! pick among. Selecting an option fills every field of the selector at
//! once, and options belong to one specific environment.

use indexmap::IndexMap;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct VarDecl {
    pub description: Option<String>,
    pub default: Option<String>,
    pub secret: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SelectorDecl {
    pub description: Option<String>,
    /// Ordered field names; every option of the selector supplies all of
    /// them. Never empty: a selector with no fields is a parse error.
    pub fields: Vec<String>,
    /// A shared selector's options live in `variables.toml` (the model's
    /// own `options`), identical in every environment, instead of in each
    /// `environments/<env>.toml`.
    pub shared: bool,
}

/// One named record of a selector, in one environment.
#[derive(Debug, Clone, PartialEq)]
pub struct OptionDecl {
    pub description: Option<String>,
    /// field name → value
    pub values: IndexMap<String, String>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct VarModel {
    pub vars: IndexMap<String, VarDecl>,
    pub selectors: IndexMap<String, SelectorDecl>,
    /// Shared selectors' options (`selector name → option name → option`),
    /// parsed from `[options.*]` tables in `variables.toml` itself.
    pub options: IndexMap<String, IndexMap<String, OptionDecl>>,
}

/// Flat values for simple variables plus this environment's selector
/// options.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EnvData {
    pub values: IndexMap<String, String>,
    /// selector name → option name → option
    pub options: IndexMap<String, IndexMap<String, OptionDecl>>,
}

/// One env's `selector name → selected option name`. Legacy per-variable
/// selections use the same map (a migrated enumerated variable becomes a
/// one-field selector of the same name, so its key carries over unchanged).
pub type Selections = IndexMap<String, String>;

/// One env's `name → secret value`.
pub type SecretValues = IndexMap<String, String>;

/// Why a resolved (or unresolved) name has the value it has.
#[derive(Debug, Clone, PartialEq)]
pub enum VarMeta {
    Simple,
    SelectorMember {
        selector: String,
        selected: String,
    },
    Secret,
    /// A selector field whose selector has no (or a stale) selection.
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
    /// Every declared name (vars + selector fields) has an entry.
    pub meta: IndexMap<String, VarMeta>,
}

#[derive(thiserror::Error, Debug, Clone, PartialEq)]
pub enum ModelError {
    #[error("could not parse TOML: {0}")]
    Toml(String),
    #[error(
        "\"{0}\" is a reserved name and can't be used for a variable or selector; rename it to something else"
    )]
    ReservedName(String),
    #[error(
        "\"{0}\" is not a valid name; use only ASCII letters, digits, `_`, and `-`, and make it non-empty"
    )]
    InvalidName(String),
    #[error(
        "\"{0}\" is declared as both a variable and a selector; rename one so the name is unique"
    )]
    NameCollision(String),
    #[error("\"{0}\" must be a table, e.g. [{0}]")]
    NotATable(String),
    #[error("[{table}] has unknown field \"{field}\"; remove it or fix the typo")]
    UnknownField { table: String, field: String },
    #[error("[{table}] field \"{field}\" must be a string")]
    NotAString { table: String, field: String },
    #[error("[{table}] field \"{field}\" must be a boolean (true or false)")]
    NotABool { table: String, field: String },
    #[error("[selectors.{selector}] field \"fields\" must be an array of variable names")]
    FieldsNotArray { selector: String },
    #[error("[selectors.{selector}] is missing required field \"fields\"")]
    MissingFields { selector: String },
    #[error("[selectors.{selector}] has an empty field list; a selector needs at least one field")]
    EmptyFields { selector: String },
    #[error(
        "variable \"{0}\" is secret = true and also sets a default; remove default so no secret value is committed to variables.toml"
    )]
    SecretWithDefault(String),
    #[error(
        "selector \"{selector}\" lists field \"{field}\", which is declared secret = true; a secret can't be a selector field"
    )]
    FieldIsSecret { selector: String, field: String },
    #[error(
        "selector \"{selector}\" lists field \"{field}\", which is also declared as a variable; a selector field's value comes from the selector's options, so remove the [{field}] declaration"
    )]
    FieldCollidesWithVar { selector: String, field: String },
    #[error(
        "\"{field}\" is a field of both selector \"{first}\" and selector \"{second}\"; a field can only belong to one selector"
    )]
    FieldInMultipleSelectors {
        field: String,
        first: String,
        second: String,
    },
    #[error("[options.{0}] must be a table (e.g. [options.{0}.\"option name\"])")]
    OptionsNotTable(String),
    #[error("[options.{selector}.\"{option}\"] must be a table of field values")]
    OptionNotTable { selector: String, option: String },
    #[error("[options.{selector}.\"{option}\"] field \"{field}\" must be a string")]
    OptionFieldNotString {
        selector: String,
        option: String,
        field: String,
    },
    #[error(
        "[options.{0}] does not match a declared selector; declare [selectors.{0}] in variables.toml or fix the typo"
    )]
    OptionForUndeclaredSelector(String),
    #[error(
        "[options.{selector}.\"{option}\"] is missing field \"{field}\"; every option must supply all of the selector's fields"
    )]
    OptionMissingField {
        selector: String,
        option: String,
        field: String,
    },
    #[error(
        "[options.{selector}.\"{option}\"] sets \"{field}\", which is not a field of selector \"{selector}\"; fix the typo or add \"{field}\" to the selector's fields"
    )]
    OptionUnknownField {
        selector: String,
        option: String,
        field: String,
    },
    #[error("[options.{selector}] has an option with an empty name; give every option a name")]
    OptionEmptyName { selector: String },
    #[error(
        "[options.{selector}] has an option named \"description\"; that name is reserved for an option's own description, so rename the option"
    )]
    OptionNameReserved { selector: String },
    #[error(
        "selector \"{0}\" is not shared, so its options live in environments/<env>.toml; move [options.{0}] there or declare shared = true on [selectors.{0}]"
    )]
    OptionsForUnsharedSelector(String),
    #[error(
        "selector \"{0}\" is shared, so its options live in variables.toml; move [options.{0}] there or remove shared = true from [selectors.{0}]"
    )]
    EnvOptionsForSharedSelector(String),
    #[error(
        "environment sets a flat value for secret variable \"{0}\"; secrets can't be committed to environments/<env>.toml"
    )]
    EnvValueForSecret(String),
    #[error(
        "environment sets a flat value for \"{0}\", which is a selector name; selectors don't take a flat value, use [options.{0}.\"<option>\"] instead"
    )]
    EnvValueForSelector(String),
    #[error(
        "environment sets a flat value for \"{name}\", which is a field of selector \"{selector}\"; its value comes from the selector's selected option, not a flat value"
    )]
    EnvValueForField { name: String, selector: String },
    #[error(
        "environment still uses the old [entries.*] tables; accept the migration prompt (or rename them to [options.*]) to load it"
    )]
    EnvLegacyEntries,
}

// ---------------------------------------------------------------------
// variables.toml
// ---------------------------------------------------------------------

const RESERVED_NAMES: [&str; 4] = ["options", "groups", "entries", "selectors"];

/// The one key inside an option table that is the option's own description
/// rather than a field value — and so the one name an option may not have.
pub const OPTION_DESCRIPTION: &str = "description";

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

fn parse_selector_decl(
    value: &toml::Value,
    selector_name: &str,
) -> Result<SelectorDecl, ModelError> {
    let table_path = format!("selectors.{selector_name}");
    let table = as_table(value, &table_path)?;
    check_unknown_fields(table, &["description", "fields", "shared"], &table_path)?;
    let description = get_string(table, "description", &table_path)?;
    let shared = get_bool(table, "shared", &table_path)?;
    let fields_value = table
        .get("fields")
        .ok_or_else(|| ModelError::MissingFields {
            selector: selector_name.to_string(),
        })?;
    let fields_array = fields_value
        .as_array()
        .ok_or_else(|| ModelError::FieldsNotArray {
            selector: selector_name.to_string(),
        })?;
    let mut fields = Vec::new();
    for f in fields_array {
        let name = f.as_str().ok_or_else(|| ModelError::FieldsNotArray {
            selector: selector_name.to_string(),
        })?;
        check_name(name)?;
        fields.push(name.to_string());
    }
    if fields.is_empty() {
        return Err(ModelError::EmptyFields {
            selector: selector_name.to_string(),
        });
    }
    Ok(SelectorDecl {
        description,
        fields,
        shared,
    })
}

pub fn parse_variables(s: &str) -> Result<VarModel, ModelError> {
    let top: IndexMap<String, toml::Value> =
        toml::from_str(s).map_err(|e| ModelError::Toml(e.to_string()))?;

    let mut vars = IndexMap::new();
    for (name, value) in &top {
        if name == "selectors" || name == "options" {
            continue;
        }
        check_name(name)?;
        vars.insert(name.clone(), parse_var_decl(value, name)?);
    }

    let mut selectors = IndexMap::new();
    // field name -> selector name that has already claimed it
    let mut field_owner: IndexMap<String, String> = IndexMap::new();
    if let Some(selectors_value) = top.get("selectors") {
        let selectors_table = as_table(selectors_value, "selectors")?;
        for (selector_name, value) in selectors_table {
            // A flat (non-table) entry under `[selectors]` means someone
            // tried to use the reserved name "selectors" as a plain
            // variable table (e.g. `[selectors]\ndefault = "x"`) rather
            // than declaring a nested `[selectors.<name>]`.
            if value.as_table().is_none() {
                return Err(ModelError::ReservedName("selectors".to_string()));
            }
            check_name(selector_name)?;
            if vars.contains_key(selector_name) {
                return Err(ModelError::NameCollision(selector_name.clone()));
            }
            let decl = parse_selector_decl(value, selector_name)?;
            for field in &decl.fields {
                if let Some(existing) = field_owner.get(field) {
                    return Err(ModelError::FieldInMultipleSelectors {
                        field: field.clone(),
                        first: existing.clone(),
                        second: selector_name.clone(),
                    });
                }
                field_owner.insert(field.clone(), selector_name.clone());
                // A selector MAY share its name with one of its own fields
                // (the one-field selector a migrated enumerated variable
                // becomes); it may not share it with a *variable*, which
                // the NameCollision check above already rejected.
                if let Some(var_decl) = vars.get(field) {
                    if var_decl.secret {
                        return Err(ModelError::FieldIsSecret {
                            selector: selector_name.clone(),
                            field: field.clone(),
                        });
                    }
                    return Err(ModelError::FieldCollidesWithVar {
                        selector: selector_name.clone(),
                        field: field.clone(),
                    });
                }
            }
            selectors.insert(selector_name.clone(), decl);
        }
    }

    let mut options: IndexMap<String, IndexMap<String, OptionDecl>> = IndexMap::new();
    if let Some(options_value) = top.get("options") {
        let options_table = as_table(options_value, "options")?;
        for (selector, selector_value) in options_table {
            // A flat (non-table) entry under `[options]` means someone
            // tried to use the reserved name "options" as a plain variable
            // table — same shape as the `[selectors]` case above.
            let Some(selector_table) = selector_value.as_table() else {
                return Err(ModelError::ReservedName("options".to_string()));
            };
            let decl = selectors
                .get(selector)
                .ok_or_else(|| ModelError::OptionForUndeclaredSelector(selector.clone()))?;
            if !decl.shared {
                return Err(ModelError::OptionsForUnsharedSelector(selector.clone()));
            }
            let per_selector = parse_selector_options(selector, selector_table)?;
            check_options_supply_fields(decl, selector, &per_selector)?;
            options.insert(selector.clone(), per_selector);
        }
    }

    Ok(VarModel {
        vars,
        selectors,
        options,
    })
}

// ---------------------------------------------------------------------
// environments/<env>.toml
// ---------------------------------------------------------------------

fn parse_option(
    value: &toml::Value,
    selector: &str,
    option: &str,
) -> Result<OptionDecl, ModelError> {
    let table = value.as_table().ok_or_else(|| ModelError::OptionNotTable {
        selector: selector.to_string(),
        option: option.to_string(),
    })?;
    let mut description = None;
    let mut values = IndexMap::new();
    for (field, field_value) in table {
        let s = field_value
            .as_str()
            .ok_or_else(|| ModelError::OptionFieldNotString {
                selector: selector.to_string(),
                option: option.to_string(),
                field: field.clone(),
            })?;
        if field == OPTION_DESCRIPTION {
            description = Some(s.to_string());
            continue;
        }
        values.insert(field.clone(), s.to_string());
    }
    Ok(OptionDecl {
        description,
        values,
    })
}

/// One selector's `[options.<selector>.*]` tables — the same shape in
/// `variables.toml` (a shared selector's options) and an environment file.
fn parse_selector_options(
    selector: &str,
    selector_table: &toml::map::Map<String, toml::Value>,
) -> Result<IndexMap<String, OptionDecl>, ModelError> {
    let mut per_selector = IndexMap::new();
    for (option_name, option_value) in selector_table {
        if option_name.is_empty() {
            return Err(ModelError::OptionEmptyName {
                selector: selector.to_string(),
            });
        }
        // `description` inside a selector's options table is an
        // option's own description, never an option name.
        if option_name == OPTION_DESCRIPTION {
            return Err(ModelError::OptionNameReserved {
                selector: selector.to_string(),
            });
        }
        per_selector.insert(
            option_name.clone(),
            parse_option(option_value, selector, option_name)?,
        );
    }
    Ok(per_selector)
}

pub fn parse_environment(s: &str) -> Result<EnvData, ModelError> {
    let top: IndexMap<String, toml::Value> =
        toml::from_str(s).map_err(|e| ModelError::Toml(e.to_string()))?;

    let mut values = IndexMap::new();
    let mut options: IndexMap<String, IndexMap<String, OptionDecl>> = IndexMap::new();

    for (key, value) in &top {
        if key == "entries" {
            return Err(ModelError::EnvLegacyEntries);
        }
        if key == "options" {
            let options_table = as_table(value, "options")?;
            for (selector, selector_value) in options_table {
                let selector_table = selector_value
                    .as_table()
                    .ok_or_else(|| ModelError::OptionsNotTable(selector.clone()))?;
                options.insert(
                    selector.clone(),
                    parse_selector_options(selector, selector_table)?,
                );
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

/// `selector`'s options from wherever they live — the model for a shared
/// selector, `env` for everyone else. `None` when there are no options (or
/// no such selector).
pub fn options_of<'a>(
    model: &'a VarModel,
    env: &'a EnvData,
    selector: &str,
) -> Option<&'a IndexMap<String, OptionDecl>> {
    if model.selectors.get(selector).is_some_and(|d| d.shared) {
        model.options.get(selector)
    } else {
        env.options.get(selector)
    }
}

/// This environment's options for `selector`, or `None` when the selector
/// has no options here.
pub fn selector_options<'a>(
    env: &'a EnvData,
    selector: &str,
) -> Option<&'a IndexMap<String, OptionDecl>> {
    env.options.get(selector)
}

// ---------------------------------------------------------------------
// env validation
// ---------------------------------------------------------------------

/// Friendly errors: a flat value naming a secret, a selector, or a
/// selector field; an `[options.<selector>]` table for an undeclared
/// selector; an option missing one of its selector's fields or setting a
/// field the selector doesn't declare.
pub fn validate_env(model: &VarModel, env: &EnvData) -> Result<(), ModelError> {
    for key in env.values.keys() {
        if let Some(decl) = model.vars.get(key) {
            if decl.secret {
                return Err(ModelError::EnvValueForSecret(key.clone()));
            }
            continue;
        }
        if model.selectors.contains_key(key) {
            return Err(ModelError::EnvValueForSelector(key.clone()));
        }
        if let Some(selector_name) = model
            .selectors
            .iter()
            .find(|(_, g)| g.fields.contains(key))
            .map(|(gname, _)| gname.clone())
        {
            return Err(ModelError::EnvValueForField {
                name: key.clone(),
                selector: selector_name,
            });
        }
    }

    for (selector, selector_opts) in &env.options {
        let decl = model
            .selectors
            .get(selector)
            .ok_or_else(|| ModelError::OptionForUndeclaredSelector(selector.clone()))?;
        if decl.shared {
            return Err(ModelError::EnvOptionsForSharedSelector(selector.clone()));
        }
        check_options_supply_fields(decl, selector, selector_opts)?;
    }

    Ok(())
}

/// Every option must supply exactly its selector's declared fields — the
/// same rule for an environment's options and a shared selector's options
/// in `variables.toml`.
fn check_options_supply_fields(
    decl: &SelectorDecl,
    selector: &str,
    options: &IndexMap<String, OptionDecl>,
) -> Result<(), ModelError> {
    for (option_name, option) in options {
        for field in &decl.fields {
            if !option.values.contains_key(field) {
                return Err(ModelError::OptionMissingField {
                    selector: selector.to_string(),
                    option: option_name.clone(),
                    field: field.clone(),
                });
            }
        }
        for field in option.values.keys() {
            if !decl.fields.contains(field) {
                return Err(ModelError::OptionUnknownField {
                    selector: selector.to_string(),
                    option: option_name.clone(),
                    field: field.clone(),
                });
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
/// env → the selected option's value for a selector field → flat env value
/// → declaration default. Names needing a selection or a secret are
/// omitted from `values` but still get a `meta` entry. Undeclared env
/// values pass through into `values` with no `meta` entry (stage-3
/// leniency).
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

    for (selector_name, selector_decl) in &model.selectors {
        // A shared selector's options live in the model itself; everyone
        // else's in the environment.
        let options_home = if selector_decl.shared {
            &model.options
        } else {
            &env.options
        };
        let selected = selections.get(selector_name).and_then(|name| {
            options_home
                .get(selector_name)
                .and_then(|options| options.get(name))
                .map(|option| (name, option))
        });

        for field in &selector_decl.fields {
            match selected {
                Some((name, option)) => {
                    if let Some(value) = option.values.get(field) {
                        values.insert(field.clone(), value.clone());
                    } else {
                        values.shift_remove(field);
                    }
                    meta.insert(
                        field.clone(),
                        VarMeta::SelectorMember {
                            selector: selector_name.clone(),
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
        .selectors
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
    fn parses_vars_and_selectors_with_fields() {
        let m = parse_variables(
            r#"
[base_url]
description = "API root"
default = "http://localhost:8080"

[api_key]
secret = true

[selectors.user]
description = "Linked user/customer pair"
fields = ["user_id", "customer_id"]
"#,
        )
        .unwrap();
        assert_eq!(m.selectors["user"].fields, ["user_id", "customer_id"]);
        assert_eq!(
            m.selectors["user"].description.as_deref(),
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
    fn parses_shared_selector_with_options_in_variables_toml() {
        let m = parse_variables(
            r#"
[selectors.locale]
shared = true
fields = ["lang", "date_format"]

[options.locale.english]
lang = "en"
date_format = "MM/DD"

[options.locale.french]
description = "the fancy one"
lang = "fr"
date_format = "DD/MM"
"#,
        )
        .unwrap();
        assert!(m.selectors["locale"].shared);
        assert_eq!(m.options["locale"]["english"].values["lang"], "en");
        assert_eq!(
            m.options["locale"]["french"].description.as_deref(),
            Some("the fancy one")
        );
    }

    #[test]
    fn selector_without_shared_flag_is_not_shared() {
        let m = parse_variables("[selectors.user]\nfields = [\"user_id\"]\n").unwrap();
        assert!(!m.selectors["user"].shared);
        assert!(m.options.is_empty());
    }

    #[test]
    fn variables_options_for_a_non_shared_selector_are_rejected() {
        let err = parse_variables(
            "[selectors.user]\nfields = [\"user_id\"]\n\n[options.user.alice]\nuser_id = \"1\"\n",
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("is not shared")
                && err.to_string().contains("shared = true"),
            "{err}"
        );
    }

    #[test]
    fn variables_options_for_an_undeclared_selector_are_rejected() {
        let err = parse_variables("[options.ghost.a]\nx = \"1\"\n").unwrap_err();
        assert!(
            err.to_string()
                .contains("[options.ghost] does not match a declared selector"),
            "{err}"
        );
    }

    #[test]
    fn shared_option_missing_a_field_is_rejected_at_parse() {
        let err = parse_variables(
            "[selectors.locale]\nshared = true\nfields = [\"lang\", \"fmt\"]\n\n[options.locale.en]\nlang = \"en\"\n",
        )
        .unwrap_err();
        assert!(
            err.to_string()
                .contains("[options.locale.\"en\"] is missing field \"fmt\""),
            "{err}"
        );
    }

    #[test]
    fn shared_option_with_an_extra_field_is_rejected_at_parse() {
        let err = parse_variables(
            "[selectors.locale]\nshared = true\nfields = [\"lang\"]\n\n[options.locale.en]\nlang = \"en\"\nnope = \"x\"\n",
        )
        .unwrap_err();
        assert!(
            err.to_string()
                .contains("sets \"nope\", which is not a field of selector \"locale\""),
            "{err}"
        );
    }

    #[test]
    fn env_options_for_a_shared_selector_are_rejected() {
        let m = parse_variables(
            "[selectors.locale]\nshared = true\nfields = [\"lang\"]\n\n[options.locale.en]\nlang = \"en\"\n",
        )
        .unwrap();
        let e = parse_environment("[options.locale.fr]\nlang = \"fr\"\n").unwrap();
        let err = validate_env(&m, &e).unwrap_err();
        assert!(
            err.to_string().contains("\"locale\" is shared")
                && err.to_string().contains("variables.toml"),
            "{err}"
        );
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
        // `groups` was the stage-7 selector container; it stays reserved so
        // stale files fail loudly rather than reading as a variable.
        let err = parse_variables("[groups]\ndefault = \"x\"\n").unwrap_err();
        assert!(
            err.to_string().contains("\"groups\" is a reserved name"),
            "{err}"
        );
    }

    #[test]
    fn legacy_groups_table_is_rejected_as_reserved() {
        // A stage-7 file's `[groups.user]` declaration parses as a table
        // named "groups" — reserved, so it errors instead of loading wrong.
        let err = parse_variables("[groups.user]\nfields = [\"user_id\"]\n").unwrap_err();
        assert!(
            err.to_string().contains("\"groups\" is a reserved name"),
            "{err}"
        );
    }

    #[test]
    fn variable_named_selectors_is_rejected_as_reserved() {
        // `[selectors]` is always the selector container; a flat field
        // under it (as if declaring a variable named "selectors") is
        // rejected.
        let err = parse_variables("[selectors]\ndefault = \"x\"\n").unwrap_err();
        assert!(
            err.to_string().contains("\"selectors\" is a reserved name"),
            "{err}"
        );
    }

    #[test]
    fn invalid_variable_name_is_rejected() {
        let err = parse_variables("[\"has space\"]\ndefault = \"x\"\n").unwrap_err();
        assert!(err.to_string().contains("is not a valid name"), "{err}");
    }

    #[test]
    fn selector_colliding_with_a_variable_name_is_rejected() {
        let err = parse_variables(
            r#"
[user]
default = "x"

[selectors.user]
fields = ["user_id"]
"#,
        )
        .unwrap_err();
        assert!(
            err.to_string()
                .contains("\"user\" is declared as both a variable and a selector"),
            "{err}"
        );
    }

    #[test]
    fn field_in_two_selectors_is_rejected() {
        let err = parse_variables(
            r#"
[selectors.a]
fields = ["shared"]

[selectors.b]
fields = ["shared"]
"#,
        )
        .unwrap_err();
        assert!(
            err.to_string()
                .contains("\"shared\" is a field of both selector \"a\" and selector \"b\""),
            "{err}"
        );
    }

    #[test]
    fn field_that_is_also_a_declared_variable_is_rejected() {
        let err = parse_variables(
            r#"
[user_id]
default = "1"

[selectors.user]
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

[selectors.user]
fields = ["token"]
"#,
        )
        .unwrap_err();
        assert!(
            err.to_string()
                .contains("which is declared secret = true; a secret can't be a selector field"),
            "{err}"
        );
    }

    #[test]
    fn selector_may_share_its_name_with_its_own_single_field() {
        let m = parse_variables("[selectors.tier]\nfields = [\"tier\"]\n").unwrap();
        assert_eq!(m.selectors["tier"].fields, ["tier"]);
    }

    #[test]
    fn selector_without_fields_is_rejected() {
        let err = parse_variables("[selectors.user]\ndescription = \"x\"\n").unwrap_err();
        assert!(
            err.to_string()
                .contains("[selectors.user] is missing required field \"fields\""),
            "{err}"
        );
    }

    #[test]
    fn selector_with_empty_fields_is_rejected() {
        let err = parse_variables("[selectors.user]\nfields = []\n").unwrap_err();
        assert!(
            err.to_string()
                .contains("[selectors.user] has an empty field list"),
            "{err}"
        );
    }

    #[test]
    fn selector_fields_must_be_an_array_of_strings() {
        let err = parse_variables("[selectors.user]\nfields = \"user_id\"\n").unwrap_err();
        assert!(
            err.to_string()
                .contains("field \"fields\" must be an array of variable names"),
            "{err}"
        );
        let err = parse_variables("[selectors.user]\nfields = [1]\n").unwrap_err();
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
        let err =
            parse_variables("[selectors.user]\nfields = [\"a\"]\nmembers = []\n").unwrap_err();
        assert!(
            err.to_string()
                .contains("[selectors.user] has unknown field \"members\""),
            "{err}"
        );
    }

    // -----------------------------------------------------------------
    // environments/<env>.toml
    // -----------------------------------------------------------------

    #[test]
    fn parses_env_options_and_resolves_selection() {
        let m =
            parse_variables("[selectors.user]\nfields = [\"user_id\", \"customer_id\"]\n").unwrap();
        let e = parse_environment(
            r#"
base_url = "https://stg.example.com"

[options.user."user 1"]
user_id = "1001"
customer_id = "cust-77"

[options.user."user 2"]
description = "the premium one"
user_id = "1002"
customer_id = "cust-91"
"#,
        )
        .unwrap();
        assert_eq!(e.values["base_url"], "https://stg.example.com");
        assert_eq!(e.options["user"]["user 2"].values["customer_id"], "cust-91");
        assert_eq!(
            e.options["user"]["user 2"].description.as_deref(),
            Some("the premium one")
        );
        assert!(
            !e.options["user"]["user 2"]
                .values
                .contains_key("description"),
            "an option's description is not one of its fields"
        );
        validate_env(&m, &e).unwrap();

        let mut sel = Selections::new();
        sel.insert("user".into(), "user 2".into());
        let r = resolve_env(&m, &e, &sel, &SecretValues::new());
        assert_eq!(r.values["user_id"], "1002");
        assert_eq!(
            r.meta["customer_id"],
            VarMeta::SelectorMember {
                selector: "user".into(),
                selected: "user 2".into()
            }
        );
    }

    #[test]
    fn legacy_env_entries_table_is_rejected() {
        let err = parse_environment("[entries.user.\"user 1\"]\nuser_id = \"1\"\n").unwrap_err();
        assert!(err.to_string().contains("old [entries.*] tables"), "{err}");
    }

    #[test]
    fn options_of_looks_in_the_selectors_home() {
        let m = parse_variables(
            "[selectors.locale]\nshared = true\nfields = [\"lang\"]\n\n[options.locale.en]\nlang = \"en\"\n\n[selectors.user]\nfields = [\"user_id\"]\n",
        )
        .unwrap();
        let e = parse_environment("[options.user.alice]\nuser_id = \"1\"\n").unwrap();
        assert!(options_of(&m, &e, "locale").unwrap().contains_key("en"));
        assert!(options_of(&m, &e, "user").unwrap().contains_key("alice"));
        assert!(options_of(&m, &e, "nope").is_none());
    }

    #[test]
    fn selector_options_looks_up_one_selectors_options() {
        let e = parse_environment("[options.user.\"user 1\"]\nuser_id = \"1\"\n").unwrap();
        assert_eq!(selector_options(&e, "user").unwrap().len(), 1);
        assert!(selector_options(&e, "nope").is_none());
    }

    #[test]
    fn options_table_for_an_undeclared_selector_is_rejected() {
        let m = parse_variables("").unwrap();
        let e = parse_environment("[options.ghost.\"a\"]\nx = \"1\"\n").unwrap();
        let err = validate_env(&m, &e).unwrap_err();
        assert!(
            err.to_string()
                .contains("[options.ghost] does not match a declared selector"),
            "{err}"
        );
    }

    #[test]
    fn option_missing_a_field_is_rejected() {
        let m =
            parse_variables("[selectors.user]\nfields = [\"user_id\", \"customer_id\"]\n").unwrap();
        let e = parse_environment("[options.user.\"user 1\"]\nuser_id = \"1\"\n").unwrap();
        let err = validate_env(&m, &e).unwrap_err();
        assert!(
            err.to_string()
                .contains("[options.user.\"user 1\"] is missing field \"customer_id\""),
            "{err}"
        );
    }

    #[test]
    fn option_with_an_extra_field_is_rejected() {
        let m = parse_variables("[selectors.user]\nfields = [\"user_id\"]\n").unwrap();
        let e = parse_environment("[options.user.\"user 1\"]\nuser_id = \"1\"\nnope = \"2\"\n")
            .unwrap();
        let err = validate_env(&m, &e).unwrap_err();
        assert!(
            err.to_string().contains(
                "[options.user.\"user 1\"] sets \"nope\", which is not a field of selector \"user\""
            ),
            "{err}"
        );
    }

    #[test]
    fn option_field_must_be_a_string() {
        let err = parse_environment("[options.user.\"user 1\"]\nuser_id = 1001\n").unwrap_err();
        assert!(
            err.to_string()
                .contains("[options.user.\"user 1\"] field \"user_id\" must be a string"),
            "{err}"
        );
    }

    #[test]
    fn option_must_be_a_table() {
        let err = parse_environment("[options.user]\n\"user 1\" = \"nope\"\n").unwrap_err();
        assert!(
            err.to_string()
                .contains("[options.user.\"user 1\"] must be a table of field values"),
            "{err}"
        );
    }

    #[test]
    fn options_selector_must_be_a_table() {
        let err = parse_environment("[options]\nuser = \"nope\"\n").unwrap_err();
        assert!(
            err.to_string().contains("[options.user] must be a table"),
            "{err}"
        );
    }

    #[test]
    fn empty_option_name_is_rejected() {
        let err = parse_environment("[options.user.\"\"]\nuser_id = \"1\"\n").unwrap_err();
        assert!(
            err.to_string()
                .contains("[options.user] has an option with an empty name"),
            "{err}"
        );
    }

    #[test]
    fn an_option_named_description_is_rejected() {
        // `description` in an options table is an option's own description,
        // so it can't double as an option name.
        let err = parse_environment("[options.user.description]\nuser_id = \"1\"\n").unwrap_err();
        assert!(
            err.to_string()
                .contains("[options.user] has an option named \"description\""),
            "{err}"
        );
    }

    #[test]
    fn flat_env_value_for_a_selector_is_rejected() {
        let m = parse_variables("[selectors.user]\nfields = [\"user_id\"]\n").unwrap();
        let e = parse_environment("user = \"x\"\n").unwrap();
        let err = validate_env(&m, &e).unwrap_err();
        assert!(
            err.to_string().contains("which is a selector name"),
            "{err}"
        );
    }

    #[test]
    fn flat_env_value_for_a_selector_field_is_rejected() {
        let m = parse_variables("[selectors.user]\nfields = [\"user_id\"]\n").unwrap();
        let e = parse_environment("user_id = \"x\"\n").unwrap();
        let err = validate_env(&m, &e).unwrap_err();
        assert!(
            err.to_string()
                .contains("which is a field of selector \"user\""),
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
        parse_variables("[selectors.user]\nfields = [\"user_id\", \"customer_id\"]\n").unwrap()
    }

    fn user_env() -> EnvData {
        parse_environment(
            "[options.user.\"user 1\"]\nuser_id = \"1001\"\ncustomer_id = \"cust-77\"\n",
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
        sel.insert("user".into(), "deleted option".into());
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
    fn shared_selector_resolves_from_model_options_in_any_env() {
        let m = parse_variables(
            "[selectors.locale]\nshared = true\nfields = [\"lang\"]\n\n[options.locale.en]\nlang = \"en\"\n\n[options.locale.fr]\nlang = \"fr\"\n",
        )
        .unwrap();
        let mut sel = Selections::new();
        sel.insert("locale".into(), "fr".into());
        // An empty env: the options come from the model, not the env.
        let r = resolve_env(&m, &EnvData::default(), &sel, &SecretValues::new());
        assert_eq!(r.values["lang"], "fr");
        assert_eq!(
            r.meta["lang"],
            VarMeta::SelectorMember {
                selector: "locale".into(),
                selected: "fr".into()
            }
        );
    }

    #[test]
    fn shared_selector_without_selection_needs_one() {
        let m = parse_variables(
            "[selectors.locale]\nshared = true\nfields = [\"lang\"]\n\n[options.locale.en]\nlang = \"en\"\n",
        )
        .unwrap();
        let r = resolve_env(
            &m,
            &EnvData::default(),
            &Selections::new(),
            &SecretValues::new(),
        );
        assert_eq!(r.meta["lang"], VarMeta::NeedsSelection);
        assert!(r.values.get("lang").is_none());
    }

    #[test]
    fn one_field_selector_sharing_its_name_resolves_through_the_selection() {
        let m = parse_variables("[selectors.tier]\nfields = [\"tier\"]\n").unwrap();
        let e = parse_environment(
            "[options.tier.gold]\ntier = \"g-1\"\n[options.tier.free]\ntier = \"f-1\"\n",
        )
        .unwrap();
        validate_env(&m, &e).unwrap();
        let mut sel = Selections::new();
        sel.insert("tier".into(), "gold".into());
        let r = resolve_env(&m, &e, &sel, &SecretValues::new());
        assert_eq!(r.values["tier"], "g-1");
        assert_eq!(
            r.meta["tier"],
            VarMeta::SelectorMember {
                selector: "tier".into(),
                selected: "gold".into()
            }
        );
    }
}

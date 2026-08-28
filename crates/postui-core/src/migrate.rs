//! One-shot conversion of older variable files to the selector/option
//! format: stage-6 enumerated variables (`[<var>.options.*]`,
//! `[groups.<g>]` with `members`, per-env `[options.*]` overrides) and the
//! stage-7 linked-records spelling (`[groups.<g>]` with `fields`, per-env
//! `[entries.*]`) both land as `[selectors.<g>]` + `[options.<g>."<name>"]`.

use crate::varedit::{self, EditError};
use indexmap::IndexMap;
use toml_edit::{DocumentMut, Item};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct MigrationOutcome {
    /// New `variables.toml` text, `None` when it needs no change.
    pub variables: Option<String>,
    /// `(env name, new text)` — only environments whose text changed.
    pub envs: Vec<(String, String)>,
    /// `Some(text)` => write `environments/default.toml`.
    pub new_default_env: Option<String>,
    /// Human-readable, shown in the confirm modal.
    pub notes: Vec<String>,
}

fn parse_err(msg: impl Into<String>) -> EditError {
    EditError::Parse(msg.into())
}

fn top_level(doc: &str, what: &str) -> Result<IndexMap<String, toml::Value>, EditError> {
    toml::from_str(doc).map_err(|e| parse_err(format!("{what}: {e}")))
}

/// One legacy `[…options.<key>]` row: field values plus its description.
#[derive(Debug, Clone, Default)]
struct LegacyOption {
    description: Option<String>,
    /// For a variable's option this is `{<var>: value}`; for a group's it
    /// is the member -> value map as written.
    values: IndexMap<String, String>,
}

#[derive(Debug, Clone, Default)]
struct LegacyVar {
    description: Option<String>,
    options: IndexMap<String, LegacyOption>,
}

/// A `[groups.<g>]` declaration — stage-6 (`members`, inline `options`) or
/// stage-7 (`fields`) spelling; either way it becomes `[selectors.<g>]`.
#[derive(Debug, Clone, Default)]
struct LegacyGroup {
    fields: Vec<String>,
    options: IndexMap<String, LegacyOption>,
    /// Whether this group still uses stage-6 spelling (`members` /
    /// inline `options`) and so needs its keys reshaped, not just the
    /// container renamed.
    stage6: bool,
}

#[derive(Debug, Clone, Default)]
struct Legacy {
    vars: IndexMap<String, LegacyVar>,
    groups: IndexMap<String, LegacyGroup>,
}

/// One environment's legacy tables: stage-6 `[options.*]` overrides (name
/// -> key -> field -> value) and whether a stage-7 `[entries]` table is
/// present (its records are already per-env and only need the container
/// renamed).
#[derive(Debug, Clone, Default)]
struct LegacyEnv {
    stage6_options: IndexMap<String, IndexMap<String, IndexMap<String, String>>>,
    has_entries: bool,
}

/// One migrated record.
#[derive(Debug, Clone, Default)]
struct OptionData {
    description: Option<String>,
    values: IndexMap<String, String>,
}

/// selector -> option name -> option.
type SelectorOptions = IndexMap<String, IndexMap<String, OptionData>>;

fn parse_legacy_group(value: &toml::Value, name: &str) -> Result<LegacyGroup, EditError> {
    let table = value
        .as_table()
        .ok_or_else(|| parse_err(format!("[groups.{name}] must be a table")))?;
    if table.contains_key("members") && table.contains_key("fields") {
        return Err(parse_err(format!(
            "[groups.{name}] has both `members` and `fields`; keep only `fields`"
        )));
    }
    let stage6 = table.contains_key("members") || table.contains_key("options");
    let list = table
        .get("members")
        .or_else(|| table.get("fields"))
        .and_then(toml::Value::as_array)
        .ok_or_else(|| {
            parse_err(format!(
                "[groups.{name}] needs `fields` (stage 6: `members`) as an array of names"
            ))
        })?;
    let mut fields = Vec::new();
    for f in list {
        let f = f.as_str().ok_or_else(|| {
            parse_err(format!(
                "[groups.{name}] `fields` must be an array of names"
            ))
        })?;
        fields.push(f.to_string());
    }

    let mut options = IndexMap::new();
    if let Some(opts) = table.get("options") {
        let opts = opts
            .as_table()
            .ok_or_else(|| parse_err(format!("[groups.{name}.options] must be a table")))?;
        for (key, row) in opts {
            let row_path = format!("groups.{name}.options.{key}");
            let row = row
                .as_table()
                .ok_or_else(|| parse_err(format!("[{row_path}] must be a table")))?;
            let mut option = LegacyOption::default();
            for (field, v) in row {
                let v = v.as_str().ok_or_else(|| {
                    parse_err(format!("[{row_path}] \"{field}\" must be a string"))
                })?;
                if field == "description" {
                    option.description = Some(v.to_string());
                    continue;
                }
                if !fields.contains(field) {
                    return Err(parse_err(format!(
                        "[{row_path}] sets \"{field}\", which is not a member of group \"{name}\""
                    )));
                }
                option.values.insert(field.clone(), v.to_string());
            }
            options.insert(key.clone(), option);
        }
    }

    Ok(LegacyGroup {
        fields,
        options,
        stage6,
    })
}

fn parse_legacy_var(value: &toml::Value, name: &str) -> Result<LegacyVar, EditError> {
    let table = value
        .as_table()
        .ok_or_else(|| parse_err(format!("[{name}] must be a table")))?;
    let description = table
        .get("description")
        .and_then(toml::Value::as_str)
        .map(str::to_string);

    let mut options = IndexMap::new();
    if let Some(opts) = table.get("options") {
        if table.contains_key("default") {
            return Err(parse_err(format!(
                "[{name}] declares both `default` and `options`; a selector has no declaration default, so remove one before migrating"
            )));
        }
        let opts = opts
            .as_table()
            .ok_or_else(|| parse_err(format!("[{name}.options] must be a table")))?;
        for (key, row) in opts {
            let row_path = format!("{name}.options.{key}");
            let row = row
                .as_table()
                .ok_or_else(|| parse_err(format!("[{row_path}] must be a table")))?;
            let mut option = LegacyOption::default();
            let mut seen_value = false;
            for (field, v) in row {
                let v = v.as_str().ok_or_else(|| {
                    parse_err(format!("[{row_path}] \"{field}\" must be a string"))
                })?;
                match field.as_str() {
                    "description" => option.description = Some(v.to_string()),
                    "value" => {
                        seen_value = true;
                        option.values.insert(name.to_string(), v.to_string());
                    }
                    other => {
                        return Err(parse_err(format!(
                            "[{row_path}] has unknown field \"{other}\""
                        )));
                    }
                }
            }
            if !seen_value {
                return Err(parse_err(format!(
                    "[{row_path}] is missing required field \"value\""
                )));
            }
            options.insert(key.clone(), option);
        }
    }

    Ok(LegacyVar {
        description,
        options,
    })
}

fn parse_legacy_vars(vars_doc: &str) -> Result<Legacy, EditError> {
    let top = top_level(vars_doc, "variables.toml")?;
    let mut legacy = Legacy::default();
    for (name, value) in &top {
        if name == "selectors" {
            continue;
        }
        if name == "groups" {
            let groups = value
                .as_table()
                .ok_or_else(|| parse_err("[groups] must be a table"))?;
            for (group, gvalue) in groups {
                legacy
                    .groups
                    .insert(group.clone(), parse_legacy_group(gvalue, group)?);
            }
            continue;
        }
        legacy
            .vars
            .insert(name.clone(), parse_legacy_var(value, name)?);
    }
    Ok(legacy)
}

/// Reads one environment's legacy tables. An `[options.<name>]` table is
/// stage-6 legacy only when `<name>` is a declared plain variable or a
/// `[groups.*]` declaration — the shapes stage 6 could enumerate. Options
/// for a declared `[selectors.*]` name are new-format and left untouched;
/// options for a name declared nowhere are also left alone, so they
/// surface as the ordinary undeclared-selector validation error rather
/// than a migration prompt.
fn parse_legacy_env(env: &str, doc: &str, legacy_names: &[String]) -> Result<LegacyEnv, EditError> {
    let top = top_level(doc, &format!("environments/{env}.toml"))?;
    let mut out = LegacyEnv {
        has_entries: top.contains_key("entries"),
        ..LegacyEnv::default()
    };
    let Some(options) = top.get("options") else {
        return Ok(out);
    };
    let options = options.as_table().ok_or_else(|| {
        parse_err(format!(
            "environments/{env}.toml: [options] must be a table"
        ))
    })?;
    for (name, keys) in options {
        if !legacy_names.contains(name) {
            continue;
        }
        let keys = keys.as_table().ok_or_else(|| {
            parse_err(format!(
                "environments/{env}.toml: [options.{name}] must be a table"
            ))
        })?;
        let mut per_name = IndexMap::new();
        for (key, row) in keys {
            let row_path = format!("environments/{env}.toml: [options.{name}.{key}]");
            let row = row
                .as_table()
                .ok_or_else(|| parse_err(format!("{row_path} must be a table")))?;
            let mut fields = IndexMap::new();
            for (field, v) in row {
                let v = v
                    .as_str()
                    .ok_or_else(|| parse_err(format!("{row_path} \"{field}\" must be a string")))?;
                fields.insert(field.clone(), v.to_string());
            }
            per_name.insert(key.clone(), fields);
        }
        out.stage6_options.insert(name.clone(), per_name);
    }
    Ok(out)
}

/// Renames `[groups]` to `[selectors]` in place (decor, order and comments
/// preserved) and reshapes each declaration: stage-6 `members` becomes
/// `fields`, and any inline `options` table is dropped (its records were
/// already captured for the environment files).
fn rewrite_groups_as_selectors(doc: &str) -> Result<String, EditError> {
    let mut doc = doc
        .parse::<DocumentMut>()
        .map_err(|e| parse_err(e.to_string()))?;
    let root = doc.as_table_mut();
    if root.contains_key("groups") {
        if root.contains_key("selectors") {
            return Err(parse_err(
                "variables.toml has both [groups.*] and [selectors.*]; move the [groups.*] declarations under [selectors.*] by hand",
            ));
        }
        varedit::rename_key(root, "groups", "selectors");
    }
    if let Some(selectors) = root.get_mut("selectors").and_then(Item::as_table_mut) {
        let names: Vec<String> = selectors.iter().map(|(k, _)| k.to_string()).collect();
        for name in names {
            if let Some(table) = selectors.get_mut(&name).and_then(Item::as_table_mut) {
                if table.contains_key("members") {
                    varedit::rename_key(table, "members", "fields");
                }
                table.remove("options");
            }
        }
    }
    Ok(doc.to_string())
}

/// Renames a stage-7 `[entries]` table to `[options]` (decor preserved) and
/// removes each stage-6 legacy name from any existing `[options]` table.
/// When both containers exist, the entries subtrees are moved into the
/// options table one by one instead of renamed wholesale.
fn rewrite_env_containers(doc: &str, legacy_names: &[String]) -> Result<String, EditError> {
    let mut doc = doc
        .parse::<DocumentMut>()
        .map_err(|e| parse_err(e.to_string()))?;
    let root = doc.as_table_mut();

    if let Some(options) = root.get_mut("options").and_then(Item::as_table_mut) {
        for name in legacy_names {
            options.remove(name);
        }
        let now_empty = options.is_empty();
        if now_empty {
            root.remove("options");
        }
    }

    if root.contains_key("entries") {
        if let Some(existing) = root.get("options").and_then(Item::as_table) {
            let clash: Vec<String> = existing.iter().map(|(k, _)| k.to_string()).collect();
            let entries = root
                .get_mut("entries")
                .and_then(Item::as_table_mut)
                .ok_or_else(|| parse_err("[entries] must be a table"))?;
            let moved: Vec<(String, Item)> = {
                let names: Vec<String> = entries.iter().map(|(k, _)| k.to_string()).collect();
                for name in &names {
                    if clash.contains(name) {
                        return Err(parse_err(format!(
                            "[entries.{name}] and [options.{name}] both exist; merge them by hand before migrating"
                        )));
                    }
                }
                names
                    .into_iter()
                    .filter_map(|name| entries.remove_entry(&name).map(|(k, v)| (k.to_string(), v)))
                    .collect()
            };
            root.remove("entries");
            let options = root
                .get_mut("options")
                .and_then(Item::as_table_mut)
                .expect("checked present above");
            for (name, item) in moved {
                options.insert(&name, item);
            }
        } else {
            varedit::rename_key(root, "entries", "options");
        }
    }

    Ok(doc.to_string())
}

fn write_options(doc: &str, options: &SelectorOptions) -> Result<String, EditError> {
    let mut text = doc.to_string();
    for (selector, per_selector) in options {
        for (option, data) in per_selector {
            text = varedit::upsert_option(
                &text,
                selector,
                option,
                data.description.as_deref(),
                &data.values,
            )?;
        }
    }
    Ok(text)
}

/// True if `variables.toml` uses a legacy syntax (`[<var>.options]`,
/// `[groups.*]` in either spelling), any environment document has a
/// stage-7 `[entries]` table, or an environment `[options.<name>]` table
/// names a declared plain variable (the stage-6 per-env enumeration
/// shape). An `[options.<name>]` table naming nothing declared is NOT a
/// migration trigger — it's the ordinary undeclared-selector validation
/// error.
pub fn needs_migration(vars_doc: &str, env_docs: &[(String, String)]) -> bool {
    let mut plain_vars: Vec<String> = Vec::new();
    if let Ok(top) = toml::from_str::<IndexMap<String, toml::Value>>(vars_doc) {
        for (name, value) in &top {
            if name == "groups" {
                return true;
            }
            if name == "selectors" {
                continue;
            }
            if value.as_table().is_some_and(|t| t.contains_key("options")) {
                return true;
            }
            plain_vars.push(name.clone());
        }
    }
    env_docs.iter().any(|(_, doc)| {
        toml::from_str::<IndexMap<String, toml::Value>>(doc).is_ok_and(|top| {
            if top.contains_key("entries") {
                return true;
            }
            top.get("options")
                .and_then(toml::Value::as_table)
                .is_some_and(|options| options.keys().any(|name| plain_vars.contains(name)))
        })
    })
}

/// Converts a legacy project to the selector/option format. Nothing is
/// written here: the caller decides whether to apply the returned texts.
///
/// Enumerated variables become one-field selectors of the same name;
/// `[groups.*]` declarations become `[selectors.*]` in place (stage-6
/// `members` becomes `fields`); every declaration-level option value
/// becomes an option in *every* environment, with that environment's keyed
/// overrides merged on top per field; stage-7 `[entries.*]` tables are
/// renamed to `[options.*]` where they sit. Anything the legacy shapes
/// could hold that the new format can't express is reported as
/// [`EditError::Parse`] naming the offending path, so no construct is
/// silently lost.
pub fn migrate(
    vars_doc: &str,
    env_docs: &[(String, String)],
) -> Result<MigrationOutcome, EditError> {
    let legacy = parse_legacy_vars(vars_doc)?;
    let legacy_names: Vec<String> = legacy
        .vars
        .keys()
        .chain(legacy.groups.keys())
        .cloned()
        .collect();
    let mut legacy_envs: Vec<(String, LegacyEnv)> = Vec::new();
    for (name, doc) in env_docs {
        legacy_envs.push((name.clone(), parse_legacy_env(name, doc, &legacy_names)?));
    }

    // Variables that become one-field selectors: those with declaration
    // options, plus any plain variable an environment enumerates.
    let mut converted: Vec<String> = legacy
        .vars
        .iter()
        .filter(|(_, v)| !v.options.is_empty())
        .map(|(name, _)| name.clone())
        .collect();
    for (_env, legacy_env) in &legacy_envs {
        for name in legacy_env.stage6_options.keys() {
            if legacy.groups.contains_key(name) || converted.contains(name) {
                continue;
            }
            converted.push(name.clone());
        }
    }

    let mut notes = Vec::new();

    // Every selector's field list, keyed by name.
    let mut selector_fields: IndexMap<String, Vec<String>> = IndexMap::new();
    for name in &converted {
        selector_fields.insert(name.clone(), vec![name.clone()]);
    }
    for (name, group) in &legacy.groups {
        selector_fields.insert(name.clone(), group.fields.clone());
    }

    // Declaration-level option values, as per-env records.
    let mut decl_options: SelectorOptions = IndexMap::new();
    for name in &converted {
        let var = &legacy.vars[name];
        let mut per_selector = IndexMap::new();
        for (key, option) in &var.options {
            per_selector.insert(
                key.clone(),
                OptionData {
                    description: option.description.clone(),
                    values: option.values.clone(),
                },
            );
        }
        decl_options.insert(name.clone(), per_selector);
        notes.push(format!(
            "variable \"{name}\" became a one-field selector with an option per value"
        ));
    }
    for (name, group) in &legacy.groups {
        if !group.stage6 {
            notes.push(format!("group \"{name}\" became selector \"{name}\""));
        }
        let mut per_selector = IndexMap::new();
        for (key, option) in &group.options {
            let mut values = IndexMap::new();
            for field in &group.fields {
                let value = match option.values.get(field) {
                    Some(v) => v.clone(),
                    None => {
                        notes.push(format!(
                            "option \"{key}\" of selector \"{name}\" had no value for \"{field}\"; filled in as empty"
                        ));
                        String::new()
                    }
                };
                values.insert(field.clone(), value);
            }
            per_selector.insert(
                key.clone(),
                OptionData {
                    description: option.description.clone(),
                    values,
                },
            );
        }
        if !per_selector.is_empty() || !decl_options.contains_key(name) {
            decl_options.insert(name.clone(), per_selector);
        }
    }

    // ---- variables.toml ----
    let rewrite_vars = !converted.is_empty() || !legacy.groups.is_empty();
    let new_variables = if rewrite_vars {
        let mut text = vars_doc.to_string();
        for name in &converted {
            text = varedit::delete_var(&text, name)?;
        }
        text = rewrite_groups_as_selectors(&text)?;
        for name in &converted {
            text = varedit::upsert_selector(
                &text,
                name,
                legacy.vars[name].description.as_deref(),
                std::slice::from_ref(name),
            )?;
        }
        (text != vars_doc).then_some(text)
    } else {
        None
    };

    // ---- environments/<env>.toml ----
    let mut envs = Vec::new();
    for ((env, doc), (_, legacy_env)) in env_docs.iter().zip(&legacy_envs) {
        let mut options = decl_options.clone();
        for (name, keys) in &legacy_env.stage6_options {
            let fields = selector_fields
                .get(name)
                .expect("every legacy option owner is a selector by now")
                .clone();
            let is_converted_var = converted.contains(name);
            let per_selector = options.entry(name.clone()).or_default();
            for (key, row) in keys {
                let slot = per_selector
                    .entry(key.clone())
                    .or_insert_with(|| OptionData {
                        description: None,
                        values: fields.iter().map(|f| (f.clone(), String::new())).collect(),
                    });
                for (field, v) in row {
                    if field == "description" {
                        slot.description = Some(v.clone());
                        continue;
                    }
                    if is_converted_var {
                        if field != "value" {
                            return Err(parse_err(format!(
                                "environments/{env}.toml: [options.{name}.{key}] has unknown field \"{field}\""
                            )));
                        }
                        slot.values.insert(name.clone(), v.clone());
                    } else {
                        if !fields.contains(field) {
                            return Err(parse_err(format!(
                                "environments/{env}.toml: [options.{name}.{key}.{field}] is not a member of group \"{name}\""
                            )));
                        }
                        slot.values.insert(field.clone(), v.clone());
                    }
                }
            }
        }

        let nothing_to_do = legacy_env.stage6_options.is_empty()
            && !legacy_env.has_entries
            && options.values().all(|per_selector| per_selector.is_empty());
        if nothing_to_do {
            continue;
        }
        let legacy_names: Vec<String> = legacy_env.stage6_options.keys().cloned().collect();
        let stripped = rewrite_env_containers(doc, &legacy_names)?;
        // Records already renamed in place (stage-7 entries) don't need
        // rewriting; only the in-memory conversions are written out.
        let text = write_options(&stripped, &options)?;
        if text != *doc {
            envs.push((env.clone(), text));
        }
    }

    // ---- environments/default.toml ----
    let mut new_default_env = None;
    if env_docs.is_empty()
        && decl_options
            .values()
            .any(|per_selector| !per_selector.is_empty())
    {
        new_default_env = Some(write_options("", &decl_options)?);
        notes.push("created environments/default.toml to hold the migrated options".to_string());
    }

    Ok(MigrationOutcome {
        variables: new_variables,
        envs,
        new_default_env,
        notes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrates_enumerated_var_group_and_env_overrides() {
        let vars = r#"
[tier]
description = "pricing tier"
[tier.options.gold]
description = "the good one"
value = "g-1"
[tier.options.free]
value = "f-1"

[groups.user]
members = ["user_id", "customer_id"]
[groups.user.options.alice]
user_id = "1001"
customer_id = "c-77"
"#;
        let qa_env = (
            "qa".to_string(),
            "[options.tier.gold]\nvalue = \"g-qa\"\n".to_string(),
        );
        let out = migrate(vars, &[qa_env]).unwrap();
        let new_vars = out.variables.unwrap();
        let m = crate::varmodel::parse_variables(&new_vars).unwrap();
        assert_eq!(m.selectors["tier"].fields, ["tier"]);
        assert_eq!(m.selectors["user"].fields, ["user_id", "customer_id"]);
        assert!(m.vars.is_empty());
        assert_eq!(
            m.selectors["tier"].description.as_deref(),
            Some("pricing tier"),
            "the variable's description moves to the selector"
        );
        let (env_name, qa_text) = &out.envs[0];
        assert_eq!(env_name, "qa");
        let e = crate::varmodel::parse_environment(qa_text).unwrap();
        assert_eq!(e.options["tier"]["gold"].values["tier"], "g-qa"); // env override won
        assert_eq!(e.options["tier"]["free"].values["tier"], "f-1");
        assert_eq!(
            e.options["tier"]["gold"].description.as_deref(),
            Some("the good one")
        );
        assert_eq!(e.options["user"]["alice"].values["customer_id"], "c-77");
        crate::varmodel::validate_env(&m, &e).unwrap();
        assert!(
            out.notes.iter().any(|n| n.contains("tier")),
            "the converted variable is reported: {:?}",
            out.notes
        );
        assert!(out.new_default_env.is_none());
        assert!(needs_migration(vars, &[]));
    }

    #[test]
    fn migrates_stage7_groups_and_entries_to_selectors_and_options() {
        let vars = r#"# variables.toml

[base_url]
default = "http://localhost"

# the linked pair
[groups.user]
description = "user with customer"
fields = ["user_id", "customer_id"]
"#;
        let env = r#"# environments/qa.toml

base_url = "https://qa.example.com"

[entries.user."user 1"]
user_id = "1001"
customer_id = "c-77"
"#;
        assert!(needs_migration(
            vars,
            &[("qa".to_string(), env.to_string())]
        ));
        let out = migrate(vars, &[("qa".to_string(), env.to_string())]).unwrap();
        let new_vars = out.variables.unwrap();
        assert!(
            new_vars.contains("# the linked pair\n[selectors.user]"),
            "the declaration renames in place, comment kept: {new_vars}"
        );
        assert!(!new_vars.contains("[groups."), "{new_vars}");
        let m = crate::varmodel::parse_variables(&new_vars).unwrap();
        assert_eq!(m.selectors["user"].fields, ["user_id", "customer_id"]);
        assert_eq!(
            m.selectors["user"].description.as_deref(),
            Some("user with customer")
        );
        let (_, qa_text) = &out.envs[0];
        assert!(
            qa_text.contains("[options.user.\"user 1\"]"),
            "entries rename to options in place: {qa_text}"
        );
        assert!(!qa_text.contains("[entries."), "{qa_text}");
        assert!(qa_text.starts_with("# environments/qa.toml\n"), "{qa_text}");
        let e = crate::varmodel::parse_environment(qa_text).unwrap();
        assert_eq!(e.options["user"]["user 1"].values["user_id"], "1001");
        crate::varmodel::validate_env(&m, &e).unwrap();
    }

    #[test]
    fn new_format_needs_no_migration_and_changes_nothing() {
        let vars = "[base_url]\ndefault = \"x\"\n\n[selectors.user]\nfields = [\"user_id\"]\n";
        let envs = vec![(
            "qa".to_string(),
            "base_url = \"y\"\n\n[options.user.\"user 1\"]\nuser_id = \"1\"\n".to_string(),
        )];
        assert!(!needs_migration(vars, &envs));
        let out = migrate(vars, &envs).unwrap();
        assert_eq!(out, MigrationOutcome::default());
    }

    #[test]
    fn stage7_group_without_entries_still_renames_the_declaration() {
        let vars = "[groups.user]\nfields = [\"user_id\"]\n";
        assert!(needs_migration(vars, &[]));
        let out = migrate(vars, &[]).unwrap();
        assert!(out.new_default_env.is_none());
        let m = crate::varmodel::parse_variables(&out.variables.unwrap()).unwrap();
        assert_eq!(m.selectors["user"].fields, ["user_id"]);
    }

    #[test]
    fn a_project_with_no_environments_gets_a_default_env_holding_the_options() {
        let vars = r#"[tier]
[tier.options.gold]
value = "g-1"
"#;
        let out = migrate(vars, &[]).unwrap();
        assert!(out.envs.is_empty());
        let text = out.new_default_env.expect("options need somewhere to live");
        let e = crate::varmodel::parse_environment(&text).unwrap();
        assert_eq!(e.options["tier"]["gold"].values["tier"], "g-1");
        let m = crate::varmodel::parse_variables(&out.variables.unwrap()).unwrap();
        crate::varmodel::validate_env(&m, &e).unwrap();
        assert!(
            out.notes
                .iter()
                .any(|n| n.contains("environments/default.toml")),
            "{:?}",
            out.notes
        );
    }

    #[test]
    fn plain_vars_and_secrets_pass_through_untouched() {
        let vars = r#"# variables.toml

[base_url]
description = "API root"
default = "http://localhost:8080"

[api_key]
secret = true

[tier]
[tier.options.gold]
value = "g-1"
"#;
        let out = migrate(vars, &[]).unwrap();
        let new_vars = out.variables.unwrap();
        assert!(new_vars.starts_with("# variables.toml\n"), "{new_vars}");
        assert!(
            new_vars.contains(
                "[base_url]\ndescription = \"API root\"\ndefault = \"http://localhost:8080\""
            ),
            "{new_vars}"
        );
        assert!(new_vars.contains("[api_key]\nsecret = true"), "{new_vars}");
        let m = crate::varmodel::parse_variables(&new_vars).unwrap();
        assert_eq!(m.vars.keys().collect::<Vec<_>>(), ["base_url", "api_key"]);
    }

    #[test]
    fn comments_elsewhere_in_env_docs_survive() {
        let vars = "[tier]\n[tier.options.gold]\nvalue = \"g-1\"\n";
        let env = r#"# environments/qa.toml

# the qa API root
base_url = "https://qa.example.com"

[options.tier.gold]
value = "g-qa"
"#;
        let out = migrate(vars, &[("qa".to_string(), env.to_string())]).unwrap();
        let (_, text) = &out.envs[0];
        assert!(text.starts_with("# environments/qa.toml\n"), "{text}");
        assert!(text.contains("# the qa API root\nbase_url ="), "{text}");
        let e = crate::varmodel::parse_environment(text).unwrap();
        assert_eq!(e.values["base_url"], "https://qa.example.com");
        assert_eq!(e.options["tier"]["gold"].values["tier"], "g-qa");
    }

    #[test]
    fn group_with_both_members_and_fields_is_an_error() {
        let vars = "[groups.user]\nmembers = [\"a\"]\nfields = [\"a\"]\n";
        let err = migrate(vars, &[]).unwrap_err();
        assert!(
            matches!(&err, EditError::Parse(m) if m.contains("groups.user")),
            "{err:?}"
        );
    }

    #[test]
    fn env_only_option_keys_become_options_of_that_env_only() {
        let vars = "[tier]\n[tier.options.gold]\nvalue = \"g-1\"\n";
        let envs = vec![
            (
                "qa".to_string(),
                "[options.tier.qa-only]\ndescription = \"just here\"\nvalue = \"q-1\"\n"
                    .to_string(),
            ),
            ("prod".to_string(), "base_url = \"p\"\n".to_string()),
        ];
        let out = migrate(vars, &envs).unwrap();
        let qa = crate::varmodel::parse_environment(&out.envs[0].1).unwrap();
        assert_eq!(qa.options["tier"]["qa-only"].values["tier"], "q-1");
        assert_eq!(
            qa.options["tier"]["qa-only"].description.as_deref(),
            Some("just here")
        );
        assert_eq!(qa.options["tier"]["gold"].values["tier"], "g-1");
        let prod = crate::varmodel::parse_environment(&out.envs[1].1).unwrap();
        assert!(!prod.options["tier"].contains_key("qa-only"));
        assert_eq!(prod.options["tier"]["gold"].values["tier"], "g-1");
    }

    #[test]
    fn a_plain_variable_enumerated_only_in_an_env_also_becomes_a_selector() {
        let vars = "[shard]\ndescription = \"which shard\"\n";
        let envs = vec![(
            "qa".to_string(),
            "[options.shard.east]\nvalue = \"e-1\"\n".to_string(),
        )];
        assert!(needs_migration(vars, &envs));
        let out = migrate(vars, &envs).unwrap();
        let m = crate::varmodel::parse_variables(&out.variables.unwrap()).unwrap();
        assert_eq!(m.selectors["shard"].fields, ["shard"]);
        assert!(m.vars.is_empty());
        let e = crate::varmodel::parse_environment(&out.envs[0].1).unwrap();
        assert_eq!(e.options["shard"]["east"].values["shard"], "e-1");
        crate::varmodel::validate_env(&m, &e).unwrap();
    }

    #[test]
    fn new_format_env_options_are_left_alone_while_legacy_ones_convert() {
        // `[options.region.*]` is new-format (region is a declared
        // selector); `[options.shard.*]` is stage-6 legacy (shard is a
        // plain variable). Only the legacy one is rewritten.
        let vars = r#"[shard]

[selectors.region]
fields = ["region"]
"#;
        let env = r#"[options.region.east]
region = "r-east"

[options.shard.a]
value = "s-a"
"#;
        let envs = vec![("qa".to_string(), env.to_string())];
        assert!(needs_migration(vars, &envs));
        let out = migrate(vars, &envs).unwrap();
        let e = crate::varmodel::parse_environment(&out.envs[0].1).unwrap();
        assert_eq!(e.options["region"]["east"].values["region"], "r-east");
        assert_eq!(e.options["shard"]["a"].values["shard"], "s-a");
        let m = crate::varmodel::parse_variables(&out.variables.unwrap()).unwrap();
        assert_eq!(m.selectors["shard"].fields, ["shard"]);
        assert_eq!(m.selectors["region"].fields, ["region"]);
    }

    #[test]
    fn env_options_for_an_undeclared_name_is_not_a_migration_matter() {
        // `[options.ghost.*]` with ghost declared nowhere is the ordinary
        // undeclared-selector validation error, not a legacy shape — a
        // migration prompt here would blank the whole model over a typo.
        let vars = "[base_url]\ndefault = \"x\"\n";
        let envs = vec![(
            "qa".to_string(),
            "[options.ghost.a]\nvalue = \"1\"\n".to_string(),
        )];
        assert!(!needs_migration(vars, &envs));
        let out = migrate(vars, &envs).unwrap();
        assert_eq!(out, MigrationOutcome::default());
    }

    #[test]
    fn env_group_option_naming_a_non_member_is_an_error() {
        let vars = "[groups.user]\nmembers = [\"user_id\"]\n";
        let envs = vec![(
            "qa".to_string(),
            "[options.user.alice]\nghost = \"1\"\n".to_string(),
        )];
        let err = migrate(vars, &envs).unwrap_err();
        assert!(
            matches!(&err, EditError::Parse(m) if m.contains("options.user.alice.ghost")),
            "{err:?}"
        );
    }

    #[test]
    fn enumerated_variable_with_a_default_is_an_error_rather_than_a_silent_drop() {
        let vars = "[tier]\ndefault = \"d\"\n[tier.options.gold]\nvalue = \"g-1\"\n";
        let err = migrate(vars, &[]).unwrap_err();
        assert!(
            matches!(&err, EditError::Parse(m) if m.contains("tier") && m.contains("default")),
            "{err:?}"
        );
    }

    #[test]
    fn a_group_option_missing_a_member_is_filled_in_and_noted() {
        // Stage 6 let a group option omit a member (it simply resolved to
        // nothing); the new format requires every option to supply every
        // field, so the gap is filled with an empty value and called out
        // rather than silently invented.
        let vars = r#"[groups.user]
members = ["user_id", "customer_id"]
[groups.user.options.alice]
user_id = "1001"
"#;
        let out = migrate(vars, &[("qa".to_string(), String::new())]).unwrap();
        let m = crate::varmodel::parse_variables(&out.variables.unwrap()).unwrap();
        let e = crate::varmodel::parse_environment(&out.envs[0].1).unwrap();
        assert_eq!(e.options["user"]["alice"].values["customer_id"], "");
        crate::varmodel::validate_env(&m, &e).unwrap();
        assert!(
            out.notes
                .iter()
                .any(|n| n.contains("customer_id") && n.contains("alice")),
            "{:?}",
            out.notes
        );
    }

    #[test]
    fn needs_migration_spots_each_legacy_shape() {
        assert!(needs_migration(
            "[tier]\n[tier.options.gold]\nvalue = \"g\"\n",
            &[]
        ));
        assert!(needs_migration("[groups.user]\nmembers = [\"a\"]\n", &[]));
        assert!(needs_migration("[groups.user]\nfields = [\"a\"]\n", &[]));
        assert!(needs_migration(
            "",
            &[(
                "qa".to_string(),
                "[entries.user.\"u\"]\nuser_id = \"1\"\n".to_string()
            )]
        ));
        assert!(needs_migration(
            "[base_url]\ndefault = \"x\"\n",
            &[(
                "qa".to_string(),
                "[options.base_url.a]\nvalue = \"1\"\n".to_string()
            )]
        ));
        assert!(!needs_migration("", &[("qa".to_string(), String::new())]));
        assert!(!needs_migration(
            "[selectors.user]\nfields = [\"user_id\"]\n",
            &[(
                "qa".to_string(),
                "[options.user.\"u\"]\nuser_id = \"1\"\n".to_string()
            )]
        ));
    }
}

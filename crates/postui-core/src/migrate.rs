//! One-shot conversion of stage-6 variable files to the stage-7
//! linked-records format (spec §3.3).

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

#[derive(Debug, Clone, Default)]
struct LegacyGroup {
    fields: Vec<String>,
    options: IndexMap<String, LegacyOption>,
    /// Whether this group still uses stage-6 spelling and so has to be
    /// rewritten at all.
    legacy: bool,
}

#[derive(Debug, Clone, Default)]
struct Legacy {
    vars: IndexMap<String, LegacyVar>,
    groups: IndexMap<String, LegacyGroup>,
}

/// One environment's `[options]` table: name -> key -> field -> value.
type EnvOptions = IndexMap<String, IndexMap<String, IndexMap<String, String>>>;

/// One migrated entry.
#[derive(Debug, Clone, Default)]
struct EntryData {
    description: Option<String>,
    values: IndexMap<String, String>,
}

/// group -> entry name -> entry.
type GroupEntries = IndexMap<String, IndexMap<String, EntryData>>;

fn parse_legacy_group(value: &toml::Value, name: &str) -> Result<LegacyGroup, EditError> {
    let table = value
        .as_table()
        .ok_or_else(|| parse_err(format!("[groups.{name}] must be a table")))?;
    if table.contains_key("members") && table.contains_key("fields") {
        return Err(parse_err(format!(
            "[groups.{name}] has both `members` and `fields`; keep only `fields`"
        )));
    }
    let legacy = table.contains_key("members") || table.contains_key("options");
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
        legacy,
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
                "[{name}] declares both `default` and `options`; a group has no declaration default, so remove one before migrating"
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

fn parse_env_options(env: &str, doc: &str) -> Result<EnvOptions, EditError> {
    let top = top_level(doc, &format!("environments/{env}.toml"))?;
    let Some(options) = top.get("options") else {
        return Ok(EnvOptions::new());
    };
    let options = options.as_table().ok_or_else(|| {
        parse_err(format!(
            "environments/{env}.toml: [options] must be a table"
        ))
    })?;
    let mut out = EnvOptions::new();
    for (name, keys) in options {
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
        out.insert(name.clone(), per_name);
    }
    Ok(out)
}

/// Drops the stage-6 `members`/`options` keys from every `[groups.*]`
/// table, leaving the rest of the document (comments, order, blank lines)
/// exactly as written.
fn strip_group_legacy_keys(doc: &str) -> Result<String, EditError> {
    let mut doc = doc
        .parse::<DocumentMut>()
        .map_err(|e| parse_err(e.to_string()))?;
    if let Some(groups) = doc
        .as_table_mut()
        .get_mut("groups")
        .and_then(Item::as_table_mut)
    {
        let names: Vec<String> = groups.iter().map(|(k, _)| k.to_string()).collect();
        for name in names {
            if let Some(table) = groups.get_mut(&name).and_then(Item::as_table_mut) {
                table.remove("members");
                table.remove("options");
            }
        }
    }
    Ok(doc.to_string())
}

/// Drops the whole `[options]` table from an environment document.
fn strip_env_options(doc: &str) -> Result<String, EditError> {
    let mut doc = doc
        .parse::<DocumentMut>()
        .map_err(|e| parse_err(e.to_string()))?;
    doc.as_table_mut().remove("options");
    Ok(doc.to_string())
}

fn write_entries(doc: &str, entries: &GroupEntries) -> Result<String, EditError> {
    let mut text = doc.to_string();
    for (group, per_group) in entries {
        for (entry, data) in per_group {
            text = varedit::upsert_entry(
                &text,
                group,
                entry,
                data.description.as_deref(),
                &data.values,
            )?;
        }
    }
    Ok(text)
}

/// True if `variables.toml` uses stage-6 syntax (`[<var>.options]`,
/// `[groups.<g>]` with `members`, `[groups.<g>.options]`), or any
/// environment document has a top-level `[options]` table.
pub fn needs_migration(vars_doc: &str, env_docs: &[(String, String)]) -> bool {
    if let Ok(top) = toml::from_str::<IndexMap<String, toml::Value>>(vars_doc) {
        for (name, value) in &top {
            if name == "groups" {
                let has_legacy_group = value.as_table().is_some_and(|groups| {
                    groups.values().any(|g| {
                        g.as_table()
                            .is_some_and(|t| t.contains_key("members") || t.contains_key("options"))
                    })
                });
                if has_legacy_group {
                    return true;
                }
            } else if value.as_table().is_some_and(|t| t.contains_key("options")) {
                return true;
            }
        }
    }
    env_docs.iter().any(|(_, doc)| {
        toml::from_str::<IndexMap<String, toml::Value>>(doc)
            .is_ok_and(|top| top.contains_key("options"))
    })
}

/// Converts a stage-6 project to the stage-7 format. Nothing is written
/// here: the caller decides whether to apply the returned texts.
///
/// Enumerated variables become one-field groups of the same name; group
/// `members` become `fields`; every declaration-level option value becomes
/// an entry in *every* environment, with that environment's keyed
/// overrides merged on top per field. Anything the stage-6 shapes could
/// hold that the new format can't express is reported as
/// [`EditError::Parse`] naming the offending path, so no construct is
/// silently lost.
pub fn migrate(
    vars_doc: &str,
    env_docs: &[(String, String)],
) -> Result<MigrationOutcome, EditError> {
    let legacy = parse_legacy_vars(vars_doc)?;
    let mut env_options: Vec<(String, EnvOptions)> = Vec::new();
    for (name, doc) in env_docs {
        env_options.push((name.clone(), parse_env_options(name, doc)?));
    }

    // Variables that become one-field groups: those with declaration
    // options, plus any plain variable an environment enumerates.
    let mut converted: Vec<String> = legacy
        .vars
        .iter()
        .filter(|(_, v)| !v.options.is_empty())
        .map(|(name, _)| name.clone())
        .collect();
    for (env, options) in &env_options {
        for name in options.keys() {
            if legacy.groups.contains_key(name) || converted.contains(name) {
                continue;
            }
            if !legacy.vars.contains_key(name) {
                return Err(parse_err(format!(
                    "environments/{env}.toml: [options.{name}] does not match a declared variable or group"
                )));
            }
            converted.push(name.clone());
        }
    }

    let mut notes = Vec::new();

    // Every group's field list, keyed by group name.
    let mut group_fields: IndexMap<String, Vec<String>> = IndexMap::new();
    for name in &converted {
        group_fields.insert(name.clone(), vec![name.clone()]);
    }
    for (name, group) in &legacy.groups {
        group_fields.insert(name.clone(), group.fields.clone());
    }

    // Declaration-level option values, as entries.
    let mut decl_entries: GroupEntries = IndexMap::new();
    for name in &converted {
        let var = &legacy.vars[name];
        let mut per_group = IndexMap::new();
        for (key, option) in &var.options {
            per_group.insert(
                key.clone(),
                EntryData {
                    description: option.description.clone(),
                    values: option.values.clone(),
                },
            );
        }
        decl_entries.insert(name.clone(), per_group);
        notes.push(format!(
            "variable \"{name}\" became a one-field group with an entry per option"
        ));
    }
    for (name, group) in &legacy.groups {
        let mut per_group = IndexMap::new();
        for (key, option) in &group.options {
            let mut values = IndexMap::new();
            for field in &group.fields {
                let value = match option.values.get(field) {
                    Some(v) => v.clone(),
                    None => {
                        notes.push(format!(
                            "entry \"{key}\" of group \"{name}\" had no value for \"{field}\"; filled in as empty"
                        ));
                        String::new()
                    }
                };
                values.insert(field.clone(), value);
            }
            per_group.insert(
                key.clone(),
                EntryData {
                    description: option.description.clone(),
                    values,
                },
            );
        }
        decl_entries.insert(name.clone(), per_group);
    }

    // ---- variables.toml ----
    let rewrite_vars = !converted.is_empty() || legacy.groups.values().any(|g| g.legacy);
    let new_variables = if rewrite_vars {
        let mut text = vars_doc.to_string();
        for name in &converted {
            text = varedit::delete_var(&text, name)?;
        }
        text = strip_group_legacy_keys(&text)?;
        for (name, group) in &legacy.groups {
            if group.legacy {
                text = varedit::upsert_group(&text, name, None, &group.fields)?;
            }
        }
        for name in &converted {
            text = varedit::upsert_group(
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
    for ((env, doc), (_, options)) in env_docs.iter().zip(&env_options) {
        let mut entries = decl_entries.clone();
        for (name, keys) in options {
            let fields = group_fields
                .get(name)
                .expect("every option owner is a group by now")
                .clone();
            let is_converted_var = converted.contains(name);
            let per_group = entries.entry(name.clone()).or_default();
            for (key, row) in keys {
                let slot = per_group.entry(key.clone()).or_insert_with(|| EntryData {
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

        let nothing_to_do =
            options.is_empty() && entries.values().all(|per_group| per_group.is_empty());
        if nothing_to_do {
            continue;
        }
        let text = write_entries(&strip_env_options(doc)?, &entries)?;
        if text != *doc {
            envs.push((env.clone(), text));
        }
    }

    // ---- environments/default.toml ----
    let mut new_default_env = None;
    if env_docs.is_empty() && decl_entries.values().any(|per_group| !per_group.is_empty()) {
        new_default_env = Some(write_entries("", &decl_entries)?);
        notes.push("created environments/default.toml to hold the migrated entries".to_string());
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
        assert_eq!(m.groups["tier"].fields, ["tier"]);
        assert_eq!(m.groups["user"].fields, ["user_id", "customer_id"]);
        assert!(m.vars.is_empty());
        assert_eq!(
            m.groups["tier"].description.as_deref(),
            Some("pricing tier"),
            "the variable's description moves to the group"
        );
        let (env_name, qa_text) = &out.envs[0];
        assert_eq!(env_name, "qa");
        let e = crate::varmodel::parse_environment(qa_text).unwrap();
        assert_eq!(e.entries["tier"]["gold"].values["tier"], "g-qa"); // env override won
        assert_eq!(e.entries["tier"]["free"].values["tier"], "f-1");
        assert_eq!(
            e.entries["tier"]["gold"].description.as_deref(),
            Some("the good one")
        );
        assert_eq!(e.entries["user"]["alice"].values["customer_id"], "c-77");
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
    fn a_project_with_no_environments_gets_a_default_env_holding_the_entries() {
        let vars = r#"[tier]
[tier.options.gold]
value = "g-1"
"#;
        let out = migrate(vars, &[]).unwrap();
        assert!(out.envs.is_empty());
        let text = out.new_default_env.expect("entries need somewhere to live");
        let e = crate::varmodel::parse_environment(&text).unwrap();
        assert_eq!(e.entries["tier"]["gold"].values["tier"], "g-1");
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
    fn no_entries_means_no_default_env_is_created() {
        let vars = "[groups.user]\nmembers = [\"user_id\"]\n";
        let out = migrate(vars, &[]).unwrap();
        assert!(out.new_default_env.is_none());
        let m = crate::varmodel::parse_variables(&out.variables.unwrap()).unwrap();
        assert_eq!(m.groups["user"].fields, ["user_id"]);
    }

    #[test]
    fn already_new_format_needs_no_migration_and_changes_nothing() {
        let vars = "[base_url]\ndefault = \"x\"\n\n[groups.user]\nfields = [\"user_id\"]\n";
        let envs = vec![(
            "qa".to_string(),
            "base_url = \"y\"\n\n[entries.user.\"user 1\"]\nuser_id = \"1\"\n".to_string(),
        )];
        assert!(!needs_migration(vars, &envs));
        let out = migrate(vars, &envs).unwrap();
        assert_eq!(out, MigrationOutcome::default());
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
        assert!(!text.contains("[options."), "{text}");
        let e = crate::varmodel::parse_environment(text).unwrap();
        assert_eq!(e.values["base_url"], "https://qa.example.com");
        assert_eq!(e.entries["tier"]["gold"].values["tier"], "g-qa");
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
    fn env_only_option_keys_become_entries_of_that_env_only() {
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
        assert_eq!(qa.entries["tier"]["qa-only"].values["tier"], "q-1");
        assert_eq!(
            qa.entries["tier"]["qa-only"].description.as_deref(),
            Some("just here")
        );
        assert_eq!(qa.entries["tier"]["gold"].values["tier"], "g-1");
        let prod = crate::varmodel::parse_environment(&out.envs[1].1).unwrap();
        assert!(!prod.entries["tier"].contains_key("qa-only"));
        assert_eq!(prod.entries["tier"]["gold"].values["tier"], "g-1");
    }

    #[test]
    fn a_plain_variable_enumerated_only_in_an_env_also_becomes_a_group() {
        let vars = "[shard]\ndescription = \"which shard\"\n";
        let envs = vec![(
            "qa".to_string(),
            "[options.shard.east]\nvalue = \"e-1\"\n".to_string(),
        )];
        assert!(needs_migration(vars, &envs));
        let out = migrate(vars, &envs).unwrap();
        let m = crate::varmodel::parse_variables(&out.variables.unwrap()).unwrap();
        assert_eq!(m.groups["shard"].fields, ["shard"]);
        assert!(m.vars.is_empty());
        let e = crate::varmodel::parse_environment(&out.envs[0].1).unwrap();
        assert_eq!(e.entries["shard"]["east"].values["shard"], "e-1");
        crate::varmodel::validate_env(&m, &e).unwrap();
    }

    #[test]
    fn env_options_for_an_undeclared_name_is_an_error() {
        let vars = "[base_url]\ndefault = \"x\"\n";
        let envs = vec![(
            "qa".to_string(),
            "[options.ghost.a]\nvalue = \"1\"\n".to_string(),
        )];
        let err = migrate(vars, &envs).unwrap_err();
        assert!(
            matches!(&err, EditError::Parse(m) if m.contains("qa") && m.contains("options.ghost")),
            "{err:?}"
        );
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
        // nothing); stage 7 requires every entry to supply every field, so
        // the gap is filled with an empty value and called out rather than
        // silently invented.
        let vars = r#"[groups.user]
members = ["user_id", "customer_id"]
[groups.user.options.alice]
user_id = "1001"
"#;
        let out = migrate(vars, &[("qa".to_string(), String::new())]).unwrap();
        let m = crate::varmodel::parse_variables(&out.variables.unwrap()).unwrap();
        let e = crate::varmodel::parse_environment(&out.envs[0].1).unwrap();
        assert_eq!(e.entries["user"]["alice"].values["customer_id"], "");
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
        assert!(needs_migration(
            "[groups.user]\nfields = [\"a\"]\n[groups.user.options.x]\na = \"1\"\n",
            &[]
        ));
        assert!(needs_migration(
            "[base_url]\ndefault = \"x\"\n",
            &[(
                "qa".to_string(),
                "[options.base_url.a]\nvalue = \"1\"\n".to_string()
            )]
        ));
        assert!(!needs_migration("", &[("qa".to_string(), String::new())]));
    }
}

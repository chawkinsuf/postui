//! `VarEdit`: one enum for every variable-file mutation the Variable
//! Manager performs. `Project::apply_var_edit` runs the journaled text
//! primitive (and its cascade) as one undo entry. The request-scope edits
//! (`SetRequestVar`) and the pure selection change (`SelectOption`) stay
//! in the app: they do not touch variable files.

use super::*;
use crate::varedit::{self, EditError, PromoteTarget};
use crate::vars::is_valid_var_name;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VarEdit {
    SetEnvValue { env: String, name: String, value: String },
    SetDefault { name: String, value: String },
    SetDescription { owner: String, value: String },
    SetSecretValue { env: String, name: String, value: String },
    SetOptionValue { env: String, selector: String, option: String, field: String, value: String },
    SetOptionDescription { env: String, selector: String, option: String, description: Option<String> },
    NewVar { name: String, description: Option<String> },
    NewSelector { name: String, fields: Vec<String>, shared: bool },
    Rename { from: String, to: String },
    Delete { name: String },
    ToggleSecret { name: String },
    SetFields { selector: String, fields: Vec<String> },
    /// `request_value` is the request-scope value being promoted; the app
    /// removes it from the editor and saves the request afterwards.
    Promote { name: String, request_value: String, target: PromoteTarget },
    NewOption { env: String, selector: String, name: String, description: Option<String>, values: IndexMap<String, String> },
    RenameOption { env: String, selector: String, from: String, to: String },
    DeleteOption { env: String, selector: String, name: String },
    /// Lands a copied option row in `env` (spec: the Manager's copy/paste
    /// pair). The row travels with the app, so its description and values
    /// come in as data rather than being read from a source environment.
    PasteOption {
        env: String,
        selector: String,
        name: String,
        description: Option<String>,
        values: IndexMap<String, String>,
    },
}

impl VarEdit {
    /// The journal label.
    pub fn label(&self) -> &'static str {
        match self {
            VarEdit::SetEnvValue { .. }
            | VarEdit::SetDefault { .. }
            | VarEdit::SetDescription { .. }
            | VarEdit::SetSecretValue { .. }
            | VarEdit::SetOptionValue { .. }
            | VarEdit::SetOptionDescription { .. } => "edit variable",
            VarEdit::NewVar { .. } => "new variable",
            VarEdit::NewSelector { .. } => "new selector",
            VarEdit::Rename { .. } => "rename variable",
            VarEdit::Delete { .. } => "delete variable",
            VarEdit::ToggleSecret { .. } => "toggle secret",
            VarEdit::SetFields { .. } => "edit selector fields",
            VarEdit::Promote { .. } => "promote variable",
            VarEdit::NewOption { .. } => "new option",
            VarEdit::RenameOption { .. } => "rename option",
            VarEdit::DeleteOption { .. } => "delete option",
            VarEdit::PasteOption { .. } => "paste option",
        }
    }
}

fn edit(msg: String) -> Error {
    Error::Edit(msg)
}

impl Project {
    /// Whether `selector` is a shared selector — its options (and its one
    /// global selection) live in `variables.toml`, not per environment.
    fn selector_is_shared(&self, selector: &str) -> bool {
        self.model.selectors.get(selector).is_some_and(|d| d.shared)
    }

    /// Whether `n` can't be a new declaration's name: one of the reserved
    /// table names, or a variable / selector that already exists.
    fn name_taken(&self, n: &str) -> bool {
        n == "options"
            || n == "groups"
            || n == "entries"
            || n == "selectors"
            || self.model.vars.contains_key(n)
            || self.model.selectors.contains_key(n)
    }

    /// `selector`'s options as they currently stand, from wherever they
    /// live: the model's own for a shared selector, `env`'s otherwise.
    fn options_of_for(&mut self, env: &str, selector: &str) -> Option<IndexMap<String, varmodel::OptionDecl>> {
        if self.selector_is_shared(selector) {
            self.model.options.get(selector).cloned()
        } else {
            let data = self.env_data_for(env);
            varmodel::selector_options(&data, selector).cloned()
        }
    }

    /// `env`'s data: the active environment's is already in memory; any
    /// other is read fresh, degrading to empty rather than erroring.
    fn env_data_for(&mut self, env: &str) -> EnvData {
        if self.active_env.as_deref() == Some(env) {
            self.env_data.clone()
        } else {
            self.load_environment(env).unwrap_or_default()
        }
    }

    /// Applies an option-table edit to wherever `selector`'s options live:
    /// `variables.toml` for a shared selector (`env` is ignored — the same
    /// `[options.*]` verbs apply, just in the model's own file), otherwise
    /// `environments/<env>.toml`.
    fn edit_options_home(
        &mut self,
        selector: &str,
        env: &str,
        f: impl FnOnce(&str) -> Result<String, EditError>,
    ) -> Result<(), Error> {
        if self.selector_is_shared(selector) {
            self.edit_variables(f)
        } else {
            self.edit_env(env, f)
        }
    }

    /// Applies one Variable Manager mutation as a single journal entry.
    /// `Err` is safe to toast (never a secret value) and leaves every file
    /// and the in-memory model exactly as they were.
    pub fn apply_var_edit(&mut self, e: &VarEdit) -> Result<(), Error> {
        let label = e.label();
        match e {
            VarEdit::SetEnvValue { env, name, value } => {
                self.cascade(label, |p| p.edit_env(env, |doc| varedit::set_env_value(doc, name, Some(value))))
            }
            VarEdit::SetDefault { name, value } => {
                self.cascade(label, |p| p.edit_variables(|doc| varedit::upsert_var(doc, name, None, Some(value))))
            }
            VarEdit::SetDescription { owner, value } => {
                if self.model.vars.contains_key(owner) {
                    self.cascade(label, |p| p.edit_variables(|doc| varedit::upsert_var(doc, owner, Some(value), None)))
                } else if let Some(fields) = self.model.selectors.get(owner).map(|g| g.fields.clone()) {
                    self.cascade(label, |p| {
                        p.edit_variables(|doc| varedit::upsert_selector(doc, owner, Some(value), &fields))
                    })
                } else {
                    Err(edit(format!("\"{owner}\" is not a declared variable or selector")))
                }
            }
            VarEdit::SetSecretValue { env, name, value } => self.set_secret_for(env, name, value.clone()),
            VarEdit::SetOptionValue { env, selector, option, field, value } => {
                // An option's values live in one file — its selector's env
                // file, or variables.toml for a shared selector; the cell
                // being edited is one field of that option.
                let mut values = IndexMap::new();
                values.insert(field.clone(), value.clone());
                self.cascade(label, |p| {
                    p.edit_options_home(selector, env, |doc| varedit::upsert_option(doc, selector, option, None, &values))
                })
            }
            VarEdit::SetOptionDescription { env, selector, option, description } => {
                self.cascade(label, |p| {
                    p.edit_options_home(selector, env, |doc| match description {
                        Some(d) => varedit::upsert_option(doc, selector, option, Some(d), &IndexMap::new()),
                        None => varedit::remove_option_description(doc, selector, option),
                    })
                })
            }
            VarEdit::NewVar { name, description } => {
                if !is_valid_var_name(name) {
                    return Err(edit(format!("\"{name}\" is not a valid variable name")));
                }
                if self.name_taken(name) {
                    return Err(edit(format!("\"{name}\" already exists")));
                }
                self.cascade(label, |p| {
                    p.edit_variables(|doc| varedit::upsert_var(doc, name, description.as_deref(), None))
                })
            }
            VarEdit::NewSelector { name, fields, shared } => {
                if !is_valid_var_name(name) {
                    return Err(edit(format!("\"{name}\" is not a valid selector name")));
                }
                if self.name_taken(name) {
                    return Err(edit(format!("\"{name}\" already exists")));
                }
                for f in fields {
                    if !is_valid_var_name(f) {
                        return Err(edit(format!("\"{f}\" is not a valid field name")));
                    }
                }
                self.cascade(label, |p| {
                    p.edit_variables(|doc| {
                        let out = varedit::upsert_selector(doc, name, None, fields)?;
                        if *shared {
                            varedit::set_selector_shared(&out, name, true)
                        } else {
                            Ok(out)
                        }
                    })
                })
            }
            VarEdit::Rename { from, to } => {
                if !is_valid_var_name(to) {
                    return Err(edit(format!("\"{to}\" is not a valid variable name")));
                }
                if self.name_taken(to) {
                    return Err(edit(format!("\"{to}\" already exists")));
                }
                if self.model.selectors.contains_key(from) {
                    return self.rename_selector_cascade(from, to);
                }
                // `rename_var` only ever touches `variables.toml` — an
                // active env override for `from` would otherwise silently
                // degrade to the default post-rename. Cascade into every
                // environment's flat pair and its `[options.<from>]` table
                // too; `rename_env_var` no-ops for an environment with
                // nothing to rename.
                let envs = self.environments.clone();
                self.cascade(label, |p| {
                    p.edit_variables(|doc| varedit::rename_var(doc, from, to))?;
                    for env in &envs {
                        p.edit_env(env, |doc| varedit::rename_env_var(doc, from, to))?;
                    }
                    Ok(())
                })
            }
            VarEdit::Delete { name } => self.delete_declaration(name),
            VarEdit::ToggleSecret { name } => self.toggle_secret(name),
            VarEdit::SetFields { selector, fields } => {
                for f in fields {
                    if !is_valid_var_name(f) {
                        return Err(edit(format!("\"{f}\" is not a valid field name")));
                    }
                }
                // A shared selector's options sit in the same file and must
                // supply exactly the declared fields, so the list change
                // carries them along in the one write.
                let current: Vec<String> =
                    self.model.selectors.get(selector).map(|g| g.fields.clone()).unwrap_or_default();
                let shared = self.selector_is_shared(selector);
                self.cascade(label, |p| {
                    p.edit_variables(|doc| {
                        let mut out = varedit::upsert_selector(doc, selector, None, fields)?;
                        if shared {
                            for field in fields.iter().filter(|f| !current.contains(f)) {
                                out = varedit::ensure_option_field(&out, selector, field)?;
                            }
                            for field in current.iter().filter(|f| !fields.contains(f)) {
                                out = varedit::strip_option_field(&out, selector, field)?;
                            }
                        }
                        Ok(out)
                    })
                })
            }
            VarEdit::Promote { name, request_value, target } => {
                let vars_text = self.variables_text()?;
                let env_name = self.active_env.clone();
                let env_text = match &env_name {
                    Some(env) => Some(self.env_text(env)?),
                    None => None,
                };
                let (new_vars, new_env) =
                    varedit::promote_var(&vars_text, env_text.as_deref(), name, request_value, *target)
                        .map_err(|e| edit(e.to_string()))?;
                self.cascade(label, |p| {
                    p.edit_variables(|_| Ok(new_vars))?;
                    if let (Some(new_env), Some(env)) = (new_env, env_name) {
                        p.edit_env(&env, |_| Ok(new_env))?;
                    }
                    Ok(())
                })
            }
            VarEdit::NewOption { env, selector, name, description, values } => self.cascade(label, |p| {
                p.edit_options_home(selector, env, |doc| {
                    varedit::upsert_option(doc, selector, name, description.as_deref(), values)
                })
            }),
            VarEdit::RenameOption { env, selector, from, to } => {
                let shared = self.selector_is_shared(selector);
                self.cascade(label, |p| {
                    p.edit_options_home(selector, env, |doc| varedit::rename_option(doc, selector, from, to))?;
                    // A selection names an option by key: carry it across
                    // the rename rather than leaving a dangling one behind.
                    // (A shared selector's selection is the global one;
                    // `set_selection_for` routes there itself.)
                    let selected = if shared {
                        p.local.shared_selections.get(selector)
                    } else {
                        p.selections_for(env).get(selector)
                    };
                    if selected.map(String::as_str) == Some(from.as_str()) {
                        p.set_selection_for(env, selector, to);
                    }
                    Ok(())
                })
            }
            VarEdit::DeleteOption { env, selector, name } => {
                // An option that is already gone is a quiet no-op success
                // (a stale row — nothing left to do). Any per-env selection
                // naming it is cleared everywhere so local state doesn't
                // accumulate dead selections.
                let present = self.options_of_for(env, selector).is_some_and(|o| o.contains_key(name));
                let shared = self.selector_is_shared(selector);
                let envs = self.environments.clone();
                self.cascade(label, |p| {
                    if present {
                        p.edit_options_home(selector, env, |doc| varedit::delete_option(doc, selector, name))?;
                    }
                    if shared {
                        if p.local.shared_selections.get(selector).map(String::as_str) == Some(name.as_str()) {
                            p.clear_selection_for(env, selector);
                        }
                        return Ok(());
                    }
                    for other in &envs {
                        if p.selections_for(other).get(selector).map(String::as_str) == Some(name.as_str()) {
                            p.clear_selection_for(other, selector);
                        }
                    }
                    Ok(())
                })
            }
            VarEdit::PasteOption {
                env,
                selector,
                name,
                description,
                values,
            } => {
                // The copied row keeps its own name when `env` has nothing
                // by it, and steps aside when it does — `"<name> copy"`,
                // then `"<name> copy-2"`, … while the name is taken. A
                // paste never overwrites the row it collides with, so
                // pasting back into the environment a row came from is
                // exactly a duplicate of it.
                let taken = self.options_of_for(env, selector).unwrap_or_default();
                let mut fresh = name.clone();
                if taken.contains_key(&fresh) {
                    fresh = format!("{name} copy");
                    let mut n = 2;
                    while taken.contains_key(&fresh) {
                        fresh = format!("{name} copy-{n}");
                        n += 1;
                    }
                }
                self.cascade(label, |p| {
                    p.edit_options_home(selector, env, |doc| {
                        varedit::upsert_option(doc, selector, &fresh, description.as_deref(), values)
                    })
                })
            }
        }
    }

    /// [`VarEdit::Rename`] for a selector. Both halves of the declaration
    /// have to move at once: an environment's `[options.<old>]` table names
    /// a selector the renamed model no longer declares, and the new name
    /// has no options yet — so `validate_env` refuses whichever half lands
    /// first, in either order. Each environment's recorded selection is
    /// carried across too.
    fn rename_selector_cascade(&mut self, from: &str, to: &str) -> Result<(), Error> {
        let shared = self.selector_is_shared(from);
        let envs = self.environments.clone();
        self.cascade("rename selector", |p| {
            // A shared selector renames wholly inside variables.toml — the
            // declaration and its `[options.<from>]` subtree in one write —
            // and carries its one global selection.
            if shared {
                p.edit_variables(|doc| {
                    let renamed = varedit::rename_selector(doc, from, to)?;
                    varedit::rename_selector_options(&renamed, from, to)
                })?;
                if let Some(key) = p.local.shared_selections.get(from).cloned() {
                    p.clear_selection_for("", from);
                    p.set_selection_for("", to, &key);
                }
                return Ok(());
            }
            p.edit_variables_and_envs(
                |doc| varedit::rename_selector(doc, from, to),
                |doc| varedit::rename_selector_options(doc, from, to),
            )?;
            for env in &envs {
                if let Some(key) = p.selections_for(env).get(from).cloned() {
                    p.clear_selection_for(env, from);
                    p.set_selection_for(env, to, &key);
                }
            }
            Ok(())
        })
    }

    /// [`VarEdit::Delete`]. `delete_var`/`delete_selector` only ever touch
    /// `variables.toml`; an env's `[options.<name>]` table for the deleted
    /// name would otherwise strand that env file. Cascade into every
    /// environment FIRST — a strip can only shrink an env file, so it can
    /// never itself fail `validate_env` — and only THEN remove the
    /// declaration. A shared selector's options live in `variables.toml`
    /// with the declaration: both halves go in one write.
    fn delete_declaration(&mut self, name: &str) -> Result<(), Error> {
        let is_group = self.model.selectors.contains_key(name);
        // Mirror `delete_var`'s own "still a selector field" conflict up
        // front, using the already-loaded model — before any environment
        // file is touched, so a refusal here leaves everything unchanged.
        if !is_group
            && let Some(gname) = self
                .model
                .selectors
                .iter()
                .find_map(|(gname, g)| g.fields.iter().any(|f| f == name).then(|| gname.clone()))
        {
            return Err(edit(format!(
                "variable \"{name}\" is a field of selector \"{gname}\"; remove it from the selector first"
            )));
        }
        let shared = is_group && self.selector_is_shared(name);
        let envs = self.environments.clone();
        self.cascade("delete variable", |p| {
            if shared {
                p.edit_variables(|doc| {
                    let stripped = varedit::delete_selector_options(doc, name)?;
                    varedit::delete_selector(&stripped, name)
                })?;
                p.clear_selection_for("", name);
                return Ok(());
            }
            for env in &envs {
                if is_group {
                    p.edit_env(env, |doc| varedit::delete_selector_options(doc, name))?;
                    p.clear_selection_for(env, name);
                } else {
                    p.edit_env(env, |doc| varedit::delete_env_var(doc, name))?;
                }
            }
            if is_group {
                p.edit_variables(|doc| varedit::delete_selector(doc, name))
            } else {
                p.edit_variables(|doc| varedit::delete_var(doc, name))
            }
        })
    }

    /// [`VarEdit::ToggleSecret`]'s two directions. Off->on moves every
    /// environment's flat value for `name` into that environment's
    /// `.local/secrets.toml` slot and strips it from the env file; on->off
    /// only flips the flag — the local secret value is left exactly where
    /// it is (never silently promoted into a git-tracked file).
    fn toggle_secret(&mut self, name: &str) -> Result<(), Error> {
        let currently_secret = self.model.vars.get(name).is_some_and(|d| d.secret);
        if currently_secret {
            return self.cascade("toggle secret", |p| {
                p.edit_variables(|doc| varedit::set_secret_flag(doc, name, false))
            });
        }
        let mut to_move: Vec<(String, String)> = Vec::new();
        for env in self.environments.clone() {
            let data = self.env_data_for(&env);
            if let Some(v) = data.values.get(name) {
                to_move.push((env, v.clone()));
            }
        }
        // Order matters: `edit_variables` validates the flipped flag
        // against the *active* env's current data, so a still-present flat
        // value there would reject the flag flip. Move each value into
        // secrets.toml and strip it from its env file first, so the flag
        // flip last sees a model already consistent with every
        // environment's (now-empty) flat value.
        self.cascade("toggle secret", |p| {
            for (env, value) in &to_move {
                p.set_secret_for(env, name, value.clone())?;
            }
            for (env, _) in &to_move {
                p.edit_env(env, |doc| varedit::set_env_value(doc, name, None))?;
            }
            p.edit_variables(|doc| varedit::set_secret_flag(doc, name, true))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::read;
    use super::*;

    /// The shared `fixture()` has one flat variable and no selector, so
    /// the Manager's ops have nothing to bite on. This one declares
    /// `base_url` (default), `api_key` (secret) and the `creds` selector
    /// (`user_id`, `customer_id`), with `qa` holding one option.
    fn vars_fixture() -> (tempfile::TempDir, Project) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("requests/main")).unwrap();
        std::fs::create_dir_all(root.join("environments")).unwrap();
        std::fs::write(
            root.join("project.toml"),
            "name = \"Vars\"\nspaces = [\"main\"]\n",
        )
        .unwrap();
        std::fs::write(
            root.join("variables.toml"),
            "[base_url]\ndefault = \"http://localhost\"\n\n[api_key]\nsecret = true\n\n[selectors.creds]\nfields = [\"user_id\", \"customer_id\"]\n",
        )
        .unwrap();
        std::fs::write(root.join("environments/dev.toml"), "").unwrap();
        std::fs::write(
            root.join("environments/qa.toml"),
            "[options.creds.alice]\nuser_id = \"1\"\ncustomer_id = \"2\"\n",
        )
        .unwrap();
        let (project, warnings) = Project::open(root.to_path_buf()).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        (dir, project)
    }

    #[test]
    fn set_default_and_description_edit_variables_toml_as_one_entry_each() {
        let (dir, mut p) = vars_fixture();
        p.apply_var_edit(&VarEdit::SetDefault {
            name: "base_url".into(),
            value: "http://x".into(),
        })
        .unwrap();
        assert!(read(&dir, "variables.toml").unwrap().contains("default = \"http://x\""));
        assert_eq!(p.journal_len(), 1);
        p.apply_var_edit(&VarEdit::SetDescription {
            owner: "creds".into(),
            value: "paired".into(),
        })
        .unwrap();
        assert!(read(&dir, "variables.toml").unwrap().contains("description = \"paired\""));
        assert_eq!(p.journal_len(), 2);
        assert_eq!(
            p.apply_var_edit(&VarEdit::SetDescription {
                owner: "nope".into(),
                value: "x".into()
            })
            .unwrap_err()
            .to_string(),
            "\"nope\" is not a declared variable or selector"
        );
    }

    #[test]
    fn rename_var_cascades_into_every_env_file_under_one_entry_and_undoes_whole() {
        let (dir, mut p) = vars_fixture();
        std::fs::write(dir.path().join("environments/dev.toml"), "base_url = \"d\"\n").unwrap();
        p.invalidate_stamps();
        p.poll();
        p.apply_var_edit(&VarEdit::Rename {
            from: "base_url".into(),
            to: "root_url".into(),
        })
        .unwrap();
        assert!(read(&dir, "environments/dev.toml").unwrap().contains("root_url"));
        assert!(read(&dir, "variables.toml").unwrap().contains("[root_url]"));
        assert_eq!(p.journal_len(), 1);
        p.undo().unwrap();
        assert!(read(&dir, "environments/dev.toml").unwrap().contains("base_url = \"d\""));
        assert!(read(&dir, "variables.toml").unwrap().contains("[base_url]"));
    }

    #[test]
    fn delete_selector_clears_its_selection_in_every_env() {
        let (_dir, mut p) = vars_fixture();
        p.set_selection_for("qa", "creds", "alice");
        p.apply_var_edit(&VarEdit::Delete { name: "creds".into() }).unwrap();
        assert!(p.selections_for("qa").get("creds").is_none());
        assert!(!p.variables().selectors.contains_key("creds"));
        // a field of a still-declared selector is refused with today's wording
        p.apply_var_edit(&VarEdit::NewSelector {
            name: "pair".into(),
            fields: vec!["left".into()],
            shared: false,
        })
        .unwrap();
        assert_eq!(
            p.apply_var_edit(&VarEdit::Delete { name: "left".into() })
                .unwrap_err()
                .to_string(),
            "variable \"left\" is a field of selector \"pair\"; remove it from the selector first"
        );
    }

    #[test]
    fn toggle_secret_moves_env_values_into_secrets_and_back_on_undo() {
        let (dir, mut p) = vars_fixture();
        std::fs::write(dir.path().join("environments/dev.toml"), "base_url = \"d\"\n").unwrap();
        p.invalidate_stamps();
        p.poll();
        p.apply_var_edit(&VarEdit::ToggleSecret {
            name: "base_url".into(),
        })
        .unwrap();
        assert!(!read(&dir, "environments/dev.toml").unwrap().contains("base_url"));
        assert_eq!(
            p.secrets().get("dev").and_then(|m| m.get("base_url")).map(String::as_str),
            Some("d")
        );
        assert!(p.variables().vars["base_url"].secret);
        p.undo().unwrap();
        assert!(read(&dir, "environments/dev.toml").unwrap().contains("base_url = \"d\""));
        assert!(p.secrets().get("dev").and_then(|m| m.get("base_url")).is_none());
    }

    #[test]
    fn option_edits_go_to_the_shared_or_env_home() {
        let (dir, mut p) = vars_fixture();
        let mut values = IndexMap::new();
        values.insert("user_id".to_string(), "9".to_string());
        values.insert("customer_id".to_string(), "8".to_string());
        p.apply_var_edit(&VarEdit::NewOption {
            env: "qa".into(),
            selector: "creds".into(),
            name: "carol".into(),
            description: None,
            values,
        })
        .unwrap();
        assert!(read(&dir, "environments/qa.toml").unwrap().contains("[options.creds.carol]"));
        p.apply_var_edit(&VarEdit::RenameOption {
            env: "qa".into(),
            selector: "creds".into(),
            from: "carol".into(),
            to: "dave".into(),
        })
        .unwrap();
        assert!(read(&dir, "environments/qa.toml").unwrap().contains("[options.creds.dave]"));
        p.apply_var_edit(&VarEdit::DeleteOption {
            env: "qa".into(),
            selector: "creds".into(),
            name: "dave".into(),
        })
        .unwrap();
        assert!(!read(&dir, "environments/qa.toml").unwrap().contains("dave"));
    }

    #[test]
    fn paste_option_lands_the_whole_row_in_another_env_and_undoes_whole() {
        let (dir, mut p) = vars_fixture();
        let mut values = IndexMap::new();
        values.insert("user_id".to_string(), "1".to_string());
        values.insert("customer_id".to_string(), "2".to_string());
        p.apply_var_edit(&VarEdit::PasteOption {
            env: "dev".into(),
            selector: "creds".into(),
            name: "alice".into(),
            description: Some("the first one".into()),
            values,
        })
        .unwrap();
        let dev = read(&dir, "environments/dev.toml").unwrap();
        assert!(dev.contains("[options.creds.alice]"), "{dev}");
        assert!(dev.contains("user_id = \"1\""), "{dev}");
        assert!(dev.contains("customer_id = \"2\""), "{dev}");
        assert!(dev.contains("description = \"the first one\""), "{dev}");
        // The source environment is untouched.
        assert!(read(&dir, "environments/qa.toml").unwrap().contains("[options.creds.alice]"));
        assert_eq!(p.journal_len(), 1);
        p.undo().unwrap();
        assert!(!read(&dir, "environments/dev.toml").unwrap().contains("alice"));
    }

    #[test]
    fn paste_option_freshens_a_name_the_target_env_already_has() {
        let (dir, mut p) = vars_fixture();
        let mut values = IndexMap::new();
        values.insert("user_id".to_string(), "1".to_string());
        values.insert("customer_id".to_string(), "2".to_string());
        let paste = VarEdit::PasteOption {
            env: "qa".into(),
            selector: "creds".into(),
            name: "alice".into(),
            description: None,
            values,
        };
        p.apply_var_edit(&paste).unwrap();
        assert!(read(&dir, "environments/qa.toml").unwrap().contains("[options.creds.\"alice copy\"]"));
        p.apply_var_edit(&paste).unwrap();
        let qa = read(&dir, "environments/qa.toml").unwrap();
        assert!(qa.contains("[options.creds.\"alice copy-2\"]"), "{qa}");
        // The row it collided with is still there, unchanged.
        assert!(qa.contains("[options.creds.alice]"), "{qa}");
    }
}

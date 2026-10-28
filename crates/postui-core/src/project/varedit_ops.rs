//! `VarEdit`: one enum for every variable-file mutation the Variable
//! Manager performs. `Project::apply_var_edit` runs the journaled text
//! primitive (and its cascade) as one undo entry. The request-scope edits
//! (`SetRequestVar`) and the pure selection change (`SelectOption`) stay
//! in the app: they do not touch variable files.

use super::*;
use crate::model::HttpRequest;
use crate::varedit::{self, EditError, PromoteTarget};
use crate::vars::is_valid_var_name;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VarEdit {
    SetEnvValue {
        env: String,
        name: String,
        value: String,
    },
    SetDefault {
        name: String,
        value: String,
    },
    SetDescription {
        owner: String,
        value: String,
    },
    SetSecretValue {
        env: String,
        name: String,
        value: String,
    },
    SetOptionValue {
        env: String,
        selector: String,
        option: String,
        field: String,
        value: String,
    },
    SetOptionDescription {
        env: String,
        selector: String,
        option: String,
        description: Option<String>,
    },
    NewVar {
        name: String,
        description: Option<String>,
    },
    NewSelector {
        name: String,
        fields: Vec<String>,
        shared: bool,
    },
    Rename {
        from: String,
        to: String,
    },
    Delete {
        name: String,
    },
    ToggleSecret {
        name: String,
    },
    SetFields {
        selector: String,
        fields: Vec<String>,
    },
    /// `request_value` is the request-scope value being promoted; the app
    /// removes it from the editor and saves the request afterwards.
    Promote {
        name: String,
        request_value: String,
        target: PromoteTarget,
    },
    /// Creates option `name`; refused when the file already holds one by
    /// that name ([`varedit::create_option`]).
    NewOption {
        env: String,
        selector: String,
        name: String,
        description: Option<String>,
        values: IndexMap<String, String>,
    },
    RenameOption {
        env: String,
        selector: String,
        from: String,
        to: String,
    },
    DeleteOption {
        env: String,
        selector: String,
        name: String,
    },
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

/// The name a pasted option called `name` takes among the options `taken`
/// says exist ([`VarEdit::PasteOption`]): its own when free, else
/// `"<name> copy"`, then `"<name> copy-2"`, … while the name is taken. The
/// app reads it before the paste to know which row the paste lands.
pub fn pasted_option_name(name: &str, taken: impl Fn(&str) -> bool) -> String {
    if !taken(name) {
        return name.to_string();
    }
    let mut fresh = format!("{name} copy");
    let mut n = 2;
    while taken(&fresh) {
        fresh = format!("{name} copy-{n}");
        n += 1;
    }
    fresh
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
        varmodel::is_reserved(n)
            || self.model.vars.contains_key(n)
            || self.model.selectors.contains_key(n)
    }

    /// Whether the project has `n`: a variable, selector or selector field
    /// it declares, or a value some environment file sets without
    /// declaring it (`resolve_env` reads those too). With
    /// `varmodel::is_reserved`, what `create_request_with_secrets` refuses,
    /// for a caller to filter first. An environment file that can't be
    /// read is an error: what it sets is unknown.
    pub fn variable_exists(&mut self, n: &str) -> Result<bool, Error> {
        if self.model.vars.contains_key(n)
            || self.model.selectors.contains_key(n)
            || self.model.selectors.values().any(|d| d.fields.iter().any(|f| f == n))
        {
            return Ok(true);
        }
        for env in self.environments.clone() {
            if self.load_environment(&env)?.values.contains_key(n) {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// `selector`'s options as they currently stand, from wherever they
    /// live: the model's own for a shared selector, `env`'s otherwise.
    fn options_of_for(
        &mut self,
        env: &str,
        selector: &str,
    ) -> Option<IndexMap<String, varmodel::OptionDecl>> {
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
            VarEdit::SetEnvValue { env, name, value } => self.cascade(label, |p| {
                p.edit_env(env, |doc| varedit::set_env_value(doc, name, Some(value)))
            }),
            VarEdit::SetDefault { name, value } => self.cascade(label, |p| {
                p.edit_variables(|doc| varedit::upsert_var(doc, name, None, Some(value)))
            }),
            VarEdit::SetDescription { owner, value } => {
                if self.model.vars.contains_key(owner) {
                    self.cascade(label, |p| {
                        p.edit_variables(|doc| varedit::upsert_var(doc, owner, Some(value), None))
                    })
                } else if let Some(fields) =
                    self.model.selectors.get(owner).map(|g| g.fields.clone())
                {
                    self.cascade(label, |p| {
                        p.edit_variables(|doc| {
                            varedit::upsert_selector(doc, owner, Some(value), &fields)
                        })
                    })
                } else {
                    Err(edit(format!("\"{owner}\" is not a declared variable or selector")))
                }
            }
            VarEdit::SetSecretValue { env, name, value } => {
                self.set_secret_for(env, name, value.clone())
            }
            VarEdit::SetOptionValue { env, selector, option, field, value } => {
                // An option's values live in one file — its selector's env
                // file, or variables.toml for a shared selector; the cell
                // being edited is one field of that option.
                let mut values = IndexMap::new();
                values.insert(field.clone(), value.clone());
                self.cascade(label, |p| {
                    p.edit_options_home(selector, env, |doc| {
                        varedit::upsert_option(doc, selector, option, None, &values)
                    })
                })
            }
            VarEdit::SetOptionDescription { env, selector, option, description } => {
                self.cascade(label, |p| {
                    p.edit_options_home(selector, env, |doc| match description {
                        Some(d) => {
                            varedit::upsert_option(doc, selector, option, Some(d), &IndexMap::new())
                        }
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
                    p.edit_variables(|doc| {
                        varedit::upsert_var(doc, name, description.as_deref(), None)
                    })
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
                let current: Vec<String> = self
                    .model
                    .selectors
                    .get(selector)
                    .map(|g| g.fields.clone())
                    .unwrap_or_default();
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
                let (new_vars, new_env) = varedit::promote_var(
                    &vars_text,
                    env_text.as_deref(),
                    name,
                    request_value,
                    *target,
                )
                .map_err(|e| edit(e.to_string()))?;
                self.cascade(label, |p| {
                    p.edit_variables(|_| Ok(new_vars))?;
                    if let (Some(new_env), Some(env)) = (new_env, env_name) {
                        p.edit_env(&env, |_| Ok(new_env))?;
                    }
                    Ok(())
                })
            }
            // A create, never an update: an option `name` that the file
            // already holds is refused, checked against the file text the
            // write reads (task #196).
            VarEdit::NewOption { env, selector, name, description, values } => {
                self.cascade(label, |p| {
                    p.edit_options_home(selector, env, |doc| {
                        varedit::create_option(doc, selector, name, description.as_deref(), values)
                    })
                })
            }
            VarEdit::RenameOption { env, selector, from, to } => {
                let shared = self.selector_is_shared(selector);
                self.cascade(label, |p| {
                    p.edit_options_home(selector, env, |doc| {
                        varedit::rename_option(doc, selector, from, to)
                    })?;
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
                let present =
                    self.options_of_for(env, selector).is_some_and(|o| o.contains_key(name));
                let shared = self.selector_is_shared(selector);
                let envs = self.environments.clone();
                self.cascade(label, |p| {
                    if present {
                        p.edit_options_home(selector, env, |doc| {
                            varedit::delete_option(doc, selector, name)
                        })?;
                    }
                    if shared {
                        if p.local.shared_selections.get(selector).map(String::as_str)
                            == Some(name.as_str())
                        {
                            p.clear_selection_for(env, selector);
                        }
                        return Ok(());
                    }
                    for other in &envs {
                        if p.selections_for(other).get(selector).map(String::as_str)
                            == Some(name.as_str())
                        {
                            p.clear_selection_for(other, selector);
                        }
                    }
                    Ok(())
                })
            }
            VarEdit::PasteOption { env, selector, name, description, values } => {
                // The copied row keeps its own name when `env` has nothing
                // by it, and steps aside when it does — `"<name> copy"`,
                // then `"<name> copy-2"`, … while the name is taken. A
                // paste never overwrites the row it collides with, so
                // pasting back into the environment a row came from is
                // exactly a duplicate of it. The name is picked from the
                // loaded options; the write is a create, so a name another
                // program took since the last reload is refused rather
                // than written over (task #196). Refused, not stepped
                // aside: re-picking against the file would be a second
                // name rule inside the write; after a reload the paste
                // steps aside as usual.
                let taken = self.options_of_for(env, selector).unwrap_or_default();
                let fresh = pasted_option_name(name, |n| taken.contains_key(n));
                self.cascade(label, |p| {
                    p.edit_options_home(selector, env, |doc| {
                        varedit::create_option(
                            doc,
                            selector,
                            &fresh,
                            description.as_deref(),
                            values,
                        )
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
            && let Some(gname) =
                self.model.selectors.iter().find_map(|(gname, g)| {
                    g.fields.iter().any(|f| f == name).then(|| gname.clone())
                })
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
    /// `secrets.toml` slot and strips it from the env file; on->off
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

    /// `create_request`, with each of `secrets` declared as a secret
    /// variable holding its value, if it has one, in the active
    /// environment, all one undo entry (curl import spec R7). A name that
    /// is not a valid variable name, is reserved or that the project has
    /// (`variable_exists`) is refused before anything lands: an import
    /// never changes a variable the project has. With no secrets it is
    /// `create_request` itself.
    pub fn create_request_with_secrets(
        &mut self,
        display_path: &str,
        req: HttpRequest,
        secrets: &[(String, Option<String>)],
    ) -> Result<(String, String), Error> {
        if secrets.is_empty() {
            return self.create_request(display_path, req);
        }
        self.check_new_secrets(secrets)?;
        self.transaction("create request", EntryMeta::default(), |p| {
            let created = p.create_request(display_path, req)?;
            p.write_new_secrets(secrets)?;
            Ok(created)
        })
    }

    /// Each of `secrets` declared as a secret variable holding its value,
    /// if it has one, in the active environment, as one undo entry (curl
    /// replace spec U3/U5). Refuses as `create_request_with_secrets`
    /// does, before anything lands; with no secrets it does nothing.
    pub fn declare_secrets(&mut self, secrets: &[(String, Option<String>)]) -> Result<(), Error> {
        if secrets.is_empty() {
            return Ok(());
        }
        self.check_new_secrets(secrets)?;
        self.transaction("declare variables", EntryMeta::default(), |p| {
            p.write_new_secrets(secrets)
        })
    }

    /// Refuses a name that is not a valid variable name, is reserved, or
    /// that the project has (`variable_exists`).
    fn check_new_secrets(&mut self, secrets: &[(String, Option<String>)]) -> Result<(), Error> {
        for (name, _) in secrets {
            if !is_valid_var_name(name) {
                return Err(edit(format!("\"{name}\" is not a valid variable name")));
            }
            // Not `AlreadyExists`: callers read that as the request's
            // own name taken.
            if varmodel::is_reserved(name) || self.variable_exists(name)? {
                return Err(edit(format!("a variable named \"{name}\" already exists")));
            }
        }
        Ok(())
    }

    /// The writes of a checked `secrets`, inside the caller's transaction.
    fn write_new_secrets(&mut self, secrets: &[(String, Option<String>)]) -> Result<(), Error> {
        let env = self.env_key();
        // A value an earlier declaration of the name left in the secrets
        // file is not the paste's: the new secret holds the paste's value
        // or none, in every environment.
        let mut changed = false;
        for values in self.secrets.values_mut() {
            for (name, _) in secrets {
                changed |= values.shift_remove(name).is_some();
            }
        }
        for (name, value) in secrets {
            if let Some(value) = value {
                self.secrets.entry(env.clone()).or_default().insert(name.clone(), value.clone());
                changed = true;
            }
        }
        // One write of each file; declaring re-resolves with the secrets
        // already in place.
        if changed {
            self.write_secrets_journaled()?;
        }
        self.edit_variables(|doc| {
            secrets.iter().try_fold(doc.to_string(), |doc, (name, _)| {
                let declared = varedit::upsert_var(&doc, name, None, None)?;
                varedit::set_secret_flag(&declared, name, true)
            })
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
        std::fs::write(root.join("project.toml"), "name = \"Vars\"\nspaces = [\"main\"]\n")
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
            p.apply_var_edit(&VarEdit::SetDescription { owner: "nope".into(), value: "x".into() })
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
        p.apply_var_edit(&VarEdit::Rename { from: "base_url".into(), to: "root_url".into() })
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
            p.apply_var_edit(&VarEdit::Delete { name: "left".into() }).unwrap_err().to_string(),
            "variable \"left\" is a field of selector \"pair\"; remove it from the selector first"
        );
    }

    #[test]
    fn toggle_secret_moves_env_values_into_secrets_and_back_on_undo() {
        let (dir, mut p) = vars_fixture();
        std::fs::write(dir.path().join("environments/dev.toml"), "base_url = \"d\"\n").unwrap();
        p.invalidate_stamps();
        p.poll();
        p.apply_var_edit(&VarEdit::ToggleSecret { name: "base_url".into() }).unwrap();
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
    fn a_request_created_with_secrets_declares_and_sets_them_in_one_entry() {
        let (dir, mut p) = vars_fixture();
        p.set_active_env(Some("dev".into()));
        let req = HttpRequest::from_toml_str("url = \"https://x/{{TOKEN}}\"\n").unwrap();
        let secrets =
            vec![("TOKEN".to_string(), Some("abc123".to_string())), ("KEY".to_string(), None)];
        let (slug, leaf) = p.create_request_with_secrets("main/GET me", req, &secrets).unwrap();
        assert_eq!((slug.as_str(), leaf.as_str()), ("main/get-me", "GET me"));
        assert_eq!(p.journal_len(), 1, "one undo entry");
        assert!(p.variables().vars["TOKEN"].secret);
        assert_eq!(p.secrets()["dev"]["TOKEN"], "abc123");
        assert_eq!(p.resolved().values.get("TOKEN").map(String::as_str), Some("abc123"));
        assert!(
            !read(&dir, "variables.toml").unwrap().contains("abc123"),
            "the value stays out of the project files"
        );
        // No value: declared, and missing until someone fills it in.
        assert!(p.variables().vars["KEY"].secret);
        assert_eq!(p.resolved().meta.get("KEY"), Some(&varmodel::VarMeta::MissingSecret));
        p.undo().unwrap();
        assert!(!p.request_exists("main/get-me"));
        assert!(!p.variables().vars.contains_key("TOKEN"));
        assert!(!p.variables().vars.contains_key("KEY"));
        assert!(p.secrets().get("dev").and_then(|m| m.get("TOKEN")).is_none());
    }

    #[test]
    fn a_request_created_with_secrets_writes_each_file_once() {
        let (_dir, mut p) = vars_fixture();
        p.set_active_env(Some("dev".into()));
        let req = HttpRequest::from_toml_str("url = \"https://x/{{A}}/{{B}}/{{C}}\"\n").unwrap();
        let secrets = vec![
            ("A".to_string(), Some("1".to_string())),
            ("B".to_string(), Some("2".to_string())),
            ("C".to_string(), None),
        ];
        p.create_request_with_secrets("main/x", req, &secrets).unwrap();
        assert_eq!(p.secrets()["dev"]["B"], "2");
        assert_eq!(p.resolved().values.get("A").map(String::as_str), Some("1"));
        let e = p.journal.pop_undo().unwrap();
        let writes = |file: &str| {
            e.ops
                .iter()
                .filter(|op| matches!(op, Op::Text { path, .. } if path.as_str() == file))
                .count()
        };
        assert_eq!((writes(VARIABLES_TOML), writes(SECRETS_TOML)), (1, 1));
    }

    #[test]
    fn a_value_left_from_a_deleted_declaration_is_not_the_imports() {
        let (_dir, mut p) = vars_fixture();
        p.set_active_env(Some("dev".into()));
        // Deleting a secret's declaration leaves its values behind.
        for env in ["dev", "qa"] {
            p.set_secret_for(env, "KEY", "old".into()).unwrap();
            p.set_secret_for(env, "TOKEN", "old".into()).unwrap();
        }
        let before = p.journal_len();
        let req = HttpRequest::from_toml_str("url = \"https://x/{{TOKEN}}/{{KEY}}\"\n").unwrap();
        let secrets =
            vec![("TOKEN".to_string(), Some("new".to_string())), ("KEY".to_string(), None)];
        p.create_request_with_secrets("main/GET me", req, &secrets).unwrap();
        assert_eq!(p.journal_len(), before + 1, "one undo entry");
        assert_eq!(
            p.resolved().meta.get("KEY"),
            Some(&varmodel::VarMeta::MissingSecret),
            "no value, as the summary says"
        );
        assert_eq!(p.secrets()["dev"].get("TOKEN").map(String::as_str), Some("new"));
        assert!(!p.secrets()["qa"].contains_key("KEY") && !p.secrets()["qa"].contains_key("TOKEN"));
        p.undo().unwrap();
        for env in ["dev", "qa"] {
            assert_eq!(p.secrets()[env].get("KEY").map(String::as_str), Some("old"), "{env}");
            assert_eq!(p.secrets()[env].get("TOKEN").map(String::as_str), Some("old"), "{env}");
        }
    }

    #[test]
    fn secrets_declared_alone_are_one_entry_and_refuse_a_taken_name() {
        let (dir, mut p) = vars_fixture();
        p.set_active_env(Some("dev".into()));
        p.set_secret_for("qa", "KEY", "old".into()).unwrap();
        let before = p.journal_len();
        let secrets =
            vec![("TOKEN".to_string(), Some("abc123".to_string())), ("KEY".to_string(), None)];
        p.declare_secrets(&secrets).unwrap();
        assert_eq!(p.journal_len(), before + 1, "one undo entry");
        assert!(p.variables().vars["TOKEN"].secret && p.variables().vars["KEY"].secret);
        assert_eq!(p.secrets()["dev"]["TOKEN"], "abc123");
        assert!(!p.secrets()["qa"].contains_key("KEY"), "a leftover value is not the paste's");
        assert_eq!(p.resolved().meta.get("KEY"), Some(&varmodel::VarMeta::MissingSecret));
        assert!(!read(&dir, "variables.toml").unwrap().contains("abc123"));
        p.undo().unwrap();
        assert!(!p.variables().vars.contains_key("TOKEN"));
        assert!(!p.variables().vars.contains_key("KEY"));
        assert_eq!(p.secrets()["qa"].get("KEY").map(String::as_str), Some("old"));
        // Taken, reserved and invalid names refuse before anything lands.
        let len = p.journal_len();
        for name in ["base_url", "entries", "bad name"] {
            let err = p.declare_secrets(&[(name.to_string(), None)]).unwrap_err();
            assert!(matches!(err, Error::Edit(_)), "{name}: {err:?}");
        }
        assert_eq!(p.journal_len(), len);
        p.declare_secrets(&[]).unwrap();
        assert_eq!(p.journal_len(), len, "nothing to declare journals nothing");
    }

    #[test]
    fn a_name_the_project_has_is_refused_and_nothing_lands() {
        let (dir, mut p) = vars_fixture();
        // Only qa sets it, undeclared: `resolve_env` reads it there.
        std::fs::write(
            dir.path().join("environments/qa.toml"),
            "TOKEN = \"real\"\n\n[options.creds.alice]\nuser_id = \"1\"\ncustomer_id = \"2\"\n",
        )
        .unwrap();
        let req = HttpRequest::from_toml_str("url = \"https://x\"\n").unwrap();
        for name in ["base_url", "creds", "user_id", "TOKEN", "entries", "bad name"] {
            assert_eq!(
                p.variable_exists(name).unwrap(),
                !matches!(name, "entries" | "bad name"),
                "{name}"
            );
            let secrets = vec![(name.to_string(), Some("v".to_string()))];
            let err =
                p.create_request_with_secrets("main/GET x", req.clone(), &secrets).unwrap_err();
            assert!(
                matches!(err, Error::Edit(_)),
                "{name}: never read as the request's name taken: {err:?}"
            );
        }
        assert!(!p.variable_exists("OTHER").unwrap());
        // What an unreadable environment sets is unknown: refused, not guessed.
        std::fs::write(dir.path().join("environments/qa.toml"), "TOKEN = [").unwrap();
        assert!(p.variable_exists("OTHER").is_err());
        let secrets = vec![("OTHER".to_string(), None)];
        assert!(p.create_request_with_secrets("main/GET x", req.clone(), &secrets).is_err());
        assert!(!p.request_exists("main/get-x"));
        assert_eq!(p.journal_len(), 0);
    }

    #[test]
    fn a_selector_may_take_a_field_name() {
        let (_dir, mut p) = vars_fixture();
        p.apply_var_edit(&VarEdit::NewSelector {
            name: "customer_id".into(),
            fields: vec!["c".into()],
            shared: false,
        })
        .unwrap();
        p.apply_var_edit(&VarEdit::Rename { from: "creds".into(), to: "user_id".into() }).unwrap();
        assert!(p.variables().selectors.contains_key("user_id"));
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
        assert!(
            read(&dir, "environments/qa.toml").unwrap().contains("[options.creds.\"alice copy\"]")
        );
        p.apply_var_edit(&paste).unwrap();
        let qa = read(&dir, "environments/qa.toml").unwrap();
        assert!(qa.contains("[options.creds.\"alice copy-2\"]"), "{qa}");
        // The row it collided with is still there, unchanged.
        assert!(qa.contains("[options.creds.alice]"), "{qa}");
    }

    fn creds_values(user_id: &str, customer_id: &str) -> IndexMap<String, String> {
        IndexMap::from([
            ("user_id".to_string(), user_id.to_string()),
            ("customer_id".to_string(), customer_id.to_string()),
        ])
    }

    /// Task #196: `NewOption` creates, so an option the file already holds
    /// is refused, nothing is written and no journal entry lands. It used
    /// to write the new values over the existing option's.
    #[test]
    fn new_option_refuses_a_name_the_file_already_holds() {
        let (dir, mut p) = vars_fixture();
        let before = read(&dir, "environments/qa.toml").unwrap();
        let err = p
            .apply_var_edit(&VarEdit::NewOption {
                env: "qa".into(),
                selector: "creds".into(),
                name: "alice".into(),
                description: None,
                values: creds_values("", ""),
            })
            .unwrap_err();
        assert_eq!(err.to_string(), "option \"alice\" already exists on creds");
        assert_eq!(read(&dir, "environments/qa.toml").unwrap(), before);
        assert_eq!(p.journal_len(), 0);
    }

    /// The refusal reads the file the write reads, not the loaded options:
    /// an option another program added since the project was read is
    /// refused too, and keeps its values.
    #[test]
    fn new_option_refuses_an_option_added_outside_since_the_last_read() {
        let (dir, mut p) = vars_fixture();
        p.set_active_env(Some("qa".into()));
        let outside = "[options.creds.alice]\nuser_id = \"1\"\ncustomer_id = \"2\"\n\n[options.creds.carol]\nuser_id = \"7\"\ncustomer_id = \"8\"\n";
        std::fs::write(dir.path().join("environments/qa.toml"), outside).unwrap();
        assert!(!p.env_data().options["creds"].contains_key("carol"), "not reloaded yet");
        let err = p
            .apply_var_edit(&VarEdit::NewOption {
                env: "qa".into(),
                selector: "creds".into(),
                name: "carol".into(),
                description: None,
                values: creds_values("", ""),
            })
            .unwrap_err();
        assert_eq!(err.to_string(), "option \"carol\" already exists on creds");
        assert_eq!(read(&dir, "environments/qa.toml").unwrap(), outside);
    }

    /// A paste picks its name from the loaded options; one another program
    /// took since is refused by the same create, not written over.
    #[test]
    fn paste_option_refuses_a_name_taken_outside_since_the_last_read() {
        let (dir, mut p) = vars_fixture();
        // Active, so the paste picks its name from the loaded options.
        p.set_active_env(Some("qa".into()));
        let outside = "[options.creds.alice]\nuser_id = \"1\"\ncustomer_id = \"2\"\n\n[options.creds.bob]\nuser_id = \"7\"\ncustomer_id = \"8\"\n";
        std::fs::write(dir.path().join("environments/qa.toml"), outside).unwrap();
        assert!(!p.env_data().options["creds"].contains_key("bob"), "not reloaded yet");
        let err = p
            .apply_var_edit(&VarEdit::PasteOption {
                env: "qa".into(),
                selector: "creds".into(),
                name: "bob".into(),
                description: None,
                values: creds_values("", ""),
            })
            .unwrap_err();
        assert_eq!(err.to_string(), "option \"bob\" already exists on creds");
        assert_eq!(read(&dir, "environments/qa.toml").unwrap(), outside);
    }
}

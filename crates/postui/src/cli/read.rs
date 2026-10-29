//! The read commands' reports: what each one finds, as the `Serialize`
//! shape its JSON takes, and its text rendering, which carries the same
//! facts with paths relative to the root it names once (R19, R20). Every
//! project is opened read-only ([`open`]), so nothing here can write (R16).

use super::out::{count, fields, rel, table, table_under};
use super::{CliError, Global, target};
use indexmap::IndexMap;
use postui_core::model::Method;
use postui_core::prepare::SECRET_MASK;
use postui_core::project::{Project, SECRETS_TOML, Warning};
use postui_core::varmodel::{self, VarMeta};
use postui_core::{order, storage};
use serde::Serialize;
use std::path::{Path, PathBuf};

/// Opens `root` read-only. A refused open is the error, naming the file
/// that refused it. For `secrets.toml` the reason is withheld: a parser's
/// message quotes the file's text, and that text is secret values.
pub(super) fn open(root: &Path) -> Result<(Project, Vec<Warning>), CliError> {
    Project::open_read_only(root.to_path_buf()).map_err(|e| {
        let file = (!e.file.is_empty()).then(|| e.root.join(&e.file));
        let message = if e.file == SECRETS_TOML {
            "could not be read or parsed (details withheld: the file holds secret values)"
                .to_string()
        } else {
            e.error
        };
        CliError { message, file }
    })
}

/// Makes `env` (a slug, else a display name, exact before case-insensitive)
/// the project's environment for this run, and gives its slug; `None` keeps
/// the one the open chose (the TUI's: its saved one, else the first). The
/// result is `None` when the project has no environments. A missing
/// environment, or one whose file doesn't parse, is the error. Nothing is
/// written: the project is read-only, so `set_active_env`'s persist of the
/// choice is refused and ignored.
pub(super) fn pick_env(
    project: &mut Project,
    env: Option<&str>,
) -> Result<Option<String>, CliError> {
    let Some(wanted) = env else {
        return Ok(project.active_env().map(str::to_string));
    };
    let slugs = project.environments().to_vec();
    let named = |slug: &String| project.env_name(slug);
    let slug = slugs
        .iter()
        .find(|s| *s == wanted)
        .or_else(|| slugs.iter().find(|s| named(s) == wanted))
        .or_else(|| {
            slugs
                .iter()
                .find(|s| s.eq_ignore_ascii_case(wanted) || named(s).eq_ignore_ascii_case(wanted))
        })
        .cloned()
        .ok_or_else(|| {
            CliError::new(if slugs.is_empty() {
                format!("no such environment \"{wanted}\"; this project has none")
            } else {
                format!(
                    "no such environment \"{wanted}\"; the environments are {}",
                    slugs.join(", ")
                )
            })
        })?;
    if let Err(e) = project.load_environment(&slug) {
        return Err(match e {
            postui_core::project::Error::Parse { file, error } => {
                CliError::at(format!("environment \"{slug}\": {error}"), project.root().join(file))
            }
            other => CliError::new(format!("environment \"{slug}\": {other}")),
        });
    }
    // The file just parsed, so this loads it too; a failure now is a race.
    if let Some(warning) = project.set_active_env(Some(slug.clone())).into_iter().next() {
        return Err(CliError::new(warning));
    }
    Ok(Some(slug))
}

/// `postui projects`.
#[derive(Serialize)]
pub struct ProjectsReport {
    config_dir: Option<PathBuf>,
    projects: Vec<KnownProject>,
}

#[derive(Serialize)]
struct KnownProject {
    name: String,
    root: PathBuf,
    exists: bool,
}

/// The registered projects, in the registry's order.
pub fn projects() -> Result<ProjectsReport, CliError> {
    let projects = target::registered()?
        .into_iter()
        .map(|root| KnownProject {
            name: Project::peek_display_name(&root),
            exists: root.is_dir(),
            root,
        })
        .collect();
    Ok(ProjectsReport { config_dir: postui_core::config_dir(), projects })
}

impl ProjectsReport {
    pub fn text(&self) -> String {
        let config_dir = match &self.config_dir {
            Some(dir) => dir.display().to_string(),
            None => "(none)".to_string(),
        };
        let rows: Vec<Vec<String>> = self
            .projects
            .iter()
            .map(|p| {
                let missing = if p.exists { "" } else { "(missing)" };
                vec![p.name.clone(), p.root.display().to_string(), missing.to_string()]
            })
            .collect();
        let listed = if rows.is_empty() { "  (none)\n".to_string() } else { table(&rows) };
        format!("config dir  {config_dir}\n\nprojects\n{listed}")
    }
}

/// `postui show`.
#[derive(Serialize)]
pub struct ShowReport {
    name: String,
    root: PathBuf,
    files: ShowFiles,
    environments: Vec<ShownEnv>,
    spaces: Vec<ShownSpace>,
    warnings: Vec<String>,
}

#[derive(Serialize)]
struct ShowFiles {
    project: PathBuf,
    variables: PathBuf,
}

#[derive(Serialize)]
struct ShownEnv {
    slug: String,
    name: String,
    file: PathBuf,
    /// The environment the TUI would open on.
    active: bool,
}

#[derive(Serialize)]
struct ShownSpace {
    slug: String,
    name: String,
    dir: PathBuf,
    requests: usize,
}

/// The project's name, files, environments (a broken inactive one is
/// listed all the same) and spaces, and the warnings opening it gave.
pub fn show(g: &Global) -> Result<ShowReport, CliError> {
    let root = target::resolve(g.project.as_deref())?;
    let (project, mut warnings) = open(&root)?;
    warnings.extend(project.listing_warning());
    let environments = project
        .environments()
        .iter()
        .map(|slug| ShownEnv {
            slug: slug.clone(),
            name: project.env_name(slug),
            file: env_file(&root, slug),
            active: project.active_env() == Some(slug.as_str()),
        })
        .collect();
    let spaces = project
        .spaces()
        .iter()
        .map(|slug| ShownSpace {
            slug: slug.clone(),
            name: project.space_name(slug),
            dir: root.join("requests").join(slug),
            requests: displayed(&project, slug).len(),
        })
        .collect();
    Ok(ShowReport {
        name: project.display_name(),
        files: ShowFiles {
            project: root.join("project.toml"),
            variables: root.join("variables.toml"),
        },
        root,
        environments,
        spaces,
        warnings,
    })
}

impl ShowReport {
    pub fn text(&self) -> String {
        let root = &self.root;
        let mut out = fields(&[
            ("name", self.name.clone()),
            ("root", root.display().to_string()),
            ("project", rel(root, &self.files.project)),
            ("variables", rel(root, &self.files.variables)),
        ]);
        let envs: Vec<Vec<String>> = self
            .environments
            .iter()
            .map(|e| {
                let active = if e.active { "(active)" } else { "" };
                vec![e.slug.clone(), e.name.clone(), active.to_string(), rel(root, &e.file)]
            })
            .collect();
        out.push_str("\nenvironments\n");
        out.push_str(&if envs.is_empty() { "  (none)\n".to_string() } else { table(&envs) });
        let spaces: Vec<Vec<String>> = self
            .spaces
            .iter()
            .map(|s| {
                vec![
                    s.slug.clone(),
                    s.name.clone(),
                    count(s.requests, "request"),
                    rel(root, &s.dir),
                ]
            })
            .collect();
        out.push_str("\nspaces\n");
        out.push_str(&if spaces.is_empty() { "  (none)\n".to_string() } else { table(&spaces) });
        out.push_str(&warnings_text(&self.warnings));
        out
    }
}

/// `postui init`.
#[derive(Serialize)]
pub struct InitReport {
    root: PathBuf,
    files: InitFiles,
    warnings: Vec<String>,
}

#[derive(Serialize)]
struct InitFiles {
    project: PathBuf,
    variables: PathBuf,
    environments: PathBuf,
    requests: PathBuf,
}

/// Makes `dir` (and any missing parents) a project. The one command that
/// writes, and the one that doesn't register the project in the config. A
/// directory that already holds a `project.toml` is refused untouched.
pub fn init(dir: &Path, name: Option<&str>) -> Result<InitReport, CliError> {
    let dir = std::path::absolute(dir)
        .map_err(|e| CliError::new(format!("can't resolve \"{}\": {e}", dir.display())))?;
    if Project::is_project(&dir) {
        return Err(CliError::at("already a postui project", dir.join("project.toml")));
    }
    let (_, warnings) = Project::init(&dir, name)
        .map_err(|e| CliError { message: e.to_string(), file: Some(dir.clone()) })?;
    // Canonical like every other command's root, which needs it to exist.
    let root = dir
        .canonicalize()
        .map_err(|e| CliError::new(format!("can't resolve \"{}\": {e}", dir.display())))?;
    Ok(InitReport {
        files: InitFiles {
            project: root.join("project.toml"),
            variables: root.join("variables.toml"),
            environments: root.join("environments"),
            requests: root.join("requests"),
        },
        root,
        warnings,
    })
}

impl InitReport {
    pub fn text(&self) -> String {
        let root = &self.root;
        let mut out = fields(&[
            ("root", root.display().to_string()),
            ("project", rel(root, &self.files.project)),
            ("variables", rel(root, &self.files.variables)),
            ("environments", rel(root, &self.files.environments)),
            ("requests", rel(root, &self.files.requests)),
        ]);
        out.push_str(&warnings_text(&self.warnings));
        out
    }
}

/// A `warnings` section for the text; nothing when there are none.
pub(super) fn warnings_text(warnings: &[String]) -> String {
    if warnings.is_empty() {
        return String::new();
    }
    let rows: Vec<Vec<String>> = warnings.iter().map(|w| vec![w.clone()]).collect();
    format!("\nwarnings\n{}", table(&rows))
}

/// `space`'s requests as full slugs, in the order the sidebar draws them.
pub(super) fn displayed(project: &Project, space: &str) -> Vec<String> {
    order::displayed_slugs(project.requests(), order::space_order(project.meta(), space), space)
}

/// The spaces a `--space` names: that one, or every space in project order
/// when `None`. A space the project doesn't have is the error.
pub(super) fn spaces_of(project: &Project, space: Option<&str>) -> Result<Vec<String>, CliError> {
    match space {
        None => Ok(project.spaces().to_vec()),
        Some(wanted) if project.spaces().iter().any(|s| s == wanted) => {
            Ok(vec![wanted.to_string()])
        }
        Some(wanted) => Err(CliError::new(format!("no such space \"{wanted}\" in this project"))),
    }
}

/// `postui requests`.
#[derive(Serialize)]
pub struct RequestsReport {
    requests: Vec<ListedRequest>,
    warnings: Vec<String>,
    /// What the text's paths are relative to; the JSON's are absolute.
    #[serde(skip)]
    root: PathBuf,
}

#[derive(Serialize)]
struct ListedRequest {
    slug: String,
    space: String,
    name: Option<String>,
    method: Option<Method>,
    url: Option<String>,
    file: PathBuf,
    error: Option<String>,
}

/// The requests of `space` (every space when `None`), spaces in project
/// order, each in sidebar order. A file that doesn't parse is listed with
/// its error.
pub fn requests(g: &Global, space: Option<&str>) -> Result<RequestsReport, CliError> {
    let root = target::resolve(g.project.as_deref())?;
    let (mut project, mut warnings) = open(&root)?;
    warnings.extend(project.listing_warning());
    let spaces = spaces_of(&project, space)?;
    let mut requests = Vec::new();
    for space in spaces {
        for slug in displayed(&project, &space) {
            let (name, broken) = project
                .requests()
                .iter()
                .find(|l| l.slug == slug)
                .map(|l| (l.display_name().to_string(), l.broken.clone()))
                .unwrap_or_default();
            let parsed = match broken {
                Some(error) => Err(error),
                None => project.read_request(&slug).map_err(|e| e.to_string()),
            };
            let file = storage::request_path(&root, &slug);
            requests.push(match parsed {
                Ok(req) => ListedRequest {
                    slug,
                    space: space.clone(),
                    name: Some(name),
                    method: Some(req.method),
                    url: Some(req.url),
                    file,
                    error: None,
                },
                Err(error) => ListedRequest {
                    slug,
                    space: space.clone(),
                    name: None,
                    method: None,
                    url: None,
                    file,
                    error: Some(error),
                },
            });
        }
    }
    Ok(RequestsReport { requests, warnings, root })
}

impl RequestsReport {
    /// The root, then a row a request: slug, method, URL, name and file; a
    /// broken one's error goes under its row.
    pub fn text(&self) -> String {
        let rows: Vec<(Vec<String>, &str)> = self
            .requests
            .iter()
            .map(|r| {
                let file = rel(&self.root, &r.file);
                match &r.error {
                    Some(error) => (
                        vec![r.slug.clone(), "broken".into(), String::new(), String::new(), file],
                        error.as_str(),
                    ),
                    None => (
                        vec![
                            r.slug.clone(),
                            r.method.map_or("", Method::as_str).to_string(),
                            r.url.clone().unwrap_or_default(),
                            r.name.clone().unwrap_or_default(),
                            file,
                        ],
                        "",
                    ),
                }
            })
            .collect();
        let listed = if rows.is_empty() { "  (none)\n".to_string() } else { table_under(&rows) };
        format!(
            "{}{listed}{}",
            fields(&[("root", self.root.display().to_string())]),
            warnings_text(&self.warnings)
        )
    }
}

/// `postui vars`.
#[derive(Serialize)]
pub struct VarsReport {
    environment: Option<VarsEnv>,
    files: VarsFiles,
    variables: Vec<VarEntry>,
    selectors: Vec<SelectorEntry>,
    warnings: Vec<String>,
    /// What the text's paths are relative to; the JSON's are absolute.
    #[serde(skip)]
    root: PathBuf,
}

/// `postui vars <name>`: one variable, or one selector with its options in
/// full; the other is `null`.
#[derive(Serialize)]
pub struct VarReport {
    environment: Option<VarsEnv>,
    files: VarsFiles,
    variable: Option<VarEntry>,
    selector: Option<SelectorDetail>,
    warnings: Vec<String>,
    #[serde(skip)]
    root: PathBuf,
}

#[derive(Serialize)]
struct VarsEnv {
    slug: String,
    name: String,
    file: PathBuf,
}

#[derive(Serialize)]
struct VarsFiles {
    variables: PathBuf,
}

#[derive(Serialize)]
struct VarEntry {
    name: String,
    description: Option<String>,
    /// `null` for `needs_selection`, `missing_secret` and `unset`.
    value: Option<String>,
    source: &'static str,
    secret: bool,
    /// The selector a field belongs to.
    selector: Option<String>,
}

/// A selector as `vars` lists it: its options by name, the name
/// `--select` takes; `vars <name>` gives them in full.
#[derive(Serialize)]
struct SelectorEntry {
    name: String,
    description: Option<String>,
    shared: bool,
    fields: Vec<String>,
    options: Vec<String>,
    /// The option picked, `null` when none (or a stale pick) is.
    selected: Option<String>,
}

/// A selector as `vars <name>` gives it.
#[derive(Serialize)]
struct SelectorDetail {
    name: String,
    description: Option<String>,
    shared: bool,
    fields: Vec<String>,
    /// The file its options are written in: `variables.toml` for a shared
    /// selector, else the environment's (`null` with no environment).
    file: Option<PathBuf>,
    options: Vec<OptionEntry>,
    selected: Option<String>,
}

#[derive(Serialize)]
struct OptionEntry {
    name: String,
    description: Option<String>,
    values: IndexMap<String, String>,
}

/// What `vars` and `vars <name>` both report: every selector in full.
struct Gathered {
    root: PathBuf,
    environment: Option<VarsEnv>,
    files: VarsFiles,
    variables: Vec<VarEntry>,
    selectors: Vec<SelectorDetail>,
    warnings: Vec<String>,
}

/// Every variable's value in an environment and where it comes from, and
/// every selector with its options' names and the one selected. A secret's
/// value is masked unless `show_secrets`. A project with no environments
/// gives `environment: null` and resolves against nothing.
pub fn vars(g: &Global, env: Option<&str>, show_secrets: bool) -> Result<VarsReport, CliError> {
    let all = gather(g, env, show_secrets)?;
    let selectors = all
        .selectors
        .into_iter()
        .map(|s| SelectorEntry {
            name: s.name,
            description: s.description,
            shared: s.shared,
            fields: s.fields,
            options: s.options.into_iter().map(|o| o.name).collect(),
            selected: s.selected,
        })
        .collect();
    Ok(VarsReport {
        environment: all.environment,
        files: all.files,
        variables: all.variables,
        selectors,
        warnings: all.warnings,
        root: all.root,
    })
}

/// [`vars`] for the one variable or selector called `name`, a selector with
/// its options' values. `variables.toml` refuses a name that is both. A
/// name that is neither is the error, naming the choices.
pub fn var(
    g: &Global,
    env: Option<&str>,
    show_secrets: bool,
    name: &str,
) -> Result<VarReport, CliError> {
    let Gathered { root, environment, files, mut variables, mut selectors, warnings } =
        gather(g, env, show_secrets)?;
    let variable = variables.iter().position(|v| v.name == name).map(|i| variables.remove(i));
    let selector = selectors.iter().position(|s| s.name == name).map(|i| selectors.remove(i));
    if variable.is_none() && selector.is_none() {
        let names = |list: Vec<&str>| {
            if list.is_empty() { "(none)".to_string() } else { list.join(", ") }
        };
        return Err(CliError::new(format!(
            "no variable or selector \"{name}\"; the variables are {}; the selectors are {}",
            names(variables.iter().map(|v| v.name.as_str()).collect()),
            names(selectors.iter().map(|s| s.name.as_str()).collect()),
        )));
    }
    Ok(VarReport { environment, files, variable, selector, warnings, root })
}

fn gather(g: &Global, env: Option<&str>, show_secrets: bool) -> Result<Gathered, CliError> {
    let root = target::resolve(g.project.as_deref())?;
    let (mut project, mut warnings) = open(&root)?;
    warnings.extend(project.listing_warning());
    let slug = pick_env(&mut project, env)?;

    let model = project.variables();
    let resolved = project.resolved();
    let variables = resolved
        .meta
        .iter()
        .map(|(name, meta)| {
            let decl = model.vars.get(name);
            let owner = || {
                model
                    .selectors
                    .iter()
                    .find(|(_, d)| d.fields.contains(name))
                    .map(|(n, _)| n.clone())
            };
            let (source, selector) = match meta {
                VarMeta::Simple if project.env_data().values.contains_key(name) => ("env", None),
                VarMeta::Simple if decl.is_some_and(|d| d.default.is_some()) => ("default", None),
                VarMeta::Simple => ("unset", None),
                VarMeta::SelectorMember { selector, .. } => ("selector", Some(selector.clone())),
                VarMeta::Secret => ("secret", None),
                VarMeta::NeedsSelection => ("needs_selection", owner()),
                VarMeta::MissingSecret => ("missing_secret", None),
            };
            let secret = matches!(meta, VarMeta::Secret | VarMeta::MissingSecret);
            let value = resolved
                .values
                .get(name)
                .map(|v| if secret && !show_secrets { SECRET_MASK.to_string() } else { v.clone() });
            VarEntry {
                name: name.clone(),
                description: decl.and_then(|d| d.description.clone()),
                value,
                source,
                secret,
                selector,
            }
        })
        .collect();

    let key = slug.as_deref().unwrap_or_default();
    let selectors = model
        .selectors
        .iter()
        .map(|(name, decl)| {
            let options = varmodel::options_of(model, project.env_data(), name);
            let picked = if decl.shared {
                project.local().shared_selections.get(name)
            } else {
                project.selections_for(key).get(name)
            };
            // Where `options` came from: the same shared-or-environment
            // choice `varmodel::options_of` makes; change both together.
            let file = if decl.shared {
                Some(root.join("variables.toml"))
            } else {
                slug.as_ref().map(|slug| env_file(&root, slug))
            };
            SelectorDetail {
                name: name.clone(),
                description: decl.description.clone(),
                shared: decl.shared,
                fields: decl.fields.clone(),
                file,
                options: options
                    .into_iter()
                    .flatten()
                    .map(|(option, o)| OptionEntry {
                        name: option.clone(),
                        description: o.description.clone(),
                        values: o.values.clone(),
                    })
                    .collect(),
                selected: picked.filter(|p| options.is_some_and(|o| o.contains_key(*p))).cloned(),
            }
        })
        .collect();

    Ok(Gathered {
        environment: slug.map(|slug| VarsEnv {
            name: project.env_name(&slug),
            file: env_file(&root, &slug),
            slug,
        }),
        files: VarsFiles { variables: root.join("variables.toml") },
        variables,
        selectors,
        warnings,
        root,
    })
}

/// Where environment `slug` is written.
pub(super) fn env_file(root: &Path, slug: &str) -> PathBuf {
    root.join("environments").join(format!("{slug}.toml"))
}

/// The lines `vars` and `vars <name>` start with: the root, the
/// environment and `variables.toml`.
fn vars_head(root: &Path, environment: &Option<VarsEnv>, files: &VarsFiles) -> String {
    let env = match environment {
        Some(e) => format!("{}  {}  {}", e.slug, e.name, rel(root, &e.file)),
        None => "(none)".to_string(),
    };
    fields(&[
        ("root", root.display().to_string()),
        ("environment", env),
        ("variables", rel(root, &files.variables)),
    ])
}

/// A variable as a row: name, source (with its selector), value, description.
fn var_row(v: &VarEntry) -> Vec<String> {
    let source = match &v.selector {
        Some(selector) => format!("{} {selector}", v.source),
        None => v.source.to_string(),
    };
    vec![
        v.name.clone(),
        source,
        v.value.clone().unwrap_or_else(|| "-".to_string()),
        v.description.clone().unwrap_or_default(),
    ]
}

/// `rows` as a table, or `(none)`.
fn table_or_none(rows: &[Vec<String>]) -> String {
    if rows.is_empty() { "  (none)\n".to_string() } else { table(rows) }
}

impl VarsReport {
    /// The head, a row a variable, then a row a selector with its options'
    /// names.
    pub fn text(&self) -> String {
        let mut out = vars_head(&self.root, &self.environment, &self.files);
        let rows: Vec<Vec<String>> = self.variables.iter().map(var_row).collect();
        out.push_str("\nvariables\n");
        out.push_str(&table_or_none(&rows));
        let rows: Vec<Vec<String>> = self
            .selectors
            .iter()
            .map(|s| {
                let options =
                    if s.options.is_empty() { "(none)".to_string() } else { s.options.join(", ") };
                vec![
                    s.name.clone(),
                    format!("selected {}", s.selected.as_deref().unwrap_or("-")),
                    format!("fields {}", s.fields.join(", ")),
                    format!("options {options}"),
                    if s.shared { "shared".to_string() } else { String::new() },
                    s.description.clone().unwrap_or_default(),
                ]
            })
            .collect();
        out.push_str("\nselectors\n");
        out.push_str(&table_or_none(&rows));
        out.push_str(&warnings_text(&self.warnings));
        out
    }
}

impl VarReport {
    /// The head, then the variable's row, or the selector's facts and a row
    /// an option with its values.
    pub fn text(&self) -> String {
        let root = &self.root;
        let mut out = vars_head(root, &self.environment, &self.files);
        if let Some(v) = &self.variable {
            out.push_str("\nvariable\n");
            out.push_str(&table(&[var_row(v)]));
        }
        if let Some(s) = &self.selector {
            out.push('\n');
            let mut facts = vec![("selector", s.name.clone())];
            if let Some(description) = &s.description {
                facts.push(("description", description.clone()));
            }
            facts.extend([
                ("shared", if s.shared { "yes" } else { "no" }.to_string()),
                ("fields", s.fields.join(", ")),
                ("selected", s.selected.clone().unwrap_or_else(|| "-".to_string())),
                ("file", s.file.as_ref().map_or_else(|| "-".to_string(), |f| rel(root, f))),
            ]);
            out.push_str(&fields(&facts));
            out.push_str("\noptions\n");
            let rows: Vec<Vec<String>> = s
                .options
                .iter()
                .map(|o| {
                    // A column a field, in the selector's order, so the
                    // columns line up whatever order each option is written
                    // in. `varmodel` makes an option set every field and no
                    // other key, so the `-` is only a guard.
                    let mut row = vec![o.name.clone()];
                    row.extend(s.fields.iter().map(|field| {
                        format!("{field}={}", o.values.get(field).map_or("-", String::as_str))
                    }));
                    row.push(o.description.clone().unwrap_or_default());
                    row
                })
                .collect();
            out.push_str(&table_or_none(&rows));
        }
        out.push_str(&warnings_text(&self.warnings));
        out
    }
}

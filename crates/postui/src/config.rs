//! Global project registry stored in the app's `config.toml`, under the
//! `[projects]` table: known project paths (cycle order), an optional
//! configured root directory, and the last-used project.

use std::path::{Path, PathBuf};
use std::time::Duration;

/// The set of known projects plus the configured root and last-used project,
/// as stored under `[projects]` in the global config file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProjectsRegistry {
    pub known: Vec<PathBuf>,
    pub root: Option<PathBuf>,
    pub last: Option<PathBuf>,
}

/// The file name every `config.toml` message names, so a warning still
/// tells the user which file to go and fix.
const CONFIG_TOML: &str = "config.toml";
const KEYS_TOML: &str = "keys.toml";
const UI_TOML: &str = "ui.toml";
const THEMES_DIR: &str = "themes";

/// The one place the "`config.toml` will not parse" sentence is written.
/// What was *done* about it differs by caller — startup drops to the
/// defaults, a reload keeps what it has — so each caller appends its own
/// clause rather than the sentence being written out again.
fn config_parse_error(e: &toml::de::Error) -> String {
    format!("could not parse {CONFIG_TOML}: {e}")
}

impl ProjectsRegistry {
    /// Parses the registry out of `config.toml`'s text. An empty string
    /// (the missing file) is the empty registry; a mistyped piece of the
    /// `[projects]` table degrades to the default for that piece. Text
    /// that can't be parsed also yields the empty registry — there is
    /// nothing else to run on — but with a warning saying so, and every
    /// save then refuses to touch the file (see [`Config::edit`]).
    pub fn parse(text: &str) -> (Self, Vec<String>) {
        match toml::from_str::<toml::Value>(text) {
            Ok(value) => (Self::from_value(&value), Vec::new()),
            Err(e) => (Self::default(), vec![config_parse_error(&e)]),
        }
    }

    /// [`Self::parse`] on an already-parsed document. The form
    /// [`Config::read_all`] uses, so `config.toml` is parsed exactly once
    /// for this and [`UiSettings`] together instead of once each.
    pub fn from_value(value: &toml::Value) -> Self {
        let mut registry = Self::default();
        let Some(projects) = value.get("projects").and_then(|v| v.as_table()) else {
            return registry;
        };

        if let Some(known) = projects.get("known").and_then(|v| v.as_array()) {
            registry.known = known
                .iter()
                .filter_map(|v| v.as_str())
                .map(expand_tilde)
                .collect();
        }
        registry.root = projects
            .get("root")
            .and_then(|v| v.as_str())
            .map(expand_tilde);
        registry.last = projects
            .get("last")
            .and_then(|v| v.as_str())
            .map(expand_tilde);

        registry
    }

    /// Writes the registry into `doc`, touching only the `[projects]`
    /// table; every unrelated key is preserved byte-for-byte. The I/O
    /// around it (and the refusal to write over a file that doesn't
    /// parse) is [`Config::edit`]'s business.
    pub fn write_into(&self, doc: &mut toml_edit::DocumentMut) {
        let mut table = toml_edit::Table::new();

        let mut known = toml_edit::Array::new();
        for p in &self.known {
            known.push(p.to_string_lossy().into_owned());
        }
        table["known"] = toml_edit::value(known);

        match &self.root {
            Some(root) => table["root"] = toml_edit::value(root.to_string_lossy().into_owned()),
            None => {
                table.remove("root");
            }
        }
        match &self.last {
            Some(last) => table["last"] = toml_edit::value(last.to_string_lossy().into_owned()),
            None => {
                table.remove("last");
            }
        }

        doc["projects"] = toml_edit::Item::Table(table);
    }

    /// Registers `path` as known and as the last-used project. Dedups on
    /// path: if `path` is already known it is not re-pushed, but `last` is
    /// always updated.
    pub fn register(&mut self, path: PathBuf) {
        self.add_known(path.clone());
        self.last = Some(path);
    }

    /// Adds `path` to `known` if not already present. Does not touch
    /// `last` — for callers that must not commit to a project as current
    /// until some later gate (e.g. a dirty-editor confirm) resolves.
    pub fn add_known(&mut self, path: PathBuf) {
        if !self.known.contains(&path) {
            self.known.push(path);
        }
    }

    /// The configured root, or `~/postui-projects` if unset (falling back to
    /// `.` if the home directory can't be determined).
    pub fn default_root(&self) -> PathBuf {
        self.root.clone().unwrap_or_else(|| {
            directories::BaseDirs::new()
                .map(|dirs| dirs.home_dir().join("postui-projects"))
                .unwrap_or_else(|| PathBuf::from("."))
        })
    }

    /// The next project after `current` in cycle order, wrapping, skipping
    /// over entries that no longer exist on disk or that equal `current`.
    /// `None` when fewer than two projects are known or none qualify.
    pub fn next_after(&self, current: &Path) -> Option<PathBuf> {
        self.neighbor(current, 1)
    }

    /// The registered project `delta` steps (`1` next, `-1` previous)
    /// from `current`, wrapping and skipping roots that no longer exist
    /// on disk. `None` with fewer than two projects registered or none
    /// other than `current` present.
    pub fn neighbor(&self, current: &Path, delta: i32) -> Option<PathBuf> {
        if self.known.len() < 2 {
            return None;
        }
        let len = self.known.len() as i32;
        let pos = self.known.iter().position(|p| p == current);
        let start = match pos {
            Some(i) => (i as i32 + delta).rem_euclid(len) as usize,
            None => 0,
        };
        // Step onward in `delta`'s direction past any missing roots.
        let step_dir = if delta < 0 { -1 } else { 1 };
        for step in 0..self.known.len() {
            let idx = (start as i32 + step_dir * step as i32).rem_euclid(len) as usize;
            let candidate = &self.known[idx];
            if candidate != current && candidate.is_dir() {
                return Some(candidate.clone());
            }
        }
        None
    }
}

/// Config-tunable eased-transition durations for the motion catalog,
/// parsed from the optional `[animation_ms]` table in `config.toml` (each
/// key an integer count of milliseconds). Missing keys take these
/// defaults; an unknown key degrades to being ignored and is reported in
/// the returned warnings, the same posture `theme` uses for a bad value.
/// This table only tunes *how long* an eased transition takes; the
/// top-level `animations` bool on [`UiSettings`] remains the separate,
/// all-or-nothing kill switch for whether it eases at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AnimDurations {
    /// The tab-underline slide, e.g. switching between Params/Headers/Body.
    pub tab_slide: Duration,
    /// A hovered control's fill/edges easing in.
    pub hover: Duration,
    /// A focused control's fill/edges easing in.
    pub focus: Duration,
    /// A scrollable list's selection band sliding row-to-row.
    pub list_travel: Duration,
    /// A modal dialog's open transition.
    pub modal_open: Duration,
    /// A dropdown's open transition.
    pub dropdown_open: Duration,
    /// A collapsing pane's transition.
    pub pane_collapse: Duration,
    /// A toast's fade.
    pub toast: Duration,
    /// The in-flight Send button's breathe, per pole.
    pub send_breathe: Duration,
}

impl Default for AnimDurations {
    fn default() -> Self {
        Self {
            tab_slide: Duration::from_millis(250),
            hover: Duration::from_millis(70),
            focus: Duration::from_millis(90),
            list_travel: Duration::from_millis(100),
            modal_open: Duration::from_millis(100),
            dropdown_open: Duration::from_millis(90),
            pane_collapse: Duration::from_millis(120),
            toast: Duration::from_millis(100),
            send_breathe: Duration::from_millis(700),
        }
    }
}

/// What Tab does in the response pane's jq bar while a completion ghost
/// is showing (`jq_tab` in `config.toml`): list the candidates under the
/// bar and step through them, shell-style (`menu`, the default; `accept`
/// is accepted as an older name for it), or ghost the best one after the
/// caret and step through the rest in place (`cycle`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum JqTab {
    Cycle,
    #[default]
    Menu,
}

/// Mouse-first-GUI UI settings stored at the top level of `config.toml`:
/// the tiered clipboard's optional external command and the OSC 52 size
/// threshold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UiSettings {
    pub clipboard_cmd: Option<String>,
    pub osc52_limit: usize,
    /// The configured theme's registry name (e.g. `"terminal"`,
    /// `"gruvbox-dark"`, or a custom theme file's stem). Free-form: name
    /// validity is the registry's business at resolve time, not this
    /// loader's.
    pub theme: String,
    /// Whether eased transitions (tab underline, hover, modal open, ...)
    /// play, or every animated value jumps straight to its target.
    pub animations: bool,
    /// Whether a hovered button explains itself on the footer row (see
    /// `crate::hint`). Off leaves the footer's chips alone under the
    /// pointer -- the hints are a teaching aid, and a user who knows the
    /// app may prefer the chip row to stay put.
    pub hover_hints: bool,
    /// Per-transition durations, tunable via the optional `[animation_ms]`
    /// table; see [`AnimDurations`].
    pub anim_ms: AnimDurations,
    /// The shell command "Describe a filter…" runs, with the prompt piped
    /// in on stdin: `claude -p` by default. Only its first word (the
    /// program name) is looked up on PATH to gate the menu item.
    pub ai_cmd: String,
    /// Whether the user has already confirmed sending the response's shape
    /// to `ai_cmd` — set once via the "Always send" choice and persisted,
    /// so later requests skip the confirmation.
    pub ai_confirmed: bool,
    /// Tab's job in the jq bar while a completion ghost shows; see
    /// [`JqTab`].
    pub jq_tab: JqTab,
}

impl Default for UiSettings {
    fn default() -> Self {
        Self {
            clipboard_cmd: None,
            osc52_limit: 65536,
            theme: "terminal".into(),
            animations: true,
            hover_hints: true,
            anim_ms: AnimDurations::default(),
            ai_cmd: "claude -p".into(),
            ai_confirmed: false,
            jq_tab: JqTab::Menu,
        }
    }
}

impl UiSettings {
    /// Reads the top-level `clipboard_cmd` (string), `osc52_limit`
    /// (integer), `theme` (string), `animations` (bool), `hover_hints`
    /// (bool), `ai_cmd` (string), `ai_confirmed` (bool), and `jq_tab`
    /// (string) keys out of
    /// `config.toml`'s text. Never errors: an empty string (the missing
    /// file) or a mistyped key degrades that piece to its default. Text
    /// that can't be parsed leaves everything at its default too, but says
    /// so in the returned warnings — the user's settings didn't apply and
    /// they need to know why. `theme` is taken verbatim as a raw name
    /// string — whether it names a real registry entry is the registry's
    /// business at resolve time, not this parser's, so no warning is
    /// produced here for an unrecognized value.
    pub fn parse(text: &str) -> (UiSettings, Vec<String>) {
        match toml::from_str::<toml::Value>(text) {
            Ok(value) => Self::from_value(&value),
            Err(e) => (
                UiSettings::default(),
                vec![format!("{}; using default settings", config_parse_error(&e))],
            ),
        }
    }

    /// [`Self::parse`] on an already-parsed document — see
    /// [`ProjectsRegistry::from_value`] for why the parse is hoisted out.
    pub fn from_value(value: &toml::Value) -> (UiSettings, Vec<String>) {
        let mut settings = UiSettings::default();
        let mut warnings = Vec::new();

        if let Some(cmd) = value.get("clipboard_cmd").and_then(|v| v.as_str()) {
            settings.clipboard_cmd = Some(cmd.to_string());
        }
        if let Some(limit) = value.get("osc52_limit").and_then(|v| v.as_integer())
            && let Ok(limit) = usize::try_from(limit)
        {
            settings.osc52_limit = limit;
        }
        if let Some(raw) = value.get("theme").and_then(|v| v.as_str()) {
            settings.theme = raw.to_string();
        }
        if let Some(b) = value.get("animations").and_then(|v| v.as_bool()) {
            settings.animations = b;
        }
        if let Some(b) = value.get("hover_hints").and_then(|v| v.as_bool()) {
            settings.hover_hints = b;
        }
        if let Some(cmd) = value.get("ai_cmd").and_then(|v| v.as_str()) {
            settings.ai_cmd = cmd.to_string();
        }
        if let Some(b) = value.get("ai_confirmed").and_then(|v| v.as_bool()) {
            settings.ai_confirmed = b;
        }
        if let Some(raw) = value.get("jq_tab").and_then(|v| v.as_str()) {
            match raw {
                "cycle" => settings.jq_tab = JqTab::Cycle,
                "menu" | "accept" => settings.jq_tab = JqTab::Menu,
                other => warnings.push(format!(
                    "invalid value {other:?} for jq_tab in config.toml \
                     (expected \"menu\" or \"cycle\"); using \"menu\""
                )),
            }
        }

        if let Some(table) = value.get("animation_ms").and_then(|v| v.as_table()) {
            const KNOWN_KEYS: [&str; 9] = [
                "tab_slide",
                "hover",
                "focus",
                "list_travel",
                "modal_open",
                "dropdown_open",
                "pane_collapse",
                "toast",
                "send_breathe",
            ];
            let mut set_ms = |key: &str, field: &mut std::time::Duration| {
                let Some(v) = table.get(key) else {
                    return;
                };
                match v.as_integer().and_then(|ms| u64::try_from(ms).ok()) {
                    Some(ms) => *field = std::time::Duration::from_millis(ms),
                    None => warnings.push(format!(
                        "invalid value for {key:?} in [animation_ms] section of \
                         config.toml (expected a non-negative integer); using default"
                    )),
                }
            };
            set_ms("tab_slide", &mut settings.anim_ms.tab_slide);
            set_ms("hover", &mut settings.anim_ms.hover);
            set_ms("focus", &mut settings.anim_ms.focus);
            set_ms("list_travel", &mut settings.anim_ms.list_travel);
            set_ms("modal_open", &mut settings.anim_ms.modal_open);
            set_ms("dropdown_open", &mut settings.anim_ms.dropdown_open);
            set_ms("pane_collapse", &mut settings.anim_ms.pane_collapse);
            set_ms("toast", &mut settings.anim_ms.toast);
            set_ms("send_breathe", &mut settings.anim_ms.send_breathe);

            for key in table.keys() {
                if !KNOWN_KEYS.contains(&key.as_str()) {
                    warnings.push(format!(
                        "unknown key {key:?} in [animation_ms] section of config.toml"
                    ));
                }
            }
        }

        (settings, warnings)
    }
}

/// Everything [`Config::load`] read at startup: the parsed contents of
/// every config file, each already degraded to its defaults where the
/// file was missing or unusable (the warnings say which).
pub struct Loaded {
    pub registry: ProjectsRegistry,
    pub ui: UiSettings,
    pub keymap: crate::keys::Keymap,
    pub themes: crate::theme::ThemeRegistry,
    pub usage: crate::usage::UsageStore,
}

/// What one pass of [`Config::read_all`] got off disk. Each field is a
/// `Result` so the *caller* decides what a failure means: startup falls
/// back to defaults, a live reload keeps what it already has. A file that
/// is simply absent is not a failure — it reads as its defaults, in both
/// callers.
struct ReadAll {
    /// `config.toml`'s two halves, parsed from one `toml::Value`: they
    /// succeed and fail together, so they travel together.
    config: Result<(ProjectsRegistry, UiSettings), String>,
    keymap: Result<crate::keys::Keymap, String>,
    /// `Err` only when `themes/` itself could not be listed; an
    /// individual theme file that will not read or parse is skipped with
    /// a warning and the rest of the registry still builds.
    themes: Result<crate::theme::ThemeRegistry, String>,
}

/// What a live reload re-read. A field is `None` when its file exists but
/// could not be read or parsed — the caller keeps what it has and shows
/// the warning. A missing file reads as its defaults, exactly as at
/// startup.
pub struct Reloaded {
    /// `config.toml`'s registry and UI settings — one file, one outcome.
    pub config: Option<(ProjectsRegistry, UiSettings)>,
    pub keymap: Option<crate::keys::Keymap>,
    /// `None` when `themes/` could not be listed at all; the app then
    /// keeps the registry it has.
    pub themes: Option<crate::theme::ThemeRegistry>,
}

/// The one reader and writer of the XDG config files (`config.toml`,
/// `keys.toml`, `ui.toml`, `themes/*.toml`), through its own
/// [`postui_core::disk::Disk`] rooted at the config directory: every read
/// goes through here, every write is atomic, and a file that does not
/// parse is reported rather than replaced.
pub struct Config {
    /// `None` when no config dir resolved for this platform, and in tests
    /// — every save is then a silent no-op and every load a default, so a
    /// test run never touches the user's real config.
    disk: Option<postui_core::disk::Disk>,
}

impl Config {
    /// Reads `config.toml` (the registry and the UI settings), `keys.toml`,
    /// `themes/*.toml` and `ui.toml`, returning the parsed contents and
    /// every warning the user needs to see at startup. A file that will
    /// not read or parse yields its defaults and a warning; it is never
    /// written over afterwards (see [`Self::edit`]).
    pub fn load(macos: bool) -> (Config, Loaded, Vec<String>) {
        Self::load_from(
            Config {
                disk: postui_core::config_dir().map(postui_core::disk::Disk::new),
            },
            macos,
        )
    }

    /// [`Self::load`] with the config directory already resolved: the half
    /// that reads and words, split from the half that decides *where*, so
    /// startup's behaviour — which file failure yields which defaults and
    /// which sentence — can be tested against a tempdir instead of only
    /// against the user's real XDG directory.
    pub(crate) fn load_from(mut cfg: Config, macos: bool) -> (Config, Loaded, Vec<String>) {
        let (read, mut warnings) = cfg.read_all();

        // Startup's answer to every failure is the same: run on the
        // defaults and say so. (A reload's answer is the opposite — see
        // [`Self::reload`] — which is exactly why `read_all` reports the
        // bare error and leaves the consequence to us.)
        let (registry, ui) = match read.config {
            Ok(pair) => pair,
            Err(e) => {
                warnings.push(format!("{e}; using default settings"));
                (ProjectsRegistry::default(), UiSettings::default())
            }
        };
        let keymap = match read.keymap {
            Ok(keymap) => keymap,
            Err(e) => {
                warnings.push(format!("keys.toml ignored, using default keys: {e}"));
                crate::keys::Keymap::default_bindings()
            }
        };
        warnings.extend(keymap.caret_warnings(macos));
        let themes = read
            .themes
            .unwrap_or_else(|_| crate::theme::ThemeRegistry::from_files(Vec::new()).0);

        // `ui.toml` is the app's own state file rather than a config file
        // the user edits, so it is read here and *not* by `read_all` —
        // a reload has no business re-reading it (see [`Self::reload`]).
        let usage = match cfg.read(UI_TOML) {
            Ok(Some(text)) => {
                let (usage, error) = crate::usage::UsageStore::parse(&text);
                if let Some(e) = error {
                    warnings.push(format!(
                        "could not parse {UI_TOML}: {e}; palette usage stats will not be saved until it is fixed"
                    ));
                }
                usage
            }
            Ok(None) => crate::usage::UsageStore::default(),
            Err(e) => {
                warnings.push(e);
                crate::usage::UsageStore::default()
            }
        };

        (
            cfg,
            Loaded {
                registry,
                ui,
                keymap,
                themes,
                usage,
            },
            warnings,
        )
    }

    /// A `Config` with no files behind it: every save a silent `Ok(())`,
    /// every read a `None`. What `App::bare` (and so every test) uses.
    pub fn none() -> Config {
        Config { disk: None }
    }

    /// A `Config` rooted at `dir`, for app tests that need a real (but
    /// tempdir-backed) disk behind reload.
    #[cfg(test)]
    pub(crate) fn at(dir: PathBuf) -> Config {
        Config {
            disk: Some(postui_core::disk::Disk::new(dir)),
        }
    }

    /// The single reader of the user-editable config files: `config.toml`
    /// (parsed once, into both the registry and the UI settings),
    /// `keys.toml` and `themes/`. Every outcome that is not a clean read
    /// comes back as the bare error text, because [`Self::load`] and
    /// [`Self::reload`] disagree about what to do with it — the only
    /// thing they agree on is what the files say.
    ///
    /// `ui.toml` is deliberately not here: it is app-owned state, written
    /// by the app itself, and only startup reads it.
    fn read_all(&mut self) -> (ReadAll, Vec<String>) {
        let mut warnings = Vec::new();

        let config = match self.read(CONFIG_TOML) {
            // A missing file is the empty document, which parses to the
            // defaults — the same thing an empty `config.toml` says.
            Ok(text) => match toml::from_str::<toml::Value>(&text.unwrap_or_default()) {
                Ok(value) => {
                    let registry = ProjectsRegistry::from_value(&value);
                    let (ui, ui_warnings) = UiSettings::from_value(&value);
                    // Warnings from values that ARE valid TOML but out of
                    // range (a bad `jq_tab`, an unknown `animation_ms`
                    // key): the file still applied, so these are advice,
                    // not the failure above.
                    warnings.extend(ui_warnings);
                    Ok((registry, ui))
                }
                Err(e) => Err(config_parse_error(&e)),
            },
            Err(e) => Err(e),
        };

        let (themes, theme_warnings) = self.reload_themes();
        warnings.extend(theme_warnings);
        let themes = themes.ok_or_else(|| format!("{THEMES_DIR}/ could not be listed"));

        let keymap = match self.read(KEYS_TOML) {
            Ok(Some(text)) => crate::keys::Keymap::try_from_overrides(&text),
            Ok(None) => Ok(crate::keys::Keymap::default_bindings()),
            Err(e) => Err(e),
        };

        (
            ReadAll {
                config,
                keymap,
                themes,
            },
            warnings,
        )
    }

    /// Re-reads the user-editable XDG config files — `config.toml`,
    /// `keys.toml` and `themes/` — for the user-triggered Reload command.
    /// `ui.toml` is app-owned state and is not re-read.
    ///
    /// Unlike [`Self::load`], a file that exists but will not read or
    /// parse yields `None` here rather than silently falling back to its
    /// defaults: reload must never discard settings the user is relying
    /// on because a file grew a syntax error, or because a read failed.
    /// The caller keeps whatever it already has for a `None` field and
    /// shows the accompanying warning.
    /// `current_keys` is the keymap the app is running on: the
    /// caret-conflict advisories are re-issued only when the file's
    /// bindings actually differ from it, so re-reading an unchanged
    /// keys.toml is silent rather than a nag.
    pub fn reload(
        &mut self,
        macos: bool,
        current_keys: &crate::keys::Keymap,
    ) -> (Reloaded, Vec<String>) {
        let (read, mut warnings) = self.read_all();

        let config = match read.config {
            Ok(pair) => Some(pair),
            Err(e) => {
                warnings.push(format!("{e}; keeping the current settings"));
                None
            }
        };
        let keymap = match read.keymap {
            Ok(keymap) => Some(keymap),
            Err(e) => {
                warnings.push(format!("keys.toml ignored, keeping the current keys: {e}"));
                None
            }
        };
        if let Some(keymap) = &keymap
            && keymap != current_keys
        {
            warnings.extend(keymap.caret_warnings(macos));
        }
        // A `themes/` that would not list already warned inside
        // `read_all`; here it just means "keep the registry you have".
        let themes = read.themes.ok();

        (
            Reloaded {
                config,
                keymap,
                themes,
            },
            warnings,
        )
    }

    /// The file's text: `Ok(None)` when it (or the whole config dir)
    /// isn't there, `Err` when it is there but could not be read (a
    /// permission or I/O error, invalid UTF-8). The two are kept apart on
    /// purpose — an unreadable file is not an absent one, and only the
    /// caller knows whether "absent" is a safe thing to substitute.
    fn read(&mut self, name: &str) -> Result<Option<String>, String> {
        let Some(disk) = self.disk.as_mut() else {
            return Ok(None);
        };
        let rel = postui_core::disk::RelPath::new(name).map_err(|e| e.to_string())?;
        disk.read(&rel).map_err(|e| e.to_string())
    }

    /// Applies `f` to `name`'s document (an empty one when the file is
    /// absent) and writes the result back atomically, so only the keys `f`
    /// touches change and everything else survives byte-for-byte. A file
    /// that does not parse is refused, not replaced: the user's config is
    /// never the casualty of a syntax error they have yet to fix.
    fn edit(
        &mut self,
        name: &str,
        f: impl FnOnce(&mut toml_edit::DocumentMut),
    ) -> Result<(), String> {
        let Some(disk) = self.disk.as_mut() else {
            return Ok(());
        };
        let rel = postui_core::disk::RelPath::new(name).map_err(|e| e.to_string())?;
        let existing = disk
            .read(&rel)
            .map_err(|e| e.to_string())?
            .unwrap_or_default();
        let mut doc: toml_edit::DocumentMut =
            existing.parse().map_err(|e: toml_edit::TomlError| {
                format!("{name} has a syntax error and was left unchanged: {e}")
            })?;
        f(&mut doc);
        disk.write(&rel, &doc.to_string())
            .map_err(|e| e.to_string())
    }

    /// Persists the `[projects]` table of `config.toml`.
    pub fn save_registry(&mut self, registry: &ProjectsRegistry) -> Result<(), String> {
        self.edit(CONFIG_TOML, |doc| registry.write_into(doc))
    }

    /// Persists the top-level `theme` key of `config.toml`.
    pub fn save_ui_theme(&mut self, name: &str) -> Result<(), String> {
        self.edit(CONFIG_TOML, |doc| doc["theme"] = toml_edit::value(name))
    }

    /// Persists one top-level boolean of `config.toml` (the `ai_confirmed`
    /// "don't ask again" flag).
    pub fn save_ui_flag(&mut self, key: &str, value: bool) -> Result<(), String> {
        self.edit(CONFIG_TOML, |doc| doc[key] = toml_edit::value(value))
    }

    /// Persists the palette usage stats to `ui.toml`.
    pub fn save_usage(&mut self, usage: &crate::usage::UsageStore) -> Result<(), String> {
        self.edit(UI_TOML, |doc| usage.write_into(doc))
    }

    /// Rescans `themes/` and rebuilds the registry, so a custom theme file
    /// added or edited since startup shows up without a restart. A missing
    /// directory is silently just the built-ins; a file that can't be read
    /// or parsed is one warning and is skipped. `None` — with a warning —
    /// when the directory itself could not be listed: nothing was read, so
    /// there is no registry to speak of and the caller must not mistake
    /// the built-ins for "the user has no custom themes".
    pub fn reload_themes(&mut self) -> (Option<crate::theme::ThemeRegistry>, Vec<String>) {
        let mut files = Vec::new();
        let mut warnings = Vec::new();
        if let Some(disk) = self.disk.as_mut()
            && let Ok(dir) = postui_core::disk::RelPath::new(THEMES_DIR)
        {
            let entries = match disk.list(&dir) {
                Ok(entries) => entries,
                Err(e) => {
                    warnings.push(format!(
                        "could not list {THEMES_DIR}/: {e}; custom themes unavailable"
                    ));
                    return (None, warnings);
                }
            };
            for entry in entries
                .into_iter()
                .filter(|e| !e.is_dir && e.name.ends_with(".toml"))
            {
                let Ok(rel) = dir.join(&entry.name) else {
                    continue;
                };
                match disk.read(&rel) {
                    Ok(Some(text)) => files.push((entry.name, text)),
                    Ok(None) => {}
                    Err(e) => warnings.push(format!(
                        "theme file {}: unreadable: {e}; skipped",
                        entry.name
                    )),
                }
            }
        }
        let (registry, parse_warnings) = crate::theme::ThemeRegistry::from_files(files);
        warnings.extend(parse_warnings);
        (Some(registry), warnings)
    }
}

/// Expands a leading `~/` to the home directory. Paths not starting with
/// `~/` are returned unchanged.
pub fn expand_tilde(s: &str) -> PathBuf {
    let home = || directories::BaseDirs::new().map(|d| d.home_dir().to_path_buf());
    if s == "~" {
        return home().unwrap_or_else(|| PathBuf::from(s));
    }
    match s.strip_prefix("~/") {
        Some(rest) => match home() {
            Some(h) => h.join(rest),
            None => PathBuf::from(s),
        },
        None => PathBuf::from(s),
    }
}

/// Result of parsing the single optional CLI argument `postui` accepts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CliParse {
    /// A project directory (or none), tilde-expanded.
    Root(Option<PathBuf>),
    /// A leading-dash argument (`--help`, `-x`, ...): print usage and exit.
    Usage,
    /// `--setup`: print terminal keyboard-config guidance and exit.
    Setup,
    /// `--keydump[=FLAGS]`: run the raw key-event echo loop and exit.
    /// `flags` is an explicit kitty-keyboard-protocol enhancement bitmask
    /// to push (`0` pushes nothing); `None` means "push what the app
    /// itself pushes", so a plain `--keydump` reproduces the app's exact
    /// input conditions.
    Keydump { flags: Option<u8> },
}

/// Parses `postui`'s single optional argument. Any value starting with `-`
/// (e.g. `--help`, `-x`) is treated as a request for usage rather than a
/// project directory, since no real path starts with a dash without `./`.
pub fn parse_cli(arg: Option<String>) -> CliParse {
    match arg {
        Some(s) if s == "--setup" => CliParse::Setup,
        Some(s) if s == "--keydump" => CliParse::Keydump { flags: None },
        Some(s) if s.starts_with("--keydump=") => match s["--keydump=".len()..].parse::<u8>() {
            Ok(bits) => CliParse::Keydump { flags: Some(bits) },
            Err(_) => CliParse::Usage,
        },
        Some(s) if s.starts_with('-') => CliParse::Usage,
        Some(s) => CliParse::Root(Some(expand_tilde(&s))),
        None => CliParse::Root(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::tempdir;

    #[test]
    fn parse_empty_is_default_and_a_mistyped_table_degrades_silently() {
        let (r, warnings) = ProjectsRegistry::parse("");
        assert!(r.known.is_empty() && r.last.is_none());
        assert!(warnings.is_empty(), "a missing file is nothing to report");
        let (r, warnings) = ProjectsRegistry::parse("projects = 5\n");
        assert!(r.known.is_empty(), "mistyped table degrades to default");
        assert!(warnings.is_empty());
    }

    #[test]
    fn a_config_that_does_not_parse_is_reported_and_never_overwritten() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("config.toml");
        let broken = "theme = \"dark\"\nclipboard_cmd = \"xclip\nai_cmd = \"claude -p\"\n\n[projects]\nknown = [\"/tmp/a\"]\n";
        std::fs::write(&p, broken).unwrap();

        let (r, warnings) = ProjectsRegistry::parse(broken);
        assert!(r.known.is_empty());
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("could not parse"), "{warnings:?}");
        let (s, warnings) = UiSettings::parse(broken);
        assert_eq!(s, UiSettings::default());
        assert!(
            warnings.iter().any(|w| w.contains("could not parse")),
            "{warnings:?}"
        );

        let mut cfg = Config {
            disk: Some(postui_core::disk::Disk::new(dir.path().to_path_buf())),
        };
        let mut r = r;
        r.register(PathBuf::from("/tmp/b"));
        assert!(cfg.save_registry(&r).is_err(), "the registry save refuses");
        assert!(cfg.save_ui_theme("light").is_err());
        assert!(cfg.save_ui_flag("ai_confirmed", true).is_err());
        assert_eq!(
            std::fs::read_to_string(&p).unwrap(),
            broken,
            "every refused save leaves the user's file byte-for-byte"
        );
    }

    /// The `ui.toml` half of the same rule: `save_usage` refuses a file it
    /// cannot parse rather than replacing the user's document with a fresh
    /// one holding only `[palette.usage]`.
    #[test]
    fn config_edit_refuses_a_file_that_does_not_parse_and_leaves_it_alone() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("ui.toml"), "not = [toml").unwrap();
        let mut cfg = Config {
            disk: Some(postui_core::disk::Disk::new(dir.path().to_path_buf())),
        };
        let err = cfg
            .save_usage(&crate::usage::UsageStore::default())
            .unwrap_err();
        assert!(err.contains("ui.toml has a syntax error"), "{err}");
        assert_eq!(
            std::fs::read_to_string(dir.path().join("ui.toml")).unwrap(),
            "not = [toml"
        );
    }

    /// Every save on a `Config` with no config dir behind it is a silent
    /// no-op, so a test run never touches the user's real files.
    #[test]
    fn config_none_saves_nothing_and_succeeds() {
        let mut cfg = Config::none();
        assert!(cfg.save_registry(&ProjectsRegistry::default()).is_ok());
        assert!(cfg.save_ui_theme("dusk").is_ok());
        assert!(cfg.save_ui_flag("ai_confirmed", true).is_ok());
        assert!(cfg.save_usage(&crate::usage::UsageStore::default()).is_ok());
        let (themes, warnings) = cfg.reload_themes();
        assert!(warnings.is_empty());
        assert_eq!(
            themes
                .expect("no config dir is not a listing failure")
                .entries()
                .len(),
            9,
            "the built-ins"
        );
    }

    /// A valid theme text, as `from_files`' own tests write them.
    const THEME_TOML: &str = "bg = \"#101418\"\nfg = \"#e2e2e6\"\naccent = \"#0178d4\"\n\
         success = \"#9ece6a\"\nwarning = \"#e0af68\"\nerror = \"#f7768e\"\n";

    /// `from_files` is unit-tested on pairs it is handed; this covers the
    /// listing half `Config` does: only `*.toml` *files* in `themes/` are
    /// read, so a note file and a directory that happens to end in `.toml`
    /// are both skipped without a warning.
    #[test]
    fn reload_themes_reads_only_toml_files_from_the_themes_directory() {
        let dir = tempdir().unwrap();
        let themes = dir.path().join("themes");
        std::fs::create_dir_all(&themes).unwrap();
        std::fs::write(themes.join("good.toml"), THEME_TOML).unwrap();
        std::fs::write(themes.join("notes.txt"), "not a theme").unwrap();
        std::fs::create_dir_all(themes.join("sub.toml")).unwrap();

        let mut cfg = Config {
            disk: Some(postui_core::disk::Disk::new(dir.path().to_path_buf())),
        };
        let (registry, warnings) = cfg.reload_themes();
        assert!(warnings.is_empty(), "{warnings:?}");
        let registry = registry.expect("the directory listed");
        let customs: Vec<&str> = registry
            .entries()
            .iter()
            .filter(|e| matches!(e.source, crate::theme::ThemeSource::Custom(_)))
            .map(|e| e.name.as_str())
            .collect();
        assert_eq!(customs, vec!["good"], "named from the file stem");
    }

    #[cfg(unix)]
    #[test]
    fn reload_themes_warns_and_skips_a_theme_file_it_cannot_read() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempdir().unwrap();
        let themes = dir.path().join("themes");
        std::fs::create_dir_all(&themes).unwrap();
        std::fs::write(themes.join("good.toml"), THEME_TOML).unwrap();
        let locked = themes.join("locked.toml");
        std::fs::write(&locked, THEME_TOML).unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();

        let mut cfg = Config {
            disk: Some(postui_core::disk::Disk::new(dir.path().to_path_buf())),
        };
        let (registry, warnings) = cfg.reload_themes();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o644)).unwrap();
        let registry = registry.expect("one unreadable file is not a listing failure");
        let customs: Vec<&str> = registry
            .entries()
            .iter()
            .filter(|e| matches!(e.source, crate::theme::ThemeSource::Custom(_)))
            .map(|e| e.name.as_str())
            .collect();
        if customs == vec!["good", "locked"] {
            // Running as root: nothing is unreadable. Not a failure.
            return;
        }
        assert_eq!(customs, vec!["good"], "the readable one still loads");
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(
            warnings[0].starts_with("theme file locked.toml: unreadable: ")
                && warnings[0].ends_with("; skipped"),
            "{warnings:?}"
        );
    }

    #[test]
    fn save_round_trips_and_preserves_unrelated_keys() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("config.toml");
        std::fs::write(&p, "theme = \"dark\"\n").unwrap();
        let mut cfg = Config {
            disk: Some(postui_core::disk::Disk::new(dir.path().to_path_buf())),
        };
        let (mut r, _) = ProjectsRegistry::parse(&std::fs::read_to_string(&p).unwrap());
        r.register(PathBuf::from("/tmp/a"));
        r.register(PathBuf::from("/tmp/b"));
        r.register(PathBuf::from("/tmp/a")); // dedup, but last updates
        r.root = Some(PathBuf::from("/tmp/root"));
        cfg.save_registry(&r).unwrap();

        let text = std::fs::read_to_string(&p).unwrap();
        assert!(
            text.contains("theme = \"dark\""),
            "unrelated key preserved: {text}"
        );
        let (r2, _) = ProjectsRegistry::parse(&text);
        assert_eq!(
            r2.known,
            vec![PathBuf::from("/tmp/a"), PathBuf::from("/tmp/b")]
        );
        assert_eq!(r2.last, Some(PathBuf::from("/tmp/a")));
        assert_eq!(r2.root, Some(PathBuf::from("/tmp/root")));
    }

    #[test]
    fn next_after_cycles_and_wraps() {
        let (mut r, _) = ProjectsRegistry::parse("");
        assert!(
            r.next_after(&PathBuf::from("/tmp/a")).is_none(),
            "fewer than two projects"
        );
        let a = tempdir().unwrap();
        let b = tempdir().unwrap();
        let c = tempdir().unwrap();
        r.register(a.path().to_path_buf());
        r.register(b.path().to_path_buf());
        r.register(c.path().to_path_buf());
        assert_eq!(r.next_after(b.path()), Some(c.path().to_path_buf()));
        assert_eq!(
            r.next_after(c.path()),
            Some(a.path().to_path_buf()),
            "wraps"
        );
        assert_eq!(
            r.next_after(&PathBuf::from("/elsewhere")),
            Some(a.path().to_path_buf()),
            "unknown current starts from the top"
        );
    }

    #[test]
    fn next_after_skips_dead_registry_paths() {
        let live_a = tempdir().unwrap();
        let dead = tempdir().unwrap();
        let dead_path = dead.path().to_path_buf();
        drop(dead); // directory no longer exists on disk
        let live_b = tempdir().unwrap();

        let mut r = ProjectsRegistry::default();
        r.register(live_a.path().to_path_buf());
        r.register(dead_path);
        r.register(live_b.path().to_path_buf());

        assert_eq!(
            r.next_after(live_a.path()),
            Some(live_b.path().to_path_buf()),
            "dead entry between the two live ones is skipped"
        );
    }

    #[test]
    fn ui_settings_parse_empty_is_default() {
        let (s, warnings) = UiSettings::parse("");
        assert_eq!(s, UiSettings::default());
        assert_eq!(s.clipboard_cmd, None);
        assert_eq!(s.osc52_limit, 65536);
        assert_eq!(s.theme, "terminal");
        assert!(warnings.is_empty());
    }

    #[test]
    fn ui_settings_parse_configured_values() {
        let (s, warnings) = UiSettings::parse("clipboard_cmd = \"xclip\"\nosc52_limit = 1000\n");
        assert_eq!(s.clipboard_cmd, Some("xclip".to_string()));
        assert_eq!(s.osc52_limit, 1000);
        assert!(warnings.is_empty());
    }

    #[test]
    fn ui_settings_wrong_types_degrade_to_defaults() {
        let (s, _warnings) =
            UiSettings::parse("clipboard_cmd = 5\nosc52_limit = \"not a number\"\n");
        assert_eq!(s, UiSettings::default());
    }

    #[test]
    fn ui_settings_corrupt_text_is_default_with_a_warning() {
        let (s, warnings) = UiSettings::parse("not valid toml [[[");
        assert_eq!(s, UiSettings::default());
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(
            warnings[0].contains("using default settings"),
            "{warnings:?}"
        );
    }

    #[test]
    fn ui_settings_theme_is_a_raw_name_string() {
        let (s, warnings) = UiSettings::parse("theme = \"gruvbox-dark\"\n");
        assert_eq!(s.theme, "gruvbox-dark");
        assert!(
            warnings.is_empty(),
            "name validity is the registry's business at resolve time, not load's: {warnings:?}"
        );
    }

    #[test]
    fn save_ui_theme_sets_the_key_and_preserves_unrelated_content() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("config.toml");
        std::fs::write(&p, "clipboard_cmd = \"xclip\"\n\n[projects]\nknown = []\n").unwrap();
        let mut cfg = Config {
            disk: Some(postui_core::disk::Disk::new(dir.path().to_path_buf())),
        };
        cfg.save_ui_theme("catppuccin-mocha").unwrap();
        let text = std::fs::read_to_string(&p).unwrap();
        assert!(text.contains("theme = \"catppuccin-mocha\""), "{text}");
        assert!(text.contains("clipboard_cmd = \"xclip\""), "{text}");
        assert!(text.contains("[projects]"), "{text}");
        assert_eq!(UiSettings::parse(&text).0.theme, "catppuccin-mocha");
        cfg.save_ui_theme("dark").unwrap();
        let text = std::fs::read_to_string(&p).unwrap();
        assert_eq!(
            UiSettings::parse(&text).0.theme,
            "dark",
            "overwrites an existing key"
        );
    }

    #[test]
    fn save_ui_theme_creates_a_missing_file() {
        let dir = tempdir().unwrap();
        let mut cfg = Config {
            disk: Some(postui_core::disk::Disk::new(dir.path().join("sub"))),
        };
        cfg.save_ui_theme("light").unwrap();
        let text = std::fs::read_to_string(dir.path().join("sub").join("config.toml")).unwrap();
        assert_eq!(UiSettings::parse(&text).0.theme, "light");
    }

    #[test]
    fn ai_settings_default_and_parse() {
        let (s, _) = UiSettings::parse("");
        assert_eq!(s.ai_cmd, "claude -p");
        assert!(!s.ai_confirmed);
        let (s, warnings) = UiSettings::parse("ai_cmd = \"my-llm --jq\"\nai_confirmed = true\n");
        assert_eq!(s.ai_cmd, "my-llm --jq");
        assert!(s.ai_confirmed);
        assert!(warnings.is_empty());
    }

    #[test]
    fn jq_tab_defaults_to_menu_and_parses_cycle() {
        assert_eq!(UiSettings::parse("").0.jq_tab, JqTab::Menu);
        let (s, warnings) = UiSettings::parse("jq_tab = \"cycle\"\n");
        assert_eq!(s.jq_tab, JqTab::Cycle);
        assert!(warnings.is_empty());
        let (s, warnings) = UiSettings::parse("jq_tab = \"accept\"\n");
        assert_eq!(s.jq_tab, JqTab::Menu, "the older name still works");
        assert!(warnings.is_empty());
        assert_eq!(
            UiSettings::parse("jq_tab = \"menu\"\n").0.jq_tab,
            JqTab::Menu
        );
        let (s, warnings) = UiSettings::parse("jq_tab = \"popup\"\n");
        assert_eq!(s.jq_tab, JqTab::Menu, "a bad value falls back");
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("jq_tab"), "{warnings:?}");
    }

    #[test]
    fn save_ui_flag_sets_the_key_and_preserves_unrelated_content() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("config.toml");
        std::fs::write(&p, "theme = \"x\"\n\n[projects]\nknown = []\n").unwrap();
        let mut cfg = Config {
            disk: Some(postui_core::disk::Disk::new(dir.path().to_path_buf())),
        };
        cfg.save_ui_flag("ai_confirmed", true).unwrap();
        let text = std::fs::read_to_string(&p).unwrap();
        assert!(text.contains("ai_confirmed = true"), "{text}");
        assert!(
            text.contains("theme = \"x\"") && text.contains("[projects]"),
            "{text}"
        );
        assert!(UiSettings::parse(&text).0.ai_confirmed);
    }

    #[test]
    fn ui_settings_missing_theme_key_defaults_to_terminal() {
        let (s, warnings) = UiSettings::parse("clipboard_cmd = \"xclip\"\n");
        assert_eq!(s.theme, "terminal");
        assert!(warnings.is_empty());
    }

    #[test]
    fn hover_hints_key_parses_and_defaults_true() {
        let (s, warnings) = UiSettings::parse("hover_hints = false\n");
        assert!(!s.hover_hints);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert!(UiSettings::default().hover_hints);
    }

    #[test]
    fn animations_key_parses_and_defaults_true() {
        let (s, warnings) = UiSettings::parse("animations = false\n");
        assert!(!s.animations);
        assert!(warnings.is_empty());
        assert!(UiSettings::default().animations);
    }

    #[test]
    fn animation_ms_defaults_without_a_table() {
        let (s, warnings) = UiSettings::parse("");
        assert_eq!(s.anim_ms, AnimDurations::default());
        assert_eq!(s.anim_ms.tab_slide, Duration::from_millis(250));
        assert_eq!(s.anim_ms.hover, Duration::from_millis(70));
        assert_eq!(s.anim_ms.focus, Duration::from_millis(90));
        assert_eq!(s.anim_ms.list_travel, Duration::from_millis(100));
        assert_eq!(s.anim_ms.modal_open, Duration::from_millis(100));
        assert_eq!(s.anim_ms.dropdown_open, Duration::from_millis(90));
        assert_eq!(s.anim_ms.pane_collapse, Duration::from_millis(120));
        assert_eq!(s.anim_ms.toast, Duration::from_millis(100));
        assert_eq!(s.anim_ms.send_breathe, Duration::from_millis(700));
        assert!(warnings.is_empty());
    }

    #[test]
    fn animation_ms_table_overrides_one_key_and_defaults_the_rest() {
        let (s, warnings) = UiSettings::parse("[animation_ms]\ntab_slide = 400\n");
        assert_eq!(s.anim_ms.tab_slide, Duration::from_millis(400));
        assert_eq!(s.anim_ms.hover, Duration::from_millis(70));
        assert_eq!(s.anim_ms.focus, Duration::from_millis(90));
        assert_eq!(s.anim_ms.list_travel, Duration::from_millis(100));
        assert_eq!(s.anim_ms.modal_open, Duration::from_millis(100));
        assert_eq!(s.anim_ms.dropdown_open, Duration::from_millis(90));
        assert_eq!(s.anim_ms.pane_collapse, Duration::from_millis(120));
        assert_eq!(s.anim_ms.toast, Duration::from_millis(100));
        assert_eq!(s.anim_ms.send_breathe, Duration::from_millis(700));
        assert!(warnings.is_empty());
    }

    #[test]
    fn animation_ms_table_can_override_every_key() {
        let (s, warnings) = UiSettings::parse(
            "[animation_ms]\n\
             tab_slide = 1\n\
             hover = 2\n\
             focus = 3\n\
             list_travel = 4\n\
             modal_open = 5\n\
             dropdown_open = 6\n\
             pane_collapse = 7\n\
             toast = 8\n\
             send_breathe = 9\n",
        );
        assert_eq!(
            s.anim_ms,
            AnimDurations {
                tab_slide: Duration::from_millis(1),
                hover: Duration::from_millis(2),
                focus: Duration::from_millis(3),
                list_travel: Duration::from_millis(4),
                modal_open: Duration::from_millis(5),
                dropdown_open: Duration::from_millis(6),
                pane_collapse: Duration::from_millis(7),
                toast: Duration::from_millis(8),
                send_breathe: Duration::from_millis(9),
            }
        );
        assert!(warnings.is_empty());
    }

    #[test]
    fn animation_ms_unknown_key_warns_and_is_ignored() {
        let (s, warnings) = UiSettings::parse("[animation_ms]\nbogus = 5\n");
        assert_eq!(s.anim_ms, AnimDurations::default());
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("bogus"));
    }

    /// A present-but-unusable value (not an integer, e.g. a string) must
    /// warn and default, the same as an unknown key -- not silently default
    /// with no warning.
    #[test]
    fn animation_ms_non_integer_value_warns_and_defaults() {
        let (s, warnings) = UiSettings::parse("[animation_ms]\nhover = \"fast\"\n");
        assert_eq!(s.anim_ms, AnimDurations::default());
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("hover"));
    }

    /// A negative value fails the `u64` conversion the same way a
    /// non-integer does, and must warn rather than silently default.
    #[test]
    fn animation_ms_negative_value_warns_and_defaults() {
        let (s, warnings) = UiSettings::parse("[animation_ms]\nhover = -5\n");
        assert_eq!(s.anim_ms, AnimDurations::default());
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("hover"));
    }

    #[test]
    fn parse_cli_leading_dash_is_usage() {
        assert_eq!(parse_cli(Some("--help".into())), CliParse::Usage);
        assert_eq!(parse_cli(Some("-x".into())), CliParse::Usage);
    }

    #[test]
    fn parse_cli_setup_flag() {
        assert_eq!(parse_cli(Some("--setup".into())), CliParse::Setup);
        // Not a prefix match: anything else dashed is still usage.
        assert_eq!(parse_cli(Some("--setup-x".into())), CliParse::Usage);
    }

    #[test]
    fn parse_cli_keydump_flag_with_optional_bitmask() {
        assert_eq!(
            parse_cli(Some("--keydump".into())),
            CliParse::Keydump { flags: None }
        );
        assert_eq!(
            parse_cli(Some("--keydump=0".into())),
            CliParse::Keydump { flags: Some(0) }
        );
        assert_eq!(
            parse_cli(Some("--keydump=5".into())),
            CliParse::Keydump { flags: Some(5) }
        );
        // A malformed bitmask is usage, not a silent default.
        assert_eq!(parse_cli(Some("--keydump=x".into())), CliParse::Usage);
        assert_eq!(parse_cli(Some("--keydump=".into())), CliParse::Usage);
        assert_eq!(parse_cli(Some("--keydump-x".into())), CliParse::Usage);
    }

    #[test]
    fn parse_cli_normal_path_and_none() {
        assert_eq!(
            parse_cli(Some("/abs/x".into())),
            CliParse::Root(Some(PathBuf::from("/abs/x")))
        );
        assert_eq!(parse_cli(None), CliParse::Root(None));
    }

    #[test]
    fn tilde_expansion() {
        let home = directories::BaseDirs::new()
            .unwrap()
            .home_dir()
            .to_path_buf();
        assert_eq!(expand_tilde("~/x/y"), home.join("x/y"));
        assert_eq!(expand_tilde("~"), home);
        assert_eq!(expand_tilde("/abs/x"), PathBuf::from("/abs/x"));
        assert_eq!(expand_tilde("rel"), PathBuf::from("rel"));
    }

    #[test]
    fn reload_parses_a_good_config_toml() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("config.toml"),
            "theme = \"x\"\n\n[projects]\nknown = [\"/tmp/a\", \"/tmp/b\"]\n",
        )
        .unwrap();
        let mut cfg = Config::at(dir.path().to_path_buf());

        let (reloaded, warnings) = cfg.reload(false, &crate::keys::Keymap::default_bindings());

        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(reloaded.config.as_ref().unwrap().1.theme, "x");
        assert_eq!(
            reloaded.config.unwrap().0.known,
            vec![PathBuf::from("/tmp/a"), PathBuf::from("/tmp/b")]
        );
    }

    #[test]
    fn reload_reports_an_unparsable_config_toml_and_keeps_current_settings() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("config.toml"),
            "theme = \"dark\"\nclipboard_cmd = \"xclip\n",
        )
        .unwrap();
        let mut cfg = Config::at(dir.path().to_path_buf());

        let (reloaded, warnings) = cfg.reload(false, &crate::keys::Keymap::default_bindings());

        assert!(reloaded.config.is_none());
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("config.toml"), "{warnings:?}");
        assert!(warnings[0].contains("keeping"), "{warnings:?}");
    }

    #[test]
    fn reload_surfaces_a_ui_settings_warning_from_otherwise_valid_toml() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("config.toml"),
            "theme = \"x\"\njq_tab = \"bogus\"\n",
        )
        .unwrap();
        let mut cfg = Config::at(dir.path().to_path_buf());

        let (reloaded, warnings) = cfg.reload(false, &crate::keys::Keymap::default_bindings());

        assert!(reloaded.config.is_some(), "valid TOML still parses");
        assert!(
            warnings.iter().any(|w| w.contains("jq_tab")),
            "{warnings:?}"
        );
    }

    #[test]
    fn reload_missing_config_toml_is_defaults_with_no_warnings() {
        let dir = tempdir().unwrap();
        let mut cfg = Config::at(dir.path().to_path_buf());

        let (reloaded, warnings) = cfg.reload(false, &crate::keys::Keymap::default_bindings());

        assert_eq!(
            reloaded.config,
            Some((ProjectsRegistry::default(), UiSettings::default()))
        );
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    #[test]
    fn reload_applies_a_good_keys_toml() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("keys.toml"), "save = \"alt+shift+s\"\n").unwrap();
        let mut cfg = Config::at(dir.path().to_path_buf());

        let (reloaded, _warnings) = cfg.reload(false, &crate::keys::Keymap::default_bindings());

        let keymap = reloaded.keymap.expect("good keys.toml parses");
        let combo = crate::keys::KeyCombo::parse("alt+shift+s").unwrap();
        assert_eq!(
            keymap.lookup(&combo),
            Some(crate::action::Action::SaveRequest)
        );
    }

    #[test]
    fn reload_reports_a_bad_keys_toml_and_keeps_current_keys() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("keys.toml"), "save = \"not-a-combo\"\n").unwrap();
        let mut cfg = Config::at(dir.path().to_path_buf());

        let (reloaded, warnings) = cfg.reload(false, &crate::keys::Keymap::default_bindings());

        assert!(reloaded.keymap.is_none());
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("keys.toml") && w.contains("keeping")),
            "{warnings:?}"
        );
    }

    #[test]
    fn reload_picks_up_a_theme_file_added_after_startup() {
        let dir = tempdir().unwrap();
        let themes = dir.path().join("themes");
        std::fs::create_dir_all(&themes).unwrap();
        std::fs::write(themes.join("good.toml"), THEME_TOML).unwrap();
        let mut cfg = Config::at(dir.path().to_path_buf());

        let (reloaded, _warnings) = cfg.reload(false, &crate::keys::Keymap::default_bindings());

        let themes = reloaded.themes.expect("the directory listed");
        let customs: Vec<&str> = themes
            .entries()
            .iter()
            .filter(|e| matches!(e.source, crate::theme::ThemeSource::Custom(_)))
            .map(|e| e.name.as_str())
            .collect();
        assert_eq!(customs, vec!["good"]);
    }

    // --- startup, against a tempdir ------------------------------------
    //
    // `Config::load` resolves the real XDG directory, so these go through
    // `load_from` — the same reading and wording, handed a config dir the
    // test owns. Startup's contract: every failure runs on the defaults
    // and says so, and a *missing* file is not a failure at all.

    #[test]
    fn startup_runs_on_defaults_and_says_so_when_config_toml_will_not_parse() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("config.toml"),
            "theme = \"dark\"\nclipboard_cmd = \"xclip\n",
        )
        .unwrap();

        let (_cfg, loaded, warnings) =
            Config::load_from(Config::at(dir.path().to_path_buf()), false);

        assert_eq!(loaded.ui, UiSettings::default());
        assert_eq!(loaded.registry, ProjectsRegistry::default());
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].starts_with("could not parse config.toml: "), "{warnings:?}");
        assert!(warnings[0].ends_with("; using default settings"), "{warnings:?}");
    }

    /// An unreadable config.toml gets the same treatment as an unparsable
    /// one at startup — the app has to come up on something — but the
    /// sentence names the read failure rather than a parse error.
    #[cfg(unix)]
    #[test]
    fn startup_runs_on_defaults_and_says_so_when_config_toml_cannot_be_read() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "theme = \"gruvbox-dark\"\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();

        let (_cfg, loaded, warnings) =
            Config::load_from(Config::at(dir.path().to_path_buf()), false);

        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        if loaded.ui.theme == "gruvbox-dark" {
            // Running as root: nothing is unreadable. Not a failure.
            return;
        }
        assert_eq!(loaded.ui, UiSettings::default());
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("config.toml"), "{warnings:?}");
        assert!(warnings[0].ends_with("; using default settings"), "{warnings:?}");
    }

    #[test]
    fn startup_with_no_config_files_at_all_is_silent_defaults() {
        let dir = tempdir().unwrap();

        let (_cfg, loaded, warnings) =
            Config::load_from(Config::at(dir.path().to_path_buf()), false);

        assert!(warnings.is_empty(), "a missing file is not a problem: {warnings:?}");
        assert_eq!(loaded.ui, UiSettings::default());
        assert_eq!(loaded.registry, ProjectsRegistry::default());
        assert_eq!(loaded.keymap, crate::keys::Keymap::default_bindings());
    }

    #[test]
    fn startup_falls_back_to_the_default_keys_and_says_so_on_a_bad_keys_toml() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("keys.toml"), "save = \"not-a-combo\"\n").unwrap();

        let (_cfg, loaded, warnings) =
            Config::load_from(Config::at(dir.path().to_path_buf()), false);

        assert_eq!(loaded.keymap, crate::keys::Keymap::default_bindings());
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(
            warnings[0].starts_with("keys.toml ignored, using default keys: "),
            "{warnings:?}"
        );
    }

    /// The caret-conflict lines are advice about what a keys.toml costs
    /// on macOS — true the moment the file is written, and no truer the
    /// tenth time the user reloads. Repeating them on every reload turns
    /// the reload into a nag.
    #[test]
    fn reload_repeats_the_caret_advisories_only_when_the_keys_changed() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("keys.toml"), "save = \"ctrl+a\"\n").unwrap();
        let mut cfg = Config::at(dir.path().to_path_buf());

        let (first, warnings) = cfg.reload(true, &crate::keys::Keymap::default_bindings());
        let keymap = first.keymap.expect("the file parsed");
        assert!(
            warnings.iter().any(|w| w.contains("shadows")),
            "the new bindings are worth one warning: {warnings:?}"
        );

        // Same file, and the app is already running on it.
        let (_, warnings) = cfg.reload(true, &keymap);
        assert!(
            !warnings.iter().any(|w| w.contains("shadows")),
            "nothing changed, so there is nothing to warn about: {warnings:?}"
        );
    }

    /// An UNREADABLE file is not a MISSING file: reload must not hand the
    /// app defaults (emptying its registry, resetting its settings)
    /// because a read failed. `#[cfg(unix)]` — the mode bits are the only
    /// portable way to make a read fail on demand.
    #[cfg(unix)]
    #[test]
    fn reload_treats_an_unreadable_config_toml_as_a_failure_not_an_absent_file() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "theme = \"x\"\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
        let mut cfg = Config::at(dir.path().to_path_buf());

        let (reloaded, warnings) = cfg.reload(false, &crate::keys::Keymap::default_bindings());

        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        if reloaded.config.is_some() {
            // Running as root: nothing is unreadable. Not a failure.
            return;
        }
        assert!(
            warnings.iter().any(|w| w.contains("config.toml")),
            "{warnings:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn reload_treats_an_unreadable_keys_toml_as_a_failure_not_an_absent_file() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempdir().unwrap();
        let path = dir.path().join("keys.toml");
        std::fs::write(&path, "save = \"alt+shift+s\"\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
        let mut cfg = Config::at(dir.path().to_path_buf());

        let (reloaded, warnings) = cfg.reload(false, &crate::keys::Keymap::default_bindings());

        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        if reloaded.keymap.is_some() {
            // Running as root: nothing is unreadable. Not a failure.
            return;
        }
        assert!(
            warnings.iter().any(|w| w.contains("keys.toml")),
            "{warnings:?}"
        );
    }

    /// A `themes/` that cannot be listed is not an empty `themes/`: the
    /// app must keep the registry it built at startup rather than lose
    /// every custom theme to a transient permission problem.
    #[cfg(unix)]
    #[test]
    fn reload_reports_an_unlistable_themes_dir_rather_than_emptying_the_registry() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempdir().unwrap();
        let themes = dir.path().join("themes");
        std::fs::create_dir_all(&themes).unwrap();
        std::fs::write(themes.join("good.toml"), THEME_TOML).unwrap();
        std::fs::set_permissions(&themes, std::fs::Permissions::from_mode(0o000)).unwrap();
        let mut cfg = Config::at(dir.path().to_path_buf());

        let (reloaded, warnings) = cfg.reload(false, &crate::keys::Keymap::default_bindings());

        std::fs::set_permissions(&themes, std::fs::Permissions::from_mode(0o755)).unwrap();
        if reloaded.themes.is_some() {
            // Running as root: nothing is unlistable. Not a failure.
            return;
        }
        assert!(
            warnings.iter().any(|w| w.contains("themes/")),
            "{warnings:?}"
        );
    }

    #[test]
    fn reload_on_config_none_is_all_defaults_no_warnings() {
        let mut cfg = Config::none();

        let (reloaded, warnings) = cfg.reload(false, &crate::keys::Keymap::default_bindings());

        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(
            reloaded.config,
            Some((ProjectsRegistry::default(), UiSettings::default()))
        );
        assert!(reloaded.keymap.is_some());
    }
}

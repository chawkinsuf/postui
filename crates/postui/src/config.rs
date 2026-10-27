//! Global project registry stored in the app's `config.toml`, under the
//! `[projects]` table: known project paths (cycle order), an optional
//! configured root directory, and the last-used project.

use std::ffi::OsString;
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
pub const CONFIG_TOML: &str = "config.toml";
pub const KEYS_TOML: &str = "keys.toml";
const UI_TOML: &str = "ui.toml";
const THEMES_DIR: &str = "themes";

/// The one place the "`config.toml` will not parse" sentence is written.
/// What was *done* about it differs by caller — startup drops to the
/// defaults, a reload keeps what it has — so each caller appends its own
/// clause rather than the sentence being written out again.
pub fn config_parse_error(e: &toml::de::Error) -> String {
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
            registry.known = known.iter().filter_map(|v| v.as_str()).map(expand_tilde).collect();
        }
        registry.root = projects.get("root").and_then(|v| v.as_str()).map(expand_tilde);
        registry.last = projects.get("last").and_then(|v| v.as_str()).map(expand_tilde);

        registry
    }

    /// Writes the registry into `doc`, touching only the `[projects]`
    /// table; every unrelated key is preserved byte-for-byte. The I/O
    /// around it (and the refusal to write over a file that doesn't
    /// parse) is [`Config::edit`]'s business.
    ///
    /// A path that isn't UTF-8 is never written (#167): TOML can only hold
    /// it lossily, under a name no directory has, which the next launch
    /// would open and every later save would add again. Such a project
    /// (only the command line opens one) isn't remembered: it is left out
    /// of `known`, and `last` keeps the file's, so the next launch reopens
    /// the project used before it. (`root` is only ever read from the
    /// file's text, so it is always UTF-8.)
    pub fn write_into(&self, doc: &mut toml_edit::DocumentMut) {
        let mut table = toml_edit::Table::new();

        let mut known = toml_edit::Array::new();
        for p in self.known.iter().filter_map(|p| p.to_str()) {
            known.push(p);
        }
        table["known"] = toml_edit::value(known);

        if let Some(root) = self.root.as_deref().and_then(Path::to_str) {
            table["root"] = toml_edit::value(root);
        }
        let last = match self.last.as_deref().map(Path::to_str) {
            Some(Some(last)) => Some(last.to_owned()),
            Some(None) => doc
                .get("projects")
                .and_then(|p| p.get("last"))
                .and_then(|v| v.as_str())
                .map(str::to_owned),
            None => None,
        };
        if let Some(last) = last {
            table["last"] = toml_edit::value(last);
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
/// caret and step through the rest in place (`ghost`; `cycle` is
/// accepted as its older name).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum JqTab {
    Cycle,
    #[default]
    Menu,
}

/// Which keys the app runs on (`keymap` in `config.toml`, piece 4 spec
/// §4.1). The two are equals: an unset `keymap` is `None` in
/// [`UiSettings`], never one of these. `Default` exists only so zeroed
/// context structs (`HintCtx::default()`) can be built; the running app
/// asks `App::profile`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum KeyProfile {
    /// Arrows and Enter: modeless, every verb on a named key, a chord or a
    /// shared letter.
    #[default]
    Arrows,
    /// Vim keys on lists, panes and fields.
    Vim,
}

impl KeyProfile {
    /// The `keymap` value written for this profile.
    pub fn spelling(self) -> &'static str {
        match self {
            KeyProfile::Arrows => "arrows",
            KeyProfile::Vim => "vim",
        }
    }

    /// The profile a `keymap` value names, or `None` for anything else.
    pub fn from_spelling(s: &str) -> Option<Self> {
        [KeyProfile::Arrows, KeyProfile::Vim].into_iter().find(|p| p.spelling() == s)
    }
}

/// What a body with no indentation of its own indents with (`indent` in
/// `config.toml`, spec 2026-10-15 R1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum IndentKind {
    #[default]
    Spaces,
    Tabs,
}

impl IndentKind {
    /// The `indent` value written for this kind.
    pub fn spelling(self) -> &'static str {
        match self {
            IndentKind::Spaces => "spaces",
            IndentKind::Tabs => "tabs",
        }
    }

    /// The kind an `indent` value names, or `None` for anything else.
    pub fn from_spelling(s: &str) -> Option<Self> {
        [IndentKind::Spaces, IndentKind::Tabs].into_iter().find(|k| k.spelling() == s)
    }
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
    /// The global contrast level every theme is resolved at
    /// (`theme_contrast`, spec 2026-10-16).
    pub theme_contrast: crate::theme::Contrast,
    /// Whether eased transitions (tab underline, hover, modal open, ...)
    /// play, or every animated value jumps straight to its target.
    pub animations: bool,
    /// Whether a hovered button explains itself on the footer row (see
    /// `crate::hint`). Off leaves the footer's chips alone under the
    /// pointer -- the hints are a teaching aid, and a user who knows the
    /// app may prefer the chip row to stay put.
    pub hover_hints: bool,
    /// Whether the sidebar watches the clipboard for curl and offers an
    /// "Import curl" button while it holds some (spec 2026-10-15 curl
    /// import visibility V13). Off by default: the watch reads the
    /// clipboard unasked, which macOS's pasteboard privacy may alert on.
    pub curl_button: bool,
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
    /// What a body with no indentation of its own indents with (`indent`,
    /// spec 2026-10-15 R1).
    pub indent: IndentKind,
    /// The spaces in one level of a space indent, 1..=8 (`indent_width`).
    /// Kept under `indent = "tabs"` too: a tab still displays 2 wide.
    pub indent_width: usize,
    /// The chosen key profile, or `None` while `config.toml` has never
    /// carried a `keymap` (the first-launch question's trigger).
    pub keymap: Option<KeyProfile>,
}

impl Default for UiSettings {
    fn default() -> Self {
        Self {
            clipboard_cmd: None,
            osc52_limit: 65536,
            theme: "terminal".into(),
            theme_contrast: crate::theme::Contrast::default(),
            animations: true,
            hover_hints: true,
            curl_button: false,
            anim_ms: AnimDurations::default(),
            ai_cmd: "claude -p".into(),
            ai_confirmed: false,
            jq_tab: JqTab::Menu,
            indent: IndentKind::Spaces,
            indent_width: 2,
            keymap: None,
        }
    }
}

impl UiSettings {
    /// The indent a body with none of its own uses (spec R1, R3).
    pub fn default_indent(&self) -> crate::text::indent::Indent {
        use crate::text::indent::Indent;
        match self.indent {
            IndentKind::Spaces => Indent::Spaces(self.indent_width),
            IndentKind::Tabs => Indent::Tabs,
        }
    }

    /// The key profile these settings run: the chosen one, or arrows while
    /// `keymap` is unset, until the first-launch question is raised (plan
    /// M1 Deviation 2; M3 raises it). The one place that rule lives.
    pub fn profile(&self) -> KeyProfile {
        self.keymap.unwrap_or(KeyProfile::Arrows)
    }

    /// Reads the top-level `clipboard_cmd` (string), `osc52_limit`
    /// (integer), `theme` (string), `animations` (bool), `hover_hints`
    /// (bool), `curl_button` (bool), `ai_cmd` (string), `ai_confirmed`
    /// (bool), `indent` (string), `indent_width` (integer), and `jq_tab`
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

        // A key that is present but of the wrong type falls back to its
        // default *and says so*: silent per-key defaulting is how a
        // customised app quietly becomes a default one.
        let mut wrong_type = |key: &str, expected: &str| {
            warnings.push(format!(
                "invalid value for {key:?} in config.toml (expected {expected}); using default"
            ));
        };

        match value.get("clipboard_cmd") {
            None => {}
            Some(v) => match v.as_str() {
                Some(cmd) => settings.clipboard_cmd = Some(cmd.to_string()),
                None => wrong_type("clipboard_cmd", "a string"),
            },
        }
        match value.get("osc52_limit") {
            None => {}
            Some(v) => match v.as_integer().and_then(|n| usize::try_from(n).ok()) {
                Some(limit) => settings.osc52_limit = limit,
                None => wrong_type("osc52_limit", "a non-negative integer"),
            },
        }
        match value.get("theme") {
            None => {}
            Some(v) => match v.as_str() {
                Some(raw) => settings.theme = raw.to_string(),
                None => wrong_type("theme", "a string"),
            },
        }
        match value.get("theme_contrast") {
            None => {}
            Some(v) => match v
                .as_integer()
                .and_then(|n| i8::try_from(n).ok())
                .and_then(crate::theme::Contrast::new)
            {
                Some(c) => settings.theme_contrast = c,
                None => wrong_type(
                    "theme_contrast",
                    &format!(
                        "an integer from {} to {}",
                        crate::theme::Contrast::MIN,
                        crate::theme::Contrast::MAX
                    ),
                ),
            },
        }
        match value.get("animations") {
            None => {}
            Some(v) => match v.as_bool() {
                Some(b) => settings.animations = b,
                None => wrong_type("animations", "a boolean"),
            },
        }
        match value.get("hover_hints") {
            None => {}
            Some(v) => match v.as_bool() {
                Some(b) => settings.hover_hints = b,
                None => wrong_type("hover_hints", "a boolean"),
            },
        }
        match value.get("curl_button") {
            None => {}
            Some(v) => match v.as_bool() {
                Some(b) => settings.curl_button = b,
                None => wrong_type("curl_button", "a boolean"),
            },
        }
        match value.get("ai_cmd") {
            None => {}
            Some(v) => match v.as_str() {
                Some(cmd) => settings.ai_cmd = cmd.to_string(),
                None => wrong_type("ai_cmd", "a string"),
            },
        }
        match value.get("ai_confirmed") {
            None => {}
            Some(v) => match v.as_bool() {
                Some(b) => settings.ai_confirmed = b,
                None => wrong_type("ai_confirmed", "a boolean"),
            },
        }
        match value.get("indent") {
            None => {}
            Some(v) => match v.as_str().and_then(IndentKind::from_spelling) {
                Some(kind) => settings.indent = kind,
                None => wrong_type("indent", "\"spaces\" or \"tabs\""),
            },
        }
        match value.get("indent_width") {
            None => {}
            Some(v) => match v.as_integer().and_then(|n| usize::try_from(n).ok()) {
                Some(w) if crate::text::indent::valid_width(w) => settings.indent_width = w,
                _ => wrong_type("indent_width", &crate::text::indent::width_range_text()),
            },
        }
        match value.get("jq_tab") {
            None => {}
            Some(v) => match v.as_str() {
                // "ghost" is the written spelling; "cycle" is its older name
                // and stays parseable forever, exactly as "accept" is kept as
                // the older name for "menu".
                Some("ghost" | "cycle") => settings.jq_tab = JqTab::Cycle,
                Some("menu" | "accept") => settings.jq_tab = JqTab::Menu,
                Some(other) => warnings.push(format!(
                    "invalid value {other:?} for jq_tab in config.toml \
                     (expected \"menu\" or \"ghost\", or the older name \"cycle\"); \
                     using \"menu\""
                )),
                None => wrong_type("jq_tab", "a string"),
            },
        }
        match value.get("keymap") {
            None => {}
            Some(v) => match v.as_str() {
                Some(s) if let Some(profile) = KeyProfile::from_spelling(s) => {
                    settings.keymap = Some(profile)
                }
                Some(other) => warnings.push(format!(
                    "invalid value {other:?} for keymap in config.toml \
                     (expected \"arrows\" or \"vim\"); leaving the choice unmade"
                )),
                // Not `wrong_type`: that one says "using default", and
                // `keymap` has no default to fall back to.
                None => warnings.push(
                    "invalid value for \"keymap\" in config.toml (expected a string); \
                     leaving the choice unmade"
                        .to_string(),
                ),
            },
        }

        if let Some(table) = value.get("animation_ms").and_then(|v| v.as_table()) {
            const KNOWN_KEYS: [&str; 8] = [
                "tab_slide",
                "hover",
                "focus",
                "list_travel",
                "modal_open",
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
            set_ms("pane_collapse", &mut settings.anim_ms.pane_collapse);
            set_ms("toast", &mut settings.anim_ms.toast);
            set_ms("send_breathe", &mut settings.anim_ms.send_breathe);

            for key in table.keys() {
                // Dropdowns used to grow in over `dropdown_open` ms; they
                // now open whole on their first frame. A config that still
                // sets the key gets told so, rather than a generic
                // "unknown key" that reads like a typo.
                if key == "dropdown_open" {
                    warnings.push(
                        "\"dropdown_open\" in [animation_ms] section of config.toml no \
                         longer has an effect: dropdowns open instantly"
                            .to_string(),
                    );
                } else if !KNOWN_KEYS.contains(&key.as_str()) {
                    warnings.push(format!(
                        "unknown key {key:?} in [animation_ms] section of config.toml"
                    ));
                }
            }
        }

        (settings, warnings)
    }
}

/// A fully commented `config.toml`, written when Edit… finds no file, so
/// the editor opens something self-documenting rather than an empty
/// buffer. Commented rather than live: a seed that *set* every value
/// would pin choices the user never made and starve them of any default
/// that ships later.
pub fn config_seed() -> String {
    let d = UiSettings::default();
    format!(
        "\
# postui settings. Uncomment a line to override its default.
# Anything not listed here keeps the app's default.

# The theme's name: a built-in, or the file stem of a themes/*.toml.
# theme = {:?}

# The theme's contrast, from {} (softer) to {} (crisper); 0 is the theme as
# made. Also set from the theme picker's Contrast row.
# theme_contrast = {}

# Whether eased transitions play at all (tab underline, hover, modals).
# animations = {}

# Whether a hovered control explains itself on the footer row.
# hover_hints = {}

# Whether the sidebar offers an \"Import curl\" button while the clipboard
# holds a curl command. It reads the clipboard when postui regains focus.
# curl_button = {}

# What Tab does in the jq bar while a completion is showing:
#   \"menu\"  - list the candidates below the bar, shell-style
#   \"ghost\" - ghost the best one after the caret and step in place
# jq_tab = \"menu\"

# Which keys the app runs on: \"arrows\" (arrows and Enter) or \"vim\".
# keymap = \"arrows\"

# How a body with no indentation of its own indents: \"spaces\" or \"tabs\".
# A body that is already indented keeps its own style.
# indent = {:?}

# The spaces in one indent level, from 1 to {}.
# indent_width = {}

# The shell command \"Describe a filter…\" pipes its prompt into.
# ai_cmd = {:?}

# Set once by the AI prompt's \"Always send\" choice. false asks again.
# ai_confirmed = {}

# An external clipboard command, when the built-in tiers do not suit.
# clipboard_cmd = \"xclip -selection clipboard\"

# The size in bytes above which OSC 52 copying is not attempted.
# osc52_limit = {}
",
        d.theme,
        crate::theme::Contrast::MIN,
        crate::theme::Contrast::MAX,
        d.theme_contrast.level(),
        d.animations,
        d.hover_hints,
        d.curl_button,
        d.indent.spelling(),
        crate::text::indent::MAX_WIDTH,
        d.indent_width,
        d.ai_cmd,
        d.ai_confirmed,
        d.osc52_limit,
    )
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
    /// The `:` line's history (plan M5b Task 5), from `ui.toml`.
    pub cmdline: crate::cmdline::CmdlineHistory,
    /// `Some` when `config.toml` exists but will not parse. Startup
    /// blocks on this rather than running on defaults: `[projects]`
    /// lives in the same file, so defaulting loses the project list and
    /// `registry.last`, which is what picks the project to open.
    pub config_error: Option<String>,
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
    /// The profile `keymap` was built on.
    profile: KeyProfile,
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
    /// The bare parse error when `config` is `None` because the file
    /// exists but will not parse — the same text [`Loaded::config_error`]
    /// carries at startup, so the Settings tab's banner can show it
    /// without hunting through `warnings` for the right sentence.
    pub config_error: Option<String>,
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
            Config { disk: postui_core::config_dir().map(postui_core::disk::Disk::new) },
            macos,
        )
    }

    /// [`Self::load`] with the config directory already resolved: the half
    /// that reads and words, split from the half that decides *where*, so
    /// startup's behaviour — which file failure yields which defaults and
    /// which sentence — can be tested against a tempdir instead of only
    /// against the user's real XDG directory.
    pub(crate) fn load_from(mut cfg: Config, macos: bool) -> (Config, Loaded, Vec<String>) {
        // Nothing runs yet: a `config.toml` that will not parse starts on
        // the default settings, so on the default profile's keys too.
        let (read, mut warnings) = cfg.read_all(KeyProfile::default());

        // Startup's answer to every failure is the same: run on the
        // defaults and say so. (A reload's answer is the opposite — see
        // [`Self::reload`] — which is exactly why `read_all` reports the
        // bare error and leaves the consequence to us.)
        let mut config_error = None;
        let (registry, ui) = match read.config {
            Ok(pair) => pair,
            Err(e) => {
                config_error = Some(e.clone());
                warnings.push(format!("{e}; using default settings"));
                (ProjectsRegistry::default(), UiSettings::default())
            }
        };
        let keymap = match read.keymap {
            Ok(keymap) => keymap,
            Err(e) => {
                warnings.push(format!("keys.toml ignored, using default keys: {e}"));
                crate::keys::Keymap::default_bindings_for(ui.profile())
            }
        };
        warnings.extend(keymap.caret_warnings(macos));
        if ui.keymap == Some(KeyProfile::Vim) {
            warnings.extend(keymap.vim_shadow_warnings());
        }
        let themes =
            read.themes.unwrap_or_else(|_| crate::theme::ThemeRegistry::from_files(Vec::new()).0);

        // `ui.toml` is the app's own state file rather than a config file
        // the user edits, so it is read here and *not* by `read_all` —
        // a reload has no business re-reading it (see [`Self::reload`]).
        let (usage, cmdline) = match cfg.read(UI_TOML) {
            Ok(Some(text)) => {
                let (usage, error) = crate::usage::UsageStore::parse(&text);
                if let Some(e) = error {
                    warnings.push(format!(
                        "could not parse {UI_TOML}: {e}; palette usage stats and the command-line history will not be saved until it is fixed"
                    ));
                }
                let (cmdline, warning) = crate::cmdline::CmdlineHistory::parse(&text);
                warnings.extend(warning.map(|w| format!("{UI_TOML}: {w}")));
                (usage, cmdline)
            }
            Ok(None) => {
                (crate::usage::UsageStore::default(), crate::cmdline::CmdlineHistory::default())
            }
            Err(e) => {
                warnings.push(e);
                // The file may be fine on disk; a quit save must not replace it.
                (crate::usage::UsageStore::default(), crate::cmdline::CmdlineHistory::unreadable())
            }
        };

        (cfg, Loaded { registry, ui, keymap, themes, usage, cmdline, config_error }, warnings)
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
        Config { disk: Some(postui_core::disk::Disk::new(dir)) }
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
    ///
    /// `fallback` is the profile `keys.toml` layers on when `config.toml`
    /// will not parse: the caller's, since only it knows which profile
    /// survives that failure.
    fn read_all(&mut self, fallback: KeyProfile) -> (ReadAll, Vec<String>) {
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

        // The chosen profile is the base `keys.toml` layers on. Unset runs
        // arrows (plan M1 Deviation 2); a `config.toml` that will not
        // parse runs the caller's `fallback` (Deviation 23).
        let profile = match &config {
            Ok((_, ui)) => ui.profile(),
            Err(_) => fallback,
        };
        let keymap = match self.read(KEYS_TOML) {
            Ok(Some(text)) => crate::keys::Keymap::try_from_overrides_for(profile, &text),
            Ok(None) => Ok(crate::keys::Keymap::default_bindings_for(profile)),
            Err(e) => Err(e),
        };

        (ReadAll { config, keymap, profile, themes }, warnings)
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
    /// caret-conflict and vim-shadow advisories are re-issued only when
    /// the file's bindings actually differ from it, so re-reading an
    /// unchanged keys.toml is silent rather than a nag. `running` is the
    /// profile the app runs: a `config.toml` that will not parse keeps
    /// it, so the keymap is built on it, and the advisories speak of the
    /// keymap the app will install (plan M1 Deviation 23).
    pub fn reload(
        &mut self,
        macos: bool,
        current_keys: &crate::keys::Keymap,
        running: KeyProfile,
    ) -> (Reloaded, Vec<String>) {
        let (read, mut warnings) = self.read_all(running);

        let mut config_error = None;
        let config = match read.config {
            Ok(pair) => Some(pair),
            Err(e) => {
                warnings.push(format!("{e}; keeping the current settings"));
                config_error = Some(e);
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
            if read.profile == KeyProfile::Vim {
                warnings.extend(keymap.vim_shadow_warnings());
            }
        }
        // A `themes/` that would not list already warned inside
        // `read_all`; here it just means "keep the registry you have".
        let themes = read.themes.ok();

        (Reloaded { config, config_error, keymap, themes }, warnings)
    }

    /// The file's text: `Ok(None)` when it (or the whole config dir)
    /// isn't there, `Err` when it is there but could not be read (a
    /// permission or I/O error, invalid UTF-8). The two are kept apart on
    /// purpose — an unreadable file is not an absent one, and only the
    /// caller knows whether "absent" is a safe thing to substitute.
    pub fn read(&mut self, name: &str) -> Result<Option<String>, String> {
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
        let existing = disk.read(&rel).map_err(|e| e.to_string())?.unwrap_or_default();
        let mut doc: toml_edit::DocumentMut =
            existing.parse().map_err(|e: toml_edit::TomlError| {
                format!("{name} has a syntax error and was left unchanged: {e}")
            })?;
        f(&mut doc);
        disk.write(&rel, &doc.to_string()).map_err(|e| e.to_string())
    }

    /// Persists the `[projects]` table of `config.toml`.
    pub fn save_registry(&mut self, registry: &ProjectsRegistry) -> Result<(), String> {
        self.edit(CONFIG_TOML, |doc| registry.write_into(doc))
    }

    /// Persists the top-level `theme` and `theme_contrast` keys of
    /// `config.toml` in one edit, so the two are never half-saved. Level 0
    /// removes `theme_contrast` rather than writing it, so picking a theme
    /// adds no line to a config that never chose a level (spec 2026-10-16
    /// C10).
    pub fn save_ui_theme(
        &mut self,
        name: &str,
        contrast: crate::theme::Contrast,
    ) -> Result<(), String> {
        self.edit(CONFIG_TOML, |doc| {
            doc["theme"] = toml_edit::value(name);
            match contrast.level() {
                0 => {
                    doc.remove("theme_contrast");
                }
                n => doc["theme_contrast"] = toml_edit::value(i64::from(n)),
            }
        })
    }

    /// Writes `text` to `name` verbatim and atomically, replacing whatever
    /// is there. The deliberate counterpart to [`Self::edit`]: `edit`
    /// parses first and refuses a file it cannot read, which is right for
    /// a structured key change but wrong here, where a validator has
    /// already accepted `text` and the user's own comments and spacing
    /// must survive byte-for-byte. Only for text that has just been
    /// validated by its own parser — never for unchecked input.
    pub fn write_validated(&mut self, name: &str, text: &str) -> Result<(), String> {
        let Some(disk) = self.disk.as_mut() else {
            return Ok(());
        };
        let rel = postui_core::disk::RelPath::new(name).map_err(|e| e.to_string())?;
        disk.write(&rel, text).map_err(|e| e.to_string())
    }

    /// Persists one top-level boolean of `config.toml` (the `ai_confirmed`
    /// "don't ask again" flag).
    pub fn save_ui_flag(&mut self, key: &str, value: bool) -> Result<(), String> {
        self.edit(CONFIG_TOML, |doc| doc[key] = toml_edit::value(value))
    }

    /// Persists one top-level string of `config.toml`. An empty `value`
    /// *removes* the key instead of writing `""`: the two string
    /// settings the Settings tab exposes are shell commands, and an
    /// empty one is not a command but a request to fall back to the
    /// default — an empty `clipboard_cmd` would otherwise run `sh -c ""`
    /// and swallow every copy in silence. Removing rather than writing a
    /// default is the same shape [`ProjectsRegistry::write_into`] uses.
    pub fn save_ui_string(&mut self, key: &str, value: &str) -> Result<(), String> {
        self.edit(CONFIG_TOML, |doc| {
            if value.is_empty() {
                doc.remove(key);
            } else {
                doc[key] = toml_edit::value(value)
            }
        })
    }

    /// Persists one top-level integer of `config.toml`. `config.toml`
    /// stores integers as TOML i64s, so a `value` past `i64::MAX` is
    /// refused here rather than silently written back as a negative
    /// number (the same rule `parse_osc52_limit` already enforces at the
    /// field's edit boundary; this is the write boundary's own copy of
    /// it, so any caller -- not just a validated field commit -- gets
    /// the same refusal).
    pub fn save_ui_int(&mut self, key: &str, value: usize) -> Result<(), String> {
        let value = i64::try_from(value)
            .map_err(|_| format!("too large to store; the most is {}", i64::MAX))?;
        self.edit(CONFIG_TOML, |doc| doc[key] = toml_edit::value(value))
    }

    /// Removes one top-level key of `config.toml`, so the setting falls
    /// back to its default: what an undo writes for a key the file did
    /// not carry before the change (task #195).
    pub fn remove_ui_key(&mut self, key: &str) -> Result<(), String> {
        self.edit(CONFIG_TOML, |doc| {
            doc.remove(key);
        })
    }

    /// Whether `config.toml` carries the top-level `key`: `Some(false)`
    /// when the file or the key is absent. `None` when that is not known:
    /// no files behind this `Config` ([`Self::none`]), or a file that will
    /// not read or parse (which [`Self::edit`] refuses to write anyway).
    /// A query, so it reads without recording a stamp (`Disk::peek`).
    pub fn holds_ui_key(&self, key: &str) -> Option<bool> {
        let disk = self.disk.as_ref()?;
        let rel = postui_core::disk::RelPath::new(CONFIG_TOML).ok()?;
        let text = disk.peek(&rel).ok()?.unwrap_or_default();
        let doc: toml_edit::DocumentMut = text.parse().ok()?;
        Some(doc.contains_key(key))
    }

    /// Clears the UI settings from `config.toml` by *removing* their
    /// keys, so the file stays minimal and every unrelated key survives
    /// byte-for-byte -- the pattern `ProjectsRegistry::write_into`
    /// already uses for `root` and `last`.
    ///
    /// `[projects]` is deliberately untouched: wiping `known`/`last`
    /// would silently destroy the user's project list, which has nothing
    /// to do with resetting preferences.
    ///
    /// `keymap` is not among the removed keys: a reset keeps the keyboard
    /// profile (plan M1 Deviation 25).
    pub fn reset_ui_settings(&mut self) -> Result<(), String> {
        self.edit(CONFIG_TOML, |doc| {
            for key in [
                "theme",
                "theme_contrast",
                "animations",
                "hover_hints",
                "curl_button",
                "jq_tab",
                "indent",
                "indent_width",
                "ai_cmd",
                "ai_confirmed",
                "clipboard_cmd",
                "osc52_limit",
            ] {
                doc.remove(key);
            }
            doc.remove("animation_ms");
        })
    }

    /// Persists the palette usage stats to `ui.toml`.
    pub fn save_usage(&mut self, usage: &crate::usage::UsageStore) -> Result<(), String> {
        self.edit(UI_TOML, |doc| usage.write_into(doc))
    }

    /// Persists the `:` line's history to `ui.toml` (plan M5b Deviation 16).
    /// A `[history]` that did not read at startup is left as it is.
    pub fn save_cmdline_history(
        &mut self,
        history: &crate::cmdline::CmdlineHistory,
    ) -> Result<(), String> {
        if !history.writable() {
            return Ok(());
        }
        self.edit(UI_TOML, |doc| history.write_into(doc))
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
            for entry in entries.into_iter().filter(|e| !e.is_dir && e.name.ends_with(".toml")) {
                let Ok(rel) = dir.join(&entry.name) else {
                    continue;
                };
                match disk.read(&rel) {
                    Ok(Some(text)) => files.push((entry.name, text)),
                    Ok(None) => {}
                    Err(e) => warnings
                        .push(format!("theme file {}: unreadable: {e}; skipped", entry.name)),
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

/// Result of parsing `postui`'s command-line arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CliParse {
    /// Run the app: a project directory (or none), tilde-expanded, and
    /// whether `--debug-ai` asked for the AI command's log.
    Root { root: Option<PathBuf>, debug_ai: bool },
    /// `--help` or `-h` on an otherwise valid line: print usage to stdout
    /// and exit 0.
    Help,
    /// A mistake (an unknown option, a second directory, ...), said in
    /// `reason`: it and the usage go to stderr and the exit status is 2
    /// (#144). A help flag on the line doesn't hide it.
    Usage { reason: String },
    /// `--setup`: print terminal keyboard-config guidance and exit.
    Setup,
    /// `--version` or `-V`: print `postui <version>` and exit.
    Version,
    /// `--keydump[=FLAGS]`: run the raw key-event echo loop and exit.
    /// `flags` is an explicit kitty-keyboard-protocol enhancement bitmask
    /// to push (`0` pushes nothing); `None` means "push what the app
    /// itself pushes", so a plain `--keydump` reproduces the app's exact
    /// input conditions.
    Keydump { flags: Option<u8> },
}

/// What `--help` prints to stdout, and a usage error to stderr after its
/// reason. Every flag [`parse_cli`] reads is listed (a test checks).
pub const USAGE: &str = "\
usage: postui [--debug-ai] [directory]
       postui --setup | --version | --keydump[=BITS] | --help

  --debug-ai        log every AI filter prompt and reply to a file
  --setup           terminal keyboard setup (macOS opt/cmd keys)
  --version, -V     print the version
  --keydump[=BITS]  echo raw key and mouse events (kitty flag bits; 0=none)
  --help, -h        print this
";

/// Parses `postui`'s arguments (without the program name). `--setup`,
/// `--version` and `--keydump` run alone; otherwise at most one project
/// directory and at most one `--debug-ai`, in either order. Any other
/// value starting with `-` (e.g. `-x`) is a usage error rather than a
/// project directory, since no real path starts with a dash without `./`.
/// `--help` (or `-h`) anywhere on a line that is otherwise valid asks for
/// help; it never turns a mistake into a success.
///
/// The arguments come as the OS gave them (#161). Each is matched and
/// named by its lossy text: a value that isn't UTF-8 holds a U+FFFD there,
/// which no flag spelling has, so it is never taken for a valid flag (a
/// `--keydump=` with such a value is a malformed one). The directory keeps
/// the original bytes.
pub fn parse_cli(args: Vec<OsString>) -> CliParse {
    let is_help = |a: &OsString| a == "--help" || a == "-h";
    let help = args.iter().any(is_help);
    let parsed = parse_cli_line(args.into_iter().filter(|a| !is_help(a)).collect());
    if help && !matches!(parsed, CliParse::Usage { .. }) { CliParse::Help } else { parsed }
}

/// [`parse_cli`] for a line with its help flags taken out.
fn parse_cli_line(args: Vec<OsString>) -> CliParse {
    let usage = |reason: String| CliParse::Usage { reason };
    if let [only] = args.as_slice()
        && let Some(parsed) = run_alone_flag(&only.to_string_lossy())
    {
        return parsed;
    }
    let mut root = None;
    let mut debug_ai = false;
    for os_arg in args {
        let arg = os_arg.to_string_lossy();
        if let Some(parsed) = run_alone_flag(&arg) {
            // A malformed one says so first: that is the mistake to fix.
            return match parsed {
                CliParse::Usage { .. } => parsed,
                _ => usage(format!("{arg} takes no other arguments")),
            };
        } else if arg == "--debug-ai" {
            if debug_ai {
                return usage("--debug-ai is given twice".into());
            }
            debug_ai = true;
        } else if arg.starts_with('-') {
            return usage(format!("unknown option '{arg}'"));
        } else if root.is_some() {
            return usage(format!("one directory at most; '{arg}' is a second"));
        } else {
            // A name that isn't UTF-8 is taken as written: the tilde
            // expansion is for a quoted `~`, and the shell has already
            // expanded a bare one.
            root = Some(match os_arg.to_str() {
                Some(s) => expand_tilde(s),
                None => PathBuf::from(os_arg),
            });
        }
    }
    CliParse::Root { root, debug_ai }
}

/// What a flag that runs alone (`--setup`, `--version`, `--keydump`)
/// parses as, a malformed `--keydump=BITS` included; `None` for any
/// other argument.
fn run_alone_flag(arg: &str) -> Option<CliParse> {
    Some(match arg {
        "--setup" => CliParse::Setup,
        "--version" | "-V" => CliParse::Version,
        "--keydump" => CliParse::Keydump { flags: None },
        _ => {
            let bits = arg.strip_prefix("--keydump=")?;
            match bits.parse::<u8>() {
                Ok(bits) => CliParse::Keydump { flags: Some(bits) },
                Err(_) => CliParse::Usage {
                    reason: format!("--keydump=BITS takes a number from 0 to 255, not '{bits}'"),
                },
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::tempdir;

    /// Startup must not silently swallow a broken config: the parse error
    /// travels out so the app can block on it. `[projects]` is in the same
    /// file, so defaulting here also loses the project list.
    #[test]
    fn startup_reports_a_config_that_will_not_parse() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("config.toml"), "not = [toml").unwrap();
        let (_cfg, loaded, _warnings) =
            Config::load_from(Config::at(dir.path().to_path_buf()), false);
        let err = loaded
            .config_error
            .expect("a config that will not parse must be reported, not defaulted");
        assert!(err.contains("config.toml"), "{err}");
    }

    #[test]
    fn startup_reports_nothing_for_a_valid_or_absent_config() {
        let dir = tempfile::tempdir().unwrap();
        let (_cfg, loaded, _) = Config::load_from(Config::at(dir.path().to_path_buf()), false);
        assert!(loaded.config_error.is_none(), "absent is not broken");

        std::fs::write(dir.path().join("config.toml"), "animations = false\n").unwrap();
        let (_cfg, loaded, _) = Config::load_from(Config::at(dir.path().to_path_buf()), false);
        assert!(loaded.config_error.is_none(), "valid is not broken");
        assert!(!loaded.ui.animations);
    }

    /// Plan M5b Deviations 3 and 16: the `:` history comes back from
    /// `ui.toml` at startup and goes back at quit, beside the usage stats.
    #[test]
    fn the_cmdline_history_round_trips_through_ui_toml() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("ui.toml"), "[history]\ncmdline = [\"w\", \"send\"]\n")
            .unwrap();
        let disk = || Config { disk: Some(postui_core::disk::Disk::new(dir.path().to_path_buf())) };
        let (mut cfg, loaded, warnings) = Config::load_from(disk(), false);
        assert!(!warnings.iter().any(|w| w.contains("ui.toml")), "{warnings:?}");
        assert_eq!(loaded.cmdline.lines(), ["w", "send"]);
        let mut history = loaded.cmdline;
        history.add("w");
        cfg.save_cmdline_history(&history).unwrap();
        cfg.save_usage(&crate::usage::UsageStore::default()).unwrap();
        let (_, loaded, _) = Config::load_from(disk(), false);
        assert_eq!(loaded.cmdline.lines(), ["send", "w"]);
    }

    /// Review Focus 4 (the user's rule: parse errors never default): a
    /// `[history]` that does not read is reported at startup and never
    /// written over; a `ui.toml` that does not parse names the history too.
    #[test]
    fn a_history_that_does_not_read_is_reported_and_never_overwritten() {
        let dir = tempdir().unwrap();
        let disk = || Config { disk: Some(postui_core::disk::Disk::new(dir.path().to_path_buf())) };
        let text = "[history]\ncmdline = 5\n";
        std::fs::write(dir.path().join("ui.toml"), text).unwrap();
        let (mut cfg, loaded, warnings) = Config::load_from(disk(), false);
        assert!(
            warnings.iter().any(|w| w.contains("command-line history will not be saved")),
            "{warnings:?}"
        );
        assert!(loaded.cmdline.lines().is_empty());
        let mut history = loaded.cmdline;
        history.add("w");
        cfg.save_cmdline_history(&history).unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("ui.toml")).unwrap(),
            text,
            "byte for byte"
        );
        std::fs::write(dir.path().join("ui.toml"), "not = [toml").unwrap();
        let (_, _, warnings) = Config::load_from(disk(), false);
        assert!(warnings.iter().any(|w| w.contains("command-line history")), "{warnings:?}");
    }

    /// A `ui.toml` that cannot be read at startup (here: a directory in its
    /// place) leaves the history unwritable, so a quit save cannot replace
    /// history the app never read.
    #[test]
    fn a_ui_toml_that_cannot_be_read_leaves_the_history_unwritable() {
        let dir = tempdir().unwrap();
        std::fs::create_dir(dir.path().join("ui.toml")).unwrap();
        let (_, loaded, warnings) = Config::load_from(Config::at(dir.path().to_path_buf()), false);
        assert!(warnings.iter().any(|w| w.contains("ui.toml")), "{warnings:?}");
        assert!(!loaded.cmdline.writable());
    }

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
        assert!(warnings.iter().any(|w| w.contains("could not parse")), "{warnings:?}");

        let mut cfg = Config { disk: Some(postui_core::disk::Disk::new(dir.path().to_path_buf())) };
        let mut r = r;
        r.register(PathBuf::from("/tmp/b"));
        assert!(cfg.save_registry(&r).is_err(), "the registry save refuses");
        assert!(cfg.save_ui_theme("light", crate::theme::Contrast::default()).is_err());
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
        let mut cfg = Config { disk: Some(postui_core::disk::Disk::new(dir.path().to_path_buf())) };
        let err = cfg.save_usage(&crate::usage::UsageStore::default()).unwrap_err();
        assert!(err.contains("ui.toml has a syntax error"), "{err}");
        assert_eq!(std::fs::read_to_string(dir.path().join("ui.toml")).unwrap(), "not = [toml");
    }

    /// Every save on a `Config` with no config dir behind it is a silent
    /// no-op, so a test run never touches the user's real files.
    #[test]
    fn config_none_saves_nothing_and_succeeds() {
        let mut cfg = Config::none();
        assert!(cfg.save_registry(&ProjectsRegistry::default()).is_ok());
        assert!(cfg.save_ui_theme("dusk", crate::theme::Contrast::default()).is_ok());
        assert!(cfg.save_ui_flag("ai_confirmed", true).is_ok());
        assert!(cfg.save_usage(&crate::usage::UsageStore::default()).is_ok());
        let (themes, warnings) = cfg.reload_themes();
        assert!(warnings.is_empty());
        assert_eq!(
            themes.expect("no config dir is not a listing failure").entries().len(),
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

        let mut cfg = Config { disk: Some(postui_core::disk::Disk::new(dir.path().to_path_buf())) };
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

        let mut cfg = Config { disk: Some(postui_core::disk::Disk::new(dir.path().to_path_buf())) };
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
        let mut cfg = Config { disk: Some(postui_core::disk::Disk::new(dir.path().to_path_buf())) };
        let (mut r, _) = ProjectsRegistry::parse(&std::fs::read_to_string(&p).unwrap());
        r.register(PathBuf::from("/tmp/a"));
        r.register(PathBuf::from("/tmp/b"));
        r.register(PathBuf::from("/tmp/a")); // dedup, but last updates
        r.root = Some(PathBuf::from("/tmp/root"));
        cfg.save_registry(&r).unwrap();

        let text = std::fs::read_to_string(&p).unwrap();
        assert!(text.contains("theme = \"dark\""), "unrelated key preserved: {text}");
        let (r2, _) = ProjectsRegistry::parse(&text);
        assert_eq!(r2.known, vec![PathBuf::from("/tmp/a"), PathBuf::from("/tmp/b")]);
        assert_eq!(r2.last, Some(PathBuf::from("/tmp/a")));
        assert_eq!(r2.root, Some(PathBuf::from("/tmp/root")));
    }

    /// #167: a path TOML can't hold as written (not UTF-8) is never saved,
    /// lossily or otherwise: it is left out of `known`, and `last` keeps
    /// the project used before it.
    #[cfg(unix)]
    #[test]
    fn save_leaves_out_a_path_that_is_not_utf8() {
        use std::os::unix::ffi::OsStringExt;
        let dir = tempdir().unwrap();
        let p = dir.path().join("config.toml");
        let mut cfg = Config { disk: Some(postui_core::disk::Disk::new(dir.path().to_path_buf())) };
        let raw = PathBuf::from(OsString::from_vec(b"/tmp/api-\xff".to_vec()));
        let (mut r, _) = ProjectsRegistry::parse("");
        r.register(PathBuf::from("/tmp/a"));
        cfg.save_registry(&r).unwrap();
        r.register(raw.clone());
        cfg.save_registry(&r).unwrap();
        let text = std::fs::read_to_string(&p).unwrap();
        assert!(!text.contains('\u{FFFD}'), "{text}");
        let (mut r2, _) = ProjectsRegistry::parse(&text);
        assert_eq!(r2.known, vec![PathBuf::from("/tmp/a")]);
        assert_eq!(r2.last, Some(PathBuf::from("/tmp/a")), "the project used before it");
        // Opened again, it stays out of the file.
        r2.register(raw);
        cfg.save_registry(&r2).unwrap();
        let (r3, _) = ProjectsRegistry::parse(&std::fs::read_to_string(&p).unwrap());
        assert_eq!(r3.known, vec![PathBuf::from("/tmp/a")]);
        assert_eq!(r3.last, Some(PathBuf::from("/tmp/a")));
    }

    #[test]
    fn next_after_cycles_and_wraps() {
        let (mut r, _) = ProjectsRegistry::parse("");
        assert!(r.next_after(&PathBuf::from("/tmp/a")).is_none(), "fewer than two projects");
        let a = tempdir().unwrap();
        let b = tempdir().unwrap();
        let c = tempdir().unwrap();
        r.register(a.path().to_path_buf());
        r.register(b.path().to_path_buf());
        r.register(c.path().to_path_buf());
        assert_eq!(r.next_after(b.path()), Some(c.path().to_path_buf()));
        assert_eq!(r.next_after(c.path()), Some(a.path().to_path_buf()), "wraps");
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
        assert!(warnings[0].contains("using default settings"), "{warnings:?}");
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
        let mut cfg = Config { disk: Some(postui_core::disk::Disk::new(dir.path().to_path_buf())) };
        let zero = crate::theme::Contrast::default();
        cfg.save_ui_theme("catppuccin-mocha", zero).unwrap();
        let text = std::fs::read_to_string(&p).unwrap();
        assert!(text.contains("theme = \"catppuccin-mocha\""), "{text}");
        assert!(text.contains("clipboard_cmd = \"xclip\""), "{text}");
        assert!(text.contains("[projects]"), "{text}");
        assert_eq!(UiSettings::parse(&text).0.theme, "catppuccin-mocha");
        cfg.save_ui_theme("dark", zero).unwrap();
        let text = std::fs::read_to_string(&p).unwrap();
        assert_eq!(UiSettings::parse(&text).0.theme, "dark", "overwrites an existing key");
    }

    /// Spec 2026-10-16 C11: an integer from -3 to 3; anything else warns
    /// and keeps the default 0.
    #[test]
    fn theme_contrast_parses_in_range_and_warns_otherwise() {
        let (s, warnings) = UiSettings::parse("theme_contrast = -2\n");
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(s.theme_contrast.level(), -2);
        for bad in ["9", "-4", "\"high\"", "1.5"] {
            let (s, warnings) = UiSettings::parse(&format!("theme_contrast = {bad}\n"));
            assert_eq!(s.theme_contrast, crate::theme::Contrast::default(), "{bad}");
            assert_eq!(warnings.len(), 1, "{bad}: {warnings:?}");
            assert!(warnings[0].contains("theme_contrast"), "{warnings:?}");
            assert!(warnings[0].contains("-3 to 3"), "{warnings:?}");
        }
    }

    /// Spec C10: theme and level land in one edit, and level 0 removes
    /// the key rather than writing it.
    #[test]
    fn save_ui_theme_writes_the_contrast_and_removes_it_at_zero() {
        use crate::theme::Contrast;
        let dir = tempdir().unwrap();
        let p = dir.path().join("config.toml");
        std::fs::write(&p, "animations = false\n").unwrap();
        let mut cfg = Config::at(dir.path().to_path_buf());
        cfg.save_ui_theme("dark", Contrast::new(2).unwrap()).unwrap();
        let text = std::fs::read_to_string(&p).unwrap();
        let (s, _) = UiSettings::parse(&text);
        assert_eq!((s.theme.as_str(), s.theme_contrast.level()), ("dark", 2), "{text}");
        cfg.save_ui_theme("light", Contrast::default()).unwrap();
        let text = std::fs::read_to_string(&p).unwrap();
        assert!(!text.contains("theme_contrast"), "{text}");
        assert!(text.contains("animations = false"), "{text}");
        assert_eq!(UiSettings::parse(&text).0.theme, "light");
    }

    #[test]
    fn save_ui_theme_creates_a_missing_file() {
        let dir = tempdir().unwrap();
        let mut cfg = Config { disk: Some(postui_core::disk::Disk::new(dir.path().join("sub"))) };
        cfg.save_ui_theme("light", crate::theme::Contrast::default()).unwrap();
        let text = std::fs::read_to_string(dir.path().join("sub").join("config.toml")).unwrap();
        assert_eq!(UiSettings::parse(&text).0.theme, "light");
    }

    /// Reset clears preferences, not the project list: `[projects]` lives in
    /// the same file, and wiping `known`/`last` would silently destroy the
    /// user's projects.
    #[test]
    fn resetting_settings_spares_the_projects_table() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("config.toml"),
            "animations = false\nai_confirmed = true\ntheme_contrast = 2\n\n[projects]\nknown = [\"/tmp/a\"]\nlast = \"/tmp/a\"\n",
        )
        .unwrap();
        let mut cfg = Config::at(dir.path().to_path_buf());
        cfg.reset_ui_settings().unwrap();

        let text = std::fs::read_to_string(dir.path().join("config.toml")).unwrap();
        let (settings, _) = UiSettings::parse(&text);
        assert_eq!(settings, UiSettings::default(), "preferences are back to default");
        assert!(settings.animations, "removed, so the default applies");
        assert!(!settings.ai_confirmed, "consent returns; that is correct");

        let (registry, _) = ProjectsRegistry::parse(&text);
        assert_eq!(registry.known, vec![std::path::PathBuf::from("/tmp/a")]);
        assert_eq!(registry.last, Some(std::path::PathBuf::from("/tmp/a")));
    }

    /// The editor round-trip has already validated the text and must
    /// preserve it exactly -- comments included, which `edit`'s
    /// parse-and-reserialise would not guarantee.
    #[test]
    fn write_validated_preserves_text_byte_for_byte() {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = Config::at(dir.path().to_path_buf());
        let text = "# my settings\n\nanimations = false  # off on purpose\n";
        cfg.write_validated(CONFIG_TOML, text).unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("config.toml")).unwrap(),
            text,
            "comments and spacing must survive"
        );
    }

    /// Unlike `edit`, it does not re-parse: the caller's validator is the
    /// gate, and refusing here would make a deliberate rewrite impossible.
    #[test]
    fn write_validated_overwrites_a_file_that_did_not_parse() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("config.toml"), "not = [toml").unwrap();
        let mut cfg = Config::at(dir.path().to_path_buf());
        cfg.write_validated(CONFIG_TOML, "animations = true\n").unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("config.toml")).unwrap(),
            "animations = true\n"
        );
    }

    #[test]
    fn write_validated_without_a_config_dir_is_a_silent_ok() {
        let mut cfg = Config::none();
        assert!(cfg.write_validated(CONFIG_TOML, "animations = true\n").is_ok());
    }

    /// The seed documents every setting without setting any of them: an
    /// uncommented seed would pin values the user never chose.
    #[test]
    fn the_config_seed_is_entirely_commented_and_parses_as_empty() {
        let seed = config_seed();
        for line in seed.lines() {
            let line = line.trim();
            assert!(
                line.is_empty() || line.starts_with('#'),
                "seed line is live, not commented: {line:?}"
            );
        }
        let (settings, warnings) = UiSettings::parse(&seed);
        assert_eq!(settings, UiSettings::default(), "a seed changes nothing");
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    #[test]
    fn the_config_seed_names_every_setting() {
        let seed = config_seed();
        for key in [
            "animations",
            "hover_hints",
            "curl_button",
            "jq_tab",
            "ai_cmd",
            "ai_confirmed",
            "clipboard_cmd",
            "osc52_limit",
            "theme",
            "theme_contrast",
            "indent",
            "indent_width",
        ] {
            assert!(seed.contains(key), "{key} missing from the seed");
        }
    }

    #[test]
    fn indent_keys_parse_and_combine() {
        let (ui, warnings) = UiSettings::parse("indent = \"tabs\"\nindent_width = 4\n");
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(ui.indent, IndentKind::Tabs);
        assert_eq!(ui.indent_width, 4);
        assert_eq!(ui.default_indent(), crate::text::indent::Indent::Tabs);
        let (ui, _) = UiSettings::parse("indent = \"spaces\"\nindent_width = 4\n");
        assert_eq!(ui.default_indent(), crate::text::indent::Indent::Spaces(4));
        let (ui, _) = UiSettings::parse("");
        assert_eq!(ui.default_indent(), crate::text::indent::Indent::Spaces(2));
    }

    #[test]
    fn bad_indent_values_warn_and_default() {
        for text in [
            "indent = 3",
            "indent = \"tab\"",
            "indent_width = \"4\"",
            "indent_width = 0",
            "indent_width = 9",
            "indent_width = -1",
        ] {
            let (ui, warnings) = UiSettings::parse(text);
            assert_eq!(warnings.len(), 1, "{text}: {warnings:?}");
            assert_eq!(ui.indent, IndentKind::Spaces, "{text}");
            assert_eq!(ui.indent_width, 2, "{text}");
        }
    }

    #[test]
    fn the_seed_documents_the_indent_keys() {
        let seed = config_seed();
        assert!(seed.contains("# indent = \"spaces\""), "{seed}");
        assert!(seed.contains("# indent_width = 2"), "{seed}");
    }

    #[test]
    fn curl_button_key_parses_and_defaults_off() {
        let (s, warnings) = UiSettings::parse("curl_button = true\n");
        assert!(s.curl_button);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert!(!UiSettings::default().curl_button);
        let (s, warnings) = UiSettings::parse("curl_button = 1\n");
        assert!(!s.curl_button);
        assert_eq!(warnings.len(), 1, "{warnings:?}");
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
        assert_eq!(UiSettings::parse("jq_tab = \"menu\"\n").0.jq_tab, JqTab::Menu);
        let (s, warnings) = UiSettings::parse("jq_tab = \"popup\"\n");
        assert_eq!(s.jq_tab, JqTab::Menu, "a bad value falls back");
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("jq_tab"), "{warnings:?}");
    }

    #[test]
    fn a_mistyped_key_warns_instead_of_silently_defaulting() {
        let (settings, warnings) = UiSettings::parse("animations = \"yes\"\n");
        assert!(settings.animations, "falls back to the default");
        assert!(
            warnings.iter().any(|w| w.contains("animations") && w.contains("boolean")),
            "the fallback must name the key and the expected type: {warnings:?}"
        );
    }

    #[test]
    fn every_wrong_typed_key_warns() {
        let text = "\
clipboard_cmd = 1
osc52_limit = \"big\"
theme = true
animations = \"yes\"
hover_hints = 3
ai_cmd = []
ai_confirmed = \"yes\"
";
        let (_, warnings) = UiSettings::parse(text);
        for key in [
            "clipboard_cmd",
            "osc52_limit",
            "theme",
            "animations",
            "hover_hints",
            "ai_cmd",
            "ai_confirmed",
        ] {
            assert!(
                warnings.iter().any(|w| w.contains(key)),
                "{key} defaulted with no warning: {warnings:?}"
            );
        }
    }

    #[test]
    fn ghost_and_cycle_are_the_same_mode() {
        let (ghost, w1) = UiSettings::parse("jq_tab = \"ghost\"\n");
        let (cycle, w2) = UiSettings::parse("jq_tab = \"cycle\"\n");
        assert_eq!(ghost.jq_tab, JqTab::Cycle);
        assert_eq!(cycle.jq_tab, JqTab::Cycle);
        assert!(w1.is_empty() && w2.is_empty(), "{w1:?} {w2:?}");
    }

    #[test]
    fn an_unknown_jq_tab_names_every_accepted_spelling() {
        let (settings, warnings) = UiSettings::parse("jq_tab = \"nope\"\n");
        assert_eq!(settings.jq_tab, JqTab::Menu);
        let w = warnings.join(" ");
        for spelling in ["menu", "ghost", "cycle"] {
            assert!(w.contains(spelling), "{spelling} not named: {w}");
        }
    }

    #[test]
    fn a_valid_file_produces_no_warnings() {
        let text = "\
animations = false
hover_hints = false
jq_tab = \"ghost\"
ai_cmd = \"claude -p\"
osc52_limit = 1024
";
        let (_, warnings) = UiSettings::parse(text);
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    #[test]
    fn save_ui_flag_sets_the_key_and_preserves_unrelated_content() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("config.toml");
        std::fs::write(&p, "theme = \"x\"\n\n[projects]\nknown = []\n").unwrap();
        let mut cfg = Config { disk: Some(postui_core::disk::Disk::new(dir.path().to_path_buf())) };
        cfg.save_ui_flag("ai_confirmed", true).unwrap();
        let text = std::fs::read_to_string(&p).unwrap();
        assert!(text.contains("ai_confirmed = true"), "{text}");
        assert!(text.contains("theme = \"x\"") && text.contains("[projects]"), "{text}");
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
                pane_collapse: Duration::from_millis(7),
                toast: Duration::from_millis(8),
                send_breathe: Duration::from_millis(9),
            }
        );
        assert!(warnings.is_empty());
    }

    #[test]
    fn animation_ms_retired_dropdown_open_key_warns_specifically() {
        let (s, warnings) = UiSettings::parse("[animation_ms]\ndropdown_open = 90\n");
        assert_eq!(s.anim_ms, AnimDurations::default());
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("dropdown_open"), "{warnings:?}");
        assert!(warnings[0].contains("no longer"), "{warnings:?}");
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

    fn cli(args: &[&str]) -> CliParse {
        parse_cli(args.iter().map(OsString::from).collect())
    }

    /// #161: an argument that isn't UTF-8 is never a flag. Without a dash
    /// it is the directory, its bytes kept; with one it is an unknown
    /// option, said lossily. Either way the line's other mistakes still
    /// count.
    #[cfg(unix)]
    #[test]
    fn parse_cli_takes_a_non_utf8_argument_as_a_path() {
        use std::os::unix::ffi::OsStringExt;
        let raw = |b: &[u8]| OsString::from_vec(b.to_vec());
        assert_eq!(
            parse_cli(vec![raw(b"api-\xff")]),
            CliParse::Root { root: Some(PathBuf::from(raw(b"api-\xff"))), debug_ai: false }
        );
        assert_eq!(
            parse_cli(vec!["--debug-ai".into(), raw(b"\xff")]),
            CliParse::Root { root: Some(PathBuf::from(raw(b"\xff"))), debug_ai: true }
        );
        let reason = |args| match parse_cli(args) {
            CliParse::Usage { reason } => reason,
            other => panic!("parsed as {other:?}"),
        };
        assert_eq!(reason(vec![raw(b"-\xff")]), "unknown option '-\u{FFFD}'");
        assert_eq!(
            reason(vec![raw(b"\xff"), "b".into()]),
            "one directory at most; 'b' is a second"
        );
        assert_eq!(
            reason(vec!["a".into(), raw(b"\xff")]),
            "one directory at most; '\u{FFFD}' is a second"
        );
        assert_eq!(parse_cli(vec![raw(b"\xff"), "-h".into()]), CliParse::Help);
    }

    /// The reason a usage error gives; panics on anything else.
    fn mistake(args: &[&str]) -> String {
        match cli(args) {
            CliParse::Usage { reason } => reason,
            other => panic!("{args:?} parsed as {other:?}"),
        }
    }

    #[test]
    fn parse_cli_leading_dash_is_usage() {
        assert_eq!(mistake(&["-x"]), "unknown option '-x'");
        assert_eq!(mistake(&["/abs/x", "-x"]), "unknown option '-x'");
    }

    /// #144: help asked for is not a mistake (stdout, exit 0), wherever it
    /// sits on an otherwise valid line; on a line with a mistake the
    /// mistake wins, so a script still sees it.
    #[test]
    fn parse_cli_help_flag() {
        assert_eq!(cli(&["--help"]), CliParse::Help);
        assert_eq!(cli(&["-h"]), CliParse::Help);
        assert_eq!(cli(&["--help", "/abs/x"]), CliParse::Help);
        assert_eq!(cli(&["--debug-ai", "-h"]), CliParse::Help);
        assert_eq!(cli(&["--version", "-h"]), CliParse::Help);
        assert_eq!(mistake(&["-x", "--help"]), "unknown option '-x'");
        assert_eq!(mistake(&["a", "b", "-h"]), "one directory at most; 'b' is a second");
        assert_eq!(mistake(&["--help-x"]), "unknown option '--help-x'");
    }

    /// The usage names every flag `parse_cli` reads, each as a whole word.
    #[test]
    fn usage_lists_every_flag() {
        let words: Vec<&str> = USAGE
            .split(|c: char| c.is_whitespace() || "[]|,".contains(c))
            .map(|w| w.split('=').next().unwrap_or(w))
            .collect();
        for flag in ["--help", "-h", "--setup", "--version", "-V", "--keydump", "--debug-ai"] {
            assert!(words.contains(&flag), "{flag} missing from:\n{USAGE}");
        }
    }

    #[test]
    fn parse_cli_setup_flag() {
        assert_eq!(cli(&["--setup"]), CliParse::Setup);
        // Not a prefix match: anything else dashed is still usage.
        assert_eq!(mistake(&["--setup-x"]), "unknown option '--setup-x'");
        // It runs alone.
        assert_eq!(mistake(&["--setup", "/abs/x"]), "--setup takes no other arguments");
        assert_eq!(mistake(&["--debug-ai", "--setup"]), "--setup takes no other arguments");
    }

    #[test]
    fn parse_cli_version_flag() {
        assert_eq!(cli(&["--version"]), CliParse::Version);
        assert_eq!(cli(&["-V"]), CliParse::Version);
        // Exact spellings only, and it runs alone.
        assert_eq!(mistake(&["-v"]), "unknown option '-v'");
        assert_eq!(mistake(&["--version-x"]), "unknown option '--version-x'");
        assert_eq!(mistake(&["--version", "/abs/x"]), "--version takes no other arguments");
        assert_eq!(mistake(&["--debug-ai", "-V"]), "-V takes no other arguments");
    }

    #[test]
    fn parse_cli_keydump_flag_with_optional_bitmask() {
        assert_eq!(cli(&["--keydump"]), CliParse::Keydump { flags: None });
        assert_eq!(cli(&["--keydump=0"]), CliParse::Keydump { flags: Some(0) });
        assert_eq!(cli(&["--keydump=5"]), CliParse::Keydump { flags: Some(5) });
        // A malformed bitmask is usage, not a silent default.
        let bits = |b: &str| format!("--keydump=BITS takes a number from 0 to 255, not '{b}'");
        assert_eq!(mistake(&["--keydump=x"]), bits("x"));
        assert_eq!(mistake(&["--keydump="]), bits(""));
        assert_eq!(mistake(&["--keydump=300"]), bits("300"));
        assert_eq!(mistake(&["--keydump-x"]), "unknown option '--keydump-x'");
        assert_eq!(mistake(&["--keydump", "/abs/x"]), "--keydump takes no other arguments");
        // A malformed one beside another argument names the value first.
        assert_eq!(mistake(&["--keydump=abc", "/abs/x"]), bits("abc"));
    }

    #[test]
    fn parse_cli_normal_path_and_none() {
        assert_eq!(
            cli(&["/abs/x"]),
            CliParse::Root { root: Some(PathBuf::from("/abs/x")), debug_ai: false }
        );
        assert_eq!(cli(&[]), CliParse::Root { root: None, debug_ai: false });
        // One directory at most.
        assert_eq!(mistake(&["/abs/x", "/abs/y"]), "one directory at most; '/abs/y' is a second");
    }

    #[test]
    fn parse_cli_debug_ai_goes_either_side_of_the_directory() {
        let with_root = CliParse::Root { root: Some(PathBuf::from("/abs/x")), debug_ai: true };
        assert_eq!(cli(&["--debug-ai", "/abs/x"]), with_root);
        assert_eq!(cli(&["/abs/x", "--debug-ai"]), with_root);
        assert_eq!(cli(&["--debug-ai"]), CliParse::Root { root: None, debug_ai: true });
        // Once, like the directory.
        assert_eq!(mistake(&["--debug-ai", "--debug-ai"]), "--debug-ai is given twice");
    }

    #[test]
    fn tilde_expansion() {
        let home = directories::BaseDirs::new().unwrap().home_dir().to_path_buf();
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

        let (reloaded, warnings) =
            cfg.reload(false, &crate::keys::Keymap::default_bindings(), KeyProfile::Arrows);

        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(reloaded.config.as_ref().unwrap().1.theme, "x");
        assert_eq!(
            reloaded.config.unwrap().0.known,
            vec![PathBuf::from("/tmp/a"), PathBuf::from("/tmp/b")]
        );
    }

    /// Task #195: what an undo step reads and writes for a key the file
    /// did not carry.
    #[test]
    fn holds_ui_key_tells_a_carried_key_from_an_absent_one_and_remove_drops_it() {
        assert_eq!(Config::none().holds_ui_key("ai_cmd"), None, "no files: unknown");

        let dir = tempdir().unwrap();
        let mut cfg = Config::at(dir.path().to_path_buf());
        assert_eq!(cfg.holds_ui_key("ai_cmd"), Some(false), "no file: absent");

        std::fs::write(
            dir.path().join("config.toml"),
            "theme = \"y\"\nai_cmd = \"x\"\nanimations = false\n",
        )
        .unwrap();
        assert_eq!(cfg.holds_ui_key("ai_cmd"), Some(true));
        assert_eq!(cfg.holds_ui_key("hover_hints"), Some(false));

        cfg.remove_ui_key("ai_cmd").unwrap();
        let text = std::fs::read_to_string(dir.path().join("config.toml")).unwrap();
        assert_eq!(text, "theme = \"y\"\nanimations = false\n", "only the key went");

        std::fs::write(dir.path().join("config.toml"), "ai_cmd = [").unwrap();
        assert_eq!(cfg.holds_ui_key("ai_cmd"), None, "unparseable: unknown");
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

        let (reloaded, warnings) =
            cfg.reload(false, &crate::keys::Keymap::default_bindings(), KeyProfile::Arrows);

        assert!(reloaded.config.is_none());
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("config.toml"), "{warnings:?}");
        assert!(warnings[0].contains("keeping"), "{warnings:?}");
    }

    #[test]
    fn reload_surfaces_a_ui_settings_warning_from_otherwise_valid_toml() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("config.toml"), "theme = \"x\"\njq_tab = \"bogus\"\n")
            .unwrap();
        let mut cfg = Config::at(dir.path().to_path_buf());

        let (reloaded, warnings) =
            cfg.reload(false, &crate::keys::Keymap::default_bindings(), KeyProfile::Arrows);

        assert!(reloaded.config.is_some(), "valid TOML still parses");
        assert!(warnings.iter().any(|w| w.contains("jq_tab")), "{warnings:?}");
    }

    #[test]
    fn reload_missing_config_toml_is_defaults_with_no_warnings() {
        let dir = tempdir().unwrap();
        let mut cfg = Config::at(dir.path().to_path_buf());

        let (reloaded, warnings) =
            cfg.reload(false, &crate::keys::Keymap::default_bindings(), KeyProfile::Arrows);

        assert_eq!(reloaded.config, Some((ProjectsRegistry::default(), UiSettings::default())));
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    #[test]
    fn reload_applies_a_good_keys_toml() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("keys.toml"), "save = \"alt+shift+s\"\n").unwrap();
        let mut cfg = Config::at(dir.path().to_path_buf());

        let (reloaded, _warnings) =
            cfg.reload(false, &crate::keys::Keymap::default_bindings(), KeyProfile::Arrows);

        let keymap = reloaded.keymap.expect("good keys.toml parses");
        let combo = crate::keys::KeyCombo::parse("alt+shift+s").unwrap();
        assert_eq!(keymap.lookup(&combo), Some(crate::action::Action::SaveRequest));
    }

    #[test]
    fn reload_reports_a_bad_keys_toml_and_keeps_current_keys() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("keys.toml"), "save = \"not-a-combo\"\n").unwrap();
        let mut cfg = Config::at(dir.path().to_path_buf());

        let (reloaded, warnings) =
            cfg.reload(false, &crate::keys::Keymap::default_bindings(), KeyProfile::Arrows);

        assert!(reloaded.keymap.is_none());
        assert!(
            warnings.iter().any(|w| w.contains("keys.toml") && w.contains("keeping")),
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

        let (reloaded, _warnings) =
            cfg.reload(false, &crate::keys::Keymap::default_bindings(), KeyProfile::Arrows);

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
        assert!(warnings[0].starts_with("keys.toml ignored, using default keys: "), "{warnings:?}");
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

        let (first, warnings) =
            cfg.reload(true, &crate::keys::Keymap::default_bindings(), KeyProfile::Arrows);
        let keymap = first.keymap.expect("the file parsed");
        assert!(
            warnings.iter().any(|w| w.contains("shadows")),
            "the new bindings are worth one warning: {warnings:?}"
        );

        // Same file, and the app is already running on it.
        let (_, warnings) = cfg.reload(true, &keymap, KeyProfile::Arrows);
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

        let (reloaded, warnings) =
            cfg.reload(false, &crate::keys::Keymap::default_bindings(), KeyProfile::Arrows);

        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        if reloaded.config.is_some() {
            // Running as root: nothing is unreadable. Not a failure.
            return;
        }
        assert!(warnings.iter().any(|w| w.contains("config.toml")), "{warnings:?}");
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

        let (reloaded, warnings) =
            cfg.reload(false, &crate::keys::Keymap::default_bindings(), KeyProfile::Arrows);

        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        if reloaded.keymap.is_some() {
            // Running as root: nothing is unreadable. Not a failure.
            return;
        }
        assert!(warnings.iter().any(|w| w.contains("keys.toml")), "{warnings:?}");
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

        let (reloaded, warnings) =
            cfg.reload(false, &crate::keys::Keymap::default_bindings(), KeyProfile::Arrows);

        std::fs::set_permissions(&themes, std::fs::Permissions::from_mode(0o755)).unwrap();
        if reloaded.themes.is_some() {
            // Running as root: nothing is unlistable. Not a failure.
            return;
        }
        assert!(warnings.iter().any(|w| w.contains("themes/")), "{warnings:?}");
    }

    #[test]
    fn reload_on_config_none_is_all_defaults_no_warnings() {
        let mut cfg = Config::none();

        let (reloaded, warnings) =
            cfg.reload(false, &crate::keys::Keymap::default_bindings(), KeyProfile::Arrows);

        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(reloaded.config, Some((ProjectsRegistry::default(), UiSettings::default())));
        assert!(reloaded.keymap.is_some());
    }

    #[test]
    fn keymap_parses_both_profiles_and_defaults_to_unset() {
        assert_eq!(UiSettings::parse("").0.keymap, None, "never asked");
        let (ui, w) = UiSettings::parse("keymap = \"vim\"\n");
        assert_eq!((ui.keymap, w.len()), (Some(KeyProfile::Vim), 0));
        let (ui, w) = UiSettings::parse("keymap = \"arrows\"\n");
        assert_eq!((ui.keymap, w.len()), (Some(KeyProfile::Arrows), 0));
    }

    /// vim-mode spelled the arrows profile "default"; there is no default
    /// any more, so that spelling is as unknown as any other (spec §4.1).
    #[test]
    fn an_unknown_keymap_warns_and_leaves_the_choice_unmade() {
        for text in ["keymap = \"emacs\"\n", "keymap = \"default\"\n", "keymap = 3\n"] {
            let (ui, w) = UiSettings::parse(text);
            assert_eq!(ui.keymap, None, "{text}");
            assert!(
                w.iter().any(|w| w.contains("keymap") && w.contains("leaving the choice unmade")),
                "{text}: {w:?}"
            );
        }
    }

    #[test]
    fn keys_toml_layers_on_the_chosen_profile() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("config.toml"), "keymap = \"vim\"\n").unwrap();
        std::fs::write(dir.path().join("keys.toml"), "save = [\"ctrl+s\", \"f2\"]\n").unwrap();
        let (_cfg, loaded, _w) = Config::load_from(Config::at(dir.path().to_path_buf()), false);
        let get = |s: &str| loaded.keymap.lookup(&crate::keys::KeyCombo::parse(s).unwrap());
        assert_eq!(get("f2"), Some(crate::action::Action::SaveRequest), "keys.toml applied");
        assert_eq!(get("q"), None, "on the vim profile's defaults");
    }
}

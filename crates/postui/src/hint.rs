//! Hover hints for the footer (the TUI's stand-in for tooltips).
//!
//! A popup tooltip is too much furniture for the many small controls a
//! terminal UI packs in, so a hovered button explains itself on the
//! footer's content row instead — one short line, painted between the
//! per-pane chips and the right-hand commands/quit pair (see
//! `footer::draw_footer`). [`hint_for`] is the single table: it maps the
//! `Hit` under the pointer to that line, or to nothing for surfaces that
//! aren't buttons (panes, rows, cells, text inputs, scrollbars).
//!
//! The match is deliberately exhaustive — no `_ =>` arm — so adding a
//! `Hit` variant is a compile error until it has decided whether it is a
//! button with a hint or a surface without one.

use crate::action::{Action, CopyTarget, ExtractDestination, KeymapAnswer};
use crate::components::palette::all_commands;
use crate::hit::Hit;
use crate::keys::Keymap;
use crate::split::SplitStop;

/// A header's copy pill: its click copies the value, its shift+click the
/// whole header (spec 2026-10-26 H6). The tip says so in every terminal,
/// though one that keeps shift+click for its own selection never passes
/// it on (mine (approved)).
const HEADER_PILL_TIP: &str = "Copy this header's value · shift+click: the whole header";

/// The app state a hint needs to say what its button would do *now*.
/// Most controls under the pointer are two-state — send/cancel, open/close,
/// enable/disable, show/hide — and a hint that names both ("Enable or
/// disable this row") makes the reader work out which a click would do.
/// So the hint names one, and this says which.
///
/// [`crate::app::App::hint_ctx`] fills it for the hovered hit alone: one
/// place reads the app, and `hint_for` stays a table of wording. The two
/// must agree on what a control's second state is — [`tests`] walks the
/// two-state hits to keep them honest.
#[derive(Debug, Default, Clone, Copy)]
pub struct HintCtx {
    /// The hovered control is in the second of its two states: sending,
    /// not idle; open, not closed; enabled, not disabled; shown, not
    /// hidden; removed, not present; acting on the response's headers,
    /// not its body. `false` for controls with no second state (nothing
    /// reads it there).
    pub on: bool,
    /// The scope the open value popup's "✕ remove" would clear, from
    /// `ModalStack::value_popup_remove_scope`.
    pub remove_scope: Option<ExtractDestination>,
    /// The Manage screen's open tab, naming what its buttons act on: a
    /// space on the Spaces tab, an environment on the Environments tab.
    pub manage_tab: Option<crate::components::manage::ManageTab>,
    /// What the Variable Manager's detail pane has open, naming what its
    /// shared `[Rename]`/`[Delete]` buttons act on.
    pub vm_noun: VmNoun,
    /// What the variable tooltip's `󰏫` pill opens for the token under it.
    pub tip_edit: TipEditVerb,
    /// The hovered dropdown row button's tooltip words (its
    /// `RowButton::tip`).
    pub row_tip: &'static str,
    /// Where the keyboard is: the key a hint names is spelled for it
    /// (piece 4 spec §4.6).
    pub spell: crate::keyroute::SpellCtx,
}

/// What `Action::OpenVarTokenPopup` will open for a token, so the tip's
/// edit pill can say so: the popup action picks the editor from the
/// declaration, and the hint must name that same one.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum TipEditVerb {
    /// A plain value (declared or request-scoped): the value popup.
    #[default]
    Value,
    /// A selector field: the option picker.
    Option,
    /// A secret, set or missing: the secret prompt.
    Secret,
    /// A name defined nowhere: the create flow.
    Define,
}

/// The two things the Variable Manager's shared buttons can be pointed at.
/// One pane, one pair of buttons, two nouns — so the hint asks rather than
/// assuming the commoner of the two.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum VmNoun {
    #[default]
    Variable,
    Selector,
}

impl VmNoun {
    fn label(self) -> &'static str {
        match self {
            VmNoun::Variable => "variable",
            VmNoun::Selector => "selector",
        }
    }
}

/// What the Manage screen's buttons act on, from the open tab. "item" is
/// unreachable in practice — those buttons only paint on the two list
/// tabs — but keeps the wording sane if that ever changes.
fn manage_noun(tab: Option<crate::components::manage::ManageTab>) -> &'static str {
    use crate::components::manage::ManageTab;
    match tab {
        Some(ManageTab::Spaces) => "space",
        Some(ManageTab::Environments) => "environment",
        Some(ManageTab::Variables) => "variable",
        Some(ManageTab::Settings) => "item",
        None => "item",
    }
}

/// What the footer should say while `hit` is hovered, or `None` for a hit
/// that is not a button. Where the hit is a button for an action the
/// palette also lists, the hint is that command's description; a bound key
/// is appended (`… · alt+t`) only for controls that don't show their own
/// keycap — see [`shows_its_own_keycap`].
pub fn hint_for(hit: &Hit, keymap: &Keymap, ctx: &HintCtx) -> Option<String> {
    match hint_source(hit, ctx)? {
        Source::Text(s) => Some(s),
        Source::Keyed(s, action) => {
            Some(match crate::keyroute::spell::spell(ctx.spell, &action, keymap) {
                Some(key) => format!("{s} \u{b7} {key}"),
                None => s,
            })
        }
        Source::Of(action) => {
            Some(describe_action(&action, keymap, !shows_its_own_keycap(hit), ctx.spell))
        }
    }
}

/// Whether `hit`'s control paints the bound key on itself. Those hints
/// leave the key off: repeating it on the footer line says the same thing
/// twice, a hand's width apart. Everything else keeps `· key`, so the
/// hover teaches the keyboard route.
fn shows_its_own_keycap(hit: &Hit) -> bool {
    matches!(
        hit,
        // The chip's own keycap pill.
        Hit::FooterChip(_)
            // The header bar leads a chip with the keycap that drives it —
            // but the selector pills (`alt+z`/`alt+x`) drive the *cycle*,
            // a hit of its own. The name chip beside one wears no key, so
            // its chooser shortcut is news, not an echo.
            | Hit::HeaderProjectCycle
            | Hit::HeaderEnvCycle
            // A single chip whose leading keycap is its own action.
            | Hit::HeaderTheme
    )
}

enum Source {
    /// A fixed line for a control with no single palette action.
    Text(String),
    /// A fixed line, then the key of `action`, which does the same as
    /// the control: for a control whose palette wording doesn't fit it.
    Keyed(String, Action),
    /// A control that dispatches `action`: described like the palette does.
    Of(Action),
}

fn text(s: &str) -> Option<Source> {
    Some(Source::Text(s.to_string()))
}

fn of(action: Action) -> Option<Source> {
    Some(Source::Of(action))
}

/// One line for `action`: the palette command's description plus its key
/// as the profile spells it at `at`, else [`fallback_description`]'s
/// wording, else the action's debug name (a last resort that keeps every
/// chip hintable).
fn describe_action(
    action: &Action,
    keymap: &Keymap,
    with_key: bool,
    at: crate::keyroute::SpellCtx,
) -> String {
    if let Some(cmd) = all_commands().into_iter().find(|c| c.action == *action) {
        // The palette has a whole row for its caveats ("(live preview;
        // Esc reverts)"); the footer line keeps just the verb phrase.
        let full = match cmd.description.rfind(" (") {
            Some(i) if cmd.description.ends_with(')') => &cmd.description[..i],
            _ => cmd.description,
        };
        let description = short_description(action).unwrap_or(full);
        let combo = with_key.then(|| crate::keyroute::spell::spell(at, action, keymap)).flatten();
        return match combo {
            Some(combo) => format!("{description} \u{b7} {combo}"),
            None => description.to_string(),
        };
    }
    fallback_description(action).unwrap_or_else(|| format!("{action:?}"))
}

/// The footer's shorter wording for a palette command whose description is
/// written for the palette's roomier two-line row. The palette keeps its
/// own prose — it has the width for the extra clause and the neighbours
/// that make it worth having; only the descriptions that outrun the
/// footer's gap are restated here.
fn short_description(action: &Action) -> Option<&'static str> {
    Some(match action {
        Action::OpenJqDescribe => "Write a jq filter with AI",
        Action::CopyToClipboard(CopyTarget::Url) => "Copy the resolved URL",
        Action::OpenJqBar => "Filter the JSON with jq",
        Action::OpenJqViews => "Pick or save a jq view",
        Action::RevertJqView => "Revert this view to its saved filter",
        Action::ClearJqFilter => "Start over: no view, empty filter",
        Action::PromptNewRequest => "Create a request in this folder",
        // "Open" over "Switch to", matching the ^O the hint carries; the
        // space and env choosers keep "Switch to", since neither is a
        // thing you open.
        Action::OpenProjectChooser => "Open another project",
        Action::CycleProject(1) => "Switch to the next project",
        Action::CycleEnv(1) => "Switch to the next environment",
        Action::OpenMethodDropdown => "Pick an HTTP method",
        // Reachable with no project open (the Manage screen is), so it
        // must not promise a project's files unconditionally.
        Action::ReloadFromDisk => "Re-read config and any open project's files from disk",
        _ => return None,
    })
}

/// Descriptions for the actions footer chips dispatch that the palette
/// does not list (context-bound ones: row toggles, view modes, the jq
/// bar's own keys).
fn fallback_description(action: &Action) -> Option<String> {
    use crate::components::response::ViewMode;
    Some(match action {
        Action::Quit => "Quit postui".to_string(),
        // Vim's quit chip (`:q`, `:qa` on Manage): the dirty gate asks
        // first. The chip runs `:q` only off Manage, and shows only with
        // no dialog open (task #85).
        Action::GatedQuit | Action::CloseWindow { write: false, force: false } => {
            "Quit postui, asking first about unsaved changes".to_string()
        }
        Action::OpenPalette => "Open the command palette".to_string(),
        Action::OpenExPalette => "Open the : command line".to_string(),
        Action::CancelSend => "Cancel the request in flight".to_string(),
        Action::CloseField => "Close the field, keeping what you typed".to_string(),
        Action::FocusUrl => "Edit the URL".to_string(),
        Action::DeleteTableRow(_) => "Delete this row".to_string(),
        // An auto row's pill (task #56).
        Action::CopyToClipboard(CopyTarget::ComputedHeader(_)) => {
            "Copy this header's value".to_string()
        }
        // The copy chips: the value, as the pills copy, in both profiles
        // (spec 2026-10-26 H5); a header's whole-header key is on the
        // keycap (`Y`).
        Action::CopyToClipboard(CopyTarget::TableRow(_)) => "Copy this row's value".to_string(),
        Action::CopyRow { whole: false } => "Copy this header's value".to_string(),
        Action::OverrideAutoHeader(_) => "Edit this header in the request".to_string(),
        Action::ToggleVarReveal => "Reveal or hide the secret value".to_string(),
        // Closing switches the filter off — when there is one: an empty
        // bar, or one still asking the AI, just closes.
        Action::ToggleJqBar => "Close the jq filter".to_string(),
        Action::ResponseViewMode(ViewMode::Raw) => "Show the raw response body".to_string(),
        Action::ResponseViewMode(ViewMode::Headers) => "Show the response headers".to_string(),
        Action::ResponseViewMode(ViewMode::Pretty) => "Show the body as a JSON tree".to_string(),
        Action::CycleTabs(d) if *d < 0 => "Switch to the previous tab".to_string(),
        Action::CycleTabs(_) => "Switch to the next tab".to_string(),
        Action::CloseScreen => "Go back to the request screen".to_string(),
        Action::SelectManageTab(tab) => format!("Show the {} tab", tab.label()),
        Action::ConfirmTopModal => "Confirm the dialog".to_string(),
        Action::CancelTopModal => "Cancel the dialog".to_string(),
        _ => return None,
    })
}

fn hint_source(hit: &Hit, ctx: &HintCtx) -> Option<Source> {
    use crate::components::settings::SettingsField;
    match hit {
        // -- Surfaces, not buttons: no hint. --
        Hit::Pane(_)
        | Hit::ManageRow(_)
        | Hit::SettingsRow(_)
        | Hit::SidebarRow(_)
        | Hit::UrlBar
        | Hit::TableRow(_)
        | Hit::TableCell { .. }
        | Hit::AutoHeaderRow(_)
        | Hit::BodyEditor
        | Hit::SplitHandle
        | Hit::StripControls
        | Hit::SidebarGutter
        | Hit::JsonRow(_)
        | Hit::JsonArrow(_)
        | Hit::ScrollbarThumb(_)
        | Hit::ScrollbarTrack(..)
        | Hit::HScrollThumb(_)
        | Hit::HScrollTrack(..)
        | Hit::DropdownRow(_)
        | Hit::ChooserRow(_)
        | Hit::PickerRow(_)
        | Hit::PaletteRow(_)
        | Hit::VarPickerRow(_)
        | Hit::VmLeftRow(_)
        | Hit::VmFormField(_)
        | Hit::VmEntryCell { .. }
        | Hit::VarToken(_)
        | Hit::TipPanel(_)
        | Hit::ModalOutside
        | Hit::ModalBody
        | Hit::ModalField(_)
        | Hit::ModalInput(_)
        | Hit::ResponseJqBar => None,

        // -- Tab strips --
        // A tab's label is most of its own hint already, but not all of
        // it: two strips carry a `Headers` — the request's, and what came
        // back — so the hint says what the tab holds rather than saying
        // the label again. The strips paint no keycaps (`editor_tab_N` is
        // bindable but unbound by default), so nothing is echoed there.
        Hit::EditorTab(i) => {
            use crate::components::editor::EditorTab;
            text(match EditorTab::from_draw_position(*i) {
                EditorTab::Params => "Edit the query params",
                EditorTab::Headers => "Edit the request headers",
                EditorTab::Vars => "Edit the request variables",
                EditorTab::Body => "Edit the request body",
            })
        }
        Hit::ResponseTab(mode) => of(Action::ResponseViewMode(*mode)),
        // Not `SelectManageTab`'s own wording ("Show the Spaces tab"),
        // which is written for a footer chip a strip away from the tab;
        // on the tab itself that is the label twice over.
        Hit::ManageTab(i) => {
            use crate::components::manage::ManageTab;
            text(match ManageTab::from_index(*i) {
                ManageTab::Variables => "Manage your variables",
                ManageTab::Environments => "Manage your environments",
                ManageTab::Spaces => "Manage your spaces",
                ManageTab::Settings => "Manage app settings",
            })
        }

        // -- Header --
        Hit::HeaderProject => of(Action::OpenProjectChooser),
        Hit::HeaderProjectCycle => of(Action::CycleProject(1)),
        Hit::HeaderSpace => text("Switch to another space"),
        Hit::HeaderSpaceCycle => text("Switch to the next space"),
        Hit::HeaderEnv => of(Action::OpenEnvChooser),
        Hit::HeaderEnvCycle => of(Action::CycleEnv(1)),
        Hit::HeaderManage => {
            text(if ctx.on { "Close the Manage screen" } else { "Open the Manage screen" })
        }
        Hit::HeaderTheme => of(Action::OpenThemeChooser),
        // The row toggle's chip drives the same two-state control as the
        // row's own checkbox, so it says the same thing. Handled here
        // rather than through `describe_action`, which has no `ctx`.
        Hit::FooterChip(Action::ToggleTableRow(_)) => {
            text(if ctx.on { "Disable this row" } else { "Enable this row" })
        }
        // The sidebar takes no text: its paste chip imports (spec
        // 2026-10-15 curl import visibility V9), so it says what its
        // Import curl button does.
        Hit::FooterChip(Action::Paste)
            if ctx.spell.surface == crate::keyroute::Surface::Sidebar =>
        {
            text("Import curl from clipboard")
        }
        Hit::FooterChip(action) => of(action.clone()),

        // -- Manage screen --
        Hit::ManageNew => {
            Some(Source::Text(format!("Create a new {}", manage_noun(ctx.manage_tab))))
        }
        Hit::ManageRename => {
            Some(Source::Text(format!("Rename this {}", manage_noun(ctx.manage_tab))))
        }
        Hit::ManageDelete => {
            Some(Source::Text(format!("Delete this {}", manage_noun(ctx.manage_tab))))
        }
        Hit::ManageMoveAll => text("Move all requests to another space"),

        // -- Settings tab --
        // Each control names the single action a click performs, in the
        // state it is currently in. `ctx.on` is the tick the user sees:
        // `ai_confirmed` is stored inverted from its row, which is
        // worded as the consent question.
        Hit::SettingsControl(SettingsField::Animations) => {
            text(if ctx.on { "Turn eased transitions off" } else { "Turn eased transitions on" })
        }
        Hit::SettingsControl(SettingsField::HoverHints) => text(if ctx.on {
            "Stop explaining hovered buttons"
        } else {
            "Explain hovered buttons in the footer"
        }),
        Hit::SettingsControl(SettingsField::AiConfirmed) => text(if ctx.on {
            "Stop asking before sending to the AI"
        } else {
            "Ask before sending to the AI command"
        }),
        Hit::SettingsControl(SettingsField::CurlButton) => text(if ctx.on {
            "Stop watching the clipboard for curl"
        } else {
            "Watch the clipboard for curl"
        }),
        Hit::SettingsControl(SettingsField::AiCmd) => text("Edit the command the AI filter runs"),
        Hit::SettingsControl(SettingsField::ClipboardCmd) => {
            text("Edit the external clipboard command")
        }
        Hit::SettingsControl(SettingsField::Osc52Limit) => {
            text("Edit the terminal clipboard size limit")
        }
        // The two-state control's own row hit is never registered — its
        // segments are — so this arm only keeps the match exhaustive.
        Hit::SettingsControl(SettingsField::JqTab) => text("Choose what Tab does in the jq bar"),
        Hit::SettingsJqTab(crate::config::JqTab::Menu) => text("List jq completions under the bar"),
        Hit::SettingsJqTab(crate::config::JqTab::Cycle) => {
            text("Ghost the best jq completion in place")
        }
        // Same as the jq Tab row: only the segments register hits.
        Hit::SettingsControl(SettingsField::Keymap) => text("Choose the app's keyboard profile"),
        Hit::SettingsKeymap(crate::config::KeyProfile::Arrows) => text("Use arrows and Enter"),
        Hit::SettingsKeymap(crate::config::KeyProfile::Vim) => text("Use vim keys"),
        // Same as the jq Tab row: only the segments register hits.
        Hit::SettingsControl(SettingsField::Indent) => text("Choose how new indents are made"),
        Hit::SettingsIndent(crate::config::IndentKind::Spaces) => {
            text("Indent new levels with spaces")
        }
        Hit::SettingsIndent(crate::config::IndentKind::Tabs) => text("Indent new levels with tabs"),
        Hit::SettingsControl(SettingsField::IndentWidth) => {
            text("Edit the spaces in one indent level")
        }
        Hit::SettingsFile { file, reset: false } => {
            Some(Source::Text(format!("Open {} in your editor", file.name())))
        }
        Hit::SettingsFile { file, reset: true } => {
            Some(Source::Text(format!("Reset {} to its defaults", file.name())))
        }
        Hit::ManageEnvTls(None) => text("Let each request decide on TLS checks"),
        Hit::ManageEnvTls(Some(postui_core::project::TlsPolicy::Verify)) => {
            text("Always check TLS certificates here")
        }
        Hit::ManageEnvTls(Some(postui_core::project::TlsPolicy::Insecure)) => {
            text("Never check TLS certificates here")
        }

        // -- Sidebar --
        Hit::SidebarNewRequest => of(Action::PromptNewRequest),
        Hit::SidebarImportCurl => text("Import curl from clipboard"),
        Hit::SidebarFolderArrow(_) => {
            text(if ctx.on { "Collapse this folder" } else { "Expand this folder" })
        }

        // -- Editor --
        // The button itself flips to "Cancel" on hover while a send is in
        // flight (`components::editor`); its hint flips with it, rather
        // than naming both actions and leaving the reader to work out
        // which one a click would do.
        Hit::SendButton if ctx.on => of(Action::CancelSend),
        Hit::SendButton => of(Action::Send),
        Hit::MethodSelector => of(Action::OpenMethodDropdown),
        Hit::CopyUrl => of(Action::CopyToClipboard(CopyTarget::Url)),
        Hit::TableCheckbox(_) => text(if ctx.on { "Disable this row" } else { "Enable this row" }),
        // A Headers-tab row's pill (`on`) takes shift+click too.
        Hit::TableCopy(_) if ctx.on => text(HEADER_PILL_TIP),
        Hit::TableCopy(i) => of(Action::CopyToClipboard(CopyTarget::TableRow(*i))),
        Hit::TableDelete(_) => text("Delete this row"),
        // A dropdown row's own buttons, with the menu's key for each.
        // No tip when the button is gone (a menu rebuilt under the pointer).
        Hit::DropdownRowButton { verb, .. } => (!ctx.row_tip.is_empty())
            .then(|| Source::Keyed(ctx.row_tip.to_string(), Action::OnSelection(*verb))),
        // The two end chips are the hide/show toggles (spec E12a): a click
        // hides the other pane, and on the lit chip (`on`) shows it again
        // at the share it had. Each carries its toggle's key. The middle
        // stops are no chip, so they have no hint.
        Hit::SplitStop(stop) => {
            let (pane, toggle) = match stop {
                SplitStop::EditorFull => ("response", Action::ToggleResponseCollapse),
                SplitStop::ResponseFull => ("editor", Action::ToggleTableCollapse),
                SplitStop::EditorBig | SplitStop::Even | SplitStop::ResponseBig => return None,
            };
            let verb = if ctx.on { "Show" } else { "Hide" };
            Some(Source::Keyed(format!("{verb} the {pane}"), toggle))
        }
        // Worded once, with the footer chips that dispatch the same actions;
        // the copy pill says its shift+click too, which the chip lacks.
        Hit::AutoHeaderCopy(_) => text(HEADER_PILL_TIP),
        Hit::AutoHeaderOverride(i) => of(Action::OverrideAutoHeader(*i)),
        Hit::AutoHeaderReveal => {
            text(if ctx.on { "Hide the computed secrets" } else { "Show the computed secrets" })
        }

        // -- Response --
        // The toolbar's three buttons act on the tab that is up, not on
        // the body: they dispatch `CopyTarget::ResponseView` and
        // `PromptSaveView`, where the palette's own copy/save commands
        // always take the body. So they say what the open tab holds
        // rather than borrowing those palette lines.
        Hit::CopyBodyButton => {
            text(if ctx.on { "Copy the response headers" } else { "Copy the response body" })
        }
        Hit::SaveBodyButton => text(if ctx.on {
            "Save the headers to a file"
        } else {
            "Save the response body to a file"
        }),
        Hit::ResponseEditorButton => text(if ctx.on {
            "Open the headers in your editor"
        } else {
            "Open the response body in your editor"
        }),
        Hit::ResponseSearchButton => {
            text(if ctx.on { "Search the response headers" } else { "Search the response body" })
        }
        Hit::ResponseSearchNext => text("Go to the next match"),
        Hit::ResponseSearchPrev => text("Go to the previous match"),
        Hit::HeaderCopy(_) => text(HEADER_PILL_TIP),
        // A toggle: pressed (the bar showing), a click closes it.
        Hit::ResponseJqButton if ctx.on => of(Action::ToggleJqBar),
        Hit::ResponseJqButton => of(Action::OpenJqBar),
        Hit::ResponseJqAiButton => of(Action::OpenJqDescribe),
        Hit::ResponseJqChip => of(Action::OpenJqViews),
        Hit::ResponseJqRevert => of(Action::RevertJqView),
        Hit::ResponseJqClear => of(Action::ClearJqFilter),

        // -- Choosers, pickers, modals --
        Hit::ChooserToggle => text(if ctx.on {
            "Show the light themes instead"
        } else {
            "Show the dark themes instead"
        }),
        Hit::ChooserStepDown => text("Lower the theme's contrast"),
        Hit::ChooserStepUp => text("Raise the theme's contrast"),
        Hit::PickerPrimary => {
            text(if ctx.on { "Save under the typed name" } else { "Open the shown folder" })
        }
        Hit::PickerHidden => {
            text(if ctx.on { "Hide the hidden files again" } else { "Show the hidden files too" })
        }
        Hit::NewProjectBrowse => text("Browse for the project folder"),
        Hit::ConfirmChoice(_) => text("Answer with this choice"),
        Hit::KeymapChoice(KeymapAnswer::Profile(crate::config::KeyProfile::Arrows)) => {
            text("Use arrows and Enter")
        }
        Hit::KeymapChoice(KeymapAnswer::Profile(crate::config::KeyProfile::Vim)) => {
            text("Use vim keys")
        }
        // About the choice, not files: this is the ordinary quit, which
        // may ask about unsaved edits first; only the keymap is left unset.
        Hit::KeymapChoice(KeymapAnswer::Quit) => text("Quit and ask again next launch"),
        Hit::ConfigStartupChoice(choice) => {
            use crate::action::ConfigStartupChoice as C;
            text(match choice {
                C::Edit => "Open config.toml in your editor",
                C::Reset => "Reset config.toml to its defaults",
                C::ContinueUnsaved => "Run this session on defaults; nothing will be saved",
                C::Quit => "Quit without changing config.toml",
            })
        }
        Hit::ConfigEditKeepEditing => text("Resume editing this file"),
        Hit::ConfigEditDiscard => text("Discard these edits"),
        Hit::ModalCancel => text("Cancel and close \u{b7} esc on the buttons"),
        Hit::ModalConfirm => text("Confirm and close \u{b7} enter"),
        Hit::ModalChoiceArrow { dir, .. } => {
            text(if *dir > 0 { "Go to the next choice" } else { "Go to the previous choice" })
        }
        Hit::ModalRowToggle(_) => {
            text(if ctx.on { "Bring this field back" } else { "Remove this field" })
        }
        Hit::ModalAddRow => text("Add another field"),
        Hit::ModalSharedToggle => text("Use the same options in every environment"),
        Hit::ModalRevealToggle => text(if ctx.on {
            "Hide the secret you're typing"
        } else {
            "Show the secret you're typing"
        }),
        // Names the scope the chosen Write-to row would clear, the way
        // the modal's own remove chip does — "this scope" makes the
        // reader look back up at the popup to find out which.
        Hit::ModalRemove => text(match ctx.remove_scope {
            Some(ExtractDestination::Request) => "Remove this request's value",
            Some(ExtractDestination::ActiveEnv) => "Remove this environment's value",
            _ => "Remove the project default value",
        }),
        Hit::TipEdit(_) => text(match ctx.tip_edit {
            TipEditVerb::Value => "Edit this variable's value",
            TipEditVerb::Option => "Choose this selector's option",
            TipEditVerb::Secret => "Set this secret",
            TipEditVerb::Define => "Define this variable",
        }),
        Hit::TipCopy(_) => text("Copy this variable's value"),
        Hit::TipReveal(_) => {
            text(if ctx.on { "Hide this secret's value" } else { "Show this secret's value" })
        }

        // -- Variable Manager --
        Hit::VmNewVar => of(Action::PromptNewVar),
        Hit::VmNewSelector => of(Action::PromptNewSelector),
        Hit::VmSecretToggle => text(if ctx.on {
            "Stop treating this as a secret"
        } else {
            "Mark this variable as a secret"
        }),
        Hit::VmRevealToggle => {
            text(if ctx.on { "Hide this secret's value" } else { "Show this secret's value" })
        }
        Hit::VmRemoveEnvValue => text("Remove this environment's value"),
        Hit::VmRename => text(&format!("Rename this {}", ctx.vm_noun.label())),
        Hit::VmDelete => text(&format!("Delete this {}", ctx.vm_noun.label())),
        Hit::VmEntryRadio(_) => text("Select this option for the environment"),
        Hit::VmEntryCopy(_) => text("Copy this option to paste into another environment"),
        Hit::VmEntryDelete(_) => text("Delete this option"),
        Hit::VmPasteOption => text("Paste the copied option into this environment"),
        Hit::VmNewOption => text("Add another option"),
        Hit::VmEditFields => text("Edit the group's list of fields"),
        Hit::VmPromoteBtn => text("Promote this value to the variable"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> HintCtx {
        HintCtx::default()
    }

    /// The sidebar's paste chip imports curl, so it says so; elsewhere a
    /// paste is a paste.
    #[test]
    fn the_sidebar_paste_chip_names_the_import() {
        let keymap = Keymap::default_bindings();
        let chip = Hit::FooterChip(Action::Paste);
        let mut sidebar = ctx();
        sidebar.spell.surface = crate::keyroute::Surface::Sidebar;
        assert_eq!(hint_for(&chip, &keymap, &sidebar).unwrap(), "Import curl from clipboard");
        assert!(!hint_for(&chip, &keymap, &ctx()).unwrap().contains("curl"));
    }

    /// The Variable Manager's shared `[Rename]`/`[Delete]` buttons act on
    /// whatever the detail pane has open, so the hint has to name it.
    #[test]
    fn the_managers_rename_and_delete_name_what_is_open() {
        let keymap = Keymap::default_bindings();
        for (noun, expected) in [(VmNoun::Variable, "variable"), (VmNoun::Selector, "selector")] {
            let ctx = HintCtx { vm_noun: noun, ..ctx() };
            for hit in [Hit::VmRename, Hit::VmDelete] {
                let h = hint_for(&hit, &keymap, &ctx).unwrap();
                assert!(h.contains(expected), "{hit:?} with {noun:?} open said {h:?}");
            }
        }
    }

    /// The tooltip's edit pill opens a different dialog per token, and
    /// the hint names the one it will open.
    #[test]
    fn the_tips_edit_pill_names_the_dialog_it_opens() {
        let keymap = Keymap::default_bindings();
        for (verb, expected) in [
            (TipEditVerb::Value, "Edit this variable's value"),
            (TipEditVerb::Option, "Choose this selector's option"),
            (TipEditVerb::Secret, "Set this secret"),
            (TipEditVerb::Define, "Define this variable"),
        ] {
            let ctx = HintCtx { tip_edit: verb, ..ctx() };
            let h = hint_for(&Hit::TipEdit("tok".into()), &keymap, &ctx).unwrap();
            assert_eq!(h, expected, "{verb:?}");
        }
    }

    /// A control with no keycap of its own teaches the keyboard route.
    #[test]
    fn a_keyless_button_carries_the_bound_key() {
        let keymap = Keymap::default_bindings();
        let h = hint_for(&Hit::MethodSelector, &keymap, &ctx()).unwrap();
        assert!(
            h.starts_with("Pick an HTTP method \u{b7} "),
            "the method cap shows no key of its own: {h:?}"
        );
    }

    /// The copy-URL button's hint spells its key for the profile at hand
    /// (spec §4.6): through the vim keymap and the vim table in vim, as
    /// the arrows keymap in arrows. M1's vim keymap keeps the alt chords,
    /// so both read `alt+y`.
    #[test]
    fn the_copy_url_hint_spells_its_key_for_the_profile() {
        use crate::config::KeyProfile;
        use crate::keyroute::{SpellCtx, Surface};
        let alt = crate::keys::alt_label();
        let want = format!("Copy the resolved URL \u{b7} {alt}+y");
        for profile in [KeyProfile::Arrows, KeyProfile::Vim] {
            let keymap = Keymap::default_bindings_for(profile);
            let ctx = HintCtx {
                spell: SpellCtx {
                    profile,
                    surface: Surface::UrlLine,
                    field: crate::keyroute::FieldSpell::None,
                },
                ..ctx()
            };
            assert_eq!(
                hint_for(&Hit::CopyUrl, &keymap, &ctx).as_deref(),
                Some(want.as_str()),
                "{profile:?}"
            );
        }
    }

    /// A dropdown row's own buttons name the menu's key for each, as the
    /// profile spells it (jq views spec 17).
    #[test]
    fn a_dropdown_rows_buttons_carry_the_menus_keys() {
        use crate::config::KeyProfile;
        use crate::keyroute::{SpellCtx, Surface};
        for (profile, del) in [(KeyProfile::Arrows, "d"), (KeyProfile::Vim, "dd")] {
            let keymap = Keymap::default_bindings_for(profile);
            let ctx = |row_tip| HintCtx {
                spell: SpellCtx {
                    profile,
                    surface: Surface::Menu,
                    field: crate::keyroute::FieldSpell::None,
                },
                row_tip,
                ..ctx()
            };
            assert_eq!(
                hint_for(&Hit::row_rename(0), &keymap, &ctx("Rename this view")).as_deref(),
                Some("Rename this view \u{b7} r"),
                "{profile:?}"
            );
            assert_eq!(
                hint_for(&Hit::row_delete(0), &keymap, &ctx("Delete this view")),
                Some(format!("Delete this view \u{b7} {del}")),
                "{profile:?}"
            );
        }
    }

    /// Spec E12a, E12b: the two split chips are the hide/show toggles, so
    /// each hint says which pane a click hides (or, lit, shows again) and
    /// carries that toggle's key as the profile spells it.
    #[test]
    fn split_chip_hints_are_the_hide_show_toggles() {
        use crate::config::KeyProfile;
        let alt = crate::keys::alt_label();
        for profile in [KeyProfile::Arrows, KeyProfile::Vim] {
            let keymap = Keymap::default_bindings_for(profile);
            let spell = crate::keyroute::SpellCtx { profile, ..Default::default() };
            let hint = |stop, on| {
                hint_for(&Hit::SplitStop(stop), &keymap, &HintCtx { on, spell, ..ctx() }).unwrap()
            };
            assert_eq!(
                hint(SplitStop::EditorFull, false),
                format!("Hide the response \u{b7} {alt}+shift+p"),
                "{profile:?}"
            );
            assert_eq!(
                hint(SplitStop::EditorFull, true),
                format!("Show the response \u{b7} {alt}+shift+p"),
                "{profile:?}"
            );
            assert_eq!(
                hint(SplitStop::ResponseFull, false),
                format!("Hide the editor \u{b7} {alt}+p"),
                "{profile:?}"
            );
            assert_eq!(
                hint(SplitStop::ResponseFull, true),
                format!("Show the editor \u{b7} {alt}+p"),
                "{profile:?}"
            );
        }
    }

    /// A toggle with no key in the active keymap leaves the key off.
    #[test]
    fn a_split_chip_hint_leaves_off_an_unbound_toggle_key() {
        let mut keymap = Keymap::default_bindings();
        keymap.apply_overrides("toggle_response_collapse = []").unwrap();
        assert_eq!(
            hint_for(&Hit::SplitStop(SplitStop::EditorFull), &keymap, &ctx()).unwrap(),
            "Hide the response"
        );
    }

    /// A control that paints the key on itself doesn't repeat it: the
    /// Theme chip's own `alt+t` pill sits a hand's width from the footer.
    #[test]
    fn a_keycapped_control_leaves_the_key_off() {
        let keymap = Keymap::default_bindings();
        assert_eq!(hint_for(&Hit::HeaderTheme, &keymap, &ctx()).unwrap(), "Pick a color theme");
        assert_eq!(
            hint_for(&Hit::FooterChip(Action::PromptRenameRequest), &keymap, &ctx()).unwrap(),
            "Rename the open request"
        );
    }

    /// The chip toggles, so its hint names the direction the click would
    /// go rather than both.
    #[test]
    fn the_manage_chip_hint_follows_the_screen() {
        let keymap = Keymap::default_bindings();
        assert_eq!(
            hint_for(&Hit::HeaderManage, &keymap, &ctx()).unwrap(),
            "Open the Manage screen"
        );
        assert_eq!(
            hint_for(&Hit::HeaderManage, &keymap, &HintCtx { on: true, ..ctx() }).unwrap(),
            "Close the Manage screen"
        );
    }

    /// The selector pill drives the cycle, not the chooser: the name chip
    /// beside it wears no key, so it carries its own.
    #[test]
    fn a_selector_name_chip_carries_its_chooser_key_but_the_cycle_pill_does_not() {
        let keymap = Keymap::default_bindings();
        assert_eq!(
            hint_for(&Hit::HeaderProject, &keymap, &ctx()).unwrap(),
            "Open another project \u{b7} ^O"
        );
        assert_eq!(
            hint_for(&Hit::HeaderProjectCycle, &keymap, &ctx()).unwrap(),
            "Switch to the next project"
        );
    }

    #[test]
    fn surfaces_have_no_hint_and_buttons_always_do() {
        let keymap = Keymap::default_bindings();
        assert_eq!(hint_for(&Hit::UrlBar, &keymap, &ctx()), None);
        assert_eq!(hint_for(&Hit::SidebarRow(0), &keymap, &ctx()), None);
        assert!(hint_for(&Hit::TableDelete(0), &keymap, &ctx()).is_some());
        assert_eq!(hint_for(&Hit::TableCopy(0), &keymap, &ctx()).unwrap(), "Copy this row's value");
        assert!(hint_for(&Hit::FooterChip(Action::Quit), &keymap, &ctx()).is_some());
    }

    /// A palette caveat in parentheses stays in the palette.
    #[test]
    fn palette_parentheticals_are_dropped() {
        let keymap = Keymap::default_bindings();
        assert_eq!(
            hint_for(&Hit::FooterChip(Action::DeleteSelectedRequest), &keymap, &ctx()).unwrap(),
            "Delete the open request"
        );
    }

    /// A chip action the palette doesn't list still gets real words, not
    /// the `Debug` last resort.
    #[test]
    fn chip_actions_outside_the_palette_use_the_fallback_wording() {
        let keymap = Keymap::default_bindings();
        assert_eq!(
            hint_for(&Hit::FooterChip(Action::DeleteTableRow(3)), &keymap, &ctx()).unwrap(),
            "Delete this row"
        );
        // The variable form's reveal chip dispatches this (plan M2 Task 7).
        assert_eq!(
            hint_for(&Hit::FooterChip(Action::ToggleVarReveal), &keymap, &ctx()).unwrap(),
            "Reveal or hide the secret value"
        );
    }

    /// A one-field prompt's chips run `ConfirmTopModal` and
    /// `CancelTopModal`; each hints in words, not by its debug name.
    #[test]
    fn a_prompts_chips_hint_with_their_own_wording() {
        use crate::components::line_input::LineInput;
        use crate::components::modal::{Modal, ModalStack, PromptKind};
        let keymap = Keymap::default_bindings();
        let mut stack = ModalStack::default();
        stack.push(Modal::Prompt {
            title: "Name".into(),
            input: LineInput::new(""),
            kind: PromptKind::NewRequest,
            revealed: false,
        });
        let chips = stack.footer_chips().unwrap();
        let hints: Vec<String> = chips
            .into_iter()
            .filter_map(|(_, _, action)| action)
            .map(|a| hint_for(&Hit::FooterChip(a), &keymap, &ctx()).unwrap())
            .collect();
        assert_eq!(hints, ["Confirm the dialog", "Cancel the dialog"]);
    }

    /// No control falls through to the `{action:?}` last resort: every
    /// action the footer paints as a chip, in every pane and state, has
    /// real words. (`alt+w` read "CycleSplit" before this test existed.)
    #[test]
    fn no_chip_falls_through_to_the_debug_name() {
        use crate::components::footer::{JqBarState, TableCursor, footer_chips};
        use crate::layout::PaneId;
        let keymap = Keymap::default_bindings();
        for focus in [PaneId::Sidebar, PaneId::Editor, PaneId::Response] {
            for sending in [false, true] {
                for url_focused in [false, true] {
                    for row in [
                        None,
                        Some(TableCursor::Row { index: 0, enabled: true, headers: false }),
                        Some(TableCursor::Row { index: 0, enabled: true, headers: true }),
                        Some(TableCursor::Auto { index: 0, pencil: true }),
                        Some(TableCursor::ResponseHeader),
                    ] {
                        for jq in [
                            JqBarState::Closed,
                            JqBarState::Open,
                            JqBarState::Focused,
                            JqBarState::Menu,
                            JqBarState::Completing { cycle: true },
                            JqBarState::Completing { cycle: false },
                            JqBarState::Closer,
                        ] {
                            for field_open in [false, true] {
                                let chips = footer_chips(
                                    focus,
                                    false,
                                    sending,
                                    Some("add param"),
                                    url_focused,
                                    false,
                                    row,
                                    jq,
                                    field_open,
                                );
                                for (_, _, action) in chips {
                                    let Some(action) = action else { continue };
                                    let hit = Hit::FooterChip(action.clone());
                                    let h = hint_for(&hit, &keymap, &ctx()).unwrap();
                                    assert_ne!(
                                        h,
                                        format!("{action:?}"),
                                        "{action:?} has no wording of its own"
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
        // The always-present right-hand pair, which `footer_chips` doesn't
        // list.
        for action in [
            Action::OpenPalette,
            Action::OpenExPalette,
            Action::Quit,
            Action::GatedQuit,
            Action::CloseWindow { write: false, force: false },
        ] {
            let h = hint_for(&Hit::FooterChip(action.clone()), &keymap, &ctx()).unwrap();
            assert_ne!(h, format!("{action:?}"), "{action:?} has no wording");
        }
    }

    /// The Send button's label flips to Cancel in flight; so does its hint.
    #[test]
    fn the_send_hint_says_which_action_a_click_would_do() {
        let keymap = Keymap::default_bindings();
        let idle = hint_for(&Hit::SendButton, &keymap, &ctx()).unwrap();
        assert!(idle.starts_with("Send the open request"), "{idle:?}");
        let busy = hint_for(&Hit::SendButton, &keymap, &HintCtx { on: true, ..ctx() }).unwrap();
        assert_eq!(busy, "Cancel the request in flight");
    }

    /// The value popup's remove names the scope it would clear, matching
    /// the modal's own remove chip.
    #[test]
    fn the_value_popup_remove_hint_names_its_scope() {
        let keymap = Keymap::default_bindings();
        let for_scope = |d| {
            hint_for(&Hit::ModalRemove, &keymap, &HintCtx { remove_scope: Some(d), ..ctx() })
                .unwrap()
        };
        assert_eq!(for_scope(ExtractDestination::Request), "Remove this request's value");
        assert_eq!(for_scope(ExtractDestination::ActiveEnv), "Remove this environment's value");
        assert_eq!(
            for_scope(ExtractDestination::ProjectDefault),
            "Remove the project default value"
        );
    }

    /// Every two-state control says one thing, and a different thing in
    /// its other state — never "X or Y", which leaves the reader to work
    /// out which a click would do. `App::hint_ctx` is the other half of
    /// this: it decides which state each of these is in.
    #[test]
    fn two_state_controls_name_one_action_each_way() {
        let keymap = Keymap::default_bindings();
        for hit in [
            Hit::SendButton,
            Hit::HeaderManage,
            Hit::TableCheckbox(0),
            Hit::FooterChip(Action::ToggleTableRow(0)),
            Hit::SidebarFolderArrow(0),
            Hit::AutoHeaderReveal,
            Hit::ChooserToggle,
            Hit::PickerPrimary,
            Hit::PickerHidden,
            Hit::ModalRowToggle(0),
            Hit::TipReveal("tok".into()),
            Hit::VmRevealToggle,
            Hit::ModalRevealToggle,
            Hit::VmSecretToggle,
            Hit::CopyBodyButton,
            Hit::SaveBodyButton,
            Hit::ResponseEditorButton,
            Hit::ResponseSearchButton,
            Hit::SettingsControl(crate::components::settings::SettingsField::Animations),
            Hit::SettingsControl(crate::components::settings::SettingsField::HoverHints),
            Hit::SettingsControl(crate::components::settings::SettingsField::AiConfirmed),
            Hit::SettingsControl(crate::components::settings::SettingsField::CurlButton),
        ] {
            let off = hint_for(&hit, &keymap, &ctx()).unwrap();
            let on = hint_for(&hit, &keymap, &HintCtx { on: true, ..ctx() }).unwrap();
            assert_ne!(off, on, "{hit:?} reads the same in both states");
            for h in [&off, &on] {
                assert!(!h.contains(" or "), "{hit:?} names both: {h:?}");
            }
        }
    }

    /// A tab strip is hintable like any other button. The label alone
    /// doesn't say which pane a tab belongs to — `Headers` is a tab in
    /// both — so each hint names what its tab holds.
    #[test]
    fn every_tab_says_what_it_holds() {
        use crate::components::editor::EditorTab;
        use crate::components::manage::ManageTab;
        use crate::components::response::ViewMode;
        let keymap = Keymap::default_bindings();
        let mut seen: Vec<String> = Vec::new();
        for i in 0..4 {
            seen.push(hint_for(&Hit::EditorTab(i), &keymap, &ctx()).unwrap());
        }
        for mode in [ViewMode::Pretty, ViewMode::Raw, ViewMode::Headers] {
            seen.push(hint_for(&Hit::ResponseTab(mode), &keymap, &ctx()).unwrap());
        }
        for i in 0..ManageTab::ALL.len() {
            seen.push(hint_for(&Hit::ManageTab(i), &keymap, &ctx()).unwrap());
        }
        // The two `Headers` tabs read differently, which is the whole point.
        assert_eq!(
            hint_for(&Hit::EditorTab(EditorTab::Headers.draw_position()), &keymap, &ctx()).unwrap(),
            "Edit the request headers"
        );
        assert_eq!(
            hint_for(&Hit::ResponseTab(ViewMode::Headers), &keymap, &ctx()).unwrap(),
            "Show the response headers"
        );
        let mut sorted = seen.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), seen.len(), "two tabs read the same: {seen:?}");
    }

    /// Hints read as one voice: an imperative phrase, one clause, no
    /// trailing stop, and short enough to sit beside the chips whole.
    #[test]
    fn hints_share_one_shape() {
        let keymap = Keymap::default_bindings();
        for hit in [
            Hit::ManageNew,
            Hit::ManageRename,
            Hit::ManageDelete,
            Hit::SidebarFolderArrow(0),
            Hit::TableCheckbox(0),
            Hit::AutoHeaderReveal,
            Hit::ChooserToggle,
            Hit::PickerPrimary,
            Hit::PickerHidden,
            Hit::VmSecretToggle,
            Hit::ManageMoveAll,
            Hit::ManageEnvTls(Some(postui_core::project::TlsPolicy::Insecure)),
            Hit::AutoHeaderReveal,
            Hit::ChooserToggle,
            Hit::ChooserStepDown,
            Hit::ChooserStepUp,
            Hit::ModalSharedToggle,
            Hit::ModalRevealToggle,
            Hit::ModalRowToggle(0),
            Hit::ModalRemove,
            Hit::PickerPrimary,
            Hit::ResponseSearchNext,
            Hit::SplitStop(SplitStop::EditorFull),
            Hit::TipReveal("tok".into()),
            Hit::VmEntryRadio(0),
            Hit::VmPromoteBtn,
            Hit::VmRemoveEnvValue,
            Hit::VmSecretToggle,
            Hit::FooterChip(Action::ToggleJqBar),
            Hit::FooterChip(Action::CloseScreen),
            Hit::ResponseJqAiButton,
            Hit::ResponseJqChip,
            Hit::ResponseJqRevert,
            Hit::ResponseJqClear,
            Hit::ResponseEditorButton,
            Hit::CopyBodyButton,
            Hit::SaveBodyButton,
            Hit::CopyUrl,
            Hit::SidebarNewRequest,
            Hit::SidebarImportCurl,
            Hit::MethodSelector,
            Hit::EditorTab(0),
            Hit::ResponseTab(crate::components::response::ViewMode::Pretty),
            Hit::ManageTab(0),
            Hit::SettingsControl(crate::components::settings::SettingsField::AiCmd),
            Hit::SettingsControl(crate::components::settings::SettingsField::Osc52Limit),
            Hit::SettingsControl(crate::components::settings::SettingsField::CurlButton),
            Hit::SettingsJqTab(crate::config::JqTab::Menu),
            Hit::SettingsJqTab(crate::config::JqTab::Cycle),
            Hit::SettingsKeymap(crate::config::KeyProfile::Arrows),
            Hit::SettingsKeymap(crate::config::KeyProfile::Vim),
            Hit::SettingsIndent(crate::config::IndentKind::Spaces),
            Hit::SettingsIndent(crate::config::IndentKind::Tabs),
            Hit::SettingsControl(crate::components::settings::SettingsField::IndentWidth),
            Hit::KeymapChoice(KeymapAnswer::Profile(crate::config::KeyProfile::Arrows)),
            Hit::KeymapChoice(KeymapAnswer::Profile(crate::config::KeyProfile::Vim)),
            Hit::KeymapChoice(KeymapAnswer::Quit),
            Hit::SettingsFile { file: crate::action::ConfigFile::Config, reset: false },
            Hit::SettingsFile { file: crate::action::ConfigFile::Keys, reset: true },
        ] {
            let h = hint_for(&hit, &keymap, &ctx()).unwrap();
            // The key, where one is appended, doesn't count against the
            // wording — it is the same three or four cells everywhere.
            let words = h.split(" \u{b7} ").next().unwrap();
            assert!(words.chars().count() <= 42, "too long \u{2014} {hit:?}: {h:?}");
            assert!(
                !words.contains(';') && !words.contains('('),
                "one clause, no caveats \u{2014} {hit:?}: {h:?}"
            );
            assert!(!words.ends_with('.'), "no full stop \u{2014} {hit:?}: {h:?}");
            let first = words.chars().next().unwrap();
            assert!(first.is_uppercase(), "sentence case \u{2014} {hit:?}: {h:?}");
        }
    }
}

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

use crate::action::{Action, CopyTarget, ExtractDestination};
use crate::components::palette::{all_commands, keymap_action_name};
use crate::hit::Hit;
use crate::keys::Keymap;
use crate::split::SplitStop;

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
        Source::Of(action) => Some(describe_action(&action, keymap, !shows_its_own_keycap(hit))),
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
    /// A control that dispatches `action`: described like the palette does.
    Of(Action),
}

fn text(s: &str) -> Option<Source> {
    Some(Source::Text(s.to_string()))
}

fn of(action: Action) -> Option<Source> {
    Some(Source::Of(action))
}

/// One line for `action`: the palette command's description plus its key,
/// else [`fallback_description`]'s wording, else the action's debug name
/// (a last resort that keeps every chip hintable).
fn describe_action(action: &Action, keymap: &Keymap, with_key: bool) -> String {
    if let Some(cmd) = all_commands().into_iter().find(|c| c.action == *action) {
        // The palette has a whole row for its caveats ("(live preview;
        // Esc reverts)"); the footer line keeps just the verb phrase.
        let full = match cmd.description.rfind(" (") {
            Some(i) if cmd.description.ends_with(')') => &cmd.description[..i],
            _ => cmd.description,
        };
        let description = short_description(action).unwrap_or(full);
        let combo = with_key
            .then(|| keymap_action_name(cmd.id).and_then(|n| keymap.combo_for(n)))
            .flatten();
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
        Action::OpenPalette => "Open the command palette".to_string(),
        Action::CancelSend => "Cancel the request in flight".to_string(),
        Action::CycleSplit => "Step the editor/response split".to_string(),
        Action::CycleSplitBack => "Step the split back".to_string(),
        Action::DeleteTableRow(_) => "Delete this row".to_string(),
        Action::CancelJqEdit => "Undo the edits to this filter".to_string(),
        Action::ToggleJqBar => "Turn the jq filter off".to_string(),
        Action::ResponseViewMode(ViewMode::Raw) => "Show the raw response body".to_string(),
        Action::ResponseViewMode(ViewMode::Headers) => "Show the response headers".to_string(),
        Action::ResponseViewMode(ViewMode::Pretty) => "Show the body as a JSON tree".to_string(),
        Action::CloseScreen => "Go back to the request screen".to_string(),
        Action::SelectManageTab(tab) => format!("Show the {} tab", tab.label()),
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
        | Hit::BodyEditor
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
        Hit::HeaderManage => text(if ctx.on {
            "Close the Manage screen"
        } else {
            "Open the Manage screen"
        }),
        Hit::HeaderTheme => of(Action::OpenThemeChooser),
        // The row toggle's chip drives the same two-state control as the
        // row's own checkbox, so it says the same thing. Handled here
        // rather than through `describe_action`, which has no `ctx`.
        Hit::FooterChip(Action::ToggleTableRow(_)) => text(if ctx.on {
            "Disable this row"
        } else {
            "Enable this row"
        }),
        Hit::FooterChip(action) => of(action.clone()),

        // -- Manage screen --
        Hit::ManageNew => Some(Source::Text(format!(
            "Create a new {}",
            manage_noun(ctx.manage_tab)
        ))),
        Hit::ManageRename => Some(Source::Text(format!(
            "Rename this {}",
            manage_noun(ctx.manage_tab)
        ))),
        Hit::ManageDelete => Some(Source::Text(format!(
            "Delete this {}",
            manage_noun(ctx.manage_tab)
        ))),
        Hit::ManageMoveAll => text("Move all requests to another space"),

        // -- Settings tab --
        // Each control names the single action a click performs, in the
        // state it is currently in. `ctx.on` is the tick the user sees:
        // `ai_confirmed` is stored inverted from its row, which is
        // worded as the consent question.
        Hit::SettingsControl(SettingsField::Animations) => text(if ctx.on {
            "Turn eased transitions off"
        } else {
            "Turn eased transitions on"
        }),
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
        Hit::SettingsFile { file, reset: false } => {
            Some(Source::Text(format!("Open {} in your editor", file.name())))
        }
        Hit::SettingsFile { file, reset: true } => Some(Source::Text(format!(
            "Reset {} to its defaults",
            file.name()
        ))),
        Hit::ManageEnvTls(None) => text("Let each request decide on TLS checks"),
        Hit::ManageEnvTls(Some(postui_core::project::TlsPolicy::Verify)) => {
            text("Always check TLS certificates here")
        }
        Hit::ManageEnvTls(Some(postui_core::project::TlsPolicy::Insecure)) => {
            text("Never check TLS certificates here")
        }

        // -- Sidebar --
        Hit::SidebarNewRequest => of(Action::PromptNewRequest),
        Hit::SidebarFolderArrow(_) => text(if ctx.on {
            "Collapse this folder"
        } else {
            "Expand this folder"
        }),

        // -- Editor --
        // The button itself flips to "Cancel" on hover while a send is in
        // flight (`components::editor`); its hint flips with it, rather
        // than naming both actions and leaving the reader to work out
        // which one a click would do.
        Hit::SendButton if ctx.on => of(Action::CancelSend),
        Hit::SendButton => of(Action::Send),
        Hit::MethodSelector => of(Action::OpenMethodDropdown),
        Hit::CopyUrl => of(Action::CopyToClipboard(CopyTarget::Url)),
        Hit::TableCheckbox(_) => text(if ctx.on {
            "Disable this row"
        } else {
            "Enable this row"
        }),
        Hit::TableCopy(_) => text("Copy this row's value"),
        Hit::TableDelete(_) => text("Delete this row"),
        Hit::SplitStop(stop) => text(match stop {
            SplitStop::EditorFull => "Editor full size",
            SplitStop::EditorBig => "Split 75/25",
            SplitStop::Even => "Split evenly",
            SplitStop::ResponseBig => "Split 25/75",
            SplitStop::ResponseFull => "Response full size",
        }),
        Hit::SplitStep(d) => text(if *d > 0 {
            "Give the response more room"
        } else {
            "Give the editor more room"
        }),
        Hit::AutoHeaderCopy(_) => text("Copy this header's value"),
        Hit::AutoHeaderReveal => text(if ctx.on {
            "Hide the computed secrets"
        } else {
            "Show the computed secrets"
        }),

        // -- Response --
        // The toolbar's three buttons act on the tab that is up, not on
        // the body: they dispatch `CopyTarget::ResponseView` and
        // `PromptSaveView`, where the palette's own copy/save commands
        // always take the body. So they say what the open tab holds
        // rather than borrowing those palette lines.
        Hit::CopyBodyButton => text(if ctx.on {
            "Copy the response headers"
        } else {
            "Copy the response body"
        }),
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
        Hit::ResponseSearchButton => text(if ctx.on {
            "Search the response headers"
        } else {
            "Search the response body"
        }),
        Hit::ResponseSearchNext => text("Go to the next match"),
        Hit::ResponseSearchPrev => text("Go to the previous match"),
        Hit::HeaderCopy(_) => text("Copy this header's value"),
        Hit::ResponseJqButton => of(Action::OpenJqBar),
        Hit::ResponseJqAiButton => of(Action::OpenJqDescribe),

        // -- Choosers, pickers, modals --
        Hit::ChooserToggle => text(if ctx.on {
            "Show the light themes instead"
        } else {
            "Show the dark themes instead"
        }),
        Hit::PickerPrimary => text(if ctx.on {
            "Save under the typed name"
        } else {
            "Open the shown folder"
        }),
        Hit::PickerHidden => text(if ctx.on {
            "Hide the hidden files again"
        } else {
            "Show the hidden files too"
        }),
        Hit::NewProjectBrowse => text("Browse for the project folder"),
        Hit::ConfirmChoice(_) => text("Answer with this choice"),
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
        Hit::ModalCancel => text("Close without changes \u{b7} esc"),
        Hit::ModalConfirm => text("Confirm and close \u{b7} enter"),
        Hit::ModalChoiceArrow { dir, .. } => text(if *dir > 0 {
            "Go to the next choice"
        } else {
            "Go to the previous choice"
        }),
        Hit::ModalRowToggle(_) => text(if ctx.on {
            "Bring this field back"
        } else {
            "Remove this field"
        }),
        Hit::ModalAddRow => text("Add another field"),
        Hit::ModalSharedToggle => text("Use the same options in every environment"),
        // Names the scope the chosen Write-to row would clear, the way
        // the modal's own remove chip does — "this scope" makes the
        // reader look back up at the popup to find out which.
        Hit::ModalRemove => text(match ctx.remove_scope {
            Some(ExtractDestination::Request) => "Remove this request's value",
            Some(ExtractDestination::ActiveEnv) => "Remove this environment's value",
            _ => "Remove the project default value",
        }),
        Hit::TipCopy(_) => text("Copy this variable's value"),
        Hit::TipReveal(_) => text(if ctx.on {
            "Hide this secret's value"
        } else {
            "Show this secret's value"
        }),

        // -- Variable Manager --
        Hit::VmNewVar => of(Action::PromptNewVar),
        Hit::VmNewSelector => of(Action::PromptNewSelector),
        Hit::VmSecretToggle => text(if ctx.on {
            "Stop treating this as a secret"
        } else {
            "Mark this variable as a secret"
        }),
        Hit::VmRevealToggle => text(if ctx.on {
            "Hide this secret's value"
        } else {
            "Show this secret's value"
        }),
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

    /// The Variable Manager's shared `[Rename]`/`[Delete]` buttons act on
    /// whatever the detail pane has open, so the hint has to name it.
    #[test]
    fn the_managers_rename_and_delete_name_what_is_open() {
        let keymap = Keymap::default_bindings();
        for (noun, expected) in [
            (VmNoun::Variable, "variable"),
            (VmNoun::Selector, "selector"),
        ] {
            let ctx = HintCtx {
                vm_noun: noun,
                ..ctx()
            };
            for hit in [Hit::VmRename, Hit::VmDelete] {
                let h = hint_for(&hit, &keymap, &ctx).unwrap();
                assert!(h.contains(expected), "{hit:?} with {noun:?} open said {h:?}");
            }
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

    /// A control that paints the key on itself doesn't repeat it: the
    /// Theme chip's own `alt+t` pill sits a hand's width from the footer.
    #[test]
    fn a_keycapped_control_leaves_the_key_off() {
        let keymap = Keymap::default_bindings();
        assert_eq!(
            hint_for(&Hit::HeaderTheme, &keymap, &ctx()).unwrap(),
            "Pick a color theme"
        );
        assert_eq!(
            hint_for(
                &Hit::FooterChip(Action::PromptRenameRequest),
                &keymap,
                &ctx()
            )
            .unwrap(),
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
        assert_eq!(
            hint_for(&Hit::TableCopy(0), &keymap, &ctx()).unwrap(),
            "Copy this row's value"
        );
        assert!(hint_for(&Hit::FooterChip(Action::Quit), &keymap, &ctx()).is_some());
    }

    /// A palette caveat in parentheses stays in the palette.
    #[test]
    fn palette_parentheticals_are_dropped() {
        let keymap = Keymap::default_bindings();
        assert_eq!(
            hint_for(
                &Hit::FooterChip(Action::DeleteSelectedRequest),
                &keymap,
                &ctx()
            )
            .unwrap(),
            "Delete the open request"
        );
    }

    /// A chip action the palette doesn't list still gets real words, not
    /// the `Debug` last resort — including the `alt+w` split pill, which
    /// is a `FooterChip` hit painted on the editor's tab strip.
    #[test]
    fn chip_actions_outside_the_palette_use_the_fallback_wording() {
        let keymap = Keymap::default_bindings();
        assert_eq!(
            hint_for(&Hit::FooterChip(Action::CycleSplit), &keymap, &ctx()).unwrap(),
            "Step the editor/response split"
        );
        assert_eq!(
            hint_for(&Hit::FooterChip(Action::DeleteTableRow(3)), &keymap, &ctx()).unwrap(),
            "Delete this row"
        );
    }

    /// No control falls through to the `{action:?}` last resort: every
    /// action the footer paints as a chip, in every pane and state, has
    /// real words. (`alt+w` read "CycleSplit" before this test existed.)
    #[test]
    fn no_chip_falls_through_to_the_debug_name() {
        use crate::components::footer::{JqBarState, footer_chips};
        use crate::layout::PaneId;
        let keymap = Keymap::default_bindings();
        for focus in [PaneId::Sidebar, PaneId::Editor, PaneId::Response] {
            for sending in [false, true] {
                for url_focused in [false, true] {
                    for row in [None, Some((0, true))] {
                        for jq in [
                            JqBarState::Closed,
                            JqBarState::Open,
                            JqBarState::Focused,
                            JqBarState::Menu,
                            JqBarState::Completing { cycle: true },
                            JqBarState::Completing { cycle: false },
                        ] {
                            let chips = footer_chips(
                                focus,
                                false,
                                sending,
                                Some("add param"),
                                url_focused,
                                row,
                                jq,
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
        // The split pill and the always-present right-hand pair, which
        // `footer_chips` doesn't list.
        for action in [
            Action::CycleSplit,
            Action::CycleSplitBack,
            Action::OpenPalette,
            Action::Quit,
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
            hint_for(
                &Hit::ModalRemove,
                &keymap,
                &HintCtx {
                    remove_scope: Some(d),
                    ..ctx()
                },
            )
            .unwrap()
        };
        assert_eq!(
            for_scope(ExtractDestination::Request),
            "Remove this request's value"
        );
        assert_eq!(
            for_scope(ExtractDestination::ActiveEnv),
            "Remove this environment's value"
        );
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
            Hit::VmSecretToggle,
            Hit::CopyBodyButton,
            Hit::SaveBodyButton,
            Hit::ResponseEditorButton,
            Hit::ResponseSearchButton,
            Hit::SettingsControl(crate::components::settings::SettingsField::Animations),
            Hit::SettingsControl(crate::components::settings::SettingsField::HoverHints),
            Hit::SettingsControl(crate::components::settings::SettingsField::AiConfirmed),
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
            hint_for(
                &Hit::EditorTab(EditorTab::Headers.draw_position()),
                &keymap,
                &ctx()
            )
            .unwrap(),
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
            Hit::FooterChip(Action::CycleSplit),
            Hit::ManageMoveAll,
            Hit::ManageEnvTls(Some(postui_core::project::TlsPolicy::Insecure)),
            Hit::AutoHeaderReveal,
            Hit::ChooserToggle,
            Hit::ModalSharedToggle,
            Hit::ModalRowToggle(0),
            Hit::ModalRemove,
            Hit::PickerPrimary,
            Hit::ResponseSearchNext,
            Hit::SplitStep(1),
            Hit::TipReveal("tok".into()),
            Hit::VmEntryRadio(0),
            Hit::VmPromoteBtn,
            Hit::VmRemoveEnvValue,
            Hit::VmSecretToggle,
            Hit::FooterChip(Action::CancelJqEdit),
            Hit::FooterChip(Action::ToggleJqBar),
            Hit::FooterChip(Action::CloseScreen),
            Hit::ResponseJqAiButton,
            Hit::ResponseEditorButton,
            Hit::CopyBodyButton,
            Hit::SaveBodyButton,
            Hit::CopyUrl,
            Hit::SidebarNewRequest,
            Hit::MethodSelector,
            Hit::EditorTab(0),
            Hit::ResponseTab(crate::components::response::ViewMode::Pretty),
            Hit::ManageTab(0),
            Hit::SettingsControl(crate::components::settings::SettingsField::AiCmd),
            Hit::SettingsControl(crate::components::settings::SettingsField::Osc52Limit),
            Hit::SettingsJqTab(crate::config::JqTab::Menu),
            Hit::SettingsJqTab(crate::config::JqTab::Cycle),
            Hit::SettingsFile {
                file: crate::action::ConfigFile::Config,
                reset: false,
            },
            Hit::SettingsFile {
                file: crate::action::ConfigFile::Keys,
                reset: true,
            },
        ] {
            let h = hint_for(&hit, &keymap, &ctx()).unwrap();
            // The key, where one is appended, doesn't count against the
            // wording — it is the same three or four cells everywhere.
            let words = h.split(" \u{b7} ").next().unwrap();
            assert!(
                words.chars().count() <= 42,
                "too long \u{2014} {hit:?}: {h:?}"
            );
            assert!(
                !words.contains(';') && !words.contains('('),
                "one clause, no caveats \u{2014} {hit:?}: {h:?}"
            );
            assert!(
                !words.ends_with('.'),
                "no full stop \u{2014} {hit:?}: {h:?}"
            );
            let first = words.chars().next().unwrap();
            assert!(
                first.is_uppercase(),
                "sentence case \u{2014} {hit:?}: {h:?}"
            );
        }
    }
}

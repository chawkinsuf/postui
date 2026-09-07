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

/// The app state a hint needs to say what its button would do *now*. A
/// control whose label changes with state (Send becomes Cancel in flight)
/// must have a hint that changes with it, or the footer contradicts the
/// button the pointer is sitting on. Kept to exactly the controls with
/// that problem — everything else is a pure `Hit` → line lookup.
#[derive(Debug, Default, Clone, Copy)]
pub struct HintCtx {
    /// The open request is in flight, so the Send button is a Cancel
    /// button (`components::editor` swaps its label on hover).
    pub sending: bool,
    /// The scope the open value popup's "✕ remove" would clear, from
    /// `ModalStack::value_popup_remove_scope`.
    pub remove_scope: Option<ExtractDestination>,
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
        Action::OpenJqDescribe => "Write a jq filter from a sentence",
        Action::OpenResponseInEditor => "Open the response in your editor",
        Action::CopyToClipboard(CopyTarget::Url) => "Copy the resolved URL",
        Action::CopyToClipboard(CopyTarget::ResponseBody) => "Copy the response body",
        Action::OpenJqBar => "Filter the JSON with jq",
        Action::OpenResponseSearch => "Search the response body",
        Action::PromptNewRequest => "Create a request in this folder",
        Action::PromptSaveBody => "Save the response body to a file",
        // "Open" over "Switch to", matching the ^O the hint carries; the
        // space and env choosers keep "Switch to", since neither is a
        // thing you open.
        Action::OpenProjectChooser => "Open another project",
        Action::CycleProject(1) => "Switch to the next project",
        Action::CycleEnv(1) => "Switch to the next environment",
        Action::OpenMethodDropdown => "Pick an HTTP method",
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
        Action::ToggleTableRow(_) => "Enable or disable this row".to_string(),
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
    match hit {
        // -- Surfaces, not buttons: no hint. --
        Hit::Pane(_)
        | Hit::ManageRow(_)
        | Hit::SidebarRow(_)
        | Hit::UrlBar
        | Hit::EditorTab(_)
        | Hit::TableRow(_)
        | Hit::TableCell { .. }
        | Hit::BodyEditor
        | Hit::ResponseTab(_)
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
        | Hit::ResponseJqBar
        | Hit::ManageTab(_) => None,

        // -- Header --
        Hit::HeaderProject => of(Action::OpenProjectChooser),
        Hit::HeaderProjectCycle => of(Action::CycleProject(1)),
        Hit::HeaderSpace => text("Switch to another space"),
        Hit::HeaderSpaceCycle => text("Switch to the next space"),
        Hit::HeaderEnv => of(Action::OpenEnvChooser),
        Hit::HeaderEnvCycle => of(Action::CycleEnv(1)),
        Hit::HeaderManage => text("Open or close the Manage screen"),
        Hit::HeaderTheme => of(Action::OpenThemeChooser),
        Hit::FooterChip(action) => of(action.clone()),

        // -- Manage screen --
        Hit::ManageNew => text("Create a new space or environment"),
        Hit::ManageRename => text("Rename the selected row"),
        Hit::ManageDelete => text("Delete the selected row"),
        Hit::ManageMoveAll => text("Move all requests to another space"),
        Hit::ManageEnvTls(None) => text("Let each request decide on TLS checks"),
        Hit::ManageEnvTls(Some(postui_core::project::TlsPolicy::Verify)) => {
            text("Always check TLS certificates here")
        }
        Hit::ManageEnvTls(Some(postui_core::project::TlsPolicy::Insecure)) => {
            text("Never check TLS certificates here")
        }

        // -- Sidebar --
        Hit::SidebarNewRequest => of(Action::PromptNewRequest),
        Hit::SidebarFolderArrow(_) => text("Expand or collapse this folder"),

        // -- Editor --
        // The button itself flips to "Cancel" on hover while a send is in
        // flight (`components::editor`); its hint flips with it, rather
        // than naming both actions and leaving the reader to work out
        // which one a click would do.
        Hit::SendButton if ctx.sending => of(Action::CancelSend),
        Hit::SendButton => of(Action::Send),
        Hit::MethodSelector => of(Action::OpenMethodDropdown),
        Hit::CopyUrl => of(Action::CopyToClipboard(CopyTarget::Url)),
        Hit::TableCheckbox(_) => text("Enable or disable this row"),
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
        Hit::AutoHeaderReveal => text("Show or hide the computed secrets"),

        // -- Response --
        Hit::CopyBodyButton => of(Action::CopyToClipboard(CopyTarget::ResponseBody)),
        Hit::SaveBodyButton => of(Action::PromptSaveBody),
        Hit::ResponseEditorButton => of(Action::OpenResponseInEditor),
        Hit::ResponseSearchButton => of(Action::OpenResponseSearch),
        Hit::ResponseSearchNext => text("Go to the next match"),
        Hit::ResponseSearchPrev => text("Go to the previous match"),
        Hit::HeaderCopy(_) => text("Copy this header's value"),
        Hit::ResponseJqButton => of(Action::OpenJqBar),
        Hit::ResponseJqAiButton => of(Action::OpenJqDescribe),

        // -- Choosers, pickers, modals --
        Hit::ChooserToggle => text("Filter the list to dark or light themes"),
        Hit::PickerPrimary => text("Confirm the typed name or shown folder"),
        Hit::PickerHidden => text("Show or hide hidden files"),
        Hit::NewProjectBrowse => text("Browse for the project folder"),
        Hit::ConfirmChoice(_) => text("Answer with this choice"),
        Hit::ModalCancel => text("Close without changes \u{b7} esc"),
        Hit::ModalConfirm => text("Confirm and close \u{b7} enter"),
        Hit::ModalChoiceArrow { dir, .. } => text(if *dir > 0 {
            "Go to the next choice"
        } else {
            "Go to the previous choice"
        }),
        Hit::ModalRowToggle(_) => text("Remove this field, or bring it back"),
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
        Hit::TipCopy(_) => text("Copy this variable's real value"),
        Hit::TipReveal(_) => text("Show or hide this secret's value"),

        // -- Variable Manager --
        Hit::VmNewVar => of(Action::PromptNewVar),
        Hit::VmNewSelector => of(Action::PromptNewSelector),
        Hit::VmSecretToggle => text("Mark this variable as a secret"),
        Hit::VmRevealToggle => text("Show or hide this secret's value"),
        Hit::VmRemoveEnvValue => text("Remove this environment's value"),
        Hit::VmRename => text("Rename this variable"),
        Hit::VmDelete => text("Delete this variable"),
        Hit::VmEntryRadio(_) => text("Select this option for the environment"),
        Hit::VmEntryDelete(_) => text("Delete this option"),
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
            hint_for(&Hit::FooterChip(Action::PromptRenameRequest), &keymap, &ctx()).unwrap(),
            "Rename the open request"
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
            hint_for(&Hit::FooterChip(Action::ToggleTableRow(3)), &keymap, &ctx()).unwrap(),
            "Enable or disable this row"
        );
    }

    /// The Send button's label flips to Cancel in flight; so does its hint.
    #[test]
    fn the_send_hint_says_which_action_a_click_would_do() {
        let keymap = Keymap::default_bindings();
        let idle = hint_for(&Hit::SendButton, &keymap, &ctx()).unwrap();
        assert!(idle.starts_with("Send the open request"), "{idle:?}");
        let busy = hint_for(
            &Hit::SendButton,
            &keymap,
            &HintCtx {
                sending: true,
                ..ctx()
            },
        )
        .unwrap();
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

    /// Hints read as one voice: an imperative phrase, one clause, no
    /// trailing stop, and short enough to sit beside the chips whole.
    #[test]
    fn hints_share_one_shape() {
        let keymap = Keymap::default_bindings();
        for hit in [
            Hit::ManageNew,
            Hit::ManageRename,
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
            Hit::CopyUrl,
            Hit::SidebarNewRequest,
            Hit::MethodSelector,
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

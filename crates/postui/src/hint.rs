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

use crate::action::{Action, CopyTarget};
use crate::components::palette::{all_commands, keymap_action_name};
use crate::hit::Hit;
use crate::keys::Keymap;
use crate::split::SplitStop;

/// What the footer should say while `hit` is hovered, or `None` for a hit
/// that is not a button. Where the hit is a button for an action the
/// palette also lists, the hint is that command's description with the
/// action's bound key appended (`… · alt+t`), so the mouse hover teaches
/// the keyboard route too; a key-less description stands alone.
pub fn hint_for(hit: &Hit, keymap: &Keymap) -> Option<String> {
    match hint_source(hit)? {
        Source::Text(s) => Some(s),
        Source::Of(action) => Some(describe_action(&action, keymap)),
    }
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
fn describe_action(action: &Action, keymap: &Keymap) -> String {
    if let Some(cmd) = all_commands().into_iter().find(|c| c.action == *action) {
        return match keymap_action_name(cmd.id).and_then(|n| keymap.combo_for(n)) {
            Some(combo) => format!("{} \u{b7} {combo}", cmd.description),
            None => cmd.description.to_string(),
        };
    }
    fallback_description(action).unwrap_or_else(|| format!("{action:?}"))
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
        Action::ToggleTableRow(_) => "Enable or disable the selected row".to_string(),
        Action::DeleteTableRow(_) => "Delete the selected row (undoable)".to_string(),
        Action::CancelJqEdit => "Leave the bar; restore the previous filter".to_string(),
        Action::ToggleJqBar => "Close the bar; show the unfiltered body".to_string(),
        Action::ResponseViewMode(ViewMode::Raw) => "Raw response body".to_string(),
        Action::ResponseViewMode(ViewMode::Headers) => "Response headers".to_string(),
        Action::ResponseViewMode(ViewMode::Pretty) => "Response body as a JSON tree".to_string(),
        Action::CloseScreen => "Back to the request screen".to_string(),
        Action::SelectManageTab(tab) => format!("Show the {} tab", tab.label()),
        _ => return None,
    })
}

fn hint_source(hit: &Hit) -> Option<Source> {
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
        Hit::HeaderSpaceCycle => text("Cycle to the next space"),
        Hit::HeaderEnv => of(Action::OpenEnvChooser),
        Hit::HeaderEnvCycle => of(Action::CycleEnv(1)),
        Hit::HeaderManage => text("Open or close the Manage screen"),
        Hit::HeaderTheme => of(Action::OpenThemeChooser),
        Hit::FooterChip(action) => of(action.clone()),

        // -- Manage screen --
        Hit::ManageNew => text("Create a new space or environment"),
        Hit::ManageRename => text("Rename the selected space or environment"),
        Hit::ManageDelete => text("Delete the selected space or environment"),
        Hit::ManageMoveAll => text("Move all this space's requests elsewhere"),
        Hit::ManageEnvTls(None) => text("TLS checks: each request decides"),
        Hit::ManageEnvTls(Some(postui_core::project::TlsPolicy::Verify)) => {
            text("TLS checks: always verify here")
        }
        Hit::ManageEnvTls(Some(postui_core::project::TlsPolicy::Insecure)) => {
            text("TLS checks: always skip here")
        }

        // -- Sidebar --
        Hit::SidebarNewRequest => of(Action::PromptNewRequest),
        Hit::SidebarFolderArrow(_) => text("Expand or collapse this folder"),

        // -- Editor --
        Hit::SendButton => text("Send the request (cancel while in flight)"),
        Hit::MethodSelector => of(Action::OpenMethodDropdown),
        Hit::CopyUrl => of(Action::CopyToClipboard(CopyTarget::Url)),
        Hit::TableCheckbox(_) => text("Enable or disable this row"),
        Hit::TableDelete(_) => text("Delete this row (undoable)"),
        Hit::SplitStop(stop) => text(match stop {
            SplitStop::EditorFull => "Editor takes the whole column",
            SplitStop::EditorBig => "Split 75/25, editor first",
            SplitStop::Even => "Split evenly",
            SplitStop::ResponseBig => "Split 75/25, response first",
            SplitStop::ResponseFull => "Response takes the whole column",
        }),
        Hit::SplitStep(d) => text(if *d > 0 {
            "Give the response pane more room"
        } else {
            "Give the editor pane more room"
        }),
        Hit::AutoHeaderCopy(_) => text("Copy this header's value"),
        Hit::AutoHeaderReveal => text("Reveal or mask computed secrets"),

        // -- Response --
        Hit::CopyBodyButton => of(Action::CopyToClipboard(CopyTarget::ResponseBody)),
        Hit::SaveBodyButton => of(Action::PromptSaveBody),
        Hit::ResponseEditorButton => of(Action::OpenResponseInEditor),
        Hit::ResponseSearchButton => of(Action::OpenResponseSearch),
        Hit::ResponseSearchNext => text("Jump to the next match"),
        Hit::ResponseSearchPrev => text("Jump to the previous match"),
        Hit::HeaderCopy(_) => text("Copy this header's value"),
        Hit::ResponseJqButton => of(Action::OpenJqBar),
        Hit::ResponseJqAiButton => of(Action::OpenJqDescribe),

        // -- Choosers, pickers, modals --
        Hit::ChooserToggle => text("Filter to dark or light themes"),
        Hit::PickerPrimary => text("Confirm the typed name or shown folder"),
        Hit::PickerHidden => text("Show or hide dotfiles"),
        Hit::NewProjectBrowse => text("Browse for the project folder"),
        Hit::ConfirmChoice(_) => text("Answer with this choice"),
        Hit::ModalCancel => text("Close without changes (esc)"),
        Hit::ModalConfirm => text("Confirm (enter)"),
        Hit::ModalChoiceArrow { dir, .. } => text(if *dir > 0 {
            "Next choice"
        } else {
            "Previous choice"
        }),
        Hit::ModalRowToggle(_) => text("Remove this field, or put it back"),
        Hit::ModalAddRow => text("Add another field"),
        Hit::ModalSharedToggle => text("Same options in every environment"),
        Hit::ModalRemove => text("Remove the stored value at this scope"),
        Hit::TipCopy(_) => text("Copy this variable's real value"),
        Hit::TipReveal(_) => text("Reveal or hide the secret's value"),

        // -- Variable Manager --
        Hit::VmNewVar => of(Action::PromptNewVar),
        Hit::VmNewSelector => of(Action::PromptNewSelector),
        Hit::VmSecretToggle => text("Toggle secret"),
        Hit::VmRevealToggle => text("Reveal or hide the secret's value"),
        Hit::VmRemoveEnvValue => text("Remove this environment's value"),
        Hit::VmRename => text("Rename this variable"),
        Hit::VmDelete => text("Delete this variable"),
        Hit::VmEntryRadio(_) => text("Use this option in the active environment"),
        Hit::VmEntryDelete(_) => text("Delete this option"),
        Hit::VmNewOption => text("Add an option"),
        Hit::VmEditFields => text("Edit the group's field list"),
        Hit::VmPromoteBtn => text("Promote the request's value to the variable"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palette_backed_hints_carry_the_bound_key() {
        let keymap = Keymap::default_bindings();
        assert_eq!(
            hint_for(&Hit::HeaderTheme, &keymap).unwrap(),
            "Pick a color theme (live preview; Esc reverts) \u{b7} alt+t"
        );
    }

    #[test]
    fn surfaces_have_no_hint_and_buttons_always_do() {
        let keymap = Keymap::default_bindings();
        assert_eq!(hint_for(&Hit::UrlBar, &keymap), None);
        assert_eq!(hint_for(&Hit::SidebarRow(0), &keymap), None);
        assert!(hint_for(&Hit::TableDelete(0), &keymap).is_some());
        assert!(hint_for(&Hit::FooterChip(Action::Quit), &keymap).is_some());
    }

    /// A chip action the palette doesn't list still gets real words, not
    /// the `Debug` last resort.
    #[test]
    fn chip_actions_outside_the_palette_use_the_fallback_wording() {
        let keymap = Keymap::default_bindings();
        assert_eq!(
            hint_for(&Hit::FooterChip(Action::ToggleTableRow(3)), &keymap).unwrap(),
            "Enable or disable the selected row"
        );
    }

    /// Hints are succinct: the fixed ones stay well under the footer's
    /// swap threshold so they usually sit beside the chips whole.
    #[test]
    fn hints_stay_short() {
        let keymap = Keymap::default_bindings();
        for hit in [
            Hit::ManageMoveAll,
            Hit::AutoHeaderReveal,
            Hit::VmRemoveEnvValue,
            Hit::VmPromoteBtn,
            Hit::ModalSharedToggle,
        ] {
            let h = hint_for(&hit, &keymap).unwrap();
            assert!(h.chars().count() <= 44, "{hit:?}: {h:?}");
        }
    }
}

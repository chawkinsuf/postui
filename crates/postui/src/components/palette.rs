use super::line_input::LineInput;
use crate::action::{Action, CopyTarget};
use crate::layout::PaneId;
use crate::paint::{self, ControlState, FIELD_HEIGHT, ListRow, RowHighlight, TextField};
use crate::theme::Theme;
use crate::ui::clip;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;

#[derive(Clone)]
pub struct Command {
    /// A stable kebab-case identifier, independent of the display `name`,
    /// used as the key for palette frecency stats (`UsageStore`).
    pub id: &'static str,
    pub name: &'static str,
    /// A one-line muted caption painted on the row directly under `name`
    /// (spec: "bold title + muted description", two-line dense options).
    pub description: &'static str,
    pub action: Action,
}

pub fn all_commands() -> Vec<Command> {
    vec![
        Command {
            id: "focus-sidebar",
            name: "Focus: request tree",
            description: "Move keyboard focus to the sidebar",
            action: Action::FocusPane(PaneId::Sidebar),
        },
        Command {
            id: "focus-editor",
            name: "Focus: editor",
            description: "Move keyboard focus to the request editor",
            action: Action::FocusPane(PaneId::Editor),
        },
        Command {
            id: "focus-response",
            name: "Focus: response",
            description: "Move keyboard focus to the response pane",
            action: Action::FocusPane(PaneId::Response),
        },
        Command {
            id: "about",
            name: "Help: about postui",
            description: "Show version and app info",
            action: Action::ShowAbout,
        },
        Command {
            id: "theme-choose",
            name: "Change theme…",
            description: "Pick a color theme (live preview; Esc reverts)",
            action: Action::OpenThemeChooser,
        },
        Command {
            id: "send",
            name: "Send request",
            description: "Send the open request",
            action: Action::Send,
        },
        Command {
            id: "table-add-row",
            name: "Table: add row",
            description: "Add a row to the active Params/Headers/Vars table",
            action: Action::TableAddRow,
        },
        Command {
            id: "request-new",
            name: "Request: new",
            description: "Create a new request in the current folder",
            action: Action::PromptNewRequest,
        },
        Command {
            id: "request-import-curl",
            name: "Request: import curl from clipboard",
            description: "Create requests from the curl commands on the clipboard",
            action: Action::ImportCurl { folder: None },
        },
        Command {
            id: "request-save",
            name: "Request: save",
            description: "Save the open request to disk",
            action: Action::SaveRequest,
        },
        Command {
            id: "request-discard",
            name: "Request: discard changes",
            description: "Revert unsaved edits to the saved request (undoable)",
            action: Action::DiscardChanges,
        },
        Command {
            id: "request-rename",
            name: "Request: rename",
            description: "Rename the open request",
            action: Action::PromptRenameRequest,
        },
        Command {
            id: "request-duplicate",
            name: "Request: duplicate",
            description: "Copy the open request under a new name",
            action: Action::DuplicateRequest,
        },
        Command {
            id: "request-copy-curl",
            name: "Request: copy as curl",
            description: "Copy the open request as a curl command, variables resolved",
            action: Action::CopyAsCurl(None),
        },
        Command {
            id: "request-replace-curl",
            name: "Request: replace with curl from clipboard",
            description: "Replace the open request with the curl command on the clipboard",
            action: Action::ReplaceWithCurl(None),
        },
        Command {
            id: "request-delete",
            name: "Request: delete",
            description: "Delete the open request (undo restores it)",
            action: Action::DeleteSelectedRequest,
        },
        Command {
            id: "request-move-to-space",
            name: "Request: move to space",
            description: "Move the selected request into another space",
            action: Action::PromptMoveSelectedRequestToSpace,
        },
        Command {
            id: "method-cycle",
            name: "Method: cycle",
            description: "Step to the next HTTP method",
            action: Action::CycleMethod,
        },
        Command {
            id: "method-choose",
            name: "Method: choose…",
            description: "Pick an HTTP method from a list",
            action: Action::OpenMethodDropdown,
        },
        Command {
            id: "body-format",
            name: "Body: format JSON",
            description: "Pretty-print the request body",
            action: Action::FormatBody,
        },
        Command {
            id: "body-minify",
            name: "Body: minify JSON",
            description: "Collapse the request body to one line",
            action: Action::MinifyBody,
        },
        Command {
            id: "body-clear",
            name: "Body: clear",
            description: "Empty the request body (undoable)",
            action: Action::BodyClear,
        },
        Command {
            id: "body-external-editor",
            name: "Body: open in $EDITOR",
            description: "Edit the body in your external editor",
            action: Action::OpenBodyInEditor,
        },
        Command {
            id: "body-toggle-vars",
            name: "Body: toggle {{var}} substitution",
            description: "Enable or disable variable substitution in the body",
            action: Action::ToggleBodyVars,
        },
        Command {
            id: "tls-toggle-verify",
            name: "TLS: toggle certificate verification",
            description: "Skip or restore TLS certificate checks for this request (insecure)",
            action: Action::ToggleInsecure,
        },
        Command {
            id: "response-toggle",
            name: "Response: hide/show panel",
            description: "Collapse the response pane to its header, or bring it back",
            action: Action::ToggleResponseCollapse,
        },
        Command {
            id: "editor-toggle",
            name: "Editor: hide/show panel",
            description: "Collapse the editor pane to its address bar, or bring it back",
            action: Action::ToggleTableCollapse,
        },
        // Spec E12a: the split keys, findable here now that the split
        // chips' hints name the hide/show toggles' keys instead.
        Command {
            id: "split-step",
            name: "Step the split",
            description: "Move the split line to the next stop",
            action: Action::CycleSplit,
        },
        Command {
            id: "split-step-back",
            name: "Step the split back",
            description: "Move the split line to the previous stop",
            action: Action::CycleSplitBack,
        },
        // Spec S6b: the sidebar's width steps, findable here. No key column:
        // `alt+w` steps the sidebar only while it has focus.
        Command {
            id: "sidebar-widen",
            name: "Widen sidebar",
            description: "Move the sidebar's edge a few columns right",
            action: Action::StepSidebar(crate::layout::SIDEBAR_STEP_COLS),
        },
        Command {
            id: "sidebar-narrow",
            name: "Narrow sidebar",
            description: "Move the sidebar's edge a few columns left",
            action: Action::StepSidebar(-crate::layout::SIDEBAR_STEP_COLS),
        },
        Command {
            id: "project-choose",
            name: "Project: choose…",
            description: "Switch to another registered project",
            action: Action::OpenProjectChooser,
        },
        Command {
            id: "project-next",
            name: "Project: next",
            description: "Cycle to the next registered project",
            action: Action::CycleProject(1),
        },
        Command {
            id: "project-prev",
            name: "Project: previous",
            description: "Cycle to the previous registered project",
            action: Action::CycleProject(-1),
        },
        Command {
            id: "project-open-path",
            name: "Project: open by path…",
            description: "Browse to a project folder and open it",
            action: Action::PromptOpenProjectPath,
        },
        Command {
            id: "reload-from-disk",
            name: "Reload from disk",
            description: "Re-read the configuration and project files from disk",
            action: Action::ReloadFromDisk,
        },
        Command {
            id: "project-new",
            name: "Project: new…",
            description: "Create a new project",
            action: Action::PromptNewProject,
        },
        Command {
            id: "env-choose",
            name: "Environment: choose…",
            description: "Switch to another environment",
            action: Action::OpenEnvChooser,
        },
        Command {
            id: "env-next",
            name: "Environment: next",
            description: "Cycle to the next environment",
            action: Action::CycleEnv(1),
        },
        Command {
            id: "env-prev",
            name: "Environment: previous",
            description: "Cycle to the previous environment",
            action: Action::CycleEnv(-1),
        },
        Command {
            id: "env-new",
            name: "Environment: new…",
            description: "Create a new environment",
            action: Action::OpenNewEnvPrompt,
        },
        Command {
            id: "vars-insert",
            name: "Variables: insert…",
            description: "Insert a {{variable}} token at the cursor",
            action: Action::OpenVarPicker { completing: false },
        },
        // The variable tooltip's pills (#135). Listed only while the
        // caret's tooltip offers the pill, and then for its variable
        // (`PaletteState::with_tip`, `App::tip_offers`).
        Command {
            id: "vars-tip-edit",
            name: "Variables: edit the tooltip's variable",
            description: "Edit the variable whose tooltip is open, as its 󰏫 does",
            action: Action::TipPill(crate::action::TipPill::Edit),
        },
        Command {
            id: "vars-tip-copy",
            name: "Variables: copy the tooltip's value",
            description: "Copy the value of the variable whose tooltip is open",
            action: Action::TipPill(crate::action::TipPill::Copy),
        },
        Command {
            id: "vars-tip-reveal",
            name: "Variables: reveal the tooltip's secret",
            // The palette covering the tooltip masks its secret again (a
            // tip out of sight never stays revealed), so from here it
            // always reveals; alt+h on the tooltip toggles.
            description: "Show the secret in the tooltip",
            action: Action::TipPill(crate::action::TipPill::Reveal),
        },
        Command {
            id: "edit-paste",
            name: "Edit: paste",
            description: "Paste clipboard text at the cursor",
            action: Action::Paste,
        },
        Command {
            id: "response-copy-body",
            name: "Response: copy body",
            description: "Copy the response body to the clipboard",
            action: Action::CopyToClipboard(CopyTarget::ResponseBody),
        },
        Command {
            id: "request-copy-url",
            name: "Request: copy URL",
            description: "Copy the resolved request URL to the clipboard",
            action: Action::CopyToClipboard(CopyTarget::Url),
        },
        Command {
            id: "response-save-body",
            name: "Response: save body to file…",
            description: "Save the response body to a file on disk",
            action: Action::PromptSaveBody,
        },
        Command {
            id: "response-open-editor",
            name: "Response: open in $EDITOR",
            description: "View the response's active tab in your external editor",
            action: Action::OpenResponseInEditor,
        },
        Command {
            id: "response-search",
            name: "Response: search",
            description: "Search the response's active tab",
            action: Action::OpenResponseSearch { backward: false },
        },
        Command {
            id: "response-jq",
            name: "Response: jq filter",
            description: "Filter the JSON response with a jq expression",
            action: Action::OpenJqBar,
        },
        Command {
            id: "response-jq-describe",
            name: "Response: describe a filter (AI)…",
            description: "Ask the configured AI command to write the jq filter from a sentence",
            action: Action::OpenJqDescribe,
        },
        Command {
            id: "response-jq-views",
            name: "Response: jq views…",
            description: "Pick, save, rename or delete the request's jq views",
            action: Action::OpenJqViews,
        },
        Command {
            id: "response-jq-save-view",
            name: "Response: save filter as view…",
            description: "Save the jq bar's filter as a named view on the request",
            action: Action::PromptSaveJqView,
        },
        Command {
            id: "response-jq-revert-view",
            name: "Response: revert jq view",
            description: "Put the current jq view back to its saved filter",
            action: Action::RevertJqView,
        },
        Command {
            id: "response-jq-clear-filter",
            name: "Response: clear jq filter",
            description: "Start over with an empty jq bar and no view; the view keeps its filter",
            action: Action::ClearJqFilter,
        },
        Command {
            id: "manage-variables",
            name: "Manage: variables",
            description: "Open the Manage screen's Variables tab",
            action: Action::OpenManage {
                tab: Some(crate::components::manage::ManageTab::Variables),
            },
        },
        Command {
            id: "manage-environments",
            name: "Manage: environments",
            description: "Open the Manage screen's Environments tab",
            action: Action::OpenManage {
                tab: Some(crate::components::manage::ManageTab::Environments),
            },
        },
        Command {
            id: "manage-spaces",
            name: "Manage: spaces",
            description: "Open the Manage screen's Spaces tab",
            action: Action::OpenManage { tab: Some(crate::components::manage::ManageTab::Spaces) },
        },
        Command {
            id: "vars-new-variable",
            name: "Variables: new variable…",
            description: "Declare a new project variable",
            action: Action::PromptNewVar,
        },
        Command {
            id: "vars-new-selector",
            name: "Variables: new selector…",
            description: "Declare a new variable selector",
            action: Action::PromptNewSelector,
        },
        Command {
            id: "vars-extract",
            name: "Extract to variable",
            description: "Turn the selected text into a variable",
            action: Action::ExtractToVariable,
        },
        Command {
            id: "vars-extract-selector",
            name: "Extract to selector",
            description: "Turn the selected text into a new selector's only option",
            action: Action::ExtractToSelector,
        },
        Command {
            id: "undo",
            name: "Undo",
            description: "Undo the last change",
            action: Action::Undo,
        },
        Command {
            id: "redo",
            name: "Redo",
            description: "Redo the last undone change",
            action: Action::Redo,
        },
        // It meets the dirty gate, and is refused when vim's `:` line
        // opened over a dialog (`GatedQuit`, task #85).
        Command { id: "quit", name: "Quit", description: "Exit postui", action: Action::GatedQuit },
    ]
}

/// The action whose key a row with `action` shows: its own, except the
/// palette's Quit and the tooltip's pills. The Quit row runs `GatedQuit`,
/// the quit that is refused while a dialog is up (task #85), and the
/// keymap binds `^C` to `Quit`, the same quit as a key press; the row
/// shows `^C`. A pill row names the tooltip's variable (`TipPillOn`),
/// while its key presses the open tooltip's pill (`TipPill`, #135).
fn key_action(action: &Action) -> std::borrow::Cow<'_, Action> {
    use std::borrow::Cow;
    match action {
        Action::GatedQuit => Cow::Owned(Action::Quit),
        Action::TipPillOn { pill, .. } => Cow::Owned(Action::TipPill(*pill)),
        other => Cow::Borrowed(other),
    }
}

/// Case-insensitive subsequence match: every char of `needle` appears in
/// `haystack`, in order, with any gaps. The chooser and variable picker
/// filter on this alone; the palette ranks the survivors with
/// [`match_rank`].
pub fn fuzzy_match(needle: &str, haystack: &str) -> bool {
    match_rank(needle, haystack).is_some()
}

/// How well `needle` matches `haystack`, as a key that sorts best-first;
/// `None` when it doesn't match at all (the same test as [`fuzzy_match`]).
///
/// The key is `(tier, span, start)`: an exact name, then a prefix, then a
/// run starting at a word boundary, then a run anywhere, then a scattered
/// subsequence — so "quit" lists Quit above "Request: duplicate". Within a
/// tier, a tighter match (fewer chars between first and last hit) beats a
/// looser one, and an earlier one beats a later one. Rows that tie on all
/// three are left in the order they came, which is what lets frecency
/// break ties.
///
/// The scattered span is the tightest window a leftmost-end/rightmost-
/// start pass finds — not always the global minimum, but stable and
/// cheap, and the tiers above catch every case where it would matter.
pub fn match_rank(needle: &str, haystack: &str) -> Option<(u8, usize, usize)> {
    let needle: Vec<char> = needle.to_lowercase().chars().collect();
    let hay: Vec<char> = haystack.to_lowercase().chars().collect();
    if needle.is_empty() || needle == hay {
        return Some((0, 0, 0));
    }
    // A contiguous run: the earliest one, and whether it starts a word.
    let run_start = (0..hay.len().saturating_sub(needle.len() - 1))
        .find(|&s| hay[s..s + needle.len()] == needle[..]);
    if let Some(s) = run_start {
        let tier = if s == 0 {
            1
        } else if !hay[s - 1].is_alphanumeric() {
            2
        } else {
            3
        };
        return Some((tier, needle.len(), s));
    }
    // Scattered: leftmost end, then the latest start that still reaches it.
    let mut i = 0;
    let mut end = None;
    for (pos, &h) in hay.iter().enumerate() {
        if h == needle[i] {
            i += 1;
            if i == needle.len() {
                end = Some(pos);
                break;
            }
        }
    }
    let end = end?;
    let mut j = needle.len();
    let mut start = end;
    for pos in (0..=end).rev() {
        if hay[pos] == needle[j - 1] {
            j -= 1;
            start = pos;
            if j == 0 {
                break;
            }
        }
    }
    Some((4, end - start + 1, start))
}

pub struct PaletteState {
    input: LineInput,
    selected: usize,
    /// `all_commands()` sorted by frecency score descending (stable, so
    /// zero-score commands keep declaration order) as of the moment the
    /// palette opened. `refilter` filters *this* order rather than
    /// re-deriving it, so an empty query shows frecency order and a typed
    /// query ranks its matches with this order breaking ties (spec §6).
    base: Vec<Command>,
    filtered: Vec<Command>,
    /// First visible row's index into `filtered`. See `ChooserState` for the
    /// `ensure_visible` contract this mirrors.
    scroll: usize,
    ensure_visible: bool,
    /// `Some` when vim's `:` opened this palette (`Action::OpenExPalette`):
    /// the scope its verbs resolve in (spec §5.9). `None` (`ctrl+p`) never
    /// pins a row, so a query of `q` there is only a fuzzy query.
    ex: Option<crate::keyroute::ex::Scope>,
    /// The verb the query resolves to, shown as row 0 above `filtered`.
    /// Row indices everywhere (`selected`, `select`, `Hit::PaletteRow`)
    /// count it first: `filtered[i]` is row `i + self.offset()`.
    pinned: Option<crate::keyroute::ex::ExRow>,
    /// A query that runs nothing (`Not supported: :s`): the one muted
    /// row's text; `filtered` is empty while it is `Some`.
    refused: Option<String>,
    /// Whether a row was picked (moved to, or clicked) since the query last
    /// changed: on the ex line, Enter with nothing typed runs only a
    /// picked row, as Vim's `:<CR>` runs nothing.
    picked: bool,
    /// Where the palette opened, for the key column's spelling (task #132):
    /// taken as it opens, since once it is up `App::surface` is the
    /// palette's own `Picker`, where no row is looked up. What it holds
    /// cannot change while the palette is up: keys go to the palette (so
    /// no field or focus moves), its own Reload row closes it before the
    /// reload runs, and the one part of a surface that can change under it
    /// (the response's view mode, as a response lands) decides no palette
    /// row's key.
    spell: crate::keyroute::SpellCtx,
    /// What a key press's binding takes where the palette opened
    /// (`App::key_context`, spec S6a), for the combo check. It is not
    /// derived from `spell`: `App::key_context` reads the focus and screen
    /// under any modal, `spell` the top modal's surface, and `:` opens over
    /// a prompt. Over the sidebar's rename prompt the spelling is the
    /// prompt's, while the context stays the sidebar's, and there `alt+w`
    /// does not move the split, so the split row must not name it
    /// (`the_ex_palette_over_a_sidebar_prompt_keeps_the_sidebar_key_context`).
    key_context: crate::keys::KeyContext,
    /// The `:` line's history as it stood when the line opened, oldest
    /// first (plan M5b Task 5). Empty for ctrl+p's palette.
    history: Vec<String>,
    /// The line ↑ or ↓ last recalled; `None` below the newest, the typed
    /// line.
    hist_at: Option<usize>,
    /// What was on the line at the first ↑ or ↓ since it was last typed in
    /// (Vim's `lookfor`): recall looks for lines that start with it, and ↓
    /// past the newest puts it back.
    lookfor: Option<String>,
}

impl PaletteState {
    /// Builds the palette's base (frecency-sorted) order from `usage`/
    /// `now_secs` and opens with an empty query, so `filtered()` starts out
    /// equal to `base`.
    pub fn new(usage: &crate::usage::UsageStore, now_secs: i64) -> Self {
        // The tooltip's pills come in only with an open tooltip (`with_tip`).
        let mut base: Vec<Command> = all_commands()
            .into_iter()
            .filter(|c| !matches!(c.action, Action::TipPill(_)))
            .collect();
        base.sort_by(|a, b| {
            usage
                .score(b.id, now_secs)
                .partial_cmp(&usage.score(a.id, now_secs))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        Self {
            input: LineInput::new(""),
            selected: 0,
            filtered: base.clone(),
            base,
            scroll: 0,
            ensure_visible: true,
            ex: None,
            pinned: None,
            refused: None,
            picked: false,
            spell: crate::keyroute::SpellCtx::default(),
            key_context: crate::keys::KeyContext::default(),
            history: Vec::new(),
            hist_at: None,
            lookfor: None,
        }
    }

    /// Records where the palette opened (`App::spell_ctx` and
    /// `App::key_context`, see the fields).
    pub fn opened_at(
        self,
        spell: crate::keyroute::SpellCtx,
        key_context: crate::keys::KeyContext,
    ) -> Self {
        Self { spell, key_context, ..self }
    }

    /// Lists the pills of the variable tooltip open as the palette opened,
    /// `name`'s and only those it `offers`, first (#135). Each row names
    /// the variable, so it acts on it whatever the closed palette uncovers.
    pub fn with_tip(mut self, name: &str, offers: &[crate::action::TipPill]) -> Self {
        let rows = all_commands().into_iter().filter_map(|c| match c.action {
            Action::TipPill(pill) if offers.contains(&pill) => {
                Some(Command { action: Action::TipPillOn { pill, name: name.to_string() }, ..c })
            }
            _ => None,
        });
        self.base.splice(0..0, rows);
        self.filtered = self.base.clone();
        self
    }

    /// The same palette opened by vim's `:` (spec §5.9), reading its query
    /// as a verb in `scope`, with the line's history to recall.
    pub fn new_ex(
        usage: &crate::usage::UsageStore,
        now_secs: i64,
        scope: crate::keyroute::ex::Scope,
        history: &[String],
    ) -> Self {
        Self { ex: Some(scope), history: history.to_vec(), ..Self::new(usage, now_secs) }
    }

    pub fn is_ex(&self) -> bool {
        self.ex.is_some()
    }

    pub fn pinned(&self) -> Option<&crate::keyroute::ex::ExRow> {
        self.pinned.as_ref()
    }

    pub fn refused(&self) -> Option<&str> {
        self.refused.as_deref()
    }

    /// How many rows the pinned row shifts `filtered` down by: 1 or 0.
    fn offset(&self) -> usize {
        usize::from(self.pinned.is_some())
    }

    /// Rows shown: the pinned row, if any, and `filtered`.
    pub fn len(&self) -> usize {
        self.offset() + self.filtered.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn input(&self) -> &str {
        self.input.text()
    }

    /// The query, for the vim engine (plan M3b): it edits it in place.
    pub(crate) fn input_mut(&mut self) -> &mut LineInput {
        &mut self.input
    }

    /// The query changed outside this picker's own keys (the vim engine
    /// typed into it): the rows follow, as after a typed key.
    pub(crate) fn filter_changed(&mut self) {
        self.lookfor = None;
        self.refilter();
    }

    /// Pastes into the fuzzy query (the bracketed-paste/ctrl+v path),
    /// flattened to one line like every single-line surface.
    pub fn paste(&mut self, text: &str) {
        self.lookfor = None;
        self.input.paste(text);
        self.refilter();
    }

    pub fn filtered(&self) -> &[Command] {
        &self.filtered
    }

    pub fn selected(&self) -> usize {
        self.selected
    }

    /// The row Enter runs, which is the one painted chosen. On the ex line
    /// with no verb pinned, none until one is picked (Vim's `:` line
    /// chooses no completion until one is asked for).
    pub fn chosen_row(&self) -> Option<usize> {
        let chosen = self.ex.is_none() || self.picked || self.pinned.is_some();
        (chosen && self.selected < self.len()).then_some(self.selected)
    }

    /// Moves the cursor to filtered row `i` (clamped in range) and asks the
    /// next draw to scroll it into view.
    pub fn select(&mut self, i: usize) {
        if i < self.len() {
            self.selected = i;
            self.ensure_visible = true;
            self.picked = true;
        }
    }

    /// The `ModalResult` an `Enter` (or a confirming click) on the current
    /// selection produces — `None` when nothing is selected.
    pub fn confirm(&self) -> Option<super::modal::ModalResult> {
        use super::modal::ModalResult;
        // A refused query: the line closes and runs nothing (spec §5.9).
        if self.refused.is_some() {
            return Some(ModalResult { actions: vec![], close: true, usage: None });
        }
        // The pinned verb: typed, not picked, so no frecency record.
        if let Some(row) = &self.pinned
            && self.selected == 0
        {
            return Some(ModalResult { actions: row.actions.clone(), close: true, usage: None });
        }
        // On the ex line a fuzzy row runs only when it was picked (moved
        // to, or clicked), as Vim's wildmenu runs only the completion
        // tabbed to: typed text no verb spells is Vim's E492, never
        // whichever row the filter ranked first (`:W`).
        let chosen = self.chosen_row().and_then(|row| self.filtered.get(row - self.offset()));
        match chosen {
            Some(chosen) => Some(ModalResult {
                actions: vec![chosen.action.clone()],
                close: true,
                usage: Some(chosen.id.to_string()),
            }),
            // The ex line with nothing to run: a note (Deviation 16);
            // ctrl+p's palette swallows Enter on an empty list, as on main.
            // The note's text is Vim's E492 without the number.
            None if self.ex.is_some() => Some(ModalResult {
                actions: vec![Action::ShowToast(
                    format!("Not a command: :{}", self.input.text()),
                    super::toast::ToastKind::Info,
                )],
                close: true,
                usage: None,
            }),
            None => None,
        }
    }

    /// Adjusts `scroll` by `delta` lines, clamped, without moving
    /// `selected`. A no-op on an empty list.
    pub fn scroll_by(&mut self, delta: i16) {
        if self.is_empty() {
            return;
        }
        let max = self.len().saturating_sub(1);
        self.scroll = (self.scroll as i32 + delta as i32).clamp(0, max as i32) as usize;
        self.ensure_visible = false;
    }

    /// Re-runs the fuzzy filter and ranks the survivors by match quality
    /// ([`match_rank`]). The ex line first resolves the query as a verb.
    fn refilter(&mut self) {
        use crate::keyroute::ex;
        let query = self.input.text();
        self.pinned = None;
        self.refused = None;
        self.picked = false;
        if let Some(scope) = &self.ex {
            match ex::resolve(query, scope) {
                ex::Resolved::Pinned(row) => self.pinned = Some(row),
                ex::Resolved::Refused(text) => {
                    self.refused = Some(text);
                    self.filtered.clear();
                    self.selected = 0;
                    self.scroll = 0;
                    self.ensure_visible = true;
                    return;
                }
                ex::Resolved::Nothing => {}
            }
        }
        let mut ranked: Vec<((u8, usize, usize), &Command)> = self
            .base
            .iter()
            .filter_map(|c| match_rank(query, c.name).map(|rank| (rank, c)))
            .collect();
        // Stable, so rows that match equally well keep `base`'s frecency
        // order — frecency only breaks ties.
        ranked.sort_by_key(|(rank, _)| *rank);
        self.filtered = ranked.into_iter().map(|(_, c)| c.clone()).collect();
        self.selected = 0;
        self.scroll = 0;
        self.ensure_visible = true;
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Option<super::modal::ModalResult> {
        match key.code {
            // The ex line closes on Backspace with nothing typed (spec
            // §5.9, Deviation 15); `App::box_claims` hands the key here
            // ahead of the engine. ctrl+p's palette keeps it inert.
            KeyCode::Backspace if self.ex.is_some() && self.input.text().is_empty() => {
                return Some(super::modal::ModalResult {
                    actions: vec![],
                    close: true,
                    usage: None,
                });
            }
            KeyCode::Esc => {
                return Some(super::modal::ModalResult {
                    actions: vec![],
                    close: true,
                    ..Default::default()
                });
            }
            // Vim's `:<CR>`: nothing typed and nothing picked runs nothing,
            // though the frecency list shows under the line.
            KeyCode::Enter
                if self.ex.is_some() && self.input.text().trim().is_empty() && !self.picked =>
            {
                return Some(super::modal::ModalResult {
                    actions: vec![],
                    close: true,
                    usage: None,
                });
            }
            KeyCode::Enter => return self.confirm(),
            // Vim's ↑ and ↓ on the `:` line recall its history (plan M5b
            // Deviation 13); ctrl+p and ctrl+n go on moving the list.
            KeyCode::Up if self.ex.is_some() => self.recall(true),
            KeyCode::Down if self.ex.is_some() => self.recall(false),
            KeyCode::Up => {
                self.selected = self.selected.saturating_sub(1);
                self.ensure_visible = true;
                self.picked = true;
            }
            // The list is always open, as Vim's wildmenu is only after Tab
            // (Deviation 14). With no row chosen, the first ctrl+p or
            // ctrl+n picks the last row or the first, as Vim's open
            // wildmenu does from the typed text (`Tab ^P ^N` is the first
            // match, `Tab ^P ^P` the last).
            KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.selected = if self.chosen_row().is_none() {
                    self.len().saturating_sub(1)
                } else {
                    self.selected.saturating_sub(1)
                };
                self.ensure_visible = true;
                self.picked = true;
            }
            KeyCode::Down => {
                if self.selected + 1 < self.len() {
                    self.selected += 1;
                }
                self.ensure_visible = true;
                self.picked = true;
            }
            KeyCode::Char('n') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if self.chosen_row().is_none() {
                    self.selected = 0;
                } else if self.selected + 1 < self.len() {
                    self.selected += 1;
                }
                self.ensure_visible = true;
                self.picked = true;
            }
            _ => {
                let before = self.input.text().to_string();
                self.input.handle_key(key);
                if self.input.text() != before {
                    self.lookfor = None;
                    self.refilter();
                }
            }
        }
        None
    }

    /// Vim's ↑ (`older`) and ↓ on the `:` line (plan M5b Deviation 13;
    /// probes H1–H21): the next line that way that starts with what was on
    /// the line at the first ↑ or ↓ since it was last typed in. ↓ past the
    /// newest puts that text back; ↑ past the oldest match changes nothing.
    fn recall(&mut self, older: bool) {
        let prefix = self.lookfor.get_or_insert_with(|| self.input.text().to_string()).clone();
        let mut at = self.hist_at.unwrap_or(self.history.len());
        let found = loop {
            if older {
                let Some(prev) = at.checked_sub(1) else { return };
                at = prev;
            } else {
                at += 1;
                if at >= self.history.len() {
                    break None;
                }
            }
            if self.history[at].starts_with(&prefix) {
                break Some(at);
            }
        };
        self.hist_at = found;
        let text = found.map_or(prefix, |i| self.history[i].clone());
        if text != self.input.text() {
            self.input.set_text(&text);
            self.refilter();
        }
    }

    /// Whether the filter box has keystrokes of its own to step — what
    /// the app's undo routing asks before handing it the key.
    pub(crate) fn filter_edited(&self) -> bool {
        self.input.edited()
    }

    /// Steps the filter's edit history one place (back, or forward when
    /// `redo`) and re-runs the filter, mirroring what typing that step
    /// forward did. Returns whether there was a step to take.
    pub(crate) fn undo_filter(&mut self, redo: bool) -> bool {
        let stepped = if redo { self.input.redo() } else { self.input.undo() };
        if stepped {
            self.refilter();
        }
        stepped
    }

    /// The key `c`'s row shows: what [`crate::keyroute::spell`] names for
    /// its action where the palette opened, as for a footer chip's
    /// `Key::Of` (spec E12a, amended for task #132): the router's table
    /// row, then the keymap's combo, then the ex verb where `:` opens. The
    /// row's own action is looked up (task #105), so no row can miss its
    /// key, and a row whose action no key sends shows none: "Manage:
    /// variables" (`alt+r` opens the tab Manage last had), and the sidebar
    /// width rows (only `alt+w` remapped on the sidebar sends theirs, S6b).
    /// On Manage an ex verb runs what its row runs (`:split` steps the
    /// split there): the screen's whitelist gates keymap combos, not the
    /// `:` line.
    ///
    /// The footer's extras are not copied: a chip that runs the row's
    /// action through another (the sidebar's `a new` is the list verb
    /// `Add`) is not matched to it, `Key::Send`'s `⇧enter` and `Key::Quit`'s
    /// `:q` are not used (Quit shows the `^C` of the quit its row runs),
    /// and arrow names stay `alt+left`, not glyphs.
    ///
    /// The combo is skipped where a key press would remap it (spec S6a,
    /// `KeyContext::action` on `key_context`); a table row or an ex verb is
    /// never remapped, so from the vim sidebar the split rows show
    /// `:split`. That check lives here, not in `SpellCtx`: the palette is
    /// the only place that spells a split key (S6 dropped the footer's
    /// split chip; E12a's split-chip hints name the hide/show toggles).
    ///
    /// It runs per visible row per frame, as the footer's chips spell per
    /// frame: a few scans of the binding table each, too few to cache.
    pub(crate) fn key_column(&self, c: &Command, keymap: &crate::keys::Keymap) -> Option<String> {
        let action = key_action(&c.action);
        crate::keyroute::spell::spell_with(self.spell, &action, || {
            keymap
                .combo_for_action(&action)
                .filter(|_| self.key_context.action(action.clone().into_owned()) == *action)
        })
    }

    #[cfg(test)]
    fn filtered_is_empty_for_test(&self) -> bool {
        self.filtered.is_empty()
    }

    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        frame: &mut Frame,
        screen: Rect,
        theme: &Theme,
        hits: &mut crate::hit::HitMap,
        hovered: Option<&crate::hit::Hit>,
        keymap: &crate::keys::Keymap,
        t: f32,
    ) {
        let width = 50.min(screen.width);
        const CHROME: u16 = 10;
        let content_rows = (self.len() as u16).clamp(1, 10) * 2;
        let height = (CHROME + content_rows).clamp(13, 26).min(screen.height);
        let area = super::modal::centered_rect(screen, width, height);
        hits.register(area, crate::hit::Hit::ModalBody);
        paint::floating_panel_settling(frame.buffer_mut(), area, screen, theme, t);
        if t < 1.0 {
            return;
        }

        let title_y = area.y + 1;
        paint::text(
            frame.buffer_mut(),
            area.x + 2,
            title_y,
            "Commands",
            theme.text,
            theme.panel,
            true,
        );

        let field_area = Rect {
            x: area.x + 1,
            y: title_y + 2,
            width: area.width.saturating_sub(2),
            height: FIELD_HEIGHT,
        };
        let content =
            self.input.draw_line_windowed(true, theme, field_area.width.saturating_sub(2));
        TextField { content, state: ControlState::Focused }.paint(
            frame.buffer_mut(),
            field_area,
            theme,
        );

        let list_area = Rect {
            x: area.x + 1,
            y: field_area.y + FIELD_HEIGHT + 2,
            width: area.width.saturating_sub(2),
            height: area.height.saturating_sub(CHROME),
        };
        let list_h = (list_area.height / 2) as usize;
        if self.ensure_visible {
            if list_h > 0 {
                if self.selected < self.scroll {
                    self.scroll = self.selected;
                } else if self.selected >= self.scroll + list_h {
                    self.scroll = self.selected + 1 - list_h;
                }
                let max_scroll = self.len().saturating_sub(list_h);
                self.scroll = self.scroll.min(max_scroll);
            }
            self.ensure_visible = false;
        }

        // No hover-fade animation is wired for popup lists (transient
        // surfaces — see the task report); a hovered row shows its full
        // hover fill immediately, same convention as `DrawCtx::hover_t`'s
        // own documented default when no fade is in flight.
        let hover_t = 1.0;
        if let Some(text) = &self.refused {
            paint::text(
                frame.buffer_mut(),
                list_area.x + 1,
                list_area.y,
                clip(text, list_area.width.saturating_sub(2)),
                theme.text_muted,
                theme.panel,
                false,
            );
            return;
        }
        let rows = self.len();
        for i in self.scroll..rows.min(self.scroll + list_h.max(1)) {
            let (name, description, combo): (&str, &str, Option<String>) = match (i, &self.pinned) {
                (0, Some(row)) => (row.caption, "Ex command", Some(format!(":{}", row.name))),
                _ => {
                    let c = &self.filtered[i - self.offset()];
                    (c.name, c.description, self.key_column(c, keymap))
                }
            };
            let title_row = list_area.y + ((i - self.scroll) as u16) * 2;
            let desc_row = title_row + 1;
            let selected = self.chosen_row() == Some(i);
            let row_hovered = hovered == Some(&crate::hit::Hit::PaletteRow(i));
            let highlight = if selected {
                RowHighlight::Selected
            } else if row_hovered {
                RowHighlight::Hover
            } else {
                RowHighlight::None
            };
            for row_y in [title_row, desc_row] {
                ListRow { highlight, zebra: None }.paint(
                    frame.buffer_mut(),
                    row_y,
                    list_area.x,
                    list_area.width,
                    theme.panel,
                    hover_t,
                    theme,
                );
            }
            let row_fill = ListRow::resolve_fill(theme, highlight, theme.panel, hover_t);

            let right = list_area.x + list_area.width;
            let x = list_area.x + 1;
            let combo_w = combo.as_ref().map(|s| crate::ui::cells(s) as u16 + 1).unwrap_or(0);
            let title_right = right.saturating_sub(combo_w);
            paint::text(
                frame.buffer_mut(),
                x,
                title_row,
                clip(name, title_right.saturating_sub(x)),
                theme.text,
                row_fill,
                true,
            );
            if let Some(combo) = &combo {
                let combo_x = right.saturating_sub(crate::ui::cells(combo) as u16);
                paint::text(
                    frame.buffer_mut(),
                    combo_x,
                    title_row,
                    combo,
                    theme.text_muted,
                    row_fill,
                    false,
                );
            }
            paint::text(
                frame.buffer_mut(),
                x,
                desc_row,
                clip(description, right.saturating_sub(x)),
                theme.text_muted,
                row_fill,
                false,
            );

            let row_rect = Rect { x: list_area.x, y: title_row, width: list_area.width, height: 2 };
            hits.register(row_rect, crate::hit::Hit::PaletteRow(i));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::KeyModifiers;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    #[test]
    fn ctrl_n_and_ctrl_p_are_down_and_up() {
        let mut a = PaletteState::new(&crate::usage::UsageStore::default(), 0);
        let mut b = PaletteState::new(&crate::usage::UsageStore::default(), 0);
        a.handle_key(key(KeyCode::Down));
        b.handle_key(ctrl('n'));
        assert_eq!(a.selected(), b.selected());
        a.handle_key(key(KeyCode::Up));
        b.handle_key(ctrl('p'));
        assert_eq!(a.selected(), b.selected());
        assert_eq!(b.input(), "", "ctrl+n/p never type");
    }

    /// Puts the cursor on command `id`. A query is a subsequence match, so
    /// more than one command can survive it (e.g. "quit" also matches
    /// "Request: duplicate") — a test that means one command names it.
    fn select_id(p: &mut PaletteState, id: &str) {
        let i = p
            .filtered()
            .iter()
            .position(|c| c.id == id)
            .unwrap_or_else(|| panic!("{id} was filtered out"));
        p.select(i);
    }

    /// The user-triggered reload is reachable from the palette, and its
    /// row shows the same `alt+shift+r` the key path uses.
    #[test]
    fn palette_carries_reload_from_disk_with_its_binding() {
        let cmd = all_commands()
            .into_iter()
            .find(|c| c.id == "reload-from-disk")
            .expect("reload-from-disk command");
        assert_eq!(cmd.action, Action::ReloadFromDisk);
        assert_eq!(cmd.name, "Reload from disk");
        let keymap = crate::keys::Keymap::default_bindings();
        let p = opened_in(crate::config::KeyProfile::Arrows);
        let alt = crate::keys::alt_label();
        assert_eq!(p.key_column(&cmd, &keymap), Some(format!("{alt}+shift+r")));
    }

    /// The response's open-in-$EDITOR action is reachable from the palette,
    /// like the request body's is.
    #[test]
    fn palette_carries_response_open_in_editor() {
        let cmd = all_commands()
            .into_iter()
            .find(|c| c.id == "response-open-editor")
            .expect("response-open-editor command");
        assert_eq!(cmd.action, Action::OpenResponseInEditor);
    }

    /// Spec S6b: the sidebar's width steps are in the palette, so a
    /// keyboard user can find them. They send the sidebar-only step, and
    /// show no key: `alt+w` steps the sidebar only while it has focus.
    #[test]
    fn palette_carries_the_sidebar_width_steps() {
        let step = crate::layout::SIDEBAR_STEP_COLS;
        let keymap = crate::keys::Keymap::default_bindings();
        let p = opened_in(crate::config::KeyProfile::Arrows);
        for (id, name, action) in [
            ("sidebar-widen", "Widen sidebar", Action::StepSidebar(step)),
            ("sidebar-narrow", "Narrow sidebar", Action::StepSidebar(-step)),
        ] {
            let cmd = all_commands().into_iter().find(|c| c.id == id).expect(id);
            assert_eq!(p.key_column(&cmd, &keymap), None, "{id}: no key column");
            assert_eq!((cmd.name, cmd.action), (name, action), "{id}");
        }
    }

    /// Spec E12a: the split's keyboard steps are in the palette, whose key
    /// column shows the keymap's split keys, so `alt+w` can be found with
    /// the chip hints naming the hide/show toggles.
    #[test]
    fn palette_carries_the_split_steps_with_their_keys() {
        let alt = crate::keys::alt_label();
        let keymap = crate::keys::Keymap::default_bindings();
        let p = opened_in(crate::config::KeyProfile::Arrows);
        for (id, name, action, key) in [
            ("split-step", "Step the split", Action::CycleSplit, format!("{alt}+w")),
            (
                "split-step-back",
                "Step the split back",
                Action::CycleSplitBack,
                format!("{alt}+shift+w"),
            ),
        ] {
            let cmd = all_commands().into_iter().find(|c| c.id == id).expect(id);
            assert_eq!(p.key_column(&cmd, &keymap), Some(key), "{id}");
            assert_eq!((cmd.name, cmd.action), (name, action), "{id}");
        }
    }

    /// A palette opened at `at` as the app opens one with no modal up: a
    /// key press's context is the sidebar's on `Surface::Sidebar`.
    fn open_at(at: crate::keyroute::SpellCtx) -> PaletteState {
        let sidebar_on_main = at.surface == crate::keyroute::Surface::Sidebar;
        PaletteState::new(&crate::usage::UsageStore::default(), 0)
            .opened_at(at, crate::keys::KeyContext { sidebar_on_main })
    }

    /// A palette opened in `profile` with no surface of its own.
    fn opened_in(profile: crate::config::KeyProfile) -> PaletteState {
        open_at(crate::keyroute::SpellCtx { profile, ..Default::default() })
    }

    /// Task #91: the hide/show panel rows show the key that does the same
    /// thing, `alt+p` and `alt+shift+p`, in both profiles (vim keeps both).
    #[test]
    fn the_hide_show_panel_rows_show_their_keys() {
        let alt = crate::keys::alt_label();
        let commands = all_commands();
        for profile in [crate::config::KeyProfile::Arrows, crate::config::KeyProfile::Vim] {
            let keymap = crate::keys::Keymap::default_bindings_for(profile);
            let p = opened_in(profile);
            for (id, key) in [
                ("editor-toggle", format!("{alt}+p")),
                ("response-toggle", format!("{alt}+shift+p")),
            ] {
                let cmd = commands.iter().find(|c| c.id == id).expect(id);
                assert_eq!(p.key_column(cmd, &keymap), Some(key), "{id} in {profile:?}");
            }
        }
    }

    /// Task #105: a row shows the key bound to its own action, so the
    /// eight rows a hand-kept name table missed show theirs, in both
    /// profiles; Quit keeps `^C` through its one exception; and "Manage:
    /// variables" shows none: `alt+r` opens the tab Manage last had, not
    /// Variables.
    #[test]
    fn rows_show_the_key_bound_to_their_action() {
        let alt = crate::keys::alt_label();
        let commands = all_commands();
        for profile in [crate::config::KeyProfile::Arrows, crate::config::KeyProfile::Vim] {
            let keymap = crate::keys::Keymap::default_bindings_for(profile);
            let p = opened_in(profile);
            let key = |id: &str| {
                let cmd = commands.iter().find(|c| c.id == id).expect(id);
                p.key_column(cmd, &keymap)
            };
            for (id, want) in [
                ("body-clear", format!("{alt}+k")),
                ("tls-toggle-verify", format!("{alt}+i")),
                ("project-prev", format!("{alt}+shift+z")),
                ("env-prev", format!("{alt}+shift+x")),
                ("edit-paste", "^V".to_string()),
                ("request-copy-url", format!("{alt}+y")),
                ("response-jq", format!("{alt}+q")),
                ("response-jq-views", format!("{alt}+shift+j")),
                ("quit", "^C".to_string()),
            ] {
                assert_eq!(key(id), Some(want), "{id} in {profile:?}");
            }
            assert_eq!(key("manage-variables"), None, "{profile:?}");
        }
    }

    /// The key column of row `id` in a palette opened at `at` with the
    /// profile's default keymap.
    fn key_at(at: crate::keyroute::SpellCtx, id: &str) -> Option<String> {
        let keymap = crate::keys::Keymap::default_bindings_for(at.profile);
        let p = open_at(at);
        let cmd = all_commands().into_iter().find(|c| c.id == id).expect(id);
        p.key_column(&cmd, &keymap)
    }

    /// Task #132 (spec E12a, amended): a row names the key the footer
    /// names where the palette opened, the router's table and the Ex verbs
    /// included. In vim, undo is `u`, not the keymap's `^Z`, and save is
    /// `:w`; Quit keeps `^C`, the key of the quit it runs.
    #[test]
    fn vim_rows_name_the_keys_the_footer_names() {
        use crate::keyroute::{SpellCtx, Surface};
        let vim = crate::config::KeyProfile::Vim;
        let surfaces = [
            Surface::Sidebar,
            Surface::UrlLine,
            Surface::Body,
            Surface::Response(crate::components::response::ViewMode::Pretty),
        ];
        for surface in surfaces {
            let at = SpellCtx { profile: vim, surface, ..SpellCtx::default() };
            for (id, want) in [
                ("undo", "u"),
                ("redo", "^R"),
                ("request-save", ":w"),
                ("split-step", ":split"),
                ("split-step-back", ":splitback"),
                ("request-import-curl", ":import"),
                ("request-copy-curl", ":copycurl"),
                ("quit", "^C"),
            ] {
                assert_eq!(key_at(at, id).as_deref(), Some(want), "{id} on {surface:?}");
            }
        }
    }

    /// Task #132: the response's search row is `/` opened from the
    /// response, in both profiles, and has no key elsewhere.
    #[test]
    fn the_search_row_is_slash_from_the_response() {
        use crate::keyroute::{SpellCtx, Surface};
        let response = Surface::Response(crate::components::response::ViewMode::Pretty);
        for profile in [crate::config::KeyProfile::Arrows, crate::config::KeyProfile::Vim] {
            let at = |surface| SpellCtx { profile, surface, ..SpellCtx::default() };
            assert_eq!(
                key_at(at(response), "response-search").as_deref(),
                Some("/"),
                "{profile:?}"
            );
            assert_eq!(key_at(at(Surface::Sidebar), "response-search"), None, "{profile:?}");
        }
    }

    /// Task #132: opened from a field that is typing, every key types, so
    /// only the keymap's combos spell, as in the footer.
    #[test]
    fn rows_opened_from_a_typing_field_name_keymap_combos_only() {
        use crate::keyroute::{FieldSpell, SpellCtx, Surface};
        let at = SpellCtx {
            profile: crate::config::KeyProfile::Vim,
            surface: Surface::UrlLine,
            field: FieldSpell::Typing,
        };
        assert_eq!(key_at(at, "undo").as_deref(), Some("^Z"));
        assert_eq!(key_at(at, "request-save"), None);
    }

    /// Task #132 with S6a: opened from the sidebar, the split rows drop
    /// the keymap's `alt+w` (it widens the sidebar there) but keep an Ex
    /// verb, which always moves the split.
    #[test]
    fn the_sidebar_drops_the_split_combo_but_not_the_ex_verb() {
        use crate::keyroute::{SpellCtx, Surface};
        // The arrows keymap in both profiles: it binds `alt+w`, as a vim
        // keys.toml could.
        let keymap = crate::keys::Keymap::default_bindings();
        let cmd = all_commands().into_iter().find(|c| c.id == "split-step").expect("split-step");
        let alt = crate::keys::alt_label();
        for (profile, surface, want) in [
            (crate::config::KeyProfile::Arrows, Surface::Sidebar, None),
            (crate::config::KeyProfile::Vim, Surface::Sidebar, Some(":split".to_string())),
            (crate::config::KeyProfile::Vim, Surface::UrlLine, Some(format!("{alt}+w"))),
        ] {
            let at = SpellCtx { profile, surface, ..SpellCtx::default() };
            assert_eq!(open_at(at).key_column(&cmd, &keymap), want, "{profile:?} on {surface:?}");
        }
    }

    #[test]
    fn fuzzy_match_is_case_insensitive_subsequence() {
        assert!(fuzzy_match("fre", "Focus: request editor"));
        assert!(fuzzy_match("QUIT", "Quit"));
        assert!(fuzzy_match("", "anything"));
        assert!(!fuzzy_match("xyz", "Quit"));
    }

    #[test]
    fn the_filter_has_a_caret_and_undo() {
        let mut p = PaletteState::new(&crate::usage::UsageStore::default(), 0);
        for c in "send".chars() {
            p.handle_key(key(KeyCode::Char(c)));
        }
        assert_eq!(p.input(), "send");
        p.handle_key(key(KeyCode::Left));
        p.handle_key(key(KeyCode::Char('X')));
        assert_eq!(p.input(), "senXd", "the caret moved before the last char");
        p.undo_filter(false);
        assert_eq!(p.input(), "send");
        assert!(!p.filtered_is_empty_for_test() || p.input().is_empty());
    }

    #[test]
    fn typing_filters_and_backspace_restores() {
        let mut p = PaletteState::new(&crate::usage::UsageStore::default(), 0);
        let total = p.filtered().len();
        for c in "quit".chars() {
            p.handle_key(key(KeyCode::Char(c)));
        }
        assert!(p.filtered().len() < total, "the query narrowed the list");
        assert!(p.filtered().iter().any(|c| c.name == "Quit"));
        assert!(
            p.filtered().iter().all(|c| fuzzy_match("quit", c.name)),
            "every surviving row matches the query"
        );
        p.handle_key(key(KeyCode::Backspace));
        p.handle_key(key(KeyCode::Backspace));
        p.handle_key(key(KeyCode::Backspace));
        p.handle_key(key(KeyCode::Backspace));
        assert_eq!(p.filtered().len(), total);
    }

    #[test]
    fn arrows_move_selection_within_bounds() {
        let mut p = PaletteState::new(&crate::usage::UsageStore::default(), 0);
        assert_eq!(p.selected(), 0);
        p.handle_key(key(KeyCode::Up)); // clamped at top
        assert_eq!(p.selected(), 0);
        p.handle_key(key(KeyCode::Down));
        assert_eq!(p.selected(), 1);
    }

    #[test]
    fn enter_returns_selected_action_and_closes() {
        let mut p = PaletteState::new(&crate::usage::UsageStore::default(), 0);
        for c in "quit".chars() {
            p.handle_key(key(KeyCode::Char(c)));
        }
        select_id(&mut p, "quit");
        let res = p.handle_key(key(KeyCode::Enter)).unwrap();
        assert!(res.close);
        assert_eq!(res.actions, vec![Action::GatedQuit]);
        assert_eq!(res.usage.as_deref(), Some("quit"));
    }

    #[test]
    fn heavily_used_command_sorts_to_top_with_empty_query() {
        let mut usage = crate::usage::UsageStore::default();
        // "quit" is declared last but heavy usage should put it first when
        // the query is empty.
        for _ in 0..50 {
            usage.record("quit", 1_000_000);
        }
        let p = PaletteState::new(&usage, 1_000_000);
        assert_eq!(p.filtered()[0].id, "quit");
    }

    #[test]
    fn typing_still_fuzzy_filters_within_frecency_order() {
        let mut usage = crate::usage::UsageStore::default();
        for _ in 0..50 {
            usage.record("quit", 1_000_000);
        }
        let mut p = PaletteState::new(&usage, 1_000_000);
        for c in "focus".chars() {
            p.handle_key(key(KeyCode::Char(c)));
        }
        assert!(
            p.filtered().iter().all(|c| c.id.starts_with("focus-")),
            "typed query must still fuzzy-filter, frecency only breaks ties"
        );
        assert_eq!(p.filtered().len(), 3);
    }

    #[test]
    fn zero_score_commands_keep_declaration_order() {
        let p = PaletteState::new(&crate::usage::UsageStore::default(), 0);
        let ids: Vec<&str> = p.filtered().iter().map(|c| c.id).collect();
        // The tooltip's pills wait for an open tooltip (`with_tip`).
        let declared: Vec<&str> = all_commands()
            .iter()
            .filter(|c| !matches!(c.action, Action::TipPill(_)))
            .map(|c| c.id)
            .collect();
        assert_eq!(ids, declared, "stable sort must preserve order among ties");
    }

    /// Task 17, spec §5: palette audit — every command the sweep added
    /// (Response: search, Variables: new variable/selector) is present with a
    /// stable kebab-case id, alongside the ids that already covered Body:
    /// format/minify/toggle-vars/open-in-$EDITOR and Request: duplicate.
    #[test]
    fn palette_covers_the_mouse_parity_sweep_gaps() {
        let commands = all_commands();
        let ids: Vec<&str> = commands.iter().map(|c| c.id).collect();
        for expected in [
            "body-format",
            "body-minify",
            "body-toggle-vars",
            "body-external-editor",
            "request-duplicate",
            "response-search",
            "vars-new-variable",
            "vars-new-selector",
        ] {
            assert!(ids.contains(&expected), "missing palette command {expected:?}");
        }
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), ids.len(), "every command id must be unique");

        assert_eq!(
            commands.iter().find(|c| c.id == "response-search").unwrap().action,
            Action::OpenResponseSearch { backward: false }
        );
        assert_eq!(
            commands.iter().find(|c| c.id == "vars-new-variable").unwrap().action,
            Action::PromptNewVar
        );
        assert_eq!(
            commands.iter().find(|c| c.id == "vars-new-selector").unwrap().action,
            Action::PromptNewSelector
        );
    }

    #[test]
    fn enter_on_empty_results_does_nothing() {
        let mut p = PaletteState::new(&crate::usage::UsageStore::default(), 0);
        for c in "zzzz".chars() {
            p.handle_key(key(KeyCode::Char(c)));
        }
        assert!(p.filtered().is_empty());
        assert!(p.handle_key(key(KeyCode::Enter)).is_none());
    }

    #[test]
    fn esc_closes_without_action() {
        let mut p = PaletteState::new(&crate::usage::UsageStore::default(), 0);
        let res = p.handle_key(key(KeyCode::Esc)).unwrap();
        assert!(res.close);
        assert!(res.actions.is_empty());
    }

    #[test]
    fn selection_resets_when_filter_changes() {
        let mut p = PaletteState::new(&crate::usage::UsageStore::default(), 0);
        p.handle_key(key(KeyCode::Down));
        p.handle_key(key(KeyCode::Char('q')));
        assert_eq!(p.selected(), 0);
    }

    #[test]
    fn no_key_hint_footer_row() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut p = PaletteState::new(&crate::usage::UsageStore::default(), 0);
        let theme = Theme::dark();
        let keymap = crate::keys::Keymap::default_bindings();
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal.draw(|f| p.draw(f, f.area(), &theme, &mut hits, None, &keymap, 1.0)).unwrap();
        let content = format!("{:?}", terminal.backend().buffer());
        assert!(!content.contains("enter run"), "{content}");
        assert!(!content.contains("esc cancel"), "{content}");
    }

    #[test]
    fn field_fill_and_gap_row_survive_the_list_draw() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut p = PaletteState::new(&crate::usage::UsageStore::default(), 0);
        let theme = Theme::dark();
        let keymap = crate::keys::Keymap::default_bindings();
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal.draw(|f| p.draw(f, f.area(), &theme, &mut hits, None, &keymap, 1.0)).unwrap();
        let area = hits.rect_of(&crate::hit::Hit::ModalBody).unwrap();
        let title_y = area.y + 1;
        let field_area = Rect {
            x: area.x + 1,
            y: title_y + 2,
            width: area.width.saturating_sub(2),
            height: FIELD_HEIGHT,
        };
        let buffer = terminal.backend().buffer();
        // The focused field's lifted fill reaches its bottom bevel row...
        let lifted = crate::theme::lift_color(theme.control, 0.12);
        let bevel = buffer[(field_area.x, field_area.y + FIELD_HEIGHT - 1)].clone();
        assert_eq!(bevel.bg, lifted, "field bottom row keeps the lifted fill");
        // ...and row 0's top pad must not creep into the gap row below it.
        let gap = buffer[(field_area.x, field_area.y + FIELD_HEIGHT)].clone();
        assert_eq!(gap.bg, theme.panel, "gap row below the field stays panel");
        assert_eq!(gap.symbol(), " ");
    }

    #[test]
    fn hovered_row_background_fills_the_full_row_width() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut p = PaletteState::new(&crate::usage::UsageStore::default(), 0);
        let theme = Theme::dark();
        let keymap = crate::keys::Keymap::default_bindings();
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal
            .draw(|f| {
                // Row 1 (not the default-selected row 0) so this exercises
                // the plain Hover fill rather than Selected.
                p.draw(
                    f,
                    f.area(),
                    &theme,
                    &mut hits,
                    Some(&crate::hit::Hit::PaletteRow(1)),
                    &keymap,
                    1.0,
                )
            })
            .unwrap();
        let row1 = hits.rect_of(&crate::hit::Hit::PaletteRow(1)).unwrap();
        let buffer = terminal.backend().buffer();
        // The label ("Focus: editor") is well short of the row's right
        // edge, so a cell out there only picks up the pill's background if
        // the fill spans the whole row, not just the label glyphs.
        let right_edge = (row1.x + row1.width - 1, row1.y);
        assert_eq!(
            buffer[right_edge].bg, theme.control,
            "the pill fill must span the full row width, not just the label glyphs"
        );
    }

    #[test]
    fn entries_sit_on_a_dense_two_line_pitch_with_zero_gap() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut p = PaletteState::new(&crate::usage::UsageStore::default(), 0);
        let theme = Theme::dark();
        let keymap = crate::keys::Keymap::default_bindings();
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal.draw(|f| p.draw(f, f.area(), &theme, &mut hits, None, &keymap, 1.0)).unwrap();
        let row0 = hits.rect_of(&crate::hit::Hit::PaletteRow(0)).unwrap();
        let row1 = hits.rect_of(&crate::hit::Hit::PaletteRow(1)).unwrap();
        assert_eq!(row0.height, 2, "each option's hit box covers both its lines");
        assert_eq!(
            row1.y - row0.y,
            2,
            "consecutive options sit back-to-back with no gap row between them"
        );
    }

    #[test]
    fn selected_entry_paints_the_accent_bar_and_selection_fill_on_both_lines() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut p = PaletteState::new(&crate::usage::UsageStore::default(), 0);
        let theme = Theme::dark();
        let keymap = crate::keys::Keymap::default_bindings();
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal.draw(|f| p.draw(f, f.area(), &theme, &mut hits, None, &keymap, 1.0)).unwrap();
        let row0 = hits.rect_of(&crate::hit::Hit::PaletteRow(0)).unwrap();
        let buffer = terminal.backend().buffer();
        for dy in [0u16, 1] {
            let cell = &buffer[(row0.x, row0.y + dy)];
            assert_eq!(cell.symbol(), "\u{258c}", "accent bar on line {dy}");
            assert_eq!(cell.fg, theme.accent);
            assert_eq!(
                buffer[(row0.x + row0.width - 1, row0.y + dy)].bg,
                theme.selection,
                "selection fill spans the full row width on line {dy}"
            );
        }
    }

    #[test]
    fn title_line_shows_bold_name_and_description_line_shows_muted_text() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut p = PaletteState::new(&crate::usage::UsageStore::default(), 0);
        let theme = Theme::dark();
        let keymap = crate::keys::Keymap::default_bindings();
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal.draw(|f| p.draw(f, f.area(), &theme, &mut hits, None, &keymap, 1.0)).unwrap();
        let content = format!("{:?}", terminal.backend().buffer());
        let first = p.filtered()[0].clone();
        assert!(content.contains(first.name), "{content}");
        assert!(content.contains(first.description), "{content}");
    }

    #[test]
    fn keybinding_column_shows_the_bound_combo_right_aligned() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut p = PaletteState::new(&crate::usage::UsageStore::default(), 0);
        for c in "quit".chars() {
            p.handle_key(key(KeyCode::Char(c)));
        }
        select_id(&mut p, "quit");
        let theme = Theme::dark();
        let keymap = crate::keys::Keymap::default_bindings();
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal.draw(|f| p.draw(f, f.area(), &theme, &mut hits, None, &keymap, 1.0)).unwrap();
        let content = format!("{:?}", terminal.backend().buffer());
        assert!(
            content.contains("^C"),
            "the quit row's bound combo must render in caret notation: {content}"
        );
    }

    fn ex_scope() -> crate::keyroute::ex::Scope {
        crate::keyroute::ex::Scope {
            surface: crate::keyroute::Surface::Sidebar,
            verbs: vec![crate::action::Verb::Add],
        }
    }

    fn ex() -> PaletteState {
        PaletteState::new_ex(&crate::usage::UsageStore::default(), 0, ex_scope(), &[])
    }

    /// Plan M5b Deviation 13: ↑ and ↓ on the `:` line recall its history as
    /// Vim's do (probes H1–H21), from `echo 1`, `let g:a=1`, `echo 22`.
    #[test]
    fn the_ex_line_recalls_its_history_as_vim_does() {
        let history = ["echo 1".to_string(), "let g:a=1".to_string(), "echo 22".to_string()];
        let run = |keys: &[KeyEvent]| {
            let mut p =
                PaletteState::new_ex(&crate::usage::UsageStore::default(), 0, ex_scope(), &history);
            for k in keys {
                p.handle_key(*k);
            }
            p.input().to_string()
        };
        let up = key(KeyCode::Up);
        let down = key(KeyCode::Down);
        let bs = key(KeyCode::Backspace);
        let c = |ch: char| key(KeyCode::Char(ch));
        assert_eq!(run(&[up]), "echo 22", "H1");
        assert_eq!(run(&[up, up]), "let g:a=1", "H2");
        assert_eq!(run(&[c('e'), up]), "echo 22", "H3a");
        assert_eq!(run(&[c('e'), up, up]), "echo 1", "H3b");
        assert_eq!(run(&[c('e'), up, up, up]), "echo 1", "H3c: it stops at the oldest match");
        assert_eq!(run(&[c('e'), up, down]), "e", "H4: back to what was typed");
        assert_eq!(run(&[up, down]), "", "H5");
        assert_eq!(run(&[up, up, up, up, up]), "echo 1", "H8");
        assert_eq!(run(&[c('l'), up]), "let g:a=1", "H9a");
        assert_eq!(run(&[c('x'), up]), "x", "H9b: no match, no change");
        assert_eq!(
            run(&[c('e'), up, c('x'), up]),
            "echo 22x",
            "H10: the typed text is the new prefix"
        );
        assert_eq!(run(&[up, up, down]), "echo 22", "H17");
        assert_eq!(run(&[c('e'), up, up, down, down]), "e", "H18");
        assert_eq!(
            run(&[c('e'), c('c'), up, bs, up]),
            "echo 2",
            "H21: the place in the list is kept"
        );
        assert_eq!(run(&[ctrl('p')]), "", "ctrl+p moves the list (Deviation 14)");
        let mut p = PaletteState::new(&crate::usage::UsageStore::default(), 0);
        p.handle_key(down);
        assert_eq!(p.selected(), 1, "ctrl+p's palette: ↓ still moves the list");
    }

    fn type_in(p: &mut PaletteState, s: &str) {
        for c in s.chars() {
            p.handle_key(key(KeyCode::Char(c)));
        }
    }

    /// Spec §5.9: the verb the query resolves to is row 0, selected, and
    /// Enter runs it; the fuzzy rows follow and stay real.
    #[test]
    fn the_ex_line_pins_the_verb_it_will_run() {
        let mut p = ex();
        assert!(p.pinned().is_none(), ": alone pins nothing");
        type_in(&mut p, "q");
        assert_eq!(p.pinned().map(|r| r.name), Some("quit"));
        assert_eq!(p.selected(), 0);
        assert_eq!(p.len(), 1 + p.filtered().len());
        let res = p.confirm().unwrap();
        assert_eq!(
            res.actions,
            vec![Action::CloseField, Action::CloseWindow { write: false, force: false }]
        );
        assert!(res.close && res.usage.is_none());
        p.handle_key(ctrl('n'));
        assert_eq!(p.selected(), 1);
        let res = p.confirm().unwrap();
        assert_eq!(res.actions, vec![p.filtered()[0].action.clone()], "a fuzzy row runs itself");
        let mut plain = PaletteState::new(&crate::usage::UsageStore::default(), 0);
        type_in(&mut plain, "q");
        assert!(plain.pinned().is_none(), "ctrl+p never pins");
    }

    #[test]
    fn a_refused_query_shows_one_row_and_runs_nothing() {
        let mut p = ex();
        type_in(&mut p, "s");
        assert_eq!(p.refused(), Some("Not supported: :s"));
        assert!(p.filtered().is_empty() && p.pinned().is_none());
        let res = p.confirm().unwrap();
        assert!(res.actions.is_empty() && res.close);
        let mut p = ex();
        type_in(&mut p, "zzzz");
        assert!(p.is_empty());
        let res = p.confirm().unwrap();
        assert!(
            matches!(res.actions.as_slice(), [Action::ShowToast(t, _)] if t == "Not a command: :zzzz")
        );
    }

    /// The second code review's second finding: typed text no verb spells
    /// runs no fuzzy row on Enter, though the rows still show (Vim's E492
    /// for `:W`); a row picked from them runs, by key or click.
    #[test]
    fn an_unspelled_query_runs_only_a_picked_row() {
        let mut p = ex();
        type_in(&mut p, "W");
        assert!(p.pinned().is_none() && p.refused().is_none());
        assert!(!p.filtered().is_empty(), "the fuzzy rows show");
        let res = p.handle_key(key(KeyCode::Enter)).unwrap();
        assert!(res.close && res.usage.is_none());
        assert!(
            matches!(res.actions.as_slice(), [Action::ShowToast(t, _)] if t == "Not a command: :W")
        );
        p.handle_key(ctrl('n'));
        p.handle_key(ctrl('p'));
        let res = p.handle_key(key(KeyCode::Enter)).unwrap();
        assert_eq!(res.actions, vec![p.filtered()[0].action.clone()], "a picked row runs");
        let mut p = ex();
        type_in(&mut p, "W");
        p.select(0);
        assert_eq!(
            p.confirm().unwrap().actions,
            vec![p.filtered()[0].action.clone()],
            "a clicked row runs"
        );
        let mut plain = PaletteState::new(&crate::usage::UsageStore::default(), 0);
        type_in(&mut plain, "W");
        let res = plain.handle_key(key(KeyCode::Enter)).unwrap();
        assert_eq!(
            res.actions,
            vec![plain.filtered()[0].action.clone()],
            "ctrl+p's Enter runs the top row"
        );
    }

    /// Vim's open wildmenu back at the typed text chooses no match, and
    /// from there ctrl+n chooses the first and ctrl+p the last (probed
    /// on Vim 9.1: `:A<Tab><C-P><C-N>` is `Aa`, `<C-P><C-P>` is `Ac`).
    /// The list here is always open (Deviation 14), so on the ex line
    /// with no verb pinned no row paints chosen, and the first ctrl+n
    /// picks row 0 rather than stepping past it.
    #[test]
    fn the_ex_line_chooses_no_row_until_one_is_picked() {
        let mut p = ex();
        type_in(&mut p, "curl");
        assert!(p.pinned().is_none() && p.filtered().len() > 1);
        assert_eq!(p.chosen_row(), None, "Enter would run no row, so none is chosen");
        p.handle_key(ctrl('n'));
        assert_eq!(p.chosen_row(), Some(0), "the first ctrl+n picks the first row");
        assert_eq!(p.confirm().unwrap().actions, vec![p.filtered()[0].action.clone()]);
        p.handle_key(ctrl('n'));
        assert_eq!(p.chosen_row(), Some(1));
        let mut p = ex();
        type_in(&mut p, "curl");
        p.handle_key(ctrl('p'));
        assert_eq!(p.chosen_row(), Some(p.len() - 1), "the first ctrl+p picks the last row");
        let mut p = ex();
        type_in(&mut p, "q");
        assert_eq!(p.chosen_row(), Some(0), "a pinned verb is what Enter runs");
        let mut plain = PaletteState::new(&crate::usage::UsageStore::default(), 0);
        type_in(&mut plain, "curl");
        assert_eq!(plain.chosen_row(), Some(0), "ctrl+p's Enter runs row 0");
    }

    /// Vim's `:<CR>` does nothing (the code review's third finding): with
    /// nothing typed and no row picked, Enter closes the line and runs no
    /// row, though the frecency list shows under it. A row picked from that
    /// list still runs; ctrl+p's palette is unchanged.
    #[test]
    fn enter_on_an_empty_ex_line_runs_nothing_unless_a_row_was_picked() {
        let mut p = ex();
        assert!(!p.filtered().is_empty(), "the list shows");
        let res = p.handle_key(key(KeyCode::Enter)).expect("closes");
        assert!(res.close && res.actions.is_empty() && res.usage.is_none());
        let mut p = ex();
        type_in(&mut p, "  ");
        let res = p.handle_key(key(KeyCode::Enter)).expect("closes");
        assert!(res.actions.is_empty(), "spaces are nothing typed");
        let mut p = ex();
        p.handle_key(ctrl('n'));
        p.handle_key(ctrl('p'));
        let res = p.handle_key(key(KeyCode::Enter)).unwrap();
        assert_eq!(res.actions, vec![p.filtered()[0].action.clone()], "a picked row runs");
        let mut p = ex();
        type_in(&mut p, "a");
        p.handle_key(ctrl('n'));
        p.handle_key(key(KeyCode::Backspace));
        assert!(p.input().is_empty());
        assert!(
            p.handle_key(key(KeyCode::Enter)).unwrap().actions.is_empty(),
            "an edit drops the pick"
        );
        let mut plain = PaletteState::new(&crate::usage::UsageStore::default(), 0);
        let res = plain.handle_key(key(KeyCode::Enter)).unwrap();
        assert_eq!(
            res.actions,
            vec![plain.filtered()[0].action.clone()],
            "ctrl+p's Enter runs row 0"
        );
    }

    #[test]
    fn backspace_on_an_empty_ex_line_closes_it() {
        let mut p = ex();
        type_in(&mut p, "a");
        assert!(p.handle_key(key(KeyCode::Backspace)).is_none(), "a char to delete");
        let res = p.handle_key(key(KeyCode::Backspace)).expect("closes");
        assert!(res.close && res.actions.is_empty());
        let mut plain = PaletteState::new(&crate::usage::UsageStore::default(), 0);
        assert!(
            plain.handle_key(key(KeyCode::Backspace)).is_none(),
            "ctrl+p's palette keeps it inert"
        );
    }
}

/// Ranking: the filter is still a subsequence match, but the surviving
/// rows sort by how well they match ("quit" must list Quit above
/// "Request: duplicate"), and frecency only breaks ties.
#[cfg(test)]
mod rank_tests {
    use super::*;
    use ratatui::crossterm::event::KeyModifiers;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn typed(query: &str, usage: &crate::usage::UsageStore) -> PaletteState {
        let mut p = PaletteState::new(usage, 1_000_000);
        for c in query.chars() {
            p.handle_key(key(KeyCode::Char(c)));
        }
        p
    }

    fn names(p: &PaletteState) -> Vec<&'static str> {
        p.filtered().iter().map(|c| c.name).collect()
    }

    #[test]
    fn match_rank_orders_exact_prefix_word_start_substring_then_subsequence() {
        let exact = match_rank("quit", "Quit").unwrap();
        let prefix = match_rank("qui", "Quit").unwrap();
        let word = match_rank("save", "Request: save").unwrap();
        let inside = match_rank("ave", "Request: save").unwrap();
        let scattered = match_rank("quit", "Request: duplicate").unwrap();
        assert!(exact < prefix, "{exact:?} vs {prefix:?}");
        assert!(prefix < word, "{prefix:?} vs {word:?}");
        assert!(word < inside, "{word:?} vs {inside:?}");
        assert!(inside < scattered, "{inside:?} vs {scattered:?}");
        assert_eq!(match_rank("xyz", "Quit"), None);
        assert_eq!(match_rank("", "Quit"), match_rank("", "Send request"));
    }

    /// Two scattered matches: the tighter one wins, then the earlier one.
    #[test]
    fn scattered_matches_rank_by_span_then_start() {
        let tight = match_rank("qut", "aq-u-t").unwrap();
        let loose = match_rank("qut", "q--u--t").unwrap();
        assert!(tight < loose, "{tight:?} vs {loose:?}");
        let early = match_rank("qt", "q-t--").unwrap();
        let late = match_rank("qt", "--q-t").unwrap();
        assert!(early < late, "{early:?} vs {late:?}");
    }

    #[test]
    fn the_exact_name_lists_first() {
        let p = typed("quit", &crate::usage::UsageStore::default());
        assert_eq!(names(&p)[0], "Quit");
        assert!(names(&p).contains(&"Request: duplicate"));
    }

    /// `qui` is a prefix of Quit and a scatter across "reQUest: duplIcate":
    /// the prefix wins, whatever the declaration order says.
    #[test]
    fn a_prefix_beats_a_scattered_match() {
        let p = typed("qui", &crate::usage::UsageStore::default());
        assert_eq!(names(&p)[0], "Quit");
        let p = typed("qu", &crate::usage::UsageStore::default());
        assert_eq!(names(&p)[0], "Quit");
    }

    /// Frecency can't lift a scattered match over a prefix — it only
    /// orders rows that match equally well.
    #[test]
    fn frecency_breaks_ties_within_a_tier_not_across_tiers() {
        let mut usage = crate::usage::UsageStore::default();
        for _ in 0..50 {
            usage.record("request-duplicate", 1_000_000);
        }
        let p = typed("qu", &usage);
        assert_eq!(names(&p)[0], "Quit", "prefix beats heavy use of a scatter");
        for _ in 0..50 {
            usage.record("focus-response", 1_000_000);
        }
        let p = typed("focus", &usage);
        assert_eq!(
            names(&p)[0],
            "Focus: response",
            "all three focus rows are equal prefix matches, so use decides"
        );
    }
}

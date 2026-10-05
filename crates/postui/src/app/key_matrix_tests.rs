//! The key-list matrix (piece 4 spec §8.1): table-driven cases, each a
//! key-list row ID, a profile, a named app state, keys in vim notation,
//! and what must hold after. Keys go through `App::handle_key`, and the
//! app renders at 160×48 after every key, so hit maps and viewports are
//! real. M2 adds the undo check (spec §8.1): a case marked `undo`
//! snapshots the app, runs, undoes once, and must be back exactly.

use super::*;
use crate::components::manage::ManageTab;
use crate::components::manage_list::{DetailRow, ListFocus};
use crate::components::sidebar::Row;
use crate::components::varmanager::{FormStop, VmField, VmFocus, VmRow};
use crate::config::KeyProfile;
use crate::keyroute::keylist::{self, Status};

#[derive(Debug, Clone, Copy)]
pub(super) enum Fixture {
    /// Requests alpha, beta and gamma; the cursor on beta.
    SidebarThree,
    /// `SidebarThree` with beta (under the cursor) open in the editor.
    SidebarThreeBetaOpen,
    /// `SidebarThree` with a y/n confirm on top; `y` toasts "yes".
    ConfirmYesNo,
    /// Headers a, b, c on an open request; the content focused on row 1.
    TableThreeHeaders,
    /// `TableThreeHeaders`, then row 0 deleted as one undo step.
    TableAfterDelete,
    /// `TableThreeHeaders` with the method badge selected.
    Method,
    /// `TableThreeHeaders` with the URL line selected, not open.
    UrlLine,
    /// A 200-element JSON array response, focused, the cursor on line 0.
    ResponseLines,
    /// The Environments list with three environments, cursor on row 0.
    ManageEnvs,
    /// `ManageEnvs` with the keyboard in the detail pane, on its first
    /// button (Rename).
    ManageEnvDetail,
    /// The Settings tab, cursor on row 0.
    Settings,
    /// The Variables list (variables alpha and beta, the selector `user`
    /// with options alice and bob in qa, the active environment), the
    /// cursor on alpha.
    VarList,
    /// `VarList` with `user` selected and its grid focused on (0, 0).
    VarGrid,
    /// `VarList` with alpha's form focused on Description.
    VarForm,
    /// The Spaces list with spaces main and auth, the cursor on row 0.
    ManageSpaces,
    /// `VarList` with the selector `user` under the cursor.
    VarListGroup,
    /// An open POST request whose body is `{`, `  "a": 1`, `}`, the body
    /// focused with the caret on row 0.
    Body,
}

#[derive(Debug, Clone, Copy)]
enum Expect {
    /// The sidebar cursor's request slug.
    SidebarSel(&'static str),
    TableRow(usize),
    /// 0 the name cell, 1 the value cell.
    TableCol(usize),
    TableLen(usize),
    ResponseLine(usize),
    ResponseLast,
    ManageCursor(usize),
    /// Which half of the Manage screen has the keyboard.
    ManageFocus(ListFocus),
    /// The detail pane has the keyboard, aimed at this row and column.
    DetailAim(DetailRow, usize),
    SettingsRow(usize),
    /// The editor pane's selected stop.
    Sub(SubFocus),
    /// The Variables list cursor's declared name.
    VarSel(&'static str),
    VmFocusIs(VmFocus),
    /// The options grid's cursor (row, column).
    GridCell(usize, usize),
    /// The variable form's field cursor.
    FormAt(FormStop),
    PromptOpen,
    /// Whether a field has the caret.
    FieldLive(bool),
    EditorSlug(&'static str),
    /// No request is open (a scratch).
    NoEditorSlug,
    Mode(&'static str),
    NoMode,
    Echo(&'static str),
    Toast(&'static str),
    NoToast,
    Palette,
    NoModal,
    /// The sidebar's request slugs, in row order.
    Requests(&'static [&'static str]),
    /// The Headers table's keys, in order.
    Headers(&'static [&'static str]),
    /// The Environments list, in order.
    EnvNames(&'static [&'static str]),
    /// How many options the selector `user` has in the active environment.
    OptionCount(usize),
    /// A chooser is the top modal.
    Picker,
    /// A confirm is the top dialog.
    Dialog,
    /// The palette's query text.
    PaletteQuery(&'static str),
    /// The palette's highlighted row.
    PaletteRow(usize),
    /// The row register holds this request.
    Yanked(&'static str),
    /// A selector's fields editor is the top modal.
    FieldsEditor,
    /// The main pane with the keyboard.
    FocusIs(PaneId),
    EditorTabIs(EditorTab),
    ResponseModeIs(crate::components::response::ViewMode),
    ManageTabIs(ManageTab),
    /// How many lines the response view shows.
    ResponseVisible(usize),
    /// The response cursor is on the view's top line, and the view has
    /// scrolled (a half page down from the top, `R.ctrl-d`).
    ResponsePaged,
}

struct Case {
    id: &'static str,
    profile: KeyProfile,
    fixture: Fixture,
    keys: &'static str,
    expect: &'static [Expect],
    undo: bool,
}

const fn vim(id: &'static str, fixture: Fixture, keys: &'static str, expect: &'static [Expect]) -> Case {
    Case { id, profile: KeyProfile::Vim, fixture, keys, expect, undo: false }
}

const fn arrows(id: &'static str, fixture: Fixture, keys: &'static str, expect: &'static [Expect]) -> Case {
    Case { id, profile: KeyProfile::Arrows, fixture, keys, expect, undo: false }
}

/// A case that changes data: after its checks, one undo (`u` in vim,
/// ctrl+z in arrows) must restore the snapshot taken before its keys.
const fn vim_u(id: &'static str, fixture: Fixture, keys: &'static str, expect: &'static [Expect]) -> Case {
    Case { id, profile: KeyProfile::Vim, fixture, keys, expect, undo: true }
}

const fn arrows_u(id: &'static str, fixture: Fixture, keys: &'static str, expect: &'static [Expect]) -> Case {
    Case { id, profile: KeyProfile::Arrows, fixture, keys, expect, undo: true }
}

use Expect::*;
use Fixture::*;

const CASES: &[Case] = &[
    vim("L.move", SidebarThree, "j", &[SidebarSel("main/gamma")]),
    vim("L.move", SidebarThree, "k", &[SidebarSel("main/alpha")]),
    vim("L.move", SidebarThree, "G", &[SidebarSel("main/gamma")]),
    vim("L.move", SidebarThree, "gg", &[SidebarSel("main/alpha")]),
    vim("L.move", SidebarThree, "2G", &[SidebarSel("main/beta")]),
    vim("L.move", SidebarThree, "9j", &[SidebarSel("main/gamma")]),
    vim("L.move", TableThreeHeaders, "j", &[TableRow(2)]),
    vim("L.move", TableThreeHeaders, "k", &[TableRow(0)]),
    vim("L.move", ManageEnvs, "j", &[ManageCursor(1)]),
    vim("L.move", ManageEnvDetail, "j", &[DetailAim(DetailRow::Tls, 0)]),
    vim("L.move", Settings, "j", &[SettingsRow(1)]),
    vim("L.move", VarList, "j", &[VarSel("beta")]),
    vim("L.move", VarList, "jk", &[VarSel("alpha")]),
    vim("L.move", VarGrid, "j", &[GridCell(1, 0)]),
    vim("L.move", VarForm, "j", &[FormAt(FormStop::Field(VmField::Default))]),
    // The badge and the URL line step down into the table.
    vim("L.move", Method, "j", &[Sub(SubFocus::Content)]),
    vim("L.move", UrlLine, "j", &[Sub(SubFocus::Content), FieldLive(false)]),
    arrows("L.move", SidebarThree, "<Down>", &[SidebarSel("main/gamma")]),
    arrows("L.move", SidebarThree, "j", &[SidebarSel("main/beta")]),
    arrows("L.move", VarGrid, "<Down>", &[GridCell(1, 0)]),
    arrows("L.move", VarGrid, "j", &[GridCell(0, 0)]),
    vim("L.hl", TableThreeHeaders, "l", &[TableCol(1)]),
    vim("L.hl", TableThreeHeaders, "lh", &[TableCol(0)]),
    vim("L.hl", ManageEnvDetail, "l", &[DetailAim(DetailRow::Buttons, 1)]),
    vim("L.hl", ManageEnvDetail, "h", &[ManageFocus(ListFocus::List)]),
    vim("L.hl", VarList, "l", &[VmFocusIs(VmFocus::Form)]),
    vim("L.hl", VarGrid, "l", &[GridCell(0, 1)]),
    vim("L.hl", VarGrid, "lh", &[GridCell(0, 0), VmFocusIs(VmFocus::Grid)]),
    vim("L.hl", VarGrid, "h", &[VmFocusIs(VmFocus::List)]),
    vim("L.hl", VarForm, "h", &[VmFocusIs(VmFocus::List)]),
    vim("L.hl", Method, "l", &[Sub(SubFocus::Url), FieldLive(false)]),
    arrows("L.hl", TableThreeHeaders, "<Right>", &[TableCol(1)]),
    vim("L.open", SidebarThree, "o", &[EditorSlug("main/beta")]),
    vim("L.open", TableThreeHeaders, "i", &[FieldLive(true)]),
    // `i` opens a field where `Surfaces::OPENS_FIELD` says, and only there.
    vim("L.open", VarGrid, "i", &[FieldLive(true), VmFocusIs(VmFocus::Grid)]),
    vim("L.open", VarForm, "i", &[FieldLive(true), VmFocusIs(VmFocus::Form)]),
    vim("L.open", VarList, "i", &[FieldLive(false), VmFocusIs(VmFocus::List), NoModal]),
    // Plan M1 Deviation 24: the aimed button's own Enter (Rename).
    vim("L.open", ManageEnvDetail, "i", &[PromptOpen, FieldLive(true)]),
    vim("L.open", UrlLine, "i", &[FieldLive(true), Sub(SubFocus::Url)]),
    arrows("L.open", UrlLine, "i", &[FieldLive(false)]),
    arrows("L.open", SidebarThree, "<CR>", &[EditorSlug("main/beta")]),
    arrows("L.open", TableThreeHeaders, "i", &[FieldLive(false)]),
    vim("L.undo", TableAfterDelete, "u", &[TableLen(3)]),
    vim("L.undo", TableAfterDelete, "u<C-r>", &[TableLen(2)]),
    arrows("L.undo", TableAfterDelete, "<C-z>", &[TableLen(3)]),
    arrows("L.undo", TableAfterDelete, "u", &[TableLen(2)]),
    vim("L.search", SidebarThree, "/j", &[Toast("No search in this list yet"), SidebarSel("main/gamma")]),
    vim("L.hml", SidebarThree, "Hj", &[SidebarSel("main/gamma"), NoToast]),
    vim("L.hml", SidebarThree, "zzj", &[SidebarSel("main/gamma"), NoToast]),
    vim("R.move", ResponseLines, "j", &[ResponseLine(1)]),
    vim("R.move", ResponseLines, "5j", &[ResponseLine(5)]),
    vim("R.move", ResponseLines, "G", &[ResponseLast]),
    vim("R.move", ResponseLines, "Ggg", &[ResponseLine(0)]),
    vim("R.move", ResponseLines, "10G", &[ResponseLine(9)]),
    arrows("R.move", ResponseLines, "<Down>", &[ResponseLine(1)]),
    vim("R.search", ResponseLines, "/", &[FieldLive(true)]),
    arrows("R.search", ResponseLines, "/", &[FieldLive(true)]),
    // Key list §7, M4a: half a page of view and cursor together, both
    // profiles; the tree's folds (ResponseLines' root is one array).
    vim("R.ctrl-d", ResponseLines, "<C-d>", &[ResponsePaged]),
    vim("R.ctrl-d", ResponseLines, "<C-d><C-u>", &[ResponseLine(0)]),
    arrows("R.ctrl-d", ResponseLines, "<C-d>", &[ResponsePaged]),
    vim("R.fold", ResponseLines, "jzc", &[ResponseVisible(1), ResponseLine(0)]),
    vim("R.fold", ResponseLines, "jzczo", &[ResponseVisible(202)]),
    vim("R.fold", ResponseLines, "za", &[ResponseVisible(1)]),
    vim("R.fold", ResponseLines, "zaj", &[ResponseVisible(1), ResponseLine(0), NoToast]),
    // Key list §8, M4a: panes by geometry (spec §5.8).
    vim("P.ctrlw", ResponseLines, "<C-w>k", &[FocusIs(PaneId::Editor)]),
    vim("P.ctrlw", ResponseLines, "<C-w>h", &[FocusIs(PaneId::Sidebar)]),
    vim("P.ctrlw", ResponseLines, "<C-w>h<C-w>l", &[FocusIs(PaneId::Response)]),
    vim("P.ctrlw", ResponseLines, "<C-w>k<C-w>j", &[FocusIs(PaneId::Response)]),
    vim("P.ctrlw", ResponseLines, "<C-w>w", &[FocusIs(PaneId::Sidebar)]),
    vim("P.ctrlw", ResponseLines, "<C-w>W", &[FocusIs(PaneId::Editor)]),
    vim("P.ctrlw", ResponseLines, "<C-w>k<C-w>p", &[FocusIs(PaneId::Response)]),
    vim("P.ctrlw", ResponseLines, "<C-w>xj", &[FocusIs(PaneId::Response), ResponseLine(1), NoToast]),
    vim("P.ctrlw", UrlLine, "i<Esc><C-w>j", &[FocusIs(PaneId::Response), FieldLive(false)]),
    vim("P.ctrlw", VarList, "<C-w>l", &[VmFocusIs(VmFocus::Form)]),
    vim("P.ctrlw", VarForm, "<C-w>h", &[VmFocusIs(VmFocus::List)]),
    vim("P.ctrlw", ManageEnvs, "<C-w>l", &[ManageFocus(ListFocus::Detail)]),
    vim("P.ctrlw", ManageEnvDetail, "<C-w>h", &[ManageFocus(ListFocus::List)]),
    arrows("P.ctrlw", ResponseLines, "<Tab>", &[FocusIs(PaneId::Sidebar)]),
    // Key list §8, M4a: tab strips (spec §5.8). The fixture is a GET, so
    // the editor's Body tab is disabled; the strip is Headers, Params, Vars.
    vim("P.gt", TableThreeHeaders, "gt", &[EditorTabIs(EditorTab::Params), Sub(SubFocus::Content)]),
    vim("P.gt", TableThreeHeaders, "gT", &[EditorTabIs(EditorTab::Vars)]),
    vim("P.gt", TableThreeHeaders, "3gt", &[EditorTabIs(EditorTab::Vars)]),
    vim("P.gt", TableThreeHeaders, "2gT", &[EditorTabIs(EditorTab::Params)]),
    vim("P.gt", UrlLine, "gt", &[EditorTabIs(EditorTab::Params), Sub(SubFocus::Content)]),
    vim("P.gt", UrlLine, "i<Esc>gt", &[EditorTabIs(EditorTab::Params), Sub(SubFocus::Content), FieldLive(false)]),
    vim("P.gt", ResponseLines, "gt", &[ResponseModeIs(crate::components::response::ViewMode::Raw), FocusIs(PaneId::Response)]),
    vim("P.gt", ManageEnvs, "gt", &[ManageTabIs(ManageTab::Spaces)]),
    arrows("P.gt", TableThreeHeaders, "<A-Right>", &[EditorTabIs(EditorTab::Params)]),
    arrows("P.gt", UrlLine, "<A-Right>", &[EditorTabIs(EditorTab::Params), Sub(SubFocus::Url)]),
    vim("G.echo", ResponseLines, "<C-w>", &[Echo("^W")]),
    vim("G.unsupported", SidebarThree, "xj", &[SidebarSel("main/gamma"), NoModal, NoToast]),
    vim("G.echo", SidebarThree, "3", &[Echo("3")]),
    vim("G.echo", SidebarThree, "2g", &[Echo("2g")]),
    vim("G.echo", SidebarThree, "2g<Esc>", &[Echo("")]),
    vim("G.mode", SidebarThree, "", &[Mode("NORMAL")]),
    vim("G.mode", TableThreeHeaders, "i", &[Mode("INSERT"), FieldLive(true)]),
    arrows("G.mode", SidebarThree, "", &[NoMode]),
    // `:` is X.verbs' (M4); M1's row opens the plain palette (Deviation 20).
    vim("X.verbs", SidebarThree, ":", &[Palette]),
    arrows("X.verbs", SidebarThree, ":", &[NoModal]),
    // Key list §6, M2: the list verbs (spec §5.1–§5.4). The shared letters
    // are the same in both profiles (the user's rule).
    vim("L.verbs", SidebarThree, "a", &[PromptOpen]),
    vim("L.verbs", SidebarThree, "r", &[PromptOpen]),
    vim("L.verbs", SidebarThree, "m", &[Toast("Only one space")]),
    vim("L.verbs", TableThreeHeaders, "a", &[FieldLive(true)]),
    vim("L.verbs", ManageEnvs, "a", &[PromptOpen]),
    vim("L.verbs", ManageEnvs, "r", &[PromptOpen]),
    vim("L.verbs", ManageSpaces, "m", &[Picker]),
    vim("L.verbs", VarList, "a", &[PromptOpen]),
    vim("L.verbs", VarList, "A", &[PromptOpen]),
    vim("L.verbs", VarList, "r", &[PromptOpen]),
    vim("L.verbs", VarGrid, "a", &[FieldLive(true)]),
    arrows("L.verbs", SidebarThree, "a", &[PromptOpen]),
    arrows("L.verbs", SidebarThree, "n", &[NoModal]),
    arrows("L.verbs", SidebarThree, "r", &[PromptOpen]),
    arrows("L.verbs", TableThreeHeaders, "a", &[FieldLive(true)]),
    arrows("L.verbs", ManageEnvs, "a", &[PromptOpen]),
    arrows("L.verbs", ManageSpaces, "m", &[Picker]),
    arrows("L.verbs", VarList, "A", &[PromptOpen]),
    arrows("L.verbs", VarList, "r", &[PromptOpen]),
    arrows("L.verbs", VarListGroup, "m", &[FieldsEditor]),
    arrows("L.verbs", VarGrid, "a", &[FieldLive(true)]),
    arrows("L.verbs", VarGrid, "o", &[FieldLive(true)]),
    // Delete never confirms (the user's rule); one undo brings it back.
    vim_u("L.dd", SidebarThree, "dd", &[Requests(&["main/alpha", "main/gamma"]), NoModal]),
    arrows_u("L.dd", SidebarThree, "d", &[Requests(&["main/alpha", "main/gamma"]), NoModal]),
    vim_u("L.dd", TableThreeHeaders, "dd", &[Headers(&["a", "c"])]),
    arrows_u("L.dd", TableThreeHeaders, "d", &[Headers(&["a", "c"])]),
    vim_u("L.dd", ManageEnvs, "dd", &[EnvNames(&["qa", "dev"]), NoModal]),
    arrows_u("L.dd", ManageEnvs, "d", &[EnvNames(&["qa", "dev"]), NoModal]),
    // A `d` that is not completed is vim's beep: nothing is deleted.
    vim("L.dd", SidebarThree, "dj", &[Requests(&["main/alpha", "main/beta", "main/gamma"]), SidebarSel("main/beta"), Echo("")]),
    // Review Focus 1: counted deletes, clamped, one undo step.
    vim_u("L.Ndd", SidebarThree, "2dd", &[Requests(&["main/alpha"]), Toast("Deleted 2 requests")]),
    vim_u("L.Ndd", SidebarThree, "9dd", &[Requests(&["main/alpha"])]),
    // The batch holds the open request: it becomes a scratch, and one undo
    // brings back the files, the open request and its field exactly.
    vim_u("L.Ndd", SidebarThreeBetaOpen, "2dd", &[Requests(&["main/alpha"]), NoEditorSlug]),
    arrows_u("L.dd", SidebarThreeBetaOpen, "d", &[Requests(&["main/alpha", "main/gamma"]), NoEditorSlug]),
    vim_u("L.Ndd", TableThreeHeaders, "2dd", &[Headers(&["a"])]),
    vim("L.Ndd", ManageEnvs, "2dd", &[Toast("Counted delete works on requests and table rows"), EnvNames(&["prod", "qa", "dev"])]),
    // Yank is invisible; put lands under (p) or over (P) the cursor.
    vim("L.yank", SidebarThree, "yy", &[Yanked("main/beta"), NoToast, Requests(&["main/alpha", "main/beta", "main/gamma"])]),
    vim("L.yank", SidebarThree, "3yy", &[Toast("Counted yank is not supported")]),
    vim_u(
        "L.yank",
        SidebarThree,
        "yyp",
        &[Requests(&["main/alpha", "main/beta", "main/beta-copy", "main/gamma"]), EditorSlug("main/beta-copy")],
    ),
    vim_u("L.yank", SidebarThree, "yyP", &[Requests(&["main/alpha", "main/beta-copy", "main/beta", "main/gamma"])]),
    arrows_u("L.yank", SidebarThree, "yp", &[Requests(&["main/alpha", "main/beta", "main/beta-copy", "main/gamma"])]),
    vim_u("L.yank", TableThreeHeaders, "yyp", &[Headers(&["a", "b", "b-copy", "c"]), TableRow(2)]),
    arrows_u("L.yank", TableThreeHeaders, "yP", &[Headers(&["a", "b-copy", "b", "c"]), TableRow(1)]),
    vim_u("L.yank", VarGrid, "yyp", &[OptionCount(3)]),
    arrows_u("L.yank", VarGrid, "cv", &[OptionCount(3)]),
    // Key list §9, M3: prompts, pickers and confirms (spec §5.10).
    vim("D.prompt", SidebarThree, "r", &[PromptOpen, Mode("INSERT")]),
    vim("D.prompt", SidebarThree, "r<Esc>", &[PromptOpen, Mode("NORMAL")]),
    vim("D.prompt", SidebarThree, "r<Esc><Esc>", &[NoModal, Requests(&["main/alpha", "main/beta", "main/gamma"])]),
    vim_u("D.prompt", SidebarThree, "r<Esc>ccdelta<CR>", &[NoModal, Requests(&["main/alpha", "main/delta", "main/gamma"])]),
    arrows_u("D.prompt", SidebarThree, "r<C-u>delta<CR>", &[NoModal, Requests(&["main/alpha", "main/delta", "main/gamma"])]),
    arrows("D.prompt", SidebarThree, "r<Esc>", &[NoModal, Requests(&["main/alpha", "main/beta", "main/gamma"])]),
    vim("D.picker", SidebarThree, ":xyz", &[PaletteQuery("xyz"), Mode("INSERT")]),
    vim("D.picker", SidebarThree, ":<C-n>", &[PaletteRow(1)]),
    vim("D.picker", SidebarThree, ":<Esc>", &[NoModal]),
    arrows("D.picker", SidebarThree, "<C-p>xyz", &[PaletteQuery("xyz")]),
    arrows("D.picker", SidebarThree, "<C-p><C-n>", &[PaletteRow(1)]),
    vim("D.confirm", ConfirmYesNo, "y", &[NoModal, Toast("yes")]),
    vim("D.confirm", ConfirmYesNo, "jx", &[Dialog, NoToast]),
    vim("D.confirm", ConfirmYesNo, "<Esc>", &[NoModal, NoToast]),
    arrows("D.confirm", ConfirmYesNo, "y", &[NoModal, Toast("yes")]),
    arrows("D.confirm", ConfirmYesNo, "<Esc>", &[NoModal]),
];

fn req(url: &str) -> postui_core::model::HttpRequest {
    postui_core::model::HttpRequest::from_toml_str(&format!(r#"url = "{url}""#)).unwrap()
}

/// An app on a project with variables alpha and beta, the selector `user`
/// (field `user`) with options alice and bob in qa, and qa active; the
/// Variables tab open with its rows built.
fn var_app() -> App {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    postui_core::fixtures::init_project(root, Some("matrix")).unwrap();
    std::fs::write(
        root.join("variables.toml"),
        "[alpha]\ndefault = \"1\"\n\n[beta]\ndefault = \"2\"\n\n[selectors.user]\nfields = [\"user\"]\n",
    )
    .unwrap();
    std::fs::write(
        root.join("environments/qa.toml"),
        "[options.user.alice]\nuser = \"1001\"\n\n[options.user.bob]\nuser = \"2002\"\n",
    )
    .unwrap();
    postui_core::fixtures::save_local_state(
        root,
        &postui_core::project::LocalState { environment: Some("qa".into()), ..Default::default() },
    )
    .unwrap();
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, root.to_path_buf());
    app._test_rx = Some(rx);
    app._test_dir = Some(dir);
    app.update(Action::OpenManage { tab: Some(ManageTab::Variables) });
    app.sync_varmanager();
    app
}

/// Puts the Variables list cursor (and detail pane) on `row`.
fn select_var_row(app: &mut App, row: VmRow) {
    let i = app.varmanager.left_rows.iter().position(|r| *r == row).expect("the row is listed");
    app.varmanager.select_row(i);
}

pub(super) fn build(f: Fixture) -> App {
    let mut app = match f {
        VarList | VarGrid | VarForm | VarListGroup => var_app(),
        _ => App::new_for_test(),
    };
    match f {
        SidebarThree | SidebarThreeBetaOpen | ConfirmYesNo => {
            for slug in ["main/alpha", "main/beta", "main/gamma"] {
                postui_core::fixtures::save_request(app.proj().root(), slug, &req("https://x/1")).unwrap();
            }
            app.update(Action::RefreshSidebar);
            if matches!(f, SidebarThreeBetaOpen) {
                app.update(Action::ForceOpenRequest("main/beta".into()));
                assert_eq!(app.editor.slug.as_deref(), Some("main/beta"));
            }
            // `.local/state.toml` on disk before any snapshot, through the
            // project's own writer (plan M2 Deviation 15, amended): core
            // writes a missing one at the first journaled op, and undo
            // leaves it.
            let expanded = app.proj().local().expanded.clone();
            app.proj_mut().set_expanded(expanded);
            app.focus = PaneId::Sidebar;
            let beta = app
                .sidebar
                .rows
                .iter()
                .position(|r| matches!(r, Row::Request { slug, .. } if slug == "main/beta"))
                .expect("beta is listed");
            app.sidebar.selected = Some(beta);
            if matches!(f, ConfirmYesNo) {
                app.push_modal(Modal::Confirm {
                    title: "Sure?".into(),
                    body: "b".into(),
                    choices: vec![
                        ('y', "Yes".into(), vec![Action::ShowToast("yes".into(), ToastKind::Info)]),
                        ('n', "No".into(), vec![]),
                    ],
                });
            }
        }
        TableThreeHeaders | TableAfterDelete | Method | UrlLine => {
            app.update(Action::CreateRequest("matrix".into()));
            for (k, v) in [("a", "1"), ("b", "2"), ("c", "3")] {
                app.editor
                    .headers
                    .insert(k.into(), postui_core::model::Entry { value: v.into(), enabled: true });
            }
            app.editor.active_tab = EditorTab::Headers;
            app.editor.preferred_tab = EditorTab::Headers;
            app.focus = PaneId::Editor;
            app.editor.sub_focus = SubFocus::Content;
            app.editor.table.selected = Some(1);
            app.capture_undo();
            if matches!(f, TableAfterDelete) {
                app.update(Action::DeleteTableRow(0));
                app.capture_undo();
            }
            match f {
                Method => app.editor.sub_focus = SubFocus::Method,
                UrlLine => app.editor.sub_focus = SubFocus::Url,
                _ => {}
            }
        }
        ResponseLines => {
            let body = format!("[{}]", (0..200).map(|i| i.to_string()).collect::<Vec<_>>().join(","));
            app.session.response.set_state(
                ResponseState::Ready(Box::new(crate::http::ResponseData {
                    status: 200,
                    url: "https://x.test/a".into(),
                    headers: vec![("content-type".into(), "application/json".into())],
                    sent_headers: Vec::new(),
                    body: body.clone(),
                    ttfb: std::time::Duration::from_millis(1),
                    elapsed: std::time::Duration::from_millis(1),
                    size: body.len(),
                    content_type: Some("application/json".into()),
                })),
                app.session.send_generation,
            );
            app.focus = PaneId::Response;
        }
        ManageEnvs | ManageEnvDetail => {
            for name in ["prod", "qa", "dev"] {
                app.update(Action::CreateEnv(name.into()));
            }
            app.update(Action::OpenManage { tab: Some(ManageTab::Environments) });
            app.manage.list.cursor = 0;
            if matches!(f, ManageEnvDetail) {
                // In by the arrows profile's own key, after a draw has
                // painted the buttons the aim lands on.
                draw(&mut app);
                app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
                assert_eq!(
                    (app.manage.list.focus, app.manage.list.detail_row, app.manage.list.detail_col),
                    (ListFocus::Detail, DetailRow::Buttons, 0)
                );
            }
        }
        Settings => {
            app.update(Action::OpenManage { tab: Some(ManageTab::Settings) });
            app.settings.set_cursor(0);
        }
        VarList => select_var_row(&mut app, VmRow::Var("alpha".into())),
        VarGrid => {
            select_var_row(&mut app, VmRow::Group("user".into()));
            app.varmanager.focus = VmFocus::Grid;
            app.varmanager.grid.cursor = (0, 0);
        }
        VarForm => {
            select_var_row(&mut app, VmRow::Var("alpha".into()));
            app.varmanager.focus = VmFocus::Form;
            app.varmanager.form_cursor = FormStop::Field(VmField::Description);
        }
        ManageSpaces => {
            app.update(Action::CreateSpace("auth".into()));
            app.update(Action::OpenManage { tab: Some(ManageTab::Spaces) });
            app.manage.list.cursor = 0;
        }
        VarListGroup => select_var_row(&mut app, VmRow::Group("user".into())),
        Body => {
            app.update(Action::CreateRequest("matrix".into()));
            app.editor.method = postui_core::model::Method::Post;
            app.editor.set_body_text("{\n  \"a\": 1\n}");
            app.editor.active_tab = EditorTab::Body;
            app.editor.preferred_tab = EditorTab::Body;
            app.focus = PaneId::Editor;
            app.editor.sub_focus = SubFocus::Content;
            app.capture_undo();
        }
    }
    app.toasts = Default::default();
    app
}

pub(super) fn draw(app: &mut App) {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    app.anims.finish_all();
    let mut terminal = Terminal::new(TestBackend::new(160, 48)).unwrap();
    terminal.draw(|f| crate::ui::draw(f, app)).unwrap();
}

fn check(app: &App, e: Expect) -> Result<(), String> {
    let ok = match e {
        SidebarSel(slug) => app.sidebar.selected_slug().as_deref() == Some(slug),
        TableRow(n) => app.editor.table.selected == Some(n),
        TableCol(n) => usize::from(app.editor.table.col == crate::components::table_editor::Col::Value) == n,
        TableLen(n) => app.editor.headers.len() == n,
        ResponseLine(n) => app.session.response.view().is_some_and(|v| v.cursor == n),
        ResponseLast => app.session.response.view().is_some_and(|v| v.cursor + 1 == v.visible_len()),
        FocusIs(p) => app.focus == p,
        EditorTabIs(t) => app.editor.active_tab == t,
        ResponseModeIs(m) => app.session.response.view().is_some_and(|v| v.mode == m),
        ManageTabIs(t) => app.manage.tab == t,
        ResponseVisible(n) => app.session.response.view().is_some_and(|v| v.visible_len() == n),
        ResponsePaged => app.session.response.view().is_some_and(|v| v.scroll > 0 && v.cursor == v.scroll),
        ManageCursor(n) => app.manage.list.cursor == n,
        ManageFocus(f) => app.manage.list.focus == f,
        DetailAim(row, col) => {
            app.manage.list.focus == ListFocus::Detail
                && app.manage.list.detail_row == row
                && app.manage.list.detail_col == col
        }
        SettingsRow(n) => app.settings.cursor == n,
        Sub(s) => app.focus == PaneId::Editor && app.editor.sub_focus == s,
        VarSel(name) => {
            app.varmanager.left_rows.get(app.varmanager.left_cursor).and_then(VmRow::name) == Some(name)
        }
        VmFocusIs(f) => app.varmanager.focus == f,
        GridCell(row, col) => app.varmanager.grid.cursor == (row, col),
        FormAt(stop) => app.varmanager.form_cursor == stop,
        PromptOpen => matches!(app.modals.top(), Some(Modal::Prompt { .. })),
        FieldLive(live) => app.field_caret().is_live() == live,
        EditorSlug(slug) => app.editor.slug.as_deref() == Some(slug),
        NoEditorSlug => app.editor.slug.is_none(),
        Mode(m) => app.mode_indicator() == Some(m),
        NoMode => app.mode_indicator().is_none(),
        Echo(s) => app.key_echo() == s,
        Toast(s) => app.toasts.messages().iter().any(|m| m.contains(s)),
        NoToast => app.toasts.messages().is_empty(),
        Palette => matches!(app.modals.top(), Some(Modal::Palette(_))),
        NoModal => app.modals.is_empty(),
        Requests(slugs) => app
            .sidebar
            .rows
            .iter()
            .filter_map(|r| match r {
                Row::Request { slug, .. } => Some(slug.as_str()),
                Row::Folder { .. } => None,
            })
            .eq(slugs.iter().copied()),
        Headers(keys) => app.editor.headers.keys().map(String::as_str).eq(keys.iter().copied()),
        EnvNames(names) => app
            .manage_items(ManageTab::Environments)
            .iter()
            .map(String::as_str)
            .eq(names.iter().copied()),
        OptionCount(n) => {
            app.project()
                .and_then(|p| postui_core::varmodel::options_of(p.variables(), p.env_data(), "user"))
                .map_or(0, |o| o.len())
                == n
        }
        Dialog => matches!(app.modals.top(), Some(Modal::Confirm { .. })),
        PaletteQuery(q) => matches!(app.modals.top(), Some(Modal::Palette(p)) if p.input() == q),
        PaletteRow(n) => matches!(app.modals.top(), Some(Modal::Palette(p)) if p.selected() == n),
        Picker => matches!(app.modals.top(), Some(Modal::Chooser(_))),
        Yanked(slug) => app.yank == crate::yank::Register::Request(slug.into()),
        FieldsEditor => matches!(app.modals.top(), Some(Modal::FieldsEditor(_))),
    };
    ok.then_some(()).ok_or_else(|| {
        format!(
            "{e:?} does not hold (surface {:?}, selected {:?}, table {:?}, sub-focus {:?}, \
             vm {:?} {:?} {:?}, detail {:?} {:?}, echo {:?}, mode {:?}, toasts {:?})",
            app.surface(),
            app.sidebar.selected_slug(),
            app.editor.table.selected,
            app.editor.sub_focus,
            app.varmanager.focus,
            app.varmanager.grid.cursor,
            app.varmanager.form_cursor,
            app.manage.list.detail_row,
            app.manage.list.detail_col,
            app.key_echo(),
            app.mode_indicator(),
            app.toasts.messages()
        )
    })
}

/// What an undo must bring back exactly (spec §8.1; the user's rule:
/// side effects included). The project's trash is left out: a delete
/// fills it and an undo empties it again, which is the journal's
/// bookkeeping, not state the user sees (plan M2 Deviation 15).
#[derive(Debug, PartialEq)]
pub(super) struct Snapshot {
    files: std::collections::BTreeMap<String, Vec<u8>>,
    rows: Vec<String>,
    cursor: Option<crate::components::sidebar::RowKey>,
    slug: Option<String>,
    request: String,
    field: crate::keyroute::FieldCaret,
}

impl Snapshot {
    /// Every part but `field`. Whether a text field is open is not undo
    /// state (user ruling 2026-10-03), so a test that starts from a
    /// mid-session snapshot compares with this and checks `field` itself.
    pub(super) fn same_but_field(&self, o: &Snapshot) -> bool {
        self.files == o.files
            && self.rows == o.rows
            && self.cursor == o.cursor
            && self.slug == o.slug
            && self.request == o.request
    }
}

pub(super) fn snapshot(app: &App) -> Snapshot {
    let root = app.proj().root().to_path_buf();
    let mut files = std::collections::BTreeMap::new();
    let mut dirs = vec![root.clone()];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            let rel = path.strip_prefix(&root).unwrap().to_string_lossy().replace('\\', "/");
            if rel == ".local/trash" || rel.starts_with(".local/trash/") {
                continue;
            }
            if path.is_dir() {
                dirs.push(path);
            } else {
                files.insert(rel, std::fs::read(&path).unwrap());
            }
        }
    }
    Snapshot {
        files,
        rows: app.sidebar.rows.iter().map(|r| format!("{r:?}")).collect(),
        cursor: app.sidebar.selected_key(),
        slug: app.editor.slug.clone(),
        request: app.editor.current_request().to_toml_string(),
        field: app.field_caret(),
    }
}

fn run(case: &Case) -> Result<(), String> {
    let mut app = build(case.fixture);
    app.use_profile(case.profile);
    draw(&mut app);
    let before = case.undo.then(|| {
        assert!(
            app.proj().root().join(".local/state.toml").is_file(),
            "{:?} must write .local/state.toml before the snapshot (plan M2 Deviation 15)",
            case.fixture
        );
        snapshot(&app)
    });
    feed(&mut app, case.keys);
    for e in case.expect {
        check(&app, *e)?;
    }
    if let Some(before) = before {
        feed(&mut app, if case.profile == KeyProfile::Vim { "u" } else { "<C-z>" });
        let after = snapshot(&app);
        if after != before {
            return Err(format!("undo did not restore exactly:\n{}", snapshot_diff(&before, &after)));
        }
    }
    Ok(())
}

/// Each field where `after` differs from `before`, files as text.
pub(super) fn snapshot_diff(before: &Snapshot, after: &Snapshot) -> String {
    let text = |b: Option<&Vec<u8>>| b.map(|b| String::from_utf8_lossy(b).into_owned());
    let mut out = Vec::new();
    let paths: std::collections::BTreeSet<&String> = before.files.keys().chain(after.files.keys()).collect();
    for path in paths {
        let (b, a) = (before.files.get(path), after.files.get(path));
        if b != a {
            out.push(format!("file {path}:\n  before {:?}\n  after  {:?}", text(b), text(a)));
        }
    }
    if before.rows != after.rows {
        out.push(format!("rows:\n  before {:?}\n  after  {:?}", before.rows, after.rows));
    }
    if before.cursor != after.cursor {
        out.push(format!("cursor: before {:?}, after {:?}", before.cursor, after.cursor));
    }
    if before.slug != after.slug {
        out.push(format!("slug: before {:?}, after {:?}", before.slug, after.slug));
    }
    if before.request != after.request {
        out.push(format!("request:\n  before {:?}\n  after  {:?}", before.request, after.request));
    }
    if before.field != after.field {
        out.push(format!("field: before {:?}, after {:?}", before.field, after.field));
    }
    out.join("\n")
}

/// Keys through `App::handle_key`, each followed by the event loop's
/// undo capture and a draw (spec §8.1).
pub(super) fn feed(app: &mut App, keys: &str) {
    for ev in crate::keyroute::notation::events(keys) {
        app.handle_key(ev);
        app.capture_undo();
        draw(app);
    }
}

#[test]
fn the_matrix_holds() {
    let failures: Vec<String> = CASES
        .iter()
        .filter_map(|c| run(c).err().map(|m| format!("{} {:?} {:?}: {m}", c.id, c.profile, c.keys)))
        .collect();
    assert!(failures.is_empty(), "{} case(s) failed:\n{}", failures.len(), failures.join("\n"));
}

/// Spec §5.3: undo of a counted table delete reselects the first restored
/// row (amendment A8-3).
#[test]
fn a_counted_table_delete_undone_reselects_the_first_restored_row() {
    let mut app = build(TableThreeHeaders);
    app.use_profile(KeyProfile::Vim);
    draw(&mut app);
    feed(&mut app, "2dd");
    check(&app, Headers(&["a"])).unwrap();
    // Move off the row the delete left selected, so an undo that never
    // touches the cursor cannot pass.
    feed(&mut app, "k");
    check(&app, TableRow(0)).unwrap();
    feed(&mut app, "u");
    check(&app, Headers(&["a", "b", "c"])).unwrap();
    check(&app, TableRow(1)).unwrap();
}

/// A refused counted put is one refusal: one toast, not one per copy
/// (final review M1; the count rides on the one action).
#[test]
fn a_refused_counted_put_toasts_once() {
    let mut app = build(SidebarThree);
    app.use_profile(KeyProfile::Vim);
    draw(&mut app);
    app.yank = crate::yank::Register::Request("auth/login".into());
    feed(&mut app, "3p");
    let n = app.toasts.messages().iter().filter(|m| m.contains("was yanked from another space")).count();
    assert_eq!(n, 1, "{:?}", app.toasts.messages());
}

/// A counted put that works makes every copy, and one `u` takes them all
/// back (vim's `3p` is one change).
#[test]
fn a_counted_put_that_works_makes_every_copy_as_one_undo_step() {
    let mut app = build(SidebarThree);
    app.use_profile(KeyProfile::Vim);
    draw(&mut app);
    let requests = |app: &App| app.sidebar.rows.iter().filter(|r| matches!(r, Row::Request { .. })).count();
    feed(&mut app, "yy3p");
    assert_eq!(requests(&app), 6, "{:?}", app.sidebar.rows);
    feed(&mut app, "u");
    assert_eq!(requests(&app), 3, "{:?}", app.sidebar.rows);
}

/// Spec §8.2's coverage, up to this build's milestone.
#[test]
fn every_shipped_key_list_row_has_a_case() {
    for row in keylist::KEY_LIST.iter().filter(|r| r.milestone <= keylist::MILESTONE) {
        let cases: Vec<&Case> = CASES.iter().filter(|c| c.id == row.id).collect();
        match row.status {
            Status::Ship => {
                assert!(cases.iter().any(|c| c.profile == KeyProfile::Vim), "{} has no vim case", row.id);
                if row.arrows {
                    assert!(cases.iter().any(|c| c.profile == KeyProfile::Arrows), "{} has no arrows case", row.id);
                }
            }
            Status::Out => assert!(
                cases.iter().any(|c| c.keys.ends_with('j')),
                "{} needs a negative case: its keys, then j",
                row.id
            ),
            Status::Engine => {}
        }
    }
    for c in CASES {
        assert!(keylist::KEY_LIST.iter().any(|r| r.id == c.id), "a case names an unknown row: {}", c.id);
    }
}

use crate::action::Verb;
use crate::keyroute::table::{self, Intent, Surfaces};

/// Every verb, once (a count does not change what a verb reaches).
const VERBS: [Verb; 14] = [
    Verb::Add,
    Verb::AddAlt,
    Verb::Rename,
    Verb::Move,
    Verb::Delete { count: 1 },
    Verb::Yank,
    Verb::Put { above: false, count: 1 },
    Verb::EditValue,
    Verb::Promote,
    Verb::Fields,
    Verb::Secret,
    Verb::ClearValue,
    Verb::Reveal,
    Verb::Tls,
];

/// Plan M2 Deviations 1 and 9: the verbs vim reaches only through M4's ex
/// rows (`:tls`, `:secret`, `:fields`, `:promote`, `:clear`, `:reveal`,
/// `:rename`, `:value`, `:delete` on a detail pane), or by mouse until
/// then. M4 empties this list; nothing else may join it.
const VIM_UNTIL_M4: &[(Surfaces, Verb)] = &[
    (Surfaces::MANAGE_LIST, Verb::Tls),
    (Surfaces::MANAGE_DETAIL, Verb::Add),
    (Surfaces::MANAGE_DETAIL, Verb::Rename),
    (Surfaces::MANAGE_DETAIL, Verb::Delete { count: 1 }),
    (Surfaces::MANAGE_DETAIL, Verb::Move),
    (Surfaces::MANAGE_DETAIL, Verb::Tls),
    (Surfaces::VAR_LIST, Verb::Secret),
    (Surfaces::VAR_LIST, Verb::Fields),
    (Surfaces::VAR_GRID, Verb::Rename),
    (Surfaces::VAR_GRID, Verb::EditValue),
    (Surfaces::VAR_GRID, Verb::Fields),
    (Surfaces::VAR_FORM, Verb::Secret),
    (Surfaces::VAR_FORM, Verb::ClearValue),
    (Surfaces::VAR_FORM, Verb::Promote),
    (Surfaces::VAR_FORM, Verb::Reveal),
];

/// No `VIM_UNTIL_M4` entry may have a vim row: that is the signal to take
/// it off the list, and it needs no fixture (some entries no fixture
/// offers).
#[test]
fn vim_until_m4_names_only_verbs_vim_cannot_reach() {
    for (on, verb) in VIM_UNTIL_M4 {
        let stale = table::TABLE.iter().find(|b| {
            b.profile.has(KeyProfile::Vim)
                && b.on.minus(*on) != b.on // the row's surfaces overlap the entry's
                && matches!(b.intent, Intent::Verb(v) if v.kind() == verb.kind())
        });
        assert!(stale.is_none(), "{verb:?} has a vim row on {on:?} now; take it off VIM_UNTIL_M4");
    }
}

const SWEEP: [Fixture; 9] = [
    SidebarThree,
    TableThreeHeaders,
    ManageEnvs,
    ManageEnvDetail,
    ManageSpaces,
    VarList,
    VarListGroup,
    VarGrid,
    VarForm,
];

fn verb_row(profile: KeyProfile, surface: crate::keyroute::Surface, verb: Verb) -> bool {
    table::TABLE.iter().any(|b| {
        b.profile.has(profile) && b.on.has(surface) && matches!(b.intent, Intent::Verb(v) if v.kind() == verb.kind())
    })
}

/// Spec §8.2, as plan M2 Deviation 9 reads it: (1) every list verb a
/// fixture offers has a row in each profile, but for `VIM_UNTIL_M4`;
/// (2) every action an arrows default key reaches, vim reaches too — by
/// its keymap, a table row, a verb row, or the palette (`:` opens it).
#[test]
fn every_action_is_keyboard_reachable_in_both_profiles() {
    let mut vim_verb_actions: Vec<Action> = Vec::new();
    for f in SWEEP {
        let app = build(f);
        let surface = app.surface();
        for verb in VERBS {
            let Some(action) = app.selection_action(surface, verb) else { continue };
            assert!(verb_row(KeyProfile::Arrows, surface, verb), "{f:?}: arrows has no key for {verb:?}");
            let waits = VIM_UNTIL_M4.iter().any(|(on, v)| on.has(surface) && v.kind() == verb.kind());
            if verb_row(KeyProfile::Vim, surface, verb) {
                assert!(!waits, "{f:?}: {verb:?} has a vim row now; take it off VIM_UNTIL_M4");
                vim_verb_actions.push(action);
            } else {
                assert!(waits, "{f:?}: vim has no key for {verb:?}");
            }
        }
    }
    let arrows = crate::keys::Keymap::default_bindings_for(KeyProfile::Arrows);
    let vim = crate::keys::Keymap::default_bindings_for(KeyProfile::Vim);
    let palette: Vec<Action> = crate::components::palette::all_commands().into_iter().map(|c| c.action).collect();
    for (name, action) in crate::keys::named_actions() {
        if arrows.combo_for(name).is_none() {
            continue; // keys.toml-only: no profile has a default key (Deviation 9)
        }
        let reached = vim.combo_for(name).is_some()
            || table::TABLE.iter().any(|b| {
                b.profile.has(KeyProfile::Vim)
                    && match b.intent {
                        Intent::Act(f) => f() == action,
                        Intent::Tab { delta, leave: false } => action == Action::CycleTabs(delta),
                        _ => false,
                    }
            })
            || vim_verb_actions.contains(&action)
            || palette.contains(&action);
        assert!(reached, "{name}: arrows binds a key to it, and vim reaches it no way at all");
    }
}

/// The footer chips a fixture paints, unspelled: the Manage screens'
/// own, or the main panes' as `ui` builds them.
fn chips(app: &App, f: Fixture) -> Vec<crate::components::footer::FooterChip> {
    use crate::components::footer::{JqBarState, footer_chips};
    let main = |pane: PaneId, row: Option<(usize, bool)>| -> Vec<crate::components::footer::FooterChip> {
        footer_chips(pane, false, false, Some("add header"), false, false, row, JqBarState::Closed, false)
            .into_iter()
            .map(|(k, l, a)| (k, l.to_string(), a))
            .collect()
    };
    match f {
        SidebarThree => main(PaneId::Sidebar, None),
        TableThreeHeaders => main(PaneId::Editor, Some((1, true))),
        _ => crate::ui::manage_tab_chips(app),
    }
}

/// Spec §8.2's spelling test, for the verb chips (plan M2 Deviation 3):
/// a chip whose keycap is a verb's row dispatches exactly what that verb
/// resolves to on its surface, and the keycap's keys, fed to the router,
/// run that verb.
#[test]
fn every_verb_chip_resolves_to_its_chip() {
    use crate::keyroute::spell::Key;
    use crate::keyroute::{KeyCtx, Resolved, Router, Step};
    for profile in [KeyProfile::Vim, KeyProfile::Arrows] {
        let mut checked = 0;
        for f in SWEEP {
            let mut app = build(f);
            app.use_profile(profile);
            draw(&mut app);
            let surface = app.surface();
            for (key, label, action) in chips(&app, f) {
                let verb = match key {
                    Key::Of(Action::OnSelection(v)) | Key::OneOf(Action::OnSelection(v), _) => v,
                    _ => continue,
                };
                let Some(action) = action else { continue };
                assert_eq!(
                    app.selection_action(surface, verb),
                    Some(action.clone()),
                    "{profile:?} {f:?}: the {label} chip and its key disagree"
                );
                let Some(keys) = table::keys_for(profile, surface, &Action::OnSelection(verb)) else {
                    continue; // no key in this profile: the chip drops its keycap
                };
                let ctx = KeyCtx { profile, surface, field: app.field_caret(), user_bare: None, cap: usize::MAX, engine: None, answers: Vec::new() };
                let mut router = Router::default();
                let mut last = Resolved::Declined;
                for ev in crate::keyroute::notation::events(keys) {
                    last = router.resolve(ev, &ctx);
                }
                let ran: Vec<&Step> = match &last {
                    Resolved::Run(steps) => steps.iter().filter(|s| !matches!(s, Step::OpenAs(_))).collect(),
                    _ => Vec::new(),
                };
                assert!(
                    matches!(ran.as_slice(), [Step::Action(Action::OnSelection(v))] if v.kind() == verb.kind()),
                    "{profile:?} {f:?}: {keys:?} ran {last:?}, not the {label} chip's verb"
                );
                checked += 1;
            }
        }
        // A floor, so a `continue` that skips every chip cannot pass.
        assert!(checked >= 20, "{profile:?}: only {checked} verb chips were checked");
    }
}

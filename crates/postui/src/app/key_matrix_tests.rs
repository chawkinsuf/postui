//! The key-list matrix (piece 4 spec §8.1): table-driven cases, each a
//! key-list row ID, a profile, a named app state, keys in vim notation,
//! and what must hold after. Keys go through `App::handle_key`, and the
//! app renders at 160×48 after every key, so hit maps and viewports are
//! real. M1 has no undo check yet (plan M1 Deviation 12).

use super::*;
use crate::components::manage::ManageTab;
use crate::components::sidebar::Row;
use crate::config::KeyProfile;
use crate::keyroute::keylist::{self, Status};

#[derive(Debug, Clone, Copy)]
enum Fixture {
    /// Requests alpha, beta and gamma; the cursor on beta.
    SidebarThree,
    /// Headers a, b, c on an open request; the content focused on row 1.
    TableThreeHeaders,
    /// `TableThreeHeaders`, then row 0 deleted as one undo step.
    TableAfterDelete,
    /// A 200-element JSON array response, focused, the cursor on line 0.
    ResponseLines,
    /// The Environments list with three environments, cursor on row 0.
    ManageEnvs,
    /// The Settings tab, cursor on row 0.
    Settings,
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
    SettingsRow(usize),
    /// Whether a field has the caret.
    FieldLive(bool),
    EditorSlug(&'static str),
    Mode(&'static str),
    NoMode,
    Echo(&'static str),
    Toast(&'static str),
    NoToast,
    Palette,
    NoModal,
}

struct Case {
    id: &'static str,
    profile: KeyProfile,
    fixture: Fixture,
    keys: &'static str,
    expect: &'static [Expect],
}

const fn vim(id: &'static str, fixture: Fixture, keys: &'static str, expect: &'static [Expect]) -> Case {
    Case { id, profile: KeyProfile::Vim, fixture, keys, expect }
}

const fn arrows(id: &'static str, fixture: Fixture, keys: &'static str, expect: &'static [Expect]) -> Case {
    Case { id, profile: KeyProfile::Arrows, fixture, keys, expect }
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
    vim("L.move", Settings, "j", &[SettingsRow(1)]),
    arrows("L.move", SidebarThree, "<Down>", &[SidebarSel("main/gamma")]),
    arrows("L.move", SidebarThree, "j", &[SidebarSel("main/beta")]),
    vim("L.hl", TableThreeHeaders, "l", &[TableCol(1)]),
    vim("L.hl", TableThreeHeaders, "lh", &[TableCol(0)]),
    arrows("L.hl", TableThreeHeaders, "<Right>", &[TableCol(1)]),
    vim("L.open", SidebarThree, "o", &[EditorSlug("main/beta")]),
    vim("L.open", TableThreeHeaders, "i", &[FieldLive(true)]),
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
];

fn req(url: &str) -> postui_core::model::HttpRequest {
    postui_core::model::HttpRequest::from_toml_str(&format!(r#"url = "{url}""#)).unwrap()
}

fn build(f: Fixture) -> App {
    let mut app = App::new_for_test();
    match f {
        SidebarThree => {
            for slug in ["main/alpha", "main/beta", "main/gamma"] {
                postui_core::fixtures::save_request(app.proj().root(), slug, &req("https://x/1")).unwrap();
            }
            app.update(Action::RefreshSidebar);
            app.focus = PaneId::Sidebar;
            let beta = app
                .sidebar
                .rows
                .iter()
                .position(|r| matches!(r, Row::Request { slug, .. } if slug == "main/beta"))
                .expect("beta is listed");
            app.sidebar.selected = Some(beta);
        }
        TableThreeHeaders | TableAfterDelete => {
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
        ManageEnvs => {
            for name in ["prod", "qa", "dev"] {
                app.update(Action::CreateEnv(name.into()));
            }
            app.update(Action::OpenManage { tab: Some(ManageTab::Environments) });
            app.manage.list.cursor = 0;
        }
        Settings => {
            app.update(Action::OpenManage { tab: Some(ManageTab::Settings) });
            app.settings.set_cursor(0);
        }
    }
    app.toasts = Default::default();
    app
}

fn draw(app: &mut App) {
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
        ManageCursor(n) => app.manage.list.cursor == n,
        SettingsRow(n) => app.settings.cursor == n,
        FieldLive(live) => app.field_caret().is_live() == live,
        EditorSlug(slug) => app.editor.slug.as_deref() == Some(slug),
        Mode(m) => app.mode_indicator() == Some(m),
        NoMode => app.mode_indicator().is_none(),
        Echo(s) => app.key_echo() == s,
        Toast(s) => app.toasts.messages().iter().any(|m| m.contains(s)),
        NoToast => app.toasts.messages().is_empty(),
        Palette => matches!(app.modals.top(), Some(Modal::Palette(_))),
        NoModal => app.modals.is_empty(),
    };
    ok.then_some(()).ok_or_else(|| {
        format!(
            "{e:?} does not hold (selected {:?}, table {:?}, echo {:?}, mode {:?}, toasts {:?})",
            app.sidebar.selected_slug(),
            app.editor.table.selected,
            app.key_echo(),
            app.mode_indicator(),
            app.toasts.messages()
        )
    })
}

fn run(case: &Case) -> Result<(), String> {
    let mut app = build(case.fixture);
    app.use_profile(case.profile);
    draw(&mut app);
    for ev in crate::keyroute::notation::events(case.keys) {
        app.handle_key(ev);
        draw(&mut app);
    }
    for e in case.expect {
        check(&app, *e)?;
    }
    Ok(())
}

#[test]
fn the_matrix_holds() {
    let failures: Vec<String> = CASES
        .iter()
        .filter_map(|c| run(c).err().map(|m| format!("{} {:?} {:?}: {m}", c.id, c.profile, c.keys)))
        .collect();
    assert!(failures.is_empty(), "{} case(s) failed:\n{}", failures.len(), failures.join("\n"));
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

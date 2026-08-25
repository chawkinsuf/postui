//! Stage-6 acceptance sweep (spec §7 tier 3), carried onto the stage-7
//! variable model: builds a project's entire variable system —
//! declarations, groups with their per-environment entries, a secret,
//! environment values, a picker selection, and a request-scope override — purely
//! through the same `Action`s the painted UI dispatches (no hand-written
//! `variables.toml`; the one exception, per the task brief, is the
//! `variables_toml_comments_survive_one_manager_edit` fixture proving
//! write-fidelity on a human-authored file). Then drives a real send
//! through wiremock with everything substituted, including the secret
//! reached via the send-time prompt chain, and asserts the on-disk files
//! match exact expected text.

use indexmap::IndexMap;
use postui::action::Action;
use postui::app::App;
use postui::components::editor::SubFocus;
use postui::components::line_input::LineInput;
use postui::components::modal::{Modal, PromptKind};
use postui::components::varmanager::{VarEditOp, VarStructOp};
use postui::layout::PaneId;
use postui_core::model::Entry;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn render(app: &mut App) -> String {
    app.anims.finish_all();
    let mut terminal = Terminal::new(TestBackend::new(140, 44)).unwrap();
    terminal.draw(|f| postui::ui::draw(f, app)).unwrap();
    format!("{:?}", terminal.backend().buffer())
}

fn plain(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
}
fn enter() -> KeyEvent {
    KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)
}

/// Puts the caret inside the first occurrence of `token` in `url`, focuses
/// the editor's URL field — the state `ctrl+v`'s selection-picker redirect
/// (`App::selection_picker_target`) reads.
fn focus_url_with_cursor_on(app: &mut App, url: &str, token: &str) {
    let mid = url.find(token).unwrap() + token.len() / 2;
    let mut input = LineInput::new(url);
    input.set_cursor(mid);
    app.editor.url = input;
    app.focus = PaneId::Editor;
    app.editor.sub_focus = SubFocus::Url;
}

/// Drains `rx`, applying every action through `app.update` as the main loop
/// would, until the `ResponseArrived`/`RequestFailed` tagged `generation`
/// lands (same pattern as `stage3_acceptance.rs`).
async fn drain_until(
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<Action>,
    app: &mut App,
    generation: u64,
) {
    loop {
        let action = rx.recv().await.expect("a background task result");
        let done = matches!(
            &action,
            Action::ResponseArrived { generation: g, .. } | Action::RequestFailed { generation: g, .. }
                if *g == generation
        );
        app.update(action);
        if done {
            break;
        }
    }
}

/// End-to-end sweep: declares a simple variable, a one-field group with
/// an entry per environment, a two-field group with one entry, and a
/// secret — all through `Action::VarStruct`/`Action::VarEdit`, exactly as
/// the Manager and in-context flows dispatch them. Selects the one-field
/// group's entry through the real `{{`-token → `ctrl+v` picker path, switches
/// environments and checks the resolved values follow, overrides one
/// variable at request scope, and sends — hitting the send-time secret
/// prompt chain — to a wiremock server that only accepts the fully
/// substituted URL/headers, secret included.
#[tokio::test]
async fn stage6_acceptance_flow() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/orders/east-1"))
        .and(query_param("user", "1001"))
        .and(query_param("cust", "c-77"))
        .and(header("x-api-key", "sk-qa-999"))
        .and(header("x-trace", "req-trace-override"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"env": "qa"})))
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    postui_core::project::init_project(dir.path(), Some("acme")).unwrap();

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());

    // ------------------------------------------------------------------
    // Build the whole variable system through actions.
    // ------------------------------------------------------------------

    // A simple variable, its default never used directly (each env sets
    // its own base).
    app.update(Action::VarStruct(VarStructOp::NewVar {
        name: "base_url".into(),
        description: Some("API root".into()),
    }));

    // A one-field group — what an enumerated variable is in stage 7.
    app.update(Action::VarStruct(VarStructOp::NewGroup {
        name: "region".into(),
        fields: vec!["region".into()],
    }));

    // A group whose entries carry both fields at once.
    app.update(Action::VarStruct(VarStructOp::NewGroup {
        name: "creds".into(),
        fields: vec!["user_id".into(), "customer_id".into()],
    }));

    // A secret variable — declared plain, then flipped (spec §3's
    // off->on transition, no default ever committed).
    app.update(Action::VarStruct(VarStructOp::NewVar {
        name: "api_key".into(),
        description: Some("service key".into()),
    }));
    app.update(Action::VarStruct(VarStructOp::ToggleSecret {
        name: "api_key".into(),
    }));

    // A variable that gets overridden at request scope below.
    app.update(Action::VarStruct(VarStructOp::NewVar {
        name: "trace_id".into(),
        description: Some("trace header".into()),
    }));
    app.update(Action::VarEdit(VarEditOp::SetDefault {
        name: "trace_id".into(),
        value: "proj-trace".into(),
    }));

    assert!(
        app.toasts.is_empty(),
        "declarations must all succeed: {}",
        render(&mut app)
    );

    // ------------------------------------------------------------------
    // Environment values (also action-driven — `edit_env` creates the env
    // file lazily on first write, so no environments/*.toml is ever
    // hand-written).
    // ------------------------------------------------------------------

    app.update(Action::VarEdit(VarEditOp::SetEnvValue {
        env: "qa".into(),
        name: "base_url".into(),
        value: server.uri(),
    }));
    app.update(Action::VarEdit(VarEditOp::SetEnvValue {
        env: "prod".into(),
        name: "base_url".into(),
        value: server.uri(),
    }));

    // Entries belong to one environment each (spec §3.1), so each one is
    // written while its environment is the active one.
    app.update(Action::SwitchEnv(Some("prod".into())));
    let mut west = IndexMap::new();
    west.insert("region".to_string(), "west-9".to_string());
    app.update(Action::VarStruct(VarStructOp::NewEntry {
        env: "prod".into(),
        group: "region".into(),
        name: "west".into(),
        description: None,
        values: west,
    }));

    app.update(Action::SwitchEnv(Some("qa".into())));
    assert_eq!(app.project.active_env.as_deref(), Some("qa"));
    let mut east = IndexMap::new();
    east.insert("region".to_string(), "east-1".to_string());
    app.update(Action::VarStruct(VarStructOp::NewEntry {
        env: "qa".into(),
        group: "region".into(),
        name: "east".into(),
        description: None,
        values: east,
    }));
    let mut alice = IndexMap::new();
    alice.insert("user_id".to_string(), "1001".to_string());
    alice.insert("customer_id".to_string(), "c-77".to_string());
    app.update(Action::VarStruct(VarStructOp::NewEntry {
        env: "qa".into(),
        group: "creds".into(),
        name: "alice".into(),
        description: None,
        values: alice,
    }));
    assert!(
        postui_core::varmodel::group_entries(&app.project.env_data, "region")
            .is_some_and(|e| e.contains_key("east")),
        "qa's own entry landed"
    );
    assert!(
        postui_core::varmodel::group_entries(&app.project.env_data, "creds")
            .is_some_and(|e| e.contains_key("alice")),
        "the two-field group's entry landed too"
    );

    // ------------------------------------------------------------------
    // Create the request and wire up its URL/headers with every kind of
    // token: simple, enumerated, group members, secret.
    // ------------------------------------------------------------------

    app.update(Action::CreateRequest("orders".into()));
    assert_eq!(app.editor.slug.as_deref(), Some("orders"));

    let url = "{{base_url}}/orders/{{region}}?user={{user_id}}&cust={{customer_id}}";
    app.editor.url = LineInput::new(url);
    app.editor.headers.insert(
        "x-api-key".to_string(),
        Entry {
            value: "{{api_key}}".into(),
            enabled: true,
        },
    );
    app.editor.headers.insert(
        "x-trace".to_string(),
        Entry {
            value: "{{trace_id}}".into(),
            enabled: true,
        },
    );

    // --- select the enumerated variable via the real picker path ------
    focus_url_with_cursor_on(&mut app, url, "{{region}}");
    app.update(Action::OpenVarPicker { completing: false });
    assert!(
        matches!(app.modals.top(), Some(Modal::VarPicker(_))),
        "cursor on a group field's token opens the selection-context picker"
    );
    let keymap = postui::keys::Keymap::default_bindings();
    app.handle_key(&keymap, enter()); // qa declares one entry: east
    assert!(app.modals.is_empty(), "confirming closes the picker");
    assert_eq!(
        app.project.selections_for("qa")["region"],
        "east",
        "picker confirm recorded the selection"
    );

    // --- select the group's entry directly (VarEdit::SelectEntry is the
    // same wire type the picker itself dispatches on confirm) ----------
    app.update(Action::VarEdit(VarEditOp::SelectEntry {
        env: "qa".into(),
        group: "creds".into(),
        entry: "alice".into(),
    }));

    // --- request-scope override: `trace_id` shadows the project default
    app.update(Action::VarEdit(VarEditOp::SetRequestVar {
        name: "trace_id".into(),
        value: "req-trace-override".into(),
    }));
    assert_eq!(app.editor.variables["trace_id"].value, "req-trace-override");

    app.update(Action::SaveRequest);
    assert!(!app.editor.is_dirty(), "saved: editor is clean again");

    // ------------------------------------------------------------------
    // Environment switch flips resolved values (spec §1/§2): prod has no
    // selections yet, so region/creds read back as "needs a selection"
    // there, distinct from qa's fully-resolved picture.
    // ------------------------------------------------------------------

    app.update(Action::SwitchEnv(Some("prod".into())));
    assert_eq!(app.project.active_env.as_deref(), Some("prod"));
    assert!(
        !app.project.resolved.values.contains_key("region"),
        "prod has no region selection yet: {:?}",
        app.project.resolved.values.get("region")
    );
    app.update(Action::VarEdit(VarEditOp::SelectEntry {
        env: "prod".into(),
        group: "region".into(),
        entry: "west".into(),
    }));
    assert_eq!(app.project.resolved.values["region"], "west-9");

    app.update(Action::SwitchEnv(Some("qa".into())));
    assert_eq!(
        app.project.resolved.values["region"], "east-1",
        "switching back to qa restores qa's own resolved value"
    );

    // ------------------------------------------------------------------
    // Send: the secret is still unset, so the first `Send` must pause on
    // the masked send-time secret prompt (spec §3) rather than sending or
    // toasting an ordinary unresolved-variable error.
    // ------------------------------------------------------------------

    app.update(Action::Send);
    let Some(Modal::Prompt { kind, revealed, .. }) = app.modals.top() else {
        panic!("expected the send-time secret prompt");
    };
    assert_eq!(
        *kind,
        PromptKind::SecretValue {
            name: "api_key".into(),
            env: "qa".into(),
        }
    );
    assert!(!*revealed, "the secret prompt must render masked");
    assert!(
        app.session.in_flight.is_none(),
        "nothing sends while a secret is missing"
    );

    for c in "sk-qa-999".chars() {
        app.handle_key(&keymap, plain(c));
    }
    app.handle_key(&keymap, enter());

    assert!(app.modals.is_empty(), "confirming the secret closes it");
    let secrets = postui_core::project::load_secrets(dir.path()).unwrap();
    assert_eq!(secrets["qa"]["api_key"], "sk-qa-999");

    let generation = app.session.send_generation;
    assert!(
        app.session.in_flight.is_some(),
        "the secret prompt's confirm re-runs ForceSend, which now resolves"
    );
    drain_until(&mut rx, &mut app, generation).await;
    let frame = render(&mut app);
    assert!(frame.contains("200"), "response is Ready: {frame}");
    assert!(frame.contains("qa"), "response body visible: {frame}");

    // ------------------------------------------------------------------
    // On-disk write fidelity: variables.toml and both environment files
    // match exact expected text — every edit above was a surgical
    // `toml_edit` mutation, never a fresh serialize.
    // ------------------------------------------------------------------

    let variables_toml = std::fs::read_to_string(dir.path().join("variables.toml")).unwrap();
    assert_eq!(
        variables_toml,
        "[base_url]\n\
description = \"API root\"\n\
\n\
[groups.region]\n\
fields = [\"region\"]\n\
\n\
[groups.creds]\n\
fields = [\"user_id\", \"customer_id\"]\n\
\n\
[api_key]\n\
description = \"service key\"\n\
secret = true\n\
\n\
[trace_id]\n\
description = \"trace header\"\n\
default = \"proj-trace\"\n\
# Declare variables: [name] with optional description/default\n",
    );

    let qa_toml = std::fs::read_to_string(dir.path().join("environments/qa.toml")).unwrap();
    assert_eq!(
        qa_toml,
        format!(
            "base_url = \"{}\"\n\
\n\
[entries.region.east]\n\
region = \"east-1\"\n\
\n\
[entries.creds.alice]\n\
user_id = \"1001\"\n\
customer_id = \"c-77\"\n",
            server.uri()
        )
    );

    let prod_toml = std::fs::read_to_string(dir.path().join("environments/prod.toml")).unwrap();
    assert_eq!(
        prod_toml,
        format!(
            "base_url = \"{}\"\n\
\n\
[entries.region.west]\n\
region = \"west-9\"\n",
            server.uri()
        )
    );

    // The secret and selections live only under `.local/`, never in the
    // shareable files above.
    assert!(
        !qa_toml.contains("sk-qa-999") && !variables_toml.contains("sk-qa-999"),
        "the secret value must never land in a git-tracked file"
    );

    let request_toml = std::fs::read_to_string(dir.path().join("requests/orders.toml")).unwrap();
    assert!(
        request_toml.contains("req-trace-override"),
        "the request-scope override is saved on the request: {request_toml}"
    );
}

/// Write fidelity (spec §7): a hand-authored `variables.toml` — the one
/// file this suite is allowed to write by hand, per the task brief, since
/// its whole point is proving the Manager's edits are surgical — keeps its
/// comments, ordering, and every untouched entry through one Manager edit;
/// only the targeted key changes.
#[test]
fn variables_toml_comments_survive_one_manager_edit() {
    let dir = tempfile::tempdir().unwrap();
    postui_core::project::init_project(dir.path(), Some("acme")).unwrap();
    std::fs::write(
        dir.path().join("variables.toml"),
        "\
# Declared variables for this project — hand-edit freely, the Manager
# preserves whatever it doesn't touch.

[base_url] # the API root
description = \"API root\"
default = \"http://localhost:8080\" # local default, override per-env

# do not rename without updating the client SDK
[api_token]
description = \"legacy auth token\"
",
    )
    .unwrap();

    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    assert!(app.project.model.vars.contains_key("api_token"));

    app.update(Action::VarEdit(VarEditOp::SetDefault {
        name: "base_url".into(),
        value: "https://new.example.com".into(),
    }));
    assert!(app.toasts.is_empty(), "edit must succeed");

    let on_disk = std::fs::read_to_string(dir.path().join("variables.toml")).unwrap();
    assert!(
        on_disk.contains("# Declared variables for this project — hand-edit freely, the Manager"),
        "leading file comment survives: {on_disk}"
    );
    assert!(
        on_disk.contains("[base_url] # the API root"),
        "the edited table's own inline comment survives: {on_disk}"
    );
    assert!(
        on_disk.contains("default = \"https://new.example.com\""),
        "the edited value actually changed: {on_disk}"
    );
    assert!(
        !on_disk.contains("http://localhost:8080"),
        "the old value is gone: {on_disk}"
    );
    assert!(
        on_disk.contains("# do not rename without updating the client SDK"),
        "an untouched entry's comment survives: {on_disk}"
    );
    assert!(
        on_disk.contains("[api_token]") && on_disk.contains("description = \"legacy auth token\""),
        "the untouched entry itself is unchanged: {on_disk}"
    );
}

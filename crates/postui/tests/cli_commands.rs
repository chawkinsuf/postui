//! The commands (`postui projects`, `postui show`, ...), run for real in a
//! throwaway home: what they print, the exit status a script sees, and
//! that a read writes nothing (spec 2026-10-28 §4, R16).

mod common;

use common::*;
use std::path::Path;

#[test]
fn show_reports_paths_envs_and_spaces() {
    let home = Home::new();
    let root = write_project(&home, "api");
    let before = tree(&root);

    let out = home.run(&["show", "-p", s(&root)]);
    assert_ok(&out, "show");
    let v = json(&out);
    assert_eq!(v["name"], "Billing \"API\" {{x}} ✓");
    assert_eq!(v["root"], s(&root));
    assert_eq!(v["files"]["project"], s(&root.join("project.toml")));
    assert_eq!(v["files"]["variables"], s(&root.join("variables.toml")));
    assert_eq!(
        v["environments"],
        serde_json::json!([
            {"slug": "dev", "name": "Dev", "file": s(&root.join("environments/dev.toml")),
             "active": true},
            {"slug": "qa", "name": "qa", "file": s(&root.join("environments/qa.toml")),
             "active": false},
        ])
    );
    assert_eq!(
        v["spaces"],
        serde_json::json!([
            {"slug": "main", "name": "main", "dir": s(&root.join("requests/main")),
             "requests": 2},
            {"slug": "admin", "name": "Admin", "dir": s(&root.join("requests/admin")),
             "requests": 1},
        ])
    );
    assert_eq!(v["warnings"], serde_json::json!([]));

    let out = home.run(&["show", "-p", s(&root), "--text"]);
    assert_ok(&out, "show --text");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("Billing \"API\" {{x}} ✓") && text.contains("Admin"), "{text}");

    assert_eq!(tree(&root), before, "show writes nothing");
}

/// Review Focus 1: a broken environment that isn't the active one is
/// listed, not fatal.
#[test]
fn show_survives_a_broken_inactive_env() {
    let home = Home::new();
    let root = write_project(&home, "api");
    std::fs::write(root.join("environments/qa.toml"), "[[").unwrap();
    let before = tree(&root);

    let out = home.run(&["show", "-p", s(&root)]);
    assert_ok(&out, "show");
    let v = json(&out);
    let slugs: Vec<&str> =
        v["environments"].as_array().unwrap().iter().map(|e| e["slug"].as_str().unwrap()).collect();
    assert_eq!(slugs, ["dev", "qa"]);
    assert_eq!(tree(&root), before);
}

/// A broken active environment refuses the open, as the TUI does, naming
/// the file.
#[test]
fn show_refuses_a_broken_active_env() {
    let home = Home::new();
    let root = write_project(&home, "api");
    std::fs::write(root.join("environments/dev.toml"), "[[").unwrap();

    let out = home.run(&["show", "-p", s(&root)]);
    assert_eq!(out.status.code(), Some(2));
    let v = json(&out);
    assert_eq!(v["file"], s(&root.join("environments/dev.toml")));
}

#[test]
fn requests_in_sidebar_order_with_paths() {
    let home = Home::new();
    let root = write_project(&home, "api");
    // The space's order array puts `list` ahead of `alpha`: sidebar order,
    // not alphabetical.
    std::fs::write(
        root.join("requests/main/alpha.toml"),
        "name = \"Alpha\"\nmethod = \"GET\"\nurl = \"{{base}}/a\"\n",
    )
    .unwrap();
    let meta = std::fs::read_to_string(root.join("project.toml")).unwrap();
    std::fs::write(
        root.join("project.toml"),
        meta + "\n[space.main]\norder = [\"list\", \"alpha\"]\n",
    )
    .unwrap();
    let before = tree(&root);

    let out = home.run(&["requests", "-p", s(&root)]);
    assert_ok(&out, "requests");
    let v = json(&out);
    let slugs: Vec<&str> =
        v["requests"].as_array().unwrap().iter().map(|r| r["slug"].as_str().unwrap()).collect();
    assert_eq!(slugs, ["main/list", "main/alpha", "main/users/get-user", "admin/x"]);
    assert_eq!(
        v["requests"][2],
        serde_json::json!({
            "slug": "main/users/get-user", "space": "main", "name": "Get user",
            "method": "GET", "url": "{{base}}/users",
            "file": s(&root.join("requests/main/users/get-user.toml")), "error": null,
        })
    );
    assert_eq!(v["warnings"], serde_json::json!([]));

    let out = home.run(&["requests", "-p", s(&root), "--space", "admin"]);
    assert_ok(&out, "requests --space");
    let v = json(&out);
    assert_eq!(v["requests"].as_array().unwrap().len(), 1);
    assert_eq!(v["requests"][0]["slug"], "admin/x");

    let out = home.run(&["requests", "-p", s(&root), "--space", "nope"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(json(&out)["error"].as_str().unwrap().contains("nope"));

    let out = home.run(&["requests", "-p", s(&root), "--text"]);
    assert_ok(&out, "requests --text");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("main/users/get-user") && text.contains("{{base}}/users"), "{text}");

    assert_eq!(tree(&root), before, "requests writes nothing");
}

/// Review Focus 1: a request that doesn't parse is listed with what is
/// wrong.
#[test]
fn requests_reports_a_broken_file() {
    let home = Home::new();
    let root = write_project(&home, "api");
    std::fs::write(root.join("requests/main/list.toml"), "nope = [").unwrap();

    let out = home.run(&["requests", "-p", s(&root)]);
    assert_ok(&out, "requests");
    let v = json(&out);
    let broken = v["requests"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["slug"] == "main/list")
        .expect("the broken request is listed");
    assert!(broken["method"].is_null() && broken["url"].is_null() && broken["name"].is_null());
    assert!(broken["error"].as_str().is_some_and(|e| !e.is_empty()), "{broken}");
    assert_eq!(broken["file"], s(&root.join("requests/main/list.toml")));
}

/// The `variables` entry named `name`.
fn var<'a>(v: &'a serde_json::Value, name: &str) -> &'a serde_json::Value {
    v["variables"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["name"] == name)
        .unwrap_or_else(|| panic!("no variable {name} in {v}"))
}

#[test]
fn vars_reports_values_and_sources() {
    let home = Home::new();
    let root = write_project(&home, "api");
    let before = tree(&root);

    let out = home.run(&["vars", "-p", s(&root)]);
    assert_ok(&out, "vars");
    assert!(!String::from_utf8_lossy(&out.stdout).contains("s3cr3t-value"));
    let v = json(&out);
    assert_eq!(
        v["environment"],
        serde_json::json!({"slug": "dev", "name": "Dev",
                           "file": s(&root.join("environments/dev.toml"))})
    );
    assert_eq!(v["files"], serde_json::json!({"variables": s(&root.join("variables.toml"))}));
    assert_eq!(
        *var(&v, "base"),
        serde_json::json!({"name": "base", "description": "API root", "value": "https://dev",
                           "source": "env", "secret": false, "selector": null})
    );
    assert_eq!(var(&v, "region")["source"], "default");
    assert_eq!(var(&v, "region")["value"], "eu");
    assert_eq!(var(&v, "id")["source"], "unset");
    assert!(var(&v, "id")["value"].is_null());
    assert_eq!(var(&v, "user_id")["source"], "selector");
    assert_eq!(var(&v, "user_id")["selector"], "user");
    assert_eq!(var(&v, "user_id")["value"], "42");
    assert_eq!(var(&v, "token")["source"], "secret");
    assert_eq!(var(&v, "token")["secret"], true);
    assert_eq!(var(&v, "token")["value"], "●●●●●●");
    assert_eq!(
        v["selectors"],
        serde_json::json!([{
            "name": "user", "description": null, "shared": false, "fields": ["user_id"],
            "options": [{"name": "alice", "description": null, "values": {"user_id": "42"}}],
            "selected": "alice",
        }])
    );
    assert_eq!(v["warnings"], serde_json::json!([]));

    let out = home.run(&["vars", "-p", s(&root), "--text"]);
    assert_ok(&out, "vars --text");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("https://dev") && text.contains("●●●●●●"), "{text}");
    assert!(!text.contains("s3cr3t-value"), "{text}");

    assert_eq!(tree(&root), before, "vars writes nothing, state.toml included");
}

#[test]
fn vars_show_secrets_reveals() {
    let home = Home::new();
    let root = write_project(&home, "api");
    let out = home.run(&["vars", "-p", s(&root), "--show-secrets"]);
    assert_ok(&out, "vars --show-secrets");
    assert_eq!(var(&json(&out), "token")["value"], "s3cr3t-value");
    let out = home.run(&["vars", "-p", s(&root), "--show-secrets", "--text"]);
    assert!(String::from_utf8_lossy(&out.stdout).contains("s3cr3t-value"));
}

#[test]
fn vars_env_by_display_name_and_unknown() {
    let home = Home::new();
    let root = write_project(&home, "api");
    let meta = std::fs::read_to_string(root.join("project.toml")).unwrap();
    std::fs::write(root.join("project.toml"), meta + "\n[environment.qa]\nname = \"QA\"\n")
        .unwrap();
    let before = tree(&root);

    for name in ["QA", "qa"] {
        let out = home.run(&["vars", "-p", s(&root), "--env", name]);
        assert_ok(&out, name);
        let v = json(&out);
        assert_eq!(v["environment"]["slug"], "qa", "{name}");
        assert_eq!(var(&v, "base")["value"], "https://qa");
        assert_eq!(var(&v, "user_id")["source"], "needs_selection");
        assert!(var(&v, "user_id")["value"].is_null());
        assert_eq!(var(&v, "token")["source"], "missing_secret");
        assert!(var(&v, "token")["value"].is_null());
        assert_eq!(v["selectors"][0]["selected"], serde_json::Value::Null);
    }

    let out = home.run(&["vars", "-p", s(&root), "--env", "nope"]);
    assert_eq!(out.status.code(), Some(2));
    let error = json(&out)["error"].as_str().unwrap().to_string();
    assert!(error.contains("nope") && error.contains("dev, qa"), "{error}");

    assert_eq!(tree(&root), before, "picking an environment writes nothing");
}

/// A broken environment that was asked for is the error, naming its file.
#[test]
fn vars_env_with_a_broken_file_is_an_error() {
    let home = Home::new();
    let root = write_project(&home, "api");
    std::fs::write(root.join("environments/qa.toml"), "[[").unwrap();
    let out = home.run(&["vars", "-p", s(&root), "--env", "qa"]);
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(json(&out)["file"], s(&root.join("environments/qa.toml")));
}

/// A project with no environments still resolves its variables.
#[test]
fn vars_without_environments() {
    let home = Home::new();
    let root = write_project(&home, "api");
    std::fs::remove_dir_all(root.join("environments")).unwrap();
    std::fs::remove_dir_all(root.join(".postui")).unwrap();
    let before = tree(&root);

    let out = home.run(&["vars", "-p", s(&root)]);
    assert_ok(&out, "vars");
    let v = json(&out);
    assert!(v["environment"].is_null(), "{v}");
    assert_eq!(var(&v, "region")["source"], "default");
    assert_eq!(var(&v, "id")["source"], "unset");
    assert_eq!(var(&v, "user_id")["source"], "needs_selection");
    assert!(v["selectors"][0]["selected"].is_null());
    assert!(v["selectors"][0]["options"].as_array().unwrap().is_empty());

    let out = home.run(&["vars", "-p", s(&root), "--text"]);
    assert_ok(&out, "vars --text");
    let out = home.run(&["vars", "-p", s(&root), "--env", "dev"]);
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(tree(&root), before);
}

#[test]
fn not_a_project_is_an_error_envelope() {
    let home = Home::new();
    let empty = home.path().join("empty");
    std::fs::create_dir(&empty).unwrap();

    let out = home.run(&["show", "-p", s(&empty)]);
    assert_eq!(out.status.code(), Some(2));
    let v = json(&out);
    let error = v["error"].as_str().unwrap();
    assert!(error.contains("not a postui project"), "{error}");
    assert_eq!(v["file"], s(&empty.join("project.toml")));

    let out = home.run(&["show", "-p", s(&empty), "--text"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(out.stdout.is_empty(), "{}", String::from_utf8_lossy(&out.stdout));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.starts_with("postui: not a postui project"), "{stderr}");
}

/// Review Focus 3: a project's subdirectory is not the project; the error
/// says how to name it.
#[test]
fn subdirectory_of_a_project_is_not_a_project() {
    let home = Home::new();
    let proj = home.project("api", "API");
    let sub = proj.join("requests").join("main");
    std::fs::create_dir_all(&sub).unwrap();

    let out = home.run_in(&sub, &["show"], |_| {});
    assert_eq!(out.status.code(), Some(2));
    let v = json(&out);
    let error = v["error"].as_str().unwrap();
    assert!(error.contains("not a postui project") && error.contains("-p"), "{error}");
    assert_eq!(v["file"], s(&sub.join("project.toml")));
}

/// Review Focus 2: every spelling of one directory is one root, `..`
/// included (the natural answer to the "pass -p" hint from inside it).
#[test]
fn project_path_forms_resolve_alike() {
    let home = Home::new();
    let proj = home.project("api", "API");
    let main = proj.join("requests").join("main");
    std::fs::create_dir_all(&main).unwrap();
    let trailing = format!("{}/", s(&proj));
    let up = format!("{}/requests/..", s(&proj));
    for form in [s(&proj), &trailing, &up, "api", "./api", "api/", "~/api", "api/requests/.."] {
        let out = home.run(&["show", "-p", form]);
        assert_ok(&out, form);
        assert_eq!(json(&out)["root"], s(&proj), "{form}");
    }
    for (cwd, args) in [(&main, &["show", "-p", "../.."][..]), (&proj, &["show"])] {
        let out = home.run_in(cwd, args, |_| {});
        assert_ok(&out, &format!("{args:?} from {}", cwd.display()));
        assert_eq!(json(&out)["root"], s(&proj), "{args:?} from {}", cwd.display());
    }
}

#[test]
fn projects_lists_the_registry() {
    let home = Home::new();
    let proj = home.project("billing", "Billing API");
    let gone = home.path().join("gone");
    home.register(&[&proj, &gone]);
    let before = tree(&home.config_dir());

    let out = home.run(&["projects"]);
    assert_ok(&out, "projects");
    let v = json(&out);
    assert_eq!(
        v["projects"][0],
        serde_json::json!({"name": "Billing API", "root": s(&proj), "exists": true})
    );
    assert_eq!(v["projects"][1]["root"], s(&gone));
    assert_eq!(v["projects"][1]["exists"], false);
    assert_eq!(v["projects"].as_array().unwrap().len(), 2);
    let config_dir = v["config_dir"].as_str().unwrap();
    assert!(Path::new(config_dir).ends_with("config/postui"), "{config_dir}");

    let out = home.run(&["projects", "--text"]);
    assert_ok(&out, "projects --text");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("Billing API") && text.contains(s(&gone)), "{text}");

    assert_eq!(tree(&home.config_dir()), before, "the config dir is untouched");
}

#[test]
fn registered_name_resolves() {
    let home = Home::new();
    let proj = home.project("billing", "Billing API");
    home.register(&[&proj]);

    for name in ["Billing API", "billing api"] {
        let out = home.run(&["show", "-p", name]);
        assert_ok(&out, name);
        assert_eq!(json(&out)["root"], s(&proj), "{name}");
    }

    let out = home.run(&["show", "-p", "Payments"]);
    assert_eq!(out.status.code(), Some(2));
    let error = json(&out)["error"].as_str().unwrap().to_string();
    assert!(error.contains("no such directory or registered project \"Payments\""), "{error}");

    let twin = home.project("other", "Billing API");
    home.register(&[&proj, &twin]);
    let out = home.run(&["show", "-p", "Billing API"]);
    assert_eq!(out.status.code(), Some(2));
    let error = json(&out)["error"].as_str().unwrap().to_string();
    assert!(error.contains("names 2 registered projects"), "{error}");
    assert!(error.contains(s(&proj)) && error.contains(s(&twin)), "{error}");
}

/// `check`'s findings in `list` (`errors` or `warnings`) as
/// `(file, message)`, `file` empty when the finding names none.
fn findings(v: &serde_json::Value, list: &str) -> Vec<(String, String)> {
    v[list]
        .as_array()
        .unwrap_or_else(|| panic!("no {list} in {v}"))
        .iter()
        .map(|f| {
            let file = f["file"].as_str().unwrap_or_default().to_string();
            (file, f["message"].as_str().unwrap().to_string())
        })
        .collect()
}

#[test]
fn check_clean_project_is_ok() {
    let home = Home::new();
    let root = write_project(&home, "api");
    let before = tree(&root);

    let out = home.run(&["check", "-p", s(&root)]);
    assert_ok(&out, "check");
    let v = json(&out);
    assert_eq!(
        v,
        serde_json::json!({"ok": true, "environment": "dev", "errors": [], "warnings": []})
    );

    let out = home.run(&["check", "-p", s(&root), "--text"]);
    assert_ok(&out, "check --text");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("dev") && text.contains("ok"), "{text}");

    assert_eq!(tree(&root), before, "check writes nothing");
}

/// Review Focus 1: each broken file is an error naming that file, and the
/// rest of the project is still checked.
#[test]
fn check_reports_each_broken_file() {
    let cases: [(&str, &str); 3] = [
        ("requests/main/list.toml", "nope = ["),
        ("environments/qa.toml", "[["),
        // The active environment's option lacks the selector's field.
        ("environments/dev.toml", "base = \"https://dev\"\n\n[options.user.alice]\nnote = \"x\"\n"),
    ];
    for (rel, text) in cases {
        let home = Home::new();
        let root = write_project(&home, "api");
        std::fs::write(root.join(rel), text).unwrap();
        let before = tree(&root);

        let out = home.run(&["check", "-p", s(&root)]);
        assert_eq!(out.status.code(), Some(1), "{rel}: {}", String::from_utf8_lossy(&out.stdout));
        let v = json(&out);
        assert_eq!(v["ok"], false, "{rel}");
        let errors = findings(&v, "errors");
        let file = s(&root.join(rel)).to_string();
        assert!(errors.iter().any(|(f, m)| *f == file && !m.is_empty()), "{rel}: {errors:?}");
        assert_eq!(tree(&root), before, "{rel}: check writes nothing");
    }

    // Asking for the broken environment reports it once and checks the
    // rest; an environment that doesn't exist is a mistake on the line.
    let home = Home::new();
    let root = write_project(&home, "api");
    std::fs::write(root.join("environments/qa.toml"), "[[").unwrap();
    std::fs::write(root.join("requests/main/list.toml"), "nope = [").unwrap();
    let out = home.run(&["check", "-p", s(&root), "--env", "qa"]);
    assert_eq!(out.status.code(), Some(1));
    let errors = findings(&json(&out), "errors");
    let qa = s(&root.join("environments/qa.toml")).to_string();
    assert_eq!(errors.iter().filter(|(f, _)| *f == qa).count(), 1, "{errors:?}");
    let list = s(&root.join("requests/main/list.toml")).to_string();
    assert!(errors.iter().any(|(f, _)| *f == list), "{errors:?}");

    let out = home.run(&["check", "-p", s(&root), "--env", "nope"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(json(&out)["error"].as_str().unwrap().contains("nope"));
}

/// A project that won't open is a finding on the file that refused it,
/// not the error envelope.
#[test]
fn check_reports_an_unopenable_project() {
    let home = Home::new();
    let root = write_project(&home, "api");
    std::fs::write(root.join("variables.toml"), "[[").unwrap();

    let out = home.run(&["check", "-p", s(&root)]);
    assert_eq!(out.status.code(), Some(1));
    let v = json(&out);
    assert_eq!(v["ok"], false);
    let errors = findings(&v, "errors");
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert_eq!(errors[0].0, s(&root.join("variables.toml")));

    let out = home.run(&["check", "-p", s(&root), "--text"]);
    assert_eq!(out.status.code(), Some(1));
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains(s(&root.join("variables.toml"))), "{text}");

    // Not a project at all is still the envelope.
    let empty = home.path().join("empty");
    std::fs::create_dir(&empty).unwrap();
    let out = home.run(&["check", "-p", s(&empty)]);
    assert_eq!(out.status.code(), Some(2));
    assert!(json(&out)["error"].as_str().unwrap().contains("not a postui project"));
}

#[test]
fn check_undeclared_is_error_missing_secret_is_warning() {
    let home = Home::new();
    let root = write_project(&home, "api");
    let x = root.join("requests/admin/x.toml");
    let file = s(&x).to_string();
    std::fs::write(&x, "name = \"X\"\nmethod = \"DELETE\"\nurl = \"{{bsae}}/x\"\n").unwrap();

    let out = home.run(&["check", "-p", s(&root)]);
    assert_eq!(out.status.code(), Some(1));
    let errors = findings(&json(&out), "errors");
    assert_eq!(errors, [(file.clone(), "{{bsae}} not declared".to_string())]);

    // A secret this checkout lacks, or a variable this environment gives
    // no value, is a warning: the run would fail, the files are fine.
    std::fs::write(&x, "name = \"X\"\nmethod = \"DELETE\"\nurl = \"{{base}}/{{token}}/{{id}}\"\n")
        .unwrap();
    std::fs::remove_file(root.join(".postui/secrets.toml")).unwrap();
    let out = home.run(&["check", "-p", s(&root)]);
    assert_ok(&out, "check");
    let v = json(&out);
    assert_eq!(v["ok"], true);
    let warnings = findings(&v, "warnings");
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert_eq!(warnings[0].0, file);
    assert!(warnings[0].1.contains("missing secret: token"), "{warnings:?}");
    assert!(warnings[0].1.contains("unresolved: id"), "{warnings:?}");

    std::fs::write(&x, "name = \"X\"\nmethod = \"DELETE\"\nurl = \"{{base}}/{{user_id}}\"\n")
        .unwrap();
    let out = home.run(&["check", "-p", s(&root), "--env", "qa"]);
    assert_ok(&out, "check --env qa");
    let v = json(&out);
    assert_eq!(v["environment"], "qa");
    let warnings = findings(&v, "warnings");
    assert_eq!(warnings, [(file, "need a selection: user_id".to_string())]);
}

/// Run and suite can't mask a secret under 4 characters, so `check` says
/// so, naming the secret and never its value.
#[test]
fn check_warns_about_a_secret_too_short_to_mask() {
    let home = Home::new();
    let root = write_project(&home, "api");
    std::fs::write(root.join(".postui/secrets.toml"), "[dev]\ntoken = \"abc\"\n").unwrap();

    let out = home.run(&["check", "-p", s(&root)]);
    assert_ok(&out, "check");
    let v = json(&out);
    assert_eq!(v["ok"], true);
    let warnings = findings(&v, "warnings");
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert_eq!(warnings[0].0, s(&root.join(".postui/secrets.toml")));
    assert_eq!(
        warnings[0].1,
        "secret token is shorter than 4 characters, so run and suite output can't mask it"
    );
    assert!(!warnings[0].1.contains("abc"), "{warnings:?}");
    assert!(!String::from_utf8_lossy(&out.stdout).contains("\"abc\""));
}

/// A name only a disabled request variable gives is not a typo: the error
/// says the variable is switched off.
#[test]
fn check_disabled_request_variable_is_not_declared_wording() {
    let home = Home::new();
    let root = write_project(&home, "api");
    let x = root.join("requests/admin/x.toml");
    std::fs::write(
        &x,
        "name = \"X\"\nmethod = \"DELETE\"\nurl = \"{{base}}/{{x}}\"\n\n\
         [variables]\nx = { value = \"1\", enabled = false }\n",
    )
    .unwrap();

    let out = home.run(&["check", "-p", s(&root)]);
    assert_eq!(out.status.code(), Some(1));
    let errors = findings(&json(&out), "errors");
    assert_eq!(
        errors,
        [(s(&x).to_string(), "{{x}} is disabled in the request's [variables]".to_string())]
    );
}

/// Spec §4: an invalid name in `project.toml` is an error on that file,
/// not also the open's warning.
#[test]
fn check_invalid_name_is_an_error_on_project_toml() {
    let home = Home::new();
    let root = write_project(&home, "api");
    let meta = std::fs::read_to_string(root.join("project.toml")).unwrap();
    std::fs::write(
        root.join("project.toml"),
        meta.replace(
            "spaces = [\"main\", \"admin\"]",
            "spaces = [\"main\", \"Bad Name\"]\nenvironments = [\"dev\", \"Bad Env\"]",
        ),
    )
    .unwrap();

    let out = home.run(&["check", "-p", s(&root)]);
    assert_eq!(out.status.code(), Some(1));
    let v = json(&out);
    let errors = findings(&v, "errors");
    let project_toml = s(&root.join("project.toml")).to_string();
    for name in ["Bad Name", "Bad Env"] {
        assert!(errors.iter().any(|(f, m)| *f == project_toml && m.contains(name)), "{errors:?}");
    }
    let warnings = findings(&v, "warnings");
    assert!(!warnings.iter().any(|(_, m)| m.contains("Bad ")), "{warnings:?}");
}

/// Spec §4: an environment file or space directory whose name postui
/// can't use is an error on it, not passed over; what is inside is not
/// checked, and the walk's warnings about it are not repeated.
#[test]
fn check_invalid_file_and_dir_names_are_errors() {
    let home = Home::new();
    let root = write_project(&home, "api");
    std::fs::write(root.join("environments/Staging.toml"), "base = [").unwrap();
    let out = home.run(&["check", "-p", s(&root)]);
    assert_eq!(out.status.code(), Some(1), "{}", String::from_utf8_lossy(&out.stdout));
    let v = json(&out);
    let staging = s(&root.join("environments/Staging.toml")).to_string();
    let errors = findings(&v, "errors");
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert_eq!(errors[0].0, staging);
    assert!(errors[0].1.contains("not a valid environment name"), "{errors:?}");
    std::fs::remove_file(root.join("environments/Staging.toml")).unwrap();

    std::fs::create_dir_all(root.join("requests/Main")).unwrap();
    std::fs::write(
        root.join("requests/Main/x.toml"),
        "name = \"X\"\nmethod = \"GET\"\nurl = \"{{nope}}\"\n",
    )
    .unwrap();
    let out = home.run(&["check", "-p", s(&root)]);
    assert_eq!(out.status.code(), Some(1), "{}", String::from_utf8_lossy(&out.stdout));
    let v = json(&out);
    let errors = findings(&v, "errors");
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert_eq!(errors[0].0, s(&root.join("requests/Main")));
    assert!(errors[0].1.contains("not a valid space name"), "{errors:?}");
    assert_eq!(
        findings(&v, "warnings"),
        [],
        "the walk's warning about Main/x.toml is not repeated"
    );

    // A request outside any space is still a warning, now on its file.
    std::fs::remove_dir_all(root.join("requests/Main")).unwrap();
    std::fs::write(root.join("requests/loose.toml"), "name = \"L\"\n").unwrap();
    let out = home.run(&["check", "-p", s(&root)]);
    assert_ok(&out, "check");
    let warnings = findings(&json(&out), "warnings");
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert_eq!(warnings[0].0, s(&root.join("requests/loose.toml")));
}

/// A hidden name (an editor's lock file, macOS's `._` twin, a dot-dir) is
/// no one's mistake: `check` passes over it and what is inside it.
#[test]
fn check_passes_over_hidden_files_and_dirs() {
    let home = Home::new();
    let root = write_project(&home, "api");
    std::fs::write(root.join("environments/.#dev.toml"), "base = [").unwrap();
    std::fs::write(root.join("environments/._dev.toml"), "\0\u{1}junk").unwrap();
    std::fs::create_dir_all(root.join("requests/.drafts")).unwrap();
    std::fs::write(
        root.join("requests/.drafts/x.toml"),
        "name = \"X\"\nmethod = \"GET\"\nurl = \"{{nope}}\"\n",
    )
    .unwrap();

    let out = home.run(&["check", "-p", s(&root)]);
    assert_ok(&out, "check");
    let v = json(&out);
    assert_eq!(v["ok"], true);
    for list in ["errors", "warnings"] {
        assert_eq!(findings(&v, list), [], "{list}");
    }
}

/// A `secrets.toml` that won't parse is named, but nothing of its text is
/// printed: the parser's message quotes the line, secret value and all.
#[test]
fn a_broken_secrets_file_never_prints_its_values() {
    let home = Home::new();
    let root = write_project(&home, "api");
    let secret = "sk-live-SUPERSECRET";
    for text in [format!("[dev]\ntoken = \"{secret}\n"), format!("token = \"{secret}\"\n")] {
        std::fs::write(root.join(".postui/secrets.toml"), &text).unwrap();
        for command in ["check", "show", "vars", "requests"] {
            for text_flag in [false, true] {
                let mut args = vec![command, "-p", s(&root)];
                if text_flag {
                    args.push("--text");
                }
                let out = home.run(&args);
                let printed = format!(
                    "{}{}",
                    String::from_utf8_lossy(&out.stdout),
                    String::from_utf8_lossy(&out.stderr)
                );
                assert!(!printed.contains(secret), "{args:?}: {printed}");
                assert!(printed.contains("secrets.toml"), "{args:?} names the file: {printed}");
                let status = if command == "check" { 1 } else { 2 };
                assert_eq!(out.status.code(), Some(status), "{args:?}: {printed}");
            }
        }
        let out = home.run(&["check", "-p", s(&root)]);
        let errors = findings(&json(&out), "errors");
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert_eq!(errors[0].0, s(&root.join(".postui/secrets.toml")));
    }
}

/// R16: no read command writes anything, in the project (its trash and a
/// missing `environments/` included) or the config dir.
#[test]
fn every_read_command_writes_nothing() {
    let home = Home::new();
    let root = write_project(&home, "api");
    std::fs::remove_dir_all(root.join("environments")).unwrap();
    std::fs::create_dir_all(root.join(".postui/trash/1")).unwrap();
    std::fs::write(root.join(".postui/trash/1/x.toml"), "name = \"gone\"\n").unwrap();
    home.register(&[&root]);
    let project = tree(&root);
    let config = tree(&home.config_dir());

    for args in [
        &["projects"][..],
        &["show", "-p", s(&root)],
        &["requests", "-p", s(&root)],
        &["vars", "-p", s(&root)],
        &["check", "-p", s(&root)],
    ] {
        for text in [false, true] {
            let mut args = args.to_vec();
            if text {
                args.push("--text");
            }
            let out = home.run(&args);
            assert_ne!(
                out.status.code(),
                Some(2),
                "{args:?}: {}",
                String::from_utf8_lossy(&out.stdout)
            );
            assert_eq!(tree(&root), project, "{args:?} writes nothing in the project");
            assert_eq!(tree(&home.config_dir()), config, "{args:?} writes nothing in the config");
        }
    }
}

/// Review Focus 5: a reader that stopped ends the run quietly.
#[test]
fn closed_stdout_ends_quietly() {
    let home = Home::new();
    let proj = home.project("api", "API");
    for args in [&["projects"][..], &["show", "-p", s(&proj)]] {
        let (reader, writer) = std::io::pipe().unwrap();
        drop(reader);
        let out = home.run_in(home.path(), args, |c| {
            c.stdout(writer);
        });
        assert_eq!(out.status.code(), Some(0), "{args:?}");
        assert!(out.stderr.is_empty(), "{args:?}: {}", String::from_utf8_lossy(&out.stderr));
    }
}

/// A command's help is its own usage on stdout; a mistake is the error
/// envelope on stdout, or with `--text` the reason and the command's own
/// usage on stderr; status 2 either way (spec 2026-10-28 §5).
#[test]
fn command_help_and_mistakes() {
    let home = Home::new();
    for (args, usage) in [
        (&["help", "vars"][..], "usage: postui vars"),
        (&["vars", "-h"], "usage: postui vars"),
        (&["show", "--help"], "usage: postui show"),
        (&["help"], "usage: postui"),
    ] {
        let out = home.run(args);
        assert_ok(&out, &format!("{args:?}"));
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(stdout.starts_with(usage), "{args:?}: {stdout}");
    }
    let out = home.run(&["show", "--bogus"]);
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(json(&out), serde_json::json!({"error": "unknown option '--bogus'"}));
    assert!(out.stderr.is_empty(), "{}", String::from_utf8_lossy(&out.stderr));

    let out = home.run(&["show", "--bogus", "--text"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(out.stdout.is_empty(), "{}", String::from_utf8_lossy(&out.stdout));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.starts_with("postui: unknown option '--bogus'\n\n"), "{stderr}");
    assert!(stderr.contains("usage: postui show "), "the command's own usage: {stderr}");
    assert!(!stderr.contains("commands ("), "not the general usage: {stderr}");
}

#[test]
fn init_makes_a_checkable_project() {
    let home = Home::new();
    let root = home.path().join("kit");
    let before = home.config_dir().exists();
    let out = home.run(&["init", s(&root), "--name", "Kit"]);
    assert_ok(&out, "init");
    let v = json(&out);
    assert_eq!(v["root"], s(&root));
    assert_eq!(v["warnings"], serde_json::json!([]));
    for (key, rel) in [
        ("project", "project.toml"),
        ("variables", "variables.toml"),
        ("environments", "environments"),
        ("requests", "requests"),
    ] {
        assert_eq!(v["files"][key], s(&root.join(rel)));
        assert!(root.join(rel).exists(), "{key} exists");
    }
    let out = home.run(&["check", "-p", s(&root)]);
    assert_ok(&out, "check");
    assert_eq!(json(&out)["ok"], true);
    assert_eq!(json(&home.run(&["show", "-p", s(&root)]))["name"], "Kit");
    assert_eq!(home.config_dir().exists(), before, "init does not register the project");
}

#[test]
fn init_refuses_an_existing_project() {
    let home = Home::new();
    let root = home.project("kit", "Kit");
    let before = std::fs::read(root.join("project.toml")).unwrap();
    let out = home.run(&["init", s(&root), "--name", "Other"]);
    assert_eq!(out.status.code(), Some(2));
    let v = json(&out);
    assert!(v["error"].as_str().unwrap().contains("already a postui project"), "{v}");
    assert_eq!(std::fs::read(root.join("project.toml")).unwrap(), before);
}

#[test]
fn init_creates_a_missing_directory() {
    let home = Home::new();
    let root = home.path().join("a").join("b");
    let out = home.run(&["init", s(&root)]);
    assert_ok(&out, "init");
    assert_eq!(json(&out)["root"], s(&root));
    assert!(root.join("project.toml").is_file());
    assert_ok(&home.run(&["check", "-p", s(&root)]), "check");
}

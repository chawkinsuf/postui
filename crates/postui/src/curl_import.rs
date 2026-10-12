//! One curl import in progress (spec curl §4.2–4.3): the parsed
//! commands, where they go, and what each name prompt made of them. It
//! rides inside the prompt's `PromptKind` and the actions the prompt
//! dispatches, so a prompt closed any way at all leaves nothing behind.

use crate::components::toast::ToastKind;
use indexmap::IndexSet;
use postui_core::curl::{CurlBatch, CurlImport};

/// What one Enter created.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreatedImport {
    /// The request's display name as created.
    pub name: String,
    pub ignored: Vec<String>,
    pub converted: Vec<String>,
    /// Variables the request uses, declared as secrets, and the value
    /// saved with each: `None` when the paste gave none, so the secret
    /// has none yet.
    pub secrets: Vec<(String, Option<String>)>,
    /// Variables the request uses that the project already had (perhaps
    /// from an earlier command of this run), left exactly as they were,
    /// and whether the project's value in the active environment is the
    /// paste's: when not, the request sends something else.
    pub kept: Vec<(String, bool)>,
    /// Variables the request uses under a name no variable may have
    /// (`entries`): not saved, so the request leaves them unresolved.
    pub reserved: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CurlImportRun {
    /// The folder slug the requests go into (`main/api`); `None` is the
    /// active space's root.
    pub folder: Option<String>,
    pub commands: Vec<CurlImport>,
    /// The command the open prompt is for.
    pub index: usize,
    pub created: Vec<CreatedImport>,
    pub skipped_lines: usize,
    /// `CurlBatch::values_dropped`.
    pub values_dropped: bool,
    /// The environment the secrets went into, as its display name; `None`
    /// when none was active.
    pub secrets_env: Option<String>,
}

impl CurlImportRun {
    pub fn new(batch: CurlBatch, folder: Option<String>) -> Self {
        Self {
            folder,
            commands: batch.commands,
            index: 0,
            created: Vec::new(),
            skipped_lines: batch.skipped_lines,
            values_dropped: batch.values_dropped,
            secrets_env: None,
        }
    }

    pub fn current(&self) -> &CurlImport {
        &self.commands[self.index]
    }

    pub fn title(&self) -> String {
        if self.commands.len() == 1 {
            "Import curl".to_string()
        } else {
            format!("Import curl ({} of {})", self.index + 1, self.commands.len())
        }
    }

    /// The toast after the last prompt (spec §4.3); `None` when nothing was
    /// created or there is nothing to report.
    pub fn summary(&self) -> Option<(String, ToastKind)> {
        if self.created.is_empty() {
            return None;
        }
        let total = self.commands.len();
        let mut parts = Vec::new();
        if total > 1 || self.created.len() < total {
            parts.push(format!("imported {} of {total}", self.created.len()));
        }
        let ignored: Vec<String> = self
            .created
            .iter()
            .filter(|c| !c.ignored.is_empty())
            .map(|c| {
                let flags = c.ignored.join(", ");
                if total > 1 { format!("{}: {flags}", c.name) } else { flags }
            })
            .collect();
        if !ignored.is_empty() {
            parts.push(format!("ignored: {}", ignored.join("; ")));
        }
        let converted = unique(self.created.iter().flat_map(|c| &c.converted));
        if !converted.is_empty() {
            let list: Vec<String> = converted.iter().map(|n| format!("${n} \u{2192} {{{{{n}}}}}")).collect();
            parts.push(format!("converted {}", list.join(", ")));
        }
        let secrets = || self.created.iter().flat_map(|c| &c.secrets);
        let saved = unique(secrets().filter(|(_, v)| v.is_some()).map(|(n, _)| n));
        if !saved.is_empty() {
            let what = if saved.len() == 1 { "a secret" } else { "secrets" };
            let mut part = format!("saved {} as {what}", saved.join(", "));
            if let Some(env) = &self.secrets_env {
                part.push_str(&format!(" in {env}"));
            }
            parts.push(part);
        }
        // Send refuses these until someone fills them in.
        let empty = unique(secrets().filter(|(_, v)| v.is_none()).map(|(n, _)| n));
        if !empty.is_empty() {
            let what = if empty.len() == 1 { "a secret" } else { "secrets" };
            parts.push(format!("declared {} as {what} with no value", empty.join(", ")));
        }
        // A name an earlier command of this run saved is no news when the
        // later command gave it the same value. Given another, or when the
        // project's own value is not the paste's, the request sends
        // something other than what the curl sent.
        let by_this_run = unique(secrets().map(|(n, _)| n));
        let all_kept: Vec<&(String, bool)> = self.created.iter().flat_map(|c| &c.kept).collect();
        let kept_where = |same: bool, by_run: bool| {
            unique(
                all_kept
                    .iter()
                    .filter(|(n, s)| *s == same && by_this_run.contains(&n.as_str()) == by_run)
                    .map(|(n, _)| n),
            )
        };
        let differs = kept_where(false, false);
        let mut kept = kept_where(true, false);
        kept.retain(|n| !differs.contains(n));
        if !kept.is_empty() {
            parts.push(format!("kept the existing {}", kept.join(", ")));
        }
        if !differs.is_empty() {
            parts.push(format!("kept the existing {}, not the paste's value", differs.join(", ")));
        }
        let (unsaved, changed): (Vec<&str>, Vec<&str>) =
            kept_where(false, true).into_iter().partition(|n| empty.contains(n));
        if !changed.is_empty() {
            parts.push(format!("kept the first value of {}", changed.join(", ")));
        }
        // An earlier command declared it with no value: the later value
        // went nowhere.
        if !unsaved.is_empty() {
            parts.push(format!("did not save a later value of {}", unsaved.join(", ")));
        }
        let reserved = unique(self.created.iter().flat_map(|c| &c.reserved));
        if !reserved.is_empty() {
            let why = if reserved.len() == 1 { "a reserved name" } else { "reserved names" };
            parts.push(format!("did not save {} ({why})", reserved.join(", ")));
        }
        if self.values_dropped {
            parts.push("saved no values: the paste has other shell lines".to_string());
        }
        if self.skipped_lines > 0 {
            let s = if self.skipped_lines == 1 { "" } else { "s" };
            parts.push(format!("skipped {} non-curl line{s}", self.skipped_lines));
        }
        if parts.is_empty() {
            return None;
        }
        let kind = if !ignored.is_empty()
            || !empty.is_empty()
            || !differs.is_empty()
            || !changed.is_empty()
            || !unsaved.is_empty()
            || !reserved.is_empty()
            || self.skipped_lines > 0
        {
            ToastKind::Warning
        } else {
            ToastKind::Success
        };
        Some((parts.join("; "), kind))
    }
}

/// `names` in first-seen order, each once.
fn unique<'a>(names: impl Iterator<Item = &'a String>) -> Vec<&'a str> {
    names.map(String::as_str).collect::<IndexSet<_>>().into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(text: &str) -> CurlImportRun {
        CurlImportRun::new(postui_core::curl::parse(text).unwrap(), None)
    }

    fn created(name: &str, ignored: &[&str], converted: &[&str]) -> CreatedImport {
        CreatedImport {
            name: name.into(),
            ignored: ignored.iter().map(|s| s.to_string()).collect(),
            converted: converted.iter().map(|s| s.to_string()).collect(),
            secrets: Vec::new(),
            kept: Vec::new(),
            reserved: Vec::new(),
        }
    }

    fn pairs(v: &[(&str, &str)]) -> Vec<(String, Option<String>)> {
        v.iter().map(|(n, v)| (n.to_string(), Some(v.to_string()))).collect()
    }

    fn kept(v: &[(&str, bool)]) -> Vec<(String, bool)> {
        v.iter().map(|(n, same)| (n.to_string(), *same)).collect()
    }

    #[test]
    fn saved_secrets_name_their_environment_and_kept_names_are_reported() {
        let mut r = run("curl https://x/a\ncurl https://x/b");
        r.secrets_env = Some("dev".into());
        let mut a = created("GET a", &[], &["T", "K"]);
        a.secrets = pairs(&[("T", "1")]);
        a.kept = kept(&[("K", true)]);
        let mut b = created("GET b", &[], &["T", "U"]);
        b.secrets = pairs(&[("U", "u")]);
        // `a` saved T with this value, so `b` finding it declared is no news.
        b.kept = kept(&[("T", true)]);
        r.created.extend([a, b]);
        assert_eq!(
            r.summary(),
            Some((
                "imported 2 of 2; converted $T \u{2192} {{T}}, $K \u{2192} {{K}}, $U \u{2192} {{U}}; saved T, U as secrets in dev; kept the existing K".into(),
                ToastKind::Success
            ))
        );
        let mut one = run("curl https://x/a");
        one.secrets_env = None;
        let mut a = created("GET a", &[], &["T"]);
        a.secrets = pairs(&[("T", "1")]);
        one.created.push(a);
        assert_eq!(one.summary().unwrap().0, "converted $T \u{2192} {{T}}; saved T as a secret");
    }

    #[test]
    fn a_later_value_the_first_one_shadows_is_a_warning() {
        let mut r = run("curl https://x/a\ncurl https://x/b");
        let mut a = created("GET a", &[], &["T"]);
        a.secrets = pairs(&[("T", "1")]);
        let mut b = created("GET b", &[], &["T"]);
        b.kept = kept(&[("T", false)]);
        r.created.extend([a, b]);
        assert_eq!(
            r.summary(),
            Some((
                "imported 2 of 2; converted $T \u{2192} {{T}}; saved T as a secret; kept the first value of T".into(),
                ToastKind::Warning
            ))
        );
    }

    #[test]
    fn a_project_value_that_is_not_the_pastes_is_a_warning() {
        let mut r = run("curl https://x/a\ncurl https://x/b");
        let mut a = created("GET a", &[], &["K", "T"]);
        a.kept = kept(&[("K", true), ("T", false)]);
        // Once it differs, the name is reported only as differing.
        let mut b = created("GET b", &[], &["K"]);
        b.kept = kept(&[("K", false)]);
        r.created.push(a.clone());
        assert_eq!(
            r.summary(),
            Some((
                "imported 1 of 2; converted $K \u{2192} {{K}}, $T \u{2192} {{T}}; kept the existing K; kept the existing T, not the paste's value".into(),
                ToastKind::Warning
            ))
        );
        r.created.push(b);
        assert!(r.summary().unwrap().0.ends_with("; kept the existing T, K, not the paste's value"));
    }

    #[test]
    fn a_secret_with_no_value_is_a_warning() {
        let mut r = run("curl https://x/a\ncurl https://x/b");
        r.secrets_env = Some("dev".into());
        let mut a = created("GET a", &[], &["T", "S"]);
        a.secrets = vec![("T".into(), Some("1".into())), ("S".into(), None)];
        // `b` knew S, but `a` declared it first: still no value, and the
        // toast says so.
        let mut b = created("GET b", &[], &["S"]);
        b.kept = kept(&[("S", false)]);
        r.created.extend([a, b]);
        assert_eq!(
            r.summary(),
            Some((
                "imported 2 of 2; converted $T \u{2192} {{T}}, $S \u{2192} {{S}}; saved T as a secret in dev; declared S as a secret with no value; did not save a later value of S".into(),
                ToastKind::Warning
            ))
        );
    }

    #[test]
    fn a_reserved_name_is_not_saved_and_warns() {
        let mut r = run("curl https://x/a");
        let mut a = created("GET a", &[], &["entries"]);
        a.reserved = vec!["entries".into()];
        r.created.push(a);
        assert_eq!(
            r.summary(),
            Some((
                "converted $entries \u{2192} {{entries}}; did not save entries (a reserved name)".into(),
                ToastKind::Warning
            ))
        );
    }

    #[test]
    fn titles_count_only_in_a_batch() {
        assert_eq!(run("curl https://x").title(), "Import curl");
        let mut r = run("curl https://x/a\ncurl https://x/b\ncurl https://x/c");
        assert_eq!(r.title(), "Import curl (1 of 3)");
        r.index = 2;
        assert_eq!(r.title(), "Import curl (3 of 3)");
    }

    #[test]
    fn one_clean_import_says_nothing_more() {
        let mut r = run("curl https://x/a");
        assert_eq!(r.summary(), None);
        r.created.push(created("GET a", &[], &[]));
        assert_eq!(r.summary(), None);
    }

    #[test]
    fn one_import_with_ignored_flags_warns_without_a_name() {
        let mut r = run("curl https://x/a");
        r.created.push(created("GET a", &["--compressed", "-L"], &[]));
        assert_eq!(r.summary(), Some(("ignored: --compressed, -L".into(), ToastKind::Warning)));
    }

    #[test]
    fn a_batch_reports_count_names_conversions_and_skips() {
        let mut r = run("echo hi\ncurl https://x/a\ncurl https://x/b\ncurl https://x/c");
        r.created.push(created("GET a", &["--compressed"], &["T"]));
        r.created.push(created("GET c", &[], &["T", "U"]));
        assert_eq!(
            r.summary(),
            Some((
                "imported 2 of 3; ignored: GET a: --compressed; converted $T \u{2192} {{T}}, $U \u{2192} {{U}}; skipped 1 non-curl line".into(),
                ToastKind::Warning
            ))
        );
    }

    #[test]
    fn values_a_skipped_line_dropped_are_reported_before_the_skip() {
        let mut r = run("export T=1\nif x; then :; fi\ncurl -H \"A: $T\" https://x/a");
        assert!(r.values_dropped);
        let mut a = created("GET a", &[], &["T"]);
        a.secrets = vec![("T".into(), None)];
        r.created.push(a);
        assert_eq!(
            r.summary(),
            Some((
                "converted $T \u{2192} {{T}}; declared T as a secret with no value; saved no values: the paste has other shell lines; skipped 3 non-curl lines".into(),
                ToastKind::Warning
            ))
        );
    }

    #[test]
    fn a_clean_batch_is_a_success_and_nothing_created_is_silent() {
        let mut r = run("curl https://x/a\ncurl https://x/b");
        assert_eq!(r.summary(), None);
        r.created.push(created("GET a", &[], &[]));
        assert_eq!(r.summary(), Some(("imported 1 of 2".into(), ToastKind::Success)));
    }
}

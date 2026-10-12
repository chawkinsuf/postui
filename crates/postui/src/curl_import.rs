//! One curl import in progress (spec curl §4.2–4.3): the parsed
//! commands, where they go, and what each name prompt made of them. It
//! rides inside the prompt's `PromptKind` and the actions the prompt
//! dispatches, so a prompt closed any way at all leaves nothing behind.

use crate::components::toast::ToastKind;
use postui_core::curl::{CurlBatch, CurlImport};

/// What one Enter created.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreatedImport {
    /// The request's display name as created.
    pub name: String,
    pub ignored: Vec<String>,
    pub converted: Vec<String>,
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
}

impl CurlImportRun {
    pub fn new(batch: CurlBatch, folder: Option<String>) -> Self {
        Self {
            folder,
            commands: batch.commands,
            index: 0,
            created: Vec::new(),
            skipped_lines: batch.skipped_lines,
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
        let mut converted: Vec<&str> = Vec::new();
        for n in self.created.iter().flat_map(|c| &c.converted) {
            if !converted.contains(&n.as_str()) {
                converted.push(n);
            }
        }
        if !converted.is_empty() {
            let list: Vec<String> = converted.iter().map(|n| format!("${n} \u{2192} {{{{{n}}}}}")).collect();
            parts.push(format!("converted {}", list.join(", ")));
        }
        if self.skipped_lines > 0 {
            let s = if self.skipped_lines == 1 { "" } else { "s" };
            parts.push(format!("skipped {} non-curl line{s}", self.skipped_lines));
        }
        if parts.is_empty() {
            return None;
        }
        let kind = if !ignored.is_empty() || self.skipped_lines > 0 {
            ToastKind::Warning
        } else {
            ToastKind::Success
        };
        Some((parts.join("; "), kind))
    }
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
        }
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
    fn a_clean_batch_is_a_success_and_nothing_created_is_silent() {
        let mut r = run("curl https://x/a\ncurl https://x/b");
        assert_eq!(r.summary(), None);
        r.created.push(created("GET a", &[], &[]));
        assert_eq!(r.summary(), Some(("imported 1 of 2".into(), ToastKind::Success)));
    }
}

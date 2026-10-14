//! Curl import and export flows (spec
//! `docs/superpowers/specs/2026-10-11-curl-import-export-design.md`).
//! Split from `app.rs`; every method here is `impl App`.

use super::*;
use crate::curl_import::{CreatedImport, CurlImportRun};

impl App {
    /// `Action::ImportCurl` (spec §4.2 steps 1–2): clipboard → parse →
    /// dirty gate → first prompt. A failure toasts and creates nothing.
    pub(super) fn import_curl(&mut self, folder: Option<String>) -> bool {
        if self.refuse_without_project() {
            return true;
        }
        let Ok(text) = self.clipboard.read() else {
            self.toasts.push("clipboard doesn't hold a curl command", ToastKind::Error);
            return true;
        };
        let batch = match postui_core::curl::parse(&text) {
            Ok(b) => b,
            Err(e) => {
                self.toasts.push(e.to_string(), ToastKind::Error);
                return true;
            }
        };
        let run = Box::new(CurlImportRun::new(batch, folder));
        // Each create opens its request, replacing the editor: ask once,
        // up front. Discard discards there and then, so skipping every
        // prompt afterwards still leaves what the answer said.
        if self.editor_holds_unsaved() {
            self.dirty_gate_split(
                "import",
                Action::BeginCurlImport { run: run.clone(), discard: false },
                Action::BeginCurlImport { run, discard: true },
            );
        } else {
            self.push_curl_prompt(*run);
        }
        true
    }

    /// `Action::BeginCurlImport`: the dirty gate's answer, then the first
    /// prompt. A saved request's discard is `Action::DiscardChanges`; a
    /// scratch request has nothing to go back to, so it becomes a blank
    /// one: a fresh scratch, its body's engine state gone with the old
    /// text, but with the shadow kept so the discard is an undo step.
    pub(super) fn begin_curl_import(&mut self, run: CurlImportRun, discard: bool) -> bool {
        if discard {
            if self.editor.slug.is_some() {
                self.update(Action::DiscardChanges);
            } else {
                self.flush_field_session();
                self.no_coalesce = true;
                self.reseed_editor(None, postui_core::model::HttpRequest::default());
                self.editor.saved = None;
                self.vim_drop_body();
                self.toasts.push(format!("Request discarded{}", self.undo_hint()), ToastKind::Info);
            }
        }
        self.push_curl_prompt(run);
        true
    }

    /// The name prompt for `run`'s current command (spec §4.2 step 3).
    pub(super) fn push_curl_prompt(&mut self, run: CurlImportRun) {
        let space = self.active_space();
        let folder = run.folder.clone().unwrap_or_else(|| space.clone());
        let leaf = self.free_request_name(&folder, &run.current().suggested_name);
        // The prompt speaks folders inside the space, as New request does.
        let prefill = match folder.strip_prefix(&format!("{space}/")) {
            Some(rel) => format!("{rel}/{leaf}"),
            None => leaf,
        };
        self.push_modal(Modal::Prompt {
            title: run.title(),
            input: LineInput::new(&prefill),
            kind: PromptKind::ImportCurl(Box::new(run)),
            revealed: false,
        });
    }

    /// `base`, else `base 2`, `base 3`, …: the first name no request in
    /// `folder` answers to (spec R8).
    fn free_request_name(&self, folder: &str, base: &str) -> String {
        let Some(p) = self.project() else {
            return base.to_string();
        };
        if !p.sibling_name_taken(folder, base, None) {
            return base.to_string();
        }
        (2..)
            .map(|n| format!("{base} {n}"))
            .find(|c| !p.sibling_name_taken(folder, c, None))
            .expect("some suffix is free")
    }

    /// `Action::ImportCurlCreate` (spec §4.2 step 4). A refused name toasts
    /// and sets `last_action_failed`, which brings the prompt back with
    /// its run untouched (`apply_modal_result`). A request that was written
    /// but could not be opened counts as created and the run moves on:
    /// asking again would only meet its own name as taken.
    pub(super) fn import_curl_create(&mut self, mut run: CurlImportRun, name: &str) -> bool {
        let import = run.current().clone();
        // The paste's values go in as secrets (spec R7), never over a
        // variable the project already has, nor under a reserved name.
        let (reserved, variables): (Vec<_>, Vec<_>) = import
            .variables
            .iter()
            .cloned()
            .partition(|(n, _)| postui_core::varmodel::is_reserved(n));
        // An environment file that can't be read leaves the name to the
        // create, which refuses it and says why.
        let (secrets, kept): (Vec<_>, Vec<_>) = variables.into_iter().partition(|(n, _)| {
            !self.project_mut().is_some_and(|p| p.variable_exists(n).unwrap_or(false))
        });
        // A paste's value not known is no news against the project's.
        let kept: Vec<(String, bool)> = kept
            .into_iter()
            .map(|(n, v)| {
                let same = v.as_ref().is_none_or(|v| {
                    self.project().is_some_and(|p| p.resolved().values.get(&n) == Some(v))
                });
                (n, same)
            })
            .collect();
        let env = self.project().and_then(|p| p.active_env().map(|e| p.env_name(e)));
        let name = match self.create_request_from(name, |_| import.request, &secrets) {
            CreateOutcome::Refused => return true,
            CreateOutcome::Opened => self.editor.name.clone().unwrap_or_else(|| name.to_string()),
            CreateOutcome::Unopened(leaf) => {
                // `land` toasted why; the prompt must not come back.
                self.last_action_failed = false;
                leaf
            }
        };
        if secrets.iter().any(|(_, v)| v.is_some()) {
            run.secrets_env = env;
        }
        run.created.push(CreatedImport {
            name,
            ignored: import.ignored,
            converted: import.converted,
            secrets,
            kept,
            reserved: reserved.into_iter().map(|(n, _)| n).collect(),
        });
        self.advance_curl_import(run);
        true
    }

    /// The next command's prompt, or, past the last, the summary toast
    /// (spec §4.2 step 5). The last request created is already open: each
    /// create opened its own.
    pub(super) fn advance_curl_import(&mut self, mut run: CurlImportRun) {
        if run.index + 1 < run.commands.len() {
            run.index += 1;
            self.push_curl_prompt(run);
        } else if let Some((text, kind)) = run.summary() {
            self.toasts.push(text, kind);
        }
    }

    /// `Action::CopyAsCurl` (spec §5): Send's own resolution, then the
    /// command on the clipboard. Unresolved variables copy nothing.
    pub(super) fn copy_as_curl(&mut self, slug: Option<String>) -> bool {
        let live = match &slug {
            None => true,
            Some(s) => self.editor.slug.as_deref() == Some(s.as_str()),
        };
        // As Send does: an open cell's text goes out, and the variables
        // are read fresh (plan P6).
        if live {
            self.commit_table_edit();
        }
        self.apply(Action::ReloadProjectFiles);
        let req = if live {
            if self.editor.url.text().trim().is_empty() {
                self.toasts.push("cannot copy: URL is empty", ToastKind::Warning);
                return true;
            }
            self.editor.current_request()
        } else {
            let slug = slug.expect("a request that is not open has a slug");
            match self.project_mut().map(|p| p.read_request(&slug)) {
                Some(Ok(r)) if r.url.trim().is_empty() => {
                    self.toasts.push("cannot copy: URL is empty", ToastKind::Warning);
                    return true;
                }
                Some(Ok(r)) => r,
                Some(Err(e)) => {
                    let msg = format!("could not read {}: {e}", self.request_display(&slug));
                    self.toasts.push(msg, ToastKind::Error);
                    return true;
                }
                None => return true,
            }
        };
        let ctx = self.prepare_context();
        match postui_core::prepare::prepare(&req, &ctx) {
            Ok((prepared, _warnings)) => {
                let text = postui_core::curl::to_curl(&prepared);
                let msg = if postui_core::curl::includes_secrets(&req, &ctx, &prepared) {
                    "Copied as curl \u{2014} includes secret values"
                } else {
                    "Copied as curl"
                };
                self.copy_text_with_toast(&text, msg.to_string());
            }
            Err(postui_core::prepare::PrepareError::Unresolved(causes)) => {
                let msg = self.unresolved_message(&causes);
                self.toasts.push(msg, ToastKind::Error);
            }
        }
        true
    }
}

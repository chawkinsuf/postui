//! Curl import and export flows (spec
//! `docs/superpowers/specs/2026-10-11-curl-import-export-design.md`).
//! Split from `app.rs`; every method here is `impl App`.

use super::*;
use crate::curl_import::{CreatedImport, CurlImportRun, CurlReplace, CurlVariables};
use postui_core::curl::{CurlBatch, CurlImport};

impl App {
    /// The clipboard's curl commands; a clipboard that can't be read or
    /// does not parse toasts why and gives `None`.
    fn clipboard_curl(&mut self) -> Option<CurlBatch> {
        let Ok(text) = self.clipboard.read() else {
            self.toasts.push("clipboard doesn't hold a curl command", ToastKind::Error);
            return None;
        };
        match postui_core::curl::parse(&text) {
            Ok(b) => Some(b),
            Err(e) => {
                self.toasts.push(e.to_string(), ToastKind::Error);
                None
            }
        }
    }

    /// `Action::ImportCurl` (spec §4.2 steps 1–2): clipboard → parse →
    /// dirty gate → first prompt. A failure toasts and creates nothing.
    pub(super) fn import_curl(&mut self, folder: Option<String>) -> bool {
        if self.refuse_without_project() {
            return true;
        }
        let Some(batch) = self.clipboard_curl() else {
            return true;
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
        let name = self.free_request_name(&folder, &run.current().suggested_name);
        // The prompt speaks folders inside the space, as New request does.
        let prefill = match folder.strip_prefix(&format!("{space}/")) {
            Some(rel) => format!("{rel}/{name}"),
            None => name,
        };
        self.push_modal(Modal::Prompt {
            title: run.title(),
            input: LineInput::new(&prefill),
            kind: PromptKind::ImportCurl(Box::new(run)),
            revealed: false,
        });
    }

    /// `base`, else `base 2`, `base 3`, …: the first name no request in
    /// `folder` answers to (spec R8). A `base` with folders in it
    /// (`api/List`) numbers its leaf, in the folder it points into, read as
    /// the create will read it; one the create refuses is left as it is,
    /// for the prompt to say why.
    fn free_request_name(&self, folder: &str, base: &str) -> String {
        let Some(p) = self.project() else {
            return base.to_string();
        };
        let path = format!("{folder}/{base}");
        let Some((folder, leaf)) = postui_core::storage::split_display_path(&path) else {
            return base.to_string();
        };
        if !p.sibling_name_taken(&folder, &leaf, None) {
            return base.to_string();
        }
        let n = (2..)
            .find(|n| !p.sibling_name_taken(&folder, &format!("{leaf} {n}"), None))
            .expect("some suffix is free");
        format!("{} {n}", base.trim_end())
    }

    /// `Action::ImportCurlCreate` (spec §4.2 step 4). A refused name toasts
    /// and sets `last_action_failed`, which brings the prompt back with
    /// its run untouched (`apply_modal_result`). A request that was written
    /// but could not be opened counts as created and the run moves on:
    /// asking again would only meet its own name as taken.
    pub(super) fn import_curl_create(&mut self, mut run: CurlImportRun, name: &str) -> bool {
        let import = run.current().clone();
        let CurlVariables { secrets, kept, reserved } =
            match self.curl_variables(&import, &Default::default()) {
                Ok(v) => v,
                Err(e) => {
                    self.toasts.push(format!("could not save {name}: {e}"), ToastKind::Error);
                    self.last_action_failed = true;
                    return true;
                }
            };
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
            reserved,
        });
        self.advance_curl_import(run);
        true
    }

    /// `import`'s variables sorted as spec R7 says: the paste's values go
    /// in as secrets, never over a variable the project (or, for a
    /// replace, the request itself, `request_vars`) already has, nor under
    /// a reserved name. Only an enabled request variable counts: a
    /// disabled one sends nothing (U10). An environment file that can't be
    /// read is an error: what it sets is unknown.
    fn curl_variables(
        &mut self,
        import: &CurlImport,
        request_vars: &indexmap::IndexMap<String, postui_core::model::Entry>,
    ) -> Result<CurlVariables, String> {
        let mut vars =
            CurlVariables { secrets: Vec::new(), kept: Vec::new(), reserved: Vec::new() };
        for (n, v) in import.variables.iter().cloned() {
            if postui_core::varmodel::is_reserved(&n) {
                vars.reserved.push(n);
                continue;
            }
            let request = request_vars.get(&n).filter(|e| e.enabled);
            // The request's own wins at send: the project's is not asked.
            let exists = request.is_some()
                || match self.project_mut() {
                    Some(p) => p.variable_exists(&n).map_err(|e| e.to_string())?,
                    None => false,
                };
            if !exists {
                vars.secrets.push((n, v));
                continue;
            }
            // A paste's value not known is no news against the project's.
            let same = v.as_ref().is_none_or(|v| match request {
                Some(e) => e.value == *v,
                None => self.project().is_some_and(|p| p.resolved().values.get(&n) == Some(v)),
            });
            vars.kept.push((n, same));
        }
        Ok(vars)
    }

    /// `Action::ReplaceWithCurl` (spec 2026-10-14 replace-with-curl): the
    /// clipboard's one curl command replaces the request in the editor.
    /// `slug` names a sidebar row, opened first, through the dirty gate
    /// (U7). Everything that can refuse (the clipboard, the parse, the
    /// target's file, the variables' sort) runs before the gate, so a
    /// refusal only toasts and the gate's answer is never left undone.
    pub(super) fn replace_with_curl(&mut self, slug: Option<String>) -> bool {
        if self.refuse_without_project() {
            return true;
        }
        let Some(batch) = self.clipboard_curl() else {
            return true;
        };
        if batch.commands.len() > 1 {
            let n = batch.commands.len();
            let msg = format!("clipboard holds {n} curl commands; replace takes one");
            self.toasts.push(msg, ToastKind::Error);
            return true;
        }
        // The editor as it stands, as Copy as curl reads it: an open cell's
        // text and an open field's keys land first, as their own step.
        self.commit_table_edit();
        self.capture_undo();
        self.flush_field_session();
        // The variables are sorted against the files as they are now, as
        // Send and Copy as curl read them (plan P6).
        self.apply(Action::ReloadProjectFiles);
        let other = slug.filter(|s| self.editor.slug.as_deref() != Some(s.as_str()));
        // The variables the request will have once open: its file's, for a
        // row that is not open yet.
        let request_vars = match &other {
            None => self.editor.variables.clone(),
            Some(s) => match self.project_mut().map(|p| p.read_request(s)) {
                Some(Ok(r)) => r.variables,
                Some(Err(e)) => {
                    let msg = format!("could not read {}: {e}", self.request_display(s));
                    self.toasts.push(msg, ToastKind::Error);
                    return true;
                }
                None => return true,
            },
        };
        let vars = match self.curl_variables(&batch.commands[0], &request_vars) {
            Ok(v) => v,
            Err(e) => {
                self.toasts.push(format!("cannot replace: {e}"), ToastKind::Error);
                return true;
            }
        };
        let CurlBatch { commands, skipped_lines, values_dropped } = batch;
        let import = commands.into_iter().next().expect("checked: one command");
        let replace = CurlReplace { import, skipped_lines, values_dropped, vars };
        match other {
            Some(slug) if self.editor_holds_unsaved() => {
                let replace = Box::new(replace);
                self.dirty_gate("open", Action::ForceReplaceWithCurl { slug, replace });
            }
            Some(slug) => self.open_and_replace(slug, replace),
            None => self.apply_curl_replace(replace),
        }
        true
    }

    /// `Action::ForceReplaceWithCurl`, and a clean editor's way there:
    /// opens `slug`, then replaces it.
    pub(super) fn open_and_replace(&mut self, slug: String, replace: CurlReplace) {
        self.apply(Action::ForceOpenRequest(slug.clone()));
        if self.editor.slug.as_deref() != Some(slug.as_str()) {
            // The open failed and toasted why.
            return;
        }
        // The open and the replace share an input event, and a capture
        // that sees the slug change only re-seeds: seat the opened request
        // as the shadow, so the replace is a step of its own. Here, not in
        // the shared open, where a reopen of the open request would stop
        // recording its Discard (fourth review round).
        self.shadow = Some((self.editor.slug.clone(), self.editor.current_request()));
        self.apply_curl_replace(replace);
    }

    /// The replace itself (U2, U3, U5, U11). The declarations are a project
    /// step recorded before the editor changes, as extract-to-variable's
    /// are: the editor change is then the next `capture_undo`'s own step,
    /// so undoing it never reloads the request over unsaved work.
    fn apply_curl_replace(&mut self, replace: CurlReplace) {
        let CurlReplace {
            import,
            skipped_lines,
            values_dropped,
            vars: CurlVariables { secrets, kept, reserved },
        } = replace;
        let current = self.editor.current_request();
        let env = self.project().and_then(|p| p.active_env().map(|e| p.env_name(e)));
        if !secrets.is_empty() {
            let names: Vec<&str> = secrets.iter().map(|(n, _)| n.as_str()).collect();
            let label =
                self.open_request_label(crate::undo::StepLabel::variable(&names.join(", ")));
            let t = self.begin_op();
            let result = match self.project_mut() {
                Some(p) => p.declare_secrets(&secrets).map_err(|e| e.to_string()),
                None => Err(NO_PROJECT.to_string()),
            };
            self.sync_varmanager();
            self.record_project_step(t, label);
            if let Err(msg) = result {
                self.toasts.push(format!("cannot replace: {msg}"), ToastKind::Error);
                return;
            }
        }
        let replaced = postui_core::model::HttpRequest {
            name: current.name.clone(),
            jq: current.jq.clone(),
            jq_enabled: current.jq_enabled,
            variables: current.variables.clone(),
            ..import.request
        };
        self.no_coalesce = true;
        self.editor.apply_snapshot(&replaced);
        self.sync_active_tab();
        let changed = self.editor.current_request() != current;
        let name = match self.editor.slug.clone() {
            Some(slug) => self.request_display(&slug),
            None => "the unsaved request".to_string(),
        };
        // The hint only when there is a step to undo: a curl the request
        // already matches changes nothing in the editor.
        let undoable = changed || !secrets.is_empty();
        let saved_any = secrets.iter().any(|(_, v)| v.is_some());
        let created = CreatedImport {
            name: name.clone(),
            ignored: import.ignored,
            converted: import.converted,
            secrets,
            kept,
            reserved,
        };
        let pasted = crate::curl_import::Pasted {
            skipped_lines,
            values_dropped,
            secrets_env: env.as_deref().filter(|_| saved_any),
        };
        let (parts, kind) = crate::curl_import::summary_parts(&[created], false, &pasted);
        let head = if changed {
            format!("Replaced {name} from curl")
        } else {
            format!("Nothing to replace: {name} already matches the curl")
        };
        let text = std::iter::once(head).chain(parts).collect::<Vec<_>>().join("; ");
        let hint = if undoable { self.undo_hint() } else { String::new() };
        self.toasts.push(format!("{text}{hint}"), kind);
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

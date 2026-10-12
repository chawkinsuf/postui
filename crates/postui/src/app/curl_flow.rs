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
            self.toasts
                .push("clipboard doesn't hold a curl command", ToastKind::Error);
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
        // up front. Discard defers nothing — the first create replaces the
        // edit, and Esc on every prompt leaves it where it was.
        if self.editor_holds_unsaved() {
            self.dirty_gate("import", Action::BeginCurlImport(run));
        } else {
            self.push_curl_prompt(*run);
        }
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
    /// its run untouched (`apply_modal_result`).
    pub(super) fn import_curl_create(&mut self, mut run: CurlImportRun, name: &str) -> bool {
        let import = run.current().clone();
        if !self.create_or_save_as(name, |_| import.request) {
            return true;
        }
        run.created.push(CreatedImport {
            name: self.editor.name.clone().unwrap_or_else(|| name.to_string()),
            ignored: import.ignored,
            converted: import.converted,
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
}

//! `postui edit begin|end|status`: the edit flag (spec 2026-10-28 §4, piece
//! 3). An agent raises it before it edits the project's TOML and lowers it
//! once `check` passes. It edits only a project no postui has open: `edit
//! begin` refuses while one does, and postui won't open a flagged project
//! until the flag comes down. The flag and the TUI's lock live in
//! `postui_core::project::edit_flag`; this is the command line around them.
//!
//! A refusal (a flag already up, the project open in postui, `check` errors
//! at the end, a flag already gone) ends the run with 1, as a failed result
//! does; a mistake or a project that won't do ends it with 2 (mine
//! (approved)).

use super::check::{self, CheckReport, reason};
use super::out::{fields, rel};
use super::{CliError, Global, target};
use postui_core::project::{self, EDITING_TOML, EditFlag, Project};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::SystemTime;

/// `postui edit begin`.
#[derive(Serialize)]
pub struct BeginReport {
    root: PathBuf,
    flag: PathBuf,
    started: String,
    note: Option<String>,
}

/// `postui edit end`: `check`'s report, and whether the flag came down.
#[derive(Serialize)]
pub struct EndReport {
    root: PathBuf,
    check: CheckReport,
    flag_removed: bool,
}

/// `postui edit status`.
#[derive(Serialize)]
pub struct StatusReport {
    root: PathBuf,
    flag: Option<FlagState>,
    tui_open: bool,
}

/// A flag that is up, as `status` reports it.
#[derive(Serialize)]
struct FlagState {
    started: Option<String>,
    /// Whole seconds since `started`; `null` when it can't be told.
    age_secs: Option<u64>,
    note: Option<String>,
    /// Why the flag's file can't be read (the rest is `null` then): it is
    /// up all the same, and `edit end --force` takes it down.
    error: Option<String>,
    /// "started 4 minutes ago", for the text.
    #[serde(skip)]
    started_text: String,
}

/// Raises the flag for an edit of the project `g` names. Refuses a flag
/// already up unless `force`, and refuses (whatever `force` says) while a
/// TUI has the project open, putting back what was there before.
pub fn begin(g: &Global, note: Option<&str>, force: bool) -> Result<BeginReport, CliError> {
    let root = target::resolve(g.project.as_deref())?;
    let file = root.join(EDITING_TOML);
    let tui_open = || Project::tui_open(&root).map_err(|e| CliError::at(reason(e), root.clone()));
    // Asked first so an open project raises no flag at all; asked again
    // after raising it, which is the answer that counts (see the module
    // doc of `edit_flag.rs`).
    if tui_open()? {
        return Err(open_refusal(root));
    }
    // The raise and any put-back below, as one change of the flag.
    let _hold = hold_flag(&root)?;
    let flag = EditFlag::new(note.map(str::to_string));
    let replaced = match Project::raise_edit_flag(&root, &flag, force) {
        Ok(replaced) => replaced,
        Err(project::Error::AlreadyExists(_)) => {
            let held = match Project::read_edit_flag(&root) {
                Ok(Some(held)) => describe(&held),
                // Gone in between, or broken: the refusal stands, said
                // without what can't be read.
                _ => "an edit".to_string(),
            };
            return Err(CliError::refusal(
                format!(
                    "an edit is already in progress ({held}); postui edit end finishes it, \
                     --force takes it over"
                ),
                file,
            ));
        }
        Err(e) => return Err(CliError::at(reason(e), file)),
    };
    // The flag first, then the lock: either this sees the TUI's lock, or
    // the TUI, taking its lock, sees the flag and opens nothing.
    let open = tui_open();
    if matches!(open, Ok(false)) {
        return Ok(BeginReport { flag: file, started: flag.started, note: flag.note, root });
    }
    // Refused either way: the flag goes back to what it was. One that won't
    // come down is said with the refusal, so the agent knows to clear it.
    let restored = Project::restore_edit_flag(&root, replaced.as_deref());
    let mut refusal = match open {
        Ok(_) => open_refusal(root),
        Err(e) => e,
    };
    if let Err(e) = restored {
        refusal.message.push_str(&format!(
            "; its flag could not be taken back down ({}): postui edit end --force removes it",
            reason(e)
        ));
    }
    Err(refusal)
}

/// Holds the flag still for this command's change of it
/// (`Project::hold_edit_flag`); another command holding it is a refusal.
fn hold_flag(root: &Path) -> Result<project::FlagHold, CliError> {
    match Project::hold_edit_flag(root) {
        Ok(Some(hold)) => Ok(hold),
        Ok(None) => Err(CliError::refusal(
            "another postui edit command is changing the flag; retry",
            root.join(project::EDITING_LOCK),
        )),
        Err(e) => Err(CliError::at(reason(e), root.join(project::EDITING_LOCK))),
    }
}

/// `edit begin` with a TUI that has the project open.
fn open_refusal(root: PathBuf) -> CliError {
    CliError::refusal(
        "postui has this project open; ask the user to switch to another project or quit \
         postui, then retry",
        root,
    )
}

/// Lowers the flag once `check` finds no error (or with `force`). A flag
/// already down is a refusal: the user overrode it, so the agent's edit
/// may have met theirs. A project whose `project.toml` the edit removed
/// is still this flag's, and `check` says what is missing.
pub fn end(g: &Global, force: bool) -> Result<EndReport, CliError> {
    let root = target::resolve_flagged(g.project.as_deref())?;
    let file = root.join(EDITING_TOML);
    let read = || Project::read_edit_flag(&root).map_err(|e| CliError::at(reason(e), file.clone()));
    // A flag file that won't parse is up all the same: `--force` takes it
    // down without knowing whose it is; without, its error is the answer.
    let ours = match read() {
        Ok(None) => return Err(overridden(file)),
        Ok(Some(flag)) => Some(flag),
        Err(_) if force && Project::edit_flag_up(&root) => None,
        Err(e) => return Err(e),
    };
    let check = check::check_root(root.clone(), None)?;
    let flag_removed = check.status() == ExitCode::SUCCESS || force;
    if flag_removed {
        // The look and the take-down as one change of the flag.
        let _hold = hold_flag(&root)?;
        // Taken down while `check` ran (an override), or taken over by
        // another `edit begin --force`, whose flag is left up. An
        // unreadable flag it found is lowered only while it still is one.
        let now = Project::read_edit_flag(&root);
        let still_ours = match (&ours, &now) {
            (Some(ours), Ok(Some(now))) => now == ours,
            (None, Err(_)) => true,
            _ => false,
        };
        if !still_ours {
            return Err(match now {
                Ok(None) => overridden(file),
                Ok(Some(_)) => CliError::refusal(
                    "another edit took this one over (edit begin --force) while check ran; its \
                     flag is left up",
                    file,
                ),
                // Readable when this began: rewritten since, by whom it
                // can't say, so it is left up and its error is the answer.
                Err(e) => CliError::refusal(
                    format!(
                        "the flag changed while check ran and can't be read now ({}); it is \
                         left up: postui edit status shows it, --force removes it",
                        reason(e)
                    ),
                    file,
                ),
            });
        }
        Project::lower_edit_flag(&root).map_err(|e| CliError::at(reason(e), file.clone()))?;
    }
    Ok(EndReport { root, check, flag_removed })
}

/// `edit end` with no flag to take down.
fn overridden(file: PathBuf) -> CliError {
    CliError::refusal(
        "no edit is in progress: the user overrode it in postui, or edit begin never ran",
        file,
    )
}

/// Whether the flag is up and whether a TUI has the project open. Writes
/// nothing.
pub fn status(g: &Global) -> Result<StatusReport, CliError> {
    let root = target::resolve_flagged(g.project.as_deref())?;
    let now = SystemTime::now();
    let flag = match Project::read_edit_flag(&root) {
        Ok(flag) => flag.map(|f| FlagState {
            age_secs: f.age(now).map(|a| a.as_secs()),
            started_text: f.started_text(now),
            started: Some(f.started),
            note: f.note,
            error: None,
        }),
        // Up, but unreadable: said, rather than failing the question.
        Err(e) if Project::edit_flag_up(&root) => Some(FlagState {
            started: None,
            age_secs: None,
            note: None,
            error: Some(reason(e)),
            started_text: String::new(),
        }),
        Err(e) => return Err(CliError::at(reason(e), root.join(EDITING_TOML))),
    };
    let tui_open = Project::tui_open(&root).map_err(|e| CliError::at(reason(e), root.clone()))?;
    Ok(StatusReport { root, flag, tui_open })
}

/// "started 4 minutes ago: "add orders"".
fn describe(flag: &EditFlag) -> String {
    let started = flag.started_text(SystemTime::now());
    match &flag.note {
        Some(note) => format!("{started}: {note:?}"),
        None => started,
    }
}

impl BeginReport {
    pub fn text(&self) -> String {
        let mut rows =
            vec![("root", self.root.display().to_string()), ("flag", rel(&self.root, &self.flag))];
        if let Some(note) = &self.note {
            rows.push(("note", note.clone()));
        }
        fields(&rows)
    }
}

impl EndReport {
    /// 1 when the flag stayed up, or `check` found errors (a `--force`d end
    /// is still a project with errors in it, as `check` says), else 0.
    pub fn status(&self) -> ExitCode {
        if self.flag_removed && self.check.status() == ExitCode::SUCCESS {
            ExitCode::SUCCESS
        } else {
            ExitCode::from(1)
        }
    }

    /// `check`'s report, then what became of the flag.
    pub fn text(&self) -> String {
        let flag = if self.flag_removed {
            "removed".to_string()
        } else {
            "kept: fix the errors and run postui edit end again, or --force".to_string()
        };
        format!("{}\n{}", self.check.text(), fields(&[("flag", flag)]))
    }
}

impl StatusReport {
    pub fn text(&self) -> String {
        let mut rows = vec![("root", self.root.display().to_string())];
        match &self.flag {
            None => rows.push(("flag", "down".to_string())),
            Some(FlagState { error: Some(error), .. }) => rows.push((
                "flag",
                format!("up, but its file can't be read ({error}); edit end --force removes it"),
            )),
            Some(f) => {
                rows.push(("flag", format!("up, {}", f.started_text)));
                if let Some(note) = &f.note {
                    rows.push(("note", note.clone()));
                }
            }
        }
        rows.push(("postui", if self.tui_open { "open" } else { "not open" }.to_string()));
        fields(&rows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn begin_text_names_the_flag_relative() {
        let root = PathBuf::from("/p/api");
        let report = BeginReport {
            flag: root.join(EDITING_TOML),
            root,
            started: "2026-10-29T20:31:05Z".into(),
            note: Some("add orders".into()),
        };
        assert_eq!(report.text(), "root  /p/api\nflag  .postui/editing.toml\nnote  add orders\n");
    }

    #[test]
    fn status_text_says_down_and_not_open() {
        let report = StatusReport { root: PathBuf::from("/p"), flag: None, tui_open: false };
        assert_eq!(report.text(), "root    /p\nflag    down\npostui  not open\n");
    }
}

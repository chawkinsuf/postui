//! The app's side of the edit flag (spec 2026-10-28 §4, piece 3). An agent
//! edits only a project no postui has open (the user: "require the project
//! be closed entirely before the agent starts"): the app holds the open
//! project's lock ([`ProjectLock`]) for as long as it has it open, and
//! every open takes the lock before it looks for the flag
//! ([`App::lock_for_open`]). A project another window has open doesn't
//! open here. One whose flag is up doesn't open either: the "Updates in
//! progress" screen waits with no project open, and opens it once the flag
//! comes down, fresh from its files. Switch projects, Override (a flag left
//! behind by an agent that stopped) and Quit are the ways out.

use super::*;
use postui_core::project::{EditFlag, ProjectLock};
use std::path::Path;

/// How often the waiting screen looks at the flag: a stat a second.
const POLL_EVERY: Duration = Duration::from_secs(1);

/// The Override question's title, by which an open finds it to close.
const OVERRIDE_TITLE: &str = "Override the agent's edit";

/// The waiting screen's state: the project it waits on, which is not open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditBlock {
    /// The flagged project.
    pub root: PathBuf,
    /// Its display name, read once when the screen goes up.
    pub name: String,
    /// The flag as last read; `None` when its file won't parse (it is up
    /// all the same).
    pub flag: Option<EditFlag>,
}

/// The screen's three buttons, each with a key both profiles share.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EditBlockButton {
    Switch,
    Override,
    Quit,
}

impl EditBlockButton {
    /// Left to right. Switch, the way on, holds the left; Quit, which ends
    /// the session, is last; Override, the risky one, sits between.
    pub const ALL: [EditBlockButton; 3] =
        [EditBlockButton::Switch, EditBlockButton::Override, EditBlockButton::Quit];

    pub fn key(self) -> char {
        match self {
            EditBlockButton::Switch => 's',
            EditBlockButton::Override => 'o',
            EditBlockButton::Quit => 'q',
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            EditBlockButton::Switch => "Switch projects",
            EditBlockButton::Override => "Override",
            EditBlockButton::Quit => "Quit",
        }
    }

    fn action(self) -> Action {
        match self {
            EditBlockButton::Switch => Action::OpenProjectChooser,
            EditBlockButton::Override => Action::ConfirmOverrideEditFlag,
            EditBlockButton::Quit => Action::Quit,
        }
    }
}

/// What an open may do with a project, as [`App::lock_for_open`] finds it.
pub(super) enum Opening {
    /// Open it. `lock` is `None` for a directory with no `project.toml`
    /// (the create-project prompt's, which takes it once it writes one), or
    /// when the lock couldn't be taken: `warning` then says so, since an
    /// agent's `edit begin` can't see this window.
    Open { lock: Option<ProjectLock>, warning: Option<String> },
    /// Another window has it open.
    InUse,
    /// An agent's edit flag is up: wait behind the screen.
    Flagged,
}

/// The warning for a project open without its lock, `why` it couldn't be
/// had.
fn unlocked(why: &str) -> String {
    format!(
        "could not lock the project ({why}): an agent's postui edit begin won't know it is open"
    )
}

impl App {
    /// The lock, then the flag, for opening `root` (the order the module
    /// doc of `postui_core::project::edit_flag` explains): either the
    /// agent's `edit begin` sees the lock and takes its flag back down, or
    /// this sees the flag and opens nothing, letting the lock go.
    pub(super) fn lock_for_open(root: &Path) -> Opening {
        let taken = Project::is_project(root).then(|| ProjectLock::take(root));
        if matches!(taken, Some(Ok(None))) {
            return Opening::InUse;
        }
        if Project::edit_flag_up(root) {
            return Opening::Flagged;
        }
        match taken {
            Some(Ok(lock)) => Opening::Open { lock, warning: None },
            Some(Err(e)) => Opening::Open { lock: None, warning: Some(unlocked(&e.to_string())) },
            None => Opening::Open { lock: None, warning: None },
        }
    }

    /// Takes `root`'s lock for a project about to be made in place (the
    /// create-project answers), where no open took it first: before its
    /// `project.toml` is written, since an agent's `edit begin` needs one
    /// to find the project. The make goes ahead whatever the take finds,
    /// and a lock it can't have is said.
    pub(super) fn take_project_lock(&mut self, root: &Path) {
        self.project_lock = None;
        match ProjectLock::take(root) {
            Ok(Some(lock)) => self.project_lock = Some(lock),
            Ok(None) => self
                .toasts
                .push(unlocked("another postui window has it open too"), ToastKind::Warning),
            Err(e) => self.toasts.push(unlocked(&e.to_string()), ToastKind::Warning),
        }
    }

    /// The waiting screen's look at the flag, at most once a second unless
    /// `now_please`: re-reads it (its note) while it is up, and opens the
    /// project once it is down. Whether to redraw: always while the screen
    /// is up, so its age stays current.
    pub(crate) fn poll_edit_flag(&mut self, now: Instant, now_please: bool) -> bool {
        let Some(root) = self.edit_block.as_ref().map(|b| b.root.clone()) else {
            return false;
        };
        if !now_please && self.edit_flag_polled.is_some_and(|at| now - at < POLL_EVERY) {
            return false;
        }
        self.edit_flag_polled = Some(now);
        match Project::read_edit_flag(&root) {
            Ok(None) => {
                // The Override question asked about an edit that is over.
                // Any other dialog (the projects chooser, a file picker) is
                // the user's, about where to go next: the open waits for it
                // to close rather than opening under it.
                self.close_override_question();
                if self.modals.is_empty() {
                    self.apply(Action::ForceSwitchProject(root));
                }
            }
            Ok(Some(flag)) => {
                if let Some(block) = self.edit_block.as_mut() {
                    block.flag = Some(flag);
                }
            }
            // Up, but unreadable: shown without its age and note.
            Err(_) => {
                if let Some(block) = self.edit_block.as_mut() {
                    block.flag = None;
                }
            }
        }
        true
    }

    /// Puts the screen up for `root`'s flag. No project is open behind it.
    pub(super) fn raise_edit_block(&mut self, root: PathBuf) {
        let flag = Project::read_edit_flag(&root).ok().flatten();
        let name = Project::peek_display_name(&root);
        self.edit_block = Some(EditBlock { root, name, flag });
        self.hovered = None;
    }

    /// Closes the Override question, if it is open: the screen it asked
    /// about is gone.
    pub(super) fn close_override_question(&mut self) {
        if matches!(self.modals.top(), Some(Modal::Confirm { title, .. }) if title == OVERRIDE_TITLE)
        {
            self.modals.pop();
        }
    }

    /// Override, confirmed: the flag comes down, as `edit end` would take
    /// it, the agent's `edit end` will say the user overrode it, and the
    /// project opens. `asked` is the flag the screen showed when the
    /// question was put, which the poll may have re-read since: only that
    /// one is taken down.
    pub(super) fn override_edit_flag(&mut self, asked: Option<EditFlag>) -> bool {
        let Some(root) = self.edit_block.as_ref().map(|b| b.root.clone()) else {
            return false;
        };
        // The look and the take-down as one change of the flag, as `edit
        // end`'s are.
        let hold = match Project::hold_edit_flag(&root) {
            Ok(Some(hold)) => hold,
            Ok(None) => {
                self.toasts
                    .push("An agent's command is changing the flag; try again", ToastKind::Info);
                return true;
            }
            Err(e) => {
                self.toasts.push(format!("could not remove the edit flag: {e}"), ToastKind::Error);
                return true;
            }
        };
        // An unreadable flag is `None`, as the screen holds one: a question
        // asked about a readable flag doesn't lower one gone unreadable.
        let now = match Project::read_edit_flag(&root) {
            Ok(None) => {
                drop(hold);
                return self.apply(Action::ForceSwitchProject(root));
            }
            Ok(Some(flag)) => Some(flag),
            Err(_) => None,
        };
        if now != asked {
            if let Some(block) = self.edit_block.as_mut() {
                block.flag = now;
            }
            self.toasts.push("Another edit began; it is the one showing now", ToastKind::Info);
            return true;
        }
        if let Err(e) = Project::lower_edit_flag(&root) {
            self.toasts.push(format!("could not remove the edit flag: {e}"), ToastKind::Error);
            return true;
        }
        drop(hold);
        self.apply(Action::ForceSwitchProject(root))
    }

    /// The Override button's question.
    pub(super) fn confirm_override_edit_flag(&mut self) -> bool {
        let Some(asked) = self.edit_block.as_ref().map(|b| b.flag.clone()) else {
            return false;
        };
        self.push_modal(Modal::Confirm {
            title: OVERRIDE_TITLE.into(),
            body: "The agent may be in the middle of its edit. Open the project with its files \
                   as they are now?"
                .into(),
            choices: vec![
                ('o', "Override".into(), vec![Action::OverrideEditFlag(asked)]),
                ('n', "Cancel".into(), vec![]),
            ],
        });
        true
    }

    /// A key while the screen is up and no dialog is over it (one that is
    /// takes keys as anywhere): only the buttons' keys, the quit combo and
    /// the project cycle do anything.
    pub(super) fn blocked_key(&mut self, ev: KeyEvent) -> bool {
        let ev = crate::keys::normalize_super_keys(ev);
        let armed = self.disarm_quit();
        let combo = KeyCombo::from_event(&ev);
        if ev.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) {
            return match self.lookup_key(&combo) {
                // The quit combo quits on its second press, as everywhere.
                Some(Action::Quit) => self.press_quit_combo(combo, armed.as_ref()),
                // Cycling through the projects goes on past one an agent
                // is editing.
                Some(action @ Action::CycleProject(_)) => self.update(action),
                _ => armed.is_some(),
            };
        }
        let KeyCode::Char(c) = ev.code else {
            return armed.is_some();
        };
        match EditBlockButton::ALL.into_iter().find(|b| b.key() == c.to_ascii_lowercase()) {
            Some(button) => self.update(button.action()),
            None => armed.is_some(),
        }
    }

    /// A mouse event while the screen is up and no dialog is over it: the
    /// pointer lights the buttons and a left press on one runs it. The
    /// frame behind is not drawn, so its hits are gone.
    pub(super) fn blocked_mouse(&mut self, m: ratatui::crossterm::event::MouseEvent) -> bool {
        use ratatui::crossterm::event::{MouseButton, MouseEventKind};
        let (x, y) = (m.column, m.row);
        self.pointer = Some((x, y));
        let hit = self.hits.hit_at(x, y).cloned();
        match m.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                let disarmed = self.disarm_quit().is_some();
                match hit {
                    Some(Hit::EditBlock(button)) => self.update(button.action()),
                    _ => disarmed,
                }
            }
            MouseEventKind::Moved | MouseEventKind::Drag(_) if hit != self.hovered => {
                self.hovered = hit;
                self.begin_hover_fade();
                true
            }
            _ => false,
        }
    }
}

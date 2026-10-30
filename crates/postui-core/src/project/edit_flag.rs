//! The edit flag (spec 2026-10-28 §4, piece 3): `.postui/editing.toml`,
//! which an agent raises with `postui edit begin` before it edits the
//! project's files and lowers with `postui edit end`.
//!
//! An agent edits only a project no postui has open (the user: "require the
//! project be closed entirely before the agent starts"). A TUI holds an
//! exclusive lock on `tui.lock` for as long as it has the project open
//! ([`ProjectLock`]), which also keeps a second TUI from opening it. The
//! two sides go in opposite orders, so they can't both go ahead:
//!
//! - `edit begin` raises the flag, then asks whether the lock is held, and
//!   takes its flag back down if it is.
//! - A TUI takes the lock, then looks for the flag, and lets the lock go
//!   (opening nothing) if it is up.
//!
//! One edit at a time: a flag left up by an edit the user confirms isn't
//! running any more comes down with `edit end --force` or Override (the
//! user: "its not meant to allow two things to edit at once").
//!
//! The lock is an OS advisory lock on an open file, so a TUI that crashes
//! leaves none held; only the flag can be left behind, and the TUI's
//! Override and `edit end --force` take that down. Everything here goes
//! through [`Disk`]; nothing opens the project.

use super::*;
use crate::disk::Lock;
use std::time::{Duration, SystemTime};

/// What `editing.toml` holds.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct EditFlag {
    /// When the edit began, RFC 3339 in UTC (`2026-10-29T20:31:05Z`).
    pub started: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl EditFlag {
    /// A flag that starts now.
    pub fn new(note: Option<String>) -> EditFlag {
        EditFlag { started: rfc3339(SystemTime::now()), note }
    }

    /// How long ago it started, as of `now`; `None` when `started` isn't a
    /// time this module writes (someone edited the file). One in the future
    /// (another clock, or this one set back) started just now.
    pub fn age(&self, now: SystemTime) -> Option<Duration> {
        Some(now.duration_since(parse_rfc3339(&self.started)?).unwrap_or_default())
    }

    /// "started 4 minutes ago", or the stamp itself when its age can't be
    /// told.
    pub fn started_text(&self, now: SystemTime) -> String {
        match self.age(now) {
            Some(age) => format!("started {}", ago(age)),
            None => format!("started {}", self.started),
        }
    }
}

/// `age` in words: "just now", "1 minute ago", "3 hours ago", "2 days ago".
pub fn ago(age: Duration) -> String {
    let secs = age.as_secs();
    let (n, unit) = match secs {
        0..60 => return "just now".to_string(),
        60..3600 => (secs / 60, "minute"),
        3600..86400 => (secs / 3600, "hour"),
        _ => (secs / 86400, "day"),
    };
    format!("{n} {unit}{} ago", if n == 1 { "" } else { "s" })
}

/// `t` as RFC 3339 in UTC, to the second: `2026-10-29T20:31:05Z`. Also the
/// `--debug-ai` log's stamp.
pub fn rfc3339(t: SystemTime) -> String {
    let secs = t.duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let (y, m, d) = civil_from_days((secs / 86400) as i64);
    let s = secs % 86400;
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", s / 3600, s / 60 % 60, s % 60)
}

/// The instant [`rfc3339`] wrote; `None` for any other text.
fn parse_rfc3339(text: &str) -> Option<SystemTime> {
    let b = text.as_bytes();
    let shape = b.len() == 20
        && [4, 7].iter().all(|&i| b[i] == b'-')
        && b[10] == b'T'
        && [13, 16].iter().all(|&i| b[i] == b':')
        && b[19] == b'Z';
    if !shape {
        return None;
    }
    // Digits only: `parse` alone would take a sign.
    let num = |range: std::ops::Range<usize>| {
        let digits = text.get(range)?;
        digits.bytes().all(|c| c.is_ascii_digit()).then(|| digits.parse::<u32>().ok())?
    };
    let (y, m, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (hh, mm, ss) = (num(11..13)?, num(14..16)?, num(17..19)?);
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) || hh > 23 || mm > 59 || ss > 59 {
        return None;
    }
    let days = days_from_civil(i64::from(y), m, d);
    // A day past its month's end (`02-31`) would come back as another date.
    if civil_from_days(days) != (i64::from(y), m, d) {
        return None;
    }
    let days = u64::try_from(days).ok()?;
    let secs = days * 86400 + u64::from(hh * 3600 + mm * 60 + ss);
    Some(SystemTime::UNIX_EPOCH + Duration::from_secs(secs))
}

/// The proleptic Gregorian date `days` after 1970-01-01 (Howard
/// Hinnant's `civil_from_days`).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

/// The inverse of [`civil_from_days`].
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = i64::from(if m > 2 { m - 3 } else { m + 9 });
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

impl Project {
    /// Whether the flag is up at `root`: one stat.
    pub fn edit_flag_up(root: &Path) -> bool {
        Disk::new(root.to_path_buf()).is_file(&rel(EDITING_TOML).expect("constant path"))
    }

    /// The flag at `root`, `None` when it is down. A file that can't be
    /// read or parsed is an error naming it.
    pub fn read_edit_flag(root: &Path) -> Result<Option<EditFlag>, Error> {
        let disk = Disk::new(root.to_path_buf());
        let Some(text) = disk.peek(&rel(EDITING_TOML)?)? else {
            return Ok(None);
        };
        toml::from_str(&text).map(Some).map_err(|e| parse_err(EDITING_TOML)(&e))
    }

    /// Raises `flag` at `root`. A flag already up refuses (`AlreadyExists`)
    /// and nothing is written: one edit at a time.
    pub fn raise_edit_flag(root: &Path, flag: &EditFlag) -> Result<(), Error> {
        let mut disk = Self::disk_for(root.to_path_buf());
        let text = toml::to_string(flag).expect("EditFlag always serializes");
        match disk.write_new(&rel(EDITING_TOML)?, &text) {
            Ok(()) => Ok(()),
            Err(DiskError::AlreadyExists(_)) => Err(Error::AlreadyExists(EDITING_TOML.into())),
            Err(e) => Err(e.into()),
        }
    }

    /// Takes the flag down; whether it was up.
    pub fn lower_edit_flag(root: &Path) -> Result<bool, Error> {
        let mut disk = Self::disk_for(root.to_path_buf());
        let rel = rel(EDITING_TOML)?;
        let was_up = disk.is_file(&rel);
        disk.remove(&rel)?;
        Ok(was_up)
    }

    /// Whether a TUI has the project at `root` open: a shared try of its
    /// lock, released when the try's file closes. Shared, so two commands
    /// asking at once don't keep each other out; only a TUI's exclusive
    /// hold does. A missing file has never been locked, and asking makes
    /// none.
    pub fn tui_open(root: &Path) -> Result<bool, Error> {
        let disk = Disk::new(root.to_path_buf());
        let Some(lock) = disk.existing_lock(&rel(TUI_LOCK)?)? else {
            return Ok(false);
        };
        Ok(!lock.try_lock_shared()?)
    }
}

/// A TUI's hold on the project it has open (see the module doc): an
/// exclusive lock on `tui.lock`, released when it is dropped or the process
/// ends.
pub struct ProjectLock {
    _lock: Lock,
}

impl ProjectLock {
    /// Takes `root`'s lock, making the file (and the local folder) when
    /// missing: `Ok(None)` when another TUI holds it. Never waits, but a
    /// try that fails is tried again a few times 5 ms apart, since a
    /// command asking [`Project::tui_open`] holds a shared lock for an
    /// instant.
    pub fn take(root: &Path) -> Result<Option<ProjectLock>, Error> {
        const TRIES: u32 = 10;
        let mut disk = Project::disk_for(root.to_path_buf());
        let lock = disk.open_lock(&rel(TUI_LOCK)?)?;
        for attempt in 1..=TRIES {
            if lock.try_lock()? {
                return Ok(Some(ProjectLock { _lock: lock }));
            }
            if attempt < TRIES {
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{fixture, read};
    use super::*;

    #[test]
    fn rfc3339_writes_utc_and_reads_back() {
        let t = SystemTime::UNIX_EPOCH + Duration::from_secs(1_793_305_865);
        assert_eq!(rfc3339(t), "2026-10-29T20:31:05Z");
        assert_eq!(parse_rfc3339("2026-10-29T20:31:05Z"), Some(t));
        assert_eq!(rfc3339(SystemTime::UNIX_EPOCH), "1970-01-01T00:00:00Z");
        // A leap day, and the turn of a century that is a leap year.
        for text in ["2024-02-29T23:59:59Z", "2000-03-01T00:00:00Z", "2100-12-31T12:00:00Z"] {
            assert_eq!(parse_rfc3339(text).map(rfc3339).as_deref(), Some(text));
        }
        for bad in [
            "2026-10-29 20:31:05Z",
            "2026-13-01T00:00:00Z",
            "2026-02-31T00:00:00Z",
            "2025-02-29T00:00:00Z",
            "+026-10-29T20:31:05Z",
            "yesterday",
            "",
        ] {
            assert_eq!(parse_rfc3339(bad), None, "{bad}");
        }
    }

    #[test]
    fn age_is_told_in_words() {
        let flag = EditFlag { started: "2026-10-29T20:31:05Z".into(), note: None };
        let at = |secs| parse_rfc3339("2026-10-29T20:31:05Z").unwrap() + Duration::from_secs(secs);
        assert_eq!(flag.started_text(at(5)), "started just now");
        assert_eq!(flag.started_text(at(60)), "started 1 minute ago");
        assert_eq!(flag.started_text(at(4 * 60 + 59)), "started 4 minutes ago");
        assert_eq!(flag.started_text(at(7200)), "started 2 hours ago");
        assert_eq!(flag.started_text(at(86400 * 3)), "started 3 days ago");
        let early = parse_rfc3339("2026-10-29T20:30:00Z").unwrap();
        assert_eq!(flag.started_text(early), "started just now", "a clock behind the flag's");
        let edited = EditFlag { started: "a while back".into(), note: None };
        assert_eq!(edited.started_text(at(0)), "started a while back");
    }

    #[test]
    fn the_flag_goes_up_refuses_a_second_and_comes_down() {
        let (dir, _p) = fixture();
        let root = dir.path();
        assert!(!Project::edit_flag_up(root));
        assert_eq!(Project::read_edit_flag(root).unwrap(), None);
        let flag = EditFlag::new(Some("add orders".into()));
        Project::raise_edit_flag(root, &flag).unwrap();
        assert!(Project::edit_flag_up(root));
        assert_eq!(Project::read_edit_flag(root).unwrap(), Some(flag.clone()));
        assert!(read(&dir, ".postui/.gitignore").is_some(), "the local folder is sealed");

        let second = EditFlag { started: "2026-01-01T00:00:00Z".into(), note: None };
        assert!(matches!(Project::raise_edit_flag(root, &second), Err(Error::AlreadyExists(_))));
        assert_eq!(Project::read_edit_flag(root).unwrap(), Some(flag));

        assert!(Project::lower_edit_flag(root).unwrap());
        assert!(!Project::lower_edit_flag(root).unwrap());
        assert!(!Project::edit_flag_up(root));
    }

    #[test]
    fn a_broken_flag_file_is_an_error_naming_it() {
        let (dir, _p) = fixture();
        std::fs::create_dir_all(dir.path().join(LOCAL_DIR)).unwrap();
        std::fs::write(dir.path().join(EDITING_TOML), "started = [").unwrap();
        let err = Project::read_edit_flag(dir.path()).unwrap_err().to_string();
        assert!(err.starts_with(EDITING_TOML), "{err}");
    }

    /// One TUI holds the project; a second is kept out, and the command
    /// line sees it, until the first lets go.
    #[test]
    fn one_tui_holds_the_project_and_the_command_line_sees_it() {
        let (dir, _p) = fixture();
        let root = dir.path();
        assert!(!Project::tui_open(root).unwrap());
        assert!(!root.join(TUI_LOCK).exists(), "asking makes nothing");

        let one = ProjectLock::take(root).unwrap().expect("free");
        assert!(Project::tui_open(root).unwrap());
        assert!(ProjectLock::take(root).unwrap().is_none(), "a second TUI is kept out");
        drop(one);
        assert!(!Project::tui_open(root).unwrap());
        assert!(ProjectLock::take(root).unwrap().is_some());
    }

    /// A command's question (a shared hold) keeps a take out only while it
    /// is held: the take, which tries a few times, gets in once it goes.
    #[test]
    fn a_commands_question_keeps_a_take_out_only_while_held() {
        let (dir, _p) = fixture();
        let root = dir.path();
        drop(ProjectLock::take(root).unwrap());
        let asking =
            Disk::new(root.to_path_buf()).existing_lock(&rel(TUI_LOCK).unwrap()).unwrap().unwrap();
        assert!(asking.try_lock_shared().unwrap());
        assert!(ProjectLock::take(root).unwrap().is_none(), "held past every try");
        drop(asking);
        assert!(ProjectLock::take(root).unwrap().is_some());
    }
}

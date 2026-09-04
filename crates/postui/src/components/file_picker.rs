//! The file picker modal: a browsable directory list with a filter/name
//! field, used wherever the app used to ask for a typed path — saving a
//! response body or view, opening a project by path, and choosing the
//! parent folder of a new project.
//!
//! Two modes share one shape:
//! - `SaveFile`: the field is the filename; Enter saves it into the
//!   folder being shown. Rows list folders and files.
//! - `ChooseDir`: only folders are listed; Enter descends, or opens the
//!   folder outright when it is already a postui project. A separate
//!   "open this folder" affordance confirms the folder being shown.
//!
//! Every path is built with `std::path`, home and Downloads come from the
//! `directories` crate, and hidden means a leading dot everywhere plus
//! the hidden attribute on Windows, so the same rules hold on every
//! platform the terminal runs on.

use super::line_input::LineInput;
use super::modal::ModalResult;
use crate::action::Action;
use crate::paint::{self, ControlState, FIELD_HEIGHT, ListRow, RowHighlight, TextField};
use crate::theme::Theme;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use std::path::{Path, PathBuf};

/// What the picker is for: a file to write, or a folder to pick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickerMode {
    SaveFile,
    ChooseDir,
}

/// Who asked for the picker — decides what `Action::PickerConfirm` does
/// with the chosen path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickerTarget {
    /// Write the raw response body to the chosen file.
    SaveBody,
    /// Write the response pane's active view to the chosen file.
    SaveView,
    /// Open (or offer to create) a project at the chosen folder.
    OpenProject,
    /// Fill the New project modal's path field with the chosen folder.
    NewProjectDir,
}

impl PickerTarget {
    pub fn mode(self) -> PickerMode {
        match self {
            PickerTarget::SaveBody | PickerTarget::SaveView => PickerMode::SaveFile,
            PickerTarget::OpenProject | PickerTarget::NewProjectDir => PickerMode::ChooseDir,
        }
    }
}

/// One listed entry of the folder being shown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub is_dir: bool,
    /// A folder that holds a `project.toml` — opened directly rather than
    /// descended into when the picker is choosing a project.
    pub is_project: bool,
}

/// One row of the list: the parent link, or an entry index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Row {
    Parent,
    Entry(usize),
}

pub struct FilePickerState {
    title: String,
    target: PickerTarget,
    dir: PathBuf,
    input: LineInput,
    /// The name the field opened with (save mode): descending into a
    /// folder puts it back so the suggested name survives browsing.
    suggested: String,
    /// True once the user has edited the field: only then does its text
    /// filter the rows. The opening prefill never filters.
    filter_active: bool,
    /// True once the user has moved the selection (arrows, a click) since
    /// the rows were last rebuilt: Enter then acts on that row even when
    /// the field still holds the save prefill.
    row_chosen: bool,
    entries: Vec<Entry>,
    rows: Vec<Row>,
    selected: usize,
    scroll: usize,
    ensure_visible: bool,
    show_hidden: bool,
    /// The last folder that could not be listed, shown in the list area.
    error: Option<String>,
    /// Windows only in practice: the picker is showing the list of drive
    /// roots rather than a folder (what sits "above" `C:\`). `entries`
    /// then hold one folder per root, named by its full path.
    at_drives: bool,
}

/// Lists `dir`: directories first, then files, each group sorted
/// case-insensitively. Dotfiles (and Windows hidden-attribute entries) are
/// skipped unless `show_hidden`. In `ChooseDir` mode only directories are
/// returned.
pub fn list_dir(dir: &Path, show_hidden: bool, mode: PickerMode) -> std::io::Result<Vec<Entry>> {
    let mut entries = Vec::new();
    for item in std::fs::read_dir(dir)? {
        // One unreadable entry (a dangling symlink, a race with a delete)
        // is skipped rather than failing the whole listing.
        let Ok(item) = item else { continue };
        let name = item.file_name().to_string_lossy().into_owned();
        // `metadata` follows symlinks, so a link to a folder lists as one.
        let Ok(meta) = std::fs::metadata(item.path()) else {
            continue;
        };
        if !show_hidden && is_hidden(&name, platform_attributes(&meta)) {
            continue;
        }
        let is_dir = meta.is_dir();
        if mode == PickerMode::ChooseDir && !is_dir {
            continue;
        }
        let is_project = is_dir && postui_core::project::is_project(&item.path());
        entries.push(Entry {
            name,
            is_dir,
            is_project,
        });
    }
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.name.cmp(&b.name))
    });
    Ok(entries)
}

const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;

#[cfg(windows)]
fn platform_attributes(meta: &std::fs::Metadata) -> u32 {
    use std::os::windows::fs::MetadataExt;
    meta.file_attributes()
}

#[cfg(not(windows))]
fn platform_attributes(_meta: &std::fs::Metadata) -> u32 {
    0
}

/// Whether an entry with this name and these raw platform attributes is
/// hidden: a leading dot on every platform, or the Windows hidden bit.
pub fn is_hidden(name: &str, file_attributes: u32) -> bool {
    name.starts_with('.') || file_attributes & FILE_ATTRIBUTE_HIDDEN != 0
}

/// Resolves text typed into the field as a path jump: `~` and `~/…`
/// against `home`, absolute paths (and drive-letter paths on Windows) as
/// themselves, and anything containing a separator relative to `cwd`.
/// Plain names return `None` — they are filenames, not jumps.
pub fn resolve_jump(text: &str, cwd: &Path, home: Option<&Path>) -> Option<PathBuf> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    if text == "~" {
        return home.map(Path::to_path_buf);
    }
    if let Some(rest) = text.strip_prefix("~/").or_else(|| text.strip_prefix("~\\")) {
        return home.map(|h| h.join(rest));
    }
    let path = Path::new(text);
    if path.is_absolute() {
        return Some(path.to_path_buf());
    }
    if text.contains(std::path::is_separator) {
        return Some(cwd.join(path));
    }
    None
}

fn home_dir() -> Option<PathBuf> {
    directories::BaseDirs::new().map(|d| d.home_dir().to_path_buf())
}

/// The folder a save picker opens in: Downloads, else home, else `.`.
pub fn default_save_dir() -> PathBuf {
    directories::UserDirs::new()
        .and_then(|u| u.download_dir().map(Path::to_path_buf))
        .filter(|d| d.is_dir())
        .or_else(home_dir)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// `dir` itself when it is a folder, else its nearest existing ancestor,
/// else `.` — so a picker always opens somewhere real.
fn nearest_existing_dir(dir: &Path) -> PathBuf {
    let mut cur = dir;
    loop {
        if cur.is_dir() {
            return cur.to_path_buf();
        }
        match cur.parent() {
            Some(p) => cur = p,
            None => return PathBuf::from("."),
        }
    }
}

/// The mounted drive roots (`C:\`, `D:\`, …) on Windows; nothing anywhere
/// else, where `/` is the top and has no parent.
#[cfg(windows)]
fn drive_roots() -> Vec<PathBuf> {
    (b'A'..=b'Z')
        .map(|l| PathBuf::from(format!("{}:\\", l as char)))
        .filter(|p| p.is_dir())
        .collect()
}

#[cfg(not(windows))]
fn drive_roots() -> Vec<PathBuf> {
    Vec::new()
}

impl FilePickerState {
    pub fn new(title: &str, target: PickerTarget, start_dir: &Path, suggested: &str) -> Self {
        let mut picker = Self {
            title: title.to_string(),
            target,
            dir: PathBuf::new(),
            input: LineInput::new(suggested),
            suggested: suggested.to_string(),
            filter_active: false,
            row_chosen: false,
            entries: Vec::new(),
            rows: Vec::new(),
            selected: 0,
            scroll: 0,
            ensure_visible: true,
            show_hidden: false,
            error: None,
            at_drives: false,
        };
        picker.enter_dir(&nearest_existing_dir(start_dir));
        picker
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn target(&self) -> PickerTarget {
        self.target
    }

    pub fn mode(&self) -> PickerMode {
        self.target.mode()
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn input(&self) -> &LineInput {
        &self.input
    }

    pub fn input_mut(&mut self) -> &mut LineInput {
        &mut self.input
    }

    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    pub fn entry(&self, i: usize) -> Option<&Entry> {
        self.entries.get(i)
    }

    pub fn selected(&self) -> usize {
        self.selected
    }

    pub fn show_hidden(&self) -> bool {
        self.show_hidden
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub fn at_drives(&self) -> bool {
        self.at_drives
    }

    /// What the folder row shows: the folder (home folded to `~`), or
    /// "Drives" above a drive root.
    pub fn dir_label(&self) -> String {
        if self.at_drives {
            "Drives".to_string()
        } else {
            display_dir(&self.dir)
        }
    }

    /// Shows `roots` as the list (one folder row per drive, no parent
    /// row). Public so the Windows-only behavior can be exercised on
    /// every platform.
    pub fn show_drives(&mut self, roots: Vec<PathBuf>) {
        self.at_drives = true;
        self.entries = roots
            .into_iter()
            .map(|r| Entry {
                name: r.display().to_string(),
                is_dir: true,
                is_project: false,
            })
            .collect();
        self.error = None;
        self.input = LineInput::new("");
        self.filter_active = false;
        self.rebuild_rows();
    }

    /// The full path a row's entry names: under the folder shown, or the
    /// drive root itself in the drive view.
    fn entry_path(&self, entry: &Entry) -> PathBuf {
        if self.at_drives {
            PathBuf::from(&entry.name)
        } else {
            self.dir.join(&entry.name)
        }
    }

    /// The row labels in display order — what a test (or the footer)
    /// reads without touching the buffer.
    pub fn row_names(&self) -> Vec<String> {
        self.rows
            .iter()
            .map(|r| match r {
                Row::Parent => "..".to_string(),
                Row::Entry(i) => self.entries[*i].name.clone(),
            })
            .collect()
    }

    fn row_entry(&self, row: usize) -> Option<&Entry> {
        match self.rows.get(row)? {
            Row::Parent => None,
            Row::Entry(i) => self.entries.get(*i),
        }
    }

    /// Whether row `i` names the file the field would save over.
    pub fn row_will_overwrite(&self, i: usize) -> bool {
        self.mode() == PickerMode::SaveFile
            && self
                .row_entry(i)
                .is_some_and(|e| !e.is_dir && e.name == self.input.text().trim())
    }

    pub fn select(&mut self, i: usize) {
        self.selected = i.min(self.rows.len().saturating_sub(1));
        self.ensure_visible = true;
        self.row_chosen = true;
    }

    pub fn scroll_by(&mut self, delta: i16) {
        let max = self.rows.len().saturating_sub(1);
        self.scroll = (self.scroll as i64 + i64::from(delta)).clamp(0, max as i64) as usize;
        self.ensure_visible = false;
    }

    pub fn toggle_hidden(&mut self) {
        self.show_hidden = !self.show_hidden;
        if self.at_drives {
            return;
        }
        let keep = self.rows.get(self.selected).copied();
        let name = keep.and_then(|r| match r {
            Row::Parent => None,
            Row::Entry(i) => Some(self.entries[i].name.clone()),
        });
        self.relist();
        if let Some(name) = name {
            self.select_name(&name);
        }
    }

    /// The typed filter, when the user has edited the field.
    fn filter(&self) -> Option<&str> {
        let text = self.input.text().trim();
        (self.filter_active && !text.is_empty()).then_some(text)
    }

    fn rebuild_rows(&mut self) {
        let filter = self.filter().map(str::to_string);
        self.rows.clear();
        match filter {
            None => {
                if !self.at_drives && self.dir.parent().is_some() {
                    self.rows.push(Row::Parent);
                }
                self.rows.extend((0..self.entries.len()).map(Row::Entry));
            }
            Some(q) => {
                // Names match by substring, not fuzzily: a filename filter
                // that pulls in every name sharing a few letters is noise.
                // Prefix matches first so the obvious candidate sits on
                // top, then the rest in list order.
                let lower = q.to_lowercase();
                let (prefix, rest): (Vec<usize>, Vec<usize>) = (0..self.entries.len())
                    .filter(|&i| self.entries[i].name.to_lowercase().contains(&lower))
                    .partition(|&i| self.entries[i].name.to_lowercase().starts_with(&lower));
                self.rows
                    .extend(prefix.into_iter().chain(rest).map(Row::Entry));
            }
        }
        self.selected = 0;
        self.scroll = 0;
        self.ensure_visible = true;
        self.row_chosen = false;
    }

    /// Parks the selection on `name` after a relist (the folder just
    /// left, the row under the hidden toggle) — a cursor courtesy, not a
    /// choice, so it leaves `row_chosen` alone.
    fn select_name(&mut self, name: &str) {
        if let Some(pos) = self
            .rows
            .iter()
            .position(|r| matches!(r, Row::Entry(i) if self.entries[*i].name == name))
        {
            self.selected = pos;
            self.ensure_visible = true;
        }
    }

    fn relist(&mut self) {
        match list_dir(&self.dir, self.show_hidden, self.mode()) {
            Ok(entries) => {
                self.entries = entries;
                self.error = None;
            }
            Err(e) => {
                self.entries.clear();
                self.error = Some(format!("cannot list {}: {e}", self.dir.display()));
            }
        }
        self.rebuild_rows();
    }

    /// Re-lists `dir` and shows it; on failure keeps the current folder
    /// and records the error.
    pub fn enter_dir(&mut self, dir: &Path) {
        match list_dir(dir, self.show_hidden, self.mode()) {
            Ok(entries) => {
                self.dir = dir.to_path_buf();
                self.entries = entries;
                self.error = None;
                self.at_drives = false;
                // Browsing resets the field: the suggested name in save
                // mode, nothing in folder mode. Either way the filter is
                // off again so the whole folder shows.
                self.input = LineInput::new(&self.suggested);
                self.filter_active = false;
                self.rebuild_rows();
            }
            Err(e) => {
                self.error = Some(format!("cannot open {}: {e}", dir.display()));
            }
        }
    }

    /// Climbs one level; a no-op at a filesystem root.
    pub fn climb(&mut self) {
        if self.at_drives {
            return;
        }
        let Some(parent) = self.dir.parent().map(Path::to_path_buf) else {
            // Above a drive root there is the list of drives (Windows);
            // above `/` there is nothing.
            let roots = drive_roots();
            if !roots.is_empty() {
                let leaving = self.dir.clone();
                self.show_drives(roots);
                self.select_name(&leaving.display().to_string());
            }
            return;
        };
        let child = self
            .dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned());
        self.enter_dir(&parent);
        if let Some(child) = child
            && self.dir == parent
        {
            self.select_name(&child);
        }
    }

    /// A folder choice closes the picker outright. A save leaves it open:
    /// the app closes it once the write goes ahead, so a declined
    /// overwrite question lands back on the picker with nothing lost.
    fn confirm(&self, path: PathBuf) -> Option<ModalResult> {
        Some(ModalResult {
            actions: vec![Action::PickerConfirm {
                target: self.target,
                path,
            }],
            close: self.mode() == PickerMode::ChooseDir,
            ..Default::default()
        })
    }

    /// A folder row chosen in `ChooseDir` mode: a project opens outright
    /// when a project is what's wanted; anything else is browsed into.
    fn choose_folder(&mut self, path: PathBuf, is_project: bool) -> Option<ModalResult> {
        if is_project && self.target == PickerTarget::OpenProject {
            return self.confirm(path);
        }
        self.enter_dir(&path);
        None
    }

    /// Enter: act on the selected row, or confirm the field's name.
    pub fn activate(&mut self) -> Option<ModalResult> {
        let text = self.input.text().trim().to_string();
        // A typed path (absolute, `~`, or with a separator) is a jump
        // when it names a folder, and in save mode a full target when it
        // doesn't.
        if let Some(path) = resolve_jump(&text, &self.dir, home_dir().as_deref()) {
            if path.is_dir() {
                self.enter_dir(&path);
                return None;
            }
            return match self.mode() {
                PickerMode::SaveFile => self.confirm(path),
                PickerMode::ChooseDir => {
                    self.error = Some(format!("{} is not a folder", path.display()));
                    None
                }
            };
        }
        // With the opening prefill untouched and no row reached for, the
        // field is the answer in save mode: Enter saves that name here.
        if self.mode() == PickerMode::SaveFile
            && !self.filter_active
            && !self.row_chosen
            && !text.is_empty()
        {
            return self.confirm(self.dir.join(&text));
        }
        match self.rows.get(self.selected).copied() {
            Some(Row::Parent) => {
                self.climb();
                None
            }
            Some(Row::Entry(i)) => {
                let entry = self.entries[i].clone();
                let path = self.entry_path(&entry);
                if !entry.is_dir {
                    self.confirm(path)
                } else if self.mode() == PickerMode::ChooseDir {
                    self.choose_folder(path, entry.is_project)
                } else {
                    self.enter_dir(&path);
                    None
                }
            }
            None => match self.mode() {
                PickerMode::SaveFile if !text.is_empty() => self.confirm(self.dir.join(&text)),
                _ => None,
            },
        }
    }

    /// The primary button / alt+enter: confirm the folder being shown
    /// (`ChooseDir`) or the field's name inside it (`SaveFile`).
    pub fn confirm_here(&mut self) -> Option<ModalResult> {
        match self.mode() {
            PickerMode::ChooseDir => self.confirm(self.dir.clone()),
            PickerMode::SaveFile => {
                let text = self.input.text().trim().to_string();
                if text.is_empty() {
                    return None;
                }
                match resolve_jump(&text, &self.dir, home_dir().as_deref()) {
                    Some(path) if path.is_dir() => {
                        self.enter_dir(&path);
                        None
                    }
                    Some(path) => self.confirm(path),
                    None => self.confirm(self.dir.join(&text)),
                }
            }
        }
    }

    /// Pastes into the field; a pasted path that names a folder jumps
    /// there at once.
    pub fn paste(&mut self, text: &str) {
        let flat = super::line_input::flatten_paste(text);
        if let Some(path) = resolve_jump(&flat, &self.dir, home_dir().as_deref())
            && path.is_dir()
        {
            self.enter_dir(&path);
            return;
        }
        self.input.paste(&flat);
        self.filter_active = true;
        self.rebuild_rows();
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Option<ModalResult> {
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        match key.code {
            KeyCode::Esc => {
                return Some(ModalResult {
                    close: true,
                    ..Default::default()
                });
            }
            KeyCode::Enter if alt => return self.confirm_here(),
            KeyCode::Enter => return self.activate(),
            KeyCode::Char('h') if alt => self.toggle_hidden(),
            KeyCode::Up => {
                self.selected = self.selected.saturating_sub(1);
                self.ensure_visible = true;
                self.row_chosen = true;
            }
            KeyCode::Down => {
                if self.selected + 1 < self.rows.len() {
                    self.selected += 1;
                }
                self.ensure_visible = true;
                self.row_chosen = true;
            }
            KeyCode::Backspace if self.input.text().is_empty() => self.climb(),
            _ => {
                let before = self.input.text().to_string();
                self.input.handle_key(key);
                if self.input.text() != before {
                    self.filter_active = true;
                    self.rebuild_rows();
                }
            }
        }
        None
    }

    fn primary_label(&self) -> &'static str {
        match self.mode() {
            PickerMode::SaveFile => "Save",
            PickerMode::ChooseDir => "Open this folder",
        }
    }

    pub fn draw(
        &mut self,
        frame: &mut Frame,
        screen: Rect,
        theme: &Theme,
        hits: &mut crate::hit::HitMap,
        hovered: Option<&crate::hit::Hit>,
        t: f32,
    ) {
        use crate::hit::Hit;
        use crate::paint::button::{BUTTON_HEIGHT, Button, ButtonKind};

        let width = 64.min(screen.width);
        // Chrome: 1 pad + title + folder row + 1 gap + 3-row field + 1 gap
        // + list + 1 gap + 3-row buttons + 1 pad.
        const CHROME: u16 = 1 + 1 + 1 + 1 + FIELD_HEIGHT + 1 + 1 + 3 + 1;
        let content_rows = (self.rows.len() as u16).clamp(1, 12);
        let height = (CHROME + content_rows).min(screen.height);
        let area = super::modal::centered_rect(screen, width, height);
        hits.register(area, Hit::ModalBody);
        paint::floating_panel_settling(frame.buffer_mut(), area, screen, theme, t);
        if t < 1.0 {
            return;
        }

        let right = area.x + area.width;
        let title_y = area.y + 1;
        paint::text(
            frame.buffer_mut(),
            area.x + 2,
            title_y,
            &self.title,
            theme.text,
            theme.panel,
            true,
        );
        // Title-row toggle, mirrored by alt+h.
        let toggle = if self.show_hidden {
            "hidden \u{25CF}"
        } else {
            "hidden \u{25CB}"
        };
        let tw = toggle.chars().count() as u16;
        let tx = right.saturating_sub(tw + 2);
        let toggle_fg = if hovered == Some(&Hit::PickerHidden) {
            theme.text
        } else {
            theme.accent
        };
        paint::text(
            frame.buffer_mut(),
            tx,
            title_y,
            toggle,
            toggle_fg,
            theme.panel,
            false,
        );
        hits.register(Rect::new(tx, title_y, tw, 1), Hit::PickerHidden);

        // The folder being shown, clipped from the left so the deepest
        // part (what the user just chose) stays visible.
        let folder_y = title_y + 1;
        let folder_w = area.width.saturating_sub(4) as usize;
        let shown = self.dir_label();
        let shown: String = if shown.chars().count() > folder_w {
            let tail: String = shown
                .chars()
                .rev()
                .take(folder_w.saturating_sub(1))
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            format!("\u{2026}{tail}")
        } else {
            shown
        };
        paint::text(
            frame.buffer_mut(),
            area.x + 2,
            folder_y,
            &shown,
            theme.text_muted,
            theme.panel,
            false,
        );

        let field_area = Rect {
            x: area.x + 1,
            y: folder_y + 2,
            width: area.width.saturating_sub(2),
            height: FIELD_HEIGHT,
        };
        TextField {
            content: self
                .input
                .draw_line_windowed(true, theme, field_area.width.saturating_sub(2)),
            state: ControlState::Focused,
        }
        .paint(frame.buffer_mut(), field_area, theme);
        hits.register(field_area, Hit::ModalInput(0));

        let buttons_y = area.y + area.height.saturating_sub(1 + BUTTON_HEIGHT);
        let list_area = Rect {
            x: area.x + 1,
            y: field_area.y + FIELD_HEIGHT + 1,
            width: area.width.saturating_sub(2),
            height: buttons_y.saturating_sub(1 + field_area.y + FIELD_HEIGHT + 1),
        };
        let list_h = list_area.height as usize;
        if self.ensure_visible && list_h > 0 {
            if self.selected < self.scroll {
                self.scroll = self.selected;
            } else if self.selected >= self.scroll + list_h {
                self.scroll = self.selected + 1 - list_h;
            }
            self.scroll = self.scroll.min(self.rows.len().saturating_sub(list_h));
            self.ensure_visible = false;
        }

        if let Some(err) = &self.error {
            paint::text(
                frame.buffer_mut(),
                list_area.x + 1,
                list_area.y,
                super::chooser::clip(err, list_area.width.saturating_sub(2)),
                theme.warning,
                theme.panel,
                false,
            );
        }
        let rows_y = list_area.y + u16::from(self.error.is_some());
        let rows_h = list_h.saturating_sub(usize::from(self.error.is_some()));
        let hover_t = 1.0;
        for (i, row) in self.rows.iter().enumerate().skip(self.scroll).take(rows_h) {
            let y = rows_y + (i - self.scroll) as u16;
            let selected = i == self.selected;
            let highlight = if selected {
                RowHighlight::Selected
            } else if hovered == Some(&Hit::PickerRow(i)) {
                RowHighlight::Hover
            } else {
                RowHighlight::None
            };
            ListRow {
                highlight,
                zebra: None,
            }
            .paint(
                frame.buffer_mut(),
                y,
                list_area.x,
                list_area.width,
                theme.panel,
                hover_t,
                theme,
            );
            let fill = ListRow::resolve_fill(theme, highlight, theme.panel, hover_t);
            let (icon, label, tag): (&str, String, Option<(&str, ratatui::style::Color)>) =
                match row {
                    Row::Parent => ("  ", "..".to_string(), None),
                    Row::Entry(e) => {
                        let entry = &self.entries[*e];
                        if entry.is_dir {
                            let tag = entry.is_project.then_some(("project", theme.accent));
                            let label = if self.at_drives {
                                entry.name.clone()
                            } else {
                                format!("{}{}", entry.name, std::path::MAIN_SEPARATOR)
                            };
                            ("\u{F024B} ", label, tag)
                        } else {
                            let tag = self
                                .row_will_overwrite(i)
                                .then_some(("will overwrite", theme.warning));
                            ("  ", entry.name.clone(), tag)
                        }
                    }
                };
            let x = list_area.x + 1;
            paint::text(
                frame.buffer_mut(),
                x,
                y,
                icon,
                theme.text_muted,
                fill,
                false,
            );
            let label_x = x + 2;
            let tag_w = tag.map_or(0, |(s, _)| s.chars().count() as u16 + 1);
            let label_w = (list_area.x + list_area.width).saturating_sub(label_x + tag_w + 1);
            paint::text(
                frame.buffer_mut(),
                label_x,
                y,
                super::chooser::clip(&label, label_w),
                theme.text,
                fill,
                selected,
            );
            if let Some((s, color)) = tag {
                let tx = (list_area.x + list_area.width).saturating_sub(tag_w);
                paint::text(frame.buffer_mut(), tx, y, s, color, fill, false);
            }
            hits.register(
                Rect::new(list_area.x, y, list_area.width, 1),
                Hit::PickerRow(i),
            );
        }

        // Buttons: Cancel then the primary, right-aligned like every
        // other modal's row. The primary is its own hit because it must
        // not synthesize Enter (Enter acts on the selected row).
        let buttons = [
            ("Cancel", ButtonKind::Secondary, Hit::ModalCancel),
            (
                self.primary_label(),
                ButtonKind::Primary,
                Hit::PickerPrimary,
            ),
        ];
        let row_w: u16 = buttons
            .iter()
            .map(|(l, ..)| paint::button_min_width(l))
            .sum::<u16>()
            + 2;
        let mut x = right.saturating_sub(2 + row_w);
        for (label, kind, hit) in buttons {
            let w = paint::button_min_width(label);
            let btn = Rect::new(x, buttons_y, w, BUTTON_HEIGHT);
            let state = if hovered == Some(&hit) {
                ControlState::Hover
            } else {
                ControlState::Normal
            };
            Button { label, kind, state }.paint(frame.buffer_mut(), btn, theme);
            hits.register(btn, hit);
            x += w + 2;
        }
    }
}

/// The folder as shown on the picker: the home prefix folded to `~` so a
/// long personal path reads short, native separators otherwise.
fn display_dir(dir: &Path) -> String {
    if let Some(home) = home_dir()
        && let Ok(rest) = dir.strip_prefix(&home)
    {
        if rest.as_os_str().is_empty() {
            return "~".to_string();
        }
        return format!("~{}{}", std::path::MAIN_SEPARATOR, rest.display());
    }
    dir.display().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn alt(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::ALT)
    }

    fn type_str(p: &mut FilePickerState, s: &str) {
        for ch in s.chars() {
            p.handle_key(key(KeyCode::Char(ch)));
        }
    }

    /// A tree: `beta/`, `Alpha/`, `.hidden/`, `proj/project.toml`,
    /// `zed.txt`, `apple.json`, `.secret`.
    fn tree() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        std::fs::create_dir(p.join("beta")).unwrap();
        std::fs::create_dir(p.join("Alpha")).unwrap();
        std::fs::create_dir(p.join(".hidden")).unwrap();
        std::fs::create_dir(p.join("proj")).unwrap();
        std::fs::write(p.join("proj/project.toml"), "").unwrap();
        std::fs::write(p.join("zed.txt"), "").unwrap();
        std::fs::write(p.join("apple.json"), "").unwrap();
        std::fs::write(p.join(".secret"), "").unwrap();
        dir
    }

    fn confirmed_path(res: Option<ModalResult>) -> (PickerTarget, PathBuf) {
        let res = res.expect("a result");
        // A folder choice closes the picker; a save leaves it open for the
        // app to close once the write goes ahead (an overwrite may still
        // be declined).
        let save = matches!(
            res.actions.first(),
            Some(Action::PickerConfirm {
                target: PickerTarget::SaveBody | PickerTarget::SaveView,
                ..
            })
        );
        assert_eq!(res.close, !save, "close flag follows the mode");
        match res.actions.as_slice() {
            [Action::PickerConfirm { target, path }] => (*target, path.clone()),
            other => panic!("expected PickerConfirm, got {other:?}"),
        }
    }

    #[test]
    fn list_dir_sorts_directories_first_case_insensitively_and_hides_dotfiles() {
        let dir = tree();
        let names: Vec<(String, bool)> = list_dir(dir.path(), false, PickerMode::SaveFile)
            .unwrap()
            .into_iter()
            .map(|e| (e.name, e.is_dir))
            .collect();
        assert_eq!(
            names,
            vec![
                ("Alpha".to_string(), true),
                ("beta".to_string(), true),
                ("proj".to_string(), true),
                ("apple.json".to_string(), false),
                ("zed.txt".to_string(), false),
            ]
        );
    }

    #[test]
    fn list_dir_marks_project_folders() {
        let dir = tree();
        let entries = list_dir(dir.path(), false, PickerMode::ChooseDir).unwrap();
        let proj = entries.iter().find(|e| e.name == "proj").unwrap();
        assert!(proj.is_project);
        assert!(
            !entries
                .iter()
                .find(|e| e.name == "beta")
                .unwrap()
                .is_project
        );
    }

    #[test]
    fn list_dir_in_choose_dir_mode_lists_only_directories() {
        let dir = tree();
        let entries = list_dir(dir.path(), false, PickerMode::ChooseDir).unwrap();
        assert!(entries.iter().all(|e| e.is_dir), "{entries:?}");
        assert_eq!(entries.len(), 3);
    }

    #[test]
    fn show_hidden_includes_dotfiles() {
        let dir = tree();
        let names: Vec<String> = list_dir(dir.path(), true, PickerMode::SaveFile)
            .unwrap()
            .into_iter()
            .map(|e| e.name)
            .collect();
        assert_eq!(names[0], ".hidden");
        assert!(names.contains(&".secret".to_string()));
    }

    #[test]
    fn is_hidden_by_leading_dot_or_windows_attribute() {
        const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
        assert!(is_hidden(".git", 0));
        assert!(!is_hidden("src", 0));
        assert!(is_hidden("desktop.ini", FILE_ATTRIBUTE_HIDDEN));
        assert!(!is_hidden("readme.md", 0x20)); // FILE_ATTRIBUTE_ARCHIVE alone
    }

    #[test]
    fn resolve_jump_handles_tilde_absolute_and_relative_paths() {
        let cwd = Path::new("/work");
        let home = Path::new("/home/me");
        assert_eq!(
            resolve_jump("~", cwd, Some(home)),
            Some(PathBuf::from("/home/me"))
        );
        assert_eq!(
            resolve_jump("~/dl", cwd, Some(home)),
            Some(PathBuf::from("/home/me/dl"))
        );
        assert_eq!(
            resolve_jump("/etc", cwd, Some(home)),
            Some(PathBuf::from("/etc"))
        );
        assert_eq!(
            resolve_jump("sub/x.json", cwd, Some(home)),
            Some(PathBuf::from("/work/sub/x.json"))
        );
        assert_eq!(resolve_jump("x.json", cwd, Some(home)), None);
        assert_eq!(resolve_jump("", cwd, Some(home)), None);
        // No home known: a tilde is not a jump.
        assert_eq!(resolve_jump("~/dl", cwd, None), None);
    }

    #[test]
    fn dir_mode_opens_with_a_parent_row_then_folders() {
        let dir = tree();
        let p = FilePickerState::new("Open project", PickerTarget::OpenProject, dir.path(), "");
        assert_eq!(p.row_names(), vec!["..", "Alpha", "beta", "proj"]);
        assert_eq!(p.selected(), 0);
        assert_eq!(p.dir(), dir.path());
    }

    #[test]
    fn enter_on_a_folder_descends_and_backspace_on_an_empty_field_climbs() {
        let dir = tree();
        let mut p = FilePickerState::new("Open project", PickerTarget::OpenProject, dir.path(), "");
        p.handle_key(key(KeyCode::Down)); // Alpha
        assert!(p.handle_key(key(KeyCode::Enter)).is_none());
        assert_eq!(p.dir(), dir.path().join("Alpha"));
        assert_eq!(p.row_names(), vec![".."]);

        assert!(p.handle_key(key(KeyCode::Backspace)).is_none());
        assert_eq!(p.dir(), dir.path());
        // Climbing lands the selection back on the folder just left.
        assert_eq!(p.row_names()[p.selected()], "Alpha");
    }

    #[test]
    fn enter_on_the_parent_row_climbs() {
        let dir = tree();
        let start = dir.path().join("beta");
        let mut p = FilePickerState::new("Open project", PickerTarget::OpenProject, &start, "");
        assert_eq!(p.selected(), 0);
        assert!(p.handle_key(key(KeyCode::Enter)).is_none());
        assert_eq!(p.dir(), dir.path());
    }

    #[test]
    fn typing_filters_rows_and_hides_the_parent_row() {
        let dir = tree();
        let mut p = FilePickerState::new("Save", PickerTarget::SaveBody, dir.path(), "");
        type_str(&mut p, "ap");
        assert_eq!(p.row_names(), vec!["apple.json"]);
        assert_eq!(p.selected(), 0);
        // Substring, not just prefix — and not fuzzy: "zt" matches nothing.
        p.handle_key(key(KeyCode::Backspace));
        p.handle_key(key(KeyCode::Backspace));
        type_str(&mut p, "ed");
        assert_eq!(p.row_names(), vec!["zed.txt"]);
        p.handle_key(key(KeyCode::Backspace));
        p.handle_key(key(KeyCode::Backspace));
        type_str(&mut p, "zt");
        assert!(p.row_names().is_empty());
        // Clearing the field brings the parent row back.
        p.handle_key(key(KeyCode::Backspace));
        p.handle_key(key(KeyCode::Backspace));
        assert_eq!(p.row_names()[0], "..");
    }

    #[test]
    fn enter_on_a_project_folder_confirms_it_when_choosing_a_project() {
        let dir = tree();
        let mut p = FilePickerState::new("Open project", PickerTarget::OpenProject, dir.path(), "");
        type_str(&mut p, "proj");
        let (target, path) = confirmed_path(p.handle_key(key(KeyCode::Enter)));
        assert_eq!(target, PickerTarget::OpenProject);
        assert_eq!(path, dir.path().join("proj"));
    }

    #[test]
    fn enter_on_a_project_folder_descends_when_choosing_a_parent_folder() {
        let dir = tree();
        let mut p =
            FilePickerState::new("Choose folder", PickerTarget::NewProjectDir, dir.path(), "");
        type_str(&mut p, "proj");
        assert!(p.handle_key(key(KeyCode::Enter)).is_none());
        assert_eq!(p.dir(), dir.path().join("proj"));
    }

    #[test]
    fn alt_enter_confirms_the_folder_being_shown() {
        let dir = tree();
        let mut p = FilePickerState::new("Open project", PickerTarget::OpenProject, dir.path(), "");
        let (_, path) = confirmed_path(p.handle_key(alt(KeyCode::Enter)));
        assert_eq!(path, dir.path());
    }

    #[test]
    fn save_mode_prefill_does_not_filter_and_flags_the_file_it_would_overwrite() {
        let dir = tree();
        let p = FilePickerState::new("Save", PickerTarget::SaveBody, dir.path(), "apple.json");
        assert_eq!(p.input().text(), "apple.json");
        assert_eq!(
            p.row_names(),
            vec!["..", "Alpha", "beta", "proj", "apple.json", "zed.txt"]
        );
        let apple = p
            .row_names()
            .iter()
            .position(|n| n == "apple.json")
            .unwrap();
        assert!(p.row_will_overwrite(apple));
        assert!(!p.row_will_overwrite(apple + 1));
    }

    #[test]
    fn save_mode_enter_confirms_the_field_name_inside_the_folder_shown() {
        let dir = tree();
        let mut p = FilePickerState::new("Save", PickerTarget::SaveBody, dir.path(), "out.json");
        let (target, path) = confirmed_path(p.handle_key(key(KeyCode::Enter)));
        assert_eq!(target, PickerTarget::SaveBody);
        assert_eq!(path, dir.path().join("out.json"));
    }

    /// The prefill is the answer only until the user reaches for a row:
    /// moving the selection onto a folder and pressing Enter browses into
    /// it, keeping the suggested name for the save that follows.
    #[test]
    fn save_mode_enter_on_a_folder_the_user_selected_descends() {
        let dir = tree();
        let mut p = FilePickerState::new("Save", PickerTarget::SaveBody, dir.path(), "out.json");
        p.handle_key(key(KeyCode::Down)); // Alpha
        assert!(p.handle_key(key(KeyCode::Enter)).is_none());
        assert_eq!(p.dir(), dir.path().join("Alpha"));
        assert_eq!(p.input().text(), "out.json");
        // Back at the top of the fresh listing, Enter is the save again.
        let (_, path) = confirmed_path(p.handle_key(key(KeyCode::Enter)));
        assert_eq!(path, dir.path().join("Alpha").join("out.json"));
    }

    /// A click that lands the selection on a file row makes that row the
    /// answer; a click on a folder row browses.
    #[test]
    fn save_mode_selected_file_row_wins_over_the_prefill() {
        let dir = tree();
        let mut p = FilePickerState::new("Save", PickerTarget::SaveBody, dir.path(), "out.json");
        let zed = p.row_names().iter().position(|n| n == "zed.txt").unwrap();
        p.select(zed);
        let (_, path) = confirmed_path(p.activate());
        assert_eq!(path, dir.path().join("zed.txt"));
    }

    #[test]
    fn save_mode_enter_with_an_empty_field_confirms_nothing() {
        let dir = tree();
        let mut p = FilePickerState::new("Save", PickerTarget::SaveBody, dir.path(), "");
        // Selection sits on the parent row; Enter climbs rather than saving.
        p.handle_key(key(KeyCode::Down));
        p.handle_key(key(KeyCode::Down)); // beta
        assert!(p.handle_key(key(KeyCode::Enter)).is_none());
        assert_eq!(p.dir(), dir.path().join("beta"));
        assert!(p.handle_key(key(KeyCode::Enter)).is_none()); // ".." row
        assert_eq!(p.dir(), dir.path());
    }

    #[test]
    fn save_mode_enter_on_a_filtered_file_row_uses_that_rows_name() {
        let dir = tree();
        let mut p = FilePickerState::new("Save", PickerTarget::SaveView, dir.path(), "out.json");
        p.input_mut().select_all();
        type_str(&mut p, "zed");
        assert_eq!(p.row_names(), vec!["zed.txt"]);
        let (_, path) = confirmed_path(p.handle_key(key(KeyCode::Enter)));
        assert_eq!(path, dir.path().join("zed.txt"));
    }

    #[test]
    fn save_mode_descending_puts_the_suggested_name_back() {
        let dir = tree();
        let mut p = FilePickerState::new("Save", PickerTarget::SaveBody, dir.path(), "out.json");
        p.input_mut().select_all();
        type_str(&mut p, "bet");
        assert_eq!(p.row_names(), vec!["beta"]);
        assert!(p.handle_key(key(KeyCode::Enter)).is_none());
        assert_eq!(p.dir(), dir.path().join("beta"));
        assert_eq!(p.input().text(), "out.json");
        let (_, path) = confirmed_path(p.handle_key(key(KeyCode::Enter)));
        assert_eq!(path, dir.path().join("beta/out.json"));
    }

    #[test]
    fn a_typed_absolute_folder_path_jumps_there_on_enter() {
        let dir = tree();
        let elsewhere = tempfile::tempdir().unwrap();
        let mut p = FilePickerState::new("Save", PickerTarget::SaveBody, dir.path(), "");
        type_str(&mut p, &elsewhere.path().to_string_lossy());
        assert!(p.handle_key(key(KeyCode::Enter)).is_none());
        assert_eq!(p.dir(), elsewhere.path());
        assert_eq!(p.input().text(), "");
    }

    #[test]
    fn a_typed_relative_file_path_saves_under_the_folder_shown() {
        let dir = tree();
        let mut p = FilePickerState::new("Save", PickerTarget::SaveBody, dir.path(), "");
        type_str(&mut p, "beta/out.json");
        let (_, path) = confirmed_path(p.handle_key(key(KeyCode::Enter)));
        assert_eq!(path, dir.path().join("beta").join("out.json"));
    }

    #[test]
    fn pasting_a_folder_path_jumps_at_once() {
        let dir = tree();
        let mut p = FilePickerState::new("Open project", PickerTarget::OpenProject, dir.path(), "");
        p.paste(&dir.path().join("beta").to_string_lossy());
        assert_eq!(p.dir(), dir.path().join("beta"));
        assert_eq!(p.input().text(), "");
        // A pasted plain name just lands in the field.
        p.paste("hello");
        assert_eq!(p.input().text(), "hello");
        assert_eq!(p.dir(), dir.path().join("beta"));
    }

    #[test]
    fn hidden_toggle_relists_and_keeps_the_folder() {
        let dir = tree();
        let mut p = FilePickerState::new("Open project", PickerTarget::OpenProject, dir.path(), "");
        assert!(!p.show_hidden());
        p.handle_key(alt(KeyCode::Char('h')));
        assert!(p.show_hidden());
        assert_eq!(
            p.row_names(),
            vec!["..", ".hidden", "Alpha", "beta", "proj"]
        );
        p.toggle_hidden();
        assert_eq!(p.row_names(), vec!["..", "Alpha", "beta", "proj"]);
    }

    #[cfg(unix)]
    #[test]
    fn an_unreadable_folder_keeps_the_current_one_and_records_the_error() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tree();
        let locked = dir.path().join("beta");
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
        let mut p = FilePickerState::new("Open project", PickerTarget::OpenProject, dir.path(), "");
        p.enter_dir(&locked);
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
        if p.dir() == locked {
            // Running as root: nothing is unreadable. Not a failure.
            return;
        }
        assert_eq!(p.dir(), dir.path());
        assert!(
            p.error().is_some_and(|e| e.contains("beta")),
            "{:?}",
            p.error()
        );
    }

    #[test]
    fn a_missing_start_folder_falls_back_to_its_nearest_existing_parent() {
        let dir = tree();
        let missing = dir.path().join("beta").join("nope").join("deeper");
        let p = FilePickerState::new("Save", PickerTarget::SaveBody, &missing, "x.json");
        assert_eq!(p.dir(), dir.path().join("beta"));
    }

    /// Above a drive root (Windows) the picker lists the drives instead
    /// of a parent: exercised here through the injectable root list so
    /// the logic runs on every platform.
    #[test]
    fn climbing_above_a_root_shows_the_drive_list_and_entering_one_leaves_it() {
        let dir = tree();
        let mut p = FilePickerState::new("Open project", PickerTarget::OpenProject, dir.path(), "");
        p.show_drives(vec![
            dir.path().to_path_buf(),
            PathBuf::from("/nonexistent-drive"),
        ]);
        assert!(p.at_drives());
        assert!(p.dir_label().contains("Drives"));
        let names = p.row_names();
        assert_eq!(names.len(), 2, "no parent row above the drives: {names:?}");
        assert_eq!(names[0], dir.path().display().to_string());
        // Backspace / climb is a no-op at the top.
        p.climb();
        assert!(p.at_drives());
        // Entering a listed root leaves the drive view.
        assert!(p.handle_key(key(KeyCode::Enter)).is_none());
        assert!(!p.at_drives());
        assert_eq!(p.dir(), dir.path());
        assert_eq!(p.row_names()[0], "..");
    }

    #[test]
    fn a_filesystem_root_has_no_parent_row() {
        let root = if cfg!(windows) { "C:\\" } else { "/" };
        let mut p = FilePickerState::new(
            "Open project",
            PickerTarget::OpenProject,
            Path::new(root),
            "",
        );
        assert_ne!(p.row_names().first().map(String::as_str), Some(".."));
        p.climb();
        // Unix has no drives: climbing stays put. Windows shows the drives.
        assert_eq!(p.at_drives(), cfg!(windows));
    }

    #[test]
    fn esc_closes_with_no_actions() {
        let dir = tree();
        let mut p = FilePickerState::new("Save", PickerTarget::SaveBody, dir.path(), "x");
        let res = p.handle_key(key(KeyCode::Esc)).unwrap();
        assert!(res.close);
        assert!(res.actions.is_empty());
    }

    #[test]
    fn up_and_down_clamp_and_click_select_lands_on_the_row() {
        let dir = tree();
        let mut p = FilePickerState::new("Open project", PickerTarget::OpenProject, dir.path(), "");
        p.handle_key(key(KeyCode::Up));
        assert_eq!(p.selected(), 0);
        for _ in 0..10 {
            p.handle_key(key(KeyCode::Down));
        }
        assert_eq!(p.selected(), 3);
        p.select(1);
        assert_eq!(p.selected(), 1);
        p.select(99);
        assert_eq!(p.selected(), 3);
    }

    #[test]
    fn draw_shows_title_folder_field_rows_buttons_and_the_overwrite_tag() {
        let dir = tree();
        let mut p = FilePickerState::new(
            "Save response body",
            PickerTarget::SaveBody,
            dir.path(),
            "apple.json",
        );
        let theme = Theme::dark();
        let backend = TestBackend::new(80, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal
            .draw(|f| p.draw(f, f.area(), &theme, &mut hits, None, 1.0))
            .unwrap();
        let content = format!("{:?}", terminal.backend().buffer());
        let folder = dir
            .path()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .to_string();
        for needle in [
            "Save response body",
            folder.as_str(),
            "apple.json",
            "zed.txt",
            "beta",
            "will overwrite",
            "Save",
            "Cancel",
            "hidden",
        ] {
            assert!(content.contains(needle), "missing {needle:?}: {content}");
        }
        assert!(hits.rect_of(&crate::hit::Hit::PickerRow(0)).is_some());
        assert!(hits.rect_of(&crate::hit::Hit::PickerPrimary).is_some());
        assert!(hits.rect_of(&crate::hit::Hit::ModalCancel).is_some());
        assert!(hits.rect_of(&crate::hit::Hit::PickerHidden).is_some());
        assert!(hits.rect_of(&crate::hit::Hit::ModalInput(0)).is_some());
    }

    #[test]
    fn dir_mode_primary_button_reads_open_this_folder() {
        let dir = tree();
        let mut p = FilePickerState::new("Open project", PickerTarget::OpenProject, dir.path(), "");
        let theme = Theme::dark();
        let backend = TestBackend::new(80, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal
            .draw(|f| p.draw(f, f.area(), &theme, &mut hits, None, 1.0))
            .unwrap();
        let content = format!("{:?}", terminal.backend().buffer());
        assert!(content.contains("Open this folder"), "{content}");
        assert!(!content.contains("zed.txt"), "files hidden in dir mode");
        assert!(content.contains("project"), "project marker: {content}");
    }
}

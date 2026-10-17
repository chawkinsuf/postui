use crate::theme::Theme;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use std::ops::Range;

thread_local! {
    /// Set while a key the vim engine handed back is routed on to a dialog
    /// or a query box (plan M3b Deviation 6).
    static QUIET: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// While alive, [`LineInput::handle_key`] types nothing and moves nothing:
/// the engine owns the text, and a key it handed back reaches only the
/// container's own arms (Enter, Esc, Tab, the picker moves, alt chords).
/// Ends with its scope, restoring what was there before.
pub(crate) struct Quiet(bool);

pub(crate) fn quiet() -> Quiet {
    Quiet(QUIET.replace(true))
}

impl Drop for Quiet {
    fn drop(&mut self) {
        QUIET.set(self.0);
    }
}

/// A single-line, unicode-safe text input. Cursor position is a char index
/// (not a byte offset), so operations stay correct with multi-byte
/// characters. Shared by every single-line field in the app: the URL
/// line, table cells, the jq and search bars, modal fields, the filter
/// pickers, Settings edits and the Variable Manager's fields.
///
/// It keeps its own step history while an edit is open (spec 2026-09-15,
/// "Undo inside a text field"): a run of typed characters, a run of
/// deletions in one direction, or a single paste/replace is one step;
/// a caret move or a change of run kind starts a new one. The owner
/// calls [`Self::end_edit`] when the field closes, and the app history
/// takes over from there.
#[derive(Debug, Clone, Default)]
pub struct LineInput {
    text: String,
    /// Cursor position, in chars (not bytes).
    cursor: usize,
    /// Selection anchor, in chars: the fixed end of a selection whose
    /// moving end is the cursor. `None` (or equal to the cursor) means no
    /// selection.
    anchor: Option<usize>,
    /// The span a double click (a word or a bracket's block) or a triple
    /// click (the whole text) selected, direction kept, while its drag
    /// sweep is live: [`Self::extend_mouse_selection_to`] keeps it covered
    /// and grows the selection word by word past it. Cleared by any plain
    /// caret placement or edit.
    click_span: Option<crate::text::mouse::Span>,
    /// The vim engine's Visual selection (piece 3 §3.3): its anchor, in
    /// chars, and its shape. Paint only, set by `OneLineBuf::show`; kept
    /// apart from the GUI `anchor` because the engine owns the Visual
    /// anchor. `handle_key` never reads or writes it.
    vim_visual: Option<(usize, VisualShape)>,
    /// Snapshots taken before each step, newest last.
    undo: Vec<Snapshot>,
    /// Snapshots undone, newest last; cleared by any new edit.
    redo: Vec<Snapshot>,
    /// The kind of the step in progress, so a same-kind edit extends it
    /// instead of opening a new one. `None` after a caret move, an undo,
    /// or a whole-value step.
    run: Option<EditKind>,
    /// The text when the vim engine opened this field (plan M3b Deviation
    /// 11): its edits record no snapshot, so `text_at_open` reads this.
    opened: Option<String>,
}

/// Text and caret as they stood before a step.
#[derive(Debug, Clone)]
struct Snapshot {
    text: String,
    cursor: usize,
}

/// What kind of edit a step is made of. Consecutive edits of the same
/// kind (`Insert`, `DeleteBack`, `DeleteForward`) extend one step; a
/// `Whole` edit (paste, replace-selection, programmatic insert) is always
/// its own step and never extends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EditKind {
    Insert,
    DeleteBack,
    DeleteForward,
    Whole,
}

/// Most snapshots an input keeps; the app history uses the same figure.
pub(crate) const HISTORY_CAP: usize = 200;

/// The shape of the vim engine's Visual selection (piece 3 §3.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum VisualShape {
    /// Inclusive of the char under the caret, Vim's `selection=inclusive`.
    Char,
    /// The whole line.
    Line,
}

impl LineInput {
    pub fn new(text: &str) -> Self {
        let cursor = text.chars().count();
        Self {
            text: text.to_string(),
            cursor,
            anchor: None,
            click_span: None,
            vim_visual: None,
            undo: Vec::new(),
            redo: Vec::new(),
            run: None,
            opened: None,
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// Moves the cursor to char index `idx`, clamped to the text's char
    /// count. Used by mouse click-to-place. Drops any selection.
    pub fn set_cursor(&mut self, idx: usize) {
        self.cursor = idx.min(self.len_chars());
        self.anchor = None;
        self.click_span = None;
        self.break_run();
    }

    /// Sets or clears the vim engine's Visual paint: the anchor (a char
    /// index) and the shape. The caret is the moving end.
    pub(crate) fn set_visual(&mut self, visual: Option<(usize, VisualShape)>) {
        self.vim_visual = visual;
    }

    #[cfg(test)]
    pub(crate) fn visual(&self) -> Option<(usize, VisualShape)> {
        self.vim_visual
    }

    /// The half-open char span the renderer reverses: the engine's Visual
    /// span when one is set (the whole text for linewise, anchor..=caret
    /// for charwise), otherwise [`Self::selection`]. `None` when empty.
    pub(crate) fn paint_span(&self) -> Option<(usize, usize)> {
        let len = self.len_chars();
        let span = match self.vim_visual {
            None => return self.selection(),
            Some((_, VisualShape::Line)) => (0, len),
            Some((anchor, VisualShape::Char)) => {
                let anchor = anchor.min(len);
                (anchor.min(self.cursor), (anchor.max(self.cursor) + 1).min(len))
            }
        };
        (span.0 < span.1).then_some(span)
    }

    /// Replaces chars `start..end` with `text` and leaves the caret after
    /// it. Records nothing in this input's own history: the vim engine
    /// keeps its own (piece 3 §3.3). Drops any GUI selection.
    pub(crate) fn splice_raw(&mut self, start: usize, end: usize, text: &str) {
        let len = self.len_chars();
        let start = start.min(len);
        let end = end.clamp(start, len);
        let (bs, be) = (self.byte_offset(start), self.byte_offset(end));
        self.text.replace_range(bs..be, text);
        self.cursor = start + text.chars().count();
        self.anchor = None;
        self.click_span = None;
        self.run = None;
    }

    /// Records the state before an edit of `kind`. A same-kind edit
    /// extends the current run (no new snapshot); anything else opens a
    /// new step. Any edit drops the redo branch.
    fn record(&mut self, kind: EditKind) {
        self.redo.clear();
        if self.run == Some(kind) && kind != EditKind::Whole {
            return;
        }
        self.undo.push(Snapshot { text: self.text.clone(), cursor: self.cursor });
        if self.undo.len() > HISTORY_CAP {
            self.undo.remove(0);
        }
        self.run = if kind == EditKind::Whole { None } else { Some(kind) };
    }

    /// Ends the current run so the next edit starts a fresh step: called
    /// on every caret move and selection change.
    fn break_run(&mut self) {
        self.run = None;
    }

    /// Undoes the newest step. `false` when there is nothing to undo.
    pub fn undo(&mut self) -> bool {
        let Some(snap) = self.undo.pop() else {
            return false;
        };
        self.redo.push(Snapshot {
            text: std::mem::replace(&mut self.text, snap.text),
            cursor: std::mem::replace(&mut self.cursor, snap.cursor),
        });
        self.anchor = None;
        self.click_span = None;
        self.run = None;
        true
    }

    /// Redoes the newest undone step. `false` when there is none.
    pub fn redo(&mut self) -> bool {
        let Some(snap) = self.redo.pop() else {
            return false;
        };
        self.undo.push(Snapshot {
            text: std::mem::replace(&mut self.text, snap.text),
            cursor: std::mem::replace(&mut self.cursor, snap.cursor),
        });
        self.anchor = None;
        self.click_span = None;
        self.run = None;
        true
    }

    /// Whether this edit session has recorded anything — what the app
    /// asks before routing an undo key here rather than to its own
    /// history.
    pub fn edited(&self) -> bool {
        !self.undo.is_empty() || !self.redo.is_empty()
    }

    /// Closes the edit session: the text stays, the history goes. The
    /// owner calls this when the field closes (Esc/Enter/blur) so a later
    /// ctrl+z steps through the app history, never through keystrokes.
    pub fn end_edit(&mut self) {
        self.undo.clear();
        self.redo.clear();
        self.run = None;
        self.opened = None;
    }

    /// Marks the text a vim engine session starts from, unless an earlier
    /// open of this same edit already did (a cover over the field leaves
    /// the edit open). For a container that records "what changed across
    /// this open" (plan M3b Deviation 11).
    pub(crate) fn mark_open(&mut self) {
        if self.opened.is_none() {
            self.opened = Some(self.text_at_open());
        }
    }

    /// The text as it stood when this open began — the mark a vim session
    /// left ([`Self::mark_open`]), else the bottom of the undo stack, or
    /// the current text if nothing was edited. The mark wins: engine edits
    /// record no snapshot, so after a profile switch the first snapshot is
    /// already past them. Used by a container (a form modal) that needs
    /// "what changed across this whole open", not step-by-step history.
    pub fn text_at_open(&self) -> String {
        self.opened
            .clone()
            .or_else(|| self.undo.first().map(|s| s.text.clone()))
            .unwrap_or_else(|| self.text.clone())
    }

    /// Replaces the whole text (caret at the end) as one undoable step.
    /// For owners that reseed a live field — a picker's `seed_filter`, a
    /// choice cycle — rather than constructing a fresh `LineInput`, which
    /// would drop the session's history.
    pub fn set_text(&mut self, text: &str) {
        self.record(EditKind::Whole);
        self.text = text.to_string();
        self.cursor = self.text.chars().count();
        self.anchor = None;
        self.click_span = None;
    }

    /// The selected char range as half-open `(start, end)` with
    /// `start < end`, or `None` when nothing is selected.
    pub fn selection(&self) -> Option<(usize, usize)> {
        let anchor = self.anchor?;
        match anchor.cmp(&self.cursor) {
            std::cmp::Ordering::Less => Some((anchor, self.cursor)),
            std::cmp::Ordering::Equal => None,
            std::cmp::Ordering::Greater => Some((self.cursor, anchor)),
        }
    }

    /// The selected text, or `None` when nothing is selected.
    pub fn selected_text(&self) -> Option<String> {
        let (start, end) = self.selection()?;
        Some(self.text.chars().skip(start).take(end - start).collect())
    }

    pub fn clear_selection(&mut self) {
        self.anchor = None;
        self.click_span = None;
        self.break_run();
    }

    pub fn select_all(&mut self) {
        self.anchor = Some(0);
        self.cursor = self.len_chars();
        self.click_span = None;
        self.break_run();
    }

    /// The double-click gesture at char index `idx` (spec 2026-10-14, align
    /// the text mouse): the shared mouse word under it, a press past the end
    /// taking the last word; on a bracket (or blanks before one), the block
    /// to its match. Remembers what it selected, so a following drag keeps
    /// it and extends word by word. An empty text just takes the caret.
    pub fn select_word_at(&mut self, idx: usize) {
        let chars: Vec<char> = self.text.chars().collect();
        match crate::text::mouse::double_click(&chars[..], crate::text::Pos::new(0, idx)) {
            Some(span) => {
                self.select_span(span);
                self.click_span = Some(span);
            }
            None => self.set_cursor(idx),
        }
    }

    /// The triple-click gesture: the whole text, kept whole by a following
    /// drag. An empty text just takes the caret.
    pub fn select_line(&mut self) {
        use crate::text::{Pos, mouse::Span};
        self.select_all();
        self.click_span = self
            .len_chars()
            .checked_sub(1)
            .map(|last| Span { fixed: Pos::new(0, 0), moving: Pos::new(0, last) });
    }

    /// Selects an inclusive `span` as the GUI selection
    /// (`text::mouse::Span::gui_ends`), widened to whole graphemes: the
    /// mouse words go by chars, and a ZWJ emoji is several.
    fn select_span(&mut self, span: crate::text::mouse::Span) {
        let (anchor, caret) = span.gui_ends(|p| crate::text::Pos::new(p.row, p.col + 1));
        let (anchor, caret) = (anchor.col, caret.col);
        let glyphs = glyphs(&self.text);
        let down = |i: usize| glyph_start(&glyphs, i);
        let up = |i: usize| match glyphs.get(glyph_at(&glyphs, i)) {
            Some(g) if g.char < i => g.char + g.chars,
            _ => i,
        };
        (self.anchor, self.cursor) = match anchor <= caret {
            true => (Some(down(anchor)), up(caret)),
            false => (Some(up(anchor)), down(caret)),
        };
        self.break_run();
    }

    /// Anchors a mouse selection at the current cursor; subsequent
    /// [`Self::extend_mouse_selection_to`] calls grow the selection to the
    /// drag point.
    pub fn begin_mouse_selection(&mut self) {
        self.anchor = Some(self.cursor);
        self.click_span = None;
        self.break_run();
    }

    /// A click planted the anchor ([`Self::begin_mouse_selection`]) and no
    /// drag has extended it yet.
    pub(crate) fn click_anchored(&self) -> bool {
        self.anchor.is_some() && self.click_span.is_none() && self.selection().is_none()
    }

    /// Extends the live mouse selection to char index `idx`: after a double
    /// or triple click, what it selected stays covered and the selection
    /// grows by the shared mouse words past it (`text::mouse::grow_by_words`);
    /// by character otherwise.
    pub fn extend_mouse_selection_to(&mut self, idx: usize) {
        let Some(span) = self.click_span else {
            self.set_cursor_extending(idx);
            return;
        };
        let chars: Vec<char> = self.text.chars().collect();
        let at = crate::text::Pos::new(0, idx.min(chars.len()));
        self.select_span(crate::text::mouse::grow_by_words(&chars[..], span, at));
    }

    /// Moves the cursor (clamped) while keeping the selection anchor, so a
    /// mouse drag extends the selection instead of collapsing it.
    pub fn set_cursor_extending(&mut self, idx: usize) {
        self.cursor = idx.min(self.len_chars());
        self.break_run();
    }

    /// Removes the selected text (cursor lands at the selection start).
    /// Returns whether a selection was removed. Never records: the
    /// caller records the step this deletion belongs to.
    fn delete_selection(&mut self) -> bool {
        let Some((start, end)) = self.selection() else {
            self.anchor = None;
            return false;
        };
        let bs = self.byte_offset(start);
        let be = self.byte_offset(end);
        self.text.replace_range(bs..be, "");
        self.cursor = start;
        self.anchor = None;
        self.click_span = None;
        true
    }

    fn len_chars(&self) -> usize {
        self.text.chars().count()
    }

    /// Byte offset of the char at `idx` (or the end-of-string offset when
    /// `idx == len_chars()`).
    fn byte_offset(&self, idx: usize) -> usize {
        self.text.char_indices().nth(idx).map(|(b, _)| b).unwrap_or(self.text.len())
    }

    /// Removes the chars from index `idx` up to the cursor (nothing when
    /// `idx` is at or past it) and lands the cursor at `idx`, dropping any
    /// selection. For a caller with its own idea of a "word" — the jq
    /// bar's path segment — where the input's ctrl+backspace rule is the
    /// wrong one.
    pub fn delete_back_to(&mut self, idx: usize) {
        let idx = idx.min(self.cursor);
        if idx < self.cursor {
            self.record(EditKind::DeleteBack);
            let start = self.byte_offset(idx);
            let end = self.byte_offset(self.cursor);
            self.text.replace_range(start..end, "");
            self.cursor = idx;
        }
        self.anchor = None;
    }

    /// ctrl+backspace as a call, not a key, so it runs under
    /// [`quiet`]: the selection, else the word behind the caret. For the
    /// jq bar's fallback when there is no path segment to delete (plan
    /// M3b Deviation 5).
    pub fn delete_word_back(&mut self) {
        self.snap_to_graphemes();
        if self.selection().is_some() {
            self.record(EditKind::Whole);
            self.delete_selection();
            return;
        }
        self.delete_back_from(self.word_target(false));
    }

    /// Backspace's delete without a selection: the chars from `target` up
    /// to the caret, one `DeleteBack` step that a burst extends.
    fn delete_back_from(&mut self, target: usize) {
        // A click plants the anchor on the caret and leaves it there; the
        // caret moving off it would read as a selection. Not
        // `clear_selection`: that breaks the run, and a burst of
        // backspaces is one step.
        self.anchor = None;
        self.click_span = None;
        if self.cursor > target {
            self.record(EditKind::DeleteBack);
            let start = self.byte_offset(target);
            let end = self.byte_offset(self.cursor);
            self.text.replace_range(start..end, "");
            self.cursor = target;
        }
    }

    /// Inserts `s` at the cursor and advances the cursor by `s`'s char
    /// count. Used to splice in multi-character text (e.g. a picked
    /// variable token) in one shot, rather than one `handle_key` per char.
    pub fn insert_str(&mut self, s: &str) {
        self.record(EditKind::Whole);
        self.snap_to_graphemes();
        let at = self.byte_offset(self.cursor);
        self.text.insert_str(at, s);
        self.cursor += s.chars().count();
        self.anchor = None;
        self.click_span = None;
    }

    /// Pastes `text` at the caret: a live selection is replaced (GUI
    /// paste semantics), and the text is flattened to a single line via
    /// [`flatten_paste`], so a multi-line paste can't smuggle an Enter
    /// into a one-line field.
    pub fn paste(&mut self, text: &str) {
        self.record(EditKind::Whole);
        self.snap_to_graphemes();
        self.delete_selection();
        let flat = flatten_paste(text);
        let at = self.byte_offset(self.cursor);
        self.text.insert_str(at, &flat);
        self.cursor += flat.chars().count();
        self.anchor = None;
        self.click_span = None;
    }

    /// Whether the text *before* the cursor ends with `suffix`. Used to spot
    /// a just-typed `{{` trigger without caring what (if anything) follows
    /// the cursor.
    pub fn ends_with_at_cursor(&self, suffix: &str) -> bool {
        let end = self.byte_offset(self.cursor);
        self.text[..end].ends_with(suffix)
    }

    /// Handles a key event, returning `true` if it was consumed (state may
    /// have changed) or `false` if the caller should treat it as unhandled.
    /// The char index a word-left/word-right motion from the cursor lands
    /// on (ctrl+arrow, or alt+arrow for macOS muscle memory).
    /// Graphemes hop as their first chars do, so a mark stays in its word.
    fn word_target(&self, forward: bool) -> usize {
        let glyphs = glyphs(&self.text);
        let firsts: Vec<char> =
            glyphs.iter().filter_map(|g| self.text[g.byte..].chars().next()).collect();
        let at = glyph_at(&glyphs, self.cursor);
        let to = if forward {
            super::word_nav::next_word_boundary(&firsts, at)
        } else {
            super::word_nav::prev_word_boundary(&firsts, at)
        };
        glyph_char(&glyphs, to)
    }

    /// The grapheme edge a one-step move back from char `idx` lands on:
    /// the start of the grapheme before it (or the one it is inside), so
    /// what looks like one thing on screen takes one press.
    fn edge_before(&self, idx: usize) -> usize {
        glyphs(&self.text).iter().rev().map(|g| g.char).find(|&c| c < idx).unwrap_or(0)
    }

    /// Moves a cursor (and anchor) the vim engine left inside a grapheme
    /// to that grapheme's start, where its caret is drawn, so every key
    /// and edit after it works between whole graphemes.
    fn snap_to_graphemes(&mut self) {
        let glyphs = glyphs(&self.text);
        self.cursor = glyph_start(&glyphs, self.cursor);
        self.anchor = self.anchor.map(|a| glyph_start(&glyphs, a));
    }

    /// The grapheme edge a one-step move forward from char `idx` lands on:
    /// the end of the grapheme it starts (or is inside).
    fn edge_after(&self, idx: usize) -> usize {
        let glyphs = glyphs(&self.text);
        glyphs.iter().map(|g| g.char + g.chars).find(|&e| e > idx).unwrap_or(idx)
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> bool {
        if QUIET.get() {
            return false;
        }
        // The emacs caret bytes (ctrl+e, ESC b/f — also what mac
        // terminals send for cmd/option+arrows) fold into the
        // End/word-hop keys matched below on every platform; only ^A is
        // platform-split (mac: Home, Linux: falls through to select-all).
        let key = crate::keys::fold_text_nav_bytes(key, cfg!(target_os = "macos"));
        self.snap_to_graphemes();
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        // ctrl+arrow skips words; alt+arrow is the same gesture as macOS
        // terminals deliver it (option+arrow), so both spellings work on
        // every platform.
        let word = key.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
        // Shifted motion extends a selection from an anchor planted at the
        // pre-move cursor; unshifted motion collapses any selection first
        // (Left/Right land on the selection's own edge, GUI-style).
        match key.code {
            // Select-all is ctrl+shift+a — arriving as 'a' with
            // SHIFT|CONTROL, or (kitty alternate-keys reporting, and the
            // SUPER fold's cmd+a) as the pre-shifted 'A' with CONTROL.
            KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) && shift => {
                self.select_all();
                true
            }
            KeyCode::Char('A') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.select_all();
                true
            }
            // Plain ctrl+a is select-all on Linux; on macOS builds it
            // never reaches this arm — `fold_text_nav_bytes` turned it
            // into Home (it's the byte cmd+left sends).
            KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.select_all();
                true
            }
            // The shell's ctrl+u: delete from the line start to the caret
            // (or just the selection, when there is one). Like the deletes
            // below, it records only when there is something to remove.
            KeyCode::Char('u') if key.modifiers == KeyModifiers::CONTROL => {
                if self.selection().is_some() {
                    self.record(EditKind::Whole);
                    self.delete_selection();
                } else if self.cursor > 0 {
                    self.record(EditKind::Whole);
                    let end = self.byte_offset(self.cursor);
                    self.text.replace_range(0..end, "");
                    self.cursor = 0;
                }
                self.anchor = None;
                self.click_span = None;
                true
            }
            // A physical ctrl+backspace reaches a legacy terminal as the
            // 0x08 byte, which crossterm parses as ctrl+h — same word
            // deletion as the enhanced-keys `Backspace + CONTROL` below.
            KeyCode::Char('h') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.record(if self.selection().is_some() {
                    EditKind::Whole
                } else {
                    EditKind::DeleteBack
                });
                if !self.delete_selection() {
                    let target = self.word_target(false);
                    if self.cursor > target {
                        let start = self.byte_offset(target);
                        let end = self.byte_offset(self.cursor);
                        self.text.replace_range(start..end, "");
                        self.cursor = target;
                    }
                }
                true
            }
            KeyCode::Char(c) if key.modifiers.difference(KeyModifiers::SHIFT).is_empty() => {
                self.record(if self.selection().is_some() {
                    EditKind::Whole
                } else {
                    EditKind::Insert
                });
                self.delete_selection();
                let at = self.byte_offset(self.cursor);
                self.text.insert(at, c);
                self.cursor += 1;
                true
            }
            // Both deletes record only once there is something to remove:
            // a Backspace on an empty field (or a Delete at the end) that
            // recorded first would push a phantom snapshot, drop the redo
            // branch, and flip `edited()` on a pristine field — which is
            // what decides whether ctrl+z belongs to this field or to the
            // app history.
            KeyCode::Backspace => {
                if self.selection().is_some() {
                    self.record(EditKind::Whole);
                    self.delete_selection();
                    return true;
                }
                // ctrl/alt+backspace removes the whole word behind the
                // caret, the same hop word-left would make.
                let target =
                    if word { self.word_target(false) } else { self.edge_before(self.cursor) };
                self.delete_back_from(target);
                true
            }
            KeyCode::Delete => {
                if self.selection().is_some() {
                    self.record(EditKind::Whole);
                    self.delete_selection();
                    return true;
                }
                self.anchor = None;
                self.click_span = None;
                if self.cursor < self.len_chars() {
                    self.record(EditKind::DeleteForward);
                    let start = self.byte_offset(self.cursor);
                    let end = self.byte_offset(self.edge_after(self.cursor));
                    self.text.replace_range(start..end, "");
                }
                true
            }
            KeyCode::Left => {
                self.break_run();
                if shift {
                    self.anchor.get_or_insert(self.cursor);
                    self.cursor =
                        if word { self.word_target(false) } else { self.edge_before(self.cursor) };
                } else if !word && let Some((start, _)) = self.selection() {
                    self.cursor = start;
                    self.anchor = None;
                } else {
                    self.anchor = None;
                    self.cursor =
                        if word { self.word_target(false) } else { self.edge_before(self.cursor) };
                }
                true
            }
            KeyCode::Right => {
                self.break_run();
                if shift {
                    self.anchor.get_or_insert(self.cursor);
                    self.cursor =
                        if word { self.word_target(true) } else { self.edge_after(self.cursor) };
                } else if !word && let Some((_, end)) = self.selection() {
                    self.cursor = end;
                    self.anchor = None;
                } else {
                    self.anchor = None;
                    self.cursor =
                        if word { self.word_target(true) } else { self.edge_after(self.cursor) };
                }
                true
            }
            KeyCode::Home => {
                self.break_run();
                if shift {
                    self.anchor.get_or_insert(self.cursor);
                } else {
                    self.anchor = None;
                }
                self.cursor = 0;
                true
            }
            KeyCode::End => {
                self.break_run();
                if shift {
                    self.anchor.get_or_insert(self.cursor);
                } else {
                    self.anchor = None;
                }
                self.cursor = self.len_chars();
                true
            }
            _ => false,
        }
    }

    /// Renders the input as a single ratatui `Line`; when `focused`, the
    /// character under the cursor (or a trailing blank cell at end-of-text)
    /// is rendered with a REVERSED style to represent the caret.
    pub fn draw_line(&self, focused: bool, theme: &Theme) -> Line<'static> {
        self.render(focused, theme, theme.text, None, false, true)
    }

    /// Like [`Self::draw_line`], but every character renders as `●` — used
    /// for a secret cell's in-place edit (Variable Manager, spec §5), so
    /// the typed value never appears in plaintext on screen.
    pub fn draw_line_masked(&self, focused: bool, theme: &Theme) -> Line<'static> {
        self.render(focused, theme, theme.text, None, true, true)
    }

    /// Like [`Self::draw_line`], but windowed to `width` columns: when
    /// focused, the window scrolls so the cursor always stays visible
    /// (in `[0, width)`), which matters once the text is longer than the
    /// pane — otherwise the caret (and everything past it) renders off
    /// the edge of the pane and the user can't see what they're typing.
    /// Unfocused text is never scrolled; it always renders from char 0
    /// (matching `draw_line`, just clipped to `width`).
    pub fn draw_line_windowed(&self, focused: bool, theme: &Theme, width: u16) -> Line<'static> {
        self.render_windowed(focused, theme, width, false, true)
    }

    /// [`Self::draw_line_windowed`] without the painted caret: the window
    /// still follows the cursor and a selection still renders REVERSED,
    /// but the cursor cell itself is left plain, for a field that shows
    /// the terminal's own cursor there instead (placed at
    /// [`Self::caret_column`]). The trailing blank cell past end-of-text
    /// is not emitted either; the terminal cursor stands in that cell.
    pub fn draw_line_windowed_no_caret(
        &self,
        focused: bool,
        theme: &Theme,
        width: u16,
    ) -> Line<'static> {
        self.render_windowed(focused, theme, width, false, false)
    }

    /// The column, within a focused `width`-column window, the caret sits
    /// in: the cells from the window's start to the caret's grapheme,
    /// always in `[0, width)` because the window scrolls to keep the
    /// cursor visible. Where a caller places
    /// the terminal's own cursor for a field drawn with
    /// [`Self::draw_line_windowed_no_caret`].
    pub fn caret_column(&self, width: u16) -> u16 {
        let glyphs = glyphs(&self.text);
        let start = window_glyph(&glyphs, self.cursor, true, width);
        // Cells, not chars: a wide glyph before the caret occupies two
        // columns on screen, and the terminal's cursor must land after it.
        // Inside a grapheme, the caret stands where the grapheme starts.
        let caret = glyph_at(&glyphs, self.cursor);
        glyphs[start..caret].iter().map(|g| g.width as u16).sum()
    }

    /// The char a click on column `col` of the `width`-column window lands
    /// on: the first char of the grapheme painted there (either cell of a
    /// wide one), or the end of the text past it.
    pub fn char_at_column(&self, focused: bool, width: u16, col: u16) -> usize {
        let glyphs = glyphs(&self.text);
        let start = window_glyph(&glyphs, self.cursor, focused, width);
        let cells = drawn(&glyphs, start, width.max(1) as usize);
        let mut x = 0;
        for (g, w) in glyphs[start..].iter().zip(&cells) {
            x += w;
            if usize::from(col) < x {
                return g.char;
            }
        }
        glyph_char(&glyphs, start + cells.len())
    }

    /// The columns, within the `width`-column window
    /// [`Self::draw_line_windowed`] draws, that the text's bytes `range`
    /// take on screen, clipped to the text the window shows
    /// ([`Self::visible_window`]) and measured as it lays that text out:
    /// `None` when none of it takes a cell there, or `range` is not a
    /// slice of the text. Where a caller paints over part of the drawn
    /// text (the jq bar underlines an error's span).
    pub fn span_columns(
        &self,
        focused: bool,
        width: u16,
        range: Range<usize>,
    ) -> Option<Range<u16>> {
        self.text.get(range.clone())?;
        if range.is_empty() {
            return None;
        }
        let glyphs = glyphs(&self.text);
        let start = window_glyph(&glyphs, self.cursor, focused, width);
        let cells = drawn(&glyphs, start, width.max(1) as usize);
        // The cells of every drawn grapheme the bytes touch.
        let mut x = 0u16;
        let mut cols: Option<Range<u16>> = None;
        for (g, w) in glyphs[start..].iter().zip(&cells) {
            let next = x + *w as u16;
            if g.byte < range.end && range.start < g.byte + g.len {
                cols = Some(cols.map_or(x, |c| c.start)..next);
            }
            x = next;
        }
        cols.filter(|c| !c.is_empty())
    }

    /// [`Self::draw_line_windowed`] and [`Self::draw_line_masked`] combined
    /// — a secret cell's in-place edit (Variable Manager, spec §5) narrower
    /// than the typed value: masked so the value never appears in
    /// plaintext, and windowed so the caret stays visible while typing.
    pub fn draw_line_windowed_masked(
        &self,
        focused: bool,
        theme: &Theme,
        width: u16,
    ) -> Line<'static> {
        self.render_windowed(focused, theme, width, true, true)
    }

    /// The first char index [`Self::draw_line_windowed`] would render at a
    /// `width`-column window: 0 unless the input is focused and the caret
    /// has scrolled the window right. For tests: a painter reads the
    /// window through [`Self::visible_window`], [`Self::span_columns`] or
    /// [`Self::char_at_column`], which measure it as the draw does.
    #[cfg(test)]
    pub fn window_start(&self, focused: bool, width: u16) -> usize {
        let glyphs = glyphs(&self.text);
        glyph_char(&glyphs, window_glyph(&glyphs, self.cursor, focused, width))
    }

    /// The exact text [`Self::draw_line_windowed`] puts on screen for a
    /// `width`-column window (unmasked; a masked field shows no tokens to
    /// highlight).
    pub fn visible_window(&self, focused: bool, width: u16) -> String {
        let glyphs = glyphs(&self.text);
        let start = window_glyph(&glyphs, self.cursor, focused, width);
        let cells = drawn(&glyphs, start, width.max(1) as usize);
        glyphs[start..].iter().zip(cells).map(|(g, w)| g.face(&self.text, w)).collect()
    }

    /// The field as a value at rest draws it — unfocused, windowed to
    /// `width`, in `fg` (a muted row, a placeholder) — so a value looks the
    /// same at rest and in edit, and the click that opens it lands where it
    /// points ([`Self::char_at_column`] reads this same layout).
    pub fn draw_line_at_rest(&self, theme: &Theme, fg: Color, width: u16) -> Line<'static> {
        self.render(false, theme, fg, Some(width.max(1) as usize), false, false)
    }

    fn render_windowed(
        &self,
        focused: bool,
        theme: &Theme,
        width: u16,
        mask: bool,
        caret: bool,
    ) -> Line<'static> {
        self.render(focused, theme, theme.text, Some(width.max(1) as usize), mask, caret)
    }

    /// The single renderer behind all the `draw_line*` variants: optionally
    /// masked, optionally windowed to `width` columns (the window scrolls
    /// with the cursor when focused), in `fg`, with any selected range
    /// rendered REVERSED and — unless `caret` is off — the cursor cell too.
    fn render(
        &self,
        focused: bool,
        theme: &Theme,
        fg: Color,
        width: Option<usize>,
        mask: bool,
        caret: bool,
    ) -> Line<'static> {
        // The color rides on the spans, not the `Line`: callers that lift
        // `line.spans` into their own row (the jq bar) would otherwise draw
        // the text in the terminal's default foreground.
        let base = Style::default().fg(fg);
        let code = Style::default().fg(theme.text_muted);
        let glyphs = glyphs(&self.text);
        // Masked, each grapheme shows a dot per cell it shows, so the
        // masked line lays out exactly as the value it hides.
        let show = |out: &mut String, g: &Glyph, cells: usize| match mask {
            true => out.extend(std::iter::repeat_n('\u{25cf}', cells)),
            false => out.push_str(g.face(&self.text, cells)),
        };
        // Smallest window start that keeps the cursor visible; 0 when the
        // whole text is drawn, or unfocused.
        let start = match width {
            Some(w) => window_glyph(&glyphs, self.cursor, focused, w.min(u16::MAX as usize) as u16),
            None => 0,
        };
        let cells = drawn(&glyphs, start, width.unwrap_or(usize::MAX));
        let end = start + cells.len();
        let reversed = base.add_modifier(Modifier::REVERSED);
        let selection = self.paint_span().filter(|_| focused);
        let caret = caret && focused;
        // While a selection is live the reversed range *is* the visual
        // focus; the caret cell hides so it can't dangle outside the
        // selection's edge as a stray reversed cell. Both style whole
        // graphemes: the one holding the cursor, every one a selection
        // touches. A code stands out muted until either reverses it, but
        // not masked: muted dots would mark where such a char hides.
        let style_of = |g: &Glyph| {
            let chars = g.char..g.char + g.chars;
            if caret && selection.is_none() && chars.contains(&self.cursor) {
                return reversed;
            }
            match selection {
                Some((s, e)) if chars.start < e && s < chars.end => reversed,
                _ if g.code.is_some() && !mask => code,
                _ => base,
            }
        };
        // Group consecutive same-styled graphemes into spans.
        let mut spans: Vec<Span<'static>> = Vec::new();
        let mut run = String::new();
        let mut run_style = base;
        for (g, w) in glyphs[start..end].iter().zip(cells) {
            let style = style_of(g);
            if style != run_style && !run.is_empty() {
                spans.push(Span::styled(std::mem::take(&mut run), run_style));
            }
            run_style = style;
            show(&mut run, g, w);
        }
        if !run.is_empty() {
            spans.push(Span::styled(run, run_style));
        }
        // The trailing caret cell when the cursor sits past the drawn text
        // — suppressed like the in-text caret while a selection is live.
        if caret && selection.is_none() && self.cursor >= glyph_char(&glyphs, end) {
            spans.push(Span::styled(" ", reversed));
        }
        Line::from(spans)
    }
}

/// One grapheme of a field's text as the field lays it out: ratatui
/// paints a grapheme into its cells as one, so the window, the caret and
/// a selection all go by graphemes, while the cursor is a char index.
struct Glyph {
    /// The char index it starts at, and how many chars it holds.
    char: usize,
    chars: usize,
    /// Its byte offset and length in the text.
    byte: usize,
    len: usize,
    /// What a grapheme that takes no cell shows instead, so nothing in a
    /// field is invisible: its [`code_of`].
    code: Option<String>,
    /// The cells it takes as shown: [`crate::ui::cells`]'s measure, or
    /// its code's.
    width: usize,
}

impl Glyph {
    /// What the field paints for it in `cells` cells ([`drawn`]'s): its
    /// own text, or its code, cut when the window's edge cuts it.
    fn face<'a>(&'a self, text: &'a str, cells: usize) -> &'a str {
        match &self.code {
            Some(code) => &code[..cells.min(code.len())],
            None => &text[self.byte..self.byte + self.len],
        }
    }
}

/// `text`'s graphemes, in order.
fn glyphs(text: &str) -> Vec<Glyph> {
    let mut char = 0;
    crate::ui::graphemes(text)
        .map(|(byte, g, width)| {
            let chars = g.chars().count();
            char += chars;
            let code = code_of(g, width);
            let width = code.as_ref().map_or(width, String::len);
            Glyph { char: char - chars, chars, byte, len: g.len(), code, width }
        })
        .collect()
}

/// The index of the grapheme holding char `cursor`, or `glyphs.len()` at
/// (or past) the end of the text.
fn glyph_at(glyphs: &[Glyph], cursor: usize) -> usize {
    glyphs.partition_point(|g| g.char + g.chars <= cursor)
}

/// The char index grapheme `i` starts at; the text's char count at
/// `glyphs.len()`.
fn glyph_char(glyphs: &[Glyph], i: usize) -> usize {
    glyphs.get(i).map_or_else(|| glyphs.last().map_or(0, |g| g.char + g.chars), |g| g.char)
}

/// The char the grapheme holding char `i` starts at (`i` itself on a
/// grapheme edge; the text's end past it).
fn glyph_start(glyphs: &[Glyph], i: usize) -> usize {
    glyph_char(glyphs, glyph_at(glyphs, i))
}

/// The first grapheme a `width`-column window draws: 0 unless `focused`
/// with the caret scrolled right. Counted in cells, so the caret's column
/// (the cells from the window start up to the cursor) always fits: the
/// caret's own grapheme — or a one-cell blank at end-of-text — takes its
/// room first, then graphemes before it are admitted from the right
/// while they still fit.
fn window_glyph(glyphs: &[Glyph], cursor: usize, focused: bool, width: u16) -> usize {
    if !focused {
        return 0;
    }
    let width = width.max(1) as usize;
    let mut start = glyph_at(glyphs, cursor);
    let mut used = glyphs.get(start).map_or(1, |g| g.width);
    while start > 0 {
        let w = glyphs[start - 1].width;
        if used + w > width {
            break;
        }
        used += w;
        start -= 1;
    }
    start
}

/// The cells each grapheme from `start` shows in a `width`-column window,
/// one entry per grapheme drawn: the one layout the draw, the clicks, the
/// painted spans and [`LineInput::visible_window`] all read. A grapheme
/// shows all its cells while they fit; a code the window's edge cuts shows
/// the cells left (it is ASCII), so the caret on a code wider than a
/// narrow field is still drawn; a wide glyph the edge would split is left
/// out, as ratatui leaves it.
fn drawn(glyphs: &[Glyph], start: usize, width: usize) -> Vec<usize> {
    let mut used = 0;
    let mut cells = Vec::new();
    for g in glyphs.get(start..).unwrap_or_default() {
        let room = width - used;
        let w = match g.code {
            _ if g.width <= room => g.width,
            Some(_) if room > 0 => room,
            _ => break,
        };
        cells.push(w);
        used += w;
    }
    cells
}

/// How Vim shows a grapheme's chars where they would take no cell (its
/// `strtrans`): `^I` for a control char, `^?` for DEL, `<200b>` for the
/// rest. `None` for a grapheme that takes a cell, which shows as itself.
fn code_of(g: &str, cells: usize) -> Option<String> {
    let code = |c: char| match c {
        '\0'..='\x1f' => format!("^{}", char::from(c as u8 + 0x40)),
        '\x7f' => "^?".to_string(),
        _ => format!("<{:x}>", c as u32),
    };
    (cells == 0).then(|| g.chars().map(code).collect())
}

/// Flattens pasted text to one line: runs of line breaks and tabs collapse
/// to a single space (none at the edges). Shared by [`LineInput::paste`]
/// and the plain-`String` filter queries (palette, chooser, var picker)
/// so every single-line surface flattens a paste identically.
pub fn flatten_paste(text: &str) -> String {
    let mut flat = String::with_capacity(text.len());
    let mut pending_gap = false;
    for c in text.chars() {
        if matches!(c, '\n' | '\r' | '\t') {
            pending_gap = !flat.is_empty();
        } else {
            if pending_gap {
                flat.push(' ');
                pending_gap = false;
            }
            flat.push(c);
        }
    }
    flat
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }
    fn code(c: KeyCode) -> KeyEvent {
        KeyEvent::new(c, KeyModifiers::NONE)
    }

    /// Plan M3b Deviation 6: a key the vim engine handed back reaches a
    /// dialog's own arms, never its text.
    #[test]
    fn a_quiet_input_types_and_moves_nothing() {
        let mut input = LineInput::new("ab");
        {
            let _quiet = quiet();
            assert!(!input.handle_key(key('x')));
            assert!(!input.handle_key(code(KeyCode::Home)));
        }
        assert_eq!((input.text(), input.cursor()), ("ab", 2));
        input.handle_key(key('x'));
        assert_eq!(input.text(), "abx", "the guard ends with its scope");
    }

    /// Plan M3b Deviation 11: the engine's edits record no snapshot here,
    /// so a dialog's close step reads the text at open from the mark.
    #[test]
    fn text_at_open_falls_back_to_the_mark() {
        let mut input = LineInput::new("before");
        input.mark_open();
        input.splice_raw(0, 6, "after");
        assert_eq!(input.text_at_open(), "before");
        input.end_edit();
        assert_eq!(input.text_at_open(), "after", "a closed edit forgets its mark");
    }

    /// A cover over a dialog field leaves its edit open: the session that
    /// reopens it keeps the first mark.
    #[test]
    fn a_second_mark_keeps_the_first() {
        let mut input = LineInput::new("before");
        input.mark_open();
        input.splice_raw(0, 6, "during");
        input.mark_open();
        input.splice_raw(0, 6, "after");
        assert_eq!(input.text_at_open(), "before");
    }

    /// Final review M-2: a profile switch mid-edit puts snapshot-recording
    /// edits after engine edits; the close step still reads the marked text.
    #[test]
    fn the_mark_beats_the_first_snapshot() {
        let mut input = LineInput::new("before");
        input.mark_open();
        input.splice_raw(0, 6, "during");
        input.handle_key(key('!'));
        assert_eq!(input.text(), "during!");
        assert_eq!(input.text_at_open(), "before");
    }

    #[test]
    fn ctrl_u_deletes_to_the_start_of_the_line() {
        let mut i = LineInput::new("abc def");
        i.set_cursor(4);
        assert!(i.handle_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL)));
        assert_eq!(i.text(), "def");
        assert_eq!(i.cursor(), 0);
    }

    #[test]
    fn ctrl_u_at_the_line_start_records_nothing() {
        let mut i = LineInput::new("abc");
        i.set_cursor(0);
        assert!(i.handle_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL)));
        assert_eq!(i.text(), "abc");
        assert!(!i.edited(), "nothing was removed, so there is nothing to undo");
    }

    #[test]
    fn insert_at_cursor() {
        let mut input = LineInput::new("helo");
        input.handle_key(code(KeyCode::Left));
        assert!(input.handle_key(key('l')));
        assert_eq!(input.text(), "hello");
        assert_eq!(input.cursor(), 4);
    }

    #[test]
    fn backspace_removes_before_cursor() {
        let mut input = LineInput::new("hello");
        assert!(input.handle_key(code(KeyCode::Backspace)));
        assert_eq!(input.text(), "hell");
        assert_eq!(input.cursor(), 4);
    }

    #[test]
    fn home_and_end_move_cursor_to_bounds() {
        let mut input = LineInput::new("hello");
        assert!(input.handle_key(code(KeyCode::Home)));
        assert_eq!(input.cursor(), 0);
        assert!(input.handle_key(code(KeyCode::End)));
        assert_eq!(input.cursor(), 5);
    }

    #[test]
    fn arrow_keys_clamp_at_both_ends() {
        let mut input = LineInput::new("hi");
        input.handle_key(code(KeyCode::Home));
        assert!(input.handle_key(code(KeyCode::Left))); // clamps at 0
        assert_eq!(input.cursor(), 0);
        input.handle_key(code(KeyCode::End));
        assert!(input.handle_key(code(KeyCode::Right))); // clamps at len
        assert_eq!(input.cursor(), 2);
    }

    #[test]
    fn unicode_safe_char_indices() {
        let mut input = LineInput::new("héllo");
        input.handle_key(code(KeyCode::Home));
        for _ in 0..2 {
            input.handle_key(code(KeyCode::Right));
        }
        assert_eq!(input.cursor(), 2);
        assert!(input.handle_key(code(KeyCode::Backspace)));
        assert_eq!(input.text(), "hllo");
    }

    #[test]
    fn insert_str_at_cursor_and_suffix_probe() {
        let mut i = LineInput::new("ab");
        i.handle_key(code(KeyCode::Left));
        i.insert_str("{{x}}");
        assert_eq!(i.text(), "a{{x}}b");
        assert_eq!(i.cursor(), 6);
        let mut j = LineInput::new("http://{{");
        assert!(j.ends_with_at_cursor("{{"));
        j.handle_key(code(KeyCode::Left));
        assert!(!j.ends_with_at_cursor("{{"), "cursor moved off the braces");
    }

    #[test]
    fn paste_replaces_the_selection_and_flattens_line_breaks() {
        // Pasting over a selection replaces it, GUI-style.
        let mut i = LineInput::new("hello world");
        i.set_cursor(0);
        for _ in 0..5 {
            i.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::SHIFT));
        }
        i.paste("goodbye");
        assert_eq!(i.text(), "goodbye world");
        assert_eq!(i.cursor(), 7);
        assert_eq!(i.selection(), None);

        // A single-line input flattens pasted line breaks (and tabs) to
        // single spaces — a multi-line paste must not smuggle an Enter.
        let mut j = LineInput::new("");
        j.paste("a\r\nb\n\nc\td");
        assert_eq!(j.text(), "a b c d");
    }

    #[test]
    fn set_cursor_clamps_to_char_length() {
        let mut input = LineInput::new("héllo");
        input.set_cursor(2);
        assert_eq!(input.cursor(), 2);
        input.set_cursor(99);
        assert_eq!(input.cursor(), 5, "clamped to char count, not byte count");
    }

    #[test]
    fn unhandled_key_returns_false() {
        let mut input = LineInput::new("hi");
        assert!(!input.handle_key(code(KeyCode::Esc)));
    }

    fn shift(c: KeyCode) -> KeyEvent {
        KeyEvent::new(c, KeyModifiers::SHIFT)
    }

    #[test]
    fn a_typing_run_is_one_undo_step() {
        let mut i = LineInput::new("");
        for c in "abc".chars() {
            i.handle_key(key(c));
        }
        assert!(i.edited());
        assert!(i.undo());
        assert_eq!(i.text(), "");
        assert_eq!(i.cursor(), 0);
        assert!(i.redo());
        assert_eq!(i.text(), "abc");
        assert_eq!(i.cursor(), 3);
    }

    #[test]
    fn a_cursor_move_splits_the_typing_run() {
        let mut i = LineInput::new("");
        for c in "abc".chars() {
            i.handle_key(key(c));
        }
        i.handle_key(code(KeyCode::Left));
        i.handle_key(key('d'));
        assert_eq!(i.text(), "abdc");
        assert!(i.undo());
        assert_eq!(i.text(), "abc", "only the run after the move comes off");
        assert_eq!(i.cursor(), 2, "the caret goes back to where that run began");
        assert!(i.undo());
        assert_eq!(i.text(), "");
    }

    #[test]
    fn backspaces_and_typing_are_separate_runs_and_a_paste_is_its_own() {
        let mut i = LineInput::new("");
        for c in "abc".chars() {
            i.handle_key(key(c));
        }
        i.handle_key(code(KeyCode::Backspace));
        i.handle_key(code(KeyCode::Backspace));
        assert_eq!(i.text(), "a");
        i.paste("xy");
        assert_eq!(i.text(), "axy");
        assert!(i.undo());
        assert_eq!(i.text(), "a", "the paste comes off alone");
        assert!(i.undo());
        assert_eq!(i.text(), "abc", "the two backspaces come off together");
        assert!(i.undo());
        assert_eq!(i.text(), "");
        assert!(!i.undo(), "nothing left");
    }

    #[test]
    fn typing_over_a_selection_is_one_step() {
        let mut i = LineInput::new("hello");
        i.set_cursor(0);
        for _ in 0..5 {
            i.handle_key(shift(KeyCode::Right));
        }
        i.handle_key(key('x'));
        assert_eq!(i.text(), "x");
        assert!(i.undo());
        assert_eq!(i.text(), "hello");
    }

    #[test]
    fn a_new_edit_clears_redo_and_end_edit_clears_everything() {
        let mut i = LineInput::new("");
        i.handle_key(key('a'));
        assert!(i.undo());
        assert!(i.redo());
        assert!(i.undo());
        i.handle_key(key('b'));
        assert!(!i.redo(), "a fresh edit after undo drops the redo branch");
        assert_eq!(i.text(), "b");
        i.end_edit();
        assert!(!i.edited());
        assert!(!i.undo());
        assert_eq!(i.text(), "b", "end_edit keeps the text, drops the history");
    }

    #[test]
    fn set_text_is_one_undoable_step() {
        let mut i = LineInput::new("old");
        i.set_text("new");
        assert_eq!(i.text(), "new");
        assert_eq!(i.cursor(), 3);
        assert!(i.undo());
        assert_eq!(i.text(), "old");
    }

    #[test]
    fn the_history_is_capped() {
        let mut i = LineInput::new("");
        for n in 0..(HISTORY_CAP + 10) {
            i.handle_key(key('a'));
            // A move between each char makes each char its own step.
            i.handle_key(code(KeyCode::Left));
            i.handle_key(code(KeyCode::End));
            let _ = n;
        }
        let mut undone = 0;
        while i.undo() {
            undone += 1;
        }
        assert_eq!(undone, HISTORY_CAP);
    }

    fn line_text(line: &Line<'static>) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn windowed_cursor_at_end_of_long_text_scrolls_so_the_slice_ends_at_the_cursor() {
        let text: String = (0..200).map(|i| char::from(b'a' + (i % 26) as u8)).collect();
        let input = LineInput::new(&text); // cursor defaults to end (200)
        let theme = Theme::dark();
        let line = input.draw_line_windowed(true, &theme, 20);
        let rendered = line_text(&line);
        // 19 chars of visible text before the caret, plus the trailing
        // caret cell: exactly `width` columns, ending right at the cursor.
        assert_eq!(rendered.chars().count(), 20);
        let expected_before: String = text.chars().skip(181).collect(); // chars[181..200]
        assert!(rendered.starts_with(&expected_before));
    }

    #[test]
    fn unfocused_text_carries_the_theme_color_on_its_span() {
        // Callers that lift a drawn line's spans into their own row (the
        // jq bar) never see the `Line`'s own style, so the color must
        // ride on the span itself or the terminal's default foreground
        // shows through — invisible on a dark page, washed out on a light
        // one.
        let input = LineInput::new(".data.items");
        let theme = Theme::light();
        for line in [
            input.draw_line(false, &theme),
            input.draw_line_windowed(false, &theme, 40),
            input.draw_line_windowed_no_caret(false, &theme, 40),
        ] {
            assert_eq!(line.spans.len(), 1);
            assert_eq!(line.spans[0].style.fg, Some(theme.text));
        }
    }

    #[test]
    fn windowed_cursor_at_zero_renders_from_zero() {
        let mut input = LineInput::new("hello world, this is long");
        input.handle_key(code(KeyCode::Home));
        let theme = Theme::dark();
        let line = input.draw_line_windowed(true, &theme, 10);
        let rendered = line_text(&line);
        assert!(rendered.starts_with('h'), "must start from char 0: {rendered:?}");
    }

    #[test]
    fn masked_line_never_renders_the_underlying_text() {
        let input = LineInput::new("sk-live-secret");
        let theme = Theme::dark();
        let focused = line_text(&input.draw_line_masked(true, &theme));
        let unfocused = line_text(&input.draw_line_masked(false, &theme));
        assert!(!focused.contains("secret"), "{focused}");
        assert!(!unfocused.contains("secret"), "{unfocused}");
        assert_eq!(unfocused.chars().count(), "sk-live-secret".chars().count());
        assert!(unfocused.chars().all(|c| c == '\u{25cf}'));
    }

    #[test]
    fn windowed_masked_never_renders_the_underlying_text_and_keeps_the_caret_visible() {
        let text: String = (0..40).map(|i| char::from(b'a' + (i % 26) as u8)).collect();
        let input = LineInput::new(&text); // cursor defaults to end (40)
        let theme = Theme::dark();
        let rendered = line_text(&input.draw_line_windowed_masked(true, &theme, 10));
        assert_eq!(rendered.chars().count(), 10);
        assert!(
            rendered.chars().all(|c| c == '\u{25cf}' || c == ' '),
            "must be all masked dots (plus the trailing caret cell): {rendered:?}"
        );
    }

    fn shifted(c: KeyCode) -> KeyEvent {
        KeyEvent::new(c, KeyModifiers::SHIFT)
    }

    #[test]
    fn shift_right_extends_a_selection() {
        let mut input = LineInput::new("abc");
        input.handle_key(code(KeyCode::Home));
        assert!(input.handle_key(shifted(KeyCode::Right)));
        assert_eq!(input.selection(), Some((0, 1)));
        assert_eq!(input.cursor(), 1);
        assert_eq!(input.selected_text().as_deref(), Some("a"));
    }

    #[test]
    fn shift_home_selects_back_to_start_reversed() {
        let mut input = LineInput::new("abc"); // cursor at end
        assert!(input.handle_key(shifted(KeyCode::Home)));
        assert_eq!(input.selection(), Some((0, 3)));
        assert_eq!(input.cursor(), 0);
        assert_eq!(input.selected_text().as_deref(), Some("abc"));
    }

    #[test]
    fn unshifted_left_collapses_to_selection_start() {
        let mut input = LineInput::new("abc");
        input.handle_key(code(KeyCode::Home));
        input.handle_key(shifted(KeyCode::Right));
        input.handle_key(shifted(KeyCode::Right));
        assert!(input.handle_key(code(KeyCode::Left)));
        assert_eq!(input.cursor(), 0, "collapses to the start, no extra move");
        assert_eq!(input.selection(), None);
    }

    #[test]
    fn unshifted_right_collapses_to_selection_end() {
        let mut input = LineInput::new("abc");
        input.handle_key(code(KeyCode::Home));
        input.handle_key(shifted(KeyCode::Right));
        input.handle_key(shifted(KeyCode::Right));
        assert!(input.handle_key(code(KeyCode::Right)));
        assert_eq!(input.cursor(), 2, "collapses to the end, no extra move");
        assert_eq!(input.selection(), None);
    }

    #[test]
    fn typing_replaces_the_selection() {
        let mut input = LineInput::new("abc");
        input.handle_key(code(KeyCode::Home));
        input.handle_key(shifted(KeyCode::Right));
        input.handle_key(shifted(KeyCode::Right));
        assert!(input.handle_key(key('x')));
        assert_eq!(input.text(), "xc");
        assert_eq!(input.cursor(), 1);
        assert_eq!(input.selection(), None);
    }

    /// A delete with nothing to remove is not an edit: it records no
    /// snapshot, keeps the redo branch, and leaves a pristine field
    /// pristine (so the undo keys still belong to the app history).
    #[test]
    fn a_no_op_backspace_or_delete_records_nothing() {
        let mut input = LineInput::new("");
        assert!(input.handle_key(code(KeyCode::Backspace)));
        assert!(input.handle_key(code(KeyCode::Delete)));
        assert!(!input.edited(), "an untouched field stays untouched");

        let mut input = LineInput::new("");
        for c in "abc".chars() {
            input.handle_key(key(c));
        }
        assert!(input.undo());
        assert_eq!(input.text(), "");
        input.handle_key(code(KeyCode::Backspace));
        assert!(input.redo(), "the redo branch survives a no-op delete");
        assert_eq!(input.text(), "abc");
        input.handle_key(code(KeyCode::End));
        input.handle_key(code(KeyCode::Delete));
        assert!(input.undo(), "the typing step is still the one to undo");
        assert_eq!(input.text(), "");
        assert!(!input.undo(), "and no phantom step sits under it");
    }

    #[test]
    fn backspace_and_delete_remove_only_the_selection() {
        let mut input = LineInput::new("abcd");
        input.handle_key(code(KeyCode::Home));
        input.handle_key(code(KeyCode::Right));
        input.handle_key(shifted(KeyCode::Right));
        input.handle_key(shifted(KeyCode::Right));
        assert!(input.handle_key(code(KeyCode::Backspace)));
        assert_eq!(input.text(), "ad");
        assert_eq!(input.cursor(), 1);

        let mut input = LineInput::new("abcd");
        input.handle_key(code(KeyCode::Home));
        input.handle_key(shifted(KeyCode::Right));
        assert!(input.handle_key(code(KeyCode::Delete)));
        assert_eq!(input.text(), "bcd");
        assert_eq!(input.cursor(), 0);
    }

    /// A plain click plants the anchor on the caret (a drag may follow)
    /// and nothing clears it on release. Backspace/Delete with no
    /// selection must drop that stale anchor, or the caret moving off it
    /// reads back as a selection the next keystroke silently eats.
    #[test]
    fn backspace_and_delete_drop_a_stale_click_anchor() {
        let mut input = LineInput::new("abcd");
        input.set_cursor(2);
        input.begin_mouse_selection();
        assert_eq!(input.selection(), None, "anchor == cursor is no selection");
        assert!(input.handle_key(code(KeyCode::Backspace)));
        assert_eq!(input.text(), "acd");
        assert_eq!(input.selection(), None, "no phantom selection after Backspace");
        input.handle_key(code(KeyCode::Char('x')));
        assert_eq!(input.text(), "axcd", "typing inserts, it does not replace");

        let mut input = LineInput::new("abcd");
        input.set_cursor(1);
        input.begin_mouse_selection();
        assert!(input.handle_key(code(KeyCode::Delete)));
        assert_eq!(input.text(), "acd");
        assert_eq!(input.selection(), None, "no phantom selection after Delete");
        input.handle_key(code(KeyCode::Right));
        input.handle_key(shifted(KeyCode::Left));
        assert_eq!(input.selection(), Some((1, 2)), "a fresh selection still works");
    }

    #[test]
    fn ctrl_shift_a_selects_all_in_both_reported_spellings() {
        // Kitty-protocol spelling: base char with SHIFT|CONTROL.
        let mut input = LineInput::new("hello");
        input.set_cursor(2);
        assert!(input.handle_key(KeyEvent::new(
            KeyCode::Char('a'),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT
        )));
        assert_eq!(input.selection(), Some((0, 5)));
        assert_eq!(input.selected_text().as_deref(), Some("hello"));
        // Alternate-keys / SUPER-fold spelling: pre-shifted 'A' + CONTROL.
        let mut input = LineInput::new("hello");
        input.set_cursor(2);
        assert!(input.handle_key(KeyEvent::new(KeyCode::Char('A'), KeyModifiers::CONTROL)));
        assert_eq!(input.selection(), Some((0, 5)));
    }

    #[test]
    fn plain_ctrl_a_selects_all_on_linux_builds() {
        // On Linux there is no cmd key sending ^A bytes, so ctrl+a keeps
        // its GUI select-all meaning (this test exercises the identity
        // fold explicitly, so it documents both platforms from either).
        let mut input = LineInput::new("hello");
        input.set_cursor(2);
        let ev = crate::keys::fold_text_nav_bytes(
            KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL),
            false,
        );
        assert!(input.handle_key(ev));
        assert_eq!(input.selection(), Some((0, 5)));
    }

    #[test]
    fn ctrl_e_jumps_to_line_end_and_mac_ctrl_a_to_line_start() {
        // On macOS builds ^A/^E are the bytes cmd+left/right send; the
        // fold turns them into Home/End before the match sees them.
        let fold = |c| {
            crate::keys::fold_text_nav_bytes(
                KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL),
                true,
            )
        };
        let mut input = LineInput::new("hello");
        input.set_cursor(2);
        assert!(input.handle_key(fold('a')));
        assert_eq!(input.cursor(), 0);
        assert_eq!(input.selection(), None);
        assert!(input.handle_key(fold('e')));
        assert_eq!(input.cursor(), 5);
    }

    #[test]
    fn alt_b_and_alt_f_are_word_motions_on_every_platform() {
        // ESC b / ESC f: option+arrows as mac terminals deliver them,
        // folded to ctrl+arrow word hops on macOS builds.
        let fold = |c, mods: KeyModifiers| {
            crate::keys::fold_text_nav_bytes(KeyEvent::new(KeyCode::Char(c), mods), true)
        };
        let mut input = LineInput::new("one two three");
        input.set_cursor(13);
        assert!(input.handle_key(fold('b', KeyModifiers::ALT)));
        assert_eq!(input.cursor(), 8, "alt+b hops a word left");
        assert!(input.handle_key(fold('f', KeyModifiers::ALT)));
        assert_eq!(input.cursor(), 13, "alt+f hops a word right");
        // The shifted spellings select the same hop.
        assert!(input.handle_key(fold('B', KeyModifiers::ALT)));
        assert_eq!(input.selected_text().as_deref(), Some("three"));
        assert!(input.handle_key(fold('f', KeyModifiers::ALT | KeyModifiers::SHIFT)));
        assert_eq!(input.selection(), None, "selection collapsed to anchor");
    }

    #[test]
    fn empty_selection_is_none_and_set_cursor_clears_the_anchor() {
        let mut input = LineInput::new("abc");
        input.handle_key(shifted(KeyCode::Left));
        input.handle_key(shifted(KeyCode::Right)); // back to the anchor
        assert_eq!(input.selection(), None, "anchor == cursor is no selection");
        input.handle_key(shifted(KeyCode::Left));
        assert!(input.selection().is_some());
        input.set_cursor(0);
        assert_eq!(input.selection(), None, "set_cursor drops the anchor");
    }

    #[test]
    fn mouse_selection_api_extends_from_the_anchor() {
        let mut input = LineInput::new("hello world");
        input.set_cursor(0);
        input.begin_mouse_selection();
        input.set_cursor_extending(5);
        assert_eq!(input.selection(), Some((0, 5)));
        assert_eq!(input.selected_text().as_deref(), Some("hello"));
        input.select_all();
        assert_eq!(input.selection(), Some((0, 11)));
        input.clear_selection();
        assert_eq!(input.selection(), None);
    }

    #[test]
    fn selected_cells_render_reversed() {
        let mut input = LineInput::new("abcd");
        input.handle_key(code(KeyCode::Home));
        input.handle_key(shifted(KeyCode::Right));
        input.handle_key(shifted(KeyCode::Right));
        let theme = Theme::dark();
        let line = input.draw_line(true, &theme);
        // Walk the spans char-by-char and record which cells carry REVERSED.
        let mut reversed = Vec::new();
        for span in &line.spans {
            for ch in span.content.chars() {
                reversed.push((ch, span.style.add_modifier.contains(Modifier::REVERSED)));
            }
        }
        assert_eq!(reversed[0], ('a', true), "selected");
        assert_eq!(reversed[1], ('b', true), "selected");
        assert_eq!(reversed[2], ('c', false), "the caret hides while a selection is live");
        assert_eq!(reversed[3], ('d', false), "outside the selection");
    }

    #[test]
    fn delete_back_to_removes_up_to_the_cursor_and_drops_the_selection() {
        let mut input = LineInput::new("abcdef");
        input.set_cursor(4);
        input.delete_back_to(1);
        assert_eq!(input.text(), "aef");
        assert_eq!(input.cursor(), 1);
        input.select_all();
        input.set_cursor_extending(3);
        input.delete_back_to(5);
        assert_eq!(input.text(), "aef", "at or past the cursor: nothing");
        assert_eq!(input.selection(), None, "…but the selection is dropped");
    }

    #[test]
    fn no_caret_variant_paints_nothing_reversed_and_reports_the_column() {
        let theme = Theme::dark();
        let mut input = LineInput::new("hello");
        let line = input.draw_line_windowed_no_caret(true, &theme, 20);
        assert_eq!(line_text(&line), "hello", "no trailing caret cell");
        assert!(
            line.spans.iter().all(|s| !s.style.add_modifier.contains(Modifier::REVERSED)),
            "cursor past the end: nothing reversed"
        );
        assert_eq!(input.caret_column(20), 5);
        input.set_cursor(2);
        let line = input.draw_line_windowed_no_caret(true, &theme, 20);
        assert_eq!(line_text(&line), "hello");
        assert!(
            line.spans.iter().all(|s| !s.style.add_modifier.contains(Modifier::REVERSED)),
            "cursor mid-text: the cell under it stays plain"
        );
        assert_eq!(input.caret_column(20), 2);
    }

    #[test]
    fn no_caret_variant_still_follows_the_cursor_and_paints_a_selection() {
        let theme = Theme::dark();
        let mut input = LineInput::new("abcdefghijklmnop");
        // Cursor at the end, 8 wide: the window ends at the cursor, whose
        // cell (past the text) is the last column.
        let line = input.draw_line_windowed_no_caret(true, &theme, 8);
        assert_eq!(line_text(&line), "jklmnop");
        assert_eq!(input.caret_column(8), 7);
        input.select_all();
        let line = input.draw_line_windowed_no_caret(true, &theme, 8);
        assert!(
            line.spans.iter().all(|s| s.style.add_modifier.contains(Modifier::REVERSED)),
            "the selection is still drawn reversed"
        );
    }

    #[test]
    fn select_all_renders_no_trailing_caret_cell() {
        let mut input = LineInput::new("ab");
        input.handle_key(KeyEvent::new(KeyCode::Char('A'), KeyModifiers::CONTROL));
        let theme = Theme::dark();
        let line = input.draw_line(true, &theme);
        let rendered = line_text(&line);
        assert_eq!(rendered, "ab", "no reversed blank caret cell after the selection");
    }

    fn word_key(c: KeyCode, extra: KeyModifiers) -> KeyEvent {
        KeyEvent::new(c, extra)
    }

    #[test]
    fn ctrl_arrows_jump_by_word() {
        let mut input = LineInput::new("foo bar");
        input.handle_key(code(KeyCode::Home));
        assert!(input.handle_key(word_key(KeyCode::Right, KeyModifiers::CONTROL)));
        assert_eq!(input.cursor(), 3);
        assert!(input.handle_key(word_key(KeyCode::Right, KeyModifiers::CONTROL)));
        assert_eq!(input.cursor(), 7);
        assert!(input.handle_key(word_key(KeyCode::Left, KeyModifiers::CONTROL)));
        assert_eq!(input.cursor(), 4);
    }

    #[test]
    fn alt_arrows_jump_by_word_for_macos() {
        let mut input = LineInput::new("foo bar");
        input.handle_key(code(KeyCode::Home));
        assert!(input.handle_key(word_key(KeyCode::Right, KeyModifiers::ALT)));
        assert_eq!(input.cursor(), 3);
        assert!(input.handle_key(word_key(KeyCode::Left, KeyModifiers::ALT)));
        assert_eq!(input.cursor(), 0);
    }

    #[test]
    fn ctrl_shift_arrows_extend_the_selection_by_word() {
        let mut input = LineInput::new("foo bar");
        input.handle_key(code(KeyCode::Home));
        assert!(
            input.handle_key(word_key(KeyCode::Right, KeyModifiers::CONTROL | KeyModifiers::SHIFT))
        );
        assert_eq!(input.selection(), Some((0, 3)));
        assert_eq!(input.selected_text().as_deref(), Some("foo"));
    }

    #[test]
    fn ctrl_backspace_deletes_the_previous_word() {
        let mut input = LineInput::new("foo bar"); // cursor at end
        assert!(input.handle_key(word_key(KeyCode::Backspace, KeyModifiers::CONTROL)));
        assert_eq!(input.text(), "foo ");
        assert_eq!(input.cursor(), 4);
        // The macOS spelling of the same gesture.
        assert!(input.handle_key(word_key(KeyCode::Backspace, KeyModifiers::ALT)));
        assert_eq!(input.text(), "");
    }

    #[test]
    fn ctrl_h_is_word_backspace_for_legacy_terminals() {
        // Terminals without the enhanced-keys protocol deliver a physical
        // ctrl+backspace as the 0x08 byte, which crossterm parses as
        // ctrl+h — so ctrl+h must mean the same word deletion.
        let mut input = LineInput::new("foo bar");
        assert!(input.handle_key(word_key(KeyCode::Char('h'), KeyModifiers::CONTROL)));
        assert_eq!(input.text(), "foo ");
    }

    #[test]
    fn word_backspace_with_a_selection_removes_only_the_selection() {
        let mut input = LineInput::new("foo bar");
        input.handle_key(code(KeyCode::Home));
        input.handle_key(shifted(KeyCode::Right));
        assert!(input.handle_key(word_key(KeyCode::Backspace, KeyModifiers::CONTROL)));
        assert_eq!(input.text(), "oo bar");
    }

    #[test]
    fn caret_column_counts_cells_not_chars_past_wide_characters() {
        let mut input = LineInput::new("日本x");
        input.set_cursor(2);
        assert_eq!(input.caret_column(20), 4, "two wide chars = four cells");
        input.set_cursor(3);
        assert_eq!(input.caret_column(20), 5);
        input.set_cursor(0);
        assert_eq!(input.caret_column(20), 0);
    }

    #[test]
    fn span_columns_are_cells_in_the_drawn_window() {
        // Bytes: `日` 0..3, `本` 3..6, `x` 6, `y` 7, `z` 8.
        // Unfocused, drawn from char 0: wide chars count two cells.
        let mut input = LineInput::new("日本xyz");
        assert_eq!(input.span_columns(false, 20, 6..7), Some(4..5));
        assert_eq!(input.span_columns(false, 20, 3..7), Some(2..5));
        // Nothing to underline: empty, off a char boundary, past the end.
        assert_eq!(input.span_columns(false, 20, 6..6), None);
        assert_eq!(input.span_columns(false, 20, 1..3), None);
        assert_eq!(input.span_columns(false, 20, 8..10), None);
        // Clipped to the window: `日本x` fills 5 cells, `y` is cut off.
        assert_eq!(input.span_columns(false, 5, 6..8), Some(4..5));
        assert_eq!(input.span_columns(false, 5, 7..9), None);
        // Focused at the end, the window scrolls: `本xyz` + the caret's
        // blank fill 6 cells, so `日` is out of view and `x` sits at 2.
        input.set_cursor(5);
        assert_eq!(input.window_start(true, 6), 1);
        assert_eq!(input.span_columns(true, 6, 6..7), Some(2..3));
        assert_eq!(input.span_columns(true, 6, 0..7), Some(0..3));
        assert_eq!(input.span_columns(true, 6, 0..3), None);
        // Measured by grapheme, as the field is laid out: a combining mark
        // has no cell of its own, so a span of it alone underlines the
        // glyph it sits on.
        let input = LineInput::new("本\u{301}x");
        assert_eq!(input.span_columns(false, 20, 0..6), Some(0..3));
        assert_eq!(input.span_columns(false, 20, 3..5), Some(0..2));
        assert_eq!(input.span_columns(false, 20, 5..6), Some(2..3));
    }

    /// The caret is a whole grapheme: on either char of `❤️` (the heart
    /// and its presentation selector) it reverses both cells, and its
    /// column is where the emoji starts.
    #[test]
    fn a_caret_inside_a_grapheme_paints_the_whole_grapheme() {
        let theme = Theme::dark();
        let mut input = LineInput::new("a\u{2764}\u{fe0f}b");
        for cursor in [1, 2] {
            input.set_cursor(cursor);
            let line = input.draw_line(true, &theme);
            let reversed: Vec<&str> = line
                .spans
                .iter()
                .filter(|s| s.style.add_modifier.contains(Modifier::REVERSED))
                .map(|s| s.content.as_ref())
                .collect();
            assert_eq!(reversed, ["\u{2764}\u{fe0f}"], "cursor {cursor}");
            assert_eq!(input.caret_column(20), 1, "cursor {cursor}");
        }
        input.set_cursor(3);
        assert_eq!(input.caret_column(20), 3, "past the emoji's two cells");
    }

    /// The window scrolls by grapheme cells: three `❤️` (two cells each)
    /// and the caret's blank in five cells keep the last two whole.
    #[test]
    fn the_window_measures_graphemes_in_cells() {
        let theme = Theme::dark();
        let heart = "\u{2764}\u{fe0f}";
        let input = LineInput::new(&heart.repeat(3));
        assert_eq!(input.window_start(true, 5), 2);
        assert_eq!(input.visible_window(true, 5), heart.repeat(2));
        assert_eq!(input.caret_column(5), 4);
        let unfocused = line_text(&input.draw_line_windowed(false, &theme, 5));
        assert_eq!(unfocused, heart.repeat(2), "a third would take cells 5 and 6");
    }

    const HEART: &str = "\u{2764}\u{fe0f}";
    const FAMILY: &str = "\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}";

    /// What looks like one thing is one press: ←/→ cross a whole grapheme.
    #[test]
    fn arrows_step_over_a_whole_grapheme() {
        let mut input = LineInput::new(&format!("a{HEART}{FAMILY}b"));
        input.handle_key(code(KeyCode::Home));
        let mut stops = Vec::new();
        for _ in 0..4 {
            input.handle_key(code(KeyCode::Right));
            stops.push(input.cursor());
        }
        assert_eq!(stops, [1, 3, 8, 9]);
        input.handle_key(code(KeyCode::Left));
        input.handle_key(code(KeyCode::Left));
        assert_eq!(input.cursor(), 3);
    }

    /// shift+→ selects the whole grapheme, so what is highlighted is what
    /// a copy takes.
    #[test]
    fn shift_arrows_select_whole_graphemes() {
        let mut input = LineInput::new(&format!("e\u{301}{HEART}"));
        input.handle_key(code(KeyCode::Home));
        input.handle_key(shifted(KeyCode::Right));
        assert_eq!(input.selected_text().as_deref(), Some("e\u{301}"));
        input.handle_key(shifted(KeyCode::Right));
        assert_eq!(input.selected_text().unwrap(), format!("e\u{301}{HEART}"));
    }

    /// Backspace and Delete remove the whole grapheme, an accent with its
    /// letter (user, 2026-10-17).
    #[test]
    fn backspace_and_delete_remove_a_whole_grapheme() {
        let mut input = LineInput::new(&format!("ae\u{301}{HEART}"));
        input.handle_key(code(KeyCode::Backspace));
        assert_eq!(input.text(), "ae\u{301}");
        input.handle_key(code(KeyCode::Backspace));
        assert_eq!(input.text(), "a");
        let mut input = LineInput::new(&format!("a{FAMILY}b"));
        input.set_cursor(1);
        input.handle_key(code(KeyCode::Delete));
        assert_eq!(input.text(), "ab");
    }

    /// A word hop treats a grapheme as its first char: a decomposed `é`
    /// stays inside its word.
    #[test]
    fn word_motions_keep_graphemes_whole() {
        let mut input = LineInput::new("cafe\u{301} bar");
        input.handle_key(word_key(KeyCode::Left, KeyModifiers::CONTROL));
        assert_eq!(input.cursor(), 6);
        input.handle_key(word_key(KeyCode::Left, KeyModifiers::CONTROL));
        assert_eq!(input.cursor(), 0);
        let mut input = LineInput::new("x cafe\u{301}");
        input.handle_key(word_key(KeyCode::Backspace, KeyModifiers::CONTROL));
        assert_eq!(input.text(), "x ");
    }

    /// A double click's word widens to whole graphemes.
    #[test]
    fn a_double_click_word_keeps_its_marks() {
        let mut input = LineInput::new("cafe\u{301} bar");
        input.select_word_at(1);
        assert_eq!(input.selected_text().as_deref(), Some("cafe\u{301}"));
        let mut input = LineInput::new(&format!("a {FAMILY} b"));
        input.select_word_at(2);
        assert_eq!(input.selected_text().as_deref(), Some(FAMILY));
    }

    /// A cursor the vim engine left inside a grapheme works from the
    /// grapheme's start, where its caret is drawn: no key or edit splits
    /// the grapheme.
    #[test]
    fn a_cursor_inside_a_grapheme_works_from_its_start() {
        let inside = || {
            let mut input = LineInput::new(&format!("a{HEART}b"));
            input.set_cursor(2);
            input
        };
        let after = |key| {
            let mut input = inside();
            input.handle_key(code(key));
            (input.text().to_string(), input.cursor())
        };
        assert_eq!(after(KeyCode::Right), (format!("a{HEART}b"), 3));
        assert_eq!(after(KeyCode::Left), (format!("a{HEART}b"), 0));
        assert_eq!(after(KeyCode::Backspace), (format!("{HEART}b"), 0));
        assert_eq!(after(KeyCode::Delete), ("ab".to_string(), 1));
        assert_eq!(after(KeyCode::Char('x')), (format!("ax{HEART}b"), 2));
        let mut input = inside();
        input.paste("yz");
        assert_eq!(input.text(), format!("ayz{HEART}b"));
    }

    /// A grapheme that takes no cell (a zero-width space, a control char)
    /// shows as Vim's muted code for it, so nothing in a field is
    /// invisible (user, 2026-10-17).
    #[test]
    fn a_zero_width_grapheme_shows_its_vim_code() {
        let theme = Theme::dark();
        let shown = |text: &str| line_text(&LineInput::new(text).draw_line(false, &theme));
        assert_eq!(shown("abc\u{200b}def"), "abc<200b>def");
        assert_eq!(shown("a\tb"), "a^Ib");
        assert_eq!(shown("a\u{7f}\u{85}b"), "a^?<85>b");
        assert_eq!(shown("a\r\nb"), "a^M^Jb");
        let line = LineInput::new("a\u{200b}b").draw_line(false, &theme);
        let code = line.spans.iter().find(|s| s.content == "<200b>").expect("its own span");
        assert_eq!(code.style.fg, Some(theme.text_muted));
    }

    /// The caret on a zero-width grapheme reverses its whole code, and →
    /// crosses the code in one press.
    #[test]
    fn the_caret_on_a_zero_width_grapheme_reverses_its_code() {
        let theme = Theme::dark();
        let mut input = LineInput::new("abc\u{200b}def");
        input.set_cursor(3);
        let line = input.draw_line(true, &theme);
        let reversed: Vec<&str> = line
            .spans
            .iter()
            .filter(|s| s.style.add_modifier.contains(Modifier::REVERSED))
            .map(|s| s.content.as_ref())
            .collect();
        assert_eq!(reversed, ["<200b>"]);
        assert_eq!(input.caret_column(20), 3);
        input.handle_key(code(KeyCode::Right));
        assert_eq!((input.cursor(), input.caret_column(20)), (4, 9));
    }

    /// The code lays out as drawn: the window, clicks and the text a
    /// painter reads all count its cells, and a mask hides each of them.
    #[test]
    fn a_zero_width_code_takes_its_cells_in_the_layout() {
        let mut input = LineInput::new("abcde\u{200b}");
        input.set_cursor(5);
        assert_eq!(input.window_start(true, 10), 1, "the code's 6 cells and 4 letters");
        assert_eq!(input.caret_column(10), 4);
        assert_eq!(input.visible_window(true, 10), "bcde<200b>");
        let input = LineInput::new("a\u{200b}b");
        let at = |col| input.char_at_column(false, 20, col);
        assert_eq!([at(0), at(1), at(6), at(7)], [0, 1, 1, 2]);
        assert_eq!(input.span_columns(false, 20, 1..4), Some(1..7));
        let masked = line_text(&input.draw_line_masked(false, &Theme::dark()));
        assert_eq!(masked, "\u{25cf}".repeat(8));
    }

    /// At rest a value draws in the cell's color, with its codes muted as
    /// in the field it opens into, laid out as that field's unfocused draw.
    #[test]
    fn a_value_at_rest_draws_as_its_field_in_the_cells_color() {
        let theme = Theme::dark();
        let input = LineInput::new("a\tb");
        let line = input.draw_line_at_rest(&theme, theme.error, 10);
        let styled: Vec<(&str, _)> =
            line.spans.iter().map(|s| (s.content.as_ref(), s.style.fg)).collect();
        assert_eq!(
            styled,
            [("a", Some(theme.error)), ("^I", Some(theme.text_muted)), ("b", Some(theme.error))]
        );
        assert_eq!(line_text(&line), line_text(&input.draw_line_windowed(false, &theme, 10)));
    }

    /// A code wider than what is left of the window shows the cells that
    /// fit, so the caret on it in a narrow field is still a reversed code,
    /// not a blank; the layout counts the cut cells.
    #[test]
    fn a_code_the_window_edge_cuts_shows_what_fits() {
        let theme = Theme::dark();
        let mut input = LineInput::new("ab\u{200b}cd");
        input.set_cursor(2);
        let line = input.draw_line_windowed(true, &theme, 4);
        assert_eq!(line_text(&line), "<200");
        assert!(line.spans[0].style.add_modifier.contains(Modifier::REVERSED));
        assert_eq!(input.visible_window(true, 4), "<200");
        assert_eq!(input.span_columns(true, 4, 2..5), Some(0..4));
        assert_eq!(input.char_at_column(true, 4, 3), 2);
        let unfocused = LineInput::new("ab\u{200b}cd");
        assert_eq!(line_text(&unfocused.draw_line_windowed(false, &theme, 5)), "ab<20");
        assert_eq!(
            line_text(&unfocused.draw_line_windowed_masked(false, &theme, 5)),
            "\u{25cf}".repeat(5)
        );
    }

    /// A selection that covers part of a grapheme (the vim engine's Visual
    /// goes by its own chars) paints all of it, so the mark is never split
    /// off its letter into a span of its own.
    #[test]
    fn a_selection_paints_every_grapheme_it_touches() {
        let mut input = LineInput::new("e\u{301}x");
        input.set_cursor(0);
        input.set_visual(Some((0, VisualShape::Char)));
        let line = input.draw_line(true, &Theme::dark());
        assert_eq!(line.spans[0].content, "e\u{301}");
        assert!(line.spans[0].style.add_modifier.contains(Modifier::REVERSED));
        assert_eq!(line.spans[1].content, "x");
    }

    /// A masked field shows a dot per cell, so it lays out as the value
    /// it hides would: the same window, caret and clicks.
    #[test]
    fn a_masked_field_shows_a_dot_per_cell() {
        let input = LineInput::new("日e\u{301}");
        let masked = line_text(&input.draw_line_masked(false, &Theme::dark()));
        assert_eq!(masked, "\u{25cf}".repeat(3));
    }

    /// A column maps to the grapheme painted there: either cell of a wide
    /// glyph lands on its first char, past the text on its end.
    #[test]
    fn char_at_column_reads_the_drawn_cells() {
        let input = LineInput::new("日本x");
        let at = |col| input.char_at_column(false, 20, col);
        assert_eq!([at(0), at(1), at(2), at(3), at(4), at(5), at(9)], [0, 0, 1, 1, 2, 3, 3]);
        let input = LineInput::new("\u{2764}\u{fe0f}b");
        let at = |col| input.char_at_column(false, 20, col);
        assert_eq!([at(0), at(1), at(2)], [0, 0, 2]);
        // Focused at the end, from the window's first char.
        let input = LineInput::new("日本語日本語");
        assert_eq!(input.window_start(true, 5), 4);
        assert_eq!(input.char_at_column(true, 5, 0), 4);
        assert_eq!(input.char_at_column(true, 5, 2), 5);
    }

    #[test]
    fn window_scrolls_by_cells_so_the_caret_stays_inside_a_narrow_field() {
        let theme = Theme::dark();
        // 12 wide chars in a 10-cell field, cursor at the end: the window
        // must hold at most 4 wide chars (8 cells) plus the caret's blank.
        let input = LineInput::new("日本語日本語日本語日本語");
        assert!(input.caret_column(10) < 10, "{}", input.caret_column(10));
        assert_eq!(input.caret_column(10), 8);
        assert_eq!(input.window_start(true, 10), 8);
        assert_eq!(input.visible_window(true, 10), "語日本語");
        let line = input.draw_line_windowed_no_caret(true, &theme, 10);
        assert_eq!(line_text(&line), "語日本語");
        // Mid-text on a wide char: that char's two cells count too.
        let mut input = input;
        input.set_cursor(6);
        assert_eq!(input.window_start(true, 10), 2);
        assert_eq!(input.caret_column(10), 8);
    }

    #[test]
    fn windowed_selection_paints_only_the_visible_slice() {
        let text: String = (0..40).map(|i| char::from(b'a' + (i % 26) as u8)).collect();
        let mut input = LineInput::new(&text); // cursor at end
        input.handle_key(shifted(KeyCode::Home)); // select all, cursor 0
        // Cursor at 0 keeps the window at the head; everything visible is
        // selected, and nothing panics even though the range extends past
        // the window.
        let theme = Theme::dark();
        let line = input.draw_line_windowed(true, &theme, 10);
        let total: usize = line.spans.iter().map(|s| s.content.chars().count()).sum();
        assert!(total <= 10);
        assert!(
            line.spans.iter().all(|s| s.style.add_modifier.contains(Modifier::REVERSED)),
            "every visible cell is inside the selection"
        );
    }

    #[test]
    fn windowed_unfocused_long_text_renders_from_zero() {
        let text: String = (0..200).map(|i| char::from(b'a' + (i % 26) as u8)).collect();
        let input = LineInput::new(&text);
        let theme = Theme::dark();
        let line = input.draw_line_windowed(false, &theme, 20);
        let rendered = line_text(&line);
        assert_eq!(rendered, text.chars().take(20).collect::<String>());
    }

    /// Piece 3 §3.3: the engine's charwise Visual paints anchor..=caret,
    /// inclusive of the char under the caret (Vim's `selection=inclusive`).
    #[test]
    fn engine_visual_char_paints_through_the_caret_char() {
        let mut input = LineInput::new("abcdef");
        input.set_cursor(3);
        input.set_visual(Some((1, VisualShape::Char)));
        assert_eq!(input.paint_span(), Some((1, 4)));
        input.set_cursor(0);
        input.set_visual(Some((1, VisualShape::Char)));
        assert_eq!(input.paint_span(), Some((0, 2)), "a caret before the anchor");
    }

    #[test]
    fn engine_visual_line_paints_the_whole_text() {
        let mut input = LineInput::new("abc");
        input.set_cursor(1);
        input.set_visual(Some((1, VisualShape::Line)));
        assert_eq!(input.paint_span(), Some((0, 3)));
        let mut empty = LineInput::new("");
        empty.set_visual(Some((0, VisualShape::Line)));
        assert_eq!(empty.paint_span(), None, "nothing to paint on an empty field");
    }

    #[test]
    fn engine_visual_renders_reversed_and_clears() {
        let mut input = LineInput::new("abcd");
        input.set_cursor(2);
        input.set_visual(Some((1, VisualShape::Char)));
        let theme = Theme::dark();
        let reversed: Vec<bool> = input
            .draw_line(true, &theme)
            .spans
            .iter()
            .flat_map(|s| {
                s.content.chars().map(move |_| s.style.add_modifier.contains(Modifier::REVERSED))
            })
            .collect();
        assert_eq!(reversed, [false, true, true, false]);
        input.set_visual(None);
        assert_eq!(input.visual(), None);
        assert_eq!(input.paint_span(), None);
    }

    /// Piece 3 §8.4: the arrows profile never touches the engine's paint,
    /// and its own shift-selection paints exactly as before.
    #[test]
    fn handle_key_never_sets_the_engine_visual() {
        let mut input = LineInput::new("hello world");
        for ev in [
            shifted(KeyCode::Left),
            shifted(KeyCode::Left),
            key('x'),
            code(KeyCode::Home),
            shifted(KeyCode::End),
            code(KeyCode::Backspace),
            KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL | KeyModifiers::SHIFT),
        ] {
            input.handle_key(ev);
            assert_eq!(input.visual(), None);
            assert_eq!(input.paint_span(), input.selection());
        }
    }

    #[test]
    fn splice_raw_is_char_indexed_and_records_nothing() {
        let mut input = LineInput::new("héllo wörld");
        input.splice_raw(1, 2, "e");
        assert_eq!(input.text(), "hello wörld");
        assert_eq!(input.cursor(), 2, "the caret lands after the inserted text");
        input.splice_raw(6, 11, "日本");
        assert_eq!(input.text(), "hello 日本");
        assert_eq!(input.cursor(), 8);
        assert!(!input.edited(), "the engine keeps its own history");
    }

    #[test]
    fn splice_raw_drops_a_gui_selection() {
        let mut input = LineInput::new("abcdef");
        input.set_cursor(1);
        input.begin_mouse_selection();
        input.extend_mouse_selection_to(4);
        input.splice_raw(0, 0, "x");
        assert_eq!(input.selection(), None);
        assert_eq!(input.text(), "xabcdef");
    }
}

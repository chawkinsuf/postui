use crate::theme::Theme;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

/// A single-line, unicode-safe text input. Cursor position is a char index
/// (not a byte offset), so operations stay correct with multi-byte
/// characters. Shared by the URL field, prompt modals (Task 14), and
/// response search (Task 16).
#[derive(Debug, Clone, Default)]
pub struct LineInput {
    text: String,
    /// Cursor position, in chars (not bytes).
    cursor: usize,
    /// Selection anchor, in chars: the fixed end of a selection whose
    /// moving end is the cursor. `None` (or equal to the cursor) means no
    /// selection.
    anchor: Option<usize>,
}

impl LineInput {
    pub fn new(text: &str) -> Self {
        let cursor = text.chars().count();
        Self {
            text: text.to_string(),
            cursor,
            anchor: None,
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
    }

    pub fn select_all(&mut self) {
        self.anchor = Some(0);
        self.cursor = self.len_chars();
    }

    /// Anchors a mouse selection at the current cursor; subsequent
    /// [`Self::set_cursor_extending`] calls grow the selection to the drag
    /// point.
    pub fn begin_mouse_selection(&mut self) {
        self.anchor = Some(self.cursor);
    }

    /// Moves the cursor (clamped) while keeping the selection anchor, so a
    /// mouse drag extends the selection instead of collapsing it.
    pub fn set_cursor_extending(&mut self, idx: usize) {
        self.cursor = idx.min(self.len_chars());
    }

    /// Removes the selected text (cursor lands at the selection start).
    /// Returns whether a selection was removed.
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
        true
    }

    fn len_chars(&self) -> usize {
        self.text.chars().count()
    }

    /// Byte offset of the char at `idx` (or the end-of-string offset when
    /// `idx == len_chars()`).
    fn byte_offset(&self, idx: usize) -> usize {
        self.text
            .char_indices()
            .nth(idx)
            .map(|(b, _)| b)
            .unwrap_or(self.text.len())
    }

    /// Inserts `s` at the cursor and advances the cursor by `s`'s char
    /// count. Used to splice in multi-character text (e.g. a picked
    /// variable token) in one shot, rather than one `handle_key` per char.
    pub fn insert_str(&mut self, s: &str) {
        let at = self.byte_offset(self.cursor);
        self.text.insert_str(at, s);
        self.cursor += s.chars().count();
        self.anchor = None;
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
    fn word_target(&self, forward: bool) -> usize {
        let chars: Vec<char> = self.text.chars().collect();
        if forward {
            super::word_nav::next_word_boundary(&chars, self.cursor)
        } else {
            super::word_nav::prev_word_boundary(&chars, self.cursor)
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> bool {
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        // ctrl+arrow skips words; alt+arrow is the same gesture as macOS
        // terminals deliver it (option+arrow), so both spellings work on
        // every platform.
        let word = key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
        // Shifted motion extends a selection from an anchor planted at the
        // pre-move cursor; unshifted motion collapses any selection first
        // (Left/Right land on the selection's own edge, GUI-style).
        match key.code {
            KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.select_all();
                true
            }
            // A physical ctrl+backspace reaches a legacy terminal as the
            // 0x08 byte, which crossterm parses as ctrl+h — same word
            // deletion as the enhanced-keys `Backspace + CONTROL` below.
            KeyCode::Char('h') if key.modifiers.contains(KeyModifiers::CONTROL) => {
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
                self.delete_selection();
                let at = self.byte_offset(self.cursor);
                self.text.insert(at, c);
                self.cursor += 1;
                true
            }
            KeyCode::Backspace => {
                if self.delete_selection() {
                    return true;
                }
                // ctrl/alt+backspace removes the whole word behind the
                // caret, the same hop word-left would make.
                let target = if word {
                    self.word_target(false)
                } else {
                    self.cursor.saturating_sub(1)
                };
                if self.cursor > target {
                    let start = self.byte_offset(target);
                    let end = self.byte_offset(self.cursor);
                    self.text.replace_range(start..end, "");
                    self.cursor = target;
                }
                true
            }
            KeyCode::Delete => {
                if self.delete_selection() {
                    return true;
                }
                if self.cursor < self.len_chars() {
                    let start = self.byte_offset(self.cursor);
                    let end = self.byte_offset(self.cursor + 1);
                    self.text.replace_range(start..end, "");
                }
                true
            }
            KeyCode::Left => {
                if shift {
                    self.anchor.get_or_insert(self.cursor);
                    self.cursor = if word {
                        self.word_target(false)
                    } else {
                        self.cursor.saturating_sub(1)
                    };
                } else if !word && let Some((start, _)) = self.selection() {
                    self.cursor = start;
                    self.anchor = None;
                } else {
                    self.anchor = None;
                    self.cursor = if word {
                        self.word_target(false)
                    } else {
                        self.cursor.saturating_sub(1)
                    };
                }
                true
            }
            KeyCode::Right => {
                if shift {
                    self.anchor.get_or_insert(self.cursor);
                    self.cursor = if word {
                        self.word_target(true)
                    } else {
                        (self.cursor + 1).min(self.len_chars())
                    };
                } else if !word && let Some((_, end)) = self.selection() {
                    self.cursor = end;
                    self.anchor = None;
                } else {
                    self.anchor = None;
                    self.cursor = if word {
                        self.word_target(true)
                    } else {
                        (self.cursor + 1).min(self.len_chars())
                    };
                }
                true
            }
            KeyCode::Home => {
                if shift {
                    self.anchor.get_or_insert(self.cursor);
                } else {
                    self.anchor = None;
                }
                self.cursor = 0;
                true
            }
            KeyCode::End => {
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
        self.render(focused, theme, None, false)
    }

    /// Like [`Self::draw_line`], but every character renders as `●` — used
    /// for a secret cell's in-place edit (Variable Manager, spec §5), so
    /// the typed value never appears in plaintext on screen.
    pub fn draw_line_masked(&self, focused: bool, theme: &Theme) -> Line<'static> {
        self.render(focused, theme, None, true)
    }

    /// Like [`Self::draw_line`], but windowed to `width` columns: when
    /// focused, the window scrolls so the cursor always stays visible
    /// (in `[0, width)`), which matters once the text is longer than the
    /// pane — otherwise the caret (and everything past it) renders off
    /// the edge of the pane and the user can't see what they're typing.
    /// Unfocused text is never scrolled; it always renders from char 0
    /// (matching `draw_line`, just clipped to `width`).
    pub fn draw_line_windowed(&self, focused: bool, theme: &Theme, width: u16) -> Line<'static> {
        self.render_windowed(focused, theme, width, false)
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
        self.render_windowed(focused, theme, width, true)
    }

    /// The first char index [`Self::draw_line_windowed`] would render at a
    /// `width`-column window: 0 unless the input is focused and the caret
    /// has scrolled the window right. Callers that need to paint *over* the
    /// drawn text (inline `{{token}}` highlighting, spec §7) use this to
    /// slice the same visible window the draw used.
    pub fn window_start(&self, focused: bool, width: u16) -> usize {
        if !focused {
            return 0;
        }
        (self.cursor + 1).saturating_sub(width.max(1) as usize)
    }

    /// The exact text [`Self::draw_line_windowed`] puts on screen for a
    /// `width`-column window (unmasked; a masked field shows no tokens to
    /// highlight).
    pub fn visible_window(&self, focused: bool, width: u16) -> String {
        let start = self.window_start(focused, width);
        self.text
            .chars()
            .skip(start)
            .take(width.max(1) as usize)
            .collect()
    }

    fn render_windowed(
        &self,
        focused: bool,
        theme: &Theme,
        width: u16,
        mask: bool,
    ) -> Line<'static> {
        self.render(focused, theme, Some(width.max(1) as usize), mask)
    }

    /// The single renderer behind all the `draw_line*` variants: optionally
    /// masked, optionally windowed to `width` columns (the window scrolls
    /// with the cursor when focused), with the caret and any selected range
    /// rendered REVERSED.
    fn render(
        &self,
        focused: bool,
        theme: &Theme,
        width: Option<usize>,
        mask: bool,
    ) -> Line<'static> {
        let base = Style::default().fg(theme.text);
        let chars: Vec<char> = if mask {
            self.text.chars().map(|_| '\u{25cf}').collect()
        } else {
            self.text.chars().collect()
        };
        if !focused {
            let visible: String = match width {
                Some(w) => chars.iter().take(w).collect(),
                None => chars.iter().collect(),
            };
            return Line::styled(visible, base);
        }
        // Smallest window start that keeps the cursor visible; 0 when the
        // whole text is drawn.
        let start = match width {
            Some(w) => self.window_start(true, w.min(u16::MAX as usize) as u16),
            None => 0,
        };
        let end = match width {
            Some(w) => chars.len().min(start + w),
            None => chars.len(),
        };
        let reversed = base.add_modifier(Modifier::REVERSED);
        let selection = self.selection();
        // While a selection is live the reversed range *is* the visual
        // focus; the caret cell hides so it can't dangle outside the
        // selection's edge as a stray reversed cell.
        let style_at = |i: usize| {
            if selection.is_none() && i == self.cursor {
                return reversed;
            }
            match selection {
                Some((s, e)) if i >= s && i < e => reversed,
                _ => base,
            }
        };
        // Group consecutive same-styled cells into spans.
        let mut spans: Vec<Span<'static>> = Vec::new();
        let mut run = String::new();
        let mut run_style = base;
        for (i, &ch) in chars.iter().enumerate().take(end).skip(start) {
            let style = style_at(i);
            if style != run_style && !run.is_empty() {
                spans.push(Span::styled(std::mem::take(&mut run), run_style));
            }
            run_style = style;
            run.push(ch);
        }
        if !run.is_empty() {
            spans.push(Span::styled(run, run_style));
        }
        // The trailing caret cell when the cursor sits past the drawn text
        // — suppressed like the in-text caret while a selection is live.
        if selection.is_none() && self.cursor >= end {
            spans.push(Span::styled(" ", reversed));
        }
        Line::from(spans)
    }
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

    fn line_text(line: &Line<'static>) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn windowed_cursor_at_end_of_long_text_scrolls_so_the_slice_ends_at_the_cursor() {
        let text: String = (0..200)
            .map(|i| char::from(b'a' + (i % 26) as u8))
            .collect();
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
    fn windowed_cursor_at_zero_renders_from_zero() {
        let mut input = LineInput::new("hello world, this is long");
        input.handle_key(code(KeyCode::Home));
        let theme = Theme::dark();
        let line = input.draw_line_windowed(true, &theme, 10);
        let rendered = line_text(&line);
        assert!(
            rendered.starts_with('h'),
            "must start from char 0: {rendered:?}"
        );
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

    #[test]
    fn ctrl_a_selects_all() {
        let mut input = LineInput::new("hello");
        input.set_cursor(2);
        assert!(input.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL)));
        assert_eq!(input.selection(), Some((0, 5)));
        assert_eq!(input.selected_text().as_deref(), Some("hello"));
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
        assert_eq!(
            reversed[2],
            ('c', false),
            "the caret hides while a selection is live"
        );
        assert_eq!(reversed[3], ('d', false), "outside the selection");
    }

    #[test]
    fn select_all_renders_no_trailing_caret_cell() {
        let mut input = LineInput::new("ab");
        input.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL));
        let theme = Theme::dark();
        let line = input.draw_line(true, &theme);
        let rendered = line_text(&line);
        assert_eq!(
            rendered, "ab",
            "no reversed blank caret cell after the selection"
        );
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
        assert!(input.handle_key(word_key(
            KeyCode::Right,
            KeyModifiers::CONTROL | KeyModifiers::SHIFT
        )));
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
            line.spans
                .iter()
                .all(|s| s.style.add_modifier.contains(Modifier::REVERSED)),
            "every visible cell is inside the selection"
        );
    }

    #[test]
    fn windowed_unfocused_long_text_renders_from_zero() {
        let text: String = (0..200)
            .map(|i| char::from(b'a' + (i % 26) as u8))
            .collect();
        let input = LineInput::new(&text);
        let theme = Theme::dark();
        let line = input.draw_line_windowed(false, &theme, 20);
        let rendered = line_text(&line);
        assert_eq!(rendered, text.chars().take(20).collect::<String>());
    }
}

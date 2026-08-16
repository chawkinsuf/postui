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
}

impl LineInput {
    pub fn new(text: &str) -> Self {
        let cursor = text.chars().count();
        Self {
            text: text.to_string(),
            cursor,
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn cursor(&self) -> usize {
        self.cursor
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
    pub fn handle_key(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Char(c) if key.modifiers.difference(KeyModifiers::SHIFT).is_empty() => {
                let at = self.byte_offset(self.cursor);
                self.text.insert(at, c);
                self.cursor += 1;
                true
            }
            KeyCode::Backspace => {
                if self.cursor > 0 {
                    let start = self.byte_offset(self.cursor - 1);
                    let end = self.byte_offset(self.cursor);
                    self.text.replace_range(start..end, "");
                    self.cursor -= 1;
                }
                true
            }
            KeyCode::Delete => {
                if self.cursor < self.len_chars() {
                    let start = self.byte_offset(self.cursor);
                    let end = self.byte_offset(self.cursor + 1);
                    self.text.replace_range(start..end, "");
                }
                true
            }
            KeyCode::Left => {
                self.cursor = self.cursor.saturating_sub(1);
                true
            }
            KeyCode::Right => {
                if self.cursor < self.len_chars() {
                    self.cursor += 1;
                }
                true
            }
            KeyCode::Home => {
                self.cursor = 0;
                true
            }
            KeyCode::End => {
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
        let base = Style::default().fg(theme.text);
        if !focused {
            return Line::styled(self.text.clone(), base);
        }
        let chars: Vec<char> = self.text.chars().collect();
        let mut spans = Vec::new();
        if self.cursor > 0 {
            let before: String = chars[..self.cursor].iter().collect();
            spans.push(Span::styled(before, base));
        }
        let cursor_style = base.add_modifier(Modifier::REVERSED);
        if self.cursor < chars.len() {
            spans.push(Span::styled(chars[self.cursor].to_string(), cursor_style));
            if self.cursor + 1 < chars.len() {
                let after: String = chars[self.cursor + 1..].iter().collect();
                spans.push(Span::styled(after, base));
            }
        } else {
            spans.push(Span::styled(" ", cursor_style));
        }
        Line::from(spans)
    }

    /// Like [`Self::draw_line`], but windowed to `width` columns: when
    /// focused, the window scrolls so the cursor always stays visible
    /// (in `[0, width)`), which matters once the text is longer than the
    /// pane — otherwise the caret (and everything past it) renders off
    /// the edge of the pane and the user can't see what they're typing.
    /// Unfocused text is never scrolled; it always renders from char 0
    /// (matching `draw_line`, just clipped to `width`).
    pub fn draw_line_windowed(&self, focused: bool, theme: &Theme, width: u16) -> Line<'static> {
        let base = Style::default().fg(theme.text);
        let width = width.max(1) as usize;
        let chars: Vec<char> = self.text.chars().collect();
        if !focused {
            let visible: String = chars.iter().take(width).collect();
            return Line::styled(visible, base);
        }
        // Smallest `start` that keeps `cursor` within the last column of the
        // window; 0 when the cursor already fits without scrolling.
        let start = (self.cursor + 1).saturating_sub(width);
        let cursor_style = base.add_modifier(Modifier::REVERSED);
        let mut spans = Vec::new();
        if self.cursor > start {
            let before: String = chars[start..self.cursor].iter().collect();
            spans.push(Span::styled(before, base));
        }
        if self.cursor < chars.len() {
            spans.push(Span::styled(chars[self.cursor].to_string(), cursor_style));
            let after_end = (start + width).min(chars.len());
            if self.cursor + 1 < after_end {
                let after: String = chars[self.cursor + 1..after_end].iter().collect();
                spans.push(Span::styled(after, base));
            }
        } else {
            spans.push(Span::styled(" ", cursor_style));
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

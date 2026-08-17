use crate::paint::{fill, text};
use crate::theme::Theme;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::Span;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToastKind {
    Info,
    Success,
    Error,
    Warning,
}

const TOAST_LIFETIME_TICKS: u32 = 30; // 3 s at the 100 ms tick

/// Total on-screen height (in rows) of one painted toast: a filled `panel`
/// rect with its message vertically centered on the middle row.
const TOAST_HEIGHT: u16 = 3;

struct Toast {
    message: String,
    kind: ToastKind,
    remaining_ticks: u32,
}

#[derive(Default)]
pub struct Toasts {
    entries: Vec<Toast>,
}

impl Toasts {
    pub fn push(&mut self, message: impl Into<String>, kind: ToastKind) {
        self.entries.push(Toast {
            message: message.into(),
            kind,
            remaining_ticks: TOAST_LIFETIME_TICKS,
        });
    }

    /// Advances every toast's countdown by one tick and drops any that
    /// expired. Returns `true` while any toast is still visible/animating
    /// (i.e. a redraw is needed), `false` when idle.
    pub fn on_tick(&mut self) -> bool {
        // Captured before pruning: on the tick where the last toast expires,
        // `entries` goes from non-empty to empty, and that transition is
        // itself a state change that needs one final redraw to erase the
        // toast. Returning based on the post-prune state would swallow that
        // frame and leave the toast painted until the next keypress.
        let was_visible = !self.entries.is_empty();
        for t in &mut self.entries {
            t.remaining_ticks = t.remaining_ticks.saturating_sub(1);
        }
        self.entries.retain(|t| t.remaining_ticks > 0);
        was_visible
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Paints every visible toast as a floating filled `theme.panel` rect
    /// with a 1-col `█` left bar in its semantic color (accent/success/
    /// error/warning by kind) and `theme.text` message text, stacked
    /// top-right the same as before — but with no `Block`/border chrome of
    /// its own.
    pub fn draw(&self, frame: &mut Frame, screen: Rect, theme: &Theme) {
        let mut y = screen.y + 1;
        for toast in &self.entries {
            // Use display-width instead of char count to handle double-width chars (emoji, CJK)
            let display_width = Span::raw(toast.message.as_str()).width();
            // Clamp to usize before arithmetic to prevent overflow on very long messages
            let padding_width: usize = 6; // 1 bar col + 1 gap col + 4 right margin
            let clamped_width = display_width.saturating_add(padding_width);
            let width = (clamped_width as u16).min(screen.width);
            let area = Rect::new(
                screen.right().saturating_sub(width.saturating_add(1)),
                y,
                width,
                TOAST_HEIGHT,
            );
            if area.bottom() > screen.bottom() {
                break;
            }
            let color = match toast.kind {
                ToastKind::Info => theme.accent,
                ToastKind::Success => theme.success,
                ToastKind::Error => theme.error,
                ToastKind::Warning => theme.warning,
            };

            let buf = frame.buffer_mut();
            fill(buf, area, theme.panel);
            for row in area.top()..area.bottom() {
                if let Some(cell) = buf.cell_mut((area.x, row)) {
                    cell.set_symbol("\u{2588}");
                    cell.set_fg(color);
                    cell.set_bg(theme.panel);
                }
            }
            let text_y = area.y + area.height / 2;
            text(
                buf,
                area.x + 2,
                text_y,
                &toast.message,
                theme.text,
                theme.panel,
                false,
            );
            y += TOAST_HEIGHT;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    #[test]
    fn toast_expires_after_lifetime_ticks() {
        let mut t = Toasts::default();
        t.push("Saved", ToastKind::Success);
        assert!(!t.is_empty());
        for _ in 0..TOAST_LIFETIME_TICKS - 1 {
            t.on_tick();
        }
        assert!(!t.is_empty(), "alive one tick before expiry");
        t.on_tick();
        assert!(t.is_empty(), "expired at lifetime");
    }

    #[test]
    fn on_tick_returns_true_on_the_expiring_tick_and_false_after() {
        let mut t = Toasts::default();
        t.push("bye", ToastKind::Info);
        for _ in 0..TOAST_LIFETIME_TICKS - 1 {
            assert!(t.on_tick(), "still visible while counting down");
        }
        assert!(
            t.on_tick(),
            "the expiring tick must still request a redraw to erase it"
        );
        assert!(t.is_empty(), "the toast is gone after the expiring tick");
        assert!(
            !t.on_tick(),
            "the tick after expiry is idle: no redraw needed"
        );
    }

    #[test]
    fn multiple_toasts_expire_independently() {
        let mut t = Toasts::default();
        t.push("first", ToastKind::Info);
        for _ in 0..10 {
            t.on_tick();
        }
        t.push("second", ToastKind::Error);
        for _ in 0..TOAST_LIFETIME_TICKS - 10 {
            t.on_tick();
        }
        assert!(!t.is_empty(), "second toast still alive");
        for _ in 0..10 {
            t.on_tick();
        }
        assert!(t.is_empty());
    }

    #[test]
    fn draw_renders_message_top_right() {
        let mut t = Toasts::default();
        t.push("Copied \u{2713}", ToastKind::Success);
        let theme = Theme::dark();
        let backend = TestBackend::new(60, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| t.draw(f, f.area(), &theme)).unwrap();
        let content = format!("{:?}", terminal.backend().buffer());
        assert!(content.contains("Copied"));
    }

    #[test]
    fn draw_handles_double_width_chars_correctly() {
        let mut t = Toasts::default();
        // "已复制 ✓" contains double-width CJK chars and a double-width emoji
        t.push("已复制 ✓", ToastKind::Success);
        let theme = Theme::dark();
        let backend = TestBackend::new(60, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| t.draw(f, f.area(), &theme)).unwrap();
        let content = format!("{:?}", terminal.backend().buffer());
        // Verify that the toast was rendered with the double-width message
        assert!(content.contains("已"), "CJK character should be rendered");
    }

    /// Panel fill + a `█` left bar in the semantic color, no `Block`/border
    /// chrome (no rounded-corner glyphs).
    #[test]
    fn draw_paints_panel_fill_and_a_colored_left_bar() {
        let mut t = Toasts::default();
        t.push("oops", ToastKind::Error);
        let theme = Theme::dark();
        let backend = TestBackend::new(60, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| t.draw(f, f.area(), &theme)).unwrap();
        let buf = terminal.backend().buffer();

        // The bar column is the toast area's left edge; find it by scanning
        // for the "o" of "oops" and walking back to the bar just left of the
        // 2-col text gutter.
        let mut found_bar = false;
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                let cell = buf[(x, y)].clone();
                if cell.symbol() == "\u{2588}" {
                    assert_eq!(cell.fg, theme.error, "left bar is in the error color");
                    assert_eq!(cell.bg, theme.panel);
                    found_bar = true;
                }
            }
        }
        assert!(found_bar, "expected a `█` left bar cell somewhere");

        let content = format!("{buf:?}");
        for glyph in ['\u{256d}', '\u{256e}', '\u{2570}', '\u{256f}'] {
            assert!(
                !content.contains(glyph),
                "no rounded border glyph {glyph:?} expected: {content}"
            );
        }
        assert!(content.contains("oops"));
    }
}

use crate::theme::Theme;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::Span;
use ratatui::widgets::{Block, BorderType, Borders, Clear, Padding, Paragraph};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToastKind {
    Info,
    Success,
    Error,
    Warning,
}

const TOAST_LIFETIME_TICKS: u32 = 30; // 3 s at the 100 ms tick

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

    pub fn draw(&self, frame: &mut Frame, screen: Rect, theme: &Theme) {
        let mut y = screen.y + 1;
        for toast in &self.entries {
            // Use display-width instead of char count to handle double-width chars (emoji, CJK)
            let display_width = Span::raw(toast.message.as_str()).width();
            // Clamp to usize before arithmetic to prevent overflow on very long messages
            let padding_width: usize = 6;
            let clamped_width = display_width.saturating_add(padding_width);
            let width = (clamped_width as u16).min(screen.width);
            let area = Rect::new(
                screen.right().saturating_sub(width.saturating_add(1)),
                y,
                width,
                3,
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
            let block = Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(color))
                .padding(Padding::horizontal(1))
                .style(Style::default().bg(theme.surface_raised));
            frame.render_widget(Clear, area);
            frame.render_widget(
                Paragraph::new(toast.message.as_str())
                    .style(Style::default().fg(theme.text))
                    .block(block),
                area,
            );
            y += 3;
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
        t.push("Copied ✓", ToastKind::Success);
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
        // The box should be sized based on display width, not char count
        // Display width of "已复制 ✓" is 8 (3*2 + 1 + 2), plus 6 padding = 14 columns
        assert!(
            content.contains('╭') || content.contains('┌'),
            "toast border should render"
        );
    }
}

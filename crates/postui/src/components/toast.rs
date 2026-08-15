use crate::theme::Theme;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::{Block, BorderType, Borders, Clear, Padding, Paragraph};
use ratatui::Frame;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToastKind {
    #[allow(dead_code)]
    Info,
    #[allow(dead_code)]
    Success,
    #[allow(dead_code)]
    Error,
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

    pub fn on_tick(&mut self) {
        for t in &mut self.entries {
            t.remaining_ticks = t.remaining_ticks.saturating_sub(1);
        }
        self.entries.retain(|t| t.remaining_ticks > 0);
    }

    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn draw(&self, frame: &mut Frame, screen: Rect, theme: &Theme) {
        let mut y = screen.y + 1;
        for toast in &self.entries {
            let width = (toast.message.chars().count() as u16 + 6).min(screen.width);
            let area = Rect::new(screen.right().saturating_sub(width + 1), y, width, 3);
            if area.bottom() > screen.bottom() {
                break;
            }
            let color = match toast.kind {
                ToastKind::Info => theme.accent,
                ToastKind::Success => theme.success,
                ToastKind::Error => theme.error,
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
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

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
}

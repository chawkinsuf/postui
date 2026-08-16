use crate::layout::PaneId;
use crate::theme::Theme;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::Paragraph;

/// A typed clickable target. Registered with its screen Rect during render.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Hit {
    /// Pane background: click focuses, wheel scrolls.
    Pane(PaneId),
    HeaderProject,
    HeaderEnv,
    /// A footer hint chip that dispatches its action on click.
    FooterChip(crate::action::Action),
    SidebarNewRequest,
    /// Index into `sidebar.rows`.
    SidebarRow(usize),
    SidebarFolderArrow(usize),
    /// Renders as Cancel while a request is in flight.
    SendButton,
    CopyUrlButton,
    MethodSelector,
    /// 0 = Params, 1 = Headers, 2 = Body.
    EditorTab(usize),
    TableRow(usize),
    TableCheckbox(usize),
    /// Raw mouse event forwarded to edtui (click-to-place, wheel).
    BodyEditor,
    ResponseTab(crate::components::response::ViewMode),
    CopyBodyButton,
    SaveBodyButton,
    /// Copy icon on row `i` of the response Headers view.
    HeaderCopy(usize),
    /// Visible row `i` of the JSON tree (click selects).
    JsonRow(usize),
    /// The ▸/▾ glyph cell of visible row `i` (click toggles).
    JsonArrow(usize),
    ScrollbarThumb(PaneId),
    /// Signed page delta applied on click (±viewport height).
    ScrollbarTrack(PaneId, i16),
    DropdownRow(usize),
    ChooserRow(usize),
    PaletteRow(usize),
    VarPickerRow(usize),
    /// A clickable `[y] Label` chip in a Confirm modal.
    ConfirmChoice(char),
    /// Full-screen region under an open modal; click closes (same as Esc).
    ModalOutside,
}

/// Rebuilt each frame during render; maps screen regions to typed [`Hit`]s.
#[derive(Default)]
pub struct HitMap {
    regions: Vec<(Rect, Hit)>,
}

impl HitMap {
    pub fn clear(&mut self) {
        self.regions.clear();
    }

    pub fn register(&mut self, rect: Rect, hit: Hit) {
        self.regions.push((rect, hit));
    }

    /// Topmost (= last registered) hit containing the point.
    pub fn hit_at(&self, x: u16, y: u16) -> Option<&Hit> {
        self.regions
            .iter()
            .rev()
            .find(|(rect, _)| rect.contains(ratatui::layout::Position { x, y }))
            .map(|(_, hit)| hit)
    }

    /// Topmost `Hit::Pane` containing the point (for wheel routing).
    pub fn pane_at(&self, x: u16, y: u16) -> Option<PaneId> {
        self.regions.iter().rev().find_map(|(rect, hit)| match hit {
            Hit::Pane(pane) if rect.contains(ratatui::layout::Position { x, y }) => Some(*pane),
            _ => None,
        })
    }

    /// Last-registered rect for `hit` — the test helper click tests use.
    pub fn rect_of(&self, hit: &Hit) -> Option<Rect> {
        self.regions
            .iter()
            .rev()
            .find(|(_, h)| h == hit)
            .map(|(rect, _)| *rect)
    }
}

/// `[ label ]` rendered width, for layout math.
pub fn button_width(label: &str) -> u16 {
    // "[ " + label + " ]"
    (label.chars().count() + 4) as u16
}

/// Draws a bracketed button `[ label ]` and registers it (only when enabled).
/// Styling: accent fg at rest; inverted (accent bg, surface fg) when
/// `hovered == Some(&hit)`; text_muted and unregistered when disabled.
#[allow(clippy::too_many_arguments)]
pub fn button(
    frame: &mut Frame,
    hits: &mut HitMap,
    area: Rect,
    label: &str,
    hit: Hit,
    hovered: Option<&Hit>,
    enabled: bool,
    theme: &Theme,
) {
    let text = format!("[ {label} ]");
    let style = if !enabled {
        Style::default().fg(theme.text_muted)
    } else if hovered == Some(&hit) {
        Style::default().bg(theme.accent).fg(theme.surface)
    } else {
        Style::default().fg(theme.accent)
    };
    frame.render_widget(Paragraph::new(Line::styled(text, style)), area);
    if enabled {
        hits.register(area, hit);
    }
}

/// Same styling contract for a plain (unbracketed) clickable chip/label.
/// `base` overrides the rest (non-hovered) style — e.g. an active tab's
/// accent+bold — so hover inversion still lives in exactly one place while
/// callers vary the quiet state. `None` falls back to the plain accent
/// foreground `button`/the original `chip` contract used.
#[allow(clippy::too_many_arguments)]
pub fn chip(
    frame: &mut Frame,
    hits: &mut HitMap,
    area: Rect,
    label: &str,
    hit: Hit,
    hovered: Option<&Hit>,
    base: Option<Style>,
    theme: &Theme,
) {
    let style = if hovered == Some(&hit) {
        Style::default().bg(theme.accent).fg(theme.surface)
    } else {
        base.unwrap_or_else(|| Style::default().fg(theme.accent))
    };
    frame.render_widget(Paragraph::new(Line::styled(label.to_string(), style)), area);
    hits.register(area, hit);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    #[test]
    fn last_registered_hit_wins_at_a_point() {
        let mut m = HitMap::default();
        m.register(Rect::new(0, 0, 10, 10), Hit::Pane(PaneId::Sidebar));
        m.register(Rect::new(2, 2, 3, 1), Hit::SidebarRow(0));
        assert_eq!(m.hit_at(3, 2), Some(&Hit::SidebarRow(0)));
        assert_eq!(m.hit_at(0, 0), Some(&Hit::Pane(PaneId::Sidebar)));
        assert_eq!(m.hit_at(50, 50), None);
        assert_eq!(
            m.pane_at(3, 2),
            Some(PaneId::Sidebar),
            "pane_at sees through overlays"
        );
        assert_eq!(m.rect_of(&Hit::SidebarRow(0)), Some(Rect::new(2, 2, 3, 1)));
    }

    #[test]
    fn button_renders_and_registers_only_when_enabled() {
        let theme = Theme::for_terminal();
        let area = Rect::new(0, 0, 20, 1);

        // Enabled, not hovered: text present, registered.
        let backend = TestBackend::new(20, 1);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = HitMap::default();
        terminal
            .draw(|f| {
                button(
                    f,
                    &mut hits,
                    area,
                    "Send",
                    Hit::SendButton,
                    None,
                    true,
                    &theme,
                )
            })
            .unwrap();
        let content = format!("{:?}", terminal.backend().buffer());
        assert!(content.contains("[ Send ]"));
        assert_eq!(hits.rect_of(&Hit::SendButton), Some(area));

        // Disabled: not registered, muted styling.
        let backend = TestBackend::new(20, 1);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = HitMap::default();
        terminal
            .draw(|f| {
                button(
                    f,
                    &mut hits,
                    area,
                    "Send",
                    Hit::SendButton,
                    None,
                    false,
                    &theme,
                )
            })
            .unwrap();
        assert_eq!(hits.rect_of(&Hit::SendButton), None);
        let cell = terminal.backend().buffer()[(1, 0)].clone();
        assert_eq!(cell.fg, theme.text_muted);

        // Hovered: bg == theme.accent on a cell inside the button.
        let backend = TestBackend::new(20, 1);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = HitMap::default();
        terminal
            .draw(|f| {
                button(
                    f,
                    &mut hits,
                    area,
                    "Send",
                    Hit::SendButton,
                    Some(&Hit::SendButton),
                    true,
                    &theme,
                )
            })
            .unwrap();
        let cell = terminal.backend().buffer()[(1, 0)].clone();
        assert_eq!(cell.bg, theme.accent);
    }
}

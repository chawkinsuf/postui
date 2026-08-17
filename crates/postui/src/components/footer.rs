use crate::action::Action;
use crate::hit::{Hit, HitMap};
use crate::layout::PaneId;
use crate::paint::{fill, text};
use crate::theme::Theme;
use ratatui::Frame;
use ratatui::layout::Rect;

/// The footer is always exactly this many rows tall: a blank panel row on
/// top, the content row (chips + right-aligned quit), a blank panel row on
/// the bottom — the same painted 3-row rhythm as the header app bar.
pub const FOOTER_HEIGHT: u16 = 3;

/// The context-sensitive chips for the focused pane, plus the palette chip
/// always appended at the end. Each entry is `(key, label, action)`; `None`
/// actions render as plain (unregistered, muted) text on the panel
/// background rather than a filled chip — they describe a binding with no
/// single dispatchable `Action` (e.g. multi-key hints).
fn footer_chips(focus: PaneId) -> Vec<(&'static str, &'static str, Option<Action>)> {
    let mut chips: Vec<(&'static str, &'static str, Option<Action>)> = match focus {
        PaneId::Sidebar => vec![
            ("enter", "open", None),
            ("n", "new", Some(Action::PromptNewRequest)),
            ("r", "rename", Some(Action::PromptRenameRequest)),
            ("d", "delete", Some(Action::ConfirmDeleteRequest)),
        ],
        PaneId::Editor => vec![
            ("ctrl+r", "send", Some(Action::Send)),
            ("ctrl+s", "save", Some(Action::SaveRequest)),
            ("alt+1/2/3", "tabs", None),
        ],
        PaneId::Response => vec![
            ("r", "raw", None),
            ("h", "headers", None),
            ("/", "search", None),
        ],
    };
    chips.push(("^P", "commands", Some(Action::OpenPalette)));
    chips
}

/// `" q "` + `"quit "` — the always-present, right-aligned quit hint. Kept
/// separate from `footer_chips` since it never varies with focus and paints
/// plain muted text directly on the panel (not a `control`-filled chip).
const QUIT_LABEL: &str = " q quit ";

pub fn draw_footer(
    frame: &mut Frame,
    area: Rect,
    theme: &Theme,
    focus: PaneId,
    hits: &mut HitMap,
    hovered: Option<&Hit>,
) {
    let buf = frame.buffer_mut();
    fill(buf, area, theme.panel);

    if area.height == 0 {
        return;
    }
    let mid_y = area.y + area.height / 2;

    let quit_w = QUIT_LABEL.chars().count() as u16;
    let quit_x = (area.x + area.width).saturating_sub(quit_w);
    text(
        buf,
        quit_x,
        mid_y,
        QUIT_LABEL,
        theme.text_muted,
        theme.panel,
        false,
    );
    hits.register(
        Rect {
            x: quit_x,
            y: mid_y,
            width: quit_w,
            height: 1,
        },
        Hit::FooterChip(Action::Quit),
    );

    // Chips stop one column shy of the quit hint so the two never collide.
    let right_limit = quit_x.saturating_sub(1);
    let mut x = area.x + 1;
    for (key, label, action) in footer_chips(focus) {
        let key_text = format!(" {key}");
        let label_text = format!(" {label} ");
        let width = key_text.chars().count() as u16 + label_text.chars().count() as u16;
        if x + width > right_limit {
            break;
        }
        let chip_area = Rect {
            x,
            y: mid_y,
            width,
            height: 1,
        };
        let chip_hit = action.map(Hit::FooterChip);
        let chip_fill = if chip_hit.is_some() && hovered == chip_hit.as_ref() {
            theme.control_hover
        } else {
            theme.control
        };
        fill(buf, chip_area, chip_fill);
        text(buf, x, mid_y, &key_text, theme.accent, chip_fill, true);
        text(
            buf,
            x + key_text.chars().count() as u16,
            mid_y,
            &label_text,
            theme.text_muted,
            chip_fill,
            false,
        );
        if let Some(hit) = chip_hit {
            hits.register(chip_area, hit);
        }
        x += width + 2;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn render(focus: PaneId) -> String {
        let theme = Theme::for_terminal();
        let backend = TestBackend::new(120, FOOTER_HEIGHT);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal
            .draw(|f| draw_footer(f, f.area(), &theme, focus, &mut hits, None))
            .unwrap();
        format!("{:?}", terminal.backend().buffer())
    }

    #[test]
    fn sidebar_focus_shows_sidebar_hints() {
        let content = render(PaneId::Sidebar);
        assert!(content.contains("enter open"));
        assert!(content.contains("commands"));
        assert!(content.contains("quit"));
    }

    #[test]
    fn editor_focus_shows_editor_hints() {
        let content = render(PaneId::Editor);
        assert!(content.contains("ctrl+r send"));
    }

    #[test]
    fn response_focus_shows_response_hints() {
        let content = render(PaneId::Response);
        assert!(content.contains("r raw"));
        assert!(content.contains("/ search"));
    }

    #[test]
    fn action_chips_are_registered_as_hits() {
        let theme = Theme::for_terminal();
        let backend = TestBackend::new(120, FOOTER_HEIGHT);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal
            .draw(|f| draw_footer(f, f.area(), &theme, PaneId::Sidebar, &mut hits, None))
            .unwrap();
        assert!(
            hits.rect_of(&Hit::FooterChip(Action::PromptNewRequest))
                .is_some()
        );
        assert!(
            hits.rect_of(&Hit::FooterChip(Action::OpenPalette))
                .is_some()
        );
        assert!(hits.rect_of(&Hit::FooterChip(Action::Quit)).is_some());
    }

    #[test]
    fn footer_is_a_three_row_panel_toolbar() {
        let theme = Theme::for_terminal();
        let backend = TestBackend::new(120, FOOTER_HEIGHT);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal
            .draw(|f| draw_footer(f, f.area(), &theme, PaneId::Sidebar, &mut hits, None))
            .unwrap();
        let buf = terminal.backend().buffer();
        // Top and bottom rows are flat panel fill, no chip content.
        for y in [0u16, 2] {
            let cell = buf.cell((3, y)).unwrap();
            assert_eq!(cell.symbol(), " ");
            assert_eq!(cell.bg, theme.panel);
        }
    }

    #[test]
    fn chip_paints_accent_bold_key_and_muted_label_on_control_fill() {
        let theme = Theme::for_terminal();
        let backend = TestBackend::new(120, FOOTER_HEIGHT);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal
            .draw(|f| draw_footer(f, f.area(), &theme, PaneId::Sidebar, &mut hits, None))
            .unwrap();
        let rect = hits
            .rect_of(&Hit::FooterChip(Action::PromptNewRequest))
            .expect("new-request chip hit registered");
        let buf = terminal.backend().buffer();
        // First cell of the chip is a leading space, then the key glyph.
        let key_cell = buf.cell((rect.x + 1, rect.y)).unwrap();
        assert_eq!(key_cell.symbol(), "n");
        assert_eq!(key_cell.fg, theme.accent);
        assert_eq!(key_cell.bg, theme.control);
        assert!(key_cell.modifier.contains(ratatui::style::Modifier::BOLD));
        // The label follows on the same control fill, muted and not bold.
        let label_cell = buf.cell((rect.x + 3, rect.y)).unwrap();
        assert_eq!(label_cell.symbol(), "n"); // first letter of "new"
        assert_eq!(label_cell.fg, theme.text_muted);
        assert_eq!(label_cell.bg, theme.control);
        assert!(!label_cell.modifier.contains(ratatui::style::Modifier::BOLD));
    }

    #[test]
    fn quit_hint_is_right_aligned_muted_text_on_panel() {
        let theme = Theme::for_terminal();
        let backend = TestBackend::new(120, FOOTER_HEIGHT);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal
            .draw(|f| draw_footer(f, f.area(), &theme, PaneId::Sidebar, &mut hits, None))
            .unwrap();
        let rect = hits
            .rect_of(&Hit::FooterChip(Action::Quit))
            .expect("quit hit registered");
        assert_eq!(
            rect.x + rect.width,
            120,
            "quit hint sits flush against the footer's right edge"
        );
        let buf = terminal.backend().buffer();
        let cell = buf.cell((rect.x + 1, rect.y)).unwrap();
        assert_eq!(cell.symbol(), "q");
        assert_eq!(cell.fg, theme.text_muted);
        assert_eq!(cell.bg, theme.panel);
    }
}

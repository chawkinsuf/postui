use crate::action::Action;
use crate::hit::{Hit, HitMap};
use crate::layout::PaneId;
use crate::paint::{fill, text};
use crate::theme::Theme;
use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

/// The footer is always exactly this many rows tall: a blank panel row on
/// top, the content row (chips + right-aligned quit), a blank panel row on
/// the bottom — the same painted 3-row rhythm as the header app bar.
pub const FOOTER_HEIGHT: u16 = 3;

/// The context-sensitive chips for the focused pane, plus the palette chip
/// always appended at the end. Each entry is `(key, label, action)`; `None`
/// actions render as plain (unregistered, muted) text on the panel
/// background rather than a filled chip — they describe a binding with no
/// single dispatchable `Action` (e.g. multi-key hints). `pub(crate)` so
/// `app::tests`'s mouse-parity sweep (spec §5) can enumerate the same
/// actions `draw_footer` paints as chips, rather than a copy of this list.
pub(crate) fn footer_chips(focus: PaneId) -> Vec<(&'static str, &'static str, Option<Action>)> {
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
            // Arrows are the primary route (method ← URL ↓ tabs ↓ content);
            // alt+1/2/3 still work where the terminal passes them through.
            ("↑↓←→", "navigate", None),
        ],
        PaneId::Response => vec![
            (
                "r",
                "raw",
                Some(Action::ResponseViewMode(
                    crate::components::response::ViewMode::Raw,
                )),
            ),
            (
                "h",
                "headers",
                Some(Action::ResponseViewMode(
                    crate::components::response::ViewMode::Headers,
                )),
            ),
            ("/", "search", Some(Action::OpenResponseSearch)),
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
    let chips = footer_chips(focus);
    paint_chip_row(
        buf,
        mid_y,
        area.x + 1,
        right_limit,
        &chips,
        theme,
        hits,
        hovered,
    );
}

/// Paints a left-to-right row of `(key, label, action)` chips starting at
/// `start_x` on row `y`, stopping before drawing one that would cross
/// `right_limit`. Each chip with `Some(action)` is a `theme.control`-filled
/// pill (lifting to `theme.control_hover` under the mouse per `hovered`)
/// registering `Hit::FooterChip(action)`; a `None` action renders as plain
/// (unregistered, muted) text directly on the caller's background instead —
/// the fill IS the clickability signal, so a hint with no single
/// dispatchable action never visually promises a click it can't honor.
/// Shared by the footer's own hint row and the editor toolbar. Returns the
/// x position just past the last chip painted.
#[allow(clippy::too_many_arguments)]
pub fn paint_chip_row(
    buf: &mut Buffer,
    y: u16,
    start_x: u16,
    right_limit: u16,
    chips: &[(&str, &str, Option<Action>)],
    theme: &Theme,
    hits: &mut HitMap,
    hovered: Option<&Hit>,
) -> u16 {
    let mut x = start_x;
    for (key, label, action) in chips {
        let key_text = format!(" {key}");
        let label_text = format!(" {label} ");
        let width = key_text.chars().count() as u16 + label_text.chars().count() as u16;
        if x + width > right_limit {
            break;
        }
        let chip_area = Rect {
            x,
            y,
            width,
            height: 1,
        };
        let chip_bg = match action {
            Some(a) if hovered == Some(&Hit::FooterChip(a.clone())) => {
                fill(buf, chip_area, theme.control_hover);
                theme.control_hover
            }
            Some(_) => {
                fill(buf, chip_area, theme.control);
                theme.control
            }
            None => theme.panel,
        };
        text(buf, x, y, &key_text, theme.accent, chip_bg, true);
        text(
            buf,
            x + key_text.chars().count() as u16,
            y,
            &label_text,
            theme.text_muted,
            chip_bg,
            false,
        );
        if let Some(action) = action {
            hits.register(chip_area, Hit::FooterChip(action.clone()));
        }
        x += width + 2;
    }
    x
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

    /// Task 17, spec §5: the Response pane's `r`/`h`/`/` chips used to be
    /// plain unregistered text (`None` action) — they must now be clickable
    /// like every other chip.
    #[test]
    fn response_chips_are_clickable() {
        let theme = Theme::for_terminal();
        let backend = TestBackend::new(120, FOOTER_HEIGHT);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal
            .draw(|f| draw_footer(f, f.area(), &theme, PaneId::Response, &mut hits, None))
            .unwrap();
        use crate::components::response::ViewMode;
        assert!(
            hits.rect_of(&Hit::FooterChip(Action::ResponseViewMode(ViewMode::Raw)))
                .is_some()
        );
        assert!(
            hits.rect_of(&Hit::FooterChip(Action::ResponseViewMode(
                ViewMode::Headers
            )))
            .is_some()
        );
        assert!(
            hits.rect_of(&Hit::FooterChip(Action::OpenResponseSearch))
                .is_some()
        );
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

    /// Controller ruling: the chip fill IS the clickability signal.
    /// Non-clickable (`None`-action) entries must render as plain text
    /// directly on the panel — no fill, no hover — while clickable entries
    /// keep their `control` chip fill.
    #[test]
    fn only_clickable_entries_get_the_control_chip_fill() {
        let theme = Theme::for_terminal();
        let backend = TestBackend::new(120, FOOTER_HEIGHT);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal
            .draw(|f| draw_footer(f, f.area(), &theme, PaneId::Sidebar, &mut hits, None))
            .unwrap();
        let buf = terminal.backend().buffer();

        // "enter open" (Sidebar's first binding) has no dispatchable
        // Action: plain text on the panel, not a chip.
        assert!(hits.rect_of(&Hit::FooterChip(Action::Quit)).is_some()); // sanity: hits exist at all
        let plain_x = 1u16; // area.x + 1, the loop's starting column
        let plain_cell = buf.cell((plain_x + 1, 1)).unwrap();
        assert_eq!(plain_cell.symbol(), "e", "first letter of \"enter\"");
        assert_eq!(
            plain_cell.bg, theme.panel,
            "non-clickable entry is unfilled"
        );
        assert_eq!(plain_cell.fg, theme.accent);

        // "n new" (PromptNewRequest) IS clickable: control-filled chip.
        let rect = hits
            .rect_of(&Hit::FooterChip(Action::PromptNewRequest))
            .expect("new-request chip hit registered");
        let chip_cell = buf.cell((rect.x + 1, rect.y)).unwrap();
        assert_eq!(
            chip_cell.bg, theme.control,
            "clickable entry keeps its chip fill"
        );
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

    /// Regression test for the controller sweep's Paint Gap C report: a
    /// tmux capture showed the bottom screen row (the footer's 3rd row)
    /// unpainted. Checked both directly against `draw_footer` and through
    /// the real `ui::draw` path, where the footer sits at the very bottom
    /// of the terminal.
    #[test]
    fn panel_fill_reaches_the_footer_s_bottom_row() {
        let theme = Theme::for_terminal();
        let backend = TestBackend::new(120, FOOTER_HEIGHT);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal
            .draw(|f| draw_footer(f, f.area(), &theme, PaneId::Sidebar, &mut hits, None))
            .unwrap();
        let buf = terminal.backend().buffer();
        let bottom = buf.cell((3, FOOTER_HEIGHT - 1)).unwrap();
        assert_eq!(
            bottom.bg, theme.panel,
            "the footer's own bottom row must be panel-filled: {bottom:?}"
        );

        // Same assertion through the app's actual draw path, where the
        // footer is the terminal's very last row.
        let mut app = crate::app::App::new_for_test();
        let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
        terminal.draw(|f| crate::ui::draw(f, &mut app)).unwrap();
        let buf = terminal.backend().buffer();
        let bottom = buf.cell((3, 39)).unwrap();
        assert_eq!(
            bottom.bg, app.theme.panel,
            "the terminal's bottom-most row (the footer's 3rd row) must be panel-filled: {bottom:?}"
        );
    }
}

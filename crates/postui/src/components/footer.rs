use crate::action::Action;
use crate::hit::{Hit, HitMap};
use crate::layout::PaneId;
use crate::paint::{Chip, fill, text};
use crate::theme::Theme;
use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

/// The footer is always exactly this many rows tall: a blank panel row on
/// top, the content row (chips + right-aligned quit), a blank panel row on
/// the bottom — the same painted 3-row rhythm as the header app bar.
pub const FOOTER_HEIGHT: u16 = 3;

/// The context-sensitive chips for the focused pane. Each entry is `(key,
/// label, action)`; `None` actions render as plain (unregistered, muted)
/// text on the panel background rather than a filled chip — they describe
/// a binding with no single dispatchable `Action` (e.g. multi-key hints).
/// The always-present palette chip is NOT part of this list: like the quit
/// hint, it never varies with focus, so it lives right-aligned beside quit
/// (see `PALETTE_CHIP` / `draw_footer`). `pub(crate)` so `app::tests`'s
/// mouse-parity sweep (spec §5) can enumerate the same actions
/// `draw_footer` paints as chips, rather than a copy of this list.
pub(crate) fn footer_chips(focus: PaneId) -> Vec<(&'static str, &'static str, Option<Action>)> {
    let chips: Vec<(&'static str, &'static str, Option<Action>)> = match focus {
        PaneId::Sidebar => vec![
            ("enter", "open", None),
            ("n", "new", Some(Action::PromptNewRequest)),
            ("r", "rename", Some(Action::PromptRenameRequest)),
            ("d", "delete", Some(Action::ConfirmDeleteRequest)),
        ],
        PaneId::Editor => vec![
            ("^R", "send", Some(Action::Send)),
            ("^S", "save", Some(Action::SaveRequest)),
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
    chips
}

/// The command-palette chip: always present regardless of focus, so it
/// sits right-aligned next to the quit hint rather than trailing the
/// per-pane chips.
const PALETTE_CHIP: (&str, &str, Option<Action>) = ("^P", "commands", Some(Action::OpenPalette));

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

    // The palette chip sits right-aligned, one gap column left of quit.
    let (pk, pl, _) = PALETTE_CHIP;
    let palette_w = (pk.chars().count() + pl.chars().count() + 4) as u16;
    let palette_x = quit_x.saturating_sub(palette_w + 1);
    paint_chip_row(
        buf,
        mid_y,
        palette_x,
        quit_x,
        &[PALETTE_CHIP],
        theme,
        hits,
        hovered,
    );

    // Per-pane chips stop one column shy of the palette chip so the two
    // never collide.
    let right_limit = palette_x.saturating_sub(1);
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
/// `right_limit`. Each chip with `Some(action)` is a quiet chip: the key
/// combo sits in a small `Chip`-style pill tinted `theme.accent` on
/// `theme.control` (lifting to `theme.control_hover` under the mouse per
/// `hovered`), with the label following in plain `theme.text_muted` text
/// beside it — registering `Hit::FooterChip(action)` over the combined
/// span. A `None` action renders as fully plain (unregistered, muted) text
/// directly on the caller's background instead — the pill IS the
/// clickability signal, so a hint with no single dispatchable action never
/// visually promises a click it can't honor. Shared by the footer's own
/// hint row and the editor toolbar. Returns the x position just past the
/// last chip painted.
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
        // Total footprint is the same either way — 4 extra columns beyond
        // key+label — so layout math never has to branch on `action`: a
        // clickable chip is `" key "` (the tinted pill, +2) + `" label "`,
        // and a plain entry is `" key"` + `" label "` with one trailing pad
        // column making up the difference.
        let width = key.chars().count() as u16 + label.chars().count() as u16 + 4;
        if x + width > right_limit {
            break;
        }
        let chip_area = Rect {
            x,
            y,
            width,
            height: 1,
        };
        match action {
            Some(a) => {
                let on = if hovered == Some(&Hit::FooterChip(a.clone())) {
                    theme.control_hover
                } else {
                    theme.control
                };
                let pill_w = Chip {
                    label: key,
                    color: theme.accent,
                }
                .paint(buf, x, y, on, theme);
                let label_text = format!(" {label} ");
                text(
                    buf,
                    x + pill_w,
                    y,
                    &label_text,
                    theme.text_muted,
                    theme.panel,
                    false,
                );
                hits.register(chip_area, Hit::FooterChip(a.clone()));
            }
            None => {
                let key_text = format!(" {key}");
                let label_text = format!(" {label}  ");
                text(buf, x, y, &key_text, theme.accent, theme.panel, true);
                text(
                    buf,
                    x + key_text.chars().count() as u16,
                    y,
                    &label_text,
                    theme.text_muted,
                    theme.panel,
                    false,
                );
            }
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
        assert!(content.contains("^R  send"));
    }

    #[test]
    fn response_focus_shows_response_hints() {
        let content = render(PaneId::Response);
        assert!(content.contains("r  raw"));
        assert!(content.contains("/  search"));
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
        // The palette chip is right-aligned: after the last per-pane chip,
        // one gap column before the quit hint.
        let palette = hits
            .rect_of(&Hit::FooterChip(Action::OpenPalette))
            .expect("palette chip registered");
        let quit = hits.rect_of(&Hit::FooterChip(Action::Quit)).unwrap();
        let delete = hits
            .rect_of(&Hit::FooterChip(Action::ConfirmDeleteRequest))
            .unwrap();
        assert_eq!(palette.x + palette.width + 1, quit.x);
        assert!(
            delete.x + delete.width < palette.x,
            "per-pane chips stay left of the palette chip"
        );
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
    fn chip_paints_accent_bold_key_in_a_tinted_pill_and_muted_label_beside_it() {
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
        // First cell of the chip is the pill's leading space, then the key
        // glyph, bold, on the accent-tinted pill fill — not a flat
        // `theme.control` fill. The glyph's own color is a contrast pick
        // against that fill (checkpoint-2: `theme.accent` is itself light
        // enough that the tinted fill reads light too, so painting the
        // key in `theme.accent` unconditionally would be light-on-light).
        let key_cell = buf.cell((rect.x + 1, rect.y)).unwrap();
        assert_eq!(key_cell.symbol(), "n");
        let fill = theme.tint(theme.accent, theme.control);
        assert_eq!(key_cell.bg, fill);
        if crate::theme::is_light(fill) {
            assert!(
                !crate::theme::is_light(key_cell.fg),
                "light fill needs dark key text: {key_cell:?}"
            );
        } else {
            assert_eq!(key_cell.fg, theme.accent);
        }
        assert!(key_cell.modifier.contains(ratatui::style::Modifier::BOLD));
        // The label follows one gap column after the pill (" n " is 3
        // cells, then the label's own leading space), muted, not bold, and
        // sitting on the plain panel — no chip fill of its own.
        let label_cell = buf.cell((rect.x + 4, rect.y)).unwrap();
        assert_eq!(label_cell.symbol(), "n"); // first letter of "new"
        assert_eq!(label_cell.fg, theme.text_muted);
        assert_eq!(label_cell.bg, theme.panel);
        assert!(!label_cell.modifier.contains(ratatui::style::Modifier::BOLD));
    }

    /// Controller ruling: the chip pill IS the clickability signal.
    /// Non-clickable (`None`-action) entries must render as plain text
    /// directly on the panel — no fill, no hover — while clickable entries
    /// keep their tinted key pill.
    #[test]
    fn only_clickable_entries_get_the_tinted_key_pill() {
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

        // "n new" (PromptNewRequest) IS clickable: tinted key pill.
        let rect = hits
            .rect_of(&Hit::FooterChip(Action::PromptNewRequest))
            .expect("new-request chip hit registered");
        let chip_cell = buf.cell((rect.x + 1, rect.y)).unwrap();
        assert_eq!(
            chip_cell.bg,
            theme.tint(theme.accent, theme.control),
            "clickable entry keeps its tinted key pill"
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

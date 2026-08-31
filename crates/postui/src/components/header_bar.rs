use crate::hit::{Hit, HitMap};
use crate::paint::{fill, text};
use crate::theme::Theme;
use ratatui::Frame;
use ratatui::layout::Rect;

/// The app bar is always exactly this many rows tall: a blank panel row on
/// top, the content row (wordmark + chips), a blank panel row on the
/// bottom — matching the painted 3-row rhythm of buttons/fields elsewhere.
pub const HEADER_HEIGHT: u16 = 3;

/// Paints the app bar: a flat `theme.panel` fill across all 3 rows, the bold
/// `postui` wordmark, and the project/env selectors as single-row
/// `theme.control`-filled chips (lifting to `theme.control_hover` while
/// hovered) with a trailing `▾` marker. Registers the same
/// [`Hit::HeaderProject`]/[`Hit::HeaderEnv`] hits on the chip rects.
#[allow(clippy::too_many_arguments)]
pub fn draw_header(
    frame: &mut Frame,
    area: Rect,
    theme: &Theme,
    project: &str,
    env: &str,
    vars_active: bool,
    // Shows the save/discard group beside the Theme chip. Only ever true
    // while the open request has unsaved edits (a clean request needs
    // neither button) on the Main screen with no modal capturing keys.
    dirty: bool,
    hits: &mut HitMap,
    hovered: Option<&Hit>,
) {
    let buf = frame.buffer_mut();
    fill(buf, area, theme.panel);

    if area.height == 0 {
        return;
    }
    let mid_y = area.y + area.height / 2;

    let wordmark = "postui";
    let wordmark_x = area.x + 3;
    text(
        buf,
        wordmark_x,
        mid_y,
        wordmark,
        theme.text,
        theme.panel,
        true,
    );

    let mut x = wordmark_x + wordmark.chars().count() as u16 + 3;

    let project_label = format!(" {project} \u{25be} ");
    let project_w = project_label.chars().count() as u16;
    let project_rect = Rect {
        x,
        y: mid_y,
        width: project_w,
        height: 1,
    };
    let project_bg = if hovered == Some(&Hit::HeaderProject) {
        theme.control_hover
    } else {
        theme.control
    };
    fill(buf, project_rect, project_bg);
    text(
        buf,
        project_rect.x,
        mid_y,
        &project_label,
        theme.text,
        project_bg,
        false,
    );
    hits.register(project_rect, Hit::HeaderProject);
    x += project_w + 1;

    // This chip is the app's one environment control, and the environment
    // shapes what every screen shows (resolved {{vars}}, "Value in <env>",
    // selector grids) — so it announces itself: full "Environment:" label,
    // bright bold text, unlike the quiet muted project chip.
    let env_label = format!(" Environment: {env} \u{25be} ");
    let env_w = env_label.chars().count() as u16;
    let env_rect = Rect {
        x,
        y: mid_y,
        width: env_w,
        height: 1,
    };
    let env_bg = if hovered == Some(&Hit::HeaderEnv) {
        theme.control_hover
    } else {
        theme.control
    };
    fill(buf, env_rect, env_bg);
    text(buf, env_rect.x, mid_y, &env_label, theme.text, env_bg, true);
    hits.register(env_rect, Hit::HeaderEnv);
    x += env_w + 1;

    // The env chip opens the chooser; this keycap pill beside it is the
    // cycle affordance — the footer chips' keycap styling (muted tint over
    // the control fill, lifting on hover), one gap column off the chip so
    // it reads as its own button rather than the chip's opener key.
    let cycle_on = if hovered == Some(&Hit::HeaderEnvCycle) {
        theme.control_hover
    } else {
        theme.control
    };
    let cycle_w = crate::paint::Chip {
        label: "alt+c",
        color: theme.text_muted,
    }
    .paint(buf, x, mid_y, cycle_on, theme);
    hits.register(
        Rect {
            x,
            y: mid_y,
            width: cycle_w,
            height: 1,
        },
        Hit::HeaderEnvCycle,
    );
    // A wide group gap (the footer's own save-group width): the Variable
    // Manager chip follows in the left cluster, and the env-cycle pill
    // must keep reading as the env chip's — not the manager's — shortcut.
    x += cycle_w + 8;

    // The Variable Manager toggle, in the footer's clickable idiom with
    // the keycap trailing the name: prominent full name + `alt+v` pill.
    // While the manager screen is open the whole chip holds the pressed
    // fill, keeping the old `vars` toggle's stateful read. Paints
    // unconditionally like the rest of the left cluster (a too-narrow bar
    // clips it rather than dropping it).
    let vm_label = " Variable Manager ";
    let vm_label_w = vm_label.chars().count() as u16;
    let (vm_pill_on, vm_label_bg) = if vars_active {
        (theme.control_pressed, theme.control_pressed)
    } else if hovered == Some(&Hit::HeaderVars) {
        (theme.control_hover, theme.panel)
    } else {
        (theme.control, theme.panel)
    };
    text(buf, x, mid_y, vm_label, theme.text, vm_label_bg, false);
    let vm_pill_w = crate::paint::Chip {
        label: "alt+v",
        color: theme.text_muted,
    }
    .paint(buf, x + vm_label_w, mid_y, vm_pill_on, theme);
    let vm_rect = Rect {
        x,
        y: mid_y,
        width: vm_label_w + vm_pill_w,
        height: 1,
    };
    hits.register(vm_rect, Hit::HeaderVars);
    x += vm_rect.width + 1;

    // The theme-picker chip sits alone at the bar's right edge, mirroring
    // the wordmark's 3-column margin — same name-plus-trailing-keycap
    // idiom as the Variable Manager chip.
    let theme_label = " Theme ";
    let theme_key_w = " alt+b ".chars().count() as u16;
    let theme_w = theme_label.chars().count() as u16 + theme_key_w;
    let theme_x = (area.x + area.width).saturating_sub(theme_w + 3);

    // The save/discard group, in the bar's same name-plus-trailing-keycap
    // idiom, right-aligned a group gap left of the Theme chip — up here
    // near the data being saved rather than down in the footer. Present
    // only while there is actually something to save: both chips appear
    // together when the request goes dirty and leave when it's clean
    // again, so an idle bar carries no dead buttons. Registered as
    // `Hit::FooterChip` so clicks dispatch through the existing routing.
    //
    // On a bar too narrow to hold both, the group outranks the Theme
    // chip (saving beats restyling): it takes the right margin and the
    // Theme chip sits out until the request is clean again. Tighter
    // still, discard drops before save — the group's essential half
    // survives longest.
    const GROUP_GAP: u16 = 8;
    let save_label = " Save ";
    let discard_label = " Discard ";
    let save_w = save_label.chars().count() as u16 + " ^S ".chars().count() as u16;
    let discard_w = discard_label.chars().count() as u16 + " \u{21a9} ".chars().count() as u16;
    let group_w = discard_w + 2 + save_w;
    let beside_theme = theme_x > x + group_w + GROUP_GAP;
    let theme_visible = if dirty {
        theme_x > x && beside_theme
    } else {
        theme_x > x
    };
    if dirty {
        use crate::action::Action;
        let save_hit = Hit::FooterChip(Action::SaveRequest);
        let discard_hit = Hit::FooterChip(Action::ConfirmDiscardChanges);
        let group_right = if beside_theme {
            theme_x.saturating_sub(GROUP_GAP)
        } else {
            (area.x + area.width).saturating_sub(3)
        };
        let save_x = group_right.saturating_sub(save_w);
        // Discard sits left of save so save keeps its anchored spot.
        let discard_x = save_x.saturating_sub(discard_w + 2);
        if save_x > x {
            let pill_on = if hovered == Some(&save_hit) {
                theme.control_hover
            } else {
                theme.control
            };
            text(buf, save_x, mid_y, save_label, theme.text, theme.panel, false);
            crate::paint::Chip {
                label: "^S",
                color: theme.text_muted,
            }
            .paint(
                buf,
                save_x + save_label.chars().count() as u16,
                mid_y,
                pill_on,
                theme,
            );
            hits.register(
                Rect {
                    x: save_x,
                    y: mid_y,
                    width: save_w,
                    height: 1,
                },
                save_hit,
            );
            if discard_x > x {
                let pill_on = if hovered == Some(&discard_hit) {
                    theme.control_hover
                } else {
                    theme.control
                };
                text(
                    buf,
                    discard_x,
                    mid_y,
                    discard_label,
                    theme.text,
                    theme.panel,
                    false,
                );
                crate::paint::Chip {
                    label: "\u{21a9}",
                    color: theme.text_muted,
                }
                .paint(
                    buf,
                    discard_x + discard_label.chars().count() as u16,
                    mid_y,
                    pill_on,
                    theme,
                );
                hits.register(
                    Rect {
                        x: discard_x,
                        y: mid_y,
                        width: discard_w,
                        height: 1,
                    },
                    discard_hit,
                );
            }
        }
    }

    // Never collide with the left-side chips on a very narrow bar (and
    // yield to the save group while it needs the right margin).
    if theme_visible {
        let theme_rect = Rect {
            x: theme_x,
            y: mid_y,
            width: theme_w,
            height: 1,
        };
        let pill_on = if hovered == Some(&Hit::HeaderTheme) {
            theme.control_hover
        } else {
            theme.control
        };
        text(
            buf,
            theme_x,
            mid_y,
            theme_label,
            theme.text,
            theme.panel,
            false,
        );
        crate::paint::Chip {
            label: "alt+b",
            color: theme.text_muted,
        }
        .paint(
            buf,
            theme_x + theme_label.chars().count() as u16,
            mid_y,
            pill_on,
            theme,
        );
        hits.register(theme_rect, Hit::HeaderTheme);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Theme;
    use ratatui::backend::TestBackend;
    use ratatui::style::Modifier;
    use ratatui::{Frame, Terminal};

    fn render(
        theme: &Theme,
        project: &str,
        env: &str,
        hovered: Option<&Hit>,
    ) -> (Terminal<TestBackend>, HitMap) {
        render_wide(theme, project, env, false, hovered, 60)
    }

    fn render_wide(
        theme: &Theme,
        project: &str,
        env: &str,
        vars_active: bool,
        hovered: Option<&Hit>,
        width: u16,
    ) -> (Terminal<TestBackend>, HitMap) {
        let backend = TestBackend::new(width, HEADER_HEIGHT);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = HitMap::default();
        terminal
            .draw(|f: &mut Frame| {
                draw_header(
                    f,
                    f.area(),
                    theme,
                    project,
                    env,
                    vars_active,
                    false,
                    &mut hits,
                    hovered,
                )
            })
            .unwrap();
        (terminal, hits)
    }

    fn render_dirty(theme: &Theme, width: u16) -> (Terminal<TestBackend>, HitMap) {
        let backend = TestBackend::new(width, HEADER_HEIGHT);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = HitMap::default();
        terminal
            .draw(|f: &mut Frame| {
                draw_header(f, f.area(), theme, "alpha", "qa", false, true, &mut hits, None)
            })
            .unwrap();
        (terminal, hits)
    }

    fn row_text(term: &Terminal<TestBackend>, rect: &Rect) -> String {
        (rect.x..rect.x + rect.width)
            .map(|x| cell(term, x, rect.y).symbol().to_string())
            .collect()
    }

    fn cell(term: &Terminal<TestBackend>, x: u16, y: u16) -> ratatui::buffer::Cell {
        term.backend().buffer().cell((x, y)).unwrap().clone()
    }

    /// A bar too narrow for the right-aligned chip drops it rather than
    /// painting over the left-side chips.
    #[test]
    fn theme_chip_disappears_on_a_too_narrow_bar() {
        let theme = Theme::dark();
        let backend = TestBackend::new(38, HEADER_HEIGHT);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = HitMap::default();
        terminal
            .draw(|f: &mut Frame| {
                draw_header(
                    f,
                    f.area(),
                    &theme,
                    "a-rather-long-project",
                    "qa",
                    false,
                    false,
                    &mut hits,
                    None,
                )
            })
            .unwrap();
        assert!(hits.rect_of(&Hit::HeaderTheme).is_none());
    }

    #[test]
    fn wordmark_is_bold_on_panel_background() {
        let theme = Theme::dark();
        let (term, _hits) = render(&theme, "alpha", "qa", None);
        let c = cell(&term, 3, 1);
        assert_eq!(c.symbol(), "p");
        assert_eq!(c.fg, theme.text);
        assert_eq!(c.bg, theme.panel);
        assert!(c.modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn top_and_bottom_rows_are_flat_panel_fill() {
        let theme = Theme::dark();
        let (term, _hits) = render(&theme, "alpha", "qa", None);
        for y in [0, 2] {
            let c = cell(&term, 3, y);
            assert_eq!(c.symbol(), " ");
            assert_eq!(c.bg, theme.panel);
        }
    }

    #[test]
    fn project_chip_fills_control_and_registers_hit() {
        let theme = Theme::dark();
        let (term, hits) = render(&theme, "alpha", "qa", None);
        let rect = hits
            .rect_of(&Hit::HeaderProject)
            .expect("project hit registered");
        let c = cell(&term, rect.x + 1, rect.y);
        assert_eq!(c.bg, theme.control);
        assert_eq!(c.fg, theme.text);
        assert_eq!(c.symbol(), "a"); // first char of "alpha"
    }

    #[test]
    fn env_chip_announces_itself_with_the_full_label() {
        let theme = Theme::dark();
        let (term, hits) = render(&theme, "alpha", "qa", None);
        let rect = hits.rect_of(&Hit::HeaderEnv).expect("env hit registered");
        // Full label on every screen — the chip is the app's only env
        // control.
        let label: String = (rect.x..rect.x + rect.width)
            .map(|x| cell(&term, x, rect.y).symbol().to_string())
            .collect();
        assert_eq!(label, " Environment: qa \u{25be} ");
        let c = cell(&term, rect.x + 1, rect.y);
        assert_eq!(c.bg, theme.control);
        assert_eq!(c.fg, theme.text, "bright, not muted");
        assert!(c.modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn hovered_chip_lifts_background_to_control_hover_and_leaves_the_other_alone() {
        let theme = Theme::dark();
        let (term, hits) = render(&theme, "alpha", "qa", Some(&Hit::HeaderProject));
        let project_rect = hits.rect_of(&Hit::HeaderProject).unwrap();
        let env_rect = hits.rect_of(&Hit::HeaderEnv).unwrap();
        assert_eq!(
            cell(&term, project_rect.x, project_rect.y).bg,
            theme.control_hover
        );
        assert_eq!(cell(&term, env_rect.x, env_rect.y).bg, theme.control);
    }

    #[test]
    fn no_reversed_video_cell_anywhere_in_the_bar() {
        let theme = Theme::dark();
        let (term, _hits) = render(&theme, "alpha", "qa", None);
        let buf = term.backend().buffer();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                let c = buf.cell((x, y)).unwrap();
                assert_ne!(c.bg, theme.accent, "no accent-filled cell at ({x},{y})");
            }
        }
    }

    /// The env chip opens the chooser; the keycap pill beside it is the
    /// cycle affordance — footer-chip keycap styling (muted tint over the
    /// control fill), one gap column off the chip so it reads as its own
    /// button, lifting on hover like any clickable pill.
    #[test]
    fn alt_c_keycap_pill_sits_one_column_off_the_env_chip() {
        let theme = Theme::dark();
        let (term, hits) = render(&theme, "alpha", "qa", None);
        let env_rect = hits.rect_of(&Hit::HeaderEnv).unwrap();
        let rect = hits
            .rect_of(&Hit::HeaderEnvCycle)
            .expect("env-cycle pill registered");
        assert_eq!(rect.x, env_rect.x + env_rect.width + 1);
        assert_eq!(
            row_text(&term, &rect),
            format!(" {}+c ", crate::keys::alt_label())
        );
        assert_eq!(
            cell(&term, rect.x + 1, rect.y).bg,
            theme.tint(theme.text_muted, theme.control),
            "keycap pill tint matches the footer chips'"
        );

        let (term, hits) = render(&theme, "alpha", "qa", Some(&Hit::HeaderEnvCycle));
        let rect = hits.rect_of(&Hit::HeaderEnvCycle).unwrap();
        assert_eq!(
            cell(&term, rect.x + 1, rect.y).bg,
            theme.tint(theme.text_muted, theme.control_hover),
            "hover lifts the pill fill"
        );
    }

    /// The Variable Manager chip sits in the left cluster — a wide group
    /// gap after the env-cycle pill, so that pill still clearly belongs
    /// to the env chip — in the footer's clickable idiom with the keycap
    /// trailing the name: prominent full name + `alt+v` pill.
    #[test]
    fn variable_manager_chip_follows_the_env_cluster_with_a_trailing_keycap() {
        let theme = Theme::dark();
        let (term, hits) = render_wide(&theme, "alpha", "qa", false, None, 100);
        let rect = hits
            .rect_of(&Hit::HeaderVars)
            .expect("variable manager chip registered");
        let cycle_rect = hits.rect_of(&Hit::HeaderEnvCycle).unwrap();
        assert_eq!(
            rect.x,
            cycle_rect.x + cycle_rect.width + 8,
            "a group gap after the env-cycle pill"
        );
        assert_eq!(
            row_text(&term, &rect),
            format!(" Variable Manager  {}+v ", crate::keys::alt_label())
        );
        let label_cell = cell(&term, rect.x + 1, rect.y);
        assert_eq!(label_cell.symbol(), "V");
        assert_eq!(label_cell.fg, theme.text, "prominent label, not muted");
        assert_eq!(label_cell.bg, theme.panel);
        assert_eq!(
            cell(&term, rect.x + 19, rect.y).bg,
            theme.tint(theme.text_muted, theme.control),
            "trailing keycap pill tint matches the footer chips'"
        );
    }

    /// While the manager screen is open the whole chip holds the pressed
    /// fill, same as the old `vars` toggle did.
    #[test]
    fn variable_manager_chip_holds_the_pressed_fill_while_active() {
        let theme = Theme::dark();
        let (term, hits) = render_wide(&theme, "alpha", "qa", true, None, 100);
        let rect = hits.rect_of(&Hit::HeaderVars).unwrap();
        assert_eq!(
            cell(&term, rect.x + 1, rect.y).bg,
            theme.control_pressed,
            "label ground shows the pressed state"
        );
        assert_eq!(
            cell(&term, rect.x + 19, rect.y).bg,
            theme.tint(theme.text_muted, theme.control_pressed),
            "keycap tint derives from the pressed fill"
        );
    }

    /// The Theme chip gets the same treatment: prominent name + trailing
    /// `alt+b` keycap pill, right-aligned at the wordmark's 3-column
    /// margin, the pill lifting on hover.
    #[test]
    fn theme_chip_shows_its_name_and_trailing_keycap() {
        let theme = Theme::dark();
        let (term, hits) = render_wide(&theme, "alpha", "qa", false, None, 110);
        let rect = hits.rect_of(&Hit::HeaderTheme).unwrap();
        assert_eq!(rect.x + rect.width, 110 - 3, "right-aligned");
        assert_eq!(
            row_text(&term, &rect),
            format!(" Theme  {}+b ", crate::keys::alt_label())
        );
        let label_cell = cell(&term, rect.x + 1, rect.y);
        assert_eq!(label_cell.symbol(), "T");
        assert_eq!(label_cell.fg, theme.text, "prominent label, not muted");
        assert_eq!(label_cell.bg, theme.panel);
        assert_eq!(
            cell(&term, rect.x + 8, rect.y).bg,
            theme.tint(theme.text_muted, theme.control),
            "trailing keycap pill tint matches the footer chips'"
        );

        let (term, hits) = render_wide(&theme, "alpha", "qa", false, Some(&Hit::HeaderTheme), 110);
        let rect = hits.rect_of(&Hit::HeaderTheme).unwrap();
        assert_eq!(
            cell(&term, rect.x + 8, rect.y).bg,
            theme.tint(theme.text_muted, theme.control_hover),
            "hover lifts the keycap pill fill"
        );
    }

    /// The save/discard group appears beside the Theme chip only while
    /// the request is dirty — a clean request needs neither button, so
    /// the bar carries none.
    #[test]
    fn save_group_appears_beside_the_theme_chip_only_while_dirty() {
        use crate::action::Action;
        let theme = Theme::dark();
        let (_term, hits) = render_wide(&theme, "alpha", "qa", false, None, 160);
        assert!(
            hits.rect_of(&Hit::FooterChip(Action::SaveRequest)).is_none(),
            "clean: no save chip"
        );
        assert!(
            hits.rect_of(&Hit::FooterChip(Action::ConfirmDiscardChanges))
                .is_none(),
            "clean: no discard chip"
        );

        let (term, hits) = render_dirty(&theme, 160);
        let save = hits
            .rect_of(&Hit::FooterChip(Action::SaveRequest))
            .expect("dirty: save chip registered");
        let discard = hits
            .rect_of(&Hit::FooterChip(Action::ConfirmDiscardChanges))
            .expect("dirty: discard chip registered");
        let theme_rect = hits.rect_of(&Hit::HeaderTheme).unwrap();
        assert!(
            discard.x + discard.width < save.x,
            "discard sits left of save"
        );
        assert!(
            save.x + save.width + 8 <= theme_rect.x,
            "the group sits a clear gap left of the Theme chip: save {save:?} theme {theme_rect:?}"
        );
        assert_eq!(
            row_text(&term, &save),
            " Save  ^S ",
            "name + trailing keycap idiom"
        );
        assert_eq!(row_text(&term, &discard), " Discard  \u{21a9} ");
    }

    /// On a bar too narrow to hold both, the save group outranks the
    /// Theme chip: it takes the right margin and Theme sits out until the
    /// request is clean again.
    #[test]
    fn save_group_takes_the_theme_chips_place_on_a_narrow_bar() {
        use crate::action::Action;
        let theme = Theme::dark();
        let (_term, hits) = render_dirty(&theme, 120);
        assert!(
            hits.rect_of(&Hit::FooterChip(Action::SaveRequest)).is_some(),
            "save survives the squeeze"
        );
        assert!(
            hits.rect_of(&Hit::HeaderTheme).is_none(),
            "theme yields to the save group"
        );
    }

    /// The Variable Manager chip is part of the left cluster now: it
    /// paints (clipped, like the project/env chips) on a bar too narrow
    /// for the right-aligned theme chip, which still drops.
    #[test]
    fn variable_manager_chip_stays_when_the_theme_chip_drops() {
        let theme = Theme::dark();
        let (_term, hits) =
            render_wide(&theme, "a-rather-long-project", "staging", false, None, 60);
        assert!(hits.rect_of(&Hit::HeaderVars).is_some());
        assert!(hits.rect_of(&Hit::HeaderTheme).is_none());
    }

    /// Regression test for the controller sweep's Paint Gap A report (a
    /// tmux capture showing the panel fill stopping around column 34, past
    /// the env chip). Checked both directly against `draw_header` at a wide
    /// (200-col) width and through the real `ui::draw` path the app itself
    /// uses, since `draw_header`'s own `fill(buf, area, theme.panel)` call
    /// paints `area` in full before anything else is drawn.
    #[test]
    fn panel_fill_reaches_the_full_area_width_past_the_chips() {
        let theme = Theme::dark();
        let backend = TestBackend::new(200, HEADER_HEIGHT);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = HitMap::default();
        terminal
            .draw(|f: &mut Frame| {
                draw_header(f, f.area(), &theme, "alpha", "qa", false, false, &mut hits, None)
            })
            .unwrap();
        let buf = terminal.backend().buffer();
        let far_right = buf.cell((198, 1)).unwrap();
        assert_eq!(
            far_right.bg, theme.panel,
            "the panel fill must reach the far-right column: {far_right:?}"
        );

        // Same assertion through the app's actual draw path.
        let mut app = crate::app::App::new_for_test();
        let mut terminal = Terminal::new(TestBackend::new(200, 40)).unwrap();
        terminal.draw(|f| crate::ui::draw(f, &mut app)).unwrap();
        let buf = terminal.backend().buffer();
        let far_right = buf.cell((197, 1)).unwrap();
        assert_eq!(
            far_right.bg, app.theme.panel,
            "app bar panel fill must reach the far-right column in the live draw path: {far_right:?}"
        );
    }
}

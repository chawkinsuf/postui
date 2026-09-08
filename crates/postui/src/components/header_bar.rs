use crate::hit::{Hit, HitMap};
use crate::paint::{fill, text};
use crate::theme::Theme;
use ratatui::Frame;
use ratatui::layout::Rect;

/// The app bar is always exactly this many rows tall: a blank panel row on
/// top, the content row (wordmark + chips), a blank panel row on the
/// bottom — matching the painted 3-row rhythm of buttons/fields elsewhere.
pub const HEADER_HEIGHT: u16 = 3;

/// The gap after a chip that another chip's keycap pill follows (project
/// → env pill, env → space pill, Manage → Theme pill): wide enough
/// against the one-column pill-to-chip gap that each pill reads as the
/// shortcut of the chip it leads rather than of the chip it follows, and
/// no wider — three of them have to fit a 120-column bar beside a
/// ten-character project name. The left cluster's collapse to the
/// ordinary one-column chip gap when the pills yield on a narrow bar.
const CHIP_GROUP_GAP: u16 = 4;

/// The gap between the save/discard group and the Manage chip: wider
/// than a chip group gap, so the dirty-request pair reads as its own
/// group rather than as more of the app menu.
const SAVE_GROUP_GAP: u16 = 8;

const SAVE_LABEL: &str = " Save ";
const DISCARD_LABEL: &str = " Discard ";
/// The dirty bar's save/discard group: ` alt+d  Discard ` + 2 + ` ^S  Save `.
const SAVE_GROUP_W: u16 =
    (DISCARD_LABEL.len() + " alt+d ".len() + 2 + SAVE_LABEL.len() + " ^S ".len()) as u16;

const RELOAD_LABEL: &str = "Reload";
/// The Manage screen's Reload chip: ` alt+r Reload`.
const RELOAD_W: u16 = (RELOAD_LABEL.len() + " alt+r ".len()) as u16;

/// Paints the app bar: a flat `theme.panel` fill across all 3 rows and the
/// project/env/space selectors as single-row `theme.control`-filled chips
/// (lifting to `theme.control_hover` while hovered), the env and space
/// chips with a trailing `▾` marker for the dropdown they anchor.
/// Registers the [`Hit::HeaderProject`]/[`Hit::HeaderEnv`]/
/// [`Hit::HeaderSpace`] hits on the chip rects (in that on-screen order),
/// plus the `alt+z`/`alt+x`/`alt+c` cycle pills leading them
/// ([`Hit::HeaderProjectCycle`]/[`Hit::HeaderEnvCycle`]/
/// [`Hit::HeaderSpaceCycle`]).
///
/// The bar has two clusters. The left one answers "where am I": the
/// three selectors, each cycling in place. The right one, anchored at the
/// bar's 3-column right margin, is the app menu — `Manage` then `Theme`,
/// both of which leave the current screen for another — with the
/// save/discard group a wider gap left of them while the open request is
/// dirty. Manage sits with Theme rather than after the selectors because
/// it is the same kind of button: a door, not a dial.
///
/// Every keycap on the bar sits *left* of the name it belongs to. The
/// selector labels change width as they cycle (a longer environment
/// name, a shorter project name), and a pill trailing its chip would
/// slide with every cycle — the very button the pointer is parked on to
/// keep clicking. Leading, each pill holds still while its own chip
/// grows or shrinks to its right; only the chips further along move.
/// Manage, Theme, Save and Discard follow the same pill-then-name order
/// so the bar reads as one idiom.
///
/// Narrow-bar rule: the chip *labels* never yield — their keycaps do, in
/// order. The left cluster is measured through the space chip; the right
/// cluster's essentials are the Manage chip and, while dirty, the save
/// group. As the bar narrows: the Theme chip drops first (it is the only
/// chip with nothing to do with the request); then the three cycle pills,
/// and the group gaps after the selector chips collapse to the ordinary
/// one-column gap; then the Manage chip's own `alt+v` keycap, leaving a
/// bare ` Manage `; then Discard, then Save. The keys themselves keep
/// working in every case, and their hints stay in the footer/palette. If
/// even the bare Manage chip can't fit right-anchored beside the
/// selectors, it follows them and clips at the bar's edge as before.
#[allow(clippy::too_many_arguments)]
pub fn draw_header(
    frame: &mut Frame,
    area: Rect,
    theme: &Theme,
    project: &str,
    space: &str,
    env: &str,
    manage_active: bool,
    // Shows the save/discard group beside the Manage chip. Only ever true
    // while the open request has unsaved edits (a clean request needs
    // neither button) on the Main screen with no modal capturing keys.
    dirty: bool,
    // Shows the Reload chip in the same slot the save/discard group uses.
    // Only ever true on the Manage screen, and `dirty` requires
    // `Screen::Main`, so the two can never both be true.
    show_reload: bool,
    hits: &mut HitMap,
    hovered: Option<&Hit>,
) {
    let buf = frame.buffer_mut();
    fill(buf, area, theme.panel);

    if area.height == 0 {
        return;
    }
    let mid_y = area.y + area.height / 2;

    // The bar opens at a 3-column margin (mirrored by the right cluster
    // at the right edge). No wordmark: the terminal title carries the app
    // name, and the project chip is the bar's first word.
    let mut x = area.x + 3;

    // The three selectors share one idiom — a keycap pill that cycles,
    // then a labelled, bold chip that opens its picker — because each
    // shapes what every screen shows (the project, its resolved {{vars}},
    // the visible request set). Only the env and space chips carry the
    // `▾`: they open a dropdown anchored under the chip, while the
    // project chip opens the centred project chooser.
    let project_label = format!(" Project: {project} ");
    let space_label = format!(" Space: {space} \u{25be} ");
    let env_label = format!(" Environment: {env} \u{25be} ");
    let project_pill = crate::paint::Chip {
        label: "alt+z",
        color: theme.text_muted,
    };
    let env_pill = crate::paint::Chip {
        label: "alt+x",
        color: theme.text_muted,
    };
    let space_pill = crate::paint::Chip {
        label: "alt+c",
        color: theme.text_muted,
    };
    let chips = [
        (
            &project_label,
            Hit::HeaderProject,
            &project_pill,
            Hit::HeaderProjectCycle,
        ),
        (&env_label, Hit::HeaderEnv, &env_pill, Hit::HeaderEnvCycle),
        (
            &space_label,
            Hit::HeaderSpace,
            &space_pill,
            Hit::HeaderSpaceCycle,
        ),
    ];

    // The right cluster's pieces, measured before anything is painted:
    // the left cluster's keycaps yield to whatever the right cluster
    // must show.
    let manage_label = " Manage ";
    let manage_label_w = manage_label.chars().count() as u16;
    let manage_pill = crate::paint::Chip {
        label: "alt+v",
        color: theme.text_muted,
    };
    let theme_label = " Theme ";
    let theme_pill = crate::paint::Chip {
        label: "alt+t",
        color: theme.text_muted,
    };
    let theme_w = theme_label.chars().count() as u16 + theme_pill.width();
    // What the right cluster needs at minimum, margin included: the bare
    // Manage name, plus whichever of the save group (dirty) or the
    // Reload chip (Manage screen) occupies the slot beside it, with its
    // wider gap. `dirty` and `show_reload` are mutually exclusive by
    // construction, so at most one of these ever adds to the budget.
    let right_essential = 3
        + manage_label_w
        + if dirty {
            SAVE_GROUP_W + SAVE_GROUP_GAP
        } else if show_reload {
            RELOAD_W + SAVE_GROUP_GAP
        } else {
            0
        };

    // Measure the left cluster two ways (see the narrow-bar rule above):
    // everything, then without the three cycle pills — which also
    // collapses the wide group gaps, since with no pill left to
    // disambiguate they have nothing to separate. The labels are in both
    // measurements: they never yield. Each measurement is the column
    // just past the cluster's last chip.
    let labels_w: u16 = chips
        .iter()
        .map(|(label, _, _, _)| label.chars().count() as u16)
        .sum();
    let pills_w: u16 = chips.iter().map(|(_, _, pill, _)| pill.width() + 1).sum();
    let margin = x - area.x;
    let cluster_full = margin + labels_w + pills_w + CHIP_GROUP_GAP * (chips.len() as u16 - 1);
    let cluster_no_cycle_pills = margin + labels_w + (chips.len() as u16 - 1);
    // Both clusters fit when a column of panel separates them.
    let fits = |left_end: u16, right_w: u16| left_end + 1 + right_w <= area.width;
    let show_cycle_pills = fits(cluster_full, right_essential + manage_pill.width());
    let show_manage_pill = show_cycle_pills
        || fits(
            cluster_no_cycle_pills,
            right_essential + manage_pill.width(),
        );

    for (i, (label, hit, pill, cycle_hit)) in chips.iter().enumerate() {
        // The keycap pill leads: it is the cycle affordance — the footer
        // chips' keycap styling (muted tint over the control fill,
        // lifting on hover), one gap column off the chip so it reads as
        // its own button rather than the chip's opener key. It sits left
        // of the chip so cycling — which changes the chip's width — never
        // moves the pill out from under the pointer. The wider group gap
        // follows the chip, so the pill keeps reading as this chip's —
        // not the previous chip's — shortcut.
        if show_cycle_pills {
            let on = if hovered == Some(cycle_hit) {
                theme.control_hover
            } else {
                theme.control
            };
            let pill_w = pill.paint(buf, x, mid_y, on, theme);
            hits.register(
                Rect {
                    x,
                    y: mid_y,
                    width: pill_w,
                    height: 1,
                },
                cycle_hit.clone(),
            );
            x += pill_w + 1;
        }

        // Then the chip that opens the picker.
        let w = label.chars().count() as u16;
        let rect = Rect {
            x,
            y: mid_y,
            width: w,
            height: 1,
        };
        let bg = if hovered == Some(hit) {
            theme.control_hover
        } else {
            theme.control
        };
        fill(buf, rect, bg);
        text(buf, rect.x, mid_y, label, theme.text, bg, true);
        hits.register(rect, hit.clone());
        x += w;
        if i + 1 < chips.len() {
            x += if show_cycle_pills { CHIP_GROUP_GAP } else { 1 };
        }
    }
    // The column just past the left cluster: nothing on the right may
    // start before `left_end + 1`.
    let left_end = x;

    // The right cluster is laid out from the right margin inwards, each
    // piece taking its place only if it was budgeted for above.
    let right_edge = (area.x + area.width).saturating_sub(3);
    let manage_pill_w = if show_manage_pill {
        manage_pill.width()
    } else {
        0
    };
    let manage_w = manage_label_w + manage_pill_w;
    let right_w = if dirty {
        SAVE_GROUP_W + SAVE_GROUP_GAP + manage_w
    } else {
        manage_w
    };
    // Theme goes first: it shows only when the whole cluster, Theme
    // included, sits clear of the selectors.
    let theme_visible = fits(left_end, 3 + right_w + CHIP_GROUP_GAP + theme_w);
    let mut rx = right_edge;
    if theme_visible {
        let theme_x = rx - theme_w;
        let pill_on = if hovered == Some(&Hit::HeaderTheme) {
            theme.control_hover
        } else {
            theme.control
        };
        let key_w = theme_pill.paint(buf, theme_x, mid_y, pill_on, theme);
        text(
            buf,
            theme_x + key_w,
            mid_y,
            theme_label,
            theme.text,
            theme.panel,
            false,
        );
        hits.register(
            Rect {
                x: theme_x,
                y: mid_y,
                width: theme_w,
                height: 1,
            },
            Hit::HeaderTheme,
        );
        // A group gap before Theme, so its `alt+t` pill reads as Theme's
        // key and not as a trailing key of Manage.
        rx = theme_x - CHIP_GROUP_GAP;
    }

    // The Manage-screen toggle, in the footer's clickable idiom with the
    // keycap leading the name: `alt+v` pill + prominent full name. While
    // the Manage screen is open the whole chip holds the pressed fill,
    // keeping the old `vars` toggle's stateful read. The name paints
    // unconditionally (a bar too narrow even for it beside the selectors
    // pushes it right of them, clipping at the edge, rather than dropping
    // it: this chip is the only mouse path to the Manage screen); only
    // its keycap yields.
    let manage_x = rx.saturating_sub(manage_w).max(left_end + 1);
    let (vm_pill_on, vm_label_bg) = if manage_active {
        (theme.control_pressed, theme.control_pressed)
    } else if hovered == Some(&Hit::HeaderManage) {
        (theme.control_hover, theme.panel)
    } else {
        (theme.control, theme.panel)
    };
    if show_manage_pill {
        manage_pill.paint(buf, manage_x, mid_y, vm_pill_on, theme);
    }
    text(
        buf,
        manage_x + manage_pill_w,
        mid_y,
        manage_label,
        theme.text,
        vm_label_bg,
        false,
    );
    hits.register(
        Rect {
            x: manage_x,
            y: mid_y,
            width: manage_w,
            height: 1,
        },
        Hit::HeaderManage,
    );

    // Reload takes the save/discard slot on the Manage screen. The two
    // are mutually exclusive by construction -- save/discard requires
    // Screen::Main -- so no arbitration is needed. Same keycap-then-name
    // idiom as its neighbours, and the same anchor, so it inherits the
    // slot's narrow-bar behaviour: dropped rather than overlapping the
    // selectors.
    if show_reload {
        use crate::action::Action;
        let hit = Hit::FooterChip(Action::ReloadFromDisk);
        let w = RELOAD_W;
        let x = manage_x.saturating_sub(SAVE_GROUP_GAP).saturating_sub(w);
        if x > left_end {
            let pill_on = if hovered == Some(&hit) {
                theme.control_hover
            } else {
                theme.control
            };
            let key_w = crate::paint::Chip {
                label: "alt+r",
                color: theme.text_muted,
            }
            .paint(buf, x, mid_y, pill_on, theme);
            text(
                buf,
                x + key_w,
                mid_y,
                RELOAD_LABEL,
                theme.text,
                theme.panel,
                false,
            );
            hits.register(
                Rect {
                    x,
                    y: mid_y,
                    width: w,
                    height: 1,
                },
                hit,
            );
        }
    }

    // The save/discard group, in the bar's same keycap-then-name idiom,
    // a wide gap left of the Manage chip — up here near the data being
    // saved rather than down in the footer. Present only while there is
    // actually something to save: both chips appear together when the
    // request goes dirty and leave when it's clean again, so an idle bar
    // carries no dead buttons. Registered as `Hit::FooterChip` so clicks
    // dispatch through the existing routing.
    //
    // Tighter still, discard drops before save — the group's essential
    // half survives longest — and either drops rather than overlapping
    // the selectors.
    if dirty {
        use crate::action::Action;
        let save_hit = Hit::FooterChip(Action::SaveRequest);
        let discard_hit = Hit::FooterChip(Action::DiscardChanges);
        let save_w = SAVE_LABEL.chars().count() as u16 + " ^S ".chars().count() as u16;
        let discard_w = DISCARD_LABEL.chars().count() as u16 + " alt+d ".chars().count() as u16;
        let group_right = manage_x.saturating_sub(SAVE_GROUP_GAP);
        let save_x = group_right.saturating_sub(save_w);
        // Discard sits left of save so save keeps its anchored spot.
        let discard_x = save_x.saturating_sub(discard_w + 2);
        if save_x > left_end {
            let pill_on = if hovered == Some(&save_hit) {
                theme.control_hover
            } else {
                theme.control
            };
            let key_w = crate::paint::Chip {
                label: "^S",
                color: theme.text_muted,
            }
            .paint(buf, save_x, mid_y, pill_on, theme);
            text(
                buf,
                save_x + key_w,
                mid_y,
                SAVE_LABEL,
                theme.text,
                theme.panel,
                false,
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
            if discard_x > left_end {
                let pill_on = if hovered == Some(&discard_hit) {
                    theme.control_hover
                } else {
                    theme.control
                };
                let key_w = crate::paint::Chip {
                    label: "alt+d",
                    color: theme.text_muted,
                }
                .paint(buf, discard_x, mid_y, pill_on, theme);
                text(
                    buf,
                    discard_x + key_w,
                    mid_y,
                    DISCARD_LABEL,
                    theme.text,
                    theme.panel,
                    false,
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
        // Wide enough for the whole left cluster (the three chips, each
        // with its cycle pill and group gap, then Manage); the
        // right-aligned Theme chip needs more room still, so its tests
        // pass their own width to `render_wide`.
        render_wide(theme, project, env, false, hovered, 130)
    }

    fn render_wide(
        theme: &Theme,
        project: &str,
        env: &str,
        manage_active: bool,
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
                    "main",
                    env,
                    manage_active,
                    false,
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
                draw_header(
                    f,
                    f.area(),
                    theme,
                    "alpha",
                    "main",
                    "qa",
                    false,
                    true,
                    false,
                    &mut hits,
                    None,
                )
            })
            .unwrap();
        (terminal, hits)
    }

    /// Mirrors `render_dirty`, but for the Manage-screen slot: sets
    /// `manage_active` and `show_reload` instead of `dirty`, since the two
    /// groups that live in this slot are mutually exclusive.
    fn render_manage(
        theme: &Theme,
        manage_active: bool,
        show_reload: bool,
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
                    "alpha",
                    "main",
                    "qa",
                    manage_active,
                    false,
                    show_reload,
                    &mut hits,
                    None,
                )
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

    /// Reload takes the save/discard slot, which is free on this screen:
    /// that group requires Screen::Main, so the two can never collide.
    #[test]
    fn reload_occupies_the_save_slot_on_the_manage_screen() {
        let theme = Theme::dark();
        let (term, hits) = render_manage(&theme, true, true, 150);
        let content = format!("{:?}", term.backend().buffer());
        assert!(content.contains("Reload"), "{content}");
        let hit = hits
            .rect_of(&Hit::FooterChip(crate::action::Action::ReloadFromDisk))
            .expect("the chip routes through footer-chip dispatch");
        let manage = hits.rect_of(&Hit::HeaderManage).unwrap();
        assert!(
            hit.x + hit.width <= manage.x,
            "it sits left of the Manage chip"
        );
    }

    #[test]
    fn reload_is_absent_on_the_main_screen_and_never_shares_with_save() {
        let theme = Theme::dark();
        let (term, hits) = render_dirty(&theme, 180);
        let content = format!("{:?}", term.backend().buffer());
        assert!(!content.contains("Reload"), "{content}");
        assert!(
            content.contains("Save"),
            "save still shows when dirty: {content}"
        );
        assert!(
            hits.rect_of(&Hit::FooterChip(crate::action::Action::ReloadFromDisk))
                .is_none()
        );
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
                    "main",
                    "qa",
                    false,
                    false,
                    false,
                    &mut hits,
                    None,
                )
            })
            .unwrap();
        assert!(hits.rect_of(&Hit::HeaderTheme).is_none());
    }

    /// No wordmark: the project chip is the bar's first word (after its
    /// own cycle pill at the 3-column margin), and it reads like the
    /// other two selectors — labelled, bold — but without the `▾`, since
    /// it opens the centred chooser rather than an anchored dropdown.
    #[test]
    fn project_chip_opens_the_bar_labelled_and_without_a_dropdown_marker() {
        let theme = Theme::dark();
        let (term, hits) = render(&theme, "alpha", "qa", None);
        let rect = hits.rect_of(&Hit::HeaderProject).unwrap();
        let pill = hits.rect_of(&Hit::HeaderProjectCycle).unwrap();
        assert_eq!(pill.x, 3, "the project pill is the first thing on the bar");
        assert_eq!(rect.x, pill.x + pill.width + 1, "its chip follows");
        assert_eq!(row_text(&term, &rect), " Project: alpha ");
        let c = cell(&term, rect.x + 1, rect.y);
        assert_eq!(c.fg, theme.text);
        assert!(c.modifier.contains(Modifier::BOLD));
        let row = row_text(&term, &Rect::new(0, 1, 130, 1));
        assert!(!row.contains("postui"), "{row:?}");
    }

    /// The project chip gets the same cycle pill as the other two, one
    /// column ahead of the chip, reading `alt+z` — the bottom row, in
    /// on-screen order with the env (`alt+x`) and space (`alt+c`) pills.
    #[test]
    fn project_cycle_pill_leads_the_project_chip() {
        let theme = Theme::dark();
        let (term, hits) = render(&theme, "alpha", "qa", None);
        let chip = hits.rect_of(&Hit::HeaderProject).unwrap();
        let pill = hits
            .rect_of(&Hit::HeaderProjectCycle)
            .expect("project-cycle pill registered");
        assert_eq!(chip.x, pill.x + pill.width + 1);
        assert_eq!(
            row_text(&term, &pill),
            format!(" {}+z ", crate::keys::alt_label())
        );
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
        assert_eq!(c.symbol(), "P"); // "Project: alpha"
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

    /// The left cluster reads (pill +) project, (pill +) env, (pill +)
    /// space — each chip's pill one column ahead of it and the same wide
    /// group gap between chips, so no keycap pill reads as the previous
    /// chip's key. Manage is not part of it: it sits in the right cluster
    /// with Theme, well clear of the space chip.
    #[test]
    fn project_then_env_then_space_each_a_group_gap_apart() {
        let theme = Theme::dark();
        let (term, hits) = render_wide(&theme, "alpha", "qa", false, None, 130);
        let project = hits.rect_of(&Hit::HeaderProject).unwrap();
        let project_cycle = hits
            .rect_of(&Hit::HeaderProjectCycle)
            .expect("project cycle hit");
        let env = hits.rect_of(&Hit::HeaderEnv).unwrap();
        let env_cycle = hits.rect_of(&Hit::HeaderEnvCycle).expect("env cycle hit");
        let space = hits.rect_of(&Hit::HeaderSpace).expect("space hit");
        let space_cycle = hits
            .rect_of(&Hit::HeaderSpaceCycle)
            .expect("space cycle hit");
        let manage = hits.rect_of(&Hit::HeaderManage).unwrap();
        assert_eq!(project.x, project_cycle.x + project_cycle.width + 1);
        assert_eq!(
            env_cycle.x,
            project.x + project.width + 4,
            "a group gap after the project chip, same as the others"
        );
        assert_eq!(
            env.x,
            env_cycle.x + env_cycle.width + 1,
            "its pill leads it"
        );
        assert_eq!(
            space_cycle.x,
            env.x + env.width + 4,
            "a group gap before the space pill"
        );
        assert_eq!(space.x, space_cycle.x + space_cycle.width + 1);
        assert!(
            manage.x > space.x + space.width + 4,
            "Manage is off in the right cluster, not the next chip along: {manage:?} vs {space:?}"
        );
        let label: String = (space.x..space.x + space.width)
            .map(|x| cell(&term, x, space.y).symbol().to_string())
            .collect();
        assert_eq!(label, " Space: main \u{25be} ");
        let pill: String = (space_cycle.x..space_cycle.x + space_cycle.width)
            .map(|x| cell(&term, x, space_cycle.y).symbol().to_string())
            .collect();
        assert!(pill.contains("alt+c"), "{pill:?}");
    }

    /// The narrow-bar rule: chips keep their full labels and the three
    /// cycle pills yield, so the Manage chip stays on the bar.
    #[test]
    fn cycle_pills_yield_on_a_bar_too_narrow_for_the_whole_cluster() {
        let theme = Theme::dark();
        let (_term, hits) = render_wide(&theme, "alpha", "qa", false, None, 75);
        assert!(
            hits.rect_of(&Hit::HeaderSpaceCycle).is_none(),
            "the space cycle pill yields"
        );
        assert!(
            hits.rect_of(&Hit::HeaderEnvCycle).is_none(),
            "so does the env cycle pill"
        );
        assert!(
            hits.rect_of(&Hit::HeaderProjectCycle).is_none(),
            "and the project cycle pill"
        );
        let project = hits
            .rect_of(&Hit::HeaderProject)
            .expect("project chip stays");
        let space = hits.rect_of(&Hit::HeaderSpace).expect("space chip stays");
        let env = hits.rect_of(&Hit::HeaderEnv).expect("env chip stays");
        let manage = hits.rect_of(&Hit::HeaderManage).expect("Manage chip stays");
        assert_eq!(env.x, project.x + project.width + 1, "no pill gap left");
        assert_eq!(
            space.width,
            " Space: main \u{25be} ".chars().count() as u16,
            "chips keep their full labels"
        );
        assert_eq!(space.x, env.x + env.width + 1, "no pill gap left");
        assert!(
            manage.x > space.x + space.width,
            "Manage sits clear of the selectors: {manage:?} vs {space:?}"
        );
        assert_eq!(
            manage.x + manage.width,
            75 - 3,
            "and keeps its place at the right margin: {manage:?}"
        );
        assert_eq!(
            manage.width,
            " Manage ".chars().count() as u16 + alt_pill_w(),
            "with room for its keycap once the cycle pills are gone"
        );

        // Given the room for the full cluster, all three pills are back.
        let (_term, hits) = render_wide(&theme, "alpha", "qa", false, None, 130);
        assert!(hits.rect_of(&Hit::HeaderSpaceCycle).is_some());
        assert!(hits.rect_of(&Hit::HeaderEnvCycle).is_some());
        assert!(hits.rect_of(&Hit::HeaderProjectCycle).is_some());
    }

    /// Last to yield: with a long project name at 78 columns the Manage
    /// chip's own `alt+v` keycap goes after the cycle pills, and the
    /// chip — now a bare ` Manage ` — still fits inside the bar.
    #[test]
    fn manage_keycap_yields_after_the_cycle_pills_on_a_long_project_name() {
        let theme = Theme::dark();
        let project = "a-long-project"; // 14 chars
        let (term, hits) = render_wide(&theme, project, "qa", false, None, 78);
        assert!(hits.rect_of(&Hit::HeaderSpaceCycle).is_none());
        assert!(hits.rect_of(&Hit::HeaderEnvCycle).is_none());
        let manage = hits.rect_of(&Hit::HeaderManage).expect("Manage chip stays");
        assert_eq!(
            manage.width,
            " Manage ".chars().count() as u16,
            "the keycap yielded; the label never does"
        );
        assert!(
            manage.x + manage.width <= 78,
            "the whole chip is inside the bar: {manage:?}"
        );
        assert_eq!(row_text(&term, &manage), " Manage ");

        // Wide enough for the keycap but not the cycle pills: only the
        // pills yield.
        let (_term, hits) = render_wide(&theme, project, "qa", false, None, 90);
        assert!(hits.rect_of(&Hit::HeaderSpaceCycle).is_none());
        let manage = hits.rect_of(&Hit::HeaderManage).unwrap();
        assert!(
            manage.width > " Manage ".chars().count() as u16,
            "the keycap is back once there is room for it"
        );
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

    /// The env chip opens the chooser; the keycap pill ahead of it is the
    /// cycle affordance — footer-chip keycap styling (muted tint over the
    /// control fill), one gap column off the chip so it reads as its own
    /// button, lifting on hover like any clickable pill.
    #[test]
    fn alt_x_keycap_pill_sits_one_column_ahead_of_the_env_chip() {
        let theme = Theme::dark();
        let (term, hits) = render(&theme, "alpha", "qa", None);
        let env_rect = hits.rect_of(&Hit::HeaderEnv).unwrap();
        let rect = hits
            .rect_of(&Hit::HeaderEnvCycle)
            .expect("env-cycle pill registered");
        assert_eq!(env_rect.x, rect.x + rect.width + 1);
        assert_eq!(
            row_text(&term, &rect),
            format!(" {}+x ", crate::keys::alt_label())
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

    /// The Manage chip sits in the right cluster, a wide group gap left
    /// of the Theme chip — so Theme's own pill clearly belongs to Theme
    /// rather than trailing Manage — in the footer's clickable idiom with
    /// the keycap leading the name: `alt+v` pill + prominent full name.
    #[test]
    fn manage_chip_leads_the_theme_chip_with_a_leading_keycap() {
        let theme = Theme::dark();
        let (term, hits) = render_wide(&theme, "alpha", "qa", false, None, 150);
        let rect = hits
            .rect_of(&Hit::HeaderManage)
            .expect("manage chip registered");
        let theme_rect = hits.rect_of(&Hit::HeaderTheme).expect("theme chip");
        assert_eq!(
            theme_rect.x,
            rect.x + rect.width + 4,
            "a group gap between Manage and Theme"
        );
        assert_eq!(
            row_text(&term, &rect),
            format!(" {}+v  Manage ", crate::keys::alt_label())
        );
        let label_cell = cell(&term, rect.x + alt_pill_w() + 1, rect.y);
        assert_eq!(label_cell.symbol(), "M");
        assert_eq!(label_cell.fg, theme.text, "prominent label, not muted");
        assert_eq!(label_cell.bg, theme.panel);
        assert_eq!(
            cell(&term, rect.x + 1, rect.y).bg,
            theme.tint(theme.text_muted, theme.control),
            "leading keycap pill tint matches the footer chips'"
        );
    }

    /// Width of a ` alt+? ` keycap pill on this platform.
    fn alt_pill_w() -> u16 {
        format!(" {}+v ", crate::keys::alt_label()).chars().count() as u16
    }

    /// While the Manage screen is open the whole chip holds the pressed
    /// fill, same as the old `vars` toggle did.
    #[test]
    fn manage_chip_holds_the_pressed_fill_while_active() {
        let theme = Theme::dark();
        let (term, hits) = render_wide(&theme, "alpha", "qa", true, None, 130);
        let rect = hits.rect_of(&Hit::HeaderManage).unwrap();
        assert_eq!(
            cell(&term, rect.x + alt_pill_w() + 1, rect.y).bg,
            theme.control_pressed,
            "label ground shows the pressed state"
        );
        assert_eq!(
            cell(&term, rect.x + 1, rect.y).bg,
            theme.tint(theme.text_muted, theme.control_pressed),
            "keycap tint derives from the pressed fill"
        );
    }

    /// The Theme chip gets the same treatment: leading `alt+t` keycap
    /// pill + prominent name, right-aligned at the bar's 3-column margin,
    /// the pill lifting on hover.
    #[test]
    fn theme_chip_shows_its_leading_keycap_and_name() {
        let theme = Theme::dark();
        let (term, hits) = render_wide(&theme, "alpha", "qa", false, None, 150);
        let rect = hits.rect_of(&Hit::HeaderTheme).unwrap();
        assert_eq!(rect.x + rect.width, 150 - 3, "right-aligned");
        assert_eq!(
            row_text(&term, &rect),
            format!(" {}+t  Theme ", crate::keys::alt_label())
        );
        let label_cell = cell(&term, rect.x + alt_pill_w() + 1, rect.y);
        assert_eq!(label_cell.symbol(), "T");
        assert_eq!(label_cell.fg, theme.text, "prominent label, not muted");
        assert_eq!(label_cell.bg, theme.panel);
        assert_eq!(
            cell(&term, rect.x + 1, rect.y).bg,
            theme.tint(theme.text_muted, theme.control),
            "leading keycap pill tint matches the footer chips'"
        );

        let (term, hits) = render_wide(&theme, "alpha", "qa", false, Some(&Hit::HeaderTheme), 150);
        let rect = hits.rect_of(&Hit::HeaderTheme).unwrap();
        assert_eq!(
            cell(&term, rect.x + 1, rect.y).bg,
            theme.tint(theme.text_muted, theme.control_hover),
            "hover lifts the keycap pill fill"
        );
    }

    /// Saving beats keycaps: on a bar where the full left cluster would
    /// leave no room for the save/discard group, the cycle pills yield
    /// (exactly as they do for the Manage chip) so both Save and Discard
    /// stay on the bar while the request is dirty.
    #[test]
    fn cycle_pills_yield_to_the_save_group_on_a_dirty_bar() {
        let theme = Theme::dark();
        let (_term, hits) = render_wide(&theme, "alpha", "qa", false, None, 120);
        assert!(
            hits.rect_of(&Hit::HeaderSpaceCycle).is_some(),
            "clean: pills stay"
        );
        let (_term, hits) = render_dirty(&theme, 120);
        assert!(
            hits.rect_of(&Hit::FooterChip(crate::action::Action::SaveRequest))
                .is_some(),
            "dirty: the save chip is on the bar"
        );
        assert!(
            hits.rect_of(&Hit::FooterChip(crate::action::Action::DiscardChanges))
                .is_some(),
            "and so is discard"
        );
        assert!(
            hits.rect_of(&Hit::HeaderSpaceCycle).is_none()
                && hits.rect_of(&Hit::HeaderEnvCycle).is_none(),
            "the pills made the room"
        );
    }

    /// The save/discard group appears beside the Manage chip only while
    /// the request is dirty — a clean request needs neither button, so
    /// the bar carries none.
    #[test]
    fn save_group_appears_beside_the_manage_chip_only_while_dirty() {
        use crate::action::Action;
        let theme = Theme::dark();
        let (_term, hits) = render_wide(&theme, "alpha", "qa", false, None, 180);
        assert!(
            hits.rect_of(&Hit::FooterChip(Action::SaveRequest))
                .is_none(),
            "clean: no save chip"
        );
        assert!(
            hits.rect_of(&Hit::FooterChip(Action::DiscardChanges))
                .is_none(),
            "clean: no discard chip"
        );

        let (term, hits) = render_dirty(&theme, 180);
        let save = hits
            .rect_of(&Hit::FooterChip(Action::SaveRequest))
            .expect("dirty: save chip registered");
        let discard = hits
            .rect_of(&Hit::FooterChip(Action::DiscardChanges))
            .expect("dirty: discard chip registered");
        let manage = hits.rect_of(&Hit::HeaderManage).unwrap();
        let theme_rect = hits.rect_of(&Hit::HeaderTheme).unwrap();
        assert!(
            discard.x + discard.width < save.x,
            "discard sits left of save"
        );
        assert_eq!(
            save.x + save.width + 8,
            manage.x,
            "the group sits a clear gap left of the Manage chip: save {save:?} manage {manage:?}"
        );
        assert!(
            manage.x + manage.width < theme_rect.x,
            "and Manage, then Theme, close the bar"
        );
        assert_eq!(
            row_text(&term, &save),
            " ^S  Save ",
            "keycap-then-name idiom"
        );
        assert_eq!(
            row_text(&term, &discard),
            format!(" {}+d  Discard ", crate::keys::alt_label())
        );
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
            hits.rect_of(&Hit::FooterChip(Action::SaveRequest))
                .is_some(),
            "save survives the squeeze"
        );
        assert!(
            hits.rect_of(&Hit::HeaderTheme).is_none(),
            "theme yields to the save group"
        );
    }

    /// The Manage chip never drops: on a bar too narrow to right-anchor
    /// it beside the selectors it follows them and paints (clipped, like
    /// the project/env chips), while the Theme chip drops.
    #[test]
    fn manage_chip_stays_when_the_theme_chip_drops() {
        let theme = Theme::dark();
        let (_term, hits) =
            render_wide(&theme, "a-rather-long-project", "staging", false, None, 60);
        assert!(hits.rect_of(&Hit::HeaderManage).is_some());
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
                draw_header(
                    f,
                    f.area(),
                    &theme,
                    "alpha",
                    "main",
                    "qa",
                    false,
                    false,
                    false,
                    &mut hits,
                    None,
                )
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

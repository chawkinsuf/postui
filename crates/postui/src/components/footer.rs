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

/// Where the response pane's jq bar is, for the chips around it: `alt+q
/// filter` always (it focuses the bar, switching it on if needed), and
/// `alt+shift+q unfilter` only while there is an open bar to switch off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JqBarState {
    Closed,
    Open,
    Focused,
    /// Focused with a completion ghost showing; `cycle` is the `jq_tab`
    /// mode (Tab steps) versus menu (Tab completes and opens the row).
    Completing {
        cycle: bool,
    },
    /// Focused with menu mode's candidate row open: Tab/shift+Tab step it.
    Menu,
}

/// The context-sensitive chips for the focused pane. Each entry is `(key,
/// label, action)`; `None` actions render as plain (unregistered, muted)
/// text on the panel background rather than a filled chip — they describe
/// a binding with no single dispatchable `Action` (e.g. multi-key hints).
/// The always-present palette chip is NOT part of this list: like the quit
/// hint, it never varies with focus, so it lives right-aligned beside quit
/// (see `PALETTE_CHIP` / `draw_footer`). `pub(crate)` so `app::tests`'s
/// mouse-parity sweep (spec §5) can enumerate the same actions
/// `draw_footer` paints as chips, rather than a copy of this list.
pub(crate) fn footer_chips(
    focus: PaneId,
    shift_enter_send: bool,
    sending: bool,
    add_row_label: Option<&'static str>,
    // The editor pane's focus sits on the address bar (method or URL).
    // There alt+a targets no visible table, so its chip gives way to the
    // address bar's own actions: copy url + tls verify.
    url_focused: bool,
    // A data row of the active table is selected (content focus, no cell
    // edit live): advertise its toggle/delete keys. `(index, enabled)` —
    // the toggle chip names the state change it would make.
    table_row_selected: Option<(usize, bool)>,
    // The response pane's jq bar: focused swaps the chip set for the bar's
    // own (apply/cancel/unfilter/describe); open adds the `close` chip.
    jq_bar: JqBarState,
) -> Vec<(&'static str, &'static str, Option<Action>)> {
    let chips: Vec<(&'static str, &'static str, Option<Action>)> = match focus {
        PaneId::Sidebar => vec![
            ("enter", "open", None),
            ("n", "new", Some(Action::PromptNewRequest)),
            ("r", "rename", Some(Action::PromptRenameRequest)),
            ("d", "delete", Some(Action::DeleteSelectedRequest)),
            // Space *switching* is the header's job (its space pill and
            // cycle pill); the sidebar's own space action is moving the
            // selected request into another one.
            ("m", "move", Some(Action::PromptMoveSelectedRequestToSpace)),
        ],
        PaneId::Editor => {
            let mut chips = vec![
                // Shift+Enter is only reportable under the kitty keyboard
                // protocol; where the terminal can't deliver it, ^R is the
                // advertised send key (both bindings stay active regardless).
                // While the open request is in flight the send shortcuts go
                // dead, and esc — the cancel shortcut — is advertised instead.
                if sending {
                    ("esc", "cancel", Some(Action::CancelSend))
                } else {
                    (
                        if shift_enter_send { "⇧enter" } else { "^R" },
                        "send",
                        Some(Action::Send),
                    )
                },
                (
                    "alt+shift+v",
                    "vars",
                    Some(Action::OpenVarPicker { completing: false }),
                ),
                // The split cycle's alt+w is advertised beside the split
                // control itself (the tab-bar row's keycap pill), not here.
                // Arrows are the primary route (method ← URL ↓ tabs ↓ content);
                // alt+left/right cycle tabs; ctrl-digits now switch spaces.
                ("↑↓←→", "navigate", None),
            ];
            if url_focused {
                // "tls" leans on context (the padlock sits in the focused
                // bar); labels stay tight so the vars chip survives the
                // 120-col dirty footer.
                chips.insert(
                    1,
                    (
                        "alt+y",
                        "copy url",
                        Some(Action::CopyToClipboard(crate::action::CopyTarget::Url)),
                    ),
                );
                chips.insert(2, ("alt+i", "tls", Some(Action::ToggleInsecure)));
            } else if let Some(label) = add_row_label {
                // Named for what it adds on the active tab ("add header" on
                // Headers, …); hidden on the Body tab, where the action is
                // inert.
                chips.insert(1, ("alt+a", label, Some(Action::TableAddRow)));
            }
            if let Some((i, enabled)) = table_row_selected {
                // Keyboard twins of the expanded row's ● toggle and 🗑
                // delete buttons; the ␣ keycap keeps the row narrow.
                let toggle_label = if enabled { "disable" } else { "enable" };
                let pos = chips.len() - 2; // before vars + navigate
                chips.insert(pos, ("␣", toggle_label, Some(Action::ToggleTableRow(i))));
                chips.insert(pos + 1, ("d", "delete", Some(Action::DeleteTableRow(i))));
            }
            chips
        }
        PaneId::Response => {
            if let JqBarState::Focused | JqBarState::Completing { .. } | JqBarState::Menu = jq_bar {
                // Enter commits (the filter is live already; Enter just
                // hands focus back to the tree with the filter on), Esc
                // cancels the edit (the filter goes back to what it was),
                // alt+shift+q closes the bar — switching the filter off
                // without losing it. While a completion ghost
                // shows, the Tab chip(s) lead: "tab next" + "→ accept" in
                // cycle mode, or "tab complete" in menu mode; with the
                // menu row open, "tab next" + "shift+tab prev".
                let mut chips = Vec::new();
                match jq_bar {
                    JqBarState::Completing { cycle: true } => {
                        chips.push(("tab", "next", None));
                        chips.push(("→", "accept", None));
                    }
                    JqBarState::Completing { cycle: false } => {
                        chips.push(("tab", "complete", None))
                    }
                    JqBarState::Menu => {
                        chips.push(("tab", "next", None));
                        chips.push(("shift+tab", "prev", None));
                    }
                    _ => {}
                }
                // Enter confirms an entered row's chip (and stays in the
                // bar); otherwise it applies — leaves with the edit kept.
                chips.extend([
                    (
                        "enter",
                        if jq_bar == JqBarState::Menu {
                            "select"
                        } else {
                            "apply"
                        },
                        None,
                    ),
                    ("esc", "cancel", Some(Action::CancelJqEdit)),
                    ("alt+shift+q", "unfilter", Some(Action::ToggleJqBar)),
                    ("✦", "describe…", Some(Action::OpenJqDescribe)),
                ]);
                chips
            } else {
                let mut chips = vec![
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
                    ("alt+q", "filter", Some(Action::OpenJqBar)),
                ];
                if jq_bar == JqBarState::Open {
                    chips.push(("alt+shift+q", "unfilter", Some(Action::ToggleJqBar)));
                }
                chips
            }
        }
    };
    chips
}

/// An owned footer chip — keycap, label, action — for chip sets built at
/// runtime (a confirm modal's own answer keys, the Variable Manager's
/// name-carrying actions) rather than the static per-pane lists.
pub type FooterChip = (String, String, Option<Action>);

/// The command-palette chip: always present regardless of focus, so it
/// sits right-aligned next to the quit hint rather than trailing the
/// per-pane chips.
const PALETTE_CHIP: (&str, &str, Option<Action>) = ("^P", "commands", Some(Action::OpenPalette));

/// The always-present, right-aligned quit chip. Kept separate from
/// `footer_chips` since it never varies with focus; painted through
/// `paint_chip_row` so it reads exactly like its neighbours.
const QUIT_CHIP: (&str, &str, Option<Action>) = ("q", "quit", Some(Action::Quit));

/// A `paint_chip_row` entry's total footprint: ` key ` pill + ` label `
/// (see its own layout comment — 4 columns beyond key+label either way).
fn chip_width((key, label, _): &(&str, &str, Option<Action>)) -> u16 {
    (key.chars().count() + label.chars().count() + 4) as u16
}

#[allow(clippy::too_many_arguments)]
pub fn draw_footer(
    frame: &mut Frame,
    area: Rect,
    theme: &Theme,
    focus: PaneId,
    shift_enter_send: bool,
    sending: bool,
    add_row_label: Option<&'static str>,
    // See `footer_chips`: the editor pane's focus sits on the address bar.
    url_focused: bool,
    // See `footer_chips`: a data row of the active table is selected.
    table_row_selected: Option<(usize, bool)>,
    // See `footer_chips`: where the response pane's jq bar is.
    jq_bar: JqBarState,
    // Replaces the per-pane chips wholesale when `Some` — a modal's own
    // chip set, or the Variable Manager screen's
    // (`VarManager::footer_chips`), whose actions target
    // options/declarations rather than the main screen's requests. The
    // right-aligned save/palette/quit chips stay put.
    chips_override: Option<Vec<FooterChip>>,
    // `false` while an open modal supplies the chips: it captures the
    // keyboard, so the palette chip — whose shortcut goes dead under it —
    // hides rather than advertise a key that won't work. Quit stays (the
    // modified quit combo pre-empts everything).
    globals_live: bool,
    // Whether a plain `q` actually reaches `Action::Quit` right now. Where
    // it doesn't — a modal open, a non-Main screen, the editor's text
    // areas — the quit chip's keycap shows the pre-empting `^C` combo
    // instead of advertising a key that would just type.
    plain_q_quits: bool,
    hits: &mut HitMap,
    hovered: Option<&Hit>,
) {
    let buf = frame.buffer_mut();
    fill(buf, area, theme.panel);

    if area.height == 0 {
        return;
    }
    let mid_y = area.y + area.height / 2;

    let quit_chip = if plain_q_quits {
        QUIT_CHIP
    } else {
        ("^C", "quit", Some(Action::Quit))
    };
    let quit_w = chip_width(&quit_chip);
    let quit_x = (area.x + area.width).saturating_sub(quit_w + 1);
    paint_chip_row(
        buf,
        mid_y,
        quit_x,
        quit_x + quit_w,
        &[quit_chip],
        theme,
        hits,
        hovered,
    );

    let right_limit = if globals_live {
        // The palette chip sits right-aligned, one gap column left of quit.
        let palette_w = chip_width(&PALETTE_CHIP);
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

        // Per-pane chips stop one column shy of the palette so the two
        // never collide. (The save/discard group lives in the app bar now,
        // beside the Theme chip.)
        palette_x.saturating_sub(1)
    } else {
        quit_x.saturating_sub(1)
    };
    // `paint_chip_row` works on borrowed strs; an override's owned chips
    // borrow from `owned`, the static per-pane list is used as-is.
    let owned: Vec<FooterChip>;
    let chips: Vec<(&str, &str, Option<Action>)> = match chips_override {
        Some(c) => {
            owned = c;
            owned
                .iter()
                .map(|(k, l, a)| (k.as_str(), l.as_str(), a.clone()))
                .collect()
        }
        None => footer_chips(
            focus,
            shift_enter_send,
            sending,
            add_row_label,
            url_focused,
            table_row_selected,
            jq_bar,
        ),
    };
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
/// `right_limit`. Each chip with `Some(action)` emphasizes its label: the
/// key combo sits in a small `Chip`-style pill tinted `theme.text_muted`
/// on `theme.control` (lifting to `theme.control_hover` under the mouse per
/// `hovered`), with the label following in prominent `theme.text` text
/// beside it — the action is the content, the shortcut the affordance.
/// Registers `Hit::FooterChip(action)` over the combined span. A `None`
/// action renders as fully plain (unregistered) text: key muted, label
/// prominent — the pill IS the clickability signal, so a hint with no
/// single dispatchable action never visually promises a click it can't honor.
/// Shared by the footer's own hint row and the editor toolbar. Returns the x
/// position just past the last chip painted.
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
                    color: theme.text_muted,
                }
                .paint(buf, x, y, on, theme);
                let label_text = format!(" {label} ");
                text(
                    buf,
                    x + pill_w,
                    y,
                    &label_text,
                    theme.text,
                    theme.panel,
                    false,
                );
                hits.register(chip_area, Hit::FooterChip(a.clone()));
            }
            None => {
                let key_text = format!(" {key}");
                let label_text = format!(" {label}  ");
                text(buf, x, y, &key_text, theme.text_muted, theme.panel, true);
                text(
                    buf,
                    x + key_text.chars().count() as u16,
                    y,
                    &label_text,
                    theme.text,
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

    #[test]
    fn a_completion_ghost_advertises_tab_and_accept() {
        let chips = |state: JqBarState| {
            footer_chips(PaneId::Response, false, false, None, false, None, state)
                .into_iter()
                .map(|(k, l, _)| format!("{k} {l}"))
                .collect::<Vec<_>>()
        };
        let focused = chips(JqBarState::Focused);
        assert!(!focused.iter().any(|c| c.starts_with("tab")), "{focused:?}");
        let cycle = chips(JqBarState::Completing { cycle: true });
        assert_eq!(
            &cycle[..2],
            &["tab next".to_string(), "→ accept".to_string()]
        );
        assert_eq!(&cycle[2..], &focused[..], "the usual chips follow");
        let menu_ghost = chips(JqBarState::Completing { cycle: false });
        assert_eq!(menu_ghost[0], "tab complete");
        assert_eq!(&menu_ghost[1..], &focused[..]);
        let menu = chips(JqBarState::Menu);
        assert_eq!(
            &menu[..3],
            &[
                "tab next".to_string(),
                "shift+tab prev".to_string(),
                "enter select".to_string()
            ]
        );
        assert_eq!(&menu[3..], &focused[1..], "enter reads select, not apply");
    }

    fn render(focus: PaneId) -> String {
        let (content, _) = render_hits(focus);
        content
    }

    fn render_hits(focus: PaneId) -> (String, crate::hit::HitMap) {
        let theme = Theme::for_terminal();
        let backend = TestBackend::new(120, FOOTER_HEIGHT);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal
            .draw(|f| {
                draw_footer(
                    f,
                    f.area(),
                    &theme,
                    focus,
                    false,
                    false,
                    Some("add header"),
                    false,
                    None,
                    JqBarState::Closed,
                    None,
                    true,
                    true,
                    &mut hits,
                    None,
                )
            })
            .unwrap();
        (format!("{:?}", terminal.backend().buffer()), hits)
    }

    #[test]
    fn send_chip_advertises_shift_enter_only_when_the_terminal_reports_it() {
        let with = footer_chips(
            PaneId::Editor,
            true,
            false,
            Some("add header"),
            false,
            None,
            JqBarState::Closed,
        );
        assert!(with.iter().any(|(k, l, _)| *k == "⇧enter" && *l == "send"));
        let without = footer_chips(
            PaneId::Editor,
            false,
            false,
            Some("add header"),
            false,
            None,
            JqBarState::Closed,
        );
        assert!(without.iter().any(|(k, l, _)| *k == "^R" && *l == "send"));
    }

    /// While the open request is in flight, the send shortcuts go dead and
    /// esc is the cancel shortcut — the footer must advertise that instead
    /// of a send key that would do nothing.
    #[test]
    fn send_chip_becomes_esc_cancel_while_the_open_request_is_in_flight() {
        let sending = footer_chips(
            PaneId::Editor,
            false,
            true,
            Some("add header"),
            false,
            None,
            JqBarState::Closed,
        );
        assert!(
            sending
                .iter()
                .any(|(k, l, a)| *k == "esc" && *l == "cancel" && *a == Some(Action::CancelSend))
        );
        assert!(
            !sending.iter().any(|(_, l, _)| *l == "send"),
            "a dead send key must not be advertised"
        );
    }

    /// The save/discard group lives in the app bar now (beside the Theme
    /// chip): the footer must not carry it on any pane.
    #[test]
    fn footer_carries_no_save_or_discard_chips() {
        for focus in [PaneId::Sidebar, PaneId::Editor, PaneId::Response] {
            let (_, hits) = render_hits(focus);
            assert!(
                hits.rect_of(&Hit::FooterChip(Action::SaveRequest))
                    .is_none(),
                "{focus:?}: save moved to the app bar"
            );
            assert!(
                hits.rect_of(&Hit::FooterChip(Action::DiscardChanges))
                    .is_none(),
                "{focus:?}: discard moved to the app bar"
            );
        }
    }

    /// The editor's context chips advertise vars (alt+shift+v — ctrl+v is
    /// paste now) since save moved to the global right-side group — and no
    /// longer a second ^S.
    #[test]
    fn editor_context_chips_offer_vars_and_no_duplicate_save() {
        let chips = footer_chips(
            PaneId::Editor,
            false,
            false,
            Some("add header"),
            false,
            None,
            JqBarState::Closed,
        );
        assert!(chips.iter().any(|(k, l, a)| *k == "alt+shift+v"
            && *l == "vars"
            && *a == Some(Action::OpenVarPicker { completing: false })));
        assert!(
            !chips
                .iter()
                .any(|(_, _, a)| *a == Some(Action::SaveRequest)),
            "save lives in the global right-side group, not the context chips"
        );
    }

    /// With the address bar focused, alt+a targets no visible table, so
    /// its chip gives way to the address bar's own actions: copy the
    /// resolved URL and toggle TLS verification.
    #[test]
    fn address_bar_focus_swaps_add_row_for_copy_url_and_tls_chips() {
        let chips = footer_chips(
            PaneId::Editor,
            false,
            false,
            Some("add header"),
            true,
            None,
            JqBarState::Closed,
        );
        assert!(
            !chips
                .iter()
                .any(|(_, _, a)| *a == Some(Action::TableAddRow)),
            "add-row is inert while the address bar is focused"
        );
        assert!(chips.iter().any(|(k, l, a)| *k == "alt+y"
            && *l == "copy url"
            && *a == Some(Action::CopyToClipboard(crate::action::CopyTarget::Url))));
        assert!(
            chips.iter().any(|(k, l, a)| *k == "alt+i"
                && *l == "tls"
                && *a == Some(Action::ToggleInsecure))
        );

        let chips = footer_chips(
            PaneId::Editor,
            false,
            false,
            Some("add header"),
            false,
            None,
            JqBarState::Closed,
        );
        assert!(
            chips
                .iter()
                .any(|(_, _, a)| *a == Some(Action::TableAddRow)),
            "content focus keeps the add-row chip"
        );
        assert!(
            !chips
                .iter()
                .any(|(_, _, a)| *a == Some(Action::ToggleInsecure)),
            "address-bar chips stay off the content-focus footer"
        );
    }

    /// The split shortcut is advertised beside the split control itself
    /// (the editor tab-bar row's keycap pill), not as a footer chip — no
    /// pane's footer list carries it.
    #[test]
    fn no_footer_list_carries_the_split_chip() {
        for pane in [PaneId::Sidebar, PaneId::Editor, PaneId::Response] {
            for url_focused in [false, true] {
                let chips = footer_chips(
                    pane,
                    false,
                    false,
                    Some("add header"),
                    url_focused,
                    None,
                    JqBarState::Closed,
                );
                assert!(
                    !chips.iter().any(|(_, _, a)| *a == Some(Action::CycleSplit)),
                    "{pane:?} url_focused={url_focused} still advertises split"
                );
            }
        }
    }

    /// A selected data row advertises its own quick actions — the
    /// keyboard twins of the expanded row's ● toggle and 🗑 delete
    /// buttons.
    #[test]
    fn selected_table_row_adds_toggle_and_delete_chips() {
        // The toggle chip names the state change it would make: an enabled
        // row offers "disable", a disabled one "enable".
        let chips = footer_chips(
            PaneId::Editor,
            false,
            false,
            Some("add header"),
            false,
            Some((2, true)),
            JqBarState::Closed,
        );
        assert!(chips.iter().any(|(k, l, a)| *k == "␣"
            && *l == "disable"
            && *a == Some(Action::ToggleTableRow(2))));
        assert!(
            chips.iter().any(|(k, l, a)| *k == "d"
                && *l == "delete"
                && *a == Some(Action::DeleteTableRow(2)))
        );

        let chips = footer_chips(
            PaneId::Editor,
            false,
            false,
            Some("add header"),
            false,
            Some((2, false)),
            JqBarState::Closed,
        );
        assert!(
            chips
                .iter()
                .any(|(_, l, a)| *l == "enable" && *a == Some(Action::ToggleTableRow(2)))
        );

        let chips = footer_chips(
            PaneId::Editor,
            false,
            false,
            Some("add header"),
            false,
            None,
            JqBarState::Closed,
        );
        assert!(
            !chips
                .iter()
                .any(|(_, l, _)| *l == "disable" || *l == "enable" || *l == "delete"),
            "no row selected: no row chips"
        );
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
            .draw(|f| {
                draw_footer(
                    f,
                    f.area(),
                    &theme,
                    PaneId::Response,
                    false,
                    false,
                    Some("add header"),
                    false,
                    None,
                    JqBarState::Closed,
                    None,
                    true,
                    true,
                    &mut hits,
                    None,
                )
            })
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
            .draw(|f| {
                draw_footer(
                    f,
                    f.area(),
                    &theme,
                    PaneId::Sidebar,
                    false,
                    false,
                    Some("add header"),
                    false,
                    None,
                    JqBarState::Closed,
                    None,
                    true,
                    true,
                    &mut hits,
                    None,
                )
            })
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
            .rect_of(&Hit::FooterChip(Action::DeleteSelectedRequest))
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
            .draw(|f| {
                draw_footer(
                    f,
                    f.area(),
                    &theme,
                    PaneId::Sidebar,
                    false,
                    false,
                    Some("add header"),
                    false,
                    None,
                    JqBarState::Closed,
                    None,
                    true,
                    true,
                    &mut hits,
                    None,
                )
            })
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
    fn chip_paints_muted_bold_key_in_a_tinted_pill_and_prominent_label_beside_it() {
        let theme = Theme::for_terminal();
        let backend = TestBackend::new(120, FOOTER_HEIGHT);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal
            .draw(|f| {
                draw_footer(
                    f,
                    f.area(),
                    &theme,
                    PaneId::Sidebar,
                    false,
                    false,
                    Some("add header"),
                    false,
                    None,
                    JqBarState::Closed,
                    None,
                    true,
                    true,
                    &mut hits,
                    None,
                )
            })
            .unwrap();
        let rect = hits
            .rect_of(&Hit::FooterChip(Action::PromptNewRequest))
            .expect("new-request chip hit registered");
        let buf = terminal.backend().buffer();
        // First cell of the chip is the pill's leading space, then the key
        // glyph, bold, on the text_muted-tinted pill fill — not a flat
        // `theme.control` fill. The glyph's own color is a contrast pick
        // against that fill (checkpoint-2: `theme.text_muted` is itself light
        // enough that the tinted fill reads light too, so painting the
        // key in `theme.text_muted` unconditionally would be light-on-light).
        let key_cell = buf.cell((rect.x + 1, rect.y)).unwrap();
        assert_eq!(key_cell.symbol(), "n");
        let fill = theme.tint(theme.text_muted, theme.control);
        assert_eq!(key_cell.bg, fill);
        if crate::theme::is_light(fill) {
            assert!(
                !crate::theme::is_light(key_cell.fg),
                "light fill needs dark key text: {key_cell:?}"
            );
        } else {
            // The key keeps text_muted's hue but contrast-guarded against
            // its own tinted fill (soft palettes land the two within a
            // whisper of each other otherwise).
            assert_eq!(
                key_cell.fg,
                crate::theme::ensure_min_contrast(theme.text_muted, fill, 0.35)
            );
        }
        assert!(key_cell.modifier.contains(ratatui::style::Modifier::BOLD));
        // The label follows one gap column after the pill (" n " is 3
        // cells, then the label's own leading space), prominent (text, not muted),
        // not bold, and sitting on the plain panel — no chip fill of its own.
        let label_cell = buf.cell((rect.x + 4, rect.y)).unwrap();
        assert_eq!(label_cell.symbol(), "n"); // first letter of "new"
        assert_eq!(label_cell.fg, theme.text);
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
            .draw(|f| {
                draw_footer(
                    f,
                    f.area(),
                    &theme,
                    PaneId::Sidebar,
                    false,
                    false,
                    Some("add header"),
                    false,
                    None,
                    JqBarState::Closed,
                    None,
                    true,
                    true,
                    &mut hits,
                    None,
                )
            })
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
        assert_eq!(plain_cell.fg, theme.text_muted);

        // "n new" (PromptNewRequest) IS clickable: tinted key pill.
        let rect = hits
            .rect_of(&Hit::FooterChip(Action::PromptNewRequest))
            .expect("new-request chip hit registered");
        let chip_cell = buf.cell((rect.x + 1, rect.y)).unwrap();
        assert_eq!(
            chip_cell.bg,
            theme.tint(theme.text_muted, theme.control),
            "clickable entry keeps its tinted key pill"
        );
    }

    /// Quit paints exactly like its neighbours — a quiet keycap pill with
    /// a prominent label — rather than the plain muted text it used to be, so
    /// the right-side chips read as one consistent family.
    #[test]
    fn quit_is_a_chip_like_its_neighbours() {
        let theme = Theme::for_terminal();
        let backend = TestBackend::new(120, FOOTER_HEIGHT);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal
            .draw(|f| {
                draw_footer(
                    f,
                    f.area(),
                    &theme,
                    PaneId::Sidebar,
                    false,
                    false,
                    Some("add header"),
                    false,
                    None,
                    JqBarState::Closed,
                    None,
                    true,
                    true,
                    &mut hits,
                    None,
                )
            })
            .unwrap();
        let rect = hits
            .rect_of(&Hit::FooterChip(Action::Quit))
            .expect("quit hit registered");
        assert!(
            rect.x + rect.width >= 119,
            "quit chip is right-aligned: {rect:?}"
        );
        let buf = terminal.backend().buffer();
        let cell = buf.cell((rect.x + 1, rect.y)).unwrap();
        assert_eq!(cell.symbol(), "q");
        assert_eq!(
            cell.bg,
            theme.tint(theme.text_muted, theme.control),
            "quit's key sits in the same tinted pill as every other chip"
        );
    }

    /// The action label is the content and the shortcut is the affordance —
    /// emphasis was inverted before (accent keycap, muted label). The label
    /// must paint in `theme.text` and the keycap pill's tint must derive from
    /// `theme.text_muted`, not `theme.accent`.
    #[test]
    fn chip_emphasis_action_label_is_prominent_and_keycap_is_quiet() {
        let theme = Theme::for_terminal();
        let backend = TestBackend::new(60, 1);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal
            .draw(|f| {
                paint_chip_row(
                    f.buffer_mut(),
                    0,
                    0,
                    60,
                    &[("^R", "send", Some(Action::Send)), ("g", "top", None)],
                    &theme,
                    &mut hits,
                    None,
                );
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        // Clickable chip: pill " ^R " occupies columns 0..4; label " send "
        // follows. The pill fill tints from text_muted, not accent.
        assert_eq!(
            buffer[(1, 0)].bg,
            theme.tint(theme.text_muted, theme.control),
            "keycap pill tint derives from text_muted"
        );
        let label_cell = buffer[(5, 0)].clone(); // 's' of " send "
        assert_eq!(label_cell.symbol(), "s");
        assert_eq!(label_cell.fg, theme.text, "action label is prominent");
        // Plain (None-action) entry: key muted, label prominent. The plain
        // entry starts at x = 4 (pill) + 6 (" send ") + 2 (gap) = 12; its key
        // text " g" puts 'g' at column 13 and the label 't' at column 15.
        let key_cell = buffer[(13, 0)].clone();
        assert_eq!(key_cell.symbol(), "g");
        assert_eq!(key_cell.fg, theme.text_muted, "plain key is quiet");
        let plain_label = buffer[(15, 0)].clone();
        assert_eq!(plain_label.symbol(), "t");
        assert_eq!(plain_label.fg, theme.text, "plain label is prominent");
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
            .draw(|f| {
                draw_footer(
                    f,
                    f.area(),
                    &theme,
                    PaneId::Sidebar,
                    false,
                    false,
                    Some("add header"),
                    false,
                    None,
                    JqBarState::Closed,
                    None,
                    true,
                    true,
                    &mut hits,
                    None,
                )
            })
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

use crate::action::{Action, Verb};
use crate::hit::{Hit, HitMap};
use crate::keyroute::spell::Key;
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
    /// Focused with a closing-bracket ghost: Tab, Right and End all accept
    /// it (there is only one right answer, so menu mode's Tab-completes
    /// convention applies even though no candidate row is open).
    Closer,
    /// Focused with an AI "describe" request outstanding: Esc cancels
    /// that request (the bar's own Esc arm), so the chip must say so
    /// rather than promise `done`.
    Describing,
}

/// The row under the focused pane's cursor, for the chips naming its
/// keys: an editable row (its copy, toggle and delete; `headers` on the
/// Headers tab, where copy has a whole-header key too), an auto row of the
/// Headers tab's section (copy, and the pencil's take-over where the row
/// has one; spec 2026-10-25 S16), or a header row of the response's
/// Headers view (copy). One copy chip each (spec 2026-10-26 H5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TableCursor {
    Row { index: usize, enabled: bool, headers: bool },
    Auto { index: usize, pencil: bool },
    ResponseHeader,
}

/// The context-sensitive chips for the focused pane. Each entry is `(key,
/// label, action)`, the key a [`Key`] kind that `draw_footer` spells for
/// the active profile; `None` actions render as plain (unregistered, muted)
/// text on the panel background rather than a filled chip — they describe
/// a binding with no single dispatchable `Action` (e.g. multi-key hints).
/// The always-present palette chip is NOT part of this list: like the quit
/// hint, it never varies with focus, so it lives right-aligned beside quit
/// (see `draw_footer`). `pub(crate)` so `app::tests`'s
/// mouse-parity sweep (spec §5) can enumerate the same actions
/// `draw_footer` paints as chips, rather than a copy of this list.
#[allow(clippy::too_many_arguments)]
pub(crate) fn footer_chips(
    focus: PaneId,
    shift_enter_send: bool,
    sending: bool,
    add_row_label: Option<&'static str>,
    // The editor pane's focus sits on the address bar (method or URL).
    // There alt+a targets no visible table, so its chip gives way to the
    // address bar's own actions: copy url + tls verify.
    url_focused: bool,
    // Precisely the URL line, selected (not open) — narrower than
    // `url_focused` (which also covers the Method badge). Gates the
    // "enter edit" chip alone: that chip names `Action::FocusUrl`, which
    // is right only when the URL line itself, not the method badge, has
    // focus (spec 2026-09-16, "the hint must name the action actually
    // dispatched").
    url_selected: bool,
    // A data row of the active table is selected (content focus, no cell
    // edit live): advertise its toggle/delete keys, the toggle chip naming
    // the state change it would make. On an auto row, its copy and
    // take-over keys instead.
    table_row_selected: Option<TableCursor>,
    // The response pane's jq bar: focused swaps the chip set for the bar's
    // own (apply/cancel/unfilter/describe); open adds the `close` chip.
    jq_bar: JqBarState,
    // *This pane's* own text field owns the caret — `App::pane_field_open`,
    // not the app-wide `field_open` (a live cell edit survives a jump to
    // the response pane, and must not put the editor's chips on its row).
    // Esc closes such a field keeping its text (the field rule, spec
    // 2026-09-15), so the pane advertises `esc done` and never `esc
    // cancel`. On the response pane it also picks out the search box's own
    // pair, the field the jq bar isn't.
    field_open: bool,
) -> Vec<(Key, &'static str, Option<Action>)> {
    let chips: Vec<(Key, &'static str, Option<Action>)> = match focus {
        PaneId::Sidebar => vec![
            (Key::Named("enter"), "open", None),
            // The keycaps spell the router's verb rows (`a`, `dd`/`d`, …);
            // each action is what `Sidebar::verb_action` resolves on a
            // request row (plan M2 Deviation 3).
            (Key::Of(Action::OnSelection(Verb::Add)), "new", Some(Action::PromptNewRequest)),
            (
                Key::Of(Action::OnSelection(Verb::Rename)),
                "rename",
                Some(Action::PromptRenameRequest),
            ),
            (
                Key::Of(Action::OnSelection(Verb::Delete { count: 1 })),
                "delete",
                Some(Action::DeleteSelectedRequest),
            ),
            // Space *switching* is the header's job (its space pill and
            // cycle pill); the sidebar's own space action is moving the
            // selected request into another one.
            (
                Key::Of(Action::OnSelection(Verb::Move)),
                "move",
                Some(Action::PromptMoveSelectedRequestToSpace),
            ),
            (Key::Named("alt+\u{2191}\u{2193}"), "reorder", None),
            // The sidebar takes no text: a paste there imports its curl
            // (spec 2026-10-15 curl import visibility V9).
            (Key::Of(Action::Paste), "import curl", Some(Action::Paste)),
        ],
        PaneId::Editor => {
            let mut chips: Vec<(Key, &'static str, Option<Action>)> = Vec::new();
            // Shift+Enter is only reportable under the kitty keyboard
            // protocol; where the terminal can't deliver it, ^R is the
            // advertised send key (both bindings stay active regardless).
            // While the open request is in flight the send shortcuts go
            // dead, and esc — the cancel shortcut — is advertised instead.
            // With a field open there is nothing honest to put in that
            // slot: Esc belongs to the field, and the send keys are dead
            // too (`Action::Send` refuses while the open request is in
            // flight), so the slot goes empty and `esc done` leads the row.
            match (sending, field_open) {
                (true, true) => {}
                (true, false) => {
                    chips.push((Key::Named("esc"), "cancel", Some(Action::CancelSend)))
                }
                _ => chips.push((
                    Key::Send { shift_enter: shift_enter_send },
                    "send",
                    Some(Action::Send),
                )),
            }
            chips.extend([
                (
                    Key::Of(Action::OpenVarPicker { completing: false }),
                    "vars",
                    Some(Action::OpenVarPicker { completing: false }),
                ),
                // The split cycle's alt+w is found in the palette (spec
                // E12a), not here.
                // Arrows are the primary route (method ← URL ↓ content)
                // and get no chip of their own (2026-09-18: the old
                // `↑↓←→ navigate` chip made way for the tab chord — the
                // URL-focused row must keep hover-hint room at 160 cols);
                // ctrl-digits switch spaces.
                // The tab chord chip every strip shares (response pane,
                // Manage tabs): same keycap, same wording. Last, so the
                // dynamic inserts below never shift it.
                (tabs_key(), "tabs", Some(Action::CycleTabs(1))),
            ]);
            // Every insert below lands before the trailing vars + tabs
            // pair, so it keeps its place whether or not the leading
            // send/cancel slot is there.
            if url_focused {
                // "tls" leans on context (the padlock sits in the focused
                // bar); labels stay tight so the vars chip survives the
                // 120-col dirty footer.
                let at = chips.len() - 2;
                chips.insert(
                    at,
                    (
                        Key::Of(Action::CopyToClipboard(crate::action::CopyTarget::Url)),
                        "copy url",
                        Some(Action::CopyToClipboard(crate::action::CopyTarget::Url)),
                    ),
                );
                chips.insert(
                    at + 1,
                    (Key::Of(Action::ToggleInsecure), "tls", Some(Action::ToggleInsecure)),
                );
            } else if let Some(label) = add_row_label {
                // Named for what it adds on the active tab ("add header" on
                // Headers, …); hidden on the Body tab, where the action is
                // inert.
                let at = chips.len() - 2;
                // The keycap is the table's `a`; where that row does not
                // apply (`SubFocus::None`, a live cell edit) arrows'
                // `alt+a` stands in, and vim shows no key.
                chips.insert(
                    at,
                    (
                        Key::OneOf(Action::OnSelection(Verb::Add), Action::TableAddRow),
                        label,
                        Some(Action::TableAddRow),
                    ),
                );
            }
            let pos = chips.len() - 2; // before vars + tabs
            // One copy chip on a header row, its keycap both keys: the
            // value's and the whole header's (`y/Y`, vim's `yy/"+yy`; spec
            // 2026-10-26 H5).
            let header_copy_key = || {
                Key::Pair(Action::OnSelection(Verb::Yank), Action::OnSelection(Verb::CopyHeader))
            };
            match table_row_selected {
                Some(TableCursor::Row { index: i, enabled, headers }) => {
                    // Keyboard twins of the expanded row's copy pill, ●
                    // toggle and 󰆴 delete buttons; the ␣ keycap keeps the
                    // row narrow. The copy chip runs the yank key, which in
                    // arrows copies as the pill does (H1).
                    let copy_key = if headers {
                        header_copy_key()
                    } else {
                        Key::Of(Action::OnSelection(Verb::Yank))
                    };
                    chips.insert(pos, (copy_key, "copy", Some(Action::YankTableRow(i))));
                    let toggle_label = if enabled { "disable" } else { "enable" };
                    chips.insert(
                        pos + 1,
                        (Key::Named("␣"), toggle_label, Some(Action::ToggleTableRow(i))),
                    );
                    chips.insert(
                        pos + 2,
                        (
                            Key::Of(Action::OnSelection(Verb::Delete { count: 1 })),
                            "delete",
                            Some(Action::DeleteTableRow(i)),
                        ),
                    );
                }
                Some(TableCursor::Auto { index: i, pencil }) => {
                    // Keyboard twins of the auto row's copy pill and, where
                    // it has one, its 󰏫 pencil (spec 2026-10-25 S14–S16).
                    chips.insert(
                        pos,
                        (
                            header_copy_key(),
                            "copy",
                            Some(Action::CopyToClipboard(
                                crate::action::CopyTarget::ComputedHeader(i),
                            )),
                        ),
                    );
                    if pencil {
                        chips.insert(
                            pos + 1,
                            (Key::Named("enter"), "edit", Some(Action::OverrideAutoHeader(i))),
                        );
                    }
                }
                Some(TableCursor::ResponseHeader) | None => {}
            }
            if field_open {
                // Inserted last so the row/address-bar chips above land
                // where they mean to; it leads the row all the same.
                chips.insert(0, (Key::Named("esc"), "done", Some(Action::CloseField)));
            } else if url_selected {
                // Selected, not open: the field rule's other half — Enter
                // opens it, matching every other surface (spec
                // 2026-09-16). `Action::FocusUrl` both focuses the bar and
                // opens it (`Editor::open_url_from_app`), so it is the
                // right action here too, not just for a jump from another
                // pane. Gated on `url_selected`, not `url_focused`: the
                // method badge's Enter opens the method dropdown, not
                // this action.
                chips.insert(0, (Key::Named("enter"), "edit", Some(Action::FocusUrl)));
            }
            chips
        }
        PaneId::Response => {
            if jq_bar == JqBarState::Describing {
                // The bar keeps the caret while the AI request runs, but
                // its Esc is the request's cancel — the chip names that
                // action, not the field close it would otherwise be.
                vec![(Key::Named("esc"), "cancel describe", Some(Action::CancelJqDescribe))]
            } else if let JqBarState::Focused
            | JqBarState::Completing { .. }
            | JqBarState::Menu
            | JqBarState::Closer = jq_bar
            {
                // Enter commits (the filter is live already; Enter just
                // hands focus back to the tree with the filter on), Esc
                // does the same — it leaves with the edit kept (the field
                // rule), alt+shift+q closes the bar — switching the filter
                // off without losing it. While a completion ghost
                // shows, the Tab chip(s) lead: "tab next" + "→ accept" in
                // cycle mode, or "tab complete" in menu mode; with the
                // menu row open, "tab next" + "shift+tab prev".
                let mut chips = Vec::new();
                match jq_bar {
                    JqBarState::Completing { cycle: true } => {
                        chips.push((Key::Named("tab"), "next", None));
                        chips.push((Key::Named("→"), "accept", None));
                    }
                    JqBarState::Completing { cycle: false } => {
                        chips.push((Key::Named("tab"), "complete", None))
                    }
                    JqBarState::Menu => {
                        chips.push((Key::Named("tab"), "next", None));
                        chips.push((Key::Named("shift+tab"), "prev", None));
                    }
                    JqBarState::Closer => {
                        chips.push((Key::Named("tab"), "accept", None));
                        chips.push((Key::Named("→"), "accept", None));
                    }
                    _ => {}
                }
                // Enter confirms an entered row's chip (and stays in the
                // bar); otherwise it applies — leaves with the edit kept.
                chips.extend([
                    (
                        Key::Named("enter"),
                        if jq_bar == JqBarState::Menu { "select" } else { "apply" },
                        None,
                    ),
                    (Key::Named("esc"), "done", Some(Action::CloseField)),
                    (Key::Of(Action::OpenJqViews), "views", Some(Action::OpenJqViews)),
                    (Key::Of(Action::ToggleJqBar), "unfilter", Some(Action::ToggleJqBar)),
                ]);
                // Only keys. The bar's own buttons with no key (↺ revert,
                // ✕ clear, ✦ describe) get no chip: one would repeat a
                // button already on screen and advertise no key (the user,
                // #102). The palette reaches each of them too.
                chips
            } else if field_open {
                // The jq bar isn't focused, so the open field is the
                // search box: Enter runs the search, Esc leaves with the
                // term kept. Its pair takes the pane — the view chips
                // below are all single letters that would just type.
                vec![
                    (Key::Named("enter"), "search", None),
                    (Key::Named("esc"), "done", Some(Action::CloseField)),
                ]
            } else {
                let mut chips = vec![
                    (tabs_key(), "tabs", Some(Action::CycleTabs(1))),
                    // The search row's key in both profiles (spec §4.6).
                    (
                        Key::Of(Action::OpenResponseSearch { backward: false }),
                        "search",
                        Some(Action::OpenResponseSearch { backward: false }),
                    ),
                    (Key::Of(Action::OpenJqBar), "filter", Some(Action::OpenJqBar)),
                ];
                if jq_bar == JqBarState::Open {
                    chips.push((
                        Key::Of(Action::ToggleJqBar),
                        "unfilter",
                        Some(Action::ToggleJqBar),
                    ));
                }
                // A header row under the cursor: its copy, last so the
                // chips before it never move (spec 2026-10-26 H5). Arrows
                // only: vim's `yy` there is the engine's line yank, so
                // the pair has no first key and the chip drops.
                if table_row_selected == Some(TableCursor::ResponseHeader) {
                    let copy = |whole| Action::CopyResponseHeader { whole };
                    chips.push((Key::Pair(copy(false), copy(true)), "copy", Some(copy(false))));
                }
                chips
            }
        }
    };
    chips
}

/// The tab chord chip every strip shares (editor, response, Manage):
/// both directions spelled, the shared `alt+` merged (`alt+←→`).
pub(crate) fn tabs_key() -> Key {
    Key::Pair(Action::CycleTabs(-1), Action::CycleTabs(1))
}

/// An owned footer chip — keycap kind, label, action — for chip sets
/// built at runtime (a confirm modal's own answer keys, the Variable
/// Manager's name-carrying actions) rather than the static per-pane
/// lists. `draw_footer` spells the keycap (piece 4 spec §4.6).
pub type FooterChip = (Key, String, Option<Action>);

/// What the footer's centre line may say. Both lines take only the room
/// the chips leave: a hovered chip must stay under the pointer, and hover
/// is re-resolved against every frame's hit map. The hint comes first;
/// where there is none, or it doesn't fit, the error does.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CentreLine {
    /// The hovered button's hint (`hint::hint_for`), muted, left out
    /// below [`HINT_MIN_SHOWN`] cells rather than cut to a stub.
    pub hint: Option<String>,
    /// What is wrong with the row under the pointer or the keyboard
    /// cursor (task #59: a header the client owns), in the error colour:
    /// the same message from its longest form to its shortest. The
    /// longest form that fits shows whole; below the shortest, none does.
    pub error: Vec<String>,
}

/// A `paint_chip_row` entry's total footprint: ` key ` pill + ` label `
/// (see its own layout comment — 4 columns beyond key+label either way).
fn chip_width((key, label, _): &(&str, &str, Option<Action>)) -> u16 {
    (crate::ui::cells(key) + crate::ui::cells(label) + 4) as u16
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
    // See `footer_chips`: precisely the URL line, selected (not open).
    url_selected: bool,
    // See `footer_chips`: the stop under the active table's cursor.
    table_row_selected: Option<TableCursor>,
    // See `footer_chips`: where the response pane's jq bar is.
    jq_bar: JqBarState,
    // See `footer_chips`: this pane's own text field owns the caret, so
    // Esc is its.
    field_open: bool,
    // Replaces the per-pane chips wholesale when `Some` — a modal's own
    // chip set, or the Variable Manager screen's
    // (`VarManager::footer_chips`), whose actions target
    // options/declarations rather than the main screen's requests. The
    // right-aligned save/palette/quit chips stay put.
    chips_override: Option<Vec<FooterChip>>,
    // `false` while an open modal supplies the chips: it captures the
    // keyboard, so the palette chip — whose shortcut goes dead under it —
    // hides rather than advertise a key that won't work.
    globals_live: bool,
    // `false` while any dialog is open: no quit runs under one (task
    // #85), so the quit chip hides too.
    quit_live: bool,
    // Whether a plain `q` actually reaches `Action::Quit` right now. Where
    // it doesn't — a non-Main screen, the editor's text areas — the quit
    // chip's keycap shows the pre-empting `^C` combo instead of
    // advertising a key that would just type.
    plain_q_quits: bool,
    // Vim's mode and pending keys (`App::vim_status`): `None` in arrows.
    // Paints at the right end before the commands chip, whole or not at
    // all (piece 4 spec §3.8).
    vim_status: Option<&str>,
    // Where the keyboard is and the live keymap: every chip's keycap is
    // spelled from them (piece 4 spec §4.6).
    spell: crate::keyroute::SpellCtx,
    keymap: &crate::keys::Keymap,
    // The centre line (see `CentreLine`), centred in the gap between the
    // per-pane chips and the commands/quit pair while enough of it fits
    // there (ellipsized).
    centre: &CentreLine,
    hits: &mut HitMap,
    hovered: Option<&Hit>,
    // The shared hover fade's 0->1 progress, so a hovered chip's keycap
    // pill warms up rather than snapping (`paint_chip_row`).
    hover_t: f32,
) {
    let buf = frame.buffer_mut();
    fill(buf, area, theme.panel);

    if area.height == 0 {
        return;
    }
    let mid_y = area.y + area.height / 2;

    use crate::keyroute::spell::{keycap, quit_action, resolve};
    // Each chip's click runs the action its keycap is spelled from (the
    // code review's fifth and sixth findings).
    let quit_key =
        keycap(&Key::Quit { plain_q: plain_q_quits }, spell, keymap).unwrap_or_else(|| "^C".into());
    let quit_chip = (quit_key.as_str(), "quit", Some(quit_action(spell)));
    // Where the quit chip starts: the row's right edge when it is hidden.
    let quit_x = if quit_live {
        let quit_w = chip_width(&quit_chip);
        let quit_x = (area.x + area.width).saturating_sub(quit_w + 1);
        paint_chip_row(
            buf,
            area,
            mid_y,
            quit_x,
            quit_x + quit_w,
            &[quit_chip],
            theme,
            hits,
            hovered,
            hover_t,
        );
        quit_x
    } else {
        area.x + area.width
    };

    let right_limit = if globals_live {
        // The palette chip sits right-aligned, one gap column left of quit.
        // `:` in vim (the ex line is a superset of the palette), `^P` in
        // arrows (plan M4b Deviation 17).
        let (ex, plain) = (Action::OpenExPalette, Action::OpenPalette);
        let palette_key = keycap(&Key::OneOf(ex.clone(), plain.clone()), spell, keymap)
            .unwrap_or_else(|| "^P".into());
        let palette_action = crate::keyroute::spell::one_of(spell, &ex, &plain, keymap).clone();
        let palette_chip = (palette_key.as_str(), "commands", Some(palette_action));
        let palette_w = chip_width(&palette_chip);
        let palette_x = quit_x.saturating_sub(palette_w + 1);
        paint_chip_row(
            buf,
            area,
            mid_y,
            palette_x,
            quit_x,
            &[palette_chip],
            theme,
            hits,
            hovered,
            hover_t,
        );

        // Per-pane chips stop one column shy of the palette so the two
        // never collide. (The save/discard group lives in the app bar now,
        // beside the Theme chip.)
        palette_x.saturating_sub(1)
    } else {
        quit_x.saturating_sub(1)
    };
    // Every chip's keycap is spelled for where the keyboard is; a chip
    // with no key in this profile is dropped. `paint_chip_row` works on
    // borrowed strs, which borrow from `owned`.
    // Vim's Esc abandons the response's search box (arrows' runs it), so
    // there its chip says cancel.
    let vim_search_box = spell.profile == crate::config::KeyProfile::Vim
        && focus == PaneId::Response
        && field_open
        && matches!(jq_bar, JqBarState::Closed | JqBarState::Open);
    let raw: Vec<FooterChip> = match chips_override {
        Some(c) => c,
        None => footer_chips(
            focus,
            shift_enter_send,
            sending,
            add_row_label,
            url_focused,
            url_selected,
            table_row_selected,
            jq_bar,
            field_open,
        )
        .into_iter()
        .map(|(k, l, a)| {
            let l = if vim_search_box && k == Key::Named("esc") { "cancel" } else { l };
            (k, l.to_string(), a)
        })
        .collect(),
    };
    let owned = resolve(raw, spell, keymap);
    let chips: Vec<(&str, &str, Option<Action>)> =
        owned.iter().map(|(k, l, a)| (k.as_str(), l.as_str(), a.clone())).collect();
    let start_x = area.x + 1;
    paint_chip_row(buf, area, mid_y, start_x, right_limit, &chips, theme, hits, hovered, hover_t);
    let chips_end = chip_row_end(start_x, right_limit, &chips);
    // One column shy of `right_limit`: the commands chip's leading pad is
    // painted in the pill's own fill, so anything ending right against it
    // would read as part of the chip.
    let mut hint_end = right_limit.saturating_sub(1);
    // Vim's mode and echo sit at the right end, the way vim echoes to the
    // bottom right. Never truncated: it paints only when it fits whole
    // past the chips, and the hint takes what is left to its left.
    if let Some(status) = vim_status {
        let w = crate::ui::cells(status) as u16;
        if hint_end >= chips_end && hint_end - chips_end >= w {
            let x = hint_end - w;
            text(buf, x, mid_y, status, theme.text_muted, theme.panel, false);
            hint_end = x.saturating_sub(1);
        }
    }
    // The centre line sits in the gap between the chips and the status
    // (or the commands/quit pair), and the chips never yield it more room
    // (see `CentreLine`). A hint too cut down to be useful gives way to
    // the error, which shows its longest form that fits.
    let hint_start = chips_end + 1;
    let free = hint_end.saturating_sub(hint_start);
    match &centre.hint {
        Some(hint) if free >= HINT_MIN_SHOWN => {
            paint_hint(buf, mid_y, hint_start, hint_end, hint, theme.text_muted, theme);
        }
        _ => {
            let fits = centre.error.iter().find(|form| crate::ui::cells(form) <= usize::from(free));
            if let Some(form) = fits {
                paint_hint(buf, mid_y, hint_start, hint_end, form, theme.error, theme);
            }
        }
    }
}

/// The hint shows only while at least this many cells of it fit between
/// the chip clusters (ellipsized past that); with less room than that it
/// is left out, so a hover never reads as a three-letter stub beside an
/// ellipsis.
const HINT_MIN_SHOWN: u16 = 24;

/// Where `paint_chip_row` would stop for `chips` from `start_x` — the same
/// fit rule, without painting — so the footer can decide where the hint
/// goes before anything is drawn.
fn chip_row_end(start_x: u16, right_limit: u16, chips: &[(&str, &str, Option<Action>)]) -> u16 {
    let mut x = start_x;
    for chip in chips {
        let width = chip_width(chip);
        if x + width > right_limit {
            break;
        }
        x += width + 2;
    }
    x
}

/// Paints `hint` centred in the free span `[start_x, end_x)`, in `color`
/// on the panel, ellipsized to fit.
fn paint_hint(
    buf: &mut Buffer,
    y: u16,
    start_x: u16,
    end_x: u16,
    hint: &str,
    color: ratatui::style::Color,
    theme: &Theme,
) {
    let free = end_x.saturating_sub(start_x);
    if free == 0 {
        return;
    }
    let shown = crate::ui::ellipsize(hint, free as usize);
    let w = crate::ui::cells(&shown) as u16;
    let x = start_x + (free - w) / 2;
    text(buf, x, y, &shown, color, theme.panel, false);
}

/// Paints a left-to-right row of `(key, label, action)` chips starting at
/// `start_x` on row `y`, stopping before drawing one that would cross
/// `right_limit`. Each chip with `Some(action)` emphasizes its label: the
/// key combo sits in a small `Chip`-style pill tinted `theme.text_muted`
/// on `theme.control`. Under the mouse (per `hovered`) the pill warms up —
/// its tint source eases toward `theme.text` and its surface toward
/// `theme.control_hover` — over `hover_t`, the shared `AnimKey::Hover` fade
/// the list rows blend through. With the label following in prominent
/// `theme.text` text
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
    // The strip the chips live in (the footer's own rows, the editor's
    // toolbar row). `cap::chip_block` needs it to know whether there is
    // room above and below `y` for the chips' caps.
    bounds: Rect,
    y: u16,
    start_x: u16,
    right_limit: u16,
    chips: &[(&str, &str, Option<Action>)],
    theme: &Theme,
    hits: &mut HitMap,
    hovered: Option<&Hit>,
    // The 0->1 progress of the current hover fade (`DrawCtx::hover_t`).
    // Only the hovered chip reads it; everything else rests regardless.
    hover_t: f32,
) -> u16 {
    let mut x = start_x;
    for chip @ (key, label, action) in chips {
        // Total footprint is the same either way — 4 extra columns beyond
        // key+label — so layout math never has to branch on `action`: a
        // clickable chip is `" key "` (the tinted pill, +2) + `" label "`,
        // and a plain entry is `" key"` + `" label "` with one trailing pad
        // column making up the difference.
        let width = chip_width(chip);
        if x + width > right_limit {
            break;
        }
        match action {
            Some(a) => {
                // A hovered chip warms up through the shared keycap funnel
                // (`paint::keycap_face`): tint source toward `theme.text`,
                // surface toward `control_hover`, both on `hover_t`.
                // The very same composite button the app bar's Theme and
                // Manage are, painted by the very same function: keycap and
                // word under one hit, warming together. The `" label "`
                // padding here is what makes the two agree on width —
                // keycap + label + 4 either way.
                let block = crate::paint::keycap_button(
                    buf,
                    bounds,
                    x,
                    y,
                    key,
                    &format!(" {label} "),
                    hovered == Some(&Hit::FooterChip(a.clone())),
                    hover_t,
                    theme,
                );
                debug_assert_eq!(block.width, width, "layout and paint agree");
                hits.register(block, Hit::FooterChip(a.clone()));
            }
            None => {
                let key_text = format!(" {key}");
                let label_text = format!(" {label}  ");
                let label_x = text(buf, x, y, &key_text, theme.text_muted, theme.panel, true);
                text(buf, label_x, y, &label_text, theme.text, theme.panel, false);
            }
        }
        x += width + 2;
    }
    x
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::display_keycap;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    /// The content row of a chip's hit rect. A chip's hit is its whole
    /// capped block (see `cap::CHIP_CAPS`), so the rect's own `y` is the
    /// sliver above the chip, not the row its keycap and label sit on.
    /// Derived from the block's height, so it stays right when the constant
    /// is flipped or a cramped strip degrades a chip to one row.
    fn mid_of(r: &Rect) -> u16 {
        r.y + r.height / 2
    }

    /// The spelling context of the sidebar: a verb chip is spelled from
    /// the row on its pane's surface.
    fn sidebar_spell() -> crate::keyroute::SpellCtx {
        crate::keyroute::SpellCtx {
            surface: crate::keyroute::Surface::Sidebar,
            ..Default::default()
        }
    }

    /// `super::footer_chips` as the arrows profile paints it: these tests
    /// read the keycaps as text. A verb chip (`a`, `d`) is spelled from the
    /// row on its pane's surface: the sidebar, or the editor's table.
    #[allow(clippy::too_many_arguments)]
    fn footer_chips(
        focus: PaneId,
        shift_enter_send: bool,
        sending: bool,
        add_row_label: Option<&'static str>,
        url_focused: bool,
        url_selected: bool,
        table_row_selected: Option<(usize, bool)>,
        jq_bar: JqBarState,
        field_open: bool,
    ) -> Vec<(String, &'static str, Option<Action>)> {
        let surface = match focus {
            PaneId::Sidebar => crate::keyroute::Surface::Sidebar,
            _ => crate::keyroute::Surface::Table(crate::components::editor::EditorTab::Headers),
        };
        crate::keyroute::spell::spelled_on(
            surface,
            super::footer_chips(
                focus,
                shift_enter_send,
                sending,
                add_row_label,
                url_focused,
                url_selected,
                table_row_selected.map(|(index, enabled)| TableCursor::Row {
                    index,
                    enabled,
                    headers: true,
                }),
                jq_bar,
                field_open,
            ),
        )
    }

    /// A footer chip is the same composite button Theme and Manage are — a
    /// keycap and a name under one hit — so it hovers the same way: the
    /// name's ground lifts with the keycap, on one clock. Lifting only the
    /// keycap made the button come apart under the pointer.
    #[test]
    fn a_hovered_footer_chip_lifts_its_word_with_its_keycap() {
        let theme = Theme::for_terminal();
        let hovered = Hit::FooterChip(Action::Send);
        let mut terminal = Terminal::new(TestBackend::new(60, 3)).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal
            .draw(|f| {
                let area = f.area();
                crate::paint::fill(f.buffer_mut(), area, theme.panel);
                paint_chip_row(
                    f.buffer_mut(),
                    area,
                    1,
                    0,
                    60,
                    &[("^R", "send", Some(Action::Send))],
                    &theme,
                    &mut hits,
                    Some(&hovered),
                    0.5,
                );
            })
            .unwrap();

        let buf = terminal.backend().buffer();
        let lifted = crate::paint::hover_surface(&theme, theme.panel, true, 0.5);
        assert_ne!(lifted, theme.panel, "fixture: the lift is visible at all");

        // " ^R " is the pill; the word " send " follows it.
        let word_x = 4;
        assert_eq!(buf[(word_x, 1)].symbol(), " ", "the pad before the word");
        assert_eq!(buf[(word_x + 1, 1)].symbol(), "s", "fixture: the word starts here");
        assert_eq!(
            buf[(word_x + 1, 1)].bg,
            lifted,
            "the word's ground lifts with the keycap, halfway at t=0.5"
        );
    }

    /// An error never takes room from the chips (task #59, the user: "just
    /// make it shorter when there is less space so we dont have to hide
    /// the chips"): in a narrower gap it shows its longest form that fits
    /// whole, in the error colour, and below its shortest, nothing.
    #[test]
    fn an_error_shortens_to_fit_and_never_cuts_a_chip() {
        let theme = Theme::for_terminal();
        let keymap = crate::keys::Keymap::default_bindings();
        let draw = |width: u16, centre: &CentreLine| {
            let mut terminal = Terminal::new(TestBackend::new(width, FOOTER_HEIGHT)).unwrap();
            let mut hits = crate::hit::HitMap::default();
            let chips: Vec<FooterChip> = vec![
                (Key::Named("esc"), "cancel drag".into(), None),
                (Key::Named("enter"), "drop here".into(), None),
            ];
            terminal
                .draw(|f| {
                    draw_footer(
                        f,
                        f.area(),
                        &theme,
                        PaneId::Sidebar,
                        false,
                        false,
                        None,
                        false,
                        false,
                        None,
                        JqBarState::Closed,
                        false,
                        Some(chips),
                        true,
                        true,
                        true,
                        None,
                        sidebar_spell(),
                        &keymap,
                        centre,
                        &mut hits,
                        None,
                        1.0,
                    )
                })
                .unwrap();
            format!("{:?}", terminal.backend().buffer())
        };
        let forms = ["Content-Length cannot be set manually", "Content-Length can't be set"];
        let error = CentreLine {
            hint: None,
            error: forms
                .iter()
                .map(|f| f.to_string())
                .chain(["can't be set".to_string()])
                .collect(),
        };
        // The chips end near column 42 and the commands/quit pair takes
        // the last ~28, so the gap is about `width - 70` cells.
        for (width, shown) in
            [(130, Some(forms[0])), (100, Some(forms[1])), (86, Some("can't be set")), (70, None)]
        {
            let content = draw(width, &error);
            assert!(content.contains("cancel drag"), "{width}: {content}");
            assert!(content.contains("drop here"), "{width}: {content}");
            match shown {
                Some(form) => {
                    assert!(content.contains(form), "{width}: {content}");
                    assert!(!content.contains('…'), "shown whole, {width}: {content}");
                }
                None => assert!(!content.contains("be set"), "{width}: {content}"),
            }
        }
        assert!(!draw(100, &error).contains(forms[0]), "the longer form gave way");

        // A hint has the line where it fits; where it doesn't (a gap under
        // its minimum), the error still shows rather than nothing.
        let both = CentreLine { hint: Some("Delete this row".into()), ..error.clone() };
        let wide = draw(130, &both);
        assert!(wide.contains("Delete this row") && !wide.contains("be set"), "{wide}");
        let narrow = draw(86, &both);
        assert!(narrow.contains("can't be set") && !narrow.contains("Delete"), "{narrow}");
    }

    /// The footer's chips are the same controls as the app bar's, so they
    /// carry the same caps and register the same block — otherwise the bar
    /// stands 1.25 rows and the footer reads as a thinner, lesser row of
    /// the same buttons.
    ///
    /// Written against `CHIP_CAPS` rather than a literal height so it holds
    /// whichever way the constant is set.
    #[test]
    fn a_footer_chip_stands_and_hits_as_tall_as_every_other_chip() {
        use crate::paint::cap::{CHIP_CAPS, ChipCaps};
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
                    false,
                    None,
                    JqBarState::Closed,
                    false,
                    None,
                    true,
                    true,
                    true,
                    None,
                    sidebar_spell(),
                    &crate::keys::Keymap::default_bindings(),
                    &CentreLine::default(),
                    &mut hits,
                    None,
                    1.0,
                )
            })
            .unwrap();

        let rect =
            hits.rect_of(&Hit::FooterChip(Action::PromptNewRequest)).expect("new-request chip hit");
        assert_eq!(
            rect.height,
            CHIP_CAPS.height(),
            "the footer chip's hit matches the app bar's chips"
        );

        let buf = terminal.backend().buffer();
        let mid = rect.y + rect.height / 2;
        let pill = theme.tint(theme.text_muted, theme.control);
        assert_eq!(buf[(rect.x + 1, mid)].bg, pill, "the keycap fill");

        if CHIP_CAPS == ChipCaps::Sliver {
            let top = &buf[(rect.x + 1, rect.y)];
            assert_eq!(top.symbol(), crate::paint::cap::SLIVER_TOP);
            assert_eq!(top.fg, pill, "the sliver carries the keycap's own fill");
            let bottom = &buf[(rect.x + 1, rect.y + 2)];
            assert_eq!(bottom.symbol(), crate::paint::cap::SLIVER_BOTTOM);
            assert_eq!(bottom.fg, pill);
        }
    }

    /// A hovered chip's keycap pill warms up over the shared hover fade,
    /// through the same `paint::keycap_face` funnel every other keycap in
    /// the app uses. Tinting alone moves the fill by only ~2 RGB points --
    /// `tint` blends just 22% of the surface in -- so the *tint source* has
    /// to move for the hover to be visible at all.
    #[test]
    fn a_hovered_chips_pill_warms_through_the_shared_keycap_funnel() {
        let theme = Theme::for_terminal();
        assert_ne!(
            theme.control, theme.control_hover,
            "fixture: the two surfaces must differ for the blend to be visible"
        );
        let chips = [("^R", "send", Some(Action::Send))];
        let hovered = Hit::FooterChip(Action::Send);

        let pill_bg = |hover_t: f32| {
            let mut terminal = Terminal::new(TestBackend::new(60, 1)).unwrap();
            let mut hits = crate::hit::HitMap::default();
            terminal
                .draw(|f| {
                    let area = f.area();
                    paint_chip_row(
                        f.buffer_mut(),
                        area,
                        0,
                        0,
                        60,
                        &chips,
                        &theme,
                        &mut hits,
                        Some(&hovered),
                        hover_t,
                    );
                })
                .unwrap();
            terminal.backend().buffer()[(1, 0)].bg
        };

        let face = |t: f32| {
            let (color, on) = crate::paint::keycap_face(&theme, true, t);
            theme.tint(color, on)
        };
        assert_eq!(
            pill_bg(0.0),
            theme.tint(theme.text_muted, theme.control),
            "the fade starts from the resting pill exactly"
        );
        assert_eq!(pill_bg(0.5), face(0.5), "mid-fade it is halfway warmed");
        assert_eq!(pill_bg(1.0), face(1.0), "and it lands fully warmed");

        // The point of the whole change: the landed hover must actually be
        // visible. The old surface-only lift moved the fill by ~0.008.
        let l = |c| crate::theme::oklab_l(crate::theme::rgb_of(c));
        let delta = (l(pill_bg(1.0)) - l(pill_bg(0.0))).abs();
        assert!(delta > 0.05, "a hovered pill must be visibly different, not {delta:.4} apart");
    }

    /// The fade belongs to the chip under the pointer. An unhovered chip
    /// keeps its resting surface no matter where the fade has got to.
    #[test]
    fn an_unhovered_chip_ignores_the_hover_fade() {
        let theme = Theme::for_terminal();
        let mut terminal = Terminal::new(TestBackend::new(60, 1)).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal
            .draw(|f| {
                let area = f.area();
                paint_chip_row(
                    f.buffer_mut(),
                    area,
                    0,
                    0,
                    60,
                    &[("^R", "send", Some(Action::Send))],
                    &theme,
                    &mut hits,
                    None,
                    0.5,
                );
            })
            .unwrap();
        assert_eq!(
            terminal.backend().buffer()[(1, 0)].bg,
            theme.tint(theme.text_muted, theme.control),
            "no hover, no lift"
        );
    }

    #[test]
    fn a_completion_ghost_advertises_tab_and_accept() {
        let chips = |state: JqBarState| {
            footer_chips(PaneId::Response, false, false, None, false, false, None, state, false)
                .into_iter()
                .map(|(k, l, _)| format!("{k} {l}"))
                .collect::<Vec<_>>()
        };
        let focused = chips(JqBarState::Focused);
        assert!(!focused.iter().any(|c| c.starts_with("tab")), "{focused:?}");
        let cycle = chips(JqBarState::Completing { cycle: true });
        assert_eq!(&cycle[..2], &["tab next".to_string(), "→ accept".to_string()]);
        assert_eq!(&cycle[2..], &focused[..], "the usual chips follow");
        let menu_ghost = chips(JqBarState::Completing { cycle: false });
        assert_eq!(menu_ghost[0], "tab complete");
        assert_eq!(&menu_ghost[1..], &focused[..]);
        let menu = chips(JqBarState::Menu);
        assert_eq!(
            &menu[..3],
            &["tab next".to_string(), "shift+tab prev".to_string(), "enter select".to_string()]
        );
        assert_eq!(&menu[3..], &focused[1..], "enter reads select, not apply");
        let closer = chips(JqBarState::Closer);
        assert_eq!(&closer[..2], &["tab accept".to_string(), "→ accept".to_string()]);
        assert_eq!(&closer[2..], &focused[..]);
    }

    /// The focused bar offers the views dropdown right after its own
    /// apply and done (jq views spec 16).
    #[test]
    fn the_focused_bar_advertises_views_in_the_footer() {
        let chips = footer_chips(
            PaneId::Response,
            false,
            false,
            None,
            false,
            false,
            None,
            JqBarState::Focused,
            true,
        );
        let labels: Vec<&str> = chips.iter().map(|(_, l, _)| *l).collect();
        assert_eq!(&labels[..3], &["apply", "done", "views"], "{chips:?}");
        assert_eq!(chips[2].0, display_keycap("alt+shift+j"));
        assert_eq!(chips[2].2, Some(Action::OpenJqViews));
    }

    /// The focused bar's footer lists keys only: its keyless buttons (↺
    /// revert, ✕ clear, ✦ describe) are on the bar and get no chip (the
    /// user, #102).
    #[test]
    fn the_focused_bar_advertises_keys_only() {
        let chips = super::footer_chips(
            PaneId::Response,
            false,
            false,
            None,
            false,
            false,
            None,
            JqBarState::Focused,
            true,
        );
        let names: Vec<&str> = chips.iter().map(|(_, l, _)| *l).collect();
        assert_eq!(names, ["apply", "done", "views", "unfilter"]);
    }

    /// With an AI describe outstanding the bar's Esc cancels the request,
    /// so the chip names that action — never a `done` that would abort it.
    #[test]
    fn a_pending_describe_advertises_its_cancel_instead_of_done() {
        let chips = footer_chips(
            PaneId::Response,
            false,
            false,
            None,
            false,
            false,
            None,
            JqBarState::Describing,
            true,
        );
        assert_eq!(
            chips,
            vec![("esc".to_string(), "cancel describe", Some(Action::CancelJqDescribe))]
        );
    }

    fn render(focus: PaneId) -> String {
        let (content, _) = render_hits(focus);
        content
    }

    fn render_hits(focus: PaneId) -> (String, crate::hit::HitMap) {
        render_hits_on(focus, crate::keyroute::SpellCtx::default())
    }

    /// The footer drawn as the app draws it: with the focused surface's
    /// spelling context (a row-spelled chip only shows where its row binds).
    fn render_hits_on(
        focus: PaneId,
        spell: crate::keyroute::SpellCtx,
    ) -> (String, crate::hit::HitMap) {
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
                    false,
                    None,
                    JqBarState::Closed,
                    false,
                    None,
                    true,
                    true,
                    true,
                    None,
                    spell,
                    &crate::keys::Keymap::default_bindings(),
                    &CentreLine::default(),
                    &mut hits,
                    None,
                    1.0,
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
            false,
            None,
            JqBarState::Closed,
            false,
        );
        assert!(with.iter().any(|(k, l, _)| *k == "⇧enter" && *l == "send"));
        let without = footer_chips(
            PaneId::Editor,
            false,
            false,
            Some("add header"),
            false,
            false,
            None,
            JqBarState::Closed,
            false,
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
            false,
            None,
            JqBarState::Closed,
            false,
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

    /// An open text field owns Esc (the field rule, spec 2026-09-15): the
    /// footer says `esc done` and never `esc cancel` on that surface —
    /// sending or not.
    #[test]
    fn an_open_field_advertises_esc_done_and_never_esc_cancel() {
        let chips = footer_chips(
            PaneId::Editor,
            false,
            true,
            Some("add header"),
            true,
            false,
            None,
            JqBarState::Closed,
            true,
        );
        assert!(
            chips
                .iter()
                .any(|(k, l, a)| *k == "esc" && *l == "done" && *a == Some(Action::CloseField))
        );
        assert!(
            !chips.iter().any(|(k, l, _)| *k == "esc" && *l == "cancel"),
            "sending or not, an open field owns Esc"
        );
        assert!(
            !chips.iter().any(|(_, l, _)| *l == "send"),
            "the send keys are dead in flight: {chips:?}"
        );
        let chips = footer_chips(
            PaneId::Response,
            false,
            false,
            None,
            false,
            false,
            None,
            JqBarState::Focused,
            true,
        );
        assert!(
            chips
                .iter()
                .any(|(k, l, a)| *k == "esc" && *l == "done" && *a == Some(Action::CloseField))
        );
        // The search box is a field too, and the jq bar is not focused
        // while it is: its own pair takes the pane.
        let chips = footer_chips(
            PaneId::Response,
            false,
            false,
            None,
            false,
            false,
            None,
            JqBarState::Closed,
            true,
        );
        assert_eq!(
            chips.iter().map(|(k, l, _)| format!("{k} {l}")).collect::<Vec<_>>(),
            vec!["enter search".to_string(), "esc done".to_string()]
        );
    }

    /// The save/discard group lives in the app bar now (beside the Theme
    /// chip): the footer must not carry it on any pane.
    #[test]
    fn footer_carries_no_save_or_discard_chips() {
        for focus in [PaneId::Sidebar, PaneId::Editor, PaneId::Response] {
            let (_, hits) = render_hits(focus);
            assert!(
                hits.rect_of(&Hit::FooterChip(Action::SaveRequest)).is_none(),
                "{focus:?}: save moved to the app bar"
            );
            assert!(
                hits.rect_of(&Hit::FooterChip(Action::DiscardChanges)).is_none(),
                "{focus:?}: discard moved to the app bar"
            );
        }
    }

    /// The editor's context chips advertise vars (alt+v — ctrl+v is
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
            false,
            None,
            JqBarState::Closed,
            false,
        );
        assert!(chips.iter().any(|(k, l, a)| *k == display_keycap("alt+v")
            && *l == "vars"
            && *a == Some(Action::OpenVarPicker { completing: false })));
        assert!(
            !chips.iter().any(|(_, _, a)| *a == Some(Action::SaveRequest)),
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
            false,
            None,
            JqBarState::Closed,
            false,
        );
        assert!(
            !chips.iter().any(|(_, _, a)| *a == Some(Action::TableAddRow)),
            "add-row is inert while the address bar is focused"
        );
        assert!(chips.iter().any(|(k, l, a)| *k == display_keycap("alt+y")
            && *l == "copy url"
            && *a == Some(Action::CopyToClipboard(crate::action::CopyTarget::Url))));
        assert!(chips.iter().any(|(k, l, a)| *k == display_keycap("alt+i")
            && *l == "tls"
            && *a == Some(Action::ToggleInsecure)));

        let chips = footer_chips(
            PaneId::Editor,
            false,
            false,
            Some("add header"),
            false,
            false,
            None,
            JqBarState::Closed,
            false,
        );
        assert!(
            chips.iter().any(|(_, _, a)| *a == Some(Action::TableAddRow)),
            "content focus keeps the add-row chip"
        );
        assert!(
            !chips.iter().any(|(_, _, a)| *a == Some(Action::ToggleInsecure)),
            "address-bar chips stay off the content-focus footer"
        );
    }

    /// The URL line, selected but not open (no caret live), advertises
    /// the field rule's other half: Enter opens it. `Action::FocusUrl`
    /// both focuses the address bar and opens it, so it doubles as this
    /// chip's action.
    #[test]
    fn a_selected_url_line_advertises_enter_edit() {
        let chips = footer_chips(
            PaneId::Editor,
            false,
            false,
            None,
            true,
            true,
            None,
            JqBarState::Closed,
            false,
        );
        assert!(
            chips
                .iter()
                .any(|(k, l, a)| *k == "enter" && *l == "edit" && *a == Some(Action::FocusUrl))
        );
    }

    /// The method badge sits in the same address-bar area as the URL line
    /// (`url_focused` covers both), but its Enter opens the method
    /// dropdown, not `Action::FocusUrl` — so the "enter edit" chip must not
    /// appear there (final-review finding 3: the chip must name the
    /// action actually dispatched).
    #[test]
    fn the_method_badge_does_not_advertise_enter_edit() {
        let chips = footer_chips(
            PaneId::Editor,
            false,
            false,
            None,
            true,
            false,
            None,
            JqBarState::Closed,
            false,
        );
        assert!(
            !chips.iter().any(|(_, l, a)| *l == "edit" && *a == Some(Action::FocusUrl)),
            "the method badge must not advertise the URL line's enter-edit chip: {chips:?}"
        );
    }

    /// No footer carries a width chip, in either profile (spec S6, S7):
    /// footers carry actions on the focused area's content, not window
    /// keys. The sidebar's width keys and the split's `alt+w` are found in
    /// the palette (S6b, E12a).
    #[test]
    fn no_footer_carries_a_width_chip() {
        use crate::config::KeyProfile;
        use crate::keyroute::Surface;
        let sizes = |a: &Option<Action>| {
            matches!(
                a,
                Some(
                    Action::StepSidebar(_)
                        | Action::PaneResize { .. }
                        | Action::CycleSplit
                        | Action::CycleSplitBack
                )
            )
        };
        for (pane, surface) in [
            (PaneId::Sidebar, Surface::Sidebar),
            (PaneId::Editor, Surface::UrlLine),
            (PaneId::Response, Surface::Response(crate::components::response::ViewMode::Pretty)),
        ] {
            for url_focused in [false, true] {
                let chips = super::footer_chips(
                    pane,
                    false,
                    false,
                    Some("add header"),
                    url_focused,
                    false,
                    None,
                    JqBarState::Closed,
                    false,
                );
                let at = format!("{pane:?} url_focused={url_focused}");
                assert!(
                    !chips.iter().any(|(k, l, a)| *l == "width"
                        || sizes(a)
                        || matches!(
                            k,
                            Key::Of(Action::CycleSplit | Action::PaneResize { .. })
                                | Key::OneOf(_, Action::CycleSplit | Action::PaneResize { .. })
                                | Key::OneOf(Action::CycleSplit | Action::PaneResize { .. }, _)
                        )),
                    "{at}: {chips:?}"
                );
                for profile in [KeyProfile::Arrows, KeyProfile::Vim] {
                    let painted = crate::keyroute::spell::resolve(
                        chips.clone(),
                        crate::keyroute::SpellCtx { profile, surface, ..Default::default() },
                        &crate::keys::Keymap::default_bindings_for(profile),
                    );
                    assert!(
                        !painted.iter().any(|(k, l, _)| *l == "width"
                            || k.ends_with("+w")
                            || k.starts_with("^W")
                            || k.starts_with(":split")),
                        "{at} {profile:?}: {painted:?}"
                    );
                }
            }
        }
    }

    /// A selected data row advertises its own quick actions — the
    /// keyboard twins of the expanded row's ● toggle and 󰆴 delete
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
            false,
            Some((2, true)),
            JqBarState::Closed,
            false,
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
            false,
            Some((2, false)),
            JqBarState::Closed,
            false,
        );
        assert!(
            chips.iter().any(|(_, l, a)| *l == "enable" && *a == Some(Action::ToggleTableRow(2)))
        );

        let chips = footer_chips(
            PaneId::Editor,
            false,
            false,
            Some("add header"),
            false,
            false,
            None,
            JqBarState::Closed,
            false,
        );
        assert!(
            !chips.iter().any(|(_, l, _)| *l == "disable" || *l == "enable" || *l == "delete"),
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
        // The same tab chord chip as the response pane and every Manage
        // tab: one wording wherever a strip is on screen.
        assert!(content.contains(&format!("{}  tabs", display_keycap("alt+←→"))), "{content}");
    }

    /// The spelling context while the response is focused.
    fn response_spell() -> crate::keyroute::SpellCtx {
        crate::keyroute::SpellCtx {
            surface: crate::keyroute::Surface::Response(
                crate::components::response::ViewMode::Pretty,
            ),
            ..Default::default()
        }
    }

    #[test]
    fn response_focus_shows_response_hints() {
        let (content, _) = render_hits_on(PaneId::Response, response_spell());
        assert!(content.contains(&format!("{}  tabs", display_keycap("alt+←→"))), "{content}");
        assert!(!content.contains("t  view"), "{content}");
        assert!(content.contains("/  search"));
    }

    /// Task 17, spec §5: the Response pane's `t`/`/` chips used to be
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
                    false,
                    None,
                    JqBarState::Closed,
                    false,
                    None,
                    true,
                    true,
                    true,
                    None,
                    response_spell(),
                    &crate::keys::Keymap::default_bindings(),
                    &CentreLine::default(),
                    &mut hits,
                    None,
                    1.0,
                )
            })
            .unwrap();
        assert!(hits.rect_of(&Hit::FooterChip(Action::CycleTabs(1))).is_some());
        assert!(
            hits.rect_of(&Hit::FooterChip(Action::OpenResponseSearch { backward: false }))
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
                    false,
                    None,
                    JqBarState::Closed,
                    false,
                    None,
                    true,
                    true,
                    true,
                    None,
                    sidebar_spell(),
                    &crate::keys::Keymap::default_bindings(),
                    &CentreLine::default(),
                    &mut hits,
                    None,
                    1.0,
                )
            })
            .unwrap();
        assert!(hits.rect_of(&Hit::FooterChip(Action::PromptNewRequest)).is_some());
        // The palette chip is right-aligned: after the last per-pane chip,
        // one gap column before the quit hint.
        let palette =
            hits.rect_of(&Hit::FooterChip(Action::OpenPalette)).expect("palette chip registered");
        let quit = hits.rect_of(&Hit::FooterChip(Action::Quit)).unwrap();
        let delete = hits.rect_of(&Hit::FooterChip(Action::DeleteSelectedRequest)).unwrap();
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
                    false,
                    None,
                    JqBarState::Closed,
                    false,
                    None,
                    true,
                    true,
                    true,
                    None,
                    crate::keyroute::SpellCtx::default(),
                    &crate::keys::Keymap::default_bindings(),
                    &CentreLine::default(),
                    &mut hits,
                    None,
                    1.0,
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
                    false,
                    None,
                    JqBarState::Closed,
                    false,
                    None,
                    true,
                    true,
                    true,
                    None,
                    sidebar_spell(),
                    &crate::keys::Keymap::default_bindings(),
                    &CentreLine::default(),
                    &mut hits,
                    None,
                    1.0,
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
        let key_cell = buf.cell((rect.x + 1, mid_of(&rect))).unwrap();
        assert_eq!(key_cell.symbol(), "a");
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
        let label_cell = buf.cell((rect.x + 4, mid_of(&rect))).unwrap();
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
                    false,
                    None,
                    JqBarState::Closed,
                    false,
                    None,
                    true,
                    true,
                    true,
                    None,
                    sidebar_spell(),
                    &crate::keys::Keymap::default_bindings(),
                    &CentreLine::default(),
                    &mut hits,
                    None,
                    1.0,
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
        assert_eq!(plain_cell.bg, theme.panel, "non-clickable entry is unfilled");
        assert_eq!(plain_cell.fg, theme.text_muted);

        // "a new" (PromptNewRequest) IS clickable: tinted key pill.
        let rect = hits
            .rect_of(&Hit::FooterChip(Action::PromptNewRequest))
            .expect("new-request chip hit registered");
        let chip_cell = buf.cell((rect.x + 1, mid_of(&rect))).unwrap();
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
                    false,
                    None,
                    JqBarState::Closed,
                    false,
                    None,
                    true,
                    true,
                    true,
                    None,
                    crate::keyroute::SpellCtx::default(),
                    &crate::keys::Keymap::default_bindings(),
                    &CentreLine::default(),
                    &mut hits,
                    None,
                    1.0,
                )
            })
            .unwrap();
        let rect = hits.rect_of(&Hit::FooterChip(Action::Quit)).expect("quit hit registered");
        assert!(rect.x + rect.width >= 119, "quit chip is right-aligned: {rect:?}");
        let buf = terminal.backend().buffer();
        let cell = buf.cell((rect.x + 1, mid_of(&rect))).unwrap();
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
                let area = f.area();
                paint_chip_row(
                    f.buffer_mut(),
                    area,
                    0,
                    0,
                    60,
                    &[("^R", "send", Some(Action::Send)), ("g", "top", None)],
                    &theme,
                    &mut hits,
                    None,
                    1.0,
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
                    false,
                    None,
                    JqBarState::Closed,
                    false,
                    None,
                    true,
                    true,
                    true,
                    None,
                    crate::keyroute::SpellCtx::default(),
                    &crate::keys::Keymap::default_bindings(),
                    &CentreLine::default(),
                    &mut hits,
                    None,
                    1.0,
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

    /// The status paints whole, just left of the commands chip, or not at
    /// all: never truncated (spec §3.8).
    #[test]
    fn the_vim_status_paints_whole_or_not_at_all() {
        let render = |width: u16, status: Option<&str>| -> String {
            let theme = Theme::for_terminal();
            let mut terminal = Terminal::new(TestBackend::new(width, FOOTER_HEIGHT)).unwrap();
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
                        None,
                        false,
                        false,
                        None,
                        JqBarState::Closed,
                        false,
                        None,
                        true,
                        true,
                        true,
                        status,
                        crate::keyroute::SpellCtx::default(),
                        &crate::keys::Keymap::default_bindings(),
                        &CentreLine::default(),
                        &mut hits,
                        None,
                        1.0,
                    )
                })
                .unwrap();
            let buf = terminal.backend().buffer().clone();
            (0..buf.area.width).map(|x| buf[(x, 1)].symbol().to_string()).collect()
        };
        assert!(render(160, Some("NORMAL  3")).contains("NORMAL  3"));
        assert!(!render(160, None).contains("NORMAL"));
        assert!(!render(40, Some("NORMAL  3")).contains("NORMAL"), "no room, no status");
    }
}

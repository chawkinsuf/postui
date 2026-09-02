//! The Manage screen's shell: which tab is up, and the top bar shared by
//! every tab (tab strip left, Close right, the Variables tab's own "new"
//! buttons between). Tab bodies are drawn by `ui.rs` — `VarManager` for
//! Variables, `ManageList` for Environments and Spaces.

use crate::action::Action;
use crate::hit::{Hit, HitMap};
use crate::paint::{
    BUTTON_HEIGHT, Button, ButtonKind, ControlState, TabStrip, button_min_width, fill,
};
use crate::theme::Theme;
use ratatui::Frame;
use ratatui::layout::Rect;

/// One tab of the Manage screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ManageTab {
    #[default]
    Variables,
    Environments,
    Spaces,
}

impl ManageTab {
    /// Every tab, in on-screen order — the order `index`/`from_index`/
    /// `cycle` and the tab strip all read from.
    pub const ALL: [ManageTab; 3] = [
        ManageTab::Variables,
        ManageTab::Environments,
        ManageTab::Spaces,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ManageTab::Variables => "Variables",
            ManageTab::Environments => "Environments",
            ManageTab::Spaces => "Spaces",
        }
    }

    pub fn index(self) -> usize {
        Self::ALL
            .iter()
            .position(|t| *t == self)
            .expect("ALL lists every tab")
    }

    pub fn from_index(i: usize) -> Self {
        Self::ALL[i.min(Self::ALL.len() - 1)]
    }

    /// Each tab's `(x, width)` span relative to the strip's origin — the
    /// geometry `draw_manage_bar` lays the strip out with, exposed so the
    /// app can glide the underline between them.
    pub fn strip_spans() -> Vec<(u16, u16)> {
        let tabs: Vec<(String, Option<(char, ratatui::style::Color)>)> = Self::ALL
            .iter()
            .map(|t| (t.label().to_string(), None))
            .collect();
        TabStrip::spans(&tabs)
    }

    /// Steps `delta` tabs along `ALL`, wrapping in both directions.
    pub fn cycle(self, delta: i32) -> Self {
        let n = Self::ALL.len() as i32;
        Self::from_index((self.index() as i32 + delta).rem_euclid(n) as usize)
    }
}

/// The Manage screen's own state: which tab is up, plus the list-edit
/// body the Environments and Spaces tabs share. The Variables tab's body
/// keeps its state elsewhere (`App::varmanager`).
#[derive(Default)]
pub struct Manage {
    pub tab: ManageTab,
    /// The Environments/Spaces tabs' shared list-edit body.
    pub list: crate::components::manage_list::ManageList,
}

/// The bar's height: the Variables tab's buttons are `BUTTON_HEIGHT` tall
/// and the tab strip needs two rows (label + underline) inside that.
pub const BAR_HEIGHT: u16 = BUTTON_HEIGHT;

/// Paints the top bar: tab strip at the left edge (registering
/// `Hit::ManageTab(i)`), `Close (esc)` at the right, and on the Variables
/// tab the `+ Variable` / `+ Selector` buttons before it. `underline` is
/// the accent segment's `(left, width)` in fractional columns relative to
/// the strip's origin — the app's eased edges mid-glide — or `None` for
/// the active tab's own static span.
///
/// The strip has priority for its natural width: the right-aligned
/// buttons lay out only into the room left of it, and a button that would
/// cross into the strip is dropped rather than painted over it — lowest
/// priority (leftmost) first, so `+ Selector` goes before `+ Variable`
/// and `Close (esc)` is the last to go. Dropped buttons stay reachable by
/// key (`n`, `g`, `esc`) and through the footer chips, so nothing is lost.
pub fn draw_manage_bar(
    frame: &mut Frame,
    bar: Rect,
    theme: &Theme,
    tab: ManageTab,
    underline: Option<(f32, f32)>,
    hits: &mut HitMap,
    hovered: Option<&Hit>,
) {
    let buf = frame.buffer_mut();
    fill(buf, bar, theme.panel);
    if bar.height < BAR_HEIGHT || bar.width < 8 {
        return;
    }
    let state_of = |hit: &Hit| {
        if hovered == Some(hit) {
            ControlState::Hover
        } else {
            ControlState::Normal
        }
    };

    let left_edge = bar.x + 1;

    // The strip's natural width, measured before anything is laid out:
    // the buttons are fitted into what is left of the bar beyond it.
    let tabs: Vec<(String, Option<(char, ratatui::style::Color)>)> = ManageTab::ALL
        .iter()
        .map(|t| (t.label().to_string(), None))
        .collect();
    let spans = TabStrip::spans(&tabs);
    let strip_w = spans
        .last()
        .map_or(0, |(x, w)| x + w)
        .min(bar.width.saturating_sub(2));
    // No button may start left of here — a 2-column breathing gap after
    // the strip's last tab.
    let buttons_limit = left_edge + 1 + strip_w + 2;

    // Right-aligned buttons, laid out from the right edge inward in
    // keep-priority order, so the loop's `break` drops the least
    // important first. The close button is the mouse's way back to the
    // main screen (the header's Manage chip toggles it too), labelled
    // with the key that does the same thing, so it is the last to go.
    let mut x = bar.x + bar.width;
    let mut buttons: Vec<(&str, ButtonKind, Hit)> = vec![(
        "Close (esc)",
        ButtonKind::Secondary,
        Hit::FooterChip(Action::CloseScreen),
    )];
    if tab == ManageTab::Variables {
        buttons.push(("+ Variable", ButtonKind::Primary, Hit::VmNewVar));
        buttons.push(("+ Selector", ButtonKind::Secondary, Hit::VmNewSelector));
    }
    for (label, kind, hit) in buttons {
        let w = button_min_width(label);
        if x < buttons_limit + w + 1 {
            break;
        }
        x -= w + 1;
        let rect = Rect {
            x,
            y: bar.y,
            width: w,
            height: BUTTON_HEIGHT,
        };
        let state = state_of(&hit);
        Button { label, kind, state }.paint(buf, rect, theme);
        hits.register(rect, hit);
    }

    // Tab strip: label row on the bar's middle row, underline below it.
    let hovered_tab = ManageTab::ALL
        .iter()
        .enumerate()
        .find(|(i, _)| hovered == Some(&Hit::ManageTab(*i)))
        .map(|(i, _)| i);
    let (ul_x, ul_w) = underline.unwrap_or_else(|| {
        spans
            .get(tab.index())
            .map(|(x, w)| (*x as f32, *w as f32))
            .unwrap_or((0.0, 0.0))
    });
    let strip_area = Rect {
        x: left_edge + 1,
        y: bar.y + BUTTON_HEIGHT / 2,
        width: strip_w.min(x.saturating_sub(left_edge + 1)),
        height: 2,
    };
    let rects = TabStrip {
        tabs: &tabs,
        active: tab.index(),
        hovered: hovered_tab,
        focused: false,
        underline: (ul_x, ul_w),
        disabled: None,
    }
    .paint(buf, strip_area, theme.panel, theme);
    // Belt and braces: the buttons now yield to the strip's full width,
    // but `TabStrip::paint` returns each tab's whole span even where the
    // strip area is narrower than its tabs (a bar too narrow for even the
    // strip), so clip every rect to the strip's own room rather than let
    // one register over a button.
    let strip_end = strip_area.x + strip_area.width;
    for (i, rect) in rects.iter().enumerate() {
        if rect.x >= strip_end {
            continue;
        }
        let clipped = Rect {
            width: rect.width.min(strip_end - rect.x),
            ..*rect
        };
        hits.register(clipped, Hit::ManageTab(i));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn render(tab: ManageTab) -> (String, HitMap) {
        render_at(tab, 100)
    }

    fn render_at(tab: ManageTab, width: u16) -> (String, HitMap) {
        let theme = Theme::dark();
        let mut terminal = Terminal::new(TestBackend::new(width, 3)).unwrap();
        let mut hits = HitMap::default();
        terminal
            .draw(|f| draw_manage_bar(f, f.area(), &theme, tab, None, &mut hits, None))
            .unwrap();
        (format!("{:?}", terminal.backend().buffer()), hits)
    }

    fn intersects(a: Rect, b: Rect) -> bool {
        a.x < b.x + b.width && b.x < a.x + a.width
    }

    /// The tab strip has priority for its natural width: at 80 columns —
    /// the width `app::tests::rendered_text` renders at, and too narrow
    /// for the strip plus all three buttons — every tab label still
    /// paints in full and no button overlaps the strip's last tab. At 120
    /// there is room for everything.
    #[test]
    fn buttons_yield_to_the_tab_strip_on_a_narrow_bar() {
        let (content, hits) = render_at(ManageTab::Variables, 80);
        for t in ManageTab::ALL {
            assert!(
                content.contains(t.label()),
                "{} missing: {content}",
                t.label()
            );
        }
        let spaces = hits
            .rect_of(&Hit::ManageTab(2))
            .expect("the last tab is registered");
        if let Some(new_var) = hits.rect_of(&Hit::VmNewVar) {
            assert!(
                !intersects(spaces, new_var),
                "the + Variable button must not sit on the Spaces tab: \
                 {spaces:?} vs {new_var:?}"
            );
        }
        assert!(
            hits.rect_of(&Hit::FooterChip(Action::CloseScreen))
                .is_some(),
            "close is the last button to be dropped"
        );

        let (_content, hits) = render_at(ManageTab::Variables, 120);
        assert!(hits.rect_of(&Hit::VmNewVar).is_some());
        assert!(hits.rect_of(&Hit::VmNewSelector).is_some());
        let spaces = hits.rect_of(&Hit::ManageTab(2)).unwrap();
        for hit in [
            Hit::VmNewVar,
            Hit::VmNewSelector,
            Hit::FooterChip(Action::CloseScreen),
        ] {
            let r = hits.rect_of(&hit).unwrap();
            assert!(!intersects(spaces, r), "{hit:?} overlaps the strip: {r:?}");
        }
    }

    /// The bar paints the underline the caller hands it — the eased
    /// edges from the app's animation — not the active tab's static span,
    /// so a switch glides. Painted mid-glide, the accent segment sits
    /// between the two tabs.
    #[test]
    fn bar_paints_the_underline_where_it_is_told_to() {
        let theme = Theme::dark();
        let spans = ManageTab::strip_spans();
        let (x0, _) = spans[0];
        let (x2, w2) = spans[2];
        let mid = ((x0 + x2) / 2) as f32;
        let mut terminal = Terminal::new(TestBackend::new(100, 3)).unwrap();
        let mut hits = HitMap::default();
        terminal
            .draw(|f| {
                draw_manage_bar(
                    f,
                    f.area(),
                    &theme,
                    ManageTab::Spaces,
                    Some((mid, w2 as f32)),
                    &mut hits,
                    None,
                )
            })
            .unwrap();
        let buf = terminal.backend().buffer();
        let strip = hits.rect_of(&Hit::ManageTab(0)).unwrap();
        let ul_y = strip.y + 1;
        let accent_at = |x: u16| {
            buf.cell((x, ul_y)).unwrap().fg == theme.accent
                && buf.cell((x, ul_y)).unwrap().symbol() != " "
        };
        let strip_x = hits.rect_of(&Hit::ManageTab(0)).unwrap().x;
        assert!(
            accent_at(strip_x + mid as u16 + 1),
            "the segment is painted at the handed-in position"
        );
        assert!(
            !accent_at(strip_x + x2 + w2 - 1),
            "and not at the active tab's own static span"
        );
    }

    #[test]
    fn cycle_wraps_in_both_directions() {
        assert_eq!(ManageTab::Spaces.cycle(1), ManageTab::Variables);
        assert_eq!(ManageTab::Variables.cycle(-1), ManageTab::Spaces);
        assert_eq!(ManageTab::Variables.cycle(1), ManageTab::Environments);
        for (i, t) in ManageTab::ALL.iter().enumerate() {
            assert_eq!(t.index(), i);
            assert_eq!(ManageTab::from_index(i), *t);
        }
        assert_eq!(ManageTab::from_index(99), ManageTab::Spaces, "clamps");
    }

    /// Moved here from `VarManager`'s own top-bar test when the bar became
    /// the Manage screen's: the Variables tab still carries both "new"
    /// buttons and the close button that is the mouse's way back.
    #[test]
    fn variables_tab_registers_both_new_buttons_and_close() {
        let (content, hits) = render(ManageTab::Variables);
        assert!(hits.rect_of(&Hit::VmNewVar).is_some());
        assert!(hits.rect_of(&Hit::VmNewSelector).is_some());
        assert!(content.contains("+ Variable"), "{content}");
        assert!(content.contains("+ Selector"), "{content}");
        assert!(content.contains("Close (esc)"), "{content}");
        assert!(
            hits.rect_of(&Hit::FooterChip(Action::CloseScreen))
                .is_some(),
            "the close button is the mouse's way back"
        );
    }

    #[test]
    fn every_tab_gets_a_label_and_a_hit_and_only_variables_gets_new_buttons() {
        for tab in ManageTab::ALL {
            let (content, hits) = render(tab);
            for (i, t) in ManageTab::ALL.iter().enumerate() {
                assert!(
                    content.contains(t.label()),
                    "{} missing: {content}",
                    t.label()
                );
                assert!(hits.rect_of(&Hit::ManageTab(i)).is_some());
            }
            assert!(
                hits.rect_of(&Hit::FooterChip(Action::CloseScreen))
                    .is_some()
            );
            assert_eq!(
                hits.rect_of(&Hit::VmNewVar).is_some(),
                tab == ManageTab::Variables,
                "the new buttons belong to the Variables tab only"
            );
        }
    }
}

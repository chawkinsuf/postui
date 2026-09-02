//! The Manage screen's shell: which tab is up, and the top bar shared by
//! every tab (tab strip left, Close right). Each tab's "new" buttons live
//! at the top of its own left column. Tab bodies are drawn by `ui.rs` — `VarManager` for
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
/// `Hit::ManageTab(i)`) and `Close (esc)` at the right. `underline` is
/// the accent segment's `(left, width)` in fractional columns relative to
/// the strip's origin — the app's eased edges mid-glide — or `None` for
/// the active tab's own static span.
///
/// The strip has priority for its natural width: the Close button lays
/// out only into the room left of it and is dropped rather than painted
/// over the strip. Dropped, it stays reachable by key (`esc`) and through
/// the footer chips, so nothing is lost.
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

    // The close button, right-aligned: the mouse's way back to the main
    // screen (the header's Manage chip toggles it too), labelled with the
    // key that does the same thing.
    let mut x = bar.x + bar.width;
    let buttons: Vec<(&str, ButtonKind, Hit)> = vec![(
        "Close (esc)",
        ButtonKind::Secondary,
        Hit::FooterChip(Action::CloseScreen),
    )];
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

    /// The tab strip has priority for its natural width: on a bar too
    /// narrow for the strip plus the Close button, every tab label still
    /// paints in full and Close drops rather than overlapping the strip's
    /// last tab. At 80 columns there is room for both.
    #[test]
    fn close_yields_to_the_tab_strip_on_a_narrow_bar() {
        let (content, hits) = render_at(ManageTab::Variables, 44);
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
        if let Some(close) = hits.rect_of(&Hit::FooterChip(Action::CloseScreen)) {
            assert!(
                !intersects(spaces, close),
                "Close must not sit on the Spaces tab: {spaces:?} vs {close:?}"
            );
        }

        let (_content, hits) = render_at(ManageTab::Variables, 80);
        let spaces = hits.rect_of(&Hit::ManageTab(2)).unwrap();
        let r = hits
            .rect_of(&Hit::FooterChip(Action::CloseScreen))
            .expect("Close fits at 80");
        assert!(!intersects(spaces, r), "Close overlaps the strip: {r:?}");
    }

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

    /// The close button is the mouse's way back on every tab; the "new"
    /// buttons moved into the Variables tab's own left column, so no tab
    /// puts anything but the strip and Close on the bar.
    #[test]
    fn every_tab_gets_a_label_a_hit_and_close_and_nothing_else() {
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
            assert!(content.contains("Close (esc)"), "{content}");
            assert!(
                hits.rect_of(&Hit::FooterChip(Action::CloseScreen))
                    .is_some(),
                "the close button is the mouse's way back"
            );
            assert!(hits.rect_of(&Hit::VmNewVar).is_none(), "{tab:?}");
            assert!(hits.rect_of(&Hit::VmNewSelector).is_none(), "{tab:?}");
        }
    }
}

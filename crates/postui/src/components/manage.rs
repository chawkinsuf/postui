//! The Manage screen's shell: which tab is up, and the top bar shared by
//! every tab (nothing but the tab strip). Each tab's "new" buttons live
//! at the top of its own left column. Tab bodies are drawn by `ui.rs` — `VarManager` for
//! Variables, `ManageList` for Environments and Spaces.

use crate::hit::{Hit, HitMap};
use crate::paint::{BUTTON_HEIGHT, TabStrip, fill};
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
    /// Global app settings. The one tab that is not about the open
    /// project, which is why it sits apart at the strip's right edge and
    /// why it still works with no project open.
    Settings,
}

impl ManageTab {
    /// Every tab, in on-screen order — the order `index`/`from_index`/
    /// `cycle` and the tab strip all read from.
    pub const ALL: [ManageTab; 4] = [
        ManageTab::Variables,
        ManageTab::Environments,
        ManageTab::Spaces,
        ManageTab::Settings,
    ];

    /// How many trailing tabs sit at the strip's right edge.
    pub const RIGHT_ANCHORED: usize = 1;

    pub fn label(self) -> &'static str {
        match self {
            ManageTab::Variables => "Variables",
            ManageTab::Environments => "Environments",
            ManageTab::Spaces => "Spaces",
            ManageTab::Settings => "Settings",
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

    /// Each tab's `(x, width)` span relative to the strip's origin, at
    /// strip `width` — the geometry `draw_manage_bar` lays the strip out
    /// with, exposed so the app can glide the underline between them.
    pub fn strip_spans(width: u16) -> Vec<(u16, u16)> {
        let tabs: Vec<(String, Option<(&'static str, ratatui::style::Color)>)> = Self::ALL
            .iter()
            .map(|t| (t.label().to_string(), None))
            .collect();
        TabStrip::spans_in(&tabs, Self::RIGHT_ANCHORED, width)
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

/// The tab strip's rect inside the Manage `bar` — the *one* place the
/// strip's geometry is decided.
///
/// Three callers need it and they must agree exactly: `draw_manage_bar`
/// lays the strip out here, `ui.rs` records this width on the app, and
/// `App::retarget_manage_tab_underline` glides the underline along
/// `ManageTab::strip_spans` at that same width. Computing it three ways
/// is how the right-anchored Settings tab ended up painting contiguously
/// while the underline aimed at the bar's right edge.
///
/// Nothing else shares the bar, so the strip gets the bar's full width
/// less its two-column left inset, running out to the bar's right edge.
pub fn strip_area(bar: Rect) -> Rect {
    Rect {
        x: bar.x + 2,
        y: bar.y + BUTTON_HEIGHT / 2,
        width: bar.width.saturating_sub(2),
        height: 2,
    }
}

/// Paints the top bar: nothing but the tab strip, registering
/// `Hit::ManageTab(i)` for each tab. `underline` is the accent segment's
/// `(left, width)` in fractional columns relative to the strip's origin —
/// the app's eased edges mid-glide — or `None` for the active tab's own
/// static span.
///
/// Close and Reload All used to share this bar with the strip. Close was
/// redundant — `OpenManage` toggles, so the header's Manage chip is
/// already a working close for the mouse, and `esc` is still bound — and
/// Reload moved into the header's save/discard slot, which is free
/// whenever this screen is up. With both gone the strip gets the bar's
/// full width.
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

    let tabs: Vec<(String, Option<(&'static str, ratatui::style::Color)>)> = ManageTab::ALL
        .iter()
        .map(|t| (t.label().to_string(), None))
        .collect();

    // Tab strip: label row on the bar's middle row, underline below it.
    let hovered_tab = ManageTab::ALL
        .iter()
        .enumerate()
        .find(|(i, _)| hovered == Some(&Hit::ManageTab(*i)))
        .map(|(i, _)| i);
    let strip_area = strip_area(bar);
    // The static fallback must agree with what `TabStrip::paint` below
    // will actually lay out (including the right-anchored Settings tab)
    // — otherwise an untracked underline would land under the contiguous
    // position while the labels themselves sit right-anchored.
    let (ul_x, ul_w) = underline.unwrap_or_else(|| {
        TabStrip::spans_in(&tabs, ManageTab::RIGHT_ANCHORED, strip_area.width)
            .get(tab.index())
            .map(|(x, w)| (*x as f32, *w as f32))
            .unwrap_or((0.0, 0.0))
    });
    let rects = TabStrip {
        tabs: &tabs,
        active: tab.index(),
        hovered: hovered_tab,
        focused: false,
        underline: (ul_x, ul_w),
        disabled: None,
        right_anchored: ManageTab::RIGHT_ANCHORED,
    }
    .paint(buf, strip_area, theme.panel, theme);
    // `TabStrip::paint` returns each tab's whole span even where the strip
    // area is narrower than its tabs (a bar too narrow for the strip's
    // natural width), so clip every rect to the strip's own room rather
    // than register a hit past the bar's edge.
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
    use crate::action::Action;
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

    /// The bar has nothing left to yield to: the strip now owns the full
    /// bar width, so even on a narrow bar the contiguous left group's
    /// labels paint in full. The right-anchored Settings tab is exempt
    /// from the "paints in full" guarantee -- `TabStrip::spans_in` clips
    /// it at the right edge rather than dropping it, so it stays
    /// reachable by keyboard even where there isn't room to show its
    /// whole label.
    #[test]
    fn the_strip_gets_the_full_bar_width() {
        let (content, hits) = render_at(ManageTab::Variables, 44);
        for t in [
            ManageTab::Variables,
            ManageTab::Environments,
            ManageTab::Spaces,
        ] {
            assert!(
                content.contains(t.label()),
                "{} missing: {content}",
                t.label()
            );
        }
        assert!(
            hits.rect_of(&Hit::ManageTab(ManageTab::Settings.index()))
                .is_some(),
            "Settings stays reachable even clipped"
        );
    }

    /// The Settings tab is right-anchored, and the whole point of that
    /// is that it sits at the strip's right edge -- not contiguously
    /// after Spaces. The strip is laid out at `strip_area(bar).width`,
    /// so its right edge is that rect's right edge.
    ///
    /// This is the seam that shipped inert: the strip used to be sized
    /// to its own *contiguous* total, which made the anchored start and
    /// the contiguous floor identical and the shift always zero.
    #[test]
    fn the_settings_tab_paints_at_the_strips_right_edge() {
        for width in [80u16, 100, 120] {
            let bar = Rect {
                x: 0,
                y: 0,
                width,
                height: 3,
            };
            let strip = strip_area(bar);
            // The strip is entitled to the whole bar, so its right edge
            // is the bar's -- asserted here rather than folded into the
            // next assertion, which would otherwise pass with the strip
            // sized to its own contiguous total (both sides shrinking
            // together, which is exactly the bug).
            assert_eq!(strip.x + strip.width, bar.x + bar.width);
            let (_, hits) = render_at(ManageTab::Variables, width);
            let settings = hits
                .rect_of(&Hit::ManageTab(ManageTab::Settings.index()))
                .expect("Settings registers a hit");
            assert_eq!(
                settings.x + settings.width,
                strip.x + strip.width,
                "Settings is flush with the strip's right edge at bar width {width}"
            );
            let spaces = hits
                .rect_of(&Hit::ManageTab(ManageTab::Spaces.index()))
                .expect("Spaces registers a hit");
            assert!(
                settings.x > spaces.x + spaces.width,
                "and it is pushed clear of the contiguous left group at width {width}"
            );
        }
    }

    #[test]
    fn bar_paints_the_underline_where_it_is_told_to() {
        let theme = Theme::dark();
        let spans = ManageTab::strip_spans(100);
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
    fn settings_is_the_right_anchored_tab_and_joins_the_cycle() {
        assert_eq!(ManageTab::ALL.len(), 4);
        assert_eq!(ManageTab::ALL[3], ManageTab::Settings);
        assert_eq!(ManageTab::Spaces.cycle(1), ManageTab::Settings);
        assert_eq!(ManageTab::Settings.cycle(1), ManageTab::Variables, "wraps");
        assert_eq!(ManageTab::Variables.cycle(-1), ManageTab::Settings);
    }

    #[test]
    fn cycle_wraps_in_both_directions() {
        assert_eq!(ManageTab::Settings.cycle(1), ManageTab::Variables);
        assert_eq!(ManageTab::Variables.cycle(-1), ManageTab::Settings);
        assert_eq!(ManageTab::Variables.cycle(1), ManageTab::Environments);
        for (i, t) in ManageTab::ALL.iter().enumerate() {
            assert_eq!(t.index(), i);
            assert_eq!(ManageTab::from_index(i), *t);
        }
        assert_eq!(ManageTab::from_index(99), ManageTab::Settings, "clamps");
    }

    /// The bar is now nothing but the strip. Close is gone -- the header's
    /// Manage chip already toggles the screen and esc is still bound -- and
    /// Reload moved to the header's save/discard slot, which is free
    /// whenever this screen is up.
    #[test]
    fn the_bar_is_the_strip_and_nothing_else() {
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
            assert!(!content.contains("Close"), "Close was removed: {content}");
            assert!(
                !content.contains("Reload"),
                "Reload moved to the header: {content}"
            );
            assert!(
                hits.rect_of(&Hit::FooterChip(Action::CloseScreen))
                    .is_none(),
                "no close hit remains"
            );
            assert!(
                hits.rect_of(&Hit::FooterChip(Action::ReloadFromDisk))
                    .is_none(),
                "no reload hit remains on the bar"
            );
        }
    }
}

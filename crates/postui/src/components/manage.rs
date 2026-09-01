//! The Manage screen's shell: which tab is up, and the top bar shared by
//! every tab (tab strip left, Close right, the Variables tab's own "new"
//! buttons between). Tab bodies are drawn by `ui.rs` — `VarManager` for
//! Variables, the flat-panel placeholder for Environments and Spaces until
//! Task 14 fills them in.

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

    /// Steps `delta` tabs along `ALL`, wrapping in both directions.
    pub fn cycle(self, delta: i32) -> Self {
        let n = Self::ALL.len() as i32;
        Self::from_index((self.index() as i32 + delta).rem_euclid(n) as usize)
    }
}

/// The Manage screen's own state: which tab is up. Each tab's body keeps
/// its own state elsewhere (`App::varmanager` for Variables).
#[derive(Default)]
pub struct Manage {
    pub tab: ManageTab,
}

/// The bar's height: the Variables tab's buttons are `BUTTON_HEIGHT` tall
/// and the tab strip needs two rows (label + underline) inside that.
pub const BAR_HEIGHT: u16 = BUTTON_HEIGHT;

/// Paints the top bar: tab strip at the left edge (registering
/// `Hit::ManageTab(i)`), `Close (esc)` at the right, and on the Variables
/// tab the `+ Variable` / `+ Selector` buttons before it.
pub fn draw_manage_bar(
    frame: &mut Frame,
    bar: Rect,
    theme: &Theme,
    tab: ManageTab,
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

    // Right-aligned buttons, laid out from the right edge inward. The
    // close button is the mouse's way back to the main screen (the
    // header's Manage chip toggles it too), labelled with the key that
    // does the same thing.
    let left_edge = bar.x + 1;
    let mut x = bar.x + bar.width;
    let mut buttons: Vec<(&str, ButtonKind, Hit)> = vec![(
        "Close (esc)",
        ButtonKind::Secondary,
        Hit::FooterChip(Action::CloseScreen),
    )];
    if tab == ManageTab::Variables {
        buttons.push(("+ Selector", ButtonKind::Secondary, Hit::VmNewSelector));
        buttons.push(("+ Variable", ButtonKind::Primary, Hit::VmNewVar));
    }
    for (label, kind, hit) in buttons {
        let w = button_min_width(label);
        if x < left_edge + w + 2 {
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
    let tabs: Vec<(String, Option<(char, ratatui::style::Color)>)> = ManageTab::ALL
        .iter()
        .map(|t| (t.label().to_string(), None))
        .collect();
    let hovered_tab = ManageTab::ALL
        .iter()
        .enumerate()
        .find(|(i, _)| hovered == Some(&Hit::ManageTab(*i)))
        .map(|(i, _)| i);
    let spans = TabStrip::spans(&tabs);
    let (ul_x, ul_w) = spans
        .get(tab.index())
        .map(|(x, w)| (*x as f32, *w as f32))
        .unwrap_or((0.0, 0.0));
    let strip_area = Rect {
        x: left_edge + 1,
        y: bar.y + BUTTON_HEIGHT / 2,
        width: x.saturating_sub(left_edge + 2),
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
    // `TabStrip::paint` returns each tab's full span even where the strip
    // is narrower than its tabs, so clip every rect to the room the
    // buttons left: on a bar too narrow for both, the buttons stay
    // clickable and the last tab is merely clipped rather than stealing
    // their clicks.
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
        let theme = Theme::dark();
        let mut terminal = Terminal::new(TestBackend::new(100, 3)).unwrap();
        let mut hits = HitMap::default();
        terminal
            .draw(|f| draw_manage_bar(f, f.area(), &theme, tab, &mut hits, None))
            .unwrap();
        (format!("{:?}", terminal.backend().buffer()), hits)
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

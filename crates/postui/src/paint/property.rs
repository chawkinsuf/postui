//! Property rows: a label column and a one-row control on the same
//! line — the control language of the Manage screen's detail panes.
//!
//! Deliberately *not* a [`crate::paint::ListRow`]. The band
//! `RowHighlight::Selected` paints belongs to **lists of items**: the
//! sidebar, the Manage screen's left columns, the choosers, the
//! pickers, the request table. A row of controls is not one of those —
//! there is no item, no selection and nothing to reorder — so painting
//! it in the list vocabulary promises behaviour the row can never have.
//! A property row shows its keyboard cursor by lifting its *control's*
//! own fill instead, the app's focus language everywhere else.
//!
//! Every painter here is exactly one row tall, and takes its faces from
//! the same [`crate::paint::control_face`] ladder the full-size
//! controls use. Height is the only thing that separates the two: a
//! [`Well`] is a one-row [`crate::paint::TextField`], a [`Pill`] a
//! one-row [`crate::paint::Button`], and a pane picks whichever size
//! its content wants. Nothing anywhere in the app is bevelled — these
//! rows were the first surface to go flat, and the rest followed.
//!
//! One rule governs the hover wash, and every pane converted to these
//! rows follows it: **a row washes when the pointer is over something
//! that row will respond to** — its label half, its control, or any
//! pill it carries. A row that registers no hit of its own is not a
//! click target and never washes, however it is laid out; a row that
//! activates from either half must wash from either half, or the wash
//! becomes a lie about where the click will land.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
};

use crate::hit::{Hit, HitMap};
use crate::paint::{ButtonKind, ControlState, control_face, fill, text};
use crate::theme::Theme;

/// Padding either side of a [`Well`]'s content. The mouse maps a click
/// back through this same number, so it is what keeps the caret under
/// the pointer.
pub const WELL_PAD: u16 = 1;

/// Padding either side of a [`Pill`]'s label.
pub const PILL_PAD: u16 = 1;

/// A [`Toggle`]'s width: one glyph with a column of face either side.
pub const TOGGLE_W: u16 = 3;

/// Bounds on a pane's label column. The floor keeps a pane of short
/// labels from collapsing its controls against the left edge; the
/// ceiling keeps one runaway label from eating the row.
pub const LABEL_W_MIN: u16 = 16;
pub const LABEL_W_MAX: u16 = 32;

/// The widest a property row is painted, however wide the pane is: a
/// text well stretched across a 200-column terminal is unreadable.
/// Applies to all three Manage panes, so the control column does not
/// jump as the tab strip switches between them.
pub const PROPERTY_MAX_W: u16 = 76;

/// How far a hovered row's fill blends from `page` toward `control`.
/// Far below `RowHighlight::Selected`'s 0.35 accent blend, and with no
/// accent bar — enough to say "this whole row is a click target",
/// nowhere near enough to say "this is a list item you can drag".
pub const HOVER_WASH: f32 = 0.2;

/// A pane's label column, from its own longest label: the label plus
/// two columns of gutter, clamped to [`LABEL_W_MIN`]..=[`LABEL_W_MAX`].
///
/// A pane computes this once and hands it to every row it paints, so
/// its controls line up and no label is truncated. A fixed constant
/// could not do both: Settings' longest label is "Ask before sending to
/// AI" while the Variables pane's is "Value in <env>", whose width
/// depends on the environment's display name.
pub fn label_column(labels: &[&str]) -> u16 {
    let widest = labels
        .iter()
        .map(|l| l.chars().count() as u16)
        .max()
        .unwrap_or(0);
    widest.saturating_add(2).clamp(LABEL_W_MIN, LABEL_W_MAX)
}

/// The narrowest a [`Pill`] can be painted with `label` intact.
pub fn pill_min_width(label: &str) -> u16 {
    label.chars().count() as u16 + PILL_PAD * 2
}

/// A [`Pill`] pinned to a row's right edge, with the hit it registers.
/// [`PropertyRow::paint`] lays these out and registers them itself, so
/// no caller has to repeat the right-to-left geometry by hand.
pub struct TrailingPill<'a> {
    pub label: &'a str,
    pub kind: ButtonKind,
    pub state: ControlState,
    pub hit: Hit,
}

/// Where a row's own control goes, and the background it landed on — so
/// a control painting on top of the row knows its own backdrop, the way
/// `ListRow::resolve_fill` serves list rows.
pub struct ControlSlot {
    pub rect: Rect,
    pub bg: Color,
}

/// A label and one control on a single row.
pub struct PropertyRow<'a> {
    pub label: &'a str,
    /// The pane's label column, from [`label_column`].
    pub label_w: u16,
    pub hovered: bool,
    pub disabled: bool,
    pub trailing: &'a [TrailingPill<'a>],
}

impl PropertyRow<'_> {
    /// Paints the row across `area` (exactly one row tall), registers
    /// its trailing pills, and returns the slot the caller paints its
    /// own control into.
    pub fn paint(
        &self,
        buf: &mut Buffer,
        hits: &mut HitMap,
        area: Rect,
        theme: &Theme,
    ) -> ControlSlot {
        // A disabled row never washes: there is nothing under the
        // pointer to activate, and a wash would say otherwise.
        let bg = if self.hovered && !self.disabled {
            crate::theme::mix(theme.page, theme.control, HOVER_WASH)
        } else {
            theme.page
        };
        fill(buf, area, bg);
        let label_fg = if self.disabled {
            theme.text_disabled
        } else {
            theme.text
        };
        text(buf, area.x, area.y, self.label, label_fg, bg, false);

        // Trailing pills first, right-to-left from the row's right edge:
        // the slot is measured against where they stopped, so a long
        // value can never paint over the button beside it. A pill that
        // would reach back into the label column is dropped rather than
        // painted over it — it stays reachable by its own key.
        let mut right = area.x + area.width;
        for t in self.trailing {
            let w = pill_min_width(t.label);
            if right.saturating_sub(w) <= area.x + self.label_w {
                break;
            }
            right -= w;
            let rect = Rect {
                x: right,
                y: area.y,
                width: w,
                height: 1,
            };
            Pill {
                label: t.label,
                kind: t.kind,
                state: t.state,
            }
            .paint(buf, rect, theme);
            hits.register(rect, t.hit.clone());
            right = right.saturating_sub(1);
        }

        // Clamped to where the pills stopped: a pane too narrow for its
        // own label column would otherwise hand back a slot starting
        // past the row's right edge, and a control painted from there
        // lands outside the row entirely.
        let x = (area.x + self.label_w).min(right);
        ControlSlot {
            rect: Rect {
                x,
                y: area.y,
                width: right.saturating_sub(x),
                height: 1,
            },
            bg,
        }
    }
}

/// A one-row filled text box: the compact [`crate::paint::TextField`].
pub struct Well<'a> {
    pub content: Line<'a>,
    pub state: ControlState,
}

impl Well<'_> {
    pub fn paint(&self, buf: &mut Buffer, area: Rect, theme: &Theme) {
        let (face, disabled_fg) = control_face(theme, ButtonKind::Secondary, self.state);
        fill(buf, area, face);
        let inner = area.width.saturating_sub(WELL_PAD * 2);
        if inner == 0 {
            return;
        }
        // Disabled recolors every span: a content line carries its own
        // styling (a caret, a selection) that must not survive into a
        // control the user cannot reach.
        let line = if self.state == ControlState::Disabled {
            Line::from(
                self.content
                    .spans
                    .iter()
                    .map(|s| Span::styled(s.content.clone(), Style::default().fg(disabled_fg)))
                    .collect::<Vec<_>>(),
            )
        } else {
            let mut l = self.content.clone();
            l.style = Style::default().fg(theme.text).bg(face).patch(l.style);
            l
        };
        buf.set_line(area.x + WELL_PAD, area.y, &line, inner);
    }
}

/// A one-row filled button: the compact [`crate::paint::Button`].
pub struct Pill<'a> {
    pub label: &'a str,
    pub kind: ButtonKind,
    pub state: ControlState,
}

impl Pill<'_> {
    pub fn paint(&self, buf: &mut Buffer, area: Rect, theme: &Theme) {
        let (face, fg) = control_face(theme, self.kind, self.state);
        fill(buf, area, face);
        text(buf, area.x + PILL_PAD, area.y, self.label, fg, face, false);
    }
}

/// The rows a Manage pane's title-row button spans.
///
/// The same block [`crate::paint::Button`] occupies, because it *is* a
/// [`crate::paint::Button`] — the Manage screen's title-row buttons used
/// to have a painter of their own ("TallPill") for the one thing that
/// made them different: they were flat while `Button` was bevelled. Now
/// that every control in the app is flat, there is nothing left to keep
/// apart, and the name survives only as the Manage screen's word for
/// the height its panes lay out against.
pub const TALL_PILL_H: u16 = crate::paint::BUTTON_HEIGHT;

/// A checkbox on a [`Well`]-height face.
pub struct Toggle {
    pub on: bool,
    pub state: ControlState,
}

impl Toggle {
    /// Paints into the left [`TOGGLE_W`] columns of `area` — or fewer,
    /// if `area` is narrower — and returns the rect it actually
    /// painted, which is the rect a caller must register its hit over.
    /// The clamp lives here rather than in the callers: a caller that
    /// builds its own `TOGGLE_W`-wide rect from a slot narrower than
    /// that registers a click target past the row's right edge.
    pub fn paint(&self, buf: &mut Buffer, area: Rect, theme: &Theme) -> Rect {
        let (face, _) = control_face(theme, ButtonKind::Secondary, self.state);
        let rect = Rect {
            width: area.width.min(TOGGLE_W),
            ..area
        };
        fill(buf, rect, face);
        // The glyph sits one column in, so a face narrower than two
        // columns has nowhere to put it: painting anyway would land it
        // past the rect this returns, outside the row.
        if rect.width < 2 {
            return rect;
        }
        let glyph = if self.on {
            crate::glyph::CHECKBOX
        } else {
            crate::glyph::CHECKBOX_OFF
        };
        let fg = match self.state {
            ControlState::Disabled => theme.text_disabled,
            _ if self.on => theme.accent,
            _ => theme.text_muted,
        };
        text(buf, rect.x + 1, rect.y, glyph, fg, face, false);
        rect
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hit::HitMap;
    use crate::theme::Theme;
    use ratatui::{Terminal, backend::TestBackend};

    fn buf_cell(term: &Terminal<TestBackend>, x: u16, y: u16) -> &ratatui::buffer::Cell {
        term.backend().buffer().cell((x, y)).unwrap()
    }

    /// Relative luminance, for asserting one face outshines another
    /// without pinning exact channel values.
    fn lum(c: ratatui::style::Color) -> f32 {
        match c {
            ratatui::style::Color::Rgb(r, g, b) => {
                0.2126 * r as f32 + 0.7152 * g as f32 + 0.0722 * b as f32
            }
            other => panic!("expected an rgb theme color, got {other:?}"),
        }
    }

    /// The whole point of the primitive: a property row is not a list
    /// row. The band belongs to lists of items, and painting it here
    /// promises a drag that a row of controls can never offer.
    #[test]
    fn a_property_row_never_paints_the_list_band_or_its_accent_bar() {
        let theme = Theme::dark();
        for hovered in [false, true] {
            let mut term = Terminal::new(TestBackend::new(60, 3)).unwrap();
            let mut hits = HitMap::default();
            term.draw(|f| {
                PropertyRow {
                    label: "Hover hints",
                    label_w: 22,
                    hovered,
                    disabled: false,
                    trailing: &[],
                }
                .paint(f.buffer_mut(), &mut hits, Rect::new(0, 1, 60, 1), &theme);
            })
            .unwrap();
            for x in 0..60 {
                let cell = buf_cell(&term, x, 1);
                assert_ne!(
                    cell.bg, theme.selection,
                    "hovered={hovered}: the list band has no business here"
                );
                assert_ne!(cell.symbol(), "▌", "hovered={hovered}: nor its bar");
            }
        }
    }

    /// Hover is a wash, not a band: visibly present, and far below the
    /// 0.35 accent blend `RowHighlight::Selected` paints.
    #[test]
    fn hover_washes_the_row_well_below_the_list_band() {
        let theme = Theme::dark();
        let mut term = Terminal::new(TestBackend::new(60, 1)).unwrap();
        let mut hits = HitMap::default();
        term.draw(|f| {
            PropertyRow {
                label: "Hover hints",
                label_w: 22,
                hovered: true,
                disabled: false,
                trailing: &[],
            }
            .paint(f.buffer_mut(), &mut hits, Rect::new(0, 0, 60, 1), &theme);
        })
        .unwrap();
        let washed = buf_cell(&term, 40, 0).bg;
        assert_eq!(
            washed,
            crate::theme::mix(theme.page, theme.control, HOVER_WASH)
        );
        assert_ne!(washed, theme.page, "the wash is visible");
        assert!(
            lum(washed) < lum(theme.selection),
            "and quieter than the band it replaces"
        );
    }

    /// The cursor is the control lifting its own fill, so Focused must
    /// outshine Hover — otherwise "where am I" and "where is the mouse"
    /// paint identically.
    #[test]
    fn focused_outshines_hover_for_both_pill_kinds_and_the_well() {
        let theme = Theme::dark();
        assert!(
            lum(control_face(&theme, ButtonKind::Secondary, ControlState::Focused).0)
                > lum(control_face(&theme, ButtonKind::Secondary, ControlState::Hover).0),
            "a secondary pill and a well share this face"
        );
        assert!(
            lum(control_face(&theme, ButtonKind::Primary, ControlState::Focused).0)
                > lum(control_face(&theme, ButtonKind::Primary, ControlState::Hover).0),
            "and an active segment must show its cursor too"
        );
    }

    /// A well paints its content one column in, across `width - 2`.
    /// The mouse maps a click back through the same two numbers.
    #[test]
    fn a_well_pads_its_content_by_one_column_each_side() {
        let theme = Theme::dark();
        let mut term = Terminal::new(TestBackend::new(20, 1)).unwrap();
        term.draw(|f| {
            Well {
                content: ratatui::text::Line::raw("abc"),
                state: ControlState::Normal,
            }
            .paint(f.buffer_mut(), Rect::new(2, 0, 10, 1), &theme);
        })
        .unwrap();
        assert_eq!(buf_cell(&term, 2, 0).symbol(), " ", "left pad");
        assert_eq!(buf_cell(&term, 3, 0).symbol(), "a");
        assert_eq!(buf_cell(&term, 3, 0).bg, theme.control);
        assert_eq!(WELL_PAD, 1, "the mouse math depends on this");
    }

    /// Trailing pills are laid out from the right edge inward and the
    /// slot stops short of them, so a long value can never paint over
    /// the reveal button beside it.
    #[test]
    fn trailing_pills_take_the_right_edge_and_the_slot_stops_short() {
        let theme = Theme::dark();
        let mut term = Terminal::new(TestBackend::new(60, 1)).unwrap();
        let mut hits = HitMap::default();
        let mut slot = None;
        term.draw(|f| {
            slot = Some(
                PropertyRow {
                    label: "Value in prod",
                    label_w: 22,
                    hovered: false,
                    disabled: false,
                    trailing: &[TrailingPill {
                        label: "reveal",
                        kind: ButtonKind::Secondary,
                        state: ControlState::Normal,
                        hit: Hit::VmRevealToggle,
                    }],
                }
                .paint(f.buffer_mut(), &mut hits, Rect::new(0, 0, 60, 1), &theme),
            );
        })
        .unwrap();
        let slot = slot.unwrap();
        let pill = hits
            .rect_of(&Hit::VmRevealToggle)
            .expect("the row registers it");
        assert_eq!(pill.width, pill_min_width("reveal"));
        assert_eq!(pill.x + pill.width, 60, "flush with the row's right edge");
        assert!(
            slot.rect.x + slot.rect.width <= pill.x,
            "slot {:?} must stop short of the pill at {}",
            slot.rect,
            pill.x
        );
        assert_eq!(slot.rect.x, 22, "and start at the label column");
    }

    /// The label column fits the longest label and never truncates it,
    /// which a fixed constant could not promise for two panes at once.
    #[test]
    fn the_label_column_fits_the_longest_label_within_its_clamp() {
        assert_eq!(label_column(&["Ask before sending to AI"]), 26);
        assert_eq!(label_column(&["a"]), LABEL_W_MIN, "short labels clamp up");
        assert_eq!(
            label_column(&["a label far longer than any pane should ever use"]),
            LABEL_W_MAX,
            "and a runaway one clamps down rather than eating the row"
        );
    }

    /// Disabled content blends toward the control's own fill, the same
    /// treatment `Button` and `TextField` already use.
    #[test]
    fn a_disabled_well_blends_its_content_toward_its_fill() {
        let theme = Theme::dark();
        let mut term = Terminal::new(TestBackend::new(20, 1)).unwrap();
        term.draw(|f| {
            Well {
                content: ratatui::text::Line::raw("abc"),
                state: ControlState::Disabled,
            }
            .paint(f.buffer_mut(), Rect::new(0, 0, 10, 1), &theme);
        })
        .unwrap();
        assert_eq!(
            buf_cell(&term, 1, 0).fg,
            crate::theme::mix(
                theme.control,
                theme.text_muted,
                crate::paint::DISABLED_LABEL_MIX
            )
        );
    }

    /// A pane too narrow for its label column hands out a slot of one
    /// or zero columns. Neither control may paint or register a single
    /// cell outside the row it was given: a hit registered past the
    /// right edge is a click target the row does not own.
    #[test]
    fn a_starved_slot_keeps_both_controls_inside_the_row() {
        let theme = Theme::dark();
        // label_w 16 leaves two columns; label_w 20 leaves none at all.
        for (label_w, area) in [(16, Rect::new(4, 1, 18, 1)), (20, Rect::new(4, 1, 18, 1))] {
            let mut term = Terminal::new(TestBackend::new(40, 3)).unwrap();
            let mut hits = HitMap::default();
            let mut toggle_rect = None;
            term.draw(|f| {
                let slot = PropertyRow {
                    label: "Secret",
                    label_w,
                    hovered: false,
                    disabled: false,
                    trailing: &[],
                }
                .paint(f.buffer_mut(), &mut hits, area, &theme);
                assert!(
                    slot.rect.width <= 2,
                    "label_w {label_w}: this test is about a starved slot, got {:?}",
                    slot.rect
                );
                Well {
                    content: ratatui::text::Line::raw("a value far wider than the slot"),
                    state: ControlState::Normal,
                }
                .paint(f.buffer_mut(), slot.rect, &theme);
                let r = Toggle {
                    on: true,
                    state: ControlState::Normal,
                }
                .paint(f.buffer_mut(), slot.rect, &theme);
                hits.register(r, Hit::VmSecretToggle);
                toggle_rect = Some(r);
            })
            .unwrap();

            let toggle = toggle_rect.unwrap();
            assert!(
                toggle.x >= area.x && toggle.x + toggle.width <= area.x + area.width,
                "label_w {label_w}: the toggle registered {toggle:?}, outside {area:?}"
            );
            let blank = ratatui::buffer::Cell::default();
            for y in 0..3 {
                for x in 0..40 {
                    if y == area.y && x >= area.x && x < area.x + area.width {
                        continue;
                    }
                    assert_eq!(
                        buf_cell(&term, x, y),
                        &blank,
                        "label_w {label_w}: painted outside the row at {x},{y}"
                    );
                }
            }
        }
    }

    /// A toggle is one cell of glyph in a three-cell face — the icon
    /// must occupy exactly one column (Material, never a VS16 emoji).
    #[test]
    fn a_toggle_paints_one_cell_of_glyph_on_a_three_cell_face() {
        let theme = Theme::dark();
        for (on, want) in [
            (true, crate::glyph::CHECKBOX),
            (false, crate::glyph::CHECKBOX_OFF),
        ] {
            let mut term = Terminal::new(TestBackend::new(10, 1)).unwrap();
            term.draw(|f| {
                Toggle {
                    on,
                    state: ControlState::Normal,
                }
                .paint(f.buffer_mut(), Rect::new(0, 0, TOGGLE_W, 1), &theme);
            })
            .unwrap();
            assert_eq!(buf_cell(&term, 1, 0).symbol(), want);
            assert_eq!(buf_cell(&term, 0, 0).bg, theme.control);
            assert_eq!(buf_cell(&term, 2, 0).bg, theme.control);
            assert_eq!(
                buf_cell(&term, 1, 0).fg,
                if on { theme.accent } else { theme.text_muted }
            );
        }
    }
}

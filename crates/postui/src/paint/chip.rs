//! Painted chips (small tinted pills used for method/status/count badges)
//! and the tab strip (labels row + underline row under the active tab).

use ratatui::{buffer::Buffer, layout::Rect, style::Color};

use crate::paint::text;
use crate::theme::Theme;

/// A single-row tinted pill: `" label "` with a bg tinted toward the chip's
/// `color` and bold text in `color`. Used for method/status/count chips.
pub struct Chip<'a> {
    pub label: &'a str,
    pub color: Color,
}

impl Chip<'_> {
    /// Paints `" label "` at `(x, y)` on top of surface `on`. Returns the
    /// width (in columns) painted, so callers can lay out subsequent chips.
    pub fn paint(&self, buf: &mut Buffer, x: u16, y: u16, on: Color, theme: &Theme) -> u16 {
        let s = format!(" {} ", self.label);
        let width = s.chars().count() as u16;
        let bg = theme.tint(self.color, on);
        text(buf, x, y, &s, self.color, bg, true);
        width
    }
}

/// A horizontal strip of flat GUI-style tabs: labels sit directly on
/// surface `on` (no block fills) over a full-width hairline rule, with an
/// accent segment sliding under the active tab. Each tuple is `(label,
/// badge)` — a badge renders a trailing colored glyph (e.g. the Body tab's
/// JSON-validity `✓`/`✗`) after the label.
pub struct TabStrip<'a> {
    pub tabs: &'a [(String, Option<(char, Color)>)],
    pub active: usize,
    /// The index of the tab currently under the mouse, if any.
    pub hovered: Option<usize>,
    /// Whether the strip itself holds keyboard focus (arrow keys switch
    /// tabs). Recolors the underline segment in the focus-ring color.
    pub focused: bool,
    /// The underline segment in fractional columns relative to `area.x`:
    /// `(left, width)`. Callers animate this (Task 10); pass the active
    /// tab's own span (from [`TabStrip::spans`]) for a static strip.
    pub underline: (f32, f32),
}

impl TabStrip<'_> {
    /// Pure geometry: each tab's `(x_offset, width)` span on the label row,
    /// relative to the strip's own origin, using the same `" label "` (+2
    /// for a badge) padding and 2-column inter-tab gap that [`Self::paint`]
    /// lays out with. Callers use this to compute underline animation
    /// targets and hit rects without painting.
    pub fn spans(tabs: &[(String, Option<(char, Color)>)]) -> Vec<(u16, u16)> {
        let mut x = 0u16;
        let mut spans = Vec::with_capacity(tabs.len());
        for (label, badge) in tabs {
            let label_w = label.chars().count() as u16;
            let width = label_w + 2 + badge.map_or(0, |_| 2);
            spans.push((x, width));
            x += width + 2; // 2-column gap between tabs
        }
        spans
    }

    /// Paints into the top 2 rows of `area` on top of surface `on`: row 0
    /// is flat labels (no fills), row 1 is a full-width hairline rule with
    /// the accent underline segment on top. Returns each tab's 2-row hit
    /// [`Rect`] (label span + the underline row), for hit registration by
    /// the caller.
    pub fn paint(&self, buf: &mut Buffer, area: Rect, on: Color, theme: &Theme) -> Vec<Rect> {
        let labels_y = area.y;
        let rule_y = area.y + 1;
        let spans = Self::spans(self.tabs);
        let mut rects = Vec::with_capacity(self.tabs.len());

        for (i, ((label, badge), (offset, width))) in self.tabs.iter().zip(&spans).enumerate() {
            let x = area.x + offset;
            let active = i == self.active;
            let fg = if active || self.hovered == Some(i) {
                theme.text
            } else {
                theme.text_muted
            };
            let label_w = label.chars().count() as u16;

            text(buf, x + 1, labels_y, label, fg, on, active);
            if let Some((glyph, color)) = badge {
                text(
                    buf,
                    x + 1 + label_w,
                    labels_y,
                    &format!(" {glyph}"),
                    *color,
                    on,
                    true,
                );
            }

            rects.push(Rect::new(x, labels_y, *width, 2));
        }

        // Row 1: the full-width hairline rule (box-drawing heavy
        // horizontal, matching the reference app's own Bar renderable),
        // then the accent segment on top of it. The segment is
        // distinguished from the rest of the rule by color alone, not a
        // thicker glyph — both track and highlight are the same `━`.
        for x in area.x..area.right() {
            if let Some(cell) = buf.cell_mut((x, rule_y)) {
                cell.set_symbol("━");
                cell.set_fg(theme.hairline);
                cell.set_bg(on);
            }
        }
        let accent = if self.focused {
            theme.focus_ring
        } else {
            theme.accent
        };
        let (left, width) = self.underline;
        if width > 0.0 {
            let right = left + width;
            // Snapped to the nearest half-cell (not whole cell) so a
            // segment mid-slide between columns still reads precisely: a
            // boundary that lands mid-cell paints a half-covered box-
            // drawing glyph (`╺` right-half, `╸` left-half) instead of
            // rounding the whole cell in or out.
            let l2 = (left * 2.0).round() as i64;
            let r2 = (right * 2.0).round() as i64;
            if r2 > l2 {
                let first_cell = l2.div_euclid(2);
                let last_cell = (r2 - 1).div_euclid(2);
                for cell in first_cell..=last_cell {
                    let Ok(cell_u16) = u16::try_from(cell) else {
                        continue;
                    };
                    let x = area.x.saturating_add(cell_u16);
                    if x >= area.right() {
                        break;
                    }
                    let cell_start = cell * 2;
                    let cell_end = cell_start + 2;
                    let overlap_start = l2.max(cell_start);
                    let overlap_end = r2.min(cell_end);
                    let glyph = if overlap_end - overlap_start >= 2 {
                        "━"
                    } else if overlap_start == cell_start {
                        "╸" // left half of the cell covered
                    } else {
                        "╺" // right half of the cell covered
                    };
                    if let Some(buf_cell) = buf.cell_mut((x, rule_y)) {
                        buf_cell.set_symbol(glyph);
                        buf_cell.set_fg(accent);
                        buf_cell.set_bg(on);
                    }
                }
            }
        }

        rects
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Theme;
    use ratatui::{Terminal, backend::TestBackend, style::Modifier};

    fn buf_cell(term: &Terminal<TestBackend>, x: u16, y: u16) -> &ratatui::buffer::Cell {
        term.backend().buffer().cell((x, y)).unwrap()
    }

    #[test]
    fn chip_paints_tinted_pill() {
        let theme = Theme::dark();
        let mut term = Terminal::new(TestBackend::new(12, 1)).unwrap();
        term.draw(|f| {
            Chip {
                label: "GET",
                color: theme.success,
            }
            .paint(f.buffer_mut(), 0, 0, theme.panel, &theme);
        })
        .unwrap();
        let c = buf_cell(&term, 1, 0);
        assert_eq!(c.symbol(), "G");
        assert_eq!(c.bg, theme.tint(theme.success, theme.panel));
        assert!(c.modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn tabstrip_paints_flat_labels_with_accent_underline_under_active() {
        let theme = Theme::dark();
        let mut term = Terminal::new(TestBackend::new(40, 2)).unwrap();
        let tabs = vec![("Params".to_string(), None), ("Headers".to_string(), None)];
        let spans = TabStrip::spans(&tabs);
        let mut rects = Vec::new();
        term.draw(|f| {
            rects = TabStrip {
                tabs: &tabs,
                active: 0,
                hovered: None,
                focused: false,
                underline: (spans[0].0 as f32, spans[0].1 as f32),
            }
            .paint(f.buffer_mut(), Rect::new(0, 0, 40, 2), theme.panel, &theme);
        })
        .unwrap();
        let label = buf_cell(&term, rects[0].x + 1, 0);
        assert_eq!(label.symbol(), "P");
        assert_eq!(label.fg, theme.text);
        assert_eq!(label.bg, theme.panel, "flat: no block fill behind labels");
        assert!(label.modifier.contains(Modifier::BOLD));
        assert_eq!(buf_cell(&term, rects[1].x + 1, 0).fg, theme.text_muted);
        // underline row: accent segment under the active tab...
        let under_active = buf_cell(&term, rects[0].x + 1, 1);
        assert_eq!(under_active.symbol(), "━");
        assert_eq!(under_active.fg, theme.accent);
        // ...hairline rule elsewhere
        let under_inactive = buf_cell(&term, rects[1].x + 1, 1);
        assert_eq!(under_inactive.symbol(), "━");
        assert_eq!(under_inactive.fg, theme.hairline);
    }

    #[test]
    fn focused_tabstrip_recolors_underline_and_mid_slide_underline_straddles_tabs() {
        let theme = Theme::dark();
        let tabs = vec![("Params".to_string(), None), ("Headers".to_string(), None)];
        let spans = TabStrip::spans(&tabs);

        // focused: segment fg == theme.focus_ring
        let mut term = Terminal::new(TestBackend::new(40, 2)).unwrap();
        let mut rects = Vec::new();
        term.draw(|f| {
            rects = TabStrip {
                tabs: &tabs,
                active: 0,
                hovered: None,
                focused: true,
                underline: (spans[0].0 as f32, spans[0].1 as f32),
            }
            .paint(f.buffer_mut(), Rect::new(0, 0, 40, 2), theme.panel, &theme);
        })
        .unwrap();
        let under_active = buf_cell(&term, rects[0].x + 1, 1);
        assert_eq!(under_active.symbol(), "━");
        assert_eq!(
            under_active.fg, theme.focus_ring,
            "focus recolors the segment"
        );

        // underline (spans[0].0 + 3.0, w): segment paints at the given
        // offset, not under either tab exactly — proves the caller-driven
        // position (mid-slide between the two tabs, not snapped to one).
        let (left0, width0) = spans[0];
        let mut term2 = Terminal::new(TestBackend::new(40, 2)).unwrap();
        let mut rects2 = Vec::new();
        term2
            .draw(|f| {
                rects2 = TabStrip {
                    tabs: &tabs,
                    active: 0,
                    hovered: None,
                    focused: false,
                    underline: (left0 as f32 + 3.0, width0 as f32),
                }
                .paint(f.buffer_mut(), Rect::new(0, 0, 40, 2), theme.panel, &theme);
            })
            .unwrap();
        // The segment now starts 3 columns into what was the active tab's
        // own span, not at its left edge.
        let shifted_x = rects2[0].x + 3;
        let cell = buf_cell(&term2, shifted_x, 1);
        assert_eq!(cell.symbol(), "━");
        assert_eq!(
            cell.fg, theme.accent,
            "the shifted segment is still accent-colored, distinguishing it \
             from the plain hairline it shares a glyph with"
        );
        let at_old_left = buf_cell(&term2, rects2[0].x, 1);
        assert_eq!(
            at_old_left.symbol(),
            "━",
            "the tab's own left edge is now bare hairline — the segment \
             tracks the caller-given offset, not the active tab index"
        );
        assert_eq!(
            at_old_left.fg, theme.hairline,
            "bare hairline, not the accent segment"
        );
    }

    /// A fractional edge that rounds to a half-cell (not a whole one) paints
    /// the box-drawing half glyph there instead of snapping the whole cell
    /// in or out: `╺` (right half) at the left boundary, `╸` (left half) at
    /// the right boundary — matching the reference app's own Bar
    /// renderable, whose track and highlight share one glyph family and
    /// differ only by color.
    #[test]
    fn tabstrip_underline_half_cell_boundaries_use_box_drawing_half_glyphs() {
        let theme = Theme::dark();
        let tabs = vec![("Params".to_string(), None), ("Headers".to_string(), None)];
        let mut term = Terminal::new(TestBackend::new(40, 2)).unwrap();
        let mut rects = Vec::new();
        term.draw(|f| {
            rects = TabStrip {
                tabs: &tabs,
                active: 0,
                hovered: None,
                focused: false,
                // Left edge at column 2.5, right edge at column 6.5: both
                // boundaries land mid-cell.
                underline: (2.5, 4.0),
            }
            .paint(f.buffer_mut(), Rect::new(0, 0, 40, 2), theme.panel, &theme);
        })
        .unwrap();
        let x0 = rects[0].x;
        let left_boundary = buf_cell(&term, x0 + 2, 1);
        assert_eq!(
            left_boundary.symbol(),
            "╺",
            "left boundary shows only its right half"
        );
        assert_eq!(left_boundary.fg, theme.accent);
        for x in 3..=5 {
            let full = buf_cell(&term, x0 + x, 1);
            assert_eq!(full.symbol(), "━", "interior cell {x} stays full");
            assert_eq!(full.fg, theme.accent);
        }
        let right_boundary = buf_cell(&term, x0 + 6, 1);
        assert_eq!(
            right_boundary.symbol(),
            "╸",
            "right boundary shows only its left half"
        );
        assert_eq!(right_boundary.fg, theme.accent);
        // Just outside the segment on either side: bare hairline.
        assert_eq!(buf_cell(&term, x0 + 1, 1).symbol(), "━");
        assert_eq!(buf_cell(&term, x0 + 1, 1).fg, theme.hairline);
        assert_eq!(buf_cell(&term, x0 + 7, 1).fg, theme.hairline);
    }

    #[test]
    fn tabstrip_badge_appends_colored_glyph_after_the_label() {
        let theme = Theme::dark();
        let mut term = Terminal::new(TestBackend::new(40, 2)).unwrap();
        let tabs = vec![("Body".to_string(), Some(('✓', theme.success)))];
        let spans = TabStrip::spans(&tabs);
        let mut rects = Vec::new();
        term.draw(|f| {
            rects = TabStrip {
                tabs: &tabs,
                active: 0,
                hovered: None,
                focused: false,
                underline: (spans[0].0 as f32, spans[0].1 as f32),
            }
            .paint(f.buffer_mut(), Rect::new(0, 0, 40, 2), theme.panel, &theme);
        })
        .unwrap();
        assert_eq!(rects[0].width, " Body ✓ ".chars().count() as u16);
        let glyph = buf_cell(&term, rects[0].x + 6, 0);
        assert_eq!(glyph.symbol(), "✓");
        assert_eq!(glyph.fg, theme.success, "badge keeps its own color");
        assert_eq!(glyph.bg, theme.panel, "flat: badge sits on the surface");
    }

    #[test]
    fn tabstrip_hover_lifts_only_the_hovered_inactive_labels_color() {
        let theme = Theme::dark();
        let tabs = vec![("Params".to_string(), None), ("Headers".to_string(), None)];
        let spans = TabStrip::spans(&tabs);
        let mut term = Terminal::new(TestBackend::new(40, 2)).unwrap();
        let mut rects = Vec::new();
        term.draw(|f| {
            rects = TabStrip {
                tabs: &tabs,
                active: 0,
                hovered: Some(1),
                focused: false,
                underline: (spans[0].0 as f32, spans[0].1 as f32),
            }
            .paint(f.buffer_mut(), Rect::new(0, 0, 40, 2), theme.panel, &theme);
        })
        .unwrap();
        let hovered_cell = buf_cell(&term, rects[1].x + 1, 0);
        assert_eq!(
            hovered_cell.fg, theme.text,
            "the hovered inactive tab's label lifts to theme.text"
        );
        let active_cell = buf_cell(&term, rects[0].x + 1, 0);
        assert_eq!(
            active_cell.fg, theme.text,
            "the active tab stays theme.text"
        );
        assert!(active_cell.modifier.contains(Modifier::BOLD));
        assert!(
            !hovered_cell.modifier.contains(Modifier::BOLD),
            "hover alone isn't bold — only the active tab is"
        );
    }
}

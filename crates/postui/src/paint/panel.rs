//! The floating panel shell: a backdrop dim (for modal-style overlays) plus
//! a filled panel with a soft drop shadow along its right and bottom edges.

use ratatui::{buffer::Buffer, layout::Rect};

use crate::paint::fill;
use crate::paint::ring::ring;
use crate::theme::{Theme, dim55, mix};

/// Dims every cell in `area` — fg and bg each blended toward their `dim55`
/// (55% toward black) counterpart by `t` — used to push a backdrop behind a
/// floating panel out of visual focus. `t == 1.0` is byte-identical to the
/// original always-fully-dimmed behavior (see `theme::mix`'s endpoint
/// short-circuit); `t == 0.0` leaves the backdrop untouched. Modal open
/// drives `t` from `AnimKey::ModalOpen` so the dim fades in rather than
/// snapping; every other caller (steady-state redraws) passes `1.0`.
pub fn dim_backdrop(buf: &mut Buffer, area: Rect, t: f32) {
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            if let Some(cell) = buf.cell_mut((x, y)) {
                let fg = cell.fg;
                let bg = cell.bg;
                cell.set_fg(mix(fg, dim55(fg), t));
                cell.set_bg(mix(bg, dim55(bg), t));
            }
        }
    }
}

/// Fills `area` with `theme.panel`, darkens a 1-cell drop-shadow band along
/// its right edge (offset 1 down) and bottom edge (offset 2 right), clipped
/// to `screen` so the shadow never paints outside the terminal, then strokes
/// a quiet accent ring hugging the panel's inside border.
pub fn floating_panel(buf: &mut Buffer, area: Rect, screen: Rect, theme: &Theme) {
    fill(buf, area, theme.panel);

    let darken = |buf: &mut Buffer, x: u16, y: u16| {
        if x < screen.left() || x >= screen.right() || y < screen.top() || y >= screen.bottom() {
            return;
        }
        if let Some(cell) = buf.cell_mut((x, y)) {
            let fg = cell.fg;
            let bg = cell.bg;
            cell.set_fg(dim55(fg));
            cell.set_bg(dim55(bg));
        }
    };

    // Right-edge shadow band: one column right of the panel, offset one
    // row down from its top.
    let shadow_x = area.right();
    for y in (area.top() + 1)..=area.bottom() {
        darken(buf, shadow_x, y);
    }

    // Bottom-edge shadow band: one row below the panel, offset two columns
    // right of its left edge.
    let shadow_y = area.bottom();
    for x in (area.left() + 2)..=area.right() {
        darken(buf, x, shadow_y);
    }

    ring(buf, area, theme.hairline, theme.panel);
}

/// The modal-open settle: at `t < 1.0`, paints only the panel's growing
/// background — no shadow, no ring, no contents — sized to
/// `lerp(0.8, 1.0, t)` of `area`'s full height and vertically centered on
/// it, with fractional top/bottom edge rows via `frac_vspan` so the growth
/// reads smoothly rather than snapping row-by-row. At `t >= 1.0` this is
/// exactly `floating_panel` (full shell: fill, shadow, ring). Callers draw
/// modal contents only once `t == 1.0` — see `components::modal::draw`.
pub fn floating_panel_settling(buf: &mut Buffer, area: Rect, screen: Rect, theme: &Theme, t: f32) {
    if t >= 1.0 {
        floating_panel(buf, area, screen, theme);
        return;
    }
    let t = t.clamp(0.0, 1.0);
    let full_h = area.height as f32;
    let painted_h = (0.8 + 0.2 * t) * full_h;
    let y0 = area.top() as f32 + (full_h - painted_h) / 2.0;
    let y1 = y0 + painted_h;
    // Approximates what's actually behind the panel (the dimmed backdrop,
    // itself faded in over the same `t`) closely enough for a 2-3 frame
    // transition — see the panel module doc.
    let behind = mix(theme.page, dim55(theme.page), t);
    crate::paint::frac_vspan(buf, area.left(), area.right(), y0, y1, theme.panel, behind);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Theme;
    use ratatui::{Terminal, backend::TestBackend, style::Color};

    fn buf_cell(term: &Terminal<TestBackend>, x: u16, y: u16) -> &ratatui::buffer::Cell {
        term.backend().buffer().cell((x, y)).unwrap()
    }

    #[test]
    fn floating_panel_darkens_shadow_band() {
        let theme = Theme::dark();
        let mut term = Terminal::new(TestBackend::new(20, 10)).unwrap();
        term.draw(|f| {
            fill(f.buffer_mut(), Rect::new(0, 0, 20, 10), theme.page);
            floating_panel(
                f.buffer_mut(),
                Rect::new(2, 2, 10, 5),
                Rect::new(0, 0, 20, 10),
                &theme,
            );
        })
        .unwrap();
        assert_eq!(buf_cell(&term, 5, 4).bg, theme.panel); // panel fill
        let shadow = buf_cell(&term, 12, 3).bg; // band right of panel
        let page = theme.page;
        assert_ne!(shadow, page); // darkened
    }

    #[test]
    fn dim_backdrop_at_full_strength_blends_every_cell_toward_black() {
        let theme = Theme::dark();
        let mut term = Terminal::new(TestBackend::new(10, 5)).unwrap();
        term.draw(|f| {
            fill(f.buffer_mut(), Rect::new(0, 0, 10, 5), theme.page);
            dim_backdrop(f.buffer_mut(), Rect::new(0, 0, 10, 5), 1.0);
        })
        .unwrap();
        assert_ne!(buf_cell(&term, 3, 2).bg, theme.page);
        assert_eq!(buf_cell(&term, 3, 2).bg, dim55(theme.page));
    }

    #[test]
    fn dim_backdrop_at_zero_leaves_the_backdrop_untouched() {
        let theme = Theme::dark();
        let mut term = Terminal::new(TestBackend::new(10, 5)).unwrap();
        term.draw(|f| {
            fill(f.buffer_mut(), Rect::new(0, 0, 10, 5), theme.page);
            dim_backdrop(f.buffer_mut(), Rect::new(0, 0, 10, 5), 0.0);
        })
        .unwrap();
        assert_eq!(buf_cell(&term, 3, 2).bg, theme.page);
    }

    #[test]
    fn dim_backdrop_at_half_strength_lands_strictly_between_undimmed_and_dim55() {
        let theme = Theme::dark();
        let mut term = Terminal::new(TestBackend::new(10, 5)).unwrap();
        term.draw(|f| {
            fill(f.buffer_mut(), Rect::new(0, 0, 10, 5), theme.page);
            dim_backdrop(f.buffer_mut(), Rect::new(0, 0, 10, 5), 0.5);
        })
        .unwrap();
        let bg = buf_cell(&term, 3, 2).bg;
        assert_ne!(
            bg, theme.page,
            "half strength must move off the undimmed color"
        );
        assert_ne!(
            bg,
            dim55(theme.page),
            "half strength must not already be fully dimmed"
        );
        // Both endpoints are `Color::Rgb`, so the blend is channel-wise
        // monotone between them — a stronger assertion than merely "not
        // equal to either endpoint".
        let (Color::Rgb(pr, pg, pb), Color::Rgb(dr, dg, db), Color::Rgb(hr, hg, hb)) =
            (theme.page, dim55(theme.page), bg)
        else {
            panic!("expected Rgb colors");
        };
        let between = |p: u8, d: u8, h: u8| (p.min(d)..=p.max(d)).contains(&h);
        assert!(between(pr, dr, hr));
        assert!(between(pg, dg, hg));
        assert!(between(pb, db, hb));
    }

    #[test]
    fn floating_panel_paints_a_hairline_ring_on_its_inside_border() {
        let theme = Theme::dark();
        let mut term = Terminal::new(TestBackend::new(20, 10)).unwrap();
        term.draw(|f| {
            fill(f.buffer_mut(), Rect::new(0, 0, 20, 10), theme.page);
            floating_panel(
                f.buffer_mut(),
                Rect::new(2, 2, 10, 5),
                Rect::new(0, 0, 20, 10),
                &theme,
            );
        })
        .unwrap();
        // Top edge (non-corner column), inside the panel's own border row.
        let top = buf_cell(&term, 5, 2);
        assert_eq!(top.symbol(), "─");
        assert_eq!(top.fg, theme.hairline);
        assert_eq!(top.bg, theme.panel);
        // Left edge (non-corner row).
        let left = buf_cell(&term, 2, 4);
        assert_eq!(left.symbol(), "│");
        assert_eq!(left.fg, theme.hairline);
        assert_eq!(left.bg, theme.panel);
    }

    #[test]
    fn shadow_bands_clip_to_screen_without_panic() {
        let theme = Theme::dark();
        let mut term = Terminal::new(TestBackend::new(12, 7)).unwrap();
        term.draw(|f| {
            fill(f.buffer_mut(), Rect::new(0, 0, 12, 7), theme.page);
            // panel touches the right/bottom screen edge, so its shadow
            // bands fall entirely outside `screen` — must not panic.
            floating_panel(
                f.buffer_mut(),
                Rect::new(2, 2, 10, 5),
                Rect::new(0, 0, 12, 7),
                &theme,
            );
        })
        .unwrap();
        assert_eq!(buf_cell(&term, 5, 4).bg, theme.panel);
    }
}

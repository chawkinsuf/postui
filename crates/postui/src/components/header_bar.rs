use crate::hit::{Hit, HitMap};
use crate::paint::{fill, text};
use crate::theme::Theme;
use ratatui::Frame;
use ratatui::layout::Rect;

/// The app bar is always exactly this many rows tall: a blank panel row on
/// top, the content row (wordmark + chips), a blank panel row on the
/// bottom — matching the painted 3-row rhythm of buttons/fields elsewhere.
pub const HEADER_HEIGHT: u16 = 3;

/// Paints the app bar: a flat `theme.panel` fill across all 3 rows, the bold
/// `postui` wordmark, and the project/env selectors as single-row
/// `theme.control`-filled chips (lifting to `theme.control_hover` while
/// hovered) with a trailing `▾` marker. Registers the same
/// [`Hit::HeaderProject`]/[`Hit::HeaderEnv`] hits on the chip rects.
#[allow(clippy::too_many_arguments)]
pub fn draw_header(
    frame: &mut Frame,
    area: Rect,
    theme: &Theme,
    project: &str,
    env: &str,
    hits: &mut HitMap,
    hovered: Option<&Hit>,
) {
    let buf = frame.buffer_mut();
    fill(buf, area, theme.panel);

    if area.height == 0 {
        return;
    }
    let mid_y = area.y + area.height / 2;

    let wordmark = "postui";
    let wordmark_x = area.x + 3;
    text(
        buf,
        wordmark_x,
        mid_y,
        wordmark,
        theme.text,
        theme.panel,
        true,
    );

    let mut x = wordmark_x + wordmark.chars().count() as u16 + 3;

    let project_label = format!(" {project} \u{25be} ");
    let project_w = project_label.chars().count() as u16;
    let project_rect = Rect {
        x,
        y: mid_y,
        width: project_w,
        height: 1,
    };
    let project_bg = if hovered == Some(&Hit::HeaderProject) {
        theme.control_hover
    } else {
        theme.control
    };
    fill(buf, project_rect, project_bg);
    text(
        buf,
        project_rect.x,
        mid_y,
        &project_label,
        theme.text,
        project_bg,
        false,
    );
    hits.register(project_rect, Hit::HeaderProject);
    x += project_w + 1;

    let env_label = format!(" {env} \u{25be} ");
    let env_w = env_label.chars().count() as u16;
    let env_rect = Rect {
        x,
        y: mid_y,
        width: env_w,
        height: 1,
    };
    let env_bg = if hovered == Some(&Hit::HeaderEnv) {
        theme.control_hover
    } else {
        theme.control
    };
    fill(buf, env_rect, env_bg);
    text(
        buf,
        env_rect.x,
        mid_y,
        &env_label,
        theme.text_muted,
        env_bg,
        false,
    );
    hits.register(env_rect, Hit::HeaderEnv);
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
        let backend = TestBackend::new(60, HEADER_HEIGHT);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = HitMap::default();
        terminal
            .draw(|f: &mut Frame| draw_header(f, f.area(), theme, project, env, &mut hits, hovered))
            .unwrap();
        (terminal, hits)
    }

    fn cell(term: &Terminal<TestBackend>, x: u16, y: u16) -> ratatui::buffer::Cell {
        term.backend().buffer().cell((x, y)).unwrap().clone()
    }

    #[test]
    fn wordmark_is_bold_on_panel_background() {
        let theme = Theme::dark();
        let (term, _hits) = render(&theme, "alpha", "qa", None);
        let c = cell(&term, 3, 1);
        assert_eq!(c.symbol(), "p");
        assert_eq!(c.fg, theme.text);
        assert_eq!(c.bg, theme.panel);
        assert!(c.modifier.contains(Modifier::BOLD));
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
        assert_eq!(c.symbol(), "a"); // first char of "alpha"
    }

    #[test]
    fn env_chip_uses_muted_text_on_control_fill() {
        let theme = Theme::dark();
        let (term, hits) = render(&theme, "alpha", "qa", None);
        let rect = hits.rect_of(&Hit::HeaderEnv).expect("env hit registered");
        let c = cell(&term, rect.x + 1, rect.y);
        assert_eq!(c.bg, theme.control);
        assert_eq!(c.fg, theme.text_muted);
        assert_eq!(c.symbol(), "q"); // first char of "qa"
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
}

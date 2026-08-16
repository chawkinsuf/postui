use crate::hit::{Hit, HitMap};
use crate::theme::Theme;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

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
    let brand = " postui ";
    let brand_gap = "  ";
    let project_label = format!("{project} \u{25be}");
    let sep = " \u{b7} ";
    let env_label = format!("{env} \u{25be}");

    let base_env_style = if env == "no env" {
        Style::default().fg(theme.text_muted).italic()
    } else {
        Style::default().fg(theme.text_muted)
    };
    let project_style = if hovered == Some(&Hit::HeaderProject) {
        Style::default()
            .fg(theme.text)
            .add_modifier(Modifier::UNDERLINED)
    } else {
        Style::default().fg(theme.text)
    };
    let env_style = if hovered == Some(&Hit::HeaderEnv) {
        base_env_style.add_modifier(Modifier::UNDERLINED)
    } else {
        base_env_style
    };

    let line = Line::from(vec![
        Span::styled(
            brand,
            Style::default().fg(theme.surface).bg(theme.accent).bold(),
        ),
        Span::raw(brand_gap),
        Span::styled(project_label.clone(), project_style),
        Span::styled(sep, Style::default().fg(theme.text_muted)),
        Span::styled(env_label.clone(), env_style),
    ]);
    frame.render_widget(
        Paragraph::new(line).style(Style::default().bg(theme.surface_raised)),
        area,
    );

    let prefix_w = (brand.chars().count() + brand_gap.chars().count()) as u16;
    let project_w = project_label.chars().count() as u16;
    let sep_w = sep.chars().count() as u16;
    let env_w = env_label.chars().count() as u16;

    let project_rect = Rect {
        x: area.x + prefix_w,
        y: area.y,
        width: project_w,
        height: 1,
    };
    let env_rect = Rect {
        x: area.x + prefix_w + project_w + sep_w,
        y: area.y,
        width: env_w,
        height: 1,
    };
    hits.register(project_rect, Hit::HeaderProject);
    hits.register(env_rect, Hit::HeaderEnv);
}

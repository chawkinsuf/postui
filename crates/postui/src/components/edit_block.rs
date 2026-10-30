//! The edit flag's waiting screen (spec 2026-10-28 §4, piece 3): a card in
//! the middle of an emptied frame saying an agent is editing the project
//! the app is waiting to open, and the three ways on. Drawn over everything
//! but dialogs and toasts; its state and keys live in `app/edit_block.rs`.

use crate::app::{EditBlock, EditBlockButton};
use crate::hit::{Hit, HitMap};
use crate::paint;
use crate::theme::Theme;
use ratatui::Frame;
use ratatui::layout::Rect;
use std::time::{Duration, SystemTime};

/// The gap between two buttons.
const BUTTON_GAP: u16 = 3;

/// How old a flag is before the card says the agent may have stopped
/// (mine): longer than an agent's edit of TOML files takes.
const OLD_FLAG: Duration = Duration::from_secs(30 * 60);

/// What the card says once a flag is [`OLD_FLAG`] old.
const OLD_HINT: &str = "If the agent has stopped, Override opens it now.";

/// The age the card is sized for, the widest it says in practice, so the
/// card and its buttons stay put as the age ticks on (the user's "no layout
/// shifts").
const WIDEST_AGE: &str = "started 59 minutes ago";

/// The card's first line: who is editing and since when.
fn who(name: &str, started: Option<&str>) -> String {
    match started {
        Some(started) => format!("An agent is editing {name} ({started})."),
        None => format!("An agent is editing {name}."),
    }
}

/// The lines the card says, top to bottom, each with whether it is muted:
/// who is editing and since when, the note, what happens next, and a row
/// for an old flag's hint, that it may be left behind, kept (empty) while
/// the flag is young so the card's height doesn't change when it comes.
fn lines(block: &EditBlock, name: &str, now: SystemTime) -> Vec<(String, bool)> {
    let started = block.flag.as_ref().map(|f| f.started_text(now));
    let mut lines = vec![(who(name, started.as_deref()), false)];
    if let Some(note) = block.flag.as_ref().and_then(|f| f.note.as_deref()) {
        lines.push((format!("\"{}\"", shown(note)), true));
    }
    lines.push((String::new(), false));
    lines.push(("postui opens it when the agent finishes.".into(), true));
    let old = block.flag.as_ref().and_then(|f| f.age(now)).is_some_and(|age| age >= OLD_FLAG);
    lines.push((if old { OLD_HINT } else { "" }.into(), true));
    lines
}

/// The agent's note with its control chars (a line break, an escape) shown
/// as their codes, as a field shows them (`^J`, `^[`): painted as they are,
/// ratatui would drop them without a trace.
fn shown(note: &str) -> String {
    note.chars()
        .map(
            |c| if c.is_control() { crate::components::line_input::char_code(c) } else { c.into() },
        )
        .collect()
}

/// One button's keycap and label, as `paint::keycap_button` paints them.
fn button_parts(button: EditBlockButton) -> (String, String) {
    (button.key().to_string(), format!(" {} ", button.label()))
}

/// What `paint::keycap_button` will take for `button`.
fn button_width(button: EditBlockButton, theme: &Theme) -> u16 {
    let (key, label) = button_parts(button);
    paint::chip::Chip { label: &key, color: theme.accent }.width() + crate::ui::cells(&label) as u16
}

/// Paints the screen over all of `screen` and registers its buttons.
pub fn draw(
    frame: &mut Frame,
    screen: Rect,
    theme: &Theme,
    block: &EditBlock,
    hits: &mut HitMap,
    hovered: Option<&Hit>,
    hover_t: f32,
) {
    let buf = frame.buffer_mut();
    paint::fill(buf, screen, theme.page);
    let title = "Updates in progress";
    let body = lines(block, &block.name, SystemTime::now());
    let buttons: u16 = EditBlockButton::ALL.iter().map(|b| button_width(*b, theme)).sum::<u16>()
        + BUTTON_GAP * (EditBlockButton::ALL.len() as u16 - 1);
    // As wide as the widest age and the old flag's hint would make it, so
    // neither moves the buttons; no wider than the screen, since the
    // agent's note can be any length.
    let fixed = [who(&block.name, Some(WIDEST_AGE)), OLD_HINT.to_string()];
    let text_w = body.iter().map(|(l, _)| l).chain(&fixed).map(|l| crate::ui::cells(l));
    let text_w = text_w.max().unwrap_or(0);
    let text_w = text_w.min(usize::from(screen.width)) as u16;
    // Two cells of ring and padding each side.
    let width = text_w.max(buttons).max(crate::ui::cells(title) as u16).saturating_add(6);
    // Title, blank, the body, blank, the buttons, and a row of padding
    // above and below inside the ring.
    let height = body.len() as u16 + 6;
    let card = crate::components::modal::centered_rect(screen, width, height);
    paint::panel::floating_panel(buf, card, screen, theme);
    let inner = Rect { x: card.x + 3, width: card.width.saturating_sub(6), ..card };
    // Inside the ring: the title on the first row, the buttons, the way
    // out, on the last, whatever the terminal's height; the body takes what
    // is left between them, a line short of the buttons, and loses its last
    // lines first.
    if card.height < 3 {
        return;
    }
    let (top, last) = (card.y + 1, card.bottom() - 2);
    let centred = |s: &str| inner.x + inner.width.saturating_sub(crate::ui::cells(s) as u16) / 2;
    let title = crate::ui::clip(title, inner.width);
    paint::text(buf, centred(title), top, title, theme.text, theme.panel, true);
    for ((line, muted), y) in body.iter().zip(top + 2..last.saturating_sub(1)) {
        let line = crate::ui::clip(line, inner.width);
        let fg = if *muted { theme.text_muted } else { theme.text };
        paint::text(buf, centred(line), y, line, fg, theme.panel, false);
    }
    if last <= top {
        return;
    }
    let y = last;
    let mut x = inner.x + inner.width.saturating_sub(buttons) / 2;
    for button in EditBlockButton::ALL {
        let (key, label) = button_parts(button);
        if x + button_width(button, theme) > inner.right() {
            break;
        }
        let hit = Hit::EditBlock(button);
        let painted = paint::keycap_button(
            buf,
            inner,
            x,
            y,
            &key,
            &label,
            hovered == Some(&hit),
            hover_t,
            theme,
        );
        hits.register(painted, hit);
        x += painted.width + BUTTON_GAP;
    }
}

//! The icon glyphs, in one place: Nerd Font Material (`nf-md-*`) code
//! points, every one exactly one terminal cell wide. Icons are drawn from
//! here rather than typed inline so the family stays consistent and the
//! one-cell rule (ratatui never paints a wide glyph's second cell, and
//! some terminals then leave it stale) is checked once, below.
//! Typography — carets, arrows, ticks, radios, the spinner — stays plain
//! Unicode and lives with the code that draws it.

pub const COPY: &str = "\u{F018F}"; // 󰆏 nf-md-content_copy
pub const SAVE: &str = "\u{F0193}"; // 󰆓 nf-md-content_save
pub const SEARCH: &str = "\u{F0349}"; // 󰍉 nf-md-magnify
pub const PENCIL: &str = "\u{F03EB}"; // 󰏫 nf-md-pencil
pub const FILTER: &str = "\u{F0232}"; // 󰈲 nf-md-filter
pub const LOCK: &str = "\u{F033E}"; // 󰌾 nf-md-lock
pub const LOCK_OPEN: &str = "\u{F0340}"; // 󰍀 nf-md-lock_outline
pub const DELETE: &str = "\u{F01B4}"; // 󰆴 nf-md-delete
pub const CLOSE: &str = "\u{F0156}"; // 󰅖 nf-md-close
pub const EYE: &str = "\u{F06D0}"; // 󰈈 nf-md-eye
pub const EYE_OFF: &str = "\u{F06D1}"; // 󰈉 nf-md-eye_off
pub const CREATION: &str = "\u{F0674}"; // 󰙴 nf-md-creation
pub const FOLDER: &str = "\u{F024B}"; // 󰉋 nf-md-folder

/// Every icon, for the width check.
pub const ALL: [&str; 13] = [
    COPY, SAVE, SEARCH, PENCIL, FILTER, LOCK, LOCK_OPEN, DELETE, CLOSE, EYE, EYE_OFF, CREATION,
    FOLDER,
];

#[cfg(test)]
mod tests {
    use unicode_width::UnicodeWidthStr;

    /// The one-cell rule: `unicode-width` (what ratatui uses to advance
    /// the cursor) must count each icon as exactly one column.
    #[test]
    fn every_icon_is_one_cell_wide() {
        for g in super::ALL {
            assert_eq!(
                g.width(),
                1,
                "{g:?} ({:#X})",
                g.chars().next().unwrap() as u32
            );
            assert_eq!(g.chars().count(), 1, "{g:?} is a single scalar");
        }
    }
}

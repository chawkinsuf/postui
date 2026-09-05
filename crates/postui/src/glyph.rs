//! The icon glyphs, in one place: Nerd Font Material (`nf-md-*`) code
//! points, every one exactly one terminal cell wide. Icons are drawn from
//! here rather than typed inline so the family stays consistent and the
//! one-cell rule (ratatui never paints a wide glyph's second cell, and
//! some terminals then leave it stale) is checked once, below.
//! Typography — carets, arrows, ticks, radios, the spinner — stays plain
//! Unicode and lives with the code that draws it.
//!
//! Each icon comes with a `*_PILL` twin — the glyph in a three-cell
//! ` 󰆏 ` pill, the shape every hoverable icon control is painted in — as
//! a constant, so per-frame painters need no allocation.

macro_rules! icons {
    ($($name:ident / $pill:ident = $glyph:literal, $padded:literal, $doc:literal;)*) => {
        $(
            #[doc = $doc]
            pub const $name: &str = $glyph;
            #[doc = concat!("` ", $doc, " ` in a three-cell pill.")]
            pub const $pill: &str = $padded;
        )*
        /// Every `(icon, pill)` pair, for the width and padding checks.
        #[cfg(test)]
        const ALL: &[(&str, &str)] = &[$(($name, $pill)),*];
    };
}

icons! {
    COPY / COPY_PILL = "\u{F018F}", " \u{F018F} ", "󰆏 nf-md-content_copy";
    SAVE / SAVE_PILL = "\u{F0193}", " \u{F0193} ", "󰆓 nf-md-content_save";
    SEARCH / SEARCH_PILL = "\u{F0349}", " \u{F0349} ", "󰍉 nf-md-magnify";
    PENCIL / PENCIL_PILL = "\u{F03EB}", " \u{F03EB} ", "󰏫 nf-md-pencil";
    FILTER / FILTER_PILL = "\u{F0232}", " \u{F0232} ", "󰈲 nf-md-filter";
    LOCK / LOCK_PILL = "\u{F033E}", " \u{F033E} ", "󰌾 nf-md-lock";
    LOCK_OPEN / LOCK_OPEN_PILL = "\u{F0340}", " \u{F0340} ", "󰍀 nf-md-lock_outline";
    DELETE / DELETE_PILL = "\u{F01B4}", " \u{F01B4} ", "󰆴 nf-md-delete";
    CLOSE / CLOSE_PILL = "\u{F0156}", " \u{F0156} ", "󰅖 nf-md-close";
    EYE / EYE_PILL = "\u{F06D0}", " \u{F06D0} ", "󰈈 nf-md-eye";
    EYE_OFF / EYE_OFF_PILL = "\u{F06D1}", " \u{F06D1} ", "󰈉 nf-md-eye_off";
    CREATION / CREATION_PILL = "\u{F0674}", " \u{F0674} ", "󰙴 nf-md-creation";
    FOLDER / FOLDER_PILL = "\u{F024B}", " \u{F024B} ", "󰉋 nf-md-folder";
}

#[cfg(test)]
mod tests {
    use unicode_width::UnicodeWidthStr;

    /// The one-cell rule: `unicode-width` (what ratatui uses to advance
    /// the cursor) must count each icon as exactly one column — and each
    /// pill as exactly that icon with one blank cell either side.
    #[test]
    fn every_icon_is_one_cell_and_its_pill_pads_it() {
        for (icon, pill) in super::ALL {
            let cp = icon.chars().next().unwrap() as u32;
            assert_eq!(icon.chars().count(), 1, "{icon:?} is a single scalar");
            assert_eq!(icon.width(), 1, "{icon:?} ({cp:#X}) is one cell");
            assert_eq!(*pill, format!(" {icon} "), "{icon:?}'s pill");
        }
    }
}

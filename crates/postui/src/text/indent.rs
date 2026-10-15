//! A body's indentation (spec 2026-10-15 editor indent): the configured
//! default, and what a body's own text says it uses. Profile-neutral, like
//! the rest of `text`: the arrows body, Format and the vim seam all read it.

use super::Lines;

/// One indent level: `n` spaces, or one tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Indent {
    Spaces(usize),
    Tabs,
}

/// The widest space indent the setting accepts and detection reports.
pub const MAX_WIDTH: usize = 8;

impl Indent {
    /// `config.toml`'s defaults: `indent = "spaces"`, `indent_width = 2`.
    pub const DEFAULT: Indent = Indent::Spaces(2);

    /// The text of one level.
    pub fn unit(self) -> String {
        match self {
            Indent::Spaces(n) => " ".repeat(n),
            Indent::Tabs => "\t".to_string(),
        }
    }
}

/// What `text`'s indented lines use (spec R4): tabs when more content lines
/// start with a tab than with a space, spaces when more start with a space,
/// at the width that occurs most often as the step between consecutive
/// non-tab content lines' indents (from a virtual unindented line before
/// the first; steps over [`MAX_WIDTH`] are not widths; a tie goes to the
/// smaller width). `None` when it can't say: a tie, or no indented line.
/// Reads the lines in place: the body asks on every key that needs it
/// (spec amendment, review round 1).
pub fn detect(text: &(impl Lines + ?Sized)) -> Option<Indent> {
    let (mut tab_led, mut space_led) = (0usize, 0usize);
    let mut steps = [0usize; MAX_WIDTH + 1];
    let mut prev = 0usize;
    for row in 0..text.line_count() {
        let line = text.line(row);
        if line.iter().all(|c| matches!(c, ' ' | '\t' | '\r')) {
            continue;
        }
        if line[0] == '\t' {
            tab_led += 1;
            continue;
        }
        let n = line.iter().take_while(|&&c| c == ' ').count();
        if n > 0 {
            space_led += 1;
        }
        let step = n.abs_diff(prev);
        if (1..=MAX_WIDTH).contains(&step) {
            steps[step] += 1;
        }
        prev = n;
    }
    match tab_led.cmp(&space_led) {
        std::cmp::Ordering::Greater => Some(Indent::Tabs),
        std::cmp::Ordering::Equal => None,
        std::cmp::Ordering::Less => {
            let width = (1..=MAX_WIDTH).max_by_key(|&w| (steps[w], std::cmp::Reverse(w)))?;
            (steps[width] > 0).then_some(Indent::Spaces(width))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn detect_text(text: &str) -> Option<Indent> {
        detect(&edtui::Lines::from(text))
    }

    #[test]
    fn two_space_json_detects_two() {
        assert_eq!(detect_text("{\n  \"a\": {\n    \"b\": 1\n  }\n}"), Some(Indent::Spaces(2)));
    }

    #[test]
    fn four_space_json_detects_four() {
        assert_eq!(detect_text("{\n    \"a\": [\n        1\n    ]\n}"), Some(Indent::Spaces(4)));
    }

    #[test]
    fn tab_json_detects_tabs() {
        assert_eq!(detect_text("{\n\t\"a\": {\n\t\t\"b\": 1\n\t}\n}"), Some(Indent::Tabs));
    }

    #[test]
    fn a_tie_between_tab_and_space_lines_detects_nothing() {
        assert_eq!(detect_text("{\n\t\"a\": 1,\n  \"b\": 2\n}"), None);
    }

    #[test]
    fn minified_and_empty_text_detect_nothing() {
        assert_eq!(detect_text("{\"a\":{\"b\":1}}"), None);
        assert_eq!(detect_text(""), None);
    }

    #[test]
    fn one_indented_line_measures_from_column_zero() {
        assert_eq!(detect_text("   x"), Some(Indent::Spaces(3)));
    }

    #[test]
    fn deep_closing_dedents_do_not_skew_the_width() {
        // Indents 0 2 4 6 0: steps 2 2 2 6 -> 2.
        assert_eq!(detect_text("{\n  [\n    [\n      1\n]]}"), Some(Indent::Spaces(2)));
    }

    #[test]
    fn blank_and_whitespace_only_lines_are_ignored() {
        assert_eq!(detect_text("{\n\n        \n    \"a\": 1\n\t\n}"), Some(Indent::Spaces(4)));
    }

    #[test]
    fn a_tab_after_leading_spaces_counts_as_space_led() {
        assert_eq!(detect_text("  \tx\n  y"), Some(Indent::Spaces(2)));
    }

    #[test]
    fn crlf_text_detects_like_lf() {
        assert_eq!(detect_text("{\r\n    \"a\": 1\r\n}"), Some(Indent::Spaces(4)));
        assert_eq!(detect_text("{\r\n\r\n}"), None, "a lone CR is a blank line");
    }

    #[test]
    fn steps_past_the_max_width_are_not_widths() {
        assert_eq!(detect_text("          x"), None, "a lone 10-space indent names no width");
    }

    #[test]
    fn a_width_tie_goes_to_the_smaller_width() {
        // Indents 0 2 6: steps 2 and 4, once each.
        assert_eq!(detect_text("a\n  b\n      c"), Some(Indent::Spaces(2)));
    }

    #[test]
    fn unit_is_one_level() {
        assert_eq!(Indent::Spaces(4).unit(), "    ");
        assert_eq!(Indent::Tabs.unit(), "\t");
    }
}

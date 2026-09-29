//! The engine's pinned Vim settings (spec §3.4). `generate.py` reads
//! `SETTINGS_LINE` from this file (keep it one string literal on one line)
//! and runs Vim with it; the golden header repeats it and the conformance
//! test asserts the two match, so changing a value here without
//! regenerating the golden file fails loudly.

/// The exact `:set` arguments the oracle runs Vim with (spec §3.4, plus
/// deviation 8: `cpoptions`, `formatoptions`, `virtualedit` and the
/// search options, all Vim defaults, pinned).
pub const SETTINGS_LINE: &str = "expandtab shiftwidth=2 autoindent selection=inclusive tabstop=2 softtabstop=0 backspace=indent,eol,start whichwrap=b,s startofline nojoinspaces nrformats=bin,hex noshiftround nosmartindent nocindent textwidth=0 noignorecase notildeop matchpairs=(:),{:},[:] iskeyword=@,48-57,_,192-255 cpoptions=aABceFs formatoptions=tcq virtualedit= wrapscan magic nosmartcase";

/// `shiftwidth`: one `>` step and the autoindent unit.
#[allow(dead_code)] // used from Task 11
pub const SHIFTWIDTH: usize = 2;
/// `tabstop`: a tab's width in virtual columns; Insert `Tab` (with
/// `expandtab`) fills spaces to the next multiple.
pub const TABSTOP: usize = 2;
/// `undolevels`: the most steps one buffer's history keeps.
pub const UNDOLEVELS: usize = 1000;
/// The largest count a key sequence can build (vim-mode's `seq.rs`).
pub const MAX_COUNT: usize = 9_999;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_constants_agree_with_the_settings_line() {
        let words: Vec<&str> = SETTINGS_LINE.split(' ').collect();
        assert!(words.contains(&format!("shiftwidth={SHIFTWIDTH}").as_str()));
        assert!(words.contains(&format!("tabstop={TABSTOP}").as_str()));
        assert!(!SETTINGS_LINE.contains('\n') && !SETTINGS_LINE.contains('"'));
    }
}

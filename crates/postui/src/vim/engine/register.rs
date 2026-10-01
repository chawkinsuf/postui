//! Registers (spec §3.10). Tier 1 has only the unnamed register, shared by
//! every field and the body; tier 2 adds "0 (plan 3b) and "+ (plan 3c).

/// Charwise or linewise, as Vim's `getregtype()` says `v` or `V`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RegKind {
    #[default]
    Char,
    Line,
}

/// A register's text as `getreg()` reports it: a linewise register ends
/// with `'\n'`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Register {
    pub text: String,
    pub kind: RegKind,
}

/// Vim's unnamed register is whichever register was written last (its
/// `y_previous`); only its text matters here, so it is kept as a copy next
/// to `"0`, the yank register (tier 2, plan 3b). Plan 3c adds `"+`.
#[derive(Debug, Clone, Default)]
pub struct Registers {
    unnamed: Register,
    zero: Register,
}

impl Registers {
    pub fn unnamed(&self) -> &Register {
        &self.unnamed
    }

    pub fn set_unnamed(&mut self, reg: Register) {
        self.unnamed = reg;
    }

    /// `"0`: the last yank.
    pub fn zero(&self) -> &Register {
        &self.zero
    }

    /// The register a command reads: `"0`, or the unnamed one (`None` or
    /// `"`). The parser refuses every other name (spec §3.10).
    pub(crate) fn read(&self, name: Option<char>) -> &Register {
        match name {
            Some('0') => &self.zero,
            _ => {
                debug_assert!(matches!(name, None | Some('"')), "register {name:?}");
                &self.unnamed
            }
        }
    }

    /// A yank (Vim's `op_yank()`): `"0` and the unnamed register.
    pub(crate) fn yank(&mut self, name: Option<char>, reg: Register) {
        debug_assert!(matches!(name, None | Some('"' | '0')), "register {name:?}");
        self.zero = reg.clone();
        self.unnamed = reg;
    }

    /// A delete or change (Vim's `op_delete()`): the unnamed register (Vim
    /// writes `"1` or `"-` and points the unnamed register there), and `"0`
    /// too when the command named `"` or `0` (Vim's `get_yank_register()`
    /// maps both to register 0). `_` is the black hole, used only inside
    /// the engine (Visual `P` and its `.`): nothing is written.
    pub(crate) fn delete(&mut self, name: Option<char>, reg: Register) {
        match name {
            Some('_') => {}
            Some('"' | '0') => {
                self.zero = reg.clone();
                self.unnamed = reg;
            }
            _ => {
                debug_assert!(name.is_none(), "register {name:?}");
                self.unnamed = reg;
            }
        }
    }
}

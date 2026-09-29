//! Registers (spec §3.10). Tier 1 has only the unnamed register, shared by
//! every field and the body. Plan 3b adds `"0`; 3c adds `"+`.

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

#[derive(Debug, Clone, Default)]
pub struct Registers {
    unnamed: Register,
}

impl Registers {
    pub fn unnamed(&self) -> &Register {
        &self.unnamed
    }

    pub fn set_unnamed(&mut self, reg: Register) {
        self.unnamed = reg;
    }
}

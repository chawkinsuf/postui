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

    /// The register a command named (`None` or `"`: the unnamed one).
    /// Other names never get here: the parser refuses them (spec §3.10).
    #[allow(dead_code)] // used from Task 10
    pub(crate) fn read(&self, name: Option<char>) -> &Register {
        debug_assert!(matches!(name, None | Some('"')), "register {name:?}");
        &self.unnamed
    }

    /// What a delete, change or yank leaves behind. Plan 3b splits yanks
    /// off to `"0`.
    pub(crate) fn write(&mut self, name: Option<char>, reg: Register) {
        debug_assert!(matches!(name, None | Some('"')), "register {name:?}");
        self.unnamed = reg;
    }
}

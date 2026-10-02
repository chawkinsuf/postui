//! The key router (piece 4 spec §3): where a key lands, what a key means
//! there, and what is half-typed. `App` builds the context once per key
//! (`app/router.rs`); everything in this module is pure.

pub mod ctx;

pub use ctx::{FieldCaret, FieldId, Surface};

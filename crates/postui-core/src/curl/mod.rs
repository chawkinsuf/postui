//! Curl commands in and out (spec
//! `docs/superpowers/specs/2026-10-11-curl-import-export-design.md`): a
//! prepared request becomes a command a shell can run. Pure functions:
//! no file access, no app state.

mod export;

pub use export::{includes_secrets, to_curl};

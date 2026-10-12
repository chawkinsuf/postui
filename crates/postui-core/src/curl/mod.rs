//! Curl commands in and out (spec
//! `docs/superpowers/specs/2026-10-11-curl-import-export-design.md`): a
//! pasted command becomes requests (`parse`), a prepared request becomes
//! a command a shell can run (`to_curl`). Pure functions: no file access,
//! no app state.

mod export;
mod import;
mod shell;

pub use export::{includes_secrets, to_curl};
pub use import::{CurlBatch, CurlError, CurlErrorKind, CurlImport, parse};

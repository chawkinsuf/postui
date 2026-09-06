//! The open project: one object that owns every read and write of the
//! project's files, holds the parsed documents, and records its own undo
//! journal. See docs/superpowers/specs/2026-09-05-project-file-access-design.md.
//!
//! `legacy` holds the stateless free functions the app still calls; they
//! are deleted once the app has migrated (stage 3). New code never calls
//! their disk paths, only their pure text helpers.

mod legacy;
pub use legacy::*;

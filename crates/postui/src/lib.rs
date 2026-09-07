pub mod action;
pub mod ai;
pub mod anim;
pub mod app;
pub mod clipboard;
pub mod components;
pub mod config;
pub mod glyph;
pub mod hint;
pub mod hit;
pub mod hostfs;
pub mod http;
pub mod keydump;
pub mod keys;
pub mod layout;
pub mod paint;
pub mod session;
pub mod setup;
pub mod split;
pub mod theme;
pub mod ui;
pub mod undo;
pub mod usage;

#[cfg(test)]
mod fs_lint_test {
    #[test]
    fn std_fs_appears_only_in_hostfs_and_test_code() {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let hits = postui_core::fs_lint::check(&src, &["hostfs.rs"]);
        assert!(hits.is_empty(), "std::fs outside hostfs:\n{}", hits.join("\n"));
    }
}

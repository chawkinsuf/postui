pub mod action;
pub mod ai;
pub mod anim;
pub mod app;
pub mod clipboard;
pub mod cmdline;
pub mod components;
pub mod config;
pub mod curl_import;
pub mod glyph;
pub mod hint;
pub mod hit;
pub mod hostfs;
pub mod http;
pub mod keydump;
pub mod keyroute;
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
pub mod vim;
pub mod yank;

#[cfg(test)]
mod fs_lint_test {
    #[test]
    fn std_fs_appears_only_in_hostfs_and_test_code() {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let hits = postui_core::fs_lint::check(&src, &["hostfs.rs"]);
        assert!(hits.is_empty(), "std::fs outside hostfs:\n{}", hits.join("\n"));
    }
}

/// The vim profile stays behind one seam, so it can be taken out whole:
/// outside the vim side, no code names the engine, its glue, `App::vim`,
/// or the seam's own internals. The rest of the App reaches vim only
/// through the seam's `vim_*` methods (`app/vim_seam.rs`).
#[cfg(test)]
mod vim_boundary_test {
    use std::path::{Path, PathBuf};

    /// The vim side: the engine, the key router that hosts its glue, and
    /// the App's seam.
    fn vim_side(rel: &str) -> bool {
        rel.starts_with("vim/") || rel.starts_with("keyroute/") || rel == "app/vim_seam.rs"
    }

    /// Shared components that use the engine's code itself, each for a
    /// reason, and so may name it (but not `App::vim` or the seam's
    /// internals): the response viewer's window and search follow Vim in
    /// both profiles (`View`, `Rows`, the pattern compiler, the last
    /// pattern it is lent); the body editor draws the engine's Visual span
    /// and search highlight.
    const ENGINE_USERS: [&str; 2] = ["components/response.rs", "components/editor.rs"];

    const ENGINE: [&str; 3] = ["crate::vim::", "vim::engine", "engine_glue"];
    const SEAM: [&str; 5] = [".vim.", "glue_of(", "live_engine_field(", "session_parts(", "sync_session("];

    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, out)
            } else if p.extension().is_some_and(|x| x == "rs") {
                out.push(p)
            }
        }
    }

    #[test]
    fn only_the_vim_side_names_the_engine_or_the_seam_internals() {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        walk(&src, &mut files);
        let mut hits = Vec::new();
        for file in files {
            let rel = file.strip_prefix(&src).unwrap().to_string_lossy().replace('\\', "/");
            // Test code reaches in on purpose: it checks what the seam did.
            if vim_side(&rel) || rel.ends_with("tests.rs") || rel == "lib.rs" {
                continue;
            }
            let banned: Vec<&str> = if ENGINE_USERS.contains(&rel.as_str()) {
                SEAM.to_vec()
            } else {
                ENGINE.iter().chain(SEAM.iter()).copied().collect()
            };
            let text = std::fs::read_to_string(&file).unwrap();
            let lines: Vec<&str> = text.lines().collect();
            for (i, line) in lines.iter().enumerate() {
                // A test module at the bottom of a file ends the code.
                if line.trim() == "#[cfg(test)]" && lines.get(i + 1).is_some_and(|n| n.trim_start().starts_with("mod ")) {
                    break;
                }
                if line.trim_start().starts_with("//") {
                    continue;
                }
                if let Some(word) = banned.iter().find(|w| line.contains(*w)) {
                    hits.push(format!("{rel}:{}: `{word}` in {}", i + 1, line.trim()));
                }
            }
        }
        assert!(hits.is_empty(), "vim reached outside its seam:\n{}", hits.join("\n"));
    }
}

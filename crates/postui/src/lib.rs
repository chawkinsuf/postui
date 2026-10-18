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
pub mod text;
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
    use super::source_scan::{code_lines, sources};

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
    const SEAM: [&str; 5] =
        [".vim.", "glue_of(", "live_engine_field(", "session_parts(", "sync_session("];

    #[test]
    fn only_the_vim_side_names_the_engine_or_the_seam_internals() {
        let mut hits = Vec::new();
        for (rel, text) in sources() {
            if vim_side(&rel) {
                continue;
            }
            let banned: Vec<&str> = if ENGINE_USERS.contains(&rel.as_str()) {
                SEAM.to_vec()
            } else {
                ENGINE.iter().chain(SEAM.iter()).copied().collect()
            };
            for (n, line) in code_lines(&text) {
                if let Some(word) = banned.iter().find(|w| line.contains(*w)) {
                    hits.push(format!("{rel}:{n}: `{word}` in {}", line.trim()));
                }
            }
        }
        assert!(hits.is_empty(), "vim reached outside its seam:\n{}", hits.join("\n"));
    }
}

/// Text is measured one way: `ui::cells` and its helpers, by grapheme, as
/// ratatui paints it. Only the places that must count per char like
/// another program may measure for themselves. (ratatui's own
/// `Span::width` / `Line::width` measure by `unicode-width` too; they
/// are not caught here, so don't size with them either.)
#[cfg(test)]
mod width_measure_test {
    use super::source_scan::{code_lines, sources};

    /// Files that may measure text themselves: `ui.rs` is the measure;
    /// the body editor maps clicks the way edtui lays text out; the vim
    /// engine counts columns as Vim does, and so does the response view's
    /// Vim cursor (`vim_cells`; everything it paints walks `ui::graphemes`,
    /// task #45).
    const MEASURERS: [&str; 3] = ["ui.rs", "components/editor.rs", "components/response.rs"];

    const MEASURES: [&str; 3] = ["unicode_width", "unicode_segmentation", "CellWidth"];

    #[test]
    fn only_the_measurers_measure_text_themselves() {
        let mut hits = Vec::new();
        for (rel, text) in sources() {
            if rel.starts_with("vim/") || MEASURERS.contains(&rel.as_str()) {
                continue;
            }
            for (n, line) in code_lines(&text) {
                if let Some(word) = MEASURES.iter().find(|w| line.contains(*w)) {
                    hits.push(format!("{rel}:{n}: `{word}` in {}", line.trim()));
                }
            }
        }
        assert!(hits.is_empty(), "measure text with `ui::cells`:\n{}", hits.join("\n"));
    }
}

/// The crate's own source, for the tests that hold it to a rule.
#[cfg(test)]
mod source_scan {
    use std::path::{Path, PathBuf};

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

    /// Every source file but test files and this one, by its path under
    /// `src/`, with its text. Test code is left out on purpose: it reaches
    /// in to check what the code did.
    pub fn sources() -> Vec<(String, String)> {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        walk(&src, &mut files);
        files
            .into_iter()
            .map(|f| {
                let rel = f.strip_prefix(&src).unwrap().to_string_lossy().replace('\\', "/");
                (rel, f)
            })
            .filter(|(rel, _)| !rel.ends_with("tests.rs") && rel != "lib.rs")
            .map(|(rel, f)| (rel, std::fs::read_to_string(f).unwrap()))
            .collect()
    }

    /// The code lines of a file's `text`, numbered from 1: comments are
    /// skipped, and an inline test module (`mod tests {`) ends the code —
    /// clippy's `items_after_test_module` keeps it at the bottom. A
    /// declared one (`mod tests;`) is a file of its own, so the code goes
    /// on past it.
    pub fn code_lines(text: &str) -> Vec<(usize, &str)> {
        let inline_mod =
            |n: &&str| n.trim_start().starts_with("mod ") && n.trim_end().ends_with('{');
        let lines: Vec<&str> = text.lines().collect();
        let mut code = Vec::new();
        for (i, line) in lines.iter().enumerate() {
            if line.trim() == "#[cfg(test)]" && lines.get(i + 1).is_some_and(inline_mod) {
                break;
            }
            if !line.trim_start().starts_with("//") {
                code.push((i + 1, *line));
            }
        }
        code
    }

    /// A declared test module is skipped over; an inline one ends the code.
    #[test]
    fn only_an_inline_test_module_ends_the_code() {
        let text = "a\n#[cfg(test)]\nmod t;\n// c\nb\n#[cfg(test)]\nmod tests {\nx\n}";
        let lines: Vec<&str> = code_lines(text).into_iter().map(|(_, l)| l).collect();
        assert_eq!(lines, ["a", "#[cfg(test)]", "mod t;", "b"]);
    }
}

//! A text lint: `std::fs` may appear only in `Disk`, `hostfs`, test code,
//! and the test-only fixtures. It is a test in each crate so the rule
//! fails the build the moment a new caller appears.
#![cfg(any(test, feature = "test-util"))]

use std::path::Path;

/// Every line naming `std::fs` (or `fs::` after a `use std::fs`) outside
/// the allowed files and test code. Test code is: a file whose name is
/// `tests.rs` or that lives under a `tests/` directory; a file whose
/// first attribute is `#![cfg(any(test, …))]`; and everything from a
/// `#[cfg(test)]` line that is directly followed by `mod ` to the end of
/// the file (test modules sit at the bottom in this workspace — a
/// `#[cfg(test)]` on a single item does not start the exempt region).
pub fn check(src_dir: &Path, allowed_files: &[&str]) -> Vec<String> {
    let mut out = Vec::new();
    let mut files = Vec::new();
    walk(src_dir, &mut files);
    for file in files {
        let rel = file
            .strip_prefix(src_dir)
            .unwrap_or(&file)
            .to_string_lossy()
            .replace('\\', "/");
        if allowed_files.contains(&rel.as_str())
            || rel.ends_with("tests.rs")
            || rel.split('/').any(|seg| seg == "tests")
        {
            continue;
        }
        let text = std::fs::read_to_string(&file).unwrap();
        if text
            .lines()
            .find(|l| !l.trim().is_empty() && !l.trim_start().starts_with("//"))
            .is_some_and(|l| l.trim_start().starts_with("#![cfg(any(test"))
        {
            continue;
        }
        let lines: Vec<&str> = text.lines().collect();
        let mut in_tests = false;
        let uses_fs_alias = lines
            .iter()
            .any(|l| l.trim_start().starts_with("use std::fs"));
        for (i, line) in lines.iter().enumerate() {
            if line.trim() == "#[cfg(test)]"
                && lines
                    .get(i + 1)
                    .is_some_and(|n| n.trim_start().starts_with("mod "))
            {
                in_tests = true;
            }
            if in_tests {
                // The module's own closing brace, at column 0, ends it.
                if *line == "}" {
                    in_tests = false;
                }
                continue;
            }
            let hit = line.contains("std::fs") || (uses_fs_alias && line.contains("fs::"));
            if hit && !line.trim_start().starts_with("//") {
                out.push(format!("{rel}:{}: {}", i + 1, line.trim()));
            }
        }
    }
    out
}

fn walk(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let p = e.path();
        if p.is_dir() {
            walk(&p, out)
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p)
        }
    }
}

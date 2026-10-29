//! What a command prints: its report as text, or as one JSON document on
//! stdout with `--json` (R17); an error as `postui: <file>: <message>` on
//! stderr (R22), or as the envelope `{"error": ..., "file": ...}` on
//! stdout with `--json`, with status 2. Everything goes through `cli_out`,
//! so a reader that stopped (`postui show | head -c1`) ends the run
//! quietly.

use super::CliError;
use crate::cli_out::{print_err, print_out};
use serde::Serialize;
use std::path::Path;
use std::process::ExitCode;

/// The status every error ends the run with.
const ERROR: u8 = 2;

/// What [`emit`] prints for `report`: `render_text`'s lines, or pretty JSON
/// with `json`. A report JSON can't hold (a path that isn't UTF-8) is an
/// error rather than a guess at the bytes.
pub fn render<T: Serialize>(
    report: &T,
    json: bool,
    render_text: impl FnOnce(&T) -> String,
) -> Result<String, CliError> {
    if !json {
        return Ok(render_text(report));
    }
    serde_json::to_string_pretty(report)
        .map(|json| json + "\n")
        .map_err(|e| CliError::new(format!("can't print the report: {e}")))
}

/// Prints `report` as [`render`] makes it. `None` when it all went out;
/// otherwise the status to end the run with (see [`print_out`]).
pub fn emit<T: Serialize>(
    report: &T,
    json: bool,
    render_text: impl FnOnce(&T) -> String,
) -> Option<ExitCode> {
    match render(report, json, render_text) {
        Ok(printed) => print_out(&printed),
        Err(e) => Some(fail(&e, json)),
    }
}

/// The JSON error envelope; `file` is left out when the error names none.
#[derive(Serialize)]
struct Envelope<'a> {
    error: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    file: Option<String>,
}

/// Prints `err` and gives the error status: `postui: <file>: <message>` on
/// stderr, the file first as `check` names its findings and absolute (an
/// error prints no root to be relative to); or the envelope on stdout with
/// `json`.
pub fn fail(err: &CliError, json: bool) -> ExitCode {
    // An error message, not data: a path that isn't UTF-8 is shown lossily.
    let file = err.file.as_ref().map(|f| f.to_string_lossy().into_owned());
    if json {
        let envelope = Envelope { error: &err.message, file };
        // Two strings: nothing in them can fail to serialize.
        let json = serde_json::to_string_pretty(&envelope).unwrap_or_default();
        let _ = print_out(&(json + "\n"));
    } else {
        let at = file.map(|f| format!("{f}: ")).unwrap_or_default();
        print_err(&format!("postui: {at}{}\n", err.message.trim_end()));
    }
    ExitCode::from(ERROR)
}

/// Prints a mistake on a command's line: the reason and the command's
/// `usage` on stderr, or the envelope on stdout with `json`; the error
/// status either way.
pub fn fail_usage(reason: &str, json: bool, usage: &str) -> ExitCode {
    if json {
        fail(&CliError::new(reason), true)
    } else {
        print_err(&format!("postui: {reason}\n\n{usage}"));
        ExitCode::from(ERROR)
    }
}

/// `path` as the text prints it: relative to `root` when it is inside it
/// (R20), else as it is.
pub fn rel(root: &Path, path: &Path) -> String {
    path.strip_prefix(root).unwrap_or(path).display().to_string()
}

/// `rows` as `label  value` lines, the values aligned and not indented: the
/// facts a report starts with. A value over several lines continues as a
/// table cell does (R21).
pub fn fields(rows: &[(&str, String)]) -> String {
    let width = rows.iter().map(|(label, _)| label.len()).max().unwrap_or(0);
    let mut out = String::new();
    for (label, value) in rows {
        match first_line(value) {
            "" => out.push_str(label),
            first => out.push_str(&format!("{label:width$}  {first}")),
        }
        out.push('\n');
        push_under(&mut out, value.lines().skip(1));
    }
    out
}

/// `text`'s first line: what goes in a row (R21).
fn first_line(text: &str) -> &str {
    text.lines().next().unwrap_or_default()
}

/// Pushes `lines` under a row, indented by four, a blank line kept blank.
fn push_under<'a>(out: &mut String, lines: impl Iterator<Item = &'a str>) {
    for line in lines {
        if !line.is_empty() {
            out.push_str("    ");
            out.push_str(line);
        }
        out.push('\n');
    }
}

/// `n` `thing`s, the plural when `n` isn't 1.
pub fn count(n: usize, thing: &str) -> String {
    if n == 1 { format!("1 {thing}") } else { format!("{n} {thing}s") }
}

/// `rows` as aligned columns, two spaces apart and indented by two, one
/// row per line; a row stops after its last non-empty cell, unpadded, so a
/// value's own trailing spaces are kept and no padding trails. Widths are
/// display cells. A cell that runs over several lines puts its first line
/// in the row and the rest on the lines under it, indented by four, so one
/// long value doesn't push the columns apart (R21).
pub fn table<R: AsRef<[String]>>(rows: &[R]) -> String {
    let rows: Vec<(&[String], &str)> = rows.iter().map(|r| (r.as_ref(), "")).collect();
    table_under(&rows)
}

/// [`table`], each row with text printed whole on the lines under it,
/// indented by four (nothing when empty): what doesn't fit a column, such
/// as a broken file's error.
pub fn table_under<R: AsRef<[String]>>(rows: &[(R, &str)]) -> String {
    let columns = rows.iter().map(|(cells, _)| cells.as_ref().len()).max().unwrap_or(0);
    let widths: Vec<usize> = (0..columns)
        .map(|c| {
            rows.iter()
                .filter_map(|(cells, _)| cells.as_ref().get(c))
                .map(|s| crate::ui::cells(first_line(s)))
                .max()
                .unwrap_or(0)
        })
        .collect();
    let mut out = String::new();
    for (cells, under) in rows {
        let cells = cells.as_ref();
        let shown =
            cells.iter().rposition(|c| !first_line(c).is_empty()).map_or(0, |last| last + 1);
        if shown > 0 {
            out.push_str("  ");
        }
        for (c, cell) in cells[..shown].iter().enumerate() {
            let cell = first_line(cell);
            out.push_str(cell);
            if c + 1 < shown {
                out.push_str(&" ".repeat(widths[c] - crate::ui::cells(cell) + 2));
            }
        }
        out.push('\n');
        push_under(
            &mut out,
            cells.iter().flat_map(|cell| cell.lines().skip(1)).chain(under.lines()),
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_aligns_by_display_cells() {
        let rows = vec![
            vec!["a".to_string(), "x".to_string(), "end".to_string()],
            vec!["日本".to_string(), "".to_string(), "z".to_string()],
        ];
        assert_eq!(table(&rows), "  a     x  end\n  日本     z\n");
    }

    /// R21: only a cell's first line counts toward its column; the rest go
    /// under the row, indented by four, blank lines kept blank.
    #[test]
    fn a_multi_line_cell_continues_under_its_row() {
        let rows = vec![
            vec!["a".to_string(), "one\n  |\n\nthree".to_string(), "end".to_string()],
            vec!["bb".to_string(), "x".to_string(), "z".to_string()],
        ];
        assert_eq!(table(&rows), "  a   one  end\n      |\n\n    three\n  bb  x    z\n");
    }

    /// A row stops after its last non-empty cell, so padding never trails
    /// and a value's own trailing spaces are kept.
    #[test]
    fn a_row_keeps_its_values_trailing_spaces_and_trails_no_padding() {
        let rows = vec![
            vec!["a".to_string(), "x ".to_string(), String::new()],
            vec!["bb".to_string(), "y".to_string(), "z".to_string()],
        ];
        assert_eq!(table(&rows), "  a   x \n  bb  y   z\n");
        assert_eq!(fields(&[("k", "v ".to_string())]), "k  v \n");
    }

    /// R21 in `fields`: a value's later lines go under it, not at column 0
    /// where they would read as more labels.
    #[test]
    fn a_multi_line_field_continues_under_it() {
        let printed = fields(&[("description", "one\ntwo".to_string()), ("k", "v".to_string())]);
        assert_eq!(printed, "description  one\n    two\nk            v\n");
    }

    #[test]
    fn under_text_follows_its_row() {
        let a = ["a".to_string(), "b".to_string()];
        let c = ["cc".to_string(), "d".to_string()];
        let rows: Vec<(&[String], &str)> = vec![(&a, "why\nwhere"), (&c, "")];
        assert_eq!(table_under(&rows), "  a   b\n    why\n    where\n  cc  d\n");
    }

    #[test]
    fn fields_align_their_values_unindented() {
        let printed = fields(&[("root", "/p".to_string()), ("environment", String::new())]);
        assert_eq!(printed, "root         /p\nenvironment\n");
    }

    #[test]
    fn rel_is_relative_inside_the_root_only() {
        let root = Path::new("/p/api");
        assert_eq!(rel(root, Path::new("/p/api/requests/a.toml")), "requests/a.toml");
        assert_eq!(rel(root, Path::new("/elsewhere/out.bin")), "/elsewhere/out.bin");
    }
}

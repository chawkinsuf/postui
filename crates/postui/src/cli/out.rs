//! What a command prints: its report as one JSON document on stdout, or
//! `--text` for people; an error as the envelope
//! `{"error": ..., "file": ...}` on stdout, or `postui: ...` on stderr,
//! with status 2. Everything goes through `cli_out`, so a reader that
//! stopped (`postui show | head -c1`) ends the run quietly.

use super::CliError;
use crate::cli_out::{print_err, print_out};
use serde::Serialize;
use std::process::ExitCode;

/// The status every error ends the run with.
const ERROR: u8 = 2;

/// What [`emit`] prints for `report`: pretty JSON, or `render_text`'s lines
/// with `text`. A report JSON can't hold (a path that isn't UTF-8) is an
/// error rather than a guess at the bytes.
pub fn render<T: Serialize>(
    report: &T,
    text: bool,
    render_text: impl FnOnce(&T) -> String,
) -> Result<String, CliError> {
    if text {
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
    text: bool,
    render_text: impl FnOnce(&T) -> String,
) -> Option<ExitCode> {
    match render(report, text, render_text) {
        Ok(printed) => print_out(&printed),
        Err(e) => Some(fail(&e, text)),
    }
}

/// The JSON error envelope; `file` is left out when the error names none.
#[derive(Serialize)]
struct Envelope<'a> {
    error: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    file: Option<String>,
}

/// Prints `err` (the envelope on stdout, or `postui: <message>` on stderr
/// with `text`) and gives the error status.
pub fn fail(err: &CliError, text: bool) -> ExitCode {
    // An error message, not data: a path that isn't UTF-8 is shown lossily.
    let file = err.file.as_ref().map(|f| f.to_string_lossy().into_owned());
    if text {
        let at = file.map(|f| format!(" ({f})")).unwrap_or_default();
        print_err(&format!("postui: {}{at}\n", err.message));
    } else {
        let envelope = Envelope { error: &err.message, file };
        // Two strings: nothing in them can fail to serialize.
        let json = serde_json::to_string_pretty(&envelope).unwrap_or_default();
        let _ = print_out(&(json + "\n"));
    }
    ExitCode::from(ERROR)
}

/// Prints a mistake on a command's line: the envelope on stdout, or with
/// `text` the reason and the command's `usage` on stderr; the error
/// status either way.
pub fn fail_usage(reason: &str, text: bool, usage: &str) -> ExitCode {
    if text {
        print_err(&format!("postui: {reason}\n\n{usage}"));
        ExitCode::from(ERROR)
    } else {
        fail(&CliError::new(reason), false)
    }
}

/// `rows` as aligned columns, two spaces apart and indented by two, one
/// row per line; the last column is not padded. Widths are display cells.
pub fn table(rows: &[Vec<String>]) -> String {
    let columns = rows.iter().map(Vec::len).max().unwrap_or(0);
    let widths: Vec<usize> = (0..columns)
        .map(|c| {
            rows.iter().filter_map(|r| r.get(c)).map(|s| crate::ui::cells(s)).max().unwrap_or(0)
        })
        .collect();
    let mut out = String::new();
    for row in rows {
        let mut line = String::from("  ");
        for (c, cell) in row.iter().enumerate() {
            line.push_str(cell);
            if c + 1 < row.len() {
                line.push_str(&" ".repeat(widths[c] - crate::ui::cells(cell) + 2));
            }
        }
        out.push_str(line.trim_end());
        out.push('\n');
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
}

# Body editor indentation: a default, and detection (task #8)

Status: APPROVED by the user 2026-10-15, after a section-by-section review. Branch `editor-indent`.

## 1. Findings

Today the body editor has no indentation setting, and its paths disagree:

| Path | Today | Where |
|---|---|---|
| Arrows: plain Tab, no selection | edtui inserts a literal `\t` | edtui 0.11.6 `events/key.rs:1110` |
| Arrows: Tab / shift+Tab on a selection, shift+Tab on the caret's line | adds 2 spaces / removes a leading `\t` or up to 2 spaces | `Editor::indent_body_lines`, `BODY_TAB_WIDTH = 2` (`editor.rs:1445`) |
| Arrows: Enter | a bare line break, no indent | edtui `LineBreak` |
| Vim profile, body | `expandtab shiftwidth=2 tabstop=2 autoindent`, pinned | `vim/engine/settings.rs`; text-engine spec §3 settings table (`mine`) |
| Format body | serde's 2-space pretty-print | `postui_core::json::format` |
| Display | a `\t` renders 2 columns wide; clicks map with the same 2 | edtui `ViewState::tab_width` default; `body_char_width` |

So a body already ends up mixed: Tab typed in arrows plants tabs, and selection indent and Format plant 2-space levels. A 4-space or tab-indented body loaded from disk gets 2-space levels mixed in by every indent command.

## 2. Scope

Decided during review (you, 2026-10-15): this task covers the setting, detection, the arrows profile's indent keys, Format, and one slice of the vim profile: Vim's `shiftwidth` follows a space-indented body's width. The rest of vim stays pinned (`expandtab tabstop=2 softtabstop=0`) and moves to a follow-up task: `noexpandtab` for tab bodies, and Insert Tab stepping by the width. That was most of the drafted design's complexity: per-buffer `tabstop` threaded through the engine's column maths, `noexpandtab` indent maths, and a second golden file. Because the engine's column maths must agree with the display, a tab keeps displaying 2 columns wide until that follow-up too.

Already true in the vim profile with no change: Format follows the body (R10 is an app action), and `autoindent` copies the previous line's indent, so new lines on Enter/`o`/`O` match the body.

## 3. Rules

| # | Rule | Source |
|---|---|---|
| R1 | `config.toml` gains `indent = "spaces" \| "tabs"` (default `"spaces"`) and `indent_width` (default `2`, range 1–8): the spaces in one level. A wrong type or an out-of-range value warns and uses the default, like every other key. | kind + one width: you; names, defaults, range: mine |
| R2 | The Settings tab gains two rows after "Keyboard profile": **Indent** (a `spaces` / `tabs` segmented control, like jq Tab behavior) and **Indent width** (an integer field, like OSC 52 limit). | mine |
| R3 | A body's effective indent comes from detection when the body has indented lines, and from R1 when it has none. Detection reads both the kind and the width. | you (reconfirmed during review over default-only, spaces-only and normalize-on-open) |
| R4 | Detection: among lines with non-blank content, count the ones whose indent starts with `\t` (tab-led) and with a space (space-led). More tab-led → tabs. More space-led → spaces, at the width that occurs most often as the difference between consecutive space-indented content lines' indents (counting a virtual unindented line before the first line, ignoring differences over 8, ties going to the smaller width). A tie, or no indented line at all, detects nothing. | algorithm: mine (VS Code's guessIndentation idea) |
| R5 | Opening a request detects afresh: nothing detected → R1. Any other wholesale replacement of the body's text (Format, Minify, Clear, the `$EDITOR` round-trip, an undo/redo snapshot swap, replace-with-curl) detects again, and keeps the current indent when the new text detects nothing, so Minify then Format gives back the body's own indent. Typing and pasting never re-detect. | mine |
| R6 | Changing R1 in Settings applies at once: a body that detected nothing follows the new default; a body that detected an indent keeps it. | mine |
| R7 | No indicator and no per-body override. | you |
| R8 | Arrows, plain Tab with no selection: spaces → spaces up to the next multiple of the width (by display column); tabs → one `\t`. | mine (today's literal `\t` under spaces is the inconsistency #8 is about) |
| R9 | Arrows, indent (Tab on a selection): each non-empty line gets one level in front, either `width` spaces or one `\t`. Dedent (shift+Tab): removes a leading `\t`, or else up to `width` leading spaces. Empty lines are still skipped, and the selection and caret still ride along. | today's rule, with the width and kind generalised |
| R10 | Format body pretty-prints with the effective indent: `width` spaces or one `\t` per level. In both profiles: Format is an app action, not an engine command. | you (scope); tab form: mine |
| R11 | Vim profile, body: `shiftwidth` is the body's width when its indent is `Spaces(n)`, and the pinned 2 for a tab body. So `>>`, `<<`, Visual `>` / `<` and Insert `ctrl+t` / `ctrl+d` step by it, exactly as Vim 9.1 does under `expandtab shiftwidth=n tabstop=2 softtabstop=0`. One-line fields keep `shiftwidth=2`. | you (the slice); Vim (behaviour, probed); tab bodies and one-line fields at 2: mine |
| R12 | Unchanged by this task: vim's `expandtab`, Insert Tab's 2-column step (`tabstop=2 softtabstop=0`), and a `\t`'s 2-column display. All three move to the follow-up task. | you (split) |
| R13 | Out of scope: smart (bracket-aware) indent; Backspace over a space indent (still deletes one space); converting a body's existing indentation; the response pane's tab width. | mine; offered as follow-up tasks |
| R14 | Arrows, plain Enter in the body, caret **after** the line's first non-blank: the new line starts with the line's leading whitespace, and the text that moves down loses its own leading blanks, as Vim's `autoindent` does (probed, Vim 9.1.0016: `    foo  \|bar` → `    foo  ` / `    bar`; `  a\|   b` → `  a` / `  b`). It copies whatever the line has, tabs or spaces, so it needs no setting. Caret **at or before** the first non-blank (anywhere in the indent, or on a line that is only whitespace): Enter opens an empty line above, and the line moves down whole, its indent untouched, the caret keeping its column. Repeated Enters therefore leave no trailing blanks behind, which is what Vim gets by removing an autoindent nothing followed (probed: `A<CR><CR>x` on `    a` leaves an empty middle line). Inside the indent this differs from Vim, which cuts the copied indent at the caret, so Enter at column 0 of `    }` gives `` / `}` (probed); arrows keeps the line's indent, as GUI editors do. A paste never auto-indents (`paste_body` inserts raw line breaks). Until #26, the vim profile's autoindent turns a copied tab indent into spaces (`expandtab`), while arrows copies tabs as tabs. | auto-indent: you (copy, not bracket-aware); inside the indent: you (open a line above, over Vim's cut); paste: mine |

### Amendment 2026-10-15, review round 1 (you): the indent is read from the text

Review round 1 found that the remembered `body_detected` broke "undo restores exactly": Format on a flat body pinned the default as detected, undoing the Format kept the pin, and a later Settings change then missed that body. The same root made Format pin in arrows but not under a vim session, and let an unrelated undo after a paste switch the indent. You chose to drop the memory: **a body's effective indent is `detect(its current text)`, else the configured default**, computed when a key, Format or the vim seam asks. This replaces R3's "comes from detection" with "from detection of the text as it is now" and replaces R5 entirely:

- R5 (new): there is no detection event. Opening, Format, Minify, Clear, `$EDITOR`, undo/redo, replace-with-curl, typing and pasting all just change the text, and the indent follows the text. So Minify then Format formats with the configured default (the flat text has no indent of its own), and typing or pasting indented lines can change the body's indent. Undo restores it exactly, and both profiles agree, by construction.
- R6 is unchanged in effect: a body whose text has no indented lines follows a Settings change at once; one with indented lines keeps its own.
- Cost (mine, accepted with the choice): one scan of the body's lines each time the indent is asked for (arrows Tab/shift+Tab, Format, and each key the vim seam lends the body for). It reads the edtui lines in place, with no copy of the text.

Round 2 (mine, following the round-1 choice): the seam hands the glue only the configured default, and the glue works out the body's `shiftwidth` from the body lines it is lent, inside each body target (`text::indent::effective`, the rule the editor uses too). So the scan runs only for body calls, not for every field's, and nothing derived from the text is pushed across the seam. A carry sets no width: it never shifts, and the next key goes through a body target.

### Amendment 2026-10-15, review round 4 (you): the indent is read from the text as last saved

Round 4 found that reading the live text was self-reinforcing: on a body with few indented lines, each indent changed what was detected, so repeated Tab, indent or vim `>>` doubled (2, 4, 8) instead of stepping by a fixed width. Rounds 1 and 4 flagged opposite shapes: a remembered value was stable but not undo-exact, the live text undo-exact but unstable. You chose the shape that keeps both: **a body's effective indent is `detect(the body as last opened or saved)`, else the configured default.** That text is the editor's existing dirty baseline (`Editor::saved`), which only opening, saving and the curl import's discard replace (`Editor::set_saved`, the one writer, detects with it).

- Supersedes the round-1 amendment and its round-2 note. Undo never changes the saved text, so it restores the indent exactly; Format, Minify, Clear, paste and typing never move it, in either profile; Minify then Format gives the body's own indent back (R5's original intent); an indent never feeds the next one's width.
- Until a save, a style that arrives in the editor (pasted into a new or flat request, or brought by `$EDITOR` or replace-with-curl) gives way to the saved text's indent, or to the default when that is flat. A scratch request that was never saved uses the default.
- Detection runs once per open or save, so no key scans the body. The vim seam hands the glue the editor's ready indent, and the glue puts its width on the body's state inside each body target.

Also from round 1 (you): dropping an unused auto-indent when the caret leaves the line (Vim's behaviour, VS Code's `trimAutoWhitespace`) is a follow-up task, not #8.

## 4. Design

**One value, one owner.** A new `crate::text::indent` module (shared code, outside the vim seam, like `text::class`) holds:

```rust
pub enum Indent { Spaces(usize), Tabs }   // Spaces(1..=8)

pub fn detect(text: &str) -> Option<Indent>;   // R4
impl Indent {
    pub fn unit(self) -> String;               // one level: N spaces or "\t"
}
```

**Config.** `UiSettings` gains `indent_tabs: bool` and `indent_width: usize`, one per key (so each Settings row and its undo step writes one field), and `UiSettings::default_indent() -> Indent` combines them. They are parsed from the two keys as R1, seeded (commented) in `config_seed`, and saved by the Settings rows through the existing `save_ui_string` / `save_ui_int`. `SettingsField` gains `Indent` and `IndentWidth`. Under `indent = "tabs"` the width is still stored and shown, ready for the follow-up, but nothing reads it yet.

**Editor (the owner).** (The `body_detected` memory and its three bullets below are superseded by the round-1 amendment above: `body_indent()` is `detect(lines).unwrap_or(indent_default)`, and neither `load` nor `set_body_text` detects.) `Editor` gains `body_detected: Option<Indent>` and `indent_default: Indent`. Its `body_indent()` is `body_detected.unwrap_or(indent_default)`, read on demand, so R6 needs no re-detection. `App` sets `indent_default` when settings load or change.

- `Editor::load` clears `body_detected` before it sets the text.
- `set_body_text`, the one path every wholesale replacement in R5 takes, runs `detect` and keeps the old value on `None`. Replace-with-curl reaches it through `apply_snapshot`.
- Under a vim session, Format, Minify and Clear go through the engine (`vim_replace_body`) instead and skip detection. The result is the same, because Clear and Minify leave nothing to detect (so R5 keeps the indent) and Format writes the body's own unit.
- `indent_body_lines` uses `body_indent()`'s unit (R9). The body's plain-Tab arm, which today falls through to edtui, inserts R8's text itself. It sits beside the existing Tab arms, which the vim profile's engine session already gates.
- A plain-Enter arm beside it (R14) splits the line with the copied indent, instead of edtui's bare `LineBreak`. A selection is still deleted first, as today, so Enter over a selection replaces it with an indented line break.
- `BODY_TAB_WIDTH` stays as the tab's display width (R12).

**Format.** `postui_core::json` gains `format_with(text, unit: &str)`, built on serde_json's `PrettyFormatter::with_indent`. `format` becomes `format_with(text, "  ")` for its other callers. `Action::FormatBody` passes the body's unit. `transform_body` takes a closure instead of a `fn` pointer so it can carry the unit.

**Vim engine: one option goes runtime (R11).** The `settings.rs` constant `SHIFTWIDTH` is read directly in `indent.rs` (Insert `ctrl+t`/`ctrl+d`) and `op.rs` (`>` / `<`). It becomes a per-buffer `shiftwidth` on `BufState`, Vim's own buffer-local option, defaulting to the pinned 2. Those readers take it from the buffer's state. `SETTINGS_LINE` and the main golden don't change, because the default is the pinned value.

- The glue (`keyroute/engine_glue.rs`) sets the body `BufState`'s `shiftwidth` from the width the seam hands it, on every sync. The places that reset that state (`BufState::new()` after an external replace) then can't leave a stale or default value behind.
- `vim_seam.rs` reads `editor.body_indent()` and passes the width in. The rest of the app still reaches the engine only through the seam, so the isolation test in `lib.rs` is unchanged.
- No second golden file. The behaviour at `shiftwidth=4` is pinned by engine unit tests whose expected text was probed from the installed Vim 9.1.0016 (`vim -Es` with `expandtab shiftwidth=4 tabstop=2 softtabstop=0`), each test naming its probe.

**Contracts that change.** `UiSettings` (a field), `SettingsField` (two variants that the Settings tab and its hit-testing match on), `transform_body`'s parameter, and the engine's `BufState` (a `shiftwidth` field, which the glue sets).

## 5. Tests

| Rule | Test |
|---|---|
| R1 | `config.rs`: both keys parse; a wrong type, a bad spelling, 0 and 9 each warn and default. |
| R2 | `settings.rs`: the rows exist in order; toggling and editing write `indent` / `indent_width`. |
| R4 | `text/indent.rs` unit tests: 2-space and 4-space JSON; tab JSON; a tie; minified JSON; a body with one indented line; deep closing-brace dedents not skewing the width; blank and whitespace-only lines ignored. |
| R3, R5 | `editor.rs`: `load` of a 4-space body with a 2-space default → 4; `load` of a flat body → default; Minify then Format of a 4-space body → 4 again; an `apply_snapshot` swap re-detects; typing never changes it. |
| R6 | App test: a Settings change moves an undetected body and leaves a detected body alone. |
| R8, R9 | `editor.rs`: under spaces/2, Tab at column 1 inserts one space (to the stop) and at 0 inserts two; under tabs, Tab inserts `\t`. The existing `tab_indents_…`, `tab_skips_empty_…` and dedent tests keep their 2-space expectations and gain 4-space and tab cases. |
| R10 | `json.rs` `format_with` for `"\t"` and 4 spaces, keeping key order and number text; an app test that Format follows a 4-space body, in arrows and under a vim session. |
| R11 | Engine unit tests at `shiftwidth=4`, probed from Vim: `>>`, `3>>`, `<<` on a 3-space line (rounds as Vim does), Visual `>` with a count, `ctrl+t`/`ctrl+d` in Insert, and `.` repeating `>>`. App tests: on a 4-space body, vim `>>` shifts by 4 and survives an `$EDITOR` replace (the `BufState` reset); a tab body and a URL `>>` still shift by 2. |
| R12 | The vim conformance golden passes unchanged (the default is the pinned value). |
| R14 | `editor.rs`: Enter at the end of a 4-space line → new line with 4 spaces; Enter mid-text → the moved text's leading blanks go and the indent is copied; tab indent copied as tabs; Enter at column 0, inside the indent and at the first non-blank of `    }` → an empty line above and `    }` unchanged, caret keeping its column; Enter on a whitespace-only line → an empty line above it; Enter, Enter → no trailing blanks; Enter over a selection; a bracketed paste of several lines is not re-indented. An app test: undo treats the indented Enter as today's Enter (it coalesces with the typing burst as before), so one undo after an Enter alone restores the line whole, indent included, and the indent never takes an undo step of its own. |

## 6. `mine` choices, for veto

1. The key names `indent` / `indent_width`, the 1–8 range, the defaults (spaces, 2), and the Settings rows' place (R1, R2).
2. The detection algorithm and its tie rules (R4).
3. When detection runs: fresh on open, detect-or-keep on other wholesale replacements, never on typing or paste (R5).
4. Settings changes apply live to undetected bodies only (R6).
5. Arrows Tab under spaces goes to the next stop rather than inserting a fixed `width` spaces (R8).
6. Format's tab form is one `\t` per level (R10).
7. Vim `shiftwidth` stays 2 for tab bodies and one-line fields (R11).
8. Vim at other widths is checked by probed unit tests, not a second golden file (§4).
9. The out-of-scope list (R13), plus the vim follow-up task (R12).
10. A paste never auto-indents (R14).

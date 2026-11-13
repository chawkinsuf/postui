//! Grapheme lookups on a row of painted text, by display column and by
//! char: the response view's and the variable tooltip's (#294).

/// A resume point for a column walk (`viewport_window`, `grapheme_at_col`)
/// — a grapheme's byte offset, char index and the display column it
/// starts at. `(0, 0, 0)` is the line start.
pub(crate) type ColMark = (usize, usize, usize);

/// The first char of the grapheme painted at display column `col` of
/// `text` (wide graphemes span several columns), or `None` when `col` is
/// past the text. The walk resumes from `from`, a `col_marks` entry at or
/// before `col` (or the line start), as `viewport_window`'s does, so on
/// an indexed raw line it segments `O(COL_MARK_STEP)` chars however far
/// the row is scrolled (its caller still copies the line's text).
pub(crate) fn grapheme_at_col(text: &str, col: usize, from: ColMark) -> Option<usize> {
    let (from_byte, from_char, mut w) = from;
    for g in crate::ui::char_graphemes(&text[from_byte..]) {
        w += g.width;
        if w > col {
            return Some(from_char + g.char);
        }
    }
    None
}

/// The first char of `text`'s last grapheme (0 when empty). The grapheme
/// is segmented from the end; only its char offset needs the line, as a
/// plain char count (no segmenting, as cheap as the char count it took
/// before #45).
pub(crate) fn last_grapheme(text: &str) -> usize {
    crate::ui::graphemes(text).next_back().map_or(0, |(byte, _, _)| text[..byte].chars().count())
}

/// The char offsets `text`'s graphemes start at, then its char count: the
/// edges a highlight or a copy may end on, walked only until the first
/// one past char `reach` — all that widening ranges whose finite ends lie
/// by `reach` needs. `None` for ASCII text, where every char is a
/// grapheme. One walk per row, however many ranges it widens.
pub(crate) fn grapheme_edges(text: &str, reach: usize) -> Option<Vec<usize>> {
    if text.is_ascii() {
        return None;
    }
    let mut edges = Vec::new();
    let mut end = 0;
    for g in crate::ui::char_graphemes(text) {
        edges.push(g.char);
        if g.char > reach {
            return Some(edges);
        }
        end = g.char + g.chars;
    }
    edges.push(end);
    Some(edges)
}

/// The char range `[from, to)` widened out to whole graphemes (`edges`,
/// from [`grapheme_edges`]), for a highlight or a copy: a span cut inside
/// a grapheme makes ratatui paint each part alone, dropping a zero-width
/// tail (a heart's selector, an accent) and shifting the row. An end
/// already on an edge, or past the text, stays as it is.
pub(crate) fn whole_graphemes(
    edges: Option<&[usize]>,
    (from, to): (usize, usize),
) -> (usize, usize) {
    let Some(edges) = edges else {
        return (from, to);
    };
    let last = edges[edges.len() - 1];
    let start = if from < last { edges[edges.partition_point(|&e| e <= from) - 1] } else { from };
    let end = if to < last { edges[edges.partition_point(|&e| e < to)] } else { to };
    (start, end)
}

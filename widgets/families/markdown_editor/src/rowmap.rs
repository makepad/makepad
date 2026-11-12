//! Between a row's source and the text its flow drew: the runs a markdown
//! walk recorded ([`MarkdownPiece`]) map source offsets to flow indices and
//! back. Source the flow does not show (markers, prefixes, fences) maps to
//! the edge of the shown text it sits at; text the flow shows that is not
//! source (a list bullet) maps to the start of the content after it.

use makepad_widgets_rich_text::markdown::MarkdownPiece;
use std::ops::Range;

/// The flow index of source offset `src`: one to one inside a run that
/// spells its source, the start of a run that stands for its source as a
/// whole (the end when `src` is that source's end), and for hidden source
/// the start of the next run (the end of the last run after all of them).
pub fn flat_of(pieces: &[MarkdownPiece], src: usize) -> usize {
    let mut after_last = 0;
    for piece in pieces {
        if src < piece.src.start {
            return piece.flat.start;
        }
        // A run standing for no source (an empty table cell's holder) holds
        // its place's caret after it, where reading the caret back finds it.
        if piece.src.is_empty() && src == piece.src.start {
            return piece.flat.end;
        }
        // A run's end is its own (a table cell's end is not the next cell's start).
        if src < piece.src.end || (src == piece.src.end && (piece.identity || src > piece.src.start)) {
            return if piece.identity {
                piece.flat.start + (src - piece.src.start).min(piece.flat.len())
            } else if src == piece.src.end {
                piece.flat.end
            } else {
                piece.flat.start
            };
        }
        after_last = piece.flat.end;
    }
    after_last
}

/// The source offset of flow index `flat`: one to one inside a run that
/// spells its source, the start or end of a run that stands for its source
/// as a whole (whichever end is nearer), and for drawn text that is not
/// source (between runs) the start of the next run.
pub fn src_of(pieces: &[MarkdownPiece], flat: usize) -> usize {
    let mut after_last = 0;
    for piece in pieces {
        if flat < piece.flat.start {
            return piece.src.start;
        }
        if flat < piece.flat.end {
            return if piece.identity {
                piece.src.start + (flat - piece.flat.start)
            } else if flat - piece.flat.start > (piece.flat.end - flat) {
                piece.src.end
            } else {
                piece.src.start
            };
        }
        after_last = piece.src.end;
    }
    after_last
}

/// Where the caret stands at flow index `flat`, as a source offset. As
/// [`src_of`], except at a run's end, where it takes that run's end: on a
/// boundary with only hidden source after it (markers) typing there
/// continues the text before it (a bold word stays bold), and a table
/// cell's end stays in its cell. At the end of a link's text it leaves the
/// link: typing after a link does not extend it.
/// `links` are the source ranges of the row's links.
pub fn caret_src(pieces: &[MarkdownPiece], flat: usize, links: &[Range<usize>]) -> usize {
    let i = pieces.partition_point(|piece| piece.flat.end < flat);
    let mut src = match pieces.get(i) {
        Some(left) if left.flat.end == flat => left.src.end,
        _ => src_of(pieces, flat),
    };
    for link in links {
        let inside = link.start < src && src < link.end;
        if inside && !pieces.iter().any(|piece| piece.src.end > src && piece.src.start < link.end) {
            src = link.end;
        }
    }
    src
}

/// The start of the word before `at` in `text` (after the spaces before it).
pub fn prev_word(text: &str, at: usize) -> usize {
    let mut at = floor_boundary(text, at);
    while at > 0 && text[..at].chars().next_back().is_some_and(char::is_whitespace) {
        at = prev_char(text, at);
    }
    while at > 0 && text[..at].chars().next_back().is_some_and(|c| !c.is_whitespace()) {
        at = prev_char(text, at);
    }
    at
}

/// The start of the word after `at` in `text` (past the word and the spaces
/// after it).
pub fn next_word(text: &str, at: usize) -> usize {
    let mut at = floor_boundary(text, at);
    while at < text.len() && text[at..].chars().next().is_some_and(|c| !c.is_whitespace()) {
        at = next_char(text, at);
    }
    while at < text.len() && text[at..].chars().next().is_some_and(char::is_whitespace) {
        at = next_char(text, at);
    }
    at
}

/// The char boundary before `at`.
pub fn prev_char(text: &str, at: usize) -> usize {
    text[..at.min(text.len())].char_indices().next_back().map_or(0, |(i, _)| i)
}

/// The char boundary after `at`.
pub fn next_char(text: &str, at: usize) -> usize {
    text.get(at..).and_then(|rest| rest.chars().next()).map_or(text.len(), |c| at + c.len_utf8())
}

/// What a double click on the character at `at` selects: its run of word
/// characters, or of spaces, or the one other character. Line breaks bound
/// every run; on one (a line's end) the character before it counts.
pub fn word_around(text: &str, at: usize) -> Range<usize> {
    fn class(c: char) -> u8 {
        if c.is_alphanumeric() || c == '_' {
            0
        } else if c == ' ' || c == '\t' {
            1
        } else {
            2
        }
    }
    let at = floor_boundary(text, at);
    let (start, c) = match text[at..].chars().next().filter(|c| *c != '\n') {
        Some(c) => (at, c),
        None => match text[..at].chars().next_back().filter(|c| *c != '\n') {
            Some(c) => (at - c.len_utf8(), c),
            None => return at..at,
        },
    };
    let kind = class(c);
    if kind == 2 {
        return start..start + c.len_utf8();
    }
    let same = |c: char| c != '\n' && class(c) == kind;
    let from = text[..start].char_indices().rev().take_while(|(_, c)| same(*c)).last().map_or(start, |(i, _)| i);
    let to = start + text[start..].char_indices().take_while(|(_, c)| same(*c)).last().map_or(0, |(i, c)| i + c.len_utf8());
    from..to
}

/// What a triple click at `at` selects: its line of the flow's text (a
/// paragraph, a heading, an item, up to a hard break), without the break.
pub fn line_around(text: &str, at: usize) -> Range<usize> {
    let at = floor_boundary(text, at);
    let start = text[..at].rfind('\n').map_or(0, |i| i + 1);
    let end = text[at..].find('\n').map_or(text.len(), |i| at + i);
    start..end
}

fn floor_boundary(text: &str, at: usize) -> usize {
    let mut at = at.min(text.len());
    while !text.is_char_boundary(at) {
        at -= 1;
    }
    at
}

#[cfg(test)]
mod tests {
    use super::*;

    fn piece(src: std::ops::Range<usize>, flat: std::ops::Range<usize>, identity: bool) -> MarkdownPiece {
        MarkdownPiece { src, flat, identity }
    }

    /// `- **ab** &amp; c` drawn as `• ab & c`: a bullet that is not source,
    /// hidden markers, and an entity that stands for its source as a whole.
    #[test]
    fn source_and_flow_map_both_ways() {
        let pieces = [
            piece(4..6, 2..4, true),   // "ab" (after "- **", drawn after "• ")
            piece(8..9, 4..5, true),   // " "
            piece(9..14, 5..6, false), // "&amp;" drawn as "&"
            piece(14..16, 6..8, true), // " c"
        ];
        assert_eq!(flat_of(&pieces, 0), 2, "the list marker maps to the content start");
        assert_eq!(flat_of(&pieces, 5), 3, "inside a run, one to one");
        assert_eq!(flat_of(&pieces, 6), 4, "the closing marker maps to the next run's start");
        assert_eq!(flat_of(&pieces, 11), 5, "inside an entity, its start");
        assert_eq!(flat_of(&pieces, 14), 6, "an entity's end is the next run's start");
        assert_eq!(flat_of(&pieces, 16), 8, "the end of the source maps to the end of the text");
        assert_eq!(flat_of(&[], 3), 0, "an empty row");

        assert_eq!(src_of(&pieces, 0), 4, "the bullet maps to the content start");
        assert_eq!(src_of(&pieces, 3), 5, "inside a run, one to one");
        assert_eq!(src_of(&pieces, 5), 9, "an entity's start");
        assert_eq!(src_of(&pieces, 8), 16, "past the last run, the end of its source");
        for flat in [2, 3, 4, 6, 7, 8] {
            assert_eq!(flat_of(&pieces, src_of(&pieces, flat)), flat, "flow {flat} -> source -> back");
        }

        // `| a |  | c |` drawn as `a\t\u{FFFC}\tc`: the empty cell's place
        // holder stands for its place (6), the caret there sits after it and
        // reads back; a cell's end and the next cell's start are apart.
        let cells = [piece(2..3, 0..1, true), piece(6..6, 2..5, false), piece(9..10, 6..7, true)];
        assert_eq!(flat_of(&cells, 6), 5, "an empty cell's place is after its holder");
        assert_eq!(caret_src(&cells, 5, &[]), 6, "and reads back as the place");
        assert_eq!(caret_src(&cells, 2, &[]), 6, "before the holder is the place too");
        assert_eq!(flat_of(&cells, 3), 1, "a cell's end stays in its cell");
        assert_eq!(caret_src(&cells, 1, &[]), 3, "and reads back as its end");
        assert_eq!(caret_src(&cells, 6, &[]), 9, "the next cell starts past the break");
    }

    /// `**ab**c [de](u) f` drawn as `abc de f`: the caret keeps to the left
    /// of hidden markers, leaves a link at its end, and words step as drawn.
    #[test]
    fn caret_places_and_words() {
        let pieces = [
            piece(2..4, 0..2, true),   // "ab"
            piece(6..8, 2..4, true),   // "c "
            piece(9..11, 4..6, true),  // "de"
            piece(15..17, 6..8, true), // " f"
        ];
        let links = [8..15];
        assert_eq!(caret_src(&pieces, 0, &links), 2, "a row's start takes the first character's containers");
        assert_eq!(caret_src(&pieces, 2, &links), 4, "after the bold word, inside its markers");
        assert_eq!(caret_src(&pieces, 3, &links), 7, "inside a run, one to one");
        assert_eq!(caret_src(&pieces, 6, &links), 15, "after the link's text, out of the link");
        assert_eq!(caret_src(&pieces, 5, &links), 10, "inside the link's text");

        let text = "one  two\nthree";
        assert_eq!(next_word(text, 0), 5, "past the word and its spaces");
        assert_eq!(next_word(text, 5), 9, "a line break counts as a space");
        assert_eq!(prev_word(text, 9), 5, "back over the break to the word start");
        assert_eq!(prev_word(text, 3), 0, "inside the first word, its start");
        assert_eq!(next_char("aé", 1), 3, "a step covers the whole character");
        assert_eq!(prev_char("aé", 3), 1, "and back");
    }

    /// A double click takes a word (or a run of spaces, or one mark), a
    /// triple click a line of the flow's text.
    #[test]
    fn double_and_triple_click_units() {
        let text = "one  twö_3, x\nnext line";
        assert_eq!(word_around(text, 0), 0..3, "a word from its first character");
        assert_eq!(word_around(text, 6), 5..11, "a word from its middle, letters, digits and `_`");
        assert_eq!(word_around(text, 3), 3..5, "a run of spaces");
        assert_eq!(word_around(text, 11), 11..12, "a mark alone");
        assert_eq!(word_around(text, 14), 13..14, "at a line's end, the character before");
        assert_eq!(word_around(text, 15), 15..19, "a word after a line break");
        assert_eq!(word_around(text, text.len()), 20..24, "at the text's end, the last word");
        assert_eq!(word_around("", 0), 0..0, "an empty row");
        assert_eq!(word_around("\n", 0), 0..0, "a break alone selects nothing");
        assert_eq!(line_around(text, 6), 0..14, "a line up to its break");
        assert_eq!(line_around(text, 17), 15..24, "the last line to the end");
        assert_eq!(line_around(text, 14), 0..14, "at the break, the line before it");
    }
}

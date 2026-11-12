//! Code blocks in the final view, coloured as Code's editor colours code:
//! the editor's tokenizer for the language the fence names, and the editor's
//! token colours. A block is lexed once per version of it; a fence naming no
//! language the editor knows draws plain.

use makepad_code_editor::{
    text::Text,
    token::{Token, TokenKind},
    Tokenizer, TokenColors,
};
use makepad_code_language::detect_label;
use makepad_widgets_core::*;
use makepad_widgets_rich_text::markdown::CodeHighlight;
use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, HashSet},
    ops::Range,
};

/// A code block's tokens, line by line (none for a language not lexed).
type Lines = Vec<Vec<Token>>;

/// A code block: the generation of its top-level block (new on every edit
/// of that block), the row part that draws it and its place among the
/// part's code blocks. A long block drawn in several rows is one entry.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
struct BlockKey {
    gen: u64,
    part: (u8, usize),
    ordinal: usize,
}

/// The tokens of the code blocks drawn so far.
#[derive(Default)]
pub(crate) struct Highlights {
    blocks: HashMap<BlockKey, Lines>,
    /// The entry count after the last sweep; the next one waits until the
    /// entries have doubled, so sweeping costs O(1) per block lexed.
    swept: usize,
}

impl Highlights {
    /// Drops the tokens of blocks that are gone (edited or removed), given
    /// the generations of the blocks there are.
    pub(crate) fn sweep(&mut self, live: impl Iterator<Item = u64>) {
        if self.blocks.len() <= 2 * self.swept.max(32) {
            return;
        }
        let live: HashSet<u64> = live.collect();
        self.blocks.retain(|key, _| live.contains(&key.gen));
        self.swept = self.blocks.len();
    }

    pub(crate) fn clear(&mut self) {
        self.blocks.clear();
        self.swept = 0;
    }
}

/// A long code block drawn in several rows: its content lines, and the
/// first of them the row draws.
pub(crate) struct WholeBlock<'a> {
    pub lines: &'a [String],
    pub first: usize,
}

/// One row's code blocks: which they are, for their tokens.
pub(crate) struct RowBlocks<'a> {
    pub highlights: RefCell<&'a mut Highlights>,
    pub gen: u64,
    pub part: (u8, usize),
    pub whole: Option<WholeBlock<'a>>,
    /// The code blocks the walk drew so far in this row.
    pub calls: Cell<usize>,
}

impl RowBlocks<'_> {
    /// Gives `push` the tokens of the row's next code block, `text` in the
    /// language `label` names, as ranges of `text`: ascending, disjoint, no
    /// whitespace.
    fn tokens(&self, label: &str, text: &str, mut push: impl FnMut(Range<usize>, TokenKind)) {
        let ordinal = self.calls.get();
        self.calls.set(ordinal + 1);
        let (key, first) = match &self.whole {
            Some(whole) => (BlockKey { gen: self.gen, part: (2, usize::MAX), ordinal: 0 }, whole.first),
            None => (BlockKey { gen: self.gen, part: self.part, ordinal }, 0),
        };
        let mut highlights = self.highlights.borrow_mut();
        let lines = highlights.blocks.entry(key).or_insert_with(|| match &self.whole {
            Some(whole) => lex(label, &whole.lines.join("\n")),
            None => lex(label, text),
        });
        let mut at = 0;
        for line in lines.iter().skip(first) {
            if at >= text.len() {
                break;
            }
            for token in line {
                let end = (at + token.len).min(text.len());
                if token.kind != TokenKind::Whitespace && at < end {
                    push(at..end, token.kind);
                }
                at = end;
            }
            // The line break.
            at += 1;
        }
    }
}

/// One row's code highlighting, for the walk that draws the row.
pub(crate) struct RowHighlight<'a> {
    pub blocks: RowBlocks<'a>,
    pub colors: &'a TokenColors,
}

impl CodeHighlight for RowHighlight<'_> {
    fn highlight(&self, language: &str, text: &str, spans: &mut Vec<(Range<usize>, Vec4f)>) {
        self.blocks.tokens(language, text, |range, kind| spans.push((range, self.colors.color(kind))));
    }
}

/// `text`'s tokens in the editor's lexer for the language `label` names;
/// none when it names none the editor knows.
fn lex(label: &str, text: &str) -> Lines {
    let Some(detection) = detect_label(label) else { return Vec::new() };
    let text = Text::from(text);
    let mut tokens = vec![Vec::new(); text.as_lines().len()];
    Tokenizer::for_detection(detection, tokens.len()).update(&text, &mut tokens);
    tokens
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A block is lexed once per key, a long block once for all its rows,
    /// and a sweep drops the blocks whose generation is gone.
    #[test]
    fn blocks_lex_once_and_sweep() {
        let lines: Vec<String> = ["let a = 1;", "// two", "let b = \"x\";"].iter().map(|s| s.to_string()).collect();
        let mut highlights = Highlights::default();
        let mut row = |gen, whole, label: &str, text: &str| {
            let blocks = RowBlocks { highlights: RefCell::new(&mut highlights), gen, part: (0, 0), whole, calls: Cell::new(0) };
            let mut tokens = Vec::new();
            blocks.tokens(label, text, |range, kind| tokens.push((range, kind)));
            tokens
        };
        let tokens = row(1, None, "rust", "let a = 1;\n");
        assert!(tokens.contains(&(8..9, TokenKind::Number)), "a Rust number: {tokens:?}");
        assert!(tokens.windows(2).all(|pair| pair[0].0.end <= pair[1].0.start), "ascending, disjoint");
        assert_eq!(row(1, None, "rust", "let a = 1;\n"), tokens, "drawn again, the same tokens");

        // Rows 2 and 3 of a long block: ranges start at the row's text.
        let tokens = row(2, Some(WholeBlock { lines: &lines, first: 1 }), "rust", "// two\nlet b = \"x\";\n");
        assert_eq!(tokens.first(), Some(&(0..6, TokenKind::Comment)), "the comment line first: {tokens:?}");
        assert!(tokens.contains(&(15..18, TokenKind::String)), "the string on the next line: {tokens:?}");

        assert!(row(3, None, "mermaid", "a --> b\n").is_empty(), "a label naming no language draws plain");
        for gen in 10..80 {
            row(gen, None, "rust", "let c = 2;\n");
        }
        assert_eq!(highlights.blocks.len(), 73, "each block lexed once");
        highlights.sweep([2u64, 79].into_iter());
        assert_eq!(highlights.blocks.len(), 2, "only live generations stay");
    }
}

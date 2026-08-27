//! Cell and row storage.
//!
//! Ghostty stores cells in paged memory with interned styles; here rows own
//! their cells inline (styles are small) and scrollback rows are truncated
//! at the last meaningful cell to keep memory sane. The cell semantics —
//! wide heads/tails, spacer heads at wrapped wide chars, grapheme clusters —
//! are ported from ghostty `src/terminal/page.zig`.

use crate::term::style::Style;

/// What a cell displays.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum CellContent {
    /// Never written; renders as background.
    #[default]
    Empty,
    /// Single codepoint, width 1.
    Char(char),
    /// Single codepoint, width 2. The following cell must be `WideTail`.
    WideChar(char),
    /// The second column of a wide char.
    WideTail,
    /// Last column of a row when a wide char had to wrap: renders as blank,
    /// marks that the wrap was forced by width (ghostty `spacer_head`).
    WideSpacerHead,
    /// A multi-codepoint grapheme cluster (mode 2027 or combining marks).
    Cluster(Box<Cluster>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Cluster {
    pub cps: Vec<char>,
    /// 1 or 2.
    pub width: u8,
}

impl CellContent {
    /// Display width this content occupies (0 for tails/spacers — they are
    /// covered by their head cell).
    pub fn width(&self) -> u8 {
        match self {
            CellContent::Empty | CellContent::Char(_) => 1,
            CellContent::WideChar(_) => 2,
            CellContent::WideTail | CellContent::WideSpacerHead => 0,
            CellContent::Cluster(c) => c.width,
        }
    }

    pub fn is_wide_head(&self) -> bool {
        match self {
            CellContent::WideChar(_) => true,
            CellContent::Cluster(c) => c.width == 2,
            _ => false,
        }
    }

    /// The primary codepoint, if any.
    pub fn primary(&self) -> Option<char> {
        match self {
            CellContent::Char(c) | CellContent::WideChar(c) => Some(*c),
            CellContent::Cluster(c) => c.cps.first().copied(),
            _ => None,
        }
    }

    /// Append the textual content to `out` (for selection/copy).
    pub fn push_text(&self, out: &mut String) {
        match self {
            CellContent::Empty => out.push(' '),
            CellContent::Char(c) | CellContent::WideChar(c) => out.push(*c),
            CellContent::Cluster(c) => out.extend(c.cps.iter()),
            CellContent::WideTail | CellContent::WideSpacerHead => {}
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Cell {
    pub content: CellContent,
    pub style: Style,
    /// Hyperlink id into the terminal's hyperlink table; 0 = none.
    pub hyperlink: u32,
}

impl Cell {
    pub fn is_default(&self) -> bool {
        self.content == CellContent::Empty && self.style.is_default() && self.hyperlink == 0
    }

    /// A blank cell carrying only a background style (erase semantics: bg
    /// color survives, everything else resets — ghostty erase behavior).
    pub fn blank_with_bg(style: &Style) -> Cell {
        Cell {
            content: CellContent::Empty,
            style: Style {
                bg_color: style.bg_color,
                ..Style::default()
            },
            hyperlink: 0,
        }
    }
}

/// OSC 133 semantic row marking.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SemanticPrompt {
    #[default]
    Output,
    Prompt,
    PromptContinuation,
    Input,
}

#[derive(Clone, Debug, Default)]
pub struct Row {
    /// May be shorter than the grid width; cells beyond are default.
    pub cells: Vec<Cell>,
    /// This row soft-wraps into the next row.
    pub wrapped: bool,
    pub semantic: SemanticPrompt,
}

impl Row {
    pub fn new() -> Row {
        Row::default()
    }

    #[inline]
    pub fn cell(&self, col: usize) -> Option<&Cell> {
        self.cells.get(col)
    }

    /// Mutable access, growing storage (with default cells) as needed.
    #[inline]
    pub fn cell_mut(&mut self, col: usize) -> &mut Cell {
        if col >= self.cells.len() {
            self.cells.resize(col + 1, Cell::default());
        }
        &mut self.cells[col]
    }

    /// Trim trailing default cells (called when a row moves to scrollback).
    pub fn trim(&mut self) {
        while self.cells.last().map(|c| c.is_default()).unwrap_or(false) {
            self.cells.pop();
        }
        self.cells.shrink_to_fit();
    }

    /// Clear the whole row to blank-with-bg of `style`, at `cols` width.
    /// A fully default clear stores nothing (empty vec).
    pub fn clear(&mut self, style: &Style, cols: usize) {
        self.wrapped = false;
        self.semantic = SemanticPrompt::Output;
        let blank = Cell::blank_with_bg(style);
        if blank.is_default() {
            self.cells.clear();
        } else {
            self.cells.clear();
            self.cells.resize(cols, blank);
        }
    }

    /// If `col` lands on a wide tail, step back to its head column.
    pub fn head_of(&self, col: usize) -> usize {
        if col > 0 {
            if let Some(cell) = self.cell(col) {
                if cell.content == CellContent::WideTail {
                    return col - 1;
                }
            }
        }
        col
    }

    /// Clearing/overwriting `col` must not leave halves of wide chars:
    /// if `col` is a wide head, blank its tail; if a tail, blank its head.
    /// Returns nothing; the caller writes `col` itself afterwards.
    pub fn split_wide_at(&mut self, col: usize, style: &Style) {
        let len = self.cells.len();
        if col < len {
            match self.cells[col].content {
                CellContent::WideChar(_) => {
                    if col + 1 < len && self.cells[col + 1].content == CellContent::WideTail {
                        self.cells[col + 1] = Cell::blank_with_bg(style);
                    }
                }
                CellContent::Cluster(ref c) if c.width == 2 => {
                    if col + 1 < len && self.cells[col + 1].content == CellContent::WideTail {
                        self.cells[col + 1] = Cell::blank_with_bg(style);
                    }
                }
                CellContent::WideTail => {
                    if col > 0 {
                        self.cells[col - 1] = Cell::blank_with_bg(style);
                    }
                }
                _ => {}
            }
        }
    }

    /// Row text with trailing whitespace trimmed (for copy).
    pub fn text(&self) -> String {
        let mut s = String::new();
        for cell in &self.cells {
            cell.content.push_text(&mut s);
        }
        while s.ends_with(' ') {
            s.pop();
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wide_split() {
        let mut row = Row::new();
        *row.cell_mut(0) = Cell {
            content: CellContent::WideChar('漢'),
            ..Default::default()
        };
        *row.cell_mut(1) = Cell {
            content: CellContent::WideTail,
            ..Default::default()
        };
        // Overwriting the tail blanks the head.
        row.split_wide_at(1, &Style::default());
        assert_eq!(row.cells[0].content, CellContent::Empty);
        assert_eq!(row.cells[1].content, CellContent::WideTail);
    }

    #[test]
    fn trim_drops_default_tail() {
        let mut row = Row::new();
        *row.cell_mut(0) = Cell {
            content: CellContent::Char('a'),
            ..Default::default()
        };
        row.cell_mut(9); // grow with defaults
        assert_eq!(row.cells.len(), 10);
        row.trim();
        assert_eq!(row.cells.len(), 1);
    }
}

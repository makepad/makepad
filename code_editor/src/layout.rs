use {
    crate::{
        inlays::{BlockInlay, InlineInlay}, selection::Affinity, str::StrExt, widgets::{BlockWidget, InlineWidget},
        wrap::WrapData, Token,
        state::{SessionLayout, DocumentLayout},
        text::Text,
    },
    std::{cell::Ref, slice::Iter},
};

#[derive(Debug)]
pub struct Layout<'a> {
    pub text: Ref<'a, Text>,
    pub document_layout: Ref<'a, DocumentLayout>,
    pub session_layout: Ref<'a, SessionLayout>,
}

impl<'a> Layout<'a> {
    pub fn width(&self) -> f64 {
        let mut width: f64 = 0.0;
        for line in self.lines(0, self.line_count()) {
            width = width.max(line.width());
        }
        width
    }

    pub fn height(&self) -> f64 {
        self.session_layout.y[self.line_count()]
    }

    pub fn line_count(&self) -> usize {
        self.text.as_lines().len()
    }

    pub fn find_first_line_ending_after_y(&self, y: f64) -> usize {
        match self.session_layout.y[..self.session_layout.y.len() - 1]
            .binary_search_by(|current_y| current_y.partial_cmp(&y).unwrap())
        {
            Ok(line) => line,
            Err(line) => line.saturating_sub(1),
        }
    }

    pub fn find_first_line_starting_after_y(&self, y: f64) -> usize {
        match self.session_layout.y[..self.session_layout.y.len() - 1]
            .binary_search_by(|current_y| current_y.partial_cmp(&y).unwrap())
        {
            Ok(line) => line + 1,
            Err(line) => line,
        }
    }

    pub fn line(&self, index: usize) -> Line<'_> {
        Line {
            y: self.session_layout.y.get(index).copied(),
            column_count: self.session_layout.column_count[index],
            fold_column: self.session_layout.fold_column[index],
            scale: self.session_layout.scale[index],
            text: &self.text.as_lines()[index],
            tokens: &self.document_layout.tokens[index],
            inline_inlays: &self.document_layout.inline_inlays[index],
            wrap_data: self.session_layout.wrap_data[index].as_ref(),
        }
    }

    pub fn lines(&self, start: usize, end: usize) -> Lines<'_> {
        Lines {
            y: self.session_layout.y[start.min(self.session_layout.y.len())..end.min(self.session_layout.y.len())].iter(),
            column_count: self.session_layout.column_count[start..end].iter(),
            fold_column: self.session_layout.fold_column[start..end].iter(),
            scale: self.session_layout.scale[start..end].iter(),
            text: self.text.as_lines()[start..end].iter(),
            tokens: self.document_layout.tokens[start..end].iter(),
            inline_inlays: self.document_layout.inline_inlays[start..end].iter(),
            wrap_data: self.session_layout.wrap_data[start..end].iter(),
        }
    }

    pub fn blocks(&self, line_start: usize, line_end: usize) -> Blocks<'_> {
        let mut block_inlays = self.document_layout.block_inlays.iter();
        while block_inlays
            .as_slice()
            .first()
            .map_or(false, |&(position, _)| position < line_start)
        {
            block_inlays.next();
        }
        Blocks {
            lines: self.lines(line_start, line_end),
            block_inlays,
            position: line_start,
        }
    }
}


#[derive(Clone, Debug)]
pub struct Lines<'a> {
    y: Iter<'a, f64>,
    column_count: Iter<'a, Option<usize>>,
    fold_column: Iter<'a, usize>,
    scale: Iter<'a, f64>,
    text: Iter<'a, String>,
    tokens: Iter<'a, Vec<Token>>,
    inline_inlays: Iter<'a, Vec<(usize, InlineInlay)>>,
    wrap_data: Iter<'a, Option<WrapData>>,
}

impl<'a> Iterator for Lines<'a> {
    type Item = Line<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        let text = self.text.next()?;
        Some(Line {
            y: self.y.next().copied(),
            column_count: *self.column_count.next().unwrap(),
            fold_column: *self.fold_column.next().unwrap(),
            scale: *self.scale.next().unwrap(),
            text,
            tokens: self.tokens.next().unwrap(),
            inline_inlays: self.inline_inlays.next().unwrap(),
            wrap_data: self.wrap_data.next().unwrap().as_ref(),
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Line<'a> {
    pub y: Option<f64>,
    pub column_count: Option<usize>,
    pub fold_column: usize,
    pub scale: f64,
    pub text: &'a str,
    pub tokens: &'a [Token],
    pub inline_inlays: &'a [(usize, InlineInlay)],
    pub wrap_data: Option<&'a WrapData>,
}

impl<'a> Line<'a> {
    pub fn y(&self) -> f64 {
        self.y.unwrap()
    }

    pub fn row_count(&self) -> usize {
        self.wrap_data.unwrap().wraps.len() + 1
    }

    pub fn column_count(&self) -> usize {
        self.column_count.unwrap()
    }

    pub fn height(&self) -> f64 {
        self.row_count() as f64 * self.scale
    }

    pub fn width(&self) -> f64 {
        self.column_to_x(self.column_count())
    }

    pub fn byte_and_affinity_to_row_and_column(
        &self,
        byte: usize,
        affinity: Affinity,
        tab_column_count: usize,
    ) -> (usize, usize) {
        let mut current_byte = 0;
        let mut row = 0;
        let mut column = 0;
        if current_byte == byte && affinity == Affinity::Before {
            return (row, column);
        }
        for wrapped in self.wrappeds() {
            match wrapped {
                Wrapped::Text {
                    is_inlay: false,
                    text,
                } => {
                    for grapheme in text.graphemes() {
                        if current_byte == byte && affinity == Affinity::After {
                            return (row, column);
                        }
                        current_byte += grapheme.len();
                        column += grapheme.column_count(tab_column_count);
                        if current_byte == byte && affinity == Affinity::Before {
                            return (row, column);
                        }
                    }
                }
                Wrapped::Text {
                    is_inlay: true,
                    text,
                } => {
                    column += text.column_count(tab_column_count);
                }
                Wrapped::Widget(widget) => {
                    column += widget.column_count;
                }
                Wrapped::Wrap => {
                    row += 1;
                    column = self.wrap_indent_column_count();
                }
            }
        }
        if current_byte == byte && affinity == Affinity::After {
            return (row, column);
        }
        panic!()
    }

    pub fn row_and_column_to_byte_and_affinity(
        &self,
        row: usize,
        column: usize,
        tab_width: usize,
    ) -> (usize, Affinity) {
        let mut current_row = 0;
        let mut current_column = 0;
        let mut byte = 0;
        for wrapped in self.wrappeds() {
            match wrapped {
                Wrapped::Text {
                    is_inlay: false,
                    text,
                } => {
                    for grapheme in text.graphemes() {
                        let next_column = current_column + grapheme.column_count(tab_width);
                        if current_row == row && (current_column..next_column).contains(&column) {
                            return (byte, Affinity::After);
                        }
                        byte += grapheme.len();
                        current_column = next_column;
                    }
                }
                Wrapped::Text {
                    is_inlay: true,
                    text,
                } => {
                    let next_column = current_column + text.column_count(tab_width);
                    if current_row == row && (current_column..next_column).contains(&column) {
                        return (byte, Affinity::Before);
                    }
                    current_column = next_column;
                }
                Wrapped::Widget(widget) => {
                    current_column += widget.column_count;
                }
                Wrapped::Wrap => {
                    if current_row == row {
                        return (byte, Affinity::Before);
                    }
                    current_row += 1;
                    current_column = self.wrap_indent_column_count();
                }
            }
        }
        if current_row == row {
            return (byte, Affinity::After);
        }
        panic!()
    }

    pub fn column_to_x(&self, column: usize) -> f64 {
        let column_count_before_fold = column.min(self.fold_column);
        let column_count_after_fold = column - column_count_before_fold;
        column_count_before_fold as f64 + column_count_after_fold as f64 * self.scale
    }

    pub fn fold_column(&self) -> usize {
        self.fold_column
    }

    pub fn scale(&self) -> f64 {
        self.scale
    }

    pub fn wrap_indent_column_count(self) -> usize {
        self.wrap_data.unwrap().indent_column_count
    }

    pub fn text(&self) -> &str {
        self.text
    }

    pub fn tokens(&self) -> &[Token] {
        self.tokens
    }

    pub fn inlines(&self) -> Inlines<'a> {
        Inlines {
            text: self.text,
            inline_inlays: self.inline_inlays.iter(),
            position: 0,
        }
    }

    pub fn wrappeds(&self) -> Wrappeds<'a> {
        let mut inlines = self.inlines();
        Wrappeds {
            inline: inlines.next(),
            inlines,
            wraps: self.wrap_data.unwrap().wraps.iter(),
            position: 0,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Inlines<'a> {
    pub(super) text: &'a str,
    pub(super) inline_inlays: Iter<'a, (usize, InlineInlay)>,
    pub(super) position: usize,
}

impl<'a> Iterator for Inlines<'a> {
    type Item = Inline<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if self
            .inline_inlays
            .as_slice()
            .first()
            .map_or(false, |&(position, _)| position == self.position)
        {
            let (_, inline_inlay) = self.inline_inlays.next().unwrap();
            return Some(match *inline_inlay {
                InlineInlay::Text(ref text) => Inline::Text {
                    is_inlay: true,
                    text,
                },
                InlineInlay::Widget(widget) => Inline::Widget(widget),
            });
        }
        if self.text.is_empty() {
            return None;
        }
        let mut mid: usize = self.text.len();
        if let Some(&(byte, _)) = self.inline_inlays.as_slice().first() {
            mid = mid.min(byte - self.position);
        }
        let (text_0, text_1) = self.text.split_at(mid);
        self.text = text_1;
        self.position += text_0.len();
        Some(Inline::Text {
            is_inlay: false,
            text: text_0,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Inline<'a> {
    Text { is_inlay: bool, text: &'a str },
    Widget(InlineWidget),
}

#[derive(Clone, Debug)]
pub struct Wrappeds<'a> {
    pub(super) inline: Option<Inline<'a>>,
    pub(super) inlines: Inlines<'a>,
    pub(super) wraps: Iter<'a, usize>,
    pub(super) position: usize,
}

impl<'a> Iterator for Wrappeds<'a> {
    type Item = Wrapped<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if self
            .wraps
            .as_slice()
            .first()
            .map_or(false, |&position| position == self.position)
        {
            self.wraps.next();
            return Some(Wrapped::Wrap);
        }
        Some(match self.inline.take()? {
            Inline::Text { is_inlay, text } => {
                let mut mid: usize = text.len();
                if let Some(&position) = self.wraps.as_slice().first() {
                    mid = mid.min(position - self.position);
                }
                let text = if mid < text.len() {
                    let (text_0, text_1) = text.split_at(mid);
                    self.inline = Some(Inline::Text {
                        is_inlay,
                        text: text_1,
                    });
                    text_0
                } else {
                    self.inline = self.inlines.next();
                    text
                };
                self.position += text.len();
                Wrapped::Text { is_inlay, text }
            }
            Inline::Widget(widget) => {
                self.position += 1;
                Wrapped::Widget(widget)
            }
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Wrapped<'a> {
    Text { is_inlay: bool, text: &'a str },
    Widget(InlineWidget),
    Wrap,
}


#[derive(Clone, Debug)]
pub struct Blocks<'a> {
    lines: Lines<'a>,
    block_inlays: Iter<'a, (usize, BlockInlay)>,
    position: usize,
}

impl<'a> Iterator for Blocks<'a> {
    type Item = Block<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if self
            .block_inlays
            .as_slice()
            .first()
            .map_or(false, |&(line, _)| line == self.position)
        {
            let (_, block_inlay) = self.block_inlays.next().unwrap();
            return Some(match *block_inlay {
                BlockInlay::Widget(widget) => Block::Widget(widget),
            });
        }
        let line = self.lines.next()?;
        self.position += 1;
        Some(Block::Line {
            is_inlay: false,
            line,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Block<'a> {
    Line { is_inlay: bool, line: Line<'a> },
    Widget(BlockWidget),
}
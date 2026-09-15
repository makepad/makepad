use {
    crate::{
        document::{DocumentLayout, IndentState},
        inlays::{BlockInlay, InlineInlay},
        selection::Affinity,
        session::SessionLayout,
        str::StrExt,
        text::{Position, Text},
        widgets::{BlockWidget, InlineWidget},
        wrap::WrapData,
        Token,
    },
    std::{cell::Ref, ops::Range, slice::Iter},
};

#[derive(Debug)]
pub struct Layout<'a> {
    pub text: Ref<'a, Text>,
    pub document_layout: Ref<'a, DocumentLayout>,
    pub session_layout: Ref<'a, SessionLayout>,
    pub view_range: Option<Range<Position>>,
    pub tab_column_count: usize,
}

impl<'a> Layout<'a> {
    pub fn as_text(&self) -> &Text {
        &self.text
    }

    pub fn width(&self) -> f64 {
        let mut width: f64 = 0.0;
        for index in self.view_line_range() {
            let line = self.line(index);
            let bytes = self.line_byte_range(index);
            if bytes.start == 0 && bytes.end == line.text.len() {
                width = width.max(line.width());
                continue;
            }
            let (mut byte, mut row, mut column) = (0, 0, 0);
            for element in line.wrapped_elements() {
                match element {
                    WrappedElement::Text {
                        is_inlay: false,
                        text,
                    } => {
                        for grapheme in text.graphemes() {
                            column += grapheme.column_count_at(column, line.tab_column_count);
                            if bytes.contains(&byte) {
                                width = width.max(line.grid_to_normalized_position(row, column).0);
                            }
                            byte += grapheme.len();
                        }
                    }
                    WrappedElement::Text {
                        is_inlay: true,
                        text,
                    } => {
                        column += text.column_count_at(column, line.tab_column_count);
                    }
                    WrappedElement::Widget(widget) => {
                        column += widget.column_count;
                    }
                    WrappedElement::Wrap => {
                        row += 1;
                        column = line.wrap_indent_column_count();
                    }
                }
            }
        }
        width
    }

    pub fn height(&self) -> f64 {
        if self.view_range.is_none() {
            return *self.session_layout.y.last().unwrap();
        }
        let range = self.view_line_range();
        let line = self.line(range.end - 1);
        if self
            .view_range
            .as_ref()
            .is_some_and(|range| range.is_empty())
        {
            return line.scale();
        }
        let bytes = self.line_byte_range(range.end - 1);
        let last_byte = line.text[..bytes.end]
            .graphemes()
            .last()
            .map_or(0, |g| bytes.end - g.len());
        let (row, _) = line.logical_to_grid_position(last_byte, Affinity::After);
        (line.y() + (row + 1) as f64 * line.scale()).max(0.0)
    }

    pub fn view_line_range(&self) -> Range<usize> {
        match &self.view_range {
            Some(range) => {
                range.start.line_index
                    ..(range.end.line_index
                        + usize::from(
                            range.end.byte_index > 0
                                || range.start.line_index == range.end.line_index,
                        ))
            }
            None => 0..self.text.as_lines().len(),
        }
    }

    pub fn line_byte_range(&self, index: usize) -> Range<usize> {
        let mut bytes = 0..self.text.as_lines()[index].len();
        if let Some(range) = &self.view_range {
            if index < range.start.line_index || index > range.end.line_index {
                return 0..0;
            }
            if index == range.start.line_index {
                bytes.start = range.start.byte_index;
            }
            if index == range.end.line_index {
                bytes.end = range.end.byte_index;
            }
        }
        bytes
    }

    fn view_y_origin(&self) -> f64 {
        let Some(range) = &self.view_range else {
            return 0.0;
        };
        let mut line = self.line_without_view(range.start.line_index);
        line.y = Some(0.0);
        let (row, _) = line.logical_to_grid_position(range.start.byte_index, Affinity::After);
        self.session_layout
            .y
            .get(range.start.line_index)
            .copied()
            .unwrap_or(0.0)
            + row as f64 * line.scale()
    }

    pub fn find_first_line_ending_after_y(&self, y: f64) -> usize {
        let range = self.view_line_range();
        let y = y + self.view_y_origin();
        let index = self.session_layout.y[range.clone()].partition_point(|current| *current <= y);
        (range.start + index.saturating_sub(1)).min(range.end - 1)
    }

    pub fn find_first_line_starting_after_y(&self, y: f64) -> usize {
        let range = self.view_line_range();
        let y = y + self.view_y_origin();
        range.start + self.session_layout.y[range].partition_point(|current| *current <= y)
    }

    pub fn logical_to_normalized_position(
        &self,
        position: Position,
        affinity: Affinity,
    ) -> (f64, f64) {
        let line = self.line(position.line_index);
        let (row_index, column_index) =
            line.logical_to_grid_position(position.byte_index, affinity);
        let (x, y) = line.grid_to_normalized_position(row_index, column_index);
        (x, line.y() + y)
    }

    pub fn line(&self, index: usize) -> Line<'_> {
        let mut line = self.line_without_view(index);
        line.y = line.y.map(|y| y - self.view_y_origin());
        line
    }

    fn line_without_view(&self, index: usize) -> Line<'_> {
        Line {
            y: self.session_layout.y.get(index).copied(),
            column_count: self.session_layout.column_count[index],
            fold: self.session_layout.fold_column[index],
            scale: self.session_layout.scale[index],
            text: &self.text.as_lines()[index],
            indent_state: self.document_layout.indent_state[index],
            tokens: &self.document_layout.tokens[index],
            inlays: &self.document_layout.inline_inlays[index],
            wrap_data: self.session_layout.wrap_data[index].as_ref(),
            tab_column_count: self.tab_column_count,
        }
    }

    pub fn lines(&self, start: usize, end: usize) -> Lines<'_> {
        Lines {
            y_offset: self.view_y_origin(),
            y: self.session_layout.y
                [start.min(self.session_layout.y.len())..end.min(self.session_layout.y.len())]
                .iter(),
            column_count: self.session_layout.column_count[start..end].iter(),
            fold: self.session_layout.fold_column[start..end].iter(),
            scale: self.session_layout.scale[start..end].iter(),
            text: self.text.as_lines()[start..end].iter(),
            indent_state: self.document_layout.indent_state[start..end].iter(),
            tokens: self.document_layout.tokens[start..end].iter(),
            inline_inlays: self.document_layout.inline_inlays[start..end].iter(),
            wrap_data: self.session_layout.wrap_data[start..end].iter(),
            tab_column_count: self.tab_column_count,
        }
    }

    pub fn block_elements(&self, line_start: usize, line_end: usize) -> BlockElements<'_> {
        let mut block_inlays = self.document_layout.block_inlays.iter();
        while block_inlays
            .as_slice()
            .first()
            .map_or(false, |&(position, _)| position < line_start)
        {
            block_inlays.next();
        }
        BlockElements {
            lines: self.lines(line_start, line_end),
            block_inlays,
            position: line_start,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Lines<'a> {
    y_offset: f64,
    y: Iter<'a, f64>,
    column_count: Iter<'a, Option<usize>>,
    fold: Iter<'a, usize>,
    scale: Iter<'a, f64>,
    text: Iter<'a, String>,
    indent_state: Iter<'a, Option<IndentState>>,
    tokens: Iter<'a, Vec<Token>>,
    inline_inlays: Iter<'a, Vec<(usize, InlineInlay)>>,
    wrap_data: Iter<'a, Option<WrapData>>,
    tab_column_count: usize,
}

impl<'a> Iterator for Lines<'a> {
    type Item = Line<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        let text = self.text.next()?;
        Some(Line {
            y: self.y.next().map(|y| y - self.y_offset),
            column_count: *self.column_count.next().unwrap(),
            fold: *self.fold.next().unwrap(),
            scale: *self.scale.next().unwrap(),
            text,
            indent_state: *self.indent_state.next().unwrap(),
            tokens: self.tokens.next().unwrap(),
            inlays: self.inline_inlays.next().unwrap(),
            wrap_data: self.wrap_data.next().unwrap().as_ref(),
            tab_column_count: self.tab_column_count,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Line<'a> {
    pub y: Option<f64>,
    pub column_count: Option<usize>,
    pub fold: usize,
    pub scale: f64,
    pub text: &'a str,
    pub indent_state: Option<IndentState>,
    pub tokens: &'a [Token],
    pub inlays: &'a [(usize, InlineInlay)],
    pub wrap_data: Option<&'a WrapData>,
    pub tab_column_count: usize,
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

    pub fn width(&self) -> f64 {
        let mut width: f64 = 0.0;
        for row_index in 0..self.row_count() {
            let (x, _) = self.grid_to_normalized_position(row_index, self.column_count());
            width = width.max(x);
        }
        width
    }

    pub fn height(&self) -> f64 {
        self.row_count() as f64 * self.scale
    }

    pub fn logical_to_grid_position(
        &self,
        byte_index: usize,
        affinity: Affinity,
    ) -> (usize, usize) {
        let mut current_byte_index = 0;
        let mut current_row_index = 0;
        let mut current_column_index = 0;
        if current_byte_index == byte_index && affinity == Affinity::Before {
            return (current_row_index, current_column_index);
        }
        for element in self.wrapped_elements() {
            match element {
                WrappedElement::Text {
                    is_inlay: false,
                    text,
                } => {
                    for grapheme in text.graphemes() {
                        if current_byte_index == byte_index && affinity == Affinity::After {
                            return (current_row_index, current_column_index);
                        }
                        current_byte_index += grapheme.len();
                        current_column_index +=
                            grapheme.column_count_at(current_column_index, self.tab_column_count);
                        if current_byte_index == byte_index && affinity == Affinity::Before {
                            return (current_row_index, current_column_index);
                        }
                    }
                }
                WrappedElement::Text {
                    is_inlay: true,
                    text,
                } => {
                    current_column_index +=
                        text.column_count_at(current_column_index, self.tab_column_count);
                }
                WrappedElement::Widget(widget) => {
                    current_column_index += widget.column_count;
                }
                WrappedElement::Wrap => {
                    current_row_index += 1;
                    current_column_index = self.wrap_indent_column_count();
                }
            }
        }
        if current_byte_index == byte_index && affinity == Affinity::After {
            return (current_row_index, current_column_index);
        }
        panic!()
    }

    pub fn grid_to_logical_position(
        &self,
        row_index: usize,
        column_index: usize,
    ) -> (usize, Affinity) {
        let mut current_row_index = 0;
        let mut current_column_index = 0;
        let mut current_byte_index = 0;
        for element in self.wrapped_elements() {
            match element {
                WrappedElement::Text {
                    is_inlay: false,
                    text,
                } => {
                    for grapheme in text.graphemes() {
                        let next_column = current_column_index
                            + grapheme.column_count_at(current_column_index, self.tab_column_count);
                        if current_row_index == row_index
                            && (current_column_index..next_column).contains(&column_index)
                        {
                            return (current_byte_index, Affinity::After);
                        }
                        current_byte_index += grapheme.len();
                        current_column_index = next_column;
                    }
                }
                WrappedElement::Text {
                    is_inlay: true,
                    text,
                } => {
                    let next_column = current_column_index
                        + text.column_count_at(current_column_index, self.tab_column_count);
                    if current_row_index == row_index
                        && (current_column_index..next_column).contains(&column_index)
                    {
                        return (current_byte_index, Affinity::Before);
                    }
                    current_column_index = next_column;
                }
                WrappedElement::Widget(widget) => {
                    current_column_index += widget.column_count;
                }
                WrappedElement::Wrap => {
                    if current_row_index == row_index {
                        return (current_byte_index, Affinity::Before);
                    }
                    current_row_index += 1;
                    current_column_index = self.wrap_indent_column_count();
                }
            }
        }
        if current_row_index == row_index {
            return (current_byte_index, Affinity::After);
        }
        panic!()
    }

    pub fn grid_to_normalized_position(&self, row_index: usize, column_index: usize) -> (f64, f64) {
        let before_fold = column_index.min(self.fold);
        let after_fold = column_index - before_fold;
        (
            before_fold as f64 + after_fold as f64 * self.scale,
            row_index as f64 * self.scale,
        )
    }

    pub fn fold(&self) -> usize {
        self.fold
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

    pub fn indent_column_count(&self) -> usize {
        match self.indent_state.unwrap() {
            IndentState::Empty(indent_column_count) => indent_column_count,
            IndentState::NonEmpty(indent_column_count, _) => indent_column_count,
        }
    }

    pub fn tokens(&self) -> &[Token] {
        self.tokens
    }

    pub fn inline_elements(&self) -> InlineElements<'a> {
        InlineElements {
            text: self.text,
            inlays: self.inlays.iter(),
            position: 0,
        }
    }

    pub fn wrapped_elements(&self) -> WrappedElements<'a> {
        let mut elements = self.inline_elements();
        WrappedElements {
            element: elements.next(),
            elements,
            wraps: self.wrap_data.unwrap().wraps.iter(),
            position: 0,
        }
    }
}

#[derive(Clone, Debug)]
pub struct InlineElements<'a> {
    text: &'a str,
    inlays: Iter<'a, (usize, InlineInlay)>,
    position: usize,
}

impl<'a> Iterator for InlineElements<'a> {
    type Item = InlineElement<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if self
            .inlays
            .as_slice()
            .first()
            .map_or(false, |&(position, _)| position == self.position)
        {
            let (_, inline_inlay) = self.inlays.next().unwrap();
            return Some(match *inline_inlay {
                InlineInlay::Text(ref text) => InlineElement::Text {
                    is_inlay: true,
                    text,
                },
                InlineInlay::Widget(widget) => InlineElement::Widget(widget),
            });
        }
        if self.text.is_empty() {
            return None;
        }
        let mut len: usize = self.text.len();
        if let Some(&(position, _)) = self.inlays.as_slice().first() {
            len = len.min(position - self.position);
        }
        let (text_0, text_1) = self.text.split_at(len);
        self.text = text_1;
        self.position += text_0.len();
        Some(InlineElement::Text {
            is_inlay: false,
            text: text_0,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum InlineElement<'a> {
    Text { is_inlay: bool, text: &'a str },
    Widget(InlineWidget),
}

#[derive(Clone, Debug)]
pub struct WrappedElements<'a> {
    element: Option<InlineElement<'a>>,
    elements: InlineElements<'a>,
    wraps: Iter<'a, usize>,
    position: usize,
}

impl<'a> Iterator for WrappedElements<'a> {
    type Item = WrappedElement<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if self
            .wraps
            .as_slice()
            .first()
            .map_or(false, |&position| position == self.position)
        {
            self.wraps.next();
            return Some(WrappedElement::Wrap);
        }
        Some(match self.element.take()? {
            InlineElement::Text { is_inlay, text } => {
                let mut len: usize = text.len();
                if let Some(&position) = self.wraps.as_slice().first() {
                    len = len.min(position - self.position);
                }
                let text = if len < text.len() {
                    let (text_0, text_1) = text.split_at(len);
                    self.element = Some(InlineElement::Text {
                        is_inlay,
                        text: text_1,
                    });
                    text_0
                } else {
                    self.element = self.elements.next();
                    text
                };
                self.position += text.len();
                WrappedElement::Text { is_inlay, text }
            }
            InlineElement::Widget(widget) => {
                self.position += 1;
                WrappedElement::Widget(widget)
            }
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum WrappedElement<'a> {
    Text { is_inlay: bool, text: &'a str },
    Widget(InlineWidget),
    Wrap,
}

#[derive(Clone, Debug)]
pub struct BlockElements<'a> {
    lines: Lines<'a>,
    block_inlays: Iter<'a, (usize, BlockInlay)>,
    position: usize,
}

impl<'a> Iterator for BlockElements<'a> {
    type Item = BlockElement<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if self
            .block_inlays
            .as_slice()
            .first()
            .map_or(false, |&(line, _)| line == self.position)
        {
            let (_, block_inlay) = self.block_inlays.next().unwrap();
            return Some(match *block_inlay {
                BlockInlay::Widget(widget) => BlockElement::Widget(widget),
            });
        }
        let line = self.lines.next()?;
        self.position += 1;
        Some(BlockElement::Line {
            is_inlay: false,
            line,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BlockElement<'a> {
    Line { is_inlay: bool, line: Line<'a> },
    Widget(BlockWidget),
}

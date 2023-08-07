use {
    super::{Block, Blocks, Document, Line, Lines, Session, Wrapped},
    crate::{char::CharExt, Settings},
    std::{mem, ops::Range},
};

#[derive(Clone, Debug)]
pub struct View<'a> {
    pub(super) settings: &'a Settings,
    pub(super) session: &'a Session,
    pub(super) document: &'a Document,
}

impl<'a> View<'a> {
    pub fn width(&self) -> f64 {
        let mut width: f64 = 0.0;
        for line in self.lines(0..self.line_count()) {
            width = width.max(line.width());
        }
        width
    }

    pub fn height(&self) -> f64 {
        let line = self.line(self.line_count() - 1);
        let mut y = line.y() + line.height();
        for block in self.blocks(self.line_count()..self.line_count()) {
            match block {
                Block::Line {
                    is_inlay: true,
                    line,
                } => y += line.height(),
                Block::Widget(widget) => y += widget.height,
                _ => unreachable!(),
            }
        }
        y
    }

    pub fn max_column(&self) -> usize {
        self.session.max_column
    }

    pub fn line_count(&self) -> usize {
        self.document.text.len()
    }

    pub fn find_first_line_ending_after_y(&self, y: f64) -> usize {
        match self
            .session
            .y
            .binary_search_by(|current_y| current_y.partial_cmp(&y).unwrap())
        {
            Ok(index) => index,
            Err(index) => (index + 1).min(self.line_count()),
        }
    }

    pub fn find_first_line_starting_after_y(&self, y: f64) -> usize {
        match self
            .session
            .y
            .binary_search_by(|current_y| current_y.partial_cmp(&y).unwrap())
        {
            Ok(index) => index + 1,
            Err(index) => index,
        }
    }

    pub fn line(&self, index: usize) -> Line<'a> {
        Line {
            settings: self.settings,
            y: self.session.y.get(index).copied(),
            column_count: self.session.column_count[index],
            fold: self.session.fold[index],
            scale: self.session.scale[index],
            text: &self.document.text[index],
            tokens: &self.document.tokens[index],
            inline_inlays: &self.document.inline_inlays[index],
            wraps: &self.session.wraps[index],
            indent: self.session.indent[index],
        }
    }

    pub fn lines(&self, range: Range<usize>) -> Lines<'a> {
        Lines {
            settings: self.settings,
            y: self.session.y
                [range.start.min(self.session.y.len())..range.end.min(self.session.y.len())]
                .iter(),
            column_count: self.session.column_count[range.start..range.end].iter(),
            fold: self.session.fold[range.start..range.end].iter(),
            scale: self.session.scale[range.start..range.end].iter(),
            indent: self.session.indent[range.start..range.end].iter(),
            text: self.document.text[range.start..range.end].iter(),
            tokens: self.document.tokens[range.start..range.end].iter(),
            inline_inlays: self.document.inline_inlays[range.start..range.end].iter(),
            wraps: self.session.wraps[range.start..range.end].iter(),
        }
    }

    pub fn blocks(&self, range: Range<usize>) -> Blocks<'a> {
        let mut block_inlays = self.document.block_inlays.iter();
        while block_inlays
            .as_slice()
            .first()
            .map_or(false, |&(index, _)| index < range.start)
        {
            block_inlays.next();
        }
        Blocks {
            lines: self.lines(range.start..range.end),
            block_inlays,
            index: range.start,
        }
    }
}

#[derive(Debug)]
pub struct ViewMut<'a> {
    pub(super) settings: &'a Settings,
    pub(super) session: &'a mut Session,
    pub(super) document: &'a Document,
}

impl<'a> ViewMut<'a> {
    pub fn as_view(&self) -> View<'_> {
        View {
            settings: self.settings,
            session: self.session,
            document: self.document,
        }
    }

    pub fn width(&self) -> f64 {
        self.as_view().width()
    }

    pub fn height(&self) -> f64 {
        self.as_view().height()
    }

    pub fn max_column(&self) -> usize {
        self.as_view().max_column()
    }

    pub fn line_count(&self) -> usize {
        self.as_view().line_count()
    }

    pub fn find_first_line_ending_after_y(&self, y: f64) -> usize {
        self.as_view().find_first_line_ending_after_y(y)
    }

    pub fn find_first_line_starting_after_y(&self, y: f64) -> usize {
        self.as_view().find_first_line_starting_after_y(y)
    }

    pub fn line(&self, index: usize) -> Line<'_> {
        self.as_view().line(index)
    }

    pub fn lines(&self, range: Range<usize>) -> Lines<'_> {
        self.as_view().lines(range)
    }

    pub fn blocks(&self, range: Range<usize>) -> Blocks<'_> {
        self.as_view().blocks(range)
    }

    pub fn set_max_column(&mut self, max_column: usize) {
        if self.session.max_column == max_column {
            return;
        }
        self.session.max_column = max_column;
        for index in 0..self.line_count() {
            self.update_indent_and_wraps(index);
        }
        self.update_y();
    }

    pub(super) fn update_y(&mut self) {
        let start = self.session.y.len();
        if start == self.line_count() + 1 {
            return;
        }
        let mut y = if start == 0 {
            0.0
        } else {
            let line = self.line(start - 1);
            line.y() + line.height()
        };
        let mut ys = mem::take(&mut self.session.y);
        for block in self.blocks(start..self.line_count()) {
            match block {
                Block::Line { is_inlay, line } => {
                    if !is_inlay {
                        ys.push(y);
                    }
                    y += line.height();
                }
                Block::Widget(widget) => {
                    y += widget.height;
                }
            }
        }
        ys.push(y);
        self.session.y = ys;
    }

    pub(super) fn update_column_count(&mut self, index: usize) {
        let mut column_count = 0;
        let mut column = 0;
        let line = self.line(index);
        for wrapped in line.wrappeds() {
            match wrapped {
                Wrapped::Text { text, .. } => {
                    column += text
                        .chars()
                        .map(|char| char.column_count(self.settings.tab_column_count))
                        .sum::<usize>();
                }
                Wrapped::Widget(widget) => {
                    column += widget.column_count;
                }
                Wrapped::Wrap => {
                    column_count = column_count.max(column);
                    column = line.indent();
                }
            }
        }
        self.session.column_count[index] = Some(column_count.max(column));
    }

    pub(super) fn update_indent_and_wraps(&mut self, index: usize) {
        let (indent, wraps) = self
            .line(index)
            .compute_indent_and_wraps(self.session.max_column);
        self.session.wraps[index] = wraps;
        self.session.indent[index] = indent;
        self.update_column_count(index);
        self.session.y.truncate(index + 1);
    }
}

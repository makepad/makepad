use {
    crate::{
        geometry::{Point, Rect, Size},
        text::Position,
    },
    std::{
        cell::RefCell,
        collections::{HashMap, HashSet},
        ops::ControlFlow,
        slice::Iter,
    },
};

#[derive(Clone, Debug, Default)]
pub struct State {
    session_id: usize,
    sessions: HashMap<SessionId, Session>,
    document_id: usize,
    documents: HashMap<DocumentId, Document>,
}

impl State {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn view(&self, session_id: SessionId) -> View<'_> {
        let session = &self.sessions[&session_id];
        let document = &self.documents[&session.document_id];
        View {
            text: &document.text,
            inline_inlays: &document.inline_inlays,
            soft_breaks: &session.soft_breaks,
            scale: &session.scale,
            pivot: &session.pivot,
            block_inlays: &document.block_inlays,
            summed_scaled_heights: &session.summed_scaled_heights,
        }
    }

    pub fn view_mut(&mut self, session_id: SessionId) -> ViewMut<'_> {
        let session = self.sessions.get_mut(&session_id).unwrap();
        let document = self.documents.get_mut(&session.document_id).unwrap();
        ViewMut {
            text: &mut document.text,
            inline_inlays: &mut document.inline_inlays,
            soft_breaks: &mut session.soft_breaks,
            scale: &mut session.scale,
            pivot: &mut session.pivot,
            block_inlays: &mut document.block_inlays,
            summed_scaled_heights: &mut session.summed_scaled_heights,
            folding_lines: &mut session.folding_lines,
            unfolding_lines: &mut session.unfolding_lines,
        }
    }

    pub fn open_session(&mut self) -> SessionId {
        let document_id = self.open_document();
        let session_id = SessionId(self.session_id);
        let line_count = self.documents[&document_id].text.len();
        self.sessions.insert(
            session_id,
            Session {
                document_id,
                soft_breaks: (0..line_count).map(|_| [].into()).collect(),
                pivot: (0..line_count).map(|_| 0).collect(),
                scale: (0..line_count).map(|_| 1.0).collect(),
                summed_scaled_heights: RefCell::new(Vec::new()),
                folding_lines: HashSet::new(),
                unfolding_lines: HashSet::new(),
            },
        );
        self.session_id += 1;
        session_id
    }

    fn open_document(&mut self) -> DocumentId {
        let document_id = DocumentId(self.document_id);
        self.document_id += 1;
        let text: Vec<String> = include_str!("state.rs")
            .lines()
            .map(|line| line.to_string())
            .collect();
        let line_count = text.len();
        self.documents.insert(
            document_id,
            Document {
                text,
                inline_inlays: (0..line_count)
                    .map(|_| {
                        [
                            (20, InlineInlay::Text("uvw xyz".into())),
                            (40, InlineInlay::Text("uvw xyz".into())),
                            (60, InlineInlay::Text("uvw xyz".into())),
                            (80, InlineInlay::Text("uvw xyz".into())),
                        ]
                        .into()
                    })
                    .collect(),
                block_inlays: [
                    (10, BlockInlay::Line(LineInlay::new("UVW XYZ".into()))),
                    (20, BlockInlay::Line(LineInlay::new("UVW XYZ".into()))),
                    (30, BlockInlay::Line(LineInlay::new("UVW XYZ".into()))),
                    (40, BlockInlay::Line(LineInlay::new("UVW XYZ".into()))),
                ]
                .into(),
            },
        );
        document_id
    }
}

#[derive(Clone, Copy, Debug)]
pub struct View<'a> {
    text: &'a [String],
    inline_inlays: &'a [Vec<(usize, InlineInlay)>],
    soft_breaks: &'a [Vec<usize>],
    pivot: &'a [usize],
    scale: &'a [f64],
    block_inlays: &'a [(usize, BlockInlay)],
    summed_scaled_heights: &'a RefCell<Vec<f64>>,
}

impl<'a> View<'a> {
    pub fn line_count(&self) -> usize {
        self.text.len()
    }

    pub fn line(&self, line_index: usize) -> Line<'a> {
        Line {
            text: &self.text[line_index],
            inline_inlays: &self.inline_inlays[line_index],
            soft_breaks: &self.soft_breaks[line_index],
            pivot: self.pivot[line_index],
            scale: self.scale[line_index],
        }
    }

    pub fn lines(&self, start_line_index: usize, end_line_index: usize) -> Lines<'a> {
        Lines {
            text: self.text[start_line_index..end_line_index].iter(),
            inline_inlays: self.inline_inlays.iter(),
            soft_breaks: self.soft_breaks.iter(),
            pivot: self.pivot.iter(),
            scale: self.scale.iter(),
        }
    }

    pub fn blocks(&self, start_line_index: usize, end_line_index: usize) -> Blocks<'a> {
        Blocks {
            lines: self.lines(start_line_index, end_line_index),
            block_inlays: self.block_inlays[self
                .block_inlays
                .iter()
                .position(|&(index, _)| index >= start_line_index)
                .unwrap_or(self.block_inlays.len())..]
                .iter(),
            line_index: start_line_index,
        }
    }

    pub fn scaled_width(&self, tab_width: usize) -> f64 {
        let mut max_width = 0.0f64;
        for block in self.blocks(0, self.line_count()) {
            max_width = max_width.max(block.scaled_width(tab_width));
        }
        max_width
    }

    pub fn summed_scaled_height(&self, line_index: usize) -> f64 {
        self.update_summed_scaled_heights();
        self.summed_scaled_heights.borrow()[line_index]
    }

    pub fn scaled_height(&self) -> f64 {
        self.summed_scaled_height(self.line_count() - 1)
    }

    pub fn find_first_line_ending_after(&self, scaled_y: f64) -> usize {
        self.update_summed_scaled_heights();
        match self
            .summed_scaled_heights
            .borrow()
            .binary_search_by(|summed_scaled_heights| {
                summed_scaled_heights.partial_cmp(&scaled_y).unwrap()
            }) {
            Ok(index) => index + 1,
            Err(index) => index,
        }
    }

    pub fn find_first_line_starting_after(&self, scaled_y: f64) -> usize {
        self.update_summed_scaled_heights();
        match self
            .summed_scaled_heights
            .borrow()
            .binary_search_by(|summed_scaled_height| {
                summed_scaled_height.partial_cmp(&scaled_y).unwrap()
            }) {
            Ok(index) => index + 1,
            Err(index) => {
                if index == self.line_count() {
                    index
                } else {
                    index + 1
                }
            }
        }
    }

    pub fn layout<T>(
        &self,
        start_line_index: usize,
        end_line_index: usize,
        tab_width: usize,
        mut handle_event: impl FnMut(LayoutEvent<'_>) -> ControlFlow<T, bool>,
    ) -> ControlFlow<T, bool> {
        use crate::str::StrExt;

        let mut scaled_y = if start_line_index == 0 {
            0.0
        } else {
            self.summed_scaled_height(start_line_index - 1)
        };
        for block in self.blocks(start_line_index, end_line_index) {
            match block {
                Block::Line { is_inlay, line } => {
                    if !handle_event(LayoutEvent {
                        scaled_rect: Rect::new(
                            Point::new(0.0, scaled_y),
                            Size::new(line.scaled_width(tab_width), line.scaled_height()),
                        ),
                        kind: LayoutEventKind::Line { is_inlay, line },
                    })? {
                        scaled_y += line.scaled_height();
                        continue;
                    }
                    let mut x = 0;
                    for wrapped_inline in line.wrapped_inlines() {
                        match wrapped_inline {
                            WrappedInline::Inline(inline) => match inline {
                                Inline::Text { is_inlay, text } => {
                                    for grapheme in text.graphemes() {
                                        let scaled_x = line.scaled_x(x);
                                        let next_x = x + grapheme.width(tab_width);
                                        handle_event(LayoutEvent {
                                            scaled_rect: Rect::new(
                                                Point::new(scaled_x, scaled_y),
                                                Size::new(
                                                    line.scaled_x(next_x) - scaled_x,
                                                    line.scale(),
                                                ),
                                            ),
                                            kind: LayoutEventKind::Grapheme {
                                                is_inlay,
                                                text: grapheme,
                                            },
                                        })?;
                                        x = next_x;
                                    }
                                }
                                Inline::Widget(widget) => {
                                    let scaled_x = line.scaled_x(x);
                                    let next_x = x + widget.width;
                                    handle_event(LayoutEvent {
                                        scaled_rect: Rect::new(
                                            Point::new(line.scaled_x(x), scaled_y),
                                            Size::new(
                                                line.scaled_x(next_x) - scaled_x,
                                                line.scale(),
                                            ),
                                        ),
                                        kind: LayoutEventKind::Widget { id: widget.id },
                                    })?;
                                    x = next_x;
                                }
                            },
                            WrappedInline::SoftBreak => {
                                handle_event(LayoutEvent {
                                    scaled_rect: Rect::new(
                                        Point::new(line.scaled_x(x), scaled_y),
                                        Size::new(0.0, line.scale()),
                                    ),
                                    kind: LayoutEventKind::SoftBreak,
                                })?;
                                scaled_y += line.scale();
                                x = 0;
                            }
                        }
                    }
                    handle_event(LayoutEvent {
                        scaled_rect: Rect::new(
                            Point::new(line.scaled_x(x), scaled_y),
                            Size::new(0.0, line.scale()),
                        ),
                        kind: LayoutEventKind::Break,
                    })?;
                    scaled_y += line.scale();
                }
                Block::Widget(widget) => {
                    handle_event(LayoutEvent {
                        scaled_rect: Rect::new(Point::new(0.0, scaled_y), widget.scaled_size),
                        kind: LayoutEventKind::Widget { id: widget.id },
                    })?;
                    scaled_y += widget.scaled_size.height;
                }
            }
        }
        ControlFlow::Continue(true)
    }

    pub fn pick(&self, scaled_point: Point, tab_width: usize) -> Option<Position> {
        let mut position = Position::origin();
        match self.layout(0, self.line_count(), tab_width, |event| {
            match event.kind {
                LayoutEventKind::Line { is_inlay: true, .. } => {
                    if event.scaled_rect.contains(scaled_point) {
                        return ControlFlow::Break(Some(position));
                    }
                    return ControlFlow::Continue(false);
                }
                LayoutEventKind::Widget { .. } => {
                    return ControlFlow::Break(None);
                }
                LayoutEventKind::Grapheme { is_inlay, text } => {
                    if event.scaled_rect.contains(scaled_point) {
                        return ControlFlow::Break(Some(position));
                    }
                    if !is_inlay {
                        position.byte_index += text.len();
                    }
                }
                LayoutEventKind::Break => {
                    position.line_index += 1;
                    position.byte_index = 0;
                }
                _ => {}
            }
            ControlFlow::Continue(true)
        }) {
            ControlFlow::Continue(_) => None,
            ControlFlow::Break(position) => position,
        }
    }

    fn update_summed_scaled_heights(&self) {
        let start_line_index = self.summed_scaled_heights.borrow().len();
        let mut summed_scaled_height = if start_line_index == 0 {
            0.0
        } else {
            self.summed_scaled_heights.borrow()[start_line_index - 1]
        };
        for block in self.blocks(start_line_index, self.line_count()) {
            summed_scaled_height += block.scaled_height();
            if let Block::Line {
                is_inlay: false, ..
            } = block
            {
                self.summed_scaled_heights
                    .borrow_mut()
                    .push(summed_scaled_height);
            }
        }
    }
}

#[derive(Debug)]
pub struct ViewMut<'a> {
    text: &'a mut Vec<String>,
    inline_inlays: &'a mut Vec<Vec<(usize, InlineInlay)>>,
    soft_breaks: &'a mut Vec<Vec<usize>>,
    scale: &'a mut Vec<f64>,
    pivot: &'a mut Vec<usize>,
    summed_scaled_heights: &'a mut RefCell<Vec<f64>>,
    block_inlays: &'a mut Vec<(usize, BlockInlay)>,
    folding_lines: &'a mut HashSet<usize>,
    unfolding_lines: &'a mut HashSet<usize>,
}

impl<'a> ViewMut<'a> {
    pub fn as_view(&self) -> View<'_> {
        View {
            text: &self.text,
            inline_inlays: &self.inline_inlays,
            soft_breaks: &self.soft_breaks,
            scale: self.scale,
            pivot: self.pivot,
            block_inlays: &self.block_inlays,
            summed_scaled_heights: &self.summed_scaled_heights,
        }
    }

    pub fn wrap_line(&mut self, line_index: usize, max_width: usize, tab_width: usize) {
        use std::mem;

        self.soft_breaks[line_index].clear();
        let mut soft_breaks = mem::take(&mut self.soft_breaks[line_index]);
        let mut offset = 0;
        let mut width = 0;
        for inline in self.as_view().line(line_index).inlines() {
            let mut next_width = width + inline.width(tab_width);
            if next_width > max_width {
                next_width = 0;
                soft_breaks.push(offset);
            }
            if let Inline::Text { text, .. } = inline {
                offset += text.len();
            }
            width = next_width;
        }
        self.soft_breaks[line_index] = soft_breaks;
        self.summed_scaled_heights.borrow_mut().truncate(line_index);
    }

    pub fn unwrap_line(&mut self, line_index: usize) {
        self.soft_breaks[line_index].clear();
        self.summed_scaled_heights.borrow_mut().truncate(line_index);
    }

    pub fn fold_line(&mut self, line_index: usize, pivot: usize) {
        self.pivot[line_index] = pivot;
        self.unfolding_lines.remove(&line_index);
        self.folding_lines.insert(line_index);
    }

    pub fn unfold_line(&mut self, line_index: usize) {
        self.folding_lines.remove(&line_index);
        self.unfolding_lines.insert(line_index);
    }

    pub fn update_fold_animations(&mut self) -> bool {
        use std::mem;

        if self.folding_lines.is_empty() && self.unfolding_lines.is_empty() {
            return false;
        }
        let folding_lines = mem::take(self.folding_lines);
        let mut new_folding_lines = HashSet::new();
        for index in folding_lines {
            self.scale[index] *= 0.9;
            if self.scale[index] < 0.001 {
                self.scale[index] = 0.0;
            } else {
                new_folding_lines.insert(index);
            }
            self.summed_scaled_heights.borrow_mut().truncate(index);
        }
        *self.folding_lines = new_folding_lines;
        let unfolding_lines = mem::take(self.unfolding_lines);
        let mut new_unfolding_lines = HashSet::new();
        for index in unfolding_lines {
            self.scale[index] = 1.0 - 0.9 * (1.0 - self.scale[index]);
            if self.scale[index] > 1.0 - 0.001 {
                self.scale[index] = 1.0;
            } else {
                new_unfolding_lines.insert(index);
            }
            self.summed_scaled_heights.borrow_mut().truncate(index);
        }
        *self.unfolding_lines = new_unfolding_lines;
        true
    }
}

#[derive(Debug)]
pub struct Lines<'a> {
    text: Iter<'a, String>,
    inline_inlays: Iter<'a, Vec<(usize, InlineInlay)>>,
    soft_breaks: Iter<'a, Vec<usize>>,
    pivot: Iter<'a, usize>,
    scale: Iter<'a, f64>,
}

impl<'a> Clone for Lines<'a> {
    fn clone(&self) -> Self {
        unimplemented!()
    }
}

impl<'a> Iterator for Lines<'a> {
    type Item = Line<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        Some(Line {
            text: self.text.next()?,
            inline_inlays: self.inline_inlays.next()?,
            soft_breaks: self.soft_breaks.next()?,
            pivot: *self.pivot.next()?,
            scale: *self.scale.next()?,
        })
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Line<'a> {
    text: &'a str,
    inline_inlays: &'a [(usize, InlineInlay)],
    soft_breaks: &'a [usize],
    pivot: usize,
    scale: f64,
}

impl<'a> Line<'a> {
    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn inlines(&self) -> Inlines<'a> {
        Inlines {
            text: self.text,
            inline_inlays: self.inline_inlays.iter(),
            byte_index: 0,
        }
    }

    pub fn wrapped_inlines(&self) -> WrappedInlines<'a> {
        let mut inlines = self.inlines();
        WrappedInlines {
            inline: inlines.next(),
            inlines,
            soft_breaks: self.soft_breaks.iter(),
            byte_index: 0,
        }
    }

    pub fn pivot(&self) -> usize {
        self.pivot
    }

    pub fn scale(&self) -> f64 {
        self.scale
    }

    pub fn width(&self, tab_width: usize) -> usize {
        let mut max_summed_width = 0;
        let mut summed_width = 0;
        for wrapped_inline in self.wrapped_inlines() {
            match wrapped_inline {
                WrappedInline::Inline(inline) => summed_width += inline.width(tab_width),
                WrappedInline::SoftBreak => {
                    max_summed_width = max_summed_width.max(summed_width);
                    summed_width = 0;
                }
            }
        }
        max_summed_width = max_summed_width.max(summed_width);
        max_summed_width
    }

    pub fn height(&self) -> usize {
        self.soft_breaks.len() + 1
    }

    pub fn scaled_width(&self, tab_width: usize) -> f64 {
        self.scaled_x(self.width(tab_width))
    }

    pub fn scaled_height(&self) -> f64 {
        self.scale * self.height() as f64
    }

    pub fn scaled_x(&self, x: usize) -> f64 {
        let width_before_pivot = x.min(self.pivot);
        let width_after_pivot = x - width_before_pivot;
        width_before_pivot as f64 + self.scale * width_after_pivot as f64
    }
}

#[derive(Clone, Debug)]
pub struct Inlines<'a> {
    text: &'a str,
    inline_inlays: Iter<'a, (usize, InlineInlay)>,
    byte_index: usize,
}

impl<'a> Iterator for Inlines<'a> {
    type Item = Inline<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if self
            .inline_inlays
            .as_slice()
            .first()
            .map_or(false, |&(byte_index, _)| byte_index == self.byte_index)
        {
            let (_, inline_inlay) = self.inline_inlays.next().unwrap();
            return Some(match inline_inlay {
                InlineInlay::Text(text) => Inline::Text {
                    is_inlay: true,
                    text,
                },
                InlineInlay::Widget(widget) => Inline::Widget(widget),
            });
        }
        if self.text.is_empty() {
            return None;
        }
        let mut byte_count = self.text.len();
        if let Some(&(byte_index, _)) = self.inline_inlays.as_slice().first() {
            byte_count = byte_count.min(byte_index - self.byte_index);
        }
        let (text, remaining_text) = self.text.split_at(byte_count);
        self.text = remaining_text;
        self.byte_index += text.len();
        Some(Inline::Text {
            is_inlay: false,
            text,
        })
    }
}

#[derive(Clone, Copy, Debug)]
pub enum Inline<'a> {
    Text { is_inlay: bool, text: &'a str },
    Widget(&'a InlineWidget),
}

impl<'a> Inline<'a> {
    pub fn width(&self, tab_width: usize) -> usize {
        use crate::str::StrExt;

        match self {
            Self::Text { text, .. } => text.width(tab_width),
            Self::Widget(widget) => widget.width,
        }
    }
}

#[derive(Clone, Debug)]
pub struct WrappedInlines<'a> {
    inline: Option<Inline<'a>>,
    inlines: Inlines<'a>,
    soft_breaks: Iter<'a, usize>,
    byte_index: usize,
}

impl<'a> Iterator for WrappedInlines<'a> {
    type Item = WrappedInline<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if self
            .soft_breaks
            .as_slice()
            .first()
            .map_or(false, |&byte_index| byte_index == self.byte_index)
        {
            self.soft_breaks.next().unwrap();
            return Some(WrappedInline::SoftBreak);
        }
        Some(WrappedInline::Inline(match self.inline.take()? {
            Inline::Text { is_inlay, text } => {
                let mut byte_count = text.len();
                if let Some(&byte_index) = self.soft_breaks.as_slice().first() {
                    byte_count = byte_count.min(byte_index - self.byte_index);
                }
                let text = if byte_count < text.len() {
                    let (text, remaining_text) = text.split_at(byte_count);
                    self.inline = Some(Inline::Text {
                        is_inlay,
                        text: remaining_text,
                    });
                    text
                } else {
                    self.inline = self.inlines.next();
                    text
                };
                self.byte_index += text.len();
                Inline::Text { is_inlay, text }
            }
            inline @ Inline::Widget(_) => {
                self.inline = self.inlines.next();
                inline
            }
        }))
    }
}

#[derive(Clone, Copy, Debug)]
pub enum WrappedInline<'a> {
    Inline(Inline<'a>),
    SoftBreak,
}

#[derive(Clone, Debug)]
pub struct Blocks<'a> {
    lines: Lines<'a>,
    block_inlays: Iter<'a, (usize, BlockInlay)>,
    line_index: usize,
}

impl<'a> Iterator for Blocks<'a> {
    type Item = Block<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if self
            .block_inlays
            .as_slice()
            .first()
            .map_or(false, |&(line_index, _)| line_index == self.line_index)
        {
            let (_, block_inlays) = self.block_inlays.next().unwrap();
            return Some(match block_inlays {
                BlockInlay::Line(line) => Block::Line {
                    is_inlay: true,
                    line: line.as_line(),
                },
                BlockInlay::Widget(widget) => Block::Widget(widget),
            });
        }
        let line = self.lines.next()?;
        self.line_index += 1;
        Some(Block::Line {
            is_inlay: false,
            line,
        })
    }
}

#[derive(Clone, Copy, Debug)]
pub enum Block<'a> {
    Line { is_inlay: bool, line: Line<'a> },
    Widget(&'a BlockWidget),
}

impl<'a> Block<'a> {
    pub fn scaled_width(&self, tab_width: usize) -> f64 {
        match self {
            Self::Line { line, .. } => line.scaled_width(tab_width),
            Self::Widget(widget) => widget.scaled_size.width,
        }
    }

    pub fn scaled_height(&self) -> f64 {
        match self {
            Self::Line { line, .. } => line.scaled_height(),
            Self::Widget(widget) => widget.scaled_size.height,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct LayoutEvent<'a> {
    pub scaled_rect: Rect,
    pub kind: LayoutEventKind<'a>,
}

#[derive(Clone, Copy, Debug)]
pub enum LayoutEventKind<'a> {
    Line { is_inlay: bool, line: Line<'a> },
    Grapheme { is_inlay: bool, text: &'a str },
    SoftBreak,
    Break,
    Widget { id: usize },
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct SessionId(usize);

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum InlineInlay {
    Text(String),
    Widget(InlineWidget),
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct InlineWidget {
    pub id: usize,
    pub width: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub enum BlockInlay {
    Line(LineInlay),
    Widget(BlockWidget),
}

#[derive(Clone, Debug, PartialEq)]
pub struct LineInlay {
    text: String,
    inline_inlays: Vec<(usize, InlineInlay)>,
    soft_breaks: Vec<usize>,
    pivot: usize,
    scale: f64,
}

impl LineInlay {
    pub fn new(text: String) -> Self {
        Self {
            text,
            inline_inlays: Vec::new(),
            soft_breaks: Vec::new(),
            pivot: 0,
            scale: 1.0,
        }
    }

    pub fn as_line(&self) -> Line<'_> {
        Line {
            text: &self.text,
            inline_inlays: &self.inline_inlays,
            soft_breaks: &self.soft_breaks,
            pivot: self.pivot,
            scale: self.scale,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BlockWidget {
    pub id: usize,
    pub scaled_size: Size,
}

#[derive(Clone, Debug)]
struct Session {
    document_id: DocumentId,
    soft_breaks: Vec<Vec<usize>>,
    pivot: Vec<usize>,
    scale: Vec<f64>,
    summed_scaled_heights: RefCell<Vec<f64>>,
    folding_lines: HashSet<usize>,
    unfolding_lines: HashSet<usize>,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct DocumentId(usize);

#[derive(Clone, Debug)]
struct Document {
    text: Vec<String>,
    inline_inlays: Vec<Vec<(usize, InlineInlay)>>,
    block_inlays: Vec<(usize, BlockInlay)>,
}

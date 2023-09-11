use {
    crate::{
        char::CharExt,
        history::Edit,
        history::EditKind,
        inlays::{BlockInlay, InlineInlay},
        iter::IteratorExt,
        line::Wrapped,
        move_ops,
        selection::{Affinity, Cursor, SelectionSet},
        str::StrExt,
        text::{Change, Drift, Length, Position, Text},
        token::TokenKind,
        widgets::BlockWidget,
        wrap,
        wrap::WrapData,
        History, Line, Selection, Settings, Token, Tokenizer,
    },
    std::{
        cell::RefCell,
        cmp,
        collections::{HashMap, HashSet},
        fmt::Write,
        iter, mem, ops,
        rc::Rc,
        slice::Iter,
        sync::{
            atomic,
            atomic::AtomicUsize,
            mpsc,
            mpsc::{Receiver, Sender},
        },
    },
};

#[derive(Debug)]
pub struct Session {
    id: SessionId,
    settings: Rc<Settings>,
    document: Rc<RefCell<Document>>,
    wrap_column: Option<usize>,
    y: Vec<f64>,
    column_count: Vec<Option<usize>>,
    fold_column: Vec<usize>,
    scale: Vec<f64>,
    wrap_data: Vec<Option<WrapData>>,
    folding_lines: HashSet<usize>,
    folded_lines: HashSet<usize>,
    unfolding_lines: HashSet<usize>,
    selections: SelectionSet,
    pending_selection_index: Option<usize>,
    edit_receiver: Receiver<(Option<SelectionSet>, Vec<Edit>)>,
}

impl Session {
    pub fn new(document: Rc<RefCell<Document>>) -> Self {
        static ID: AtomicUsize = AtomicUsize::new(0);

        let (edit_sender, edit_receiver) = mpsc::channel();
        let line_count = document.borrow().history.as_text().as_lines().len();
        let mut session = Self {
            id: SessionId(ID.fetch_add(1, atomic::Ordering::AcqRel)),
            settings: Rc::new(Settings::default()),
            document,
            wrap_column: None,
            y: Vec::new(),
            column_count: (0..line_count).map(|_| None).collect(),
            fold_column: (0..line_count).map(|_| 0).collect(),
            scale: (0..line_count).map(|_| 1.0).collect(),
            wrap_data: (0..line_count).map(|_| None).collect(),
            folding_lines: HashSet::new(),
            folded_lines: HashSet::new(),
            unfolding_lines: HashSet::new(),
            selections: SelectionSet::new(),
            pending_selection_index: None,
            edit_receiver,
        };
        for line in 0..line_count {
            session.update_wrap_data(line);
        }
        session.update_y();
        session
            .document
            .borrow_mut()
            .edit_senders
            .insert(session.id, edit_sender);
        session
    }

    pub fn id(&self) -> SessionId {
        self.id
    }

    pub fn width(&self) -> f64 {
        self.lines(0, self.document.borrow().history.as_text().as_lines().len(), |lines| {
            let mut width: f64 = 0.0;
            for line in lines {
                width = width.max(line.width());
            }
            width
        })
    }

    pub fn height(&self) -> f64 {
        let index = self.document.borrow().history.as_text().as_lines().len() - 1;
        let mut y = self.line(index, |line| line.y() + line.height());
        self.blocks(index, index, |blocks| {
            for block in blocks {
                match block {
                    Block::Line {
                        is_inlay: true,
                        line,
                    } => y += line.height(),
                    Block::Widget(widget) => y += widget.height,
                    _ => unreachable!(),
                }
            }
        });
        y
    }

    pub fn settings(&self) -> &Rc<Settings> {
        &self.settings
    }

    pub fn document(&self) -> &Rc<RefCell<Document>> {
        &self.document
    }

    pub fn wrap_column(&self) -> Option<usize> {
        self.wrap_column
    }

    pub fn find_first_line_ending_after_y(&self, y: f64) -> usize {
        match self.y[..self.y.len() - 1]
            .binary_search_by(|current_y| current_y.partial_cmp(&y).unwrap())
        {
            Ok(line) => line,
            Err(line) => line.saturating_sub(1),
        }
    }

    pub fn find_first_line_starting_after_y(&self, y: f64) -> usize {
        match self.y[..self.y.len() - 1]
            .binary_search_by(|current_y| current_y.partial_cmp(&y).unwrap())
        {
            Ok(line) => line + 1,
            Err(line) => line,
        }
    }

    pub fn line<T>(&self, line: usize, f: impl FnOnce(Line<'_>) -> T) -> T {
        let document = self.document.borrow();
        f(Line {
            y: self.y.get(line).copied(),
            column_count: self.column_count[line],
            fold_column: self.fold_column[line],
            scale: self.scale[line],
            text: &document.history.as_text().as_lines()[line],
            tokens: &document.tokens[line],
            inline_inlays: &document.inline_inlays[line],
            wrap_data: self.wrap_data[line].as_ref(),
        })
    }

    pub fn lines<T>(
        &self,
        start_line: usize,
        end_line: usize,
        f: impl FnOnce(Lines<'_>) -> T,
    ) -> T {
        let document = self.document.borrow();
        f(Lines {
            y: self.y[start_line.min(self.y.len())..end_line.min(self.y.len())].iter(),
            column_count: self.column_count[start_line..end_line].iter(),
            fold_column: self.fold_column[start_line..end_line].iter(),
            scale: self.scale[start_line..end_line].iter(),
            text: document.history.as_text().as_lines()[start_line..end_line].iter(),
            tokens: document.tokens[start_line..end_line].iter(),
            inline_inlays: document.inline_inlays[start_line..end_line].iter(),
            wrap_data: self.wrap_data[start_line..end_line].iter(),
        })
    }

    pub fn blocks<T>(
        &self,
        start_line: usize,
        end_line: usize,
        f: impl FnOnce(Blocks<'_>) -> T,
    ) -> T {
        let document = self.document.borrow();
        let mut block_inlays = document.block_inlays.iter();
        while block_inlays
            .as_slice()
            .first()
            .map_or(false, |&(position, _)| position < start_line)
        {
            block_inlays.next();
        }
        self.lines(start_line, end_line, |lines| {
            f(Blocks {
                lines,
                block_inlays,
                position: start_line,
            })
        })
    }

    pub fn selections(&self) -> &[Selection] {
        &self.selections
    }

    pub fn set_wrap_column(&mut self, wrap_column: Option<usize>) {
        if self.wrap_column == wrap_column {
            return;
        }
        self.wrap_column = wrap_column;
        let line_count = self.document.borrow().history.as_text().as_lines().len();
        for line in 0..line_count {
            self.update_wrap_data(line);
        }
        self.update_y();
    }

    pub fn fold(&mut self) {
        let document = self.document.borrow();
        let lines = document.history.as_text().as_lines();
        for line in 0..lines.len() {
            let indent_level = lines[line]
                .indentation()
                .unwrap_or("")
                .column_count(self.settings.tab_column_count)
                / self.settings.indent_column_count;
            if indent_level >= self.settings.fold_level && !self.folded_lines.contains(&line) {
                self.fold_column[line] =
                    self.settings.fold_level * self.settings.indent_column_count;
                self.unfolding_lines.remove(&line);
                self.folding_lines.insert(line);
            }
        }
    }

    pub fn unfold(&mut self) {
        for line in self.folding_lines.drain() {
            self.unfolding_lines.insert(line);
        }
        for line in self.folded_lines.drain() {
            self.unfolding_lines.insert(line);
        }
    }

    pub fn update_folds(&mut self) -> bool {
        if self.folding_lines.is_empty() && self.unfolding_lines.is_empty() {
            return false;
        }
        let mut new_folding_lines = HashSet::new();
        for &line in &self.folding_lines {
            self.scale[line] *= 0.9;
            if self.scale[line] < 0.1 + 0.001 {
                self.scale[line] = 0.1;
                self.folded_lines.insert(line);
            } else {
                new_folding_lines.insert(line);
            }
            self.y.truncate(line + 1);
        }
        self.folding_lines = new_folding_lines;
        let mut new_unfolding_lines = HashSet::new();
        for &line in &self.unfolding_lines {
            self.scale[line] = 1.0 - 0.9 * (1.0 - self.scale[line]);
            if self.scale[line] > 1.0 - 0.001 {
                self.scale[line] = 1.0;
            } else {
                new_unfolding_lines.insert(line);
            }
            self.y.truncate(line + 1);
        }
        self.unfolding_lines = new_unfolding_lines;
        self.update_y();
        true
    }

    pub fn set_cursor(&mut self, position: Position, affinity: Affinity) {
        self.selections.set_selection(Selection::from(Cursor {
            position,
            affinity,
            preferred_column: None,
        }));
        self.pending_selection_index = Some(0);
        self.document.borrow_mut().force_new_edit_group();
    }

    pub fn push_cursor(&mut self, position: Position, affinity: Affinity) {
        self.pending_selection_index =
            Some(self.selections.push_selection(Selection::from(Cursor {
                position,
                affinity,
                preferred_column: None,
            })));
        self.document.borrow_mut().force_new_edit_group();
    }

    pub fn move_to(&mut self, position: Position, affinity: Affinity) {
        self.pending_selection_index = Some(self.selections.update_selection(
            self.pending_selection_index.unwrap(),
            |selection| {
                selection.update_cursor(|_| Cursor {
                    position,
                    affinity,
                    preferred_column: None,
                })
            },
        ));
        self.document.borrow_mut().force_new_edit_group();
    }

    pub fn move_left(&mut self, reset_anchor: bool) {
        self.modify_selections(reset_anchor, |session, selection| {
            selection.update_cursor(|cursor| {
                move_ops::move_left(session.document.borrow().history.as_text().as_lines(), cursor)
            })
        });
    }

    pub fn move_right(&mut self, reset_anchor: bool) {
        self.modify_selections(reset_anchor, |session, selection| {
            selection.update_cursor(|cursor| {
                move_ops::move_right(session.document.borrow().history.as_text().as_lines(), cursor)
            })
        });
    }

    pub fn move_up(&mut self, reset_anchor: bool) {
        self.modify_selections(reset_anchor, |session, selection| {
            selection.update_cursor(|cursor| move_ops::move_up(session, cursor))
        });
    }

    pub fn move_down(&mut self, reset_anchor: bool) {
        self.modify_selections(reset_anchor, |session, selection| {
            selection.update_cursor(|cursor| move_ops::move_down(session, cursor))
        });
    }

    pub fn insert(&mut self, text: Text) {
        self.document.borrow_mut().edit(
            self.id,
            if text.as_lines().len() == 1 && &text.as_lines()[0] == " " {
                EditKind::Space
            } else {
                EditKind::Insert
            },
            &self.selections,
            self.settings.use_soft_tabs,
            self.settings.tab_column_count,
            self.settings.indent_column_count,
            |_, _, _| (Length::zero(), false, Some(text.clone()), None),
        );
    }

    pub fn enter(&mut self) {
        self.document.borrow_mut().edit(
            self.id,
            EditKind::Other,
            &self.selections,
            self.settings.use_soft_tabs,
            self.settings.tab_column_count,
            self.settings.indent_column_count,
            |line, index, _| {
                (
                    if line[..index].chars().all(|char| char.is_whitespace()) {
                        Length {
                            line_count: 0,
                            byte_count: index,
                        }
                    } else {
                        Length::zero()
                    },
                    false,
                    Some(Text::newline()),
                    if line[..index]
                        .chars()
                        .rev()
                        .find_map(|char| {
                            if char.is_opening_delimiter() {
                                return Some(true);
                            }
                            if char.is_closing_delimiter() {
                                return Some(false);
                            }
                            None
                        })
                        .unwrap_or(false)
                        && line[index..]
                            .chars()
                            .find_map(|char| {
                                if char.is_closing_delimiter() {
                                    return Some(true);
                                }
                                if !char.is_whitespace() {
                                    return Some(false);
                                }
                                None
                            })
                            .unwrap_or(false)
                    {
                        Some(Text::newline())
                    } else {
                        None
                    },
                )
            },
        );
    }

    pub fn indent(&mut self) {
        self.document.borrow_mut().edit_lines(
            self.id,
            EditKind::Indent,
            &self.selections,
            |line| {
                reindent(
                    line,
                    self.settings.use_soft_tabs,
                    self.settings.tab_column_count,
                    |indentation_column_count| {
                        (indentation_column_count + self.settings.indent_column_count)
                            / self.settings.indent_column_count
                            * self.settings.indent_column_count
                    },
                )
            },
        );
    }

    pub fn outdent(&mut self) {
        self.document.borrow_mut().edit_lines(
            self.id,
            EditKind::Outdent,
            &self.selections,
            |line| {
                reindent(
                    line,
                    self.settings.use_soft_tabs,
                    self.settings.tab_column_count,
                    |indentation_column_count| {
                        indentation_column_count.saturating_sub(1)
                            / self.settings.indent_column_count
                            * self.settings.indent_column_count
                    },
                )
            },
        );
    }

    pub fn delete(&mut self) {
        self.document.borrow_mut().edit(
            self.id,
            EditKind::Delete,
            &self.selections,
            self.settings.use_soft_tabs,
            self.settings.tab_column_count,
            self.settings.indent_column_count,
            |_, _, is_empty| (Length::zero(), is_empty, None, None),
        );
    }

    pub fn backspace(&mut self) {
        self.document.borrow_mut().edit(
            self.id,
            EditKind::Delete,
            &self.selections,
            self.settings.use_soft_tabs,
            self.settings.tab_column_count,
            self.settings.indent_column_count,
            |line, index, is_empty| {
                (
                    if is_empty {
                        if index == 0 {
                            Length {
                                line_count: 1,
                                byte_count: 0,
                            }
                        } else {
                            Length {
                                line_count: 0,
                                byte_count: line[..index].graphemes().next_back().unwrap().len(),
                            }
                        }
                    } else {
                        Length::zero()
                    },
                    false,
                    None,
                    None,
                )
            },
        );
    }

    pub fn copy(&self) -> String {
        let mut string = String::new();
        for range in self
            .selections
            .iter()
            .copied()
            .merge(
                |selection_0, selection_1| match selection_0.merge_with(selection_1) {
                    Some(selection) => Ok(selection),
                    None => Err((selection_0, selection_1)),
                },
            )
            .map(|selection| selection.range())
        {
            write!(
                &mut string,
                "{}",
                self.document
                    .borrow()
                    .text()
                    .slice(range.start(), range.extent())
            )
            .unwrap();
        }
        string
    }

    pub fn undo(&mut self) -> bool {
        self.document.borrow_mut().undo(self.id, &self.selections)
    }

    pub fn redo(&mut self) -> bool {
        self.document.borrow_mut().redo(self.id, &self.selections)
    }

    fn update_y(&mut self) {
        let start = self.y.len();
        let end = self.document.borrow().history.as_text().as_lines().len();
        if start == end + 1 {
            return;
        }
        let mut y = if start == 0 {
            0.0
        } else {
            self.line(start - 1, |line| line.y() + line.height())
        };
        let mut ys = mem::take(&mut self.y);
        self.blocks(start, end, |blocks| {
            for block in blocks {
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
        });
        ys.push(y);
        self.y = ys;
    }

    pub fn handle_changes(&mut self) {
        while let Ok((selections, edits)) = self.edit_receiver.try_recv() {
            self.apply_edits(selections, &edits);
        }
    }

    fn update_column_count(&mut self, index: usize) {
        let mut column_count = 0;
        let mut column = 0;
        self.line(index, |line| {
            for wrapped in line.wrappeds() {
                match wrapped {
                    Wrapped::Text { text, .. } => {
                        column += text.column_count(self.settings.tab_column_count);
                    }
                    Wrapped::Widget(widget) => {
                        column += widget.column_count;
                    }
                    Wrapped::Wrap => {
                        column_count = column_count.max(column);
                        column = line.wrap_indent_column_count();
                    }
                }
            }
        });
        self.column_count[index] = Some(column_count.max(column));
    }

    fn update_wrap_data(&mut self, line: usize) {
        let wrap_data = match self.wrap_column {
            Some(wrap_column) => self.line(line, |line| {
                wrap::compute_wrap_data(line, wrap_column, self.settings.tab_column_count)
            }),
            None => WrapData::default(),
        };
        self.wrap_data[line] = Some(wrap_data);
        self.y.truncate(line + 1);
        self.update_column_count(line);
    }

    fn modify_selections(
        &mut self,
        reset_anchor: bool,
        mut f: impl FnMut(&Session, Selection) -> Selection,
    ) {
        // TODO: This should not be needed!!!
        let mut selections = mem::take(&mut self.selections);
        self.pending_selection_index =
            selections.update_all_selections(self.pending_selection_index, |selection| {
                let mut selection = f(&self, selection);
                if reset_anchor {
                    selection = selection.reset_anchor();
                }
                selection
            });
        self.selections = selections;
        self.document.borrow_mut().force_new_edit_group();
    }

    fn apply_edits(&mut self, selections: Option<SelectionSet>, edits: &[Edit]) {
        for edit in edits {
            match edit.change {
                Change::Insert(point, ref text) => {
                    self.column_count[point.line_index] = None;
                    self.wrap_data[point.line_index] = None;
                    let line_count = text.length().line_count;
                    if line_count > 0 {
                        let line = point.line_index + 1;
                        self.y.truncate(line);
                        self.column_count
                            .splice(line..line, (0..line_count).map(|_| None));
                        self.fold_column
                            .splice(line..line, (0..line_count).map(|_| 0));
                        self.scale.splice(line..line, (0..line_count).map(|_| 1.0));
                        self.wrap_data
                            .splice(line..line, (0..line_count).map(|_| None));
                    }
                }
                Change::Delete(start, length) => {
                    self.column_count[start.line_index] = None;
                    self.wrap_data[start.line_index] = None;
                    let line_count = length.line_count;
                    if line_count > 0 {
                        let start_line = start.line_index + 1;
                        let end_line = start_line + line_count;
                        self.y.truncate(start_line);
                        self.column_count.drain(start_line..end_line);
                        self.fold_column.drain(start_line..end_line);
                        self.scale.drain(start_line..end_line);
                        self.wrap_data.drain(start_line..end_line);
                    }
                }
            }
        }
        let line_count = self.document.borrow().history.as_text().as_lines().len();
        for line in 0..line_count {
            if self.wrap_data[line].is_none() {
                self.update_wrap_data(line);
            }
        }
        if let Some(selections) = selections {
            self.selections = selections;
        } else {
            for edit in edits {
                self.selections.apply_change(&edit.change, edit.drift);
            }
        }
        self.update_y();
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.document.borrow_mut().edit_senders.remove(&self.id);
    }
}

#[derive(Clone, Debug)]
pub struct Lines<'a> {
    pub y: Iter<'a, f64>,
    pub column_count: Iter<'a, Option<usize>>,
    pub fold_column: Iter<'a, usize>,
    pub scale: Iter<'a, f64>,
    pub text: Iter<'a, String>,
    pub tokens: Iter<'a, Vec<Token>>,
    pub inline_inlays: Iter<'a, Vec<(usize, InlineInlay)>>,
    pub wrap_data: Iter<'a, Option<WrapData>>,
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

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct SessionId(usize);

#[derive(Debug)]
pub struct Document {
    history: History,
    tokens: Vec<Vec<Token>>,
    inline_inlays: Vec<Vec<(usize, InlineInlay)>>,
    block_inlays: Vec<(usize, BlockInlay)>,
    tokenizer: Tokenizer,
    edit_senders: HashMap<SessionId, Sender<(Option<SelectionSet>, Vec<Edit>)>>,
}

impl Document {
    pub fn new(text: Text) -> Self {
        let line_count = text.as_lines().len();
        let tokens: Vec<_> = (0..line_count)
            .map(|line| tokenize(&text.as_lines()[line]).collect::<Vec<_>>())
            .collect();
        let mut document = Self {
            history: History::from(text),
            tokens,
            inline_inlays: (0..line_count).map(|_| Vec::new()).collect(),
            block_inlays: Vec::new(),
            tokenizer: Tokenizer::new(line_count),
            edit_senders: HashMap::new(),
        };
        document
            .tokenizer
            .update(&document.history.as_text(), &mut document.tokens);
        document
    }

    pub fn text(&self) -> &Text {
        self.history.as_text()
    }

    fn edit(
        &mut self,
        origin_id: SessionId,
        kind: EditKind,
        selections: &SelectionSet,
        use_soft_tabs: bool,
        tab_column_count: usize,
        indent_column_count: usize,
        mut f: impl FnMut(&String, usize, bool) -> (Length, bool, Option<Text>, Option<Text>),
    ) {
        self.history
            .push_or_extend_edit_group(origin_id, kind, selections);
        let mut edits = Vec::new();
        let mut line_ranges = Vec::new();
        let mut point = Position::zero();
        let mut prev_range_end = Position::zero();
        for range in selections
            .iter()
            .copied()
            .merge(
                |selection_0, selection_1| match selection_0.merge_with(selection_1) {
                    Some(selection) => Ok(selection),
                    None => Err((selection_0, selection_1)),
                },
            )
            .map(|selection| selection.range())
        {
            point += range.start() - prev_range_end;
            if !range.is_empty() {
                let edit = Edit {
                    change: Change::Delete(point, range.extent()),
                    drift: Drift::Before,
                };
                edits.push(edit.clone());
                self.history.edit(edit);
            }
            let (delete_extent_before, delete_after, insert_text_before, insert_text_after) = f(
                &self.history.as_text().as_lines()[point.line_index],
                point.byte_index,
                range.is_empty(),
            );
            if delete_extent_before != Length::zero() {
                if delete_extent_before.line_count == 0 {
                    point.byte_index -= delete_extent_before.byte_count;
                } else {
                    point.line_index -= delete_extent_before.line_count;
                    point.byte_index = self.history.as_text().as_lines()[point.line_index].len()
                        - delete_extent_before.byte_count;
                }
                let edit = Edit {
                    change: Change::Delete(point, delete_extent_before),
                    drift: Drift::Before,
                };
                edits.push(edit.clone());
                self.history.edit(edit);
            }
            if delete_after {
                let delete_extent_after = if let Some(grapheme) = self.history.as_text().as_lines()
                    [point.line_index][point.byte_index..]
                    .graphemes()
                    .next()
                {
                    Some(Length {
                        line_count: 0,
                        byte_count: grapheme.len(),
                    })
                } else if point.line_index < self.history.as_text().as_lines().len() - 1 {
                    Some(Length {
                        line_count: 1,
                        byte_count: 0,
                    })
                } else {
                    None
                };
                if let Some(delete_extent_after) = delete_extent_after {
                    let edit = Edit {
                        change: Change::Delete(point, delete_extent_after),
                        drift: Drift::Before,
                    };
                    edits.push(edit.clone());
                    self.history.edit(edit);
                }
            }
            if let Some(insert_text_before) = insert_text_before {
                let line_count = insert_text_before.as_lines().len();
                if line_count > 1 {
                    line_ranges.push(
                        (if self.history.as_text().as_lines()[point.line_index][..point.byte_index]
                            .chars()
                            .all(|char| char.is_whitespace())
                        {
                            point.line_index
                        } else {
                            point.line_index + 1
                        })..point.line_index + line_count,
                    );
                }
                let extent = insert_text_before.length();
                let edit = Edit {
                    change: Change::Insert(point, insert_text_before),
                    drift: Drift::Before,
                };
                point += extent;
                edits.push(edit.clone());
                self.history.edit(edit);
            }
            if let Some(insert_text_after) = insert_text_after {
                let line_count = insert_text_after.as_lines().len();
                if line_count > 1 {
                    line_ranges.push(
                        (if self.history.as_text().as_lines()[point.line_index][..point.byte_index]
                            .chars()
                            .all(|char| char.is_whitespace())
                        {
                            point.line_index
                        } else {
                            point.line_index + 1
                        })..point.line_index + line_count,
                    );
                }
                let extent = insert_text_after.length();
                let edit = Edit {
                    change: Change::Insert(point, insert_text_after),
                    drift: Drift::After,
                };
                point += extent;
                edits.push(edit.clone());
                self.history.edit(edit);
            }
            prev_range_end = range.end();
        }
        self.autoindent(
            &line_ranges,
            use_soft_tabs,
            tab_column_count,
            indent_column_count,
            &mut edits,
        );
        self.apply_edits(origin_id, None, &edits);
    }

    fn autoindent(
        &mut self,
        line_ranges: &[ops::Range<usize>],
        use_soft_tabs: bool,
        tab_column_count: usize,
        indent_column_count: usize,
        changes: &mut Vec<Edit>,
    ) {
        fn next_line_indentation_column_count(
            line: &str,
            tab_column_count: usize,
            indent_column_count: usize,
        ) -> Option<usize> {
            if let Some(indentation) = line.indentation() {
                let mut indentation_column_count = indentation.column_count(tab_column_count);
                if line
                    .chars()
                    .rev()
                    .find_map(|char| {
                        if char.is_opening_delimiter() {
                            return Some(true);
                        }
                        if char.is_closing_delimiter() {
                            return Some(false);
                        }
                        None
                    })
                    .unwrap_or(false)
                {
                    indentation_column_count += indent_column_count;
                };
                Some(indentation_column_count)
            } else {
                None
            }
        }

        for line_range in line_ranges
            .iter()
            .cloned()
            .merge(|line_range_0, line_range_1| {
                if line_range_0.end >= line_range_1.start {
                    Ok(line_range_0.start..line_range_1.end)
                } else {
                    Err((line_range_0, line_range_1))
                }
            })
        {
            let mut desired_indentation_column_count = self.history.as_text().as_lines()[..line_range.start]
                .iter()
                .rev()
                .find_map(|line| {
                    next_line_indentation_column_count(line, tab_column_count, indent_column_count)
                })
                .unwrap_or(0);
            for line in line_range {
                if self.history.as_text().as_lines()[line]
                    .chars()
                    .find_map(|char| {
                        if char.is_closing_delimiter() {
                            return Some(true);
                        }
                        if !char.is_whitespace() {
                            return Some(false);
                        }
                        None
                    })
                    .unwrap_or(false)
                {
                    desired_indentation_column_count -= 4;
                }
                self.edit_lines_internal(line, changes, |line| {
                    reindent(line, use_soft_tabs, tab_column_count, |_| {
                        desired_indentation_column_count
                    })
                });
                if let Some(next_line_indentation_column_count) = next_line_indentation_column_count(
                    &self.history.as_text().as_lines()[line],
                    tab_column_count,
                    indent_column_count,
                ) {
                    desired_indentation_column_count = next_line_indentation_column_count;
                }
            }
        }
    }

    fn edit_lines(
        &mut self,
        origin_id: SessionId,
        kind: EditKind,
        selections: &SelectionSet,
        mut f: impl FnMut(&str) -> (usize, usize, String),
    ) {
        let mut edits = Vec::new();
        self.history
            .push_or_extend_edit_group(origin_id, kind, selections);
        for line_range in selections
            .iter()
            .copied()
            .map(|selection| selection.line_range())
            .merge(|line_range_0, line_range_1| {
                if line_range_0.end >= line_range_1.start {
                    Ok(line_range_0.start..line_range_1.end)
                } else {
                    Err((line_range_0, line_range_1))
                }
            })
        {
            for line in line_range {
                self.edit_lines_internal(line, &mut edits, &mut f);
            }
        }
        self.apply_edits(origin_id, None, &edits);
    }

    fn edit_lines_internal(
        &mut self,
        line: usize,
        edits: &mut Vec<Edit>,
        mut f: impl FnMut(&str) -> (usize, usize, String),
    ) {
        let (byte, delete_byte_count, insert_text) = f(&self.history.as_text().as_lines()[line]);
        if delete_byte_count > 0 {
            let edit = Edit {
                change: Change::Delete(
                    Position {
                        line_index: line,
                        byte_index: byte,
                    },
                    Length {
                        line_count: 0,
                        byte_count: delete_byte_count,
                    },
                ),
                drift: Drift::Before,
            };
            edits.push(edit.clone());
            self.history.edit(edit);
        }
        if !insert_text.is_empty() {
            let edit = Edit {
                change: Change::Insert(
                    Position {
                        line_index: line,
                        byte_index: byte,
                    },
                    insert_text.into(),
                ),
                drift: Drift::Before,
            };
            edits.push(edit.clone());
            self.history.edit(edit);
        }
    }

    fn force_new_edit_group(&mut self) {
        self.history.force_new_edit_group()
    }

    fn undo(&mut self, origin_id: SessionId, selections: &SelectionSet) -> bool {
        let mut changes = Vec::new();
        if let Some(selections) = self.history.undo(selections, &mut changes) {
            self.apply_edits(origin_id, Some(selections), &changes);
            true
        } else {
            false
        }
    }

    fn redo(&mut self, origin_id: SessionId, selections: &SelectionSet) -> bool {
        let mut changes = Vec::new();
        if let Some(selections) = self.history.redo(selections, &mut changes) {
            self.apply_edits(origin_id, Some(selections), &changes);
            true
        } else {
            false
        }
    }

    fn apply_edits(
        &mut self,
        origin_id: SessionId,
        selections: Option<SelectionSet>,
        edits: &[Edit],
    ) {
        for edit in edits {
            self.apply_change_to_tokens(&edit.change);
            self.apply_change_to_inline_inlays(&edit.change, edit.drift);
            self.tokenizer.apply_change(&edit.change);
        }
        self.tokenizer.update(self.history.as_text(), &mut self.tokens);
        for (&session_id, edit_sender) in &self.edit_senders {
            if session_id == origin_id {
                edit_sender
                    .send((selections.clone(), edits.to_vec()))
                    .unwrap();
            } else {
                edit_sender
                    .send((
                        None,
                        edits
                            .iter()
                            .cloned()
                            .map(|edit| Edit {
                                change: edit.change,
                                drift: Drift::Before,
                            })
                            .collect(),
                    ))
                    .unwrap();
            }
        }
    }

    fn apply_change_to_tokens(&mut self, change: &Change) {
        match *change {
            Change::Insert(point, ref text) => {
                let mut byte = 0;
                let mut index = self.tokens[point.line_index]
                    .iter()
                    .position(|token| {
                        if byte + token.len > point.byte_index {
                            return true;
                        }
                        byte += token.len;
                        false
                    })
                    .unwrap_or(self.tokens[point.line_index].len());
                if byte != point.byte_index {
                    let token = self.tokens[point.line_index][index];
                    let mid = point.byte_index - byte;
                    self.tokens[point.line_index][index] = Token {
                        len: mid,
                        kind: token.kind,
                    };
                    index += 1;
                    self.tokens[point.line_index].insert(
                        index,
                        Token {
                            len: token.len - mid,
                            kind: token.kind,
                        },
                    );
                }
                if text.length().line_count == 0 {
                    self.tokens[point.line_index]
                        .splice(index..index, tokenize(text.as_lines().first().unwrap()));
                } else {
                    let mut tokens = (0..text.as_lines().len())
                        .map(|line| tokenize(&text.as_lines()[line]).collect::<Vec<_>>())
                        .collect::<Vec<_>>();
                    tokens
                        .first_mut()
                        .unwrap()
                        .splice(..0, self.tokens[point.line_index][..index].iter().copied());
                    tokens
                        .last_mut()
                        .unwrap()
                        .splice(..0, self.tokens[point.line_index][index..].iter().copied());
                    self.tokens
                        .splice(point.line_index..point.line_index + 1, tokens);
                }
            }
            Change::Delete(start, length) => {
                let end = start + length;
                let mut byte = 0;
                let mut start_token = self.tokens[start.line_index]
                    .iter()
                    .position(|token| {
                        if byte + token.len > start.byte_index {
                            return true;
                        }
                        byte += token.len;
                        false
                    })
                    .unwrap_or(self.tokens[start.line_index].len());
                if byte != start.byte_index {
                    let token = self.tokens[start.line_index][start_token];
                    let mid = start.byte_index - byte;
                    self.tokens[start.line_index][start_token] = Token {
                        len: mid,
                        kind: token.kind,
                    };
                    start_token += 1;
                    self.tokens[start.line_index].insert(
                        start_token,
                        Token {
                            len: token.len - mid,
                            kind: token.kind,
                        },
                    );
                }
                let mut byte = 0;
                let mut end_token = self.tokens[end.line_index]
                    .iter()
                    .position(|token| {
                        if byte + token.len > end.byte_index {
                            return true;
                        }
                        byte += token.len;
                        false
                    })
                    .unwrap_or(self.tokens[end.line_index].len());
                if byte != end.byte_index {
                    let token = self.tokens[end.line_index][end_token];
                    let mid = end.byte_index - byte;
                    self.tokens[end.line_index][end_token] = Token {
                        len: mid,
                        kind: token.kind,
                    };
                    end_token += 1;
                    self.tokens[end.line_index].insert(
                        end_token,
                        Token {
                            len: token.len - mid,
                            kind: token.kind,
                        },
                    );
                }
                if length.line_count == 0 {
                    self.tokens[start.line_index].drain(start_token..end_token);
                } else {
                    let mut tokens = self.tokens[start.line_index][..start_token]
                        .iter()
                        .copied()
                        .collect::<Vec<_>>();
                    tokens.extend(self.tokens[end.line_index][end_token..].iter().copied());
                    self.tokens
                        .splice(start.line_index..end.line_index + 1, iter::once(tokens));
                }
            }
        }
    }

    fn apply_change_to_inline_inlays(&mut self, change: &Change, drift: Drift) {
        match *change {
            Change::Insert(point, ref text) => {
                let index = self.inline_inlays[point.line_index]
                    .iter()
                    .position(|(byte, _)| match byte.cmp(&point.byte_index) {
                        cmp::Ordering::Less => false,
                        cmp::Ordering::Equal => match drift {
                            Drift::Before => true,
                            Drift::After => false,
                        },
                        cmp::Ordering::Greater => true,
                    })
                    .unwrap_or(self.inline_inlays[point.line_index].len());
                if text.length().line_count == 0 {
                    for (byte, _) in &mut self.inline_inlays[point.line_index][index..] {
                        *byte += text.length().byte_count;
                    }
                } else {
                    let mut inline_inlays = (0..text.as_lines().len())
                        .map(|_| Vec::new())
                        .collect::<Vec<_>>();
                    inline_inlays
                        .first_mut()
                        .unwrap()
                        .splice(..0, self.inline_inlays[point.line_index].drain(..index));
                    inline_inlays.last_mut().unwrap().splice(
                        ..0,
                        self.inline_inlays[point.line_index].drain(..).map(
                            |(byte, inline_inlay)| (byte + text.length().byte_count, inline_inlay),
                        ),
                    );
                    self.inline_inlays
                        .splice(point.line_index..point.line_index + 1, inline_inlays);
                }
            }
            Change::Delete(start, length) => {
                let end = start + length;
                let start_inlay = self.inline_inlays[start.line_index]
                    .iter()
                    .position(|&(byte, _)| byte >= start.byte_index)
                    .unwrap_or(self.inline_inlays[start.line_index].len());
                let end_inlay = self.inline_inlays[end.line_index]
                    .iter()
                    .position(|&(byte, _)| byte >= end.byte_index)
                    .unwrap_or(self.inline_inlays[end.line_index].len());
                if length.line_count == 0 {
                    self.inline_inlays[start.line_index].drain(start_inlay..end_inlay);
                    for (byte, _) in &mut self.inline_inlays[start.line_index][start_inlay..] {
                        *byte = start.byte_index + (*byte - end.byte_index.min(*byte));
                    }
                } else {
                    let mut inline_inlays = self.inline_inlays[start.line_index]
                        .drain(..start_inlay)
                        .collect::<Vec<_>>();
                    inline_inlays.extend(
                        self.inline_inlays[end.line_index].drain(end_inlay..).map(
                            |(byte, inline_inlay)| {
                                (
                                    start.byte_index + byte - end.byte_index.min(byte),
                                    inline_inlay,
                                )
                            },
                        ),
                    );
                    self.inline_inlays.splice(
                        start.line_index..end.line_index + 1,
                        iter::once(inline_inlays),
                    );
                }
            }
        }
    }
}

fn tokenize(text: &str) -> impl Iterator<Item = Token> + '_ {
    text.split_whitespace_boundaries().map(|string| Token {
        len: string.len(),
        kind: if string.chars().next().unwrap().is_whitespace() {
            TokenKind::Whitespace
        } else {
            TokenKind::Unknown
        },
    })
}

fn reindent(
    string: &str,
    use_soft_tabs: bool,
    tab_column_count: usize,
    f: impl FnOnce(usize) -> usize,
) -> (usize, usize, String) {
    let indentation = string.indentation().unwrap_or("");
    let indentation_column_count = indentation.column_count(tab_column_count);
    let new_indentation_column_count = f(indentation_column_count);
    let new_indentation = new_indentation(
        new_indentation_column_count,
        use_soft_tabs,
        tab_column_count,
    );
    let len = indentation.longest_common_prefix(&new_indentation).len();
    (
        len,
        indentation.len() - len.min(indentation.len()),
        new_indentation[len..].to_owned(),
    )
}

fn new_indentation(column_count: usize, use_soft_tabs: bool, tab_column_count: usize) -> String {
    let tab_count;
    let space_count;
    if use_soft_tabs {
        tab_count = 0;
        space_count = column_count;
    } else {
        tab_count = column_count / tab_column_count;
        space_count = column_count % tab_column_count;
    }
    let tabs = iter::repeat("\t").take(tab_count);
    let spaces = iter::repeat(" ").take(space_count);
    tabs.chain(spaces).collect()
}

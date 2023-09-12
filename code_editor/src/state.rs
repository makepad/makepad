use {
    crate::{
        char::CharExt,
        history::EditKind,
        inlays::{BlockInlay, InlineInlay},
        iter::IteratorExt,
        line::Wrapped,
        move_ops,
        selection::{Affinity, Cursor, SelectionSet},
        str::StrExt,
        text::{Change, Drift, Edit, Length, Position, Text},
        token::TokenKind,
        widgets::BlockWidget,
        wrap,
        wrap::WrapData,
        History, Line, Selection, Settings, Token, Tokenizer,
    },
    std::{
        cell::{Ref, RefCell},
        cmp,
        collections::{HashMap, HashSet},
        fmt::Write,
        iter, mem, ops,
        ops::Range,
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
    document: Document,
    wrap_column: Option<usize>,
    layout: SessionLayout,
    folding_lines: HashSet<usize>,
    folded_lines: HashSet<usize>,
    unfolding_lines: HashSet<usize>,
    selections: SelectionSet,
    pending_selection_index: Option<usize>,
    delimiter_stack: Vec<char>,
    edit_receiver: Receiver<(Option<SelectionSet>, Vec<Edit>)>,
}

impl Session {
    pub fn new(document: Document) -> Self {
        static ID: AtomicUsize = AtomicUsize::new(0);

        let (edit_sender, edit_receiver) = mpsc::channel();
        let line_count = document.text().as_lines().len();
        let mut session = Self {
            id: SessionId(ID.fetch_add(1, atomic::Ordering::AcqRel)),
            settings: Rc::new(Settings::default()),
            document,
            wrap_column: None,
            layout: SessionLayout {
                y: Vec::new(),
                column_count: (0..line_count).map(|_| None).collect(),
                fold_column: (0..line_count).map(|_| 0).collect(),
                scale: (0..line_count).map(|_| 1.0).collect(),
                wrap_data: (0..line_count).map(|_| None).collect(),
            },
            folding_lines: HashSet::new(),
            folded_lines: HashSet::new(),
            unfolding_lines: HashSet::new(),
            selections: SelectionSet::new(),
            pending_selection_index: None,
            delimiter_stack: Vec::new(),
            edit_receiver,
        };
        for line in 0..line_count {
            session.update_wrap_data(line);
        }
        session.update_y();
        session
            .document
            .0
            .edit_senders
            .borrow_mut()
            .insert(session.id, edit_sender);
        session
    }

    pub fn id(&self) -> SessionId {
        self.id
    }

    pub fn width(&self) -> f64 {
        self.lines(0, self.document.text().as_lines().len(), |lines| {
            let mut width: f64 = 0.0;
            for line in lines {
                width = width.max(line.width());
            }
            width
        })
    }

    pub fn height(&self) -> f64 {
        let index = self.document.text().as_lines().len() - 1;
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

    pub fn document(&self) -> &Document {
        &self.document
    }

    pub fn wrap_column(&self) -> Option<usize> {
        self.wrap_column
    }

    pub fn find_first_line_ending_after_y(&self, y: f64) -> usize {
        match self.layout.y[..self.layout.y.len() - 1]
            .binary_search_by(|current_y| current_y.partial_cmp(&y).unwrap())
        {
            Ok(line) => line,
            Err(line) => line.saturating_sub(1),
        }
    }

    pub fn find_first_line_starting_after_y(&self, y: f64) -> usize {
        match self.layout.y[..self.layout.y.len() - 1]
            .binary_search_by(|current_y| current_y.partial_cmp(&y).unwrap())
        {
            Ok(line) => line + 1,
            Err(line) => line,
        }
    }

    pub fn line<T>(&self, line: usize, f: impl FnOnce(Line<'_>) -> T) -> T {
        let document = self.document();
        f(Line {
            y: self.layout.y.get(line).copied(),
            column_count: self.layout.column_count[line],
            fold_column: self.layout.fold_column[line],
            scale: self.layout.scale[line],
            text: &document.0.history.borrow().as_text().as_lines()[line],
            tokens: &document.0.layout.borrow().tokens[line],
            inline_inlays: &document.0.layout.borrow().inline_inlays[line],
            wrap_data: self.layout.wrap_data[line].as_ref(),
        })
    }

    pub fn lines<T>(
        &self,
        start_line: usize,
        end_line: usize,
        f: impl FnOnce(Lines<'_>) -> T,
    ) -> T {
        let document = self.document();
        f(Lines {
            y: self.layout.y[start_line.min(self.layout.y.len())..end_line.min(self.layout.y.len())].iter(),
            column_count: self.layout.column_count[start_line..end_line].iter(),
            fold_column: self.layout.fold_column[start_line..end_line].iter(),
            scale: self.layout.scale[start_line..end_line].iter(),
            text: document.0.history.borrow().as_text().as_lines()[start_line..end_line].iter(),
            tokens: document.0.layout.borrow().tokens[start_line..end_line].iter(),
            inline_inlays: document.0.layout.borrow().inline_inlays[start_line..end_line].iter(),
            wrap_data: self.layout.wrap_data[start_line..end_line].iter(),
        })
    }

    pub fn blocks<T>(
        &self,
        start_line: usize,
        end_line: usize,
        f: impl FnOnce(Blocks<'_>) -> T,
    ) -> T {
        let document = self.document();
        let layout = document.0.layout.borrow();
        let mut block_inlays = layout.block_inlays.iter();
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
        let line_count = self.document.text().as_lines().len();
        for line in 0..line_count {
            self.update_wrap_data(line);
        }
        self.update_y();
    }

    pub fn fold(&mut self) {
        let text = self.document.text();
        let lines = text.as_lines();
        for line in 0..lines.len() {
            let indent_level = lines[line]
                .leading_whitespace()
                .unwrap_or("")
                .column_count(self.settings.tab_column_count)
                / self.settings.indent_column_count;
            if indent_level >= self.settings.fold_level && !self.folded_lines.contains(&line) {
                self.layout.fold_column[line] =
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
            self.layout.scale[line] *= 0.9;
            if self.layout.scale[line] < 0.1 + 0.001 {
                self.layout.scale[line] = 0.1;
                self.folded_lines.insert(line);
            } else {
                new_folding_lines.insert(line);
            }
            self.layout.y.truncate(line + 1);
        }
        self.folding_lines = new_folding_lines;
        let mut new_unfolding_lines = HashSet::new();
        for &line in &self.unfolding_lines {
            self.layout.scale[line] = 1.0 - 0.9 * (1.0 - self.layout.scale[line]);
            if self.layout.scale[line] > 1.0 - 0.001 {
                self.layout.scale[line] = 1.0;
            } else {
                new_unfolding_lines.insert(line);
            }
            self.layout.y.truncate(line + 1);
        }
        self.unfolding_lines = new_unfolding_lines;
        self.update_y();
        true
    }

    pub fn set_cursor(&mut self, position: Position, affinity: Affinity) {
        self.selections.set_selection(Selection::from(Cursor {
            position,
            affinity,
            preferred_column_index: None,
        }));
        self.pending_selection_index = Some(0);
        self.delimiter_stack.clear();
        self.document.force_new_group();
    }

    pub fn push_cursor(&mut self, position: Position, affinity: Affinity) {
        self.pending_selection_index =
            Some(self.selections.push_selection(Selection::from(Cursor {
                position,
                affinity,
                preferred_column_index: None,
            })));
        self.delimiter_stack.clear();
        self.document.force_new_group();
    }

    pub fn move_to(&mut self, position: Position, affinity: Affinity) {
        self.pending_selection_index = Some(self.selections.update_selection(
            self.pending_selection_index.unwrap(),
            |selection| {
                selection.update_cursor(|_| Cursor {
                    position,
                    affinity,
                    preferred_column_index: None,
                })
            },
        ));
        self.delimiter_stack.clear();
        self.document.force_new_group();
    }

    pub fn move_left(&mut self, reset_anchor: bool) {
        self.modify_selections(reset_anchor, |session, selection| {
            selection.update_cursor(|cursor| {
                move_ops::move_left(session.document.text().as_lines(), cursor)
            })
        });
    }

    pub fn move_right(&mut self, reset_anchor: bool) {
        self.modify_selections(reset_anchor, |session, selection| {
            selection.update_cursor(|cursor| {
                move_ops::move_right(session.document.text().as_lines(), cursor)
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
        let mut edit_kind = EditKind::Insert;
        let mut inject_delimiter = None;
        let mut uninject_delimiter = None;
        match text.to_single_char() {
            Some(' ') => {
                edit_kind = EditKind::InsertSpace;
            }
            Some(char) if char.is_opening_delimiter() => {
                let char = char.opposite_delimiter().unwrap();
                inject_delimiter = Some(char);
                self.delimiter_stack.push(char);
            }
            Some(char)
                if self
                    .delimiter_stack
                    .last()
                    .map_or(false, |&last_char| last_char == char) =>
            {
                uninject_delimiter = Some(self.delimiter_stack.pop().unwrap());
            }
            _ => {}
        }
        self.document.edit_selections(
            self.id,
            edit_kind,
            &self.selections,
            &self.settings,
            |mut editor, position, length| {
                editor.apply_edit(Edit {
                    change: Change::Delete(position, length),
                    drift: Drift::Before,
                });
                if let Some(uninject_delimiter) = uninject_delimiter {
                    editor.apply_edit(Edit {
                        change: Change::Delete(
                            position,
                            Length {
                                line_count: 0,
                                byte_count: uninject_delimiter.len_utf8(),
                            },
                        ),
                        drift: Drift::Before,
                    })
                }
                editor.apply_edit(Edit {
                    change: Change::Insert(position, text.clone()),
                    drift: Drift::Before,
                });
                if let Some(inject_delimiter) = inject_delimiter {
                    editor.apply_edit(Edit {
                        change: Change::Insert(
                            position + text.length(),
                            Text::from(inject_delimiter),
                        ),
                        drift: Drift::After,
                    })
                }
            },
        );
    }

    pub fn enter(&mut self) {
        self.document.edit_selections(
            self.id,
            EditKind::Other,
            &self.selections,
            &self.settings,
            |mut editor, position, length| {
                let line = &editor.as_text().as_lines()[position.line_index];
                let delete_whitespace = line.chars().all(|char| char.is_whitespace());
                let inject_newline = line[..position.byte_index]
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
                    && line[position.byte_index..]
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
                        .unwrap_or(false);
                if delete_whitespace {
                    editor.apply_edit(Edit {
                        change: Change::Delete(
                            Position {
                                line_index: position.line_index,
                                byte_index: 0,
                            },
                            Length {
                                line_count: 0,
                                byte_count: position.byte_index,
                            },
                        ),
                        drift: Drift::Before,
                    });
                }
                editor.apply_edit(Edit {
                    change: Change::Delete(position, length),
                    drift: Drift::Before,
                });
                editor.apply_edit(Edit {
                    change: Change::Insert(position, Text::newline()),
                    drift: Drift::Before,
                });
                if inject_newline {
                    editor.apply_edit(Edit {
                        change: Change::Insert(
                            Position {
                                line_index: position.line_index + 1,
                                byte_index: 0,
                            },
                            Text::newline(),
                        ),
                        drift: Drift::After,
                    });
                }
            },
        );
    }

    pub fn indent(&mut self) {
        self.document
            .edit_lines(self.id, EditKind::Indent, &self.selections, |line| {
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
            });
    }

    pub fn outdent(&mut self) {
        self.document
            .edit_lines(self.id, EditKind::Outdent, &self.selections, |line| {
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
            });
    }

    pub fn delete(&mut self) {
        self.document.edit_selections(
            self.id,
            EditKind::Delete,
            &self.selections,
            &self.settings,
            |mut editor, position, length| {
                let mut length = length;
                if length == Length::zero() {
                    let lines = editor.as_text().as_lines();
                    if position.byte_index < lines[position.line_index].len() {
                        length.byte_count += 1;
                    } else if position.line_index < lines.len() {
                        length.line_count += 1;
                        length.byte_count = 0;
                    }
                }
                editor.apply_edit(Edit {
                    change: Change::Delete(position, length),
                    drift: Drift::Before,
                });
            },
        );
    }

    pub fn backspace(&mut self) {
        self.document.edit_selections(
            self.id,
            EditKind::Delete,
            &self.selections,
            &self.settings,
            |mut editor, position, length| {
                let mut position = position;
                let mut length = length;
                if length == Length::zero() {
                    let lines = editor.as_text().as_lines();
                    if position.byte_index > 0 {
                        let byte_count = lines[position.line_index]
                            .graphemes()
                            .next_back()
                            .unwrap()
                            .len();
                        position.byte_index -= byte_count;
                        length.byte_count += byte_count;
                    } else if position.line_index > 0 {
                        position.line_index -= 1;
                        position.byte_index = lines[position.line_index].len();
                        length.line_count += 1;
                    }
                }
                editor.apply_edit(Edit {
                    change: Change::Delete(position, length),
                    drift: Drift::Before,
                });
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
                self.document.text().slice(range.start(), range.extent())
            )
            .unwrap();
        }
        string
    }

    pub fn undo(&mut self) -> bool {
        self.document.undo(self.id, &self.selections)
    }

    pub fn redo(&mut self) -> bool {
        self.document.redo(self.id, &self.selections)
    }

    fn update_y(&mut self) {
        let start = self.layout.y.len();
        let end = self.document.text().as_lines().len();
        if start == end + 1 {
            return;
        }
        let mut y = if start == 0 {
            0.0
        } else {
            self.line(start - 1, |line| line.y() + line.height())
        };
        let mut ys = mem::take(&mut self.layout.y);
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
        self.layout.y = ys;
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
        self.layout.column_count[index] = Some(column_count.max(column));
    }

    fn update_wrap_data(&mut self, line: usize) {
        let wrap_data = match self.wrap_column {
            Some(wrap_column) => self.line(line, |line| {
                wrap::compute_wrap_data(line, wrap_column, self.settings.tab_column_count)
            }),
            None => WrapData::default(),
        };
        self.layout.wrap_data[line] = Some(wrap_data);
        self.layout.y.truncate(line + 1);
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
        self.delimiter_stack.clear();
        self.document.force_new_group();
    }

    fn apply_edits(&mut self, selections: Option<SelectionSet>, edits: &[Edit]) {
        for edit in edits {
            match edit.change {
                Change::Insert(point, ref text) => {
                    self.layout.column_count[point.line_index] = None;
                    self.layout.wrap_data[point.line_index] = None;
                    let line_count = text.length().line_count;
                    if line_count > 0 {
                        let line = point.line_index + 1;
                        self.layout.y.truncate(line);
                        self.layout.column_count
                            .splice(line..line, (0..line_count).map(|_| None));
                        self.layout.fold_column
                            .splice(line..line, (0..line_count).map(|_| 0));
                        self.layout.scale.splice(line..line, (0..line_count).map(|_| 1.0));
                        self.layout.wrap_data
                            .splice(line..line, (0..line_count).map(|_| None));
                    }
                }
                Change::Delete(start, length) => {
                    self.layout.column_count[start.line_index] = None;
                    self.layout.wrap_data[start.line_index] = None;
                    let line_count = length.line_count;
                    if line_count > 0 {
                        let start_line = start.line_index + 1;
                        let end_line = start_line + line_count;
                        self.layout.y.truncate(start_line);
                        self.layout.column_count.drain(start_line..end_line);
                        self.layout.fold_column.drain(start_line..end_line);
                        self.layout.scale.drain(start_line..end_line);
                        self.layout.wrap_data.drain(start_line..end_line);
                    }
                }
            }
        }
        let line_count = self.document.text().as_lines().len();
        for line in 0..line_count {
            if self.layout.wrap_data[line].is_none() {
                self.update_wrap_data(line);
            }
        }
        if let Some(selections) = selections {
            self.selections = selections;
        } else {
            for edit in edits {
                self.selections.apply_change(edit);
            }
        }
        self.update_y();
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.document.0.edit_senders.borrow_mut().remove(&self.id);
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
struct SessionLayout {
    y: Vec<f64>,
    column_count: Vec<Option<usize>>,
    fold_column: Vec<usize>,
    scale: Vec<f64>,
    wrap_data: Vec<Option<WrapData>>,
}

#[derive(Clone, Debug)]
pub struct Document(Rc<DocumentInner>);

#[derive(Debug)]
struct DocumentInner {
    history: RefCell<History>,
    layout: RefCell<DocumentLayout>,
    tokenizer: RefCell<Tokenizer>,
    edit_senders: RefCell<HashMap<SessionId, Sender<(Option<SelectionSet>, Vec<Edit>)>>>,
}

impl Document {
    pub fn new(text: Text) -> Self {
        let line_count = text.as_lines().len();
        let tokens: Vec<_> = (0..line_count)
            .map(|line| tokenize(&text.as_lines()[line]).collect::<Vec<_>>())
            .collect();
        let session = Self(Rc::new(DocumentInner {
            history: RefCell::new(History::from(text)),
            layout: RefCell::new(DocumentLayout {
                tokens,
                inline_inlays: (0..line_count).map(|_| Vec::new()).collect(),
                block_inlays: Vec::new(),
            }),
            tokenizer: RefCell::new(Tokenizer::new(line_count)),
            edit_senders: RefCell::new(HashMap::new()),
        }));
        session
            .0
            .tokenizer
            .borrow_mut()
            .update(&session.0.history.borrow().as_text(), &mut session.0.layout.borrow_mut().tokens);
        session
    }

    pub fn text(&self) -> Ref<'_, Text> {
        Ref::map(self.0.history.borrow(), |history| history.as_text())
    }

    pub fn edit_selections(
        &self,
        session_id: SessionId,
        kind: EditKind,
        selections: &SelectionSet,
        settings: &Settings,
        mut f: impl FnMut(Editor<'_>, Position, Length),
    ) {
        let mut history = self
            .0
            .history
            .borrow_mut();
        history.push_or_extend_group(session_id, kind, selections);
        let mut edits = Vec::new();
        let mut line_ranges = Vec::new();
        let mut prev_start = Position::zero();
        let mut prev_adjusted_start = Position::zero();
        let mut prev_edit_start = 0;
        for &selection in selections {
            let mut adjusted_start = prev_adjusted_start + (selection.start() - prev_start);
            for edit in &edits[prev_edit_start..] {
                adjusted_start = adjusted_start.apply_edit(edit);
            }
            let edit_start = edits.len();
            f(
                Editor {
                    history: &mut *history,
                    edits: &mut edits,
                },
                adjusted_start,
                selection.length(),
            );
            for edit in &edits[edit_start..] {
                match edit.change {
                    Change::Insert(position, ref text) if text.as_lines().len() > 1 => {
                        line_ranges.push(Range {
                            start: if history.as_text().as_lines()[position.line_index]
                                [..position.byte_index]
                                .chars()
                                .all(|char| char.is_whitespace())
                            {
                                position.line_index
                            } else {
                                position.line_index + 1
                            },
                            end: position.line_index + text.as_lines().len(),
                        });
                    }
                    _ => {}
                }
            }
            prev_start = selection.start();
            prev_adjusted_start = adjusted_start;
            prev_edit_start = edit_start;
        }
        drop(history);
        self.autoindent(
            &line_ranges,
            settings.use_soft_tabs,
            settings.tab_column_count,
            settings.indent_column_count,
            &mut edits,
        );
        self.apply_edits(session_id, None, &edits);
    }

    fn autoindent(
        &self,
        line_ranges: &[ops::Range<usize>],
        use_soft_tabs: bool,
        tab_column_count: usize,
        indent_column_count: usize,
        edits: &mut Vec<Edit>,
    ) {
        fn next_line_indentation_column_count(
            line: &str,
            tab_column_count: usize,
            indent_column_count: usize,
        ) -> Option<usize> {
            if let Some(indentation) = line.leading_whitespace() {
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
            let mut desired_indentation_column_count = self.text().as_lines()[..line_range.start]
                .iter()
                .rev()
                .find_map(|line| {
                    next_line_indentation_column_count(line, tab_column_count, indent_column_count)
                })
                .unwrap_or(0);
            for line in line_range {
                if self.text().as_lines()[line]
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
                self.edit_lines_internal(line, edits, |line| {
                    reindent(line, use_soft_tabs, tab_column_count, |_| {
                        desired_indentation_column_count
                    })
                });
                if let Some(next_line_indentation_column_count) = next_line_indentation_column_count(
                    &self.text().as_lines()[line],
                    tab_column_count,
                    indent_column_count,
                ) {
                    desired_indentation_column_count = next_line_indentation_column_count;
                }
            }
        }
    }

    fn edit_lines(
        &self,
        origin_id: SessionId,
        kind: EditKind,
        selections: &SelectionSet,
        mut f: impl FnMut(&str) -> (usize, usize, String),
    ) {
        let mut edits = Vec::new();
        self.0.history.borrow_mut()
            .push_or_extend_group(origin_id, kind, selections);
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
        &self,
        line: usize,
        edits: &mut Vec<Edit>,
        mut f: impl FnMut(&str) -> (usize, usize, String),
    ) {
        let mut history = self.0.history.borrow_mut();
        let (byte, delete_byte_count, insert_text) = f(&history.as_text().as_lines()[line]);
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
            history.apply_edit(edit);
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
            history.apply_edit(edit);
        }
    }

    fn force_new_group(&mut self) {
        self.0.history.borrow_mut().force_new_group()
    }

    fn undo(&mut self, origin_id: SessionId, selections: &SelectionSet) -> bool {
        let mut changes = Vec::new();
        let selections = self.0.history.borrow_mut().undo(selections, &mut changes);
        if let Some(selections) = selections {
            self.apply_edits(origin_id, Some(selections), &changes);
            true
        } else {
            false
        }
    }

    fn redo(&mut self, origin_id: SessionId, selections: &SelectionSet) -> bool {
        let mut changes = Vec::new();
        let selections = self.0.history.borrow_mut().redo(selections, &mut changes);
        if let Some(selections) = selections {
            self.apply_edits(origin_id, Some(selections), &changes);
            true
        } else {
            false
        }
    }

    fn apply_edits(&self, origin_id: SessionId, selections: Option<SelectionSet>, edits: &[Edit]) {
        for edit in edits {
            self.apply_change_to_tokens(&edit.change);
            self.apply_change_to_inline_inlays(&edit.change, edit.drift);
            self.0.tokenizer.borrow_mut().apply_change(&edit.change);
        }
        self.0
            .tokenizer
            .borrow_mut()
            .update(self.0.history.borrow().as_text(), &mut self.0.layout.borrow_mut().tokens);
        for (&session_id, edit_sender) in &*self.0.edit_senders.borrow() {
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

    fn apply_change_to_tokens(&self, change: &Change) {
        let mut layout = self.0.layout.borrow_mut();
        let tokens = &mut layout.tokens;
        match *change {
            Change::Insert(point, ref text) => {
                let mut byte = 0;
                let mut index = tokens[point.line_index]
                    .iter()
                    .position(|token| {
                        if byte + token.len > point.byte_index {
                            return true;
                        }
                        byte += token.len;
                        false
                    })
                    .unwrap_or(tokens[point.line_index].len());
                if byte != point.byte_index {
                    let token = tokens[point.line_index][index];
                    let mid = point.byte_index - byte;
                    tokens[point.line_index][index] = Token {
                        len: mid,
                        kind: token.kind,
                    };
                    index += 1;
                    tokens[point.line_index].insert(
                        index,
                        Token {
                            len: token.len - mid,
                            kind: token.kind,
                        },
                    );
                }
                if text.length().line_count == 0 {
                    tokens[point.line_index]
                        .splice(index..index, tokenize(text.as_lines().first().unwrap()));
                } else {
                    let mut new_tokens = (0..text.as_lines().len())
                        .map(|line| tokenize(&text.as_lines()[line]).collect::<Vec<_>>())
                        .collect::<Vec<_>>();
                    new_tokens
                        .first_mut()
                        .unwrap()
                        .splice(..0, tokens[point.line_index][..index].iter().copied());
                    new_tokens
                        .last_mut()
                        .unwrap()
                        .splice(..0, tokens[point.line_index][index..].iter().copied());
                    tokens
                        .splice(point.line_index..point.line_index + 1, new_tokens);
                }
            }
            Change::Delete(start, length) => {
                let end = start + length;
                let mut byte = 0;
                let mut start_token = tokens[start.line_index]
                    .iter()
                    .position(|token| {
                        if byte + token.len > start.byte_index {
                            return true;
                        }
                        byte += token.len;
                        false
                    })
                    .unwrap_or(tokens[start.line_index].len());
                if byte != start.byte_index {
                    let token = tokens[start.line_index][start_token];
                    let mid = start.byte_index - byte;
                    tokens[start.line_index][start_token] = Token {
                        len: mid,
                        kind: token.kind,
                    };
                    start_token += 1;
                    tokens[start.line_index].insert(
                        start_token,
                        Token {
                            len: token.len - mid,
                            kind: token.kind,
                        },
                    );
                }
                let mut byte = 0;
                let mut end_token = tokens[end.line_index]
                    .iter()
                    .position(|token| {
                        if byte + token.len > end.byte_index {
                            return true;
                        }
                        byte += token.len;
                        false
                    })
                    .unwrap_or(tokens[end.line_index].len());
                if byte != end.byte_index {
                    let token = tokens[end.line_index][end_token];
                    let mid = end.byte_index - byte;
                    tokens[end.line_index][end_token] = Token {
                        len: mid,
                        kind: token.kind,
                    };
                    end_token += 1;
                    tokens[end.line_index].insert(
                        end_token,
                        Token {
                            len: token.len - mid,
                            kind: token.kind,
                        },
                    );
                }
                if length.line_count == 0 {
                    tokens[start.line_index].drain(start_token..end_token);
                } else {
                    let mut new_tokens = tokens[start.line_index][..start_token]
                        .iter()
                        .copied()
                        .collect::<Vec<_>>();
                    new_tokens.extend(tokens[end.line_index][end_token..].iter().copied());
                    tokens
                        .splice(start.line_index..end.line_index + 1, iter::once(new_tokens));
                }
            }
        }
    }

    fn apply_change_to_inline_inlays(&self, change: &Change, drift: Drift) {
        let mut layout = self.0.layout.borrow_mut();
        let inline_inlays = &mut layout.inline_inlays;
        match *change {
            Change::Insert(point, ref text) => {
                let index = inline_inlays[point.line_index]
                    .iter()
                    .position(|(byte, _)| match byte.cmp(&point.byte_index) {
                        cmp::Ordering::Less => false,
                        cmp::Ordering::Equal => match drift {
                            Drift::Before => true,
                            Drift::After => false,
                        },
                        cmp::Ordering::Greater => true,
                    })
                    .unwrap_or(inline_inlays[point.line_index].len());
                if text.length().line_count == 0 {
                    for (byte, _) in &mut inline_inlays[point.line_index][index..] {
                        *byte += text.length().byte_count;
                    }
                } else {
                    let mut new_inline_inlays = (0..text.as_lines().len())
                        .map(|_| Vec::new())
                        .collect::<Vec<_>>();
                    new_inline_inlays
                        .first_mut()
                        .unwrap()
                        .splice(..0, inline_inlays[point.line_index].drain(..index));
                    new_inline_inlays.last_mut().unwrap().splice(
                        ..0,
                        inline_inlays[point.line_index].drain(..).map(
                            |(byte, inline_inlay)| (byte + text.length().byte_count, inline_inlay),
                        ),
                    );
                    inline_inlays
                        .splice(point.line_index..point.line_index + 1, new_inline_inlays);
                }
            }
            Change::Delete(start, length) => {
                let end = start + length;
                let start_inlay = inline_inlays[start.line_index]
                    .iter()
                    .position(|&(byte, _)| byte >= start.byte_index)
                    .unwrap_or(inline_inlays[start.line_index].len());
                let end_inlay = inline_inlays[end.line_index]
                    .iter()
                    .position(|&(byte, _)| byte >= end.byte_index)
                    .unwrap_or(inline_inlays[end.line_index].len());
                if length.line_count == 0 {
                    inline_inlays[start.line_index].drain(start_inlay..end_inlay);
                    for (byte, _) in &mut inline_inlays[start.line_index][start_inlay..] {
                        *byte = start.byte_index + (*byte - end.byte_index.min(*byte));
                    }
                } else {
                    let mut new_inline_inlays = inline_inlays[start.line_index]
                        .drain(..start_inlay)
                        .collect::<Vec<_>>();
                        new_inline_inlays.extend(
                        inline_inlays[end.line_index].drain(end_inlay..).map(
                            |(byte, inline_inlay)| {
                                (
                                    start.byte_index + byte - end.byte_index.min(byte),
                                    inline_inlay,
                                )
                            },
                        ),
                    );
                    inline_inlays.splice(
                        start.line_index..end.line_index + 1,
                        iter::once(new_inline_inlays),
                    );
                }
            }
        }
    }
}

#[derive(Debug)]
pub struct Editor<'a> {
    history: &'a mut History,
    edits: &'a mut Vec<Edit>,
}

impl<'a> Editor<'a> {
    pub fn as_text(&mut self) -> &Text {
        self.history.as_text()
    }

    pub fn apply_edit(&mut self, edit: Edit) {
        self.history.apply_edit(edit.clone());
        self.edits.push(edit);
    }
}

#[derive(Debug)]
struct DocumentLayout {
    tokens: Vec<Vec<Token>>,
    inline_inlays: Vec<Vec<(usize, InlineInlay)>>,
    block_inlays: Vec<(usize, BlockInlay)>,
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
    let indentation = string.leading_whitespace().unwrap_or("");
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

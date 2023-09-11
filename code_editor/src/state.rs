use {
    crate::{
        char::CharExt,
        history::EditKind,
        inlays::{BlockInlay, InlineInlay},
        iter::IteratorExt,
        line::Wrapped,
        move_ops,
        selection::Affinity,
        str::StrExt,
        text::{Change, ChangeKind, Drift, Extent, Point, Range, Text},
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
    selections: Vec<Selection>,
    pending_selection_index: Option<usize>,
    change_receiver: Receiver<(Option<Vec<Selection>>, Vec<Change>)>,
}

impl Session {
    pub fn new(document: Rc<RefCell<Document>>) -> Self {
        static ID: AtomicUsize = AtomicUsize::new(0);

        let (change_sender, change_receiver) = mpsc::channel();
        let line_count = document.borrow().text.as_lines().len();
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
            selections: vec![Selection::default()].into(),
            pending_selection_index: None,
            change_receiver,
        };
        for line in 0..line_count {
            session.update_wrap_data(line);
        }
        session.update_y();
        session
            .document
            .borrow_mut()
            .change_senders
            .insert(session.id, change_sender);
        session
    }

    pub fn id(&self) -> SessionId {
        self.id
    }

    pub fn width(&self) -> f64 {
        self.lines(0, self.document.borrow().text.as_lines().len(), |lines| {
            let mut width: f64 = 0.0;
            for line in lines {
                width = width.max(line.width());
            }
            width
        })
    }

    pub fn height(&self) -> f64 {
        let index = self.document.borrow().text.as_lines().len() - 1;
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
            text: &document.text.as_lines()[line],
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
            text: document.text.as_lines()[start_line..end_line].iter(),
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
        let line_count = self.document.borrow().text.as_lines().len();
        for line in 0..line_count {
            self.update_wrap_data(line);
        }
        self.update_y();
    }

    pub fn fold(&mut self) {
        let document = self.document.borrow();
        let lines = document.text.as_lines();
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

    pub fn set_cursor(&mut self, cursor: Point, affinity: Affinity) {
        self.selections.clear();
        self.selections.push(Selection {
            anchor: cursor,
            cursor,
            affinity,
            preferred_column: None,
        });
        self.pending_selection_index = Some(0);
        self.document.borrow_mut().force_new_edit_group();
    }

    pub fn add_cursor(&mut self, cursor: Point, affinity: Affinity) {
        let selection = Selection {
            anchor: cursor,
            cursor,
            affinity,
            preferred_column: None,
        };
        self.pending_selection_index = Some(
            match self.selections.binary_search_by(|selection| {
                if selection.end() <= cursor {
                    return cmp::Ordering::Less;
                }
                if selection.start() >= cursor {
                    return cmp::Ordering::Greater;
                }
                cmp::Ordering::Equal
            }) {
                Ok(index) => {
                    self.selections[index] = selection;
                    index
                }
                Err(index) => {
                    self.selections.insert(index, selection);
                    index
                }
            },
        );
        self.document.borrow_mut().force_new_edit_group();
    }

    pub fn move_to(&mut self, cursor: Point, affinity: Affinity) {
        let mut pending_selection_index = self.pending_selection_index.unwrap();
        self.selections[pending_selection_index] = Selection {
            cursor,
            affinity,
            ..self.selections[pending_selection_index]
        };
        while pending_selection_index > 0 {
            let prev_selection_index = pending_selection_index - 1;
            if !self.selections[prev_selection_index]
                .should_merge(self.selections[pending_selection_index])
            {
                break;
            }
            self.selections.remove(prev_selection_index);
            pending_selection_index -= 1;
        }
        while pending_selection_index + 1 < self.selections.len() {
            let next_selection_index = pending_selection_index + 1;
            if !self.selections[pending_selection_index]
                .should_merge(self.selections[next_selection_index])
            {
                break;
            }
            self.selections.remove(next_selection_index);
        }
        self.pending_selection_index = Some(pending_selection_index);
        self.document.borrow_mut().force_new_edit_group();
    }

    pub fn move_left(&mut self, reset_anchor: bool) {
        self.modify_selections(reset_anchor, |session, selection| {
            selection.update_cursor(|cursor, _, _| {
                (
                    move_ops::move_left(session.document.borrow().text.as_lines(), cursor),
                    Affinity::Before,
                    None,
                )
            })
        });
    }

    pub fn move_right(&mut self, reset_anchor: bool) {
        self.modify_selections(reset_anchor, |session, selection| {
            selection.update_cursor(|cursor, _, _| {
                (
                    move_ops::move_right(session.document.borrow().text.as_lines(), cursor),
                    Affinity::Before,
                    None,
                )
            })
        });
    }

    pub fn move_up(&mut self, reset_anchor: bool) {
        self.modify_selections(reset_anchor, |session, selection| {
            selection.update_cursor(|cursor, affinity, preferred_column| {
                move_ops::move_up(session, cursor, affinity, preferred_column)
            })
        });
    }

    pub fn move_down(&mut self, reset_anchor: bool) {
        self.modify_selections(reset_anchor, |session, selection| {
            selection.update_cursor(|cursor, affinity, preferred_column| {
                move_ops::move_down(session, cursor, affinity, preferred_column)
            })
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
            |_, _, _| (Extent::zero(), false, Some(text.clone()), None),
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
                        Extent {
                            line_count: 0,
                            byte_count: index,
                        }
                    } else {
                        Extent::zero()
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
            |_, _, is_empty| (Extent::zero(), is_empty, None, None),
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
                            Extent {
                                line_count: 1,
                                byte_count: 0,
                            }
                        } else {
                            Extent {
                                line_count: 0,
                                byte_count: line[..index].graphemes().next_back().unwrap().len(),
                            }
                        }
                    } else {
                        Extent::zero()
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
                |selection_0, selection_1| match selection_0.merge(selection_1) {
                    Some(selection) => Ok(selection),
                    None => Err((selection_0, selection_1)),
                },
            )
            .map(|selection| selection.range())
        {
            write!(
                &mut string,
                "{}",
                self.document.borrow().text().slice(range)
            )
            .unwrap();
        }
        string
    }

    pub fn undo(&mut self) -> bool {
        self.document.borrow_mut().undo(self.id)
    }

    pub fn redo(&mut self) -> bool {
        self.document.borrow_mut().redo(self.id)
    }

    fn update_y(&mut self) {
        let start = self.y.len();
        let end = self.document.borrow().text.as_lines().len();
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
        while let Ok((selections, changes)) = self.change_receiver.try_recv() {
            self.apply_changes(selections, &changes);
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
        let mut selections = mem::take(&mut self.selections);
        for selection in &mut selections {
            *selection = f(&self, *selection);
            if reset_anchor {
                *selection = selection.reset_anchor();
            }
        }
        self.selections = selections;
        let mut current_selection_index = 0;
        while current_selection_index + 1 < self.selections.len() {
            let next_selection_index = current_selection_index + 1;
            let current_selection = self.selections[current_selection_index];
            let next_selection = self.selections[next_selection_index];
            assert!(current_selection.start() <= next_selection.start());
            if let Some(merged_selection) = current_selection.merge(next_selection) {
                self.selections[current_selection_index] = merged_selection;
                self.selections.remove(next_selection_index);
                if let Some(pending_selection_index) = self.pending_selection_index.as_mut() {
                    if next_selection_index < *pending_selection_index {
                        *pending_selection_index -= 1;
                    }
                }
            } else {
                current_selection_index += 1;
            }
        }
        self.document.borrow_mut().force_new_edit_group();
    }

    fn apply_changes(&mut self, selections: Option<Vec<Selection>>, changes: &[Change]) {
        for change in changes {
            match &change.kind {
                ChangeKind::Insert(point, text) => {
                    self.column_count[point.line] = None;
                    self.wrap_data[point.line] = None;
                    let line_count = text.extent().line_count;
                    if line_count > 0 {
                        let line = point.line + 1;
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
                ChangeKind::Delete(range) => {
                    self.column_count[range.start().line] = None;
                    self.wrap_data[range.start().line] = None;
                    let line_count = range.extent().line_count;
                    if line_count > 0 {
                        let start_line = range.start().line + 1;
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
        let line_count = self.document.borrow().text.as_lines().len();
        for line in 0..line_count {
            if self.wrap_data[line].is_none() {
                self.update_wrap_data(line);
            }
        }
        if let Some(selections) = selections {
            self.selections = selections;
        } else {
            for change in changes {
                for selection in &mut self.selections {
                    *selection = selection.apply_change(&change);
                }
            }
        }
        self.update_y();
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.document.borrow_mut().change_senders.remove(&self.id);
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
    text: Text,
    tokens: Vec<Vec<Token>>,
    inline_inlays: Vec<Vec<(usize, InlineInlay)>>,
    block_inlays: Vec<(usize, BlockInlay)>,
    history: History,
    tokenizer: Tokenizer,
    change_senders: HashMap<SessionId, Sender<(Option<Vec<Selection>>, Vec<Change>)>>,
}

impl Document {
    pub fn new(text: Text) -> Self {
        let line_count = text.as_lines().len();
        let tokens: Vec<_> = (0..line_count)
            .map(|line| tokenize(&text.as_lines()[line]).collect::<Vec<_>>())
            .collect();
        let mut document = Self {
            text,
            tokens,
            inline_inlays: (0..line_count).map(|_| Vec::new()).collect(),
            block_inlays: Vec::new(),
            history: History::new(),
            tokenizer: Tokenizer::new(line_count),
            change_senders: HashMap::new(),
        };
        document
            .tokenizer
            .update(&document.text, &mut document.tokens);
        document
    }

    pub fn text(&self) -> &Text {
        &self.text
    }

    fn edit(
        &mut self,
        origin_id: SessionId,
        kind: EditKind,
        selections: &[Selection],
        use_soft_tabs: bool,
        tab_column_count: usize,
        indent_column_count: usize,
        mut f: impl FnMut(&String, usize, bool) -> (Extent, bool, Option<Text>, Option<Text>),
    ) {
        let mut changes = Vec::new();
        let mut inverted_changes = Vec::new();
        let mut line_ranges = Vec::new();
        let mut point = Point::zero();
        let mut prev_range_end = Point::zero();
        for range in selections
            .iter()
            .copied()
            .merge(
                |selection_0, selection_1| match selection_0.merge(selection_1) {
                    Some(selection) => Ok(selection),
                    None => Err((selection_0, selection_1)),
                },
            )
            .map(|selection| selection.range())
        {
            point += range.start() - prev_range_end;
            if !range.is_empty() {
                let change = Change {
                    drift: Drift::Before,
                    kind: ChangeKind::Delete(Range::from_start_and_extent(point, range.extent())),
                };
                let inverted_change = change.clone().invert(&self.text);
                self.text.apply_change(change.clone());
                changes.push(change);
                inverted_changes.push(inverted_change);
            }
            let (delete_extent_before, delete_after, insert_text_before, insert_text_after) = f(
                &self.text.as_lines()[point.line],
                point.byte,
                range.is_empty(),
            );
            if delete_extent_before != Extent::zero() {
                if delete_extent_before.line_count == 0 {
                    point.byte -= delete_extent_before.byte_count;
                } else {
                    point.line -= delete_extent_before.line_count;
                    point.byte =
                        self.text.as_lines()[point.line].len() - delete_extent_before.byte_count;
                }
                let change = Change {
                    drift: Drift::Before,
                    kind: ChangeKind::Delete(Range::from_start_and_extent(
                        point,
                        delete_extent_before,
                    )),
                };
                let inverted_change = change.clone().invert(&self.text);
                self.text.apply_change(change.clone());
                changes.push(change);
                inverted_changes.push(inverted_change);
            }
            if delete_after {
                let delete_extent_after = if let Some(grapheme) = self.text.as_lines()[point.line]
                    [point.byte..]
                    .graphemes()
                    .next()
                {
                    Some(Extent {
                        line_count: 0,
                        byte_count: grapheme.len(),
                    })
                } else if point.line < self.text.as_lines().len() - 1 {
                    Some(Extent {
                        line_count: 1,
                        byte_count: 0,
                    })
                } else {
                    None
                };
                if let Some(delete_extent_after) = delete_extent_after {
                    let change = Change {
                        drift: Drift::Before,
                        kind: ChangeKind::Delete(Range::from_start_and_extent(
                            point,
                            delete_extent_after,
                        )),
                    };
                    let inverted_change = change.clone().invert(&self.text);
                    self.text.apply_change(change.clone());
                    changes.push(change);
                    inverted_changes.push(inverted_change);
                }
            }
            if let Some(insert_text_before) = insert_text_before {
                let line_count = insert_text_before.as_lines().len();
                if line_count > 1 {
                    line_ranges.push(
                        (if self.text.as_lines()[point.line][..point.byte]
                            .chars()
                            .all(|char| char.is_whitespace())
                        {
                            point.line
                        } else {
                            point.line + 1
                        })..point.line + line_count,
                    );
                }
                let extent = insert_text_before.extent();
                let change = Change {
                    drift: Drift::Before,
                    kind: ChangeKind::Insert(point, insert_text_before),
                };
                point += extent;
                let inverted_change = change.clone().invert(&self.text);
                self.text.apply_change(change.clone());
                changes.push(change);
                inverted_changes.push(inverted_change);
            }
            if let Some(insert_text_after) = insert_text_after {
                let line_count = insert_text_after.as_lines().len();
                if line_count > 1 {
                    line_ranges.push(
                        (if self.text.as_lines()[point.line][..point.byte]
                            .chars()
                            .all(|char| char.is_whitespace())
                        {
                            point.line
                        } else {
                            point.line + 1
                        })..point.line + line_count,
                    );
                }
                let extent = insert_text_after.extent();
                let change = Change {
                    drift: Drift::After,
                    kind: ChangeKind::Insert(point, insert_text_after),
                };
                point += extent;
                let inverted_change = change.clone().invert(&self.text);
                self.text.apply_change(change.clone());
                changes.push(change);
                inverted_changes.push(inverted_change);
            }
            prev_range_end = range.end();
        }
        self.autoindent(
            &line_ranges,
            use_soft_tabs,
            tab_column_count,
            indent_column_count,
            &mut changes,
            &mut inverted_changes,
        );
        self.history
            .edit(origin_id, kind, selections, inverted_changes);
        self.apply_changes(origin_id, None, &changes);
    }

    fn autoindent(
        &mut self,
        line_ranges: &[ops::Range<usize>],
        use_soft_tabs: bool,
        tab_column_count: usize,
        indent_column_count: usize,
        changes: &mut Vec<Change>,
        inverted_changes: &mut Vec<Change>,
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
            let mut desired_indentation_column_count = self.text.as_lines()[..line_range.start]
                .iter()
                .rev()
                .find_map(|line| {
                    next_line_indentation_column_count(line, tab_column_count, indent_column_count)
                })
                .unwrap_or(0);
            for line in line_range {
                if self.text.as_lines()[line]
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
                self.edit_lines_internal(line, changes, inverted_changes, |line| {
                    reindent(line, use_soft_tabs, tab_column_count, |_| {
                        desired_indentation_column_count
                    })
                });
                if let Some(next_line_indentation_column_count) = next_line_indentation_column_count(
                    &self.text.as_lines()[line],
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
        selections: &[Selection],
        mut f: impl FnMut(&str) -> (usize, usize, String),
    ) {
        let mut changes = Vec::new();
        let mut inverted_changes = Vec::new();
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
                self.edit_lines_internal(line, &mut changes, &mut inverted_changes, &mut f);
            }
        }
        self.history
            .edit(origin_id, kind, selections, inverted_changes);
        self.apply_changes(origin_id, None, &changes);
    }

    fn edit_lines_internal(
        &mut self,
        line: usize,
        changes: &mut Vec<Change>,
        inverted_changes: &mut Vec<Change>,
        mut f: impl FnMut(&str) -> (usize, usize, String),
    ) {
        let (byte, delete_byte_count, insert_text) = f(&self.text.as_lines()[line]);
        if delete_byte_count > 0 {
            let change = Change {
                drift: Drift::Before,
                kind: ChangeKind::Delete(Range::from_start_and_extent(
                    Point { line, byte },
                    Extent {
                        line_count: 0,
                        byte_count: delete_byte_count,
                    },
                )),
            };
            let inverted_change = change.clone().invert(&self.text);
            self.text.apply_change(change.clone());
            changes.push(change);
            inverted_changes.push(inverted_change);
        }
        if !insert_text.is_empty() {
            let change = Change {
                drift: Drift::Before,
                kind: ChangeKind::Insert(Point { line, byte }, insert_text.into()),
            };
            let inverted_change = change.clone().invert(&self.text);
            self.text.apply_change(change.clone());
            changes.push(change);
            inverted_changes.push(inverted_change);
        }
    }

    fn force_new_edit_group(&mut self) {
        self.history.force_new_edit_group()
    }

    fn undo(&mut self, origin_id: SessionId) -> bool {
        if let Some((selections, changes)) = self.history.undo(&mut self.text) {
            self.apply_changes(origin_id, Some(selections), &changes);
            true
        } else {
            false
        }
    }

    fn redo(&mut self, origin_id: SessionId) -> bool {
        if let Some((selections, changes)) = self.history.redo(&mut self.text) {
            self.apply_changes(origin_id, Some(selections), &changes);
            true
        } else {
            false
        }
    }

    fn apply_changes(
        &mut self,
        origin_id: SessionId,
        selections: Option<Vec<Selection>>,
        changes: &[Change],
    ) {
        for change in changes {
            self.apply_change_to_tokens(change);
            self.apply_change_to_inline_inlays(change);
            self.tokenizer.apply_change(change);
        }
        self.tokenizer.update(&self.text, &mut self.tokens);
        for (&session_id, change_sender) in &self.change_senders {
            if session_id == origin_id {
                change_sender
                    .send((selections.clone(), changes.to_vec()))
                    .unwrap();
            } else {
                change_sender
                    .send((
                        None,
                        changes
                            .iter()
                            .cloned()
                            .map(|change| Change {
                                drift: Drift::Before,
                                ..change
                            })
                            .collect(),
                    ))
                    .unwrap();
            }
        }
    }

    fn apply_change_to_tokens(&mut self, change: &Change) {
        match change.kind {
            ChangeKind::Insert(point, ref text) => {
                let mut byte = 0;
                let mut index = self.tokens[point.line]
                    .iter()
                    .position(|token| {
                        if byte + token.len > point.byte {
                            return true;
                        }
                        byte += token.len;
                        false
                    })
                    .unwrap_or(self.tokens[point.line].len());
                if byte != point.byte {
                    let token = self.tokens[point.line][index];
                    let mid = point.byte - byte;
                    self.tokens[point.line][index] = Token {
                        len: mid,
                        kind: token.kind,
                    };
                    index += 1;
                    self.tokens[point.line].insert(
                        index,
                        Token {
                            len: token.len - mid,
                            kind: token.kind,
                        },
                    );
                }
                if text.extent().line_count == 0 {
                    self.tokens[point.line]
                        .splice(index..index, tokenize(text.as_lines().first().unwrap()));
                } else {
                    let mut tokens = (0..text.as_lines().len())
                        .map(|line| tokenize(&text.as_lines()[line]).collect::<Vec<_>>())
                        .collect::<Vec<_>>();
                    tokens
                        .first_mut()
                        .unwrap()
                        .splice(..0, self.tokens[point.line][..index].iter().copied());
                    tokens
                        .last_mut()
                        .unwrap()
                        .splice(..0, self.tokens[point.line][index..].iter().copied());
                    self.tokens.splice(point.line..point.line + 1, tokens);
                }
            }
            ChangeKind::Delete(range) => {
                let mut byte = 0;
                let mut start = self.tokens[range.start().line]
                    .iter()
                    .position(|token| {
                        if byte + token.len > range.start().byte {
                            return true;
                        }
                        byte += token.len;
                        false
                    })
                    .unwrap_or(self.tokens[range.start().line].len());
                if byte != range.start().byte {
                    let token = self.tokens[range.start().line][start];
                    let mid = range.start().byte - byte;
                    self.tokens[range.start().line][start] = Token {
                        len: mid,
                        kind: token.kind,
                    };
                    start += 1;
                    self.tokens[range.start().line].insert(
                        start,
                        Token {
                            len: token.len - mid,
                            kind: token.kind,
                        },
                    );
                }
                let mut byte = 0;
                let mut end = self.tokens[range.end().line]
                    .iter()
                    .position(|token| {
                        if byte + token.len > range.end().byte {
                            return true;
                        }
                        byte += token.len;
                        false
                    })
                    .unwrap_or(self.tokens[range.end().line].len());
                if byte != range.end().byte {
                    let token = self.tokens[range.end().line][end];
                    let mid = range.end().byte - byte;
                    self.tokens[range.end().line][end] = Token {
                        len: mid,
                        kind: token.kind,
                    };
                    end += 1;
                    self.tokens[range.end().line].insert(
                        end,
                        Token {
                            len: token.len - mid,
                            kind: token.kind,
                        },
                    );
                }
                if range.start().line == range.end().line {
                    self.tokens[range.start().line].drain(start..end);
                } else {
                    let mut tokens = self.tokens[range.start().line][..start]
                        .iter()
                        .copied()
                        .collect::<Vec<_>>();
                    tokens.extend(self.tokens[range.end().line][end..].iter().copied());
                    self.tokens
                        .splice(range.start().line..range.end().line + 1, iter::once(tokens));
                }
            }
        }
    }

    fn apply_change_to_inline_inlays(&mut self, change: &Change) {
        match change.kind {
            ChangeKind::Insert(point, ref text) => {
                let index = self.inline_inlays[point.line]
                    .iter()
                    .position(|(byte, _)| match byte.cmp(&point.byte) {
                        cmp::Ordering::Less => false,
                        cmp::Ordering::Equal => match change.drift {
                            Drift::Before => true,
                            Drift::After => false,
                        },
                        cmp::Ordering::Greater => true,
                    })
                    .unwrap_or(self.inline_inlays[point.line].len());
                if text.extent().line_count == 0 {
                    for (byte, _) in &mut self.inline_inlays[point.line][index..] {
                        *byte += text.extent().byte_count;
                    }
                } else {
                    let mut inline_inlays = (0..text.as_lines().len())
                        .map(|_| Vec::new())
                        .collect::<Vec<_>>();
                    inline_inlays
                        .first_mut()
                        .unwrap()
                        .splice(..0, self.inline_inlays[point.line].drain(..index));
                    inline_inlays.last_mut().unwrap().splice(
                        ..0,
                        self.inline_inlays[point.line]
                            .drain(..)
                            .map(|(byte, inline_inlay)| {
                                (byte + text.extent().byte_count, inline_inlay)
                            }),
                    );
                    self.inline_inlays
                        .splice(point.line..point.line + 1, inline_inlays);
                }
            }
            ChangeKind::Delete(range) => {
                let start = self.inline_inlays[range.start().line]
                    .iter()
                    .position(|&(byte, _)| byte >= range.start().byte)
                    .unwrap_or(self.inline_inlays[range.start().line].len());
                let end = self.inline_inlays[range.end().line]
                    .iter()
                    .position(|&(byte, _)| byte >= range.end().byte)
                    .unwrap_or(self.inline_inlays[range.end().line].len());
                if range.start().line == range.end().line {
                    self.inline_inlays[range.start().line].drain(start..end);
                    for (byte, _) in &mut self.inline_inlays[range.start().line][start..] {
                        *byte = range.start().byte + (*byte - range.end().byte.min(*byte));
                    }
                } else {
                    let mut inline_inlays = self.inline_inlays[range.start().line]
                        .drain(..start)
                        .collect::<Vec<_>>();
                    inline_inlays.extend(self.inline_inlays[range.end().line].drain(end..).map(
                        |(byte, inline_inlay)| {
                            (
                                range.start().byte + byte - range.end().byte.min(byte),
                                inline_inlay,
                            )
                        },
                    ));
                    self.inline_inlays.splice(
                        range.start().line..range.end().line + 1,
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

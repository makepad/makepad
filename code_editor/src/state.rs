use {
    crate::{
        char::CharExt,
        document::Document,
        history::EditKind,
        iter::IteratorExt,
        layout::{BlockElement, Layout, WrappedElement},
        selection::{Affinity, Cursor, SelectionSet},
        str::StrExt,
        text::{Change, Drift, Edit, Length, Position, Text},
        wrap,
        wrap::WrapData,
        Selection, Settings,
    },
    std::{
        cell::RefCell,
        collections::HashSet,
        fmt::Write,
        iter, mem,
        rc::Rc,
        sync::{atomic, atomic::AtomicUsize, mpsc, mpsc::Receiver},
    },
};

#[derive(Debug)]
pub struct Session {
    id: SessionId,
    settings: Rc<Settings>,
    document: Document,
    layout: RefCell<SessionLayout>,
    wrap_column: Option<usize>,
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
        let line_count = document.as_text().as_lines().len();
        let mut session = Self {
            id: SessionId(ID.fetch_add(1, atomic::Ordering::AcqRel)),
            settings: Rc::new(Settings::default()),
            document,
            layout: RefCell::new(SessionLayout {
                y: Vec::new(),
                column_count: (0..line_count).map(|_| None).collect(),
                fold_column: (0..line_count).map(|_| 0).collect(),
                scale: (0..line_count).map(|_| 1.0).collect(),
                wrap_data: (0..line_count).map(|_| None).collect(),
            }),
            wrap_column: None,
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
        session.document.add_session(session.id, edit_sender);
        session
    }

    pub fn id(&self) -> SessionId {
        self.id
    }

    pub fn settings(&self) -> &Rc<Settings> {
        &self.settings
    }

    pub fn document(&self) -> &Document {
        &self.document
    }

    pub fn layout(&self) -> Layout<'_> {
        Layout {
            text: self.document.as_text(),
            document_layout: self.document.layout(),
            session_layout: self.layout.borrow(),
        }
    }

    pub fn wrap_column(&self) -> Option<usize> {
        self.wrap_column
    }

    pub fn selections(&self) -> &[Selection] {
        &self.selections
    }

    pub fn set_wrap_column(&mut self, wrap_column: Option<usize>) {
        if self.wrap_column == wrap_column {
            return;
        }
        self.wrap_column = wrap_column;
        let line_count = self.document.as_text().as_lines().len();
        for line in 0..line_count {
            self.update_wrap_data(line);
        }
        self.update_y();
    }

    pub fn fold(&mut self) {
        let text = self.document.as_text();
        let lines = text.as_lines();
        for line in 0..lines.len() {
            let indent_level = lines[line]
                .leading_whitespace()
                .unwrap_or("")
                .column_count(self.settings.tab_column_count)
                / self.settings.indent_column_count;
            if indent_level >= self.settings.fold_level && !self.folded_lines.contains(&line) {
                self.layout.borrow_mut().fold_column[line] =
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
            self.layout.borrow_mut().scale[line] *= 0.9;
            if self.layout.borrow().scale[line] < 0.1 + 0.001 {
                self.layout.borrow_mut().scale[line] = 0.1;
                self.folded_lines.insert(line);
            } else {
                new_folding_lines.insert(line);
            }
            self.layout.borrow_mut().y.truncate(line + 1);
        }
        self.folding_lines = new_folding_lines;
        let mut new_unfolding_lines = HashSet::new();
        for &line in &self.unfolding_lines {
            let scale = self.layout.borrow().scale[line];
            self.layout.borrow_mut().scale[line] = 1.0 - 0.9 * (1.0 - scale);
            if self.layout.borrow().scale[line] > 1.0 - 0.001 {
                self.layout.borrow_mut().scale[line] = 1.0;
            } else {
                new_unfolding_lines.insert(line);
            }
            self.layout.borrow_mut().y.truncate(line + 1);
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
        self.modify_selections(reset_anchor, |selection, layout| {
            selection.update_cursor(|cursor| cursor.move_left(layout.as_text().as_lines()))
        });
    }

    pub fn move_right(&mut self, reset_anchor: bool) {
        self.modify_selections(reset_anchor, |selection, layout| {
            selection.update_cursor(|cursor| cursor.move_right(layout.as_text().as_lines()))
        });
    }

    pub fn move_up(&mut self, reset_anchor: bool) {
        let tab_column_count = self.settings.tab_column_count;
        self.modify_selections(reset_anchor, |selection, layout| {
            selection.update_cursor(|cursor| cursor.move_up(layout, tab_column_count))
        });
    }

    pub fn move_down(&mut self, reset_anchor: bool) {
        let tab_column_count = self.settings.tab_column_count;
        self.modify_selections(reset_anchor, |selection, layout| {
            selection.update_cursor(|cursor| cursor.move_down(layout, tab_column_count))
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
                self.document.as_text().slice(range.start(), range.extent())
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
        let start = self.layout.borrow().y.len();
        let end = self.document.as_text().as_lines().len();
        if start == end + 1 {
            return;
        }
        let mut y = if start == 0 {
            0.0
        } else {
            let layout = self.layout();
            let line = layout.line(start - 1);
            line.y() + line.height()
        };
        let mut ys = mem::take(&mut self.layout.borrow_mut().y);
        for block in self.layout().blocks(start, end) {
            match block {
                BlockElement::Line { is_inlay, line } => {
                    if !is_inlay {
                        ys.push(y);
                    }
                    y += line.height();
                }
                BlockElement::Widget(widget) => {
                    y += widget.height;
                }
            }
        }
        ys.push(y);
        self.layout.borrow_mut().y = ys;
    }

    pub fn handle_changes(&mut self) {
        while let Ok((selections, edits)) = self.edit_receiver.try_recv() {
            self.apply_edits(selections, &edits);
        }
    }

    fn update_column_count(&mut self, index: usize) {
        let mut column_count = 0;
        let mut column = 0;
        let layout = self.layout();
        let line = layout.line(index);
        for wrapped in line.wrapped_elements() {
            match wrapped {
                WrappedElement::Text { text, .. } => {
                    column += text.column_count(self.settings.tab_column_count);
                }
                WrappedElement::Widget(widget) => {
                    column += widget.column_count;
                }
                WrappedElement::Wrap => {
                    column_count = column_count.max(column);
                    column = line.wrap_indent_column_count();
                }
            }
        }
        drop(layout);
        self.layout.borrow_mut().column_count[index] = Some(column_count.max(column));
    }

    fn update_wrap_data(&mut self, line: usize) {
        let wrap_data = match self.wrap_column {
            Some(wrap_column) => {
                let layout = self.layout();
                let line = layout.line(line);
                wrap::compute_wrap_data(line, wrap_column, self.settings.tab_column_count)
            }
            None => WrapData::default(),
        };
        self.layout.borrow_mut().wrap_data[line] = Some(wrap_data);
        self.layout.borrow_mut().y.truncate(line + 1);
        self.update_column_count(line);
    }

    fn modify_selections(
        &mut self,
        reset_anchor: bool,
        mut f: impl FnMut(Selection, &Layout) -> Selection,
    ) {
        let layout = Layout {
            text: self.document.as_text(),
            document_layout: self.document.layout(),
            session_layout: self.layout.borrow(),
        };
        self.pending_selection_index =
            self.selections
                .update_all_selections(self.pending_selection_index, |selection| {
                    let mut selection = f(selection, &layout);
                    if reset_anchor {
                        selection = selection.reset_anchor();
                    }
                    selection
                });
        drop(layout);
        self.delimiter_stack.clear();
        self.document.force_new_group();
    }

    fn apply_edits(&mut self, selections: Option<SelectionSet>, edits: &[Edit]) {
        for edit in edits {
            match edit.change {
                Change::Insert(point, ref text) => {
                    self.layout.borrow_mut().column_count[point.line_index] = None;
                    self.layout.borrow_mut().wrap_data[point.line_index] = None;
                    let line_count = text.length().line_count;
                    if line_count > 0 {
                        let line = point.line_index + 1;
                        self.layout.borrow_mut().y.truncate(line);
                        self.layout
                            .borrow_mut()
                            .column_count
                            .splice(line..line, (0..line_count).map(|_| None));
                        self.layout
                            .borrow_mut()
                            .fold_column
                            .splice(line..line, (0..line_count).map(|_| 0));
                        self.layout
                            .borrow_mut()
                            .scale
                            .splice(line..line, (0..line_count).map(|_| 1.0));
                        self.layout
                            .borrow_mut()
                            .wrap_data
                            .splice(line..line, (0..line_count).map(|_| None));
                    }
                }
                Change::Delete(start, length) => {
                    self.layout.borrow_mut().column_count[start.line_index] = None;
                    self.layout.borrow_mut().wrap_data[start.line_index] = None;
                    let line_count = length.line_count;
                    if line_count > 0 {
                        let start_line = start.line_index + 1;
                        let end_line = start_line + line_count;
                        self.layout.borrow_mut().y.truncate(start_line);
                        self.layout
                            .borrow_mut()
                            .column_count
                            .drain(start_line..end_line);
                        self.layout
                            .borrow_mut()
                            .fold_column
                            .drain(start_line..end_line);
                        self.layout.borrow_mut().scale.drain(start_line..end_line);
                        self.layout
                            .borrow_mut()
                            .wrap_data
                            .drain(start_line..end_line);
                    }
                }
            }
        }
        let line_count = self.document.as_text().as_lines().len();
        for line in 0..line_count {
            if self.layout.borrow().wrap_data[line].is_none() {
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
        self.document.remove_session(self.id);
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct SessionId(usize);

#[derive(Debug)]
pub struct SessionLayout {
    pub y: Vec<f64>,
    pub column_count: Vec<Option<usize>>,
    pub fold_column: Vec<usize>,
    pub scale: Vec<f64>,
    pub wrap_data: Vec<Option<WrapData>>,
}

pub fn reindent(
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

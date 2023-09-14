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
        cell::{Ref, RefCell},
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
    selection_state: RefCell<SelectionState>,
    wrap_column: Option<usize>,
    folding_lines: HashSet<usize>,
    folded_lines: HashSet<usize>,
    unfolding_lines: HashSet<usize>,
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
            selection_state: RefCell::new(SelectionState {
                selections: SelectionSet::new(),
                last_added_selection_index: None,
                delimiter_stack: Vec::new(),
            }),
            wrap_column: None,
            folding_lines: HashSet::new(),
            folded_lines: HashSet::new(),
            unfolding_lines: HashSet::new(),

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

    pub fn selections(&self) -> Ref<'_, [Selection]> {
        Ref::map(self.selection_state.borrow(), |selection_state| {
            selection_state.selections.as_selections()
        })
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
            let indent_level =
                lines[line].indent().unwrap_or("").column_count() / self.settings.tab_column_count;
            if indent_level >= self.settings.fold_level && !self.folded_lines.contains(&line) {
                self.layout.borrow_mut().fold_column[line] =
                    self.settings.fold_level * self.settings.tab_column_count;
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
        let mut selection_state = self.selection_state.borrow_mut();
        selection_state
            .selections
            .set_selection(Selection::from(Cursor {
                position,
                affinity,
                preferred_column_index: None,
            }));
        selection_state.last_added_selection_index = Some(0);
        selection_state.delimiter_stack.clear();
        drop(selection_state);
        self.document.force_new_group();
    }

    pub fn add_cursor(&mut self, position: Position, affinity: Affinity) {
        let mut selection_state = self.selection_state.borrow_mut();
        selection_state.last_added_selection_index = Some(
            selection_state
                .selections
                .add_selection(Selection::from(Cursor {
                    position,
                    affinity,
                    preferred_column_index: None,
                })),
        );
        selection_state.delimiter_stack.clear();
        drop(selection_state);
        self.document.force_new_group();
    }

    pub fn move_to(&mut self, position: Position, affinity: Affinity) {
        let mut selection_state = self.selection_state.borrow_mut();
        let last_added_selection_index = selection_state.last_added_selection_index.unwrap();
        selection_state.last_added_selection_index = Some(
            selection_state
                .selections
                .update_selection(last_added_selection_index, |selection| {
                    selection.update_cursor(|_| Cursor {
                        position,
                        affinity,
                        preferred_column_index: None,
                    })
                }),
        );
        selection_state.delimiter_stack.clear();
        drop(selection_state);
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
        self.modify_selections(reset_anchor, |selection, layout| {
            selection.update_cursor(|cursor| cursor.move_up(layout))
        });
    }

    pub fn move_down(&mut self, reset_anchor: bool) {
        self.modify_selections(reset_anchor, |selection, layout| {
            selection.update_cursor(|cursor| cursor.move_down(layout))
        });
    }

    pub fn insert(&mut self, text: Text) {
        let mut edit_kind = EditKind::Insert;
        let mut inject_delimiter = None;
        let mut uninject_delimiter = None;
        let mut selection_state = self.selection_state.borrow_mut();
        match text.to_single_char() {
            Some(' ') => {
                edit_kind = EditKind::InsertSpace;
            }
            Some(char) if char.is_opening_delimiter() => {
                if selection_state.selections.iter().all(|selection| {
                    !selection.is_empty()
                        || self.document.as_text().as_lines()[selection.cursor.position.line_index]
                            [selection.cursor.position.byte_index..]
                            .chars()
                            .all(|char| char.is_whitespace())
                }) {
                    let char = char.opposite_delimiter().unwrap();
                    inject_delimiter = Some(char);
                    selection_state.delimiter_stack.push(char);
                }
            }
            Some(char)
                if selection_state
                    .delimiter_stack
                    .last()
                    .map_or(false, |&last_char| last_char == char) =>
            {
                uninject_delimiter = Some(selection_state.delimiter_stack.pop().unwrap());
            }
            _ => {}
        }
        drop(selection_state);
        self.document.edit_selections(
            self.id,
            edit_kind,
            &self.selection_state.borrow().selections,
            &self.settings,
            |mut editor, position, length| {
                let mut position = position;
                let mut length = length;
                if inject_delimiter.is_none() {
                    editor.apply_edit(Edit {
                        change: Change::Delete(position, length),
                        drift: Drift::Before,
                    });
                    length = Length::zero();
                }
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
                position += text.length();
                if let Some(inject_delimiter) = inject_delimiter {
                    editor.apply_edit(Edit {
                        change: Change::Insert(position + length, Text::from(inject_delimiter)),
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
            &self.selection_state.borrow().selections,
            &self.settings,
            |mut editor, position, length| {
                let line = &editor.as_text().as_lines()[position.line_index];
                let delete_whitespace = !line.is_empty()
                    && line[..position.byte_index]
                        .chars()
                        .all(|char| char.is_whitespace());
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
                let mut position = position;
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
                    position.byte_index = 0;
                }
                editor.apply_edit(Edit {
                    change: Change::Delete(position, length),
                    drift: Drift::Before,
                });
                editor.apply_edit(Edit {
                    change: Change::Insert(position, Text::newline()),
                    drift: Drift::Before,
                });
                position.line_index += 1;
                if inject_newline {
                    editor.apply_edit(Edit {
                        change: Change::Insert(position, Text::newline()),
                        drift: Drift::After,
                    });
                }
            },
        );
    }

    pub fn tab(&mut self) {
        self.document.edit_selections(
            self.id,
            EditKind::Insert,
            &self.selection_state.borrow().selections,
            &self.settings,
            |mut editor, position, length| {
                let lines = editor.as_text().as_lines();
                let column_index = lines[position.line_index][..position.byte_index].column_count();
                let column_count =
                    self.settings.tab_column_count - column_index % self.settings.tab_column_count;
                editor.apply_edit(Edit {
                    change: Change::Delete(position, length),
                    drift: Drift::Before,
                });
                editor.apply_edit(Edit {
                    change: Change::Insert(
                        position,
                        iter::repeat(' ').take(column_count).collect(),
                    ),
                    drift: Drift::Before,
                });
            },
        );
    }

    pub fn delete(&mut self) {
        self.document.edit_selections(
            self.id,
            EditKind::Delete,
            &self.selection_state.borrow().selections,
            &self.settings,
            |mut editor, position, length| {
                let mut length = length;
                if length == Length::zero() {
                    // The selection is non-empty, so delete forward.
                    let lines = editor.as_text().as_lines();
                    if lines[position.line_index][position.byte_index..]
                        .chars()
                        .all(|char| char.is_whitespace())
                    {

                    } else {

                    }
                    if position.byte_index < lines[position.line_index].len() {
                        length.byte_count += 1;
                    } else if position.line_index < lines.len() {
                        length.line_count += 1;
                        length.byte_count = 0;
                    }
                } else {
                    // The selection is non-empty, so delete it.
                    editor.apply_edit(Edit {
                        change: Change::Delete(position, length),
                        drift: Drift::Before,
                    });
                }
            },
        );
    }

    pub fn backspace(&mut self) {
        self.document.edit_selections(
            self.id,
            EditKind::Delete,
            &self.selection_state.borrow().selections,
            &self.settings,
            |mut editor, position, length| {
                if length == Length::zero() {
                    // The selection is empty, so delete backwards.
                    let lines = editor.as_text().as_lines();
                    if lines[position.line_index][..position.byte_index]
                        .chars()
                        .all(|char| char.is_whitespace())
                    {
                        // There are only whitespace characters before the cursor on this line, so
                        // delete backwards until either the first non-whitespace character on the
                        // previous line, if it exists, or otherwise until the start of the text.
                        if position.line_index > 0 {
                            // There is a previous line, so delete until the first non-whitespace
                            // character on the previous line.
                            let byte_count = lines[position.line_index - 1]
                                    .chars()
                                    .rev()
                                    .take_while(|char| char.is_whitespace())
                                    .map(|char| char.len_utf8())
                                    .sum::<usize>();
                            let byte_index = lines[position.line_index - 1].len() - byte_count;
                            if byte_index == 0 {
                                // The previous line is empty, so keep the indentation on the
                                // current line.
                                editor.apply_edit(Edit {
                                    change: Change::Delete(
                                        Position {
                                            line_index: position.line_index - 1,
                                            byte_index,
                                        },
                                        Length {
                                            line_count: 1,
                                            byte_count: 0,
                                        },
                                    ),
                                    drift: Drift::Before,
                                });
                            } else {
                                // The previous line is non-empty, so don't keep the indentation on
                                // the current line.
                                editor.apply_edit(Edit {
                                    change: Change::Delete(
                                        Position {
                                            line_index: position.line_index - 1,
                                            byte_index,
                                        },
                                        Length {
                                            line_count: 1,
                                            byte_count: position.byte_index,
                                        },
                                    ),
                                    drift: Drift::Before,
                                });
                            }
                        } else {
                            // There is no previous line, so delete backwards until the start of
                            // the text.
                            editor.apply_edit(Edit {
                                change: Change::Delete(
                                    Position::zero(),
                                    Length {
                                        line_count: 0,
                                        byte_count: position.byte_index,
                                    },
                                ),
                                drift: Drift::Before,
                            });
                        }
                    } else {
                        // There is at least one non-whitespace character before the cursor on the
                        // current line, so delete backwards by a single grapheme.
                        let byte_count = lines[position.line_index]
                            .graphemes()
                            .next_back()
                            .unwrap()
                            .len();
                        editor.apply_edit(Edit {
                            change: Change::Delete(
                                Position {
                                    line_index: position.line_index,
                                    byte_index: position.byte_index - byte_count,
                                },
                                Length {
                                    line_count: 0,
                                    byte_count,
                                },
                            ),
                            drift: Drift::Before,
                        });
                    }
                } else {
                    // The selection is non-empty, so delete it.
                    editor.apply_edit(Edit {
                        change: Change::Delete(position, length),
                        drift: Drift::Before,
                    });
                }
            }
        );
    }

    pub fn indent(&mut self) {
        self.document.edit_linewise(
            self.id,
            EditKind::Other,
            &self.selection_state.borrow().selections,
            |mut editor, line_index| {
                let indent_column_count = editor.as_text().as_lines()[line_index]
                    .indent()
                    .unwrap_or("")
                    .len();
                let column_count = self.settings.tab_column_count
                    - indent_column_count % self.settings.tab_column_count;
                editor.apply_edit(Edit {
                    change: Change::Insert(
                        Position {
                            line_index,
                            byte_index: indent_column_count,
                        },
                        iter::repeat(' ').take(column_count).collect(),
                    ),
                    drift: Drift::Before,
                });
            },
        );
    }

    pub fn outdent(&mut self) {
        self.document.edit_linewise(
            self.id,
            EditKind::Other,
            &self.selection_state.borrow().selections,
            |mut editor, line_index| {
                let indent_column_count = editor.as_text().as_lines()[line_index]
                    .indent()
                    .unwrap_or("")
                    .len();
                let column_count = indent_column_count.min(
                    (indent_column_count + self.settings.tab_column_count - 1)
                        % self.settings.tab_column_count
                        + 1,
                );
                editor.apply_edit(Edit {
                    change: Change::Delete(
                        Position {
                            line_index,
                            byte_index: indent_column_count - column_count,
                        },
                        Length {
                            line_count: 0,
                            byte_count: column_count,
                        },
                    ),
                    drift: Drift::Before,
                });
            },
        );
    }

    pub fn copy(&self) -> String {
        let mut string = String::new();
        for range in self
            .selection_state
            .borrow()
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
        self.document
            .undo(self.id, &self.selection_state.borrow().selections)
    }

    pub fn redo(&mut self) -> bool {
        self.document
            .redo(self.id, &self.selection_state.borrow().selections)
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
        for block in self.layout().block_elements(start, end) {
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
                    column += text.column_count();
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
                wrap::compute_wrap_data(line, wrap_column)
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
        let mut selection_state = self.selection_state.borrow_mut();
        let last_added_selection_index = selection_state.last_added_selection_index;
        selection_state.last_added_selection_index = selection_state
            .selections
            .update_all_selections(last_added_selection_index, |selection| {
                let mut selection = f(selection, &layout);
                if reset_anchor {
                    selection = selection.reset_anchor();
                }
                selection
            });
        selection_state.delimiter_stack.clear();
        drop(selection_state);
        drop(layout);
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
        let mut selection_state = self.selection_state.borrow_mut();
        if let Some(selections) = selections {
            selection_state.selections = selections;
        } else {
            for edit in edits {
                selection_state.selections.apply_change(edit);
            }
        }
        drop(selection_state);
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

#[derive(Debug)]
struct SelectionState {
    selections: SelectionSet,
    last_added_selection_index: Option<usize>,
    delimiter_stack: Vec<char>,
}

pub fn reindent(string: &str, f: impl FnOnce(usize) -> usize) -> (usize, usize, String) {
    let indentation = string.indent().unwrap_or("");
    let indentation_column_count = indentation.column_count();
    let new_indentation_column_count = f(indentation_column_count);
    let new_indentation = new_indentation(new_indentation_column_count);
    let len = indentation.longest_common_prefix(&new_indentation).len();
    (
        len,
        indentation.len() - len.min(indentation.len()),
        new_indentation[len..].to_owned(),
    )
}

fn new_indentation(column_count: usize) -> String {
    iter::repeat(' ').take(column_count).collect()
}

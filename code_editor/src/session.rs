use {
    crate::{
        char::CharExt,
        document::CodeDocument,
        history::{EditKind,NewGroup},
        layout::{BlockElement, Layout, WrappedElement},
        selection::{Affinity, Cursor, SelectionSet},
        str::StrExt,
        text::{Change, Drift, Edit, Length, Position, Text},
        wrap,
        wrap::WrapData,
        Selection, Settings,
    },
    std::{
        cell::{Cell, Ref, RefCell},
        collections::HashSet,
        fmt::Write,
        iter, mem,
        rc::Rc,
        sync::{atomic, atomic::AtomicUsize, mpsc, mpsc::Receiver},
    },
};

#[derive(Debug)]
pub struct CodeSession {
    id: SessionId,
    settings: Rc<Settings>,
    document: CodeDocument,
    layout: RefCell<SessionLayout>,
    selection_state: RefCell<SelectionState>,
    wrap_column: Cell<Option<usize>>,
    fold_state: RefCell<FoldState>,
    edit_receiver: Receiver<(Option<SelectionSet>, Vec<Edit>)>,
}

impl CodeSession {
    pub fn new(document: CodeDocument) -> Self {
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
                mode: SelectionMode::Simple,
                selections: SelectionSet::new(),
                last_added_selection_index: Some(0),
                injected_char_stack: Vec::new(),
                highlighted_delimiter_positions: HashSet::new(),
            }),
            wrap_column: Cell::new(None),
            fold_state: RefCell::new(FoldState {
                folding_lines: HashSet::new(),
                folded_lines: HashSet::new(),
                unfolding_lines: HashSet::new(),
            }),
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

    pub fn document(&self) -> &CodeDocument {
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
        self.wrap_column.get()
    }

    pub fn selections(&self) -> Ref<'_, [Selection]> {
        Ref::map(self.selection_state.borrow(), |selection_state| {
            selection_state.selections.as_selections()
        })
    }

    pub fn last_added_selection_index(&self) -> Option<usize> {
        self.selection_state.borrow().last_added_selection_index
    }

    pub fn highlighted_delimiter_positions(&self) -> Ref<'_, HashSet<Position>> {
        Ref::map(self.selection_state.borrow(), |selection_state| {
            &selection_state.highlighted_delimiter_positions
        })
    }

    pub fn set_wrap_column(&self, wrap_column: Option<usize>) {
        if self.wrap_column.get() == wrap_column {
            return;
        }
        self.wrap_column.set(wrap_column);
        let line_count = self.document.as_text().as_lines().len();
        for line in 0..line_count {
            self.update_wrap_data(line);
        }
        self.update_y();
    }

    pub fn fold(&self) {
        let mut fold_state = self.fold_state.borrow_mut();
        let line_count = self.document().as_text().as_lines().len();
        for line_index in 0..line_count {
            let layout = self.layout();
            let line = layout.line(line_index);
            let indent_level = line.indent_column_count() / self.settings.tab_column_count;
            drop(layout);
            if indent_level >= self.settings.fold_level
                && !fold_state.folded_lines.contains(&line_index)
            {
                self.layout.borrow_mut().fold_column[line_index] =
                    self.settings.fold_level * self.settings.tab_column_count;
                fold_state.unfolding_lines.remove(&line_index);
                fold_state.folding_lines.insert(line_index);
            }
        }
    }

    pub fn unfold(&self) {
        let fold_state = &mut *self.fold_state.borrow_mut();
        for line in fold_state.folding_lines.drain() {
            fold_state.unfolding_lines.insert(line);
        }
        for line in fold_state.folded_lines.drain() {
            fold_state.unfolding_lines.insert(line);
        }
    }

    pub fn update_folds(&self) -> bool {
        let mut fold_state_ref = self.fold_state.borrow_mut();
        if fold_state_ref.folding_lines.is_empty() && fold_state_ref.unfolding_lines.is_empty() {
            return false;
        }
        let mut layout = self.layout.borrow_mut();
        let mut new_folding_lines = HashSet::new();
        let fold_state = &mut *fold_state_ref;
        for &line in &fold_state.folding_lines {
            layout.scale[line] *= 0.9;
            if layout.scale[line] < 0.1 + 0.001 {
                layout.scale[line] = 0.1;
                fold_state.folded_lines.insert(line);
            } else {
                new_folding_lines.insert(line);
            }
            layout.y.truncate(line + 1);
        }
        fold_state.folding_lines = new_folding_lines;
        let mut new_unfolding_lines = HashSet::new();
        for &line in &fold_state_ref.unfolding_lines {
            let scale = layout.scale[line];
            layout.scale[line] = 1.0 - 0.9 * (1.0 - scale);
            if layout.scale[line] > 1.0 - 0.001 {
                layout.scale[line] = 1.0;
            } else {
                new_unfolding_lines.insert(line);
            }
            layout.y.truncate(line + 1);
        }
        fold_state_ref.unfolding_lines = new_unfolding_lines;
        drop(layout);
        drop(fold_state_ref);
        self.update_y();
        true
    }

    pub fn set_selection(&self, position: Position, affinity: Affinity, mode: SelectionMode, new_group:NewGroup) {
        let position = self.clamp_position(position);
        let selection = grow_selection(
            Selection::from(Cursor {
                position,
                affinity,
                preferred_column_index: None,
            }),
            self.document().as_text().as_lines(),
            mode,
            &self.settings.word_separators,
        );
        let mut selection_state = self.selection_state.borrow_mut();
        selection_state.mode = mode;
        selection_state.selections.set_selection(selection);
        selection_state.last_added_selection_index = Some(0);
        selection_state.injected_char_stack.clear();
        drop(selection_state);
        self.update_highlighted_delimiter_positions();
        if let NewGroup::Yes = new_group{
            self.document().force_new_group();
        }
    }
    
    fn clamp_position(&self, mut position: Position) -> Position {
        let text = self.document().as_text();
        let lines = text.as_lines();
        if position.line_index >= lines.len() {
            position.line_index = lines.len().saturating_sub(1);
            position.byte_index = lines[position.line_index].len();
        } else {
            let line_len = lines[position.line_index].len();
            if position.byte_index > line_len {
                position.byte_index = line_len;
            }
        }
        position
    }

    pub fn add_selection(&self, position: Position, affinity: Affinity, mode: SelectionMode) {
        let selection = grow_selection(
            Selection::from(Cursor {
                position,
                affinity,
                preferred_column_index: None,
            }),
            self.document().as_text().as_lines(),
            mode,
            &self.settings.word_separators,
        );
        let mut selection_state = self.selection_state.borrow_mut();
        selection_state.mode = mode;
        selection_state.last_added_selection_index =
            Some(selection_state.selections.add_selection(selection));
        selection_state.injected_char_stack.clear();
        drop(selection_state);
        self.update_highlighted_delimiter_positions();
        self.document().force_new_group();
    }

    pub fn move_to(&self, position: Position, affinity: Affinity, new_group:NewGroup) {
        let mut selection_state = self.selection_state.borrow_mut();
        let last_added_selection_index = selection_state.last_added_selection_index.unwrap();
        let mode = selection_state.mode;
        selection_state.last_added_selection_index = Some(
            selection_state
                .selections
                .update_selection(last_added_selection_index, |selection| {
                    grow_selection(
                        selection.update_cursor(|_| Cursor {
                            position,
                            affinity,
                            preferred_column_index: None,
                        }),
                        self.document.as_text().as_lines(),
                        mode,
                        &self.settings.word_separators,
                    )
                }),
        );
        selection_state.injected_char_stack.clear();
        drop(selection_state);
        self.update_highlighted_delimiter_positions();
        if let NewGroup::Yes = new_group{
            self.document().force_new_group();
        }
    }

    pub fn move_left(&self, reset_anchor: bool) {
        self.modify_selections(reset_anchor, |selection, layout| {
            selection.update_cursor(|cursor| cursor.move_left(layout.as_text().as_lines()))
        });
    }

    pub fn move_right(&self, reset_anchor: bool) {
        self.modify_selections(reset_anchor, |selection, layout| {
            selection.update_cursor(|cursor| cursor.move_right(layout.as_text().as_lines()))
        });
    }

    pub fn move_up(&self, reset_anchor: bool) {
        self.modify_selections(reset_anchor, |selection, layout| {
            selection.update_cursor(|cursor| cursor.move_up(layout))
        });
    }

    pub fn move_down(&self, reset_anchor: bool) {
        self.modify_selections(reset_anchor, |selection, layout| {
            selection.update_cursor(|cursor| cursor.move_down(layout))
        });
    }

    pub fn home(&self, reset_anchor: bool) {
        self.modify_selections(reset_anchor, |selection, layout| {
            selection.update_cursor(|cursor| cursor.home(layout.as_text().as_lines()))
        });
    }

    pub fn end(&self, reset_anchor: bool) {
        self.modify_selections(reset_anchor, |selection, layout| {
            selection.update_cursor(|cursor| cursor.end(layout.as_text().as_lines()))
        });
    }

    pub fn insert(&self, text: Text) {

        let mut edit_kind = EditKind::Insert;
        let mut inject_char = None;
        let mut uninject_char = None;
        if let Some(char) = text.to_single_char() {
            let mut selection_state = self.selection_state.borrow_mut();
            if char == ' ' {
                edit_kind = EditKind::InsertSpace;
            } else if char == '"' || char.is_opening_delimiter() {
                if selection_state
                    .selections
                    .iter()
                    .all(|selection| !selection.is_empty())
                    || selection_state.selections.iter().all(|selection| {
                        selection.is_empty()
                            && match self.document.as_text().as_lines()
                                [selection.cursor.position.line_index]
                                [selection.cursor.position.byte_index..]
                                .chars()
                                .next()
                            {
                                Some(char) => {
                                    char == '"'
                                        || char.is_closing_delimiter()
                                        || char.is_whitespace()
                                }
                                None => true,
                            }
                    })
                {
                    // We are inserting either a string or opening delimiter, and either all
                    // selections are non-empty, or all selections are empty and followed by either
                    // a string or closing delimiter or whitespace. In this case, we automatically
                    // inject the corresponding string or closing delimiter.
                    let opposite_char = if char == '"' {
                        '"'
                    } else {
                        char.opposite_delimiter().unwrap()
                    };
                    inject_char = Some(opposite_char);
                    selection_state.injected_char_stack.push(opposite_char);
                }
            } else if selection_state
                .injected_char_stack
                .last()
                .map_or(false, |&last_char| last_char == char)
            {
                // We are inserting a single character that we automatically injected earlier, so we need
                // to uninject it before inserting it again.
                uninject_char = Some(selection_state.injected_char_stack.pop().unwrap());
            }
            drop(selection_state);
        }
        self.document.edit_selections(
            self.id,
            edit_kind,
            &self.selection_state.borrow().selections,
            &self.settings,
            |mut editor, position, length| {
                let mut position = position;
                let mut length = length;
                if inject_char.is_none() {
                    // Only delete the selection if we are NOT injecting a character. This is for the
                    // use case where we have selected `abc` and want to enclose it like: `{abc}`.
                    editor.apply_edit(Edit {
                        change: Change::Delete(position, length),
                        drift: Drift::Before,
                    });
                    length = Length::zero();
                }
                if let Some(uninject_delimiter) = uninject_char {
                    // To uninject a character, we simply delete it.
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
                if let Some(inject_delimiter) = inject_char {
                    // To inject a character, we do an extra insert with Drift::After so that the
                    // cursor stays in place. Note that we have to add the selected length to our
                    // position, because the selection is only deleted if we are NOT injecting a
                    // character. This is for the use case where we have selected `abc` and want
                    // to enclose it like: `{abc}`.
                    editor.apply_edit(Edit {
                        change: Change::Insert(position + length, Text::from(inject_delimiter)),
                        drift: Drift::After,
                    })
                }
            },
        );
    }

    pub fn paste(&self, text: Text) {
        self.document.edit_selections(
            self.id,
            EditKind::Other,
            &self.selection_state.borrow().selections,
            &self.settings,
            |mut editor, position, length| {
                editor.apply_edit(Edit {
                    change: Change::Delete(position, length),
                    drift: Drift::Before,
                });
                editor.apply_edit(Edit {
                    change: Change::Insert(position, text.clone()),
                    drift: Drift::Before,
                });
            },
        );
    }
    
    pub fn paste_grouped(&self, text: Text, group:u64) {
        self.document.edit_selections(
            self.id,
            EditKind::Group(group),
            &self.selection_state.borrow().selections,
            &self.settings,
            |mut editor, position, length| {
                editor.apply_edit(Edit {
                    change: Change::Delete(position, length),
                    drift: Drift::Before,
                });
                editor.apply_edit(Edit {
                    change: Change::Insert(position, text.clone()),
                    drift: Drift::Before,
                });
            },
        );
    }
    
    pub fn enter(&self) {
        self.selection_state
            .borrow_mut()
            .injected_char_stack
            .clear();
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
                position.byte_index = 0;
                if inject_newline {
                    editor.apply_edit(Edit {
                        change: Change::Insert(position, Text::newline()),
                        drift: Drift::After,
                    });
                }
            },
        );
    }

    pub fn delete(&self) {
        self.selection_state
            .borrow_mut()
            .injected_char_stack
            .clear();
        self.document.edit_selections(
            self.id,
            EditKind::Delete,
            &self.selection_state.borrow().selections,
            &self.settings,
            |mut editor, position, length| {
                if length == Length::zero() {
                    // The selection is empty, so delete forward.
                    let lines = editor.as_text().as_lines();
                    if lines[position.line_index][position.byte_index..]
                        .chars()
                        .all(|char| char.is_whitespace())
                    {
                        // There are only whitespace characters after the cursor on this line, so
                        // delete forward until either the first non-whitespace character on the
                        // next line, if it exists, or otherwise until the end of the text.
                        if position.line_index < lines.len() - 1 {
                            // There is a next line, so delete until the first non-whitespace
                            // character on the next line.
                            let byte_count = lines[position.line_index + 1]
                                .chars()
                                .take_while(|char| char.is_whitespace())
                                .map(|char| char.len_utf8())
                                .sum::<usize>();
                            editor.apply_edit(Edit {
                                change: Change::Delete(
                                    position,
                                    Length {
                                        line_count: 1,
                                        byte_count,
                                    },
                                ),
                                drift: Drift::Before,
                            });
                        } else {
                            // There is no next line, so delete forward until the start of the
                            // text.
                            let byte_count = lines[position.line_index].len() - position.byte_index;
                            editor.apply_edit(Edit {
                                change: Change::Delete(
                                    position,
                                    Length {
                                        line_count: 0,
                                        byte_count,
                                    },
                                ),
                                drift: Drift::Before,
                            });
                        }
                    } else {
                        // There is at least one non-whitespace character before the cursor on the
                        // current line, so delete forward by a single grapheme.
                        let byte_count =
                            lines[position.line_index].graphemes().next().unwrap().len();
                        editor.apply_edit(Edit {
                            change: Change::Delete(
                                position,
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
            },
        );
    }

    pub fn backspace(&self) {
        self.selection_state
            .borrow_mut()
            .injected_char_stack
            .clear();
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
                            // There is no previous line, so delete backwards until the start of the
                            // text.
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
            },
        );
    }

    pub fn indent(&self) {
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

    pub fn outdent(&self) {
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
        for selection in &self.selection_state.borrow().selections {
            write!(
                &mut string,
                "{}",
                self.document
                    .as_text()
                    .slice(selection.start(), selection.length())
            )
            .unwrap();
        }
        string
    }

    pub fn undo(&self) -> bool {
        self.selection_state
            .borrow_mut()
            .injected_char_stack
            .clear();
        self.document
            .undo(self.id, &self.selection_state.borrow().selections)
    }

    pub fn redo(&self) -> bool {
        self.selection_state
            .borrow_mut()
            .injected_char_stack
            .clear();
        self.document
            .redo(self.id, &self.selection_state.borrow().selections)
    }

    pub fn handle_changes(&mut self) {
        while let Ok((selections, edits)) = self.edit_receiver.try_recv() {
            self.update_after_edit(selections, &edits);
        }
    }

    fn modify_selections(
        &self,
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
        selection_state.injected_char_stack.clear();
        drop(selection_state);
        drop(layout);
        self.update_highlighted_delimiter_positions();
        self.document().force_new_group();
    }

    fn update_after_edit(&self, selections: Option<SelectionSet>, edits: &[Edit]) {
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
        self.update_y();
        let mut selection_state = self.selection_state.borrow_mut();
        if let Some(selections) = selections {
            selection_state.selections = selections;
        } else {
            for edit in edits {
                let last_added_selection_index = selection_state.last_added_selection_index;
                selection_state.last_added_selection_index = selection_state
                    .selections
                    .apply_edit(edit, last_added_selection_index);
            }
        }
        drop(selection_state);
        self.update_highlighted_delimiter_positions();
    }

    fn update_y(&self) {
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

    fn update_column_count(&self, index: usize) {
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

    fn update_wrap_data(&self, line: usize) {
        let wrap_data = match self.wrap_column.get() {
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

    fn update_highlighted_delimiter_positions(&self) {
        let mut selection_state = self.selection_state.borrow_mut();
        let mut highlighted_delimiter_positions =
            mem::take(&mut selection_state.highlighted_delimiter_positions);
        highlighted_delimiter_positions.clear();
        for selection in &selection_state.selections {
            if !selection.is_empty() {
                continue;
            }
            if let Some((opening_delimiter_position, closing_delimiter_position)) =
                find_highlighted_delimiter_pair(
                    self.document.as_text().as_lines(),
                    selection.cursor.position,
                )
            {
                highlighted_delimiter_positions.insert(opening_delimiter_position);
                highlighted_delimiter_positions.insert(closing_delimiter_position);
            }
        }
        selection_state.highlighted_delimiter_positions = highlighted_delimiter_positions;
    }
}

impl Drop for CodeSession {
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

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum SelectionMode {
    Simple,
    Word,
    Line,
    All,
}

#[derive(Debug)]
struct SelectionState {
    mode: SelectionMode,
    selections: SelectionSet,
    last_added_selection_index: Option<usize>,
    injected_char_stack: Vec<char>,
    highlighted_delimiter_positions: HashSet<Position>,
}

#[derive(Debug)]
struct FoldState {
    folding_lines: HashSet<usize>,
    folded_lines: HashSet<usize>,
    unfolding_lines: HashSet<usize>,
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

fn grow_selection(
    selection: Selection,
    lines: &[String],
    mode: SelectionMode,
    word_separators: &[char],
) -> Selection {
    match mode {
        SelectionMode::Simple => selection,
        SelectionMode::Word => {
            let position = selection.cursor.position;
            let start_byte_index = lines[position.line_index]
                .find_prev_word_boundary(position.byte_index, word_separators);
            let end_byte_index = lines[position.line_index]
                .find_next_word_boundary(position.byte_index, word_separators);
            if selection.anchor < selection.cursor.position {
                Selection {
                    cursor: Cursor {
                        position: Position {
                            line_index: position.line_index,
                            byte_index: end_byte_index,
                        },
                        affinity: Affinity::Before,
                        preferred_column_index: None,
                    },
                    anchor: selection.anchor,
                }
            } else if selection.anchor > selection.cursor.position {
                Selection {
                    cursor: Cursor {
                        position: Position {
                            line_index: position.line_index,
                            byte_index: start_byte_index,
                        },
                        affinity: Affinity::After,
                        preferred_column_index: None,
                    },
                    anchor: selection.anchor,
                }
            } else {
                Selection {
                    cursor: Cursor {
                        position: Position {
                            line_index: position.line_index,
                            byte_index: end_byte_index,
                        },
                        affinity: Affinity::After,
                        preferred_column_index: None,
                    },
                    anchor: Position {
                        line_index: position.line_index,
                        byte_index: start_byte_index,
                    },
                }
            }
        }
        SelectionMode::Line => {
            let position = selection.cursor.position;
            if selection.anchor < selection.cursor.position {
                Selection {
                    cursor: Cursor {
                        position: Position {
                            line_index: position.line_index,
                            byte_index: lines[position.line_index].len(),
                        },
                        affinity: Affinity::Before,
                        preferred_column_index: None,
                    },
                    anchor: Position {
                        line_index: selection.anchor.line_index,
                        byte_index: 0,
                    },
                }
            } else if selection.anchor > selection.cursor.position {
                Selection {
                    cursor: Cursor {
                        position: Position {
                            line_index: position.line_index,
                            byte_index: 0,
                        },
                        affinity: Affinity::After,
                        preferred_column_index: None,
                    },
                    anchor: Position {
                        line_index: selection.anchor.line_index,
                        byte_index: lines[selection.anchor.line_index].len(),
                    },
                }
            } else {
                Selection {
                    cursor: Cursor {
                        position: Position {
                            line_index: position.line_index,
                            byte_index: lines[position.line_index].len(),
                        },
                        affinity: Affinity::After,
                        preferred_column_index: None,
                    },
                    anchor: Position {
                        line_index: position.line_index,
                        byte_index: 0,
                    },
                }
            }
        }
        SelectionMode::All => Selection {
            cursor: Cursor {
                position: Position {
                    line_index: lines.len() - 1,
                    byte_index: lines[lines.len() - 1].len(),
                },
                affinity: Affinity::After,
                preferred_column_index: None,
            },
            anchor: Position {
                line_index: 0,
                byte_index: 0,
            },
        },
    }
}

fn new_indentation(column_count: usize) -> String {
    iter::repeat(' ').take(column_count).collect()
}

fn find_highlighted_delimiter_pair(
    lines: &[String],
    position: Position,
) -> Option<(Position, Position)> {
    // Cursor is before an opening delimiter
    match lines[position.line_index][position.byte_index..]
        .chars()
        .next()
    {
        Some(ch) if ch.is_opening_delimiter() => {
            let opening_delimiter_position = position;
            if let Some(closing_delimiter_position) = find_closing_delimiter(
                lines,
                Position {
                    line_index: position.line_index,
                    byte_index: position.byte_index + ch.len_utf8(),
                },
                ch,
            ) {
                return Some((opening_delimiter_position, closing_delimiter_position));
            }
        }
        _ => {}
    }
    // Cursor is before a closing delimiter
    match lines[position.line_index][position.byte_index..]
        .chars()
        .next()
    {
        Some(ch) if ch.is_closing_delimiter() => {
            let closing_delimiter_position = position;
            if let Some(opening_delimiter_position) = find_opening_delimiter(lines, position, ch) {
                return Some((opening_delimiter_position, closing_delimiter_position));
            }
        }
        _ => {}
    }
    // Cursor is after a closing delimiter
    match lines[position.line_index][..position.byte_index]
        .chars()
        .next_back()
    {
        Some(ch) if ch.is_closing_delimiter() => {
            let closing_delimiter_position = Position {
                line_index: position.line_index,
                byte_index: position.byte_index - ch.len_utf8(),
            };
            if let Some(opening_delimiter_position) =
                find_opening_delimiter(lines, closing_delimiter_position, ch)
            {
                return Some((opening_delimiter_position, closing_delimiter_position));
            }
        }
        _ => {}
    }
    // Cursor is after an opening delimiter
    match lines[position.line_index][..position.byte_index]
        .chars()
        .next_back()
    {
        Some(ch) if ch.is_opening_delimiter() => {
            let opening_delimiter_position = Position {
                line_index: position.line_index,
                byte_index: position.byte_index - ch.len_utf8(),
            };
            if let Some(closing_delimiter_position) = find_closing_delimiter(lines, position, ch) {
                return Some((opening_delimiter_position, closing_delimiter_position));
            }
        }
        _ => {}
    }
    None
}

fn find_opening_delimiter(
    lines: &[String],
    position: Position,
    closing_delimiter: char,
) -> Option<Position> {
    let mut delimiter_stack = vec![closing_delimiter];
    let mut position = position;
    loop {
        for char in lines[position.line_index][..position.byte_index]
            .chars()
            .rev()
        {
            position.byte_index -= char.len_utf8();
            if char.is_closing_delimiter() {
                delimiter_stack.push(char);
            }
            if char.is_opening_delimiter() {
                if delimiter_stack.last() != Some(&char.opposite_delimiter().unwrap()) {
                    return None;
                }
                delimiter_stack.pop().unwrap();
                if delimiter_stack.is_empty() {
                    return Some(position);
                }
            }
        }
        if position.line_index == 0 {
            return None;
        }
        position.line_index -= 1;
        position.byte_index = lines[position.line_index].len();
    }
}

fn find_closing_delimiter(
    lines: &[String],
    position: Position,
    opening_delimiter: char,
) -> Option<Position> {
    let mut delimiter_stack = vec![opening_delimiter];
    let mut position = position;
    loop {
        for char in lines[position.line_index][position.byte_index..].chars() {
            if char.is_opening_delimiter() {
                delimiter_stack.push(char);
            }
            if char.is_closing_delimiter() {
                if delimiter_stack.last() != Some(&char.opposite_delimiter().unwrap()) {
                    return None;
                }
                delimiter_stack.pop().unwrap();
                if delimiter_stack.is_empty() {
                    return Some(position);
                }
            }
            position.byte_index += char.len_utf8();
        }
        if position.line_index == lines.len() - 1 {
            return None;
        }
        position.line_index += 1;
        position.byte_index = 0;
    }
}

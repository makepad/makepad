use {
    crate::{
        components::{code_editor, root},
        core::{
            delta,
            generational::{Arena, Index, IndexAllocator},
            position_set, range_set, Position, PositionSet, Range, RangeSet, Text,
        },
    },
    std::collections::HashMap,
};

#[derive(Clone, Debug)]
pub struct State {
    pub document_index_allocator: IndexAllocator,
    pub documents: Arena<Document>,
    pub editor_index_allocator: IndexAllocator,
    pub editors: Arena<Editor>,
    pub editor_index: Index,
}

impl State {
    pub fn new() -> State {
        State::default()
    }

    pub fn handle_action(&mut self, action: root::Action) {
        let editor = &mut self.editors[self.editor_index];
        match action {
            root::Action(code_editor::Action::AddCursor { position }) => {
                editor.cursors.0.push(Cursor {
                    head: position,
                    tail: position,
                    max_column: position.column,
                });
                editor.update_selections_and_carets();
            }
            root::Action(code_editor::Action::MoveCursorLeft { select }) => {
                let lines = &self.documents[editor.document_index].text.as_lines();
                for cursor in &mut editor.cursors.0 {
                    if cursor.head.column == 0 {
                        if cursor.head.line > 0 {
                            cursor.head.line -= 1;
                            cursor.head.column = lines[cursor.head.line].len();
                        }
                    } else {
                        cursor.head.column -= 1;
                    }
                    if !select {
                        cursor.tail = cursor.head;
                    }
                    cursor.max_column = cursor.head.column;
                }
                editor.update_selections_and_carets();
            }
            root::Action(code_editor::Action::MoveCursorRight { select }) => {
                let lines = self.documents[editor.document_index].text.as_lines();
                for cursor in &mut editor.cursors.0 {
                    if cursor.head.column == lines[cursor.head.line].len() {
                        if cursor.head.line < lines.len() {
                            cursor.head.line += 1;
                            cursor.head.column = 0;
                        }
                    } else {
                        cursor.head.column += 1;
                    }
                    if !select {
                        cursor.tail = cursor.head;
                    }
                    cursor.max_column = cursor.head.column;
                }
                editor.update_selections_and_carets();
            }
            root::Action(code_editor::Action::MoveCursorUp { select }) => {
                let lines = &self.documents[editor.document_index].text.as_lines();
                for cursor in &mut editor.cursors.0 {
                    if cursor.head.line == 0 {
                        continue;
                    }
                    cursor.head.line -= 1;
                    cursor.head.column = cursor.max_column.min(lines[cursor.head.line].len());
                    if !select {
                        cursor.tail = cursor.head;
                    }
                }
                editor.update_selections_and_carets();
            }
            root::Action(code_editor::Action::MoveCursorDown { select }) => {
                let lines = &self.documents[editor.document_index].text.as_lines();
                for cursor in &mut editor.cursors.0 {
                    if cursor.head.line == lines.len() - 1 {
                        continue;
                    }
                    cursor.head.line += 1;
                    cursor.head.column = cursor.max_column.min(lines[cursor.head.line].len());
                    if !select {
                        cursor.tail = cursor.head;
                    }
                }
                editor.update_selections_and_carets();
            }
            root::Action(code_editor::Action::MoveCursorTo { position, select }) => {
                let cursors = &mut editor.cursors;
                if !select {
                    cursors.0.drain(..cursors.0.len() - 1);
                }
                let mut cursor = cursors.0.last_mut().unwrap();
                cursor.head = position;
                if !select {
                    cursor.tail = position;
                }
                cursor.max_column = position.column;
                editor.update_selections_and_carets();
            }
            root::Action(code_editor::Action::InsertText { text }) => {
                let mut builder = delta::Builder::new();
                for span in editor.selections.spans() {
                    if span.is_included {
                        builder.delete(span.len);
                    } else {
                        builder.retain(span.len);
                    }
                }
                let delta_0 = builder.build();
                let mut builder = delta::Builder::new();
                let mut position = Position::origin();
                for distance in editor.carets.distances() {
                    builder.retain(distance);
                    position += distance;
                    if !editor.selections.contains_position(position) {
                        builder.insert(text.clone());
                        position += text.len();
                    }
                }
                let delta_1 = builder.build();
                let (_, delta_1) = delta_0.clone().transform(delta_1);
                let delta = delta_0.compose(delta_1);
                let map = editor
                    .carets
                    .iter()
                    .zip(editor.carets.transform(&delta))
                    .collect::<HashMap<_, _>>();
                self.documents[editor.document_index]
                    .text
                    .apply_delta(delta);
                for cursor in &mut editor.cursors.0 {
                    cursor.head = *map.get(&cursor.head).unwrap();
                    cursor.tail = cursor.head;
                    cursor.max_column = cursor.head.column;
                }
                editor.update_selections_and_carets();
            }
        }
    }
}

impl Default for State {
    fn default() -> State {
        let mut document_index_allocator = IndexAllocator::default();
        let mut documents = Arena::default();
        let document_index = document_index_allocator.allocate();
        documents.insert(
            document_index,
            Document {
                text: include_str!("components/code_editor.rs")
                    .lines()
                    .map(|line| line.chars().collect::<Vec<_>>())
                    .collect::<Vec<_>>()
                    .into(),
            },
        );
        let mut editor_index_allocator = IndexAllocator::default();
        let mut editors = Arena::default();
        let editor_index = editor_index_allocator.allocate();
        let cursors = CursorSet(vec![Cursor::default()]);
        let selections = cursors.selections();
        let carets = cursors.carets();
        editors.insert(
            editor_index,
            Editor {
                document_index,
                cursors,
                selections,
                carets,
            },
        );
        State {
            document_index_allocator,
            documents,
            editor_index_allocator,
            editors,
            editor_index,
        }
    }
}

#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub struct Document {
    pub text: Text,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Editor {
    pub document_index: Index,
    pub cursors: CursorSet,
    pub selections: RangeSet,
    pub carets: PositionSet,
}

impl Editor {
    pub fn update_selections_and_carets(&mut self) {
        self.selections = self.cursors.selections();
        self.carets = self.cursors.carets();
    }
}

#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub struct CursorSet(pub Vec<Cursor>);

impl CursorSet {
    pub fn selections(&self) -> RangeSet {
        let mut builder = range_set::Builder::new();
        for cursor in &self.0 {
            builder.include(cursor.range());
        }
        builder.build()
    }

    pub fn carets(&self) -> PositionSet {
        let mut builder = position_set::Builder::new();
        for cursor in &self.0 {
            builder.insert(cursor.head);
        }
        builder.build()
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct Cursor {
    pub head: Position,
    pub tail: Position,
    pub max_column: usize,
}

impl Cursor {
    pub fn range(self) -> Range {
        Range {
            start: self.head.min(self.tail),
            end: self.head.max(self.tail),
        }
    }
}

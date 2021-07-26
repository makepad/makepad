use {
    crate::{
        cursor::Cursor,
        cursor_set::CursorSet,
        delta::{self, Delta},
        document::{Document, DocumentId},
        position::Position,
        position_set::PositionSet,
        range_set::RangeSet,
        size::Size,
        text::Text,
    },
    std::collections::HashMap,
};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct SessionId(pub usize);

pub struct Session {
    cursors: CursorSet,
    selections: RangeSet,
    carets: PositionSet,
    document_id: DocumentId,
}

impl Session {
    pub fn new(document_id: DocumentId) -> Session {
        let mut session = Session {
            cursors: CursorSet::new(),
            selections: RangeSet::new(),
            carets: PositionSet::new(),
            document_id,
        };
        session.update_selections_and_carets();
        session
    }

    pub fn cursors(&self) -> &CursorSet {
        &self.cursors
    }

    pub fn selections(&self) -> &RangeSet {
        &self.selections
    }

    pub fn carets(&self) -> &PositionSet {
        &self.carets
    }

    pub fn document_id(&self) -> DocumentId {
        self.document_id
    }

    pub fn add_cursor(&mut self, position: Position) {
        self.cursors.add(position);
        self.update_selections_and_carets();
    }

    pub fn move_cursors_left(&mut self, documents: &HashMap<DocumentId, Document>, select: bool) {
        let document = &documents[&self.document_id];
        self.cursors.move_right(document.text(), select);
        self.update_selections_and_carets();
    }

    pub fn move_cursors_right(&mut self, documents: &HashMap<DocumentId, Document>, select: bool) {
        let document = &documents[&self.document_id];
        self.cursors.move_right(document.text(), select);
        self.update_selections_and_carets();
    }

    pub fn move_cursors_up(&mut self, documents: &HashMap<DocumentId, Document>, select: bool) {
        let document = &documents[&self.document_id];
        self.cursors.move_up(document.text(), select);
        self.update_selections_and_carets();
    }

    pub fn move_cursors_down(&mut self, documents: &HashMap<DocumentId, Document>, select: bool) {
        let document = &documents[&self.document_id];
        self.cursors.move_down(document.text(), select);
        self.update_selections_and_carets();
    }

    pub fn move_cursors_to(&mut self, position: Position, select: bool) {
        self.cursors.move_to(position, select);
        self.update_selections_and_carets();
    }

    pub fn insert_text(&mut self, documents: &mut HashMap<DocumentId, Document>, text: Text) {
        let document = documents.get_mut(&self.document_id).unwrap();
        let mut builder = delta::Builder::new();
        for span in self.selections.spans() {
            if span.is_included {
                builder.delete(span.len);
            } else {
                builder.retain(span.len);
            }
        }
        let delta_0 = builder.build();
        let mut builder = delta::Builder::new();
        let mut position = Position::origin();
        for distance in self.carets.distances() {
            position += distance;
            builder.retain(distance);
            if !self.selections.contains_position(position) {
                builder.insert(text.clone());
                position += text.len();
            }
        }
        let delta_1 = builder.build();
        let (_, delta_1) = delta_0.clone().transform(delta_1);
        self.apply_delta(document, delta_0.compose(delta_1));
    }

    pub fn insert_backspace(&mut self, documents: &mut HashMap<DocumentId, Document>) {
        let document = documents.get_mut(&self.document_id).unwrap();
        let mut builder = delta::Builder::new();
        for span in self.selections.spans() {
            if span.is_included {
                builder.delete(span.len);
            } else {
                builder.retain(span.len);
            }
        }
        let delta_0 = builder.build();
        let mut builder = delta::Builder::new();
        let mut position = Position::origin();
        for distance in self.carets.distances() {
            position += distance;
            if !self.selections.contains_position(position) {
                if distance.column == 0 {
                    builder.retain(Size {
                        line: distance.line - 1,
                        column: document.text().as_lines()[position.line - 1].len(),
                    });
                    builder.delete(Size { line: 1, column: 0 })
                } else {
                    builder.retain(Size {
                        line: distance.line,
                        column: distance.column - 1,
                    });
                    builder.delete(Size { line: 0, column: 1 });
                }
            } else {
                builder.retain(distance);
            }
        }
        let delta_1 = builder.build();
        let (_, delta_1) = delta_0.clone().transform(delta_1);
        self.apply_delta(document, delta_0.compose(delta_1));
    }

    fn apply_delta(&mut self, document: &mut Document, delta: Delta) {
        let map = self
            .carets
            .iter()
            .cloned()
            .zip(self.carets.transform(&delta))
            .collect::<HashMap<_, _>>();
        self.cursors.map(|cursor| {
            let new_head = *map.get(&cursor.head).unwrap();
            Cursor {
                head: new_head,
                tail: new_head,
                max_column: new_head.column,
            }
        });
        document.apply_delta(delta);
        self.update_selections_and_carets();
    }

    fn update_selections_and_carets(&mut self) {
        self.selections = self.cursors.selections();
        self.carets = self.cursors.carets();
    }
}

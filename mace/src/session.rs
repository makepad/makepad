use {
    crate::{
        cursor_set::CursorSet,
        delta::{Delta, DeltaBuilder},
        document::Document,
        position::Position,
        position_set::PositionSet,
        range_set::RangeSet,
        size::Size,
        text::Text,
    },
    std::{
        collections::HashMap,
        path::{Path, PathBuf},
    },
};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct SessionId(pub usize);

pub struct Session {
    cursors: CursorSet,
    selections: RangeSet,
    carets: PositionSet,
    path: PathBuf,
}

impl Session {
    pub fn new(path: PathBuf) -> Session {
        let mut session = Session {
            cursors: CursorSet::new(),
            selections: RangeSet::new(),
            carets: PositionSet::new(),
            path,
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

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn add_cursor(&mut self, position: Position) {
        self.cursors.add(position);
        self.update_selections_and_carets();
    }

    pub fn move_cursors_left(
        &mut self,
        documents_by_path: &HashMap<PathBuf, Document>,
        select: bool,
    ) {
        let document = &documents_by_path[&self.path];
        self.cursors.move_left(document.text(), select);
        self.update_selections_and_carets();
    }

    pub fn move_cursors_right(
        &mut self,
        documents_by_path: &HashMap<PathBuf, Document>,
        select: bool,
    ) {
        let document = &documents_by_path[&self.path];
        self.cursors.move_right(document.text(), select);
        self.update_selections_and_carets();
    }

    pub fn move_cursors_up(
        &mut self,
        documents_by_path: &HashMap<PathBuf, Document>,
        select: bool,
    ) {
        let document = &documents_by_path[&self.path];
        self.cursors.move_up(document.text(), select);
        self.update_selections_and_carets();
    }

    pub fn move_cursors_down(
        &mut self,
        documents_by_path: &HashMap<PathBuf, Document>,
        select: bool,
    ) {
        let document = &documents_by_path[&self.path];
        self.cursors.move_down(document.text(), select);
        self.update_selections_and_carets();
    }

    pub fn move_cursors_to(&mut self, position: Position, select: bool) {
        self.cursors.move_to(position, select);
        self.update_selections_and_carets();
    }

    pub fn insert_text(&mut self, documents_by_path: &mut HashMap<PathBuf, Document>, text: Text) {
        let document = documents_by_path.get_mut(&self.path).unwrap();
        let mut builder = DeltaBuilder::new();
        for span in self.selections.spans() {
            if span.is_included {
                builder.delete(span.len);
            } else {
                builder.retain(span.len);
            }
        }
        let delta_0 = builder.build();
        let mut builder = DeltaBuilder::new();
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

    pub fn insert_backspace(&mut self, documents_by_path: &mut HashMap<PathBuf, Document>) {
        let document = documents_by_path.get_mut(&self.path).unwrap();
        let mut builder = DeltaBuilder::new();
        for span in self.selections.spans() {
            if span.is_included {
                builder.delete(span.len);
            } else {
                builder.retain(span.len);
            }
        }
        let delta_0 = builder.build();
        let mut builder = DeltaBuilder::new();
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
        self.cursors.apply_delta(&delta);
        document.apply_delta(delta);
        self.update_selections_and_carets();
    }

    fn update_selections_and_carets(&mut self) {
        self.selections = self.cursors.selections();
        self.carets = self.cursors.carets();
    }
}

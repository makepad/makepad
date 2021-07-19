use {
    crate::{
        position::Position,
        position_set::{self, PositionSet},
        range::Range,
        range_set::{self, RangeSet},
        text::Text,
    },
    std::collections::HashMap,
};

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct CursorSet {
    cursors: Vec<Cursor>,
}

impl CursorSet {
    pub fn new() -> CursorSet {
        CursorSet::default()
    }

    pub fn selections(&self) -> RangeSet {
        let mut builder = range_set::Builder::new();
        for cursor in &self.cursors {
            builder.include(cursor.range());
        }
        builder.build()
    }

    pub fn carets(&self) -> PositionSet {
        let mut builder = position_set::Builder::new();
        for cursor in &self.cursors {
            builder.insert(cursor.head);
        }
        builder.build()
    }

    pub fn insert(&mut self, position: Position) {
        self.cursors.push(Cursor {
            head: position,
            tail: position,
            max_column: position.column,
        });
    }

    pub fn move_left(&mut self, text: &Text, select: bool) {
        for cursor in &mut self.cursors {
            if cursor.head.column == 0 {
                if cursor.head.line > 0 {
                    cursor.head.line -= 1;
                    cursor.head.column = text.as_lines()[cursor.head.line].len();
                }
            } else {
                cursor.head.column -= 1;
            }
            if !select {
                cursor.tail = cursor.head;
            }
            cursor.max_column = cursor.head.column;
        }
    }

    pub fn move_right(&mut self, text: &Text, select: bool) {
        for cursor in &mut self.cursors {
            if cursor.head.column == text.as_lines()[cursor.head.line].len() {
                if cursor.head.line < text.as_lines().len() {
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
    }

    pub fn move_up(&mut self, text: &Text, select: bool) {
        for cursor in &mut self.cursors {
            if cursor.head.line == 0 {
                continue;
            }
            cursor.head.line -= 1;
            cursor.head.column = cursor
                .max_column
                .min(text.as_lines()[cursor.head.line].len());
            if !select {
                cursor.tail = cursor.head;
            }
        }
    }

    pub fn move_down(&mut self, text: &Text, select: bool) {
        for cursor in &mut self.cursors {
            if cursor.head.line == text.as_lines().len() - 1 {
                continue;
            }
            cursor.head.line += 1;
            cursor.head.column = cursor
                .max_column
                .min(text.as_lines()[cursor.head.line].len());
            if !select {
                cursor.tail = cursor.head;
            }
        }
    }

    pub fn move_to(&mut self, position: Position, select: bool) {
        let cursors = &mut self.cursors;
        if !select {
            cursors.drain(..cursors.len() - 1);
        }
        let mut cursor = cursors.last_mut().unwrap();
        cursor.head = position;
        if !select {
            cursor.tail = position;
        }
        cursor.max_column = position.column;
    }

    pub fn transform(&mut self, transformation: &HashMap<Position, Position>) {
        for cursor in &mut self.cursors {
            cursor.head = *transformation.get(&cursor.head).unwrap();
            cursor.tail = cursor.head;
            cursor.max_column = cursor.head.column;
        }
    }
}

impl Default for CursorSet {
    fn default() -> CursorSet {
        CursorSet {
            cursors: vec![Cursor::default()],
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
struct Cursor {
    head: Position,
    tail: Position,
    max_column: usize,
}

impl Cursor {
    fn range(self) -> Range {
        Range {
            start: self.head.min(self.tail),
            end: self.head.max(self.tail),
        }
    }
}

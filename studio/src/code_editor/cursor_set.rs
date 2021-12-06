use crate::{
    code_editor::{
        cursor::Cursor,
        delta::{Delta, Whose},
        position::Position,
        position_set::{self, PositionSet},
        range_set::{self, RangeSet},
        text::Text,
    }
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

    pub fn last(&self) -> Cursor {
        *self.cursors.last().unwrap()
    }

    pub fn carets(&self) -> PositionSet {
        let mut builder = position_set::Builder::new();
        for cursor in &self.cursors {
            builder.insert(cursor.head);
        }
        builder.build()
    }

    pub fn add(&mut self, position: Position) {
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

    pub fn apply_delta(&mut self, delta: &Delta, whose: Whose) {
        for cursor in &mut self.cursors {
            let new_head = cursor.head.apply_delta(&delta);
            let new_tail = match whose {
                Whose::Ours => new_head,
                Whose::Theirs => cursor.tail.apply_delta(&delta),
            };
            *cursor = Cursor {
                head: new_head,
                tail: new_tail,
                max_column: new_head.column.max(new_tail.column),
            };
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

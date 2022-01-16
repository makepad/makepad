use {
    crate::{
        makepad_live_tokenizer::{
            delta::Delta,
            position::Position,
            position_set,
            position_set::PositionSet,
            range::Range,
            range_set,
            range_set::RangeSet,
            size::Size,
            text::Text,
        },
        code_editor::{
            cursor::Cursor,
        },
    },
    
    std::slice,
};

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct CursorSet {
    cursors: Vec<Cursor>,
    last_inserted_index: usize,
}

impl CursorSet {
    pub fn new() -> CursorSet {
        CursorSet {
            cursors: vec![Cursor::new()],
            last_inserted_index: 0,
        }
    }
    
    pub fn len(&self) -> usize {
        self.cursors.len()
    }
    
    pub fn iter(&self) -> Iter<'_> {
        Iter {
            iter: self.cursors.iter(),
        }
    }
    
    pub fn last_inserted(&self) -> &Cursor {
        &self.cursors[self.last_inserted_index]
    }
    
    pub fn selections(&self) -> RangeSet {
        let mut builder = range_set::Builder::new();
        for cursor in &self.cursors {
            builder.include(Range {
                start: cursor.start(),
                end: cursor.end()
            });
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
    
    pub fn select_all(&mut self, text: &Text) {
        self.cursors.clear();
        self.last_inserted_index = 0;
        let lines = text.as_lines();
        self.cursors.push(Cursor {
            head: Position {line: 0, column: 0},
            tail: if let Some(last) = lines.last() {
                Position {line: text.as_lines().len() - 1, column: last.len()}
            }
            else {
                Position {line: 0, column: 0}
            },
            max_column: 0
        });
    }
    
    pub fn add(&mut self, position: Position) {
        let index = self.cursors.iter().position( | cursor | {
            cursor.start() > position
        }).unwrap_or_else( || self.cursors.len());
        if index > 0 && position < self.cursors[index - 1].end() {
            return;
        }
        self.cursors.insert(index, Cursor {
            head: position,
            tail: position,
            max_column: position.column
        });
        self.last_inserted_index = index;
        self.normalize();
    }
    
    pub fn move_left(&mut self, text: &Text, select: bool) {
        for cursor in &mut self.cursors {
            cursor.move_left(text, select);
        }
        self.normalize();
    }
    
    pub fn move_right(&mut self, text: &Text, select: bool) {
        for cursor in &mut self.cursors {
            cursor.move_right(text, select);
        }
        self.normalize();
    }
    
    pub fn move_up(&mut self, text: &Text, select: bool) {
        for cursor in &mut self.cursors {
            cursor.move_up(text, select);
        }
        self.normalize();
    }
    
    pub fn move_down(&mut self, text: &Text, select: bool) {
        for cursor in &mut self.cursors {
            cursor.move_down(text, select);
        }
        self.normalize();
    }
    
    pub fn move_to(&mut self, position: Position, select: bool) {
        if select {
            self.cursors[self.last_inserted_index].move_to(position, true);
            self.normalize();
        } else {
            self.cursors.clear();
            self.cursors.push(Cursor {
                head: position,
                tail: position,
                max_column: position.column,
            });
            self.last_inserted_index = 0;
        }
    }
    
    pub fn apply_delta(&mut self, delta: &Delta) {
        for cursor in &mut self.cursors {
            cursor.apply_delta(delta);
        }
        self.normalize();
    }
    
    pub fn apply_offsets(&mut self, offsets: &[Size]) {
        for (cursor, &offset) in self.cursors.iter_mut().zip(offsets) {
            cursor.apply_offset(offset);
        }
    }
    
    fn normalize(&mut self) {
        let mut index = 0;
        while index + 1 < self.cursors.len() {
            if self.cursors[index].tail >= self.cursors[index + 1].head {
                self.cursors[index + 1].head = self.cursors[index].head;
                self.cursors.remove(index);
                if self.last_inserted_index > index {
                    self.last_inserted_index -= 1;
                }
            } else if self.cursors[index].head >= self.cursors[index + 1].tail {
                self.cursors[index + 1].tail = self.cursors[index].tail;
                self.cursors.remove(index);
                if self.last_inserted_index > index {
                    self.last_inserted_index -= 1;
                }
            } else {
                index += 1;
            }
        }
    }
}

impl Default for CursorSet {
    fn default() -> CursorSet {
        CursorSet::new()
    }
}

impl<'a> IntoIterator for &'a CursorSet {
    type Item = &'a Cursor;
    type IntoIter = Iter<'a>;
    
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

pub struct Iter<'a> {
    iter: slice::Iter<'a, Cursor>
}

impl<'a> Iterator for Iter<'a> {
    type Item = &'a Cursor;
    
    fn next(&mut self) -> Option<Self::Item> {
        self.iter.next()
    }
}
use {
    crate::makepad_live_tokenizer::{
        delta::Delta,
        position::Position,
        size::Size,
        text::Text,
    },    
};

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct Cursor {
    pub head: Position,
    pub tail: Position,
    pub max_column: usize,
}

impl Cursor {
    pub fn new() -> Self {
        Self {
            head: Position::origin(),
            tail: Position::origin(),
            max_column: 0,
        }
    }

    pub fn start(&self) -> Position {
        self.head.min(self.tail)
    }
    
    pub fn end(&self) -> Position {
        self.head.max(self.tail)
    }

    pub fn move_left(&mut self, text: &Text, select: bool) {
        if self.head.column == 0 {
            if self.head.line == 0 {
                return
            }
            self.head.line -= 1;
            self.head.column = text.as_lines()[self.head.line].len();
        } else {
            self.head.column -= 1;
        }
        if !select {
            self.tail = self.head;
        }
        self.max_column = self.head.column;
    }

    pub fn move_right(&mut self, text: &Text, select: bool) {
        if self.head.column == text.as_lines()[self.head.line].len() {
            if self.head.line == text.as_lines().len() - 1 {
                return;
            }
            self.head.line += 1;
            self.head.column = 0;
        } else {
            self.head.column += 1;
        }
        if !select {
            self.tail = self.head;
        }
        self.max_column = self.head.column;
    }
    
    pub fn move_up(&mut self, text: &Text, select: bool) {
        if self.head.line == 0 {
            return;
        }
        self.head.line -= 1;
        self.head.column = self
            .max_column
            .min(text.as_lines()[self.head.line].len());
        if !select {
            self.tail = self.head;
        }
    }

    pub fn move_down(&mut self, text: &Text, select: bool) {
        if self.head.line == text.as_lines().len() - 1 {
            return;
        }
        self.head.line += 1;
        self.head.column = self
            .max_column
            .min(text.as_lines()[self.head.line].len());
        if !select {
            self.tail = self.head;
        }
    }

    pub fn move_to(&mut self, position: Position, select: bool) {
        self.head = position;
        if !select {
            self.tail = position;
        }
        self.max_column = position.column;
    }

    pub fn apply_delta(&mut self, delta: &Delta) {
        self.head = self.head.apply_delta(&delta);
        self.tail = self.tail.apply_delta(&delta);
        self.max_column = self.head.column;
    }

    pub fn apply_offset(&mut self, offset: Size) {
        self.head += offset;
        self.tail = self.head;
        self.max_column = self.head.column;
    }
}

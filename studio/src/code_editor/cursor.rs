use {
    crate::makepad_editor_core::{
        delta::Delta,
        position::Position,
        size::Size,
        text::Text,
    },    
};

/// A type for representing a cursor in a text.
/// 
/// A cursor consists of a selection and a caret. The caret is the little
/// blinking line identifying where text can be inserted.
/// 
/// We represent this by giving the cursor a `head` and a `tail` position. The position of the
/// caret is given by the `head`, while the range of the selection is given by both `head` and
/// `tail`. Note that unlike a range, where the start position always lies before the end
/// position, the `head` of a cursor can lie before its `tail` (one scenario where this happens
/// is moving the cursor backwards while selecting).
/// 
/// Each cursor also has a `max_column` field. This is used when moving the cursor up or down. If
/// the line the cursor moved to is not long enough to keep the cursor at the same column it was
/// was before, the cursor is moved to the end of the line instead, but we remember the original
/// column it was on. If the cursor is then moved up or down again, and the line the cursor
/// moved to is long enough, the cursor is moved back to the original column it was on.
/// 
/// This implementation assumes that each code point represents a single character on the screen.
/// While this is true for the Latin script, it is definitely not true for others. To correctly
/// implement this for all Unicode scripts, we need to group code points into grapheme clusters.
/// Doing so requires generating several large Unicode tables, which is why we've held off from this
/// for now.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct Cursor {
    pub head: Position,
    pub tail: Position,
    pub max_column: usize,
}

impl Cursor {
    /// Creates a new `Cursor` at the start of the text.
    /// 
    /// # Examples
    /// 
    /// ```
    /// use makepad_studio::code_editor::Cursor;
    /// 
    /// let cursor = Cursor::new();
    /// ```
    pub fn new() -> Self {
        Self {
            head: Position::origin(),
            tail: Position::origin(),
            max_column: 0,
        }
    }

    /// Returns the start of the selection of this `Cursor`.
    /// 
    /// This is either the `head` or the `tail` of the cursor, whichever is smaller.
    /// 
    /// # Examples
    /// 
    /// ```
    /// use makepad_studio::code_editor::{Cursor};
    /// use makepad_editor_core::{Position};
    /// 
    /// let cursor = Cursor {
    ///     head: Position { line: 2, column: 2 },
    ///     tail: Position { line: 1, column: 1 },
    ///     max_column: 0,
    /// };
    /// assert_eq!(cursor.end(), Position { line: 2, column: 2 });
    /// ```
    pub fn start(&self) -> Position {
        self.head.min(self.tail)
    }
    
    /// Returns the end of the selection of this `Cursor`.
    /// 
    /// This is either the `head` or the `tail` of the cursor, whichever is larger.
    /// 
    /// # Examples
    /// 
    /// ```
    /// use makepad_studio::code_editor::{Cursor};
    /// use makepad_editor_core::{Position};
    /// 
    /// let cursor = Cursor {
    ///     head: Position { line: 2, column: 2 },
    ///     tail: Position { line: 1, column: 1 },
    ///     max_column: 0,
    /// };
    /// assert_eq!(cursor.end(), Position { line: 2, column: 2 });
    /// ```
    pub fn end(&self) -> Position {
        self.head.max(self.tail)
    }

    /// Moves this `Cursor` one column to the left.
    ///
    /// This method takes the `text` on which the cursor operates as argument, because the structure
    /// of the text determines the behavior of the cursor when it moves. If there is no previous
    /// column (i.e. the cursor is at the start of the line) the cursor is moved to the previous
    /// line instead. If there is no previous line either (i.e. the cursor) is at the start of the
    /// `text`, this method has no effect.
    /// 
    /// The `select` argument indicates whether the cursor is selecting while it moves. If `true`,
    /// only the `head` of the cursor is changed, while the `tail` remains unchanged. Otherwise, the
    /// `tail` is set to the same position as the `head`.
    /// 
    /// # Examples
    /// 
    /// ```
    /// use makepad_studio::code_editor::{Cursor};
    /// use makepad_editor_core::{Position, Text};
    /// 
    /// let text = Text::from("abc");
    /// let mut cursor = Cursor {
    ///     head: Position { line: 0, column: 1 },
    ///     tail: Position { line: 0, column: 1 },
    ///     max_column: 1,
    /// };
    /// cursor.move_left(&text, false);
    /// assert_eq!(
    ///     cursor,
    ///     Cursor {
    ///         head: Position { line: 0, column: 0 },
    ///         tail: Position { line: 0, column: 0 },
    ///         max_column: 0,
    ///     }
    /// );
    /// ```
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

    /// Moves this `Cursor` one column to the right.
    ///
    /// This method takes the `text` on which the cursor operates as argument, because the structure
    /// of the text determines the behavior of the cursor when it moves. If there is no next column
    /// (i.e. the cursor is at the end of the line) the cursor is moved to the next line instead. If
    /// there is no next line either (i.e. the cursor) is at the end of the `text`, this method has
    /// no effect.
    /// 
    /// The `select` argument indicates whether the cursor is selecting while it moves. If `true`,
    /// only the `head` of the cursor is changed, while the `tail` remains unchanged. Otherwise, the
    /// `tail` is set to the same position as the `head`.
    /// 
    /// # Examples
    /// 
    /// ```
    /// use makepad_studio::code_editor::{Cursor};
    /// use makepad_editor_core::{Position, Text};
    /// 
    /// let text = Text::from("abc");
    /// let mut cursor = Cursor {
    ///     head: Position { line: 0, column: 1 },
    ///     tail: Position { line: 0, column: 1 },
    ///     max_column: 1,
    /// };
    /// cursor.move_right(&text, false);
    /// assert_eq!(
    ///     cursor,
    ///     Cursor {
    ///         head: Position { line: 0, column: 2 },
    ///         tail: Position { line: 0, column: 2 },
    ///         max_column: 2,
    ///     }
    /// );
    /// ```
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
    
    /// Moves this `Cursor` one line up.
    /// 
    /// This method takes the `text` on which the cursor operates as argument, because the structure
    /// of the text determines the behavior of the cursor when it moves. If there is no previous line
    /// (i.e. the cursor is at the start of the `text`), this method has no effect.
    ///
    /// # Examples
    /// 
    /// The `select` argument indicates whether the cursor is selecting while it moves. If `true`,
    /// only the `head` of the cursor is changed, while the `tail` remains unchanged. Otherwise, the
    /// `tail` is set to the same position as the `head`.
    /// 
    /// ```
    /// use makepad_studio::code_editor::{Cursor};
    /// use makepad_editor_core::{Position, Text};
    /// 
    /// let text = Text::from("abc\ndef\nghi");
    /// let mut cursor = Cursor {
    ///     head: Position { line: 1, column: 1 },
    ///     tail: Position { line: 1, column: 1 },
    ///     max_column: 1,
    /// };
    /// cursor.move_up(&text, false);
    /// assert_eq!(
    ///     cursor,
    ///     Cursor {
    ///         head: Position { line: 0, column: 1 },
    ///         tail: Position { line: 0, column: 1 },
    ///         max_column: 1,
    ///     }
    /// );
    /// ```
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

    /// Moves this `Cursor` one line down.
    /// 
    /// This method takes the `text` on which the cursor operates as argument, because the structure
    /// of the text determines the behavior of the cursor when it moves. If there is no next line
    /// (i.e. the cursor is at the end of the `text`), this method has no effect.
    /// 
    /// The `select` argument indicates whether the cursor is selecting while it moves. If `true`,
    /// only the `head` of the cursor is changed, while the `tail` remains unchanged. Otherwise, the
    /// `tail` is set to the same position as the `head`.
    /// 
    /// # Examples
    /// 
    /// ```
    /// use makepad_studio::code_editor::{Cursor};
    /// use makepad_editor_core::{Position, Text};
    /// 
    /// let text = Text::from("abc\ndef\nghi");
    /// let mut cursor = Cursor {
    ///     head: Position { line: 1, column: 1 },
    ///     tail: Position { line: 1, column: 1 },
    ///     max_column: 1,
    /// };
    /// cursor.move_down(&text, false);
    /// assert_eq!(
    ///     cursor,
    ///     Cursor {
    ///         head: Position { line: 2, column: 1 },
    ///         tail: Position { line: 2, column: 1 },
    ///         max_column: 1,
    ///     }
    /// );
    /// ```
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

    /// Moves this `Cursor` to the given `position`.
    /// 
    /// The `select` argument indicates whether the cursor is selecting while it moves. If `true`,
    /// only the `head` of the cursor is changed, while the `tail` remains unchanged. Otherwise, the
    /// `tail` is set to the same position as the `head`.
    /// 
    /// # Examples
    /// 
    /// ```
    /// use makepad_studio::code_editor::{Cursor};
    /// use makepad_editor_core::{Position};
    /// 
    /// let mut cursor = Cursor {
    ///     head: Position::origin(),
    ///     tail: Position::origin(),
    ///     max_column: 0,
    /// };
    /// cursor.move_to(Position { line: 1, column: 1 }, true);
    /// assert_eq!(
    ///     cursor,
    ///     Cursor {
    ///         head: Position { line: 1, column: 1 },
    ///         tail: Position::origin(),
    ///         max_column: 1,
    ///     }
    /// );
    /// ```
    pub fn move_to(&mut self, position: Position, select: bool) {
        self.head = position;
        if !select {
            self.tail = position;
        }
        self.max_column = position.column;
    }

    /// Applies the given `delta` to this `Cursor`.
    /// 
    /// When a change is made to the text on which this cursor operates that did not originate at
    /// this cursor, the position of the cursor is shifted as a result of this change. Applying a
    /// delta to a cursor shifts its position to accomodate the corresponding change.
    /// 
    /// # Examples
    /// 
    /// ```
    /// use makepad_studio::code_editor::{Cursor};
    /// use makepad_editor_core::{delta, Delta, Position, Size, Text};
    /// 
    /// let mut cursor = Cursor {
    ///     head: Position { line: 1, column: 1 },
    ///     tail: Position { line: 2, column: 2 },
    ///     max_column: 1,
    /// };
    /// let mut builder = delta::Builder::new();
    /// builder.retain(Size { line: 1, column: 1 });
    /// builder.insert(Text::from("abc\ndef"));
    /// let delta = builder.build();
    /// cursor.apply_delta(&delta);
    /// assert_eq!(
    ///     cursor,
    ///     Cursor {
    ///         head: Position { line: 1, column: 1 },
    ///         tail: Position { line: 3, column: 2 },
    ///         max_column: 1,
    ///     }
    /// );
    /// ```
    /// 
    pub fn apply_delta(&mut self, delta: &Delta) {
        self.head = self.head.apply_delta(&delta);
        self.tail = self.tail.apply_delta(&delta);
        self.max_column = self.head.column;
    }

    /// Shifts this `Cursor` forward by the given `offset`.
    pub fn apply_offset(&mut self, offset: Size) {
        self.head += offset;
        self.tail = self.head;
        self.max_column = self.head.column;
    }
}

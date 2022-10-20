use {
    crate::{
        makepad_editor_core::{
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

/// A type for representing a set of non-overlapping `Cursor`s.
///
/// This is used to implement multiple cursor support in the editor. The cursors are stored in a
/// list, and the list is sorted by the start position of each cursor. The last inserted cursor is
/// special (because it determines the scroll position in the document), so we also remember its
/// index in the list. A `CursorSet` always contains at least one cursor.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct CursorSet {
    cursors: Vec<Cursor>,
    last_inserted_index: usize,
}

impl CursorSet {
    /// Creates a `CursorSet` with a single cursor at the start of the text.
    ///
    /// # Examples
    ///
    /// ```
    /// use makepad_studio::code_editor::CursorSet;
    ///
    /// let cursors = CursorSet::new();
    /// ```
    pub fn new() -> CursorSet {
        CursorSet {
            cursors: vec![Cursor::new()],
            last_inserted_index: 0,
        }
    }

    /// Returns the number of cursors in this `CursorSet`.
    ///
    /// This is always at least 1.
    ///
    /// # Examples
    ///
    /// ```
    /// use makepad_studio::code_editor::CursorSet;
    ///
    /// let cursors = CursorSet::new();
    /// assert_eq!(cursors.len(), 1);
    /// ```
    pub fn len(&self) -> usize {
        self.cursors.len()
    }

    /// Returns an iterator over the cursors in this `CursorSet`.
    ///
    /// # Examples
    ///
    /// ```
    /// use makepad_studio::code_editor::{Cursor, CursorSet};
    /// use makepad_editor_core::{Position};
    ///
    /// let mut cursors = CursorSet::new();
    /// cursors.add(Position { line: 1, column: 1 });
    /// let mut iter = cursors.iter();
    /// assert_eq!(iter.next(), Some(&Cursor::new()));
    /// assert_eq!(
    ///     iter.next(),
    ///     Some(
    ///         &Cursor {
    ///             head: Position { line: 1, column: 1 },
    ///             tail: Position { line: 1, column: 1 },
    ///             max_column: 1,
    ///         }
    ///     )
    /// );
    /// assert_eq!(iter.next(), None);
    /// ```
    pub fn iter(&self) -> Iter<'_> {
        Iter {
            iter: self.cursors.iter(),
        }
    }

    /// Returns the the last inserted cursor in this `CursorSet`.
    ///
    /// # Examples
    ///
    /// ```
    /// use makepad_studio::code_editor::{Cursor, CursorSet};
    /// use makepad_editor_core::{Position};
    ///
    /// let mut cursors = CursorSet::new();
    /// cursors.add(Position { line: 1, column: 1 });
    /// assert_eq!(
    ///     cursors.last_inserted(),
    ///     &Cursor {
    ///         head: Position { line: 1, column: 1 },
    ///         tail: Position { line: 1, column: 1 },
    ///         max_column: 1,
    ///     }
    /// );
    /// ```
    pub fn last_inserted(&self) -> &Cursor {
        &self.cursors[self.last_inserted_index]
    }

    /// Returns the minimal set of non-overlapping ranges that covers the selections of all cursors
    /// in this `CursorSet`.
    ///
    /// This is used during rendering, where we perform a linear scan over the text to determine
    /// where each selection should be drawn.
    ///
    /// # Examples
    ///
    /// ```
    /// use makepad_studio::code_editor::{CursorSet};
    /// use makepad_editor_core::{range_set::Span, Position, Size, Text};
    ///
    /// let mut cursors = CursorSet::new();
    /// cursors.add(Position { line: 1, column: 1 });
    /// let text = Text::from("abc\ndef");
    /// cursors.move_right(&text, true);
    /// let selections = cursors.selections();
    /// let mut spans = selections.spans();
    /// assert_eq!(
    ///     spans.next(),
    ///     Some(Span { len: Size { line: 0, column: 0 }, is_included: false })
    /// );
    /// assert_eq!(
    ///     spans.next(),
    ///     Some(Span { len: Size { line: 0, column: 1 }, is_included: true })
    /// );
    /// assert_eq!(
    ///     spans.next(),
    ///     Some(Span { len: Size { line: 1, column: 1 }, is_included: false })
    /// );
    /// assert_eq!(
    ///     spans.next(),
    ///     Some(Span { len: Size { line: 0, column: 1 }, is_included: true })
    /// );
    /// ```
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

    /// Returns the minimal set of positions that covers the carets of all cursors in this
    /// `CursorSet`.
    ///
    ///
    /// This is used during rendering, where we perform a linear scan over the text to determine
    /// where each caret should be drawn.
    ///
    /// # Examples
    ///
    /// ```
    /// use makepad_studio::code_editor::{CursorSet};
    /// use makepad_editor_core::{Position, Size};
    ///
    /// let mut cursors = CursorSet::new();
    /// cursors.add(Position { line: 1, column: 1 });
    /// let carets = cursors.carets();
    /// let mut distances = carets.distances();
    /// assert_eq!(distances.next(), Some(Size { line: 0, column: 0 }));
    /// assert_eq!(distances.next(), Some(Size { line: 1, column: 1 }));
    /// assert_eq!(distances.next(), None);
    /// ```
    pub fn carets(&self) -> PositionSet {
        let mut builder = position_set::Builder::new();
        for cursor in &self.cursors {
            builder.insert(cursor.head);
        }
        builder.build()
    }

    /// Replaces this `CursorSet` with a single cursor, such that the caret is at the start of the
    /// given `text` and the selection covers the entire given `text`.
    ///
    /// # Examples
    ///
    /// ```
    /// use makepad_studio::code_editor::{Cursor, CursorSet};
    /// use makepad_editor_core::{Position, Text};
    ///
    /// let mut cursors = CursorSet::new();
    /// let text = Text::from("abc\ndef");
    /// cursors.select_all(&text);
    /// let mut iter = cursors.iter();
    /// assert_eq!(
    ///     iter.next(),
    ///     Some(
    ///         &Cursor {
    ///             head: Position { line: 0, column: 0 },
    ///             tail: Position { line: 1, column: 3 },
    ///             max_column: 0,
    ///         }
    ///     )
    /// );
    /// assert_eq!(iter.next(), None);
    /// ```
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

    /// Adds a cursor to this `CursorSet`, with the caret at the given `position` and an empty
    /// selection.
    ///
    /// # Examples
    ///
    /// ```
    /// use makepad_studio::code_editor::{Cursor, CursorSet};
    /// use makepad_editor_core::{Position};
    ///
    /// let mut cursors = CursorSet::new();
    /// cursors.add(Position { line: 1, column: 1 });
    /// let mut iter = cursors.iter();
    /// assert_eq!(iter.next(), Some(&Cursor::new()));
    /// assert_eq!(
    ///     iter.next(),
    ///     Some(
    ///         &Cursor {
    ///             head: Position { line: 1, column: 1 },
    ///             tail: Position { line: 1, column: 1 },
    ///             max_column: 1,
    ///         }
    ///     )
    /// );
    /// assert_eq!(iter.next(), None);
    /// ```
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

    /// Move all cursors in this `CursorSet` one column to the left.
    pub fn move_left(&mut self, text: &Text, select: bool) {
        for cursor in &mut self.cursors {
            cursor.move_left(text, select);
        }
        self.normalize();
    }

    /// Move all cursors in this `CursorSet` one column to the right.
    pub fn move_right(&mut self, text: &Text, select: bool) {
        for cursor in &mut self.cursors {
            cursor.move_right(text, select);
        }
        self.normalize();
    }

    /// Move all cursors in this `CursorSet` one line up.
    pub fn move_up(&mut self, text: &Text, select: bool) {
        for cursor in &mut self.cursors {
            cursor.move_up(text, select);
        }
        self.normalize();
    }

    /// Move all cursors in this `CursorSet` one line down.
    pub fn move_down(&mut self, text: &Text, select: bool) {
        for cursor in &mut self.cursors {
            cursor.move_down(text, select);
        }
        self.normalize();
    }

    /// Move all cursors in this `CursorSet` to the given `position`.
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
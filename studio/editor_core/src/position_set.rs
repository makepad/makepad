use {
    crate::{
        position::Position,
        size::Size
    },
    std::{ops::Deref, slice::Iter},
};

/// A set of positions.
/// 
/// This type is useful if you have collection of positions, and you want to find all unique
/// positions. A new position set can be created in `O(n log n)` time via the `Builder` type. Once
/// created, the position set is immutable.
#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub struct PositionSet {
    positions: Vec<Position>,
}

impl PositionSet {
    /// Creates an empty set of positions
    /// 
    /// # Examples
    /// 
    /// ```
    /// use makepad_editor_core::position_set;
    /// 
    /// let positions = position_set::PositionSet::new();
    /// ```
    pub fn new() -> PositionSet {
        PositionSet::default()
    }

    /// Returns `true` if this set is empty.
    /// 
    /// # Examples
    /// 
    /// ```
    /// use makepad_editor_core::{position_set, Position, PositionSet};
    /// 
    /// let positions = PositionSet::new();
    /// assert!(positions.is_empty());
    /// let mut builder = position_set::Builder::new();
    /// builder.insert(Position::origin());
    /// let positions = builder.build();
    /// assert!(!positions.is_empty());
    /// ```
    pub fn is_empty(&self) -> bool {
        self.positions.is_empty()
    }

    /// Returns the number of positions in this set.
    /// 
    /// # Examples
    /// 
    /// ```
    /// use makepad_editor_core::{position_set, Position, PositionSet};
    /// 
    /// let mut builder = position_set::Builder::new();
    /// builder.insert(Position { line: 1, column: 1 });
    /// builder.insert(Position { line: 2, column: 2 });
    /// builder.insert(Position { line: 2, column: 2 });
    /// let positions = builder.build();
    /// assert!(positions.len() == 2);
    /// ```
    pub fn len(&self) -> usize {
        self.positions.len()
    }
    
    /// Returns an iterator over the distances between each position in this set.
    /// 
    /// Iterating over the distances between each position, rather than the positions themselves, is
    /// often useful because the editor renders things such as carets by performing a downward sweep
    /// over the lines of a document while maintaining a current position. During each step of the
    /// sweep, the current position is incremented with the distance to the position of the next
    /// caret.
    /// 
    /// # Examples
    /// 
    /// ```
    /// use makepad_editor_core::{position_set, Position, Size};
    /// 
    /// let mut builder = position_set::Builder::new();
    /// builder.insert(Position { line: 1, column: 2 });
    /// builder.insert(Position { line: 2, column: 1 });
    /// let positions = builder.build();
    /// let mut distances = positions.distances();
    /// assert_eq!(distances.next(), Some(Size { line: 1, column: 2 }));
    /// assert_eq!(distances.next(), Some(Size { line: 1, column: 1 }));
    /// assert_eq!(distances.next(), None);
    /// ```
    pub fn distances(&self) -> Distances {
        Distances {
            next_position_iter: self.positions.iter(),
            position: Position::origin(),
        }
    }
}

impl Deref for PositionSet {
    type Target = [Position];
    
    fn deref(&self) -> &Self::Target {
        &self.positions
    }
}

impl<'a> IntoIterator for &'a PositionSet {
    type Item = &'a Position;
    type IntoIter = Iter<'a, Position>;
    
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

/// A builder for sets of positions.
#[derive(Default, Debug)]
pub struct Builder {
    positions: Vec<Position>,
}

impl Builder {
    /// Creates a builder for a set of positions.
    /// 
    /// # Examples
    /// 
    /// ```
    /// use makepad_editor_core::position_set::Builder;
    /// 
    /// let builder = Builder::new();
    /// ```
    pub fn new() -> Builder {
        Builder::default()
    }
    
    /// Adds the given position to the set under construction.
    /// 
    /// This method takes `O(1)` time.
    /// 
    /// # Examples
    /// 
    /// ```
    /// use makepad_editor_core::{position_set::Builder, Position};
    /// 
    /// let mut builder = Builder::new();
    /// builder.insert(Position { line: 1, column: 1 });
    /// ```
    pub fn insert(&mut self, position: Position) {
        self.positions.push(position);
    }

    /// Finishes the set under construction.
    /// 
    /// This method takes `O(n log n)` time.
    /// 
    /// # Examples
    /// 
    /// ```
    /// use makepad_editor_core::{position_set::Builder};
    /// 
    /// let mut builder = Builder::new();
    /// let positions = builder.build();
    /// ```
    pub fn build(mut self) -> PositionSet {
        self.positions.sort();
        self.positions.dedup();
        PositionSet {
            positions: self.positions,
        }
    }
}

/// An iterator over the distances between each position in a set.
/// 
/// This struct is created by the `distances` method on `PositionSet`. See its documentation for
/// more.
#[derive(Debug)]
pub struct Distances<'a> {
    next_position_iter: Iter<'a, Position>,
    position: Position,
}

impl<'a> Iterator for Distances<'a> {
    type Item = Size;
    
    fn next(&mut self) -> Option<Self::Item> {
        let next_position = *self.next_position_iter.next() ?;
        let distance = next_position - self.position;
        self.position = next_position;
        Some(distance)
    }
}

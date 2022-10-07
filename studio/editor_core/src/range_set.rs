use {
    crate::{
        position::Position,
        range::Range,
        size::Size
    },
    std::{
        collections::{btree_map::Entry, BTreeMap},
        slice::Iter,
    },
};

/// A set of non-overlapping ranges.
///
/// This type is useful if you have a collection of overlapping ranges, and you want to find the
/// minimal set of non-overlapping ranges that covers all ranges in the original collection. A new
/// range set can be created in `O(n log n)` time via the `Builder` type. Once created, the range
/// set is immutable.
#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub struct RangeSet {
    // We store the boundaries of each range. That is, every odd position is the start of a range,
    // and every even position is the end of a range. Note that ranges can never be adjacent to
    // each other.
    positions: Vec<Position>,
}

impl RangeSet {
    /// Creates an empty set of non-overlapping ranges.
    /// 
    /// # Examples
    /// 
    /// ```
    /// use makepad_editor_core::RangeSet;
    /// 
    /// let ranges = RangeSet::new();
    /// ```
    pub fn new() -> RangeSet {
        RangeSet::default()
    }
    
    /// Returns `true` if this set is empty.
    /// 
    /// # Examples
    /// 
    /// ```
    /// use makepad_editor_core::{range_set, Position, Range, RangeSet};
    /// 
    /// let ranges = RangeSet::new();
    /// assert!(ranges.is_empty());
    /// let mut builder = range_set::Builder::new();
    /// builder.include(Range {
    ///     start: Position { line: 1, column: 1 },
    ///     end: Position { line: 2, column: 2 },
    /// });
    /// let ranges = builder.build();
    /// assert!(!ranges.is_empty());
    /// ```
    pub fn is_empty(&self) -> bool {
        self.positions.is_empty()
    }

    /// Returns the number of ranges in this set.
    /// 
    /// # Examples
    /// 
    /// ```
    /// use makepad_editor_core::{range_set, Position, Range, RangeSet};
    /// let mut builder = range_set::Builder::new();
    /// builder.include(Range {
    ///     start: Position { line: 1, column: 1 },
    ///     end: Position { line: 2, column: 2 },
    /// });
    /// let ranges = builder.build();
    /// assert_eq!(ranges.len(), 1);
    ///
    pub fn len(&self) -> usize {
        self.positions.len() / 2
    }

    /// Returns `true` is this set contains a range that contains the given position.
    /// 
    /// # Examples
    ///
    /// ```
    /// use makepad_editor_core::{range_set, Position, Range, RangeSet};
    /// 
    /// let mut builder = range_set::Builder::new();
    /// builder.include(Range {
    ///     start: Position { line: 1, column: 1 },
    ///     end: Position { line: 2, column: 2 },
    /// });
    /// let ranges = builder.build();
    /// assert!(ranges.contains_position(Position { line: 1, column: 2 }));
    /// assert!(ranges.contains_position(Position { line: 2, column: 1 }));
    /// assert!(!ranges.contains_position(Position { line: 3, column: 0 }));
    /// ```
    pub fn contains_position(&self, position: Position) -> bool {
        match self.positions.binary_search(&position) {
            Ok(_) => false,
            Err(index) => index % 2 == 1,
        }
    }
    
    /// Returns an iterator over the spans in this set.
    /// 
    /// A set of non-overlapping ranges can be thought of as a sequence of contiguous spans, where
    /// each span is an amount of text that is either included in the set or not. This method
    /// returns an iterator over those spans.
    /// 
    /// Iterating over the spans in a set, rather than the ranges themselves, is often useful
    /// because the editor renders things such as selections by performing a downwards scan over the
    /// lines of a document while maintaining a current position. During each step of the sweep, the
    /// current position is incremented with the size of the next span.
    ///
    /// # Examples
    /// 
    /// ```
    /// use makepad_editor_core::{range_set, range_set::Span, Position, Range, RangeSet, Size};
    /// 
    /// let mut builder = range_set::Builder::new();
    /// builder.include(Range {
    ///     start: Position { line: 1, column: 1 },
    ///     end: Position { line: 2, column: 2 },
    /// });
    /// let ranges = builder.build();
    /// let mut spans = ranges.spans();
    /// assert_eq!(spans.next(), Some(Span {
    ///    len: Size { line: 1, column: 1 },
    ///    is_included: false,
    /// }));
    /// assert_eq!(spans.next(), Some(Span {
    ///    len: Size { line: 1, column: 2 },
    ///    is_included: true,
    /// }));
    /// assert_eq!(spans.next(), None);
    /// ```
    pub fn spans(&self) -> Spans {
        Spans {
            next_position_iter: self.positions.iter(),
            position: Position::origin(),
            is_included: false,
        }
    }
}

/// A builder for sets of non-overlapping ranges.
#[derive(Debug, Default)]
pub struct Builder {
    // We store a map from positions to changes in the number of ranges after that position. A
    // positive delta means one or more ranges start at this position.
    deltas_by_position: BTreeMap<Position, i32>,
}

impl Builder {
    /// Creates a builder for a set of non-overlapping ranges.
    /// 
    /// # Examples
    /// 
    /// ```
    /// use makepad_editor_core::range_set;
    /// 
    /// let builder = range_set::Builder::new();
    /// ```
    pub fn new() -> Builder {
        Builder::default()
    }
    
    /// Includes the given range in the set under construction.
    /// 
    /// This method takes `O(1)` time.
    /// 
    /// # Examples
    /// 
    /// ```
    /// use makepad_editor_core::{range_set::Builder, Position, Range};
    /// 
    /// let mut builder = Builder::new();
    /// builder.include(Range {
    ///     start: Position { line: 1, column: 1 },
    ///     end: Position { line: 2, column: 2 },
    /// });
    /// ```
    pub fn include(&mut self, range: Range) {
        match self.deltas_by_position.entry(range.start) {
            Entry::Occupied(mut entry) => {
                *entry.get_mut() += 1;
                if *entry.get() == 0 {
                    entry.remove();
                }
            }
            Entry::Vacant(entry) => {
                entry.insert(1);
            }
        }
        match self.deltas_by_position.entry(range.end) {
            Entry::Occupied(mut entry) => {
                *entry.get_mut() -= 1;
                if *entry.get() == 0 {
                    entry.remove();
                }
            }
            Entry::Vacant(entry) => {
                entry.insert(-1);
            }
        }
    }
    
    /// Finishes the set under construction.
    /// 
    /// This method takes `O(n log n)` time.
    /// 
    /// # Examples
    /// 
    /// ```
    /// use makepad_editor_core::range_set::Builder;
    ///
    /// let mut builder = Builder::new();
    /// let ranges = builder.build(); 
    /// ```
    pub fn build(&self) -> RangeSet {
        let mut positions = Vec::new();
        let mut value = 0;
        for (position, delta) in &self.deltas_by_position {
            let next_value = value + delta;
            // If there were zero ranges before this position, and one or more ranges after this
            // position, or vice versa, then this position is the boundary of a range, and we need
            // to store it in the set.
            if (value == 0) != (next_value == 0) {
                positions.push(*position);
            }
            value = next_value;
        }
        RangeSet {positions}
    }
}

/// An iterator over the spans in a set.
/// 
/// This struct is created by the `spans` method on `RangeSet`. See its documentation for more.
#[derive(Debug)]
pub struct Spans<'a> {
    next_position_iter: Iter<'a, Position>,
    position: Position,
    is_included: bool,
}

impl<'a> Iterator for Spans<'a> {
    type Item = Span;
    
    fn next(&mut self) -> Option<Self::Item> {
        let next_position = *self.next_position_iter.next() ?;
        let span = Span {
            len: next_position - self.position,
            is_included: self.is_included,
        };
        self.position = next_position;
        self.is_included = !self.is_included;
        Some(span)
    }
}

/// A span in a set of ranges.
///
/// See the `spans` method on `RangeSet` for more.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Span {
    /// The length of this span.
    pub len: Size,
    /// Is this span included in the set?
    pub is_included: bool,
}

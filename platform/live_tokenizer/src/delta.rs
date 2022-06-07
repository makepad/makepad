use {
    crate::{
        position::Position,
        range::Range,
        size::Size,
        text::Text
    },
    std::{
        cmp::Ordering,
        mem,
        ops::Deref,
        slice::Iter,
        vec::IntoIter
    },
    makepad_micro_serde::{SerBin, DeBin, DeBinErr}
};

/// A type for representing changes in a text.
/// 
/// A delta can be thought of as a recipe for changing on text into another. It consists of a
/// sequence of operations. To apply a delta to a text, create an imaginary cursor at the start of
/// the text, and then apply the operations in order. Each operation eithers move the cursor forward
/// by a given amount, effectively retaining that part of the text, or modify the text at the cursor
/// by inserting/removing a given amount of text, keeping the cursor in place.
/// 
/// A delta is always defined with respect to a given text. If another delta is applied to the text
/// first, the original delta can no longer be applied. However, it is possible to transform the
/// original delta so that it can be applied to the text after it has been modified by the other
/// delta. This is the key idea behind operational transform (OT), which is what we use to implement
/// collaboration in the editor.
#[derive(Clone, Debug, Default, Eq, Hash, PartialEq, SerBin, DeBin)]
pub struct Delta {
    operations: Vec<Operation>,
}

impl Delta {
    /// Creates a delta that does nothing when applied.
    /// 
    /// # Examples
    /// 
    /// ```
    /// use makepad_live_tokenizer::{Delta, Text};
    /// 
    /// let mut text = Text::new();
    /// let delta = Delta::identity();
    /// text.apply_delta(delta);
    /// assert_eq!(text, Text::new());
    /// ```
    pub fn identity() -> Delta {
        Delta::default()
    }
    
    /// Returns an iterator over the range of the operations in this delta, and their kind.
    /// 
    /// The range of an operation is defined as follows: for an insert operation, it is the range
    /// of the inserted text after it has been inserted. For a delete operation, it is the range
    /// of the deleted text before it was deleted. Retain operations have no associated range,
    /// since they have no effect.
    /// 
    /// Iterating over the range of each operation, rather than the operations itself, is often
    /// useful because the editor maintains a cache for different kinds of derived information
    /// about a text, and the structure of this cache matches that of the text itself. When a
    /// delta is applied to a text, the structure of the text, so the structure of the cache needs
    /// to change accordingly. That is, we need to either insert a new range into the cache, or
    /// remove an existing range from the cache.
    /// 
    /// # Examples
    /// 
    /// ```
    /// use makepad_live_tokenizer::{delta, Delta, Position, OperationRange, Range, Size, Text};
    /// 
    /// let mut builder = delta::Builder::new();
    /// builder.retain(Size { line: 1, column: 1 });
    /// builder.delete(Size { line: 2, column: 2 });
    /// builder.insert(Text::from("abc"));
    /// let delta = builder.build();
    /// 
    /// let mut operation_ranges = delta.operation_ranges();
    /// assert_eq!(operation_ranges.next(), Some(OperationRange::Delete(Range {
    ///     start: Position { line: 1, column: 1 },
    ///     end: Position { line: 3, column: 2 }
    /// })));
    /// assert_eq!(operation_ranges.next(), Some(OperationRange::Insert(Range {
    ///     start: Position { line: 1, column: 1 },
    ///     end: Position { line: 1, column: 4 }
    /// })));
    /// assert_eq!(operation_ranges.next(), None);
    /// ```
    pub fn operation_ranges(&self) -> OperationRanges<'_> {
        OperationRanges {
            position: Position::origin(),
            iter: self.operations.iter(),
        }
    }
    
    /// Returns the inverse of this delta. That is, returns a delta that, when applied to a text to
    /// which this delta has been applied, reverses the effect of applying this delta.
    /// 
    /// For efficiency, deltas does not store all the data they needs to invert itself.
    /// Consequently, we need to pass the text with respect to which this delta is defined to invert
    /// it.
    /// 
    /// # Examples
    /// 
    /// ```
    /// use makepad_live_tokenizer::{delta, Delta, Size, Text};
    /// 
    /// let mut text = Text::from("abc");
    /// 
    /// let mut builder = delta::Builder::new();
    /// builder.retain(Size { line: 0, column: 3 });
    /// builder.insert(Text::from("def"));
    /// let delta = builder.build();
    /// 
    /// let inverse_delta = delta.clone().invert(&text);
    /// 
    /// text.apply_delta(delta);
    /// assert_eq!(text, Text::from("abcdef"));
    /// text.apply_delta(inverse_delta);
    /// assert_eq!(text, Text::from("abc"));
    /// ```
    pub fn invert(self, text: &Text) -> Delta {
        let mut builder = Builder::new();
        let mut position = Position::origin();
        for operation in self.operations {
            match operation {
                Operation::Retain(count) => {
                    builder.retain(count);
                    position += count;
                }
                Operation::Insert(text) => {
                    builder.delete(text.len());
                }
                Operation::Delete(count) => {
                    let new_position = position + count;
                    builder.insert(text.copy(Range {
                        start: position,
                        end: new_position,
                    }));
                    position = new_position;
                }
            }
        }
        builder.build()
    }
    
    /// Returns the composite of this delta and the given delta. That is, returns a delta that, when
    /// applied to a text, has the same effect as first applying this delta to the text, and then
    /// applying the given delta to the text.
    /// 
    /// # Examples
    /// 
    /// ```
    /// use makepad_live_tokenizer::{delta, Delta, Size, Text};
    /// let mut text = Text::from("abc");
    /// 
    /// let mut builder = delta::Builder::new();
    /// builder.retain(Size { line: 0, column: 3 });
    /// builder.insert(Text::from("def"));
    /// let delta_0 = builder.build();
    /// 
    /// let mut builder = delta::Builder::new();
    /// builder.retain(Size { line: 0, column: 6 });
    /// builder.insert(Text::from("ghi"));
    /// let delta_1 = builder.build();
    /// 
    /// let composite_delta = delta_0.compose(delta_1);
    /// 
    /// text.apply_delta(composite_delta);
    /// assert_eq!(text, Text::from("abcdefghi"));
    /// ```
    pub fn compose(self, other: Delta) -> Delta {
        let mut builder = Builder::new();
        let mut operation_iter_0 = self.operations.into_iter();
        let mut operation_iter_1 = other.operations.into_iter();
        let mut operation_slot_0 = operation_iter_0.next();
        let mut operation_slot_1 = operation_iter_1.next();
        loop {
            match (operation_slot_0, operation_slot_1) {
                (Some(Operation::Retain(count_0)), Some(Operation::Retain(count_1))) => {
                    match count_0.cmp(&count_1) {
                        Ordering::Less => {
                            builder.retain(count_0);
                            operation_slot_0 = operation_iter_0.next();
                            operation_slot_1 = Some(Operation::Retain(count_1 - count_0));
                        }
                        Ordering::Equal => {
                            builder.retain(count_0);
                            operation_slot_0 = operation_iter_0.next();
                            operation_slot_1 = operation_iter_1.next();
                        }
                        Ordering::Greater => {
                            builder.retain(count_1);
                            operation_slot_0 = Some(Operation::Retain(count_0 - count_1));
                            operation_slot_1 = operation_iter_1.next();
                        }
                    }
                }
                (Some(Operation::Retain(count_0)), Some(Operation::Delete(count_1))) => {
                    match count_0.cmp(&count_1) {
                        Ordering::Less => {
                            builder.delete(count_0);
                            operation_slot_0 = operation_iter_0.next();
                            operation_slot_1 = Some(Operation::Delete(count_1 - count_0));
                        }
                        Ordering::Equal => {
                            builder.delete(count_0);
                            operation_slot_0 = operation_iter_0.next();
                            operation_slot_1 = operation_iter_1.next();
                        }
                        Ordering::Greater => {
                            builder.delete(count_1);
                            operation_slot_0 = Some(Operation::Retain(count_0 - count_1));
                            operation_slot_1 = operation_iter_1.next();
                        }
                    }
                }
                (Some(Operation::Insert(mut text)), Some(Operation::Retain(count))) => {
                    let len = text.len();
                    match len.cmp(&count) {
                        Ordering::Less => {
                            builder.insert(text);
                            operation_slot_0 = operation_iter_0.next();
                            operation_slot_1 = Some(Operation::Retain(count - len));
                        }
                        Ordering::Equal => {
                            builder.insert(text);
                            operation_slot_0 = operation_iter_0.next();
                            operation_slot_1 = operation_iter_1.next();
                        }
                        Ordering::Greater => {
                            builder.insert(text.take(count));
                            operation_slot_0 = Some(Operation::Insert(text));
                            operation_slot_1 = operation_iter_1.next();
                        }
                    }
                }
                (Some(Operation::Insert(mut text)), Some(Operation::Delete(count))) => {
                    match text.len().cmp(&count) {
                        Ordering::Less => {
                            operation_slot_0 = operation_iter_0.next();
                            operation_slot_1 = Some(Operation::Delete(count - text.len()));
                        }
                        Ordering::Equal => {
                            operation_slot_0 = operation_iter_0.next();
                            operation_slot_1 = operation_iter_1.next();
                        }
                        Ordering::Greater => {
                            text.skip(count);
                            operation_slot_0 = Some(Operation::Insert(text));
                            operation_slot_1 = operation_iter_1.next();
                        }
                    }
                }
                (Some(Operation::Insert(text)), None) => {
                    builder.insert(text);
                    operation_slot_0 = operation_iter_0.next();
                    operation_slot_1 = None;
                }
                (Some(Operation::Retain(count)), None) => {
                    builder.retain(count);
                    operation_slot_0 = operation_iter_0.next();
                    operation_slot_1 = None;
                }
                (Some(Operation::Delete(count)), operation) => {
                    builder.delete(count);
                    operation_slot_0 = operation_iter_0.next();
                    operation_slot_1 = operation;
                }
                (None, Some(Operation::Retain(count))) => {
                    builder.retain(count);
                    operation_slot_0 = None;
                    operation_slot_1 = operation_iter_1.next();
                }
                (None, Some(Operation::Delete(count))) => {
                    builder.delete(count);
                    operation_slot_0 = None;
                    operation_slot_1 = operation_iter_1.next();
                }
                (None, None) => break,
                (operation, Some(Operation::Insert(text))) => {
                    builder.insert(text);
                    operation_slot_0 = operation;
                    operation_slot_1 = operation_iter_1.next();
                }
            }
        }
        builder.build()
    }
    
    /// This is the operational transform function that forms the heart of operational transform
    /// (OT). Given a pair of deltas (A, B) that were defined with respect to the same text, it
    /// returns a new pair of deltas (A', B'), such that applying A' after B has the same effect
    /// as applying B' after A.
    /// 
    /// # Examples
    /// 
    /// ```
    /// ```
    pub fn transform(self, other: Delta) -> (Delta, Delta) {
        let mut builder_0 = Builder::new();
        let mut builder_1 = Builder::new();
        let mut operation_iter_0 = self.operations.into_iter();
        let mut operation_iter_1 = other.operations.into_iter();
        let mut operation_slot_0 = operation_iter_0.next();
        let mut operation_slot_1 = operation_iter_1.next();
        loop {
            match (operation_slot_0, operation_slot_1) {
                (Some(Operation::Retain(count_0)), Some(Operation::Retain(count_1))) => {
                    match count_0.cmp(&count_1) {
                        Ordering::Less => {
                            builder_0.retain(count_0);
                            builder_1.retain(count_0);
                            operation_slot_0 = operation_iter_0.next();
                            operation_slot_1 = Some(Operation::Retain(count_1 - count_0));
                        }
                        Ordering::Equal => {
                            builder_0.retain(count_0);
                            builder_1.retain(count_0);
                            operation_slot_0 = operation_iter_0.next();
                            operation_slot_1 = operation_iter_1.next();
                        }
                        Ordering::Greater => {
                            builder_0.retain(count_1);
                            builder_1.retain(count_1);
                            operation_slot_0 = Some(Operation::Retain(count_0 - count_1));
                            operation_slot_1 = operation_iter_1.next();
                        }
                    }
                }
                (Some(Operation::Retain(count_0)), Some(Operation::Delete(count_1))) => {
                    match count_0.cmp(&count_1) {
                        Ordering::Less => {
                            builder_1.delete(count_0);
                            operation_slot_0 = operation_iter_0.next();
                            operation_slot_1 = Some(Operation::Delete(count_1 - count_0));
                        }
                        Ordering::Equal => {
                            builder_1.delete(count_0);
                            operation_slot_0 = operation_iter_0.next();
                            operation_slot_1 = operation_iter_1.next();
                        }
                        Ordering::Greater => {
                            builder_1.delete(count_1);
                            operation_slot_0 = Some(Operation::Retain(count_0 - count_1));
                            operation_slot_1 = operation_iter_1.next();
                        }
                    }
                }
                (Some(Operation::Retain(count)), None) => {
                    builder_0.retain(count);
                    builder_1.retain(count);
                    operation_slot_0 = operation_iter_0.next();
                    operation_slot_1 = None;
                }
                (Some(Operation::Insert(text)), operation) => {
                    let len = text.len();
                    builder_0.insert(text);
                    builder_1.retain(len);
                    operation_slot_0 = operation_iter_0.next();
                    operation_slot_1 = operation;
                }
                (Some(Operation::Delete(count_0)), Some(Operation::Retain(count_1))) => {
                    match count_0.cmp(&count_1) {
                        Ordering::Less => {
                            builder_0.delete(count_0);
                            operation_slot_0 = operation_iter_0.next();
                            operation_slot_1 = Some(Operation::Retain(count_1 - count_0));
                        }
                        Ordering::Equal => {
                            builder_0.delete(count_0);
                            operation_slot_0 = operation_iter_0.next();
                            operation_slot_1 = operation_iter_1.next();
                        }
                        Ordering::Greater => {
                            builder_0.delete(count_1);
                            operation_slot_0 = Some(Operation::Delete(count_0 - count_1));
                            operation_slot_1 = operation_iter_1.next();
                        }
                    }
                }
                (Some(Operation::Delete(count_0)), Some(Operation::Delete(count_1))) => {
                    match count_0.cmp(&count_1) {
                        Ordering::Less => {
                            operation_slot_0 = operation_iter_0.next();
                            operation_slot_1 = Some(Operation::Delete(count_1 - count_0));
                        }
                        Ordering::Equal => {
                            operation_slot_0 = operation_iter_0.next();
                            operation_slot_1 = operation_iter_1.next();
                        }
                        Ordering::Greater => {
                            operation_slot_0 = Some(Operation::Delete(count_0 - count_1));
                            operation_slot_1 = operation_iter_1.next();
                        }
                    }
                }
                (Some(Operation::Delete(count)), None) => {
                    builder_0.delete(count);
                    operation_slot_0 = operation_iter_0.next();
                    operation_slot_1 = None;
                }
                (None, Some(Operation::Retain(count))) => {
                    builder_0.retain(count);
                    builder_1.retain(count);
                    operation_slot_0 = None;
                    operation_slot_1 = operation_iter_1.next();
                }
                (None, Some(Operation::Delete(count))) => {
                    builder_1.delete(count);
                    operation_slot_0 = None;
                    operation_slot_1 = operation_iter_1.next();
                }
                (None, None) => break,
                (operation, Some(Operation::Insert(text))) => {
                    builder_0.retain(text.len());
                    builder_1.insert(text);
                    operation_slot_0 = operation;
                    operation_slot_1 = operation_iter_1.next();
                }
            }
        }
        (builder_0.build(), builder_1.build())
    }
}

impl Deref for Delta {
    type Target = [Operation];
    
    fn deref(&self) -> &Self::Target {
        &self.operations
    }
}

impl<'a> IntoIterator for &'a Delta {
    type Item = &'a Operation;
    type IntoIter = Iter<'a, Operation>;
    
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl IntoIterator for Delta {
    type Item = Operation;
    type IntoIter = IntoIter<Operation>;
    
    fn into_iter(self) -> Self::IntoIter {
        self.operations.into_iter()
    }
}

pub struct OperationRanges<'a> {
    position: Position,
    iter: Iter<'a, Operation>,
}

impl<'a> Iterator for OperationRanges<'a> {
    type Item = OperationRange;
    
    fn next(&mut self) -> Option<OperationRange> {
        loop {
            match self.iter.next() ? .span() {
                OperationSpan::Retain(count) => {
                    self.position += count;
                }
                OperationSpan::Insert(count) => {
                    let start = self.position;
                    self.position += count;
                    break Some(OperationRange::Insert(Range {
                        start,
                        end: self.position,
                    }));
                }
                OperationSpan::Delete(count) => {
                    break Some(OperationRange::Delete(Range {
                        start: self.position,
                        end: self.position + count,
                    }));
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum OperationRange {
    Insert(Range),
    Delete(Range),
}

#[derive(Debug, Default)]
pub struct Builder {
    operations: Vec<Operation>,
}

impl Builder {
    pub fn new() -> Builder {
        Builder::default()
    }
    
    pub fn retain(&mut self, count: Size) {
        if count.is_zero() {
            return;
        }
        match self.operations.last_mut() {
            Some(Operation::Retain(last_count)) => {
                *last_count += count;
            }
            _ => self.operations.push(Operation::Retain(count)),
        }
    }
    
    pub fn insert(&mut self, text: Text) {
        if text.is_empty() {
            return;
        }
        match self.operations.as_mut_slice() {
            [.., Operation::Insert(last_text)] => {
                *last_text += text;
            }
            _ => self.operations.push(Operation::Insert(text)),
        }
    }
    
    pub fn delete(&mut self, count: Size) {
        if count.is_zero() {
            return;
        }
        match self.operations.as_mut_slice() {
            [.., Operation::Delete(last_count)] => {
                *last_count += count;
            }
            [.., Operation::Delete(second_to_last_count), Operation::Insert(_)] => {
                *second_to_last_count += count;
            }
            [.., last_operation @ Operation::Insert(_)] => {
                let operation = mem::replace(last_operation, Operation::Delete(count));
                self.operations.push(operation);
            }
            _ => self.operations.push(Operation::Delete(count)),
        }
    }
    
    pub fn build(mut self) -> Delta {
        if let Some(Operation::Retain(_)) = self.operations.last() {
            self.operations.pop();
        }
        Delta {
            operations: self.operations,
        }
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq, SerBin, DeBin)]
pub enum Operation {
    Retain(Size),
    Insert(Text),
    Delete(Size),
}

impl Operation {
    pub fn span(&self) -> OperationSpan {
        match self {
            Operation::Retain(count) => OperationSpan::Retain(*count),
            Operation::Insert(text) => OperationSpan::Insert(text.len()),
            Operation::Delete(count) => OperationSpan::Delete(*count),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum OperationSpan {
    Retain(Size),
    Insert(Size),
    Delete(Size),
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Whose {
    Ours,
    Theirs,
}

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

#[derive(Clone, Debug, Default, Eq, Hash, PartialEq, SerBin, DeBin)]
pub struct Delta {
    operations: Vec<Operation>,
}

impl Delta {
    pub fn identity() -> Delta {
        Delta::default()
    }
    
    pub fn operation_ranges(&self) -> OperationRanges<'_> {
        OperationRanges {
            position: Position::origin(),
            iter: self.operations.iter(),
        }
    }
    
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

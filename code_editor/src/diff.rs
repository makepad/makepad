use {
    crate::{Length, Text},
    std::{slice, vec},
};

#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub struct Diff {
    operations: Vec<Operation>,
}

impl Diff {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.operations.is_empty()
    }

    pub fn len(&self) -> usize {
        self.operations.len()
    }

    pub fn iter(&self) -> Iter<'_> {
        Iter {
            iter: self.operations.iter(),
        }
    }

    pub fn compose(self, other: Self) -> Self {
        use std::cmp::Ordering;

        let mut builder = Builder::new();
        let mut operations_0 = self.operations.into_iter();
        let mut operations_1 = other.operations.into_iter();
        let mut operation_slot_0 = operations_0.next();
        let mut operation_slot_1 = operations_1.next();
        loop {
            match (operation_slot_0, operation_slot_1) {
                (Some(Operation::Retain(length_0)), Some(Operation::Retain(length_1))) => {
                    match length_0.cmp(&length_1) {
                        Ordering::Less => {
                            builder.retain(length_0);
                            operation_slot_0 = operations_0.next();
                            operation_slot_1 = Some(Operation::Retain(length_1 - length_0));
                        }
                        Ordering::Equal => {
                            builder.retain(length_0);
                            operation_slot_0 = operations_0.next();
                            operation_slot_1 = operations_1.next();
                        }
                        Ordering::Greater => {
                            builder.retain(length_1);
                            operation_slot_0 = Some(Operation::Retain(length_0 - length_1));
                            operation_slot_1 = operations_1.next();
                        }
                    }
                }
                (Some(Operation::Retain(length_0)), Some(Operation::Delete(length_1))) => {
                    match length_0.cmp(&length_1) {
                        Ordering::Less => {
                            builder.delete(length_0);
                            operation_slot_0 = operations_0.next();
                            operation_slot_1 = Some(Operation::Delete(length_1 - length_0));
                        }
                        Ordering::Equal => {
                            builder.delete(length_0);
                            operation_slot_0 = operations_0.next();
                            operation_slot_1 = operations_1.next();
                        }
                        Ordering::Greater => {
                            builder.delete(length_1);
                            operation_slot_0 = Some(Operation::Retain(length_0 - length_1));
                            operation_slot_1 = operations_1.next();
                        }
                    }
                }
                (Some(Operation::Insert(mut text)), Some(Operation::Retain(length))) => {
                    match text.length().cmp(&length) {
                        Ordering::Less => {
                            let text_length = text.length();
                            builder.insert(text);
                            operation_slot_0 = operations_0.next();
                            operation_slot_1 = Some(Operation::Retain(length - text_length));
                        }
                        Ordering::Equal => {
                            builder.insert(text);
                            operation_slot_0 = operations_0.next();
                            operation_slot_1 = operations_1.next();
                        }
                        Ordering::Greater => {
                            builder.insert(text.take(length));
                            operation_slot_0 = Some(Operation::Insert(text));
                            operation_slot_1 = operations_1.next();
                        }
                    }
                }
                (Some(Operation::Insert(mut text)), Some(Operation::Delete(length))) => {
                    match text.length().cmp(&length) {
                        Ordering::Less => {
                            operation_slot_0 = operations_0.next();
                            operation_slot_1 = Some(Operation::Delete(text.length() - length));
                        }
                        Ordering::Equal => {
                            operation_slot_0 = operations_0.next();
                            operation_slot_1 = operations_1.next();
                        }
                        Ordering::Greater => {
                            text.skip(length);
                            operation_slot_0 = Some(Operation::Insert(text));
                            operation_slot_1 = operations_1.next();
                        }
                    }
                }
                (Some(Operation::Insert(text)), None) => {
                    builder.insert(text);
                    operation_slot_0 = operations_0.next();
                    operation_slot_1 = None;
                }
                (Some(Operation::Retain(len)), None) => {
                    builder.retain(len);
                    operation_slot_0 = operations_0.next();
                    operation_slot_1 = None;
                }
                (Some(Operation::Delete(len)), op) => {
                    builder.delete(len);
                    operation_slot_0 = operations_0.next();
                    operation_slot_1 = op;
                }
                (None, Some(Operation::Retain(len))) => {
                    builder.retain(len);
                    operation_slot_0 = None;
                    operation_slot_1 = operations_1.next();
                }
                (None, Some(Operation::Delete(len))) => {
                    builder.delete(len);
                    operation_slot_0 = None;
                    operation_slot_1 = operations_1.next();
                }
                (None, None) => break,
                (op, Some(Operation::Insert(text))) => {
                    builder.insert(text);
                    operation_slot_0 = op;
                    operation_slot_1 = operations_1.next();
                }
            }
        }
        builder.finish()
    }
}

impl<'a> IntoIterator for &'a Diff {
    type Item = &'a Operation;
    type IntoIter = Iter<'a>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl IntoIterator for Diff {
    type Item = Operation;
    type IntoIter = IntoIter;

    fn into_iter(self) -> Self::IntoIter {
        IntoIter {
            iter: self.operations.into_iter(),
        }
    }
}

#[derive(Debug, Default)]
pub struct Builder {
    operations: Vec<Operation>,
}

impl Builder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn delete(&mut self, length: Length) {
        use std::mem;

        if length == Length::default() {
            return;
        }
        match self.operations.as_mut_slice() {
            [.., Operation::Delete(last_length)] => {
                *last_length += length;
            }
            [.., Operation::Delete(second_last_length), Operation::Insert(_)] => {
                *second_last_length += length;
            }
            [.., last_operation @ Operation::Insert(_)] => {
                let operation = mem::replace(last_operation, Operation::Delete(length));
                self.operations.push(operation);
            }
            _ => self.operations.push(Operation::Delete(length)),
        }
    }

    pub fn retain(&mut self, length: Length) {
        if length == Length::default() {
            return;
        }
        match self.operations.last_mut() {
            Some(Operation::Retain(last_length)) => {
                *last_length += length;
            }
            _ => self.operations.push(Operation::Retain(length)),
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

    pub fn finish(mut self) -> Diff {
        if let Some(Operation::Retain(_)) = self.operations.last() {
            self.operations.pop();
        }
        Diff {
            operations: self.operations,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Iter<'a> {
    iter: slice::Iter<'a, Operation>,
}

impl<'a> Iterator for Iter<'a> {
    type Item = &'a Operation;

    fn next(&mut self) -> Option<Self::Item> {
        self.iter.next()
    }
}

#[derive(Clone, Debug)]
pub struct IntoIter {
    iter: vec::IntoIter<Operation>,
}

impl Iterator for IntoIter {
    type Item = Operation;

    fn next(&mut self) -> Option<Self::Item> {
        self.iter.next()
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum Operation {
    Delete(Length),
    Retain(Length),
    Insert(Text),
}

impl Operation {
    pub fn info(&self) -> OperationInfo {
        match *self {
            Self::Delete(length) => OperationInfo::Delete(length),
            Self::Retain(length) => OperationInfo::Retain(length),
            Self::Insert(ref text) => OperationInfo::Insert(text.length()),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum OperationInfo {
    Delete(Length),
    Retain(Length),
    Insert(Length),
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Strategy {
    InsertBefore,
    InsertAfter,
}
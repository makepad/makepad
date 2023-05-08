use {
    crate::{Len, Pos, Range, Text},
    std::{slice, vec},
};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Diff {
    ops: Vec<Op>,
}

impl Diff {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.ops.is_empty()
    }

    pub fn len(&self) -> usize {
        self.ops.len()
    }

    pub fn iter(&self) -> Iter<'_> {
        Iter {
            iter: self.ops.iter(),
        }
    }

    pub fn invert(self, text: &Text) -> Self {
        let mut builder = Builder::new();
        let mut pos = Pos::default();
        for op in self.ops {
            match op {
                Op::Retain(len) => {
                    builder.retain(len);
                    pos += len;
                }
                Op::Insert(text) => {
                    builder.delete(text.len());
                }
                Op::Delete(count) => {
                    let next_pos = pos + count;
                    builder.insert(text.get(Range {
                        start: pos,
                        end: next_pos,
                    }));
                    pos = next_pos;
                }
            }
        }
        builder.finish()
    }

    pub fn compose(self, other: Self) -> Self {
        use std::cmp::Ordering;

        let mut builder = Builder::new();
        let mut op_iter_0 = self.ops.into_iter();
        let mut op_iter_1 = other.ops.into_iter();
        let mut op_opt_0 = op_iter_0.next();
        let mut op_opt_1 = op_iter_1.next();
        loop {
            match (op_opt_0, op_opt_1) {
                (Some(Op::Retain(len_0)), Some(Op::Retain(len_1))) => match len_0.cmp(&len_1) {
                    Ordering::Less => {
                        builder.retain(len_0);
                        op_opt_0 = op_iter_0.next();
                        op_opt_1 = Some(Op::Retain(len_1 - len_0));
                    }
                    Ordering::Equal => {
                        builder.retain(len_0);
                        op_opt_0 = op_iter_0.next();
                        op_opt_1 = op_iter_1.next();
                    }
                    Ordering::Greater => {
                        builder.retain(len_1);
                        op_opt_0 = Some(Op::Retain(len_0 - len_1));
                        op_opt_1 = op_iter_1.next();
                    }
                },
                (Some(Op::Retain(len_0)), Some(Op::Delete(len_1))) => match len_0.cmp(&len_1) {
                    Ordering::Less => {
                        builder.delete(len_0);
                        op_opt_0 = op_iter_0.next();
                        op_opt_1 = Some(Op::Delete(len_1 - len_0));
                    }
                    Ordering::Equal => {
                        builder.delete(len_0);
                        op_opt_0 = op_iter_0.next();
                        op_opt_1 = op_iter_1.next();
                    }
                    Ordering::Greater => {
                        builder.delete(len_1);
                        op_opt_0 = Some(Op::Retain(len_0 - len_1));
                        op_opt_1 = op_iter_1.next();
                    }
                },
                (Some(Op::Insert(mut text)), Some(Op::Retain(len))) => {
                    let text_len = text.len();
                    match text_len.cmp(&len) {
                        Ordering::Less => {
                            builder.insert(text);
                            op_opt_0 = op_iter_0.next();
                            op_opt_1 = Some(Op::Retain(len - text_len));
                        }
                        Ordering::Equal => {
                            builder.insert(text);
                            op_opt_0 = op_iter_0.next();
                            op_opt_1 = op_iter_1.next();
                        }
                        Ordering::Greater => {
                            builder.insert(text.take(len));
                            op_opt_0 = Some(Op::Insert(text));
                            op_opt_1 = op_iter_1.next();
                        }
                    }
                }
                (Some(Op::Insert(mut text)), Some(Op::Delete(len))) => match text.len().cmp(&len) {
                    Ordering::Less => {
                        op_opt_0 = op_iter_0.next();
                        op_opt_1 = Some(Op::Delete(len - text.len()));
                    }
                    Ordering::Equal => {
                        op_opt_0 = op_iter_0.next();
                        op_opt_1 = op_iter_1.next();
                    }
                    Ordering::Greater => {
                        text.skip(len);
                        op_opt_0 = Some(Op::Insert(text));
                        op_opt_1 = op_iter_1.next();
                    }
                },
                (Some(Op::Insert(text)), None) => {
                    builder.insert(text);
                    op_opt_0 = op_iter_0.next();
                    op_opt_1 = None;
                }
                (Some(Op::Retain(len)), None) => {
                    builder.retain(len);
                    op_opt_0 = op_iter_0.next();
                    op_opt_1 = None;
                }
                (Some(Op::Delete(len)), op) => {
                    builder.delete(len);
                    op_opt_0 = op_iter_0.next();
                    op_opt_1 = op;
                }
                (None, Some(Op::Retain(len))) => {
                    builder.retain(len);
                    op_opt_0 = None;
                    op_opt_1 = op_iter_1.next();
                }
                (None, Some(Op::Delete(len))) => {
                    builder.delete(len);
                    op_opt_0 = None;
                    op_opt_1 = op_iter_1.next();
                }
                (None, None) => break,
                (op, Some(Op::Insert(text))) => {
                    builder.insert(text);
                    op_opt_0 = op;
                    op_opt_1 = op_iter_1.next();
                }
            }
        }
        builder.finish()
    }
}

impl<'a> IntoIterator for &'a Diff {
    type Item = &'a Op;
    type IntoIter = Iter<'a>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl IntoIterator for Diff {
    type Item = Op;
    type IntoIter = IntoIter;

    fn into_iter(self) -> Self::IntoIter {
        IntoIter {
            iter: self.ops.into_iter(),
        }
    }
}

#[derive(Debug, Default)]
pub struct Builder {
    ops: Vec<Op>,
}

impl Builder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn retain(&mut self, len: Len) {
        if len == Len::default() {
            return;
        }
        match self.ops.last_mut() {
            Some(Op::Retain(last_len)) => {
                *last_len += len;
            }
            _ => self.ops.push(Op::Retain(len)),
        }
    }

    pub fn delete(&mut self, len: Len) {
        use std::mem;

        if len == Len::default() {
            return;
        }
        match self.ops.as_mut_slice() {
            [.., Op::Delete(last_len)] => {
                *last_len += len;
            }
            [.., Op::Delete(second_last_len), Op::Insert(_)] => {
                *second_last_len += len;
            }
            [.., last_op @ Op::Insert(_)] => {
                let op = mem::replace(last_op, Op::Delete(len));
                self.ops.push(op);
            }
            _ => self.ops.push(Op::Delete(len)),
        }
    }

    pub fn insert(&mut self, text: Text) {
        if text.is_empty() {
            return;
        }
        match self.ops.as_mut_slice() {
            [.., Op::Insert(last_text)] => {
                *last_text += text;
            }
            _ => self.ops.push(Op::Insert(text)),
        }
    }

    pub fn finish(mut self) -> Diff {
        if let Some(Op::Retain(_)) = self.ops.last() {
            self.ops.pop();
        }
        Diff { ops: self.ops }
    }
}

#[derive(Clone, Debug)]
pub struct Iter<'a> {
    iter: slice::Iter<'a, Op>,
}

impl<'a> Iterator for Iter<'a> {
    type Item = &'a Op;

    fn next(&mut self) -> Option<Self::Item> {
        self.iter.next()
    }
}

#[derive(Clone, Debug)]
pub struct IntoIter {
    iter: vec::IntoIter<Op>,
}

impl Iterator for IntoIter {
    type Item = Op;

    fn next(&mut self) -> Option<Self::Item> {
        self.iter.next()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Op {
    Retain(Len),
    Insert(Text),
    Delete(Len),
}

impl Op {
    pub fn len_only(&self) -> LenOnlyOp {
        match *self {
            Self::Retain(len) => LenOnlyOp::Retain(len),
            Self::Insert(ref text) => LenOnlyOp::Insert(text.len()),
            Self::Delete(len) => LenOnlyOp::Delete(len),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LenOnlyOp {
    Retain(Len),
    Insert(Len),
    Delete(Len),
}

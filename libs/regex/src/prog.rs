use super::{char_class::CharClass, range::Range};

#[derive(Clone, Debug)]
pub struct Prog {
    pub insts: Vec<Inst>,
    pub start: InstPtr,
    pub byte_classes: Box<[u8]>,
    pub has_word_boundary: bool,
    pub slot_count: usize,
}

impl Prog {
    pub fn byte_class_count(&self) -> usize {
        self.byte_classes[255] as usize + 1
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum Inst {
    Match,
    ByteRange(ByteRangeInst),
    Char(CharInst),
    Class(ClassInst),
    Nop(NopInst),
    Save(SaveInst),
    Assert(AssertInst),
    Split(SplitInst),
}

impl Inst {
    pub fn byte_range(out: InstPtr, range: Range<u8>) -> Self {
        Self::ByteRange(ByteRangeInst { out, range })
    }

    pub fn char(out: InstPtr, c: char) -> Self {
        Self::Char(CharInst { out, c })
    }

    pub fn class(out: InstPtr, class: CharClass) -> Self {
        Self::Class(ClassInst { out, class })
    }

    pub fn nop(out: InstPtr) -> Self {
        Self::Nop(NopInst { out })
    }

    pub fn save(out: InstPtr, slot_index: usize) -> Self {
        Self::Save(SaveInst { out, slot_index })
    }

    pub fn assert(out: InstPtr, pred: Pred) -> Self {
        Self::Assert(AssertInst { out, pred })
    }

    pub fn split(out_0: InstPtr, out_1: InstPtr) -> Self {
        Self::Split(SplitInst { out_0, out_1 })
    }

    pub fn out_0(&self) -> &InstPtr {
        match self {
            Self::ByteRange(inst) => &inst.out,
            Self::Char(inst) => &inst.out,
            Self::Class(inst) => &inst.out,
            Self::Nop(inst) => &inst.out,
            Self::Save(inst) => &inst.out,
            Self::Assert(inst) => &inst.out,
            Self::Split(inst) => &inst.out_0,
            _ => panic!(),
        }
    }

    pub fn out_1(&self) -> &InstPtr {
        match self {
            Self::Split(inst) => &inst.out_1,
            _ => panic!(),
        }
    }

    pub fn out_0_mut(&mut self) -> &mut InstPtr {
        match self {
            Self::ByteRange(inst) => &mut inst.out,
            Self::Char(inst) => &mut inst.out,
            Self::Class(inst) => &mut inst.out,
            Self::Nop(inst) => &mut inst.out,
            Self::Save(inst) => &mut inst.out,
            Self::Assert(inst) => &mut inst.out,
            Self::Split(inst) => &mut inst.out_0,
            _ => panic!(),
        }
    }

    pub fn out_1_mut(&mut self) -> &mut InstPtr {
        match self {
            Self::Split(inst) => &mut inst.out_1,
            _ => panic!(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ByteRangeInst {
    pub out: InstPtr,
    pub range: Range<u8>,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct CharInst {
    pub out: InstPtr,
    pub c: char,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ClassInst {
    pub out: InstPtr,
    pub class: CharClass,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct NopInst {
    pub out: InstPtr,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct SaveInst {
    pub out: InstPtr,
    pub slot_index: usize,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct AssertInst {
    pub out: InstPtr,
    pub pred: Pred,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Pred {
    TextStart,
    TextEnd,
    LineStart,
    LineEnd,
    WordBoundary,
    NotWordBoundary,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct SplitInst {
    pub out_0: InstPtr,
    pub out_1: InstPtr,
}

pub type InstPtr = usize;

pub const NULL_INST: InstPtr = usize::MAX;

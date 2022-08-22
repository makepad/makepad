use crate::{CharClass, Range};

pub(crate) const NULL_INSTR_PTR: InstrPtr = usize::MAX;

#[derive(Clone, Debug)]
pub(crate) struct Program {
    pub(crate) start: usize,
    pub(crate) slot_count: usize,
    pub(crate) instrs: Vec<Instr>,
}

#[derive(Clone, Debug)]
pub(crate) enum Instr {
    Match,
    ByteRange(Range<u8>, InstrPtr),
    Char(char, InstrPtr),
    CharClass(CharClass, InstrPtr),
    Save(usize, InstrPtr),
    Split(InstrPtr, InstrPtr),
}

impl Instr {
    pub fn next_0(&self) -> &InstrPtr {
        match self {
            Self::ByteRange(_, next_0) => next_0,
            Self::Char(_, next_0) => next_0,
            Self::CharClass(_, next_0) => next_0,
            Self::Save(_, next_0) => next_0,
            Self::Split(next_0, _) => next_0,
            _ => panic!(),
        }
    }

    pub(crate) fn next_1(&self) -> &InstrPtr {
        match self {
            Self::Split(_, next_1) => next_1,
            _ => panic!(),
        }
    }

    pub fn next_0_mut(&mut self) -> &mut InstrPtr {
        match self {
            Self::ByteRange(_, next_0) => next_0,
            Self::Char(_, next_0) => next_0,
            Self::CharClass(_, next_0) => next_0,
            Self::Save(_, next_0) => next_0,
            Self::Split(next_0, _) => next_0,
            _ => panic!(),
        }
    }

    pub(crate) fn next_1_mut(&mut self) -> &mut InstrPtr {
        match self {
            Self::Split(_, next_1) => next_1,
            _ => panic!(),
        }
    }
}

pub(crate) type InstrPtr = usize;

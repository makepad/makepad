use {
    crate::{CharClass, Range},
    std::fmt,
};

pub(crate) const NULL_INSTR_PTR: InstrPtr = usize::MAX;

#[derive(Clone)]
pub(crate) struct Program {
    pub(crate) slot_count: usize,
    pub(crate) instrs: Vec<Instr>,
    pub(crate) start: usize,
}

impl fmt::Debug for Program {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, instr) in self.instrs.iter().enumerate() {
            write!(f, "{:04} {:?}", index, instr)?;
            if index == self.start {
                write!(f, " <start>")?;
            }
            writeln!(f)?;
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) enum Instr {
    Match,
    ByteRange(Range<u8>, InstrPtr),
    Char(char, InstrPtr),
    CharClass(CharClass, InstrPtr),
    Nop(InstrPtr),
    Save(usize, InstrPtr),
    Assert(Pred, InstrPtr),
    Split(InstrPtr, InstrPtr),
}

impl Instr {
    pub fn next_0(&self) -> &InstrPtr {
        match self {
            Self::ByteRange(_, next_0) => next_0,
            Self::Char(_, next_0) => next_0,
            Self::CharClass(_, next_0) => next_0,
            Self::Nop(next_0) => next_0,
            Self::Save(_, next_0) => next_0,
            Self::Assert(_, next_0) => next_0,
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
            Self::Nop(next_0) => next_0,
            Self::ByteRange(_, next_0) => next_0,
            Self::Char(_, next_0) => next_0,
            Self::CharClass(_, next_0) => next_0,
            Self::Nop(next_0) => next_0,
            Self::Save(_, next_0) => next_0,
            Self::Assert(_, next_0) => next_0,
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

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum Pred {
    IsAtStartOfText,
    IsAtEndOfText,
}

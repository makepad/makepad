pub const NULL_INST_PTR: InstPtr = 0;

pub struct Program {
    pub start: InstPtr,
    pub insts: Vec<Inst>,
}

pub enum Inst {
    Match,
    Char(InstPtr, char),
    Split(InstPtr, InstPtr),
}

impl Inst {
    pub fn char(next_0: InstPtr, c: char) -> Self {
        Self::Char(next_0, c)
    }

    pub fn split(next_0: InstPtr, next_1: InstPtr) -> Self {
        Self::Split(next_0, next_1)
    }

    pub fn next_0(&self) -> &InstPtr {
        match self {
            Self::Char(next_0, _) | Self::Split(next_0, _) => next_0,
            _ => panic!(),
        }
    }

    pub fn next_1(&self) -> &InstPtr {
        match self {
            Self::Split(_, next_1) => next_1,
            _ => panic!(),
        }
    }

    pub fn next_0_mut(&mut self) -> &mut InstPtr {
        match self {
            Self::Char(next_0, _) | Self::Split(next_0, _) => next_0,
            _ => panic!(),
        }
    }

    pub fn next_1_mut(&mut self) -> &mut InstPtr {
        match self {
            Self::Split(_, next_1) => next_1,
            _ => panic!(),
        }
    }
}

pub type InstPtr = usize;

#[derive(Clone)]
pub(crate) struct Program {
    pub(crate) instrs: Vec<Instr>,
}

#[derive(Clone, Copy)]
pub(crate) enum Instr {
    Split(InstrPtr, InstrPtr)
}

pub(crate) type InstrPtr = usize;
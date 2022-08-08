#[derive(Clone)]
pub struct Program {
    pub instrs: Vec<Instr>,
}

#[derive(Clone, Copy)]
pub enum Instr {
    Split(InstrPtr, InstrPtr)
}

pub type InstrPtr = u32;
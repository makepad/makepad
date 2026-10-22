//! Machine code back to instructions (makepad inspect `asm`): the JIT's code in the live process
//! and a native build's code of the same fn are decoded by the same decoder, so both sides of a
//! comparison are spelled and classified alike. Text is this module's assembler syntax (Intel on
//! x86-64), so a decoded instruction assembles back to the same instruction ([`super::assemble`]);
//! the tests check that round trip.
//!
//! The decoders ([`super::dis_x64`], [`super::dis_arm64`]) never fail: bytes they do not know
//! become one [`Class::Unknown`] instruction (`.byte` / `.word`) and decoding goes on.

use super::Arch;

/// What an instruction does, for tallies and diffs (one class per instruction).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Class {
    /// a load from memory that is not the stack frame
    Load,
    /// a store to memory that is not the stack frame
    Store,
    /// a load from the stack frame (sp / fp based): reloads of spills, slot reads
    StackLoad,
    /// a store to the stack frame: spills, slot writes
    StackStore,
    /// x64 push / pop, arm64 pre/post-indexed sp stores and loads of a prologue/epilogue pair
    PushPop,
    /// register to register copy (incl. zero/sign extension of a register)
    MovReg,
    /// a constant into a register (mov imm, movz/movk/movn, xor r,r zeroing)
    MovImm,
    /// address arithmetic: lea, adr/adrp, add with a shifted/extended index
    Addr,
    /// integer arithmetic and logic, shifts, bit ops
    Alu,
    /// integer multiply
    Mul,
    /// integer divide / remainder
    Div,
    /// compare / test (flags only)
    Cmp,
    /// setcc / cmov / csel family
    Select,
    /// scalar floating point arithmetic, conversion, compare
    Fp,
    /// packed SIMD (more than one lane), any element type
    Vector,
    /// conditional direct branch (jcc, b.cond, cbz/cbnz, tbz/tbnz)
    CondBranch,
    /// unconditional direct branch
    Branch,
    /// branch through a register / memory (jump tables, tail calls through a pointer)
    IndirectBranch,
    /// direct call
    Call,
    /// call through a register / memory
    IndirectCall,
    Ret,
    /// atomic read-modify-write, exclusive / acquire / release memory access, fences
    Atomic,
    /// ud2 / brk / int3 / hlt
    Trap,
    Nop,
    /// known instruction without a better class (system, cpuid, rdtsc, ...)
    Other,
    /// bytes the decoder does not know (`.byte` / `.word`)
    Unknown,
}

impl Class {
    pub fn name(self) -> &'static str {
        match self {
            Class::Load => "load",
            Class::Store => "store",
            Class::StackLoad => "stack-load",
            Class::StackStore => "stack-store",
            Class::PushPop => "push/pop",
            Class::MovReg => "mov-reg",
            Class::MovImm => "mov-imm",
            Class::Addr => "addr",
            Class::Alu => "alu",
            Class::Mul => "mul",
            Class::Div => "div",
            Class::Cmp => "cmp",
            Class::Select => "select",
            Class::Fp => "fp",
            Class::Vector => "vector",
            Class::CondBranch => "cond-branch",
            Class::Branch => "branch",
            Class::IndirectBranch => "indirect-branch",
            Class::Call => "call",
            Class::IndirectCall => "indirect-call",
            Class::Ret => "ret",
            Class::Atomic => "atomic",
            Class::Trap => "trap",
            Class::Nop => "nop",
            Class::Other => "other",
            Class::Unknown => "unknown",
        }
    }

    /// Ends a basic block (the next instruction starts one).
    pub fn ends_block(self) -> bool {
        matches!(self, Class::CondBranch | Class::Branch | Class::IndirectBranch | Class::Ret | Class::Trap)
    }
}

/// One decoded instruction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Insn {
    /// byte offset from the start of the decoded code
    pub off: u32,
    /// length in bytes (arm64: 4)
    pub len: u8,
    /// lowercase mnemonic (`mov`, `vaddps`, `b.ne`, `ldr`)
    pub mnem: String,
    /// operands in this module's assembler syntax; a direct branch / call target is written as
    /// its absolute address in hex (`0x1234`), the same value as `target`
    pub ops: String,
    pub class: Class,
    /// absolute address a direct branch or call goes to (decode base + displacement)
    pub target: Option<u64>,
    /// absolute address of a pc-relative data operand (x64 `[rip+disp]`, arm64 `adr` / `adrp`
    /// page / literal `ldr`), when the instruction has one
    pub pc_ref: Option<u64>,
    /// a memory operand is present (load, store, or a memory source/destination of an ALU op)
    pub mem: bool,
    /// the memory operand's base is the stack or frame pointer (rsp / rbp, sp / x29)
    pub stack: bool,
}

impl Insn {
    /// `mnem ops`
    pub fn text(&self) -> String {
        if self.ops.is_empty() {
            self.mnem.clone()
        } else {
            format!("{} {}", self.mnem, self.ops)
        }
    }
}

/// Decodes `code`, which is mapped at `addr` (branch targets and pc-relative references are
/// absolute addresses from it). Never fails: unknown bytes become `Class::Unknown` entries.
pub fn decode(arch: Arch, code: &[u8], addr: u64) -> Vec<Insn> {
    match arch {
        Arch::X64 => super::dis_x64::decode(code, addr),
        Arch::Arm64 => super::dis_arm64::decode(code, addr),
    }
}

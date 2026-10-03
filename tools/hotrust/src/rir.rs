//! RIR: HotRust's low-level IR. A CFG of blocks over typed virtual registers
//! (mutable, not SSA, like wasm locals), explicit memory, explicit traps.
//! Every instruction carries a source position for the PC -> source map.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Cls {
    /// 64-bit integer / pointer register
    I,
    F32,
    F64,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct VReg(pub u32);

/// Integer type of an operation: width in bits and signedness. Narrow values are
/// kept extended to 64 bits in registers (sign- or zero-extended by type).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct IntTy {
    pub bits: u8,
    pub signed: bool,
}

pub const I64: IntTy = IntTy { bits: 64, signed: true };
pub const U64: IntTy = IntTy { bits: 64, signed: false };

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum IOp {
    Add,
    Sub,
    Mul,
    And,
    Or,
    Xor,
    Shl,
    Shr,
    Div,
    Rem,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Cond {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FOp {
    Add,
    Sub,
    Mul,
    Div,
    /// Rust `f64::min`/`max`: if one operand is NaN the other is returned (IEEE minNum)
    Min,
    Max,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FUn {
    Neg,
    Abs,
    Sqrt,
    Floor,
    Ceil,
    Trunc,
    RoundEven,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Conv {
    /// re-extend an integer to a (narrower or wider) integer type
    IntToInt(IntTy),
    IntToF(IntTy, bool),   // from, to f64?
    FToInt(bool, IntTy),   // from f64?, to (saturating, NaN -> 0)
    F32ToF64,
    F64ToF32,
    BitsToF(bool),
    FToBits(bool),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mem {
    /// integer load/store of `bytes` (load extends by signedness)
    Int(u8, bool),
    F32,
    F64,
}

#[derive(Clone, PartialEq, Debug)]
pub enum Callee {
    /// through the function table slot (live mode) / direct (AOT)
    Fn(u32),
    Indirect(VReg),
    /// host runtime function by absolute address
    Host(u64),
    /// variadic C function: (address, number of fixed ABI arguments). Darwin arm64 passes
    /// the variadic ones on the stack; SysV x64 sets al = number of vector registers used.
    HostVariadic(u64, u32),
}

#[derive(Clone, PartialEq, Debug)]
pub enum Inst {
    Iconst(VReg, i64),
    Fconst(VReg, u64, bool),
    Mov(VReg, VReg),
    IBin(IOp, IntTy, VReg, VReg, VReg),
    /// `d = a op imm` (imm fits in i32)
    IBinI(IOp, IntTy, VReg, VReg, i64),
    INeg(IntTy, VReg, VReg),
    INot(IntTy, VReg, VReg),
    ICmp(Cond, bool, VReg, VReg, VReg), // signed?, d, a, b
    ICmpI(Cond, bool, VReg, VReg, i64),
    FBin(FOp, bool, VReg, VReg, VReg),  // f64?
    FUnary(FUn, bool, VReg, VReg),
    FCmp(Cond, bool, VReg, VReg, VReg),
    Conv(Conv, VReg, VReg),
    Load(Mem, VReg, VReg, i32),  // d, base, offset
    Store(Mem, VReg, i32, VReg), // base, offset, src
    SlotAddr(VReg, u32),
    /// absolute address constant (data, function table, host object)
    Addr(VReg, u64),
    FnAddr(VReg, u32),
    Call(Callee, Vec<VReg>, Vec<VReg>),
    /// memcpy of a constant size
    Copy(VReg, VReg, u32),
    /// hang-watchdog poll on loop back-edges (live mode)
    Poll,
}

#[derive(Clone, Debug)]
pub enum Term {
    Jump(u32),
    Branch(VReg, u32, u32),
    Ret(Vec<VReg>),
    Unreachable,
}

#[derive(Clone)]
pub struct Block {
    pub insts: Vec<Inst>,
    /// source position per inst: (file << 32) | byte offset
    pub pos: Vec<u64>,
    pub term: Term,
    pub term_pos: u64,
}

pub struct Slot {
    pub size: u32,
    pub align: u32,
}

pub struct Func {
    pub name: String,
    pub blocks: Vec<Block>,
    pub vregs: Vec<Cls>,
    pub slots: Vec<Slot>,
    /// parameters in ABI order
    pub params: Vec<VReg>,
    /// classes of returned values in ABI order
    pub rets: Vec<Cls>,
    pub file: u32,
    /// parameters holding pointers no other pointer of this function aliases
    /// (`&mut T` params, by-pointer aggregate copies); used by load/store forwarding
    pub noalias: Vec<VReg>,
}

impl Func {
    pub fn new(name: String, file: u32) -> Func {
        Func { name, blocks: Vec::new(), vregs: Vec::new(), slots: Vec::new(), params: Vec::new(), rets: Vec::new(), file, noalias: Vec::new() }
    }
    pub fn vreg(&mut self, c: Cls) -> VReg {
        self.vregs.push(c);
        VReg(self.vregs.len() as u32 - 1)
    }
    pub fn block(&mut self) -> u32 {
        self.blocks.push(Block { insts: Vec::new(), pos: Vec::new(), term: Term::Unreachable, term_pos: 0 });
        self.blocks.len() as u32 - 1
    }
    pub fn slot(&mut self, size: u32, align: u32) -> u32 {
        self.slots.push(Slot { size: size.max(1), align: align.max(1) });
        self.slots.len() as u32 - 1
    }

    /// Text dump for debugging and the crash report.
    pub fn dump(&self) -> String {
        let mut s = format!("fn {} params {:?} rets {:?}\n", self.name, self.params, self.rets);
        for (i, b) in self.blocks.iter().enumerate() {
            s.push_str(&format!("b{}:\n", i));
            for x in &b.insts {
                s.push_str(&format!("  {:?}\n", x));
            }
            s.push_str(&format!("  {:?}\n", b.term));
        }
        s
    }

    /// Structural verifier: every use has a class-correct vreg, every target exists.
    pub fn verify(&self) -> Result<(), String> {
        let nb = self.blocks.len() as u32;
        let nv = self.vregs.len() as u32;
        let chk = |v: VReg| -> Result<(), String> {
            if v.0 >= nv {
                return Err(format!("{}: vreg v{} out of range", self.name, v.0));
            }
            Ok(())
        };
        for (bi, b) in self.blocks.iter().enumerate() {
            for x in &b.insts {
                let mut regs: Vec<VReg> = Vec::new();
                match x {
                    Inst::Iconst(d, _) | Inst::Fconst(d, _, _) | Inst::SlotAddr(d, _) | Inst::Addr(d, _) | Inst::FnAddr(d, _) => regs.push(*d),
                    Inst::Mov(d, a) | Inst::INeg(_, d, a) | Inst::INot(_, d, a) | Inst::FUnary(_, _, d, a) | Inst::Conv(_, d, a) | Inst::IBinI(_, _, d, a, _) | Inst::ICmpI(_, _, d, a, _) => {
                        regs.push(*d);
                        regs.push(*a);
                    }
                    Inst::IBin(_, _, d, a, b) | Inst::ICmp(_, _, d, a, b) | Inst::FBin(_, _, d, a, b) | Inst::FCmp(_, _, d, a, b) => {
                        regs.push(*d);
                        regs.push(*a);
                        regs.push(*b);
                    }
                    Inst::Load(_, d, b, _) => {
                        regs.push(*d);
                        regs.push(*b);
                    }
                    Inst::Store(_, b, _, s) => {
                        regs.push(*b);
                        regs.push(*s);
                    }
                    Inst::Call(c, a, r) => {
                        if let Callee::Indirect(v) = c {
                            regs.push(*v);
                        }
                        regs.extend_from_slice(a);
                        regs.extend_from_slice(r);
                    }
                    Inst::Copy(a, b, _) => {
                        regs.push(*a);
                        regs.push(*b);
                    }
                    Inst::Poll => {}
                }
                for r in regs {
                    chk(r)?;
                }
                match x {
                    Inst::IBin(_, _, d, a, b) | Inst::ICmp(_, _, d, a, b) => {
                        if self.vregs[a.0 as usize] != Cls::I || self.vregs[b.0 as usize] != Cls::I || self.vregs[d.0 as usize] != Cls::I {
                            return Err(format!("{}: b{} int op on non-int vreg: {:?}", self.name, bi, x));
                        }
                    }
                    Inst::FBin(_, f64_, d, a, b) => {
                        let c = if *f64_ { Cls::F64 } else { Cls::F32 };
                        if self.vregs[a.0 as usize] != c || self.vregs[b.0 as usize] != c || self.vregs[d.0 as usize] != c {
                            return Err(format!("{}: b{} float op class mismatch: {:?}", self.name, bi, x));
                        }
                    }
                    _ => {}
                }
            }
            match &b.term {
                Term::Jump(t) => {
                    if *t >= nb {
                        return Err(format!("{}: bad jump target", self.name));
                    }
                }
                Term::Branch(c, t, f) => {
                    chk(*c)?;
                    if *t >= nb || *f >= nb {
                        return Err(format!("{}: bad branch target", self.name));
                    }
                }
                Term::Ret(v) => {
                    if v.len() != self.rets.len() {
                        return Err(format!("{}: b{} returns {} values, expected {}", self.name, bi, v.len(), self.rets.len()));
                    }
                    for (i, r) in v.iter().enumerate() {
                        chk(*r)?;
                        if self.vregs[r.0 as usize] != self.rets[i] {
                            return Err(format!("{}: b{} return class mismatch", self.name, bi));
                        }
                    }
                }
                Term::Unreachable => {}
            }
        }
        Ok(())
    }
}

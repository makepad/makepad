//! RIR: Rapid's low-level IR. A CFG of blocks over typed virtual registers
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
    /// Rapid runtime function by absolute address (scalar arguments and results only)
    Host(u64),
    /// foreign (C ABI) function by absolute address
    CHost(u64, Box<CSig>),
    /// call through an `extern "C" fn` pointer
    CIndirect(VReg, Box<CSig>),
    /// direct call of a Rapid `extern "C" fn` (function table slot)
    CFn(u32, Box<CSig>),
}

/// An aggregate passed or returned by value under the C ABI.
#[derive(Clone, PartialEq, Debug)]
pub struct CAgg {
    pub size: u32,
    pub align: u32,
    /// scalar fields in offset order (nested structs and arrays flattened); empty when
    /// unknown or very large (the aggregate is then classified as memory)
    pub fields: Vec<(u32, Mem)>,
}

#[derive(Clone, PartialEq, Debug)]
pub enum CArg {
    /// the vreg holds the value (integers extended to 64 bits as everywhere in RIR)
    Scalar(Mem),
    /// the vreg holds the address of a private copy of the aggregate
    Agg(CAgg),
}

/// C ABI shape of a call or of an `extern "C"` function body; the backend lowers it.
/// ABI argument vregs: [result address if `ret` is Some] then one per `args` entry.
/// With an aggregate return the result is written through that address and the call
/// has no result vregs; scalar results come back in the call's result vregs as usual.
#[derive(Clone, PartialEq, Debug)]
pub struct CSig {
    pub args: Vec<CArg>,
    pub ret: Option<CAgg>,
    /// variadic: the number of fixed arguments in `args`; u32::MAX when not variadic
    pub n_fixed: u32,
    /// a narrow integer result (i8..i32, u8..u32, bool): C leaves the register's upper bits
    /// undefined, so the backend re-extends it after the call
    pub ret_narrow: Option<IntTy>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AtomOrd {
    Relaxed,
    Acquire,
    Release,
    AcqRel,
    SeqCst,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RmwOp {
    Xchg,
    Add,
    Sub,
    And,
    Nand,
    Or,
    Xor,
    Max,
    Min,
    UMax,
    UMin,
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
    /// backend-only (made by `opt::addr_modes` just before codegen):
    /// d = [base + (idx << shift) + offset]
    LoadX(Mem, VReg, VReg, VReg, u8, i32),
    /// [base + (idx << shift) + offset] = src
    StoreX(Mem, VReg, VReg, u8, i32, VReg),
    SlotAddr(VReg, u32),
    /// absolute address constant (data, function table, host object)
    Addr(VReg, u64),
    FnAddr(VReg, u32),
    Call(Callee, Vec<VReg>, Vec<VReg>),
    /// memcpy of a constant size
    Copy(VReg, VReg, u32),
    /// atomic load (Mem::Int only; the value is extended by signedness)
    AtomicLoad(Mem, AtomOrd, VReg, VReg), // d, addr
    AtomicStore(Mem, AtomOrd, VReg, VReg), // addr, src
    /// d = old value; [addr] = old op src
    AtomicRmw(RmwOp, Mem, AtomOrd, VReg, VReg, VReg), // d, addr, src
    /// compare-exchange: d = old value; stores `new` iff old == expected
    AtomicCas(Mem, AtomOrd, AtomOrd, VReg, VReg, VReg, VReg), // ok ord, fail ord, d, addr, expected, new
    /// memory fence; `true` = compiler-only (single-thread) fence
    Fence(AtomOrd, bool),
    /// d = address of this thread's copy of the thread-local block at `offset`
    TlsAddr(VReg, u32),
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

#[derive(Clone)]
pub struct Slot {
    pub size: u32,
    pub align: u32,
}

#[derive(Clone)]
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
    /// Some for an `extern "C" fn` body: params/returns follow the C ABI (see CSig)
    pub cabi: Option<Box<CSig>>,
}

impl Func {
    pub fn new(name: String, file: u32) -> Func {
        Func { name, blocks: Vec::new(), vregs: Vec::new(), slots: Vec::new(), params: Vec::new(), rets: Vec::new(), file, noalias: Vec::new(), cabi: None }
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

    /// C ABI vregs (call arguments or body params) against their CSig.
    fn check_csig(&self, sig: &CSig, vs: &[VReg], what: &str) -> Result<(), String> {
        let skip = sig.ret.is_some() as usize;
        if vs.len() != sig.args.len() + skip {
            return Err(format!("{}: {} ABI values for a C signature of {} (+{} result address)", what, vs.len(), sig.args.len(), skip));
        }
        if skip == 1 && self.vregs[vs[0].0 as usize] != Cls::I {
            return Err(format!("{}: result address is not an integer vreg", what));
        }
        for (k, a) in sig.args.iter().enumerate() {
            let c = self.vregs[vs[skip + k].0 as usize];
            let want = match a {
                CArg::Scalar(Mem::F32) => Cls::F32,
                CArg::Scalar(Mem::F64) => Cls::F64,
                _ => Cls::I,
            };
            if c != want {
                return Err(format!("{}: argument {} is {:?}, its C type needs {:?}", what, k, c, want));
            }
        }
        Ok(())
    }

    /// Structural verifier: every use has a class-correct vreg, every target exists.
    pub fn verify(&self) -> Result<(), String> {
        if let Some(sig) = &self.cabi {
            for p in &self.params {
                if p.0 as usize >= self.vregs.len() {
                    return Err(format!("{}: param vreg out of range", self.name));
                }
            }
            self.check_csig(sig, &self.params, &format!("{}: extern \"C\" params", self.name))?;
            if sig.ret.is_some() && !self.rets.is_empty() {
                return Err(format!("{}: aggregate C result and result registers", self.name));
            }
        }
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
                    Inst::LoadX(_, d, b, x, _, _) => {
                        regs.push(*d);
                        regs.push(*b);
                        regs.push(*x);
                    }
                    Inst::StoreX(_, b, x, _, _, s) => {
                        regs.push(*b);
                        regs.push(*x);
                        regs.push(*s);
                    }
                    Inst::Call(c, a, r) => {
                        if let Callee::Indirect(v) | Callee::CIndirect(v, _) = c {
                            regs.push(*v);
                        }
                        regs.extend_from_slice(a);
                        regs.extend_from_slice(r);
                    }
                    Inst::Copy(a, b, _) => {
                        regs.push(*a);
                        regs.push(*b);
                    }
                    Inst::Fence(..) => {}
                    Inst::TlsAddr(d, _) => regs.push(*d),
                    Inst::AtomicLoad(_, _, d, a) | Inst::AtomicStore(_, _, d, a) => {
                        regs.push(*d);
                        regs.push(*a);
                    }
                    Inst::AtomicRmw(_, _, _, d, a, b) => {
                        regs.push(*d);
                        regs.push(*a);
                        regs.push(*b);
                    }
                    Inst::AtomicCas(_, _, _, d, a, b, c) => {
                        regs.push(*d);
                        regs.push(*a);
                        regs.push(*b);
                        regs.push(*c);
                    }
                }
                for r in regs {
                    chk(r)?;
                }
                if let Inst::Call(Callee::CHost(_, sig) | Callee::CIndirect(_, sig) | Callee::CFn(_, sig), a, r) = x {
                    let what = format!("{}: b{} C call", self.name, bi);
                    self.check_csig(sig, a, &what)?;
                    if sig.ret.is_some() && !r.is_empty() {
                        return Err(format!("{}: aggregate result and result vregs", what));
                    }
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

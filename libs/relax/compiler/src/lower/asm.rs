//! `asm!` (blocked by `--no-asm`; typeck reports it): the block is assembled here, with
//! its operands in registers chosen for it, into one `Callee::Asm` call whose arguments are the
//! inputs and whose results are the outputs (rir AsmBlock). Semantics follow the Reference
//! (inline-assembly.md) and rustc_builtin_macros/src/asm.rs + rustc_target/src/asm/{x86,aarch64}.rs:
//! register classes and the registers they hold, the registers no operand may name, template
//! modifiers and their defaults, `clobber_abi` sets, `noreturn`.
//!
//! Register choice: every register operand gets its own register (an `inout` one register for
//! both directions), explicit ones first; class operands take the first free register of the
//! class, caller-saved before callee-saved, the backend's scratch registers last. The register
//! allocator keeps all other values out of the block's registers (AsmBlock::regs), so the
//! backend only moves inputs in and outputs out.

use super::*;
use relax_backend::asm::{self as rasm, Arch, Arg, PReg, Piece};
use relax_backend::rir::{AsmBlock, ASM_NORETURN as R_NORETURN, ASM_NOMEM as R_NOMEM, ASM_NOSTACK as R_NOSTACK, ASM_PRESERVES_FLAGS as R_PF, ASM_PURE as R_PURE, ASM_READONLY as R_READONLY};

/// A register class (rustc_target asm/{x86,aarch64}.rs `def_reg_class!`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Class {
    X86Reg,
    X86RegAbcd,
    X86RegByte,
    X86Xmm,
    X86Ymm,
    A64Reg,
    A64Vreg,
    A64VregLow16,
}

impl Class {
    fn parse(arch: Arch, name: &str) -> Result<Class, String> {
        Ok(match (arch, name) {
            (Arch::X64, "reg") => Class::X86Reg,
            (Arch::X64, "reg_abcd") => Class::X86RegAbcd,
            (Arch::X64, "reg_byte") => Class::X86RegByte,
            (Arch::X64, "xmm_reg") => Class::X86Xmm,
            (Arch::X64, "ymm_reg") => Class::X86Ymm,
            (Arch::X64, "zmm_reg" | "kreg" | "kreg0" | "mmx_reg" | "x87_reg" | "tmm_reg") => return Err(format!("register class `{}` is not supported by Relax's assembler yet", name)),
            (Arch::Arm64, "reg") => Class::A64Reg,
            (Arch::Arm64, "vreg") => Class::A64Vreg,
            (Arch::Arm64, "vreg_low16") => Class::A64VregLow16,
            (Arch::Arm64, "preg") => return Err("register class `preg` can only be used as a clobber, not as an input or output".to_string()),
            _ => return Err(format!("invalid register class `{}`: unknown register class", name)),
        })
    }
    /// The template modifier a placeholder of this class prints with when it names none
    /// (rustc_target asm `default_modifier` / the AsmPrinter's default): x86-64 `reg` the 64-bit
    /// name, `reg_byte` the low byte, `xmm_reg` / `ymm_reg` their own width; arm64 `reg` the x
    /// name, `vreg` the v name.
    fn default_modifier(self) -> char {
        match self {
            Class::X86Reg | Class::X86RegAbcd => 'r',
            Class::X86RegByte => 'l',
            Class::X86Xmm => 'x',
            Class::X86Ymm => 'y',
            Class::A64Reg => 'x',
            Class::A64Vreg | Class::A64VregLow16 => 'v',
        }
    }
    /// The modifiers the class takes (rustc_target asm `valid_modifiers`).
    fn modifiers(self) -> &'static [char] {
        match self {
            Class::X86Reg => &['l', 'x', 'e', 'r'],
            Class::X86RegAbcd => &['l', 'h', 'x', 'e', 'r'],
            Class::X86RegByte => &[],
            Class::X86Xmm | Class::X86Ymm => &['x', 'y', 'z'],
            Class::A64Reg => &['w', 'x'],
            Class::A64Vreg | Class::A64VregLow16 => &['b', 'h', 's', 'd', 'q', 'v'],
        }
    }
    /// The registers the class allocates from, in the order they are taken (see the module doc).
    fn candidates(self) -> Vec<PReg> {
        let ints = |v: &[u8]| v.iter().map(|&n| PReg::int(n)).collect::<Vec<_>>();
        let vecs = |v: &[u8]| v.iter().map(|&n| PReg::vec(n)).collect::<Vec<_>>();
        match self {
            // rax rcx rdx rsi rdi r8 r9, r10 r11 (backend scratch), r12..r15 (callee-saved);
            // rbx, rbp, rsp are not operands
            Class::X86Reg | Class::X86RegByte => ints(&[0, 1, 2, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15]),
            Class::X86RegAbcd => ints(&[0, 1, 2]),
            Class::X86Xmm | Class::X86Ymm => vecs(&[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15]),
            // x0..x13 x15 (caller-saved; x14 / x15 / x16 / x17 are backend scratch, last), x20..x28
            // (callee-saved); x18 (platform), x19 (LLVM), x29 (fp), x30 (lr) are not taken
            Class::A64Reg => ints(&[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 20, 21, 22, 23, 24, 25, 26, 27, 28]),
            Class::A64Vreg => vecs(&[0, 1, 2, 3, 4, 5, 6, 7, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 8, 9, 10, 11, 12, 13, 14, 15]),
            Class::A64VregLow16 => vecs(&[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15]),
        }
    }
}

/// An explicit register operand (`in("rdi")`): the register, or rustc's error for the names no
/// operand may use.
fn explicit_reg(arch: Arch, name: &str) -> Result<PReg, String> {
    let l = name.to_ascii_lowercase();
    match arch {
        Arch::X64 => {
            match l.as_str() {
                "bp" | "bpl" | "ebp" | "rbp" => return Err("invalid register `rbp`: the frame pointer cannot be used as an operand for inline asm".to_string()),
                "sp" | "spl" | "esp" | "rsp" => return Err("invalid register `rsp`: the stack pointer cannot be used as an operand for inline asm".to_string()),
                "ip" | "eip" | "rip" => return Err("invalid register `rip`: the instruction pointer cannot be used as an operand for inline asm".to_string()),
                "bx" | "bl" | "bh" | "ebx" | "rbx" => return Err(format!("invalid register `{}`: rbx is used internally by LLVM and cannot be used as an operand for inline asm", name)),
                _ => {}
            }
            match rasm::x64::parse_reg(&l) {
                Some((r, _, _)) => Ok(r),
                None => Err(format!("invalid register `{}`: unknown register", name)),
            }
        }
        Arch::Arm64 => {
            match l.as_str() {
                "x19" | "w19" => return Err(format!("invalid register `{}`: x19 is used internally by LLVM and cannot be used as an operand for inline asm", name)),
                "x29" | "w29" | "fp" | "wfp" => return Err(format!("invalid register `{}`: the frame pointer cannot be used as an operand for inline asm", name)),
                "sp" | "wsp" => return Err(format!("invalid register `{}`: the stack pointer cannot be used as an operand for inline asm", name)),
                "xzr" | "wzr" => return Err(format!("invalid register `{}`: the zero register cannot be used as an operand for inline asm", name)),
                "x18" | "w18" if reserves_x18() => return Err(format!("invalid register `{}`: x18 is a reserved register on this target", name)),
                _ => {}
            }
            match rasm::arm64::parse_reg(&l) {
                Some((r, _)) => Ok(r),
                None => Err(format!("invalid register `{}`: unknown register", name)),
            }
        }
    }
}

/// rustc_target asm/aarch64.rs target_reserves_x18: Darwin, Android, Windows (and Fuchsia, OHOS).
fn reserves_x18() -> bool {
    cfg!(any(target_vendor = "apple", target_os = "android", windows))
}

/// The registers `clobber_abi(abi)` clobbers (rustc_target asm/mod.rs InlineAsmClobberAbi
/// clobbered_regs) as a mask (PReg::bit).
fn clobber_abi_mask(arch: Arch, bits: u32) -> u64 {
    let mut m = 0u64;
    let int = |l: &[u8]| l.iter().fold(0u64, |a, &n| a | 1 << n);
    let all_vec = 0xffff_ffffu64 << 32;
    match arch {
        Arch::X64 => {
            let sysv = int(&[0, 1, 2, 6, 7, 8, 9, 10, 11]) | all_vec;
            let win = int(&[0, 1, 2, 8, 9, 10, 11]) | all_vec;
            if bits & ASM_CLOBBER_C != 0 {
                m |= if cfg!(windows) { win } else { sysv };
            }
            if bits & ASM_CLOBBER_SYSV64 != 0 {
                m |= sysv;
            }
            if bits & ASM_CLOBBER_WIN64 != 0 {
                m |= win;
            }
        }
        Arch::Arm64 => {
            if bits & (ASM_CLOBBER_C | ASM_CLOBBER_SYSV64 | ASM_CLOBBER_WIN64) != 0 {
                m |= int(&[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 30]) | all_vec;
                if !reserves_x18() {
                    m |= 1 << 18;
                }
            }
        }
    }
    m
}

/// How an operand's value sits in its register.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Rep {
    /// an integer / pointer of `bits` (signed?): an I vreg
    Int(u8, bool),
    /// f32 / f64: an F32 / F64 vreg
    Float(bool),
    /// a 16-byte vector in memory: a V128 vreg
    V128,
    /// an 8-byte vector in memory: the low half of a vector register (an F64 vreg)
    V64,
}

impl<'a> Lcx<'a> {
    /// The register form of a value of type `t`, or rustc's error for a type the asm block takes
    /// in no register.
    fn asm_rep(&mut self, t: TyId) -> Result<Rep, String> {
        let k = self.kind(t);
        Ok(match k {
            TyKind::Int(p) => {
                let (bits, signed) = match p {
                    Prim::I8 => (8, true),
                    Prim::I16 => (16, true),
                    Prim::I32 => (32, true),
                    Prim::I64 | Prim::Isize => (64, true),
                    Prim::U8 => (8, false),
                    Prim::U16 => (16, false),
                    Prim::U32 => (32, false),
                    Prim::U64 | Prim::Usize => (64, false),
                    _ => return Err("cannot use value of type `i128`/`u128` for inline assembly".to_string()),
                };
                Rep::Int(bits, signed)
            }
            TyKind::Float(p) => match p {
                Prim::F32 => Rep::Float(false),
                Prim::F64 => Rep::Float(true),
                _ => return Err("this float type is not supported in inline assembly by Relax".to_string()),
            },
            TyKind::Ptr(..) | TyKind::Ref(..) | TyKind::FnPtr(..) if !self.unsized_ty(t) || false => {
                if let TyKind::Ptr(_, inner) | TyKind::Ref(_, inner, _) = k {
                    if self.unsized_ty(inner) {
                        return Err("cannot use value of a wide pointer type for inline assembly".to_string());
                    }
                }
                Rep::Int(64, false)
            }
            _ => {
                let l = self.layout(t);
                let size = l.size;
                match (self.simd_lanes(t), size) {
                    (true, 16) => Rep::V128,
                    (true, 8) => Rep::V64,
                    _ => {
                        let s = crate::typeck::ty_to_string(false, &self.u.prog, &self.u.tcx, t);
                        return Err(format!("cannot use value of type `{}` for inline assembly: only integers, floats, SIMD vectors, pointers and function pointers can be used as arguments for inline assembly", s));
                    }
                }
            }
        })
    }

    /// Whether `t` is a `#[repr(simd)]` vector.
    fn simd_lanes(&mut self, t: TyId) -> bool {
        self.simd_shape(t).is_some()
    }

    /// rustc_target `supported_types` per class, as far as the representation tells.
    fn asm_type_ok(&self, c: Class, r: Rep) -> bool {
        match (c, r) {
            (Class::X86RegByte, Rep::Int(8, _)) => true,
            (Class::X86RegByte, _) => false,
            (Class::X86Reg | Class::X86RegAbcd, Rep::Int(b, _)) => b >= 16,
            (Class::X86Reg | Class::X86RegAbcd, Rep::Float(_)) => true,
            (Class::X86Reg | Class::X86RegAbcd, _) => false,
            (Class::X86Xmm, Rep::Int(b, _)) => b >= 32,
            (Class::X86Xmm, Rep::Float(_) | Rep::V128) => true,
            (Class::X86Xmm, Rep::V64) => false,
            (Class::X86Ymm, Rep::Int(b, _)) => b >= 32,
            (Class::X86Ymm, Rep::Float(_) | Rep::V128) => true,
            (Class::X86Ymm, _) => false,
            (Class::A64Reg, Rep::Int(..) | Rep::Float(_)) => true,
            (Class::A64Reg, _) => false,
            (Class::A64Vreg | Class::A64VregLow16, _) => true,
        }
    }

    /// The input vreg for `v` of representation `r` in a register of the vector file (`vec`) or
    /// the integer file.
    fn asm_in(&mut self, v: Val, t: TyId, r: Rep, vec: bool) -> VReg {
        match r {
            Rep::V128 => {
                let p = self.val_to_place(v);
                let a = self.place_addr(&p, t);
                let d = self.f.vreg(Cls::V128);
                self.emit(Inst::VLoad(d, a, 0));
                d
            }
            Rep::V64 => {
                let p = self.val_to_place(v);
                let a = self.place_addr(&p, t);
                let d = self.f.vreg(Cls::F64);
                self.emit(Inst::Load(Mem::F64, d, a, 0));
                d
            }
            Rep::Int(..) => {
                let x = self.to_leaves(v, t)[0];
                if vec {
                    let d = self.f.vreg(Cls::F64);
                    self.emit(Inst::Conv(Conv::BitsToF(true), d, x));
                    d
                } else {
                    x
                }
            }
            Rep::Float(f64_) => {
                let x = self.to_leaves(v, t)[0];
                if vec {
                    x
                } else {
                    let d = self.f.vreg(Cls::I);
                    self.emit(Inst::Conv(Conv::FToBits(f64_), d, x));
                    d
                }
            }
        }
    }

    /// The result vreg class an output of representation `r` comes back in.
    fn asm_out_cls(r: Rep, vec: bool) -> Cls {
        match r {
            Rep::V128 => Cls::V128,
            Rep::V64 => Cls::F64,
            Rep::Int(..) if vec => Cls::F64,
            Rep::Int(..) => Cls::I,
            Rep::Float(f64_) if vec => if f64_ { Cls::F64 } else { Cls::F32 },
            Rep::Float(_) => Cls::I,
        }
    }

    /// Writes an output's register value `x` to its place.
    fn asm_out(&mut self, place: &(Place, TyId), x: VReg, r: Rep, vec: bool) {
        let (p, t) = place;
        match r {
            Rep::V128 => {
                let a = self.place_addr(p, *t);
                self.emit(Inst::VStore(a, 0, x));
            }
            Rep::V64 => {
                let a = self.place_addr(p, *t);
                self.emit(Inst::Store(Mem::F64, a, 0, x));
            }
            Rep::Int(bits, signed) => {
                let mut v = x;
                if vec {
                    let d = self.f.vreg(Cls::I);
                    self.emit(Inst::Conv(Conv::FToBits(true), d, v));
                    v = d;
                }
                // the register's bits above the type are undefined after the block: re-extend
                if bits < 64 {
                    let d = self.f.vreg(Cls::I);
                    self.emit(Inst::Conv(Conv::IntToInt(IntTy { bits, signed }), d, v));
                    v = d;
                }
                self.write_place(p, *t, Val::L(vec![v]));
            }
            Rep::Float(f64_) => {
                let mut v = x;
                if !vec {
                    let d = self.f.vreg(if f64_ { Cls::F64 } else { Cls::F32 });
                    self.emit(Inst::Conv(Conv::BitsToF(f64_), d, v));
                    v = d;
                }
                self.write_place(p, *t, Val::L(vec![v]));
            }
        }
    }

    /// `sym path`: a function's callable address (a foreign fn: the loader's symbol), or a
    /// foreign static's address.
    fn asm_sym(&mut self, x: ExprId) -> Result<relax_backend::rir::Sym, String> {
        let t = self.body_ty(x);
        if let TyKind::FnDef(d, args, _) = self.kind(t) {
            if self.u.prog.def(d).kind == DefKind::ForeignFn {
                return self.u.foreign_sym(d).ok_or_else(|| format!("sym operand: `{}` has no address on this host", self.u.prog.name(d)));
            }
            let args = self.u.tcx.tys.list(args).to_vec();
            let instance = Instance::resolve_for_fn_ptr(&self.u.tcx, &self.u.prog, d, &args).map_err(|_| "sym operand: the function has no instance".to_string())?;
            let id = self.u.instance_fn_id(instance);
            return Ok(self.u.syms.intern(crate::syms::SymKind::Thunk(id), self.u.entry(id)));
        }
        if let Some(Res::Def(d)) = self.b.res.get(&x.0).copied() {
            if self.u.prog.def(d).kind == DefKind::ForeignStatic {
                return self.u.foreign_sym(d).ok_or_else(|| format!("sym operand: `{}` has no address on this host", self.u.prog.name(d)));
            }
            if self.u.prog.def(d).kind == DefKind::Static {
                // the compile domain's copy (assembly cannot add a thread's domain base)
                let a = self.static_addr(d);
                return Ok(self.u.syms.intern(crate::syms::SymKind::StaticAddr(d), a));
            }
        }
        Err("invalid `sym` operand: the path must refer to a function or a static".to_string())
    }

    /// `const expr`: the integer the expression is (an `Iconst` lowering made).
    fn asm_const(&mut self, x: ExprId) -> Result<i128, String> {
        let t = self.body_ty(x);
        if !matches!(self.kind(t), TyKind::Int(_)) {
            return Err("invalid type for `const` operand: integers only".to_string());
        }
        let v = self.lower_expr(x);
        let r = self.to_leaves(v, t);
        let blk = &self.f.blocks[self.cur as usize];
        for i in blk.insts.iter().rev() {
            if let Inst::Iconst(d, k) = i {
                if *d == r[0] {
                    return Ok(*k as i128);
                }
            }
        }
        Err("`const` operand: the expression must be a constant integer".to_string())
    }

    /// Whether the fn item carries `#[naked]` / `#[unsafe(naked)]`.
    pub(super) fn is_naked(&self, item: ItemId) -> bool {
        let f = &self.u.prog.files[self.b.file as usize];
        f.ast.list(f.ast.item(item).attrs).iter().any(|a| (a.toks.lo..a.toks.hi).take(3).any(|t| f.tok_text(t) == "naked"))
    }

    /// A naked fn's body: one `naked_asm!` (Reference [attributes.codegen.naked]), assembled as
    /// the whole function (Func::naked).
    pub(super) fn lower_naked(&mut self, body: Option<BlockId>) {
        let pos = self.pos;
        let r = (|| -> Result<(), String> {
            let ast = self.ast();
            let b = body.ok_or("a naked fn needs a body")?;
            let stmts = ast.list(ast.block(b).stmts);
            let [Stmt::Expr(e, _)] = stmts else { return Err("the body of a naked function must be a single `naked_asm!` invocation".to_string()) };
            let ExprKind::Mac(m) = &ast.expr(*e).kind else { return Err("the body of a naked function must be a single `naked_asm!` invocation".to_string()) };
            let m = ast.row(*m);
            let name = ast.segs(m.path.segs).last().map(|s| self.ident_text(s.name)).unwrap_or("");
            if name != "naked_asm" || m.args == NO_MAC_ARGS {
                return Err("the body of a naked function must be a single `naked_asm!` invocation".to_string());
            }
            let margs = ast.mac_arg(m.args);
            self.pos = ast.expr(*e).lo;
            self.lower_asm_inner(&margs, true).map(|_| ())
        })();
        if let Err(m) = r {
            self.err(pos, m);
            self.term(Term::Unreachable);
        }
    }

    pub(super) fn lower_asm(&mut self, margs: &MacArgs) -> Val {
        let pos = self.pos;
        match self.lower_asm_inner(margs, false) {
            Ok(v) => v,
            Err(m) => {
                self.err(pos, m);
                self.unit()
            }
        }
    }

    fn lower_asm_inner(&mut self, margs: &MacArgs, naked: bool) -> Result<Val, String> {
        let ast = self.ast();
        if self.u.prog.session.options.no_asm {
            return Err(relax_session::options::asm_blocked(&self.u.prog.file_crate_name(self.b.file)));
        }
        let spec = margs.asm.ok_or("asm! without operands")?;
        let arch = Arch::host();
        let mexprs = ast.list(margs.exprs);
        let templates: Vec<String> = mexprs[..spec.templates as usize]
            .iter()
            .map(|&e| {
                let ex = ast.expr(e);
                crate::lit::unescape_str(&self.src_text(ex.lo, ex.hi))
            })
            .collect();
        if spec.options & ASM_CLOBBER_UNKNOWN != 0 {
            return Err(format!("invalid ABI for `clobber_abi`: valid ABIs are {}", if arch == Arch::X64 { "`C`, `system`, `efiapi`, `win64`, `sysv64`" } else { "`C`, `system`, `efiapi`" }));
        }
        if spec.options & ASM_MAY_UNWIND != 0 {
            return Err("asm! with `options(may_unwind)` is not supported by Relax".to_string());
        }
        if spec.options & ASM_ATT_SYNTAX != 0 && arch != Arch::X64 {
            return Err("the `att_syntax` option is only supported on x86".to_string());
        }
        let ops: Vec<AsmOp> = ast.list(spec.ops).to_vec();
        // registers: explicit ones first, then classes in operand order
        let mut used: u64 = 0;
        let mut regs: Vec<Option<(PReg, Option<Class>)>> = vec![None; ops.len()];
        for (k, op) in ops.iter().enumerate() {
            if !op.explicit || matches!(op.dir, AsmDir::Const | AsmDir::Sym) {
                continue;
            }
            let name = crate::lit::unescape_str(&self.src_text(op.reg.lo, op.reg.hi));
            let r = explicit_reg(arch, &name)?;
            if used & (1 << r.bit()) != 0 {
                return Err(format!("register `{}` conflicts with another operand's register", name));
            }
            used |= 1 << r.bit();
            regs[k] = Some((r, None));
        }
        for (k, op) in ops.iter().enumerate() {
            if op.explicit || matches!(op.dir, AsmDir::Const | AsmDir::Sym) {
                continue;
            }
            let c = Class::parse(arch, &self.src_text(op.reg.lo, op.reg.hi))?;
            let r = c.candidates().into_iter().find(|r| used & (1 << r.bit()) == 0).ok_or("inline assembly requires more registers than available")?;
            used |= 1 << r.bit();
            regs[k] = Some((r, Some(c)));
        }
        // operand values: inputs before the block, output places
        let mut ins: Vec<VReg> = Vec::new();
        let mut in_regs: Vec<PReg> = Vec::new();
        let mut outs: Vec<(VReg, (Place, TyId), Rep, bool)> = Vec::new();
        let mut writes: u64 = clobber_abi_mask(arch, spec.options);
        let mut args: Vec<Arg> = Vec::with_capacity(ops.len());
        for (k, op) in ops.iter().enumerate() {
            match op.dir {
                AsmDir::Sym => {
                    let s = self.asm_sym(op.input.ok_or("sym operand without a path")?)?;
                    args.push(Arg::Sym(rasm::SymRef::Sym(s)));
                    continue;
                }
                AsmDir::Const => {
                    let v = self.asm_const(op.input.ok_or("const operand without a value")?)?;
                    args.push(Arg::Const(v));
                    continue;
                }
                _ => {}
            }
            if naked {
                return Err("naked_asm! takes only `const` and `sym` operands".to_string());
            }
            let (r, class) = regs[k].unwrap();
            args.push(Arg::Reg(r));
            let vec = r.vec;
            let reads = matches!(op.dir, AsmDir::In | AsmDir::InOut | AsmDir::InLateOut);
            let writes_it = !matches!(op.dir, AsmDir::In);
            let mut in_ty = None;
            if reads {
                let x = op.input.ok_or("input operand without a value")?;
                let t = self.body_ty(x);
                let rep = self.asm_rep(t)?;
                if let Some(c) = class {
                    if !self.asm_type_ok(c, rep) {
                        let s = crate::typeck::ty_to_string(false, &self.u.prog, &self.u.tcx, t);
                        return Err(format!("type `{}` cannot be used with this register class", s));
                    }
                }
                let v = self.lower_expr(x);
                let vr = self.asm_in(v, t, rep, vec);
                ins.push(vr);
                in_regs.push(r);
                in_ty = Some(rep);
            }
            if writes_it {
                writes |= 1 << r.bit();
                if let Some(y) = op.output {
                    let t = self.body_ty(y);
                    let rep = self.asm_rep(t)?;
                    if let Some(c) = class {
                        if !self.asm_type_ok(c, rep) {
                            let s = crate::typeck::ty_to_string(false, &self.u.prog, &self.u.tcx, t);
                            return Err(format!("type `{}` cannot be used with this register class", s));
                        }
                    }
                    let _ = in_ty;
                    let place = self.lower_place(y);
                    let d = self.f.vreg(Self::asm_out_cls(rep, vec));
                    outs.push((d, place, rep, vec));
                }
            }
        }
        // the template: placeholders take their class's default modifier; explicit registers
        // cannot be named (rustc: "explicit register arguments cannot be used in the asm template")
        let names: Vec<(String, u32)> = ops.iter().enumerate().filter_map(|(k, op)| op.name.map(|n| (self.ident_text(n).to_string(), k as u32))).collect();
        let positional = ops.iter().take_while(|op| op.name.is_none() && !op.explicit).count() as u32;
        let tpl: Vec<&str> = templates.iter().map(String::as_str).collect();
        let mut pieces = rasm::parse_template(&tpl, &names, positional, ops.len() as u32, spec.options & ASM_RAW != 0)?;
        for p in &mut pieces {
            if let Piece::Op(k, m) = p {
                let op = &ops[*k as usize];
                match regs[*k as usize] {
                    Some((_, None)) => return Err("invalid asm template string: explicit register arguments cannot be used in the asm template".to_string()),
                    Some((_, Some(c))) => match m {
                        None => *m = Some(c.default_modifier()),
                        Some(x) if !c.modifiers().contains(x) => return Err(format!("invalid asm template modifier `{}` for this register class", x)),
                        Some(_) => {}
                    },
                    None if m.is_some() && matches!(op.dir, AsmDir::Const | AsmDir::Sym) => return Err("asm template modifiers are not allowed for `const` or `sym` arguments".to_string()),
                    None => {}
                }
            }
        }
        let out = rasm::assemble(arch, rasm::Format::host(), rasm::Kind::Inline, &pieces, &args, spec.options & ASM_ATT_SYNTAX != 0).map_err(|e| format!("asm!: {}", e))?;
        if out.sections.len() != 1 {
            return Err("asm!: an inline block has one section".to_string());
        }
        let code = out.sections[0].bytes.clone();
        if arch == Arch::Arm64 && code.len() % 4 != 0 {
            return Err("asm!: the block's size is not a multiple of 4 bytes".to_string());
        }
        let mut fixups = Vec::new();
        for f in &out.fixups {
            let s = match &f.target {
                rasm::Value::Sym(rasm::SymRef::Sym(s), off) => s.plus(*off),
                rasm::Value::Sym(rasm::SymRef::Name(n), _) => return Err(format!("asm!: `{}` is no label of this block (name a function or static with a `sym` operand)", n)),
                _ => return Err("asm!: a label of this block used as an address".to_string()),
            };
            let ok = match arch {
                Arch::X64 => matches!(f.kind, rasm::FixKind::X64Branch32 | rasm::FixKind::X64Rel32 { .. } | rasm::FixKind::Abs64 | rasm::FixKind::Abs32),
                Arch::Arm64 => matches!(f.kind, rasm::FixKind::A64Branch26 | rasm::FixKind::A64Page21 | rasm::FixKind::A64Lo12 { scale: 0 | 3 } | rasm::FixKind::Abs64 | rasm::FixKind::Abs32),
            };
            if !ok {
                return Err(format!("asm!: a symbol in this instruction form ({:?}) cannot be linked by Relax", f.kind));
            }
            fixups.push((f.at, f.kind, s));
        }
        let mut opts = 0;
        use crate::ast as x;
        for (a, b) in [(x::ASM_NORETURN, R_NORETURN), (x::ASM_NOSTACK, R_NOSTACK), (x::ASM_NOMEM, R_NOMEM), (x::ASM_READONLY, R_READONLY), (x::ASM_PURE, R_PURE), (x::ASM_PRESERVES_FLAGS, R_PF)] {
            if spec.options & a != 0 {
                opts |= b;
            }
        }
        if naked {
            opts |= R_NORETURN;
        }
        if opts & R_NORETURN != 0 && !outs.is_empty() {
            return Err("asm outputs are not allowed with the `noreturn` option".to_string());
        }
        let block = AsmBlock { code, fixups, ins: in_regs, outs: outs.iter().map(|o| regs_of(&ops, &regs, o.0, &outs)).collect(), regs: used | writes, writes, opts };
        let rets: Vec<VReg> = outs.iter().map(|o| o.0).collect();
        self.emit(Inst::Call(Callee::Asm(Box::new(block)), ins, rets));
        if naked {
            self.f.naked = true;
        }
        if opts & R_NORETURN != 0 {
            self.term(Term::Unreachable);
            self.dead();
            return Ok(self.unit());
        }
        for (d, place, rep, vec) in outs {
            self.asm_out(&place, d, rep, vec);
        }
        Ok(self.unit())
    }
}

/// The register of output `d` (outputs are pushed in operand order with their register).
fn regs_of(ops: &[AsmOp], regs: &[Option<(PReg, Option<Class>)>], d: VReg, outs: &[(VReg, (Place, TyId), Rep, bool)]) -> PReg {
    let n = outs.iter().position(|o| o.0 == d).unwrap();
    let mut k = 0;
    for (i, op) in ops.iter().enumerate() {
        if matches!(op.dir, AsmDir::Out | AsmDir::LateOut | AsmDir::InOut | AsmDir::InLateOut) && op.output.is_some() {
            if k == n {
                return regs[i].unwrap().0;
            }
            k += 1;
        }
    }
    unreachable!()
}

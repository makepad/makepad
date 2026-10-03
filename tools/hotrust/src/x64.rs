//! x86-64 backend (tier 0): liveness, linear-scan register allocation over
//! hull intervals, instruction selection and encoding. Produces position-
//! independent code apart from absolute addresses for data, the function
//! table and host functions (fine in-process; AOT adds relocations later).
//!
//! ABI (HotRust's own and SysV for host calls): int args rdi rsi rdx rcx r8 r9,
//! float args xmm0-7, int returns rax rdx, float returns xmm0 xmm1. Callee-saved
//! rbx rbp r12-r15. Reserved scratch: rax rcx rdx r10 r11, xmm0 xmm1 xmm14 xmm15.

use crate::cabi::{self, ArgPlan, PLoc, Plan, RetPlan};
use crate::regalloc::{self, Alloc, Loc, MLoc, RegConfig};
use crate::rir::*;

const RAX: u8 = 0;
const RCX: u8 = 1;
const RDX: u8 = 2;
const RBX: u8 = 3;
const RSP: u8 = 4;
const RBP: u8 = 5;
const RSI: u8 = 6;
const RDI: u8 = 7;
const R8: u8 = 8;
const R9: u8 = 9;
const R10: u8 = 10;
const R11: u8 = 11;
const R12: u8 = 12;
const R13: u8 = 13;
const R14: u8 = 14;
const R15: u8 = 15;

const INT_ARGS: [u8; 6] = [RDI, RSI, RDX, RCX, R8, R9];
const CALLEE_SAVED: [u8; 5] = [RBX, R12, R13, R14, R15];
const CALLER_SAVED_ALLOC: [u8; 4] = [RSI, RDI, R8, R9];
const XS: u8 = 15; // float scratch
const XS2: u8 = 14;


pub struct Compiled {
    pub code: Vec<u8>,
    /// (code offset, source position) in ascending offset order
    pub pc_map: Vec<(u32, u64)>,
    pub frame_size: u32,
    /// bytes of instructions (the float constant pool follows)
    pub text_len: u32,
}

/// Addresses the backend needs from the runtime.
pub struct Env {
    pub table_base: u64,
    pub thunk_base: u64,
    pub thunk_size: u64,
    pub poll_flag: u64,
    pub rt_hang: u64,
    /// fs-relative offset of the runtime's per-thread block pointer
    pub tls_key: u64,
    /// glue that allocates the calling thread's block: returns it in r11, preserves all else
    pub tls_slow: u64,
}

struct Asm {
    b: Vec<u8>,
    /// pending scaled index (index reg, shift) for the next memory operand
    sib: Option<(u8, u8)>,
}

impl Asm {
    fn byte(&mut self, x: u8) {
        self.b.push(x);
    }
    fn u32(&mut self, x: u32) {
        self.b.extend_from_slice(&x.to_le_bytes());
    }
    fn u64(&mut self, x: u64) {
        self.b.extend_from_slice(&x.to_le_bytes());
    }
    fn pos(&self) -> usize {
        self.b.len()
    }

    fn rex(&mut self, w: bool, r: u8, x: u8, b: u8, force: bool) {
        let x = match self.sib {
            Some((ix, _)) => ix,
            None => x,
        };
        let v = 0x40 | ((w as u8) << 3) | (((r >> 3) & 1) << 2) | (((x >> 3) & 1) << 1) | ((b >> 3) & 1);
        if v != 0x40 || force {
            self.byte(v);
        }
    }

    /// ModRM for register-register.
    fn modrm_rr(&mut self, reg: u8, rm: u8) {
        self.byte(0xc0 | ((reg & 7) << 3) | (rm & 7));
    }

    /// ModRM + SIB + disp for [base + disp].
    fn modrm_mem(&mut self, reg: u8, base: u8, disp: i32) {
        if let Some((ix, sh)) = self.sib.take() {
            // [base + index << sh + disp]: rm = 100, SIB follows
            let r = (reg & 7) << 3;
            let sib = (sh << 6) | ((ix & 7) << 3) | (base & 7);
            if disp == 0 && base & 7 != 5 {
                self.byte(r | 4);
                self.byte(sib);
            } else if (-128..128).contains(&disp) {
                self.byte(0x40 | r | 4);
                self.byte(sib);
                self.byte(disp as i8 as u8);
            } else {
                self.byte(0x80 | r | 4);
                self.byte(sib);
                self.u32(disp as u32);
            }
            return;
        }
        let r = (reg & 7) << 3;
        let bb = base & 7;
        if disp == 0 && bb != 5 {
            self.byte(r | bb);
            if bb == 4 {
                self.byte(0x24);
            }
        } else if (-128..128).contains(&disp) {
            self.byte(0x40 | r | bb);
            if bb == 4 {
                self.byte(0x24);
            }
            self.byte(disp as i8 as u8);
        } else {
            self.byte(0x80 | r | bb);
            if bb == 4 {
                self.byte(0x24);
            }
            self.u32(disp as u32);
        }
    }

    // ---- integer

    /// op r/m64, r64 style (`opc` = e.g. 0x01 add, 0x29 sub, 0x21 and, 0x09 or, 0x31 xor, 0x39 cmp, 0x89 mov)
    fn alu_rr(&mut self, opc: u8, dst: u8, src: u8) {
        self.rex(true, src, 0, dst, false);
        self.byte(opc);
        self.modrm_rr(src, dst);
    }
    fn mov_rr(&mut self, dst: u8, src: u8) {
        if dst != src {
            self.alu_rr(0x89, dst, src);
        }
    }
    fn mov_ri(&mut self, dst: u8, v: i64) {
        if v == 0 {
            // xor r32, r32
            self.rex(false, dst, 0, dst, false);
            self.byte(0x31);
            self.modrm_rr(dst, dst);
        } else if v >= 0 && v <= u32::MAX as i64 {
            self.rex(false, 0, 0, dst, false);
            self.byte(0xb8 + (dst & 7));
            self.u32(v as u32);
        } else if v >= i32::MIN as i64 && v <= i32::MAX as i64 {
            self.rex(true, 0, 0, dst, false);
            self.byte(0xc7);
            self.modrm_rr(0, dst);
            self.u32(v as i32 as u32);
        } else {
            self.rex(true, 0, 0, dst, false);
            self.byte(0xb8 + (dst & 7));
            self.u64(v as u64);
        }
    }
    fn load64(&mut self, dst: u8, base: u8, disp: i32) {
        self.rex(true, dst, 0, base, false);
        self.byte(0x8b);
        self.modrm_mem(dst, base, disp);
    }
    fn store64(&mut self, base: u8, disp: i32, src: u8) {
        self.rex(true, src, 0, base, false);
        self.byte(0x89);
        self.modrm_mem(src, base, disp);
    }
    fn lea(&mut self, dst: u8, base: u8, disp: i32) {
        self.rex(true, dst, 0, base, false);
        self.byte(0x8d);
        self.modrm_mem(dst, base, disp);
    }
    fn imul_rr(&mut self, dst: u8, src: u8) {
        self.rex(true, dst, 0, src, false);
        self.byte(0x0f);
        self.byte(0xaf);
        self.modrm_rr(dst, src);
    }
    /// group 3 (F7): /3 neg, /2 not, /6 div, /7 idiv
    fn grp3(&mut self, ext: u8, r: u8) {
        self.rex(true, 0, 0, r, false);
        self.byte(0xf7);
        self.modrm_rr(ext, r);
    }
    /// shifts by cl: /4 shl, /5 shr, /7 sar
    fn shift_cl(&mut self, ext: u8, r: u8) {
        self.rex(true, 0, 0, r, false);
        self.byte(0xd3);
        self.modrm_rr(ext, r);
    }
    fn cqo(&mut self) {
        self.byte(0x48);
        self.byte(0x99);
    }
    fn test_rr(&mut self, a: u8, b: u8) {
        self.rex(true, b, 0, a, false);
        self.byte(0x85);
        self.modrm_rr(b, a);
    }
    fn setcc(&mut self, cc: u8, r: u8) {
        self.rex(false, 0, 0, r, r >= 4);
        self.byte(0x0f);
        self.byte(0x90 | cc);
        self.modrm_rr(0, r);
    }
    /// movzx r64, r8
    fn movzx8(&mut self, dst: u8, src: u8) {
        self.rex(true, dst, 0, src, src >= 4);
        self.byte(0x0f);
        self.byte(0xb6);
        self.modrm_rr(dst, src);
    }
    /// sign/zero extension of the low `bits` of `r` in place
    fn extend(&mut self, r: u8, bits: u8, signed: bool) {
        match (bits, signed) {
            (8, true) => {
                self.rex(true, r, 0, r, r >= 4);
                self.byte(0x0f);
                self.byte(0xbe);
                self.modrm_rr(r, r);
            }
            (8, false) => self.movzx8(r, r),
            (16, true) => {
                self.rex(true, r, 0, r, false);
                self.byte(0x0f);
                self.byte(0xbf);
                self.modrm_rr(r, r);
            }
            (16, false) => {
                self.rex(true, r, 0, r, false);
                self.byte(0x0f);
                self.byte(0xb7);
                self.modrm_rr(r, r);
            }
            (32, true) => {
                // movsxd r64, r32
                self.rex(true, r, 0, r, false);
                self.byte(0x63);
                self.modrm_rr(r, r);
            }
            (32, false) => {
                // mov r32, r32 zero-extends
                self.rex(false, r, 0, r, false);
                self.byte(0x89);
                self.modrm_rr(r, r);
            }
            _ => {}
        }
    }
    fn load_ext(&mut self, dst: u8, base: u8, disp: i32, bytes: u8, signed: bool) {
        match (bytes, signed) {
            (1, s) => {
                self.rex(true, dst, 0, base, false);
                self.byte(0x0f);
                self.byte(if s { 0xbe } else { 0xb6 });
                self.modrm_mem(dst, base, disp);
            }
            (2, s) => {
                self.rex(true, dst, 0, base, false);
                self.byte(0x0f);
                self.byte(if s { 0xbf } else { 0xb7 });
                self.modrm_mem(dst, base, disp);
            }
            (4, true) => {
                self.rex(true, dst, 0, base, false);
                self.byte(0x63);
                self.modrm_mem(dst, base, disp);
            }
            (4, false) => {
                self.rex(false, dst, 0, base, false);
                self.byte(0x8b);
                self.modrm_mem(dst, base, disp);
            }
            _ => self.load64(dst, base, disp),
        }
    }
    fn store_n(&mut self, base: u8, disp: i32, src: u8, bytes: u8) {
        match bytes {
            1 => {
                self.rex(false, src, 0, base, src >= 4);
                self.byte(0x88);
                self.modrm_mem(src, base, disp);
            }
            2 => {
                self.byte(0x66);
                self.rex(false, src, 0, base, false);
                self.byte(0x89);
                self.modrm_mem(src, base, disp);
            }
            4 => {
                self.rex(false, src, 0, base, false);
                self.byte(0x89);
                self.modrm_mem(src, base, disp);
            }
            _ => self.store64(base, disp, src),
        }
    }
    fn push(&mut self, r: u8) {
        self.rex(false, 0, 0, r, false);
        self.byte(0x50 + (r & 7));
    }
    fn pop(&mut self, r: u8) {
        self.rex(false, 0, 0, r, false);
        self.byte(0x58 + (r & 7));
    }
    fn sub_rsp(&mut self, n: u32) {
        if n == 0 {
            return;
        }
        self.byte(0x48);
        self.byte(0x81);
        self.modrm_rr(5, RSP);
        self.u32(n);
    }
    fn call_r(&mut self, r: u8) {
        self.rex(false, 0, 0, r, false);
        self.byte(0xff);
        self.modrm_rr(2, r);
    }
    fn call_mem(&mut self, base: u8, disp: i32) {
        self.rex(false, 0, 0, base, false);
        self.byte(0xff);
        self.modrm_mem(2, base, disp);
    }
    fn jmp_mem(&mut self, base: u8, disp: i32) {
        self.rex(false, 0, 0, base, false);
        self.byte(0xff);
        self.modrm_mem(4, base, disp);
    }
    fn ret(&mut self) {
        self.byte(0xc3);
    }
    fn ud2(&mut self) {
        self.byte(0x0f);
        self.byte(0x0b);
    }
    /// jmp rel32; returns the patch position
    fn jmp32(&mut self) -> usize {
        self.byte(0xe9);
        let p = self.pos();
        self.u32(0);
        p
    }
    fn jcc32(&mut self, cc: u8) -> usize {
        self.byte(0x0f);
        self.byte(0x80 | cc);
        let p = self.pos();
        self.u32(0);
        p
    }
    fn patch(&mut self, at: usize, target: usize) {
        let rel = target as i64 - (at as i64 + 4);
        self.b[at..at + 4].copy_from_slice(&(rel as i32).to_le_bytes());
    }

    // ---- SSE: `pfx` F2 (sd) / F3 (ss) / 66

    fn sse_rr(&mut self, pfx: u8, op: u8, dst: u8, src: u8, w: bool) {
        if pfx != 0 {
            self.byte(pfx);
        }
        self.rex(w, dst, 0, src, false);
        self.byte(0x0f);
        self.byte(op);
        self.modrm_rr(dst, src);
    }
    fn sse_rm(&mut self, pfx: u8, op: u8, reg: u8, base: u8, disp: i32) {
        if pfx != 0 {
            self.byte(pfx);
        }
        self.rex(false, reg, 0, base, false);
        self.byte(0x0f);
        self.byte(op);
        self.modrm_mem(reg, base, disp);
    }
    fn movf_rr(&mut self, f64_: bool, dst: u8, src: u8) {
        if dst != src {
            // movaps xmm, xmm (full register copy)
            self.sse_rr(0, 0x28, dst, src, false);
        }
        let _ = f64_;
    }
    fn loadf(&mut self, f64_: bool, dst: u8, base: u8, disp: i32) {
        self.sse_rm(if f64_ { 0xf2 } else { 0xf3 }, 0x10, dst, base, disp);
    }
    fn storef(&mut self, f64_: bool, base: u8, disp: i32, src: u8) {
        self.sse_rm(if f64_ { 0xf2 } else { 0xf3 }, 0x11, src, base, disp);
    }
    /// movq xmm, r64 (or movd for 32 bits)
    fn movq_xr(&mut self, x: u8, r: u8, w: bool) {
        self.sse_rr(0x66, 0x6e, x, r, w);
    }
    /// movq r64, xmm (or movd)
    fn movq_rx(&mut self, r: u8, x: u8, w: bool) {
        // 66 REX.W 0F 7E /r : reg = xmm, rm = gpr
        self.byte(0x66);
        self.rex(w, x, 0, r, false);
        self.byte(0x0f);
        self.byte(0x7e);
        self.modrm_rr(x, r);
    }
    fn roundf(&mut self, f64_: bool, dst: u8, src: u8, mode: u8) {
        self.byte(0x66);
        self.rex(false, dst, 0, src, false);
        self.byte(0x0f);
        self.byte(0x3a);
        self.byte(if f64_ { 0x0b } else { 0x0a });
        self.modrm_rr(dst, src);
        self.byte(mode | 8);
    }
}

// condition codes
const CC_O: u8 = 0;
const CC_B: u8 = 2;
const CC_AE: u8 = 3;
const CC_E: u8 = 4;
const CC_NE: u8 = 5;
const CC_BE: u8 = 6;
const CC_A: u8 = 7;
const CC_P: u8 = 0xa;
const CC_NP: u8 = 0xb;
const CC_L: u8 = 0xc;
const CC_GE: u8 = 0xd;
const CC_LE: u8 = 0xe;
const CC_G: u8 = 0xf;

fn icc(c: Cond, signed: bool) -> u8 {
    match (c, signed) {
        (Cond::Eq, _) => CC_E,
        (Cond::Ne, _) => CC_NE,
        (Cond::Lt, true) => CC_L,
        (Cond::Le, true) => CC_LE,
        (Cond::Gt, true) => CC_G,
        (Cond::Ge, true) => CC_GE,
        (Cond::Lt, false) => CC_B,
        (Cond::Le, false) => CC_BE,
        (Cond::Gt, false) => CC_A,
        (Cond::Ge, false) => CC_AE,
    }
}

pub fn reg_config() -> RegConfig {
    let mut flt = Vec::new();
    for x in 2..14u8 {
        flt.push(x);
    }
    RegConfig { int_caller: CALLER_SAVED_ALLOC.to_vec(), int_callee: CALLEE_SAVED.to_vec(), flt_caller: flt, flt_callee: Vec::new() }
}

// ------------------------------------------------------------ code generation

struct Gen<'a> {
    a: Asm,
    f: &'a Func,
    al: Alloc,
    env: &'a Env,
    /// rbp offset base for spills/slots: [rbp - (saved + x)]
    saved: i32,
    slot_off: Vec<i32>,
    block_pos: Vec<usize>,
    fixups: Vec<(usize, u32)>,
    pc_map: Vec<(u32, u64)>,
    /// position (regalloc numbering) of the instruction being emitted
    cur_pos: u32,
    /// float constant pool: (patch offset, bits)
    fpool: Vec<(usize, u64)>,
    /// emission order successor of each block (u32::MAX for the last)
    next_block: Vec<u32>,
    frame: i32,
    text_len: u32,
    /// a single-use float constant whose use (the next FBin) reads it from the pool
    mem_const: Option<(VReg, u64)>,
    memconst: bool,
    /// C ABI body: its plan, per-argument buffer (rbp disp, 0 = none), result buffer,
    /// the cell keeping an indirect result address for rax
    cplan: Option<Plan>,
    cbuf: Vec<i32>,
    cret_buf: i32,
    cret_ptr: i32,
    /// rbp disp of the C call staging cells (8 bytes each, ascending)
    stage: i32,
}

impl<'a> Gen<'a> {
    fn spill_disp(&self, off: i32) -> i32 {
        -(self.saved + off)
    }

    /// Integer operand in a register (loads spills into `scratch`).
    fn use_i(&mut self, v: VReg, scratch: u8) -> u8 {
        match self.al.loc[v.0 as usize] {
            Loc::Reg(r) => r,
            Loc::Stack(o) => {
                let d = self.spill_disp(o);
                self.a.load64(scratch, RBP, d);
                scratch
            }
            _ => {
                self.a.mov_ri(scratch, 0);
                scratch
            }
        }
    }
    fn def_i(&self, v: VReg, scratch: u8) -> u8 {
        match self.al.loc[v.0 as usize] {
            Loc::Reg(r) => r,
            _ => scratch,
        }
    }
    fn fin_i(&mut self, v: VReg, r: u8) {
        if let Loc::Stack(o) = self.al.loc[v.0 as usize] {
            let d = self.spill_disp(o);
            self.a.store64(RBP, d, r);
        }
    }
    fn use_f(&mut self, v: VReg, scratch: u8) -> u8 {
        match self.al.loc[v.0 as usize] {
            Loc::Flt(x) => x,
            Loc::Stack(o) => {
                let d = self.spill_disp(o);
                let f64_ = self.f.vregs[v.0 as usize] == Cls::F64;
                self.a.loadf(f64_, scratch, RBP, d);
                scratch
            }
            _ => scratch,
        }
    }
    fn def_f(&self, v: VReg, scratch: u8) -> u8 {
        match self.al.loc[v.0 as usize] {
            Loc::Flt(x) => x,
            _ => scratch,
        }
    }
    fn fin_f(&mut self, v: VReg, x: u8) {
        if let Loc::Stack(o) = self.al.loc[v.0 as usize] {
            let d = self.spill_disp(o);
            let f64_ = self.f.vregs[v.0 as usize] == Cls::F64;
            self.a.storef(f64_, RBP, d, x);
        }
    }
    fn mloc(&self, v: VReg) -> Option<MLoc> {
        match self.al.loc[v.0 as usize] {
            Loc::Reg(r) => Some(MLoc::R(r)),
            Loc::Flt(x) => Some(MLoc::F(x)),
            Loc::Stack(o) => Some(MLoc::M(self.spill_disp(o))),
            Loc::None => None,
        }
    }

    /// One move between locations (memory is [rbp + disp], 8 bytes).
    fn mv(&mut self, d: MLoc, s: MLoc) {
        match (d, s) {
            (MLoc::R(a), MLoc::R(b)) => self.a.mov_rr(a, b),
            (MLoc::R(a), MLoc::M(o)) => self.a.load64(a, RBP, o),
            (MLoc::M(o), MLoc::R(b)) => self.a.store64(RBP, o, b),
            (MLoc::F(a), MLoc::F(b)) => self.a.movf_rr(true, a, b),
            (MLoc::F(a), MLoc::M(o)) => self.a.loadf(true, a, RBP, o),
            (MLoc::M(o), MLoc::F(b)) => self.a.storef(true, RBP, o, b),
            (MLoc::M(o), MLoc::M(p)) => {
                self.a.load64(R10, RBP, p);
                self.a.store64(RBP, o, R10);
            }
            (MLoc::R(a), MLoc::F(b)) => self.a.movq_rx(a, b, true),
            (MLoc::F(a), MLoc::R(b)) => self.a.movq_xr(a, b, true),
        }
    }

    fn gen(&mut self) {
        let reach = regalloc::reachable(self.f);
        // prologue
        self.a.push(RBP);
        self.a.mov_rr(RBP, RSP);
        for r in self.al.used_callee_int.clone() {
            self.a.push(r);
        }
        self.saved = 8 * self.al.used_callee_int.len() as i32;
        let mut off = self.al.spill_bytes as i32;
        for s in &self.f.slots {
            let al = s.align.max(1) as i32;
            off += s.size as i32;
            off = (off + al - 1) / al * al;
            self.slot_off.push(off);
        }
        off = (off + 7) / 8 * 8;
        // C ABI body: buffers rebuilding aggregate params from registers, the result buffer
        if let Some(sig) = &self.f.cabi {
            let plan = cabi::plan(sig, cabi::Target::SysV);
            for (k, a) in plan.args.iter().enumerate() {
                match (a, &sig.args[k]) {
                    (ArgPlan::Parts(_), CArg::Agg(ag)) => {
                        off += ((ag.size as i32 + 7) & !7) + 8;
                        self.cbuf.push(-(self.saved + off));
                    }
                    _ => self.cbuf.push(0),
                }
            }
            match (&plan.ret, &sig.ret) {
                (RetPlan::Parts(_), Some(ag)) => {
                    off += ((ag.size as i32 + 7) & !7) + 8;
                    self.cret_buf = -(self.saved + off);
                }
                (RetPlan::Indirect, Some(_)) => {
                    off += 8;
                    self.cret_ptr = -(self.saved + off);
                }
                _ => {}
            }
            self.cplan = Some(plan);
        }
        // staging cells for C calls (cell 0: the result address; then aggregate pieces)
        let mut cells = 0i32;
        for b in &self.f.blocks {
            for i in &b.insts {
                if let Inst::Call(Callee::CHost(_, sig) | Callee::CIndirect(_, sig) | Callee::CFn(_, sig), _, _) = i {
                    let plan = cabi::plan(sig, cabi::Target::SysV);
                    let mut n = 0i32;
                    for ap in &plan.args {
                        if let ArgPlan::Parts(ps) = ap {
                            n += ps.len() as i32;
                        }
                    }
                    if let RetPlan::Parts(ps) = &plan.ret {
                        n = n.max(ps.len() as i32);
                    }
                    cells = cells.max(1 + n);
                }
            }
        }
        off += 8 * cells;
        self.stage = -(self.saved + off);
        let mut frame = (off + 7) / 8 * 8;
        // keep rsp 16-aligned at calls
        while (self.saved + frame) % 16 != 0 {
            frame += 8;
        }
        self.frame = frame;
        self.stack_probe(frame);
        self.a.sub_rsp(frame as u32);
        if self.cplan.is_some() {
            self.c_prologue();
        } else {
            self.params_prologue();
        }
        self.blocks_and_pool(reach);
    }

    /// HotRust ABI params: ABI registers -> allocated locations (parallel move).
    fn params_prologue(&mut self) {
        let mut ni = 0;
        let mut nf = 0;
        let mut nstack = 0;
        let mut moves = Vec::new();
        for p in &self.f.params {
            let src = match self.f.vregs[p.0 as usize] {
                Cls::I if ni < 6 => {
                    ni += 1;
                    MLoc::R(INT_ARGS[ni - 1])
                }
                Cls::F32 | Cls::F64 if nf < 8 => {
                    nf += 1;
                    MLoc::F(nf as u8 - 1)
                }
                _ => {
                    // SysV: overflow arguments on the caller's stack, in order
                    nstack += 1;
                    MLoc::M(16 + 8 * (nstack - 1))
                }
            };
            if let Some(d) = self.mloc(*p) {
                moves.push((d, src));
            }
        }
        for (d, s2) in regalloc::seq_moves(&moves, R10, XS) {
            self.mv(d, s2);
        }
    }

    fn blocks_and_pool(&mut self, reach: Vec<bool>) {
        // blocks in order, skipping unreachable ones
        let nb = self.f.blocks.len();
        self.block_pos = vec![usize::MAX; nb];
        self.next_block = vec![u32::MAX; nb];
        let mut prev: Option<usize> = None;
        for bi in 0..nb {
            if reach[bi] {
                if let Some(p) = prev {
                    self.next_block[p] = bi as u32;
                }
                prev = Some(bi);
            }
        }
        for bi in 0..nb {
            if !reach[bi] {
                continue;
            }
            self.block_pos[bi] = self.a.pos();
            let b = &self.f.blocks[bi];
            let n = b.insts.len();
            // compare feeding the branch: fused into cmp + jcc
            let mut fused = false;
            if let Term::Branch(c, _, _) = &b.term {
                if n > 0 && self.al.uses[c.0 as usize] == 1 {
                    match &b.insts[n - 1] {
                        Inst::ICmp(_, _, d, _, _) | Inst::ICmpI(_, _, d, _, _) | Inst::FCmp(_, _, d, _, _) if d == c => fused = true,
                        _ => {}
                    }
                }
            }
            for (ii, inst) in b.insts.iter().enumerate() {
                self.pc_map.push((self.a.pos() as u32, b.pos[ii]));
                self.cur_pos = self.al.inst_pos(bi, ii);
                if fused && ii == n - 1 {
                    break;
                }
                // Fconst used once by the next FBin as its second operand (or either
                // operand of a commutative op, when the other one is not the same const)
                if let Inst::Fconst(cd, bits, _) = inst {
                    if self.memconst && self.al.uses[cd.0 as usize] == 1 && ii + 1 < n && !(fused && ii + 1 == n - 1) {
                        if let Inst::FBin(op @ (FOp::Add | FOp::Sub | FOp::Mul | FOp::Div), _, _, x, y) = &b.insts[ii + 1] {
                            let comm = matches!(op, FOp::Add | FOp::Mul);
                            if (y == cd && x != cd) || (comm && x == cd && y != cd) {
                                self.mem_const = Some((*cd, *bits));
                                continue;
                            }
                        }
                    }
                }
                self.inst(inst);
            }
            self.pc_map.push((self.a.pos() as u32, b.term_pos));
            let fuse_inst = if fused { Some(b.insts[n - 1].clone()) } else { None };
            self.term(&b.term, bi, fuse_inst);
        }
        for (at, t) in std::mem::take(&mut self.fixups) {
            let target = self.block_pos[t as usize];
            self.a.patch(at, target);
        }
        self.text_len = self.a.pos() as u32;
        // float constant pool
        if !self.fpool.is_empty() {
            while self.a.pos() % 8 != 0 {
                self.a.byte(0xcc);
            }
            let mut placed: Vec<(u64, usize)> = Vec::new();
            for (at, bits) in std::mem::take(&mut self.fpool) {
                let mut addr = usize::MAX;
                for (b2, a2) in &placed {
                    if *b2 == bits {
                        addr = *a2;
                    }
                }
                if addr == usize::MAX {
                    addr = self.a.pos();
                    self.a.u64(bits);
                    placed.push((bits, addr));
                }
                self.a.patch(at, addr);
            }
        }
    }

    /// Prologue of an `extern "C"` body (SysV): aggregate params are rebuilt in frame
    /// buffers (their address is the param), memory-class ones are addressed in the
    /// caller's argument area, an indirect result pointer is kept for rax.
    fn c_prologue(&mut self) {
        let plan = self.cplan.clone().unwrap();
        let sig = self.f.cabi.clone().unwrap();
        let params = self.f.params.clone();
        let has_ret = sig.ret.is_some();
        let pargs: Vec<VReg> = if has_ret { params[1..].to_vec() } else { params.clone() };
        for (k, ap) in plan.args.iter().enumerate() {
            if let ArgPlan::Parts(parts) = ap {
                let buf = self.cbuf[k];
                for pt in parts {
                    match pt.loc {
                        PLoc::Int(r) => self.a.store64(RBP, buf + pt.off as i32, INT_ARGS[r as usize]),
                        PLoc::Flt(x) => self.a.storef(true, RBP, buf + pt.off as i32, x),
                        PLoc::Stack(_) => {}
                    }
                }
            }
        }
        let mut moves = Vec::new();
        if let (RetPlan::Indirect, true) = (&plan.ret, has_ret) {
            self.a.store64(RBP, self.cret_ptr, RDI);
            if let Some(d) = self.mloc(params[0]) {
                moves.push((d, MLoc::R(RDI)));
            }
        }
        for (k, ap) in plan.args.iter().enumerate() {
            let d = match self.mloc(pargs[k]) {
                Some(d) => d,
                None => continue,
            };
            match ap {
                ArgPlan::Scalar(PLoc::Int(r), _) | ArgPlan::ByRef(PLoc::Int(r)) => moves.push((d, MLoc::R(INT_ARGS[*r as usize]))),
                ArgPlan::Scalar(PLoc::Flt(x), _) => moves.push((d, MLoc::F(*x))),
                _ => {}
            }
        }
        for (d, s2) in regalloc::seq_moves(&moves, R10, XS) {
            self.mv(d, s2);
        }
        for (k, ap) in plan.args.iter().enumerate() {
            let p = pargs[k];
            if self.al.loc[p.0 as usize] == Loc::None {
                continue;
            }
            let m = match &sig.args[k] {
                CArg::Scalar(m) => *m,
                CArg::Agg(_) => Mem::Int(8, false),
            };
            match ap {
                ArgPlan::Scalar(PLoc::Stack(o), _) | ArgPlan::ByRef(PLoc::Stack(o)) => {
                    let disp = 16 + *o as i32;
                    match m {
                        Mem::Int(n, sg) => {
                            let rd = self.def_i(p, R10);
                            self.a.load_ext(rd, RBP, disp, n, sg);
                            self.fin_i(p, rd);
                        }
                        _ => {
                            let xd = self.def_f(p, XS);
                            self.a.loadf(m == Mem::F64, xd, RBP, disp);
                            self.fin_f(p, xd);
                        }
                    }
                }
                ArgPlan::Parts(_) => {
                    let rd = self.def_i(p, R10);
                    self.a.lea(rd, RBP, self.cbuf[k]);
                    self.fin_i(p, rd);
                }
                ArgPlan::Stack(o, _) => {
                    let rd = self.def_i(p, R10);
                    self.a.lea(rd, RBP, 16 + *o as i32);
                    self.fin_i(p, rd);
                }
                ArgPlan::Scalar(PLoc::Int(_), _) => {
                    // C leaves the upper bits of narrow integer arguments undefined
                    if let Mem::Int(n, sg) = m {
                        if n < 8 {
                            let r = self.use_i(p, R10);
                            self.a.extend(r, n * 8, sg);
                            self.fin_i(p, r);
                        }
                    }
                }
                _ => {}
            }
        }
        if let (RetPlan::Parts(_), true) = (&plan.ret, has_ret) {
            if self.al.loc[params[0].0 as usize] != Loc::None {
                let rd = self.def_i(params[0], R10);
                self.a.lea(rd, RBP, self.cret_buf);
                self.fin_i(params[0], rd);
            }
        }
    }

    /// Ret of an `extern "C"` body: an aggregate result into rax/rdx/xmm0/xmm1, or the
    /// indirect result address into rax.
    fn c_ret(&mut self) {
        let plan = match &self.cplan {
            Some(p) => p.clone(),
            None => return,
        };
        match &plan.ret {
            RetPlan::Parts(parts) => {
                for pt in parts {
                    let disp = self.cret_buf + pt.off as i32;
                    match pt.loc {
                        PLoc::Int(r) => self.a.load64(if r == 0 { RAX } else { RDX }, RBP, disp),
                        PLoc::Flt(x) => self.a.loadf(true, x, RBP, disp),
                        PLoc::Stack(_) => {}
                    }
                }
            }
            RetPlan::Indirect if self.f.cabi.as_ref().map(|s| s.ret.is_some()).unwrap_or(false) => self.a.load64(RAX, RBP, self.cret_ptr),
            _ => {}
        }
    }

    /// Touches every page of a frame larger than a page, top down (guard page first).
    fn stack_probe(&mut self, frame: i32) {
        const PAGE: i32 = 4096;
        if frame <= PAGE {
            return;
        }
        // mov r11, rsp
        self.a.mov_rr(R11, RSP);
        let n = frame / PAGE;
        let body = |a: &mut Asm| {
            // sub r11, 4096 ; or byte [r11], 0
            a.b.extend_from_slice(&[0x49, 0x81, 0xeb, 0x00, 0x10, 0x00, 0x00]);
            a.b.extend_from_slice(&[0x41, 0x80, 0x0b, 0x00]);
        };
        if n <= 16 {
            for _ in 0..n {
                body(&mut self.a);
            }
            return;
        }
        self.a.mov_ri(RAX, n as i64);
        let top = self.a.pos();
        body(&mut self.a);
        // dec rax ; jnz top
        self.a.b.extend_from_slice(&[0x48, 0xff, 0xc8]);
        let p = self.a.jcc32(CC_NE);
        self.a.patch(p, top);
    }

    /// Copies `size` bytes [src + so] -> [dst + do_] through rax.
    fn copy_bytes(&mut self, dst: u8, do_: i32, src: u8, so: i32, size: u32) {
        let mut o = 0i32;
        let size = size as i32;
        while o < size {
            let n = if size - o >= 8 {
                8
            } else if size - o >= 4 {
                4
            } else if size - o >= 2 {
                2
            } else {
                1
            };
            self.a.load_ext(RAX, src, so + o, n as u8, false);
            self.a.store_n(dst, do_ + o, RAX, n as u8);
            o += n;
        }
    }

    /// A call under the C ABI (SysV).
    fn c_call(&mut self, c: &Callee, sig: &CSig, args: &[VReg], rets: &[VReg]) {
        let plan = cabi::plan(sig, cabi::Target::SysV);
        let saves = self.al.saves.get(&self.cur_pos).cloned().unwrap_or_default();
        for (v, slot) in &saves {
            let d = self.spill_disp(*slot);
            if let Some(src) = self.mloc(*v) {
                self.mv(MLoc::M(d), src);
            }
        }
        let has_ret = sig.ret.is_some();
        let cargs: Vec<VReg> = if has_ret { args[1..].to_vec() } else { args.to_vec() };
        let st = self.stage;
        if has_ret {
            let r = self.use_i(args[0], R10);
            self.a.store64(RBP, st, r);
        }
        if let Callee::CIndirect(v, _) = c {
            let r = self.use_i(*v, R11);
            self.a.mov_rr(R11, r);
        }
        let stack_bytes = plan.stack_bytes;
        if stack_bytes > 0 {
            self.a.sub_rsp(stack_bytes);
        }
        let mut moves = Vec::new();
        let mut cell = 1i32;
        for (k, ap) in plan.args.iter().enumerate() {
            let v = cargs[k];
            match ap {
                ArgPlan::Scalar(PLoc::Int(r), _) | ArgPlan::ByRef(PLoc::Int(r)) => {
                    if let Some(s2) = self.mloc(v) {
                        moves.push((MLoc::R(INT_ARGS[*r as usize]), s2));
                    }
                }
                ArgPlan::Scalar(PLoc::Flt(x), _) => {
                    if let Some(s2) = self.mloc(v) {
                        moves.push((MLoc::F(*x), s2));
                    }
                }
                ArgPlan::Scalar(PLoc::Stack(o), _) | ArgPlan::ByRef(PLoc::Stack(o)) => {
                    if self.f.vregs[v.0 as usize] == Cls::I {
                        let r = self.use_i(v, R10);
                        self.a.store64(RSP, *o as i32, r);
                    } else {
                        let x = self.use_f(v, XS);
                        self.a.storef(true, RSP, *o as i32, x);
                    }
                }
                ArgPlan::ByRef(PLoc::Flt(_)) => {}
                ArgPlan::Parts(parts) => {
                    let ra = self.use_i(v, R10);
                    for pt in parts {
                        let cd = st + 8 * cell;
                        self.copy_bytes(RBP, cd, ra, pt.off as i32, pt.size);
                        match pt.loc {
                            PLoc::Int(r) => moves.push((MLoc::R(INT_ARGS[r as usize]), MLoc::M(cd))),
                            PLoc::Flt(x) => moves.push((MLoc::F(x), MLoc::M(cd))),
                            PLoc::Stack(_) => {}
                        }
                        cell += 1;
                    }
                }
                ArgPlan::Stack(o, size) => {
                    let ra = self.use_i(v, R10);
                    self.copy_bytes(RSP, *o as i32, ra, 0, *size);
                }
            }
        }
        if let (RetPlan::Indirect, true) = (&plan.ret, has_ret) {
            moves.push((MLoc::R(RDI), MLoc::M(st)));
        }
        for (d, s2) in regalloc::seq_moves(&moves, R10, XS) {
            self.mv(d, s2);
        }
        match c {
            Callee::CFn(id, _) => {
                let slot = self.env.table_base + *id as u64 * 8;
                self.a.mov_ri(RAX, slot as i64);
                self.a.call_mem(RAX, 0);
            }
            Callee::CHost(addr, _) => {
                self.a.mov_ri(R11, *addr as i64);
                self.a.mov_ri(RAX, plan.n_flt as i64);
                self.a.call_r(R11);
            }
            _ => {
                self.a.mov_ri(RAX, plan.n_flt as i64);
                self.a.call_r(R11);
            }
        }
        if stack_bytes > 0 {
            // add rsp, imm32
            self.a.byte(0x48);
            self.a.byte(0x81);
            self.a.modrm_rr(0, RSP);
            self.a.u32(stack_bytes);
        }
        if let RetPlan::Parts(parts) = &plan.ret {
            for (j, pt) in parts.iter().enumerate() {
                let cd = st + 8 * (1 + j as i32);
                match pt.loc {
                    PLoc::Int(r) => self.a.store64(RBP, cd, if r == 0 { RAX } else { RDX }),
                    PLoc::Flt(x) => self.a.storef(true, RBP, cd, x),
                    PLoc::Stack(_) => {}
                }
            }
        }
        let mut ri = 0;
        let mut rf = 0;
        let mut ret_moves = Vec::new();
        for r in rets {
            let src = match self.f.vregs[r.0 as usize] {
                Cls::I => {
                    ri += 1;
                    MLoc::R(if ri == 1 { RAX } else { RDX })
                }
                _ => {
                    rf += 1;
                    MLoc::F(rf as u8 - 1)
                }
            };
            if let Some(d) = self.mloc(*r) {
                ret_moves.push((d, src));
            }
        }
        for (d, s2) in regalloc::seq_moves(&ret_moves, R10, XS) {
            self.mv(d, s2);
        }
        for (v, slot) in &saves {
            let sd = self.spill_disp(*slot);
            if let Some(dst) = self.mloc(*v) {
                self.mv(dst, MLoc::M(sd));
            }
        }
        if let RetPlan::Parts(parts) = &plan.ret {
            self.a.load64(R10, RBP, st);
            for (j, pt) in parts.iter().enumerate() {
                let cd = st + 8 * (1 + j as i32);
                self.copy_bytes(R10, pt.off as i32, RBP, cd, pt.size);
            }
        }
    }

    /// `[lock] op [base], reg` with the operand width of `bytes` (0F-prefixed when `ext`).
    fn mem_rw(&mut self, lock: bool, ext: bool, op8: u8, bytes: u8, reg: u8, base: u8) {
        if lock {
            self.a.byte(0xf0);
        }
        if bytes == 2 {
            self.a.byte(0x66);
        }
        self.a.rex(bytes == 8, reg, 0, base, bytes == 1 && reg >= 4);
        if ext {
            self.a.byte(0x0f);
        }
        self.a.byte(if bytes == 1 { op8 } else { op8 + 1 });
        self.a.modrm_mem(reg, base, 0);
    }

    fn atomic(&mut self, i: &Inst) {
        let width = |m: &Mem| -> (u8, bool) {
            match m {
                Mem::Int(n, s) => (*n, *s),
                _ => (8, false),
            }
        };
        match i {
            Inst::AtomicLoad(m, _, d, a) => {
                // x86 loads are acquire; SeqCst stores are xchg, so a plain load suffices
                let (n, sg) = width(m);
                let rb = self.use_i(*a, R11);
                let rd = self.def_i(*d, R10);
                self.a.load_ext(rd, rb, 0, n, sg);
                self.fin_i(*d, rd);
            }
            Inst::AtomicStore(m, o, a, v) => {
                let (n, _) = width(m);
                let rb = self.use_i(*a, R11);
                let rv = self.use_i(*v, R10);
                if *o == AtomOrd::SeqCst {
                    self.a.mov_rr(RAX, rv);
                    // xchg [rb], rax (locked implicitly)
                    self.mem_rw(false, false, 0x86, n, RAX, rb);
                } else {
                    self.a.store_n(rb, 0, rv, n);
                }
            }
            Inst::AtomicRmw(op, m, _, d, a, v) => {
                let (n, sg) = width(m);
                let bits = n * 8;
                let ra = self.use_i(*a, R11);
                let rv = self.use_i(*v, R10);
                match op {
                    RmwOp::Xchg => {
                        self.a.mov_rr(RAX, rv);
                        self.mem_rw(false, false, 0x86, n, RAX, ra);
                    }
                    RmwOp::Add | RmwOp::Sub => {
                        self.a.mov_rr(RAX, rv);
                        if *op == RmwOp::Sub {
                            self.a.grp3(3, RAX);
                        }
                        // lock xadd [ra], rax
                        self.mem_rw(true, true, 0xc0, n, RAX, ra);
                    }
                    _ => {
                        // rax = old; loop { rcx = f(rax, v); lock cmpxchg [ra], rcx }
                        self.a.load_ext(RAX, ra, 0, n, false);
                        let top = self.a.pos();
                        self.a.mov_rr(RCX, RAX);
                        match op {
                            RmwOp::And => self.a.alu_rr(0x21, RCX, rv),
                            RmwOp::Or => self.a.alu_rr(0x09, RCX, rv),
                            RmwOp::Xor => self.a.alu_rr(0x31, RCX, rv),
                            RmwOp::Nand => {
                                self.a.alu_rr(0x21, RCX, rv);
                                self.a.grp3(2, RCX);
                            }
                            _ => {
                                // max/min: compare the extended old value with v, cmov v in
                                let signed = matches!(op, RmwOp::Max | RmwOp::Min);
                                if bits < 64 {
                                    self.a.extend(RCX, bits, signed);
                                }
                                self.a.alu_rr(0x39, RCX, rv);
                                let cc = match op {
                                    RmwOp::Max => CC_L,
                                    RmwOp::Min => CC_G,
                                    RmwOp::UMax => CC_B,
                                    _ => CC_A,
                                };
                                // cmovcc rcx, rv
                                self.a.rex(true, RCX, 0, rv, false);
                                self.a.byte(0x0f);
                                self.a.byte(0x40 | cc);
                                self.a.modrm_rr(RCX, rv);
                            }
                        }
                        self.mem_rw(true, true, 0xb0, n, RCX, ra);
                        let p = self.a.jcc32(CC_NE);
                        self.a.patch(p, top);
                    }
                }
                if bits < 64 {
                    self.a.extend(RAX, bits, sg);
                }
                let rd = self.def_i(*d, R10);
                self.a.mov_rr(rd, RAX);
                self.fin_i(*d, rd);
            }
            Inst::AtomicCas(m, _, _, d, a, e, nw) => {
                let (n, sg) = width(m);
                let ra = self.use_i(*a, R11);
                let re = self.use_i(*e, R10);
                let rn = self.use_i(*nw, RDX);
                self.a.mov_rr(RAX, re);
                self.mem_rw(true, true, 0xb0, n, rn, ra);
                if n < 8 {
                    self.a.extend(RAX, n * 8, sg);
                }
                let rd = self.def_i(*d, R10);
                self.a.mov_rr(rd, RAX);
                self.fin_i(*d, rd);
            }
            Inst::Fence(o, single) => {
                if !*single && *o == AtomOrd::SeqCst {
                    // mfence
                    self.a.b.extend_from_slice(&[0x0f, 0xae, 0xf0]);
                }
            }
            _ => {}
        }
    }

    /// d = this thread's thread-local block + off: `mov r11, fs:[key]` (the runtime's
    /// static-TLS block pointer), slow path `tls_slow` when it is still null.
    fn tls_addr(&mut self, d: VReg, off: u32) {
        let k = self.env.tls_key as i64 as i32;
        self.a.b.extend_from_slice(&[0x64, 0x4c, 0x8b, 0x1c, 0x25]);
        self.a.u32(k as u32);
        self.a.test_rr(R11, R11);
        let p = self.a.jcc32(CC_NE);
        self.a.mov_ri(RAX, self.env.tls_slow as i64);
        self.a.call_r(RAX);
        let skip = self.a.pos();
        self.a.patch(p, skip);
        let rd = self.def_i(d, R10);
        self.a.lea(rd, R11, off as i32);
        self.fin_i(d, rd);
    }

    fn epilogue(&mut self) {
        if self.frame != 0 {
            self.a.lea(RSP, RBP, -self.saved);
        }
        let saved = self.al.used_callee_int.clone();
        let mut i = saved.len();
        while i > 0 {
            i -= 1;
            self.a.pop(saved[i]);
        }
        self.a.pop(RBP);
        self.a.ret();
    }

    fn jump_to(&mut self, target: u32, bi: usize) {
        if self.next_block[bi] != target {
            let p = self.a.jmp32();
            self.fixups.push((p, target));
        }
    }

    fn jcc_to(&mut self, cc: u8, target: u32) {
        let p = self.a.jcc32(cc);
        self.fixups.push((p, target));
    }

    /// cmp r, imm32 (sign-extended)
    fn cmp_imm(&mut self, r: u8, k: i64) {
        self.a.rex(true, 0, 0, r, false);
        if (-128..128).contains(&k) {
            self.a.byte(0x83);
            self.a.modrm_rr(7, r);
            self.a.byte(k as i8 as u8);
        } else {
            self.a.byte(0x81);
            self.a.modrm_rr(7, r);
            self.a.u32(k as i32 as u32);
        }
    }

    fn term(&mut self, t: &Term, bi: usize, fused: Option<Inst>) {
        match t {
            Term::Jump(target) => self.jump_to(*target, bi),
            Term::Branch(c, tt, ff) => {
                let next = self.next_block[bi];
                // integer condition code for `c` true
                let icond: Option<u8> = match &fused {
                    Some(Inst::ICmp(cond, signed, _, x, y)) => {
                        let rx = self.use_i(*x, R10);
                        let ry = self.use_i(*y, R11);
                        self.a.alu_rr(0x39, rx, ry);
                        Some(icc(*cond, *signed))
                    }
                    Some(Inst::ICmpI(cond, signed, _, x, k)) => {
                        let rx = self.use_i(*x, R10);
                        self.cmp_imm(rx, *k);
                        Some(icc(*cond, *signed))
                    }
                    Some(Inst::FCmp(cond, f64_, _, x, y)) => {
                        let xa = self.use_f(*x, XS);
                        let xb = self.use_f(*y, XS2);
                        let pfx = if *f64_ { 0x66 } else { 0 };
                        match cond {
                            Cond::Lt | Cond::Le => {
                                self.a.sse_rr(pfx, 0x2e, xb, xa, false);
                                Some(if *cond == Cond::Lt { CC_A } else { CC_AE })
                            }
                            Cond::Gt | Cond::Ge => {
                                self.a.sse_rr(pfx, 0x2e, xa, xb, false);
                                Some(if *cond == Cond::Gt { CC_A } else { CC_AE })
                            }
                            Cond::Eq => {
                                self.a.sse_rr(pfx, 0x2e, xa, xb, false);
                                // unordered -> false
                                self.jcc_to(CC_P, *ff);
                                Some(CC_E)
                            }
                            Cond::Ne => {
                                self.a.sse_rr(pfx, 0x2e, xa, xb, false);
                                self.jcc_to(CC_P, *tt);
                                Some(CC_NE)
                            }
                        }
                    }
                    _ => {
                        let r = self.use_i(*c, R10);
                        self.a.test_rr(r, r);
                        Some(CC_NE)
                    }
                };
                let cc = icond.unwrap();
                if *tt == next {
                    // fall into the true block: branch on the inverted condition
                    self.jcc_to(cc ^ 1, *ff);
                } else {
                    self.jcc_to(cc, *tt);
                    self.jump_to(*ff, bi);
                }
            }
            Term::Ret(vals) => {
                self.c_ret();
                let mut ni = 0;
                let mut nf = 0;
                let mut moves = Vec::new();
                for v in vals {
                    let dst = match self.f.vregs[v.0 as usize] {
                        Cls::I => {
                            ni += 1;
                            MLoc::R(if ni == 1 { RAX } else { RDX })
                        }
                        _ => {
                            nf += 1;
                            MLoc::F(nf as u8 - 1)
                        }
                    };
                    if let Some(s2) = self.mloc(*v) {
                        moves.push((dst, s2));
                    }
                }
                for (d, s2) in regalloc::seq_moves(&moves, R10, XS) {
                    self.mv(d, s2);
                }
                self.epilogue();
            }
            Term::Unreachable => self.a.ud2(),
        }
    }

    fn inst(&mut self, i: &Inst) {
        match i {
            Inst::AtomicLoad(..) | Inst::AtomicStore(..) | Inst::AtomicRmw(..) | Inst::AtomicCas(..) | Inst::Fence(..) => self.atomic(i),
            Inst::TlsAddr(d, off) => self.tls_addr(*d, *off),
            Inst::IBinI(op, it, d, a, k) => self.ibin_imm(*op, *it, *d, *a, *k),
            Inst::ICmpI(c, signed, d, a, k) => {
                let ra = self.use_i(*a, R10);
                self.cmp_imm(ra, *k);
                let rd = self.def_i(*d, R10);
                self.a.setcc(icc(*c, *signed), rd);
                self.a.movzx8(rd, rd);
                self.fin_i(*d, rd);
            }
            Inst::Iconst(d, v) => {
                let r = self.def_i(*d, R10);
                self.a.mov_ri(r, *v);
                self.fin_i(*d, r);
            }
            Inst::Fconst(d, bits, f64_) => {
                let x = self.def_f(*d, XS);
                if *bits == 0 {
                    // xorps x, x
                    self.a.sse_rr(0, 0x57, x, x, false);
                } else {
                    // movsd/movss x, [rip + pool]
                    self.a.byte(if *f64_ { 0xf2 } else { 0xf3 });
                    self.a.rex(false, x, 0, 0, false);
                    self.a.byte(0x0f);
                    self.a.byte(0x10);
                    self.a.byte(((x & 7) << 3) | 5);
                    let at = self.a.pos();
                    self.a.u32(0);
                    self.fpool.push((at, *bits));
                }
                self.fin_f(*d, x);
            }
            Inst::Mov(d, s) => match self.f.vregs[d.0 as usize] {
                Cls::I => {
                    let rs = self.use_i(*s, R10);
                    let rd = self.def_i(*d, R10);
                    self.a.mov_rr(rd, rs);
                    self.fin_i(*d, rd);
                }
                c => {
                    let xs = self.use_f(*s, XS);
                    let xd = self.def_f(*d, XS);
                    self.a.movf_rr(c == Cls::F64, xd, xs);
                    self.fin_f(*d, xd);
                }
            },
            Inst::IBin(op, it, d, a, b) => self.ibin(*op, *it, *d, *a, *b),
            Inst::INeg(it, d, a) | Inst::INot(it, d, a) => {
                let ra = self.use_i(*a, R10);
                let rd = self.def_i(*d, R10);
                self.a.mov_rr(rd, ra);
                let ext = if matches!(i, Inst::INeg(..)) { 3 } else { 2 };
                self.a.grp3(ext, rd);
                self.a.extend(rd, it.bits, it.signed);
                self.fin_i(*d, rd);
            }
            Inst::ICmp(c, signed, d, a, b) => {
                let ra = self.use_i(*a, R10);
                let rb = self.use_i(*b, R11);
                self.a.alu_rr(0x39, ra, rb);
                let rd = self.def_i(*d, R10);
                self.a.setcc(icc(*c, *signed), rd);
                self.a.movzx8(rd, rd);
                self.fin_i(*d, rd);
            }
            Inst::FBin(op @ (FOp::Min | FOp::Max), f64_, d, a, b) => {
                // Rust semantics: if one operand is NaN the other is returned
                let xa = self.use_f(*a, XS);
                let xb = self.use_f(*b, XS2);
                let pfx = if *f64_ { 0xf2 } else { 0xf3 };
                let opc = if *op == FOp::Min { 0x5d } else { 0x5f };
                // XS2 <- b, XS <- a (copies first: d may alias either)
                if xb != XS2 {
                    self.a.movf_rr(true, XS2, xb);
                }
                if xa != XS {
                    self.a.movf_rr(true, XS, xa);
                }
                let xd = self.def_f(*d, XS);
                // tmp = op(a, b) yields b when either is NaN; then pick a when b is NaN
                let tmp = 1u8; // xmm0/xmm1 are never allocated: free inside one instruction
                self.a.movf_rr(true, tmp, XS);
                self.a.sse_rr(pfx, opc, tmp, XS2, false); // tmp = op(a, b) (b if any NaN)
                // mask = b unordered with itself (b is NaN)
                self.a.movf_rr(true, 0, XS2);
                self.a.sse_rr(if *f64_ { 0xf2 } else { 0xf3 }, 0xc2, 0, 0, false);
                self.a.byte(3); // cmpunord
                // result = mask ? a : tmp  =  (a & mask) | (tmp & !mask)
                self.a.sse_rr(0x66, 0x54, XS, 0, false); // andpd XS(a), mask
                self.a.sse_rr(0x66, 0x55, 0, tmp, false); // andnpd mask -> !mask & tmp
                self.a.sse_rr(0x66, 0x56, 0, XS, false); // orpd
                self.a.movf_rr(true, xd, 0);
                self.fin_f(*d, xd);
            }
            Inst::FBin(op, f64_, d, a, b) if self.mem_const.is_some() => {
                // one operand is a constant folded into a [rip + pool] memory operand
                let (cv, bits) = self.mem_const.take().unwrap();
                let pfx = if *f64_ { 0xf2 } else { 0xf3 };
                let opc = match op {
                    FOp::Add => 0x58,
                    FOp::Mul => 0x59,
                    FOp::Sub => 0x5c,
                    FOp::Div => 0x5e,
                    FOp::Min => 0x5d,
                    FOp::Max => 0x5f,
                };
                let other = if *b == cv { *a } else { *b };
                let xo = self.use_f(other, XS);
                let xd = self.def_f(*d, XS);
                self.a.movf_rr(*f64_, xd, xo);
                self.a.byte(pfx);
                self.a.rex(false, xd, 0, 0, false);
                self.a.byte(0x0f);
                self.a.byte(opc);
                self.a.byte(((xd & 7) << 3) | 5);
                let at = self.a.pos();
                self.a.u32(0);
                self.fpool.push((at, bits));
                self.fin_f(*d, xd);
            }
            Inst::FBin(op, f64_, d, a, b) => {
                let xa = self.use_f(*a, XS);
                let xb = self.use_f(*b, XS2);
                let xd = self.def_f(*d, XS);
                let pfx = if *f64_ { 0xf2 } else { 0xf3 };
                let opc = match op {
                    FOp::Add => 0x58,
                    FOp::Mul => 0x59,
                    FOp::Sub => 0x5c,
                    FOp::Div => 0x5e,
                    FOp::Min => 0x5d,
                    FOp::Max => 0x5f,
                };
                if xd == xb && xd != xa && (*op == FOp::Add || *op == FOp::Mul) {
                    // commutative: d already holds b
                    self.a.sse_rr(pfx, opc, xd, xa, false);
                } else if xd == xb && xd != xa {
                    // compute in scratch to keep b intact
                    self.a.movf_rr(*f64_, XS, xa);
                    self.a.sse_rr(pfx, opc, XS, xb, false);
                    self.a.movf_rr(*f64_, xd, XS);
                } else {
                    self.a.movf_rr(*f64_, xd, xa);
                    self.a.sse_rr(pfx, opc, xd, xb, false);
                }
                self.fin_f(*d, xd);
            }
            Inst::FUnary(op, f64_, d, a) => {
                let xa = self.use_f(*a, XS);
                let xd = self.def_f(*d, XS);
                match op {
                    FUn::Neg | FUn::Abs => {
                        let mask: u64 = match (op, f64_) {
                            (FUn::Neg, true) => 0x8000_0000_0000_0000,
                            (FUn::Neg, false) => 0x8000_0000,
                            (_, true) => 0x7fff_ffff_ffff_ffff,
                            (_, false) => 0x7fff_ffff,
                        };
                        self.a.mov_ri(R10, mask as i64);
                        self.a.movq_xr(XS2, R10, true);
                        self.a.movf_rr(*f64_, xd, xa);
                        // xorpd / andpd (66 0F 57 / 54)
                        let opc = if *op == FUn::Neg { 0x57 } else { 0x54 };
                        self.a.sse_rr(0x66, opc, xd, XS2, false);
                    }
                    FUn::Sqrt => {
                        self.a.sse_rr(if *f64_ { 0xf2 } else { 0xf3 }, 0x51, xd, xa, false);
                    }
                    FUn::Floor => self.a.roundf(*f64_, xd, xa, 1),
                    FUn::Ceil => self.a.roundf(*f64_, xd, xa, 2),
                    FUn::Trunc => self.a.roundf(*f64_, xd, xa, 3),
                    FUn::RoundEven => self.a.roundf(*f64_, xd, xa, 0),
                }
                self.fin_f(*d, xd);
            }
            Inst::FCmp(c, f64_, d, a, b) => {
                let xa = self.use_f(*a, XS);
                let xb = self.use_f(*b, XS2);
                let pfx = if *f64_ { 0x66 } else { 0 };
                let rd = self.def_i(*d, R10);
                // ucomisd/ucomiss: 66 0F 2E / 0F 2E
                match c {
                    Cond::Lt | Cond::Le => {
                        self.a.sse_rr(pfx, 0x2e, xb, xa, false);
                        self.a.setcc(if *c == Cond::Lt { CC_A } else { CC_AE }, rd);
                        self.a.movzx8(rd, rd);
                    }
                    Cond::Gt | Cond::Ge => {
                        self.a.sse_rr(pfx, 0x2e, xa, xb, false);
                        self.a.setcc(if *c == Cond::Gt { CC_A } else { CC_AE }, rd);
                        self.a.movzx8(rd, rd);
                    }
                    Cond::Eq => {
                        self.a.sse_rr(pfx, 0x2e, xa, xb, false);
                        self.a.setcc(CC_E, rd);
                        self.a.setcc(CC_NP, R11);
                        self.a.movzx8(rd, rd);
                        self.a.movzx8(R11, R11);
                        self.a.alu_rr(0x21, rd, R11);
                    }
                    Cond::Ne => {
                        self.a.sse_rr(pfx, 0x2e, xa, xb, false);
                        self.a.setcc(CC_NE, rd);
                        self.a.setcc(CC_P, R11);
                        self.a.movzx8(rd, rd);
                        self.a.movzx8(R11, R11);
                        self.a.alu_rr(0x09, rd, R11);
                    }
                }
                self.fin_i(*d, rd);
            }
            Inst::Conv(c, d, s) => self.conv(*c, *d, *s),
            Inst::LoadX(m, d, base, idx, sh, off) => {
                let ri = self.use_i(*idx, R10);
                let ri = if ri == R10 {
                    self.a.mov_rr(RAX, R10);
                    RAX
                } else {
                    ri
                };
                let rb = self.use_i(*base, R11);
                match m {
                    Mem::Int(n, signed) => {
                        let rd = self.def_i(*d, R10);
                        self.a.sib = Some((ri, *sh));
                        self.a.load_ext(rd, rb, *off, *n, *signed);
                        self.fin_i(*d, rd);
                    }
                    Mem::F32 | Mem::F64 => {
                        let xd = self.def_f(*d, XS);
                        self.a.sib = Some((ri, *sh));
                        self.a.loadf(*m == Mem::F64, xd, rb, *off);
                        self.fin_f(*d, xd);
                    }
                }
            }
            Inst::StoreX(m, base, idx, sh, off, src) => {
                let ri = self.use_i(*idx, R10);
                let ri = if ri == R10 {
                    self.a.mov_rr(RAX, R10);
                    RAX
                } else {
                    ri
                };
                let rb = self.use_i(*base, R11);
                match m {
                    Mem::Int(n, _) => {
                        let rs = self.use_i(*src, R10);
                        self.a.sib = Some((ri, *sh));
                        self.a.store_n(rb, *off, rs, *n);
                    }
                    Mem::F32 | Mem::F64 => {
                        let xs = self.use_f(*src, XS);
                        self.a.sib = Some((ri, *sh));
                        self.a.storef(*m == Mem::F64, rb, *off, xs);
                    }
                }
            }
            Inst::Load(m, d, base, off) => {
                let rb = self.use_i(*base, R11);
                match m {
                    Mem::Int(n, signed) => {
                        let rd = self.def_i(*d, R10);
                        self.a.load_ext(rd, rb, *off, *n, *signed);
                        self.fin_i(*d, rd);
                    }
                    Mem::F32 | Mem::F64 => {
                        let xd = self.def_f(*d, XS);
                        self.a.loadf(*m == Mem::F64, xd, rb, *off);
                        self.fin_f(*d, xd);
                    }
                }
            }
            Inst::Store(m, base, off, src) => {
                let rb = self.use_i(*base, R11);
                match m {
                    Mem::Int(n, _) => {
                        let rs = self.use_i(*src, R10);
                        self.a.store_n(rb, *off, rs, *n);
                    }
                    Mem::F32 | Mem::F64 => {
                        let xs = self.use_f(*src, XS);
                        self.a.storef(*m == Mem::F64, rb, *off, xs);
                    }
                }
            }
            Inst::SlotAddr(d, s) => {
                let rd = self.def_i(*d, R10);
                let off = self.slot_off[*s as usize];
                self.a.lea(rd, RBP, -(self.saved + off));
                self.fin_i(*d, rd);
            }
            Inst::Addr(d, v) => {
                let rd = self.def_i(*d, R10);
                self.a.mov_ri(rd, *v as i64);
                self.fin_i(*d, rd);
            }
            Inst::FnAddr(d, id) => {
                let rd = self.def_i(*d, R10);
                let a = self.env.thunk_base + *id as u64 * self.env.thunk_size;
                self.a.mov_ri(rd, a as i64);
                self.fin_i(*d, rd);
            }
            Inst::Call(c, args, rets) => self.call(c, args, rets),
            Inst::Copy(dst, src, size) => {
                let rd = self.use_i(*dst, R10);
                let rs = self.use_i(*src, R11);
                if *size > 128 {
                    // rep movsb (fast-string copy) with rdi/rsi/rcx preserved
                    self.a.push(RSI);
                    self.a.push(RDI);
                    self.a.push(RCX);
                    self.a.mov_rr(RAX, rs);
                    self.a.mov_rr(RDI, rd);
                    self.a.mov_rr(RSI, RAX);
                    self.a.mov_ri(RCX, *size as i64);
                    self.a.byte(0xf3);
                    self.a.byte(0xa4);
                    self.a.pop(RCX);
                    self.a.pop(RDI);
                    self.a.pop(RSI);
                    return;
                }
                let mut o = 0i32;
                let size = *size as i32;
                while o + 8 <= size {
                    self.a.load64(RAX, rs, o);
                    self.a.store64(rd, o, RAX);
                    o += 8;
                }
                while o + 4 <= size {
                    self.a.load_ext(RAX, rs, o, 4, false);
                    self.a.store_n(rd, o, RAX, 4);
                    o += 4;
                }
                while o < size {
                    self.a.load_ext(RAX, rs, o, 1, false);
                    self.a.store_n(rd, o, RAX, 1);
                    o += 1;
                }
            }
            Inst::Poll => {
                // cmp byte [flag], 0 ; je skip ; call rt_hang (never returns)
                self.a.mov_ri(RAX, self.env.poll_flag as i64);
                self.a.byte(0x80);
                self.a.modrm_mem(7, RAX, 0);
                self.a.byte(0);
                self.a.byte(0x74); // je rel8
                let p = self.a.pos();
                self.a.byte(0);
                self.a.mov_ri(R11, self.env.rt_hang as i64);
                self.a.call_r(R11);
                let skip = self.a.pos();
                self.a.b[p] = (skip - p - 1) as u8;
            }
        }
    }

    fn ibin(&mut self, op: IOp, it: IntTy, d: VReg, a: VReg, b: VReg) {
        let ra = self.use_i(a, R10);
        let rb = self.use_i(b, R11);
        let rd = self.def_i(d, R10);
        match op {
            IOp::Div | IOp::Rem => {
                self.a.mov_rr(RAX, ra);
                let rb2 = if rb == RAX || rb == RDX {
                    self.a.mov_rr(R11, rb);
                    R11
                } else {
                    rb
                };
                if it.signed {
                    self.a.cqo();
                    self.a.grp3(7, rb2);
                } else {
                    self.a.mov_ri(RDX, 0);
                    self.a.grp3(6, rb2);
                }
                self.a.mov_rr(rd, if op == IOp::Div { RAX } else { RDX });
                self.a.extend(rd, it.bits, it.signed);
                self.fin_i(d, rd);
                return;
            }
            IOp::Shl | IOp::Shr => {
                self.a.mov_rr(RCX, rb);
                self.a.mov_rr(RAX, ra);
                let ext = match op {
                    IOp::Shl => 4,
                    _ => {
                        if it.signed {
                            7
                        } else {
                            5
                        }
                    }
                };
                self.a.shift_cl(ext, RAX);
                self.a.extend(RAX, it.bits, it.signed);
                self.a.mov_rr(rd, RAX);
                self.fin_i(d, rd);
                return;
            }
            _ => {}
        }
        // two-address form; when d aliases b, swap commutative operands or compute in rax
        let commutative = matches!(op, IOp::Add | IOp::Mul | IOp::And | IOp::Or | IOp::Xor);
        let (ra, rb) = if rd == rb && rd != ra && commutative { (rb, ra) } else { (ra, rb) };
        let work = if rd == rb && rd != ra { RAX } else { rd };
        self.a.mov_rr(work, ra);
        match op {
            IOp::Add => self.a.alu_rr(0x01, work, rb),
            IOp::Sub => self.a.alu_rr(0x29, work, rb),
            IOp::And => self.a.alu_rr(0x21, work, rb),
            IOp::Or => self.a.alu_rr(0x09, work, rb),
            IOp::Xor => self.a.alu_rr(0x31, work, rb),
            IOp::Mul => self.a.imul_rr(work, rb),
            _ => {}
        }
        if it.bits < 64 && !matches!(op, IOp::And | IOp::Or | IOp::Xor) {
            self.a.extend(work, it.bits, it.signed);
        }
        self.a.mov_rr(rd, work);
        self.fin_i(d, rd);
    }

    fn conv(&mut self, c: Conv, d: VReg, s: VReg) {
        match c {
            Conv::IntToInt(it) => {
                let rs = self.use_i(s, R10);
                let rd = self.def_i(d, R10);
                self.a.mov_rr(rd, rs);
                self.a.extend(rd, it.bits, it.signed);
                self.fin_i(d, rd);
            }
            Conv::IntToF(from, to64) => {
                let rs = self.use_i(s, R10);
                let xd = self.def_f(d, XS);
                let pfx = if to64 { 0xf2 } else { 0xf3 };
                if from.bits == 64 && !from.signed {
                    // u64 -> float: if negative as i64, halve with sticky bit, convert, double
                    self.a.mov_rr(RAX, rs);
                    self.a.test_rr(RAX, RAX);
                    let p = self.a.jcc32(0x8); // js
                    self.a.sse_rr(pfx, 0x2a, xd, RAX, true);
                    let q = self.a.jmp32();
                    let neg = self.a.pos();
                    self.a.patch(p, neg);
                    self.a.mov_rr(RCX, RAX);
                    // shr rax,1 ; and ecx,1 ; or rax,rcx
                    self.a.rex(true, 0, 0, RAX, false);
                    self.a.byte(0xd1);
                    self.a.modrm_rr(5, RAX);
                    self.a.mov_ri(RDX, 1);
                    self.a.alu_rr(0x21, RCX, RDX);
                    self.a.alu_rr(0x09, RAX, RCX);
                    self.a.sse_rr(pfx, 0x2a, xd, RAX, true);
                    self.a.sse_rr(pfx, 0x58, xd, xd, false);
                    let end = self.a.pos();
                    self.a.patch(q, end);
                } else {
                    // values are extended to 64 bits by type, so a 64-bit signed convert is exact
                    self.a.sse_rr(pfx, 0x2a, xd, rs, true);
                }
                self.fin_f(d, xd);
            }
            Conv::FToInt(from64, it) => {
                let xs = self.use_f(s, XS);
                // work in f64 (exact for f32 inputs)
                if from64 {
                    self.a.movf_rr(true, XS2, xs);
                } else {
                    self.a.sse_rr(0xf3, 0x5a, XS2, xs, false); // cvtss2sd
                }
                let rd = self.def_i(d, R10);
                self.f_to_int(rd, it);
                self.fin_i(d, rd);
            }
            Conv::F32ToF64 => {
                let xs = self.use_f(s, XS);
                let xd = self.def_f(d, XS);
                self.a.sse_rr(0xf3, 0x5a, xd, xs, false);
                self.fin_f(d, xd);
            }
            Conv::F64ToF32 => {
                let xs = self.use_f(s, XS);
                let xd = self.def_f(d, XS);
                self.a.sse_rr(0xf2, 0x5a, xd, xs, false);
                self.fin_f(d, xd);
            }
            Conv::BitsToF(f64_) => {
                let rs = self.use_i(s, R10);
                let xd = self.def_f(d, XS);
                self.a.movq_xr(xd, rs, f64_);
                self.fin_f(d, xd);
            }
            Conv::FToBits(f64_) => {
                let xs = self.use_f(s, XS);
                let rd = self.def_i(d, R10);
                self.a.movq_rx(rd, xs, f64_);
                self.fin_i(d, rd);
            }
        }
    }

    /// Saturating f64 (in XS2) -> int, Rust `as` semantics: NaN -> 0, clamp to range.
    fn f_to_int(&mut self, rd: u8, it: IntTy) {
        if it.signed && it.bits == 64 {
            // fast path: cvttsd2si gives 0x8000.. only for NaN / out of range; `cmp rax, 1`
            // overflows exactly for that value, then the exact saturating path runs
            self.a.sse_rr(0xf2, 0x2c, RAX, XS2, true);
            self.a.rex(true, 0, 0, RAX, false);
            self.a.byte(0x83);
            self.a.modrm_rr(7, RAX);
            self.a.byte(1);
            let p = self.a.jcc32(CC_O);
            let done = self.a.jmp32();
            let slow = self.a.pos();
            self.a.patch(p, slow);
            self.f_to_int_slow(it);
            let end = self.a.pos();
            self.a.patch(done, end);
            self.a.mov_rr(rd, RAX);
            return;
        }
        self.f_to_int_slow(it);
        self.a.mov_rr(rd, RAX);
    }

    /// Saturating f64 (in XS2) -> int in rax, Rust `as` semantics.
    fn f_to_int_slow(&mut self, it: IntTy) {
        let bits = it.bits as i32;
        let (lo_f, hi_f, min_v, max_v): (f64, f64, i64, i64) = if it.signed {
            let hi = (2f64).powi(bits - 1);
            (-hi, hi, if bits == 64 { i64::MIN } else { -(1i64 << (bits - 1)) }, if bits == 64 { i64::MAX } else { (1i64 << (bits - 1)) - 1 })
        } else {
            let hi = (2f64).powi(bits);
            (0.0, hi, 0, if bits == 64 { -1 } else { ((1u64 << bits) - 1) as i64 })
        };
        let mut done = Vec::new();
        // NaN -> 0
        self.a.mov_ri(RAX, 0);
        self.a.sse_rr(0x66, 0x2e, XS2, XS2, false);
        done.push(self.a.jcc32(CC_P));
        // >= hi -> max
        self.a.mov_ri(RAX, max_v);
        self.a.mov_ri(R11, hi_f.to_bits() as i64);
        self.a.movq_xr(XS, R11, true);
        self.a.sse_rr(0x66, 0x2e, XS2, XS, false);
        done.push(self.a.jcc32(CC_AE));
        // < lo (signed) / <= -1 (unsigned) -> min
        self.a.mov_ri(RAX, min_v);
        let lo_cmp = if it.signed { lo_f } else { -1.0 };
        self.a.mov_ri(R11, lo_cmp.to_bits() as i64);
        self.a.movq_xr(XS, R11, true);
        self.a.sse_rr(0x66, 0x2e, XS2, XS, false);
        done.push(self.a.jcc32(if it.signed { CC_B } else { CC_BE }));
        if !it.signed && bits == 64 {
            // >= 2^63: subtract 2^63, convert, set top bit
            self.a.mov_ri(R11, (2f64).powi(63).to_bits() as i64);
            self.a.movq_xr(XS, R11, true);
            self.a.sse_rr(0x66, 0x2e, XS2, XS, false);
            let small = self.a.jcc32(CC_B);
            self.a.sse_rr(0xf2, 0x5c, XS2, XS, false);
            self.a.sse_rr(0xf2, 0x2c, RAX, XS2, true);
            self.a.mov_ri(R11, i64::MIN);
            self.a.alu_rr(0x31, RAX, R11);
            done.push(self.a.jmp32());
            let s = self.a.pos();
            self.a.patch(small, s);
        }
        // in range: cvttsd2si r64
        self.a.sse_rr(0xf2, 0x2c, RAX, XS2, true);
        let end = self.a.pos();
        for p in done {
            self.a.patch(p, end);
        }
    }

    fn call(&mut self, c: &Callee, args: &[VReg], rets: &[VReg]) {
        if let Callee::CHost(_, sig) | Callee::CIndirect(_, sig) | Callee::CFn(_, sig) = c {
            return self.c_call(c, sig, args, rets);
        }
        // values in caller-saved registers that live across this call
        let saves = self.al.saves.get(&self.cur_pos).cloned().unwrap_or_default();
        for (v, slot) in &saves {
            let d = self.spill_disp(*slot);
            if let Some(src) = self.mloc(*v) {
                self.mv(MLoc::M(d), src);
            }
        }
        if let Callee::Indirect(v) = c {
            let r = self.use_i(*v, R11);
            self.a.mov_rr(R11, r);
        }
        let mut ni = 0;
        let mut nf = 0;
        let mut moves = Vec::new();
        let mut stack_args: Vec<VReg> = Vec::new();
        for a in args {
            let dst = match self.f.vregs[a.0 as usize] {
                Cls::I if ni < 6 => {
                    ni += 1;
                    MLoc::R(INT_ARGS[ni - 1])
                }
                Cls::F32 | Cls::F64 if nf < 8 => {
                    nf += 1;
                    MLoc::F(nf as u8 - 1)
                }
                _ => {
                    stack_args.push(*a);
                    continue;
                }
            };
            if let Some(src) = self.mloc(*a) {
                moves.push((dst, src));
            }
        }
        // overflow arguments: below the current rsp, 16-byte aligned area
        let stack_bytes = ((stack_args.len() as u32 * 8) + 15) / 16 * 16;
        if stack_bytes > 0 {
            self.a.sub_rsp(stack_bytes);
            for (k, a) in stack_args.iter().enumerate() {
                match self.mloc(*a) {
                    Some(MLoc::R(r)) => self.a.store64(RSP, 8 * k as i32, r),
                    Some(MLoc::F(x)) => self.a.storef(true, RSP, 8 * k as i32, x),
                    Some(MLoc::M(o)) => {
                        self.a.load64(R10, RBP, o);
                        self.a.store64(RSP, 8 * k as i32, R10);
                    }
                    None => {}
                }
            }
        }
        for (d, s2) in regalloc::seq_moves(&moves, R10, XS) {
            self.mv(d, s2);
        }
        match c {
            Callee::Fn(id) => {
                let slot = self.env.table_base + *id as u64 * 8;
                self.a.mov_ri(RAX, slot as i64);
                self.a.call_mem(RAX, 0);
            }
            Callee::Indirect(_) => self.a.call_r(R11),
            Callee::Host(addr) | Callee::HostVariadic(addr, _) => {
                self.a.mov_ri(R11, *addr as i64);
                self.a.mov_ri(RAX, nf as i64);
                self.a.call_r(R11);
            }
            Callee::CHost(..) | Callee::CIndirect(..) | Callee::CFn(..) => {}
        }
        if stack_bytes > 0 {
            // add rsp, imm32
            self.a.byte(0x48);
            self.a.byte(0x81);
            self.a.modrm_rr(0, RSP);
            self.a.u32(stack_bytes);
        }
        let mut ri = 0;
        let mut rf = 0;
        let mut ret_moves = Vec::new();
        for r in rets {
            let src = match self.f.vregs[r.0 as usize] {
                Cls::I => {
                    ri += 1;
                    MLoc::R(if ri == 1 { RAX } else { RDX })
                }
                _ => {
                    rf += 1;
                    MLoc::F(rf as u8 - 1)
                }
            };
            if let Some(d) = self.mloc(*r) {
                ret_moves.push((d, src));
            }
        }
        for (d, s2) in regalloc::seq_moves(&ret_moves, R10, XS) {
            self.mv(d, s2);
        }
        for (v, slot) in &saves {
            let sd = self.spill_disp(*slot);
            if let Some(dst) = self.mloc(*v) {
                self.mv(dst, MLoc::M(sd));
            }
        }
    }

    fn ibin_imm(&mut self, op: IOp, it: IntTy, d: VReg, a: VReg, k: i64) {
        let ra = self.use_i(a, R10);
        let rd = self.def_i(d, R10);
        match op {
            IOp::Mul => {
                // imul rd, ra, imm32
                self.a.rex(true, rd, 0, ra, false);
                self.a.byte(0x69);
                self.a.modrm_rr(rd, ra);
                self.a.u32(k as i32 as u32);
            }
            IOp::Shl | IOp::Shr => {
                self.a.mov_rr(rd, ra);
                let ext = match op {
                    IOp::Shl => 4,
                    _ => {
                        if it.signed {
                            7
                        } else {
                            5
                        }
                    }
                };
                self.a.rex(true, 0, 0, rd, false);
                self.a.byte(0xc1);
                self.a.modrm_rr(ext, rd);
                self.a.byte((k & 63) as u8);
            }
            _ => {
                self.a.mov_rr(rd, ra);
                let ext = match op {
                    IOp::Add => 0,
                    IOp::Or => 1,
                    IOp::And => 4,
                    IOp::Sub => 5,
                    _ => 6, // Xor
                };
                self.a.rex(true, 0, 0, rd, false);
                if (-128..128).contains(&k) {
                    self.a.byte(0x83);
                    self.a.modrm_rr(ext, rd);
                    self.a.byte(k as i8 as u8);
                } else {
                    self.a.byte(0x81);
                    self.a.modrm_rr(ext, rd);
                    self.a.u32(k as i32 as u32);
                }
            }
        }
        if it.bits < 64 && !matches!(op, IOp::And | IOp::Or | IOp::Xor) {
            self.a.extend(rd, it.bits, it.signed);
        }
        self.fin_i(d, rd);
    }
}

pub fn compile(f: &Func, env: &Env) -> Result<Compiled, String> {
    let mut cfg = reg_config();
    let mut uses_rcx_rdx = false;
    for b in &f.blocks {
        for i in &b.insts {
            match i {
                Inst::IBin(IOp::Div | IOp::Rem | IOp::Shl | IOp::Shr, _, _, _, _) | Inst::Conv(..) => uses_rcx_rdx = true,
                // cmpxchg loops use rax/rcx
                Inst::AtomicRmw(..) | Inst::AtomicCas(..) => uses_rcx_rdx = true,
                _ => {}
            }
        }
    }
    if !uses_rcx_rdx && std::env::var("HOTRUST_NO_RCX").is_err() {
        cfg.int_caller.push(RCX);
        cfg.int_caller.push(RDX);
    }
    let al = regalloc::allocate(f, &cfg);
    let mut g = Gen {
        a: Asm { b: Vec::with_capacity(256), sib: None },
        f,
        al,
        env,
        saved: 0,
        slot_off: Vec::new(),
        block_pos: Vec::new(),
        fixups: Vec::new(),
        pc_map: Vec::new(),
        cur_pos: 0,
        fpool: Vec::new(),
        next_block: Vec::new(),
        frame: 0,
        text_len: 0,
        mem_const: None,
        // off: pinned on node 165 it costs 12% on the parity test and nothing elsewhere; code/pool
        // cache-line separation does not change that (tested). Cause not proven.
        memconst: std::env::var("HOTRUST_MEMCONST").is_ok(),
        cplan: None,
        cbuf: Vec::new(),
        cret_buf: 0,
        cret_ptr: 0,
        stage: 0,
    };
    g.gen();
    let frame = g.frame as u32;
    let text_len = g.text_len;
    Ok(Compiled { code: g.a.b, pc_map: g.pc_map, frame_size: frame, text_len })
}

/// Slow path of `TlsAddr`: calls `host() -> block` and returns it in r11, preserving
/// every other register JIT code may hold. Entered with rsp = 8 mod 16.
pub fn tls_slow(host: u64) -> Vec<u8> {
    let mut a = Asm { b: Vec::new(), sib: None };
    a.push(RBP);
    a.mov_rr(RBP, RSP);
    let regs = [RCX, RDX, RSI, RDI, R8, R9, R10];
    for r in regs {
        a.push(r);
    }
    // 16 xmm registers + 8 bytes to realign
    a.sub_rsp(136);
    for x in 0..16u8 {
        a.storef(true, RSP, 8 * x as i32, x);
    }
    a.mov_ri(RAX, host as i64);
    a.call_r(RAX);
    a.mov_rr(R11, RAX);
    for x in 0..16u8 {
        a.loadf(true, x, RSP, 8 * x as i32);
    }
    a.byte(0x48);
    a.byte(0x81);
    a.modrm_rr(0, RSP);
    a.u32(136);
    let mut i = regs.len();
    while i > 0 {
        i -= 1;
        a.pop(regs[i]);
    }
    a.pop(RBP);
    a.ret();
    a.b
}

/// `fs_base() -> u64`: the thread pointer (fs:0 holds it on x86-64 Linux).
pub fn fs_reader() -> Vec<u8> {
    // mov rax, fs:[0] ; ret
    vec![0x64, 0x48, 0x8b, 0x04, 0x25, 0, 0, 0, 0, 0xc3]
}

/// The lazy-compile entry shared by all stubs: saves argument registers, calls
/// `compile_fn(slot) -> code address`, restores them and jumps to the code.
/// On entry rax holds the slot index.
pub fn lazy_entry(host_compile: u64) -> Vec<u8> {
    let mut a = Asm { b: Vec::new(), sib: None };
    a.push(RBP);
    a.mov_rr(RBP, RSP);
    // save rdi rsi rdx rcx r8 r9 + xmm0-7: 6*8 + 8*8 = 112, plus 8 to align -> 120? keep 16-aligned
    a.sub_rsp(128);
    let regs = [RDI, RSI, RDX, RCX, R8, R9];
    for (i, r) in regs.iter().enumerate() {
        a.store64(RSP, 8 * i as i32, *r);
    }
    for x in 0..8u8 {
        a.storef(true, RSP, 48 + 8 * x as i32, x);
    }
    a.mov_rr(RDI, RAX);
    a.mov_ri(R11, host_compile as i64);
    a.call_r(R11);
    a.mov_rr(R11, RAX);
    for (i, r) in regs.iter().enumerate() {
        a.load64(*r, RSP, 8 * i as i32);
    }
    for x in 0..8u8 {
        a.loadf(true, x, RSP, 48 + 8 * x as i32);
    }
    a.mov_rr(RSP, RBP);
    a.pop(RBP);
    // jmp r11
    a.rex(false, 0, 0, R11, false);
    a.byte(0xff);
    a.modrm_rr(4, R11);
    a.b
}

/// Per-slot thunk: `jmp [table + slot*8]` (the permanent address of a function).
pub fn thunk(slot_addr: u64, size: usize) -> Vec<u8> {
    let mut a = Asm { b: Vec::new(), sib: None };
    a.mov_ri(RAX, slot_addr as i64);
    a.jmp_mem(RAX, 0);
    while a.b.len() < size {
        a.byte(0xcc);
    }
    a.b
}

/// Per-slot stub: `mov eax, slot; jmp lazy_entry` (absolute via r11).
pub fn stub(slot: u32, lazy: u64) -> Vec<u8> {
    let mut a = Asm { b: Vec::new(), sib: None };
    a.mov_ri(RAX, slot as i64);
    a.mov_ri(R11, lazy as i64);
    a.rex(false, 0, 0, R11, false);
    a.byte(0xff);
    a.modrm_rr(4, R11);
    a.b
}

/// enter(fn, arg, ctx): saves callee-saved registers and rsp into *ctx, calls fn(arg).
/// Returns 0 normally. `leave_with(ctx, code)` restores them and returns `code`
/// from enter (used by panics, faults and hangs to get back to the host).
pub fn enter_leave() -> (Vec<u8>, usize) {
    let mut a = Asm { b: Vec::new(), sib: None };
    // enter: rdi = fn, rsi = arg, rdx = ctx
    a.push(RBP);
    a.mov_rr(RBP, RSP);
    a.push(RBX);
    a.push(R12);
    a.push(R13);
    a.push(R14);
    a.push(R15);
    a.sub_rsp(8);
    // ctx[0] = rsp
    a.store64(RDX, 0, RSP);
    a.mov_rr(RAX, RDI);
    a.mov_rr(RDI, RSI);
    a.call_r(RAX);
    a.mov_ri(RAX, 0);
    let tail = a.pos();
    // common exit: add rsp,8; pop...; ret
    a.byte(0x48);
    a.byte(0x83);
    a.modrm_rr(0, RSP);
    a.byte(8);
    a.pop(R15);
    a.pop(R14);
    a.pop(R13);
    a.pop(R12);
    a.pop(RBX);
    a.pop(RBP);
    a.ret();
    // leave(ctx = rdi, code = rsi)
    let leave = a.pos();
    a.load64(RSP, RDI, 0);
    a.mov_rr(RAX, RSI);
    let j = a.jmp32();
    a.patch(j, tail);
    (a.b, leave)
}

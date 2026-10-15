//! x86-64 backend (tier 0): liveness, linear-scan register allocation over
//! hull intervals, instruction selection and encoding. Produces position-
//! independent code apart from absolute addresses for data, the function
//! table and host functions (fine in-process; AOT adds relocations later).
//!
//! ABI (Rapid's own and SysV for host calls): int args rdi rsi rdx rcx r8 r9,
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
    /// 32-bit ALU op (writes zero-extend the destination: free u32 re-extension)
    fn alu_rr32(&mut self, opc: u8, dst: u8, src: u8) {
        self.rex(false, src, 0, dst, false);
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
    /// F3 REX.W 0F op /r: popcnt (b8), tzcnt (bc), lzcnt (bd) -- d = op(a)
    fn f3_rr(&mut self, op: u8, d: u8, a: u8) {
        self.byte(0xf3);
        self.rex(true, d, 0, a, false);
        self.byte(0x0f);
        self.byte(op);
        self.modrm_rr(d, a);
    }
    fn bswap(&mut self, r: u8) {
        self.rex(true, 0, 0, r, false);
        self.byte(0x0f);
        self.byte(0xc8 + (r & 7));
    }
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
    fn jmp_r(&mut self, r: u8) {
        self.rex(false, 0, 0, r, false);
        self.byte(0xff);
        self.modrm_rr(4, r);
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
    /// [pfx] [rex] 0F esc op /r (esc 0x38 / 0x3A), register form
    fn sse_ext(&mut self, pfx: u8, esc: u8, op: u8, reg: u8, rm: u8) {
        if pfx != 0 {
            self.byte(pfx);
        }
        self.rex(false, reg, 0, rm, false);
        self.byte(0x0f);
        self.byte(esc);
        self.byte(op);
        self.modrm_rr(reg, rm);
    }
    /// VEX (3-byte) register form, 128-bit: map 1 = 0F, 2 = 0F38, 3 = 0F3A; pp 1 = 66
    fn vex_rr(&mut self, pp: u8, map: u8, w: bool, op: u8, reg: u8, vvvv: u8, rm: u8) {
        self.byte(0xc4);
        self.byte(((!(reg >> 3) & 1) << 7) | (1 << 6) | ((!(rm >> 3) & 1) << 5) | map);
        self.byte(((w as u8) << 7) | ((!vvvv & 15) << 3) | pp);
        self.byte(op);
        self.modrm_rr(reg, rm);
    }
    /// VEX (3-byte) [rip + disp32] form, 128-bit; returns the disp32 patch position
    fn vex_rip(&mut self, pp: u8, map: u8, w: bool, op: u8, reg: u8, vvvv: u8) -> usize {
        self.byte(0xc4);
        self.byte(((!(reg >> 3) & 1) << 7) | (1 << 6) | (1 << 5) | map);
        self.byte(((w as u8) << 7) | ((!vvvv & 15) << 3) | pp);
        self.byte(op);
        self.byte(((reg & 7) << 3) | 5);
        let at = self.pos();
        self.u32(0);
        at
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
    /// `xorps x, x` before an instruction that writes only x's low lane: breaks the false
    /// dependency on x's old value (a loop-carried latency chain otherwise)
    fn break_dep(&mut self, x: u8, src: u8) {
        if x != src {
            self.sse_rr(0, 0x57, x, x, false);
        }
    }
    fn roundf(&mut self, f64_: bool, dst: u8, src: u8, mode: u8) {
        self.break_dep(dst, src);
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
    /// Last integer spill store: stack offset, register, and end position.
    last_spill: Option<(i32, u8, usize)>,
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
    /// splatted 32-bit vector constants: (patch offset, lane bits), 16-byte entries
    vpool: Vec<(usize, u32)>,
    /// emission order successor of each block (u32::MAX for the last)
    next_block: Vec<u32>,
    frame: i32,
    text_len: u32,
    /// a single-use float constant whose use (the next FBin) reads it from the pool
    mem_const: Option<(VReg, u64)>,
    memconst: bool,
    /// AVX: three-operand VEX forms of scalar float ops (and memory-operand constants)
    avx: bool,
    /// the compare fused into the next select
    sel_cmp: Option<Inst>,
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
                self.load_spill(scratch, d);
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
            self.last_spill = Some((d, r, self.a.pos()));
        }
    }
    fn load_spill(&mut self, dst: u8, disp: i32) {
        if let Some((stored_disp, register, position)) = self.last_spill {
            if stored_disp == disp && position == self.a.pos() {
                self.a.mov_rr(dst, register);
                return;
            }
        }
        self.a.load64(dst, RBP, disp);
    }
    fn use_f(&mut self, v: VReg, scratch: u8) -> u8 {
        match self.al.loc[v.0 as usize] {
            Loc::Flt(x) => x,
            Loc::Stack(o) if self.f.vregs[v.0 as usize] == Cls::V128 => {
                let d = self.spill_disp(o);
                self.a.sse_rm(0, 0x10, scratch, RBP, d); // movups
                scratch
            }
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
            if self.f.vregs[v.0 as usize] == Cls::V128 {
                self.a.sse_rm(0, 0x11, x, RBP, d); // movups
                return;
            }
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

    /// Rapid ABI params: ABI registers -> allocated locations (parallel move).
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
            // A block can be entered without executing the preceding store.
            self.last_spill = None;
            for (ii, inst) in b.insts.iter().enumerate() {
                self.pc_map.push((self.a.pos() as u32, b.pos[ii]));
                self.cur_pos = self.al.inst_pos(bi, ii);
                if fused && ii == n - 1 {
                    break;
                }
                // a compare whose one use is the next select: fused into it (flags + cmov,
                // or a compare mask + blend)
                if ii + 1 < n && !(fused && ii + 1 == n - 1) {
                    if let (Some(cd), Inst::Sel(sd, sc, _, _)) = (cmp_def(inst), &b.insts[ii + 1]) {
                        let data = self.f.vregs[sd.0 as usize];
                        let ok = match inst {
                            Inst::ICmp(..) | Inst::ICmpI(..) => true,
                            Inst::FCmp(_, w, ..) => data == if *w { Cls::F64 } else { Cls::F32 },
                            _ => false,
                        };
                        if *sc == cd && self.al.uses[cd.0 as usize] == 1 && ok {
                            self.sel_cmp = Some(inst.clone());
                            continue;
                        }
                    }
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
            // 128-bit memory operands of the last entry may read 8 bytes past it
            self.a.u64(0);
        }
        if !self.vpool.is_empty() {
            while self.a.pos() % 16 != 0 {
                self.a.byte(0xcc);
            }
            let mut placed: Vec<(u32, usize)> = Vec::new();
            for (at, bits) in std::mem::take(&mut self.vpool) {
                let addr = match placed.iter().find(|(b2, _)| *b2 == bits) {
                    Some((_, a2)) => *a2,
                    None => {
                        let a2 = self.a.pos();
                        for _ in 0..4 {
                            self.a.u32(bits);
                        }
                        placed.push((bits, a2));
                        a2
                    }
                };
                self.a.patch(at, addr);
            }
        }
    }

    /// AVX: xd = mask ? a : b by each lane's top bit (vblendvpd / vblendvps, 4 operands)
    fn vblendv(&mut self, pd: bool, xd: u8, xb: u8, xa: u8, mask: u8) {
        self.a.vex_rr(1, 3, false, if pd { 0x4b } else { 0x4a }, xd, xb, xa);
        self.a.byte(mask << 4);
    }

    /// `vop xd, xa, [rip + c]` (AVX, VEX.66.0F: and/or/xor/add... packed-double forms read
    /// 16 bytes from the 8-byte pool entry; the upper lane is never used)
    fn vex_pool(&mut self, pp: u8, op: u8, xd: u8, xa: u8, bits: u64) {
        let at = self.a.vex_rip(pp, 1, false, op, xd, xa);
        self.fpool.push((at, bits));
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
        // C leaves a narrow integer result's upper bits undefined
        if let Some(it) = sig.ret_narrow {
            self.a.extend(RAX, it.bits, it.signed);
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
            Inst::Sel(d, c, a, b) => self.sel(*d, *c, *a, *b),
            Inst::VBin(..) | Inst::VUn(..) | Inst::VSel(..) | Inst::VFma(..) | Inst::VSplat(..) | Inst::VExtract(..) | Inst::VInsert(..) | Inst::VLoad(..) | Inst::VStore(..) | Inst::VAny(..) | Inst::VGather(..) | Inst::VConst(..) | Inst::VShift(..) => self.vinst(i),
            Inst::FMulAdd(np, na, w, d, a, b, c) => self.fma(*np, *na, *w, *d, *a, *b, *c),
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
            Inst::IBit(o, d, a) => {
                let ra = self.use_i(*a, R10);
                let rd = self.def_i(*d, R10);
                match o {
                    BitOp::Popcnt => self.a.f3_rr(0xb8, rd, ra),
                    BitOp::Ctz => self.a.f3_rr(0xbc, rd, ra),
                    BitOp::Clz => self.a.f3_rr(0xbd, rd, ra),
                    // bit reverse is expanded before allocation (no x86 instruction)
                    BitOp::Bswap | BitOp::Rev => {
                        self.a.mov_rr(rd, ra);
                        self.a.bswap(rd);
                    }
                }
                self.fin_i(*d, rd);
            }
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
                    FOp::Min | FOp::MinLt => 0x5d,
                    FOp::Max | FOp::MaxGt => 0x5f,
                };
                let other = if *b == cv { *a } else { *b };
                let xo = self.use_f(other, XS);
                let xd = self.def_f(*d, XS);
                if self.avx {
                    // vop xd, xo, [rip + c]
                    let at = self.a.vex_rip(if *f64_ { 3 } else { 2 }, 1, false, opc, xd, xo);
                    self.fpool.push((at, bits));
                    self.fin_f(*d, xd);
                    return;
                }
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
                    FOp::Min | FOp::MinLt => 0x5d,
                    FOp::Max | FOp::MaxGt => 0x5f,
                };
                if self.avx {
                    // three-operand VEX form: no copies
                    self.a.vex_rr(if *f64_ { 3 } else { 2 }, 1, false, opc, xd, xa, xb);
                } else if xd == xb && xd != xa && (*op == FOp::Add || *op == FOp::Mul) {
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
                    FUn::Neg | FUn::Abs if self.avx => {
                        let mask: u64 = match (op, f64_) {
                            (FUn::Neg, true) => 0x8000_0000_0000_0000,
                            (FUn::Neg, false) => 0x8000_0000,
                            (_, true) => 0x7fff_ffff_ffff_ffff,
                            (_, false) => 0x7fff_ffff,
                        };
                        self.vex_pool(1, if *op == FUn::Neg { 0x57 } else { 0x54 }, xd, xa, mask);
                    }
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
                        self.a.break_dep(xd, xa);
                        self.a.sse_rr(if *f64_ { 0xf2 } else { 0xf3 }, 0x51, xd, xa, false);
                    }
                    FUn::Floor => self.a.roundf(*f64_, xd, xa, 1),
                    FUn::Ceil => self.a.roundf(*f64_, xd, xa, 2),
                    FUn::Trunc => self.a.roundf(*f64_, xd, xa, 3),
                    FUn::RoundEven => self.a.roundf(*f64_, xd, xa, 0),
                    FUn::RoundAway => self.round_away(*f64_, xd, xa),
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
        }
    }

    /// All four lanes = the 32-bit constant `bits` (through r10).
    fn vsplat_const(&mut self, x: u8, bits: u32) {
        // movups x, [rip + 16-byte pool entry]
        self.a.rex(false, x, 0, 0, false);
        self.a.byte(0x0f);
        self.a.byte(0x10);
        self.a.byte(((x & 7) << 3) | 5);
        let at = self.a.pos();
        self.a.u32(0);
        self.vpool.push((at, bits));
    }

    /// `op dst, src` two-address on a vector: d = a op b with aliasing handled.
    fn v2(&mut self, d: VReg, a: VReg, b: VReg, emit: &dyn Fn(&mut Asm, u8, u8)) {
        let xa = self.use_f(a, XS);
        let xb = self.use_f(b, XS2);
        let xd = self.def_f(d, XS);
        if xd == xb && xd != xa {
            self.a.movf_rr(true, 0, xa);
            emit(&mut self.a, 0, xb);
            self.a.movf_rr(true, xd, 0);
        } else {
            self.a.movf_rr(true, xd, xa);
            emit(&mut self.a, xd, xb);
        }
        self.fin_f(d, xd);
    }

    /// d = a op b: the VEX three-operand form with AVX (no copies), else the SSE two-address
    /// form. pp: 0 none / 1 66; map: 1 0F / 2 0F38 / 3 0F3A; imm for compares.
    fn v3(&mut self, d: VReg, a: VReg, b: VReg, pp: u8, map: u8, op: u8, imm: Option<u8>) {
        if self.avx {
            let xa = self.use_f(a, XS);
            let xb = self.use_f(b, XS2);
            let xd = self.def_f(d, XS);
            self.a.vex_rr(pp, map, false, op, xd, xa, xb);
            if let Some(k) = imm {
                self.a.byte(k);
            }
            self.fin_f(d, xd);
            return;
        }
        let pfx = if pp == 1 { 0x66 } else { 0 };
        self.v2(d, a, b, &move |m: &mut Asm, x: u8, y: u8| {
            match map {
                1 => m.sse_rr(pfx, op, x, y, false),
                2 => m.sse_ext(pfx, 0x38, op, x, y),
                _ => m.sse_ext(pfx, 0x3a, op, x, y),
            }
            if let Some(k) = imm {
                m.byte(k);
            }
        });
    }

    /// 128-bit vector instructions (SSE4.1; AVX2 for per-lane shifts, FMA3 for VFma).
    fn vinst(&mut self, i: &Inst) {
        match *i {
            Inst::VBin(op, d, a, b) => {
                match op {
                    VOp::FAdd => self.v3(d, a, b, 0, 1, 0x58, None),
                    VOp::FSub => self.v3(d, a, b, 0, 1, 0x5c, None),
                    VOp::FMul => self.v3(d, a, b, 0, 1, 0x59, None),
                    VOp::FDiv => self.v3(d, a, b, 0, 1, 0x5e, None),
                    VOp::FMinLt => self.v3(d, a, b, 0, 1, 0x5d, None),
                    VOp::FMaxGt => self.v3(d, a, b, 0, 1, 0x5f, None),
                    VOp::IAdd => self.v3(d, a, b, 1, 1, 0xfe, None),
                    VOp::ISub => self.v3(d, a, b, 1, 1, 0xfa, None),
                    VOp::IMul => self.v3(d, a, b, 1, 2, 0x40, None),
                    VOp::And => self.v3(d, a, b, 1, 1, 0xdb, None),
                    VOp::Or => self.v3(d, a, b, 1, 1, 0xeb, None),
                    VOp::Xor => self.v3(d, a, b, 1, 1, 0xef, None),
                    VOp::UMin => self.v3(d, a, b, 1, 2, 0x3b, None), // pminud
                    VOp::AndNot => {
                        // pandn x, y: x = !x & y; here d = a & !b
                        let xa = self.use_f(a, XS);
                        let xb = self.use_f(b, XS2);
                        self.a.movf_rr(true, 0, xb);
                        self.a.sse_rr(0x66, 0xdf, 0, xa, false);
                        let xd = self.def_f(d, XS);
                        self.a.movf_rr(true, xd, 0);
                        self.fin_f(d, xd);
                    }
                    VOp::Shl | VOp::ShrS | VOp::ShrU => {
                        let xa = self.use_f(a, XS);
                        let xb = self.use_f(b, XS2);
                        let xd = self.def_f(d, XS);
                        let opc = match op {
                            VOp::Shl => 0x47,
                            VOp::ShrS => 0x46,
                            _ => 0x45,
                        };
                        self.a.vex_rr(1, 2, false, opc, xd, xa, xb); // vps{ll,ra,rl}vd
                        self.fin_f(d, xd);
                    }
                    VOp::FCmp(c) => {
                        let (x, y, pred) = match c {
                            Cond::Lt => (a, b, 1),
                            Cond::Le => (a, b, 2),
                            Cond::Gt => (b, a, 1),
                            Cond::Ge => (b, a, 2),
                            Cond::Eq => (a, b, 0),
                            Cond::Ne => (a, b, 4),
                        };
                        self.v3(d, x, y, 0, 1, 0xc2, Some(pred));
                    }
                    VOp::ICmp(c) => {
                        let (x, y, gt, not) = match c {
                            Cond::Eq => (a, b, false, false),
                            Cond::Ne => (a, b, false, true),
                            Cond::Gt => (a, b, true, false),
                            Cond::Lt => (b, a, true, false),
                            Cond::Le => (a, b, true, true),
                            Cond::Ge => (b, a, true, true),
                        };
                        self.v2(d, x, y, &move |m: &mut Asm, p: u8, q: u8| {
                            m.sse_rr(0x66, if gt { 0x66 } else { 0x76 }, p, q, false);
                            if not {
                                m.sse_rr(0x66, 0x76, 1, 1, false); // xmm1 = all ones
                                m.sse_rr(0x66, 0xef, p, 1, false);
                            }
                        });
                    }
                }
            }
            Inst::VUn(op, d, a) => {
                let xa = self.use_f(a, XS);
                match op {
                    VUn::FNeg | VUn::FAbs => {
                        self.vsplat_const(1, if op == VUn::FNeg { 0x8000_0000 } else { 0x7fff_ffff });
                        self.a.sse_rr(0, if op == VUn::FNeg { 0x57 } else { 0x54 }, 1, xa, false);
                        let xd = self.def_f(d, XS);
                        self.a.movf_rr(true, xd, 1);
                        self.fin_f(d, xd);
                    }
                    VUn::FSqrt | VUn::IToF => {
                        let xd = self.def_f(d, XS);
                        self.a.sse_rr(0, if op == VUn::FSqrt { 0x51 } else { 0x5b }, xd, xa, false);
                        self.fin_f(d, xd);
                    }
                    VUn::FFloor | VUn::FCeil | VUn::FTrunc => {
                        let xd = self.def_f(d, XS);
                        self.a.sse_ext(0x66, 0x3a, 0x08, xd, xa); // roundps
                        self.a.byte(match op {
                            VUn::FFloor => 9,
                            VUn::FCeil => 10,
                            _ => 11,
                        });
                        self.fin_f(d, xd);
                    }
                    VUn::FRoundAway => {
                        // trunc(x + copysign(0.5 - ulp/2, x))
                        self.vsplat_const(1, 0x8000_0000);
                        self.a.sse_rr(0, 0x54, 1, xa, false);
                        self.vsplat_const(0, 0.49999997f32.to_bits());
                        self.a.sse_rr(0, 0x56, 1, 0, false);
                        self.a.movf_rr(true, 0, xa);
                        self.a.sse_rr(0, 0x58, 0, 1, false);
                        let xd = self.def_f(d, XS);
                        self.a.sse_ext(0x66, 0x3a, 0x08, xd, 0);
                        self.a.byte(11);
                        self.fin_f(d, xd);
                    }
                    VUn::FToI => {
                        // cvttps2dq gives 0x80000000 for NaN and out of range: NaN -> 0,
                        // x >= 2^31 -> 0x7fffffff (0x80000000 ^ all ones)
                        self.a.sse_rr(0xf3, 0x5b, 0, xa, false);
                        self.a.movf_rr(true, 1, xa);
                        self.a.sse_rr(0, 0xc2, 1, 1, false);
                        self.a.byte(3); // unord
                        self.a.sse_rr(0x66, 0xdf, 1, 0, false); // xmm1 = !nan & r
                        self.vsplat_const(XS2, 0x4f00_0000); // 2^31
                        self.a.sse_rr(0, 0xc2, XS2, xa, false);
                        self.a.byte(2); // 2^31 <= x
                        self.a.sse_rr(0x66, 0xef, 1, XS2, false);
                        let xd = self.def_f(d, XS);
                        self.a.movf_rr(true, xd, 1);
                        self.fin_f(d, xd);
                    }
                    VUn::INeg => {
                        self.a.sse_rr(0x66, 0xef, 0, 0, false);
                        self.a.sse_rr(0x66, 0xfa, 0, xa, false);
                        let xd = self.def_f(d, XS);
                        self.a.movf_rr(true, xd, 0);
                        self.fin_f(d, xd);
                    }
                }
            }
            Inst::VSel(d, m, a, b) => {
                let xm = self.use_f(m, XS);
                self.a.movf_rr(true, 0, xm);
                self.a.movf_rr(true, 1, xm);
                let xa = self.use_f(a, XS);
                self.a.sse_rr(0, 0x54, 1, xa, false); // m & a
                let xb = self.use_f(b, XS2);
                self.a.sse_rr(0, 0x55, 0, xb, false); // !m & b
                self.a.sse_rr(0, 0x56, 0, 1, false);
                let xd = self.def_f(d, XS);
                self.a.movf_rr(true, xd, 0);
                self.fin_f(d, xd);
            }
            Inst::VFma(np, na, d, a, b, c) => {
                let xa = self.use_f(a, XS);
                let xb = self.use_f(b, XS2);
                let xc = self.use_f(c, 1);
                let xd0 = self.def_f(d, XS);
                if xd0 != xa && xd0 != xb && xd0 != XS {
                    // accumulate straight into d
                    self.a.movf_rr(true, xd0, xc);
                    let op = match (np, na) {
                        (false, false) => 0xb8,
                        (false, true) => 0xba,
                        (true, false) => 0xbc,
                        (true, true) => 0xbe,
                    };
                    self.a.vex_rr(1, 2, false, op, xd0, xa, xb);
                    self.fin_f(d, xd0);
                    return;
                }
                self.a.movf_rr(true, 0, xc);
                let op = match (np, na) {
                    (false, false) => 0xb8,
                    (false, true) => 0xba,
                    (true, false) => 0xbc,
                    (true, true) => 0xbe,
                };
                self.a.vex_rr(1, 2, false, op, 0, xa, xb); // vf[n]m{add,sub}231ps xmm0, a, b
                let xd = self.def_f(d, XS);
                self.a.movf_rr(true, xd, 0);
                self.fin_f(d, xd);
            }
            Inst::VSplat(d, s) => {
                let xd = self.def_f(d, XS);
                if self.f.vregs[s.0 as usize] == Cls::I {
                    let rs = self.use_i(s, R10);
                    self.a.sse_rr(0x66, 0x6e, 0, rs, false); // movd xmm0, r32
                    self.a.sse_rr(0x66, 0x70, xd, 0, false);
                } else {
                    let xs = self.use_f(s, XS2);
                    self.a.sse_rr(0x66, 0x70, xd, xs, false);
                }
                self.a.byte(0);
                self.fin_f(d, xd);
            }
            Inst::VExtract(d, v, lane) => {
                let xv = self.use_f(v, XS);
                if self.f.vregs[d.0 as usize] == Cls::I {
                    let rd = self.def_i(d, R10);
                    self.a.sse_ext(0x66, 0x3a, 0x16, xv, rd); // pextrd r32, xmm, lane
                    self.a.byte(lane);
                    self.a.extend(rd, 32, true);
                    self.fin_i(d, rd);
                } else {
                    let xd = self.def_f(d, XS2);
                    self.a.sse_rr(0x66, 0x70, xd, xv, false); // pshufd: lane -> lane 0
                    self.a.byte(lane);
                    self.fin_f(d, xd);
                }
            }
            Inst::VInsert(d, v, lane, s) => {
                let xv = self.use_f(v, XS);
                self.a.movf_rr(true, 0, xv);
                if self.f.vregs[s.0 as usize] == Cls::I {
                    let rs = self.use_i(s, R10);
                    self.a.sse_ext(0x66, 0x3a, 0x22, 0, rs); // pinsrd xmm0, r32, lane
                    self.a.byte(lane);
                } else {
                    let xs = self.use_f(s, XS2);
                    self.a.sse_ext(0x66, 0x3a, 0x21, 0, xs); // insertps xmm0, xs, lane
                    self.a.byte(lane << 4);
                }
                let xd = self.def_f(d, XS);
                self.a.movf_rr(true, xd, 0);
                self.fin_f(d, xd);
            }
            Inst::VLoad(d, base, off) => {
                let rb = self.use_i(base, R11);
                let xd = self.def_f(d, XS);
                self.a.sse_rm(0, 0x10, xd, rb, off);
                self.fin_f(d, xd);
            }
            Inst::VStore(base, off, v) => {
                let rb = self.use_i(base, R11);
                let xv = self.use_f(v, XS);
                self.a.sse_rm(0, 0x11, xv, rb, off);
            }
            Inst::VShift(op, d, a, k) => {
                // psll/psrl/psra d, imm8 (66 0F 72 /6, /2, /4)
                let xa = self.use_f(a, XS);
                let xd = self.def_f(d, XS);
                self.a.movf_rr(true, xd, xa);
                self.a.byte(0x66);
                self.a.rex(false, 0, 0, xd, false);
                self.a.byte(0x0f);
                self.a.byte(0x72);
                self.a.modrm_rr(match op {
                    VOp::Shl => 6,
                    VOp::ShrS => 4,
                    _ => 2,
                }, xd);
                self.a.byte(k & 31);
                self.fin_f(d, xd);
            }
            Inst::VConst(d, bits) => {
                // movups xd, [rip + entry]
                let xd = self.def_f(d, XS);
                self.a.rex(false, xd, 0, 0, false);
                self.a.byte(0x0f);
                self.a.byte(0x10);
                self.a.byte(((xd & 7) << 3) | 5);
                let at = self.a.pos();
                self.a.u32(0);
                self.vpool.push((at, bits));
                self.fin_f(d, xd);
            }
            Inst::VGather(d, base, idx) if !use_gather() => {
                // per lane: pextrd / mov / pinsrd (faster than vpgatherdd for four lanes on the
                // CPUs measured; RAPID_GATHER=1 selects the gather)
                let rb = self.use_i(base, R11);
                let xi = self.use_f(idx, XS2);
                for l in 0..4u8 {
                    self.a.sse_ext(0x66, 0x3a, 0x16, xi, RAX); // pextrd eax, xi, l
                    self.a.byte(l);
                    // mov eax, [rb + rax*4]
                    self.a.sib = Some((RAX, 2));
                    self.a.rex(false, RAX, 0, rb, false);
                    self.a.byte(0x8b);
                    self.a.modrm_mem(RAX, rb, 0);
                    self.a.sse_ext(0x66, 0x3a, 0x22, 1, RAX); // pinsrd xmm1, eax, l
                    self.a.byte(l);
                }
                let xd = self.def_f(d, XS);
                self.a.movf_rr(true, xd, 1);
                self.fin_f(d, xd);
            }
            Inst::VGather(d, base, idx) => {
                // vpgatherdd xmm1, [rb + xmm_idx*4], xmm0 (mask: all lanes; the instruction
                // clears it): VEX.128.66.0F38.W0 90 /r with a VSIB address
                let rb = self.use_i(base, R11);
                let xi = self.use_f(idx, XS2);
                self.a.sse_rr(0x66, 0x76, 0, 0, false); // pcmpeqd xmm0, xmm0
                self.a.sse_rr(0x66, 0xef, 1, 1, false); // pxor xmm1, xmm1 (no stale lanes)
                // C4 RXB.00010 W0.vvvv(xmm0).0.01 90 modrm(reg=xmm1, rm=100) sib(scale 4, index xi, base rb)
                self.a.byte(0xc4);
                self.a.byte((1 << 7) | ((!(xi >> 3) & 1) << 6) | ((!(rb >> 3) & 1) << 5) | 0b00010);
                self.a.byte((0b1111 << 3) | 0b01);
                self.a.byte(0x90);
                if rb & 7 == 5 {
                    // rbp/r13 base needs a disp8
                    self.a.byte(0x40 | (1 << 3) | 4);
                    self.a.byte((2 << 6) | ((xi & 7) << 3) | (rb & 7));
                    self.a.byte(0);
                } else {
                    self.a.byte((1 << 3) | 4);
                    self.a.byte((2 << 6) | ((xi & 7) << 3) | (rb & 7));
                }
                let xd = self.def_f(d, XS);
                self.a.movf_rr(true, xd, 1);
                self.fin_f(d, xd);
            }
            Inst::VAny(d, v) => {
                let xv = self.use_f(v, XS);
                self.a.sse_ext(0x66, 0x38, 0x17, xv, xv); // ptest
                let rd = self.def_i(d, R10);
                self.a.setcc(CC_NE, rd);
                self.a.movzx8(rd, rd);
                self.fin_i(d, rd);
            }
            _ => {}
        }
    }

    /// After flags are set: d = cc ? a : b (loads do not touch the flags).
    fn cmov_sel(&mut self, d: VReg, a: VReg, b: VReg, cc: u8) {
        let ra = self.use_i(a, R10);
        let rb = self.use_i(b, R11);
        self.a.mov_rr(RAX, rb);
        self.a.rex(true, RAX, 0, ra, false);
        self.a.byte(0x0f);
        self.a.byte(0x40 | cc);
        self.a.modrm_rr(RAX, ra);
        let rd = self.def_i(d, R10);
        self.a.mov_rr(rd, RAX);
        self.fin_i(d, rd);
    }

    /// d = c != 0 ? a : b. Integers: cmov; floats: a short branch through xmm0.
    fn sel(&mut self, d: VReg, c: VReg, a: VReg, b: VReg) {
        let int_data = self.f.vregs[d.0 as usize] == Cls::I;
        match self.sel_cmp.take() {
            Some(Inst::ICmp(cc, signed, _, x, y)) if int_data => {
                let rx = self.use_i(x, R10);
                let ry = self.use_i(y, R11);
                self.a.alu_rr(0x39, rx, ry);
                return self.cmov_sel(d, a, b, icc(cc, signed));
            }
            Some(Inst::ICmpI(cc, signed, _, x, k)) if int_data => {
                let rx = self.use_i(x, R10);
                self.cmp_imm(rx, k);
                return self.cmov_sel(d, a, b, icc(cc, signed));
            }
            Some(i @ (Inst::ICmp(..) | Inst::ICmpI(..))) => {
                // an integer compare choosing floats: setcc, then -1/0 as the blend mask
                let cc = match i {
                    Inst::ICmp(cc, signed, _, x, y) => {
                        let rx = self.use_i(x, R10);
                        let ry = self.use_i(y, R11);
                        self.a.alu_rr(0x39, rx, ry);
                        icc(cc, signed)
                    }
                    Inst::ICmpI(cc, signed, _, x, k) => {
                        let rx = self.use_i(x, R10);
                        self.cmp_imm(rx, k);
                        icc(cc, signed)
                    }
                    _ => unreachable!(),
                };
                if self.avx {
                    // the bits through GPRs: rax = b, r11 = a, cmovcc rax, r11
                    let w = self.f.vregs[d.0 as usize] == Cls::F64;
                    let xb = self.use_f(b, XS);
                    self.a.movq_rx(RAX, xb, w);
                    let xa = self.use_f(a, XS2);
                    self.a.movq_rx(R11, xa, w);
                    self.a.rex(true, RAX, 0, R11, false);
                    self.a.byte(0x0f);
                    self.a.byte(0x40 | cc);
                    self.a.modrm_rr(RAX, R11);
                    let xd = self.def_f(d, XS);
                    self.a.movq_xr(xd, RAX, w);
                    self.fin_f(d, xd);
                    return;
                }
                self.a.setcc(cc, RAX);
                self.a.movzx8(RAX, RAX);
                self.a.grp3(3, RAX); // neg: 1 -> all ones
                self.a.sse_rr(0x66, 0x6e, 0, RAX, true); // movq xmm0, rax
                let xb = self.use_f(b, XS);
                self.a.movf_rr(true, 1, xb);
                let xa = self.use_f(a, XS2);
                self.a.sse_ext(0x66, 0x38, 0x15, 1, xa); // blendvpd (bit 63 = the mask)
                let xd = self.def_f(d, XS);
                self.a.movf_rr(true, xd, 1);
                self.fin_f(d, xd);
                return;
            }
            Some(Inst::FCmp(cc, w, _, x, y)) => {
                // xmm0 = the compare's lane mask; blendv picks a where it is set
                let (p, q, pred) = match cc {
                    Cond::Lt => (x, y, 1),
                    Cond::Le => (x, y, 2),
                    Cond::Gt => (y, x, 1),
                    Cond::Ge => (y, x, 2),
                    Cond::Eq => (x, y, 0),
                    Cond::Ne => (x, y, 4),
                };
                let xp = self.use_f(p, XS);
                let xq = self.use_f(q, XS2);
                if self.avx {
                    // vcmpsd xmm0, p, q, pred; vblendv d, b, a, xmm0
                    self.a.vex_rr(if w { 3 } else { 2 }, 1, false, 0xc2, 0, xp, xq);
                    self.a.byte(pred);
                    let xb = self.use_f(b, XS);
                    let xa = self.use_f(a, XS2);
                    let xd = self.def_f(d, XS);
                    self.vblendv(w, xd, xb, xa, 0);
                    self.fin_f(d, xd);
                    return;
                }
                self.a.movf_rr(true, 0, xp);
                self.a.sse_rr(if w { 0xf2 } else { 0xf3 }, 0xc2, 0, xq, false);
                self.a.byte(pred);
                let xb = self.use_f(b, XS);
                self.a.movf_rr(true, 1, xb);
                let xa = self.use_f(a, XS2);
                self.a.sse_ext(0x66, 0x38, if w { 0x15 } else { 0x14 }, 1, xa); // blendvpd / blendvps
                let xd = self.def_f(d, XS);
                self.a.movf_rr(true, xd, 1);
                self.fin_f(d, xd);
                return;
            }
            _ => {}
        }
        if self.f.vregs[d.0 as usize] == Cls::I {
            let ra = self.use_i(a, R11);
            let rb = self.use_i(b, RAX);
            self.a.mov_rr(RAX, rb);
            let rc = self.use_i(c, R10);
            self.a.test_rr(rc, rc);
            // cmovne rax, ra
            self.a.rex(true, RAX, 0, ra, false);
            self.a.byte(0x0f);
            self.a.byte(0x40 | CC_NE);
            self.a.modrm_rr(RAX, ra);
            let rd = self.def_i(d, R10);
            self.a.mov_rr(rd, RAX);
            self.fin_i(d, rd);
        } else {
            // branchless (data-dependent selects mispredict): xmm0 = (c == 0) as a 64-bit
            // mask, then blendvpd picks b where it is set (its sign bit covers both lane
            // widths' low lane)
            let rc = self.use_i(c, R10);
            self.a.sse_rr(0x66, 0x6e, 0, rc, true); // movq xmm0, rc
            self.a.sse_rr(0x66, 0xef, 1, 1, false); // pxor xmm1, xmm1
            self.a.sse_ext(0x66, 0x38, 0x29, 0, 1); // pcmpeqq xmm0, xmm1
            if self.avx {
                // mask (c == 0) picks b
                let xa = self.use_f(a, XS);
                let xb = self.use_f(b, XS2);
                let xd = self.def_f(d, XS);
                self.vblendv(true, xd, xa, xb, 0);
                self.fin_f(d, xd);
                return;
            }
            let xa = self.use_f(a, XS);
            self.a.movf_rr(true, 1, xa);
            let xb = self.use_f(b, XS2);
            self.a.sse_ext(0x66, 0x38, 0x15, 1, xb); // blendvpd xmm1, xb, xmm0
            let xd = self.def_f(d, XS);
            self.a.movf_rr(true, xd, 1);
            self.fin_f(d, xd);
        }
    }

    /// Fused multiply-add (FMA3, one rounding): xmm0 = c, then vf[n]m{add,sub}231 xmm0, a, b.
    #[allow(clippy::too_many_arguments)]
    fn fma(&mut self, neg_prod: bool, neg_add: bool, w: bool, d: VReg, a: VReg, b: VReg, c: VReg) {
        let xa = self.use_f(a, XS);
        let xb = self.use_f(b, XS2);
        let xc = self.use_f(c, 1);
        self.a.movf_rr(true, 0, xc);
        let op = match (neg_prod, neg_add) {
            (false, false) => 0xb9, // vfmadd231
            (false, true) => 0xbb,  // vfmsub231
            (true, false) => 0xbd,  // vfnmadd231
            (true, true) => 0xbf,   // vfnmsub231
        };
        // VEX.LIG.66.0F38.W{0,1} op /r: reg = xmm0, vvvv = xa, rm = xb
        let r = 0u8;
        self.a.byte(0xc4);
        self.a.byte(((!(r >> 3) & 1) << 7) | (1 << 6) | ((!(xb >> 3) & 1) << 5) | 0b00010);
        self.a.byte(((w as u8) << 7) | ((!xa & 15) << 3) | 0b01);
        self.a.byte(op);
        self.a.modrm_rr(r, xb);
        let xd = self.def_f(d, XS);
        self.a.movf_rr(true, xd, 0);
        self.fin_f(d, xd);
    }

    /// Round half away from zero: trunc(x + copysign(0.5 - ulp/2, x)) -- exact for every
    /// f32 (checked exhaustively) and f64 (sampled), NaN payloads included.
    fn round_away(&mut self, w: bool, xd: u8, xa: u8) {
        let pfx = if w { 0xf2 } else { 0xf3 };
        let (sign, half): (u64, u64) = if w {
            (0x8000_0000_0000_0000, 0.49999999999999994f64.to_bits())
        } else {
            (0x8000_0000, 0.49999997f32.to_bits() as u64)
        };
        if self.avx {
            // xmm0 = (x & sign) | half; xmm0 = x + xmm0; xd = trunc(xmm0)
            self.vex_pool(1, 0x54, 0, xa, sign);
            self.vex_pool(1, 0x56, 0, 0, half);
            self.a.vex_rr(if w { 3 } else { 2 }, 1, false, 0x58, 0, xa, 0);
            // vroundsd/ss xd, xmm0, xmm0, 11
            self.a.vex_rr(1, 3, false, if w { 0x0b } else { 0x0a }, xd, 0, 0);
            self.a.byte(11);
            return;
        }
        self.a.movq_rx(R10, xa, true);
        self.a.mov_ri(R11, sign as i64);
        self.a.alu_rr(0x21, R10, R11);
        self.a.mov_ri(R11, half as i64);
        self.a.alu_rr(0x09, R10, R11);
        self.a.movq_xr(1, R10, true);
        self.a.movf_rr(true, 0, xa);
        self.a.sse_rr(pfx, 0x58, 0, 1, false); // x + copysign(c, x)
        self.a.roundf(w, xd, 0, 3); // trunc
    }

    fn ibin(&mut self, op: IOp, it: IntTy, d: VReg, a: VReg, b: VReg) {
        // a spilled first operand of a register result loads straight into it, unless b lives there
        let ra = match (self.al.loc[a.0 as usize], self.al.loc[d.0 as usize], self.al.loc[b.0 as usize]) {
            (Loc::Stack(o), Loc::Reg(r), lb) if lb != Loc::Reg(r) && !matches!(op, IOp::Div | IOp::Rem | IOp::Shl | IOp::Shr) => {
                let disp = self.spill_disp(o);
                self.load_spill(r, disp);
                r
            }
            _ => self.use_i(a, R10),
        };
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
            IOp::Shl | IOp::Shr if it == U32 => {
                // 32-bit shift by cl: zero-extends, count mod 32 (callers mask it)
                self.a.mov_rr(RCX, rb);
                self.a.mov_rr(RAX, ra);
                self.a.rex(false, 0, 0, RAX, false);
                self.a.byte(0xd3);
                self.a.modrm_rr(if op == IOp::Shl { 4 } else { 5 }, RAX);
                self.a.mov_rr(rd, RAX);
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
        if it == U32 {
            // 32-bit forms zero-extend for free
            match op {
                IOp::Add => self.a.alu_rr32(0x01, work, rb),
                IOp::Sub => self.a.alu_rr32(0x29, work, rb),
                IOp::And => self.a.alu_rr32(0x21, work, rb),
                IOp::Or => self.a.alu_rr32(0x09, work, rb),
                IOp::Xor => self.a.alu_rr32(0x31, work, rb),
                _ => {
                    // imul r32, r32
                    self.a.rex(false, work, 0, rb, false);
                    self.a.byte(0x0f);
                    self.a.byte(0xaf);
                    self.a.modrm_rr(work, rb);
                }
            }
            self.a.mov_rr(rd, work);
            self.fin_i(d, rd);
            return;
        }
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
                    self.a.break_dep(xd, 0xff);
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
                    self.a.break_dep(xd, 0xff);
                    self.a.sse_rr(pfx, 0x2a, xd, RAX, true);
                    self.a.sse_rr(pfx, 0x58, xd, xd, false);
                    let end = self.a.pos();
                    self.a.patch(q, end);
                } else {
                    // values are extended to 64 bits by type, so a 64-bit signed convert is exact
                    self.a.break_dep(xd, 0xff);
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
                    self.a.break_dep(XS2, xs);
                    self.a.sse_rr(0xf3, 0x5a, XS2, xs, false); // cvtss2sd
                }
                let rd = self.def_i(d, R10);
                self.f_to_int(rd, it);
                self.fin_i(d, rd);
            }
            Conv::F32ToF64 => {
                let xs = self.use_f(s, XS);
                let xd = self.def_f(d, XS);
                self.a.break_dep(xd, xs);
                self.a.sse_rr(0xf3, 0x5a, xd, xs, false);
                self.fin_f(d, xd);
            }
            Conv::F64ToF32 => {
                let xs = self.use_f(s, XS);
                let xd = self.def_f(d, XS);
                self.a.break_dep(xd, xs);
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
        if it.signed && it.bits == 32 {
            // cvttsd2si eax gives 0x80000000 only for NaN / out of range (`cmp eax, 1`
            // overflows exactly then): the exact saturating path runs only for those
            self.a.sse_rr(0xf2, 0x2c, RAX, XS2, false);
            self.a.byte(0x83);
            self.a.modrm_rr(7, RAX);
            self.a.byte(1);
            let p = self.a.jcc32(CC_O);
            self.a.extend(RAX, 32, true);
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
            Callee::Host(addr) => {
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
        // a spilled operand of a register result loads straight into it (no scratch + copy)
        let ra = match (self.al.loc[a.0 as usize], self.al.loc[d.0 as usize]) {
            (Loc::Stack(o), Loc::Reg(r)) if !matches!(op, IOp::Div | IOp::Rem) => {
                let disp = self.spill_disp(o);
                self.load_spill(r, disp);
                r
            }
            _ => self.use_i(a, R10),
        };
        let rd = self.def_i(d, R10);
        let w = it != U32;
        if !w && matches!(op, IOp::Div | IOp::Rem) {
            // not formed by the optimiser; keep the general path
        } else if !w {
            // u32: 32-bit forms zero-extend for free
            match op {
                IOp::Mul => {
                    self.a.rex(false, rd, 0, ra, false);
                    self.a.byte(0x69);
                    self.a.modrm_rr(rd, ra);
                    self.a.u32(k as i32 as u32);
                }
                IOp::Shl | IOp::Shr => {
                    self.a.mov_rr(rd, ra);
                    self.a.rex(false, 0, 0, rd, false);
                    self.a.byte(0xc1);
                    self.a.modrm_rr(if op == IOp::Shl { 4 } else { 5 }, rd);
                    self.a.byte((k & 31) as u8);
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
                    self.a.rex(false, 0, 0, rd, false);
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
            self.fin_i(d, rd);
            return;
        }
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

fn use_gather() -> bool {
    static G: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *G.get_or_init(|| std::env::var("RAPID_GATHER").is_ok())
}

/// The vreg a compare defines.
fn cmp_def(i: &Inst) -> Option<VReg> {
    match i {
        Inst::ICmp(_, _, d, _, _) | Inst::ICmpI(_, _, d, _, _) | Inst::FCmp(_, _, d, _, _) => Some(*d),
        _ => None,
    }
}

/// Does this CPU (and the environment: RAPID_NO_AVX) allow VEX-encoded AVX instructions?
pub fn host_avx() -> bool {
    static AVX: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *AVX.get_or_init(|| {
        #[cfg(target_arch = "x86_64")]
        {
            std::is_x86_feature_detected!("avx") && std::env::var("RAPID_NO_AVX").is_err()
        }
        #[cfg(not(target_arch = "x86_64"))]
        false
    })
}

/// popcnt / lzcnt / tzcnt available (x86-64-v3 has all three; older CPUs get the
/// shift/mask expansions).
pub fn host_bits() -> (bool, bool, bool) {
    static B: std::sync::OnceLock<(bool, bool, bool)> = std::sync::OnceLock::new();
    *B.get_or_init(|| {
        #[cfg(target_arch = "x86_64")]
        {
            if std::env::var("RAPID_NO_BITOPS").is_ok() {
                return (false, false, false);
            }
            (std::is_x86_feature_detected!("popcnt"), std::is_x86_feature_detected!("lzcnt"), std::is_x86_feature_detected!("bmi1"))
        }
        #[cfg(not(target_arch = "x86_64"))]
        (false, false, false)
    })
}

pub fn compile(f: &Func, env: &Env) -> Result<Compiled, String> {
    let expanded;
    let f = if f.blocks.iter().any(|b| b.insts.iter().any(|i| matches!(i, Inst::IBit(..)))) {
        let (pc, lz, tz) = host_bits();
        let mut g = f.clone();
        crate::opt::expand_bit_ops(&mut g, &|o| match o {
            BitOp::Popcnt => pc,
            BitOp::Clz => lz,
            BitOp::Ctz => tz,
            BitOp::Bswap => true,
            BitOp::Rev => false,
        });
        expanded = g;
        &expanded
    } else {
        f
    };
    let mut cfg = reg_config();
    let vectors = f.vregs.contains(&Cls::V128);
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
    if !uses_rcx_rdx && std::env::var("RAPID_NO_RCX").is_err() {
        cfg.int_caller.push(RCX);
        cfg.int_caller.push(RDX);
    }
    let al = regalloc::allocate(f, &cfg);
    if vectors {
        // caller-saved saves around calls are 8 bytes: no vector may live across a call
        for sv in al.saves.values() {
            for (v, _) in sv {
                if f.vregs[v.0 as usize] == Cls::V128 {
                    return Err(format!("{}: a vector lives across a call", f.name));
                }
            }
        }
    }
    let mut g = Gen {
        last_spill: None,
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
        memconst: std::env::var("RAPID_MEMCONST").is_ok() || host_avx(),
        avx: host_avx(),
        sel_cmp: None,
        vpool: Vec::new(),
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

/// `jmp target` through r11 (caller-saved, never an argument register): every argument
/// register (incl. al for variadic calls) and the stack arrive at `target` unchanged.
pub fn jump_to(target: u64) -> Vec<u8> {
    let mut a = Asm { b: Vec::new(), sib: None };
    a.mov_ri(R11, target as i64);
    a.jmp_r(R11);
    a.b
}

/// `rdi = arg; jmp target` (tail jump: [rsp] stays the JIT caller's return address).
pub fn jump_with_arg0(arg: u64, target: u64) -> Vec<u8> {
    let mut a = Asm { b: Vec::new(), sib: None };
    a.mov_ri(RDI, arg as i64);
    a.mov_ri(R11, target as i64);
    a.jmp_r(R11);
    a.b
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

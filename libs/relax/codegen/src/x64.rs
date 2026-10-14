//! x86-64 backend (O1): liveness, linear-scan register allocation over
//! hull intervals, instruction selection and encoding. Produces position-
//! independent code apart from absolute addresses for data, the function
//! table and host functions (fine in-process; AOT adds relocations later).
//!
//! ABI (Relax's own, on every x86-64 OS, and the runtime's host entries): int args rdi rsi rdx
//! rcx r8 r9, float args xmm0-7, int returns rax rdx, float returns xmm0 xmm1. Callee-saved
//! rbx rbp r12-r15. Reserved scratch: rax rcx rdx r10 r11, xmm0 xmm1 xmm14 xmm15.
//! C-family boundaries (CHost / CIndirect / CFn calls and bodies) use the signature's convention:
//! the host's C ABI, explicit SysV, or explicit Win64 (rcx rdx r8 r9 / xmm0-3 by position, shadow
//! space; a Win64 body also keeps rsi, rdi and xmm6-15, which Relax's own convention clobbers).

use crate::cabi::{self, ArgPlan, PLoc, Plan, RetPlan};
use crate::regalloc::{self, Alloc, Loc, MLoc, RegConfig, Remat};
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
/// Integer argument registers of a signature's C ABI (`PLoc::Int(k)` of a cabi plan).
fn c_int_args(target: cabi::Target) -> &'static [u8] {
    match target {
        cabi::Target::Win64 => &[RCX, RDX, R8, R9],
        _ => &INT_ARGS,
    }
}
/// Win64 C bodies: rsi, rdi, then xmm6-15 (16 bytes each) saved below the frame's other areas
const WIN64_SAVES: i32 = 16 + 10 * 16;
/// the segment register of the thread block the runtime's TLS key indexes (fs: Linux static TLS,
/// gs: Windows TEB.TlsSlots)
const TLS_SEG: u8 = match cabi::HOST {
    cabi::Target::Win64 => 0x65,
    _ => 0x64,
};
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
    /// regalloc::alloc_sig of the allocation (0 unless regalloc::SIG_ON)
    pub alloc_sig: u64,
    /// the relocated fields, emitted zeroed: the embedder links the code (`rir::link`) where it
    /// installs it
    pub relocs: Vec<RelocAt>,
    /// (code offset, block, instruction index; u32::MAX = the terminator) of the function as
    /// emitted -- filled only while `set_rir_map(true)` (asm dumps: tools/run/tools/asmcmp)
    pub rir_map: Vec<(u32, u32, u32)>,
    /// the emitted function's RIR (after the back end's own splits and rewrites), with rir_map
    pub rir_emitted: String,
}

static RIR_MAP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Record `Compiled::rir_map` (code offset -> RIR block/instruction) in later compiles.
pub fn set_rir_map(on: bool) {
    RIR_MAP.store(on, std::sync::atomic::Ordering::Relaxed);
}

/// Addresses the backend needs from the runtime.
pub struct Env {
    /// where the function slots' tables and thunks are
    pub slots: SlotsPtr,
    /// fs-relative offset of the runtime's per-thread block pointer
    pub tls_key: u64,
    /// glue that allocates the calling thread's block: returns it in r11, preserves all else
    pub tls_slow: u64,
    /// `Term::Resume`'s call target (the runtime's resume trampoline; never returns)
    pub resume: u64,
    /// thorough register allocation (live-range splitting at loops, copy coalescing): for code
    /// that runs hot (O3, compute kernels); first compiles take the quicker allocation
    pub opt_regs: bool,
    /// the embedder's symbol value table (u64 per `Sym::id`)
    pub syms: u64,
    /// how each symbol may be encoded (`SymClass` as u8 per `Sym::id`; 0 = no table: all Far)
    pub sym_class: u64,
}

impl Env {
    /// The current value of a relocated word.
    pub fn reloc_value(&self, r: Reloc) -> u64 {
        match r {
            Reloc::Sym(s) => unsafe { s.value(self.syms) },
            Reloc::Slot(id) => self.slots.table(id),
            Reloc::Thunk(id) => self.slots.callable(id),
            // fs-relative offset of the block pointer cell (mov r11, fs:[rax])
            Reloc::TlsKey => self.tls_key,
            Reloc::TlsSlow => self.tls_slow,
            Reloc::Resume => self.resume,
            Reloc::Value(v) => v,
        }
    }
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
    /// mov_ri that leaves the flags alone (no `xor r, r` for 0): operands materialised between a
    /// fused compare and its jcc / cmovcc / setcc
    fn mov_ri_nf(&mut self, dst: u8, v: i64) {
        if v == 0 {
            self.rex(false, 0, 0, dst, false);
            self.byte(0xb8 + (dst & 7));
            self.u32(0);
        } else {
            self.mov_ri(dst, v);
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
    /// add dst, [base + disp] (64-bit)
    fn add_load64(&mut self, dst: u8, base: u8, disp: i32) {
        self.rex(true, dst, 0, base, false);
        self.byte(0x03);
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
        // rel32 (jmp / jcc / rip-relative pool loads): +-2 GB, never truncated
        assert!(rel >= i32::MIN as i64 && rel <= i32::MAX as i64, "x64 rel32 out of range: {}", rel);
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
    /// 66 [rex.w] 0F [esc] op /r with an explicit REX.W (mixed xmm / general register forms)
    fn sse_w(&mut self, esc: u8, op: u8, reg: u8, rm: u8, w: bool) {
        self.byte(0x66);
        self.rex(w, reg, 0, rm, false);
        self.byte(0x0f);
        if esc != 0 {
            self.byte(esc);
        }
        self.byte(op);
        self.modrm_rr(reg, rm);
    }
    /// 66 0F op /ext ib: the shift-by-immediate groups (71 w, 72 d, 73 q / byte shifts)
    fn sse_grp_imm(&mut self, op: u8, ext: u8, rm: u8, imm: u8) {
        self.byte(0x66);
        self.rex(false, 0, 0, rm, false);
        self.byte(0x0f);
        self.byte(op);
        self.modrm_rr(ext, rm);
        self.byte(imm);
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

/// rcx and rdx as an instruction's own scratch: divisions and multiply-high (rdx:rax), variable
/// shifts (cl), conversions, cmpxchg loops. Values live there go to other registers.
fn x64_clobbers(options: &relax_session::Options, i: &Inst) -> u64 {
    match i {
        Inst::IBin(IOp::Shl | IOp::Shr, _, _, _, _) if host_bmi2(options) => 0,
        Inst::IBin(IOp::Div | IOp::Rem | IOp::Shl | IOp::Shr | IOp::Rotl | IOp::MulHi, _, _, _, _) | Inst::Conv(..) | Inst::AtomicRmw(..) | Inst::AtomicCas(..) => (1 << RCX) | (1 << RDX),
        _ => 0,
    }
}

/// BMI2 (shlx / shrx / sarx) on this CPU, unless MAKEPAD_RUN_NO_BMI2: the JIT uses what the machine has.
/// MAKEPAD_RUN_CHECK_WINDOWS=1: every read of a spill window's register checks it against the value's
/// slot and traps (ud2) when they differ -- finds a window whose register something clobbered.
fn check_windows(options: &relax_session::Options) -> bool {
    options.check_windows
}

pub fn host_bmi2(options: &relax_session::Options) -> bool {
    options.cpu.bmi2
}

pub fn reg_config(options: &relax_session::Options) -> RegConfig {
    let mut flt = Vec::new();
    for x in 2..14u8 {
        flt.push(x);
    }
    let mut int_caller = CALLER_SAVED_ALLOC.to_vec();
    if !options.no_rcx {
        int_caller.push(RCX);
        int_caller.push(RDX);
    }
    RegConfig { int_caller, int_callee: CALLEE_SAVED.to_vec(), flt_caller: flt, flt_callee: Vec::new(), slot_index_free: true, clobbers: Some(x64_clobbers), sym_class: 0 }
}

// ------------------------------------------------------------ code generation

struct Gen<'a> {
    a: Asm,
    f: &'a Func,
    al: Alloc,
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
    /// address pool: (disp32 patch offset, relocated value), 8-byte entries
    apool: Vec<(usize, Reloc)>,
    /// `Env::sym_class`: how each symbol is encoded (SymClass::Domain: an offset in the domain block)
    sym_class: u64,
    /// rbp offset of the frame word holding the domain base (0: the function names no domain symbol)
    dom_slot: i32,
    relocs: Vec<RelocAt>,
    /// emission order successor of each block (u32::MAX for the last)
    next_block: Vec<u32>,
    /// block -> where its code goes on: blocks that emit only their jump (moves the allocator
    /// coalesced, edge blocks of split ranges) are passed through by the jumps into them
    thru: Vec<u32>,
    frame: i32,
    text_len: u32,
    /// a single-use float constant whose use (the next FBin) reads it from the pool
    mem_const: Option<(VReg, u64)>,
    /// the block ends `t = x & K; t ==/!= 0; branch`: emitted as test x, K (the And skipped)
    test_fuse: Option<(VReg, i64)>,
    /// `t = load [base + off]; c = t cmp K` feeding the branch (t used once): cmp [base + off], K
    /// in the load's width -- (base, off, bytes, K)
    load_fuse: Option<(VReg, i32, u8, i64)>,
    /// the last spill store emitted: (rbp displacement, register, position right after it)
    last_spill: Option<(i32, u8, usize)>,
    /// spill windows (Alloc::windows): vreg -> (first use pos, last use pos, register); and the
    /// loads to emit before the instruction at a position
    win: std::collections::HashMap<u32, Vec<(u32, u32, u8)>>,
    win_start: std::collections::HashMap<u32, Vec<(u32, u8)>>,
    rir_map: Vec<(u32, u32, u32)>,
    rir_map_on: bool,
    /// (register, code position right after it): the last instruction emitted was an integer ALU op
    /// (add/sub/and/or/xor, not lea) writing that register; its ZF says whether the result is 0
    last_alu: Option<(u8, usize)>,
    /// jump tables (arm64::jump_tables): head block -> (x, min, targets, default)
    jt_plan: std::collections::HashMap<u32, (VReg, i64, Vec<u32>, u32)>,
    /// table entries to fill once blocks are placed: (byte position, table start, target)
    jt_fix: Vec<(usize, usize, u32)>,
    /// single-def float constants: vreg -> (bits, f64?)
    fconsts: std::collections::HashMap<u32, (u64, bool)>,
    memconst: bool,
    /// AVX: three-operand VEX forms of scalar float ops (and memory-operand constants)
    avx: bool,
    /// Values whose bytes are all 0x00 / 0xff, so VSel can use a byte blend.
    byte_masks: Vec<bool>,
    /// the compare fused into the next select
    sel_cmp: Option<Inst>,
    /// an integer compare whose result has other uses, emitted right before a select on it:
    /// (its result, condition code) -- the select's cmov reads the flags, no test
    sel_flags: Option<(VReg, u8)>,
    /// C ABI body: its plan, per-argument buffer (rbp disp, 0 = none), result buffer,
    /// the cell keeping an indirect result address for rax
    cplan: Option<Plan>,
    cbuf: Vec<i32>,
    cret_buf: i32,
    cret_ptr: i32,
    /// Win64 C body: rbp disp of the rsi / rdi / xmm6-15 save area
    win_saves: i32,
    /// rbp disp of the C call staging cells (8 bytes each, ascending)
    stage: i32,
    /// code offset right after the last call instruction (its return address)
    last_call_ret: usize,
    /// invoked calls: (return offset, caller-saved values to reload, cleanup pad block)
    pads: Vec<(usize, Vec<(VReg, i32)>, u32)>,
    /// (return offset, landing stub offset) of each invoked call
    unwind_tab: Vec<(u32, u32)>,
}

impl<'a> Gen<'a> {
    /// Saves (or restores) a caller-saved register's value around a call: 16 bytes for a vector.
    fn save_restore(&mut self, v: VReg, slot: i32, save: bool) {
        let d = self.spill_disp(slot);
        if self.f.vregs[v.0 as usize] == Cls::V128 {
            if let Loc::Flt(x) = self.al.loc[v.0 as usize] {
                // movups [rbp+d], x / movups x, [rbp+d]
                self.a.sse_rm(0, if save { 0x11 } else { 0x10 }, x, RBP, d);
            }
            return;
        }
        if let Some(r) = self.mloc(v) {
            if save {
                self.mv(MLoc::M(d), r);
            } else {
                self.mv(r, MLoc::M(d));
            }
        }
    }

    /// a single-def float constant of this width that is not NaN
    fn non_nan_const(&self, v: VReg, f64_: bool) -> bool {
        match self.fconsts.get(&v.0) {
            Some((bits, w)) if *w == f64_ => {
                if f64_ {
                    !f64::from_bits(*bits).is_nan()
                } else {
                    !f32::from_bits(*bits as u32).is_nan()
                }
            }
            _ => false,
        }
    }

    fn spill_disp(&self, off: i32) -> i32 {
        -(self.saved + off)
    }

    /// Integer operand in a register (loads spills into `scratch`).
    fn use_i(&mut self, options: &relax_session::Options, v: VReg, scratch: u8) -> u8 {
        match self.al.loc[v.0 as usize] {
            Loc::Reg(r) => r,
            Loc::Stack(o) => {
                // held in a register over this run of uses (Alloc::windows)
                if let Some(ws) = self.win.get(&v.0) {
                    let p = self.cur_pos;
                    if let Some(&(_, _, r)) = ws.iter().find(|(a, z, _)| *a <= p && p <= *z) {
                        if check_windows(options) {
                            // the slot stays the value's home: the register must still equal it
                            // (pushfq; cmp r, [rbp+d]; je +2; ud2; popfq -- flags kept)
                            let d = self.spill_disp(o);
                            self.a.byte(0x9c);
                            self.a.rex(true, r, 0, RBP, false);
                            self.a.byte(0x3b);
                            self.a.modrm_mem(r, RBP, d);
                            self.a.byte(0x74);
                            self.a.byte(0x02);
                            self.a.byte(0x0f);
                            self.a.byte(0x0b);
                            self.a.byte(0x9d);
                        }
                        return r;
                    }
                }
                let d = self.spill_disp(o);
                self.load_spill(scratch, d);
                scratch
            }
            _ => {
                // flags-preserving: a fused compare may have set them for the next jcc / cmov
                match self.al.remat[v.0 as usize] {
                    Remat::Const(k) => self.a.mov_ri_nf(scratch, k),
                    Remat::Slot(s) => {
                        let off = self.slot_off[s as usize];
                        self.a.lea(scratch, RBP, -(self.saved + off));
                    }
                    // mov r, [rip + pool] leaves the flags alone
                    Remat::Addr(a) => self.mov_reloc(scratch, Reloc::Sym(a)),
                    Remat::FnAddr(id) => self.mov_reloc(scratch, Reloc::Thunk(id)),
                    _ => self.a.mov_ri_nf(scratch, 0),
                }
                scratch
            }
        }
    }
    /// A load/store base and displacement: a stack-slot address that has no register (see
    /// Alloc::remat) folds into [rbp + disp].
    fn base_disp(&mut self, options: &relax_session::Options, base: VReg, scratch: u8, off: i32) -> (u8, i32) {
        if let (Loc::None, Remat::Slot(s)) = (self.al.loc[base.0 as usize], self.al.remat[base.0 as usize]) {
            let so = self.slot_off[s as usize];
            return (RBP, off - (self.saved + so));
        }
        (self.use_i(options, base, scratch), off)
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
    /// dst = the spill slot at disp: a register move (or nothing) when the slot was stored from a
    /// register by the very last instruction emitted (same block: block starts clear it)
    fn load_spill(&mut self, dst: u8, disp: i32) {
        if let Some((d, r, p)) = self.last_spill {
            if d == disp && p == self.a.pos() {
                self.a.mov_rr(dst, r);
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

    fn gen(&mut self, options: &relax_session::Options) {
        let reach = regalloc::reachable(self.f);
        // prologue
        self.a.push(RBP);
        self.a.mov_rr(RBP, RSP);
        for r in self.al.used_callee_int.clone() {
            self.a.push(r);
        }
        self.saved = 8 * self.al.used_callee_int.len() as i32;
        // only slots something addresses take space; slots whose lifetimes cannot overlap share
        // it (opt::slot_layout)
        let (at, end) = crate::opt::slot_layout(self.f, self.al.spill_bytes as i64, !crate::opt::opt_off(options, "-slotcolor"));
        self.slot_off.extend(at.iter().map(|&x| x as i32));
        let mut off = end as i32;
        off = (off + 7) / 8 * 8;
        // C ABI body: buffers rebuilding aggregate params from registers, the result buffer
        if let Some(sig) = &self.f.cabi {
            let plan = cabi::plan(sig, sig.conv.target());
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
            if sig.conv.target() == cabi::Target::Win64 {
                off += WIN64_SAVES;
                self.win_saves = -(self.saved + off);
            }
        }
        // staging cells for C calls (cell 0: the result address; then aggregate pieces)
        let mut cells = 0i32;
        for b in &self.f.blocks {
            for i in &b.insts {
                if let Inst::Call(Callee::CHost(_, sig) | Callee::CIndirect(_, sig) | Callee::CFn(_, sig), _, _) = i {
                    let plan = cabi::plan(sig, sig.conv.target());
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
        // the domain base, read once in the prologue when the function names a domain static
        if uses_domain(self.f, self.sym_class) {
            off += 8;
            self.dom_slot = -(self.saved + off);
        }
        let mut frame = (off + 7) / 8 * 8;
        // keep rsp 16-aligned at calls
        while (self.saved + frame) % 16 != 0 {
            frame += 8;
        }
        self.frame = frame;
        self.stack_probe(frame);
        self.a.sub_rsp(frame as u32);
        if self.dom_slot != 0 {
            self.dom_base_prologue();
        }
        if self.cplan.is_some() {
            self.win64_saves(true);
            self.c_prologue(options);
        } else {
            self.params_prologue();
        }
        self.blocks_and_pool(options, reach);
    }

    /// Relax ABI params: ABI registers -> allocated locations (parallel move).
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

    fn blocks_and_pool(&mut self, options: &relax_session::Options, reach: Vec<bool>) {
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
        self.thru_blocks(&reach);
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
            // `t = x & K; c = t ==/!= 0` feeding the branch (t used once): test x, K
            self.test_fuse = None;
            if fused && n >= 2 {
                if let (Inst::IBinI(IOp::And, it, t, x, k), Inst::ICmpI(Cond::Eq | Cond::Ne, _, _, t2, 0)) = (&b.insts[n - 2], &b.insts[n - 1]) {
                    // imm32 is sign-extended: a narrower value's mask must not reach its upper bits
                    let lo = if it.bits == 64 { i32::MIN as i64 } else { 0 };
                    if t == t2 && *x != *t && self.al.uses[t.0 as usize] == 1 && (lo..=i32::MAX as i64).contains(k) {
                        self.test_fuse = Some((*x, *k));
                    }
                }
            }
            // `t = load [p + o]; c = t cmp K` feeding the branch (t used once): cmp [p + o], K
            self.load_fuse = None;
            if fused && n >= 2 && self.test_fuse.is_none() {
                if let (Inst::Load(Mem::Int(sz, sx), t, base, off), Inst::ICmpI(cond, signed, _, t2, k)) = (&b.insts[n - 2], &b.insts[n - 1]) {
                    if t == t2 && base != t && self.al.uses[t.0 as usize] == 1 && load_cmp_fits(*sz, *sx, *cond, *signed, *k) {
                        self.load_fuse = Some((*base, *off, *sz, *k));
                    }
                }
            }
            // a branch on a loaded value itself (a bool field): cmp [p + o], 0
            let mut load_branch = false;
            if !fused && n >= 1 {
                if let (Term::Branch(c, _, _), Inst::Load(Mem::Int(sz, _), t, base, off)) = (&b.term, &b.insts[n - 1]) {
                    if c == t && base != t && self.al.uses[t.0 as usize] == 1 {
                        self.load_fuse = Some((*base, *off, *sz, 0));
                        load_branch = true;
                    }
                }
            }
            // a block start can be a jump target: no spill store before it carries over, nor
            // the flags of the ALU op that ended the block above (a jump arrives with other ones)
            self.last_spill = None;
            self.last_alu = None;
            for (ii, inst) in b.insts.iter().enumerate() {
                self.pc_map.push((self.a.pos() as u32, b.pos[ii]));
                if self.rir_map_on {
                    self.rir_map.push((self.a.pos() as u32, bi as u32, ii as u32));
                }
                self.cur_pos = self.al.inst_pos(bi, ii);
                if let Some(ls) = self.win_start.get(&self.cur_pos).cloned() {
                    for (v, r) in ls {
                        if let Loc::Stack(o) = self.al.loc[v as usize] {
                            let d = self.spill_disp(o);
                            self.load_spill(r, d);
                        }
                    }
                }
                if fused && ii == n - 1 {
                    break;
                }
                if (self.test_fuse.is_some() || self.load_fuse.is_some() && !load_branch) && ii == n - 2 {
                    continue;
                }
                if load_branch && ii == n - 1 {
                    break;
                }
                // a compare whose one use is the next select: fused into it (flags + cmov,
                // or a scalar float branch)
                if ii + 1 < n && !(fused && ii + 1 == n - 1) {
                    if let (Some(cd), Inst::Sel(sd, sc, sa, sb)) = (cmp_def(inst), &b.insts[ii + 1]) {
                        let data = self.f.vregs[sd.0 as usize];
                        let ok = match inst {
                            Inst::ICmp(..) | Inst::ICmpI(..) => true,
                            Inst::FCmp(..) => matches!(data, Cls::F32 | Cls::F64),
                            _ => false,
                        };
                        if *sc == cd && self.al.uses[cd.0 as usize] == 1 && ok {
                            self.sel_cmp = Some(inst.clone());
                            continue;
                        }
                        // Keep a shared compare (e.g. argmax's index), but let the float
                        // select use scalar min/max independently of the bool materialization.
                        if *sc == cd && ok && matches!(data, Cls::F32 | Cls::F64) && scalar_minmax(inst, *sa, *sb).is_some() {
                            self.inst(options, inst);
                            self.sel_cmp = Some(inst.clone());
                            continue;
                        }
                        // other uses: the compare is emitted (setcc) and the select reads its flags
                        if *sc == cd && data == Cls::I {
                            if let Inst::ICmp(cc, sg, ..) | Inst::ICmpI(cc, sg, ..) = inst {
                                self.inst(options, inst);
                                self.sel_flags = Some((cd, icc(*cc, *sg)));
                                continue;
                            }
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
                // A min/max select can also follow other instructions/users of the
                // compare. Stop at any redefinition of the bool or selected values:
                // RIR is not necessarily SSA (loop-carried values are often assigned).
                if self.sel_cmp.is_none() {
                    if let Inst::Sel(d, c, a, z) = inst {
                        if matches!(self.f.vregs[d.0 as usize], Cls::F32 | Cls::F64) {
                            for prev in b.insts[..ii].iter().rev() {
                                let mut changed = false;
                                crate::regalloc::for_defs(prev, |v| changed |= v == *c || v == *a || v == *z);
                                if changed {
                                    if cmp_def(prev) == Some(*c) && scalar_minmax(prev, *a, *z).is_some() {
                                        self.sel_cmp = Some(prev.clone());
                                    }
                                    break;
                                }
                            }
                        }
                    }
                }
                self.inst(options, inst);
            }
            self.pc_map.push((self.a.pos() as u32, b.term_pos));
            if self.rir_map_on {
                self.rir_map.push((self.a.pos() as u32, bi as u32, u32::MAX));
            }
            let fuse_inst = if fused { Some(b.insts[n - 1].clone()) } else { None };
            self.term(options, &b.term, bi, fuse_inst);
        }
        // landing stubs of invoked calls: the unwinder resumes here with rbp, rsp and the
        // callee-saved registers of this frame restored; reload the caller-saved values the
        // call's normal path reloads, then the cleanup pad
        let mut unwind_tab: Vec<(u32, u32)> = Vec::new();
        for (ret, saves, pad) in std::mem::take(&mut self.pads) {
            // only the values the pad reads come back (live into the pad block); none: the
            // unwinder lands on the pad itself
            let at = self.al.block_start[pad as usize];
            let reload: Vec<(VReg, i32)> = saves.into_iter().filter(|(v, _)| self.al.start[v.0 as usize] <= at && self.al.end[v.0 as usize] >= at).collect();
            if reload.is_empty() {
                unwind_tab.push((ret as u32, self.block_pos[pad as usize] as u32));
                continue;
            }
            let stub = self.a.pos();
            for (v, slot) in &reload {
                self.save_restore(*v, *slot, false);
            }
            let p = self.a.jmp32();
            self.fixups.push((p, pad));
            unwind_tab.push((ret as u32, stub as u32));
        }
        self.unwind_tab = unwind_tab;
        for (at, t) in std::mem::take(&mut self.fixups) {
            let target = self.block_pos[t as usize];
            self.a.patch(at, target);
        }
        for (at, start, t) in std::mem::take(&mut self.jt_fix) {
            let rel = self.block_pos[t as usize] as i64 - start as i64;
            self.a.b[at..at + 4].copy_from_slice(&(rel as i32).to_le_bytes());
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
        // address pool (one 8-byte word per distinct relocated value; Compiled::relocs)
        if !self.apool.is_empty() {
            while self.a.pos() % 8 != 0 {
                self.a.byte(0xcc);
            }
            let mut placed: Vec<(Reloc, usize)> = Vec::new();
            for (at, r) in std::mem::take(&mut self.apool) {
                let addr = match placed.iter().find(|(r2, _)| *r2 == r) {
                    Some((_, a2)) => *a2,
                    None => {
                        let a2 = self.a.pos();
                        self.a.u64(0);
                        placed.push((r, a2));
                        self.relocs.push(RelocAt { at: a2 as u32, kind: RelocKind::Pool64, target: r });
                        a2
                    }
                };
                self.a.patch(at, addr);
            }
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

    /// `vop xd, xa, [rip + c]` (AVX, VEX.66.0F: and/or/xor/add... packed-double forms read
    /// 16 bytes from the 8-byte pool entry; the upper lane is never used)
    fn vex_pool(&mut self, pp: u8, op: u8, xd: u8, xa: u8, bits: u64) {
        let at = self.a.vex_rip(pp, 1, false, op, xd, xa);
        self.fpool.push((at, bits));
    }

    /// Prologue of a C-family body: aggregate params are rebuilt in frame
    /// buffers (their address is the param), memory-class ones are addressed in the
    /// caller's argument area, an indirect result pointer is kept for rax.
    fn c_prologue(&mut self, options: &relax_session::Options) {
        let plan = self.cplan.clone().unwrap();
        let sig = self.f.cabi.clone().unwrap();
        let int_args = c_int_args(sig.conv.target());
        let params = self.f.params.clone();
        let has_ret = sig.ret.is_some();
        let pargs: Vec<VReg> = if has_ret { params[1..].to_vec() } else { params.clone() };
        for (k, ap) in plan.args.iter().enumerate() {
            if let ArgPlan::Parts(parts) = ap {
                let buf = self.cbuf[k];
                for pt in parts {
                    match pt.loc {
                        PLoc::Int(r) => self.a.store64(RBP, buf + pt.off as i32, int_args[r as usize]),
                        PLoc::Flt(x) => self.a.storef(true, RBP, buf + pt.off as i32, x),
                        PLoc::Stack(_) => {}
                    }
                }
            }
        }
        let mut moves = Vec::new();
        if let (RetPlan::Indirect, true) = (&plan.ret, has_ret) {
            self.a.store64(RBP, self.cret_ptr, int_args[0]);
            if let Some(d) = self.mloc(params[0]) {
                moves.push((d, MLoc::R(int_args[0])));
            }
        }
        for (k, ap) in plan.args.iter().enumerate() {
            let d = match self.mloc(pargs[k]) {
                Some(d) => d,
                None => continue,
            };
            match ap {
                ArgPlan::Scalar(PLoc::Int(r), _) | ArgPlan::ByRef(PLoc::Int(r)) => moves.push((d, MLoc::R(int_args[*r as usize]))),
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
                            let r = self.use_i(options, p, R10);
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

    /// A call under the signature's C-family convention.
    fn c_call(&mut self, options: &relax_session::Options, c: &Callee, sig: &CSig, args: &[VReg], rets: &[VReg]) {
        let target = sig.conv.target();
        let plan = cabi::plan(sig, target);
        let int_args = c_int_args(target);
        let saves = self.al.saves.get(&self.cur_pos).cloned().unwrap_or_default();
        for (v, slot) in &saves {
            self.save_restore(*v, *slot, true);
        }
        let has_ret = sig.ret.is_some();
        let cargs: Vec<VReg> = if has_ret { args[1..].to_vec() } else { args.to_vec() };
        let st = self.stage;
        if has_ret {
            let r = self.use_i(options, args[0], R10);
            self.a.store64(RBP, st, r);
        }
        if let Callee::CIndirect(v, _) = c {
            let r = self.use_i(options, *v, R11);
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
                        moves.push((MLoc::R(int_args[*r as usize]), s2));
                    }
                }
                ArgPlan::Scalar(PLoc::Flt(x), _) => {
                    if let Some(s2) = self.mloc(v) {
                        moves.push((MLoc::F(*x), s2));
                    }
                }
                ArgPlan::Scalar(PLoc::Stack(o), _) | ArgPlan::ByRef(PLoc::Stack(o)) => {
                    if self.f.vregs[v.0 as usize] == Cls::I {
                        let r = self.use_i(options, v, R10);
                        self.a.store64(RSP, *o as i32, r);
                    } else {
                        let x = self.use_f(v, XS);
                        self.a.storef(true, RSP, *o as i32, x);
                    }
                }
                ArgPlan::ByRef(PLoc::Flt(_)) => {}
                ArgPlan::Parts(parts) => {
                    let ra = self.use_i(options, v, R10);
                    for pt in parts {
                        let cd = st + 8 * cell;
                        self.copy_bytes(RBP, cd, ra, pt.off as i32, pt.size);
                        match pt.loc {
                            PLoc::Int(r) => moves.push((MLoc::R(int_args[r as usize]), MLoc::M(cd))),
                            PLoc::Flt(x) => moves.push((MLoc::F(x), MLoc::M(cd))),
                            PLoc::Stack(_) => {}
                        }
                        cell += 1;
                    }
                }
                ArgPlan::Stack(o, size) => {
                    let ra = self.use_i(options, v, R10);
                    self.copy_bytes(RSP, *o as i32, ra, 0, *size);
                }
            }
        }
        if let (RetPlan::Indirect, true) = (&plan.ret, has_ret) {
            moves.push((MLoc::R(int_args[0]), MLoc::M(st)));
        }
        for (d, s2) in regalloc::seq_moves(&moves, R10, XS) {
            self.mv(d, s2);
        }
        match c {
            Callee::CFn(id, _) => {
                self.mov_reloc(RAX, Reloc::Slot(*id));
                self.a.call_mem(RAX, 0);
            }
            Callee::CHost(sym, _) => {
                self.mov_reloc(R11, Reloc::Sym(*sym));
                if target == cabi::Target::SysV {
                    // SysV variadic calls: the number of vector registers used, in al
                    self.a.mov_ri(RAX, plan.n_flt as i64);
                }
                self.a.call_r(R11);
            }
            _ => {
                if target == cabi::Target::SysV {
                    self.a.mov_ri(RAX, plan.n_flt as i64);
                }
                self.a.call_r(R11);
            }
        }
        // a C-unwind call is invoked like a Relax one: its landing stub is keyed by this address
        self.last_call_ret = self.a.pos();
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
            self.save_restore(*v, *slot, false);
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

    fn atomic(&mut self, options: &relax_session::Options, i: &Inst) {
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
                let rb = self.use_i(options, *a, R11);
                let rd = self.def_i(*d, R10);
                self.a.load_ext(rd, rb, 0, n, sg);
                self.fin_i(*d, rd);
            }
            Inst::AtomicStore(m, o, a, v) => {
                let (n, _) = width(m);
                let rb = self.use_i(options, *a, R11);
                let rv = self.use_i(options, *v, R10);
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
                let ra = self.use_i(options, *a, R11);
                let rv = self.use_i(options, *v, R10);
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
                let ra = self.use_i(options, *a, R11);
                let re = self.use_i(options, *e, R10);
                let rn = self.use_i(options, *nw, RDX);
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

    // Snapshot operands before touching fixed registers. Preserve all CPUID clobbers,
    // including callee-saved RBX, so allocator values remain valid on SysV and Win64.
    // Frames/spills are RBP-relative; temporary pushes need no frame reservation.
    fn cpuid(&mut self, options: &relax_session::Options, ds: [VReg; 4], leaf: VReg, subleaf: VReg) {
        let a = self.use_i(options, leaf, R10);
        self.a.mov_rr(R10, a);
        let b = self.use_i(options, subleaf, R11);
        self.a.mov_rr(R11, b);
        for r in [RAX, RBX, RCX, RDX] { self.a.push(r); }
        self.a.mov_rr(RAX, R10);
        self.a.mov_rr(RCX, R11);
        self.a.b.extend_from_slice(&[0x0f, 0xa2]);
        // Save every result before restoring live registers or assigning destinations.
        for r in [RAX, RBX, RCX, RDX] { self.a.push(r); }
        for (i, r) in [RDX, RCX, RBX, RAX].iter().enumerate() {
            self.a.load64(*r, RSP, 32 + i as i32 * 8);
        }
        for d in ds.iter().rev() {
            let rd = self.def_i(*d, R10);
            self.a.pop(rd);
            self.fin_i(*d, rd);
        }
        for _ in 0..4 { self.a.pop(R11); }
    }

    fn xgetbv(&mut self, options: &relax_session::Options, d: VReg, index: VReg) {
        let a = self.use_i(options, index, R10);
        self.a.mov_rr(R10, a);
        self.a.push(RCX);
        self.a.push(RDX);
        self.a.mov_rr(RCX, R10);
        self.a.b.extend_from_slice(&[0x0f, 0x01, 0xd0]);
        // shl rdx, 32; or rax, rdx (both outputs are zero-extended by XGETBV).
        self.a.b.extend_from_slice(&[0x48, 0xc1, 0xe2, 0x20, 0x48, 0x09, 0xd0]);
        self.a.pop(RDX);
        self.a.pop(RCX);
        let rd = self.def_i(d, R10);
        self.a.mov_rr(rd, RAX);
        self.fin_i(d, rd);
    }

    /// MXCSR: stmxcsr / ldmxcsr take a memory operand, so the word goes through a stack slot
    /// pushed for the instruction (push / pop leave the flags alone; frames are rbp-based):
    /// read `push rax; stmxcsr [rsp]; pop rd; mov rd32, rd32`, write `push rx; ldmxcsr [rsp];
    /// pop rx`.
    fn fp_env(&mut self, options: &relax_session::Options, set: bool, v: VReg) {
        let push = |a: &mut Asm, r: u8| {
            if r >= 8 {
                a.byte(0x41);
            }
            a.byte(0x50 | (r & 7));
        };
        let pop = |a: &mut Asm, r: u8| {
            if r >= 8 {
                a.byte(0x41);
            }
            a.byte(0x58 | (r & 7));
        };
        if set {
            let rx = self.use_i(options, v, R10);
            push(&mut self.a, rx);
            // 0F AE /2: ldmxcsr m32
            self.a.b.extend_from_slice(&[0x0f, 0xae, 0x14, 0x24]);
            pop(&mut self.a, rx);
        } else {
            let rd = self.def_i(v, R10);
            push(&mut self.a, RAX);
            // 0F AE /3: stmxcsr m32
            self.a.b.extend_from_slice(&[0x0f, 0xae, 0x1c, 0x24]);
            pop(&mut self.a, rd);
            // mov rd32, rd32: zero-extends the 32-bit word
            self.a.rex(false, rd, 0, rd, false);
            self.a.byte(0x89);
            self.a.modrm_rr(rd, rd);
            self.fin_i(v, rd);
        }
    }

    /// r11 = this thread's thread-local block: `mov r11, fs:[key]` (the runtime's static-TLS
    /// block pointer), slow path `tls_slow` when it is still null (keeps every register but rax/r11).
    fn tls_block_r11(&mut self) {
        // mov rax, [rip + key]; mov r11, fs:[rax] (gs on Windows)
        self.mov_reloc(RAX, Reloc::TlsKey);
        self.a.b.extend_from_slice(&[TLS_SEG, 0x4c, 0x8b, 0x18]);
        self.a.test_rr(R11, R11);
        let p = self.a.jcc32(CC_NE);
        self.mov_reloc(RAX, Reloc::TlsSlow);
        self.a.call_r(RAX);
        let skip = self.a.pos();
        self.a.patch(p, skip);
    }

    /// d = this thread's thread-local block + off.
    fn tls_addr(&mut self, d: VReg, off: Sym) {
        self.tls_block_r11();
        // the variable's offset in the block (a symbol: it is assigned per run)
        let rd = self.def_i(d, R10);
        self.mov_reloc(rd, Reloc::Sym(off));
        self.a.alu_rr(0x01, rd, R11);
        self.fin_i(d, rd);
    }

    /// Prologue of a function that names domain statics: the running thread's domain base
    /// (slot 0 of its thread-local block) into the frame's domain slot, read once per call.
    fn dom_base_prologue(&mut self) {
        // the shared TLS routine (fast path inside), not the inline sequence: one call per entry
        self.mov_reloc(RAX, Reloc::TlsSlow);
        self.a.call_r(RAX);
        // mov r11, [r11]: the domain base
        self.a.load64(R11, R11, 0);
        self.a.store64(RBP, self.dom_slot, R11);
    }

    /// rd = the running thread's domain static block + `off` (a domain symbol: the offset in
    /// the block, assigned per run): the base the prologue stored + the offset.
    fn dom_addr(&mut self, rd: u8, off: Sym) {
        self.mov_reloc(rd, Reloc::Sym(off));
        self.a.add_load64(rd, RBP, self.dom_slot);
    }

    /// `mov rd, [rip + pool]`: a relocated value from the address pool after the code.
    fn mov_reloc(&mut self, rd: u8, r: Reloc) {
        self.a.rex(true, rd, 0, 0, false);
        self.a.byte(0x8b);
        self.a.byte(((rd & 7) << 3) | 5);
        let at = self.a.pos();
        self.a.u32(0);
        self.apool.push((at, r));
    }

    /// Win64 C body: rsi, rdi and xmm6-15 are callee-saved for its C caller but scratch in Relax's
    /// own convention (the body's code and its callees use them): stored after the prologue,
    /// reloaded before every return.
    fn win64_saves(&mut self, store: bool) {
        if self.win_saves == 0 {
            return;
        }
        let b = self.win_saves;
        if store {
            self.a.store64(RBP, b, RSI);
            self.a.store64(RBP, b + 8, RDI);
        } else {
            self.a.load64(RSI, RBP, b);
            self.a.load64(RDI, RBP, b + 8);
        }
        for k in 0..10u8 {
            // movups [rbp + d], xmm / movups xmm, [rbp + d]
            self.a.sse_rm(0, if store { 0x11 } else { 0x10 }, 6 + k, RBP, b + 16 + 16 * k as i32);
        }
    }

    fn epilogue(&mut self) {
        self.win64_saves(false);
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

    /// Blocks whose code is only their `jmp`: a block (not the entry) of moves the allocator
    /// placed in one location, no spill-window loads, ending in a jump elsewhere. `thru` maps
    /// every block to the end of such a chain (itself otherwise).
    fn thru_blocks(&mut self, reach: &[bool]) {
        let nb = self.f.blocks.len();
        let mut step: Vec<u32> = (0..nb as u32).collect();
        for bi in 1..nb {
            let b = &self.f.blocks[bi];
            let Term::Jump(t) = b.term else { continue };
            if !reach[bi] || t as usize == bi {
                continue;
            }
            let silent = b.insts.iter().enumerate().all(|(ii, i)| match *i {
                Inst::Mov(d, s2) => {
                    let (ld, ls) = (self.al.loc[d.0 as usize], self.al.loc[s2.0 as usize]);
                    ld == ls && matches!(ld, Loc::Reg(_) | Loc::Flt(_) | Loc::Stack(_)) && !self.win_start.contains_key(&self.al.inst_pos(bi, ii))
                }
                _ => false,
            });
            if silent {
                step[bi] = t;
            }
        }
        self.thru = (0..nb as u32)
            .map(|b| {
                let mut x = b;
                for _ in 0..8 {
                    if step[x as usize] == x {
                        break;
                    }
                    x = step[x as usize];
                }
                x
            })
            .collect();
    }

    fn thru(&self, b: u32) -> u32 {
        self.thru.get(b as usize).copied().unwrap_or(b)
    }

    fn jump_to(&mut self, target: u32, bi: usize) {
        let target = self.thru(target);
        if self.thru(self.next_block[bi]) != target {
            let p = self.a.jmp32();
            self.fixups.push((p, target));
        }
    }

    fn jcc_to(&mut self, cc: u8, target: u32) {
        let target = self.thru(target);
        let p = self.a.jcc32(cc);
        self.fixups.push((p, target));
    }

    /// cmp r, imm32 (sign-extended)
    /// cmp x, y (flags of x - y); a spilled operand is read by the compare itself
    /// (cmp r, [rbp - o] / cmp [rbp - o], r) instead of loaded into a scratch first
    fn cmp_vregs(&mut self, options: &relax_session::Options, x: VReg, y: VReg) {
        match (self.al.loc[x.0 as usize], self.al.loc[y.0 as usize]) {
            (_, Loc::Stack(o)) => {
                let rx = self.use_i(options, x, R10);
                let d = self.spill_disp(o);
                // REX.W 3B /r: cmp r64, r/m64
                self.a.rex(true, rx, 0, RBP, false);
                self.a.byte(0x3b);
                self.a.modrm_mem(rx, RBP, d);
            }
            (Loc::Stack(o), _) => {
                let ry = self.use_i(options, y, R11);
                let d = self.spill_disp(o);
                // REX.W 39 /r: cmp r/m64, r64
                self.a.rex(true, ry, 0, RBP, false);
                self.a.byte(0x39);
                self.a.modrm_mem(ry, RBP, d);
            }
            _ => {
                let rx = self.use_i(options, x, R10);
                let ry = self.use_i(options, y, R11);
                self.a.alu_rr(0x39, rx, ry);
            }
        }
    }

    fn cmp_imm(&mut self, r: u8, k: i64) {
        if k == 0 {
            // test r, r: the flags of cmp r, 0 (CF = OF = 0), one byte shorter
            self.a.alu_rr(0x85, r, r);
            return;
        }
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

    /// cmp <sz bytes> [rb + disp], k (k fits the width's immediate: load_cmp_fits)
    fn cmp_mem_imm(&mut self, rb: u8, disp: i32, sz: u8, k: i64) {
        let small = (-128..128).contains(&k);
        if sz == 2 {
            self.a.byte(0x66);
        }
        self.a.rex(sz == 8, 0, 0, rb, false);
        match sz {
            1 => self.a.byte(0x80),
            _ if small => self.a.byte(0x83),
            _ => self.a.byte(0x81),
        }
        self.a.modrm_mem(7, rb, disp);
        match sz {
            1 => self.a.byte(k as u8),
            _ if small => self.a.byte(k as i8 as u8),
            2 => {
                self.a.byte(k as u8);
                self.a.byte((k >> 8) as u8);
            }
            _ => self.a.u32(k as i32 as u32),
        }
    }

    fn term(&mut self, options: &relax_session::Options, t: &Term, bi: usize, fused: Option<Inst>) {
        match t {
            Term::Jump(target) => self.jump_to(*target, bi),
            Term::Branch(c, tt, ff) => {
                if let Some((x, min, targets, default)) = self.jt_plan.get(&(bi as u32)).cloned() {
                    // a chain of compares of x with constants (an integer match): one table dispatch
                    let r = self.use_i(options, x, R10);
                    self.a.mov_rr(RAX, r);
                    self.a.mov_ri(R11, min);
                    self.a.alu_rr(0x29, RAX, R11); // sub rax, r11
                    self.a.mov_ri(R11, targets.len() as i64 - 1);
                    self.a.alu_rr(0x39, RAX, R11); // cmp rax, r11
                    self.jcc_to(CC_A, default);
                    // lea r11, [rip + table]
                    self.a.byte(0x4c);
                    self.a.byte(0x8d);
                    self.a.byte(0x1d);
                    let at = self.a.pos();
                    self.a.u32(0);
                    // movsxd rax, dword [r11 + rax * 4]; add rax, r11; jmp rax
                    self.a.byte(0x49);
                    self.a.byte(0x63);
                    self.a.byte(0x04);
                    self.a.byte(0x83);
                    self.a.byte(0x4c);
                    self.a.byte(0x01);
                    self.a.byte(0xd8);
                    self.a.byte(0xff);
                    self.a.byte(0xe0);
                    let start = self.a.pos();
                    self.a.patch(at, start);
                    for t in targets {
                        let p = self.a.pos();
                        self.a.u32(0);
                        self.jt_fix.push((p, start, t));
                    }
                    let _ = (c, tt, ff, fused);
                    return;
                }
                let next = self.next_block[bi];
                // integer condition code for `c` true
                let icond: Option<u8> = match &fused {
                    Some(Inst::ICmp(cond, signed, _, x, y)) => {
                        self.cmp_vregs(options, *x, *y);
                        Some(icc(*cond, *signed))
                    }
                    Some(Inst::ICmpI(cond, signed, _, _, _)) if self.load_fuse.is_some() => {
                        let (base, off, sz, k) = self.load_fuse.take().unwrap();
                        let (rb, disp) = self.base_disp(options, base, R11, off);
                        self.cmp_mem_imm(rb, disp, sz, k);
                        Some(icc(*cond, *signed))
                    }
                    Some(Inst::ICmpI(cond, signed, _, x, k)) => match self.test_fuse.take() {
                        Some((src, mask)) => {
                            // test src, imm32 (REX.W F7 /0 id)
                            let rs = self.use_i(options, src, R10);
                            self.a.rex(true, 0, 0, rs, false);
                            self.a.byte(0xf7);
                            self.a.modrm_rr(0, rs);
                            self.a.u32(mask as i32 as u32);
                            Some(icc(*cond, *signed))
                        }
                        None => {
                            let rx = self.use_i(options, *x, R10);
                            // ZF of the ALU instruction that just wrote x is x == 0
                            if !(*k == 0 && matches!(cond, Cond::Eq | Cond::Ne) && self.last_alu == Some((rx, self.a.pos()))) {
                                self.cmp_imm(rx, *k);
                            }
                            Some(icc(*cond, *signed))
                        }
                    },
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
                    _ if self.load_fuse.is_some() => {
                        let (base, off, sz, k) = self.load_fuse.take().unwrap();
                        let (rb, disp) = self.base_disp(options, base, R11, off);
                        self.cmp_mem_imm(rb, disp, sz, k);
                        Some(CC_NE)
                    }
                    _ => {
                        let r = self.use_i(options, *c, R10);
                        self.a.test_rr(r, r);
                        Some(CC_NE)
                    }
                };
                let cc = icond.unwrap();
                if self.thru(*tt) == self.thru(next) {
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
            Term::Invoke(normal, unwind) => {
                // the call was the block's last instruction: unwinding returns to a landing
                // stub (see blocks_and_pool) that reloads what the normal path reloads
                let saves = self.al.saves.get(&self.cur_pos).cloned().unwrap_or_default();
                self.pads.push((self.last_call_ret, saves, *unwind));
                self.jump_to(*normal, bi);
            }
            Term::Resume => {
                self.mov_reloc(R11, Reloc::Resume);
                self.a.call_r(R11);
                self.a.ud2();
            }
        }
    }

    fn inst(&mut self, options: &relax_session::Options, i: &Inst) {
        match i {
            // recomputed at each use (Alloc::remat)
            Inst::Iconst(d, _) | Inst::SlotAddr(d, _) | Inst::Addr(d, _) | Inst::FnAddr(d, _) | Inst::Mov(d, _) if self.al.remat[d.0 as usize].is_some() => {}
            Inst::Sel(d, c, a, b) => self.sel(options, *d, *c, *a, *b),
            Inst::VBin(..) | Inst::VUn(..) | Inst::VSel(..) | Inst::VFma(..) | Inst::VSplat(..) | Inst::VExtract(..) | Inst::VInsert(..) | Inst::VLoad(..) | Inst::VStore(..) | Inst::VAny(..) | Inst::VGather(..) | Inst::VConst(..) | Inst::VShift(..) => self.vinst(options, i),
            Inst::VX(op, d, a, b) => self.vx(options, *op, *d, *a, *b),
            Inst::VX3(op, d, a, b, c) => self.vx3(*op, *d, *a, *b, *c),
            Inst::FMulAdd(np, na, w, d, a, b, c) => self.fma(*np, *na, *w, *d, *a, *b, *c),
            Inst::AtomicLoad(..) | Inst::AtomicStore(..) | Inst::AtomicRmw(..) | Inst::AtomicCas(..) | Inst::Fence(..) => self.atomic(options, i),
            Inst::TlsAddr(d, off) => self.tls_addr(*d, *off),
            Inst::FpEnv(set, v) => self.fp_env(options, *set, *v),
            Inst::Cpuid(ds, a, b) => self.cpuid(options, *ds, *a, *b),
            Inst::Xgetbv(d, a) => self.xgetbv(options, *d, *a),
            Inst::IBinI(op, it, d, a, k) => self.ibin_imm(options, *op, *it, *d, *a, *k),
            Inst::ICmpI(c, signed, d, a, k) => {
                let ra = self.use_i(options, *a, R10);
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
            // pieces of one split value sharing a slot
            Inst::Mov(d, s) if matches!(self.al.loc[d.0 as usize], Loc::Stack(_)) && self.al.loc[d.0 as usize] == self.al.loc[s.0 as usize] => {}
            Inst::Mov(d, s) => match self.f.vregs[d.0 as usize] {
                Cls::I => match (self.al.loc[d.0 as usize], self.al.loc[s.0 as usize]) {
                    // straight between a register and a spill slot
                    (Loc::Stack(o), Loc::Reg(r)) => {
                        let disp = self.spill_disp(o);
                        self.a.store64(RBP, disp, r);
                    }
                    (Loc::Reg(r), Loc::Stack(o)) => {
                        let disp = self.spill_disp(o);
                        self.load_spill(r, disp);
                    }
                    _ => {
                        let rs = self.use_i(options, *s, R10);
                        let rd = self.def_i(*d, R10);
                        self.a.mov_rr(rd, rs);
                        self.fin_i(*d, rd);
                    }
                },
                c => {
                    let xs = self.use_f(*s, XS);
                    let xd = self.def_f(*d, XS);
                    self.a.movf_rr(c == Cls::F64, xd, xs);
                    self.fin_f(*d, xd);
                }
            },
            Inst::IBin(op, it, d, a, b) => self.ibin(options, *op, *it, *d, *a, *b),
            Inst::IBit(o, d, a) => {
                let ra = self.use_i(options, *a, R10);
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
                let ra = self.use_i(options, *a, R10);
                let rd = self.def_i(*d, R10);
                self.a.mov_rr(rd, ra);
                let ext = if matches!(i, Inst::INeg(..)) { 3 } else { 2 };
                self.a.grp3(ext, rd);
                self.a.extend(rd, it.bits, it.signed);
                self.fin_i(*d, rd);
            }
            Inst::ICmp(c, signed, d, a, b) => {
                self.cmp_vregs(options, *a, *b);
                let rd = self.def_i(*d, R10);
                self.a.setcc(icc(*c, *signed), rd);
                self.a.movzx8(rd, rd);
                self.fin_i(*d, rd);
            }
            // min/max with a constant that is not NaN: x86 minsd/maxsd return their second operand
            // when either is NaN, which is then the constant -- Rust's answer -- in one instruction
            Inst::FBin(op @ (FOp::Min | FOp::Max), f64_, d, a, b) if self.mem_const.is_none() && (self.non_nan_const(*a, *f64_) || self.non_nan_const(*b, *f64_)) => {
                let (x, c) = if self.non_nan_const(*b, *f64_) { (*a, *b) } else { (*b, *a) };
                let xo = self.use_f(x, XS);
                let xc = self.use_f(c, XS2);
                let xd = self.def_f(*d, XS);
                let pfx = if *f64_ { 0xf2 } else { 0xf3 };
                let opc = if *op == FOp::Min { 0x5d } else { 0x5f };
                if self.avx {
                    self.a.vex_rr(if *f64_ { 3 } else { 2 }, 1, false, opc, xd, xo, xc);
                } else if xd == xc && xd != xo {
                    self.a.movf_rr(true, XS, xo);
                    self.a.sse_rr(pfx, opc, XS, xc, false);
                    self.a.movf_rr(true, xd, XS);
                } else {
                    self.a.movf_rr(true, xd, xo);
                    self.a.sse_rr(pfx, opc, xd, xc, false);
                }
                self.fin_f(*d, xd);
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
            Inst::Conv(c, d, s) => self.conv(options, *c, *d, *s),
            Inst::LoadX(m, d, base, idx, sh, off) => {
                let ri = self.use_i(options, *idx, R10);
                let ri = if ri == R10 {
                    self.a.mov_rr(RAX, R10);
                    RAX
                } else {
                    ri
                };
                let (rb, off) = self.base_disp(options, *base, R11, *off);
                match m {
                    Mem::Int(n, signed) => {
                        let rd = self.def_i(*d, R10);
                        self.a.sib = Some((ri, *sh));
                        self.a.load_ext(rd, rb, off, *n, *signed);
                        self.fin_i(*d, rd);
                    }
                    Mem::F32 | Mem::F64 => {
                        let xd = self.def_f(*d, XS);
                        self.a.sib = Some((ri, *sh));
                        self.a.loadf(*m == Mem::F64, xd, rb, off);
                        self.fin_f(*d, xd);
                    }
                }
            }
            Inst::StoreX(m, base, idx, sh, off, src) => {
                let ri = self.use_i(options, *idx, R10);
                let ri = if ri == R10 {
                    self.a.mov_rr(RAX, R10);
                    RAX
                } else {
                    ri
                };
                let (rb, off) = self.base_disp(options, *base, R11, *off);
                match m {
                    Mem::Int(n, _) => {
                        let rs = self.use_i(options, *src, R10);
                        self.a.sib = Some((ri, *sh));
                        self.a.store_n(rb, off, rs, *n);
                    }
                    Mem::F32 | Mem::F64 => {
                        let xs = self.use_f(*src, XS);
                        self.a.sib = Some((ri, *sh));
                        self.a.storef(*m == Mem::F64, rb, off, xs);
                    }
                }
            }
            Inst::Load(m, d, base, off) => {
                let (rb, off) = self.base_disp(options, *base, R11, *off);
                match m {
                    Mem::Int(n, signed) => {
                        let rd = self.def_i(*d, R10);
                        self.a.load_ext(rd, rb, off, *n, *signed);
                        self.fin_i(*d, rd);
                    }
                    Mem::F32 | Mem::F64 => {
                        let xd = self.def_f(*d, XS);
                        self.a.loadf(*m == Mem::F64, xd, rb, off);
                        self.fin_f(*d, xd);
                    }
                }
            }
            Inst::Store(m, base, off, src) => {
                let (rb, off) = self.base_disp(options, *base, R11, *off);
                match m {
                    Mem::Int(n, _) => {
                        let rs = self.use_i(options, *src, R10);
                        self.a.store_n(rb, off, rs, *n);
                    }
                    Mem::F32 | Mem::F64 => {
                        let xs = self.use_f(*src, XS);
                        self.a.storef(*m == Mem::F64, rb, off, xs);
                    }
                }
            }
            Inst::SlotAddr(d, s) => {
                let rd = self.def_i(*d, R10);
                let off = self.slot_off[*s as usize];
                self.a.lea(rd, RBP, -(self.saved + off));
                self.fin_i(*d, rd);
            }
            Inst::Addr(d, sym) => {
                let rd = self.def_i(*d, R10);
                if SymClass::of(self.sym_class, sym.id) == SymClass::Domain {
                    self.dom_addr(rd, *sym);
                } else {
                    self.mov_reloc(rd, Reloc::Sym(*sym));
                }
                self.fin_i(*d, rd);
            }
            Inst::FnAddr(d, id) => {
                let rd = self.def_i(*d, R10);
                self.mov_reloc(rd, Reloc::Thunk(*id));
                self.fin_i(*d, rd);
            }
            Inst::Call(c, args, rets) => self.call(options, c, args, rets),
            Inst::Copy(dst, src, size) => {
                let rd = self.use_i(options, *dst, R10);
                let rs = self.use_i(options, *src, R11);
                if *size >= 32 && *size <= 512 {
                    // 16-byte movups through the float scratch (rep movsb's start-up costs
                    // ~30 cycles: Option<T> moves of a few hundred bytes in hot paths)
                    let size = *size as i32;
                    let mut o = 0i32;
                    while o + 16 <= size {
                        self.a.sse_rm(0, 0x10, XS, rs, o);
                        self.a.sse_rm(0, 0x11, XS, rd, o);
                        o += 16;
                    }
                    if o < size {
                        // the tail as one overlapping 16-byte move
                        self.a.sse_rm(0, 0x10, XS, rs, size - 16);
                        self.a.sse_rm(0, 0x11, XS, rd, size - 16);
                    }
                    return;
                }
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


    fn vx3(&mut self, op: VX, d: VReg, a: VReg, b: VReg, c: VReg) {
        assert_eq!(op, VX::X86Sha256Rnds2);
        // xmm0 is the implicit WK input; save it before writing an aliased d.
        // xmm0/xmm1 are never allocated. Use xmm1 when d aliases the source b.
        let xc = self.use_f(c, 0);
        self.a.movf_rr(true, 0, xc);
        let xa = self.use_f(a, XS);
        let xb = self.use_f(b, XS2);
        let xd = self.def_f(d, XS);
        let tmp = if xd == xb && xd != xa { 1 } else { xd };
        self.a.movf_rr(true, tmp, xa);
        self.a.sse_ext(0, 0x38, 0xcb, tmp, xb);
        self.a.movf_rr(true, xd, tmp);
        self.fin_f(d, xd);
    }

    /// Lane-typed vector ops (core::arch intrinsics): SSE2 .. SSE4.1, xmm0/xmm1 as scratch.
    fn vx(&mut self, options: &relax_session::Options, op: VX, d: VReg, a: VReg, b: VReg) {
        // SHA-NI has legacy SSE encodings only, with NO mandatory 66 prefix.
        // Do not route these through v3: AVX hosts must still get legacy SHA.
        let sha = match op {
            VX::X86Sha1Rnds4(k) => Some((0x3a, 0xcc, Some(k))),
            VX::X86Sha1NextE => Some((0x38, 0xc8, None)),
            VX::X86Sha1Msg1 => Some((0x38, 0xc9, None)),
            VX::X86Sha1Msg2 => Some((0x38, 0xca, None)),
            VX::X86Sha256Msg1 => Some((0x38, 0xcc, None)),
            VX::X86Sha256Msg2 => Some((0x38, 0xcd, None)),
            _ => None,
        };
        if let Some((map, opcode, imm)) = sha {
            self.v2(d, a, b, &move |m, x, y| {
                m.sse_ext(0, map, opcode, x, y);
                if let Some(k) = imm {
                    m.byte(k);
                }
            });
            return;
        }
        // binary op through the SSE two-address / VEX three-address form: (map, opcode)
        let bin = |op: VX| -> Option<(u8, u8)> {
            Some(match op {
                VX::Add(w) => (1, [0xfc, 0xfd, 0xfe, 0xd4][w as usize]),
                VX::Sub(w) => (1, [0xf8, 0xf9, 0xfa, 0xfb][w as usize]),
                VX::MulLo(VW::H) => (1, 0xd5),
                VX::MulLo(VW::S) => (2, 0x40),
                VX::MinS(VW::B) => (2, 0x38),
                VX::MinS(VW::H) => (1, 0xea),
                VX::MinS(VW::S) => (2, 0x39),
                VX::MaxS(VW::B) => (2, 0x3c),
                VX::MaxS(VW::H) => (1, 0xee),
                VX::MaxS(VW::S) => (2, 0x3d),
                VX::MinU(VW::B) => (1, 0xda),
                VX::MinU(VW::H) => (2, 0x3a),
                VX::MinU(VW::S) => (2, 0x3b),
                VX::MaxU(VW::B) => (1, 0xde),
                VX::MaxU(VW::H) => (2, 0x3e),
                VX::MaxU(VW::S) => (2, 0x3f),
                VX::CmpEq(w) => match w {
                    VW::D => (2, 0x29),
                    _ => (1, [0x74, 0x75, 0x76, 0][w as usize]),
                },
                VX::CmpGtS(w) => match w {
                    VW::D => (2, 0x37),
                    _ => (1, [0x64, 0x65, 0x66, 0][w as usize]),
                },
                VX::AddSatU(VW::B) => (1, 0xdc),
                VX::AddSatU(VW::H) => (1, 0xdd),
                VX::SubSatU(VW::B) => (1, 0xd8),
                VX::SubSatU(VW::H) => (1, 0xd9),
                VX::AddSatS(VW::B) => (1, 0xec),
                VX::AddSatS(VW::H) => (1, 0xed),
                VX::SubSatS(VW::B) => (1, 0xe8),
                VX::SubSatS(VW::H) => (1, 0xe9),
                VX::AvgU(VW::B) => (1, 0xe0),
                VX::AvgU(VW::H) => (1, 0xe3),
                VX::And => (1, 0xdb),
                VX::Or => (1, 0xeb),
                VX::Xor => (1, 0xef),
                VX::ZipLo(w) => (1, [0x60, 0x61, 0x62, 0x6c][w as usize]),
                VX::ZipHi(w) => (1, [0x68, 0x69, 0x6a, 0x6d][w as usize]),
                VX::MaddH => (1, 0xf5),
                VX::MaddUbs => (2, 0x04),
                VX::SadU8 => (1, 0xf6),
                VX::PackUsH => (1, 0x67),
                VX::PackSsH => (1, 0x63),
                VX::PackSsS => (1, 0x6b),
                VX::PackUsS => (2, 0x2b),
                VX::ByteShuffle => (2, 0x00),
                _ => return None,
            })
        };
        if let Some((map, opc)) = bin(op) {
            self.v3(d, a, b, 1, map, opc, None);
            return;
        }
        match op {
            VX::AndNot => {
                // pandn x, y = !x & y: d = !b & a
                let xa = self.use_f(a, XS);
                let xb = self.use_f(b, XS2);
                self.a.movf_rr(true, 0, xb);
                self.a.sse_rr(0x66, 0xdf, 0, xa, false);
                let xd = self.def_f(d, XS);
                self.a.movf_rr(true, xd, 0);
                self.fin_f(d, xd);
            }
            VX::Abs(w) => {
                let xa = self.use_f(a, XS);
                let xd = self.def_f(d, XS);
                self.a.sse_ext(0x66, 0x38, [0x1c, 0x1d, 0x1e, 0x1e][w as usize], xd, xa);
                self.fin_f(d, xd);
            }
            VX::ShlI(w, k) | VX::ShrUI(w, k) | VX::ShrSI(w, k) => {
                let xa = self.use_f(a, XS);
                let xd = self.def_f(d, XS);
                self.a.movf_rr(true, xd, xa);
                let opc = [0x71, 0x71, 0x72, 0x73][w as usize];
                let ext = match op {
                    VX::ShlI(..) => 6,
                    VX::ShrUI(..) => 2,
                    _ => 4,
                };
                if w == VW::B {
                    // no byte shifts: shift 16-bit lanes and mask the bits that crossed
                    let k = k.min(if matches!(op, VX::ShrSI(..)) { 7 } else { 8 });
                    self.a.sse_grp_imm(0x71, ext, xd, k);
                    let m: u8 = match op {
                        VX::ShlI(..) => (0xffu32 << k) as u8,
                        _ => (0xffu32 >> k) as u8,
                    };
                    self.splat_byte(1, m);
                    self.a.sse_rr(0x66, 0xdb, xd, 1, false);
                    if let VX::ShrSI(..) = op {
                        // sign: (x ^ s) - s with s = 0x80 >> k
                        let sb = (0x80u32 >> k) as u8;
                        self.splat_byte(1, sb);
                        self.a.sse_rr(0x66, 0xef, xd, 1, false);
                        self.a.sse_rr(0x66, 0xf8, xd, 1, false);
                    }
                } else if matches!(op, VX::ShrSI(VW::D, _)) {
                    // SSE2 qword arithmetic shift: logical shift plus sign fill.
                    // Duplicate each qword's high dword, then broadcast its sign.
                    self.a.sse_rr(0x66, 0x70, 1, xd, false);
                    self.a.byte(0xf5);
                    self.a.sse_grp_imm(0x72, 4, 1, 31);
                    let k = k.min(63);
                    self.a.sse_grp_imm(0x73, 2, xd, k);
                    self.a.sse_grp_imm(0x73, 6, 1, 64 - k);
                    self.a.sse_rr(0x66, 0xeb, xd, 1, false);
                } else {
                    self.a.sse_grp_imm(opc, ext, xd, k);
                }
                self.fin_f(d, xd);
            }
            VX::ShufS(k) => {
                let xa = self.use_f(a, XS);
                let xd = self.def_f(d, XS);
                self.a.sse_rr(0x66, 0x70, xd, xa, false);
                self.a.byte(k);
                self.fin_f(d, xd);
            }
            VX::BytesShrI(k) | VX::BytesShlI(k) => {
                let xa = self.use_f(a, XS);
                let xd = self.def_f(d, XS);
                self.a.movf_rr(true, xd, xa);
                self.a.sse_grp_imm(0x73, if let VX::BytesShrI(_) = op { 3 } else { 7 }, xd, k);
                self.fin_f(d, xd);
            }
            VX::WidenLoU(w) | VX::WidenHiU(w) | VX::WidenLoS(w) | VX::WidenHiS(w) => {
                let xa = self.use_f(a, XS);
                let xd = self.def_f(d, XS);
                let mut src = xa;
                if matches!(op, VX::WidenHiU(_) | VX::WidenHiS(_)) {
                    self.a.movf_rr(true, 0, xa);
                    self.a.sse_grp_imm(0x73, 3, 0, 8);
                    src = 0;
                }
                let signed = matches!(op, VX::WidenLoS(_) | VX::WidenHiS(_));
                let opc = [0x20u8, 0x23, 0x25, 0x25][w as usize] + if signed { 0 } else { 0x10 };
                self.a.sse_ext(0x66, 0x38, opc, xd, src);
                self.fin_f(d, xd);
            }
            VX::PairAddLongU(w) => {
                let xa = self.use_f(a, XS);
                let xd = self.def_f(d, XS);
                match w {
                    VW::B => {
                        // pmaddubsw(a, splat(1)): u8 pairs summed into u16 (no saturation: <= 510)
                        self.splat_byte(1, 1);
                        self.a.movf_rr(true, 0, xa);
                        self.a.sse_ext(0x66, 0x38, 0x04, 0, 1);
                        self.a.movf_rr(true, xd, 0);
                    }
                    VW::H => {
                        // (a & 0xffff) + (a >> 16) per 32-bit lane
                        self.a.movf_rr(true, 0, xa);
                        self.a.sse_grp_imm(0x72, 2, 0, 16);
                        self.a.movf_rr(true, 1, xa);
                        self.a.sse_grp_imm(0x72, 6, 1, 16);
                        self.a.sse_grp_imm(0x72, 2, 1, 16);
                        self.a.sse_rr(0x66, 0xfe, 0, 1, false);
                        self.a.movf_rr(true, xd, 0);
                    }
                    _ => {
                        // (a & 0xffffffff) + (a >> 32) per 64-bit lane
                        self.a.movf_rr(true, 0, xa);
                        self.a.sse_grp_imm(0x73, 2, 0, 32);
                        self.a.movf_rr(true, 1, xa);
                        self.a.sse_grp_imm(0x73, 6, 1, 32);
                        self.a.sse_grp_imm(0x73, 2, 1, 32);
                        self.a.sse_rr(0x66, 0xd4, 0, 1, false);
                        self.a.movf_rr(true, xd, 0);
                    }
                }
                self.fin_f(d, xd);
            }
            VX::Splat(w) => {
                let ra = self.use_i(options, a, R10);
                let xd = self.def_f(d, XS);
                self.a.sse_w(0, 0x6e, xd, ra, w == VW::D); // movd / movq xmm, r
                match w {
                    VW::B => {
                        self.a.sse_rr(0x66, 0x60, xd, xd, false); // punpcklbw
                        self.a.sse_rr(0xf2, 0x70, xd, xd, false); // pshuflw 0
                        self.a.byte(0);
                        self.a.sse_rr(0x66, 0x70, xd, xd, false); // pshufd 0
                        self.a.byte(0);
                    }
                    VW::H => {
                        self.a.sse_rr(0xf2, 0x70, xd, xd, false);
                        self.a.byte(0);
                        self.a.sse_rr(0x66, 0x70, xd, xd, false);
                        self.a.byte(0);
                    }
                    VW::S => {
                        self.a.sse_rr(0x66, 0x70, xd, xd, false);
                        self.a.byte(0);
                    }
                    VW::D => self.a.sse_rr(0x66, 0x6c, xd, xd, false), // punpcklqdq
                }
                self.fin_f(d, xd);
            }
            VX::FromLo64 => {
                let ra = self.use_i(options, a, R10);
                let xd = self.def_f(d, XS);
                self.a.sse_w(0, 0x6e, xd, ra, true);
                self.fin_f(d, xd);
            }
            VX::Extract(w, k) => {
                let xa = self.use_f(a, XS);
                let rd = self.def_i(d, R10);
                match w {
                    VW::S | VW::D if k == 0 => {
                        self.a.sse_w(0, 0x7e, xa, rd, w == VW::D); // movd / movq r, xmm
                    }
                    VW::H => {
                        self.a.sse_w(0, 0xc5, rd, xa, false); // pextrw r32, xmm, k
                        self.a.byte(k);
                    }
                    _ => {
                        // pextrb / pextrd / pextrq r, xmm, k (reg = xmm, rm = r)
                        self.a.sse_w(0x3a, if w == VW::B { 0x14 } else { 0x16 }, xa, rd, w == VW::D);
                        self.a.byte(k);
                    }
                }
                self.fin_i(d, rd);
            }
            VX::Insert(w, k) => {
                let xa = self.use_f(a, XS);
                let rb = self.use_i(options, b, R10);
                let xd = self.def_f(d, XS);
                self.a.movf_rr(true, xd, xa);
                match w {
                    VW::H => self.a.sse_w(0, 0xc4, xd, rb, false),
                    VW::B => self.a.sse_w(0x3a, 0x20, xd, rb, false),
                    _ => self.a.sse_w(0x3a, 0x22, xd, rb, w == VW::D),
                }
                self.a.byte(k);
                self.fin_f(d, xd);
            }
            VX::MoveMaskB => {
                let xa = self.use_f(a, XS);
                let rd = self.def_i(d, R10);
                self.a.sse_w(0, 0xd7, rd, xa, false);
                self.fin_i(d, rd);
            }
            VX::AddAcross(w) | VX::MaxAcrossU(w) => {
                let xa = self.use_f(a, XS);
                let rd = self.def_i(d, R10);
                self.a.movf_rr(true, 0, xa);
                let max = matches!(op, VX::MaxAcrossU(_));
                // fold halves: 64-bit, 32-bit, 16-bit, 8-bit steps down to the lane width
                let steps: &[(u8, bool)] = match w {
                    VW::D => &[(8, true)],
                    VW::S => &[(8, true), (4, true)],
                    VW::H => &[(8, true), (4, true), (2, true)],
                    VW::B => &[(8, true), (4, true), (2, true), (1, true)],
                };
                for &(sh, _) in steps {
                    self.a.movf_rr(true, 1, 0);
                    self.a.sse_grp_imm(0x73, 3, 1, sh);
                    let opc: (u8, u8) = if max {
                        match w {
                            VW::B => (1, 0xde),
                            VW::H => (2, 0x3e),
                            _ => (2, 0x3f),
                        }
                    } else {
                        (1, [0xfc, 0xfd, 0xfe, 0xd4][w as usize])
                    };
                    if opc.0 == 1 {
                        self.a.sse_rr(0x66, opc.1, 0, 1, false);
                    } else {
                        self.a.sse_ext(0x66, 0x38, opc.1, 0, 1);
                    }
                }
                self.a.sse_w(0, 0x7e, 0, rd, w == VW::D); // movd / movq r, xmm0
                match w {
                    VW::B => self.a.extend(rd, 8, false),
                    VW::H => self.a.extend(rd, 16, false),
                    _ => {}
                }
                self.fin_i(d, rd);
            }
            _ => unreachable!("vx {:?}", op),
        }
    }

    /// xmm `x` = the byte `b` in every lane (via a general register)
    fn splat_byte(&mut self, x: u8, b: u8) {
        self.a.mov_ri(R11, (b as u64 * 0x0101_0101_0101_0101) as i64);
        self.a.sse_w(0, 0x6e, x, R11, true);
        self.a.sse_rr(0x66, 0x6c, x, x, false);
    }

    /// NEON float semantics on SSE. Only the four unallocated scratch XMMs are
    /// written until the final result; spilled inputs can therefore be reloaded.
    fn neon_float(&mut self, d: VReg, a: VReg, b: VReg, opc: u8) {
        let xa = self.use_f(a, XS);
        let xb = self.use_f(b, XS2);
        self.a.movf_rr(true, 1, xa);
        self.a.sse_rr(0, opc, 1, if opc == 0x51 { xa } else { xb }, false);
        if matches!(opc, 0x5d | 0x5f) {
            self.a.movf_rr(true, 0, xa);
            self.a.sse_rr(0, 0xc2, 0, xb, false);
            self.a.byte(0); // equal, including opposite signed zeros
            self.a.movf_rr(true, XS, xa);
            self.a.sse_rr(0, if opc == 0x5d { 0x56 } else { 0x54 }, XS, xb, false);
            self.a.sse_ext(0x66, 0x38, 0x10, 1, XS); // pblendvb, mask xmm0
        }
        self.a.movf_rr(true, 0, 1);
        self.a.sse_rr(0, 0xc2, 0, 0, false);
        self.a.byte(3); // unordered result -> positive default NaN
        self.vsplat_const(XS, 0x7fc00000);
        self.a.sse_ext(0x66, 0x38, 0x10, 1, XS);
        // Last selection wins: first operand NaN, then signaling NaNs in order.
        for (input, signaling) in [(b, false), (a, false), (b, true), (a, true)] {
            let x = self.use_f(input, XS);
            self.a.movf_rr(true, XS, x);
            self.a.movf_rr(true, 0, XS);
            self.a.sse_rr(0, 0xc2, 0, 0, false);
            self.a.byte(3);
            if signaling {
                self.a.movf_rr(true, XS2, XS);
                self.a.sse_grp_imm(0x72, 6, XS2, 9); // quiet bit -> sign bit
                self.a.sse_grp_imm(0x72, 4, XS2, 31);
                self.a.sse_rr(0x66, 0xdf, XS2, 0, false); // !quiet & nan
                self.a.movf_rr(true, 0, XS2);
            }
            self.vsplat_const(XS2, 0x00400000);
            self.a.sse_rr(0, 0x56, XS, XS2, false);
            self.a.sse_ext(0x66, 0x38, 0x10, 1, XS);
        }
        let xd = self.def_f(d, XS);
        self.a.movf_rr(true, xd, 1);
        self.fin_f(d, xd);
    }

    /// 128-bit vector instructions (SSE4.1; AVX2 for per-lane shifts, FMA3 for VFma).
    fn vinst(&mut self, options: &relax_session::Options, i: &Inst) {
        match *i {
            Inst::VBin(op, d, a, b) => {
                match op {
                    VOp::FAdd | VOp::FAddIntel => self.v3(d, a, b, 0, 1, 0x58, None),
                    VOp::FSub | VOp::FSubIntel => self.v3(d, a, b, 0, 1, 0x5c, None),
                    VOp::FMul | VOp::FMulIntel => self.v3(d, a, b, 0, 1, 0x59, None),
                    VOp::FDiv | VOp::FDivIntel => self.v3(d, a, b, 0, 1, 0x5e, None),
                    VOp::FAddNeon => self.neon_float(d, a, b, 0x58),
                    VOp::FSubNeon => self.neon_float(d, a, b, 0x5c),
                    VOp::FMulNeon => self.neon_float(d, a, b, 0x59),
                    VOp::FDivNeon => self.neon_float(d, a, b, 0x5e),
                    VOp::FMinNeon => self.neon_float(d, a, b, 0x5d),
                    VOp::FMaxNeon => self.neon_float(d, a, b, 0x5f),
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
                    VUn::FSqrtNeon => self.neon_float(d, a, a, 0x51),
                    VUn::FNeg | VUn::FAbs => {
                        self.vsplat_const(1, if op == VUn::FNeg { 0x8000_0000 } else { 0x7fff_ffff });
                        self.a.sse_rr(0, if op == VUn::FNeg { 0x57 } else { 0x54 }, 1, xa, false);
                        let xd = self.def_f(d, XS);
                        self.a.movf_rr(true, xd, 1);
                        self.fin_f(d, xd);
                    }
                    VUn::FSqrt | VUn::FSqrtIntel | VUn::IToF => {
                        let xd = self.def_f(d, XS);
                        self.a.sse_rr(0, if matches!(op, VUn::FSqrt | VUn::FSqrtIntel) { 0x51 } else { 0x5b }, xd, xa, false);
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
                if self.avx && self.byte_masks[m.0 as usize] {
                    let xm = self.use_f(m, 0);
                    let xa = self.use_f(a, XS);
                    let xb = self.use_f(b, XS2);
                    let xd = self.def_f(d, XS);
                    self.a.vex_rr(1, 3, false, 0x4c, xd, xb, xa); // vpblendvb d, b, a, mask
                    self.a.byte(xm << 4);
                    self.fin_f(d, xd);
                    return;
                }
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
                    let rs = self.use_i(options, s, R10);
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
                    let rs = self.use_i(options, s, R10);
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
                let rb = self.use_i(options, base, R11);
                let xd = self.def_f(d, XS);
                self.a.sse_rm(0, 0x10, xd, rb, off);
                self.fin_f(d, xd);
            }
            Inst::VStore(base, off, v) => {
                let rb = self.use_i(options, base, R11);
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
            Inst::VGather(d, base, idx) if !use_gather(options) => {
                // per lane: pextrd / mov / pinsrd (faster than vpgatherdd for four lanes on the
                // CPUs measured; MAKEPAD_RUN_GATHER=1 selects the gather)
                let rb = self.use_i(options, base, R11);
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
                let rb = self.use_i(options, base, R11);
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
    fn cmov_sel(&mut self, options: &relax_session::Options, d: VReg, a: VReg, b: VReg, cc: u8) {
        let ra = self.use_i(options, a, R10);
        let rb = self.use_i(options, b, R11);
        let rd = self.def_i(d, R10);
        // cmovcc dst, src (REX.W 0F 40+cc /r); mov does not touch the flags
        let cmov = |g: &mut Self, cc: u8, dst: u8, src: u8| {
            g.a.rex(true, dst, 0, src, false);
            g.a.byte(0x0f);
            g.a.byte(0x40 | cc);
            g.a.modrm_rr(dst, src);
        };
        if rd == rb {
            // d already holds b
            cmov(self, cc, rd, ra);
        } else if rd == ra {
            // d holds a: take b on the inverse condition
            cmov(self, cc ^ 1, rd, rb);
        } else {
            self.a.mov_rr(rd, rb);
            cmov(self, cc, rd, ra);
        }
        self.fin_i(d, rd);
    }

    /// Select after setting flags. Loads/movaps preserve them; handle destination
    /// aliasing either input before overwriting anything (including spilled inputs).
    fn float_sel(&mut self, d: VReg, a: VReg, b: VReg, cc: u8, parity: Option<bool>) {
        let xa = self.use_f(a, XS);
        let xb = self.use_f(b, XS2);
        let xd = self.def_f(d, XS);
        // Normally initialize b and overwrite with a when true. If d aliases a,
        // initialize a instead and overwrite with b when false.
        let inverse = xd == xa;
        let (initial, other, take) = if inverse { (xa, xb, cc ^ 1) } else { (xb, xa, cc) };
        self.a.movf_rr(true, xd, initial);
        let mut skips = Vec::new();
        let mut unordered_take = None;
        if let Some(true_on_unordered) = parity {
            if true_on_unordered != inverse {
                unordered_take = Some(self.a.jcc32(CC_P));
            } else {
                skips.push(self.a.jcc32(CC_P));
            }
        }
        skips.push(self.a.jcc32(take ^ 1));
        let take_pos = self.a.pos();
        if let Some(p) = unordered_take {
            self.a.patch(p, take_pos);
        }
        self.a.movf_rr(true, xd, other);
        let end = self.a.pos();
        for p in skips {
            self.a.patch(p, end);
        }
        self.fin_f(d, xd);
    }

    /// d = c != 0 ? a : b. Integers use cmov; scalar floats use min/max or a branch.
    fn sel(&mut self, options: &relax_session::Options, d: VReg, c: VReg, a: VReg, b: VReg) {
        let int_data = self.f.vregs[d.0 as usize] == Cls::I;
        if let Some(i) = self.sel_cmp.take() {
            if !int_data {
                if let Some((w, op)) = scalar_minmax(&i, a, b) {
                    // x86 chooses its second input on both equality and unordered.
                    // Keep b second, even if the allocator made d alias b.
                    let xa = self.use_f(a, XS);
                    let xb = self.use_f(b, XS2);
                    let xd = self.def_f(d, XS);
                    if self.avx {
                        self.a.vex_rr(if w { 3 } else { 2 }, 1, false, op, xd, xa, xb);
                    } else {
                        let tmp = if xd == xb { 0 } else { xd };
                        self.a.movf_rr(true, tmp, xa);
                        self.a.sse_rr(if w { 0xf2 } else { 0xf3 }, op, tmp, xb, false);
                        self.a.movf_rr(true, xd, tmp);
                    }
                    self.fin_f(d, xd);
                    return;
                }
            }
            let (cc, parity) = match i {
                Inst::ICmp(cc, signed, _, x, y) => {
                    self.cmp_vregs(options, x, y);
                    (icc(cc, signed), None)
                }
                Inst::ICmpI(cc, signed, _, x, k) => {
                    let rx = self.use_i(options, x, R10);
                    self.cmp_imm(rx, k);
                    (icc(cc, signed), None)
                }
                Inst::FCmp(cc, w, _, x, y) => {
                    let xa = self.use_f(x, XS);
                    let xb = self.use_f(y, XS2);
                    let (left, right) = if matches!(cc, Cond::Lt | Cond::Le) { (xb, xa) } else { (xa, xb) };
                    self.a.sse_rr(if w { 0x66 } else { 0 }, 0x2e, left, right, false);
                    match cc {
                        Cond::Lt | Cond::Gt => (CC_A, None),
                        Cond::Le | Cond::Ge => (CC_AE, None),
                        Cond::Eq => (CC_E, Some(false)),
                        Cond::Ne => (CC_NE, Some(true)),
                    }
                }
                _ => unreachable!(),
            };
            if int_data {
                return self.cmov_sel(options, d, a, b, cc);
            }
            return self.float_sel(d, a, b, cc, parity);
        }
        if int_data {
            if let Some((fc, cc)) = self.sel_flags.take() {
                if fc == c {
                    return self.cmov_sel(options, d, a, b, cc);
                }
            }
            let rc = self.use_i(options, c, R10);
            self.a.test_rr(rc, rc);
            self.cmov_sel(options, d, a, b, CC_NE);
        } else {
            let rc = self.use_i(options, c, R10);
            self.a.test_rr(rc, rc);
            self.float_sel(d, a, b, CC_NE, None);
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

    fn ibin(&mut self, options: &relax_session::Options, op: IOp, it: IntTy, d: VReg, a: VReg, b: VReg) {
        // a spilled first operand of a register result loads straight into it, unless b lives there
        let ra = match (self.al.loc[a.0 as usize], self.al.loc[d.0 as usize], self.al.loc[b.0 as usize]) {
            (Loc::Stack(o), Loc::Reg(r), lb) if lb != Loc::Reg(r) && !matches!(op, IOp::Div | IOp::Rem | IOp::Shl | IOp::Shr | IOp::Rotl | IOp::MulHi) => {
                let disp = self.spill_disp(o);
                self.load_spill(r, disp);
                r
            }
            _ => self.use_i(options, a, R10),
        };
        // a spilled second operand is read by the operation itself: op rd, [rbp - o]
        if let (Loc::Stack(o), IOp::Add | IOp::Sub | IOp::And | IOp::Or | IOp::Xor | IOp::Mul) = (self.al.loc[b.0 as usize], op) {
            let disp = self.spill_disp(o);
            let rd = self.def_i(d, R10);
            self.a.mov_rr(rd, ra);
            // 32-bit forms read the slot's low half and zero-extend: u32 as the register forms
            let w = it != U32;
            self.a.rex(w, rd, 0, RBP, false);
            match op {
                IOp::Mul => {
                    self.a.byte(0x0f);
                    self.a.byte(0xaf);
                }
                IOp::Add => self.a.byte(0x03),
                IOp::Sub => self.a.byte(0x2b),
                IOp::And => self.a.byte(0x23),
                IOp::Or => self.a.byte(0x0b),
                _ => self.a.byte(0x33),
            }
            self.a.modrm_mem(rd, RBP, disp);
            if w && it.bits < 64 && !matches!(op, IOp::And | IOp::Or | IOp::Xor) {
                self.a.extend(rd, it.bits, it.signed);
            }
            self.fin_i(d, rd);
            return;
        }
        let rb = self.use_i(options, b, R11);
        let rd = self.def_i(d, R10);
        match op {
            IOp::MulHi => {
                // rdx:rax = rax * rb (mul / imul r/m64)
                let rb2 = if rb == RAX {
                    self.a.mov_rr(R11, rb);
                    R11
                } else {
                    rb
                };
                self.a.mov_rr(RAX, ra);
                self.a.grp3(if it.signed { 5 } else { 4 }, rb2);
                self.a.mov_rr(rd, RDX);
                self.fin_i(d, rd);
                return;
            }
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
            // BMI2 shlx / shrx / sarx: any registers, flags untouched (no rcx)
            IOp::Shl | IOp::Shr if host_bmi2(options) => {
                let w = it != U32;
                let pp = match op {
                    IOp::Shl => 1,
                    _ if it.signed && w => 2,
                    _ => 3,
                };
                self.a.vex_rr(pp, 2, w, 0xf7, rd, rb, ra);
                if w {
                    self.a.extend(rd, it.bits, it.signed);
                }
                self.fin_i(d, rd);
                return;
            }
            IOp::Rotl => {
                // rol by cl (32-bit form for u32: zero-extends)
                self.a.mov_rr(RCX, rb);
                self.a.mov_rr(RAX, ra);
                self.a.rex(it != U32, 0, 0, RAX, false);
                self.a.byte(0xd3);
                self.a.modrm_rr(0, RAX);
                self.a.mov_rr(rd, RAX);
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
        // a 64-bit sum into a third register: lea rd, [ra + rb] (one instruction for mov + add;
        // flags are not read after a register-register add, see last_alu)
        if op == IOp::Add && it.bits == 64 && rd != ra && rd != rb && rb != RSP {
            self.a.sib = Some((rb, 0));
            self.a.lea(rd, ra, 0);
            self.fin_i(d, rd);
            return;
        }
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

    fn conv(&mut self, options: &relax_session::Options, c: Conv, d: VReg, s: VReg) {
        match c {
            Conv::IntToInt(it) => {
                let rs = self.use_i(options, s, R10);
                let rd = self.def_i(d, R10);
                self.a.mov_rr(rd, rs);
                self.a.extend(rd, it.bits, it.signed);
                self.fin_i(d, rd);
            }
            Conv::IntToF(from, to64) => {
                let rs = self.use_i(options, s, R10);
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
                let rs = self.use_i(options, s, R10);
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

    fn call(&mut self, options: &relax_session::Options, c: &Callee, args: &[VReg], rets: &[VReg]) {
        if let Callee::CHost(_, sig) | Callee::CIndirect(_, sig) | Callee::CFn(_, sig) = c {
            return self.c_call(options, c, sig, args, rets);
        }
        // values in caller-saved registers that live across this call
        let saves = self.al.saves.get(&self.cur_pos).cloned().unwrap_or_default();
        for (v, slot) in &saves {
            self.save_restore(*v, *slot, true);
        }
        if let Callee::Indirect(v) = c {
            let r = self.use_i(options, *v, R11);
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
                self.mov_reloc(RAX, Reloc::Slot(*id));
                self.a.call_mem(RAX, 0);
            }
            Callee::Indirect(_) => self.a.call_r(R11),
            Callee::Host(sym) => {
                self.mov_reloc(R11, Reloc::Sym(*sym));
                self.a.mov_ri(RAX, nf as i64);
                self.a.call_r(R11);
            }
            Callee::CHost(..) | Callee::CIndirect(..) | Callee::CFn(..) => {}
        }
        self.last_call_ret = self.a.pos();
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
            self.save_restore(*v, *slot, false);
        }
    }

    fn ibin_imm(&mut self, options: &relax_session::Options, op: IOp, it: IntTy, d: VReg, a: VReg, k: i64) {
        // a spilled operand of a register result loads straight into it (no scratch + copy)
        let ra = match (self.al.loc[a.0 as usize], self.al.loc[d.0 as usize]) {
            (Loc::Stack(o), Loc::Reg(r)) if !matches!(op, IOp::Div | IOp::Rem | IOp::MulHi) => {
                let disp = self.spill_disp(o);
                self.load_spill(r, disp);
                r
            }
            _ => self.use_i(options, a, R10),
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
                IOp::Shl | IOp::Shr | IOp::Rotl => {
                    self.a.mov_rr(rd, ra);
                    self.a.rex(false, 0, 0, rd, false);
                    self.a.byte(0xc1);
                    self.a.modrm_rr(match op { IOp::Shl => 4, IOp::Rotl => 0, _ => 5 }, rd);
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
            IOp::Rotl => {
                // rol r64, imm8
                self.a.mov_rr(rd, ra);
                self.a.rex(true, 0, 0, rd, false);
                self.a.byte(0xc1);
                self.a.modrm_rr(0, rd);
                self.a.byte((k & 63) as u8);
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
            // d = a +- k into another register: lea d, [a + k] (no copy first)
            IOp::Add | IOp::Sub if rd != ra && (if op == IOp::Add { k } else { k.wrapping_neg() }) as i32 as i64 == (if op == IOp::Add { k } else { k.wrapping_neg() }) => {
                let disp = if op == IOp::Add { k } else { -k } as i32;
                self.a.lea(rd, ra, disp);
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
        let used_lea = matches!(op, IOp::Add | IOp::Sub) && rd != ra && (if op == IOp::Add { k } else { k.wrapping_neg() }) as i32 as i64 == (if op == IOp::Add { k } else { k.wrapping_neg() });
        if it.bits < 64 && !matches!(op, IOp::And | IOp::Or | IOp::Xor) {
            self.a.extend(rd, it.bits, it.signed);
        } else if matches!(op, IOp::Add | IOp::Sub | IOp::And | IOp::Or | IOp::Xor) && !used_lea && (-(1i64 << 31)..(1i64 << 31)).contains(&k) {
            self.last_alu = Some((rd, self.a.pos()));
        }
        self.fin_i(d, rd);
    }
}

fn use_gather(options: &relax_session::Options) -> bool {
    options.gather
}

/// The vreg a compare defines.
/// Can `load (sz bytes, sx: sign-extended) cmp k` read the memory in the load's width
/// (cmp [m], imm)? k must be a value of that width as the load extends it; a zero-extended
/// narrow value compared signed (or ordered against a constant outside its range) differs.
fn load_cmp_fits(sz: u8, sx: bool, c: Cond, signed: bool, k: i64) -> bool {
    let ordered = !matches!(c, Cond::Eq | Cond::Ne);
    match sz {
        8 => (i32::MIN as i64..=i32::MAX as i64).contains(&k),
        1 | 2 | 4 => {
            let b = sz as u32 * 8;
            if sx {
                (-(1i64 << (b - 1))..(1i64 << (b - 1))).contains(&k)
            } else {
                (0..(1i64 << b)).contains(&k) && !(ordered && signed)
            }
        }
        _ => false,
    }
}

fn cmp_def(i: &Inst) -> Option<VReg> {
    match i {
        Inst::ICmp(_, _, d, _, _) | Inst::ICmpI(_, _, d, _, _) | Inst::FCmp(_, _, d, _, _) => Some(*d),
        _ => None,
    }
}

/// Strict ordered selects return b for ties (including signed zero) and NaNs,
/// exactly like x86 MIN/MAX with b as the source. Non-strict comparisons cannot
/// use this: equality picks a but unordered picks b, requiring distinct handling.
fn scalar_minmax(i: &Inst, a: VReg, b: VReg) -> Option<(bool, u8)> {
    let Inst::FCmp(cc, w, _, x, y) = i else { return None };
    let direct = *x == a && *y == b;
    let swapped = *x == b && *y == a;
    match cc {
        Cond::Gt if direct => Some((*w, 0x5f)),
        Cond::Lt if swapped => Some((*w, 0x5f)),
        Cond::Lt if direct => Some((*w, 0x5d)),
        Cond::Gt if swapped => Some((*w, 0x5d)),
        _ => None,
    }
}

/// Does this CPU (and the environment: MAKEPAD_RUN_NO_AVX) allow VEX-encoded AVX instructions?
pub fn host_avx(options: &relax_session::Options) -> bool {
    options.cpu.avx
}

/// popcnt / lzcnt / tzcnt available (x86-64-v3 has all three; older CPUs get the
/// shift/mask expansions).
pub fn host_bits(options: &relax_session::Options) -> (bool, bool, bool) {
    (options.cpu.popcnt, options.cpu.lzcnt, options.cpu.bmi1)
}

pub fn compile(options: &relax_session::Options, f: &Func, env: &Env) -> Result<Compiled, String> {
    compile_freq(options, f, env, None)
}

/// `compile` with measured block frequencies of `f` (0..=1 of its hottest block, per block), which
/// the register allocator weighs uses by instead of loop depth.
/// Immediates the x64 emitter encodes: imm32 (sign-extended) for ALU ops, imul and compares,
/// any shift count (masked like the register form).
fn imm_fits(i: &Inst) -> bool {
    let imm32 = |v: i64| (i32::MIN as i64..=i32::MAX as i64).contains(&v);
    match *i {
        Inst::ICmpI(_, _, _, _, v) => imm32(v),
        Inst::IBinI(op, _, _, _, v) => match op {
            IOp::Shl | IOp::Shr | IOp::Rotl => true,
            IOp::Add | IOp::Sub | IOp::Mul | IOp::And | IOp::Or | IOp::Xor => imm32(v),
            _ => false,
        },
        _ => true,
    }
}

pub fn compile_freq(options: &relax_session::Options, f: &Func, env: &Env, freq: Option<&[f32]>) -> Result<Compiled, String> {
    let _warm = crate::warm::Scope::backend(&f.name);
    let _cost_codegen = crate::fncost::phase(5);
    crate::fncost::size(false, f);
    let expanded;
    let f = if f.blocks.iter().any(|b| b.insts.iter().any(|i| matches!(i, Inst::IBit(..)))) {
        let (pc, lz, tz) = host_bits(options);
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
    // constants used in loops defined once at the entry (the allocator keeps them in registers
    // or recomputes them); constant operands that fit an imm32 become immediates
    // (hot code only: the rewrite copies the function, +15% back-end time at O1)
    let hoisted = if env.opt_regs || f.full { regalloc::hoist_loop_consts(f, &imm_fits) } else { None };
    let f = hoisted.as_ref().unwrap_or(f);
    // compares next to the selects that read them (the cmov takes the flags)
    let sunk = if env.opt_regs || f.full { regalloc::sink_compares(f) } else { None };
    let f = sunk.as_ref().unwrap_or(f);
    let mut cfg = reg_config(options);
    cfg.sym_class = env.sym_class;
    let _cost_ra = crate::fncost::phase(9);
    let t_ra = crate::opt::pass_clock();
    let (split, al) = regalloc::allocate_split(options, f, &cfg, env.opt_regs || f.full, freq);
    let f = split.as_ref().unwrap_or(f);
    crate::opt::pass_time(11, t_ra);
    drop(_cost_ra);
    let alloc_sig = regalloc::alloc_sig(&al);
    let _cost_emit = crate::fncost::phase(10);
    let t_emit = crate::opt::pass_clock();
    let jt_plan = crate::arm64::jump_tables(options, f, &al);
    let (win, win_start) = {
        let mut w: std::collections::HashMap<u32, Vec<(u32, u32, u8)>> = std::collections::HashMap::new();
        let mut ws: std::collections::HashMap<u32, Vec<(u32, u8)>> = std::collections::HashMap::new();
        for &(v, a, z, r) in &al.windows {
            w.entry(v).or_default().push((a, z, r));
            ws.entry(a).or_default().push((v, r));
        }
        (w, ws)
    };
    let mut g = Gen {
        a: Asm { b: Vec::with_capacity(256), sib: None },
        f,
        al,
        saved: 0,
        slot_off: Vec::new(),
        block_pos: Vec::new(),
        fixups: Vec::new(),
        pc_map: Vec::new(),
        rir_map: Vec::new(),
        rir_map_on: RIR_MAP.load(std::sync::atomic::Ordering::Relaxed),
        last_alu: None,
        cur_pos: 0,
        fpool: Vec::new(),
        next_block: Vec::new(),
        thru: Vec::new(),
        frame: 0,
        text_len: 0,
        mem_const: None,
        test_fuse: None,
        load_fuse: None,
        jt_plan,
        jt_fix: Vec::new(),
        last_spill: None,
        win,
        win_start,
        fconsts: {
            let mut nd: std::collections::HashMap<u32, (u32, u64, bool)> = std::collections::HashMap::new();
            for b in &f.blocks {
                for i in &b.insts {
                    if let Inst::Fconst(d, bits, w) = i {
                        nd.entry(d.0).or_insert((0, *bits, *w)).0 += 1;
                    } else {
                        crate::regalloc::for_defs(i, |d| {
                            if let Some(e) = nd.get_mut(&d.0) {
                                e.0 += 2;
                            }
                        });
                    }
                }
            }
            nd.into_iter().filter(|(_, v)| v.0 == 1).map(|(k, v)| (k, (v.1, v.2))).collect()
        },
        // off: pinned on node 165 it costs 12% on the parity test and nothing elsewhere; code/pool
        // cache-line separation does not change that (tested). Cause not proven.
        memconst: options.memconst || host_avx(options),
        avx: host_avx(options),
        byte_masks: crate::opt::vector_byte_masks(f),
        sel_cmp: None,
        sel_flags: None,
        vpool: Vec::new(),
        apool: Vec::new(),
        sym_class: env.sym_class,
        dom_slot: 0,
        relocs: Vec::new(),
        cplan: None,
        cbuf: Vec::new(),
        cret_buf: 0,
        win_saves: 0,
        cret_ptr: 0,
        stage: 0,
        last_call_ret: 0,
        pads: Vec::new(),
        unwind_tab: Vec::new(),
    };
    g.gen(options);
    // unwind info for the runtime's frame walk (rir::unwind_info_words), 4-byte aligned
    while g.a.b.len() % 4 != 0 {
        g.a.byte(0xcc);
    }
    let saved: Vec<u32> = g.al.used_callee_int.iter().map(|r| *r as u32).collect();
    let tab = std::mem::take(&mut g.unwind_tab);
    for w in crate::rir::unwind_info_words(&saved, &tab, (g.saved + g.frame) as u32) {
        g.a.u32(w);
    }
    let frame = g.frame as u32;
    let text_len = g.text_len;
    crate::opt::pass_time(12, t_emit);
    crate::fncost::code(g.a.b.len());
    Ok(Compiled { code: g.a.b, pc_map: g.pc_map, frame_size: frame, text_len, alloc_sig, relocs: g.relocs, rir_emitted: if g.rir_map_on { g.f.dump() } else { String::new() }, rir_map: g.rir_map })
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

/// Whether `f` takes the address of a domain symbol (a static: its code reads the domain base).
fn uses_domain(f: &Func, sym_class: u64) -> bool {
    sym_class != 0 && f.blocks.iter().any(|b| b.insts.iter().any(|i| matches!(i, Inst::Addr(_, s) if SymClass::of(sym_class, s.id) == SymClass::Domain)))
}

/// This thread's thread-local block in r11, preserving every other register JIT code may hold:
/// the fast path `tls_addr` inlines (`mov r11, fs:[key]`, gs on Windows), and when the cell is
/// still null `host() -> block`. `tls_addr` calls it after its own fast path found null; a
/// prologue that needs the domain base calls it directly. Entered with rsp = 8 mod 16.
pub fn tls_slow(host: u64, key: u64) -> Vec<u8> {
    let mut a = Asm { b: Vec::new(), sib: None };
    // mov r11, seg:[disp32]: modrm 00/011/100 + SIB 0x25 (no base, no index)
    a.b.extend_from_slice(&[TLS_SEG, 0x4c, 0x8b, 0x1c, 0x25]);
    a.u32(key as i64 as i32 as u32);
    a.test_rr(R11, R11);
    // jz +1 (past the ret)
    a.b.extend_from_slice(&[0x74, 0x01]);
    a.ret();
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

/// Windows hang watchdog glue: the watching thread redirects a looping JIT thread here as if it
/// had called it from the interrupted pc ([rsp] = that pc, any stack alignment, every register
/// live). Saves flags, the 15 general registers and xmm0-15, calls `host(regs)` (Relax's
/// convention; regs = [r15, r14, .., rax, rflags, return pc], ascending) and, when it returns,
/// restores everything and returns to the interrupted pc. The host never returns when it
/// unwinds the call with a hang report.
pub fn hang_tramp(host: u64) -> Vec<u8> {
    let mut a = Asm { b: Vec::new(), sib: None };
    a.byte(0x9c); // pushfq
    let regs = [RAX, RCX, RDX, RBX, RBP, RSI, RDI, R8, R9, R10, R11, R12, R13, R14, R15];
    for r in regs {
        a.push(r);
    }
    a.mov_rr(RBP, RSP);
    // and rsp, -16
    a.b.extend_from_slice(&[0x48, 0x83, 0xe4, 0xf0]);
    a.sub_rsp(256);
    for x in 0..16u8 {
        a.sse_rm(0xf3, 0x7f, x, RSP, 16 * x as i32); // movdqu [rsp + 16x], xmm
    }
    a.mov_rr(RDI, RBP);
    a.mov_ri(RAX, host as i64);
    a.call_r(RAX);
    for x in 0..16u8 {
        a.sse_rm(0xf3, 0x6f, x, RSP, 16 * x as i32); // movdqu xmm, [rsp + 16x]
    }
    a.mov_rr(RSP, RBP);
    let mut i = regs.len();
    while i > 0 {
        i -= 1;
        a.pop(regs[i]);
    }
    a.byte(0x9d); // popfq
    a.ret();
    a.b
}

/// `tls_read(key) -> u64` (Relax's convention: key in rdi): the word `TlsAddr` reads, `gs:[key]`
/// (Windows: the runtime checks its TlsSlots offset against TlsGetValue).
pub fn gs_reader() -> Vec<u8> {
    // mov rax, gs:[rdi] ; ret
    vec![0x65, 0x48, 0x8b, 0x07, 0xc3]
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
    // A 128-byte area saves rdi rsi rdx rcx r8 r9 and the low words of xmm0-7, 16-byte aligned.
    // A stub may be entered under Win64 on any x86-64 OS (`extern "win64"`, or Windows C
    // callbacks), for which xmm6-15 are callee-saved;
    // `host_compile` is a sysv64 fn and may clobber them, so they are kept here too (rsi / rdi,
    // also callee-saved for Win64, are among the argument registers saved above).
    let xsave = 160;
    a.sub_rsp(128 + xsave as u32);
    let regs = [RDI, RSI, RDX, RCX, R8, R9];
    for (i, r) in regs.iter().enumerate() {
        a.store64(RSP, 8 * i as i32, *r);
    }
    for x in 0..8u8 {
        a.storef(true, RSP, 48 + 8 * x as i32, x);
    }
    for k in 0..(xsave / 16) as u8 {
        // movups [rbp - 16 (k + 1)], xmm(6 + k)
        a.sse_rm(0, 0x11, 6 + k, RBP, -16 * (k as i32 + 1));
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
    for k in 0..(xsave / 16) as u8 {
        a.sse_rm(0, 0x10, 6 + k, RBP, -16 * (k as i32 + 1));
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
    let mut a = Asm { b: Vec::with_capacity(48), sib: None };
    a.mov_ri(RAX, slot_addr as i64);
    a.jmp_mem(RAX, 0);
    while a.b.len() < size {
        a.byte(0xcc);
    }
    a.b
}

/// Per-slot call counter (tier-up): while a function runs at O1 its table slot points
/// here. `*counter -= 1`; on reaching 0 jumps to `hot` (lazy_entry-style glue: rax = slot,
/// returns the address to continue at), else to the O1 code. Uses rax and r11 only.
pub fn count_stub(counter: u64, slot: u32, code: u64, hot: u64) -> Vec<u8> {
    let mut a = Asm { b: Vec::with_capacity(48), sib: None };
    a.mov_ri(RAX, counter as i64);
    // sub qword [rax], 1
    for x in [0x48u8, 0x83, 0x28, 0x01] {
        a.byte(x);
    }
    // je hot (rel8, patched below)
    a.byte(0x74);
    a.byte(0);
    let je = a.b.len();
    a.mov_ri(R11, code as i64);
    a.rex(false, 0, 0, R11, false);
    a.byte(0xff);
    a.modrm_rr(4, R11);
    let off = a.b.len() - je;
    assert!(off <= 127, "count stub: je rel8 out of range");
    a.b[je - 1] = off as u8;
    a.mov_ri(RAX, slot as i64);
    a.mov_ri(R11, hot as i64);
    a.rex(false, 0, 0, R11, false);
    a.byte(0xff);
    a.modrm_rr(4, R11);
    a.b
}

/// Per-slot stub: `mov eax, slot; jmp lazy_entry` (absolute via r11).
pub fn stub(slot: u32, lazy: u64) -> Vec<u8> {
    let mut a = Asm { b: Vec::with_capacity(48), sib: None };
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

/// land(regs, fp, sp, pc): continues at `pc` (a landing stub) with rbx, r12..r15 from the
/// register block `regs` ([rbp, return address, rbx, r12, r13, r14, r15], the panic
/// trampoline's layout), rbp = fp and rsp = sp. Never returns.
pub fn land() -> Vec<u8> {
    let mut a = Asm { b: Vec::new(), sib: None };
    a.load64(RBX, RDI, 16);
    a.load64(R12, RDI, 24);
    a.load64(R13, RDI, 32);
    a.load64(R14, RDI, 40);
    a.load64(R15, RDI, 48);
    a.mov_rr(RBP, RSI);
    a.mov_rr(RSP, RDX);
    a.jmp_r(RCX);
    a.b
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A block entered both by falling through from an ALU op and by a jump must test its
    /// `x == 0` branch itself: the ALU op's ZF holds only on the fall-through edge (the jump
    /// arrives with its own compare's flags). shader_ir::ty crashed this way under O3 (a
    /// None read as Some, then a null dereference).
    #[test]
    fn merge_block_branch_tests_its_value() {
        let options = &relax_session::Options::default();
        let mut f = Func::new("merge".to_string(), 0);
        let (p, q) = (f.vreg(Cls::I), f.vreg(Cls::I));
        f.params.push(p);
        f.params.push(q);
        f.rets.push(Cls::I);
        // b0: q != 0 ? b1 (x = [p], jumps to b3) : b2 (x = p + 12, falls through into b3)
        let (b0, b1, b2, b3, b4, b5) = (f.block(), f.block(), f.block(), f.block(), f.block(), f.block());
        let (c, x, z, one) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I));
        let u64t = IntTy { bits: 64, signed: false };
        let push = |f: &mut Func, b: u32, i: Inst| {
            f.blocks[b as usize].insts.push(i);
            f.blocks[b as usize].pos.push(0);
        };
        push(&mut f, b0, Inst::ICmpI(Cond::Ne, false, c, q, 0));
        f.blocks[b0 as usize].term = Term::Branch(c, b1, b2);
        push(&mut f, b1, Inst::Load(Mem::Int(8, false), x, p, 0));
        f.blocks[b1 as usize].term = Term::Jump(b3);
        push(&mut f, b2, Inst::IBinI(IOp::Add, u64t, x, p, 12));
        f.blocks[b2 as usize].term = Term::Jump(b3);
        push(&mut f, b3, Inst::ICmpI(Cond::Eq, false, z, x, 0));
        f.blocks[b3 as usize].term = Term::Branch(z, b4, b5);
        push(&mut f, b4, Inst::Iconst(one, 1));
        f.blocks[b4 as usize].term = Term::Ret(vec![one]);
        f.blocks[b5 as usize].term = Term::Ret(vec![x]);
        let env = Env { slots: SlotsPtr::NONE, tls_key: 0, tls_slow: 0, resume: 0, opt_regs: true, syms: 0, sym_class: 0 };
        let r = compile(options, &f, &env).expect("compiles");
        let code = &r.code[..r.text_len as usize];
        // add r64, 12: REX.W(+B) 83 /0 ib
        let k = (0..code.len().saturating_sub(4)).find(|&k| (code[k] & 0xfa) == 0x48 && code[k + 1] == 0x83 && code[k + 2] & 0xf8 == 0xc0 && code[k + 3] == 12).expect("add r, 12");
        let next = &code[k + 4..];
        let jcc = next[0] & 0xf0 == 0x70 || (next[0] == 0x0f && next[1] & 0xf0 == 0x80);
        assert!(!jcc, "the merge block branches on the add's flags: {:02x?}", &code[k..(k + 12).min(code.len())]);
    }
}

/// Detect the effective JIT capabilities before the driver freezes Options.
/// rustc: compiler/rustc_session/src/session.rs:91-93 (target and options).
pub fn detect_cpu(options: &relax_session::Options) -> relax_session::options::CpuFeatures {
    #[cfg(target_arch = "x86_64")]
    {
        let mut cpu = relax_session::options::CpuFeatures::default();
        cpu.avx = !options.no_avx && std::is_x86_feature_detected!("avx");
        cpu.bmi2 = !options.no_bmi2 && std::is_x86_feature_detected!("bmi2");
        cpu.popcnt = !options.no_bitops && std::is_x86_feature_detected!("popcnt");
        cpu.lzcnt = !options.no_bitops && std::is_x86_feature_detected!("lzcnt");
        cpu.bmi1 = !options.no_bitops && std::is_x86_feature_detected!("bmi1");
        cpu.fma = cpu.avx && std::is_x86_feature_detected!("fma");
        cpu
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        let _ = options;
        relax_session::options::CpuFeatures::default()
    }
}

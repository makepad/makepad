//! AArch64 backend (O1): instruction selection and encoding over the shared
//! register allocator (regalloc.rs), plus the runtime glue code the live image
//! needs (lazy-compile entry, stubs, thunks, enter/leave, panic trampolines).
//!
//! ABI (Relax's own = AAPCS64 for host calls): int args x0-x7, float args v0-v7,
//! int returns x0 x1, float returns v0 v1. Callee-saved x19-x28 and the low 64 bits
//! of v8-v15. x18 is the platform register on Darwin and never touched. Frame
//! pointers always on: every function stores its frame record (x29, x30) at x29.
//! Scratch, never allocated: x14 x15 (operands), x16 (call targets, immediates),
//! x17 (address offsets), v29 v30 v31.
//!
//! Frame (fp = x29 after the prologue):
//!   [fp + 8]  return address, [fp] caller's fp
//!   [fp - 8 * k]  callee-saved registers (ints then floats)
//!   below: spill area, stack slots, C ABI buffers/staging cells, outgoing stack arguments;
//!   sp = fp - frame (16-aligned)
//! The body addresses the frame through sp with positive offsets.
//!
//! Encodings follow the hand-written encoders of platform/script/compute/src/arm64.rs,
//! extended to 64-bit integer forms.

use crate::cabi::{self, ArgPlan, PLoc, Plan, RetPlan};
use crate::regalloc::{self, Alloc, Loc, MLoc, RegConfig, Remat};
use crate::rir::*;

// NEON vector instruction words (.4s / .16b forms; registers or-ed in by the emitters)
mod v4s {
    pub const FADD: u32 = 0x4E20_D400;
    pub const FSUB: u32 = 0x4EA0_D400;
    pub const FMUL: u32 = 0x6E20_DC00;
    pub const FDIV: u32 = 0x6E20_FC00;
    pub const FCMEQ: u32 = 0x4E20_E400;
    pub const FCMGE: u32 = 0x6E20_E400;
    pub const FCMGT: u32 = 0x6EA0_E400;
    pub const BSL: u32 = 0x6E60_1C00;
    pub const CMEQ: u32 = 0x6EA0_8C00;
    pub const CMGT: u32 = 0x4EA0_3400;
    pub const CMGE: u32 = 0x4EA0_3C00;
    pub const SSHL: u32 = 0x4EA0_4400;
    pub const USHL: u32 = 0x6EA0_4400;
    pub const NEG: u32 = 0x6EA0_B800;
    pub const NOT: u32 = 0x6E20_5800;
    pub const VFNEG: u32 = 0x6EA0_F800;
}


pub const THUNK_SIZE: usize = 32;
pub const STUB_SIZE: usize = 32;

const SP: u8 = 31;
const ZR: u8 = 31;
const FP: u8 = 29;
const LR: u8 = 30;
const S0: u8 = 14; // operand scratch
const S1: u8 = 15; // operand scratch
const IP0: u8 = 16; // call targets, immediates
const IP1: u8 = 17; // address offsets
const FS0: u8 = 30; // float scratch
const FS1: u8 = 31;
/// third float scratch (fused multiply-add's addend)
const FS2: u8 = 29;
/// x8: indirect result register of the C ABI; otherwise a scratch for atomics loops
const X8: u8 = 8;

/// Whether the CPU running the JIT'd code has ARMv8.1 LSE atomics (`cas`, `ldadd`, ...). Every
/// Apple arm64 CPU does; Android and Linux ask the kernel (HWCAP_ATOMICS: Cortex-A53/A73 class
/// cores have none). `MAKEPAD_RUN_NO_LSE=1` forces the ARMv8.0 exclusive-load/store loops (to test
/// them on an LSE machine; the compile cache does not key on this, so test with an empty cache).
pub fn lse_atomics() -> bool {
    static LSE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *LSE.get_or_init(|| {
        if std::env::var_os("MAKEPAD_RUN_NO_LSE").is_some_and(|v| !v.is_empty()) {
            return false;
        }
        #[cfg(all(target_arch = "aarch64", any(target_os = "linux", target_os = "android")))]
        {
            extern "C" {
                fn getauxval(t: u64) -> u64;
            }
            const AT_HWCAP: u64 = 16;
            const HWCAP_ATOMICS: u64 = 1 << 8;
            return unsafe { getauxval(AT_HWCAP) } & HWCAP_ATOMICS != 0;
        }
        #[allow(unreachable_code)]
        true
    })
}

const INT_ARGS: usize = 8;
const FLT_ARGS: usize = 8;

/// arm64 isel switches for measurements: `MAKEPAD_RUN_A64=none` or `-fuse,-fconst`.
const A64_FUSE: u8 = 1;
const A64_FCONST: u8 = 2;
/// struct copies of >= 32 bytes through 16-byte vector pairs (off: 8-byte pieces; measurement switch)
const A64_QCOPY: u8 = 4;

fn isel_flags(options: &relax_session::Options) -> u8 {
    let mut f = A64_FUSE | A64_FCONST | A64_QCOPY;
    for part in &options.a64 {
        match part.as_str() {
            "none" => f = 0,
            "-fuse" => f &= !A64_FUSE,
            "-fconst" => f &= !A64_FCONST,
            "-qcopy" => f &= !A64_QCOPY,
            _ => {}
        }
    }
    f
}

pub fn reg_config() -> RegConfig {
    RegConfig {
        int_caller: vec![9, 10, 11, 12, 13, 2, 3, 4, 5, 6, 7],
        int_callee: vec![19, 20, 21, 22, 23, 24, 25, 26, 27, 28],
        flt_caller: vec![16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 0, 1, 2, 3, 4, 5, 6, 7],
        flt_callee: vec![8, 9, 10, 11, 12, 13, 14, 15],
        slot_index_free: false,
        clobbers: Some(arm64_clobbers),
        sym_class: 0,
        store_imm: |k| k == 0,
        fconst_remat: true,
        mem_operands: false,
        remat_cost: arm64_remat_cost,
        float_windows: true,
        defers: arm64_defers,
    }
}

/// RegConfig::defers: the pairs the block loop emits at the second instruction's position --
/// a constant shift into the next add / sub / and / orr / eor (shifted_operand), a 64-bit multiply
/// into the next add / sub or indexed access (mul_add), a byte / bit reverse into the next
/// right shift by 32 / 48 (narrow_bswap), and `t = x & K; c = t ==/!= 0` before a branch on c (the
/// tst). The first result is read only by the second. The emitter's own conditions (flags, bounds,
/// operand locations) only narrow these shapes; the loop asks this before each deferral.
fn arm64_defers(b: &Block, k: usize, uses: &[u32]) -> bool {
    let n = b.insts.len();
    if k + 1 >= n {
        return false;
    }
    let once = |t: VReg| uses[t.0 as usize] == 1;
    let next = &b.insts[k + 1];
    match b.insts[k] {
        Inst::IBinI(IOp::Shl | IOp::Shr, it, t, _, sh) if it.bits == 64 && (1..64).contains(&sh) && once(t) => match *next {
            Inst::IBin(IOp::Add | IOp::Sub | IOp::And | IOp::Or | IOp::Xor, it, _, x, y) if it.bits == 64 && y == t && x != t => true,
            Inst::IBin(IOp::Add | IOp::And | IOp::Or | IOp::Xor, it, _, x, y) if it.bits == 64 && x == t && y != t => true,
            _ => false,
        },
        Inst::IBin(IOp::Mul, it, t, a, m) if it.bits == 64 && a != t && m != t && once(t) => mul_add_next(t, next),
        Inst::IBinI(IOp::Mul, it, t, a, _) if it.bits == 64 && a != t && once(t) => mul_add_next(t, next),
        Inst::IBit(BitOp::Bswap | BitOp::Rev, t, _) if once(t) => matches!(*next, Inst::IBinI(IOp::Shr, it, _, x, 32 | 48) if x == t && it.bits == 64 && !it.signed),
        Inst::IBinI(IOp::And, _, t, x, _) if k + 2 == n && x != t && once(t) => matches!((next, &b.term), (Inst::ICmpI(Cond::Eq | Cond::Ne, _, c, t2, 0), Term::Branch(c2, _, _)) if *t2 == t && c == c2 && once(*c)),
        _ => false,
    }
}

/// mul_add's second instructions for a product t
fn mul_add_next(t: VReg, next: &Inst) -> bool {
    match *next {
        Inst::LoadX(_, _, base, x, 0, _) => x == t && base != t,
        Inst::StoreX(_, base, x, 0, _, src) => x == t && base != t && src != t,
        Inst::IBin(IOp::Add, it, _, x, y) => it.bits == 64 && (x == t) != (y == t),
        Inst::IBin(IOp::Sub, it, _, x, y) => it.bits == 64 && y == t && x != t,
        _ => false,
    }
}

/// Instructions Gen::remat_into / mov_imm take to recompute a value (RegConfig::remat_cost): a
/// constant is one orr for a logical immediate, else one movz / movn plus a movk per remaining
/// halfword that is neither all zeros nor all ones; addresses and slot offsets one.
fn arm64_remat_cost(r: &Remat) -> u8 {
    let Remat::Const(v) = *r else { return 1 };
    let u = v as u64;
    let (mut zeros, mut ones) = (0u8, 0u8);
    for k in 0..4 {
        let h = (u >> (16 * k)) & 0xffff;
        zeros += (h == 0) as u8;
        ones += (h == 0xffff) as u8;
    }
    if zeros.max(ones) < 3 && bitmask_imm(u).is_some() {
        return 1;
    }
    (4 - zeros.max(ones)).max(1)
}

/// Registers an instruction overwrites while it runs (RegConfig::clobbers): an asm! block's
/// (every register it names; regalloc also keeps its results out of them), and v0-v7 (bits
/// 32..40) for every instruction whose emitter arm may name them: calls (float arguments and
/// results, aggregate result registers) and the vector sequences with v4-v7 temporaries.
/// v0-v7 are allocatable caller-saved registers everywhere else (reg_config).
fn arm64_clobbers(_options: &relax_session::Options, f: &Func, i: &Inst) -> u64 {
    match i {
        Inst::Call(Callee::Asm(b), _, _) => b.regs | V0_7,
        _ if keeps_v0_7(f, i) => 0,
        _ => V0_7,
    }
}

/// pair_access's stand-in memory kind for VLoad / VStore pairs
const V128_MEM: Mem = Mem::F64;

/// v0-v7 as regalloc clobber bits (float register r = bit 32 + r)
const V0_7: u64 = 0xff << 32;

/// The instructions whose emitter arms (and the helpers they call) never name v0-v7: they reach
/// vector registers only through use_f / def_f / fin_f / vtmp and the scratch registers v29-v31.
/// Checked against `Gen::inst`, `vinst`, `vx_lanes` and `vx` arm by arm; anything not listed
/// clobbers v0-v7.
fn keeps_v0_7(f: &Func, i: &Inst) -> bool {
    use VW::*;
    match i {
        Inst::Iconst(..) | Inst::IBinI(..) | Inst::ICmpI(..) | Inst::ICmp(..) | Inst::IBin(..) | Inst::IBit(..) | Inst::INeg(..) | Inst::INot(..) => true,
        Inst::SlotAddr(..) | Inst::Addr(..) | Inst::FnAddr(..) | Inst::Mov(..) | Inst::Load(..) | Inst::Store(..) | Inst::LoadX(..) | Inst::StoreX(..) => true,
        Inst::Fconst(..) | Inst::FBin(..) | Inst::FUnary(..) | Inst::FCmp(..) | Inst::FMulAdd(..) | Inst::Sel(..) | Inst::Conv(..) | Inst::Copy(..) => true,
        Inst::VLoad(..) | Inst::VStore(..) | Inst::VLoadX(..) | Inst::VStoreX(..) | Inst::VConst(..) | Inst::VSplat(..) | Inst::VExtract(..) | Inst::VInsert(..) => true,
        Inst::VSel(..) | Inst::VFma(..) | Inst::VAny(..) | Inst::VGather(..) => true,
        Inst::VX(op, ..) => {
            let _ = f;
            match *op {
                VX::FAdd(w, n) | VX::FSub(w, n) | VX::FMul(w, n) | VX::FDiv(w, n) => matches!(w, S | D) && n != NanBits::Intel,
                VX::FMinNeon(w) | VX::FMaxNeon(w) | VX::FMinLt(w) | VX::FMaxGt(w) | VX::FCmp(_, w) => matches!(w, S | D),
                VX::MinS(D) | VX::MaxS(D) | VX::MinU(D) | VX::MaxU(D) => true,
                VX::ShlV(_) | VX::ShrSV(_) | VX::ShrUV(_) | VX::ICmp(..) | VX::UCmp(..) | VX::Neg(_) | VX::FCvtLo => true,
                VX::FNeg(w) | VX::FAbs(w) | VX::FRound(w, _) | VX::FToI(w) | VX::IToF(w) => matches!(w, S | D),
                VX::FSqrt(w, n) => matches!(w, S | D) && n != NanBits::Intel,
                // vx's three-same table
                VX::Add(_) | VX::Sub(_) | VX::MaxS(_) | VX::MinS(_) | VX::MaxU(_) | VX::MinU(_) | VX::CmpEq(_) | VX::CmpGtS(_) => true,
                VX::AddSatU(_) | VX::AddSatS(_) | VX::SubSatU(_) | VX::SubSatS(_) | VX::AvgU(_) | VX::ZipLo(_) | VX::ZipHi(_) => true,
                VX::MulLo(w) => w != D,
                VX::And | VX::Or | VX::Xor | VX::AndNot | VX::Splat(_) | VX::FromLo64 | VX::Extract(..) => true,
                _ => false,
            }
        }
        _ => false,
    }
}

static RIR_MAP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Stack storage alignment floor (frame::slot_layout_aligned): ldr / str x and ldp / stp scale
/// their immediate by the access size, so an 8-byte field of a 4-aligned aggregate past the
/// unscaled range needs `mov ip, #off; ldr [sp, ip]`, and q pairs on 16 need a 16-aligned base
fn a64_slot_align(size: u32) -> u32 {
    if size >= 16 {
        16
    } else if size >= 8 {
        8
    } else {
        1
    }
}

/// The emitted function's RIR, then `# counts` with each block's profile count (-1 = not counted)
fn emitted_dump(f: &Func) -> String {
    let mut s = f.dump();
    s.push_str("# counts");
    for b in &f.blocks {
        s.push_str(&format!(" {}", b.count));
    }
    s.push('\n');
    s
}

/// Record `Compiled::rir_map` (code offset -> RIR block/instruction) in later compiles (asm dumps)
pub fn set_rir_map(on: bool) {
    RIR_MAP.store(on, std::sync::atomic::Ordering::Relaxed);
}

pub struct Compiled {
    pub code: Vec<u8>,
    /// (code offset, source position) in ascending offset order
    pub pc_map: Vec<(u32, u64)>,
    pub frame_size: u32,
    /// bytes of instructions (the float literal pool follows)
    pub text_len: u32,
    /// regalloc::alloc_sig of the allocation (0 unless regalloc::SIG_ON)
    pub alloc_sig: u64,
    /// the relocated fields, emitted zeroed: the embedder links the code (`rir::link`) where it
    /// installs it
    pub relocs: Vec<RelocAt>,
    /// (code offset, block, instruction or u32::MAX = terminator) of every RIR instruction
    /// emitted -- filled only while `set_rir_map(true)` (asm dumps)
    pub rir_map: Vec<(u32, u32, u32)>,
    pub rir_emitted: String,
}

/// Addresses the backend needs from the runtime.
pub struct Env {
    /// where the function slots' tables and thunks are
    pub slots: SlotsPtr,
    /// pthread TSD slot holding this thread's thread-local block (0 = none yet)
    pub tls_key: u64,
    /// glue that allocates the calling thread's block: returns it in x16, preserves all else
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
    /// the embedder links branch fields (`X64Branch32` / `A64Branch26`) with veneers for targets
    /// out of reach (`rir::link_with`): host and runtime calls become direct `call`/`bl`. Off:
    /// they call through pool words (position-independent code, e.g. stored compute kernels).
    pub near: bool,
}

impl Env {
    /// The current value of a relocated word.
    pub fn reloc_value(&self, r: Reloc) -> u64 {
        match r {
            Reloc::Sym(s) => unsafe { s.value(self.syms) },
            Reloc::Slot(id) => self.slots.table(id),
            Reloc::Thunk(id) => self.slots.callable(id),
            Reloc::Call(id) => self.slots.thunk(id),
            // the TSD slot's byte offset (ldr x16, [x16, x17])
            Reloc::TlsKey => self.tls_key * 8,
            Reloc::TlsSlow => self.tls_slow,
            Reloc::Resume => self.resume,
            Reloc::Value(v) => v,
        }
    }
}

// ------------------------------------------------------------ encoder

// condition codes
const EQ: u8 = 0;
const NE: u8 = 1;
const HS: u8 = 2;
const LO: u8 = 3;
const MI: u8 = 4;
const HI: u8 = 8;
const LS: u8 = 9;
const GE: u8 = 10;
const LT: u8 = 11;
const GT: u8 = 12;
const LE: u8 = 13;

// load/store, unsigned scaled offset forms
const LDRB: u32 = 0x3940_0000;
const LDRSB: u32 = 0x3980_0000;
const STRB: u32 = 0x3900_0000;
const LDRH: u32 = 0x7940_0000;
const LDRSH: u32 = 0x7980_0000;
const STRH: u32 = 0x7900_0000;
const LDRW: u32 = 0xB940_0000;
const LDRSW: u32 = 0xB980_0000;
const STRW: u32 = 0xB900_0000;
const LDRX: u32 = 0xF940_0000;
const STRX: u32 = 0xF900_0000;
const LDRS: u32 = 0xBD40_0000;
const STRS: u32 = 0xBD00_0000;
const LDRD: u32 = 0xFD40_0000;
const STRD: u32 = 0xFD00_0000;

// float data processing (single; | DBL for double)
const DBL: u32 = 0x0040_0000;
const FADD: u32 = 0x1E20_2800;
const FSUB: u32 = 0x1E20_3800;
const FMUL: u32 = 0x1E20_0800;
const FDIV: u32 = 0x1E20_1800;
// added by the compiler lane for FOp::Min/Max (IEEE minNum/maxNum = Rust min/max)
const FMAXNM: u32 = 0x1E20_6800;
const FMINNM: u32 = 0x1E20_7800;
const FABS: u32 = 0x1E20_C000;
const FNEG: u32 = 0x1E21_4000;
const FSQRT: u32 = 0x1E21_C000;
const FRINTN: u32 = 0x1E24_4000;
const FRINTP: u32 = 0x1E24_C000;
const FRINTM: u32 = 0x1E25_4000;
const FRINTZ: u32 = 0x1E25_C000;
const FRINTA: u32 = 0x1E26_4000;
const FMOVR: u32 = 0x1E20_4000;

struct Asm {
    w: Vec<u32>,
}

impl Asm {
    fn e(&mut self, x: u32) {
        self.w.push(x);
    }
    fn pos(&self) -> usize {
        self.w.len()
    }

    // ---- integer moves and constants
    fn mov(&mut self, d: u8, m: u8) {
        if d != m {
            // orr xd, xzr, xm
            self.e(0xAA00_03E0 | (m as u32) << 16 | d as u32);
        }
    }
    /// mov to/from sp (add xd, xn, #0)
    fn mov_sp(&mut self, d: u8, n: u8) {
        self.e(0x9100_0000 | (n as u32) << 5 | d as u32);
    }
    fn mov_imm(&mut self, d: u8, v: i64) {
        let u = v as u64;
        let mut zeros = 0;
        let mut ones = 0;
        for k in 0..4 {
            let h = (u >> (16 * k)) & 0xffff;
            if h == 0 {
                zeros += 1;
            }
            if h == 0xffff {
                ones += 1;
            }
        }
        // a value movz/movn + movk need 2+ instructions for that is a logical immediate (SWAR masks
        // 0x8080.., 0x0101.., 0x00ff00ff..): orr d, xzr, #imm (register 31 is sp as orr's destination)
        if zeros.max(ones) < 3 && d != 31 {
            if let Some((n, immr, imms)) = bitmask_imm(u) {
                self.e(0xB200_03E0 | n << 22 | immr << 16 | imms << 10 | d as u32);
                return;
            }
        }
        if ones > zeros {
            // movn then movk the halves that are not 0xffff
            let mut first = true;
            for k in 0..4u32 {
                let h = ((u >> (16 * k)) & 0xffff) as u32;
                if h == 0xffff {
                    continue;
                }
                if first {
                    self.e(0x9280_0000 | k << 21 | ((!h) & 0xffff) << 5 | d as u32);
                    first = false;
                } else {
                    self.e(0xF280_0000 | k << 21 | h << 5 | d as u32);
                }
            }
            if first {
                // all ones
                self.e(0x9280_0000 | d as u32);
            }
        } else {
            let mut first = true;
            for k in 0..4u32 {
                let h = ((u >> (16 * k)) & 0xffff) as u32;
                if h == 0 {
                    continue;
                }
                if first {
                    self.e(0xD280_0000 | k << 21 | h << 5 | d as u32);
                    first = false;
                } else {
                    self.e(0xF280_0000 | k << 21 | h << 5 | d as u32);
                }
            }
            if first {
                self.e(0xD280_0000 | d as u32);
            }
        }
    }
    /// Always 4 instructions (patchable absolute address).
    fn mov_imm64_fixed(&mut self, d: u8, v: u64) {
        self.e(0xD280_0000 | ((v & 0xffff) as u32) << 5 | d as u32);
        for k in 1..4u32 {
            self.e(0xF280_0000 | k << 21 | (((v >> (16 * k)) & 0xffff) as u32) << 5 | d as u32);
        }
    }

    // ---- integer arithmetic (64-bit)
    fn rrr(&mut self, op: u32, d: u8, n: u8, m: u8) {
        self.e(op | (m as u32) << 16 | (n as u32) << 5 | d as u32);
    }
    fn add_imm(&mut self, d: u8, n: u8, imm: u32) {
        self.e(0x9100_0000 | imm << 10 | (n as u32) << 5 | d as u32);
    }
    fn sub_imm(&mut self, d: u8, n: u8, imm: u32) {
        self.e(0xD100_0000 | imm << 10 | (n as u32) << 5 | d as u32);
    }
    fn add_imm_lsl12(&mut self, d: u8, n: u8, imm: u32) {
        self.e(0x9140_0000 | imm << 10 | (n as u32) << 5 | d as u32);
    }
    fn sub_imm_lsl12(&mut self, d: u8, n: u8, imm: u32) {
        self.e(0xD140_0000 | imm << 10 | (n as u32) << 5 | d as u32);
    }
    /// d = n + v for any v (n and d may be sp); uses IP1 when v does not fit.
    fn add_any(&mut self, d: u8, n: u8, v: i64) {
        let (neg, a) = if v < 0 { (true, (-v) as u64) } else { (false, v as u64) };
        if a < (1 << 24) {
            let lo = (a & 0xfff) as u32;
            let hi = (a >> 12) as u32;
            let mut src = n;
            if hi != 0 {
                if neg {
                    self.sub_imm_lsl12(d, src, hi);
                } else {
                    self.add_imm_lsl12(d, src, hi);
                }
                src = d;
            }
            if lo != 0 || src != d {
                if neg {
                    self.sub_imm(d, src, lo);
                } else {
                    self.add_imm(d, src, lo);
                }
            }
        } else {
            self.mov_imm(IP1, v);
            // add xd, xn, x17, uxtx (extended form accepts sp)
            self.e(0x8B20_6000 | (IP1 as u32) << 16 | (n as u32) << 5 | d as u32);
        }
    }
    fn cmp(&mut self, n: u8, m: u8) {
        self.e(0xEB00_001F | (m as u32) << 16 | (n as u32) << 5);
    }
    fn cset(&mut self, d: u8, cond: u8) {
        self.e(0x9A9F_07E0 | ((cond ^ 1) as u32) << 12 | d as u32);
    }
    fn csel(&mut self, d: u8, n: u8, m: u8, cond: u8) {
        self.e(0x9A80_0000 | (m as u32) << 16 | (cond as u32) << 12 | (n as u32) << 5 | d as u32);
    }
    fn sbfm(&mut self, d: u8, n: u8, immr: u32, imms: u32) {
        self.e(0x9340_0000 | immr << 16 | imms << 10 | (n as u32) << 5 | d as u32);
    }
    fn ubfm(&mut self, d: u8, n: u8, immr: u32, imms: u32) {
        self.e(0xD340_0000 | immr << 16 | imms << 10 | (n as u32) << 5 | d as u32);
    }
    /// sign/zero extension of the low `bits` of n into d
    fn extend(&mut self, d: u8, n: u8, bits: u8, signed: bool) {
        if bits >= 64 {
            self.mov(d, n);
            return;
        }
        let imms = bits as u32 - 1;
        if signed {
            self.sbfm(d, n, 0, imms);
        } else {
            self.ubfm(d, n, 0, imms);
        }
    }

    // ---- memory
    /// Load/store with any offset. `op` is the unsigned-offset form, `size` the access size.
    fn ldst(&mut self, op: u32, size: u32, t: u8, n: u8, off: i64) {
        if off >= 0 && off % size as i64 == 0 && off / (size as i64) < 4096 {
            self.e(op | ((off / size as i64) as u32) << 10 | (n as u32) << 5 | t as u32);
        } else if (-256..256).contains(&off) {
            self.e((op & !0x0100_0000) | ((off as u32) & 0x1ff) << 12 | (n as u32) << 5 | t as u32);
        } else {
            self.mov_imm(IP1, off);
            self.e((op & !0x0100_0000) | 0x0020_6800 | (IP1 as u32) << 16 | (n as u32) << 5 | t as u32);
        }
    }
    fn stp_pre(&mut self, t1: u8, t2: u8, n: u8, off: i32) {
        self.e(0xA980_0000 | (((off / 8) as u32) & 0x7f) << 15 | (t2 as u32) << 10 | (n as u32) << 5 | t1 as u32);
    }
    fn ldp_post(&mut self, t1: u8, t2: u8, n: u8, off: i32) {
        self.e(0xA8C0_0000 | (((off / 8) as u32) & 0x7f) << 15 | (t2 as u32) << 10 | (n as u32) << 5 | t1 as u32);
    }
    fn stp(&mut self, fp: bool, t1: u8, t2: u8, n: u8, off: i32) {
        let op = if fp { 0x6D00_0000 } else { 0xA900_0000 };
        self.e(op | (((off / 8) as u32) & 0x7f) << 15 | (t2 as u32) << 10 | (n as u32) << 5 | t1 as u32);
    }
    fn ldp(&mut self, fp: bool, t1: u8, t2: u8, n: u8, off: i32) {
        let op = if fp { 0x6D40_0000 } else { 0xA940_0000 };
        self.e(op | (((off / 8) as u32) & 0x7f) << 15 | (t2 as u32) << 10 | (n as u32) << 5 | t1 as u32);
    }

    // ---- control flow
    fn blr(&mut self, n: u8) {
        self.e(0xD63F_0000 | (n as u32) << 5);
    }
    fn br(&mut self, n: u8) {
        self.e(0xD61F_0000 | (n as u32) << 5);
    }
    fn ret(&mut self) {
        self.e(0xD65F_03C0);
    }
    fn udf(&mut self) {
        self.e(0x0000_0000);
    }
    /// b (patched later); returns the word index
    fn b(&mut self) -> usize {
        self.e(0x1400_0000);
        self.pos() - 1
    }
    fn bcond(&mut self, cond: u8) -> usize {
        self.e(0x5400_0000 | cond as u32);
        self.pos() - 1
    }
    fn cbnz(&mut self, t: u8) -> usize {
        self.e(0xB500_0000 | t as u32);
        self.pos() - 1
    }
    fn cbz(&mut self, t: u8) -> usize {
        self.e(0xB400_0000 | t as u32);
        self.pos() - 1
    }
    fn tbz(&mut self, nz: bool, t: u8, bit: u32) -> usize {
        self.e(if nz { 0x3700_0000 } else { 0x3600_0000 } | (bit >> 5) << 31 | (bit & 31) << 19 | t as u32);
        self.pos() - 1
    }
    fn patch(&mut self, at: usize, target: usize) {
        let rel = target as i64 - at as i64;
        let w = self.w[at];
        // tbz / tbnz: imm14 (+-32 KB); out of range flags TBZ_RANGE (emitted again without them)
        if w & 0x7E00_0000 == 0x3600_0000 {
            if rel < -(1i64 << 13) || rel >= 1i64 << 13 {
                TBZ_RANGE.with(|c| c.set(true));
            }
            self.w[at] = (w & !(0x3FFF << 5)) | ((rel as u32) & 0x3FFF) << 5;
            return;
        }
        // b: imm26 (+-128 MB); b.cond / cbz / cbnz / ldr literal: imm19 (+-1 MB). An offset out
        // of range is never truncated: it flags the emission, which compile() repeats with
        // relaxed branches (or fails past imm26). tbz / tbnz (imm14) flag TBZ_RANGE instead
        // (emitted again with tst + b.cond); adr (imm21) is not patched.
        let is_b = w & 0xFC00_0000 == 0x1400_0000;
        let is_imm19 = w & 0xFF00_0010 == 0x5400_0000 || w & 0x7E00_0000 == 0x3400_0000 || w & 0x3B00_0000 == 0x1800_0000;
        assert!(is_b || is_imm19, "arm64 patch of an unknown encoding {:08x}", w);
        let lim = if w & 0xFC00_0000 == 0x1400_0000 { 1i64 << 25 } else { 1i64 << 18 };
        if rel < -lim || rel >= lim {
            BRANCH_RANGE.with(|c| c.set(true));
        }
        if w & 0xFC00_0000 == 0x1400_0000 {
            self.w[at] = 0x1400_0000 | (rel as u32 & 0x03FF_FFFF);
        } else {
            // b.cond / cbz / cbnz / ldr literal: imm19 at bit 5
            self.w[at] = (w & !(0x7FFFF << 5)) | ((rel as u32) & 0x7FFFF) << 5;
        }
    }

    // ---- float
    fn fp2(&mut self, op: u32, dbl: bool, d: u8, n: u8) {
        self.e(op | if dbl { DBL } else { 0 } | (n as u32) << 5 | d as u32);
    }
    fn fp3(&mut self, op: u32, dbl: bool, d: u8, n: u8, m: u8) {
        self.e(op | if dbl { DBL } else { 0 } | (m as u32) << 16 | (n as u32) << 5 | d as u32);
    }
    fn fmov(&mut self, d: u8, n: u8) {
        if d != n {
            // full 64-bit move (holds either precision)
            self.fp2(FMOVR, true, d, n);
        }
    }
    fn fcmp(&mut self, dbl: bool, n: u8, m: u8) {
        self.e(0x1E20_2000 | if dbl { DBL } else { 0 } | (m as u32) << 16 | (n as u32) << 5);
    }
    /// fmov dd, xn / sd, wn
    fn fmov_from_gp(&mut self, dbl: bool, d: u8, n: u8) {
        self.e(if dbl { 0x9E67_0000 } else { 0x1E27_0000 } | (n as u32) << 5 | d as u32);
    }
    /// fmov xd, dn / wd, sn
    fn fmov_to_gp(&mut self, dbl: bool, d: u8, n: u8) {
        self.e(if dbl { 0x9E66_0000 } else { 0x1E26_0000 } | (n as u32) << 5 | d as u32);
    }
}

fn icc(c: Cond, signed: bool) -> u8 {
    match (c, signed) {
        (Cond::Eq, _) => EQ,
        (Cond::Ne, _) => NE,
        (Cond::Lt, true) => LT,
        (Cond::Le, true) => LE,
        (Cond::Gt, true) => GT,
        (Cond::Ge, true) => GE,
        (Cond::Lt, false) => LO,
        (Cond::Le, false) => LS,
        (Cond::Gt, false) => HI,
        (Cond::Ge, false) => HS,
    }
}

/// Float compare conditions after `fcmp a, b`; all false on unordered except Ne.
fn fcc(c: Cond) -> u8 {
    match c {
        Cond::Eq => EQ,
        Cond::Ne => NE,
        Cond::Lt => MI,
        Cond::Le => LS,
        Cond::Gt => GT,
        Cond::Ge => GE,
    }
}

/// The 8-bit FMOV immediate for a float, when it has one: +-(16..31)/16 * 2^(-3..4).
fn fp_imm8(bits: u64, f64_: bool) -> Option<u32> {
    let (sign, exp, frac, ebias, fbits) = if f64_ {
        ((bits >> 63) as u32, ((bits >> 52) & 0x7ff) as i32, bits & ((1u64 << 52) - 1), 1023, 52)
    } else {
        (((bits >> 31) & 1) as u32, ((bits >> 23) & 0xff) as i32, bits & ((1u64 << 23) - 1), 127, 23)
    };
    // only the top 4 fraction bits may be set
    if frac & ((1u64 << (fbits - 4)) - 1) != 0 {
        return None;
    }
    let e = exp - ebias;
    if !(-3..=4).contains(&e) {
        return None;
    }
    // imm8 = a:b:cd:efgh with exponent = NOT(b):cc..c:de (3 bits used: b, cd)
    let e3 = ((e + 3) as u32) ^ 0b100; // b = NOT(top bit) encoding of e in -3..4
    let top4 = (frac >> (fbits - 4)) as u32;
    Some(sign << 7 | e3 << 4 | top4)
}

/// One-instruction encodings of a 4 x 32-bit splat (without Rd): movi / mvni with a
/// shifted byte, or fmov with a float immediate.
fn vimm(bits: u32) -> Option<u32> {
    let enc = |op: u32, v: u32| -> Option<u32> {
        (0..4u32).find(|k| v & !(0xFF << (8 * k)) == 0).map(|k| {
            let imm8 = (v >> (8 * k)) & 0xFF;
            op | (imm8 >> 5) << 16 | (k * 2) << 12 | (imm8 & 31) << 5
        })
    };
    if let Some(w) = enc(0x4F00_0400, bits) {
        return Some(w);
    }
    if let Some(w) = enc(0x6F00_0400, !bits) {
        return Some(w);
    }
    fp_imm8(bits as u64, false).map(|imm8| 0x4F00_F400 | (imm8 >> 5) << 16 | (imm8 & 31) << 5)
}

/// Does an immediate-operand instruction encode without materialising its constant (see ibin_imm,
/// cmp_imm)?
fn imm_fits(i: &Inst) -> bool {
    match *i {
        Inst::ICmpI(_, _, _, _, v) => (-4095..4096).contains(&v),
        Inst::IBinI(op, it, _, _, v) => match op {
            IOp::Add | IOp::Sub => v.unsigned_abs() < (1 << 24),
            IOp::Shl | IOp::Shr => true,
            IOp::Mul => v > 0 && ((v & (v - 1)) == 0 || mul_shift_add(v).is_some()),
            IOp::And | IOp::Or | IOp::Xor => {
                (op == IOp::And && (v == 0xff || v == 0xffff || v == 0xffff_ffff)) || ((it.bits == 64 || (it == U32 && (0..1i64 << 32).contains(&v))) && bitmask_imm(v as u64).is_some())
            }
            _ => false,
        },
        _ => true,
    }
}

/// x * v as one shifted add or subtract: v = 2^k + 1 -> (k, true) `add d, x, x, lsl k`;
/// v = 2^k - 1 -> (k, false) `lsl t, x, k; sub d, t, x` (k >= 2; 1 and powers of two are shifts).
fn mul_shift_add(v: i64) -> Option<(u32, bool)> {
    if v > 2 && ((v - 1) & (v - 2)) == 0 {
        return Some(((v - 1).trailing_zeros(), true));
    }
    if v > 2 && v < i64::MAX && ((v + 1) & v) == 0 {
        return Some(((v + 1).trailing_zeros(), false));
    }
    None
}

/// The (N, immr, imms) fields of a logical-instruction immediate for a 64-bit value: a repeating
/// element (2..64 bits) that is a rotated run of ones. None for 0, all ones and other values.
fn bitmask_imm(v: u64) -> Option<(u32, u32, u32)> {
    if v == 0 || v == !0 {
        return None;
    }
    let mut e = 64u32;
    // smallest period
    while e > 2 {
        let h = e / 2;
        let m = (1u64 << h) - 1;
        if (v & m) != ((v >> h) & m) {
            break;
        }
        e = h;
    }
    let mask = if e == 64 { !0u64 } else { (1u64 << e) - 1 };
    let x = v & mask;
    let c = x.count_ones();
    if c == 0 || c == e {
        return None;
    }
    let ones = (1u64 << c) - 1;
    let ror = |y: u64, r: u32| -> u64 {
        if r == 0 {
            y
        } else {
            ((y >> r) | (y << (e - r))) & mask
        }
    };
    for immr in 0..e {
        if ror(ones, immr) == x {
            let n = if e == 64 { 1 } else { 0 };
            let imms = ((((-(e as i32)) << 1) & 0x3f) as u32) | (c - 1);
            return Some((n, immr, imms));
        }
    }
    None
}

fn mem_op(m: Mem, load: bool) -> (u32, u32) {
    match (m, load) {
        (Mem::Int(1, s), true) => (if s { LDRSB } else { LDRB }, 1),
        (Mem::Int(2, s), true) => (if s { LDRSH } else { LDRH }, 2),
        (Mem::Int(4, s), true) => (if s { LDRSW } else { LDRW }, 4),
        (Mem::Int(_, _), true) => (LDRX, 8),
        (Mem::Int(1, _), false) => (STRB, 1),
        (Mem::Int(2, _), false) => (STRH, 2),
        (Mem::Int(4, _), false) => (STRW, 4),
        (Mem::Int(_, _), false) => (STRX, 8),
        (Mem::F32, true) => (LDRS, 4),
        (Mem::F32, false) => (STRS, 4),
        (Mem::F64, true) => (LDRD, 8),
        (Mem::F64, false) => (STRD, 8),
    }
}

// ------------------------------------------------------------ code generation

#[derive(Clone, Copy)]
enum CondBr {
    B(u8),
    Z(u8),
    Nz(u8),
    /// tbz / tbnz register, bit (only when `Gen::tbz`: +-32 KB)
    Tz(u8, u32),
    Tnz(u8, u32),
}

struct Gen<'a> {
    /// isel_flags(options) & A64_FCONST: float constants by fmov #imm / literal loads
    fconst_isel: bool,
    qcopy: bool,
    /// the block ends `t = x & K; t ==/!= 0; branch`: emitted as tst x, #K (the And skipped)
    test_fuse: Option<(VReg, i64)>,
    /// a compare whose flags the selects right after it read (csel / fcsel on the condition, no
    /// `cmp c, #0`): (its bool vreg, condition). Valid only for the selects that directly follow
    /// the compare in its block: any other instruction, a fused compare-and-branch, or a block
    /// boundary ends it (the flags then belong to something else).
    sel_chain: Option<(VReg, u8)>,
    /// a zero vector whose definition was skipped: the VInsert of lane 0 right after it reads
    /// it (fmov s, which zeroes lanes 1-3)
    zero_ins: Option<VReg>,
    /// a skipped multiply's operands: the negation right after it is `fnmul`
    fnmul_ins: Option<(VReg, VReg)>,
    /// vregs defined once, as a zero vector (VConst 0 / FromLo64 of a constant 0)
    zero_vec: Vec<bool>,
    /// single-definition integer / float / vector zeros (Iconst 0, Fconst +0.0, VConst 0, a zero
    /// vector from FromLo64 of an integer 0): stores of them store zr
    zero_val: Vec<bool>,
    /// vregs defined once, by an f32 load (a register holding one has its upper 96 bits zero)
    f32_load: Vec<bool>,
    /// single-def +0.0 constants read only by float compares: never built (fcmp #0.0)
    zero_cmp: Vec<bool>,
    /// the last spill store emitted: (sp offset, register, word position right after it)
    last_spill: Option<(i64, u8, usize)>,
    /// spill windows (Alloc::windows): vreg -> (first use pos, last use pos, register); and the
    /// loads to emit before the instruction at a position
    win: std::collections::HashMap<u32, Vec<(u32, u32, u8)>>,
    win_start: std::collections::HashMap<u32, Vec<(u32, u8)>>,
    /// block-local evictions (Alloc::evicts / prestores): transfers before the instruction at a position
    evict_at: std::collections::HashMap<u32, Vec<regalloc::EvOp>>,
    /// passes::low_bits_only: narrow results whose re-extension nobody reads
    low_only: Vec<bool>,
    /// passes::zero_above: values whose register is zero above that many bits
    zero_above: Vec<u8>,
    /// switch::switch_in_range: per block, its Switch needs no range test
    switch_in_range: Vec<bool>,
    /// regalloc::copy_reuse: (use position, spilled vreg) -> the register of a copy holding it
    copy_reg: std::collections::HashMap<(u32, u32), u8>,
    /// table words to fill once blocks are placed: (word index, table start, target block)
    jt_fix: Vec<(usize, usize, u32)>,
    a: Asm,
    f: &'a Func,
    al: Alloc,
    /// total frame size below fp (sp = fp - frame)
    frame: i64,
    /// per block: the index of its sibling call (tail::sibling_calls; empty: none)
    tail: Vec<Option<usize>>,
    /// bytes of callee-saved registers below fp
    saved: i64,
    slot_off: Vec<i64>,
    /// per value: its frame position when it is a slot address (frame::slot_addr_positions)
    slot_pos: Vec<i64>,
    block_pos: Vec<usize>,
    next_block: Vec<u32>,
    /// the function carries PGO block counts (the direction of two-way branches follows them)
    counted: bool,
    fixups: Vec<(usize, u32)>,
    /// float literal pool: (ldr word index, bits)
    fpool: Vec<(usize, u64)>,
    /// vector literal pool: (ldr q word index, lane bits splatted to 4 lanes)
    vpool: Vec<(usize, u32)>,
    /// address pool: (ldr x word index, relocated value)
    apool: Vec<(usize, Reloc)>,
    /// asm! b / bl to symbols: (word index, symbol), sent through veneers after the text
    asm_veneers: Vec<(usize, Sym)>,
    /// `Env::sym_class`: how each symbol is encoded (SymClass::Domain: an offset in the domain block)
    sym_class: u64,
    /// `Env::near`: host and runtime calls are `bl` (the embedder places veneers)
    near: bool,
    /// sp offset of the frame word holding the domain base (-1: the function names no domain symbol)
    dom_slot: i64,
    /// per block: whether its start reads the domain base into that slot (block 0: the prologue;
    /// empty: the function names no domain symbol)
    dom_read: Vec<bool>,
    relocs: Vec<RelocAt>,
    text_words: usize,
    pc_map: Vec<(u32, u64)>,
    cur_pos: u32,
    /// C ABI body: its plan, per-argument buffer (sp offset, -1 = none), result buffer
    cplan: Option<Plan>,
    cbuf: Vec<i64>,
    cret_buf: i64,
    /// sp offset of the C call staging cells (8 bytes each)
    stage: i64,
    /// word index right after the last call's branch-and-link (its return address)
    last_call_ret: usize,
    /// invoked calls: (return word index, caller-saved values to reload, cleanup pad block)
    pads: Vec<(usize, Vec<(VReg, i32)>, u32)>,
    /// (instruction position, return offset in bytes) of every call: the variable table's sites (dbgloc)
    var_calls: Vec<(u32, u32)>,
    rir_map: Vec<(u32, u32, u32)>,
    rir_map_on: bool,
    /// branch relaxation (a function past imm19's +-1 MB): conditional block branches become
    /// an inverted short branch over a `b` (+-128 MB), constants are built in registers
    /// instead of loaded from the literal pools after the code
    long: bool,
    /// single-bit tests branch with tbz / tbnz (off when one of them did not reach: emitted again)
    tbz: bool,
}

impl<'a> Gen<'a> {
    /// sp offset of spill slot `o` (Loc::Stack(o))
    fn spill_sp(&self, o: i32) -> i64 {
        self.frame - (self.saved + o as i64)
    }

    fn cls(&self, v: VReg) -> Cls {
        self.f.vregs[v.0 as usize]
    }

    fn use_i(&mut self, v: VReg, scratch: u8) -> u8 {
        match self.al.loc[v.0 as usize] {
            Loc::Reg(r) => r,
            Loc::Stack(o) => {
                // held in a register over this run of uses (Alloc::windows)
                if let Some(ws) = self.win.get(&v.0) {
                    let p = self.cur_pos;
                    if let Some(&(_, _, r)) = ws.iter().find(|(a, z, _)| *a <= p && p <= *z) {
                        return r;
                    }
                }
                if let Some(&r) = self.copy_reg.get(&(self.cur_pos, v.0)) {
                    return r;
                }
                let off = self.spill_sp(o);
                // stored from a register by the very last instruction emitted (same block)
                if let Some((d, r, p)) = self.last_spill {
                    if d == off && p == self.a.pos() {
                        if r != scratch {
                            self.a.mov(scratch, r);
                        }
                        return scratch;
                    }
                }
                self.a.ldst(LDRX, 8, scratch, SP, off);
                scratch
            }
            // recomputed at each use (Alloc::remat)
            _ => match self.al.remat[v.0 as usize] {
                Remat::No => ZR,
                _ if self.win_reg(v).is_some() => self.win_reg(v).unwrap(),
                Remat::Offset(b, k) => {
                    let (b, o) = self.resolve_off(b, k as i64);
                    let (rb, o) = match self.slot_base(b) {
                        Some(so) => (SP, so + o),
                        None => (self.use_i(b, scratch), o),
                    };
                    self.a.add_any(scratch, rb, o);
                    scratch
                }
                _ => {
                    self.remat_into(scratch, v);
                    scratch
                }
            },
        }
    }
    /// The register of a window (Alloc::windows) holding v at the current position.
    fn win_reg(&self, v: VReg) -> Option<u8> {
        let p = self.cur_pos;
        self.win.get(&v.0)?.iter().find(|(a, z, _)| *a <= p && p <= *z).map(|w| w.2)
    }
    /// A store's integer value: a recomputed 0 outside a window is xzr (register 31 is xzr as
    /// a store's source).
    fn use_stored(&mut self, v: VReg, scratch: u8) -> u8 {
        match (self.al.loc[v.0 as usize], self.al.remat[v.0 as usize]) {
            (Loc::None, Remat::Const(0)) if self.win_reg(v).is_none() => ZR,
            _ => self.use_i(v, scratch),
        }
    }
    /// A memory base and offset with base + k values that have no register (Alloc::remat)
    /// folded: their base and the summed offset.
    fn resolve_off(&self, mut base: VReg, mut off: i64) -> (VReg, i64) {
        while let (Loc::None, Remat::Offset(b, k)) = (self.al.loc[base.0 as usize], self.al.remat[base.0 as usize]) {
            if off + k as i64 != (off + k as i64) as i32 as i64 {
                break;
            }
            base = b;
            off += k as i64;
        }
        (base, off)
    }
    /// Eviction stores / reloads before an instruction (regalloc::evict_ops); flags untouched.
    fn evict_transfers(&mut self, ops: &[regalloc::EvOp]) {
        for op in ops {
            match *op {
                regalloc::EvOp::Store(r, o) => {
                    let off = self.spill_sp(o);
                    self.a.ldst(STRX, 8, r, SP, off);
                }
                regalloc::EvOp::Load(r, o) => {
                    let off = self.spill_sp(o);
                    self.a.ldst(LDRX, 8, r, SP, off);
                }
                regalloc::EvOp::Remat(r, k) => self.remat_kind_into(r, k),
            }
        }
    }
    /// Recomputes a value without a location (Alloc::remat) into r.
    fn remat_into(&mut self, r: u8, v: VReg) {
        self.remat_kind_into(r, self.al.remat[v.0 as usize]);
    }
    /// remat_into for a recomputation kind (also an evicted constant's reload, Alloc::evicts).
    fn remat_kind_into(&mut self, r: u8, k: Remat) {
        match k {
            // never xzr for 0: register 31 is sp in add-immediate and address operands
            Remat::Const(k) => self.a.mov_imm(r, k),
            Remat::Slot(s) => {
                let off = self.slot_off[s as usize];
                self.a.add_any(r, SP, off);
            }
            Remat::Addr(a) => self.ldr_reloc(r, Reloc::Sym(a)),
            Remat::FnAddr(id) => self.ldr_reloc(r, Reloc::Thunk(id)),
            Remat::Offset(..) => unreachable!("base + k moves through use_i"),
            Remat::VConst(_) => unreachable!("a vector constant is made by use_f"),
            Remat::FConst(..) => unreachable!("a float constant is made by use_f / fconst_to"),
            Remat::No => self.a.mov_imm(r, 0),
        }
    }
    /// A recomputed float constant (Remat::FConst) into x.
    fn fconst_to(&mut self, x: u8, v: VReg) {
        match self.al.remat[v.0 as usize] {
            Remat::FConst(bits, f64_) => self.fconst_into(x, bits, f64_),
            r => unreachable!("float move from {:?}", r),
        }
    }
    /// A stack-slot address without a register (Alloc::remat) as a load/store base: sp + its offset.
    fn slot_base(&self, base: VReg) -> Option<i64> {
        match (self.al.loc[base.0 as usize], self.al.remat[base.0 as usize]) {
            (Loc::None, Remat::Slot(s)) => Some(self.slot_off[s as usize]),
            _ => None,
        }
    }
    /// [base + (idx << sh) + off] for an indexed access: the register-offset form when the shift
    /// is 0 or the access size and nothing is added, else ip1 = the address and the offset.
    fn index_addr(&mut self, base: VReg, ri: u8, sh: u8, off: i32, size: u32) -> (u8, Option<(u8, bool)>, i64) {
        let (base, off) = match self.resolve_off(base, off as i64) {
            (b, o) if o == o as i32 as i64 => (b, o as i32),
            _ => (base, off),
        };
        let scaled = sh as u32 == size.trailing_zeros();
        match self.slot_base(base) {
            Some(so) => {
                let off = so + off as i64;
                if off == 0 && (sh == 0 || scaled) {
                    return (SP, Some((ri, sh != 0)), 0);
                }
                // ADD (shifted register) reads register 31 as xzr: sp goes through add_any first
                self.a.add_any(IP1, SP, off);
                if sh == 0 || scaled {
                    return (IP1, Some((ri, sh != 0)), 0);
                }
                self.a.e(0x8B00_0000 | ((sh as u32) << 10) | ((ri as u32) << 16) | ((IP1 as u32) << 5) | IP1 as u32);
                (IP1, None, 0)
            }
            None => {
                let rb = self.use_i(base, S1);
                if off == 0 && (sh == 0 || scaled) {
                    return (rb, Some((ri, sh != 0)), 0);
                }
                self.a.e(0x8B00_0000 | ((sh as u32) << 10) | ((ri as u32) << 16) | ((rb as u32) << 5) | IP1 as u32);
                // ldst's large-offset form needs ip1 itself: fold big offsets in first
                if (off as i64).abs() >= 256 {
                    self.a.add_any(IP1, IP1, off as i64);
                    return (IP1, None, 0);
                }
                (IP1, None, off as i64)
            }
        }
    }
    /// One load/store at [rn + (rm << (scaled ? size : 0))] (register-offset form) or [rn + off].
    fn ldst_x(&mut self, op: u32, size: u32, t: u8, rn: u8, rm: Option<(u8, bool)>, off: i64) {
        match rm {
            // LDR/STR (register), option LSL (011), S = scaled
            Some((rm, scaled)) => self.a.e((op & !0x0100_0000) | 0x0020_6800 | if scaled { 0x1000 } else { 0 } | (rm as u32) << 16 | (rn as u32) << 5 | t as u32),
            None => self.a.ldst(op, size, t, rn, off),
        }
    }
    fn def_i(&self, v: VReg, scratch: u8) -> u8 {
        match self.al.loc[v.0 as usize] {
            Loc::Reg(r) => r,
            // a window that starts at this definition (regalloc def windows): computed into its
            // register, which keeps the value after fin_i stores it home
            Loc::Stack(_) => self.win.get(&v.0).and_then(|ws| ws.iter().find(|w| w.0 == self.cur_pos + 1)).map_or(scratch, |w| w.2),
            _ => scratch,
        }
    }
    fn fin_i(&mut self, v: VReg, r: u8) {
        if let Loc::Stack(o) = self.al.loc[v.0 as usize] {
            // nobody reads this store home (Alloc::store_skip): the window's register only
            if self.al.store_skip.contains(&(self.cur_pos, v.0)) {
                self.def_win_copy(v, r);
                return;
            }
            let off = self.spill_sp(o);
            self.a.ldst(STRX, 8, r, SP, off);
            self.last_spill = Some((off, r, self.a.pos()));
            self.def_win_copy(v, r);
        }
    }
    /// A window that starts at this definition (def windows, regions over redefining blocks)
    /// holds the result: computed elsewhere (a scratch), it is copied there.
    fn def_win_copy(&mut self, v: VReg, r: u8) {
        if let Some(c) = self.win.get(&v.0).and_then(|ws| ws.iter().find(|w| w.0 == self.cur_pos + 1)).map(|w| w.2) {
            if c != r {
                self.a.mov(c, r);
            }
        }
    }
    fn use_f(&mut self, v: VReg, scratch: u8) -> u8 {
        if !matches!(self.al.loc[v.0 as usize], Loc::Flt(_)) {
            // held in a register over this run of uses (float windows)
            if let Some(r) = self.win_reg(v) {
                return r;
            }
        }
        match self.al.loc[v.0 as usize] {
            Loc::Flt(x) => x,
            Loc::Stack(o) if self.cls(v) == Cls::V128 => {
                let off = self.spill_sp(o);
                self.vldst(true, scratch, SP, off);
                scratch
            }
            Loc::Stack(o) => {
                let off = self.spill_sp(o);
                self.a.ldst(LDRD, 8, scratch, SP, off);
                scratch
            }
            // recomputed at each use (Alloc::remat)
            Loc::None => {
                match self.al.remat[v.0 as usize] {
                    Remat::VConst(bits) => self.vconst_into(scratch, bits),
                    Remat::FConst(bits, f64_) => self.fconst_into(scratch, bits, f64_),
                    _ => {}
                }
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
            let off = self.spill_sp(o);
            if self.cls(v) == Cls::V128 {
                self.vldst(false, x, SP, off);
                return;
            }
            self.a.ldst(STRD, 8, x, SP, off);
        }
    }

    fn mloc(&self, v: VReg) -> MLoc {
        match self.al.loc[v.0 as usize] {
            Loc::Reg(r) => MLoc::R(r),
            Loc::Flt(x) => MLoc::F(x),
            Loc::Stack(o) => MLoc::M(self.spill_sp(o) as i32),
            Loc::None if self.al.remat[v.0 as usize].is_some() => MLoc::K(v.0),
            Loc::None => MLoc::R(ZR),
        }
    }

    /// Emits a sequentialized parallel move. Memory locations are sp offsets of
    /// 8-byte cells; floats are moved as full 64-bit registers.
    fn moves(&mut self, mv: &[(MLoc, MLoc)]) {
        let seq = regalloc::seq_moves(mv, S0, FS0);
        for (d, s) in seq {
            match (d, s) {
                (MLoc::R(d), MLoc::R(s)) => self.a.mov(d, s),
                (MLoc::F(d), MLoc::F(s)) => self.a.fmov(d, s),
                (MLoc::R(d), MLoc::M(o)) => self.a.ldst(LDRX, 8, d, SP, o as i64),
                (MLoc::F(d), MLoc::M(o)) => self.a.ldst(LDRD, 8, d, SP, o as i64),
                (MLoc::M(o), MLoc::R(s)) => self.a.ldst(STRX, 8, s, SP, o as i64),
                (MLoc::M(o), MLoc::F(s)) => self.a.ldst(STRD, 8, s, SP, o as i64),
                (MLoc::M(o), MLoc::M(p)) => {
                    self.a.ldst(LDRX, 8, S1, SP, p as i64);
                    self.a.ldst(STRX, 8, S1, SP, o as i64);
                }
                (MLoc::R(d), MLoc::F(s)) => self.a.fmov_to_gp(true, d, s),
                (MLoc::F(d), MLoc::R(s)) => self.a.fmov_from_gp(true, d, s),
                (MLoc::F(d), MLoc::K(v)) => self.fconst_to(d, VReg(v)),
                (MLoc::M(o), MLoc::K(v)) if self.cls(VReg(v)) != Cls::I => {
                    self.fconst_to(FS1, VReg(v));
                    self.a.ldst(STRD, 8, FS1, SP, o as i64);
                }
                (MLoc::R(d), MLoc::K(v)) => self.remat_into(d, VReg(v)),
                (MLoc::M(o), MLoc::K(v)) => {
                    self.remat_into(S1, VReg(v));
                    self.a.ldst(STRX, 8, S1, SP, o as i64);
                }
                (d @ MLoc::K(_), s) => unreachable!("move {:?} <- {:?}", d, s),
            }
        }
    }

    fn gen(&mut self, options: &relax_session::Options) {
        // frame layout
        let n_saved = self.al.used_callee_int.len() + self.al.used_callee_flt.len();
        self.saved = 8 * n_saved as i64;
        // only slots something addresses take space (SROA and inlining leave many unreferenced:
        // the Splash VM's O3 opcode() had 1749 slots, 515 in use); slots whose lifetimes cannot
        // overlap share it (frame::slot_layout)
        let (slot_fp, mut off) = crate::frame::slot_layout_aligned(self.f, self.saved + self.al.spill_bytes as i64, !crate::stats::opt_off(options, "-slotcolor"), if crate::stats::opt_off(options, "-a64slotalign") { |_| 1 } else { a64_slot_align });
        self.slot_pos = crate::frame::slot_addr_positions(self.f, &slot_fp);
        // C ABI body: buffers that rebuild aggregate params from their registers, and the
        // buffer an aggregate result is written to before Ret loads it into registers
        let mut cbuf_fp: Vec<i64> = Vec::new();
        let mut cret_fp = 0i64;
        if let Some(sig) = &self.f.cabi {
            let plan = cabi::plan(sig, sig.conv.target());
            for (k, a) in plan.args.iter().enumerate() {
                match (a, &sig.args[k]) {
                    (ArgPlan::Parts(_), CArg::Agg(ag)) => {
                        off += ((ag.size as i64 + 7) & !7) + 8;
                        cbuf_fp.push(off);
                    }
                    _ => cbuf_fp.push(-1),
                }
            }
            if let (RetPlan::Parts(_), Some(ag)) = (&plan.ret, &sig.ret) {
                off += ((ag.size as i64 + 7) & !7) + 8;
                cret_fp = off;
            }
            self.cplan = Some(plan);
        }
        // outgoing stack-argument area at the bottom of the frame ([sp, sp + out)) and
        // staging cells for C calls (aggregate pieces, the result address)
        let mut out = 0i64;
        let mut cells = 0i64;
        for b in &self.f.blocks {
            for i in &b.insts {
                if let Inst::Call(c, a, _) = i {
                    match c {
                        Callee::CHost(_, sig) | Callee::CIndirect(_, sig) | Callee::CFn(_, sig) => {
                            let plan = cabi::plan(sig, sig.conv.target());
                            out = out.max(plan.stack_bytes as i64);
                            let mut n = 0i64;
                            for ap in &plan.args {
                                if let ArgPlan::Parts(ps) = ap {
                                    n += ps.len() as i64;
                                }
                            }
                            if let RetPlan::Parts(ps) = &plan.ret {
                                n = n.max(ps.len() as i64);
                            }
                            cells = cells.max(1 + n);
                        }
                        _ => {
                            let (ni, nf) = count_classes(self.f, a);
                            let over = ni.saturating_sub(INT_ARGS) + nf.saturating_sub(FLT_ARGS);
                            out = out.max(8 * over as i64);
                        }
                    }
                }
            }
        }
        off += 8 * cells;
        let stage_fp = off;
        // the domain base, read once (where regalloc::domain_base_block puts it) when the function
        // names a domain static
        let reach = regalloc::reachable(self.f);
        let reads = regalloc::domain_base_blocks(self.f, self.sym_class, &reach);
        let dom_fp = if !reads.is_empty() {
            self.dom_read = vec![false; self.f.blocks.len()];
            for b in reads {
                self.dom_read[b] = true;
            }
            off += 8;
            off
        } else {
            -1
        };
        let frame = (off + out + 15) / 16 * 16;
        self.frame = frame;
        if dom_fp >= 0 {
            self.dom_slot = frame - dom_fp;
        }
        for x in slot_fp {
            self.slot_off.push(frame - x);
        }
        for x in cbuf_fp {
            self.cbuf.push(if x < 0 { -1 } else { frame - x });
        }
        self.cret_buf = frame - cret_fp;
        self.stage = frame - stage_fp;

        // prologue (a naked fn is its asm block only)
        if !self.f.naked {
            self.a.stp_pre(FP, LR, SP, -16);
            self.a.mov_sp(FP, SP);
            self.stack_probe(frame);
            self.a.add_any(SP, SP, -frame);
            self.callee_saves(true);
            self.tail = crate::tail::sibling_calls(options, self.f, &reach, crate::tail::Arch::Arm64);
            if self.dom_read.first() == Some(&true) {
                self.dom_base();
            }
            if self.cplan.is_some() {
                self.c_prologue();
            } else {
                self.params_prologue();
            }
        }

        let nb = self.f.blocks.len();
        self.block_pos = vec![0; nb];
        self.next_block = vec![u32::MAX; nb];
        self.counted = crate::profile::has_counts(self.f);
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
            self.block_pos[bi] = self.a.pos();
            if !reach[bi] {
                continue;
            }
            if bi != 0 && self.dom_read.get(bi) == Some(&true) {
                self.dom_base();
            }
            let b = &self.f.blocks[bi];
            let n = b.insts.len();
            // a compare feeding only the branch is fused into cmp + b.cond
            let mut fused = false;
            if let Term::Branch(c, _, _) = &b.term {
                if n > 0 && self.al.uses[c.0 as usize] == 1 && isel_flags(options) & A64_FUSE != 0 {
                    match &b.insts[n - 1] {
                        Inst::ICmp(_, _, d, _, _) | Inst::ICmpI(_, _, d, _, _) | Inst::FCmp(_, _, d, _, _) if d == c => fused = true,
                        _ => {}
                    }
                }
            }
            // `t = x & K; c = t ==/!= 0` feeding the branch (t used once, K a bitmask immediate): tst x, #K
            self.test_fuse = None;
            let mut test_pair = false;
            if fused && n >= 2 {
                if let (Inst::IBinI(IOp::And, it, t, x, k), Inst::ICmpI(Cond::Eq | Cond::Ne, _, _, t2, 0)) = (&b.insts[n - 2], &b.insts[n - 1]) {
                    if t == t2 && *x != *t && self.al.uses[t.0 as usize] == 1 && (it.bits == 64 || (0..1i64 << 32).contains(k)) && bitmask_imm(*k as u64).is_some() && arm64_defers(b, n - 2, &self.al.uses) {
                        self.test_fuse = Some((*x, *k));
                        test_pair = true;
                    }
                }
            }
            // `c = x & K` branched on directly (c used once): tst x, #K / tbnz (switch bit tests:
            // `lsr; and #1; cbnz` was three instructions)
            if let Term::Branch(c, _, _) = &b.term {
                if !fused && n > 0 && self.al.uses[c.0 as usize] == 1 && isel_flags(options) & A64_FUSE != 0 {
                    if let Inst::IBinI(IOp::And, it, t, x, k) = &b.insts[n - 1] {
                        if t == c && *x != *t && (it.bits == 64 || (0..1i64 << 32).contains(k)) && bitmask_imm(*k as u64).is_some() {
                            self.test_fuse = Some((*x, *k));
                            fused = true;
                        }
                    }
                }
            }
            let mut skip_next = false;
            // instructions before this were stored by one zero run (zero_run)
            let mut skip_to = 0usize;
            let mut deferred: Option<usize> = None;
            // a block start can be a jump target: no spill store before it carries over
            self.last_spill = None;
            // nor flags: the previous block's terminator (a fused cmp + b.cond) or another
            // predecessor set them, not the compare a select chain was started for
            self.sel_chain = None;
            // window lookups of this block never see the previous block's last position (an empty
            // block's terminator reads at its start: no window there, the slot is read)
            self.cur_pos = self.al.block_start[bi];
            for (ii, inst) in b.insts.iter().enumerate() {
                self.pc_map.push(((self.a.pos() * 4) as u32, b.pos[ii]));
                if self.rir_map_on {
                    self.rir_map.push(((self.a.pos() * 4) as u32, bi as u32, ii as u32));
                }
                self.cur_pos = self.al.inst_pos(bi, ii);
                if let Some(ops) = self.evict_at.get(&self.cur_pos).cloned() {
                    self.evict_transfers(&ops);
                }
                if let Some(ls) = self.win_start.get(&self.cur_pos).cloned() {
                    for (v, r) in ls {
                        match (self.cls(VReg(v)), self.al.loc[v as usize]) {
                            (Cls::I, Loc::Stack(o)) => {
                                let off = self.spill_sp(o);
                                self.a.ldst(LDRX, 8, r, SP, off);
                            }
                            (Cls::I, _) => self.remat_into(r, VReg(v)),
                            (Cls::V128, Loc::Stack(o)) => {
                                let off = self.spill_sp(o);
                                self.vldst(true, r, SP, off);
                            }
                            (_, Loc::Stack(o)) => {
                                let off = self.spill_sp(o);
                                self.a.ldst(LDRD, 8, r, SP, off);
                            }
                            (_, _) => match self.al.remat[v as usize] {
                                Remat::VConst(bits) => self.vconst_into(r, bits),
                                Remat::FConst(bits, f64_) => self.fconst_into(r, bits, f64_),
                                m => unreachable!("float window of {:?}", m),
                            },
                        }
                    }
                }
                // only selects continue a chain; checked before any instruction is skipped or
                // fused into the terminator (a fused compare rewrites the flags)
                if !matches!(inst, Inst::Sel(..)) {
                    self.sel_chain = None;
                }
                if fused && ii == n - 1 {
                    break;
                }
                if test_pair && ii == n - 2 {
                    continue;
                }
                if skip_next {
                    skip_next = false;
                    continue;
                }
                if ii < skip_to {
                    continue;
                }
                // the second instruction of a deferred pair: both are emitted here, at its position
                // (after its window loads): its result's def window starts after it, its store_skip
                // is keyed by it, and the register allocator keeps the first one's operands
                // readable at the next instruction (deferred reads)
                if let Some(k) = deferred.take() {
                    let first = &b.insts[k];
                    let done = self.shifted_operand(options, first, inst, true).is_some() || self.narrow_bswap(options, first, inst, true).is_some() || self.mul_add(options, first, inst, true).is_some();
                    debug_assert!(done, "a deferred pair no longer matches");
                    continue;
                }
                // three or more zero stores off one base in a row: the widest zr stores over their bytes
                let limit = if test_pair { n - 2 } else if fused { n - 1 } else { n };
                if ii + 2 < limit && isel_flags(options) & A64_FUSE != 0 {
                    let k = self.zero_run(bi, ii, limit);
                    if k > 0 {
                        skip_to = ii + k;
                        continue;
                    }
                }
                if ii + 1 < n && !(fused && ii + 1 == n - 1) {
                    // a shift by a constant feeding only the next add/sub/and/or/xor: the shifted
                    // register operand form (`eor d, a, b, lsl #13`); a 64-bit byte swap whose only
                    // use shifts the low half back down: the 32-bit forms (u32 / u16
                    // from_be_bytes: rev w; rev16 w). Emitted with the next instruction (above).
                    if arm64_defers(b, ii, &self.al.uses) && (self.shifted_operand(options, inst, &b.insts[ii + 1], false).is_some() || self.narrow_bswap(options, inst, &b.insts[ii + 1], false).is_some() || self.mul_add(options, inst, &b.insts[ii + 1], false).is_some()) {
                        deferred = Some(ii);
                        continue;
                    }
                    // two accesses of adjacent words off one base: ldp / stp
                    if let Some(()) = self.pair_access(options, inst, &b.insts[ii + 1], self.al.inst_pos(bi, ii + 1)) {
                        skip_next = true;
                        continue;
                    }
                }
                // a float multiply only read by the negation right after it: fnmul (one
                // instruction; -(a*b) bit for bit, NaNs included)
                if ii + 1 < n && !(fused && ii + 1 == n - 1) {
                    if let (Inst::FBin(FOp::Mul, w1, t, a, bb), Inst::FUnary(FUn::Neg, w2, _, t2)) = (inst, &b.insts[ii + 1]) {
                        if w1 == w2 && t == t2 && t != a && t != bb && self.al.uses[t.0 as usize] == 1 {
                            self.fnmul_ins = Some((*a, *bb));
                            continue;
                        }
                    }
                }
                // a zero vector only read by the lane-0 insert right after it (SROA'd vector cells,
                // `_mm_setr_ps` gathers): no movi; the insert is `fmov s` (zeroes lanes 1-3)
                if ii + 1 < n && !(fused && ii + 1 == n - 1) && self.zero_then_insert(inst, &b.insts[ii + 1]) {
                    if let Inst::VConst(z, _) | Inst::VX(_, z, _, _) = inst {
                        self.zero_ins = Some(*z);
                    }
                    continue;
                }
                // a compare feeding the selects right after it: they read its flags (nothing between
                // them sets flags); when they are all its uses, the bool is never materialised
                // (`cmp; cset; cmp #0; csel` was `cmp; csel`: Paeth's lo / hi / thresh selects)
                if let Inst::ICmp(_, _, d, ..) | Inst::ICmpI(_, _, d, ..) | Inst::FCmp(_, _, d, ..) = inst {
                    let mut k = 0;
                    while ii + 1 + k < n && !(fused && ii + 1 + k == n - 1) {
                        match &b.insts[ii + 1 + k] {
                            Inst::Sel(sd, c, x, y) if c == d && x != d && y != d && sd != d => k += 1,
                            _ => break,
                        }
                    }
                    if k > 0 {
                        if self.al.uses[d.0 as usize] as usize == k {
                            let cond = self.cmp_flags(inst);
                            self.sel_chain = Some((*d, cond));
                            continue;
                        }
                        if let Inst::ICmp(c, sg, ..) | Inst::ICmpI(c, sg, ..) = inst {
                            self.inst(options, inst);
                            self.sel_chain = Some((*d, icc(*c, *sg)));
                            continue;
                        }
                    }
                }
                if self.tail.get(bi) == Some(&Some(ii)) {
                    if let Inst::Call(c, args, _) = inst {
                        self.tail_call(c, args);
                    }
                    break;
                }
                self.inst(options, inst);
            }
            // a sibling call ended the block: its Ret is the callee's
            if self.tail.get(bi).is_some_and(|t| t.is_some()) {
                continue;
            }
            // region windows entered from here: loaded before the terminator (regalloc edge loads)
            if matches!(b.term, Term::Jump(_) | Term::Branch(..) | Term::Switch(..)) {
                let tp = self.al.block_end[bi] - 1;
                if let Some(ops) = self.evict_at.get(&tp).cloned() {
                    self.evict_transfers(&ops);
                }
                if let Some(ls) = self.win_start.get(&tp).cloned() {
                    for (v, r) in ls {
                        if let Loc::Stack(o) = self.al.loc[v as usize] {
                            let off = self.spill_sp(o);
                            self.a.ldst(LDRX, 8, r, SP, off);
                        }
                    }
                }
            }
            self.pc_map.push(((self.a.pos() * 4) as u32, b.term_pos));
            if self.rir_map_on {
                self.rir_map.push(((self.a.pos() * 4) as u32, bi as u32, u32::MAX));
            }
            let fuse_inst = if fused { Some(b.insts[n - 1].clone()) } else { None };
            self.term(&b.term, bi, fuse_inst);
        }
        if crate::rir::unwind_stats_on() {
            // bytes of blocks only reachable through unwind edges (cleanup pads)
            let nb = self.f.blocks.len();
            let mut normal = vec![false; nb];
            let mut st = vec![0u32];
            while let Some(b) = st.pop() {
                if normal[b as usize] {
                    continue;
                }
                normal[b as usize] = true;
                match self.f.blocks[b as usize].term {
                    Term::Invoke(n, _) => st.push(n),
                    ref t => st.extend(t.succs()),
                }
            }
            let end_pos = self.a.pos();
            let mut order: Vec<(usize, usize)> = (0..nb).filter(|b| reach[*b]).map(|b| (self.block_pos[b], b)).collect();
            order.sort();
            let mut pad = 0usize;
            for k in 0..order.len() {
                let next = if k + 1 < order.len() { order[k + 1].0 } else { end_pos };
                if !normal[order[k].1] {
                    pad += (next - order[k].0) * 4;
                }
            }
            crate::rir::UNWIND_STATS[0].fetch_add(pad as u64, std::sync::atomic::Ordering::Relaxed);
        }
        let stub0 = self.a.pos();
        // landing stubs of invoked calls: the unwinder resumes here with fp, sp and the
        // callee-saved registers of this frame restored; reload the caller-saved values the
        // call's normal path reloads, then the cleanup pad
        let mut unwind_tab: Vec<(u32, u32)> = Vec::new();
        for (ret, saves, pad) in std::mem::take(&mut self.pads) {
            // only the values the pad reads come back (live into the pad block); none: the
            // unwinder lands on the pad itself
            let at = self.al.block_start[pad as usize];
            let reload: Vec<(VReg, i32)> = saves.into_iter().filter(|(v, _)| self.al.start[v.0 as usize] <= at && self.al.end[v.0 as usize] >= at).collect();
            if reload.is_empty() {
                unwind_tab.push(((ret * 4) as u32, (self.block_pos[pad as usize] * 4) as u32));
                continue;
            }
            let stub = self.a.pos();
            for (v, slot) in &reload {
                self.save_restore(*v, *slot, false);
            }
            let p = self.a.b();
            self.fixups.push((p, pad));
            unwind_tab.push(((ret * 4) as u32, (stub * 4) as u32));
        }
        if crate::rir::unwind_stats_on() {
            crate::rir::UNWIND_STATS[1].fetch_add(((self.a.pos() - stub0) * 4) as u64, std::sync::atomic::Ordering::Relaxed);
            crate::rir::UNWIND_STATS[3].fetch_add(unwind_tab.len() as u64, std::sync::atomic::Ordering::Relaxed);
        }
        for (at, t) in std::mem::take(&mut self.fixups) {
            let target = self.block_pos[t as usize];
            self.a.patch(at, target);
        }
        for (at, start, t) in std::mem::take(&mut self.jt_fix) {
            let rel = (self.block_pos[t as usize] as i64 - start as i64) * 4;
            self.a.w[at] = rel as i32 as u32;
        }
        // asm! branches to symbols: one veneer per distinct symbol (ldr x16, =sym; br x16)
        let mut veneers: Vec<(Sym, usize)> = Vec::new();
        for (at, s) in std::mem::take(&mut self.asm_veneers) {
            let v = match veneers.iter().find(|x| x.0 == s) {
                Some(x) => x.1,
                None => {
                    let v = self.a.pos();
                    self.ldr_reloc(IP0, Reloc::Sym(s));
                    self.a.br(IP0);
                    veneers.push((s, v));
                    v
                }
            };
            let rel = v as i64 - at as i64;
            if !(-(1i64 << 25)..(1i64 << 25)).contains(&rel) {
                BRANCH_RANGE.with(|c| c.set(true));
            }
            self.a.w[at] = (self.a.w[at] & 0xFC00_0000) | (rel as u32 & 0x03FF_FFFF);
        }
        self.text_words = self.a.pos();
        // float literal pool (8-byte aligned entries, deduplicated)
        if !self.fpool.is_empty() {
            if self.a.pos() % 2 != 0 {
                self.a.udf();
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
                    self.a.e(bits as u32);
                    self.a.e((bits >> 32) as u32);
                    placed.push((bits, addr));
                }
                self.a.patch(at, addr);
            }
        }
        // address pool (8-byte words, one per distinct relocated value; Compiled::relocs)
        if !self.apool.is_empty() {
            if self.a.pos() % 2 != 0 {
                self.a.udf();
            }
            let mut placed: Vec<(Reloc, usize)> = Vec::new();
            for (at, r) in std::mem::take(&mut self.apool) {
                let addr = match placed.iter().find(|(r2, _)| *r2 == r) {
                    Some((_, a2)) => *a2,
                    None => {
                        let a2 = self.a.pos();
                        self.a.e(0);
                        self.a.e(0);
                        placed.push((r, a2));
                        self.relocs.push(RelocAt { at: (a2 * 4) as u32, kind: RelocKind::Pool64, target: r });
                        a2
                    }
                };
                if self.long {
                    let delta = ((addr - at) * 4) as u32;
                    if delta >= 1 << 24 {
                        BRANCH_RANGE.with(|c| c.set(true));
                    }
                    self.a.w[at + 1] |= ((delta >> 12) & 0xFFF) << 10;
                    self.a.w[at + 2] |= (delta & 0xFFF) << 10;
                } else {
                    self.a.patch(at, addr);
                }
            }
        }
        // vector literal pool (16-byte aligned, deduplicated)
        if !self.vpool.is_empty() {
            while self.a.pos() % 4 != 0 {
                self.a.udf();
            }
            let mut placed: Vec<(u32, usize)> = Vec::new();
            for (at, bits) in std::mem::take(&mut self.vpool) {
                let addr = match placed.iter().find(|(b2, _)| *b2 == bits) {
                    Some((_, a2)) => *a2,
                    None => {
                        let a2 = self.a.pos();
                        for _ in 0..4 {
                            self.a.e(bits);
                        }
                        placed.push((bits, a2));
                        a2
                    }
                };
                self.a.patch(at, addr);
            }
        }
        // unwind info for the runtime's frame walk (crate::unwind_info)
        let mut saved: Vec<u32> = self.al.used_callee_int.iter().map(|r| *r as u32).collect();
        saved.extend(self.al.used_callee_flt.iter().map(|x| 0x100 | *x as u32));
        let words = crate::rir::unwind_info_words(&saved, &unwind_tab, self.frame as u32);
        if crate::rir::unwind_stats_on() {
            crate::rir::UNWIND_STATS[2].fetch_add((words.len() * 4) as u64, std::sync::atomic::Ordering::Relaxed);
        }
        for w in words {
            self.a.e(w);
        }
    }

    /// Relax ABI params: incoming registers -> allocated locations; overflow params
    /// from the caller's outgoing area, [fp + 16 + 8k].
    fn params_prologue(&mut self) {
        let mut mv = Vec::new();
        let mut ni = 0usize;
        let mut nf = 0usize;
        let mut stack_params: Vec<(VReg, i64)> = Vec::new();
        for p in &self.f.params {
            let src = match self.cls(*p) {
                Cls::I if ni < INT_ARGS => {
                    ni += 1;
                    MLoc::R(ni as u8 - 1)
                }
                Cls::F32 | Cls::F64 if nf < FLT_ARGS => {
                    nf += 1;
                    MLoc::F(nf as u8 - 1)
                }
                _ => {
                    stack_params.push((*p, 16 + 8 * stack_params.len() as i64));
                    continue;
                }
            };
            if self.al.loc[p.0 as usize] != Loc::None {
                mv.push((self.mloc(*p), src));
            }
        }
        self.moves(&mv);
        for (p, off) in stack_params {
            let m = if self.cls(p) == Cls::I { Mem::Int(8, false) } else { Mem::F64 };
            self.load_param(p, m, off);
        }
    }

    /// Loads a stack-passed parameter ([fp + off], `m` sized) into its location.
    fn load_param(&mut self, p: VReg, m: Mem, off: i64) {
        let (op, size) = mem_op(m, true);
        match self.al.loc[p.0 as usize] {
            Loc::Reg(r) => self.a.ldst(op, size, r, FP, off),
            Loc::Flt(x) => self.a.ldst(op, size, x, FP, off),
            Loc::Stack(o) => {
                let so = self.spill_sp(o);
                if matches!(m, Mem::Int(..)) {
                    self.a.ldst(op, size, S1, FP, off);
                    self.a.ldst(STRX, 8, S1, SP, so);
                } else {
                    self.a.ldst(op, size, FS0, FP, off);
                    self.a.ldst(STRD, 8, FS0, SP, so);
                }
            }
            Loc::None => {}
        }
    }

    /// Sets a parameter's location to `base + off` (an address).
    fn def_addr_param(&mut self, p: VReg, base: u8, off: i64) {
        if self.al.loc[p.0 as usize] == Loc::None {
            return;
        }
        let rd = self.def_i(p, S0);
        self.a.add_any(rd, base, off);
        self.fin_i(p, rd);
    }

    /// Prologue of an `extern "C"` body: aggregate params are rebuilt in frame buffers
    /// (their address is the param), the rest arrive like Relax params but with C's
    /// placement (Apple stack packing, x8 result address); narrow ints are re-extended.
    fn c_prologue(&mut self) {
        let plan = self.cplan.clone().unwrap();
        let sig = self.f.cabi.clone().unwrap();
        let params = self.f.params.clone();
        let has_ret = sig.ret.is_some();
        let pargs = if has_ret { &params[1..] } else { &params[..] };
        // 1. aggregate pieces out of the argument registers (before any move clobbers them)
        for (k, ap) in plan.args.iter().enumerate() {
            if let ArgPlan::Parts(parts) = ap {
                let buf = self.cbuf[k];
                for pt in parts {
                    let o = buf + pt.off as i64;
                    match pt.loc {
                        PLoc::Int(r) => {
                            if pt.size == 4 {
                                self.a.ldst(STRW, 4, r, SP, o);
                            } else {
                                self.a.ldst(STRX, 8, r, SP, o);
                            }
                        }
                        PLoc::Flt(x) => {
                            if pt.size == 4 {
                                self.a.ldst(STRS, 4, x, SP, o);
                            } else {
                                self.a.ldst(STRD, 8, x, SP, o);
                            }
                        }
                        PLoc::Stack(_) => {}
                    }
                }
            }
        }
        // 2. register params (scalars, by-reference aggregates, the x8 result address)
        let mut mv = Vec::new();
        if let (RetPlan::Indirect, true) = (&plan.ret, has_ret) {
            if self.al.loc[params[0].0 as usize] != Loc::None {
                mv.push((self.mloc(params[0]), MLoc::R(X8)));
            }
        }
        for (k, ap) in plan.args.iter().enumerate() {
            let p = pargs[k];
            if self.al.loc[p.0 as usize] == Loc::None {
                continue;
            }
            match ap {
                ArgPlan::Scalar(PLoc::Int(r), _) | ArgPlan::ByRef(PLoc::Int(r)) => mv.push((self.mloc(p), MLoc::R(*r))),
                ArgPlan::Scalar(PLoc::Flt(x), _) => mv.push((self.mloc(p), MLoc::F(*x))),
                _ => {}
            }
        }
        self.moves(&mv);
        // 3. stack params, aggregate addresses, re-extension of narrow C integers
        for (k, ap) in plan.args.iter().enumerate() {
            let p = pargs[k];
            let m = match &sig.args[k] {
                CArg::Scalar(m) => *m,
                CArg::Agg(_) => Mem::Int(8, false),
            };
            match ap {
                ArgPlan::Scalar(PLoc::Stack(o), _) => self.load_param(p, m, 16 + *o as i64),
                ArgPlan::ByRef(PLoc::Stack(o)) => self.load_param(p, Mem::Int(8, false), 16 + *o as i64),
                ArgPlan::Parts(_) => {
                    let b = self.cbuf[k];
                    self.def_addr_param(p, SP, b);
                }
                ArgPlan::Stack(o, _) => self.def_addr_param(p, FP, 16 + *o as i64),
                ArgPlan::Scalar(PLoc::Int(_), _) => {
                    if let Mem::Int(b, sg) = m {
                        if b < 8 && self.al.loc[p.0 as usize] != Loc::None {
                            let r = self.use_i(p, S0);
                            self.a.extend(r, r, b * 8, sg);
                            self.fin_i(p, r);
                        }
                    }
                }
                _ => {}
            }
        }
        if let (RetPlan::Parts(_), true) = (&plan.ret, has_ret) {
            let b = self.cret_buf;
            self.def_addr_param(params[0], SP, b);
        }
    }

    /// Ret of an `extern "C"` body with an aggregate result held in registers.
    fn c_ret_parts(&mut self) {
        if let Some(RetPlan::Parts(parts)) = self.cplan.as_ref().map(|p| p.ret.clone()) {
            let buf = self.cret_buf;
            for pt in &parts {
                let o = buf + pt.off as i64;
                match pt.loc {
                    PLoc::Int(r) => self.a.ldst(LDRX, 8, r, SP, o),
                    PLoc::Flt(x) => {
                        if pt.size == 4 {
                            self.a.ldst(LDRS, 4, x, SP, o);
                        } else {
                            self.a.ldst(LDRD, 8, x, SP, o);
                        }
                    }
                    PLoc::Stack(_) => {}
                }
            }
        }
    }

    /// Touches every page of a big new frame from the top down, so a frame larger than
    /// the guard page faults on the guard instead of skipping past it.
    fn stack_probe(&mut self, frame: i64) {
        const PAGE: i64 = 4096;
        if frame <= PAGE {
            return;
        }
        // x17 = sp; loop: x17 -= PAGE; str xzr, [x17]; until x17 <= sp - frame
        self.a.mov_sp(IP1, SP);
        let mut o = PAGE;
        if frame <= 16 * PAGE {
            while o < frame {
                self.a.sub_imm_lsl12(IP1, IP1, (PAGE >> 12) as u32);
                self.a.ldst(STRX, 8, ZR, IP1, 0);
                o += PAGE;
            }
            return;
        }
        self.a.mov_imm(IP0, frame / PAGE);
        let top = self.a.pos();
        self.a.sub_imm_lsl12(IP1, IP1, (PAGE >> 12) as u32);
        self.a.ldst(STRX, 8, ZR, IP1, 0);
        self.a.sub_imm(IP0, IP0, 1);
        let p = self.a.cbnz(IP0);
        self.a.patch(p, top);
    }

    /// Copies `size` bytes [src + so] -> [dst + do_] through S1 (bases may be sp).
    fn copy_bytes(&mut self, dst: u8, do_: i64, src: u8, so: i64, size: u32) {
        let mut o = 0i64;
        let size = size as i64;
        while o < size {
            let (lop, sop, n) = if size - o >= 8 {
                (LDRX, STRX, 8)
            } else if size - o >= 4 {
                (LDRW, STRW, 4)
            } else if size - o >= 2 {
                (LDRH, STRH, 2)
            } else {
                (LDRB, STRB, 1)
            };
            self.a.ldst(lop, n as u32, S1, src, so + o);
            self.a.ldst(sop, n as u32, S1, dst, do_ + o);
            o += n;
        }
    }

    /// A call under the C ABI (foreign function, `extern "C"` pointer or body).
    fn c_call(&mut self, c: &Callee, sig: &CSig, args: &[VReg], rets: &[VReg]) {
        let plan = cabi::plan(sig, sig.conv.target());
        let saves = self.al.saves.get(&self.cur_pos).cloned().unwrap_or_default();
        for (v, slot) in &saves {
            // Alloc::save_skip: the slot already holds it / nobody reads the register
            if !self.al.save_skip.contains(&(self.cur_pos, v.0)) {
                self.save_restore(*v, *slot, true);
            }
        }
        let has_ret = sig.ret.is_some();
        let cargs = if has_ret { &args[1..] } else { args };
        let st = self.stage;
        // the result address survives the call in staging cell 0
        if has_ret {
            let r = self.use_i(args[0], S0);
            self.a.ldst(STRX, 8, r, SP, st);
        }
        if let Callee::CIndirect(v, _) = c {
            let r = self.use_i(*v, IP0);
            self.a.mov(IP0, r);
        }
        // stack arguments and aggregate pieces (staged in cells), then register moves
        let mut mv = Vec::new();
        let mut cell = 1i64;
        for (k, ap) in plan.args.iter().enumerate() {
            let v = cargs[k];
            match ap {
                ArgPlan::Scalar(PLoc::Int(r), _) | ArgPlan::ByRef(PLoc::Int(r)) => mv.push((MLoc::R(*r), self.mloc(v))),
                ArgPlan::Scalar(PLoc::Flt(x), _) => mv.push((MLoc::F(*x), self.mloc(v))),
                ArgPlan::Scalar(PLoc::Stack(o), size) => {
                    let o = *o as i64;
                    if self.cls(v) == Cls::I {
                        let r = self.use_i(v, S0);
                        let op = match size {
                            1 => STRB,
                            2 => STRH,
                            4 => STRW,
                            _ => STRX,
                        };
                        self.a.ldst(op, (*size).max(1), r, SP, o);
                    } else {
                        let x = self.use_f(v, FS0);
                        if *size == 4 && self.cls(v) == Cls::F32 {
                            self.a.ldst(STRS, 4, x, SP, o);
                        } else if self.cls(v) == Cls::F32 {
                            // variadic f32 would be promoted by C; store as f64
                            self.a.e(0x1E22_C000 | (x as u32) << 5 | FS1 as u32);
                            self.a.ldst(STRD, 8, FS1, SP, o);
                        } else {
                            self.a.ldst(STRD, 8, x, SP, o);
                        }
                    }
                }
                ArgPlan::ByRef(PLoc::Stack(o)) => {
                    let r = self.use_i(v, S0);
                    self.a.ldst(STRX, 8, r, SP, *o as i64);
                }
                ArgPlan::ByRef(PLoc::Flt(_)) => {}
                ArgPlan::Parts(parts) => {
                    let ra = self.use_i(v, S0);
                    for pt in parts {
                        let cell_off = st + 8 * cell;
                        self.copy_bytes(SP, cell_off, ra, pt.off as i64, pt.size);
                        match pt.loc {
                            PLoc::Int(r) => mv.push((MLoc::R(r), MLoc::M(cell_off as i32))),
                            PLoc::Flt(x) => mv.push((MLoc::F(x), MLoc::M(cell_off as i32))),
                            PLoc::Stack(_) => {}
                        }
                        cell += 1;
                    }
                }
                ArgPlan::Stack(o, size) => {
                    let ra = self.use_i(v, S0);
                    self.copy_bytes(SP, *o as i64, ra, 0, *size);
                }
            }
        }
        if let (RetPlan::Indirect, true) = (&plan.ret, has_ret) {
            mv.push((MLoc::R(X8), MLoc::M(st as i32)));
        }
        self.moves(&mv);
        match c {
            Callee::CFn(id, _) => self.bl_reloc(Reloc::Call(*id)),
            Callee::CHost(sym, _) => self.call_far(Reloc::Sym(*sym)),
            _ => self.a.blr(IP0),
        }
        // a C-unwind call is invoked like a Relax one: its landing stub is keyed by this address
        self.last_call_ret = self.a.pos();
        self.var_calls.push((self.cur_pos, (self.a.pos() * 4) as u32));
        // C leaves a narrow integer result's upper bits undefined
        if let Some(it) = sig.ret_narrow {
            self.a.extend(0, 0, it.bits, it.signed);
        }
        // scalar results
        let mut mv = Vec::new();
        let mut ri = 0u8;
        let mut rf = 0u8;
        for r in rets {
            if self.cls(*r) == Cls::I {
                if self.al.loc[r.0 as usize] != Loc::None {
                    mv.push((self.mloc(*r), MLoc::R(ri)));
                }
                ri += 1;
            } else {
                if self.al.loc[r.0 as usize] != Loc::None {
                    mv.push((self.mloc(*r), MLoc::F(rf)));
                }
                rf += 1;
            }
        }
        // aggregate result registers -> cells (x0/x1/v0-v3 are never allocated)
        if let RetPlan::Parts(parts) = &plan.ret {
            for (j, pt) in parts.iter().enumerate() {
                let o = st + 8 * (1 + j as i64);
                match pt.loc {
                    PLoc::Int(r) => self.a.ldst(STRX, 8, r, SP, o),
                    PLoc::Flt(x) => self.a.ldst(STRD, 8, x, SP, o),
                    PLoc::Stack(_) => {}
                }
            }
        }
        self.moves(&mv);
        for (v, slot) in &saves {
            // Alloc::restore_skip: the slot already holds it / nobody reads the register
            if !self.al.restore_skip.contains(&(self.cur_pos, v.0)) {
                self.save_restore(*v, *slot, false);
            }
        }
        if let RetPlan::Parts(parts) = &plan.ret {
            self.a.ldst(LDRX, 8, S0, SP, st);
            for (j, pt) in parts.iter().enumerate() {
                let o = st + 8 * (1 + j as i64);
                self.copy_bytes(S0, pt.off as i64, SP, o, pt.size);
            }
        }
    }

    // ---- atomics (LSE when the CPU has ARMv8.1 atomics, else exclusive-access loops: lse_atomics)

    fn atomic(&mut self, i: &Inst) {
        fn size_bits(m: Mem) -> (u32, u8, bool) {
            match m {
                Mem::Int(1, s) => (0, 8, s),
                Mem::Int(2, s) => (1, 16, s),
                Mem::Int(4, s) => (2, 32, s),
                Mem::Int(_, s) => (3, 64, s),
                _ => (3, 64, false),
            }
        }
        fn acq(o: AtomOrd) -> bool {
            matches!(o, AtomOrd::Acquire | AtomOrd::AcqRel | AtomOrd::SeqCst)
        }
        fn rel(o: AtomOrd) -> bool {
            matches!(o, AtomOrd::Release | AtomOrd::AcqRel | AtomOrd::SeqCst)
        }
        match i {
            Inst::AtomicLoad(m, o, d, a) => {
                let (sz, bits, sg) = size_bits(*m);
                let ra = self.use_i(*a, S1);
                let rd = self.def_i(*d, S0);
                if acq(*o) {
                    // ldar{b,h,w,x}
                    self.a.e(0x08DF_FC00 | sz << 30 | (ra as u32) << 5 | rd as u32);
                    if sg && bits < 64 {
                        self.a.extend(rd, rd, bits, true);
                    }
                } else {
                    let (op, size) = mem_op(*m, true);
                    self.a.ldst(op, size, rd, ra, 0);
                }
                self.fin_i(*d, rd);
            }
            Inst::AtomicStore(m, o, a, v) => {
                let (sz, _, _) = size_bits(*m);
                let ra = self.use_i(*a, S1);
                let rv = self.use_i(*v, S0);
                if rel(*o) {
                    // stlr{b,h,w,x}
                    self.a.e(0x089F_FC00 | sz << 30 | (ra as u32) << 5 | rv as u32);
                } else {
                    let (op, size) = mem_op(*m, false);
                    self.a.ldst(op, size, rv, ra, 0);
                }
            }
            Inst::AtomicRmw(op, m, o, d, a, v) => {
                let (sz, bits, sg) = size_bits(*m);
                let ar = (acq(*o) as u32) << 23 | (rel(*o) as u32) << 22;
                let ra = self.use_i(*a, IP0);
                let rv = self.use_i(*v, S1);
                let rd0 = self.def_i(*d, S0);
                // the old value goes to a register no operand uses
                let rd = if rd0 == ra || rd0 == rv { S0 } else { rd0 };
                if !lse_atomics() {
                    // ARMv8.0 (no LSE): a load-acquire-exclusive / store-release-exclusive loop.
                    // x8 = the old value, ip1 = the new one, rd = the store status (a temp before it).
                    let top = self.a.pos();
                    // ldaxr{b,h,,} x8, [ra]
                    self.a.e(0x085F_FC00 | sz << 30 | (ra as u32) << 5 | X8 as u32);
                    match op {
                        RmwOp::Xchg => self.a.mov(IP1, rv),
                        RmwOp::Add => self.a.rrr(0x8B00_0000, IP1, X8, rv),
                        RmwOp::Sub => self.a.rrr(0xCB00_0000, IP1, X8, rv),
                        RmwOp::And => self.a.rrr(0x8A00_0000, IP1, X8, rv),
                        RmwOp::Or => self.a.rrr(0xAA00_0000, IP1, X8, rv),
                        RmwOp::Xor => self.a.rrr(0xCA00_0000, IP1, X8, rv),
                        RmwOp::Nand => {
                            self.a.rrr(0x8A00_0000, IP1, X8, rv);
                            self.a.rrr(0xAA20_0000, IP1, ZR, IP1);
                        }
                        RmwOp::Max | RmwOp::Min | RmwOp::UMax | RmwOp::UMin => {
                            let signed = matches!(op, RmwOp::Max | RmwOp::Min);
                            let keep_old = match op {
                                RmwOp::Max => GT,
                                RmwOp::Min => LT,
                                RmwOp::UMax => HI,
                                _ => LO,
                            };
                            if bits == 64 {
                                self.a.cmp(X8, rv);
                            } else if bits == 32 {
                                // cmp w8, w(rv)
                                self.a.e(0x6B00_001F | (rv as u32) << 16 | (X8 as u32) << 5);
                            } else {
                                // compare at the access width: the load zero-extended x8
                                let lhs = if signed {
                                    self.a.extend(IP1, X8, bits, true);
                                    IP1
                                } else {
                                    X8
                                };
                                self.a.extend(rd, rv, bits, signed);
                                self.a.cmp(lhs, rd);
                            }
                            self.a.csel(IP1, X8, rv, keep_old);
                        }
                    }
                    // stlxr{b,h,,} w(rd), ip1, [ra]
                    self.a.e(0x0800_FC00 | sz << 30 | (rd as u32) << 16 | (ra as u32) << 5 | IP1 as u32);
                    let retry = self.a.cbnz(rd);
                    self.a.patch(retry, top);
                    if sg && bits < 64 {
                        self.a.extend(rd0, X8, bits, true);
                    } else {
                        self.a.mov(rd0, X8);
                    }
                    self.fin_i(*d, rd0);
                    return;
                }
                // LDADD family: size 111000 A R 1 Rs o3 opc 00 Rn Rt
                let lse = |opc: u32, o3: u32, rs: u8| 0x3820_0000 | sz << 30 | ar | (rs as u32) << 16 | o3 << 15 | opc << 12 | (ra as u32) << 5;
                match op {
                    RmwOp::Xchg => self.a.e(lse(0, 1, rv) | rd as u32),
                    RmwOp::Add => self.a.e(lse(0, 0, rv) | rd as u32),
                    RmwOp::Sub => {
                        self.a.rrr(0xCB00_0000, IP1, ZR, rv);
                        self.a.e(lse(0, 0, IP1) | rd as u32);
                    }
                    RmwOp::And => {
                        // ldclr with the complement
                        self.a.rrr(0xAA20_0000, IP1, ZR, rv);
                        self.a.e(lse(1, 0, IP1) | rd as u32);
                    }
                    RmwOp::Xor => self.a.e(lse(2, 0, rv) | rd as u32),
                    RmwOp::Or => self.a.e(lse(3, 0, rv) | rd as u32),
                    RmwOp::Max => self.a.e(lse(4, 0, rv) | rd as u32),
                    RmwOp::Min => self.a.e(lse(5, 0, rv) | rd as u32),
                    RmwOp::UMax => self.a.e(lse(6, 0, rv) | rd as u32),
                    RmwOp::UMin => self.a.e(lse(7, 0, rv) | rd as u32),
                    RmwOp::Nand => {
                        // old = [a]; loop { new = !(old & v); cas(old -> new) }
                        let (lop, lsz) = mem_op(Mem::Int(1 << sz, false), true);
                        self.a.ldst(lop, lsz, X8, ra, 0);
                        let top = self.a.pos();
                        self.a.rrr(0x8A00_0000, IP1, X8, rv);
                        self.a.rrr(0xAA20_0000, IP1, ZR, IP1);
                        self.a.mov(rd, X8);
                        // casal x8(expected -> observed), ip1(new), [ra]
                        self.a.e(0x08A0_7C00 | sz << 30 | 1 << 22 | 1 << 15 | (X8 as u32) << 16 | (ra as u32) << 5 | IP1 as u32);
                        self.a.cmp(X8, rd);
                        let p = self.a.bcond(NE);
                        self.a.patch(p, top);
                    }
                }
                if sg && bits < 64 {
                    self.a.extend(rd0, rd, bits, true);
                } else {
                    self.a.mov(rd0, rd);
                }
                self.fin_i(*d, rd0);
            }
            Inst::AtomicCas(m, ok, fail, d, a, e, n) => {
                let (sz, bits, sg) = size_bits(*m);
                let ra = self.use_i(*a, IP0);
                let re = self.use_i(*e, S0);
                let rn = self.use_i(*n, S1);
                if !lse_atomics() {
                    // ARMv8.0: ldaxr / cmp / stlxr loop. x8 = the observed value; ip1 = the expected
                    // value at the access width, then the store status.
                    let top = self.a.pos();
                    self.a.e(0x085F_FC00 | sz << 30 | (ra as u32) << 5 | X8 as u32);
                    if bits < 64 {
                        self.a.extend(IP1, re, bits, false);
                        self.a.cmp(X8, IP1);
                    } else {
                        self.a.cmp(X8, re);
                    }
                    let miss = self.a.bcond(NE);
                    self.a.e(0x0800_FC00 | sz << 30 | (IP1 as u32) << 16 | (ra as u32) << 5 | rn as u32);
                    let retry = self.a.cbnz(IP1);
                    self.a.patch(retry, top);
                    let done = self.a.pos();
                    self.a.patch(miss, done);
                    let rd = self.def_i(*d, S0);
                    self.a.mov(rd, X8);
                    if sg && bits < 64 {
                        self.a.extend(rd, rd, bits, true);
                    }
                    self.fin_i(*d, rd);
                    return;
                }
                let a_bit = (acq(*ok) || acq(*fail)) as u32;
                let r_bit = rel(*ok) as u32;
                // cas: Rs = expected in, observed out
                self.a.mov(IP1, re);
                self.a.e(0x08A0_7C00 | sz << 30 | a_bit << 22 | r_bit << 15 | (IP1 as u32) << 16 | (ra as u32) << 5 | rn as u32);
                let rd = self.def_i(*d, S0);
                self.a.mov(rd, IP1);
                if sg && bits < 64 {
                    self.a.extend(rd, rd, bits, true);
                }
                self.fin_i(*d, rd);
            }
            Inst::Fence(o, single) => {
                if !*single && *o != AtomOrd::Relaxed {
                    if *o == AtomOrd::Acquire {
                        self.a.e(0xD503_39BF); // dmb ishld
                    } else {
                        self.a.e(0xD503_3BBF); // dmb ish
                    }
                }
            }
            _ => {}
        }
    }

    /// x16 = this thread's thread-local block. Fast path: the block pointer sits in a word of
    /// the thread's slot array (macOS: pthread TSD slot `key` at tpidrro_el0; Android: bionic
    /// TLS_SLOT_APP at tpidr_el0, os/tls.rs); a null slot calls `tls_slow`, which keeps every
    /// register but x16/x17 (glue), so no saves here; lr is preserved by the frame record.
    fn tls_block_ip0(&mut self) {
        // mrs x16, <thread slot base>; and x16, x16, #~7; ldr x17, =key*8; ldr x16, [x16, x17]
        self.a.e(MRS_SLOT_BASE | IP0 as u32);
        self.a.e(0x9243_F000 | (IP0 as u32) << 5 | IP0 as u32);
        self.ldr_reloc(IP1, Reloc::TlsKey);
        self.a.e(0xF860_6800 | (IP1 as u32) << 16 | (IP0 as u32) << 5 | IP0 as u32);
        let p = self.a.cbnz(IP0);
        if self.near {
            self.bl_reloc(Reloc::TlsSlow);
        } else {
            self.ldr_reloc(IP1, Reloc::TlsSlow);
            self.a.blr(IP1);
        }
        let skip = self.a.pos();
        self.a.patch(p, skip);
    }

    /// d = this thread's thread-local block + off.
    fn tls_addr(&mut self, d: VReg, off: Sym) {
        self.tls_block_ip0();
        // the variable's offset in the block (a symbol: it is assigned per run)
        self.ldr_reloc(IP1, Reloc::Sym(off));
        let rd = self.def_i(d, S0);
        self.a.rrr(0x8B00_0000, rd, IP0, IP1);
        self.fin_i(d, rd);
    }

    /// The running thread's domain base (slot 0 of its thread-local block) into the frame's domain
    /// slot: the inline TLS read, the shared routine only for a thread that has no block yet.
    fn dom_base(&mut self) {
        self.tls_block_ip0();
        // ldr x16, [x16]: the domain base
        self.a.e(0xF940_0000 | (IP0 as u32) << 5 | IP0 as u32);
        self.a.ldst(STRX, 8, IP0, SP, self.dom_slot);
    }

    /// rd = the running thread's domain static block + `off` (a domain symbol: the offset in
    /// the block, assigned per run): the base the prologue stored + the offset.
    fn dom_addr(&mut self, rd: u8, off: Sym) {
        self.a.ldst(LDRX, 8, IP0, SP, self.dom_slot);
        self.ldr_reloc(IP1, Reloc::Sym(off));
        self.a.rrr(0x8B00_0000, rd, IP0, IP1);
    }

    /// `bl` to a relocated target (a slot's thunk, a near host or runtime entry): the embedder
    /// links the field, through a veneer (x16 only) when the target is out of reach.
    fn bl_reloc(&mut self, r: Reloc) {
        let at = self.a.pos();
        self.a.e(0x9400_0000);
        self.relocs.push(RelocAt { at: (at * 4) as u32, kind: RelocKind::A64Branch26, target: r });
    }

    /// A call of a host / runtime entry `r`: `bl` (near), else `blr x16` loaded from the pool.
    fn call_far(&mut self, r: Reloc) {
        if self.near {
            self.bl_reloc(r);
        } else {
            self.ldr_reloc(IP0, r);
            self.a.blr(IP0);
        }
    }

    /// `ldr xd, =<relocated value>` from the address pool after the code.
    fn ldr_reloc(&mut self, rd: u8, r: Reloc) {
        let at = self.a.pos();
        if self.long {
            // past +-1 MB: adr rd, .; add rd, rd, #hi, lsl 12; add rd, rd, #lo; ldr rd, [rd]
            // (pc-relative within the code image, up to 16 MB ahead; the adds are patched when
            // the pool is placed)
            self.a.e(0x1000_0000 | rd as u32);
            self.a.e(0x9140_0000 | (rd as u32) << 5 | rd as u32);
            self.a.e(0x9100_0000 | (rd as u32) << 5 | rd as u32);
            self.a.e(0xF940_0000 | (rd as u32) << 5 | rd as u32);
        } else {
            self.a.e(0x5800_0000 | rd as u32);
        }
        self.apool.push((at, r));
    }

    // ---- 128-bit vectors (NEON, 4S lanes)

    /// q register t <-> [n + off] (any offset; x17 for far ones)
    /// Saves (or restores) a caller-saved register's value around a call: 16 bytes for a vector.
    fn save_restore(&mut self, v: VReg, slot: i32, save: bool) {
        let off = self.spill_sp(slot);
        match self.al.loc[v.0 as usize] {
            Loc::Reg(r) => self.a.ldst(if save { STRX } else { LDRX }, 8, r, SP, off),
            Loc::Flt(x) if self.f.vregs[v.0 as usize] == Cls::V128 => self.vldst(!save, x, SP, off),
            Loc::Flt(x) => self.a.ldst(if save { STRD } else { LDRD }, 8, x, SP, off),
            _ => {}
        }
    }

    /// q register xd = every lane `bits`: movi / mvni / fmov when it encodes, else a literal load
    fn vconst_into(&mut self, xd: u8, bits: u32) {
        if let Some(w) = vimm(bits) {
            self.a.e(w | xd as u32);
        } else if self.long {
            // dup vd.4s, w16
            self.a.mov_imm(IP0, bits as i64);
            self.a.e(0x4E04_0C00 | (IP0 as u32) << 5 | xd as u32);
        } else {
            // ldr qd, =bits x4 (vector literal pool after the code)
            let at = self.a.pos();
            self.a.e(0x9C00_0000 | xd as u32);
            self.vpool.push((at, bits));
        }
    }

    /// `z = 0` (VConst 0, or FromLo64 of a constant 0) read only by the next instruction, an
    /// insert of a 32-bit lane 0 into it
    fn zero_then_insert(&self, a: &Inst, b: &Inst) -> bool {
        let z = match *a {
            Inst::VConst(z, 0) => z,
            Inst::VX(VX::FromLo64, z, k, _) if self.al.loc[k.0 as usize] == Loc::None && self.al.remat[k.0 as usize] == Remat::Const(0) => z,
            _ => return false,
        };
        if self.al.loc[z.0 as usize] == Loc::None {
            return false;
        }
        match *b {
            // d == z: the insert redefines the vreg, so the zero dies there
            Inst::VInsert(d, v, 0, s) => v == z && s != z && (d == z || self.al.uses[z.0 as usize] == 1) && matches!(self.cls(s), Cls::I | Cls::F32),
            _ => false,
        }
    }

    /// A vector access's base register and offset: register-less slot addresses and base + k
    /// values (Alloc::remat) folded in (sp + offset / the base's register + summed offset)
    fn vbase(&mut self, base: VReg, off: i64) -> (u8, i64) {
        let (b, o) = self.resolve_off(base, off);
        match self.slot_base(b) {
            Some(so) => (SP, so + o),
            None => (self.use_i(b, S1), o),
        }
    }

    /// q register at [n + (m << sh) + off]
    fn vldst_x(&mut self, load: bool, t: u8, n: u8, m: u8, sh: u8, off: i64) {
        if off == 0 && (sh == 0 || sh == 4) {
            // LDR/STR (register, SIMD&FP) 128-bit, option LSL, S = shift by 4
            self.a.e(if load { 0x3CE0_6800 } else { 0x3CA0_6800 } | if sh == 4 { 0x1000 } else { 0 } | (m as u32) << 16 | (n as u32) << 5 | t as u32);
            return;
        }
        // add s1, n, m, lsl sh (vldst's large-offset form uses ip1)
        self.a.e(0x8B00_0000 | (m as u32) << 16 | (sh as u32) << 10 | (n as u32) << 5 | S1 as u32);
        self.vldst(load, t, S1, off);
    }

    /// A float constant into x: fmov #0 / #imm8, else one literal load (ldr d / ldr s from the pool
    /// after the code), else (code past the literal range) through ip0.
    fn fconst_into(&mut self, x: u8, bits: u64, f64_: bool) {
        if bits == 0 {
            self.a.fmov_from_gp(true, x, ZR);
        } else if !self.fconst_isel {
            self.a.mov_imm(IP0, bits as i64);
            self.a.fmov_from_gp(f64_, x, IP0);
        } else if let Some(imm8) = fp_imm8(bits, f64_) {
            // fmov dd/sd, #imm
            self.a.e(0x1E20_1000 | if f64_ { DBL } else { 0 } | imm8 << 13 | x as u32);
        } else if !self.long {
            // ldr dd / sd, =bits (an f32 reads the low word of its 8-byte entry)
            let at = self.a.pos();
            self.a.e(if f64_ { 0x5C00_0000 } else { 0x1C00_0000 } | x as u32);
            self.fpool.push((at, if f64_ { bits } else { bits & 0xffff_ffff }));
        } else {
            self.a.mov_imm(IP0, bits as i64);
            self.a.fmov_from_gp(f64_, x, IP0);
        }
    }

    fn vldst(&mut self, load: bool, t: u8, n: u8, off: i64) {
        if off >= 0 && off % 16 == 0 && off / 16 < 4096 {
            self.a.e(if load { 0x3DC0_0000 } else { 0x3D80_0000 } | ((off / 16) as u32) << 10 | (n as u32) << 5 | t as u32);
        } else if (-256..256).contains(&off) {
            self.a.e(if load { 0x3CC0_0000 } else { 0x3C80_0000 } | ((off as u32) & 0x1ff) << 12 | (n as u32) << 5 | t as u32);
        } else {
            self.a.add_any(IP1, n, off);
            self.a.e(if load { 0x3DC0_0000 } else { 0x3D80_0000 } | (IP1 as u32) << 5 | t as u32);
        }
    }

    /// An `asm!` block (Callee::Asm): the inputs into the block's registers, its words, the
    /// outputs out of them. The allocator keeps every other value out of `b.regs` over the block
    /// (arm64_clobbers), so no input's source and no output's home is one of them; values in
    /// caller-saved registers that live across it are saved as around a call. Inputs bound for a
    /// backend scratch register go last (loading another input may use one).
    fn asm_block(&mut self, b: &AsmBlock, args: &[VReg], rets: &[VReg]) {
        let saves = self.al.saves.get(&self.cur_pos).cloned().unwrap_or_default();
        for (v, slot) in &saves {
            if !self.al.save_skip.contains(&(self.cur_pos, v.0)) {
                self.save_restore(*v, *slot, true);
            }
        }
        let scratch = |r: crate::asm::PReg| if r.vec { r.n == FS0 || r.n == FS1 || r.n == FS2 } else { matches!(r.n, S0 | S1 | IP0 | IP1) };
        let mut order: Vec<usize> = (0..args.len()).collect();
        order.sort_by_key(|&k| scratch(b.ins[k]));
        for k in order {
            let (v, r) = (args[k], b.ins[k]);
            if r.vec {
                let x = self.use_f(v, r.n);
                self.vmov(r.n, x);
            } else {
                let x = self.use_i(v, r.n);
                self.a.mov(r.n, x);
            }
        }
        let base = self.a.pos();
        for c in b.code.chunks(4) {
            self.a.e(u32::from_le_bytes([c[0], c[1], c[2], c[3]]));
        }
        for &(at, kind, s) in &b.fixups {
            let w = base + at as usize / 4;
            match kind {
                // a branch to a symbol goes through a veneer after the text (ldr x16, =sym; br x16:
                // IP0, which AAPCS64 lets veneers use)
                crate::asm::FixKind::A64Branch26 => self.asm_veneers.push((w, s)),
                crate::asm::FixKind::A64Page21 => self.relocs.push(RelocAt { at: (w * 4) as u32, kind: RelocKind::A64Adrp, target: Reloc::Sym(s) }),
                crate::asm::FixKind::A64Lo12 { scale: 0 } => self.relocs.push(RelocAt { at: (w * 4) as u32, kind: RelocKind::A64AddLo12, target: Reloc::Sym(s) }),
                crate::asm::FixKind::A64Lo12 { scale: 3 } => self.relocs.push(RelocAt { at: (w * 4) as u32, kind: RelocKind::A64LdrLo12X, target: Reloc::Sym(s) }),
                crate::asm::FixKind::Abs64 => self.relocs.push(RelocAt { at: (w * 4) as u32, kind: RelocKind::Pool64, target: Reloc::Sym(s) }),
                crate::asm::FixKind::Abs32 => self.relocs.push(RelocAt { at: (w * 4) as u32, kind: RelocKind::Imm32, target: Reloc::Sym(s) }),
                other => unreachable!("arm64 asm! fixup {:?} (lowering rejects it)", other),
            }
        }
        if b.opts & ASM_NORETURN != 0 {
            return;
        }
        for (k, &v) in rets.iter().enumerate() {
            let r = b.outs[k];
            if r.vec {
                let x = self.def_f(v, r.n);
                self.vmov(x, r.n);
                self.fin_f(v, x);
            } else {
                let x = self.def_i(v, r.n);
                self.a.mov(x, r.n);
                self.fin_i(v, x);
            }
        }
        for (v, slot) in &saves {
            if !self.al.restore_skip.contains(&(self.cur_pos, v.0)) {
                self.save_restore(*v, *slot, false);
            }
        }
    }

    fn vmov(&mut self, d: u8, n: u8) {
        if d != n {
            // orr vd.16b, vn.16b, vn.16b
            self.a.e(0x4EA0_1C00 | (n as u32) << 16 | (n as u32) << 5 | d as u32);
        }
    }

    /// Where an in-place vector op builds `d`: d's own register unless one of `keep`
    /// (read after the build starts) lives there, else FS2 (spilled d stores from FS2).
    fn vtmp(&self, d: VReg, keep: &[VReg]) -> u8 {
        let xd = self.def_f(d, FS2);
        let clash = keep.iter().enumerate().any(|(k, &r)| self.def_f(r, if k == 0 { FS0 } else { FS1 }) == xd);
        if clash {
            FS2
        } else {
            xd
        }
    }

    // Repair the ARM arithmetic result to SSE's NaN selection, all in vectors.
    // a/b remain intact even if the allocated result aliases an operand.
    fn intel_nan(&mut self, w: VW, result: u8, a: u8, b: u8) {
        const NOT: u32 = 0x6E20_5800;
        const BSL: u32 = 0x6E60_1C00;
        const ORR: u32 = 0x4EA0_1C00;
        // fcmeq .4s / .2d; the negative default NaN and the quiet bit of the lane type
        let (fcmeq, nan, quiet) = if w == VW::D {
            (0x4E60_E400, 0xfff8_0000_0000_0000u64, 0x0008_0000_0000_0000u64)
        } else {
            (0x4E20_E400, 0xffc0_0000_ffc0_0000, 0x0040_0000_0040_0000)
        };
        self.const128(5, nan, nan);
        self.v3(fcmeq, FS2, result, result);
        self.v2(NOT, FS2, FS2);
        self.v3(BSL, FS2, 5, result);
        self.vmov(result, FS2);
        self.const128(5, quiet, quiet);
        for input in [b, a] {
            self.v3(ORR, 6, input, 5);
            self.v3(fcmeq, FS2, input, input);
            self.v2(NOT, FS2, FS2);
            self.v3(BSL, FS2, 6, result);
            self.vmov(result, FS2);
        }
    }

    fn v3(&mut self, op: u32, d: u8, n: u8, m: u8) {
        self.a.e(op | (m as u32) << 16 | (n as u32) << 5 | d as u32);
    }

    fn v2(&mut self, op: u32, d: u8, n: u8) {
        self.a.e(op | (n as u32) << 5 | d as u32);
    }


    /// Reverse four 32-bit words, without reversing bytes inside a word.
    fn sha_reverse(&mut self, d: u8, a: u8) {
        self.v2(0x4EA0_0800, d, a); // rev64 .4s
        self.v3(0x6E00_4000, d, d, d); // ext #8
    }

    fn vx3(&mut self, op: VX, d: VReg, a: VReg, b: VReg, c: VReg) {
        if op == VX::X86Sha256Rnds2 {
            // Intel's [H,G,D,C], [F,E,B,A] -> Arm's [A,B,C,D], [E,F,G,H].
            // After four Arm rounds, lanes 2/3 still contain the round-2/1 values.
            let xa = self.use_f(a, FS0);
            let xb = self.use_f(b, FS1);
            let xc = self.use_f(c, FS2);
            self.v3(0x4EC0_7800, 4, xa, xb); // zip2 .2d
            self.sha_reverse(4, 4);
            self.v3(0x4EC0_3800, 5, xa, xb); // zip1 .2d
            self.sha_reverse(5, 5);
            self.vmov(6, 4);
            self.v3(0x5E00_4000, 6, 5, xc); // sha256h
            self.v3(0x5E00_5000, 5, 4, xc); // sha256h2, original ABCD
            self.v3(0x4EC0_7800, 4, 5, 6); // [E2,E1,A2,A1]
            self.v2(0x4EA0_0800, 4, 4); // [E1,E2,A1,A2]
            let xd = self.def_f(d, FS0);
            self.vmov(xd, 4);
            self.fin_f(d, xd);
            return;
        }
        let encoding = match op {
            VX::ArmSha1C => 0x5E00_0000,
            VX::ArmSha1P => 0x5E00_1000,
            VX::ArmSha1M => 0x5E00_2000,
            VX::ArmSha1Su0 => 0x5E00_3000,
            VX::ArmSha256H => 0x5E00_4000,
            VX::ArmSha256H2 => 0x5E00_5000,
            VX::ArmSha256Su1 => 0x5E00_6000,
            _ => unreachable!("arm64 ternary vx {:?}", op),
        };
        let xa = self.use_f(a, FS0);
        let xb = self.use_f(b, FS1);
        // v4 is unallocated; leave FS2 available for an aliased/spilled result.
        let xc = self.use_f(c, 4);
        let xd = self.def_f(d, FS2);
        let tmp = if xd == xb || xd == xc { FS2 } else { xd };
        self.vmov(tmp, xa);
        self.v3(encoding, tmp, xb, xc);
        self.vmov(xd, tmp);
        self.fin_f(d, xd);
    }

    /// Lane-typed vector ops (core::arch intrinsics), NEON. Scratch: v29-v31 (FS*) and
    /// v4-v7 (argument registers, unallocated inside an instruction).
    /// The lane-wise float, compare, shift, min/max and conversion ops of every width NEON
    /// has them for (`vx_native`); false for other ops.
    fn vx_lanes(&mut self, op: VX, d: VReg, a: VReg, b: VReg) -> bool {
        use v4s::*;
        // integer three-same / two-reg-misc: the size field; float: the sz bit
        let size = |w: VW| (w as u32) << 22;
        let sz = |w: VW| if w == VW::D { 1u32 << 22 } else { 0 };
        let s4 = !(3u32 << 22);
        const T0: u8 = 4;
        const T1: u8 = 5;
        const FMINNM_V: u32 = 0x4EA0_C400;
        const FMAXNM_V: u32 = 0x4E20_C400;
        const CMHI: u32 = 0x6E20_3400;
        const CMHS: u32 = 0x6E20_3C00;
        match op {
            VX::FAdd(w, _) | VX::FSub(w, _) | VX::FMul(w, _) | VX::FDiv(w, _) | VX::FMinNeon(w) | VX::FMaxNeon(w) | VX::FMinLt(w) | VX::FMaxGt(w) | VX::FMinNum(w) | VX::FMaxNum(w) | VX::FCmp(_, w) | VX::MinS(w @ VW::D) | VX::MaxS(w @ VW::D) | VX::MinU(w @ VW::D) | VX::MaxU(w @ VW::D) if matches!(w, VW::S | VW::D) => {
                let xa = self.use_f(a, FS0);
                let xb = self.use_f(b, FS1);
                let xd = self.def_f(d, FS0);
                let f = sz(w);
                match op {
                    VX::FAdd(_, NanBits::Host | NanBits::Neon) => self.v3(FADD | f, xd, xa, xb),
                    VX::FSub(_, NanBits::Host | NanBits::Neon) => self.v3(FSUB | f, xd, xa, xb),
                    // by element (fold_lane_splats): fmul vd.4s, va.4s, vb.s[0]
                    VX::FMul(_, NanBits::Host | NanBits::Neon) if self.cls(b) == Cls::F32 => self.v3(0x4F80_9000, xd, xa, xb),
                    VX::FMul(_, NanBits::Host | NanBits::Neon) => self.v3(FMUL | f, xd, xa, xb),
                    VX::FDiv(_, NanBits::Host | NanBits::Neon) => self.v3(FDIV | f, xd, xa, xb),
                    VX::FMinNeon(_) => self.v3(0x4EA0_F400 | f, xd, xa, xb),
                    VX::FMaxNeon(_) => self.v3(0x4E20_F400 | f, xd, xa, xb),
                    VX::FAdd(_, NanBits::Intel) | VX::FSub(_, NanBits::Intel) | VX::FMul(_, NanBits::Intel) | VX::FDiv(_, NanBits::Intel) => {
                        let opc = match op {
                            VX::FAdd(..) => FADD,
                            VX::FSub(..) => FSUB,
                            VX::FMul(..) => FMUL,
                            _ => FDIV,
                        };
                        self.v3(opc | f, 4, xa, xb);
                        self.intel_nan(w, 4, xa, xb);
                        self.vmov(xd, 4);
                    }
                    VX::FMinLt(_) | VX::FMaxGt(_) | VX::MinS(_) | VX::MaxS(_) | VX::MinU(_) | VX::MaxU(_) => {
                        // mask = b > a (min) / a > b (max), false for NaN lanes: b there
                        let (gt, min) = match op {
                            VX::FMinLt(_) => (FCMGT | f, true),
                            VX::FMaxGt(_) => (FCMGT | f, false),
                            VX::MinS(_) => (CMGT & s4 | size(w), true),
                            VX::MaxS(_) => (CMGT & s4 | size(w), false),
                            VX::MinU(_) => (CMHI | size(w), true),
                            _ => (CMHI | size(w), false),
                        };
                        if min {
                            self.v3(gt, FS2, xb, xa);
                        } else {
                            self.v3(gt, FS2, xa, xb);
                        }
                        self.v3(BSL, FS2, xa, xb);
                        self.vmov(xd, FS2);
                    }
                    VX::FMinNum(_) | VX::FMaxNum(_) => {
                        // IEEE minimumNumber: quiet both inputs (fminnm x, x, x) so a
                        // signaling NaN gives the other operand too, as rustc emits
                        self.v3(FMINNM_V | f, T0, xa, xa);
                        self.v3(FMINNM_V | f, T1, xb, xb);
                        self.v3(if let VX::FMinNum(_) = op { FMINNM_V } else { FMAXNM_V } | f, xd, T0, T1);
                    }
                    VX::FCmp(c, _) => match c {
                        Cond::Lt => self.v3(FCMGT | f, xd, xb, xa),
                        Cond::Le => self.v3(FCMGE | f, xd, xb, xa),
                        Cond::Gt => self.v3(FCMGT | f, xd, xa, xb),
                        Cond::Ge => self.v3(FCMGE | f, xd, xa, xb),
                        Cond::Eq => self.v3(FCMEQ | f, xd, xa, xb),
                        Cond::Ne => {
                            self.v3(FCMEQ | f, xd, xa, xb);
                            self.v2(NOT, xd, xd);
                        }
                    },
                    _ => unreachable!("vx {:?}", op),
                }
                self.fin_f(d, xd);
            }
            VX::ShlV(w) | VX::ShrSV(w) | VX::ShrUV(w) | VX::ArmUShl(w) | VX::ArmSShl(w) | VX::ICmp(_, w) | VX::UCmp(_, w) => {
                let xa = self.use_f(a, FS0);
                let xb = self.use_f(b, FS1);
                let xd = self.def_f(d, FS0);
                let s = size(w);
                let (gt, ge, eq) = match op {
                    VX::UCmp(..) => (CMHI | s, CMHS | s, CMEQ & s4 | s),
                    _ => (CMGT & s4 | s, CMGE & s4 | s, CMEQ & s4 | s),
                };
                match op {
                    VX::ShlV(_) | VX::ArmUShl(_) => self.v3(USHL & s4 | s, xd, xa, xb),
                    VX::ArmSShl(_) => self.v3(SSHL & s4 | s, xd, xa, xb),
                    VX::ShrSV(_) | VX::ShrUV(_) => {
                        self.v2(NEG & s4 | s, FS2, xb);
                        self.v3(if let VX::ShrSV(_) = op { SSHL } else { USHL } & s4 | s, xd, xa, FS2);
                    }
                    VX::ICmp(c, _) | VX::UCmp(c, _) => match c {
                        Cond::Lt => self.v3(gt, xd, xb, xa),
                        Cond::Le => self.v3(ge, xd, xb, xa),
                        Cond::Gt => self.v3(gt, xd, xa, xb),
                        Cond::Ge => self.v3(ge, xd, xa, xb),
                        Cond::Eq => self.v3(eq, xd, xa, xb),
                        Cond::Ne => {
                            self.v3(eq, xd, xa, xb);
                            self.v2(NOT, xd, xd);
                        }
                    },
                    _ => unreachable!(),
                }
                self.fin_f(d, xd);
            }
            VX::FNeg(w) | VX::FAbs(w) | VX::FSqrt(w, _) | VX::FRound(w, _) | VX::FToI(w) | VX::IToF(w) if matches!(w, VW::S | VW::D) => {
                let xa = self.use_f(a, FS0);
                let xd = self.def_f(d, FS0);
                if op == VX::FSqrt(w, NanBits::Intel) {
                    self.v2(0x6EA1_F800 | sz(w), 4, xa);
                    self.intel_nan(w, 4, xa, xa);
                    self.vmov(xd, 4);
                    self.fin_f(d, xd);
                    return true;
                }
                let opc = match op {
                    VX::FNeg(_) => VFNEG,
                    VX::FAbs(_) => 0x4EA0_F800,
                    VX::FSqrt(_, NanBits::Host | NanBits::Neon) => 0x6EA1_F800,
                    VX::FRound(_, RoundTo::Floor) => 0x4E21_9800,
                    VX::FRound(_, RoundTo::Ceil) => 0x4EA1_8800,
                    VX::FRound(_, RoundTo::Trunc) => 0x4EA1_9800,
                    VX::FRound(_, RoundTo::Away) => 0x6E21_8800,
                    VX::FRound(_, RoundTo::Even) => 0x4E21_8800, // frintn
                    VX::FToI(_) => 0x4EA1_B800,
                    VX::IToF(_) => 0x4E21_D800,
                    _ => unreachable!("vx {:?}", op),
                };
                self.v2(opc | sz(w), xd, xa);
                self.fin_f(d, xd);
            }
            VX::Neg(w) => {
                let xa = self.use_f(a, FS0);
                let xd = self.def_f(d, FS0);
                self.v2(NEG & s4 | size(w), xd, xa);
                self.fin_f(d, xd);
            }
            VX::Narrow(_) | VX::FCvtNarrow if op != VX::Narrow(VW::B) => {
                // xtn / fcvtn into the low half, xtn2 / fcvtn2 into the high half
                let w = if let VX::Narrow(w) = op { w } else { VW::D };
                let xa = self.use_f(a, FS0);
                let xb = self.use_f(b, FS1);
                let (lo, hi) = match op {
                    VX::Narrow(_) => (0x0E21_2800 | size(w) - (1 << 22), 0x4E21_2800 | size(w) - (1 << 22)),
                    _ => (0x0E61_6800, 0x4E61_6800),
                };
                self.v2(lo, T0, xa);
                self.v2(hi, T0, xb);
                let xd = self.def_f(d, FS0);
                self.vmov(xd, T0);
                self.fin_f(d, xd);
            }
            VX::FCvtLo => {
                let xa = self.use_f(a, FS0);
                let xd = self.def_f(d, FS0);
                self.v2(0x0E61_7800, xd, xa); // fcvtl .2d, .2s
                self.fin_f(d, xd);
            }
            VX::MoveMask(VW::D) => {
                // two lanes: ushr #63; lane 0 | lane 1 << 1 (fmov x, d; umov x, v.d[1]; orr .., lsl #1)
                let xa = self.use_f(a, FS0);
                self.v2(0x6F00_0400 | 65 << 16, T0, xa); // ushr v.2d, #63 (immh:immb = 128 - 63)
                let rd = self.def_i(d, S0);
                self.v2(0x9E66_0000, rd, T0); // fmov x, d (lane 0)
                self.a.e(0x4E18_3C00 | (T0 as u32) << 5 | IP0 as u32); // umov x16, v.d[1]
                self.a.e(0xAA00_0000 | (IP0 as u32) << 16 | 1 << 10 | (rd as u32) << 5 | rd as u32); // orr rd, rd, x16, lsl #1
                self.fin_i(d, rd);
            }
            VX::MoveMask(w) if w != VW::B => {
                // top bits to 0/1 lanes, each shifted to its lane index, summed across
                let xa = self.use_f(a, FS0);
                let es = w.bits();
                self.v2(0x6F00_0400 | (es + 1) << 16, T0, xa); // ushr #(es - 1)
                let (mut lo, mut hi) = (0u64, 0u64);
                for k in 0..128 / es {
                    let bit = k * es;
                    if bit < 64 {
                        lo |= (k as u64) << bit;
                    } else {
                        hi |= (k as u64) << (bit - 64);
                    }
                }
                self.const128(T1, lo, hi);
                self.v3(USHL & s4 | size(w), T0, T0, T1);
                let rd = self.def_i(d, S0);
                if w == VW::D {
                    self.v2(0x5EF1_B800, T0, T0); // addp d, .2d
                    self.v2(0x9E66_0000, rd, T0); // fmov x, d
                } else {
                    self.v2(0x4E31_B800 | size(w), T0, T0); // addv
                    self.v2(0x0E00_3C00 | (1u32 << (w as u32)) << 16, rd, T0); // umov lane 0
                }
                self.fin_i(d, rd);
            }
            _ => return false,
        }
        true
    }

    fn vx(&mut self, op: VX, d: VReg, a: VReg, b: VReg) {
        if self.vx_lanes(op, d, a, b) {
            return;
        }
        // core::arch exposes Intel semantics on Arm too (check_arch's cross-ISA
        // fixtures exercise them). Keep these in hardware, with lane conversion.
        if matches!(op, VX::X86Sha1Rnds4(_) | VX::X86Sha1NextE | VX::X86Sha1Msg1
            | VX::X86Sha1Msg2 | VX::X86Sha256Msg1 | VX::X86Sha256Msg2) {
            let xa = self.use_f(a, FS0);
            let xb = self.use_f(b, FS1);
            match op {
                VX::X86Sha1Rnds4(k) => {
                    self.sha_reverse(4, xa);
                    self.sha_reverse(5, xb);
                    let kword = [0x5a82_7999u64, 0x6ed9_eba1, 0x8f1b_bcdc, 0xca62_c1d6][k as usize];
                    let pair = kword | kword << 32;
                    self.const128(6, pair, pair);
                    self.v3(0x4EA0_8400, 5, 5, 6); // add .4s, WK includes K on Arm
                    self.movi16b(6, 0); // E is already included in Intel's first WK
                    let enc = match k { 0 => 0x5E00_0000, 2 => 0x5E00_2000, _ => 0x5E00_1000 };
                    self.v3(enc, 4, 6, 5);
                    self.sha_reverse(4, 4);
                }
                VX::X86Sha1NextE => {
                    self.sha_reverse(4, xa);
                    self.v2(0x5E28_0800, 4, 4); // sha1h, zero upper lanes
                    self.movi16b(5, 0);
                    self.v3(0x6E00_2000, 4, 5, 4); // ext #4: move E to lane 3
                    self.v3(0x4EA0_8400, 4, xb, 4);
                }
                VX::X86Sha1Msg1 | VX::X86Sha1Msg2 => {
                    self.sha_reverse(4, xa);
                    self.sha_reverse(5, xb);
                    if op == VX::X86Sha1Msg1 {
                        self.movi16b(6, 0);
                        self.v3(0x5E00_3000, 4, 5, 6);
                    } else {
                        self.v2(0x5E28_1800, 4, 5);
                    }
                    self.sha_reverse(4, 4);
                }
                VX::X86Sha256Msg1 => {
                    self.vmov(4, xa);
                    self.v2(0x5E28_2800, 4, xb);
                }
                VX::X86Sha256Msg2 => {
                    self.vmov(4, xa);
                    self.movi16b(5, 0);
                    self.vmov(6, xb);
                    self.v2(0x4E04_1C00, 6, 31); // ins v6.s[0], wzr
                    self.v3(0x5E00_6000, 4, 5, 6); // no middle-word additions
                }
                _ => unreachable!(),
            }
            let xd = self.def_f(d, FS0);
            self.vmov(xd, 4);
            self.fin_f(d, xd);
            return;
        }
        if op == VX::ArmSha1H {
            let xa = self.use_f(a, FS0);
            let xd = self.def_f(d, FS2);
            self.v2(0x5E28_0800, xd, xa);
            self.fin_f(d, xd);
            return;
        }
        let sha = match op {
            VX::ArmSha1Su1 => Some(0x5E28_1800),
            VX::ArmSha256Su0 => Some(0x5E28_2800),
            _ => None,
        };
        if let Some(encoding) = sha {
            let xa = self.use_f(a, FS0);
            let xb = self.use_f(b, FS1);
            let xd = self.def_f(d, FS2);
            let tmp = if xd == xb && xd != xa { FS2 } else { xd };
            self.vmov(tmp, xa);
            self.v2(encoding, tmp, xb);
            self.vmov(xd, tmp);
            self.fin_f(d, xd);
            return;
        }
        // three-same (Q=1): u, opcode
        let ts = |u: u32, w: VW, opc: u32| -> u32 { 0x4E20_0400 | u << 29 | (w as u32) << 22 | opc << 11 };
        const AND: u32 = 0x4E20_1C00;
        const ORR: u32 = 0x4EA0_1C00;
        const EOR: u32 = 0x6E20_1C00;
        const BIC: u32 = 0x4E60_1C00;
        const T0: u8 = 4;
        const T1: u8 = 5;
        const T2: u8 = 6;
        const T3: u8 = 7;
        let three = match op {
            VX::Add(w) => Some(ts(0, w, 0b10000)),
            VX::Sub(w) => Some(ts(1, w, 0b10000)),
            VX::MulLo(w) => Some(ts(0, w, 0b10011)),
            VX::MaxS(w) => Some(ts(0, w, 0b01100)),
            VX::MinS(w) => Some(ts(0, w, 0b01101)),
            VX::MaxU(w) => Some(ts(1, w, 0b01100)),
            VX::MinU(w) => Some(ts(1, w, 0b01101)),
            VX::CmpEq(w) => Some(ts(1, w, 0b10001)),
            VX::CmpGtS(w) => Some(ts(0, w, 0b00110)),
            VX::AddSatU(w) => Some(ts(1, w, 0b00001)),
            VX::AddSatS(w) => Some(ts(0, w, 0b00001)),
            VX::SubSatU(w) => Some(ts(1, w, 0b00101)),
            VX::SubSatS(w) => Some(ts(0, w, 0b00101)),
            VX::AvgU(w) => Some(ts(1, w, 0b00010)),
            VX::And => Some(AND),
            VX::Or => Some(ORR),
            VX::Xor => Some(EOR),
            VX::AndNot => Some(BIC),
            VX::ZipLo(w) => Some(0x4E00_3800 | (w as u32) << 22),
            VX::ZipHi(w) => Some(0x4E00_7800 | (w as u32) << 22),
            _ => None,
        };
        if let Some(opc) = three {
            let xa = self.use_f(a, FS0);
            let xb = self.use_f(b, FS1);
            let xd = self.def_f(d, FS0);
            self.v3(opc, xd, xa, xb);
            self.fin_f(d, xd);
            return;
        }
        let esize = |w: VW| w.bits();
        match op {
            VX::MaddH => {
                // smull lo / smull2 hi (h -> s), then pairwise add
                let xa = self.use_f(a, FS0);
                let xb = self.use_f(b, FS1);
                self.v3(0x0E60_C000, T0, xa, xb);
                self.v3(0x4E60_C000, T1, xa, xb);
                let xd = self.def_f(d, FS0);
                self.v3(0x4EA0_BC00, xd, T0, T1); // addp .4s
                self.fin_f(d, xd);
            }
            VX::MaddUbs => {
                let xa = self.use_f(a, FS0);
                let xb = self.use_f(b, FS1);
                self.v2(0x2F08_A400, T0, xa); // ushll .8h, .8b, #0
                self.v2(0x6F08_A400, T1, xa); // ushll2
                self.v2(0x0F08_A400, T2, xb); // sshll
                self.v2(0x4F08_A400, T3, xb); // sshll2
                self.v3(ts(0, VW::H, 0b10011), T0, T0, T2); // mul .8h
                self.v3(ts(0, VW::H, 0b10011), T1, T1, T3);
                self.v2(0x4E60_2800, T0, T0); // saddlp .4s, .8h
                self.v2(0x4E60_2800, T1, T1);
                let xd = self.def_f(d, FS0);
                self.v2(0x0E61_4800, xd, T0); // sqxtn .4h, .4s
                self.v2(0x4E61_4800, xd, T1); // sqxtn2 .8h, .4s
                self.fin_f(d, xd);
            }
            VX::SadU8 => {
                let xa = self.use_f(a, FS0);
                let xb = self.use_f(b, FS1);
                self.v3(0x6E20_7400, T0, xa, xb); // uabd .16b
                self.v2(0x6E20_2800, T0, T0); // uaddlp .8h
                self.v2(0x6E60_2800, T0, T0); // uaddlp .4s
                let xd = self.def_f(d, FS0);
                self.v2(0x6EA0_2800, xd, T0); // uaddlp .2d
                self.fin_f(d, xd);
            }
            VX::PackUsH | VX::PackSsH | VX::PackSsS | VX::PackUsS => {
                let xa = self.use_f(a, FS0);
                let xb = self.use_f(b, FS1);
                let (lo, hi) = match op {
                    VX::PackSsH => (0x0E21_4800, 0x4E21_4800), // sqxtn(2) .8b/.16b
                    VX::PackUsH => (0x2E21_2800, 0x6E21_2800), // sqxtun(2) .8b/.16b from .8h
                    VX::PackSsS => (0x0E61_4800, 0x4E61_4800), // sqxtn(2) .4h/.8h from .4s
                    _ => (0x2E61_2800, 0x6E61_2800),           // sqxtun(2) .4h/.8h from .4s
                };
                self.v2(lo, T0, xa);
                self.v2(hi, T0, xb);
                let xd = self.def_f(d, FS0);
                self.vmov(xd, T0);
                self.fin_f(d, xd);
            }
            VX::ByteShuffle => {
                // tbl gives 0 for indices >= 16: keep bit 7 and the low 4 bits
                let xa = self.use_f(a, FS0);
                let xb = self.use_f(b, FS1);
                self.movi16b(T0, 0x8f);
                self.v3(AND, T0, xb, T0);
                let xd = self.def_f(d, FS0);
                self.v3(0x4E00_0000, xd, xa, T0); // tbl
                self.fin_f(d, xd);
            }
            VX::Abs(w) => {
                let xa = self.use_f(a, FS0);
                let xd = self.def_f(d, FS0);
                self.v2(0x4E20_B800 | (w as u32) << 22, xd, xa);
                self.fin_f(d, xd);
            }
            VX::ShlI(w, k) | VX::ShrUI(w, k) | VX::ShrSI(w, k) => {
                let xa = self.use_f(a, FS0);
                let xd = self.def_f(d, FS0);
                let es = esize(w);
                let k = k as u32;
                match op {
                    VX::ShlI(..) if k >= es => self.a.e(0x6F00_E400 | xd as u32), // movi 0
                    VX::ShlI(..) if k == 0 => self.vmov(xd, xa),
                    VX::ShlI(..) => self.v2(0x4F00_5400 | (es + k) << 16, xd, xa),
                    VX::ShrUI(..) if k >= es => self.a.e(0x6F00_E400 | xd as u32),
                    VX::ShrUI(..) if k == 0 => self.vmov(xd, xa),
                    VX::ShrUI(..) => self.v2(0x6F00_0400 | (2 * es - k) << 16, xd, xa),
                    _ if k == 0 => self.vmov(xd, xa),
                    _ => self.v2(0x4F00_0400 | (2 * es - k.min(es)) << 16, xd, xa),
                }
                self.fin_f(d, xd);
            }
            VX::ShufS(k) => {
                // tbl with the byte indices of the selected 32-bit lanes
                let xa = self.use_f(a, FS0);
                let mut lo = 0u64;
                let mut hi = 0u64;
                for lane in 0..4u64 {
                    let sel = ((k as u64) >> (2 * lane)) & 3;
                    for byte in 0..4u64 {
                        let idx = sel * 4 + byte;
                        let pos = lane * 4 + byte;
                        if pos < 8 {
                            lo |= idx << (8 * pos);
                        } else {
                            hi |= idx << (8 * (pos - 8));
                        }
                    }
                }
                self.const128(T0, lo, hi);
                let xd = self.def_f(d, FS0);
                self.v3(0x4E00_0000, xd, xa, T0);
                self.fin_f(d, xd);
            }
            VX::BytesShrI(k) | VX::BytesShlI(k) => {
                let xa = self.use_f(a, FS0);
                self.a.e(0x6F00_E400 | T0 as u32); // movi T0, #0
                let xd = self.def_f(d, FS0);
                let k = (k as u32).min(16);
                if k == 16 {
                    self.vmov(xd, T0);
                } else if let VX::BytesShrI(_) = op {
                    // ext d, a, zero, #k: bytes k.. of a, then zeros
                    self.v3(0x6E00_0000 | k << 11, xd, xa, T0);
                } else if k == 0 {
                    self.vmov(xd, xa);
                } else {
                    // ext d, zero, a, #(16-k)
                    self.v3(0x6E00_0000 | (16 - k) << 11, xd, T0, xa);
                }
                self.fin_f(d, xd);
            }
            VX::WidenLoU(w) | VX::WidenHiU(w) | VX::WidenLoS(w) | VX::WidenHiS(w) => {
                let xa = self.use_f(a, FS0);
                let xd = self.def_f(d, FS0);
                let immh = 1u32 << (w as u32); // B 0001, H 0010, S 0100
                let base = match op {
                    VX::WidenLoU(_) => 0x2F00_A400,
                    VX::WidenHiU(_) => 0x6F00_A400,
                    VX::WidenLoS(_) => 0x0F00_A400,
                    _ => 0x4F00_A400,
                };
                self.v2(base | immh << 19, xd, xa);
                self.fin_f(d, xd);
            }
            VX::PairAddLongU(w) => {
                let xa = self.use_f(a, FS0);
                let xd = self.def_f(d, FS0);
                self.v2(0x6E20_2800 | (w as u32) << 22, xd, xa);
                self.fin_f(d, xd);
            }
            VX::Splat(w) => {
                let ra = self.use_i(a, S0);
                let xd = self.def_f(d, FS0);
                self.v2(0x4E00_0C00 | (1u32 << (w as u32)) << 16, xd, ra);
                self.fin_f(d, xd);
            }
            VX::FromLo64 => {
                let ra = self.use_i(a, S0);
                let xd = self.def_f(d, FS0);
                self.v2(0x9E67_0000, xd, ra); // fmov d, x
                self.fin_f(d, xd);
            }
            VX::Extract(w, k) => {
                let xa = self.use_f(a, FS0);
                let rd = self.def_i(d, S0);
                let sz = w as u32;
                let imm5 = ((k as u32) << (sz + 1)) | (1 << sz);
                let q = if w == VW::D { 0x4000_0000 } else { 0 };
                self.v2(0x0E00_3C00 | q | imm5 << 16, rd, xa); // umov
                self.fin_i(d, rd);
            }
            VX::Insert(w, k) => {
                let xa = self.use_f(a, FS0);
                let rb = self.use_i(b, S0);
                let xd = self.def_f(d, FS0);
                self.vmov(xd, xa);
                let sz = w as u32;
                let imm5 = ((k as u32) << (sz + 1)) | (1 << sz);
                self.v2(0x4E00_1C00 | imm5 << 16, xd, rb); // ins
                self.fin_f(d, xd);
            }
            VX::MoveMaskB | VX::MoveMask(VW::B) => {
                // top bit of each byte -> 0/1 bytes; gather each half with a multiply
                let xa = self.use_f(a, FS0);
                self.v2(0x6F09_0400, T0, xa); // ushr .16b, #7
                self.v2(0x4E08_3C00, IP0, T0); // umov x16, v.d[0]
                self.v2(0x4E18_3C00, IP1, T0); // umov x17, v.d[1]
                self.a.mov_imm(S1, 0x0102_0408_1020_4080u64 as i64);
                self.a.rrr(0x9B00_7C00, IP0, IP0, S1); // mul
                self.a.rrr(0x9B00_7C00, IP1, IP1, S1);
                let rd = self.def_i(d, S0);
                self.a.e(0xD378_FC00 | (IP0 as u32) << 5 | IP0 as u32); // lsr x16, x16, #56
                self.a.e(0xD378_FC00 | (IP1 as u32) << 5 | IP1 as u32);
                self.a.e(0xAA00_2000 | (IP1 as u32) << 16 | (IP0 as u32) << 5 | rd as u32); // orr rd, x16, x17, lsl #8
                self.fin_i(d, rd);
            }
            VX::AddAcross(w) | VX::MaxAcrossU(w) => {
                let xa = self.use_f(a, FS0);
                let rd = self.def_i(d, S0);
                if w == VW::D {
                    // addp d, v.2d (no u64 max across: the table never asks)
                    self.v2(0x5EF1_B800, T0, xa);
                    self.v2(0x9E66_0000, rd, T0); // fmov x, d
                } else {
                    let base = if let VX::AddAcross(_) = op { 0x4E31_B800 } else { 0x6E30_A800 };
                    self.v2(base | (w as u32) << 22, T0, xa);
                    let sz = w as u32;
                    self.v2(0x0E00_3C00 | (1u32 << sz) << 16, rd, T0); // umov lane 0
                }
                self.fin_i(d, rd);
            }
            _ => unreachable!("vx {:?}", op),
        }
    }

    /// movi v.16b, #imm8
    fn movi16b(&mut self, x: u8, imm: u8) {
        let i = imm as u32;
        self.a.e(0x4F00_E400 | (i >> 5) << 16 | (i & 31) << 5 | x as u32);
    }

    /// a 128-bit constant through the general scratch registers
    fn const128(&mut self, x: u8, lo: u64, hi: u64) {
        self.a.mov_imm(IP0, lo as i64);
        self.v2(0x4E08_1C00, x, IP0); // ins v.d[0], x16
        self.a.mov_imm(IP0, hi as i64);
        self.v2(0x4E18_1C00, x, IP0); // ins v.d[1], x16
    }

    /// 128-bit vector instructions (NEON, 4 x 32-bit lanes; FPCR as the scalar code).
    fn vinst(&mut self, i: &Inst) {
        use v4s::*;
        match *i {
            Inst::VShuffle(d, a, b, idx) => {
                // tbl over the table {v4, v5} = {a, b} (indices >= 32 give 0); one source
                // when no index reaches b
                let one = idx.iter().all(|&i| i < 16 || i >= 32);
                let xa = self.use_f(a, FS0);
                self.vmov(4, xa);
                if !one {
                    let xb = self.use_f(b, FS1);
                    self.vmov(5, xb);
                }
                let lo = u64::from_le_bytes(idx[..8].try_into().unwrap());
                let hi = u64::from_le_bytes(idx[8..].try_into().unwrap());
                self.const128(6, lo, hi);
                let xd = self.def_f(d, FS0);
                self.v3(if one { 0x4E00_0000 } else { 0x4E00_2000 }, xd, 4, 6); // tbl
                self.fin_f(d, xd);
            }
            Inst::VSel(d, m, a, b) => {
                // bsl works in place on the mask: build it in d's register when a and b
                // live elsewhere
                let t = self.vtmp(d, &[a, b]);
                let xm = self.use_f(m, t);
                self.vmov(t, xm);
                let xa = self.use_f(a, FS0);
                let xb = self.use_f(b, FS1);
                self.v3(BSL, t, xa, xb);
                let xd = self.def_f(d, FS2);
                self.vmov(xd, t);
                self.fin_f(d, xd);
            }
            Inst::VFma(np, na, d, a, b, c) => {
                // t = c (negated first for the - c forms: exact, and the fused op then
                // rounds once, with IEEE's sign of an exact zero); fmla / fmls t, a, b
                let t = self.vtmp(d, &[a, b]);
                let xc = self.use_f(c, t);
                if na {
                    self.v2(VFNEG, t, xc);
                } else {
                    self.vmov(t, xc);
                }
                let xa = self.use_f(a, FS0);
                let xb = self.use_f(b, FS1);
                if self.cls(b) == Cls::F32 {
                    // by element (fold_lane_splats): fmla / fmls t.4s, a.4s, b.s[0]
                    self.v3(if np { 0x4F80_5000 } else { 0x4F80_1000 }, t, xa, xb);
                } else {
                    self.v3(if np { 0x4EA0_CC00 } else { 0x4E20_CC00 }, t, xa, xb);
                }
                let xd = self.def_f(d, FS2);
                self.vmov(xd, t);
                self.fin_f(d, xd);
            }
            Inst::VSplat(d, s) => {
                let xd = self.def_f(d, FS0);
                if self.cls(s) == Cls::I {
                    let rs = self.use_i(s, S0);
                    self.a.e(0x4E04_0C00 | (rs as u32) << 5 | xd as u32); // dup vd.4s, wn
                } else {
                    let xs = self.use_f(s, FS1);
                    self.a.e(0x4E04_0400 | (xs as u32) << 5 | xd as u32); // dup vd.4s, vn.s[0]
                }
                self.fin_f(d, xd);
            }
            Inst::VExtract(d, v, lane) => {
                let xv = self.use_f(v, FS0);
                let imm5 = ((lane as u32) << 3) | 4;
                if self.cls(d) == Cls::I {
                    let rd = self.def_i(d, S0);
                    self.a.e(0x0E00_3C00 | imm5 << 16 | (xv as u32) << 5 | rd as u32); // umov
                    self.a.extend(rd, rd, 32, true);
                    self.fin_i(d, rd);
                } else {
                    let xd = self.def_f(d, FS1);
                    self.a.e(0x5E00_0400 | imm5 << 16 | (xv as u32) << 5 | xd as u32); // dup sd, vn.s[l]
                    self.fin_f(d, xd);
                }
            }
            Inst::VInsert(d, v, 0, s) if self.zero_ins.take() == Some(v) || self.zero_vec[v.0 as usize] => {
                // into a zero vector: fmov s (a scalar write zeroes the rest of the register)
                let xd = self.def_f(d, FS2);
                if self.cls(s) == Cls::I {
                    let rs = self.use_i(s, S0);
                    self.a.e(0x1E27_0000 | (rs as u32) << 5 | xd as u32); // fmov sd, wn
                } else {
                    let xs = self.use_f(s, FS1);
                    // an f32 loaded straight into d's register is already [s, 0, 0, 0] (`ldr s`
                    // zeroes the rest of the register)
                    if !(xs == xd && self.f32_load[s.0 as usize]) {
                        self.a.e(0x1E20_4000 | (xs as u32) << 5 | xd as u32); // fmov sd, sn
                    }
                }
                self.fin_f(d, xd);
            }
            Inst::VInsert(d, v, lane, s) => {
                let t = if self.cls(s) == Cls::I { self.vtmp(d, &[]) } else { self.vtmp(d, &[s]) };
                let xv = self.use_f(v, t);
                self.vmov(t, xv);
                let imm5 = ((lane as u32) << 3) | 4;
                if self.cls(s) == Cls::I {
                    let rs = self.use_i(s, S0);
                    self.a.e(0x4E00_1C00 | imm5 << 16 | (rs as u32) << 5 | t as u32); // ins vd.s[l], wn
                } else {
                    let xs = self.use_f(s, FS1);
                    self.a.e(0x6E00_0400 | imm5 << 16 | (xs as u32) << 5 | t as u32); // ins vd.s[l], vn.s[0]
                }
                let xd = self.def_f(d, FS2);
                self.vmov(xd, t);
                self.fin_f(d, xd);
            }
            // a stack-slot or base + k address without a register folds into the access, as
            // for scalar loads / stores ([sp, #k], not `add x15, sp, #k` first)
            Inst::VLoad(d, base, off) => {
                let (rb, o) = self.vbase(base, off as i64);
                let xd = self.def_f(d, FS0);
                self.vldst(true, xd, rb, o);
                self.fin_f(d, xd);
            }
            Inst::VStore(base, off, v) => {
                let (rb, o) = self.vbase(base, off as i64);
                if self.al.loc[v.0 as usize] == Loc::None && self.al.remat[v.0 as usize] == Remat::VConst(0) && o % 8 == 0 && (-512..=496).contains(&o) {
                    // a zero vector: stp xzr, xzr (no movi)
                    self.a.stp(false, ZR, ZR, rb, o as i32);
                    return;
                }
                let xv = self.use_f(v, FS0);
                self.vldst(false, xv, rb, o);
            }
            // [base + (idx << sh) + off]: ldr/str q [rb, ri{, lsl #4}] when nothing is added and the
            // shift is 0 or 4; else the address into s1 by one shifted add
            Inst::VLoadX(d, base, idx, sh, off) => {
                let rb = self.use_i(base, S1);
                let ri = self.use_i(idx, S0);
                let xd = self.def_f(d, FS0);
                self.vldst_x(true, xd, rb, ri, sh, off as i64);
                self.fin_f(d, xd);
            }
            Inst::VStoreX(base, idx, sh, off, v) => {
                let rb = self.use_i(base, S1);
                let ri = self.use_i(idx, S0);
                let xv = self.use_f(v, FS0);
                self.vldst_x(false, xv, rb, ri, sh, off as i64);
            }
            // a constant without a register is made at its uses (Alloc::remat)
            Inst::VConst(d, _) if self.al.loc[d.0 as usize] == Loc::None => {}
            Inst::VConst(d, bits) => {
                let xd = self.def_f(d, FS0);
                self.vconst_into(xd, bits);
                self.fin_f(d, xd);
            }
            Inst::VGather(d, base, idx) => {
                // per lane: umov w17, vi.s[l]; then lane 0 ldr s, [base, x17, lsl #2] (a whole-register
                // write: no dependency on the target's last value), lanes 1-3 ldr w16 + ins v.s[l], w16.
                // Built in d's own register unless that is the index.
                let rb = self.use_i(base, S1);
                let xi = self.use_f(idx, FS0);
                let t = match self.al.loc[d.0 as usize] {
                    Loc::Flt(x) if x != xi => x,
                    _ => FS2,
                };
                for l in 0..4u32 {
                    let imm5 = (l << 3) | 4;
                    self.a.e(0x0E00_3C00 | imm5 << 16 | (xi as u32) << 5 | IP1 as u32);
                    if l == 0 {
                        self.a.e(0xBC60_7800 | (IP1 as u32) << 16 | (rb as u32) << 5 | t as u32);
                    } else {
                        self.a.e(0xB860_7800 | (IP1 as u32) << 16 | (rb as u32) << 5 | IP0 as u32);
                        self.a.e(0x4E00_1C00 | imm5 << 16 | (IP0 as u32) << 5 | t as u32);
                    }
                }
                let xd = self.def_f(d, t);
                self.vmov(xd, t);
                self.fin_f(d, xd);
            }
            Inst::VTbl(d, base, bidx, n) => {
                // ld1 {v4.16b - v(3 + k).16b}, [base] (k = the table's 16-byte registers; v4-v7 are
                // unallocated scratch), tbl d.16b, {v4 ..}, bidx.16b
                let rb = self.use_i(base, S1);
                let k = (n as u32).div_ceil(4).clamp(1, 4);
                let op = [0b0111u32, 0b1010, 0b0110, 0b0010][k as usize - 1];
                self.a.e(0x4C40_0000 | op << 12 | (rb as u32) << 5 | 4);
                let xi = self.use_f(bidx, FS0);
                let xd = self.def_f(d, FS1);
                self.a.e(0x4E00_0000 | (xi as u32) << 16 | (k - 1) << 13 | 4 << 5 | xd as u32);
                self.fin_f(d, xd);
            }
            Inst::VAny(d, v) => {
                let xv = self.use_f(v, FS0);
                self.a.e(0x6EB0_A800 | (xv as u32) << 5 | FS2 as u32); // umaxv s, v.4s
                let rd = self.def_i(d, S0);
                self.a.fmov_to_gp(false, rd, FS2);
                self.cmp_imm(rd, 0);
                self.a.cset(rd, NE);
                self.fin_i(d, rd);
            }
            _ => {}
        }
    }

    /// `t = a <<|>> k; d = x op t` (t used once) as one shifted-register instruction. None: no match.
    /// `emit` false: only whether the pair matches (the loop defers it to the next instruction).
    fn shifted_operand(&mut self, options: &relax_session::Options, sh: &Inst, next: &Inst, emit: bool) -> Option<()> {
        let (t, a, k, kind) = match sh {
            Inst::IBinI(IOp::Shl, it, t, a, k) if it.bits == 64 && (1..64).contains(k) => (*t, *a, *k as u32, 0u32),
            Inst::IBinI(IOp::Shr, it, t, a, k) if it.bits == 64 && (1..64).contains(k) => (*t, *a, *k as u32, if it.signed { 2 } else { 1 }),
            _ => return None,
        };
        if self.al.uses[t.0 as usize] != 1 || isel_flags(options) & A64_FUSE == 0 {
            return None;
        }
        let (op, d, x) = match next {
            Inst::IBin(op @ (IOp::Add | IOp::Sub | IOp::And | IOp::Or | IOp::Xor), it, d, x, y) if it.bits == 64 && *y == t && *x != t => (*op, *d, *x),
            Inst::IBin(op @ (IOp::Add | IOp::And | IOp::Or | IOp::Xor), it, d, x, y) if it.bits == 64 && *x == t && *y != t => (*op, *d, *y),
            _ => return None,
        };
        let base = match op {
            IOp::Add => 0x8B00_0000u32,
            IOp::Sub => 0xCB00_0000,
            IOp::And => 0x8A00_0000,
            IOp::Or => 0xAA00_0000,
            _ => 0xCA00_0000,
        };
        if !emit {
            return Some(());
        }
        let rx = self.use_i(x, S0);
        let ra = self.use_i(a, S1);
        let rd = self.def_i(d, S0);
        self.a.e(base | kind << 22 | (ra as u32) << 16 | k << 10 | (rx as u32) << 5 | rd as u32);
        self.fin_i(d, rd);
        Some(())
    }

    /// `t = a * b; d = c + t` (or `t + c`) is `madd d, a, b, c`, `d = c - t` is `msub` (64-bit, t
    /// used once; b may be an immediate without a cheaper shift form, built in ip0). None: no match.
    /// `emit` false: only whether the pair matches.
    fn mul_add(&mut self, options: &relax_session::Options, m: &Inst, next: &Inst, emit: bool) -> Option<()> {
        let (t, a, b) = match *m {
            Inst::IBin(IOp::Mul, it, t, a, b) if it.bits == 64 && a != t && b != t => (t, a, Ok(b)),
            Inst::IBinI(IOp::Mul, it, t, a, k) if it.bits == 64 && a != t && !(k > 0 && ((k & (k - 1)) == 0 || mul_shift_add(k).is_some())) => (t, a, Err(k)),
            _ => return None,
        };
        if self.al.uses[t.0 as usize] != 1 || isel_flags(options) & A64_FUSE == 0 {
            return None;
        }
        // an indexed access whose index is the product (no shift, a register base): the address
        // by madd into ip1, then [ip1 + off]
        let reg_base = |g: &Self, v: VReg| v != t && g.slot_base(v).is_none() && g.resolve_off(v, 0).0 == v;
        match *next {
            Inst::LoadX(mm, d, base, x, 0, off) if x == t && reg_base(self, base) && (off as i64).unsigned_abs() < 1 << 24 => {
                if !emit {
                    return Some(());
                }
                let rc = self.use_i(base, S0);
                let ra = self.use_i(a, S1);
                let rb = self.mul_operand(b);
                self.a.e(0x9B00_0000 | (rb as u32) << 16 | (rc as u32) << 10 | (ra as u32) << 5 | IP1 as u32);
                let (op, size) = mem_op(mm, true);
                let off = self.ip1_off(off as i64, size);
                match mm {
                    Mem::Int(..) => {
                        let rd = self.def_i(d, S0);
                        self.a.ldst(op, size, rd, IP1, off);
                        self.fin_i(d, rd);
                    }
                    _ => {
                        let xd = self.def_f(d, FS0);
                        self.a.ldst(op, size, xd, IP1, off);
                        self.fin_f(d, xd);
                    }
                }
                return Some(());
            }
            Inst::StoreX(mm, base, x, 0, off, src) if x == t && reg_base(self, base) && src != t && (off as i64).unsigned_abs() < 1 << 24 => {
                if !emit {
                    return Some(());
                }
                let (op, size) = mem_op(mm, false);
                let rs = match mm {
                    Mem::Int(..) => self.use_stored(src, S0),
                    _ => self.use_f(src, FS0),
                };
                let rc = self.use_i(base, S1);
                let ra = self.use_i(a, IP1);
                let rb = self.mul_operand(b);
                self.a.e(0x9B00_0000 | (rb as u32) << 16 | (rc as u32) << 10 | (ra as u32) << 5 | IP1 as u32);
                let off = self.ip1_off(off as i64, size);
                self.a.ldst(op, size, rs, IP1, off);
                return Some(());
            }
            _ => {}
        }
        let (sub, d, c) = match *next {
            Inst::IBin(IOp::Add, it, d, x, y) if it.bits == 64 && y == t && x != t => (false, d, x),
            Inst::IBin(IOp::Add, it, d, x, y) if it.bits == 64 && x == t && y != t => (false, d, y),
            Inst::IBin(IOp::Sub, it, d, x, y) if it.bits == 64 && y == t && x != t => (true, d, x),
            _ => return None,
        };
        if !emit {
            return Some(());
        }
        let rc = self.use_i(c, S0);
        let ra = self.use_i(a, S1);
        let rb = self.mul_operand(b);
        let rd = self.def_i(d, S0);
        // madd / msub xd, xa, xb, xc
        self.a.e(if sub { 0x9B00_8000 } else { 0x9B00_0000 } | (rb as u32) << 16 | (rc as u32) << 10 | (ra as u32) << 5 | rd as u32);
        self.fin_i(d, rd);
        Some(())
    }

    /// mul_add's second factor: its register (ip0 when spilled), or the constant built in ip0
    fn mul_operand(&mut self, b: Result<VReg, i64>) -> u8 {
        match b {
            Ok(b) => self.use_i(b, IP0),
            Err(k) => {
                self.a.mov_imm(IP0, k);
                IP0
            }
        }
    }

    /// An offset from ip1 that ldst encodes without ip1 as its offset register (else folded in)
    fn ip1_off(&mut self, off: i64, size: u32) -> i64 {
        if (off >= 0 && off % size as i64 == 0 && off / (size as i64) < 4096) || (-256..256).contains(&off) {
            off
        } else {
            self.a.add_any(IP1, IP1, off);
            0
        }
    }

    /// `t = bswap64(a); d = t >> 32` is `rev wd, wa` (the byte-reversed low word, zero-extended);
    /// `d = t >> 48` with a zero above 16 bits is `rev16 wd, wa`; `t = bitrev64(a); d = t >> 32`
    /// is `rbit wd, wa` (t used once). None: no match.
    fn narrow_bswap(&mut self, options: &relax_session::Options, sw: &Inst, next: &Inst, emit: bool) -> Option<()> {
        let (t, a, bits) = match sw {
            Inst::IBit(BitOp::Bswap, t, a) => (*t, *a, true),
            Inst::IBit(BitOp::Rev, t, a) => (*t, *a, false),
            _ => return None,
        };
        if self.al.uses[t.0 as usize] != 1 || isel_flags(options) & A64_FUSE == 0 {
            return None;
        }
        let (d, op) = match next {
            // rbit w (bit reverse of the low word)
            Inst::IBinI(IOp::Shr, it, d, x, 32) if !bits && *x == t && it.bits == 64 && !it.signed => (*d, 0x5AC0_0000u32),
            Inst::IBinI(IOp::Shr, it, d, x, 32) if bits && *x == t && it.bits == 64 && !it.signed => (*d, 0x5AC0_0800u32),
            Inst::IBinI(IOp::Shr, it, d, x, 48) if bits && *x == t && it.bits == 64 && !it.signed && self.zero_above.get(a.0 as usize).is_some_and(|z| (1..=16).contains(z)) => (*d, 0x5AC0_0400),
            _ => return None,
        };
        if !emit {
            return Some(());
        }
        let ra = self.use_i(a, S0);
        let rd = self.def_i(d, S0);
        self.a.e(op | (ra as u32) << 5 | rd as u32);
        self.fin_i(d, rd);
        Some(())
    }

    /// `Load a, [p + o]; Load b, [p + o +- w]` (or two stores) of the same width w (8/4-byte ints,
    /// f64/f32, 16-byte vectors; 4-byte loads zero-extending) as one ldp / stp: both values in registers (no spill
    /// windows), distinct, the loads' destinations not the base, the lower offset a multiple of w
    /// within the signed 7-bit scaled range. None: no match.
    fn pair_access(&mut self, options: &relax_session::Options, i1: &Inst, i2: &Inst, pos2: u32) -> Option<()> {
        if isel_flags(options) & A64_FUSE == 0 || self.win_start.contains_key(&pos2) || self.evict_at.contains_key(&pos2) {
            return None;
        }
        let (load, m1, v1, b1, o1, m2, v2, b2, o2) = match (i1, i2) {
            (Inst::Load(m1, d1, b1, o1), Inst::Load(m2, d2, b2, o2)) => (true, *m1, *d1, *b1, *o1, *m2, *d2, *b2, *o2),
            (Inst::Store(m1, b1, o1, s1), Inst::Store(m2, b2, o2, s2)) => (false, *m1, *s1, *b1, *o1, *m2, *s2, *b2, *o2),
            // 16-byte vectors: ldp / stp q (Mem::F64 stands in for "float register"; the width is 16)
            (Inst::VLoad(d1, b1, o1), Inst::VLoad(d2, b2, o2)) => (true, V128_MEM, *d1, *b1, *o1, V128_MEM, *d2, *b2, *o2),
            (Inst::VStore(b1, o1, s1), Inst::VStore(b2, o2, s2)) => (false, V128_MEM, *s1, *b1, *o1, V128_MEM, *s2, *b2, *o2),
            _ => return None,
        };
        let vec = matches!(i1, Inst::VLoad(..) | Inst::VStore(..));
        // two zero stores of one width pair as `stp wzr / xzr` whatever their types
        let zeros = !load && !vec && self.zero_val[v1.0 as usize] && self.zero_val[v2.0 as usize] && mem_op(m1, false).1 == mem_op(m2, false).1;
        if !zeros && m1 != m2 || b1 != b2 || load && v1 == v2 {
            return None;
        }
        // (pair opcode, width, float?)
        let (op, w, fp) = match (m1, load) {
            _ if zeros => match mem_op(m1, false).1 {
                8 => (0xA900_0000u32, 8i64, false),
                4 => (0x2900_0000, 4, false),
                _ => return None,
            },
            _ if vec => (if load { 0xAD40_0000u32 } else { 0xAD00_0000 }, 16i64, true),
            (Mem::Int(8, _), true) => (0xA940_0000u32, 8i64, false),
            (Mem::Int(8, _), false) => (0xA900_0000, 8, false),
            (Mem::Int(4, false), true) => (0x2940_0000, 4, false),
            (Mem::Int(4, _), false) => (0x2900_0000, 4, false),
            (Mem::F64, true) => (0x6D40_0000, 8, true),
            (Mem::F64, false) => (0x6D00_0000, 8, true),
            (Mem::F32, true) => (0x2D40_0000, 4, true),
            (Mem::F32, false) => (0x2D00_0000, 4, true),
            _ => return None,
        };
        if load && (v1 == b1 || v2 == b1) {
            return None;
        }
        let (base, off1) = self.resolve_off(b1, o1 as i64);
        let (base2, off2) = self.resolve_off(b2, o2 as i64);
        if base != base2 {
            return None;
        }
        let (lo_v, hi_v, lo) = if off2 == off1 + w {
            (v1, v2, off1)
        } else if off1 == off2 + w {
            (v2, v1, off2)
        } else {
            return None;
        };
        let rb = match self.slot_base(base) {
            Some(so) => {
                let o = so + lo;
                if o % w != 0 || !(-64 * w..=63 * w).contains(&o) {
                    return None;
                }
                (SP, o)
            }
            None => match self.al.loc[base.0 as usize] {
                Loc::Reg(r) if lo % w == 0 && (-64 * w..=63 * w).contains(&lo) => (r, lo),
                _ => return None,
            },
        };
        let reg_of = |s: &Self, v: VReg| match (s.al.loc[v.0 as usize], fp) {
            _ if zeros => Some(ZR),
            // an integer zero store: zr (its register may not even hold it here)
            (_, false) if !load && s.zero_val[v.0 as usize] => Some(ZR),
            (Loc::Reg(r), false) | (Loc::Flt(r), true) => Some(r),
            _ => None,
        };
        let (t1, t2) = (reg_of(self, lo_v)?, reg_of(self, hi_v)?);
        if load && t1 == t2 {
            return None;
        }
        let imm = ((rb.1 / w) as u32) & 0x7f;
        self.a.e(op | imm << 15 | (t2 as u32) << 10 | (rb.0 as u32) << 5 | t1 as u32);
        Some(())
    }

    /// Instructions `ii..` that store zero (zero_values) off one base (after resolve_off), with no
    /// region window starting among them: when there are at least three, their byte ranges are
    /// stored with the widest zr stores (stp xzr, xzr per 16 bytes; a zeroed struct's 4-byte
    /// fields were one `str wzr` each, its vectors a movi + str q) and the count is returned.
    fn zero_run(&mut self, bi: usize, ii: usize, limit: usize) -> usize {
        let b = &self.f.blocks[bi];
        let zero = |s: &Self, v: VReg| s.zero_val[v.0 as usize] || s.al.loc[v.0 as usize] == Loc::None && matches!(s.al.remat[v.0 as usize], Remat::Const(0) | Remat::FConst(0, _) | Remat::VConst(0));
        let mut root: Option<VReg> = None;
        let mut spans: Vec<(i64, i64)> = Vec::new();
        let mut k = ii;
        while k < limit {
            let (base, off, size) = match b.insts[k] {
                Inst::Store(m, base, off, v) if zero(self, v) => (base, off as i64, mem_op(m, false).1 as i64),
                Inst::VStore(base, off, v) if zero(self, v) => (base, off as i64, 16),
                _ => break,
            };
            if k > ii && self.win_start.contains_key(&self.al.inst_pos(bi, k)) {
                break;
            }
            let (r, o) = self.resolve_off(base, off);
            if root.is_some_and(|x| x != r) {
                break;
            }
            root = Some(r);
            spans.push((o, o + size));
            k += 1;
        }
        if k - ii < 3 {
            return 0;
        }
        let root = root.unwrap();
        let (rb, add) = match self.slot_base(root) {
            Some(so) => (SP, so),
            None => (self.use_i(root, S1), 0),
        };
        spans.sort();
        let mut merged: Vec<(i64, i64)> = Vec::new();
        for (lo, hi) in spans {
            match merged.last_mut() {
                Some(m) if lo <= m.1 => m.1 = m.1.max(hi),
                _ => merged.push((lo, hi)),
            }
        }
        for (lo, hi) in merged {
            let (mut p, e) = (lo + add, hi + add);
            while p < e {
                let left = e - p;
                if left >= 16 && p % 8 == 0 && (-512..=504).contains(&p) {
                    self.a.stp(false, ZR, ZR, rb, p as i32);
                    p += 16;
                } else if left >= 8 && p % 8 == 0 {
                    self.a.ldst(STRX, 8, ZR, rb, p);
                    p += 8;
                } else if left >= 8 && p % 4 == 0 && (-256..=252).contains(&p) {
                    self.a.e(0x2900_0000 | (((p / 4) as u32) & 0x7f) << 15 | (ZR as u32) << 10 | (rb as u32) << 5 | ZR as u32);
                    p += 8;
                } else if left >= 4 && p % 4 == 0 {
                    self.a.ldst(STRW, 4, ZR, rb, p);
                    p += 4;
                } else if left >= 2 && p % 2 == 0 {
                    self.a.ldst(STRH, 2, ZR, rb, p);
                    p += 2;
                } else {
                    self.a.ldst(STRB, 1, ZR, rb, p);
                    p += 1;
                }
            }
        }
        k - ii
    }

    /// Sets the flags for a fused compare and returns the condition for "true".
    fn cmp_flags(&mut self, i: &Inst) -> u8 {
        match i {
            Inst::ICmp(c, signed, _, a, b) => {
                let ra = self.use_i(*a, S0);
                let rb = self.use_i(*b, S1);
                self.a.cmp(ra, rb);
                icc(*c, *signed)
            }
            Inst::ICmpI(c, signed, _, a, v) => {
                let ra = self.use_i(*a, S0);
                self.cmp_imm(ra, *v);
                icc(*c, *signed)
            }
            Inst::FCmp(c, f64_, _, a, b) => fcc(self.fcmp_v(*c, *f64_, *a, *b)),
            _ => NE,
        }
    }

    /// fcmp a, b for `a c b`; against a register-less +0.0 (either side) the zero form (`fcmp s,
    /// #0.0`, no `fmov d31, xzr`). Returns the condition on the flags (swapped with the operands:
    /// a < b == b > a, also for unordered operands).
    fn fcmp_v(&mut self, c: Cond, f64_: bool, a: VReg, b: VReg) -> Cond {
        let zero = |g: &Self, v: VReg| g.zero_cmp[v.0 as usize] || (g.al.loc[v.0 as usize] == Loc::None && matches!(g.al.remat[v.0 as usize], Remat::FConst(0, _)));
        let (x, c) = if zero(self, b) {
            (a, c)
        } else if zero(self, a) {
            (b, match c {
                Cond::Lt => Cond::Gt,
                Cond::Le => Cond::Ge,
                Cond::Gt => Cond::Lt,
                Cond::Ge => Cond::Le,
                c => c,
            })
        } else {
            let xa = self.use_f(a, FS0);
            let xb = self.use_f(b, FS1);
            self.a.fcmp(f64_, xa, xb);
            return c;
        };
        let xx = if self.zero_cmp[x.0 as usize] {
            // both sides the unbuilt +0.0
            self.fconst_into(FS0, 0, f64_);
            FS0
        } else {
            self.use_f(x, FS0)
        };
        self.a.e(0x1E20_2008 | if f64_ { DBL } else { 0 } | (xx as u32) << 5);
        c
    }

    fn cmp_imm(&mut self, ra: u8, v: i64) {
        if (0..4096).contains(&v) {
            // cmp xn, #imm (subs xzr)
            self.a.e(0xF100_001F | (v as u32) << 10 | (ra as u32) << 5);
        } else if (-4095..0).contains(&v) {
            // cmn xn, #-imm
            self.a.e(0xB100_001F | ((-v) as u32) << 10 | (ra as u32) << 5);
        } else {
            self.a.mov_imm(S1, v);
            self.a.cmp(ra, S1);
        }
    }

    /// A conditional branch to a block: short (imm19), or in long mode the inverted condition
    /// skipping a `b` to the block.
    fn cond_branch(&mut self, c: CondBr, target: u32) {
        if !self.long {
            let p = match c {
                CondBr::B(cc) => self.a.bcond(cc),
                CondBr::Z(r) => self.a.cbz(r),
                CondBr::Nz(r) => self.a.cbnz(r),
                CondBr::Tz(r, bit) => self.a.tbz(false, r, bit),
                CondBr::Tnz(r, bit) => self.a.tbz(true, r, bit),
            };
            self.fixups.push((p, target));
            return;
        }
        let skip = match c {
            CondBr::B(cc) => self.a.bcond(cc ^ 1),
            CondBr::Z(r) => self.a.cbnz(r),
            CondBr::Nz(r) => self.a.cbz(r),
            CondBr::Tz(..) | CondBr::Tnz(..) => unreachable!("tbz is not used in long mode"),
        };
        let p = self.a.b();
        self.fixups.push((p, target));
        let here = self.a.pos();
        self.a.patch(skip, here);
    }

    /// A two-way branch: `yes` branches when the condition holds (to `tt`), `no` when it does not
    /// (to `ff`). A target that is the next block falls through. When neither is, the conditional
    /// branch takes the loop's back edge (a target laid out at or before this block) and the
    /// unconditional one the other: one taken branch per iteration instead of a not-taken b.cond
    /// plus a taken b.
    fn branch_pair(&mut self, yes: CondBr, no: CondBr, tt: u32, ff: u32, bi: usize) {
        let next = self.next_block[bi];
        // neither falls through: the conditional branch goes where the code goes more often (a
        // taken b.cond is one instruction, a not-taken one plus the b two). Counted code follows
        // the counts of the two targets when both have one (the estimates of uncounted blocks,
        // inlined callees, are worse than the lowering's then-first order: +0.7% instructions on
        // rust_tokenize); static code takes the back edge, and of two targets laid out before
        // the nearer one (the innermost loop's header; an exit placed before the loop is further
        // back). (MAKEPAD_RUN_OPT=-a64backedge: off, A/B)
        let on = !crate::stats::opt_env_off("-a64backedge");
        let back = |b: u32| b as usize <= bi;
        let (ct, cf) = (self.f.blocks[tt as usize].count, self.f.blocks[ff as usize].count);
        let ff_cond = on && if self.counted { ct >= 0.0 && cf >= 0.0 && cf > ct } else { back(ff) && (!back(tt) || ff > tt) };
        if tt == next || ff != next && ff_cond {
            self.cond_branch(no, ff);
            self.jump_to(tt, bi);
        } else {
            self.cond_branch(yes, tt);
            self.jump_to(ff, bi);
        }
    }

    fn jump_to(&mut self, target: u32, bi: usize) {
        if self.next_block[bi] != target {
            let p = self.a.b();
            self.fixups.push((p, target));
        }
    }

    /// Saves (prologue) or restores the used callee-saved registers at [fp - 8k], k = 1.. (ints
    /// first, then floats: the unwinder's layout), two per stp / ldp where the classes match.
    fn callee_saves(&mut self, save: bool) {
        let mut regs: Vec<(bool, u8)> = self.al.used_callee_int.iter().map(|r| (false, *r)).collect();
        regs.extend(self.al.used_callee_flt.iter().map(|x| (true, *x)));
        let mut k = 0usize;
        while k < regs.len() {
            let (fp, r) = regs[k];
            if k + 1 < regs.len() && regs[k + 1].0 == fp {
                // [fp - 8(k+2)] holds regs[k+1], [fp - 8(k+1)] regs[k]
                let off = -8 * (k as i32 + 2);
                if save {
                    self.a.stp(fp, regs[k + 1].1, r, FP, off);
                } else {
                    self.a.ldp(fp, regs[k + 1].1, r, FP, off);
                }
                k += 2;
            } else {
                let op = match (fp, save) {
                    (false, true) => STRX,
                    (false, false) => LDRX,
                    (true, true) => STRD,
                    (true, false) => LDRD,
                };
                self.a.ldst(op, 8, r, FP, -8 * (k as i64 + 1));
                k += 1;
            }
        }
    }

    fn epilogue(&mut self) {
        self.frame_exit();
        self.a.ret();
    }

    /// The epilogue up to its `ret`: callee-saved registers back, the frame record popped.
    fn frame_exit(&mut self) {
        self.callee_saves(false);
        self.a.mov_sp(SP, FP);
        self.a.ldp_post(FP, LR, SP, 16);
    }

    /// A sibling call (tail::sibling_calls): the arguments into the callee's registers, this
    /// frame gone (lr = this function's return address), then `b` / `br x16`.
    fn tail_call(&mut self, c: &Callee, args: &[VReg]) {
        if let Callee::Indirect(v) | Callee::CIndirect(v, _) = c {
            let r = self.use_i(*v, IP0);
            self.a.mov(IP0, r);
        }
        let mut mv = Vec::new();
        match c {
            Callee::CFn(_, sig) | Callee::CIndirect(_, sig) => {
                let plan = cabi::plan(sig, sig.conv.target());
                for (k, ap) in plan.args.iter().enumerate() {
                    match ap {
                        ArgPlan::Scalar(PLoc::Int(r), _) => mv.push((MLoc::R(*r), self.mloc(args[k]))),
                        ArgPlan::Scalar(PLoc::Flt(x), _) => mv.push((MLoc::F(*x), self.mloc(args[k]))),
                        _ => unreachable!("tail call with a memory argument"),
                    }
                }
            }
            _ => {
                let (mut ni, mut nf) = (0u8, 0u8);
                for a in args {
                    if self.cls(*a) == Cls::I {
                        mv.push((MLoc::R(ni), self.mloc(*a)));
                        ni += 1;
                    } else {
                        mv.push((MLoc::F(nf), self.mloc(*a)));
                        nf += 1;
                    }
                }
            }
        }
        self.moves(&mv);
        self.frame_exit();
        match c {
            Callee::Fn(id) | Callee::CFn(id, _) => {
                let at = self.a.pos();
                self.a.e(0x1400_0000);
                self.relocs.push(RelocAt { at: (at * 4) as u32, kind: RelocKind::A64Branch26, target: Reloc::Call(*id) });
            }
            _ => self.a.br(IP0),
        }
    }

    fn term(&mut self, t: &Term, bi: usize, fused: Option<Inst>) {
        match t {
            Term::Jump(target) => self.jump_to(*target, bi),
            Term::Branch(c, tt, ff) => {
                match fused {
                    // and with a bitmask immediate, compared with zero: tst + b.cond
                    Some(ref fi @ (Inst::ICmpI(Cond::Eq | Cond::Ne, _, _, _, 0) | Inst::IBinI(IOp::And, ..))) if self.test_fuse.is_some() => {
                        // (x & K) != 0 for a branch straight on the and
                        let cnd = match fi {
                            Inst::ICmpI(c, ..) => *c,
                            _ => Cond::Ne,
                        };
                        let (src, mask) = self.test_fuse.take().unwrap();
                        let rs = self.use_i(src, S0);
                        if self.tbz && (mask as u64).is_power_of_two() {
                            // one bit: tbz / tbnz (branch to tt when the bit is zero == Eq)
                            let bit = (mask as u64).trailing_zeros();
                            let zero_to_tt = cnd == Cond::Eq;
                            let (yes, no) = if zero_to_tt { (CondBr::Tz(rs, bit), CondBr::Tnz(rs, bit)) } else { (CondBr::Tnz(rs, bit), CondBr::Tz(rs, bit)) };
                            self.branch_pair(yes, no, *tt, *ff, bi);
                            return;
                        }
                        let (nn, immr, imms) = bitmask_imm(mask as u64).unwrap();
                        // ANDS xzr, xs, #mask
                        self.a.e(0xF200_001F | nn << 22 | immr << 16 | imms << 10 | (rs as u32) << 5);
                        let cc = if cnd == Cond::Eq { EQ } else { NE };
                        self.branch_pair(CondBr::B(cc), CondBr::B(cc ^ 1), *tt, *ff, bi);
                    }
                    // compare with zero for (in)equality: cbz / cbnz, no flags
                    Some(Inst::ICmpI(cnd @ (Cond::Eq | Cond::Ne), _, _, a, 0)) => {
                        let r = self.use_i(a, S0);
                        // branch to tt when (r == 0) == (cnd == Eq)
                        let zero_to_tt = cnd == Cond::Eq;
                        let (yes, no) = if zero_to_tt { (CondBr::Z(r), CondBr::Nz(r)) } else { (CondBr::Nz(r), CondBr::Z(r)) };
                        self.branch_pair(yes, no, *tt, *ff, bi);
                    }
                    Some(i) => {
                        let cc = self.cmp_flags(&i);
                        // the condition codes come in true/false pairs: cc ^ 1 negates
                        self.branch_pair(CondBr::B(cc), CondBr::B(cc ^ 1), *tt, *ff, bi);
                    }
                    None => {
                        let r = self.use_i(*c, S0);
                        self.branch_pair(CondBr::Nz(r), CondBr::Z(r), *tt, *ff, bi);
                    }
                }
            }
            Term::Switch(x, tab) => {
                // idx = x - min; if idx >u len - 1 { default }; br table[idx]
                let r = self.use_i(*x, S0);
                let idx = if tab.min == 0 {
                    r
                } else if (1..4096).contains(&tab.min) {
                    self.a.sub_imm(IP0, r, tab.min as u32);
                    IP0
                } else {
                    self.a.mov_imm(IP0, tab.min);
                    self.a.rrr(0xCB00_0000, IP0, r, IP0); // sub x16, r, x16
                    IP0
                };
                // unless every way in bounds it (switch_in_range)
                if !self.switch_in_range.get(bi).copied().unwrap_or(false) {
                    self.cmp_imm(idx, tab.targets.len() as i64 - 1);
                    self.cond_branch(CondBr::B(HI), tab.default);
                }
                self.a.e(0x1000_0000 | (4 << 5) | IP1 as u32); // adr x17, .+16
                self.a.e(0xB8A0_7800 | (idx as u32) << 16 | (IP1 as u32) << 5 | IP0 as u32); // ldrsw x16, [x17, idx, lsl #2]
                self.a.rrr(0x8B00_0000, IP1, IP1, IP0); // add x17, x17, x16
                self.a.e(0xD61F_0000 | (IP1 as u32) << 5); // br x17
                let start = self.a.pos();
                for &t in &tab.targets {
                    let at = self.a.pos();
                    self.a.e(0);
                    self.jt_fix.push((at, start, t));
                }
            }
            Term::Ret(vals) => {
                self.c_ret_parts();
                let mut mv = Vec::new();
                let mut ni = 0u8;
                let mut nf = 0u8;
                for v in vals {
                    if self.cls(*v) == Cls::I {
                        mv.push((MLoc::R(ni), self.mloc(*v)));
                        ni += 1;
                    } else {
                        mv.push((MLoc::F(nf), self.mloc(*v)));
                        nf += 1;
                    }
                }
                self.moves(&mv);
                self.epilogue();
            }
            Term::Unreachable => self.a.udf(),
            Term::Invoke(normal, unwind) => {
                // the call was the block's last instruction: unwinding returns to a landing
                // stub (see gen) that reloads what the call's normal path reloads
                let saves = self.al.saves.get(&self.cur_pos).cloned().unwrap_or_default();
                self.pads.push((self.last_call_ret, saves, *unwind));
                self.jump_to(*normal, bi);
            }
            Term::Resume => {
                self.call_far(Reloc::Resume);
                self.a.udf();
            }
        }
    }

    fn inst(&mut self, _options: &relax_session::Options, i: &Inst) {
        match i {
            // recomputed at each use (Alloc::remat)
            Inst::Iconst(d, _) | Inst::Fconst(d, _, _) | Inst::SlotAddr(d, _) | Inst::Addr(d, _) | Inst::FnAddr(d, _) | Inst::Mov(d, _) | Inst::IBinI(IOp::Add, _, d, _, _) if self.al.remat[d.0 as usize].is_some() => {}
            // pure values nothing reads (a constant whose only use became an immediate form, a
            // copy into a dead variable): not built (a hoisted 64-bit mask was 4 instructions)
            Inst::Iconst(d, _) | Inst::Fconst(d, _, _) | Inst::VConst(d, _) | Inst::SlotAddr(d, _) | Inst::Addr(d, _) | Inst::FnAddr(d, _) | Inst::Mov(d, _) | Inst::IBinI(_, _, d, _, _) | Inst::Conv(Conv::IntToInt(_), d, _)
                if self.al.uses[d.0 as usize] == 0 => {}
            Inst::Iconst(d, v) => {
                let r = self.def_i(*d, S0);
                self.a.mov_imm(r, *v);
                self.fin_i(*d, r);
            }
            // a +0.0 only compares read: they use the zero form (fcmp_v)
            Inst::Fconst(d, ..) if self.zero_cmp[d.0 as usize] => {}
            Inst::Fconst(d, bits, f64_) => {
                let x = self.def_f(*d, FS0);
                self.fconst_into(x, *bits, *f64_);
                self.fin_f(*d, x);
            }
            // pieces of one split value sharing a slot
            Inst::Mov(d, s) if matches!(self.al.loc[d.0 as usize], Loc::Stack(_)) && self.al.loc[d.0 as usize] == self.al.loc[s.0 as usize] => {}
            Inst::Mov(d, s) => {
                // (a spilled source a window holds here is read from the window's register: use_i)
                if self.cls(*d) == Cls::I && self.win_reg(*s).is_none() && !self.copy_reg.contains_key(&(self.cur_pos, s.0)) && matches!((self.al.loc[d.0 as usize], self.al.loc[s.0 as usize]), (Loc::Stack(_), Loc::Reg(_)) | (Loc::Reg(_), Loc::Stack(_))) {
                    // straight between a register and a spill slot
                    match (self.al.loc[d.0 as usize], self.al.loc[s.0 as usize]) {
                        (Loc::Stack(o), Loc::Reg(r)) => {
                            if !self.al.store_skip.contains(&(self.cur_pos, d.0)) {
                                let off = self.spill_sp(o);
                                self.a.ldst(STRX, 8, r, SP, off);
                            }
                            self.def_win_copy(*d, r);
                        }
                        (Loc::Reg(r), Loc::Stack(o)) => {
                            let off = self.spill_sp(o);
                            self.a.ldst(LDRX, 8, r, SP, off);
                        }
                        _ => {}
                    }
                } else if self.cls(*d) == Cls::I {
                    let rs = self.use_i(*s, S0);
                    let rd = self.def_i(*d, S0);
                    self.a.mov(rd, rs);
                    self.fin_i(*d, rd);
                } else {
                    // a spilled source loads straight into d's register; a spilled d stores
                    // straight from s's (no move through the scratch)
                    let xd = self.def_f(*d, FS0);
                    let xs = self.use_f(*s, xd);
                    if matches!(self.al.loc[d.0 as usize], Loc::Stack(_)) {
                        self.fin_f(*d, xs);
                    } else {
                        if self.cls(*d) == Cls::V128 {
                            self.vmov(xd, xs);
                        } else {
                            self.a.fmov(xd, xs);
                        }
                        self.fin_f(*d, xd);
                    }
                }
            }
            Inst::IBin(op, it, d, a, b) => {
                let ra = self.use_i(*a, S0);
                let rb = self.use_i(*b, S1);
                let rd = self.def_i(*d, S0);
                let ext = !self.low_only[d.0 as usize];
                self.ibin_rr(*op, *it, rd, ra, rb, ext);
                self.fin_i(*d, rd);
            }
            Inst::IBinI(op, it, d, a, imm) => self.ibin_imm(*op, *it, *d, *a, *imm),
            Inst::IBit(o, d, a) => {
                let ra = self.use_i(*a, S0);
                let rd = self.def_i(*d, S0);
                match o {
                    BitOp::Clz => self.a.e(0xDAC0_1000 | (ra as u32) << 5 | rd as u32),
                    BitOp::Bswap => self.a.e(0xDAC0_0C00 | (ra as u32) << 5 | rd as u32),
                    BitOp::Rev => self.a.e(0xDAC0_0000 | (ra as u32) << 5 | rd as u32),
                    BitOp::Ctz => {
                        // rbit + clz
                        self.a.e(0xDAC0_0000 | (ra as u32) << 5 | rd as u32);
                        self.a.e(0xDAC0_1000 | (rd as u32) << 5 | rd as u32);
                    }
                    BitOp::Popcnt => {
                        // fmov d, x; cnt v.8b; addv b, v.8b; fmov w, s
                        self.a.fmov_from_gp(true, FS2, ra);
                        self.a.e(0x0E20_5800 | (FS2 as u32) << 5 | FS2 as u32);
                        self.a.e(0x0E31_B800 | (FS2 as u32) << 5 | FS2 as u32);
                        self.a.fmov_to_gp(false, rd, FS2);
                    }
                }
                self.fin_i(*d, rd);
            }
            Inst::INeg(it, d, a) | Inst::INot(it, d, a) => {
                let ra = self.use_i(*a, S0);
                let rd = self.def_i(*d, S0);
                if matches!(i, Inst::INeg(..)) {
                    self.a.rrr(0xCB00_0000, rd, ZR, ra);
                } else {
                    self.a.rrr(0xAA20_0000, rd, ZR, ra);
                }
                if !self.low_only[d.0 as usize] {
                    self.a.extend(rd, rd, it.bits, it.signed);
                }
                self.fin_i(*d, rd);
            }
            Inst::ICmp(c, signed, d, a, b) => {
                let ra = self.use_i(*a, S0);
                let rb = self.use_i(*b, S1);
                self.a.cmp(ra, rb);
                let rd = self.def_i(*d, S0);
                self.a.cset(rd, icc(*c, *signed));
                self.fin_i(*d, rd);
            }
            Inst::ICmpI(c, signed, d, a, imm) => {
                let ra = self.use_i(*a, S0);
                self.cmp_imm(ra, *imm);
                let rd = self.def_i(*d, S0);
                self.a.cset(rd, icc(*c, *signed));
                self.fin_i(*d, rd);
            }
            Inst::FBin(op @ (FOp::MinLt | FOp::MaxGt), f64_, d, a, b) => {
                // a < b ? a : b  (fcmp; fcsel mi / gt: unordered picks b)
                let xa = self.use_f(*a, FS0);
                let xb = self.use_f(*b, FS1);
                self.a.fcmp(*f64_, xa, xb);
                let xd = self.def_f(*d, FS0);
                let cond = if *op == FOp::MinLt { MI } else { GT };
                self.a.e(0x1E20_0C00 | if *f64_ { DBL } else { 0 } | (xb as u32) << 16 | (cond as u32) << 12 | (xa as u32) << 5 | xd as u32);
                self.fin_f(*d, xd);
            }
            Inst::FBin(op @ (FOp::Min | FOp::Max), f64_, d, a, b) => {
                // IEEE minimumNumber (Rust min/max): a NaN operand, signaling too, gives the
                // other one. fminnm turns a signaling NaN into a NaN result, so both inputs are
                // quieted first (fminnm x, x, x), as rustc emits
                let xa = self.use_f(*a, FS0);
                let xb = self.use_f(*b, FS1);
                self.a.fp3(FMINNM, *f64_, FS2, xa, xa);
                self.a.fp3(FMINNM, *f64_, FS1, xb, xb);
                let xd = self.def_f(*d, FS0);
                self.a.fp3(if *op == FOp::Min { FMINNM } else { FMAXNM }, *f64_, xd, FS2, FS1);
                self.fin_f(*d, xd);
            }
            Inst::FBin(op, f64_, d, a, b) => {
                let xa = self.use_f(*a, FS0);
                let xb = self.use_f(*b, FS1);
                let xd = self.def_f(*d, FS0);
                let opc = match op {
                    FOp::Add => FADD,
                    FOp::Sub => FSUB,
                    FOp::Mul => FMUL,
                    FOp::Div => FDIV,
                    FOp::Min | FOp::Max | FOp::MinLt | FOp::MaxGt => unreachable!("min/max have their own arms"),
                };
                self.a.fp3(opc, *f64_, xd, xa, xb);
                self.fin_f(*d, xd);
            }
            Inst::FUnary(FUn::Neg, f64_, d, _) if self.fnmul_ins.is_some() => {
                let (a, b) = self.fnmul_ins.take().unwrap();
                let xa = self.use_f(a, FS0);
                let xb = self.use_f(b, FS1);
                let xd = self.def_f(*d, FS0);
                // fnmul sd/dd, a, b
                self.a.e(0x1E20_8800 | if *f64_ { DBL } else { 0 } | (xb as u32) << 16 | (xa as u32) << 5 | xd as u32);
                self.fin_f(*d, xd);
            }
            Inst::FUnary(op, f64_, d, a) => {
                let xa = self.use_f(*a, FS0);
                let xd = self.def_f(*d, FS0);
                let opc = match op {
                    FUn::Neg => FNEG,
                    FUn::Abs => FABS,
                    FUn::Sqrt => FSQRT,
                    FUn::Floor => FRINTM,
                    FUn::Ceil => FRINTP,
                    FUn::Trunc => FRINTZ,
                    FUn::RoundEven => FRINTN,
                    FUn::RoundAway => FRINTA,
                };
                self.a.fp2(opc, *f64_, xd, xa);
                self.fin_f(*d, xd);
            }
            Inst::FCmp(c, f64_, d, a, b) => {
                let c = self.fcmp_v(*c, *f64_, *a, *b);
                let rd = self.def_i(*d, S0);
                self.a.cset(rd, fcc(c));
                self.fin_i(*d, rd);
            }
            Inst::Conv(c, d, s) => self.conv(*c, *d, *s),
            Inst::LoadX(m, d, base, idx, sh, off) => {
                let (op, size) = mem_op(*m, true);
                let ri = self.use_i(*idx, S0);
                let (rn, rm, o) = self.index_addr(*base, ri, *sh, *off, size);
                match m {
                    Mem::Int(..) => {
                        let rd = self.def_i(*d, S0);
                        self.ldst_x(op, size, rd, rn, rm, o);
                        self.fin_i(*d, rd);
                    }
                    _ => {
                        let xd = self.def_f(*d, FS0);
                        self.ldst_x(op, size, xd, rn, rm, o);
                        self.fin_f(*d, xd);
                    }
                }
            }
            Inst::StoreX(m, base, idx, sh, off, src) => {
                // the stored value first: reloading a spilled one from a far stack slot uses
                // ip1 as its offset register, which then holds the address (frames > 32 KB:
                // metal draw_pass wrote paint_dirty through a stack offset)
                let (op, size) = mem_op(*m, false);
                let rs = match m {
                    Mem::Int(..) => self.use_stored(*src, S0),
                    _ => self.use_f(*src, FS0),
                };
                let ri = self.use_i(*idx, IP0);
                let (rn, rm, o) = self.index_addr(*base, ri, *sh, *off, size);
                self.ldst_x(op, size, rs, rn, rm, o);
            }
            Inst::Load(m, d, base, off) => {
                let (base, off) = self.resolve_off(*base, *off as i64);
                let (rb, off) = match self.slot_base(base) {
                    Some(so) => (SP, &((so + off) as i32)),
                    None => (self.use_i(base, S1), &(off as i32)),
                };
                let (op, size) = mem_op(*m, true);
                match m {
                    Mem::Int(..) => {
                        let rd = self.def_i(*d, S0);
                        self.a.ldst(op, size, rd, rb, *off as i64);
                        self.fin_i(*d, rd);
                    }
                    _ => {
                        let xd = self.def_f(*d, FS0);
                        self.a.ldst(op, size, xd, rb, *off as i64);
                        self.fin_f(*d, xd);
                    }
                }
            }
            Inst::Store(m, base, off, src) => {
                let (base, off) = self.resolve_off(*base, *off as i64);
                let (rb, off) = match self.slot_base(base) {
                    Some(so) => (SP, &((so + off) as i32)),
                    None => (self.use_i(base, S1), &(off as i32)),
                };
                let (op, size) = mem_op(*m, false);
                match m {
                    Mem::Int(..) => {
                        let rs = self.use_stored(*src, S0);
                        self.a.ldst(op, size, rs, rb, *off as i64);
                    }
                    // a float 0.0 without a register: the integer store of zr (`fmov d30, xzr;
                    // str s30` was two instructions)
                    _ if self.al.loc[src.0 as usize] == Loc::None && matches!(self.al.remat[src.0 as usize], Remat::FConst(0, _)) => {
                        let op = if size == 8 { STRX } else { STRW };
                        self.a.ldst(op, size, ZR, rb, *off as i64);
                    }
                    _ => {
                        let xs = self.use_f(*src, FS0);
                        self.a.ldst(op, size, xs, rb, *off as i64);
                    }
                }
            }
            Inst::SlotAddr(d, s) => {
                let rd = self.def_i(*d, S0);
                let off = self.slot_off[*s as usize];
                self.a.add_any(rd, SP, off);
                self.fin_i(*d, rd);
            }
            Inst::Addr(d, sym) => {
                let rd = self.def_i(*d, S0);
                if SymClass::of(self.sym_class, sym.id) == SymClass::Domain {
                    self.dom_addr(rd, *sym);
                } else {
                    self.ldr_reloc(rd, Reloc::Sym(*sym));
                }
                self.fin_i(*d, rd);
            }
            Inst::FnAddr(d, id) => {
                let rd = self.def_i(*d, S0);
                self.ldr_reloc(rd, Reloc::Thunk(*id));
                self.fin_i(*d, rd);
            }
            Inst::Call(c, args, rets) => self.call(c, args, rets),
            // a slot copied onto its own storage (frame::slot_addr_positions): nothing moves
            Inst::Copy(dst, src, _) if self.slot_pos[dst.0 as usize] != i64::MIN && self.slot_pos[dst.0 as usize] == self.slot_pos[src.0 as usize] => {}
            // overlapping bytes of the frame (frame::copy_overlap; a verify build fails on it,
            // compile_freq): memmove order, each piece loaded whole before it is stored
            Inst::Copy(dst, src, size) if crate::frame::copy_overlap(&self.slot_pos, *dst, *src, *size) != 0 => {
                let delta = crate::frame::copy_overlap(&self.slot_pos, *dst, *src, *size);
                let rd = self.use_i(*dst, S0);
                let rs = self.use_i(*src, S1);
                for (o, w) in crate::frame::copy_pieces(*size, 0, delta > 0) {
                    let (ld, st) = match w {
                        8 => (LDRX, STRX),
                        4 => (LDRW, STRW),
                        2 => (LDRH, STRH),
                        _ => (LDRB, STRB),
                    };
                    self.a.ldst(ld, w as u32, IP0, rs, o as i64);
                    self.a.ldst(st, w as u32, IP0, rd, o as i64);
                }
            }
            Inst::Copy(dst, src, size) if *size > 256 => {
                // x8 = dst, x17 = src, x16 = 16-byte chunks: ldp/stp post-index loop
                let rd = self.use_i(*dst, S0);
                self.a.mov(X8, rd);
                let rs = self.use_i(*src, S1);
                self.a.mov(IP1, rs);
                let size = *size as i64;
                self.a.mov_imm(IP0, size / 16);
                let top = self.a.pos();
                self.a.ldp_post(S0, S1, IP1, 16);
                // stp x14, x15, [x8], #16
                self.a.e(0xA880_0000 | (2u32 << 15) | (S1 as u32) << 10 | (X8 as u32) << 5 | S0 as u32);
                // subs x16, x16, #1 ; b.ne top
                self.a.e(0xF100_0400 | (IP0 as u32) << 5 | IP0 as u32);
                let p = self.a.bcond(NE);
                self.a.patch(p, top);
                let mut o = 0i64;
                let rest = size % 16;
                while o + 8 <= rest {
                    self.a.ldst(LDRX, 8, S0, IP1, o);
                    self.a.ldst(STRX, 8, S0, X8, o);
                    o += 8;
                }
                while o < rest {
                    self.a.ldst(LDRB, 1, S0, IP1, o);
                    self.a.ldst(STRB, 1, S0, X8, o);
                    o += 1;
                }
            }
            Inst::Copy(dst, src, size) => {
                let size = *size as i64;
                let q = size >= 32 && self.qcopy;
                // a slot or `base + k` end is addressed off sp / the base with its offset (no `add`
                // first) when every piece's offset encodes: q pairs a multiple of 16 in -1024..=1008
                // (scaled imm7), else the 8/4/1-byte pieces' unsigned scaled or unscaled range
                let fits = |o: i64| if q { o % 16 == 0 && o >= -1024 && o + size - 16 <= 1008 } else { o >= 0 && o % 8 == 0 && o + size <= 4096 * 4 || o >= -256 && o + size <= 256 };
                let (rd, od) = {
                    let (b, o) = self.resolve_off(*dst, 0);
                    match self.slot_base(b) {
                        Some(so) if fits(so + o) => (SP, so + o),
                        _ if o != 0 && fits(o) && self.slot_base(b).is_none() => (self.use_i(b, S0), o),
                        _ => (self.use_i(*dst, S0), 0),
                    }
                };
                let (rs, os) = {
                    let (b, o) = self.resolve_off(*src, 0);
                    match self.slot_base(b) {
                        Some(so) if fits(so + o) => (SP, so + o),
                        _ if o != 0 && fits(o) && self.slot_base(b).is_none() => (self.use_i(b, S1), o),
                        _ => (self.use_i(*src, S1), 0),
                    }
                };
                let mut o = 0i64;
                if q {
                    // 16-byte vector moves through the float scratch (an `ldp`/`stp` pair moves 32
                    // bytes in two instructions): a 160-byte struct move is 10 instructions, not 40
                    let pair = |x: i64| x % 16 == 0 && (-1024..=1008).contains(&x);
                    while o + 32 <= size && pair(os + o) && pair(od + o) {
                        self.a.e(0xAD40_0000 | ((((os + o) / 16) as u32) & 0x7f) << 15 | (FS1 as u32) << 10 | (rs as u32) << 5 | FS0 as u32);
                        self.a.e(0xAD00_0000 | ((((od + o) / 16) as u32) & 0x7f) << 15 | (FS1 as u32) << 10 | (rd as u32) << 5 | FS0 as u32);
                        o += 32;
                    }
                    while o + 16 <= size {
                        self.vldst(true, FS0, rs, os + o);
                        self.vldst(false, FS0, rd, od + o);
                        o += 16;
                    }
                    // the tail falls through to the 8/4/1-byte pieces below, not one overlapping
                    // 16-byte move: a value moved through several temporaries is copied with the
                    // same pieces each time, so every load reads exactly one earlier store (an
                    // overlapping tail made the next copy's load span two stores)
                }
                while o + 8 <= size {
                    self.a.ldst(LDRX, 8, IP0, rs, os + o);
                    self.a.ldst(STRX, 8, IP0, rd, od + o);
                    o += 8;
                }
                while o + 4 <= size {
                    self.a.ldst(LDRW, 4, IP0, rs, os + o);
                    self.a.ldst(STRW, 4, IP0, rd, od + o);
                    o += 4;
                }
                while o < size {
                    self.a.ldst(LDRB, 1, IP0, rs, os + o);
                    self.a.ldst(STRB, 1, IP0, rd, od + o);
                    o += 1;
                }
            }
            Inst::Coverage { word, mask, disabled } => {
                // Reserved scratch registers: coverage adds no program live
                // ranges or spills. The NOP is patched only after this bit
                // has been observed set; an in-flight old probe is harmless.
                assert!(mask.is_power_of_two());
                let at = (self.a.pos() * 4) as u32;
                self.a.e(0xD503_201F);
                self.tls_block_ip0();
                self.ldr_reloc(IP1, Reloc::Sym(*disabled));
                self.a.e(0xF860_6800 | (IP1 as u32) << 16 | (IP0 as u32) << 5 | IP0 as u32);
                let skip = self.a.cbnz(IP0);
                self.a.mov_imm64_fixed(IP0, *word);
                self.a.mov_imm(IP1, *mask as i64);
                // stset x17, [x16] (the unused old value goes to xzr).
                self.a.e(0xF820_3000 | (IP1 as u32) << 16 | (IP0 as u32) << 5 | ZR as u32);
                let end = self.a.pos();
                self.a.patch(skip, end);
                self.relocs.push(RelocAt { at, kind: RelocKind::A64CoverageBit { bit: mask.trailing_zeros() as u8, words: end as u32 - at / 4 }, target: Reloc::Value(*word) });
            }
            Inst::AtomicLoad(..) | Inst::AtomicStore(..) | Inst::AtomicRmw(..) | Inst::AtomicCas(..) | Inst::Fence(..) => self.atomic(i),
            Inst::TlsAddr(d, off) => self.tls_addr(*d, *off),
            // static TLS is placed for x86-64 Linux only (compiler os/tls.rs)
            Inst::TlsStatic(..) => unreachable!("arm64: no static TLS placement"),
            Inst::Cpuid(..) | Inst::Xgetbv(..) => unreachable!("x86 CPU query rejected before arm64 emission"),
            Inst::FpEnv(true, x) => {
                // msr fpcr, xt
                let rx = self.use_i(*x, IP0);
                self.a.e(0xD51B_4400 | rx as u32);
            }
            Inst::FpEnv(false, d) => {
                // mrs xt, fpcr
                let rd = self.def_i(*d, IP0);
                self.a.e(0xD53B_4400 | rd as u32);
                self.fin_i(*d, rd);
            }
            Inst::VShuffle(..) | Inst::VSel(..) | Inst::VFma(..) | Inst::VSplat(..) | Inst::VExtract(..) | Inst::VInsert(..) | Inst::VLoad(..) | Inst::VStore(..) | Inst::VLoadX(..) | Inst::VStoreX(..) | Inst::VAny(..) | Inst::VGather(..) | Inst::VTbl(..) | Inst::VConst(..) => self.vinst(i),
            Inst::VX(op, d, a, b) => self.vx(*op, *d, *a, *b),
            Inst::VX3(op, d, a, b, c) => self.vx3(*op, *d, *a, *b, *c),
            Inst::Sel(d, c, a, b) => {
                let cond = match self.sel_chain {
                    Some((c0, cc)) if c0 == *c => cc,
                    _ => {
                        let rc = self.use_i(*c, S0);
                        self.cmp_imm(rc, 0);
                        // the flags now hold this test: a chained compare's are gone (a later
                        // select of that compare reads its materialised bool)
                        self.sel_chain = None;
                        NE
                    }
                };
                if self.cls(*d) == Cls::I {
                    let ra = self.use_i(*a, S0);
                    let rb = self.use_i(*b, S1);
                    let rd = self.def_i(*d, S0);
                    self.a.csel(rd, ra, rb, cond);
                    self.fin_i(*d, rd);
                } else {
                    let w = self.cls(*d) == Cls::F64;
                    let xa = self.use_f(*a, FS0);
                    let xb = self.use_f(*b, FS1);
                    let xd = self.def_f(*d, FS0);
                    self.a.e(0x1E20_0C00 | if w { DBL } else { 0 } | (xb as u32) << 16 | (cond as u32) << 12 | (xa as u32) << 5 | xd as u32);
                    self.fin_f(*d, xd);
                }
            }
            Inst::FMulAdd(np, na, w, d, a, b, c) => {
                let xa = self.use_f(*a, FS0);
                let xb = self.use_f(*b, FS1);
                let xc = self.use_f(*c, FS2);
                let xd = self.def_f(*d, FS0);
                // fmadd c + ab, fmsub c - ab, fnmsub ab - c, fnmadd -ab - c
                let base = match (np, na) {
                    (false, false) => 0x1F00_0000,
                    (true, false) => 0x1F00_8000,
                    (false, true) => 0x1F20_8000,
                    (true, true) => 0x1F20_0000,
                };
                self.a.e(base | if *w { DBL } else { 0 } | (xb as u32) << 16 | (xc as u32) << 10 | (xa as u32) << 5 | xd as u32);
                self.fin_f(*d, xd);
            }
        }
    }

    /// A rotate's source: an 8/16-bit value replicated across 32 bits in IP1 (a 32-bit rotate of
    /// it rotates every copy, so the low bits are the narrow rotate for any count), else `ra`.
    fn rot_src(&mut self, bits: u8, ra: u8) -> u8 {
        if bits >= 32 {
            return ra;
        }
        self.a.extend(IP1, ra, bits, false);
        let mut w = bits as u32;
        while w < 32 {
            // orr w17, w17, w17, lsl #w
            self.a.e(0x2A00_0000 | (IP1 as u32) << 16 | w << 10 | (IP1 as u32) << 5 | IP1 as u32);
            w *= 2;
        }
        IP1
    }
    /// A rotate result in the type's canonical form: W forms zero-extend (u32 done).
    fn rot_ext(&mut self, it: IntTy, rd: u8, ext: bool) {
        if it.bits < 64 && it != U32 && ext {
            self.a.extend(rd, rd, it.bits, it.signed);
        }
    }
    /// `ext`: re-extend a narrow result (false: nobody reads its upper bits, passes::low_bits_only)
    fn ibin_rr(&mut self, op: IOp, it: IntTy, rd: u8, ra: u8, rb: u8, ext: bool) {
        if op == IOp::Bzhi {
            // ip0 = -1 << (b mod 64); rd = a & !ip0 (movn, lslv, bic)
            self.a.e(0x9280_0000 | IP0 as u32);
            self.a.rrr(0x9AC0_2000, IP0, IP0, rb);
            self.a.rrr(0x8A20_0000, rd, ra, IP0);
            return;
        }
        if op == IOp::Rotl {
            // rotate left by b = rorv by -b (rorv takes the count mod the width; W form below 64
            // bits, an 8/16-bit value replicated across the W register first: passes::rotl)
            self.a.e(0xCB00_03E0 | (rb as u32) << 16 | IP0 as u32);
            let src = self.rot_src(it.bits, ra);
            self.a.rrr(if it.bits < 64 { 0x1AC0_2C00 } else { 0x9AC0_2C00 }, rd, src, IP0);
            self.rot_ext(it, rd, ext);
            return;
        }
        if it == U32 && !matches!(op, IOp::Div | IOp::Rem | IOp::MulHi) {
            // W forms zero-extend for free (shift amounts are taken mod 32)
            let w = match op {
                IOp::Add => 0x0B00_0000,
                IOp::Sub => 0x4B00_0000,
                IOp::Mul => 0x1B00_7C00,
                IOp::And => 0x0A00_0000,
                IOp::Or => 0x2A00_0000,
                IOp::Xor => 0x4A00_0000,
                IOp::Shl => 0x1AC0_2000,
                _ => 0x1AC0_2400, // lsrv
            };
            self.a.rrr(w, rd, ra, rb);
            return;
        }
        match op {
            IOp::Add => self.a.rrr(0x8B00_0000, rd, ra, rb),
            IOp::Sub => self.a.rrr(0xCB00_0000, rd, ra, rb),
            IOp::Mul => self.a.rrr(0x9B00_7C00, rd, ra, rb),
            IOp::And => self.a.rrr(0x8A00_0000, rd, ra, rb),
            IOp::Or => self.a.rrr(0xAA00_0000, rd, ra, rb),
            IOp::Xor => self.a.rrr(0xCA00_0000, rd, ra, rb),
            IOp::Shl => self.a.rrr(0x9AC0_2000, rd, ra, rb),
            IOp::Shr => self.a.rrr(if it.signed { 0x9AC0_2800 } else { 0x9AC0_2400 }, rd, ra, rb),
            IOp::Div => self.a.rrr(if it.signed { 0x9AC0_0C00 } else { 0x9AC0_0800 }, rd, ra, rb),
            // smulh / umulh
            IOp::MulHi => self.a.rrr(if it.signed { 0x9B40_7C00 } else { 0x9BC0_7C00 }, rd, ra, rb),
            // handled at the top
            IOp::Rotl | IOp::Bzhi => {}
            IOp::Rem => {
                // q = a / b; d = a - q * b  (rd may alias ra or rb: compute q in IP0)
                self.a.rrr(if it.signed { 0x9AC0_0C00 } else { 0x9AC0_0800 }, IP0, ra, rb);
                // msub rd, ip0, rb, ra
                self.a.e(0x9B00_8000 | (rb as u32) << 16 | (ra as u32) << 10 | (IP0 as u32) << 5 | rd as u32);
            }
        }
        if it.bits < 64 && !matches!(op, IOp::And | IOp::Or | IOp::Xor) && ext {
            self.a.extend(rd, rd, it.bits, it.signed);
        }
    }

    fn ibin_imm(&mut self, op: IOp, it: IntTy, d: VReg, a: VReg, imm: i64) {
        if op == IOp::Bzhi {
            // a constant count is a constant mask
            let n = (imm & 63) as u32;
            return self.ibin_imm(IOp::And, it, d, a, if n == 0 { 0 } else { (u64::MAX >> (64 - n)) as i64 });
        }
        let ra = self.use_i(a, S0);
        let rd = self.def_i(d, S0);
        if op == IOp::Rotl {
            // rotate left by k = ror by (w - k) = extr rd, ra, ra, #(w - k) (W form below 64 bits,
            // an 8/16-bit value replicated across the W register first: passes::rotl)
            let w = if it.bits < 64 { 32u32 } else { 64 };
            let r = (w - ((imm as u64 % it.bits as u64) as u32 % w)) % w;
            let base = if w == 32 { 0x1380_0000u32 } else { 0x93C0_0000 };
            let src = self.rot_src(it.bits, ra);
            self.a.e(base | (src as u32) << 16 | r << 10 | (src as u32) << 5 | rd as u32);
            let ext = !self.low_only[d.0 as usize];
            self.rot_ext(it, rd, ext);
            self.fin_i(d, rd);
            return;
        }
        if it == U32 && matches!(op, IOp::Shl | IOp::Shr) {
            // lsl / lsr w by an immediate (ubfm w forms) zero-extend
            let s = (imm & 31) as u32;
            if op == IOp::Shl {
                self.a.e(0x5300_0000 | ((32 - s) & 31) << 16 | (31 - s) << 10 | (ra as u32) << 5 | rd as u32);
            } else {
                self.a.e(0x5300_0000 | s << 16 | 31 << 10 | (ra as u32) << 5 | rd as u32);
            }
            self.fin_i(d, rd);
            return;
        }
        if it == U32 && matches!(op, IOp::Add | IOp::Sub) && (0..4096).contains(&imm) {
            self.a.e(if op == IOp::Add { 0x1100_0000 } else { 0x5100_0000 } | (imm as u32) << 10 | (ra as u32) << 5 | rd as u32);
            self.fin_i(d, rd);
            return;
        }
        let mut done = true;
        match op {
            IOp::Add | IOp::Sub => {
                let v = if op == IOp::Sub { imm.wrapping_neg() } else { imm };
                if v.unsigned_abs() < (1 << 24) {
                    self.a.add_any(rd, ra, v);
                } else {
                    done = false;
                }
            }
            IOp::Mul if imm > 0 && (imm & (imm - 1)) == 0 => {
                let s = imm.trailing_zeros();
                self.a.ubfm(rd, ra, (64 - s) & 63, 63 - s);
            }
            IOp::Mul if mul_shift_add(imm).is_some() => match mul_shift_add(imm).unwrap() {
                // add rd, ra, ra, lsl #k
                (k, true) => self.a.e(0x8B00_0000 | (ra as u32) << 16 | k << 10 | (ra as u32) << 5 | rd as u32),
                // lsl ip0, ra, #k; sub rd, ip0, ra
                (k, false) => {
                    self.a.ubfm(IP0, ra, (64 - k) & 63, 63 - k);
                    self.a.rrr(0xCB00_0000, rd, IP0, ra);
                }
            },
            IOp::Shl => {
                let s = (imm & 63) as u32;
                self.a.ubfm(rd, ra, (64 - s) & 63, 63 - s);
            }
            IOp::Shr => {
                let s = (imm & 63) as u32;
                if it.signed {
                    self.a.sbfm(rd, ra, s, 63);
                } else {
                    self.a.ubfm(rd, ra, s, 63);
                }
            }
            IOp::And if imm == 0xff || imm == 0xffff || imm == 0xffff_ffff => {
                let bits = match imm {
                    0xff => 8,
                    0xffff => 16,
                    _ => 32,
                };
                self.a.extend(rd, ra, bits, false);
            }
            // and / orr / eor with a bitmask immediate (64-bit values, or u32 ones without high bits)
            IOp::And | IOp::Or | IOp::Xor if (it.bits == 64 || (it == U32 && (0..1i64 << 32).contains(&imm))) && bitmask_imm(imm as u64).is_some() => {
                let (n, immr, imms) = bitmask_imm(imm as u64).unwrap();
                let base = match op {
                    IOp::And => 0x9200_0000u32,
                    IOp::Or => 0xB200_0000,
                    _ => 0xD200_0000,
                };
                self.a.e(base | n << 22 | immr << 16 | imms << 10 | (ra as u32) << 5 | rd as u32);
            }
            _ => done = false,
        }
        if !done {
            self.a.mov_imm(S1, imm);
            let ext = !self.low_only[d.0 as usize];
            self.ibin_rr(op, it, rd, ra, S1, ext);
            self.fin_i(d, rd);
            return;
        }
        if it.bits < 64 && !matches!(op, IOp::And | IOp::Or | IOp::Xor) && !self.low_only[d.0 as usize] {
            self.a.extend(rd, rd, it.bits, it.signed);
        }
        self.fin_i(d, rd);
    }

    fn conv(&mut self, c: Conv, d: VReg, s: VReg) {
        match c {
            Conv::IntToInt(it) => {
                let rs = self.use_i(s, S0);
                let rd = self.def_i(d, S0);
                // zero above fewer bits than the target: the extension is the value itself
                let z = self.zero_above[s.0 as usize];
                if self.low_only[d.0 as usize] || crate::passes::cast_is_move(z, it) {
                    self.a.mov(rd, rs);
                } else {
                    self.a.extend(rd, rs, it.bits, it.signed);
                }
                self.fin_i(d, rd);
            }
            Conv::IntToF(from, to64) => {
                let rs = self.use_i(s, S0);
                let xd = self.def_f(d, FS0);
                // registers hold values extended to 64 bits, so a 64-bit convert is exact;
                // u64 needs the unsigned form
                let op = match (from.bits == 64 && !from.signed, to64) {
                    (true, true) => 0x9E63_0000,
                    (true, false) => 0x9E23_0000,
                    (false, true) => 0x9E62_0000,
                    (false, false) => 0x9E22_0000,
                };
                self.a.e(op | (rs as u32) << 5 | xd as u32);
                self.fin_f(d, xd);
            }
            Conv::FToInt(from64, it) => {
                // fcvtz[su] saturates and maps NaN to 0: Rust `as` semantics for 32/64 bits
                let xs = self.use_f(s, FS0);
                let rd = self.def_i(d, S0);
                let src = if from64 { DBL } else { 0 };
                let uns = if it.signed { 0 } else { 0x0001_0000 };
                if it.bits == 64 {
                    self.a.e(0x9E38_0000 | src | uns | (xs as u32) << 5 | rd as u32);
                } else {
                    // 32-bit saturating convert (W form zero-extends into X)
                    self.a.e(0x1E38_0000 | src | uns | (xs as u32) << 5 | rd as u32);
                    if it.signed {
                        self.a.extend(rd, rd, 32, true);
                    }
                    if it.bits < 32 {
                        let (lo, hi): (i64, i64) = if it.signed {
                            (-(1i64 << (it.bits - 1)), (1i64 << (it.bits - 1)) - 1)
                        } else {
                            (0, (1i64 << it.bits) - 1)
                        };
                        self.a.mov_imm(IP0, hi);
                        self.a.cmp(rd, IP0);
                        self.a.csel(rd, IP0, rd, if it.signed { GT } else { HI });
                        if it.signed {
                            self.a.mov_imm(IP0, lo);
                            self.a.cmp(rd, IP0);
                            self.a.csel(rd, IP0, rd, LT);
                        }
                    }
                }
                self.fin_i(d, rd);
            }
            Conv::F32ToF64 => {
                let xs = self.use_f(s, FS0);
                let xd = self.def_f(d, FS0);
                self.a.e(0x1E22_C000 | (xs as u32) << 5 | xd as u32);
                self.fin_f(d, xd);
            }
            Conv::F64ToF32 => {
                let xs = self.use_f(s, FS0);
                let xd = self.def_f(d, FS0);
                self.a.e(0x1E62_4000 | (xs as u32) << 5 | xd as u32);
                self.fin_f(d, xd);
            }
            Conv::BitsToF(f64_) => {
                let rs = self.use_i(s, S0);
                let xd = self.def_f(d, FS0);
                self.a.fmov_from_gp(f64_, xd, rs);
                self.fin_f(d, xd);
            }
            Conv::FToBits(f64_) => {
                let xs = self.use_f(s, FS0);
                let rd = self.def_i(d, S0);
                // fmov wd, sn zero-extends: f32 bits as u32
                self.a.fmov_to_gp(f64_, rd, xs);
                self.fin_i(d, rd);
            }
        }
    }

    fn call(&mut self, c: &Callee, args: &[VReg], rets: &[VReg]) {
        if let Callee::CHost(_, sig) | Callee::CIndirect(_, sig) | Callee::CFn(_, sig) = c {
            return self.c_call(c, sig, args, rets);
        }
        if let Callee::Asm(b) = c {
            return self.asm_block(b, args, rets);
        }
        // save caller-saved values that live across this call
        let saves = self.al.saves.get(&self.cur_pos).cloned().unwrap_or_default();
        for (v, slot) in &saves {
            // Alloc::save_skip: the slot already holds it / nobody reads the register
            if !self.al.save_skip.contains(&(self.cur_pos, v.0)) {
                self.save_restore(*v, *slot, true);
            }
        }
        // indirect target first: argument moves may overwrite its register
        if let Callee::Indirect(v) = c {
            let r = self.use_i(*v, IP0);
            self.a.mov(IP0, r);
        }
        let mut mv = Vec::new();
        let mut ni = 0usize;
        let mut nf = 0usize;
        let mut k = 0i64;
        for a in args {
            let is_int = self.cls(*a) == Cls::I;
            if is_int && ni < INT_ARGS {
                mv.push((MLoc::R(ni as u8), self.mloc(*a)));
                ni += 1;
            } else if !is_int && nf < FLT_ARGS {
                mv.push((MLoc::F(nf as u8), self.mloc(*a)));
                nf += 1;
            } else {
                // overflow argument: outgoing area [sp + 8k], stored before the register
                // moves overwrite argument registers
                match self.mloc(*a) {
                    MLoc::R(r) => self.a.ldst(STRX, 8, r, SP, 8 * k),
                    MLoc::F(x) => self.a.ldst(STRD, 8, x, SP, 8 * k),
                    MLoc::M(o) => {
                        self.a.ldst(LDRX, 8, S1, SP, o as i64);
                        self.a.ldst(STRX, 8, S1, SP, 8 * k);
                    }
                    MLoc::K(v) if self.cls(VReg(v)) != Cls::I => {
                        self.fconst_to(FS1, VReg(v));
                        self.a.ldst(STRD, 8, FS1, SP, 8 * k);
                    }
                    MLoc::K(v) => {
                        self.remat_into(S1, VReg(v));
                        self.a.ldst(STRX, 8, S1, SP, 8 * k);
                    }
                }
                k += 1;
            }
        }
        self.moves(&mv);
        match c {
            Callee::Fn(id) => self.bl_reloc(Reloc::Call(*id)),
            Callee::Indirect(_) => self.a.blr(IP0),
            Callee::Host(sym) => self.call_far(Reloc::Sym(*sym)),
            Callee::CHost(..) | Callee::CIndirect(..) | Callee::CFn(..) | Callee::Asm(..) => {}
        }
        self.last_call_ret = self.a.pos();
        self.var_calls.push((self.cur_pos, (self.a.pos() * 4) as u32));
        let mut mv = Vec::new();
        let mut ri = 0u8;
        let mut rf = 0u8;
        for r in rets {
            if self.cls(*r) == Cls::I {
                if self.al.loc[r.0 as usize] != Loc::None {
                    mv.push((self.mloc(*r), MLoc::R(ri)));
                }
                ri += 1;
            } else {
                if self.al.loc[r.0 as usize] != Loc::None {
                    mv.push((self.mloc(*r), MLoc::F(rf)));
                }
                rf += 1;
            }
        }
        self.moves(&mv);
        for (v, slot) in &saves {
            // Alloc::restore_skip: the slot already holds it / nobody reads the register
            if !self.al.restore_skip.contains(&(self.cur_pos, v.0)) {
                self.save_restore(*v, *slot, false);
            }
        }
    }
}

/// FCmp of negations (or of a negation and +0.0): the compare of the negated values, mirrored.
/// `FMulAdd(.., a, b, c)` whose a or c is a negation `-x` used only there: the negation folds into
/// the multiply-add's flags (product / addend negated), x read instead. Exact: the fused forms negate
/// the operand (FPNeg) before the one rounding, as the separate fneg did (NaN signs included).
fn fold_fma_negs(f: &Func) -> Option<Func> {
    let n = f.vregs.len();
    let mut ndef = vec![0u32; n];
    let mut uses = vec![0u32; n];
    let mut neg: Vec<Option<(VReg, bool)>> = vec![None; n];
    let mut fzero = vec![false; n];
    let mut any = false;
    for p in &f.params {
        ndef[p.0 as usize] += 1;
    }
    for b in &f.blocks {
        for i in &b.insts {
            i.defs(|d| ndef[d.0 as usize] += 1);
            regalloc::for_uses(i, |u| uses[u.0 as usize] += 1);
            if let Inst::FUnary(FUn::Neg, w, d, x) = *i {
                neg[d.0 as usize] = Some((x, w));
            }
            any |= matches!(i, Inst::FMulAdd(..) | Inst::FCmp(..));
            if let Inst::Fconst(d, 0, _) = *i {
                fzero[d.0 as usize] = true;
            }
        }
        let mut t = Vec::new();
        regalloc::term_uses(&b.term, &mut t);
        for u in t {
            uses[u.0 as usize] += 1;
        }
    }
    if !any {
        return None;
    }
    let of = |v: VReg, w: bool| match neg[v.0 as usize] {
        Some((x, w2)) if w2 == w && ndef[v.0 as usize] == 1 && uses[v.0 as usize] == 1 && ndef[x.0 as usize] == 1 => Some(x),
        _ => None,
    };
    let mut g = f.clone();
    let mut dead = vec![false; n];
    let zero = |v: VReg| fzero[v.0 as usize] && ndef[v.0 as usize] == 1;
    for b in &mut g.blocks {
        for i in &mut b.insts {
            // -x c -y == x c' y and -x c 0 == x c' 0 (c' mirrored: -x < y == x > -y; unordered
            // stays unordered; -0 == +0)
            if let Inst::FCmp(c, w, _, a, bb) = i {
                let (na, nb) = (of(*a, *w), of(*bb, *w));
                let fold = match (na, nb) {
                    (Some(x), Some(y)) => Some((x, y)),
                    (Some(x), None) if zero(*bb) => Some((x, *bb)),
                    (None, Some(y)) if zero(*a) => Some((*a, y)),
                    _ => None,
                };
                if let Some((x, y)) = fold {
                    if na.is_some() {
                        dead[a.0 as usize] = true;
                    }
                    if nb.is_some() {
                        dead[bb.0 as usize] = true;
                    }
                    *a = x;
                    *bb = y;
                    *c = match *c {
                        Cond::Lt => Cond::Gt,
                        Cond::Le => Cond::Ge,
                        Cond::Gt => Cond::Lt,
                        Cond::Ge => Cond::Le,
                        c => c,
                    };
                }
            }
            if let Inst::FMulAdd(np, na, w, _, a, _, c) = i {
                if let Some(x) = of(*c, *w) {
                    dead[c.0 as usize] = true;
                    *c = x;
                    *na = !*na;
                }
                // a only: FMSUB / FNMADD negate the first factor (Rn), so a NaN b keeps its sign
                // only when the negation was on a
                if let Some(x) = of(*a, *w) {
                    dead[a.0 as usize] = true;
                    *a = x;
                    *np = !*np;
                }
            }
        }
    }
    if !dead.iter().any(|d| *d) {
        return None;
    }
    for b in &mut g.blocks {
        let keep: Vec<bool> = b.insts.iter().map(|i| !matches!(i, Inst::FUnary(FUn::Neg, _, d, _) if dead[d.0 as usize])).collect();
        let mut k = 0;
        b.insts.retain(|_| {
            k += 1;
            keep[k - 1]
        });
        let mut k = 0;
        b.pos.retain(|_| {
            k += 1;
            keep[k - 1]
        });
    }
    Some(g)
}

/// Every FCmp operand that is a single-def +0.0 (Fconst 0) defined elsewhere reads a fresh copy of
/// the constant defined right before the compare instead; the emitter builds none of them
/// (`fcmp s, #0.0`, Gen::zero_cmp).
fn split_zero_compares(f: &Func) -> Option<Func> {
    let n = f.vregs.len();
    let mut ndef = vec![0u32; n];
    let mut fz: Vec<Option<bool>> = vec![None; n];
    for p in &f.params {
        ndef[p.0 as usize] += 1;
    }
    let mut any = false;
    for b in &f.blocks {
        for i in &b.insts {
            i.defs(|d| ndef[d.0 as usize] += 1);
            if let Inst::Fconst(d, 0, w) = *i {
                fz[d.0 as usize] = Some(w);
            }
            any |= matches!(i, Inst::FCmp(..));
        }
    }
    let zero = |v: VReg| if ndef[v.0 as usize] == 1 { fz[v.0 as usize] } else { None };
    if !any {
        return None;
    }
    let mut g = f.clone();
    let mut changed = false;
    for bi in 0..g.blocks.len() {
        let mut k = 0;
        while k < g.blocks[bi].insts.len() {
            if let Inst::FCmp(c, w, d, a, b) = g.blocks[bi].insts[k] {
                let prev_def = |v: VReg, blk: &Block| k > 0 && matches!(blk.insts[k - 1], Inst::Fconst(t, 0, _) if t == v);
                let mut ins = Vec::new();
                let (mut a2, mut b2) = (a, b);
                for (op, slot) in [(a, 0), (b, 1)] {
                    if zero(op) == Some(w) && !prev_def(op, &g.blocks[bi]) && a != b {
                        let t = g.vreg(if w { Cls::F64 } else { Cls::F32 });
                        ins.push(Inst::Fconst(t, 0, w));
                        if slot == 0 {
                            a2 = t;
                        } else {
                            b2 = t;
                        }
                    }
                }
                if !ins.is_empty() {
                    let m = ins.len();
                    let pos = g.blocks[bi].pos[k];
                    g.blocks[bi].insts[k] = Inst::FCmp(c, w, d, a2, b2);
                    g.blocks[bi].insts.splice(k..k, ins);
                    g.blocks[bi].pos.splice(k..k, std::iter::repeat(pos).take(m));
                    k += m;
                    changed = true;
                }
            }
            k += 1;
        }
    }
    changed.then_some(g)
}

/// Per vreg: defined exactly once, as an all-zero vector (VConst 0, or FromLo64 of a vreg defined
/// once as the constant 0)
/// (and, per vreg, defined once by an f32 load)
fn zero_compare_consts(f: &Func) -> Vec<bool> {
    let n = f.vregs.len();
    let mut ndef = vec![0u32; n];
    let mut z = vec![false; n];
    let mut other = vec![false; n];
    for p in &f.params {
        ndef[p.0 as usize] += 1;
    }
    for b in &f.blocks {
        for i in &b.insts {
            i.defs(|d| ndef[d.0 as usize] += 1);
            if let Inst::Fconst(d, 0, _) = *i {
                z[d.0 as usize] = true;
            }
            if !matches!(i, Inst::FCmp(..)) {
                regalloc::for_uses(i, |u| other[u.0 as usize] = true);
            }
        }
        let mut t = Vec::new();
        regalloc::term_uses(&b.term, &mut t);
        for u in t {
            other[u.0 as usize] = true;
        }
    }
    (0..n).map(|v| z[v] && ndef[v] == 1 && !other[v]).collect()
}

fn zero_vectors(f: &Func) -> (Vec<bool>, Vec<bool>) {
    let n = f.vregs.len();
    let mut defs = vec![0u32; n];
    let mut zint = vec![false; n];
    let mut zvec = vec![false; n];
    let mut fload = vec![false; n];
    for b in &f.blocks {
        for i in &b.insts {
            i.defs(|d| defs[d.0 as usize] += 1);
            match *i {
                Inst::Iconst(d, 0) => zint[d.0 as usize] = true,
                Inst::VConst(d, 0) => zvec[d.0 as usize] = true,
                Inst::Load(Mem::F32, d, ..) | Inst::LoadX(Mem::F32, d, ..) => fload[d.0 as usize] = true,
                _ => {}
            }
        }
    }
    for b in &f.blocks {
        for i in &b.insts {
            if let Inst::VX(VX::FromLo64, d, k, _) = *i {
                if zint[k.0 as usize] && defs[k.0 as usize] == 1 {
                    zvec[d.0 as usize] = true;
                }
            }
        }
    }
    for p in &f.params {
        defs[p.0 as usize] += 1;
    }
    ((0..n).map(|v| zvec[v] && defs[v] == 1).collect(), (0..n).map(|v| fload[v] && defs[v] == 1).collect())
}

/// Single-definition values that are zero in every bit (zero_vectors' vectors, Iconst 0, Fconst +0.0)
fn zero_values(f: &Func, zero_vec: &[bool]) -> Vec<bool> {
    let n = f.vregs.len();
    let mut defs = vec![0u32; n];
    let mut z = zero_vec.to_vec();
    for p in &f.params {
        defs[p.0 as usize] += 1;
    }
    for b in &f.blocks {
        for i in &b.insts {
            i.defs(|d| defs[d.0 as usize] += 1);
            if let Inst::Iconst(d, 0) | Inst::Fconst(d, 0, _) = *i {
                z[d.0 as usize] = true;
            }
        }
    }
    (0..n).map(|v| z[v] && defs[v] == 1).collect()
}

fn words_to_bytes(w: &[u32]) -> Vec<u8> {
    let mut b = Vec::with_capacity(w.len() * 4);
    for x in w {
        b.extend_from_slice(&x.to_le_bytes());
    }
    b
}

fn count_classes(f: &Func, vs: &[VReg]) -> (usize, usize) {
    let mut ni = 0;
    let mut nf = 0;
    for v in vs {
        if f.vregs[v.0 as usize] == Cls::I {
            ni += 1;
        } else {
            nf += 1;
        }
    }
    (ni, nf)
}

pub fn compile(options: &relax_session::Options, f: &Func, env: &Env) -> Result<Compiled, String> {
    compile_freq(options, f, env, None)
}

/// A multiplicand that is a splat of an f32 vreg (`VSplat` of an F32 value, single def) is read as
/// that scalar: `VFma(.., a, s, c)` / `VX(FMul(S, _), d, a, s)` with `s` of class F32 (backend-only
/// form, made here just before register allocation) mean every lane times `s`, which the emitter
/// encodes by element (`fmla vd.4s, vn.4s, vm.s[0]`). A splat whose every use is folded goes. Only
/// `VFma`'s second factor folds: fmls negates Vn, and the NaN of a lane is c's, Vn's, then Vm's
/// (rir::fma_nan_f32), so a splat first factor swapped into Vm would flip the wrong operand's NaN
/// sign. `FMul`'s operand order picks the NaN for the Neon flavour, so only its second operand is
/// folded unless the NaN is the host's.
/// Right shifts by per-lane amounts (`ShrUV` / `ShrSV`, 32-bit lanes) are `neg` + `ushl` / `sshl`
/// on NEON. Where the amount is `y + c` (`c` a constant splat, the sum read only by the shift: a
/// hash's `x >> ((x >> 28) + 4)`), the amount becomes `-c - y` -- the add a `sub` from the
/// constant `-c`, built next to `c` -- and the shift `ArmUShl` / `ArmSShl` by it; `y - c` becomes
/// `c - y` (no new constant). One instruction fewer per shift.
/// A single-definition `VConst(d, k)` after another single-definition `VConst(c, k)` in the same
/// block: d's uses read c (dominated by c's definition, as d's are), d's definition goes. One
/// movi per block instead of one per use site: the unrolled `[T; N]` inits of box3d stored a
/// fresh zero vector per element. A constant of a long range is still rebuilt at its uses when
/// the allocator spills it (Remat::VConst).
fn share_vconsts(f: &Func) -> Option<Func> {
    let n = f.vregs.len();
    let mut ndef = vec![0u32; n];
    for p in &f.params {
        ndef[p.0 as usize] += 1;
    }
    let mut any = 0usize;
    for b in &f.blocks {
        for i in &b.insts {
            i.defs(|d| ndef[d.0 as usize] += 1);
            any += matches!(i, Inst::VConst(..)) as usize;
        }
    }
    if any < 2 {
        return None;
    }
    let mut to: Vec<u32> = (0..n as u32).collect();
    let mut drop: Vec<(usize, usize)> = Vec::new();
    for (bi, b) in f.blocks.iter().enumerate() {
        let mut seen: Vec<(u32, VReg)> = Vec::new();
        for (ii, i) in b.insts.iter().enumerate() {
            if let Inst::VConst(d, k) = *i {
                if ndef[d.0 as usize] != 1 {
                    continue;
                }
                match seen.iter().find(|e| e.0 == k) {
                    Some(&(_, c)) => {
                        to[d.0 as usize] = c.0;
                        drop.push((bi, ii));
                    }
                    None => seen.push((k, d)),
                }
            }
        }
    }
    if drop.is_empty() {
        return None;
    }
    let mut g = f.clone();
    for &(bi, ii) in drop.iter().rev() {
        g.blocks[bi].insts.remove(ii);
        g.blocks[bi].pos.remove(ii);
    }
    let m = |v: VReg| if (v.0 as usize) < n { VReg(to[v.0 as usize]) } else { v };
    for b in g.blocks.iter_mut() {
        for i in b.insts.iter_mut() {
            *i = crate::passes::rename(i, &m);
        }
        b.term = match &b.term {
            Term::Branch(c, x, y) => Term::Branch(m(*c), *x, *y),
            Term::Ret(v) => Term::Ret(v.iter().map(|x| m(*x)).collect()),
            Term::Switch(c, t) => Term::Switch(m(*c), t.clone()),
            other => other.clone(),
        };
    }
    g.dbg_map_vregs(&|v| Some(m(v)));
    Some(g)
}

fn fold_shift_amounts(f: &Func) -> Option<Func> {
    let n = f.vregs.len();
    let mut ndef = vec![0u32; n];
    let mut nuse = vec![0u32; n];
    for p in &f.params {
        ndef[p.0 as usize] += 1;
    }
    let mut any = false;
    for b in &f.blocks {
        for i in &b.insts {
            i.defs(|d| ndef[d.0 as usize] += 1);
            regalloc::for_uses(i, |u| nuse[u.0 as usize] += 1);
            any |= matches!(i, Inst::VX(VX::ShrUV(VW::S) | VX::ShrSV(VW::S), ..));
        }
        let mut tu = Vec::new();
        regalloc::term_uses(&b.term, &mut tu);
        for u in tu {
            nuse[u.0 as usize] += 1;
        }
    }
    if !any {
        return None;
    }
    // single-definition values: their defining instruction (block, index); constants' bits
    let mut at: Vec<(u32, u32)> = vec![(u32::MAX, 0); n];
    let mut konst: Vec<Option<u32>> = vec![None; n];
    for (bi, b) in f.blocks.iter().enumerate() {
        for (ii, i) in b.insts.iter().enumerate() {
            i.defs(|d| {
                if ndef[d.0 as usize] == 1 {
                    at[d.0 as usize] = (bi as u32, ii as u32);
                }
            });
            if let Inst::VConst(d, k) = i {
                if ndef[d.0 as usize] == 1 {
                    konst[d.0 as usize] = Some(*k);
                }
            }
        }
    }
    let is_k = |v: VReg| konst[v.0 as usize].is_some();
    let mut g = f.clone();
    // constant vreg -> its negation's vreg (built right after it)
    let mut neg: Vec<(VReg, VReg)> = Vec::new();
    let mut changed = false;
    for bi in 0..f.blocks.len() {
        for ii in 0..f.blocks[bi].insts.len() {
            let Inst::VX(op @ (VX::ShrUV(VW::S) | VX::ShrSV(VW::S)), d, x, a) = f.blocks[bi].insts[ii] else { continue };
            if ndef[a.0 as usize] != 1 || nuse[a.0 as usize] != 1 || at[a.0 as usize].0 == u32::MAX {
                continue;
            }
            let (ab, ai) = at[a.0 as usize];
            let amount = match f.blocks[ab as usize].insts[ai as usize] {
                // -(y + c) = (-c) - y
                Inst::VX(VX::Add(VW::S), _, y, k) if is_k(k) && !is_k(y) => Some((y, k, true)),
                Inst::VX(VX::Add(VW::S), _, k, y) if is_k(k) && !is_k(y) => Some((y, k, true)),
                // -(y - c) = c - y
                Inst::VX(VX::Sub(VW::S), _, y, k) if is_k(k) && !is_k(y) => Some((y, k, false)),
                _ => None,
            };
            let Some((y, k, negate)) = amount else { continue };
            let from = if negate {
                match neg.iter().find(|e| e.0 == k) {
                    Some(e) => e.1,
                    None => {
                        let m = g.vreg(Cls::V128);
                        neg.push((k, m));
                        m
                    }
                }
            } else {
                k
            };
            g.blocks[ab as usize].insts[ai as usize] = Inst::VX(VX::Sub(VW::S), a, from, y);
            let sh = if let VX::ShrUV(_) = op { VX::ArmUShl(VW::S) } else { VX::ArmSShl(VW::S) };
            g.blocks[bi].insts[ii] = Inst::VX(sh, d, x, a);
            changed = true;
        }
    }
    if !changed {
        return None;
    }
    // the negated constants, each right after its constant (it dominates every use of that one);
    // later positions of a block first, so earlier indices stay valid
    let mut ins: Vec<(u32, u32, Inst)> = neg.iter().map(|&(k, m)| (at[k.0 as usize].0, at[k.0 as usize].1 + 1, Inst::VConst(m, konst[k.0 as usize].unwrap().wrapping_neg()))).collect();
    ins.sort_by(|a, b| (b.0, b.1).cmp(&(a.0, a.1)));
    for (bi, ii, i) in ins {
        let b = &mut g.blocks[bi as usize];
        if b.pos.len() == b.insts.len() {
            let p = b.pos[ii as usize - 1];
            b.pos.insert(ii as usize, p);
        }
        b.insts.insert(ii as usize, i);
    }
    Some(g)
}

fn fold_lane_splats(f: &Func) -> Option<Func> {
    let n = f.vregs.len();
    let mut splat: Vec<Option<VReg>> = vec![None; n];
    let mut ndef = vec![0u32; n];
    for p in &f.params {
        ndef[p.0 as usize] += 1;
    }
    let mut any = false;
    // 32-bit constants (f32 bits / integers) by vreg
    let mut k32: Vec<Option<u32>> = vec![None; n];
    for b in &f.blocks {
        for i in &b.insts {
            i.defs(|d| ndef[d.0 as usize] += 1);
            match *i {
                Inst::VSplat(d, s) => {
                    splat[d.0 as usize] = Some(s);
                    any = true;
                }
                Inst::Fconst(d, bits, false) => k32[d.0 as usize] = Some(bits as u32),
                Inst::Iconst(d, k) => k32[d.0 as usize] = Some(k as u32),
                _ => {}
            }
        }
    }
    if !any {
        return None;
    }
    // a splat of a single-def constant is a vector constant (movi / fmov .4s #imm / one literal
    // load, rematerialised at its uses: `fmov d31, xzr; dup v.4s, v31.s[0]` was a 0.0 splat)
    let kof = |s: VReg| if ndef[s.0 as usize] == 1 { k32[s.0 as usize] } else { None };
    for v in 0..n {
        if let Some(s) = splat[v] {
            if f.vregs[s.0 as usize] != Cls::F32 || kof(s).is_some() {
                splat[v] = None;
            }
        }
    }
    // both single-def: the scalar holds the splatted value wherever the splat is read
    let of = |v: VReg| if ndef[v.0 as usize] == 1 { splat[v.0 as usize].filter(|s| ndef[s.0 as usize] == 1) } else { None };
    let mut g = f.clone();
    let mut changed = false;
    for b in &mut g.blocks {
        for i in &mut b.insts {
            match i {
                Inst::VSplat(d, s) if kof(*s).is_some() => {
                    *i = Inst::VConst(*d, kof(*s).unwrap());
                    changed = true;
                }
                Inst::VFma(_, _, _, _, bb, _) => {
                    if let Some(s) = of(*bb) {
                        *bb = s;
                        changed = true;
                    }
                }
                Inst::VX(VX::FMul(VW::S, nb), _, a, bb) if *a != *bb => {
                    if let Some(s) = of(*bb) {
                        *bb = s;
                        changed = true;
                    } else if let (Some(s), NanBits::Host) = (of(*a), *nb) {
                        *a = *bb;
                        *bb = s;
                        changed = true;
                    }
                }
                _ => {}
            }
        }
    }
    if !changed {
        return None;
    }
    // splats left without uses go (they are pure)
    let mut used = vec![false; n];
    for b in &g.blocks {
        for i in &b.insts {
            regalloc::for_uses(i, |u| used[u.0 as usize] = true);
        }
        let mut t = Vec::new();
        regalloc::term_uses(&b.term, &mut t);
        for u in t {
            used[u.0 as usize] = true;
        }
    }
    for b in &mut g.blocks {
        let keep: Vec<bool> = b.insts.iter().map(|i| !matches!(i, Inst::VSplat(d, _) if splat[d.0 as usize].is_some() && !used[d.0 as usize])).collect();
        if keep.iter().all(|k| *k) {
            continue;
        }
        let mut k = 0;
        b.insts.retain(|_| {
            k += 1;
            keep[k - 1]
        });
        let mut k = 0;
        b.pos.retain(|_| {
            k += 1;
            keep[k - 1]
        });
    }
    Some(g)
}

/// Whether the emitter encodes `op` (the legaliser expands the lane-wise ops it does not).
pub fn vx_native(op: VX) -> bool {
    use VW::*;
    match op {
        VX::Add(_) | VX::Sub(_) | VX::CmpEq(_) | VX::CmpGtS(_) | VX::And | VX::Or | VX::Xor | VX::AndNot => true,
        VX::AddSatU(_) | VX::SubSatU(_) | VX::AddSatS(_) | VX::SubSatS(_) | VX::Abs(_) | VX::Neg(_) => true,
        VX::MinS(_) | VX::MaxS(_) | VX::MinU(_) | VX::MaxU(_) => true,
        VX::MulLo(w) | VX::AvgU(w) => w != D,
        VX::ShlI(..) | VX::ShrUI(..) | VX::ShrSI(..) | VX::ShlV(_) | VX::ShrSV(_) | VX::ShrUV(_) => true,
        VX::ICmp(..) | VX::UCmp(..) => true,
        VX::FAdd(w, _) | VX::FSub(w, _) | VX::FMul(w, _) | VX::FDiv(w, _) | VX::FSqrt(w, _) => matches!(w, S | D),
        VX::FMinLt(w) | VX::FMaxGt(w) | VX::FMinNeon(w) | VX::FMaxNeon(w) | VX::FMinNum(w) | VX::FMaxNum(w) | VX::FCmp(_, w) => matches!(w, S | D),
        VX::FNeg(w) | VX::FAbs(w) | VX::FRound(w, _) | VX::FToI(w) | VX::IToF(w) => matches!(w, S | D),
        VX::Narrow(w) => w != B,
        VX::FCvtNarrow | VX::FCvtLo | VX::MoveMask(_) => true,
        _ => true,
    }
}

/// `compile` with measured block frequencies of `f` (0..=1 of its hottest block, per block), which
/// the register allocator weighs uses by instead of loop depth.
pub fn compile_freq(options: &relax_session::Options, f: &Func, env: &Env, freq: Option<&[f32]>) -> Result<Compiled, String> {
    let _warm = crate::probe::backend(&f.name);
    let _cost_codegen = crate::probe::phase(5);
    crate::probe::size(f);
    for b in &f.blocks {
        for i in &b.insts {
            if matches!(i, Inst::Cpuid(..) | Inst::Xgetbv(..)) {
                return Err(format!("{}: CPUID/XGETBV require x86_64; unsupported on arm64", f.name));
            }
            if let Inst::Call(_, _, r) = i {
                let (ri, rf) = count_classes(f, r);
                if ri > 2 || rf > 2 {
                    return Err(format!("{}: call with more than 2 integer or 2 float results", f.name));
                }
            }
        }
    }
    // vector ops without an instruction sequence here: per-lane scalar code
    let legal;
    // (MAKEPAD_RUN_OPT=-vxnative: every expandable op, the legaliser's reference meaning)
    let all = crate::stats::opt_off(options, "-vxnative");
    let native = |op| !all && vx_native(op);
    let f = if crate::legal::needed(f, &native, !all) {
        let mut g = f.clone();
        crate::legal::legalize(&mut g, &native, !all);
        legal = g;
        &legal
    } else {
        f
    };
    // chains of compares of one value against constants: jump tables, bit tests, binary search
    let switched = crate::switch::lower_switches(options, f, crate::switch::MIN_TABLE_CLUSTERS_ARM64);
    let (f, freq_sw) = match &switched {
        Some((g, origin)) => (g, freq.map(|fr| origin.iter().map(|b| fr.get(*b as usize).copied().unwrap_or(0.0)).collect::<Vec<f32>>())),
        None => (f, None),
    };
    let freq = if switched.is_some() { freq_sw.as_deref() } else { freq };
    let _cost_ra = crate::probe::phase(9);
    let t_ra = crate::stats::pass_clock();
    // negations read only by a fused multiply-add: its sign flags (fmsub / fnmadd / fnmsub)
    let nf = fold_fma_negs(f);
    let f = nf.as_ref().unwrap_or(f);
    // compares with a shared +0.0: each its own adjacent constant (fcmp #0.0; the shared zero
    // no longer holds a register over the function)
    let zc = split_zero_compares(f);
    let f = zc.as_ref().unwrap_or(f);
    // f32 splats read only by multiplies: the by-element forms (fmla / fmls / fmul v, v, v.s[0])
    let lanes = fold_lane_splats(f);
    let f = lanes.as_ref().unwrap_or(f);
    // a vector constant made again in its block (an unrolled array init's zero per element):
    // the first one's register
    let vk = if crate::stats::opt_off(options, "-a64vconst") { None } else { share_vconsts(f) };
    let f = vk.as_ref().unwrap_or(f);
    // right shifts by `y + c`: ushl / sshl by `-c - y` (no neg per shift)
    let shifts = fold_shift_amounts(f);
    let f = shifts.as_ref().unwrap_or(f);
    // immediates arm64 cannot encode and float constants, inside loops: registers set up once
    let hoisted = regalloc::hoist_loop_consts(f, &imm_fits, true, true, freq);
    let f = hoisted.as_ref().unwrap_or(f);
    // indexed accesses with displacements on one base and index: one address register
    let shared = regalloc::share_index_bases(f);
    let f = shared.as_ref().unwrap_or(f);
    // counter steps after the last read of the old value (no copy per iteration)
    let stepped = if env.opt_regs || f.full { regalloc::sink_increments(options, f) } else { None };
    let f = stepped.as_ref().unwrap_or(f);
    // branch compares at the end of their block (fused into cmp + b.cond)
    let brs = if env.opt_regs || f.full { regalloc::sink_branch_compares(options, f) } else { None };
    let f = brs.as_ref().unwrap_or(f);
    // the backend-internal rewrites above run after the last pipeline verify
    f.verify_after("arm64 backend rewrites (legalize/switch/folds/hoist/share/sink)")?;
    // v0-v7 are allocatable outside the instructions that name them (arm64_clobbers), except
    // in an `extern "C"` body returning an aggregate in registers: its Ret loads the parts into
    // v0-v3 before the result moves (c_ret_parts)
    let mut cfg = RegConfig { sym_class: env.sym_class, ..reg_config() };
    if f.cabi.as_ref().is_some_and(|s| matches!(cabi::plan(s, s.conv.target()).ret, RetPlan::Parts(_))) {
        cfg.flt_caller.retain(|x| *x >= 8);
    }
    let leaf = f.blocks.iter().all(|b| !b.insts.iter().any(|i| matches!(i, Inst::Call(..))));
    // likewise x0 / x1 (integer results: the return's moves place them) and x8 (the large-copy and
    // atomic loops' scratch) in a function that calls nothing: small leaf fns kept values in
    // callee-saved registers and saved / restored them on every return path
    if leaf {
        cfg.int_caller.extend_from_slice(&[0, 1]);
        let x8_free = f.blocks.iter().all(|b| {
            !b.insts.iter().any(|i| matches!(i, Inst::AtomicLoad(..) | Inst::AtomicStore(..) | Inst::AtomicRmw(..) | Inst::AtomicCas(..)) || matches!(i, Inst::Copy(_, _, n) if *n > 256))
        });
        if x8_free {
            cfg.int_caller.push(X8);
        }
    }
    let (split, mut al0) = regalloc::allocate_split(options, f, &cfg, env.opt_regs || f.full, freq);
    let f = split.as_ref().unwrap_or(f);
    if split.is_some() {
        f.verify_after("split_loops")?;
    }
    crate::remark::note_spills(f, &al0);
    crate::stats::pass_time(11, t_ra);
    drop(_cost_ra);
    let alloc_sig = regalloc::alloc_sig(&al0);
    regalloc::trace_alloc(f, &al0);
    // the allocation checker (MAKEPAD_RUN_OPT=+checkra; gates, fuzzer): a finding fails the compile
    if regalloc::check_on(options) {
        regalloc::check_or_fail(options, f, &cfg, &al0, env.opt_regs || f.full)?;
    }
    // a function past the branch ranges is emitted again in long mode: from the same
    // allocation (allocating twice doubled a 3.7 s allocation)
    let mut long = false;
    let mut tbz = true;
    loop {
    let al = al0;
    let _cost_emit = crate::probe::phase(10);
    let t_emit = crate::stats::pass_clock();
    let mut g = Gen {
        a: Asm { w: Vec::with_capacity(128) },
        test_fuse: None,
        sel_chain: None,
        zero_ins: None,
        fnmul_ins: None,
        zero_vec: Vec::new(),
        zero_val: Vec::new(),
        f32_load: Vec::new(),
        zero_cmp: Vec::new(),
        jt_fix: Vec::new(),
        last_spill: None,
        win: {
            let mut w: std::collections::HashMap<u32, Vec<(u32, u32, u8)>> = std::collections::HashMap::new();
            // chained: entered with the value in the register (no load)
            for &(v, a, z, r) in al.windows.iter().chain(al.chained.iter()) {
                w.entry(v).or_default().push((a, z, r));
            }
            w
        },
        low_only: crate::passes::low_bits_only(options, f),
        zero_above: Vec::new(),
        switch_in_range: Vec::new(),
        copy_reg: std::collections::HashMap::new(),
        evict_at: regalloc::evict_ops(&al),
        win_start: {
            let mut ws: std::collections::HashMap<u32, Vec<(u32, u8)>> = std::collections::HashMap::new();
            for &(v, a, _, r) in &al.windows {
                ws.entry(a).or_default().push((v, r));
            }
            ws
        },
        f,
        al,
        frame: 0,
        tail: Vec::new(),
        saved: 0,
        slot_off: Vec::new(),
        slot_pos: Vec::new(),
        block_pos: Vec::new(),
        next_block: Vec::new(),
        counted: false,
        fixups: Vec::new(),
        fconst_isel: isel_flags(options) & A64_FCONST != 0,
        qcopy: isel_flags(options) & A64_QCOPY != 0,
        fpool: Vec::new(),
        vpool: Vec::new(),
        apool: Vec::new(),
        asm_veneers: Vec::new(),
        sym_class: env.sym_class,
        near: env.near,
        dom_slot: -1,
        dom_read: Vec::new(),
        relocs: Vec::new(),
        text_words: 0,
        pc_map: Vec::new(),
        cur_pos: 0,
        cplan: None,
        cbuf: Vec::new(),
        cret_buf: 0,
        stage: 0,
        last_call_ret: 0,
        pads: Vec::new(),
        var_calls: Vec::new(),
        rir_map: Vec::new(),
        rir_map_on: RIR_MAP.load(std::sync::atomic::Ordering::Relaxed),
        long,
        tbz: tbz && !long,
    };
    g.zero_above = crate::passes::zero_above(options, g.f, &g.low_only);
    (g.zero_vec, g.f32_load) = zero_vectors(g.f);
    g.zero_val = zero_values(g.f, &g.zero_vec);
    g.zero_cmp = zero_compare_consts(g.f);
    if env.opt_regs || g.f.full {
        g.copy_reg = regalloc::copy_reuse(options, g.f, &g.al);
    }
    if !crate::stats::opt_off(options, "-swrange") {
        g.switch_in_range = crate::switch::switch_in_range(g.f);
    }
    BRANCH_RANGE.with(|c| c.set(false));
    TBZ_RANGE.with(|c| c.set(false));
    g.gen(options);
    // a copy between overlapping frame bytes is a bug of a pass or of the frame layout: a build
    // error where passes are verified (it is emitted in memmove order otherwise)
    if crate::rir::verify_passes_on() {
        if let Some(e) = crate::frame::overlapping_copy_at(g.f, &g.slot_pos) {
            return Err(format!("arm64 frame layout: {}", e));
        }
    }
    if TBZ_RANGE.with(|c| c.replace(false)) && g.tbz && !BRANCH_RANGE.with(|c| c.get()) {
        // a tbz past +-32 KB: emit again with tst + b.cond (from the same allocation)
        tbz = false;
        al0 = g.al;
        continue;
    }
    if BRANCH_RANGE.with(|c| c.replace(false)) {
        if !long {
            // past +-1 MB: emit again with relaxed branches and no literal pools (emission
            // reads the allocation only)
            long = true;
            al0 = g.al;
            continue;
        }
        return Err(format!("{}: function too large for arm64 branch ranges ({} instructions)", f.name, g.a.w.len()));
    }
    let frame = g.frame as u32;
    let code = words_to_bytes(&g.a.w);
    let text_len = (g.text_words * 4) as u32;
    crate::stats::pass_time(12, t_emit);
    crate::probe::code(code.len());
    // where the named locals are at each call (makepad inspect `locals`; side metadata)
    let mut pc_map = std::mem::take(&mut g.pc_map);
    if !g.f.naked && !g.f.dbg.is_empty() {
        let (saved, fsz, slot_off) = (g.saved, g.frame, &g.slot_off);
        // sp = fp - frame in the body
        let spill = |o: i32| -(saved as i32 + o);
        let slot = |s: u32| (slot_off[s as usize] - fsz) as i32;
        let fr = crate::dbgloc::Frame { callee_int: &cfg.int_callee, callee_flt: &cfg.flt_callee, arg_regs: &[0, 1, 2, 3, 4, 5, 6, 7], captured: &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13], call_writes: &[S0, S1, IP0, IP1], spill_fp: &spill, slot_fp: &slot };
        let vars = crate::dbgloc::build(g.f, &g.al, &fr, &g.var_calls);
        let e = crate::dbgloc::pc_entries(&vars, g.f.file);
        crate::dbgloc::note_stats(&pc_map, &e, vars.len());
        pc_map.extend(e);
    } else {
        crate::dbgloc::note_stats(&pc_map, &[], 0);
    }
    return Ok(Compiled { code, pc_map, frame_size: frame, text_len, alloc_sig, relocs: g.relocs, rir_emitted: if g.rir_map_on { emitted_dump(g.f) } else { String::new() }, rir_map: g.rir_map });
    }
}

thread_local! {
    /// a branch or literal load of the function being emitted did not reach its target
    static BRANCH_RANGE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// a tbz / tbnz of the function being emitted did not reach its target (+-32 KB)
    static TBZ_RANGE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

// ------------------------------------------------------------ runtime glue

/// The lazy-compile entry shared by all stubs: saves the argument registers, calls
/// `compile_fn(slot) -> code address`, restores them and jumps to the code.
/// On entry x16 holds the slot index.
pub fn lazy_entry(host_compile: u64) -> Vec<u8> {
    let mut a = Asm { w: Vec::new() };
    a.stp_pre(FP, LR, SP, -16);
    a.mov_sp(FP, SP);
    a.sub_imm(SP, SP, 144);
    for k in 0..4u8 {
        a.stp(false, 2 * k, 2 * k + 1, SP, 16 * k as i32);
        a.stp(true, 2 * k, 2 * k + 1, SP, 64 + 16 * k as i32);
    }
    // x8: the C ABI's indirect result address (extern "C" bodies reached through a stub)
    a.ldst(STRX, 8, X8, SP, 128);
    a.mov(0, IP0);
    a.mov_imm64_fixed(IP1, host_compile);
    a.blr(IP1);
    a.mov(IP0, 0);
    for k in 0..4u8 {
        a.ldp(false, 2 * k, 2 * k + 1, SP, 16 * k as i32);
        a.ldp(true, 2 * k, 2 * k + 1, SP, 64 + 16 * k as i32);
    }
    a.ldst(LDRX, 8, X8, SP, 128);
    a.mov_sp(SP, FP);
    a.ldp_post(FP, LR, SP, 16);
    a.br(IP0);
    words_to_bytes(&a.w)
}

/// Per-slot thunk (the permanent address of a function, `THUNK_SIZE` bytes, 8-aligned): its
/// head (`thunk_head`) is `b` straight to the slot's code, or, when that is out of reach or not
/// known, `b +4`: `ldr x16, =table_word; ldr x16, [x16]; br x16`. Built with the head to `+4`.
pub fn thunk(slot_addr: u64) -> Vec<u8> {
    let mut a = Asm { w: Vec::with_capacity(THUNK_SIZE / 4) };
    a.e(THUNK_HEAD_TABLE);
    // ldr x16, [pc + 12] (the literal at +16)
    a.e(0x5800_0000 | 3 << 5 | IP0 as u32);
    a.ldst(LDRX, 8, IP0, IP0, 0);
    a.br(IP0);
    a.e(slot_addr as u32);
    a.e((slot_addr >> 32) as u32);
    while a.w.len() * 4 < THUNK_SIZE {
        a.udf();
    }
    words_to_bytes(&a.w)
}

/// The thunk head that goes the table-word way: `b +4`.
const THUNK_HEAD_TABLE: u32 = 0x1400_0001;

/// The first instruction of the thunk at `at` (one aligned 4-byte store patches it; `b` may be
/// modified while other cores execute it) for a slot whose table word holds `target`: a direct
/// `b` when in reach (+-128 MB), else the table-word path.
pub fn thunk_head(at: u64, target: u64) -> u32 {
    let d = target.wrapping_sub(at) as i64;
    if target == 0 || d & 3 != 0 || !(-(1i64 << 27)..1i64 << 27).contains(&d) {
        return THUNK_HEAD_TABLE;
    }
    0x1400_0000 | ((d >> 2) as u32 & 0x03FF_FFFF)
}

/// A veneer: `ldr x16, [pc + 8]; br x16; .quad target` (16 bytes; a near branch to a far
/// target, x16 only, as the AAPCS allows a veneer).
pub fn veneer(target: u64) -> Vec<u8> {
    let mut a = Asm { w: Vec::with_capacity(4) };
    a.e(0x5800_0000 | 2 << 5 | IP0 as u32);
    a.br(IP0);
    a.e(target as u32);
    a.e((target >> 32) as u32);
    words_to_bytes(&a.w)
}

/// Bytes per count stub (`count_stub`; the stubs of a block are this far apart).
pub const COUNT_STUB_SIZE: usize = 80;

/// Per-slot cost budget (tier-up) at `at` (0: not known yet): while a function runs below O3 its
/// table slot points here. `*budget -= weight` (the call's estimated cost, at most 4095); at or
/// below 0 (signed) jumps to `hot` (a lazy_entry-style glue: x16 = slot, returns the address to
/// continue at), else to the code: `b` when in reach, else through x16. Argument registers, sp and
/// lr are untouched; only x16/x17 (the AAPCS veneer registers) are used.
pub fn count_stub(at: u64, counter: u64, slot: u32, code: u64, hot: u64, weight: u32) -> Vec<u8> {
    let mut a = Asm { w: Vec::with_capacity(COUNT_STUB_SIZE / 4) };
    a.mov_imm64_fixed(IP0, counter);
    a.ldst(LDRX, 8, IP1, IP0, 0);
    // subs x17, x17, #weight
    a.e(0xF100_0000 | weight.clamp(1, 4095) << 10 | (IP1 as u32) << 5 | IP1 as u32);
    a.ldst(STRX, 8, IP1, IP0, 0);
    // b.le hot (patched below)
    let beq = a.pos();
    a.e(0x5400_000D);
    let pc = at + 4 * a.pos() as u64;
    let d = code.wrapping_sub(pc) as i64;
    if at != 0 && d & 3 == 0 && (-(1i64 << 27)..1i64 << 27).contains(&d) {
        a.e(0x1400_0000 | ((d >> 2) as u32 & 0x03FF_FFFF));
    } else {
        a.mov_imm64_fixed(IP0, code);
        a.br(IP0);
    }
    a.w[beq] |= ((a.pos() - beq) as u32) << 5;
    a.e(0xD280_0000 | (slot & 0xffff) << 5 | IP0 as u32);
    a.e(0xF2A0_0000 | (slot >> 16) << 5 | IP0 as u32);
    a.mov_imm64_fixed(IP1, hot);
    a.br(IP1);
    assert!(a.w.len() * 4 <= COUNT_STUB_SIZE);
    words_to_bytes(&a.w)
}

/// Per-slot stub: `mov x16, slot; b lazy_entry` (absolute via x17).
pub fn stub(slot: u32, lazy: u64) -> Vec<u8> {
    let mut a = Asm { w: Vec::with_capacity(12) };
    a.e(0xD280_0000 | (slot & 0xffff) << 5 | IP0 as u32);
    a.e(0xF2A0_0000 | (slot >> 16) << 5 | IP0 as u32);
    a.mov_imm64_fixed(IP1, lazy);
    a.br(IP1);
    words_to_bytes(&a.w)
}

/// enter(fn, arg, ctx): saves the callee-saved registers and sp into *ctx, calls fn(arg).
/// Returns 0 normally. `leave(ctx, code)` (at the returned offset) restores them and
/// returns `code` from enter (used by panics, faults and hangs to get back to the host).
pub fn enter_leave() -> (Vec<u8>, usize) {
    let mut a = Asm { w: Vec::new() };
    // enter: x0 = fn, x1 = arg, x2 = ctx
    a.stp_pre(FP, LR, SP, -16);
    a.mov_sp(FP, SP);
    a.sub_imm(SP, SP, 160);
    for k in 0..5u8 {
        a.stp(false, 19 + 2 * k, 20 + 2 * k, SP, 16 * k as i32);
    }
    for k in 0..4u8 {
        a.stp(true, 8 + 2 * k, 9 + 2 * k, SP, 80 + 16 * k as i32);
    }
    // ctx[0] = sp
    a.mov_sp(IP0, SP);
    a.ldst(STRX, 8, IP0, 2, 0);
    a.mov(IP0, 0);
    a.mov(0, 1);
    a.blr(IP0);
    a.mov_imm(0, 0);
    let tail = a.pos();
    for k in 0..5u8 {
        a.ldp(false, 19 + 2 * k, 20 + 2 * k, SP, 16 * k as i32);
    }
    for k in 0..4u8 {
        a.ldp(true, 8 + 2 * k, 9 + 2 * k, SP, 80 + 16 * k as i32);
    }
    a.add_imm(SP, SP, 160);
    a.ldp_post(FP, LR, SP, 16);
    a.ret();
    // leave(ctx = x0, code = x1)
    let leave = a.pos();
    a.ldst(LDRX, 8, IP0, 0, 0);
    a.mov_sp(SP, IP0);
    a.mov(0, 1);
    let j = a.b();
    a.patch(j, tail);
    (words_to_bytes(&a.w), leave * 4)
}

/// Bytes of the register block the panic trampolines store (rir REG_BLOCK layout, arm64).
pub const REG_BLOCK_BYTES: u32 = 288;

/// Stores the register block [fp, lr, x19..x28, d8..d15, x0..x15] at sp (sp lowered first).
fn store_reg_block(a: &mut Asm) {
    a.sub_imm(SP, SP, REG_BLOCK_BYTES);
    a.stp(false, FP, LR, SP, 0);
    for k in 0..5u8 {
        a.stp(false, 19 + 2 * k, 20 + 2 * k, SP, 16 + 16 * k as i32);
    }
    for k in 0..4u8 {
        a.stp(true, 8 + 2 * k, 9 + 2 * k, SP, 96 + 16 * k as i32);
    }
    // the caller-saved registers as the call began (makepad inspect `locals`: its arguments and
    // the values the caller keeps there, at a stop's innermost call)
    for k in 0..8u8 {
        a.stp(false, 2 * k, 2 * k + 1, SP, 160 + 16 * k as i32);
    }
}

/// JIT trampoline in front of a host panic function (or the unwinder's resume entry): stores
/// the JIT caller's register state as a block on the stack -- [fp, return address, x19..x28,
/// d8..d15, x0..x15] (36 words) -- passes its address as argument 6 (x6), then tail-jumps to
/// `target`, which never returns. The unwinder starts its frame walk from the block.
pub fn panic_tramp(target: u64) -> Vec<u8> {
    let mut a = Asm { w: Vec::new() };
    store_reg_block(&mut a);
    a.mov_sp(6, SP);
    a.mov_imm(7, 0);
    a.mov_imm64_fixed(IP0, target);
    a.br(IP0);
    words_to_bytes(&a.w)
}

/// JIT trampoline in front of a host function that returns (makepad inspect's breakpoint):
/// stores the register block as `panic_tramp` does, makes it the frame record (x29 = its
/// address: [fp, return address] lead it, so frame walks go on into the JIT caller), passes its
/// address as argument 6 (x6, x7 = 0), calls `target` and returns to the JIT caller.
pub fn regs_call_tramp(target: u64) -> Vec<u8> {
    let mut a = Asm { w: Vec::new() };
    store_reg_block(&mut a);
    a.mov_sp(FP, SP);
    a.mov_sp(6, SP);
    a.mov_imm(7, 0);
    a.mov_imm64_fixed(IP0, target);
    a.blr(IP0);
    a.ldp(false, FP, LR, SP, 0);
    a.add_imm(SP, SP, REG_BLOCK_BYTES);
    a.ret();
    words_to_bytes(&a.w)
}

/// land(regs, fp, sp, pc): continues at `pc` (a landing stub) with x19..x28 and d8..d15
/// from the register block `regs` (panic_tramp's layout), x29 = fp and sp = sp. Never returns.
pub fn land() -> Vec<u8> {
    let mut a = Asm { w: Vec::new() };
    for k in 0..5u8 {
        a.ldp(false, 19 + 2 * k, 20 + 2 * k, 0, 16 + 16 * k as i32);
    }
    for k in 0..4u8 {
        a.ldp(true, 8 + 2 * k, 9 + 2 * k, 0, 96 + 16 * k as i32);
    }
    a.mov(FP, 1);
    a.mov_sp(SP, 2);
    a.br(3);
    words_to_bytes(&a.w)
}

/// `br target` through x16 (the AAPCS intra-procedure-call register a veneer may use):
/// every argument register, sp and lr arrive at `target` unchanged.
pub fn jump_to(target: u64) -> Vec<u8> {
    let mut a = Asm { w: Vec::new() };
    a.mov_imm64_fixed(IP0, target);
    a.br(IP0);
    words_to_bytes(&a.w)
}

/// `x0 = arg; br target` (tail jump keeping lr: the caller's frame stays the JIT one).
pub fn jump_with_arg0(arg: u64, target: u64) -> Vec<u8> {
    let mut a = Asm { w: Vec::new() };
    a.mov_imm64_fixed(0, arg);
    a.mov_imm64_fixed(IP0, target);
    a.br(IP0);
    words_to_bytes(&a.w)
}

/// This thread's thread-local block in x16, preserving every other register JIT code may hold
/// (x0-x15, d0-d7, d16-d29): the fast path `tls_addr` inlines (slot `key` of the thread's slot
/// array), and when the slot is still null `host() -> block`. `tls_addr` calls it after its own
/// fast path found null; a prologue that needs the domain base calls it directly.
pub fn tls_slow(host: u64, key: u64) -> Vec<u8> {
    let mut a = Asm { w: Vec::new() };
    // mrs x16, <slot base>; and x16, x16, #~7; mov x17, #key*8; ldr x16, [x16, x17]
    a.e(MRS_SLOT_BASE | IP0 as u32);
    a.e(0x9243_F000 | (IP0 as u32) << 5 | IP0 as u32);
    a.mov_imm(IP1, (key * 8) as i64);
    a.e(0xF860_6800 | (IP1 as u32) << 16 | (IP0 as u32) << 5 | IP0 as u32);
    // cbz x16, +2 (past the ret): the block exists, return it
    a.e(0xB400_0000 | 2 << 5 | IP0 as u32);
    a.ret();
    a.stp_pre(FP, LR, SP, -16);
    a.mov_sp(FP, SP);
    a.sub_imm(SP, SP, 320);
    for k in 0..8u8 {
        a.stp(false, 2 * k, 2 * k + 1, SP, 16 * k as i32);
    }
    for k in 0..4u8 {
        a.stp(true, 2 * k, 2 * k + 1, SP, 128 + 16 * k as i32);
    }
    for k in 0..7u8 {
        a.stp(true, 16 + 2 * k, 17 + 2 * k, SP, 192 + 16 * k as i32);
    }
    a.mov_imm64_fixed(IP1, host);
    a.blr(IP1);
    a.mov(IP0, 0);
    for k in 0..8u8 {
        a.ldp(false, 2 * k, 2 * k + 1, SP, 16 * k as i32);
    }
    for k in 0..4u8 {
        a.ldp(true, 2 * k, 2 * k + 1, SP, 128 + 16 * k as i32);
    }
    for k in 0..7u8 {
        a.ldp(true, 16 + 2 * k, 17 + 2 * k, SP, 192 + 16 * k as i32);
    }
    a.mov_sp(SP, FP);
    a.ldp_post(FP, LR, SP, 16);
    a.ret();
    words_to_bytes(&a.w)
}

/// `mrs xN, <register holding the thread's slot array>` (N or-ed in). macOS: tpidrro_el0 (the
/// pthread TSD base; its low 3 bits carry the cpu number, masked off after). Android: tpidr_el0,
/// bionic's thread pointer = its TLS slot array (bionic libc/platform/bionic/tls_defines.h).
#[cfg(not(target_os = "android"))]
const MRS_SLOT_BASE: u32 = 0xD53B_D060;
#[cfg(target_os = "android")]
const MRS_SLOT_BASE: u32 = 0xD53B_D040;

pub fn tsd_reader() -> Vec<u8> {
    let mut a = Asm { w: Vec::new() };
    a.e(MRS_SLOT_BASE | IP0 as u32);
    a.e(0x9243_F000 | (IP0 as u32) << 5 | IP0 as u32);
    // ldr x0, [x16, x0, lsl #3]
    a.e(0xF860_7800 | (IP0 as u32) << 5);
    a.ret();
    words_to_bytes(&a.w)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// R13: a conditional branch over more than 1 MB of code (imm19) is relaxed into an
    /// inverted short branch over a `b` instead of being truncated into other code (Amp's
    /// O3 script parser, 1.9 MB, branched into other functions).
    #[test]
    fn branch_out_of_range_is_relaxed() {
        let options = &relax_session::Options::default();
        let mut f = Func::new("big".to_string(), 0);
        let p = f.vreg(Cls::I);
        f.params.push(p);
        f.rets.push(Cls::I);
        let b0 = f.block();
        let b1 = f.block();
        let b2 = f.block();
        f.blocks[b0 as usize].term = Term::Branch(p, b1, b2);
        let mut acc = p;
        for k in 0..300_000 {
            let d = f.vreg(Cls::I);
            f.blocks[b1 as usize].insts.push(Inst::IBinI(IOp::Add, IntTy { bits: 64, signed: false }, d, acc, k + 1));
            f.blocks[b1 as usize].pos.push(0);
            acc = d;
        }
        f.blocks[b1 as usize].term = Term::Ret(vec![acc]);
        f.blocks[b2 as usize].term = Term::Ret(vec![p]);
        let env = Env { slots: SlotsPtr::NONE, tls_key: 0, tls_slow: 0, resume: 0, opt_regs: true, syms: 0, sym_class: 0, near: false };
        let r = compile(options, &f, &env).expect("relaxed");
        let w: Vec<u32> = r.code[..r.text_len as usize].chunks(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
        // the entry block's branch on p: cbz/cbnz skipping one word, then `b` beyond 1 MB
        let k = w.iter().position(|x| x & 0x7E00_0000 == 0x3400_0000).expect("cbz/cbnz");
        assert_eq!((w[k] >> 5) & 0x7FFFF, 2, "{:08x}", w[k]);
        assert_eq!(w[k + 1] & 0xFC00_0000, 0x1400_0000, "{:08x}", w[k + 1]);
        assert!((w[k + 1] & 0x03FF_FFFF) as usize > (1 << 18), "{:08x}", w[k + 1]);
        // long mode builds a pooled f64 constant in a register: as an f64 (fmov dN, x16)
        let fc = f.vreg(Cls::F64);
        let fb = f.vreg(Cls::I);
        let bits = 0.1f64.to_bits();
        f.blocks[b2 as usize].insts.push(Inst::Fconst(fc, bits, true));
        f.blocks[b2 as usize].pos.push(0);
        f.blocks[b2 as usize].insts.push(Inst::Conv(Conv::FToBits(true), fb, fc));
        f.blocks[b2 as usize].pos.push(0);
        f.blocks[b2 as usize].term = Term::Ret(vec![fb]);
        let r = compile(options, &f, &env).expect("relaxed with an f64 constant");
        let w: Vec<u32> = r.code[..r.text_len as usize].chunks(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
        assert!(w.iter().any(|x| x & 0xFFFF_FFE0 == 0x9E67_0200), "no fmov dN, x16");
        assert!(!w.iter().any(|x| x & 0xFFFF_FFE0 == 0x1E27_0200), "f64 constant built as an f32");
        assert!(!w.iter().any(|x| x & 0xFF00_0000 == 0x5C00_0000), "literal pool load in long mode");
        f.blocks[b2 as usize].insts.clear();
        f.blocks[b2 as usize].pos.clear();
        f.blocks[b2 as usize].term = Term::Ret(vec![p]);
        // a small one still compiles
        f.blocks[b1 as usize].insts.truncate(1000);
        f.blocks[b1 as usize].pos.truncate(1000);
        let last = match f.blocks[b1 as usize].insts.last() {
            Some(Inst::IBinI(_, _, d, ..)) => *d,
            _ => p,
        };
        f.blocks[b1 as usize].term = Term::Ret(vec![last]);
        assert!(compile(options, &f, &env).is_ok());
    }

    /// A branch on one bit (`x & 1 << k` against zero) is tbz / tbnz, for bits in either word;
    /// past their +-32 KB range the function is emitted again with tst + b.cond, and both
    /// versions take the right side (run on arm64 hosts).
    #[test]
    #[cfg(target_arch = "aarch64")]
    fn single_bit_branches_use_tbz_and_fall_back_past_its_range() {
        let options = &relax_session::Options::default();
        let env = Env { slots: SlotsPtr::NONE, tls_key: 0, tls_slow: 0, resume: 0, opt_regs: true, syms: 0, sym_class: 0, near: false };
        for (bit, filler) in [(0u32, 0usize), (13, 0), (31, 0), (32, 0), (63, 0), (5, 12_000), (40, 12_000)] {
            let mut f = Func::new("tbz".to_string(), 0);
            let p = f.vreg(Cls::I);
            f.params.push(p);
            f.rets.push(Cls::I);
            let (b0, b1, b2) = (f.block(), f.block(), f.block());
            let (t, c) = (f.vreg(Cls::I), f.vreg(Cls::I));
            let u = IntTy { bits: 64, signed: false };
            f.blocks[b0 as usize].insts = vec![Inst::IBinI(IOp::And, u, t, p, 1i64 << bit), Inst::ICmpI(Cond::Ne, false, c, t, 0)];
            f.blocks[b0 as usize].pos = vec![0, 0];
            f.blocks[b0 as usize].term = Term::Branch(c, b2, b1);
            // b1 (bit clear): p + 1 + filler adds, laid out between the branch and b2
            let mut acc = p;
            for k in 0..filler + 1 {
                let d = f.vreg(Cls::I);
                f.blocks[b1 as usize].insts.push(Inst::IBinI(IOp::Add, u, d, acc, if k == 0 { 1 } else { 0x1000 }));
                f.blocks[b1 as usize].pos.push(0);
                acc = d;
            }
            f.blocks[b1 as usize].term = Term::Ret(vec![acc]);
            let k2 = f.vreg(Cls::I);
            f.blocks[b2 as usize].insts = vec![Inst::Iconst(k2, 7)];
            f.blocks[b2 as usize].pos = vec![0];
            f.blocks[b2 as usize].term = Term::Ret(vec![k2]);
            let r = compile(options, &f, &env).unwrap();
            let w: Vec<u32> = r.code[..r.text_len as usize].chunks(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
            let has_tbz = w.iter().any(|x| x & 0x7E00_0000 == 0x3600_0000);
            assert_eq!(has_tbz, filler == 0, "bit {bit} filler {filler}: tbz {has_tbz}");
            // the code is arm64: it runs only on an arm64 host (an x64 host checks the encoding only)
            if !cfg!(target_arch = "aarch64") {
                continue;
            }
            let code = crate::exec::Code::new(&r.code).unwrap();
            let run: extern "C" fn(u64) -> u64 = unsafe { std::mem::transmute(code.ptr()) };
            let x = 0x5a5a_0000_0000_1234u64 & !(1u64 << bit);
            assert_eq!(run(x | 1 << bit), 7, "bit {bit} set");
            assert_eq!(run(x), x + 1 + 0x1000 * filler as u64, "bit {bit} clear");
        }
    }

    /// A select chain reads the flags of the compare right before it, never stale ones: here
    /// the compare's other select sits in the next block, after a fused `cmp; b.cond` rewrote the
    /// flags, so it must test the materialised bool again (`cmp c, #0; csel`). It used to read the
    /// branch's flags and pick the wrong side: zune-jpeg's `get_mut(..).unwrap_or(&mut tmp)`
    /// wrote every IDCT row into `tmp` at O3, and Stage's thumbnails came out as garbage.
    #[test]
    fn a_select_after_a_fused_branch_tests_its_bool_again() {
        let options = &relax_session::Options::default();
        let mut f = Func::new("selchain".to_string(), 0);
        let it = IntTy { bits: 64, signed: false };
        let (p, q) = (f.vreg(Cls::I), f.vreg(Cls::I));
        f.params.extend([p, q]);
        f.rets.push(Cls::I);
        let (b0, b1, b2) = (f.block(), f.block(), f.block());
        let (c, k, s, e, d) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I));
        for inst in [Inst::Iconst(k, 8), Inst::ICmpI(Cond::Ne, false, c, p, 0), Inst::Sel(s, c, q, k), Inst::ICmpI(Cond::Eq, false, e, s, 8)] {
            f.blocks[b0 as usize].insts.push(inst);
            f.blocks[b0 as usize].pos.push(0);
        }
        f.blocks[b0 as usize].term = Term::Branch(e, b1, b2);
        f.blocks[b1 as usize].insts.push(Inst::Sel(d, c, p, q));
        f.blocks[b1 as usize].pos.push(0);
        f.blocks[b1 as usize].term = Term::Ret(vec![d]);
        let s2 = f.vreg(Cls::I);
        f.blocks[b2 as usize].insts.push(Inst::IBinI(IOp::Add, it, s2, s, 1));
        f.blocks[b2 as usize].pos.push(0);
        f.blocks[b2 as usize].term = Term::Ret(vec![s2]);
        let env = Env { slots: SlotsPtr::NONE, tls_key: 0, tls_slow: 0, resume: 0, opt_regs: true, syms: 0, sym_class: 0, near: false };
        let r = compile(options, &f, &env).expect("compiles");
        let w: Vec<u32> = r.code[..r.text_len as usize].chunks(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
        let csel = |x: u32| x & 0x7FE0_0C00 == 0x1A80_0000;
        let cmp0 = |x: u32| x & 0x7FFF_FC1F == 0x7100_001F;
        let sels: Vec<usize> = (0..w.len()).filter(|&i| csel(w[i])).collect();
        assert_eq!(sels.len(), 2, "two selects: {:08x?}", w);
        // the first select reads the chained compare's flags; the second, after the fused
        // branch (b.cond), tests the bool again
        let bcond = (sels[0]..sels[1]).rev().find(|&i| w[i] & 0xFF00_0010 == 0x5400_0000).expect("fused b.cond");
        assert!((bcond + 1..sels[1]).any(|i| cmp0(w[i])), "the select after the fused branch reads stale flags: {:08x?}", w);
    }

    /// The fused entry branches (`x == 0` as cbz/cbnz, `x & 8 == 0` as tst + b.cond) also relax
    /// past +-1 MB: the inverted short branch over a `b`.
    #[test]
    fn fused_branches_are_relaxed() {
        let options = &relax_session::Options::default();
        for tst in [false, true] {
            let mut f = Func::new("bigf".to_string(), 0);
            let p = f.vreg(Cls::I);
            f.params.push(p);
            f.rets.push(Cls::I);
            let (b0, b1, b2) = (f.block(), f.block(), f.block());
            let c = f.vreg(Cls::I);
            let it = IntTy { bits: 64, signed: false };
            let mut x = p;
            if tst {
                x = f.vreg(Cls::I);
                f.blocks[b0 as usize].insts.push(Inst::IBinI(IOp::And, it, x, p, 8));
                f.blocks[b0 as usize].pos.push(0);
            }
            f.blocks[b0 as usize].insts.push(Inst::ICmpI(Cond::Eq, false, c, x, 0));
            f.blocks[b0 as usize].pos.push(0);
            f.blocks[b0 as usize].term = Term::Branch(c, b1, b2);
            let mut acc = p;
            for k in 0..300_000 {
                let d = f.vreg(Cls::I);
                f.blocks[b1 as usize].insts.push(Inst::IBinI(IOp::Add, it, d, acc, k + 1));
                f.blocks[b1 as usize].pos.push(0);
                acc = d;
            }
            f.blocks[b1 as usize].term = Term::Ret(vec![acc]);
            f.blocks[b2 as usize].term = Term::Ret(vec![p]);
            let env = Env { slots: SlotsPtr::NONE, tls_key: 0, tls_slow: 0, resume: 0, opt_regs: true, syms: 0, sym_class: 0, near: false };
            let r = compile(options, &f, &env).expect("relaxed");
            let w: Vec<u32> = r.code[..r.text_len as usize].chunks(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
            // the first conditional branch skips one word; a `b` beyond 1 MB follows
            let k = w.iter().position(|x| x & 0x7E00_0000 == 0x3400_0000 || x & 0xFF00_0010 == 0x5400_0000).expect("conditional branch");
            assert_eq!((w[k] >> 5) & 0x7FFFF, 2, "{:08x}", w[k]);
            assert_eq!(w[k + 1] & 0xFC00_0000, 0x1400_0000, "{:08x}", w[k + 1]);
            assert!((w[k + 1] & 0x03FF_FFFF) as usize > (1 << 18), "{:08x}", w[k + 1]);
            if tst {
                // tst x, #8 (ANDS xzr) before it, no and
                assert_eq!(w[k - 1] & 0xFF80_001F, 0xF200_001F, "{:08x}", w[k - 1]);
            }
        }
    }
}

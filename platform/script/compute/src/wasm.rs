//! WebAssembly backend: AIR compiled to a wasm module at run time, for
//! hosts that run inside a browser (or any wasm engine) and cannot map
//! native code. The browser compiles the module to machine code, so a
//! kernel runs as fast code there instead of on the AIR interpreter.
//!
//! # Module ABI
//!
//! One module per document's kernels ([`module`]: one [`Entry`] per
//! program, each scalar or four wide). A host gives a kernel one entry:
//! four wide when it has that form ([`simd_supported`] and the kernel is
//! element-local with outputs holding every record), else scalar.
//!
//! - Import: `env.memory`, the host's linear memory (min 1 page; shared
//!   with a maximum when [`Target::shared`], as a threaded host's memory
//!   is declared). Nothing else is imported: no host calls, no tables.
//! - Exports: `run0 .. runN`, one per entry in order, each of type
//!   `(ctx, state, shared, table, n, frame) -> ()`, all i32, all but `n`
//!   byte addresses in that memory:
//!   - `ctx`: the kernel's ctx words (element base, count, time, seed,
//!     cancel, overflow, ..., params; [`crate::kernel::Kernel::ctx_words`]).
//!   - `state`: one word (kernels have no state); an audio program's
//!     voice or effect state (below).
//!   - `shared`: the shared tables ([`crate::kernel::Kernel::shared_table`]).
//!   - `table`: per host buffer an i32 pair (byte address, length in
//!     words), buffer 0 the control word; lengths at least 1 and below
//!     2^31, each buffer inside the memory.
//!   - `n`: elements to run from ctx's element base, any count (0
//!     included) in either form: a four-wide entry runs its last partial
//!     group of four masked (the lanes past n store nothing). One element
//!     per call (n = 1, the element base advanced) runs elements strictly
//!     in order on a four-wide entry, as the scalar entry does: the form
//!     for calls whose outputs may not hold every record.
//!   - `frame`: per-call scratch, owned by the call (one per thread):
//!     `frame_words * 4` bytes for a scalar entry; `frame_words * 16 + 64`
//!     bytes for a four-wide one (64 bytes of operands for the exact
//!     multiply-add's fix-up, then word w of lane l at `64 + 16 w + 4 l`).
//!     16-byte alignment is best (not required). `frame_words * 4 + 16`
//!     words serve both.
//! - Audio programs (instruments and effects: AIR with `In`/`Out`, made
//!   by the audio front end) are compiled scalar, same type and order:
//!   - `ctx`: the shader's ctx words (rate, frame of the first sample,
//!     ..., params; `AudioShader::ctx_words`), `state` one voice's (or the
//!     effect's) state words, `shared` its init()-built tables (read only).
//!   - `table`: four i32 byte addresses: input channels 0 and 1, output
//!     channels 0 and 1, each `n` f32 frames (16 bytes; an instrument's
//!     inputs may be one zero buffer, its outputs anything to mix into).
//!     The program ADDS into the outputs; `In`/`Out` clamp the frame index
//!     into `0..n` exactly as the interpreter does.
//!   - `n`: frames, 1..=MAX_FRAMES (0 returns at once), `frame` the
//!     scalar scratch (`frame_words * 4` bytes).
//!   One call per voice: a host runs every active voice's state through
//!   the same entry. The math is the program's: AIR's ops as written, no
//!   fusion (the audio front end makes no float multiply-add), so the
//!   output is bit-identical to the interpreter and the native backend.
//! - Calls are linked by the host (e.g. into its function table and called
//!   indirectly); a call returns normally on any input: no trap, no access
//!   outside the regions above (every access clamps, see below).
//! - [`fma_probe`] is a separate tiny module the host runs once to decide
//!   [`Target::relaxed_fma`]; an engine without relaxed SIMD (it refuses
//!   the probe) gets modules with no relaxed op in them.
//!
//! The code is held to the same contract as every backend: bit-identical
//! to [`crate::ir::run`]. Every op has its exact AIR meaning (wasm's own
//! integer division traps, its `nearest` rounds ties to even and it has no
//! scalar fused multiply-add, so those are spelled out), and every memory
//! access clamps into its region or buffer exactly like the interpreter:
//! a program cannot touch memory outside what it was given, and the wasm
//! sandbox bounds the module to the host's memory besides.
//!
//! Two forms: scalar ([`scalar`]: one element per iteration, any program
//! but host calls) and four-wide ([`simd`]: SIMD128, the
//! NEON ×4 backend's masked SPMD over element-local kernels; see
//! [`crate::spmd`]).

use crate::ir::{self, Bin, Block, Cmp, Fma, Op, Program, Region, Stmt, Ty, Un, Val};

mod simd;

/// What the host's engine offers: how the module imports its memory, and
/// whether its relaxed multiply-add is the fused one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Target {
    /// A shared memory (a threaded host): the import must then name the
    /// memory's maximum.
    pub shared: bool,
    /// Maximum pages (required when shared).
    pub max_pages: Option<u32>,
    /// `f32x4.relaxed_madd` rounds once on this engine and machine (the
    /// host ran [`fma_probe`] and it answered fused): AIR's fused
    /// multiply-add is then that one instruction. Otherwise it is computed
    /// exactly in f64 (about twice the cost of a kernel's other math).
    pub relaxed_fma: bool,
}

/// One program of a module, and the form it is compiled in (a host builds
/// one entry per kernel; a test may build both forms of a program).
pub struct Entry<'a> {
    pub program: &'a Program,
    /// Four elements per iteration (only element-local kernels whose
    /// outputs hold every record; [`simd_supported`] says whether the
    /// program has the shape).
    pub simd: bool,
}

/// A module exporting `run0..` for `entries`; None when a program uses
/// what the backend does not compile (host calls, or the four wide form
/// for a program without the element loop's shape: audio programs are
/// scalar).
pub fn module(entries: &[Entry], target: Target) -> Option<Vec<u8>> {
    // Function indices: the entries; the four-wide exact multiply-add's
    // rare fix-up (when some four-wide code has a fused multiply-add);
    // then each entry's functions, in the entry's form.
    let n = entries.len() as u32;
    let uses_fix = !target.relaxed_fma && entries.iter().any(|e| e.simd && (has_float_fma(&e.program.body) || e.program.funcs.iter().any(|g| has_float_fma(&g.body))));
    let fix = n;
    let mut next = n + uses_fix as u32;
    let calls: Vec<Vec<u32>> = entries
        .iter()
        .map(|e| {
            e.program
                .funcs
                .iter()
                .map(|_| {
                    next += 1;
                    next - 1
                })
                .collect()
        })
        .collect();
    let mut types: Vec<(Vec<u8>, Vec<u8>)> = vec![(vec![op::I32; N_PARAMS as usize], vec![])];
    let mut ty = |params: Vec<u8>, results: Vec<u8>| -> u32 {
        match types.iter().position(|t| t.0 == params && t.1 == results) {
            Some(k) => k as u32,
            None => {
                types.push((params, results));
                types.len() as u32 - 1
            }
        }
    };
    let mut funcs = Vec::new();
    for (k, e) in entries.iter().enumerate() {
        funcs.push((if e.simd { simd::body(e.program, target, fix, &calls[k])? } else { scalar(e.program, target, &calls[k])? }, 0));
    }
    if uses_fix {
        funcs.push((simd::fma_fix(), ty(vec![op::I32], vec![])));
    }
    for (k, e) in entries.iter().enumerate() {
        for g in &e.program.funcs {
            let base = vec![op::I32; N_PARAMS as usize];
            if e.simd {
                let params = base.into_iter().chain(g.params.iter().map(|_| op::V128)).chain([op::V128]).collect();
                let t = ty(params, g.results.iter().map(|_| op::V128).collect());
                funcs.push((simd::function(g, target, fix, &calls[k])?, t));
            } else {
                let params = base.into_iter().chain(g.params.iter().map(|v| wty(g.vals[v.0 as usize]))).collect();
                let t = ty(params, g.results.iter().map(|v| wty(g.vals[v.0 as usize])).collect());
                funcs.push((scalar_fn(g, target, &calls[k])?, t));
            }
        }
    }
    let exports: Vec<(String, u32)> = (0..n).map(|k| (format!("run{}", k), k)).collect();
    let types: Vec<(&[u8], &[u8])> = types.iter().map(|(a, b)| (&a[..], &b[..])).collect();
    Some(assemble(&funcs, &types, &exports, target))
}

/// Any f32 fused multiply-add in `b`.
fn has_float_fma(b: &Block) -> bool {
    b.iter().any(|s| match s {
        Stmt::Def(_, Op::Fma(k, ..)) => *k != Fma::MulAddI,
        Stmt::If(_, t, e) => has_float_fma(t) || has_float_fma(e),
        Stmt::Loop { body, .. } => has_float_fma(body),
        _ => false,
    })
}

/// Words of host scratch an entry of `p` needs as its `frame` (the four
/// wide form's lanes and its multiply-add head included): the size a host
/// allocates, 16-byte aligned.
pub fn frame_words(p: &Program, simd: bool) -> usize {
    if simd {
        p.frame_words as usize * 4 + 16
    } else {
        p.frame_words as usize
    }
}

/// Whether [`module`] can compile `p` four wide.
pub fn simd_supported(p: &Program) -> bool {
    simd::supported(p)
}

/// A module whose export `fma(a, b, c) -> f32` is `f32x4.relaxed_madd` on
/// lane 0. An engine without relaxed SIMD refuses to compile it; one whose
/// madd is unfused answers `PROBE` with 0 instead of `PROBE_FUSED`. A host
/// sets [`Target::relaxed_fma`] only when it compiled and answered fused.
/// It imports the memory as `target`'s modules do (a threaded host links
/// every module against its shared memory, which an unshared import
/// refuses); `target.relaxed_fma` is not read.
pub fn fma_probe(target: Target) -> Vec<u8> {
    let mut f = Body::with_params(3);
    for k in 0..3 {
        f.get(k);
        f.fd(op::F32X4_SPLAT);
    }
    f.fd(op::F32X4_RELAXED_MADD);
    f.fd(op::F32X4_EXTRACT_LANE);
    f.b(0);
    assemble(&[(f, 0)], &[(&[op::F32; 3], &[op::F32])], &[("fma".into(), 0)], target)
}

/// The probe's arguments: (1 + 2^-23)^2 - (1 + 2^-22) is 2^-46 exactly,
/// and 0 when the product is rounded first.
pub const PROBE: [f32; 3] = [1.000_000_1, 1.000_000_1, -1.000_000_2];
pub const PROBE_FUSED: f32 = 1.421_085_5e-14;

// =========================================================================
// Encoding
// =========================================================================

pub(crate) mod op {
    pub const BLOCK: u8 = 0x02;
    pub const LOOP: u8 = 0x03;
    pub const IF: u8 = 0x04;
    pub const ELSE: u8 = 0x05;
    pub const END: u8 = 0x0B;
    pub const BR: u8 = 0x0C;
    pub const BR_IF: u8 = 0x0D;
    pub const RETURN: u8 = 0x0F;
    pub const CALL: u8 = 0x10;
    pub const SELECT: u8 = 0x1B;
    pub const LOCAL_GET: u8 = 0x20;
    pub const LOCAL_SET: u8 = 0x21;
    pub const LOCAL_TEE: u8 = 0x22;
    pub const I32_LOAD: u8 = 0x28;
    pub const F32_LOAD: u8 = 0x2A;
    pub const I32_STORE: u8 = 0x36;
    pub const F32_STORE: u8 = 0x38;
    pub const I32_CONST: u8 = 0x41;
    pub const I64_CONST: u8 = 0x42;
    pub const F32_CONST: u8 = 0x43;
    pub const F64_CONST: u8 = 0x44;
    pub const I32_EQZ: u8 = 0x45;
    pub const I32_EQ: u8 = 0x46;
    pub const I32_NE: u8 = 0x47;
    pub const I32_LT_S: u8 = 0x48;
    pub const I32_LT_U: u8 = 0x49;
    pub const I32_GT_S: u8 = 0x4A;
    pub const I32_GT_U: u8 = 0x4B;
    pub const I32_LE_S: u8 = 0x4C;
    pub const I32_LE_U: u8 = 0x4D;
    pub const I32_GE_S: u8 = 0x4E;
    pub const I32_GE_U: u8 = 0x4F;
    pub const I64_EQZ: u8 = 0x50;
    pub const I64_LE_U: u8 = 0x58;
    pub const F32_EQ: u8 = 0x5B;
    pub const F32_NE: u8 = 0x5C;
    pub const F32_LT: u8 = 0x5D;
    pub const F32_GT: u8 = 0x5E;
    pub const F32_LE: u8 = 0x5F;
    pub const F32_GE: u8 = 0x60;
    pub const F64_EQ: u8 = 0x61;
    pub const F64_NE: u8 = 0x62;
    pub const F64_LT: u8 = 0x63;
    pub const F64_GT: u8 = 0x64;
    pub const F64_LE: u8 = 0x65;
    pub const F64_GE: u8 = 0x66;
    pub const I32_ADD: u8 = 0x6A;
    pub const I32_SUB: u8 = 0x6B;
    pub const I32_MUL: u8 = 0x6C;
    pub const I32_DIV_S: u8 = 0x6D;
    pub const I32_REM_S: u8 = 0x6F;
    pub const I32_AND: u8 = 0x71;
    pub const I32_OR: u8 = 0x72;
    pub const I32_XOR: u8 = 0x73;
    pub const I32_SHL: u8 = 0x74;
    pub const I32_SHR_S: u8 = 0x75;
    pub const I32_SHR_U: u8 = 0x76;
    pub const I64_AND: u8 = 0x83;
    pub const I64_OR: u8 = 0x84;
    pub const I64_XOR: u8 = 0x85;
    pub const I64_SHL: u8 = 0x86;
    pub const I64_SHR_S: u8 = 0x87;
    pub const I64_SHR_U: u8 = 0x88;
    pub const I64_ADD: u8 = 0x7C;
    pub const F32_ABS: u8 = 0x8B;
    pub const F32_NEG: u8 = 0x8C;
    pub const F32_CEIL: u8 = 0x8D;
    pub const F32_FLOOR: u8 = 0x8E;
    pub const F32_TRUNC: u8 = 0x8F;
    pub const F32_SQRT: u8 = 0x91;
    pub const F32_ADD: u8 = 0x92;
    pub const F32_SUB: u8 = 0x93;
    pub const F32_MUL: u8 = 0x94;
    pub const F32_DIV: u8 = 0x95;
    pub const F32_COPYSIGN: u8 = 0x98;
    pub const F64_ABS: u8 = 0x99;
    pub const F64_NEG: u8 = 0x9A;
    pub const F64_CEIL: u8 = 0x9B;
    pub const F64_FLOOR: u8 = 0x9C;
    pub const F64_TRUNC: u8 = 0x9D;
    pub const F64_SQRT: u8 = 0x9F;
    pub const F64_ADD: u8 = 0xA0;
    pub const F64_SUB: u8 = 0xA1;
    pub const F64_MUL: u8 = 0xA2;
    pub const F64_DIV: u8 = 0xA3;
    pub const F64_COPYSIGN: u8 = 0xA6;
    pub const I32_WRAP_I64: u8 = 0xA7;
    pub const F32_CONVERT_I32_S: u8 = 0xB2;
    pub const F32_DEMOTE_F64: u8 = 0xB6;
    pub const F64_CONVERT_I32_S: u8 = 0xB7;
    pub const F64_PROMOTE_F32: u8 = 0xBB;
    pub const I64_EXTEND_I32_U: u8 = 0xAD;
    pub const I32_REINTERPRET_F32: u8 = 0xBC;
    pub const I64_REINTERPRET_F64: u8 = 0xBD;
    pub const F32_REINTERPRET_I32: u8 = 0xBE;
    pub const F64_REINTERPRET_I64: u8 = 0xBF;
    pub const PREFIX_FC: u8 = 0xFC;
    pub const PREFIX_FD: u8 = 0xFD;
    /// 0xFC sub-opcodes (saturating truncation: Rust `as`).
    pub const I32_TRUNC_SAT_F32_S: u32 = 0;
    pub const I32_TRUNC_SAT_F64_S: u32 = 2;
    pub const VOID: u8 = 0x40;
    pub const I32: u8 = 0x7F;
    pub const I64: u8 = 0x7E;
    pub const F32: u8 = 0x7D;
    pub const F64: u8 = 0x7C;
    pub const V128: u8 = 0x7B;
    /// 0xFD sub-opcodes.
    pub const V128_LOAD: u32 = 0x00;
    pub const V128_STORE: u32 = 0x0B;
    pub const V128_CONST: u32 = 0x0C;
    pub const I8X16_SHUFFLE: u32 = 0x0D;
    pub const I8X16_SWIZZLE: u32 = 0x0E;
    pub const I32X4_SPLAT: u32 = 0x11;
    pub const F32X4_SPLAT: u32 = 0x13;
    pub const I32X4_EXTRACT_LANE: u32 = 0x1B;
    pub const I32X4_REPLACE_LANE: u32 = 0x1C;
    pub const F32X4_EXTRACT_LANE: u32 = 0x1F;
    pub const I32X4_EQ: u32 = 0x37;
    pub const I32X4_NE: u32 = 0x38;
    pub const I32X4_LT_S: u32 = 0x39;
    pub const I32X4_LT_U: u32 = 0x3A;
    pub const I32X4_GT_S: u32 = 0x3B;
    pub const I32X4_LE_S: u32 = 0x3D;
    pub const I32X4_GE_S: u32 = 0x3F;
    pub const F32X4_EQ: u32 = 0x41;
    pub const F32X4_NE: u32 = 0x42;
    pub const F32X4_LT: u32 = 0x43;
    pub const F32X4_GT: u32 = 0x44;
    pub const F32X4_LE: u32 = 0x45;
    pub const F32X4_GE: u32 = 0x46;
    pub const V128_NOT: u32 = 0x4D;
    pub const V128_AND: u32 = 0x4E;
    pub const V128_ANDNOT: u32 = 0x4F;
    pub const V128_OR: u32 = 0x50;
    pub const V128_XOR: u32 = 0x51;
    pub const V128_BITSELECT: u32 = 0x52;
    pub const V128_ANY_TRUE: u32 = 0x53;
    pub const V128_LOAD32_LANE: u32 = 0x56;
    pub const V128_STORE32_LANE: u32 = 0x5A;
    pub const F32X4_DEMOTE_F64X2_ZERO: u32 = 0x5E;
    pub const F64X2_PROMOTE_LOW_F32X4: u32 = 0x5F;
    pub const I8X16_SUB: u32 = 0x71;
    pub const F32X4_CEIL: u32 = 0x67;
    pub const F32X4_FLOOR: u32 = 0x68;
    pub const F32X4_TRUNC: u32 = 0x69;
    pub const I32X4_NEG: u32 = 0xA1;
    pub const I32X4_ALL_TRUE: u32 = 0xA3;
    pub const I32X4_SHL: u32 = 0xAB;
    pub const I32X4_SHR_S: u32 = 0xAC;
    pub const I32X4_SHR_U: u32 = 0xAD;
    pub const I32X4_ADD: u32 = 0xAE;
    pub const I32X4_SUB: u32 = 0xB1;
    pub const I32X4_MUL: u32 = 0xB5;
    pub const I32X4_MIN_U: u32 = 0xB7;
    pub const I64X2_SHR_U: u32 = 0xCD;
    pub const I64X2_EXTMUL_LOW_I32X4_S: u32 = 0xDC;
    pub const I64X2_EXTMUL_HIGH_I32X4_S: u32 = 0xDD;
    pub const I64X2_EXTMUL_LOW_I32X4_U: u32 = 0xDE;
    pub const I64X2_EXTMUL_HIGH_I32X4_U: u32 = 0xDF;
    pub const F32X4_ABS: u32 = 0xE0;
    pub const F32X4_NEG: u32 = 0xE1;
    pub const F32X4_SQRT: u32 = 0xE3;
    pub const F32X4_ADD: u32 = 0xE4;
    pub const F32X4_SUB: u32 = 0xE5;
    pub const F32X4_MUL: u32 = 0xE6;
    pub const F32X4_DIV: u32 = 0xE7;
    pub const F64X2_ADD: u32 = 0xF0;
    pub const F64X2_MUL: u32 = 0xF2;
    pub const I32X4_TRUNC_SAT_F32X4_S: u32 = 0xF8;
    pub const I32X4_TRUNC_SAT_F32X4_U: u32 = 0xF9;
    pub const F32X4_CONVERT_I32X4_S: u32 = 0xFA;
    pub const F32X4_RELAXED_MADD: u32 = 0x105;
    pub const F32X4_RELAXED_NMADD: u32 = 0x106;
}

pub(crate) fn uleb(out: &mut Vec<u8>, mut x: u64) {
    loop {
        let b = (x & 0x7F) as u8;
        x >>= 7;
        if x == 0 {
            out.push(b);
            return;
        }
        out.push(b | 0x80);
    }
}

pub(crate) fn sleb(out: &mut Vec<u8>, mut x: i64) {
    loop {
        let b = (x & 0x7F) as u8;
        x >>= 7;
        if (x == 0 && b & 0x40 == 0) || (x == -1 && b & 0x40 != 0) {
            out.push(b);
            return;
        }
        out.push(b | 0x80);
    }
}

/// A function body under construction: its locals by type and its code.
pub(crate) struct Body {
    /// Local types after the parameters.
    pub locals: Vec<u8>,
    pub code: Vec<u8>,
    /// Parameters (the entries' six, by default).
    pub params: u32,
}

/// The six i32 parameters every entry takes.
pub(crate) const P_CTX: u32 = 0;
pub(crate) const P_STATE: u32 = 1;
pub(crate) const P_SHARED: u32 = 2;
pub(crate) const P_TABLE: u32 = 3;
pub(crate) const P_N: u32 = 4;
pub(crate) const P_FRAME: u32 = 5;
pub(crate) const N_PARAMS: u32 = 6;

impl Body {
    pub fn new() -> Body {
        Body::with_params(N_PARAMS)
    }

    pub fn with_params(params: u32) -> Body {
        Body { locals: Vec::new(), code: Vec::new(), params }
    }

    pub fn local(&mut self, ty: u8) -> u32 {
        self.locals.push(ty);
        self.params + self.locals.len() as u32 - 1
    }

    pub fn b(&mut self, x: u8) {
        self.code.push(x);
    }

    pub fn u(&mut self, x: u32) {
        uleb(&mut self.code, x as u64);
    }

    pub fn get(&mut self, l: u32) {
        self.b(op::LOCAL_GET);
        self.u(l);
    }

    pub fn set(&mut self, l: u32) {
        self.b(op::LOCAL_SET);
        self.u(l);
    }

    pub fn tee(&mut self, l: u32) {
        self.b(op::LOCAL_TEE);
        self.u(l);
    }

    pub fn i32c(&mut self, x: i32) {
        self.b(op::I32_CONST);
        sleb(&mut self.code, x as i64);
    }

    pub fn i64c(&mut self, x: i64) {
        self.b(op::I64_CONST);
        sleb(&mut self.code, x);
    }

    pub fn f32c(&mut self, x: f32) {
        self.b(op::F32_CONST);
        self.code.extend_from_slice(&x.to_bits().to_le_bytes());
    }

    pub fn f64c(&mut self, x: f64) {
        self.b(op::F64_CONST);
        self.code.extend_from_slice(&x.to_bits().to_le_bytes());
    }

    /// A load or store with a static byte offset (alignment 4).
    pub fn mem(&mut self, opc: u8, offset: u32) {
        self.b(opc);
        self.u(2);
        self.u(offset);
    }

    pub fn fc(&mut self, sub: u32) {
        self.b(op::PREFIX_FC);
        self.u(sub);
    }

    pub fn fd(&mut self, sub: u32) {
        self.b(op::PREFIX_FD);
        self.u(sub);
    }

    /// `a b` on the stack -> min_u(a, b) (two temps from the caller).
    pub fn min_u(&mut self, ta: u32, tb: u32) {
        self.set(tb);
        self.tee(ta);
        self.get(tb);
        self.get(ta);
        self.get(tb);
        self.b(op::I32_LT_U);
        self.b(op::SELECT);
    }
}

fn wty(t: Ty) -> u8 {
    match t {
        Ty::F32 => op::F32,
        Ty::I32 | Ty::Bool => op::I32,
        Ty::F64 => op::F64,
    }
}

/// Type section, import, function and export sections, code: functions
/// (body, type index), types (params, results), exports (name, function).
fn assemble(funcs: &[(Body, u32)], types: &[(&[u8], &[u8])], exports: &[(String, u32)], mem: Target) -> Vec<u8> {
    fn section(out: &mut Vec<u8>, id: u8, body: &[u8]) {
        out.push(id);
        uleb(out, body.len() as u64);
        out.extend_from_slice(body);
    }
    let mut out = vec![0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00];
    let mut s = Vec::new();
    uleb(&mut s, types.len() as u64);
    for (params, results) in types {
        s.push(0x60);
        uleb(&mut s, params.len() as u64);
        s.extend_from_slice(params);
        uleb(&mut s, results.len() as u64);
        s.extend_from_slice(results);
    }
    section(&mut out, 1, &s);
    // Import env.memory.
    let mut s = vec![1];
    for name in ["env", "memory"] {
        uleb(&mut s, name.len() as u64);
        s.extend_from_slice(name.as_bytes());
    }
    s.push(0x02);
    match (mem.shared, mem.max_pages) {
        (true, max) => {
            s.push(0x03);
            uleb(&mut s, 1);
            uleb(&mut s, max.unwrap_or(65536) as u64);
        }
        (false, Some(max)) => {
            s.push(0x01);
            uleb(&mut s, 1);
            uleb(&mut s, max as u64);
        }
        (false, None) => {
            s.push(0x00);
            uleb(&mut s, 1);
        }
    }
    section(&mut out, 2, &s);
    // Functions.
    let mut s = Vec::new();
    uleb(&mut s, funcs.len() as u64);
    for (_, t) in funcs {
        uleb(&mut s, *t as u64);
    }
    section(&mut out, 3, &s);
    let mut s = Vec::new();
    uleb(&mut s, exports.len() as u64);
    for (name, k) in exports {
        uleb(&mut s, name.len() as u64);
        s.extend_from_slice(name.as_bytes());
        s.push(0x00);
        uleb(&mut s, *k as u64);
    }
    section(&mut out, 7, &s);
    // Code.
    let mut s = Vec::new();
    uleb(&mut s, funcs.len() as u64);
    for (b, _) in funcs {
        let mut f = Vec::new();
        // Locals, run-length encoded by type.
        let mut runs: Vec<(u32, u8)> = Vec::new();
        for t in &b.locals {
            match runs.last_mut() {
                Some((n, ty)) if *ty == *t => *n += 1,
                _ => runs.push((1, *t)),
            }
        }
        uleb(&mut f, runs.len() as u64);
        for (n, t) in runs {
            uleb(&mut f, n as u64);
            f.push(t);
        }
        f.extend_from_slice(&b.code);
        f.push(op::END);
        uleb(&mut s, f.len() as u64);
        s.extend_from_slice(&f);
    }
    section(&mut out, 10, &s);
    out
}

// =========================================================================
// Scalar code
// =========================================================================

/// What a label on the wasm control stack is.
#[derive(Clone, Copy, PartialEq)]
enum Label {
    /// An `if`'s implicit block.
    If,
    /// A loop's exit block (break target).
    Exit,
    /// A loop's `loop` (back-edge target).
    Top,
    /// The block around a loop's body (continue target).
    IterEnd,
}

struct Sc<'a> {
    p: &'a Program,
    f: Body,
    /// wasm local of each Val, then each Var.
    val: Vec<u32>,
    var: Vec<u32>,
    /// Per host buffer used: (byte address local, last word index local).
    buf: Vec<Option<(u32, u32)>>,
    /// Audio I/O: per channel used (in 0, in 1, out 0, out 1) the local
    /// holding its byte address, loaded from `table` once.
    io: [u32; 4],
    labels: Vec<Label>,
    bounds: Vec<Option<u32>>,
    consts: Vec<Option<i32>>,
    /// Scratch locals.
    ti: [u32; 3],
    tf64: [u32; 4],
    ti64: [u32; 2],
    tf32: u32,
    relaxed: bool,
    /// The wasm function of each of the program's functions.
    calls: Vec<u32>,
}

/// The scalar body of `p` (None: host calls).
pub(crate) fn scalar(p: &Program, target: Target, calls: &[u32]) -> Option<Body> {
    if !scalar_ok(p) {
        return None;
    }
    let mut sc = Sc::new(p, target, Body::new(), &|_| true, &|_| true);
    sc.calls = calls.to_vec();
    if uses_io(p) {
        // An audio call of no frames does nothing (the interpreter returns
        // before running the program; `n - 1` clamps every I/O index).
        sc.f.get(P_N);
        sc.f.b(op::I32_EQZ);
        sc.f.b(op::IF);
        sc.f.b(op::VOID);
        sc.f.b(op::RETURN);
        sc.f.b(op::END);
    }
    sc.block(&p.body);
    Some(sc.f)
}

/// Host calls are not compiled (anywhere in `p`), nor audio I/O in a
/// program that also uses host buffers (both would take the `table`
/// parameter; validation keeps them apart: audio programs have no buffers).
fn scalar_ok(p: &Program) -> bool {
    fn ok(b: &Block) -> bool {
        b.iter().all(|s| match s {
            Stmt::CallHost { .. } => false,
            Stmt::If(_, t, e) => ok(t) && ok(e),
            Stmt::Loop { body, .. } => ok(body),
            _ => true,
        })
    }
    let all = || std::iter::once(p).chain(p.funcs.iter());
    let bufs = all().any(|g| {
        let mut used = Vec::new();
        used_bufs(&g.body, &mut used);
        !used.is_empty()
    });
    all().all(|g| ok(&g.body)) && !(bufs && uses_io(p))
}

/// The audio I/O channels `b` reads (`In`, bits 0-1) and adds to (`Out`,
/// bits 2-3).
fn io_channels(b: &Block) -> u8 {
    b.iter()
        .map(|s| match s {
            Stmt::Def(_, Op::In { ch, .. }) => 1 << (*ch & 1),
            Stmt::Out { ch, .. } => 4 << (*ch & 1),
            Stmt::If(_, t, e) => io_channels(t) | io_channels(e),
            Stmt::Loop { body, .. } => io_channels(body),
            _ => 0,
        })
        .fold(0, |a, b| a | b)
}

/// Any audio I/O in `p` or its functions.
fn uses_io(p: &Program) -> bool {
    io_channels(&p.body) != 0 || p.funcs.iter().any(|g| io_channels(&g.body) != 0)
}

/// The scalar code of function `g`: the six entry parameters, then its
/// params; its results returned.
pub(crate) fn scalar_fn(g: &Program, target: Target, calls: &[u32]) -> Option<Body> {
    if !scalar_ok(g) {
        return None;
    }
    let mut sc = Sc::new(g, target, Body::with_params(N_PARAMS + g.params.len() as u32), &|_| true, &|_| true);
    sc.calls = calls.to_vec();
    for (k, pv) in g.params.iter().enumerate() {
        sc.val[pv.0 as usize] = N_PARAMS + k as u32;
    }
    sc.block(&g.body);
    for r in &g.results {
        sc.v(*r);
    }
    Some(sc.f)
}

/// Value lifetimes in statement order (pre-order positions, the order the
/// emitters walk): per value its definition's position and its last use,
/// a use inside a loop that does not contain the definition extending to
/// that loop's end. Values whose lifetimes do not overlap share a local
/// ([`assign_locals`]): a kernel with 100k values then has as many
/// locals as are live at once, and each local index is one or two bytes.
pub(crate) struct Lifetimes {
    /// Per position, the values defined there.
    defs: Vec<Vec<u32>>,
    /// Per position, the values last used there.
    frees: Vec<Vec<u32>>,
}

pub(crate) fn lifetimes(p: &Program) -> Lifetimes {
    // Pass 1: positions and each loop's last position.
    fn ends(b: &Block, pos: &mut u32, out: &mut Vec<u32>) {
        for s in b {
            *pos += 1;
            match s {
                Stmt::If(_, t, e) => {
                    ends(t, pos, out);
                    ends(e, pos, out);
                }
                Stmt::Loop { body, .. } => {
                    let k = out.len();
                    out.push(0);
                    ends(body, pos, out);
                    out[k] = *pos;
                }
                _ => {}
            }
        }
    }
    let mut npos = 0;
    let mut loop_end = Vec::new();
    ends(&p.body, &mut npos, &mut loop_end);
    // The values a statement reads (a shift by `a + c` also reads a: the
    // four-wide code takes it from there).
    let mut add_of: Vec<Option<(Val, Val)>> = vec![None; p.vals.len()];
    fn adds(b: &Block, out: &mut Vec<Option<(Val, Val)>>) {
        for s in b {
            match s {
                Stmt::Def(v, Op::Bin(Bin::AddI, x, y)) => out[v.0 as usize] = Some((*x, *y)),
                Stmt::If(_, t, e) => {
                    adds(t, out);
                    adds(e, out);
                }
                Stmt::Loop { body, .. } => adds(body, out),
                _ => {}
            }
        }
    }
    adds(&p.body, &mut add_of);
    struct St<'a> {
        pos: u32,
        loops: Vec<usize>,
        next_loop: usize,
        loop_end: &'a [u32],
        def: Vec<(u32, u32)>,
        last: Vec<u32>,
        add_of: &'a [Option<(Val, Val)>],
    }
    impl St<'_> {
        fn used(&mut self, v: Val) {
            let k = v.0 as usize;
            let (_, depth) = self.def[k];
            let at = if (depth as usize) < self.loops.len() { self.loop_end[self.loops[depth as usize]] } else { self.pos };
            self.last[k] = self.last[k].max(at);
        }
        fn defined(&mut self, v: Val) {
            let k = v.0 as usize;
            self.def[k] = (self.pos, self.loops.len() as u32);
            self.last[k] = self.last[k].max(self.pos);
        }
        fn walk(&mut self, b: &Block) {
            for s in b {
                self.pos += 1;
                match s {
                    Stmt::Def(v, op) => {
                        for u in ir::op_uses(op) {
                            self.used(u);
                        }
                        if let Op::Bin(Bin::ShrUI | Bin::ShrI | Bin::ShlI, _, y) = op {
                            if let Some((a, c)) = self.add_of[y.0 as usize] {
                                self.used(a);
                                self.used(c);
                            }
                        }
                        self.defined(*v);
                    }
                    Stmt::Set(_, x) => self.used(*x),
                    Stmt::Store { off, val, .. } => {
                        if let Some(o) = off {
                            self.used(*o);
                        }
                        self.used(*val);
                    }
                    Stmt::Out { idx, val, .. } => {
                        self.used(*idx);
                        self.used(*val);
                    }
                    Stmt::If(c, t, e) => {
                        self.used(*c);
                        self.walk(t);
                        self.walk(e);
                    }
                    Stmt::Loop { body, .. } => {
                        self.loops.push(self.next_loop);
                        self.next_loop += 1;
                        self.walk(body);
                        self.loops.pop();
                    }
                    Stmt::CallHost { args, slices, rets, .. } => {
                        for a in args.iter().chain(slices.iter().flat_map(|x| [&x.off, &x.len])) {
                            self.used(*a);
                        }
                        for r in rets {
                            self.defined(*r);
                        }
                    }
                    Stmt::Call { args, rets, .. } => {
                        for a in args {
                            self.used(*a);
                        }
                        for r in rets {
                            self.defined(*r);
                        }
                    }
                    Stmt::Break(_) | Stmt::Continue(_) => {}
                }
            }
        }
    }
    let mut st = St { pos: 0, loops: Vec::new(), next_loop: 0, loop_end: &loop_end, def: vec![(u32::MAX, 0); p.vals.len()], last: vec![0; p.vals.len()], add_of: &add_of };
    st.walk(&p.body);
    // A function's results are read at its end.
    st.pos += 1;
    for r in &p.results {
        st.used(*r);
    }
    let npos = npos.max(st.pos);
    let mut lt = Lifetimes { defs: vec![Vec::new(); npos as usize + 1], frees: vec![Vec::new(); npos as usize + 1] };
    for (k, (d, _)) in st.def.iter().enumerate() {
        if *d != u32::MAX {
            lt.defs[*d as usize].push(k as u32);
            lt.frees[st.last[k] as usize].push(k as u32);
        }
    }
    lt
}

/// A local of type `ty(value)` (None: no local) per value, shared by values
/// whose lifetimes do not overlap (NONE for the others).
pub(crate) fn assign_locals(lt: &Lifetimes, nvals: usize, f: &mut Body, ty: &dyn Fn(usize) -> Option<u8>) -> Vec<u32> {
    let mut out = vec![u32::MAX; nvals];
    let mut free: Vec<(u8, Vec<u32>)> = Vec::new();
    for (defs, frees) in lt.defs.iter().zip(&lt.frees) {
        for v in defs {
            if let Some(t) = ty(*v as usize) {
                let pool = match free.iter_mut().find(|(x, _)| *x == t) {
                    Some((_, p)) => p,
                    None => {
                        free.push((t, Vec::new()));
                        &mut free.last_mut().unwrap().1
                    }
                };
                out[*v as usize] = pool.pop().unwrap_or_else(|| f.local(t));
            }
        }
        for v in frees {
            let l = out[*v as usize];
            if l != u32::MAX {
                let t = ty(*v as usize).unwrap();
                free.iter_mut().find(|(x, _)| *x == t).unwrap().1.push(l);
            }
        }
    }
    out
}

pub(crate) fn collect_consts(b: &Block, out: &mut Vec<Option<i32>>) {
    for s in b {
        match s {
            Stmt::Def(v, Op::ConstI(c)) => out[v.0 as usize] = Some(*c),
            Stmt::If(_, t, e) => {
                collect_consts(t, out);
                collect_consts(e, out);
            }
            Stmt::Loop { body, .. } => collect_consts(body, out),
            _ => {}
        }
    }
}

pub(crate) fn used_bufs(b: &Block, out: &mut Vec<u8>) {
    for s in b {
        let k = match s {
            Stmt::Def(_, Op::Load { region: Region::Buf(k), .. }) | Stmt::Def(_, Op::BufLen(k)) | Stmt::Store { region: Region::Buf(k), .. } => Some(*k),
            Stmt::If(_, t, e) => {
                used_bufs(t, out);
                used_bufs(e, out);
                None
            }
            Stmt::Loop { body, .. } => {
                used_bufs(body, out);
                None
            }
            _ => None,
        };
        if let Some(k) = k {
            if !out.contains(&k) {
                out.push(k);
            }
        }
    }
}

impl<'a> Sc<'a> {
    /// Scalar locals for the values and variables `want_val` / `want_var`
    /// accept (the others get none: the four-wide code holds them in
    /// vectors), scratch locals, and every used host buffer's byte address
    /// and last word index, loaded once at the top of `f`.
    fn new(p: &'a Program, target: Target, mut f: Body, want_val: &dyn Fn(usize) -> bool, want_var: &dyn Fn(usize) -> bool) -> Sc<'a> {
        let val = assign_locals(&lifetimes(p), p.vals.len(), &mut f, &|k| want_val(k).then(|| wty(p.vals[k])));
        let var: Vec<u32> = p.vars.iter().enumerate().map(|(k, t)| if want_var(k) { f.local(wty(*t)) } else { u32::MAX }).collect();
        let ti = [f.local(op::I32), f.local(op::I32), f.local(op::I32)];
        let tf64 = [f.local(op::F64), f.local(op::F64), f.local(op::F64), f.local(op::F64)];
        let ti64 = [f.local(op::I64), f.local(op::I64)];
        let tf32 = f.local(op::F32);
        let mut consts = vec![None; p.vals.len()];
        collect_consts(&p.body, &mut consts);
        let mut sc = Sc { p, f, val, var, buf: Vec::new(), io: [u32::MAX; 4], labels: Vec::new(), bounds: ir::bounds(p), consts, ti, tf64, ti64, tf32, relaxed: target.relaxed_fma, calls: Vec::new() };
        let chans = io_channels(&p.body);
        for k in 0..4 {
            if chans & (1 << k) != 0 {
                let l = sc.f.local(op::I32);
                sc.f.get(P_TABLE);
                sc.f.mem(op::I32_LOAD, 4 * k);
                sc.f.set(l);
                sc.io[k as usize] = l;
            }
        }
        let mut used = Vec::new();
        used_bufs(&p.body, &mut used);
        sc.buf = vec![None; used.iter().copied().max().map_or(0, |m| m as usize + 1)];
        for k in used {
            let (ptr, last) = (sc.f.local(op::I32), sc.f.local(op::I32));
            sc.f.get(P_TABLE);
            sc.f.mem(op::I32_LOAD, 8 * k as u32);
            sc.f.set(ptr);
            sc.f.get(P_TABLE);
            sc.f.mem(op::I32_LOAD, 8 * k as u32 + 4);
            sc.f.i32c(1);
            sc.f.b(op::I32_SUB);
            sc.f.set(last);
            sc.buf[k as usize] = Some((ptr, last));
        }
        sc
    }

    fn v(&mut self, v: Val) {
        let l = self.val[v.0 as usize];
        self.f.get(l);
    }

    fn ty(&self, v: Val) -> Ty {
        self.p.vals[v.0 as usize]
    }

    fn depth_of(&self, label: Label, d: u32) -> u32 {
        // The d-th enclosing loop's label of this kind.
        let mut loops = 0;
        for (k, l) in self.labels.iter().enumerate().rev() {
            if *l == Label::Exit {
                if loops == d {
                    let target = match label {
                        Label::Exit => k,
                        _ => k + 2,
                    };
                    return (self.labels.len() - 1 - target) as u32;
                }
                loops += 1;
            }
        }
        unreachable!("validated loop depth")
    }

    fn block(&mut self, b: &Block) {
        for s in b {
            self.stmt(s);
        }
    }

    fn stmt(&mut self, s: &Stmt) {
        match s {
            Stmt::Def(v, op) => {
                self.op(*v, op);
                let l = self.val[v.0 as usize];
                self.f.set(l);
            }
            Stmt::Set(var, v) => {
                self.v(*v);
                let l = self.var[var.0 as usize];
                self.f.set(l);
            }
            Stmt::Store { region, base, extent, off, val } => {
                self.address(*region, *base, *extent, *off);
                self.v(*val);
                let opc = if self.ty(*val) == Ty::F32 { op::F32_STORE } else { op::I32_STORE };
                self.f.mem(opc, 0);
            }
            Stmt::If(c, t, e) => {
                self.v(*c);
                self.f.b(op::IF);
                self.f.b(op::VOID);
                self.labels.push(Label::If);
                self.block(t);
                if !e.is_empty() {
                    self.f.b(op::ELSE);
                    self.block(e);
                }
                self.labels.pop();
                self.f.b(op::END);
            }
            Stmt::Loop { cap, body } => {
                let cnt = self.f.local(op::I32);
                self.f.i32c(0);
                self.f.set(cnt);
                self.f.b(op::BLOCK);
                self.f.b(op::VOID);
                self.labels.push(Label::Exit);
                self.f.b(op::LOOP);
                self.f.b(op::VOID);
                self.labels.push(Label::Top);
                self.f.get(cnt);
                self.f.i32c(*cap as i32);
                self.f.b(op::I32_GE_U);
                self.f.b(op::BR_IF);
                self.f.u(1);
                self.f.get(cnt);
                self.f.i32c(1);
                self.f.b(op::I32_ADD);
                self.f.set(cnt);
                self.f.b(op::BLOCK);
                self.f.b(op::VOID);
                self.labels.push(Label::IterEnd);
                self.block(body);
                self.labels.pop();
                self.f.b(op::END);
                self.f.b(op::BR);
                self.f.u(0);
                self.labels.pop();
                self.f.b(op::END);
                self.labels.pop();
                self.f.b(op::END);
            }
            Stmt::Break(d) => {
                let depth = self.depth_of(Label::Exit, *d);
                self.f.b(op::BR);
                self.f.u(depth);
            }
            Stmt::Continue(d) => {
                let depth = self.depth_of(Label::IterEnd, *d);
                self.f.b(op::BR);
                self.f.u(depth);
            }
            Stmt::Call { f, args, rets } => {
                for k in 0..N_PARAMS {
                    self.f.get(k);
                }
                for a in args {
                    self.v(*a);
                }
                self.f.b(op::CALL);
                self.f.u(self.calls[*f as usize]);
                for r in rets.iter().rev() {
                    let l = self.val[r.0 as usize];
                    self.f.set(l);
                }
            }
            Stmt::Out { ch, idx, val } => {
                // out[clamp(idx, 0, n - 1)] += val (f32, as the interpreter).
                let t = self.ti[2];
                self.io_address(2 + (*ch & 1), *idx);
                self.f.tee(t);
                self.f.get(t);
                self.f.mem(op::F32_LOAD, 0);
                self.v(*val);
                self.f.b(op::F32_ADD);
                self.f.mem(op::F32_STORE, 0);
            }
            Stmt::CallHost { .. } => unreachable!("declined"),
        }
    }

    /// Pushes the byte address of frame `clamp(idx, 0, n - 1)` of audio
    /// channel `k` (0, 1 in; 2, 3 out). n >= 1 here (the entry returns on 0).
    fn io_address(&mut self, k: u8, idx: Val) {
        let [t0, t1, _] = self.ti;
        self.v(idx);
        self.f.get(P_N);
        self.f.i32c(1);
        self.f.b(op::I32_SUB);
        self.f.min_u(t0, t1);
        self.f.i32c(2);
        self.f.b(op::I32_SHL);
        self.f.get(self.io[k as usize]);
        self.f.b(op::I32_ADD);
    }

    /// Pushes the byte address of an access (clamped as the interpreter
    /// clamps); the load or store uses offset 0.
    fn address(&mut self, region: Region, base: u32, extent: u32, off: Option<Val>) {
        let proven = off.is_some_and(|o| self.bounds[o.0 as usize].is_some_and(|b| b < extent));
        let off = off.map(|o| self.val[o.0 as usize]);
        self.address_at(region, base, extent, off, proven);
    }

    /// [`Sc::address`] with the offset in local `off` (`proven`: it is
    /// below `extent`, no clamp needed).
    fn address_at(&mut self, region: Region, base: u32, extent: u32, off: Option<u32>, proven: bool) {
        let [t0, t1, _] = self.ti;
        match region {
            Region::Buf(k) => {
                let (ptr, last) = self.buf[k as usize].expect("used buffer");
                // index = min(base + off, last) in 64 bits.
                match off {
                    None => {
                        self.f.i32c(base as i32);
                        self.f.get(last);
                        self.f.min_u(t0, t1);
                    }
                    Some(o) if base == 0 => {
                        self.f.get(o);
                        self.f.get(last);
                        self.f.min_u(t0, t1);
                    }
                    Some(o) => {
                        // base <= last ? min(off, last - base) + base : last
                        self.f.get(o);
                        self.f.get(last);
                        self.f.i32c(base as i32);
                        self.f.b(op::I32_SUB);
                        self.f.min_u(t0, t1);
                        self.f.i32c(base as i32);
                        self.f.b(op::I32_ADD);
                        self.f.get(last);
                        self.f.i32c(base as i32);
                        self.f.get(last);
                        self.f.b(op::I32_LE_U);
                        self.f.b(op::SELECT);
                    }
                }
                self.f.i32c(2);
                self.f.b(op::I32_SHL);
                self.f.get(ptr);
                self.f.b(op::I32_ADD);
            }
            r => {
                let p = match r {
                    Region::Ctx => P_CTX,
                    Region::State => P_STATE,
                    Region::Shared => P_SHARED,
                    Region::Frame => P_FRAME,
                    Region::Buf(_) => unreachable!(),
                };
                match off {
                    None => {
                        self.f.get(p);
                        self.f.i32c((base * 4) as i32);
                        self.f.b(op::I32_ADD);
                    }
                    Some(o) => {
                        self.f.get(o);
                        if !proven {
                            self.f.i32c(extent as i32 - 1);
                            self.f.min_u(t0, t1);
                        }
                        if base > 0 {
                            self.f.i32c(base as i32);
                            self.f.b(op::I32_ADD);
                        }
                        self.f.i32c(2);
                        self.f.b(op::I32_SHL);
                        self.f.get(p);
                        self.f.b(op::I32_ADD);
                    }
                }
            }
        }
    }

    /// Pushes the value of `op` (result type of `v`).
    fn op(&mut self, v: Val, op: &Op) {
        let [t0, t1, t2] = self.ti;
        match *op {
            Op::ConstF(x) => self.f.f32c(x),
            Op::ConstD(x) => self.f.f64c(x),
            Op::ConstI(x) => self.f.i32c(x),
            Op::ConstB(x) => self.f.i32c(x as i32),
            Op::Get(var) => {
                let l = self.var[var.0 as usize];
                self.f.get(l);
            }
            Op::FrameCount => self.f.get(P_N),
            Op::BufLen(k) => {
                let (_, last) = self.buf[k as usize].expect("used buffer");
                self.f.get(last);
                self.f.i32c(1);
                self.f.b(op::I32_ADD);
            }
            Op::Load { region, base, extent, off } => {
                self.address(region, base, extent, off);
                let opc = if self.ty(v) == Ty::F32 { op::F32_LOAD } else { op::I32_LOAD };
                self.f.mem(opc, 0);
            }
            Op::Un(u, a) => self.un(u, a),
            Op::Bin(b, x, y) => self.bin(b, x, y),
            Op::CmpF(cc, x, y) => {
                self.v(x);
                self.v(y);
                self.f.b(match cc {
                    Cmp::Lt => op::F32_LT,
                    Cmp::Le => op::F32_LE,
                    Cmp::Gt => op::F32_GT,
                    Cmp::Ge => op::F32_GE,
                    Cmp::Eq => op::F32_EQ,
                    Cmp::Ne => op::F32_NE,
                });
            }
            Op::CmpD(cc, x, y) => {
                self.v(x);
                self.v(y);
                self.f.b(match cc {
                    Cmp::Lt => op::F64_LT,
                    Cmp::Le => op::F64_LE,
                    Cmp::Gt => op::F64_GT,
                    Cmp::Ge => op::F64_GE,
                    Cmp::Eq => op::F64_EQ,
                    Cmp::Ne => op::F64_NE,
                });
            }
            Op::CmpI(cc, x, y) => {
                self.v(x);
                self.v(y);
                self.f.b(match cc {
                    Cmp::Lt => op::I32_LT_S,
                    Cmp::Le => op::I32_LE_S,
                    Cmp::Gt => op::I32_GT_S,
                    Cmp::Ge => op::I32_GE_S,
                    Cmp::Eq => op::I32_EQ,
                    Cmp::Ne => op::I32_NE,
                });
            }
            Op::Sel(c, x, y) => {
                self.v(x);
                self.v(y);
                self.v(c);
                self.f.b(op::SELECT);
            }
            Op::Wrap(x, len) => {
                self.v(x);
                if len.is_power_of_two() {
                    self.f.i32c((len - 1) as i32);
                    self.f.b(op::I32_AND);
                } else {
                    wrap_rem(&mut self.f, len, t0);
                }
            }
            Op::Fma(Fma::MulAddI, a, b, c) => {
                self.v(a);
                self.v(b);
                self.f.b(op::I32_MUL);
                self.v(c);
                self.f.b(op::I32_ADD);
            }
            Op::Fma(k, a, b, c) if self.relaxed => {
                // Lane 0 of the fused vector multiply-add.
                for (x, neg) in [(a, false), (b, false), (c, k == Fma::Sub)] {
                    self.v(x);
                    if neg {
                        self.f.b(op::F32_NEG);
                    }
                    self.f.fd(op::F32X4_SPLAT);
                }
                self.f.fd(if k == Fma::SubFrom { op::F32X4_RELAXED_NMADD } else { op::F32X4_RELAXED_MADD });
                self.f.fd(op::F32X4_EXTRACT_LANE);
                self.f.b(0);
            }
            Op::Fma(k, a, b, c) => {
                let neg_a = k == Fma::SubFrom;
                let neg_c = k == Fma::Sub;
                self.v(a);
                if neg_a {
                    self.f.b(op::F32_NEG);
                }
                self.f.b(op::F64_PROMOTE_F32);
                self.v(b);
                self.f.b(op::F64_PROMOTE_F32);
                self.f.b(op::F64_MUL);
                self.f.set(self.tf64[0]);
                self.v(c);
                if neg_c {
                    self.f.b(op::F32_NEG);
                }
                self.f.b(op::F64_PROMOTE_F32);
                self.f.set(self.tf64[1]);
                self.fma_tail();
            }
            Op::In { ch, idx } => {
                self.io_address(ch & 1, idx);
                self.f.mem(op::F32_LOAD, 0);
            }
        }
        let _ = (t1, t2);
    }

    /// With p = a * b (exact) in tf64[0] and c in tf64[1] (both f64),
    /// pushes the f32 rounding of the exact p + c, as a fused multiply-add
    /// rounds. s = p + c rounded to f64 then to f32 rounds twice, which
    /// differs from one rounding only where s lands exactly on a midpoint
    /// between two f32s: its low 29 bits are 1 << 28, or, for a result in
    /// the f32 subnormal range (|s| < 2^-125 is tested), the midpoint lies
    /// higher. There s is rounded to odd instead: its last bit forced
    /// towards the exact error when the sum was inexact. Round-to-odd then
    /// to f32 rounds once.
    fn fma_tail(&mut self) {
        fma_tail(&mut self.f, self.tf64, self.ti64[0]);
    }

    fn un(&mut self, u: Un, a: Val) {
        let tf32 = self.tf32;
        let td = self.tf64[0];
        self.v(a);
        let f = &mut self.f;
        match u {
            Un::NegF => f.b(op::F32_NEG),
            Un::AbsF => f.b(op::F32_ABS),
            Un::SqrtF => f.b(op::F32_SQRT),
            Un::FloorF => f.b(op::F32_FLOOR),
            Un::CeilF => f.b(op::F32_CEIL),
            Un::TruncF => f.b(op::F32_TRUNC),
            Un::RoundF => {
                // t = trunc(x); |x - t| >= 0.5 ? t + copysign(1, x) : t
                f.tee(tf32);
                f.b(op::F32_TRUNC);
                f.f32c(1.0);
                f.get(tf32);
                f.b(op::F32_COPYSIGN);
                f.b(op::F32_ADD);
                f.get(tf32);
                f.b(op::F32_TRUNC);
                f.get(tf32);
                f.get(tf32);
                f.b(op::F32_TRUNC);
                f.b(op::F32_SUB);
                f.b(op::F32_ABS);
                f.f32c(0.5);
                f.b(op::F32_GE);
                f.b(op::SELECT);
            }
            Un::F2I => f.fc(op::I32_TRUNC_SAT_F32_S),
            Un::I2F => f.b(op::F32_CONVERT_I32_S),
            Un::BitsFI => f.b(op::I32_REINTERPRET_F32),
            Un::BitsIF => f.b(op::F32_REINTERPRET_I32),
            Un::NegI => {
                f.i32c(-1);
                f.b(op::I32_MUL);
            }
            Un::NotB => f.b(op::I32_EQZ),
            Un::NegD => f.b(op::F64_NEG),
            Un::AbsD => f.b(op::F64_ABS),
            Un::SqrtD => f.b(op::F64_SQRT),
            Un::FloorD => f.b(op::F64_FLOOR),
            Un::CeilD => f.b(op::F64_CEIL),
            Un::TruncD => f.b(op::F64_TRUNC),
            Un::RoundD => {
                f.tee(td);
                f.b(op::F64_TRUNC);
                f.f64c(1.0);
                f.get(td);
                f.b(op::F64_COPYSIGN);
                f.b(op::F64_ADD);
                f.get(td);
                f.b(op::F64_TRUNC);
                f.get(td);
                f.get(td);
                f.b(op::F64_TRUNC);
                f.b(op::F64_SUB);
                f.b(op::F64_ABS);
                f.f64c(0.5);
                f.b(op::F64_GE);
                f.b(op::SELECT);
            }
            Un::F2D => f.b(op::F64_PROMOTE_F32),
            Un::D2F => f.b(op::F32_DEMOTE_F64),
            Un::D2I => f.fc(op::I32_TRUNC_SAT_F64_S),
            Un::I2D => f.b(op::F64_CONVERT_I32_S),
            Un::HiD => {
                f.b(op::I64_REINTERPRET_F64);
                f.i64c(32);
                f.b(op::I64_SHR_U);
                f.b(op::I32_WRAP_I64);
            }
            Un::LoD => {
                f.b(op::I64_REINTERPRET_F64);
                f.b(op::I32_WRAP_I64);
            }
        }
    }

    fn bin(&mut self, b: Bin, x: Val, y: Val) {
        let [t0, t1, _] = self.ti;
        let yc = self.consts[y.0 as usize];
        match b {
            Bin::MinF | Bin::MaxF | Bin::MinD | Bin::MaxD => {
                // a < b ? a : b / a > b ? a : b
                self.v(x);
                self.v(y);
                self.v(x);
                self.v(y);
                self.f.b(match b {
                    Bin::MinF => op::F32_LT,
                    Bin::MaxF => op::F32_GT,
                    Bin::MinD => op::F64_LT,
                    _ => op::F64_GT,
                });
                self.f.b(op::SELECT);
                return;
            }
            Bin::DivI | Bin::RemI if yc.is_some_and(|c| c != 0 && c != -1) => {
                self.v(x);
                self.v(y);
                self.f.b(if b == Bin::DivI { op::I32_DIV_S } else { op::I32_REM_S });
                return;
            }
            Bin::DivI => {
                let (lx, ly) = (self.val[x.0 as usize], self.val[y.0 as usize]);
                div_s(&mut self.f, lx, ly, t0);
                let _ = t1;
                return;
            }
            Bin::RemI => {
                let (lx, ly) = (self.val[x.0 as usize], self.val[y.0 as usize]);
                rem_s(&mut self.f, lx, ly);
                return;
            }
            Bin::MakeD => {
                self.v(x);
                self.f.b(op::I64_EXTEND_I32_U);
                self.f.i64c(32);
                self.f.b(op::I64_SHL);
                self.v(y);
                self.f.b(op::I64_EXTEND_I32_U);
                self.f.b(op::I64_OR);
                self.f.b(op::F64_REINTERPRET_I64);
                return;
            }
            _ => {}
        }
        self.v(x);
        self.v(y);
        self.f.b(match b {
            Bin::AddF => op::F32_ADD,
            Bin::SubF => op::F32_SUB,
            Bin::MulF => op::F32_MUL,
            Bin::DivF => op::F32_DIV,
            Bin::AddI => op::I32_ADD,
            Bin::SubI => op::I32_SUB,
            Bin::MulI => op::I32_MUL,
            Bin::AndI | Bin::AndB => op::I32_AND,
            Bin::OrI | Bin::OrB => op::I32_OR,
            Bin::XorI => op::I32_XOR,
            Bin::ShlI => op::I32_SHL,
            Bin::ShrI => op::I32_SHR_S,
            Bin::ShrUI => op::I32_SHR_U,
            Bin::AddD => op::F64_ADD,
            Bin::SubD => op::F64_SUB,
            Bin::MulD => op::F64_MUL,
            Bin::DivD => op::F64_DIV,
            _ => unreachable!(),
        });
    }
}

/// [`Sc::fma_tail`] on any locals: p and c (f64) in `t[0]`, `t[1]`, `t[2]`
/// and `t[3]` scratch f64, `bits` a scratch i64.
fn fma_tail(f: &mut Body, t: [u32; 4], bits: u32) {
    let [p, c, s, e] = t;
    // s = p + c
    f.get(p);
    f.get(c);
    f.b(op::F64_ADD);
    f.tee(s);
    f.b(op::I64_REINTERPRET_F64);
    f.b(op::I32_WRAP_I64);
    f.i32c(0x1FFF_FFFF);
    f.b(op::I32_AND);
    f.i32c(0x1000_0000);
    f.b(op::I32_EQ);
    f.get(s);
    f.b(op::F64_ABS);
    f.f64c(f64::from_bits(0x3820_0000_0000_0000));
    f.b(op::F64_LT);
    f.b(op::I32_OR);
    f.b(op::IF);
    f.b(op::VOID);
    // bb = s - p; e = (p - (s - bb)) + (c - bb)
    f.get(p);
    f.get(s);
    f.get(s);
    f.get(p);
    f.b(op::F64_SUB);
    f.tee(e);
    f.b(op::F64_SUB);
    f.b(op::F64_SUB);
    f.get(c);
    f.get(e);
    f.b(op::F64_SUB);
    f.b(op::F64_ADD);
    f.set(e);
    // bits = s's bits
    f.get(s);
    f.b(op::I64_REINTERPRET_F64);
    f.tee(bits);
    // delta = ((e ^ s) >> 63) | 1 (towards the error)
    f.get(e);
    f.b(op::I64_REINTERPRET_F64);
    f.get(bits);
    f.b(op::I64_XOR);
    f.i64c(63);
    f.b(op::I64_SHR_S);
    f.i64c(1);
    f.b(op::I64_OR);
    f.i64c(0);
    // when e != 0, s's last bit is 0 and s is finite
    f.get(e);
    f.f64c(0.0);
    f.b(op::F64_NE);
    f.get(bits);
    f.i64c(1);
    f.b(op::I64_AND);
    f.b(op::I64_EQZ);
    f.b(op::I32_AND);
    f.get(s);
    f.b(op::F64_ABS);
    f.f64c(f64::MAX);
    f.b(op::F64_LE);
    f.b(op::I32_AND);
    f.b(op::SELECT);
    f.b(op::I64_ADD);
    f.b(op::F64_REINTERPRET_I64);
    f.set(s);
    f.b(op::END);
    f.get(s);
    f.b(op::F32_DEMOTE_F64);
}

/// `x / y` of two i32 locals with AIR's rules (division by zero gives
/// 0, `MIN / -1` wraps): wasm's own division traps on both.
fn div_s(f: &mut Body, x: u32, y: u32, t: u32) {
    // y == 0 ? 0 : y == -1 ? 0 - x : x / y
    f.i32c(0);
    f.get(x);
    f.b(op::I32_SUB);
    f.get(x);
    f.get(y);
    f.i32c(1);
    f.get(y);
    f.i32c(1);
    f.b(op::I32_ADD);
    f.i32c(1);
    f.b(op::I32_GT_U);
    f.b(op::SELECT);
    f.b(op::I32_DIV_S);
    f.get(y);
    f.i32c(-1);
    f.b(op::I32_EQ);
    f.b(op::SELECT);
    f.set(t);
    f.i32c(0);
    f.get(t);
    f.get(y);
    f.b(op::I32_EQZ);
    f.b(op::SELECT);
}

/// `x % y` of two i32 locals with AIR's rules (`x % 0` is x; wasm's
/// `rem_s` of MIN by -1 is 0, as AIR's wrapping remainder).
fn rem_s(f: &mut Body, x: u32, y: u32) {
    f.get(x);
    f.get(x);
    f.i32c(1);
    f.get(y);
    f.get(y);
    f.b(op::I32_EQZ);
    f.b(op::SELECT);
    f.b(op::I32_REM_S);
    f.get(y);
    f.b(op::I32_EQZ);
    f.b(op::SELECT);
}

/// The i32 on the stack wrapped Euclidean into `0..len` (len not a power
/// of two): r = x rem len; r < 0 ? r + len : r (`t` a scratch i32).
fn wrap_rem(f: &mut Body, len: u32, t: u32) {
    f.i32c(len as i32);
    f.b(op::I32_REM_S);
    f.tee(t);
    f.i32c(len as i32);
    f.b(op::I32_ADD);
    f.get(t);
    f.get(t);
    f.i32c(0);
    f.b(op::I32_LT_S);
    f.b(op::SELECT);
}

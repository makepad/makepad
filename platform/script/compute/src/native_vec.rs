//! A kernel's element loop as vector code through the shared back end: `lanes` elements per
//! iteration (4 or 8), lowered to RIR's 128-bit vectors (masked SPMD, [`crate::spmd`]'s
//! analysis -- the same as wasm SIMD128) and emitted by the shared x64 (SSE4.1/AVX2) or arm64
//! (NEON) code generator. Bit-identical to the scalar code and the interpreter at every width.
//!
//! Every AIR value is `parts` V128 vregs (lanes / 4), element `4 p + l` in lane l of part p,
//! holding exactly the bits the scalar code holds for that element (bools are 0/1 words).
//! Eight lanes are two independent four-lane halves in flight: on AArch64 two NEON registers
//! per value (the halves' chains interleave and hide latency), on x86-64 with AVX2 the
//! back end's `pair256` fuses each pair of halves into one ymm instruction. Uniform control
//! flow is real branches on lane 0; divergent ifs run both sides under the execution mask `vm`,
//! masked loops keep break/continue masks, variables set while lanes are off are blended,
//! stores write the running lanes in lane order (element order, whatever the width). Integer
//! division, non-power-of-two wraps and varying-address memory run per lane with the scalar
//! sequences.
//!
//! Nothing a result depends on varies with the width: every op is lane-wise (an element's bits
//! depend on that element only), stores land in element order, and the one cross-lane sum (a
//! run's counted work, `crate::work`) is a wrapping integer sum, exact in any order.
//!
//! The element loop is the shape `lower_kernel` emits ([`crate::spmd::shape`]): its counter
//! holds `[i, i+1, .., i+lanes-1]`, steps by `lanes`, and its `i >= n` exit is taken on lane 0
//! (the caller runs `n` a multiple of `lanes`; the tail runs on scalar code). [`compile`]
//! declines (None) whatever [`crate::spmd::supported`] rejects.
//!
//! The width is a per-host parameter chosen by rule ([`host_lanes`]), no tuning: 8 on x86-64
//! with AVX2 + FMA (ymm through `pair256`), 4 elsewhere (see host_lanes for the AArch64
//! measurement). Eight-lane hosts install four-lane code and upgrade hot kernels in the
//! background ([`install_lanes`]).

use crate::ir::{Bin, Block, Cmp, Fma, Op, Program, Region, Stmt, Un, Val, Var};
use crate::native::Code;
use crate::spmd::{analyse, id, shape, steps, supported, Info};
use relax_backend::rir::{self, Cls, Cond, Conv, Func, IOp, Inst, IntTy, Mem, NanBits, RoundTo, Term, VReg, VW, VX};
use std::cell::Cell;

const I32: IntTy = IntTy { bits: 32, signed: true };
const U32: IntTy = IntTy { bits: 32, signed: false };
const I64: IntTy = rir::I64;
const W32: Mem = Mem::Int(4, true);
const PTR: Mem = Mem::Int(8, false);

/// Largest frame of an eight-lane kernel in words per lane: 8 lanes x 2048 words = 64 KiB, the
/// four-lane limit's bytes ([`crate::spmd::MAX_FRAME_WORDS`]); a larger frame runs four lanes.
pub(crate) const MAX_FRAME_WORDS_X8: u32 = 2048;

/// What the vector code needs from the CPU (x64: SSE4.1, AVX2 per-lane shifts, FMA3).
fn cpu_ok() -> bool {
    #[cfg(target_arch = "x86_64")]
    {
        std::is_x86_feature_detected!("sse4.1") && std::is_x86_feature_detected!("avx2") && std::is_x86_feature_detected!("fma")
    }
    #[cfg(not(target_arch = "x86_64"))]
    true
}

/// The lanes of this host's vector kernels, by rule per CPU class (no tuning): x86-64 with
/// AVX2 + FMA (the CPUs `cpu_ok` admits) runs eight (the back end's `pair256` makes each pair of
/// 128-bit halves one ymm instruction); AArch64 runs four. `MAKEPAD_KERNEL_LANES=4|8` overrides
/// it for the process (A/B measurement); [`with_lanes`] for one thread's compiles (tests).
pub fn host_lanes() -> u32 {
    host().0
}

/// (this host's lanes, whether MAKEPAD_KERNEL_LANES set them)
fn host() -> (u32, bool) {
    static L: std::sync::OnceLock<(u32, bool)> = std::sync::OnceLock::new();
    *L.get_or_init(|| match std::env::var("MAKEPAD_KERNEL_LANES").ok().as_deref() {
        Some("4") => (4, true),
        Some("8") => (8, true),
        _ => (if cfg!(target_arch = "x86_64") { 8 } else { 4 }, false),
    })
}

/// What a kernel install compiles now, and whether it moves to eight lanes once hot. Under the
/// host rule an eight-lane host installs four-lane code (it compiles in about 2/3 of the time:
/// Stage cold on 165 measured +8% CPU at present and +10% instructions with eight-lane installs)
/// and upgrades a kernel in the background after [`HOT_ELEMENTS`] vector elements, so cold
/// start pays nothing for the width. A thread override ([`with_lanes`]) or MAKEPAD_KERNEL_LANES
/// compiles that width at once, with no upgrade (tests, A/Bs). The results are the same bits
/// at either width, so the switch is invisible to them.
pub fn install_lanes() -> (u32, bool) {
    match LANES.with(|c| c.get()) {
        0 => match host() {
            (l, true) => (l, false),
            (8, false) => (4, true),
            (l, false) => (l, false),
        },
        l => (l, false),
    }
}

/// Vector elements a kernel runs at four lanes before its eight-lane code is compiled (in the
/// background, once): a fixed rule, no tuning.
pub const HOT_ELEMENTS: u64 = 1 << 16;

thread_local! {
    static LANES: Cell<u32> = const { Cell::new(0) };
}

/// Runs `f` with this thread's kernel compiles at `lanes` (4 or 8) instead of [`host_lanes`]:
/// the cross-width differential tests and benchmarks compile one kernel at both widths.
pub fn with_lanes<R>(lanes: u32, f: impl FnOnce() -> R) -> R {
    assert!(lanes == 4 || lanes == 8, "kernel lanes are 4 or 8");
    let old = LANES.with(|c| c.replace(lanes));
    let r = f();
    LANES.with(|c| c.set(old));
    r
}

/// The lanes a compile on this thread makes now.
pub fn lanes() -> u32 {
    match LANES.with(|c| c.get()) {
        0 => host_lanes(),
        l => l,
    }
}

/// Compiles a kernel's element loop to vector code of [`lanes`] lanes, or declines: (code,
/// lanes). An eight-lane compile whose frame is too large makes four-lane code.
pub fn compile(p: &Program) -> Option<(Code, u32)> {
    compile_at(p, lanes())
}

/// [`compile`] at `lanes` (4 or 8).
pub fn compile_at(p: &Program, lanes: u32) -> Option<(Code, u32)> {
    if !cpu_ok() {
        return None;
    }
    let mut lanes = lanes;
    if lanes == 8 && p.frame_words > MAX_FRAME_WORDS_X8 {
        lanes = 4;
    }
    let flat = crate::ir::flat(p);
    let tag = if lanes == 8 { "x8" } else { "x4" };
    let key = crate::kcache::key(tag, &flat, 0);
    if let Some(code) = key.and_then(crate::native::load) {
        if crate::kcache::verify() {
            let fresh = compile_fresh(p, lanes, None).expect("a stored kernel compiles");
            crate::kcache::same(tag, code.bytes(), fresh.bytes());
        }
        return Some((code, lanes));
    }
    compile_fresh(p, lanes, key).map(|c| (c, lanes))
}

fn compile_fresh(p: &Program, lanes: u32, key: Option<u128>) -> Option<Code> {
    let mut f = rir_of(p, lanes)?;
    crate::native::optimise_for_codegen(&mut f);
    if f.verify().is_err() {
        return None;
    }
    crate::native::pair_halves(&mut f);
    // one quick allocation round, with copy coalescing (see emit_keyed, native::options)
    crate::native::emit_keyed(&f, key, false)
}

/// A value's vector: `parts` V128 vregs (the second unused at four lanes).
type V = [VReg; 2];

/// The vector RIR of a kernel at `lanes` (4 or 8) lanes (None when declined), before
/// optimisation; at eight lanes `halves` names every value's (low, high) parts.
pub fn rir_of(p: &Program, lanes: u32) -> Option<Func> {
    assert!(lanes == 4 || lanes == 8, "kernel lanes are 4 or 8");
    let np = (lanes / 4) as usize;
    let flat = crate::ir::flat(p);
    let p = &*flat;
    let sh = shape(p)?;
    let Stmt::Loop { body, .. } = sh.element else { return None };
    if !supported(p, body) {
        return None;
    }
    let info = analyse(p, &sh)?;
    let (step, _consts) = steps(p, &info, sh.i);
    let mut f = Func::new(format!("splash_kernel_x{}", lanes), 0);
    let entry = f.block();
    let mut regions = [VReg(0); 4];
    for r in regions.iter_mut() {
        *r = f.vreg(Cls::I);
        f.params.push(*r);
    }
    let n_raw = f.vreg(Cls::I);
    f.params.push(n_raw);
    let vnew = |f: &mut Func| -> V {
        let mut v = [VReg(0); 2];
        for x in v.iter_mut().take(np) {
            *x = f.vreg(Cls::V128);
        }
        v
    };
    let vals: Vec<V> = p.vals.iter().map(|_| vnew(&mut f)).collect();
    let vars: Vec<V> = p.vars.iter().map(|_| vnew(&mut f)).collect();
    let frame = if p.frame_words > 0 { Some(f.slot(p.frame_words * 16 * np as u32, 16)) } else { None };
    let vm = vnew(&mut f);
    let halves: Vec<(VReg, VReg)> = if np > 1 { vals.iter().chain(&vars).chain([&vm]).map(|v| (v[0], v[1])).collect() } else { Vec::new() };
    let mut lx = Vx {
        p,
        f,
        np,
        cur: entry,
        vals,
        vars,
        regions,
        n_i32: VReg(0),
        frame,
        loops: Vec::new(),
        bounds: crate::ir::bounds(p),
        info,
        step,
        vm,
        full: true,
        i: sh.i,
        consts: vec![None; p.vals.len()],
        masks: vec![None; p.vals.len()],
        bufs: Vec::new(),
        defs: vec![None; p.vals.len()],
        grouped: Default::default(),
        halves,
    };
    fn walk(b: &Block, defs: &mut Vec<Option<Op>>) {
        for s in b {
            match s {
                Stmt::Def(v, op) => defs[v.0 as usize] = Some(*op),
                Stmt::If(_, t, e) => {
                    walk(t, defs);
                    walk(e, defs);
                }
                Stmt::Loop { body, .. } => walk(body, defs),
                _ => {}
            }
        }
    }
    walk(&p.body, &mut lx.defs);
    let nu = lx.conv(Conv::IntToInt(U32), n_raw);
    lx.n_i32 = lx.conv(Conv::IntToInt(I32), nu);
    let ones = lx.iconst(-1);
    for q in 0..np {
        lx.emit(Inst::VSplat(vm[q], ones));
    }
    for k in 0..=crate::native::max_buf(&p.body) {
        let ptr = lx.f.vreg(Cls::I);
        lx.emit(Inst::Load(PTR, ptr, lx.regions[3], k * 16));
        let len = lx.f.vreg(Cls::I);
        lx.emit(Inst::Load(PTR, len, lx.regions[3], k * 16 + 8));
        let last = lx.ibini(IOp::Sub, I64, len, 1);
        lx.bufs.push((ptr, last));
    }
    for s in sh.prelude {
        if let Stmt::Set(var, _) = s {
            if *var == sh.i {
                // the lanes' element offsets [0, 1, .., lanes - 1]
                let d = lx.vars[var.0 as usize];
                if np == 1 {
                    lx.lane_ids_into(d[0], 0);
                } else {
                    // built apart, then moved into the counter's halves together (pair256)
                    let t = lx.vnew();
                    for q in 0..np {
                        lx.lane_ids_into(t[q], q);
                    }
                    lx.mov_into(d, t);
                }
                continue;
            }
        }
        lx.stmt(s);
    }
    lx.stmt(sh.element);
    if let Some((var, word)) = sh.post {
        // the run's counted work: the lanes' counts summed into its ctx word (a wrapping sum:
        // the same in any order, so at any width)
        let v = lx.vars[var.0 as usize];
        let mut sum = lx.extract_i(v, 0);
        for l in 1..lanes as u8 {
            let x = lx.extract_i(v, l);
            sum = lx.ibin(IOp::Add, I32, sum, x);
        }
        lx.emit(Inst::Store(W32, lx.regions[0], 4 * word as i32, sum));
    }
    lx.term(Term::Ret(Vec::new()));
    lx.f.halves = lx.halves;
    Some(lx.f)
}

struct Loop {
    masked: bool,
    top: u32,
    exit: u32,
    iter_end: u32,
    brk: V,
    cont: V,
}

struct Vx<'a> {
    p: &'a Program,
    f: Func,
    /// V128 parts per value (lanes / 4)
    np: usize,
    cur: u32,
    vals: Vec<V>,
    vars: Vec<V>,
    regions: [VReg; 4],
    n_i32: VReg,
    frame: Option<u32>,
    loops: Vec<Loop>,
    bounds: Vec<Option<u32>>,
    info: Info,
    /// per value: lanes are `lane0 + l * step`
    step: Vec<Option<i64>>,
    /// the execution mask (lane masks)
    vm: V,
    /// statically every lane runs
    full: bool,
    i: Var,
    /// constant values (materialised at each use)
    consts: Vec<Option<u32>>,
    /// per compare / logic value: its lane mask (all-ones / zero lanes) beside its 0/1 word
    masks: Vec<Option<V>>,
    /// per host buffer: (base pointer, last word index), loaded once at entry
    bufs: Vec<(VReg, VReg)>,
    /// per value: its defining op
    defs: Vec<Option<Op>>,
    /// loads a load group already made (statement ids)
    grouped: std::collections::HashSet<usize>,
    /// every value's (low, high) halves at eight lanes (`pair256` seeds)
    halves: Vec<(VReg, VReg)>,
}

impl<'a> Vx<'a> {
    fn lanes(&self) -> u8 {
        4 * self.np as u8
    }

    fn emit(&mut self, i: Inst) {
        let b = &mut self.f.blocks[self.cur as usize];
        b.insts.push(i);
        b.pos.push(0);
    }

    fn term(&mut self, t: Term) {
        self.f.blocks[self.cur as usize].term = t;
    }

    fn block_new(&mut self) -> u32 {
        self.f.block()
    }

    fn vnew(&mut self) -> V {
        let mut v = [VReg(0); 2];
        for q in 0..self.np {
            v[q] = self.f.vreg(Cls::V128);
        }
        if self.np > 1 {
            self.halves.push((v[0], v[1]));
        }
        v
    }

    fn iconst(&mut self, k: i64) -> VReg {
        let d = self.f.vreg(Cls::I);
        self.emit(Inst::Iconst(d, k));
        d
    }

    fn conv(&mut self, c: Conv, a: VReg) -> VReg {
        let d = self.f.vreg(Cls::I);
        self.emit(Inst::Conv(c, d, a));
        d
    }

    fn ibin(&mut self, op: IOp, it: IntTy, a: VReg, b: VReg) -> VReg {
        let d = self.f.vreg(Cls::I);
        self.emit(Inst::IBin(op, it, d, a, b));
        d
    }

    fn ibini(&mut self, op: IOp, it: IntTy, a: VReg, k: i64) -> VReg {
        let c = self.iconst(k);
        self.ibin(op, it, a, c)
    }

    fn icmp(&mut self, c: Cond, signed: bool, a: VReg, b: VReg) -> VReg {
        let d = self.f.vreg(Cls::I);
        self.emit(Inst::ICmp(c, signed, d, a, b));
        d
    }

    fn sel(&mut self, c: VReg, a: VReg, b: VReg) -> VReg {
        let d = self.f.vreg(self.f.vregs[a.0 as usize]);
        self.emit(Inst::Sel(d, c, a, b));
        d
    }

    fn splat_const(&mut self, bits: u32) -> V {
        let d = self.vnew();
        for q in 0..self.np {
            self.emit(Inst::VConst(d[q], bits));
        }
        d
    }

    /// `d = op(a, b)` part by part (the halves' instructions next to each other: `pair256`).
    fn vbin_into(&mut self, op: VX, d: V, a: V, b: V) {
        for q in 0..self.np {
            self.emit(Inst::VX(op, d[q], a[q], b[q]));
        }
    }

    fn vbin(&mut self, op: VX, a: V, b: V) -> V {
        let d = self.vnew();
        self.vbin_into(op, d, a, b);
        d
    }

    fn vsel_into(&mut self, d: V, m: V, a: V, b: V) {
        for q in 0..self.np {
            self.emit(Inst::VSel(d[q], m[q], a[q], b[q]));
        }
    }

    fn mov_into(&mut self, d: V, s: V) {
        for q in 0..self.np {
            self.emit(Inst::Mov(d[q], s[q]));
        }
    }

    fn splat_into(&mut self, d: V, s: VReg) {
        for q in 0..self.np {
            self.emit(Inst::VSplat(d[q], s));
        }
    }

    /// `a` shifted by the immediate `k` (op: one of `VX::ShlI`, `ShrSI`, `ShrUI` of 32-bit lanes)
    fn vshift(&mut self, op: fn(VW, u8) -> VX, a: V, k: u8) -> V {
        self.vbin(op(VW::S, k), a, a)
    }

    /// Lane `l` of `d` = scalar `s`, building `d` lane by lane from lane 0 of each part: lane 0
    /// splats (a whole-register write: the build does not depend on d's last value, e.g. the
    /// previous element's), the others insert.
    fn put_lane(&mut self, d: V, l: u8, s: VReg) {
        let (q, k) = ((l / 4) as usize, l % 4);
        if k == 0 {
            self.emit(Inst::VSplat(d[q], s));
        } else {
            self.emit(Inst::VInsert(d[q], d[q], k, s));
        }
    }

    fn extract_i(&mut self, v: V, lane: u8) -> VReg {
        let d = self.f.vreg(Cls::I);
        self.emit(Inst::VExtract(d, v[(lane / 4) as usize], lane % 4));
        d
    }

    /// The vector of value `x` (constants splatted at each use).
    fn v(&mut self, x: Val) -> V {
        match self.consts[x.0 as usize] {
            Some(bits) => self.splat_const(bits),
            None => self.vals[x.0 as usize],
        }
    }

    /// Lane `l` of value `x` as a scalar of class `scls` (an I lane sign-extended, as `VExtract`
    /// reads it). A constant is the scalar constant itself: an extract of its splat is no constant
    /// to the back end, so LICM hoisted every such extract to the entry and the values stayed live
    /// through every per-lane block after it (Stage's largest kernel: ~4.8k of ~6.9k values live
    /// at each of its 14k blocks); a constant is rematerialised and sunk to its uses instead.
    fn lane(&mut self, x: Val, l: u8, scls: Cls) -> VReg {
        let d = self.f.vreg(scls);
        match self.consts[x.0 as usize] {
            Some(bits) if scls == Cls::F32 => self.emit(Inst::Fconst(d, bits as u64, false)),
            Some(bits) => self.emit(Inst::Iconst(d, bits as i32 as i64)),
            None => {
                let v = self.vals[x.0 as usize];
                self.emit(Inst::VExtract(d, v[(l / 4) as usize], l % 4));
            }
        }
        d
    }

    fn uniform(&self, x: Val) -> bool {
        !self.info.vval[x.0 as usize]
    }

    /// A lane mask -> the scalar code's 0/1 word.
    fn mask_to_word(&mut self, m: V) -> V {
        let one = self.splat_const(1);
        self.vbin(VX::And, m, one)
    }

    /// The lane mask of `x != 0` (the compare's own mask when `x` is one).
    fn mask(&mut self, x: Val) -> V {
        if let Some(m) = self.masks[x.0 as usize] {
            return m;
        }
        let w = self.v(x);
        self.word_to_mask(w)
    }

    /// A word (any bits) -> lane mask of `word != 0`.
    fn word_to_mask(&mut self, w: V) -> V {
        let z = self.splat_const(0);
        self.vbin(VX::ICmp(Cond::Ne, VW::S), w, z)
    }

    /// The largest unsigned lane of `v` as a scalar (zero-extended).
    fn max_across(&mut self, v: V) -> VReg {
        let mx = self.f.vreg(Cls::I);
        let mut m = v[0];
        for q in 1..self.np {
            let t = self.f.vreg(Cls::V128);
            self.emit(Inst::VX(VX::MaxU(VW::S), t, m, v[q]));
            m = t;
        }
        self.emit(Inst::VX(VX::MaxAcrossU(VW::S), mx, m, m));
        mx
    }

    // -- memory --------------------------------------------------------------

    /// Scalar address of word `base + clamp(o)` of a non-buffer, non-frame region (o: a
    /// scalar i32 offset or None).
    fn region_addr(&mut self, region: Region, base: u32, extent: u32, o: Option<(Val, VReg)>) -> (VReg, i32) {
        let rb = match region {
            Region::Ctx => self.regions[0],
            Region::State => self.regions[1],
            _ => self.regions[2],
        };
        match o {
            None => (rb, (base * 4) as i32),
            Some((ov, so)) => {
                let idx = self.clamp(ov, so, extent);
                let sc = self.ibini(IOp::Shl, I64, idx, 2);
                let a = self.ibin(IOp::Add, I64, rb, sc);
                (a, (base * 4) as i32)
            }
        }
    }

    /// `min(o as u32, extent - 1)` unless proven in range.
    fn clamp(&mut self, ov: Val, so: VReg, extent: u32) -> VReg {
        if self.bounds[ov.0 as usize].is_some_and(|b| b < extent && b < 1 << 31) {
            return so;
        }
        let top = self.iconst(extent.saturating_sub(1) as i64);
        let c = self.icmp(Cond::Le, false, so, top);
        self.sel(c, so, top)
    }

    /// Bytes per frame word (all lanes): 16 per part.
    fn frame_row(&self) -> u32 {
        16 * self.np as u32
    }

    /// Address of word `base + clamp(o)` of the per-lane frame, lane `lane`: word w of lane l
    /// lives at frame + row w + 4 l (row = 16 bytes per part).
    fn frame_addr(&mut self, base: u32, extent: u32, o: Option<(Val, VReg)>, lane: u8) -> (VReg, i32) {
        let fr = self.f.vreg(Cls::I);
        self.emit(Inst::SlotAddr(fr, self.frame.unwrap_or(0)));
        let row = self.frame_row();
        match o {
            None => (fr, (base * row + 4 * lane as u32) as i32),
            Some((ov, so)) => {
                let idx = self.clamp(ov, so, extent);
                let sc = self.ibini(IOp::Shl, I64, idx, row.trailing_zeros() as i64);
                let a = self.ibin(IOp::Add, I64, fr, sc);
                (a, (base * row + 4 * lane as u32) as i32)
            }
        }
    }

    /// Host buffer k: (base pointer, last word index).
    fn buf_regs(&mut self, k: u8) -> (VReg, VReg) {
        self.bufs[k as usize]
    }

    /// Address of word `min(base + zext(o), last)` of a buffer (64-bit, no wrap).
    fn buf_addr(&mut self, ptr: VReg, last: VReg, base: u32, so: Option<VReg>) -> VReg {
        let idx = match so {
            Some(so) => {
                let z = self.conv(Conv::IntToInt(U32), so);
                if base > 0 {
                    self.ibini(IOp::Add, I64, z, base as i64)
                } else {
                    z
                }
            }
            None => self.iconst(base as i64),
        };
        let c = self.icmp(Cond::Le, false, idx, last);
        let idx = self.sel(c, idx, last);
        let sc = self.ibini(IOp::Shl, I64, idx, 2);
        self.ibin(IOp::Add, I64, ptr, sc)
    }

    /// Lane l's scalar offset (i32) of a vector offset value.
    fn lane_off(&mut self, off: Option<Val>, l: u8) -> Option<(Val, VReg)> {
        let o = off?;
        let ov = self.v(o);
        let so = self.extract_i(ov, l);
        Some((o, so))
    }

    fn load(&mut self, d: V, region: Region, base: u32, extent: u32, off: Option<Val>, fp: bool) {
        let m = if fp { Mem::F32 } else { W32 };
        let scls = if fp { Cls::F32 } else { Cls::I };
        let uniform = off.is_none_or(|o| self.uniform(o));
        if region == Region::Frame && uniform {
            let o = self.lane_off(off, 0);
            let (a, k) = self.frame_addr(base, extent, o, 0);
            for q in 0..self.np {
                self.emit(Inst::VLoad(d[q], a, k + 16 * q as i32));
            }
            return;
        }
        if uniform {
            let o = self.lane_off(off, 0);
            let s = self.f.vreg(scls);
            match region {
                Region::Buf(k) => {
                    let (ptr, last) = self.buf_regs(k);
                    let a = self.buf_addr(ptr, last, base, o.map(|x| x.1));
                    self.emit(Inst::Load(m, s, a, 0));
                }
                _ => {
                    let (a, k) = self.region_addr(region, base, extent, o);
                    self.emit(Inst::Load(m, s, a, k));
                }
            }
            self.splat_into(d, s);
            return;
        }
        let o = off.unwrap();
        if let (Region::Buf(k), Some(1)) = (region, self.step[o.0 as usize]) {
            // `lanes` consecutive words: a vector load per part when all lie inside the buffer
            let (ptr, last) = self.buf_regs(k);
            let ov = self.v(o);
            let s0 = self.extract_i(ov, 0);
            let z = self.conv(Conv::IntToInt(U32), s0);
            let span = base as i64 + self.lanes() as i64 - 1;
            let end = self.ibini(IOp::Add, I64, z, span);
            let inside = self.icmp(Cond::Le, false, end, last);
            let (fast, slow, join) = (self.block_new(), self.block_new(), self.block_new());
            self.term(Term::Branch(inside, fast, slow));
            self.cur = fast;
            let idx = self.ibini(IOp::Add, I64, z, base as i64);
            let sc = self.ibini(IOp::Shl, I64, idx, 2);
            let a = self.ibin(IOp::Add, I64, ptr, sc);
            for q in 0..self.np {
                self.emit(Inst::VLoad(d[q], a, 16 * q as i32));
            }
            self.term(Term::Jump(join));
            self.cur = slow;
            // built in temporaries, then moved into d's parts together (every definition of d's
            // parts a pair of like instructions: `pair256` can keep d one register)
            let t = if self.np > 1 { self.vnew() } else { d };
            for l in 0..self.lanes() {
                let so = self.extract_i(ov, l);
                let a = self.buf_addr(ptr, last, base, Some(so));
                let s = self.f.vreg(scls);
                self.emit(Inst::Load(m, s, a, 0));
                self.put_lane(t, l, s);
            }
            if self.np > 1 {
                self.mov_into(d, t);
            }
            self.term(Term::Jump(join));
            self.cur = join;
            return;
        }
        // a gather of clamped word indices (one instruction on x64 AVX2)
        let ov = self.v(o);
        match region {
            Region::Buf(k) => self.buf_gather(d, k, base, ov, m, scls),
            Region::Frame => {
                // word w of lane l: frame + row w + 4 l -> gather index (row / 4) clamp(o) + l
                let idx = self.vclamp(o, ov, extent);
                let row = self.frame_row();
                let i4 = self.vshift(VX::ShlI, idx, (row / 4).trailing_zeros() as u8);
                let mut il = [VReg(0); 2];
                for q in 0..self.np {
                    let ids = self.lane_ids(q);
                    il[q] = self.f.vreg(Cls::V128);
                    self.emit(Inst::VX(VX::Add(VW::S), il[q], i4[q], ids));
                }
                let fr = self.f.vreg(Cls::I);
                self.emit(Inst::SlotAddr(fr, self.frame.unwrap_or(0)));
                let b0 = self.ibini(IOp::Add, I64, fr, (row * base) as i64);
                self.gather(d, b0, il);
            }
            _ => {
                let rb = match region {
                    Region::Ctx => self.regions[0],
                    Region::State => self.regions[1],
                    _ => self.regions[2],
                };
                if region == Region::Shared {
                    let (idx, k) = self.table_index(o, ov, extent);
                    self.table_gather(d, base, extent, idx, k);
                } else {
                    let idx = self.vclamp(o, ov, extent);
                    let b0 = self.ibini(IOp::Add, I64, rb, 4 * base as i64);
                    self.gather(d, b0, idx);
                }
            }
        }
    }

    /// d = per lane word `idx_l` at `b0` (one gather per part, built in temporaries and moved
    /// into d's parts together: see `load`'s slow path).
    fn gather(&mut self, d: V, b0: VReg, idx: V) {
        let t = if self.np > 1 { self.vnew() } else { d };
        for q in 0..self.np {
            self.emit(Inst::VGather(t[q], b0, idx[q]));
        }
        if self.np > 1 {
            self.mov_into(d, t);
        }
    }

    /// The in-range word index of shared-table load `o` (vector `ov`) as (x, k): word = x << k. An
    /// index `x << k` known in range (a vec2 row's first word, `2 * h`) gives x itself, so the TBL
    /// byte index multiplies x by `0x04040404 << k` (wrapping products agree) instead of shifting.
    fn table_index(&mut self, o: Val, ov: V, extent: u32) -> (V, u8) {
        if self.bounds[o.0 as usize].is_some_and(|b| b < extent) {
            if let Some(Op::Bin(Bin::ShlI, x, y)) = self.defs[o.0 as usize] {
                if let Some(k) = self.consts[y.0 as usize].filter(|k| *k < 8) {
                    return (self.v(x), k as u8);
                }
            }
        }
        (self.vclamp(o, ov, extent), 0)
    }

    /// d = per lane word `base + (idx_l << k)` of the shared tables (in range): a small table
    /// (noise gradients, lookup rows: up to 16 words) on NEON is one TBL by byte indices (word i =
    /// bytes 4i .. 4i + 3), else a gather.
    fn table_gather(&mut self, d: V, base: u32, extent: u32, idx: V, k: u8) {
        let b0 = self.ibini(IOp::Add, I64, self.regions[2], 4 * base as i64);
        if cfg!(target_arch = "aarch64") && extent <= 16 {
            let k1 = self.splat_const(0x0404_0404u32 << k);
            let b4 = self.vbin(VX::MulLo(VW::S), idx, k1);
            let k2 = self.splat_const(0x0302_0100);
            let bi = self.vbin(VX::Add(VW::S), b4, k2);
            for q in 0..self.np {
                self.emit(Inst::VTbl(d[q], b0, bi[q], extent as u8));
            }
        } else {
            let idx = if k == 0 { idx } else { self.vshift(VX::ShlI, idx, k) };
            self.gather(d, b0, idx);
        }
    }

    /// d = per lane word `min(base + zext(ov_l), last)` of host buffer k: a gather of clamped indices.
    fn buf_gather(&mut self, d: V, k: u8, base: u32, ov: V, m: Mem, scls: Cls) {
        // index = min(base + o, last) = base + umin(o, last - base) when last >= base
        let (ptr, last) = self.buf_regs(k);
        let bk = self.iconst(base as i64);
        let ok = self.icmp(Cond::Ge, true, last, bk);
        let (gath, low, join) = (self.block_new(), self.block_new(), self.block_new());
        self.term(Term::Branch(ok, gath, low));
        self.cur = gath;
        let lim = self.ibini(IOp::Sub, I64, last, base as i64);
        let limv = self.vnew();
        self.splat_into(limv, lim);
        let idx = self.vbin(VX::MinU(VW::S), ov, limv);
        let b0 = self.ibini(IOp::Add, I64, ptr, 4 * base as i64);
        self.gather(d, b0, idx);
        self.term(Term::Jump(join));
        self.cur = low;
        // every lane clamps to the last word (a splat apart, moved into d's parts together: two
        // splats of one scalar into d's parts would be one value to value numbering, and d's
        // parts no longer a pair of like definitions)
        let a = self.buf_addr(ptr, last, base, None);
        let s = self.f.vreg(scls);
        self.emit(Inst::Load(m, s, a, 0));
        if self.np > 1 {
            let t = self.vnew();
            self.splat_into(t, s);
            self.mov_into(d, t);
        } else {
            self.splat_into(d, s);
        }
        self.term(Term::Jump(join));
        self.cur = join;
    }

    /// Lanes `min(o as u32, extent - 1)` unless the offset is proven in range.
    fn vclamp(&mut self, o: Val, ov: V, extent: u32) -> V {
        if self.bounds[o.0 as usize].is_some_and(|b| b < extent) {
            return ov;
        }
        let top = self.splat_const(extent.saturating_sub(1));
        self.vbin(VX::MinU(VW::S), ov, top)
    }

    /// Part q's lane numbers [4q, 4q + 1, 4q + 2, 4q + 3].
    fn lane_ids(&mut self, q: usize) -> VReg {
        let d = self.f.vreg(Cls::V128);
        self.lane_ids_into(d, q);
        d
    }

    fn lane_ids_into(&mut self, d: VReg, q: usize) {
        let z = self.iconst(4 * q as i64);
        self.emit(Inst::VSplat(d, z));
        for l in 1..4u8 {
            let k = self.iconst(4 * q as i64 + l as i64);
            self.emit(Inst::VInsert(d, d, l, k));
        }
    }

    /// Stores the running lanes of vector `v` to the 16 bytes per part at [a + k + 16 part].
    fn store_q(&mut self, a: VReg, k: i32, v: V) {
        if self.full {
            for q in 0..self.np {
                self.emit(Inst::VStore(a, k + 16 * q as i32, v[q]));
            }
        } else {
            let old = self.vnew();
            for q in 0..self.np {
                self.emit(Inst::VLoad(old[q], a, k + 16 * q as i32));
            }
            let new = self.vnew();
            self.vsel_into(new, self.vm, v, old);
            for q in 0..self.np {
                self.emit(Inst::VStore(a, k + 16 * q as i32, new[q]));
            }
        }
    }

    fn store(&mut self, region: Region, base: u32, extent: u32, off: Option<Val>, val: Val) {
        let fp = self.p.vals[val.0 as usize] == crate::ir::Ty::F32;
        let m = if fp { Mem::F32 } else { W32 };
        let scls = if fp { Cls::F32 } else { Cls::I };
        let rv = self.v(val);
        let uniform = off.is_none_or(|o| self.uniform(o));
        let lanes = self.lanes();
        if region == Region::Frame && uniform {
            let o = self.lane_off(off, 0);
            let (a, k) = self.frame_addr(base, extent, o, 0);
            self.store_q(a, k, rv);
            return;
        }
        let join = self.block_new();
        let mut buf = None;
        if let Region::Buf(k) = region {
            buf = Some(self.buf_regs(k));
            if let (false, Some(o)) = (uniform, off) {
                if self.step[o.0 as usize] == Some(1) {
                    let (ptr, last) = buf.unwrap();
                    let ov = self.v(o);
                    let s0 = self.extract_i(ov, 0);
                    let z = self.conv(Conv::IntToInt(U32), s0);
                    let end = self.ibini(IOp::Add, I64, z, base as i64 + lanes as i64 - 1);
                    let inside = self.icmp(Cond::Le, false, end, last);
                    let (fast, slow) = (self.block_new(), self.block_new());
                    self.term(Term::Branch(inside, fast, slow));
                    self.cur = fast;
                    let idx = self.ibini(IOp::Add, I64, z, base as i64);
                    let sc = self.ibini(IOp::Shl, I64, idx, 2);
                    let a = self.ibin(IOp::Add, I64, ptr, sc);
                    self.store_q(a, 0, rv);
                    self.term(Term::Jump(join));
                    self.cur = slow;
                } else if let (true, Some(st)) = (self.full, self.step[o.0 as usize].filter(|s| *s > 1 && *s <= 1 << 16)) {
                    // records `st` words apart, all inside, every lane running: a store per
                    // lane in lane order, no clamps (the last lane decides: lanes increase)
                    let (ptr, last) = buf.unwrap();
                    let ov = self.v(o);
                    let s0 = self.extract_i(ov, 0);
                    let z = self.conv(Conv::IntToInt(U32), s0);
                    let end = self.ibini(IOp::Add, I64, z, base as i64 + (lanes as i64 - 1) * st);
                    let inside = self.icmp(Cond::Le, false, end, last);
                    let (fast, slow) = (self.block_new(), self.block_new());
                    self.term(Term::Branch(inside, fast, slow));
                    self.cur = fast;
                    let idx = self.ibini(IOp::Add, I64, z, base as i64);
                    let sc = self.ibini(IOp::Shl, I64, idx, 2);
                    let a = self.ibin(IOp::Add, I64, ptr, sc);
                    for l in 0..lanes {
                        let sv = self.lane(val, l, scls);
                        self.emit(Inst::Store(m, a, (4 * st * l as i64) as i32, sv));
                    }
                    self.term(Term::Jump(join));
                    self.cur = slow;
                }
            }
        }
        // per lane, in lane order: a word two lanes store ends as the later element's
        for l in 0..lanes {
            let next = self.block_new();
            if !self.full {
                let ml = self.extract_i(self.vm, l);
                let run = self.block_new();
                self.term(Term::Branch(ml, run, next));
                self.cur = run;
            }
            let s = self.lane(val, l, scls);
            let lo = self.lane_off(off, l);
            match region {
                Region::Buf(_) => {
                    let (ptr, last) = buf.unwrap();
                    let a = self.buf_addr(ptr, last, base, lo.map(|x| x.1));
                    self.emit(Inst::Store(m, a, 0, s));
                }
                Region::Frame => {
                    let (a, k) = self.frame_addr(base, extent, lo, l);
                    self.emit(Inst::Store(m, a, k, s));
                }
                _ => {
                    let (a, k) = self.region_addr(region, base, extent, lo);
                    self.emit(Inst::Store(m, a, k, s));
                }
            }
            self.term(Term::Jump(next));
            self.cur = next;
        }
        self.term(Term::Jump(join));
        self.cur = join;
    }

    // -- statements ------------------------------------------------------------

    fn block(&mut self, b: &Block) {
        let mut k = 0;
        while k < b.len() {
            if self.grouped.contains(&id(&b[k])) {
                k += 1;
                continue;
            }
            self.load_group(&b[k..]);
            if self.grouped.contains(&id(&b[k])) {
                k += 1;
                continue;
            }
            // a group of stores handles statements that never escape
            let n = self.store_group(&b[k..]).max(1);
            if n == 1 {
                self.stmt(&b[k]);
            }
            if b[k..k + n].iter().any(|s| self.info.escapes.contains(&id(s))) {
                self.full = false;
            }
            k += n;
        }
    }

    /// A varying offset as `root + c` (c >= 0 a constant; c = 0: the offset itself).
    fn root_of(&self, o: Val) -> (Val, u32) {
        let k = |x: Val| self.consts[x.0 as usize].filter(|c| *c < 1 << 16);
        match self.defs[o.0 as usize] {
            Some(Op::Bin(Bin::AddI, x, y)) if k(y).is_some() && !self.uniform(x) => (x, k(y).unwrap()),
            Some(Op::Bin(Bin::AddI, x, y)) if k(x).is_some() && !self.uniform(y) => (y, k(x).unwrap()),
            _ => (o, 0),
        }
    }

    /// Loads of one region (a host buffer, or the shared tables) at varying offsets `root + c`
    /// (a record's fields read per lane: `src[i * 8]`, `src[i * 8 + 1]`, ...; a table's rows:
    /// `T4[i].x`, `T4[i].y`, ...) among the definitions that follow `b[0]` (up to the first
    /// statement that is not a definition): every four consecutive words holding at least two
    /// of them are one 16-byte load per lane and a 4x4 transpose per four lanes, when every
    /// lane's words lie inside (checked once, on the largest lane root): a buffer's length; for
    /// tables, no load clamps and each 16-byte load stays inside its table (any rows, not only
    /// consecutive ones). Else the loads one by one, as clamped gathers. Made at b[0]; the
    /// loads it made are in `grouped`.
    fn load_group(&mut self, b: &[Stmt]) {
        let Some(Stmt::Def(_, Op::Load { region, off: Some(o0), .. })) = b.first() else { return };
        let region = *region;
        if !matches!(region, Region::Buf(_) | Region::Shared) || self.uniform(*o0) {
            return;
        }
        let (r, _) = self.root_of(*o0);
        if self.step[r.0 as usize].is_some_and(|st| st.abs() < 4) {
            // consecutive lanes: a vector load each (load's fast path)
            return;
        }
        // (word, value, statement id, last word of its table) of the loads of root r
        let mut ls: Vec<(u32, Val, usize, u32)> = Vec::new();
        for st in b.iter().take(64) {
            match st {
                // (a small table is one TBL per read on NEON: cheaper than the rows)
                Stmt::Def(v, Op::Load { region: r2, base, extent, off: Some(o) }) if *r2 == region && !self.uniform(*o) && !(region == Region::Shared && cfg!(target_arch = "aarch64") && *extent <= 16) => {
                    let (root, c) = self.root_of(*o);
                    if root == r && (*base as u64 + c as u64) < 1 << 30 && (region != Region::Shared || c < *extent) {
                        ls.push((base + c, *v, id(st), (base + extent).saturating_sub(1)));
                    }
                }
                Stmt::Def(..) => {}
                _ => break,
            }
        }
        // chunks: four words from the lowest word not yet covered, kept when two loads fall in it
        let mut words: Vec<u32> = ls.iter().map(|x| x.0).collect();
        words.sort_unstable();
        words.dedup();
        let mut chunks: Vec<u32> = Vec::new();
        let mut j = 0;
        while j < words.len() {
            let w0 = words[j];
            let n = ls.iter().filter(|x| x.0 >= w0 && x.0 < w0 + 4).count();
            let mut e = j;
            while e < words.len() && words[e] < w0 + 4 {
                e += 1;
            }
            if n >= 2 {
                chunks.push(w0);
            }
            j = e;
        }
        let mine: Vec<(u32, Val, usize, u32)> = ls.iter().filter_map(|&(w, v, sid, _)| chunks.iter().find(|&&c| w >= c && w < c + 4).map(|&c| (w, v, sid, c))).collect();
        let rv = self.v(r);
        let inside = match region {
            Region::Buf(kb) => {
                if chunks.is_empty() {
                    return;
                }
                let lim = chunks.iter().map(|c| c + 3).max().unwrap();
                let (_, last) = self.buf_regs(kb);
                // every lane inside: max(zext root) + lim <= min(last, 2^31 - 1) (no root + c wraps)
                let mx = self.max_across(rv);
                let end = self.ibini(IOp::Add, I64, mx, lim as i64);
                let top = self.iconst(i32::MAX as i64);
                let small = self.icmp(Cond::Le, false, last, top);
                let cap = self.sel(small, last, top);
                self.icmp(Cond::Le, false, end, cap)
            }
            _ => {
                // no load clamps (root + c <= the table's last word - base) and every 16-byte load
                // stays inside a table of its chunk: max(zext root) <= lim
                let mut lim = i64::MAX;
                for &(w, v, _, c) in &mine {
                    let end = match self.defs[v.0 as usize] {
                        Some(Op::Load { base, extent, .. }) => (base + extent) as i64 - 1,
                        _ => unreachable!("a load"),
                    };
                    lim = lim.min(end - w as i64).min(end - c as i64 - 3);
                }
                if chunks.is_empty() || lim < 0 {
                    return;
                }
                let mx = self.max_across(rv);
                let k = self.iconst(lim);
                self.icmp(Cond::Le, false, mx, k)
            }
        };
        let (fast, slow, join) = (self.block_new(), self.block_new(), self.block_new());
        self.term(Term::Branch(inside, fast, slow));
        self.cur = fast;
        let rb = match region {
            Region::Buf(kb) => self.buf_regs(kb).0,
            _ => self.regions[2],
        };
        let lanes = self.lanes() as usize;
        let mut addr = [VReg(0); 8];
        for l in 0..lanes {
            let so = self.extract_i(rv, l as u8);
            let z = self.conv(Conv::IntToInt(U32), so);
            let sc = self.ibini(IOp::Shl, I64, z, 2);
            addr[l] = self.ibin(IOp::Add, I64, rb, sc);
        }
        for &c in &chunks {
            let mut rows = [VReg(0); 8];
            for l in 0..lanes {
                rows[l] = self.f.vreg(Cls::V128);
                self.emit(Inst::VLoad(rows[l], addr[l], 4 * c as i32));
            }
            let mut cols = [[VReg(0); 4]; 2];
            for q in 0..self.np {
                cols[q] = self.transpose4_regs([rows[4 * q], rows[4 * q + 1], rows[4 * q + 2], rows[4 * q + 3]]);
            }
            for &(w, v, _, cc) in &mine {
                if cc == c {
                    let d = self.vals[v.0 as usize];
                    for q in 0..self.np {
                        self.emit(Inst::Mov(d[q], cols[q][(w - c) as usize]));
                    }
                }
            }
        }
        self.term(Term::Jump(join));
        self.cur = slow;
        for &(w, v, _, _) in &mine {
            // word w = base + c of lane l, as the load itself reads it: a clamped gather
            let d = self.vals[v.0 as usize];
            let fp = self.p.vals[v.0 as usize] == crate::ir::Ty::F32;
            let (m, scls) = if fp { (Mem::F32, Cls::F32) } else { (W32, Cls::I) };
            let (base, extent) = match self.defs[v.0 as usize] {
                Some(Op::Load { base, extent, .. }) => (base, extent),
                _ => unreachable!("a load"),
            };
            let c = w - base;
            let ov = if c == 0 {
                rv
            } else {
                let kv = self.splat_const(c);
                self.vbin(VX::Add(VW::S), rv, kv)
            };
            match region {
                Region::Buf(kb) => self.buf_gather(d, kb, base, ov, m, scls),
                _ => {
                    let top = self.splat_const(extent.saturating_sub(1));
                    let idx = self.vbin(VX::MinU(VW::S), ov, top);
                    self.table_gather(d, base, extent, idx, 0);
                }
            }
        }
        self.term(Term::Jump(join));
        self.cur = join;
        for &(_, _, sid, _) in &mine {
            self.grouped.insert(sid);
        }
    }

    /// Consecutive stores of words `base .. base + n` (n >= 2) of a host buffer at one varying
    /// offset (a record's fields, an emitted record): when every lane's words lie inside the
    /// buffer (checked once, on the largest lane offset), each running lane writes its n words
    /// without clamps; else the stores one by one. Four consecutive words are a 4x4 transpose per
    /// four lanes (eight zips) and one 16-byte store per lane, the rest of the words one per
    /// lane. Words go in lane order (a word two lanes store ends as the later lane's) unless the
    /// lanes' records are disjoint (offsets `st >= n` words apart): then four words of all lanes
    /// at a time, which keeps four rows live instead of all of them. Returns the stores handled
    /// (0: none).
    fn store_group(&mut self, b: &[Stmt]) -> usize {
        let Some(Stmt::Store { region: Region::Buf(kb), base, off: Some(o), .. }) = b.first() else { return 0 };
        let (kb, base, o) = (*kb, *base, *o);
        if self.uniform(o) {
            return 0;
        }
        let mut n = 1;
        while n < b.len() {
            match &b[n] {
                Stmt::Store { region: Region::Buf(r), base: bn, off: Some(on), .. } if *r == kb && *on == o && *bn == base + n as u32 => n += 1,
                _ => break,
            }
        }
        let last = base as u64 + n as u64 - 1;
        if n < 2 || last >= 1 << 31 {
            return 0;
        }
        let (ptr, last_w) = self.buf_regs(kb);
        let ov = self.v(o);
        // every lane inside: max(zext offset) + last <= the buffer's last word (64 bits)
        let mx = self.max_across(ov);
        let end = self.ibini(IOp::Add, I64, mx, last as i64);
        let inside = self.icmp(Cond::Le, false, end, last_w);
        let (fast, slow, join) = (self.block_new(), self.block_new(), self.block_new());
        self.term(Term::Branch(inside, fast, slow));
        self.cur = fast;
        let vals: Vec<Val> = b[..n].iter().map(|st| if let Stmt::Store { val, .. } = st { *val } else { unreachable!() }).collect();
        // each lane's record address, and (not every lane running) whether it runs
        let lanes = self.lanes() as usize;
        let mut addr = [VReg(0); 8];
        let mut runs = [None; 8];
        for l in 0..lanes {
            let so = self.extract_i(ov, l as u8);
            let z = self.conv(Conv::IntToInt(U32), so);
            let idx = if base > 0 { self.ibini(IOp::Add, I64, z, base as i64) } else { z };
            let sc = self.ibini(IOp::Shl, I64, idx, 2);
            addr[l] = self.ibin(IOp::Add, I64, ptr, sc);
            if !self.full {
                runs[l] = Some(self.extract_i(self.vm, l as u8));
            }
        }
        let nq = n / 4;
        let disjoint = self.step[o.0 as usize].is_some_and(|st| st >= n as i64);
        if disjoint {
            for c in 0..nq {
                for q in 0..self.np {
                    let rows = self.transpose4(&vals[4 * c..4 * c + 4], q);
                    for k in 0..4 {
                        let l = 4 * q + k;
                        self.lane_run(runs[l], |x| x.emit(Inst::VStore(addr[l], 16 * c as i32, rows[k])));
                    }
                }
            }
            for l in 0..lanes {
                self.lane_run(runs[l], |x| x.lane_words(&vals, 4 * nq, addr[l], l as u8));
            }
        } else {
            let mut rows: Vec<[[VReg; 4]; 2]> = Vec::new();
            for c in 0..nq {
                let mut r = [[VReg(0); 4]; 2];
                for q in 0..self.np {
                    r[q] = self.transpose4(&vals[4 * c..4 * c + 4], q);
                }
                rows.push(r);
            }
            for l in 0..lanes {
                self.lane_run(runs[l], |x| {
                    for (c, r) in rows.iter().enumerate() {
                        x.emit(Inst::VStore(addr[l], 16 * c as i32, r[l / 4][l % 4]));
                    }
                    x.lane_words(&vals, 4 * nq, addr[l], l as u8);
                });
            }
        }
        self.term(Term::Jump(join));
        self.cur = slow;
        for st in &b[..n] {
            self.stmt(st);
        }
        self.term(Term::Jump(join));
        self.cur = join;
        n
    }

    /// `f` in a block of its own that runs when `run` (a lane's 0/1 running flag) is set; `f`
    /// straight in the current block when it is None (every lane runs).
    fn lane_run(&mut self, run: Option<VReg>, f: impl FnOnce(&mut Self)) {
        let Some(r) = run else { return f(self) };
        let (go, next) = (self.block_new(), self.block_new());
        self.term(Term::Branch(r, go, next));
        self.cur = go;
        f(self);
        self.term(Term::Jump(next));
        self.cur = next;
    }

    /// Lane `l` of values `vals[from..]` stored as the words `from..` of the record at `a`.
    fn lane_words(&mut self, vals: &[Val], from: usize, a: VReg, l: u8) {
        for (w, val) in vals.iter().enumerate().skip(from) {
            let fp = self.p.vals[val.0 as usize] == crate::ir::Ty::F32;
            let (m, scls) = if fp { (Mem::F32, Cls::F32) } else { (W32, Cls::I) };
            let sv = self.lane(*val, l, scls);
            self.emit(Inst::Store(m, a, 4 * w as i32, sv));
        }
    }

    /// The 4x4 transpose of part q of four values' vectors (row k = lane 4q + k of each): eight
    /// zips.
    fn transpose4(&mut self, vals: &[Val], q: usize) -> [VReg; 4] {
        let w: Vec<VReg> = vals.iter().map(|v| self.v(*v)[q]).collect();
        self.transpose4_regs([w[0], w[1], w[2], w[3]])
    }

    fn transpose4_regs(&mut self, w: [VReg; 4]) -> [VReg; 4] {
        let z = |s: &mut Self, op: VX, a: VReg, b: VReg| {
            let d = s.f.vreg(Cls::V128);
            s.emit(Inst::VX(op, d, a, b));
            d
        };
        // t0 = a0 b0 a1 b1, t1 = a2 b2 a3 b3, t2 = c0 d0 c1 d1, t3 = c2 d2 c3 d3
        let t0 = z(self, VX::ZipLo(VW::S), w[0], w[1]);
        let t1 = z(self, VX::ZipHi(VW::S), w[0], w[1]);
        let t2 = z(self, VX::ZipLo(VW::S), w[2], w[3]);
        let t3 = z(self, VX::ZipHi(VW::S), w[2], w[3]);
        [z(self, VX::ZipLo(VW::D), t0, t2), z(self, VX::ZipHi(VW::D), t0, t2), z(self, VX::ZipLo(VW::D), t1, t3), z(self, VX::ZipHi(VW::D), t1, t3)]
    }

    /// Branches to `skip` when no lane of the mask `m` is set, else to a new block.
    fn skip_if_none(&mut self, m: V, skip: u32) {
        let mut u = m[0];
        for q in 1..self.np {
            let t = self.f.vreg(Cls::V128);
            self.emit(Inst::VX(VX::Or, t, u, m[q]));
            u = t;
        }
        let any = self.f.vreg(Cls::I);
        self.emit(Inst::VAny(any, u));
        let go = self.block_new();
        self.term(Term::Branch(any, go, skip));
        self.cur = go;
    }

    fn stmt(&mut self, s: &Stmt) {
        match s {
            Stmt::Def(v, op) => self.def(*v, op),
            Stmt::Set(var, x) => {
                let d = self.vars[var.0 as usize];
                if *var == self.i {
                    let w = self.splat_const(self.lanes() as u32);
                    self.vbin_into(VX::Add(VW::S), d, d, w);
                    return;
                }
                let r = self.v(*x);
                if self.full {
                    self.mov_into(d, r);
                } else {
                    // only the running lanes take the new value
                    self.vsel_into(d, self.vm, r, d);
                }
            }
            Stmt::Store { region, base, extent, off, val } => self.store(*region, *base, *extent, *off, *val),
            Stmt::If(c, t, e) => {
                if !self.info.div_if.contains(&id(s)) {
                    let cv = match self.masks[c.0 as usize] {
                        Some(m) => m,
                        None => self.v(*c),
                    };
                    let c0 = self.extract_i(cv, 0);
                    let (bt, be, join) = (self.block_new(), self.block_new(), self.block_new());
                    self.term(Term::Branch(c0, bt, be));
                    let full = self.full;
                    self.cur = bt;
                    self.block(t);
                    self.term(Term::Jump(join));
                    self.cur = be;
                    self.full = full;
                    self.block(e);
                    self.term(Term::Jump(join));
                    self.cur = join;
                    self.full = full;
                    return;
                }
                // divergent: both sides under masks, then the union
                let cm = self.mask(*c);
                let else_m = self.vbin(VX::AndNot, self.vm, cm);
                self.vbin_into(VX::And, self.vm, self.vm, cm);
                let full = self.full;
                self.full = false;
                let skip_t = self.block_new();
                self.skip_if_none(self.vm, skip_t);
                self.block(t);
                self.term(Term::Jump(skip_t));
                self.cur = skip_t;
                let then_m = self.vnew();
                self.mov_into(then_m, self.vm);
                self.mov_into(self.vm, else_m);
                let skip_e = self.block_new();
                self.skip_if_none(self.vm, skip_e);
                self.full = false;
                self.block(e);
                self.term(Term::Jump(skip_e));
                self.cur = skip_e;
                self.vbin_into(VX::Or, self.vm, self.vm, then_m);
                self.full = full;
            }
            Stmt::Loop { cap, body } => {
                let masked = self.info.masked.contains(&id(s));
                let (top, exit, iter_end) = (self.block_new(), self.block_new(), self.block_new());
                let brk = self.vnew();
                let cont = self.vnew();
                if masked {
                    let z = self.splat_const(0);
                    self.mov_into(brk, z);
                }
                let full = self.full;
                let has_masked_exit = self.info.escapes.contains(&id(s));
                self.full = full && !masked && !has_masked_exit;
                let cnt = self.iconst(0);
                self.term(Term::Jump(top));
                self.cur = top;
                let capk = self.iconst(*cap as i64);
                let more = self.icmp(Cond::Lt, false, cnt, capk);
                let go = self.block_new();
                self.term(Term::Branch(more, go, exit));
                self.cur = go;
                if masked || has_masked_exit {
                    self.skip_if_none(self.vm, exit);
                }
                self.emit(Inst::IBinI(IOp::Add, I64, cnt, cnt, 1));
                if masked {
                    let z = self.splat_const(0);
                    self.mov_into(cont, z);
                }
                self.loops.push(Loop { masked, top, exit, iter_end, brk, cont });
                self.block(body);
                self.loops.pop();
                self.term(Term::Jump(iter_end));
                self.cur = iter_end;
                if masked {
                    self.vbin_into(VX::Or, self.vm, self.vm, cont);
                    // a continued lane may be the only one left running
                    self.full = false;
                }
                self.term(Term::Jump(top));
                self.cur = exit;
                if masked {
                    self.vbin_into(VX::Or, self.vm, self.vm, brk);
                }
                self.full = full;
            }
            Stmt::Break(d) | Stmt::Continue(d) => {
                let is_break = matches!(s, Stmt::Break(_));
                let t = self.loops.len() - 1 - *d as usize;
                let (masked, top, exit, iter_end, brk, cont) = {
                    let l = &self.loops[t];
                    (l.masked, l.top, l.exit, l.iter_end, l.brk, l.cont)
                };
                if masked {
                    let slot = if is_break { brk } else { cont };
                    self.vbin_into(VX::Or, slot, slot, self.vm);
                }
                if self.info.jump.contains(&id(s)) {
                    // every running lane leaves together
                    let to = match (is_break, masked) {
                        (_, true) => {
                            if is_break {
                                let z = self.splat_const(0);
                                self.mov_into(self.vm, z);
                            }
                            iter_end
                        }
                        (true, false) => exit,
                        (false, false) => top,
                    };
                    self.term(Term::Jump(to));
                    self.cur = self.block_new();
                } else {
                    // these lanes stop until their loop's exit or next iteration
                    let z = self.splat_const(0);
                    self.mov_into(self.vm, z);
                    self.full = false;
                }
            }
            Stmt::Out { .. } | Stmt::CallHost { .. } => unreachable!("declined"),
            Stmt::Call { .. } => unreachable!("flattened"),
        }
    }

    // -- ops ---------------------------------------------------------------------

    fn def(&mut self, v: Val, op: &Op) {
        let bits = match *op {
            Op::ConstF(x) => Some(x.to_bits()),
            Op::ConstI(x) => Some(x as u32),
            Op::ConstB(x) => Some(x as u32),
            _ => None,
        };
        if let Some(b) = bits {
            self.consts[v.0 as usize] = Some(b);
            return;
        }
        let d = self.vals[v.0 as usize];
        let lanes = self.lanes();
        match *op {
            Op::Get(var) => {
                let s = self.vars[var.0 as usize];
                self.mov_into(d, s);
            }
            Op::Un(u, a) => {
                let a = self.v(a);
                let vu = match u {
                    Un::NegF => Some(VX::FNeg(VW::S)),
                    Un::AbsF => Some(VX::FAbs(VW::S)),
                    Un::SqrtF => Some(VX::FSqrt(VW::S, NanBits::Host)),
                    Un::FloorF => Some(VX::FRound(VW::S, RoundTo::Floor)),
                    Un::CeilF => Some(VX::FRound(VW::S, RoundTo::Ceil)),
                    Un::TruncF => Some(VX::FRound(VW::S, RoundTo::Trunc)),
                    Un::RoundF => Some(VX::FRound(VW::S, RoundTo::Away)),
                    Un::F2I => Some(VX::FToI(VW::S)),
                    Un::I2F => Some(VX::IToF(VW::S)),
                    Un::NegI => Some(VX::Neg(VW::S)),
                    _ => None,
                };
                match (vu, u) {
                    (Some(o), _) => self.vbin_into(o, d, a, a),
                    (None, Un::BitsFI | Un::BitsIF) => self.mov_into(d, a),
                    (None, Un::NotB) => {
                        let z = self.splat_const(0);
                        let m = self.vbin(VX::ICmp(Cond::Eq, VW::S), a, z);
                        self.masks[v.0 as usize] = Some(m);
                        let one = self.splat_const(1);
                        self.vbin_into(VX::And, d, m, one);
                    }
                    _ => unreachable!("f64 is declined"),
                }
            }
            Op::Bin(b, x, y) => {
                let (xv, yv) = (self.v(x), self.v(y));
                let vop = match b {
                    Bin::AddF => Some(VX::FAdd(VW::S, NanBits::Host)),
                    Bin::SubF => Some(VX::FSub(VW::S, NanBits::Host)),
                    Bin::MulF => Some(VX::FMul(VW::S, NanBits::Host)),
                    Bin::DivF => Some(VX::FDiv(VW::S, NanBits::Host)),
                    Bin::MinF => Some(VX::FMinLt(VW::S)),
                    Bin::MaxF => Some(VX::FMaxGt(VW::S)),
                    Bin::AddI => Some(VX::Add(VW::S)),
                    Bin::SubI => Some(VX::Sub(VW::S)),
                    Bin::MulI => Some(VX::MulLo(VW::S)),
                    Bin::AndI | Bin::AndB => Some(VX::And),
                    Bin::OrI | Bin::OrB => Some(VX::Or),
                    Bin::XorI => Some(VX::Xor),
                    _ => None,
                };
                if let Some(o) = vop {
                    self.vbin_into(o, d, xv, yv);
                    if matches!(b, Bin::AndB | Bin::OrB) {
                        if let (Some(mx), Some(my)) = (self.masks[x.0 as usize], self.masks[y.0 as usize]) {
                            self.masks[v.0 as usize] = Some(self.vbin(o, mx, my));
                        }
                    }
                    return;
                }
                match b {
                    Bin::ShlI | Bin::ShrI | Bin::ShrUI if self.consts[y.0 as usize].is_some() => {
                        let k = (self.consts[y.0 as usize].unwrap() & 31) as u8;
                        let o = match b {
                            Bin::ShlI => VX::ShlI(VW::S, k),
                            Bin::ShrI => VX::ShrSI(VW::S, k),
                            _ => VX::ShrUI(VW::S, k),
                        };
                        self.vbin_into(o, d, xv, xv);
                    }
                    Bin::ShlI | Bin::ShrI | Bin::ShrUI => {
                        // the amount masked to 31 (as AIR), unless proven below 32 already
                        let k = if self.bounds[y.0 as usize].is_some_and(|b| b < 32) {
                            yv
                        } else {
                            let m31 = self.splat_const(31);
                            self.vbin(VX::And, yv, m31)
                        };
                        let o = match b {
                            Bin::ShlI => VX::ShlV(VW::S),
                            Bin::ShrI => VX::ShrSV(VW::S),
                            _ => VX::ShrUV(VW::S),
                        };
                        self.vbin_into(o, d, xv, k);
                    }
                    Bin::DivI | Bin::RemI if self.consts[y.0 as usize].is_some_and(|c| c > 1 && c < 1 << 31 && c.is_power_of_two()) => {
                        // by 2^k, lane-wise: q = (x + ((x >> 31) >>> (32 - k))) >> k, r = x - (q << k)
                        let c = self.consts[y.0 as usize].unwrap();
                        let k = c.trailing_zeros();
                        let sg = self.vshift(VX::ShrSI, xv, 31);
                        let bias = self.vshift(VX::ShrUI, sg, (32 - k) as u8);
                        let t = self.vbin(VX::Add(VW::S), xv, bias);
                        if b == Bin::DivI {
                            self.vbin_into(VX::ShrSI(VW::S, k as u8), d, t, t);
                        } else {
                            let q = self.vshift(VX::ShrSI, t, k as u8);
                            let qk = self.vshift(VX::ShlI, q, k as u8);
                            self.vbin_into(VX::Sub(VW::S), d, xv, qk);
                        }
                    }
                    Bin::DivI | Bin::RemI if self.consts[y.0 as usize].is_some() => {
                        let c = self.consts[y.0 as usize].unwrap() as i32;
                        let t = if self.np > 1 { self.vnew() } else { d };
                        for l in 0..lanes {
                            let a = self.extract_i(xv, l);
                            let cur = self.cur;
                            let r = crate::native::Emitter { f: &mut self.f, cur }.div_const(a, c, b == Bin::RemI);
                            self.put_lane(t, l, r);
                        }
                        if self.np > 1 {
                            self.mov_into(d, t);
                        }
                    }
                    Bin::DivI | Bin::RemI => {
                        // per lane: x / 0 = 0, x % 0 = x (scalar code's sequence)
                        let t = if self.np > 1 { self.vnew() } else { d };
                        for l in 0..lanes {
                            let a = self.extract_i(xv, l);
                            let c = self.extract_i(yv, l);
                            let z = self.icmpi_eq0(c);
                            let one = self.iconst(1);
                            let safe = self.sel(z, one, c);
                            let q = self.ibin(if b == Bin::DivI { IOp::Div } else { IOp::Rem }, I32, a, safe);
                            let zc = if b == Bin::DivI { self.iconst(0) } else { a };
                            let r = self.sel(z, zc, q);
                            self.put_lane(t, l, r);
                        }
                        if self.np > 1 {
                            self.mov_into(d, t);
                        }
                    }
                    _ => unreachable!("f64 is declined"),
                }
            }
            Op::CmpF(c, x, y) | Op::CmpI(c, x, y) => {
                let (xv, yv) = (self.v(x), self.v(y));
                let cc = match c {
                    Cmp::Lt => Cond::Lt,
                    Cmp::Le => Cond::Le,
                    Cmp::Gt => Cond::Gt,
                    Cmp::Ge => Cond::Ge,
                    Cmp::Eq => Cond::Eq,
                    Cmp::Ne => Cond::Ne,
                };
                let o = if matches!(op, Op::CmpF(..)) { VX::FCmp(cc, VW::S) } else { VX::ICmp(cc, VW::S) };
                let m = self.vbin(o, xv, yv);
                self.masks[v.0 as usize] = Some(m);
                let w = self.mask_to_word(m);
                self.mov_into(d, w);
            }
            Op::Sel(c, x, y) => {
                let m = self.mask(c);
                let (xv, yv) = (self.v(x), self.v(y));
                self.vsel_into(d, m, xv, yv);
            }
            Op::Fma(Fma::MulAddI, a, b, c) => {
                let (av, bv, cv) = (self.v(a), self.v(b), self.v(c));
                let m = self.vbin(VX::MulLo(VW::S), av, bv);
                self.vbin_into(VX::Add(VW::S), d, m, cv);
            }
            Op::Fma(k, a, b, c) => {
                let (np, na) = match k {
                    Fma::Add => (false, false),
                    Fma::SubFrom => (true, false),
                    _ => (false, true),
                };
                let (av, bv, cv) = (self.v(a), self.v(b), self.v(c));
                for q in 0..self.np {
                    self.emit(Inst::VFma(np, na, d[q], av[q], bv[q], cv[q]));
                }
            }
            Op::Wrap(x, len) => {
                let xv = self.v(x);
                if len.is_power_of_two() {
                    let mk = self.splat_const(len - 1);
                    self.vbin_into(VX::And, d, xv, mk);
                } else {
                    let lk = len as i32 as i64;
                    let t = if self.np > 1 { self.vnew() } else { d };
                    for l in 0..lanes {
                        let a = self.extract_i(xv, l);
                        let cur = self.cur;
                        let r = crate::native::Emitter { f: &mut self.f, cur }.div_const(a, len as i32, true);
                        let neg = self.icmpi_lt0(r);
                        let up = self.ibini(IOp::Add, I32, r, lk);
                        let w = self.sel(neg, up, r);
                        self.put_lane(t, l, w);
                    }
                    if self.np > 1 {
                        self.mov_into(d, t);
                    }
                }
            }
            Op::Load { region, base, extent, off } => {
                let fp = self.p.vals[v.0 as usize] == crate::ir::Ty::F32;
                self.load(d, region, base, extent, off, fp);
            }
            Op::FrameCount => self.splat_into(d, self.n_i32),
            Op::BufLen(k) => {
                let s = self.f.vreg(Cls::I);
                self.emit(Inst::Load(W32, s, self.regions[3], k as i32 * 16 + 8));
                self.splat_into(d, s);
            }
            Op::ConstF(_) | Op::ConstI(_) | Op::ConstB(_) => {}
            Op::ConstD(_) | Op::CmpD(..) | Op::In { .. } => unreachable!("declined"),
        }
    }

    fn icmpi_eq0(&mut self, a: VReg) -> VReg {
        let z = self.iconst(0);
        self.icmp(Cond::Eq, true, a, z)
    }

    fn icmpi_lt0(&mut self, a: VReg) -> VReg {
        let z = self.iconst(0);
        self.icmp(Cond::Lt, true, a, z)
    }
}

//! Four elements per iteration through the shared back end: a kernel's element loop
//! lowered to RIR's 128-bit vectors (masked SPMD, [`crate::spmd`]'s analysis -- the same
//! as NEON x4 and wasm SIMD128) and emitted by the shared x64 (SSE4.1/AVX2) or arm64 (NEON)
//! code generator. Bit-identical to the scalar code and the interpreter.
//!
//! Every AIR value is a V128 vreg, one element per lane, holding exactly the bits the
//! scalar code holds for that element (bools are 0/1 words). Uniform control flow is real
//! branches on lane 0; divergent ifs run both sides under the execution mask `vm` (a
//! vreg), masked loops keep break/continue masks, variables set while lanes are off are
//! blended, stores write the running lanes in lane order. Integer division, non-power-
//! of-two wraps and varying-address memory run per lane with the scalar sequences.
//!
//! The element loop is the shape `lower_kernel` emits ([`crate::spmd::shape`]): its counter
//! holds `[i, i+1, i+2, i+3]`, steps by 4, and its `i >= n` exit is taken on lane 0 (the
//! caller runs `n` a multiple of 4; the tail runs on scalar code). [`compile`] declines
//! (None) whatever [`crate::spmd::supported`] rejects.

use crate::ir::{Bin, Block, Cmp, Fma, Op, Program, Region, Stmt, Un, Val, Var};
use crate::native::Code;
use crate::spmd::{analyse, id, shape, steps, supported, Info};
use makepad_codegen::rir::{self, Cls, Cond, Conv, Func, IOp, Inst, IntTy, Mem, Term, VOp, VReg, VUn};


const I32: IntTy = IntTy { bits: 32, signed: true };
const U32: IntTy = IntTy { bits: 32, signed: false };
const I64: IntTy = rir::I64;
const W32: Mem = Mem::Int(4, true);
const PTR: Mem = Mem::Int(8, false);

/// What the vector code needs from the CPU (x64: SSE4.1, AVX2 per-lane shifts, FMA3).
fn cpu_ok() -> bool {
    #[cfg(target_arch = "x86_64")]
    {
        std::is_x86_feature_detected!("sse4.1") && std::is_x86_feature_detected!("avx2") && std::is_x86_feature_detected!("fma")
    }
    #[cfg(not(target_arch = "x86_64"))]
    true
}

/// Compiles a kernel's element loop to four-wide vector code, or declines.
pub fn compile(p: &Program) -> Option<Code> {
    if !cpu_ok() {
        return None;
    }
    let mut f = rir_of(p)?;
    crate::native::optimise_for_codegen(&mut f);
    if f.verify().is_err() {
        return None;
    }
    crate::native::emit(&f)
}

/// The vector RIR of a kernel (None when declined), before optimisation.
pub fn rir_of(p: &Program) -> Option<Func> {
    let flat = crate::ir::flat(p);
    let p = &*flat;
    let sh = shape(p)?;
    let Stmt::Loop { body, .. } = sh.element else { return None };
    if !supported(p, body) {
        return None;
    }
    let info = analyse(p, &sh)?;
    let (step, _consts) = steps(p, &info, sh.i);
    let mut f = Func::new("splash_kernel_x4".to_string(), 0);
    let entry = f.block();
    let mut regions = [VReg(0); 4];
    for r in regions.iter_mut() {
        *r = f.vreg(Cls::I);
        f.params.push(*r);
    }
    let n_raw = f.vreg(Cls::I);
    f.params.push(n_raw);
    let vals: Vec<VReg> = p.vals.iter().map(|_| f.vreg(Cls::V128)).collect();
    let vars: Vec<VReg> = p.vars.iter().map(|_| f.vreg(Cls::V128)).collect();
    let frame = if p.frame_words > 0 { Some(f.slot(p.frame_words * 16, 16)) } else { None };
    let vm = f.vreg(Cls::V128);
    let mut lx = Vx {
        p,
        f,
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
    };
    let nu = lx.conv(Conv::IntToInt(U32), n_raw);
    lx.n_i32 = lx.conv(Conv::IntToInt(I32), nu);
    let ones = lx.iconst(-1);
    lx.emit(Inst::VSplat(vm, ones));
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
                // the lanes' element offsets [0, 1, 2, 3]
                let d = lx.vars[var.0 as usize];
                let z = lx.iconst(0);
                lx.emit(Inst::VSplat(d, z));
                for l in 1..4u8 {
                    let k = lx.iconst(l as i64);
                    lx.emit(Inst::VInsert(d, d, l, k));
                }
                continue;
            }
        }
        lx.stmt(s);
    }
    lx.stmt(sh.element);
    if let Some((var, word)) = sh.post {
        // the run's counted work: the lanes' counts summed into its ctx word
        let v = lx.vars[var.0 as usize];
        let mut sum = lx.extract_i(v, 0);
        for l in 1..4u8 {
            let x = lx.extract_i(v, l);
            sum = lx.ibin(IOp::Add, I32, sum, x);
        }
        lx.emit(Inst::Store(W32, lx.regions[0], 4 * word as i32, sum));
    }
    lx.term(Term::Ret(Vec::new()));
    Some(lx.f)
}

struct Loop {
    masked: bool,
    top: u32,
    exit: u32,
    iter_end: u32,
    brk: VReg,
    cont: VReg,
}

struct Vx<'a> {
    p: &'a Program,
    f: Func,
    cur: u32,
    vals: Vec<VReg>,
    vars: Vec<VReg>,
    regions: [VReg; 4],
    n_i32: VReg,
    frame: Option<u32>,
    loops: Vec<Loop>,
    bounds: Vec<Option<u32>>,
    info: Info,
    /// per value: lanes are `lane0 + l * step`
    step: Vec<Option<i64>>,
    /// the execution mask (lane masks)
    vm: VReg,
    /// statically every lane runs
    full: bool,
    i: Var,
    /// constant values (materialised at each use)
    consts: Vec<Option<u32>>,
    /// per compare / logic value: its lane mask (all-ones / zero lanes) beside its 0/1 word
    masks: Vec<Option<VReg>>,
    /// per host buffer: (base pointer, last word index), loaded once at entry
    bufs: Vec<(VReg, VReg)>,
}

impl<'a> Vx<'a> {
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

    fn splat_const(&mut self, bits: u32) -> VReg {
        let d = self.f.vreg(Cls::V128);
        self.emit(Inst::VConst(d, bits));
        d
    }

    fn vbin(&mut self, op: VOp, a: VReg, b: VReg) -> VReg {
        let d = self.f.vreg(Cls::V128);
        self.emit(Inst::VBin(op, d, a, b));
        d
    }

    fn vshift(&mut self, op: VOp, a: VReg, k: u8) -> VReg {
        let d = self.f.vreg(Cls::V128);
        self.emit(Inst::VShift(op, d, a, k));
        d
    }

    fn extract_i(&mut self, v: VReg, lane: u8) -> VReg {
        let d = self.f.vreg(Cls::I);
        self.emit(Inst::VExtract(d, v, lane));
        d
    }

    /// The vector of value `x` (constants splatted at each use).
    fn v(&mut self, x: Val) -> VReg {
        match self.consts[x.0 as usize] {
            Some(bits) => self.splat_const(bits),
            None => self.vals[x.0 as usize],
        }
    }

    fn uniform(&self, x: Val) -> bool {
        !self.info.vval[x.0 as usize]
    }

    /// A lane mask -> the scalar code's 0/1 word.
    fn mask_to_word(&mut self, m: VReg) -> VReg {
        let one = self.splat_const(1);
        self.vbin(VOp::And, m, one)
    }

    /// The lane mask of `x != 0` (the compare's own mask when `x` is one).
    fn mask(&mut self, x: Val) -> VReg {
        if let Some(m) = self.masks[x.0 as usize] {
            return m;
        }
        let w = self.v(x);
        self.word_to_mask(w)
    }

    /// A word (any bits) -> lane mask of `word != 0`.
    fn word_to_mask(&mut self, w: VReg) -> VReg {
        let z = self.splat_const(0);
        self.vbin(VOp::ICmp(Cond::Ne), w, z)
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

    /// Address of word `base + clamp(o)` of the per-lane frame, lane `lane`: word w of lane l
    /// lives at frame + 16 w + 4 l.
    fn frame_addr(&mut self, base: u32, extent: u32, o: Option<(Val, VReg)>, lane: u8) -> (VReg, i32) {
        let fr = self.f.vreg(Cls::I);
        self.emit(Inst::SlotAddr(fr, self.frame.unwrap_or(0)));
        match o {
            None => (fr, (base * 16 + 4 * lane as u32) as i32),
            Some((ov, so)) => {
                let idx = self.clamp(ov, so, extent);
                let sc = self.ibini(IOp::Shl, I64, idx, 4);
                let a = self.ibin(IOp::Add, I64, fr, sc);
                (a, (base * 16 + 4 * lane as u32) as i32)
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

    fn load(&mut self, d: VReg, region: Region, base: u32, extent: u32, off: Option<Val>, fp: bool) {
        let m = if fp { Mem::F32 } else { W32 };
        let scls = if fp { Cls::F32 } else { Cls::I };
        let uniform = off.is_none_or(|o| self.uniform(o));
        if region == Region::Frame && uniform {
            let o = self.lane_off(off, 0);
            let (a, k) = self.frame_addr(base, extent, o, 0);
            self.emit(Inst::VLoad(d, a, k));
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
            self.emit(Inst::VSplat(d, s));
            return;
        }
        let o = off.unwrap();
        if let (Region::Buf(k), Some(1)) = (region, self.step[o.0 as usize]) {
            // four consecutive words: one vector load when all lie inside the buffer
            let (ptr, last) = self.buf_regs(k);
            let ov = self.v(o);
            let s0 = self.extract_i(ov, 0);
            let z = self.conv(Conv::IntToInt(U32), s0);
            let span = base as i64 + 3;
            let end = self.ibini(IOp::Add, I64, z, span);
            let inside = self.icmp(Cond::Le, false, end, last);
            let (fast, slow, join) = (self.block_new(), self.block_new(), self.block_new());
            self.term(Term::Branch(inside, fast, slow));
            self.cur = fast;
            let idx = self.ibini(IOp::Add, I64, z, base as i64);
            let sc = self.ibini(IOp::Shl, I64, idx, 2);
            let a = self.ibin(IOp::Add, I64, ptr, sc);
            self.emit(Inst::VLoad(d, a, 0));
            self.term(Term::Jump(join));
            self.cur = slow;
            for l in 0..4u8 {
                let so = self.extract_i(ov, l);
                let a = self.buf_addr(ptr, last, base, Some(so));
                let s = self.f.vreg(scls);
                self.emit(Inst::Load(m, s, a, 0));
                self.emit(Inst::VInsert(d, d, l, s));
            }
            self.term(Term::Jump(join));
            self.cur = join;
            return;
        }
        // a gather of clamped word indices (one instruction on x64 AVX2)
        let o = off.unwrap();
        let ov = self.v(o);
        match region {
            Region::Buf(k) => {
                // index = min(base + o, last) = base + umin(o, last - base) when last >= base
                let (ptr, last) = self.buf_regs(k);
                let bk = self.iconst(base as i64);
                let ok = self.icmp(Cond::Ge, true, last, bk);
                let (gath, low, join) = (self.block_new(), self.block_new(), self.block_new());
                self.term(Term::Branch(ok, gath, low));
                self.cur = gath;
                let lim = self.ibini(IOp::Sub, I64, last, base as i64);
                let limv = self.f.vreg(Cls::V128);
                self.emit(Inst::VSplat(limv, lim));
                let idx = self.vbin(VOp::UMin, ov, limv);
                let b0 = self.ibini(IOp::Add, I64, ptr, 4 * base as i64);
                self.emit(Inst::VGather(d, b0, idx));
                self.term(Term::Jump(join));
                self.cur = low;
                // every lane clamps to the last word
                let a = self.buf_addr(ptr, last, base, None);
                let s = self.f.vreg(scls);
                self.emit(Inst::Load(m, s, a, 0));
                self.emit(Inst::VSplat(d, s));
                self.term(Term::Jump(join));
                self.cur = join;
            }
            Region::Frame => {
                // word w of lane l: frame + 16 w + 4 l -> gather index 4 clamp(o) + l
                let idx = self.vclamp(o, ov, extent);
                let i4 = self.vshift(VOp::Shl, idx, 2);
                let ids = self.lane_ids();
                let il = self.vbin(VOp::IAdd, i4, ids);
                let fr = self.f.vreg(Cls::I);
                self.emit(Inst::SlotAddr(fr, self.frame.unwrap_or(0)));
                let b0 = self.ibini(IOp::Add, I64, fr, 16 * base as i64);
                self.emit(Inst::VGather(d, b0, il));
            }
            _ => {
                let rb = match region {
                    Region::Ctx => self.regions[0],
                    Region::State => self.regions[1],
                    _ => self.regions[2],
                };
                let idx = self.vclamp(o, ov, extent);
                let b0 = self.ibini(IOp::Add, I64, rb, 4 * base as i64);
                self.emit(Inst::VGather(d, b0, idx));
            }
        }
        let _ = (m, scls);
    }

    /// Lanes `min(o as u32, extent - 1)` unless the offset is proven in range.
    fn vclamp(&mut self, o: Val, ov: VReg, extent: u32) -> VReg {
        if self.bounds[o.0 as usize].is_some_and(|b| b < extent) {
            return ov;
        }
        let top = self.splat_const(extent.saturating_sub(1));
        self.vbin(VOp::UMin, ov, top)
    }

    /// The vector [0, 1, 2, 3].
    fn lane_ids(&mut self) -> VReg {
        let d = self.f.vreg(Cls::V128);
        let z = self.iconst(0);
        self.emit(Inst::VSplat(d, z));
        for l in 1..4u8 {
            let k = self.iconst(l as i64);
            self.emit(Inst::VInsert(d, d, l, k));
        }
        d
    }

    #[allow(dead_code)]
    fn load_per_lane(&mut self, d: VReg, region: Region, base: u32, extent: u32, off: Option<Val>, m: Mem, scls: Cls) {
        let buf = if let Region::Buf(k) = region { Some(self.buf_regs(k)) } else { None };
        for l in 0..4u8 {
            let lo = self.lane_off(off, l);
            let s = self.f.vreg(scls);
            match region {
                Region::Buf(_) => {
                    let (ptr, last) = buf.unwrap();
                    let a = self.buf_addr(ptr, last, base, lo.map(|x| x.1));
                    self.emit(Inst::Load(m, s, a, 0));
                }
                Region::Frame => {
                    let (a, k) = self.frame_addr(base, extent, lo, l);
                    self.emit(Inst::Load(m, s, a, k));
                }
                _ => {
                    let (a, k) = self.region_addr(region, base, extent, lo);
                    self.emit(Inst::Load(m, s, a, k));
                }
            }
            self.emit(Inst::VInsert(d, d, l, s));
        }
    }

    /// Stores the running lanes of vector `v` to the 16 bytes at [a + k].
    fn store_q(&mut self, a: VReg, k: i32, v: VReg) {
        if self.full {
            self.emit(Inst::VStore(a, k, v));
        } else {
            let old = self.f.vreg(Cls::V128);
            self.emit(Inst::VLoad(old, a, k));
            let new = self.f.vreg(Cls::V128);
            self.emit(Inst::VSel(new, self.vm, v, old));
            self.emit(Inst::VStore(a, k, new));
        }
    }

    fn store(&mut self, region: Region, base: u32, extent: u32, off: Option<Val>, val: Val) {
        let fp = self.p.vals[val.0 as usize] == crate::ir::Ty::F32;
        let m = if fp { Mem::F32 } else { W32 };
        let scls = if fp { Cls::F32 } else { Cls::I };
        let rv = self.v(val);
        let uniform = off.is_none_or(|o| self.uniform(o));
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
                    let end = self.ibini(IOp::Add, I64, z, base as i64 + 3);
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
                    // records `st` words apart, all four inside, every lane running: a store
                    // per lane in lane order, no clamps (the last lane decides: lanes increase)
                    let (ptr, last) = buf.unwrap();
                    let ov = self.v(o);
                    let s0 = self.extract_i(ov, 0);
                    let z = self.conv(Conv::IntToInt(U32), s0);
                    let end = self.ibini(IOp::Add, I64, z, base as i64 + 3 * st);
                    let inside = self.icmp(Cond::Le, false, end, last);
                    let (fast, slow) = (self.block_new(), self.block_new());
                    self.term(Term::Branch(inside, fast, slow));
                    self.cur = fast;
                    let idx = self.ibini(IOp::Add, I64, z, base as i64);
                    let sc = self.ibini(IOp::Shl, I64, idx, 2);
                    let a = self.ibin(IOp::Add, I64, ptr, sc);
                    for l in 0..4u8 {
                        let sv = self.f.vreg(scls);
                        self.emit(Inst::VExtract(sv, rv, l));
                        self.emit(Inst::Store(m, a, (4 * st * l as i64) as i32, sv));
                    }
                    self.term(Term::Jump(join));
                    self.cur = slow;
                }
            }
        }
        // per lane, in lane order: a word two lanes store ends as the later element's
        for l in 0..4u8 {
            let next = self.block_new();
            if !self.full {
                let ml = self.extract_i(self.vm, l);
                let run = self.block_new();
                self.term(Term::Branch(ml, run, next));
                self.cur = run;
            }
            let s = self.f.vreg(scls);
            self.emit(Inst::VExtract(s, rv, l));
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
        for s in b {
            self.stmt(s);
            if self.info.escapes.contains(&id(s)) {
                self.full = false;
            }
        }
    }

    /// Branches to `skip` when no lane of the mask `m` is set, else to a new block.
    fn skip_if_none(&mut self, m: VReg, skip: u32) {
        let any = self.f.vreg(Cls::I);
        self.emit(Inst::VAny(any, m));
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
                    let four = self.splat_const(4);
                    let iv = self.vars[var.0 as usize];
                    self.emit(Inst::VBin(VOp::IAdd, d, iv, four));
                    return;
                }
                let r = self.v(*x);
                if self.full {
                    self.emit(Inst::Mov(d, r));
                } else {
                    // only the running lanes take the new value
                    self.emit(Inst::VSel(d, self.vm, r, d));
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
                let else_m = self.vbin(VOp::AndNot, self.vm, cm);
                self.emit(Inst::VBin(VOp::And, self.vm, self.vm, cm));
                let full = self.full;
                self.full = false;
                let skip_t = self.block_new();
                self.skip_if_none(self.vm, skip_t);
                self.block(t);
                self.term(Term::Jump(skip_t));
                self.cur = skip_t;
                let then_m = self.f.vreg(Cls::V128);
                self.emit(Inst::Mov(then_m, self.vm));
                self.emit(Inst::Mov(self.vm, else_m));
                let skip_e = self.block_new();
                self.skip_if_none(self.vm, skip_e);
                self.full = false;
                self.block(e);
                self.term(Term::Jump(skip_e));
                self.cur = skip_e;
                self.emit(Inst::VBin(VOp::Or, self.vm, self.vm, then_m));
                self.full = full;
            }
            Stmt::Loop { cap, body } => {
                let masked = self.info.masked.contains(&id(s));
                let (top, exit, iter_end) = (self.block_new(), self.block_new(), self.block_new());
                let brk = self.f.vreg(Cls::V128);
                let cont = self.f.vreg(Cls::V128);
                if masked {
                    let z = self.splat_const(0);
                    self.emit(Inst::Mov(brk, z));
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
                    self.emit(Inst::Mov(cont, z));
                }
                self.loops.push(Loop { masked, top, exit, iter_end, brk, cont });
                self.block(body);
                self.loops.pop();
                self.term(Term::Jump(iter_end));
                self.cur = iter_end;
                if masked {
                    self.emit(Inst::VBin(VOp::Or, self.vm, self.vm, cont));
                    // a continued lane may be the only one left running
                    self.full = false;
                }
                self.term(Term::Jump(top));
                self.cur = exit;
                if masked {
                    self.emit(Inst::VBin(VOp::Or, self.vm, self.vm, brk));
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
                    self.emit(Inst::VBin(VOp::Or, slot, slot, self.vm));
                }
                if self.info.jump.contains(&id(s)) {
                    // every running lane leaves together
                    let to = match (is_break, masked) {
                        (_, true) => {
                            if is_break {
                                let z = self.splat_const(0);
                                self.emit(Inst::Mov(self.vm, z));
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
                    self.emit(Inst::Mov(self.vm, z));
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
        match *op {
            Op::Get(var) => {
                let s = self.vars[var.0 as usize];
                self.emit(Inst::Mov(d, s));
            }
            Op::Un(u, a) => {
                let a = self.v(a);
                let vu = match u {
                    Un::NegF => Some(VUn::FNeg),
                    Un::AbsF => Some(VUn::FAbs),
                    Un::SqrtF => Some(VUn::FSqrt),
                    Un::FloorF => Some(VUn::FFloor),
                    Un::CeilF => Some(VUn::FCeil),
                    Un::TruncF => Some(VUn::FTrunc),
                    Un::RoundF => Some(VUn::FRoundAway),
                    Un::F2I => Some(VUn::FToI),
                    Un::I2F => Some(VUn::IToF),
                    Un::NegI => Some(VUn::INeg),
                    _ => None,
                };
                match (vu, u) {
                    (Some(o), _) => self.emit(Inst::VUn(o, d, a)),
                    (None, Un::BitsFI | Un::BitsIF) => self.emit(Inst::Mov(d, a)),
                    (None, Un::NotB) => {
                        let z = self.splat_const(0);
                        let m = self.vbin(VOp::ICmp(Cond::Eq), a, z);
                        self.masks[v.0 as usize] = Some(m);
                        let one = self.splat_const(1);
                        self.emit(Inst::VBin(VOp::And, d, m, one));
                    }
                    _ => unreachable!("f64 is declined"),
                }
            }
            Op::Bin(b, x, y) => {
                let (xv, yv) = (self.v(x), self.v(y));
                let vop = match b {
                    Bin::AddF => Some(VOp::FAdd),
                    Bin::SubF => Some(VOp::FSub),
                    Bin::MulF => Some(VOp::FMul),
                    Bin::DivF => Some(VOp::FDiv),
                    Bin::MinF => Some(VOp::FMinLt),
                    Bin::MaxF => Some(VOp::FMaxGt),
                    Bin::AddI => Some(VOp::IAdd),
                    Bin::SubI => Some(VOp::ISub),
                    Bin::MulI => Some(VOp::IMul),
                    Bin::AndI | Bin::AndB => Some(VOp::And),
                    Bin::OrI | Bin::OrB => Some(VOp::Or),
                    Bin::XorI => Some(VOp::Xor),
                    _ => None,
                };
                if let Some(o) = vop {
                    self.emit(Inst::VBin(o, d, xv, yv));
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
                            Bin::ShlI => VOp::Shl,
                            Bin::ShrI => VOp::ShrS,
                            _ => VOp::ShrU,
                        };
                        self.emit(Inst::VShift(o, d, xv, k));
                    }
                    Bin::ShlI | Bin::ShrI | Bin::ShrUI => {
                        let m31 = self.splat_const(31);
                        let k = self.vbin(VOp::And, yv, m31);
                        let o = match b {
                            Bin::ShlI => VOp::Shl,
                            Bin::ShrI => VOp::ShrS,
                            _ => VOp::ShrU,
                        };
                        self.emit(Inst::VBin(o, d, xv, k));
                    }
                    Bin::DivI | Bin::RemI if self.consts[y.0 as usize].is_some_and(|c| c > 1 && c < 1 << 31 && c.is_power_of_two()) => {
                        // by 2^k, lane-wise: q = (x + ((x >> 31) >>> (32 - k))) >> k, r = x - (q << k)
                        let c = self.consts[y.0 as usize].unwrap();
                        let k = c.trailing_zeros();
                        let sg = self.vshift(VOp::ShrS, xv, 31);
                        let bias = self.vshift(VOp::ShrU, sg, (32 - k) as u8);
                        let t = self.vbin(VOp::IAdd, xv, bias);
                        if b == Bin::DivI {
                            self.emit(Inst::VShift(VOp::ShrS, d, t, k as u8));
                        } else {
                            let q = self.vshift(VOp::ShrS, t, k as u8);
                            let qk = self.vshift(VOp::Shl, q, k as u8);
                            self.emit(Inst::VBin(VOp::ISub, d, xv, qk));
                        }
                    }
                    Bin::DivI | Bin::RemI if self.consts[y.0 as usize].is_some() => {
                        let c = self.consts[y.0 as usize].unwrap() as i32;
                        for l in 0..4u8 {
                            let a = self.extract_i(xv, l);
                            let cur = self.cur;
                            let r = crate::native::Emitter { f: &mut self.f, cur }.div_const(a, c, b == Bin::RemI);
                            self.emit(Inst::VInsert(d, d, l, r));
                        }
                    }
                    Bin::DivI | Bin::RemI => {
                        // per lane: x / 0 = 0, x % 0 = x (scalar code's sequence)
                        for l in 0..4u8 {
                            let a = self.extract_i(xv, l);
                            let c = self.extract_i(yv, l);
                            let z = self.icmpi_eq0(c);
                            let one = self.iconst(1);
                            let safe = self.sel(z, one, c);
                            let q = self.ibin(if b == Bin::DivI { IOp::Div } else { IOp::Rem }, I32, a, safe);
                            let zc = if b == Bin::DivI { self.iconst(0) } else { a };
                            let r = self.sel(z, zc, q);
                            self.emit(Inst::VInsert(d, d, l, r));
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
                let o = if matches!(op, Op::CmpF(..)) { VOp::FCmp(cc) } else { VOp::ICmp(cc) };
                let m = self.vbin(o, xv, yv);
                self.masks[v.0 as usize] = Some(m);
                let w = self.mask_to_word(m);
                self.emit(Inst::Mov(d, w));
            }
            Op::Sel(c, x, y) => {
                let m = self.mask(c);
                let (xv, yv) = (self.v(x), self.v(y));
                self.emit(Inst::VSel(d, m, xv, yv));
            }
            Op::Fma(Fma::MulAddI, a, b, c) => {
                let (av, bv, cv) = (self.v(a), self.v(b), self.v(c));
                let m = self.vbin(VOp::IMul, av, bv);
                self.emit(Inst::VBin(VOp::IAdd, d, m, cv));
            }
            Op::Fma(k, a, b, c) => {
                let (np, na) = match k {
                    Fma::Add => (false, false),
                    Fma::SubFrom => (true, false),
                    _ => (false, true),
                };
                let (av, bv, cv) = (self.v(a), self.v(b), self.v(c));
                self.emit(Inst::VFma(np, na, d, av, bv, cv));
            }
            Op::Wrap(x, len) => {
                let xv = self.v(x);
                if len.is_power_of_two() {
                    let mk = self.splat_const(len - 1);
                    self.emit(Inst::VBin(VOp::And, d, xv, mk));
                } else {
                    let lk = len as i32 as i64;
                    for l in 0..4u8 {
                        let a = self.extract_i(xv, l);
                        let cur = self.cur;
                        let r = crate::native::Emitter { f: &mut self.f, cur }.div_const(a, len as i32, true);
                        let neg = self.icmpi_lt0(r);
                        let up = self.ibini(IOp::Add, I32, r, lk);
                        let w = self.sel(neg, up, r);
                        self.emit(Inst::VInsert(d, d, l, w));
                    }
                }
            }
            Op::Load { region, base, extent, off } => {
                let fp = self.p.vals[v.0 as usize] == crate::ir::Ty::F32;
                self.load(d, region, base, extent, off, fp);
            }
            Op::FrameCount => self.emit(Inst::VSplat(d, self.n_i32)),
            Op::BufLen(k) => {
                let s = self.f.vreg(Cls::I);
                self.emit(Inst::Load(W32, s, self.regions[3], k as i32 * 16 + 8));
                self.emit(Inst::VSplat(d, s));
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

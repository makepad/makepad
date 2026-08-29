//! VIR — the vector IR between the splash pure-math subset detector and
//! the math backends (see platform/script/MATH_AOT.md).
//!
//! VIR is a small typed LINEAR IR: no loops (the batch loop is emitted by
//! each backend), no calls, no memory operations (backends alone emit the
//! batch load/store — a VIR program cannot express an address), and no
//! branches: control flow from the splash source arrives lowered to
//! `Sel*` ops, with both sides evaluated (every op is total and
//! side-effect free, so evaluating an unselected side is invisible).
//!
//! Types: `F64` scalars, `F32` scalars, `V` (f32x4 — vec2/vec3 ride f32x4
//! with the spare lanes unobservable: every op consuming lane content
//! beyond a vector's width is rejected at translation), `Mask` (per-lane
//! all-ones/zeros from packed compares), `Bool` (0/1 scalar conditions).
//!
//! One deliberate extension over the MATH_AOT.md sketch (which lists only
//! f32/f32x4 values): VIR keeps **f64 scalars with explicit
//! demote/promote**, because splash scalar arithmetic is f64 while its
//! math intrinsics and vector lanes are f32 — and the StitchBackend is
//! pinned as bit-identical to the splash interpreter. Codegen backends
//! may fuse f64 pairs under their ULP contract; the reference semantics
//! stay exact.
//!
//! The coarse vector ops (`Dot`, `Length`, `Cross`, `Normalize`) have
//! their reference semantics defined by [`eval`] below — the exact f32
//! operation sequence of the splash interpreter (`NumericValue` in
//! numeric.rs). The stitch backend lowers them to exactly that sequence;
//! codegen backends may substitute faster sequences within the documented
//! ULP contract.

/// A VIR virtual register: the index of the op that defines it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct VirReg(pub u32);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum VirTy {
    F64,
    F32,
    V,
    Mask,
    Bool,
}

/// The scalar/packed float math function family (all Rust `std` f32/f64
/// semantics, matching stitch's nonstandard 0xE0 opcodes).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MathFn {
    Sin,
    Cos,
    Tan,
    Asin,
    Acos,
    Atan,
    Exp,
    Ln,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MathFn2 {
    Atan2,
    Pow,
    /// Rust `min` (minNum: NaN loses).
    RMin,
    /// Rust `max`.
    RMax,
    /// Rust `%` (fmod).
    Rem,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CmpCc {
    Lt,
    Gt,
    Le,
    Ge,
    Eq,
    Ne,
}

/// One VIR operation. The defining register of op `i` is `VirReg(i)`.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum VirOp {
    /// A per-point function parameter (must appear first, in order): F64
    /// for scalar parameters, V for vector parameters (lanes past the
    /// width hold the batch loader's slack and are unobservable).
    Param { index: u32, ty: VirTy },

    /// A uniform parameter: read from the read-only f32 uniform block,
    /// constant across a batch, changeable between invocations without
    /// recompiling. Scalars load one f32 lane promoted to F64; vectors
    /// load their lanes into a V. Indexed statically — no expressible
    /// addresses.
    Uniform { index: u32, ty: VirTy },

    ConstF64(f64),
    ConstF32(f32),
    /// f32x4 of zeros.
    ZeroV,

    /// f64 -> f32 (IEEE demotion).
    Demote(VirReg),
    /// f32 -> f64 (exact).
    Promote(VirReg),
    /// Bool -> f64 (0.0 / 1.0), the splash `cast_to_f64` of a bool.
    BoolToF64(VirReg),

    Splat(VirReg),
    ExtractLane(VirReg, u8),
    ReplaceLane(VirReg, VirReg, u8),
    /// Single-source lane shuffle: result lane i = src lane `lanes[i]`.
    Shuffle(VirReg, [u8; 4]),

    // f64 scalar arithmetic (IEEE).
    AddF64(VirReg, VirReg),
    SubF64(VirReg, VirReg),
    MulF64(VirReg, VirReg),
    DivF64(VirReg, VirReg),
    NegF64(VirReg),
    AbsF64(VirReg),
    RemF64(VirReg, VirReg),

    // f32 scalar arithmetic (IEEE).
    AddF32(VirReg, VirReg),
    SubF32(VirReg, VirReg),
    MulF32(VirReg, VirReg),
    DivF32(VirReg, VirReg),
    SqrtF32(VirReg),
    AbsF32(VirReg),
    FloorF32(VirReg),
    CeilF32(VirReg),
    TruncF32(VirReg),

    MathF32(MathFn, VirReg),
    Math2F32(MathFn2, VirReg, VirReg),

    // packed f32x4 lane arithmetic (IEEE per lane).
    AddV(VirReg, VirReg),
    SubV(VirReg, VirReg),
    MulV(VirReg, VirReg),
    DivV(VirReg, VirReg),
    SqrtV(VirReg),
    AbsV(VirReg),
    FloorV(VirReg),
    CeilV(VirReg),
    TruncV(VirReg),

    MathV(MathFn, VirReg),
    Math2V(MathFn2, VirReg, VirReg),

    // Comparisons.
    CmpF64(CmpCc, VirReg, VirReg),
    CmpF32(CmpCc, VirReg, VirReg),
    /// Packed lane compare -> Mask.
    CmpV(CmpCc, VirReg, VirReg),

    BoolNot(VirReg),

    // Selects (the only "control flow").
    SelF64(VirReg, VirReg, VirReg),
    SelV(VirReg, VirReg, VirReg),
    /// Per-lane bit select: lanes of the first operand where the mask is
    /// set, the second elsewhere.
    SelLanes(VirReg, VirReg, VirReg),

    // Coarse vector ops (reference semantics = the interpreter's exact
    // f32 sequence; see `eval`). `w` is the vector width (2..=4).
    /// Left-associated lane sum of a*b over lanes 0..w -> F32.
    Dot { a: VirReg, b: VirReg, w: u8 },
    /// sqrt(dot(a, a, w)) -> F32.
    Length { a: VirReg, w: u8 },
    /// Vec3 cross product -> V (lane 3 zero).
    Cross { a: VirReg, b: VirReg },
    /// len = length(a); len == 0 ? a : a * splat(1/len) -> V.
    Normalize { a: VirReg, w: u8 },
}

/// A translated pure-math function.
#[derive(Clone, Debug)]
pub struct VirFn {
    /// Per-point parameter lane counts: 1 for scalar (F64), 2..=4 for
    /// vectors.
    pub param_lanes: Vec<u8>,
    /// Uniform parameter lane counts (same convention).
    pub uniform_lanes: Vec<u8>,
    pub ops: Vec<VirOp>,
    /// Types of each op's result (parallel to `ops`).
    pub types: Vec<VirTy>,
    /// The function result (always F64 in v1).
    pub result: VirReg,
}

impl VirFn {
    pub fn ty(&self, reg: VirReg) -> VirTy {
        self.types[reg.0 as usize]
    }

    /// Total input lanes per batch point.
    pub fn stride(&self) -> usize {
        self.param_lanes.iter().map(|l| *l as usize).sum()
    }

    /// Total f32 lanes in the uniform block.
    pub fn uniform_stride(&self) -> usize {
        self.uniform_lanes.iter().map(|l| *l as usize).sum()
    }
}

// =========================================================================
// The reference evaluator (InterpBackend's core)
// =========================================================================

/// A VIR value at evaluation time.
#[derive(Clone, Copy, Debug)]
pub enum VirVal {
    F64(f64),
    F32(f32),
    V([f32; 4]),
    Mask([u32; 4]),
    Bool(bool),
}

impl VirVal {
    fn f64(self) -> f64 {
        match self {
            VirVal::F64(v) => v,
            _ => unreachable!(),
        }
    }
    fn f32(self) -> f32 {
        match self {
            VirVal::F32(v) => v,
            _ => unreachable!(),
        }
    }
    fn v(self) -> [f32; 4] {
        match self {
            VirVal::V(v) => v,
            _ => unreachable!(),
        }
    }
    fn mask(self) -> [u32; 4] {
        match self {
            VirVal::Mask(v) => v,
            _ => unreachable!(),
        }
    }
    fn bool_(self) -> bool {
        match self {
            VirVal::Bool(v) => v,
            _ => unreachable!(),
        }
    }
}

fn math1_f32(f: MathFn, x: f32) -> f32 {
    match f {
        MathFn::Sin => x.sin(),
        MathFn::Cos => x.cos(),
        MathFn::Tan => x.tan(),
        MathFn::Asin => x.asin(),
        MathFn::Acos => x.acos(),
        MathFn::Atan => x.atan(),
        MathFn::Exp => x.exp(),
        MathFn::Ln => x.ln(),
    }
}

fn math2_f32(f: MathFn2, a: f32, b: f32) -> f32 {
    match f {
        MathFn2::Atan2 => a.atan2(b),
        MathFn2::Pow => a.powf(b),
        MathFn2::RMin => a.min(b),
        MathFn2::RMax => a.max(b),
        MathFn2::Rem => a % b,
    }
}

fn cmp<T: PartialOrd + PartialEq>(cc: CmpCc, a: T, b: T) -> bool {
    match cc {
        CmpCc::Lt => a < b,
        CmpCc::Gt => a > b,
        CmpCc::Le => a <= b,
        CmpCc::Ge => a >= b,
        CmpCc::Eq => a == b,
        CmpCc::Ne => a != b,
    }
}

fn map4(a: [f32; 4], f: impl Fn(f32) -> f32) -> [f32; 4] {
    [f(a[0]), f(a[1]), f(a[2]), f(a[3])]
}

fn zip4(a: [f32; 4], b: [f32; 4], f: impl Fn(f32, f32) -> f32) -> [f32; 4] {
    [f(a[0], b[0]), f(a[1], b[1]), f(a[2], b[2]), f(a[3], b[3])]
}

/// Evaluates a [`VirFn`] over one point. This is the semantic REFERENCE
/// for every backend: for the subset the translator accepts, its result
/// is bit-identical to the splash interpreter by construction.
/// `uniforms` holds `uniform_stride()` f32 lanes.
pub fn eval(f: &VirFn, params: &[VirVal], uniforms: &[f32]) -> f64 {
    let mut regs: Vec<VirVal> = Vec::with_capacity(f.ops.len());
    for op in &f.ops {
        let val = match *op {
            VirOp::Param { index, .. } => params[index as usize],
            VirOp::Uniform { index, ty } => {
                let off: usize = f.uniform_lanes[..index as usize]
                    .iter()
                    .map(|l| *l as usize)
                    .sum();
                match ty {
                    VirTy::F64 => VirVal::F64(uniforms[off] as f64),
                    _ => {
                        let lanes = f.uniform_lanes[index as usize] as usize;
                        let mut v = [0f32; 4];
                        v[..lanes].copy_from_slice(&uniforms[off..off + lanes]);
                        VirVal::V(v)
                    }
                }
            }
            VirOp::ConstF64(v) => VirVal::F64(v),
            VirOp::ConstF32(v) => VirVal::F32(v),
            VirOp::ZeroV => VirVal::V([0.0; 4]),
            VirOp::Demote(r) => VirVal::F32(regs[r.0 as usize].f64() as f32),
            VirOp::Promote(r) => VirVal::F64(regs[r.0 as usize].f32() as f64),
            VirOp::BoolToF64(r) => {
                VirVal::F64(if regs[r.0 as usize].bool_() { 1.0 } else { 0.0 })
            }
            VirOp::Splat(r) => VirVal::V([regs[r.0 as usize].f32(); 4]),
            VirOp::ExtractLane(r, lane) => VirVal::F32(regs[r.0 as usize].v()[lane as usize]),
            VirOp::ReplaceLane(v, s, lane) => {
                let mut lanes = regs[v.0 as usize].v();
                lanes[lane as usize] = regs[s.0 as usize].f32();
                VirVal::V(lanes)
            }
            VirOp::Shuffle(r, lanes) => {
                let src = regs[r.0 as usize].v();
                VirVal::V([
                    src[lanes[0] as usize],
                    src[lanes[1] as usize],
                    src[lanes[2] as usize],
                    src[lanes[3] as usize],
                ])
            }
            VirOp::AddF64(a, b) => VirVal::F64(regs[a.0 as usize].f64() + regs[b.0 as usize].f64()),
            VirOp::SubF64(a, b) => VirVal::F64(regs[a.0 as usize].f64() - regs[b.0 as usize].f64()),
            VirOp::MulF64(a, b) => VirVal::F64(regs[a.0 as usize].f64() * regs[b.0 as usize].f64()),
            VirOp::DivF64(a, b) => VirVal::F64(regs[a.0 as usize].f64() / regs[b.0 as usize].f64()),
            VirOp::NegF64(r) => VirVal::F64(-regs[r.0 as usize].f64()),
            VirOp::AbsF64(r) => VirVal::F64(regs[r.0 as usize].f64().abs()),
            VirOp::RemF64(a, b) => VirVal::F64(regs[a.0 as usize].f64() % regs[b.0 as usize].f64()),
            VirOp::AddF32(a, b) => VirVal::F32(regs[a.0 as usize].f32() + regs[b.0 as usize].f32()),
            VirOp::SubF32(a, b) => VirVal::F32(regs[a.0 as usize].f32() - regs[b.0 as usize].f32()),
            VirOp::MulF32(a, b) => VirVal::F32(regs[a.0 as usize].f32() * regs[b.0 as usize].f32()),
            VirOp::DivF32(a, b) => VirVal::F32(regs[a.0 as usize].f32() / regs[b.0 as usize].f32()),
            VirOp::SqrtF32(r) => VirVal::F32(regs[r.0 as usize].f32().sqrt()),
            VirOp::AbsF32(r) => VirVal::F32(regs[r.0 as usize].f32().abs()),
            VirOp::FloorF32(r) => VirVal::F32(regs[r.0 as usize].f32().floor()),
            VirOp::CeilF32(r) => VirVal::F32(regs[r.0 as usize].f32().ceil()),
            VirOp::TruncF32(r) => VirVal::F32(regs[r.0 as usize].f32().trunc()),
            VirOp::MathF32(fun, r) => VirVal::F32(math1_f32(fun, regs[r.0 as usize].f32())),
            VirOp::Math2F32(fun, a, b) => VirVal::F32(math2_f32(
                fun,
                regs[a.0 as usize].f32(),
                regs[b.0 as usize].f32(),
            )),
            VirOp::AddV(a, b) => {
                VirVal::V(zip4(regs[a.0 as usize].v(), regs[b.0 as usize].v(), |x, y| x + y))
            }
            VirOp::SubV(a, b) => {
                VirVal::V(zip4(regs[a.0 as usize].v(), regs[b.0 as usize].v(), |x, y| x - y))
            }
            VirOp::MulV(a, b) => {
                VirVal::V(zip4(regs[a.0 as usize].v(), regs[b.0 as usize].v(), |x, y| x * y))
            }
            VirOp::DivV(a, b) => {
                VirVal::V(zip4(regs[a.0 as usize].v(), regs[b.0 as usize].v(), |x, y| x / y))
            }
            VirOp::SqrtV(r) => VirVal::V(map4(regs[r.0 as usize].v(), |x| x.sqrt())),
            VirOp::AbsV(r) => VirVal::V(map4(regs[r.0 as usize].v(), |x| x.abs())),
            VirOp::FloorV(r) => VirVal::V(map4(regs[r.0 as usize].v(), |x| x.floor())),
            VirOp::CeilV(r) => VirVal::V(map4(regs[r.0 as usize].v(), |x| x.ceil())),
            VirOp::TruncV(r) => VirVal::V(map4(regs[r.0 as usize].v(), |x| x.trunc())),
            VirOp::MathV(fun, r) => VirVal::V(map4(regs[r.0 as usize].v(), |x| math1_f32(fun, x))),
            VirOp::Math2V(fun, a, b) => VirVal::V(zip4(
                regs[a.0 as usize].v(),
                regs[b.0 as usize].v(),
                |x, y| math2_f32(fun, x, y),
            )),
            VirOp::CmpF64(cc, a, b) => {
                VirVal::Bool(cmp(cc, regs[a.0 as usize].f64(), regs[b.0 as usize].f64()))
            }
            VirOp::CmpF32(cc, a, b) => {
                VirVal::Bool(cmp(cc, regs[a.0 as usize].f32(), regs[b.0 as usize].f32()))
            }
            VirOp::CmpV(cc, a, b) => {
                let a = regs[a.0 as usize].v();
                let b = regs[b.0 as usize].v();
                let lane = |i: usize| if cmp(cc, a[i], b[i]) { u32::MAX } else { 0 };
                VirVal::Mask([lane(0), lane(1), lane(2), lane(3)])
            }
            VirOp::BoolNot(r) => VirVal::Bool(!regs[r.0 as usize].bool_()),
            VirOp::SelF64(c, a, b) => {
                if regs[c.0 as usize].bool_() {
                    VirVal::F64(regs[a.0 as usize].f64())
                } else {
                    VirVal::F64(regs[b.0 as usize].f64())
                }
            }
            VirOp::SelV(c, a, b) => {
                if regs[c.0 as usize].bool_() {
                    VirVal::V(regs[a.0 as usize].v())
                } else {
                    VirVal::V(regs[b.0 as usize].v())
                }
            }
            VirOp::SelLanes(m, a, b) => {
                let m = regs[m.0 as usize].mask();
                let a = regs[a.0 as usize].v();
                let b = regs[b.0 as usize].v();
                let lane = |i: usize| {
                    f32::from_bits((a[i].to_bits() & m[i]) | (b[i].to_bits() & !m[i]))
                };
                VirVal::V([lane(0), lane(1), lane(2), lane(3)])
            }
            VirOp::Dot { a, b, w } => {
                let a = regs[a.0 as usize].v();
                let b = regs[b.0 as usize].v();
                let mut sum = a[0] * b[0];
                for lane in 1..w as usize {
                    sum += a[lane] * b[lane];
                }
                VirVal::F32(sum)
            }
            VirOp::Length { a, w } => {
                let a = regs[a.0 as usize].v();
                let mut sum = a[0] * a[0];
                for lane in 1..w as usize {
                    sum += a[lane] * a[lane];
                }
                VirVal::F32(sum.sqrt())
            }
            VirOp::Cross { a, b } => {
                let a = regs[a.0 as usize].v();
                let b = regs[b.0 as usize].v();
                VirVal::V([
                    a[1] * b[2] - a[2] * b[1],
                    a[2] * b[0] - a[0] * b[2],
                    a[0] * b[1] - a[1] * b[0],
                    0.0,
                ])
            }
            VirOp::Normalize { a, w } => {
                let v = regs[a.0 as usize].v();
                let mut sum = v[0] * v[0];
                for lane in 1..w as usize {
                    sum += v[lane] * v[lane];
                }
                let len = sum.sqrt();
                if len == 0.0 {
                    VirVal::V(v)
                } else {
                    let inv = 1.0 / len;
                    VirVal::V(map4(v, |x| x * inv))
                }
            }
        };
        regs.push(val);
    }
    regs[f.result.0 as usize].f64()
}

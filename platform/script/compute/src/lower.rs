//! The audio shader front end, part 2: static typing, whole-program
//! inlining and lowering of the AST into AIR ([`crate::ir`]).
//!
//! Front-end types: `f32`, `i32`, `bool`, `vec2` (a pair of f32 values)
//! and the aggregates (structs, fixed arrays), which always live in memory
//! (state, shared tables or the call frame) and are handled BY REFERENCE,
//! like Splash objects. Numeric literals stay untyped ([`V::Lit`]) until
//! they meet a typed value, so `i * 2` is integer and `x * 2` is float.
//!
//! Helpers are inlined at every call (monomorphized per call site); a
//! `return` inside one is a `Break` out of a one-iteration wrapper loop,
//! which is dropped again when the helper has no early return.

use crate::ir::{self, Bin, Block, Cmp, Op, Program, Region, Stmt as IS, Ty, Un, Val, Var};

mod block;
pub mod event;
mod fft;
pub mod kernel;
mod portable;
mod rate;
use rate::{RateNests, RatePath};
mod front;
pub mod trace;
pub use front::{AirBuilder, MathFn};

/// Which front end a program is lowered for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Domain {
    /// Audio shaders: voices and effects with state, run per sample.
    Audio,
    /// Compute kernels: stateless per-element programs over host buffers.
    Kernel,
    /// Event programs ([`event`]): entries over shared state, calling ports.
    Event,
}
use crate::parse::*;
use crate::ShaderError;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};

/// Words at the start of every state block (see `header`).
pub const HEADER_WORDS: u32 = 15;
pub mod header {
    pub const NOTE: u32 = 0;
    /// A running glide's carry: the part of its sample distance that the
    /// rounded pitch in GLIDE_NOTE cannot hold (see glide_frames()).
    pub const GLIDE_REMAINDER: u32 = 1;
    pub const GATE: u32 = 2;
    pub const VELOCITY: u32 = 3;
    /// Absolute frame of the note-on (i32): `trigger` is true there.
    pub const TRIGGER: u32 = 4;
    /// Set to 1 by `stop()`.
    pub const STOP: u32 = 5;
    pub const RNG: u32 = 6;
    /// 1 until the first control block ran (params jump, `block()` runs).
    pub const FRESH: u32 = 7;
    pub const BEND: u32 = 8;
    pub const PRESSURE: u32 = 9;
    pub const SLIDE: u32 = 10;
    pub const RELEASE_VELOCITY: u32 = 11;
    pub const VOICE: u32 = 12;
    pub const GLIDE_NOTE: u32 = 13;
    pub const GLIDE_STEP: u32 = 14;
}
/// Sample rate, absolute frame, musical time, controllers and parameter targets.
pub const CTX_RATE: u32 = 0;
pub const CTX_FRAME: u32 = 1;
pub const CTX_TEMPO: u32 = 2;
pub const CTX_BEAT_AT: u32 = 3;
pub const CTX_BEAT_FRAME: u32 = 4;
pub const CTX_BEAT_STEP: u32 = 5;
pub const CTX_BAR_BEATS: u32 = 6;
pub const CTX_PLAYING: u32 = 7;
pub const CTX_INSTANCE: u32 = CTX_PLAYING + 1;
pub const CTX_CC: u32 = CTX_INSTANCE + 1;
pub const CC_WORDS: u32 = 128;
pub const CTX_PARAMS: u32 = CTX_CC + CC_WORDS;

/// Frames per control block (params ramp across one; `block()` runs at
/// each start).
pub const CONTROL_BLOCK: u32 = 64;
/// Most frames one call may render.
pub const MAX_FRAMES: u32 = 4096;
/// Iteration cap of `while`/`loop` and of `for` loops with runtime bounds.
pub const LOOP_CAP: u32 = 1024;
/// `for` loops over constant ranges up to this many iterations (with no
/// break/continue of their own) are unrolled, when the unrolled code fits
/// [`UNROLL_BYTES`].
pub const UNROLL_MAX: u32 = 8;
/// Audio programs' `for` loops over constant ranges up to this many
/// iterations unroll (see `for_loop`).
pub const AUDIO_UNROLL_MAX: u32 = 16;
/// Most code a fully unrolled loop may emit (estimated, every backend):
/// beyond a few hundred instructions unrolling stops paying (the loop's
/// own overhead is a few instructions an iteration) and costs instruction
/// cache, on phones' 32-64 KiB L1i above all; nested loops multiply.
pub const UNROLL_BYTES: u64 = 2048;
/// Estimated bytes of emitted code per AIR operation.
const BYTES_PER_OP: u64 = 4;
/// A kernel helper estimated at more AIR operations than this, called
/// from more than one place, is lowered once as a function and called
/// (each call costs a few instructions; its body would otherwise be copied
/// to every call site). Smaller helpers, helpers called once and calls
/// whose arguments are all constants (they fold) are inlined.
pub const INLINE_OPS: u64 = 48;
pub const MAX_FRAME_WORDS: u32 = 1 << 14;
/// Largest program after inlining and unrolling (AIR values).
pub const MAX_IR_VALS: usize = 1 << 20;
/// Worst-case AIR ops of one `init()` run.
pub const MAX_INIT_COST: u64 = 200_000_000;
/// Warn above this worst-case AIR cost per frame.
pub const WARN_COST_PER_FRAME: u64 = 4_000_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Kind {
    Instrument,
    Effect,
    Control,
}

#[derive(Clone, Debug)]
pub struct ParamInfo {
    pub name: String,
    pub default: f32,
    pub min: f32,
    pub max: f32,
}

#[relax_api::relax_api(plain)]
#[repr(u8)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ParamUnit {
    #[default]
    None,
    Hz,
    Secs,
    Ms,
    Db,
    Semis,
    Cents,
    Beats,
    Percent,
}

impl ParamUnit {
    /// How a value's unit is written after it: `Hz`, `dB`, `%`, empty for none.
    pub fn symbol(self) -> &'static str {
        match self {
            ParamUnit::None => "",
            ParamUnit::Hz => "Hz",
            ParamUnit::Secs => "s",
            ParamUnit::Ms => "ms",
            ParamUnit::Db => "dB",
            ParamUnit::Semis => "st",
            ParamUnit::Cents => "ct",
            ParamUnit::Beats => "beats",
            ParamUnit::Percent => "%",
        }
    }

    /// A value as a person reads it: `1200 Hz`, `35%` (a percent param
    /// holds a 0..1 fraction), a bare number without a unit.
    pub fn show(self, value: f32) -> String {
        match self {
            ParamUnit::None => format!("{value}"),
            ParamUnit::Percent => format!("{}%", value * 100.0),
            unit => format!("{value} {}", unit.symbol()),
        }
    }
}

#[relax_api::relax_api(plain)]
#[repr(u8)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ParamCurve {
    #[default]
    Linear,
    Log,
    Exp,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ParamMeta {
    pub unit: ParamUnit,
    pub curve: ParamCurve,
    /// Zero for a continuous parameter.
    pub step: f32,
    /// An empty label inherits the parameter name.
    pub label: String,
    pub group: String,
    pub choices: Vec<String>,
    /// Smoothing time in seconds. By default continuous parameters ramp
    /// over one control block and discrete parameters jump.
    pub smooth: Option<f32>,
}

/// A state variable's place in the state block (for hot swap).
#[derive(Clone, Debug)]
pub struct StateVar {
    pub name: String,
    /// The declaration's byte span in the source the lowering read.
    pub span: Span,
    /// Type signature: equal signatures have equal layouts.
    pub sig: String,
    pub offset: u32,
    pub words: u32,
}

/// A declared audio port, in source order. Channels are numbered within its direction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioPortInfo {
    pub name: String,
    pub output: bool,
    pub channels: u16,
    pub first: u16,
    pub control: bool,
}

struct AudioPort {
    info: AudioPortInfo,
    span: Span,
    held: Vec<Var>,
    frame_write: bool,
}

/// A nonfatal finding in the source of a successfully lowered shader.
#[derive(Clone, Debug)]
pub struct ShaderWarning {
    pub span: Span,
    pub message: String,
}

pub struct Lowered {
    pub warnings: Vec<ShaderWarning>,
    pub ports: Vec<AudioPortInfo>,
    pub mono: bool,
    pub latency: u32,
    pub kind: Kind,
    pub render: Program,
    pub init: Option<Program>,
    pub params: Vec<ParamInfo>,
    pub probes: Vec<String>,
    pub sample_names: Vec<String>,
    pub param_meta: Vec<ParamMeta>,
    pub state_init: Vec<u32>,
    pub state_vars: Vec<StateVar>,
    pub shared_init: Vec<u32>,
}

// =========================================================================
// Types, values, places
// =========================================================================

#[derive(Clone, Debug, PartialEq)]
enum T {
    F,
    I,
    B,
    /// f64: a register value; two words (low, high) in memory.
    D,
    /// n f32 lanes: vec2, vec3, vec4, and mat4 (16, column-major).
    Vec(u8),
    Struct(usize),
    Array(Box<T>, u32),
    /// A view of host buffer region k: element type, stride and offset in
    /// words, and whether the kernel may write it.
    Buf(Box<T>, u32, u32, bool),
}

/// The lanes of a vector value (the first n are used).
type Lanes = [Val; 16];

fn lanes_of(vals: &[Val]) -> Lanes {
    let mut l = [Val(0); 16];
    l[..vals.len()].copy_from_slice(vals);
    l
}

fn vec_name(n: u8) -> &'static str {
    match n {
        2 => "vec2",
        3 => "vec3",
        4 => "vec4",
        _ => "mat4",
    }
}

/// The lane a component name selects (`x y z w`, `r g b a`; for vec2 also
/// `l r` = left, right, and `0 1`).
fn lane_of(n: u8, c: char) -> Option<usize> {
    let lane = match c {
        'x' => 0,
        'y' => 1,
        'z' => 2,
        'w' => 3,
        'l' if n == 2 => 0,
        'r' if n == 2 => 1,
        'r' => 0,
        'g' => 1,
        'b' => 2,
        'a' => 3,
        _ => return None,
    };
    (lane < n as usize && n <= 4).then_some(lane)
}

#[derive(Clone, Debug)]
struct StructDef {
    name: String,
    /// (name, type, word offset, default init).
    fields: Vec<(String, T, u32, CInit)>,
    words: u32,
    /// A plain object's fields a kernel cannot hold (a string, a function,
    /// a value that is not a constant): (name, why). Reading one is the
    /// error.
    opaque: Vec<(String, String)>,
}

/// The struct name of plain objects (`{a: 1, b: vec3(..)}`).
const OBJECT: &str = "object";

#[derive(Clone, Debug)]
struct Place {
    region: Region,
    root: u32,
    extent: u32,
    off: Option<Val>,
    stat: u32,
    ty: T,
}

#[derive(Clone, Debug)]
enum V {
    Lit(f64),
    F(Val),
    D(Val),
    I(Val),
    B(Val),
    Vec(u8, Lanes),
    Place(Place),
    Unit,
}

#[derive(Clone, Debug)]
enum Bind {
    Const(V),
    /// A constant vector (a top-level `let` folded at lowering: a colour,
    /// `vec3(..)`, a pure function of constants): its lanes' f32 bits,
    /// materialised as constants wherever it is read.
    ConstVec(u8, [u32; 16]),
    /// A top-level scalar `let` with a declared type (`let high: i32 = 1`):
    /// its type (`T::I` or `T::F`) and bits, materialised as a constant of
    /// that type wherever it is read, so joins and conversions see the type.
    /// An untyped one stays `Const(V::Lit)` and takes its use's type.
    Scalar(T, u32),
    /// An immutable SSA value (loop indices).
    Value(V),
    /// A local function (`let f = fn(x) {..}`, `fn f(x) {..}` in a body):
    /// inlined where called, seeing the scopes it was defined in.
    Closure(Box<FnDecl>, Vec<HashMap<String, Bind>>),
    /// A mutable scalar or vec2 local (or a promoted state scalar).
    Local(T, Vec<Var>),
    Place(Place),
    Param(usize),
    AudioPort(usize),
    /// A kernel uniform (ctx word).
    Uniform(usize),
    /// A kernel emit buffer: (data region, count region, record width,
    /// slots per element).
    Emit(u8, u8, u32, u32),
}

fn image_words(words: u64, span: Span) -> LResult<u32> {
    u32::try_from(words).map_err(|_| ShaderError::new(span.start, span.end, format!("the image needs {words} words, exceeding the 32-bit region length limit of {} words", u32::MAX)))
}

fn reserve_words(out: &mut Vec<u32>, additional: u32, span: Span) -> LResult<()> {
    let words = image_words(out.len() as u64 + u64::from(additional), span)?;
    #[cfg(target_arch = "wasm32")]
    if words >= 1 << 30 {
        return err(span, format!("the image needs {words} words before working memory; wasm memory addresses 65536 pages"));
    }
    out.try_reserve_exact(additional as usize).map_err(|_| ShaderError::new(span.start, span.end, format!("could not allocate {} bytes for the image", u64::from(words) * 4)))
}

/// A constant initializer (state, struct defaults, tables).
#[derive(Clone, Debug)]
enum CInit {
    W(u32, T),
    Vec(u8, [u32; 16]),
    Struct(usize, Vec<CInit>),
    Array(T, Vec<CInit>),
    Repeat(Box<CInit>, u32),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum LoopKind {
    User,
    Wrapper,
    Frame,
}

struct Ret {
    vars: Option<(T, Vec<Var>)>,
    used: bool,
}

type LResult<T> = Result<T, ShaderError>;

/// A speculative diagnostic lets the caller try another interpretation.
fn speculative<T>(result: LResult<T>) -> LResult<Option<T>> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(e) if e.is_cancelled() => Err(e),
        Err(_) => Ok(None),
    }
}

fn err<X>(span: Span, msg: impl Into<String>) -> LResult<X> {
    Err(ShaderError::new(span.start, span.end.max(span.start + 1), msg.into()))
}

fn float_literal(e: &Expr) -> bool {
    matches!(&e.kind, ExprKind::Num(_, false)) || matches!(&e.kind, ExprKind::Neg(x) if matches!(x.kind, ExprKind::Num(_, false)))
}

// =========================================================================
// The builder: emits AIR with constant folding
// =========================================================================

/// Value-numbering key of a pure op (loads and gets are "memory" keys:
/// dropped when a store or set may change them, and never reused across a
/// loop boundary).
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Key {
    Const(Ty, u64),
    Un(Un, Val),
    Bin(Bin, Val, Val),
    CmpF(Cmp, Val, Val),
    CmpI(Cmp, Val, Val),
    CmpD(Cmp, Val, Val),
    Sel(Val, Val, Val),
    Fma(ir::Fma, Val, Val, Val),
    Wrap(Val, u32),
    /// A load's type is part of it: a word stored as i32 and read as f32
    /// is not the stored value.
    Load(Region, u32, u32, Option<Val>, Ty),
    Get(Var),
    In(u16, Val),
    FrameCount,
    BufLen(u8),
}

impl Key {
    fn of(op: &Op, ty: Ty) -> Key {
        match *op {
            Op::ConstF(x) => Key::Const(ty, x.to_bits() as u64),
            Op::ConstD(x) => Key::Const(ty, x.to_bits()),
            Op::ConstI(x) => Key::Const(ty, x as u32 as u64),
            Op::ConstB(x) => Key::Const(ty, x as u64),
            Op::Get(v) => Key::Get(v),
            Op::Un(u, a) => Key::Un(u, a),
            Op::Bin(b, x, y) => Key::Bin(b, x, y),
            Op::CmpF(c, x, y) => Key::CmpF(c, x, y),
            Op::CmpI(c, x, y) => Key::CmpI(c, x, y),
            Op::CmpD(c, x, y) => Key::CmpD(c, x, y),
            Op::Sel(c, x, y) => Key::Sel(c, x, y),
            Op::Fma(k, a, b, c) => Key::Fma(k, a, b, c),
            Op::Wrap(x, n) => Key::Wrap(x, n),
            // Without an offset the extent does not matter: word `base`.
            Op::Load { region, base, extent, off } => Key::Load(region, base, if off.is_some() { extent } else { 1 }, off, ty),
            Op::In { ch, idx } => Key::In(ch, idx),
            Op::FrameCount => Key::FrameCount,
            Op::BufLen(k) => Key::BufLen(k),
        }
    }

    fn memory(&self) -> bool {
        matches!(self, Key::Load(..) | Key::Get(_))
    }
}

#[derive(Default)]
#[derive(Clone)]
struct Builder {
    prog: Program,
    blocks: Vec<Block>,
    /// Register bits of constant values (f64 in all 64).
    consts: Vec<Option<u64>>,
    /// Per open block: values by key, and whether the block is a loop body
    /// (memory keys from outside it are not reused inside).
    cse: Vec<(HashMap<Key, Val>, bool)>,
    /// A throwaway builder for constant evaluation ([`Lowerer::scratch`]):
    /// literals in it stay constants.
    scratch: bool,
}

impl Builder {
    fn new() -> Self {
        Builder { prog: Program::default(), blocks: vec![Vec::new()], consts: Vec::new(), cse: vec![(HashMap::new(), false)], scratch: false }
    }

    fn lookup(&self, key: &Key) -> Option<Val> {
        for (map, is_loop) in self.cse.iter().rev() {
            if let Some(v) = map.get(key) {
                return Some(*v);
            }
            if *is_loop && key.memory() {
                return None;
            }
        }
        None
    }

    fn push(&mut self, s: IS) {
        match &s {
            IS::Set(var, _) => {
                let var = *var;
                for (map, _) in &mut self.cse {
                    map.remove(&Key::Get(var));
                }
            }
            IS::Store { region, base, extent, off, val } => {
                let region = *region;
                // The words the store may write: its own, or its whole range
                // with an offset. In u64: a range ending past u32::MAX (an
                // emit's word k of u32::MAX) must not wrap to a low end and
                // read as a miss.
                let lo = *base as u64;
                let hi = lo + if off.is_some() { *extent as u64 } else { 1 };
                for (map, _) in &mut self.cse {
                    map.retain(|k, _| match *k {
                        Key::Load(r, ..) if r != region => true,
                        // Two words past a host buffer's end clamp to the same word: any may alias.
                        Key::Load(Region::Buf(_), ..) => false,
                        // A load keeps its value while the store misses every word it may read.
                        Key::Load(_, b, e, ..) => b as u64 + e as u64 <= lo || hi <= b as u64,
                        _ => true,
                    });
                }
                // A later load of that word (same type) in this block reads
                // the stored value. Not host buffers: two words past a
                // buffer's end clamp to the same word.
                if off.is_none() && !matches!(region, Region::Buf(_)) {
                    let ty = self.prog.vals[val.0 as usize];
                    self.cse.last_mut().unwrap().0.insert(Key::Load(region, *base, 1, None, ty), *val);
                }
            }
            // A function may write memory (not the caller's variables, not
            // the tables).
            IS::Call { .. } => {
                for (map, _) in &mut self.cse {
                    map.retain(|k, _| !matches!(k, Key::Load(r, ..) if *r != Region::Shared));
                }
            }
            // A host call may write its slices' buffers (and ctx words).
            IS::CallHost { slices, .. } => {
                let bufs: Vec<u8> = slices.iter().map(|x| x.buf).collect();
                for (map, _) in &mut self.cse {
                    map.retain(|k, _| !matches!(k, Key::Load(Region::Buf(b), ..) if bufs.contains(b)) && !matches!(k, Key::Load(Region::Ctx, ..)));
                }
            }
            // A port may read or write any host buffer, including unlisted ones.
            IS::Port { .. } => {
                for (map, _) in &mut self.cse {
                    map.retain(|k, _| !matches!(k, Key::Load(Region::Buf(_), ..)));
                }
            }
            _ => {}
        }
        self.blocks.last_mut().unwrap().push(s);
    }

    fn open_loop(&mut self) {
        self.blocks.push(Vec::new());
        self.cse.push((HashMap::new(), true));
    }

    fn open(&mut self) {
        self.cse.push((HashMap::new(), false));
        self.blocks.push(Vec::new());
    }

    fn close(&mut self) -> Block {
        self.cse.pop();
        self.blocks.pop().unwrap()
    }

    fn var(&mut self, ty: Ty) -> Var {
        self.prog.vars.push(ty);
        Var(self.prog.vars.len() as u32 - 1)
    }

    /// A constant's low word (every 32-bit type).
    fn cst(&self, v: Val) -> Option<u32> {
        self.cst64(v).map(|c| c as u32)
    }

    fn cst64(&self, v: Val) -> Option<u64> {
        // A value of another builder (an outer local seen while a constant
        // initializer is folded in a scratch builder) is not a constant.
        self.consts.get(v.0 as usize).copied().flatten()
    }

    fn raw(&mut self, ty: Ty, op: Op, c: Option<u64>) -> Val {
        let key = Key::of(&op, ty);
        if let Some(v) = self.lookup(&key) {
            return v;
        }
        self.cse.last_mut().unwrap().0.insert(key, Val(self.prog.vals.len() as u32));
        let v = Val(self.prog.vals.len() as u32);
        self.prog.vals.push(ty);
        self.consts.push(c);
        self.push(IS::Def(v, op));
        v
    }

    fn cf(&mut self, x: f32) -> Val {
        self.raw(Ty::F32, Op::ConstF(x), Some(x.to_bits() as u64))
    }

    fn cd(&mut self, x: f64) -> Val {
        self.raw(Ty::F64, Op::ConstD(x), Some(x.to_bits()))
    }

    fn ci(&mut self, x: i32) -> Val {
        self.raw(Ty::I32, Op::ConstI(x), Some(x as u32 as u64))
    }

    fn cb(&mut self, x: bool) -> Val {
        self.raw(Ty::Bool, Op::ConstB(x), Some(x as u64))
    }

    fn konst(&mut self, ty: Ty, bits: u64) -> Val {
        match ty {
            Ty::F32 => self.cf(f32::from_bits(bits as u32)),
            Ty::I32 => self.ci(bits as u32 as i32),
            Ty::Bool => self.cb(bits as u32 != 0),
            Ty::F64 => self.cd(f64::from_bits(bits)),
        }
    }

    /// Emits `op` of type `ty`, folding it when every operand is constant.
    fn def(&mut self, ty: Ty, op: Op) -> Val {
        let folded = match op {
            Op::Un(u, a) => self.cst64(a).map(|a| ir::eval_un(u, a)),
            Op::Bin(b, x, y) => match (self.cst64(x), self.cst64(y)) {
                (Some(x), Some(y)) => Some(ir::eval_bin(b, x, y)),
                _ => None,
            },
            Op::CmpF(cc, x, y) => match (self.cst64(x), self.cst64(y)) {
                (Some(x), Some(y)) => Some(ir::eval_cmp_f(cc, x, y)),
                _ => None,
            },
            Op::CmpD(cc, x, y) => match (self.cst64(x), self.cst64(y)) {
                (Some(x), Some(y)) => Some(ir::eval_cmp_d(cc, x, y)),
                _ => None,
            },
            Op::CmpI(cc, x, y) => match (self.cst64(x), self.cst64(y)) {
                (Some(x), Some(y)) => Some(ir::eval_cmp_i(cc, x, y)),
                _ => None,
            },
            Op::Sel(c, x, y) => match self.cst(c) {
                Some(c) => return if c != 0 { x } else { y },
                None => None,
            },
            Op::Wrap(x, len) => self.cst(x).map(|x| ir::eval_wrap(x, len) as u64),
            _ => None,
        };
        match folded {
            Some(bits) => self.konst(ty, bits),
            None => self.raw(ty, op, None),
        }
    }

    fn un(&mut self, ty: Ty, u: Un, a: Val) -> Val {
        self.def(ty, Op::Un(u, a))
    }

    fn fb(&mut self, b: Bin, x: Val, y: Val) -> Val {
        self.def(Ty::F32, Op::Bin(b, x, y))
    }

    fn ib(&mut self, b: Bin, x: Val, y: Val) -> Val {
        self.def(Ty::I32, Op::Bin(b, x, y))
    }

    fn add(&mut self, x: Val, y: Val) -> Val {
        self.fb(Bin::AddF, x, y)
    }
    fn sub(&mut self, x: Val, y: Val) -> Val {
        self.fb(Bin::SubF, x, y)
    }
    fn mul(&mut self, x: Val, y: Val) -> Val {
        self.fb(Bin::MulF, x, y)
    }
    fn div(&mut self, x: Val, y: Val) -> Val {
        self.fb(Bin::DivF, x, y)
    }
    fn mulc(&mut self, x: Val, c: f32) -> Val {
        let c = self.cf(c);
        self.mul(x, c)
    }
    fn addc(&mut self, x: Val, c: f32) -> Val {
        let c = self.cf(c);
        self.add(x, c)
    }
    fn cmpf(&mut self, cc: Cmp, x: Val, y: Val) -> Val {
        self.def(Ty::Bool, Op::CmpF(cc, x, y))
    }
    fn cmpi(&mut self, cc: Cmp, x: Val, y: Val) -> Val {
        self.def(Ty::Bool, Op::CmpI(cc, x, y))
    }
    fn sel(&mut self, c: Val, x: Val, y: Val) -> Val {
        let ty = self.prog.vals.get(x.0 as usize).copied().unwrap_or(Ty::F32);
        self.def(ty, Op::Sel(c, x, y))
    }
    fn get(&mut self, v: Var) -> Val {
        let ty = self.prog.vars.get(v.0 as usize).copied().unwrap_or(Ty::F32);
        self.raw(ty, Op::Get(v), None)
    }
    fn set(&mut self, v: Var, x: Val) {
        self.push(IS::Set(v, x));
    }
    fn load(&mut self, ty: Ty, region: Region, base: u32, extent: u32, off: Option<Val>) -> Val {
        // A constant offset folds into the base.
        if let Some(o) = off {
            if let Some(c) = self.cst(o) {
                // (Saturating: past u32::MAX is the last word, never a low one.)
                let c = ir::clamp_off(c, extent);
                return self.raw(ty, Op::Load { region, base: base.saturating_add(c), extent: 1, off: None }, None);
            }
        }
        self.raw(ty, Op::Load { region, base, extent, off }, None)
    }
    fn store(&mut self, region: Region, base: u32, extent: u32, off: Option<Val>, val: Val) {
        if let Some(o) = off {
            if let Some(c) = self.cst(o) {
                let c = ir::clamp_off(c, extent);
                self.push(IS::Store { region, base: base.saturating_add(c), extent: 1, off: None, val });
                return;
            }
        }
        self.push(IS::Store { region, base, extent, off, val });
    }

    /// True when a block only defines pure values (safe to hoist and run
    /// unconditionally).
    fn pure_block(b: &Block) -> bool {
        ir::unmarked(b).all(|s| matches!(s, IS::Def(..)))
    }

    fn splice(&mut self, b: Block) {
        self.blocks.last_mut().unwrap().extend(b);
    }

    // -- deterministic math (Cephes-style polynomials, IEEE ops only) ----

    fn poly(&mut self, x: Val, coefs: &[f32]) -> Val {
        let mut acc = self.cf(coefs[0]);
        for c in &coefs[1..] {
            let m = self.mul(acc, x);
            acc = self.addc(m, *c);
        }
        acc
    }

    fn fabs(&mut self, x: Val) -> Val {
        self.un(Ty::F32, Un::AbsF, x)
    }

    fn fneg(&mut self, x: Val) -> Val {
        self.un(Ty::F32, Un::NegF, x)
    }

    /// sin (`cos == false`) or cos: octant reduction with a 3-part π/4,
    /// then the sin or cos minimax polynomial on [-π/4, π/4].
    fn sincos(&mut self, x: Val, cos: bool) -> Val {
        const FOPI: f32 = 1.273_239_5;
        const DP1: f32 = 0.785_156_25;
        const DP2: f32 = 2.418_756_5e-4;
        const DP3: f32 = 3.774_895e-8;
        let ax = self.fabs(x);
        let s = self.mulc(ax, FOPI);
        let j = self.un(Ty::I32, Un::F2I, s);
        let one = self.ci(1);
        let odd = self.ib(Bin::AndI, j, one);
        let j = self.ib(Bin::AddI, j, odd);
        let y = self.un(Ty::F32, Un::I2F, j);
        let seven = self.ci(7);
        let j = self.ib(Bin::AndI, j, seven);
        let three = self.ci(3);
        let flip = self.cmpi(Cmp::Gt, j, three);
        let four = self.ci(4);
        let jm4 = self.ib(Bin::SubI, j, four);
        let j = self.sel(flip, jm4, j);
        let t1 = self.mulc(y, DP1);
        let r = self.sub(ax, t1);
        let t2 = self.mulc(y, DP2);
        let r = self.sub(r, t2);
        let t3 = self.mulc(y, DP3);
        let r = self.sub(r, t3);
        let z = self.mul(r, r);
        // sin poly: ((s0 z + s1) z + s2) z r + r
        let sp = self.poly(z, &[-1.951_529_6e-4, 8.332_161e-3, -1.666_665_5e-1]);
        let sp = self.mul(sp, z);
        let sp = self.mul(sp, r);
        let sp = self.add(sp, r);
        // cos poly: ((c0 z + c1) z + c2) z z - 0.5 z + 1
        let cp = self.poly(z, &[2.443_315_7e-5, -1.388_731_6e-3, 4.166_664_6e-2]);
        let cp = self.mul(cp, z);
        let cp = self.mul(cp, z);
        let hz = self.mulc(z, 0.5);
        let cp = self.sub(cp, hz);
        let cp = self.addc(cp, 1.0);
        let is1 = self.cmpi(Cmp::Eq, j, one);
        let two = self.ci(2);
        let is2 = self.cmpi(Cmp::Eq, j, two);
        let mid = self.def(Ty::Bool, Op::Bin(Bin::OrB, is1, is2));
        let (v, neg) = if cos {
            let v = self.sel(mid, sp, cp);
            let gt1 = self.cmpi(Cmp::Gt, j, one);
            let neg = self.cmpi(Cmp::Ne, flip, gt1);
            (v, neg)
        } else {
            let v = self.sel(mid, cp, sp);
            let zero = self.cf(0.0);
            let xneg = self.cmpf(Cmp::Lt, x, zero);
            let neg = self.cmpi(Cmp::Ne, flip, xneg);
            (v, neg)
        };
        let nv = self.fneg(v);
        self.sel(neg, nv, v)
    }

    /// exp over [-87.3, 88]: 2^n by exponent bits times a degree-6
    /// polynomial on the reduced argument.
    fn exp(&mut self, x: Val) -> Val {
        let lo = self.cf(-87.3);
        let hi = self.cf(88.0);
        let x = self.fb(Bin::MaxF, x, lo);
        let x = self.fb(Bin::MinF, x, hi);
        let t = self.mulc(x, std::f32::consts::LOG2_E);
        let t = self.addc(t, 0.5);
        let fx = self.un(Ty::F32, Un::FloorF, t);
        let a = self.mulc(fx, 0.693_359_4);
        let x = self.sub(x, a);
        let b = self.mulc(fx, -2.121_944_4e-4);
        let x = self.sub(x, b);
        let z = self.mul(x, x);
        let p = self.poly(x, &[1.987_569_1e-4, 1.398_199_9e-3, 8.333_452e-3, 4.166_579_6e-2, 1.666_666_5e-1, 5.000_000_1e-1]);
        let p = self.mul(p, z);
        let p = self.add(p, x);
        let p = self.addc(p, 1.0);
        let n = self.un(Ty::I32, Un::F2I, fx);
        let c127 = self.ci(127);
        let n = self.ib(Bin::AddI, n, c127);
        let c23 = self.ci(23);
        let n = self.ib(Bin::ShlI, n, c23);
        let pow2 = self.un(Ty::F32, Un::BitsIF, n);
        self.mul(p, pow2)
    }

    /// Natural log for x > 0 (x <= 0 gives -inf).
    fn log(&mut self, x: Val) -> Val {
        let bits = self.un(Ty::I32, Un::BitsFI, x);
        let c23 = self.ci(23);
        let e = self.ib(Bin::ShrI, bits, c23);
        let c126 = self.ci(126);
        let e = self.ib(Bin::SubI, e, c126);
        let mask = self.ci(0x807f_ffffu32 as i32);
        let m = self.ib(Bin::AndI, bits, mask);
        let half = self.ci(0x3f00_0000);
        let m = self.ib(Bin::OrI, m, half);
        let m = self.un(Ty::F32, Un::BitsIF, m);
        let sqrthf = self.cf(std::f32::consts::FRAC_1_SQRT_2);
        let small = self.cmpf(Cmp::Lt, m, sqrthf);
        let one_i = self.ci(1);
        let em1 = self.ib(Bin::SubI, e, one_i);
        let e = self.sel(small, em1, e);
        let m2 = self.add(m, m);
        let a = self.addc(m2, -1.0);
        let b = self.addc(m, -1.0);
        let x1 = self.sel(small, a, b);
        let z = self.mul(x1, x1);
        let p = self.poly(
            x1,
            &[
                7.037_683_6e-2, -1.151_461e-1, 1.167_699_9e-1, -1.242_014_1e-1, 1.424_932_3e-1, -1.666_805_8e-1,
                2.000_071_4e-1, -2.499_999_4e-1, 3.333_333e-1,
            ],
        );
        let p = self.mul(p, x1);
        let y = self.mul(p, z);
        let fe = self.un(Ty::F32, Un::I2F, e);
        let t = self.mulc(fe, -2.121_944_4e-4);
        let y = self.add(y, t);
        let hz = self.mulc(z, 0.5);
        let y = self.sub(y, hz);
        let r = self.add(x1, y);
        let t = self.mulc(fe, 0.693_359_4);
        let r = self.add(r, t);
        let zero = self.cf(0.0);
        let pos = self.cmpf(Cmp::Gt, x, zero);
        let ninf = self.cf(f32::NEG_INFINITY);
        self.sel(pos, r, ninf)
    }

    fn tanh(&mut self, x: Val) -> Val {
        let z = self.fabs(x);
        let z2 = self.add(z, z);
        let e = self.exp(z2);
        let e1 = self.addc(e, 1.0);
        let two = self.cf(2.0);
        let q = self.div(two, e1);
        let one = self.cf(1.0);
        let big = self.sub(one, q);
        let nbig = self.fneg(big);
        let zero = self.cf(0.0);
        let xneg = self.cmpf(Cmp::Lt, x, zero);
        let big = self.sel(xneg, nbig, big);
        let s = self.mul(x, x);
        let p = self.poly(s, &[-5.704_988_7e-3, 2.063_908_9e-2, -5.373_971_6e-2, 1.333_144_2e-1, -3.333_328_2e-1]);
        let p = self.mul(p, s);
        let p = self.mul(p, x);
        let small = self.add(p, x);
        let lim = self.cf(0.625);
        let is_big = self.cmpf(Cmp::Gt, z, lim);
        self.sel(is_big, big, small)
    }

    /// atan: reduce |x| to [0, tan(pi/8)] (x > 2.414 -> pi/2 - atan(1/x),
    /// x > 0.414 -> pi/4 + atan((x-1)/(x+1))), then a degree-9 polynomial.
    fn atan(&mut self, x: Val) -> Val {
        let ax = self.fabs(x);
        let hi = self.cf(2.414_213_6);
        let mid = self.cf(0.414_213_57);
        let is_hi = self.cmpf(Cmp::Gt, ax, hi);
        let is_mid = self.cmpf(Cmp::Gt, ax, mid);
        let one = self.cf(1.0);
        let m1 = self.cf(-1.0);
        let inv = self.div(m1, ax);
        let num = self.sub(ax, one);
        let den = self.add(ax, one);
        let q = self.div(num, den);
        let r = self.sel(is_mid, q, ax);
        let r = self.sel(is_hi, inv, r);
        let zero = self.cf(0.0);
        let pi4 = self.cf(std::f32::consts::FRAC_PI_4);
        let pi2 = self.cf(std::f32::consts::FRAC_PI_2);
        let y0 = self.sel(is_mid, pi4, zero);
        let y0 = self.sel(is_hi, pi2, y0);
        let z = self.mul(r, r);
        let p = self.poly(z, &[8.053_744_5e-2, -1.387_768_6e-1, 1.997_771_1e-1, -3.333_295e-1]);
        let p = self.mul(p, z);
        let p = self.mul(p, r);
        let p = self.add(p, r);
        let y = self.add(y0, p);
        let xneg = self.cmpf(Cmp::Lt, x, zero);
        let ny = self.fneg(y);
        self.sel(xneg, ny, y)
    }

    /// atan2(y, x) in -pi..pi (atan2(0, 0) = 0).
    fn atan2(&mut self, y: Val, x: Val) -> Val {
        let q = self.div(y, x);
        let a = self.atan(q);
        let zero = self.cf(0.0);
        let pi = self.cf(std::f32::consts::PI);
        let npi = self.cf(-std::f32::consts::PI);
        let xneg = self.cmpf(Cmp::Lt, x, zero);
        let yneg = self.cmpf(Cmp::Lt, y, zero);
        let fix = self.sel(yneg, npi, pi);
        let a2 = self.add(a, fix);
        let a = self.sel(xneg, a2, a);
        // x == 0: +-pi/2 by the sign of y (0 when y is 0 too).
        let xz = self.cmpf(Cmp::Eq, x, zero);
        let h = self.cf(std::f32::consts::FRAC_PI_2);
        let nh = self.cf(-std::f32::consts::FRAC_PI_2);
        let yz = self.cmpf(Cmp::Eq, y, zero);
        let v = self.sel(yneg, nh, h);
        let v = self.sel(yz, zero, v);
        self.sel(xz, v, a)
    }

    fn pow(&mut self, a: Val, b: Val) -> Val {
        let l = self.log(a);
        let m = self.mul(b, l);
        let e = self.exp(m);
        let zero = self.cf(0.0);
        let pos = self.cmpf(Cmp::Gt, a, zero);
        self.sel(pos, e, zero)
    }

    fn floor_to_i(&mut self, x: Val) -> Val {
        let f = self.un(Ty::F32, Un::FloorF, x);
        self.un(Ty::I32, Un::F2I, f)
    }
}

/// Removes definitions nobody reads (every op is pure), to a fixpoint.
fn dce(p: &mut Program) {
    loop {
        let mut used = vec![false; p.vals.len()];
        fn mark(b: &Block, used: &mut Vec<bool>) {
            for s in b {
                let us: Vec<Val> = match s {
                    IS::Def(_, op) => ir::op_uses(op),
                    IS::Set(_, v) => vec![*v],
                    IS::Store { off, val, .. } => off.iter().copied().chain([*val]).collect(),
                    IS::Out { idx, val, .. } => vec![*idx, *val],
                    IS::If(c, t, e) => {
                        mark(t, used);
                        mark(e, used);
                        vec![*c]
                    }
                    IS::Loop { body, .. } => {
                        mark(body, used);
                        vec![]
                    }
                    IS::CallHost { args, slices, .. } => args.iter().copied().chain(slices.iter().flat_map(|x| [x.off, x.len])).collect(),
                    IS::Call { args, .. } | IS::Port { args, .. } => args.clone(),
                    _ => vec![],
                };
                for u in us {
                    used[u.0 as usize] = true;
                }
            }
        }
        mark(&p.body, &mut used);
        let mut removed = false;
        fn sweep(b: &mut Block, used: &[bool], removed: &mut bool) {
            b.retain(|s| match s {
                IS::Def(v, _) if !used[v.0 as usize] => {
                    *removed = true;
                    false
                }
                _ => true,
            });
            for s in b.iter_mut() {
                match s {
                    IS::If(_, t, e) => {
                        sweep(t, used, removed);
                        sweep(e, used, removed);
                    }
                    IS::Loop { body, .. } => sweep(body, used, removed),
                    _ => {}
                }
            }
        }
        sweep(&mut p.body, &used, &mut removed);
        if !removed {
            return;
        }
    }
}

// =========================================================================
// The lowerer
// =========================================================================

/// What the render entry exposes to expressions.
#[derive(Clone)]
struct EntryCtx {
    rate: Var,
    // Phase and number of sub-samples actually run within this base frame.
    subsample: Option<(Val, u32)>,
    pos: Var,
    tempo: Val,
    beat_at: Val,
    beat_frame: Val,
    beat_step: Val,
    bar_beats: Val,
    playing: Val,
    voice_index: Val,
    instance: Val,
    note: Var,
    freq: Var,
    gate: Var,
    velocity: Var,
    bend: Var,
    pressure: Var,
    slide: Var,
    release_velocity: Var,
    mod_wheel: Var,
    trigger_at: Var,
    rng: Var,
    stop: Var,
    params: Vec<(Var, Var)>,
}

// Optional arguments and results used while lowering the audio frame loop.
relax_api::option_layouts! {
    Glide,
    (Val, u32),
    (Place, Lanes),
    AudioRate,
}

#[derive(Clone, Copy)]
struct Glide {
    remaining: Var,
    signed_step: Val,
    target: Val,
}

struct Lowerer<'control> {
    control: Option<&'control AtomicU32>,
    block_layout: Option<block::Layout>,
    fft_sites: Vec<fft::Site>,
    render_entries: Vec<(u64, Span)>,
    ports: Vec<AudioPort>,
    audio_channels: [u16; 2],
    // The base-frame index remains in scope inside an explicit rate region.
    port_frame: Option<Val>,
    mono: Option<Span>,
    latency: u32,
    oversampling: u32,
    rate_modes: RateModes,
    rate_reads_input: bool,
    rate_nests: RateNests,
    reads_note: bool,
    reads_freq: bool,
    b: Builder,
    structs: Vec<StructDef>,
    fns: HashMap<String, FnDecl>,
    globals: HashMap<String, Bind>,
    /// Per inlined call: a stack of lexical scopes.
    frames: Vec<Vec<HashMap<String, Bind>>>,
    loops: Vec<LoopKind>,
    rets: Vec<Ret>,
    // Distinguish anonymous functions and shadowed helpers by their definition.
    call_stack: Vec<(String, Span)>,
    params: Vec<ParamInfo>,
    /// In first-use order, with the last, minimum and maximum for this render call.
    probes: Vec<(String, [Var; 3])>,
    param_meta: Vec<ParamMeta>,
    state_init: Vec<u32>,
    state_vars: Vec<StateVar>,
    /// Promoted state scalars: (state word, var).
    promoted: Vec<(u32, Var)>,
    shared_init: Vec<u32>,
    sample_names: Vec<String>,
    frame_words: u32,
    entry: Option<EntryCtx>,
    /// Lowering `init()`: shared tables are writable, inputs absent.
    in_init: bool,
    /// Spans at or past this lie in the prelude.
    prelude_base: usize,
    // cost: off creates no marks; on adds one Stmt per user statement and condition.
    marks: bool,
    marks_spans: Vec<Span>,
    /// The marks of `marks_spans` that name a source condition ([`Lowerer::cond_site`]).
    cond_marks: Vec<(u32, bool)>,
    domain: Domain,
    /// Kernel front end: buffers, emit counters, the element value.
    kernel: kernel::KernelCtx,
    /// `math: portable`: f32 math functions evaluate in f64 with the
    /// fdlibm kernels of `makepad_csg_math::portable` and round once.
    portable: bool,
    /// `let ranges = whole`: a run-time `for` range has no cap.
    whole_ranges: bool,
    /// Helpers lowered as functions (the program's `funcs`), each with
    /// what its calls need ([`FnMeta`]).
    funcs_out: Vec<(Program, FnMeta)>,
    /// Per helper and argument kinds (literal arguments by their bits):
    /// its function (None: inlined).
    func_cache: HashMap<(String, Vec<u8>, Vec<u64>), Option<u16>>,
    /// Call sites per helper name in the source (counted once).
    call_sites: Option<HashMap<String, usize>>,
    /// Top-level tables holding plain objects, by root: their constant
    /// image, so a read by constant path (`CAM.pos.y`, `L[1].power`)
    /// folds to the constant.
    object_tables: HashMap<u32, CInit>,
    /// Evaluating a top-level table: a plain object's field that is not a
    /// constant is kept as unreadable (an error where read) instead of
    /// failing the whole `let`.
    lenient_objects: bool,
    /// Edit mode: the kernel's live literals (see [`LiveLift`]).
    live: Option<LiveLift>,
    /// A traced lowering: the calls so far (`trace.rs`).
    trace: Option<trace::TraceScope>,
    /// The event front end's declarations, ports and sites.
    event: event::EventCtx,
    /// The spans of the calls being inlined, innermost last.
    call_spans: Vec<Span>,
    /// Per helper name: whether each parameter may take an untyped literal as an i32 value
    /// ([`Lowerer::lit_safe`]).
    lit_safe: HashMap<String, Vec<bool>>,
    /// Lowering an event function: its i32 parameters that may stand for a literal, and the
    /// ones whose uses showed they may not ([`Taint`]).
    taint: Option<Taint>,
}

/// An event function's i32 parameters that a call may fill with an untyped literal,
/// by value, and a bit per parameter whose uses would compute differently from the literal's
/// (an f64 fold): those calls keep a copy.
#[derive(Default)]
struct Taint {
    vals: HashMap<Val, u32>,
    bad: u64,
}

/// Edit mode's live literals in a kernel: the source offsets of the number
/// and colour literals an editor may change (`live`, from the host), and
/// what lowering made of each: a hidden parameter the kernel reads (so a
/// changed value is a parameter write, the same code), or a constant it
/// had to stay (a loop bound, a size, an integer: `folded`).
#[derive(Default)]
pub struct LiveLift {
    pub live: std::collections::HashSet<usize>,
    /// Colour literals' offsets: their four channels lift apart.
    pub colours: std::collections::HashSet<usize>,
    /// Per (offset, channel): the hidden parameter.
    pub slots: HashMap<(usize, u8), u32>,
    /// Channels seen so far per colour offset (a colour is lowered as its
    /// four channels in order).
    seen: HashMap<usize, u32>,
    pub folded: Vec<usize>,
}

/// A literal a kernel reads from a hidden parameter: its source offset, its
/// channel (a colour's), and the parameter.
#[derive(Clone, Debug, PartialEq)]
pub struct LiveParam {
    pub offset: usize,
    pub channel: Option<u8>,
    pub param: u32,
}

/// How a call of a lowered function passes and returns values.
/// A state scalar an event function takes by value: its name, type, variable types, and whether
/// the function assigns it (then it is also returned).
type StateArg = (String, T, Vec<Ty>, bool);

#[derive(Clone)]
struct FnMeta {
    /// Its params end with the element index and the emit counters'
    /// values; its results with the counters' new values.
    emits: bool,
    /// The value it returns (its leading results; None: nothing).
    ret: Option<T>,
    /// Event programs: the state scalars it reads or assigns, passed by value after the other
    /// params (one per variable of each), the assigned ones (true) returned after the other
    /// results; the call site carries them in and out of the caller's promoted variables.
    state: Vec<(String, bool)>,
    /// Event programs: the frame words its own locals take, from word 0 until
    /// `event::lower_events` moves them above every entry's.
    frame: u32,
    name: String,
    span: Span,
    /// The source marks allocated while lowering it (those of functions first lowered inside it
    /// included: the ranges nest).
    marks: std::ops::Range<u32>,
    /// Event programs: a bit per parameter that may not take an untyped literal as a value.
    lit_bad: u64,
}

/// A helper's function key: its name, its arguments' kinds and its literal arguments' bits (a
/// literal stays a constant of the function); None for an argument a function cannot take.
fn func_key(f: &FnDecl, vals: &[V]) -> Option<(String, Vec<u8>, Vec<u64>)> {
    let mut kinds = Vec::new();
    let mut lits = Vec::new();
    for v in vals {
        kinds.push(match v {
            V::Lit(x) => {
                lits.push(x.to_bits());
                3u8
            }
            V::F(_) => 0u8,
            V::I(_) => 1,
            V::B(_) => 2,
            V::Vec(n, _) => 10 + *n,
            _ => return None,
        });
    }
    Some((f.name.clone(), kinds, lits))
}

/// A literal that an i32 holds and an f32 holds exactly, so each conversion of it (to i32, to
/// f32, to a truth value) gives the same as the literal's own; not -0, whose f32 is not +0's.
fn exact_i32_lit(x: f64) -> bool {
    x.fract() == 0.0 && x.abs() <= (1u32 << 24) as f64 && !(x == 0.0 && x.is_sign_negative())
}

fn ty_of(t: &T) -> Ty {
    match t {
        T::F => Ty::F32,
        T::I => Ty::I32,
        T::B => Ty::Bool,
        T::D => Ty::F64,
        _ => unreachable!("aggregate has no scalar type"),
    }
}

impl Lowerer<'_> {
    /// A number literal: a constant, or, a live literal lowered in code (not
    /// while evaluating a constant), a read of its hidden parameter.
    fn num(&mut self, x: f64, int: bool, span: Span) -> V {
        let Some(mut live) = self.live.take() else { return V::Lit(x) };
        let v = self.live_num(&mut live, x, int, span);
        self.live = Some(live);
        v
    }

    fn live_num(&mut self, live: &mut LiveLift, x: f64, int: bool, span: Span) -> V {
        if !live.live.contains(&span.start) {
            return V::Lit(x);
        }
        let channel = if live.colours.contains(&span.start) {
            let n = live.seen.entry(span.start).or_insert(0);
            let c = (*n % 4) as u8;
            *n += 1;
            Some(c)
        } else {
            None
        };
        let key = (span.start, channel.unwrap_or(0));
        let in_code = !int && !self.b.scratch && !self.in_init && !self.call_stack.is_empty();
        let param = match live.slots.get(&key) {
            Some(p) => Some(*p),
            None if in_code && self.params.len() < 256 => {
                let p = self.params.len() as u32;
                self.params.push(ParamInfo { name: format!("\u{1}{}.{}", span.start, key.1), default: x as f32, min: f32::MIN, max: f32::MAX });
                live.slots.insert(key, p);
                Some(p)
            }
            None => None,
        };
        match param {
            Some(p) if in_code => V::F(self.b.load(Ty::F32, Region::Ctx, kernel::K_PARAMS + p, 1, None)),
            _ => {
                if !live.folded.contains(&span.start) {
                    live.folded.push(span.start);
                }
                V::Lit(x)
            }
        }
    }

    /// A throwaway builder for constant evaluation (sees the same vars).
    fn scratch(&self) -> Builder {
        let mut b = Builder::new();
        b.prog.vars = self.b.prog.vars.clone();
        b.scratch = true;
        b
    }

    fn words(&self, t: &T) -> u32 {
        match t {
            T::F | T::I | T::B => 1,
            T::D => 2,
            T::Vec(n) => *n as u32,
            T::Struct(s) => self.structs[*s].words,
            T::Array(e, n) => self.words(e) * n,
            T::Buf(..) => 0,
        }
    }

    fn sig(&self, t: &T) -> String {
        match t {
            T::F => "f32".into(),
            T::I => "i32".into(),
            T::B => "bool".into(),
            T::D => "f64".into(),
            T::Vec(n) => vec_name(*n).into(),
            T::Struct(s) => {
                let d = &self.structs[*s];
                let fields: Vec<String> = d.fields.iter().map(|(n, t, _, _)| format!("{}:{}", n, self.sig(t))).collect();
                format!("{}{{{}}}", d.name, fields.join(","))
            }
            T::Array(e, n) => format!("[{};{}]", self.sig(e), n),
            T::Buf(e, ..) => format!("buffer<{}>", self.sig(e)),
        }
    }

    fn tname(&self, t: &T) -> String {
        match t {
            T::Struct(s) => self.structs[*s].name.clone(),
            T::Array(e, n) => format!("[{}; {}]", self.tname(e), n),
            _ => self.sig(t),
        }
    }

    fn vname(&self, v: &V) -> String {
        match v {
            V::Lit(_) if self.event.integer_literals => "i32".into(),
            V::Lit(_) | V::F(_) => "f32".into(),
            V::I(_) => "i32".into(),
            V::B(_) => "bool".into(),
            V::D(_) => "f64".into(),
            V::Vec(n, _) => vec_name(*n).into(),
            V::Place(p) => self.tname(&p.ty),
            V::Unit => "nothing".into(),
        }
    }

    // -- scopes ------------------------------------------------------------

    fn lookup(&self, name: &str) -> Option<Bind> {
        if let Some(frame) = self.frames.last() {
            for scope in frame.iter().rev() {
                if let Some(b) = scope.get(name) {
                    return Some(b.clone());
                }
            }
        }
        self.globals.get(name).cloned()
    }

    fn bind(&mut self, name: &str, b: Bind) {
        self.frames.last_mut().unwrap().last_mut().unwrap().insert(name.to_string(), b);
    }

    fn scoped<X>(&mut self, f: impl FnOnce(&mut Self) -> LResult<X>) -> LResult<X> {
        self.frames.last_mut().unwrap().push(HashMap::new());
        let r = f(self);
        self.frames.last_mut().unwrap().pop();
        r
    }

    fn alloc_frame(&mut self, words: u32, span: Span) -> LResult<u32> {
        let at = self.frame_words;
        self.frame_words = self.frame_words.saturating_add(words);
        if self.frame_words > MAX_FRAME_WORDS {
            return err(span, format!("local arrays and structs exceed {} words; use a `var` (state) for big buffers", MAX_FRAME_WORDS));
        }
        Ok(at)
    }

    // -- value helpers -----------------------------------------------------

    fn to_f(&mut self, v: &V, span: Span) -> LResult<Val> {
        Ok(match v {
            V::Lit(x) => self.b.cf(*x as f32),
            V::F(x) => *x,
            V::I(x) => self.b.un(Ty::F32, Un::I2F, *x),
            // An f64 rounds once to f32.
            V::D(x) => self.b.d2f(*x),
            _ => return err(span, format!("expected a number, found {}", self.vname(v))),
        })
    }

    /// A number as f64 (f32 and i32 widen exactly; literals keep their
    /// full f64 value).
    fn to_d(&mut self, v: &V, span: Span) -> LResult<Val> {
        Ok(match v {
            V::Lit(x) => self.b.cd(*x),
            V::D(x) => *x,
            V::F(x) => self.b.f2d(*x),
            V::I(x) => self.b.un(Ty::F64, Un::I2D, *x),
            _ => return err(span, format!("expected a number, found {}", self.vname(v))),
        })
    }

    fn literal_i32(&self, x: f64, span: Span) -> LResult<i32> {
        if self.event.integer_literals && !(i32::MIN as f64..=i32::MAX as f64).contains(&x) {
            return err(span, format!("constant {x} does not fit an i32"));
        }
        if x.fract() != 0.0 {
            return err(span, format!("{} is not an integer", x));
        }
        Ok(x as i32)
    }

    fn to_i(&mut self, v: &V, span: Span) -> LResult<Val> {
        Ok(match v {
            V::Lit(x) => {
                let x = self.literal_i32(*x, span)?;
                self.b.ci(x)
            }
            V::I(x) => *x,
            V::F(x) => self.b.floor_to_i(*x),
            V::D(x) => {
                let f = self.b.un(Ty::F64, Un::FloorD, *x);
                self.b.un(Ty::I32, Un::D2I, f)
            }
            _ => return err(span, format!("expected an integer, found {}", self.vname(v))),
        })
    }

    fn truth(&mut self, v: &V, span: Span) -> LResult<Val> {
        Ok(match v {
            V::B(x) => *x,
            V::Lit(x) => self.b.cb(*x != 0.0),
            V::F(x) => {
                let z = self.b.cf(0.0);
                self.b.cmpf(Cmp::Ne, *x, z)
            }
            V::I(x) => {
                let z = self.b.ci(0);
                self.b.cmpi(Cmp::Ne, *x, z)
            }
            V::D(x) => {
                let z = self.b.cd(0.0);
                self.b.def(Ty::Bool, Op::CmpD(Cmp::Ne, *x, z))
            }
            _ => return err(span, format!("expected a condition, found {}", self.vname(v))),
        })
    }

    /// Reads a place of value type; aggregates stay places.
    fn read(&mut self, p: &Place) -> V {
        if let Region::Buf(k) = p.region {
            if !matches!(p.ty, T::Buf(..)) {
                self.kernel.access(k, p.off);
            }
        }
        let (base, extent, off) = self.addr(p);
        match p.ty {
            T::F => V::F(self.b.load(Ty::F32, p.region, base, extent, off)),
            T::I => V::I(self.b.load(Ty::I32, p.region, base, extent, off)),
            T::B => V::B(self.b.load(Ty::Bool, p.region, base, extent, off)),
            T::D => {
                let lo = self.b.load(Ty::I32, p.region, base, extent, off);
                let o = self.offset_plus(p, 1);
                let hi = self.b.load(Ty::I32, p.region, o.0, o.1, o.2);
                V::D(self.b.def(Ty::F64, Op::Bin(Bin::MakeD, hi, lo)))
            }
            T::Vec(n) => {
                let mut l = [Val(0); 16];
                l[0] = self.b.load(Ty::F32, p.region, base, extent, off);
                for k in 1..n as usize {
                    let o = self.offset_plus(p, k as u32);
                    l[k] = self.b.load(Ty::F32, p.region, o.0, o.1, o.2);
                }
                V::Vec(n, l)
            }
            _ => V::Place(p.clone()),
        }
    }

    /// (base, extent, dynamic offset) of the place's first word.
    fn addr(&mut self, p: &Place) -> (u32, u32, Option<Val>) {
        self.offset_plus(p, 0)
    }

    fn offset_plus(&mut self, p: &Place, k: u32) -> (u32, u32, Option<Val>) {
        match p.off {
            None => (p.root + p.stat + k, 1, None),
            Some(o) => {
                // The static part goes into the base (the dynamic offset of
                // an element never exceeds the object minus that part).
                let s = p.stat + k;
                if s < p.extent {
                    (p.root + s, p.extent - s, Some(o))
                } else {
                    let c = self.b.ci(s as i32);
                    let off = self.b.ib(Bin::AddI, o, c);
                    (p.root, p.extent, Some(off))
                }
            }
        }
    }

    fn write_word(&mut self, p: &Place, k: u32, val: Val) {
        let (base, extent, off) = self.offset_plus(p, k);
        self.b.store(p.region, base, extent, off, val);
    }

    /// Stores a value into a place of the same type (aggregates copy word
    /// by word).
    fn write(&mut self, p: &Place, v: &V, span: Span) -> LResult<()> {
        if p.region == Region::Shared && !self.in_init {
            return err(span, "tables (top-level `let` arrays) are read-only; fill them in `fn init()`");
        }
        if let Region::Buf(k) = p.region {
            if self.domain == Domain::Event {
                let b = &self.event.buffers[k as usize];
                if !b.writable {
                    return err(span, format!("buffer `{}` is read-only", self.event.decls[b.decl as usize].name));
                }
            } else {
                if !self.kernel.writable(k) {
                    return err(span, "input buffers are read-only; declare an output(...) to write");
                }
                self.kernel.access(k, p.off);
            }
        }
        match (&p.ty, v) {
            (T::F, _) => {
                let x = self.to_f(v, span)?;
                self.write_word(p, 0, x);
            }
            (T::I, V::I(_) | V::Lit(_)) => {
                let x = self.to_i(v, span)?;
                self.write_word(p, 0, x);
            }
            (T::B, V::B(x)) => self.write_word(p, 0, *x),
            (T::D, V::D(_) | V::F(_) | V::I(_) | V::Lit(_)) => {
                let x = self.to_d(v, span)?;
                let lo = self.b.un(Ty::I32, Un::LoD, x);
                let hi = self.b.un(Ty::I32, Un::HiD, x);
                self.write_word(p, 0, lo);
                self.write_word(p, 1, hi);
            }
            (T::Vec(n), _) => {
                let n = *n;
                let l = self.to_vec(v, n, span)?;
                for k in 0..n as usize {
                    self.write_word(p, k as u32, l[k]);
                }
            }
            (t, V::Place(src)) if *t == src.ty => {
                let words = self.words(t);
                if words > 256 {
                    return err(span, "copying more than 256 words at once; copy the array element by element");
                }
                for k in 0..words {
                    let leaf = self.leaf_ty(t, k);
                    let (base, extent, off) = self.offset_plus(src, k);
                    let x = self.b.load(leaf, src.region, base, extent, off);
                    self.write_word(p, k, x);
                }
            }
            _ => return err(span, format!("cannot store {} into {}", self.vname(v), self.tname(&p.ty))),
        }
        Ok(())
    }

    /// The scalar type of word `k` of type `t`.
    fn leaf_ty(&self, t: &T, k: u32) -> Ty {
        match t {
            T::F | T::Vec(_) => Ty::F32,
            T::I | T::D => Ty::I32,
            T::B => Ty::Bool,
            T::Struct(s) => {
                let d = &self.structs[*s];
                for (_, ft, off, _) in d.fields.iter().rev() {
                    if k >= *off {
                        return self.leaf_ty(ft, k - off);
                    }
                }
                unreachable!()
            }
            T::Array(e, _) => {
                let w = self.words(e);
                self.leaf_ty(e, k % w)
            }
            T::Buf(e, ..) => self.leaf_ty(e, k),
        }
    }

    fn to_v2(&mut self, v: &V, span: Span) -> LResult<(Val, Val)> {
        let l = self.to_vec(v, 2, span)?;
        Ok((l[0], l[1]))
    }

    /// A value as n lanes: a vector of that width, or a scalar broadcast.
    fn to_vec(&mut self, v: &V, n: u8, span: Span) -> LResult<Lanes> {
        match v {
            V::Vec(m, l) if *m == n => Ok(*l),
            V::Lit(_) | V::F(_) | V::I(_) | V::D(_) => {
                let x = self.to_f(v, span)?;
                Ok([x; 16])
            }
            _ => err(span, format!("expected a {}, found {}", vec_name(n), self.vname(v))),
        }
    }

    /// Swizzles and components: `v.x`, `v.zyx`, `v.xy`, `c.rgb`, `m.c2`.
    fn swizzle(&mut self, n: u8, l: &Lanes, field: &str, span: Span) -> LResult<V> {
        if n == 16 {
            if let Some(c) = field.strip_prefix('c').and_then(|c| c.parse::<usize>().ok()).filter(|c| *c < 4) {
                return Ok(V::Vec(4, lanes_of(&l[c * 4..c * 4 + 4])));
            }
            return err(span, format!("mat4 has columns c0..c3, not `{}`", field));
        }
        if let Some(k) = single_lane(n, field) {
            return Ok(V::F(l[k]));
        }
        let picked: Option<Vec<Val>> = field.chars().map(|c| lane_of(n, c).map(|k| l[k])).collect();
        match picked {
            Some(p) if (2..=4).contains(&p.len()) => Ok(V::Vec(p.len() as u8, lanes_of(&p))),
            _ => err(span, format!("{} has no component `{}` (x y z w / r g b a)", vec_name(n), field)),
        }
    }

    /// Left-associated sum of products over n lanes.
    fn dot(&mut self, a: &Lanes, b: &Lanes, n: u8) -> Val {
        let mut s = self.b.mul(a[0], b[0]);
        for k in 1..n as usize {
            let p = self.b.mul(a[k], b[k]);
            s = self.b.add(s, p);
        }
        s
    }

    /// Column-major mat4 times vec4.
    fn mat_vec(&mut self, m: &Lanes, v: &Lanes) -> V {
        let mut o = [Val(0); 16];
        for r in 0..4 {
            let mut s = self.b.mul(m[r], v[0]);
            for c in 1..4 {
                let p = self.b.mul(m[c * 4 + r], v[c]);
                s = self.b.add(s, p);
            }
            o[r] = s;
        }
        V::Vec(4, o)
    }

    /// The width of the vector among some values (None when all scalars).
    fn vec_width(&self, vals: &[&V], span: Span) -> LResult<Option<u8>> {
        let mut w = None;
        for v in vals {
            if let V::Vec(n, _) = v {
                match w {
                    None => w = Some(*n),
                    Some(m) if m == *n => {}
                    Some(m) => return err(span, format!("mixing {} and {}", vec_name(m), vec_name(*n))),
                }
            }
        }
        Ok(w)
    }

    // -- constant initializers ---------------------------------------------

    fn cinit_ty(&self, c: &CInit) -> T {
        match c {
            CInit::W(_, t) => t.clone(),
            CInit::Vec(n, _) => T::Vec(*n),
            CInit::Struct(s, _) => T::Struct(*s),
            CInit::Repeat(el, n) => T::Array(Box::new(self.cinit_ty(el)), *n),
            CInit::Array(t, v) => T::Array(Box::new(t.clone()), v.len() as u32),
        }
    }

    /// Append the image after checking its entire word range and reserving it once.
    fn flatten(&self, c: &CInit, out: &mut Vec<u32>, span: Span) -> LResult<()> {
        let words = self.words(&self.cinit_ty(c));
        reserve_words(out, words, span)?;
        self.append_init(c, out);
        Ok(())
    }

    fn append_init(&self, c: &CInit, out: &mut Vec<u32>) {
        match c {
            CInit::W(w, _) => out.push(*w),
            CInit::Vec(n, l) => out.extend_from_slice(&l[..*n as usize]),
            CInit::Struct(sid, fields) => {
                let d = &self.structs[*sid];
                let start = out.len();
                for (field, (_, _, offset, _)) in fields.iter().zip(&d.fields) {
                    out.resize(start + *offset as usize, 0);
                    self.append_init(field, out);
                }
                out.resize(start + d.words as usize, 0);
            }
            CInit::Array(_, fields) => {
                for field in fields {
                    self.append_init(field, out);
                }
            }
            CInit::Repeat(el, count) => {
                let start = out.len();
                self.append_init(el, out);
                let width = out.len() - start;
                let end = start + width * *count as usize;
                if width == 1 {
                    out.resize(end, out[start]);
                } else {
                    while out.len() < end {
                        let n = (end - out.len()).min(out.len() - start);
                        out.extend_from_within(start..start + n);
                    }
                }
            }
        }
    }

    /// Evaluates a constant initializer (a scratch builder folds every
    /// scalar, so the deterministic math works in initializers too).
    fn cinit(&mut self, e: &Expr, ann: Option<&TypeAnn>) -> LResult<CInit> {
        match &e.kind {
            ExprKind::StructLit(name, fields) if name.is_empty() => self.object_cinit(fields, e.span),
            ExprKind::StructLit(name, fields) => {
                let Some(s) = self.structs.iter().position(|d| d.name == *name) else {
                    return err(e.span, format!("unknown struct `{}`", name));
                };
                let mut vals: Vec<CInit> = self.structs[s].fields.iter().map(|f| f.3.clone()).collect();
                for (fname, fe) in fields {
                    let Some(k) = self.structs[s].fields.iter().position(|f| f.0 == *fname) else {
                        return err(fe.span, format!("`{}` has no field `{}`", name, fname));
                    };
                    let v = self.cinit(fe, None)?;
                    let want = self.structs[s].fields[k].1.clone();
                    vals[k] = self.coerce_cinit(v, &want, fe.span)?;
                }
                Ok(CInit::Struct(s, vals))
            }
            ExprKind::ArrayRepeat(el, count) => {
                let n = self.const_count(count)?;
                let el = self.cinit(el, ann)?;
                image_words(self.words(&self.cinit_ty(&el)) as u64 * u64::from(n), e.span)?;
                Ok(CInit::Repeat(Box::new(el), n))
            }
            ExprKind::ArrayList(list) => {
                let mut out = Vec::new();
                for x in list {
                    out.push(self.cinit(x, ann)?);
                }
                let t = self.cinit_ty(&out[0]);
                image_words(u64::from(self.words(&t)) * list.len() as u64, e.span)?;
                for (x, c) in list.iter().zip(out.iter_mut()) {
                    let owned = c.clone();
                    *c = self.coerce_cinit(owned, &t, x.span)?;
                }
                Ok(CInit::Array(t, out))
            }
            _ => {
                let mut scratch = self.scratch();
                std::mem::swap(&mut self.b, &mut scratch);
                let saved_entry = self.entry.take();
                let saved_frame = self.frame_words;
                let v2 = self.expr(e);
                std::mem::swap(&mut self.b, &mut scratch);
                self.entry = saved_entry;
                self.frame_words = saved_frame;
                let v2 = v2?;
                let scratch_const = |v: &V, b: &Builder| -> Option<u32> {
                    match v {
                        V::F(x) | V::I(x) | V::B(x) => b.cst(*x),
                        _ => None,
                    }
                };
                let bad = || err(e.span, "initial values must be constants (numbers, colours, vectors, constants and pure functions of them)");
                Ok(match (&v2, ann) {
                    (V::Lit(x), Some(TypeAnn::I32)) => {
                        if x.fract() != 0.0 {
                            return err(e.span, "not an integer");
                        }
                        CInit::W(self.literal_i32(*x, e.span)? as u32, T::I)
                    }
                    (V::Lit(x), Some(ann @ (TypeAnn::Vec2 | TypeAnn::Vec3 | TypeAnn::Vec4))) => {
                        let b = (*x as f32).to_bits();
                        CInit::Vec(ann_width(ann), [b; 16])
                    }
                    (V::Lit(x), None) if self.event.integer_literals => CInit::W(self.literal_i32(*x, e.span)? as u32, T::I),
                    (V::Lit(x), _) => CInit::W((*x as f32).to_bits(), T::F),
                    (V::F(_), _) => CInit::W(scratch_const(&v2, &scratch).map_or_else(bad, Ok)?, T::F),
                    (V::I(_), _) => CInit::W(scratch_const(&v2, &scratch).map_or_else(bad, Ok)?, T::I),
                    (V::B(_), _) => CInit::W(scratch_const(&v2, &scratch).map_or_else(bad, Ok)?, T::B),
                    (V::Vec(n, l), _) => {
                        let mut w = [0u32; 16];
                        for k in 0..*n as usize {
                            match scratch.cst(l[k]) {
                                Some(c) => w[k] = c,
                                None => return err(e.span, "initial values must be constants (numbers, colours, vectors, constants and pure functions of them)"),
                            }
                        }
                        CInit::Vec(*n, w)
                    }
                    _ => return err(e.span, "initial values must be constants (numbers, colours, vectors, constants and pure functions of them)"),
                })
            }
        }
    }

    /// A plain object's constant image. Strings and functions are kept as
    /// unreadable fields (kernels have neither); so is a field that is not
    /// a constant when the object is a top-level table (see
    /// `lenient_objects`), so reading the other fields works.
    fn object_cinit(&mut self, fields: &[(String, Expr)], span: Span) -> LResult<CInit> {
        let mut vals: Vec<(String, CInit)> = Vec::new();
        let mut opaque: Vec<(String, String)> = Vec::new();
        for (fname, fe) in fields {
            if vals.iter().any(|v| v.0 == *fname) || opaque.iter().any(|v| v.0 == *fname) {
                return err(fe.span, format!("field `{}` is given twice", fname));
            }
            if let Some(why) = fe.kind.unreadable_reason() {
                opaque.push((fname.clone(), format!("{} (an object a kernel reads holds numbers, vectors, colours, bools, arrays and objects of those)", why)));
                continue;
            }
            match &fe.kind {
                ExprKind::Lambda(_) => {
                    opaque.push((fname.clone(), "a function kept in an object: a kernel calls functions by name (write it as `fn name(..)` or `let name = fn(..)` at the top level)".into()));
                    continue;
                }
                ExprKind::StructLit(n, _) if !n.is_empty() && !self.structs.iter().any(|d| d.name == *n) => {
                    opaque.push((fname.clone(), format!("an object made from `{}`: a kernel sees only plain objects (`{{a: 1}}`), whose fields are all written out", n)));
                    continue;
                }
                _ => {}
            }
            let c = match self.cinit(fe, None) {
                Ok(c) => c,
                Err(e) if e.is_cancelled() => return Err(e),
                Err(e) => match self.fold_const(fe)? {
                    Some(c) => c,
                    None if self.lenient_objects => {
                        opaque.push((
                            fname.clone(),
                            format!("not a constant ({}): an object a kernel reads holds constants only; read params, inputs and the time inside the kernel's functions", e.message),
                        ));
                        continue;
                    }
                    None => return Err(e),
                },
            };
            vals.push((fname.clone(), c));
        }
        let types: Vec<(String, T)> = vals.iter().map(|(n, c)| (n.clone(), self.cinit_ty(c))).collect();
        let sid = self.object_struct(&types, opaque, span)?;
        Ok(CInit::Struct(sid, vals.into_iter().map(|v| v.1).collect()))
    }

    /// The struct of plain objects with these fields (one per shape).
    fn object_struct(&mut self, types: &[(String, T)], opaque: Vec<(String, String)>, span: Span) -> LResult<usize> {
        let same = |d: &StructDef| {
            d.name == OBJECT && d.opaque == opaque && d.fields.len() == types.len() && d.fields.iter().zip(types).all(|(f, (n, t))| f.0 == *n && f.1 == *t)
        };
        if let Some(k) = self.structs.iter().position(same) {
            return Ok(k);
        }
        let mut fields = Vec::new();
        let mut off = 0;
        for (n, t) in types {
            let init = self.zero_cinit(t);
            fields.push((n.clone(), t.clone(), off, init));
            off = image_words(u64::from(off) + u64::from(self.words(t)), span)?;
        }
        self.structs.push(StructDef { name: OBJECT.into(), fields, words: off.max(1), opaque });
        Ok(self.structs.len() - 1)
    }

    /// The zero value of a type, as an initializer.
    fn zero_cinit(&self, t: &T) -> CInit {
        match t {
            T::Vec(n) => CInit::Vec(*n, [0; 16]),
            T::Struct(s) => CInit::Struct(*s, self.structs[*s].fields.iter().map(|f| f.3.clone()).collect()),
            T::Array(e, n) => CInit::Repeat(Box::new(self.zero_cinit(e)), *n),
            t => CInit::W(0, t.clone()),
        }
    }

    /// Whether an initializer holds a plain object.
    fn has_object(&self, c: &CInit) -> bool {
        match c {
            CInit::Struct(s, f) => self.structs[*s].name == OBJECT || f.iter().any(|c| self.has_object(c)),
            CInit::Repeat(el, _) => self.has_object(el),
            CInit::Array(_, f) => f.first().is_some_and(|c| self.has_object(c)),
            _ => false,
        }
    }

    /// The constant a read of a top-level table of plain objects by a
    /// constant path gives (`CAM.pos.y`, `LIGHTS[1].power`, `P[k]` in an
    /// unrolled loop): a number reads as a literal, as the same field
    /// written as its own top-level `let` would. None: read it from the
    /// table.
    fn object_read(&mut self, e: &Expr) -> Option<V> {
        // `init()` may fill tables: their words are only known after it.
        if self.in_init || self.fns.contains_key("init") {
            return None;
        }
        fn path<'a>(l: &'a Lowerer, e: &Expr) -> Option<&'a CInit> {
            match &e.kind {
                ExprKind::Ident(n) => match l.lookup(n) {
                    Some(Bind::Place(p)) if p.region == Region::Shared && p.off.is_none() && p.stat == 0 => l.object_tables.get(&p.root),
                    _ => None,
                },
                ExprKind::Field(b, f) => match path(l, b)? {
                    CInit::Struct(s, vals) => vals.get(l.structs[*s].fields.iter().position(|x| x.0 == *f)?),
                    _ => None,
                },
                ExprKind::Index(b, i) => {
                    let k = match &i.kind {
                        // Let ordinary expression lowering report a forbidden float literal.
                        ExprKind::Num(_, false) if l.event.integer_literals => return None,
                        ExprKind::Num(x, _) => *x,
                        ExprKind::Ident(n) => match l.lookup(n) {
                            Some(Bind::Const(V::Lit(x))) => x,
                            Some(Bind::Scalar(T::I, w)) => w as i32 as f64,
                            // An unrolled kernel loop counter is an i32 constant.
                            Some(Bind::Const(V::I(v))) => l.b.cst64(v)? as u32 as i32 as f64,
                            _ => return None,
                        },
                        _ => return None,
                    };
                    match path(l, b)? {
                        CInit::Repeat(el, _) => Some(el),
                        CInit::Array(_, items) if !items.is_empty() => items.get((k.floor() as i64).rem_euclid(items.len() as i64) as usize),
                        _ => None,
                    }
                }
                _ => None,
            }
        }
        let c = path(self, e)?.clone();
        Some(match c {
            CInit::W(w, T::F) => V::Lit(f32::from_bits(w) as f64),
            CInit::W(w, T::I) => V::I(self.b.ci(w as i32)),
            CInit::W(w, T::B) => V::B(self.b.cb(w != 0)),
            CInit::Vec(n, w) => {
                let mut lanes = [Val(0); 16];
                for (k, l) in lanes.iter_mut().enumerate().take(n as usize) {
                    *l = self.b.cf(f32::from_bits(w[k]));
                }
                V::Vec(n, lanes)
            }
            _ => return None,
        })
    }

    fn coerce_cinit(&self, c: CInit, want: &T, span: Span) -> LResult<CInit> {
        let have = self.cinit_ty(&c);
        if have == *want {
            return Ok(c);
        }
        match (c, want) {
            (CInit::W(w, T::F), T::I) => {
                let x = f32::from_bits(w);
                if x.fract() != 0.0 {
                    return err(span, "expected an integer");
                }
                Ok(CInit::W(x as i32 as u32, T::I))
            }
            (CInit::W(w, T::I), T::F) => Ok(CInit::W((w as i32 as f32).to_bits(), T::F)),
            (CInit::W(w, T::F), T::Vec(n)) => Ok(CInit::Vec(*n, [w; 16])),
            // Plain objects of one array: the same fields, in any order.
            (CInit::Struct(s, vals), T::Struct(w)) if self.structs[s].name == OBJECT && self.structs[*w].name == OBJECT => {
                let (have, want_d) = (&self.structs[s], &self.structs[*w]);
                for (n, ..) in &have.fields {
                    if !want_d.fields.iter().any(|f| f.0 == *n) {
                        return err(span, format!("the objects of an array need the same fields: this one has `{}`, the first does not", n));
                    }
                }
                let mut out = Vec::new();
                for (n, t, _, _) in &want_d.fields {
                    let Some(k) = have.fields.iter().position(|f| f.0 == *n) else {
                        return err(span, format!("the objects of an array need the same fields: this one has no `{}`", n));
                    };
                    out.push(self.coerce_cinit(vals[k].clone(), t, span)?);
                }
                Ok(CInit::Struct(*w, out))
            }
            _ => err(span, format!("expected {}, found {}", self.tname(want), self.tname(&have))),
        }
    }

    fn const_count(&mut self, e: &Expr) -> LResult<u32> {
        let scratch = self.scratch();
        let saved = std::mem::replace(&mut self.b, scratch);
        let r = self.expr(e);
        self.b = saved;
        match r? {
            V::Lit(x) if x.fract() == 0.0 && x >= 1.0 && x <= u32::MAX as f64 => Ok(x as u32),
            _ => err(e.span, "array sizes must be positive integer constants (`let N = 64`)"),
        }
    }

    /// Writes a constant initializer into a place (frame locals).
    fn store_cinit(&mut self, p: &Place, c: &CInit, span: Span) -> LResult<()> {
        let mut words = Vec::new();
        self.flatten(c, &mut words, span)?;
        let t = self.cinit_ty(c);
        let n = words.len() as u32;
        let uniform = words.iter().all(|w| *w == words[0]);
        if n > 32 && uniform {
            // A fill loop instead of n stores.
            let leaf = self.leaf_ty(&t, 0);
            let j = self.b.var(Ty::I32);
            let zero = self.b.ci(0);
            self.b.set(j, zero);
            self.b.open_loop();
            let jv = self.b.get(j);
            let end = self.b.ci(n as i32);
            let done = self.b.cmpi(Cmp::Ge, jv, end);
            self.b.push(IS::If(done, vec![IS::Break(0)], vec![]));
            let x = self.b.konst(leaf, words[0] as u64);
            let (base, extent, off) = match p.off {
                None => (p.root + p.stat, n, Some(jv)),
                Some(_) => {
                    let (b, e, o) = self.offset_plus(p, 0);
                    let o = self.b.ib(Bin::AddI, o.unwrap(), jv);
                    (b, e, Some(o))
                }
            };
            self.b.push(IS::Store { region: p.region, base, extent, off, val: x });
            let one = self.b.ci(1);
            let jn = self.b.ib(Bin::AddI, jv, one);
            self.b.set(j, jn);
            let body = self.b.close();
            self.b.push(IS::Loop { cap: n + 1, body });
            return Ok(());
        }
        for (k, w) in words.iter().enumerate() {
            let leaf = self.leaf_ty(&t, k as u32);
            let x = self.b.konst(leaf, *w as u64);
            self.write_word(p, k as u32, x);
        }
        Ok(())
    }

    // -- top level -----------------------------------------------------------

    fn latency_frames(&mut self, value: &Expr, span: Span) -> LResult<u32> {
        // Check the operands before folding so arithmetic cannot hide a read of state.
        fn constant(l: &Lowerer, e: &Expr) -> bool {
            match &e.kind {
                ExprKind::Num(..) => true,
                ExprKind::Ident(name) => name == "oversampling" || matches!(l.globals.get(name), Some(Bind::Const(V::Lit(_)) | Bind::Scalar(..))),
                ExprKind::Neg(x) => constant(l, x),
                ExprKind::Bin(BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Rem, a, b) => constant(l, a) && constant(l, b),
                ExprKind::Call(name, args) if name == "len" && !l.fns.contains_key(name) => {
                    matches!(args.as_slice(), [Expr { kind: ExprKind::Ident(t), .. }] if
                        matches!(l.globals.get(t), Some(Bind::Place(Place { region: Region::Shared, ty: T::Array(..), .. }))))
                }
                _ => false,
            }
        }
        if !constant(self, value) {
            return err(span, "`latency()` needs a constant frame count");
        }
        let scratch = self.scratch();
        let saved = std::mem::replace(&mut self.b, scratch);
        let folded = self.expr(value);
        let scratch = std::mem::replace(&mut self.b, saved);
        let frames = match speculative(folded)? {
            Some(V::Lit(x)) => Some(x),
            Some(V::I(v)) => scratch.cst(v).map(|w| w as i32 as f64),
            Some(V::F(v)) => scratch.cst(v).map(|w| f32::from_bits(w) as f64),
            _ => None,
        };
        match frames {
            Some(x) if x.fract() == 0.0 && x >= 0.0 && x <= u32::MAX as f64 => Ok(x as u32),
            _ => err(span, "`latency()` needs a whole number of frames from 0 to 4294967295"),
        }
    }

    fn sample_table(&mut self, name: &str, args: &[Expr], span: Span, binding: Option<(u32, u32, &[f32])>) -> LResult<()> {
        if ["lib", "code", "mono", "at", "in", "seed"].contains(&name) {
            return err(span, format!("sample table `{name}` uses a reserved score field"));
        }
        let channels = match args {
            [count] => speculative(self.const_count(count))?,
            _ => None,
        };
        let Some(channels @ (1 | 2)) = channels else {
            return err(span, format!("sample table `{name}` needs 1 or 2 channels"));
        };
        let (source_channels, frames, data) = binding.unwrap_or((1, 1, &[0.0]));
        if !matches!(source_channels, 1 | 2) {
            return err(span, format!("sample table `{name}` needs 1 or 2 source channels, got {source_channels}"));
        }
        if frames == 0 || data.len() as u64 != u64::from(source_channels) * u64::from(frames) {
            return err(span, format!("sample table `{name}` needs {frames} nonempty frames of {source_channels} channels"));
        }
        let root = self.shared_init.len() as u32;
        let extent = image_words(u64::from(frames) * u64::from(channels), span)?;
        reserve_words(&mut self.shared_init, extent, span)?;
        if channels == source_channels {
            // SAFETY: each f32 is an aligned word, every bit pattern is valid as u32,
            // and the immutable slice is borrowed only for this copy.
            let words = unsafe { std::slice::from_raw_parts(data.as_ptr().cast::<u32>(), data.len()) };
            self.shared_init.extend_from_slice(words);
        } else if channels == 1 {
            self.shared_init.extend(data.chunks_exact(2).map(|lr| ((lr[0] + lr[1]) * 0.5).to_bits()));
        } else {
            for x in data {
                self.shared_init.extend_from_slice(&[x.to_bits(); 2]);
            }
        }
        let element = if channels == 1 { T::F } else { T::Vec(2) };
        let ty = T::Array(Box::new(element), frames);
        self.globals.insert(name.to_owned(), Bind::Place(Place { region: Region::Shared, root, extent, off: None, stat: 0, ty }));
        self.sample_names.push(name.to_owned());
        Ok(())
    }

    fn top(&mut self, items: &[Item]) -> LResult<()> {
        self.top_with_samples(items, &|_| None, None)
    }

    fn top_with_samples<'a>(&mut self, items: &[Item], sample: &dyn Fn(&str) -> Option<(u32, u32, &'a [f32])>, header: Option<Vec<Option<AudioDecl>>>) -> LResult<()> {
        for item in items {
            if let Item::Fn(f) = item {
                if self.fns.insert(f.name.clone(), f.clone()).is_some() {
                    return err(f.span, format!("`{}` is defined twice", f.name));
                }
            }
        }
        // A block-only audio node has only its named outputs in the I/O table.
        if self.domain == Domain::Audio && self.fns.contains_key("block")
            && !["voice", "effect", "effect_block"].iter().any(|name| self.fns.contains_key(*name)) {
            self.audio_channels = [0, 0];
        }
        // Header words (an event program's state is its own `var`s only).
        if self.domain != Domain::Event {
            self.state_init = vec![0; HEADER_WORDS as usize];
            self.state_init[header::FRESH as usize] = 1;
            self.state_init[header::RNG as usize] = 0x9e37_79b9;
        }
        let mut latency = None;
        let mut declarations = header.into_iter().flatten();
        for item in items {
            let declaration = declarations.next().flatten();
            match item {
                Item::Struct { name, fields, span } => {
                    if self.structs.iter().any(|s| s.name == *name) {
                        return err(*span, format!("struct `{}` is defined twice", name));
                    }
                    let mut defs = Vec::new();
                    let mut off = 0;
                    for (fname, fe) in fields {
                        let c = self.cinit(fe, None)?;
                        let t = self.cinit_ty(&c);
                        let w = self.words(&t);
                        defs.push((fname.clone(), t, off, c));
                        off = image_words(u64::from(off) + u64::from(w), *span)?;
                    }
                    self.structs.push(StructDef { name: name.clone(), fields: defs, words: off.max(1), opaque: Vec::new() });
                    if off == 0 {
                        return err(*span, "a struct needs at least one field");
                    }
                }
                Item::Let { name, ann, value, span } => {
                    self.check_new_global(name, *span)?;
                    if let ExprKind::Call(f, args) = &value.kind {
                        match declaration {
                            Some(AudioDecl::Port { output, width }) => {
                                self.audio_port(name, output, width?, *span)?;
                                continue;
                            }
                            Some(AudioDecl::Sample) => {
                                self.sample_table(name, args, *span, sample(name))?;
                                continue;
                            }
                            Some(AudioDecl::Param { choices }) => {
                                let bind = self.param(name, args, value.span, choices)?;
                                self.globals.insert(name.clone(), bind);
                                continue;
                            }
                            _ => {}
                        }
                        if self.domain == Domain::Event {
                            if let Some(k) = self.event.forms.iter().position(|d| d.name == f) {
                                let bind = self.event_decl(name, k, args, value.span)?;
                                self.globals.insert(name.clone(), bind);
                                continue;
                            }
                            if f == "param" {
                                return err(value.span, "an event program has no params: declare a constant, or keep the value in a `var`");
                            }
                        }
                        if self.domain == Domain::Kernel && matches!(f.as_str(), "input" | "output" | "emit_buffer" | "param") {
                            let bind = self.kernel_decl(name, f, args, value.span)?;
                            self.globals.insert(name.clone(), bind);
                            continue;
                        }
                    }
                    if matches!(value.kind, ExprKind::ArrayRepeat(..) | ExprKind::ArrayList(..) | ExprKind::StructLit(..)) {
                        self.lenient_objects = self.domain == Domain::Kernel;
                        let c = self.cinit(value, ann.as_ref());
                        self.lenient_objects = false;
                        let c = c?;
                        let t = self.cinit_ty(&c);
                        let root = self.shared_init.len() as u32;
                        if self.has_object(&c) {
                            self.object_tables.insert(root, c.clone());
                        }
                        let mut w = std::mem::take(&mut self.shared_init);
                        self.flatten(&c, &mut w, *span)?;
                        self.shared_init = w;
                        let extent = self.words(&t);
                        self.globals.insert(
                            name.clone(),
                            Bind::Place(Place { region: Region::Shared, root, extent, off: None, stat: 0, ty: t }),
                        );
                        continue;
                    }
                    let scratch = self.scratch();
        let saved = std::mem::replace(&mut self.b, scratch);
                    let r = self.expr(value);
                    self.b = saved;
                    match r? {
                        V::Lit(x) => {
                            let bind = match ann {
                                Some(TypeAnn::I32) if x.fract() != 0.0 || x < i32::MIN as f64 || x > i32::MAX as f64 => {
                                    return err(*span, format!("`{}` is declared i32, but {} is not an i32", name, x));
                                }
                                Some(TypeAnn::I32) => Bind::Scalar(T::I, x as i32 as u32),
                                Some(TypeAnn::F32) => Bind::Scalar(T::F, (x as f32).to_bits()),
                                _ => Bind::Const(V::Lit(x)),
                            };
                            self.globals.insert(name.clone(), bind);
                        }
                        _ => {
                            // A folded non-literal constant (e.g. `sin(1)`,
                            // `linear_rgb(#eee9df)`): the builder's folding,
                            // else the expression run once at lowering.
                            let c = match self.cinit(value, ann.as_ref()) {
                                Ok(c) => c,
                                Err(e) if e.is_cancelled() => return Err(e),
                                Err(e) => match self.fold_const(value)? {
                                    Some(c) => c,
                                    None if self.domain == Domain::Kernel => {
                                        return err(
                                            *span,
                                            format!(
                                                "`{}` is not a constant: a top-level `let` holds a number, colour or vector computed from constants only (numbers, colours, other constants and pure functions of them); read params, inputs and the time inside the kernel's functions",
                                                name
                                            ),
                                        )
                                    }
                                    None => return Err(e),
                                },
                            };
                            let bind = match (c, ann) {
                                (CInit::W(w, T::I), Some(TypeAnn::I32)) => Bind::Scalar(T::I, w),
                                (CInit::W(w, T::F), Some(TypeAnn::F32)) => Bind::Scalar(T::F, w),
                                (CInit::W(w, T::F), _) => Bind::Const(V::Lit(f32::from_bits(w) as f64)),
                                (CInit::W(w, T::I), _) => Bind::Const(V::Lit(w as i32 as f64)),
                                (CInit::Vec(n, w), _) => Bind::ConstVec(n, w),
                                _ => return err(*span, "a top-level `let` must be a constant (a number, a vec2/3/4 or colour, a pure function of constants), a `param(...)` or a table; use `var` for state"),
                            };
                            self.globals.insert(name.clone(), bind);
                        }
                    }
                }
                Item::Var { name, ann, value, span } => {
                    if self.domain == Domain::Kernel {
                        return err(*span, "kernels have no state that persists between elements: use `let` inside the kernel, or write an output buffer");
                    }
                    self.check_new_global(name, *span)?;
                    let c = self.cinit(value, ann.as_ref())?;
                    self.add_state(name, c, *span)?;
                }
                Item::Decl { name, .. } if self.domain == Domain::Audio && name == "rate" => {}
                Item::Decl { name, args, span } if self.domain == Domain::Audio && name == "latency" => {
                    let [value] = args.as_slice() else { return err(*span, "`latency()` takes one argument") };
                    if latency.replace((value, *span)).is_some() {
                        return err(*span, "`latency()` declared twice");
                    }
                }
                Item::Decl { name, args, span } => {
                    if !matches!(declaration, Some(AudioDecl::Mono)) {
                        return err(*span, format!("unknown declaration `{name}()`"));
                    }
                    if !args.is_empty() {
                        return err(*span, "`mono()` takes no arguments");
                    }
                    if self.mono.replace(*span).is_some() {
                        return err(*span, "`mono()` declared twice");
                    }
                    if self.fns.contains_key("effect") || self.fns.contains_key("effect_block") {
                        return err(*span, "`mono()` is for instruments");
                    }
                }
                Item::Fn(_) => {}
                // Resolved before lowering (kernels); audio has no modules.
                Item::Use { .. } => {}
            }
        }
        if let Some((value, span)) = latency {
            self.latency = self.latency_frames(value, span)?;
        }
        Ok(())
    }

    fn audio_port(&mut self, name: &str, output: bool, channels: u16, span: Span) -> LResult<()> {
        if self.oversampling > 1 {
            return err(span, format!("audio port `{name}` needs a base-rate shader: ports at a raised rate (rate(k) or a node's oversample:) are not supported yet; run the high-rate part in oversample(...)"));
        }
        let count = &mut self.audio_channels[usize::from(output)];
        let first = *count;
        *count = first.checked_add(channels).ok_or_else(|| ShaderError::new(span.start, span.end,
            format!("audio port `{name}` exceeds the program's 16-bit channel count")))?;
        let held = if output {
            let start = self.promoted.len();
            let zero = if channels == 1 { CInit::W(0, T::F) } else { CInit::Vec(2, [0; 16]) };
            self.add_state(name, zero, span)?;
            self.promoted[start..].iter().map(|&(_, var)| var).collect()
        } else {
            Vec::new()
        };
        self.globals.insert(name.to_owned(), Bind::AudioPort(self.ports.len()));
        self.ports.push(AudioPort {
            info: AudioPortInfo { name: name.to_owned(), output, channels, first, control: false },
            span, held, frame_write: false,
        });
        Ok(())
    }

    fn add_state(&mut self, name: &str, c: CInit, span: Span) -> LResult<()> {
        let t = self.cinit_ty(&c);
        let offset = self.state_init.len() as u32;
        let mut w = std::mem::take(&mut self.state_init);
        self.flatten(&c, &mut w, span)?;
        self.state_init = w;
        let words = self.words(&t);
        self.state_vars.push(StateVar { name: name.to_owned(), span, sig: self.sig(&t), offset, words });
        let bind = match t {
            T::F | T::I | T::B => {
                let v = self.b.var(ty_of(&t));
                self.promoted.push((offset, v));
                Bind::Local(t, vec![v])
            }
            T::Vec(n) => {
                let vars: Vec<Var> = (0..n).map(|_| self.b.var(Ty::F32)).collect();
                for (k, v) in vars.iter().enumerate() {
                    self.promoted.push((offset + k as u32, *v));
                }
                Bind::Local(T::Vec(n), vars)
            }
            t => Bind::Place(Place { region: Region::State, root: offset, extent: words, off: None, stat: 0, ty: t }),
        };
        self.globals.insert(name.to_owned(), bind);
        Ok(())
    }

    fn add_rate_cascade(&mut self, kind: Kind, span: Span) -> LResult<()> {
        let factor = self.oversampling;
        let name = if kind == Kind::Effect { format!("Oversample{factor}") } else { format!("Halfband{factor}") };
        let initial = self.cinit(&Expr { kind: ExprKind::StructLit(name, Vec::new()), span }, None)?;
        let effect = kind == Kind::Effect;
        let cascade = if effect { initial } else { CInit::Repeat(Box::new(initial), 2) };
        self.rate_reads_input = self.fns.get("effect").is_some_and(|entry| {
            entry.params.iter().any(|(name, _)| reads_parameter(&entry.body, name))
        });
        let delay = self.rate_modes.latency(factor, self.rate_reads_input);
        self.latency = self.latency.checked_add(delay)
            .ok_or_else(|| ShaderError::new(span.start, span.end, "declared latency plus rate conversion exceeds 4294967295 base frames".into()))?;
        self.add_state(RATE_STATE, cascade, span)
    }

    fn check_new_global(&self, name: &str, span: Span) -> LResult<()> {
        if self.globals.contains_key(name) {
            return err(span, format!("`{}` is defined twice", name));
        }
        if BUILTIN_NAMES.contains(&name) || (self.domain == Domain::Audio && matches!(name, "oversampling" | "effect_block")) {
            return err(span, format!("`{}` is a builtin name", name));
        }
        Ok(())
    }

    fn param_number(&mut self, value: &Expr) -> LResult<f32> {
        match self.cinit(value, None)? {
            CInit::W(w, T::F) => Ok(f32::from_bits(w)),
            CInit::W(w, T::I) => Ok((w as i32) as f32),
            _ => err(value.span, "expected a constant number"),
        }
    }

    fn param(&mut self, name: &str, args: &[Expr], span: Span, choices: LResult<Vec<String>>) -> LResult<Bind> {
        let mut choices = Some(choices);
        let (numeric, fields) = param_parts(args);
        let (default, mut min, mut max) = match numeric {
            [d] => (d, f32::MIN, f32::MAX),
            [d, lo, hi] => (d, self.param_number(lo)?, self.param_number(hi)?),
            _ => return err(span, "param takes a default, optional min and max, and an optional info object"),
        };
        let mut meta = ParamMeta::default();
        let mut log_span = None;
        for (i, (field, value)) in fields.iter().enumerate() {
            let span = value.span;
            if fields[..i].iter().any(|(key, _)| key == field) {
                return err(span, format!("param field `{field}` is given twice"));
            }
            let text = |e: &Expr| param_word(field, e);
            match field.as_str() {
                "unit" => meta.unit = match text(value)?.as_str() {
                    "hz" => ParamUnit::Hz,
                    "s" => ParamUnit::Secs,
                    "ms" => ParamUnit::Ms,
                    "db" => ParamUnit::Db,
                    "st" => ParamUnit::Semis,
                    "ct" => ParamUnit::Cents,
                    "beats" => ParamUnit::Beats,
                    "percent" | "%" => ParamUnit::Percent,
                    unit => return err(span, format!("unknown unit `{unit}` (hz, s, ms, db, st, ct, beats, percent or \"%\")")),
                },
                "curve" => meta.curve = match text(value)?.as_str() {
                    "linear" => ParamCurve::Linear,
                    "log" => { log_span = Some(span); ParamCurve::Log }
                    "exp" => ParamCurve::Exp,
                    curve => return err(span, format!("unknown curve `{curve}` (linear, log, exp)")),
                },
                "label" => meta.label = text(value)?,
                "group" => meta.group = text(value)?,
                "step" | "smooth" => {
                    let is_step = field == "step";
                    let number = speculative(self.param_number(value))?.filter(|x| x.is_finite() && if is_step { *x > 0.0 } else { *x >= 0.0 });
                    let Some(number) = number else {
                        return err(span, if is_step { "step needs a constant number > 0" } else { "smooth needs a constant number >= 0" });
                    };
                    if is_step { meta.step = number } else { meta.smooth = Some(number) }
                }
                "choices" => {
                    meta.choices = choices.take().unwrap()?;
                    min = 0.0;
                    max = (meta.choices.len() - 1) as f32;
                }
                _ => return err(span, format!("unknown param field `{field}` (unit, curve, step, label, group, choices, smooth)")),
            }
        }
        if let Some(span) = log_span {
            if !(min > 0.0) {
                return err(span, "the log curve needs min > 0");
            }
        }
        let default = if meta.choices.is_empty() {
            self.param_number(default)?
        } else {
            meta.step = 1.0;
            match &default.kind {
                ExprKind::Str(word) | ExprKind::Ident(word) => match meta.choices.iter().position(|choice| choice == word) {
                    Some(index) => index as f32,
                    None => return err(default.span, format!("unknown default choice `{word}`")),
                },
                _ => {
                    let index = self.param_number(default)?;
                    if index.fract() != 0.0 || !(min..=max).contains(&index) {
                        return err(default.span, format!("the default choice must be an index from 0 to {max}"));
                    }
                    index
                }
            }
        };
        self.params.push(ParamInfo { name: name.to_string(), default, min, max });
        self.param_meta.push(meta);
        Ok(Bind::Param(self.params.len() - 1))
    }

    // -- entries -------------------------------------------------------------

    fn load_promoted(&mut self) {
        for (word, var) in self.promoted.clone() {
            let ty = self.b.prog.vars[var.0 as usize];
            let x = self.b.load(ty, Region::State, word, 1, None);
            self.b.set(var, x);
        }
    }

    fn store_promoted(&mut self) {
        for (word, var) in self.promoted.clone() {
            let x = self.b.get(var);
            self.b.store(Region::State, word, 1, None, x);
        }
    }

    fn header_var(&mut self, ty: Ty, word: u32, store_back: bool) -> Var {
        let v = self.b.var(ty);
        if store_back {
            self.promoted.push((word, v));
        } else {
            let x = self.b.load(ty, Region::State, word, 1, None);
            self.b.set(v, x);
        }
        v
    }

    fn render(&mut self, kind: Kind) -> LResult<Span> {
        let entry_name = match kind {
            Kind::Instrument => "voice",
            Kind::Effect if self.block_layout.is_some() => "effect_block",
            Kind::Effect => "effect",
            Kind::Control => "block",
        };
        let entry = self.fns.get(entry_name).cloned().unwrap();
        let fresh_word = header::FRESH;
        // Header and param smoothing words are promoted like state scalars.
        let rng = self.header_var(Ty::I32, header::RNG, true);
        let stop = self.header_var(Ty::I32, header::STOP, true);
        let fresh = self.header_var(Ty::I32, fresh_word, true);
        let mut params = Vec::new();
        for k in 0..self.params.len() as u32 {
            let base = self.header_var(Ty::F32, HEADER_WORDS + 2 * k, true);
            let step = self.header_var(Ty::F32, HEADER_WORDS + 2 * k + 1, true);
            params.push((base, step));
        }
        self.load_promoted();
        let base_note = self.b.load(Ty::F32, Region::State, header::NOTE, 1, None);
        let bend = self.header_var(Ty::F32, header::BEND, false);
        let semitones = self.b.get(bend);
        let nv = self.b.add(base_note, semitones);
        let note = self.b.var(Ty::F32);
        self.b.set(note, nv);
        // freq from note with the deterministic exp (not the host's libm).
        let freq = self.b.var(Ty::F32);
        let hz = self.note_hz(nv);
        self.b.set(freq, hz);
        let gate = self.header_var(Ty::F32, header::GATE, false);
        let velocity = self.header_var(Ty::F32, header::VELOCITY, false);
        let pressure = self.header_var(Ty::F32, header::PRESSURE, false);
        let slide = self.header_var(Ty::F32, header::SLIDE, false);
        let release_velocity = self.header_var(Ty::F32, header::RELEASE_VELOCITY, false);
        let mod_wheel = self.b.var(Ty::F32);
        let wheel = self.b.load(Ty::F32, Region::Ctx, CTX_CC + 1, 1, None);
        self.b.set(mod_wheel, wheel);
        let trigger_at = self.header_var(Ty::I32, header::TRIGGER, false);
        let rate = self.b.var(Ty::F32);
        let r = self.b.load(Ty::F32, Region::Ctx, CTX_RATE, 1, None);
        let r = if self.oversampling == 1 { r } else { self.b.mulc(r, self.oversampling as f32) };
        self.b.set(rate, r);
        let frame0 = self.b.load(Ty::I32, Region::Ctx, CTX_FRAME, 1, None);
        let n = self.b.raw(Ty::I32, Op::FrameCount, None);
        let i = self.b.var(Ty::I32);
        let zero = self.b.ci(0);
        self.b.set(i, zero);
        let pos = self.b.var(Ty::I32);
        let tempo = self.b.load(Ty::F32, Region::Ctx, CTX_TEMPO, 1, None);
        let beat_at = self.b.load(Ty::F32, Region::Ctx, CTX_BEAT_AT, 1, None);
        let beat_frame = self.b.load(Ty::I32, Region::Ctx, CTX_BEAT_FRAME, 1, None);
        let beat_step = self.b.load(Ty::F32, Region::Ctx, CTX_BEAT_STEP, 1, None);
        let bar_beats = self.b.load(Ty::F32, Region::Ctx, CTX_BAR_BEATS, 1, None);
        let playing = self.b.load(Ty::I32, Region::Ctx, CTX_PLAYING, 1, None);
        let playing = self.b.cmpi(Cmp::Ne, playing, zero);
        let voice_index = self.b.load(Ty::I32, Region::State, header::VOICE, 1, None);
        let instance = self.b.load(Ty::F32, Region::Ctx, CTX_INSTANCE, 1, None);
        self.entry = Some(EntryCtx {
            subsample: None,
            rate, pos, note, freq, gate, velocity, bend, pressure, slide, release_velocity, mod_wheel,
            tempo, beat_at, beat_frame, beat_step, bar_beats, playing, voice_index, instance,
            trigger_at, rng, stop, params: params.clone(),
        });

        let ent = self.entry.clone().unwrap();
        let steady = self.frame_loop(&entry, kind, &ent, fresh, i, frame0, n, zero, None)?;
        let loop_of = |body| IS::Loop { cap: MAX_FRAMES + 1, body };
        let frames = if kind == Kind::Instrument && (self.reads_note || self.reads_freq) {
            let step = self.b.load(Ty::F32, Region::State, header::GLIDE_STEP, 1, None);
            let zf = self.b.cf(0.0);
            let moving = self.b.cmpf(Cmp::Ne, step, zf);
            self.b.open();
            let target = self.b.un(Ty::F64, Un::F2D, base_note);
            let step64 = self.b.un(Ty::F64, Un::F2D, step);
            let pitch = self.b.load(Ty::F32, Region::State, header::GLIDE_NOTE, 1, None);
            let estimated = self.glide_frames(target, pitch, step64);
            let correction = self.b.load(Ty::F32, Region::State, header::GLIDE_REMAINDER, 1, None);
            let correction = self.b.un(Ty::F64, Un::F2D, correction);
            let distance = self.b.def(Ty::F64, Op::Bin(Bin::AddD, estimated, correction));
            let left = self.b.un(Ty::F64, Un::AbsD, distance);
            let remaining = self.b.var(Ty::F64);
            self.b.set(remaining, left);
            let zd = self.b.cd(0.0);
            let rising = self.b.def(Ty::Bool, Op::CmpD(Cmp::Gt, distance, zd));
            let down = self.b.un(Ty::F64, Un::NegD, step64);
            let glide = Glide { remaining, signed_step: self.b.sel(rising, step64, down), target };
            let sliding = self.frame_loop(&entry, kind, &ent, fresh, i, frame0, n, zero, Some(glide))?;
            self.b.push(loop_of(sliding));
            let left = self.b.get(remaining);
            let pitch = self.glide_pitch(glide, left);
            self.b.store(Region::State, header::GLIDE_NOTE, 1, None, pitch);
            let estimated = self.glide_frames(target, pitch, step64);
            let negative = self.b.un(Ty::F64, Un::NegD, left);
            let distance = self.b.sel(rising, left, negative);
            let correction = self.b.def(Ty::F64, Op::Bin(Bin::SubD, distance, estimated));
            let correction = self.b.un(Ty::F32, Un::D2F, correction);
            self.b.store(Region::State, header::GLIDE_REMAINDER, 1, None, correction);
            let done = self.b.def(Ty::Bool, Op::CmpD(Cmp::Eq, left, zd));
            let next_step = self.b.sel(done, zf, step);
            self.b.store(Region::State, header::GLIDE_STEP, 1, None, next_step);
            let slide = self.b.close();
            IS::If(moving, slide, vec![loop_of(steady)])
        } else {
            loop_of(steady)
        };
        // The calls in the body have now declared all their accumulators.
        for (_, vars) in &self.probes {
            for (var, initial) in vars.iter().zip([f32::NAN, f32::INFINITY, f32::NEG_INFINITY]) {
                let value = self.b.cf(initial);
                self.b.set(*var, value);
            }
        }
        self.b.push(frames);
        self.store_promoted();
        let mut word = CTX_PARAMS + self.params.len() as u32;
        for (_, vars) in &self.probes {
            for var in vars {
                let value = self.b.get(*var);
                self.b.store(Region::Ctx, word, 1, None, value);
                word += 1;
            }
        }
        Ok(entry.span)
    }

    fn note_hz(&mut self, note: Val) -> Val {
        let d = self.b.addc(note, -69.0);
        let y = self.b.mulc(d, std::f32::consts::LN_2 / 12.0);
        let e = self.b.exp(y);
        self.b.mulc(e, 440.0)
    }

    // The rounded pitch accounts for the whole samples it can represent.
    // Carry the difference in the spare frequency word so slicing never
    // loses a fraction of a sample, even when a pitch step is below one ulp.
    fn glide_frames(&mut self, target: Val, pitch: Val, step: Val) -> Val {
        let pitch = self.b.un(Ty::F64, Un::F2D, pitch);
        let distance = self.b.def(Ty::F64, Op::Bin(Bin::SubD, target, pitch));
        let frames = self.b.def(Ty::F64, Op::Bin(Bin::DivD, distance, step));
        self.b.un(Ty::F64, Un::RoundD, frames)
    }

    fn glide_pitch(&mut self, glide: Glide, remaining: Val) -> Val {
        let distance = self.b.def(Ty::F64, Op::Bin(Bin::MulD, remaining, glide.signed_step));
        let pitch = self.b.def(Ty::F64, Op::Bin(Bin::SubD, glide.target, distance));
        self.b.un(Ty::F32, Un::D2F, pitch)
    }

    fn frame_loop(&mut self, entry: &FnDecl, kind: Kind, ent: &EntryCtx, fresh: Var, i: Var,
        frame0: Val, n: Val, zero: Val, glide: Option<Glide>) -> LResult<Block> {
        // The frame loop.
        self.b.open_loop();
        self.loops.push(LoopKind::Frame);
        let iv = self.b.get(i);
        let done = self.b.cmpi(Cmp::Ge, iv, n);
        self.b.push(IS::If(done, vec![IS::Break(0)], vec![]));
        if let Some(glide) = glide {
            let count = self.b.get(glide.remaining);
            let zd = self.b.cd(0.0);
            let moving = self.b.def(Ty::Bool, Op::CmpD(Cmp::Gt, count, zd));
            self.b.open();
            let one = self.b.cd(1.0);
            let next = self.b.def(Ty::F64, Op::Bin(Bin::SubD, count, one));
            let rest = self.b.def(Ty::F64, Op::Bin(Bin::MaxD, next, zd));
            self.b.set(glide.remaining, rest);
            let pitch = self.glide_pitch(glide, rest);
            let bend = self.b.get(ent.bend);
            let sounding = self.b.add(pitch, bend);
            self.b.set(ent.note, sounding);
            if self.reads_freq {
                let hz = self.note_hz(sounding);
                self.b.set(ent.freq, hz);
            }
            let advance = self.b.close();
            self.b.push(IS::If(moving, advance, vec![]));
        }
        let p = self.b.ib(Bin::AddI, frame0, iv);
        self.b.set(ent.pos, p);
        let mask = self.b.ci(CONTROL_BLOCK as i32 - 1);
        let phase = self.b.ib(Bin::AndI, p, mask);
        let at_start = self.b.cmpi(Cmp::Eq, phase, zero);
        let fr = self.b.get(fresh);
        let is_fresh = self.b.cmpi(Cmp::Ne, fr, zero);
        let run_block = self.b.def(Ty::Bool, Op::Bin(Bin::OrB, at_start, is_fresh));
        // Control block start: params ramp toward their targets; block().
        self.b.open_loop();
        for (k, (base, step)) in ent.params.iter().enumerate() {
            let target = self.b.load(Ty::F32, Region::Ctx, CTX_PARAMS + k as u32, 1, None);
            let meta = &self.param_meta[k];
            if meta.smooth == Some(0.0) || (meta.smooth.is_none() && meta.step > 0.0) {
                self.b.set(*base, target);
                let zero = self.b.cf(0.0);
                self.b.set(*step, zero);
                continue;
            }
            let bv = self.b.get(*base);
            let sv = self.b.get(*step);
            let s64 = self.b.mulc(sv, CONTROL_BLOCK as f32);
            let reached = self.b.add(bv, s64);
            let nb = self.b.sel(is_fresh, target, reached);
            self.b.set(*base, nb);
            let d = self.b.sub(target, nb);
            let ns = if let Some(seconds) = meta.smooth {
                let rate = self.b.load(Ty::F32, Region::Ctx, CTX_RATE, 1, None);
                let frames = self.b.mulc(rate, seconds);
                let block = self.b.cf(CONTROL_BLOCK as f32);
                let frames = self.b.fb(Bin::MaxF, frames, block);
                let one = self.b.cf(1.0);
                let fraction = self.b.div(one, frames);
                self.b.mul(d, fraction)
            } else {
                self.b.mulc(d, 1.0 / CONTROL_BLOCK as f32)
            };
            let zf = self.b.cf(0.0);
            let ns = self.b.sel(is_fresh, zf, ns);
            self.b.set(*step, ns);
        }
        self.b.set(fresh, zero);
        if let Some(block_fn) = self.fns.get("block").cloned() {
            if !block_fn.params.is_empty() {
                return err(block_fn.span, "`fn block()` takes no parameters");
            }
            let v = self.call(&block_fn, vec![], block_fn.span)?;
            let _ = v;
        }
        let blk = self.b.close();
        self.b.push(IS::If(run_block, blk, vec![]));

        let iv2 = if kind == Kind::Control { self.b.get(i) } else { self.main_sample(entry, kind, iv, i)? };
        for port in &self.ports {
            for (lane, &var) in port.held.iter().enumerate() {
                let val = self.b.get(var);
                self.b.push(IS::Out { ch: port.info.first + lane as u16, idx: iv2, val });
            }
        }
        let one = self.b.ci(1);
        let inext = self.b.ib(Bin::AddI, iv2, one);
        self.b.set(i, inext);
        self.loops.pop();
        Ok(self.b.close())
    }

    /// Render the stereo sample for an instrument or effect, before writing its named outputs.
    fn main_sample(&mut self, entry: &FnDecl, kind: Kind, iv: Val, i: Var) -> LResult<Val> {
        if let Some(layout) = self.block_layout {
            self.block_sample(layout, entry, iv)?;
            return Ok(iv);
        }
        self.port_frame = Some(iv);
        let args = if kind == Kind::Instrument {
            if !entry.params.is_empty() {
                return err(entry.span, "`fn voice()` takes no parameters (use note, freq, gate, velocity)");
            }
            vec![]
        } else {
            if entry.params.len() != 2 {
                return err(entry.span, "`fn effect(l, r)` takes the left and right input");
            }
            let l = self.b.raw(Ty::F32, Op::In { ch: 0, idx: iv }, None);
            let r = self.b.raw(Ty::F32, Op::In { ch: 1, idx: iv }, None);
            vec![V::F(l), V::F(r)]
        };
        let out = if self.oversampling == 1 {
            self.call(entry, args, entry.span)?
        } else {
            let Some(Bind::Place(state)) = self.globals.get(RATE_STATE).cloned() else { unreachable!() };
            let (up, down) = if kind == Kind::Effect {
                let down = self.field_place(&state, "down", entry.span)?;
                let input = [self.to_f(&args[0], entry.span)?, self.to_f(&args[1], entry.span)?];
                (self.rate_reads_input.then_some((state, lanes_of(&input))), down)
            } else {
                (None, state)
            };
            let output = self.rate_region(1, self.oversampling, self.rate_modes, up, down, 2, entry.span, |l, input| {
                let args = if kind == Kind::Effect { vec![V::F(input[0]), V::F(input[1])] } else { vec![] };
                let v = l.call(entry, args, entry.span)?;
                l.to_vec(&v, 2, entry.span)
            })?;
            V::Vec(2, output)
        };
        let (l, r) = match &out {
            V::Unit => return err(entry.span, format!("`fn {}` must return the output sample (f32 or vec2)", entry.name)),
            v => self.to_v2(v, entry.span)?,
        };
        let iv2 = self.b.get(i);
        self.b.push(IS::Out { ch: 0, idx: iv2, val: l });
        self.b.push(IS::Out { ch: 1, idx: iv2, val: r });
        self.port_frame = None;
        Ok(iv2)
    }

    fn init_program(&mut self, f: &FnDecl) -> LResult<Program> {
        let saved_b = std::mem::replace(&mut self.b, Builder::new());
        let saved_frame = std::mem::replace(&mut self.frame_words, 0);
        let saved_entry = self.entry.take();
        // State `var`s are per instance: init sees only tables and constants.
        let saved_globals = self.globals.clone();
        // Nor host buffers, emit buffers or params: init() runs once at
        // compile time with none of them bound.
        self.globals.retain(|_, b| {
            !matches!(b, Bind::Local(..) | Bind::Emit(..) | Bind::Uniform(_) | Bind::Param(_))
                && !matches!(b, Bind::Place(p) if matches!(p.region, Region::State | Region::Buf(_)))
        });
        self.in_init = true;
        if !f.params.is_empty() {
            return err(f.span, "`fn init()` takes no parameters");
        }
        let r = self.call(f, vec![], f.span);
        self.in_init = false;
        self.globals = saved_globals;
        self.entry = saved_entry;
        let frame_words = std::mem::replace(&mut self.frame_words, saved_frame);
        let b = std::mem::replace(&mut self.b, saved_b);
        r?;
        let mut prog = b.prog;
        prog.body = b.blocks.into_iter().next().unwrap();
        prog.frame_words = frame_words;
        // FFT argument offsets remain live through their call markers until expansion.
        if self.fft_sites.is_empty() {
            dce(&mut prog);
            check_init_cost(&prog, f.span)?;
        }
        Ok(prog)
    }

    // -- calls (inlining) ------------------------------------------------------

    fn call(&mut self, f: &FnDecl, args: Vec<V>, span: Span) -> LResult<V> {
        if self.b.prog.vals.len() > MAX_IR_VALS {
            return err(span, "the program is too large after inlining (fewer or smaller helper calls, or smaller unrolled loops)");
        }
        if self.call_stack.iter().any(|(name, def)| *name == f.name && *def == f.span) {
            return err(span, format!("`{}` calls itself; audio shaders have no recursion", f.name));
        }
        if args.len() != f.params.len() {
            return err(span, format!("`{}` takes {} arguments, got {}", f.name, f.params.len(), args.len()));
        }
        if let Some(v) = self.try_outline(f, &args, span)? {
            return Ok(v);
        }
        self.inline_call(f, args, span, Vec::new())
    }

    /// Inlines a call of `f`; `outer`: the scopes a local function was
    /// defined in (it sees them, under its parameters).
    fn inline_call(&mut self, f: &FnDecl, args: Vec<V>, span: Span, outer: Vec<HashMap<String, Bind>>) -> LResult<V> {
        if self.call_stack.iter().any(|(name, def)| *name == f.name && *def == f.span) {
            return err(span, format!("`{}` calls itself; kernels and audio shaders have no recursion", f.name));
        }
        if args.len() != f.params.len() {
            return err(span, format!("`{}` takes {} arguments, got {}", f.name, f.params.len(), args.len()));
        }
        let mut scope = HashMap::new();
        for ((name, ann), a) in f.params.iter().zip(args) {
            let bind = match (a, ann) {
                (V::Place(p), _) => Bind::Place(p),
                (V::Lit(x), None) => Bind::Const(V::Lit(x)),
                (v, ann) => {
                    let v = match ann {
                        Some(TypeAnn::F32) => V::F(self.to_f(&v, span)?),
                        Some(TypeAnn::F64) => V::D(self.to_d(&v, span)?),
                        Some(TypeAnn::I32) => V::I(self.to_i(&v, span)?),
                        Some(ann @ (TypeAnn::Vec2 | TypeAnn::Vec3 | TypeAnn::Vec4 | TypeAnn::Mat4)) => {
                            let n = ann_width(ann);
                            V::Vec(n, self.to_vec(&v, n, span)?)
                        }
                        _ => match v {
                            V::Lit(_) if self.event.integer_literals => V::I(self.to_i(&v, span)?),
                            V::Lit(x) => V::F(self.b.cf(x as f32)),
                            v => v,
                        },
                    };
                    // A parameter the function never assigns is its value
                    // (no copy): the element index stays recognisably the
                    // element's own.
                    if assigns(&f.body, name) {
                        self.new_local(&v, span)?
                    } else {
                        Bind::Value(v)
                    }
                }
            };
            scope.insert(name.clone(), bind);
        }
        let entries_before = self.render_entries.len();
        self.call_stack.push((f.name.clone(), f.span));
        self.call_spans.push(span);
        let mut frame = outer;
        frame.push(scope);
        self.frames.push(frame);
        self.rets.push(Ret { vars: None, used: false });
        self.loops.push(LoopKind::Wrapper);
        self.b.open();
        // A traced lowering lowers a helper whose one return is its last statement as that
        // value, no wrapper: its calls stay in the block (`trace.rs`).
        let r = match f.body.split_last() {
            Some((Stmt::Return(Some(e), _), rest)) if self.trace.is_some() && !trace::has_return(rest) && !trace::returns_in(e) => {
                self.stmts(rest).and_then(|_| {
                    let v = self.expr(e)?;
                    if !matches!(v, V::Unit) && self.value_t(&v).is_none() {
                        return err(e.span, "functions return f32, i32, bool, vec2/3/4 or mat4 (structs and arrays are passed by reference instead)");
                    }
                    Ok(v)
                })
            }
            _ => self.stmts(&f.body),
        };
        let mut body = self.b.close();
        // The render dispatcher calls each entry at its declaration span.
        // Fused dispatchers do the same for each authored node. Prefer those
        // inner entries to the synthetic wrapper when attributing a warning.
        if self.domain == Domain::Audio && self.entry.is_some() && !self.in_init
            && span == f.span && self.render_entries.len() == entries_before {
            let entry = Program { body, ..Default::default() };
            self.render_entries.push((entry.total_cost(), span));
            body = entry.body;
        }
        self.loops.pop();
        let ret = self.rets.pop().unwrap();
        self.frames.pop();
        self.call_stack.pop();
        self.call_spans.pop();
        let v = match r {
            Ok(v) => v,
            // An error inside a prelude helper is reported at the user's call.
            Err(e) if !e.is_cancelled() && e.start >= self.prelude_base && span.start < self.prelude_base => {
                return err(span, format!("in `{}`: {}", f.name, e.message));
            }
            Err(e) => return Err(e),
        };
        if !ret.used {
            self.b.splice(body);
            return Ok(v);
        }
        let (t, vars) = ret.vars.clone().unwrap_or((T::F, vec![]));
        let mut body = body;
        // The fall-through value (if any) is the last return.
        if !matches!(ir::unmarked(&body).next_back(), Some(IS::Break(_))) {
            if vars.is_empty() {
                if !matches!(v, V::Unit) {
                    return err(span, format!("`{}` returns a value in some paths only", f.name));
                }
            } else {
                if matches!(v, V::Unit) {
                    return err(f.span, format!("`{}` must end with a value (its `return`s give one)", f.name));
                }
                self.b.open();
                self.assign_vars(&t, &vars, &v, f.span)?;
                let tail = self.b.close();
                body.extend(tail);
            }
        }
        self.zero_vars(&vars);
        self.b.push(IS::Loop { cap: 1, body });
        if vars.is_empty() {
            return Ok(V::Unit);
        }
        Ok(self.read_vars(&t, &vars))
    }

    /// Calls of helper `name` in the source (every function's body).
    fn sites(&mut self, name: &str) -> usize {
        if self.call_sites.is_none() {
            fn expr(e: &Expr, out: &mut HashMap<String, usize>) {
                match &e.kind {
                    ExprKind::Call(n, args) => {
                        *out.entry(n.clone()).or_default() += 1;
                        args.iter().for_each(|a| expr(a, out));
                    }
                    ExprKind::Field(a, _) | ExprKind::Neg(a) | ExprKind::Not(a) => expr(a, out),
                    ExprKind::Index(a, b) | ExprKind::Bin(_, a, b) | ExprKind::ArrayRepeat(a, b) => {
                        expr(a, out);
                        expr(b, out);
                    }
                    ExprKind::ArrayList(xs) => xs.iter().for_each(|x| expr(x, out)),
                    ExprKind::StructLit(_, fs) => fs.iter().for_each(|(_, x)| expr(x, out)),
                    ExprKind::Block(b) => stmts(b, out),
                    ExprKind::Lambda(f) => stmts(&f.body, out),
                    ExprKind::If(arms, else_) => {
                        for (c, b) in arms {
                            expr(c, out);
                            stmts(b, out);
                        }
                        if let Some(b) = else_ {
                            stmts(b, out);
                        }
                    }
                    ExprKind::Match(x, arms) => {
                        expr(x, out);
                        arms.iter().for_each(|(_, b)| stmts(b, out));
                    }
                    _ => {}
                }
            }
            fn stmts(b: &[Stmt], out: &mut HashMap<String, usize>) {
                for s in b {
                    match s {
                        Stmt::Let { value, .. } | Stmt::Expr(value) => expr(value, out),
                        Stmt::Assign { target, value, .. } => {
                            expr(target, out);
                            expr(value, out);
                        }
                        Stmt::Return(Some(e), _) => expr(e, out),
                        Stmt::For { from, to, body, .. } => {
                            expr(from, out);
                            expr(to, out);
                            stmts(body, out);
                        }
                        Stmt::While { cond, body, .. } => {
                            expr(cond, out);
                            stmts(body, out);
                        }
                        Stmt::Loop { body, .. } => stmts(body, out),
                        _ => {}
                    }
                }
            }
            let mut out = HashMap::new();
            // An event declaration's function is a handler, not a named `fn` (unless it names one).
            let handlers = self.event.handlers.iter().filter(|h| self.fns.get(&h.name).is_none_or(|g| g.span != h.span));
            for f in self.fns.values().chain(handlers) {
                stmts(&f.body, &mut out);
            }
            self.call_sites = Some(out);
        }
        self.call_sites.as_ref().unwrap().get(name).copied().unwrap_or(0)
    }

    /// Per parameter of `f`: whether an untyped literal argument may go in as an i32 value.
    /// Its name may appear only as a port's argument (converted as the literal would be) or as
    /// an operand of a comparison (an int compare of whole numbers is the literal's f64 one; a
    /// fractional other side is the `Taint`'s to catch). Anything else (arithmetic that would
    /// fold in f64, a helper or a builtin, an index, a bound, an assignment, a `let` copy, a
    /// condition, a method on it) keeps the literal.
    fn lit_safe(&mut self, f: &FnDecl) -> Vec<bool> {
        if let Some(x) = self.lit_safe.get(&f.name) {
            return x.clone();
        }
        struct Cx<'a> {
            name: &'a str,
            l: &'a Lowerer<'a>,
        }
        fn is(e: &Expr, name: &str) -> bool {
            matches!(&e.kind, ExprKind::Ident(n) if n == name)
        }
        fn expr(e: &Expr, cx: &Cx) -> bool {
            use BinOp::*;
            match &e.kind {
                ExprKind::Ident(n) => n != cx.name,
                ExprKind::Call(n, args) => {
                    let port = !cx.l.fns.contains_key(n) && cx.l.event.ports.iter().any(|p| p.name == n);
                    n.split('.').next() != Some(cx.name) && args.iter().all(|a| (port && is(a, cx.name)) || expr(a, cx))
                }
                ExprKind::Bin(Lt | Le | Gt | Ge | Eq | Ne, a, b) => (is(a, cx.name) || expr(a, cx)) && (is(b, cx.name) || expr(b, cx)),
                ExprKind::Bin(_, a, b) | ExprKind::Index(a, b) | ExprKind::ArrayRepeat(a, b) => expr(a, cx) && expr(b, cx),
                ExprKind::Field(a, _) | ExprKind::Neg(a) | ExprKind::Not(a) => expr(a, cx),
                ExprKind::ArrayList(xs) => xs.iter().all(|x| expr(x, cx)),
                ExprKind::StructLit(_, fs) => fs.iter().all(|(_, x)| expr(x, cx)),
                ExprKind::Block(b) => stmts(b, cx),
                ExprKind::Lambda(g) => g.params.iter().all(|(n, _)| n != cx.name) && stmts(&g.body, cx),
                ExprKind::If(arms, else_) => arms.iter().all(|(c, b)| expr(c, cx) && stmts(b, cx)) && else_.as_ref().is_none_or(|b| stmts(b, cx)),
                ExprKind::Match(x, arms) => expr(x, cx) && arms.iter().all(|(ps, b)| ps.as_ref().is_none_or(|ps| ps.iter().all(|p| expr(p, cx))) && stmts(b, cx)),
                ExprKind::Num(..) | ExprKind::Bool(_) | ExprKind::Str(_) | ExprKind::Unreadable(_) => true,
            }
        }
        fn stmts(b: &[Stmt], cx: &Cx) -> bool {
            b.iter().all(|s| match s {
                Stmt::Let { name, value, .. } => name != cx.name && expr(value, cx),
                Stmt::Assign { target, value, .. } => expr(target, cx) && expr(value, cx),
                Stmt::Expr(e) | Stmt::Return(Some(e), _) => expr(e, cx),
                Stmt::For { var, from, to, body, .. } => var != cx.name && expr(from, cx) && expr(to, cx) && stmts(body, cx),
                Stmt::While { cond, body, .. } => expr(cond, cx) && stmts(body, cx),
                Stmt::Loop { body, .. } => stmts(body, cx),
                Stmt::Break(_) | Stmt::Continue(_) | Stmt::Return(None, _) => true,
            })
        }
        let safe: Vec<bool> = f.params.iter().map(|(name, ann)| ann.is_none() && stmts(&f.body, &Cx { name, l: self })).collect();
        self.lit_safe.insert(f.name.clone(), safe.clone());
        safe
    }

    /// Marks the tainted values among `a` and `b` (i32 parameters that may stand for a literal)
    /// whose use by `op` (None: not an operator) would compute differently from the literal's.
    fn taint_use(&mut self, op: Option<BinOp>, a: &V, b: &V) {
        use BinOp::*;
        let Some(t) = &mut self.taint else { return };
        let bit = |v: &V| match v {
            V::I(x) => t.vals.get(x).map_or(0, |&k| 1u64 << k),
            _ => 0,
        };
        let bits = bit(a) | bit(b);
        if bits == 0 {
            return;
        }
        let fractional = |v: &V| matches!(v, V::Lit(y) if y.fract() != 0.0);
        if !matches!(op, Some(Lt | Le | Gt | Ge | Eq | Ne)) || fractional(a) || fractional(b) {
            t.bad |= bits;
        }
    }

    /// The state scalars (`Bind::Local` globals) that `f` or a helper it calls (transitively, by
    /// name, global closures included) names, in name order; a local of the same name counts too,
    /// which only passes one value more.
    fn state_in(&self, f: &FnDecl) -> Vec<StateArg> {
        use std::collections::{BTreeSet, HashSet};
        struct W<'a> {
            l: &'a Lowerer<'a>,
            seen: HashSet<String>,
            named: BTreeSet<String>,
            set: HashSet<String>,
        }
        impl W<'_> {
            fn call(&mut self, n: &str) {
                let body = match (self.l.fns.get(n), self.l.globals.get(n)) {
                    (Some(g), _) => &g.body,
                    (None, Some(Bind::Closure(g, _))) => &g.body,
                    _ => return,
                };
                if self.seen.insert(n.to_string()) {
                    self.stmts(body);
                }
            }
            fn expr(&mut self, e: &Expr) {
                match &e.kind {
                    ExprKind::Num(..) | ExprKind::Bool(_) | ExprKind::Str(_) | ExprKind::Unreadable(_) => {}
                    ExprKind::Ident(n) => {
                        self.named.insert(n.clone());
                    }
                    ExprKind::Call(n, args) => {
                        self.named.insert(n.clone());
                        args.iter().for_each(|a| self.expr(a));
                        self.call(n);
                    }
                    ExprKind::Field(a, _) | ExprKind::Neg(a) | ExprKind::Not(a) => self.expr(a),
                    ExprKind::Index(a, b) | ExprKind::Bin(_, a, b) | ExprKind::ArrayRepeat(a, b) => {
                        self.expr(a);
                        self.expr(b);
                    }
                    ExprKind::ArrayList(xs) => xs.iter().for_each(|x| self.expr(x)),
                    ExprKind::StructLit(_, fs) => fs.iter().for_each(|(_, x)| self.expr(x)),
                    ExprKind::Block(b) => self.stmts(b),
                    ExprKind::Lambda(g) => self.stmts(&g.body),
                    ExprKind::If(arms, else_) => {
                        for (c, b) in arms {
                            self.expr(c);
                            self.stmts(b);
                        }
                        if let Some(b) = else_ {
                            self.stmts(b);
                        }
                    }
                    ExprKind::Match(x, arms) => {
                        self.expr(x);
                        for (pats, b) in arms {
                            pats.iter().flatten().for_each(|p| self.expr(p));
                            self.stmts(b);
                        }
                    }
                }
            }
            fn stmts(&mut self, b: &[Stmt]) {
                for s in b {
                    match s {
                        Stmt::Let { value, .. } | Stmt::Expr(value) => self.expr(value),
                        Stmt::Assign { target, value, .. } => {
                            let mut root = target;
                            while let ExprKind::Index(a, _) | ExprKind::Field(a, _) = &root.kind {
                                root = a;
                            }
                            if let ExprKind::Ident(n) = &root.kind {
                                self.set.insert(n.clone());
                            }
                            self.expr(target);
                            self.expr(value);
                        }
                        Stmt::Return(e, _) => e.iter().for_each(|e| self.expr(e)),
                        Stmt::For { from, to, body, .. } => {
                            self.expr(from);
                            self.expr(to);
                            self.stmts(body);
                        }
                        Stmt::While { cond, body, .. } => {
                            self.expr(cond);
                            self.stmts(body);
                        }
                        Stmt::Loop { body, .. } => self.stmts(body),
                        Stmt::Break(_) | Stmt::Continue(_) => {}
                    }
                }
            }
        }
        let mut w = W { l: self, seen: HashSet::from([f.name.clone()]), named: BTreeSet::new(), set: HashSet::new() };
        w.stmts(&f.body);
        let mut out = Vec::new();
        for n in &w.named {
            if let Some(Bind::Local(t, vars)) = self.globals.get(n) {
                let tys = vars.iter().map(|v| self.b.prog.vars[v.0 as usize]).collect();
                out.push((n.clone(), t.clone(), tys, w.set.contains(n)));
            }
        }
        out
    }

    /// Whether `f` (or a helper it calls) emits.
    fn emits_in(&self, f: &FnDecl) -> bool {
        fn walk(l: &Lowerer, b: &[Stmt], seen: &mut Vec<String>) -> bool {
            fn expr(l: &Lowerer, e: &Expr, seen: &mut Vec<String>) -> bool {
                match &e.kind {
                    ExprKind::Call(n, args) => {
                        n == "emit"
                            || args.iter().any(|a| expr(l, a, seen))
                            || match l.fns.get(n) {
                                Some(g) if !seen.contains(n) => {
                                    seen.push(n.clone());
                                    walk(l, &g.body, seen)
                                }
                                _ => false,
                            }
                    }
                    ExprKind::Field(a, _) | ExprKind::Neg(a) | ExprKind::Not(a) => expr(l, a, seen),
                    ExprKind::Index(a, b) | ExprKind::Bin(_, a, b) | ExprKind::ArrayRepeat(a, b) => expr(l, a, seen) || expr(l, b, seen),
                    ExprKind::ArrayList(xs) => xs.iter().any(|x| expr(l, x, seen)),
                    ExprKind::StructLit(_, fs) => fs.iter().any(|(_, x)| expr(l, x, seen)),
                    ExprKind::Block(b) => walk(l, b, seen),
                    ExprKind::Lambda(f) => walk(l, &f.body, seen),
                    ExprKind::If(arms, else_) => arms.iter().any(|(c, b)| expr(l, c, seen) || walk(l, b, seen)) || else_.as_ref().is_some_and(|b| walk(l, b, seen)),
                    ExprKind::Match(x, arms) => expr(l, x, seen) || arms.iter().any(|(_, b)| walk(l, b, seen)),
                    _ => false,
                }
            }
            b.iter().any(|s| match s {
                Stmt::Let { value, .. } | Stmt::Expr(value) => expr(l, value, seen),
                Stmt::Assign { target, value, .. } => expr(l, target, seen) || expr(l, value, seen),
                Stmt::Return(Some(e), _) => expr(l, e, seen),
                Stmt::For { from, to, body, .. } => expr(l, from, seen) || expr(l, to, seen) || walk(l, body, seen),
                Stmt::While { cond, body, .. } => expr(l, cond, seen) || walk(l, body, seen),
                Stmt::Loop { body, .. } => walk(l, body, seen),
                _ => false,
            })
        }
        walk(self, &f.body, &mut vec![f.name.clone()])
    }

    /// A call of a helper lowered as a function (once per argument kinds),
    /// when it is big and called from several places; None: inline it.
    fn try_outline(&mut self, f: &FnDecl, args: &[V], span: Span) -> LResult<Option<V>> {
        // Only helpers of kernel element code, or of event programs whose spec asks for
        // functions (`EventSpec::call_ops`), never the entry itself; a traced lowering inlines
        // every helper (`trace.rs`).
        let event = self.domain == Domain::Event;
        if self.in_init || self.call_stack.is_empty() || self.trace.is_some() {
            return Ok(None);
        }
        if event {
            // Buffer parameters name static regions; a run-time function parameter
            // cannot carry that binding, so these helpers always inline.
            if args.iter().any(|a| matches!(a, V::Place(Place { ty: T::Buf(..), .. }))) {
                return Ok(None);
            }
            // State scalars (`Bind::Local`) go in and out by value; nothing else may be a value.
            if self.event.call_ops.is_none() || self.globals.values().any(|b| matches!(b, Bind::Value(_))) {
                return Ok(None);
            }
        } else if self.domain != Domain::Kernel || self.kernel.element.is_none() || self.globals.values().any(|b| matches!(b, Bind::Value(_) | Bind::Local(..))) {
            return Ok(None);
        }
        // Parameter annotations convert at the call, as when inlined.
        let mut vals = Vec::new();
        for ((_, ann), a) in f.params.iter().zip(args) {
            vals.push(match (ann, a) {
                (Some(TypeAnn::F32), a) => V::F(self.to_f(a, span)?),
                (Some(TypeAnn::I32), a) => V::I(self.to_i(a, span)?),
                (Some(ann @ (TypeAnn::Vec2 | TypeAnn::Vec3 | TypeAnn::Vec4 | TypeAnn::Mat4)), a) => {
                    let n = ann_width(ann);
                    V::Vec(n, self.to_vec(a, n, span)?)
                }
                (Some(_), _) => return Ok(None),
                (None, a) => a.clone(),
            });
        }
        // Event programs: an untyped literal argument goes in as an i32 value where
        // nothing can compute differently from the inlined literal (`lit_safe`, then the uses'
        // `Taint`), so calls with different constants share one function.
        let mut typed = 0u64;
        if event {
            let safe = self.lit_safe(f);
            for (k, v) in vals.iter().enumerate() {
                if let (V::Lit(x), true) = (v, k < 64 && f.params[k].1.is_none() && safe[k]) {
                    if exact_i32_lit(*x) {
                        typed |= 1 << k;
                    }
                }
            }
        }
        if typed != 0 {
            let tvals: Vec<V> = vals.iter().enumerate().map(|(k, v)| if typed >> k & 1 != 0 { V::I(Val(0)) } else { v.clone() }).collect();
            if let Some(idx) = self.typed_function(f, &tvals, typed, span)? {
                for (k, v) in vals.iter_mut().enumerate() {
                    if let (V::Lit(x), true) = (&*v, typed >> k & 1 != 0) {
                        *v = V::I(self.b.ci(*x as i32));
                    }
                }
                if vals.iter().any(|v| matches!(v, V::Lit(_))) {
                    self.count_copy(f);
                }
                return Ok(Some(self.outlined_call(idx, &vals)));
            }
        }
        // Otherwise a literal argument stays a constant in the function (it folds there as when
        // inlined): one function per literal values, or the call inlined.
        if event && vals.iter().zip(&f.params).any(|(v, (_, ann))| ann.is_none() && matches!(v, V::Lit(_))) {
            self.count_copy(f);
        }
        if !vals.is_empty() && vals.iter().all(|v| matches!(v, V::Lit(_))) {
            return Ok(None);
        }
        let Some(key) = func_key(f, &vals) else { return Ok(None) };
        let idx = match self.func_cache.get(&key) {
            Some(x) => *x,
            None => {
                let x = if self.big(f) { self.lower_function(f, &vals, span)? } else { None };
                self.func_cache.insert(key, x);
                x
            }
        };
        let Some(idx) = idx else { return Ok(None) };
        Ok(Some(self.outlined_call(idx, &vals)))
    }

    /// Whether `f` is kept as a function: called from two places or more, and bigger than the
    /// inlining limit. An event program never shares a library (prelude) helper: a port in a shared
    /// function has one site, which would name the prelude instead of the user's call, and the
    /// library's port wrappers are one-liners that cost no more inlined.
    fn big(&mut self, f: &FnDecl) -> bool {
        if self.domain == Domain::Event && f.span.start >= self.prelude_base {
            return false;
        }
        let limit = if self.domain == Domain::Event { self.event.call_ops.unwrap_or(u64::MAX) } else { INLINE_OPS };
        self.sites(&f.name) >= 2 && self.ast_cost(&f.body) > limit
    }

    /// A call that keeps a copy for its literal arguments, when `f` would be shared.
    fn count_copy(&mut self, f: &FnDecl) {
        if self.big(f) {
            *self.event.copies.entry(f.name.clone()).or_default() += 1;
        }
    }

    /// The function of `f` for arguments `vals` whose `typed` ones stand for literals; None when
    /// those may not (its uses compute differently, or it does not lower as a function with
    /// them): nothing of the attempt stays, the caller keeps a copy.
    fn typed_function(&mut self, f: &FnDecl, vals: &[V], typed: u64, span: Span) -> LResult<Option<u16>> {
        let Some(key) = func_key(f, vals) else { return Ok(None) };
        if let Some(x) = self.func_cache.get(&key) {
            return Ok(x.filter(|&i| self.funcs_out[i as usize].1.lit_bad & typed == 0));
        }
        if !self.big(f) {
            self.func_cache.insert(key, None);
            return Ok(None);
        }
        let at = (self.marks_spans.len(), self.cond_marks.len(), self.event.sites.len(), self.funcs_out.len());
        let depth = (self.frames.len(), self.rets.len(), self.call_stack.len());
        match speculative(self.lower_function(f, vals, span))? {
            Some(x) if x.is_none_or(|i| self.funcs_out[i as usize].1.lit_bad & typed == 0) => {
                self.func_cache.insert(key, x);
                Ok(x)
            }
            // A literal it may not take, or an error the inlined literal might not reach (a
            // branch it folds away): undo it all, the copy decides.
            _ => {
                self.marks_spans.truncate(at.0);
                self.cond_marks.truncate(at.1);
                self.event.sites.truncate(at.2);
                self.funcs_out.truncate(at.3);
                self.func_cache.retain(|_, x| x.is_none_or(|i| (i as usize) < at.3));
                self.frames.truncate(depth.0);
                self.rets.truncate(depth.1);
                self.call_stack.truncate(depth.2);
                Ok(None)
            }
        }
    }

    /// Emits a call of function `idx` (`vals`: the call's arguments, literals bound in it).
    fn outlined_call(&mut self, idx: u16, vals: &[V]) -> V {
        let meta = self.funcs_out[idx as usize].1.clone();
        // The arguments (constants as f32 values), the emit context.
        let mut argv = Vec::new();
        for v in vals {
            match v {
                V::Vec(n, l) => argv.extend_from_slice(&l[..*n as usize]),
                V::B(x) | V::I(x) | V::F(x) => argv.push(*x),
                // Bound in the function itself.
                _ => {}
            }
        }
        let counters: Vec<Var> = self.kernel.counters.iter().map(|(_, c)| *c).collect();
        if meta.emits {
            argv.push(self.kernel.element.unwrap());
            for c in &counters {
                let x = self.b.get(*c);
                argv.push(x);
            }
        }
        let state: Vec<(Vec<Var>, bool)> = meta.state.iter().map(|(name, set)| match self.globals.get(name) {
            Some(Bind::Local(_, vars)) => (vars.clone(), *set),
            _ => unreachable!("a function's state names are state scalars"),
        }).collect();
        for (vars, _) in &state {
            for v in vars {
                let x = self.b.get(*v);
                argv.push(x);
            }
        }
        let tys: Vec<Ty> = {
            let g = &self.funcs_out[idx as usize].0;
            g.results.iter().map(|r| g.vals[r.0 as usize]).collect()
        };
        let rets: Vec<Val> = tys
            .iter()
            .map(|t| {
                self.b.prog.vals.push(*t);
                self.b.consts.push(None);
                Val(self.b.prog.vals.len() as u32 - 1)
            })
            .collect();
        self.b.push(IS::Call { f: idx, args: argv, rets: rets.clone() });
        let words = match &meta.ret {
            Some(T::Vec(n)) => *n as usize,
            Some(_) => 1,
            None => 0,
        };
        if meta.emits {
            for (c, r) in counters.iter().zip(&rets[words..]) {
                self.b.set(*c, *r);
            }
        }
        let mut at = words + if meta.emits { counters.len() } else { 0 };
        for (vars, set) in &state {
            if *set {
                for v in vars {
                    self.b.set(*v, rets[at]);
                    at += 1;
                }
            }
        }
        match meta.ret {
            Some(T::F) => V::F(rets[0]),
            Some(T::I) => V::I(rets[0]),
            Some(T::B) => V::B(rets[0]),
            Some(T::Vec(n)) => {
                let mut l = [rets[0]; 16];
                l[..n as usize].copy_from_slice(&rets[..n as usize]);
                V::Vec(n, l)
            }
            _ => V::Unit,
        }
    }

    /// Lowers helper `f` with arguments of `kinds` as a function of the
    /// program; None when it cannot be one (it returns an aggregate, or its
    /// writes would no longer be provably the element's own): then it is
    /// inlined.
    fn lower_function(&mut self, f: &FnDecl, args: &[V], span: Span) -> LResult<Option<u16>> {
        let emits = self.emits_in(f);
        let event = self.domain == Domain::Event;
        // Event programs: the state scalars it touches, with their types and whether it assigns
        // them; a port inside names its own site (no caller's call span); its locals take frame
        // words of their own (moved above the entries' when every entry is lowered).
        let state = if event { self.state_in(f) } else { Vec::new() };
        let saved_globals: Vec<(String, Option<Bind>)> = state.iter().map(|(n, ..)| (n.clone(), self.globals.get(n).cloned())).collect();
        let saved_spans = if event { std::mem::take(&mut self.call_spans) } else { Vec::new() };
        let saved_frame = if event { std::mem::replace(&mut self.frame_words, 0) } else { 0 };
        let first_mark = self.marks_spans.len() as u32;
        let saved_b = std::mem::replace(&mut self.b, Builder::new());
        let saved_elem = self.kernel.element;
        let saved_counters = self.kernel.counters.clone();
        let saved_offsets = std::mem::take(&mut self.kernel.local_offsets);
        let saved_nonlocal = self.kernel.nonlocal;
        let saved_loops = std::mem::take(&mut self.loops);
        // Its i32 parameters that a literal may fill (their values' taint is this builder's).
        let saved_taint = std::mem::replace(&mut self.taint, event.then(Taint::default));
        let r = self.function_body(f, args, emits, &state, span);
        let taint = std::mem::replace(&mut self.taint, saved_taint);
        let fb = std::mem::replace(&mut self.b, saved_b);
        for (name, bind) in saved_globals {
            match bind {
                Some(b) => self.globals.insert(name, b),
                None => self.globals.remove(&name),
            };
        }
        let frame = if event {
            self.call_spans = saved_spans;
            std::mem::replace(&mut self.frame_words, saved_frame)
        } else {
            0
        };
        let nonlocal = self.kernel.nonlocal && !saved_nonlocal;
        self.kernel.element = saved_elem;
        self.kernel.counters = saved_counters;
        self.kernel.local_offsets = saved_offsets;
        self.kernel.nonlocal = saved_nonlocal;
        self.loops = saved_loops;
        let Some((params, results, ret)) = r? else { return Ok(None) };
        if nonlocal {
            return Ok(None);
        }
        let mut g = fb.prog;
        g.body = fb.blocks.into_iter().next().unwrap();
        g.params = params;
        g.results = results;
        let state = state.into_iter().map(|(name, _, _, set)| (name, set)).collect();
        let marks = first_mark..self.marks_spans.len() as u32;
        let lit_bad = match taint {
            Some(t) => {
                let safe = self.lit_safe(f);
                (0..f.params.len().min(64)).filter(|&k| !safe[k]).fold(t.bad, |m, k| m | 1 << k)
            }
            None => 0,
        };
        self.funcs_out.push((g, FnMeta { emits, ret, state, frame, name: f.name.clone(), span: f.span, marks, lit_bad }));
        Ok(Some(self.funcs_out.len() as u16 - 1))
    }

    /// The body of function `f` in the (fresh) builder: (params, results,
    /// returned type), or None when it returns what a function cannot.
    #[allow(clippy::type_complexity)]
    fn function_body(&mut self, f: &FnDecl, args: &[V], emits: bool, state: &[StateArg], span: Span) -> LResult<Option<(Vec<Val>, Vec<Val>, Option<T>)>> {
        let mut params = Vec::new();
        let mut param = |l: &mut Lowerer, t: Ty| {
            l.b.prog.vals.push(t);
            l.b.consts.push(None);
            let v = Val(l.b.prog.vals.len() as u32 - 1);
            params.push(v);
            v
        };
        let mut scope = HashMap::new();
        for (k, ((name, _), a)) in f.params.iter().zip(args).enumerate() {
            let v = match a {
                V::Lit(x) => {
                    // As when inlined: a constant.
                    scope.insert(name.clone(), Bind::Const(V::Lit(*x)));
                    continue;
                }
                V::F(_) => V::F(param(self, Ty::F32)),
                V::I(_) => {
                    let x = param(self, Ty::I32);
                    if let Some(t) = &mut self.taint {
                        t.vals.insert(x, k as u32);
                    }
                    V::I(x)
                }
                V::B(_) => V::B(param(self, Ty::Bool)),
                V::Vec(n, _) => {
                    let mut l = [Val(0); 16];
                    for x in l.iter_mut().take(*n as usize) {
                        *x = param(self, Ty::F32);
                    }
                    V::Vec(*n, l)
                }
                _ => unreachable!("kinds checked"),
            };
            let bind = if assigns(&f.body, name) { self.new_local(&v, span)? } else { Bind::Value(v) };
            scope.insert(name.clone(), bind);
        }
        // The emit context: the element index, the counters (as the
        // function's own variables, their values in and out).
        let mut counters = Vec::new();
        if emits {
            let e = param(self, Ty::I32);
            self.kernel.element = Some(e);
            let kds: Vec<u8> = self.kernel.counters.iter().map(|(k, _)| *k).collect();
            self.kernel.counters.clear();
            for kd in kds {
                let c = self.b.var(Ty::I32);
                let x = param(self, Ty::I32);
                self.b.set(c, x);
                self.kernel.counters.push((kd, c));
                counters.push(c);
            }
        }
        // State scalars: the function's own variables, their values in and (assigned) out.
        let mut state_out = Vec::new();
        for (name, t, tys, set) in state {
            let mut vars = Vec::new();
            for ty in tys {
                let c = self.b.var(*ty);
                let x = param(self, *ty);
                self.b.set(c, x);
                vars.push(c);
            }
            if *set {
                state_out.extend(vars.iter().copied());
            }
            self.globals.insert(name.clone(), Bind::Local(t.clone(), vars));
        }
        self.call_stack.push((f.name.clone(), f.span));
        self.frames.push(vec![scope]);
        self.rets.push(Ret { vars: None, used: false });
        self.loops.push(LoopKind::Wrapper);
        self.b.open();
        let r = self.stmts(&f.body);
        let body = self.b.close();
        self.loops.pop();
        let ret = self.rets.pop().unwrap();
        self.frames.pop();
        self.call_stack.pop();
        let v = match r {
            Ok(v) => v,
            Err(e) if !e.is_cancelled() && e.start >= self.prelude_base && span.start < self.prelude_base => {
                return err(span, format!("in `{}`: {}", f.name, e.message));
            }
            Err(e) => return Err(e),
        };
        // The returned value, as when inlined.
        let v = if !ret.used {
            self.b.splice(body);
            v
        } else {
            let (t, vars) = ret.vars.clone().unwrap_or((T::F, vec![]));
            let mut body = body;
            if !matches!(ir::unmarked(&body).next_back(), Some(IS::Break(_))) {
                if vars.is_empty() {
                    if !matches!(v, V::Unit) {
                        return err(span, format!("`{}` returns a value in some paths only", f.name));
                    }
                } else {
                    if matches!(v, V::Unit) {
                        return err(f.span, format!("`{}` must end with a value (its `return`s give one)", f.name));
                    }
                    self.b.open();
                    self.assign_vars(&t, &vars, &v, f.span)?;
                    let tail = self.b.close();
                    body.extend(tail);
                }
            }
            self.zero_vars(&vars);
            self.b.push(IS::Loop { cap: 1, body });
            if vars.is_empty() {
                V::Unit
            } else {
                self.read_vars(&t, &vars)
            }
        };
        let (t, words): (Option<T>, Vec<Val>) = match v {
            V::Unit => (None, vec![]),
            V::Lit(_) if self.event.integer_literals => (Some(T::I), vec![self.to_i(&v, span)?]),
            V::Lit(_) | V::F(_) => (Some(T::F), vec![self.to_f(&v, span)?]),
            V::I(x) => (Some(T::I), vec![x]),
            V::B(x) => (Some(T::B), vec![x]),
            V::Vec(n, l) => (Some(T::Vec(n)), l[..n as usize].to_vec()),
            V::D(_) | V::Place(_) => return Ok(None),
        };
        // Results are read at the end of the top level (through variables:
        // a value of a nested block is not visible there).
        let mut results = Vec::new();
        for w in words {
            let ty = self.b.prog.vals[w.0 as usize];
            let c = self.b.var(ty);
            self.b.set(c, w);
            results.push(c);
        }
        let mut results: Vec<Val> = results.into_iter().map(|c| self.b.get(c)).collect();
        for c in counters {
            results.push(self.b.get(c));
        }
        for c in state_out {
            results.push(self.b.get(c));
        }
        Ok(Some((params, results, t)))
    }

    fn zero_vars(&mut self, vars: &[Var]) {
        for v in vars {
            let ty = self.b.prog.vars[v.0 as usize];
            let z = self.b.konst(ty, 0);
            self.b.set(*v, z);
        }
    }

    fn new_local(&mut self, v: &V, span: Span) -> LResult<Bind> {
        let (t, vars) = match v {
            V::Lit(_) if self.event.integer_literals => (T::I, vec![self.b.var(Ty::I32)]),
            V::Lit(_) | V::F(_) => (T::F, vec![self.b.var(Ty::F32)]),
            V::I(_) => (T::I, vec![self.b.var(Ty::I32)]),
            V::B(_) => (T::B, vec![self.b.var(Ty::Bool)]),
            V::D(_) => (T::D, vec![self.b.var(Ty::F64)]),
            V::Vec(n, _) => (T::Vec(*n), (0..*n).map(|_| self.b.var(Ty::F32)).collect()),
            V::Place(p) => return Ok(Bind::Place(p.clone())),
            V::Unit => return err(span, "this has no value"),
        };
        self.assign_vars(&t, &vars, v, span)?;
        Ok(Bind::Local(t, vars))
    }

    fn assign_vars(&mut self, t: &T, vars: &[Var], v: &V, span: Span) -> LResult<()> {
        match self.held(t, v, span)? {
            V::F(x) | V::I(x) | V::B(x) | V::D(x) => self.b.set(vars[0], x),
            V::Vec(n, l) => {
                for k in 0..n as usize {
                    self.b.set(vars[k], l[k]);
                }
            }
            _ => unreachable!(),
        }
        Ok(())
    }

    /// `v` as a variable of type `t` holds it.
    fn held(&mut self, t: &T, v: &V, span: Span) -> LResult<V> {
        Ok(match t {
            T::F => V::F(self.to_f(v, span)?),
            T::I => match v {
                V::I(_) | V::Lit(_) => V::I(self.to_i(v, span)?),
                _ => return err(span, format!("expected i32, found {} (use int(x))", self.vname(v))),
            },
            T::B => {
                let V::B(x) = v else { return err(span, format!("expected bool, found {}", self.vname(v))) };
                V::B(*x)
            }
            T::D => V::D(self.to_d(v, span)?),
            T::Vec(n) => V::Vec(*n, self.to_vec(v, *n, span)?),
            _ => unreachable!(),
        })
    }

    fn read_vars(&mut self, t: &T, vars: &[Var]) -> V {
        match t {
            T::F => V::F(self.b.get(vars[0])),
            T::I => V::I(self.b.get(vars[0])),
            T::B => V::B(self.b.get(vars[0])),
            T::D => V::D(self.b.get(vars[0])),
            T::Vec(n) => {
                let mut l = [Val(0); 16];
                for k in 0..*n as usize {
                    l[k] = self.b.get(vars[k]);
                }
                V::Vec(*n, l)
            }
            _ => unreachable!(),
        }
    }

    /// The type a value would take in a fresh variable.
    fn value_t(&self, v: &V) -> Option<T> {
        Some(match v {
            V::Lit(_) if self.event.integer_literals => T::I,
            V::Lit(_) | V::F(_) => T::F,
            V::I(_) => T::I,
            V::B(_) => T::B,
            V::D(_) => T::D,
            V::Vec(n, _) => T::Vec(*n),
            _ => return None,
        })
    }

    // -- statements ------------------------------------------------------------

    /// Lowers statements; the value is the last expression statement's.
    fn stmts(&mut self, stmts: &[Stmt]) -> LResult<V> {
        let mut last = V::Unit;
        // the names whose assignments here each name a new value (`assigned_in_line`)
        let mut in_line: Vec<&str> = Vec::new();
        for (k, s) in stmts.iter().enumerate() {
            last = V::Unit;
            if self.marks && !self.b.scratch {
                let span = match s {
                    Stmt::Let { value: Expr { kind: ExprKind::Lambda(_), .. }, .. } => None,
                    Stmt::Let { span, .. } | Stmt::Assign { span, .. } | Stmt::For { span, .. } | Stmt::While { span, .. } | Stmt::Loop { span, .. }
                    | Stmt::Break(span) | Stmt::Continue(span) | Stmt::Return(_, span) => Some(*span),
                    Stmt::Expr(e) => Some(e.span),
                };
                if let Some(span) = span.filter(|span| span.start < self.prelude_base) {
                    self.b.push(IS::Mark(self.marks_spans.len() as u32));
                    self.marks_spans.push(span);
                }
            }
            match s {
                Stmt::Let { name, value: Expr { kind: ExprKind::Lambda(f), .. }, .. } => {
                    let scopes = self.frames.last().cloned().unwrap_or_default();
                    let f = FnDecl { name: name.clone(), ..(**f).clone() };
                    self.bind(name, Bind::Closure(Box::new(f), scopes));
                }
                Stmt::Let { name, ann, value, span } => {
                    let v = self.expr(value)?;
                    let v = match ann {
                        Some(TypeAnn::I32) => V::I(self.to_i(&v, *span)?),
                        Some(TypeAnn::F32) => V::F(self.to_f(&v, *span)?),
                        Some(TypeAnn::F64) => V::D(self.to_d(&v, *span)?),
                        Some(ann @ (TypeAnn::Vec2 | TypeAnn::Vec3 | TypeAnn::Vec4 | TypeAnn::Mat4)) => {
                            let n = ann_width(ann);
                            V::Vec(n, self.to_vec(&v, n, *span)?)
                        }
                        _ => v,
                    };
                    // A traced lowering's values lead to their defs (`trace.rs`): a let never
                    // assigned again names its value, as a parameter does; one assigned again
                    // only in line names each value in turn, as a chain of lets would.
                    in_line.retain(|n| *n != name.as_str());
                    let rest = &stmts[k + 1..];
                    let bind = match self.value_t(&v) {
                        Some(_) if self.trace.is_some() && !assigns(rest, name) => Bind::Value(v),
                        Some(t) if self.trace.is_some() && assigned_in_line(rest, name) => {
                            in_line.push(name.as_str());
                            Bind::Value(self.held(&t, &v, *span)?)
                        }
                        _ => self.new_local(&v, *span)?,
                    };
                    self.bind(name, bind);
                }
                Stmt::Assign { target: Expr { kind: ExprKind::Ident(name), .. }, op, value, span } if in_line.contains(&name.as_str()) => {
                    let rhs = self.expr(value)?;
                    let Some(Bind::Value(old)) = self.lookup(name) else { unreachable!("a name assigned in line is bound to its value") };
                    let t = self.value_t(&old).expect("a value a variable could hold");
                    let v = self.combined(*op, old, rhs, *span)?;
                    let v = self.held(&t, &v, *span)?;
                    self.bind(name, Bind::Value(v));
                }
                Stmt::Assign { target, op, value, span } => self.assign(target, *op, value, *span)?,
                Stmt::Expr(e) => {
                    let v = self.expr(e)?;
                    if k + 1 == stmts.len() {
                        last = v;
                    }
                }
                Stmt::For { var, from, to, body, span } => self.for_loop(var, from, to, body, *span)?,
                Stmt::While { cond, body, span } => {
                    self.b.open_loop();
                    self.loops.push(LoopKind::User);
                    let r = (|| {
                        let c = self.expr(cond)?;
                        let c = self.truth(&c, cond.span)?;
                        let nc = self.b.un(Ty::Bool, Un::NotB, c);
                        // The exit tests the condition's negation.
                        if let Some(id) = self.cond_site(cond.span, false) {
                            self.b.push(IS::Mark(id));
                        }
                        self.b.push(IS::If(nc, vec![IS::Break(0)], vec![]));
                        self.scoped(|l| l.stmts(body))
                    })();
                    self.loops.pop();
                    let b = self.b.close();
                    r?;
                    self.capped_loop(b, true, *span);
                }
                Stmt::Loop { body, span } => {
                    self.b.open_loop();
                    self.loops.push(LoopKind::User);
                    let r = self.scoped(|l| l.stmts(body));
                    self.loops.pop();
                    let b = self.b.close();
                    r?;
                    self.capped_loop(b, false, *span);
                }
                Stmt::Break(span) | Stmt::Continue(span) => {
                    let mut depth = 0;
                    let mut found = false;
                    for kind in self.loops.iter().rev() {
                        match kind {
                            LoopKind::User => {
                                found = true;
                                break;
                            }
                            LoopKind::Wrapper => depth += 1,
                            LoopKind::Frame => break,
                        }
                    }
                    // A wrapper between us and the loop means the loop is
                    // in a caller: not reachable.
                    let wrapper_between = self
                        .loops
                        .iter()
                        .rev()
                        .take_while(|k| **k != LoopKind::User)
                        .any(|k| *k == LoopKind::Wrapper);
                    if !found || wrapper_between {
                        return err(*span, "`break`/`continue` outside a loop");
                    }
                    self.b.push(if matches!(s, Stmt::Break(_)) { IS::Break(depth) } else { IS::Continue(depth) });
                }
                Stmt::Return(value, span) => {
                    let v = match value {
                        Some(e) => self.expr(e)?,
                        None => V::Unit,
                    };
                    let depth = self.loops.iter().rev().position(|k| *k == LoopKind::Wrapper).unwrap() as u32;
                    let ret_i = self.rets.len() - 1;
                    if !matches!(v, V::Unit) {
                        if self.rets[ret_i].vars.is_none() {
                            let Some(t) = self.value_t(&v) else {
                                return err(*span, "functions return f32, i32, bool, vec2/3/4 or mat4 (structs and arrays are passed by reference instead)");
                            };
                            let vars = match &t {
                                T::Vec(n) => (0..*n).map(|_| self.b.var(Ty::F32)).collect(),
                                t => vec![self.b.var(ty_of(t))],
                            };
                            self.rets[ret_i].vars = Some((t, vars));
                        }
                        let (t, vars) = self.rets[ret_i].vars.clone().unwrap();
                        self.assign_vars(&t, &vars, &v, *span)?;
                    } else if self.rets[ret_i].vars.is_some() {
                        return err(*span, "this `return` needs a value");
                    }
                    self.rets[ret_i].used = true;
                    self.b.push(IS::Break(depth));
                    // Anything after a return in this block is dead.
                    return Ok(V::Unit);
                }
            }
        }
        Ok(last)
    }

    /// What `op` makes of a target holding `old` and the value `rhs`.
    fn combined(&mut self, op: AssignOp, old: V, rhs: V, span: Span) -> LResult<V> {
        let bop = match op {
            AssignOp::Set => return Ok(rhs),
            AssignOp::Add => BinOp::Add,
            AssignOp::Sub => BinOp::Sub,
            AssignOp::Mul => BinOp::Mul,
            AssignOp::Div => BinOp::Div,
            AssignOp::Rem => BinOp::Rem,
        };
        self.binary(bop, old, rhs, span)
    }

    fn assignment_binding(&mut self, name: &str, span: Span) -> LResult<Option<Bind>> {
        let bind = self.lookup(name);
        let Some(Bind::AudioPort(k)) = bind else { return Ok(bind) };
        let port = &mut self.ports[k];
        if !port.info.output {
            return err(span, format!("input port `{name}` is read-only"));
        }
        if self.entry.is_none() {
            return err(span, format!("output port `{name}` cannot be assigned outside block(), voice() or effect()"));
        }
        port.frame_write |= self.port_frame.is_some();
        port.info.control = !port.frame_write;
        let ty = if port.info.channels == 1 { T::F } else { T::Vec(2) };
        Ok(Some(Bind::Local(ty, port.held.clone())))
    }

    fn assign(&mut self, target: &Expr, op: AssignOp, value: &Expr, span: Span) -> LResult<()> {
        let rhs = self.expr(value)?;
        let combine = |l: &mut Self, old: V| -> LResult<V> { l.combined(op, old, rhs.clone(), span) };
        match &target.kind {
            ExprKind::Ident(name) => match self.assignment_binding(name, target.span)? {
                Some(Bind::Local(t, vars)) => {
                    let old = self.read_vars(&t, &vars);
                    let v = combine(self, old)?;
                    self.assign_vars(&t, &vars, &v, span)
                }
                Some(Bind::Place(p)) => {
                    let old = self.read(&p);
                    let v = combine(self, old)?;
                    self.write(&p, &v, span)
                }
                Some(Bind::Param(_)) => err(target.span, format!("`{}` is a param: set it from the Score", name)),
                Some(_) => err(target.span, format!("`{}` is a constant", name)),
                None => err(target.span, format!("unknown name `{}`", name)),
            },
            ExprKind::Field(base, field) => {
                // A lane of a vector local.
                if let ExprKind::Ident(name) = &base.kind {
                    if let Some(Bind::Local(T::Vec(n), vars)) = self.assignment_binding(name, target.span)? {
                        let lane = single_lane(n, field).ok_or_else(|| {
                            ShaderError::new(target.span.start, target.span.end, format!("{} has no single component `{}`", vec_name(n), field))
                        })?;
                        let old = V::F(self.b.get(vars[lane]));
                        let v = combine(self, old)?;
                        let x = self.to_f(&v, span)?;
                        self.b.set(vars[lane], x);
                        return Ok(());
                    }
                }
                let p = self.place(target)?;
                let old = self.read(&p);
                let v = combine(self, old)?;
                self.write(&p, &v, span)
            }
            ExprKind::Index(..) => {
                let p = self.place(target)?;
                let old = self.read(&p);
                let v = combine(self, old)?;
                self.write(&p, &v, span)
            }
            _ => err(target.span, "cannot assign to this"),
        }
    }

    /// Resolves an lvalue/aggregate expression to a memory place.
    fn place(&mut self, e: &Expr) -> LResult<Place> {
        match &e.kind {
            ExprKind::Ident(_) | ExprKind::StructLit(..) | ExprKind::ArrayRepeat(..) | ExprKind::ArrayList(..) | ExprKind::Call(..) => {
                match self.expr(e)? {
                    V::Place(p) => Ok(p),
                    v => err(e.span, format!("expected a struct or array, found {}", self.vname(&v))),
                }
            }
            ExprKind::Field(base, field) => {
                let p = self.place(base)?;
                self.field_place(&p, field, e.span)
            }
            ExprKind::Index(base, idx) => {
                let p = self.place(base)?;
                let iv = self.expr(idx)?;
                self.index_place(&p, &iv, idx.span)
            }
            _ => err(e.span, "expected a struct or array"),
        }
    }

    fn field_place(&mut self, p: &Place, field: &str, span: Span) -> LResult<Place> {
        match &p.ty {
            T::Struct(s) => {
                let d = &self.structs[*s];
                let Some((_, ft, off, _)) = d.fields.iter().find(|f| f.0 == field) else {
                    if let Some((_, why)) = d.opaque.iter().find(|f| f.0 == field) {
                        return err(span, format!("field `{}` is {}", field, why));
                    }
                    let fields: Vec<&str> = d.fields.iter().map(|f| f.0.as_str()).collect();
                    return err(span, format!("`{}` has no field `{}` (it has {})", d.name, field, fields.join(", ")));
                };
                Ok(Place { ty: ft.clone(), stat: p.stat + off, ..p.clone() })
            }
            T::Vec(n) => {
                let n = *n;
                if n == 16 {
                    if let Some(c) = field.strip_prefix('c').and_then(|c| c.parse::<u32>().ok()).filter(|c| *c < 4) {
                        return Ok(Place { ty: T::Vec(4), stat: p.stat + c * 4, ..p.clone() });
                    }
                }
                let lane = single_lane(n, field).ok_or_else(|| ShaderError::new(span.start, span.end, format!("{} has no single component `{}`", vec_name(n), field)))?;
                Ok(Place { ty: T::F, stat: p.stat + lane as u32, ..p.clone() })
            }
            t => err(span, format!("{} has no fields", self.tname(t))),
        }
    }

    fn index_place(&mut self, p: &Place, idx: &V, span: Span) -> LResult<Place> {
        if let T::Buf(el, stride, offset, _) = &p.ty {
            // Host buffers clamp (no wrap): element idx at idx * stride.
            let (el, stride, offset) = ((**el).clone(), *stride, *offset);
            let i = self.to_i(idx, span)?;
            let off = if stride == 1 {
                i
            } else {
                let c = self.b.ci(stride as i32);
                self.b.ib(Bin::MulI, i, c)
            };
            if self.kernel.element == Some(i) {
                self.kernel.local_offsets.insert(off);
            }
            return Ok(Place { ty: el, stat: p.stat + offset, off: Some(off), ..p.clone() });
        }
        let T::Array(el, n) = &p.ty else {
            return err(span, format!("{} cannot be indexed", self.tname(&p.ty)));
        };
        let (el, n) = ((**el).clone(), *n);
        let stride = self.words(&el);
        if let V::Lit(x) = idx {
            let k = (x.floor() as i64).rem_euclid(n as i64) as u32;
            return Ok(Place { ty: el, stat: p.stat + k * stride, ..p.clone() });
        }
        let i = self.to_i(idx, span)?;
        let w = if n > i32::MAX as u32 && !n.is_power_of_two() {
            // A signed index is less than this length in magnitude.
            let zero = self.b.ci(0);
            let negative = self.b.cmpi(Cmp::Lt, i, zero);
            let len = self.b.ci(n as i32);
            let from_end = self.b.ib(Bin::AddI, i, len);
            self.b.sel(negative, from_end, i)
        } else {
            self.b.def(Ty::I32, Op::Wrap(i, n))
        };
        let w = if stride == 1 {
            w
        } else {
            let s = self.b.ci(stride as i32);
            self.b.ib(Bin::MulI, w, s)
        };
        // The static part (field offsets) stays static; it folds into the
        // access's base.
        let off = match p.off {
            None => w,
            Some(o) => self.b.ib(Bin::AddI, o, w),
        };
        Ok(Place { ty: el, off: Some(off), ..p.clone() })
    }

    /// Whether loops that reach their run-time cap report it: a kernel's
    /// element code (the call's overflow word) and an event program with a
    /// cap port; audio and init() keep plain caps.
    fn reports_caps(&self) -> bool {
        (self.domain == Domain::Kernel && self.kernel.element.is_some()) || (self.domain == Domain::Event && self.event.cap_port.is_some())
    }

    /// The report of a loop at `span` that reached its cap (when
    /// [`Lowerer::reports_caps`]): the overflow word, or the cap port called
    /// with the loop's site.
    fn cap_report(&mut self, span: Span) -> Vec<IS> {
        match self.event.cap_port.filter(|_| self.domain == Domain::Event) {
            Some(port) => {
                let site = self.event_site(span);
                vec![IS::Port { port, args: vec![], rets: vec![], site }]
            }
            None => {
                let one = self.b.ci(1);
                vec![IS::Store { region: Region::Ctx, base: crate::lower::kernel::K_OVERFLOW, extent: 1, off: None, val: one }]
            }
        }
    }

    /// A `while` or `loop` body under the run-time cap. Where caps are
    /// reported ([`Lowerer::cap_report`]), a loop that starts a 1025th
    /// iteration reports and stops (never silently cut). A `while` is
    /// checked after its condition (`cond_first`: the body's first statement
    /// is the condition's break), so one that ends after exactly 1024 is not.
    fn capped_loop(&mut self, mut body: Block, cond_first: bool, span: Span) {
        if !self.reports_caps() {
            self.b.push(IS::Loop { cap: LOOP_CAP, body });
            return;
        }
        let n = self.b.var(Ty::I32);
        let zero = self.b.ci(0);
        self.b.set(n, zero);
        self.b.open();
        let k = self.b.get(n);
        let most = self.b.ci(LOOP_CAP as i32);
        let hit = self.b.cmpi(Cmp::Ge, k, most);
        let mut report = self.cap_report(span);
        report.push(IS::Break(0));
        self.b.push(IS::If(hit, report, vec![]));
        let one = self.b.ci(1);
        let k1 = self.b.ib(Bin::AddI, k, one);
        self.b.set(n, k1);
        let check = self.b.close();
        let mut outer = Vec::with_capacity(body.len() + check.len());
        if cond_first && !body.is_empty() {
            // The condition's value definitions and its break come first.
            let at = body.iter().position(|s| matches!(s, IS::If(..))).map_or(0, |k| k + 1);
            outer.extend(body.drain(..at));
        }
        outer.extend(check);
        outer.extend(body);
        self.b.push(IS::Loop { cap: LOOP_CAP + 1, body: outer });
    }

    /// A rough count of the AIR operations `body` lowers to (helpers
    /// inlined, constant loops at their trip count, a builtin call a few
    /// ops): what unrolling it costs.
    fn ast_cost(&self, body: &[Stmt]) -> u64 {
        fn trip(l: &Lowerer, e: &Expr) -> Option<f64> {
            match &e.kind {
                ExprKind::Num(x, _) => Some(*x),
                ExprKind::Ident(n) => match l.globals.get(n) {
                    Some(Bind::Const(V::Lit(x))) => Some(*x),
                    Some(Bind::Scalar(T::I, w)) => Some(*w as i32 as f64),
                    _ => None,
                },
                _ => None,
            }
        }
        fn expr(l: &Lowerer, e: &Expr, memo: &mut HashMap<String, u64>, stack: &mut Vec<String>) -> u64 {
            1 + match &e.kind {
                ExprKind::Num(..) | ExprKind::Bool(_) | ExprKind::Ident(_) => 0,
                ExprKind::Field(a, _) | ExprKind::Neg(a) | ExprKind::Not(a) => expr(l, a, memo, stack),
                ExprKind::Index(a, b) | ExprKind::Bin(_, a, b) | ExprKind::ArrayRepeat(a, b) => expr(l, a, memo, stack) + expr(l, b, memo, stack),
                ExprKind::ArrayList(xs) => xs.iter().map(|x| expr(l, x, memo, stack)).sum(),
                ExprKind::StructLit(_, fs) => fs.iter().map(|(_, x)| expr(l, x, memo, stack)).sum(),
                ExprKind::Str(_) | ExprKind::Unreadable(_) => 0,
                // A local function: about one call's worth.
                ExprKind::Lambda(f) => stmts(l, &f.body, memo, stack),
                ExprKind::Block(b) => stmts(l, b, memo, stack),
                ExprKind::If(arms, else_) => arms.iter().map(|(c, b)| expr(l, c, memo, stack) + stmts(l, b, memo, stack)).sum::<u64>() + else_.as_ref().map_or(0, |b| stmts(l, b, memo, stack)),
                ExprKind::Match(x, arms) => expr(l, x, memo, stack) + arms.iter().map(|(_, b)| 2 + stmts(l, b, memo, stack)).sum::<u64>(),
                ExprKind::Call(name, args) => {
                    let a: u64 = args.iter().map(|x| expr(l, x, memo, stack)).sum();
                    a + match l.fns.get(name) {
                        Some(f) if !stack.contains(name) => match memo.get(name) {
                            Some(c) => *c,
                            None => {
                                stack.push(name.clone());
                                let c = stmts(l, &f.body, memo, stack);
                                stack.pop();
                                memo.insert(name.clone(), c);
                                c
                            }
                        },
                        _ if name == "emit" => 3 * args.len() as u64 + 8,
                        _ => 6,
                    }
                }
            }
        }
        fn stmts(l: &Lowerer, b: &[Stmt], memo: &mut HashMap<String, u64>, stack: &mut Vec<String>) -> u64 {
            b.iter()
                .map(|s| match s {
                    Stmt::Let { value, .. } | Stmt::Expr(value) => expr(l, value, memo, stack),
                    Stmt::Assign { target, value, .. } => expr(l, target, memo, stack) + expr(l, value, memo, stack),
                    Stmt::Return(e, _) => e.as_ref().map_or(1, |e| expr(l, e, memo, stack)),
                    Stmt::For { from, to, body, .. } => {
                        let n = match (trip(l, from), trip(l, to)) {
                            (Some(a), Some(z)) => (z.floor() - a.floor()).clamp(1.0, 1e6) as u64,
                            _ => 1,
                        };
                        let inner = stmts(l, body, memo, stack);
                        // A loop it would not unroll costs its body once.
                        if n <= UNROLL_MAX as u64 && n.saturating_mul(inner).saturating_mul(BYTES_PER_OP) <= UNROLL_BYTES {
                            n * inner
                        } else {
                            inner + 6
                        }
                    }
                    Stmt::While { cond, body, .. } => expr(l, cond, memo, stack) + stmts(l, body, memo, stack) + 6,
                    Stmt::Loop { body, .. } => stmts(l, body, memo, stack) + 6,
                    Stmt::Break(_) | Stmt::Continue(_) => 1,
                })
                .sum()
        }
        stmts(self, body, &mut HashMap::new(), &mut Vec::new())
    }

    fn for_loop(&mut self, var: &str, from: &Expr, to: &Expr, body: &[Stmt], span: Span) -> LResult<()> {
        let a = self.expr(from)?;
        let mut min_bound = None;
        let z = match &to.kind {
            ExprKind::Call(name, args) if self.domain == Domain::Event && name == "min" => self.call_expr_bound(name, args, to.span, &mut min_bound)?,
            _ => self.expr(to)?,
        };
        let start = match &a {
            _ if float_literal(from) => None,
            V::Lit(x) if x.fract() == 0.0 && *x >= i32::MIN as f64 && *x <= i32::MAX as f64 => Some(*x as i32),
            V::I(v) => self.b.cst(*v).map(|x| x as i32),
            _ => None,
        };
        // Only integer min supplies this bound: a floating min can yield NaN,
        // whose conversion to an integer would invalidate the bound.
        let bounded_cap = match (start, min_bound) {
            (Some(a), Some(k)) => Some(u32::try_from((k as i64 - a as i64).max(0) + 1).map_err(|_| ShaderError::new(span.start, span.end, "loop too long".into()))?),
            _ => None,
        };
        if let (V::Lit(a), V::Lit(z)) = (&a, &z) {
            let (a, z) = (a.floor() as i64, z.floor() as i64);
            // Saturating: a hostile bound (`1..-1e38`) is far outside i64.
            let count = z.saturating_sub(a).max(0) as u64;
            // Small loops unroll while the unrolled code stays small. Audio
            // programs keep their own rule (up to 16 iterations, or 64 of a
            // one-line body): an unrolled loop's counter is a literal, and
            // expressions of literals fold at lowering (in f64, rounded to
            // f32 once) where the loop computes them in f32 step by step, so
            // the rule decides the rendered bits, which audio renders keep
            // (their golden hashes).
            let unroll = match self.domain {
                Domain::Audio => count <= AUDIO_UNROLL_MAX as u64 || (count <= 64 && body.len() == 1),
                Domain::Kernel | Domain::Event => count <= UNROLL_MAX as u64 && count.saturating_mul(self.ast_cost(body)).saturating_mul(BYTES_PER_OP) <= UNROLL_BYTES,
            };
            if unroll && !has_own_break(body) {
                for k in a..z {
                    self.scoped(|l| {
                        // Kernel counters stay i32 when unrolled, just as
                        // in the rolled loop below. An untyped literal
                        // would fold `k / 2` as f64 and change indexing.
                        let value = match l.domain {
                            Domain::Kernel | Domain::Event => V::I(l.b.ci(k as i32)),
                            Domain::Audio => V::Lit(k as f64),
                        };
                        l.bind(var, Bind::Const(value));
                        l.stmts(body)
                    })?;
                    if self.b.prog.vals.len() > MAX_IR_VALS {
                        return err(span, "the program is too large after unrolling");
                    }
                }
                return Ok(());
            }
            if count >= u32::MAX as u64 - 1 {
                return err(span, "loop too long");
            }
        }
        let cap = match (&a, &z, bounded_cap) {
            (V::Lit(a), V::Lit(z), _) => ((z.floor() - a.floor()).max(0.0) as u32).saturating_add(1),
            (_, _, Some(cap)) => cap,
            _ if self.whole_ranges => u32::MAX,
            _ => LOOP_CAP,
        };
        let av = self.to_i(&a, from.span)?;
        let zv = self.to_i(&z, to.span)?;
        if cap == LOOP_CAP && bounded_cap.is_none() && self.reports_caps() {
            // A run-time range longer than the cap is reported, not
            // silently cut.
            let len = self.b.ib(Bin::SubI, zv, av);
            let most = self.b.ci(LOOP_CAP as i32);
            let over = self.b.cmpi(Cmp::Gt, len, most);
            let report = self.cap_report(span);
            self.b.push(IS::If(over, report, vec![]));
        }
        let iv = self.b.var(Ty::I32);
        self.b.set(iv, av);
        self.b.open_loop();
        self.loops.push(LoopKind::User);
        let cur = self.b.get(iv);
        let done = self.b.cmpi(Cmp::Ge, cur, zv);
        self.b.push(IS::If(done, vec![IS::Break(0)], vec![]));
        let one = self.b.ci(1);
        let next = self.b.ib(Bin::AddI, cur, one);
        self.b.set(iv, next);
        let r = self.scoped(|l| {
            l.bind(var, Bind::Value(V::I(cur)));
            l.stmts(body)
        });
        self.loops.pop();
        let b = self.b.close();
        r?;
        self.b.push(IS::Loop { cap, body: b });
        Ok(())
    }

    // -- expressions -------------------------------------------------------------

    fn expr(&mut self, e: &Expr) -> LResult<V> {
        let span = e.span;
        match &e.kind {
            ExprKind::Num(x, int) => {
                if self.event.integer_literals && !*int {
                    return err(span, "this program has no float type: write an integer literal");
                }
                Ok(self.num(*x, *int, span))
            }
            ExprKind::Bool(x) => Ok(V::B(self.b.cb(*x))),
            ExprKind::Ident(name) => self.ident(name, span),
            ExprKind::Neg(a) => {
                let v = self.expr(a)?;
                match v {
                    V::Lit(x) => {
                        if self.event.integer_literals {
                            self.literal_i32(-x, span)?;
                        }
                        Ok(V::Lit(-x))
                    }
                    V::F(x) => Ok(V::F(self.b.fneg(x))),
                    V::D(x) => Ok(V::D(self.b.un(Ty::F64, Un::NegD, x))),
                    V::I(x) => Ok(V::I(self.b.un(Ty::I32, Un::NegI, x))),
                    V::Vec(n, l) => {
                        let mut o = l;
                        for k in 0..n as usize {
                            o[k] = self.b.fneg(l[k]);
                        }
                        Ok(V::Vec(n, o))
                    }
                    v => err(span, format!("cannot negate {}", self.vname(&v))),
                }
            }
            ExprKind::Not(a) => {
                let v = self.expr(a)?;
                let c = self.truth(&v, a.span)?;
                Ok(V::B(self.b.un(Ty::Bool, Un::NotB, c)))
            }
            ExprKind::Bin(BinOp::And, a, b) | ExprKind::Bin(BinOp::Or, a, b) => {
                let is_and = matches!(e.kind, ExprKind::Bin(BinOp::And, ..));
                let av = self.expr(a)?;
                let ac = self.truth(&av, a.span)?;
                // `a` decides whether `b` runs, when that takes a branch (below).
                let site = self.cond_site(a.span, true);
                self.b.open();
                let bv = self.expr(b);
                let bc = match bv {
                    Ok(bv) => self.truth(&bv, b.span),
                    Err(e) => Err(e),
                };
                let blk = self.b.close();
                let bc = bc?;
                if Builder::pure_block(&blk) {
                    self.b.splice(blk);
                    let op = if is_and { Bin::AndB } else { Bin::OrB };
                    return Ok(V::B(self.b.def(Ty::Bool, Op::Bin(op, ac, bc))));
                }
                let t = self.b.var(Ty::Bool);
                self.b.set(t, ac);
                let mut blk = blk;
                blk.push(IS::Set(t, bc));
                if let Some(id) = site {
                    self.b.push(IS::Mark(id));
                }
                if is_and {
                    self.b.push(IS::If(ac, blk, vec![]));
                } else {
                    self.b.push(IS::If(ac, vec![], blk));
                }
                Ok(V::B(self.b.get(t)))
            }
            ExprKind::Bin(op, a, b) => {
                let mut av = self.expr(a)?;
                let mut bv = self.expr(b)?;
                // In a kernel a number written with a fraction or exponent
                // (`4.0`) is a float: `i / 4.0` divides as floats, as in
                // the documents (an integer-valued literal `4` still takes
                // the other side's type).
                if self.domain != Domain::Audio {
                    if let (V::I(_), V::Lit(y)) = (&av, &bv) {
                        if float_literal(b) {
                            bv = V::F(self.b.cf(*y as f32));
                        }
                    }
                    if let (V::Lit(x), V::I(_)) = (&av, &bv) {
                        if float_literal(a) {
                            av = V::F(self.b.cf(*x as f32));
                        }
                    }
                }
                self.binary(*op, av, bv, span)
            }
            ExprKind::Field(base, field) => {
                if let Some(v) = self.object_read(e) {
                    return Ok(v);
                }
                let v = self.expr(base)?;
                match v {
                    V::Vec(n, l) => self.swizzle(n, &l, field, span),
                    V::Place(p) => {
                        let fp = self.field_place(&p, field, span)?;
                        Ok(self.read(&fp))
                    }
                    v => err(span, format!("{} has no field `{}`", self.vname(&v), field)),
                }
            }
            ExprKind::Index(base, idx) => {
                if let Some(v) = self.object_read(e) {
                    return Ok(v);
                }
                let p = self.place(base)?;
                let iv = self.expr(idx)?;
                let ep = self.index_place(&p, &iv, idx.span)?;
                Ok(self.read(&ep))
            }
            ExprKind::Str(_) | ExprKind::Unreadable(_) => err(span, format!("this is {}", e.kind.unreadable_reason().unwrap())),
            ExprKind::Lambda(_) => err(span, "a function value is bound with `let name = fn(..) {..}` and called by its name; kernels cannot store, pass or return functions"),
            ExprKind::Call(name, args) => self.call_expr(name, args, span),
            ExprKind::If(arms, else_) => self.if_expr(arms, else_.as_ref(), span),
            ExprKind::Match(subject, arms) => {
                let sv = self.expr(subject)?;
                // Lower to an if/elif chain.
                let mut chain: Vec<(Val, &Vec<crate::parse::Stmt>)> = Vec::new();
                let mut default = None;
                let mut conds = Vec::new();
                for (pats, body) in arms {
                    match pats {
                        None => {
                            default = Some(body);
                        }
                        Some(pats) => {
                            let mut c: Option<Val> = None;
                            for p in pats {
                                let pv = self.expr(p)?;
                                let eq = self.binary(BinOp::Eq, sv.clone(), pv, p.span)?;
                                let V::B(eq) = eq else { unreachable!() };
                                c = Some(match c {
                                    None => eq,
                                    Some(c) => self.b.def(Ty::Bool, Op::Bin(Bin::OrB, c, eq)),
                                });
                            }
                            conds.push((c.unwrap(), body));
                        }
                    }
                }
                chain.extend(conds);
                self.if_chain(&chain, default, span)
            }
            ExprKind::Block(stmts) => self.scoped(|l| l.stmts(stmts)),
            ExprKind::StructLit(..) | ExprKind::ArrayRepeat(..) | ExprKind::ArrayList(..) => self.aggregate_literal(e),
        }
    }

    fn aggregate_literal(&mut self, e: &Expr) -> LResult<V> {
        // Constant literals initialize from their image; otherwise field by
        // field.
        if let Some(c) = speculative(self.cinit_quiet(e))? {
            let t = self.cinit_ty(&c);
            let words = self.words(&t);
            let root = self.alloc_frame(words, e.span)?;
            let p = Place { region: Region::Frame, root, extent: words, off: None, stat: 0, ty: t };
            self.store_cinit(&p, &c, e.span)?;
            return Ok(V::Place(p));
        }
        match &e.kind {
            ExprKind::StructLit(name, fields) if name.is_empty() => {
                // A plain object of values: its shape from the values.
                let mut vals = Vec::new();
                let mut types = Vec::new();
                let mut opaque = Vec::new();
                for (fname, fe) in fields {
                    if let Some(why) = fe.kind.unreadable_reason() {
                        opaque.push((fname.clone(), why.to_string()));
                        continue;
                    }
                    if matches!(fe.kind, ExprKind::Lambda(_)) {
                        opaque.push((fname.clone(), "a function kept in an object: bind it with `let name = fn(..)` and call it by name".to_string()));
                        continue;
                    }
                    let v = self.expr(fe)?;
                    let t = match &v {
                        V::Place(p) => p.ty.clone(),
                        v => self.value_t(v).ok_or_else(|| ShaderError::new(fe.span.start, fe.span.end, format!("an object field holds a value, found {}", self.vname(v))))?,
                    };
                    if types.iter().any(|(n, _)| n == fname) {
                        return err(fe.span, format!("field `{}` is given twice", fname));
                    }
                    types.push((fname.clone(), t));
                    vals.push((fname, v, fe.span));
                }
                let s = self.object_struct(&types, opaque, e.span)?;
                let words = self.structs[s].words;
                let root = self.alloc_frame(words, e.span)?;
                let p = Place { region: Region::Frame, root, extent: words, off: None, stat: 0, ty: T::Struct(s) };
                for (fname, v, span) in vals {
                    let fp = self.field_place(&p, fname, span)?;
                    self.write(&fp, &v, span)?;
                }
                Ok(V::Place(p))
            }
            ExprKind::StructLit(name, fields) => {
                let Some(s) = self.structs.iter().position(|d| d.name == *name) else {
                    return err(e.span, format!("unknown struct `{}`", name));
                };
                let words = self.structs[s].words;
                let root = self.alloc_frame(words, e.span)?;
                let p = Place { region: Region::Frame, root, extent: words, off: None, stat: 0, ty: T::Struct(s) };
                let defaults = CInit::Struct(s, self.structs[s].fields.iter().map(|f| f.3.clone()).collect());
                self.store_cinit(&p, &defaults, e.span)?;
                for (fname, fe) in fields {
                    let fp = self.field_place(&p, fname, fe.span)?;
                    let v = self.expr(fe)?;
                    self.write(&fp, &v, fe.span)?;
                }
                Ok(V::Place(p))
            }
            ExprKind::ArrayList(list) => {
                let first = self.expr(&list[0])?;
                let et = match &first {
                    V::Place(p) => p.ty.clone(),
                    v => self.value_t(v).ok_or_else(|| ShaderError::new(e.span.start, e.span.end, "bad array element".into()))?,
                };
                let ew = self.words(&et);
                let n = list.len() as u32;
                let extent = image_words(u64::from(ew) * u64::from(n), e.span)?;
                let root = self.alloc_frame(extent, e.span)?;
                let p = Place { region: Region::Frame, root, extent, off: None, stat: 0, ty: T::Array(Box::new(et.clone()), n) };
                for (k, x) in list.iter().enumerate() {
                    let v = if k == 0 { first.clone() } else { self.expr(x)? };
                    let ep = Place { ty: et.clone(), stat: k as u32 * ew, ..p.clone() };
                    self.write(&ep, &v, x.span)?;
                }
                Ok(V::Place(p))
            }
            ExprKind::ArrayRepeat(el, count) => {
                let n = self.const_count(count)?;
                let v = self.expr(el)?;
                let et = match &v {
                    V::Place(p) => p.ty.clone(),
                    v => self.value_t(v).unwrap(),
                };
                let ew = self.words(&et);
                let extent = image_words(u64::from(ew) * u64::from(n), e.span)?;
                let root = self.alloc_frame(extent, e.span)?;
                let p = Place { region: Region::Frame, root, extent, off: None, stat: 0, ty: T::Array(Box::new(et.clone()), n) };
                if n > 16 {
                    return err(e.span, "a local array filled with a non-constant needs at most 16 elements; use a loop");
                }
                for k in 0..n {
                    let ep = Place { ty: et.clone(), stat: k * ew, ..p.clone() };
                    self.write(&ep, &v, e.span)?;
                }
                Ok(V::Place(p))
            }
            _ => unreachable!(),
        }
    }

    /// A pure constant expression the builder cannot fold by itself (a
    /// helper with branches and returns: `linear_rgb(#eee9df)`), run once
    /// at lowering: only constants and tables are in scope, and a program
    /// that reads anything else (params, inputs, state, the time) is not a
    /// constant (None). Its value as a constant initializer.
    fn fold_const(&mut self, e: &Expr) -> LResult<Option<CInit>> {
        if self.control.is_some_and(|word| word.load(Ordering::Relaxed) != 0) {
            return Err(ShaderError::cancelled());
        }
        let base = self.shared_init.len() as u32;
        let end = image_words(u64::from(base) + 16, e.span)?;
        let scratch = self.scratch();
        let saved_b = std::mem::replace(&mut self.b, scratch);
        let saved_frame = std::mem::replace(&mut self.frame_words, 0);
        let saved_entry = self.entry.take();
        let saved_globals = self.globals.clone();
        let saved_init = std::mem::replace(&mut self.in_init, true);
        self.globals.retain(|_, b| matches!(b, Bind::Const(_) | Bind::ConstVec(..) | Bind::Scalar(..)) || matches!(b, Bind::Place(p) if p.region == Region::Shared));
        let r = self.expr(e).map(|v| {
            let (lanes, ty): (Vec<Val>, Option<T>) = match v {
                V::F(x) => (vec![x], Some(T::F)),
                V::I(x) => (vec![x], Some(T::I)),
                V::Vec(n, l) => (l[..n as usize].to_vec(), Some(T::Vec(n))),
                _ => (Vec::new(), None),
            };
            for (k, x) in lanes.iter().enumerate() {
                self.b.store(Region::Shared, base + k as u32, 1, None, *x);
            }
            ty
        });
        self.in_init = saved_init;
        self.globals = saved_globals;
        self.entry = saved_entry;
        let frame_words = std::mem::replace(&mut self.frame_words, saved_frame);
        let b = std::mem::replace(&mut self.b, saved_b);
        let Some(ty) = speculative(r)?.flatten() else { return Ok(None) };
        let mut prog = b.prog;
        prog.body = b.blocks.into_iter().next().unwrap();
        prog.frame_words = frame_words;
        let mut regions = ir::Regions { ctx: 0, state: 0, shared: end, frame: frame_words, shared_writable: true, bufs: Vec::new(), io: false };
        if ir::validate(&prog, &regions).is_err() {
            return Ok(None);
        }
        if prog.total_cost() > MAX_INIT_COST {
            return err(e.span, format!("this constant may take {} operations to compute; the limit is {}", prog.total_cost(), MAX_INIT_COST));
        }
        // Validate and price source AIR before granting its generated polls buffer zero.
        let buf = if let Some(control) = self.control {
            prog.poll_cancel_at_loops();
            regions.bufs.push(false);
            ir::validate(&prog, &regions).map_err(|message| ShaderError::new(e.span.start, e.span.end,
                format!("internal compiler error in constant evaluation: {message}")))?;
            Some(ir::RawBuf { ptr: std::ptr::from_ref(control).cast::<u32>().cast_mut(), len: 1, writable: false, elem: ir::Elem::Word })
        } else {
            None
        };
        let mut shared = Vec::new();
        reserve_words(&mut shared, end, e.span)?;
        shared.extend_from_slice(&self.shared_init);
        shared.resize(end as usize, 0);
        let mut scratch = vec![0u32; prog.scratch_words()];
        let (mut ctx, mut state) = ([0u32; 0], [0u32; 0]);
        let mut mem = ir::Mem { ctx: &mut ctx, state: &mut state, shared: ir::Shared::Write(&mut shared), bufs: buf.as_slice() };
        let mut io = ir::Io::default();
        ir::run(&prog, &mut scratch, &mut mem, &mut io, 1);
        if self.control.is_some_and(|word| word.load(Ordering::Relaxed) != 0) {
            return Err(ShaderError::cancelled());
        }
        let w = |k: u32| shared[(base + k) as usize];
        Ok(Some(match ty {
            T::Vec(n) => {
                let mut lanes = [0u32; 16];
                for (k, l) in lanes.iter_mut().enumerate().take(n as usize) {
                    *l = w(k as u32);
                }
                CInit::Vec(n, lanes)
            }
            t => CInit::W(w(0), t),
        }))
    }

    /// `cinit` without touching the main builder when it fails.
    fn cinit_quiet(&mut self, e: &Expr) -> LResult<CInit> {
        let saved_frame = self.frame_words;
        let r = self.cinit(e, None);
        self.frame_words = saved_frame;
        r
    }

    fn ident(&mut self, name: &str, span: Span) -> LResult<V> {
        match self.lookup(name) {
            Some(Bind::Const(v)) | Some(Bind::Value(v)) => return Ok(v),
            Some(Bind::Closure(..)) => {
                return err(span, format!("`{}` is a function: call it (`{}(..)`); kernels cannot store, pass or return functions", name, name))
            }
            Some(Bind::Scalar(t, w)) => {
                return Ok(if t == T::I { V::I(self.b.ci(w as i32)) } else { V::F(self.b.cf(f32::from_bits(w))) });
            }
            Some(Bind::ConstVec(n, w)) => {
                let mut lanes = [Val(0); 16];
                for (k, l) in lanes.iter_mut().enumerate().take(n as usize) {
                    *l = self.b.cf(f32::from_bits(w[k]));
                }
                return Ok(V::Vec(n, lanes));
            }
            Some(Bind::AudioPort(k)) => {
                let port = &self.ports[k];
                let mut lanes = [Val(0); 16];
                for lane in 0..port.info.channels {
                    lanes[usize::from(lane)] = if port.info.output {
                        if self.in_init { self.b.cf(0.0) } else if self.entry.is_some() {
                            self.b.get(port.held[usize::from(lane)])
                        } else {
                            return err(span, format!("output port `{name}` is not a constant"));
                        }
                    } else if let Some(idx) = self.port_frame {
                        self.b.def(Ty::F32, Op::In { ch: port.info.first + lane, idx })
                    } else {
                        return err(span, format!("input port `{name}` can only be read in voice() or effect()"));
                    };
                }
                return Ok(if port.info.channels == 1 { V::F(lanes[0]) } else { V::Vec(2, lanes) });
            }
            Some(Bind::Local(t, vars)) => return Ok(self.read_vars(&t, &vars)),
            Some(Bind::Place(p)) => return Ok(self.read(&p)),
            Some(Bind::Param(k)) => {
                let Some(ent) = self.entry.clone() else {
                    return err(span, format!("param `{}` is not available here", name));
                };
                let (base, step) = ent.params[k];
                let pos = self.b.get(ent.pos);
                let mask = self.b.ci(CONTROL_BLOCK as i32 - 1);
                let ph = self.b.ib(Bin::AndI, pos, mask);
                let one = self.b.ci(1);
                let ph = self.b.ib(Bin::AddI, ph, one);
                let t = self.b.un(Ty::F32, Un::I2F, ph);
                let bv = self.b.get(base);
                let sv = self.b.get(step);
                let d = self.b.mul(sv, t);
                return Ok(V::F(self.b.add(bv, d)));
            }
            Some(Bind::Uniform(k)) => return Ok(V::F(self.b.load(Ty::F32, Region::Ctx, kernel::K_PARAMS + k as u32, 1, None))),
            Some(Bind::Emit(..)) => return err(span, format!("`{}` is an emit buffer: write it with emit({}, ...)", name, name)),
            None => {}
        }
        if self.domain == Domain::Kernel {
            if let Some(v) = self.kernel_ident(name) {
                return Ok(v);
            }
        }
        if self.event.integer_literals && matches!(name, "PI" | "TAU" | "E") {
            return err(span, format!("{name} needs a float type; this program has none"));
        }
        match name {
            "oversampling" if self.domain == Domain::Audio && self.oversampling == 0 => return err(span, "the rate factor cannot depend on itself"),
            "oversampling" if self.domain == Domain::Audio => return Ok(V::Lit(self.oversampling as f64)),
            "PI" => return Ok(V::Lit(std::f64::consts::PI)),
            "TAU" => return Ok(V::Lit(std::f64::consts::TAU)),
            "E" => return Ok(V::Lit(std::f64::consts::E)),
            _ => {}
        }
        if INPUT_NAMES.contains(&name) && self.domain == Domain::Audio {
            let Some(ent) = self.entry.clone() else {
                return err(span, format!("`{}` is an audio input: not available in init() or initial values", name));
            };
            return Ok(match name {
                "sample_rate" | "SR" => V::F(self.b.get(ent.rate)),
                "tempo" => V::F(ent.tempo),
                "beats_per_bar" => V::F(ent.bar_beats),
                "playing" => V::B(ent.playing),
                "voice_index" => V::I(ent.voice_index),
                "instance" => V::F(ent.instance),
                "beat" | "bar" => {
                    let pos = self.b.get(ent.pos);
                    let elapsed = self.b.ib(Bin::SubI, pos, ent.beat_frame);
                    let elapsed = self.b.un(Ty::F32, Un::I2F, elapsed);
                    let advance = self.b.mul(elapsed, ent.beat_step);
                    let beat = self.b.add(ent.beat_at, advance);
                    V::F(if name == "bar" { self.b.div(beat, ent.bar_beats) } else { beat })
                }
                "note" => { self.reads_note = true; V::F(self.b.get(ent.note)) }
                "freq" => { self.reads_freq = true; V::F(self.b.get(ent.freq)) }
                "gate" => V::F(self.b.get(ent.gate)),
                "velocity" => V::F(self.b.get(ent.velocity)),
                "bend" => V::F(self.b.get(ent.bend)),
                "pressure" => V::F(self.b.get(ent.pressure)),
                "slide" => V::F(self.b.get(ent.slide)),
                "release_velocity" => V::F(self.b.get(ent.release_velocity)),
                "mod_wheel" => V::F(self.b.get(ent.mod_wheel)),
                "trigger" => {
                    let pos = self.b.get(ent.pos);
                    let at = self.b.get(ent.trigger_at);
                    let triggered = self.b.cmpi(Cmp::Eq, pos, at);
                    V::B(if let Some((i, _)) = ent.subsample {
                        let zero = self.b.ci(0);
                        let first = self.b.cmpi(Cmp::Eq, i, zero);
                        self.b.def(Ty::Bool, Op::Bin(Bin::AndB, triggered, first))
                    } else {
                        triggered
                    })
                }
                _ => unreachable!(),
            });
        }
        if self.in_init && self.domain == Domain::Audio && self.state_vars.iter().any(|v| v.name == name) {
            return err(span, format!("state variable `{name}` is unavailable in init()"));
        }
        let hint = self.suggest(name, false);
        err(span, format!("unknown name `{}`{}", name, hint))
    }

    /// ", did you mean `x`?" for a near miss among the names in scope (or
    /// the functions, for a call).
    fn suggest(&self, name: &str, call: bool) -> String {
        let mut cands: Vec<String> = Vec::new();
        if call {
            cands.extend(self.fns.keys().cloned());
            cands.extend(BUILTIN_FNS.iter().map(|s| s.to_string()));
        } else {
            if let Some(frame) = self.frames.last() {
                for scope in frame {
                    cands.extend(scope.keys().cloned());
                }
            }
            cands.extend(self.globals.keys().cloned());
            cands.extend(INPUT_NAMES.iter().map(|s| s.to_string()));
            cands.extend(["PI", "TAU", "E"].iter().map(|s| s.to_string()));
        }
        let best = cands
            .iter()
            .filter(|c| !c.contains("fuse_"))
            .map(|c| (edit_distance(name, c), c))
            .filter(|(d, c)| *d <= 2.max(name.len() / 4) && *d < c.len())
            .min();
        match best {
            Some((_, c)) => format!(", did you mean `{}`?", c),
            None => String::new(),
        }
    }

    /// A mark for the source condition at `span`, taken now, before its branches lower, so its id
    /// comes before theirs as marks' ids follow program order; the caller places it right before
    /// the condition's `If`, when it makes one. `tests` is whether that `If` tests the condition
    /// itself (false: its negation, as a `while`'s exit does). None while marks are off, in
    /// scratch lowering, or for the prelude's conditions.
    fn cond_site(&mut self, span: Span, tests: bool) -> Option<u32> {
        if !self.marks || self.b.scratch || span.start >= self.prelude_base {
            return None;
        }
        let id = self.marks_spans.len() as u32;
        self.marks_spans.push(span);
        self.cond_marks.push((id, tests));
        Some(id)
    }

    fn if_expr(&mut self, arms: &[(Expr, Vec<crate::parse::Stmt>)], else_: Option<&Vec<crate::parse::Stmt>>, span: Span) -> LResult<V> {
        // Conditions after the first are evaluated only when reached, so
        // build nested ifs.
        let (c0, b0) = &arms[0];
        let cv = self.expr(c0)?;
        let c = self.truth(&cv, c0.span)?;
        let site = self.cond_site(c0.span, true);
        let rest = &arms[1..];
        self.if2(c, site, &|l: &mut Self| l.scoped(|l| l.stmts(b0)), &|l: &mut Self| {
            if !rest.is_empty() {
                l.if_expr(rest, else_, span)
            } else if let Some(e) = else_ {
                l.scoped(|l| l.stmts(e))
            } else {
                Ok(V::Unit)
            }
        }, span)
    }

    fn if_chain(&mut self, chain: &[(Val, &Vec<crate::parse::Stmt>)], default: Option<&Vec<crate::parse::Stmt>>, span: Span) -> LResult<V> {
        if chain.is_empty() {
            return match default {
                Some(d) => self.scoped(|l| l.stmts(d)),
                None => Ok(V::Unit),
            };
        }
        let (c, body) = chain[0];
        let rest = &chain[1..];
        self.if2(c, None, &|l: &mut Self| l.scoped(|l| l.stmts(body)), &|l: &mut Self| l.if_chain(rest, default, span), span)
    }

    /// Two-way conditional; merges values through a select (both sides
    /// pure) or result variables. `site` is the source condition's mark
    /// ([`Lowerer::cond_site`]), placed before the `If` when there is one.
    fn if2(
        &mut self,
        c: Val,
        site: Option<u32>,
        then_: &dyn Fn(&mut Self) -> LResult<V>,
        else_: &dyn Fn(&mut Self) -> LResult<V>,
        span: Span,
    ) -> LResult<V> {
        if let Some(k) = self.b.cst(c) {
            return if k != 0 { then_(self) } else { else_(self) };
        }
        self.b.open();
        let tv = then_(self);
        let tb = self.b.close();
        let tv = tv?;
        self.b.open();
        let ev = else_(self);
        let eb = self.b.close();
        let ev = ev?;
        let t_diverges = matches!(ir::unmarked(&tb).next_back(), Some(IS::Break(_)) | Some(IS::Continue(_)));
        let e_diverges = matches!(ir::unmarked(&eb).next_back(), Some(IS::Break(_)) | Some(IS::Continue(_)));
        let tt = self.value_t(&tv);
        let et = self.value_t(&ev);
        let rt = match (&tt, &et) {
            (Some(a), Some(b)) => {
                if a == b || (matches!(tv, V::Lit(_)) && *b != T::B) {
                    Some(b.clone())
                } else if matches!(ev, V::Lit(_)) && *a != T::B {
                    Some(a.clone())
                } else if (*a == T::F && *b == T::I) || (*a == T::I && *b == T::F) {
                    Some(T::F)
                } else if (*a == T::D && matches!(b, T::F | T::I)) || (*b == T::D && matches!(a, T::F | T::I)) {
                    Some(T::D)
                } else if matches!((a, b), (T::Vec(_), T::F | T::I) | (T::F | T::I, T::Vec(_))) {
                    Some(if let T::Vec(n) = a { T::Vec(*n) } else { b.clone() })
                } else {
                    return err(span, format!("the branches give {} and {}", self.vname(&tv), self.vname(&ev)));
                }
            }
            (Some(a), None) if e_diverges => Some(a.clone()),
            (None, Some(b)) if t_diverges => Some(b.clone()),
            _ => None,
        };
        let Some(rt) = rt else {
            if let Some(id) = site {
                self.b.push(IS::Mark(id));
            }
            self.b.push(IS::If(c, tb, eb));
            return Ok(V::Unit);
        };
        // Both branches only compute: evaluate both, select.
        // Small branches that only compute: evaluate both and select (no
        // branch). Big ones branch, so only the taken side costs.
        if Builder::pure_block(&tb) && Builder::pure_block(&eb) && ir::unmarked(&tb).count() + ir::unmarked(&eb).count() <= 24 && tt.is_some() && et.is_some() {
            self.b.splice(tb.into_iter().filter(|s| !matches!(s, IS::Mark(_))).collect());
            self.b.splice(eb.into_iter().filter(|s| !matches!(s, IS::Mark(_))).collect());
            return self.select_v(c, &rt, &tv, &ev, span);
        }
        let vars = match &rt {
            T::Vec(n) => (0..*n).map(|_| self.b.var(Ty::F32)).collect(),
            t => vec![self.b.var(ty_of(t))],
        };
        // An unconditional first write keeps the variables' live ranges
        // inside this block (see arm64 liveness).
        self.zero_vars(&vars);
        let mut tb = tb;
        let mut eb = eb;
        if !t_diverges {
            self.b.open();
            self.assign_vars(&rt, &vars, &tv, span)?;
            tb.extend(self.b.close());
        }
        if !e_diverges {
            self.b.open();
            self.assign_vars(&rt, &vars, &ev, span)?;
            eb.extend(self.b.close());
        }
        if let Some(id) = site {
            self.b.push(IS::Mark(id));
        }
        self.b.push(IS::If(c, tb, eb));
        Ok(self.read_vars(&rt, &vars))
    }

    fn select_v(&mut self, c: Val, rt: &T, a: &V, b: &V, span: Span) -> LResult<V> {
        Ok(match rt {
            T::F => {
                let x = self.to_f(a, span)?;
                let y = self.to_f(b, span)?;
                V::F(self.b.sel(c, x, y))
            }
            T::I => {
                let x = self.to_i(a, span)?;
                let y = self.to_i(b, span)?;
                V::I(self.b.sel(c, x, y))
            }
            T::B => {
                let (V::B(x), V::B(y)) = (a, b) else { unreachable!() };
                V::B(self.b.sel(c, *x, *y))
            }
            T::D => {
                let x = self.to_d(a, span)?;
                let y = self.to_d(b, span)?;
                V::D(self.b.sel(c, x, y))
            }
            T::Vec(n) => {
                let la = self.to_vec(a, *n, span)?;
                let lb = self.to_vec(b, *n, span)?;
                let mut o = la;
                for k in 0..*n as usize {
                    o[k] = self.b.sel(c, la[k], lb[k]);
                }
                V::Vec(*n, o)
            }
            _ => unreachable!(),
        })
    }

    fn binary(&mut self, op: BinOp, a: V, b: V, span: Span) -> LResult<V> {
        use BinOp::*;
        self.taint_use(Some(op), &a, &b);
        // Literal folding (f64, deterministic).
        if let (V::Lit(x), V::Lit(y)) = (&a, &b) {
            let (x, y) = (*x, *y);
            let bits = |v: f64| v as i64;
            if self.event.integer_literals && matches!(op, Div | Rem) && y == 0.0 {
                return err(span, "division by zero in a constant");
            }
            let v = match op {
                Add => V::Lit(x + y),
                Sub => V::Lit(x - y),
                Mul => V::Lit(x * y),
                Div if self.event.integer_literals => V::Lit((x / y).trunc()),
                Div => V::Lit(x / y),
                Rem => V::Lit(x - y * (x / y).trunc()),
                Lt | Le | Gt | Ge | Eq | Ne => {
                    let r = match op {
                        Lt => x < y,
                        Le => x <= y,
                        Gt => x > y,
                        Ge => x >= y,
                        Eq => x == y,
                        _ => x != y,
                    };
                    V::B(self.b.cb(r))
                }
                BitAnd => V::Lit((bits(x) & bits(y)) as f64),
                BitOr => V::Lit((bits(x) | bits(y)) as f64),
                BitXor => V::Lit((bits(x) ^ bits(y)) as f64),
                Shl => V::Lit(((bits(x) as i32).wrapping_shl(bits(y) as u32)) as f64),
                Shr => V::Lit(((bits(x) as i32).wrapping_shr(bits(y) as u32)) as f64),
                ShrU => V::Lit(((bits(x) as i32 as u32).wrapping_shr(bits(y) as u32)) as i32 as f64),
                And | Or => unreachable!(),
            };
            if let V::Lit(x) = &v {
                if self.event.integer_literals {
                    self.literal_i32(*x, span)?;
                }
            }
            return Ok(v);
        }
        // Matrix products.
        if let (V::Vec(16, m), Mul) = (&a, op) {
            match &b {
                V::Vec(4, v) => return Ok(self.mat_vec(m, v)),
                V::Vec(16, m2) => {
                    let mut o = [Val(0); 16];
                    for c in 0..4 {
                        let col = lanes_of(&m2[c * 4..c * 4 + 4]);
                        let V::Vec(_, r) = self.mat_vec(m, &col) else { unreachable!() };
                        o[c * 4..c * 4 + 4].copy_from_slice(&r[..4]);
                    }
                    return Ok(V::Vec(16, o));
                }
                _ => {}
            }
        }
        // Vector arithmetic, lane-wise with scalar broadcast.
        if matches!(a, V::Vec(..)) || matches!(b, V::Vec(..)) {
            let n = self.vec_width(&[&a, &b], span)?.unwrap();
            let la = self.to_vec(&a, n, span)?;
            let lb = self.to_vec(&b, n, span)?;
            let fop = match op {
                Add => Bin::AddF,
                Sub => Bin::SubF,
                Mul => Bin::MulF,
                Div => Bin::DivF,
                _ => return err(span, format!("{} supports + - * / (compare components one by one)", vec_name(n))),
            };
            let mut o = la;
            for k in 0..n as usize {
                o[k] = self.b.fb(fop, la[k], lb[k]);
            }
            return Ok(V::Vec(n, o));
        }
        // f64: the other side widens exactly.
        if matches!(a, V::D(_)) || matches!(b, V::D(_)) {
            let x = self.to_d(&a, span)?;
            let y = self.to_d(&b, span)?;
            let arith = |l: &mut Self, op: Bin| Ok(V::D(l.b.def(Ty::F64, Op::Bin(op, x, y))));
            let c = |l: &mut Self, cc| Ok(V::B(l.b.def(Ty::Bool, Op::CmpD(cc, x, y))));
            return match op {
                Add => arith(self, Bin::AddD),
                Sub => arith(self, Bin::SubD),
                Mul => arith(self, Bin::MulD),
                Div => arith(self, Bin::DivD),
                Rem => {
                    // a - b * trunc(a / b)
                    let q = self.b.def(Ty::F64, Op::Bin(Bin::DivD, x, y));
                    let t = self.b.un(Ty::F64, Un::TruncD, q);
                    let m = self.b.def(Ty::F64, Op::Bin(Bin::MulD, y, t));
                    Ok(V::D(self.b.def(Ty::F64, Op::Bin(Bin::SubD, x, m))))
                }
                Lt => c(self, Cmp::Lt),
                Le => c(self, Cmp::Le),
                Gt => c(self, Cmp::Gt),
                Ge => c(self, Cmp::Ge),
                Eq => c(self, Cmp::Eq),
                Ne => c(self, Cmp::Ne),
                _ => err(span, "f64 supports + - * / % and comparisons"),
            };
        }
        // Bools.
        if matches!(a, V::B(_)) || matches!(b, V::B(_)) {
            let (V::B(x), V::B(y)) = (&a, &b) else {
                return err(span, format!("cannot combine {} and {}", self.vname(&a), self.vname(&b)));
            };
            return Ok(V::B(match op {
                Eq => self.b.cmpi(Cmp::Eq, *x, *y),
                Ne => self.b.cmpi(Cmp::Ne, *x, *y),
                BitAnd => self.b.def(Ty::Bool, Op::Bin(Bin::AndB, *x, *y)),
                BitOr => self.b.def(Ty::Bool, Op::Bin(Bin::OrB, *x, *y)),
                BitXor => self.b.cmpi(Cmp::Ne, *x, *y),
                _ => return err(span, "bools support == != & | ^ && ||"),
            }));
        }
        let int = match (&a, &b) {
            (V::I(_), V::I(_)) => true,
            (V::I(_), V::Lit(y)) | (V::Lit(y), V::I(_)) => y.fract() == 0.0,
            (V::F(_) | V::Lit(_), V::F(_) | V::Lit(_)) | (V::F(_), V::I(_)) | (V::I(_), V::F(_)) => false,
            _ => return err(span, format!("cannot combine {} and {}", self.vname(&a), self.vname(&b))),
        };
        let bitop = matches!(op, BitAnd | BitOr | BitXor | Shl | Shr | ShrU);
        if bitop && !int {
            return err(span, "bit operators need integers (use int(x))");
        }
        if int {
            let x = self.to_i(&a, span)?;
            let y = self.to_i(&b, span)?;
            let c = |l: &mut Self, cc| Ok(V::B(l.b.cmpi(cc, x, y)));
            return match op {
                Add => Ok(V::I(self.b.ib(Bin::AddI, x, y))),
                Sub => Ok(V::I(self.b.ib(Bin::SubI, x, y))),
                Mul => Ok(V::I(self.b.ib(Bin::MulI, x, y))),
                Div => Ok(V::I(self.b.ib(Bin::DivI, x, y))),
                Rem => Ok(V::I(self.b.ib(Bin::RemI, x, y))),
                BitAnd => Ok(V::I(self.b.ib(Bin::AndI, x, y))),
                BitOr => Ok(V::I(self.b.ib(Bin::OrI, x, y))),
                BitXor => Ok(V::I(self.b.ib(Bin::XorI, x, y))),
                Shl => Ok(V::I(self.b.ib(Bin::ShlI, x, y))),
                Shr => Ok(V::I(self.b.ib(Bin::ShrI, x, y))),
                ShrU => Ok(V::I(self.b.ib(Bin::ShrUI, x, y))),
                Lt => c(self, Cmp::Lt),
                Le => c(self, Cmp::Le),
                Gt => c(self, Cmp::Gt),
                Ge => c(self, Cmp::Ge),
                Eq => c(self, Cmp::Eq),
                Ne => c(self, Cmp::Ne),
                And | Or => unreachable!(),
            };
        }
        let x = self.to_f(&a, span)?;
        let y = self.to_f(&b, span)?;
        let c = |l: &mut Self, cc| Ok(V::B(l.b.cmpf(cc, x, y)));
        match op {
            Add => Ok(V::F(self.b.add(x, y))),
            Sub => Ok(V::F(self.b.sub(x, y))),
            Mul => Ok(V::F(self.b.mul(x, y))),
            Div => Ok(V::F(self.b.div(x, y))),
            Rem => {
                // a - b * trunc(a / b)
                let q = self.b.div(x, y);
                let t = self.b.un(Ty::F32, Un::TruncF, q);
                let m = self.b.mul(y, t);
                Ok(V::F(self.b.sub(x, m)))
            }
            Lt => c(self, Cmp::Lt),
            Le => c(self, Cmp::Le),
            Gt => c(self, Cmp::Gt),
            Ge => c(self, Cmp::Ge),
            Eq => c(self, Cmp::Eq),
            Ne => c(self, Cmp::Ne),
            _ => unreachable!(),
        }
    }

    // -- builtin calls -----------------------------------------------------------

    /// f64 arithmetic and portable math: `f64(x)`; math on f64 arguments
    /// (always the portable kernels, giving f64); f32 math in
    /// `math: portable` (in f64, rounded once); and asin, acos, hypot and
    /// cbrt, which only exist in the portable form.
    fn f64_call(&mut self, name: &str, vals: &[V], args: &[Expr], span: Span) -> LResult<Option<V>> {
        type D1 = fn(&mut Builder, Val) -> Val;
        type D2 = fn(&mut Builder, Val, Val) -> Val;
        let un: Option<D1> = match name {
            "sin" => Some(|b, x| b.p_sin(x)),
            "cos" => Some(|b, x| b.p_cos(x)),
            "tan" => Some(|b, x| b.p_tan(x)),
            "atan" => Some(|b, x| b.p_atan(x)),
            "asin" => Some(|b, x| b.p_asin_acos(x, false)),
            "acos" => Some(|b, x| b.p_asin_acos(x, true)),
            "exp" => Some(|b, x| b.p_exp(x)),
            "log" | "ln" => Some(|b, x| b.p_ln(x)),
            "cbrt" => Some(|b, x| b.p_cbrt(x)),
            "sqrt" => Some(|b, x| b.un(Ty::F64, Un::SqrtD, x)),
            "abs" => Some(|b, x| b.un(Ty::F64, Un::AbsD, x)),
            "floor" => Some(|b, x| b.un(Ty::F64, Un::FloorD, x)),
            "ceil" => Some(|b, x| b.un(Ty::F64, Un::CeilD, x)),
            "round" => Some(|b, x| b.un(Ty::F64, Un::RoundD, x)),
            "trunc" => Some(|b, x| b.un(Ty::F64, Un::TruncD, x)),
            "fract" => Some(|b, x| {
                let f = b.un(Ty::F64, Un::FloorD, x);
                b.def(Ty::F64, Op::Bin(Bin::SubD, x, f))
            }),
            _ => None,
        };
        let bin: Option<D2> = match name {
            "pow" => Some(|b, x, y| b.p_pow(x, y)),
            "atan2" => Some(|b, y, x| b.p_atan2(y, x)),
            "hypot" => Some(|b, x, y| b.p_hypot(x, y)),
            "min" => Some(|b, x, y| b.def(Ty::F64, Op::Bin(Bin::MinD, x, y))),
            "max" => Some(|b, x, y| b.def(Ty::F64, Op::Bin(Bin::MaxD, x, y))),
            _ => None,
        };
        let any_d = vals.iter().any(|v| matches!(v, V::D(_)));
        // Functions that are only portable, and the f32 math that
        // `math: portable` routes through f64.
        let only_portable = matches!(name, "asin" | "acos" | "hypot" | "cbrt");
        let exact = matches!(name, "sqrt" | "abs" | "floor" | "ceil" | "round" | "trunc" | "fract" | "min" | "max");
        let portable_f32 = (self.portable || only_portable) && !exact;
        if name == "f64" {
            if vals.len() != 1 {
                return err(span, "`f64` takes 1 argument");
            }
            return Ok(Some(V::D(self.to_d(&vals[0], args[0].span)?)));
        }
        if let Some(f) = un {
            if vals.len() != 1 || !(any_d || portable_f32) {
                return Ok(None);
            }
            return Ok(Some(match &vals[0] {
                V::D(x) => V::D(f(&mut self.b, *x)),
                V::Vec(n, l) => {
                    let mut o = *l;
                    for k in 0..*n as usize {
                        let d = self.b.f2d(l[k]);
                        let r = f(&mut self.b, d);
                        o[k] = self.b.d2f(r);
                    }
                    V::Vec(*n, o)
                }
                v => {
                    let x = self.to_f(v, args[0].span)?;
                    let d = self.b.f2d(x);
                    let r = f(&mut self.b, d);
                    V::F(self.b.d2f(r))
                }
            }));
        }
        if let Some(f) = bin {
            if vals.len() != 2 || !(any_d || portable_f32) {
                return Ok(None);
            }
            if vals.iter().any(|v| matches!(v, V::Vec(..))) {
                return err(span, format!("`{}` on vectors: apply it per component", name));
            }
            if any_d {
                let x = self.to_d(&vals[0], args[0].span)?;
                let y = self.to_d(&vals[1], args[1].span)?;
                return Ok(Some(V::D(f(&mut self.b, x, y))));
            }
            let x = self.to_f(&vals[0], args[0].span)?;
            let y = self.to_f(&vals[1], args[1].span)?;
            let x = self.b.f2d(x);
            let y = self.b.f2d(y);
            let r = f(&mut self.b, x, y);
            return Ok(Some(V::F(self.b.d2f(r))));
        }
        if any_d && matches!(name, "clamp" | "mix" | "lerp") {
            if vals.len() != 3 {
                return err(span, format!("`{}` takes 3 arguments", name));
            }
            let x = self.to_d(&vals[0], args[0].span)?;
            let y = self.to_d(&vals[1], args[1].span)?;
            let z = self.to_d(&vals[2], args[2].span)?;
            let b = &mut self.b;
            return Ok(Some(V::D(if name == "clamp" {
                let m = b.def(Ty::F64, Op::Bin(Bin::MaxD, x, y));
                b.def(Ty::F64, Op::Bin(Bin::MinD, m, z))
            } else {
                let d = b.def(Ty::F64, Op::Bin(Bin::SubD, y, x));
                let d = b.def(Ty::F64, Op::Bin(Bin::MulD, d, z));
                b.def(Ty::F64, Op::Bin(Bin::AddD, x, d))
            })));
        }
        Ok(None)
    }

    fn call_expr(&mut self, name: &str, args: &[Expr], span: Span) -> LResult<V> {
        self.call_expr_bound(name, args, span, &mut None)
    }

    fn halfband_call(&mut self, name: &str, args: Vec<V>, span: Span) -> LResult<V> {
        let f = self.fns.get(name).cloned().ok_or_else(|| ShaderError::new(span.start, span.end, format!("oversample needs `{name}` in the prelude")))?;
        self.call(&f, args, span)
    }

    fn oversample(&mut self, args: &[Expr], span: Span) -> LResult<V> {
        if self.fns.contains_key("effect_block") && self.domain == Domain::Audio {
            return err(span, block::BASE_RATE);
        }
        if self.entry.is_none() || self.b.scratch {
            return err(span, "oversample() runs on the audio path");
        }
        let [state, input, body, modes @ ..] = args else { return err(span, "expected oversample(state, input, fn(u) { .. })") };
        let modes = RateModes::parse(modes, span)?;
        let ExprKind::Lambda(f) = &body.kind else { return err(body.span, "write the oversample body here as fn(u) { .. }") };
        let p = self.place(state)?;
        let state_name = self.tname(&p.ty);
        let factor = match state_name.as_str() {
            "Oversample2" => 2,
            "Oversample4" => 4,
            "Oversample8" => 8,
            _ => return err(state.span, "oversample needs Oversample2, Oversample4 or Oversample8 state"),
        };
        let [(param, ann)] = f.params.as_slice() else { return err(body.span, "the oversample body takes one sample") };
        let reads_input = reads_parameter(&f.body, param);
        let x = self.expr(input)?;
        let channels = match x {
            V::F(_) | V::Lit(_) => 1,
            V::Vec(2, _) => 2,
            _ => return err(input.span, "oversample takes an f32 or vec2 sample"),
        };
        if !matches!((channels, ann), (_, None) | (1, Some(TypeAnn::F32)) | (2, Some(TypeAnn::Vec2))) {
            return err(body.span, "the oversample parameter must have the input sample's type");
        }
        let down = self.field_place(&p, "down", state.span)?;
        let input = self.to_vec(&x, channels, input.span)?;
        let up = reads_input.then_some((p, input));
        let frame = |v: Lanes| if channels == 1 { V::F(v[0]) } else { V::Vec(2, v) };
        let scopes = self.frames.last().unwrap().clone();
        let output = self.rate_region(self.oversampling, factor, modes, up, down, channels, span, |l, input| {
            let y = l.inline_call(f, vec![frame(input)], body.span, scopes)?;
            if l.value_t(&y) != l.value_t(&x) {
                return err(body.span, "the oversample body must return the input sample's type");
            }
            l.to_vec(&y, channels, body.span)
        })?;
        Ok(frame(output))
    }

    fn rate_region(&mut self, parent_rate: u32, factor: u32, modes: RateModes, up: Option<(Place, Lanes)>, down: Place, channels: u8, span: Span,
        body: impl FnOnce(&mut Self, Lanes) -> LResult<Lanes>) -> LResult<Lanes> {
        let (site, path, pads) = self.rate_nests.plan(parent_rate, factor, modes, up.is_some(), span)?;
        let mut inputs = Vec::new();
        let mut outputs = Vec::new();
        // The filters see a vector or four pairs. The loop indexes the
        // same contiguous frame words as individual f32 samples.
        let ty = if factor == 8 { T::Array(Box::new(T::Vec(2)), 4) } else { T::Vec(factor as u8) };
        for ch in 0..channels {
            let dst = Place { region: Region::Frame, root: self.alloc_frame(factor, span)?, extent: factor, off: None, stat: 0, ty: ty.clone() };
            if let Some((state, input)) = &up {
                let src = Place { root: self.alloc_frame(factor, span)?, ..dst.clone() };
                let x = input[ch as usize];
                let channel = V::Lit(ch as f64);
                match modes.up {
                    RateMode::Filter => {
                        let filters = self.field_place(state, "up", span)?;
                        let filter = self.index_place(&filters, &channel, span)?;
                        self.region_filter(filter, V::F(x), Some(&src), parent_rate, factor, pads[0], span)?;
                    }
                    RateMode::Linear => {
                        let last = self.field_place(state, "last", span)?;
                        let last = self.index_place(&last, &channel, span)?;
                        let prev = self.read(&last);
                        let prev = self.to_f(&prev, span)?;
                        let delta = self.b.sub(x, prev);
                        for j in 0..factor - 1 {
                            let step = self.b.mulc(delta, (j + 1) as f32 / factor as f32);
                            let sample = self.b.add(prev, step);
                            self.b.store(Region::Frame, src.root + j, 1, None, sample);
                        }
                        // Keep the endpoint exact even if x - prev lost precision.
                        self.b.store(Region::Frame, src.root + factor - 1, 1, None, x);
                        self.write(&last, &V::F(x), span)?;
                    }
                    RateMode::Hold => {
                        for j in 0..factor {
                            self.b.store(Region::Frame, src.root + j, 1, None, x);
                        }
                    }
                }
                if modes.up != RateMode::Filter && pads[0] != 0 {
                    let filters = self.field_place(state, "up", span)?;
                    let filter = self.index_place(&filters, &channel, span)?;
                    self.pad_region_buffer(&filter, &src, factor, pads[0], span)?;
                }
                inputs.push(src);
            }
            outputs.push(dst);
        }

        let outer_factor = self.oversampling;
        let outer_rate = self.entry.as_ref().unwrap().rate;
        // A whole entry already sees its raised clock in block().
        if path.absolute != outer_factor {
            let rate = self.b.get(outer_rate);
            let rate = self.b.mulc(rate, factor as f32);
            let high_rate = self.b.var(Ty::F32);
            self.b.set(high_rate, rate);
            self.entry.as_mut().unwrap().rate = high_rate;
        }
        let counter = self.b.var(Ty::I32);
        let zero = self.b.ci(0);
        self.b.set(counter, zero);
        self.b.open_loop();
        let i = self.b.get(counter);
        let mut lanes = if inputs.is_empty() { [self.b.cf(0.0); 16] } else { [Val(0); 16] };
        for (ch, src) in inputs.iter().enumerate() {
            lanes[ch] = self.b.load(Ty::F32, Region::Frame, src.root, factor, Some(i));
        }
        let outer_sample = self.entry.as_ref().unwrap().subsample;
        let subsample = if let Some((outer, count)) = outer_sample {
            let k = self.b.ci(factor as i32);
            let first = self.b.ib(Bin::MulI, outer, k);
            (self.b.ib(Bin::AddI, first, i), count * factor)
        } else { (i, factor) };
        self.entry.as_mut().unwrap().subsample = Some(subsample);
        self.oversampling = path.absolute;
        self.rate_nests.push(site, path);
        let result = body(self, lanes);
        self.oversampling = outer_factor;
        let ent = self.entry.as_mut().unwrap();
        ent.rate = outer_rate;
        ent.subsample = outer_sample;
        let latency = self.rate_nests.finish(factor, modes, self.latency, span);
        let lanes = result?;
        latency?;
        for (ch, dst) in outputs.iter().enumerate() {
            self.b.store(Region::Frame, dst.root, factor, Some(i), lanes[ch]);
        }
        // The last pass exits with i == factor - 1. The work counter can
        // then charge the fixed number of passes outside the sample loop.
        let last = self.b.ci(factor as i32 - 1);
        let done = self.b.cmpi(Cmp::Ge, i, last);
        self.b.push(IS::If(done, vec![IS::Break(0)], vec![]));
        let one = self.b.ci(1);
        let next = self.b.ib(Bin::AddI, i, one);
        self.b.set(counter, next);
        let body = self.b.close();
        self.b.push(IS::Loop { cap: factor, body });

        let mut output = [Val(0); 16];
        for (ch, dst) in outputs.iter().enumerate() {
            if modes.down != RateMode::Filter && pads[1] != 0 {
                let filter = self.index_place(&down, &V::Lit(ch as f64), span)?;
                self.pad_region_buffer(&filter, dst, factor, pads[1], span)?;
            }
            output[ch] = match modes.down {
                RateMode::Filter => {
                    let down_state = self.index_place(&down, &V::Lit(ch as f64), span)?;
                    let samples = self.read(dst);
                    let y = self.region_filter(down_state, samples, None, parent_rate, factor, pads[1], span)?;
                    self.to_f(&y, span)?
                }
                RateMode::Linear => {
                    let mut sum = self.b.load(Ty::F32, Region::Frame, dst.root, 1, None);
                    for j in 1..factor {
                        let sample = self.b.load(Ty::F32, Region::Frame, dst.root + j, 1, None);
                        sum = self.b.add(sum, sample);
                    }
                    self.b.mulc(sum, 1.0 / factor as f32)
                }
                RateMode::Hold => self.b.load(Ty::F32, Region::Frame, dst.root + factor - 1, 1, None),
            };
        }
        Ok(output)
    }

    fn probe(&mut self, args: &[Expr], span: Span) -> LResult<V> {
        if self.entry.is_none() || self.in_init || self.b.scratch {
            return err(span, "probe() runs on the audio path");
        }
        let [label, value] = args else { return err(span, "expected probe(name, value)") };
        let ExprKind::Ident(name) = &label.kind else { return err(label.span, "a probe name must be a bare word") };
        let v = self.expr(value)?;
        let x = match v {
            V::B(b) => {
                let one = self.b.cf(1.0);
                let zero = self.b.cf(0.0);
                self.b.sel(b, one, zero)
            }
            V::Vec(..) => return err(value.span, "probe one component at a time, such as `probe(left, v.x)`"),
            _ => self.to_f(&v, value.span)?,
        };
        let [last, min, max] = match self.probes.iter().find(|(n, _)| n == name) {
            Some((_, vars)) => *vars,
            None => {
                let vars = std::array::from_fn(|_| self.b.var(Ty::F32));
                self.probes.push((name.clone(), vars));
                vars
            }
        };
        let final_subsample = self.entry.as_ref().unwrap().subsample.map(|(i, count)| {
            let end = self.b.ci(count as i32 - 1);
            self.b.cmpi(Cmp::Eq, i, end)
        });
        if final_subsample.is_some() { self.b.open(); }
        self.b.set(last, x);
        let lo = self.b.get(min);
        let lo = self.b.fb(Bin::MinF, lo, x);
        self.b.set(min, lo);
        let hi = self.b.get(max);
        let hi = self.b.fb(Bin::MaxF, hi, x);
        self.b.set(max, hi);
        if let Some(at_end) = final_subsample {
            let record = self.b.close();
            self.b.push(IS::If(at_end, record, vec![]));
        }
        Ok(V::Unit)
    }

    /// Optionally records an integer min's constant upper bound for an event range.
    /// Ordinary call resolution and argument evaluation still happen exactly once.
    fn call_expr_bound(&mut self, name: &str, args: &[Expr], span: Span, min_bound: &mut Option<i32>) -> LResult<V> {
        if let Some(Bind::Closure(f, scopes)) = self.frames.last().and_then(|fr| fr.iter().rev().find_map(|s| s.get(name))).cloned() {
            let mut vals = Vec::new();
            for a in args {
                vals.push(self.expr(a)?);
            }
            // A local function named as a port: a value standing for a literal is no port's.
            for v in &vals {
                self.taint_use(None, v, &V::Unit);
            }
            // Its own name is in its scope: a call of itself is recursion
            // (an error), not an unknown name.
            let mut scopes = scopes;
            scopes.push(HashMap::from([(name.to_string(), Bind::Closure(f.clone(), Vec::new()))]));
            return self.inline_call(&f, vals, span, scopes);
        }
        if name == "emit" && self.domain == Domain::Kernel && !self.fns.contains_key("emit") {
            return self.kernel_emit(args, span);
        }
        if let Some(f) = self.fns.get(name).cloned() {
            if matches!(name, "voice" | "effect" | "block" | "init") || (self.domain == Domain::Audio && name == "effect_block") || self.event.entry_fns.iter().any(|e| e == name) {
                return err(span, format!("`{}` is an entry point and cannot be called", name));
            }
            let mut vals = Vec::new();
            for a in args {
                vals.push(self.expr(a)?);
            }
            let noted = self.traced_args(&mut vals);
            let v = self.call(&f, vals, span)?;
            if let Some(args) = noted {
                self.note_call(&f, args, &v);
            }
            return Ok(v);
        }
        if self.domain == Domain::Event {
            if let Some(k) = self.event.ports.iter().position(|p| p.name == name) {
                let mut vals = Vec::new();
                for a in args {
                    vals.push(self.expr(a)?);
                }
                return self.event_port(k, &vals, args, span);
            }
        }
        if self.domain == Domain::Audio && matches!(name, "fft" | "ifft") {
            return self.fft_call(name, args, span);
        }
        if self.domain == Domain::Audio && name == "probe" {
            return self.probe(args, span);
        }
        if self.domain == Domain::Audio && name == "oversample" {
            return self.oversample(args, span);
        }
        // `p.length()` on a plain path that names no module fn: the value
        // method, the receiver as the first argument.
        if let Some((recv, method)) = name.rsplit_once('.') {
            if let Some(f) = crate::parse::value_method(method) {
                let mut parts = recv.split('.');
                let mut e = Expr { kind: ExprKind::Ident(parts.next().unwrap_or_default().to_string()), span };
                for field in parts {
                    e = Expr { kind: ExprKind::Field(Box::new(e), field.to_string()), span };
                }
                let mut full = vec![e];
                full.extend(args.iter().cloned());
                return self.call_expr(f, &full, span);
            }
        }
        // Builtins written as other expressions.
        let num = |v: f64| Expr { kind: ExprKind::Num(v, false), span };
        let bin = |op, a: &Expr, b: Expr| Expr { kind: ExprKind::Bin(op, Box::new(a.clone()), Box::new(b)), span };
        let sugar = match (name, args) {
            ("radians", [x]) => Some(bin(BinOp::Mul, x, num(std::f64::consts::PI / 180.0))),
            ("degrees", [x]) => Some(bin(BinOp::Mul, x, num(180.0 / std::f64::consts::PI))),
            ("inverseSqrt", [x]) => {
                Some(Expr { kind: ExprKind::Bin(BinOp::Div, Box::new(num(1.0)), Box::new(Expr { kind: ExprKind::Call("sqrt".into(), vec![x.clone()]), span })), span })
            }
            // Float modulo (`fmod`): the sign of `a`.
            ("modf", [a, b]) => Some(bin(BinOp::Rem, a, b.clone())),
            _ => None,
        };
        if let Some(e) = sugar {
            return self.expr(&e);
        }
        let mut vals = Vec::new();
        for a in args {
            vals.push(self.expr(a)?);
        }
        let n = vals.len();
        let want = |k: usize| -> LResult<()> {
            if n != k {
                err(span, format!("`{}` takes {} argument{}", name, k, if k == 1 { "" } else { "s" }))
            } else {
                Ok(())
            }
        };
        if let Some(v) = self.f64_call(name, &vals, args, span)? {
            return Ok(v);
        }
        // Lane-wise unary math on f32 or vec2.
        let unary: Option<fn(&mut Builder, Val) -> Val> = match name {
            "sin" => Some(|b, x| b.sincos(x, false)),
            "cos" => Some(|b, x| b.sincos(x, true)),
            "tan" => Some(|b, x| {
                let s = b.sincos(x, false);
                let c = b.sincos(x, true);
                b.div(s, c)
            }),
            "tanh" | "fast_tanh" => Some(|b, x| b.tanh(x)),
            "atan" => Some(|b, x| b.atan(x)),
            "exp" => Some(|b, x| b.exp(x)),
            "exp2" => Some(|b, x| {
                let y = b.mulc(x, std::f32::consts::LN_2);
                b.exp(y)
            }),
            "log" | "ln" => Some(|b, x| b.log(x)),
            "log2" => Some(|b, x| {
                let l = b.log(x);
                b.mulc(l, std::f32::consts::LOG2_E)
            }),
            "log10" => Some(|b, x| {
                let l = b.log(x);
                b.mulc(l, std::f32::consts::LOG10_E)
            }),
            "sqrt" => Some(|b, x| b.un(Ty::F32, Un::SqrtF, x)),
            "abs" => Some(|b, x| b.fabs(x)),
            "floor" => Some(|b, x| b.un(Ty::F32, Un::FloorF, x)),
            "ceil" => Some(|b, x| b.un(Ty::F32, Un::CeilF, x)),
            "round" => Some(|b, x| b.un(Ty::F32, Un::RoundF, x)),
            "trunc" => Some(|b, x| b.un(Ty::F32, Un::TruncF, x)),
            "fract" => Some(|b, x| {
                let f = b.un(Ty::F32, Un::FloorF, x);
                b.sub(x, f)
            }),
            "sign" => Some(|b, x| {
                let z = b.cf(0.0);
                let one = b.cf(1.0);
                let m1 = b.cf(-1.0);
                let pos = b.cmpf(Cmp::Gt, x, z);
                let neg = b.cmpf(Cmp::Lt, x, z);
                let s = b.sel(neg, m1, z);
                b.sel(pos, one, s)
            }),
            "midi_to_hz" | "mtof" => Some(|b, x| {
                let d = b.addc(x, -69.0);
                let y = b.mulc(d, std::f32::consts::LN_2 / 12.0);
                let e = b.exp(y);
                b.mulc(e, 440.0)
            }),
            "db_to_gain" | "db" => Some(|b, x| {
                let y = b.mulc(x, std::f32::consts::LN_10 / 20.0);
                b.exp(y)
            }),
            _ => None,
        };
        if let Some(f) = unary {
            want(1)?;
            return Ok(match &vals[0] {
                V::Vec(n, l) => {
                    let mut o = *l;
                    for k in 0..*n as usize {
                        o[k] = f(&mut self.b, l[k]);
                    }
                    V::Vec(*n, o)
                }
                v => {
                    let x = self.to_f(v, args[0].span)?;
                    V::F(f(&mut self.b, x))
                }
            });
        }
        let f_args = |l: &mut Self, vals: &[V]| -> LResult<Vec<Val>> {
            vals.iter().zip(args).map(|(v, a)| l.to_f(v, a.span)).collect()
        };
        match name {
            "min" | "max" => {
                want(2)?;
                if matches!((&vals[0], &vals[1]), (V::I(_), V::I(_) | V::Lit(_)) | (V::Lit(_), V::I(_)))
                    || (self.event.integer_literals && vals.iter().all(|v| matches!(v, V::Lit(_))))
                {
                    let x = self.to_i(&vals[0], args[0].span)?;
                    let y = self.to_i(&vals[1], args[1].span)?;
                    if name == "min" && !args.iter().any(float_literal) {
                        *min_bound = [self.b.cst(x), self.b.cst(y)].into_iter().flatten().map(|k| k as i32).min();
                    }
                    let c = self.b.cmpi(if name == "min" { Cmp::Lt } else { Cmp::Gt }, x, y);
                    return Ok(V::I(self.b.sel(c, x, y)));
                }
                let op = if name == "min" { Bin::MinF } else { Bin::MaxF };
                if let Some(n) = self.vec_width(&[&vals[0], &vals[1]], span)? {
                    let la = self.to_vec(&vals[0], n, span)?;
                    let lb = self.to_vec(&vals[1], n, span)?;
                    let mut o = la;
                    for k in 0..n as usize {
                        o[k] = self.b.fb(op, la[k], lb[k]);
                    }
                    return Ok(V::Vec(n, o));
                }
                let v = f_args(self, &vals)?;
                Ok(V::F(self.b.fb(op, v[0], v[1])))
            }
            "clamp" => {
                want(3)?;
                if let V::Vec(n, _) = vals[0] {
                    let l = self.to_vec(&vals[0], n, span)?;
                    let lo = self.to_vec(&vals[1], n, span)?;
                    let hi = self.to_vec(&vals[2], n, span)?;
                    let mut o = l;
                    for k in 0..n as usize {
                        let x = self.b.fb(Bin::MaxF, l[k], lo[k]);
                        o[k] = self.b.fb(Bin::MinF, x, hi[k]);
                    }
                    return Ok(V::Vec(n, o));
                }
                if matches!(vals[0], V::I(_)) || (self.event.integer_literals && matches!(vals[0], V::Lit(_))) {
                    let x = self.to_i(&vals[0], span)?;
                    let lo = self.to_i(&vals[1], span)?;
                    let hi = self.to_i(&vals[2], span)?;
                    let c = self.b.cmpi(Cmp::Lt, x, lo);
                    let x = self.b.sel(c, lo, x);
                    let c = self.b.cmpi(Cmp::Gt, x, hi);
                    return Ok(V::I(self.b.sel(c, hi, x)));
                }
                let v = f_args(self, &vals)?;
                let x = self.b.fb(Bin::MaxF, v[0], v[1]);
                Ok(V::F(self.b.fb(Bin::MinF, x, v[2])))
            }
            "mix" | "lerp" => {
                want(3)?;
                let t = self.to_f(&vals[2], args[2].span)?;
                if let Some(n) = self.vec_width(&[&vals[0], &vals[1]], span)? {
                    let la = self.to_vec(&vals[0], n, span)?;
                    let lb = self.to_vec(&vals[1], n, span)?;
                    let mut o = la;
                    for k in 0..n as usize {
                        let d = self.b.sub(lb[k], la[k]);
                        let d = self.b.mul(d, t);
                        o[k] = self.b.add(la[k], d);
                    }
                    return Ok(V::Vec(n, o));
                }
                let v = f_args(self, &vals)?;
                let d = self.b.sub(v[1], v[0]);
                let d = self.b.mul(d, t);
                Ok(V::F(self.b.add(v[0], d)))
            }
            "pow" => {
                want(2)?;
                let v = f_args(self, &vals)?;
                Ok(V::F(self.b.pow(v[0], v[1])))
            }
            "atan2" => {
                want(2)?;
                let v = f_args(self, &vals)?;
                Ok(V::F(self.b.atan2(v[0], v[1])))
            }
            "step" => {
                want(2)?;
                let v = f_args(self, &vals)?;
                let lt = self.b.cmpf(Cmp::Lt, v[1], v[0]);
                let z = self.b.cf(0.0);
                let o = self.b.cf(1.0);
                Ok(V::F(self.b.sel(lt, z, o)))
            }
            "smoothstep" => {
                want(3)?;
                let v = f_args(self, &vals)?;
                let num = self.b.sub(v[2], v[0]);
                let den = self.b.sub(v[1], v[0]);
                let q = self.b.div(num, den);
                let z = self.b.cf(0.0);
                let o = self.b.cf(1.0);
                let t = self.b.fb(Bin::MaxF, q, z);
                let t = self.b.fb(Bin::MinF, t, o);
                let tt = self.b.mul(t, t);
                let t2 = self.b.mulc(t, 2.0);
                let three = self.b.cf(3.0);
                let inner = self.b.sub(three, t2);
                Ok(V::F(self.b.mul(tt, inner)))
            }
            "vec2" | "vec3" | "vec4" | "mat4" => {
                let w: u8 = match name {
                    "vec2" => 2,
                    "vec3" => 3,
                    "vec4" => 4,
                    _ => 16,
                };
                // One scalar broadcasts (a mat4 of one scalar is s * identity
                // is NOT implied: mat4(s) fills every lane); otherwise the
                // components concatenate (vec4(v3, 1.0), mat4(c0, c1, c2, c3)).
                if n == 1 {
                    if let V::Vec(m, l) = vals[0] {
                        if m == w {
                            return Ok(V::Vec(m, l));
                        }
                    } else {
                        let x = self.to_f(&vals[0], span)?;
                        return Ok(V::Vec(w, [x; 16]));
                    }
                }
                let mut out = Vec::new();
                for (v, a) in vals.iter().zip(args) {
                    match v {
                        V::Vec(m, l) => out.extend_from_slice(&l[..*m as usize]),
                        v => out.push(self.to_f(v, a.span)?),
                    }
                }
                if out.len() != w as usize {
                    return err(span, format!("{} needs {} components, got {}", name, w, out.len()));
                }
                Ok(V::Vec(w, lanes_of(&out)))
            }
            "dot" => {
                want(2)?;
                let n = self.vec_width(&[&vals[0], &vals[1]], span)?.unwrap_or(1);
                if n == 1 {
                    let v = f_args(self, &vals)?;
                    return Ok(V::F(self.b.mul(v[0], v[1])));
                }
                let a = self.to_vec(&vals[0], n, span)?;
                let b = self.to_vec(&vals[1], n, span)?;
                Ok(V::F(self.dot(&a, &b, n)))
            }
            "length" => {
                want(1)?;
                match &vals[0] {
                    V::Vec(n, l) => {
                        let d = self.dot(l, l, *n);
                        Ok(V::F(self.b.un(Ty::F32, Un::SqrtF, d)))
                    }
                    v => {
                        let x = self.to_f(v, span)?;
                        Ok(V::F(self.b.fabs(x)))
                    }
                }
            }
            "distance" => {
                want(2)?;
                let d = self.binary(BinOp::Sub, vals[0].clone(), vals[1].clone(), span)?;
                match &d {
                    V::Vec(n, l) => {
                        let s = self.dot(l, l, *n);
                        Ok(V::F(self.b.un(Ty::F32, Un::SqrtF, s)))
                    }
                    v => {
                        let x = self.to_f(v, span)?;
                        Ok(V::F(self.b.fabs(x)))
                    }
                }
            }
            "normalize" => {
                want(1)?;
                let V::Vec(n, l) = vals[0] else {
                    return err(span, "normalize() takes a vector");
                };
                // A zero vector stays zero.
                let d = self.dot(&l, &l, n);
                let len = self.b.un(Ty::F32, Un::SqrtF, d);
                let zero = self.b.cf(0.0);
                let is_zero = self.b.cmpf(Cmp::Eq, len, zero);
                let one = self.b.cf(1.0);
                let safe = self.b.sel(is_zero, one, len);
                let inv = self.b.div(one, safe);
                let mut o = l;
                for k in 0..n as usize {
                    o[k] = self.b.mul(l[k], inv);
                }
                Ok(V::Vec(n, o))
            }
            "cross" => {
                want(2)?;
                let a = self.to_vec(&vals[0], 3, span)?;
                let b = self.to_vec(&vals[1], 3, span)?;
                let m = |l: &mut Self, i: usize, j: usize| {
                    let p = l.b.mul(a[i], b[j]);
                    let q = l.b.mul(a[j], b[i]);
                    l.b.sub(p, q)
                };
                let x = m(self, 1, 2);
                let y = m(self, 2, 0);
                let z = m(self, 0, 1);
                Ok(V::Vec(3, lanes_of(&[x, y, z])))
            }
            "transpose" => {
                want(1)?;
                let l = self.to_vec(&vals[0], 16, span)?;
                let mut o = l;
                for c in 0..4 {
                    for r in 0..4 {
                        o[c * 4 + r] = l[r * 4 + c];
                    }
                }
                Ok(V::Vec(16, o))
            }
            "int" | "i32" => {
                want(1)?;
                Ok(match &vals[0] {
                    // A typed integer: `var w = int(0)` declares i32 state.
                    v @ V::Lit(_) if self.event.integer_literals => V::I(self.to_i(v, args[0].span)?),
                    V::Lit(x) => V::I(self.b.ci(x.trunc() as i32)),
                    V::I(x) => V::I(*x),
                    V::F(x) => V::I(self.b.un(Ty::I32, Un::F2I, *x)),
                    V::B(x) => {
                        let one = self.b.ci(1);
                        let zero = self.b.ci(0);
                        V::I(self.b.sel(*x, one, zero))
                    }
                    v => return err(span, format!("int() of {}", self.vname(v))),
                })
            }
            "float" | "f32" => {
                want(1)?;
                Ok(match &vals[0] {
                    V::B(x) => {
                        let one = self.b.cf(1.0);
                        let zero = self.b.cf(0.0);
                        V::F(self.b.sel(*x, one, zero))
                    }
                    v => V::F(self.to_f(v, span)?),
                })
            }
            "len" => {
                want(1)?;
                match &vals[0] {
                    V::Place(Place { ty: T::Array(_, n), .. }) => Ok(V::Lit(*n as f64)),
                    V::Place(Place { ty: T::Buf(_, stride, offset, _), region: Region::Buf(k), .. }) => {
                        if self.domain == Domain::Event {
                            return Ok(V::Lit(self.event.buffers[*k as usize].len as f64));
                        }
                        // Whole elements in the host buffer.
                        let (stride, offset, k) = (*stride, *offset, *k);
                        let words = self.b.raw(Ty::I32, Op::BufLen(k), None);
                        let o = self.b.ci(offset as i32);
                        let words = self.b.ib(Bin::SubI, words, o);
                        let st = self.b.ci(stride.max(1) as i32);
                        Ok(V::I(self.b.ib(Bin::DivI, words, st)))
                    }
                    v => err(span, format!("len() of {}", self.vname(v))),
                }
            }
            "read" | "read_cubic" => {
                want(2)?;
                let V::Place(p) = &vals[0] else {
                    return err(args[0].span, "read(array, position) needs an array");
                };
                let stereo = matches!(&p.ty, T::Array(e, _) if **e == T::Vec(2));
                if !stereo && !matches!(&p.ty, T::Array(e, _) if **e == T::F) {
                    return err(args[0].span, "read() needs an array of f32 or vec2");
                }
                let pos = self.to_f(&vals[1], args[1].span)?;
                let fl = self.b.un(Ty::F32, Un::FloorF, pos);
                let frac = self.b.sub(pos, fl);
                let i0 = self.b.un(Ty::I32, Un::F2I, fl);
                let tap = |l: &mut Self, k: i32| -> LResult<[Val; 2]> {
                    let c = l.b.ci(k);
                    let ik = l.b.ib(Bin::AddI, i0, c);
                    let ep = l.index_place(p, &V::I(ik), span)?;
                    Ok(match l.read(&ep) {
                        V::F(x) => [x, x],
                        V::Vec(2, v) => [v[0], v[1]],
                        _ => unreachable!(),
                    })
                };
                let cubic = name == "read_cubic";
                let xm1 = if cubic { Some(tap(self, -1)?) } else { None };
                let x0 = tap(self, 0)?;
                let x1 = tap(self, 1)?;
                let x2 = if cubic { Some(tap(self, 2)?) } else { None };
                let b = &mut self.b;
                let mut lanes = [Val(0); 16];
                for k in 0..if stereo { 2 } else { 1 } {
                    lanes[k] = if let (Some(before), Some(after)) = (xm1, x2) {
                        let d = b.sub(x1[k], before[k]);
                        let c = b.mulc(d, 0.5);
                        let v = b.sub(x0[k], x1[k]);
                        let w = b.add(c, v);
                        let s = b.sub(after[k], x0[k]);
                        let s = b.mulc(s, 0.5);
                        let a = b.add(w, v);
                        let a = b.add(a, s);
                        let bn = b.add(w, a);
                        let t = b.mul(a, frac);
                        let t = b.sub(t, bn);
                        let t = b.mul(t, frac);
                        let t = b.add(t, c);
                        let t = b.mul(t, frac);
                        b.add(t, x0[k])
                    } else {
                        let d = b.sub(x1[k], x0[k]);
                        let d = b.mul(d, frac);
                        b.add(x0[k], d)
                    };
                }
                Ok(if stereo { V::Vec(2, lanes) } else { V::F(lanes[0]) })
            }
            "cc" if self.domain == Domain::Audio => {
                if self.entry.is_none() {
                    return err(span, "`cc()` is an audio input: not available in init() or initial values");
                }
                let index = match vals.as_slice() {
                    [V::Lit(x)] if x.fract() == 0.0 && (0.0..CC_WORDS as f64).contains(x) => Some(*x as u32),
                    [V::I(v)] => self.b.cst(*v).filter(|&n| n < CC_WORDS),
                    _ => None,
                };
                let Some(index) = index else {
                    return err(span, "cc(n) needs a constant integer in 0..127");
                };
                Ok(V::F(self.b.load(Ty::F32, Region::Ctx, CTX_CC + index, 1, None)))
            }
            "rand" | "noise" if self.domain == Domain::Kernel => {
                err(span, "kernels have no running random state: use hash01(i, k) or rand01(seed, i, k) for per-element randomness")
            }
            "rand" | "noise" => {
                want(0)?;
                let Some(ent) = self.entry.clone() else {
                    return err(span, "rand() runs per voice; not available in init() (use a fixed formula)");
                };
                let b = &mut self.b;
                let x = b.get(ent.rng);
                let c13 = b.ci(13);
                let t = b.ib(Bin::ShlI, x, c13);
                let x = b.ib(Bin::XorI, x, t);
                let c17 = b.ci(17);
                let t = b.ib(Bin::ShrUI, x, c17);
                let x = b.ib(Bin::XorI, x, t);
                let c5 = b.ci(5);
                let t = b.ib(Bin::ShlI, x, c5);
                let x = b.ib(Bin::XorI, x, t);
                b.set(ent.rng, x);
                let c9 = b.ci(9);
                let m = b.ib(Bin::ShrUI, x, c9);
                let two = b.ci(0x4000_0000);
                let m = b.ib(Bin::OrI, m, two);
                let f = b.un(Ty::F32, Un::BitsIF, m);
                Ok(V::F(b.addc(f, -3.0)))
            }
            "stop" => {
                want(0)?;
                let Some(ent) = self.entry.clone() else {
                    return err(span, "stop() is for voices");
                };
                let one = self.b.ci(1);
                self.b.set(ent.stop, one);
                Ok(V::Unit)
            }
            "param" => err(span, "param(...) is declared at the top level: `let cutoff = param(1200, 20, 20000)`"),
            _ if self.domain == Domain::Kernel && crate::host::find(name).is_some() => self.kernel_host_call(name, &vals, args, span),
            _ => {
                let hint = self.suggest(name, true);
                err(span, format!("unknown function `{}`{}", name, hint))
            }
        }
    }
}

fn check_init_cost(prog: &Program, span: Span) -> LResult<()> {
    let cost = prog.total_cost();
    if cost > MAX_INIT_COST {
        return err(span, format!("init() may run {cost} operations; the limit is {MAX_INIT_COST}"));
    }
    Ok(())
}

fn ann_width(a: &TypeAnn) -> u8 {
    match a {
        TypeAnn::Vec2 => 2,
        TypeAnn::Vec3 => 3,
        TypeAnn::Vec4 => 4,
        _ => 16,
    }
}

/// One component of a vector (`x`, `r`, `0`, …).
fn single_lane(n: u8, field: &str) -> Option<usize> {
    match field {
        "0" => Some(0),
        "1" => Some(1),
        "2" if n >= 3 && n <= 4 => Some(2),
        "3" if n == 4 => Some(3),
        f if f.len() == 1 => lane_of(n, f.chars().next().unwrap()),
        _ => None,
    }
}

/// Does a loop body contain a `break`/`continue` that targets this loop
/// (not one of its nested loops)?
fn has_own_break(body: &[Stmt]) -> bool {
    fn expr(e: &Expr) -> bool {
        match &e.kind {
            ExprKind::If(arms, else_) => {
                arms.iter().any(|(c, b)| expr(c) || stmts(b)) || else_.as_ref().is_some_and(|b| stmts(b))
            }
            ExprKind::Match(s, arms) => expr(s) || arms.iter().any(|(_, b)| stmts(b)),
            ExprKind::Block(b) => stmts(b),
            ExprKind::Bin(_, a, b) => expr(a) || expr(b),
            ExprKind::Neg(a) | ExprKind::Not(a) => expr(a),
            ExprKind::Call(_, args) => args.iter().any(expr),
            _ => false,
        }
    }
    fn stmts(b: &[Stmt]) -> bool {
        b.iter().any(|s| match s {
            Stmt::Break(_) | Stmt::Continue(_) => true,
            Stmt::Expr(e) => expr(e),
            Stmt::Let { value, .. } => expr(value),
            Stmt::Assign { value, .. } => expr(value),
            Stmt::Return(Some(e), _) => expr(e),
            _ => false,
        })
    }
    stmts(body)
}

/// Syntactic reads of a parameter, including captures and either arm of
/// a branch. A local with the same name shadows it after its initializer.
fn reads_parameter(body: &[Stmt], name: &str) -> bool {
    fn expr(e: &Expr, name: &str) -> bool {
        let reads = |x: &Expr| expr(x, name);
        match &e.kind {
            ExprKind::Ident(n) => n == name,
            ExprKind::Call(n, args) => n.split_once('.').is_some_and(|(root, _)| root == name) || args.iter().any(reads),
            ExprKind::Neg(a) | ExprKind::Not(a) | ExprKind::Field(a, _) => reads(a),
            ExprKind::Bin(_, a, b) | ExprKind::Index(a, b) | ExprKind::ArrayRepeat(a, b) => reads(a) || reads(b),
            ExprKind::ArrayList(xs) => xs.iter().any(reads),
            ExprKind::StructLit(_, fields) => fields.iter().any(|(_, x)| reads(x)),
            ExprKind::Block(b) => reads_parameter(b, name),
            ExprKind::Lambda(f) => !f.params.iter().any(|(p, _)| p == name) && reads_parameter(&f.body, name),
            ExprKind::If(arms, other) => arms.iter().any(|(c, b)| reads(c) || reads_parameter(b, name))
                || other.as_ref().is_some_and(|b| reads_parameter(b, name)),
            ExprKind::Match(x, arms) => reads(x) || arms.iter().any(|(patterns, b)| {
                patterns.iter().flatten().any(reads) || reads_parameter(b, name)
            }),
            ExprKind::Num(..) | ExprKind::Bool(_) | ExprKind::Str(_) | ExprKind::Unreadable(_) => false,
        }
    }
    // A plain assignment reads its indices. A compound assignment also
    // reads the old value at the target.
    fn index_reads(target: &Expr, name: &str) -> bool {
        match &target.kind {
            ExprKind::Index(a, i) => expr(i, name) || index_reads(a, name),
            ExprKind::Field(a, _) => index_reads(a, name),
            ExprKind::Ident(_) => false,
            _ => expr(target, name),
        }
    }
    let reads = |x: &Expr| expr(x, name);
    for s in body {
        let used = match s {
            Stmt::Let { name: local, value, .. } if local == name => return reads(value),
            Stmt::Let { value, .. } | Stmt::Expr(value) | Stmt::Return(Some(value), _) => reads(value),
            Stmt::Assign { target, op, value, .. } => reads(value) || if *op == AssignOp::Set { index_reads(target, name) } else { reads(target) },
            Stmt::For { var, from, to, body, .. } => reads(from) || reads(to) || (var != name && reads_parameter(body, name)),
            Stmt::While { cond, body, .. } => reads(cond) || reads_parameter(body, name),
            Stmt::Loop { body, .. } => reads_parameter(body, name),
            Stmt::Return(None, _) | Stmt::Break(_) | Stmt::Continue(_) => false,
        };
        if used { return true }
    }
    false
}

/// Is `name` assigned (as a whole or by component) anywhere in `body`?
fn assigns(body: &[Stmt], name: &str) -> bool {
    fn root(e: &Expr) -> Option<&str> {
        match &e.kind {
            ExprKind::Ident(n) => Some(n),
            ExprKind::Field(b, _) | ExprKind::Index(b, _) => root(b),
            _ => None,
        }
    }
    body.iter().any(|s| match s {
        Stmt::Assign { target, value, .. } => root(target) == Some(name) || assigns_expr(value, name),
        Stmt::Let { value, .. } => assigns_expr(value, name),
        Stmt::Expr(e) => assigns_expr(e, name),
        Stmt::For { from, to, body, .. } => assigns_expr(from, name) || assigns_expr(to, name) || assigns(body, name),
        Stmt::While { cond, body, .. } => assigns_expr(cond, name) || assigns(body, name),
        Stmt::Loop { body, .. } => assigns(body, name),
        Stmt::Return(Some(e), _) => assigns_expr(e, name),
        _ => false,
    })
}

/// Is `name` assigned anywhere in `e`?
fn assigns_expr(e: &Expr, name: &str) -> bool {
    match &e.kind {
        ExprKind::If(arms, else_) => {
            arms.iter().any(|(c, b)| assigns_expr(c, name) || assigns(b, name)) || else_.as_ref().is_some_and(|b| assigns(b, name))
        }
        ExprKind::Match(s, arms) => assigns_expr(s, name) || arms.iter().any(|(_, b)| assigns(b, name)),
        ExprKind::Block(b) => assigns(b, name),
        ExprKind::Bin(_, a, b) | ExprKind::Index(a, b) | ExprKind::ArrayRepeat(a, b) => assigns_expr(a, name) || assigns_expr(b, name),
        ExprKind::Neg(a) | ExprKind::Not(a) | ExprKind::Field(a, _) => assigns_expr(a, name),
        ExprKind::Call(_, args) | ExprKind::ArrayList(args) => args.iter().any(|a| assigns_expr(a, name)),
        ExprKind::StructLit(_, f) => f.iter().any(|(_, a)| assigns_expr(a, name)),
        // A local function assigning a captured name assigns it.
        ExprKind::Lambda(f) => assigns(&f.body, name),
        _ => false,
    }
}

/// Is there a function (`fn(..) {..}`, a local `fn`) anywhere in `body`?
fn lambda_in(body: &[Stmt]) -> bool {
    fn expr(e: &Expr) -> bool {
        match &e.kind {
            ExprKind::Lambda(_) => true,
            ExprKind::If(arms, else_) => arms.iter().any(|(c, b)| expr(c) || lambda_in(b)) || else_.as_ref().is_some_and(|b| lambda_in(b)),
            ExprKind::Match(s, arms) => expr(s) || arms.iter().any(|(_, b)| lambda_in(b)),
            ExprKind::Block(b) => lambda_in(b),
            ExprKind::Bin(_, a, b) | ExprKind::Index(a, b) | ExprKind::ArrayRepeat(a, b) => expr(a) || expr(b),
            ExprKind::Neg(a) | ExprKind::Not(a) | ExprKind::Field(a, _) => expr(a),
            ExprKind::Call(_, args) | ExprKind::ArrayList(args) => args.iter().any(expr),
            ExprKind::StructLit(_, f) => f.iter().any(|(_, a)| expr(a)),
            _ => false,
        }
    }
    body.iter().any(|s| match s {
        Stmt::Let { value, .. } | Stmt::Expr(value) | Stmt::Return(Some(value), _) => expr(value),
        Stmt::Assign { target, value, .. } => expr(target) || expr(value),
        Stmt::For { from, to, body, .. } => expr(from) || expr(to) || lambda_in(body),
        Stmt::While { cond, body, .. } => expr(cond) || lambda_in(body),
        Stmt::Loop { body, .. } => lambda_in(body),
        _ => false,
    })
}

/// Is `name` assigned in `rest` (the statements after its `let`, in its block) only by whole
/// assignments standing in `rest` itself: none in a branch, a loop or a function, and no function
/// there to close over it? Then each assignment can name its value in turn.
fn assigned_in_line(rest: &[Stmt], name: &str) -> bool {
    !lambda_in(rest)
        && rest.iter().all(|s| match s {
            Stmt::Assign { target: Expr { kind: ExprKind::Ident(n), .. }, value, .. } if n == name => !assigns_expr(value, name),
            s => !assigns(std::slice::from_ref(s), name),
        })
}

fn edit_distance(a: &str, b: &str) -> usize {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for i in 1..=a.len() {
        let mut prev = row[0];
        row[0] = i;
        for j in 1..=b.len() {
            let cur = row[j];
            row[j] = (row[j] + 1).min(row[j - 1] + 1).min(prev + (a[i - 1] != b[j - 1]) as usize);
            prev = cur;
        }
    }
    row[b.len()]
}

const BUILTIN_FNS: &[&str] = &[
    "sin", "cos", "tan", "tanh", "atan", "atan2", "asin", "acos", "hypot", "cbrt", "f64", "exp", "exp2", "log", "ln", "log2", "log10", "sqrt", "abs", "floor", "ceil", "round", "trunc", "fract",
    "sign", "midi_to_hz", "db_to_gain", "min", "max", "clamp", "mix", "pow", "step", "smoothstep", "vec2", "int", "float", "len",
    "read", "read_cubic", "rand", "stop", "cc",
];

const INPUT_NAMES: &[&str] = &[
    "tempo", "beat", "bar", "beats_per_bar", "playing", "voice_index", "instance",
    "sample_rate", "SR", "note", "freq", "gate", "velocity", "trigger", "bend", "pressure", "slide", "release_velocity", "mod_wheel",
];
pub(crate) const BUILTIN_NAMES: &[&str] = &[
    "sample_rate", "SR", "note", "freq", "gate", "velocity", "trigger", "PI", "TAU", "E", "voice", "effect", "block", "init",
];

/// Lowers a parsed shader into its render (and optional init) programs.
fn new_lowerer<'control>(prelude_base: usize, domain: Domain) -> Lowerer<'control> {
    Lowerer {
        control: None,
        block_layout: None,
        fft_sites: Vec::new(),
        render_entries: Vec::new(),
        ports: Vec::new(),
        audio_channels: [2, 2],
        port_frame: None,
        mono: None,
        latency: 0,
        oversampling: 1,
        rate_modes: RateModes::default(),
        rate_reads_input: false,
        rate_nests: RateNests::default(),
        reads_note: false,
        reads_freq: false,
        b: Builder::new(),
        structs: Vec::new(),
        fns: HashMap::new(),
        globals: HashMap::new(),
        frames: vec![vec![HashMap::new()]],
        loops: Vec::new(),
        rets: Vec::new(),
        call_stack: Vec::new(),
        params: Vec::new(),
        probes: Vec::new(),
        param_meta: Vec::new(),
        state_init: Vec::new(),
        state_vars: Vec::new(),
        promoted: Vec::new(),
        shared_init: Vec::new(),
        sample_names: Vec::new(),
        frame_words: 0,
        entry: None,
        in_init: false,
        prelude_base,
        marks: false,
        marks_spans: Vec::new(),
        cond_marks: Vec::new(),
        domain,
        kernel: Default::default(),
        portable: false,
        whole_ranges: false,
        funcs_out: Vec::new(),
        func_cache: HashMap::new(),
        call_sites: None,
        object_tables: HashMap::new(),
        lenient_objects: false,
        live: None,
        trace: None,
        event: Default::default(),
        call_spans: Vec::new(),
        lit_safe: HashMap::new(),
        taint: None,
    }
}

// The lexer cannot produce this state name. Its type signature includes the factor.
const RATE_STATE: &str = "$rate";

fn halfband_down_latency(factor: u32) -> u32 {
    match factor { 2 => 17, 4 => 19, 8 => 20, _ => unreachable!() }
}

#[relax_api::relax_api(plain)]
#[repr(u8)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RateMode { #[default] Filter, Linear, Hold }

impl std::str::FromStr for RateMode {
    type Err = &'static str;

    fn from_str(name: &str) -> Result<Self, Self::Err> {
        match name {
            "filter" => Ok(Self::Filter),
            "linear" => Ok(Self::Linear),
            "hold" => Ok(Self::Hold),
            _ => Err("a conversion mode must be filter, linear or hold"),
        }
    }
}

impl RateMode {
    fn parse(e: &Expr) -> LResult<Self> {
        if let ExprKind::Ident(name) = &e.kind {
            if let Ok(mode) = name.parse() { return Ok(mode); }
        }
        err(e.span, "a conversion mode must be filter, linear or hold")
    }
}

/// Whole-shader rate selected by the host. A zero factor follows the declaration.
#[relax_api::relax_api(plain)]
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RateOverride {
    pub factor: u32,
    pub up: RateMode,
    pub down: RateMode,
}

#[derive(Clone, Copy, Default)]
struct RateModes { up: RateMode, down: RateMode }

impl RateModes {
    fn parse(args: &[Expr], span: Span) -> LResult<Self> {
        match args {
            [] => Ok(Self::default()),
            [both] => {
                let mode = RateMode::parse(both)?;
                Ok(Self { up: mode, down: mode })
            }
            [up, down] => Ok(Self { up: RateMode::parse(up)?, down: RateMode::parse(down)? }),
            _ => err(span, "expected at most two conversion modes (filter, linear or hold)"),
        }
    }

    fn latency(self, factor: u32, has_input: bool) -> u32 {
        RatePath::base(1).extend(factor, self, has_input).latency()
    }
}

#[derive(Clone, Copy)]
pub struct AudioRate {
    pub factor: u32,
    pub span: Span,
    modes: RateModes,
}

/// Resolves the audio rate before sizing any tables or per-instance state.
pub fn audio_rate<'a>(items: &[Item], prelude: &[Item], sample: &dyn Fn(&str) -> Option<(u32, u32, &'a [f32])>, control: Option<&AtomicU32>)
    -> Result<Option<AudioRate>, ShaderError> {
    let mut decls = items.iter().filter(|i| matches!(i, Item::Decl { name, .. } if name == "rate"));
    let Some(Item::Decl { args, span, .. }) = decls.next() else { return Ok(None) };
    if let Some(Item::Decl { span, .. }) = decls.next() { return err(*span, "`rate()` declared twice") }
    let span = *span;
    let [value, modes @ ..] = args.as_slice() else { return err(span, "`rate()` needs at least one argument (the factor)") };
    if modes.len() == 2 && items.iter().any(|i| matches!(i, Item::Fn(f) if f.name == "voice")) {
        return err(span, "an instrument converts down only; write rate(k, mode)");
    }
    let modes = RateModes::parse(modes, span)?;
    let factor = match value.kind {
        ExprKind::Num(n, _) => Some(n),
        _ => {
            // Use ordinary constant evaluation, with only the definitions
            // the factor reaches. Runtime bindings have no value here.
            let mut defs: Vec<_> = prelude.iter().chain(items)
                .filter(|i| !matches!(i, Item::Decl { .. })).cloned().collect();
            const FACTOR: &str = "$rate";
            defs.push(Item::Let { name: FACTOR.into(), ann: None, value: value.clone(), span });
            let mut deps = crate::module::prune(defs, &[FACTOR.to_owned()].into_iter().collect());
            deps.retain(|i| match i {
                Item::Var { .. } => false,
                Item::Let { value, .. } => !matches!(&value.kind, ExprKind::Call(f, _) if f == "param"),
                _ => true,
            });
            let mut constants = new_lowerer(usize::MAX, Domain::Audio);
            constants.control = control;
            constants.oversampling = 0;
            speculative(constants.top_with_samples(&deps, sample, Some(AudioHeader::new(&deps).items)))?.and_then(|_| match constants.globals.get(FACTOR) {
                Some(Bind::Const(V::Lit(n))) => Some(*n),
                _ => None,
            })
        }
    };
    match factor {
        Some(n) if [1.0, 2.0, 4.0, 8.0].contains(&n) => Ok(Some(AudioRate { factor: n as u32, span, modes })),
        _ => err(span, "`rate()` needs a constant factor of 1, 2, 4 or 8"),
    }
}

/// Audio interface as written. None of these declarations requires a folded value.
#[derive(Debug)]
pub struct AudioInventory {
    pub kind: Kind,
    pub mono: bool,
    pub params: Vec<(String, Vec<String>)>,
    pub samples: Vec<String>,
    pub ports: Vec<(String, bool, u16)>,
}

enum AudioDecl {
    Port { output: bool, width: LResult<u16> },
    Param { choices: LResult<Vec<String>> },
    Sample,
    Mono,
}

fn param_parts(args: &[Expr]) -> (&[Expr], &[(String, Expr)]) {
    match args.split_last() {
        Some((Expr { kind: ExprKind::StructLit(ty, fields), .. }, rest)) if ty.is_empty() => (rest, fields),
        _ => (args, &[]),
    }
}

fn param_word(field: &str, expr: &Expr) -> LResult<String> {
    match &expr.kind {
        ExprKind::Str(s) | ExprKind::Ident(s) => Ok(s.clone()),
        _ => err(expr.span, format!("`{field}` takes a word or a string")),
    }
}

fn param_choices(args: &[Expr]) -> LResult<Vec<String>> {
    let (numeric, fields) = param_parts(args);
    let Some((_, value)) = fields.iter().find(|(key, _)| key == "choices") else { return Ok(Vec::new()) };
    if numeric.len() != 1 {
        return err(value.span, "choices sets the index range and cannot be given with min and max");
    }
    let ExprKind::ArrayList(list) = &value.kind else {
        return err(value.span, "choices needs a list of at least two names");
    };
    if list.len() < 2 {
        return err(value.span, "choices needs at least two names");
    }
    list.iter().map(|e| param_word("choices", e)).collect()
}

// Keep declaration errors at their item. A preceding lowering error still wins.
fn audio_decl(item: &Item) -> Option<AudioDecl> {
    match item {
        Item::Decl { name, .. } if name == "mono" => Some(AudioDecl::Mono),
        Item::Let { name, value, span, .. } => {
            let ExprKind::Call(call, args) = &value.kind else { return None };
            match call.as_str() {
                "input" | "output" => {
                    let width = match args.as_slice() {
                        [Expr { kind: ExprKind::Num(n, _), .. }] if *n == 1.0 || *n == 2.0 => Ok(*n as u16),
                        _ => err(*span, format!("audio port `{name}` needs a literal channel count of 1 or 2")),
                    };
                    Some(AudioDecl::Port { output: call == "output", width })
                }
                "sample" => Some(AudioDecl::Sample),
                "param" => Some(AudioDecl::Param { choices: param_choices(args) }),
                _ => None,
            }
        }
        _ => None,
    }
}

struct AudioHeader {
    kind: LResult<Kind>,
    items: Vec<Option<AudioDecl>>,
}

impl AudioHeader {
    fn new(items: &[Item]) -> Self {
        Self { kind: audio_entry_kind(items), items: items.iter().map(audio_decl).collect() }
    }
}

pub fn audio_inventory(items: &[Item]) -> LResult<AudioInventory> {
    let header = AudioHeader::new(items);
    let mut params = Vec::new();
    let mut samples = Vec::new();
    let mut ports = Vec::new();
    let mut mono = false;
    for (item, declaration) in items.iter().zip(header.items) {
        match declaration {
            Some(AudioDecl::Param { choices }) => params.push((item.name().to_owned(), choices?)),
            Some(AudioDecl::Sample) => samples.push(item.name().to_owned()),
            Some(AudioDecl::Port { output, width }) => ports.push((item.name().to_owned(), output, width?)),
            Some(AudioDecl::Mono) => mono = true,
            None => {}
        }
    }
    Ok(AudioInventory { kind: header.kind?, mono, params, samples, ports })
}

/// Classify the parsed entry declarations before generating DSP code. A block
/// with an input declaration is diagnosed at that port during lowering.
pub fn audio_entry_kind(items: &[Item]) -> Result<Kind, ShaderError> {
    let mut voice = None;
    let mut effect_block = None;
    let (mut effect, mut block, mut ports) = (false, false, false);
    for item in items {
        match item {
            Item::Fn(f) => match f.name.as_str() {
                "voice" => voice = Some(f.span),
                "effect" => effect = true,
                "effect_block" => effect_block = Some(f.span),
                "block" => block = true,
                _ => {}
            },
            Item::Let { value, .. } if matches!(&value.kind, ExprKind::Call(name, _) if name == "input" || name == "output") => ports = true,
            _ => {}
        }
    }
    if let Some(span) = effect_block {
        if voice.is_some() {
            return err(span, "effect_block() with voice() would allocate rings per voice; put effect_block() in a separate effect");
        }
        if effect {
            return err(span, "effect() and effect_block() both process the stereo input; choose one entry");
        }
        return Ok(Kind::Effect);
    }
    match (voice, effect) {
        (Some(_), false) => Ok(Kind::Instrument),
        (None, true) => Ok(Kind::Effect),
        (Some(span), true) => err(span, "a shader is an instrument (`fn voice()`) or an effect (`fn effect(l, r)`), not both"),
        (None, false) if block && ports => Ok(Kind::Control),
        _ => err(Span { start: 0, end: 1 }, "missing entry: add `fn voice() { … }` (instrument), `fn effect(l, r) { … }` or `fn effect_block(l, r) { … }` (effect), or `fn block() { … }` with an output (control)"),
    }
}

pub fn lower(items: &[Item], prelude_base: usize) -> Result<Lowered, ShaderError> {
    lower_with_samples(items, prelude_base, &|_| None)
}

pub fn lower_with_samples<'a>(items: &[Item], prelude_base: usize, sample: &dyn Fn(&str) -> Option<(u32, u32, &'a [f32])>) -> Result<Lowered, ShaderError> {
    lower_with_rate(items, prelude_base, sample, RateOverride::default(), None)
}

pub fn lower_with_rate<'a>(items: &[Item], prelude_base: usize, sample: &dyn Fn(&str) -> Option<(u32, u32, &'a [f32])>, node: RateOverride, control: Option<&AtomicU32>) -> Result<Lowered, ShaderError> {
    lower_audio(items, prelude_base, sample, node, Vec::new(), control)
}

fn lower_audio<'a>(items: &[Item], prelude_base: usize, sample: &dyn Fn(&str) -> Option<(u32, u32, &'a [f32])>,
    node: RateOverride, branches: Vec<bool>, control: Option<&AtomicU32>) -> Result<Lowered, ShaderError> {
    let mut l = new_lowerer(prelude_base, Domain::Audio);
    l.control = control;
    let block_entry = items.iter().find_map(|item| match item {
        Item::Fn(f) if f.name == "effect_block" => Some(f),
        _ => None,
    });
    if let Some(entry) = block_entry {
        if entry.params.len() != 2 {
            return err(entry.span, "write fn effect_block(l, r) with two frame arrays to edit in place");
        }
        block::check_declarations(items, node, entry.span)?;
    }
    let declared = audio_rate(items, &[], sample, control)?;
    let span = declared.map_or(Span { start: 0, end: 1 }, |r| r.span);
    let rate = match node.factor {
        0 => declared,
        1 | 2 | 4 | 8 => Some(AudioRate { factor: node.factor, modes: RateModes { up: node.up, down: node.down }, span }),
        _ => return err(span, "the node's oversample factor must be 1, 2, 4 or 8"),
    };
    if let Some(rate) = rate {
        l.oversampling = rate.factor;
        l.rate_modes = rate.modes;
    }
    let AudioHeader { kind, items: declarations } = AudioHeader::new(items);
    l.top_with_samples(items, sample, Some(declarations))?;
    let kind = kind?;
    if kind == Kind::Control {
        if let Some(port) = l.ports.iter().find(|p| !p.info.output) {
            return err(port.span, format!("control node input `{}`: input() needs a per-frame effect or voice; block() has no audio input", port.info.name));
        }
        if let Some(span) = l.mono {
            return err(span, "mono() is for instrument voices; a control node has no voices");
        }
        for port in &mut l.ports { port.info.control = true; }
    }
    if let Some(AudioRate { factor: 2 | 4 | 8, span, .. }) = rate { l.add_rate_cascade(kind, span)?; }
    // Param smoothing words sit after the header, before user state.
    if !l.params.is_empty() {
        let shift = image_words(2 * l.params.len() as u64, span)?;
        let header = HEADER_WORDS as usize;
        let end = l.state_init.len();
        reserve_words(&mut l.state_init, shift, span)?;
        l.state_init.resize(end + shift as usize, 0);
        l.state_init.copy_within(header..end, header + shift as usize);
        for (pair, p) in l.state_init[header..header + shift as usize].chunks_exact_mut(2).zip(&l.params) {
            pair.copy_from_slice(&[p.default.to_bits(), 0]);
        }
        for sv in &mut l.state_vars {
            sv.offset += shift;
        }
        for (word, _) in &mut l.promoted {
            *word += shift;
        }
        for bind in l.globals.values_mut() {
            if let Bind::Place(p) = bind {
                if p.region == Region::State {
                    p.root += shift;
                }
            }
        }
    }
    if let Some(entry) = block_entry {
        l.block_layout = Some(l.add_block_layout(entry.span)?);
    }
    let mut init = l.fns.get("init").cloned().map(|f| l.init_program(&f)).transpose()?;
    l.rate_nests.branches = branches;
    let render_span = l.render(kind)?;
    // Only a nest needs another emission: the outer up filter precedes the
    // call that tells us where its pad belongs. Use the actual inlined call
    // tree, including local functions, state arguments and node overrides.
    if l.rate_nests.branches.is_empty() && l.rate_nests.observed.iter().any(|&branch| branch) {
        let branches = std::mem::take(&mut l.rate_nests.observed);
        drop(l);
        return lower_audio(items, prelude_base, sample, node, branches, control);
    }
    let mut render = l.b.prog;
    [render.input_channels, render.output_channels] = l.audio_channels;
    if kind == Kind::Instrument {
        if let Some(port) = l.ports.iter().find(|port| port.info.control) {
            return err(port.span, format!("control output port `{}` has no single value across instrument voices", port.info.name));
        }
    }
    render.body = l.b.blocks.into_iter().next().unwrap();
    render.frame_words = l.frame_words;
    let mut warnings = Vec::new();
    if l.fft_sites.is_empty() {
        dce(&mut render);
    } else if let Some(warning) = fft::finish(&l.fft_sites, &mut l.shared_init, &mut init, &mut render,
        l.fns.get("init").map(|f| f.span))? {
        warnings.push(warning);
    }
    if warnings.is_empty() {
        let cost = render.cost();
        if cost > WARN_COST_PER_FRAME {
            let span = l.render_entries.iter().max_by_key(|(cost, _)| cost).map_or(render_span, |e| e.1);
            warnings.push(ShaderWarning { span, message: format!("too much work per sample (worst case {cost} ops); reduce loop sizes") });
        }
    }
    // Price the source and any generated FFT loops before adding host control.
    if let (Some(program), Some(_)) = (&mut init, control) {
        program.poll_cancel_at_loops();
    }
    Ok(Lowered {
        warnings,
        ports: l.ports.into_iter().map(|p| p.info).collect(),
        mono: l.mono.is_some(),
        latency: l.latency,
        kind,
        render,
        init,
        params: l.params,
        probes: l.probes.into_iter().map(|(name, _)| name).collect(),
        param_meta: l.param_meta,
        state_init: l.state_init,
        state_vars: l.state_vars,
        shared_init: l.shared_init,
        sample_names: l.sample_names,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_declarations_are_available_without_evaluating_initializers() {
        let source = "mono() let wave = param(0, {choices: [sine saw]}) let gain = param(1)
            let recording = sample(2) let side = input(2) let tap = output(1)
            fn voice() { undefined_body_value }";
        let parse = |code: &str| crate::parse::Parser::new(&crate::parse::lex(code).unwrap()).items().unwrap();
        let items = parse(source);
        let header = audio_inventory(&items).unwrap();
        assert_eq!(header.kind, Kind::Instrument);
        assert!(header.mono);
        assert_eq!(header.params, [("wave".into(), vec!["sine".into(), "saw".into()]), ("gain".into(), vec![])]);
        assert_eq!(header.samples, ["recording"]);
        assert_eq!(header.ports, [("side".into(), false, 2), ("tap".into(), true, 1)]);

        let items = parse(&source.replace("input(2)", "input(3)"));
        let inventory = audio_inventory(&items).unwrap_err();
        let lowered = lower(&items, usize::MAX).err().expect("invalid port width");
        assert_eq!((inventory.start, inventory.end, inventory.message), (lowered.start, lowered.end, lowered.message));
    }

    /// A store drops every value-numbered load of a word it may write, also
    /// for ranges that end past u32::MAX: summed in u32 such a range wrapped
    /// to a low end, read as a miss, and a later load reused the value from
    /// before the store.
    #[test]
    fn a_store_drops_the_loads_it_may_write_at_the_top_of_the_range() {
        let top = u32::MAX;
        let mut b = Builder::new();
        let x = b.ci(7);
        let i = b.ci(0);
        // A load whose range runs past the top, a store inside it.
        let wide = Key::Load(Region::Frame, top - 4, 8, Some(i), Ty::I32);
        // A store whose range runs past the top, a load inside it.
        let word = Key::Load(Region::Frame, top - 1, 1, None, Ty::I32);
        // A load of words the store misses keeps its value.
        let below = Key::Load(Region::Frame, 0, 4, Some(i), Ty::I32);
        for key in [wide, word, below] {
            b.cse[0].0.insert(key, x);
        }
        b.push(IS::Store { region: Region::Frame, base: top - 2, extent: 1, off: None, val: x });
        assert!(b.lookup(&wide).is_none(), "a store to word top-2 drops a load of top-4..top+4");
        assert!(b.lookup(&word).is_some(), "a store to word top-2 keeps a load of word top-1");
        b.push(IS::Store { region: Region::Frame, base: top - 2, extent: 8, off: Some(i), val: x });
        assert!(b.lookup(&word).is_none(), "a store to top-2..top+6 drops a load of word top-1");
        assert_eq!(b.lookup(&below), Some(x), "a store to top-2..top+6 keeps a load of words 0..4");
    }
}

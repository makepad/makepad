//! Math AOT: compiles a pure-math splash function's EXISTING bytecode into
//! fast batch-evaluable form — the intended use is implicit-surface (SDF)
//! sampling, where one expression is evaluated over millions of points.
//!
//! Architecture (see platform/script/MATH_AOT.md, the plan of record):
//!
//! ```text
//! splash bytecode --(subset detector / translator)--> VIR --(backend)--> CompiledMath
//! ```
//!
//! - [`vir`] defines VIR, the small typed branchless vector IR, plus its
//!   reference evaluator.
//! - [`MathBackend`] / [`CompiledMath`] is the backend seam. Today:
//!   [`StitchBackend`] (threaded-code wasm with stitch's spec-SIMD subset
//!   and nonstandard float-math opcodes — the cross-platform bit-reference
//!   backend) and [`VirInterpBackend`] (the direct VIR evaluator, the
//!   semantic reference). Codegen backends (ARM64/NEON, x86-64/SSE) plug
//!   in behind the same trait as follow-on work.
//!
//! # What the subset detector accepts
//!
//! - Scalar f64 arithmetic (`+ - * / %`), comparisons, `==`/`!=`,
//!   `&&`/`||`, `if`/`else` expressions, top-level early `return`.
//! - `vec2`/`vec3`/`vec4` values: constructors from scalars, lane-wise
//!   arithmetic (including scalar broadcast), swizzles.
//! - `let`-bound locals in slot-eligible bodies, function parameters.
//! - Calls to the `math` module builtins (resolved AT COMPILE TIME through
//!   the function's captured scope chain and matched by object identity):
//!   sin cos tan asin acos atan atan2 exp log sqrt abs floor ceil fract
//!   modf pow min max clamp mix lerp step smoothstep length distance dot
//!   normalize cross inverseSqrt radians degrees — plus the pod methods
//!   `.length()`, `.normalize()`/`.normalized()`, `.dot()`, `.cross()`,
//!   `.mix()`.
//! - Free identifiers that resolve to NUMBERS in the captured scope are
//!   baked in as constants (a compile-time snapshot: mutate the scope and
//!   you must recompile).
//!
//! Anything else — objects, arrays, strings, closures, loops, `use`,
//! dynamic `let`, matrices, unknown natives, type mixes the interpreter
//! would send down a path not mirrored here — makes compilation return
//! `None` and the ordinary splash interpreter remains the (only)
//! semantics. The AOT is an accelerator, never a fork: for everything it
//! accepts, StitchBackend results are BIT-IDENTICAL to the interpreter
//! (which mixes f64 scalar arithmetic with f32 intrinsics and f32 vector
//! lanes); every translation rule below mirrors the corresponding
//! interpreter path operation for operation (opcodes_ops.rs, numeric.rs,
//! shader_builtins.rs).
//!
//! Control flow is lowered to VIR selects: both sides of an `if`/`&&`/`||`
//! are evaluated (every VIR op is total and side-effect free, so this is
//! unobservable) and `let` slots merge through selects, SSA-style.

pub mod stitch_backend;
pub mod vir;

use crate::function::ScriptFnPtr;
use crate::makepad_live_id::live_id::*;
use crate::makepad_live_id_macros::*;
use crate::opcode::{Opcode, OpcodeArgs};
use crate::value::*;
use crate::vm::ScriptVm;
use std::collections::HashMap;
use vir::{CmpCc, MathFn, MathFn2, VirFn, VirOp, VirReg, VirTy};

// =========================================================================
// The backend seam (MATH_AOT.md)
// =========================================================================

/// Returned by a backend that cannot compile a given [`VirFn`] (missing
/// host features, unsupported op). The caller falls back to the next
/// backend in line, ultimately the splash interpreter.
#[derive(Debug, Clone, Copy)]
pub struct BackendUnsupported;

pub trait MathBackend {
    fn compile(&self, f: &VirFn) -> Result<Box<dyn CompiledMath>, BackendUnsupported>;
}

/// A compiled pure-math function.
pub trait CompiledMath: Send + Sync {
    /// Batch evaluation: `input` holds the function's flattened f32 input
    /// lanes per point (per-point parameters in declaration order),
    /// `uniforms` the read-only uniform block (flattened f32 lanes of the
    /// uniform parameters — pass new values on any call, no recompile),
    /// `out` receives one f32 result per point (the f64 result, demoted).
    fn eval_batch(&self, input: &[f32], uniforms: &[f32], out: &mut [f32]);

    /// Single-point evaluation with the full-precision f64 result (the
    /// exactness suites compare this against the splash interpreter).
    /// `args` covers the per-point parameters followed by the uniforms.
    fn call(&self, args: &[MathAotValue]) -> Option<f64>;
}

/// Static type of a compiled function's parameter.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MathAotParam {
    Scalar,
    Vec2,
    Vec3,
    Vec4,
}

impl MathAotParam {
    pub fn lanes(self) -> u32 {
        match self {
            MathAotParam::Scalar => 1,
            MathAotParam::Vec2 => 2,
            MathAotParam::Vec3 => 3,
            MathAotParam::Vec4 => 4,
        }
    }

    fn ct(self) -> CtType {
        match self {
            MathAotParam::Scalar => CtType::Scalar,
            MathAotParam::Vec2 => CtType::Vec(2),
            MathAotParam::Vec3 => CtType::Vec(3),
            MathAotParam::Vec4 => CtType::Vec(4),
        }
    }
}

/// An argument value for [`CompiledMath::call`].
#[derive(Clone, Copy, Debug)]
pub enum MathAotValue {
    Scalar(f64),
    Vec2([f32; 2]),
    Vec3([f32; 3]),
    Vec4([f32; 4]),
}

impl MathAotValue {
    pub(crate) fn to_vir(self) -> vir::VirVal {
        match self {
            MathAotValue::Scalar(v) => vir::VirVal::F64(v),
            MathAotValue::Vec2(v) => vir::VirVal::V([v[0], v[1], 0.0, 0.0]),
            MathAotValue::Vec3(v) => vir::VirVal::V([v[0], v[1], v[2], 0.0]),
            MathAotValue::Vec4(v) => vir::VirVal::V(v),
        }
    }

    fn lanes(&self) -> u32 {
        match self {
            MathAotValue::Scalar(_) => 1,
            MathAotValue::Vec2(_) => 2,
            MathAotValue::Vec3(_) => 3,
            MathAotValue::Vec4(_) => 4,
        }
    }

    /// Flattens this value's f32 lanes (scalars round to f32, matching
    /// the batch/uniform lane convention).
    pub(crate) fn push_lanes(&self, out: &mut Vec<f32>) {
        match self {
            MathAotValue::Scalar(v) => out.push(*v as f32),
            MathAotValue::Vec2(v) => out.extend_from_slice(v),
            MathAotValue::Vec3(v) => out.extend_from_slice(v),
            MathAotValue::Vec4(v) => out.extend_from_slice(v),
        }
    }
}

// =========================================================================
// The VIR interpreter backend (semantic reference)
// =========================================================================

pub struct VirInterpBackend;

struct VirInterpCompiled {
    f: VirFn,
}

impl MathBackend for VirInterpBackend {
    fn compile(&self, f: &VirFn) -> Result<Box<dyn CompiledMath>, BackendUnsupported> {
        Ok(Box::new(VirInterpCompiled { f: f.clone() }))
    }
}

impl CompiledMath for VirInterpCompiled {
    fn eval_batch(&self, input: &[f32], uniforms: &[f32], out: &mut [f32]) {
        let stride = self.f.stride();
        assert!(input.len() == out.len() * stride);
        assert!(uniforms.len() == self.f.uniform_stride());
        let mut params: Vec<vir::VirVal> = Vec::new();
        for (i, out) in out.iter_mut().enumerate() {
            params.clear();
            let mut off = i * stride;
            for lanes in &self.f.param_lanes {
                match lanes {
                    1 => params.push(vir::VirVal::F64(input[off] as f64)),
                    _ => {
                        let mut v = [0f32; 4];
                        for (lane, v) in v.iter_mut().enumerate().take(*lanes as usize) {
                            *v = input[off + lane];
                        }
                        params.push(vir::VirVal::V(v));
                    }
                }
                off += *lanes as usize;
            }
            *out = vir::eval(&self.f, &params, uniforms) as f32;
        }
    }

    fn call(&self, args: &[MathAotValue]) -> Option<f64> {
        let point_count = self.f.param_lanes.len();
        if args.len() != point_count + self.f.uniform_lanes.len() {
            return None;
        }
        for (arg, lanes) in args[..point_count].iter().zip(self.f.param_lanes.iter()) {
            if arg.lanes() != *lanes as u32 {
                return None;
            }
        }
        let params: Vec<vir::VirVal> = args[..point_count].iter().map(|a| a.to_vir()).collect();
        let mut uniforms = Vec::new();
        for (arg, lanes) in args[point_count..].iter().zip(self.f.uniform_lanes.iter()) {
            if arg.lanes() != *lanes as u32 {
                return None;
            }
            arg.push_lanes(&mut uniforms);
        }
        Some(vir::eval(&self.f, &params, &uniforms))
    }
}

pub use stitch_backend::StitchBackend;

// =========================================================================
// Public compile entry
// =========================================================================

/// A compiled splash math function (the default backend's result plus its
/// parameter signature).
pub struct CompiledMathExpr {
    inner: Box<dyn CompiledMath>,
    params: Vec<MathAotParam>,
}

impl CompiledMathExpr {
    pub fn params(&self) -> &[MathAotParam] {
        &self.params
    }

    pub fn call(&mut self, args: &[MathAotValue]) -> Option<f64> {
        self.inner.call(args)
    }

    pub fn eval_batch(&mut self, input: &[f32], uniforms: &[f32], out: &mut [f32]) {
        self.inner.eval_batch(input, uniforms, out)
    }

    /// The backend-agnostic evaluator (for handing to samplers).
    pub fn into_inner(self) -> Box<dyn CompiledMath> {
        self.inner
    }
}

/// Identity snapshot of the supported natives plus the default backend.
/// Build one per VM (cheap), compile many functions with it.
pub struct MathAot {
    backend: StitchBackend,
    /// (resolved fn-object value, intrinsic) pairs; linear scan.
    natives: Vec<(ScriptValue, Intrinsic)>,
    /// (pod-type, lane count) for vec2f/vec3f/vec4f.
    vec_ctors: Vec<(ScriptPodType, u8)>,
    /// Swizzle id -> source lanes (xyzw / rgba, length 1..=4).
    swizzles: HashMap<LiveId, Vec<u8>>,
}

impl MathAot {
    pub fn new(vm: &mut ScriptVm) -> MathAot {
        use crate::trap::NoTrap;
        let mut natives = Vec::new();
        let math = vm.bx.heap.module(id!(math));
        let names: &[(&str, Intrinsic)] = &[
            ("sin", Intrinsic::Un(MathFn::Sin)),
            ("cos", Intrinsic::Un(MathFn::Cos)),
            ("tan", Intrinsic::Un(MathFn::Tan)),
            ("asin", Intrinsic::Un(MathFn::Asin)),
            ("acos", Intrinsic::Un(MathFn::Acos)),
            ("atan", Intrinsic::Un(MathFn::Atan)),
            ("exp", Intrinsic::Un(MathFn::Exp)),
            ("log", Intrinsic::Un(MathFn::Ln)),
            ("sqrt", Intrinsic::Sqrt),
            ("abs", Intrinsic::Abs),
            ("floor", Intrinsic::Floor),
            ("ceil", Intrinsic::Ceil),
            ("fract", Intrinsic::Fract),
            ("inverseSqrt", Intrinsic::InverseSqrt),
            ("radians", Intrinsic::Scale(std::f32::consts::PI / 180.0)),
            ("degrees", Intrinsic::Scale(180.0 / std::f32::consts::PI)),
            ("atan2", Intrinsic::Bin(MathFn2::Atan2)),
            ("pow", Intrinsic::Bin(MathFn2::Pow)),
            ("min", Intrinsic::Bin(MathFn2::RMin)),
            ("max", Intrinsic::Bin(MathFn2::RMax)),
            ("modf", Intrinsic::Bin(MathFn2::Rem)),
            ("step", Intrinsic::Step),
            ("clamp", Intrinsic::Clamp),
            ("mix", Intrinsic::Mix),
            ("lerp", Intrinsic::Lerp),
            ("smoothstep", Intrinsic::Smoothstep),
            ("length", Intrinsic::Length),
            ("distance", Intrinsic::Distance),
            ("dot", Intrinsic::Dot),
            ("normalize", Intrinsic::Normalize),
            ("cross", Intrinsic::Cross),
        ];
        for (name, intr) in names {
            let value = vm
                .bx
                .heap
                .value(math, LiveId::from_str(name).into(), NoTrap);
            if value.as_object().is_some() {
                natives.push((value, *intr));
            }
        }
        // Pod methods (mod_pod.rs): same lane semantics, different fn
        // objects, registered in the per-type native table.
        {
            let native = vm.bx.code.native.borrow();
            for redux in [ScriptValueType::REDUX_POD, ScriptValueType::REDUX_POD_TYPE] {
                if let Some(table) = native.type_table.get(redux.to_index()) {
                    let methods: &[(&str, Intrinsic)] = &[
                        ("length", Intrinsic::Length),
                        ("normalize", Intrinsic::Normalize),
                        ("normalized", Intrinsic::Normalize),
                        ("dot", Intrinsic::Dot),
                        ("cross", Intrinsic::Cross),
                        ("mix", Intrinsic::Mix),
                    ];
                    for (name, intr) in methods {
                        if let Some(obj) = table.get(&LiveId::from_str(name)) {
                            natives.push(((*obj).into(), *intr));
                        }
                    }
                }
            }
        }
        let pod = &vm.bx.code.builtins.pod;
        let vec_ctors = vec![
            (pod.pod_vec2f, 2u8),
            (pod.pod_vec3f, 3u8),
            (pod.pod_vec4f, 4u8),
        ];
        // Swizzle table: every xyzw / rgba combination up to 4 lanes.
        let mut swizzles = HashMap::new();
        for charset in [b"xyzw", b"rgba"] {
            let n = charset.len();
            for len in 1..=4usize {
                for combo in 0..n.pow(len as u32) {
                    let mut name = String::new();
                    let mut lanes = Vec::new();
                    let mut c = combo;
                    for _ in 0..len {
                        let lane = c % n;
                        c /= n;
                        name.push(charset[lane] as char);
                        lanes.push(lane as u8);
                    }
                    swizzles.insert(LiveId::from_str(&name), lanes);
                }
            }
        }
        MathAot {
            backend: StitchBackend::new(),
            natives,
            vec_ctors,
            swizzles,
        }
    }

    /// Translates a splash function value into VIR, or `None` if its
    /// bytecode falls outside the pure-math subset.
    ///
    /// `params` types the function's leading per-point parameters;
    /// `uniforms` types the remaining (trailing) parameters as uniforms —
    /// batch-constant values changeable per invocation without a
    /// recompile (the parametric-model loop).
    pub fn to_vir(
        &self,
        vm: &ScriptVm,
        fn_value: ScriptValue,
        params: &[MathAotParam],
        uniforms: &[MathAotParam],
    ) -> Option<VirFn> {
        let fn_obj = fn_value.as_object()?;
        let Some(ScriptFnPtr::Script(fn_ip)) = vm.bx.heap.as_fn(fn_obj) else {
            return None;
        };
        let bodies = vm.bx.code.bodies.borrow();
        let body = bodies.get(fn_ip.body as usize)?;
        let ops = &body.parser.opcodes;
        if fn_ip.index == 0 {
            return None;
        }
        let (fn_body_op, fn_body_args) = ops.get(fn_ip.index as usize - 1)?.as_opcode()?;
        // Only dynamic (untyped) fn bodies.
        if fn_body_op != Opcode::FN_BODY_DYN || !fn_body_args.is_u32() {
            return None;
        }
        let end = (fn_ip.index - 1) + fn_body_args.to_u32();
        let ops = ops.get(fn_ip.index as usize..end as usize)?;

        // Declared parameter names, in order (NIL-keyed entries are
        // varargs, `self` is not a real parameter).
        let mut param_names = Vec::new();
        for i in 0..vm.bx.heap.vec_len(fn_obj) {
            let kv = vm.bx.heap.vec_key_value(fn_obj, i, crate::trap::NoTrap);
            if let Some(id) = kv.key.as_id() {
                if id != id!(self) {
                    param_names.push(id);
                }
            }
        }
        if param_names.len() != params.len() + uniforms.len() {
            return None;
        }

        let mut f = VirFn {
            param_lanes: params.iter().map(|p| p.lanes() as u8).collect(),
            uniform_lanes: uniforms.iter().map(|p| p.lanes() as u8).collect(),
            ops: Vec::new(),
            types: Vec::new(),
            result: VirReg(0),
        };
        let mut param_map = HashMap::new();
        for (index, (name, param)) in param_names.iter().zip(params.iter()).enumerate() {
            let ty = match param {
                MathAotParam::Scalar => VirTy::F64,
                _ => VirTy::V,
            };
            f.ops.push(VirOp::Param {
                index: index as u32,
                ty,
            });
            f.types.push(ty);
            param_map.insert(*name, (VirReg(index as u32), param.ct()));
        }
        for (index, (name, param)) in param_names[params.len()..]
            .iter()
            .zip(uniforms.iter())
            .enumerate()
        {
            let ty = match param {
                MathAotParam::Scalar => VirTy::F64,
                _ => VirTy::V,
            };
            let reg = VirReg(f.ops.len() as u32);
            f.ops.push(VirOp::Uniform {
                index: index as u32,
                ty,
            });
            f.types.push(ty);
            param_map.insert(*name, (reg, param.ct()));
        }

        let mut tr = Translator {
            aot: self,
            vm,
            fn_obj,
            ops,
            f,
            param_map,
            slot_regs: Vec::new(),
            stack: Vec::new(),
            calls: Vec::new(),
        };
        let result = tr.translate_fn_tail(0, ops.len())?;
        let (result, ty) = result;
        if ty != CtType::Scalar {
            return None;
        }
        let mut f = tr.f;
        f.result = result;
        Some(f)
    }

    /// Translates and compiles with the default backend (stitch).
    pub fn compile(
        &self,
        vm: &ScriptVm,
        fn_value: ScriptValue,
        params: &[MathAotParam],
        uniforms: &[MathAotParam],
    ) -> Option<CompiledMathExpr> {
        let f = self.to_vir(vm, fn_value, params, uniforms)?;
        let inner = self.backend.compile(&f).ok()?;
        Some(CompiledMathExpr {
            inner,
            params: params.to_vec(),
        })
    }
}

// =========================================================================
// The bytecode -> VIR translator
// =========================================================================

/// Static value type on the translator's stack. Scalars are f64 whatever
/// their storage tag (every supported consumer widens through
/// `as_number`/`cast_to_f64` or branches identically for all numeric
/// tags); vectors carry their width.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum CtType {
    Scalar,
    Bool,
    Vec(u8),
}

impl CtType {
    fn vir(self) -> VirTy {
        match self {
            CtType::Scalar => VirTy::F64,
            CtType::Bool => VirTy::Bool,
            CtType::Vec(_) => VirTy::V,
        }
    }
}

/// A typed VIR register on the translator stack.
#[derive(Clone, Copy, Debug)]
struct CtVal {
    reg: VirReg,
    ty: CtType,
}

#[derive(Clone, Copy, Debug)]
enum CtItem {
    Val(CtVal),
    /// An unresolved identifier.
    Id(LiveId),
    /// A compile-time constant that is not a number (module / fn / pod
    /// type object).
    Known(ScriptValue),
    /// A nil statement marker.
    Nil,
}

/// The supported intrinsics, each mirroring one native in
/// shader_builtins.rs / mod_pod.rs.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Intrinsic {
    Un(MathFn),
    Bin(MathFn2),
    Sqrt,
    Abs,
    Floor,
    Ceil,
    Fract,
    InverseSqrt,
    /// radians/degrees: lane multiply by an f32 constant.
    Scale(f32),
    Step,
    Clamp,
    Mix,
    Lerp,
    Smoothstep,
    Length,
    Distance,
    Dot,
    Normalize,
    Cross,
}

#[derive(Clone, Copy, Debug)]
enum CallTarget {
    Intrinsic(Intrinsic),
    Ctor(u8),
}

struct CtCall {
    target: CallTarget,
    args: Vec<CtVal>,
}

/// Rejection helper: makes "return None on anything unsupported" read as
/// intent.
macro_rules! reject {
    () => {
        return None
    };
}

struct Translator<'a> {
    aot: &'a MathAot,
    vm: &'a ScriptVm<'a>,
    fn_obj: ScriptObject,
    ops: &'a [ScriptValue],
    f: VirFn,
    param_map: HashMap<LiveId, (VirReg, CtType)>,
    /// splash slot index -> current SSA value.
    slot_regs: Vec<Option<CtVal>>,
    stack: Vec<CtItem>,
    calls: Vec<CtCall>,
}

impl<'a> Translator<'a> {
    // -- VIR emission -----------------------------------------------------

    fn emit(&mut self, op: VirOp, ty: VirTy) -> VirReg {
        let reg = VirReg(self.f.ops.len() as u32);
        self.f.ops.push(op);
        self.f.types.push(ty);
        reg
    }

    fn scalar(&mut self, op: VirOp) -> CtVal {
        CtVal {
            reg: self.emit(op, VirTy::F64),
            ty: CtType::Scalar,
        }
    }

    fn f32v(&mut self, op: VirOp) -> VirReg {
        self.emit(op, VirTy::F32)
    }

    fn vec(&mut self, op: VirOp, w: u8) -> CtVal {
        CtVal {
            reg: self.emit(op, VirTy::V),
            ty: CtType::Vec(w),
        }
    }

    fn boolean(&mut self, op: VirOp) -> CtVal {
        CtVal {
            reg: self.emit(op, VirTy::Bool),
            ty: CtType::Bool,
        }
    }

    fn const_f64(&mut self, v: f64) -> CtVal {
        self.scalar(VirOp::ConstF64(v))
    }

    fn demote(&mut self, val: CtVal) -> VirReg {
        debug_assert!(val.ty == CtType::Scalar);
        self.f32v(VirOp::Demote(val.reg))
    }

    fn promote(&mut self, reg: VirReg) -> CtVal {
        self.scalar(VirOp::Promote(reg))
    }

    fn splat_scalar(&mut self, val: CtVal) -> VirReg {
        let d = self.demote(val);
        self.emit(VirOp::Splat(d), VirTy::V)
    }

    // -- stack bookkeeping ------------------------------------------------

    fn pop(&mut self) -> Option<CtItem> {
        self.stack.pop()
    }

    /// Pops an item and resolves it to a value, mirroring
    /// `pop_stack_resolved`.
    fn pop_value(&mut self) -> Option<CtVal> {
        match self.pop()? {
            CtItem::Val(v) => Some(v),
            CtItem::Id(id) => self.id_value(id),
            CtItem::Known(_) | CtItem::Nil => None,
        }
    }

    /// The value an identifier resolves to: a parameter, or a numeric
    /// compile-time constant from the captured scope chain.
    fn id_value(&mut self, id: LiveId) -> Option<CtVal> {
        if let Some((reg, ty)) = self.param_map.get(&id).copied() {
            return Some(CtVal { reg, ty });
        }
        let value = self
            .vm
            .bx
            .heap
            .scope_value(self.fn_obj, id, crate::trap::NoTrap);
        if value.is_err() {
            reject!();
        }
        if let Some(v) = value.as_number() {
            return Some(self.const_f64(v));
        }
        None
    }

    /// Resolves an identifier to a compile-time item without emitting.
    fn resolve_id(&mut self, id: LiveId) -> Option<CtItem> {
        if self.param_map.contains_key(&id) {
            return Some(CtItem::Id(id));
        }
        let value = self
            .vm
            .bx
            .heap
            .scope_value(self.fn_obj, id, crate::trap::NoTrap);
        if value.is_err() {
            reject!();
        }
        Some(CtItem::Known(value))
    }

    // -- casts ------------------------------------------------------------

    /// `cast_to_bool`: numbers are truthy iff != 0 (NaN is truthy).
    fn truthy(&mut self, val: CtVal) -> Option<VirReg> {
        match val.ty {
            CtType::Scalar => {
                let zero = self.const_f64(0.0);
                Some(self.emit(VirOp::CmpF64(CmpCc::Ne, val.reg, zero.reg), VirTy::Bool))
            }
            CtType::Bool => Some(val.reg),
            CtType::Vec(_) => None,
        }
    }

    // -- slot merge helpers (SSA phis via select) -------------------------

    fn snapshot_slots(&self) -> Vec<Option<CtVal>> {
        self.slot_regs.clone()
    }

    /// After translating a conditional region: for every slot the region
    /// changed, merge `cond ? region_value : before_value`.
    fn merge_slots(
        &mut self,
        cond: VirReg,
        before: &[Option<CtVal>],
        region: Vec<Option<CtVal>>,
    ) -> Option<()> {
        for (slot, (old, new)) in before.iter().zip(region.into_iter()).enumerate() {
            match (old, new) {
                (Some(old), Some(new)) if old.reg != new.reg => {
                    if old.ty != new.ty {
                        reject!();
                    }
                    let merged = match old.ty {
                        CtType::Scalar => VirOp::SelF64(cond, new.reg, old.reg),
                        CtType::Vec(_) => VirOp::SelV(cond, new.reg, old.reg),
                        CtType::Bool => reject!(),
                    };
                    let reg = self.emit(merged, old.ty.vir());
                    self.slot_regs[slot] = Some(CtVal { reg, ty: old.ty });
                }
                (Some(old), Some(_)) => self.slot_regs[slot] = Some(*old),
                (None, Some(_)) => {
                    // A slot first assigned inside a conditional region:
                    // there is no "before" value to merge with; keep it
                    // interpreted.
                    reject!();
                }
                (old, None) => self.slot_regs[slot] = *old,
            }
        }
        Some(())
    }

    /// Merges two branch slot states through `cond ? then : else`.
    fn merge_slots2(
        &mut self,
        cond: VirReg,
        then_slots: Vec<Option<CtVal>>,
        else_slots: Vec<Option<CtVal>>,
    ) -> Option<()> {
        for (slot, (t, e)) in then_slots
            .into_iter()
            .zip(else_slots.into_iter())
            .enumerate()
        {
            match (t, e) {
                (Some(t), Some(e)) if t.reg != e.reg => {
                    if t.ty != e.ty {
                        reject!();
                    }
                    let merged = match t.ty {
                        CtType::Scalar => VirOp::SelF64(cond, t.reg, e.reg),
                        CtType::Vec(_) => VirOp::SelV(cond, t.reg, e.reg),
                        CtType::Bool => reject!(),
                    };
                    let reg = self.emit(merged, t.ty.vir());
                    self.slot_regs[slot] = Some(CtVal { reg, ty: t.ty });
                }
                (Some(t), Some(_)) => self.slot_regs[slot] = Some(t),
                (t, e) => {
                    if t.map(|v| v.reg) != e.map(|v| v.reg) {
                        reject!();
                    }
                    self.slot_regs[slot] = t;
                }
            }
        }
        Some(())
    }

    // -- function-tail translation (handles top-level early return) -------

    /// Translates ops[ip..end] to function completion and returns the
    /// function's result value.
    fn translate_fn_tail(&mut self, mut ip: usize, end: usize) -> Option<(VirReg, CtType)> {
        while ip < end {
            // Top-level RETURN?
            if let Some((Opcode::RETURN, args)) = self.ops[ip].as_opcode() {
                if args.is_nil() {
                    reject!();
                }
                let val = self.pop_value()?;
                // Everything after a top-level return is unreachable.
                return Some((val.reg, val.ty));
            }
            // Statement-if whose then-branch RETURNS: lower to a select
            // against the translated continuation.
            if let Some(early) = self.try_early_return_if(ip, end)? {
                return Some(early);
            }
            ip = self.translate_op(ip, end)?;
        }
        // The bytecode always ends with RETURN, handled above.
        None
    }

    /// Recognizes `if cond { ... return v }` (no else) at `ip`; if
    /// matched, translates it plus the continuation and returns the
    /// merged function result.
    fn try_early_return_if(&mut self, ip: usize, end: usize) -> Option<Option<(VirReg, CtType)>> {
        let Some((Opcode::IF_TEST, args)) = self.ops[ip].as_opcode() else {
            return Some(None);
        };
        if !args.is_u32() {
            reject!();
        }
        let else_target = ip + args.to_u32() as usize;
        if else_target > end {
            reject!();
        }
        // A real if/else is handled by translate_op.
        if else_target >= 1 {
            if let Some((Opcode::IF_ELSE, _)) = self.ops[else_target - 1].as_opcode() {
                return Some(None);
            }
        }
        // Does the then-branch end with RETURN?
        let Some((Opcode::RETURN, ret_args)) = self.ops[else_target - 1].as_opcode() else {
            return Some(None);
        };
        if ret_args.is_nil() {
            reject!();
        }

        let cond = self.pop_value()?;
        let cond = self.truthy(cond)?;

        // Translate the then-branch up to (but excluding) its RETURN,
        // then take the return value. Slot writes in the branch are
        // discarded: the branch never falls through to the continuation.
        let before = self.snapshot_slots();
        let depth = self.stack.len();
        let mut tip = ip + 1;
        while tip < else_target - 1 {
            tip = self.translate_op(tip, else_target - 1)?;
        }
        let then_val = self.pop_value()?;
        // Only nil statement markers may remain.
        while self.stack.len() > depth {
            match self.stack.pop() {
                Some(CtItem::Nil) => {}
                _ => reject!(),
            }
        }
        self.slot_regs = before;

        // Continuation: with NEED_NIL the interpreter pushes NIL on the
        // not-taken path and a trailing POP_TO_ME drops it — skip that
        // marker dance entirely and translate from the join.
        let mut cont_ip = else_target;
        if args.is_need_nil() {
            if let Some((Opcode::POP_TO_ME, _)) = self.ops.get(cont_ip).and_then(|op| op.as_opcode())
            {
                cont_ip += 1;
            }
        }
        let (rest_reg, rest_ty) = self.translate_fn_tail(cont_ip, end)?;
        if rest_ty != then_val.ty {
            reject!();
        }
        let merged = match rest_ty {
            CtType::Scalar => VirOp::SelF64(cond, then_val.reg, rest_reg),
            CtType::Vec(_) => VirOp::SelV(cond, then_val.reg, rest_reg),
            CtType::Bool => reject!(),
        };
        let reg = self.emit(merged, rest_ty.vir());
        Some(Some((reg, rest_ty)))
    }

    // -- range translation (no returns) -----------------------------------

    fn translate_range(&mut self, start: usize, end: usize) -> Option<()> {
        let mut ip = start;
        while ip < end {
            ip = self.translate_op(ip, end)?;
        }
        Some(())
    }

    /// Translates one opcode or literal at `ip`; returns the next ip.
    /// RETURN is rejected here (handled only by `translate_fn_tail`).
    fn translate_op(&mut self, ip: usize, end: usize) -> Option<usize> {
        let slot = self.ops[ip];
        let Some((op, args)) = slot.as_opcode() else {
            if let Some(id) = slot.as_id() {
                if slot.is_escaped_id() {
                    reject!();
                }
                self.stack.push(CtItem::Id(id));
            } else if slot.is_nil() {
                self.stack.push(CtItem::Nil);
            } else if slot.as_bool().is_some() {
                // Boolean literals are rare in math expressions; keep
                // them interpreted.
                reject!();
            } else if let Some(v) = slot.as_number() {
                let val = self.const_f64(v);
                self.stack.push(CtItem::Val(val));
            } else {
                reject!();
            }
            return Some(ip + 1);
        };

        let mut next_ip = ip + 1;
        match op {
            Opcode::NOP => {}

            Opcode::NEG => {
                let v = self.pop_value()?;
                match v.ty {
                    CtType::Scalar => {
                        // handle_neg scalar: -f in f64.
                        let val = self.scalar(VirOp::NegF64(v.reg));
                        self.stack.push(CtItem::Val(val));
                    }
                    CtType::Vec(w) => {
                        // handle_neg pod: lane * -1.0f32 (NOT a sign flip).
                        let neg1 = self.f32v(VirOp::ConstF32(-1.0));
                        let neg1v = self.emit(VirOp::Splat(neg1), VirTy::V);
                        let val = self.vec(VirOp::MulV(v.reg, neg1v), w);
                        self.stack.push(CtItem::Val(val));
                    }
                    CtType::Bool => reject!(),
                }
            }

            Opcode::ADD | Opcode::SUB | Opcode::MUL | Opcode::DIV => {
                let (a, b) = self.binary_operands(args)?;
                let val = self.arith(op, a, b)?;
                self.stack.push(CtItem::Val(val));
            }

            Opcode::MOD => {
                // handle_f64_op: cast_to_f64 both, then Rust %.
                let (a, b) = self.binary_operands(args)?;
                if a.ty != CtType::Scalar || b.ty != CtType::Scalar {
                    reject!();
                }
                let val = self.scalar(VirOp::RemF64(a.reg, b.reg));
                self.stack.push(CtItem::Val(val));
            }

            Opcode::LT | Opcode::GT | Opcode::LEQ | Opcode::GEQ => {
                let (a, b) = self.binary_operands(args)?;
                if a.ty != CtType::Scalar || b.ty != CtType::Scalar {
                    reject!();
                }
                let cc = match op {
                    Opcode::LT => CmpCc::Lt,
                    Opcode::GT => CmpCc::Gt,
                    Opcode::LEQ => CmpCc::Le,
                    Opcode::GEQ => CmpCc::Ge,
                    _ => unreachable!(),
                };
                let val = self.boolean(VirOp::CmpF64(cc, a.reg, b.reg));
                self.stack.push(CtItem::Val(val));
            }

            // EQ/NEQ are NOT accepted: splash deep_eq starts with a raw
            // bit compare of the NaN-boxed values, so two traced NaNs
            // from the same source location compare EQUAL — a semantic
            // the compiled form cannot reproduce for data-dependent NaNs.
            // Equality tests fall back to the interpreter.

            Opcode::LOGIC_AND_TEST | Opcode::LOGIC_OR_TEST => {
                // [lhs] TEST(d) [rhs...] — value-preserving short-circuit.
                // Both sides are evaluated (side-effect free); the select
                // keeps the interpreter's result.
                if !args.is_u32() {
                    reject!();
                }
                let target = ip + args.to_u32() as usize;
                if target > end {
                    reject!();
                }
                let lhs = self.pop_value()?;
                let cond = self.truthy(lhs)?;
                let before = self.snapshot_slots();
                let depth = self.stack.len();
                self.translate_range(ip + 1, target)?;
                if self.stack.len() != depth + 1 {
                    reject!();
                }
                let rhs = self.pop_value()?;
                if rhs.ty != lhs.ty {
                    reject!();
                }
                let region = std::mem::replace(&mut self.slot_regs, before.clone());
                // AND: truthy -> rhs (with its slot writes); OR: truthy -> lhs.
                let (keep_cond, sel) = if op == Opcode::LOGIC_AND_TEST {
                    (cond, (rhs, lhs))
                } else {
                    let not = self.emit(VirOp::BoolNot(cond), VirTy::Bool);
                    (not, (rhs, lhs))
                };
                self.merge_slots(keep_cond, &before, region)?;
                let merged = match lhs.ty {
                    CtType::Scalar => VirOp::SelF64(keep_cond, sel.0.reg, sel.1.reg),
                    CtType::Vec(_) => VirOp::SelV(keep_cond, sel.0.reg, sel.1.reg),
                    CtType::Bool => reject!(),
                };
                let reg = self.emit(merged, lhs.ty.vir());
                self.stack.push(CtItem::Val(CtVal { reg, ty: lhs.ty }));
                next_ip = target;
            }

            Opcode::IF_TEST => {
                if !args.is_u32() {
                    reject!();
                }
                let else_target = ip + args.to_u32() as usize;
                if else_target > end || else_target <= ip + 1 {
                    reject!();
                }
                let cond = self.pop_value()?;
                let cond = self.truthy(cond)?;

                let (then_end, else_range) = match self.ops[else_target - 1].as_opcode() {
                    Some((Opcode::IF_ELSE, else_args)) if else_args.is_u32() => {
                        let join = (else_target - 1) + else_args.to_u32() as usize;
                        if join > end {
                            reject!();
                        }
                        (else_target - 1, Some((else_target, join)))
                    }
                    _ => (else_target, None),
                };

                let before = self.snapshot_slots();
                let depth = self.stack.len();
                self.translate_range(ip + 1, then_end)?;
                while self.stack.len() > depth
                    && matches!(self.stack.last(), Some(CtItem::Nil))
                {
                    self.stack.pop();
                }
                let then_val = match self.stack.len() - depth {
                    0 => None,
                    1 => Some(self.pop_value()?),
                    _ => reject!(),
                };
                let then_slots = std::mem::replace(&mut self.slot_regs, before.clone());

                if let Some((else_start, join)) = else_range {
                    self.translate_range(else_start, join)?;
                    while self.stack.len() > depth
                        && matches!(self.stack.last(), Some(CtItem::Nil))
                    {
                        self.stack.pop();
                    }
                    let else_val = match self.stack.len() - depth {
                        0 => None,
                        1 => Some(self.pop_value()?),
                        _ => reject!(),
                    };
                    let else_slots = std::mem::replace(&mut self.slot_regs, before);
                    self.merge_slots2(cond, then_slots, else_slots)?;
                    match (then_val, else_val) {
                        (Some(t), Some(e)) => {
                            if t.ty != e.ty {
                                reject!();
                            }
                            let merged = match t.ty {
                                CtType::Scalar => VirOp::SelF64(cond, t.reg, e.reg),
                                CtType::Vec(_) => VirOp::SelV(cond, t.reg, e.reg),
                                CtType::Bool => reject!(),
                            };
                            let reg = self.emit(merged, t.ty.vir());
                            self.stack.push(CtItem::Val(CtVal { reg, ty: t.ty }));
                        }
                        (None, None) => {}
                        _ => reject!(),
                    }
                    next_ip = join;
                } else {
                    // Statement if (no else): the then branch must not
                    // produce a value. Slot writes merge under the
                    // condition; the interpreter's NEED_NIL nil is a
                    // statement marker.
                    if then_val.is_some() {
                        reject!();
                    }
                    self.merge_slots(cond, &before, then_slots)?;
                    if args.is_need_nil() {
                        self.stack.push(CtItem::Nil);
                    }
                    next_ip = else_target;
                }
            }

            Opcode::RETURN => {
                // Only translate_fn_tail may consume returns.
                reject!();
            }

            Opcode::SLOTS_FRAME => {
                if !args.is_u32() {
                    reject!();
                }
                self.slot_regs = vec![None; args.to_u32() as usize];
            }

            Opcode::PUSH_SLOT => {
                let val = self.slot_regs.get(args.to_u32() as usize).copied().flatten()?;
                self.stack.push(CtItem::Val(val));
            }

            Opcode::LET_SLOT | Opcode::STORE_SLOT => {
                let value = self.pop_value()?;
                match self.pop()? {
                    CtItem::Id(_) => {}
                    _ => reject!(),
                }
                let slot = args.to_u32() as usize;
                if slot >= self.slot_regs.len() {
                    reject!();
                }
                if let Some(old) = self.slot_regs[slot] {
                    if old.ty != value.ty {
                        reject!();
                    }
                }
                self.slot_regs[slot] = Some(value);
                if op == Opcode::STORE_SLOT {
                    self.stack.push(CtItem::Nil);
                }
            }

            Opcode::ASSIGN_SLOT_ADD
            | Opcode::ASSIGN_SLOT_SUB
            | Opcode::ASSIGN_SLOT_MUL
            | Opcode::ASSIGN_SLOT_DIV
            | Opcode::ASSIGN_SLOT_MOD => {
                // slot = f(cast_to_f64(slot), cast_to_f64(value)); push NIL.
                let value = self.pop_value()?;
                if value.ty != CtType::Scalar {
                    reject!();
                }
                match self.pop()? {
                    CtItem::Id(_) => {}
                    _ => reject!(),
                }
                let slot = args.to_u32() as usize;
                let old = self.slot_regs.get(slot).copied().flatten()?;
                if old.ty != CtType::Scalar {
                    reject!();
                }
                let vop = match op {
                    Opcode::ASSIGN_SLOT_ADD => VirOp::AddF64(old.reg, value.reg),
                    Opcode::ASSIGN_SLOT_SUB => VirOp::SubF64(old.reg, value.reg),
                    Opcode::ASSIGN_SLOT_MUL => VirOp::MulF64(old.reg, value.reg),
                    Opcode::ASSIGN_SLOT_DIV => VirOp::DivF64(old.reg, value.reg),
                    Opcode::ASSIGN_SLOT_MOD => VirOp::RemF64(old.reg, value.reg),
                    _ => unreachable!(),
                };
                let new = self.scalar(vop);
                self.slot_regs[slot] = Some(new);
                self.stack.push(CtItem::Nil);
            }

            Opcode::FIELD => {
                // [object, field-id] — field popped raw, object resolved.
                let field = match self.pop()? {
                    CtItem::Id(id) => id,
                    _ => reject!(),
                };
                match self.pop()? {
                    CtItem::Val(v) => {
                        let val = self.swizzle(v, field)?;
                        self.stack.push(CtItem::Val(val));
                    }
                    CtItem::Id(obj_id) => {
                        if self.param_map.contains_key(&obj_id) {
                            let v = self.id_value(obj_id)?;
                            let val = self.swizzle(v, field)?;
                            self.stack.push(CtItem::Val(val));
                        } else {
                            let CtItem::Known(obj_val) = self.resolve_id(obj_id)? else {
                                reject!();
                            };
                            self.known_field(obj_val, field)?;
                        }
                    }
                    CtItem::Known(obj_val) => {
                        self.known_field(obj_val, field)?;
                    }
                    _ => reject!(),
                }
            }

            Opcode::CALL_ARGS => {
                let callee = match self.pop()? {
                    CtItem::Id(id) => match self.resolve_id(id)? {
                        CtItem::Known(v) => v,
                        _ => reject!(),
                    },
                    CtItem::Known(v) => v,
                    _ => reject!(),
                };
                let target = self.resolve_call_target(callee)?;
                self.calls.push(CtCall {
                    target,
                    args: Vec::new(),
                });
            }

            Opcode::METHOD_CALL_ARGS => {
                let method = match self.pop()? {
                    CtItem::Id(id) => id,
                    _ => reject!(),
                };
                let sself = self.pop_value()?;
                let CtType::Vec(_) = sself.ty else {
                    reject!();
                };
                let intr = self.resolve_pod_method(method)?;
                self.calls.push(CtCall {
                    target: CallTarget::Intrinsic(intr),
                    args: vec![sself],
                });
            }

            Opcode::CALL_EXEC | Opcode::METHOD_CALL_EXEC => {
                let call = self.calls.pop()?;
                let val = self.emit_call(call)?;
                self.stack.push(CtItem::Val(val));
            }

            Opcode::DROP => {
                self.pop()?;
            }

            Opcode::DUP => {
                let top = self.stack.last().copied()?;
                self.stack.push(top);
            }

            Opcode::POP_TO_ME => {
                self.pop_to_me()?;
                return Some(next_ip);
            }

            _ => reject!(),
        }

        // Mirror the interpreter's fused pop-to-me postlude.
        if args.is_pop_to_me() {
            self.pop_to_me()?;
        }
        Some(next_ip)
    }

    /// The operands of a binary opcode, honoring the parser's inline-u32
    /// fast path (an integer RHS constant fused into the opcode args).
    fn binary_operands(&mut self, args: OpcodeArgs) -> Option<(CtVal, CtVal)> {
        if args.is_u32() {
            let a = self.pop_value()?;
            let b = self.const_f64(args.to_u32() as f64);
            return Some((a, b));
        }
        let b_item = self.pop()?;
        let a = match self.pop()? {
            CtItem::Val(v) => v,
            CtItem::Id(id) => self.id_value(id)?,
            _ => reject!(),
        };
        let b = match b_item {
            CtItem::Val(v) => v,
            CtItem::Id(id) => self.id_value(id)?,
            _ => reject!(),
        };
        Some((a, b))
    }

    /// The fused / standalone POP_TO_ME: commits the top of stack to the
    /// innermost open call, or discards a statement result.
    fn pop_to_me(&mut self) -> Option<()> {
        match self.pop()? {
            CtItem::Val(v) => {
                if let Some(call) = self.calls.last_mut() {
                    call.args.push(v);
                }
                // Statement position: the interpreter discards the value.
            }
            CtItem::Id(id) => {
                let v = self.id_value(id)?;
                if let Some(call) = self.calls.last_mut() {
                    call.args.push(v);
                }
            }
            CtItem::Nil => {}
            CtItem::Known(_) => reject!(),
        }
        Some(())
    }

    // -- arithmetic (mirrors opcodes_ops.rs) ------------------------------

    fn arith(&mut self, op: Opcode, a: CtVal, b: CtVal) -> Option<CtVal> {
        match (a.ty, b.ty) {
            (CtType::Scalar, CtType::Scalar) => {
                let vop = match op {
                    Opcode::ADD => VirOp::AddF64(a.reg, b.reg),
                    Opcode::SUB => VirOp::SubF64(a.reg, b.reg),
                    Opcode::MUL => VirOp::MulF64(a.reg, b.reg),
                    Opcode::DIV => VirOp::DivF64(a.reg, b.reg),
                    _ => unreachable!(),
                };
                Some(self.scalar(vop))
            }
            (CtType::Vec(wa), CtType::Vec(wb)) => {
                if wa != wb {
                    // Mixed widths zero-fill in the interpreter; keep that
                    // path interpreted.
                    reject!();
                }
                self.packed_arith(op, a.reg, b.reg, wa)
            }
            (CtType::Vec(w), CtType::Scalar) => {
                let bs = self.splat_scalar(b);
                self.packed_arith(op, a.reg, bs, w)
            }
            (CtType::Scalar, CtType::Vec(w)) => {
                let as_ = self.splat_scalar(a);
                self.packed_arith(op, as_, b.reg, w)
            }
            _ => None,
        }
    }

    /// Packed lane arithmetic; DIV mirrors the interpreter's per-lane
    /// `if y != 0 { x / y } else { 0.0 }` guard.
    fn packed_arith(&mut self, op: Opcode, a: VirReg, b: VirReg, w: u8) -> Option<CtVal> {
        let vop = match op {
            Opcode::ADD => VirOp::AddV(a, b),
            Opcode::SUB => VirOp::SubV(a, b),
            Opcode::MUL => VirOp::MulV(a, b),
            Opcode::DIV => {
                let q = self.emit(VirOp::DivV(a, b), VirTy::V);
                let zero = self.emit(VirOp::ZeroV, VirTy::V);
                let nz = self.emit(VirOp::CmpV(CmpCc::Ne, b, zero), VirTy::Mask);
                return Some(self.vec(VirOp::SelLanes(nz, q, zero), w));
            }
            _ => unreachable!(),
        };
        Some(self.vec(vop, w))
    }

    // -- fields / swizzles ------------------------------------------------

    /// Swizzle on a vector value, mirroring pod_read_field: one lane
    /// yields an (f32-valued) scalar, multiple lanes a new vector. Lanes
    /// must be in-width (the interpreter zero-fills out-of-width reads;
    /// those stay interpreted).
    fn swizzle(&mut self, v: CtVal, field: LiveId) -> Option<CtVal> {
        let CtType::Vec(w) = v.ty else { reject!() };
        let lanes = self.aot.swizzles.get(&field)?.clone();
        if lanes.iter().any(|lane| *lane >= w) {
            reject!();
        }
        if lanes.len() == 1 {
            let lane = self.f32v(VirOp::ExtractLane(v.reg, lanes[0]));
            Some(self.promote(lane))
        } else {
            let mut shuffle = [0u8; 4];
            for (i, lane) in lanes.iter().enumerate() {
                shuffle[i] = *lane;
            }
            Some(self.vec(VirOp::Shuffle(v.reg, shuffle), lanes.len() as u8))
        }
    }

    /// Field access on a compile-time object (e.g. `math.PI`, `math.sin`).
    fn known_field(&mut self, obj_val: ScriptValue, field: LiveId) -> Option<()> {
        let obj = obj_val.as_object()?;
        let value = self
            .vm
            .bx
            .heap
            .value(obj, field.into(), crate::trap::NoTrap);
        if value.is_err() {
            reject!();
        }
        if let Some(v) = value.as_number() {
            let val = self.const_f64(v);
            self.stack.push(CtItem::Val(val));
        } else {
            self.stack.push(CtItem::Known(value));
        }
        Some(())
    }

    fn resolve_call_target(&self, callee: ScriptValue) -> Option<CallTarget> {
        if let Some(pod_ty) = self.vm.bx.heap.pod_type(callee) {
            for (ty, lanes) in &self.aot.vec_ctors {
                if *ty == pod_ty {
                    return Some(CallTarget::Ctor(*lanes));
                }
            }
            reject!();
        }
        for (value, intr) in &self.aot.natives {
            if *value == callee {
                return Some(CallTarget::Intrinsic(*intr));
            }
        }
        None
    }

    fn resolve_pod_method(&self, method: LiveId) -> Option<Intrinsic> {
        let native = self.vm.bx.code.native.borrow();
        for redux in [ScriptValueType::REDUX_POD, ScriptValueType::REDUX_POD_TYPE] {
            if let Some(table) = native.type_table.get(redux.to_index()) {
                if let Some(obj) = table.get(&method) {
                    let value: ScriptValue = (*obj).into();
                    for (known, intr) in &self.aot.natives {
                        if *known == value {
                            return Some(*intr);
                        }
                    }
                }
            }
        }
        None
    }

    // -- intrinsic emission (mirrors shader_builtins.rs / numeric.rs) -----

    fn emit_call(&mut self, call: CtCall) -> Option<CtVal> {
        match call.target {
            CallTarget::Ctor(w) => {
                if call.args.len() != w as usize
                    || call.args.iter().any(|a| a.ty != CtType::Scalar)
                {
                    reject!();
                }
                let mut v = self.emit(VirOp::ZeroV, VirTy::V);
                for (lane, arg) in call.args.iter().enumerate() {
                    let s = self.demote(*arg);
                    v = self.emit(VirOp::ReplaceLane(v, s, lane as u8), VirTy::V);
                }
                Some(CtVal {
                    reg: v,
                    ty: CtType::Vec(w),
                })
            }
            CallTarget::Intrinsic(intr) => self.intrinsic(intr, &call.args),
        }
    }

    /// `map_f32` unary application: scalars go demote -> f32 op ->
    /// promote; vectors get the packed op.
    fn map_un(
        &mut self,
        v: CtVal,
        scalar_op: impl FnOnce(VirReg) -> VirOp,
        packed_op: impl FnOnce(VirReg) -> VirOp,
    ) -> Option<CtVal> {
        match v.ty {
            CtType::Scalar => {
                let d = self.demote(v);
                let r = self.f32v(scalar_op(d));
                Some(self.promote(r))
            }
            CtType::Vec(w) => Some(self.vec(packed_op(v.reg), w)),
            CtType::Bool => None,
        }
    }

    /// `zip_f32` binary application: mirrors NumericValue::zip_f32
    /// including scalar broadcast (both-scalar rounds through f32).
    fn zip_bin(
        &mut self,
        a: CtVal,
        b: CtVal,
        scalar_op: impl FnOnce(VirReg, VirReg) -> VirOp,
        packed_op: impl FnOnce(VirReg, VirReg) -> VirOp,
    ) -> Option<CtVal> {
        match (a.ty, b.ty) {
            (CtType::Scalar, CtType::Scalar) => {
                let da = self.demote(a);
                let db = self.demote(b);
                let r = self.f32v(scalar_op(da, db));
                Some(self.promote(r))
            }
            (CtType::Vec(wa), CtType::Vec(wb)) => {
                if wa != wb {
                    reject!();
                }
                Some(self.vec(packed_op(a.reg, b.reg), wa))
            }
            (CtType::Vec(w), CtType::Scalar) => {
                let bs = self.splat_scalar(b);
                Some(self.vec(packed_op(a.reg, bs), w))
            }
            (CtType::Scalar, CtType::Vec(w)) => {
                let as_ = self.splat_scalar(a);
                Some(self.vec(packed_op(as_, b.reg), w))
            }
            _ => None,
        }
    }

    fn intrinsic(&mut self, intr: Intrinsic, args: &[CtVal]) -> Option<CtVal> {
        match intr {
            Intrinsic::Un(fun) => {
                let [v] = args else { reject!() };
                self.map_un(
                    *v,
                    |r| VirOp::MathF32(fun, r),
                    |r| VirOp::MathV(fun, r),
                )
            }
            Intrinsic::Sqrt => {
                let [v] = args else { reject!() };
                self.map_un(*v, VirOp::SqrtF32, VirOp::SqrtV)
            }
            Intrinsic::Abs => {
                let [v] = args else { reject!() };
                self.map_un(*v, VirOp::AbsF32, VirOp::AbsV)
            }
            Intrinsic::Floor => {
                let [v] = args else { reject!() };
                self.map_un(*v, VirOp::FloorF32, VirOp::FloorV)
            }
            Intrinsic::Ceil => {
                let [v] = args else { reject!() };
                self.map_un(*v, VirOp::CeilF32, VirOp::CeilV)
            }
            Intrinsic::Fract => {
                // Rust fract = self - self.trunc()
                let [v] = args else { reject!() };
                match v.ty {
                    CtType::Scalar => {
                        let d = self.demote(*v);
                        let t = self.f32v(VirOp::TruncF32(d));
                        let r = self.f32v(VirOp::SubF32(d, t));
                        Some(self.promote(r))
                    }
                    CtType::Vec(w) => {
                        let t = self.emit(VirOp::TruncV(v.reg), VirTy::V);
                        Some(self.vec(VirOp::SubV(v.reg, t), w))
                    }
                    CtType::Bool => None,
                }
            }
            Intrinsic::InverseSqrt => {
                // |v| v.sqrt().recip() = 1.0 / sqrt(v)
                let [v] = args else { reject!() };
                match v.ty {
                    CtType::Scalar => {
                        let d = self.demote(*v);
                        let s = self.f32v(VirOp::SqrtF32(d));
                        let one = self.f32v(VirOp::ConstF32(1.0));
                        let r = self.f32v(VirOp::DivF32(one, s));
                        Some(self.promote(r))
                    }
                    CtType::Vec(w) => {
                        let s = self.emit(VirOp::SqrtV(v.reg), VirTy::V);
                        let one = self.f32v(VirOp::ConstF32(1.0));
                        let ones = self.emit(VirOp::Splat(one), VirTy::V);
                        Some(self.vec(VirOp::DivV(ones, s), w))
                    }
                    CtType::Bool => None,
                }
            }
            Intrinsic::Scale(factor) => {
                let [v] = args else { reject!() };
                let c = self.f32v(VirOp::ConstF32(factor));
                match v.ty {
                    CtType::Scalar => {
                        let d = self.demote(*v);
                        let r = self.f32v(VirOp::MulF32(d, c));
                        Some(self.promote(r))
                    }
                    CtType::Vec(w) => {
                        let cs = self.emit(VirOp::Splat(c), VirTy::V);
                        Some(self.vec(VirOp::MulV(v.reg, cs), w))
                    }
                    CtType::Bool => None,
                }
            }
            Intrinsic::Bin(fun) => {
                let [a, b] = args else { reject!() };
                self.zip_bin(
                    *a,
                    *b,
                    |x, y| VirOp::Math2F32(fun, x, y),
                    |x, y| VirOp::Math2V(fun, x, y),
                )
            }
            Intrinsic::Step => {
                // step(edge, x): per lane `if x < edge { 0.0 } else { 1.0 }`
                // (identical for the scalar-edge and zip paths).
                let [edge, x] = args else { reject!() };
                match (edge.ty, x.ty) {
                    (CtType::Scalar, CtType::Scalar) => {
                        let de = self.demote(*edge);
                        let dx = self.demote(*x);
                        let lt = self.emit(VirOp::CmpF32(CmpCc::Lt, dx, de), VirTy::Bool);
                        let zero = self.const_f64(0.0);
                        let one = self.const_f64(1.0);
                        Some(self.scalar(VirOp::SelF64(lt, zero.reg, one.reg)))
                    }
                    _ => {
                        let w = match (edge.ty, x.ty) {
                            (CtType::Vec(w), _) | (_, CtType::Vec(w)) => w,
                            _ => reject!(),
                        };
                        let ev = match edge.ty {
                            CtType::Vec(_) => edge.reg,
                            CtType::Scalar => self.splat_scalar(*edge),
                            CtType::Bool => reject!(),
                        };
                        let xv = match x.ty {
                            CtType::Vec(_) => x.reg,
                            CtType::Scalar => self.splat_scalar(*x),
                            CtType::Bool => reject!(),
                        };
                        let lt = self.emit(VirOp::CmpV(CmpCc::Lt, xv, ev), VirTy::Mask);
                        let zeros = self.emit(VirOp::ZeroV, VirTy::V);
                        let one = self.f32v(VirOp::ConstF32(1.0));
                        let ones = self.emit(VirOp::Splat(one), VirTy::V);
                        Some(self.vec(VirOp::SelLanes(lt, zeros, ones), w))
                    }
                }
            }
            Intrinsic::Clamp => {
                // x.max(min).min(max), Rust lane semantics; broadcast
                // scalar bounds (identical to clamp_scalar and the zip
                // else-path).
                let [x, mn, mx] = args else { reject!() };
                let t = self.zip_bin(
                    *x,
                    *mn,
                    |a, b| VirOp::Math2F32(MathFn2::RMax, a, b),
                    |a, b| VirOp::Math2V(MathFn2::RMax, a, b),
                )?;
                self.zip_bin(
                    t,
                    *mx,
                    |a, b| VirOp::Math2F32(MathFn2::RMin, a, b),
                    |a, b| VirOp::Math2V(MathFn2::RMin, a, b),
                )
            }
            Intrinsic::Mix | Intrinsic::Lerp => {
                let [x, y, a] = args else { reject!() };
                match a.ty {
                    CtType::Scalar => self.mix_scalar(*x, *y, *a),
                    CtType::Vec(_) if intr == Intrinsic::Mix => {
                        // Component-wise: x*(1-a) + y*a per lane.
                        let (CtType::Vec(wx), CtType::Vec(wy), CtType::Vec(wa)) =
                            (x.ty, y.ty, a.ty)
                        else {
                            reject!();
                        };
                        if wx != wy || wx != wa {
                            reject!();
                        }
                        let one = self.f32v(VirOp::ConstF32(1.0));
                        let ones = self.emit(VirOp::Splat(one), VirTy::V);
                        let om = self.emit(VirOp::SubV(ones, a.reg), VirTy::V);
                        let xs = self.emit(VirOp::MulV(x.reg, om), VirTy::V);
                        let ys = self.emit(VirOp::MulV(y.reg, a.reg), VirTy::V);
                        Some(self.vec(VirOp::AddV(xs, ys), wx))
                    }
                    _ => reject!(),
                }
            }
            Intrinsic::Smoothstep => {
                // Scalar edges: t = clamp01((x-e0)/(e1-e0)); t*t*(3-2t),
                // all in f32.
                let [e0, e1, x] = args else { reject!() };
                if e0.ty != CtType::Scalar || e1.ty != CtType::Scalar {
                    reject!();
                }
                let de0 = self.demote(*e0);
                let de1 = self.demote(*e1);
                match x.ty {
                    CtType::Scalar => {
                        let dx = self.demote(*x);
                        let num = self.f32v(VirOp::SubF32(dx, de0));
                        let den = self.f32v(VirOp::SubF32(de1, de0));
                        let q = self.f32v(VirOp::DivF32(num, den));
                        let zero = self.f32v(VirOp::ConstF32(0.0));
                        let one = self.f32v(VirOp::ConstF32(1.0));
                        let t0 = self.f32v(VirOp::Math2F32(MathFn2::RMax, q, zero));
                        let t = self.f32v(VirOp::Math2F32(MathFn2::RMin, t0, one));
                        let tt = self.f32v(VirOp::MulF32(t, t));
                        let three = self.f32v(VirOp::ConstF32(3.0));
                        let two = self.f32v(VirOp::ConstF32(2.0));
                        let tt2 = self.f32v(VirOp::MulF32(two, t));
                        let inner = self.f32v(VirOp::SubF32(three, tt2));
                        let r = self.f32v(VirOp::MulF32(tt, inner));
                        Some(self.promote(r))
                    }
                    CtType::Vec(w) => {
                        let e0s = self.emit(VirOp::Splat(de0), VirTy::V);
                        let num = self.emit(VirOp::SubV(x.reg, e0s), VirTy::V);
                        let den = self.f32v(VirOp::SubF32(de1, de0));
                        let dens = self.emit(VirOp::Splat(den), VirTy::V);
                        let q = self.emit(VirOp::DivV(num, dens), VirTy::V);
                        let zeros = self.emit(VirOp::ZeroV, VirTy::V);
                        let one = self.f32v(VirOp::ConstF32(1.0));
                        let ones = self.emit(VirOp::Splat(one), VirTy::V);
                        let t0 = self.emit(VirOp::Math2V(MathFn2::RMax, q, zeros), VirTy::V);
                        let t = self.emit(VirOp::Math2V(MathFn2::RMin, t0, ones), VirTy::V);
                        let tt = self.emit(VirOp::MulV(t, t), VirTy::V);
                        let three = self.f32v(VirOp::ConstF32(3.0));
                        let threes = self.emit(VirOp::Splat(three), VirTy::V);
                        let two = self.f32v(VirOp::ConstF32(2.0));
                        let twos = self.emit(VirOp::Splat(two), VirTy::V);
                        let tt2 = self.emit(VirOp::MulV(twos, t), VirTy::V);
                        let inner = self.emit(VirOp::SubV(threes, tt2), VirTy::V);
                        Some(self.vec(VirOp::MulV(tt, inner), w))
                    }
                    CtType::Bool => None,
                }
            }
            Intrinsic::Length => {
                let [v] = args else { reject!() };
                match v.ty {
                    // NumericValue::length on a scalar: |v| in f64.
                    CtType::Scalar => Some(self.scalar(VirOp::AbsF64(v.reg))),
                    CtType::Vec(w) => {
                        let len = self.f32v(VirOp::Length { a: v.reg, w });
                        Some(self.promote(len))
                    }
                    CtType::Bool => None,
                }
            }
            Intrinsic::Dot => {
                let [a, b] = args else { reject!() };
                match (a.ty, b.ty) {
                    // Scalar dot: a * b in f64.
                    (CtType::Scalar, CtType::Scalar) => {
                        Some(self.scalar(VirOp::MulF64(a.reg, b.reg)))
                    }
                    (CtType::Vec(wa), CtType::Vec(wb)) if wa == wb => {
                        let d = self.f32v(VirOp::Dot {
                            a: a.reg,
                            b: b.reg,
                            w: wa,
                        });
                        Some(self.promote(d))
                    }
                    _ => reject!(),
                }
            }
            Intrinsic::Distance => {
                let [a, b] = args else { reject!() };
                // diff = zip_f32 sub, then length(diff).
                let diff = self.zip_bin(
                    *a,
                    *b,
                    VirOp::SubF32,
                    VirOp::SubV,
                )?;
                match diff.ty {
                    CtType::Scalar => Some(self.scalar(VirOp::AbsF64(diff.reg))),
                    CtType::Vec(w) => {
                        let len = self.f32v(VirOp::Length { a: diff.reg, w });
                        Some(self.promote(len))
                    }
                    CtType::Bool => None,
                }
            }
            Intrinsic::Normalize => {
                let [v] = args else { reject!() };
                match v.ty {
                    CtType::Scalar => {
                        // len = |v| as f32; len == 0 -> v, else +-1.0.
                        let abs = self.scalar(VirOp::AbsF64(v.reg));
                        let len = self.demote(abs);
                        let zero32 = self.f32v(VirOp::ConstF32(0.0));
                        let is_zero =
                            self.emit(VirOp::CmpF32(CmpCc::Eq, len, zero32), VirTy::Bool);
                        let zero = self.const_f64(0.0);
                        let pos =
                            self.emit(VirOp::CmpF64(CmpCc::Ge, v.reg, zero.reg), VirTy::Bool);
                        let one = self.const_f64(1.0);
                        let neg_one = self.const_f64(-1.0);
                        let sign = self.emit(VirOp::SelF64(pos, one.reg, neg_one.reg), VirTy::F64);
                        Some(self.scalar(VirOp::SelF64(is_zero, v.reg, sign)))
                    }
                    CtType::Vec(w) => Some(self.vec(VirOp::Normalize { a: v.reg, w }, w)),
                    CtType::Bool => None,
                }
            }
            Intrinsic::Cross => {
                let [a, b] = args else { reject!() };
                let (CtType::Vec(3), CtType::Vec(3)) = (a.ty, b.ty) else {
                    reject!();
                };
                Some(self.vec(VirOp::Cross { a: a.reg, b: b.reg }, 3))
            }
        }
    }

    /// mix_scalar: a32 = alpha as f32; per lane x*(1-a32) + y*a32
    /// (both-scalar rounds through f32).
    fn mix_scalar(&mut self, x: CtVal, y: CtVal, alpha: CtVal) -> Option<CtVal> {
        let a = self.demote(alpha);
        let one = self.f32v(VirOp::ConstF32(1.0));
        let om = self.f32v(VirOp::SubF32(one, a));
        match (x.ty, y.ty) {
            (CtType::Scalar, CtType::Scalar) => {
                let dx = self.demote(x);
                let dy = self.demote(y);
                let xs = self.f32v(VirOp::MulF32(dx, om));
                let ys = self.f32v(VirOp::MulF32(dy, a));
                let r = self.f32v(VirOp::AddF32(xs, ys));
                Some(self.promote(r))
            }
            (CtType::Vec(wx), CtType::Vec(wy)) if wx == wy => {
                let oms = self.emit(VirOp::Splat(om), VirTy::V);
                let as_ = self.emit(VirOp::Splat(a), VirTy::V);
                let xs = self.emit(VirOp::MulV(x.reg, oms), VirTy::V);
                let ys = self.emit(VirOp::MulV(y.reg, as_), VirTy::V);
                Some(self.vec(VirOp::AddV(xs, ys), wx))
            }
            _ => None,
        }
    }
}

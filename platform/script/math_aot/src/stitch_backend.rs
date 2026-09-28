//! StitchBackend: lowers VIR to a makepad-stitch Wasm module using the
//! spec SIMD (v128/f32x4) subset plus stitch's nonstandard float math
//! opcodes (`Extensions::ext_math`).
//!
//! This backend is the cross-platform BIT-REFERENCE: for everything the
//! translator accepts, its results are bit-identical to the splash
//! interpreter (the codegen backends are held to a ULP contract against
//! it instead).
//!
//! Lowering is deliberately naive: every VIR register becomes one wasm
//! local; each op reads its operand locals and writes its destination
//! local. stitch's own threaded-code compiler then does the operand
//! stack/register fusion. Two functions are emitted per VirFn:
//!
//! - `eval1(params...) -> f64` — flattened parameters (f64 per scalar,
//!   N f32s per vecN), full-precision result.
//! - `evaln(in_ptr, out_ptr, count)` — the batch entry: the point loop
//!   lives INSIDE the wasm function, reading each point's lanes from
//!   linear memory and storing one f32 result, so per-point cost is pure
//!   threaded-code dispatch with no host boundary.

use super::vir::{CmpCc, MathFn, MathFn2, VirFn, VirOp, VirReg, VirTy};
use super::{BackendUnsupported, CompiledMath, MathAotValue, MathBackend};
use makepad_stitch as stitch;
use std::sync::Mutex;

// =========================================================================
// Wasm emission helpers
// =========================================================================

mod wasm {
    pub const I32: u8 = 0x7F;
    pub const F32: u8 = 0x7D;
    pub const F64: u8 = 0x7C;
    pub const V128: u8 = 0x7B;

    pub const LOCAL_GET: u8 = 0x20;
    pub const LOCAL_SET: u8 = 0x21;
    pub const I32_CONST: u8 = 0x41;
    pub const F32_CONST: u8 = 0x43;
    pub const F64_CONST: u8 = 0x44;

    pub const BLOCK: u8 = 0x02;
    pub const LOOP: u8 = 0x03;
    pub const IF: u8 = 0x04;
    pub const ELSE: u8 = 0x05;
    pub const END: u8 = 0x0B;
    pub const BR: u8 = 0x0C;
    pub const BR_IF: u8 = 0x0D;
    pub const SELECT: u8 = 0x1B;
    pub const VOID_BLOCK: u8 = 0x40;

    pub const I32_EQZ: u8 = 0x45;
    pub const I32_GE_U: u8 = 0x4F;
    pub const I32_ADD: u8 = 0x6A;
    pub const I32_MUL: u8 = 0x6C;

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

    pub const F32_ABS: u8 = 0x8B;
    pub const F32_CEIL: u8 = 0x8D;
    pub const F32_FLOOR: u8 = 0x8E;
    pub const F32_TRUNC: u8 = 0x8F;
    pub const F32_SQRT: u8 = 0x91;
    pub const F32_ADD: u8 = 0x92;
    pub const F32_SUB: u8 = 0x93;
    pub const F32_MUL: u8 = 0x94;
    pub const F32_DIV: u8 = 0x95;

    pub const F64_ABS: u8 = 0x99;
    pub const F64_NEG: u8 = 0x9A;
    pub const F64_ADD: u8 = 0xA0;
    pub const F64_SUB: u8 = 0xA1;
    pub const F64_MUL: u8 = 0xA2;
    pub const F64_DIV: u8 = 0xA3;

    pub const F32_DEMOTE_F64: u8 = 0xB6;
    pub const F64_CONVERT_I32_U: u8 = 0xB8;
    pub const F64_PROMOTE_F32: u8 = 0xBB;

    pub const F32_LOAD: u8 = 0x2A;
    pub const F32_STORE: u8 = 0x38;

    pub const SIMD: u8 = 0xFD;
    pub const S_V128_LOAD: u8 = 0;
    pub const S_V128_CONST: u8 = 12;
    pub const S_I8X16_SHUFFLE: u8 = 13;
    pub const S_F32X4_SPLAT: u8 = 19;
    pub const S_F32X4_EXTRACT_LANE: u8 = 31;
    pub const S_F32X4_REPLACE_LANE: u8 = 32;
    pub const S_F32X4_EQ: u8 = 65;
    pub const S_F32X4_NE: u8 = 66;
    pub const S_F32X4_LT: u8 = 67;
    pub const S_F32X4_GT: u8 = 68;
    pub const S_F32X4_LE: u8 = 69;
    pub const S_F32X4_GE: u8 = 70;
    pub const S_V128_BITSELECT: u8 = 82;
    pub const S_F32X4_CEIL: u8 = 103;
    pub const S_F32X4_FLOOR: u8 = 104;
    pub const S_F32X4_TRUNC: u8 = 105;
    pub const S_F32X4_ABS: u8 = 224;
    pub const S_F32X4_SQRT: u8 = 227;
    pub const S_F32X4_ADD: u8 = 228;
    pub const S_F32X4_SUB: u8 = 229;
    pub const S_F32X4_MUL: u8 = 230;
    pub const S_F32X4_DIV: u8 = 231;

    /// The nonstandard math opcode prefix (stitch `Extensions::ext_math`).
    pub const EXT: u8 = 0xE0;
    pub const X_SIN: u8 = 0x00;
    pub const X_COS: u8 = 0x01;
    pub const X_TAN: u8 = 0x02;
    pub const X_ASIN: u8 = 0x03;
    pub const X_ACOS: u8 = 0x04;
    pub const X_ATAN: u8 = 0x05;
    pub const X_EXP: u8 = 0x06;
    pub const X_LN: u8 = 0x07;
    pub const X_ATAN2: u8 = 0x08;
    pub const X_POW: u8 = 0x09;
    pub const X_RMIN: u8 = 0x0A;
    pub const X_RMAX: u8 = 0x0B;
    pub const X_REM: u8 = 0x0C;
    pub const X_DOT2: u8 = 0x2D;
    pub const PACKED_OFFSET: u8 = 0x20;

    pub fn leb_u32(mut val: u32, out: &mut Vec<u8>) {
        loop {
            let byte = (val & 0x7F) as u8;
            val >>= 7;
            if val == 0 {
                out.push(byte);
                break;
            }
            out.push(byte | 0x80);
        }
    }

    pub fn leb_i32(val: i32, out: &mut Vec<u8>) {
        let mut val = val as i64;
        loop {
            let byte = (val & 0x7F) as u8;
            val >>= 7;
            let done = (val == 0 && byte & 0x40 == 0) || (val == -1 && byte & 0x40 != 0);
            if done {
                out.push(byte);
                break;
            }
            out.push(byte | 0x80);
        }
    }

    pub fn section(id: u8, payload: &[u8], out: &mut Vec<u8>) {
        out.push(id);
        leb_u32(payload.len() as u32, out);
        out.extend_from_slice(payload);
    }
}

fn ty_byte(ty: VirTy) -> u8 {
    match ty {
        VirTy::F64 => wasm::F64,
        VirTy::F32 => wasm::F32,
        VirTy::V | VirTy::Mask => wasm::V128,
        VirTy::Bool => wasm::I32,
    }
}

fn math1_sub(f: MathFn) -> u8 {
    match f {
        MathFn::Sin => wasm::X_SIN,
        MathFn::Cos => wasm::X_COS,
        MathFn::Tan => wasm::X_TAN,
        MathFn::Asin => wasm::X_ASIN,
        MathFn::Acos => wasm::X_ACOS,
        MathFn::Atan => wasm::X_ATAN,
        MathFn::Exp => wasm::X_EXP,
        MathFn::Ln => wasm::X_LN,
    }
}

fn math2_sub(f: MathFn2) -> u8 {
    match f {
        MathFn2::Atan2 => wasm::X_ATAN2,
        MathFn2::Pow => wasm::X_POW,
        MathFn2::RMin => wasm::X_RMIN,
        MathFn2::RMax => wasm::X_RMAX,
        MathFn2::Rem => wasm::X_REM,
    }
}

fn cmp_f64(cc: CmpCc) -> u8 {
    match cc {
        CmpCc::Lt => wasm::F64_LT,
        CmpCc::Gt => wasm::F64_GT,
        CmpCc::Le => wasm::F64_LE,
        CmpCc::Ge => wasm::F64_GE,
        CmpCc::Eq => wasm::F64_EQ,
        CmpCc::Ne => wasm::F64_NE,
    }
}

fn cmp_f32(cc: CmpCc) -> u8 {
    match cc {
        CmpCc::Lt => wasm::F32_LT,
        CmpCc::Gt => wasm::F32_GT,
        CmpCc::Le => wasm::F32_LE,
        CmpCc::Ge => wasm::F32_GE,
        CmpCc::Eq => wasm::F32_EQ,
        CmpCc::Ne => wasm::F32_NE,
    }
}

fn cmp_v(cc: CmpCc) -> u8 {
    match cc {
        CmpCc::Lt => wasm::S_F32X4_LT,
        CmpCc::Gt => wasm::S_F32X4_GT,
        CmpCc::Le => wasm::S_F32X4_LE,
        CmpCc::Ge => wasm::S_F32X4_GE,
        CmpCc::Eq => wasm::S_F32X4_EQ,
        CmpCc::Ne => wasm::S_F32X4_NE,
    }
}

/// A wasm function body under construction.
struct FnBody {
    code: Vec<u8>,
    param_count: u32,
    locals: Vec<u8>,
}

impl FnBody {
    fn new(param_count: u32) -> Self {
        Self {
            code: Vec::new(),
            param_count,
            locals: Vec::new(),
        }
    }

    fn alloc_local(&mut self, ty_byte: u8) -> u32 {
        self.locals.push(ty_byte);
        self.param_count + self.locals.len() as u32 - 1
    }

    fn op(&mut self, op: u8) {
        self.code.push(op);
    }

    fn op_u32(&mut self, op: u8, val: u32) {
        self.code.push(op);
        wasm::leb_u32(val, &mut self.code);
    }

    fn get(&mut self, idx: u32) {
        self.op_u32(wasm::LOCAL_GET, idx);
    }

    fn set(&mut self, idx: u32) {
        self.op_u32(wasm::LOCAL_SET, idx);
    }

    fn f64_const(&mut self, val: f64) {
        self.op(wasm::F64_CONST);
        self.code.extend_from_slice(&val.to_le_bytes());
    }

    fn f32_const(&mut self, val: f32) {
        self.op(wasm::F32_CONST);
        self.code.extend_from_slice(&val.to_le_bytes());
    }

    fn i32_const(&mut self, val: i32) {
        self.op(wasm::I32_CONST);
        wasm::leb_i32(val, &mut self.code);
    }

    fn simd(&mut self, sub: u8) {
        self.op(wasm::SIMD);
        wasm::leb_u32(sub as u32, &mut self.code);
    }

    fn ext(&mut self, sub: u8) {
        self.op(wasm::EXT);
        self.code.push(sub);
    }

    fn v128_zero(&mut self) {
        self.simd(wasm::S_V128_CONST);
        self.code.extend_from_slice(&[0u8; 16]);
    }

    fn extract_lane(&mut self, lane: u8) {
        self.simd(wasm::S_F32X4_EXTRACT_LANE);
        self.code.push(lane);
    }

    fn replace_lane(&mut self, lane: u8) {
        self.simd(wasm::S_F32X4_REPLACE_LANE);
        self.code.push(lane);
    }

    /// `i8x16.shuffle` picking f32 lanes from a single source (both
    /// operands must already be on the stack).
    fn shuffle_f32_lanes(&mut self, lanes: &[u8; 4]) {
        self.simd(wasm::S_I8X16_SHUFFLE);
        let mut bytes = [0u8; 16];
        for (i, byte) in bytes.iter_mut().enumerate() {
            *byte = lanes[i / 4] * 4 + (i % 4) as u8;
        }
        self.code.extend_from_slice(&bytes);
    }
}

// =========================================================================
// The backend
// =========================================================================

pub struct StitchBackend {
    engine: stitch::Engine,
}

impl StitchBackend {
    pub fn new() -> StitchBackend {
        StitchBackend {
            engine: stitch::Engine::new_with_extensions(stitch::Extensions { ext_math: true }),
        }
    }
}

impl Default for StitchBackend {
    fn default() -> Self {
        Self::new()
    }
}

const OUT_OFFSET: usize = 65536;
/// Start of the read-only uniform block in linear memory (last KiB of the
/// module's 2 pages; the output region ends well below it).
const UNIFORM_OFFSET: usize = 130048;

impl MathBackend for StitchBackend {
    fn compile(&self, f: &VirFn) -> Result<Box<dyn CompiledMath>, BackendUnsupported> {
        if f.types.get(f.result.0 as usize) != Some(&VirTy::F64) {
            return Err(BackendUnsupported);
        }
        let stride = f.stride();
        if stride == 0 || stride * 4 >= OUT_OFFSET {
            return Err(BackendUnsupported);
        }
        // Uniform block (+ its 4-byte v128-load pad) must fit its region.
        let uniform_stride = f.uniform_stride();
        if UNIFORM_OFFSET + uniform_stride * 4 + 4 > 131072 {
            return Err(BackendUnsupported);
        }
        let module_bytes = assemble(f);
        let module =
            stitch::Module::new(&self.engine, &module_bytes).map_err(|_| BackendUnsupported)?;
        let mut store = stitch::Store::new(self.engine.clone());
        let instance = stitch::Linker::new()
            .instantiate(&mut store, &module)
            .map_err(|_| BackendUnsupported)?;
        let eval1 = instance.exported_func("eval1").ok_or(BackendUnsupported)?;
        let eval_n = instance.exported_func("evaln").ok_or(BackendUnsupported)?;
        let mem = instance.exported_mem("mem").ok_or(BackendUnsupported)?;
        // Points per chunk: input must fit below OUT_OFFSET (minus the
        // 4-byte v128 load pad), output in the second page span.
        let chunk = ((OUT_OFFSET - 4) / (stride * 4)).min(8192);
        Ok(Box::new(StitchCompiled {
            inner: Mutex::new(StitchInner {
                store,
                eval1,
                eval_n,
                mem,
            }),
            param_lanes: f.param_lanes.clone(),
            uniform_lanes: f.uniform_lanes.clone(),
            stride,
            uniform_stride,
            chunk,
        }))
    }
}

struct StitchInner {
    store: stitch::Store,
    eval1: stitch::Func,
    eval_n: stitch::Func,
    mem: stitch::Mem,
}

// SAFETY: a `StitchInner` is a self-contained unit: the func/mem handles
// point exclusively into the owned `store` (they were created from it and
// nothing else holds them), the store shares nothing thread-affine (the
// engine is Arc+Mutex, and stitch's execution stack is a per-thread
// thread_local acquired per call), and the `Mutex` around it serializes
// all access. Moving the whole unit between threads is therefore sound.
unsafe impl Send for StitchInner {}

struct StitchCompiled {
    inner: Mutex<StitchInner>,
    param_lanes: Vec<u8>,
    uniform_lanes: Vec<u8>,
    stride: usize,
    uniform_stride: usize,
    chunk: usize,
}

impl CompiledMath for StitchCompiled {
    fn eval_batch(&self, input: &[f32], uniforms: &[f32], out: &mut [f32]) {
        assert!(
            input.len() == out.len() * self.stride,
            "input has {} lanes, expected {} points x {} lanes",
            input.len(),
            out.len(),
            self.stride
        );
        assert!(
            uniforms.len() == self.uniform_stride,
            "uniform block has {} lanes, expected {}",
            uniforms.len(),
            self.uniform_stride
        );
        let inner = &mut *self.inner.lock().unwrap();
        // Upload the uniform block (constant across the chunks of this
        // invocation; each evaln call re-reads it into locals).
        {
            let bytes = inner.mem.bytes_mut(&mut inner.store);
            for (i, v) in uniforms.iter().enumerate() {
                let off = UNIFORM_OFFSET + i * 4;
                bytes[off..off + 4].copy_from_slice(&v.to_le_bytes());
            }
            let pad = UNIFORM_OFFSET + uniforms.len() * 4;
            bytes[pad..pad + 4].copy_from_slice(&[0; 4]);
        }
        let mut done = 0;
        while done < out.len() {
            let count = (out.len() - done).min(self.chunk);
            let bytes = inner.mem.bytes_mut(&mut inner.store);
            let in_lanes = count * self.stride;
            for (i, v) in input[done * self.stride..done * self.stride + in_lanes]
                .iter()
                .enumerate()
            {
                bytes[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
            }
            // Deterministic pad past the input so a full v128 load of the
            // final point stays in bounds and reads known bytes.
            bytes[in_lanes * 4..in_lanes * 4 + 4].copy_from_slice(&[0; 4]);
            let mut results: [stitch::Val; 0] = [];
            inner
                .eval_n
                .call(
                    &mut inner.store,
                    &[
                        stitch::Val::I32(0),
                        stitch::Val::I32(OUT_OFFSET as i32),
                        stitch::Val::I32(count as i32),
                    ],
                    &mut results,
                )
                .expect("compiled math expression trapped");
            let bytes = inner.mem.bytes(&inner.store);
            for i in 0..count {
                let off = OUT_OFFSET + i * 4;
                out[done + i] = f32::from_le_bytes(bytes[off..off + 4].try_into().unwrap());
            }
            done += count;
        }
    }

    fn call(&self, args: &[MathAotValue]) -> Option<f64> {
        let point_count = self.param_lanes.len();
        if args.len() != point_count + self.uniform_lanes.len() {
            return None;
        }
        let mut vals = Vec::new();
        for (arg, lanes) in args[..point_count].iter().zip(self.param_lanes.iter()) {
            match (arg, lanes) {
                (MathAotValue::Scalar(v), 1) => vals.push(stitch::Val::F64(*v)),
                (MathAotValue::Vec2(v), 2) => {
                    vals.extend(v.iter().map(|x| stitch::Val::F32(*x)))
                }
                (MathAotValue::Vec3(v), 3) => {
                    vals.extend(v.iter().map(|x| stitch::Val::F32(*x)))
                }
                (MathAotValue::Vec4(v), 4) => {
                    vals.extend(v.iter().map(|x| stitch::Val::F32(*x)))
                }
                _ => return None,
            }
        }
        // Uniforms travel as flattened f32 lanes.
        for (arg, lanes) in args[point_count..].iter().zip(self.uniform_lanes.iter()) {
            if arg.lanes() != *lanes as u32 {
                return None;
            }
            let mut flat = Vec::new();
            arg.push_lanes(&mut flat);
            vals.extend(flat.into_iter().map(stitch::Val::F32));
        }
        let inner = &mut *self.inner.lock().unwrap();
        let mut results = [stitch::Val::F64(0.0)];
        inner
            .eval1
            .call(&mut inner.store, &vals, &mut results)
            .ok()?;
        results[0].to_f64()
    }
}

// =========================================================================
// Lowering
// =========================================================================

/// How the parameters reach a lowered body.
enum ParamSource {
    /// eval1: flattened wasm function parameters.
    CallParams,
    /// evaln: loaded from linear memory at `addr_local` (per point).
    Memory { addr_local: u32 },
}

/// Stackifying lowering: VIR registers used exactly once are emitted
/// inline on the wasm operand stack at their use site (letting stitch's
/// own stack/register fusion collapse dispatches); multi-use registers
/// get a wasm local; unused registers are dropped. VIR ops are pure and
/// total, so this reordering cannot change any computed value.
struct Lower<'a> {
    f: &'a VirFn,
    body: FnBody,
    use_count: Vec<u32>,
    local: Vec<Option<u32>>,
}

impl<'a> Lower<'a> {
    fn new(f: &'a VirFn, body: FnBody) -> Self {
        let mut use_count = vec![0u32; f.ops.len()];
        let mut bump = |r: VirReg| use_count[r.0 as usize] += 1;
        for op in &f.ops {
            match *op {
                VirOp::Param { .. }
                | VirOp::Uniform { .. }
                | VirOp::ConstF64(_)
                | VirOp::ConstF32(_)
                | VirOp::ZeroV => {}
                VirOp::Demote(r)
                | VirOp::Promote(r)
                | VirOp::BoolToF64(r)
                | VirOp::Splat(r)
                | VirOp::ExtractLane(r, _)
                | VirOp::Shuffle(r, _)
                | VirOp::NegF64(r)
                | VirOp::AbsF64(r)
                | VirOp::SqrtF32(r)
                | VirOp::AbsF32(r)
                | VirOp::FloorF32(r)
                | VirOp::CeilF32(r)
                | VirOp::TruncF32(r)
                | VirOp::MathF32(_, r)
                | VirOp::SqrtV(r)
                | VirOp::AbsV(r)
                | VirOp::FloorV(r)
                | VirOp::CeilV(r)
                | VirOp::TruncV(r)
                | VirOp::MathV(_, r)
                | VirOp::BoolNot(r)
                | VirOp::Length { a: r, .. }
                | VirOp::Normalize { a: r, .. } => bump(r),
                VirOp::ReplaceLane(a, b, _)
                | VirOp::AddF64(a, b)
                | VirOp::SubF64(a, b)
                | VirOp::MulF64(a, b)
                | VirOp::DivF64(a, b)
                | VirOp::RemF64(a, b)
                | VirOp::AddF32(a, b)
                | VirOp::SubF32(a, b)
                | VirOp::MulF32(a, b)
                | VirOp::DivF32(a, b)
                | VirOp::Math2F32(_, a, b)
                | VirOp::AddV(a, b)
                | VirOp::SubV(a, b)
                | VirOp::MulV(a, b)
                | VirOp::DivV(a, b)
                | VirOp::Math2V(_, a, b)
                | VirOp::CmpF64(_, a, b)
                | VirOp::CmpF32(_, a, b)
                | VirOp::CmpV(_, a, b)
                | VirOp::Dot { a, b, .. }
                | VirOp::Cross { a, b } => {
                    bump(a);
                    bump(b);
                }
                VirOp::SelF64(c, a, b) | VirOp::SelV(c, a, b) | VirOp::SelLanes(c, a, b) => {
                    bump(c);
                    bump(a);
                    bump(b);
                }
            }
        }
        use_count[f.result.0 as usize] += 1;
        Lower {
            local: vec![None; f.ops.len()],
            f,
            body,
            use_count,
        }
    }

    /// Emits the value of `r` onto the wasm operand stack.
    fn value(&mut self, r: VirReg) {
        if let Some(local) = self.local[r.0 as usize] {
            self.body.get(local);
            return;
        }
        self.inline_op(r);
    }

    /// Emits the computation of `r` inline, leaving its value on the
    /// stack.
    fn inline_op(&mut self, r: VirReg) {
        match self.f.ops[r.0 as usize] {
            VirOp::Param { .. } | VirOp::Uniform { .. } => {
                unreachable!("params and uniforms always have locals")
            }
            VirOp::ConstF64(v) => self.body.f64_const(v),
            VirOp::ConstF32(v) => self.body.f32_const(v),
            VirOp::ZeroV => self.body.v128_zero(),
            VirOp::Demote(a) => {
                self.value(a);
                self.body.op(wasm::F32_DEMOTE_F64);
            }
            VirOp::Promote(a) => {
                self.value(a);
                self.body.op(wasm::F64_PROMOTE_F32);
            }
            VirOp::BoolToF64(a) => {
                self.value(a);
                self.body.op(wasm::F64_CONVERT_I32_U);
            }
            VirOp::Splat(a) => {
                self.value(a);
                self.body.simd(wasm::S_F32X4_SPLAT);
            }
            VirOp::ExtractLane(a, lane) => {
                self.value(a);
                self.body.extract_lane(lane);
            }
            VirOp::ReplaceLane(a, b, lane) => {
                self.value(a);
                self.value(b);
                self.body.replace_lane(lane);
            }
            VirOp::Shuffle(a, lanes) => {
                // The shuffle needs its source twice.
                let scratch = self.spill(a, wasm::V128);
                self.body.get(scratch);
                self.body.get(scratch);
                self.body.shuffle_f32_lanes(&lanes);
            }
            VirOp::AddF64(a, b) => self.bin(a, b, wasm::F64_ADD),
            VirOp::SubF64(a, b) => self.bin(a, b, wasm::F64_SUB),
            VirOp::MulF64(a, b) => self.bin(a, b, wasm::F64_MUL),
            VirOp::DivF64(a, b) => self.bin(a, b, wasm::F64_DIV),
            VirOp::NegF64(a) => {
                self.value(a);
                self.body.op(wasm::F64_NEG);
            }
            VirOp::AbsF64(a) => {
                self.value(a);
                self.body.op(wasm::F64_ABS);
            }
            VirOp::RemF64(a, b) => {
                self.value(a);
                self.value(b);
                self.body.ext(wasm::X_REM + 0x10);
            }
            VirOp::AddF32(a, b) => self.bin(a, b, wasm::F32_ADD),
            VirOp::SubF32(a, b) => self.bin(a, b, wasm::F32_SUB),
            VirOp::MulF32(a, b) => self.bin(a, b, wasm::F32_MUL),
            VirOp::DivF32(a, b) => self.bin(a, b, wasm::F32_DIV),
            VirOp::SqrtF32(a) => {
                self.value(a);
                self.body.op(wasm::F32_SQRT);
            }
            VirOp::AbsF32(a) => {
                self.value(a);
                self.body.op(wasm::F32_ABS);
            }
            VirOp::FloorF32(a) => {
                self.value(a);
                self.body.op(wasm::F32_FLOOR);
            }
            VirOp::CeilF32(a) => {
                self.value(a);
                self.body.op(wasm::F32_CEIL);
            }
            VirOp::TruncF32(a) => {
                self.value(a);
                self.body.op(wasm::F32_TRUNC);
            }
            VirOp::MathF32(fun, a) => {
                self.value(a);
                self.body.ext(math1_sub(fun));
            }
            VirOp::Math2F32(fun, a, b) => {
                self.value(a);
                self.value(b);
                self.body.ext(math2_sub(fun));
            }
            VirOp::AddV(a, b) => self.simd_bin(a, b, wasm::S_F32X4_ADD),
            VirOp::SubV(a, b) => self.simd_bin(a, b, wasm::S_F32X4_SUB),
            VirOp::MulV(a, b) => self.simd_bin(a, b, wasm::S_F32X4_MUL),
            VirOp::DivV(a, b) => self.simd_bin(a, b, wasm::S_F32X4_DIV),
            VirOp::SqrtV(a) => {
                self.value(a);
                self.body.simd(wasm::S_F32X4_SQRT);
            }
            VirOp::AbsV(a) => {
                self.value(a);
                self.body.simd(wasm::S_F32X4_ABS);
            }
            VirOp::FloorV(a) => {
                self.value(a);
                self.body.simd(wasm::S_F32X4_FLOOR);
            }
            VirOp::CeilV(a) => {
                self.value(a);
                self.body.simd(wasm::S_F32X4_CEIL);
            }
            VirOp::TruncV(a) => {
                self.value(a);
                self.body.simd(wasm::S_F32X4_TRUNC);
            }
            VirOp::MathV(fun, a) => {
                self.value(a);
                self.body.ext(math1_sub(fun) + wasm::PACKED_OFFSET);
            }
            VirOp::Math2V(fun, a, b) => {
                self.value(a);
                self.value(b);
                self.body.ext(math2_sub(fun) + wasm::PACKED_OFFSET);
            }
            VirOp::CmpF64(cc, a, b) => self.bin(a, b, cmp_f64(cc)),
            VirOp::CmpF32(cc, a, b) => self.bin(a, b, cmp_f32(cc)),
            VirOp::CmpV(cc, a, b) => self.simd_bin(a, b, cmp_v(cc)),
            VirOp::BoolNot(a) => {
                self.value(a);
                self.body.op(wasm::I32_EQZ);
            }
            VirOp::SelF64(c, a, b) | VirOp::SelV(c, a, b) => {
                self.value(a);
                self.value(b);
                self.value(c);
                self.body.op(wasm::SELECT);
            }
            VirOp::SelLanes(m, a, b) => {
                self.value(a);
                self.value(b);
                self.value(m);
                self.body.simd(wasm::S_V128_BITSELECT);
            }
            VirOp::Dot { a, b, w } => {
                // Single-dispatch packed reduction (dot2/dot3/dot4);
                // left-associated lane sum, bit-exact.
                self.value(a);
                self.value(b);
                self.body.ext(wasm::X_DOT2 + (w - 2));
            }
            VirOp::Length { a, w } => {
                let av = self.spill(a, wasm::V128);
                self.body.get(av);
                self.body.get(av);
                self.body.ext(wasm::X_DOT2 + (w - 2));
                self.body.op(wasm::F32_SQRT);
            }
            VirOp::Cross { a, b } => {
                // a.yzx * b.zxy - a.zxy * b.yzx
                let av = self.spill(a, wasm::V128);
                let bv = self.spill(b, wasm::V128);
                self.body.get(av);
                self.body.get(av);
                self.body.shuffle_f32_lanes(&[1, 2, 0, 3]);
                self.body.get(bv);
                self.body.get(bv);
                self.body.shuffle_f32_lanes(&[2, 0, 1, 3]);
                self.body.simd(wasm::S_F32X4_MUL);
                self.body.get(av);
                self.body.get(av);
                self.body.shuffle_f32_lanes(&[2, 0, 1, 3]);
                self.body.get(bv);
                self.body.get(bv);
                self.body.shuffle_f32_lanes(&[1, 2, 0, 3]);
                self.body.simd(wasm::S_F32X4_MUL);
                self.body.simd(wasm::S_F32X4_SUB);
            }
            VirOp::Normalize { a, w } => {
                // len = length(a); len == 0 ? a : a * splat(1/len)
                let av = self.spill(a, wasm::V128);
                let len = self.body.alloc_local(wasm::F32);
                self.body.get(av);
                self.body.get(av);
                self.body.ext(wasm::X_DOT2 + (w - 2));
                self.body.op(wasm::F32_SQRT);
                self.body.set(len);
                self.body.get(len);
                self.body.f32_const(0.0);
                self.body.op(wasm::F32_EQ);
                self.body.op(wasm::IF);
                self.body.op(wasm::V128);
                self.body.get(av);
                self.body.op(wasm::ELSE);
                self.body.get(av);
                self.body.f32_const(1.0);
                self.body.get(len);
                self.body.op(wasm::F32_DIV);
                self.body.simd(wasm::S_F32X4_SPLAT);
                self.body.simd(wasm::S_F32X4_MUL);
                self.body.op(wasm::END);
            }
        }
    }

    /// Materializes `r` in a local (reusing an existing one) and returns
    /// its index — for ops that read an operand more than once.
    fn spill(&mut self, r: VirReg, ty: u8) -> u32 {
        if let Some(local) = self.local[r.0 as usize] {
            return local;
        }
        self.inline_op(r);
        let local = self.body.alloc_local(ty);
        self.body.set(local);
        self.local[r.0 as usize] = Some(local);
        local
    }

    fn bin(&mut self, a: VirReg, b: VirReg, op: u8) {
        self.value(a);
        self.value(b);
        self.body.op(op);
    }

    fn simd_bin(&mut self, a: VirReg, b: VirReg, sub: u8) {
        self.value(a);
        self.value(b);
        self.body.simd(sub);
    }
}

/// Lowers the VIR ops into `body`. Returns the finished body with the
/// result value ON THE STACK.
///
/// For `ParamSource::CallParams`, `flat_params` gives the wasm parameter
/// index of each per-point parameter's first lane, and the uniform lanes
/// follow as further f32 wasm parameters starting at `uniform_flat_base`.
/// For `ParamSource::Memory`, points load per-iteration from `addr_local`
/// and uniforms load from the fixed UNIFORM_OFFSET block.
fn lower_ops(
    f: &VirFn,
    mut body: FnBody,
    source: &ParamSource,
    flat_params: &[u32],
    uniform_flat_base: u32,
) -> FnBody {
    // Materialize the parameters into locals first.
    let mut param_locals: Vec<u32> = Vec::new();
    for (index, lanes) in f.param_lanes.iter().enumerate() {
        match source {
            ParamSource::CallParams => {
                if *lanes == 1 {
                    param_locals.push(flat_params[index]);
                } else {
                    let dest = body.alloc_local(wasm::V128);
                    body.v128_zero();
                    for lane in 0..*lanes {
                        body.get(flat_params[index] + lane as u32);
                        body.replace_lane(lane);
                    }
                    body.set(dest);
                    param_locals.push(dest);
                }
            }
            ParamSource::Memory { addr_local } => {
                let lane_off: u32 = f.param_lanes[..index].iter().map(|l| *l as u32).sum();
                if *lanes == 1 {
                    let dest = body.alloc_local(wasm::F64);
                    body.get(*addr_local);
                    body.op(wasm::F32_LOAD);
                    wasm::leb_u32(2, &mut body.code);
                    wasm::leb_u32(lane_off * 4, &mut body.code);
                    body.op(wasm::F64_PROMOTE_F32);
                    body.set(dest);
                    param_locals.push(dest);
                } else {
                    // Full 16-byte load; lanes past the width are
                    // unobservable and the host pads the input region.
                    let dest = body.alloc_local(wasm::V128);
                    body.get(*addr_local);
                    body.simd(wasm::S_V128_LOAD);
                    wasm::leb_u32(2, &mut body.code);
                    wasm::leb_u32(lane_off * 4, &mut body.code);
                    body.set(dest);
                    param_locals.push(dest);
                }
            }
        }
    }
    // Materialize the uniforms into locals (batch-constant: loaded once
    // per invocation, outside the point loop).
    let mut uniform_locals: Vec<u32> = Vec::new();
    let mut flat = uniform_flat_base;
    for (index, lanes) in f.uniform_lanes.iter().enumerate() {
        let lane_off: u32 = f.uniform_lanes[..index].iter().map(|l| *l as u32).sum();
        match source {
            ParamSource::CallParams => {
                if *lanes == 1 {
                    let dest = body.alloc_local(wasm::F64);
                    body.get(flat);
                    body.op(wasm::F64_PROMOTE_F32);
                    body.set(dest);
                    uniform_locals.push(dest);
                } else {
                    let dest = body.alloc_local(wasm::V128);
                    body.v128_zero();
                    for lane in 0..*lanes {
                        body.get(flat + lane as u32);
                        body.replace_lane(lane);
                    }
                    body.set(dest);
                    uniform_locals.push(dest);
                }
                flat += *lanes as u32;
            }
            ParamSource::Memory { .. } => {
                if *lanes == 1 {
                    let dest = body.alloc_local(wasm::F64);
                    body.i32_const(0);
                    body.op(wasm::F32_LOAD);
                    wasm::leb_u32(2, &mut body.code);
                    wasm::leb_u32(UNIFORM_OFFSET as u32 + lane_off * 4, &mut body.code);
                    body.op(wasm::F64_PROMOTE_F32);
                    body.set(dest);
                    uniform_locals.push(dest);
                } else {
                    let dest = body.alloc_local(wasm::V128);
                    body.i32_const(0);
                    body.simd(wasm::S_V128_LOAD);
                    wasm::leb_u32(2, &mut body.code);
                    wasm::leb_u32(UNIFORM_OFFSET as u32 + lane_off * 4, &mut body.code);
                    body.set(dest);
                    uniform_locals.push(dest);
                }
            }
        }
    }
    let mut lower = Lower::new(f, body);
    for (i, op) in f.ops.iter().enumerate() {
        match op {
            VirOp::Param { index, .. } => {
                lower.local[i] = Some(param_locals[*index as usize]);
            }
            VirOp::Uniform { index, .. } => {
                lower.local[i] = Some(uniform_locals[*index as usize]);
            }
            _ => {}
        }
    }
    // Multi-use registers get locals, in definition order; single-use
    // registers inline at their use site; unused registers are dropped.
    for i in 0..f.ops.len() {
        if lower.local[i].is_some() || lower.use_count[i] < 2 {
            continue;
        }
        let ty = ty_byte(f.types[i]);
        let reg = VirReg(i as u32);
        lower.inline_op(reg);
        let local = lower.body.alloc_local(ty);
        lower.body.set(local);
        lower.local[i] = Some(local);
    }
    lower.value(f.result);
    lower.body
}

/// Builds `eval1` (per-point params, then uniform lanes, all as wasm
/// function parameters).
fn lower_call_fn(f: &VirFn) -> FnBody {
    let flat_count: u32 = f
        .param_lanes
        .iter()
        .map(|l| if *l == 1 { 1 } else { *l as u32 })
        .sum();
    let uniform_count = f.uniform_stride() as u32;
    let body = FnBody::new(flat_count + uniform_count);
    let mut flat_params = Vec::new();
    let mut idx = 0u32;
    for lanes in &f.param_lanes {
        flat_params.push(idx);
        idx += if *lanes == 1 { 1 } else { *lanes as u32 };
    }
    lower_ops(f, body, &ParamSource::CallParams, &flat_params, flat_count)
}

/// Builds `evaln(in_ptr, out_ptr, count)`.
fn lower_batch_fn(f: &VirFn) -> FnBody {
    let mut body = FnBody::new(3);
    let ptr_in = 0u32;
    let ptr_out = 1u32;
    let count = 2u32;
    let idx = body.alloc_local(wasm::I32);
    let addr = body.alloc_local(wasm::I32);
    let res = body.alloc_local(wasm::F32);
    let stride = f.stride() as u32;

    body.op(wasm::BLOCK);
    body.op(wasm::VOID_BLOCK);
    body.op(wasm::LOOP);
    body.op(wasm::VOID_BLOCK);
    body.get(idx);
    body.get(count);
    body.op(wasm::I32_GE_U);
    body.op_u32(wasm::BR_IF, 1);

    // addr = in_ptr + i * stride * 4
    body.get(ptr_in);
    body.get(idx);
    body.i32_const(stride as i32 * 4);
    body.op(wasm::I32_MUL);
    body.op(wasm::I32_ADD);
    body.set(addr);

    let mut body = lower_ops(f, body, &ParamSource::Memory { addr_local: addr }, &[], 0);

    // out[i] = result as f32 (the result value is on the stack)
    body.op(wasm::F32_DEMOTE_F64);
    body.set(res);
    body.get(ptr_out);
    body.get(idx);
    body.i32_const(4);
    body.op(wasm::I32_MUL);
    body.op(wasm::I32_ADD);
    body.get(res);
    body.op(wasm::F32_STORE);
    wasm::leb_u32(2, &mut body.code);
    wasm::leb_u32(0, &mut body.code);

    // i += 1; continue
    body.get(idx);
    body.i32_const(1);
    body.op(wasm::I32_ADD);
    body.set(idx);
    body.op_u32(wasm::BR, 0);
    body.op(wasm::END);
    body.op(wasm::END);
    body
}

/// Assembles the module: 2 pages of memory, `eval1`, `evaln`.
fn assemble(f: &VirFn) -> Vec<u8> {
    let call_fn = lower_call_fn(f);
    let batch_fn = lower_batch_fn(f);
    let mut out = vec![0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00];

    // Type section.
    let mut payload = Vec::new();
    wasm::leb_u32(2, &mut payload);
    payload.push(0x60);
    let mut flat: Vec<u8> = f
        .param_lanes
        .iter()
        .flat_map(|l| {
            if *l == 1 {
                vec![wasm::F64]
            } else {
                vec![wasm::F32; *l as usize]
            }
        })
        .collect();
    flat.extend(std::iter::repeat(wasm::F32).take(f.uniform_stride()));
    wasm::leb_u32(flat.len() as u32, &mut payload);
    payload.extend_from_slice(&flat);
    wasm::leb_u32(1, &mut payload);
    payload.push(wasm::F64);
    payload.push(0x60);
    wasm::leb_u32(3, &mut payload);
    payload.extend_from_slice(&[wasm::I32, wasm::I32, wasm::I32]);
    wasm::leb_u32(0, &mut payload);
    wasm::section(1, &payload, &mut out);

    // Function section.
    wasm::section(3, &[2, 0, 1], &mut out);

    // Memory section: fixed 2 pages.
    wasm::section(5, &[1, 0x01, 2, 2], &mut out);

    // Export section.
    let mut payload = Vec::new();
    wasm::leb_u32(3, &mut payload);
    for (name, kind, idx) in [("eval1", 0u8, 0u8), ("evaln", 0, 1), ("mem", 2, 0)] {
        wasm::leb_u32(name.len() as u32, &mut payload);
        payload.extend_from_slice(name.as_bytes());
        payload.push(kind);
        wasm::leb_u32(idx as u32, &mut payload);
    }
    wasm::section(7, &payload, &mut out);

    // Code section.
    let mut payload = Vec::new();
    wasm::leb_u32(2, &mut payload);
    for func in [&call_fn, &batch_fn] {
        let mut code = Vec::new();
        wasm::leb_u32(func.locals.len() as u32, &mut code);
        for local in &func.locals {
            wasm::leb_u32(1, &mut code);
            code.push(*local);
        }
        code.extend_from_slice(&func.code);
        code.push(wasm::END);
        wasm::leb_u32(code.len() as u32, &mut payload);
        payload.extend_from_slice(&code);
    }
    wasm::section(10, &payload, &mut out);

    out
}

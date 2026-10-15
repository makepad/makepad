//! The shader IR: what the shader compiler lowers a Splash shader's bytecode
//! to, once, and what every backend prints (SPIR-V, WGSL, GLSL, HLSL,
//! Metal, Rust). Typed and structured: every expression knows its exact
//! type (abstract numbers are resolved during lowering), control flow is
//! if / for / loop with break and continue, and IO is named globals whose
//! kind says what they are (an instance field, a uniform, a varying ...).
//!
//! The design is in local/plans/shader-ir.md.
use makepad_relax_api::relax_api;

use crate::shader_ids::*;
use crate::shader_types::ShaderTypes;
use crate::shader_schema::{ShaderTypeKind, ShaderTypeInline};
use crate::shader_output::TextureType;

use makepad_live_id::LiveId;
use std::collections::HashMap;

pub type TyId = u32;
pub type ExprId = u32;
pub type GlobalId = u32;
pub type FuncId = u32;
pub type LocalId = u32;

/// No expression: a stack value with no IR (a type name, `self`, a scope
/// object ...).
pub const NO_EXPR: ExprId = u32::MAX;

#[relax_api(plain)]
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum IrScalar {
    Bool,
    F32,
    F16,
    I32,
    U32,
}

impl IrScalar {
    pub fn is_float(self) -> bool {
        matches!(self, IrScalar::F32 | IrScalar::F16)
    }
    pub fn is_int(self) -> bool {
        matches!(self, IrScalar::I32 | IrScalar::U32)
    }
    pub fn width(self) -> u32 {
        if self == IrScalar::F16 {
            2
        } else {
            4
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug, Hash)]
pub enum IrTy {
    Void,
    Scalar(IrScalar),
    /// A literal (or a fold of literals) not yet given a type.
    AbstractInt,
    AbstractFloat,
    Vec(u8, IrScalar),
    /// Columns, rows; f32 elements.
    Mat(u8, u8),
    /// A compute-only matrix distributed over a subgroup, with opaque
    /// per-invocation storage. It has no host-shareable layout.
    CooperativeMatrix(CooperativeMatrix),
    /// Element and length; 0 is a runtime-sized array.
    Array(TyId, u32),
    Struct(u32),
    Texture(TextureType),
    /// A comparison sampler when true.
    Sampler(bool),
}

#[relax_api(plain)]
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum CooperativeScalar {
    F16,
    F32,
    I8,
    U8,
    I32,
    U32,
}

impl CooperativeScalar {
    pub fn width(self) -> u32 {
        match self {
            Self::I8 | Self::U8 => 1,
            Self::F16 => 2,
            _ => 4,
        }
    }
    pub fn is_float(self) -> bool {
        matches!(self, Self::F16 | Self::F32)
    }
    pub fn is_signed(self) -> bool {
        matches!(self, Self::I8 | Self::I32)
    }
}

#[relax_api(plain)]
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum CooperativeUse {
    A,
    B,
    Accumulator,
}

#[relax_api(plain)]
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub struct CooperativeMatrix {
    pub scalar: CooperativeScalar,
    pub rows: u32,
    pub cols: u32,
    pub usage: CooperativeUse,
}

#[relax_api(plain)]
#[derive(Clone, Debug)]
pub struct IrField {
    pub name: LiveId,
    pub ty: TyId,
}

#[derive(Clone, Debug)]
pub struct IrStruct {
    pub name: LiveId,
    /// The name of a struct the compiler makes (no pod type).
    pub synth: Option<&'static str>,
    pub pod: ShaderTypeId,
    pub fields: Vec<IrField>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum IrIo {
    /// The geometry record (a struct of the vertex buffer's fields).
    Geometry,
    DynInstance,
    RustInstance,
    Uniform,
    ScopeUniform,
    UniformBuffer,
    Varying,
    FragmentOutput(u8),
    Texture(TextureType),
    /// A compute kernel's device buffer (`shader.buffer` / `buffer_rw`): a
    /// runtime-sized array reached by its device address (writable when
    /// true).
    Storage(bool),
    /// A compute kernel's workgroup-shared array (`shader.workgroup`).
    Workgroup,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum IrGlobalKind {
    Io(IrIo),
    /// `self.vertex_pos`: the clip position the vertex stage writes.
    VertexPosition,
    /// `_mp_iter`: the invocation's shared loop-pass counter
    /// (shader_lower_control::SHADER_ITERATION_BUDGET).
    IterCounter,
    /// The XR view index (0 outside XR).
    ViewId,
    /// The instance-record storage buffer (`array<u32>`), entry code only.
    InstanceBuffer,
    /// The XR depth texture `depth_clip` reads.
    XrDepth,
    /// A sampler of `ShaderOutput::samplers`.
    Sampler(u32),
    /// The vertex instance index (`InstanceIndex` reads it), set by the
    /// entry code.
    InstanceIndex,
    /// The uniform block holding the dyn uniforms / the scope uniforms
    /// (entry code copies them into their globals).
    DynUniformBlock,
    ScopeUniformBlock,
    /// GL: the host's clip-space Y flip for bottom-up render targets.
    ClipYFlip,
    /// HLSL: the draw's first instance. `SV_InstanceID` starts at zero for every draw, so the
    /// host passes the first instance (`StartInstanceLocation` is not seen by the shader) in a
    /// constant buffer at b13; an unbound buffer reads zero.
    InstanceBase,
}

#[derive(Clone, Debug)]
pub struct IrGlobal {
    pub kind: IrGlobalKind,
    pub name: LiveId,
    pub ty: TyId,
    /// Descriptor binding (set 0), assigned by the entry builder.
    pub binding: Option<u32>,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum IrLit {
    Bool(bool),
    F32(f32),
    F16(f32),
    I32(i32),
    U32(u32),
    AbstractInt(i64),
    AbstractFloat(f64),
}

#[relax_api(plain)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum UnOp {
    Neg,
    Not,
    BitNot,
}

#[relax_api(plain)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Shl,
    Shr,
    BitAnd,
    BitOr,
    BitXor,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    LogicAnd,
    LogicOr,
}

impl BinOp {
    pub fn is_compare(self) -> bool {
        matches!(self, BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge)
    }
    pub fn text(self) -> &'static str {
        match self {
            BinOp::Add => "+",
            BinOp::Sub => "-",
            BinOp::Mul => "*",
            BinOp::Div => "/",
            BinOp::Rem => "%",
            BinOp::Shl => "<<",
            BinOp::Shr => ">>",
            BinOp::BitAnd => "&",
            BinOp::BitOr => "|",
            BinOp::BitXor => "^",
            BinOp::Eq => "==",
            BinOp::Ne => "!=",
            BinOp::Lt => "<",
            BinOp::Le => "<=",
            BinOp::Gt => ">",
            BinOp::Ge => ">=",
            BinOp::LogicAnd => "&&",
            BinOp::LogicOr => "||",
        }
    }
}

/// The Splash shader builtins (shader_builtins.rs) and the few helpers the
/// entry code uses.
#[relax_api(plain)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Builtin {
    Abs,
    Acos,
    Acosh,
    Asin,
    Asinh,
    Atan,
    Atanh,
    Atan2,
    Ceil,
    Clamp,
    Cos,
    Cosh,
    Cross,
    Degrees,
    Distance,
    Dot,
    Exp,
    Exp2,
    Floor,
    Fma,
    Fract,
    Inverse,
    InverseSqrt,
    Length,
    Log,
    Log2,
    Max,
    Min,
    Mix,
    /// Splash's `modf(x, y)`: the float remainder (`x % y`).
    FMod,
    Normalize,
    Pow,
    Radians,
    Round,
    Sign,
    Sin,
    Sinh,
    Smoothstep,
    Sqrt,
    Step,
    Tan,
    Tanh,
    Trunc,
    Dpdx,
    Dpdy,
    /// One f32 carrying two f16s / four unorm8 (packed vertex formats).
    Unpack2f16,
    Unpack4u8,
    Unpack2x16Float,
    Unpack2x16Unorm,
    Unpack2x16Snorm,
    Unpack4x8Unorm,
    Unpack4x8Snorm,
    // ---- compute kernels (shader_ir_compute.rs) ----
    /// `global_id()` / `local_id()` / `group_id()` / `group_count()`: vec3u.
    GlobalId,
    LocalId,
    GroupId,
    GroupCount,
    /// `local_index()`: the flattened local id.
    LocalIndex,
    /// `subgroup_size()`, `subgroup_lane()` (the invocation's lane),
    /// `subgroup_id()` (the subgroup's index in the workgroup): u32.
    SubgroupSize,
    SubgroupLane,
    SubgroupId,
    /// `barrier()`: a workgroup execution and workgroup-memory barrier.
    Barrier,
    /// `subgroup_add/min/max(x)`: the reduction over the subgroup.
    SubgroupAdd,
    SubgroupMin,
    SubgroupMax,
    /// `subgroup_shuffle(x, lane)` / `subgroup_shuffle_xor(x, mask)`.
    SubgroupShuffle,
    SubgroupShuffleXor,
    /// `subgroup_first(x)`: x of the lowest active lane.
    SubgroupFirst,
    /// `pack_half2(v: vec2f) -> u32`.
    PackHalf2,
    /// `extract_bits(x, offset, count)`: unsigned bit field.
    ExtractBits,
    /// `dot4_u8(a, b)`: the dot product of two words of four packed
    /// unsigned bytes, as u32.
    Dot4U8,
    /// `atomic_add(buffer[i], v)`: returns the old value (u32 / i32).
    AtomicAdd,
    /// Compute-only operations; load/store's first argument is a place.
    CooperativeLoad,
    CooperativeStore,
    CooperativeMulAdd,
    CooperativeScale,
}

impl Builtin {
    pub fn from_name(name: LiveId) -> Option<Builtin> {
        use makepad_live_id::id;
        Some(match name {
            id!(abs) => Builtin::Abs,
            id!(acos) => Builtin::Acos,
            id!(acosh) => Builtin::Acosh,
            id!(asin) => Builtin::Asin,
            id!(asinh) => Builtin::Asinh,
            id!(atan) => Builtin::Atan,
            id!(atanh) => Builtin::Atanh,
            id!(atan2) => Builtin::Atan2,
            id!(ceil) => Builtin::Ceil,
            id!(clamp) => Builtin::Clamp,
            id!(cos) => Builtin::Cos,
            id!(cosh) => Builtin::Cosh,
            id!(cross) => Builtin::Cross,
            id!(degrees) => Builtin::Degrees,
            id!(distance) => Builtin::Distance,
            id!(dot) => Builtin::Dot,
            id!(exp) => Builtin::Exp,
            id!(exp2) => Builtin::Exp2,
            id!(floor) => Builtin::Floor,
            id!(fma) => Builtin::Fma,
            id!(fract) => Builtin::Fract,
            id!(inverse) => Builtin::Inverse,
            id!(inverseSqrt) => Builtin::InverseSqrt,
            id!(length) => Builtin::Length,
            id!(log) => Builtin::Log,
            id!(log2) => Builtin::Log2,
            id!(max) => Builtin::Max,
            id!(min) => Builtin::Min,
            id!(mix) => Builtin::Mix,
            id!(modf) => Builtin::FMod,
            id!(normalize) => Builtin::Normalize,
            id!(pow) => Builtin::Pow,
            id!(radians) => Builtin::Radians,
            id!(round) => Builtin::Round,
            id!(sign) => Builtin::Sign,
            id!(sin) => Builtin::Sin,
            id!(sinh) => Builtin::Sinh,
            id!(smoothstep) => Builtin::Smoothstep,
            id!(sqrt) => Builtin::Sqrt,
            id!(step) => Builtin::Step,
            id!(tan) => Builtin::Tan,
            id!(tanh) => Builtin::Tanh,
            id!(trunc) => Builtin::Trunc,
            id!(dFdx) => Builtin::Dpdx,
            id!(dFdy) => Builtin::Dpdy,
            id!(unpack2f16) => Builtin::Unpack2f16,
            id!(unpack4u8) => Builtin::Unpack4u8,
            id!(unpack_half2) => Builtin::Unpack2x16Float,
            id!(global_id) => Builtin::GlobalId,
            id!(local_id) => Builtin::LocalId,
            id!(group_id) => Builtin::GroupId,
            id!(group_count) => Builtin::GroupCount,
            id!(local_index) => Builtin::LocalIndex,
            id!(subgroup_size) => Builtin::SubgroupSize,
            id!(subgroup_lane) => Builtin::SubgroupLane,
            id!(subgroup_id) => Builtin::SubgroupId,
            id!(barrier) => Builtin::Barrier,
            id!(subgroup_add) => Builtin::SubgroupAdd,
            id!(subgroup_min) => Builtin::SubgroupMin,
            id!(subgroup_max) => Builtin::SubgroupMax,
            id!(subgroup_shuffle) => Builtin::SubgroupShuffle,
            id!(subgroup_shuffle_xor) => Builtin::SubgroupShuffleXor,
            id!(subgroup_first) => Builtin::SubgroupFirst,
            id!(pack_half2) => Builtin::PackHalf2,
            id!(extract_bits) => Builtin::ExtractBits,
            id!(dot4_u8) => Builtin::Dot4U8,
            id!(atomic_add) => Builtin::AtomicAdd,
            id!(coopmat_load) => Builtin::CooperativeLoad,
            id!(coopmat_store) => Builtin::CooperativeStore,
            id!(coopmat_mul_add) => Builtin::CooperativeMulAdd,
            id!(coopmat_scale) => Builtin::CooperativeScale,
            _ => return None,
        })
    }
}

#[relax_api(plain)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PassMatrix {
    CameraProjection,
    CameraView,
    DepthProjection,
    DepthView,
    CameraInv,
}

#[relax_api(plain)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SampleKind {
    /// Implicit lod (`sample`, `sample_repeat`, `sample_nearest` without a
    /// lod).
    Implicit,
    /// Explicit lod in `lod`.
    Lod,
    /// Explicit gradients in `dx`, `dy`.
    Grad,
    /// A depth comparison at lod 0 against `reference`.
    CompareLevel0,
    /// A video (external) texture.
    Video,
}

#[relax_api(plain)]
#[derive(Clone, Debug)]
pub struct IrSample {
    pub tex: ExprId,
    pub tex_type: TextureType,
    pub sampler: u32,
    pub kind: SampleKind,
    /// `sample_nearest` / `sample_repeat` / `sample` / `sample_lod` ... as
    /// written (the Rust shader backend keeps the sampler state in the call).
    pub method: LiveId,
    pub coord: ExprId,
    pub lod: ExprId,
    pub dx: ExprId,
    pub dy: ExprId,
    pub reference: ExprId,
}

#[derive(Clone, Debug)]
pub enum ExprKind {
    Lit(IrLit),
    Local(LocalId),
    Param(u32),
    Global(GlobalId),
    Field(ExprId, u32),
    /// Components (x=0..w=3) and their count.
    Swizzle(ExprId, [u8; 4], u8),
    Index(ExprId, ExprId),
    /// The value behind an `inout` (pointer) param.
    Deref(ExprId),
    Unary(UnOp, ExprId),
    Binary(BinOp, ExprId, ExprId),
    /// A numeric conversion to the node's type (same shape).
    Convert(ExprId),
    Bitcast(ExprId),
    /// A vector, matrix, struct or array built from its arguments (a single
    /// scalar argument of a vector splats).
    Construct(Vec<ExprId>),
    Call(FuncId, Vec<ExprId>),
    Builtin(Builtin, Vec<ExprId>),
    /// A place passed to an `inout` param.
    AddrOf(ExprId),
    Select(ExprId, ExprId, ExprId),
    Sample(Box<IrSample>),
    TexSize(ExprId),
    /// `textureLoad(tex, coord, layer, lod)` of an integer coordinate
    /// (layer NO_EXPR outside array textures).
    TexLoad(ExprId, ExprId, ExprId, ExprId),
    PassMatrix(PassMatrix),
    InstanceIndex,
    /// GL: word `slot` of this instance's record as f32 bits, from the
    /// instance data texture (records past the vertex attributes).
    InstanceTexWord(u32),
    DepthClip(ExprId, ExprId, ExprId),
    /// An entry point's input / output (entry functions only).
    EntryIn(u32),
    EntryOut(u32),
    /// Statement-only value (an assignment or a discard already emitted).
    Nop,
    /// A range `start..end` while a `for` collects it.
    Range(ExprId, ExprId),
    Error,
}

#[derive(Clone, Debug)]
pub struct Expr {
    pub kind: ExprKind,
    pub ty: TyId,
}

#[derive(Clone, Debug)]
pub enum Stmt {
    /// A local's declaration with its initial value.
    Local(LocalId, ExprId),
    Assign(ExprId, ExprId),
    CompoundAssign(ExprId, BinOp, ExprId),
    Expr(ExprId),
    If(ExprId, Vec<Stmt>, Vec<Stmt>),
    /// `for var in start..end` over u32 (start and end evaluated once).
    For(LocalId, ExprId, ExprId, Vec<Stmt>),
    Loop(Vec<Stmt>),
    Break,
    Continue,
    Return(ExprId),
    Discard,
}

#[relax_api(plain)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LocalKind {
    /// A `let` / `var` of the source (by name and shadow).
    User,
    /// An if/else value.
    Phi,
    /// A loop guard counter.
    Guard,
    /// A runtime loop bound evaluated once.
    LoopEnd,
    /// Entry code.
    Temp,
}

#[relax_api(plain)]
#[derive(Clone, Debug)]
pub struct IrLocal {
    pub name: LiveId,
    pub shadow: u32,
    pub index: u32,
    pub ty: TyId,
    pub mutable: bool,
    pub kind: LocalKind,
}

#[relax_api(plain)]
#[derive(Clone, Debug)]
pub struct IrParam {
    pub name: LiveId,
    pub shadow: u32,
    pub ty: TyId,
    /// A pod method's `self`: passed by reference.
    pub inout: bool,
}

#[relax_api(plain)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum IrBuiltinIo {
    Position,
    VertexIndex,
    InstanceIndex,
    ViewIndex,
    FrontFacing,
    FragDepth,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum IrIoBinding {
    Builtin(IrBuiltinIo),
    Location(u32),
}

#[derive(Clone, Debug)]
pub struct IrEntryIo {
    pub name: String,
    pub ty: TyId,
    pub binding: IrIoBinding,
}

#[relax_api(plain)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum IrStage {
    Vertex,
    Fragment,
    Compute,
}

#[derive(Clone, Debug)]
pub struct IrEntry {
    pub stage: IrStage,
    pub inputs: Vec<IrEntryIo>,
    pub outputs: Vec<IrEntryIo>,
}

#[derive(Clone, Debug)]
pub struct IrFunction {
    /// The emitted name's base (`io_vertex`, `Sdf2d_box`, `scope0_premul`,
    /// `_f1...` for an overload), as the text compiler names it.
    pub name: String,
    pub key_name: LiveId,
    pub overload: usize,
    pub fnobj: ShaderFnId,
    pub arg_pods: Vec<ShaderTypeId>,
    pub params: Vec<IrParam>,
    pub ret: TyId,
    pub ret_pod: ShaderTypeId,
    pub locals: Vec<IrLocal>,
    pub exprs: Vec<Expr>,
    pub body: Vec<Stmt>,
    pub cost: u64,
    pub entry: Option<IrEntry>,
}

impl IrFunction {
    pub fn new(name: String, ret: TyId) -> IrFunction {
        IrFunction {
            name,
            key_name: LiveId(0),
            overload: 0,
            fnobj: ShaderFnId::default(),
            arg_pods: Vec::new(),
            params: Vec::new(),
            ret,
            ret_pod: ShaderTypeId::VOID,
            locals: Vec::new(),
            exprs: Vec::new(),
            body: Vec::new(),
            cost: 0,
            entry: None,
        }
    }

    pub fn expr(&mut self, kind: ExprKind, ty: TyId) -> ExprId {
        self.exprs.push(Expr { kind, ty });
        (self.exprs.len() - 1) as ExprId
    }

    pub fn local(&mut self, name: LiveId, shadow: u32, ty: TyId, mutable: bool, kind: LocalKind) -> LocalId {
        let index = self.locals.len() as u32;
        self.locals.push(IrLocal { name, shadow, index, ty, mutable, kind });
        index
    }

    pub fn ty_of(&self, e: ExprId) -> TyId {
        self.exprs[e as usize].ty
    }
}

/// A shader as IR: its types, globals and functions. Built by the lowering
/// (shader_lower*.rs) and the draw entry builder (shader_ir_draw.rs).
#[derive(Clone, Debug)]
pub struct IrModule {
    pub types: Vec<IrTy>,
    ty_map: HashMap<IrTy, TyId>,
    pub structs: Vec<IrStruct>,
    pod_map: HashMap<ShaderTypeId, TyId>,
    pub globals: Vec<IrGlobal>,
    pub functions: Vec<IrFunction>,
    /// A compute kernel's workgroup size (x, y, z).
    pub workgroup: [u32; 3],
}

pub const TY_VOID: TyId = 0;
pub const TY_BOOL: TyId = 1;
pub const TY_F32: TyId = 2;
pub const TY_F16: TyId = 3;
pub const TY_I32: TyId = 4;
pub const TY_U32: TyId = 5;
pub const TY_AINT: TyId = 6;
pub const TY_AFLOAT: TyId = 7;

impl Default for IrModule {
    fn default() -> IrModule {
        IrModule::new()
    }
}

impl IrModule {
    pub fn new() -> IrModule {
        let mut m = IrModule {
            types: Vec::new(),
            ty_map: HashMap::new(),
            structs: Vec::new(),
            pod_map: HashMap::new(),
            globals: Vec::new(),
            functions: Vec::new(),
            workgroup: [1, 1, 1],
        };
        m.ty(IrTy::Void);
        m.ty(IrTy::Scalar(IrScalar::Bool));
        m.ty(IrTy::Scalar(IrScalar::F32));
        m.ty(IrTy::Scalar(IrScalar::F16));
        m.ty(IrTy::Scalar(IrScalar::I32));
        m.ty(IrTy::Scalar(IrScalar::U32));
        m.ty(IrTy::AbstractInt);
        m.ty(IrTy::AbstractFloat);
        m
    }

    pub fn ty(&mut self, ty: IrTy) -> TyId {
        if let Some(id) = self.ty_map.get(&ty) {
            return *id;
        }
        let id = self.types.len() as TyId;
        self.types.push(ty.clone());
        self.ty_map.insert(ty, id);
        id
    }

    pub fn get(&self, ty: TyId) -> &IrTy {
        &self.types[ty as usize]
    }

    pub fn scalar_ty(&mut self, s: IrScalar) -> TyId {
        match s {
            IrScalar::Bool => TY_BOOL,
            IrScalar::F32 => TY_F32,
            IrScalar::F16 => TY_F16,
            IrScalar::I32 => TY_I32,
            IrScalar::U32 => TY_U32,
        }
    }

    pub fn vec_ty(&mut self, n: u8, s: IrScalar) -> TyId {
        if n == 1 {
            return self.scalar_ty(s);
        }
        self.ty(IrTy::Vec(n, s))
    }

    /// The scalar of a scalar, vector or matrix type.
    pub fn scalar_of(&self, ty: TyId) -> Option<IrScalar> {
        match self.get(ty) {
            IrTy::Scalar(s) | IrTy::Vec(_, s) => Some(*s),
            IrTy::Mat(..) => Some(IrScalar::F32),
            _ => None,
        }
    }

    /// Lanes of a scalar (1) or vector type, 0 for anything else.
    pub fn lanes(&self, ty: TyId) -> u32 {
        match self.get(ty) {
            IrTy::Scalar(_) => 1,
            IrTy::Vec(n, _) => *n as u32,
            _ => 0,
        }
    }

    /// The same shape with another scalar.
    pub fn with_scalar(&mut self, ty: TyId, s: IrScalar) -> TyId {
        match self.get(ty).clone() {
            IrTy::Vec(n, _) => self.ty(IrTy::Vec(n, s)),
            _ => self.scalar_ty(s),
        }
    }

    pub fn is_abstract(&self, ty: TyId) -> bool {
        ty == TY_AINT || ty == TY_AFLOAT
    }

    /// The IR type of a pod type. Compact fetch formats are their logical
    /// vec2f / vec4f; a struct is registered with its fields.
    pub fn pod_ty(&mut self, heap: &impl ShaderTypes, pod: ShaderTypeId) -> TyId {
        if let Some(t) = self.pod_map.get(&pod) {
            return *t;
        }
        let data = heap.type_data(pod).clone();
        let inline = ShaderTypeInline { id: pod, data };
        let t = self.pod_inline_ty(heap, &inline);
        self.pod_map.insert(pod, t);
        t
    }

    pub fn pod_inline_ty(&mut self, heap: &impl ShaderTypes, ty: &ShaderTypeInline) -> TyId {
        use crate::shader_schema::ShaderVec as V;
        match &ty.data.ty {
            ShaderTypeKind::Void => TY_VOID,
            ShaderTypeKind::F32 => TY_F32,
            ShaderTypeKind::F16 => TY_F16,
            ShaderTypeKind::U32 | ShaderTypeKind::AtomicU32 => TY_U32,
            ShaderTypeKind::I32 | ShaderTypeKind::AtomicI32 => TY_I32,
            ShaderTypeKind::Bool => TY_BOOL,
            ShaderTypeKind::Vec(v) => {
                let (n, s) = match v {
                    V::Vec2f => (2, IrScalar::F32),
                    V::Vec3f => (3, IrScalar::F32),
                    V::Vec4f => (4, IrScalar::F32),
                    V::Vec2h => (2, IrScalar::F16),
                    V::Vec3h => (3, IrScalar::F16),
                    V::Vec4h => (4, IrScalar::F16),
                    V::Vec2u => (2, IrScalar::U32),
                    V::Vec3u => (3, IrScalar::U32),
                    V::Vec4u => (4, IrScalar::U32),
                    V::Vec2i => (2, IrScalar::I32),
                    V::Vec3i => (3, IrScalar::I32),
                    V::Vec4i => (4, IrScalar::I32),
                    V::Vec2b => (2, IrScalar::Bool),
                    V::Vec3b => (3, IrScalar::Bool),
                    V::Vec4b => (4, IrScalar::Bool),
                };
                self.ty(IrTy::Vec(n, s))
            }
            ShaderTypeKind::Mat(m) => {
                use crate::shader_schema::ShaderMat as M;
                let (c, r) = match m {
                    M::Mat2x2f => (2, 2),
                    M::Mat2x3f => (2, 3),
                    M::Mat2x4f => (2, 4),
                    M::Mat3x2f => (3, 2),
                    M::Mat3x3f => (3, 3),
                    M::Mat3x4f => (3, 4),
                    M::Mat4x2f => (4, 2),
                    M::Mat4x3f => (4, 3),
                    M::Mat4x4f => (4, 4),
                };
                self.ty(IrTy::Mat(c, r))
            }
            ShaderTypeKind::Packed(p) => {
                let n = if p.is_vec4() { 4 } else { 2 };
                self.ty(IrTy::Vec(n, IrScalar::F32))
            }
            ShaderTypeKind::FixedArray { ty: inner, len, .. } => {
                let e = self.pod_inline_ty(heap, inner);
                self.ty(IrTy::Array(e, *len as u32))
            }
            ShaderTypeKind::VariableArray { ty: inner, .. } => {
                let e = self.pod_inline_ty(heap, inner);
                self.ty(IrTy::Array(e, 0))
            }
            ShaderTypeKind::Struct { fields, .. } => {
                if let Some(t) = self.pod_map.get(&ty.id) {
                    return *t;
                }
                let index = self.structs.len() as u32;
                self.structs.push(IrStruct {
                    name: ty.data.name.unwrap_or(LiveId(0)),
                    synth: None,
                    pod: ty.id,
                    fields: Vec::new(),
                });
                let t = self.ty(IrTy::Struct(index));
                self.pod_map.insert(ty.id, t);
                let mut out = Vec::new();
                for f in fields {
                    let ft = self.pod_inline_ty(heap, &f.ty);
                    out.push(IrField { name: f.name, ty: ft });
                }
                self.structs[index as usize].fields = out;
                t
            }
            // Enums and the builder markers have no shader type.
            _ => TY_VOID,
        }
    }

    /// The struct's name may be set on the heap after it was registered
    /// (a uniform's type takes the uniform's name): refresh them.
    pub fn refresh_struct_names(&mut self, heap: &impl ShaderTypes) {
        for s in &mut self.structs {
            if s.pod == ShaderTypeId::VOID {
                continue;
            }
            if let Some(name) = heap.type_data(s.pod).name {
                s.name = name;
            }
        }
    }

    /// A struct the compiler makes (uniform blocks), not from a pod type.
    pub fn synthetic_struct(&mut self, name: &'static str, fields: Vec<IrField>) -> TyId {
        let index = self.structs.len() as u32;
        self.structs.push(IrStruct { name: LiveId(0), synth: Some(name), pod: ShaderTypeId::VOID, fields });
        self.ty(IrTy::Struct(index))
    }

    pub fn global(&mut self, kind: IrGlobalKind, name: LiveId, ty: TyId) -> GlobalId {
        for (i, g) in self.globals.iter().enumerate() {
            if g.kind == kind && g.name == name {
                return i as GlobalId;
            }
        }
        self.globals.push(IrGlobal { kind, name, ty, binding: None });
        (self.globals.len() - 1) as GlobalId
    }

    pub fn find_global(&self, kind: IrGlobalKind, name: LiveId) -> Option<GlobalId> {
        for (i, g) in self.globals.iter().enumerate() {
            if g.kind == kind && g.name == name {
                return Some(i as GlobalId);
            }
        }
        None
    }

    pub fn function_by_name(&self, name: &str) -> Option<FuncId> {
        for (i, f) in self.functions.iter().enumerate() {
            if f.name == name {
                return Some(i as FuncId);
            }
        }
        None
    }

    /// The functions reachable from `root` (itself included).
    pub fn reachable(&self, root: FuncId) -> Vec<FuncId> {
        let mut seen = vec![false; self.functions.len()];
        let mut order = Vec::new();
        let mut stack = vec![root];
        while let Some(f) = stack.pop() {
            if seen[f as usize] {
                continue;
            }
            seen[f as usize] = true;
            order.push(f);
            let func = &self.functions[f as usize];
            let live = live_exprs(func);
            for (i, e) in func.exprs.iter().enumerate() {
                if !live[i] {
                    continue;
                }
                if let ExprKind::Call(callee, _) = &e.kind {
                    if !seen[*callee as usize] {
                        stack.push(*callee);
                    }
                }
            }
        }
        order
    }

    /// The globals the statements of `funcs` read or write.
    pub fn globals_used(&self, funcs: &[FuncId]) -> Vec<bool> {
        let mut used = vec![false; self.globals.len()];
        for f in funcs {
            let func = &self.functions[*f as usize];
            let live = live_exprs(func);
            for (i, e) in func.exprs.iter().enumerate() {
                if !live[i] {
                    continue;
                }
                if let ExprKind::Global(g) = e.kind {
                    used[g as usize] = true;
                }
            }
        }
        used
    }
}

/// Size and alignment by the host-shareable layout rules (std140/std430 as
/// WGSL defines them for everything the draw path uploads).
pub fn ir_layout(m: &IrModule, ty: TyId) -> (u32, u32) {
    match m.get(ty) {
        IrTy::Scalar(s) => (s.width(), s.width()),
        IrTy::Vec(n, s) => {
            let w = s.width();
            match n {
                2 => (2 * w, 2 * w),
                3 => (3 * w, 4 * w),
                _ => (4 * w, 4 * w),
            }
        }
        IrTy::Mat(c, r) => {
            let col = if *r == 2 { 8 } else { 16 };
            (*c as u32 * col, col)
        }
        IrTy::Array(e, n) => {
            let stride = ir_array_stride(m, *e);
            let (_, ea) = ir_layout(m, *e);
            (stride * (*n).max(1), ea)
        }
        IrTy::Struct(i) => {
            let mut offset = 0u32;
            let mut align = 1u32;
            for f in &m.structs[*i as usize].fields {
                let (sz, al) = ir_layout(m, f.ty);
                offset = round_up(offset, al);
                offset += sz;
                align = align.max(al);
            }
            (round_up(offset.max(1), align), align)
        }
        _ => (4, 4),
    }
}

pub fn ir_array_stride(m: &IrModule, elem: TyId) -> u32 {
    let (es, ea) = ir_layout(m, elem);
    round_up(es, ea)
}

/// Byte offsets of a struct's fields.
pub fn ir_struct_offsets(m: &IrModule, s: u32) -> Vec<u32> {
    let mut out = Vec::new();
    let mut offset = 0u32;
    for f in &m.structs[s as usize].fields {
        let (sz, al) = ir_layout(m, f.ty);
        offset = round_up(offset, al);
        out.push(offset);
        offset += sz;
    }
    out
}

pub fn round_up(v: u32, align: u32) -> u32 {
    (v + align - 1) / align * align
}

/// The expressions a function's statements reach (the arena also holds
/// values the lowering built and dropped).
pub fn live_exprs(f: &IrFunction) -> Vec<bool> {
    let mut live = vec![false; f.exprs.len()];
    let mut stack = Vec::new();
    live_stmts(&f.body, &mut stack);
    while let Some(e) = stack.pop() {
        if e == NO_EXPR || live[e as usize] {
            continue;
        }
        live[e as usize] = true;
        expr_children(&f.exprs[e as usize].kind, &mut stack);
    }
    live
}

fn live_stmts(stmts: &[Stmt], out: &mut Vec<ExprId>) {
    for s in stmts {
        match s {
            Stmt::Local(_, e) | Stmt::Expr(e) | Stmt::Return(e) => out.push(*e),
            Stmt::Assign(a, b) | Stmt::CompoundAssign(a, _, b) => {
                out.push(*a);
                out.push(*b);
            }
            Stmt::If(c, a, b) => {
                out.push(*c);
                live_stmts(a, out);
                live_stmts(b, out);
            }
            Stmt::For(_, a, b, body) => {
                out.push(*a);
                out.push(*b);
                live_stmts(body, out);
            }
            Stmt::Loop(body) => live_stmts(body, out),
            Stmt::Break | Stmt::Continue | Stmt::Discard => {}
        }
    }
}

/// The sub-expressions of an expression.
pub fn expr_children(kind: &ExprKind, out: &mut Vec<ExprId>) {
    match kind {
        ExprKind::Field(a, _)
        | ExprKind::Swizzle(a, _, _)
        | ExprKind::Deref(a)
        | ExprKind::Unary(_, a)
        | ExprKind::Convert(a)
        | ExprKind::Bitcast(a)
        | ExprKind::AddrOf(a)
        | ExprKind::TexSize(a) => out.push(*a),
        ExprKind::Index(a, b) | ExprKind::Binary(_, a, b) | ExprKind::Range(a, b) => {
            out.push(*a);
            out.push(*b);
        }
        ExprKind::Select(a, b, c) | ExprKind::DepthClip(a, b, c) => {
            out.push(*a);
            out.push(*b);
            out.push(*c);
        }
        ExprKind::TexLoad(a, b, c, d) => {
            out.push(*a);
            out.push(*b);
            out.push(*c);
            out.push(*d);
        }
        ExprKind::Construct(args) | ExprKind::Call(_, args) | ExprKind::Builtin(_, args) => {
            for a in args {
                out.push(*a);
            }
        }
        ExprKind::Sample(s) => {
            out.push(s.tex);
            out.push(s.coord);
            out.push(s.lod);
            out.push(s.dx);
            out.push(s.dy);
            out.push(s.reference);
        }
        _ => {}
    }
}

/// Scalar destinations in a backend uniform block, in 32-bit lanes.
#[derive(Clone)]
pub struct IrUniformLayout { pub size: usize, pub align: usize, pub lanes: Vec<usize> }
fn round(value: usize, align: usize) -> usize { value.div_ceil(align) * align }
pub fn place_uniform(offset: usize, layout: &IrUniformLayout, backend: crate::shader_backend::ShaderBackend) -> usize {
    let offset = round(offset, layout.align);
    if backend == crate::shader_backend::ShaderBackend::Hlsl && layout.size <= 4 && offset % 4 + layout.size > 4 {
        round(offset, 4)
    } else { offset }
}
pub fn ir_uniform_layout(ir: &IrModule, ty: TyId, backend: crate::shader_backend::ShaderBackend) -> IrUniformLayout {
    let std140 = matches!(backend, crate::shader_backend::ShaderBackend::Glsl | crate::shader_backend::ShaderBackend::Hlsl);
    match ir.get(ty) {
        IrTy::Scalar(scalar) => {
            assert_eq!(scalar.width(), 4, "draw uniforms require 32-bit scalar lanes");
            IrUniformLayout { size: 1, align: 1, lanes: vec![0] }
        }
        IrTy::Vec(n, scalar) => {
            assert_eq!(scalar.width(), 4, "draw uniforms require 32-bit vector lanes");
            let n = *n as usize;
            IrUniformLayout { size: if backend == crate::shader_backend::ShaderBackend::Metal && n == 3 { 4 } else { n },
                align: if backend == crate::shader_backend::ShaderBackend::Hlsl { 1 } else if n == 2 { 2 } else { 4 }, lanes: (0..n).collect() }
        }
        IrTy::Mat(columns, rows) => {
            let stride = if std140 || *rows != 2 { 4 } else { 2 };
            IrUniformLayout { size: *columns as usize * stride, align: stride,
                lanes: (0..*columns as usize).flat_map(|column| (0..*rows as usize).map(move |row| column * stride + row)).collect() }
        }
        IrTy::Array(element, count) => {
            assert!(*count > 0, "uniform arrays require a static length");
            let element = ir_uniform_layout(ir, *element, backend);
            let align = if std140 { element.align.max(4) } else { element.align };
            let stride = round(element.size, align);
            IrUniformLayout { size: stride * *count as usize, align,
                lanes: (0..*count as usize).flat_map(|index| element.lanes.iter().map(move |lane| index * stride + lane)).collect() }
        }
        IrTy::Struct(index) => {
            let mut size = 0;
            let mut align = if std140 { 4 } else { 1 };
            let mut lanes = Vec::new();
            for field in &ir.structs[*index as usize].fields {
                let field = ir_uniform_layout(ir, field.ty, backend);
                size = place_uniform(size, &field, backend);
                lanes.extend(field.lanes.iter().map(|lane| size + lane));
                size += field.size;
                align = align.max(field.align);
            }
            IrUniformLayout { size: round(size, align), align, lanes }
        }
        other => panic!("unsupported uniform IR type {other:?}"),
    }
}

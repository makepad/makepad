//! Text from the shader IR: expressions, statements and functions in the
//! syntax of a text backend. The module around them (declarations, entry
//! points, preambles) is each backend's own (shader_ir_wgsl.rs,
//! shader_ir_glsl.rs, ...).
use makepad_relax_api::relax_api;

use crate::shader_ir::*;
use crate::shader_output::TextureType;
use makepad_live_id::{id, LiveId};
use std::fmt::Write;

#[relax_api(plain)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Dialect {
    Wgsl,
    /// GLSL ES 3.00 (GL ES, WebGL 2; the host adds the preamble with
    /// `VIEW_ID`, `sample2d*`, `samplecube*`, `sample2dOES`, `depth_clip`).
    Glsl,
    /// HLSL for D3D11 (shader model 5). Matrices are `row_major`: an HLSL
    /// matrix holds the IR matrix transposed, so `m[i]` is column i as
    /// everywhere else, constructors take columns, and `a * b` is
    /// `mul(b, a)`.
    Hlsl,
    /// Metal Shading Language. MSL has no mutable program-scope variables:
    /// the per-invocation state (IO globals, resources) is one `Mp` struct
    /// every function takes as `thread Mp &_mp`.
    Metal,
}

/// What a module printer decides for the shared printer.
pub struct TextCx<'a> {
    pub m: &'a IrModule,
    pub dialect: Dialect,
    /// XR variant: the pass matrices select by the view.
    pub xr: bool,
    /// Helpers the printed code calls (the module printer emits them).
    pub needs_inverse: bool,
    pub needs_pass_helpers: bool,
    /// A half type was printed (WGSL needs `enable f16;`).
    pub uses_f16: std::cell::Cell<bool>,
    /// GLSL: video textures are `samplerExternalOES` sampled by
    /// `sample2dOES` (GL ES on Android and Linux, not under Vulkan).
    pub glsl_oes_video: bool,
    /// HLSL: the structs built by a constructor (HLSL has none; the module
    /// printer emits `_mp_make_<struct>` for each).
    pub hlsl_ctors: Vec<u32>,
    /// Metal pick variant: the 2D textures with a pick twin, as (texture
    /// global, bit); their samples also read the twin.
    /// Pick variants (Metal, HLSL): each sampled 2D texture global and its bit in the twin mask.
    pub pick_twins: Vec<(GlobalId, u32)>,
}

pub fn lid(id: LiveId) -> String {
    format!("{}", id)
}

pub fn wgsl_float(v: f64) -> String {
    let mut out = String::new();
    let a = v.abs();
    if a != 0.0 && (a >= 1e15 || a < 1e-6) {
        write!(out, "{:e}", v).ok();
    } else {
        write!(out, "{}", v).ok();
        if !out.contains('.') && !out.contains('e') && !out.contains("inf") && !out.contains("NaN") {
            out.push_str(".0");
        }
    }
    out
}

/// The shortest text that reads back as the same f32, with a point or an
/// exponent (a float literal in every dialect).
pub fn f32_text(v: f32) -> String {
    if v.is_nan() {
        return "(0.0/0.0)".to_string();
    }
    if v.is_infinite() {
        return if v > 0.0 { "3.4028235e38".to_string() } else { "-3.4028235e38".to_string() };
    }
    let s = format!("{:?}", v);
    if s.contains('.') || s.contains('e') {
        s
    } else {
        format!("{}.0", s)
    }
}

impl<'a> TextCx<'a> {
    pub fn new(m: &'a IrModule, dialect: Dialect, xr: bool) -> Self {
        TextCx {
            m,
            dialect,
            xr,
            needs_inverse: false,
            needs_pass_helpers: false,
            uses_f16: std::cell::Cell::new(false),
            glsl_oes_video: false,
            hlsl_ctors: Vec::new(),
            pick_twins: Vec::new(),
        }
    }

    fn glsl(&self) -> bool {
        self.dialect == Dialect::Glsl
    }

    fn hlsl(&self) -> bool {
        self.dialect == Dialect::Hlsl
    }

    fn metal(&self) -> bool {
        self.dialect == Dialect::Metal
    }

    /// GLSL and HLSL: C-like declarations, no typed literals.
    fn c_like(&self) -> bool {
        self.dialect != Dialect::Wgsl
    }

    pub fn scalar_name(&self, s: IrScalar) -> &'static str {
        match self.dialect {
            Dialect::Wgsl => match s {
                IrScalar::Bool => "bool",
                IrScalar::F32 => "f32",
                IrScalar::F16 => "f16",
                IrScalar::I32 => "i32",
                IrScalar::U32 => "u32",
            },
            Dialect::Glsl | Dialect::Hlsl => match s {
                IrScalar::Bool => "bool",
                IrScalar::F32 | IrScalar::F16 => "float",
                IrScalar::I32 => "int",
                IrScalar::U32 => "uint",
            },
            Dialect::Metal => match s {
                IrScalar::Bool => "bool",
                IrScalar::F32 => "float",
                IrScalar::F16 => "half",
                IrScalar::I32 => "int",
                IrScalar::U32 => "uint",
            },
        }
    }

    pub fn struct_name(&self, s: u32) -> String {
        let st = &self.m.structs[s as usize];
        if let Some(n) = st.synth {
            return n.to_string();
        }
        if st.name == id!(shared) {
            return "_mp_type_shared".to_string();
        }
        if st.name == LiveId(0) {
            return format!("_mp_struct{}", s);
        }
        self.ident(lid(st.name))
    }

    /// A user-given name, kept clear of HLSL's reserved words.
    pub fn ident(&self, n: String) -> String {
        if (self.hlsl() && HLSL_RESERVED.contains(&n.as_str())) || (self.metal() && METAL_RESERVED.contains(&n.as_str())) {
            return format!("_mp_{}", n);
        }
        n
    }

    pub fn ty_name(&self, t: TyId) -> String {
        if self.m.scalar_of(t) == Some(IrScalar::F16) {
            self.uses_f16.set(true);
        }
        match self.m.get(t) {
            IrTy::Void => "void".to_string(),
            IrTy::Scalar(s) => self.scalar_name(*s).to_string(),
            IrTy::AbstractInt => self.scalar_name(IrScalar::I32).to_string(),
            IrTy::AbstractFloat => self.scalar_name(IrScalar::F32).to_string(),
            IrTy::Vec(n, s) => match self.dialect {
                Dialect::Wgsl => match s {
                    IrScalar::F32 => format!("vec{}f", n),
                    IrScalar::F16 => format!("vec{}h", n),
                    IrScalar::I32 => format!("vec{}i", n),
                    IrScalar::U32 => format!("vec{}u", n),
                    IrScalar::Bool => format!("vec{}<bool>", n),
                },
                Dialect::Glsl => match s {
                    IrScalar::F32 | IrScalar::F16 => format!("vec{}", n),
                    IrScalar::I32 => format!("ivec{}", n),
                    IrScalar::U32 => format!("uvec{}", n),
                    IrScalar::Bool => format!("bvec{}", n),
                },
                Dialect::Hlsl | Dialect::Metal => format!("{}{}", self.scalar_name(*s), n),
            },
            IrTy::Mat(c, r) => match self.dialect {
                Dialect::Wgsl => format!("mat{}x{}f", c, r),
                Dialect::Glsl => {
                    if c == r {
                        format!("mat{}", c)
                    } else {
                        format!("mat{}x{}", c, r)
                    }
                }
                // Transposed: C rows of R.
                Dialect::Hlsl => format!("float{}x{}", c, r),
                Dialect::Metal => format!("float{}x{}", c, r),
            },
            IrTy::Array(e, n) => match self.dialect {
                Dialect::Wgsl => {
                    if *n == 0 {
                        format!("array<{}>", self.ty_name(*e))
                    } else {
                        format!("array<{}, {}>", self.ty_name(*e), n)
                    }
                }
                Dialect::Glsl => format!("{}[{}]", self.ty_name(*e), n),
                // Only in a declaration (`decl` puts the length after the name).
                Dialect::Hlsl => format!("{}[{}]", self.ty_name(*e), n),
                Dialect::Metal => format!("array<{}, {}>", self.ty_name(*e), n),
            },
            IrTy::Struct(s) => self.struct_name(*s),
            IrTy::CooperativeMatrix(_) => unreachable!("cooperative matrices are compute SPIR-V types"),
            IrTy::Texture(tt) => match self.dialect {
                Dialect::Wgsl => wgsl_texture_type(*tt).to_string(),
                Dialect::Glsl => self.glsl_sampler_type(*tt).to_string(),
                Dialect::Hlsl => hlsl_texture_type(*tt).to_string(),
                Dialect::Metal => metal_texture_type(*tt).to_string(),
            },
            IrTy::Sampler(c) => {
                if self.hlsl() {
                    return if *c { "SamplerComparisonState".to_string() } else { "SamplerState".to_string() };
                }
                if *c {
                    "sampler_comparison".to_string()
                } else {
                    "sampler".to_string()
                }
            }
        }
    }

    pub fn glsl_sampler_type(&self, tt: TextureType) -> &'static str {
        match tt {
            TextureType::Texture1d | TextureType::Texture2d => "sampler2D",
            TextureType::Texture1dArray | TextureType::Texture2dArray => "sampler2DArray",
            TextureType::Texture3d | TextureType::Texture3dArray => "sampler3D",
            TextureType::TextureCube => "samplerCube",
            TextureType::TextureCubeArray => "samplerCubeArray",
            TextureType::TextureDepth => "sampler2DShadow",
            TextureType::TextureDepthArray => "sampler2DArray",
            TextureType::TextureVideo => {
                if self.glsl_oes_video {
                    "samplerExternalOES"
                } else {
                    "sampler2D"
                }
            }
        }
    }

    /// A declaration `type name` (GLSL puts an array's length after the
    /// name's type: `float[4] name`).
    pub fn decl(&self, t: TyId, name: &str) -> String {
        match self.dialect {
            Dialect::Wgsl => format!("{}: {}", name, self.ty_name(t)),
            Dialect::Glsl | Dialect::Metal => format!("{} {}", self.ty_name(t), name),
            Dialect::Hlsl => {
                // `T name[n]` (arrays of arrays: `T name[n][m]`).
                let mut dims = String::new();
                let mut e = t;
                while let IrTy::Array(inner, n) = self.m.get(e) {
                    write!(dims, "[{}]", n).ok();
                    e = *inner;
                }
                format!("{} {}{}", self.ty_name(e), name, dims)
            }
        }
    }

    pub fn global_name(&self, g: GlobalId) -> String {
        if self.metal() {
            let glob = &self.m.globals[g as usize];
            let n = lid(glob.name);
            return match glob.kind {
                IrGlobalKind::Io(IrIo::UniformBuffer) => format!("(*_mp.unibuf_{})", n),
                IrGlobalKind::Io(IrIo::Uniform) => format!("_mp.u->uni_{}", n),
                IrGlobalKind::Io(IrIo::ScopeUniform) => format!("_mp.su->su_{}", n),
                IrGlobalKind::ViewId => "0".to_string(),
                IrGlobalKind::InstanceBuffer => format!("_mp.{}", word_buffer_name(glob.name)),
                _ => format!("_mp.{}", self.plain_global_name(g)),
            };
        }
        self.plain_global_name(g)
    }

    /// A global's own name (Metal: its field in `Mp`).
    pub fn plain_global_name(&self, g: GlobalId) -> String {
        let glob = &self.m.globals[g as usize];
        let n = lid(glob.name);
        match glob.kind {
            IrGlobalKind::Io(io) => match io {
                IrIo::Geometry => format!("vb_{}", n),
                IrIo::DynInstance => format!("dyninst_{}", n),
                IrIo::RustInstance => format!("rustinst_{}", n),
                IrIo::Uniform => format!("uni_{}", n),
                IrIo::ScopeUniform => format!("su_{}", n),
                IrIo::UniformBuffer => format!("unibuf_{}", n),
                IrIo::Varying => format!("var_{}", n),
                IrIo::FragmentOutput(i) => format!("frag_fb{}", i),
                IrIo::Texture(_) => format!("tex_{}", n),
                // Compute kernels print only to SPIR-V.
                IrIo::Storage(_) | IrIo::Workgroup => format!("kernel_{}", n),
            },
            IrGlobalKind::VertexPosition => "vtx_pos".to_string(),
            IrGlobalKind::IterCounter => "_mp_iter".to_string(),
            IrGlobalKind::ViewId => {
                if self.glsl() {
                    "int(VIEW_ID)".to_string()
                } else {
                    "VIEW_ID".to_string()
                }
            }
            IrGlobalKind::InstanceIndex => "_mp_instance_index".to_string(),
            IrGlobalKind::DynUniformBlock => "_mp_dyn_uniforms".to_string(),
            IrGlobalKind::ScopeUniformBlock => "_mp_scope_uniforms".to_string(),
            IrGlobalKind::InstanceBuffer => "_mp_inst".to_string(),
            IrGlobalKind::XrDepth => "tex_xr_depth".to_string(),
            IrGlobalKind::Sampler(i) => format!("_s{}", i),
            IrGlobalKind::ClipYFlip => "mp_clip_y_flip".to_string(),
            IrGlobalKind::InstanceBase => "_mp_inst_base".to_string(),
        }
    }

    pub fn local_name(&self, f: &IrFunction, l: LocalId) -> String {
        let local = &f.locals[l as usize];
        match local.kind {
            LocalKind::Phi => format!("_phi_{}", l),
            LocalKind::Guard => format!("_mp_loop_guard_{}", l),
            LocalKind::LoopEnd => format!("_mp_end_{}", l),
            LocalKind::Temp => format!("_t{}", l),
            LocalKind::User => format!("l{}_{}", l, lid(local.name)),
        }
    }

    pub fn param_name(&self, f: &IrFunction, i: u32) -> String {
        let p = &f.params[i as usize];
        if p.inout || p.name == id!(self) {
            return "_self".to_string();
        }
        format!("p_{}", lid(p.name))
    }

    pub fn field_name(&self, base_ty: TyId, index: u32) -> String {
        match self.m.get(base_ty) {
            IrTy::Struct(s) => {
                let st = &self.m.structs[*s as usize];
                match st.fields.get(index as usize) {
                    Some(f) => {
                        // Uniform block fields carry their global's prefix.
                        if st.synth == Some("MpDynUniforms") {
                            return format!("uni_{}", lid(f.name));
                        }
                        if st.synth == Some("MpScopeUniforms") {
                            return format!("su_{}", lid(f.name));
                        }
                        self.ident(lid(f.name))
                    }
                    None => format!("_f{}", index),
                }
            }
            _ => format!("_f{}", index),
        }
    }

    fn lit(&self, l: IrLit) -> String {
        let glsl = self.c_like();
        match l {
            IrLit::Bool(b) => {
                if b {
                    "true".to_string()
                } else {
                    "false".to_string()
                }
            }
            IrLit::F32(v) => {
                if glsl {
                    f32_text(v)
                } else {
                    format!("{}f", f32_text(v))
                }
            }
            IrLit::F16(v) => {
                if self.metal() {
                    return format!("{}h", f32_text(v));
                }
                if glsl {
                    return f32_text(v);
                }
                self.uses_f16.set(true);
                format!("{}h", f32_text(v))
            }
            IrLit::I32(v) => {
                if v == i32::MIN {
                    if glsl {
                        "(-2147483647 - 1)".to_string()
                    } else {
                        "i32(-2147483648)".to_string()
                    }
                } else if glsl {
                    format!("{}", v)
                } else {
                    format!("{}i", v)
                }
            }
            IrLit::U32(v) => format!("{}u", v),
            IrLit::AbstractInt(v) => format!("{}", v),
            IrLit::AbstractFloat(v) => wgsl_float(v),
        }
    }

    fn is_mat(&self, t: TyId) -> bool {
        matches!(self.m.get(t), IrTy::Mat(..))
    }

    fn is_vec(&self, t: TyId) -> bool {
        matches!(self.m.get(t), IrTy::Vec(..))
    }

    pub fn expr(&mut self, f: &IrFunction, e: ExprId) -> String {
        if e == NO_EXPR {
            return "0".to_string();
        }
        let ex = &f.exprs[e as usize];
        let ty = ex.ty;
        let glsl = self.glsl();
        if self.hlsl() {
            if let Some(s) = self.hlsl_expr(f, e) {
                return s;
            }
        }
        if self.metal() {
            if let Some(s) = self.metal_expr(f, e) {
                return s;
            }
        }
        match &ex.kind {
            ExprKind::Lit(l) => self.lit(*l),
            ExprKind::Local(l) => self.local_name(f, *l),
            ExprKind::Param(i) => self.param_name(f, *i),
            ExprKind::Global(g) => self.global_name(*g),
            ExprKind::Field(b, i) => {
                let bt = f.exprs[*b as usize].ty;
                format!("{}.{}", self.expr(f, *b), self.field_name(bt, *i))
            }
            ExprKind::Swizzle(b, lanes, n) => {
                let mut s = self.expr(f, *b);
                s.push('.');
                for k in 0..*n as usize {
                    s.push(['x', 'y', 'z', 'w'][lanes[k] as usize & 3]);
                }
                s
            }
            ExprKind::Index(b, i) => format!("{}[{}]", self.expr(f, *b), self.expr(f, *i)),
            ExprKind::Deref(p) => {
                if self.c_like() {
                    self.expr(f, *p)
                } else {
                    format!("(*{})", self.expr(f, *p))
                }
            }
            ExprKind::Unary(op, a) => {
                let a = self.expr(f, *a);
                match op {
                    UnOp::Neg => format!("(-{})", a),
                    UnOp::Not => {
                        if glsl && self.is_vec(ty) {
                            format!("not({})", a)
                        } else {
                            format!("(!{})", a)
                        }
                    }
                    UnOp::BitNot => format!("(~{})", a),
                }
            }
            ExprKind::Binary(op, a, b) => self.binary(f, *op, *a, *b, ty),
            ExprKind::Convert(a) => format!("{}({})", self.ty_name(ty), self.expr(f, *a)),
            ExprKind::Bitcast(a) => {
                let at = f.exprs[*a as usize].ty;
                let s = self.expr(f, *a);
                if glsl {
                    let from = self.m.scalar_of(at).unwrap_or(IrScalar::F32);
                    let to = self.m.scalar_of(ty).unwrap_or(IrScalar::F32);
                    match (from.is_float(), to) {
                        (true, IrScalar::U32) => format!("floatBitsToUint({})", s),
                        (true, IrScalar::I32) => format!("floatBitsToInt({})", s),
                        (false, IrScalar::F32 | IrScalar::F16) => {
                            if from == IrScalar::I32 {
                                format!("intBitsToFloat({})", s)
                            } else {
                                format!("uintBitsToFloat({})", s)
                            }
                        }
                        _ => format!("{}({})", self.ty_name(ty), s),
                    }
                } else {
                    format!("bitcast<{}>({})", self.ty_name(ty), s)
                }
            }
            ExprKind::Construct(args) => {
                if args.is_empty() && glsl {
                    return self.glsl_zero(ty);
                }
                let mut s = format!("{}(", self.ty_name(ty));
                for (k, a) in args.iter().enumerate() {
                    if k > 0 {
                        s.push_str(", ");
                    }
                    let t = self.expr(f, *a);
                    s.push_str(&t);
                }
                s.push(')');
                s
            }
            ExprKind::Call(fi, args) => {
                let name = self.m.functions[*fi as usize].name.replace('$', "_mp_generic_");
                if self.metal() {
                    let mut s = self.call_text(f, &name, args);
                    let at = name.len() + 1;
                    s.insert_str(at, if args.is_empty() { "_mp" } else { "_mp, " });
                    return s;
                }
                self.call_text(f, &name, args)
            }
            ExprKind::Builtin(op, args) => self.builtin(f, *op, args, ty),
            ExprKind::AddrOf(p) => {
                if self.c_like() {
                    self.expr(f, *p)
                } else {
                    format!("&{}", self.expr(f, *p))
                }
            }
            ExprKind::Select(c, t, fl) => {
                let ct = f.exprs[*c as usize].ty;
                let cs = self.expr(f, *c);
                let ts = self.expr(f, *t);
                let fs = self.expr(f, *fl);
                if glsl {
                    if self.is_vec(ct) {
                        format!("mix({}, {}, {})", fs, ts, cs)
                    } else {
                        format!("({} ? {} : {})", cs, ts, fs)
                    }
                } else {
                    format!("select({}, {}, {})", fs, ts, cs)
                }
            }
            ExprKind::Sample(s) => {
                if glsl {
                    self.glsl_sample(f, s, ty)
                } else {
                    self.wgsl_sample(f, s, ty)
                }
            }
            ExprKind::TexSize(t) => {
                let ts = self.expr(f, *t);
                let unsigned = self.m.scalar_of(ty) == Some(IrScalar::U32);
                if glsl {
                    if unsigned {
                        format!("uvec2(textureSize({}, 0).xy)", ts)
                    } else {
                        format!("vec2(textureSize({}, 0).xy)", ts)
                    }
                } else if unsigned {
                    format!("textureDimensions({})", ts)
                } else {
                    format!("vec2f(textureDimensions({}).xy)", ts)
                }
            }
            ExprKind::TexLoad(t, c, layer, lod) => {
                let ts = self.expr(f, *t);
                let cs = self.expr(f, *c);
                let ls = self.expr(f, *lod);
                if glsl {
                    let r = if *layer != NO_EXPR {
                        let l = self.expr(f, *layer);
                        format!("texelFetch({}, ivec3({}, {}), {})", ts, cs, l, ls)
                    } else {
                        format!("texelFetch({}, {}, {})", ts, cs, ls)
                    };
                    if ty == TY_F32 {
                        format!("{}.x", r)
                    } else {
                        r
                    }
                } else if *layer != NO_EXPR {
                    let l = self.expr(f, *layer);
                    format!("textureLoad({}, {}, {}, {})", ts, cs, l, ls)
                } else {
                    format!("textureLoad({}, {}, {})", ts, cs, ls)
                }
            }
            ExprKind::PassMatrix(which) => {
                let n = pass_matrix_name(*which);
                if glsl {
                    format!("unibuf_draw_pass.{}[int(VIEW_ID)]", n)
                } else if self.xr {
                    self.needs_pass_helpers = true;
                    format!("mp_draw_pass_{}()", n)
                } else {
                    format!("unibuf_draw_pass.{}", n)
                }
            }
            ExprKind::InstanceIndex => {
                if glsl {
                    "uint(gl_InstanceID)".to_string()
                } else {
                    "_mp_instance_index".to_string()
                }
            }
            ExprKind::InstanceTexWord(slot) => format!("_mp_inst_word({})", slot),
            ExprKind::DepthClip(a, b, c) => self.call_text(f, "depth_clip", &[*a, *b, *c]),
            ExprKind::EntryIn(i) => {
                let entry = f.entry.as_ref().unwrap();
                let io = &entry.inputs[*i as usize];
                if glsl {
                    match io.binding {
                        IrIoBinding::Builtin(IrBuiltinIo::Position) => "gl_FragCoord".to_string(),
                        IrIoBinding::Builtin(IrBuiltinIo::InstanceIndex) => "uint(gl_InstanceID)".to_string(),
                        IrIoBinding::Builtin(IrBuiltinIo::VertexIndex) => "uint(gl_VertexID)".to_string(),
                        _ => io.name.clone(),
                    }
                } else {
                    format!("in.{}", io.name)
                }
            }
            ExprKind::EntryOut(i) => {
                let entry = f.entry.as_ref().unwrap();
                let io = &entry.outputs[*i as usize];
                if glsl {
                    io.name.clone()
                } else {
                    format!("out_data.{}", io.name)
                }
            }
            ExprKind::Nop | ExprKind::Error | ExprKind::Range(..) => "0".to_string(),
        }
    }

    /// GLSL has no `T()` zero constructor.
    fn glsl_zero(&self, ty: TyId) -> String {
        match self.m.get(ty) {
            IrTy::Scalar(s) | IrTy::Vec(_, s) => {
                let z = match s {
                    IrScalar::Bool => "false",
                    IrScalar::F32 | IrScalar::F16 => "0.0",
                    IrScalar::I32 => "0",
                    IrScalar::U32 => "0u",
                };
                if matches!(self.m.get(ty), IrTy::Scalar(_)) {
                    z.to_string()
                } else {
                    format!("{}({})", self.ty_name(ty), z)
                }
            }
            IrTy::Mat(..) => format!("{}(0.0)", self.ty_name(ty)),
            IrTy::Array(e, n) => {
                let mut parts = Vec::new();
                for _ in 0..*n {
                    parts.push(self.glsl_zero(*e));
                }
                format!("{}({})", self.ty_name(ty), parts.join(", "))
            }
            IrTy::Struct(s) => {
                let mut parts = Vec::new();
                for fd in &self.m.structs[*s as usize].fields {
                    parts.push(self.glsl_zero(fd.ty));
                }
                format!("{}({})", self.ty_name(ty), parts.join(", "))
            }
            _ => "0".to_string(),
        }
    }

    fn binary(&mut self, f: &IrFunction, op: BinOp, a: ExprId, b: ExprId, ty: TyId) -> String {
        let at = f.exprs[a as usize].ty;
        let bt = f.exprs[b as usize].ty;
        let sa = self.expr(f, a);
        let mut sb = self.expr(f, b);
        let glsl = self.glsl();
        if matches!(op, BinOp::Shl | BinOp::Shr) && !glsl {
            // WGSL shifts by a u32 of the left side's shape.
            let n = self.m.lanes(at);
            if n > 1 && self.m.lanes(bt) == 1 {
                sb = format!("vec{}u({})", n, sb);
            }
        }
        if glsl {
            // Component-wise comparisons of vectors are functions.
            if op.is_compare() && self.is_vec(at) {
                let fname = match op {
                    BinOp::Eq => "equal",
                    BinOp::Ne => "notEqual",
                    BinOp::Lt => "lessThan",
                    BinOp::Le => "lessThanEqual",
                    BinOp::Gt => "greaterThan",
                    _ => "greaterThanEqual",
                };
                return format!("{}({}, {})", fname, sa, sb);
            }
            // A float remainder (GLSL's `%` is integer only).
            if op == BinOp::Rem && self.m.scalar_of(ty).map(|s| s.is_float()).unwrap_or(false) {
                return format!("mod({}, {})", sa, sb);
            }
            // Logic on bool vectors.
            if matches!(op, BinOp::LogicAnd | BinOp::LogicOr | BinOp::BitAnd | BinOp::BitOr) && self.is_vec(ty) && self.m.scalar_of(ty) == Some(IrScalar::Bool) {
                let n = self.m.lanes(ty);
                let o = if matches!(op, BinOp::LogicAnd | BinOp::BitAnd) { "&&" } else { "||" };
                let mut parts = Vec::new();
                for k in 0..n {
                    let c = ['x', 'y', 'z', 'w'][k as usize];
                    parts.push(format!("({}.{} {} {}.{})", sa, c, o, sb, c));
                }
                return format!("bvec{}({})", n, parts.join(", "));
            }
            if op == BinOp::BitXor && self.m.scalar_of(ty) == Some(IrScalar::Bool) {
                return format!("({} != {})", sa, sb);
            }
        }
        format!("({} {} {})", sa, op.text(), sb)
    }

    fn call_text(&mut self, f: &IrFunction, name: &str, args: &[ExprId]) -> String {
        let mut s = format!("{}(", name);
        for (k, a) in args.iter().enumerate() {
            if k > 0 {
                s.push_str(", ");
            }
            let t = self.expr(f, *a);
            s.push_str(&t);
        }
        s.push(')');
        s
    }

    /// An argument converted to `ty`'s shape (a scalar splatted, a scalar
    /// of another kind converted), as the component-wise builtins need.
    fn arg_as(&mut self, f: &IrFunction, a: ExprId, ty: TyId) -> String {
        let at = f.exprs[a as usize].ty;
        let s = self.expr(f, a);
        if at == ty {
            return s;
        }
        if self.hlsl() {
            return format!("(({})({}))", self.ty_name(ty), s);
        }
        format!("{}({})", self.ty_name(ty), s)
    }

    fn builtin(&mut self, f: &IrFunction, op: Builtin, args: &[ExprId], ty: TyId) -> String {
        let glsl = self.glsl();
        let name = match op {
            Builtin::Abs => "abs",
            Builtin::Acos => "acos",
            Builtin::Acosh => "acosh",
            Builtin::Asin => "asin",
            Builtin::Asinh => "asinh",
            Builtin::Atan => "atan",
            Builtin::Atanh => "atanh",
            Builtin::Atan2 => {
                if glsl {
                    "atan"
                } else {
                    "atan2"
                }
            }
            Builtin::Ceil => "ceil",
            Builtin::Clamp => "clamp",
            Builtin::Cos => "cos",
            Builtin::Cosh => "cosh",
            Builtin::Cross => "cross",
            Builtin::Degrees => "degrees",
            Builtin::Distance => "distance",
            Builtin::Dot => "dot",
            Builtin::Exp => "exp",
            Builtin::Exp2 => "exp2",
            Builtin::Floor => "floor",
            Builtin::Fma => {
                if glsl {
                    let a = self.arg_as(f, args[0], ty);
                    let b = self.arg_as(f, args[1], ty);
                    let c = self.arg_as(f, args[2], ty);
                    return format!("({} * {} + {})", a, b, c);
                }
                "fma"
            }
            Builtin::Fract => "fract",
            Builtin::Inverse => {
                if glsl {
                    "inverse"
                } else {
                    self.needs_inverse = true;
                    "_mp_inverse4"
                }
            }
            Builtin::InverseSqrt => {
                if glsl {
                    "inversesqrt"
                } else {
                    "inverseSqrt"
                }
            }
            Builtin::Length => "length",
            Builtin::Log => "log",
            Builtin::Log2 => "log2",
            Builtin::Max => "max",
            Builtin::Min => "min",
            Builtin::Mix => "mix",
            Builtin::FMod => {
                let a = self.arg_as(f, args[0], ty);
                let b = self.arg_as(f, args[1], ty);
                // Each backend keeps its own float remainder: GLSL's
                // floor-based mod, WGSL's truncating %.
                if glsl {
                    return format!("mod({}, {})", a, b);
                }
                return format!("({} % {})", a, b);
            }
            Builtin::Normalize => "normalize",
            Builtin::Pow => "pow",
            Builtin::Radians => "radians",
            Builtin::Round => "round",
            Builtin::Sign => "sign",
            Builtin::Sin => "sin",
            Builtin::Sinh => "sinh",
            Builtin::Smoothstep => "smoothstep",
            Builtin::Sqrt => "sqrt",
            Builtin::Step => "step",
            Builtin::Tan => "tan",
            Builtin::Tanh => "tanh",
            Builtin::Trunc => "trunc",
            Builtin::Dpdx => {
                if glsl {
                    "dFdx"
                } else {
                    "dpdx"
                }
            }
            Builtin::Dpdy => {
                if glsl {
                    "dFdy"
                } else {
                    "dpdy"
                }
            }
            Builtin::Unpack2f16 | Builtin::Unpack4u8 => {
                let a = self.expr(f, args[0]);
                let at = f.exprs[args[0] as usize].ty;
                if glsl {
                    let n = if op == Builtin::Unpack2f16 { "_mp_unpack2f16" } else { "_mp_unpack4u8" };
                    let a = if at == TY_U32 { format!("uintBitsToFloat({})", a) } else { a };
                    return format!("{}({})", n, a);
                }
                let bits = if at == TY_U32 { a } else { format!("bitcast<u32>({})", a) };
                let n = if op == Builtin::Unpack2f16 { "unpack2x16float" } else { "unpack4x8unorm" };
                return format!("{}({})", n, bits);
            }
            Builtin::Unpack2x16Float => {
                if glsl {
                    "unpackHalf2x16"
                } else {
                    "unpack2x16float"
                }
            }
            Builtin::Unpack2x16Unorm => {
                if glsl {
                    "unpackUnorm2x16"
                } else {
                    "unpack2x16unorm"
                }
            }
            Builtin::Unpack2x16Snorm => {
                if glsl {
                    "unpackSnorm2x16"
                } else {
                    "unpack2x16snorm"
                }
            }
            Builtin::Unpack4x8Unorm => {
                if glsl {
                    "_mp_unpack_unorm4x8"
                } else {
                    "unpack4x8unorm"
                }
            }
            Builtin::Unpack4x8Snorm => {
                if glsl {
                    "_mp_unpack_snorm4x8"
                } else {
                    "unpack4x8snorm"
                }
            }
            // Compute kernels print only to SPIR-V (shader_ir_compute.rs).
            _ => "compute_kernel_builtin_unsupported",
        };
        // Component-wise builtins take arguments of the result's shape
        // (mix's selector and step's edge may be scalars).
        let componentwise = matches!(
            op,
            Builtin::Abs | Builtin::Clamp | Builtin::Max | Builtin::Min | Builtin::Mix | Builtin::Step | Builtin::Smoothstep | Builtin::Pow | Builtin::Atan2
        );
        let mut parts = Vec::new();
        for a in args {
            if componentwise && self.m.lanes(ty) >= 1 {
                parts.push(self.arg_as(f, *a, ty));
            } else {
                parts.push(self.expr(f, *a));
            }
        }
        format!("{}({})", name, parts.join(", "))
    }

    fn wgsl_sample(&mut self, f: &IrFunction, s: &IrSample, ty: TyId) -> String {
        let t = self.expr(f, s.tex);
        let smp = format!("_s{}", s.sampler);
        let c = self.expr(f, s.coord);
        let arrayed = matches!(s.tex_type, TextureType::Texture2dArray | TextureType::TextureDepthArray | TextureType::Texture1dArray);
        let coord = if arrayed { format!("({}).xy, i32(({}).z)", c, c) } else { c };
        let depth = matches!(s.tex_type, TextureType::TextureDepth | TextureType::TextureDepthArray);
        let r = match s.kind {
            SampleKind::Implicit => format!("textureSample({}, {}, {})", t, smp, coord),
            SampleKind::Lod => {
                let lod = self.expr(f, s.lod);
                if depth {
                    format!("textureSampleLevel({}, {}, {}, i32({}))", t, smp, coord, lod)
                } else {
                    format!("textureSampleLevel({}, {}, {}, {})", t, smp, coord, lod)
                }
            }
            SampleKind::Video => format!("textureSampleLevel({}, {}, {}, 0.0)", t, smp, coord),
            SampleKind::Grad => {
                let dx = self.expr(f, s.dx);
                let dy = self.expr(f, s.dy);
                format!("textureSampleGrad({}, {}, {}, {}, {})", t, smp, coord, dx, dy)
            }
            SampleKind::CompareLevel0 => {
                let r = self.expr(f, s.reference);
                return format!("textureSampleCompareLevel({}, {}, {}, {})", t, smp, coord, r);
            }
        };
        if depth && ty != TY_F32 {
            // A depth texture samples one value; the IR's sample is a vec4.
            return format!("vec4f({})", r);
        }
        r
    }

    /// GLSL samples through the host's helpers (`sample2d`, `samplecube`,
    /// `sample2dOES`); the sampler state is bound per texture by the host.
    fn glsl_sample(&mut self, f: &IrFunction, s: &IrSample, ty: TyId) -> String {
        let t = self.expr(f, s.tex);
        let c = self.expr(f, s.coord);
        let r = match s.kind {
            SampleKind::CompareLevel0 => {
                let r = self.expr(f, s.reference);
                return format!("textureLod({}, vec3({}, {}), 0.0)", t, c, r);
            }
            SampleKind::Grad => {
                let dx = self.expr(f, s.dx);
                let dy = self.expr(f, s.dy);
                format!("textureGrad({}, {}, {}, {})", t, c, dx, dy)
            }
            SampleKind::Video => {
                if self.glsl_oes_video {
                    format!("sample2dOES({}, {})", t, c)
                } else {
                    format!("sample2d({}, {})", t, c)
                }
            }
            SampleKind::Implicit | SampleKind::Lod => {
                let lod = if s.kind == SampleKind::Lod { Some(self.expr(f, s.lod)) } else { None };
                match s.tex_type {
                    TextureType::TextureCube | TextureType::TextureCubeArray => match lod {
                        Some(l) => format!("samplecube_lod({}, {}, {})", t, c, l),
                        None => format!("samplecube({}, {})", t, c),
                    },
                    TextureType::Texture2dArray | TextureType::TextureDepthArray | TextureType::Texture1dArray | TextureType::Texture3d | TextureType::Texture3dArray => match lod {
                        Some(l) => format!("textureLod({}, {}, {})", t, c, l),
                        None => format!("texture({}, {})", t, c),
                    },
                    TextureType::TextureDepth => match lod {
                        Some(l) => format!("vec4(textureLod({}, vec3({}, 0.0), {}))", t, c, l),
                        None => format!("vec4(texture({}, vec3({}, 0.0)))", t, c),
                    },
                    _ => match lod {
                        Some(l) => format!("sample2d_lod({}, {}, {})", t, c, l),
                        None => format!("sample2d({}, {})", t, c),
                    },
                }
            }
        };
        let r = match (&f.exprs[s.tex as usize].kind, s.tex_type) {
            (ExprKind::Global(g), TextureType::Texture2d) => match self.pick_twins.iter().find(|(pg, _)| pg == g) {
                Some((_, bit)) => format!("_mp_pick_sample({}u, {}__pick, {}, {})", bit, self.plain_global_name(*g), r, c),
                None => r,
            },
            _ => r,
        };
        if ty == TY_F32 {
            return format!("{}.x", r);
        }
        r
    }


    // ---- HLSL ----

    /// The expressions HLSL spells its own way (None: the shared spelling).
    fn hlsl_expr(&mut self, f: &IrFunction, e: ExprId) -> Option<String> {
        let ex = &f.exprs[e as usize];
        let ty = ex.ty;
        Some(match &ex.kind {
            ExprKind::Binary(op, a, b) => {
                let at = f.exprs[*a as usize].ty;
                let bt = f.exprs[*b as usize].ty;
                let sa = self.expr(f, *a);
                let sb = self.expr(f, *b);
                if *op == BinOp::Mul && (self.is_mat(at) || self.is_mat(bt)) {
                    return Some(format!("mul({}, {})", sb, sa));
                }
                if self.m.scalar_of(ty) == Some(IrScalar::Bool) && self.m.scalar_of(at) == Some(IrScalar::Bool) {
                    match op {
                        BinOp::BitAnd => return Some(format!("({} && {})", sa, sb)),
                        BinOp::BitOr => return Some(format!("({} || {})", sa, sb)),
                        BinOp::BitXor => return Some(format!("({} != {})", sa, sb)),
                        _ => {}
                    }
                }
                format!("({} {} {})", sa, op.text(), sb)
            }
            ExprKind::Convert(a) => format!("(({})({}))", self.ty_name(ty), self.expr(f, *a)),
            ExprKind::Bitcast(a) => {
                let s = self.expr(f, *a);
                match self.m.scalar_of(ty) {
                    Some(IrScalar::U32) => format!("asuint({})", s),
                    Some(IrScalar::I32) => format!("asint({})", s),
                    Some(IrScalar::F32 | IrScalar::F16) => format!("asfloat({})", s),
                    _ => format!("(({})({}))", self.ty_name(ty), s),
                }
            }
            ExprKind::Construct(args) => self.hlsl_construct(f, args, ty),
            ExprKind::Builtin(op, args) => return self.hlsl_builtin(f, *op, args, ty),
            ExprKind::Select(c, t, fl) => {
                let cs = self.expr(f, *c);
                let ts = self.expr(f, *t);
                let fs = self.expr(f, *fl);
                format!("({} ? {} : {})", cs, ts, fs)
            }
            ExprKind::Sample(smp) => {
                let t = self.expr(f, smp.tex);
                let c = self.expr(f, smp.coord);
                let depth = matches!(smp.tex_type, TextureType::TextureDepth | TextureType::TextureDepthArray);
                let r = match smp.kind {
                    SampleKind::CompareLevel0 => {
                        let r = self.expr(f, smp.reference);
                        return Some(format!("{}.SampleCmpLevelZero(_s{}, {}, {})", t, smp.sampler, c, r));
                    }
                    SampleKind::Grad => {
                        let dx = self.expr(f, smp.dx);
                        let dy = self.expr(f, smp.dy);
                        format!("{}.SampleGrad(_s{}, {}, {}, {})", t, smp.sampler, c, dx, dy)
                    }
                    SampleKind::Lod => {
                        let l = self.expr(f, smp.lod);
                        format!("{}.SampleLevel(_s{}, {}, {})", t, smp.sampler, c, l)
                    }
                    // D3D11 samples level 0 (no implicit derivatives), as
                    // the platform's textures have one level.
                    SampleKind::Implicit | SampleKind::Video => format!("{}.SampleLevel(_s{}, {}, 0.0)", t, smp.sampler, c),
                };
                // The pick variant: a 2D texture's sample also reads its twin.
                let r = match (&f.exprs[smp.tex as usize].kind, smp.tex_type) {
                    (ExprKind::Global(g), TextureType::Texture2d) => match self.pick_twins.iter().find(|(pg, _)| pg == g) {
                        Some((_, bit)) => format!("_mp_pick_sample({}u, {}__pick, {}, {})", bit, self.plain_global_name(*g), r, c),
                        None => r,
                    },
                    _ => r,
                };
                if ty == TY_F32 {
                    if depth {
                        return Some(r);
                    }
                    return Some(format!("{}.x", r));
                }
                if depth {
                    return Some(format!("((float4)({}))", r));
                }
                r
            }
            ExprKind::TexSize(t) => {
                let ts = self.expr(f, *t);
                if self.m.scalar_of(ty) == Some(IrScalar::U32) {
                    format!("((uint2)_mpTexSize2D({}))", ts)
                } else {
                    format!("_mpTexSize2D({})", ts)
                }
            }
            ExprKind::TexLoad(t, c, layer, lod) => {
                let tt = f.exprs[*t as usize].ty;
                let depth = matches!(self.m.get(tt), IrTy::Texture(TextureType::TextureDepth));
                let ts = self.expr(f, *t);
                let cs = self.expr(f, *c);
                let ls = self.expr(f, *lod);
                let r = if *layer != NO_EXPR {
                    let l = self.expr(f, *layer);
                    format!("{}.Load(int4({}, {}, {}))", ts, cs, l, ls)
                } else {
                    format!("{}.Load(int3({}, {}))", ts, cs, ls)
                };
                if ty == TY_F32 && !depth {
                    format!("{}.x", r)
                } else {
                    r
                }
            }
            ExprKind::PassMatrix(which) => format!("unibuf_draw_pass.{}", self.ident(pass_matrix_name(*which).to_string())),
            ExprKind::DepthClip(_, color, _) => self.expr(f, *color),
            ExprKind::EntryIn(i) => {
                let entry = f.entry.as_ref().unwrap();
                format!("input.{}", entry.inputs[*i as usize].name)
            }
            ExprKind::EntryOut(i) => {
                let entry = f.entry.as_ref().unwrap();
                format!("out_data.{}", entry.outputs[*i as usize].name)
            }
            _ => return None,
        })
    }

    fn hlsl_construct(&mut self, f: &IrFunction, args: &[ExprId], ty: TyId) -> String {
        let mut parts = Vec::new();
        for a in args {
            parts.push(self.expr(f, *a));
        }
        match self.m.get(ty).clone() {
            IrTy::Array(..) => {
                if parts.is_empty() {
                    return self.hlsl_zero_init(ty);
                }
                format!("{{{}}}", parts.join(", "))
            }
            IrTy::Struct(s) => {
                if parts.is_empty() {
                    return format!("(({})0)", self.ty_name(ty));
                }
                if !self.hlsl_ctors.contains(&s) {
                    self.hlsl_ctors.push(s);
                }
                format!("_mp_make_{}({})", self.struct_name(s), parts.join(", "))
            }
            IrTy::Mat(c, r) => {
                let name = self.ty_name(ty);
                if parts.is_empty() {
                    return format!("(({})0)", name);
                }
                if parts.len() == 1 {
                    let at = f.exprs[args[0] as usize].ty;
                    if self.is_mat(at) {
                        return format!("(({})({}))", name, parts[0]);
                    }
                    // A scalar on the diagonal.
                    let mut cells = Vec::new();
                    for i in 0..c {
                        for j in 0..r {
                            cells.push(if i == j { parts[0].clone() } else { "0.0".to_string() });
                        }
                    }
                    return format!("{}({})", name, cells.join(", "));
                }
                format!("{}({})", name, parts.join(", "))
            }
            IrTy::Vec(..) => {
                let name = self.ty_name(ty);
                if parts.is_empty() {
                    return format!("(({})0)", name);
                }
                if parts.len() == 1 && self.m.lanes(f.exprs[args[0] as usize].ty) <= 1 {
                    return format!("(({})({}))", name, parts[0]);
                }
                format!("{}({})", name, parts.join(", "))
            }
            _ => {
                let name = self.ty_name(ty);
                if parts.is_empty() {
                    return format!("(({})0)", name);
                }
                format!("(({})({}))", name, parts[0])
            }
        }
    }

    /// A zero in a declaration's initializer (arrays as initializer lists).
    pub fn hlsl_zero_init(&self, ty: TyId) -> String {
        match self.m.get(ty) {
            IrTy::Array(e, n) => {
                let mut parts = Vec::new();
                for _ in 0..*n {
                    parts.push(self.hlsl_zero_init(*e));
                }
                format!("{{{}}}", parts.join(", "))
            }
            _ => format!("(({})0)", self.ty_name(ty)),
        }
    }

    /// Assignments HLSL cannot write as one: an array from its elements.
    fn hlsl_assign(&mut self, f: &IrFunction, p: ExprId, v: ExprId, indent: usize, out: &mut String) -> bool {
        let IrTy::Array(..) = self.m.get(f.exprs[p as usize].ty) else { return false };
        let ExprKind::Construct(args) = &f.exprs[v as usize].kind else { return false };
        let place = self.expr(f, p);
        for (k, a) in args.iter().enumerate() {
            let val = self.expr(f, *a);
            Self::pad(out, indent);
            writeln!(out, "{}[{}] = {};", place, k, val).ok();
        }
        true
    }

    fn hlsl_builtin(&mut self, f: &IrFunction, op: Builtin, args: &[ExprId], ty: TyId) -> Option<String> {
        let name = match op {
            Builtin::Atan2 => "atan2",
            Builtin::Fract => "frac",
            Builtin::Mix => "lerp",
            Builtin::InverseSqrt => "rsqrt",
            Builtin::Dpdx => "ddx",
            Builtin::Dpdy => "ddy",
            Builtin::Fma => "mad",
            Builtin::Inverse => {
                self.needs_inverse = true;
                "_mp_inverse4"
            }
            Builtin::Sign => {
                let a = self.expr(f, args[0]);
                return Some(format!("(({})sign({}))", self.ty_name(ty), a));
            }
            Builtin::FMod => {
                let a = self.arg_as(f, args[0], ty);
                let b = self.arg_as(f, args[1], ty);
                return Some(format!("fmod({}, {})", a, b));
            }
            Builtin::Asinh => {
                let a = self.expr(f, args[0]);
                return Some(format!("_mp_asinh({})", a));
            }
            Builtin::Acosh => {
                let a = self.expr(f, args[0]);
                return Some(format!("_mp_acosh({})", a));
            }
            Builtin::Atanh => {
                let a = self.expr(f, args[0]);
                return Some(format!("_mp_atanh({})", a));
            }
            Builtin::Unpack2f16 | Builtin::Unpack4u8 => {
                let a = self.expr(f, args[0]);
                let at = f.exprs[args[0] as usize].ty;
                let n = if op == Builtin::Unpack2f16 { "_mp_unpack2f16" } else { "_mp_unpack4u8" };
                let a = if at == TY_F32 { a } else { format!("asfloat({})", a) };
                return Some(format!("{}({})", n, a));
            }
            Builtin::Unpack2x16Float => "_mp_unpack_half2x16",
            Builtin::Unpack2x16Unorm => "_mp_unpack_unorm2x16",
            Builtin::Unpack2x16Snorm => "_mp_unpack_snorm2x16",
            Builtin::Unpack4x8Unorm => "_mp_unpack_unorm4x8",
            Builtin::Unpack4x8Snorm => "_mp_unpack_snorm4x8",
            _ => return None,
        };
        let componentwise = matches!(op, Builtin::Mix | Builtin::Atan2 | Builtin::Fma);
        let mut parts = Vec::new();
        for a in args {
            if componentwise {
                parts.push(self.arg_as(f, *a, ty));
            } else {
                parts.push(self.expr(f, *a));
            }
        }
        Some(format!("{}({})", name, parts.join(", ")))
    }


    // ---- Metal ----

    /// The expressions MSL spells its own way (None: the shared spelling).
    fn metal_expr(&mut self, f: &IrFunction, e: ExprId) -> Option<String> {
        let ex = &f.exprs[e as usize];
        let ty = ex.ty;
        Some(match &ex.kind {
            ExprKind::Binary(op, a, b) => {
                let at = f.exprs[*a as usize].ty;
                let sa = self.expr(f, *a);
                let sb = self.expr(f, *b);
                if *op == BinOp::Rem && self.m.scalar_of(ty).map(|s| s.is_float()).unwrap_or(false) {
                    let sa = if self.m.lanes(at) < self.m.lanes(ty) { format!("{}({})", self.ty_name(ty), sa) } else { sa };
                    return Some(format!("fmod({}, {})", sa, sb));
                }
                if self.m.scalar_of(ty) == Some(IrScalar::Bool) && self.m.scalar_of(at) == Some(IrScalar::Bool) {
                    let o = match op {
                        BinOp::BitAnd | BinOp::LogicAnd => Some("&&"),
                        BinOp::BitOr | BinOp::LogicOr => Some("||"),
                        BinOp::BitXor => Some("!="),
                        _ => None,
                    };
                    if let Some(o) = o {
                        if self.is_vec(ty) {
                            let n = self.m.lanes(ty);
                            let mut parts = Vec::new();
                            for k in 0..n {
                                let c = ['x', 'y', 'z', 'w'][k as usize];
                                parts.push(format!("({}.{} {} {}.{})", sa, c, o, sb, c));
                            }
                            return Some(format!("bool{}({})", n, parts.join(", ")));
                        }
                        return Some(format!("({} {} {})", sa, o, sb));
                    }
                }
                format!("({} {} {})", sa, op.text(), sb)
            }
            ExprKind::Bitcast(a) => format!("as_type<{}>({})", self.ty_name(ty), self.expr(f, *a)),
            ExprKind::Construct(args) => {
                let mut parts = Vec::new();
                for a in args {
                    parts.push(self.expr(f, *a));
                }
                let name = self.ty_name(ty);
                match self.m.get(ty) {
                    IrTy::Struct(_) | IrTy::Array(..) => format!("{}{{{}}}", name, parts.join(", ")),
                    _ => {
                        if parts.is_empty() {
                            format!("{}(0)", name)
                        } else {
                            format!("{}({})", name, parts.join(", "))
                        }
                    }
                }
            }
            ExprKind::Builtin(op, args) => return self.metal_builtin(f, *op, args, ty),
            ExprKind::Select(c, t, fl) => {
                let ct = f.exprs[*c as usize].ty;
                let cs = self.expr(f, *c);
                let ts = self.expr(f, *t);
                let fs = self.expr(f, *fl);
                if self.is_vec(ct) {
                    format!("select({}, {}, {})", fs, ts, cs)
                } else {
                    format!("({} ? {} : {})", cs, ts, fs)
                }
            }
            ExprKind::Sample(smp) => {
                let t = self.expr(f, smp.tex);
                let c = self.expr(f, smp.coord);
                let arrayed = matches!(smp.tex_type, TextureType::Texture2dArray | TextureType::TextureDepthArray | TextureType::Texture1dArray);
                let coord = if arrayed { format!("({}).xy, uint(({}).z + 0.5)", c, c) } else { c.clone() };
                let depth = matches!(smp.tex_type, TextureType::TextureDepth | TextureType::TextureDepthArray);
                let r = match smp.kind {
                    SampleKind::CompareLevel0 => {
                        let r = self.expr(f, smp.reference);
                        return Some(format!("{}.sample_compare(_s{}, {}, {})", t, smp.sampler, coord, r));
                    }
                    SampleKind::Grad => {
                        let dx = self.expr(f, smp.dx);
                        let dy = self.expr(f, smp.dy);
                        format!("{}.sample(_s{}, {}, gradient2d({}, {}))", t, smp.sampler, coord, dx, dy)
                    }
                    SampleKind::Lod => {
                        let l = self.expr(f, smp.lod);
                        format!("{}.sample(_s{}, {}, level({}))", t, smp.sampler, coord, l)
                    }
                    SampleKind::Implicit | SampleKind::Video => format!("{}.sample(_s{}, {})", t, smp.sampler, coord),
                };
                // The pick variant: a 2D texture's sample also reads its twin.
                let r = match (&f.exprs[smp.tex as usize].kind, smp.tex_type) {
                    (ExprKind::Global(g), TextureType::Texture2d) => match self.pick_twins.iter().find(|(pg, _)| pg == g) {
                        Some((_, bit)) => {
                            let twin = format!("_mp.{}__pick", self.plain_global_name(*g));
                            format!("_mp_pick_sample(_mp, {}u, {}, {}, {})", bit, twin, r, c)
                        }
                        None => r,
                    },
                    _ => r,
                };
                if ty == TY_F32 {
                    if depth {
                        return Some(r);
                    }
                    return Some(format!("{}.x", r));
                }
                if depth {
                    return Some(format!("float4({})", r));
                }
                r
            }
            ExprKind::TexSize(t) => {
                let ts = self.expr(f, *t);
                if self.m.scalar_of(ty) == Some(IrScalar::U32) {
                    format!("uint2({0}.get_width(), {0}.get_height())", ts)
                } else {
                    format!("float2({0}.get_width(), {0}.get_height())", ts)
                }
            }
            ExprKind::TexLoad(t, c, layer, lod) => {
                let tt = f.exprs[*t as usize].ty;
                let depth = matches!(self.m.get(tt), IrTy::Texture(TextureType::TextureDepth | TextureType::TextureDepthArray));
                let ts = self.expr(f, *t);
                let cs = self.expr(f, *c);
                let ls = self.expr(f, *lod);
                let r = if *layer != NO_EXPR {
                    let l = self.expr(f, *layer);
                    format!("{}.read(uint2({}), uint({}), uint({}))", ts, cs, l, ls)
                } else {
                    format!("{}.read(uint2({}), uint({}))", ts, cs, ls)
                };
                if ty == TY_F32 && !depth {
                    format!("{}.x", r)
                } else {
                    r
                }
            }
            ExprKind::PassMatrix(which) => format!("(*_mp.unibuf_draw_pass).{}", self.ident(pass_matrix_name(*which).to_string())),
            ExprKind::DepthClip(_, color, _) => self.expr(f, *color),
            ExprKind::InstanceIndex => "_mp._mp_instance_index".to_string(),
            ExprKind::EntryIn(i) => {
                let entry = f.entry.as_ref().unwrap();
                let io = &entry.inputs[*i as usize];
                match entry.stage {
                    IrStage::Vertex | IrStage::Compute => io.name.clone(),
                    IrStage::Fragment => format!("input.{}", io.name),
                }
            }
            ExprKind::EntryOut(i) => {
                let entry = f.entry.as_ref().unwrap();
                format!("out_data.{}", entry.outputs[*i as usize].name)
            }
            _ => return None,
        })
    }

    fn metal_builtin(&mut self, f: &IrFunction, op: Builtin, args: &[ExprId], ty: TyId) -> Option<String> {
        let name = match op {
            Builtin::InverseSqrt => "rsqrt",
            Builtin::Dpdx => "dfdx",
            Builtin::Dpdy => "dfdy",
            Builtin::Inverse => "_mp_inverse",
            Builtin::FMod => {
                let a = self.arg_as(f, args[0], ty);
                let b = self.arg_as(f, args[1], ty);
                return Some(format!("fmod({}, {})", a, b));
            }
            Builtin::Unpack2f16 | Builtin::Unpack4u8 => {
                let a = self.expr(f, args[0]);
                let at = f.exprs[args[0] as usize].ty;
                let n = if op == Builtin::Unpack2f16 { "_mp_unpack2f16" } else { "_mp_unpack4u8" };
                let a = if at == TY_F32 { a } else { format!("as_type<float>({})", a) };
                return Some(format!("{}({})", n, a));
            }
            Builtin::Unpack2x16Float => {
                let a = self.expr(f, args[0]);
                return Some(format!("float2(as_type<half2>({}))", a));
            }
            Builtin::Unpack2x16Unorm => "unpack_unorm2x16_to_float",
            Builtin::Unpack2x16Snorm => "unpack_snorm2x16_to_float",
            Builtin::Unpack4x8Unorm => "unpack_unorm4x8_to_float",
            Builtin::Unpack4x8Snorm => "unpack_snorm4x8_to_float",
            _ => return None,
        };
        let mut parts = Vec::new();
        for a in args {
            parts.push(self.expr(f, *a));
        }
        Some(format!("{}({})", name, parts.join(", ")))
    }

    // ---- statements ----

    pub fn stmts(&mut self, f: &IrFunction, stmts: &[Stmt], indent: usize, out: &mut String) {
        for s in stmts {
            self.stmt(f, s, indent, out);
        }
    }

    fn pad(out: &mut String, indent: usize) {
        for _ in 0..indent {
            out.push_str("    ");
        }
    }

    fn stmt(&mut self, f: &IrFunction, s: &Stmt, indent: usize, out: &mut String) {
        let glsl = self.c_like();
        match s {
            Stmt::Local(l, init) => {
                let local = &f.locals[*l as usize];
                if local.ty == TY_VOID {
                    return;
                }
                let name = self.local_name(f, *l);
                Self::pad(out, indent);
                let missing = *init == NO_EXPR || matches!(f.exprs[*init as usize].kind, ExprKind::Error);
                if glsl {
                    let decl = self.decl(local.ty, &name);
                    if missing {
                        let z = if self.hlsl() {
                            self.hlsl_zero_init(local.ty)
                        } else if self.metal() {
                            format!("{}{{}}", self.ty_name(local.ty))
                        } else {
                            self.glsl_zero(local.ty)
                        };
                        writeln!(out, "{} = {};", decl, z).ok();
                    } else {
                        let v = self.expr(f, *init);
                        writeln!(out, "{} = {};", decl, v).ok();
                    }
                    return;
                }
                let ty = self.ty_name(local.ty);
                let composite = matches!(self.m.get(local.ty), IrTy::Vec(..) | IrTy::Mat(..) | IrTy::Struct(_) | IrTy::Array(..));
                let kw = if local.mutable || composite { "var" } else { "let" };
                if missing {
                    if kw == "var" {
                        writeln!(out, "var {}: {};", name, ty).ok();
                    } else {
                        writeln!(out, "let {}: {} = {}();", name, ty, ty).ok();
                    }
                } else {
                    let v = self.expr(f, *init);
                    writeln!(out, "{} {}: {} = {};", kw, name, ty, v).ok();
                }
            }
            Stmt::Assign(p, v) => {
                if self.hlsl() && self.hlsl_assign(f, *p, *v, indent, out) {
                    return;
                }
                if !glsl {
                    if let ExprKind::Swizzle(base, lanes, n) = f.exprs[*p as usize].kind {
                        if n > 1 {
                            // WGSL writes one lane at a time.
                            let vt = f.exprs[*v as usize].ty;
                            let val = self.expr(f, *v);
                            let b = self.expr(f, base);
                            Self::pad(out, indent);
                            write!(out, "{{ let _w: {} = {};", self.ty_name(vt), val).ok();
                            for k in 0..n as usize {
                                let c = ['x', 'y', 'z', 'w'][lanes[k] as usize & 3];
                                let src = ['x', 'y', 'z', 'w'][k];
                                write!(out, " {}.{} = _w.{};", b, c, src).ok();
                            }
                            out.push_str(" }\n");
                            return;
                        }
                    }
                }
                let p = self.expr(f, *p);
                let v = self.expr(f, *v);
                Self::pad(out, indent);
                writeln!(out, "{} = {};", p, v).ok();
            }
            Stmt::CompoundAssign(p, op, v) => {
                let pt = f.exprs[*p as usize].ty;
                let place = self.expr(f, *p);
                let mut val = self.expr(f, *v);
                Self::pad(out, indent);
                if self.hlsl() && *op == BinOp::Mul && (self.is_mat(pt) || self.is_mat(f.exprs[*v as usize].ty)) {
                    writeln!(out, "{} = mul({}, {});", place, val, place).ok();
                    return;
                }
                if self.metal() && *op == BinOp::Rem && self.m.scalar_of(pt).map(|s| s.is_float()).unwrap_or(false) {
                    writeln!(out, "{} = fmod({}, {});", place, place, val).ok();
                    return;
                }
                if self.glsl() && *op == BinOp::Rem && self.m.scalar_of(pt).map(|s| s.is_float()).unwrap_or(false) {
                    writeln!(out, "{} = mod({}, {});", place, place, val).ok();
                    return;
                }
                if matches!(op, BinOp::Shl | BinOp::Shr) && !glsl {
                    let n = self.m.lanes(pt);
                    if n > 1 && self.m.lanes(f.exprs[*v as usize].ty) == 1 {
                        val = format!("vec{}u({})", n, val);
                    }
                }
                writeln!(out, "{} {}= {};", place, op.text(), val).ok();
            }
            Stmt::Expr(e) => {
                if matches!(f.exprs[*e as usize].kind, ExprKind::Nop | ExprKind::Error) {
                    return;
                }
                let ty = f.exprs[*e as usize].ty;
                let v = self.expr(f, *e);
                Self::pad(out, indent);
                let is_call = matches!(f.exprs[*e as usize].kind, ExprKind::Call(..));
                if glsl || (ty == TY_VOID && is_call) {
                    writeln!(out, "{};", v).ok();
                } else {
                    writeln!(out, "_ = {};", v).ok();
                }
            }
            Stmt::If(c, a, b) => {
                let c = self.expr(f, *c);
                Self::pad(out, indent);
                if b.is_empty() && a.len() == 1 && matches!(a[0], Stmt::Break) {
                    writeln!(out, "if ({}) {{ break; }}", c).ok();
                    return;
                }
                writeln!(out, "if ({}) {{", c).ok();
                self.stmts(f, a, indent + 1, out);
                if !b.is_empty() {
                    Self::pad(out, indent);
                    out.push_str("} else {\n");
                    self.stmts(f, b, indent + 1, out);
                }
                Self::pad(out, indent);
                out.push_str("}\n");
            }
            Stmt::For(var, start, end, body) => {
                let v = self.local_name(f, *var);
                let s = self.expr(f, *start);
                let e = self.expr(f, *end);
                Self::pad(out, indent);
                if self.hlsl() && !matches!(f.exprs[*end as usize].kind, ExprKind::Lit(_)) {
                    // A runtime bound: FXC would try to unroll a loop with a
                    // gradient in it, and fail.
                    out.push_str("[loop] ");
                }
                if self.hlsl() || self.metal() {
                    let t = self.ty_name(f.locals[*var as usize].ty);
                    writeln!(out, "for ({3} {0} = {1}; {0} < {2}; {0}++) {{", v, s, e, t).ok();
                } else if glsl {
                    writeln!(out, "for (uint {0} = {1}; {0} < {2}; {0}++) {{", v, s, e).ok();
                } else {
                    writeln!(out, "for (var {0}: u32 = {1}; {0} < {2}; {0}++) {{", v, s, e).ok();
                }
                self.stmts(f, body, indent + 1, out);
                Self::pad(out, indent);
                out.push_str("}\n");
            }
            Stmt::Loop(body) => {
                Self::pad(out, indent);
                if self.hlsl() {
                    out.push_str("[loop] while (true) {\n");
                } else if glsl {
                    out.push_str("while (true) {\n");
                } else {
                    out.push_str("loop {\n");
                }
                self.stmts(f, body, indent + 1, out);
                Self::pad(out, indent);
                out.push_str("}\n");
            }
            Stmt::Break => {
                Self::pad(out, indent);
                out.push_str("break;\n");
            }
            Stmt::Continue => {
                Self::pad(out, indent);
                out.push_str("continue;\n");
            }
            Stmt::Discard => {
                Self::pad(out, indent);
                out.push_str(if self.metal() { "discard_fragment();\n" } else { "discard;\n" });
            }
            Stmt::Return(e) => {
                Self::pad(out, indent);
                if !self.glsl() && f.entry.as_ref().map(|en| !en.outputs.is_empty()).unwrap_or(false) {
                    out.push_str("return out_data;\n");
                } else if *e == NO_EXPR || f.entry.is_some() {
                    out.push_str("return;\n");
                } else {
                    let v = self.expr(f, *e);
                    writeln!(out, "return {};", v).ok();
                }
            }
        }
    }

    /// A function's text (not an entry point).
    pub fn function(&mut self, fi: FuncId, out: &mut String) {
        let m = self.m;
        let f = &m.functions[fi as usize];
        let mut params = Vec::new();
        for (k, p) in f.params.iter().enumerate() {
            let n = self.param_name(f, k as u32);
            match self.dialect {
                Dialect::Wgsl => {
                    if p.inout {
                        params.push(format!("{}: ptr<function, {}>", n, self.ty_name(p.ty)));
                    } else {
                        params.push(format!("{}: {}", n, self.ty_name(p.ty)));
                    }
                }
                Dialect::Glsl | Dialect::Hlsl => {
                    let d = self.decl(p.ty, &n);
                    if p.inout {
                        params.push(format!("inout {}", d));
                    } else {
                        params.push(d);
                    }
                }
                Dialect::Metal => {
                    if p.inout {
                        params.push(format!("thread {}& {}", self.ty_name(p.ty), n));
                    } else {
                        params.push(self.decl(p.ty, &n));
                    }
                }
            }
        }
        if self.metal() {
            params.insert(0, "thread Mp &_mp".to_string());
        }
        match self.dialect {
            Dialect::Wgsl => {
                let ret = if f.ret == TY_VOID { String::new() } else { format!(" -> {}", self.ty_name(f.ret)) };
                writeln!(out, "fn {}({}){} {{", f.name.replace('$', "_mp_generic_"), params.join(", "), ret).ok();
            }
            Dialect::Glsl | Dialect::Hlsl | Dialect::Metal => {
                writeln!(out, "{} {}({}) {{", self.ty_name(f.ret), f.name.replace('$', "_mp_generic_"), params.join(", ")).ok();
            }
        }
        self.stmts(f, &f.body, 1, out);
        out.push_str("}\n\n");
    }
}

pub fn pass_matrix_name(which: PassMatrix) -> &'static str {
    match which {
        PassMatrix::CameraProjection => "camera_projection",
        PassMatrix::CameraView => "camera_view",
        PassMatrix::DepthProjection => "depth_projection",
        PassMatrix::DepthView => "depth_view",
        PassMatrix::CameraInv => "camera_inv",
    }
}

pub fn hlsl_texture_type(tt: TextureType) -> &'static str {
    match tt {
        TextureType::Texture1d => "Texture1D",
        TextureType::Texture1dArray => "Texture1DArray",
        TextureType::Texture2d => "Texture2D",
        TextureType::Texture2dArray => "Texture2DArray",
        TextureType::Texture3d | TextureType::Texture3dArray => "Texture3D",
        TextureType::TextureCube => "TextureCube",
        TextureType::TextureCubeArray => "TextureCubeArray",
        TextureType::TextureDepth => "Texture2D<float>",
        TextureType::TextureDepthArray => "Texture2DArray",
        TextureType::TextureVideo => "Texture2D",
    }
}

/// HLSL's reserved words a user-given struct or field name may hit.
pub const HLSL_RESERVED: &[&str] = &[
    "AppendStructuredBuffer", "Buffer", "ByteAddressBuffer", "ConsumeStructuredBuffer", "InputPatch", "OutputPatch", "RWBuffer",
    "RWByteAddressBuffer", "RWStructuredBuffer", "RWTexture1D", "RWTexture2D", "RWTexture3D", "SamplerComparisonState", "SamplerState",
    "StructuredBuffer", "Texture1D", "Texture2D", "Texture3D", "TextureCube", "asm", "asm_fragment", "auto", "bool", "break", "case", "catch",
    "cbuffer", "centroid", "char", "class", "column_major", "compile", "compile_fragment", "const", "const_cast", "continue", "default",
    "delete", "discard", "do", "double", "dword", "dynamic_cast", "else", "enum", "explicit", "export", "extern", "false", "float", "for",
    "friend", "fxgroup", "goto", "groupshared", "half", "if", "in", "inline", "inout", "int", "interface", "line", "lineadj", "linear",
    "long", "matrix", "min10float", "min12int", "min16float", "min16int", "min16uint", "mutable", "namespace", "new", "nointerpolation",
    "noperspective", "operator", "out", "packoffset", "pass", "pixelfragment", "point", "precise", "private", "protected", "public",
    "register", "reinterpret_cast", "return", "row_major", "sample", "sampler", "shared", "short", "signed", "sizeof", "snorm", "stateblock",
    "static", "static_cast", "string", "struct", "switch", "tbuffer", "technique", "template", "texture", "this", "throw", "triangle",
    "triangleadj", "true", "try", "typedef", "typename", "uint", "uniform", "union", "unorm", "unsigned", "using", "vector", "vertexfragment",
    "virtual", "void", "volatile", "while",
];

/// The name of a word buffer global (the instance records, or Metal's
/// geometry).
pub fn word_buffer_name(name: LiveId) -> &'static str {
    if name == id!(_mp_geom) {
        "_mp_geom"
    } else {
        "_mp_inst"
    }
}

pub fn metal_texture_type(tt: TextureType) -> &'static str {
    match tt {
        TextureType::Texture1d => "texture1d<float>",
        TextureType::Texture1dArray => "texture1d_array<float>",
        TextureType::Texture2d | TextureType::TextureVideo => "texture2d<float>",
        TextureType::Texture2dArray => "texture2d_array<float>",
        TextureType::Texture3d | TextureType::Texture3dArray => "texture3d<float>",
        TextureType::TextureCube => "texturecube<float>",
        TextureType::TextureCubeArray => "texturecube_array<float>",
        TextureType::TextureDepth => "depth2d<float>",
        TextureType::TextureDepthArray => "depth2d_array<float>",
    }
}

/// MSL's (C++14's) reserved words a user-given struct or field name may hit.
pub const METAL_RESERVED: &[&str] = &[
    "alignas", "alignof", "and", "asm", "auto", "bool", "break", "case", "catch", "char", "class", "const", "constant", "constexpr",
    "const_cast", "continue", "decltype", "default", "delete", "device", "do", "double", "dynamic_cast", "else", "enum", "explicit",
    "export", "extern", "false", "float", "for", "fragment", "friend", "goto", "half", "if", "inline", "int", "kernel", "long", "mutable",
    "namespace", "new", "noexcept", "not", "nullptr", "operator", "or", "private", "protected", "public", "ray_data", "register",
    "reinterpret_cast", "return", "sampler", "short", "signed", "sizeof", "static", "static_assert", "static_cast", "struct", "switch",
    "template", "texture", "this", "thread", "threadgroup", "threadgroup_imageblock", "throw", "true", "try", "typedef", "typeid",
    "typename", "uint", "union", "unsigned", "using", "vertex", "virtual", "void", "volatile", "while", "xor", "metal", "uchar", "ushort",
];

pub fn wgsl_texture_type(tt: TextureType) -> &'static str {
    match tt {
        TextureType::Texture1d => "texture_2d<f32>",
        TextureType::Texture1dArray => "texture_2d_array<f32>",
        TextureType::Texture2d => "texture_2d<f32>",
        TextureType::Texture2dArray => "texture_2d_array<f32>",
        TextureType::Texture3d => "texture_3d<f32>",
        TextureType::Texture3dArray => "texture_3d<f32>",
        TextureType::TextureCube => "texture_cube<f32>",
        TextureType::TextureCubeArray => "texture_cube_array<f32>",
        TextureType::TextureDepth => "texture_depth_2d",
        TextureType::TextureDepthArray => "texture_depth_2d_array",
        TextureType::TextureVideo => "texture_2d<f32>",
    }
}

/// The functions `root` reaches, callees before callers (GLSL and HLSL need
/// a function declared before its first call).
pub fn callees_first(m: &IrModule, roots: &[FuncId]) -> Vec<FuncId> {
    let mut state = vec![0u8; m.functions.len()];
    let mut order = Vec::new();
    for r in roots {
        visit(m, *r, &mut state, &mut order);
    }
    order
}

fn visit(m: &IrModule, f: FuncId, state: &mut Vec<u8>, order: &mut Vec<FuncId>) {
    if state[f as usize] != 0 {
        return;
    }
    state[f as usize] = 1;
    let func = &m.functions[f as usize];
    let live = live_exprs(func);
    for (i, e) in func.exprs.iter().enumerate() {
        if !live[i] {
            continue;
        }
        if let ExprKind::Call(c, _) = &e.kind {
            visit(m, *c, state, order);
        }
    }
    state[f as usize] = 2;
    order.push(f);
}

//! Draw shader IR entry points (`vertex_main` / `fragment_main`) every backend
//! prints: geometry and instance records decoded by word offset (the
//! layout `ShaderOutput::instance_record` defines), uniforms copied from
//! their blocks, varyings packed into vec4s carrying only what the fragment
//! stage reads, and the XR variant (view index, view-dependent pass
//! matrices, the XR depth clip).
//!
//! Bindings (set 0) are the Vulkan/WebGPU layout the platform builds its
//! descriptor sets from: dyn uniforms at 2, uniform buffers at their
//! `buffer_index`, the scope uniforms after them, then textures, samplers,
//! the XR depth texture and the instance-record storage buffer.
use makepad_relax_api::relax_api;

use crate::shader_ids::*;
use crate::shader_schema::{ShaderPacked, ShaderTypeKind, ShaderTypeInline};
use crate::shader_ir::*;
use crate::shader_output::*;

use makepad_live_id::{id, LiveId};

/// What the platform binds a draw shader's resources by.
#[relax_api(plain)]
#[derive(Clone, Debug, Default)]
pub struct IrDrawInfo {
    pub dyn_uniform_binding: u32,
    pub texture_binding_base: u32,
    pub sampler_binding_base: u32,
    pub xr_depth_binding: u32,
    pub instance_binding: Option<u32>,
    pub geometry_slots: usize,
    pub instance_slots: usize,
    pub pick_binding: Option<u32>,
    pub pick_texture_base: Option<u32>,
}

#[derive(Clone, Copy, PartialEq)]
enum Source {
    /// An f32 carrying a record word's bits (a vertex attribute lane).
    Attr,
    /// A record word as u32 (the instance storage buffer).
    Word,
    /// An f32 value (a varying lane).
    Numeric,
}

struct RecField {
    io: usize,
    pod: ShaderTypeId,
    slots: usize,
    offset: usize,
}

/// Lanes a field takes in a record (`attribute`: the fetch words; else the
/// varying lanes, compact formats unpacked).
fn field_slots(ty: &ShaderTypeKind, attribute: bool) -> usize {
    match ty {
        ShaderTypeKind::Packed(p) => {
            if attribute {
                p.size_of() / 4
            } else {
                p.logical_slots()
            }
        }
        ShaderTypeKind::Struct { fields, .. } if !attribute && ty.has_compact_format() => {
            let mut n = 0;
            for f in fields {
                n += field_slots(&f.ty.data.ty, false);
            }
            n
        }
        _ => ty.slots(),
    }
}

fn is_int_format(ty: &ShaderTypeKind) -> bool {
    use crate::shader_schema::ShaderVec as V;
    match ty {
        ShaderTypeKind::U32 | ShaderTypeKind::AtomicU32 | ShaderTypeKind::Bool | ShaderTypeKind::I32 | ShaderTypeKind::AtomicI32 => true,
        ShaderTypeKind::Vec(v) => matches!(
            v,
            V::Vec2u | V::Vec3u | V::Vec4u | V::Vec2b | V::Vec3b | V::Vec4b | V::Vec2i | V::Vec3i | V::Vec4i
        ),
        _ => false,
    }
}

fn push_field(types: &impl crate::shader_types::ShaderTypes, out: &mut Vec<RecField>, io_index: usize, pod: ShaderTypeId, attribute: bool, offset: &mut usize) {
    let ty = &types.type_data(pod).ty;
    let slots = field_slots(ty, attribute);
    let int = attribute && is_int_format(ty) && slots > 1;
    if int && (*offset & 3) != 0 {
        *offset += 4 - (*offset & 3);
    }
    out.push(RecField { io: io_index, pod, slots, offset: *offset });
    *offset += slots;
    if int && (*offset & 3) != 0 {
        *offset += 4 - (*offset & 3);
    }
}

fn record_end(fields: &[RecField]) -> usize {
    match fields.last() {
        Some(f) => f.offset + f.slots,
        None => 0,
    }
}

/// The IR global an io entry reads as (created if no function used it).
fn io_global(types: &impl crate::shader_types::ShaderTypes, output: &ShaderOutput, ir: &mut IrModule, io: usize) -> Option<GlobalId> {
    let kind = output.io[io].kind.clone();
    let name = output.io[io].name;
    let pod = output.io[io].ty;
    let gk = crate::shader_output::global_kind_of(&kind)?;
    let ty = match gk {
        IrGlobalKind::Io(IrIo::Texture(t)) => ir.ty(IrTy::Texture(t)),
        _ => ir.pod_ty(types, pod),
    };
    Some(ir.global(gk, name, ty))
}

struct Bld<'a> {
    f: IrFunction,
    m: &'a mut IrModule,
}

impl<'a> Bld<'a> {
    fn e(&mut self, kind: ExprKind, ty: TyId) -> ExprId {
        self.f.expr(kind, ty)
    }
    fn u32(&mut self, v: u32) -> ExprId {
        self.e(ExprKind::Lit(IrLit::U32(v)), TY_U32)
    }
    fn i32(&mut self, v: i32) -> ExprId {
        self.e(ExprKind::Lit(IrLit::I32(v)), TY_I32)
    }
    fn f32(&mut self, v: f32) -> ExprId {
        self.e(ExprKind::Lit(IrLit::F32(v)), TY_F32)
    }
    fn glob(&mut self, g: GlobalId) -> ExprId {
        let ty = self.m.globals[g as usize].ty;
        self.e(ExprKind::Global(g), ty)
    }
    fn lane(&mut self, v: ExprId, lane: u8) -> ExprId {
        let ty = self.f.ty_of(v);
        let s = self.m.scalar_of(ty).unwrap_or(IrScalar::F32);
        let st = self.m.scalar_ty(s);
        self.e(ExprKind::Swizzle(v, [lane, 0, 0, 0], 1), st)
    }
    fn bitcast(&mut self, v: ExprId, ty: TyId) -> ExprId {
        if self.f.ty_of(v) == ty {
            return v;
        }
        self.e(ExprKind::Bitcast(v), ty)
    }
    fn convert(&mut self, v: ExprId, ty: TyId) -> ExprId {
        if self.f.ty_of(v) == ty {
            return v;
        }
        self.e(ExprKind::Convert(v), ty)
    }
    fn bin(&mut self, op: BinOp, a: ExprId, b: ExprId, ty: TyId) -> ExprId {
        self.e(ExprKind::Binary(op, a, b), ty)
    }
    fn assign(&mut self, place: ExprId, v: ExprId) {
        self.f.body.push(Stmt::Assign(place, v));
    }

    /// A record lane converted to `target` (the WGSL text emitter's
    /// `wgsl_convert_scalar_expr`).
    fn scalar(&mut self, src: Source, x: ExprId, target: &ShaderTypeKind) -> ExprId {
        match target {
            ShaderTypeKind::F32 => match src {
                Source::Word => self.bitcast(x, TY_F32),
                _ => x,
            },
            ShaderTypeKind::F16 => {
                let f = match src {
                    Source::Word => self.bitcast(x, TY_F32),
                    _ => x,
                };
                self.convert(f, TY_F16)
            }
            ShaderTypeKind::U32 | ShaderTypeKind::AtomicU32 => match src {
                Source::Attr => self.bitcast(x, TY_U32),
                Source::Word => x,
                Source::Numeric => self.convert(x, TY_U32),
            },
            ShaderTypeKind::I32 | ShaderTypeKind::AtomicI32 => match src {
                Source::Attr | Source::Word => self.bitcast(x, TY_I32),
                Source::Numeric => self.convert(x, TY_I32),
            },
            ShaderTypeKind::Bool => match src {
                Source::Attr => {
                    let u = self.bitcast(x, TY_U32);
                    let z = self.u32(0);
                    self.bin(BinOp::Ne, u, z, TY_BOOL)
                }
                Source::Word => {
                    let z = self.u32(0);
                    self.bin(BinOp::Ne, x, z, TY_BOOL)
                }
                Source::Numeric => {
                    let z = self.f32(0.0);
                    self.bin(BinOp::Ne, x, z, TY_BOOL)
                }
            },
            _ => x,
        }
    }

    /// A record word's bits as u32.
    fn bits(&mut self, src: Source, x: ExprId) -> ExprId {
        match src {
            Source::Word => x,
            _ => self.bitcast(x, TY_U32),
        }
    }

    fn builtin(&mut self, op: Builtin, args: Vec<ExprId>, ty: TyId) -> ExprId {
        self.e(ExprKind::Builtin(op, args), ty)
    }

    /// A compact leaf from its words (the WGSL text emitter's rule).
    fn packed(&mut self, p: ShaderPacked, src: Source, lanes: &[ExprId], at: &mut usize) -> ExprId {
        let v2 = self.m.vec_ty(2, IrScalar::F32);
        let v4 = self.m.vec_ty(4, IrScalar::F32);
        if src == Source::Numeric {
            let n = p.logical_slots();
            let mut parts = Vec::new();
            for _ in 0..n {
                parts.push(take(lanes, at, self));
            }
            let ty = if p.is_vec4() { v4 } else { v2 };
            return self.e(ExprKind::Construct(parts), ty);
        }
        let w0 = take(lanes, at, self);
        let b0 = self.bits(src, w0);
        match p {
            ShaderPacked::F16x4 => {
                let w1 = take(lanes, at, self);
                let b1 = self.bits(src, w1);
                let a = self.builtin(Builtin::Unpack2x16Float, vec![b0], v2);
                let b = self.builtin(Builtin::Unpack2x16Float, vec![b1], v2);
                self.e(ExprKind::Construct(vec![a, b]), v4)
            }
            ShaderPacked::F16x2 => self.builtin(Builtin::Unpack2x16Float, vec![b0], v2),
            ShaderPacked::U16x2 => {
                let mask = self.u32(0xffff);
                let lo = self.bin(BinOp::BitAnd, b0, mask, TY_U32);
                let sh = self.u32(16);
                let hi = self.bin(BinOp::Shr, b0, sh, TY_U32);
                let lo = self.convert(lo, TY_F32);
                let hi = self.convert(hi, TY_F32);
                self.e(ExprKind::Construct(vec![lo, hi]), v2)
            }
            ShaderPacked::I16x2 => {
                let sh = self.u32(16);
                let up = self.bin(BinOp::Shl, b0, sh, TY_U32);
                let up = self.bitcast(up, TY_I32);
                let sh2 = self.u32(16);
                let lo = self.bin(BinOp::Shr, up, sh2, TY_I32);
                let w = self.bitcast(b0, TY_I32);
                let sh3 = self.u32(16);
                let hi = self.bin(BinOp::Shr, w, sh3, TY_I32);
                let lo = self.convert(lo, TY_F32);
                let hi = self.convert(hi, TY_F32);
                self.e(ExprKind::Construct(vec![lo, hi]), v2)
            }
            ShaderPacked::U16x2Norm => self.builtin(Builtin::Unpack2x16Unorm, vec![b0], v2),
            ShaderPacked::I16x2Norm => self.builtin(Builtin::Unpack2x16Snorm, vec![b0], v2),
            ShaderPacked::U8x4Norm => self.builtin(Builtin::Unpack4x8Unorm, vec![b0], v4),
            ShaderPacked::I8x4Norm => self.builtin(Builtin::Unpack4x8Snorm, vec![b0], v4),
        }
    }

    /// A value of `ty` rebuilt from record lanes (the WGSL text emitter's
    /// `wgsl_reconstruct_inline`).
    fn reconstruct(&mut self, types: &impl crate::shader_types::ShaderTypes, ty: &ShaderTypeInline, src: Source, lanes: &[ExprId], at: &mut usize) -> ExprId {
        match &ty.data.ty {
            ShaderTypeKind::Struct { fields, .. } => {
                let mut parts = Vec::new();
                for f in fields {
                    parts.push(self.reconstruct(types, &f.ty, src, lanes, at));
                }
                let t = self.m.pod_inline_ty(types, ty);
                self.e(ExprKind::Construct(parts), t)
            }
            ShaderTypeKind::Vec(v) => {
                let elem = v.elem_ty();
                let mut parts = Vec::new();
                for _ in 0..v.dims() {
                    let x = take(lanes, at, self);
                    parts.push(self.scalar(src, x, &elem));
                }
                let t = self.m.pod_inline_ty(types, ty);
                self.e(ExprKind::Construct(parts), t)
            }
            ShaderTypeKind::Mat(m) => {
                let mut parts = Vec::new();
                for _ in 0..m.dim() {
                    let x = take(lanes, at, self);
                    parts.push(self.scalar(src, x, &ShaderTypeKind::F32));
                }
                let t = self.m.pod_inline_ty(types, ty);
                self.e(ExprKind::Construct(parts), t)
            }
            ShaderTypeKind::Packed(p) => self.packed(*p, src, lanes, at),
            other => {
                let x = take(lanes, at, self);
                self.scalar(src, x, other)
            }
        }
    }

    /// A value's f32 lanes for the varying stream (the WGSL text emitter's
    /// `wgsl_flatten_inline`).
    fn flatten(&mut self, types: &impl crate::shader_types::ShaderTypes, ty: &ShaderTypeInline, v: ExprId, out: &mut Vec<ExprId>) {
        match &ty.data.ty {
            ShaderTypeKind::Struct { fields, .. } => {
                for (i, f) in fields.iter().enumerate() {
                    let ft = self.m.pod_inline_ty(types, &f.ty);
                    let fe = self.e(ExprKind::Field(v, i as u32), ft);
                    self.flatten(types, &f.ty, fe, out);
                }
            }
            ShaderTypeKind::Vec(vt) => {
                for c in 0..vt.dims() {
                    let x = self.lane(v, c as u8);
                    out.push(self.to_f32(x));
                }
            }
            ShaderTypeKind::Mat(m) => {
                let (cols, rows) = m.dims();
                let col_ty = self.m.vec_ty(rows as u8, IrScalar::F32);
                for c in 0..cols {
                    let ci = self.i32(c as i32);
                    let col = self.e(ExprKind::Index(v, ci), col_ty);
                    for r in 0..rows {
                        out.push(self.lane(col, r as u8));
                    }
                }
            }
            ShaderTypeKind::Packed(p) => {
                for c in 0..p.logical_slots() {
                    out.push(self.lane(v, c as u8));
                }
            }
            _ => out.push(self.to_f32(v)),
        }
    }

    fn to_f32(&mut self, x: ExprId) -> ExprId {
        let ty = self.f.ty_of(x);
        if ty == TY_F32 {
            return x;
        }
        if ty == TY_BOOL {
            let one = self.f32(1.0);
            let zero = self.f32(0.0);
            return self.e(ExprKind::Select(x, one, zero), TY_F32);
        }
        self.convert(x, TY_F32)
    }
}

fn take(lanes: &[ExprId], at: &mut usize, b: &mut Bld) -> ExprId {
    let v = match lanes.get(*at) {
        Some(e) => *e,
        None => b.f32(0.0),
    };
    *at += 1;
    v
}

/// Appends the draw entry points (and the XR depth-clip helper) for one
/// variant and assigns the bindings. Returns what the platform binds by.
pub fn build_draw_entries(output: &ShaderOutput, ir: &mut IrModule, types: &impl crate::shader_types::ShaderTypes, xr: bool) -> IrDrawInfo {
    let mut info = IrDrawInfo::default();

    // Records.
    let mut geometry = Vec::new();
    let mut instance = Vec::new();
    let mut offset = 0;
    for (i, io) in output.io.iter().enumerate() {
        if let ShaderIoKind::VertexBuffer = io.kind {
            push_field(types, &mut geometry, i, io.ty, true, &mut offset);
        }
    }
    let mut offset = 0;
    for rust in [false, true] {
        for (i, io) in output.io.iter().enumerate() {
            let wanted = match io.kind {
                ShaderIoKind::DynInstance => !rust,
                ShaderIoKind::RustInstance => rust,
                _ => false,
            };
            if wanted {
                if let Some(lane) = io.instance_lane {
                    let slots = field_slots(&types.type_data(io.ty).ty, true);
                    instance.push(RecField { io: i, pod: io.ty, slots, offset: lane });
                    offset = offset.max(lane + slots);
                } else { push_field(types, &mut instance, i, io.ty, true, &mut offset); }
            }
        }
    }
    info.geometry_slots = record_end(&geometry);
    info.instance_slots = record_end(&instance);

    // Varyings: the instance fields the fragment stage reads, then the
    // varyings.
    let fragment_reads = match ir.function_by_name("io_fragment") {
        Some(f) => {
            let funcs = ir.reachable(f);
            ir.globals_used(&funcs)
        }
        None => vec![false; ir.globals.len()],
    };
    let reads_global = |ir: &IrModule, kind: IrGlobalKind, name: LiveId| -> bool {
        match ir.find_global(kind, name) {
            Some(g) => fragment_reads.get(g as usize).copied().unwrap_or(false),
            None => false,
        }
    };
    let mut varyings = Vec::new();
    let mut offset = 0;
    for rust in [false, true] {
        for (i, io) in output.io.iter().enumerate() {
            let kind = match io.kind {
                ShaderIoKind::DynInstance if !rust => IrGlobalKind::Io(IrIo::DynInstance),
                ShaderIoKind::RustInstance if rust => IrGlobalKind::Io(IrIo::RustInstance),
                _ => continue,
            };
            if reads_global(ir, kind, io.name) {
                push_field(types, &mut varyings, i, io.ty, false, &mut offset);
            }
        }
    }
    for (i, io) in output.io.iter().enumerate() {
        if let ShaderIoKind::Varying = io.kind {
            push_field(types, &mut varyings, i, io.ty, false, &mut offset);
        }
    }
    let varying_slots = record_end(&varyings);

    // Bindings.
    info.dyn_uniform_binding = 2;
    let ub = output.get_uniform_buffer_bindings(types);
    let scope_binding = ub.scope_uniform_buffer_index.map(|v| v as u32);
    let mut max_reserved = info.dyn_uniform_binding;
    for io in &output.io {
        if let (ShaderIoKind::UniformBuffer, Some(idx)) = (&io.kind, io.buffer_index) {
            max_reserved = max_reserved.max(idx as u32);
        }
    }
    if let Some(s) = scope_binding {
        max_reserved = max_reserved.max(s);
    }
    let mut next = max_reserved + 1;

    let mut dyn_fields = Vec::new();
    let mut scope_fields = Vec::new();
    let mut dyn_ios = Vec::new();
    let mut scope_ios = Vec::new();
    for (i, io) in output.io.iter().enumerate() {
        match io.kind {
            ShaderIoKind::Uniform => {
                let t = ir.pod_ty(types, io.ty);
                dyn_fields.push(IrField { name: io.name, ty: t });
                dyn_ios.push(i);
            }
            ShaderIoKind::ScopeUniform => {
                let t = ir.pod_ty(types, io.ty);
                scope_fields.push(IrField { name: io.name, ty: t });
                scope_ios.push(i);
            }
            _ => {}
        }
    }
    let dyn_block = if dyn_fields.is_empty() {
        None
    } else {
        let t = ir.synthetic_struct("MpDynUniforms", dyn_fields);
        let g = ir.global(IrGlobalKind::DynUniformBlock, id!(_mp_dyn_uniforms), t);
        ir.globals[g as usize].binding = Some(info.dyn_uniform_binding);
        Some(g)
    };
    let scope_block = if scope_fields.is_empty() {
        None
    } else {
        let t = ir.synthetic_struct("MpScopeUniforms", scope_fields);
        let g = ir.global(IrGlobalKind::ScopeUniformBlock, id!(_mp_scope_uniforms), t);
        ir.globals[g as usize].binding = Some(scope_binding.unwrap_or(max_reserved + 1));
        Some(g)
    };

    let mut texture_base = None;
    let mut sampler_base = None;
    for i in 0..output.io.len() {
        match output.io[i].kind.clone() {
            ShaderIoKind::UniformBuffer => {
                let binding = output.io[i].buffer_index.unwrap_or(3) as u32;
                if let Some(g) = io_global(types, output, ir, i) {
                    ir.globals[g as usize].binding = Some(binding);
                }
            }
            ShaderIoKind::Texture(_) => {
                if texture_base.is_none() {
                    texture_base = Some(next);
                }
                if let Some(g) = io_global(types, output, ir, i) {
                    ir.globals[g as usize].binding = Some(next);
                }
                next += 1;
            }
            ShaderIoKind::Sampler(_) => {
                if sampler_base.is_none() {
                    sampler_base = Some(next);
                }
                next += 1;
            }
            ShaderIoKind::StorageBuffer(_) => next += 1,
            _ => {}
        }
    }
    for s in 0..output.samplers.len() {
        if sampler_base.is_none() {
            sampler_base = Some(next);
        }
        let compare = output.samplers[s].compare;
        let t = ir.ty(IrTy::Sampler(compare));
        let g = ir.global(IrGlobalKind::Sampler(s as u32), LiveId(s as u64), t);
        ir.globals[g as usize].binding = Some(next);
        next += 1;
    }
    info.texture_binding_base = texture_base.unwrap_or(0);
    info.sampler_binding_base = sampler_base.unwrap_or(0);
    info.xr_depth_binding = next;
    let depth_tex = if xr { TextureType::TextureDepthArray } else { TextureType::TextureDepth };
    let dt = ir.ty(IrTy::Texture(depth_tex));
    // Both variants keep one global (the type follows the variant).
    let xr_depth = match ir.find_global(IrGlobalKind::XrDepth, id!(tex_xr_depth)) {
        Some(g) => {
            ir.globals[g as usize].ty = dt;
            g
        }
        None => ir.global(IrGlobalKind::XrDepth, id!(tex_xr_depth), dt),
    };
    ir.globals[xr_depth as usize].binding = Some(next);
    let inst_buffer = if info.instance_slots > 0 {
        let arr = ir.ty(IrTy::Array(TY_U32, 0));
        let g = ir.global(IrGlobalKind::InstanceBuffer, id!(_mp_inst), arr);
        let b = crate::shader_ir_wgsl::wgsl_instance_binding(next);
        ir.globals[g as usize].binding = Some(b);
        info.instance_binding = Some(b);
        Some(g)
    } else {
        None
    };

    let view_id = ir.global(IrGlobalKind::ViewId, id!(VIEW_ID), TY_I32);
    let iid = ir.global(IrGlobalKind::InstanceIndex, id!(_mp_instance_index), TY_U32);
    let vec4 = ir.vec_ty(4, IrScalar::F32);
    let vtx_pos = ir.global(IrGlobalKind::VertexPosition, id!(vertex_pos), vec4);

    // The XR depth clip helper (as the WGSL text emitter wrote it).
    build_depth_clip(ir, xr, xr_depth, view_id);

    // Vertex.
    let geometry_vec4s = (info.geometry_slots + 3) / 4;
    let varying_vec4s = (varying_slots + 3) / 4;
    let mut inputs = vec![IrEntryIo {
        name: "instance_index".to_string(),
        ty: TY_U32,
        binding: IrIoBinding::Builtin(IrBuiltinIo::InstanceIndex),
    }];
    if xr {
        inputs.push(IrEntryIo { name: "view_index".to_string(), ty: TY_I32, binding: IrIoBinding::Builtin(IrBuiltinIo::ViewIndex) });
    }
    let geo_in_base = inputs.len() as u32;
    for i in 0..geometry_vec4s {
        inputs.push(IrEntryIo { name: format!("packed_geometry_{}", i), ty: vec4, binding: IrIoBinding::Location(i as u32) });
    }
    let mut outputs = vec![IrEntryIo { name: "position".to_string(), ty: vec4, binding: IrIoBinding::Builtin(IrBuiltinIo::Position) }];
    for i in 0..varying_vec4s {
        outputs.push(IrEntryIo { name: format!("packed_varying_{}", i), ty: vec4, binding: IrIoBinding::Location(i as u32) });
    }
    let vertex_fn = ir.function_by_name("io_vertex");
    let vertex_ret_vec4 = vertex_fn.map(|f| ir.functions[f as usize].ret == vec4).unwrap_or(false);

    let mut io_globals = Vec::new();
    for i in 0..output.io.len() {
        io_globals.push(io_global(types, output, ir, i));
    }

    let mut b = Bld { f: IrFunction::new("vertex_main".to_string(), TY_VOID), m: ir };
    b.f.entry = Some(IrEntry { stage: IrStage::Vertex, inputs, outputs });
    let iid_in = b.e(ExprKind::EntryIn(0), TY_U32);
    let g = b.glob(iid);
    b.assign(g, iid_in);
    let view = if xr { b.e(ExprKind::EntryIn(1), TY_I32) } else { b.i32(0) };
    let g = b.glob(view_id);
    b.assign(g, view);
    // Geometry.
    let mut geo_lanes = Vec::new();
    for i in 0..geometry_vec4s * 4 {
        let v = b.e(ExprKind::EntryIn(geo_in_base + (i / 4) as u32), vec4);
        geo_lanes.push(b.lane(v, (i % 4) as u8));
    }
    for f in &geometry {
        let Some(g) = io_globals[f.io] else { continue };
        let inline = ShaderTypeInline { id: f.pod, data: types.type_data(f.pod).clone() };
        let mut at = f.offset;
        let v = b.reconstruct(types, &inline, Source::Attr, &geo_lanes, &mut at);
        let place = b.glob(g);
        b.assign(place, v);
    }
    // Instance record words.
    if let Some(buf) = inst_buffer {
        let iid_e = b.glob(iid);
        let stride = b.u32(info.instance_slots as u32);
        let ib_v = b.bin(BinOp::Mul, iid_e, stride, TY_U32);
        let ib = b.f.local(id!(_mp_ib), 0, TY_U32, false, LocalKind::Temp);
        b.f.body.push(Stmt::Local(ib, ib_v));
        let mut words = Vec::new();
        for k in 0..info.instance_slots {
            let base = b.e(ExprKind::Local(ib), TY_U32);
            let kk = b.u32(k as u32);
            let idx = b.bin(BinOp::Add, base, kk, TY_U32);
            let arr = b.glob(buf);
            words.push(b.e(ExprKind::Index(arr, idx), TY_U32));
        }
        for f in &instance {
            let Some(g) = io_globals[f.io] else { continue };
            let inline = ShaderTypeInline { id: f.pod, data: types.type_data(f.pod).clone() };
            let mut at = f.offset;
            let v = b.reconstruct(types, &inline, Source::Word, &words, &mut at);
            let place = b.glob(g);
            b.assign(place, v);
        }
    }
    copy_uniforms(&mut b, dyn_block, &dyn_ios, scope_block, &scope_ios, &io_globals);
    let zero = b.f32(0.0);
    let one = b.f32(1.0);
    let init = b.e(ExprKind::Construct(vec![zero, zero, zero, one]), vec4);
    let g = b.glob(vtx_pos);
    b.assign(g, init);
    if let Some(vf) = vertex_fn {
        if vertex_ret_vec4 {
            let call = b.e(ExprKind::Call(vf, Vec::new()), vec4);
            let g = b.glob(vtx_pos);
            b.assign(g, call);
        } else {
            let call = b.e(ExprKind::Call(vf, Vec::new()), TY_VOID);
            b.f.body.push(Stmt::Expr(call));
        }
    }
    let pos = b.glob(vtx_pos);
    let out0 = b.e(ExprKind::EntryOut(0), vec4);
    b.assign(out0, pos);
    for f in &varyings {
        let Some(g) = io_globals[f.io] else { continue };
        let inline = ShaderTypeInline { id: f.pod, data: types.type_data(f.pod).clone() };
        let v = b.glob(g);
        let mut lanes = Vec::new();
        b.flatten(types, &inline, v, &mut lanes);
        for s in 0..f.slots {
            let slot = f.offset + s;
            let src = match lanes.get(s) {
                Some(e) => *e,
                None => b.f32(0.0),
            };
            let o = b.e(ExprKind::EntryOut(1 + (slot / 4) as u32), vec4);
            let dst = b.lane(o, (slot % 4) as u8);
            b.assign(dst, src);
        }
    }
    b.f.body.push(Stmt::Return(NO_EXPR));
    let vertex_main = b.f;
    ir.functions.push(vertex_main);

    // Fragment.
    let mut inputs = Vec::new();
    if xr {
        inputs.push(IrEntryIo { name: "view_index".to_string(), ty: TY_I32, binding: IrIoBinding::Builtin(IrBuiltinIo::ViewIndex) });
    }
    inputs.push(IrEntryIo { name: "position".to_string(), ty: vec4, binding: IrIoBinding::Builtin(IrBuiltinIo::Position) });
    let var_in_base = inputs.len() as u32;
    for i in 0..varying_vec4s {
        inputs.push(IrEntryIo { name: format!("packed_varying_{}", i), ty: vec4, binding: IrIoBinding::Location(i as u32) });
    }
    let mut frag_outputs = Vec::new();
    for (i, io) in output.io.iter().enumerate() {
        if let ShaderIoKind::FragmentOutput(index) = io.kind {
            frag_outputs.push((index, i));
        }
    }
    frag_outputs.sort_by_key(|(index, _)| *index);
    let mut outputs = Vec::new();
    for (index, i) in &frag_outputs {
        let t = ir.pod_ty(types, output.io[*i].ty);
        outputs.push(IrEntryIo { name: format!("fb{}", index), ty: t, binding: IrIoBinding::Location(*index as u32) });
    }
    let fragment_fn = ir.function_by_name("io_fragment");
    let mut b = Bld { f: IrFunction::new("fragment_main".to_string(), TY_VOID), m: ir };
    b.f.entry = Some(IrEntry { stage: IrStage::Fragment, inputs, outputs });
    let view = if xr { b.e(ExprKind::EntryIn(0), TY_I32) } else { b.i32(0) };
    let g = b.glob(view_id);
    b.assign(g, view);
    let mut var_lanes = Vec::new();
    for i in 0..varying_vec4s * 4 {
        let v = b.e(ExprKind::EntryIn(var_in_base + (i / 4) as u32), vec4);
        var_lanes.push(b.lane(v, (i % 4) as u8));
    }
    for f in &varyings {
        let Some(g) = io_globals[f.io] else { continue };
        let inline = ShaderTypeInline { id: f.pod, data: types.type_data(f.pod).clone() };
        let mut at = f.offset;
        let v = b.reconstruct(types, &inline, Source::Numeric, &var_lanes, &mut at);
        let place = b.glob(g);
        b.assign(place, v);
    }
    copy_uniforms(&mut b, dyn_block, &dyn_ios, scope_block, &scope_ios, &io_globals);
    if let Some(ff) = fragment_fn {
        let call = b.e(ExprKind::Call(ff, Vec::new()), TY_VOID);
        b.f.body.push(Stmt::Expr(call));
    }
    for (k, (_, i)) in frag_outputs.iter().enumerate() {
        let Some(g) = io_globals[*i] else { continue };
        let ty = b.m.globals[g as usize].ty;
        let v = b.glob(g);
        let o = b.e(ExprKind::EntryOut(k as u32), ty);
        b.assign(o, v);
    }
    b.f.body.push(Stmt::Return(NO_EXPR));
    let fragment_main = b.f;
    ir.functions.push(fragment_main);
    info
}

fn copy_uniforms(b: &mut Bld, dyn_block: Option<GlobalId>, dyn_ios: &[usize], scope_block: Option<GlobalId>, scope_ios: &[usize], io_globals: &[Option<GlobalId>]) {
    for (block, ios) in [(dyn_block, dyn_ios), (scope_block, scope_ios)] {
        let Some(block) = block else { continue };
        for (k, io) in ios.iter().enumerate() {
            let Some(g) = io_globals[*io] else { continue };
            let ty = b.m.globals[g as usize].ty;
            let blk = b.glob(block);
            let field = b.e(ExprKind::Field(blk, k as u32), ty);
            let place = b.glob(g);
            b.assign(place, field);
        }
    }
}

/// `depth_clip(world, color, clip)` as an IR function (the XR depth clip:
/// fragments behind the XR depth texture are discarded).
fn build_depth_clip(ir: &mut IrModule, xr: bool, xr_depth: GlobalId, view_id: GlobalId) {
    let mut used = false;
    for f in &ir.functions {
        let live = live_exprs(f);
        for (i, e) in f.exprs.iter().enumerate() {
            if live[i] && matches!(e.kind, ExprKind::DepthClip(..)) {
                used = true;
            }
        }
    }
    if !used {
        return;
    }
    let vec4 = ir.vec_ty(4, IrScalar::F32);
    let vec3 = ir.vec_ty(3, IrScalar::F32);
    let vec2u = ir.vec_ty(2, IrScalar::U32);
    let vec2i = ir.vec_ty(2, IrScalar::I32);
    let mat4 = ir.ty(IrTy::Mat(4, 4));
    let mut b = Bld { f: IrFunction::new("depth_clip".to_string(), vec4), m: ir };
    b.f.params.push(IrParam { name: id!(world), shadow: 0, ty: vec4, inout: false });
    b.f.params.push(IrParam { name: id!(color), shadow: 0, ty: vec4, inout: false });
    b.f.params.push(IrParam { name: id!(clip), shadow: 0, ty: TY_F32, inout: false });
    let world = 0u32;
    let color = 1u32;
    let clip = 2u32;
    let ret_color = |b: &mut Bld| -> Vec<Stmt> {
        let c = b.e(ExprKind::Param(color), vec4);
        vec![Stmt::Return(c)]
    };
    // if (clip < 0.5) return color
    let p = b.e(ExprKind::Param(clip), TY_F32);
    let half = b.f32(0.5);
    let c = b.bin(BinOp::Lt, p, half, TY_BOOL);
    let r = ret_color(&mut b);
    b.f.body.push(Stmt::If(c, r, Vec::new()));
    let dp = b.e(ExprKind::PassMatrix(PassMatrix::DepthProjection), mat4);
    let dv = b.e(ExprKind::PassMatrix(PassMatrix::DepthView), mat4);
    let dp_l = b.f.local(id!(depth_projection), 0, mat4, false, LocalKind::Temp);
    b.f.body.push(Stmt::Local(dp_l, dp));
    let dv_l = b.f.local(id!(depth_view), 0, mat4, false, LocalKind::Temp);
    b.f.body.push(Stmt::Local(dv_l, dv));
    // if (abs(depth_view[3].w - 1.0) > 0.5) return color
    let dvr = b.e(ExprKind::Local(dv_l), mat4);
    let three = b.i32(3);
    let col = b.e(ExprKind::Index(dvr, three), vec4);
    let w = b.lane(col, 3);
    let one = b.f32(1.0);
    let d = b.bin(BinOp::Sub, w, one, TY_F32);
    let a = b.builtin(Builtin::Abs, vec![d], TY_F32);
    let half = b.f32(0.5);
    let c = b.bin(BinOp::Gt, a, half, TY_BOOL);
    let r = ret_color(&mut b);
    b.f.body.push(Stmt::If(c, r, Vec::new()));
    // depth_pos = depth_projection * depth_view * world
    let dpr = b.e(ExprKind::Local(dp_l), mat4);
    let dvr = b.e(ExprKind::Local(dv_l), mat4);
    let m = b.bin(BinOp::Mul, dpr, dvr, mat4);
    let wv = b.e(ExprKind::Param(world), vec4);
    let pos = b.bin(BinOp::Mul, m, wv, vec4);
    let pos_l = b.f.local(id!(depth_pos), 0, vec4, false, LocalKind::Temp);
    b.f.body.push(Stmt::Local(pos_l, pos));
    // if (abs(depth_pos.w) < 0.000001) return color
    let pr = b.e(ExprKind::Local(pos_l), vec4);
    let pw = b.lane(pr, 3);
    let a = b.builtin(Builtin::Abs, vec![pw], TY_F32);
    let eps = b.f32(0.000001);
    let c = b.bin(BinOp::Lt, a, eps, TY_BOOL);
    let r = ret_color(&mut b);
    b.f.body.push(Stmt::If(c, r, Vec::new()));
    // depth_hc = (depth_pos.xyz / vec3(depth_pos.w)) * 0.5 + 0.5
    let pr = b.e(ExprKind::Local(pos_l), vec4);
    let xyz = b.e(ExprKind::Swizzle(pr, [0, 1, 2, 0], 3), vec3);
    let pr2 = b.e(ExprKind::Local(pos_l), vec4);
    let pw = b.lane(pr2, 3);
    let pw3 = b.e(ExprKind::Construct(vec![pw]), vec3);
    let div = b.bin(BinOp::Div, xyz, pw3, vec3);
    let h = b.f32(0.5);
    let h3 = b.e(ExprKind::Construct(vec![h]), vec3);
    let mul = b.bin(BinOp::Mul, div, h3, vec3);
    let h = b.f32(0.5);
    let h3 = b.e(ExprKind::Construct(vec![h]), vec3);
    let hc = b.bin(BinOp::Add, mul, h3, vec3);
    let hc_l = b.f.local(id!(depth_hc), 0, vec3, false, LocalKind::Temp);
    b.f.body.push(Stmt::Local(hc_l, hc));
    // dims
    let tex = b.glob(xr_depth);
    let dims = b.e(ExprKind::TexSize(tex), vec2u);
    let dims_l = b.f.local(id!(dims), 0, vec2u, false, LocalKind::Temp);
    b.f.body.push(Stmt::Local(dims_l, dims));
    let dx = {
        let d = b.e(ExprKind::Local(dims_l), vec2u);
        b.lane(d, 0)
    };
    let dy = {
        let d = b.e(ExprKind::Local(dims_l), vec2u);
        b.lane(d, 1)
    };
    let z0 = b.u32(0);
    let c1 = b.bin(BinOp::Eq, dx, z0, TY_BOOL);
    let z0 = b.u32(0);
    let c2 = b.bin(BinOp::Eq, dy, z0, TY_BOOL);
    let c = b.bin(BinOp::LogicOr, c1, c2, TY_BOOL);
    let r = ret_color(&mut b);
    b.f.body.push(Stmt::If(c, r, Vec::new()));
    let mut coords = Vec::new();
    for lane in 0..2u8 {
        let hcr = b.e(ExprKind::Local(hc_l), vec3);
        let h = b.lane(hcr, lane);
        let d = b.e(ExprKind::Local(dims_l), vec2u);
        let dl = b.lane(d, lane);
        let dlf = b.convert(dl, TY_F32);
        let prod = b.bin(BinOp::Mul, h, dlf, TY_F32);
        let pi = b.convert(prod, TY_I32);
        let d = b.e(ExprKind::Local(dims_l), vec2u);
        let dl = b.lane(d, lane);
        let dli = b.convert(dl, TY_I32);
        let one = b.i32(1);
        let m1 = b.bin(BinOp::Sub, dli, one, TY_I32);
        let zero = b.i32(0);
        let hi = b.builtin(Builtin::Max, vec![m1, zero], TY_I32);
        let zero = b.i32(0);
        coords.push(b.builtin(Builtin::Clamp, vec![pi, zero, hi], TY_I32));
    }
    let coord = b.e(ExprKind::Construct(coords), vec2i);
    let layer = if xr { b.glob(view_id) } else { NO_EXPR };
    let lod = b.i32(0);
    let tex = b.glob(xr_depth);
    let z = b.e(ExprKind::TexLoad(tex, coord, layer, lod), TY_F32);
    let hcr = b.e(ExprKind::Local(hc_l), vec3);
    let hz = b.lane(hcr, 2);
    let c = b.bin(BinOp::Ge, z, hz, TY_BOOL);
    let r = ret_color(&mut b);
    b.f.body.push(Stmt::If(c, r, Vec::new()));
    b.f.body.push(Stmt::Discard);
    let zero = b.f32(0.0);
    let z4 = b.e(ExprKind::Construct(vec![zero]), vec4);
    b.f.body.push(Stmt::Return(z4));
    let f = b.f;
    ir.functions.push(f);
}

/// A draw shader's SPIR-V: the window and the XR variant's vertex and
/// fragment binaries, from one IR compile.
pub struct IrDrawSpirv {
    pub info: IrDrawInfo,
    pub vertex: Vec<u32>,
    pub fragment: Vec<u32>,
}

/// Both variants' SPIR-V of a compiled draw shader.
pub fn draw_spirv(output: &ShaderOutput, types: &impl crate::shader_types::ShaderTypes) -> Result<[IrDrawSpirv; 2], String> {
    let window = draw_variant_spirv(output, types, false)?;
    let xr = draw_variant_spirv(output, types, true)?;
    Ok([window, xr])
}

pub fn draw_variant_spirv(output: &ShaderOutput, types: &impl crate::shader_types::ShaderTypes, xr: bool) -> Result<IrDrawSpirv, String> {
    draw_variant_spirv_with_pick(output, types, xr, false)
}

#[path = "shader_ir_pick.rs"]
mod spirv_pick;

fn draw_variant_spirv_with_pick(output: &ShaderOutput, types: &impl crate::shader_types::ShaderTypes, xr: bool, pick: bool) -> Result<IrDrawSpirv, String> {
    let mut ir = output.ir.clone();
    let mut info = build_draw_entries(output, &mut ir, types, xr);
    if pick { spirv_pick::apply(output, &mut ir, &mut info)?; }
    let (Some(v), Some(f)) = (ir.function_by_name("vertex_main"), ir.function_by_name("fragment_main")) else {
        return Err("draw shader has no entry points".to_string());
    };
    let vertex = crate::shader_ir_spirv::ir_to_spirv(&ir, v, xr).map_err(|e| format!("vertex: {}", e))?;
    let fragment = crate::shader_ir_spirv::ir_to_spirv(&ir, f, xr).map_err(|e| format!("fragment: {}", e))?;
    Ok(IrDrawSpirv { info, vertex, fragment })
}

/// GLSL ES 3.00's minimum MAX_VERTEX_ATTRIBS: the vertex inputs every GL ES
/// 3 and WebGL 2 device takes. Instance data past them are read from the
/// instance data texture (`mp_inst_data`).
pub const GLSL_MAX_VERTEX_ATTRIBS: usize = 16;
/// WebGL's largest vertex attribute stride, in bytes.
pub const WEBGL_MAX_ATTRIB_STRIDE: usize = 255;
/// GL ES 3.0's minimum MAX_VERTEX_ATTRIB_STRIDE, in bytes.
pub const GLES_MAX_ATTRIB_STRIDE: usize = 2048;

/// What the GL printer needs beyond the IR.
#[relax_api(plain)]
#[derive(Clone, Debug, Default)]
pub struct GlDrawInfo {
    pub geometry_slots: usize,
    pub instance_slots: usize,
    pub varying_slots: usize,
    /// The first instance vec4 read from the instance data texture (None:
    /// every instance vec4 is an attribute).
    pub instance_fetch_from: Option<usize>,
    /// Compact geometry formats: typed attributes (`geom_*`) instead of
    /// packed vec4s.
    pub typed_geometry: bool,
}

fn has_compact(types: &impl crate::shader_types::ShaderTypes, output: &ShaderOutput) -> bool {
    for io in &output.io {
        if matches!(io.kind, ShaderIoKind::VertexBuffer | ShaderIoKind::DynInstance | ShaderIoKind::RustInstance)
            && types.type_data(io.ty).ty.has_compact_format()
        {
            return true;
        }
    }
    false
}

/// A record field by GL's rule: f32 lanes, an integer vector on a 4-lane
/// boundary in attribute records.
fn push_gl_field(types: &impl crate::shader_types::ShaderTypes, out: &mut Vec<RecField>, io_index: usize, pod: ShaderTypeId, attribute: bool, offset: &mut usize) {
    let ty = &types.type_data(pod).ty;
    let slots = ty.slots();
    let int = attribute && is_int_format(ty) && slots > 1;
    if int && (*offset & 3) != 0 {
        *offset += 4 - (*offset & 3);
    }
    out.push(RecField { io: io_index, pod, slots, offset: *offset });
    *offset += slots;
    if int && (*offset & 3) != 0 {
        *offset += 4 - (*offset & 3);
    }
}

/// The logical type a typed (compact) attribute arrives as.
fn gl_attr_ty(ir: &mut IrModule, ty: &ShaderTypeKind) -> TyId {
    use crate::shader_schema::ShaderVec as V;
    match ty {
        ShaderTypeKind::Packed(p) => ir.vec_ty(if p.is_vec4() { 4 } else { 2 }, IrScalar::F32),
        ShaderTypeKind::F32 | ShaderTypeKind::F16 => TY_F32,
        ShaderTypeKind::U32 | ShaderTypeKind::AtomicU32 | ShaderTypeKind::Bool => TY_U32,
        ShaderTypeKind::I32 | ShaderTypeKind::AtomicI32 => TY_I32,
        ShaderTypeKind::Vec(v) => {
            let (n, s) = match v {
                V::Vec2f | V::Vec2h => (2, IrScalar::F32),
                V::Vec3f | V::Vec3h => (3, IrScalar::F32),
                V::Vec4f | V::Vec4h => (4, IrScalar::F32),
                V::Vec2u | V::Vec2b => (2, IrScalar::U32),
                V::Vec3u | V::Vec3b => (3, IrScalar::U32),
                V::Vec4u | V::Vec4b => (4, IrScalar::U32),
                V::Vec2i => (2, IrScalar::I32),
                V::Vec3i => (3, IrScalar::I32),
                V::Vec4i => (4, IrScalar::I32),
            };
            ir.vec_ty(n, s)
        }
        _ => TY_F32,
    }
}

/// Appends GL's `vertex_main` / `fragment_main` (the GLSL ES 3.00 entry
/// points the GL and WebGL 2 backends link): geometry and instance records
/// from packed vec4 attributes (instance vec4s past the attribute limit from
/// the instance data texture; compact geometry from typed attributes),
/// uniforms read in place from their blocks, varyings packed as on every
/// backend, and GL's clip space (Y flipped for bottom-up targets, z moved
/// to -w..w unless Vulkan renders).
pub fn build_gl_entries(output: &ShaderOutput, ir: &mut IrModule, types: &impl crate::shader_types::ShaderTypes) -> GlDrawInfo {
    let mut info = GlDrawInfo::default();
    let typed = has_compact(types, output);
    info.typed_geometry = typed;
    let mut geometry = Vec::new();
    let mut instance = Vec::new();
    let mut offset = 0;
    for (i, io) in output.io.iter().enumerate() {
        if let ShaderIoKind::VertexBuffer = io.kind {
            push_gl_field(types, &mut geometry, i, io.ty, true, &mut offset);
        }
    }
    let mut offset = 0;
    for rust in [false, true] {
        for (i, io) in output.io.iter().enumerate() {
            let wanted = match io.kind {
                ShaderIoKind::DynInstance => !rust,
                ShaderIoKind::RustInstance => rust,
                _ => false,
            };
            if wanted {
                push_gl_field(types, &mut instance, i, io.ty, true, &mut offset);
            }
        }
    }
    info.geometry_slots = record_end(&geometry);
    info.instance_slots = record_end(&instance);
    let geometry_vec4s = (info.geometry_slots + 3) / 4;
    let instance_vec4s = (info.instance_slots + 3) / 4;
    if !typed && geometry_vec4s + instance_vec4s > GLSL_MAX_VERTEX_ATTRIBS {
        let max_stride = if output.glsl_webgl2 { WEBGL_MAX_ATTRIB_STRIDE } else { GLES_MAX_ATTRIB_STRIDE };
        info.instance_fetch_from = if info.instance_slots * 4 > max_stride {
            Some(0)
        } else {
            Some(GLSL_MAX_VERTEX_ATTRIBS.saturating_sub(geometry_vec4s))
        };
    }

    let fragment_reads = match ir.function_by_name("io_fragment") {
        Some(f) => {
            let funcs = ir.reachable(f);
            ir.globals_used(&funcs)
        }
        None => vec![false; ir.globals.len()],
    };
    let mut varyings = Vec::new();
    let mut offset = 0;
    for rust in [false, true] {
        for (i, io) in output.io.iter().enumerate() {
            let kind = match io.kind {
                ShaderIoKind::DynInstance if !rust => IrGlobalKind::Io(IrIo::DynInstance),
                ShaderIoKind::RustInstance if rust => IrGlobalKind::Io(IrIo::RustInstance),
                _ => continue,
            };
            let read = match ir.find_global(kind, io.name) {
                Some(g) => fragment_reads.get(g as usize).copied().unwrap_or(false),
                None => false,
            };
            if read {
                push_gl_field(types, &mut varyings, i, io.ty, false, &mut offset);
            }
        }
    }
    for (i, io) in output.io.iter().enumerate() {
        if let ShaderIoKind::Varying = io.kind {
            push_gl_field(types, &mut varyings, i, io.ty, false, &mut offset);
        }
    }
    info.varying_slots = record_end(&varyings);
    let varying_vec4s = (info.varying_slots + 3) / 4;

    let mut io_globals = Vec::new();
    for i in 0..output.io.len() {
        io_globals.push(io_global(types, output, ir, i));
    }
    let vec4 = ir.vec_ty(4, IrScalar::F32);
    let vtx_pos = ir.global(IrGlobalKind::VertexPosition, id!(vertex_pos), vec4);
    let flip = ir.global(IrGlobalKind::ClipYFlip, id!(mp_clip_y_flip), TY_F32);

    // Vertex.
    let mut inputs = Vec::new();
    let mut typed_inputs = Vec::new();
    if typed {
        for (i, io) in output.io.iter().enumerate() {
            let prefix = match io.kind {
                ShaderIoKind::VertexBuffer => "geom",
                ShaderIoKind::DynInstance | ShaderIoKind::RustInstance => "inst",
                _ => continue,
            };
            let pod = types.type_data(io.ty).ty.clone();
            match &pod {
                ShaderTypeKind::Struct { fields, .. } => {
                    for (k, f) in fields.iter().enumerate() {
                        let t = gl_attr_ty(ir, &f.ty.data.ty);
                        typed_inputs.push((i, Some(k), inputs.len() as u32));
                        inputs.push(IrEntryIo { name: format!("{}_{}", prefix, f.name), ty: t, binding: IrIoBinding::Location(0) });
                    }
                }
                other => {
                    let t = gl_attr_ty(ir, other);
                    typed_inputs.push((i, None, inputs.len() as u32));
                    inputs.push(IrEntryIo { name: format!("{}_{}", prefix, io.name), ty: t, binding: IrIoBinding::Location(0) });
                }
            }
        }
    }
    let geo_in_base = inputs.len() as u32;
    if !typed {
        for k in 0..geometry_vec4s {
            inputs.push(IrEntryIo { name: format!("packed_geometry_{}", k), ty: vec4, binding: IrIoBinding::Location(k as u32) });
        }
    }
    let inst_in_base = inputs.len() as u32;
    let attr_instance_vec4s = match info.instance_fetch_from {
        Some(from) => from.min(instance_vec4s),
        None => instance_vec4s,
    };
    if !typed {
        for k in 0..attr_instance_vec4s {
            inputs.push(IrEntryIo { name: format!("packed_instance_{}", k), ty: vec4, binding: IrIoBinding::Location(0) });
        }
    }
    let mut outputs = vec![IrEntryIo { name: "gl_Position".to_string(), ty: vec4, binding: IrIoBinding::Builtin(IrBuiltinIo::Position) }];
    for k in 0..varying_vec4s {
        outputs.push(IrEntryIo { name: format!("packed_varying_{}", k), ty: vec4, binding: IrIoBinding::Location(k as u32) });
    }
    let vertex_fn = ir.function_by_name("io_vertex");
    let vertex_ret_vec4 = vertex_fn.map(|f| ir.functions[f as usize].ret == vec4).unwrap_or(false);
    let mut b = Bld { f: IrFunction::new("vertex_main".to_string(), TY_VOID), m: ir };
    b.f.entry = Some(IrEntry { stage: IrStage::Vertex, inputs, outputs });
    if typed {
        for (io, field, input) in &typed_inputs {
            let Some(g) = io_globals[*io] else { continue };
            let gty = b.m.globals[g as usize].ty;
            let it = b.f.entry.as_ref().unwrap().inputs[*input as usize].ty;
            let v = b.e(ExprKind::EntryIn(*input), it);
            let base = b.glob(g);
            match field {
                Some(k) => {
                    let ft = match b.m.get(gty).clone() {
                        IrTy::Struct(s) => b.m.structs[s as usize].fields[*k].ty,
                        _ => it,
                    };
                    let place = b.e(ExprKind::Field(base, *k as u32), ft);
                    let v = b.convert(v, ft);
                    b.assign(place, v);
                }
                None => {
                    let v = b.convert(v, gty);
                    b.assign(base, v);
                }
            }
        }
    } else {
        let mut geo_lanes = Vec::new();
        for i in 0..geometry_vec4s * 4 {
            let v = b.e(ExprKind::EntryIn(geo_in_base + (i / 4) as u32), vec4);
            geo_lanes.push(b.lane(v, (i % 4) as u8));
        }
        let mut inst_lanes = Vec::new();
        for i in 0..instance_vec4s * 4 {
            let k = i / 4;
            if k < attr_instance_vec4s {
                let v = b.e(ExprKind::EntryIn(inst_in_base + k as u32), vec4);
                inst_lanes.push(b.lane(v, (i % 4) as u8));
            } else if i < info.instance_slots {
                inst_lanes.push(b.e(ExprKind::InstanceTexWord(i as u32), TY_F32));
            } else {
                inst_lanes.push(b.f32(0.0));
            }
        }
        for (fields, lanes) in [(&geometry, &geo_lanes), (&instance, &inst_lanes)] {
            for f in fields.iter() {
                let Some(g) = io_globals[f.io] else { continue };
                let inline = ShaderTypeInline { id: f.pod, data: types.type_data(f.pod).clone() };
                let mut at = f.offset;
                gl_assign_reconstructed(&mut b, types, g, &inline, Source::Attr, lanes, &mut at);
            }
        }
    }
    let zero = b.f32(0.0);
    let one = b.f32(1.0);
    let init = b.e(ExprKind::Construct(vec![zero, zero, zero, one]), vec4);
    let g = b.glob(vtx_pos);
    b.assign(g, init);
    if let Some(vf) = vertex_fn {
        if vertex_ret_vec4 {
            let call = b.e(ExprKind::Call(vf, Vec::new()), vec4);
            let g = b.glob(vtx_pos);
            b.assign(g, call);
        } else {
            let call = b.e(ExprKind::Call(vf, Vec::new()), TY_VOID);
            b.f.body.push(Stmt::Expr(call));
        }
    }
    for f in &varyings {
        let Some(g) = io_globals[f.io] else { continue };
        let inline = ShaderTypeInline { id: f.pod, data: types.type_data(f.pod).clone() };
        let v = b.glob(g);
        let mut lanes = Vec::new();
        b.flatten(types, &inline, v, &mut lanes);
        for s in 0..f.slots {
            let slot = f.offset + s;
            let src = match lanes.get(s) {
                Some(e) => *e,
                None => b.f32(0.0),
            };
            let o = b.e(ExprKind::EntryOut(1 + (slot / 4) as u32), vec4);
            let dst = b.lane(o, (slot % 4) as u8);
            b.assign(dst, src);
        }
    }
    // gl_Position: Y flipped for bottom-up targets; clip z from 0..w to
    // GL's -w..w (not when Vulkan renders this source).
    let p = b.glob(vtx_pos);
    let x = b.lane(p, 0);
    let p = b.glob(vtx_pos);
    let y = b.lane(p, 1);
    let fl = b.glob(flip);
    let half = b.f32(0.5);
    let flipped = b.bin(BinOp::Gt, fl, half, TY_BOOL);
    let ny = b.e(ExprKind::Unary(UnOp::Neg, y), TY_F32);
    let p = b.glob(vtx_pos);
    let y2 = b.lane(p, 1);
    let y = b.e(ExprKind::Select(flipped, ny, y2), TY_F32);
    let p = b.glob(vtx_pos);
    let z = b.lane(p, 2);
    let p = b.glob(vtx_pos);
    let w = b.lane(p, 3);
    let z = if output.use_vulkan {
        z
    } else {
        let two = b.f32(2.0);
        let z2 = b.bin(BinOp::Mul, two, z, TY_F32);
        let p = b.glob(vtx_pos);
        let w2 = b.lane(p, 3);
        b.bin(BinOp::Sub, z2, w2, TY_F32)
    };
    let pos = b.e(ExprKind::Construct(vec![x, y, z, w]), vec4);
    let out0 = b.e(ExprKind::EntryOut(0), vec4);
    b.assign(out0, pos);
    let vertex_main = b.f;
    ir.functions.push(vertex_main);

    // Fragment.
    let mut inputs = Vec::new();
    for k in 0..varying_vec4s {
        inputs.push(IrEntryIo { name: format!("packed_varying_{}", k), ty: vec4, binding: IrIoBinding::Location(k as u32) });
    }
    let mut outputs = Vec::new();
    let mut frag_outputs = Vec::new();
    for (i, io) in output.io.iter().enumerate() {
        if let ShaderIoKind::FragmentOutput(index) = io.kind {
            let t = ir.pod_ty(types, io.ty);
            frag_outputs.push(i);
            outputs.push(IrEntryIo { name: format!("_mp_frag_{}", index), ty: t, binding: IrIoBinding::Location(index as u32) });
        }
    }
    let fragment_fn = ir.function_by_name("io_fragment");
    let mut b = Bld { f: IrFunction::new("fragment_main".to_string(), TY_VOID), m: ir };
    b.f.entry = Some(IrEntry { stage: IrStage::Fragment, inputs, outputs });
    let mut var_lanes = Vec::new();
    for i in 0..varying_vec4s * 4 {
        let v = b.e(ExprKind::EntryIn((i / 4) as u32), vec4);
        var_lanes.push(b.lane(v, (i % 4) as u8));
    }
    for f in &varyings {
        let Some(g) = io_globals[f.io] else { continue };
        let inline = ShaderTypeInline { id: f.pod, data: types.type_data(f.pod).clone() };
        let mut at = f.offset;
        gl_assign_reconstructed(&mut b, types, g, &inline, Source::Numeric, &var_lanes, &mut at);
    }
    if let Some(ff) = fragment_fn {
        let call = b.e(ExprKind::Call(ff, Vec::new()), TY_VOID);
        b.f.body.push(Stmt::Expr(call));
    }
    for (k, i) in frag_outputs.iter().enumerate() {
        let Some(g) = io_globals[*i] else { continue };
        let ty = b.m.globals[g as usize].ty;
        let v = b.glob(g);
        let o = b.e(ExprKind::EntryOut(k as u32), ty);
        b.assign(o, v);
    }
    let fragment_main = b.f;
    ir.functions.push(fragment_main);
    info
}

/// A record field into its global: a struct one field at a time (some GL ES
/// drivers, the Android emulator's ANGLE/SwiftShader among them, reject
/// struct constructors there).
fn gl_assign_reconstructed(b: &mut Bld, types: &impl crate::shader_types::ShaderTypes, g: GlobalId, ty: &ShaderTypeInline, src: Source, lanes: &[ExprId], at: &mut usize) {
    if let ShaderTypeKind::Struct { fields, .. } = &ty.data.ty {
        for (k, f) in fields.iter().enumerate() {
            let v = b.reconstruct(types, &f.ty, src, lanes, at);
            let ft = b.m.pod_inline_ty(types, &f.ty);
            let base = b.glob(g);
            let place = b.e(ExprKind::Field(base, k as u32), ft);
            b.assign(place, v);
        }
        return;
    }
    let v = b.reconstruct(types, ty, src, lanes, at);
    let place = b.glob(g);
    b.assign(place, v);
}

/// What the HLSL printer needs beyond the IR.
#[derive(Clone, Debug, Default)]
pub struct HlslDrawInfo {
    pub instance_slots: usize,
    /// The semantic of each `vertex_main` input (D3D11's input layout names
    /// geometry `GEOM<letter><chunk>`, one letter per geometry input).
    pub vertex_semantics: Vec<String>,
}

/// D3D11's input chunks of a value of `slots` floats (matrices row by row).
pub fn hlsl_slot_chunks(slots: usize) -> Vec<usize> {
    match slots {
        0 => Vec::new(),
        9 => vec![3, 3, 3],
        16 => vec![4, 4, 4, 4],
        _ => {
            let mut rem = slots;
            let mut chunks = Vec::new();
            while rem > 0 {
                let c = rem.min(4);
                chunks.push(c);
                rem -= c;
            }
            chunks
        }
    }
}

/// The semantic letters of input `index` (A..Z, AA..AZ, BA..).
pub fn index_to_semantic(index: usize) -> String {
    const LETTERS: &[u8; 26] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ";
    let mut value = index;
    let mut out = Vec::new();
    loop {
        out.push(LETTERS[value % 26] as char);
        if value < 26 {
            break;
        }
        value = value / 26 - 1;
    }
    out.iter().rev().collect()
}

/// Appends D3D11's `vertex_main` / `fragment_main`: geometry from the input
/// elements D3D11's input layout declares (one semantic per geometry input,
/// a compact struct one per field, wider values in float chunks), instance
/// records read by instance id from the instance word buffer, uniforms read
/// in place from their constant buffers, varyings packed as on every
/// backend. No XR variant and no depth clip (`depth_clip` returns its
/// colour).
pub fn build_hlsl_entries(output: &ShaderOutput, ir: &mut IrModule, types: &impl crate::shader_types::ShaderTypes) -> HlslDrawInfo {
    let mut info = HlslDrawInfo::default();
    let mut instance = Vec::new();
    let mut offset = 0;
    for rust in [false, true] {
        for (i, io) in output.io.iter().enumerate() {
            let wanted = match io.kind {
                ShaderIoKind::DynInstance => !rust,
                ShaderIoKind::RustInstance => rust,
                _ => false,
            };
            if wanted {
                if let Some(lane) = io.instance_lane {
                    let slots = field_slots(&types.type_data(io.ty).ty, true);
                    instance.push(RecField { io: i, pod: io.ty, slots, offset: lane });
                    offset = offset.max(lane + slots);
                } else { push_field(types, &mut instance, i, io.ty, true, &mut offset); }
            }
        }
    }
    info.instance_slots = record_end(&instance);

    let fragment_reads = match ir.function_by_name("io_fragment") {
        Some(f) => {
            let funcs = ir.reachable(f);
            ir.globals_used(&funcs)
        }
        None => vec![false; ir.globals.len()],
    };
    let mut varyings = Vec::new();
    let mut offset = 0;
    for rust in [false, true] {
        for (i, io) in output.io.iter().enumerate() {
            let kind = match io.kind {
                ShaderIoKind::DynInstance if !rust => IrGlobalKind::Io(IrIo::DynInstance),
                ShaderIoKind::RustInstance if rust => IrGlobalKind::Io(IrIo::RustInstance),
                _ => continue,
            };
            let read = match ir.find_global(kind, io.name) {
                Some(g) => fragment_reads.get(g as usize).copied().unwrap_or(false),
                None => false,
            };
            if read {
                push_field(types, &mut varyings, i, io.ty, false, &mut offset);
            }
        }
    }
    for (i, io) in output.io.iter().enumerate() {
        if let ShaderIoKind::Varying = io.kind {
            push_field(types, &mut varyings, i, io.ty, false, &mut offset);
        }
    }
    let varying_vec4s = (record_end(&varyings) + 3) / 4;

    let mut io_globals = Vec::new();
    for i in 0..output.io.len() {
        io_globals.push(io_global(types, output, ir, i));
    }
    let inst_buffer = if info.instance_slots > 0 {
        let arr = ir.ty(IrTy::Array(TY_U32, 0));
        Some(ir.global(IrGlobalKind::InstanceBuffer, id!(_mp_inst), arr))
    } else {
        None
    };
    let inst_base = inst_buffer.map(|_| ir.global(IrGlobalKind::InstanceBase, id!(_mp_inst_base), TY_U32));
    let iid = ir.global(IrGlobalKind::InstanceIndex, id!(_mp_instance_index), TY_U32);
    let vec4 = ir.vec_ty(4, IrScalar::F32);
    let vtx_pos = ir.global(IrGlobalKind::VertexPosition, id!(vertex_pos), vec4);

    // Vertex inputs: geometry as D3D11's input layout declares it.
    enum Geo {
        /// A value read whole from one typed input.
        Whole(usize, u32),
        /// One field of a compact struct from its typed input.
        Field(usize, usize, u32),
        /// A value rebuilt from float chunks (first input, chunk count).
        Chunks(usize, u32, usize),
    }
    let mut inputs = Vec::new();
    let mut geo = Vec::new();
    let mut sem = 0usize;
    for (i, io) in output.io.iter().enumerate() {
        if !matches!(io.kind, ShaderIoKind::VertexBuffer) {
            continue;
        }
        let pod = types.type_data(io.ty).ty.clone();
        match &pod {
            ShaderTypeKind::Struct { fields, .. } if pod.has_compact_format() => {
                for (k, f) in fields.iter().enumerate() {
                    let t = gl_attr_ty(ir, &f.ty.data.ty);
                    geo.push(Geo::Field(i, k, inputs.len() as u32));
                    info.vertex_semantics.push(format!("GEOM{}", index_to_semantic(sem)));
                    inputs.push(IrEntryIo { name: format!("geom_{}_{}", io.name, f.name), ty: t, binding: IrIoBinding::Location(0) });
                    sem += 1;
                }
            }
            other => {
                let slots = other.slots();
                if slots > 4 || matches!(other, ShaderTypeKind::Struct { .. }) {
                    let chunks = hlsl_slot_chunks(slots);
                    geo.push(Geo::Chunks(i, inputs.len() as u32, chunks.len()));
                    for (k, n) in chunks.iter().enumerate() {
                        let t = if *n == 1 { TY_F32 } else { ir.vec_ty(*n as u8, IrScalar::F32) };
                        info.vertex_semantics.push(format!("GEOM{}{}", index_to_semantic(sem), k));
                        inputs.push(IrEntryIo { name: format!("geom_{}_{}", io.name, k), ty: t, binding: IrIoBinding::Location(0) });
                    }
                } else {
                    let t = gl_attr_ty(ir, other);
                    geo.push(Geo::Whole(i, inputs.len() as u32));
                    info.vertex_semantics.push(format!("GEOM{}", index_to_semantic(sem)));
                    inputs.push(IrEntryIo { name: format!("geom_{}", io.name), ty: t, binding: IrIoBinding::Location(0) });
                }
                sem += 1;
            }
        }
    }
    let iid_in = inputs.len() as u32;
    info.vertex_semantics.push("SV_InstanceID".to_string());
    inputs.push(IrEntryIo { name: "instance_index".to_string(), ty: TY_U32, binding: IrIoBinding::Builtin(IrBuiltinIo::InstanceIndex) });

    let mut outputs = vec![IrEntryIo { name: "position".to_string(), ty: vec4, binding: IrIoBinding::Builtin(IrBuiltinIo::Position) }];
    for k in 0..varying_vec4s {
        outputs.push(IrEntryIo { name: format!("packed_varying_{}", k), ty: vec4, binding: IrIoBinding::Location(k as u32) });
    }
    let frag_inputs = outputs.clone();
    let vertex_fn = ir.function_by_name("io_vertex");
    let vertex_ret_vec4 = vertex_fn.map(|f| ir.functions[f as usize].ret == vec4).unwrap_or(false);

    let mut b = Bld { f: IrFunction::new("vertex_main".to_string(), TY_VOID), m: ir };
    b.f.entry = Some(IrEntry { stage: IrStage::Vertex, inputs, outputs });
    let v = b.e(ExprKind::EntryIn(iid_in), TY_U32);
    let g = b.glob(iid);
    b.assign(g, v);
    for item in &geo {
        match *item {
            Geo::Whole(io, input) => {
                let Some(g) = io_globals[io] else { continue };
                let gty = b.m.globals[g as usize].ty;
                let it = b.f.entry.as_ref().unwrap().inputs[input as usize].ty;
                let v = b.e(ExprKind::EntryIn(input), it);
                let v = b.convert(v, gty);
                let place = b.glob(g);
                b.assign(place, v);
            }
            Geo::Field(io, k, input) => {
                let Some(g) = io_globals[io] else { continue };
                let gty = b.m.globals[g as usize].ty;
                let it = b.f.entry.as_ref().unwrap().inputs[input as usize].ty;
                let ft = match b.m.get(gty).clone() {
                    IrTy::Struct(s) => b.m.structs[s as usize].fields[k].ty,
                    _ => it,
                };
                let v = b.e(ExprKind::EntryIn(input), it);
                let v = b.convert(v, ft);
                let base = b.glob(g);
                let place = b.e(ExprKind::Field(base, k as u32), ft);
                b.assign(place, v);
            }
            Geo::Chunks(io, first, count) => {
                let Some(g) = io_globals[io] else { continue };
                let mut lanes = Vec::new();
                for c in 0..count {
                    let it = b.f.entry.as_ref().unwrap().inputs[first as usize + c].ty;
                    let n = b.m.lanes(it);
                    let v = b.e(ExprKind::EntryIn(first + c as u32), it);
                    if n <= 1 {
                        lanes.push(v);
                    } else {
                        for l in 0..n {
                            let v = b.e(ExprKind::EntryIn(first + c as u32), it);
                            lanes.push(b.lane(v, l as u8));
                        }
                    }
                }
                let pod = output.io[io].ty;
                let inline = ShaderTypeInline { id: pod, data: types.type_data(pod).clone() };
                let mut at = 0;
                gl_assign_reconstructed(&mut b, types, g, &inline, Source::Attr, &lanes, &mut at);
            }
        }
    }
    if let Some(buf) = inst_buffer {
        let iid_e = b.glob(iid);
        let base_e = b.glob(inst_base.expect("instance base with the instance buffer"));
        let first = b.bin(BinOp::Add, iid_e, base_e, TY_U32);
        let stride = b.u32(info.instance_slots as u32);
        let ib_v = b.bin(BinOp::Mul, first, stride, TY_U32);
        let ib = b.f.local(id!(_mp_ib), 0, TY_U32, false, LocalKind::Temp);
        b.f.body.push(Stmt::Local(ib, ib_v));
        let mut words = Vec::new();
        for k in 0..info.instance_slots {
            let base = b.e(ExprKind::Local(ib), TY_U32);
            let kk = b.u32(k as u32);
            let idx = b.bin(BinOp::Add, base, kk, TY_U32);
            let arr = b.glob(buf);
            words.push(b.e(ExprKind::Index(arr, idx), TY_U32));
        }
        for f in &instance {
            let Some(g) = io_globals[f.io] else { continue };
            let inline = ShaderTypeInline { id: f.pod, data: types.type_data(f.pod).clone() };
            let mut at = f.offset;
            gl_assign_reconstructed(&mut b, types, g, &inline, Source::Word, &words, &mut at);
        }
    }
    let zero = b.f32(0.0);
    let one = b.f32(1.0);
    let init = b.e(ExprKind::Construct(vec![zero, zero, zero, one]), vec4);
    let g = b.glob(vtx_pos);
    b.assign(g, init);
    if let Some(vf) = vertex_fn {
        if vertex_ret_vec4 {
            let call = b.e(ExprKind::Call(vf, Vec::new()), vec4);
            let g = b.glob(vtx_pos);
            b.assign(g, call);
        } else {
            let call = b.e(ExprKind::Call(vf, Vec::new()), TY_VOID);
            b.f.body.push(Stmt::Expr(call));
        }
    }
    let pos = b.glob(vtx_pos);
    let out0 = b.e(ExprKind::EntryOut(0), vec4);
    b.assign(out0, pos);
    for f in &varyings {
        let Some(g) = io_globals[f.io] else { continue };
        let inline = ShaderTypeInline { id: f.pod, data: types.type_data(f.pod).clone() };
        let v = b.glob(g);
        let mut lanes = Vec::new();
        b.flatten(types, &inline, v, &mut lanes);
        for s in 0..f.slots {
            let slot = f.offset + s;
            let src = match lanes.get(s) {
                Some(e) => *e,
                None => b.f32(0.0),
            };
            let o = b.e(ExprKind::EntryOut(1 + (slot / 4) as u32), vec4);
            let dst = b.lane(o, (slot % 4) as u8);
            b.assign(dst, src);
        }
    }
    b.f.body.push(Stmt::Return(NO_EXPR));
    let vertex_main = b.f;
    ir.functions.push(vertex_main);

    // Fragment: its input is the vertex stage's output.
    let mut outputs = Vec::new();
    let mut frag_outputs = Vec::new();
    for (i, io) in output.io.iter().enumerate() {
        if let ShaderIoKind::FragmentOutput(index) = io.kind {
            frag_outputs.push((index, i));
        }
    }
    frag_outputs.sort_by_key(|(index, _)| *index);
    for (index, i) in &frag_outputs {
        let t = ir.pod_ty(types, output.io[*i].ty);
        outputs.push(IrEntryIo { name: format!("fb{}", index), ty: t, binding: IrIoBinding::Location(*index as u32) });
    }
    let fragment_fn = ir.function_by_name("io_fragment");
    let mut b = Bld { f: IrFunction::new("fragment_main".to_string(), TY_VOID), m: ir };
    b.f.entry = Some(IrEntry { stage: IrStage::Fragment, inputs: frag_inputs, outputs });
    let mut var_lanes = Vec::new();
    for i in 0..varying_vec4s * 4 {
        let v = b.e(ExprKind::EntryIn(1 + (i / 4) as u32), vec4);
        var_lanes.push(b.lane(v, (i % 4) as u8));
    }
    for f in &varyings {
        let Some(g) = io_globals[f.io] else { continue };
        let inline = ShaderTypeInline { id: f.pod, data: types.type_data(f.pod).clone() };
        let mut at = f.offset;
        gl_assign_reconstructed(&mut b, types, g, &inline, Source::Numeric, &var_lanes, &mut at);
    }
    if let Some(ff) = fragment_fn {
        let call = b.e(ExprKind::Call(ff, Vec::new()), TY_VOID);
        b.f.body.push(Stmt::Expr(call));
    }
    for (k, (_, i)) in frag_outputs.iter().enumerate() {
        let Some(g) = io_globals[*i] else { continue };
        let ty = b.m.globals[g as usize].ty;
        let v = b.glob(g);
        let o = b.e(ExprKind::EntryOut(k as u32), ty);
        b.assign(o, v);
    }
    b.f.body.push(Stmt::Return(NO_EXPR));
    let fragment_main = b.f;
    ir.functions.push(fragment_main);
    info
}

/// Native SPIR-V compilation result, inspected without exposing Result or Vec layout.
#[relax_api(handle)]
pub struct IrDrawSpirvResult {
    result: Result<[IrDrawSpirv; 2], String>,
}
#[relax_api]
impl IrDrawSpirvResult {
    pub fn source(&self, index: usize) -> Option<&IrDrawSpirv> {
        self.result.as_ref().ok().and_then(|sources| sources.get(index))
    }
    pub fn error(&self) -> Option<&str> { self.result.as_ref().err().map(String::as_str) }
    /// Both variants (window, XR) as plain data, or the error, in one call.
    #[relax_api]
    pub fn view(&self) -> DrawSpirvResultView<'_> {
        match &self.result {
            Ok([a, b]) => DrawSpirvResultView { error: None, stages: [a.view(), b.view()] },
            Err(e) => DrawSpirvResultView { error: Some(e), stages: Default::default() },
        }
    }
}

/// [`IrDrawSpirvResult::view`].
#[relax_api(plain)]
#[repr(C)]
#[derive(Clone, Default)]
pub struct DrawSpirvResultView<'a> {
    pub error: Option<&'a str>,
    pub stages: [DrawSpirvView<'a>; 2],
}

/// One SPIR-V variant of a draw shader: both stages' words and their bindings.
#[relax_api(plain)]
#[repr(C)]
#[derive(Clone, Default)]
pub struct DrawSpirvView<'a> {
    pub info: IrDrawInfo,
    pub vertex: &'a [u32],
    pub fragment: &'a [u32],
}
pub fn draw_spirv_result(output: &ShaderOutput, types: &impl crate::shader_types::ShaderTypes) -> IrDrawSpirvResult {
    IrDrawSpirvResult { result: draw_spirv(output, types) }
}
pub fn draw_pick_spirv_result(output: &ShaderOutput, types: &impl crate::shader_types::ShaderTypes) -> IrDrawSpirvResult {
    IrDrawSpirvResult { result: (|| Ok([
        draw_variant_spirv_with_pick(output, types, false, true)?,
        draw_variant_spirv_with_pick(output, types, true, true)?,
    ]))() }
}
impl IrDrawSpirv {
    fn view(&self) -> DrawSpirvView<'_> {
        DrawSpirvView { info: self.info.clone(), vertex: &self.vertex, fragment: &self.fragment }
    }
    pub fn vertex(&self) -> &[u32] { &self.vertex }
    pub fn fragment(&self) -> &[u32] { &self.fragment }
    pub fn info(&self) -> &IrDrawInfo { &self.info }
}
impl IrDrawInfo {
    pub fn dyn_uniform_binding(&self) -> u32 { self.dyn_uniform_binding }
    pub fn texture_binding_base(&self) -> u32 { self.texture_binding_base }
    pub fn sampler_binding_base(&self) -> u32 { self.sampler_binding_base }
    pub fn xr_depth_binding(&self) -> u32 { self.xr_depth_binding }
    pub fn instance_binding(&self) -> Option<u32> { self.instance_binding }
    pub fn geometry_slots(&self) -> usize { self.geometry_slots }
    pub fn instance_slots(&self) -> usize { self.instance_slots }
    pub fn pick_binding(&self) -> Option<u32> { self.pick_binding }
    pub fn pick_texture_base(&self) -> Option<u32> { self.pick_texture_base }
}

/// What the Metal printer needs beyond the IR.
#[relax_api(plain)]
#[derive(Clone, Debug, Default)]
pub struct MetalDrawInfo {
    /// Words of one instance record (`buffer(1)`).
    pub instance_words: usize,
    /// Words of one vertex (`buffer(0)`, the geometry's packed layout).
    pub geometry_words: usize,
}

/// Appends Metal's `vertex_main` / `fragment_main`: the vertex read from
/// the geometry buffer (`buffer(0)`) by vertex id as tight words (the
/// packed vertex layout), the instance record from the instance buffer
/// (`buffer(1)`) by instance id at the words of the record layout,
/// uniforms read in place through the context, varyings packed as on every
/// backend. No XR variant and no depth clip.
pub fn build_metal_entries(output: &ShaderOutput, ir: &mut IrModule, types: &impl crate::shader_types::ShaderTypes) -> MetalDrawInfo {
    let mut info = MetalDrawInfo::default();
    let mut geometry = Vec::new();
    let mut offset = 0;
    for (i, io) in output.io.iter().enumerate() {
        if let ShaderIoKind::VertexBuffer = io.kind {
            let slots = field_slots(&types.type_data(io.ty).ty, true);
            geometry.push(RecField { io: i, pod: io.ty, slots, offset });
            offset += slots;
        }
    }
    info.geometry_words = record_end(&geometry);
    let mut instance = Vec::new();
    let mut offset = 0;
    for rust in [false, true] {
        for (i, io) in output.io.iter().enumerate() {
            let wanted = match io.kind {
                ShaderIoKind::DynInstance => !rust,
                ShaderIoKind::RustInstance => rust,
                _ => false,
            };
            if wanted {
                if let Some(lane) = io.instance_lane {
                    let slots = field_slots(&types.type_data(io.ty).ty, true);
                    instance.push(RecField { io: i, pod: io.ty, slots, offset: lane });
                    offset = offset.max(lane + slots);
                } else { push_field(types, &mut instance, i, io.ty, true, &mut offset); }
            }
        }
    }
    info.instance_words = record_end(&instance);

    let fragment_reads = match ir.function_by_name("io_fragment") {
        Some(f) => {
            let funcs = ir.reachable(f);
            ir.globals_used(&funcs)
        }
        None => vec![false; ir.globals.len()],
    };
    // The fragment stage reads the instance fields it uses from the
    // instance buffer by the instance id (passed flat), as the vertex stage
    // does: exactly the record's values, and the varyings carry only the
    // shader's own varyings.
    let mut frag_instance = Vec::new();
    for f in &instance {
        let io = &output.io[f.io];
        let kind = match io.kind {
            ShaderIoKind::DynInstance => IrGlobalKind::Io(IrIo::DynInstance),
            _ => IrGlobalKind::Io(IrIo::RustInstance),
        };
        let read = match ir.find_global(kind, io.name) {
            Some(g) => fragment_reads.get(g as usize).copied().unwrap_or(false),
            None => false,
        };
        if read {
            frag_instance.push(RecField { io: f.io, pod: f.pod, slots: f.slots, offset: f.offset });
        }
    }
    // Scalar and vector varyings travel as themselves (their own type and
    // slot, as Apple GPUs interpolate them; packing them into float4 lanes
    // moves edge pixels), the rest packed into float4 lanes.
    let mut typed = Vec::new();
    let mut varyings = Vec::new();
    let mut offset = 0;
    for (i, io) in output.io.iter().enumerate() {
        if let ShaderIoKind::Varying = io.kind {
            let pod = &types.type_data(io.ty).ty;
            if matches!(pod, ShaderTypeKind::F32 | ShaderTypeKind::F16 | ShaderTypeKind::Vec(_)) {
                typed.push(i);
            } else {
                push_field(types, &mut varyings, i, io.ty, false, &mut offset);
            }
        }
    }
    let varying_vec4s = (record_end(&varyings) + 3) / 4;

    let mut io_globals = Vec::new();
    for i in 0..output.io.len() {
        io_globals.push(io_global(types, output, ir, i));
    }
    let words = ir.ty(IrTy::Array(TY_U32, 0));
    let geom_buffer = if info.geometry_words > 0 { Some(ir.global(IrGlobalKind::InstanceBuffer, id!(_mp_geom), words)) } else { None };
    let inst_buffer = if info.instance_words > 0 { Some(ir.global(IrGlobalKind::InstanceBuffer, id!(_mp_inst), words)) } else { None };
    let iid = ir.global(IrGlobalKind::InstanceIndex, id!(_mp_instance_index), TY_U32);
    let vec4 = ir.vec_ty(4, IrScalar::F32);
    let vtx_pos = ir.global(IrGlobalKind::VertexPosition, id!(vertex_pos), vec4);

    let inputs = vec![
        IrEntryIo { name: "vertex_index".to_string(), ty: TY_U32, binding: IrIoBinding::Builtin(IrBuiltinIo::VertexIndex) },
        IrEntryIo { name: "instance_index".to_string(), ty: TY_U32, binding: IrIoBinding::Builtin(IrBuiltinIo::InstanceIndex) },
    ];
    let mut outputs = vec![IrEntryIo { name: "position".to_string(), ty: vec4, binding: IrIoBinding::Builtin(IrBuiltinIo::Position) }];
    let mut typed_out = Vec::new();
    for (k, i) in typed.iter().enumerate() {
        let t = ir.pod_ty(types, output.io[*i].ty);
        typed_out.push((*i, outputs.len() as u32, t));
        outputs.push(IrEntryIo { name: format!("var_{}", output.io[*i].name), ty: t, binding: IrIoBinding::Location(k as u32) });
    }
    let packed_base = outputs.len() as u32;
    for k in 0..varying_vec4s {
        outputs.push(IrEntryIo { name: format!("packed_varying_{}", k), ty: vec4, binding: IrIoBinding::Location((typed.len() + k) as u32) });
    }
    let iid_out = outputs.len() as u32;
    outputs.push(IrEntryIo { name: "instance_id".to_string(), ty: TY_U32, binding: IrIoBinding::Location((typed.len() + varying_vec4s) as u32) });
    let frag_inputs = outputs.clone();
    let vertex_fn = ir.function_by_name("io_vertex");
    let vertex_ret_vec4 = vertex_fn.map(|f| ir.functions[f as usize].ret == vec4).unwrap_or(false);

    let mut b = Bld { f: IrFunction::new("vertex_main".to_string(), TY_VOID), m: ir };
    b.f.entry = Some(IrEntry { stage: IrStage::Vertex, inputs, outputs });
    let v = b.e(ExprKind::EntryIn(1), TY_U32);
    let g = b.glob(iid);
    b.assign(g, v);
    for (buf, fields, stride, index_in) in [(geom_buffer, &geometry, info.geometry_words, 0u32), (inst_buffer, &instance, info.instance_words, 1u32)] {
        let Some(buf) = buf else { continue };
        let index = b.e(ExprKind::EntryIn(index_in), TY_U32);
        let st = b.u32(stride as u32);
        let base_v = b.bin(BinOp::Mul, index, st, TY_U32);
        let base = b.f.local(if index_in == 0 { id!(_mp_vb) } else { id!(_mp_ib) }, 0, TY_U32, false, LocalKind::Temp);
        b.f.body.push(Stmt::Local(base, base_v));
        let mut words = Vec::new();
        for k in 0..stride {
            let be = b.e(ExprKind::Local(base), TY_U32);
            let kk = b.u32(k as u32);
            let idx = b.bin(BinOp::Add, be, kk, TY_U32);
            let arr = b.glob(buf);
            words.push(b.e(ExprKind::Index(arr, idx), TY_U32));
        }
        for f in fields.iter() {
            let Some(g) = io_globals[f.io] else { continue };
            let inline = ShaderTypeInline { id: f.pod, data: types.type_data(f.pod).clone() };
            let mut at = f.offset;
            let v = b.reconstruct(types, &inline, Source::Word, &words, &mut at);
            let place = b.glob(g);
            b.assign(place, v);
        }
    }
    let zero = b.f32(0.0);
    let one = b.f32(1.0);
    let init = b.e(ExprKind::Construct(vec![zero, zero, zero, one]), vec4);
    let g = b.glob(vtx_pos);
    b.assign(g, init);
    if let Some(vf) = vertex_fn {
        if vertex_ret_vec4 {
            let call = b.e(ExprKind::Call(vf, Vec::new()), vec4);
            let g = b.glob(vtx_pos);
            b.assign(g, call);
        } else {
            let call = b.e(ExprKind::Call(vf, Vec::new()), TY_VOID);
            b.f.body.push(Stmt::Expr(call));
        }
    }
    let pos = b.glob(vtx_pos);
    let out0 = b.e(ExprKind::EntryOut(0), vec4);
    b.assign(out0, pos);
    let iid_v = b.e(ExprKind::EntryIn(1), TY_U32);
    let iid_o = b.e(ExprKind::EntryOut(iid_out), TY_U32);
    b.assign(iid_o, iid_v);
    for (io, out, t) in &typed_out {
        let Some(g) = io_globals[*io] else { continue };
        let v = b.glob(g);
        let o = b.e(ExprKind::EntryOut(*out), *t);
        b.assign(o, v);
    }
    for f in &varyings {
        let Some(g) = io_globals[f.io] else { continue };
        let inline = ShaderTypeInline { id: f.pod, data: types.type_data(f.pod).clone() };
        let v = b.glob(g);
        let mut lanes = Vec::new();
        b.flatten(types, &inline, v, &mut lanes);
        for s in 0..f.slots {
            let slot = f.offset + s;
            let src = match lanes.get(s) {
                Some(e) => *e,
                None => b.f32(0.0),
            };
            let o = b.e(ExprKind::EntryOut(packed_base + (slot / 4) as u32), vec4);
            let dst = b.lane(o, (slot % 4) as u8);
            b.assign(dst, src);
        }
    }
    b.f.body.push(Stmt::Return(NO_EXPR));
    let vertex_main = b.f;
    ir.functions.push(vertex_main);

    let mut outputs = Vec::new();
    let mut frag_outputs = Vec::new();
    for (i, io) in output.io.iter().enumerate() {
        if let ShaderIoKind::FragmentOutput(index) = io.kind {
            frag_outputs.push((index, i));
        }
    }
    frag_outputs.sort_by_key(|(index, _)| *index);
    for (index, i) in &frag_outputs {
        let t = ir.pod_ty(types, output.io[*i].ty);
        outputs.push(IrEntryIo { name: format!("fb{}", index), ty: t, binding: IrIoBinding::Location(*index as u32) });
    }
    let fragment_fn = ir.function_by_name("io_fragment");
    let mut b = Bld { f: IrFunction::new("fragment_main".to_string(), TY_VOID), m: ir };
    b.f.entry = Some(IrEntry { stage: IrStage::Fragment, inputs: frag_inputs, outputs });
    for (io, out, t) in &typed_out {
        let Some(g) = io_globals[*io] else { continue };
        let v = b.e(ExprKind::EntryIn(*out), *t);
        let place = b.glob(g);
        b.assign(place, v);
    }
    let mut var_lanes = Vec::new();
    for i in 0..varying_vec4s * 4 {
        let v = b.e(ExprKind::EntryIn(packed_base + (i / 4) as u32), vec4);
        var_lanes.push(b.lane(v, (i % 4) as u8));
    }
    for f in &varyings {
        let Some(g) = io_globals[f.io] else { continue };
        let inline = ShaderTypeInline { id: f.pod, data: types.type_data(f.pod).clone() };
        let mut at = f.offset;
        let v = b.reconstruct(types, &inline, Source::Numeric, &var_lanes, &mut at);
        let place = b.glob(g);
        b.assign(place, v);
    }
    if let (Some(buf), false) = (inst_buffer, frag_instance.is_empty()) {
        let iid_in = b.e(ExprKind::EntryIn(iid_out), TY_U32);
        let st = b.u32(info.instance_words as u32);
        let base_v = b.bin(BinOp::Mul, iid_in, st, TY_U32);
        let base = b.f.local(id!(_mp_ib), 0, TY_U32, false, LocalKind::Temp);
        b.f.body.push(Stmt::Local(base, base_v));
        let mut words = Vec::new();
        for k in 0..info.instance_words {
            let be = b.e(ExprKind::Local(base), TY_U32);
            let kk = b.u32(k as u32);
            let idx = b.bin(BinOp::Add, be, kk, TY_U32);
            let arr = b.glob(buf);
            words.push(b.e(ExprKind::Index(arr, idx), TY_U32));
        }
        for f in &frag_instance {
            let Some(g) = io_globals[f.io] else { continue };
            let inline = ShaderTypeInline { id: f.pod, data: types.type_data(f.pod).clone() };
            let mut at = f.offset;
            let v = b.reconstruct(types, &inline, Source::Word, &words, &mut at);
            let place = b.glob(g);
            b.assign(place, v);
        }
    }
    if let Some(ff) = fragment_fn {
        let call = b.e(ExprKind::Call(ff, Vec::new()), TY_VOID);
        b.f.body.push(Stmt::Expr(call));
    }
    for (k, (_, i)) in frag_outputs.iter().enumerate() {
        let Some(g) = io_globals[*i] else { continue };
        let ty = b.m.globals[g as usize].ty;
        let v = b.glob(g);
        let o = b.e(ExprKind::EntryOut(k as u32), ty);
        b.assign(o, v);
    }
    b.f.body.push(Stmt::Return(NO_EXPR));
    let fragment_main = b.f;
    ir.functions.push(fragment_main);
    info
}

impl IrModule {
    /// The draw owner's uniform block, shared by backend entry points and host packers.
    pub fn draw_uniform_block(&mut self, types: &impl crate::shader_types::ShaderTypes, output: &ShaderOutput) -> (TyId, Vec<usize>) {
        let mut fields = Vec::new();
        let mut indices = Vec::new();
        for (index, io) in output.io.iter().enumerate() {
            if matches!(io.kind, ShaderIoKind::Uniform) {
                let ty = self.pod_ty(types, io.ty);
                fields.push(IrField { name: io.name, ty });
                indices.push(index);
            }
        }
        (self.synthetic_struct("MpDynUniforms", fields), indices)
    }
}

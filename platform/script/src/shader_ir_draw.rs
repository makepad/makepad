//! Draw shaders as IR: the compile (bytecode -> IR through the lowering)
//! and the entry points (`vertex_main` / `fragment_main`) every backend
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

use crate::pod::{ScriptPodPacked, ScriptPodTy, ScriptPodTypeInline};
use crate::shader::{ShaderMode, ShaderType};
use crate::shader_ir::*;
use crate::shader_lower::IrFnCompiler;
use crate::shader_output::*;
use crate::trap::NoTrap;
use crate::value::{ScriptObject, ScriptPodType};
use crate::vm::ScriptVm;
use makepad_live_id::{id, LiveId};

/// What the platform binds a draw shader's resources by.
#[derive(Clone, Debug, Default)]
pub struct IrDrawInfo {
    pub dyn_uniform_binding: u32,
    pub texture_binding_base: u32,
    pub sampler_binding_base: u32,
    pub xr_depth_binding: u32,
    pub instance_binding: Option<u32>,
    pub geometry_slots: usize,
    pub instance_slots: usize,
}

/// Compiles a draw shader's `vertex` and `fragment` to IR. The io (its
/// order, kinds and buffer indices) and the samplers are the layout
/// compile's, so the IR's bindings match the mapping the platform built.
pub fn compile_draw_shader_ir(vm: &mut ScriptVm, io_self: ScriptObject, layout_source: &ShaderOutput) -> Result<ShaderOutput, String> {
    let mut output = ShaderOutput::default();
    output.const_table = layout_source.const_table;
    output.live_literals = layout_source.live_literals.clone();
    output.pre_collect_rust_instance_io(vm, io_self);
    output.pre_collect_shader_io(vm, io_self);
    for (entry, mode) in [(id!(vertex), ShaderMode::Vertex), (id!(fragment), ShaderMode::Fragment)] {
        let fnobj = vm.bx.heap.object_method(io_self, entry.into(), vm.thread().trap.pass()).as_object();
        if let Some(fnobj) = fnobj {
            output.mode = mode;
            IrFnCompiler::compile_shader_def(vm, &mut output, NoTrap, entry, fnobj, ShaderType::IoSelf(io_self), vec![]);
        }
    }
    if output.has_errors {
        return Err(format!("shader IR lowering reported errors:\n{}", output.error_report()));
    }
    let mut io = Vec::new();
    for l in &layout_source.io {
        io.push(ShaderIo { kind: l.kind.clone(), name: l.name, ty: l.ty, buffer_index: l.buffer_index });
    }
    output.io = io;
    output.samplers = layout_source.samplers.clone();
    output.assign_uniform_buffer_indices(&vm.bx.heap, 3);
    output.ir.refresh_struct_names(&vm.bx.heap);
    Ok(output)
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
    pod: ScriptPodType,
    slots: usize,
    offset: usize,
}

/// Lanes a field takes in a record (`attribute`: the fetch words; else the
/// varying lanes, compact formats unpacked).
fn field_slots(ty: &ScriptPodTy, attribute: bool) -> usize {
    match ty {
        ScriptPodTy::Packed(p) => {
            if attribute {
                p.size_of() / 4
            } else {
                p.logical_slots()
            }
        }
        ScriptPodTy::Struct { fields, .. } if !attribute && ty.has_compact_format() => {
            let mut n = 0;
            for f in fields {
                n += field_slots(&f.ty.data.ty, false);
            }
            n
        }
        _ => ty.slots(),
    }
}

fn is_int_format(ty: &ScriptPodTy) -> bool {
    use crate::pod::ScriptPodVec as V;
    match ty {
        ScriptPodTy::U32 | ScriptPodTy::AtomicU32 | ScriptPodTy::Bool | ScriptPodTy::I32 | ScriptPodTy::AtomicI32 => true,
        ScriptPodTy::Vec(v) => matches!(
            v,
            V::Vec2u | V::Vec3u | V::Vec4u | V::Vec2b | V::Vec3b | V::Vec4b | V::Vec2i | V::Vec3i | V::Vec4i
        ),
        _ => false,
    }
}

fn push_field(vm: &ScriptVm, out: &mut Vec<RecField>, io_index: usize, pod: ScriptPodType, attribute: bool, offset: &mut usize) {
    let ty = &vm.bx.heap.pod_type_ref(pod).ty;
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
fn io_global(vm: &ScriptVm, output: &ShaderOutput, ir: &mut IrModule, io: usize) -> Option<GlobalId> {
    let kind = output.io[io].kind.clone();
    let name = output.io[io].name;
    let pod = output.io[io].ty;
    let gk = crate::shader_lower_vars::global_kind_of(&kind)?;
    let ty = match gk {
        IrGlobalKind::Io(IrIo::Texture(t)) => ir.ty(IrTy::Texture(t)),
        _ => ir.pod_ty(&vm.bx.heap, pod),
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

    /// A record lane converted to `target` (shader_wgsl.rs
    /// `wgsl_convert_scalar_expr`).
    fn scalar(&mut self, src: Source, x: ExprId, target: &ScriptPodTy) -> ExprId {
        match target {
            ScriptPodTy::F32 => match src {
                Source::Word => self.bitcast(x, TY_F32),
                _ => x,
            },
            ScriptPodTy::F16 => {
                let f = match src {
                    Source::Word => self.bitcast(x, TY_F32),
                    _ => x,
                };
                self.convert(f, TY_F16)
            }
            ScriptPodTy::U32 | ScriptPodTy::AtomicU32 => match src {
                Source::Attr => self.bitcast(x, TY_U32),
                Source::Word => x,
                Source::Numeric => self.convert(x, TY_U32),
            },
            ScriptPodTy::I32 | ScriptPodTy::AtomicI32 => match src {
                Source::Attr | Source::Word => self.bitcast(x, TY_I32),
                Source::Numeric => self.convert(x, TY_I32),
            },
            ScriptPodTy::Bool => match src {
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

    /// A compact leaf from its words (shader_wgsl.rs `wgsl_reconstruct_packed`).
    fn packed(&mut self, p: ScriptPodPacked, src: Source, lanes: &[ExprId], at: &mut usize) -> ExprId {
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
            ScriptPodPacked::F16x4 => {
                let w1 = take(lanes, at, self);
                let b1 = self.bits(src, w1);
                let a = self.builtin(Builtin::Unpack2x16Float, vec![b0], v2);
                let b = self.builtin(Builtin::Unpack2x16Float, vec![b1], v2);
                self.e(ExprKind::Construct(vec![a, b]), v4)
            }
            ScriptPodPacked::F16x2 => self.builtin(Builtin::Unpack2x16Float, vec![b0], v2),
            ScriptPodPacked::U16x2 => {
                let mask = self.u32(0xffff);
                let lo = self.bin(BinOp::BitAnd, b0, mask, TY_U32);
                let sh = self.u32(16);
                let hi = self.bin(BinOp::Shr, b0, sh, TY_U32);
                let lo = self.convert(lo, TY_F32);
                let hi = self.convert(hi, TY_F32);
                self.e(ExprKind::Construct(vec![lo, hi]), v2)
            }
            ScriptPodPacked::I16x2 => {
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
            ScriptPodPacked::U16x2Norm => self.builtin(Builtin::Unpack2x16Unorm, vec![b0], v2),
            ScriptPodPacked::I16x2Norm => self.builtin(Builtin::Unpack2x16Snorm, vec![b0], v2),
            ScriptPodPacked::U8x4Norm => self.builtin(Builtin::Unpack4x8Unorm, vec![b0], v4),
            ScriptPodPacked::I8x4Norm => self.builtin(Builtin::Unpack4x8Snorm, vec![b0], v4),
        }
    }

    /// A value of `ty` rebuilt from record lanes (shader_wgsl.rs
    /// `wgsl_reconstruct_inline`).
    fn reconstruct(&mut self, vm: &ScriptVm, ty: &ScriptPodTypeInline, src: Source, lanes: &[ExprId], at: &mut usize) -> ExprId {
        match &ty.data.ty {
            ScriptPodTy::Struct { fields, .. } => {
                let mut parts = Vec::new();
                for f in fields {
                    parts.push(self.reconstruct(vm, &f.ty, src, lanes, at));
                }
                let t = self.m.pod_inline_ty(&vm.bx.heap, ty);
                self.e(ExprKind::Construct(parts), t)
            }
            ScriptPodTy::Vec(v) => {
                let elem = v.elem_ty();
                let mut parts = Vec::new();
                for _ in 0..v.dims() {
                    let x = take(lanes, at, self);
                    parts.push(self.scalar(src, x, &elem));
                }
                let t = self.m.pod_inline_ty(&vm.bx.heap, ty);
                self.e(ExprKind::Construct(parts), t)
            }
            ScriptPodTy::Mat(m) => {
                let mut parts = Vec::new();
                for _ in 0..m.dim() {
                    let x = take(lanes, at, self);
                    parts.push(self.scalar(src, x, &ScriptPodTy::F32));
                }
                let t = self.m.pod_inline_ty(&vm.bx.heap, ty);
                self.e(ExprKind::Construct(parts), t)
            }
            ScriptPodTy::Packed(p) => self.packed(*p, src, lanes, at),
            other => {
                let x = take(lanes, at, self);
                self.scalar(src, x, other)
            }
        }
    }

    /// A value's f32 lanes for the varying stream (shader_wgsl.rs
    /// `wgsl_flatten_inline`).
    fn flatten(&mut self, vm: &ScriptVm, ty: &ScriptPodTypeInline, v: ExprId, out: &mut Vec<ExprId>) {
        match &ty.data.ty {
            ScriptPodTy::Struct { fields, .. } => {
                for (i, f) in fields.iter().enumerate() {
                    let ft = self.m.pod_inline_ty(&vm.bx.heap, &f.ty);
                    let fe = self.e(ExprKind::Field(v, i as u32), ft);
                    self.flatten(vm, &f.ty, fe, out);
                }
            }
            ScriptPodTy::Vec(vt) => {
                for c in 0..vt.dims() {
                    let x = self.lane(v, c as u8);
                    out.push(self.to_f32(x));
                }
            }
            ScriptPodTy::Mat(m) => {
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
            ScriptPodTy::Packed(p) => {
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
pub fn build_draw_entries(output: &ShaderOutput, ir: &mut IrModule, vm: &ScriptVm, xr: bool) -> IrDrawInfo {
    let mut info = IrDrawInfo::default();

    // Records.
    let mut geometry = Vec::new();
    let mut instance = Vec::new();
    let mut offset = 0;
    for (i, io) in output.io.iter().enumerate() {
        if let ShaderIoKind::VertexBuffer = io.kind {
            push_field(vm, &mut geometry, i, io.ty, true, &mut offset);
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
                push_field(vm, &mut instance, i, io.ty, true, &mut offset);
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
                push_field(vm, &mut varyings, i, io.ty, false, &mut offset);
            }
        }
    }
    for (i, io) in output.io.iter().enumerate() {
        if let ShaderIoKind::Varying = io.kind {
            push_field(vm, &mut varyings, i, io.ty, false, &mut offset);
        }
    }
    let varying_slots = record_end(&varyings);

    // Bindings.
    info.dyn_uniform_binding = 2;
    let ub = output.get_uniform_buffer_bindings(&vm.bx.heap);
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
                let t = ir.pod_ty(&vm.bx.heap, io.ty);
                dyn_fields.push(IrField { name: io.name, ty: t });
                dyn_ios.push(i);
            }
            ShaderIoKind::ScopeUniform => {
                let t = ir.pod_ty(&vm.bx.heap, io.ty);
                scope_fields.push(IrField { name: io.name, ty: t });
                scope_ios.push(i);
            }
            _ => {}
        }
    }
    let dyn_block = if dyn_fields.is_empty() {
        None
    } else {
        let t = ir.synthetic_struct(id!(MpDynUniforms), dyn_fields);
        let g = ir.global(IrGlobalKind::DynUniformBlock, id!(_mp_dyn_uniforms), t);
        ir.globals[g as usize].binding = Some(info.dyn_uniform_binding);
        Some(g)
    };
    let scope_block = if scope_fields.is_empty() {
        None
    } else {
        let t = ir.synthetic_struct(id!(MpScopeUniforms), scope_fields);
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
                if let Some(g) = io_global(vm, output, ir, i) {
                    ir.globals[g as usize].binding = Some(binding);
                }
            }
            ShaderIoKind::Texture(_) => {
                if texture_base.is_none() {
                    texture_base = Some(next);
                }
                if let Some(g) = io_global(vm, output, ir, i) {
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
        let b = crate::shader_wgsl::wgsl_instance_binding(next);
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

    // The XR depth clip helper (shader_wgsl.rs `depth_clip`).
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
        io_globals.push(io_global(vm, output, ir, i));
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
        let inline = ScriptPodTypeInline { self_ref: f.pod, data: vm.bx.heap.pod_type_ref(f.pod).clone() };
        let mut at = f.offset;
        let v = b.reconstruct(vm, &inline, Source::Attr, &geo_lanes, &mut at);
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
            let inline = ScriptPodTypeInline { self_ref: f.pod, data: vm.bx.heap.pod_type_ref(f.pod).clone() };
            let mut at = f.offset;
            let v = b.reconstruct(vm, &inline, Source::Word, &words, &mut at);
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
        let inline = ScriptPodTypeInline { self_ref: f.pod, data: vm.bx.heap.pod_type_ref(f.pod).clone() };
        let v = b.glob(g);
        let mut lanes = Vec::new();
        b.flatten(vm, &inline, v, &mut lanes);
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
        let t = ir.pod_ty(&vm.bx.heap, output.io[*i].ty);
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
        let inline = ScriptPodTypeInline { self_ref: f.pod, data: vm.bx.heap.pod_type_ref(f.pod).clone() };
        let mut at = f.offset;
        let v = b.reconstruct(vm, &inline, Source::Numeric, &var_lanes, &mut at);
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

pub fn compile_draw_shader_spirv(vm: &mut ScriptVm, io_self: ScriptObject, layout_source: &ShaderOutput) -> Result<[IrDrawSpirv; 2], String> {
    let output = compile_draw_shader_ir(vm, io_self, layout_source)?;
    let window = draw_variant_spirv(&output, vm, false)?;
    let xr = draw_variant_spirv(&output, vm, true)?;
    Ok([window, xr])
}

pub fn draw_variant_spirv(output: &ShaderOutput, vm: &ScriptVm, xr: bool) -> Result<IrDrawSpirv, String> {
    let mut ir = output.ir.clone();
    let info = build_draw_entries(output, &mut ir, vm, xr);
    let (Some(v), Some(f)) = (ir.function_by_name("vertex_main"), ir.function_by_name("fragment_main")) else {
        return Err("draw shader has no entry points".to_string());
    };
    let vertex = crate::shader_ir_spirv::ir_to_spirv(&ir, v, xr).map_err(|e| format!("vertex: {}", e))?;
    let fragment = crate::shader_ir_spirv::ir_to_spirv(&ir, f, xr).map_err(|e| format!("fragment: {}", e))?;
    Ok(IrDrawSpirv { info, vertex, fragment })
}

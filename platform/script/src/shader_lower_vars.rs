//! Lowering of variables, fields, indexing and assignments, and of `self`'s
//! IO fields (shader_vars.rs's rules).

use crate::mod_shader::*;
use crate::pod::ScriptPodTy;
use crate::shader::{ShaderFnCompiler, ShaderType};
use crate::shader_ir::*;
use crate::shader_lower::*;
use crate::shader_output::*;
use crate::shader_tables::*;
use crate::suggest::*;
use crate::trap::*;
use crate::value::*;
use crate::vm::*;
use crate::*;
use makepad_live_id::*;

/// The IO kind of a shader IO marker in a stage (the backend-independent
/// half of `ShaderBackend::get_shader_io_kind_and_prefix`).
pub(crate) fn io_kind_for(mode: ShaderMode, io_type: ShaderIoType) -> Result<ShaderIoKind, String> {
    if io_type.0 >= SHADER_IO_FRAGMENT_OUTPUT_0.0 && io_type.0 <= SHADER_IO_FRAGMENT_OUTPUT_MAX.0 {
        return Ok(ShaderIoKind::FragmentOutput((io_type.0 - SHADER_IO_FRAGMENT_OUTPUT_0.0) as u8));
    }
    if let Some(t) = texture_type_of(io_type) {
        return Ok(ShaderIoKind::Texture(t));
    }
    Ok(match io_type {
        SHADER_IO_RUST_INSTANCE => ShaderIoKind::RustInstance,
        SHADER_IO_DYN_INSTANCE => ShaderIoKind::DynInstance,
        SHADER_IO_DYN_UNIFORM => ShaderIoKind::Uniform,
        SHADER_IO_UNIFORM_BUFFER => ShaderIoKind::UniformBuffer,
        SHADER_IO_VARYING => ShaderIoKind::Varying,
        SHADER_IO_VERTEX_POSITION => ShaderIoKind::VertexPosition,
        SHADER_IO_VERTEX_BUFFER => {
            if matches!(mode, ShaderMode::Fragment) {
                return Err("a geometry attribute (self.geom.*) was read in `pixel:`, but vertex attributes only exist in the vertex stage; pass it through a varying".to_string());
            }
            ShaderIoKind::VertexBuffer
        }
        SHADER_IO_SAMPLER => ShaderIoKind::Sampler(ShaderSamplerOptions::default()),
        SHADER_IO_SCOPE_UNIFORM => ShaderIoKind::ScopeUniform,
        _ => return Err(format!("no lowering for io type {:?} in {:?} stage", io_type, mode)),
    })
}

fn kinds_match(a: &ShaderIoKind, b: &ShaderIoKind) -> bool {
    use ShaderIoKind::*;
    match (a, b) {
        (StorageBuffer(_), StorageBuffer(_)) => true,
        (UniformBuffer, UniformBuffer) => true,
        (Sampler(_), Sampler(_)) => true,
        (Texture(at), Texture(bt)) => at == bt,
        (Varying, Varying) => true,
        (VertexBuffer, VertexBuffer) => true,
        (VertexPosition, VertexPosition) => true,
        (FragmentOutput(ai), FragmentOutput(bi)) => ai == bi,
        (RustInstance, RustInstance) => true,
        (Uniform, Uniform) => true,
        (DynInstance, DynInstance) => true,
        (ScopeUniform, ScopeUniform) => true,
        (RustInstance, DynInstance) | (DynInstance, RustInstance) => true,
        _ => false,
    }
}

/// The global kind a registered IO entry reads as.
pub(crate) fn global_kind_of(kind: &ShaderIoKind) -> Option<IrGlobalKind> {
    Some(match kind {
        ShaderIoKind::VertexBuffer => IrGlobalKind::Io(IrIo::Geometry),
        ShaderIoKind::DynInstance => IrGlobalKind::Io(IrIo::DynInstance),
        ShaderIoKind::RustInstance => IrGlobalKind::Io(IrIo::RustInstance),
        ShaderIoKind::Uniform => IrGlobalKind::Io(IrIo::Uniform),
        ShaderIoKind::ScopeUniform => IrGlobalKind::Io(IrIo::ScopeUniform),
        ShaderIoKind::UniformBuffer => IrGlobalKind::Io(IrIo::UniformBuffer),
        ShaderIoKind::Varying => IrGlobalKind::Io(IrIo::Varying),
        ShaderIoKind::FragmentOutput(i) => IrGlobalKind::Io(IrIo::FragmentOutput(*i)),
        ShaderIoKind::Texture(t) => IrGlobalKind::Io(IrIo::Texture(*t)),
        ShaderIoKind::VertexPosition => IrGlobalKind::VertexPosition,
        _ => return None,
    })
}

/// Swizzle lanes of a vector field name (`x`, `xy`, `rgba` ...).
pub(crate) fn swizzle_lanes(name: LiveId) -> Option<([u8; 4], u8)> {
    let text = name.as_string(|s| s.map(|s| s.to_string()))?;
    let b = text.as_bytes();
    if b.is_empty() || b.len() > 4 {
        return None;
    }
    let mut lanes = [0u8; 4];
    for (i, c) in b.iter().enumerate() {
        lanes[i] = match c {
            b'x' | b'r' => 0,
            b'y' | b'g' => 1,
            b'z' | b'b' => 2,
            b'w' | b'a' => 3,
            _ => return None,
        };
    }
    Some((lanes, b.len() as u8))
}

impl IrFnCompiler {
    /// `instance.field` of a struct or a vector (a swizzle).
    pub(crate) fn field_expr(&mut self, vm: &ScriptVm, output: &mut ShaderOutput, instance: ExprId, pod_ty: ScriptPodType, field_id: LiveId, ret_ty: ScriptPodType) -> ExprId {
        let ret = Self::ir_ty(vm, output, ret_ty);
        let inst_ty = Self::ir_ty(vm, output, pod_ty);
        match output.ir.get(inst_ty).clone() {
            IrTy::Struct(s) => {
                let mut index = 0;
                for (i, f) in output.ir.structs[s as usize].fields.iter().enumerate() {
                    if f.name == field_id {
                        index = i as u32;
                    }
                }
                self.expr(ExprKind::Field(instance, index), ret)
            }
            IrTy::Vec(..) => match swizzle_lanes(field_id) {
                Some((lanes, n)) => self.expr(ExprKind::Swizzle(instance, lanes, n), ret),
                None => {
                    script_err_not_found!(self.trap, "bad swizzle {:?}", field_id);
                    self.err_expr()
                }
            },
            _ => {
                script_err_not_found!(self.trap, "field {:?} on a value that has no fields", field_id);
                self.err_expr()
            }
        }
    }

    pub(crate) fn check_index_type(&mut self, vm: &ScriptVm, index_ty: &ShaderType) {
        let b = &vm.bx.code.builtins.pod;
        match index_ty {
            ShaderType::AbstractInt => {}
            ShaderType::Pod(t) if *t == b.pod_i32 || *t == b.pod_u32 => {}
            _ => {
                let got = match index_ty {
                    ShaderType::Pod(t) => format_pod_type_name(&vm.bx.heap, *t),
                    _ => format!("{:?}", index_ty),
                };
                script_err_pod!(self.trap, "array index must be integer, got {}", got);
            }
        }
    }

    /// `instance[index]` with the index proven or clamped into range
    /// (shader_vars.rs `clamped_shader_index`).
    pub(crate) fn index_expr(&mut self, vm: &ScriptVm, output: &mut ShaderOutput, instance: ExprId, instance_ty: ScriptPodType, index_ty: &ShaderType, index: ExprId, ret_ty: ScriptPodType) -> ExprId {
        let ret = Self::ir_ty(vm, output, ret_ty);
        let len = crate::shader_vars::shader_index_len(&vm.bx.heap.pod_types[instance_ty.index as usize].ty);
        let Some(len) = len else {
            output.push_error("shader index: an unsized array cannot be indexed in a shader".to_string());
            script_err_shader!(self.trap, "shader index: an unsized array cannot be indexed in a shader");
            return self.err_expr();
        };
        if len == 0 {
            output.push_error("shader index: cannot index an empty array".to_string());
            script_err_shader!(self.trap, "shader index: cannot index an empty array");
            return self.err_expr();
        }
        let lit = if index == NO_EXPR { None } else {
            match self.f.exprs[index as usize].kind {
                ExprKind::Lit(IrLit::AbstractInt(v)) => Some(v),
                ExprKind::Lit(IrLit::I32(v)) => Some(v as i64),
                ExprKind::Lit(IrLit::U32(v)) => Some(v as i64),
                _ => None,
            }
        };
        if let Some(v) = lit {
            if v < 0 || v as u64 >= len as u64 {
                let message = format!("index {v} is out of bounds for length {len}");
                output.push_error(format!("shader index: {message}"));
                script_err_shader!(self.trap, "shader index: {}", message);
                return self.err_expr();
            }
            self.concretize(index, IrScalar::I32);
            return self.expr(ExprKind::Index(instance, index), ret);
        }
        let max = (len - 1) as u32;
        let unsigned = matches!(index_ty, ShaderType::Pod(t) if *t == vm.bx.code.builtins.pod.pod_u32);
        let clamped = if unsigned {
            let m = self.expr(ExprKind::Lit(IrLit::U32(max)), TY_U32);
            self.expr(ExprKind::Builtin(Builtin::Min, vec![index, m]), TY_U32)
        } else {
            let i = if self.f.exprs[index as usize].ty == TY_I32 {
                index
            } else {
                self.concretize_default(index);
                if self.f.exprs[index as usize].ty == TY_I32 {
                    index
                } else {
                    self.expr(ExprKind::Convert(index), TY_I32)
                }
            };
            let lo = self.expr(ExprKind::Lit(IrLit::I32(0)), TY_I32);
            let hi = self.expr(ExprKind::Lit(IrLit::I32(max as i32)), TY_I32);
            self.expr(ExprKind::Builtin(Builtin::Clamp, vec![i, lo, hi]), TY_I32)
        };
        self.expr(ExprKind::Index(instance, clamped), ret)
    }

    fn void_nop(&mut self, vm: &ScriptVm) {
        let n = self.nop();
        self.push(ShaderType::Pod(vm.bx.code.builtins.pod.pod_void), n);
    }

    pub(crate) fn handle_assign(&mut self, vm: &mut ScriptVm, output: &mut ShaderOutput) {
        let (value_ty, value) = self.pop();
        let (value_ty, value) = self.resolve_value(vm, output, value_ty, value);
        let (id_ty, _) = self.pop();
        let ShaderType::Id(id) = id_ty else {
            script_err_immutable!(self.trap, "shader assign target is not an id");
            return self.void_nop(vm);
        };
        let Some(var) = self.scope.find(id) else {
            script_err_not_found!(
                self.trap,
                "variable {:?} not found in shader scope{}",
                id,
                suggest_from_live_ids(id, &self.scope.all_names())
            );
            return self.void_nop(vm);
        };
        if !matches!(var, IrVar::Local(_, _, true)) {
            script_err_immutable!(self.trap, "cannot assign to let binding {:?}", id);
        }
        let _ = value_ty;
        let target = var.pod();
        let place = self.var_expr(vm, output, var);
        let value = self.coerce(vm, output, value, target);
        self.emit(Stmt::Assign(place, value));
        self.void_nop(vm);
    }

    pub(crate) fn handle_assign_field(&mut self, vm: &mut ScriptVm, output: &mut ShaderOutput) {
        let (value_ty, value) = self.pop_resolved(vm, output);
        let (field_ty, _) = self.pop();
        let (instance_ty, instance) = self.pop_resolved(vm, output);
        let pods = vm.bx.code.builtins.pod.clone();
        let ShaderType::Id(field_id) = field_ty else {
            script_err_unexpected!(self.trap, "field assign requires Id field type");
            return self.void_nop(vm);
        };
        match instance_ty {
            ShaderType::Pod(pod_ty) | ShaderType::PodPtr(pod_ty) => {
                let Some(ret_ty) = vm.bx.heap.pod_field_type(pod_ty, field_id, &pods) else {
                    script_err_not_found!(
                        self.trap,
                        "field {:?} not found on pod{}",
                        field_id,
                        suggest_pod_field(&vm.bx.heap, pod_ty, field_id)
                    );
                    return self.void_nop(vm);
                };
                let val_ty = if matches!(value_ty, ShaderType::AbstractInt) && (ret_ty == pods.pod_u32 || ret_ty == pods.pod_f32 || ret_ty == pods.pod_f16) {
                    ret_ty
                } else {
                    value_ty.make_concrete(&pods).unwrap_or(pods.pod_void)
                };
                if val_ty != ret_ty && !matches!(value_ty, ShaderType::AbstractFloat) {
                    script_err_pod!(
                        self.trap,
                        "field {:?} type mismatch: expected {}, got {}",
                        field_id,
                        format_pod_type_name(&vm.bx.heap, ret_ty),
                        format_pod_type_name(&vm.bx.heap, val_ty)
                    );
                }
                let place = self.field_expr(vm, output, instance, pod_ty, field_id, ret_ty);
                let value = self.coerce(vm, output, value, ret_ty);
                self.emit(Stmt::Assign(place, value));
                self.void_nop(vm);
            }
            ShaderType::IoSelf(obj) => {
                let value_v = vm.bx.heap.value(obj, field_id.into(), self.trap.pass());
                if let Some(value_obj) = value_v.as_object() {
                    if let Some(io_type) = vm.bx.heap.as_shader_io(value_obj) {
                        let allowed = match io_type {
                            SHADER_IO_VARYING => output.mode == ShaderMode::Vertex,
                            SHADER_IO_VERTEX_POSITION => output.mode == ShaderMode::Vertex,
                            t if t.0 >= SHADER_IO_FRAGMENT_OUTPUT_0.0 && t.0 <= SHADER_IO_FRAGMENT_OUTPUT_MAX.0 => output.mode == ShaderMode::Fragment,
                            _ => false,
                        };
                        if !allowed {
                            script_err_immutable!(self.trap, "cannot assign to shader io in this mode");
                            return self.void_nop(vm);
                        }
                        let proto = vm.bx.heap.proto(value_obj);
                        let ty = ShaderFnCompiler::type_from_value(vm, proto);
                        let concrete = match ty {
                            ShaderType::Pod(pt) | ShaderType::PodType(pt) => Some(pt),
                            _ => None,
                        };
                        if let Some(pod_ty) = concrete {
                            let val_ty = value_ty.make_concrete(&pods).unwrap_or(pods.pod_void);
                            if val_ty != pod_ty && !matches!(value_ty, ShaderType::AbstractInt | ShaderType::AbstractFloat) {
                                script_err_pod!(
                                    self.trap,
                                    "shader io field {:?} type mismatch: expected {}, got {}",
                                    field_id,
                                    format_pod_type_name(&vm.bx.heap, pod_ty),
                                    format_pod_type_name(&vm.bx.heap, val_ty)
                                );
                            }
                            let kind = match io_kind_for(output.mode, io_type) {
                                Ok(k) => k,
                                Err(e) => {
                                    output.push_error(format!("shader: {}", e));
                                    return self.void_nop(vm);
                                }
                            };
                            let mut exists = false;
                            for io in &output.io {
                                if io.name == field_id {
                                    exists = true;
                                }
                            }
                            if !exists {
                                output.io.push(ShaderIo { kind, name: field_id, ty: pod_ty, buffer_index: None });
                            }
                            let place = self.io_global_expr(vm, output, field_id, pod_ty);
                            let value = self.coerce(vm, output, value, pod_ty);
                            self.emit(Stmt::Assign(place, value));
                            return self.void_nop(vm);
                        }
                    }
                }
                script_err_shader!(self.trap, "no matching shader type for self field");
                self.void_nop(vm);
            }
            _ => {
                script_err_shader!(self.trap, "no matching shader type for instance");
                self.void_nop(vm);
            }
        }
    }

    /// The global an IO entry (by name, as registered) reads as.
    pub(crate) fn io_global_expr(&mut self, vm: &ScriptVm, output: &mut ShaderOutput, name: LiveId, pod_ty: ScriptPodType) -> ExprId {
        let mut kind = None;
        for io in &output.io {
            if io.name == name {
                kind = Some(io.kind.clone());
                break;
            }
        }
        let gk = kind.as_ref().and_then(global_kind_of);
        let Some(gk) = gk else {
            script_err_shader!(self.trap, "shader io {:?} has no shader representation", name);
            return self.err_expr();
        };
        let ty = match gk {
            IrGlobalKind::Io(IrIo::Texture(t)) => output.ir.ty(IrTy::Texture(t)),
            _ => Self::ir_ty(vm, output, pod_ty),
        };
        let g = output.ir.global(gk, name, ty);
        self.expr(ExprKind::Global(g), ty)
    }

    pub(crate) fn handle_array_index(&mut self, vm: &mut ScriptVm, output: &mut ShaderOutput) {
        let (index_ty, index) = self.pop_resolved(vm, output);
        let (instance_ty, instance) = self.pop_resolved(vm, output);
        let pods = vm.bx.code.builtins.pod.clone();
        let ShaderType::Pod(pod_ty) = instance_ty else {
            script_err_shader!(self.trap, "array index requires Pod type, got {:?}", instance_ty);
            return self.void_nop(vm);
        };
        let elem = type_table_elem_type(&vm.bx.heap.pod_types[pod_ty.index as usize].ty, self.trap.pass(), &pods);
        let Some(ret_ty) = elem else {
            script_err_shader!(self.trap, "type is not indexable");
            return self.void_nop(vm);
        };
        self.check_index_type(vm, &index_ty);
        let e = self.index_expr(vm, output, instance, pod_ty, &index_ty, index, ret_ty);
        self.push(ShaderType::Pod(ret_ty), e);
    }

    pub(crate) fn handle_assign_index(&mut self, vm: &mut ScriptVm, output: &mut ShaderOutput) {
        let (value_ty, value) = self.pop_resolved(vm, output);
        let (index_ty, index) = self.pop_resolved(vm, output);
        let (instance_ty, instance) = self.pop_resolved(vm, output);
        let pods = vm.bx.code.builtins.pod.clone();
        let ShaderType::Pod(pod_ty) = instance_ty else {
            script_err_shader!(self.trap, "index assign requires Pod type");
            return self.void_nop(vm);
        };
        let elem = type_table_elem_type(&vm.bx.heap.pod_types[pod_ty.index as usize].ty, self.trap.pass(), &pods);
        let Some(ret_ty) = elem else {
            script_err_immutable!(self.trap, "index assign not supported for this type");
            return self.void_nop(vm);
        };
        self.check_index_type(vm, &index_ty);
        let val_ty = if matches!(value_ty, ShaderType::AbstractInt) && (ret_ty == pods.pod_u32 || ret_ty == pods.pod_f32 || ret_ty == pods.pod_f16) {
            ret_ty
        } else {
            value_ty.make_concrete(&pods).unwrap_or(pods.pod_void)
        };
        if val_ty != ret_ty && !matches!(value_ty, ShaderType::AbstractFloat) {
            script_err_pod!(
                self.trap,
                "index assign type mismatch: expected {}, got {}",
                format_pod_type_name(&vm.bx.heap, ret_ty),
                format_pod_type_name(&vm.bx.heap, val_ty)
            );
        }
        let place = self.index_expr(vm, output, instance, pod_ty, &index_ty, index, ret_ty);
        let value = self.coerce(vm, output, value, ret_ty);
        self.emit(Stmt::Assign(place, value));
        self.void_nop(vm);
    }

    pub(crate) fn handle_assign_me(&mut self, vm: &mut ScriptVm, output: &mut ShaderOutput) {
        let (val_ty, val) = self.pop();
        let (id_ty, _) = self.pop();
        let ShaderType::Id(id) = id_ty else {
            script_err_unexpected!(self.trap, "assign_me requires Id type");
            return self.void_nop(vm);
        };
        let (val_ty, val) = self.resolve_value(vm, output, val_ty, val);
        if let Some(IrMe::Pod { args, .. }) = self.mes.last_mut() {
            let mixed = args.last().map(|l| l.name.is_none()).unwrap_or(false);
            args.push(IrPodArg { name: Some(id), ty: val_ty, e: val });
            if mixed {
                script_err_pod!(self.trap, "mixing named and ordered args");
            }
        } else {
            script_err_unexpected!(self.trap, "assign_me requires Pod on me stack");
        }
    }

    fn infer_unmarked(vm: &ScriptVm, value: ScriptValue) -> Option<(ShaderIoType, ScriptPodType)> {
        let pod_ty = match ShaderFnCompiler::type_from_value(vm, value) {
            ShaderType::Pod(p) | ShaderType::PodType(p) => p,
            _ => return None,
        };
        let io_type = match &vm.bx.heap.pod_types[pod_ty.index as usize].ty {
            ScriptPodTy::F32 | ScriptPodTy::F16 | ScriptPodTy::U32 | ScriptPodTy::I32 => SHADER_IO_DYN_INSTANCE,
            ScriptPodTy::Vec(_) | ScriptPodTy::Mat(_) => SHADER_IO_DYN_UNIFORM,
            _ => return None,
        };
        Some((io_type, pod_ty))
    }

    /// Registers `field_id` as IO of kind `kind` (an existing entry keeps
    /// its kind) and pushes its read.
    fn push_io_field(&mut self, vm: &mut ScriptVm, output: &mut ShaderOutput, field_id: LiveId, kind: ShaderIoKind, pod_ty: ScriptPodType) {
        let mut existing = None;
        for io in &output.io {
            if io.name == field_id {
                existing = Some(io.kind.clone());
            }
        }
        let _kind = match existing {
            Some(k) if !kinds_match(&k, &kind) => k,
            _ => kind.clone(),
        };
        vm.bx.heap.pod_type_name_if_not_set(pod_ty, field_id);
        let mut exists = false;
        for io in &output.io {
            if io.name == field_id {
                exists = true;
            }
        }
        if !exists {
            output.validate_vertex_fetch_io(&vm.bx.heap, &kind, field_id, pod_ty);
            output.io.push(ShaderIo { kind, name: field_id, ty: pod_ty, buffer_index: None });
        }
        let e = self.io_global_expr(vm, output, field_id, pod_ty);
        self.push(ShaderType::Pod(pod_ty), e);
    }

    pub(crate) fn handle_field(&mut self, vm: &mut ScriptVm, output: &mut ShaderOutput) {
        let (field_ty, field_e) = self.pop();
        let (instance_ty, instance) = self.pop_resolved(vm, output);
        let pods = vm.bx.code.builtins.pod.clone();
        let ShaderType::Id(field_id) = field_ty else {
            script_err_not_found!(self.trap, "field {:?} not found on shader type {:?}", field_ty, instance_ty);
            return self.void_nop(vm);
        };
        match instance_ty {
            ShaderType::Pod(pod_ty) | ShaderType::PodPtr(pod_ty) => {
                let Some(ret_ty) = vm.bx.heap.pod_field_type(pod_ty, field_id, &pods) else {
                    script_err_not_found!(
                        self.trap,
                        "field {:?} not found on Pod{}",
                        field_id,
                        suggest_pod_field(&vm.bx.heap, pod_ty, field_id)
                    );
                    return self.void_nop(vm);
                };
                // The view-dependent pass matrices.
                if instance != NO_EXPR {
                    if let ExprKind::Global(g) = self.f.exprs[instance as usize].kind {
                        let glob = &output.ir.globals[g as usize];
                        if glob.kind == IrGlobalKind::Io(IrIo::UniformBuffer) && glob.name == id!(draw_pass) {
                            let which = match field_id {
                                id!(camera_projection) => Some(PassMatrix::CameraProjection),
                                id!(camera_view) => Some(PassMatrix::CameraView),
                                id!(depth_projection) => Some(PassMatrix::DepthProjection),
                                id!(depth_view) => Some(PassMatrix::DepthView),
                                id!(camera_inv) => Some(PassMatrix::CameraInv),
                                _ => None,
                            };
                            if let Some(which) = which {
                                let t = Self::ir_ty(vm, output, ret_ty);
                                let e = self.expr(ExprKind::PassMatrix(which), t);
                                return self.push(ShaderType::Pod(ret_ty), e);
                            }
                        }
                    }
                }
                let logical = logical_fetch_pod_type(vm, ret_ty);
                let e = self.field_expr(vm, output, instance, pod_ty, field_id, ret_ty);
                self.push(ShaderType::Pod(logical), e);
            }
            ShaderType::Texture(tex_type) => {
                self.push(ShaderType::Texture(tex_type), instance);
                self.push(ShaderType::Id(field_id), field_e);
            }
            ShaderType::ScopeObject(obj) => self.handle_scope_object_field(vm, output, obj, field_id, field_e),
            ShaderType::ScopeUniformBuffer { obj, pod_ty } => {
                let Some(ret_ty) = vm.bx.heap.pod_field_type(pod_ty, field_id, &pods) else {
                    script_err_not_found!(
                        self.trap,
                        "field {:?} not found on ScopeUniformBuffer{}",
                        field_id,
                        suggest_pod_field(&vm.bx.heap, pod_ty, field_id)
                    );
                    return self.void_nop(vm);
                };
                let mut shader_name = None;
                for sub in &output.scope_uniform_buffers {
                    if sub.obj == obj {
                        shader_name = Some(sub.shader_name);
                    }
                }
                let shader_name = match shader_name {
                    Some(n) => n,
                    None => {
                        let (shader_name, struct_type_name) = generate_scope_uniform_buffer_names(output, obj);
                        output.scope_uniform_buffers.push(ScopeUniformBufferSource { obj, pod_ty, shader_name });
                        vm.bx.heap.pod_type_name_if_not_set(pod_ty, struct_type_name);
                        output.io.push(ShaderIo {
                            kind: ShaderIoKind::UniformBuffer,
                            name: shader_name,
                            ty: pod_ty,
                            buffer_index: None,
                        });
                        shader_name
                    }
                };
                let base = self.io_global_expr(vm, output, shader_name, pod_ty);
                let e = self.field_expr(vm, output, base, pod_ty, field_id, ret_ty);
                self.push(ShaderType::Pod(ret_ty), e);
            }
            ShaderType::IoSelf(obj) => self.handle_io_self_field(vm, output, obj, field_id),
            _ => {
                script_err_not_found!(self.trap, "field {:?} not found on shader type {:?}", field_id, instance_ty);
                self.void_nop(vm);
            }
        }
    }

    fn handle_scope_object_field(&mut self, vm: &mut ScriptVm, output: &mut ShaderOutput, obj: ScriptObject, field_id: LiveId, field_e: ExprId) {
        let value = vm.bx.heap.value(obj, field_id.into(), NoTrap);
        if !value.is_nil() && !value.is_err() && self.trap.err_is_empty() {
            if let Some(value_obj) = value.as_object() {
                if vm.bx.heap.as_shader_io(value_obj).is_some() {
                    script_err_shader!(self.trap, "shader_io not supported on scope objects");
                    return self.void_nop(vm);
                }
                if vm.bx.heap.as_fn(value_obj).is_some() {
                    self.push(ShaderType::ScopeObject(obj), NO_EXPR);
                    self.push(ShaderType::Id(field_id), field_e);
                    return;
                }
                let enum_value = vm.bx.heap.value(value_obj, id!(_repr_u32_enum_value).into(), NoTrap);
                if !enum_value.is_nil() {
                    self.trap.err_take();
                    if let Some(f) = enum_value.as_f64() {
                        let e = self.expr(ExprKind::Lit(IrLit::U32(f as u32)), TY_U32);
                        return self.push(ShaderType::Pod(vm.bx.code.builtins.pod.pod_u32), e);
                    }
                }
                self.trap.err_take();
                self.push(ShaderType::ScopeObject(value_obj), NO_EXPR);
                return;
            }
            if let Some(pod_ty) = get_scope_value_pod_type(vm, value) {
                let shader_name = self.register_scope_uniform(vm, output, obj, field_id, pod_ty);
                let e = self.scope_uniform_expr(vm, output, shader_name, pod_ty);
                return self.push(ShaderType::Pod(pod_ty), e);
            }
        }
        self.trap.err_take();
        if let Some(field_type_id) = vm.bx.heap.field_type_from_type_check(obj, field_id) {
            if let Some(pod_ty) = vm.bx.heap.type_id_to_pod_type(field_type_id, &vm.bx.code.builtins.pod) {
                let shader_name = self.register_scope_uniform(vm, output, obj, field_id, pod_ty);
                let e = self.scope_uniform_expr(vm, output, shader_name, pod_ty);
                return self.push(ShaderType::Pod(pod_ty), e);
            }
        }
        script_err_not_found!(
            self.trap,
            "field {:?} not found on ScopeObject{}",
            field_id,
            suggest_property(&vm.bx.heap, obj, field_id.into())
        );
        self.void_nop(vm);
    }

    fn handle_io_self_field(&mut self, vm: &mut ScriptVm, output: &mut ShaderOutput, obj: ScriptObject, field_id: LiveId) {
        let (value, maybe_io_type) = ShaderFnCompiler::get_io_self_field_value(vm, obj, field_id, self.trap.pass());
        if let Some(io_type) = maybe_io_type {
            let value_obj = value.as_object().unwrap();
            let proto = vm.bx.heap.proto(value_obj);
            let ty = ShaderFnCompiler::type_from_value(vm, proto);
            let concrete = match ty {
                ShaderType::Pod(pt) | ShaderType::PodType(pt) => Some(pt),
                _ => None,
            };
            let kind = match io_kind_for(output.mode, io_type) {
                Ok(k) => k,
                Err(e) => {
                    output.push_error(format!("shader: {}", e));
                    script_err_shader!(self.trap, "shader: {}", e);
                    return self.void_nop(vm);
                }
            };
            if let ShaderIoKind::Texture(tex_type) = kind {
                let mut exists = false;
                for io in &output.io {
                    if io.name == field_id {
                        exists = true;
                    }
                }
                if !exists {
                    output.io.push(ShaderIo { kind: kind.clone(), name: field_id, ty: ScriptPodType::VOID, buffer_index: None });
                }
                let e = self.texture_expr(output, field_id, tex_type);
                return self.push(ShaderType::Texture(tex_type), e);
            }
            if let Some(pod_ty) = concrete {
                return self.push_io_field(vm, output, field_id, kind, pod_ty);
            }
        }
        if let Some((io_type, pod_ty)) = Self::infer_unmarked(vm, value) {
            let kind = match io_kind_for(output.mode, io_type) {
                Ok(k) => k,
                Err(e) => {
                    output.push_error(format!("shader: {}", e));
                    return self.void_nop(vm);
                }
            };
            return self.push_io_field(vm, output, field_id, kind, pod_ty);
        }
        self.trap.err_take();
        let mut rust_ty = None;
        for io in &output.io {
            if io.name == field_id && matches!(io.kind, ShaderIoKind::RustInstance) {
                rust_ty = Some(io.ty);
            }
        }
        if let Some(pod_ty) = rust_ty {
            let e = self.io_global_expr(vm, output, field_id, pod_ty);
            return self.push(ShaderType::Pod(pod_ty), e);
        }
        script_err_shader!(
            self.trap,
            "shader field `{}` needs explicit IO marker (uniform/instance/varying) or implicit scalar/vec/mat type",
            field_id
        );
        self.void_nop(vm);
    }

    pub(crate) fn handle_let_dyn(&mut self, vm: &mut ScriptVm, output: &mut ShaderOutput, opargs: crate::opcode::OpcodeArgs, is_var: bool) {
        if opargs.is_nil() {
            script_err_not_allowed!(self.trap, "shader let requires initializer");
            self.pop();
            return;
        }
        let (ty_value, value) = self.pop_resolved(vm, output);
        let (ty_id, _) = self.pop();
        let ShaderType::Id(id) = ty_id else {
            script_err_unexpected!(self.trap, "let requires Id");
            return;
        };
        let Some(ty) = ty_value.make_concrete(&vm.bx.code.builtins.pod) else {
            script_err_shader!(self.trap, "cannot determine shader type for let");
            return;
        };
        let value = self.coerce(vm, output, value, ty);
        let ir_ty = Self::ir_ty(vm, output, ty);
        let local = self.f.local(id, 0, ir_ty, is_var, LocalKind::User);
        self.scope.define(id, IrVar::Local(local, ty, is_var));
        self.emit(Stmt::Local(local, value));
    }
}

fn logical_fetch_pod_type(vm: &ScriptVm, pod_ty: ScriptPodType) -> ScriptPodType {
    match vm.bx.heap.pod_type_ref(pod_ty).ty {
        ScriptPodTy::Packed(packed) if packed.is_vec4() => vm.bx.code.builtins.pod.pod_vec4f,
        ScriptPodTy::Packed(_) => vm.bx.code.builtins.pod.pod_vec2f,
        _ => pod_ty,
    }
}

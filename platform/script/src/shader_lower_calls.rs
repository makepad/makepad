//! Lowering of calls: script functions (compiled once per argument types),
//! pod constructors, arrays, the builtins and the texture methods
//! (shader_calls.rs's rules).

use crate::function::*;
use crate::opcode::*;
use crate::pod::*;
use crate::shader::{ShaderMode, ShaderType};
use crate::shader_builtins::type_table_builtin;
use crate::shader_ir::*;
use crate::shader_lower::*;
use crate::shader_output::*;
use crate::suggest::*;
use crate::trap::*;
use crate::value::*;
use crate::vm::*;
use crate::*;
use makepad_live_id::*;

impl IrFnCompiler {
    pub(crate) fn handle_pod_type_call(&mut self, vm: &mut ScriptVm, output: &mut ShaderOutput, opargs: OpcodeArgs, pod_ty: ScriptPodType, name: LiveId) {
        if let ScriptPodTy::ArrayBuilder = &vm.bx.heap.pod_types[pod_ty.index as usize].ty {
            self.mes.push(IrMe::ArrayConstruct { args: Vec::new(), elem_ty: None });
            self.maybe_pop_to_me(vm, output, opargs);
            return;
        }
        self.ensure_struct_name(vm, output, pod_ty, name);
        self.mes.push(IrMe::Pod { pod_ty, args: Vec::new() });
        self.maybe_pop_to_me(vm, output, opargs);
    }

    pub(crate) fn handle_call_args(&mut self, vm: &mut ScriptVm, output: &mut ShaderOutput, opargs: OpcodeArgs) {
        let (ty, _) = self.pop();
        if let ShaderType::Id(name) = ty {
            if let Some(IrVar::PodType(pod_ty)) = self.scope.find(name) {
                self.handle_pod_type_call(vm, output, opargs, pod_ty, name);
                return;
            }
            let value = vm.bx.heap.scope_value(self.script_scope, name.into(), self.trap.pass());
            if let Some(pod_ty) = vm.bx.heap.pod_type(value) {
                self.handle_pod_type_call(vm, output, opargs, pod_ty, name);
                return;
            }
            if let Some(fnobj) = value.as_object() {
                if let Some(fnptr) = vm.bx.heap.as_fn(fnobj) {
                    match fnptr {
                        ScriptFnPtr::Script(_) => {
                            self.mes.push(IrMe::ScriptCall {
                                name,
                                fnobj,
                                sself: ShaderType::None,
                                self_arg: NO_EXPR,
                                args: Vec::new(),
                            });
                        }
                        ScriptFnPtr::Native(_) => {
                            let name = if name == id!(ln) { id!(log) } else { name };
                            self.mes.push(IrMe::BuiltinCall { name, args: Vec::new() });
                        }
                    }
                    self.maybe_pop_to_me(vm, output, opargs);
                    return;
                }
            }
        }
        script_err_wrong_value!(self.trap, "shader call target is not a function");
    }

    pub(crate) fn handle_call_exec(&mut self, vm: &mut ScriptVm, output: &mut ShaderOutput) {
        let is_call_me = matches!(
            self.mes.last(),
            Some(IrMe::ArrayConstruct { .. })
                | Some(IrMe::Pod { .. })
                | Some(IrMe::ScriptCall { .. })
                | Some(IrMe::TextureBuiltin { .. })
                | Some(IrMe::BuiltinCall { .. })
                | Some(IrMe::PodBuiltinMethod { .. })
        );
        if !is_call_me {
            self.push(ShaderType::Error(NIL), NO_EXPR);
            return;
        }
        match self.mes.pop() {
            Some(IrMe::ArrayConstruct { args, elem_ty }) => self.handle_array_construct(vm, output, args, elem_ty),
            Some(IrMe::Pod { pod_ty, args }) => self.handle_pod_construct(vm, output, pod_ty, args),
            Some(IrMe::ScriptCall { name, fnobj, sself, self_arg, args }) => self.handle_script_call(vm, output, name, fnobj, sself, self_arg, args),
            Some(IrMe::TextureBuiltin { method_id, tex_type, tex, args }) => self.handle_texture_builtin(vm, output, method_id, tex_type, tex, args),
            Some(IrMe::BuiltinCall { name, args }) | Some(IrMe::PodBuiltinMethod { name, args }) => self.handle_builtin_call(vm, output, name, args),
            _ => {
                script_err_not_impl!(self.trap, "CALL_EXEC: unexpected call type in shader (internal error)");
            }
        }
    }

    fn handle_array_construct(&mut self, vm: &mut ScriptVm, output: &mut ShaderOutput, args: Vec<(ShaderType, ExprId)>, elem_ty: Option<ScriptPodType>) {
        let elem_ty = elem_ty.unwrap_or(vm.bx.code.builtins.pod.pod_f32);
        let count = args.len();
        let elem_data = vm.bx.heap.pod_types[elem_ty.index as usize].clone();
        let elem_inline = ScriptPodTypeInline { self_ref: elem_ty, data: elem_data };
        let align_of = elem_inline.data.ty.align_of();
        let raw_size = elem_inline.data.ty.size_of();
        let stride = if raw_size % align_of != 0 { raw_size + (align_of - (raw_size % align_of)) } else { raw_size };
        let array_ty = vm.bx.heap.new_pod_array_type(
            ScriptPodTy::FixedArray {
                align_of,
                size_of: stride * count,
                len: count,
                ty: Box::new(elem_inline),
            },
            NIL,
        );
        if vm.bx.heap.pod_type_name(elem_ty).is_none() {
            script_err_shader!(self.trap, "no shader type for array element");
        } else if matches!(vm.bx.heap.pod_types[elem_ty.index as usize].ty, ScriptPodTy::Struct { .. }) {
            output.structs.insert(elem_ty);
        }
        let mut parts = Vec::new();
        for (_, e) in args {
            parts.push(self.coerce(vm, output, e, elem_ty));
        }
        let ty = Self::ir_ty(vm, output, array_ty);
        let e = self.expr(ExprKind::Construct(parts), ty);
        self.push(ShaderType::Pod(array_ty), e);
    }

    fn handle_pod_construct(&mut self, vm: &mut ScriptVm, output: &mut ShaderOutput, pod_ty: ScriptPodType, args: Vec<IrPodArg>) {
        let mut offset = ScriptPodOffset::default();
        let pod_ty_data = vm.bx.heap.pod_types[pod_ty.index as usize].clone();
        if vm.bx.heap.pod_type_name(pod_ty).is_none() {
            script_err_shader!(self.trap, "no shader type for pod construct");
        }
        let ir_ty = Self::ir_ty(vm, output, pod_ty);
        let mut parts = Vec::new();
        if let Some(first) = args.first() {
            if first.name.is_some() {
                if let ScriptPodTy::Struct { fields, .. } = &pod_ty_data.ty {
                    for field in fields {
                        let mut found = None;
                        for a in &args {
                            if a.name == Some(field.name) {
                                found = Some(a);
                            }
                        }
                        let Some(arg) = found else {
                            script_err_type_mismatch!(self.trap, "missing arg for field {:?}", field.name);
                            continue;
                        };
                        let b = &vm.bx.code.builtins.pod;
                        match &arg.ty {
                            ShaderType::Pod(t) => {
                                if *t != field.ty.self_ref {
                                    script_err_pod!(
                                        self.trap,
                                        "named arg {:?} type mismatch: expected {}, got {}",
                                        field.name,
                                        format_pod_type_name(&vm.bx.heap, field.ty.self_ref),
                                        format_pod_type_name(&vm.bx.heap, *t)
                                    );
                                }
                            }
                            ShaderType::AbstractInt => {
                                let f = field.ty.self_ref;
                                if f != b.pod_i32 && f != b.pod_u32 && f != b.pod_f32 {
                                    script_err_pod!(self.trap, "abstract int not compatible with field {:?} (expects {})", field.name, format_pod_type_name(&vm.bx.heap, f));
                                }
                            }
                            ShaderType::AbstractFloat => {
                                let f = field.ty.self_ref;
                                if f != b.pod_f32 {
                                    script_err_pod!(self.trap, "abstract float not compatible with field {:?} (expects {})", field.name, format_pod_type_name(&vm.bx.heap, f));
                                }
                            }
                            _ => {}
                        }
                        let e = arg.e;
                        parts.push(self.coerce(vm, output, e, field.ty.self_ref));
                    }
                    if args.len() != fields.len() {
                        script_err_invalid_args!(self.trap, "expected {} args, got {}", fields.len(), args.len());
                    }
                } else {
                    script_err_unexpected!(self.trap, "named args require struct type");
                }
            } else {
                for arg in &args {
                    match &arg.ty {
                        ShaderType::Pod(t) | ShaderType::PodPtr(t) => {
                            vm.bx.heap.pod_check_constructor_arg(pod_ty, *t, &mut offset, self.trap.pass());
                        }
                        ShaderType::AbstractInt | ShaderType::AbstractFloat => {
                            vm.bx.heap.pod_check_abstract_constructor_arg(pod_ty, &mut offset, self.trap.pass());
                        }
                        _ => {}
                    }
                }
                vm.bx.heap.pod_check_constructor_arg_count(pod_ty, &offset, self.trap.pass());
                match &pod_ty_data.ty {
                    ScriptPodTy::Struct { fields, .. } => {
                        for (i, arg) in args.iter().enumerate() {
                            let e = arg.e;
                            match fields.get(i) {
                                Some(f) => {
                                    let f = f.ty.self_ref;
                                    parts.push(self.coerce(vm, output, e, f));
                                }
                                None => parts.push(e),
                            }
                        }
                    }
                    ScriptPodTy::F32 | ScriptPodTy::F16 | ScriptPodTy::U32 | ScriptPodTy::I32 | ScriptPodTy::Bool => {
                        // A scalar constructor is a conversion.
                        if let Some(arg) = args.first() {
                            let e = arg.e;
                            let et = if e == NO_EXPR { TY_VOID } else { self.f.exprs[e as usize].ty };
                            if et == TY_AINT || et == TY_AFLOAT {
                                let s = output.ir.scalar_of(ir_ty).unwrap_or(IrScalar::F32);
                                self.concretize(e, s);
                                return self.push(ShaderType::Pod(pod_ty), e);
                            }
                            let c = if et == ir_ty { e } else { self.expr(ExprKind::Convert(e), ir_ty) };
                            return self.push(ShaderType::Pod(pod_ty), c);
                        }
                    }
                    _ => {
                        // Vector and matrix components take the element scalar.
                        let s = output.ir.scalar_of(ir_ty).unwrap_or(IrScalar::F32);
                        for arg in &args {
                            let e = arg.e;
                            if e == NO_EXPR {
                                continue;
                            }
                            let et = self.f.exprs[e as usize].ty;
                            if et == TY_AINT || et == TY_AFLOAT {
                                self.concretize(e, s);
                                parts.push(e);
                                continue;
                            }
                            // A component of another scalar kind converts.
                            match output.ir.scalar_of(et) {
                                Some(es) if es != s && matches!(output.ir.get(et), IrTy::Scalar(_) | IrTy::Vec(..)) => {
                                    let ct = output.ir.with_scalar(et, s);
                                    parts.push(self.expr(ExprKind::Convert(e), ct));
                                }
                                _ => parts.push(e),
                            }
                        }
                    }
                }
            }
        } else {
            vm.bx.heap.pod_check_constructor_arg_count(pod_ty, &offset, self.trap.pass());
        }
        let e = self.expr(ExprKind::Construct(parts), ir_ty);
        self.push(ShaderType::Pod(pod_ty), e);
    }

    /// Compiles a script function for the given argument types (once per
    /// types: the call graph is a table of functions).
    pub fn compile_shader_def(
        vm: &mut ScriptVm,
        output: &mut ShaderOutput,
        trap: ScriptTrap,
        name: LiveId,
        fnobj: ScriptObject,
        sself: ShaderType,
        args: Vec<ShaderType>,
    ) -> (ScriptPodType, Option<FuncId>) {
        let mut prefix = String::new();
        match &sself {
            ShaderType::PodType(ty) | ShaderType::Pod(ty) => {
                if let Some(n) = vm.bx.heap.pod_type_name(*ty) {
                    prefix = format!("{}_", n);
                }
            }
            ShaderType::IoSelf(_) => prefix.push_str("io_"),
            ShaderType::ScopeObject(obj) => {
                let mut at = None;
                for (i, idx) in output.scope_prefixes.iter().enumerate() {
                    if *idx == obj.index as usize {
                        at = Some(i);
                    }
                }
                let at = match at {
                    Some(a) => a,
                    None => {
                        output.scope_prefixes.push(obj.index as usize);
                        output.scope_prefixes.len() - 1
                    }
                };
                prefix = format!("scope{}_", at);
            }
            _ => {}
        }

        let builtins = vm.bx.code.builtins.pod.clone();
        let argc = vm.bx.heap.vec_len(fnobj);
        let mut resolved: Vec<ScriptPodType> = Vec::new();
        let mut argi = 0;
        let mut expected = 0;
        for i in 0..argc {
            let kv = vm.bx.heap.vec_key_value(fnobj, i, trap);
            if kv.key == id!(self).into() {
                continue;
            }
            expected += 1;
            if argi >= args.len() {
                continue;
            }
            let arg = &args[argi];
            let declared = kv.value.as_pod_type().or_else(|| vm.bx.heap.pod_type(kv.value));
            let r = match arg {
                ShaderType::AbstractInt | ShaderType::AbstractFloat => {
                    declared.unwrap_or_else(|| arg.make_concrete(&builtins).unwrap_or(builtins.pod_void))
                }
                _ => arg.make_concrete(&builtins).unwrap_or(builtins.pod_void),
            };
            resolved.push(r);
            argi += 1;
        }
        if args.len() != expected {
            output.push_error(format!(
                "shader function {:?} expects {} argument{}, but {} {} provided",
                name,
                expected,
                if expected == 1 { "" } else { "s" },
                args.len(),
                if args.len() == 1 { "was" } else { "were" }
            ));
            script_err_invalid_args!(trap, "function {:?} expects {} arguments, but {} were provided", name, expected, args.len());
        }

        for (i, f) in output.ir.functions.iter().enumerate() {
            if f.fnobj == fnobj && f.arg_pods == resolved && f.entry.is_none() {
                output.last_call_cost = f.cost;
                return (lookup_ret(vm, f), Some(i as FuncId));
            }
        }

        let mut overload = 0;
        for f in &output.ir.functions {
            if f.key_name == name {
                overload += 1;
            }
        }
        let fn_name = if overload != 0 {
            format!("_f{}{}{}", overload, prefix, name)
        } else {
            format!("{}{}", prefix, name)
        };

        let mut c = IrFnCompiler::new(fnobj);
        c.f.name = fn_name;
        c.f.key_name = name;
        c.f.overload = overload;
        c.f.fnobj = fnobj;
        c.f.arg_pods = resolved.clone();

        let mut has_self = false;
        match sself {
            ShaderType::Pod(ty) => {
                has_self = true;
                let t = output.ir.pod_ty(&vm.bx.heap, ty);
                c.f.params.push(IrParam { name: id!(self), shadow: 0, ty: t, inout: true });
                c.scope.define(id!(self), IrVar::Param(0, ty));
            }
            ShaderType::PodType(ty) => c.scope.define(id!(self), IrVar::PodType(ty)),
            ShaderType::IoSelf(obj) => c.scope.define(id!(self), IrVar::IoSelf(obj)),
            ShaderType::ScopeObject(obj) => c.scope.define(id!(self), IrVar::ScopeObject(obj)),
            _ => {}
        }

        let mut argi = 0;
        for i in 0..argc {
            let kv = vm.bx.heap.vec_key_value(fnobj, i, trap);
            if kv.key == id!(self).into() {
                if !has_self || argi != 0 {
                    output.push_error(format!("shader function {:?}: self arg must be first with has_self", name));
                    script_err_not_found!(trap, "self arg must be first with has_self");
                }
                continue;
            }
            if let Some(id) = kv.key.as_id() {
                if argi >= resolved.len() {
                    output.push_error(format!("shader function {:?}: more formal params than resolved args", name));
                    script_err_invalid_args!(trap, "more formal params than resolved args");
                    break;
                }
                let arg_ty = resolved[argi];
                let t = output.ir.pod_ty(&vm.bx.heap, arg_ty);
                let index = c.f.params.len() as u32;
                c.f.params.push(IrParam { name: id, shadow: 0, ty: t, inout: false });
                c.scope.define(id, IrVar::Param(index, arg_ty));
            }
            argi += 1;
        }

        let Some(ScriptFnPtr::Script(fnip)) = vm.bx.heap.as_fn(fnobj) else {
            output.push_error(format!("shader function {:?} is not a script function", name));
            return (builtins.pod_void, None);
        };
        if output.recur_block.contains(&fnobj) {
            output.push_error(format!("shader function {:?}: shader functions cannot recurse", name));
            script_err_not_allowed!(trap, "shader functions cannot recurse");
            return (builtins.pod_void, None);
        }
        output.recur_block.push(fnobj);
        let ret = c.compile_fn(vm, output, fnip);
        output.recur_block.pop();
        let cost = c.static_cost();
        if let ScriptPodTy::Struct { .. } = vm.bx.heap.pod_type_ref(ret).ty {
            output.structs.insert(ret);
        }
        c.f.ret = output.ir.pod_ty(&vm.bx.heap, ret);
        c.f.ret_pod = ret;
        c.f.cost = cost;
        let body = c.blocks.pop().unwrap_or_default();
        c.f.body = body;
        finish_function(&mut c.f);
        output.last_call_cost = cost;
        output.ir.functions.push(c.f);
        (ret, Some((output.ir.functions.len() - 1) as FuncId))
    }

    fn handle_script_call(
        &mut self,
        vm: &mut ScriptVm,
        output: &mut ShaderOutput,
        name: LiveId,
        fnobj: ScriptObject,
        sself: ShaderType,
        self_arg: ExprId,
        args: Vec<(ShaderType, ExprId)>,
    ) {
        let mut arg_types = Vec::new();
        for (t, _) in &args {
            arg_types.push(t.clone());
        }
        output.last_call_cost = 0;
        let (ret, func) = Self::compile_shader_def(vm, output, self.trap.pass(), name, fnobj, sself, arg_types);
        self.charge_loop_cost(output.last_call_cost);
        let Some(func) = func else {
            return self.push(ShaderType::Pod(ret), NO_EXPR);
        };
        let mut call_args = Vec::new();
        let params = output.ir.functions[func as usize].params.clone();
        let arg_pods = output.ir.functions[func as usize].arg_pods.clone();
        let mut pi = 0;
        if self_arg != NO_EXPR {
            call_args.push(self_arg);
            pi = 1;
        }
        let mut ai = 0;
        for (_, e) in args {
            let target = arg_pods.get(ai).copied();
            let e = match target {
                Some(t) => self.coerce(vm, output, e, t),
                None => e,
            };
            call_args.push(e);
            ai += 1;
            pi += 1;
        }
        let _ = (params, pi);
        let rt = Self::ir_ty(vm, output, ret);
        let e = self.expr(ExprKind::Call(func, call_args), rt);
        self.push(ShaderType::Pod(ret), e);
    }

    /// A pod method's receiver as a reference: the `self` param itself, a
    /// place's address, or a temporary holding a value.
    fn receiver_ref(&mut self, vm: &ScriptVm, output: &mut ShaderOutput, e: ExprId, pod_ty: ScriptPodType) -> ExprId {
        if e == NO_EXPR {
            return e;
        }
        let ty = Self::ir_ty(vm, output, pod_ty);
        if let ExprKind::Deref(p) = self.f.exprs[e as usize].kind {
            return p;
        }
        if !is_place(&self.f, e) {
            let tmp = self.f.local(id!(_mp_tmp), 0, ty, true, LocalKind::Temp);
            self.emit(Stmt::Local(tmp, e));
            let l = self.expr(ExprKind::Local(tmp), ty);
            return self.expr(ExprKind::AddrOf(l), ty);
        }
        self.expr(ExprKind::AddrOf(e), ty)
    }

    fn handle_builtin_call(&mut self, vm: &mut ScriptVm, output: &mut ShaderOutput, name: LiveId, args: Vec<(ShaderType, ExprId)>) {
        let builtins = vm.bx.code.builtins.pod.clone();
        if name == id!(instance_index) {
            if output.mode != ShaderMode::Vertex || !args.is_empty() {
                script_err_not_impl!(self.trap, "instance_index() requires vertex stage and no arguments");
            }
            let e = self.expr(ExprKind::InstanceIndex, TY_U32);
            return self.push(ShaderType::Pod(builtins.pod_u32), e);
        }
        if name == id!(discard) {
            self.emit(Stmt::Discard);
            let n = self.nop();
            return self.push(ShaderType::Pod(builtins.pod_void), n);
        }
        let mut concrete = Vec::new();
        let has_float = {
            let mut f = false;
            for (ty, _) in &args {
                match ty {
                    ShaderType::Pod(pt) => {
                        if vm.bx.heap.pod_types[pt.index as usize].ty.is_float_type() {
                            f = true;
                        }
                    }
                    ShaderType::AbstractFloat => f = true,
                    _ => {}
                }
            }
            f
        };
        let mut exprs = Vec::new();
        for (ty, e) in &args {
            match ty {
                ShaderType::AbstractInt | ShaderType::AbstractFloat => {
                    if has_float {
                        concrete.push(builtins.pod_f32);
                        self.concretize(*e, IrScalar::F32);
                    } else {
                        concrete.push(ty.make_concrete(&builtins).unwrap_or(builtins.pod_void));
                        self.concretize_default(*e);
                    }
                }
                ShaderType::Pod(pt) => concrete.push(*pt),
                _ => concrete.push(ty.make_concrete(&builtins).unwrap_or(builtins.pod_void)),
            }
            exprs.push(*e);
        }
        if name == id!(depth_clip) {
            let ret = type_table_builtin(name, &concrete, &builtins, self.trap.pass());
            if exprs.len() == 3 {
                let rt = Self::ir_ty(vm, output, ret);
                let e = self.expr(ExprKind::DepthClip(exprs[0], exprs[1], exprs[2]), rt);
                return self.push(ShaderType::Pod(ret), e);
            }
            return self.push(ShaderType::Pod(ret), NO_EXPR);
        }
        if name == id!(asuint) || name == id!(asint) || name == id!(asfloat) {
            let ret = type_table_builtin(name, &concrete, &builtins, self.trap.pass());
            let arg_ty = concrete.first().copied().unwrap_or(builtins.pod_void);
            let arg = exprs.first().copied().unwrap_or(NO_EXPR);
            let rt = Self::ir_ty(vm, output, ret);
            let same = arg_ty == ret || (name == id!(asfloat) && (arg_ty == builtins.pod_f32 || arg_ty == builtins.pod_f16));
            let e = if same || arg == NO_EXPR {
                arg
            } else {
                self.expr(ExprKind::Bitcast(arg), rt)
            };
            return self.push(ShaderType::Pod(ret), e);
        }
        if name == id!(dFdx) || name == id!(dFdy) {
            output.uses_derivatives = true;
        }
        let ret = type_table_builtin(name, &concrete, &builtins, self.trap.pass());
        let Some(op) = Builtin::from_name(name) else {
            return self.push(ShaderType::Pod(ret), NO_EXPR);
        };
        let rt = Self::ir_ty(vm, output, ret);
        let e = self.expr(ExprKind::Builtin(op, exprs), rt);
        self.push(ShaderType::Pod(ret), e);
    }

    fn handle_texture_builtin(&mut self, vm: &mut ScriptVm, output: &mut ShaderOutput, method_id: LiveId, tex_type: TextureType, tex: ExprId, args: Vec<(ShaderType, ExprId)>) {
        let pods = vm.bx.code.builtins.pod.clone();
        let mut exprs = Vec::new();
        for (_, e) in &args {
            exprs.push(*e);
        }
        let vec4 = output.ir.vec_ty(4, IrScalar::F32);
        match method_id {
            id!(sample_compare) => {
                if tex_type != TextureType::TextureDepth || exprs.len() != 2 {
                    script_err_invalid_args!(self.trap, "texture_depth.sample_compare requires (uv, reference_depth)");
                    let e = self.expr(ExprKind::Lit(IrLit::F32(0.0)), TY_F32);
                    return self.push(ShaderType::Pod(pods.pod_f32), e);
                }
                let sampler = output.get_or_create_sampler(ShaderSampler { compare: true, ..ShaderSampler::default() });
                let coord = self.settle_float(output, exprs[0], 2);
                let reference = self.settle_float(output, exprs[1], 1);
                let s = IrSample {
                    tex,
                    tex_type,
                    sampler: sampler as u32,
                    kind: SampleKind::CompareLevel0,
                    method: method_id,
                    coord,
                    lod: NO_EXPR,
                    dx: NO_EXPR,
                    dy: NO_EXPR,
                    reference,
                };
                let e = self.expr(ExprKind::Sample(Box::new(s)), TY_F32);
                self.push(ShaderType::Pod(pods.pod_f32), e);
            }
            id!(size) => {
                let v2 = output.ir.vec_ty(2, IrScalar::F32);
                let e = self.expr(ExprKind::TexSize(tex), v2);
                self.push(ShaderType::Pod(pods.pod_vec2f), e);
            }
            id!(sample) | id!(sample_lod) | id!(sample_nearest) | id!(sample_repeat) => {
                let args_ok = if method_id == id!(sample_lod) {
                    exprs.len() == 2
                } else if method_id == id!(sample_nearest) {
                    exprs.len() == 1 || exprs.len() == 2
                } else {
                    exprs.len() == 1
                };
                if !args_ok {
                    script_err_invalid_args!(self.trap, "texture.{:?} has the wrong number of arguments", method_id);
                    return self.push(ShaderType::Pod(pods.pod_vec4f), NO_EXPR);
                }
                let sampler = if method_id == id!(sample_nearest) {
                    ShaderSampler { filter: SamplerFilter::Nearest, ..ShaderSampler::default() }
                } else if method_id == id!(sample_repeat) {
                    ShaderSampler { address: SamplerAddress::Repeat, ..ShaderSampler::default() }
                } else {
                    ShaderSampler::default()
                };
                let sampler = output.get_or_create_sampler(sampler);
                let lanes = coord_lanes(tex_type);
                let coord = self.settle_float(output, exprs[0], lanes);
                let (kind, lod) = match exprs.get(1) {
                    Some(l) => (SampleKind::Lod, self.settle_float(output, *l, 1)),
                    None => (SampleKind::Implicit, NO_EXPR),
                };
                let s = IrSample {
                    tex,
                    tex_type,
                    sampler: sampler as u32,
                    kind,
                    method: method_id,
                    coord,
                    lod,
                    dx: NO_EXPR,
                    dy: NO_EXPR,
                    reference: NO_EXPR,
                };
                let e = self.expr(ExprKind::Sample(Box::new(s)), vec4);
                self.push(ShaderType::Pod(pods.pod_vec4f), e);
            }
            id!(sample_grad) => {
                if exprs.len() != 3 || !matches!(tex_type, TextureType::Texture2d) {
                    script_err_invalid_args!(self.trap, "texture.sample_grad(coord, dx, dy) takes 3 args, on a 2D texture");
                    return self.push(ShaderType::Pod(pods.pod_vec4f), NO_EXPR);
                }
                let sampler = output.get_or_create_sampler(ShaderSampler::default());
                let coord = self.settle_float(output, exprs[0], 2);
                let dx = self.settle_float(output, exprs[1], 2);
                let dy = self.settle_float(output, exprs[2], 2);
                let s = IrSample {
                    tex,
                    tex_type,
                    sampler: sampler as u32,
                    kind: SampleKind::Grad,
                    method: method_id,
                    coord,
                    lod: NO_EXPR,
                    dx,
                    dy,
                    reference: NO_EXPR,
                };
                let e = self.expr(ExprKind::Sample(Box::new(s)), vec4);
                self.push(ShaderType::Pod(pods.pod_vec4f), e);
            }
            id!(sample_video) => {
                if exprs.len() != 1 {
                    script_err_invalid_args!(self.trap, "texture.sample_video requires 1 arg");
                    return self.push(ShaderType::Pod(pods.pod_vec4f), NO_EXPR);
                }
                let sampler = output.get_or_create_sampler(ShaderSampler::default());
                let coord = self.settle_float(output, exprs[0], 2);
                let s = IrSample {
                    tex,
                    tex_type,
                    sampler: sampler as u32,
                    kind: SampleKind::Video,
                    method: method_id,
                    coord,
                    lod: NO_EXPR,
                    dx: NO_EXPR,
                    dy: NO_EXPR,
                    reference: NO_EXPR,
                };
                let e = self.expr(ExprKind::Sample(Box::new(s)), vec4);
                self.push(ShaderType::Pod(pods.pod_vec4f), e);
            }
            _ => {
                script_err_not_found!(
                    self.trap,
                    "unknown texture method {:?}{}",
                    method_id,
                    suggest_from_live_ids(
                        method_id,
                        &[id!(sample), id!(sample_nearest), id!(sample_repeat), id!(sample_lod), id!(sample_grad), id!(sample_video), id!(size)]
                    )
                );
            }
        }
    }

    /// A float coordinate / lod: abstract literals become f32 (splatted to
    /// `lanes`).
    fn settle_float(&mut self, output: &mut ShaderOutput, e: ExprId, lanes: u8) -> ExprId {
        if e == NO_EXPR {
            return e;
        }
        let et = self.f.exprs[e as usize].ty;
        if et == TY_AINT || et == TY_AFLOAT {
            self.concretize(e, IrScalar::F32);
            if lanes > 1 {
                let t = output.ir.vec_ty(lanes, IrScalar::F32);
                return self.expr(ExprKind::Construct(vec![e]), t);
            }
        }
        e
    }

    pub(crate) fn handle_method_call_args(&mut self, vm: &mut ScriptVm, output: &mut ShaderOutput, opargs: OpcodeArgs) {
        let (method_ty, _) = self.pop();
        let (self_ty, self_e) = self.pop();
        let ShaderType::Id(method_id) = method_ty else {
            script_err_not_impl!(self.trap, "METHOD_CALL_ARGS: method call syntax not valid here");
            return;
        };
        match self_ty {
            ShaderType::Texture(tex_type) | ShaderType::ScopeTexture { tex_type, .. } => {
                self.mes.push(IrMe::TextureBuiltin { method_id, tex_type, tex: self_e, args: Vec::new() });
            }
            ShaderType::Id(self_id) => {
                if let Some(var) = self.scope.find(self_id) {
                    if let IrVar::IoSelf(obj) = var {
                        if self.handle_io_self_method(vm, output, opargs, method_id, obj) {
                            return;
                        }
                    }
                    if let IrVar::ScopeObject(obj) = var {
                        if self.handle_scope_object_method(vm, output, opargs, method_id, obj) {
                            return;
                        }
                    }
                    if let IrVar::PodType(pod_ty) = var {
                        if self.handle_pod_type_static_method(vm, output, opargs, method_id, pod_ty, self_id) {
                            return;
                        }
                    }
                    if matches!(var, IrVar::Param(..) | IrVar::Local(..)) {
                        let pod_ty = var.pod();
                        let recv = if self_e != NO_EXPR { self_e } else { self.var_expr(vm, output, var) };
                        if self.handle_pod_method(vm, output, opargs, method_id, pod_ty, recv) {
                            return;
                        }
                    }
                    let type_name = vm
                        .bx
                        .heap
                        .pod_type_name(var.pod())
                        .map(|id| id.as_string(|s| s.unwrap_or("unknown").to_string()))
                        .unwrap_or_else(|| format!("{:?}", self_id));
                    script_err_not_found!(self.trap, "method {:?} not found on {}", method_id, type_name);
                    return;
                }
                let value = vm.bx.heap.scope_value(self.script_scope, self_id.into(), NoTrap);
                if let Some(pod_ty) = vm.bx.heap.pod_type(value) {
                    if self.handle_pod_type_static_method(vm, output, opargs, method_id, pod_ty, self_id) {
                        return;
                    }
                }
                if let Some(obj) = value.as_object() {
                    if let Some(io_type) = vm.bx.heap.as_shader_io(obj) {
                        if let Some(tex_type) = texture_type_of(io_type) {
                            let shader_name = self.register_scope_texture(output, self_id, obj, tex_type);
                            let tex = self.texture_expr(output, shader_name, tex_type);
                            self.mes.push(IrMe::TextureBuiltin { method_id, tex_type, tex, args: Vec::new() });
                            return;
                        }
                    } else if vm.bx.heap.as_fn(obj).is_none() {
                        if self.handle_scope_object_method(vm, output, opargs, method_id, obj) {
                            return;
                        }
                    }
                }
                script_err_not_found!(self.trap, "method {:?} not found on {:?}", method_id, self_id);
            }
            ShaderType::ScopeObject(obj) => {
                if !self.handle_scope_object_method(vm, output, opargs, method_id, obj) {
                    script_err_not_found!(self.trap, "method {:?} not found on scope object", method_id);
                }
            }
            ShaderType::IoSelf(obj) => {
                if !self.handle_io_self_method(vm, output, opargs, method_id, obj) {
                    script_err_not_found!(self.trap, "method {:?} not found on self", method_id);
                }
            }
            ShaderType::Pod(pod_ty) => {
                if !self.handle_pod_method(vm, output, opargs, method_id, pod_ty, self_e) {
                    let type_name = vm
                        .bx
                        .heap
                        .pod_type_name(pod_ty)
                        .map(|id| id.as_string(|s| s.unwrap_or("unknown").to_string()))
                        .unwrap_or_else(|| "unknown".to_string());
                    script_err_not_found!(self.trap, "method {:?} not found on {}", method_id, type_name);
                }
            }
            other => {
                script_err_not_found!(self.trap, "method {:?} not found on {:?}", method_id, other);
            }
        }
    }

    fn handle_io_self_method(&mut self, vm: &mut ScriptVm, output: &mut ShaderOutput, opargs: OpcodeArgs, method_id: LiveId, obj: ScriptObject) -> bool {
        let fnobj = vm.bx.heap.value(obj, method_id.into(), self.trap.pass());
        if let Some(fnobj) = fnobj.as_object() {
            if let Some(ScriptFnPtr::Script(_)) = vm.bx.heap.as_fn(fnobj) {
                self.mes.push(IrMe::ScriptCall {
                    name: method_id,
                    fnobj,
                    sself: ShaderType::IoSelf(obj),
                    self_arg: NO_EXPR,
                    args: Vec::new(),
                });
                self.maybe_pop_to_me(vm, output, opargs);
                return true;
            }
        }
        false
    }

    fn handle_scope_object_method(&mut self, vm: &mut ScriptVm, output: &mut ShaderOutput, opargs: OpcodeArgs, method_id: LiveId, obj: ScriptObject) -> bool {
        let fnobj = vm.bx.heap.value(obj, method_id.into(), NoTrap);
        if let Some(fnobj) = fnobj.as_object() {
            match vm.bx.heap.as_fn(fnobj) {
                Some(ScriptFnPtr::Script(_)) => {
                    self.mes.push(IrMe::ScriptCall {
                        name: method_id,
                        fnobj,
                        sself: ShaderType::ScopeObject(obj),
                        self_arg: NO_EXPR,
                        args: Vec::new(),
                    });
                    self.maybe_pop_to_me(vm, output, opargs);
                    return true;
                }
                Some(ScriptFnPtr::Native(_)) => {
                    script_err_shader!(self.trap, "native methods not supported on scope objects");
                    return false;
                }
                None => {}
            }
        }
        false
    }

    fn handle_pod_method(&mut self, vm: &mut ScriptVm, output: &mut ShaderOutput, opargs: OpcodeArgs, method_id: LiveId, pod_ty: ScriptPodType, recv: ExprId) -> bool {
        if matches!(method_id, id!(mix) | id!(clamp) | id!(smoothstep) | id!(step) | id!(min) | id!(max)) {
            self.mes.push(IrMe::PodBuiltinMethod {
                name: method_id,
                args: vec![(ShaderType::Pod(pod_ty), recv)],
            });
            self.maybe_pop_to_me(vm, output, opargs);
            return true;
        }
        let object = vm.bx.heap.pod_types[pod_ty.index as usize].object;
        let fnobj = vm.bx.heap.value(object, method_id.into(), NoTrap);
        if let Some(fnobj) = fnobj.as_object() {
            if let Some(fnptr) = vm.bx.heap.as_fn(fnobj) {
                match fnptr {
                    ScriptFnPtr::Script(_) => {
                        let self_arg = self.receiver_ref(vm, output, recv, pod_ty);
                        self.mes.push(IrMe::ScriptCall {
                            name: method_id,
                            fnobj,
                            sself: ShaderType::Pod(pod_ty),
                            self_arg,
                            args: Vec::new(),
                        });
                    }
                    ScriptFnPtr::Native(_) => {
                        self.mes.push(IrMe::BuiltinCall {
                            name: method_id,
                            args: vec![(ShaderType::Pod(pod_ty), recv)],
                        });
                    }
                }
                self.maybe_pop_to_me(vm, output, opargs);
                return true;
            }
        }
        false
    }

    fn handle_pod_type_static_method(&mut self, vm: &mut ScriptVm, output: &mut ShaderOutput, opargs: OpcodeArgs, method_id: LiveId, pod_ty: ScriptPodType, self_id: LiveId) -> bool {
        self.ensure_struct_name(vm, output, pod_ty, self_id);
        let object = vm.bx.heap.pod_types[pod_ty.index as usize].object;
        let fnobj = vm.bx.heap.value(object, method_id.into(), self.trap.pass());
        if let Some(fnobj) = fnobj.as_object() {
            if let Some(fnptr) = vm.bx.heap.as_fn(fnobj) {
                match fnptr {
                    ScriptFnPtr::Script(_) => {
                        self.mes.push(IrMe::ScriptCall {
                            name: method_id,
                            fnobj,
                            sself: ShaderType::PodType(pod_ty),
                            self_arg: NO_EXPR,
                            args: Vec::new(),
                        });
                    }
                    ScriptFnPtr::Native(_) => {
                        self.mes.push(IrMe::BuiltinCall { name: method_id, args: Vec::new() });
                    }
                }
                self.maybe_pop_to_me(vm, output, opargs);
                return true;
            }
        }
        false
    }
}

fn coord_lanes(tex_type: TextureType) -> u8 {
    match tex_type {
        TextureType::Texture2dArray | TextureType::TextureDepthArray | TextureType::Texture1dArray => 3,
        TextureType::Texture3d | TextureType::Texture3dArray | TextureType::TextureCube => 3,
        TextureType::TextureCubeArray => 4,
        _ => 2,
    }
}

fn lookup_ret(vm: &ScriptVm, f: &IrFunction) -> ScriptPodType {
    let _ = vm;
    f.ret_pod
}

/// A place: what an assignment or a reference may name.
pub(crate) fn is_place(f: &IrFunction, e: ExprId) -> bool {
    if e == NO_EXPR {
        return false;
    }
    match &f.exprs[e as usize].kind {
        ExprKind::Local(_) | ExprKind::Global(_) | ExprKind::Deref(_) => true,
        ExprKind::Param(i) => f.params.get(*i as usize).map(|p| !p.inout).unwrap_or(false),
        ExprKind::Field(b, _) | ExprKind::Index(b, _) => is_place(f, *b),
        ExprKind::Swizzle(b, _, n) => *n == 1 && is_place(f, *b),
        _ => false,
    }
}

/// The function's final pass: abstract literals left (a statement value,
/// an unused result) take their defaults.
fn finish_function(f: &mut IrFunction) {
    for e in &mut f.exprs {
        match e.kind {
            ExprKind::Lit(IrLit::AbstractInt(v)) => {
                *e = Expr { kind: ExprKind::Lit(IrLit::I32(v as i32)), ty: TY_I32 };
            }
            ExprKind::Lit(IrLit::AbstractFloat(v)) => {
                *e = Expr { kind: ExprKind::Lit(IrLit::F32(v as f32)), ty: TY_F32 };
            }
            _ => {}
        }
    }
}

//! Shader function and method call operations
//!
//! This module contains handle functions for function calls, method calls,
//! type construction (Pod, Array), and builtin calls.

use std::fmt::Write;
use makepad_live_id::*;
use crate::value::*;
use crate::function::*;
use crate::vm::*;
use crate::opcode::*;
use crate::pod::*;
use crate::shader::*;
use crate::shader_builtins::*;
use crate::shader_backend::*;
use crate::trap::*;
use crate::suggest::*;
use crate::*;

impl ShaderFnCompiler {
    pub(crate) fn handle_pod_type_call(&mut self, vm: &mut ScriptVm, output: &mut ShaderOutput, opargs: OpcodeArgs, pod_ty: ScriptPodType, name: LiveId) {
        if let ScriptPodTy::ArrayBuilder = &vm.heap.pod_types[pod_ty.index as usize].ty {
            self.mes.push(ShaderMe::ArrayConstruct {
                args: Vec::new(),
                elem_ty: None,
            });
            self.maybe_pop_to_me(vm, opargs);
            return;
        }

        // alright lets see what Id we got
        let _name = self.ensure_struct_name(vm, output, pod_ty, name);

        self.mes.push(ShaderMe::Pod {
            pod_ty,
            args: Vec::new(),
        });

        self.maybe_pop_to_me(vm, opargs);
    }

    pub(crate) fn handle_call_args(&mut self, vm: &mut ScriptVm, output: &mut ShaderOutput, opargs: OpcodeArgs) {
        let (ty, _s) = self.stack.pop(self.trap.pass());
        if let ShaderType::Id(name) = ty {
            // Check shader scope for PodType
            if let Some((ShaderScopeItem::PodType { ty, .. }, _)) = self.shader_scope.find_var(name) {
                self.handle_pod_type_call(vm, output, opargs, *ty, name);
                return;
            }

            // alright lets look it up on our script scope
            let value = vm.heap.scope_value(self.script_scope, name.into(), self.trap.pass());
            // lets check if our obj is a PodType
            if let Some(pod_ty) = vm.heap.pod_type(value) {
                self.handle_pod_type_call(vm, output, opargs, pod_ty, name);
                return;
            }

            if let Some(fnobj) = value.as_object() {
                if let Some(fnptr) = vm.heap.as_fn(fnobj) {
                    match fnptr {
                        // another script fn
                        ScriptFnPtr::Script(_fnptr) => {
                            let mut out = self.stack.new_string();
                            write!(out, "{}", output.backend.get_io_all(output.mode)).ok();
                            self.mes.push(ShaderMe::ScriptCall {
                                name,
                                out,
                                fnobj,
                                sself: ShaderType::None,
                                args: Default::default(),
                            });
                        }
                        // builtin shader fns
                        ScriptFnPtr::Native(fnptr) => {
                            self.mes.push(ShaderMe::BuiltinCall {
                                name,
                                fnptr,
                                args: Default::default(),
                            });
                            self.maybe_pop_to_me(vm, opargs);
                            return;
                        }
                    }

                    self.maybe_pop_to_me(vm, opargs);
                    return;
                }
            }
        }
        script_err_not_fn!(self.trap, "shader call target is not a function");
    }

    pub(crate) fn handle_array_construct(&mut self, vm: &mut ScriptVm, output: &mut ShaderOutput, args: Vec<String>, elem_ty: Option<ScriptPodType>) {
        let elem_ty = elem_ty.unwrap_or(vm.code.builtins.pod.pod_f32);
        let count = args.len();

        let elem_data = vm.heap.pod_types[elem_ty.index as usize].clone();
        let elem_inline = ScriptPodTypeInline {
            self_ref: elem_ty,
            data: elem_data,
        };

        let align_of = elem_inline.data.ty.align_of();
        let raw_size = elem_inline.data.ty.size_of();
        let stride = if raw_size % align_of != 0 { raw_size + (align_of - (raw_size % align_of)) } else { raw_size };
        let total_size = stride * count;

        let array_ty = vm.heap.new_pod_array_type(
            ScriptPodTy::FixedArray {
                align_of,
                size_of: total_size,
                len: count,
                ty: Box::new(elem_inline),
            },
            NIL,
        );

        let mut out = self.stack.new_string();

        if let Some(name) = vm.heap.pod_type_name(elem_ty) {
            if matches!(vm.heap.pod_types[elem_ty.index as usize].ty, ScriptPodTy::Struct { .. }) {
                output.structs.insert(elem_ty);
            }
            match output.backend {
                ShaderBackend::Wgsl => {
                    let name = output.backend.map_pod_name(name);
                    write!(out, "array<{}, {}>", name, count).ok();
                    write!(out, "(").ok();
                }
                ShaderBackend::Metal | ShaderBackend::Hlsl => {
                    write!(out, "{{").ok();
                }
                ShaderBackend::Glsl => {
                    let name = output.backend.map_pod_name(name);
                    write!(out, "{}[{}]", name, count).ok(); // array constructor
                    write!(out, "(").ok();
                }
            }
        } else {
            script_err_no_matching_shader_type!(self.trap, "no shader type for array element");
            match output.backend {
                ShaderBackend::Wgsl => {
                    write!(out, "(").ok();
                }
                ShaderBackend::Metal | ShaderBackend::Hlsl => {
                    write!(out, "{{").ok();
                }
                ShaderBackend::Glsl => {
                    write!(out, "(").ok(); // Should not happen if type not found
                }
            }
        }

        for (i, s) in args.iter().enumerate() {
            if i > 0 {
                out.push_str(", ");
            }
            out.push_str(s);
        }

        match output.backend {
            ShaderBackend::Wgsl | ShaderBackend::Glsl => {
                out.push_str(")");
            }
            ShaderBackend::Metal | ShaderBackend::Hlsl => {
                out.push_str("}");
            }
        }

        for s in args {
            self.stack.free_string(s);
        }

        self.stack.push(self.trap.pass(), ShaderType::Pod(array_ty), out);
    }

    pub(crate) fn handle_pod_construct(&mut self, vm: &mut ScriptVm, output: &mut ShaderOutput, pod_ty: ScriptPodType, args: Vec<ShaderPodArg>) {
        let mut offset = ScriptPodOffset::default();
        let pod_ty_data = &vm.heap.pod_types[pod_ty.index as usize];

        let mut out = self.stack.new_string();
        if let Some(name) = vm.heap.pod_type_name(pod_ty) {
            let name = output.backend.map_pod_name(name);
            match output.backend {
                ShaderBackend::Wgsl => {
                    write!(out, "{}(", name).ok();
                }
                ShaderBackend::Metal | ShaderBackend::Hlsl => {
                    if let ScriptPodTy::Struct { .. } = &pod_ty_data.ty {
                        write!(out, "{{").ok();
                    } else {
                        write!(out, "{}(", name).ok();
                    }
                }
                ShaderBackend::Glsl => {
                    write!(out, "{}(", name).ok();
                }
            }
        } else {
            script_err_no_matching_shader_type!(self.trap, "no shader type for pod construct");
        }

        if let Some(first) = args.first() {
            if first.name.is_some() {
                // Named args
                if let ScriptPodTy::Struct { fields, .. } = &pod_ty_data.ty {
                    for (i, field) in fields.iter().enumerate() {
                        if i > 0 {
                            out.push_str(", ");
                        }

                        // Find the arg with self name
                        if let Some(arg) = args.iter().find(|a| a.name.unwrap() == field.name) {
                            // Check type
                            match &arg.ty {
                                ShaderType::Pod(arg_pod_ty) => {
                                    if *arg_pod_ty != field.ty.self_ref {
                                        script_err_pod_type_not_matching!(self.trap, "named arg {:?} type mismatch", field.name);
                                    }
                                }
                                ShaderType::Id(id) => {
                                    if let Some((v, _name)) = self.shader_scope.find_var(*id) {
                                        if v.ty() != field.ty.self_ref {
                                            script_err_pod_type_not_matching!(self.trap, "var {:?} type mismatch for field {:?}", id, field.name);
                                        }
                                    } else {
                                        script_err_not_found!(self.trap, "var {:?} not found{}", id, suggest_from_live_ids(*id, &self.shader_scope.all_var_names()));
                                    }
                                }
                                ShaderType::AbstractInt => {
                                    let builtins = &vm.code.builtins.pod;
                                    if field.ty.self_ref != builtins.pod_i32
                                        && field.ty.self_ref != builtins.pod_u32
                                        && field.ty.self_ref != builtins.pod_f32
                                    {
                                        script_err_pod_type_not_matching!(self.trap, "abstract int not compatible with field {:?}", field.name);
                                    }
                                }
                                ShaderType::AbstractFloat => {
                                    let builtins = &vm.code.builtins.pod;
                                    if field.ty.self_ref != builtins.pod_f32 {
                                        script_err_pod_type_not_matching!(self.trap, "abstract float not compatible with field {:?}", field.name);
                                    }
                                }
                                _ => {}
                            }
                            out.push_str(&arg.s);
                        } else {
                            script_err_invalid_constructor_arg!(self.trap, "missing arg for field {:?}", field.name);
                        }
                    }

                    if args.len() != fields.len() {
                        script_err_invalid_arg_count!(self.trap, "expected {} args, got {}", fields.len(), args.len());
                    }
                } else {
                    script_err_unexpected!(self.trap, "named args require struct type");
                }
            } else {
                // Positional args
                for (i, arg) in args.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    match &arg.ty {
                        ShaderType::Pod(pod_ty_field) | ShaderType::PodPtr(pod_ty_field) => {
                            vm.heap.pod_check_constructor_arg(pod_ty, *pod_ty_field, &mut offset, self.trap.pass());
                        }
                        ShaderType::Id(id) => {
                            if let Some((v, _name)) = self.shader_scope.find_var(*id) {
                                vm.heap.pod_check_constructor_arg(pod_ty, v.ty(), &mut offset, self.trap.pass());
                            } else {
                                script_err_not_found!(self.trap, "var {:?} not found in constructor{}", id, suggest_from_live_ids(*id, &self.shader_scope.all_var_names()));
                            }
                        }
                        ShaderType::AbstractInt | ShaderType::AbstractFloat => {
                            vm.heap.pod_check_abstract_constructor_arg(pod_ty, &mut offset, self.trap.pass());
                        }
                        ShaderType::None
                        | ShaderType::Range { .. }
                        | ShaderType::Error(_)
                        | ShaderType::IoSelf(_)
                        | ShaderType::ScopeObject(_)
                        | ShaderType::ScopeUniformBuffer { .. }
                        | ShaderType::ScopeTexture { .. }
                        | ShaderType::PodType(_)
                        | ShaderType::Texture(_) => {}
                    }
                    out.push_str(&arg.s);
                }
                vm.heap.pod_check_constructor_arg_count(pod_ty, &offset, self.trap.pass());
            }
        } else {
            vm.heap.pod_check_constructor_arg_count(pod_ty, &offset, self.trap.pass());
        }

        match output.backend {
            ShaderBackend::Wgsl => {
                out.push_str(")");
            }
            ShaderBackend::Metal | ShaderBackend::Hlsl => {
                if let ScriptPodTy::Struct { .. } = &pod_ty_data.ty {
                    out.push_str("}");
                } else {
                    out.push_str(")");
                }
            }
            ShaderBackend::Glsl => {
                out.push_str(")");
            }
        }

        for arg in args {
            self.stack.free_string(arg.s);
        }

        self.stack.push(self.trap.pass(), ShaderType::Pod(pod_ty), out);
    }

    pub fn compile_shader_def(
        vm: &mut ScriptVm,
        output: &mut ShaderOutput,
        name: LiveId,
        fnobj: ScriptObject,
        sself: ShaderType,
        args: Vec<ShaderType>,
    ) -> (ScriptPodType, String) {
        let mut method_name_prefix = String::new();
        if let ShaderType::PodType(ty) = sself {
            if let Some(name) = vm.heap.pod_type_name(ty) {
                write!(method_name_prefix, "{}_", name).ok();
            }
        } else if let ShaderType::Pod(ty) = sself {
            if let Some(name) = vm.heap.pod_type_name(ty) {
                write!(method_name_prefix, "{}_", name).ok();
            }
        } else if let ShaderType::IoSelf(_) = sself {
            write!(method_name_prefix, "io_").ok();
        } else if let ShaderType::ScopeObject(obj) = sself {
            // Use the object index to create a unique prefix for scope object methods
            write!(method_name_prefix, "scope{}_", obj.index).ok();
        }

        // First pass: resolve AbstractInt/AbstractFloat against declared parameter types
        let builtins = &vm.code.builtins.pod;
        let argc = vm.heap.vec_len(fnobj);
        let mut resolved_args: Vec<ScriptPodType> = Vec::new();
        let mut argi = 0;
        for i in 0..argc {
            let kv = vm.heap.vec_key_value(fnobj, i, vm.thread.trap.pass());
            if kv.key == id!(self).into() {
                continue;
            }
            if argi >= args.len() {
                break;
            }
            let arg = &args[argi];
            // Get declared parameter type from kv.value
            // Try both direct pod_type value and object-based pod_type
            let declared_ty = kv.value.as_pod_type().or_else(|| vm.heap.pod_type(kv.value));

            let resolved = match arg {
                ShaderType::AbstractInt | ShaderType::AbstractFloat => {
                    // Use declared type if available, otherwise fall back to default
                    if let Some(declared) = declared_ty {
                        declared
                    } else {
                        arg.make_concrete(builtins).unwrap_or(builtins.pod_void)
                    }
                }
                _ => arg.make_concrete(builtins).unwrap_or(builtins.pod_void),
            };
            resolved_args.push(resolved);
            argi += 1;
        }

        // lets see if we already have fnobj with our argstypes
        if let Some(fun) = output.functions.iter().find(|v| v.fnobj == fnobj && v.args == resolved_args) {
            let mut fn_name = String::new();
            if fun.overload != 0 {
                write!(fn_name, "_f{}{}{}", fun.overload, method_name_prefix, name).ok();
            } else {
                write!(fn_name, "{}{}", method_name_prefix, name).ok();
            }
            write!(fn_name, "(").ok(); // Add opening paren to match new function path
            return (fun.ret, fn_name);
        }

        let overload = output.functions.iter().filter(|v| v.name == name).count();

        let mut compiler = ShaderFnCompiler::new(fnobj);
        let mut call_sig = String::new();

        let mut fn_name = String::new();
        let mut fn_args = String::new();

        if overload != 0 {
            write!(fn_name, "_f{}{}{}", overload, method_name_prefix, name).ok();
        } else {
            write!(fn_name, "{}{}", method_name_prefix, name).ok();
        }

        let mut has_self = false;
        write!(fn_args, "{}", output.backend.get_io_all_decl(output.mode)).ok();
        if let ShaderType::Pod(ty) = sself {
            has_self = true;
            match output.backend {
                ShaderBackend::Wgsl => {
                    if fn_args.len() > 0 {
                        write!(fn_args, ", ").ok();
                    }
                    write!(fn_args, "_self:ptr<function, ").ok();
                    if let Some(name) = vm.heap.pod_type_name(ty) {
                        let name = output.backend.map_pod_name(name);
                        write!(fn_args, "{}", name).ok();
                    }
                    write!(fn_args, ">").ok();
                }
                ShaderBackend::Metal => {
                    if let Some(name) = vm.heap.pod_type_name(ty) {
                        let name = output.backend.map_pod_name(name);
                        if fn_args.len() > 0 {
                            write!(fn_args, ", ").ok();
                        }
                        write!(fn_args, "thread {}& _self", name).ok();
                    }
                }
                ShaderBackend::Hlsl => {
                    if let Some(name) = vm.heap.pod_type_name(ty) {
                        let name = output.backend.map_pod_name(name);
                        if fn_args.len() > 0 {
                            write!(fn_args, ", ").ok();
                        }
                        write!(fn_args, "inout {} _self", name).ok();
                    }
                }
                ShaderBackend::Glsl => {
                    if let Some(name) = vm.heap.pod_type_name(ty) {
                        let name = output.backend.map_pod_name(name);
                        if fn_args.len() > 0 {
                            write!(fn_args, ", ").ok();
                        }
                        write!(fn_args, "inout {} _self", name).ok();
                    }
                }
            }
            compiler.shader_scope.define_let(id!(self), ty);
        } else if let ShaderType::PodType(ty) = sself {
            compiler.shader_scope.define_pod_type(id!(self), ty);
        } else if let ShaderType::IoSelf(obj) = sself {
            if fn_args.len() > 0 {
                write!(fn_args, ", ").ok();
            }
            write!(fn_args, "{}", output.backend.get_io_self_decl(output.mode)).ok();
            compiler.shader_scope.define_io_self(obj);
        } else if let ShaderType::ScopeObject(obj) = sself {
            // ScopeObject methods don't have a _self parameter - `self` references
            // are resolved to IoScopeUniform accesses at compile time
            compiler.shader_scope.define_scope_object(obj);
        }

        let argc = vm.heap.vec_len(fnobj);
        let mut argi = 0;
        for i in 0..argc {
            let kv = vm.heap.vec_key_value(fnobj, i, vm.thread.trap.pass());

            if kv.key == id!(self).into() {
                if !has_self || argi != 0 {
                    script_err_invalid_arg_name!(vm.thread.trap, "self arg must be first with has_self");
                }
                continue;
            }

            if let Some(id) = kv.key.as_id() {
                if fn_args.len() > 0 {
                    write!(fn_args, ", ").ok();
                }
                if argi >= resolved_args.len() {
                    script_err_invalid_arg_count!(vm.thread.trap, "more formal params than resolved args");
                    break;
                }
                let arg_ty = resolved_args[argi];

                match output.backend {
                    ShaderBackend::Wgsl => {
                        write!(fn_args, "{}:", id).ok();
                        if let Some(name) = vm.heap.pod_type_name(arg_ty) {
                            let name = output.backend.map_pod_name(name);
                            write!(fn_args, "{}", name).ok();
                        } else {
                            // todo!()
                        }
                    }
                    ShaderBackend::Metal | ShaderBackend::Hlsl | ShaderBackend::Glsl => {
                        if let Some(name) = vm.heap.pod_type_name(arg_ty) {
                            let name = output.backend.map_pod_name(name);
                            write!(fn_args, "{} {}", name, id).ok();
                        } else {
                            // todo!()
                        }
                    }
                }
                compiler.shader_scope.define_let(id, arg_ty);
            }
            argi += 1;
        }
        if argi < resolved_args.len() {
            script_err_invalid_arg_count!(vm.thread.trap, "fewer formal params than resolved args");
        }

        if let Some(fnptr) = vm.heap.as_fn(fnobj) {
            if let ScriptFnPtr::Script(fnip) = fnptr {
                if output.recur_block.iter().any(|v| *v == fnobj) {
                    script_err_recursion_not_allowed!(vm.thread.trap, "shader functions cannot recurse");
                    (vm.code.builtins.pod.pod_void, fn_name)
                } else {
                    output.recur_block.push(fnobj);
                    let ret = compiler.compile_fn(vm, output, fnip);
                    output.recur_block.pop();

                    // Ensure struct return types are registered in output.structs
                    if let ScriptPodTy::Struct{..} = vm.heap.pod_type_ref(ret).ty {
                        output.structs.insert(ret);
                    }

                    match output.backend {
                        ShaderBackend::Wgsl => {
                            write!(call_sig, "fn {}({})", fn_name, fn_args).ok();
                            if let Some(name) = vm.heap.pod_type_name(ret) {
                                if name != id!(void) {
                                    let name = output.backend.map_pod_name(name);
                                    write!(call_sig, "->{}", name).ok();
                                }
                            }
                        }
                        ShaderBackend::Metal | ShaderBackend::Hlsl | ShaderBackend::Glsl => {
                            let ret_name = if let Some(name) = vm.heap.pod_type_name(ret) {
                                output.backend.map_pod_name(name)
                            } else {
                                id!(void)
                            };
                            write!(call_sig, "{} {}({})", ret_name, fn_name, fn_args).ok();
                        }
                    }

                    output.functions.push(ShaderFn {
                        overload,
                        call_sig,
                        name,
                        args: resolved_args,
                        fnobj,
                        out: compiler.out,
                        ret,
                    });
                    write!(fn_name, "(").ok();
                    (ret, fn_name)
                }
            } else {
                panic!()
            }
        } else {
            panic!()
        }
    }

    pub(crate) fn handle_script_call(
        &mut self,
        vm: &mut ScriptVm,
        output: &mut ShaderOutput,
        mut out: String,
        name: LiveId,
        fnobj: ScriptObject,
        sself: ShaderType,
        args: Vec<ShaderType>,
    ) {
        // we should compare number of arguments (needs to be exact)
        // Note: fn_name already includes "(" at the end from compile_shader_def
        let (ret, fn_name) = Self::compile_shader_def(vm, output, name, fnobj, sself, args);
        out.insert_str(0, &fn_name);
        out.push_str(")");
        self.stack.push(self.trap.pass(), ShaderType::Pod(ret), out);
    }

    pub(crate) fn handle_call_exec(&mut self, vm: &mut ScriptVm, output: &mut ShaderOutput) {
        // Only pop if the top of the stack is a call-related ME, not a control flow ME
        // This prevents corrupting the ME stack when a call setup (CALL_ARGS/CALL_METHOD_ARGS) fails
        let is_call_me = matches!(
            self.mes.last(),
            Some(ShaderMe::ArrayConstruct { .. })
                | Some(ShaderMe::Pod { .. })
                | Some(ShaderMe::ScriptCall { .. })
                | Some(ShaderMe::TextureBuiltin { .. })
                | Some(ShaderMe::BuiltinCall { .. })
        );
        if !is_call_me {
            // No call ME was pushed - the call setup must have failed
            // Push a dummy value onto the stack so subsequent code doesn't break
            let s = self.stack.new_string();
            self.stack.push(self.trap.pass(), ShaderType::Error(NIL), s);
            return;
        }
        if let Some(me) = self.mes.pop() {
            match me {
                ShaderMe::ArrayConstruct { args, elem_ty } => {
                    self.handle_array_construct(vm, output, args, elem_ty);
                }
                ShaderMe::Pod { pod_ty, args } => {
                    self.handle_pod_construct(vm, output, pod_ty, args);
                }
                ShaderMe::ScriptCall { out, name, fnobj, sself, args } => {
                    self.handle_script_call(vm, output, out, name, fnobj, sself, args);
                }
                ShaderMe::TextureBuiltin { method_id, tex_type: _, texture_expr, args } => {
                    self.handle_texture_builtin_exec(vm, output, method_id, texture_expr, args);
                }
                ShaderMe::BuiltinCall { name, fnptr: _, args } => {
                    let builtins = &vm.code.builtins.pod;

                    // Check if any arg is a float type - if so, abstract ints should be floats
                    let has_float = args.iter().any(|(ty, _)| match ty {
                        ShaderType::Pod(pt) => vm.heap.pod_types[pt.index as usize].ty.is_float_type(),
                        ShaderType::AbstractFloat => true,
                        _ => false,
                    });

                    // Build concrete args for type_table_builtin and format output
                    let mut concrete_args = Vec::new();
                    let mut out = self.stack.new_string();
                    let mapped_name = output.backend.map_builtin_name(name);
                    write!(out, "{}(", mapped_name).ok();

                    for (i, (ty, s)) in args.into_iter().enumerate() {
                        if i > 0 {
                            out.push_str(", ");
                        }

                        match &ty {
                            ShaderType::AbstractInt | ShaderType::AbstractFloat => {
                                if has_float {
                                    // Format as float literal
                                    concrete_args.push(builtins.pod_f32);
                                    // Check if s is a simple integer that needs .0 suffix
                                    if s.chars().all(|c| c.is_ascii_digit() || c == '-') {
                                        out.push_str(&s);
                                        out.push_str(".0");
                                    } else {
                                        out.push_str(&s);
                                    }
                                } else {
                                    concrete_args.push(ty.make_concrete(builtins).unwrap_or(builtins.pod_void));
                                    out.push_str(&s);
                                }
                            }
                            ShaderType::Pod(pt) => {
                                concrete_args.push(*pt);
                                out.push_str(&s);
                            }
                            _ => {
                                concrete_args.push(ty.make_concrete(builtins).unwrap_or(builtins.pod_void));
                                out.push_str(&s);
                            }
                        }
                        self.stack.free_string(s);
                    }

                    out.push_str(")");
                    let ret = type_table_builtin(name, &concrete_args, builtins, self.trap.pass());
                    self.stack.push(self.trap.pass(), ShaderType::Pod(ret), out);
                }
                _ => {
                    // This case should not be reached due to the guard at the top of handle_call_exec
                    script_err_not_impl!(self.trap, "CALL_EXEC: unexpected call type in shader (internal error)");
                }
            }
        }
    }

    pub(crate) fn handle_texture_builtin_exec(
        &mut self,
        vm: &mut ScriptVm,
        output: &mut ShaderOutput,
        method_id: LiveId,
        texture_expr: String,
        args: Vec<String>,
    ) {
        // Handle texture methods - these are virtual methods that transpile to backend-specific code
        match method_id {
            id!(size) => {
                // size() returns vec2f with the texture dimensions
                let mut s = self.stack.new_string();
                match output.backend {
                    ShaderBackend::Metal => {
                        // Metal: float2(texture.get_width(), texture.get_height())
                        write!(s, "float2({}.get_width(), {}.get_height())", texture_expr, texture_expr).ok();
                    }
                    ShaderBackend::Wgsl => {
                        // WGSL: textureDimensions(texture) returns vec2<u32>, cast to vec2<f32>
                        write!(s, "vec2f(textureDimensions({}))", texture_expr).ok();
                    }
                    ShaderBackend::Hlsl => {
                        // HLSL: GetDimensions requires output params, use helper function
                        output.hlsl_needs_tex_size = true;
                        write!(s, "_mpTexSize2D({})", texture_expr).ok();
                    }
                    ShaderBackend::Glsl => {
                        // GLSL: textureSize(texture, 0) returns ivec2, cast to vec2
                        write!(s, "vec2(textureSize({}, 0))", texture_expr).ok();
                    }
                }
                self.stack.push(self.trap.pass(), ShaderType::Pod(vm.code.builtins.pod.pod_vec2f), s);
            }
            id!(sample) => {
                // sample(coord) samples the texture at normalized coordinates
                // Returns vec4f
                if args.len() != 1 {
                    script_err_invalid_arg_count!(self.trap, "texture.sample requires 1 arg");
                    let empty = self.stack.new_string();
                    self.stack.push(self.trap.pass(), ShaderType::Pod(vm.code.builtins.pod.pod_vec4f), empty);
                } else {
                    let coord = &args[0];
                    let mut s = self.stack.new_string();

                    // Get or create the default sampler (linear, repeat, normalized)
                    let sampler = ShaderSampler::default();
                    let sampler_idx = output.get_or_create_sampler(sampler);

                    match output.backend {
                        ShaderBackend::Metal => {
                            // Metal: texture.sample(sampler, coord)
                            write!(s, "{}.sample(_s{}, {})", texture_expr, sampler_idx, coord).ok();
                        }
                        ShaderBackend::Wgsl => {
                            // WGSL: textureSample(texture, sampler, coord)
                            write!(s, "textureSample({}, _s{}, {})", texture_expr, sampler_idx, coord).ok();
                        }
                        ShaderBackend::Hlsl => {
                            // HLSL: texture.Sample(sampler, coord)
                            write!(s, "{}.Sample(_s{}, {})", texture_expr, sampler_idx, coord).ok();
                        }
                        ShaderBackend::Glsl => {
                            // GLSL 4.0+: texture(sampler2D(texture, sampler), coord)
                            write!(s, "texture(sampler2D({}, _s{}), {})", texture_expr, sampler_idx, coord).ok();
                        }
                    }
                    self.stack.push(self.trap.pass(), ShaderType::Pod(vm.code.builtins.pod.pod_vec4f), s);
                }
            }
            _ => {
                script_err_not_found!(self.trap, "unknown texture method {:?}{}",  method_id, suggest_from_live_ids(method_id, &[id!(sample), id!(size)]));
            }
        }
        self.stack.free_string(texture_expr);
        for arg in args {
            self.stack.free_string(arg);
        }
    }

    pub(crate) fn handle_method_call_args(&mut self, vm: &mut ScriptVm, output: &mut ShaderOutput, opargs: OpcodeArgs) {
        let (method_ty, method_s) = self.stack.pop(self.trap.pass());
        let (self_ty, self_s) = self.stack.pop(self.trap.pass());
        self.stack.free_string(method_s);

        if let ShaderType::Id(method_id) = method_ty {
            // Handle method calls on Texture types (e.g., texture.size())
            if let ShaderType::Texture(tex_type) = self_ty {
                self.handle_texture_method_call_args(vm, output, opargs, method_id, tex_type, self_s);
                return;
            }
            
            // Handle method calls on ScopeTexture types (e.g., scope_texture.sample())
            if let ShaderType::ScopeTexture { tex_type, .. } = self_ty {
                self.handle_texture_method_call_args(vm, output, opargs, method_id, tex_type, self_s);
                return;
            }

            if let ShaderType::Id(self_id) = self_ty {
                // Try to resolve as variable on shader scope - extract info before mutable borrows
                let scope_info = self.shader_scope.find_var(self_id).map(|(var, _)| match var {
                    ShaderScopeItem::IoSelf(obj) => (Some(*obj), None),
                    _ => (None, Some(var.ty())),
                });

                if let Some((io_self_obj, pod_ty_opt)) = scope_info {
                    // Method call on IoSelf
                    if let Some(obj) = io_self_obj {
                        if self.handle_io_self_method_call_args(vm, output, opargs, method_id, obj, &self_s) {
                            self.stack.free_string(self_s);
                            return;
                        }
                    }

                    // Method call on a Pod instance
                    if let Some(pod_ty) = pod_ty_opt {
                        let self_s_slice = if self_id == id!(self) { "_self" } else { &self_s };
                        if self.handle_pod_method_call_args(vm, output, opargs, method_id, pod_ty, self_s_slice, &self_s) {
                            return;
                        }
                    }
                    
                    // Method not found on the type
                    self.stack.free_string(self_s);
                    let type_name = if let Some(pod_ty) = pod_ty_opt {
                        vm.heap.pod_type_name(pod_ty)
                            .map(|id| id.as_string(|s| s.unwrap_or("unknown").to_string()))
                            .unwrap_or_else(|| "unknown".to_string())
                    } else {
                        format!("{:?}", self_id)
                    };
                    script_err_not_found!(self.trap, "method {:?} not found on {}", method_id, type_name);
                    return;
                } else {
                    // Try to resolve as PodType static method in script scope
                    if self.handle_pod_type_method_call_args(vm, output, opargs, method_id, self_id, &self_s) {
                        return;
                    }
                    
                    // Not a PodType - try as ScopeObject method call
                    if self.handle_scope_object_method_call_by_id(vm, output, opargs, method_id, self_id) {
                        self.stack.free_string(self_s);
                        return;
                    }
                    
                    // Try as scope texture method call (e.g., test_tex.sample(...))
                    if self.handle_scope_texture_method_call_by_id(vm, output, opargs, method_id, self_id) {
                        self.stack.free_string(self_s);
                        return;
                    }
                    
                    // Nothing matched - variable or type not found
                    self.stack.free_string(self_s);
                    script_err_not_found!(self.trap, "method {:?} not found on {:?}", method_id, self_id);
                    return;
                }
            }
            
            // self_ty wasn't an Id - could be a Pod or other type
            let type_name = self.shader_type_to_string(vm, &self_ty);
            self.stack.free_string(self_s);
            script_err_not_found!(self.trap, "method {:?} not found on {}", method_id, type_name);
            return;
        }
        
        self.stack.free_string(self_s);
        script_err_not_impl!(self.trap, "METHOD_CALL_ARGS: method call syntax not valid here");
    }

    pub(crate) fn handle_io_self_method_call_args(
        &mut self,
        vm: &mut ScriptVm,
        output: &mut ShaderOutput,
        opargs: OpcodeArgs,
        method_id: LiveId,
        obj: ScriptObject,
        _self_s: &str,
    ) -> bool {
        let fnobj = vm.heap.value(obj, method_id.into(), self.trap.pass());
        if let Some(fnobj) = fnobj.as_object() {
            if let Some(fnptr) = vm.heap.as_fn(fnobj) {
                match fnptr {
                    ScriptFnPtr::Script(_fnptr) => {
                        let mut out = self.stack.new_string();
                        write!(out, "{}", output.backend.get_io_all(output.mode)).ok();
                        if out.len() > 0 {
                            write!(out, ", ").ok();
                        }
                        write!(out, "{}", output.backend.get_io_self(output.mode)).ok();
                        self.mes.push(ShaderMe::ScriptCall {
                            name: method_id,
                            out,
                            fnobj,
                            sself: ShaderType::IoSelf(obj),
                            args: vec![],
                        });
                    }
                    ScriptFnPtr::Native(_) => {
                        todo!()
                    }
                }
                self.maybe_pop_to_me(vm, opargs);
                return true;
            }
        }
        false
    }
    
    pub(crate) fn handle_scope_object_method_call_args(
        &mut self,
        vm: &mut ScriptVm,
        output: &mut ShaderOutput,
        opargs: OpcodeArgs,
        method_id: LiveId,
        obj: ScriptObject,
    ) -> bool {
        // Look up the method on the scope object
        let fnobj = vm.heap.value(obj, method_id.into(), self.trap.pass());
        if let Some(fnobj) = fnobj.as_object() {
            if let Some(fnptr) = vm.heap.as_fn(fnobj) {
                match fnptr {
                    ScriptFnPtr::Script(_fnptr) => {
                        // For ScopeObject methods, we only pass the io_all parameter
                        // since `self` references are resolved to IoScopeUniform accesses
                        // at compile time (no runtime _self parameter)
                        let mut out = self.stack.new_string();
                        write!(out, "{}", output.backend.get_io_all(output.mode)).ok();
                        self.mes.push(ShaderMe::ScriptCall {
                            name: method_id,
                            out,
                            fnobj,
                            sself: ShaderType::ScopeObject(obj),
                            args: vec![],
                        });
                    }
                    ScriptFnPtr::Native(_) => {
                        // Native methods on scope objects not supported
                        script_err_opcode_not_supported_in_shader!(self.trap, "native methods not supported on scope objects");
                        return false;
                    }
                }
                self.maybe_pop_to_me(vm, opargs);
                return true;
            }
        }
        false
    }
    
    /// Handle method call on a scope object identified by name (self_id).
    /// This is called when PodType handling didn't match - we try to resolve
    /// the identifier as a scope object and call the method on it.
    pub(crate) fn handle_scope_object_method_call_by_id(
        &mut self,
        vm: &mut ScriptVm,
        output: &mut ShaderOutput,
        opargs: OpcodeArgs,
        method_id: LiveId,
        self_id: LiveId,
    ) -> bool {
        // Look up self_id in script scope
        let script_value = vm.heap.scope_value(self.script_scope, self_id.into(), NoTrap);
        if script_value.is_nil() || script_value.is_err() {
            return false;
        }
        
        // Must be an object
        let value_obj = match script_value.as_object() {
            Some(obj) => obj,
            None => return false,
        };
        
        // Must not be a shader_io type or a function
        if vm.heap.as_shader_io(value_obj).is_some() || vm.heap.as_fn(value_obj).is_some() {
            return false;
        }
        
        // It's a scope object - handle the method call
        self.handle_scope_object_method_call_args(vm, output, opargs, method_id, value_obj)
    }
    
    /// Handle method call on a scope texture identified by name (self_id).
    /// This is called for expressions like `test_tex.sample(coord)` where `test_tex`
    /// is a texture defined in the script scope.
    pub(crate) fn handle_scope_texture_method_call_by_id(
        &mut self,
        vm: &mut ScriptVm,
        output: &mut ShaderOutput,
        _opargs: OpcodeArgs,
        method_id: LiveId,
        self_id: LiveId,
    ) -> bool {
        use crate::mod_shader::*;
        use std::fmt::Write;
        
        // Look up self_id in script scope
        let script_value = vm.heap.scope_value(self.script_scope, self_id.into(), NoTrap);
        if script_value.is_nil() || script_value.is_err() {
            return false;
        }
        
        // Must be an object
        let value_obj = match script_value.as_object() {
            Some(obj) => obj,
            None => return false,
        };
        
        // Must be a texture shader_io type
        let io_type = match vm.heap.as_shader_io(value_obj) {
            Some(io_type) => io_type,
            None => return false,
        };
        
        // Check if it's a texture type
        let tex_type = match io_type {
            SHADER_IO_TEXTURE_1D => TextureType::Texture1d,
            SHADER_IO_TEXTURE_1D_ARRAY => TextureType::Texture1dArray,
            SHADER_IO_TEXTURE_2D => TextureType::Texture2d,
            SHADER_IO_TEXTURE_2D_ARRAY => TextureType::Texture2dArray,
            SHADER_IO_TEXTURE_3D => TextureType::Texture3d,
            SHADER_IO_TEXTURE_3D_ARRAY => TextureType::Texture3dArray,
            SHADER_IO_TEXTURE_CUBE => TextureType::TextureCube,
            SHADER_IO_TEXTURE_CUBE_ARRAY => TextureType::TextureCubeArray,
            SHADER_IO_TEXTURE_DEPTH => TextureType::TextureDepth,
            SHADER_IO_TEXTURE_DEPTH_ARRAY => TextureType::TextureDepthArray,
            _ => return false,
        };
        
        // Check if we already have this scope texture registered
        let existing = output.scope_textures.iter().find(|st| st.obj == value_obj);
        
        let shader_name = if let Some(existing) = existing {
            existing.shader_name
        } else {
            // Generate unique name for this scope texture
            let shader_name = self.generate_scope_texture_name(output, self_id, value_obj);
            
            // Add to scope_textures for runtime tracking
            output.scope_textures.push(ScopeTextureSource {
                obj: value_obj,
                tex_type,
                shader_name,
            });
            
            // Add to IO list as Texture
            if !output.io.iter().any(|io| io.name == shader_name && matches!(io.kind, ShaderIoKind::Texture(_))) {
                output.io.push(ShaderIo {
                    kind: ShaderIoKind::Texture(tex_type),
                    name: shader_name,
                    ty: ScriptPodType::VOID, // Textures don't have a pod type
                    buffer_index: None,
                });
            }
            
            shader_name
        };
        
        // Generate the texture expression with proper prefix
        let mut texture_expr = self.stack.new_string();
        let (_, prefix) = output.backend.get_shader_io_kind_and_prefix(output.mode, io_type);
        match prefix {
            ShaderIoPrefix::Prefix(prefix) => write!(texture_expr, "{}{}", prefix, shader_name).ok(),
            ShaderIoPrefix::Full(full) => write!(texture_expr, "{}", full).ok(),
            ShaderIoPrefix::FullOwned(full) => write!(texture_expr, "{}", full).ok(),
        };
        
        // Push TextureBuiltin ME to handle the method call
        self.mes.push(ShaderMe::TextureBuiltin {
            method_id,
            tex_type,
            texture_expr,
            args: vec![],
        });
        
        true
    }

    pub(crate) fn handle_pod_method_call_args(
        &mut self,
        vm: &mut ScriptVm,
        output: &mut ShaderOutput,
        opargs: OpcodeArgs,
        method_id: LiveId,
        pod_ty: ScriptPodType,
        self_s_slice: &str,
        self_s: &String,
    ) -> bool {
        let pod_ty_data = &vm.heap.pod_types[pod_ty.index as usize];
        let fnobj = vm.heap.value(pod_ty_data.object, method_id.into(), self.trap.pass());

        if let Some(fnobj) = fnobj.as_object() {
            if let Some(fnptr) = vm.heap.as_fn(fnobj) {
                match fnptr {
                    ScriptFnPtr::Script(_fnptr) => {
                        let mut out = self.stack.new_string();
                        write!(out, "{}", output.backend.get_io_all(output.mode)).ok();
                        match output.backend {
                            ShaderBackend::Wgsl => {
                                if out.len() > 0 {
                                    write!(out, ", ").ok();
                                }
                                write!(out, "&{}", self_s_slice).ok();
                            }
                            ShaderBackend::Metal => {
                                // Metal uses references (thread T&), not pointers
                                // Pass the variable directly without &
                                if out.len() > 0 {
                                    write!(out, ", ").ok();
                                }
                                write!(out, "{}", self_s_slice).ok();
                            }
                            ShaderBackend::Hlsl | ShaderBackend::Glsl => {
                                if out.len() > 0 {
                                    write!(out, ", ").ok();
                                }
                                write!(out, "{}", self_s_slice).ok();
                            }
                        }
                        self.mes.push(ShaderMe::ScriptCall {
                            name: method_id,
                            out,
                            fnobj,
                            sself: ShaderType::Pod(pod_ty),
                            args: vec![],
                        });
                    }
                    ScriptFnPtr::Native(fnptr) => {
                        // Store self as first argument
                        let mut self_arg = self.stack.new_string();
                        write!(self_arg, "{}", self_s_slice).ok();
                        self.mes.push(ShaderMe::BuiltinCall {
                            name: method_id,
                            fnptr,
                            args: vec![(ShaderType::Pod(pod_ty), self_arg)],
                        });
                    }
                }
                self.stack.free_string(self_s.clone());
                self.maybe_pop_to_me(vm, opargs);
                return true;
            }
        }
        false
    }

    pub(crate) fn handle_pod_type_method_call_args(
        &mut self,
        vm: &mut ScriptVm,
        output: &mut ShaderOutput,
        opargs: OpcodeArgs,
        method_id: LiveId,
        self_id: LiveId,
        self_s: &String,
    ) -> bool {
        let value = vm.heap.scope_value(self.script_scope, self_id.into(), self.trap.pass());
        if let Some(pod_ty) = vm.heap.pod_type(value) {
            self.ensure_struct_name(vm, output, pod_ty, self_id);
            // It is a PodType. Look up static method.
            let pod_ty_data = &vm.heap.pod_types[pod_ty.index as usize];
            let fnobj = vm.heap.value(pod_ty_data.object, method_id.into(), self.trap.pass());

            if let Some(fnobj) = fnobj.as_object() {
                if let Some(fnptr) = vm.heap.as_fn(fnobj) {
                    match fnptr {
                        ScriptFnPtr::Script(_fnptr) => {
                            let mut out = self.stack.new_string();
                            write!(out, "{}", output.backend.get_io_all(output.mode)).ok();
                            self.mes.push(ShaderMe::ScriptCall {
                                name: method_id,
                                out,
                                fnobj,
                                sself: ShaderType::PodType(pod_ty),
                                args: Default::default(),
                            });
                        }
                        ScriptFnPtr::Native(fnptr) => {
                            self.mes.push(ShaderMe::BuiltinCall {
                                name: method_id,
                                fnptr,
                                args: Default::default(),
                            });
                        }
                    }
                    self.stack.free_string(self_s.clone());
                    self.maybe_pop_to_me(vm, opargs);
                    return true;
                }
            }
        }
        false
    }

    pub(crate) fn handle_texture_method_call_args(
        &mut self,
        _vm: &mut ScriptVm,
        _output: &mut ShaderOutput,
        _opargs: OpcodeArgs,
        method_id: LiveId,
        tex_type: TextureType,
        texture_expr: String,
    ) {
        // Push TextureBuiltin to collect arguments - actual code gen happens in handle_call_exec
        self.mes.push(ShaderMe::TextureBuiltin {
            method_id,
            tex_type,
            texture_expr,
            args: vec![],
        });
    }
}

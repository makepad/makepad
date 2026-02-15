//! Shader function and method call operations
//!
//! This module contains handle functions for function calls, method calls,
//! type construction (Pod, Array), and builtin calls.

use crate::function::*;
use crate::opcode::*;
use crate::pod::*;
use crate::shader::*;
use crate::shader_backend::*;
use crate::shader_builtins::*;
use crate::suggest::*;
use crate::trap::*;
use crate::value::*;
use crate::vm::*;
use crate::*;
use makepad_live_id::*;
use std::fmt::Write;

impl ShaderFnCompiler {
    pub(crate) fn handle_pod_type_call(
        &mut self,
        vm: &mut ScriptVm,
        output: &mut ShaderOutput,
        opargs: OpcodeArgs,
        pod_ty: ScriptPodType,
        name: LiveId,
    ) {
        if let ScriptPodTy::ArrayBuilder = &vm.bx.heap.pod_types[pod_ty.index as usize].ty {
            self.mes.push(ShaderMe::ArrayConstruct {
                args: Vec::new(),
                elem_ty: None,
            });
            self.maybe_pop_to_me(vm, output, opargs);
            return;
        }

        // alright lets see what Id we got
        let _name = self.ensure_struct_name(vm, output, pod_ty, name);

        self.mes.push(ShaderMe::Pod {
            pod_ty,
            args: Vec::new(),
        });

        self.maybe_pop_to_me(vm, output, opargs);
    }

    pub(crate) fn handle_call_args(
        &mut self,
        vm: &mut ScriptVm,
        output: &mut ShaderOutput,
        opargs: OpcodeArgs,
    ) {
        let (ty, _s) = self.stack.pop(self.trap.pass());
        if let ShaderType::Id(name) = ty {
            // Check shader scope for PodType
            if let Some((ShaderScopeItem::PodType { ty, .. }, _)) = self.shader_scope.find_var(name)
            {
                self.handle_pod_type_call(vm, output, opargs, *ty, name);
                return;
            }

            // alright lets look it up on our script scope
            let value = vm
                .bx
                .heap
                .scope_value(self.script_scope, name.into(), self.trap.pass());
            // lets check if our obj is a PodType
            if let Some(pod_ty) = vm.bx.heap.pod_type(value) {
                self.handle_pod_type_call(vm, output, opargs, pod_ty, name);
                return;
            }

            if let Some(fnobj) = value.as_object() {
                if let Some(fnptr) = vm.bx.heap.as_fn(fnobj) {
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
                            self.maybe_pop_to_me(vm, output, opargs);
                            return;
                        }
                    }

                    self.maybe_pop_to_me(vm, output, opargs);
                    return;
                }
            }
        }
        script_err_wrong_value!(self.trap, "shader call target is not a function");
    }

    pub(crate) fn handle_array_construct(
        &mut self,
        vm: &mut ScriptVm,
        output: &mut ShaderOutput,
        args: Vec<String>,
        elem_ty: Option<ScriptPodType>,
    ) {
        let elem_ty = elem_ty.unwrap_or(vm.bx.code.builtins.pod.pod_f32);
        let count = args.len();

        let elem_data = vm.bx.heap.pod_types[elem_ty.index as usize].clone();
        let elem_inline = ScriptPodTypeInline {
            self_ref: elem_ty,
            data: elem_data,
        };

        let align_of = elem_inline.data.ty.align_of();
        let raw_size = elem_inline.data.ty.size_of();
        let stride = if raw_size % align_of != 0 {
            raw_size + (align_of - (raw_size % align_of))
        } else {
            raw_size
        };
        let total_size = stride * count;

        let array_ty = vm.bx.heap.new_pod_array_type(
            ScriptPodTy::FixedArray {
                align_of,
                size_of: total_size,
                len: count,
                ty: Box::new(elem_inline),
            },
            NIL,
        );

        let mut out = self.stack.new_string();

        if let Some(name) = vm.bx.heap.pod_type_name(elem_ty) {
            if matches!(
                vm.bx.heap.pod_types[elem_ty.index as usize].ty,
                ScriptPodTy::Struct { .. }
            ) {
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
                ShaderBackend::Glsl | ShaderBackend::Rust => {
                    let name = output.backend.map_pod_name(name);
                    write!(out, "{}[{}]", name, count).ok(); // array constructor
                    write!(out, "(").ok();
                }
            }
        } else {
            script_err_shader!(self.trap, "no shader type for array element");
            match output.backend {
                ShaderBackend::Wgsl => {
                    write!(out, "(").ok();
                }
                ShaderBackend::Metal | ShaderBackend::Hlsl => {
                    write!(out, "{{").ok();
                }
                ShaderBackend::Glsl | ShaderBackend::Rust => {
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
            ShaderBackend::Wgsl | ShaderBackend::Glsl | ShaderBackend::Rust => {
                out.push_str(")");
            }
            ShaderBackend::Metal | ShaderBackend::Hlsl => {
                out.push_str("}");
            }
        }

        for s in args {
            self.stack.free_string(s);
        }

        self.stack
            .push(self.trap.pass(), ShaderType::Pod(array_ty), out);
    }

    pub(crate) fn handle_pod_construct(
        &mut self,
        vm: &mut ScriptVm,
        output: &mut ShaderOutput,
        pod_ty: ScriptPodType,
        args: Vec<ShaderPodArg>,
    ) {
        let mut offset = ScriptPodOffset::default();
        let pod_ty_data = &vm.bx.heap.pod_types[pod_ty.index as usize];

        let mut out = self.stack.new_string();
        if let Some(name) = vm.bx.heap.pod_type_name(pod_ty) {
            let name = output.backend.map_pod_name(name);
            match output.backend {
                ShaderBackend::Wgsl => {
                    write!(out, "{}(", name).ok();
                }
                ShaderBackend::Metal => {
                    if let ScriptPodTy::Struct { .. } = &pod_ty_data.ty {
                        write!(out, "{{").ok();
                    } else {
                        write!(out, "{}(", name).ok();
                    }
                }
                ShaderBackend::Hlsl => {
                    if let ScriptPodTy::Struct { .. } = &pod_ty_data.ty {
                        write!(out, "consfn_{}(", name).ok();
                    } else {
                        write!(out, "{}(", name).ok();
                    }
                }
                ShaderBackend::Glsl => {
                    write!(out, "{}(", name).ok();
                }
                ShaderBackend::Rust => {
                    if let ScriptPodTy::Struct { .. } = &pod_ty_data.ty {
                        write!(out, "{} {{ ", name).ok();
                    } else {
                        // For vec types, we need to expand heterogeneous constructors
                        // like vec4f(vec3, f32) → vec4(v.x, v.y, v.z, s)
                        // This is handled by rust_expand_pod_construct below
                        let total_slots = pod_ty_data.ty.slots();
                        if args.len() != total_slots && total_slots > 1 {
                            // Expand heterogeneous constructor
                            let base_name = match total_slots {
                                2 => "vec2",
                                3 => "vec3",
                                4 => "vec4",
                                _ => "vec4",
                            };
                            let expanded = self.rust_expand_pod_construct(vm, &args, total_slots);
                            write!(out, "{}({})", base_name, expanded).ok();

                            for arg in args {
                                self.stack.free_string(arg.s);
                            }
                            self.stack
                                .push(self.trap.pass(), ShaderType::Pod(pod_ty), out);
                            return;
                        }
                        write!(out, "{}(", name).ok();
                    }
                }
            }
        } else {
            script_err_shader!(self.trap, "no shader type for pod construct");
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
                                        script_err_pod!(
                                            self.trap,
                                            "named arg {:?} type mismatch: expected {}, got {}",
                                            field.name,
                                            format_pod_type_name(&vm.bx.heap, field.ty.self_ref),
                                            format_pod_type_name(&vm.bx.heap, *arg_pod_ty)
                                        );
                                    }
                                }
                                ShaderType::Id(id) => {
                                    if let Some((v, _name)) = self.shader_scope.find_var(*id) {
                                        if v.ty() != field.ty.self_ref {
                                            script_err_pod!(self.trap, "var {:?} type mismatch for field {:?}: expected {}, got {}", id, field.name, format_pod_type_name(&vm.bx.heap, field.ty.self_ref), format_pod_type_name(&vm.bx.heap, v.ty()));
                                        }
                                    } else {
                                        script_err_not_found!(
                                            self.trap,
                                            "var {:?} not found{}",
                                            id,
                                            suggest_from_live_ids(
                                                *id,
                                                &self.shader_scope.all_var_names()
                                            )
                                        );
                                    }
                                }
                                ShaderType::AbstractInt => {
                                    let builtins = &vm.bx.code.builtins.pod;
                                    if field.ty.self_ref != builtins.pod_i32
                                        && field.ty.self_ref != builtins.pod_u32
                                        && field.ty.self_ref != builtins.pod_f32
                                    {
                                        script_err_pod!(self.trap, "abstract int not compatible with field {:?} (expects {})", field.name, format_pod_type_name(&vm.bx.heap, field.ty.self_ref));
                                    }
                                }
                                ShaderType::AbstractFloat => {
                                    let builtins = &vm.bx.code.builtins.pod;
                                    if field.ty.self_ref != builtins.pod_f32 {
                                        script_err_pod!(self.trap, "abstract float not compatible with field {:?} (expects {})", field.name, format_pod_type_name(&vm.bx.heap, field.ty.self_ref));
                                    }
                                }
                                _ => {}
                            }
                            if matches!(output.backend, ShaderBackend::Rust) {
                                write!(out, "{}: ", field.name).ok();
                            }
                            out.push_str(&arg.s);
                        } else {
                            script_err_type_mismatch!(
                                self.trap,
                                "missing arg for field {:?}",
                                field.name
                            );
                        }
                    }

                    if args.len() != fields.len() {
                        script_err_invalid_args!(
                            self.trap,
                            "expected {} args, got {}",
                            fields.len(),
                            args.len()
                        );
                    }
                } else {
                    script_err_unexpected!(self.trap, "named args require struct type");
                }
            } else {
                // Positional args
                let pod_name = vm
                    .bx
                    .heap
                    .pod_type_name(pod_ty)
                    .map(|name| output.backend.map_pod_name(name));
                let hlsl_splat_len =
                    if matches!(output.backend, ShaderBackend::Hlsl) && args.len() == 1 {
                        match pod_name {
                            Some(id!(float2)) | Some(id!(half2)) | Some(id!(uint2))
                            | Some(id!(int2)) | Some(id!(bool2)) => Some(2usize),
                            Some(id!(float3)) | Some(id!(half3)) | Some(id!(uint3))
                            | Some(id!(int3)) | Some(id!(bool3)) => Some(3usize),
                            Some(id!(float4)) | Some(id!(half4)) | Some(id!(uint4))
                            | Some(id!(int4)) | Some(id!(bool4)) => Some(4usize),
                            _ => None,
                        }
                    } else {
                        None
                    };

                // For Rust struct types, get field names for positional args
                let rust_struct_fields: Option<Vec<LiveId>> =
                    if matches!(output.backend, ShaderBackend::Rust) {
                        if let ScriptPodTy::Struct { fields, .. } = &pod_ty_data.ty {
                            Some(fields.iter().map(|f| f.name).collect())
                        } else {
                            None
                        }
                    } else {
                        None
                    };

                for (i, arg) in args.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    match &arg.ty {
                        ShaderType::Pod(pod_ty_field) | ShaderType::PodPtr(pod_ty_field) => {
                            vm.bx.heap.pod_check_constructor_arg(
                                pod_ty,
                                *pod_ty_field,
                                &mut offset,
                                self.trap.pass(),
                            );
                        }
                        ShaderType::Id(id) => {
                            if let Some((v, _name)) = self.shader_scope.find_var(*id) {
                                vm.bx.heap.pod_check_constructor_arg(
                                    pod_ty,
                                    v.ty(),
                                    &mut offset,
                                    self.trap.pass(),
                                );
                            } else {
                                script_err_not_found!(
                                    self.trap,
                                    "var {:?} not found in constructor{}",
                                    id,
                                    suggest_from_live_ids(*id, &self.shader_scope.all_var_names())
                                );
                            }
                        }
                        ShaderType::AbstractInt | ShaderType::AbstractFloat => {
                            vm.bx.heap.pod_check_abstract_constructor_arg(
                                pod_ty,
                                &mut offset,
                                self.trap.pass(),
                            );
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

                    if i == 0 {
                        if let Some(n) = hlsl_splat_len {
                            for j in 0..n {
                                if j > 0 {
                                    out.push_str(", ");
                                }
                                out.push_str(&arg.s);
                            }
                            break;
                        }
                    }
                    // For Rust struct types, prefix with field name
                    if let Some(ref fields) = rust_struct_fields {
                        if i < fields.len() {
                            write!(out, "{}: ", fields[i]).ok();
                        }
                    }
                    out.push_str(&arg.s);
                }
                vm.bx
                    .heap
                    .pod_check_constructor_arg_count(pod_ty, &offset, self.trap.pass());
            }
        } else {
            vm.bx
                .heap
                .pod_check_constructor_arg_count(pod_ty, &offset, self.trap.pass());
        }

        match output.backend {
            ShaderBackend::Wgsl => {
                out.push_str(")");
            }
            ShaderBackend::Metal => {
                if let ScriptPodTy::Struct { .. } = &pod_ty_data.ty {
                    out.push_str("}");
                } else {
                    out.push_str(")");
                }
            }
            ShaderBackend::Hlsl => {
                out.push_str(")");
            }
            ShaderBackend::Glsl => {
                out.push_str(")");
            }
            ShaderBackend::Rust => {
                if let ScriptPodTy::Struct { .. } = &pod_ty_data.ty {
                    out.push_str(" }");
                } else {
                    out.push_str(")");
                }
            }
        }

        for arg in args {
            self.stack.free_string(arg.s);
        }

        self.stack
            .push(self.trap.pass(), ShaderType::Pod(pod_ty), out);
    }

    pub fn compile_shader_def(
        vm: &mut ScriptVm,
        output: &mut ShaderOutput,
        trap: ScriptTrap,
        name: LiveId,
        fnobj: ScriptObject,
        sself: ShaderType,
        args: Vec<ShaderType>,
    ) -> (ScriptPodType, String) {
        let mut method_name_prefix = String::new();
        if let ShaderType::PodType(ty) = sself {
            if let Some(name) = vm.bx.heap.pod_type_name(ty) {
                write!(method_name_prefix, "{}_", name).ok();
            }
        } else if let ShaderType::Pod(ty) = sself {
            if let Some(name) = vm.bx.heap.pod_type_name(ty) {
                write!(method_name_prefix, "{}_", name).ok();
            }
        } else if let ShaderType::IoSelf(_) = sself {
            write!(method_name_prefix, "io_").ok();
        } else if let ShaderType::ScopeObject(obj) = sself {
            // Use the object index to create a unique prefix for scope object methods
            write!(method_name_prefix, "scope{}_", obj.index).ok();
        }

        // First pass: resolve AbstractInt/AbstractFloat against declared parameter types
        // Also count expected parameters to validate argument count
        let builtins = &vm.bx.code.builtins.pod;
        let argc = vm.bx.heap.vec_len(fnobj);
        let mut resolved_args: Vec<ScriptPodType> = Vec::new();
        let mut argi = 0;
        let mut expected_param_count = 0;
        for i in 0..argc {
            let kv = vm.bx.heap.vec_key_value(fnobj, i, trap);
            if kv.key == id!(self).into() {
                continue;
            }
            expected_param_count += 1;
            if argi >= args.len() {
                continue; // Keep counting expected params but skip arg processing
            }
            let arg = &args[argi];
            // Get declared parameter type from kv.value
            // Try both direct pod_type value and object-based pod_type
            let declared_ty = kv
                .value
                .as_pod_type()
                .or_else(|| vm.bx.heap.pod_type(kv.value));

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

        // Validate argument count
        if args.len() != expected_param_count {
            output.has_errors = true;
            script_err_invalid_args!(
                trap,
                "function {:?} expects {} argument{}, but {} {} provided",
                name,
                expected_param_count,
                if expected_param_count == 1 { "" } else { "s" },
                args.len(),
                if args.len() == 1 { "was" } else { "were" }
            );
        }

        // lets see if we already have fnobj with our argstypes
        if let Some(fun) = output
            .functions
            .iter()
            .find(|v| v.fnobj == fnobj && v.args == resolved_args)
        {
            let mut fn_name_base = String::new();
            if fun.overload != 0 {
                write!(
                    fn_name_base,
                    "_f{}{}{}",
                    fun.overload, method_name_prefix, name
                )
                .ok();
            } else {
                write!(fn_name_base, "{}{}", method_name_prefix, name).ok();
            }
            let mut fn_name = output.backend.map_function_name(&fn_name_base);
            write!(fn_name, "(").ok(); // Add opening paren to match new function path
            return (fun.ret, fn_name);
        }

        let overload = output.functions.iter().filter(|v| v.name == name).count();

        let mut compiler = ShaderFnCompiler::new(fnobj);
        let mut call_sig = String::new();

        let mut fn_name_base = String::new();
        let mut fn_args = String::new();

        if overload != 0 {
            write!(fn_name_base, "_f{}{}{}", overload, method_name_prefix, name).ok();
        } else {
            write!(fn_name_base, "{}{}", method_name_prefix, name).ok();
        }
        let mut fn_name = output.backend.map_function_name(&fn_name_base);

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
                    if let Some(name) = vm.bx.heap.pod_type_name(ty) {
                        let name = output.backend.map_pod_name(name);
                        write!(fn_args, "{}", name).ok();
                    }
                    write!(fn_args, ">").ok();
                }
                ShaderBackend::Metal => {
                    if let Some(name) = vm.bx.heap.pod_type_name(ty) {
                        let name = output.backend.map_pod_name(name);
                        if fn_args.len() > 0 {
                            write!(fn_args, ", ").ok();
                        }
                        write!(fn_args, "thread {}& _self", name).ok();
                    }
                }
                ShaderBackend::Hlsl => {
                    if let Some(name) = vm.bx.heap.pod_type_name(ty) {
                        let name = output.backend.map_pod_name(name);
                        if fn_args.len() > 0 {
                            write!(fn_args, ", ").ok();
                        }
                        write!(fn_args, "inout {} _self", name).ok();
                    }
                }
                ShaderBackend::Glsl => {
                    if let Some(name) = vm.bx.heap.pod_type_name(ty) {
                        let name = output.backend.map_pod_name(name);
                        if fn_args.len() > 0 {
                            write!(fn_args, ", ").ok();
                        }
                        write!(fn_args, "inout {} _self", name).ok();
                    }
                }
                ShaderBackend::Rust => {
                    if let Some(name) = vm.bx.heap.pod_type_name(ty) {
                        let name = output.backend.map_pod_name(name);
                        if fn_args.len() > 0 {
                            write!(fn_args, ", ").ok();
                        }
                        write!(fn_args, "_self: *mut {}", name).ok();
                    }
                }
            }
            compiler.shader_scope.define_param(id!(self), ty);
        } else if let ShaderType::PodType(ty) = sself {
            compiler.shader_scope.define_pod_type(id!(self), ty);
        } else if let ShaderType::IoSelf(obj) = sself {
            let io_self_decl = output.backend.get_io_self_decl(output.mode);
            if !io_self_decl.is_empty() {
                if fn_args.len() > 0 {
                    write!(fn_args, ", ").ok();
                }
                write!(fn_args, "{}", io_self_decl).ok();
            }
            compiler.shader_scope.define_io_self(obj);
        } else if let ShaderType::ScopeObject(obj) = sself {
            // ScopeObject methods don't have a _self parameter - `self` references
            // are resolved to IoScopeUniform accesses at compile time
            compiler.shader_scope.define_scope_object(obj);
        }

        let argc = vm.bx.heap.vec_len(fnobj);
        let mut argi = 0;
        for i in 0..argc {
            let kv = vm.bx.heap.vec_key_value(fnobj, i, trap);

            if kv.key == id!(self).into() {
                if !has_self || argi != 0 {
                    output.has_errors = true;
                    script_err_not_found!(trap, "self arg must be first with has_self");
                }
                continue;
            }

            if let Some(id) = kv.key.as_id() {
                if fn_args.len() > 0 {
                    write!(fn_args, ", ").ok();
                }
                if argi >= resolved_args.len() {
                    output.has_errors = true;
                    script_err_invalid_args!(trap, "more formal params than resolved args");
                    break;
                }
                let arg_ty = resolved_args[argi];
                let param_shadow = compiler.shader_scope.define_param(id, arg_ty);
                let param_name = output.backend.map_param_name(id, param_shadow);

                match output.backend {
                    ShaderBackend::Wgsl => {
                        write!(fn_args, "{}:", param_name).ok();
                        if let Some(name) = vm.bx.heap.pod_type_name(arg_ty) {
                            let name = output.backend.map_pod_name(name);
                            write!(fn_args, "{}", name).ok();
                        } else {
                            // todo!()
                        }
                    }
                    ShaderBackend::Metal | ShaderBackend::Hlsl | ShaderBackend::Glsl => {
                        if let Some(name) = vm.bx.heap.pod_type_name(arg_ty) {
                            let name = output.backend.map_pod_name(name);
                            write!(fn_args, "{} {}", name, param_name).ok();
                        } else {
                            // todo!()
                        }
                    }
                    ShaderBackend::Rust => {
                        if let Some(name) = vm.bx.heap.pod_type_name(arg_ty) {
                            let name = output.backend.map_pod_name(name);
                            write!(fn_args, "{}: {}", param_name, name).ok();
                        } else {
                            // todo!()
                        }
                    }
                }
            }
            argi += 1;
        }
        if argi < resolved_args.len() {
            output.has_errors = true;
            script_err_invalid_args!(trap, "fewer formal params than resolved args");
        }

        if let Some(fnptr) = vm.bx.heap.as_fn(fnobj) {
            if let ScriptFnPtr::Script(fnip) = fnptr {
                if output.recur_block.iter().any(|v| *v == fnobj) {
                    output.has_errors = true;
                    script_err_not_allowed!(trap, "shader functions cannot recurse");
                    (vm.bx.code.builtins.pod.pod_void, fn_name)
                } else {
                    output.recur_block.push(fnobj);
                    let ret = compiler.compile_fn(vm, output, fnip);
                    output.recur_block.pop();

                    // Ensure struct return types are registered in output.structs
                    if let ScriptPodTy::Struct { .. } = vm.bx.heap.pod_type_ref(ret).ty {
                        output.structs.insert(ret);
                    }

                    match output.backend {
                        ShaderBackend::Wgsl => {
                            write!(call_sig, "fn {}({})", fn_name, fn_args).ok();
                            if let Some(name) = vm.bx.heap.pod_type_name(ret) {
                                if name != id!(void) {
                                    let name = output.backend.map_pod_name(name);
                                    write!(call_sig, "->{}", name).ok();
                                }
                            }
                        }
                        ShaderBackend::Metal | ShaderBackend::Hlsl | ShaderBackend::Glsl => {
                            let ret_name = if let Some(name) = vm.bx.heap.pod_type_name(ret) {
                                output.backend.map_pod_name(name)
                            } else {
                                id!(void)
                            };
                            write!(call_sig, "{} {}({})", ret_name, fn_name, fn_args).ok();
                        }
                        ShaderBackend::Rust => {
                            let ret_name = if let Some(name) = vm.bx.heap.pod_type_name(ret) {
                                output.backend.map_pod_name(name)
                            } else {
                                id!(void)
                            };
                            if ret_name == id!(void) {
                                write!(call_sig, "fn {}({})", fn_name, fn_args).ok();
                            } else {
                                write!(call_sig, "fn {}({}) -> {}", fn_name, fn_args, ret_name)
                                    .ok();
                            }
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
        let arg_types = args.clone();
        let resolved_arg_types =
            Self::resolve_script_call_arg_types(vm, fnobj, &arg_types, self.trap.pass());
        let (ret, fn_name) =
            Self::compile_shader_def(vm, output, self.trap.pass(), name, fnobj, sself, args);
        if matches!(output.backend, ShaderBackend::Glsl | ShaderBackend::Rust) {
            out = Self::glsl_rewrite_call_args(vm, &out, &arg_types, &resolved_arg_types);
        }
        out.insert_str(0, &fn_name);
        out.push_str(")");
        self.stack.push(self.trap.pass(), ShaderType::Pod(ret), out);
    }

    fn resolve_script_call_arg_types(
        vm: &ScriptVm,
        fnobj: ScriptObject,
        args: &[ShaderType],
        trap: ScriptTrap,
    ) -> Vec<ScriptPodType> {
        let builtins = &vm.bx.code.builtins.pod;
        let argc = vm.bx.heap.vec_len(fnobj);
        let mut resolved_args: Vec<ScriptPodType> = Vec::new();
        let mut argi = 0;

        for i in 0..argc {
            let kv = vm.bx.heap.vec_key_value(fnobj, i, trap);
            if kv.key == id!(self).into() {
                continue;
            }
            if argi >= args.len() {
                break;
            }
            let arg = &args[argi];
            let declared_ty = kv
                .value
                .as_pod_type()
                .or_else(|| vm.bx.heap.pod_type(kv.value));

            let resolved = match arg {
                ShaderType::AbstractInt | ShaderType::AbstractFloat => declared_ty
                    .unwrap_or_else(|| arg.make_concrete(builtins).unwrap_or(builtins.pod_void)),
                _ => arg.make_concrete(builtins).unwrap_or(builtins.pod_void),
            };
            resolved_args.push(resolved);
            argi += 1;
        }

        resolved_args
    }

    fn glsl_rewrite_call_args(
        vm: &ScriptVm,
        raw_args: &str,
        arg_types: &[ShaderType],
        resolved_arg_types: &[ScriptPodType],
    ) -> String {
        let mut parts = Self::split_call_args_top_level(raw_args);
        if parts.is_empty() || arg_types.is_empty() || parts.len() < arg_types.len() {
            return raw_args.to_string();
        }

        let explicit_start = parts.len() - arg_types.len();
        let explicit_len = arg_types.len().min(resolved_arg_types.len());
        for i in 0..explicit_len {
            if !matches!(arg_types[i], ShaderType::AbstractInt) {
                continue;
            }
            let resolved_ty = resolved_arg_types[i];
            if !vm.bx.heap.pod_types[resolved_ty.index as usize]
                .ty
                .is_float_type()
            {
                continue;
            }
            let arg_index = explicit_start + i;
            let value = parts[arg_index].trim();
            if Self::is_simple_int_literal(value) {
                parts[arg_index] = format!("{}.0", value);
            }
        }

        parts.join(", ")
    }

    fn split_call_args_top_level(raw_args: &str) -> Vec<String> {
        if raw_args.trim().is_empty() {
            return Vec::new();
        }
        let mut out = Vec::new();
        let mut start = 0usize;
        let mut paren_depth = 0usize;
        let mut bracket_depth = 0usize;
        let mut brace_depth = 0usize;

        for (idx, ch) in raw_args.char_indices() {
            match ch {
                '(' => paren_depth += 1,
                ')' => paren_depth = paren_depth.saturating_sub(1),
                '[' => bracket_depth += 1,
                ']' => bracket_depth = bracket_depth.saturating_sub(1),
                '{' => brace_depth += 1,
                '}' => brace_depth = brace_depth.saturating_sub(1),
                ',' if paren_depth == 0 && bracket_depth == 0 && brace_depth == 0 => {
                    out.push(raw_args[start..idx].trim().to_string());
                    start = idx + ch.len_utf8();
                }
                _ => {}
            }
        }

        if start < raw_args.len() {
            out.push(raw_args[start..].trim().to_string());
        }
        out
    }

    fn is_simple_int_literal(value: &str) -> bool {
        !value.is_empty()
            && value
                .chars()
                .all(|c| c.is_ascii_digit() || c == '-' || c == '+')
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
                | Some(ShaderMe::PodBuiltinMethod { .. })
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
                ShaderMe::ScriptCall {
                    out,
                    name,
                    fnobj,
                    sself,
                    args,
                } => {
                    self.handle_script_call(vm, output, out, name, fnobj, sself, args);
                }
                ShaderMe::TextureBuiltin {
                    method_id,
                    tex_type: _,
                    texture_expr,
                    args,
                } => {
                    self.handle_texture_builtin_exec(vm, output, method_id, texture_expr, args);
                }
                ShaderMe::BuiltinCall {
                    name,
                    fnptr: _,
                    args,
                } => {
                    self.handle_builtin_call(vm, output, name, args);
                }
                ShaderMe::PodBuiltinMethod {
                    name,
                    self_ty: _,
                    args,
                } => {
                    // Pod builtin method: x.mix(y, a) -> mix(x, y, a)
                    // Reuse the same logic as BuiltinCall
                    self.handle_builtin_call(vm, output, name, args);
                }
                _ => {
                    // This case should not be reached due to the guard at the top of handle_call_exec
                    script_err_not_impl!(
                        self.trap,
                        "CALL_EXEC: unexpected call type in shader (internal error)"
                    );
                }
            }
        }
    }

    /// Handle a builtin function call (either from BuiltinCall or PodBuiltinMethod)
    fn handle_builtin_call(
        &mut self,
        vm: &mut ScriptVm,
        output: &mut ShaderOutput,
        name: LiveId,
        args: Vec<(ShaderType, String)>,
    ) {
        let builtins = &vm.bx.code.builtins.pod;

        // Special case: discard() - emits backend-specific discard statement
        if name == id!(discard) {
            for (_, s) in args {
                self.stack.free_string(s);
            }
            let mut out = self.stack.new_string();
            match output.backend {
                // Metal: discard_fragment() is a function call
                ShaderBackend::Metal => write!(out, "discard_fragment()").ok(),
                // GLSL, WGSL, HLSL: discard is a keyword/statement (no parens)
                ShaderBackend::Glsl | ShaderBackend::Wgsl | ShaderBackend::Hlsl => {
                    write!(out, "discard").ok()
                }
                ShaderBackend::Rust => write!(out, "{{ rcx.discard = 1.0; return }}").ok(),
            };
            self.stack
                .push(self.trap.pass(), ShaderType::Pod(builtins.pod_void), out);
            return;
        }

        // Check if any arg is a float type - if so, abstract ints should be floats
        let has_float = args.iter().any(|(ty, _)| match ty {
            ShaderType::Pod(pt) => vm.bx.heap.pod_types[pt.index as usize].ty.is_float_type(),
            ShaderType::AbstractFloat => true,
            _ => false,
        });

        // Build concrete args for type_table_builtin and format output
        let mut concrete_args = Vec::new();
        let mut out = self.stack.new_string();
        let mapped_name = output.backend.map_builtin_name(name);
        let hlsl_ctor_splat_len =
            if matches!(output.backend, ShaderBackend::Hlsl) && args.len() == 1 {
                match mapped_name {
                    id!(float2) | id!(half2) | id!(uint2) | id!(int2) | id!(bool2) => Some(2usize),
                    id!(float3) | id!(half3) | id!(uint3) | id!(int3) | id!(bool3) => Some(3usize),
                    id!(float4) | id!(half4) | id!(uint4) | id!(int4) | id!(bool4) => Some(4usize),
                    _ => None,
                }
            } else {
                None
            };

        // For Rust backend, collect formatted args first so we can suffix the
        // function name with the first-argument type (e.g. clamp_2f, max_2f)
        let mut formatted_args = Vec::new();

        for (i, (ty, s)) in args.into_iter().enumerate() {
            let mut formatted = s.clone();
            match &ty {
                ShaderType::AbstractInt | ShaderType::AbstractFloat => {
                    if has_float {
                        // Format as float literal
                        concrete_args.push(builtins.pod_f32);
                        // Check if s is a simple integer that needs .0 suffix
                        if s.chars().all(|c| c.is_ascii_digit() || c == '-') {
                            formatted.push_str(".0");
                        }
                    } else {
                        concrete_args.push(ty.make_concrete(builtins).unwrap_or(builtins.pod_void));
                    }
                }
                ShaderType::Pod(pt) => {
                    concrete_args.push(*pt);
                }
                _ => {
                    concrete_args.push(ty.make_concrete(builtins).unwrap_or(builtins.pod_void));
                }
            }

            if i == 0 {
                if let Some(n) = hlsl_ctor_splat_len {
                    for _j in 0..n {
                        formatted_args.push(formatted.clone());
                    }
                    self.stack.free_string(s);
                    break;
                }
            }
            formatted_args.push(formatted);
            self.stack.free_string(s);
        }

        // For Rust backend, dFdx/dFdy are emitted as inline record/compute blocks
        // using the 3-pass quad approach. In recording passes (quad_mode 0=dx, 1=dy),
        // the value is stored into quad_dx_buf/quad_dy_buf. In compute pass (mode 2),
        // the stored neighbor value is diffed against the current value.
        if matches!(output.backend, ShaderBackend::Rust)
            && (name == id!(dFdx) || name == id!(dFdy))
            && !formatted_args.is_empty()
        {
            let expr = &formatted_args[0];
            let is_dfdx = name == id!(dFdx);
            // Determine number of f32 slots from the argument type
            let first_ty = if !concrete_args.is_empty() {
                concrete_args[0]
            } else {
                builtins.pod_f32
            };
            let slots = if first_ty == builtins.pod_vec4f {
                4
            } else if first_ty == builtins.pod_vec3f {
                3
            } else if first_ty == builtins.pod_vec2f {
                2
            } else {
                1
            };
            // The compute buffer: dFdx reads from quad_dx_buf, dFdy from quad_dy_buf
            let compute_buf = if is_dfdx {
                "quad_dx_buf"
            } else {
                "quad_dy_buf"
            };
            let lane_field = if is_dfdx {
                "quad_lane_x"
            } else {
                "quad_lane_y"
            };

            if slots == 1 {
                write!(
                    out,
                    "{{ let __s = rcx.quad_slot as usize; rcx.quad_slot = rcx.quad_slot.saturating_add(1); \
                     let __v: f32 = ({expr}); \
                     if __s < rcx.quad_dx_buf.len() {{ \
                        if rcx.quad_mode == 2 {{ let __d = rcx.{compute_buf}[__s] - __v; if rcx.{lane_field} == 0 {{ __d }} else {{ -__d }} }} \
                        else {{ if rcx.quad_mode == 0 {{ rcx.quad_dx_buf[__s] = __v; }} \
                        else {{ rcx.quad_dy_buf[__s] = __v; }}; 0.0f32 }} \
                     }} else {{ 0.0f32 }} }}"
                )
                .ok();
            } else if slots == 2 {
                write!(out,
                    "{{ let __s = rcx.quad_slot as usize; rcx.quad_slot = rcx.quad_slot.saturating_add(2); \
                     let __v: Vec2f = ({expr}); \
                     if __s <= rcx.quad_dx_buf.len().saturating_sub(2) {{ \
                        if rcx.quad_mode == 2 {{ let __d = vec2f(rcx.{compute_buf}[__s] - __v.x, rcx.{compute_buf}[__s+1] - __v.y); if rcx.{lane_field} == 0 {{ __d }} else {{ vec2f(-__d.x, -__d.y) }} }} \
                        else {{ if rcx.quad_mode == 0 {{ rcx.quad_dx_buf[__s] = __v.x; rcx.quad_dx_buf[__s+1] = __v.y; }} \
                        else {{ rcx.quad_dy_buf[__s] = __v.x; rcx.quad_dy_buf[__s+1] = __v.y; }}; vec2f(0.0, 0.0) }} \
                     }} else {{ vec2f(0.0, 0.0) }} }}"
                ).ok();
            } else if slots == 3 {
                write!(out,
                    "{{ let __s = rcx.quad_slot as usize; rcx.quad_slot = rcx.quad_slot.saturating_add(3); \
                     let __v: Vec3f = ({expr}); \
                     if __s <= rcx.quad_dx_buf.len().saturating_sub(3) {{ \
                        if rcx.quad_mode == 2 {{ let __d = vec3f(rcx.{compute_buf}[__s] - __v.x, rcx.{compute_buf}[__s+1] - __v.y, rcx.{compute_buf}[__s+2] - __v.z); if rcx.{lane_field} == 0 {{ __d }} else {{ vec3f(-__d.x, -__d.y, -__d.z) }} }} \
                        else {{ if rcx.quad_mode == 0 {{ rcx.quad_dx_buf[__s] = __v.x; rcx.quad_dx_buf[__s+1] = __v.y; rcx.quad_dx_buf[__s+2] = __v.z; }} \
                        else {{ rcx.quad_dy_buf[__s] = __v.x; rcx.quad_dy_buf[__s+1] = __v.y; rcx.quad_dy_buf[__s+2] = __v.z; }}; vec3f(0.0, 0.0, 0.0) }} \
                     }} else {{ vec3f(0.0, 0.0, 0.0) }} }}"
                ).ok();
            } else {
                write!(out,
                    "{{ let __s = rcx.quad_slot as usize; rcx.quad_slot = rcx.quad_slot.saturating_add(4); \
                     let __v: Vec4f = ({expr}); \
                     if __s <= rcx.quad_dx_buf.len().saturating_sub(4) {{ \
                        if rcx.quad_mode == 2 {{ let __d = vec4f(rcx.{compute_buf}[__s] - __v.x, rcx.{compute_buf}[__s+1] - __v.y, rcx.{compute_buf}[__s+2] - __v.z, rcx.{compute_buf}[__s+3] - __v.w); if rcx.{lane_field} == 0 {{ __d }} else {{ vec4f(-__d.x, -__d.y, -__d.z, -__d.w) }} }} \
                        else {{ if rcx.quad_mode == 0 {{ rcx.quad_dx_buf[__s] = __v.x; rcx.quad_dx_buf[__s+1] = __v.y; rcx.quad_dx_buf[__s+2] = __v.z; rcx.quad_dx_buf[__s+3] = __v.w; }} \
                        else {{ rcx.quad_dy_buf[__s] = __v.x; rcx.quad_dy_buf[__s+1] = __v.y; rcx.quad_dy_buf[__s+2] = __v.z; rcx.quad_dy_buf[__s+3] = __v.w; }}; vec4f(0.0, 0.0, 0.0, 0.0) }} \
                     }} else {{ vec4f(0.0, 0.0, 0.0, 0.0) }} }}"
                ).ok();
            }
        } else {
            // For Rust backend, append a type suffix for overloaded builtins
            if matches!(output.backend, ShaderBackend::Rust) {
                let needs_suffix = matches!(
                    name,
                    id if id == id!(clamp)
                        || id == id!(max)
                        || id == id!(min)
                        || id == id!(abs)
                        || id == id!(length)
                        || id == id!(dot)
                        || id == id!(normalize)
                        || id == id!(distance)
                        || id == id!(floor)
                        || id == id!(ceil)
                        || id == id!(fract)
                        || id == id!(round)
                        || id == id!(sign)
                        || id == id!(sqrt)
                        || id == id!(sin)
                        || id == id!(cos)
                        || id == id!(step)
                        || id == id!(smoothstep)
                );
                if needs_suffix && !concrete_args.is_empty() {
                    let first_ty = concrete_args[0];
                    let suffix = if first_ty == builtins.pod_vec2f {
                        "_2f"
                    } else if first_ty == builtins.pod_vec3f {
                        "_3f"
                    } else if first_ty == builtins.pod_vec4f {
                        "_4f"
                    } else {
                        ""
                    };
                    write!(out, "{}{}(", mapped_name, suffix).ok();
                } else {
                    write!(out, "{}(", mapped_name).ok();
                }
            } else {
                write!(out, "{}(", mapped_name).ok();
            }

            for (i, formatted) in formatted_args.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                out.push_str(formatted);
            }

            out.push_str(")");
        }
        let ret = type_table_builtin(name, &concrete_args, builtins, self.trap.pass());
        self.stack.push(self.trap.pass(), ShaderType::Pod(ret), out);
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
                        write!(
                            s,
                            "float2({}.get_width(), {}.get_height())",
                            texture_expr, texture_expr
                        )
                        .ok();
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
                    ShaderBackend::Rust => {
                        // Rust: texture.size() returns Vec2f
                        write!(
                            s,
                            "vec2({}.width as f32, {}.height as f32)",
                            texture_expr, texture_expr
                        )
                        .ok();
                    }
                }
                self.stack.push(
                    self.trap.pass(),
                    ShaderType::Pod(vm.bx.code.builtins.pod.pod_vec2f),
                    s,
                );
            }
            id!(sample) | id!(sample_as_bgra) => {
                // sample(coord) samples the texture at normalized coordinates.
                // sample_as_bgra(coord) is identical except on WebGL GLSL, where it
                // applies a BGRA->RGBA swizzle in the sampler helper.
                let method_name = if method_id == id!(sample_as_bgra) {
                    "sample_as_bgra"
                } else {
                    "sample"
                };
                if args.len() != 1 {
                    script_err_invalid_args!(self.trap, "texture.{} requires 1 arg", method_name);
                    let empty = self.stack.new_string();
                    self.stack.push(
                        self.trap.pass(),
                        ShaderType::Pod(vm.bx.code.builtins.pod.pod_vec4f),
                        empty,
                    );
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
                            write!(
                                s,
                                "textureSample({}, _s{}, {})",
                                texture_expr, sampler_idx, coord
                            )
                            .ok();
                        }
                        ShaderBackend::Hlsl => {
                            // Use explicit LOD in HLSL so sampling is valid inside dynamic loops.
                            write!(
                                s,
                                "{}.SampleLevel(_s{}, {}, 0.0)",
                                texture_expr, sampler_idx, coord
                            )
                            .ok();
                        }
                        ShaderBackend::Glsl => {
                            // GLSL ES uses runtime-bound sampler state (glBindSampler),
                            // so we sample via helper functions and track texture->sampler
                            // bindings separately in ShaderOutput.
                            output.bind_texture_sampler(&texture_expr, sampler_idx);
                            if method_id == id!(sample_as_bgra) {
                                write!(s, "sample2d_bgra({}, {})", texture_expr, coord).ok();
                            } else {
                                write!(s, "sample2d({}, {})", texture_expr, coord).ok();
                            }
                        }
                        ShaderBackend::Rust => {
                            // Rust headless backend keeps texture data in logical RGBA,
                            // so sample_as_bgra is a no-op alias of sample.
                            write!(s, "{}.sample({})", texture_expr, coord).ok();
                        }
                    }
                    self.stack.push(
                        self.trap.pass(),
                        ShaderType::Pod(vm.bx.code.builtins.pod.pod_vec4f),
                        s,
                    );
                }
            }
            _ => {
                script_err_not_found!(
                    self.trap,
                    "unknown texture method {:?}{}",
                    method_id,
                    suggest_from_live_ids(
                        method_id,
                        &[id!(sample), id!(sample_as_bgra), id!(size)]
                    )
                );
            }
        }
        self.stack.free_string(texture_expr);
        for arg in args {
            self.stack.free_string(arg);
        }
    }

    pub(crate) fn handle_method_call_args(
        &mut self,
        vm: &mut ScriptVm,
        output: &mut ShaderOutput,
        opargs: OpcodeArgs,
    ) {
        let (method_ty, method_s) = self.stack.pop(self.trap.pass());
        let (self_ty, self_s) = self.stack.pop(self.trap.pass());
        self.stack.free_string(method_s);

        if let ShaderType::Id(method_id) = method_ty {
            // Handle method calls on Texture types (e.g., texture.size())
            if let ShaderType::Texture(tex_type) = self_ty {
                self.handle_texture_method_call_args(
                    vm, output, opargs, method_id, tex_type, self_s,
                );
                return;
            }

            // Handle method calls on ScopeTexture types (e.g., scope_texture.sample())
            if let ShaderType::ScopeTexture { tex_type, .. } = self_ty {
                self.handle_texture_method_call_args(
                    vm, output, opargs, method_id, tex_type, self_s,
                );
                return;
            }

            if let ShaderType::Id(self_id) = self_ty {
                // Try to resolve as variable on shader scope - extract info before mutable borrows
                let scope_info = self
                    .shader_scope
                    .find_var(self_id)
                    .map(|(var, _)| match var {
                        ShaderScopeItem::IoSelf(obj) => (Some(*obj), None),
                        _ => (None, Some(var.ty())),
                    });

                if let Some((io_self_obj, pod_ty_opt)) = scope_info {
                    // Method call on IoSelf
                    if let Some(obj) = io_self_obj {
                        if self.handle_io_self_method_call_args(
                            vm, output, opargs, method_id, obj, &self_s,
                        ) {
                            self.stack.free_string(self_s);
                            return;
                        }
                    }

                    // Method call on a Pod instance
                    if let Some(pod_ty) = pod_ty_opt {
                        let self_s_slice = if self_id == id!(self) {
                            // In Rust backend, _self is *mut T (raw pointer)
                            if matches!(output.backend, ShaderBackend::Rust) {
                                "(*_self)"
                            } else {
                                "_self"
                            }
                        } else {
                            &self_s
                        };
                        if self.handle_pod_method_call_args(
                            vm,
                            output,
                            opargs,
                            method_id,
                            pod_ty,
                            self_s_slice,
                            &self_s,
                        ) {
                            return;
                        }
                    }

                    // Method not found on the type
                    self.stack.free_string(self_s);
                    let type_name = if let Some(pod_ty) = pod_ty_opt {
                        vm.bx
                            .heap
                            .pod_type_name(pod_ty)
                            .map(|id| id.as_string(|s| s.unwrap_or("unknown").to_string()))
                            .unwrap_or_else(|| "unknown".to_string())
                    } else {
                        format!("{:?}", self_id)
                    };
                    script_err_not_found!(
                        self.trap,
                        "method {:?} not found on {}",
                        method_id,
                        type_name
                    );
                    return;
                } else {
                    // Try to resolve as PodType static method in script scope
                    if self.handle_pod_type_method_call_args(
                        vm, output, opargs, method_id, self_id, &self_s,
                    ) {
                        return;
                    }

                    // Not a PodType - try as ScopeObject method call
                    if self.handle_scope_object_method_call_by_id(
                        vm, output, opargs, method_id, self_id,
                    ) {
                        self.stack.free_string(self_s);
                        return;
                    }

                    // Try as scope texture method call (e.g., test_tex.sample(...))
                    if self.handle_scope_texture_method_call_by_id(
                        vm, output, opargs, method_id, self_id,
                    ) {
                        self.stack.free_string(self_s);
                        return;
                    }

                    // Nothing matched - variable or type not found
                    self.stack.free_string(self_s);
                    script_err_not_found!(
                        self.trap,
                        "method {:?} not found on {:?}",
                        method_id,
                        self_id
                    );
                    return;
                }
            }

            // self_ty is directly a Pod type (not an Id that resolves to a Pod)
            if let ShaderType::Pod(pod_ty) = self_ty {
                if self.handle_pod_method_call_args(
                    vm,
                    output,
                    opargs,
                    method_id,
                    pod_ty,
                    &self_s.clone(),
                    &self_s,
                ) {
                    return;
                }
                // Method not found on pod type
                let type_name = vm
                    .bx
                    .heap
                    .pod_type_name(pod_ty)
                    .map(|id| id.as_string(|s| s.unwrap_or("unknown").to_string()))
                    .unwrap_or_else(|| "unknown".to_string());
                self.stack.free_string(self_s);
                script_err_not_found!(
                    self.trap,
                    "method {:?} not found on {}",
                    method_id,
                    type_name
                );
                return;
            }

            // self_ty wasn't an Id or Pod - some other type
            let type_name = self.shader_type_to_string(vm, &self_ty);
            self.stack.free_string(self_s);
            script_err_not_found!(
                self.trap,
                "method {:?} not found on {}",
                method_id,
                type_name
            );
            return;
        }

        self.stack.free_string(self_s);
        script_err_not_impl!(
            self.trap,
            "METHOD_CALL_ARGS: method call syntax not valid here"
        );
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
        let fnobj = vm.bx.heap.value(obj, method_id.into(), self.trap.pass());
        if let Some(fnobj) = fnobj.as_object() {
            if let Some(fnptr) = vm.bx.heap.as_fn(fnobj) {
                match fnptr {
                    ScriptFnPtr::Script(_fnptr) => {
                        let mut out = self.stack.new_string();
                        write!(out, "{}", output.backend.get_io_all(output.mode)).ok();
                        let io_self = output.backend.get_io_self(output.mode);
                        if !io_self.is_empty() {
                            if out.len() > 0 {
                                write!(out, ", ").ok();
                            }
                            write!(out, "{}", io_self).ok();
                        }
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
                self.maybe_pop_to_me(vm, output, opargs);
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
        let fnobj = vm.bx.heap.value(obj, method_id.into(), self.trap.pass());
        if let Some(fnobj) = fnobj.as_object() {
            if let Some(fnptr) = vm.bx.heap.as_fn(fnobj) {
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
                        script_err_shader!(
                            self.trap,
                            "native methods not supported on scope objects"
                        );
                        return false;
                    }
                }
                self.maybe_pop_to_me(vm, output, opargs);
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
        let script_value = vm
            .bx
            .heap
            .scope_value(self.script_scope, self_id.into(), NoTrap);
        if script_value.is_nil() || script_value.is_err() {
            return false;
        }

        // Must be an object
        let value_obj = match script_value.as_object() {
            Some(obj) => obj,
            None => return false,
        };

        // Must not be a shader_io type or a function
        if vm.bx.heap.as_shader_io(value_obj).is_some() || vm.bx.heap.as_fn(value_obj).is_some() {
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
        let script_value = vm
            .bx
            .heap
            .scope_value(self.script_scope, self_id.into(), NoTrap);
        if script_value.is_nil() || script_value.is_err() {
            return false;
        }

        // Must be an object
        let value_obj = match script_value.as_object() {
            Some(obj) => obj,
            None => return false,
        };

        // Must be a texture shader_io type
        let io_type = match vm.bx.heap.as_shader_io(value_obj) {
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
            if !output
                .io
                .iter()
                .any(|io| io.name == shader_name && matches!(io.kind, ShaderIoKind::Texture(_)))
            {
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
        let (_, prefix) = output
            .backend
            .get_shader_io_kind_and_prefix(output.mode, io_type);
        match prefix {
            ShaderIoPrefix::Prefix(prefix) => {
                write!(texture_expr, "{}{}", prefix, shader_name).ok()
            }
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
        // First check for known shader builtin methods (mix, clamp, etc.)
        // These translate to builtin shader functions: x.mix(y, a) -> mix(x, y, a)
        if Self::is_pod_builtin_method(method_id) {
            let mut self_arg = self.stack.new_string();
            write!(self_arg, "{}", self_s_slice).ok();
            self.mes.push(ShaderMe::PodBuiltinMethod {
                name: method_id,
                self_ty: pod_ty,
                args: vec![(ShaderType::Pod(pod_ty), self_arg)],
            });
            self.stack.free_string(self_s.clone());
            self.maybe_pop_to_me(vm, output, opargs);
            return true;
        }

        // Look up method on the pod type's object (use NoTrap to avoid error messages)
        let pod_ty_data = &vm.bx.heap.pod_types[pod_ty.index as usize];
        let fnobj = vm
            .bx
            .heap
            .value(pod_ty_data.object, method_id.into(), NoTrap);

        if let Some(fnobj) = fnobj.as_object() {
            if let Some(fnptr) = vm.bx.heap.as_fn(fnobj) {
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
                            ShaderBackend::Rust => {
                                if out.len() > 0 {
                                    write!(out, ", ").ok();
                                }
                                // If self_s_slice is "(*_self)", _self is already *mut T
                                if self_s_slice == "(*_self)" {
                                    write!(out, "_self").ok();
                                } else {
                                    write!(out, "&mut {} as *mut _", self_s_slice).ok();
                                }
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
                self.maybe_pop_to_me(vm, output, opargs);
                return true;
            }
        }

        false
    }

    /// Check if a method name is a known shader builtin that can be called on pod types
    fn is_pod_builtin_method(method_id: LiveId) -> bool {
        method_id == id!(mix)
            || method_id == id!(clamp)
            || method_id == id!(smoothstep)
            || method_id == id!(step)
            || method_id == id!(min)
            || method_id == id!(max)
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
        let value = vm
            .bx
            .heap
            .scope_value(self.script_scope, self_id.into(), self.trap.pass());
        if let Some(pod_ty) = vm.bx.heap.pod_type(value) {
            self.ensure_struct_name(vm, output, pod_ty, self_id);
            // It is a PodType. Look up static method.
            let pod_ty_data = &vm.bx.heap.pod_types[pod_ty.index as usize];
            let fnobj = vm
                .bx
                .heap
                .value(pod_ty_data.object, method_id.into(), self.trap.pass());

            if let Some(fnobj) = fnobj.as_object() {
                if let Some(fnptr) = vm.bx.heap.as_fn(fnobj) {
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
                    self.maybe_pop_to_me(vm, output, opargs);
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

    /// Expand a heterogeneous pod constructor to individual float components for Rust backend.
    /// E.g., `vec4(vec3_expr, f32_expr)` → `vec3_expr.x, vec3_expr.y, vec3_expr.z, f32_expr`
    /// and `vec2(f32_expr)` → `f32_expr, f32_expr` (splat)
    fn rust_expand_pod_construct(
        &self,
        vm: &ScriptVm,
        args: &[ShaderPodArg],
        total_slots: usize,
    ) -> String {
        //let builtins = &vm.bx.code.builtins.pod;
        let mut components = Vec::new();

        for arg in args {
            let arg_slots = match &arg.ty {
                ShaderType::Pod(pt) | ShaderType::PodPtr(pt) => {
                    vm.bx.heap.pod_types[pt.index as usize].ty.slots()
                }
                ShaderType::AbstractInt | ShaderType::AbstractFloat => 1,
                _ => 1,
            };

            match arg_slots {
                1 => {
                    // Scalar: might need .0 suffix for abstract ints
                    let mut s = arg.s.clone();
                    if matches!(arg.ty, ShaderType::AbstractInt) {
                        if s.chars().all(|c| c.is_ascii_digit() || c == '-') {
                            s.push_str(".0");
                        }
                    }
                    components.push(s);
                }
                2 => {
                    components.push(format!("{}.x", arg.s));
                    components.push(format!("{}.y", arg.s));
                }
                3 => {
                    components.push(format!("{}.x", arg.s));
                    components.push(format!("{}.y", arg.s));
                    components.push(format!("{}.z", arg.s));
                }
                4 => {
                    components.push(format!("{}.x", arg.s));
                    components.push(format!("{}.y", arg.s));
                    components.push(format!("{}.z", arg.s));
                    components.push(format!("{}.w", arg.s));
                }
                _ => {
                    components.push(arg.s.clone());
                }
            }
        }

        // Handle splat: if we have fewer components than needed, repeat the last one
        while components.len() < total_slots {
            if let Some(last) = components.last().cloned() {
                components.push(last);
            } else {
                components.push("0.0".to_string());
            }
        }

        components.join(", ")
    }
}

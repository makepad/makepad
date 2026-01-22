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
        let (ty, _s) = self.stack.pop(&self.trap);
        if let ShaderType::Id(name) = ty {
            // Check shader scope for PodType
            if let Some((ShaderScopeItem::PodType { ty, .. }, _)) = self.shader_scope.find_var(name) {
                self.handle_pod_type_call(vm, output, opargs, *ty, name);
                return;
            }

            // alright lets look it up on our script scope
            let value = vm.heap.scope_value(self.script_scope, name.into(), &self.trap);
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
        self.trap.err_not_fn();
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
            self.trap.err_no_matching_shader_type();
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

        self.stack.push(&self.trap, ShaderType::Pod(array_ty), out);
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
            self.trap.err_no_matching_shader_type();
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
                                        self.trap.err_pod_type_not_matching();
                                    }
                                }
                                ShaderType::Id(id) => {
                                    if let Some((v, _name)) = self.shader_scope.find_var(*id) {
                                        if v.ty() != field.ty.self_ref {
                                            self.trap.err_pod_type_not_matching();
                                        }
                                    } else {
                                        self.trap.err_not_found();
                                    }
                                }
                                ShaderType::AbstractInt => {
                                    let builtins = &vm.code.builtins.pod;
                                    if field.ty.self_ref != builtins.pod_i32
                                        && field.ty.self_ref != builtins.pod_u32
                                        && field.ty.self_ref != builtins.pod_f32
                                    {
                                        self.trap.err_pod_type_not_matching();
                                    }
                                }
                                ShaderType::AbstractFloat => {
                                    let builtins = &vm.code.builtins.pod;
                                    if field.ty.self_ref != builtins.pod_f32 {
                                        self.trap.err_pod_type_not_matching();
                                    }
                                }
                                _ => {}
                            }
                            out.push_str(&arg.s);
                        } else {
                            self.trap.err_invalid_constructor_arg();
                        }
                    }

                    if args.len() != fields.len() {
                        self.trap.err_invalid_arg_count();
                    }
                } else {
                    self.trap.err_unexpected();
                }
            } else {
                // Positional args
                for (i, arg) in args.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    match &arg.ty {
                        ShaderType::Pod(pod_ty_field) | ShaderType::PodPtr(pod_ty_field) => {
                            vm.heap.pod_check_constructor_arg(pod_ty, *pod_ty_field, &mut offset, &self.trap);
                        }
                        ShaderType::Id(id) => {
                            if let Some((v, _name)) = self.shader_scope.find_var(*id) {
                                vm.heap.pod_check_constructor_arg(pod_ty, v.ty(), &mut offset, &self.trap);
                            } else {
                                self.trap.err_not_found();
                            }
                        }
                        ShaderType::AbstractInt | ShaderType::AbstractFloat => {
                            vm.heap.pod_check_abstract_constructor_arg(pod_ty, &mut offset, &self.trap);
                        }
                        ShaderType::None
                        | ShaderType::Range { .. }
                        | ShaderType::Error(_)
                        | ShaderType::IoSelf(_)
                        | ShaderType::PodType(_)
                        | ShaderType::Texture(_) => {}
                    }
                    out.push_str(&arg.s);
                }
                vm.heap.pod_check_constructor_arg_count(pod_ty, &offset, &self.trap);
            }
        } else {
            vm.heap.pod_check_constructor_arg_count(pod_ty, &offset, &self.trap);
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

        self.stack.push(&self.trap, ShaderType::Pod(pod_ty), out);
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
        }

        // First pass: resolve AbstractInt/AbstractFloat against declared parameter types
        let builtins = &vm.code.builtins.pod;
        let argc = vm.heap.vec_len(fnobj);
        let mut resolved_args: Vec<ScriptPodType> = Vec::new();
        let mut argi = 0;
        for i in 0..argc {
            let kv = vm.heap.vec_key_value(fnobj, i, &vm.thread.trap);
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
        }

        let argc = vm.heap.vec_len(fnobj);
        let mut argi = 0;
        for i in 0..argc {
            let kv = vm.heap.vec_key_value(fnobj, i, &vm.thread.trap);

            if kv.key == id!(self).into() {
                if !has_self || argi != 0 {
                    vm.thread.trap.err_invalid_arg_name();
                }
                continue;
            }

            if let Some(id) = kv.key.as_id() {
                if fn_args.len() > 0 {
                    write!(fn_args, ", ").ok();
                }
                if argi >= resolved_args.len() {
                    vm.thread.trap.err_invalid_arg_count();
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
            vm.thread.trap.err_invalid_arg_count();
        }

        if let Some(fnptr) = vm.heap.as_fn(fnobj) {
            if let ScriptFnPtr::Script(fnip) = fnptr {
                if output.recur_block.iter().any(|v| *v == fnobj) {
                    vm.thread.trap.err_recursion_not_allowed();
                    (vm.code.builtins.pod.pod_void, fn_name)
                } else {
                    output.recur_block.push(fnobj);
                    let ret = compiler.compile_fn(vm, output, fnip);
                    output.recur_block.pop();

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
        self.stack.push(&self.trap, ShaderType::Pod(ret), out);
    }

    pub(crate) fn handle_call_exec(&mut self, vm: &mut ScriptVm, output: &mut ShaderOutput) {
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

                    // Helper to check if a pod type is float-based
                    let is_float_type = |pt: ScriptPodType| -> bool {
                        let pod_ty = &vm.heap.pod_types[pt.index as usize];
                        match &pod_ty.ty {
                            ScriptPodTy::F32 | ScriptPodTy::F16 => true,
                            ScriptPodTy::Vec(v) => matches!(v.elem_ty(), ScriptPodTy::F32 | ScriptPodTy::F16),
                            ScriptPodTy::Mat(_) => true, // Matrices are float-based
                            _ => false,
                        }
                    };

                    // Check if any arg is a float type - if so, abstract ints should be floats
                    let has_float = args.iter().any(|(ty, _)| match ty {
                        ShaderType::Pod(pt) => is_float_type(*pt),
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
                    let ret = type_table_builtin(name, &concrete_args, builtins, &self.trap);
                    self.stack.push(&self.trap, ShaderType::Pod(ret), out);
                }
                _ => {
                    self.trap.err_not_impl();
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
                        // WGSL: textureDimensions(texture) - returns vec2<u32>, cast to vec2<f32>
                        write!(s, "vec2f(0.0, 0.0)").ok(); // Placeholder
                    }
                    ShaderBackend::Hlsl => {
                        // HLSL: texture.GetDimensions()
                        write!(s, "float2(0.0, 0.0)").ok(); // Placeholder
                    }
                    ShaderBackend::Glsl => {
                        // GLSL: textureSize(texture, 0)
                        write!(s, "vec2(0.0, 0.0)").ok(); // Placeholder
                    }
                }
                self.stack.push(&self.trap, ShaderType::Pod(vm.code.builtins.pod.pod_vec2f), s);
            }
            id!(sample_2d) => {
                // sample_2d(coord) samples the texture at normalized coordinates
                // Returns vec4f
                if args.len() != 1 {
                    self.trap.err_invalid_arg_count();
                    let empty = self.stack.new_string();
                    self.stack.push(&self.trap, ShaderType::Pod(vm.code.builtins.pod.pod_vec4f), empty);
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
                            write!(s, "vec4f(0.0, 0.0, 0.0, 0.0)").ok(); // Placeholder
                        }
                        ShaderBackend::Hlsl => {
                            // HLSL: texture.Sample(sampler, coord)
                            write!(s, "float4(0.0, 0.0, 0.0, 0.0)").ok(); // Placeholder
                        }
                        ShaderBackend::Glsl => {
                            // GLSL: texture(sampler2D, coord)
                            write!(s, "vec4(0.0, 0.0, 0.0, 0.0)").ok(); // Placeholder
                        }
                    }
                    self.stack.push(&self.trap, ShaderType::Pod(vm.code.builtins.pod.pod_vec4f), s);
                }
            }
            _ => {
                self.trap.err_not_found();
            }
        }
        self.stack.free_string(texture_expr);
        for arg in args {
            self.stack.free_string(arg);
        }
    }

    pub(crate) fn handle_method_call_args(&mut self, vm: &mut ScriptVm, output: &mut ShaderOutput, opargs: OpcodeArgs) {
        let (method_ty, method_s) = self.stack.pop(&self.trap);
        let (self_ty, self_s) = self.stack.pop(&self.trap);
        self.stack.free_string(method_s);

        if let ShaderType::Id(method_id) = method_ty {
            // Handle method calls on Texture types (e.g., texture.size())
            if let ShaderType::Texture(tex_type) = self_ty {
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
                } else {
                    // Try to resolve as PodType static method in script scope
                    if self.handle_pod_type_method_call_args(vm, output, opargs, method_id, self_id, &self_s) {
                        return;
                    }
                }
            }
        }
        self.stack.free_string(self_s);
        self.trap.err_not_impl();
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
        let fnobj = vm.heap.value(obj, method_id.into(), &self.trap);
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
        let fnobj = vm.heap.value(pod_ty_data.object, method_id.into(), &self.trap);

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
        let value = vm.heap.scope_value(self.script_scope, self_id.into(), &self.trap);
        if let Some(pod_ty) = vm.heap.pod_type(value) {
            self.ensure_struct_name(vm, output, pod_ty, self_id);
            // It is a PodType. Look up static method.
            let pod_ty_data = &vm.heap.pod_types[pod_ty.index as usize];
            let fnobj = vm.heap.value(pod_ty_data.object, method_id.into(), &self.trap);

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

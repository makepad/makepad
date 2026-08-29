use crate::heap::*;
use crate::native::*;
use crate::shader::*;
use crate::shader_backend::*;
use crate::trap::NoTrap;
use crate::value::*;
#[allow(unused)]
use crate::*;
use makepad_live_id::*;

#[derive(PartialEq, Eq, Clone, Copy, Debug)]
pub struct ShaderIoType(pub(crate) u32);

pub const SHADER_IO_RUST_INSTANCE: ShaderIoType = ShaderIoType(0);
pub const SHADER_IO_DYN_INSTANCE: ShaderIoType = ShaderIoType(1);
pub const SHADER_IO_DYN_UNIFORM: ShaderIoType = ShaderIoType(2);
pub const SHADER_IO_UNIFORM_BUFFER: ShaderIoType = ShaderIoType(3);
pub const SHADER_IO_VERTEX_BUFFER: ShaderIoType = ShaderIoType(4);
pub const SHADER_IO_VARYING: ShaderIoType = ShaderIoType(5);
pub const SHADER_IO_VERTEX_POSITION: ShaderIoType = ShaderIoType(6);
pub const SHADER_IO_TEXTURE_1D: ShaderIoType = ShaderIoType(7);
pub const SHADER_IO_TEXTURE_VIDEO: ShaderIoType = ShaderIoType(8);
pub const SHADER_IO_TEXTURE_1D_ARRAY: ShaderIoType = ShaderIoType(9);
pub const SHADER_IO_TEXTURE_2D: ShaderIoType = ShaderIoType(10);
pub const SHADER_IO_TEXTURE_2D_ARRAY: ShaderIoType = ShaderIoType(11);
pub const SHADER_IO_TEXTURE_3D: ShaderIoType = ShaderIoType(12);
pub const SHADER_IO_TEXTURE_3D_ARRAY: ShaderIoType = ShaderIoType(13);
pub const SHADER_IO_TEXTURE_CUBE: ShaderIoType = ShaderIoType(14);
pub const SHADER_IO_TEXTURE_CUBE_ARRAY: ShaderIoType = ShaderIoType(15);
pub const SHADER_IO_TEXTURE_DEPTH: ShaderIoType = ShaderIoType(16);
pub const SHADER_IO_TEXTURE_DEPTH_ARRAY: ShaderIoType = ShaderIoType(17);
pub const SHADER_IO_SAMPLER: ShaderIoType = ShaderIoType(18);
pub const SHADER_IO_BUFFER_R: ShaderIoType = ShaderIoType(19);
pub const SHADER_IO_BUFFER_W: ShaderIoType = ShaderIoType(20);
pub const SHADER_IO_BUFFER_RW: ShaderIoType = ShaderIoType(21);
pub const SHADER_IO_SCOPE_UNIFORM: ShaderIoType = ShaderIoType(22);
pub const SHADER_IO_FRAGMENT_OUTPUT_0: ShaderIoType = ShaderIoType(23);
pub const SHADER_IO_FRAGMENT_OUTPUT_MAX: ShaderIoType =
    ShaderIoType(SHADER_IO_FRAGMENT_OUTPUT_0.0 + 7);

pub fn define_shader_module(heap: &mut ScriptHeap, native: &mut ScriptNative) {
    let shader = heap.new_module(id!(shader));

    native.add_method(
        heap,
        shader,
        id_lut!(depth_clip),
        script_args!(world = NIL, color = NIL, clip = 0.0),
        |vm, args| {
            vm.bx
                .heap
                .value(args, id!(color).into(), vm.bx.threads.cur_ref().trap.pass())
        },
    );

    native.add_method(
        heap,
        shader,
        id_lut!(instance),
        script_args!(value = NIL),
        |vm, args| {
            let value = script_value!(vm, args.value);
            let obj = vm.bx.heap.new_with_proto(value);
            vm.bx.heap.set_shader_io(obj, SHADER_IO_DYN_INSTANCE);
            obj.into()
        },
    );

    native.add_method(
        heap,
        shader,
        id_lut!(uniform),
        script_args!(value = NIL),
        |vm, args| {
            let value = script_value!(vm, args.value);
            let obj = vm.bx.heap.new_with_proto(value);
            vm.bx.heap.set_shader_io(obj, SHADER_IO_DYN_UNIFORM);
            obj.into()
        },
    );

    native.add_method(
        heap,
        shader,
        id_lut!(uniform_buffer),
        script_args!(value = NIL),
        |vm, args| {
            let value = script_value!(vm, args.value);
            let obj = vm.bx.heap.new_with_proto(value);
            vm.bx.heap.set_shader_io(obj, SHADER_IO_UNIFORM_BUFFER);
            obj.into()
        },
    );

    native.add_method(
        heap,
        shader,
        id_lut!(vertex_buffer),
        script_args!(value = NIL, buf = NIL),
        |vm, args| {
            let value = script_value!(vm, args.value);
            let buffer = script_value!(vm, args.buf);
            let obj = vm.bx.heap.new_with_proto(value);
            set_script_value!(vm, obj.buffer = buffer);
            vm.bx.heap.set_shader_io(obj, SHADER_IO_VERTEX_BUFFER);
            obj.into()
        },
    );

    native.add_method(
        heap,
        shader,
        id_lut!(varying),
        script_args!(value = NIL),
        |vm, args| {
            let value = script_value!(vm, args.value);
            let obj = vm.bx.heap.new_with_proto(value);
            vm.bx.heap.set_shader_io(obj, SHADER_IO_VARYING);
            obj.into()
        },
    );

    native.add_method(
        heap,
        shader,
        id_lut!(vertex_position),
        script_args!(value = NIL),
        |vm, args| {
            let value = script_value!(vm, args.value);
            let obj = vm.bx.heap.new_with_proto(value);
            vm.bx.heap.set_shader_io(obj, SHADER_IO_VERTEX_POSITION);
            obj.into()
        },
    );

    native.add_method(
        heap,
        shader,
        id_lut!(texture_1d),
        script_args!(value = NIL),
        |vm, args| {
            let value = script_value!(vm, args.value);
            let obj = vm.bx.heap.new_with_proto(value);
            vm.bx.heap.set_shader_io(obj, SHADER_IO_TEXTURE_1D);
            obj.into()
        },
    );

    native.add_method(
        heap,
        shader,
        id_lut!(texture_1d_array),
        script_args!(value = NIL),
        |vm, args| {
            let value = script_value!(vm, args.value);
            let obj = vm.bx.heap.new_with_proto(value);
            vm.bx.heap.set_shader_io(obj, SHADER_IO_TEXTURE_1D_ARRAY);
            obj.into()
        },
    );

    native.add_method(
        heap,
        shader,
        id_lut!(texture_2d),
        script_args!(value = NIL),
        |vm, args| {
            let value = script_value!(vm, args.value);
            let obj = vm.bx.heap.new_with_proto(value);
            vm.bx.heap.set_shader_io(obj, SHADER_IO_TEXTURE_2D);
            obj.into()
        },
    );

    native.add_method(
        heap,
        shader,
        id_lut!(texture_2d_array),
        script_args!(value = NIL),
        |vm, args| {
            let value = script_value!(vm, args.value);
            let obj = vm.bx.heap.new_with_proto(value);
            vm.bx.heap.set_shader_io(obj, SHADER_IO_TEXTURE_2D_ARRAY);
            obj.into()
        },
    );

    native.add_method(
        heap,
        shader,
        id_lut!(texture_3d),
        script_args!(value = NIL),
        |vm, args| {
            let value = script_value!(vm, args.value);
            let obj = vm.bx.heap.new_with_proto(value);
            vm.bx.heap.set_shader_io(obj, SHADER_IO_TEXTURE_3D);
            obj.into()
        },
    );

    native.add_method(
        heap,
        shader,
        id_lut!(texture_3d_array),
        script_args!(value = NIL),
        |vm, args| {
            let value = script_value!(vm, args.value);
            let obj = vm.bx.heap.new_with_proto(value);
            vm.bx.heap.set_shader_io(obj, SHADER_IO_TEXTURE_3D_ARRAY);
            obj.into()
        },
    );

    native.add_method(
        heap,
        shader,
        id_lut!(texture_cube),
        script_args!(value = NIL),
        |vm, args| {
            let value = script_value!(vm, args.value);
            let obj = vm.bx.heap.new_with_proto(value);
            vm.bx.heap.set_shader_io(obj, SHADER_IO_TEXTURE_CUBE);
            obj.into()
        },
    );

    native.add_method(
        heap,
        shader,
        id_lut!(texture_cube_array),
        script_args!(value = NIL),
        |vm, args| {
            let value = script_value!(vm, args.value);
            let obj = vm.bx.heap.new_with_proto(value);
            vm.bx.heap.set_shader_io(obj, SHADER_IO_TEXTURE_CUBE_ARRAY);
            obj.into()
        },
    );

    native.add_method(
        heap,
        shader,
        id_lut!(texture_depth),
        script_args!(value = NIL),
        |vm, args| {
            let value = script_value!(vm, args.value);
            let obj = vm.bx.heap.new_with_proto(value);
            vm.bx.heap.set_shader_io(obj, SHADER_IO_TEXTURE_DEPTH);
            obj.into()
        },
    );

    native.add_method(
        heap,
        shader,
        id_lut!(texture_depth_array),
        script_args!(value = NIL),
        |vm, args| {
            let value = script_value!(vm, args.value);
            let obj = vm.bx.heap.new_with_proto(value);
            vm.bx.heap.set_shader_io(obj, SHADER_IO_TEXTURE_DEPTH_ARRAY);
            obj.into()
        },
    );

    native.add_method(
        heap,
        shader,
        id_lut!(texture_video),
        script_args!(value = NIL),
        |vm, args| {
            let value = script_value!(vm, args.value);
            let obj = vm.bx.heap.new_with_proto(value);
            vm.bx.heap.set_shader_io(obj, SHADER_IO_TEXTURE_VIDEO);
            obj.into()
        },
    );

    native.add_method(
        heap,
        shader,
        id_lut!(fragment_output),
        script_args!(index = NIL, ty = NIL),
        |vm, args| {
            let index = script_value!(vm, args.index);
            let ty = script_value!(vm, args.ty);
            let obj = vm.bx.heap.new_with_proto(ty);
            let index = index.as_index().min(7) as u32;
            vm.bx
                .heap
                .set_shader_io(obj, ShaderIoType(SHADER_IO_FRAGMENT_OUTPUT_0.0 + index));
            obj.into()
        },
    );

    native.add_method(
        heap,
        shader,
        id_lut!(test_compile_draw),
        script_args!(io_self = NIL),
        |vm, args| {
            // lets fetch the code
            let io_self = script_value!(vm, args.io_self);

            // ok we're going to take a function, and then call it to generate/typetrace it out
            // for every function we make a 'shadercompiler'
            if let Some(io_self) = io_self.as_object() {
                let mut output = ShaderOutput::default();
                output.backend = ShaderBackend::Metal;
                output.use_vulkan = false;

                output.pre_collect_rust_instance_io(vm, io_self);
                output.pre_collect_shader_io(vm, io_self);

                if let Some(fnobj) = vm
                    .bx
                    .heap
                    .object_method(
                        io_self,
                        id!(vertex).into(),
                        vm.bx.threads.cur_ref().trap.pass(),
                    )
                    .as_object()
                {
                    output.mode = ShaderMode::Vertex;
                    // Entry point shaders don't have script-level arguments to validate, use NoTrap
                    ShaderFnCompiler::compile_shader_def(
                        vm,
                        &mut output,
                        NoTrap,
                        id!(vertex),
                        fnobj,
                        ShaderType::IoSelf(io_self),
                        vec![],
                    );
                }
                if let Some(fnobj) = vm
                    .bx
                    .heap
                    .object_method(
                        io_self,
                        id!(fragment).into(),
                        vm.bx.threads.cur_ref().trap.pass(),
                    )
                    .as_object()
                {
                    output.mode = ShaderMode::Fragment;
                    // Entry point shaders don't have script-level arguments to validate, use NoTrap
                    ShaderFnCompiler::compile_shader_def(
                        vm,
                        &mut output,
                        NoTrap,
                        id!(fragment),
                        fnobj,
                        ShaderType::IoSelf(io_self),
                        vec![],
                    );
                }

                output.assign_uniform_buffer_indices(&vm.bx.heap, 3);

                let mut out = String::new();
                output.create_struct_defs(vm, &mut out);
                output.metal_create_instance_struct(vm, &mut out);
                output.metal_create_uniform_struct(vm, &mut out);
                output.metal_create_scope_uniform_struct(vm, &mut out);
                output.metal_create_io_struct(vm, &mut out);
                output.metal_create_varying_struct(vm, &mut out);
                output.metal_create_vertex_buffer_struct(vm, &mut out);
                output.metal_create_sampler_decls(&mut out);
                output.metal_create_helpers(&mut out);
                output.metal_create_io_vertex_struct(vm, &mut out);
                output.metal_create_vertex_fn(vm, &mut out);
                output.metal_create_io_fragment_struct(vm, &mut out);
                /*
                output.metal_create_fragment_main_fn(vm, &mut out);
                println!("Structs:\n{}", out);
                for fns in &output.functions{
                    println!("{}{{\n{}}}\n",fns.call_sig, fns.out);
                }

                // Print scope uniforms for debugging
                if !output.scope_uniforms.is_empty() {
                    println!("\nScope Uniforms ({} entries):", output.scope_uniforms.len());
                    for su in &output.scope_uniforms {
                        println!("  - source_obj: {}, key: {}, shader_name: {}",
                            su.source_obj.index, su.key, su.shader_name);
                    }
                }*/

                return NIL;
            }
            // trap error
            NIL
        },
    );

    native.add_method(
        heap,
        shader,
        id_lut!(test_compile_draw_contains),
        script_args!(io_self = NIL, needle = NIL),
        |vm, args| {
            let io_self = script_value!(vm, args.io_self);
            let needle_val = script_value!(vm, args.needle);
            let Some(needle) = vm.bx.heap.string_with(needle_val, |_heap, s| s.to_string()) else {
                return ScriptValue::from_bool(false);
            };

            if let Some(io_self) = io_self.as_object() {
                let mut output = ShaderOutput::default();
                output.backend = ShaderBackend::Metal;
                output.use_vulkan = false;

                output.pre_collect_rust_instance_io(vm, io_self);
                output.pre_collect_shader_io(vm, io_self);

                if let Some(fnobj) = vm
                    .bx
                    .heap
                    .object_method(
                        io_self,
                        id!(vertex).into(),
                        vm.bx.threads.cur_ref().trap.pass(),
                    )
                    .as_object()
                {
                    output.mode = ShaderMode::Vertex;
                    ShaderFnCompiler::compile_shader_def(
                        vm,
                        &mut output,
                        NoTrap,
                        id!(vertex),
                        fnobj,
                        ShaderType::IoSelf(io_self),
                        vec![],
                    );
                }
                if let Some(fnobj) = vm
                    .bx
                    .heap
                    .object_method(
                        io_self,
                        id!(fragment).into(),
                        vm.bx.threads.cur_ref().trap.pass(),
                    )
                    .as_object()
                {
                    output.mode = ShaderMode::Fragment;
                    ShaderFnCompiler::compile_shader_def(
                        vm,
                        &mut output,
                        NoTrap,
                        id!(fragment),
                        fnobj,
                        ShaderType::IoSelf(io_self),
                        vec![],
                    );
                }

                output.assign_uniform_buffer_indices(&vm.bx.heap, 3);

                let mut out = String::new();
                output.create_struct_defs(vm, &mut out);
                output.metal_create_instance_struct(vm, &mut out);
                output.metal_create_uniform_struct(vm, &mut out);
                output.metal_create_scope_uniform_struct(vm, &mut out);
                output.metal_create_io_struct(vm, &mut out);
                output.metal_create_varying_struct(vm, &mut out);
                output.metal_create_vertex_buffer_struct(vm, &mut out);
                output.metal_create_sampler_decls(&mut out);
                output.metal_create_helpers(&mut out);
                output.create_functions(&mut out);
                output.metal_create_io_vertex_struct(vm, &mut out);
                output.metal_create_io_framebuffer_struct(vm, &mut out);
                output.metal_create_io_fragment_struct(vm, &mut out);
                output.metal_create_vertex_fn(vm, &mut out);
                output.metal_create_fragment_main_fn(vm, &mut out);

                return ScriptValue::from_bool(out.contains(&needle));
            }

            ScriptValue::from_bool(false)
        },
    );

    native.add_method(
        heap,
        shader,
        id_lut!(test_compile_draw_errors),
        script_args!(io_self = NIL),
        |vm, args| {
            let io_self = script_value!(vm, args.io_self);

            if let Some(io_self) = io_self.as_object() {
                let mut output = ShaderOutput::default();
                output.backend = ShaderBackend::Metal;
                output.use_vulkan = false;

                output.pre_collect_rust_instance_io(vm, io_self);
                output.pre_collect_shader_io(vm, io_self);

                if let Some(fnobj) = vm
                    .bx
                    .heap
                    .object_method(
                        io_self,
                        id!(vertex).into(),
                        vm.bx.threads.cur_ref().trap.pass(),
                    )
                    .as_object()
                {
                    output.mode = ShaderMode::Vertex;
                    ShaderFnCompiler::compile_shader_def(
                        vm,
                        &mut output,
                        NoTrap,
                        id!(vertex),
                        fnobj,
                        ShaderType::IoSelf(io_self),
                        vec![],
                    );
                }
                if let Some(fnobj) = vm
                    .bx
                    .heap
                    .object_method(
                        io_self,
                        id!(fragment).into(),
                        vm.bx.threads.cur_ref().trap.pass(),
                    )
                    .as_object()
                {
                    output.mode = ShaderMode::Fragment;
                    ShaderFnCompiler::compile_shader_def(
                        vm,
                        &mut output,
                        NoTrap,
                        id!(fragment),
                        fnobj,
                        ShaderType::IoSelf(io_self),
                        vec![],
                    );
                }

                // Contract under test: every compile failure must surface a
                // human-readable message — a silently failing shader (gray
                // fallback, no log) is itself the bug.
                if output.has_errors {
                    return output.error_report().script_to_value(vm);
                }
                return "".to_string().script_to_value(vm);
            }

            "no shader object".to_string().script_to_value(vm)
        },
    );

    // Test seam for the constant table: compile a draw shader for a named
    // backend with the table on or off and return the emitted source (the
    // scope-uniform declaration plus every function body), so a test can
    // assert exactly what the literal became.
    native.add_method(
        heap,
        shader,
        id_lut!(test_compile_draw_source),
        script_args!(io_self = NIL, backend = NIL, const_table = NIL),
        |vm, args| {
            let io_self = script_value!(vm, args.io_self);
            let backend_val = script_value!(vm, args.backend);
            let const_table = script_value!(vm, args.const_table)
                .as_bool()
                .unwrap_or(false);
            let backend_name = vm
                .bx
                .heap
                .string_with(backend_val, |_heap, s| s.to_string())
                .unwrap_or_default();
            let backend = match backend_name.as_str() {
                "metal" => ShaderBackend::Metal,
                "hlsl" => ShaderBackend::Hlsl,
                "glsl" => ShaderBackend::Glsl,
                "wgsl" => ShaderBackend::Wgsl,
                other => {
                    return format!("unknown backend {}", other).script_to_value(vm);
                }
            };
            let Some(io_self) = io_self.as_object() else {
                return "no shader object".to_string().script_to_value(vm);
            };
            let mut output = ShaderOutput::default();
            output.backend = backend;
            output.use_vulkan = false;
            output.const_table = const_table;
            output.pre_collect_rust_instance_io(vm, io_self);
            output.pre_collect_shader_io(vm, io_self);
            for (entry, mode) in [(id!(vertex), ShaderMode::Vertex), (id!(fragment), ShaderMode::Fragment)] {
                if let Some(fnobj) = vm
                    .bx
                    .heap
                    .object_method(io_self, entry.into(), vm.bx.threads.cur_ref().trap.pass())
                    .as_object()
                {
                    output.mode = mode;
                    ShaderFnCompiler::compile_shader_def(
                        vm,
                        &mut output,
                        NoTrap,
                        entry,
                        fnobj,
                        ShaderType::IoSelf(io_self),
                        vec![],
                    );
                }
            }
            if output.has_errors {
                return format!("ERRORS: {}", output.error_report()).script_to_value(vm);
            }
            output.assign_uniform_buffer_indices(&vm.bx.heap, 3);
            let mut out = String::new();
            match backend {
                ShaderBackend::Metal => {
                    output.create_struct_defs(vm, &mut out);
                    output.metal_create_scope_uniform_struct(vm, &mut out);
                }
                ShaderBackend::Hlsl => {
                    output.hlsl_create_scope_uniform_cbuffer(vm, &mut out);
                }
                ShaderBackend::Glsl => {
                    let mut shared_defs = String::new();
                    output.create_struct_defs(vm, &mut shared_defs);
                    output.glsl_create_fragment_shader(vm, &shared_defs, &mut out);
                }
                ShaderBackend::Wgsl => {
                    match crate::shader_wgsl::compile_draw_shader_wgsl_source(vm, io_self, &output, false) {
                        Ok(src) => out.push_str(&src.wgsl),
                        Err(e) => return format!("ERRORS: {}", e).script_to_value(vm),
                    }
                }
                _ => {}
            }
            for f in &output.functions {
                out.push_str(&f.out);
                out.push('\n');
            }
            out.script_to_value(vm)
        },
    );

    // Test seam: the constant table a draw shader compiles to (Metal, table
    // on), one line per entry: `name|doc|value|file:line:col`.
    native.add_method(
        heap,
        shader,
        id_lut!(test_compile_draw_const_table),
        script_args!(io_self = NIL),
        |vm, args| {
            let io_self = script_value!(vm, args.io_self);
            let Some(io_self) = io_self.as_object() else {
                return "no shader object".to_string().script_to_value(vm);
            };
            let mut output = ShaderOutput::default();
            output.backend = ShaderBackend::Metal;
            output.use_vulkan = false;
            output.const_table = true;
            output.pre_collect_rust_instance_io(vm, io_self);
            output.pre_collect_shader_io(vm, io_self);
            for (entry, mode) in [(id!(vertex), ShaderMode::Vertex), (id!(fragment), ShaderMode::Fragment)] {
                if let Some(fnobj) = vm
                    .bx
                    .heap
                    .object_method(io_self, entry.into(), vm.bx.threads.cur_ref().trap.pass())
                    .as_object()
                {
                    output.mode = mode;
                    ShaderFnCompiler::compile_shader_def(
                        vm,
                        &mut output,
                        NoTrap,
                        entry,
                        fnobj,
                        ShaderType::IoSelf(io_self),
                        vec![],
                    );
                }
            }
            if output.has_errors {
                return format!("ERRORS: {}", output.error_report()).script_to_value(vm);
            }
            let mut out = String::new();
            for tc in &output.table_consts {
                let loc = vm
                    .bx
                    .code
                    .ip_to_loc(tc.ip)
                    .map(|l| format!("{}:{}:{}", l.file, l.line, l.col))
                    .unwrap_or_default();
                // raw row/col of the literal's own token plus the mod line,
                // so a test can pin the line convention exactly
                let raw = {
                    let bodies = vm.bx.code.bodies.borrow();
                    let body = &bodies[tc.ip.body as usize];
                    let tok = body.parser.source_map.get(tc.ip.index as usize).copied().flatten();
                    let rc = tok.and_then(|t| body.tokenizer.token_index_to_row_col(t));
                    let mod_line = match &body.source {
                        crate::vm::ScriptSource::Mod(m) => m.line as i64,
                        _ => -1,
                    };
                    format!("row={:?} col={:?} modline={}", rc.map(|r| r.0), rc.map(|r| r.1), mod_line)
                };
                out.push_str(&format!("{}|{}|{}|{}|{}\n", tc.shader_name, tc.doc, tc.value, loc, raw));
            }
            out.script_to_value(vm)
        },
    );

    native.add_method(
        heap,
        shader,
        id_lut!(test_compile_draw_rust_contains),
        script_args!(io_self = NIL, needle = NIL),
        |vm, args| {
            let io_self = script_value!(vm, args.io_self);
            let needle_val = script_value!(vm, args.needle);
            let Some(needle) = vm.bx.heap.string_with(needle_val, |_heap, s| s.to_string()) else {
                return ScriptValue::from_bool(false);
            };

            if let Some(io_self) = io_self.as_object() {
                let mut output = ShaderOutput::default();
                output.backend = ShaderBackend::Rust;
                output.use_vulkan = false;

                output.pre_collect_rust_instance_io(vm, io_self);
                output.pre_collect_shader_io(vm, io_self);

                if let Some(fnobj) = vm
                    .bx
                    .heap
                    .object_method(
                        io_self,
                        id!(vertex).into(),
                        vm.bx.threads.cur_ref().trap.pass(),
                    )
                    .as_object()
                {
                    output.mode = ShaderMode::Vertex;
                    ShaderFnCompiler::compile_shader_def(
                        vm,
                        &mut output,
                        NoTrap,
                        id!(vertex),
                        fnobj,
                        ShaderType::IoSelf(io_self),
                        vec![],
                    );
                }
                if let Some(fnobj) = vm
                    .bx
                    .heap
                    .object_method(
                        io_self,
                        id!(fragment).into(),
                        vm.bx.threads.cur_ref().trap.pass(),
                    )
                    .as_object()
                {
                    output.mode = ShaderMode::Fragment;
                    ShaderFnCompiler::compile_shader_def(
                        vm,
                        &mut output,
                        NoTrap,
                        id!(fragment),
                        fnobj,
                        ShaderType::IoSelf(io_self),
                        vec![],
                    );
                }

                output.assign_uniform_buffer_indices(&vm.bx.heap, 3);

                // The Rust backend has no monolithic module emitter here (the
                // headless runtime owns that); struct defs plus the compiled
                // function bodies cover everything expression codegen produces.
                let mut out = String::new();
                output.create_struct_defs(vm, &mut out);
                output.create_functions(&mut out);

                return ScriptValue::from_bool(out.contains(&needle));
            }

            ScriptValue::from_bool(false)
        },
    );
}

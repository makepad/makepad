use makepad_relax_api::relax_api;
use crate::heap::*;
use crate::native::*;
use crate::value::*;
#[allow(unused)]
use crate::*;
use makepad_live_id::*;

#[relax_api(plain)]
#[derive(PartialEq, Eq, Clone, Copy, Debug)]
pub struct ShaderIoType(pub(crate) u32);

impl ShaderIoType {
    /// The io type's number, as the `SHADER_IO_*` constants define it.
    pub const fn index(self) -> u32 {
        self.0
    }
}

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
/// A compute kernel's workgroup-shared array (`shader.workgroup(T, len)`).
pub const SHADER_IO_WORKGROUP: ShaderIoType = ShaderIoType(31);
/// A compute-only type descriptor, never an IO global.
pub const SHADER_COOPERATIVE_MATRIX: ShaderIoType = ShaderIoType(32);

pub fn define_shader_module(heap: &mut ScriptHeap, native: &mut ScriptNative) {
    let shader = heap.new_module(id!(shader));

    // Narrow integer components are matrix-only; memory may be packed in
    // u32/vec4u arrays. Offsets and strides use the array's element units.
    heap.set_value(shader, id!(i8).into(), 0u32.into(), NoTrap);
    heap.set_value(shader, id!(u8).into(), 1u32.into(), NoTrap);
    heap.set_value(shader, id!(matrix_a).into(), 0u32.into(), NoTrap);
    heap.set_value(shader, id!(matrix_b).into(), 1u32.into(), NoTrap);
    heap.set_value(shader, id!(matrix_accumulator).into(), 2u32.into(), NoTrap);
    heap.set_value(shader, id!(row_major).into(), 0u32.into(), NoTrap);
    heap.set_value(shader, id!(column_major).into(), 1u32.into(), NoTrap);
    native.add_method(heap, shader, id_lut!(coopmat),
        script_args!(component = NIL, rows = NIL, cols = NIL, usage = NIL), |vm, args| {
            let component = script_value!(vm, args.component);
            let rows = script_value!(vm, args.rows);
            let cols = script_value!(vm, args.cols);
            let usage = script_value!(vm, args.usage);
            let obj = vm.bx.heap.new_with_proto(component);
            set_script_value!(vm, obj.rows = rows);
            set_script_value!(vm, obj.cols = cols);
            set_script_value!(vm, obj.usage = usage);
            vm.bx.heap.set_shader_io(obj, SHADER_COOPERATIVE_MATRIX);
            obj.into()
        });

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

    // Compute kernels: a device buffer of `value` elements, read-only or
    // read-write, and a workgroup-shared array of `len` elements.
    native.add_method(
        heap,
        shader,
        id_lut!(buffer),
        script_args!(value = NIL),
        |vm, args| {
            let value = script_value!(vm, args.value);
            let obj = vm.bx.heap.new_with_proto(value);
            vm.bx.heap.set_shader_io(obj, SHADER_IO_BUFFER_R);
            obj.into()
        },
    );

    native.add_method(
        heap,
        shader,
        id_lut!(buffer_rw),
        script_args!(value = NIL),
        |vm, args| {
            let value = script_value!(vm, args.value);
            let obj = vm.bx.heap.new_with_proto(value);
            vm.bx.heap.set_shader_io(obj, SHADER_IO_BUFFER_RW);
            obj.into()
        },
    );

    native.add_method(
        heap,
        shader,
        id_lut!(workgroup),
        script_args!(value = NIL, len = 1.0),
        |vm, args| {
            let value = script_value!(vm, args.value);
            let len = script_value!(vm, args.len);
            let obj = vm.bx.heap.new_with_proto(value);
            set_script_value!(vm, obj.len = len);
            vm.bx.heap.set_shader_io(obj, SHADER_IO_WORKGROUP);
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

    #[cfg(not(all(target_arch = "wasm32", makepad_precompiled_shaders)))]
    define_shader_compile_natives(heap, native, shader);
}

/// The compile test seams (`mod.shader.test_compile_*`). They run the
/// shader compiler, which a build on shader packs leaves out.
#[cfg(not(all(target_arch = "wasm32", makepad_precompiled_shaders)))]
fn define_shader_compile_natives(heap: &mut ScriptHeap, native: &mut ScriptNative, shader: ScriptObject) {
    use crate::shader_backend::*;

    native.add_method(
        heap,
        shader,
        id_lut!(test_compile_draw),
        script_args!(io_self = NIL),
        |vm, args| {
            // Compiles a draw shader (Metal); the errors are reported.
            let io_self = script_value!(vm, args.io_self);
            if let Some(io_self) = io_self.as_object() {
                let output = test_ir_compile(vm, io_self, ShaderBackend::Metal);
                if !output.has_errors {
                    let _ = crate::shader_ir_metal::metal_draw_source(&output, vm, false);
                }
            }
            NIL
        },
    );

    native.add_method(
        heap,
        shader,
        id_lut!(test_compile_draw_contains),
        script_args!(io_self = NIL, needle = NIL),
        |vm, args| {
            // Whether the shader's Metal source contains `needle`.
            let io_self = script_value!(vm, args.io_self);
            let needle_val = script_value!(vm, args.needle);
            let Some(needle) = vm.bx.heap.string_with(needle_val, |_heap, s| s.to_string()) else {
                return ScriptValue::from_bool(false);
            };
            let Some(io_self) = io_self.as_object() else {
                return ScriptValue::from_bool(false);
            };
            let output = test_ir_compile(vm, io_self, ShaderBackend::Metal);
            if output.has_errors {
                return ScriptValue::from_bool(false);
            }
            let src = crate::shader_ir_metal::metal_draw_source(&output, vm, false);
            ScriptValue::from_bool(src.contains(&needle))
        },
    );

    native.add_method(
        heap,
        shader,
        id_lut!(test_compile_draw_errors),
        script_args!(io_self = NIL),
        |vm, args| {
            // Contract under test: every compile failure must surface a
            // human-readable message — a silently failing shader (gray
            // fallback, no log) is itself the bug.
            let io_self = script_value!(vm, args.io_self);
            let Some(io_self) = io_self.as_object() else {
                return "no shader object".to_string().script_to_value(vm);
            };
            let output = test_ir_compile(vm, io_self, ShaderBackend::Metal);
            if output.has_errors {
                return output.error_report().script_to_value(vm);
            }
            "".to_string().script_to_value(vm)
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
                // SPIR-V through the shader IR: the layout compile is GLSL,
                // as the Vulkan renderer's.
                "spirv" => ShaderBackend::Glsl,
                other => {
                    return format!("unknown backend {}", other).script_to_value(vm);
                }
            };
            let Some(io_self) = io_self.as_object() else {
                return "no shader object".to_string().script_to_value(vm);
            };
            // The IR backends: one compile, printed.
            {
                let opts = crate::shader_ir_draw::IrCompileOptions {
                    const_table,
                    backend: match backend {
                        ShaderBackend::Hlsl | ShaderBackend::Metal => backend,
                        _ => ShaderBackend::Glsl,
                    },
                    ..Default::default()
                };
                let mut output = crate::shader_ir_draw::compile_draw_shader_ir_with(vm, io_self, &opts);
                if output.has_errors {
                    return format!("ERRORS: {}", output.error_report()).script_to_value(vm);
                }
                let mut out = String::new();
                match backend_name.as_str() {
                    "glsl" => {
                        let src = crate::shader_ir_glsl::glsl_draw_sources(&mut output, vm);
                        out.push_str(&src.vertex);
                        out.push_str(&src.fragment);
                    }
                    "hlsl" => out.push_str(&crate::shader_ir_hlsl::hlsl_draw_source(&output, vm)),
                    "metal" => out.push_str(&crate::shader_ir_metal::metal_draw_source(&output, vm, false)),
                    "wgsl" => match crate::shader_ir_wgsl::draw_wgsl(&output, vm, false) {
                        Ok((wgsl, _)) => out.push_str(&wgsl),
                        Err(e) => return format!("ERRORS: {}", e).script_to_value(vm),
                    },
                    _ => match crate::shader_ir_draw::draw_spirv(&output, vm) {
                        // The window variant's words, `v:` and `f:` lines of hex.
                        Ok([window, _]) => {
                            out.push_str("v:");
                            for w in &window.vertex {
                                out.push_str(&format!("{:08x}", w));
                            }
                            out.push_str("\nf:");
                            for w in &window.fragment {
                                out.push_str(&format!("{:08x}", w));
                            }
                        }
                        Err(e) => return format!("ERRORS: {}", e).script_to_value(vm),
                    },
                }
                out.script_to_value(vm)
            }
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
            let opts = crate::shader_ir_draw::IrCompileOptions {
                const_table: true,
                backend: ShaderBackend::Metal,
                ..Default::default()
            };
            let output = crate::shader_ir_draw::compile_draw_shader_ir_with(vm, io_self, &opts);
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
            // Whether the shader's generated Rust functions contain `needle`.
            let io_self = script_value!(vm, args.io_self);
            let needle_val = script_value!(vm, args.needle);
            let Some(needle) = vm.bx.heap.string_with(needle_val, |_heap, s| s.to_string()) else {
                return ScriptValue::from_bool(false);
            };
            let Some(io_self) = io_self.as_object() else {
                return ScriptValue::from_bool(false);
            };
            let output = test_ir_compile(vm, io_self, ShaderBackend::Rust);
            if output.has_errors {
                return ScriptValue::from_bool(false);
            }
            let src = crate::shader_ir_rust::rust_shader_source(&output, vm).source;
            ScriptValue::from_bool(src.contains(&needle))
        },
    );
}

/// A test seam's compile: the draw shader to IR for `backend`.
#[cfg(not(all(target_arch = "wasm32", makepad_precompiled_shaders)))]
fn test_ir_compile(vm: &mut ScriptVm, io_self: ScriptObject, backend: crate::shader_backend::ShaderBackend) -> crate::shader_output::ShaderOutput {
    let opts = crate::shader_ir_draw::IrCompileOptions { backend, ..Default::default() };
    crate::shader_ir_draw::compile_draw_shader_ir_with(vm, io_self, &opts)
}

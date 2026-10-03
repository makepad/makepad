//! Instance records are read from the instance buffer by instance index on
//! every backend that has such a buffer (Metal, D3D11, Vulkan/WebGPU), at the
//! word offsets the draw list writes them; GL ES / WebGL 2 keep vertex
//! attributes and read what does not fit from the instance data texture. A
//! record far over every attribute limit compiles everywhere.
use makepad_script::*;

/// 40 mat4s (640 words), then a vec3, a vec2i (on a 4-word boundary), a
/// u32 and a float: past every backend's vertex attributes. Fields enter
/// the record in the order the shader first reads them.
const FIELDS: &str = "m0: shader.instance(mat4x4f)\n";

fn compile(backend: &str) -> String {
    let host = Box::leak(Box::new(ScriptVmHost::new(0i32, ())));
    let mut vm = ScriptVm { host, bx: Box::new(ScriptVmBase::new()) };
    vm.bx.captured_errors = Some(Vec::new());
    let mut fields = String::new();
    let mut sum = String::from("self.m0");
    for k in 0..40 {
        fields.push_str(&FIELDS.replace("m0", &format!("m{k}")));
        if k > 0 {
            sum.push_str(&format!(" + self.m{k}"));
        }
    }
    let code = format!(
        "use mod.pod.*\nuse mod.math.*\nuse mod.shader\nlet base = {{\ngeom: shader.vertex_buffer(vec2f, nil)\nvertex_pos: shader.vertex_position(vec4f)\npixel: shader.fragment_output(0, vec4f)\n\
         vertex: fn() {{ self.vertex_pos = ({sum}) * vec4(self.geom.x, self.geom.y, 0.0, 1.0) }}\n}}\n\
         let sh = base{{\n{fields}\
         f: shader.instance(0.5)\nn: shader.instance(vec2i)\nu: shader.instance(u32)\np: shader.instance(vec3f)\n\
         fragment: fn() {{ self.pixel = vec4(self.p.z, float(self.n.y), float(self.u), self.f) }}\n}}\n\
         shader.test_compile_draw_source(sh, \"{backend}\", false)"
    );
    let value = vm.with_instruction_limit(50_000_000, |vm| vm.eval(ScriptMod { file: "x".into(), code, ..Default::default() }));
    let errors = vm.take_errors();
    assert!(errors.is_empty(), "{backend}: {errors:?}");
    let src = vm.bx.heap.string_with(value, |_, s| s.to_string()).unwrap_or_default();
    assert!(!src.is_empty() && !src.starts_with("ERRORS") && !src.starts_with("unknown"), "{backend}: {src}");
    src
}

// Word offsets: m0..m39 at 0..640, p at 640, n at 644 (a vec4 boundary),
// the words after it padded to 648, u at 648, f at 649; stride 650.

#[test]
fn metal_reads_the_record_by_word_offset() {
    let src = compile("metal");
    assert!(!src.contains("IoInstanceRaw"), "{src}");
    assert!(src.contains("#define MP_INSTANCE_WORDS 650u"), "{src}");
    assert!(src.contains("constant uint *i_raw [[buffer(1)]]"), "{src}");
    assert!(src.contains("out_instance.f = as_type<float>(w[649]);"), "{src}");
    assert!(src.contains("out_instance.n = int2(as_type<int>(w[644]), as_type<int>(w[645]));"), "{src}");
    assert!(src.contains("out_instance.u = w[648];"), "{src}");
    assert!(src.contains("out_instance.p = float3(as_type<float>(w[640]), as_type<float>(w[641]), as_type<float>(w[642]));"), "{src}");
}

#[test]
fn hlsl_reads_the_record_from_a_buffer_and_carries_only_what_the_pixel_stage_reads() {
    let src = compile("hlsl");
    assert!(src.contains("Buffer<uint> _mp_inst : register(t0);"), "{src}");
    assert!(!src.contains(": INST"), "{src}");
    assert!(src.contains("uint _mp_ib = input.iid * 650;"), "{src}");
    assert!(src.contains("_mp_iov.i.io_n = int2(asint(_mp_inst[_mp_ib + 644]), asint(_mp_inst[_mp_ib + 645]));"), "{src}");
    assert!(src.contains("_mp_iov.i.io_u = _mp_inst[_mp_ib + 648];"), "{src}");
    // The pixel stage reads p, n, u and f, never the matrices.
    let varying = &src[src.find("struct IoVarying").unwrap()..];
    let varying = &varying[..varying.find("};").unwrap()];
    assert!(!varying.contains(" io_m0 ") && !varying.contains(" io_m39 "), "{varying}");
    assert!(varying.contains(" io_p : VARY"), "{varying}");
}

#[test]
fn wgsl_reads_the_record_from_a_storage_buffer_and_validates() {
    let src = compile("wgsl");
    assert!(src.contains("var<storage, read> _mp_inst: array<u32>;"), "{src}");
    assert!(!src.contains("packed_instance_0: vec4f"), "{src}");
    assert!(src.contains("let _mp_ib = in.instance_index * 650u;"), "{src}");
    // The emitter's pass helpers read the draw pass's uniform buffer, which
    // every real draw shader declares (DrawPassUniforms); this one stands in.
    let src = format!(
        "{src}\nstruct MpTestPass {{ camera_projection: mat4x4f, camera_view: mat4x4f, depth_projection: mat4x4f, depth_view: mat4x4f, camera_inv: mat4x4f }}\n\
         @group(1) @binding(0) var<uniform> unibuf_draw_pass: MpTestPass;\n"
    );
    let (vertex, fragment) =
        makepad_script::shader_spirv::compile_wgsl_to_spirv(&src).unwrap_or_else(|e| panic!("{e}\n{src}"));
    for words in [vertex.expect("vertex_main"), fragment.expect("fragment_main")] {
        if let Some(Err(e)) = makepad_script::shader_spirv::spirv_val(&words) {
            panic!("spirv-val: {e}\n{src}");
        }
    }
}

#[test]
fn glsl_keeps_attributes_and_reads_the_rest_from_the_instance_texture() {
    let src = compile("glsl");
    // 650 words is wider than GL ES's 2048-byte stride: all of it is fetched.
    assert!(src.contains("uniform highp usampler2D mp_inst_data;"), "{src}");
    assert!(!src.contains("in vec4 packed_instance_"), "{src}");
    assert!(src.contains("gl_InstanceID * 650 + slot"), "{src}");
}

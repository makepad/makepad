//! Instance records are read from the instance buffer by instance index on
//! every backend that has such a buffer (Metal, D3D11, Vulkan/WebGPU), at the
//! word offsets the draw list writes them; GL ES / WebGL 2 keep vertex
//! attributes and read what does not fit from the instance data texture. A
//! record far over every attribute limit compiles everywhere. The instance
//! fields the pixel stage reads reach it exact, in flat integer varyings.
use makepad_script::*;
use makepad_script::makepad_shader::spirv::{DECO_FLAT, DECO_LOCATION, OP_DECORATE, OP_TYPE_INT, OP_TYPE_POINTER, OP_TYPE_VECTOR, OP_VARIABLE, SC_INPUT};

/// 40 mat4s (640 words), then a vec3, a vec2i (on a 4-word boundary), a
/// u32 and a float: past every backend's vertex attributes. Fields enter
/// the record in the order the shader first reads them.
const FIELDS: &str = "m0: shader.instance(mat4x4f)\n";

/// `code`'s value: the source a `shader.test_compile_draw_source` gave.
fn eval_source(backend: &str, code: &str) -> String {
    let host = Box::leak(Box::new(ScriptVmHost::new(0i32, ())));
    let mut vm = ScriptVm {
        host: host.host_context(),
        bx: Box::new(ScriptVmBase::new()),
    };
    vm.bx.captured_errors = Some(Vec::new());
    let value = vm.with_instruction_limit(50_000_000, |vm| vm.eval(ScriptMod::new("", "", "x", 0, 0, code, &[])));
    let errors = vm.take_errors();
    assert!(errors.is_empty(), "{backend}: {errors:?}");
    let src = vm.bx.heap.string_with(value, |_, s| s.to_string()).unwrap_or_default();
    assert!(!src.is_empty() && !src.starts_with("ERRORS") && !src.starts_with("unknown"), "{backend}: {src}");
    src
}

const BASE: &str = "use mod.pod.*\nuse mod.math.*\nuse mod.shader\nlet base = {\ngeom: shader.vertex_buffer(vec2f, nil)\nvertex_pos: shader.vertex_position(vec4f)\npixel: shader.fragment_output(0, vec4f)\n";

fn compile(backend: &str) -> String {
    let mut fields = String::new();
    let mut sum = String::from("self.m0");
    for k in 0..40 {
        fields.push_str(&FIELDS.replace("m0", &format!("m{k}")));
        if k > 0 {
            sum.push_str(&format!(" + self.m{k}"));
        }
    }
    let code = format!(
        "{BASE}\
         vertex: fn() {{ self.vertex_pos = ({sum}) * vec4(self.geom.x, self.geom.y, 0.0, 1.0) }}\n}}\n\
         let sh = base{{\n{fields}\
         f: shader.instance(0.5)\nn: shader.instance(vec2i)\nu: shader.instance(u32)\np: shader.instance(vec3f)\n\
         fragment: fn() {{ self.pixel = vec4(self.p.z, float(self.n.y), float(self.u), self.f) }}\n}}\n\
         shader.test_compile_draw_source(sh, \"{backend}\", false)"
    );
    eval_source(backend, &code)
}

// Word offsets: m0..m39 at 0..640, p at 640, n at 644 (a vec4 boundary),
// the words after it padded to 648, u at 648, f at 649; stride 650.

#[test]
fn metal_reads_the_record_by_word_offset() {
    let src = compile("metal");
    assert!(src.contains("constant uint *_mp_inst [[buffer(1)]]"), "{src}");
    assert!(src.contains(" = (instance_index * 650u);"), "{src}");
    assert!(src.contains("_mp.dyninst_f = as_type<float>(_mp._mp_inst[(_t"), "{src}");
    assert!(src.contains(" + 649u)]);"), "{src}");
    assert!(src.contains("_mp.dyninst_n = int2(as_type<int>(_mp._mp_inst[(_t"), "{src}");
    assert!(src.contains(" + 648u)];"), "{src}");
}

#[test]
fn hlsl_reads_the_record_from_a_buffer_and_carries_only_what_the_pixel_stage_reads() {
    let src = compile("hlsl");
    assert!(src.contains("Buffer<uint> _mp_inst : register(t0);"), "{src}");
    assert!(src.contains("uint _t0 = ((_mp_instance_index + _mp_inst_base) * 650u);"), "{src}");
    assert!(src.contains("dyninst_n = int2(asint(_mp_inst[(_t0 + 644u)]), asint(_mp_inst[(_t0 + 645u)]));"), "{src}");
    assert!(src.contains("dyninst_u = _mp_inst[(_t0 + 648u)];"), "{src}");
    // The pixel stage reads p, n, u and f (7 words), never the matrices.
    let varying = &src[src.find("struct VertexOut").unwrap()..];
    let varying = &varying[..varying.find("};").unwrap()];
    assert!(varying.contains("nointerpolation uint4 flat_instance_1 : TEXCOORD1;") && !varying.contains("flat_instance_2"), "{varying}");
}

#[test]
fn wgsl_reads_the_record_from_a_storage_buffer() {
    let src = compile("wgsl");
    assert!(src.contains("var<storage, read> _mp_inst: array<u32>;"), "{src}");
    assert!(!src.contains("packed_instance_0: vec4f"), "{src}");
    assert!(src.contains("(_mp_instance_index * 650u)"), "{src}");
}

#[test]
fn spirv_reads_the_record_from_a_storage_buffer_and_validates() {
    let src = compile("spirv");
    for line in src.lines() {
        let hex = &line[2..];
        let mut words = Vec::new();
        for k in 0..hex.len() / 8 {
            words.push(u32::from_str_radix(&hex[k * 8..k * 8 + 8], 16).unwrap());
        }
        assert!(words.len() > 5, "{line}");
        if let Some(Err(e)) = makepad_script::shader_ir_spirv::spirv_val(&words) {
            panic!("spirv-val ({}): {e}", &line[..1]);
        }
    }
}

#[test]
fn glsl_keeps_attributes_and_reads_the_rest_from_the_instance_texture() {
    let src = compile("glsl");
    // 650 words is wider than GL ES's 2048-byte stride: all of it is fetched.
    assert!(src.contains("uniform highp usampler2D mp_inst_data;"), "{src}");
    assert!(!src.contains("in vec4 packed_instance_"), "{src}");
    assert!(src.contains("(gl_InstanceID + mp_inst_base) * 650 + slot"), "{src}");
}

/// A pixel stage that reads an integer (an enum's record word) and a float
/// instance field, and has no varyings of its own: both fields reach it in
/// one flat integer varying, as the record holds them. As interpolated float
/// lanes, perspective correction turned a constant 2 into 1.9999999, read as
/// 1: a ring shape drew as a capsule.
fn compile_flat(backend: &str) -> String {
    let code = format!(
        "{BASE}\
         vertex: fn() {{ self.vertex_pos = vec4(self.geom.x, self.geom.y, self.depth, 1.0) }}\n}}\n\
         let sh = base{{\nkind: shader.instance(u32)\nscale: shader.instance(0.5)\ndepth: shader.instance(0.25)\n\
         fragment: fn() {{ self.pixel = vec4(self.scale, float(self.kind), 0.0, 1.0) }}\n}}\n\
         shader.test_compile_draw_source(sh, \"{backend}\", false)"
    );
    eval_source(backend, &code)
}

#[test]
fn the_pixel_stage_reads_its_instance_fields_flat_and_exact() {
    // kind and scale: two words, one varying; never an interpolated float one
    let flat_lines = |src: &str, qualifier: &str| src.lines().filter(|l| l.contains("flat_instance_") && l.contains(qualifier)).count();
    for backend in ["wgsl", "wgsl_xr"] {
        let src = compile_flat(backend);
        assert!(src.contains("@interpolate(flat) flat_instance_0: vec4u"), "{backend}: {src}");
        assert_eq!(flat_lines(&src, "@interpolate(flat)"), 2, "{backend}: the vertex output and the fragment input:\n{src}");
        assert!(!src.contains("flat_instance_1") && !src.contains("packed_varying"), "{backend}: {src}");
    }
    let src = compile_flat("glsl");
    assert!(src.lines().any(|l| l.starts_with("flat out ") && l.contains("uvec4") && l.contains("flat_instance_0")), "{src}");
    assert!(src.lines().any(|l| l.starts_with("flat in ") && l.contains("uvec4") && l.contains("flat_instance_0")), "{src}");
    assert!(!src.contains("flat_instance_1") && !src.contains("packed_varying"), "{src}");
    let src = compile_flat("hlsl");
    let varying = &src[src.find("struct VertexOut").unwrap()..];
    let varying = &varying[..varying.find("};").unwrap()];
    assert!(varying.contains("nointerpolation uint4 flat_instance_0 : TEXCOORD0;"), "{varying}");
    assert!(!varying.contains("flat_instance_1") && !varying.contains("float4 packed_varying"), "{varying}");
    // SPIR-V, window and XR: every fragment input at a location is an integer vector decorated Flat
    for backend in ["spirv", "spirv_xr"] {
        let src = compile_flat(backend);
        let fragment = src.lines().find_map(|l| l.strip_prefix("f:")).expect("fragment words");
        let words: Vec<u32> = (0..fragment.len() / 8).map(|k| u32::from_str_radix(&fragment[k * 8..k * 8 + 8], 16).unwrap()).collect();
        let inputs = location_inputs(&words);
        assert_eq!(inputs.len(), 1, "{backend}: {inputs:?}");
        for (int_vector, flat) in inputs {
            assert!(int_vector && flat, "{backend}: a fragment input that is not a flat integer vector");
        }
        for line in src.lines() {
            let hex = &line[2..];
            let words: Vec<u32> = (0..hex.len() / 8).map(|k| u32::from_str_radix(&hex[k * 8..k * 8 + 8], 16).unwrap()).collect();
            if let Some(Err(e)) = makepad_script::shader_ir_spirv::spirv_val(&words) {
                panic!("spirv-val ({backend} {}): {e}", &line[..1]);
            }
        }
    }
}

/// The module's Input variables with a Location: (an integer vector, decorated Flat).
fn location_inputs(words: &[u32]) -> Vec<(bool, bool)> {
    let mut int_vectors = Vec::new();
    let mut ints = Vec::new();
    let mut pointers = Vec::new();
    let mut variables = Vec::new();
    let mut located = Vec::new();
    let mut flat = Vec::new();
    let mut at = 5;
    while at < words.len() {
        let (op, len) = (words[at] & 0xffff, (words[at] >> 16) as usize);
        assert!(len > 0);
        let w = &words[at + 1..at + len];
        match op {
            OP_TYPE_INT => ints.push(w[0]),
            OP_TYPE_VECTOR if ints.contains(&w[1]) => int_vectors.push(w[0]),
            OP_TYPE_POINTER => pointers.push((w[0], w[1], w[2])),
            OP_VARIABLE => variables.push((w[1], w[0], w[2])),
            OP_DECORATE if w[1] == DECO_LOCATION => located.push(w[0]),
            OP_DECORATE if w[1] == DECO_FLAT => flat.push(w[0]),
            _ => {}
        }
        at += len;
    }
    variables
        .iter()
        .filter(|(id, _, class)| *class == SC_INPUT && located.contains(id))
        .map(|(id, ptr, _)| {
            let pointee = pointers.iter().find(|(p, _, _)| p == ptr).map(|(_, _, t)| *t);
            (pointee.is_some_and(|t| int_vectors.contains(&t)), flat.contains(id))
        })
        .collect()
}

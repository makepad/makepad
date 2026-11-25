//! SPIR-V: every call to a function that reads a texture goes to its own
//! copy of that function; other functions are emitted once and shared. A
//! driver (PowerVR, test phone) refuses the pipeline with ERROR_UNKNOWN when a
//! texture-reading function of some size is called from two places.
use makepad_script::*;

fn spirv_fragment() -> Vec<u32> {
    let host = Box::leak(Box::new(ScriptVmHost::new(0i32, ())));
    let mut vm = ScriptVm {
        host: host.host_context(),
        bx: Box::new(ScriptVmBase::new()),
    };
    vm.bx.captured_errors = Some(Vec::new());
    let code = "use mod.pod.*\nuse mod.math.*\nuse mod.shader\nlet sh = {\n\
        geom: shader.vertex_buffer(vec2f, nil)\nvertex_pos: shader.vertex_position(vec4f)\npixel: shader.fragment_output(0, vec4f)\n\
        u_n: shader.uniform(0.0)\ntex: shader.texture_2d(float)\n\
        half: fn(x: float) -> float { return x * 0.5 }\n\
        fetch: fn(uv: vec2) -> vec4 { return self.tex.sample(uv) * self.half(uv.x) }\n\
        vertex: fn() { self.vertex_pos = vec4(self.geom.x, self.geom.y, 0.0, 1.0) }\n\
        fragment: fn() {\n\
          if self.u_n > 0.5 { self.pixel = self.fetch(vec2(self.u_n, 0.25)) * self.half(self.u_n) }\n\
          else { self.pixel = self.fetch(vec2(0.5, self.u_n)) * self.half(0.75) }\n\
        }\n}\nshader.test_compile_draw_source(sh, \"spirv\", false)";
    let value = vm.with_instruction_limit(5_000_000, |vm| vm.eval(ScriptMod::new("", "", "x", 0, 0, code, &[])));
    let errors = vm.take_errors();
    assert!(errors.is_empty(), "{errors:?}");
    let src = vm.bx.heap.string_with(value, |_, s| s.to_string()).unwrap_or_default();
    fragment_words(&src)
}

fn fragment_words(src: &str) -> Vec<u32> {
    assert!(src.starts_with("v:"), "{src}");
    let line = src.lines().find(|l| l.starts_with("f:")).expect("fragment words");
    let hex = &line[2..];
    let words: Vec<u32> = (0..hex.len() / 8).map(|k| u32::from_str_radix(&hex[k * 8..k * 8 + 8], 16).unwrap()).collect();
    if let Some(Err(e)) = makepad_script::shader_ir_spirv::spirv_val(&words) {
        panic!("spirv-val: {e}");
    }
    words
}

fn instructions(words: &[u32]) -> impl Iterator<Item = (u32, &[u32])> {
    let mut remaining = &words[5..];
    std::iter::from_fn(move || {
        let header = *remaining.first()?;
        let len = (header >> 16) as usize;
        assert!(len > 0);
        let (instruction, rest) = remaining.split_at(len);
        remaining = rest;
        Some((header & 0xffff, &instruction[1..]))
    })
}

/// Per function (in module order): whether it samples, and the functions it calls.
fn functions(words: &[u32]) -> Vec<(bool, Vec<u32>)> {
    const OP_FUNCTION: u32 = 54;
    const OP_FUNCTION_CALL: u32 = 57;
    const OP_IMAGE_SAMPLE_IMPLICIT_LOD: u32 = 87;
    let mut out: Vec<(bool, Vec<u32>)> = Vec::new();
    for (op, args) in instructions(words) {
        match op {
            OP_FUNCTION => out.push((false, Vec::new())),
            OP_IMAGE_SAMPLE_IMPLICIT_LOD => out.last_mut().unwrap().0 = true,
            OP_FUNCTION_CALL => out.last_mut().unwrap().1.push(args[2]),
            _ => {}
        }
    }
    out
}

#[test]
fn a_texture_reading_function_gets_a_copy_per_call_and_others_are_shared() {
    let fns = functions(&spirv_fragment());
    // `fetch`, called twice, is two functions that sample.
    assert_eq!(fns.iter().filter(|f| f.0).count(), 2, "{fns:?}");
    // `half` is one function, called from both copies of `fetch` and the fragment.
    let calls: Vec<u32> = fns.iter().flat_map(|f| f.1.iter().copied()).collect();
    assert!(calls.iter().any(|c| calls.iter().filter(|d| *d == c).count() >= 4), "{calls:?}");
}

fn check_volume_spirv(src: &str, chain: bool) {
    use makepad_script::makepad_shader::spirv::*;
    let words = fragment_words(src);
    let operands = |op| instructions(&words).filter_map(move |(code, args)| (code == op).then_some(args));
    let lanes = |ty| operands(OP_TYPE_VECTOR).find(|a| a[0] == ty).expect("vector type")[2];
    assert!(operands(OP_TYPE_IMAGE).any(|a| a[2] == DIM_3D && a[4] == 0), "non-array 3D image");
    let fetch = operands(OP_IMAGE_FETCH).next().expect("volume fetch");
    assert_eq!(lanes(fetch[0]), 4, "load result arity");
    let coord = operands(OP_LOAD).chain(operands(OP_COMPOSITE_CONSTRUCT))
        .find(|a| a[1] == fetch[3]).expect("load coordinate");
    assert_eq!(lanes(coord[0]), 3, "spatial coordinate arity");
    assert_eq!(fetch[4], IMAGE_OPERAND_LOD);
    assert!(operands(OP_CONSTANT).any(|a| a[1] == fetch[5] && a[2] == 0), "level zero");
    assert!(operands(OP_STORE).any(|a| a[1] == fetch[1]), "the fetched texel is used");
    if chain {
        let mut body = instructions(&words);
        let (_, merge) = body.find(|i| i.0 == OP_LOOP_MERGE).expect("dependent loop");
        assert!(body.take_while(|&(op, a)| op != OP_LABEL || a[0] != merge[0]).any(|i| i.0 == OP_IMAGE_FETCH), "fetch in loop");
        assert!(operands(OP_CONSTANT).any(|a| a[2] == 24), "static count");
    } else {
        let size = operands(OP_IMAGE_QUERY_SIZE_LOD).next().expect("volume dimensions");
        assert_eq!(lanes(size[0]), 3, "integer size arity");
        assert!(operands(OP_CONVERT_U_TO_F).any(|a| a[2] == size[1] && lanes(a[0]) == 3), "size returns vec3f");
    }
}

#[test]
fn volume_load_and_size() {
    let mut host = ScriptVmHost::new(0i32, ());
    let mut vm = ScriptVm { host: host.host_context(), bx: Box::new(ScriptVmBase::new()) };
    let mut compile = |backend: &str, case: &str, body: &str| {
        let file = format!("volume_{case}_{backend}");
        let code = format!(r#"
use mod.pod.*
use mod.math.*
use mod.shader
let sh = {{
    vertex_pos: shader.vertex_position(vec4f)
    pixel: shader.fragment_output(0, vec4f)
    tex: shader.texture_3d(float)
    vertex: fn() {{ self.vertex_pos = vec4(0.0, 0.0, 0.0, 1.0) }}
    fragment: fn() {{ {body} }}
}}
shader.test_compile_draw_source(sh, "{backend}", false)"#);
        vm.capture_errors();
        let value = vm.eval(ScriptMod::new("", "", &file, 0, 0, &code, &[]));
        let src = vm.bx.heap.string_with(value, |_, s| s.to_owned()).unwrap_or_default();
        let errors = vm.take_errors();
        if errors.is_empty() { src } else { format!("ERRORS: {errors:?} {src}") }
    };
    let direct = "
        let dims = self.tex.size()
        let point = vec3i(i32(dims.x) - 1i, i32(dims.y) - 1i, i32(dims.z) - 1i)
        self.pixel = self.tex.load(point)";
    let chain = "
        var point = vec3i(0i, 0i, 0i)
        var texel = vec4(0.0)
        for step in 0..24 {
            texel = self.tex.load(point)
            point = vec3i(i32(round(texel.r * 255.0)), point.z, point.y)
        }
        self.pixel = texel";
    for backend in ["metal", "glsl", "gles", "hlsl", "wgsl", "spirv"] {
        let src = compile(backend, "direct", direct);
        let loop_src = compile(backend, "chain", chain);
        for s in [&src, &loop_src] {
            assert!(!s.is_empty() && !s.starts_with("ERRORS"), "{backend}: {s}");
        }
        if backend == "spirv" {
            check_volume_spirv(&src, false);
            check_volume_spirv(&loop_src, true);
            continue;
        }
        let (texture, coord, load, size) = match backend {
            "metal" => ("texture3d<float>", "int3", ".read(uint3(", ".get_depth()"),
            "glsl" | "gles" => ("sampler3D", "ivec3", "texelFetch(tex_tex,", "vec3(textureSize(tex_tex, 0))"),
            "hlsl" => ("Texture3D", "int3", ".Load(int4(", "_mpTexSize3D(tex_tex)"),
            "wgsl" => ("texture_3d<f32>", "vec3i", "textureLoad(tex_tex,", "vec3f(textureDimensions(tex_tex))"),
            _ => unreachable!(),
        };
        for needle in [texture, coord, size] {
            assert!(src.contains(needle), "{backend} missing {needle}: {src}");
        }
        assert!(src.lines().any(|line| line.contains("frag_fb0") && line.contains(load)), "{backend} load to pixel: {src}");
        if backend == "hlsl" {
            assert!(src.contains("GetDimensions(w, h, d)"), "HLSL dimension arity: {src}");
        }
        if backend == "gles" {
            for s in [&src, &loop_src] {
                assert!(s.starts_with("#version 300 es") && s.contains("precision highp sampler3D;"), "{s}");
            }
        }
        let (_, for_body) = loop_src.split_once("for (").expect("static loop");
        assert!(for_body.lines().next().unwrap().contains("24"), "{backend} loop bound: {loop_src}");
        assert!(for_body.lines().any(|l| l.contains("_texel = ") && l.contains(load) && l.contains("_point")), "{backend} dependent fetch: {loop_src}");
        assert!(for_body.lines().any(|l| l.contains("_point = ") && l.contains("_texel")), "{backend} next coordinate: {loop_src}");
        assert!(for_body.lines().any(|l| l.contains("frag_fb0") && l.contains("_texel")), "{backend} final texel: {loop_src}");
    }
    for (case, coord) in [("two_lanes", "vec2i(0i, 0i)"), ("float", "vec3(0.0, 0.0, 0.0)")] {
        let error = compile("metal", case, &format!("self.pixel = self.tex.load({coord})"));
        assert!(error.starts_with("ERRORS") && error.contains("texture_3d.load") && error.contains("vec3i"), "{coord}: {error}");
    }
}

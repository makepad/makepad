//! SPIR-V: every call to a function that reads a texture goes to its own
//! copy of that function; other functions are emitted once and shared. A
//! driver (PowerVR, Pixel 11) refuses the pipeline with ERROR_UNKNOWN when a
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
    assert!(src.starts_with("v:"), "{src}");
    let line = src.lines().find(|l| l.starts_with("f:")).expect("fragment words");
    let hex = &line[2..];
    let words: Vec<u32> = (0..hex.len() / 8).map(|k| u32::from_str_radix(&hex[k * 8..k * 8 + 8], 16).unwrap()).collect();
    if let Some(Err(e)) = makepad_script::shader_ir_spirv::spirv_val(&words) {
        panic!("spirv-val: {e}");
    }
    words
}

/// Per function (in module order): whether it samples, and the functions it calls.
fn functions(words: &[u32]) -> Vec<(bool, Vec<u32>)> {
    const OP_FUNCTION: u32 = 54;
    const OP_FUNCTION_CALL: u32 = 57;
    const OP_IMAGE_SAMPLE_IMPLICIT_LOD: u32 = 87;
    let mut out: Vec<(bool, Vec<u32>)> = Vec::new();
    let mut i = 5;
    while i < words.len() {
        let (op, len) = (words[i] & 0xffff, (words[i] >> 16) as usize);
        assert!(len > 0);
        match op {
            OP_FUNCTION => out.push((false, Vec::new())),
            OP_IMAGE_SAMPLE_IMPLICIT_LOD => out.last_mut().unwrap().0 = true,
            OP_FUNCTION_CALL => out.last_mut().unwrap().1.push(words[i + 3]),
            _ => {}
        }
        i += len;
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

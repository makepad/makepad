//! One clip-z convention on every backend: clip z runs 0..w and a depth
//! buffer holds ndc z (Metal, D3D, Vulkan, WebGPU natively); the OpenGL
//! backend moves its -w..w clip z there in the vertex output.
use makepad_script::*;

fn compile(backend: &str) -> String {
    let host = Box::leak(Box::new(ScriptVmHost::new(0i32, ())));
    let mut vm = ScriptVm { host, bx: Box::new(ScriptVmBase::new()) };
    vm.bx.captured_errors = Some(Vec::new());
    let code = format!(
        "use mod.pod.*\nuse mod.math.*\nuse mod.shader\nlet sh = {{\nvertex_pos: shader.vertex_position(vec4f)\npixel: shader.fragment_output(0, vec4f)\n\
         vertex: fn() {{ self.vertex_pos = vec4(0.25, 0.5, 0.75, 1.0) }}\nfragment: fn() {{ self.pixel = vec4(1.0) }}\n}}\nshader.test_compile_draw_source(sh, \"{backend}\", false)"
    );
    let value = vm.with_instruction_limit(5_000_000, |vm| vm.eval(ScriptMod { file: "x".into(), code, ..Default::default() }));
    assert!(vm.take_errors().is_empty());
    vm.bx.heap.string_with(value, |_, s| s.to_string()).unwrap()
}

#[test]
fn opengl_moves_clip_z_to_the_shared_convention() {
    let glsl = compile("glsl");
    assert!(glsl.contains("2.0 * vtx_pos.z - vtx_pos.w"), "{glsl}");
    for backend in ["metal", "hlsl", "wgsl"] {
        let src = compile(backend);
        assert!(!src.contains("2.0 * vtx_pos.z"), "{backend} keeps its native 0..w clip z:\n{src}");
    }
}

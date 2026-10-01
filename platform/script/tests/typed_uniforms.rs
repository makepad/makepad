//! Matrices are uniforms and instance fields of their own type on every
//! backend (`name: uniform(mat4x4f(..))`, set with 16 floats, column-major):
//! no host packs a mat4 into four vec4 rows of renamed slots.
use makepad_script::*;

fn compile(backend: &str, decl: &str) -> (Vec<String>, String) {
    let host = Box::leak(Box::new(ScriptVmHost::new(0i32, ())));
    let mut vm = ScriptVm { host, bx: Box::new(ScriptVmBase::new()) };
    vm.bx.captured_errors = Some(Vec::new());
    let code = format!(
        "use mod.pod.*\nuse mod.math.*\nuse mod.shader\nlet sh = {{\nvertex_pos: shader.vertex_position(vec4f)\npixel: shader.fragment_output(0, vec4f)\n{decl}\n\
         vertex: fn() {{ self.vertex_pos = self.u_view * vec4(0.25, 0.5, 0.75, 1.0) }}\nfragment: fn() {{ self.pixel = self.u_view * vec4(1.0) }}\n}}\nshader.test_compile_draw_source(sh, \"{backend}\", false)"
    );
    let value = vm.with_instruction_limit(5_000_000, |vm| vm.eval(ScriptMod { file: "x".into(), code, ..Default::default() }));
    let errors = vm.take_errors();
    (errors, vm.bx.heap.string_with(value, |_, s| s.to_string()).unwrap_or_default())
}

#[test]
fn mat4_uniforms_and_instances_compile_on_every_backend() {
    let m = "mat4x4f(1.0,0.0,0.0,0.0, 0.0,1.0,0.0,0.0, 0.0,0.0,1.0,0.0, 0.0,0.0,0.0,1.0)";
    for io in ["uniform", "instance"] {
        for backend in ["metal", "hlsl", "glsl", "wgsl"] {
            let (errors, src) = compile(backend, &format!("u_view: shader.{io}({m})"));
            assert!(errors.is_empty() && !src.starts_with("ERRORS"), "{io} on {backend}: {errors:?} {src}");
        }
    }
}

//! Named per-draw params declared in script (`name: shader.instance(..)`)
//! reach the shader as instance inputs on every backend: a host sets them by
//! name (`DrawVars::set_dyn_instance`) instead of renaming `self.<param>` text
//! into fixed slots.
use makepad_script::*;

fn compile(backend: &str) -> String {
    let host = Box::leak(Box::new(ScriptVmHost::new(0i32, ())));
    let mut vm = ScriptVm { host, bx: Box::new(ScriptVmBase::new()) };
    vm.bx.captured_errors = Some(Vec::new());
    let code = format!(
        "use mod.pod.*\nuse mod.math.*\nuse mod.shader\nlet base = {{\nvertex_pos: shader.vertex_position(vec4f)\npixel: shader.fragment_output(0, vec4f)\nvertex: fn() {{ self.vertex_pos = vec4(0.0, 0.0, 0.0, 1.0) }}\n}}\n\
         let sh = base{{\nswirl_amount: shader.instance(0.5)\nglow_tint: shader.instance(vec4(1.0, 0.5, 0.25, 1.0))\nfragment: fn() {{ self.pixel = self.glow_tint * self.swirl_amount }}\n}}\nshader.test_compile_draw_source(sh, \"{backend}\", false)"
    );
    let value = vm.with_instruction_limit(5_000_000, |vm| vm.eval(ScriptMod { file: "x".into(), code, ..Default::default() }));
    let errors = vm.take_errors();
    assert!(errors.is_empty(), "{backend}: {errors:?}");
    vm.bx.heap.string_with(value, |_, s| s.to_string()).unwrap_or_else(|| format!("ERRORS: not a string {value:?}"))
}

#[test]
fn script_declared_instance_params_compile_on_every_backend() {
    for backend in ["metal", "hlsl", "glsl", "wgsl"] {
        let src = compile(backend);
        assert!(!src.starts_with("ERRORS") && !src.starts_with("unknown"), "{backend}: {src}");
        for name in ["swirl_amount", "glow_tint"] {
            assert!(src.contains(name), "{backend}: instance param {name} missing:\n{src}");
        }
    }
}

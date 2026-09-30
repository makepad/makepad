//! A shader function whose last statement is an `if` (with or without
//! `else`, arms without values) compiles: its implicit return carries no
//! value. It used to fail with "shader stack underflow".
use makepad_script::*;
fn compile(vertex_body: &str, fragment_body: &str) -> String {
    let host = Box::leak(Box::new(ScriptVmHost::new(0i32, ())));
    let mut vm = ScriptVm { host, bx: Box::new(ScriptVmBase::new()) };
    vm.bx.captured_errors = Some(Vec::new());
    let code = format!("use mod.pod.*\nuse mod.math.*\nuse mod.shader\nlet sh = {{\nvertex_pos: shader.vertex_position(vec4f)\npixel: shader.fragment_output(0, vec4f)\nu_n: shader.uniform(0.0)\nv_f: shader.varying(f32)\nvertex: fn() {{\n{vertex_body}\n}}\nfragment: fn() {{\n{fragment_body}\n}}\n}}\nshader.test_compile_draw_source(sh, \"metal\", false)");
    let value = vm.with_instruction_limit(5_000_000, |vm| vm.eval(ScriptMod { file: "x".into(), code, ..Default::default() }));
    vm.bx.heap.string_with(value, |_, s| s.to_string()).unwrap_or_else(|| format!("ERRORS: not a string {value:?}"))
}
#[test]
fn functions_ending_in_a_statement_if_compile() {
    let v = "self.vertex_pos = vec4(0.0, 0.0, 0.0, 1.0)";
    for (name, vb, fb) in [
        ("vertex if/else", format!("{v}\nif self.u_n > 1.0 {{ self.v_f = 1.0 }} else {{ self.v_f = 2.0 }}"), "self.pixel = vec4(self.v_f)".to_string()),
        ("vertex if", format!("{v}\nself.v_f = 2.0\nif self.u_n > 1.0 {{ self.v_f = 1.0 }}"), "self.pixel = vec4(self.v_f)".to_string()),
        ("fragment if/else", format!("{v}\nself.v_f = 1.0"), "if self.u_n > 1.0 { self.pixel = vec4(1.0) } else { self.pixel = vec4(0.0) }".to_string()),
        ("multi-line arms", format!("{v}\nif self.u_n > 1.0 {{\n self.v_f = 1.0\n }} else {{\n self.v_f = 2.0\n }}"), "self.pixel = vec4(self.v_f)".to_string()),
        ("else-if chain", format!("{v}\nif self.u_n > 1.0 {{ self.v_f = 1.0 }} else if self.u_n > 0.5 {{ self.v_f = 3.0 }} else {{ self.v_f = 2.0 }}"), "self.pixel = vec4(self.v_f)".to_string()),
    ] {
        let src = compile(&vb, &fb);
        assert!(!src.starts_with("ERRORS"), "{name}: {src}");
        assert!(src.contains("l_v_f") || src.contains("v_f"), "{name}:\n{src}");
    }
}

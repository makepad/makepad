//! A `while` whose condition calls a function on a var the body reassigns
//! (render's CSM cascade search: `while ci < 3.5 && self.csm_inside(q, 0.99)
//! < 0.5 { ci = ci + 1.0  q = self.csm_proj(min(ci, 3.0), wp) }`) re-tests
//! the whole condition, call included, at the top of every pass, on every
//! backend. The GPU result is checked in the platform's metal_gpu_tests.
use makepad_script::*;

fn compile(backend: &str, fragment_body: &str, extra: &str) -> String {
    let host = Box::leak(Box::new(ScriptVmHost::new(0i32, ())));
    let mut vm = ScriptVm { host, bx: Box::new(ScriptVmBase::new()) };
    vm.bx.captured_errors = Some(Vec::new());
    let code = format!("use mod.pod.*\nuse mod.math.*\nuse mod.shader\nlet sh = {{\nvertex_pos: shader.vertex_position(vec4f)\npixel: shader.fragment_output(0, vec4f)\nu_n: shader.uniform(0.0)\n{extra}\nvertex: fn() {{\nself.vertex_pos = vec4(0.0, 0.0, 0.0, 1.0)\n}}\nfragment: fn() {{\n{fragment_body}\n}}\n}}\nshader.test_compile_draw_source(sh, \"{backend}\", false)");
    let value = vm.with_instruction_limit(5_000_000, |vm| vm.eval(ScriptMod { file: "x".into(), code, ..Default::default() }));
    vm.bx.heap.string_with(value, |_, s| s.to_string()).unwrap_or_else(|| format!("ERRORS: not a string {value:?}"))
}

/// The emitted body of the loop that follows q's declaration: from its
/// `while(true){` / `loop{` to the matching close.
fn loop_body(src: &str) -> &str {
    let start = src.find("l_q = ").expect("q declared");
    let rest = &src[start..];
    let open = rest.find("while(true){").or_else(|| rest.find("loop{")).expect("loop");
    let body = &rest[open..];
    let mut depth = 0;
    for (i, c) in body.char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return &body[..=i];
                }
            }
            _ => {}
        }
    }
    panic!("unclosed loop in\n{src}")
}

#[test]
fn a_while_with_a_called_condition_retests_it_every_pass() {
    let extra = "inside: fn(q: vec3, m: float) -> float {\nif q.x > m { return 0.0 }\nreturn 1.0\n}\nproj: fn(ci: float) -> vec3 {\nreturn vec3(2.0 - ci + self.u_n, 0.0, 0.0)\n}";
    let body = "var ci = 0.0\nvar q = self.proj(0.0)\nwhile ci < 3.5 && self.inside(q, 0.99) < 0.5 {\nci = ci + 1.0\nq = self.proj(min(ci, 3.0))\n}\nself.pixel = vec4(ci, q.x, 0.0, 1.0)";
    for backend in ["metal", "hlsl", "glsl", "wgsl"] {
        let src = compile(backend, body, extra);
        assert!(!src.starts_with("ERRORS"), "{backend}: {src}");
        let lp = loop_body(&src);
        let lines: Vec<&str> = lp.lines().map(str::trim).collect();
        // The guard, then the whole condition (the call included) as the
        // pass's exit test, then the body's two reassignments, in order.
        let cond = lines.iter().position(|l| l.starts_with("if(!(") && l.contains("< 3.5") && l.contains("inside(") && l.ends_with("{break;}")).unwrap_or_else(|| panic!("{backend}: no in-loop condition test\n{lp}"));
        let ci = lines.iter().position(|l| l.starts_with("l_ci = (l_ci + 1")).unwrap_or_else(|| panic!("{backend}: no ci step\n{lp}"));
        let q = lines.iter().position(|l| l.starts_with("l_q = ") && l.contains("proj(") && l.contains("min(l_ci, 3.0)")).unwrap_or_else(|| panic!("{backend}: no q reassignment\n{lp}"));
        assert!(cond < ci && ci < q, "{backend}: condition, step, reassignment out of order\n{lp}");
        // The reassigned var is the one the condition reads: no copy made
        // before the loop stands in for it.
        assert!(lines[cond].contains("inside(") && lines[cond].contains("l_q, 0.99"), "{backend}: {}", lines[cond]);
        // Declared once, before the loop (a declaration inside would shadow).
        assert!(!lp.contains("float3 l_q") && !lp.contains("vec3 l_q") && !lp.contains("var l_q"), "{backend}: q redeclared in the loop\n{lp}");
    }
}

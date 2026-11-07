//! `mod(x, y)` in shaders is the floored modulo, `x - y * floor(x / y)`
//! (the result takes y's sign), and `modf(x, y)` the truncating remainder,
//! `x - y * trunc(x / y)` (x's sign), on every backend and as in script
//! functions; `mod` alone stays the module root, so `use mod.math.*` and
//! `mod.math.floor` still resolve.

use makepad_script::*;

fn test_vm() -> ScriptVm<'static> {
    let host = Box::leak(Box::new(ScriptVmHost::new(0i32, ())));
    ScriptVm {
        host: host.host_context(),
        bx: Box::new(ScriptVmBase::new()),
    }
}

/// Compile `fragment_body` (plus `extra` members) as a draw shader for
/// `backend` and return the emitted source, or `ERRORS: …`.
fn compile(backend: &str, extra: &str, fragment_body: &str) -> String {
    let mut vm = test_vm();
    vm.bx.captured_errors = Some(Vec::new());
    let code = format!(
        "use mod.pod.*\nuse mod.math.*\nuse mod.shader\nlet sh = {{\n\
         vertex_pos: shader.vertex_position(vec4f)\n\
         pixel: shader.fragment_output(0, vec4f)\n\
         u_n: shader.uniform(0.0)\n\
         {extra}\n\
         vertex: fn() {{ self.vertex_pos = vec4(0.0, 0.0, 0.0, 1.0) }}\n\
         fragment: fn() {{\n{fragment_body}\n}}\n\
         }}\nshader.test_compile_draw_source(sh, \"{backend}\", false)"
    );
    let value = vm.with_instruction_limit(5_000_000, |vm| vm.eval(ScriptMod::new("", "", "shader_mod_call", 0, 0, &code, &[])));
    let src = vm.bx.heap.string_with(value, |_, s| s.to_string()).unwrap_or_else(|| format!("ERRORS: not a string: {value:?}"));
    match vm.bx.captured_errors.take() {
        Some(errors) if !errors.is_empty() => format!("ERRORS: {errors:?} {src}"),
        _ => src,
    }
}

const BACKENDS: [&str; 5] = ["metal", "hlsl", "glsl", "wgsl", "spirv"];

fn ok(backend: &str, extra: &str, body: &str) -> String {
    let src = compile(backend, extra, body);
    assert!(!src.starts_with("ERRORS") && !src.starts_with("unknown") && !src.starts_with("no shader"), "{backend}: {src}");
    src
}

/// A number's `f` suffix dropped (`2.0f` as `2.0`), so one needle checks
/// every text backend.
fn plain(src: &str) -> String {
    let b = src.as_bytes();
    let mut out = String::with_capacity(src.len());
    for (i, &c) in b.iter().enumerate() {
        let after_digit = i > 0 && b[i - 1].is_ascii_digit();
        let ends = i + 1 >= b.len() || !(b[i + 1].is_ascii_alphanumeric() || b[i + 1] == b'_');
        if c == b'f' && after_digit && ends {
            continue;
        }
        out.push(c as char);
    }
    out
}

#[test]
fn mod_of_literals_folds_to_the_floored_result() {
    // -1 mod 3 is 2 (not -1), 7.5 mod -2 is -0.5, -7 mod 3 is 2.
    let body = "self.pixel = vec4(mod(-1.0, 3.0), mod(7.5, -2.0), mod(-7, 3), 1.0)";
    for backend in ["metal", "hlsl", "glsl", "wgsl"] {
        let src = plain(&ok(backend, "", body));
        assert!(src.contains("2.0, -0.5, 2.0, 1.0"), "{backend}: {src}");
    }
}

#[test]
fn modf_of_literals_folds_to_the_truncated_result() {
    // -1 rem 3 is -1, 7.5 rem -2 is 1.5, -7 rem 3 is -1: x's sign, as `%`.
    let body = "self.pixel = vec4(modf(-1.0, 3.0), modf(7.5, -2.0), modf(-7, 3), 1.0)";
    for backend in ["metal", "hlsl", "glsl", "wgsl"] {
        let src = plain(&ok(backend, "", body));
        assert!(src.contains("-1.0, 1.5, -1.0, 1.0"), "{backend}: {src}");
    }
}

#[test]
fn modf_of_values_is_trunc_arithmetic_on_every_backend() {
    // No backend's own remainder: GLSL's `mod` floors where the others
    // truncate, so none of them is emitted.
    let body = "\
        let a = modf(self.u_n, 3.0)\n\
        let v = modf(vec2(self.u_n, -1.5), vec2(0.5, 0.25))\n\
        self.pixel = vec4(a, v, 1.0)";
    for backend in BACKENDS {
        let src = ok(backend, "", body);
        if backend != "spirv" {
            assert!(src.contains("trunc("), "{backend}: {src}");
            assert!(!src.contains("mod("), "{backend}: {src}");
        }
    }
}

#[test]
fn mod_of_values_is_floor_arithmetic_on_every_backend() {
    let extra = "wrap: fn(a: float) -> float { return mod(a, 1.0) }";
    let body = "\
        let a = mod(self.u_n, 3.0)\n\
        let v = mod(vec2(self.u_n, -1.5), 0.5)\n\
        let w = mod(vec2(self.u_n, 2.0), vec2(1.0, 3.0))\n\
        let s = mod(1.0, vec2(self.u_n, 0.25))\n\
        let c = mod(self.wrap(self.u_n) * 2.0, 0.75)\n\
        let mut k = 0.0\n\
        if self.u_n > 0.0 && mod(self.u_n * 3.0, 2.0) > 1.0 { k = 1.0 }\n\
        self.pixel = vec4(a + c + k, v.x + w.y, s.x + s.y, 1.0)";
    for backend in BACKENDS {
        let src = ok(backend, extra, body);
        if backend != "spirv" {
            assert!(src.contains("floor("), "{backend}: {src}");
        }
    }
}

#[test]
fn mod_takes_floats() {
    let src = compile("metal", "", "let i = i32(self.u_n)\nlet k = mod(i, 2)\nself.pixel = vec4(0.0, 0.0, 0.0, 1.0)");
    assert!(src.starts_with("ERRORS") && src.contains("integers use %"), "{src}");
    let src = compile("metal", "", "let k = mod(self.u_n)\nself.pixel = vec4(k, 0.0, 0.0, 1.0)");
    assert!(src.starts_with("ERRORS") && src.contains("two arguments"), "{src}");
}

#[test]
fn mod_module_paths_still_resolve() {
    let mut vm = test_vm();
    vm.bx.captured_errors = Some(Vec::new());
    let code = "use mod.math.*\nlet t = [mod.math.floor(-0.5), mod(-1.0, 3.0), floor(1.5)]\nt[0] * 100.0 + t[1] * 10.0 + t[2]";
    let value = vm.eval(ScriptMod::new("", "", "shader_mod_call", 0, 0, code, &[]));
    assert!(vm.bx.captured_errors.take().unwrap_or_default().is_empty());
    assert_eq!(value.as_f64(), Some(-100.0 + 20.0 + 1.0));
}

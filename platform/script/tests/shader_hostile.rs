//! Hostile shader source: the emitters clamp every dynamic index (reads,
//! assignments and compound assignments), refuse out-of-range literal
//! indices, keep loop variables immutable, and bound nested and called loops
//! by one static per-invocation budget (KERNELS.md §5.2, P0-S). The GPU half
//! of the suite (the same shaders compiled and run on Metal) is in
//! platform/src/os/apple/metal_gpu_tests.rs.

use makepad_script::*;

fn test_vm() -> ScriptVm<'static> {
    let host = Box::leak(Box::new(ScriptVmHost::new(0i32, ())));
    ScriptVm { host, bx: Box::new(ScriptVmBase::new()) }
}

/// Compile `fragment_body` (plus `extra` members) as a draw shader for
/// `backend` and return the emitted functions, or `ERRORS: …`.
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
    let value = vm.with_instruction_limit(5_000_000, |vm| {
        vm.eval(ScriptMod {
            cargo_manifest_path: String::new(),
            module_path: String::new(),
            file: "shader_hostile".into(),
            line: 0,
            column: 0,
            code,
            values: vec![],
        })
    });
    vm.bx.heap.string_with(value, |_, s| s.to_string()).unwrap_or_else(|| format!("ERRORS: not a string: {value:?}"))
}

fn ok(backend: &str, extra: &str, body: &str) -> String {
    let src = compile(backend, extra, body);
    assert!(!src.starts_with("ERRORS") && !src.starts_with("unknown") && !src.starts_with("no shader"), "{backend}: {src}");
    src
}

fn fails(backend: &str, extra: &str, body: &str, needle: &str) {
    let src = compile(backend, extra, body);
    assert!(src.starts_with("ERRORS") && src.contains(needle), "{backend}: expected an error with {needle:?}, got:\n{src}");
}

const BACKENDS: [&str; 4] = ["metal", "hlsl", "glsl", "wgsl"];

#[test]
fn dynamic_array_indices_are_clamped_for_read_assign_and_compound_assign() {
    let body = "var arr = array(1f, 2f, 3f, 4f)\n\
                var i = int(self.u_n) + 1000000\n\
                let a = arr[i]\n\
                arr[i] = 5f\n\
                arr[i] += 1f\n\
                arr[i] *= 2f\n\
                self.pixel = vec4(a, arr[0], 0.0, 1.0)";
    for backend in BACKENDS {
        let src = ok(backend, "", body);
        let clamp = if backend == "wgsl" { "clamp(i32(" } else { "clamp(int(" };
        // (GLSL's source lists the functions twice.)
        let n = src.matches(&format!("{clamp}l_i), 0, 3)]")).count();
        assert!(n >= 4 && n % 4 == 0, "{backend}: every dynamic index is clamped:\n{src}");
        assert!(!src.contains("[l_i]") && !src.contains("[i]"), "{backend}: an unclamped index:\n{src}");
        // The local array is declared with the backend's array syntax.
        let decl = match backend { "metal" => "array<float, 4> l_arr", "hlsl" => "float l_arr[4]", "glsl" => "float[4] l_arr", _ => "var l_arr" };
        assert!(src.contains(decl), "{backend}:\n{src}");
        // The literal index stays as written.
        assert!(src.contains("[0]"), "{backend}:\n{src}");
    }
}

#[test]
fn unsigned_vector_and_matrix_indices_are_clamped() {
    let body = "var v = vec3(1f, 2f, 3f)\n\
                var m = mat4x4f(1.0)\n\
                let k = uint(self.u_n) + 7u\n\
                v[k] = 9f\n\
                v[k] -= 1f\n\
                let col = m[k]\n\
                self.pixel = vec4(v[k], col.x, 0.0, 1.0)";
    for backend in BACKENDS {
        let src = ok(backend, "", body);
        let (vec_clamp, mat_clamp) = if backend == "wgsl" { ("min(u32(", "3u)]") } else { ("min(uint(", "3u)]") };
        let n = src.matches(vec_clamp).count();
        assert!(n >= 4 && n % 4 == 0, "{backend}:\n{src}");
        assert!(!src.contains("[l_k]") && !src.contains("[k]"), "{backend}: an unclamped index:\n{src}");
        assert!(src.contains(", 2u)]"), "{backend}: vec3 clamps to 2:\n{src}");
        assert!(src.contains(mat_clamp), "{backend}: mat4 clamps to 3:\n{src}");
    }
}

#[test]
fn out_of_range_literal_indices_are_compile_errors() {
    for backend in BACKENDS {
        fails(backend, "", "var arr = array(1f, 2f)\nlet a = arr[2]\nself.pixel = vec4(a)", "out of bounds");
        fails(backend, "", "var v = vec4(1f)\nv[4] = 1f\nself.pixel = v", "out of bounds");
        fails(backend, "", "var v = vec2(1f)\nv[7] += 1f\nself.pixel = vec4(v.x)", "out of bounds");
    }
}

#[test]
fn loop_variables_cannot_be_reassigned() {
    fails("metal", "", "var s = 0f\nfor i in 0..4 { i = 0\n s += 1f }\nself.pixel = vec4(s)", "cannot assign");
    fails("metal", "", "var s = 0f\nfor i in 0..4 { i += 1\n s += 1f }\nself.pixel = vec4(s)", "");
}

#[test]
fn runtime_bounded_loops_charge_the_shared_invocation_budget() {
    // Every runtime loop keeps its per-loop cap and charges one body pass to
    // the invocation's counter, which every function shares.
    let src = ok("metal", "", "var s = 0f\nfor i in 0..uint(self.u_n) { s += 1f }\nself.pixel = vec4(s)");
    assert!(src.contains("> 65536u || _io._mp_iter > 4194304u){break;}"), "{src}");
    assert!(src.contains("_io._mp_iter += 1u;"), "{src}");
    // A runtime loop around a literal one charges the literal's whole cost
    // per pass (1 + 1000 x 1).
    let src = ok("metal", "", "var s = 0f\nfor i in 0..uint(self.u_n) { for j in 0..1000 { s += 1f } }\nself.pixel = vec4(s)");
    assert!(src.contains("_io._mp_iter += 1001u;"), "{src}");
    // Nested runtime loops each charge; `loop` too.
    let src = ok("metal", "", "var s = 0f\nfor i in 0..uint(self.u_n) { loop { s += 1f\n if s > 1e30 { break } } }\nself.pixel = vec4(s)");
    assert_eq!(src.matches("_io._mp_iter > 4194304u").count(), 2, "{src}");
    // The runtime bound is evaluated once, before the loop.
    assert!(src.contains("_end = uint("), "{src}");
    // No placeholder survives into the output.
    assert!(!src.contains('\u{1}'), "{src}");
    // Every GPU backend declares and charges its counter.
    for (backend, needle) in [("hlsl", "_mp_iter += 1u;"), ("glsl", "_mp_iter += 1u;"), ("wgsl", "_mp_iter = _mp_iter + 1u;")] {
        let src = ok(backend, "", "var s = 0f\nfor i in 0..uint(self.u_n) { s += 1f }\nself.pixel = vec4(s)");
        assert!(src.contains(needle) && src.contains("_mp_iter > 4194304u"), "{backend}:\n{src}");
    }
}

#[test]
fn loops_in_called_functions_charge_their_callers() {
    let extra = "inner: fn() { var s = 0f\n for j in 0..64 { s += 1f }\n return s }";
    let src = ok("metal", extra, "var s = 0f\nfor i in 0..uint(self.u_n) { s += self.inner() }\nself.pixel = vec4(s)");
    // One pass of the caller's loop: 1 + the callee's 1 + 64.
    assert!(src.contains("_io._mp_iter += 66u;"), "{src}");
    // The callee's own runtime loop charges the same shared counter.
    let extra = "inner: fn() { var s = 0f\n for j in 0..uint(self.u_n) { s += 1f }\n return s }";
    let src = ok("metal", extra, "var s = 0f\nfor i in 0..uint(self.u_n) { s += self.inner() }\nself.pixel = vec4(s)");
    assert_eq!(src.matches("_io._mp_iter > 4194304u").count(), 2, "{src}");
}

#[test]
fn literal_nesting_over_the_budget_is_refused() {
    for backend in BACKENDS {
        fails(backend, "", "var s = 0f\nfor i in 0..60000 { for j in 0..60000 { s += 1f } }\nself.pixel = vec4(s)", "loops too costly");
    }
    // Literal nesting through calls is refused too.
    let extra = "inner: fn() { var s = 0f\n for j in 0..60000 { s += 1f }\n return s }";
    fails("metal", extra, "var s = 0f\nfor i in 0..60000 { s += self.inner() }\nself.pixel = vec4(s)", "loops too costly");
    // A modest literal nest is fine and gets no guard.
    let src = ok("metal", "", "var s = 0f\nfor i in 0..64 { for j in 0..64 { s += 1f } }\nself.pixel = vec4(s)");
    assert!(!src.contains("{break;}"), "{src}");
}

#[test]
fn uint_operands_take_unsigned_literals_in_glsl_and_wgsl() {
    let extra = "bump: fn(a: u32, b: u32) -> u32 { return a + b }";
    let body = "var u = u32(self.u_n)\n\
                var c = 0.0\n\
                if u == 5 { c = 1.0 }\n\
                if 7 != u { c = 2.0 }\n\
                if u > 3 { c = 3.0 }\n\
                var w = u + 1\n\
                w = 2 * w\n\
                w += 3\n\
                w = w & 255\n\
                w = w % 4\n\
                w = 9\n\
                w = self.bump(w, 6)\n\
                self.pixel = vec4(c, float(w), 0.0, 1.0)";
    for backend in ["glsl", "wgsl"] {
        let src = ok(backend, extra, body);
        for needle in ["== 5u", "7u !=", "> 3u", "+ 1u", "2u *", "+= 3u", "& 255u", "% 4u", "= 9u", ", 6u)"] {
            if backend == "wgsl" && needle == ", 6u)" {
                continue; // WGSL converts abstract-int call arguments itself
            }
            assert!(src.contains(needle), "{backend}: missing {needle:?} in\n{src}");
        }
    }
    // an int next to an int stays unsuffixed
    let src = ok("glsl", "", "var i = int(self.u_n)\nif i == 5 { i = 6 }\nself.pixel = vec4(float(i))");
    assert!(src.contains("== 5)") && !src.contains("5u"), "{src}");
}

#[test]
fn uint_literals_in_returns_and_index_assignments() {
    let extra = "pick: fn(a: u32) -> u32 { if a > 3 { return a }\nreturn 0 }";
    let body = "var u = u32(self.u_n)\n\
                var v = vec2u(1u, 2u)\n\
                v[1] = 7\n\
                var arr = array(1u, 2u)\n\
                arr[0] = 4\n\
                u = self.pick(u)\n\
                self.pixel = vec4(float(u), float(v.y), float(arr[0]), 1.0)";
    for backend in ["glsl", "wgsl"] {
        let src = ok(backend, extra, body);
        for needle in ["return 0u", "[1] = 7u", "[0] = 4u"] {
            assert!(src.contains(needle) || src.contains(&needle.replace("[1]", "[1u]")), "{backend}: missing {needle:?} in\n{src}");
        }
    }
}

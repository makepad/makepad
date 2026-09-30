//! Regression tests for the language gaps found while authoring the
//! reference-film plates (FP1, 2026-09-30).

use makepad_script::*;

fn test_vm() -> ScriptVm<'static> {
    let host = Box::leak(Box::new(ScriptVmHost::new(0i32, ())));
    ScriptVm { host, bx: Box::new(ScriptVmBase::new()) }
}

fn run(name: &str, code: &str) -> (ScriptValue, Vec<String>, ScriptVm<'static>) {
    let mut vm = test_vm();
    vm.bx.captured_errors = Some(Vec::new());
    let value = vm.with_instruction_limit(500_000, |vm| {
        vm.eval(ScriptMod {
            cargo_manifest_path: String::new(),
            module_path: String::new(),
            file: format!("fp1_{name}"),
            line: 0,
            column: 0,
            code: code.to_string(),
            values: vec![],
        })
    });
    let mut errs: Vec<String> = vm.bx.captured_errors.take().unwrap().into_iter().map(|e| format!("{e:?}")).collect();
    errs.extend(vm.take_errors().into_iter().map(|e| format!("{e:?}")));
    (value, errs, vm)
}

fn number(name: &str, code: &str) -> f64 {
    let (value, errs, _vm) = run(name, code);
    assert!(errs.is_empty(), "{name}: eval errors: {errs:?}");
    value.as_number().unwrap_or_else(|| panic!("{name}: not a number: {value:?}"))
}

fn fails(name: &str, code: &str) -> Vec<String> {
    let (_value, errs, _vm) = run(name, code);
    assert!(!errs.is_empty(), "{name}: expected an error");
    errs
}

const PRELUDE: &str = "use mod.math.*\nuse mod.pod.*\n";

/// A binary op folds a constant right operand into its immediate. The fold
/// took the last constant of an if/match arm or a `||` operand too, and
/// popping it moved their jump targets: `var z = 1.0 * (if v { 0.2 } else
/// { 1.0 })` skipped the declaration when the true arm ran.
#[test]
fn an_op_after_an_if_ending_in_a_whole_float() {
    let f = |expr: &str, arg: &str| format!("let v = 2\nlet f = fn(c) {{\n    var z = {expr}\n    return z\n}}\nf({arg})");
    for (name, expr, t, e) in [
        ("fp1", "1.0 * (if v == 2 { 0.2 } else { 1.0 })", 0.2, 0.2),
        ("param", "1.0 * (if c { 0.2 } else { 1.0 })", 0.2, 1.0),
        ("mul", "3 * (if c { 0.5 } else { 2.0 })", 1.5, 6.0),
        ("add", "1 + (if c { 2 } else { 3.0 })", 3.0, 4.0),
        ("sub", "10 - (if c { 2.5 } else { 4.0 })", 7.5, 6.0),
        ("nested", "2 * (1 + (if c { 1 } else { 3.0 }))", 4.0, 8.0),
        ("elif", "2 * (if c { 1 } elif v == 3 { 5 } else { 3.0 })", 2.0, 6.0),
        ("match", "2 * (match c { true => 1.5 _ => 3.0 })", 3.0, 6.0),
        ("or", "2 * (c || 3.0)", 2.0, 6.0),
        ("nil_or", "2 * ((if c { 1.5 } else { nil }) |? 3.0)", 3.0, 6.0),
        ("cam", "1.0 * (1.0 + 0.5 * 2.0 * (if c { 0.2 } else { 1.0 }))", 1.2, 2.0),
    ] {
        assert_eq!(number(name, &f(expr, "true")), t, "{name} (true)");
        assert_eq!(number(name, &f(expr, "false")), e, "{name} (false)");
        let with_let = f(expr, "true").replace("var z", "let z");
        assert_eq!(number(name, &with_let), t, "{name} (let)");
    }
    // Constants still fold where they are the whole operand.
    assert_eq!(number("plain", "let a = 7\na * 2.0 + a - 3.0"), 18.0);
}

/// `int(x)` (and `i32`, `u32`, `f32`, `f16`) make a scalar pod: it indexes
/// arrays and does arithmetic as the number it holds.
#[test]
fn int_values_index_arrays() {
    let a = format!("{PRELUDE}let a = [5, 6, 7]\n");
    assert_eq!(number("int_lit", &format!("{a}a[int(1.7)]")), 6.0);
    assert_eq!(number("int_var", &format!("{a}let i = int(2)\na[i]")), 7.0);
    assert_eq!(number("i32", &format!("{a}a[i32(0)]")), 5.0);
    assert_eq!(number("u32", &format!("{a}a[u32(2)]")), 7.0);
    assert_eq!(number("f32", &format!("{a}a[f32(1.0)]")), 6.0);
    assert_eq!(number("sum", &format!("{a}a[int(0.9) + 1]")), 6.0);
    assert_eq!(number("arith", &format!("{PRELUDE}int(2.9) * 1.5 + u32(1)")), 4.0);
    assert_eq!(number("cmp", &format!("{PRELUDE}if int(2.5) < 3 {{ 1 }} else {{ 0 }}")), 1.0);
    assert_eq!(number("loop", &format!("{PRELUDE}let a = [1, 2, 3]\nvar s = 0\nfor k in 0..3 {{ s += a[int(k)] }}\ns")), 6.0);
    assert_eq!(number("assign", &format!("{a}a[int(1)] = 9\na[1]")), 9.0);
    // Still only whole, non-negative, in-range indices.
    fails("negative", &format!("{a}a[int(-1)]"));
    fails("fraction", &format!("{a}a[f32(0.5)]"));
    // `float(1.0)` stays a pod, which shader declarations take their type from.
    let (value, errs, _vm) = run("pod", &format!("{PRELUDE}float(1.0)"));
    assert!(errs.is_empty() && value.as_pod().is_some(), "{value:?} {errs:?}");
}

/// A call of `mod` is the modulo of shading languages; `mod` itself stays
/// the module root.
#[test]
fn mod_call_is_modulo_and_mod_stays_the_modules() {
    assert_eq!(number("mod", "mod(7, 3)"), 1.0);
    assert_eq!(number("mod_neg", "mod(-1, 3)"), 2.0);
    assert_eq!(number("mod_frac", "mod(5.5, 2)"), 1.5);
    assert_eq!(number("mod_math", &format!("{PRELUDE}mod(7.5, 2.0) + floor(0.5)")), 1.5);
    assert_eq!(number("mod_in_fn", &format!("{PRELUDE}let f = fn(t) {{ mod(t, 2.0) }}\nf(5.5)")), 1.5);
    assert_eq!(number("mod_vec", &format!("{PRELUDE}let v = mod(vec2(5, -1), vec2(3, 3))\nv.x * 10 + v.y")), 22.0);
    // mod.x and use mod.x.* mean what they did.
    assert_eq!(number("mod_field", "let m = mod.math\nm.floor(2.5)"), 2.0);
    assert_eq!(number("mod_path", "mod.math.floor(3.5)"), 3.0);
    assert_eq!(number("mod_use", "use mod.math.*\nfloor(4.5)"), 4.0);
    assert_eq!(number("modf", &format!("{PRELUDE}modf(-1, 3)")), -1.0);
    // A local named mod is that local.
    assert_eq!(number("mod_shadow", "let mod = fn(a, b) { a + b }\nmod(1, 2)"), 3.0);
}

/// `a.b ?? d`: an optional field read (and `a[i] ?? d`, `a[i].b ?? d`). `??` was a parse error before
/// ("Parser stuck on character Operator(?)") in every spelling, so no
/// program that parsed changes meaning. It is `|?` (nil-or) whose left
/// operand's trailing field reads are quiet: a missing field, or a field of
/// nil, is nil without an error.
#[test]
fn optional_field_read() {
    let o = "let o = {a: 1 n: nil f: false s: {c: 2}}\n";
    for (name, expr, want) in [
        ("missing", "o.b ?? 7", 7.0),
        ("present", "o.a ?? 7", 1.0),
        ("nil_field", "o.n ?? 5", 5.0),
        ("chain_missing", "o.x.c ?? 9", 9.0),
        ("chain_present", "o.s.c ?? 9", 2.0),
        ("chain_tail_missing", "o.s.d ?? 9", 9.0),
        ("then", "o.b ?? o.x ?? 8", 8.0),
        ("precedence", "o.b ?? 3 + 4", 7.0),
        ("in_fn", "let f = fn(x) { x.b ?? x.a }\nf(o)", 1.0),
        ("plain", "let a = 3\na ?? 4", 3.0),
        ("pod", "use mod.pod.*\nlet v = vec2(3, 4)\nv.y ?? 0", 4.0),
        ("nospace", "o.b??6", 6.0),
        // The index read before the trailing fields is optional too.
        ("index_out", "let a = [1, 2]\na[5] ?? 7", 7.0),
        ("index_in", "let a = [1, 2]\na[1] ?? 7", 2.0),
        ("index_field", "let w = [{s: [{start: 1.5}]}]\nw[0].s[1].start ?? 9", 9.0),
        ("index_field_in", "let w = [{s: [{start: 1.5}]}]\nw[0].s[0].start ?? 9", 1.5),
        ("index_nil", "let a = nil\na[0] ?? 4", 4.0),
        ("index_map", "let m = {k: 3}\nm[@x] ?? 5", 5.0),
    ] {
        assert_eq!(number(name, &format!("{o}{expr}")), want, "{name}");
    }
    // false is a value, not absent.
    let (value, errs, _vm) = run("false", &format!("{o}o.f ?? 5"));
    assert!(errs.is_empty() && value.as_bool() == Some(false), "{value:?} {errs:?}");
    // Without `??` a missing field is still an error.
    fails("plain_missing", &format!("{o}o.b"));
    fails("nil_or_missing", &format!("{o}o.b |? 7"));
    fails("plain_index", "let a = [1]\na[3]");
}

/// `ln(x)` is `log(x)`, the natural log, in script code; in shaders it
/// compiles to each backend's `log` (tests/if_for_else.rs's harness).
#[test]
fn ln_is_the_natural_log() {
    assert!((number("ln", &format!("{PRELUDE}ln(2.718281828)")) - 1.0).abs() < 1e-5);
    assert!((number("log", &format!("{PRELUDE}log(2.718281828)")) - 1.0).abs() < 1e-5);
    assert!((number("ln_vec", &format!("{PRELUDE}ln(vec2(1.0, 2.718281828)).y")) - 1.0).abs() < 1e-5);
}

mod shader_ln {
    use makepad_script::makepad_math::*;
    use makepad_script::traits::*;
    use makepad_script::*;

    #[derive(Script, ScriptHook)]
    #[repr(C)]
    pub struct ShaderLnTest {
        #[live]
        pub tint: Vec4f,
    }

    const SHADER: &str = r#"
        use mod.shader
        use mod.pod.*
        use mod.math.*

        let sh = #(0){
            vertex_pos: shader.vertex_position(vec4f)
            fb0: shader.fragment_output(0, vec4f)
            vertex: fn() {
                return vec4(0.0, 0.0, 0.0, 1.0)
            }
            fragment: fn() {
                let x = max(self.tint.x, 0.5)
                self.fb0 = vec4(ln(x), log(x), ln(vec2(x, x)).y, 1.0)
            }
        }
    "#;

    #[test]
    fn ln_compiles_to_log_on_every_backend() {
        let host = Box::leak(Box::new(ScriptVmHost::new(0i32, ())));
        let mut vm = ScriptVm { host, bx: Box::new(ScriptVmBase::new()) };
        for backend in ["metal", "hlsl", "glsl", "wgsl"] {
            let shader_obj = ShaderLnTest::script_shader(&mut vm);
            let value = vm.eval(ScriptMod {
                cargo_manifest_path: env!("CARGO_MANIFEST_DIR").to_string(),
                module_path: "shader_ln".to_string(),
                file: "shader_ln.rs".to_string(),
                line: 1,
                column: 1,
                code: format!("{SHADER}\n        shader.test_compile_draw_source(sh, \"{backend}\", false)"),
                values: vec![shader_obj],
            });
            assert!(!value.is_err(), "{backend}: script errored: {value:?}");
            let source = vm.bx.heap.string_with(value, |_heap, s| s.to_string()).unwrap_or_default();
            assert!(!source.is_empty() && !source.starts_with("ERRORS"), "{backend}: {source}");
            assert!(!source.contains("ln("), "{backend}: ln reached the backend:\n{source}");
        }
    }
}

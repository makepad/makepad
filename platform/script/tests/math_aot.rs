//! Math-AOT test suite.
//!
//! Layer map (each test names its layer):
//! - translate: one accepted splash form -> correct compiled result,
//!   bit-identical to the interpreter; one rejected form -> clean `None`.
//! - slots: splash `let` locals / params mapped to wasm locals.
//! - batch: the eval_batch entry and its edges.
//! - fuzz: differential fuzzing interpreter-vs-AOT over random expressions.

use makepad_script::math_aot::{MathAot, MathAotParam, MathAotValue};
use makepad_script::makepad_math::{Vec2f, Vec3f, Vec4f};
use makepad_script::*;

fn test_vm() -> ScriptVm<'static> {
    let host = Box::leak(Box::new(ScriptVmHost::new(0i32, ())));
    ScriptVm {
        host,
        bx: Box::new(ScriptVmBase::new()),
    }
}

/// Evaluates `code` (which must end with an expression yielding a fn) and
/// returns the fn value.
fn eval_fn(vm: &mut ScriptVm, code: &str) -> ScriptValue {
    vm.bx.captured_errors = Some(Vec::new());
    let value = vm.eval(ScriptMod {
        cargo_manifest_path: String::new(),
        module_path: String::new(),
        file: "math_aot_test".to_string(),
        line: 0,
        column: 0,
        code: code.to_string(),
        values: vec![],
    });
    let errors = vm.take_errors();
    assert!(errors.is_empty(), "script errors: {errors:?}");
    assert!(value.as_object().is_some(), "script did not yield a fn: {code}");
    value
}

fn to_script_arg(vm: &mut ScriptVm, arg: &MathAotValue) -> ScriptValue {
    match arg {
        MathAotValue::Scalar(v) => (*v).into(),
        MathAotValue::Vec2(v) => Vec2f { x: v[0], y: v[1] }.script_to_value(vm),
        MathAotValue::Vec3(v) => Vec3f {
            x: v[0],
            y: v[1],
            z: v[2],
        }
        .script_to_value(vm),
        MathAotValue::Vec4(v) => Vec4f {
            x: v[0],
            y: v[1],
            z: v[2],
            w: v[3],
        }
        .script_to_value(vm),
    }
}

/// Compiles `code`'s fn and checks AOT-vs-interpreter bit identity for
/// every argument tuple.
fn check_bit_identical(
    code: &str,
    params: &[MathAotParam],
    arg_sets: &[Vec<MathAotValue>],
) {
    let mut vm = test_vm();
    let fn_value = eval_fn(&mut vm, code);
    let aot = MathAot::new(&mut vm);
    let mut compiled = aot
        .compile(&vm, fn_value, params, &[])
        .unwrap_or_else(|| panic!("expression rejected by the AOT: {code}"));
    for args in arg_sets {
        let script_args: Vec<ScriptValue> =
            args.iter().map(|a| to_script_arg(&mut vm, a)).collect();
        let expected = vm.call(fn_value, &script_args);
        let expected = expected
            .as_number()
            .unwrap_or_else(|| panic!("interpreter returned non-number for {code}: {expected:?}"));
        let actual = compiled.call(args).expect("aot call failed");
        // Bit-identical, except at the NaN boundary: the interpreter boxes
        // NaN RESULTS as traced NaNs (payload = source location), so any
        // NaN output compares as NaN-vs-NaN.
        let same = if actual.is_nan() && expected.is_nan() {
            true
        } else {
            actual.to_bits() == expected.to_bits()
        };
        assert!(
            same,
            "MISMATCH for {code}\n  args {args:?}\n  interp {expected:?} ({:#x}) aot {actual:?} ({:#x})",
            expected.to_bits(),
            actual.to_bits()
        );
    }
}

/// Asserts the AOT cleanly rejects `code` (returns None, no panic).
fn check_rejected(code: &str, params: &[MathAotParam]) {
    let mut vm = test_vm();
    let fn_value = eval_fn(&mut vm, code);
    let aot = MathAot::new(&mut vm);
    assert!(
        aot.compile(&vm, fn_value, params, &[]).is_none(),
        "expected rejection: {code}"
    );
}

fn scalar_args(sets: &[&[f64]]) -> Vec<Vec<MathAotValue>> {
    sets.iter()
        .map(|set| set.iter().map(|v| MathAotValue::Scalar(*v)).collect())
        .collect()
}

const XS: &[f64] = &[
    0.0, 1.0, -1.0, 0.5, -0.75, 2.5, 3.14159, -7.25, 100.5, 1.0e10, -0.0,
];

fn one_scalar_sets() -> Vec<Vec<MathAotValue>> {
    XS.iter().map(|x| vec![MathAotValue::Scalar(*x)]).collect()
}

fn two_scalar_sets() -> Vec<Vec<MathAotValue>> {
    let mut out = Vec::new();
    for a in XS {
        for b in XS {
            out.push(vec![MathAotValue::Scalar(*a), MathAotValue::Scalar(*b)]);
        }
    }
    out
}

// -- layer: translate (accepted forms) ------------------------------------

#[test]
fn translate_scalar_arithmetic() {
    let s2 = two_scalar_sets();
    check_bit_identical("let f = |a, b| a + b\n(f)", &[MathAotParam::Scalar; 2], &s2);
    check_bit_identical("let f = |a, b| a - b\n(f)", &[MathAotParam::Scalar; 2], &s2);
    check_bit_identical("let f = |a, b| a * b\n(f)", &[MathAotParam::Scalar; 2], &s2);
    check_bit_identical("let f = |a, b| a / b\n(f)", &[MathAotParam::Scalar; 2], &s2);
    check_bit_identical("let f = |a, b| a % b\n(f)", &[MathAotParam::Scalar; 2], &s2);
    check_bit_identical("let f = |a| -a\n(f)", &[MathAotParam::Scalar], &one_scalar_sets());
    // Inline-constant fast path (the parser fuses small integer RHS).
    check_bit_identical("let f = |a| a * 3\n(f)", &[MathAotParam::Scalar], &one_scalar_sets());
    check_bit_identical("let f = |a| a + 7\n(f)", &[MathAotParam::Scalar], &one_scalar_sets());
}

#[test]
fn translate_scalar_intrinsics() {
    let s1 = one_scalar_sets();
    for name in [
        "sin", "cos", "tan", "asin", "acos", "atan", "exp", "log", "sqrt", "abs", "floor",
        "ceil", "fract",
    ] {
        let code = format!("use mod.math.*\nlet f = |a| {name}(a)\n(f)");
        check_bit_identical(&code, &[MathAotParam::Scalar], &s1);
    }
    let s2 = two_scalar_sets();
    for name in ["atan2", "pow", "min", "max"] {
        let code = format!("use mod.math.*\nlet f = |a, b| {name}(a, b)\n(f)");
        check_bit_identical(&code, &[MathAotParam::Scalar; 2], &s2);
    }
}

#[test]
fn translate_scalar_composite() {
    check_bit_identical(
        "use mod.math.*\nlet f = |x, y, z| sin(x) * cos(z) - y + 0.3 * sin(5 * x) * sin(5 * y) * sin(5 * z)\n(f)",
        &[MathAotParam::Scalar; 3],
        &scalar_args(&[
            &[0.1, 0.2, 0.3],
            &[1.5, -2.5, 3.5],
            &[0.0, 0.0, 0.0],
            &[-10.25, 5.125, 0.75],
        ]),
    );
}

#[test]
fn translate_comparisons_and_if() {
    let s2 = two_scalar_sets();
    check_bit_identical(
        "let f = |a, b| if a < b { a } else { b }\n(f)",
        &[MathAotParam::Scalar; 2],
        &s2,
    );
    check_bit_identical(
        "let f = |a, b| if a >= b { a * 2 } else { b - 1 }\n(f)",
        &[MathAotParam::Scalar; 2],
        &s2,
    );
}

#[test]
fn translate_early_return() {
    check_bit_identical(
        "let f = |a| { if a < 0 { return 0 - a }\na * 2 }\n(f)",
        &[MathAotParam::Scalar],
        &one_scalar_sets(),
    );
}

#[test]
fn translate_logic_ops() {
    let s2 = two_scalar_sets();
    check_bit_identical(
        "let f = |a, b| a && b\n(f)",
        &[MathAotParam::Scalar; 2],
        &s2,
    );
    check_bit_identical(
        "let f = |a, b| a || b\n(f)",
        &[MathAotParam::Scalar; 2],
        &s2,
    );
}

#[test]
fn translate_scope_constant() {
    check_bit_identical(
        "let r = 1.25\nlet f = |a| a - r\n(f)",
        &[MathAotParam::Scalar],
        &one_scalar_sets(),
    );
    // Module constant through the scope chain.
    check_bit_identical(
        "use mod.math.*\nlet f = |a| a * PI\n(f)",
        &[MathAotParam::Scalar],
        &one_scalar_sets(),
    );
}

// -- layer: slots ---------------------------------------------------------

#[test]
fn slots_let_locals() {
    check_bit_identical(
        "use mod.math.*\nlet f = |x, y| {\nlet a = x * 2\nlet b = sin(a) + y\nb * a\n}\n(f)",
        &[MathAotParam::Scalar; 2],
        &two_scalar_sets(),
    );
}

#[test]
fn slots_compound_assign() {
    check_bit_identical(
        "let f = |x| {\nlet a = x\na += 2\na *= 3\na -= x\na /= 2\na\n}\n(f)",
        &[MathAotParam::Scalar],
        &one_scalar_sets(),
    );
}

// -- layer: translate (vectors) -------------------------------------------

fn vec3_sets() -> Vec<Vec<MathAotValue>> {
    [
        [0.0f32, 0.0, 0.0],
        [1.0, 2.0, 3.0],
        [-1.5, 0.25, -8.0],
        [0.1, -0.2, 0.3],
    ]
    .iter()
    .map(|v| vec![MathAotValue::Vec3(*v)])
    .collect()
}

#[test]
fn translate_vec3_length_sphere() {
    check_bit_identical(
        "use mod.math.*\nlet f = |p| length(p) - 1.0\n(f)",
        &[MathAotParam::Vec3],
        &vec3_sets(),
    );
}

#[test]
fn translate_vec_arithmetic_and_swizzle() {
    check_bit_identical(
        "let f = |p| (p * 2.0 + p).x\n(f)",
        &[MathAotParam::Vec3],
        &vec3_sets(),
    );
    check_bit_identical(
        "let f = |p| p.z * p.y + p.x\n(f)",
        &[MathAotParam::Vec3],
        &vec3_sets(),
    );
    check_bit_identical(
        "use mod.math.*\nlet f = |p| length(p.zyx - p.xxz)\n(f)",
        &[MathAotParam::Vec3],
        &vec3_sets(),
    );
    // Vector division has the interpreter's zero-divisor guard.
    check_bit_identical(
        "let f = |p| (p / p.yzx).x\n(f)",
        &[MathAotParam::Vec3],
        &vec3_sets(),
    );
}

#[test]
fn translate_vec_constructor() {
    check_bit_identical(
        "use mod.math.*\nuse mod.pod.*\nlet f = |x, y, z| length(vec3(x, y, z))\n(f)",
        &[MathAotParam::Scalar; 3],
        &scalar_args(&[&[1.0, 2.0, 3.0], &[0.0, 0.0, 0.0], &[-4.5, 0.5, 9.0]]),
    );
}

#[test]
fn translate_vec_intrinsics() {
    let sets: Vec<Vec<MathAotValue>> = [
        ([1.0f32, 2.0, 3.0], [4.0f32, -5.0, 6.0]),
        ([0.0, 0.0, 0.0], [1.0, 1.0, 1.0]),
        ([-0.5, 0.25, -0.125], [8.0, -16.0, 32.0]),
    ]
    .iter()
    .map(|(a, b)| vec![MathAotValue::Vec3(*a), MathAotValue::Vec3(*b)])
    .collect();
    check_bit_identical(
        "use mod.math.*\nlet f = |a, b| dot(a, b)\n(f)",
        &[MathAotParam::Vec3; 2],
        &sets,
    );
    check_bit_identical(
        "use mod.math.*\nlet f = |a, b| distance(a, b)\n(f)",
        &[MathAotParam::Vec3; 2],
        &sets,
    );
    check_bit_identical(
        "use mod.math.*\nlet f = |a, b| length(cross(a, b))\n(f)",
        &[MathAotParam::Vec3; 2],
        &sets,
    );
    check_bit_identical(
        "use mod.math.*\nlet f = |a, b| length(normalize(a) + normalize(b))\n(f)",
        &[MathAotParam::Vec3; 2],
        &sets,
    );
    check_bit_identical(
        "use mod.math.*\nlet f = |a, b| length(min(a, b) - max(a, b))\n(f)",
        &[MathAotParam::Vec3; 2],
        &sets,
    );
    check_bit_identical(
        "use mod.math.*\nlet f = |a, b| length(mix(a, b, 0.25))\n(f)",
        &[MathAotParam::Vec3; 2],
        &sets,
    );
    check_bit_identical(
        "use mod.math.*\nlet f = |a, b| length(abs(a) - floor(b))\n(f)",
        &[MathAotParam::Vec3; 2],
        &sets,
    );
    check_bit_identical(
        "use mod.math.*\nlet f = |a, b| length(sin(a) + cos(b))\n(f)",
        &[MathAotParam::Vec3; 2],
        &sets,
    );
    check_bit_identical(
        "use mod.math.*\nlet f = |a, b| length(clamp(a, 0.0 - 1.0, 1.0))\n(f)",
        &[MathAotParam::Vec3; 2],
        &sets,
    );
    // Pod methods.
    check_bit_identical(
        "let f = |a, b| a.dot(b)\n(f)",
        &[MathAotParam::Vec3; 2],
        &sets,
    );
    check_bit_identical(
        "let f = |a, b| a.cross(b).length()\n(f)",
        &[MathAotParam::Vec3; 2],
        &sets,
    );
    check_bit_identical(
        "let f = |a, b| a.normalized().dot(b)\n(f)",
        &[MathAotParam::Vec3; 2],
        &sets,
    );
}

// -- layer: translate (rejected forms) ------------------------------------

#[test]
fn rejects_outside_subset() {
    // Object literal.
    check_rejected("let f = |a| {x: a}\n(f)", &[MathAotParam::Scalar]);
    // Array literal.
    check_rejected("let f = |a| [a]\n(f)", &[MathAotParam::Scalar]);
    // String.
    check_rejected("let f = |a| \"s\"\n(f)", &[MathAotParam::Scalar]);
    // Loop.
    check_rejected(
        "let f = |a| { let t = 0\nfor i in 0..3 { t += a }\nt }\n(f)",
        &[MathAotParam::Scalar],
    );
    // Closure creation inside.
    check_rejected("let f = |a| { let g = |b| b\ng(a) }\n(f)", &[MathAotParam::Scalar]);
    // Unknown free identifier.
    check_rejected("let f = |a| a + undefined_thing\n(f)", &[MathAotParam::Scalar]);
    // Calling a non-math native.
    check_rejected(
        "let f = |a| { log(a)\na }\n(f)",
        &[MathAotParam::Scalar],
    );
    // `if` without else in value position (nil on the untaken path).
    check_rejected("let f = |a| if a > 0 { a }\n(f)", &[MathAotParam::Scalar]);
    // Param count mismatch.
    check_rejected("let f = |a, b| a + b\n(f)", &[MathAotParam::Scalar]);
    // Equality: splash deep_eq compares raw NaN-box bits (traced NaNs),
    // which the compiled form cannot mirror for data-dependent NaNs.
    check_rejected(
        "let f = |a, b| if a == b { 1.0 } else { 0.0 }\n(f)",
        &[MathAotParam::Scalar; 2],
    );
}

// -- layer: batch ----------------------------------------------------------

#[test]
fn batch_matches_single_calls() {
    let mut vm = test_vm();
    let fn_value = eval_fn(
        &mut vm,
        "use mod.math.*\nlet f = |p| length(p) - 1.0\n(f)",
    );
    let aot = MathAot::new(&mut vm);
    let mut compiled = aot.compile(&vm, fn_value, &[MathAotParam::Vec3], &[]).unwrap();
    // 10_001 points: not a multiple of the chunk size, crosses a chunk
    // boundary, exercises reuse of the same instance across chunks.
    let n = 10_001;
    let mut input = Vec::with_capacity(n * 3);
    for i in 0..n {
        input.push((i as f32) * 0.01 - 37.0);
        input.push((i as f32) * -0.003 + 1.0);
        input.push((i as f32) * 0.02 - 100.0);
    }
    let mut out = vec![0f32; n];
    compiled.eval_batch(&input, &[], &mut out);
    for i in (0..n).step_by(997) {
        let expected = compiled
            .call(&[MathAotValue::Vec3([
                input[i * 3],
                input[i * 3 + 1],
                input[i * 3 + 2],
            ])])
            .unwrap() as f32;
        assert_eq!(out[i].to_bits(), expected.to_bits(), "point {i}");
    }
}

#[test]
fn batch_edges() {
    let mut vm = test_vm();
    let fn_value = eval_fn(&mut vm, "use mod.math.*\nlet f = |p| length(p)\n(f)");
    let aot = MathAot::new(&mut vm);
    let mut compiled = aot.compile(&vm, fn_value, &[MathAotParam::Vec3], &[]).unwrap();
    // N = 0
    compiled.eval_batch(&[], &[], &mut []);
    // N = 1
    let mut out = [0f32];
    compiled.eval_batch(&[3.0, 4.0, 12.0], &[], &mut out);
    assert_eq!(out[0], 13.0);
    // Exactly one chunk, then chunk+1.
    for n in [4096usize, 4097] {
        let input: Vec<f32> = (0..n * 3).map(|i| (i % 17) as f32 - 8.0).collect();
        let mut out = vec![0f32; n];
        compiled.eval_batch(&input, &[], &mut out);
        let expected = compiled
            .call(&[MathAotValue::Vec3([
                input[(n - 1) * 3],
                input[(n - 1) * 3 + 1],
                input[(n - 1) * 3 + 2],
            ])])
            .unwrap() as f32;
        assert_eq!(out[n - 1].to_bits(), expected.to_bits(), "n={n}");
    }
    // Two compiled expressions interleaved on one MathAot.
    let fn2 = eval_fn(&mut vm, "use mod.math.*\nlet g = |p| p.x + p.y + p.z\n(g)");
    let mut compiled2 = aot.compile(&vm, fn2, &[MathAotParam::Vec3], &[]).unwrap();
    let mut out1 = [0f32];
    let mut out2 = [0f32];
    compiled.eval_batch(&[1.0, 2.0, 2.0], &[], &mut out1);
    compiled2.eval_batch(&[1.0, 2.0, 2.0], &[], &mut out2);
    compiled.eval_batch(&[3.0, 4.0, 12.0], &[], &mut out1);
    assert_eq!(out1[0], 13.0);
    assert_eq!(out2[0], 5.0);
}

#[test]
fn batch_scalar_params() {
    let mut vm = test_vm();
    let fn_value = eval_fn(
        &mut vm,
        "use mod.math.*\nlet f = |x, y, z| sin(x) * cos(z) - y\n(f)",
    );
    let aot = MathAot::new(&mut vm);
    let mut compiled = aot
        .compile(&vm, fn_value, &[MathAotParam::Scalar; 3], &[])
        .unwrap();
    let n = 100;
    let input: Vec<f32> = (0..n * 3).map(|i| (i as f32) * 0.05 - 3.0).collect();
    let mut out = vec![0f32; n];
    compiled.eval_batch(&input, &[], &mut out);
    for i in 0..n {
        let expected = compiled
            .call(&[
                MathAotValue::Scalar(input[i * 3] as f64),
                MathAotValue::Scalar(input[i * 3 + 1] as f64),
                MathAotValue::Scalar(input[i * 3 + 2] as f64),
            ])
            .unwrap() as f32;
        assert_eq!(out[i].to_bits(), expected.to_bits(), "point {i}");
    }
}


// -- layer: uniforms (parametric models) -----------------------------------

/// A parametric sphere: `|p, r| length(p) - r` with `r` a uniform.
/// Changing `r` between calls on ONE compiled function must match the
/// interpreter with the same values — no recompile.
#[test]
fn uniforms_parametric_sphere() {
    let mut vm = test_vm();
    let fn_value = eval_fn(&mut vm, "use mod.math.*\nlet f = |p, r| length(p) - r\n(f)");
    let aot = MathAot::new(&mut vm);
    let mut compiled = aot
        .compile(&vm, fn_value, &[MathAotParam::Vec3], &[MathAotParam::Scalar])
        .expect("parametric sphere in subset");
    for r in [1.0f32, 2.5, 0.25] {
        let p = [3.0f32, 0.0, 4.0];
        let arg_p = Vec3f { x: p[0], y: p[1], z: p[2] }.script_to_value(&mut vm);
        let expected = vm.call(fn_value, &[arg_p, (r as f64).into()]);
        let expected = expected.as_number().unwrap();
        let actual = compiled
            .call(&[MathAotValue::Vec3(p), MathAotValue::Scalar(r as f64)])
            .unwrap();
        assert_eq!(actual.to_bits(), expected.to_bits(), "r={r}");
        // Batch entry with the uniform block.
        let mut out = [0f32; 2];
        compiled.eval_batch(&[3.0, 0.0, 4.0, 0.0, 0.0, 0.0], &[r], &mut out);
        assert_eq!(out[0], 5.0 - r);
        assert_eq!(out[1], -r);
    }
}

/// The round-shapes idiom: polynomial smooth-min of two spheres with the
/// blend radius `k` and the sphere offset `c` as uniforms (vec uniform +
/// scalar uniform); resampled with several k values on one compiled fn.
#[test]
fn uniforms_smooth_min_blend() {
    let mut vm = test_vm();
    let code = "use mod.math.*\nuse mod.pod.*\nlet f = |p, c, k| {\n\
        let a = length(p - c) - 0.6\n\
        let b = length(p + c) - 0.6\n\
        let h = clamp(0.5 + 0.5 * (b - a) / k, 0.0, 1.0)\n\
        mix(b, a, h) - k * h * (1.0 - h)\n}\n(f)";
    let fn_value = eval_fn(&mut vm, code);
    let aot = MathAot::new(&mut vm);
    let mut compiled = aot
        .compile(
            &vm,
            fn_value,
            &[MathAotParam::Vec3],
            &[MathAotParam::Vec3, MathAotParam::Scalar],
        )
        .expect("smooth-min in subset");
    let c = [0.4f32, 0.0, 0.0];
    for k in [0.1f32, 0.3, 0.7] {
        for p in [[0.0f32, 0.3, 0.1], [0.5, -0.2, 0.4], [-0.8, 0.0, 0.0]] {
            let arg_p = Vec3f { x: p[0], y: p[1], z: p[2] }.script_to_value(&mut vm);
            let arg_c = Vec3f { x: c[0], y: c[1], z: c[2] }.script_to_value(&mut vm);
            let expected = vm.call(fn_value, &[arg_p, arg_c, (k as f64).into()]);
            let expected = expected.as_number().unwrap();
            let actual = compiled
                .call(&[
                    MathAotValue::Vec3(p),
                    MathAotValue::Vec3(c),
                    MathAotValue::Scalar(k as f64),
                ])
                .unwrap();
            assert!(
                (actual.is_nan() && expected.is_nan()) || actual.to_bits() == expected.to_bits(),
                "k={k} p={p:?}: interp {expected:?} aot {actual:?}"
            );
            // Batch with the uniform block [cx, cy, cz, k].
            let mut out = [0f32];
            compiled.eval_batch(&p, &[c[0], c[1], c[2], k], &mut out);
            assert_eq!(out[0].to_bits(), (actual as f32).to_bits());
        }
    }
}
